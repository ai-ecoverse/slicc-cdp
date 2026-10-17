use crate::net::{ws_connect, Ws};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub struct Raw {
    ws: Ws,
    next: i64,
}

impl Raw {
    pub fn open(url: &str) -> Self {
        Self {
            ws: ws_connect(url).unwrap(),
            next: 0,
        }
    }

    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.ws.send_text(&message.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let Some(text) = self.ws.recv_text(deadline)? else {
                return Err(format!("{method} timed out"));
            };
            let value: Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
            if value.get("id").and_then(|v| v.as_i64()) == Some(id) {
                if let Some(error) = value.get("error") {
                    return Err(error.to_string());
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    pub fn create(&mut self, url: &str) -> String {
        self.call(
            "Target.createTarget",
            json!({ "url": url, "background": true }),
            None,
        )
        .unwrap()["targetId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    pub fn session(&mut self, target: &str) -> String {
        self.call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
            None,
        )
        .unwrap()["sessionId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    pub fn eval(&mut self, session: &str, expression: &str) -> Value {
        self.call(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true, "userGesture": true }),
            Some(session),
        )
        .unwrap_or_else(|error| panic!("Runtime.evaluate {expression} failed: {error}"))
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null)
    }

    pub fn wait_href(&mut self, session: &str, prefix: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let href = self
                .call(
                    "Runtime.evaluate",
                    json!({ "expression": "document.readyState === 'complete' ? location.href : ''", "returnByValue": true }),
                    Some(session),
                )
                .ok()
                .and_then(|result| result.pointer("/result/value").cloned())
                .unwrap_or(Value::Null);
            if href.as_str().is_some_and(|href| href.starts_with(prefix)) {
                return;
            }
            assert!(Instant::now() < deadline, "never loaded {prefix}: {href}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub fn close(&mut self, target: &str) {
        let _ = self.call("Target.closeTarget", json!({ "targetId": target }), None);
    }
}
