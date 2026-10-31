use crate::args::{Invocation, Output};
use crate::browser;
use crate::cdp::Cdp;
use crate::session::{self, Session};
use crate::webmcp_watch::Watch;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(crate) const FALLBACK_EMPTY: &str = "webmcp: no tools are available through the other-extension fallback; page startup or parse-time checks may have happened before installation\n";

pub fn bridge_name(value: &str) -> bool {
    value.len() == 39 && value.starts_with("__wmcp_") && value[7..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn hold(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let Some(target) = invocation.flags.get("tab").filter(|value| !value.is_empty()) else {
        return Output::err("webmcp-polyfill requires --tab <targetId>\n");
    };
    let session = match browser::watch_attach(cdp, state, target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let previous_owner = state.webmcp.get(target).and_then(|entry| entry.get("owner")).and_then(Value::as_str).map(str::to_string);
    let result = run(cdp, state, target, &session, invocation.flags.contains_key("reload"));
    let result = cdp.webmcp_trace_result(&session, "holder run", result);
    cdp.stop_webmcp(&session);
    let _ = browser::watch_detach(cdp, state, target, &session);
    let mut latest = session::load_in(state.session_name.as_deref(), state.cwd.as_deref());
    let ours = state.webmcp.get(target).and_then(|entry| entry.get("owner")).and_then(Value::as_str);
    if ours.is_some() && ours != previous_owner.as_deref() && latest.webmcp.get(target).and_then(|entry| entry.get("owner")).and_then(Value::as_str) == ours {
        latest.webmcp.remove(target);
        if let Err(message) = session::save_webmcp(&latest) { return Output::err(message); }
    }
    latest.webmcp_connection = state.webmcp_connection.clone();
    *state = latest;
    match result {
        Ok(()) => Output::ok(""),
        Err(message) if message.starts_with("slicc: refused:") => Output::err(format!("webmcp-polyfill: Runtime.evaluate: {message}")),
        Err(message) => Output::err(message),
    }
}

fn save(cdp: &Cdp, state: &mut Session, target: &str, owner: &str, changes: Value) -> Result<(), String> {
    let mut latest = session::load_in(state.session_name.as_deref(), state.cwd.as_deref());
    latest.webmcp_connection = state.webmcp_connection.clone();
    let data = json!({"owner":owner,"connection":state.webmcp_connection,"sessions":cdp.webmcp_snapshot(),"changes":changes});
    latest.webmcp.insert(target.to_string(), data.clone());
    session::save_webmcp(&latest)?;
    state.webmcp.insert(target.to_string(), data);
    Ok(())
}

pub(crate) type Input = Box<dyn FnMut() -> Result<(), mpsc::TryRecvError>>;

fn run(cdp: &mut Cdp, state: &mut Session, target: &str, session: &str, reload: bool) -> Result<(), String> {
    run_with_input(cdp, state, target, session, reload, input)
}

fn run_with_input(cdp: &mut Cdp, state: &mut Session, target: &str, session: &str, reload: bool, make_input: impl FnOnce() -> Result<Input, String>) -> Result<(), String> {
    let owner = cdp.start_webmcp_for(session, reload)?;
    cdp.webmcp_auto_attach(session)?;
    save(cdp, state, target, &owner, json!({}))?;
    let mut input = make_input()?;
    let tree = cdp.call("Page.getFrameTree", json!({}), Some(session))?;
    let frame = tree.pointer("/frameTree/frame/id").and_then(Value::as_str).ok_or_else(|| "webmcp-polyfill: unverified document\n".to_string())?;
    let world = cdp.call("Page.createIsolatedWorld", json!({"frameId":frame,"worldName":"slicc-webmcp"}), Some(session))?;
    let native = cdp.call("Runtime.evaluate", json!({"expression":"'modelContext' in Document.prototype","contextId":world["executionContextId"],"returnByValue":true}), Some(session))?.pointer("/result/value").and_then(Value::as_bool) == Some(true);
    let mode = if native { json!("native") } else { browser::evaluate(cdp, session, &format!("(() => {{ 'use strict'; if (typeof {owner} !== 'undefined' && {owner}) return 'polyfill'; return document.modelContext ? 'page' : 'unavailable'; }})()"), false)?.value };
    let mut watch = Watch::new(target, session, frame);
    watch.arm(cdp, state)?;
    if cdp.webmcp_plain(session) && watch.empty() { eprint!("{FALLBACK_EMPTY}"); }
    let status = json!({"event":"ready","loaderId":tree["frameTree"]["frame"]["loaderId"],"mode":mode,"tab":target,"late":!reload,"reloaded":reload,"pauseMicros":cdp.webmcp_pause_micros()});
    println!("{status}");
    io::stdout().flush().map_err(|_| "webmcp-polyfill: failed to write status\n")?;
    let mut saved = serde_json::to_string(&cdp.webmcp_snapshot()).unwrap_or_default();
    let mut readiness = Readiness { seen: cdp.events().len(), waiting: false };
    loop {
        match input() {
            Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            Err(mpsc::TryRecvError::Empty) | Ok(()) => {}
        }
        if !cdp.webmcp_alive(session) {
            cdp.webmcp_trace_stage(session, "holder-exit", "root liveness check");
            return Err("slicc: refused: WebMCP target closed, changed or was refused by the guard; tools cannot be verified\n".into());
        }
        let pumped = cdp.pump(Instant::now() + Duration::from_millis(50));
        cdp.webmcp_trace_result(session, "event pump", pumped)?;
        for notice in cdp.take_notices() { eprint!("{notice}"); }
        let current = serde_json::to_string(&cdp.webmcp_snapshot()).unwrap_or_default();
        if current != saved {
            save(cdp, state, target, &owner, watch.changes())?;
            saved = current;
        }
        for (_target, child, _bridge) in cdp.take_webmcp_children() {
            let attached = cdp.webmcp_auto_attach(&child);
            cdp.webmcp_trace_result(&child, "child auto attach", attached)?;
        }
        if readiness.due(cdp.events(), session, cdp.webmcp_settled()) && cdp.webmcp_alive(session) {
            let document = cdp.webmcp_document(session, frame);
            let document = cdp.webmcp_trace_result(session, "document readiness", document)?;
            save(cdp, state, target, &owner, json!({}))?;
            watch.arm(cdp, state)?;
            if cdp.webmcp_plain(session) && watch.empty() { eprint!("{FALLBACK_EMPTY}"); }
            println!("{}", json!({"event":"document-ready","tab":target,"loaderId":document["loaderId"]}));
            io::stdout().flush().map_err(|_| "webmcp-polyfill: failed to write status\n")?;
        }
        let events = watch.tick(cdp, state);
        let events = cdp.webmcp_trace_result(session, "change verification", events)?;
        if !events.is_empty() {
            if cdp.webmcp_plain(session) && events.iter().any(|event| event.get("tools").and_then(Value::as_array).is_some_and(Vec::is_empty)) { eprint!("{FALLBACK_EMPTY}"); }
            save(cdp, state, target, &owner, watch.changes())?;
            for event in events { println!("{}", crate::sanitize::json_c1(&event.to_string())); }
            io::stdout().flush().map_err(|_| "webmcp-polyfill: failed to write status\n")?;
        }
        if cdp.events().iter().rev().any(|event| event.get("method").and_then(Value::as_str) == Some("Target.targetDestroyed") && event.pointer("/params/targetId").and_then(Value::as_str) == Some(target)) {
            return Ok(());
        }
    }
}

struct Readiness {
    seen: usize,
    waiting: bool,
}

impl Readiness {
    fn due(&mut self, events: &[Value], session: &str, settled: bool) -> bool {
        self.waiting |= events[self.seen..].iter().any(|event| event["method"] == "Page.loadEventFired" && event["sessionId"] == session);
        self.seen = events.len();
        let due = self.waiting && settled;
        self.waiting &= !due;
        due
    }
}

#[cfg(not(target_os = "wasi"))]
fn read_input(mut reader: impl io::Read, tx: mpsc::Sender<()>) {
    let mut bytes = [0; 1024];
    loop {
        match reader.read(&mut bytes) {
            Ok(0) | Err(_) => break,
            Ok(_) => if tx.send(()).is_err() { break; },
        }
    }
}

pub(crate) fn input() -> Result<Input, String> {
    #[cfg(not(target_os = "wasi"))]
    {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || read_input(io::stdin(), tx));
        Ok(Box::new(move || rx.try_recv()))
    }
    #[cfg(target_os = "wasi")]
    {
        use std::io::Read;
        #[link(wasm_import_module = "wasi_snapshot_preview1")]
        unsafe extern "C" {
            fn fd_fdstat_set_flags(fd: u32, flags: u32) -> u32;
        }
        if unsafe { fd_fdstat_set_flags(0, 4) } != 0 {
            return Err("webmcp-polyfill: nonblocking stdin unavailable\n".into());
        }
        Ok(Box::new(move || {
            let mut bytes = [0; 1024];
            match io::stdin().read(&mut bytes) {
                Ok(0) => Err(mpsc::TryRecvError::Disconnected),
                Ok(_) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(mpsc::TryRecvError::Empty),
                Err(_) => Err(mpsc::TryRecvError::Disconnected),
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(session: &str) -> Value {
        json!({"method":"Page.loadEventFired","sessionId":session,"params":{"timestamp":1}})
    }

    #[test]
    fn document_readiness_waits_for_frames_admitted_after_the_top_load() {
        let mut events = vec![load("TOP")];
        let mut readiness = Readiness { seen: 1, waiting: false };
        assert!(!readiness.due(&events, "TOP", true));
        events.push(load("CHILD"));
        assert!(!readiness.due(&events, "TOP", true));
        events.push(load("TOP"));
        assert!(!readiness.due(&events, "TOP", false));
        assert!(!readiness.due(&events, "TOP", false));
        assert!(readiness.due(&events, "TOP", true));
        assert!(!readiness.due(&events, "TOP", true));
        events.extend([load("TOP"), load("TOP")]);
        assert!(readiness.due(&events, "TOP", true));
        assert!(!readiness.due(&events, "TOP", true));
    }
}
