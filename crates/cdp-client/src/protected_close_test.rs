struct TabsBackend {
    inbox: VecDeque<String>,
    sent: std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
}

fn tab_info(target: &str) -> Value {
    match target {
        "P" => json!({"targetId":"P","type":"page","url":"http://127.0.0.1:8085/?code=SECRETCODE","title":"SECRETCODE"}),
        "V" => json!({"targetId":"V","type":"page","url":"https://vault.sliccy.ai/landing?k=SECRETKEY","title":"vault"}),
        "U" => json!({"targetId":"U","type":"browser_ui","url":"chrome://omnibox-popup.top-chrome/","title":"Omnibox Popup"}),
        _ => json!({"targetId":target,"type":"page","url":"https://example.test/","title":"ordinary"}),
    }
}

impl Transport for TabsBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        self.sent.borrow_mut().push(message.clone());
        let target = message["params"]["targetId"].as_str().unwrap_or("T");
        let result = match message["method"].as_str().unwrap() {
            "Target.getTargetInfo" => json!({"targetInfo":tab_info(target)}),
            "Target.getTargets" => json!({"targetInfos":Value::Array(["P","V","U","T"].map(tab_info).to_vec())}),
            "Target.attachToTarget" => json!({"sessionId":format!("S{target}")}),
            "Target.closeTarget" => json!({"success":true}),
            _ => json!({}),
        };
        self.inbox.push_back(json!({"id":message["id"],"result":result}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn tabs_client() -> (Cdp, std::rc::Rc<std::cell::RefCell<Vec<Value>>>) {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(TabsBackend { inbox: VecDeque::new(), sent: std::rc::Rc::clone(&sent) }));
    cdp.protect_origins(&["http://127.0.0.1:8085"]);
    (cdp, sent)
}

fn forwarded(sent: &std::rc::Rc<std::cell::RefCell<Vec<Value>>>, method: &str, target: &str) -> bool {
    sent.borrow().iter().any(|call| call["method"] == method && call["params"]["targetId"] == target)
}

#[test]
fn a_protected_tab_can_be_closed_but_not_reached() {
    for tab in ["P", "V"] {
        let (mut cdp, sent) = tabs_client();
        assert!(cdp.target_protected(tab, false).unwrap(), "{tab}");
        for (method, params) in [
            ("Target.attachToTarget", json!({"targetId":tab,"flatten":true})),
            ("Target.activateTarget", json!({"targetId":tab})),
            ("Browser.getWindowForTarget", json!({"targetId":tab})),
        ] {
            let before = sent.borrow().len();
            let refused = cdp.call(method, params, None).unwrap_err();
            assert!(refused.starts_with("slicc: refused:"), "{tab} {method}: {refused}");
            assert!(!sent.borrow()[before..].iter().any(|call| call["method"] == method), "{tab} {method}");
        }
        cdp.session_targets.insert(format!("S{tab}"), tab.to_string());
        for (method, params) in [
            ("Runtime.evaluate", json!({"expression":"document.title"})),
            ("Page.navigate", json!({"url":"about:blank"})),
            ("Target.closeTarget", json!({"targetId":tab})),
        ] {
            let refused = cdp.call(method, params, Some(&format!("S{tab}"))).unwrap_err();
            assert!(refused.starts_with("slicc: refused:"), "{tab} {method} on its session: {refused}");
        }
        assert!(!forwarded(&sent, "Target.closeTarget", tab), "{tab}");
        let closed = cdp.call("Target.closeTarget", json!({"targetId":tab}), None).unwrap();
        assert_eq!(closed, json!({"success":true}), "{tab}");
        assert!(forwarded(&sent, "Target.closeTarget", tab), "{tab}");
        let listed = cdp.call("Target.getTargets", json!({}), None).unwrap().to_string();
        assert!(!listed.contains("SECRET"), "{listed}");
    }
}

#[test]
fn a_protected_tab_found_only_by_id_can_be_closed() {
    let (mut cdp, sent) = tabs_client();
    cdp.protect_target("P");
    assert!(!cdp.targets.contains_key("P"));
    assert_eq!(cdp.call("Target.closeTarget", json!({"targetId":"P"}), None).unwrap(), json!({"success":true}));
    assert!(forwarded(&sent, "Target.closeTarget", "P"));
    assert!(cdp.call("Target.attachToTarget", json!({"targetId":"P","flatten":true}), None).unwrap_err().starts_with("slicc: refused:"));
}

#[test]
fn protected_browser_ui_and_unknown_targets_stay_unclosable() {
    let (mut cdp, sent) = tabs_client();
    let refused = cdp.call("Target.closeTarget", json!({"targetId":"U"}), None).unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{refused}");
    assert!(!forwarded(&sent, "Target.closeTarget", "U"));
    let (mut cdp, sent) = tabs_client();
    cdp.protect_target("X");
    cdp.targets.insert("X".into(), json!({"targetId":"X","url":"https://seven.sliccy.ai/"}));
    assert!(cdp.call("Target.closeTarget", json!({"targetId":"X"}), None).unwrap_err().starts_with("slicc: refused:"));
    assert!(!forwarded(&sent, "Target.closeTarget", "X"));
}
