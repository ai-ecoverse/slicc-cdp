struct LateReplyFixture {
    sent: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
    inbox: VecDeque<String>,
    probe: Option<Value>,
}

impl Transport for LateReplyFixture {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let request: Value = serde_json::from_str(text).unwrap();
        self.sent.lock().unwrap().push(request.clone());
        if request["method"] == "Page.getFrameTree" {
            self.probe = Some(request);
        } else {
            if let Some(probe) = self.probe.take() {
                self.inbox.push_back(json!({"id":probe["id"],"sessionId":probe["sessionId"],"result":{"owner":"internal"}}).to_string());
            }
            self.inbox.push_back(json!({"id":request["id"],"sessionId":request["sessionId"],"result":{"owner":"caller","session":request["sessionId"]}}).to_string());
        }
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

#[test]
fn late_internal_replies_never_reach_callers_or_reuse_timed_out_ids() {
    let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(LateReplyFixture { sent:sent.clone(), inbox:VecDeque::new(), probe:None }));
    for owner in ["S1", "S2"] {
        assert_eq!(cdp.raw_call("Page.getFrameTree", json!({}), Some(owner), Duration::ZERO).unwrap_err(), "Page.getFrameTree timed out\n");
        assert!(cdp.waiting_calls.is_empty());
        let reply = cdp.call_for("Browser.getVersion", json!({}), None, Duration::from_secs(1)).unwrap();
        assert_eq!(reply, json!({"owner":"caller","session":null}));
        assert!(cdp.call_replies.is_empty());
    }
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 4);
    assert_eq!(sent.iter().map(|request| request["id"].as_i64().unwrap()).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    assert_eq!(sent[0]["sessionId"], "S1");
    assert_eq!(sent[2]["sessionId"], "S2");
}

#[test]
fn out_of_order_replies_remain_owned_by_their_request_and_session() {
    let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(LateReplyFixture { sent, inbox:VecDeque::new(), probe:None }));
    let first = cdp.send("Runtime.getProperties", json!({"objectId":"first"}), Some("S1")).unwrap();
    let second = cdp.send("Runtime.getProperties", json!({"objectId":"second"}), Some("S2")).unwrap();
    cdp.waiting_calls.extend([first, second]);
    assert_eq!(cdp.raw_reply("Runtime.getProperties", second, Some("S2"), Duration::from_secs(1), false).unwrap(), json!({"owner":"caller","session":"S2"}));
    assert_eq!(cdp.raw_reply("Runtime.getProperties", first, Some("S1"), Duration::ZERO, false).unwrap(), json!({"owner":"caller","session":"S1"}));
    assert!(cdp.call_replies.is_empty());
}
