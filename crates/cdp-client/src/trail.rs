use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

static CONNECTIONS: AtomicUsize = AtomicUsize::new(0);

pub fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("SLICC_FETCH_TRAIL").is_ok_and(|value| value == "1"))
}

fn start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

#[derive(Default)]
pub struct Trail {
    connection: usize,
    sent: BTreeMap<i64, String>,
}

impl Trail {
    pub fn new() -> Self {
        if !on() { return Self::default() }
        start();
        Self { connection: CONNECTIONS.fetch_add(1, Ordering::Relaxed) + 1, sent: BTreeMap::new() }
    }

    pub fn note(&self, line: impl FnOnce() -> String) {
        if self.connection > 0 {
            eprintln!("slicc-trail +{}ms c{} {}", start().elapsed().as_millis(), self.connection, line());
        }
    }

    pub fn sent(&mut self, id: i64, method: &str, params: &Value, session: Option<&str>) {
        if self.connection == 0 || !traced(method) { return }
        self.sent.insert(id, method.to_string());
        let mut line = format!("send #{id} {method} session={}", word(session.unwrap_or("-")));
        for key in ["requestId", "sessionId", "targetId", "errorReason", "responseCode"] {
            if let Some(value) = params.get(key) { line.push_str(&format!(" {key}={}", scalar(value))); }
        }
        if let Some(patterns) = params.get("patterns").and_then(Value::as_array) {
            let shown: Vec<String> = patterns.iter().map(|pattern| format!("{}/{}/{}",
                scalar(pattern.get("requestStage").unwrap_or(&Value::Null)),
                scalar(pattern.get("resourceType").unwrap_or(&Value::Null)),
                if pattern.get("urlPattern").and_then(Value::as_str) == Some("*") { "any" } else { "some" })).collect();
            line.push_str(&format!(" patterns={}", shown.join(",")));
        }
        self.note(|| line);
    }

    pub fn received(&mut self, text: &str) {
        if self.connection == 0 { return }
        let Ok(value) = serde_json::from_str::<Value>(text) else { self.note(|| "recv unparsed".into()); return };
        if let Some(id) = value.get("id").and_then(Value::as_i64) {
            let Some(method) = self.sent.remove(&id) else { return };
            let line = match value.get("error") {
                Some(error) => format!("reply #{id} {method} error code={} message={}", scalar(error.get("code").unwrap_or(&Value::Null)), prose(error.get("message").and_then(Value::as_str).unwrap_or(""))),
                None => format!("reply #{id} {method} ok{}", value.pointer("/result/errorText").and_then(Value::as_str).map(|text| format!(" errorText={}", word(text))).unwrap_or_default()),
            };
            self.note(|| line);
            return;
        }
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        let params = value.get("params").unwrap_or(&Value::Null);
        let field = |pointer: &str| params.pointer(pointer).map_or_else(|| "-".to_string(), scalar);
        let session = word(value.get("sessionId").and_then(Value::as_str).unwrap_or("-"));
        let line = match method {
            "Fetch.requestPaused" | "Fetch.authRequired" => format!(
                "event {method} session={session} requestId={} networkId={} stage={} type={} status={} error={} frame={} redirected={}",
                field("/requestId"), field("/networkId"),
                if params.get("responseStatusCode").is_some() || params.get("responseErrorReason").is_some() { "Response" } else { "Request" },
                field("/resourceType"), field("/responseStatusCode"), field("/responseErrorReason"), field("/frameId"), field("/redirectedRequestId")),
            "Target.attachedToTarget" => format!("event {method} session={session} child={} target={} type={} waiting={}", field("/sessionId"), field("/targetInfo/targetId"), field("/targetInfo/type"), field("/waitingForDebugger")),
            "Target.detachedFromTarget" => format!("event {method} session={session} child={} target={}", field("/sessionId"), field("/targetId")),
            "Target.targetDestroyed" | "Target.targetCrashed" => format!("event {method} target={}", field("/targetId")),
            "Page.frameNavigated" if params.pointer("/frame/parentId").is_none() => format!("event {method} session={session} frame={} loader={}", field("/frame/id"), field("/frame/loaderId")),
            "Page.frameStartedNavigating" | "Page.frameRequestedNavigation" | "Page.frameStartedLoading" | "Page.frameStoppedLoading" | "Page.loadEventFired" =>
                format!("event {method} session={session} frame={} loader={} kind={}", field("/frameId"), field("/loaderId"), field("/navigationType")),
            "Network.requestWillBeSent" if params.get("type").and_then(Value::as_str) == Some("Document") =>
                format!("event {method} session={session} requestId={} loader={} frame={} redirect={}", field("/requestId"), field("/loaderId"), field("/frameId"), params.get("redirectResponse").is_some()),
            "Network.responseReceived" if params.get("type").and_then(Value::as_str) == Some("Document") =>
                format!("event {method} session={session} requestId={} loader={} status={}", field("/requestId"), field("/loaderId"), field("/response/status")),
            "Network.loadingFailed" => format!("event {method} session={session} requestId={} error={} blocked={}", field("/requestId"), field("/errorText"), field("/blockedReason")),
            "Inspector.detached" | "Inspector.targetCrashed" => format!("event {method} session={session} reason={}", field("/reason")),
            _ => return,
        };
        self.note(|| line);
    }
}

fn traced(method: &str) -> bool {
    method.starts_with("Fetch.") || matches!(method, "Target.attachToTarget" | "Target.detachFromTarget" | "Page.navigate" | "Page.getFrameTree")
}

fn withheld(text: &str) -> bool {
    text.contains(['/', '?', '=', '@', '#', '%'])
}

pub fn word(text: &str) -> String {
    if withheld(text) { return "withheld".into() }
    let kept: String = text.chars().filter(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '_' | '-')).take(48).collect();
    if kept.is_empty() { "-".into() } else { kept }
}

fn prose(text: &str) -> String {
    if withheld(text) { return "withheld".into() }
    let kept: String = text.chars().filter(|char| char.is_ascii_alphabetic() || *char == ' ').take(80).collect();
    if kept.trim().is_empty() { "-".into() } else { kept.trim().replace(' ', "_") }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => word(text),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => "-".into(),
    }
}
