use crate::snapshot::RefRec;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub current: Option<String>,
    pub snapshots: BTreeMap<String, BTreeMap<String, RefRec>>,
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
    Session { current, snapshots }
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
    })
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
        };
        let again = decode(&encode(&session).to_string());
        assert_eq!(stored_label(&again, "T", "e1"), Some("button \"Sign in\""));
        assert!(stored_label(&again, "T", "e2").is_none());
        assert_eq!(again.snapshots["T"]["e2"].frame_id.as_deref(), Some("F"));
    }
}
