use cdp_client::cdp::Cdp;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, Write};
#[cfg(not(target_os = "wasi"))]
use std::io::BufRead;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const BUSY: Duration = Duration::from_millis(5);
const IDLE: Duration = Duration::from_millis(50);
const SUPERSEDED: &str = "bridge evaluation navigated or superseded";

struct Document {
    frame: String,
    loader: String,
    context: String,
}

struct Pending {
    key: String,
    session: String,
    document: Document,
    from: usize,
    started: Instant,
}

fn diagnostic(id: u64, phase: &str, document: Option<&Document>, message: Option<&str>) {
    emit(json!({"diagnostic":{"id":id,"phase":phase,"frameId":document.map(|doc| &doc.frame),"loaderId":document.map(|doc| &doc.loader),"uniqueContextId":document.map(|doc| &doc.context),"error":message.map(|message| json!({"message":message}))}}));
}

fn frame_identity(tree: &Value, frame_id: Option<&str>) -> Option<(String, String)> {
    fn find<'a>(node: &'a Value, id: Option<&str>) -> Option<&'a Value> {
        let frame = node.get("frame")?;
        if id.is_none_or(|id| frame.get("id").and_then(Value::as_str) == Some(id)) { return Some(frame) }
        node.get("childFrames")?.as_array()?.iter().find_map(|child| find(child, id))
    }
    let frame = find(tree.get("frameTree")?, frame_id)?;
    let id = frame.get("id")?.as_str().filter(|id| !matches!(*id, "" | "null" | "://"))?;
    let loader = frame.get("loaderId")?.as_str().filter(|loader| !matches!(*loader, "" | "null" | "://"))?;
    Some((id.to_string(), loader.to_string()))
}

fn bind_document(cdp: &mut Cdp, session: &str, params: &Value) -> Option<Document> {
    cdp.call_for("Runtime.enable", json!({}), Some(session), Duration::from_secs(2)).ok()?;
    let tree = cdp.call_for("Page.getFrameTree", json!({}), Some(session), Duration::from_secs(2)).ok()?;
    let root = frame_identity(&tree, None)?.0;
    let mut contexts = BTreeMap::<String, Value>::new();
    for event in cdp.events().iter().filter(|event| event["sessionId"] == session) {
        match event["method"].as_str() {
            Some("Runtime.executionContextsCleared") => contexts.clear(),
            Some("Runtime.executionContextDestroyed") => {
                contexts.retain(|unique, context| event["params"]["executionContextUniqueId"] != unique.as_str()
                    && event["params"]["executionContextId"] != context["id"]);
            }
            Some("Runtime.executionContextCreated") => {
                let context = &event["params"]["context"];
                if let Some(unique) = context["uniqueId"].as_str() { contexts.insert(unique.to_string(), context.clone()); }
            }
            _ => {}
        }
    }
    let (unique, context) = contexts.iter().find(|(unique, context)| {
        if let Some(id) = params.get("uniqueContextId").or_else(|| params.get("executionContextUniqueId")).and_then(Value::as_str) { unique.as_str() == id }
        else if let Some(id) = params.get("contextId").or_else(|| params.get("executionContextId")) { &context["id"] == id }
        else if let Some(name) = params.get("executionContextName") { &context["name"] == name }
        else { context["auxData"]["isDefault"] == true && context["auxData"]["frameId"] == root }
    })?;
    let (frame, loader) = frame_identity(&tree, context["auxData"]["frameId"].as_str())?;
    Some(Document { frame, loader, context:unique.clone() })
}

fn superseded(cdp: &Cdp, pending: &Pending) -> bool {
    cdp.events()[pending.from..].iter().any(|event| {
        event["sessionId"] == pending.session && (
            event["method"] == "Page.frameNavigated" && event["params"]["frame"]["id"] == pending.document.frame
                && event["params"]["frame"]["loaderId"].as_str().is_some_and(|loader| !loader.is_empty() && loader != pending.document.loader)
            || event["method"] == "Runtime.executionContextDestroyed"
                && event["params"]["executionContextUniqueId"] == pending.document.context
        )
    })
}

fn document_changed(cdp: &mut Cdp, pending: &Pending) -> bool {
    superseded(cdp, pending) || cdp.call_for("Page.getFrameTree", json!({}), Some(&pending.session), Duration::from_secs(2))
        .ok().and_then(|tree| frame_identity(&tree, Some(&pending.document.frame)))
        .is_some_and(|(_, loader)| loader != pending.document.loader)
}

fn poll(cdp: &mut Cdp, pending: &Pending) -> Option<Result<Value, String>> {
    if superseded(cdp, pending) { return Some(Err(SUPERSEDED.to_string())) }
    let expression = format!("JSON.stringify(globalThis[{:?}])", pending.key);
    let result = cdp.call_for("Runtime.evaluate", json!({"expression":expression,"returnByValue":true,"uniqueContextId":pending.document.context}), Some(&pending.session), Duration::from_secs(2));
    if superseded(cdp, pending) { return Some(Err(SUPERSEDED.to_string())) }
    match result {
        Ok(value) => {
            if let Some(state) = value.pointer("/result/value").and_then(Value::as_str).and_then(|text| serde_json::from_str::<Value>(text).ok()) {
                if state["done"] == true {
                    return Some(match state.get("error") {
                        Some(error) => Err(error.as_str().unwrap_or("").to_string()),
                        None => Ok(json!({"result":{"type":"object","value":state["value"]}})),
                    });
                }
            } else if document_changed(cdp, pending) { return Some(Err(SUPERSEDED.to_string())) }
        }
        Err(message) => return Some(Err(if document_changed(cdp, pending) { SUPERSEDED.to_string() } else { message })),
    }
    (pending.started.elapsed() > Duration::from_secs(30)).then(|| Err("bridge evaluation timeout".into()))
}

fn emit(value: Value) {
    println!("{value}");
    io::stdout().flush().unwrap();
}

fn answer(id: Value, result: Result<Value, String>) {
    match result {
        Ok(value) => emit(json!({"id":id,"result":value})),
        Err(message) => emit(json!({"id":id,"error":{"message":message}})),
    }
}

fn input() -> Box<dyn FnMut() -> Result<String, mpsc::TryRecvError>> {
    #[cfg(not(target_os = "wasi"))]
    {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        Box::new(move || rx.try_recv())
    }
    #[cfg(target_os = "wasi")]
    {
        use std::io::Read;
        #[link(wasm_import_module = "wasi_snapshot_preview1")]
        unsafe extern "C" {
            fn fd_fdstat_set_flags(fd: u32, flags: u32) -> u32;
        }
        unsafe { assert_eq!(fd_fdstat_set_flags(0, 4), 0); }
        let mut buffered = Vec::new();
        Box::new(move || {
            if !buffered.contains(&b'\n') {
                let mut bytes = [0; 8192];
                match io::stdin().read(&mut bytes) {
                    Ok(0) => return Err(mpsc::TryRecvError::Disconnected),
                    Ok(count) => buffered.extend_from_slice(&bytes[..count]),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => panic!("{error}"),
                }
            }
            match buffered.iter().position(|byte| *byte == b'\n') {
                Some(end) => Ok(String::from_utf8(buffered.drain(..=end).collect()).unwrap()),
                None => Err(mpsc::TryRecvError::Empty),
            }
        })
    }
}

fn main() {
    #[cfg(target_os = "wasi")]
    let url = std::env::var("SLICC_CDP_URL").unwrap();
    #[cfg(not(target_os = "wasi"))]
    let url = std::env::args().nth(1).unwrap();
    let mut cdp = Cdp::connect(&url).unwrap();
    let origin = std::env::args().nth(2).unwrap();
    cdp.protect_origins(&[&origin]);
    let mut input = input();
    let mut seen = 0;
    let mut pending = BTreeMap::<u64, Pending>::new();
    let mut wait = BUSY;
    loop {
        let mut busy = !pending.is_empty();
        match input() {
            Ok(line) => {
                busy = true;
                let message: Value = serde_json::from_str(&line).unwrap();
                let method = message["method"].as_str().unwrap();
                let mut params = message["params"].clone();
                let notify = params.as_object_mut().and_then(|params| params.remove("__guardBridgeNotify")).and_then(|value| value.as_bool()).unwrap_or(false);
                let session = message["sessionId"].as_str();
                if notify && method == "Page.navigate" {
                    answer(message["id"].clone(), cdp.notify(method, params, session).map(|_| json!({})));
                } else if method == "Runtime.evaluate" && params["awaitPromise"] == true {
                    let id = message["id"].as_u64().unwrap();
                    let key = format!("__guard_bridge_{id}");
                    let expression =
                        serde_json::to_string(params["expression"].as_str().unwrap()).unwrap();
                    let script = format!("(() => {{ const key = {key:?}; globalThis[key] = {{ done: false }}; Promise.resolve((0, eval)({expression})).then(value => {{ globalThis[key] = {{ done: true, value }}; }}, error => {{ globalThis[key] = {{ done: true, error: String(error) }}; }}); return true; }})()");
                    let document = session.and_then(|session| bind_document(&mut cdp, session, &params));
                    let from = cdp.events().len();
                    let mut original = params.clone();
                    original["expression"] = json!(script);
                    original["awaitPromise"] = json!(false);
                    original["returnByValue"] = json!(true);
                    if let Some(document) = &document {
                        for key in ["contextId", "executionContextId", "executionContextUniqueId", "executionContextName"] {
                            original.as_object_mut().unwrap().remove(key);
                        }
                        original["uniqueContextId"] = json!(document.context);
                    }
                    match cdp.call("Runtime.evaluate", original, session) {
                        Ok(value) if value.get("exceptionDetails").is_none() => {
                            if let Some(document) = document {
                                diagnostic(id, "original", Some(&document), None);
                                pending.insert(id, Pending { key, session:session.unwrap_or("").to_string(), document, from, started:Instant::now() });
                            } else {
                                diagnostic(id, "original", None, Some("bridge document identity unavailable"));
                                answer(json!(id), Err("bridge document identity unavailable".into()));
                            }
                        }
                        result => {
                            diagnostic(id, "original", document.as_ref(), result.as_ref().err().map(String::as_str));
                            answer(message["id"].clone(), result);
                        }
                    }
                } else {
                    let result = cdp.call(method, params, session);
                    for event in &cdp.events()[seen..] {
                        emit(json!({"event":event}));
                    }
                    seen = cdp.events().len();
                    answer(message["id"].clone(), result);
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        busy |= cdp.pump(Instant::now() + wait).unwrap_or(false);
        while let Ok(Some(paused)) = cdp.next_paused(None, Duration::ZERO) {
            busy = true;
            let headers: BTreeMap<String, String> = paused.headers.into_iter().collect();
            emit(
                json!({"event":{"method":"Fetch.requestPaused","sessionId":paused.session_id,"params":{"requestId":paused.request_id,"frameId":paused.frame_id,"resourceType":paused.resource_type,"responseStatusCode":paused.status,"responseHeaders":paused.response_headers.as_ref().map(|headers| headers.iter().map(|(name, value)| json!({"name":name,"value":value})).collect::<Vec<_>>()),"request":{"url":paused.url,"method":paused.method,"headers":headers}}}}),
            );
        }
        for event in &cdp.events()[seen..] {
            emit(json!({"event":event}));
        }
        busy |= cdp.events().len() > seen;
        seen = cdp.events().len();
        wait = if busy { BUSY } else { (wait * 2).min(IDLE) };
        let mut done = Vec::new();
        for (id, evaluation) in &pending {
            if let Some(result) = poll(&mut cdp, evaluation) {
                let message = result.as_ref().err().map(String::as_str);
                diagnostic(*id, if message == Some(SUPERSEDED) { "superseded" } else { "poll" }, Some(&evaluation.document), message);
                answer(json!(id), result);
                done.push(*id);
            }
        }
        for id in done {
            pending.remove(&id);
        }
    }
}
