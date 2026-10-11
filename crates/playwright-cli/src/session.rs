use crate::snapshot::RefRec;
use crate::trace::{self, ConsoleEntry, NetRequest};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub current: Option<String>,
    pub snapshots: BTreeMap<String, BTreeMap<String, RefRec>>,
    pub console: BTreeMap<String, Vec<ConsoleEntry>>,
    pub requests: BTreeMap<String, Vec<NetRequest>>,
}

fn path() -> PathBuf {
    PathBuf::from(".playwright-cli").join("session.json")
}

pub fn load() -> Session {
    let Ok(text) = fs::read_to_string(path()) else {
        return Session::default();
    };
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
        snapshots,
        console: decode_console(&value),
        requests: decode_requests(&value),
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
        "snapshots": snapshots,
        "console": encode_lists(&session.console, trace::console_json),
        "requests": encode_lists(&session.requests, trace::request_json),
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
    let dir = PathBuf::from(".playwright-cli");
    fs::create_dir_all(&dir).map_err(|err| format!("session: {err}\n"))?;
    fs::write(
        path(),
        serde_json::to_string_pretty(&encode(session)).unwrap_or_else(|_| "{}".into()),
    )
    .map_err(|err| format!("session: {err}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(session.console.is_empty());
        assert!(session.requests.is_empty());
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
}
