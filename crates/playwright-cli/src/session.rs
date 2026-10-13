use crate::route::{self, Route};
use crate::snapshot::RefRec;
use crate::trace::{self, ConsoleEntry, NetRequest};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub current: Option<String>,
    pub endpoint: Option<String>,
    pub provenance: Option<String>,
    pub owned: Vec<String>,
    pub snapshots: BTreeMap<String, BTreeMap<String, RefRec>>,
    pub console: BTreeMap<String, Vec<ConsoleEntry>>,
    pub requests: BTreeMap<String, Vec<NetRequest>>,
    pub routes: Vec<Route>,
}

pub fn storage_path(cwd: &Path) -> Option<PathBuf> {
    #[cfg(target_arch = "wasm32")]
    {
        Some(cwd.join(".playwright-cli").join("session.json"))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        session_path(&config_root(), &canonical)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn session_path(config_dir: &str, cwd: &Path) -> Option<PathBuf> {
    let root = Path::new(config_dir);
    if !root.is_absolute() {
        return None;
    }
    let key = cdp_client::hash::sha256_hex(cwd.as_os_str().as_encoded_bytes());
    Some(
        root.join("slicc")
            .join("playwright-cli")
            .join("sessions")
            .join(format!("{key}.json")),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn config_root() -> String {
    #[cfg(test)]
    {
        std::env::temp_dir()
            .join("slicc-cdp-session-tests")
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(all(not(test), not(target_arch = "wasm32")))]
    {
        crate::launch::user_config_dir()
    }
}

fn path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    storage_path(&cwd)
}

pub fn load() -> Session {
    path().as_deref().map(load_at).unwrap_or_default()
}

pub fn load_at(path: &Path) -> Session {
    let Ok(text) = fs::read_to_string(path) else {
        return Session::default();
    };
    let _ = private_file(path);
    decode(&text)
}

fn decode(text: &str) -> Session {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Session::default();
    };
    let current = value
        .get("current")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let endpoint = value
        .get("endpoint")
        .and_then(|v| v.as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let provenance = value
        .get("provenance")
        .and_then(|v| v.as_str())
        .filter(|text| known_provenance(text))
        .map(str::to_string);
    let mut owned = Vec::new();
    if let Some(items) = value.get("owned").and_then(|item| item.as_array()) {
        for item in items {
            if let Some(id) = item.as_str().filter(|id| !id.is_empty()) {
                if !owned.iter().any(|have: &String| have == id) {
                    owned.push(id.to_string());
                }
            }
        }
    }
    let mut snapshots = BTreeMap::new();
    if let Some(tabs) = value.get("snapshots").and_then(|v| v.as_object()) {
        for (target, refs) in tabs {
            let mut map = BTreeMap::new();
            if let Some(object) = refs.as_object() {
                for (id, rec) in object {
                    map.insert(
                        id.clone(),
                        RefRec {
                            backend_node_id: rec.get("backendNodeId").and_then(|v| v.as_u64()),
                            selector: rec
                                .get("selector")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            frame_id: rec
                                .get("frameId")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                            label: rec
                                .get("label")
                                .and_then(|v| v.as_str())
                                .filter(|label| !label.is_empty())
                                .map(str::to_string),
                        },
                    );
                }
            }
            snapshots.insert(target.clone(), map);
        }
    }
    Session {
        current,
        endpoint,
        provenance,
        owned,
        snapshots,
        console: decode_console(&value),
        requests: decode_requests(&value),
        routes: value
            .get("routes")
            .and_then(|item| item.as_array())
            .map(|items| items.iter().filter_map(route::decode).collect())
            .unwrap_or_default(),
    }
}

fn decode_console(value: &Value) -> BTreeMap<String, Vec<ConsoleEntry>> {
    let mut map = BTreeMap::new();
    let Some(tabs) = value.get("console").and_then(|item| item.as_object()) else {
        return map;
    };
    for (target, entries) in tabs {
        let list = entries
            .as_array()
            .map(|items| items.iter().filter_map(trace::console_from_json).collect())
            .unwrap_or_default();
        map.insert(target.clone(), list);
    }
    map
}

fn decode_requests(value: &Value) -> BTreeMap<String, Vec<NetRequest>> {
    let mut map = BTreeMap::new();
    let Some(tabs) = value.get("requests").and_then(|item| item.as_object()) else {
        return map;
    };
    for (target, entries) in tabs {
        let list = entries
            .as_array()
            .map(|items| items.iter().filter_map(trace::request_from_json).collect())
            .unwrap_or_default();
        map.insert(target.clone(), list);
    }
    map
}

fn encode(session: &Session) -> Value {
    let mut snapshots = serde_json::Map::new();
    for (target, refs) in &session.snapshots {
        let mut object = serde_json::Map::new();
        for (id, rec) in refs {
            object.insert(
                id.clone(),
                json!({
                    "backendNodeId": rec.backend_node_id,
                    "selector": rec.selector,
                    "frameId": rec.frame_id,
                    "label": rec.label,
                }),
            );
        }
        snapshots.insert(target.clone(), Value::Object(object));
    }
    json!({
        "current": session.current,
        "endpoint": session.endpoint,
        "provenance": session.provenance,
        "owned": session.owned,
        "snapshots": snapshots,
        "console": encode_lists(&session.console, trace::console_json),
        "requests": encode_lists(&session.requests, trace::request_json),
        "routes": session.routes.iter().map(route::encode).collect::<Vec<_>>(),
    })
}

fn encode_lists<T>(
    tabs: &BTreeMap<String, Vec<T>>,
    encode_one: fn(&T) -> Value,
) -> Value {
    let mut object = serde_json::Map::new();
    for (target, entries) in tabs {
        object.insert(
            target.clone(),
            Value::Array(entries.iter().map(encode_one).collect()),
        );
    }
    Value::Object(object)
}

pub fn push_trace(
    session: &mut Session,
    target: &str,
    console: Vec<ConsoleEntry>,
    requests: Vec<NetRequest>,
) {
    if !console.is_empty() {
        let stored = session.console.entry(target.to_string()).or_default();
        for entry in console {
            if !entry.stamp.is_empty()
                && stored.iter().any(|have| {
                    have.stamp == entry.stamp
                        && have.kind == entry.kind
                        && have.text == entry.text
                        && have.url == entry.url
                        && have.line == entry.line
                })
            {
                continue;
            }
            stored.push(entry);
        }
    }
    if !requests.is_empty() {
        session
            .requests
            .entry(target.to_string())
            .or_default()
            .extend(requests);
    }
}

pub fn clear_console(session: &mut Session, target: &str) {
    session.console.remove(target);
}

pub fn clear_requests(session: &mut Session, target: &str) {
    session.requests.remove(target);
}

pub fn stored_label<'a>(session: &'a Session, target: &str, id: &str) -> Option<&'a str> {
    session
        .snapshots
        .get(target)?
        .get(id)?
        .label
        .as_deref()
        .filter(|label| !label.is_empty())
}

pub fn save(session: &Session) -> Result<(), String> {
    let mut stored = session.clone();
    if usable_endpoint(stored.endpoint.as_deref(), stored.provenance.as_deref()).is_none() {
        stored.endpoint = None;
        stored.provenance = None;
    }
    let Some(file) = path() else {
        return Err("session: user config dir is not absolute\n".into());
    };
    write_at(&file, &stored)
}

pub fn write_at(path: &Path, session: &Session) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|err| format!("session: {err}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::Permissions::from_mode(0o700);
            let _ = fs::set_permissions(dir, mode.clone());
            if let Some(parent) = dir.parent() {
                let _ = fs::set_permissions(parent, mode);
            }
        }
    }
    let body = serde_json::to_string_pretty(&encode(session)).unwrap_or_else(|_| "{}".into());
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let existed = path.is_file();
        if existed {
            private_file(path)?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|err| format!("session: {err}\n"))?;
        if !existed {
            if let Err(err) = private_file(path) {
                let _ = fs::remove_file(path);
                return Err(err);
            }
        }
        file.write_all(body.as_bytes())
            .map_err(|err| format!("session: {err}\n"))?;
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        fs::write(path, body).map_err(|err| format!("session: {err}\n"))
    }
}

pub fn known_provenance(text: &str) -> bool {
    matches!(text, "explicit" | "seven-profile" | "probe")
}

pub fn note_owned(session: &mut Session, id: &str) {
    if id.is_empty() || session.owned.iter().any(|have| have == id) {
        return;
    }
    session.owned.push(id.to_string());
}

pub fn usable_endpoint(endpoint: Option<&str>, provenance: Option<&str>) -> Option<String> {
    let url = endpoint.filter(|text| !text.is_empty())?;
    if !known_provenance(provenance?) {
        return None;
    }
    if !loopback_url(url) {
        return None;
    }
    Some(url.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn reuse_saved(
    endpoint: Option<&str>,
    provenance: Option<&str>,
    list_body: Option<&str>,
) -> Option<String> {
    let url = usable_endpoint(endpoint, provenance)?;
    if provenance == Some("probe") {
        list_url(&url)?;
        if !crate::discover::list_has_seven_tab(list_body?) {
            return None;
        }
    }
    Some(url)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn list_url(endpoint: &str) -> Option<String> {
    let lower = endpoint.to_ascii_lowercase();
    if lower.starts_with("ws://") || lower.starts_with("wss://") {
        return None;
    }
    if !lower.starts_with("http://") && !lower.starts_with("https://") {
        return None;
    }
    if let Some(index) = endpoint.find("/json/version") {
        let mut list = endpoint.to_string();
        list.replace_range(index..index + "/json/version".len(), "/json/list");
        return Some(list);
    }
    Some(format!("{}/json/list", endpoint.trim_end_matches('/')))
}

pub fn page_visible(provenance: Option<&str>, url: &str, id: &str, owned: &[String]) -> bool {
    if provenance != Some("probe") {
        return true;
    }
    crate::discover::seven_origin(url) || owned.iter().any(|item| item == id)
}

#[cfg(test)]
thread_local! {
    static FAIL_PRIVATE_FILE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn private_file(path: &Path) -> Result<(), String> {
    #[cfg(test)]
    if FAIL_PRIVATE_FILE.get() {
        return Err("session: Operation not permitted\n".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("session: {err}\n"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub fn loopback_url(url: &str) -> bool {
    let Ok(endpoint) = cdp_client::net::parse_endpoint(url) else {
        return false;
    };
    if !matches!(endpoint.scheme.as_str(), "http" | "https" | "ws" | "wss") {
        return false;
    }
    let host = endpoint.host.to_ascii_lowercase();
    host == "localhost" || host == "::1" || ipv4_loopback(&host)
}

fn ipv4_loopback(host: &str) -> bool {
    let mut parts = host.split('.');
    let Some(first) = parts.next().and_then(|text| text.parse::<u8>().ok()) else {
        return false;
    };
    let Some(_) = parts.next().and_then(|text| text.parse::<u8>().ok()) else {
        return false;
    };
    let Some(_) = parts.next().and_then(|text| text.parse::<u8>().ok()) else {
        return false;
    };
    let Some(_) = parts.next().and_then(|text| text.parse::<u8>().ok()) else {
        return false;
    };
    parts.next().is_none() && first == 127
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdp_client::hash::sha256_hex;

    #[test]
    fn an_old_session_without_label_still_loads() {
        let text = r#"{"current":"T","snapshots":{"T":{"e1":{"backendNodeId":4,"selector":"button","frameId":null}}}}"#;
        let session = decode(text);
        assert_eq!(session.current.as_deref(), Some("T"));
        let rec = &session.snapshots["T"]["e1"];
        assert_eq!(rec.selector, "button");
        assert_eq!(rec.backend_node_id, Some(4));
        assert!(rec.label.is_none());
        assert!(stored_label(&session, "T", "e1").is_none());
        assert!(stored_label(&session, "T", "e9").is_none());
        assert!(session.endpoint.is_none());
        assert!(session.provenance.is_none());
        assert!(session.owned.is_empty());
        assert!(session.console.is_empty());
        assert!(session.requests.is_empty());
    }

    #[test]
    fn an_endpoint_round_trips_and_a_blank_one_is_absent() {
        let session = Session {
            endpoint: Some("http://127.0.0.1:9333/json/version".to_string()),
            ..Session::default()
        };
        let again = decode(&encode(&session).to_string());
        assert_eq!(
            again.endpoint.as_deref(),
            Some("http://127.0.0.1:9333/json/version")
        );
        let blank = decode(r#"{"endpoint":"","current":null}"#);
        assert!(blank.endpoint.is_none());
    }

    #[test]
    fn a_label_round_trips_and_an_empty_one_is_absent() {
        let mut refs = BTreeMap::new();
        refs.insert(
            "e1".to_string(),
            RefRec {
                backend_node_id: Some(7),
                selector: "button".to_string(),
                frame_id: None,
                label: Some("button \"Sign in\"".to_string()),
            },
        );
        refs.insert(
            "e2".to_string(),
            RefRec {
                backend_node_id: None,
                selector: "input".to_string(),
                frame_id: Some("F".to_string()),
                label: Some(String::new()),
            },
        );
        let session = Session {
            current: Some("T".to_string()),
            snapshots: BTreeMap::from([("T".to_string(), refs)]),
            ..Session::default()
        };
        let again = decode(&encode(&session).to_string());
        assert_eq!(stored_label(&again, "T", "e1"), Some("button \"Sign in\""));
        assert!(stored_label(&again, "T", "e2").is_none());
        assert_eq!(again.snapshots["T"]["e2"].frame_id.as_deref(), Some("F"));
    }

    #[test]
    fn console_and_requests_round_trip_with_query_and_cookie() {
        let mut session = Session::default();
        push_trace(
            &mut session,
            "T",
            vec![ConsoleEntry {
                kind: "log".to_string(),
                text: "hi".to_string(),
                url: "https://user:secret@example.test/a?q=1#h".to_string(),
                line: 4,
                stamp: "9.5".to_string(),
            }],
            vec![NetRequest {
                method: "GET".to_string(),
                url: "https://example.test/a?token=1#x".to_string(),
                resource_type: "fetch".to_string(),
                status: Some(200),
                status_text: "OK".to_string(),
                failed: None,
                mime: "text/plain".to_string(),
                request_headers: vec![("cookie".to_string(), "session=secret".to_string())],
                response_headers: vec![("set-cookie".to_string(), "a=b".to_string())],
                request_body: Some("body".to_string()),
                response_body: Some("ok".to_string()),
                response_base64: false,
            }],
        );
        let mut again = decode(&encode(&session).to_string());
        assert_eq!(
            again.console["T"][0].url,
            "https://user:secret@example.test/a?q=1#h"
        );
        assert_eq!(again.console["T"][0].stamp, "9.5");
        push_trace(
            &mut again,
            "T",
            vec![
                ConsoleEntry {
                    kind: "log".to_string(),
                    text: "hi".to_string(),
                    url: "https://user:secret@example.test/a?q=1#h".to_string(),
                    line: 4,
                    stamp: "9.5".to_string(),
                },
                ConsoleEntry {
                    kind: "log".to_string(),
                    text: "hi".to_string(),
                    url: "https://user:secret@example.test/a?q=1#h".to_string(),
                    line: 4,
                    stamp: "9.75".to_string(),
                },
            ],
            Vec::new(),
        );
        assert_eq!(again.console["T"].len(), 2);
        assert_eq!(again.console["T"][1].stamp, "9.75");
        push_trace(
            &mut again,
            "T",
            vec![ConsoleEntry {
                kind: "log".to_string(),
                text: "again".to_string(),
                url: String::new(),
                line: 0,
                stamp: String::new(),
            }],
            Vec::new(),
        );
        assert_eq!(again.console["T"].len(), 3);
        assert_eq!(again.requests["T"][0].url, "https://example.test/a?token=1#x");
        assert_eq!(
            again.requests["T"][0].request_headers[0].1,
            "session=secret"
        );
        assert_eq!(again.requests["T"][0].response_body.as_deref(), Some("ok"));
        clear_console(&mut again, "T");
        clear_requests(&mut again, "T");
        assert!(again.console.is_empty());
        assert!(again.requests.is_empty());
    }

    #[test]
    fn a_planted_cwd_session_is_not_the_saved_endpoint() {
        let cwd = std::env::temp_dir().join(format!(
            "slicc-planted-cwd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        ));
        let config = std::env::temp_dir().join(format!(
            "slicc-planted-config-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&cwd);
        fs::create_dir_all(cwd.join(".playwright-cli")).unwrap();
        fs::write(
            cwd.join(".playwright-cli").join("session.json"),
            r#"{"endpoint":"https://evil.example/json/version","provenance":"explicit","current":"personal"}"#,
        )
        .unwrap();
        let stored = session_path(config.to_str().unwrap(), &cwd).unwrap();
        assert_ne!(stored, cwd.join(".playwright-cli").join("session.json"));
        assert!(stored.starts_with(&config));
        assert!(!stored.starts_with(&cwd));
        assert!(load_at(&stored).endpoint.is_none());
        assert!(load_at(&cwd.join(".playwright-cli").join("session.json"))
            .endpoint
            .is_some());
        let _ = fs::remove_dir_all(&cwd);
        let _ = fs::remove_dir_all(&config);
    }

    #[test]
    fn saved_state_never_yields_a_remote_endpoint() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let cases = [
            "https://evil.example/json/version",
            "http://evil.example:8080/json/version",
            "wss://evil.example/devtools/browser/x",
            "ws://10.1.2.3:8999/devtools/browser/x",
            "ws://evil.example:8999/devtools/browser/x",
            "http://127.0.0.1@evil.example/json/version",
            "ftp://127.0.0.1:9333/json/version",
        ];
        for url in cases {
            assert!(usable_endpoint(Some(url), Some("explicit")).is_none(), "{url}");
            assert!(!loopback_url(url), "{url}");
        }
        assert_eq!(
            usable_endpoint(Some("http://127.0.0.1:9333/json/version"), Some("explicit")).as_deref(),
            Some("http://127.0.0.1:9333/json/version")
        );
        assert_eq!(
            usable_endpoint(Some("ws://localhost:9333/devtools/browser/abc"), Some("explicit")).as_deref(),
            Some("ws://localhost:9333/devtools/browser/abc")
        );
        assert_eq!(
            usable_endpoint(Some("http://[::1]:9333/json/version"), Some("seven-profile")).as_deref(),
            Some("http://[::1]:9333/json/version")
        );
        assert!(usable_endpoint(Some("http://127.0.0.1:9333/json/version"), None).is_none());
        assert!(usable_endpoint(Some("http://127.0.0.1:9333/json/version"), Some("planted")).is_none());
        assert!(loopback_url("http://127.8.8.8:9222/json/version"));
        assert!(!loopback_url("http://10.0.0.1/json/version"));
        let disguised = "http://evil.example@127.0.0.1:9222/json/version";
        let parsed = cdp_client::net::parse_endpoint(disguised).unwrap();
        assert_eq!(parsed.host, "127.0.0.1");
        assert_eq!(parsed.port, 9222);
        assert!(loopback_url(disguised));
        let remote = "http://127.0.0.1:9222@evil.example/json/version";
        let parsed = cdp_client::net::parse_endpoint(remote).unwrap();
        assert_eq!(parsed.host, "evil.example");
        assert!(!loopback_url(remote));
        assert!(usable_endpoint(Some(remote), Some("explicit")).is_none());
        assert!(loopback_url("http://user@[::1]:9333/json/version"));
        assert!(loopback_url("http://127.0.0.1:9333?x=1"));
        assert_eq!(
            list_url("http://127.0.0.1:9333/json/version").as_deref(),
            Some("http://127.0.0.1:9333/json/list")
        );
        assert!(list_url("ws://127.0.0.1:9333/devtools/browser/abc").is_none());
        let seven = r#"[{"id":"T1","type":"page","url":"https://lars.sliccy.ai/","title":"seven"},{"id":"P1","type":"page","url":"https://example.com/inbox","title":"PERSONAL"}]"#;
        let personal = r#"[{"id":"P1","type":"page","url":"https://example.com/inbox","title":"PERSONAL"}]"#;
        assert_eq!(
            reuse_saved(Some("http://127.0.0.1:9222/json/version"), Some("probe"), Some(seven)).as_deref(),
            Some("http://127.0.0.1:9222/json/version")
        );
        assert!(reuse_saved(Some("http://127.0.0.1:9222/json/version"), Some("probe"), Some(personal)).is_none());
        assert!(reuse_saved(Some("http://127.0.0.1:9222/json/version"), Some("probe"), None).is_none());
        assert!(reuse_saved(Some("ws://127.0.0.1:9222/devtools/browser/x"), Some("probe"), Some(seven)).is_none());
        assert!(reuse_saved(Some("https://evil.example/json/version"), Some("explicit"), None).is_none());
        assert_eq!(
            reuse_saved(Some("http://127.0.0.1:9333/json/version"), Some("explicit"), None).as_deref(),
            Some("http://127.0.0.1:9333/json/version")
        );
        assert_eq!(
            reuse_saved(Some("http://127.0.0.1:9333/json/version"), Some("seven-profile"), None).as_deref(),
            Some("http://127.0.0.1:9333/json/version")
        );
    }

    #[test]
    fn a_probe_session_hides_personal_tabs_and_keeps_owned_ones() {
        let owned = vec!["OWN".to_string()];
        assert!(page_visible(Some("probe"), "https://abc.sliccy.ai/", "S", &owned));
        assert!(!page_visible(Some("probe"), "https://example.com/inbox", "P", &owned));
        assert!(page_visible(Some("probe"), "https://example.com/inbox", "OWN", &owned));
        assert!(page_visible(Some("explicit"), "https://example.com/inbox", "P", &[]));
        assert!(page_visible(Some("seven-profile"), "https://mail.example/", "P", &[]));
        assert!(page_visible(None, "https://example.com/", "P", &[]));
    }

    #[cfg(unix)]
    #[test]
    fn a_saved_session_is_private_to_the_user() {
        let root = std::env::temp_dir().join(format!("slicc-session-mode-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let path = session_path(root.to_str().unwrap(), Path::new("/work")).unwrap();
        let session = Session {
            endpoint: Some("http://127.0.0.1:9333/json/version".to_string()),
            provenance: Some("probe".to_string()),
            owned: vec!["OWN".to_string()],
            ..Session::default()
        };
        write_at(&path, &session).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let dir_mode = fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        let parent_mode = fs::metadata(path.parent().unwrap().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
        assert_eq!(parent_mode, 0o700);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o666);
        write_at(&path, &session).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        let _ = load_at(&path);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let loaded = load_at(&path);
        assert_eq!(loaded.provenance.as_deref(), Some("probe"));
        assert_eq!(loaded.owned, vec!["OWN".to_string()]);
        let locked = session_path(root.to_str().unwrap(), Path::new("/work-locked")).unwrap();
        fs::write(&locked, "ORIGINAL\n").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o666)).unwrap();
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                super::FAIL_PRIVATE_FILE.set(false);
            }
        }
        super::FAIL_PRIVATE_FILE.set(true);
        let _reset = Reset;
        let err = write_at(&locked, &session).unwrap_err();
        assert_eq!(err, "session: Operation not permitted\n");
        assert_eq!(fs::read_to_string(&locked).unwrap(), "ORIGINAL\n");
        assert_eq!(fs::metadata(&locked).unwrap().permissions().mode() & 0o777, 0o666);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_relative_config_dir_is_not_a_session_path() {
        assert!(session_path(".config", Path::new("work")).is_none());
        assert!(session_path("AppData\\Roaming", Path::new("work")).is_none());
        assert!(session_path("", Path::new("work")).is_none());
        let stored = session_path(
            std::env::temp_dir().to_str().unwrap(),
            Path::new("/work"),
        )
        .unwrap();
        assert!(stored.is_absolute());
        let key = sha256_hex(Path::new("/work").as_os_str().as_encoded_bytes());
        assert!(stored.ends_with(format!("{key}.json")));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_directories_do_not_share_a_session_file() {
        use std::os::unix::ffi::OsStrExt;
        let left = Path::new(std::ffi::OsStr::from_bytes(b"/work/\xff"));
        let right = Path::new(std::ffi::OsStr::from_bytes(b"/work/\xfe"));
        assert_eq!(left.to_string_lossy(), right.to_string_lossy());
        let root = std::env::temp_dir();
        let root = root.to_str().unwrap();
        let a = session_path(root, left).unwrap();
        let b = session_path(root, right).unwrap();
        assert_ne!(a, b);
    }
}
