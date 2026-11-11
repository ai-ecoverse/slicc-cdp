struct TabsBackend {
    inbox: VecDeque<String>,
    sent: std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
}

const TABS: [(&str, &str, &str); 14] = [
    ("P", "page", "http://127.0.0.1:8085/?code=SECRETCODE"),
    ("C", "page", "https://seven.sliccy.ai/os/callback.html"),
    ("W", "page", "https://www.sliccy.ai/?code=SECRETCODE"),
    ("G", "page", "https://get.sliccy.ai/os/"),
    ("N", "page", "https://www.sliccy.ai:8443/os/"),
    ("O", "page", "https://seven.sliccy.ai/os/"),
    ("D", "page", "https://seven.sliccy.ai./os/terminal?k=SECRETKEY"),
    ("M", "page", "https://www.sliccy.ai/os/index.html"),
    ("L", "page", "http://127.0.0.1:8085/os/"),
    ("V", "page", "https://vault.sliccy.ai/landing?k=SECRETKEY"),
    ("X", "page", "https://vault-x.sliccy.ai/"),
    ("H", "page", "http://vault.sliccy.ai:8080/"),
    ("U", "browser_ui", "chrome://omnibox-popup.top-chrome/"),
    ("T", "page", "https://example.test/"),
];
const CLOSABLE: [&str; 5] = ["P", "C", "W", "G", "N"];
const KEPT: [&str; 7] = ["O", "D", "M", "L", "V", "X", "H"];

fn tab_info(target: &str) -> Value {
    let (id, kind, url) = TABS.iter().find(|(id, _, _)| *id == target).copied().unwrap_or(("T", "page", "https://example.test/"));
    json!({"targetId":id,"type":kind,"url":url,"title":if id == "T" { "ordinary" } else { "SECRETTITLE" }})
}

impl Transport for TabsBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        self.sent.borrow_mut().push(message.clone());
        let target = message["params"]["targetId"].as_str().unwrap_or("T");
        let result = match message["method"].as_str().unwrap() {
            "Target.getTargetInfo" => json!({"targetInfo":tab_info(target)}),
            "Target.getTargets" => json!({"targetInfos":Value::Array(TABS.iter().map(|(id, _, _)| tab_info(id)).collect())}),
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
    for tab in CLOSABLE {
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

#[test]
fn a_live_seven_ui_or_vault_page_stays_unclosable() {
    for tab in KEPT {
        let (mut cdp, sent) = tabs_client();
        assert!(cdp.target_protected(tab, false).unwrap(), "{tab}");
        for (method, params) in [
            ("Target.closeTarget", json!({"targetId":tab})),
            ("Target.attachToTarget", json!({"targetId":tab,"flatten":true})),
        ] {
            let refused = cdp.call(method, params, None).unwrap_err();
            assert!(refused.starts_with("slicc: refused:"), "{tab} {method}: {refused}");
        }
        assert!(!forwarded(&sent, "Target.closeTarget", tab), "{tab}");
    }
}

#[test]
fn an_error_page_left_by_a_failed_vault_or_seven_ui_load_can_be_closed() {
    for url in ["https://vault.sliccy.ai/path?secret", "https://seven.sliccy.ai/os/?secret"] {
        let mut cdp = error_page_client_url(url);
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"type":"BackForwardCacheRestore","frame":{"id":"F","url":url,"securityOrigin":url::Url::parse(url).unwrap().origin().ascii_serialization()}}}));
        assert!(cdp.is_protected_target("T"), "{url}");
        assert!(cdp.call("Target.closeTarget", json!({"targetId":"T"}), None).unwrap_err().starts_with("slicc: refused:"), "{url}");
        error_commit(&mut cdp, url, "://");
        assert!(cdp.target_protected("T", false).unwrap(), "{url}");
        assert!(cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap_err().starts_with("slicc: refused:"), "{url}");
        assert!(cdp.call("Target.closeTarget", json!({"targetId":"T"}), None).is_ok(), "{url}");
    }
}
