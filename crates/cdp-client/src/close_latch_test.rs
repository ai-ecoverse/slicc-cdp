const GONE: &str = "websocket closed 1011 the CDP host is gone\n";

type CloseOn = Box<dyn FnMut(&Value) -> bool>;

struct ClosingBackend {
    inbox: VecDeque<String>,
    closed: bool,
    close_on: CloseOn,
    fail: CloseOn,
    sent: std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
}

impl Transport for ClosingBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        if self.closed { return Err(GONE.into()) }
        let message: Value = serde_json::from_str(text).unwrap();
        self.sent.borrow_mut().push(message.clone());
        self.closed = (self.close_on)(&message);
        if (self.fail)(&message) {
            self.inbox.push_back(json!({"id":message["id"],"error":{"message":"No frame"}}).to_string());
        } else if !self.closed {
            self.inbox.push_back(json!({"id":message["id"],"result":closing_answer(&message)}).to_string());
        }
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        match self.inbox.pop_front() {
            Some(text) => Ok(Some(text)),
            None if self.closed => Err(GONE.into()),
            None => Ok(None),
        }
    }
}

fn closing_answer(message: &Value) -> Value {
    let target = message["params"]["targetId"].as_str().unwrap_or("T");
    let url = if target == "E" { "https://seven.sliccy.ai/" } else { "https://example.test/" };
    match message["method"].as_str().unwrap() {
        "Target.getTargetInfo" => json!({"targetInfo":{"targetId":target,"type":"page","url":url}}),
        "Target.getTargets" => json!({"targetInfos":[{"targetId":"E","type":"page","url":"https://seven.sliccy.ai/"}]}),
        "Target.attachToTarget" => json!({"sessionId":format!("S{}", message["id"])}),
        "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"T","loaderId":"L","url":"https://example.test/","securityOrigin":"https://example.test"}}}),
        _ => json!({}),
    }
}

fn failing_client(close_on: impl FnMut(&Value) -> bool + 'static, fail: impl FnMut(&Value) -> bool + 'static) -> (Cdp, std::rc::Rc<std::cell::RefCell<Vec<Value>>>) {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let cdp = Cdp::from_transport(Box::new(ClosingBackend {
        inbox: VecDeque::new(), closed: false, close_on: Box::new(close_on), fail: Box::new(fail), sent: std::rc::Rc::clone(&sent),
    }));
    (cdp, sent)
}

fn closing_client(close_on: impl FnMut(&Value) -> bool + 'static) -> (Cdp, std::rc::Rc<std::cell::RefCell<Vec<Value>>>) {
    failing_client(close_on, |_| false)
}

fn counted(method: &'static str, hit: impl Fn(usize) -> bool) -> impl FnMut(&Value) -> bool {
    let mut seen = 0;
    move |message| {
        if message["method"] != method { return false }
        seen += 1;
        hit(seen)
    }
}

fn nth(method: &'static str, at: usize) -> impl FnMut(&Value) -> bool {
    counted(method, move |seen| seen == at)
}

fn guarded_error_page(cdp: &mut Cdp) {
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.guards.insert("T".into(), "G".into());
    cdp.guard_sessions.insert("G".into());
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":"https://example.test/"}}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"G","params":{"frame":{"id":"F","url":"https://example.test/","securityOrigin":"https://example.test"}}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"G","params":{"frame":{"id":"F","url":"chrome-error://chromewebdata/","securityOrigin":"://","unreachableUrl":"https://example.test/missing"}}}));
    assert_eq!(cdp.document_evidence("T"), "url=chrome-error://chromewebdata/ unreachable=https://example.test/missing navigating=false previous=false probed=false");
}

#[test]
fn only_a_close_turns_a_later_guard_refusal_into_the_close() {
    let (mut cdp, _) = closing_client(|_| false);
    let refusal = format!("{}\n", cdp.guard().refusal());
    assert_eq!(cdp.refusal(), refusal);
    assert_eq!(cdp.connected(), Ok(()));
    for other in ["websocket read failed: reset\n", "CDP response was not JSON: x\n", "Page.getFrameTree timed out\n"] {
        assert_eq!(cdp.closing(other.into()), other);
        assert_eq!(cdp.refusal(), refusal);
    }
    assert_eq!(cdp.closing(GONE.into()), GONE);
    assert_eq!(cdp.closing("websocket connection closed\n".into()), GONE);
    assert_eq!(cdp.refusal(), GONE);
    assert_eq!(cdp.connected(), Err(GONE.to_string()));
}

#[test]
fn a_close_swallowed_by_a_page_proof_is_reported_instead_of_a_refusal() {
    let (mut cdp, sent) = closing_client(nth("Target.getTargets", 1));
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.guards.insert("T".into(), "G".into());
    cdp.guard_sessions.insert("G".into());
    cdp.targets.insert("T".into(), json!({"targetId":"T","type":"page","url":"https://example.test/"}));
    assert_eq!(cdp.call("Runtime.evaluate", json!({"expression":"1"}), Some("S")).unwrap_err(), GONE);
    assert!(sent.borrow().iter().any(|call| call["method"] == "Target.getTargets"));
    let count = sent.borrow().len();
    assert_eq!(cdp.call("Runtime.evaluate", json!({"expression":"1"}), Some("S")).unwrap_err(), GONE);
    assert_eq!(sent.borrow().len(), count);
}

#[test]
fn a_close_during_an_error_page_refresh_is_passed_through() {
    let (mut cdp, sent) = closing_client(nth("Page.getFrameTree", 1));
    guarded_error_page(&mut cdp);
    assert_eq!(cdp.target_protected("T", false).unwrap_err(), GONE);
    assert_eq!(sent.borrow().iter().filter(|call| call["method"] == "Page.getFrameTree").count(), 1);
    let (mut cdp, _) = closing_client(nth("Page.getFrameTree", 1));
    guarded_error_page(&mut cdp);
    assert_eq!(cdp.refresh("T"), Err(GONE.to_string()));
    let (mut cdp, _) = closing_client(nth("Page.getFrameTree", 1));
    guarded_error_page(&mut cdp);
    assert_eq!(cdp.call("Runtime.evaluate", json!({"expression":"1"}), Some("S")).unwrap_err(), GONE);
    let (mut cdp, _) = closing_client(|_| false);
    guarded_error_page(&mut cdp);
    assert_eq!(cdp.refresh("T"), Ok(true));
}

#[test]
fn a_close_during_an_error_probe_is_passed_through() {
    for (method, at) in [("Target.attachToTarget", 1), ("Page.getFrameTree", 1), ("Target.detachFromTarget", 1)] {
        let (mut cdp, _) = closing_client(nth(method, at));
        assert_eq!(cdp.call("Target.getTargets", json!({}), None).unwrap_err(), GONE, "{method}");
        let (mut cdp, _) = closing_client(nth(method, at));
        cdp.targets.insert("T".into(), json!({"targetId":"T","type":"page","url":"https://example.test/"}));
        assert_eq!(cdp.call("Target.getTargetInfo", json!({"targetId":"E"}), None).unwrap_err(), GONE, "{method}");
    }
    let (mut cdp, _) = closing_client(|_| false);
    assert!(cdp.call("Target.getTargets", json!({}), None).is_ok());
}

#[test]
fn a_close_on_the_guard_setup_frame_tree_fails_the_attach_with_the_close() {
    let (mut cdp, sent) = failing_client(nth("Page.getFrameTree", 3), counted("Page.getFrameTree", |seen| seen < 3));
    assert_eq!(cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap_err(), GONE);
    let trees = sent.borrow().iter().filter(|call| call["method"] == "Page.getFrameTree").count();
    assert_eq!(trees, 3);
    assert_eq!(sent.borrow().last().unwrap()["method"], "Page.getFrameTree");
    assert!(!cdp.guards.contains_key("T"));
    let (mut cdp, sent) = failing_client(|_| false, counted("Page.getFrameTree", |seen| seen < 3));
    assert!(cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).is_ok());
    assert_eq!(sent.borrow().iter().filter(|call| call["method"] == "Page.getFrameTree").count(), 3);
}

