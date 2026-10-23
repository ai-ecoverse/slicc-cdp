use crate::args::{Invocation, Output};
use crate::cdp::{self, Cdp};
use crate::session::{self, Session};
use serde_json::{json, Value};
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const LIMIT: usize = 16384;
type Input = Box<dyn FnMut() -> Result<Vec<u8>, mpsc::TryRecvError>>;

pub fn hold(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let Some(target) = invocation.flags.get("tab").filter(|target| !target.is_empty()) else {
        return Output::err("dialog-hold requires --tab <targetId>\n");
    };
    let held = match cdp.attach_dialog(target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = run(cdp, state, target, &held, input, emit);
    cdp::detach(cdp, &held);
    *state = session::load_in(state.session_name.as_deref(), state.cwd.as_deref());
    match result { Ok(()) => Output::ok(""), Err(message) => Output::err(message) }
}

fn emit(value: Value) -> Result<(), String> {
    write_event(&mut io::stdout().lock(), value)
}

fn write_event(writer: &mut impl Write, value: Value) -> Result<(), String> {
    writeln!(writer, "{}", crate::sanitize::json_c1(&value.to_string()))
        .and_then(|_| writer.flush()).map_err(|_| "dialog-hold: failed to write event\n".into())
}

fn run(cdp: &mut Cdp, state: &mut Session, target: &str, held: &str, make_input: impl FnOnce() -> Result<Input, String>, mut emit: impl FnMut(Value) -> Result<(), String>) -> Result<(), String> {
    let mut input = make_input()?;
    let mut ready = cdp.dialog_ready(held)?;
    state.current = Some(target.to_string());
    session::save(state)?;
    ready["event"] = json!("ready");
    emit(ready.clone())?;
    let mut pending = Vec::new();
    let mut overflow = false;
    let mut last_dialog = None;
    let mut seen = cdp.events().len();
    loop {
        cdp.pump(Instant::now() + Duration::from_millis(50))?;
        let dialog = cdp.dialog_id(held);
        let mut notifications = Vec::new();
        if dialog != last_dialog {
            if let Some(epoch) = dialog {
                let allowed = cdp.verify_dialog(held, epoch).is_ok();
                notifications.push(json!({"event":if allowed {"dialog-opened"} else {"dialog-refused"},"target":target,"dialogId":epoch}));
            } else {
                notifications.push(json!({"event":"dialog-closed","target":target,"dialogId":last_dialog}));
            }
            last_dialog = dialog;
        }
        let changed = cdp.events()[seen..].iter().any(|event| event["sessionId"] == held && event["method"] == "Page.frameNavigated");
        seen = cdp.events().len();
        if changed {
            let mut document = cdp.dialog_ready(held)?;
            if !document["loaderId"].is_null() && document["loaderId"] != ready["loaderId"] {
                document["event"] = json!("document-ready");
                notifications.push(document.clone());
            }
            ready = document;
        }
        for notification in notifications { emit(notification)?; }
        if cdp.session_blocked(held) || cdp.is_protected_target(target)
            || cdp.events().iter().rev().any(|event| event["method"] == "Target.targetDestroyed" && event["params"]["targetId"] == target) {
            emit(json!({"event":"stopped","target":target,"reason":"target closed, changed or refused"}))?;
            return Err("dialog-hold: target closed, changed or refused\n".into());
        }
        match input() {
            Err(mpsc::TryRecvError::Disconnected) => {
                if !pending.is_empty() || overflow { emit(json!({"event":"invalid-request","id":null,"reason":"incomplete JSON line at EOF"}))?; }
                emit(json!({"event":"stopped","target":target,"reason":"stdin-eof"}))?;
                return Ok(());
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Ok(bytes) => for byte in bytes {
                if byte == b'\n' {
                    if overflow { emit(json!({"event":"invalid-request","id":null,"reason":"JSON line exceeds 16384 bytes"}))?; }
                    else if !pending.is_empty() { emit(request(cdp, held, &pending))?; }
                    pending.clear();
                    overflow = false;
                } else if pending.len() < LIMIT && !overflow { pending.push(byte); }
                else { pending.clear(); overflow = true; }
            },
        }
    }
}

fn request(cdp: &mut Cdp, held: &str, bytes: &[u8]) -> Value {
    let parsed = serde_json::from_slice::<Value>(bytes);
    let id = parsed.as_ref().ok().and_then(|value| value.get("id")).and_then(Value::as_u64);
    let invalid = || json!({"event":"invalid-request","id":id,"reason":"expected id, dialogId, command (dialog-accept or dialog-dismiss), and optional promptText"});
    let Ok(value) = parsed else { return invalid() };
    let Some(object) = value.as_object() else { return invalid() };
    let Some(id) = id else { return invalid() };
    let Some(epoch) = value.get("dialogId").and_then(Value::as_u64) else { return invalid() };
    let accept = match value.get("command").and_then(Value::as_str) {
        Some("dialog-accept") => true,
        Some("dialog-dismiss") => false,
        _ => return invalid(),
    };
    if object.keys().any(|key| !matches!(key.as_str(), "id" | "dialogId" | "command" | "promptText"))
        || value.get("promptText").is_some_and(|text| !text.is_string())
        || !accept && value.get("promptText").is_some() { return invalid() }
    let Some(current) = cdp.dialog_id(held) else {
        return json!({"event":"no-dialog","id":id,"reason":"No observed dialog is open; dialogs predating holder attachment cannot be answered."});
    };
    if current != epoch { return json!({"event":"stale-dialog","id":id,"dialogId":epoch}) }
    if cdp.verify_dialog(held, epoch).is_err() { return json!({"event":"dialog-refused","id":id,"dialogId":epoch}) }
    let mut params = json!({"accept":accept});
    if let Some(text) = value.get("promptText") { params["promptText"] = text.clone(); }
    match cdp.call_for("Page.handleJavaScriptDialog", params, Some(held), Duration::from_secs(5)) {
        Ok(_) => json!({"event":"handled","id":id,"dialogId":epoch,"accept":accept}),
        Err(_) => json!({"event":"dialog-refused","id":id,"dialogId":epoch}),
    }
}

#[cfg(not(target_os = "wasi"))]
fn read_input(mut reader: impl io::Read, tx: mpsc::SyncSender<Vec<u8>>) {
    let mut bytes = [0; 1024];
    loop {
        match reader.read(&mut bytes) {
            Ok(0) | Err(_) => return,
            Ok(length) => if tx.send(bytes[..length].to_vec()).is_err() { return },
        }
    }
}

fn input() -> Result<Input, String> {
    #[cfg(not(target_os = "wasi"))]
    {
        let (tx, rx) = mpsc::sync_channel(16);
        std::thread::spawn(move || read_input(io::stdin(), tx));
        Ok(Box::new(move || rx.try_recv()))
    }
    #[cfg(target_os = "wasi")]
    {
        use std::io::Read;
        #[link(wasm_import_module = "wasi_snapshot_preview1")]
        unsafe extern "C" { fn fd_fdstat_set_flags(fd: u32, flags: u32) -> u32; }
        if unsafe { fd_fdstat_set_flags(0, 4) } != 0 { return Err("dialog-hold: nonblocking stdin unavailable\n".into()) }
        Ok(Box::new(move || {
            let mut bytes = [0; 1024];
            match io::stdin().read(&mut bytes) {
                Ok(0) => Err(mpsc::TryRecvError::Disconnected),
                Ok(length) => Ok(bytes[..length].to_vec()),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(mpsc::TryRecvError::Empty),
                Err(_) => Err(mpsc::TryRecvError::Disconnected),
            }
        }))
    }
}
