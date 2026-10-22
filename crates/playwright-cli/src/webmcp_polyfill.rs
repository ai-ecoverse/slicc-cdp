use crate::args::{Invocation, Output};
use crate::browser;
use crate::cdp::Cdp;
use crate::session::{self, Session};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

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
        Err(message) => Output::err(message),
    }
}

fn save(cdp: &Cdp, state: &mut Session, target: &str, owner: &str) -> Result<(), String> {
    let mut latest = session::load_in(state.session_name.as_deref(), state.cwd.as_deref());
    latest.webmcp_connection = state.webmcp_connection.clone();
    let data = json!({"owner":owner,"connection":state.webmcp_connection,"sessions":cdp.webmcp_snapshot()});
    latest.webmcp.insert(target.to_string(), data.clone());
    session::save_webmcp(&latest)?;
    state.webmcp.insert(target.to_string(), data);
    Ok(())
}

type Input = Box<dyn FnMut() -> Result<(), mpsc::TryRecvError>>;

fn run(cdp: &mut Cdp, state: &mut Session, target: &str, session: &str, reload: bool) -> Result<(), String> {
    run_with_input(cdp, state, target, session, reload, input)
}

fn run_with_input(cdp: &mut Cdp, state: &mut Session, target: &str, session: &str, reload: bool, make_input: impl FnOnce() -> Result<Input, String>) -> Result<(), String> {
    let owner = cdp.start_webmcp(session)?;
    cdp.webmcp_auto_attach(session)?;
    save(cdp, state, target, &owner)?;
    let mut input = make_input()?;
    if reload {
        let from = cdp.events().len();
        cdp.call("Page.reload", json!({}), Some(session))?;
        if cdp.wait_event(from, Duration::from_secs(15), |event| event.get("method").and_then(Value::as_str) == Some("Page.loadEventFired") && event.get("sessionId").and_then(Value::as_str) == Some(session))?.is_none() {
            return Err("webmcp-polyfill: reload did not complete\n".into());
        }
        save(cdp, state, target, &owner)?;
    }
    let tree = cdp.call("Page.getFrameTree", json!({}), Some(session))?;
    let frame = tree.pointer("/frameTree/frame/id").and_then(Value::as_str).ok_or_else(|| "webmcp-polyfill: unverified document\n".to_string())?;
    let world = cdp.call("Page.createIsolatedWorld", json!({"frameId":frame,"worldName":"slicc-webmcp"}), Some(session))?;
    let native = cdp.call("Runtime.evaluate", json!({"expression":"'modelContext' in Document.prototype","contextId":world["executionContextId"],"returnByValue":true}), Some(session))?.pointer("/result/value").and_then(Value::as_bool) == Some(true);
    let mode = if native { json!("native") } else { browser::evaluate(cdp, session, &format!("(() => {{ 'use strict'; if (typeof {owner} !== 'undefined' && {owner}) return 'polyfill'; return document.modelContext ? 'page' : 'unavailable'; }})()"), false)?.value };
    let status = json!({"event":"ready","loaderId":tree["frameTree"]["frame"]["loaderId"],"mode":mode,"tab":target,"late":!reload,"reloaded":reload,"pauseMicros":cdp.webmcp_pause_micros()});
    println!("{status}");
    io::stdout().flush().map_err(|_| "webmcp-polyfill: failed to write status\n")?;
    let mut saved = serde_json::to_string(&cdp.webmcp_snapshot()).unwrap_or_default();
    loop {
        match input() {
            Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            Err(mpsc::TryRecvError::Empty) | Ok(()) => {}
        }
        if !cdp.webmcp_alive(session) {
            return Err("webmcp-polyfill: target closed, changed or was refused by the guard\n".into());
        }
        let from = cdp.events().len();
        cdp.pump(Instant::now() + Duration::from_millis(50))?;
        for notice in cdp.take_notices() { eprint!("{notice}"); }
        let current = serde_json::to_string(&cdp.webmcp_snapshot()).unwrap_or_default();
        if current != saved {
            save(cdp, state, target, &owner)?;
            saved = current;
        }
        for (_target, child, _bridge) in cdp.take_webmcp_children() {
            cdp.webmcp_auto_attach(&child)?;
        }
        let loaded = cdp.events()[from..].iter().any(|event| event["method"] == "Page.loadEventFired" && event["sessionId"] == session);
        if loaded && cdp.webmcp_alive(session) {
            let document = cdp.webmcp_document(session, frame)?;
            save(cdp, state, target, &owner)?;
            println!("{}", json!({"event":"document-ready","tab":target,"loaderId":document["loaderId"]}));
            io::stdout().flush().map_err(|_| "webmcp-polyfill: failed to write status\n")?;
        }
        if cdp.events().iter().rev().any(|event| event.get("method").and_then(Value::as_str) == Some("Target.targetDestroyed") && event.pointer("/params/targetId").and_then(Value::as_str) == Some(target)) {
            return Ok(());
        }
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

fn input() -> Result<Input, String> {
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
