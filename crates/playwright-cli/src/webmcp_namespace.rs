use crate::session::{self, Session};
use serde_json::{json, Value};
use std::io::Write;

fn process_id() -> Option<u32> {
    #[cfg(target_os = "wasi")]
    { std::fs::read_to_string("/proc/self/stat").ok()?.split_whitespace().next()?.parse().ok() }
    #[cfg(not(target_os = "wasi"))]
    { Some(std::process::id()) }
}

fn payload(state: &Session, target: &str, phase: &str, selected: Value, env: impl Fn(&str) -> Option<String>) -> Value {
    let file = session::path_named(state.session_name.as_deref(), state.cwd.as_deref());
    let metadata = state.webmcp.get(target);
    json!({
        "phase":phase,
        "pid":process_id(),
        "HOME":env("HOME"),
        "XDG_STATE_HOME":env("XDG_STATE_HOME"),
        "XDG_CONFIG_HOME":env("XDG_CONFIG_HOME"),
        "PWD":env("PWD"),
        "file":file.as_ref().map(|file| file.to_string_lossy().to_string()),
        "fileHash":file.as_ref().and_then(|file| file.file_stem()).map(|hash| hash.to_string_lossy().to_string()),
        "connectionHash":state.webmcp_connection,
        "metadataPresent":metadata.is_some(),
        "metadataConnectionMatches":metadata.and_then(|entry| entry.get("connection")).and_then(Value::as_str) == Some(state.webmcp_connection.as_str()),
        "selected":selected
    })
}

fn append(file: &std::path::Path, value: &Value) {
    let Ok(mut log) = std::fs::OpenOptions::new().create(true).append(true).open(file) else { return; };
    let _ = writeln!(log, "{value}");
}

pub(crate) fn record(state: &Session, target: &str, phase: &str, selected: Value) {
    if std::env::var("SLICC_WEBMCP_TRACE").ok().as_deref() != Some("1") { return; }
    let value = payload(state, target, phase, selected, |name| std::env::var(name).ok());
    append(&std::env::temp_dir().join("cdp-675-namespace.ndjson"), &value);
}
