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

struct CloseProbe {
    inbox: VecDeque<String>,
    sent: std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
    info: std::rc::Rc<std::cell::RefCell<Value>>,
    frame: std::rc::Rc<std::cell::RefCell<Value>>,
}

impl Transport for CloseProbe {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        self.sent.borrow_mut().push(message.clone());
        let result = match message["method"].as_str().unwrap() {
            "Target.getTargetInfo" => json!({"targetInfo":self.info.borrow().clone()}),
            "Target.getTargets" => json!({"targetInfos":[self.info.borrow().clone()]}),
            "Target.attachToTarget" => {
                let target = message["params"]["targetId"].as_str().unwrap_or("T");
                json!({"sessionId":format!("CLOSE-{target}")})
            }
            "Page.getFrameTree" => json!({"frameTree":{"frame":self.frame.borrow().clone()}}),
            "Target.closeTarget" => json!({"success":true}),
            "Target.detachFromTarget" => json!({}),
            _ => json!({}),
        };
        self.inbox.push_back(json!({"id":message["id"],"result":result}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn close_probe(url: &str) -> (Cdp, std::rc::Rc<std::cell::RefCell<Vec<Value>>>, std::rc::Rc<std::cell::RefCell<Value>>, std::rc::Rc<std::cell::RefCell<Value>>) {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let info = std::rc::Rc::new(std::cell::RefCell::new(json!({"targetId":"T","type":"page","url":url,"title":"SECRETTITLE"})));
    let frame = std::rc::Rc::new(std::cell::RefCell::new(json!({"id":"F","url":url,"securityOrigin":origin_of(url),"loaderId":"L"})));
    let mut cdp = Cdp::from_transport(Box::new(CloseProbe {
        inbox: VecDeque::new(),
        sent: std::rc::Rc::clone(&sent),
        info: std::rc::Rc::clone(&info),
        frame: std::rc::Rc::clone(&frame),
    }));
    cdp.protect_origins(&["http://127.0.0.1:8085"]);
    (cdp, sent, info, frame)
}

fn origin_of(url: &str) -> String {
    url::Url::parse(url).ok().map(|parsed| parsed.origin().ascii_serialization()).unwrap_or_else(|| "://".into())
}

fn refuse_close(cdp: &mut Cdp, sent: &std::rc::Rc<std::cell::RefCell<Vec<Value>>>, label: &str) {
    let before = sent.borrow().len();
    let refused = cdp.call("Target.closeTarget", json!({"targetId":"T"}), None).unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{label}: {refused}");
    assert!(!refused.contains("SECRET"), "{label}: {refused}");
    assert!(!sent.borrow()[before..].iter().any(|call| call["method"] == "Target.closeTarget"), "{label}");
}

fn allow_close(cdp: &mut Cdp, sent: &std::rc::Rc<std::cell::RefCell<Vec<Value>>>, label: &str) {
    let closed = cdp.call("Target.closeTarget", json!({"targetId":"T"}), None).unwrap();
    assert_eq!(closed, json!({"success":true}), "{label}");
    assert!(forwarded(sent, "Target.closeTarget", "T"), "{label}");
    assert!(!closed.to_string().contains("SECRET"), "{label}");
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
        let (mut cdp, sent, info, _) = close_probe(url);
        cdp.session_targets.insert("S".into(), "T".into());
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"type":"BackForwardCacheRestore","frame":{"id":"F","url":url,"securityOrigin":origin_of(url),"loaderId":"L0"}}}));
        *info.borrow_mut() = json!({"targetId":"T","type":"page","url":url,"title":"SECRETTITLE"});
        assert!(cdp.protected_targets.contains("T"), "{url} protected_set");
        refuse_close(&mut cdp, &sent, &format!("restored {url}"));
        cdp.session_targets.insert("S".into(), "T".into());
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"chrome-error://chromewebdata/","securityOrigin":"://","unreachableUrl":url,"loaderId":"L1"}}}));
        *info.borrow_mut() = json!({"targetId":"T","type":"page","url":url,"title":"SECRETTITLE"});
        assert!(cdp.target_protected("T", false).unwrap(), "{url}");
        assert!(cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap_err().starts_with("slicc: refused:"), "{url}");
        allow_close(&mut cdp, &sent, &format!("error {url}"));
    }
}

#[test]
fn inner_scheme_vault_and_seven_ui_pages_stay_unclosable() {
    for url in [
        "view-source:https://vault.sliccy.ai/secret?k=SECRET",
        "blob:https://vault.sliccy.ai/11111111-1111-1111-1111-111111111111",
        "filesystem:https://vault.sliccy.ai/temporary/a",
        "view-source:https://seven.sliccy.ai/os/",
    ] {
        let (mut cdp, sent, _, _) = close_probe(url);
        assert!(cdp.target_protected("T", false).unwrap(), "{url}");
        refuse_close(&mut cdp, &sent, url);
        assert!(cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap_err().starts_with("slicc: refused:"), "{url}");
        assert!(cdp.call("Target.activateTarget", json!({"targetId":"T"}), None).unwrap_err().starts_with("slicc: refused:"), "{url}");
    }
}

#[test]
fn seven_ui_with_userinfo_stays_unclosable() {
    let url = "https://user:secret@seven.sliccy.ai/os/index.html";
    let (mut cdp, sent, _, _) = close_probe(url);
    assert!(cdp.target_protected("T", false).unwrap(), "{url}");
    refuse_close(&mut cdp, &sent, url);
}

#[test]
fn navigating_to_the_vault_stays_unclosable() {
    let start = "https://example.test/start";
    let vault = "https://vault.sliccy.ai/app?k=SECRET";
    let (mut cdp, sent, info, frame) = close_probe(start);
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.guards.insert("T".into(), "S".into());
    cdp.guard_sessions.insert("S".into());
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L0"}}}));
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":"S","params":{"frameId":"F","url":vault,"navigationType":"differentDocument"}}));
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":start,"title":"ordinary"});
    *frame.borrow_mut() = json!({"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L0"});
    assert!(cdp.call("Target.activateTarget", json!({"targetId":"T"}), None).unwrap_err().starts_with("slicc: refused:"));
    refuse_close(&mut cdp, &sent, vault);
}

#[test]
fn an_error_page_whose_current_target_url_is_a_kept_live_url_stays_unclosable() {
    let failed = "https://vault.sliccy.ai/failed?k=SECRET";
    let live = "https://vault.sliccy.ai/app?k=SECRET";
    let (mut cdp, sent, info, frame) = close_probe(live);
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.guards.insert("T".into(), "S".into());
    cdp.guard_sessions.insert("S".into());
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"https://example.test/start","securityOrigin":"https://example.test","loaderId":"L0"}}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"chrome-error://chromewebdata/","securityOrigin":"://","unreachableUrl":failed,"loaderId":"L1"}}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":live,"title":"SECRETTITLE"}}}));
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":live,"title":"SECRETTITLE"});
    *frame.borrow_mut() = json!({"id":"F","url":"chrome-error://chromewebdata/","securityOrigin":"://","unreachableUrl":failed,"loaderId":"L1"});
    assert!(cdp.call("Target.activateTarget", json!({"targetId":"T"}), None).unwrap_err().starts_with("slicc: refused:"));
    refuse_close(&mut cdp, &sent, live);
}

#[test]
fn about_blank_with_a_kept_security_origin_stays_unclosable() {
    for (url, origin) in [
        ("about:blank", "https://vault.sliccy.ai"),
        ("about:srcdoc", "https://vault.sliccy.ai"),
        ("about:blank", "https://seven.sliccy.ai"),
        ("about:srcdoc", "https://seven.sliccy.ai"),
    ] {
        let (mut cdp, sent, info, frame) = close_probe(url);
        cdp.session_targets.insert("S".into(), "T".into());
        cdp.guards.insert("T".into(), "S".into());
        cdp.guard_sessions.insert("S".into());
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":url,"securityOrigin":origin,"loaderId":"L0"}}}));
        *info.borrow_mut() = json!({"targetId":"T","type":"page","url":url,"title":""});
        *frame.borrow_mut() = json!({"id":"F","url":url,"securityOrigin":origin,"loaderId":"L0"});
        assert!(cdp.is_protected_target("T"), "{url} {origin}");
        assert!(cdp.call("Target.activateTarget", json!({"targetId":"T"}), None).unwrap_err().starts_with("slicc: refused:"), "{url} {origin}");
        refuse_close(&mut cdp, &sent, &format!("{url} {origin}"));
    }
}

#[test]
fn intended_protected_closes_still_work_after_kept_hardening() {
    for url in [
        "http://127.0.0.1:8085/?code=SECRET",
        "https://seven.sliccy.ai/os/callback.html?code=SECRET",
        "https://www.sliccy.ai/?code=SECRET",
        "https://seven.sliccy.ai/landing?k=SECRET",
        "https://get.sliccy.ai/os/",
    ] {
        let (mut cdp, sent, _, _) = close_probe(url);
        assert!(cdp.target_protected("T", false).unwrap(), "{url}");
        allow_close(&mut cdp, &sent, url);
    }
}
