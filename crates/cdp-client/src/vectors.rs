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
    auth: Option<Value>,
    seen_events: usize,
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
                self.auth = step.get("auth").cloned();
                json!({ "ok": true })
            }
            "launch" => {
                let slot = step
                    .get("slot")
                    .and_then(|v| v.as_str())
                    .unwrap_or("vector");
                let expression = step
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let done = format!("{slot}:done");
                self.evaluate(&format!(
                    "globalThis[{slot:?}] = ({expression}); Promise.resolve(globalThis[{slot:?}]).finally(() => {{ globalThis[{done:?}] = true; }}); true"
                ))
            }
            "until" => {
                let slot = step
                    .get("slot")
                    .and_then(|v| v.as_str())
                    .unwrap_or("vector");
                let ms = step.get("ms").and_then(|v| v.as_u64()).unwrap_or(0);
                self.until(slot, Duration::from_millis(ms))
            }
            "read" => {
                let slot = step
                    .get("slot")
                    .and_then(|v| v.as_str())
                    .unwrap_or("vector");
                self.evaluate(&format!("globalThis[{slot:?}]"))
            }
            "guess" => self.guess(step),
            "wait" => {
                let ms = step.get("ms").and_then(|v| v.as_u64()).unwrap_or(0);
                self.wait(Duration::from_millis(ms))
            }
            _ => json!({ "ok": false, "error": format!("unknown op {op}") }),
        };
        let mut entry = step.clone();
        entry["outcome"] = record;
        entry["at"] = json!(epoch_ms());
        self.steps.push(entry);
    }

    fn evaluate(&mut self, expression: &str) -> Value {
        let session = self.session.clone();
        outcome(self.cdp.call_for(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            Some(&session),
            Duration::from_secs(30),
        ))
    }

    fn id_for(&self, needle: &str) -> Option<String> {
        self.paused
            .iter()
            .find(|seen| {
                seen["url"].as_str().is_some_and(|url| url.contains(needle))
                    && seen["stage"] == json!("Request")
            })
            .and_then(|seen| seen["requestId"].as_str().map(str::to_string))
    }

    fn guess(&mut self, step: &Value) -> Value {
        self.drain();
        let split = |id: &str| -> Option<(String, u64)> {
            let stem = id.strip_suffix(".0")?;
            let digits = stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
            let (prefix, number) = stem.split_at(stem.len() - digits);
            Some((prefix.to_string(), number.parse().ok()?))
        };
        let before = step.get("before").and_then(|v| v.as_str()).unwrap_or("");
        let after = step.get("after").and_then(|v| v.as_str()).unwrap_or("");
        let bounds = self
            .id_for(before)
            .and_then(|id| split(&id))
            .zip(self.id_for(after).and_then(|id| split(&id)));
        let Some(((prefix, low), (_, high))) = bounds else {
            return json!({ "ok": false, "error": "no bracketing request ids" });
        };
        let session = self.session.clone();
        let calls = step
            .get("calls")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut attempts = Vec::new();
        for call in &calls {
            for number in low + 1..high {
                let id = format!("{prefix}{number}.0");
                let method = call.get("method").and_then(|v| v.as_str()).unwrap_or("");
                let mut params = call.get("params").cloned().unwrap_or(json!({}));
                params["requestId"] = json!(id);
                let result = outcome(self.cdp.call_for(
                    method,
                    params,
                    Some(&session),
                    Duration::from_secs(5),
                ));
                attempts.push(json!({ "method": method, "requestId": id, "outcome": result }));
            }
        }
        json!({ "ok": true, "attempts": attempts })
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

    fn until(&mut self, slot: &str, span: Duration) -> Value {
        let deadline = Instant::now() + span;
        let check = format!("globalThis[{:?}] === true", format!("{slot}:done"));
        let session = self.session.clone();
        loop {
            self.drain();
            let settled = self.cdp.call_for(
                "Runtime.evaluate",
                json!({ "expression": check, "returnByValue": true }),
                Some(&session),
                Duration::from_secs(5),
            );
            match settled {
                Ok(value) if value["result"]["value"] == json!(true) => {
                    return json!({ "ok": true })
                }
                Ok(_) => {}
                Err(message) => return json!({ "ok": false, "error": message }),
            }
            let now = Instant::now();
            if now >= deadline {
                return json!({ "ok": false, "error": format!("{slot} did not settle within {}ms", span.as_millis()) });
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
            let stage = if paused.is_response() {
                "Response"
            } else {
                "Request"
            };
            let mut seen = json!({
                "url": paused.url,
                "resourceType": paused.resource_type,
                "requestId": paused.request_id,
                "stage": stage,
            });
            let rule = self.rule.clone().filter(|rule| {
                rule.get("match")
                    .and_then(|v| v.as_str())
                    .is_some_and(|needle| paused.url.contains(needle))
                    && rule
                        .get("stage")
                        .and_then(|v| v.as_str())
                        .is_none_or(|want| want == stage)
            });
            match rule {
                Some(rule) if rule.get("hold").and_then(|v| v.as_bool()) == Some(true) => {
                    seen["held"] = json!(true);
                }
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
        let challenges: Vec<Value> = self.cdp.events()[self.seen_events..]
            .iter()
            .filter(|event| {
                event["method"] == json!("Fetch.authRequired")
                    && event["sessionId"].as_str() == Some(session.as_str())
            })
            .cloned()
            .collect();
        self.seen_events = self.cdp.events().len();
        for event in challenges {
            let url = event
                .pointer("/params/request/url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let id = event
                .pointer("/params/requestId")
                .cloned()
                .unwrap_or(Value::Null);
            let response = self
                .auth
                .as_ref()
                .filter(|auth| {
                    auth.get("match")
                        .and_then(|v| v.as_str())
                        .is_some_and(|needle| url.contains(needle))
                })
                .and_then(|auth| auth.get("response").cloned())
                .unwrap_or(json!({ "response": "Default" }));
            let _ = self.cdp.notify(
                "Fetch.continueWithAuth",
                json!({ "requestId": id, "authChallengeResponse": response }),
                Some(&session),
            );
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

fn execute(ws: &str, plan: &Value, note: &dyn Fn(&str)) -> Value {
    note("connect");
    match Cdp::connect(ws) {
        Ok(cdp) => execute_on(cdp, plan, note, &mut |target: &str| observe(ws, target)),
        Err(message) => json!({ "key": plan["key"], "error": message }),
    }
}

fn execute_on(
    cdp: Cdp,
    plan: &Value,
    note: &dyn Fn(&str),
    observe: &mut dyn FnMut(&str) -> Value,
) -> Value {
    let mut run = Run {
        cdp,
        target: String::new(),
        session: String::new(),
        rule: None,
        auth: None,
        seen_events: 0,
        paused: Vec::new(),
        steps: Vec::new(),
    };
    let all = plan["steps"].as_array().cloned().unwrap_or_default();
    let exit = all
        .iter()
        .position(|step| step["op"] == json!("exit"))
        .unwrap_or(all.len());
    for (index, step) in all[..exit].iter().enumerate() {
        note(&format!("step {index} {}", label(step)));
        run.step(step);
    }
    let events = run.cdp.events().to_vec();
    let Run {
        cdp,
        target,
        mut steps,
        paused,
        ..
    } = run;
    let mut cdp = Some(cdp);
    let mut detached = None;
    if exit < all.len() {
        cdp = None;
        detached = Some(epoch_ms());
        for (index, step) in all.iter().enumerate().skip(exit) {
            note(&format!("step {index} {}", label(step)));
            steps.push(after_exit(step));
        }
    }
    note("observe");
    let observed = observe(&target);
    drop(cdp);
    json!({
        "key": plan["key"],
        "detached": detached.unwrap_or_else(epoch_ms),
        "target": target,
        "steps": steps,
        "paused": paused,
        "events": events,
        "observed": observed,
    })
}

fn after_exit(step: &Value) -> Value {
    let outcome = match step.get("op").and_then(|v| v.as_str()) {
        Some("exit") => json!({ "ok": true }),
        Some("wait") => {
            let ms = step.get("ms").and_then(|v| v.as_u64()).unwrap_or(0);
            std::thread::sleep(Duration::from_millis(ms));
            json!({ "ok": true })
        }
        _ => json!({ "ok": false, "error": "only wait can follow exit" }),
    };
    let mut entry = step.clone();
    entry["outcome"] = outcome;
    entry["at"] = json!(epoch_ms());
    entry
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

fn label(step: &Value) -> String {
    let op = step.get("op").and_then(|v| v.as_str()).unwrap_or("?");
    match step.get("method").and_then(|v| v.as_str()) {
        Some(method) => format!("{op} {method}"),
        None => op.to_string(),
    }
}

struct Slot {
    key: String,
    step: String,
    since: Instant,
}

fn overdue(slots: &[Option<Slot>], limit: Duration, now: Instant) -> Option<String> {
    slots.iter().flatten().find_map(|slot| {
        let spent = now.saturating_duration_since(slot.since);
        (spent > limit).then(|| {
            format!(
                "{} hung for {}ms at {}",
                slot.key,
                spent.as_millis(),
                slot.step
            )
        })
    })
}

type Runner = dyn Fn(&Value, &dyn Fn(&str)) -> Value + Send + Sync;
type Say = dyn Fn(String) + Send + Sync;

fn drive(
    runs: Vec<Value>,
    workers: usize,
    limit: Duration,
    run: Arc<Runner>,
    say: Arc<Say>,
) -> Result<Vec<Value>, String> {
    let workers = workers.max(1);
    let total = runs.len();
    let queue = Arc::new(Mutex::new(VecDeque::from(runs)));
    let slots: Arc<Mutex<Vec<Option<Slot>>>> =
        Arc::new(Mutex::new((0..workers).map(|_| None).collect()));
    let results = Arc::new(Mutex::new(Vec::new()));
    let handles: Vec<_> = (0..workers)
        .map(|index| {
            let queue = Arc::clone(&queue);
            let slots = Arc::clone(&slots);
            let results = Arc::clone(&results);
            let run = Arc::clone(&run);
            let say = Arc::clone(&say);
            std::thread::spawn(move || loop {
                let next = queue.lock().unwrap().pop_front();
                let Some(next) = next else { break };
                let key = next["key"].as_str().unwrap_or("").to_string();
                say(format!("start {key}"));
                slots.lock().unwrap()[index] = Some(Slot {
                    key: key.clone(),
                    step: "start".into(),
                    since: Instant::now(),
                });
                let note = |step: &str| {
                    for slot in slots.lock().unwrap()[index].iter_mut() {
                        slot.step = step.to_string();
                    }
                };
                let done = run(&next, &note);
                let spent = slots.lock().unwrap()[index]
                    .take()
                    .map(|slot| slot.since.elapsed())
                    .unwrap_or_default();
                let count = {
                    let mut results = results.lock().unwrap();
                    results.push(done);
                    results.len()
                };
                say(format!(
                    "done {key} in {}ms ({count}/{total})",
                    spent.as_millis()
                ));
            })
        })
        .collect();
    while !handles.iter().all(|handle| handle.is_finished()) {
        if let Some(message) = overdue(&slots.lock().unwrap(), limit, Instant::now()) {
            say(format!("hung: {message}"));
            return Err(message);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for handle in handles {
        handle
            .join()
            .map_err(|_| "a vector worker panicked".to_string())?;
    }
    let results = results.lock().unwrap().clone();
    Ok(results)
}

#[test]
#[ignore = "needs SLICC_GUARD_PLAN from test/integration/guard-vectors.test.mjs"]
fn live_guard_vectors() {
    let plan_path = std::env::var("SLICC_GUARD_PLAN").expect("SLICC_GUARD_PLAN is not set");
    let out_path = std::env::var("SLICC_GUARD_OUT").expect("SLICC_GUARD_OUT is not set");
    let plan: Value = serde_json::from_str(&std::fs::read_to_string(plan_path).unwrap()).unwrap();
    let ws = plan["ws"].as_str().unwrap().to_string();
    let workers = plan["workers"].as_u64().unwrap_or(1) as usize;
    let limit = Duration::from_secs(plan["rowSeconds"].as_u64().unwrap_or(180));
    let runs = plan["runs"].as_array().cloned().unwrap_or_default();
    let started = Instant::now();
    let results = drive(
        runs,
        workers,
        limit,
        Arc::new(move |next: &Value, note: &dyn Fn(&str)| execute(&ws, next, note)),
        Arc::new(move |text: String| {
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr().lock(),
                "[vectors +{:.1}s] {text}",
                started.elapsed().as_secs_f64()
            );
        }),
    )
    .expect("guard vector row");
    std::fs::write(out_path, Value::Array(results).to_string()).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> Arc<Say> {
        Arc::new(|_: String| {})
    }

    fn slot(key: &str, step: &str, since: Instant) -> Option<Slot> {
        Some(Slot {
            key: key.into(),
            step: step.into(),
            since,
        })
    }

    #[test]
    fn a_row_within_its_limit_is_not_overdue() {
        let now = Instant::now();
        let slots = [None, slot("a", "step 0 open", now)];
        assert_eq!(overdue(&slots, Duration::from_secs(1), now), None);
    }

    #[test]
    fn an_overdue_row_names_its_key_and_step() {
        let since = Instant::now();
        let now = since + Duration::from_secs(5);
        let slots = [
            slot("fresh", "step 1", now),
            slot("C2.twohop", "step 6 wait", since),
        ];
        assert_eq!(
            overdue(&slots, Duration::from_secs(2), now).as_deref(),
            Some("C2.twohop hung for 5000ms at step 6 wait")
        );
    }

    #[test]
    fn steps_are_labelled_by_op_and_method() {
        assert_eq!(
            label(&json!({ "op": "call", "method": "Page.enable" })),
            "call Page.enable"
        );
        assert_eq!(label(&json!({ "op": "wait" })), "wait");
        assert_eq!(label(&json!({})), "?");
    }

    #[test]
    fn every_row_runs_once_and_reports_progress() {
        let said = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&said);
        let runs: Vec<Value> = (0..7)
            .map(|n| json!({ "key": format!("row{n}") }))
            .collect();
        let results = drive(
            runs,
            0,
            Duration::from_secs(30),
            Arc::new(|next: &Value, note: &dyn Fn(&str)| {
                note("step 0 open");
                json!({ "key": next["key"] })
            }),
            Arc::new(move |text: String| heard.lock().unwrap().push(text)),
        )
        .unwrap();
        let mut keys: Vec<String> = results
            .iter()
            .map(|item| item["key"].as_str().unwrap().to_string())
            .collect();
        keys.sort();
        assert_eq!(keys, (0..7).map(|n| format!("row{n}")).collect::<Vec<_>>());
        let lines = said.lock().unwrap().clone();
        assert_eq!(lines.len(), 14, "{lines:?}");
        assert!(lines.contains(&"start row0".to_string()), "{lines:?}");
        assert!(
            lines.iter().any(|line| line.starts_with("done row6 in ")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.ends_with("(7/7)")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_hung_row_stops_the_run_and_names_the_step() {
        let said = Arc::new(Mutex::new(Vec::new()));
        let heard = Arc::clone(&said);
        let runs = vec![json!({ "key": "quick" }), json!({ "key": "stalled" })];
        let error = drive(
            runs,
            2,
            Duration::from_millis(200),
            Arc::new(|next: &Value, note: &dyn Fn(&str)| {
                if next["key"] == json!("stalled") {
                    note("step 3 eval");
                    std::thread::sleep(Duration::from_secs(2));
                }
                json!({ "key": next["key"] })
            }),
            Arc::new(move |text: String| heard.lock().unwrap().push(text)),
        )
        .unwrap_err();
        assert!(error.starts_with("stalled hung for "), "{error}");
        assert!(error.ends_with("ms at step 3 eval"), "{error}");
        assert!(said.lock().unwrap().contains(&format!("hung: {error}")));
    }

    struct Scripted {
        inbox: VecDeque<String>,
        sent: Arc<Mutex<Vec<Value>>>,
        open: Arc<Mutex<bool>>,
    }

    impl crate::cdp::Transport for Scripted {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            let message: Value = serde_json::from_str(text).unwrap();
            let result = match message["method"].as_str().unwrap_or("") {
                "Target.createTarget" => json!({ "targetId": "T1" }),
                "Target.attachToTarget" => json!({ "sessionId": format!("S{}", message["id"]) }),
                _ => json!({}),
            };
            self.inbox
                .push_back(json!({ "id": message["id"], "result": result }).to_string());
            self.sent.lock().unwrap().push(message);
            Ok(())
        }

        fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
            Ok(self.inbox.pop_front())
        }
    }

    impl Drop for Scripted {
        fn drop(&mut self) {
            *self.open.lock().unwrap() = false;
        }
    }

    #[test]
    fn the_guarded_client_stays_attached_until_the_target_is_observed() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let open = Arc::new(Mutex::new(true));
        let cdp = Cdp::from_transport(Box::new(Scripted {
            inbox: VecDeque::new(),
            sent: Arc::clone(&sent),
            open: Arc::clone(&open),
        }));
        let plan = json!({
            "key": "C2.twohop.seven.lagging.raw",
            "steps": [{ "op": "open" }, { "op": "wait", "ms": 0 }],
        });
        let notes = Mutex::new(Vec::new());
        let mut seen = Vec::new();
        let out = execute_on(
            cdp,
            &plan,
            &|step: &str| notes.lock().unwrap().push(step.to_string()),
            &mut |target: &str| {
                seen.push((target.to_string(), *open.lock().unwrap()));
                json!({ "closed": target })
            },
        );
        assert_eq!(seen, vec![("T1".to_string(), true)]);
        assert!(!*open.lock().unwrap());
        assert_eq!(out["observed"], json!({ "closed": "T1" }));
        assert_eq!(out["target"], json!("T1"));
        assert_eq!(out["steps"][0]["outcome"]["ok"], json!(true));
        assert!(out["steps"][1]["at"].as_u64().unwrap() <= out["detached"].as_u64().unwrap());
        assert_eq!(
            *notes.lock().unwrap(),
            vec!["step 0 open", "step 1 wait", "observe"]
        );
        let methods: Vec<String> = sent
            .lock()
            .unwrap()
            .iter()
            .map(|item| item["method"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(methods.contains(&"Fetch.enable".to_string()), "{methods:?}");
    }

    #[test]
    fn a_refused_connection_is_reported_for_the_row() {
        let notes = Mutex::new(Vec::new());
        let out = execute(
            "ws://127.0.0.1:1/devtools/browser/x",
            &json!({ "key": "row", "steps": [{ "op": "open" }] }),
            &|step: &str| notes.lock().unwrap().push(step.to_string()),
        );
        assert_eq!(*notes.lock().unwrap(), vec!["connect"]);
        assert_eq!(out["key"], json!("row"));
        assert!(
            out["error"].as_str().is_some_and(|text| !text.is_empty()),
            "{out}"
        );
    }

    #[test]
    fn an_exit_step_drops_the_client_before_the_target_is_observed() {
        let open = Arc::new(Mutex::new(true));
        let cdp = Cdp::from_transport(Box::new(Scripted {
            inbox: VecDeque::new(),
            sent: Arc::new(Mutex::new(Vec::new())),
            open: Arc::clone(&open),
        }));
        let plan = json!({
            "key": "X1.after-exit.seven.prompt.raw",
            "steps": [
                { "op": "open" },
                { "op": "exit" },
                { "op": "wait", "ms": 20 },
                { "op": "eval", "expression": "1" },
            ],
        });
        let notes = Mutex::new(Vec::new());
        let mut seen = Vec::new();
        let out = execute_on(
            cdp,
            &plan,
            &|step: &str| notes.lock().unwrap().push(step.to_string()),
            &mut |target: &str| {
                seen.push((target.to_string(), *open.lock().unwrap()));
                json!({})
            },
        );
        assert_eq!(seen, vec![("T1".to_string(), false)]);
        let steps = out["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[1]["outcome"], json!({ "ok": true }));
        assert_eq!(steps[2]["outcome"], json!({ "ok": true }));
        assert!(steps[2]["at"].as_u64().unwrap() >= out["detached"].as_u64().unwrap() + 20);
        assert_eq!(
            steps[3]["outcome"],
            json!({ "ok": false, "error": "only wait can follow exit" })
        );
        assert_eq!(
            *notes.lock().unwrap(),
            vec![
                "step 0 open",
                "step 1 exit",
                "step 2 wait",
                "step 3 eval",
                "observe"
            ]
        );
    }

    #[test]
    fn a_panicking_worker_fails_the_run() {
        let error = drive(
            vec![json!({ "key": "boom" })],
            1,
            Duration::from_secs(30),
            Arc::new(|_: &Value, _: &dyn Fn(&str)| panic!("worker failed")),
            quiet(),
        )
        .unwrap_err();
        assert_eq!(error, "a vector worker panicked");
    }
}
