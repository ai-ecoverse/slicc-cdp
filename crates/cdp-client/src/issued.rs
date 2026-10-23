use super::Cdp;
use crate::fetch::Paused;
use crate::net::base64_decode;
use crate::protected::Protected;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

const ARM: Duration = Duration::from_secs(10);
const FETCH_LIKE: [&str; 3] = ["XHR", "Fetch", "EventSource"];
const NAMED: [&str; 12] = ["Document", "Stylesheet", "Image", "Media", "Font", "Script", "XHR", "Fetch", "EventSource", "Prefetch", "Ping", "CSPViolationReport"];

#[derive(Default)]
pub(super) struct Issued {
    client: BTreeMap<String, Value>,
    armed: BTreeSet<String>,
    chains: BTreeSet<(String, String)>,
    urls: BTreeMap<(String, String), String>,
    auth: BTreeSet<(String, String)>,
    responses: BTreeMap<(String, String), (u64, Vec<String>)>,
    arming: BTreeSet<String>,
    deferred: BTreeMap<String, Vec<Paused>>,
}

enum Token {
    Any,
    One,
    Char(char),
}

fn wildcard(pattern: &str, text: &str) -> bool {
    let mut tokens = Vec::new();
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        tokens.push(match ch {
            '*' => Token::Any,
            '?' => Token::One,
            '\\' => Token::Char(chars.next().unwrap_or('\\')),
            ch => Token::Char(ch),
        });
    }
    let text: Vec<char> = text.chars().collect();
    let (mut at, mut from, mut star) = (0, 0, None);
    while from < text.len() {
        match tokens.get(at) {
            Some(Token::Any) => {
                star = Some((at, from));
                at += 1;
            }
            Some(Token::One) => {
                at += 1;
                from += 1;
            }
            Some(Token::Char(ch)) if *ch == text[from] => {
                at += 1;
                from += 1;
            }
            _ => {
                let Some((mark, seen)) = star else { return false };
                star = Some((mark, seen + 1));
                at = mark + 1;
                from = seen + 1;
            }
        }
    }
    tokens[at.min(tokens.len())..].iter().all(|token| matches!(token, Token::Any))
}

fn kind_matches(pattern: &str, kind: &str) -> bool {
    pattern == kind
        || FETCH_LIKE.contains(&pattern) && FETCH_LIKE.contains(&kind)
        || pattern == "Fetch" && kind == "Prefetch"
        || pattern == "Other" && !NAMED.contains(&kind)
}

fn refusal_text(guard: &Protected, source: &str, target: &str) -> String {
    let resolved = url::Url::parse(source).ok().and_then(|base| base.join(target.trim()).ok()).map(|url| url.to_string()).filter(|url| guard.url(url));
    format!("{}\n", guard.refusal_for_url(resolved.as_deref().unwrap_or(target)))
}

pub(crate) fn target_refusal(guard: &Protected, source: &str, target: &str) -> Result<String, String> {
    let targets = Cdp::carrier_targets(guard, target, source).ok_or_else(|| refusal_text(guard, source, target))?;
    let url = &targets[0].url;
    if target.trim().is_empty() || targets.iter().any(|target| target.protected)
        || !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err(refusal_text(guard, source, target));
    }
    Ok(url.to_string())
}

fn header_locations(fields: Option<&Value>, binary: Option<&Value>) -> Vec<String> {
    let mut found: Vec<String> = fields.and_then(Value::as_array).into_iter().flatten()
        .filter(|field| field.get("name").and_then(Value::as_str).is_some_and(|name| name.trim().eq_ignore_ascii_case("location")))
        .map(|field| field.get("value").and_then(Value::as_str).unwrap_or("").to_string()).collect();
    if let Some(text) = binary.and_then(Value::as_str) {
        let bytes = base64_decode(text).unwrap_or_else(|_| b"location:".to_vec());
        found.extend(raw_locations(&String::from_utf8_lossy(&bytes), &['\0', '\n']));
    }
    found
}

fn raw_locations(text: &str, separators: &[char]) -> Vec<String> {
    text.split(separators).filter_map(|line| line.split_once(':')).filter(|(name, _)| name.trim().eq_ignore_ascii_case("location")).map(|(_, value)| value.trim().to_string()).collect()
}

fn redirect_answer(method: &str, params: &Value) -> (u64, Vec<String>) {
    if method == "Network.continueInterceptedRequest" {
        let Some(raw) = params.get("rawResponse").and_then(Value::as_str) else { return (0, Vec::new()) };
        let text = String::from_utf8_lossy(&base64_decode(raw).unwrap_or_default()).to_string();
        let head = text.split("\r\n\r\n").next().unwrap_or("").split("\n\n").next().unwrap_or("");
        let status = head.split_whitespace().nth(1).and_then(|code| code.parse().ok()).unwrap_or(300);
        return (status, raw_locations(head, &['\n']));
    }
    let status = params.get("responseCode").and_then(Value::as_u64).unwrap_or(0);
    (status, header_locations(params.get("responseHeaders"), params.get("binaryResponseHeaders")))
}

impl Cdp {
    pub(super) fn hop_refusal(&self, source: &str, target: &str) -> Option<String> {
        match target_refusal(&self.guard, source, target) {
            Ok(url) => self.redirector_protected(&url).then(|| format!("{}\n", self.guard.refusal_for_url(&url))),
            Err(reason) => Some(reason),
        }
    }

    pub(super) fn client_issued(&self, method: &str, params: &Value, session: Option<&str>) -> Result<bool, String> {
        let id = crate::shield::request_key(params).unwrap_or_default();
        let key = session.map(|session| (session.to_string(), id));
        let source = key.as_ref().and_then(|key| self.issued.urls.get(key)).cloned();
        match method {
            "Fetch.continueRequest" | "Network.continueInterceptedRequest" if params.get("url").is_some_and(|url| !url.is_null()) => {
                let url = params["url"].as_str().ok_or_else(|| self.refusal())?;
                match self.hop_refusal(url, url) {
                    Some(reason) => Err(reason),
                    None => Ok(true),
                }
            }
            "Fetch.continueWithAuth" => Ok(true),
            "Fetch.fulfillRequest" | "Fetch.continueResponse" | "Network.continueInterceptedRequest" => {
                let (mut status, mut locations) = redirect_answer(method, params);
                if method == "Fetch.continueResponse" {
                    if !["responseCode", "responseHeaders", "binaryResponseHeaders"].iter().any(|name| params.get(name).is_some()) { return Ok(false) }
                    let original = key.as_ref().and_then(|key| self.issued.responses.get(key)).ok_or_else(|| self.refusal())?;
                    if params.get("responseCode").is_none() { status = original.0; }
                    if params.get("responseHeaders").is_none() && params.get("binaryResponseHeaders").is_none() { locations = original.1.clone(); }
                }
                if !(300..400).contains(&status) || locations.is_empty() { return Ok(false) }
                let source = source.ok_or_else(|| self.refusal())?;
                match locations.iter().find_map(|location| self.hop_refusal(&source, location)) {
                    Some(reason) => Err(reason),
                    None => Ok(true),
                }
            }
            _ => Ok(false),
        }
    }

    fn armed_params(&self, session: &str) -> Value {
        let client = self.issued.client.get(session);
        let mut patterns = match client.and_then(|params| params.get("patterns")) {
            Some(Value::Array(list)) => list.clone(),
            None | Some(Value::Null) if client.is_some() => vec![json!({ "urlPattern": "*", "requestStage": "Request" })],
            _ => Vec::new(),
        };
        patterns.push(json!({ "urlPattern": "*", "requestStage": "Request" }));
        let auth = client.and_then(|params| params.get("handleAuthRequests")).and_then(Value::as_bool).unwrap_or(false);
        json!({ "patterns": patterns, "handleAuthRequests": auth })
    }

    fn arm_fetch(&mut self, session: &str) -> Result<(), String> {
        if self.issued.armed.contains(session) { return Ok(()) }
        let params = self.armed_params(session);
        self.issued.arming.insert(session.to_string());
        let result = self.raw_call("Fetch.enable", params, Some(session), ARM).map_err(|_| self.refusal());
        self.issued.arming.remove(session);
        if result.is_ok() { self.issued.armed.insert(session.to_string()); }
        let deferred = self.issued.deferred.remove(session).unwrap_or_default();
        for paused in deferred {
            if result.is_ok() { self.decide_inline(paused); }
            else { self.continue_plain(&paused.session_id, &paused.request_id); }
        }
        result.map(|_| ())
    }

    pub(super) fn defer_issued(&mut self, paused: &Paused) -> bool {
        if !self.issued.arming.contains(&paused.session_id) { return false }
        self.issued.deferred.entry(paused.session_id.clone()).or_default().push(paused.clone());
        true
    }

    pub(super) fn track_issued(&mut self, session: &str, id: &str) {
        let mut ids = self.request_aliases.get(id).cloned().unwrap_or_default();
        ids.insert(id.to_string());
        for id in ids { self.issued.chains.insert((session.to_string(), id)); }
    }

    pub(super) fn issue(&mut self, params: &Value, session: &str) -> Result<(), String> {
        self.arm_fetch(session)?;
        self.track_issued(session, &crate::shield::request_key(params).unwrap_or_default());
        Ok(())
    }

    fn client_matches(&self, session: &str, url: &str, kind: &str) -> bool {
        match self.issued.client.get(session).and_then(|params| params.get("patterns")) {
            _ if !self.issued.client.contains_key(session) => false,
            None | Some(Value::Null) => true,
            Some(Value::Array(list)) => list.iter().any(|pattern| {
                pattern.get("requestStage").and_then(Value::as_str).unwrap_or("Request") == "Request"
                    && pattern.get("resourceType").and_then(Value::as_str).filter(|wanted| !wanted.is_empty()).is_none_or(|wanted| kind_matches(wanted, kind))
                    && wildcard(pattern.get("urlPattern").and_then(Value::as_str).unwrap_or("*"), url)
            }),
            Some(_) => false,
        }
    }

    pub(super) fn guard_only(&self, paused: &Paused) -> bool {
        self.issued.armed.contains(&paused.session_id) && !paused.is_response() && !self.client_matches(&paused.session_id, &paused.url, &paused.resource_type)
    }

    pub(super) fn guard_auth(&mut self, value: &Value) -> bool {
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
        let params = &value["params"];
        let id = params.get("requestId").and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(url) = params.pointer("/request/url").and_then(Value::as_str) {
            self.issued.urls.entry((session.clone(), id.clone())).or_insert_with(|| url.to_string());
        }
        self.issued.auth.insert((session.clone(), id.clone()));
        let wanted = self.issued.client.get(&session).and_then(|params| params.get("handleAuthRequests")).and_then(Value::as_bool) == Some(true)
            && self.client_matches(&session, params.pointer("/request/url").and_then(Value::as_str).unwrap_or(""), params.get("resourceType").and_then(Value::as_str).unwrap_or(""));
        if !self.issued.armed.contains(&session) || wanted { return false }
        let _ = self.send("Fetch.continueWithAuth", json!({ "requestId": id, "authChallengeResponse": { "response": "Default" } }), Some(&session));
        true
    }

    pub(super) fn issued_hop(&mut self, value: &Value, paused: &Paused) -> bool {
        let session = paused.session_id.clone();
        let key = |id: &str| (session.clone(), id.to_string());
        self.issued.urls.insert(key(&paused.request_id), paused.url.clone());
        self.issued.auth.remove(&key(&paused.request_id));
        if let Some(status) = paused.status {
            let locations = paused.response_headers.iter().flatten().filter(|(name, _)| name.trim().eq_ignore_ascii_case("location")).map(|(_, value)| value.clone()).collect();
            self.issued.responses.insert(key(&paused.request_id), (u64::from(status), locations));
        }
        let redirected = value.pointer("/params/redirectedRequestId").and_then(Value::as_str);
        let tracked = [Some(paused.request_id.as_str()), paused.network_id.as_deref(), redirected].into_iter().flatten().any(|id| self.issued.chains.contains(&key(id)));
        if !tracked { return false }
        let refusal = if paused.is_response() {
            let locations: Vec<String> = paused.response_headers.iter().flatten().filter(|(name, _)| name.trim().eq_ignore_ascii_case("location")).map(|(_, value)| value.clone()).collect();
            paused.status.filter(|status| (300..400).contains(status)).and_then(|_| locations.iter().find_map(|location| self.hop_refusal(&paused.url, location)))
        } else {
            self.hop_refusal(&paused.full_url(), &paused.full_url())
        };
        let ids: Vec<String> = [Some(paused.request_id.clone()), paused.network_id.clone()].into_iter().flatten().collect();
        if refusal.is_none() {
            for id in &ids { self.track_issued(&session, id); }
            return false;
        }
        for id in ids { self.requests.insert(id, true); }
        self.fail_seen_network(value);
        self.settle(&session, &paused.request_id);
        let _ = self.send("Fetch.failRequest", json!({ "requestId": paused.request_id, "errorReason": "BlockedByClient" }), Some(&session));
        true
    }

    pub(super) fn fetch_command(&mut self, method: &str, params: Value, session: Option<&str>) -> (String, Value) {
        let Some(session) = session else { return (method.to_string(), params) };
        let armed = self.issued.armed.contains(session);
        match method {
            "Fetch.enable" => {
                self.issued.client.insert(session.to_string(), params.clone());
                if armed { return (method.to_string(), self.armed_params(session)) }
            }
            "Fetch.disable" => {
                self.issued.client.remove(session);
                if armed && !self.blocked.contains(session) {
                    self.release_handed(session);
                    return ("Fetch.enable".to_string(), self.armed_params(session));
                }
                self.issued.armed.remove(session);
            }
            _ => {}
        }
        (method.to_string(), params)
    }

    fn release_handed(&mut self, session: &str) {
        self.paused.retain(|paused| paused.session_id != session);
        let held: Vec<String> = self.handed.iter().filter(|(peer, _)| peer == session).map(|(_, id)| id.clone()).collect();
        for id in held {
            if self.issued.auth.contains(&(session.to_string(), id.clone())) {
                self.settle(session, &id);
                let _ = self.send("Fetch.continueWithAuth", json!({ "requestId": id, "authChallengeResponse": { "response": "Default" } }), Some(session));
            } else {
                self.continue_plain(session, &id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    include!("client_issued_test.rs");

    #[test]
    fn wildcards_follow_chrome_match_pattern() {
        assert!(wildcard("*", ""));
        assert!(wildcard("http://a.test/*", "http://a.test/x?y"));
        assert!(wildcard("http://a.test/?", "http://a.test/x"));
        assert!(!wildcard("http://a.test/?", "http://a.test/"));
        assert!(wildcard("*b*", "abc"));
        assert!(!wildcard("*b", "abc"));
        assert!(wildcard("a\\*c", "a*c"));
        assert!(!wildcard("a\\*c", "abc"));
        assert!(wildcard("a\\", "a\\"));
        assert!(!wildcard("abc", "ab"));
        assert!(!wildcard("ab", "abc"));
    }

    #[test]
    fn resource_kinds_group_like_chrome() {
        assert!(kind_matches("Document", "Document"));
        assert!(kind_matches("XHR", "Fetch"));
        assert!(kind_matches("Fetch", "Prefetch"));
        assert!(kind_matches("Other", "WebSocket"));
        assert!(!kind_matches("Other", "Script"));
        assert!(!kind_matches("Image", "Script"));
    }

    #[test]
    fn redirect_answers_read_every_header_form() {
        let fields = json!([{ "name": "Location", "value": "/a" }, { "name": "x", "value": "y" }]);
        let binary = crate::net::base64_encode(b"Location: /b\0X: y");
        assert_eq!(redirect_answer("Fetch.fulfillRequest", &json!({ "responseCode": 302, "responseHeaders": fields, "binaryResponseHeaders": binary })), (302, vec!["/a".to_string(), "/b".to_string()]));
        assert_eq!(redirect_answer("Fetch.fulfillRequest", &json!({ "binaryResponseHeaders": "!" })).1, vec![String::new()]);
        let raw = crate::net::base64_encode(b"HTTP/1.1 307 Temporary\r\nLocation: /c\r\n\r\nbody: x");
        assert_eq!(redirect_answer("Network.continueInterceptedRequest", &json!({ "rawResponse": raw })), (307, vec!["/c".to_string()]));
        assert_eq!(redirect_answer("Network.continueInterceptedRequest", &json!({})), (0, Vec::new()));
        assert_eq!(redirect_answer("Network.continueInterceptedRequest", &json!({ "rawResponse": crate::net::base64_encode(b"junk") })).0, 300);
    }
}
