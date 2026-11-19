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
            "Target.getTargetInfo" => json!({ "targetInfo": browser.targets.iter().find(|info| info["targetId"] == call["params"]["targetId"]).cloned().unwrap_or(Value::Null) }),
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
fn window_bounds_are_admitted_whatever_else_shares_the_window() {
    for extra in [
        vec![],
        vec![window_page("OTHER", "https://other.test/", "CTX")],
        vec![window_page("SEVEN", "https://seven.sliccy.ai/", "ELSEWHERE")],
        vec![window_page("OS", "https://www.sliccy.ai/os/", "CTX")],
        vec![window_page("SEVEN", "https://seven.sliccy.ai/?secret=1", "CTX")],
        vec![window_page("VAULT", "http://vault.sliccy.ai/", "CTX")],
        vec![window_page("BLANK", "", "CTX")],
        vec![window_page("SETTINGS", "chrome://settings/", "CTX")],
        vec![window_page("NOCTX", "https://seven.sliccy.ai/", "")],
        vec![json!({ "targetId": "FRAME", "type": "iframe", "url": "https://vault.sliccy.ai/frame", "browserContextId": "CTX", "parentFrameId": "T" })],
        vec![json!({ "targetId": "PANEL", "type": "other", "url": "chrome-extension://abc/panel.html", "browserContextId": "CTX" })],
        vec![json!({ "targetId": "", "type": "page", "url": "https://other.test/", "browserContextId": "CTX" })],
        vec![json!({ "targetId": "SW", "type": "service_worker", "url": "https://seven.sliccy.ai/sw.js", "browserContextId": "CTX" })],
        vec![json!({ "targetId": "UISEVEN", "type": "browser_ui", "url": "https://seven.sliccy.ai/", "browserContextId": "CTX" })],
    ] {
        let (mut cdp, session, browser) = window_client(extra.clone(), vec![]);
        cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).unwrap_or_else(|error| panic!("{extra:?}: {error}"));
        assert_eq!(window_resized(&browser), 1, "{extra:?}");
        let others = browser.borrow().sent.iter().filter(|call| call["method"].as_str().is_some_and(|method| method.starts_with("Browser.")) && call["params"].get("targetId").is_some()).count();
        assert_eq!(others, 0, "{extra:?}");
    }
}

#[test]
fn an_about_blank_tab_beside_seven_os_in_one_window_is_resized() {
    let (mut cdp, session, browser) = window_client(vec![window_page("OS", "https://www.sliccy.ai/os/", "CTX")], vec![]);
    browser.borrow_mut().targets[0]["url"] = json!("about:blank");
    cdp.call("Browser.setWindowBounds", window_bounds(2048, 1536), Some(&session)).unwrap();
    let sent = browser.borrow().sent.iter().filter(|call| call["method"] == "Browser.setWindowBounds").cloned().collect::<Vec<_>>();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["sessionId"], json!(session));
    assert_eq!(sent[0]["params"], window_bounds(2048, 1536));
}

#[test]
fn window_bounds_are_admitted_for_a_page_with_a_protected_child_frame() {
    let child = json!({ "frame": { "id": "C", "loaderId": "LC", "parentId": "T", "url": "https://seven.sliccy.ai/", "securityOrigin": "https://seven.sliccy.ai" } });
    let (mut cdp, session, browser) = window_client(vec![], vec![child]);
    cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).unwrap();
    assert_eq!(window_resized(&browser), 1);
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
fn window_bounds_shape_and_window_refusals_do_not_blame_seven() {
    for (params, expected) in [
        (json!({ "windowId": 7, "bounds": { "windowState": "minimized" } }), crate::cdp::WINDOW_SHAPE),
        (json!({ "windowId": 8, "bounds": { "width": 1024, "height": 768 } }), crate::cdp::WINDOW_OWN),
    ] {
        let (mut cdp, session, _) = window_client(vec![window_page("OS", "https://www.sliccy.ai/os/", "CTX")], vec![]);
        assert_eq!(cdp.call("Browser.setWindowBounds", params, Some(&session)).unwrap_err(), expected);
    }
    let (mut cdp, _, _) = window_client(vec![], vec![]);
    let sessionless: Option<&str> = None;
    assert_eq!(cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), sessionless).unwrap_err(), crate::cdp::WINDOW_OWN);
}

#[test]
fn window_bounds_are_refused_for_a_blocked_or_protected_session() {
    let (mut cdp, session, browser) = window_client(vec![], vec![]);
    cdp.blocked.insert(session.clone());
    assert!(cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).unwrap_err().starts_with("slicc: refused:"));
    assert_eq!(window_resized(&browser), 0);
    let (mut cdp, session, browser) = window_client(vec![], vec![]);
    cdp.protected_targets.insert("T".into());
    assert!(cdp.call("Browser.setWindowBounds", window_bounds(1024, 768), Some(&session)).unwrap_err().starts_with("slicc: refused:"));
    assert_eq!(window_resized(&browser), 0);
}
