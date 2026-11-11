use super::{Cdp, OPTIONAL_STEP};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

const WORKER_NOTICE: &str = "har-start: dedicated worker traffic is not captured on this CDP route\n";
const WORKER_STEP: Duration = Duration::from_secs(1);

#[derive(Default)]
pub(super) struct Har {
    armed: BTreeSet<String>,
    tried: BTreeSet<String>,
    workers: BTreeMap<String, String>,
    urls: BTreeMap<String, String>,
}

impl Cdp {
    pub fn har_auto_attach(&mut self, session: &str) -> Result<(), String> {
        if session.is_empty() {
            return Err(self.refusal());
        }
        if self.har.armed.contains(session) {
            return Ok(());
        }
        let params = json!({
            "autoAttach": true,
            "waitForDebuggerOnStart": true,
            "flatten": true,
            "filter": [{"type": "worker", "exclude": false}]
        });
        let result = self.raw_call("Target.setAutoAttach", params, Some(session), OPTIONAL_STEP);
        self.har.tried.insert(session.to_string());
        match result {
            Ok(_) => {
                self.har.armed.insert(session.to_string());
                Ok(())
            }
            Err(message) if super::transport_error(&message) => Err(message),
            Err(_) => {
                if !self.notices.iter().any(|notice| notice == WORKER_NOTICE) {
                    self.notices.push(WORKER_NOTICE.into());
                }
                Ok(())
            }
        }
    }

    pub fn har_stop(&mut self, session: &str) {
        let workers: Vec<String> = self
            .har
            .workers
            .iter()
            .filter(|(_, owner)| *owner == session)
            .map(|(worker, _)| worker.clone())
            .collect();
        for worker in workers {
            self.har_release_worker(&worker);
        }
        if self.har.armed.remove(session) || self.har.tried.contains(session) {
            let _ = self.raw_call(
                "Target.setAutoAttach",
                json!({
                    "autoAttach": false,
                    "waitForDebuggerOnStart": false,
                    "flatten": true,
                    "filter": [{"type": "worker", "exclude": false}]
                }),
                Some(session),
                OPTIONAL_STEP,
            );
        }
        self.har.tried.remove(session);
    }

    pub fn har_owns(&self, session: &str) -> bool {
        self.har.workers.contains_key(session)
    }

    pub fn har_worker_url(&self, session: &str) -> Option<&str> {
        self.har.urls.get(session).map(String::as_str)
    }

    pub(super) fn har_event(&mut self, value: &Value) -> bool {
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        if method == "Target.detachedFromTarget" {
            if let Some(child) = value.pointer("/params/sessionId").and_then(Value::as_str) {
                self.har.workers.remove(child);
                self.har.urls.remove(child);
            }
            return false;
        }
        if method != "Target.attachedToTarget" {
            return false;
        }
        let owner = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let info = &value["params"]["targetInfo"];
        let Some(child) = value
            .pointer("/params/sessionId")
            .and_then(Value::as_str)
            .filter(|child| !child.is_empty())
            .map(str::to_string)
        else {
            return false;
        };
        let kind = info.get("type").and_then(Value::as_str).unwrap_or("");
        if kind != "worker" || owner.is_empty() || !self.har.armed.contains(owner) {
            return false;
        }
        if self.blocked.contains(owner)
            || self.guard_sessions.contains(&child)
            || self.blocked.contains(&child)
            || self.har_refused_info(owner, info)
        {
            self.har_release_worker(&child);
            return true;
        }
        let deadline = Instant::now() + WORKER_STEP;
        let target = info.get("targetId").and_then(Value::as_str).unwrap_or("").to_string();
        let url = info.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        if !target.is_empty() {
            self.targets.insert(target.clone(), info.clone());
            self.session_targets.insert(child.clone(), target);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero()
            || self
                .raw_call("Network.enable", json!({}), Some(&child), left)
                .is_err()
        {
            self.har_release_worker(&child);
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero()
            || self
                .raw_call(
                    "Runtime.runIfWaitingForDebugger",
                    json!({}),
                    Some(&child),
                    left,
                )
                .is_err()
        {
            self.har_release_worker(&child);
            return true;
        }
        self.har.workers.insert(child.clone(), owner.to_string());
        self.har.urls.insert(child, url);
        true
    }

    fn har_refused_info(&self, owner: &str, info: &Value) -> bool {
        let url = info.get("url").and_then(Value::as_str).unwrap_or("");
        if url.is_empty() {
            return true;
        }
        let creator_protected = self.content_blocked(owner)
            || self.session_target(owner).is_some_and(|target| {
                self.is_protected_target(&target) || self.abandoned.contains(&target)
            });
        let opaque = url::Url::parse(url).is_ok_and(|parsed| matches!(parsed.scheme(), "blob" | "data"));
        if opaque {
            return creator_protected;
        }
        if creator_protected {
            return true;
        }
        self.guard.url(url)
            || self.guard.extension_frame(url)
            || self.guard.error_page_url(url)
            || self.guard.target(url, false, crate::protected::Opener::Unknown)
            || url::Url::parse(url).is_err()
    }

    fn har_release_worker(&mut self, child: &str) {
        self.har.workers.remove(child);
        self.har.urls.remove(child);
        let _ = self.send(
            "Target.detachFromTarget",
            json!({ "sessionId": child }),
            None,
        );
        self.blocked.insert(child.to_string());
        self.session_targets.remove(child);
    }

    pub(super) fn har_forget(&mut self, session: &str) {
        if self.har.workers.remove(session).is_some() {
            self.har.urls.remove(session);
            return;
        }
        if self.har.armed.contains(session) || self.har.tried.contains(session) {
            let children: Vec<String> = self
                .har
                .workers
                .iter()
                .filter(|(_, owner)| *owner == session)
                .map(|(worker, _)| worker.clone())
                .collect();
            for child in children {
                self.har_release_worker(&child);
            }
            self.har.armed.remove(session);
            self.har.tried.remove(session);
        }
    }
}

#[cfg(test)]
mod tests {
    include!("har_guard_test.rs");
}
