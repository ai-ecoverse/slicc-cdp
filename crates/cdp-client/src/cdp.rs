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

    pub fn wait_load(&mut self, session_id: &str) -> Result<(), String> {
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

fn is_load(event: &Value, session_id: &str) -> bool {
    if event.get("sessionId").and_then(|v| v.as_str()) != Some(session_id) {
        return false;
    }
    match event.get("method").and_then(|v| v.as_str()) {
        Some("Page.loadEventFired") => true,
        Some("Page.lifecycleEvent") => {
            event
                .pointer("/params/name")
                .and_then(|v| v.as_str())
                == Some("load")
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
