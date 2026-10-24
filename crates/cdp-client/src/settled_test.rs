fn settled_answer(message: &Value) -> Vec<Value> {
    vec![json!({"id":message["id"],"result":{}})]
}

fn settled_client(mark: &str) -> Cdp {
    let mut cdp = pending(settled_answer);
    cdp.protect_origins(&["https://private.example.invalid"]);
    match mark {
        "blocked" => { cdp.blocked.insert("S".into()); }
        "dialog" => { cdp.dialog_sessions.insert("S".into()); }
        "guard" => { cdp.guard_sessions.insert("S".into()); }
        "protected" => { cdp.protected_targets.insert("fixture:S".into()); }
        "abandoned" => { cdp.abandoned.insert("ABANDONED".into()); }
        _ => {}
    }
    cdp
}

fn settled_commands() -> Vec<(&'static str, Value, Option<&'static str>)> {
    let network = json!({"offline":true,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1});
    vec![
        ("Network.loadNetworkResource", json!({"url":"https://example.test/"}), Some("S")),
        ("Page.captureSnapshot", json!({}), None),
        ("Page.addScriptToEvaluateOnNewDocument", json!({"source":"1"}), Some("S")),
        ("Page.addScriptToEvaluateOnNewDocument", json!({"source":"1"}), None),
        ("Emulation.setDeviceMetricsOverride", json!({"width":1,"height":1,"deviceScaleFactor":1,"mobile":false}), Some("S")),
        ("Network.emulateNetworkConditions", network.clone(), Some("S")),
        ("Network.emulateNetworkConditions", network.clone(), None),
        ("Network.emulateNetworkConditions", json!({"offline":true}), Some("S")),
        ("Network.setRequestInterception", json!({"patterns":[]}), Some("S")),
        ("Runtime.addBinding", json!({"name":"x"}), Some("S")),
        ("Debugger.setBreakpointsActive", json!({"active":true}), Some("S")),
        ("Browser.setWindowBounds", json!({"windowId":1,"bounds":{}}), None),
        ("Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true}), None),
        ("Target.autoAttachRelated", json!({"targetId":"T","waitForDebuggerOnStart":false}), None),
        ("Target.attachToTarget", json!({"targetId":"T"}), None),
        ("Target.attachToTarget", json!({"targetId":"ABANDONED"}), None),
        ("Target.setAutoAttach", json!({"autoAttach":false,"waitForDebuggerOnStart":false}), None),
        ("Fetch.continueRequest", json!({"requestId":"R"}), Some("S")),
        ("Page.handleJavaScriptDialog", json!({"accept":true}), Some("S")),
        ("Browser.getVersion", json!({}), None),
        ("DOM.getDocument", json!({"depth":-1}), Some("S")),
    ]
}

#[test]
fn settled_refusals_are_the_admission_refusals_they_shortcut() {
    let mut decided = 0;
    for mark in ["plain", "blocked", "dialog", "guard", "protected", "abandoned"] {
        for (method, params, session) in settled_commands() {
            let mut cdp = settled_client(mark);
            let Some(reason) = cdp.settled_refusal(method, &params, session) else { continue };
            decided += 1;
            assert_eq!(cdp.call(method, params.clone(), session).unwrap_err(), reason, "{mark} {method} {params}");
            let cdp = settled_client(mark);
            assert_eq!(cdp.settled_refusal(method, &params, session.map(|_| "")), cdp.settled_refusal(method, &params, None));
        }
    }
    assert!(decided > 20, "{decided}");
}

#[test]
fn settled_refusals_leave_undecidable_commands_to_admission() {
    let cdp = settled_client("plain");
    let network = json!({"offline":true,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1});
    for (method, params, session) in [
        ("Network.emulateNetworkConditions", network, Some("S")),
        ("Runtime.addBinding", json!({"name":"x"}), Some("S")),
        ("Debugger.setBreakpointsActive", json!({"active":true}), Some("S")),
        ("Browser.setWindowBounds", json!({"windowId":1,"bounds":{}}), None),
        ("Target.setAutoAttach", json!({"autoAttach":false,"waitForDebuggerOnStart":false,"flatten":true}), None),
        ("Browser.getVersion", json!({}), None),
        ("DOM.getDocument", json!({"depth":-1}), Some("S")),
    ] {
        assert_eq!(cdp.settled_refusal(method, &params, session), None, "{method}");
    }
    let blocked = settled_client("blocked");
    assert_eq!(blocked.settled_refusal("Fetch.continueRequest", &json!({"requestId":"R"}), Some("S")), None);
    assert_eq!(blocked.settled_refusal("Page.handleJavaScriptDialog", &json!({"accept":true}), Some("S")), None);
    assert_eq!(blocked.settled_refusal("DOM.getDocument", &json!({}), Some("S")), Some(blocked.refusal()));
    let mut owned = settled_client("plain");
    owned.contexts.insert("OWN".into());
    assert_eq!(owned.settled_refusal("Page.addScriptToEvaluateOnNewDocument", &json!({"source":"1","browserContextId":"OWN"}), Some("S")).as_deref(), Some(super::PERSISTENT));
    assert_eq!(owned.settled_refusal("Network.setRequestInterception", &json!({"patterns":[]}), Some("S")).as_deref(), Some("slicc: refused: Network.setRequestInterception would hand out seven's requests; use Fetch on a page session\n"));
}

#[test]
fn a_waiter_runs_while_a_reply_is_still_held() {
    struct Held {
        sent: Rc<RefCell<Vec<i64>>>,
        release: Instant,
    }
    impl Transport for Held {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            let message: Value = serde_json::from_str(text).unwrap();
            self.sent.borrow_mut().push(message["id"].as_i64().unwrap());
            Ok(())
        }
        fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
            let id = self.sent.borrow().last().copied();
            if Instant::now() >= self.release && id.is_some() {
                self.sent.borrow_mut().clear();
                return Ok(Some(json!({"id":id,"result":{"product":"held"}}).to_string()));
            }
            std::thread::sleep(deadline.min(self.release).saturating_duration_since(Instant::now()));
            Ok(None)
        }
    }
    let sent = Rc::new(RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(Held { sent: sent.clone(), release: Instant::now() + Duration::from_millis(60) }));
    let calls = Rc::new(RefCell::new(0));
    let counted = calls.clone();
    cdp.set_waiter(Some(Box::new(move |cdp: &Cdp| {
        assert!(cdp.settled_refusal("Page.captureSnapshot", &json!({}), None).is_some());
        *counted.borrow_mut() += 1;
    })));
    assert_eq!(cdp.call("Browser.getVersion", json!({}), None).unwrap()["product"], "held");
    assert!(*calls.borrow() >= 3, "{}", calls.borrow());
    let before = *calls.borrow();
    assert!(!cdp.pump(Instant::now() + Duration::from_millis(20)).unwrap());
    assert!(*calls.borrow() > before);
    cdp.set_waiter(None);
    let quiet = *calls.borrow();
    assert!(!cdp.pump(Instant::now() + Duration::from_millis(10)).unwrap());
    assert_eq!(*calls.borrow(), quiet);
    let mut timed = Cdp::from_transport(Box::new(Held { sent: Rc::new(RefCell::new(Vec::new())), release: Instant::now() + Duration::from_secs(60) }));
    timed.set_waiter(Some(Box::new(|_: &Cdp| {})));
    let started = Instant::now();
    assert_eq!(timed.call_for("Browser.getVersion", json!({}), None, Duration::from_millis(30)).unwrap_err(), "Browser.getVersion timed out\n");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_slow_waiter_still_leaves_a_window_to_read_the_reply() {
    struct Ready {
        sent: Rc<RefCell<Vec<i64>>>,
    }
    impl Transport for Ready {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            let message: Value = serde_json::from_str(text).unwrap();
            self.sent.borrow_mut().push(message["id"].as_i64().unwrap());
            Ok(())
        }
        fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
            if deadline <= Instant::now() { return Ok(None) }
            Ok(self.sent.borrow_mut().pop().map(|id| json!({"id":id,"result":{"product":"read"}}).to_string()))
        }
    }
    let mut cdp = Cdp::from_transport(Box::new(Ready { sent: Rc::new(RefCell::new(Vec::new())) }));
    cdp.set_waiter(Some(Box::new(|_: &Cdp| std::thread::sleep(Duration::from_millis(10)))));
    assert_eq!(cdp.call_for("Browser.getVersion", json!({}), None, Duration::from_millis(500)).unwrap()["product"], "read");
}
