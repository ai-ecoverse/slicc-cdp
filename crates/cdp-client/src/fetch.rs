use crate::cdp::{self, Cdp};
use crate::net::{base64_decode, base64_encode};
use crate::protected::Protected;
use serde_json::{json, Value};
use std::ops::{Deref, DerefMut};
use std::time::Duration;

pub const SIGNED_IN_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Signed in</title></head><body style=\"font-family:system-ui,sans-serif;margin:4rem auto;max-width:28rem;text-align:center\"><h1>Signed in</h1><p>You can close this tab.</p></body></html>";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Request,
    Response,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Stage::Request => "Request",
            Stage::Response => "Response",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub url: String,
    pub stage: Stage,
    pub resource_type: Option<String>,
}

impl Pattern {
    pub fn request(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            stage: Stage::Request,
            resource_type: None,
        }
    }

    pub fn response(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            stage: Stage::Response,
            resource_type: None,
        }
    }

    pub fn resource_type(mut self, kind: impl Into<String>) -> Self {
        self.resource_type = Some(kind.into());
        self
    }

    fn to_json(&self) -> Value {
        let mut value = json!({ "urlPattern": self.url, "requestStage": self.stage.as_str() });
        if let Some(kind) = &self.resource_type {
            value["resourceType"] = json!(kind);
        }
        value
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Paused {
    pub session_id: String,
    pub request_id: String,
    pub frame_id: String,
    pub resource_type: String,
    pub network_id: Option<String>,
    pub method: String,
    pub url: String,
    pub url_fragment: Option<String>,
    pub headers: Vec<(String, String)>,
    pub post_data: Option<Vec<u8>>,
    pub status: Option<u16>,
    pub status_text: Option<String>,
    pub response_headers: Option<Vec<(String, String)>>,
    pub error_reason: Option<String>,
}

impl Paused {
    pub fn from_event(event: &Value) -> Option<Self> {
        if event.get("method").and_then(|v| v.as_str()) != Some("Fetch.requestPaused") {
            return None;
        }
        let params = event.get("params")?;
        let request = params.get("request")?;
        let text =
            |value: &Value, key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
        let post_data = match request.get("postData").and_then(|v| v.as_str()) {
            Some(data) => Some(data.as_bytes().to_vec()),
            None => request
                .get("postDataEntries")
                .and_then(|v| v.as_array())
                .map(|entries| {
                    let mut bytes = Vec::new();
                    for entry in entries {
                        if let Some(chunk) = entry.get("bytes").and_then(|v| v.as_str()) {
                            bytes.extend(base64_decode(chunk).unwrap_or_default());
                        }
                    }
                    bytes
                }),
        };
        let status = params
            .get("responseStatusCode")
            .and_then(|v| v.as_u64())
            .and_then(|code| u16::try_from(code).ok());
        let response_headers = params
            .get("responseHeaders")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| Some((text(item, "name")?, text(item, "value")?)))
                    .collect()
            });
        Some(Self {
            session_id: text(event, "sessionId").unwrap_or_default(),
            request_id: text(params, "requestId")?,
            frame_id: text(params, "frameId").unwrap_or_default(),
            resource_type: text(params, "resourceType").unwrap_or_default(),
            network_id: text(params, "networkId"),
            method: text(request, "method").unwrap_or_else(|| "GET".to_string()),
            url: text(request, "url")?,
            url_fragment: text(request, "urlFragment").filter(|fragment| !fragment.is_empty()),
            headers: request
                .get("headers")
                .and_then(|v| v.as_object())
                .map(|object| {
                    object
                        .iter()
                        .map(|(name, value)| {
                            (name.clone(), value.as_str().unwrap_or("").to_string())
                        })
                        .collect()
                })
                .unwrap_or_default(),
            post_data,
            status,
            status_text: text(params, "responseStatusText"),
            response_headers,
            error_reason: text(params, "responseErrorReason"),
        })
    }

    pub fn is_response(&self) -> bool {
        self.status.is_some() || self.error_reason.is_some()
    }

    pub fn full_url(&self) -> String {
        match &self.url_fragment {
            Some(fragment) => format!("{}{fragment}", self.url),
            None => self.url.clone(),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        find(&self.headers, name)
    }

    pub fn response_header(&self, name: &str) -> Option<&str> {
        find(self.response_headers.as_deref()?, name)
    }

    pub fn location(&self) -> Option<String> {
        let raw = self.response_header("location")?;
        let base = url::Url::parse(&self.url).ok()?;
        Some(
            base.join(raw)
                .map(|url| url.to_string())
                .unwrap_or_else(|_| raw.to_string()),
        )
    }
}

fn find<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(have, _)| have.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    pub url: Option<String>,
    pub method: Option<String>,
    pub post_data: Option<Vec<u8>>,
    pub headers: Option<Vec<(String, String)>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResponseOverrides {
    pub status: Option<u16>,
    pub status_text: Option<String>,
    pub headers: Option<Vec<(String, String)>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fulfill {
    pub status: u16,
    pub status_text: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Fulfill {
    pub fn new(status: u16) -> Self {
        Self {
            status,
            status_text: None,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn html(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self::new(status)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(body)
    }

    pub fn signed_in() -> Self {
        Self::html(200, SIGNED_IN_HTML).header("Cache-Control", "no-store")
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorReason {
    Failed,
    Aborted,
    TimedOut,
    AccessDenied,
    ConnectionClosed,
    ConnectionReset,
    ConnectionRefused,
    ConnectionAborted,
    ConnectionFailed,
    NameNotResolved,
    InternetDisconnected,
    AddressUnreachable,
    BlockedByClient,
    BlockedByResponse,
}

impl ErrorReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorReason::Failed => "Failed",
            ErrorReason::Aborted => "Aborted",
            ErrorReason::TimedOut => "TimedOut",
            ErrorReason::AccessDenied => "AccessDenied",
            ErrorReason::ConnectionClosed => "ConnectionClosed",
            ErrorReason::ConnectionReset => "ConnectionReset",
            ErrorReason::ConnectionRefused => "ConnectionRefused",
            ErrorReason::ConnectionAborted => "ConnectionAborted",
            ErrorReason::ConnectionFailed => "ConnectionFailed",
            ErrorReason::NameNotResolved => "NameNotResolved",
            ErrorReason::InternetDisconnected => "InternetDisconnected",
            ErrorReason::AddressUnreachable => "AddressUnreachable",
            ErrorReason::BlockedByClient => "BlockedByClient",
            ErrorReason::BlockedByResponse => "BlockedByResponse",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let all = [
            ErrorReason::Failed,
            ErrorReason::Aborted,
            ErrorReason::TimedOut,
            ErrorReason::AccessDenied,
            ErrorReason::ConnectionClosed,
            ErrorReason::ConnectionReset,
            ErrorReason::ConnectionRefused,
            ErrorReason::ConnectionAborted,
            ErrorReason::ConnectionFailed,
            ErrorReason::NameNotResolved,
            ErrorReason::InternetDisconnected,
            ErrorReason::AddressUnreachable,
            ErrorReason::BlockedByClient,
            ErrorReason::BlockedByResponse,
        ];
        all.into_iter()
            .find(|reason| reason.as_str().eq_ignore_ascii_case(text))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Continue(Overrides),
    ContinueResponse(ResponseOverrides),
    Fulfill(Fulfill),
    Fail(ErrorReason),
}

impl Default for Decision {
    fn default() -> Self {
        Decision::Continue(Overrides::default())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Navigation {
    pub session_id: String,
    pub frame_id: String,
    pub main_frame: bool,
    pub url: String,
}

pub fn reaches_protected(guard: &Protected, paused: &Paused) -> bool {
    guard.url(&paused.full_url()) || paused.location().is_some_and(|url| guard.url(&url))
}

pub fn navigation(guard: &Protected, event: &Value) -> Option<Navigation> {
    if event.get("method").and_then(|v| v.as_str()) != Some("Page.frameNavigated") {
        return None;
    }
    let frame = event.pointer("/params/frame")?;
    let base = frame.get("url").and_then(|v| v.as_str())?;
    let fragment = frame
        .get("urlFragment")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let url = format!("{base}{fragment}");
    if guard.url(&url) || guard.url(base) {
        return None;
    }
    Some(Navigation {
        session_id: event
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        frame_id: frame
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        main_frame: frame.get("parentId").is_none(),
        url,
    })
}

fn header_list(headers: &[(String, String)]) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect(),
    )
}

fn refresh_target(value: &str) -> &str {
    let rest = value.trim_start_matches(|ch: char| ch.is_ascii_whitespace());
    let rest = rest.trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.');
    let rest = rest.trim_start_matches(|ch: char| ch.is_ascii_whitespace());
    let rest = rest
        .strip_prefix(';')
        .or_else(|| rest.strip_prefix(','))
        .unwrap_or(rest);
    let rest = rest.trim_start_matches(|ch: char| ch.is_ascii_whitespace());
    let rest = match rest.get(..3) {
        Some(head) if head.eq_ignore_ascii_case("url") => {
            let after = rest[3..].trim_start_matches(|ch: char| ch.is_ascii_whitespace());
            match after.strip_prefix('=') {
                Some(value) => value.trim_start_matches(|ch: char| ch.is_ascii_whitespace()),
                None => rest,
            }
        }
        _ => rest,
    };
    let rest = rest.trim_end_matches(|ch: char| ch.is_ascii_whitespace());
    match rest.chars().next() {
        Some(quote @ ('\'' | '"')) => {
            let inner = &rest[1..];
            inner.split(quote).next().unwrap_or(inner)
        }
        _ => rest,
    }
}

fn redirects_to_protected(guard: &Protected, base: &str, headers: &[(String, String)]) -> bool {
    let base = url::Url::parse(base).ok();
    let reaches = |target: &str| {
        let resolved = base
            .as_ref()
            .and_then(|base| base.join(target).ok())
            .map(|url| url.to_string())
            .unwrap_or_else(|| target.to_string());
        guard.url(&resolved) || guard.url(target)
    };
    headers.iter().any(|(name, value)| {
        if name.eq_ignore_ascii_case("location") {
            reaches(value) || guard.pattern(value)
        } else if name.eq_ignore_ascii_case("refresh") {
            reaches(refresh_target(value)) || guard.pattern(value)
        } else {
            false
        }
    })
}

pub fn command(
    guard: &Protected,
    paused: &Paused,
    decision: &Decision,
) -> Result<(&'static str, Value), String> {
    let id = paused.request_id.as_str();
    match decision {
        Decision::Continue(overrides) => {
            let mut params = json!({ "requestId": id });
            if let Some(url) = &overrides.url {
                if guard.url(url) {
                    return Err(format!("{}\n", guard.refusal()));
                }
                params["url"] = json!(url);
            }
            if let Some(method) = &overrides.method {
                params["method"] = json!(method);
            }
            if let Some(data) = &overrides.post_data {
                params["postData"] = json!(base64_encode(data));
            }
            if let Some(headers) = &overrides.headers {
                params["headers"] = header_list(headers);
            }
            Ok(("Fetch.continueRequest", params))
        }
        Decision::ContinueResponse(overrides) => {
            if !paused.is_response() {
                return Err(
                    "Fetch.continueResponse: the request is not at the response stage\n".into(),
                );
            }
            let mut params = json!({ "requestId": id });
            if let Some(status) = overrides.status {
                params["responseCode"] = json!(status);
            }
            if let Some(text) = &overrides.status_text {
                params["responsePhrase"] = json!(text);
            }
            if let Some(headers) = &overrides.headers {
                if redirects_to_protected(guard, &paused.url, headers) {
                    return Err(format!("{}\n", guard.refusal()));
                }
                params["responseHeaders"] = header_list(headers);
            }
            Ok(("Fetch.continueResponse", params))
        }
        Decision::Fulfill(fulfill) => {
            if redirects_to_protected(guard, &paused.url, &fulfill.headers) {
                return Err(format!("{}\n", guard.refusal()));
            }
            let mut params = json!({
                "requestId": id,
                "responseCode": fulfill.status,
                "responseHeaders": header_list(&fulfill.headers),
                "body": base64_encode(&fulfill.body),
            });
            if let Some(text) = &fulfill.status_text {
                params["responsePhrase"] = json!(text);
            }
            Ok(("Fetch.fulfillRequest", params))
        }
        Decision::Fail(reason) => Ok((
            "Fetch.failRequest",
            json!({ "requestId": id, "errorReason": reason.as_str() }),
        )),
    }
}

fn need_session(session_id: &str) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("Fetch needs a target session: browser-wide interception is refused\n".into());
    }
    Ok(())
}

pub fn enable(cdp: &mut Cdp, session_id: &str, patterns: &[Pattern]) -> Result<(), String> {
    need_session(session_id)?;
    if patterns.is_empty() {
        return Err("Fetch.enable needs at least one pattern\n".into());
    }
    if let Some(pattern) = patterns
        .iter()
        .find(|pattern| cdp.guard().pattern(&pattern.url))
    {
        return Err(format!("{} ({})\n", cdp.guard().refusal(), pattern.url));
    }
    let list: Vec<Value> = patterns.iter().map(Pattern::to_json).collect();
    cdp.call(
        "Fetch.enable",
        json!({ "patterns": list, "handleAuthRequests": false }),
        Some(session_id),
    )?;
    Ok(())
}

pub fn disable(cdp: &mut Cdp, session_id: &str) {
    cdp.forget_paused(session_id);
    if session_id.is_empty() {
        return;
    }
    let _ = cdp.call_for(
        "Fetch.disable",
        json!({}),
        Some(session_id),
        Duration::from_secs(5),
    );
}

pub fn decide(cdp: &mut Cdp, paused: &Paused, decision: &Decision) -> Result<(), String> {
    let (method, params) = command(cdp.guard(), paused, decision)?;
    cdp.call(method, params, Some(&paused.session_id))?;
    Ok(())
}

pub fn continue_request(
    cdp: &mut Cdp,
    paused: &Paused,
    overrides: Overrides,
) -> Result<(), String> {
    decide(cdp, paused, &Decision::Continue(overrides))
}

pub fn continue_response(
    cdp: &mut Cdp,
    paused: &Paused,
    overrides: ResponseOverrides,
) -> Result<(), String> {
    decide(cdp, paused, &Decision::ContinueResponse(overrides))
}

pub fn fulfill(cdp: &mut Cdp, paused: &Paused, response: Fulfill) -> Result<(), String> {
    decide(cdp, paused, &Decision::Fulfill(response))
}

pub fn fail(cdp: &mut Cdp, paused: &Paused, reason: ErrorReason) -> Result<(), String> {
    decide(cdp, paused, &Decision::Fail(reason))
}

pub fn response_body(cdp: &mut Cdp, paused: &Paused) -> Result<Vec<u8>, String> {
    if !paused.is_response() {
        return Err("Fetch.getResponseBody: the request is not at the response stage\n".into());
    }
    let result = cdp.call(
        "Fetch.getResponseBody",
        json!({ "requestId": paused.request_id }),
        Some(&paused.session_id),
    )?;
    let body = result.get("body").and_then(|v| v.as_str()).unwrap_or("");
    if result.get("base64Encoded").and_then(|v| v.as_bool()) == Some(true) {
        base64_decode(body)
    } else {
        Ok(body.as_bytes().to_vec())
    }
}

pub fn target_protected(cdp: &mut Cdp, target_id: &str) -> Result<bool, String> {
    cdp.target_protected(target_id, false)
}

pub fn refuse_target(cdp: &mut Cdp, target_id: &str) -> Result<(), String> {
    if target_protected(cdp, target_id)? {
        return Err(format!("{}\n", cdp.guard().refusal()));
    }
    Ok(())
}

pub struct Interception<'a> {
    cdp: &'a mut Cdp,
    session_id: String,
    owned: bool,
    done: bool,
}

impl<'a> Interception<'a> {
    pub fn attach(cdp: &'a mut Cdp, target_id: &str, patterns: &[Pattern]) -> Result<Self, String> {
        refuse_target(cdp, target_id)?;
        let session_id = cdp::attach(cdp, target_id)?;
        if let Err(message) = enable(cdp, &session_id, patterns) {
            cdp::detach(cdp, &session_id);
            return Err(message);
        }
        Ok(Self {
            cdp,
            session_id,
            owned: true,
            done: false,
        })
    }

    pub fn on_session(
        cdp: &'a mut Cdp,
        target_id: &str,
        session_id: &str,
        patterns: &[Pattern],
    ) -> Result<Self, String> {
        refuse_target(cdp, target_id)?;
        enable(cdp, session_id, patterns)?;
        Ok(Self {
            cdp,
            session_id: session_id.to_string(),
            owned: false,
            done: false,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn next(&mut self, timeout: Duration) -> Result<Option<Paused>, String> {
        let session = self.session_id.clone();
        self.cdp.next_paused(Some(&session), timeout)
    }

    pub fn decide(&mut self, paused: &Paused, decision: &Decision) -> Result<(), String> {
        decide(self.cdp, paused, decision)
    }

    pub fn response_body(&mut self, paused: &Paused) -> Result<Vec<u8>, String> {
        response_body(self.cdp, paused)
    }

    pub fn finish(mut self) {
        self.close();
    }

    fn close(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        disable(self.cdp, &self.session_id);
        if self.owned {
            cdp::detach(self.cdp, &self.session_id);
        }
    }
}

impl Deref for Interception<'_> {
    type Target = Cdp;

    fn deref(&self) -> &Cdp {
        self.cdp
    }
}

impl DerefMut for Interception<'_> {
    fn deref_mut(&mut self) -> &mut Cdp {
        self.cdp
    }
}

impl Drop for Interception<'_> {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdp::Transport;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::time::Instant;

    type Reply = Box<dyn FnMut(&Value) -> Vec<Value>>;

    struct Fake {
        log: Rc<RefCell<Vec<Value>>>,
        inbox: VecDeque<String>,
        reply: Reply,
        closed: Option<String>,
    }

    impl Transport for Fake {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            if let Some(closed) = &self.closed {
                return Err(closed.clone());
            }
            let value: Value = serde_json::from_str(text).unwrap();
            self.log.borrow_mut().push(value.clone());
            for out in (self.reply)(&value) {
                self.inbox.push_back(out.to_string());
            }
            Ok(())
        }

        fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
            if let Some(closed) = &self.closed {
                return Err(closed.clone());
            }
            let next = self.inbox.pop_front();
            if let Some(text) = &next {
                if let Some(reason) = serde_json::from_str::<Value>(text)
                    .ok()
                    .and_then(|value| value.get("close").and_then(|v| v.as_str()).map(str::to_string))
                {
                    self.closed = Some(reason.clone());
                    return Err(reason);
                }
            }
            Ok(next)
        }
    }

    fn fake(inbox: Vec<Value>, reply: Reply) -> (Cdp, Rc<RefCell<Vec<Value>>>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        let transport = Fake {
            log: log.clone(),
            inbox: inbox.into_iter().map(|value| value.to_string()).collect(),
            reply,
            closed: None,
        };
        (Cdp::from_transport(Box::new(transport)), log)
    }

    fn ok(message: &Value) -> Value {
        json!({ "id": message["id"], "result": {} })
    }

    fn methods(log: &Rc<RefCell<Vec<Value>>>) -> Vec<String> {
        log.borrow()
            .iter()
            .map(|item| item["method"].as_str().unwrap_or("").to_string())
            .collect()
    }

    fn paused_event(session: &str, id: &str, url: &str) -> Value {
        json!({
            "method": "Fetch.requestPaused",
            "sessionId": session,
            "params": {
                "requestId": id,
                "frameId": "F1",
                "resourceType": "Document",
                "networkId": "N1",
                "request": {
                    "url": url,
                    "method": "GET",
                    "headers": { "Accept": "text/html", "Cookie": "a=b" }
                }
            }
        })
    }

    #[test]
    fn a_paused_request_and_response_are_read() {
        let mut event = paused_event("S", "R1", "https://idp.example/cb?code=abc");
        event["params"]["request"]["urlFragment"] = json!("#access_token=xyz");
        event["params"]["request"]["method"] = json!("POST");
        event["params"]["request"]["postDataEntries"] =
            json!([{ "bytes": base64_encode(b"a=1&") }, { "bytes": base64_encode(b"b=2") }]);
        let paused = Paused::from_event(&event).unwrap();
        assert_eq!(paused.session_id, "S");
        assert_eq!(paused.request_id, "R1");
        assert_eq!(paused.method, "POST");
        assert_eq!(
            paused.full_url(),
            "https://idp.example/cb?code=abc#access_token=xyz"
        );
        assert_eq!(paused.header("cookie"), Some("a=b"));
        assert_eq!(paused.post_data.as_deref(), Some(&b"a=1&b=2"[..]));
        assert!(!paused.is_response());
        assert_eq!(paused.location(), None);
        let mut response = paused_event("S", "R2", "https://idp.example/authorize");
        response["params"]["responseStatusCode"] = json!(302);
        response["params"]["responseStatusText"] = json!("Found");
        response["params"]["responseHeaders"] =
            json!([{ "name": "Location", "value": "/done#id_token=t&state=s" }]);
        let paused = Paused::from_event(&response).unwrap();
        assert!(paused.is_response());
        assert_eq!(paused.status, Some(302));
        assert_eq!(
            paused.response_header("location"),
            Some("/done#id_token=t&state=s")
        );
        assert_eq!(
            paused.location().as_deref(),
            Some("https://idp.example/done#id_token=t&state=s")
        );
        assert!(Paused::from_event(&json!({ "method": "Page.loadEventFired" })).is_none());
    }

    #[test]
    fn a_decider_answers_a_pause_that_arrives_inside_a_call() {
        let navigate = Rc::new(RefCell::new(None));
        let held = navigate.clone();
        let (mut cdp, log) = fake(
            Vec::new(),
            Box::new(move |message| match message["method"].as_str().unwrap() {
                "Page.navigate" => {
                    *held.borrow_mut() = Some(message["id"].clone());
                    vec![paused_event("S", "R1", "https://app.example/api/users")]
                }
                "Fetch.fulfillRequest" => vec![
                    ok(message),
                    json!({ "id": held.borrow().clone().unwrap(), "result": { "frameId": "F1", "loaderId": "L" } }),
                ],
                _ => vec![ok(message)],
            }),
        );
        cdp.set_decider(
            "S",
            Some(Box::new(|paused: &Paused| {
                assert_eq!(paused.url, "https://app.example/api/users");
                Decision::Fulfill(
                    Fulfill::new(201)
                        .header("Content-Type", "application/json")
                        .body("[]"),
                )
            })),
        );
        let result = cdp
            .call(
                "Page.navigate",
                json!({ "url": "https://app.example/" }),
                Some("S"),
            )
            .unwrap();
        assert_eq!(result["loaderId"], "L");
        let sent = log.borrow();
        let fulfil = sent
            .iter()
            .find(|item| item["method"] == "Fetch.fulfillRequest")
            .unwrap();
        assert_eq!(fulfil["sessionId"], "S");
        assert_eq!(fulfil["params"]["requestId"], "R1");
        assert_eq!(fulfil["params"]["responseCode"], 201);
        assert_eq!(fulfil["params"]["body"], base64_encode(b"[]"));
        assert_eq!(
            fulfil["params"]["responseHeaders"],
            json!([{ "name": "Content-Type", "value": "application/json" }])
        );
        assert!(cdp.events().is_empty());
        assert!(cdp.take_fetch_errors().is_empty());
    }

    #[test]
    fn a_failed_decision_falls_back_to_continue() {
        let (mut cdp, log) = fake(
            vec![paused_event("S", "R1", "https://app.example/x")],
            Box::new(|message| match message["method"].as_str().unwrap() {
                "Fetch.fulfillRequest" => {
                    vec![json!({ "id": message["id"], "error": { "message": "Invalid header" } })]
                }
                _ => vec![ok(message)],
            }),
        );
        cdp.set_decider(
            "S",
            Some(Box::new(|_: &Paused| {
                Decision::Fulfill(Fulfill::new(200).header("Bad\n", "x"))
            })),
        );
        cdp.call("Runtime.evaluate", json!({}), Some("S")).unwrap();
        assert!(cdp
            .pump(Instant::now() + Duration::from_millis(10))
            .unwrap());
        assert_eq!(
            methods(&log),
            [
                "Runtime.evaluate",
                "Fetch.fulfillRequest",
                "Fetch.continueRequest"
            ]
        );
        assert_eq!(cdp.take_fetch_errors(), ["Fetch: Invalid header\n"]);
    }

    #[test]
    fn seven_and_vault_requests_are_never_handed_out() {
        let mut location = paused_event("S", "R3", "https://idp.example/authorize");
        location["params"]["responseStatusCode"] = json!(302);
        location["params"]["responseHeaders"] =
            json!([{ "name": "location", "value": "https://vault.sliccy.ai/cb#token=t" }]);
        let mut xhr = paused_event("S", "R2", "blob:https://vault.sliccy.ai/0b9e");
        xhr["params"]["resourceType"] = json!("XHR");
        let (mut cdp, log) = fake(
            vec![
                paused_event("S", "R1", "https://seven.sliccy.ai/api"),
                xhr,
                location,
                paused_event("S", "R4", "https://app.example/ok"),
            ],
            Box::new(|message| vec![ok(message)]),
        );
        let got = cdp
            .next_paused(Some("S"), Duration::from_millis(50))
            .unwrap()
            .unwrap();
        assert_eq!(got.request_id, "R4");
        assert!(cdp
            .next_paused(Some("S"), Duration::from_millis(10))
            .unwrap()
            .is_none());
        let sent = log.borrow();
        let continued: Vec<&str> = sent
            .iter()
            .filter(|item| item["method"] == "Fetch.continueRequest")
            .map(|item| item["params"]["requestId"].as_str().unwrap())
            .collect();
        assert_eq!(continued, ["R2"]);
        let failed: Vec<(&str, &str)> = sent
            .iter()
            .filter(|item| item["method"] == "Fetch.failRequest")
            .map(|item| {
                (
                    item["params"]["requestId"].as_str().unwrap(),
                    item["params"]["errorReason"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(failed, [("R1", "BlockedByClient"), ("R3", "BlockedByClient")]);
        assert!(sent.iter().all(|item| item["params"].get("url").is_none()));
        assert!(cdp.events().is_empty());
    }

    #[test]
    fn a_decider_never_sees_a_protected_request() {
        let (mut cdp, log) = fake(
            vec![paused_event(
                "S",
                "R1",
                "https://www.sliccy.ai/auth/callback?code=c",
            )],
            Box::new(|message| vec![ok(message)]),
        );
        cdp.set_decider(
            "S",
            Some(Box::new(|_: &Paused| {
                panic!("a protected request reached the decider")
            })),
        );
        cdp.call("Runtime.evaluate", json!({}), Some("S")).unwrap();
        assert_eq!(methods(&log), ["Runtime.evaluate", "Fetch.failRequest"]);
    }

    fn auth_event(session: &str, id: &str, url: &str) -> Value {
        json!({
            "method": "Fetch.authRequired",
            "sessionId": session,
            "params": {
                "requestId": id,
                "frameId": "F1",
                "resourceType": "Fetch",
                "request": { "url": url, "method": "GET", "headers": {} },
                "authChallenge": { "source": "Server", "origin": url, "scheme": "basic", "realm": "r" }
            }
        })
    }

    fn refused(result: Result<Value, String>) -> bool {
        result.is_err_and(|message| message == crate::cdp::UNSEEN)
    }

    #[test]
    fn an_issued_id_is_good_on_its_session_until_it_is_answered() {
        let mut xhr = paused_event("S", "R1", "https://app.example/api");
        xhr["params"]["resourceType"] = json!("XHR");
        let mut response = xhr.clone();
        response["params"]["responseStatusCode"] = json!(200);
        let (mut cdp, log) = fake(vec![xhr], Box::new(|message| vec![ok(message)]));
        let got = cdp.next_paused(Some("S"), Duration::from_millis(50)).unwrap().unwrap();
        assert_eq!(got.request_id, "R1");
        assert!(refused(cdp.call("Fetch.fulfillRequest", json!({ "requestId": "R2", "responseCode": 200 }), Some("S"))));
        assert!(refused(cdp.call("Fetch.continueRequest", json!({ "requestId": "R1" }), Some("T"))));
        assert!(refused(cdp.call("Fetch.continueRequest", json!({}), Some("S"))));
        cdp.call("Fetch.getResponseBody", json!({ "requestId": "R1" }), Some("S")).unwrap();
        cdp.call("Fetch.continueRequest", json!({ "requestId": "R1" }), Some("S")).unwrap();
        for (method, params) in [
            ("Fetch.failRequest", json!({ "requestId": "R1", "errorReason": "Failed" })),
            ("Fetch.fulfillRequest", json!({ "requestId": "R1", "responseCode": 200 })),
            ("Fetch.continueResponse", json!({ "requestId": "R1" })),
            ("Fetch.continueWithAuth", json!({ "requestId": "R1", "authChallengeResponse": { "response": "Default" } })),
        ] {
            assert!(refused(cdp.call(method, params, Some("S"))), "{method}");
        }
        assert!(refused(cdp.notify("Fetch.continueRequest", json!({ "requestId": "R1" }), Some("S")).map(|_| Value::Null)));
        assert_eq!(methods(&log), ["Fetch.getResponseBody", "Fetch.continueRequest"]);
        let (mut cdp, log) = fake(vec![response], Box::new(|message| vec![ok(message)]));
        let again = cdp.next_paused(Some("S"), Duration::from_millis(50)).unwrap().unwrap();
        assert!(again.is_response());
        cdp.call("Fetch.continueResponse", json!({ "requestId": "R1" }), Some("S")).unwrap();
        assert!(refused(cdp.call("Fetch.continueResponse", json!({ "requestId": "R1" }), Some("S"))));
        assert_eq!(methods(&log), ["Fetch.continueResponse"]);
    }

    #[test]
    fn an_id_a_decider_answered_is_not_issued_again() {
        let (mut cdp, log) = fake(
            vec![paused_event("S", "R1", "https://app.example/x")],
            Box::new(|message| vec![ok(message)]),
        );
        cdp.set_decider("S", Some(Box::new(|_: &Paused| Decision::default())));
        cdp.call("Runtime.evaluate", json!({}), Some("S")).unwrap();
        assert!(refused(cdp.call("Fetch.fulfillRequest", json!({ "requestId": "R1", "responseCode": 200 }), Some("S"))));
        assert_eq!(methods(&log), ["Runtime.evaluate", "Fetch.continueRequest"]);
    }

    #[test]
    fn a_protected_id_is_never_issued_even_when_guessed() {
        let mut xhr = paused_event("S", "R1", "https://seven.sliccy.ai/data");
        xhr["params"]["resourceType"] = json!("XHR");
        let (mut cdp, log) = fake(vec![xhr], Box::new(|message| vec![ok(message)]));
        assert!(cdp.next_paused(Some("S"), Duration::from_millis(50)).unwrap().is_none());
        for method in [
            "Fetch.fulfillRequest",
            "Fetch.failRequest",
            "Fetch.continueRequest",
            "Fetch.continueWithAuth",
            "Fetch.continueResponse",
            "Fetch.getResponseBody",
            "Fetch.takeResponseBodyAsStream",
            "Network.continueInterceptedRequest",
            "Network.getResponseBodyForInterception",
        ] {
            assert!(refused(cdp.call(method, json!({ "requestId": "R1", "interceptionId": "R1" }), Some("S"))), "{method}");
        }
        assert_eq!(methods(&log), ["Fetch.continueRequest"]);
        let old = cdp
            .call("Network.setRequestInterception", json!({ "patterns": [{ "urlPattern": "*" }] }), Some("S"))
            .unwrap_err();
        assert!(old.starts_with("slicc: refused"), "{old}");
        assert_eq!(methods(&log), ["Fetch.continueRequest"]);
    }

    #[test]
    fn a_protected_auth_challenge_gets_the_default_and_its_id_is_refused() {
        let (mut cdp, log) = fake(
            vec![
                auth_event("S", "A1", "https://vault.sliccy.ai/auth"),
                auth_event("S", "A2", "https://app.example/auth"),
            ],
            Box::new(|message| vec![ok(message)]),
        );
        while cdp.pump(Instant::now() + Duration::from_millis(10)).unwrap() {}
        let sent = log.borrow().clone();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["method"], "Fetch.continueWithAuth");
        assert_eq!(sent[0]["params"]["requestId"], "A1");
        assert_eq!(sent[0]["params"]["authChallengeResponse"], json!({ "response": "Default" }));
        let delivered: Vec<&Value> = cdp.events().iter().collect();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0]["params"]["requestId"], "A2");
        let answer = json!({ "response": "ProvideCredentials", "username": "u", "password": "p" });
        assert!(refused(cdp.call("Fetch.continueWithAuth", json!({ "requestId": "A1", "authChallengeResponse": answer }), Some("S"))));
        cdp.call("Fetch.continueWithAuth", json!({ "requestId": "A2", "authChallengeResponse": answer }), Some("S")).unwrap();
        assert!(refused(cdp.call("Fetch.continueWithAuth", json!({ "requestId": "A2", "authChallengeResponse": answer }), Some("S"))));
        assert_eq!(methods(&log), ["Fetch.continueWithAuth", "Fetch.continueWithAuth"]);
    }

    fn panic_decider() -> Option<crate::cdp::Decider> {
        Some(Box::new(|paused: &Paused| {
            panic!("the agent saw {} paused", paused.url)
        }))
    }

    #[test]
    fn a_tab_that_commits_seven_stops_intercepting_and_hands_out_nothing() {
        let mut leak = paused_event("S", "R1", "https://api.model.example/v1/chat");
        leak["params"]["resourceType"] = json!("Fetch");
        let (mut cdp, log) = fake(
            vec![
                json!({
                    "method": "Page.frameNavigated",
                    "sessionId": "S",
                    "params": { "frame": { "id": "F", "url": "https://seven.sliccy.ai/os/", "securityOrigin": "https://seven.sliccy.ai" } }
                }),
                leak,
                json!({
                    "method": "Network.requestWillBeSent",
                    "sessionId": "S",
                    "params": { "requestId": "N1", "documentURL": "https://seven.sliccy.ai/os/", "request": { "url": "https://api.model.example/v1/chat", "headers": { "Authorization": "Bearer KEY" } } }
                }),
            ],
            Box::new(|message| vec![ok(message)]),
        );
        cdp.set_decider("S", panic_decider());
        while cdp.pump(Instant::now() + Duration::from_millis(10)).unwrap() {}
        assert!(cdp.session_blocked("S"));
        assert_eq!(
            methods(&log),
            ["Fetch.disable", "Target.detachFromTarget", "Fetch.continueRequest"]
        );
        assert!(log.borrow()[2]["params"].get("headers").is_none());
        assert!(cdp.events().is_empty(), "{:?}", cdp.events());
        assert!(cdp.call("Runtime.evaluate", json!({}), Some("S")).unwrap_err().starts_with("slicc: refused"));
    }

    #[test]
    fn a_worker_attaches_without_a_page_guard_and_a_page_without_page_domains_still_does() {
        let (mut cdp, log) = fake(
            Vec::new(),
            Box::new(|message| match message["method"].as_str().unwrap() {
                "Target.getTargetInfo" => {
                    let kind = if message["params"]["targetId"] == "W" { "service_worker" } else { "page" };
                    vec![json!({ "id": message["id"], "result": { "targetInfo": { "targetId": message["params"]["targetId"], "type": kind, "url": "https://app.example/sw.js" } } })]
                }
                "Target.attachToTarget" => {
                    vec![json!({ "id": message["id"], "result": { "sessionId": format!("S{}", message["id"]) } })]
                }
                "Page.enable" | "Runtime.enable" => vec![json!({ "id": message["id"], "error": { "message": "'Page.enable' wasn't found" } })],
                _ => vec![ok(message)],
            }),
        );
        crate::cdp::attach(&mut cdp, "W").unwrap();
        assert_eq!(methods(&log), ["Target.getTargetInfo", "Target.attachToTarget"]);
        crate::cdp::attach(&mut cdp, "P").unwrap();
        assert!(methods(&log).contains(&"Network.setBypassServiceWorker".to_string()));
    }

    #[test]
    fn a_takeover_close_frame_wins_over_the_guard_refusal() {
        let close = "websocket closed 1011 another page took over slicc-node's /cdp (superseded-by-new-cdp-client)\n";
        let (mut cdp, _log) = fake(
            Vec::new(),
            Box::new(move |message| match message["method"].as_str().unwrap() {
                "Target.getTargetInfo" => vec![json!({ "id": message["id"], "result": { "targetInfo": { "targetId": "T", "type": "page", "url": "https://app.example/" } } })],
                "Target.attachToTarget" => vec![json!({ "id": message["id"], "result": { "sessionId": "S" } })],
                "Target.setDiscoverTargets" => vec![json!({ "close": close })],
                _ => vec![ok(message)],
            }),
        );
        assert_eq!(crate::cdp::attach(&mut cdp, "T").unwrap_err(), close);
        assert_eq!(
            cdp.call("Runtime.evaluate", json!({ "expression": "1" }), Some("S")).unwrap_err(),
            close
        );
        let (mut live, _log) = fake(
            Vec::new(),
            Box::new(|message| match message["method"].as_str().unwrap() {
                "Target.getTargetInfo" => vec![json!({ "id": message["id"], "result": { "targetInfo": { "targetId": "T", "type": "page", "url": "https://app.example/" } } })],
                "Target.attachToTarget" => vec![json!({ "id": message["id"], "result": { "sessionId": format!("S{}", message["id"]) } })],
                "Fetch.enable" => vec![json!({ "id": message["id"], "error": { "message": "Fetch is not available" } })],
                _ => vec![ok(message)],
            }),
        );
        assert_eq!(crate::cdp::attach(&mut live, "T").unwrap_err(), crate::cdp::UNGUARDED);
    }

    #[test]
    fn a_target_that_landed_on_the_vault_is_reported_protected() {
        let (mut cdp, _log) = fake(
            Vec::new(),
            Box::new(|message| vec![json!({ "id": message["id"], "result": { "targetInfo": { "targetId": "T", "type": "page", "url": "https://vault.sliccy.ai/landing" } } })]),
        );
        assert!(!cdp.is_protected_target("T"));
        assert!(cdp.target_protected("T", false).unwrap());
        assert!(cdp.is_protected_target("T"));
        assert!(cdp.call("Target.attachToTarget", json!({ "targetId": "T", "flatten": true }), None).is_err());
    }

    #[test]
    fn requests_from_a_protected_frame_or_referrer_are_continued_unseen() {
        let mut framed = paused_event("S", "R1", "https://api.example/a");
        framed["params"]["frameId"] = json!("CHILD");
        framed["params"]["resourceType"] = json!("XHR");
        let mut referred = paused_event("S", "R2", "https://api.example/b");
        referred["params"]["frameId"] = json!("OTHER");
        referred["params"]["resourceType"] = json!("XHR");
        referred["params"]["request"]["headers"] = json!({ "Referer": "https://vault.sliccy.ai/unlock" });
        let mut plain = paused_event("S", "R3", "https://api.example/c");
        plain["params"]["frameId"] = json!("OTHER");
        plain["params"]["resourceType"] = json!("XHR");
        let (mut cdp, log) = fake(
            vec![
                json!({
                    "method": "Page.frameNavigated",
                    "sessionId": "S",
                    "params": { "frame": { "id": "CHILD", "parentId": "MAIN", "url": "https://vault.sliccy.ai/frame" } }
                }),
                framed,
                referred,
                plain,
            ],
            Box::new(|message| vec![ok(message)]),
        );
        let got = cdp.next_paused(Some("S"), Duration::from_millis(50)).unwrap().unwrap();
        assert_eq!(got.request_id, "R3");
        let continued: Vec<String> = log
            .borrow()
            .iter()
            .filter(|item| item["method"] == "Fetch.continueRequest")
            .map(|item| item["params"]["requestId"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(continued, ["R1", "R2"]);
        assert!(cdp.call("Fetch.getResponseBody", json!({ "requestId": "R1" }), Some("S")).is_err());
    }

    #[test]
    fn enable_refuses_protected_patterns_and_browser_wide_interception() {
        let (mut cdp, log) = fake(Vec::new(), Box::new(|message| vec![ok(message)]));
        let refused = enable(
            &mut cdp,
            "S",
            &[Pattern::request("https://vault.sliccy.ai/*")],
        )
        .unwrap_err();
        assert!(
            refused.starts_with("slicc: refused: target is on seven's origin"),
            "{refused}"
        );
        let refused = enable(&mut cdp, "S", &[Pattern::response("*://*.sliccy.ai/*")]).unwrap_err();
        assert!(refused.starts_with("slicc: refused"), "{refused}");
        assert!(enable(&mut cdp, "", &[Pattern::request("*")])
            .unwrap_err()
            .contains("browser-wide"));
        assert!(enable(&mut cdp, "S", &[]).is_err());
        assert!(log.borrow().is_empty());
        enable(
            &mut cdp,
            "S",
            &[
                Pattern::request("*"),
                Pattern::response("https://idp.example/*").resource_type("Document"),
            ],
        )
        .unwrap();
        let sent = log.borrow();
        assert_eq!(sent[0]["method"], "Fetch.enable");
        assert_eq!(sent[0]["sessionId"], "S");
        assert_eq!(
            sent[0]["params"]["patterns"],
            json!([
                { "urlPattern": "*", "requestStage": "Request" },
                { "urlPattern": "https://idp.example/*", "requestStage": "Response", "resourceType": "Document" }
            ])
        );
    }

    #[test]
    fn decisions_cannot_send_a_tab_to_seven() {
        let guard = Protected::default();
        let paused = Paused::from_event(&paused_event("S", "R1", "https://app.example/")).unwrap();
        let fulfil =
            Decision::Fulfill(Fulfill::new(302).header("Location", "https://seven.sliccy.ai/"));
        assert!(command(&guard, &paused, &fulfil)
            .unwrap_err()
            .starts_with("slicc: refused"));
        let refresh = Decision::Fulfill(
            Fulfill::html(200, "x").header("Refresh", "0; url=https://vault.sliccy.ai/"),
        );
        assert!(command(&guard, &paused, &refresh).is_err());
        let rewrite = Decision::Continue(Overrides {
            url: Some("https://vault.sliccy.ai/".into()),
            ..Overrides::default()
        });
        assert!(command(&guard, &paused, &rewrite).is_err());
        let fine = Decision::Fulfill(Fulfill::new(302).header("Location", "/next"));
        assert_eq!(
            command(&guard, &paused, &fine).unwrap().0,
            "Fetch.fulfillRequest"
        );
        let (method, params) = command(
            &guard,
            &paused,
            &Decision::Fail(ErrorReason::BlockedByClient),
        )
        .unwrap();
        assert_eq!(method, "Fetch.failRequest");
        assert_eq!(params["errorReason"], "BlockedByClient");
        let (method, params) = command(
            &guard,
            &paused,
            &Decision::Continue(Overrides {
                method: Some("POST".into()),
                post_data: Some(b"x=1".to_vec()),
                headers: Some(vec![("X-A".into(), "1".into())]),
                ..Overrides::default()
            }),
        )
        .unwrap();
        assert_eq!(method, "Fetch.continueRequest");
        assert_eq!(params["postData"], base64_encode(b"x=1"));
        assert_eq!(params["headers"], json!([{ "name": "X-A", "value": "1" }]));
        assert!(command(
            &guard,
            &paused,
            &Decision::ContinueResponse(ResponseOverrides::default())
        )
        .is_err());
        assert_eq!(ErrorReason::parse("timedout"), Some(ErrorReason::TimedOut));
        assert_eq!(ErrorReason::parse("nope"), None);
    }

    #[test]
    fn signed_in_page_is_small_html() {
        let page = Fulfill::signed_in();
        assert_eq!(page.status, 200);
        assert!(page
            .headers
            .iter()
            .any(|(name, value)| name == "Content-Type" && value.starts_with("text/html")));
        assert!(String::from_utf8(page.body)
            .unwrap()
            .contains("You can close this tab"));
    }

    #[test]
    fn an_interception_reads_a_response_and_cleans_up_on_drop() {
        let mut response = paused_event("S1", "R1", "https://idp.example/authorize");
        response["params"]["responseStatusCode"] = json!(200);
        let queued = Rc::new(RefCell::new(Some(response)));
        let attaches = Rc::new(RefCell::new(0));
        let (mut cdp, log) = fake(
            Vec::new(),
            Box::new(move |message| match message["method"].as_str().unwrap() {
                "Target.getTargetInfo" => vec![
                    json!({ "id": message["id"], "result": { "targetInfo": { "targetId": "T1", "url": "about:blank" } } }),
                ],
                "Target.attachToTarget" => {
                    *attaches.borrow_mut() += 1;
                    let session = if *attaches.borrow() == 1 { "S1" } else { "G1" };
                    vec![json!({ "id": message["id"], "result": { "sessionId": session } })]
                }
                "Fetch.enable" if message["sessionId"] == "S1" => {
                    vec![ok(message), queued.borrow_mut().take().unwrap()]
                }
                "Fetch.getResponseBody" => vec![
                    json!({ "id": message["id"], "result": { "body": base64_encode(b"<p>hi</p>"), "base64Encoded": true } }),
                ],
                _ => vec![ok(message)],
            }),
        );
        {
            let mut interception = Interception::attach(
                &mut cdp,
                "T1",
                &[Pattern::response("https://idp.example/*")],
            )
            .unwrap();
            assert_eq!(interception.session_id(), "S1");
            let paused = interception
                .next(Duration::from_millis(50))
                .unwrap()
                .unwrap();
            assert_eq!(interception.response_body(&paused).unwrap(), b"<p>hi</p>");
            interception
                .decide(&paused, &Decision::Fulfill(Fulfill::signed_in()))
                .unwrap();
        }
        assert_eq!(
            methods(&log),
            [
                "Target.getTargetInfo",
                "Target.getTargetInfo",
                "Target.attachToTarget",
                "Target.setDiscoverTargets",
                "Target.attachToTarget",
                "Fetch.enable",
                "Network.enable",
                "Network.setBypassServiceWorker",
                "Page.enable",
                "Runtime.enable",
                "Fetch.enable",
                "Fetch.getResponseBody",
                "Fetch.fulfillRequest",
                "Fetch.disable",
                "Target.detachFromTarget"
            ]
        );
    }

    #[test]
    fn an_interception_refuses_a_seven_tab_or_its_blank_popup() {
        let (mut cdp, log) = fake(
            Vec::new(),
            Box::new(|message| {
                let url = match message["params"]["targetId"].as_str() {
                    Some("SEVEN") => "https://lars.sliccy.ai/",
                    Some("POPUP") => "about:blank",
                    _ => "",
                };
                let opener = if message["params"]["targetId"] == "POPUP" {
                    json!("SEVEN")
                } else {
                    Value::Null
                };
                vec![
                    json!({ "id": message["id"], "result": { "targetInfo": { "url": url, "openerId": opener } } }),
                ]
            }),
        );
        for target in ["SEVEN", "POPUP"] {
            let refused = Interception::attach(&mut cdp, target, &[Pattern::request("*")])
                .err()
                .unwrap();
            assert!(refused.starts_with("slicc: refused"), "{target}: {refused}");
        }
        assert!(methods(&log)
            .iter()
            .all(|method| method == "Target.getTargetInfo"));
    }

    #[test]
    fn navigation_keeps_the_fragment_and_hides_seven() {
        let guard = Protected::default();
        let event = json!({
            "method": "Page.frameNavigated",
            "sessionId": "S",
            "params": { "frame": { "id": "F", "url": "https://app.example/cb", "urlFragment": "#access_token=t" } }
        });
        let found = navigation(&guard, &event).unwrap();
        assert_eq!(found.url, "https://app.example/cb#access_token=t");
        assert!(found.main_frame);
        let seven = json!({
            "method": "Page.frameNavigated",
            "sessionId": "S",
            "params": { "frame": { "id": "F", "url": "https://seven.sliccy.ai/auth/callback", "urlFragment": "#t" } }
        });
        assert!(navigation(&guard, &seven).is_none());
    }

    fn live(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
    }

    fn encoded(text: &str) -> String {
        url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
    }

    fn evaluate(cdp: &mut Cdp, session: &str, expression: &str) -> Value {
        cdp.call(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true }),
            Some(session),
        )
        .unwrap()
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null)
    }

    fn poll(cdp: &mut Cdp, session: &str, expression: &str, want: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut last = Value::Null;
        while Instant::now() < deadline {
            last = evaluate(cdp, session, expression);
            if last == json!(want) {
                return;
            }
            let _ = cdp.pump(Instant::now() + Duration::from_millis(100));
        }
        panic!("{expression} stayed {last}, wanted {want}");
    }

    fn new_tab(cdp: &mut Cdp, url: &str) -> String {
        cdp.call(
            "Target.createTarget",
            json!({ "url": url, "background": true }),
            None,
        )
        .unwrap()["targetId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn start_script(cdp: &mut Cdp, session: &str, expression: &str) {
        let _ = cdp.notify(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true }),
            Some(session),
        );
    }

    #[test]
    #[ignore = "needs SLICC_FETCH_LIVE_WS from test/integration/fetch.test.mjs"]
    fn live_chromium_interception() {
        let ws = live("SLICC_FETCH_LIVE_WS");
        let app = live("SLICC_FETCH_LIVE_APP");
        let idp = live("SLICC_FETCH_LIVE_IDP");
        let seven = live("SLICC_FETCH_LIVE_SEVEN");
        let mut cdp = Cdp::connect(&ws).unwrap();
        let tab = new_tab(&mut cdp, "about:blank");

        let implicit = format!("{app}/cb#access_token=T&state=S");
        {
            let mut seen = Interception::attach(
                &mut cdp,
                &tab,
                &[Pattern::response(format!("{idp}/authorize*"))],
            )
            .unwrap();
            let session = seen.session_id().to_string();
            seen.call("Page.enable", json!({}), Some(&session)).unwrap();
            let start = format!("{idp}/authorize?to={}", encoded(&implicit));
            let _ = seen.notify("Page.navigate", json!({ "url": start }), Some(&session));
            let paused = seen
                .next(Duration::from_secs(10))
                .unwrap()
                .expect("the redirect paused");
            assert!(paused.is_response());
            assert_eq!(paused.status, Some(302));
            assert_eq!(paused.location().as_deref(), Some(implicit.as_str()));
            seen.decide(&paused, &Decision::Fulfill(Fulfill::signed_in()))
                .unwrap();
            poll(&mut seen, &session, "document.title", "Signed in");
            poll(
                &mut seen,
                &session,
                "location.href",
                &format!("{idp}/authorize?to={}", encoded(&implicit)),
            );
        }

        let code = format!("{app}/cb?code=C&state=S");
        {
            let mut seen =
                Interception::attach(&mut cdp, &tab, &[Pattern::request(format!("{app}/cb*"))])
                    .unwrap();
            let session = seen.session_id().to_string();
            seen.call("Page.enable", json!({}), Some(&session)).unwrap();
            let start = format!("{idp}/authorize?to={}", encoded(&code));
            let _ = seen.notify("Page.navigate", json!({ "url": start }), Some(&session));
            let paused = seen
                .next(Duration::from_secs(10))
                .unwrap()
                .expect("the callback paused");
            assert!(!paused.is_response());
            assert_eq!(paused.url, code);
            assert_eq!(paused.method, "GET");
            assert_eq!(paused.resource_type, "Document");
            seen.decide(&paused, &Decision::Fulfill(Fulfill::signed_in()))
                .unwrap();
            poll(&mut seen, &session, "document.title", "Signed in");
        }

        {
            let session = cdp::attach(&mut cdp, &tab).unwrap();
            cdp.call("Page.enable", json!({}), Some(&session)).unwrap();
            let mark = cdp.events().len();
            let landing = format!("{app}/landing#id_token=J");
            let start = format!("{idp}/js-redirect?to={}", encoded(&landing));
            let _ = cdp.notify("Page.navigate", json!({ "url": start }), Some(&session));
            let guard = cdp.guard().clone();
            let event = cdp
                .wait_event(mark, Duration::from_secs(10), |event| {
                    navigation(&guard, event).is_some_and(|found| found.url.contains("/landing"))
                })
                .unwrap()
                .expect("the script redirect navigated");
            let found = navigation(&guard, &event).unwrap();
            assert_eq!(found.url, landing);
            assert!(found.main_frame);
            assert_eq!(found.session_id, session);
            cdp::detach(&mut cdp, &session);
        }

        {
            let api = format!("{app}/api/headers");
            let mut seen = Interception::attach(
                &mut cdp,
                &tab,
                &[
                    Pattern::request(format!("{api}*")),
                    Pattern::response(format!("{api}*")),
                ],
            )
            .unwrap();
            let session = seen.session_id().to_string();
            start_script(
                &mut seen,
                &session,
                &format!(
                    "fetch({}).then((r) => r.text()).then((t) => {{ window.__got = t; }})",
                    json!(api)
                ),
            );
            let request = seen
                .next(Duration::from_secs(10))
                .unwrap()
                .expect("the request paused");
            assert!(!request.is_response());
            let mut headers = request.headers.clone();
            headers.push(("X-Added".into(), "1".into()));
            seen.decide(
                &request,
                &Decision::Continue(Overrides {
                    headers: Some(headers),
                    ..Overrides::default()
                }),
            )
            .unwrap();
            let response = seen
                .next(Duration::from_secs(10))
                .unwrap()
                .expect("the response paused");
            assert_eq!(response.status, Some(200));
            let body = String::from_utf8(seen.response_body(&response).unwrap()).unwrap();
            assert_eq!(body, "x-added=1 x-drop=none");
            seen.decide(
                &response,
                &Decision::ContinueResponse(ResponseOverrides::default()),
            )
            .unwrap();
            poll(&mut seen, &session, "window.__got || ''", &body);
        }

        {
            let mut seen =
                Interception::attach(&mut cdp, &tab, &[Pattern::request("*/blocked*")]).unwrap();
            let session = seen.session_id().to_string();
            start_script(&mut seen, &session, "fetch('/blocked').then(() => 'ok', () => 'failed').then((t) => { window.__blocked = t; })");
            let paused = seen
                .next(Duration::from_secs(10))
                .unwrap()
                .expect("the blocked request paused");
            seen.decide(&paused, &Decision::Fail(ErrorReason::BlockedByClient))
                .unwrap();
            poll(&mut seen, &session, "window.__blocked || ''", "failed");
        }

        {
            let mut seen = Interception::attach(&mut cdp, &tab, &[Pattern::request("*")]).unwrap();
            let session = seen.session_id().to_string();
            start_script(
                &mut seen,
                &session,
                &format!("fetch({}).then((r) => r.text(), (e) => 'error:' + e).then((t) => {{ window.__seven = t; }})", json!(format!("{seven}/ping"))),
            );
            assert!(seen.next(Duration::from_secs(2)).unwrap().is_none());
            poll(&mut seen, &session, "window.__seven || ''", "pong");
            let refused = enable(
                &mut seen,
                &session,
                &[Pattern::request(format!("{seven}/*"))],
            )
            .unwrap_err();
            assert!(refused.starts_with("slicc: refused"), "{refused}");
        }

        let refused = cdp
            .call("Target.createTarget", json!({ "url": format!("{seven}/landing") }), None)
            .unwrap_err();
        assert!(refused.starts_with("slicc: refused"), "{refused}");
        let seven_tab = crate::raw::Raw::open(&ws).create(&format!("{seven}/landing"));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match Interception::attach(&mut cdp, &seven_tab, &[Pattern::request("*")]) {
                Err(refused) => {
                    assert!(refused.starts_with("slicc: refused"), "{refused}");
                    break;
                }
                Ok(seen) => {
                    seen.finish();
                    assert!(Instant::now() < deadline, "a seven tab was intercepted");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
        for target in [tab, seven_tab] {
            let _ = cdp.call("Target.closeTarget", json!({ "targetId": target }), None);
        }
    }
}
