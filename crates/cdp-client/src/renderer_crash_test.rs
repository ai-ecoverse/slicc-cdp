use super::{unguarded, Navigation, DOCUMENT_CHECK, GUARD_SETUP};

struct RendererCrashBackend {
    inbox: VecDeque<String>,
    crash: Value,
    reply: bool,
    calls: Rc<RefCell<Vec<Value>>>,
}

impl Transport for RendererCrashBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        self.calls.borrow_mut().push(message.clone());
        if message["method"] == "Network.enable" {
            self.inbox.push_back(self.crash.to_string());
            if !self.reply { return Ok(()) }
        }
        self.inbox.push_back(json!({"id":message["id"],"result":{}}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(Some(self.inbox.pop_front().expect("waited after the renderer crash")))
    }
}

fn renderer_crash_client(crash: Value, reply: bool) -> (Cdp, Rc<RefCell<Vec<Value>>>) {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(RendererCrashBackend {
        inbox: VecDeque::new(), crash, reply, calls: Rc::clone(&calls),
    }));
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.guards.insert("T".into(), "G".into());
    cdp.guard_sessions.insert("G".into());
    (cdp, calls)
}

#[test]
fn renderer_crash_on_the_guard_fails_without_another_receive_or_an_ack() {
    let (mut cdp, calls) = renderer_crash_client(json!({"method":"Inspector.targetCrashed","sessionId":"G","params":{}}), false);
    assert_eq!(cdp.raw_call("Network.enable", json!({}), Some("S"), GUARD_SETUP).unwrap_err(), "slicc: refused: renderer crashed\n");
    assert_eq!(calls.borrow().iter().filter(|call| call["method"] == "Network.enable").count(), 1);
}

#[test]
fn renderer_crash_reasons_take_precedence_over_late_success_and_detach_refusals() {
    for crash in [
        json!({"method":"Inspector.targetCrashed","sessionId":"S","params":{}}),
        json!({"method":"Inspector.detached","sessionId":"G","params":{"reason":"Render process gone."}}),
        json!({"method":"Inspector.detached","sessionId":"G","params":{"reason":"render_process_gone"}}),
        json!({"method":"Inspector.detached","sessionId":"G","params":{"reason":"renderer crashed"}}),
        json!({"method":"Target.detachedFromTarget","params":{"sessionId":"G","reason":"target_crashed"}}),
        json!({"method":"Target.detachedFromTarget","params":{"sessionId":"S","reason":"crashed"}}),
    ] {
        let (mut cdp, calls) = renderer_crash_client(crash, true);
        assert_eq!(cdp.raw_call("Network.enable", json!({}), Some("S"), GUARD_SETUP).unwrap_err(), "slicc: refused: renderer crashed\n");
        let from = calls.borrow().len();
        assert_eq!(cdp.raw_call("Network.enable", json!({}), Some("S"), GUARD_SETUP).unwrap_err(), "slicc: refused: renderer crashed\n");
        assert_eq!(calls.borrow().len(), from);
        assert_eq!(cdp.call("Network.enable", json!({}), Some("S")).unwrap_err(), "slicc: refused: renderer crashed\n");
        assert_eq!(cdp.settled_refusal("Network.enable", &json!({}), Some("S")).unwrap(), "slicc: refused: renderer crashed\n");
        assert_eq!(cdp.settled_refusal("Target.attachToTarget", &json!({"targetId":"T","flatten":true}), None).unwrap(), "slicc: refused: renderer crashed\n");
        cdp.raw_call("Browser.getVersion", json!({}), None, DOCUMENT_CHECK).unwrap();
    }
    assert_eq!(unguarded("slicc: refused: renderer crashed\n".into()), "slicc: refused: renderer crashed\n");
}

#[test]
fn renderer_crash_unrelated_targets_and_noncrash_detaches_do_not_abort_healthy_calls() {
    for crash in [
        json!({"method":"Inspector.targetCrashed","sessionId":"OTHER","params":{}}),
        json!({"method":"Inspector.targetCrashed","params":{}}),
        json!({"method":"Inspector.detached","sessionId":"G","params":{"reason":"replaced_with_devtools"}}),
        json!({"method":"Runtime.executionContextsCleared","sessionId":"G","params":{"reason":"crashed"}}),
    ] {
        let (mut cdp, _) = renderer_crash_client(crash, true);
        cdp.session_targets.insert("OTHER".into(), "U".into());
        cdp.raw_call("Network.enable", json!({}), Some("S"), DOCUMENT_CHECK).unwrap();
        cdp.raw_call("Browser.getVersion", json!({}), None, DOCUMENT_CHECK).unwrap();
    }
}

#[test]
fn renderer_crash_queued_replies_and_navigation_replies_cannot_report_success() {
    let (mut cdp, _) = renderer_crash_client(Value::Null, true);
    cdp.absorb(json!({"method":"Inspector.targetCrashed","sessionId":"G","params":{}}));
    cdp.call_replies.insert(700, json!({"id":700,"result":{"value":"late success"}}));
    assert_eq!(cdp.raw_reply("Runtime.evaluate", 700, Some("S"), Duration::ZERO, true).unwrap_err(), "slicc: refused: renderer crashed\n");
    cdp.navigations.insert(701, Navigation {
        params: json!({"url":"https://example.test/"}), session: Some("S".into()),
        reply: Some(json!({"id":701,"result":{"frameId":"F","loaderId":"L"}})), detached: false,
    });
    assert_eq!(cdp.poll_navigation(701).unwrap().unwrap_err(), "slicc: refused: renderer crashed\n");
    assert!(cdp.poll_navigation(701).is_none());
}
