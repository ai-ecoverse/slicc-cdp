use crate::protected::Protected;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const REFUSAL: &str = "slicc: refused: that execution context belongs to a browser extension\n";
pub const UNSEEN: &str =
    "slicc: refused: that execution context or object is not exposed to browser automation\n";

type Key = (String, String);

#[derive(Default)]
pub(crate) struct Worlds {
    labels: BTreeMap<(String, i64), (Option<String>, Option<String>)>,
    ids: BTreeMap<(String, i64), bool>,
    unique: BTreeMap<Key, bool>,
    names: BTreeMap<Key, bool>,
    objects: BTreeMap<Key, bool>,
    scripts: BTreeMap<Key, bool>,
    calls: BTreeMap<Key, bool>,
    hidden_sessions: BTreeSet<String>,
    enabled: BTreeSet<String>,
}

impl Worlds {
    pub fn enabled(&self, session: &str) -> bool {
        self.enabled.contains(session)
    }
    pub fn enable(&mut self, session: &str) {
        self.enabled.insert(session.to_string());
    }
    pub fn disable(&mut self, session: &str) {
        self.enabled.remove(session);
    }

    pub fn sensitive(method: &str) -> bool {
        ["Debugger.", "Profiler.", "HeapProfiler."]
            .iter()
            .any(|prefix| method.starts_with(prefix))
            || method == "Runtime.addBinding"
    }

    pub fn selectors(params: &Value) -> bool {
        walk(params, &mut |key, value| {
            (matches!(key, "contextId" | "executionContextId") && value.is_i64())
                || matches!(
                    key,
                    "uniqueContextId"
                        | "executionContextUniqueId"
                        | "executionContextName"
                        | "objectId"
                        | "prototypeObjectId"
                        | "scriptId"
                        | "callFrameId"
                )
        })
    }

    fn status(&self, session: &str, key: &str, value: &Value) -> Option<Option<bool>> {
        let pair = |id: &str| (session.to_string(), id.to_string());
        match key {
            "contextId" | "executionContextId" => value
                .as_i64()
                .map(|id| self.ids.get(&(session.to_string(), id)).copied()),
            "uniqueContextId" | "executionContextUniqueId" => {
                value.as_str().map(|id| self.unique.get(&pair(id)).copied())
            }
            "executionContextName" => value.as_str().map(|id| self.names.get(&pair(id)).copied()),
            "objectId" | "prototypeObjectId" => value
                .as_str()
                .map(|id| self.objects.get(&pair(id)).copied()),
            "scriptId" => value
                .as_str()
                .map(|id| self.scripts.get(&pair(id)).copied()),
            "callFrameId" => value.as_str().map(|id| self.calls.get(&pair(id)).copied()),
            _ => None,
        }
    }

    fn has_hidden(&self, session: &str) -> bool {
        self.hidden_sessions.contains(session)
    }

    pub fn reason(&self, session: &str, method: &str, params: &Value) -> Option<&'static str> {
        if Self::sensitive(method) && self.has_hidden(session) && method != "Runtime.addBinding" {
            return Some(REFUSAL);
        }
        if method == "Runtime.addBinding" && !Self::selectors(params) {
            return Some(if self.has_hidden(session) {
                REFUSAL
            } else {
                UNSEEN
            });
        }
        if walk(params, &mut |key, value| {
            self.status(session, key, value) == Some(Some(true))
        }) {
            return Some(REFUSAL);
        }
        if walk(params, &mut |key, value| {
            self.status(session, key, value) == Some(None)
        }) {
            return Some(UNSEEN);
        }
        None
    }

    fn clear(&mut self, session: &str) {
        self.labels.retain(|(owner, _), _| owner != session);
        self.ids.retain(|(owner, _), _| owner != session);
        for map in [
            &mut self.unique,
            &mut self.names,
            &mut self.objects,
            &mut self.scripts,
            &mut self.calls,
        ] {
            map.retain(|(owner, _), _| owner != session);
        }
    }

    pub fn event(&mut self, value: &Value, guard: &Protected) -> bool {
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        let params = &value["params"];
        if method == "Runtime.executionContextCreated" {
            let context = &params["context"];
            let origin = context.get("origin").and_then(Value::as_str).unwrap_or("");
            let extension = [
                "chrome-extension:",
                "moz-extension:",
                "safari-web-extension:",
                "ms-browser-extension:",
            ]
            .iter()
            .any(|prefix| origin.starts_with(prefix));
            let hidden = context
                .pointer("/auxData/isDefault")
                .and_then(Value::as_bool)
                == Some(false)
                && (extension || guard.url(origin));
            if hidden {
                self.hidden_sessions.insert(session.to_string());
            }
            if let Some(id) = context.get("id").and_then(Value::as_i64) {
                self.ids.insert((session.to_string(), id), hidden);
                self.labels.insert(
                    (session.to_string(), id),
                    (
                        context
                            .get("uniqueId")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        context
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    ),
                );
            }
            for (key, map) in [("uniqueId", &mut self.unique), ("name", &mut self.names)] {
                if let Some(id) = context.get(key).and_then(Value::as_str) {
                    map.insert((session.to_string(), id.to_string()), hidden);
                }
            }
            return hidden;
        }
        if method == "Runtime.executionContextsCleared" {
            self.clear(session);
            return false;
        }
        let hidden = walk(params, &mut |key, value| {
            self.status(session, key, value) == Some(Some(true))
        }) || (["Debugger.", "Profiler.", "HeapProfiler."]
            .iter()
            .any(|prefix| method.starts_with(prefix))
            && self.has_hidden(session));
        if method == "Debugger.scriptParsed" || method == "Debugger.scriptFailedToParse" {
            if let Some(id) = params.get("scriptId").and_then(Value::as_str) {
                self.scripts
                    .insert((session.to_string(), id.to_string()), hidden);
            }
        }
        if method == "Debugger.paused" {
            if let Some(frames) = params.get("callFrames").and_then(Value::as_array) {
                for frame in frames {
                    if let Some(id) = frame.get("callFrameId").and_then(Value::as_str) {
                        self.calls
                            .insert((session.to_string(), id.to_string()), hidden);
                    }
                }
            }
        }
        if hidden {
            self.remember_hidden(session, params);
        }
        if method == "Runtime.executionContextDestroyed" {
            if let Some(id) = params.get("executionContextId").and_then(Value::as_i64) {
                self.ids.remove(&(session.to_string(), id));
                if let Some((unique, name)) = self.labels.remove(&(session.to_string(), id)) {
                    if let Some(unique) = unique {
                        self.unique.remove(&(session.to_string(), unique));
                    }
                    if let Some(name) = name {
                        self.names.remove(&(session.to_string(), name));
                    }
                }
            }
            if let Some(id) = params
                .get("executionContextUniqueId")
                .and_then(Value::as_str)
            {
                self.unique.remove(&(session.to_string(), id.to_string()));
            }
            if !hidden {
                self.objects.retain(|(owner, _), _| owner != session);
                self.scripts.retain(|(owner, _), _| owner != session);
            }
        }
        if method == "Target.detachedFromTarget" {
            if let Some(child) = params.get("sessionId").and_then(Value::as_str) {
                self.clear(child);
                self.hidden_sessions.remove(child);
                self.enabled.remove(child);
            }
        }
        hidden
    }

    fn remember_hidden(&mut self, session: &str, value: &Value) {
        walk(value, &mut |key, value| {
            if matches!(key, "objectId" | "prototypeObjectId") {
                if let Some(id) = value.as_str() {
                    self.objects
                        .insert((session.to_string(), id.to_string()), true);
                }
            }
            false
        });
    }

    pub fn reply(&mut self, session: &str, method: &str, value: &Value) {
        if matches!(
            method,
            "Runtime.evaluate"
                | "Runtime.callFunctionOn"
                | "Runtime.awaitPromise"
                | "Runtime.getProperties"
                | "Runtime.queryObjects"
                | "Runtime.getExceptionDetails"
                | "Runtime.compileScript"
                | "Runtime.runScript"
                | "DOM.resolveNode"
                | "Debugger.evaluateOnCallFrame"
                | "HeapProfiler.getObjectByHeapObjectId"
        ) {
            self.remember(session, value);
        }
    }

    pub fn visible_event(&mut self, session: &str, method: &str, params: &Value) {
        if matches!(
            method,
            "Runtime.consoleAPICalled"
                | "Runtime.exceptionThrown"
                | "Runtime.inspectRequested"
                | "Debugger.paused"
        ) {
            self.remember(session, params);
        }
    }

    fn remember(&mut self, session: &str, value: &Value) {
        match value {
            Value::Object(items) => {
                let remote = items.contains_key("objectId")
                    || items
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| {
                            matches!(
                                kind,
                                "object"
                                    | "function"
                                    | "undefined"
                                    | "string"
                                    | "number"
                                    | "boolean"
                                    | "symbol"
                                    | "bigint"
                            )
                        });
                if remote {
                    if let Some(id) = items.get("objectId").and_then(Value::as_str) {
                        self.objects
                            .entry((session.to_string(), id.to_string()))
                            .or_insert(false);
                    }
                }
                for (key, value) in items {
                    if !(remote && key == "value") {
                        self.remember(session, value);
                    }
                }
            }
            Value::Array(items) => {
                for value in items {
                    self.remember(session, value);
                }
            }
            _ => {}
        }
    }

    pub fn created(&mut self, session: &str, result: &Value) {
        if let Some(id) = result.get("executionContextId").and_then(Value::as_i64) {
            self.ids.entry((session.to_string(), id)).or_insert(false);
        }
    }

    pub fn script(&mut self, session: &str, result: &Value) {
        if let Some(id) = result.get("scriptId").and_then(Value::as_str) {
            self.scripts
                .entry((session.to_string(), id.to_string()))
                .or_insert(false);
        }
    }
}

fn walk(value: &Value, visit: &mut impl FnMut(&str, &Value) -> bool) -> bool {
    match value {
        Value::Object(items) => items.iter().any(|(key, value)| {
            !matches!(key.as_str(), "value" | "expression" | "functionDeclaration")
                && (visit(key, value) || walk(value, visit))
        }),
        Value::Array(items) => items.iter().any(|value| walk(value, visit)),
        _ => false,
    }
}
