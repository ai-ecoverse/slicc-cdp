use cdp_client::cdp::Cdp;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{self, Write};
#[cfg(not(target_os = "wasi"))]
use std::io::BufRead;
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
    let mut pending = BTreeMap::<u64, (String, String, Instant)>::new();
    loop {
        match input() {
            Ok(line) => {
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
                    match cdp.call(
                        "Runtime.evaluate",
                        json!({"expression":script,"returnByValue":true,"userGesture":true}),
                        session,
                    ) {
                        Ok(value) if value.get("exceptionDetails").is_none() => {
                            pending.insert(
                                id,
                                (key, session.unwrap_or("").to_string(), Instant::now()),
                            );
                        }
                        result => answer(message["id"].clone(), result),
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
        let _ = cdp.pump(Instant::now() + Duration::from_millis(5));
        while let Ok(Some(paused)) = cdp.next_paused(None, Duration::ZERO) {
            let headers: BTreeMap<String, String> = paused.headers.into_iter().collect();
            emit(
                json!({"event":{"method":"Fetch.requestPaused","sessionId":paused.session_id,"params":{"requestId":paused.request_id,"frameId":paused.frame_id,"resourceType":paused.resource_type,"responseStatusCode":paused.status,"responseHeaders":paused.response_headers.as_ref().map(|headers| headers.iter().map(|(name, value)| json!({"name":name,"value":value})).collect::<Vec<_>>()),"request":{"url":paused.url,"method":paused.method,"headers":headers}}}}),
            );
        }
        for event in &cdp.events()[seen..] {
            emit(json!({"event":event}));
        }
        seen = cdp.events().len();
        let mut done = Vec::new();
        for (id, (key, session, started)) in &pending {
            let expression = format!("JSON.stringify(globalThis[{key:?}])");
            let result = cdp.call_for(
                "Runtime.evaluate",
                json!({"expression":expression,"returnByValue":true}),
                Some(session),
                Duration::from_secs(2),
            );
            match result {
                Ok(value) => {
                    if let Some(text) = value.pointer("/result/value").and_then(Value::as_str) {
                        if let Ok(state) = serde_json::from_str::<Value>(text) {
                            if state["done"] == true {
                                if state.get("error").is_some() {
                                    answer(
                                        json!(id),
                                        Err(state["error"].as_str().unwrap_or("").to_string()),
                                    );
                                } else {
                                    answer(
                                        json!(id),
                                        Ok(
                                            json!({"result":{"type":"object","value":state["value"]}}),
                                        ),
                                    );
                                }
                                done.push(*id);
                            }
                        }
                    }
                    if started.elapsed() > Duration::from_secs(30) {
                        answer(json!(id), Err("bridge evaluation timeout".into()));
                        done.push(*id);
                    }
                }
                Err(message) => {
                    answer(json!(id), Err(message));
                    done.push(*id);
                }
            }
        }
        for id in done {
            pending.remove(&id);
        }
    }
}
