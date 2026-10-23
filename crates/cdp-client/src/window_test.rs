struct WindowBrowser {
    targets: Vec<Value>,
    children: Vec<Value>,
    window: i64,
    sent: Vec<Value>,
}

struct WindowBackend {
    inbox: VecDeque<String>,
    browser: std::rc::Rc<std::cell::RefCell<WindowBrowser>>,
    attached: usize,
}

impl Transport for WindowBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut browser = self.browser.borrow_mut();
        browser.sent.push(call.clone());
        let result = match call["method"].as_str().unwrap() {
            "Target.attachToTarget" => {
                self.attached += 1;
                json!({ "sessionId": if self.attached == 1 { "U" } else { "G" } })
            }
            "Target.getTargetInfo" => {
                let found = browser.targets.iter().find(|info| info["targetId"] == call["params"]["targetId"]).cloned();
                match found {
                    Some(info) => json!({ "targetInfo": info }),
                    None => {
                        self.inbox.push_back(json!({ "id": call["id"], "error": { "message": "No target with given id found" } }).to_string());
                        return Ok(());
                    }
                }
            }
            "Target.getTargets" => json!({ "targetInfos": browser.targets }),
            "Page.getFrameTree" => json!({ "frameTree": {
                "frame": { "id": "T", "loaderId": "LT", "url": "https://ordinary.test/", "securityOrigin": "https://ordinary.test" },
                "childFrames": browser.children,
            }}),
            "Runtime.evaluate" => json!({ "result": { "type": "string", "value": "complete" } }),
            "Browser.getWindowForTarget" => json!({ "windowId": browser.window, "bounds": { "left": 0, "top": 0, "width": 800, "height": 600, "windowState": "normal" } }),
            _ => json!({}),
        };
        self.inbox.push_back(json!({ "id": call["id"], "sessionId": call["sessionId"], "result": result }).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn window_page(id: &str, url: &str, context: &str) -> Value {
    json!({ "targetId": id, "type": "page", "url": url, "title": "", "browserContextId": context })
}

fn window_client(extra: Vec<Value>, children: Vec<Value>) -> (Cdp, String, std::rc::Rc<std::cell::RefCell<WindowBrowser>>) {
    let mut targets = vec![window_page("T", "https://ordinary.test/", "CTX")];
    targets.extend(extra);
    let browser = std::rc::Rc::new(std::cell::RefCell::new(WindowBrowser { targets, children, window: 7, sent: Vec::new() }));
    let mut cdp = Cdp::from_transport(Box::new(WindowBackend { inbox: VecDeque::new(), browser: browser.clone(), attached: 0 }));
    let session = super::attach(&mut cdp, "T").unwrap();
    (cdp, session, browser)
}

fn window_resized(browser: &std::rc::Rc<std::cell::RefCell<WindowBrowser>>) -> usize {
    browser.borrow().sent.iter().filter(|call| call["method"] == "Browser.setWindowBounds").count()
}

fn window_bounds(width: u64, height: u64) -> Value {
    json!({ "windowId": 7, "bounds": { "width": width, "height": height } })
}

#[test]
fn window_bounds_are_admitted_for_an_ordinary_page_alone_in_its_profile() {
    for extra in [
        vec![],
        vec![window_page("OTHER", "https://other.test/", "CTX")],
        vec![window_page("SEVEN", "https://seven.sliccy.ai/", "ELSEWHERE")],
        vec![json!({ "targetId": "SW", "type": "service_worker", "url": "https://seven.sliccy.ai/sw.js", "browserContextId": "CTX" })],
    ] {
        let (mut cdp, session, browser) = window_client(extra.clone(), vec![]);
        cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).unwrap_or_else(|error| panic!("{extra:?}: {error}"));
        assert_eq!(window_resized(&browser), 1, "{extra:?}");
    }
}

#[test]
fn window_bounds_are_refused_when_a_protected_or_unknown_target_could_share_the_window() {
    for extra in [
        window_page("SEVEN", "https://seven.sliccy.ai/?secret=1", "CTX"),
        window_page("VAULT", "http://vault.sliccy.ai/", "CTX"),
        window_page("BLANK", "", "CTX"),
        window_page("SETTINGS", "chrome://settings/", "CTX"),
        window_page("NOCTX", "https://seven.sliccy.ai/", ""),
        json!({ "targetId": "FRAME", "type": "iframe", "url": "https://vault.sliccy.ai/frame", "browserContextId": "CTX", "parentFrameId": "T" }),
        json!({ "targetId": "PANEL", "type": "other", "url": "chrome-extension://abc/panel.html", "browserContextId": "CTX" }),
        json!({ "targetId": "", "type": "page", "url": "https://other.test/", "browserContextId": "CTX" }),
    ] {
        let (mut cdp, session, browser) = window_client(vec![extra.clone()], vec![]);
        let refused = cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).err().unwrap_or_else(|| panic!("{extra} was admitted"));
        assert!(refused.starts_with("slicc: refused:"), "{extra}: {refused}");
        assert!(!refused.contains("secret"));
        assert_eq!(window_resized(&browser), 0, "{extra}");
    }
}

#[test]
fn window_bounds_are_refused_for_a_protected_child_frame_of_the_page() {
    let child = json!({ "frame": { "id": "C", "loaderId": "LC", "parentId": "T", "url": "https://seven.sliccy.ai/", "securityOrigin": "https://seven.sliccy.ai" } });
    let (mut cdp, session, browser) = window_client(vec![], vec![child]);
    assert!(cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).unwrap_err().starts_with("slicc: refused:"));
    assert_eq!(window_resized(&browser), 0);
}

#[test]
fn window_bounds_params_and_sessions_are_allowlisted() {
    for (params, session) in [
        (window_bounds(1024, 768), None),
        (window_bounds(1024, 768), Some("G")),
        (window_bounds(1024, 768), Some("NOBODY")),
        (json!({ "windowId": 8, "bounds": { "width": 1024, "height": 768 } }), Some("U")),
        (json!({ "windowId": "7", "bounds": { "width": 1024, "height": 768 } }), Some("U")),
        (json!({ "windowId": 7, "bounds": { "width": 1024, "height": 768, "left": 0 } }), Some("U")),
        (json!({ "windowId": 7, "bounds": { "windowState": "fullscreen" } }), Some("U")),
        (json!({ "windowId": 7, "bounds": { "width": 1024 } }), Some("U")),
        (json!({ "windowId": 7, "bounds": { "width": 1024, "height": 768 }, "extra": true }), Some("U")),
        (json!({ "windowId": 7 }), Some("U")),
        (json!([]), Some("U")),
        (window_bounds(0, 768), Some("U")),
        (window_bounds(1024, 10_000_001), Some("U")),
        (json!({ "windowId": 7, "bounds": { "width": 10.5, "height": 768 } }), Some("U")),
        (json!({ "windowId": 7, "bounds": { "width": -1, "height": 768 } }), Some("U")),
    ] {
        let (mut cdp, _, browser) = window_client(vec![], vec![]);
        let refused = cdp.call("Browser.setWindowBounds", params.clone(), session).unwrap_err();
        assert!(refused.starts_with("slicc: refused:"), "{params} {session:?}: {refused}");
        assert!(cdp.notify("Browser.setWindowBounds", params.clone(), session).is_err(), "{params}");
        assert_eq!(window_resized(&browser), 0, "{params} {session:?}");
    }
}

#[test]
fn window_bounds_are_refused_for_a_blocked_or_protected_session() {
    let (mut cdp, session, browser) = window_client(vec![], vec![]);
    cdp.blocked.insert(session.clone());
    assert!(cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).is_err());
    let (mut cdp, session, _) = window_client(vec![], vec![]);
    cdp.protected_targets.insert("T".into());
    assert!(cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).is_err());
    assert_eq!(window_resized(&browser), 0);
}

#[test]
fn window_bounds_are_refused_when_the_page_is_not_listed_as_an_ordinary_page() {
    let (mut cdp, session, browser) = window_client(vec![], vec![]);
    browser.borrow_mut().targets[0]["type"] = json!("other");
    assert!(cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).is_err());
    browser.borrow_mut().targets.clear();
    assert!(cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).is_err());
    assert_eq!(window_resized(&browser), 0);
}
