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
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
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
                        },
                    );
                }
            }
            snapshots.insert(target.clone(), map);
        }
    }
    Session { current, snapshots }
}

pub fn save(session: &Session) -> Result<(), String> {
    let dir = PathBuf::from(".playwright-cli");
    fs::create_dir_all(&dir).map_err(|err| format!("session: {err}\n"))?;
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
                }),
            );
        }
        snapshots.insert(target.clone(), Value::Object(object));
    }
    let value = json!({
        "current": session.current,
        "snapshots": snapshots,
    });
    fs::write(path(), serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into()))
        .map_err(|err| format!("session: {err}\n"))
}
