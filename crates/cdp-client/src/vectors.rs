use crate::cdp::{self as client, Cdp};
use crate::raw::Raw;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Run {
    cdp: Cdp,
    target: String,
    session: String,
    rule: Option<Value>,
    paused: Vec<Value>,
    steps: Vec<Value>,
}

fn outcome(result: Result<Value, String>) -> Value {
    match result {
        Ok(value) => json!({ "ok": true, "result": value }),
        Err(message) => json!({ "ok": false, "error": message }),
    }
}

impl Run {
    fn session_for(&self, step: &Value) -> Option<String> {
        match step.get("session").and_then(|v| v.as_str()) {
            Some("page") => Some(self.session.clone()),
            _ => None,
        }
    }

    fn step(&mut self, step: &Value) {
        let op = step.get("op").and_then(|v| v.as_str()).unwrap_or("");
        let record = match op {
            "open" => self.open(),
            "call" => {
                let method = step.get("method").and_then(|v| v.as_str()).unwrap_or("");
                let params = step.get("params").cloned().unwrap_or(json!({}));
                let session = self.session_for(step);
                if step.get("notify").and_then(|v| v.as_bool()) == Some(true) {
                    outcome(
                        self.cdp
                            .notify(method, params, session.as_deref())
                            .map(|_| Value::Null),
                    )
                } else {
                    outcome(self.cdp.call(method, params, session.as_deref()))
                }
            }
            "eval" => {
                let expression = step.get("expression").cloned().unwrap_or(Value::Null);
                let session = self.session.clone();
                outcome(self.cdp.call(
                    "Runtime.evaluate",
                    json!({ "expression": expression, "returnByValue": true, "awaitPromise": true, "userGesture": true }),
                    Some(&session),
                ))
            }
            "intercept" => {
                self.rule = step.get("rule").cloned();
                json!({ "ok": true })
            }
            "wait" => {
                let ms = step.get("ms").and_then(|v| v.as_u64()).unwrap_or(0);
                self.wait(Duration::from_millis(ms))
            }
            _ => json!({ "ok": false, "error": format!("unknown op {op}") }),
        };
        let mut entry = step.clone();
        entry["outcome"] = record;
        self.steps.push(entry);
    }

    fn open(&mut self) -> Value {
        let mut last = Value::Null;
        for _ in 0..3 {
            last = self.open_once();
            if last["ok"] == json!(true) {
                break;
            }
        }
        last
    }

    fn open_once(&mut self) -> Value {
        let created = self
            .cdp
            .call("Target.createTarget", json!({ "url": "about:blank" }), None);
        let target = match created {
            Ok(value) => value["targetId"].as_str().unwrap_or("").to_string(),
            Err(message) => return json!({ "ok": false, "error": message }),
        };
        self.target = target.clone();
        match client::attach(&mut self.cdp, &target) {
            Ok(session) => {
                self.session = session;
                json!({ "ok": true, "target": target })
            }
            Err(message) => json!({ "ok": false, "error": message }),
        }
    }

    fn wait(&mut self, span: Duration) -> Value {
        let deadline = Instant::now() + span;
        loop {
            self.drain();
            let now = Instant::now();
            if now >= deadline {
                return json!({ "ok": true });
            }
            let slice = (now + Duration::from_millis(100)).min(deadline);
            if let Err(message) = self.cdp.pump(slice) {
                return json!({ "ok": false, "error": message });
            }
        }
    }

    fn drain(&mut self) {
        let session = self.session.clone();
        while let Ok(Some(paused)) = self.cdp.next_paused(Some(&session), Duration::ZERO) {
            let mut seen = json!({ "url": paused.url, "resourceType": paused.resource_type });
            let rule = self.rule.clone().filter(|rule| {
                rule.get("match")
                    .and_then(|v| v.as_str())
                    .is_some_and(|needle| paused.url.contains(needle))
            });
            match rule {
                Some(rule) => {
                    let method = rule.get("method").and_then(|v| v.as_str()).unwrap_or("");
                    let mut params = rule.get("params").cloned().unwrap_or(json!({}));
                    params["requestId"] = json!(paused.request_id);
                    seen["answer"] = outcome(self.cdp.call(method, params, Some(&session)));
                }
                None => {
                    let _ = self.cdp.notify(
                        "Fetch.continueRequest",
                        json!({ "requestId": paused.request_id }),
                        Some(&session),
                    );
                }
            }
            self.paused.push(seen);
        }
    }
}

fn observe(ws: &str, target: &str) -> Value {
    if target.is_empty() {
        return Value::Null;
    }
    let mut raw = Raw::open(ws);
    let listed = raw
        .call("Target.getTargets", json!({}), None)
        .ok()
        .and_then(|v| v["targetInfos"].as_array().cloned())
        .unwrap_or_default();
    let popups: Vec<Value> = listed
        .iter()
        .filter(|info| {
            info["openerId"].as_str() == Some(target) && info["targetId"].as_str() != Some(target)
        })
        .map(|info| json!({ "targetId": info["targetId"], "url": info["url"] }))
        .collect();
    let page = raw
        .call(
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
            None,
        )
        .ok()
        .and_then(|v| v["sessionId"].as_str().map(str::to_string))
        .and_then(|session| {
            raw.call(
                "Runtime.evaluate",
                json!({ "expression": "JSON.stringify({ href: location.href, body: document.body ? document.body.innerText : '' })", "returnByValue": true }),
                Some(&session),
            )
            .ok()
        })
        .and_then(|v| v.pointer("/result/value").and_then(|v| v.as_str()).map(str::to_string))
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    for popup in &popups {
        raw.close(popup["targetId"].as_str().unwrap_or(""));
    }
    raw.close(target);
    json!({ "page": page, "popups": popups })
}

fn execute(ws: &str, plan: &Value) -> Value {
    let cdp = match Cdp::connect(ws) {
        Ok(cdp) => cdp,
        Err(message) => return json!({ "key": plan["key"], "error": message }),
    };
    let mut run = Run {
        cdp,
        target: String::new(),
        session: String::new(),
        rule: None,
        paused: Vec::new(),
        steps: Vec::new(),
    };
    for step in plan["steps"].as_array().cloned().unwrap_or_default() {
        run.step(&step);
    }
    let events = run.cdp.events().to_vec();
    let target = run.target.clone();
    drop(run.cdp);
    json!({
        "key": plan["key"],
        "target": target,
        "steps": run.steps,
        "paused": run.paused,
        "events": events,
        "observed": observe(ws, &target),
    })
}

#[test]
#[ignore = "needs SLICC_GUARD_PLAN from test/integration/guard-vectors.test.mjs"]
fn live_guard_vectors() {
    let plan_path = std::env::var("SLICC_GUARD_PLAN").expect("SLICC_GUARD_PLAN is not set");
    let out_path = std::env::var("SLICC_GUARD_OUT").expect("SLICC_GUARD_OUT is not set");
    let plan: Value = serde_json::from_str(&std::fs::read_to_string(plan_path).unwrap()).unwrap();
    let ws = plan["ws"].as_str().unwrap().to_string();
    let workers = plan["workers"].as_u64().unwrap_or(1).max(1) as usize;
    let queue: VecDeque<Value> = plan["runs"].as_array().cloned().unwrap_or_default().into();
    let queue = Arc::new(Mutex::new(queue));
    let results = Arc::new(Mutex::new(Vec::new()));
    let handles: Vec<_> = (0..workers)
        .map(|_| {
            let queue = Arc::clone(&queue);
            let results = Arc::clone(&results);
            let ws = ws.clone();
            std::thread::spawn(move || loop {
                let next = queue.lock().unwrap().pop_front();
                let Some(next) = next else { break };
                let done = execute(&ws, &next);
                results.lock().unwrap().push(done);
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let results = results.lock().unwrap().clone();
    std::fs::write(out_path, Value::Array(results).to_string()).unwrap();
}
