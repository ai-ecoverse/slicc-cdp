use crate::net::{ws_connect, Ws};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

enum Conn {
    Live(Ws),
    #[cfg(test)]
    Scripted {
        inbox: VecDeque<String>,
        sent: Vec<String>,
        loop_event: Option<String>,
    },
}

impl Conn {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        match self {
            Conn::Live(ws) => ws.send_text(text),
            #[cfg(test)]
            Conn::Scripted { sent, .. } => {
                sent.push(text.to_string());
                Ok(())
            }
        }
    }

    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
        match self {
            Conn::Live(ws) => ws.recv_text(deadline),
            #[cfg(test)]
            Conn::Scripted {
                inbox,
                loop_event,
                ..
            } => {
                if Instant::now() >= deadline {
                    return Ok(None);
                }
                if let Some(text) = inbox.pop_front() {
                    return Ok(Some(text));
                }
                if let Some(text) = loop_event.as_ref() {
                    return Ok(Some(text.clone()));
                }
                Ok(None)
            }
        }
    }
}

pub struct Raw {
    ws: Conn,
    next: i64,
    pending: VecDeque<Value>,
}

impl Raw {
    pub fn open(url: &str) -> Self {
        Self {
            ws: Conn::Live(ws_connect(url).unwrap()),
            next: 0,
            pending: VecDeque::new(),
        }
    }

    #[cfg(test)]
    fn scripted(inbox: Vec<Value>) -> Self {
        Self {
            ws: Conn::Scripted {
                inbox: inbox
                    .into_iter()
                    .map(|value| value.to_string())
                    .collect(),
                sent: Vec::new(),
                loop_event: None,
            },
            next: 0,
            pending: VecDeque::new(),
        }
    }

    #[cfg(test)]
    fn scripted_looping(event: Value) -> Self {
        Self {
            ws: Conn::Scripted {
                inbox: VecDeque::new(),
                sent: Vec::new(),
                loop_event: Some(event.to_string()),
            },
            next: 0,
            pending: VecDeque::new(),
        }
    }

    fn recv_socket(&mut self, deadline: Instant) -> Result<Option<Value>, String> {
        let Some(text) = self.ws.recv_text(deadline)? else {
            return Ok(None);
        };
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|err| err.to_string())
    }

    fn recv_value(&mut self, deadline: Instant) -> Result<Option<Value>, String> {
        if let Some(value) = self.pending.pop_front() {
            return Ok(Some(value));
        }
        self.recv_socket(deadline)
    }

    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, String> {
        self.call_with_timeout(method, params, session, Duration::from_secs(30))
    }

    fn call_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        session: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = json!(session);
        }
        self.ws.send_text(&message.to_string())?;
        let deadline = Instant::now() + timeout;
        let mut stashed = Vec::new();
        loop {
            if Instant::now() >= deadline {
                self.pending.extend(stashed);
                return Err(format!("{method} timed out"));
            }
            let Some(value) = self.recv_socket(deadline)? else {
                self.pending.extend(stashed);
                return Err(format!("{method} timed out"));
            };
            if value.get("id").and_then(|v| v.as_i64()) == Some(id) {
                self.pending.extend(stashed);
                if let Some(error) = value.get("error") {
                    return Err(error.to_string());
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            if value.get("method").is_some() {
                stashed.push(value);
            }
        }
    }

    pub fn wait_event(
        &mut self,
        timeout: Duration,
        mut matched: impl FnMut(&Value) -> bool,
    ) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                return Err("wait_event timed out".into());
            }
            let Some(value) = self.recv_value(deadline)? else {
                return Err("wait_event timed out".into());
            };
            if value.get("method").is_some() && matched(&value) {
                return Ok(value);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_ignores_prebuffered_events_and_preserves_them_for_wait_event() {
        let mut raw = Raw::scripted(vec![json!({ "id": 1, "result": { "ok": true } })]);
        raw.pending
            .push_back(json!({ "method": "Page.frameNavigated", "params": { "frame": {} } }));
        let started = Instant::now();
        let result = raw
            .call_with_timeout(
                "Page.enable",
                json!({}),
                None,
                Duration::from_millis(200),
            )
            .expect("call must not spin on buffered events");
        assert!(started.elapsed() < Duration::from_millis(200));
        assert_eq!(result, json!({ "ok": true }));
        let event = raw
            .wait_event(Duration::from_millis(200), |event| {
                event.get("method").and_then(Value::as_str) == Some("Page.frameNavigated")
            })
            .expect("buffered event must remain for wait_event");
        assert_eq!(
            event.get("method").and_then(Value::as_str),
            Some("Page.frameNavigated")
        );
    }

    #[test]
    fn call_stashes_socket_events_until_reply_then_wait_event_sees_them() {
        let mut raw = Raw::scripted(vec![
            json!({ "method": "Fetch.requestPaused", "params": { "requestId": "r1" } }),
            json!({ "id": 1, "result": { "sessionId": "S" } }),
        ]);
        let result = raw
            .call_with_timeout(
                "Target.attachToTarget",
                json!({ "targetId": "T", "flatten": true }),
                None,
                Duration::from_millis(200),
            )
            .unwrap();
        assert_eq!(result, json!({ "sessionId": "S" }));
        let paused = raw
            .wait_event(Duration::from_millis(200), |event| {
                event.get("method").and_then(Value::as_str) == Some("Fetch.requestPaused")
            })
            .unwrap();
        assert_eq!(
            paused.pointer("/params/requestId").and_then(Value::as_str),
            Some("r1")
        );
    }

    #[test]
    fn call_times_out_on_endless_events_instead_of_spinning() {
        let mut raw = Raw::scripted_looping(json!({ "method": "Runtime.consoleAPICalled" }));
        let started = Instant::now();
        let error = raw
            .call_with_timeout(
                "Page.enable",
                json!({}),
                None,
                Duration::from_millis(50),
            )
            .expect_err("endless events must time out");
        assert!(error.contains("timed out"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            raw.wait_event(Duration::from_millis(50), |event| {
                event.get("method").and_then(Value::as_str) == Some("Runtime.consoleAPICalled")
            })
            .is_ok()
        );
    }
}
