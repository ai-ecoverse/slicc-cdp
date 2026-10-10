use crate::net::Ws;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub struct Cdp {
    ws: Ws,
    next_id: i64,
    events: Vec<Value>,
}

impl Cdp {
    pub fn connect(url: &str) -> Result<Self, String> {
        Ok(Self {
            ws: crate::net::ws_connect(url)?,
            next_id: 0,
            events: Vec::new(),
        })
    }

    pub fn events(&self) -> &[Value] {
        &self.events
    }

    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, String> {
        self.call_for(method, params, session_id, Duration::from_secs(30))
    }

    pub fn call_for(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(session_id) = session_id {
            message["sessionId"] = json!(session_id);
        }
        self.ws.send_text(&message.to_string())?;
        let deadline = Instant::now() + timeout;
        loop {
            let text = match self.ws.recv_text(deadline)? {
                Some(text) => text,
                None => return Err(format!("{method} timed out\n")),
            };
            let value: Value = serde_json::from_str(&text)
                .map_err(|err| format!("CDP response was not JSON: {err}\n"))?;
            if value.get("id").and_then(|item| item.as_i64()) == Some(id) {
                if let Some(error) = value.get("error") {
                    let text = error
                        .get("message")
                        .and_then(|item| item.as_str())
                        .unwrap_or("CDP error");
                    return Err(format!("{method}: {text}\n"));
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            if value.get("method").is_some() {
                self.events.push(value);
            }
        }
    }

    pub fn notify(&mut self, method: &str, params: Value, session_id: Option<&str>) {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(session_id) = session_id {
            message["sessionId"] = json!(session_id);
        }
        let _ = self.ws.send_text(&message.to_string());
    }

    pub fn wait_load(&mut self, session_id: &str) -> Result<(), String> {
        if self.events.iter().any(|event| is_load(event, session_id)) {
            return Ok(());
        }
        let probe = self.call_for(
            "Runtime.evaluate",
            json!({
                "expression": "document.readyState",
                "returnByValue": true,
            }),
            Some(session_id),
            Duration::from_secs(1),
        );
        let settled = match &probe {
            Ok(result) => probe_is_settled(Ok(result
                .pointer("/result/value")
                .and_then(|value| value.as_str())))?,
            Err(err) => probe_is_settled(Err(err.as_str()))?,
        };
        if settled {
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if self.events.iter().any(|event| is_load(event, session_id)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Ok(());
            }
            match self.ws.recv_text(deadline)? {
                Some(text) => {
                    if let Ok(value) = serde_json::from_str::<Value>(&text) {
                        if value.get("method").is_some() {
                            self.events.push(value);
                        }
                    }
                }
                None => return Ok(()),
            }
        }
    }
}

fn probe_is_settled(outcome: Result<Option<&str>, &str>) -> Result<bool, String> {
    match outcome {
        Ok(Some("loading")) | Ok(Some("interactive")) => Ok(false),
        Ok(_) => Ok(true),
        Err(err)
            if err.starts_with("Runtime.evaluate timed out")
                || err.starts_with("Runtime.evaluate:") =>
        {
            Ok(true)
        }
        Err(err) => Err(err.to_string()),
    }
}

fn is_load(event: &Value, session_id: &str) -> bool {
    if event.get("sessionId").and_then(|v| v.as_str()) != Some(session_id) {
        return false;
    }
    match event.get("method").and_then(|v| v.as_str()) {
        Some("Page.loadEventFired") => true,
        Some("Page.lifecycleEvent") => {
            event.pointer("/params/name").and_then(|v| v.as_str()) == Some("load")
        }
        _ => false,
    }
}

pub fn attach(cdp: &mut Cdp, target_id: &str) -> Result<String, String> {
    let result = cdp.call(
        "Target.attachToTarget",
        json!({ "targetId": target_id, "flatten": true }),
        None,
    )?;
    result
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "Target.attachToTarget returned no sessionId\n".to_string())
}

pub fn detach(cdp: &mut Cdp, session_id: &str) {
    let _ = cdp.call(
        "Target.detachFromTarget",
        json!({ "sessionId": session_id }),
        None,
    );
}

#[derive(Clone, Debug, Default)]
pub struct ActionNote {
    pub kind: String,
    pub tab: Option<String>,
    pub target: Option<String>,
    pub url: Option<String>,
    pub key: Option<String>,
    pub length: Option<u64>,
    pub method: Option<String>,
    pub agent: Option<String>,
}

impl ActionNote {
    pub fn start_params(&self) -> Value {
        let mut params = serde_json::Map::new();
        params.insert("phase".into(), json!("start"));
        params.insert("kind".into(), json!(self.kind));
        insert_text(&mut params, "tab", self.tab.as_deref());
        insert_text(&mut params, "target", self.target.as_deref());
        if let Some(url) = self.url.as_deref().and_then(public_url) {
            params.insert("url".into(), json!(url));
        }
        insert_text(&mut params, "key", self.key.as_deref());
        if let Some(length) = self.length {
            params.insert("length".into(), json!(length));
        }
        insert_text(&mut params, "method", self.method.as_deref());
        insert_text(&mut params, "agent", self.agent.as_deref());
        Value::Object(params)
    }

    pub fn end_params(&self, ok: bool, stderr: &str) -> Value {
        let mut params = serde_json::Map::new();
        params.insert("phase".into(), json!("end"));
        params.insert("ok".into(), json!(ok));
        if !ok {
            if let Some(error) = action_error(&self.kind, stderr) {
                params.insert("error".into(), json!(error));
            }
        }
        insert_text(&mut params, "tab", self.tab.as_deref());
        Value::Object(params)
    }
}

fn insert_text(params: &mut serde_json::Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        params.insert(key.to_string(), json!(value));
    }
}

pub fn first_stderr_line(stderr: &str) -> Option<&str> {
    stderr.lines().next().filter(|line| !line.is_empty())
}

fn public_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let without_fragment = raw.split_once('#').map(|(head, _)| head).unwrap_or(raw);
    let without_query = without_fragment
        .split_once('?')
        .map(|(head, _)| head)
        .unwrap_or(without_fragment);
    if without_query.is_empty() {
        return None;
    }
    if is_about_blank(without_query) {
        return Some("about:blank".to_string());
    }
    if let Some(rest) = without_query.strip_prefix("//") {
        let (host, path) = host_and_path(rest)?;
        return Some(format!("//{host}{path}"));
    }
    let (scheme, rest) = without_query.split_once(':')?;
    if !is_scheme(scheme) || !scheme_allowed(scheme) {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    let Some(authority) = rest.strip_prefix("//") else {
        return Some(format!("{scheme}:"));
    };
    let (host, path) = host_and_path(authority)?;
    Some(format!("{scheme}://{host}{path}"))
}

fn scheme_allowed(scheme: &str) -> bool {
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http"
            | "https"
            | "ws"
            | "wss"
            | "file"
            | "data"
            | "javascript"
            | "blob"
            | "about"
            | "chrome"
            | "chrome-extension"
            | "view-source"
            | "ftp"
    )
}

fn is_about_blank(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("about") && rest.eq_ignore_ascii_case("blank")
}

fn is_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    match chars.next() {
        Some(ch) if ch.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '-' || ch == '.')
}

fn host_and_path(rest: &str) -> Option<(&str, &str)> {
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    let host = match authority.rfind('@') {
        Some(index) => &authority[index + 1..],
        None => authority,
    };
    if host.is_empty() { None } else { Some((host, path)) }
}

fn action_error(kind: &str, stderr: &str) -> Option<&'static str> {
    if kind == "eval" {
        return None;
    }
    let line = first_stderr_line(stderr).unwrap_or("").to_ascii_lowercase();
    if line.contains("timed out") {
        return Some("timeout");
    }
    if line.contains("websocket closed")
        || line.contains("websocket connection closed")
        || line.contains("websocket write failed")
        || line.contains("websocket read failed")
        || line.contains("connection reset")
        || line.contains("broken pipe")
    {
        return Some("connection lost");
    }
    if line.contains("no snapshot") || line.contains("likely stale") {
        return Some("no snapshot");
    }
    if line.contains("unknown ref")
        || line.contains("element not found")
        || line.contains("not an element ref")
        || line.contains("could not resolve")
        || line.contains("no backend node")
    {
        return Some("element not found");
    }
    if line.contains("no open tab is on") {
        return Some("not allowed");
    }
    if line.contains("unknown tab")
        || line.contains("--tab")
        || line.contains("no open tabs")
        || line.contains("explicit tab")
    {
        return Some("unknown tab");
    }
    if line.starts_with("navigate:") || line.contains("page.navigate") {
        return Some("navigation failed");
    }
    Some("failed")
}

pub fn slicc_agent() -> Option<String> {
    std::env::var("SLICC_AGENT")
        .ok()
        .filter(|value| !value.is_empty())
}

pub fn open(start: crate::connect::Start, runtime: Option<&str>) -> Result<Cdp, String> {
    let socket = match start {
        crate::connect::Start::Direct(url) => url,
        crate::connect::Start::Discover(url) => {
            let response = match crate::net::http_get(&url) {
                Ok(response) => response,
                Err(message) => {
                    if message.starts_with("HTTP ") || message.contains("does not launch Chrome") {
                        return Err(message);
                    }
                    return Err(format!(
                        "{message}This CLI does not launch Chrome. Pass --cdp or set SLICC_CDP_URL.\n"
                    ));
                }
            };
            let body = String::from_utf8_lossy(&response.body).to_string();
            crate::connect::websocket_url_from_version(&body)?
        }
    };
    let socket = crate::connect::append_runtime(&socket, runtime);
    Cdp::connect(&socket)
}

#[cfg(test)]
mod tests {
    use super::{first_stderr_line, probe_is_settled, ActionNote};
    use serde_json::json;

    #[test]
    fn a_finished_document_does_not_wait_for_load() {
        assert_eq!(probe_is_settled(Ok(Some("complete"))).unwrap(), true);
        assert_eq!(probe_is_settled(Ok(None)).unwrap(), true);
        assert_eq!(
            probe_is_settled(Err("Runtime.evaluate timed out\n")).unwrap(),
            true
        );
        assert_eq!(
            probe_is_settled(Err("Runtime.evaluate: 'Runtime.evaluate' wasn't found\n")).unwrap(),
            true
        );
    }

    #[test]
    fn a_loading_document_still_waits() {
        assert_eq!(probe_is_settled(Ok(Some("loading"))).unwrap(), false);
        assert_eq!(probe_is_settled(Ok(Some("interactive"))).unwrap(), false);
    }

    #[test]
    fn a_closed_socket_is_not_treated_as_loaded() {
        for err in [
            "websocket closed 1011 the browser went away\n",
            "websocket connection closed\n",
            "websocket write failed: broken pipe\n",
            "websocket read failed: connection reset\n",
            "CDP response was not JSON: eof\n",
        ] {
            let reported = probe_is_settled(Err(err)).unwrap_err();
            assert_eq!(reported, err);
        }
    }

    #[test]
    fn action_start_and_end_keep_the_panel_fields() {
        let click = ActionNote {
            kind: "click".to_string(),
            tab: Some("1234".to_string()),
            target: Some("button \"Sign in\"".to_string()),
            agent: Some("cone:cone-1".to_string()),
            ..ActionNote::default()
        };
        assert_eq!(
            click.start_params(),
            json!({
                "phase": "start",
                "kind": "click",
                "tab": "1234",
                "target": "button \"Sign in\"",
                "agent": "cone:cone-1"
            })
        );
        let failed = click.end_params(
            false,
            "No snapshot available. Run \"snapshot\" first.\nignored\n",
        );
        assert_eq!(
            failed,
            json!({
                "phase": "end",
                "ok": false,
                "error": "no snapshot",
                "tab": "1234"
            })
        );
        assert!(failed.get("agent").is_none());
        assert!(failed.get("kind").is_none());
        assert!(failed.get("target").is_none());
        let open = ActionNote {
            kind: "open".to_string(),
            url: Some("http://a.test/".to_string()),
            ..ActionNote::default()
        };
        assert!(open.start_params().get("tab").is_none());
        assert_eq!(open.start_params()["url"], "http://a.test/");
        let mut opened = open.clone();
        opened.tab = Some("TAB1".to_string());
        assert_eq!(opened.end_params(true, "")["tab"], "TAB1");
        assert!(opened.end_params(true, "").get("error").is_none());
    }

    #[test]
    fn action_params_omit_typed_text_eval_source_and_request_secrets() {
        let secret = "secret-text";
        let fill = ActionNote {
            kind: "fill".to_string(),
            tab: Some("T".to_string()),
            target: Some("textbox \"Email\"".to_string()),
            length: Some(secret.chars().count() as u64),
            ..ActionNote::default()
        };
        let fill_params = fill.start_params();
        assert_eq!(fill_params["length"], 11);
        assert!(!fill_params.to_string().contains(secret));
        assert!(fill_params.get("value").is_none());
        let typed = "héllo";
        let type_note = ActionNote {
            kind: "type".to_string(),
            length: Some(typed.chars().count() as u64),
            ..ActionNote::default()
        };
        assert_eq!(type_note.start_params()["length"], 5);
        assert!(!type_note.start_params().to_string().contains(typed));
        let source = "document.cookie";
        let eval_note = ActionNote {
            kind: "eval".to_string(),
            tab: Some("T".to_string()),
            ..ActionNote::default()
        };
        let eval_params = eval_note.start_params();
        assert!(!eval_params.to_string().contains(source));
        assert!(eval_params.get("expression").is_none());
        let request = ActionNote {
            kind: "request".to_string(),
            tab: Some("T1".to_string()),
            method: Some("POST".to_string()),
            url: Some("https://app.example/api/me".to_string()),
            ..ActionNote::default()
        };
        let request_params = request.start_params();
        assert_eq!(
            request_params,
            json!({
                "phase": "start",
                "kind": "request",
                "tab": "T1",
                "method": "POST",
                "url": "https://app.example/api/me"
            })
        );
        assert!(!request_params.to_string().contains("Authorization"));
        assert!(!request_params.to_string().contains(secret));
        let press = ActionNote {
            kind: "press".to_string(),
            key: Some("Enter".to_string()),
            ..ActionNote::default()
        };
        assert_eq!(press.start_params()["key"], "Enter");
        assert!(ActionNote::default().start_params().get("agent").is_none());
        assert_eq!(first_stderr_line("only-line"), Some("only-line"));
        assert_eq!(first_stderr_line("\n"), None);
    }

    #[test]
    fn action_urls_and_errors_drop_page_secrets() {
        let secret = "https://user:pass@app.example/form?q=QUERYSECRET&tok=x#FRAGSECRET";
        let open = ActionNote {
            kind: "open".to_string(),
            url: Some(secret.to_string()),
            ..ActionNote::default()
        };
        let started = open.start_params();
        assert_eq!(started["url"], "https://app.example/form");
        assert_clean(&started);
        let goto = ActionNote {
            kind: "goto".to_string(),
            url: Some(secret.to_string()),
            ..ActionNote::default()
        };
        assert_eq!(goto.start_params()["url"], "https://app.example/form");
        let posted = ActionNote {
            kind: "request".to_string(),
            method: Some("POST".to_string()),
            url: Some(secret.to_string()),
            ..ActionNote::default()
        };
        let request = posted.start_params();
        assert_eq!(request["method"], "POST");
        assert_eq!(request["url"], "https://app.example/form");
        assert_clean(&request);
        let gotten = ActionNote {
            kind: "request".to_string(),
            method: Some("GET".to_string()),
            url: Some("http://user:p%40ss@[::1]:8443/a/b?x=1#y".to_string()),
            ..ActionNote::default()
        };
        assert_eq!(gotten.start_params()["url"], "http://[::1]:8443/a/b");
        let relative = ActionNote {
            kind: "request".to_string(),
            method: Some("GET".to_string()),
            url: Some("//user:pass@app.example/path?q=QUERYSECRET#FRAGSECRET".to_string()),
            ..ActionNote::default()
        };
        let relative_params = relative.start_params();
        assert_eq!(relative_params["url"], "//app.example/path");
        assert_clean(&relative_params);
        assert_eq!(
            ActionNote {
                kind: "open".to_string(),
                url: Some("about:blank".to_string()),
                ..ActionNote::default()
            }
            .start_params()["url"],
            "about:blank"
        );
        let opaque = [
            (
                "goto",
                "data:text/html,<title>DATASECRET</title>",
                Some("data:"),
                "DATASECRET",
            ),
            (
                "open",
                "data:text/html,DATASECRET2",
                Some("data:"),
                "DATASECRET2",
            ),
            (
                "goto",
                "javascript:void('JSSECRET')",
                Some("javascript:"),
                "JSSECRET",
            ),
            (
                "open",
                "blob:https://example.com/BLOBSECRET",
                Some("blob:"),
                "BLOBSECRET",
            ),
            ("goto", "about:blank?q=BLANKSECRET", Some("about:blank"), "BLANKSECRET"),
            ("open", "ABOUT:blank?q=BLANKSECRET", Some("about:blank"), "BLANKSECRET"),
            ("goto", "About:blank", Some("about:blank"), "BLANKSECRET"),
            (
                "open",
                "DATA:text/html,DATASECRET",
                Some("data:"),
                "DATASECRET",
            ),
            (
                "open",
                "about:srcdoc,ABOUTSECRET",
                Some("about:"),
                "ABOUTSECRET",
            ),
            ("goto", "file:/tmp/PATHSECRET", Some("file:"), "PATHSECRET"),
            (
                "open",
                "https:example.com/PATHSECRET",
                Some("https:"),
                "PATHSECRET",
            ),
            ("goto", "example.com/PATHSECRET", None, "PATHSECRET"),
            ("open", "not a url PATHSECRET", None, "PATHSECRET"),
            ("goto", "HOSTSECRET3:8080/path", None, "HOSTSECRET3"),
            ("open", "host:8080/x", None, "host:"),
            (
                "goto",
                "USERSECRET5:PASSSECRET5@127.0.0.1/form",
                None,
                "USERSECRET5",
            ),
            (
                "open",
                "USERSECRET5:PASSSECRET5@127.0.0.1/form",
                None,
                "PASSSECRET5",
            ),
            ("goto", "user:pw@host/x", None, "user:"),
            ("open", "foo:bar", None, "foo:"),
            ("goto", "foo://example.com/FOOSECRET", None, "FOOSECRET"),
            (
                "open",
                "view-source:https://example.com/VIEWSECRET",
                Some("view-source:"),
                "VIEWSECRET",
            ),
        ];
        for (kind, raw, expect, secret) in opaque {
            let note = ActionNote {
                kind: kind.to_string(),
                url: Some(raw.to_string()),
                ..ActionNote::default()
            };
            let params = note.start_params();
            match expect {
                Some(url) => assert_eq!(params["url"], url, "{raw}"),
                None => assert!(params.get("url").is_none(), "{raw} {params}"),
            }
            assert!(!params.to_string().contains(secret), "{raw} {params}");
        }
        let eval_failed = ActionNote {
            kind: "eval".to_string(),
            tab: Some("T".to_string()),
            ..ActionNote::default()
        };
        let eval_end = eval_failed.end_params(false, "Error: EVALSECRET-in-error\n");
        assert_eq!(eval_end["ok"], false);
        assert!(eval_end.get("error").is_none());
        assert_clean(&eval_end);
        let cases = [
            (
                "click",
                "Unknown ref \"e1\". Available: e2\n",
                "element not found",
            ),
            (
                "screenshot",
                "No snapshot available. Run \"snapshot\" first.\n",
                "no snapshot",
            ),
            (
                "screenshot",
                "screenshot: could not resolve element e1 to a visible box — the snapshot is likely stale\n",
                "no snapshot",
            ),
            (
                "click",
                "Error: --tab <targetId> is required. Run 'playwright-cli tab-list' to get tab IDs.\n",
                "unknown tab",
            ),
            (
                "goto",
                "navigate: refused https://app.example/form?q=QUERYSECRET#FRAGSECRET\n",
                "navigation failed",
            ),
            ("click", "Page.navigate timed out\n", "timeout"),
            (
                "open",
                "websocket closed 1011 the browser went away\n",
                "connection lost",
            ),
            (
                "request",
                "curlwright: no open tab is on https://app.example — pass --tab.\nhttps://other.example/dash?q=QUERYSECRET\n",
                "not allowed",
            ),
            (
                "request",
                "curlwright: (22) The requested URL returned error: 500\n",
                "failed",
            ),
        ];
        for (kind, stderr, code) in cases {
            let note = ActionNote {
                kind: kind.to_string(),
                ..ActionNote::default()
            };
            let ended = note.end_params(false, stderr);
            assert_eq!(ended["error"], code, "{stderr}");
            assert_clean(&ended);
        }
    }

    fn assert_clean(value: &serde_json::Value) {
        let rendered = value.to_string();
        for secret in [
            "QUERYSECRET",
            "FRAGSECRET",
            "EVALSECRET",
            "user:pass",
            "tok=x",
            "p%40ss",
        ] {
            assert!(!rendered.contains(secret), "{rendered}");
        }
    }
}
