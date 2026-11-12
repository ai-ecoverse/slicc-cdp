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
const CLOSABLE: [&str; 4] = ["P", "C", "W", "G"];
const KEPT: [&str; 8] = ["O", "D", "M", "L", "V", "X", "H", "N"];

fn tab_info(target: &str) -> Value {
    let (id, kind, url) = TABS.iter().find(|(id, _, _)| *id == target).copied().unwrap_or(("T", "page", "https://example.test/"));
    json!({"targetId":id,"type":kind,"url":url,"title":if id == "T" { "ordinary" } else { "SECRETTITLE" }})
}

impl Transport for TabsBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        self.sent.borrow_mut().push(message.clone());
        let target = message["params"]["targetId"]
            .as_str()
            .or_else(|| message["sessionId"].as_str().and_then(|session| session.strip_prefix('S')))
            .unwrap_or("T");
        let result = match message["method"].as_str().unwrap() {
            "Target.getTargetInfo" => json!({"targetInfo":tab_info(target)}),
            "Target.getTargets" => json!({"targetInfos":Value::Array(TABS.iter().map(|(id, _, _)| tab_info(id)).collect())}),
            "Target.attachToTarget" => json!({"sessionId":format!("S{target}")}),
            "Page.getFrameTree" => {
                let info = tab_info(target);
                let url = info["url"].as_str().unwrap_or("");
                json!({"frameTree":{"frame":{"id":format!("F{target}"),"url":url,"securityOrigin":origin_of(url),"loaderId":"L"}}})
            }
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

fn tabs_client() -> (Cdp, std::rc::Rc<std::cell::RefCell<Vec<Value>>>) {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(TabsBackend { inbox: VecDeque::new(), sent: std::rc::Rc::clone(&sent) }));
    cdp.protect_origins(&["http://127.0.0.1:8085"]);
    (cdp, sent)
}

fn forwarded(sent: &std::rc::Rc<std::cell::RefCell<Vec<Value>>>, method: &str, target: &str) -> bool {
    sent.borrow().iter().any(|call| call["method"] == method && call["params"]["targetId"] == target)
}

#[derive(Clone, Copy, Debug)]
enum ProbeMode {
    Ok,
    Timeout,
    Error,
}

struct CloseProbe {
    inbox: VecDeque<String>,
    sent: std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
    info: std::rc::Rc<std::cell::RefCell<Value>>,
    frame: std::rc::Rc<std::cell::RefCell<Value>>,
    info_mode: std::rc::Rc<std::cell::RefCell<ProbeMode>>,
    frame_mode: std::rc::Rc<std::cell::RefCell<ProbeMode>>,
    attach_mode: std::rc::Rc<std::cell::RefCell<ProbeMode>>,
    frame_fails: std::rc::Rc<std::cell::RefCell<usize>>,
    info_queue: std::rc::Rc<std::cell::RefCell<VecDeque<Value>>>,
    frame_changed: std::rc::Rc<std::cell::RefCell<Option<Value>>>,
    opener: std::rc::Rc<std::cell::RefCell<Option<Value>>>,
}

impl Transport for CloseProbe {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        self.sent.borrow_mut().push(message.clone());
        let method = message["method"].as_str().unwrap();
        let mode = match method {
            "Target.getTargetInfo" => {
                let target = message["params"]["targetId"].as_str().unwrap_or("T");
                if target != "T" {
                    if let Some(opener) = self.opener.borrow().as_ref() {
                        if opener.get("targetId").and_then(Value::as_str) == Some(target) {
                            self.inbox.push_back(json!({"id":message["id"],"result":{"targetInfo":opener}}).to_string());
                            return Ok(());
                        }
                    }
                }
                *self.info_mode.borrow()
            }
            "Target.attachToTarget" => *self.attach_mode.borrow(),
            "Page.getFrameTree" => {
                if let Some(changed) = self.frame_changed.borrow_mut().take() {
                    self.inbox.push_back(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":changed}}).to_string());
                }
                if *self.frame_fails.borrow() > 0 {
                    *self.frame_fails.borrow_mut() -= 1;
                    ProbeMode::Timeout
                } else {
                    *self.frame_mode.borrow()
                }
            }
            _ => ProbeMode::Ok,
        };
        match mode {
            ProbeMode::Timeout => return Ok(()),
            ProbeMode::Error => {
                self.inbox.push_back(json!({"id":message["id"],"error":{"message":"fake CDP error"}}).to_string());
                return Ok(());
            }
            ProbeMode::Ok => {}
        }
        let result = match method {
            "Target.getTargetInfo" => {
                let queued = self.info_queue.borrow_mut().pop_front();
                let target_info = queued.unwrap_or_else(|| self.info.borrow().clone());
                *self.info.borrow_mut() = target_info.clone();
                json!({"targetInfo":target_info})
            }
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
    let (cdp, sent, info, frame, _, _, _, _, _, _) = close_probe_modes(url);
    (cdp, sent, info, frame)
}

fn close_probe_modes(url: &str) -> (
    Cdp,
    std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
    std::rc::Rc<std::cell::RefCell<Value>>,
    std::rc::Rc<std::cell::RefCell<Value>>,
    std::rc::Rc<std::cell::RefCell<ProbeMode>>,
    std::rc::Rc<std::cell::RefCell<ProbeMode>>,
    std::rc::Rc<std::cell::RefCell<ProbeMode>>,
    std::rc::Rc<std::cell::RefCell<usize>>,
    std::rc::Rc<std::cell::RefCell<Option<Value>>>,
    std::rc::Rc<std::cell::RefCell<Option<Value>>>,
) {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let info = std::rc::Rc::new(std::cell::RefCell::new(json!({"targetId":"T","type":"page","url":url,"title":"SECRETTITLE"})));
    let frame = std::rc::Rc::new(std::cell::RefCell::new(json!({"id":"F","url":url,"securityOrigin":origin_of(url),"loaderId":"L"})));
    let info_mode = std::rc::Rc::new(std::cell::RefCell::new(ProbeMode::Ok));
    let frame_mode = std::rc::Rc::new(std::cell::RefCell::new(ProbeMode::Ok));
    let attach_mode = std::rc::Rc::new(std::cell::RefCell::new(ProbeMode::Ok));
    let frame_fails = std::rc::Rc::new(std::cell::RefCell::new(0usize));
    let info_queue = std::rc::Rc::new(std::cell::RefCell::new(VecDeque::new()));
    let frame_changed = std::rc::Rc::new(std::cell::RefCell::new(None));
    let opener = std::rc::Rc::new(std::cell::RefCell::new(None));
    let mut cdp = Cdp::from_transport(Box::new(CloseProbe {
        inbox: VecDeque::new(),
        sent: std::rc::Rc::clone(&sent),
        info: std::rc::Rc::clone(&info),
        frame: std::rc::Rc::clone(&frame),
        info_mode: std::rc::Rc::clone(&info_mode),
        frame_mode: std::rc::Rc::clone(&frame_mode),
        attach_mode: std::rc::Rc::clone(&attach_mode),
        frame_fails: std::rc::Rc::clone(&frame_fails),
        info_queue: std::rc::Rc::clone(&info_queue),
        frame_changed: std::rc::Rc::clone(&frame_changed),
        opener: std::rc::Rc::clone(&opener),
    }));
    cdp.protect_origins(&["http://127.0.0.1:8085"]);
    (cdp, sent, info, frame, info_mode, frame_mode, attach_mode, frame_fails, frame_changed, opener)
}

fn close_probe_queue(url: &str) -> (
    Cdp,
    std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
    std::rc::Rc<std::cell::RefCell<Value>>,
    std::rc::Rc<std::cell::RefCell<ProbeMode>>,
    std::rc::Rc<std::cell::RefCell<ProbeMode>>,
    std::rc::Rc<std::cell::RefCell<usize>>,
    std::rc::Rc<std::cell::RefCell<Option<Value>>>,
    std::rc::Rc<std::cell::RefCell<VecDeque<Value>>>,
) {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let info = std::rc::Rc::new(std::cell::RefCell::new(json!({"targetId":"T","type":"page","url":url,"title":"ordinary"})));
    let frame = std::rc::Rc::new(std::cell::RefCell::new(json!({"id":"F","url":url,"securityOrigin":origin_of(url),"loaderId":"L"})));
    let info_mode = std::rc::Rc::new(std::cell::RefCell::new(ProbeMode::Ok));
    let frame_mode = std::rc::Rc::new(std::cell::RefCell::new(ProbeMode::Ok));
    let attach_mode = std::rc::Rc::new(std::cell::RefCell::new(ProbeMode::Ok));
    let frame_fails = std::rc::Rc::new(std::cell::RefCell::new(0usize));
    let info_queue = std::rc::Rc::new(std::cell::RefCell::new(VecDeque::new()));
    let frame_changed = std::rc::Rc::new(std::cell::RefCell::new(None));
    let opener = std::rc::Rc::new(std::cell::RefCell::new(None));
    let mut cdp = Cdp::from_transport(Box::new(CloseProbe {
        inbox: VecDeque::new(),
        sent: std::rc::Rc::clone(&sent),
        info: std::rc::Rc::clone(&info),
        frame: std::rc::Rc::clone(&frame),
        info_mode: std::rc::Rc::clone(&info_mode),
        frame_mode: std::rc::Rc::clone(&frame_mode),
        attach_mode: std::rc::Rc::clone(&attach_mode),
        frame_fails: std::rc::Rc::clone(&frame_fails),
        info_queue: std::rc::Rc::clone(&info_queue),
        frame_changed: std::rc::Rc::clone(&frame_changed),
        opener: std::rc::Rc::clone(&opener),
    }));
    cdp.protect_origins(&["http://127.0.0.1:8085"]);
    (cdp, sent, frame, frame_mode, attach_mode, frame_fails, frame_changed, info_queue)
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
        let (mut cdp, sent, info, frame) = close_probe(url);
        cdp.session_targets.insert("S".into(), "T".into());
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"type":"BackForwardCacheRestore","frame":{"id":"F","url":url,"securityOrigin":origin_of(url),"loaderId":"L0"}}}));
        *info.borrow_mut() = json!({"targetId":"T","type":"page","url":url,"title":"SECRETTITLE"});
        *frame.borrow_mut() = json!({"id":"F","url":url,"securityOrigin":origin_of(url),"loaderId":"L0"});
        assert!(cdp.protected_targets.contains("T"), "{url} protected_set");
        refuse_close(&mut cdp, &sent, &format!("restored {url}"));
        cdp.session_targets.insert("S".into(), "T".into());
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"chrome-error://chromewebdata/","securityOrigin":"://","unreachableUrl":url,"loaderId":"L1"}}}));
        *info.borrow_mut() = json!({"targetId":"T","type":"page","url":url,"title":"SECRETTITLE"});
        *frame.borrow_mut() = json!({"id":"F","url":"chrome-error://chromewebdata/","securityOrigin":"://","unreachableUrl":url,"loaderId":"L1"});
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
fn userinfo_on_kept_origins_stays_unclosable() {
    for url in [
        "http://user:pass@127.0.0.1:8085/os/index.html",
        "http://user:pass@seven.sliccy.ai:9443/os/",
        "https://user:pass@seven.sliccy.ai/os/index.html",
        "http://user:pass@vault.sliccy.ai:8080/secret",
        "https://user:pass@vault.sliccy.ai/app?k=SECRET",
    ] {
        let (mut cdp, sent, _, _) = close_probe(url);
        assert!(cdp.target_protected("T", false).unwrap(), "{url}");
        refuse_close(&mut cdp, &sent, url);
    }
}

#[test]
fn userinfo_on_seven_callback_stays_closable() {
    let url = "https://user:pass@seven.sliccy.ai/os/callback.html?code=SECRET";
    let (mut cdp, sent, _, _) = close_probe(url);
    assert!(cdp.target_protected("T", false).unwrap(), "{url}");
    allow_close(&mut cdp, &sent, url);
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

#[test]
fn a_failed_or_timed_out_close_proof_refuses_a_protected_tab() {
    let landing = "https://seven.sliccy.ai/landing?k=SECRET";
    let live = "https://seven.sliccy.ai/os/?k=SECRET";
    for mode in [ProbeMode::Timeout, ProbeMode::Error] {
        let (mut cdp, sent, _info, frame, info_mode, _, _, _, _, _) = close_probe_modes(landing);
        *frame.borrow_mut() = json!({"id":"F","url":live,"securityOrigin":"https://seven.sliccy.ai","loaderId":"L"});
        assert!(cdp.call("Target.activateTarget", json!({"targetId":"T"}), None).unwrap_err().starts_with("slicc: refused:"));
        *info_mode.borrow_mut() = mode;
        refuse_close(&mut cdp, &sent, &format!("getTargetInfo {mode:?}"));
        assert!(!sent.borrow().iter().any(|call| call["method"] == "Target.closeTarget"), "{mode:?}");
    }
    let vault = "https://vault.sliccy.ai/app?k=SECRET";
    let (mut cdp, sent, info, frame, _, frame_mode, _, _, _, _) = close_probe_modes(vault);
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.guards.insert("T".into(), "S".into());
    cdp.guard_sessions.insert("S".into());
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"chrome-error://chromewebdata/","securityOrigin":"://","unreachableUrl":vault,"loaderId":"L1"}}}));
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":vault,"title":"SECRETTITLE"});
    *frame.borrow_mut() = json!({"id":"F","url":"chrome-error://chromewebdata/","securityOrigin":"://","unreachableUrl":vault,"loaderId":"L1"});
    assert!(cdp.target_protected("T", false).unwrap());
    *frame_mode.borrow_mut() = ProbeMode::Timeout;
    refuse_close(&mut cdp, &sent, "getFrameTree timeout over stale error");
    let blank = "about:blank";
    let (mut cdp, sent, info, frame, _, frame_mode, _, _, _, opener) = close_probe_modes(blank);
    *opener.borrow_mut() = Some(json!({"targetId":"O","type":"page","url":"https://vault.sliccy.ai/app","title":""}));
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":blank,"title":"","openerId":"O"});
    *frame.borrow_mut() = json!({"id":"F","url":blank,"securityOrigin":"https://vault.sliccy.ai","loaderId":"L"});
    assert!(cdp.target_protected("T", false).unwrap());
    *frame_mode.borrow_mut() = ProbeMode::Error;
    refuse_close(&mut cdp, &sent, "getFrameTree CDP error on about:blank");
}

#[test]
fn a_failed_close_proof_with_a_kept_target_info_still_refuses() {
    let start = "https://example.test/start";
    let vault = "https://vault.sliccy.ai/app?k=SECRET";
    let start_info = json!({"targetId":"T","type":"page","url":start,"title":"ordinary"});
    let vault_info = json!({"targetId":"T","type":"page","url":vault,"title":"SECRETTITLE"});
    for (label, frame_mode_set, attach_mode_set) in [
        ("vault then getFrameTree timeout", Some(ProbeMode::Timeout), None),
        ("vault then getFrameTree error", Some(ProbeMode::Error), None),
        ("vault then attach timeout", None, Some(ProbeMode::Timeout)),
    ] {
        let (mut cdp, sent, frame, frame_mode, attach_mode, _, _, info_queue) = close_probe_queue(start);
        info_queue.borrow_mut().push_back(start_info.clone());
        info_queue.borrow_mut().push_back(vault_info.clone());
        *frame.borrow_mut() = json!({"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L"});
        if let Some(mode) = frame_mode_set {
            *frame_mode.borrow_mut() = mode;
        }
        if let Some(mode) = attach_mode_set {
            *attach_mode.borrow_mut() = mode;
        }
        assert!(!cdp.protected_targets.contains("T"), "{label}");
        refuse_close(&mut cdp, &sent, label);
        assert!(!sent.borrow().iter().any(|call| call["method"] == "Target.closeTarget"), "{label}");
    }
}

#[test]
fn an_unmarked_unresponsive_ordinary_page_closes_after_retries() {
    let start = "https://example.test/start";
    let start_info = json!({"targetId":"T","type":"page","url":start,"title":"ordinary"});
    let (mut cdp, sent, frame, frame_mode, _, _, _, info_queue) = close_probe_queue(start);
    info_queue.borrow_mut().push_back(start_info.clone());
    info_queue.borrow_mut().push_back(start_info);
    *frame.borrow_mut() = json!({"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L"});
    *frame_mode.borrow_mut() = ProbeMode::Timeout;
    assert!(!cdp.protected_targets.contains("T"));
    allow_close(&mut cdp, &sent, "ordinary hung frame");
    let frames = sent.borrow().iter().filter(|call| call["method"] == "Page.getFrameTree").count();
    assert!(frames >= 2, "expected retries, got {frames}");
}

#[test]
fn a_hung_frame_during_navigation_to_the_vault_stays_unclosable() {
    let start = "https://example.test/start";
    let vault = "https://vault.sliccy.ai/app?k=SECRET";
    let start_info = json!({"targetId":"T","type":"page","url":start,"title":"ordinary"});
    let (mut cdp, sent, frame, frame_mode, _, _, _, info_queue) = close_probe_queue(start);
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.guards.insert("T".into(), "S".into());
    cdp.guard_sessions.insert("S".into());
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L0"}}}));
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":"S","params":{"frameId":"F","url":vault,"navigationType":"differentDocument"}}));
    assert!(cdp.documents.get("T").is_some_and(|doc| doc.navigating && doc.navigation_url == vault), "navigation state recorded");
    info_queue.borrow_mut().push_back(start_info.clone());
    info_queue.borrow_mut().push_back(start_info);
    *frame.borrow_mut() = json!({"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L0"});
    *frame_mode.borrow_mut() = ProbeMode::Timeout;
    refuse_close(&mut cdp, &sent, "hung while navigating to vault");
    assert!(!sent.borrow().iter().any(|call| call["method"] == "Target.closeTarget"));
}

#[test]
fn a_new_tab_whose_first_frame_read_fails_then_succeeds_closes() {
    let start = "https://example.test/fresh";
    let start_info = json!({"targetId":"T","type":"page","url":start,"title":"ordinary"});
    let (mut cdp, sent, frame, _, _, frame_fails, _, info_queue) = close_probe_queue(start);
    info_queue.borrow_mut().push_back(start_info.clone());
    info_queue.borrow_mut().push_back(start_info);
    *frame.borrow_mut() = json!({"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L"});
    *frame_fails.borrow_mut() = 1;
    allow_close(&mut cdp, &sent, "first frame fail then succeed");
    let frames = sent.borrow().iter().filter(|call| call["method"] == "Page.getFrameTree").count();
    assert!(frames >= 2, "expected a retry after the first failure, got {frames}");
}

#[test]
fn a_kept_target_info_changed_during_a_hung_frame_stays_unclosable() {
    let start = "https://example.test/start";
    let vault = "https://vault.sliccy.ai/app?k=SECRET";
    let start_info = json!({"targetId":"T","type":"page","url":start,"title":"ordinary"});
    let (mut cdp, sent, frame, frame_mode, _, _, frame_changed, info_queue) = close_probe_queue(start);
    info_queue.borrow_mut().push_back(start_info.clone());
    info_queue.borrow_mut().push_back(start_info);
    *frame.borrow_mut() = json!({"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L"});
    *frame_mode.borrow_mut() = ProbeMode::Timeout;
    *frame_changed.borrow_mut() = Some(json!({"targetId":"T","type":"page","url":vault,"title":"SECRETTITLE"}));
    refuse_close(&mut cdp, &sent, "targetInfoChanged to vault during hung frame");
    assert!(cdp.is_protected_target("T"), "kept URL latched from targetInfoChanged");
    assert!(!sent.borrow().iter().any(|call| call["method"] == "Target.closeTarget"));
}

#[test]
fn a_kept_target_info_changed_before_close_is_not_cleared_by_ordinary_get_target_info() {
    let start = "https://example.test/start";
    let vault = "https://vault.sliccy.ai/app?k=SECRET";
    let start_info = json!({"targetId":"T","type":"page","url":start,"title":"ordinary"});
    let (mut cdp, sent, frame, frame_mode, _, _, _, info_queue) = close_probe_queue(start);
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":vault,"title":"SECRETTITLE"}}}));
    assert!(!cdp.events().iter().any(|event| event.to_string().contains("SECRET")), "event redacted");
    info_queue.borrow_mut().push_back(start_info.clone());
    info_queue.borrow_mut().push_back(start_info);
    *frame.borrow_mut() = json!({"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L"});
    *frame_mode.borrow_mut() = ProbeMode::Timeout;
    refuse_close(&mut cdp, &sent, "ordinary getTargetInfo after latched vault");
    assert!(cdp.is_protected_target("T"), "ordinary getTargetInfo must not clear the kept latch");
    assert!(!sent.borrow().iter().any(|call| call["method"] == "Target.closeTarget"));
}

#[test]
fn a_popup_whose_target_info_changed_to_the_vault_during_a_hung_frame_stays_unclosable() {
    let start = "https://example.test/start";
    let blank = "about:blank";
    let vault = "https://vault.sliccy.ai/app?k=SECRET";
    let blank_info = json!({"targetId":"T","type":"page","url":blank,"title":"","openerId":"O"});
    let (mut cdp, sent, info, frame, _, frame_mode, _, _, frame_changed, opener) = close_probe_modes(blank);
    *opener.borrow_mut() = Some(json!({"targetId":"O","type":"page","url":start,"title":"ordinary"}));
    *info.borrow_mut() = blank_info.clone();
    *frame.borrow_mut() = json!({"id":"F","url":blank,"securityOrigin":"https://example.test","loaderId":"L"});
    *frame_mode.borrow_mut() = ProbeMode::Timeout;
    *frame_changed.borrow_mut() = Some(json!({"targetId":"T","type":"page","url":vault,"title":"SECRETTITLE"}));
    refuse_close(&mut cdp, &sent, "popup targetInfoChanged to vault during hung frame");
    assert!(cdp.is_protected_target("T"), "popup kept URL latched");
    assert!(!sent.borrow().iter().any(|call| call["method"] == "Target.closeTarget"));
}

#[test]
fn an_empty_frame_url_with_a_seven_security_origin_stays_unclosable() {
    let (mut cdp, sent, info, frame) = close_probe("");
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":"","title":""});
    *frame.borrow_mut() = json!({"id":"F","url":"","securityOrigin":"https://seven.sliccy.ai","loaderId":"L"});
    refuse_close(&mut cdp, &sent, "empty url with seven origin");
    assert!(sent.borrow().iter().any(|call| call["method"] == "Page.getFrameTree"), "empty-url frame read");
}

#[test]
fn a_target_info_frame_disagreement_without_a_session_stays_unclosable() {
    let vault = "https://vault.sliccy.ai/app?k=SECRET";
    let decoy = "https://example.test/start";
    let (mut cdp, sent, info, frame) = close_probe(vault);
    assert!(cdp.call("Target.activateTarget", json!({"targetId":"T"}), None).unwrap_err().starts_with("slicc: refused:"));
    assert!(!cdp.guards.contains_key("T"));
    assert!(!cdp.runtime_sessions.contains_key("T"));
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":decoy,"title":"ordinary"});
    *frame.borrow_mut() = json!({"id":"F","url":vault,"securityOrigin":"https://vault.sliccy.ai","loaderId":"L"});
    refuse_close(&mut cdp, &sent, "decoy targetInfo over vault frame");
    assert!(sent.borrow().iter().any(|call| call["method"] == "Page.getFrameTree"), "frame must be read");
}

#[test]
fn a_data_document_from_the_vault_stays_unclosable() {
    let data = "data:text/html,SECRET";
    let (mut cdp, sent, info, frame) = close_probe(data);
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":data,"title":""});
    *frame.borrow_mut() = json!({"id":"F","url":data,"securityOrigin":"https://vault.sliccy.ai","loaderId":"L"});
    refuse_close(&mut cdp, &sent, "data: with vault securityOrigin");
    assert!(sent.borrow().iter().any(|call| call["method"] == "Page.getFrameTree"), "data: frame read");
    assert!(cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap_err().starts_with("slicc: refused:"));
    let (mut cdp, sent, info, frame) = close_probe("https://example.test/");
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"https://example.test/","securityOrigin":"https://example.test","loaderId":"L0"}}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":data,"securityOrigin":null,"loaderId":"L1"}}}));
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"S","params":{"context":{"id":1,"origin":"https://vault.sliccy.ai","name":"","uniqueId":"U1","auxData":{"isDefault":true,"frameId":"F","type":"default"}}}}));
    assert!(cdp.is_protected_target("T"));
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":data,"title":""});
    *frame.borrow_mut() = json!({"id":"F","url":data,"securityOrigin":null,"loaderId":"L1"});
    refuse_close(&mut cdp, &sent, "data: with vault default context origin");
}

#[test]
fn nine_nested_view_source_wrappers_around_the_vault_stay_unclosable() {
    let url = format!("{}https://vault.sliccy.ai/secret?k=SECRET", "view-source:".repeat(9));
    let (mut cdp, sent, _, _) = close_probe(&url);
    assert!(cdp.target_protected("T", false).unwrap(), "{url}");
    let refused = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{refused}");
    assert!(!refused.contains("SECRET"), "{refused}");
    refuse_close(&mut cdp, &sent, &url);
}

#[test]
fn a_restored_protected_seven_document_that_is_not_the_live_ui_can_be_closed() {
    let history = "https://seven.sliccy.ai/doc?n=history";
    let failed = "https://seven.sliccy.ai/doc/path-secret?query-secret=x#fragment-secret";
    let (mut cdp, sent, info, frame) = close_probe(history);
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":history,"securityOrigin":"https://seven.sliccy.ai","loaderId":"L0"}}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"chrome-error://chromewebdata/","securityOrigin":"://","unreachableUrl":failed,"loaderId":"L1"}}}));
    assert!(cdp.is_protected_target("T"));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"https://example.test/blank","securityOrigin":"https://example.test","loaderId":"L2"}}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"type":"BackForwardCacheRestore","frame":{"id":"F","url":history,"securityOrigin":"https://seven.sliccy.ai","loaderId":"L0"}}}));
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.blocked.insert("S".into());
    cdp.detaching.clear();
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"S","params":{"context":{"id":2,"origin":"https://seven.sliccy.ai","name":"","uniqueId":"U1","auxData":{"isDefault":true,"frameId":"F","type":"default"}}}}));
    assert!(cdp.context_proofs.values().any(|proof| proof.default && proof.origin == "https://seven.sliccy.ai"), "default context proof recorded");
    assert_eq!(cdp.session_targets.get("S").map(String::as_str), Some("T"), "close probe reuses the attached session");
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":history,"title":"SECRETTITLE"});
    *frame.borrow_mut() = json!({"id":"F","url":history,"securityOrigin":"https://seven.sliccy.ai","loaderId":"L0"});
    assert!(cdp.is_protected_target("T"));
    assert!(cdp.documents.get("T").is_some_and(|doc| doc.previous), "restore keeps previous");
    assert!(cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap_err().starts_with("slicc: refused:"));
    assert!(cdp.context_proofs.values().any(|proof| proof.default && proof.origin == "https://seven.sliccy.ai"), "default context proof survives until close");
    assert_eq!(cdp.session_targets.get("S").map(String::as_str), Some("T"));
    allow_close(&mut cdp, &sent, history);
}
