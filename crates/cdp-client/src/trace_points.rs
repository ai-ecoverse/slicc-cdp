use super::Cdp;
use crate::shield;
use crate::trace;
use serde_json::Value;

impl Cdp {
    pub(super) fn trace_kind(&self, session: &str) -> &str {
        if session.is_empty() {
            return "browser";
        }
        if self.har.workers.contains_key(session) {
            return "har-worker";
        }
        if self.guard_sessions.contains(session) {
            return "guard";
        }
        self.session_target(session)
            .and_then(|target| self.targets.get(&target))
            .and_then(|info| info.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    }

    pub fn trace_session_kind(&self, session: &str) -> String {
        self.trace_kind(session).to_string()
    }

    pub(super) fn trace_worker(&self, session: &str) -> bool {
        !session.is_empty() && matches!(self.trace_kind(session), "worker" | "har-worker")
    }

    pub(super) fn trace_wanted(value: &Value) -> bool {
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        !method.starts_with("Network.")
            || matches!(method, "Network.requestWillBeSent" | "Network.responseReceived" | "Network.loadingFinished" | "Network.loadingFailed")
    }

    pub(super) fn trace_event_on(&self, value: &Value) -> bool {
        trace::on()
            && Self::trace_wanted(value)
            && self.trace_worker(value.get("sessionId").and_then(Value::as_str).unwrap_or(""))
    }

    pub(super) fn trace_event(&self, value: &Value, what: &str) {
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
        trace::line(format_args!(
            "guard ev {what} m={} s={} kind={} rid={}",
            value.get("method").and_then(Value::as_str).unwrap_or(""),
            trace::short(session),
            self.trace_kind(session),
            value.pointer("/params/requestId").and_then(Value::as_str).unwrap_or("")
        ));
    }

    pub(super) fn trace_scrub_terms(&self, value: &Value, target: Option<&str>, terms: &mut [bool; 4]) {
        match value {
            Value::Array(items) => {
                for item in items {
                    self.trace_scrub_terms(item, target, terms);
                }
            }
            Value::Object(map) => {
                let url = map.get("url").and_then(Value::as_str).unwrap_or("");
                let unreachable = map.get("unreachableUrl").and_then(Value::as_str).unwrap_or("");
                let origin = map
                    .get("securityOrigin")
                    .or_else(|| map.get("origin"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let shaped = (map.contains_key("loaderId") && map.contains_key("securityOrigin"))
                    || map.contains_key("userTypedURL")
                    || map.contains_key("transitionType")
                    || map.contains_key("windowName")
                    || map.contains_key("suggestedFilename")
                    || (map.contains_key("targetId")
                        && map.get("type").and_then(Value::as_str).is_some_and(|kind| {
                            matches!(kind, "page" | "iframe" | "background_page" | "webview" | "app" | "other")
                        }));
                let host = self.failed_load_host_hidden(target, url, unreachable);
                let origin_hit = self.guard.url(origin);
                let loaded = shield::loaded_urls_protected(&self.guard, value);
                terms[0] |= host;
                terms[1] |= origin_hit;
                terms[2] |= loaded;
                terms[3] |= shaped && (host || origin_hit || loaded);
                for item in map.values() {
                    self.trace_scrub_terms(item, target, terms);
                }
            }
            _ => {}
        }
    }

    pub(super) fn trace_terms(&self, value: &Value, target: Option<&str>) -> String {
        let mut terms = [false; 4];
        self.trace_scrub_terms(value, target, &mut terms);
        format!(
            "hide_host={} hide_origin={} hide_loaded={} hide_acted={}",
            terms[0], terms[1], terms[2], terms[3]
        )
    }

    pub fn trace_body_state(&self, session: &str, request_id: &str) -> String {
        format!(
            "content_blocked={} creator_blocked={} request_state={:?} protected_body={} protected_page={} source={}",
            self.content_blocked(session),
            self.har_creator_blocked(session),
            self.request_state(request_id),
            self.protected_body_requests.contains(request_id),
            self.protected_request_pages.iter().any(|(_, id)| id == request_id),
            self.ordinary_network_source(session, request_id).is_some()
        )
    }
}
