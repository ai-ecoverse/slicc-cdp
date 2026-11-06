use super::{PAGE_CHANGED, UNVERIFIED};

struct ScopeBrowser {
    url: String,
    origin: String,
    loader: String,
    navigate_on_tree: u32,
    navigate_to: Option<(String, String)>,
    silent_tree: bool,
    trees: u32,
    inventories: u32,
}

struct ScopeTransport {
    browser: Rc<RefCell<ScopeBrowser>>,
    inbox: VecDeque<String>,
    attached: usize,
}

impl Transport for ScopeTransport {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut browser = self.browser.borrow_mut();
        let method = call["method"].as_str().unwrap();
        if method == "Target.getTargets" && call["params"].get("filter").is_some() {
            browser.inventories += 1;
        }
        if method == "Page.getFrameTree" {
            browser.trees += 1;
            if browser.silent_tree {
                self.inbox.push_back(json!({ "id": call["id"], "sessionId": call["sessionId"], "error": { "message": "No frame" } }).to_string());
                return Ok(());
            }
            if browser.navigate_on_tree > 0 {
                browser.navigate_on_tree -= 1;
                let (url, origin) = browser.navigate_to.clone().unwrap_or_else(|| {
                    let next = browser.trees;
                    (format!("https://ordinary.test/n{next}"), "https://ordinary.test".into())
                });
                browser.loader = format!("L{}", browser.trees);
                browser.url = url.clone();
                browser.origin = origin.clone();
                let frame = json!({
                    "id": "T",
                    "loaderId": browser.loader,
                    "url": url,
                    "securityOrigin": origin,
                });
                self.inbox.push_back(json!({
                    "method": "Page.frameNavigated",
                    "sessionId": call["sessionId"],
                    "params": { "frame": frame, "type": "Navigation" },
                }).to_string());
                self.inbox.push_back(json!({
                    "method": "Target.targetInfoChanged",
                    "params": { "targetInfo": page(&browser.url) },
                }).to_string());
            }
        }
        let result = match method {
            "Target.attachToTarget" => {
                self.attached += 1;
                json!({ "sessionId": format!("S{}", self.attached) })
            }
            "Target.getTargetInfo" => json!({ "targetInfo": page(&browser.url) }),
            "Target.getTargets" => json!({ "targetInfos": [page(&browser.url)] }),
            "Page.getFrameTree" => json!({
                "frameTree": {
                    "frame": {
                        "id": "T",
                        "loaderId": browser.loader,
                        "url": browser.url,
                        "securityOrigin": browser.origin,
                    },
                },
            }),
            "DOM.describeNode" => json!({ "node": { "nodeId": 1, "backendNodeId": 1, "nodeName": "HTML" } }),
            "DOM.getDocument" => json!({ "root": { "nodeId": 1, "backendNodeId": 1, "nodeName": "HTML" } }),
            "Runtime.evaluate" => json!({ "result": { "type": "string", "value": "ok" } }),
            "Browser.getWindowForTarget" => json!({ "windowId": 1, "bounds": { "windowState": "normal" } }),
            _ => json!({}),
        };
        self.inbox.push_back(json!({ "id": call["id"], "sessionId": call["sessionId"], "result": result }).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn page(url: &str) -> Value {
    json!({ "targetId": "T", "type": "page", "url": url, "title": "", "browserContextId": "CTX" })
}

fn client(navigate_on_tree: u32, navigate_to: Option<(&str, &str)>, silent_tree: bool) -> (Cdp, String, Rc<RefCell<ScopeBrowser>>) {
    let browser = Rc::new(RefCell::new(ScopeBrowser {
        url: "https://ordinary.test/".into(),
        origin: "https://ordinary.test".into(),
        loader: "L0".into(),
        navigate_on_tree: 0,
        navigate_to: navigate_to.map(|(url, origin)| (url.into(), origin.into())),
        silent_tree: false,
        trees: 0,
        inventories: 0,
    }));
    let mut cdp = Cdp::from_transport(Box::new(ScopeTransport {
        browser: browser.clone(),
        inbox: VecDeque::new(),
        attached: 0,
    }));
    let session = super::attach(&mut cdp, "T").unwrap();
    {
        let mut browser = browser.borrow_mut();
        browser.trees = 0;
        browser.inventories = 0;
        browser.navigate_on_tree = navigate_on_tree;
        browser.silent_tree = silent_tree;
    }
    (cdp, session, browser)
}

#[test]
fn page_changed_refusal_is_classified_like_other_check_refusals() {
    assert!(PAGE_CHANGED.starts_with("slicc: refused: "));
    assert!(!PAGE_CHANGED.contains("seven"));
    assert!(!PAGE_CHANGED.to_lowercase().contains("vault"));
    assert!(!PAGE_CHANGED.to_lowercase().contains("protect"));
    assert_ne!(PAGE_CHANGED, UNVERIFIED);
}

#[test]
fn a_navigation_to_an_ordinary_document_during_the_check_retries_and_succeeds() {
    let (mut cdp, session, browser) = client(1, Some(("https://ordinary.test/done", "https://ordinary.test")), false);
    assert!(cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).is_ok());
    assert!(browser.borrow().inventories >= 2);
    assert!(browser.borrow().url.ends_with("/done"));
}

#[test]
fn a_navigation_to_a_protected_document_during_the_check_keeps_the_protected_refusal() {
    let (mut cdp, session, browser) = client(1, Some(("https://seven.sliccy.ai/app", "https://seven.sliccy.ai")), false);
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, format!("{}\n", cdp.guard.refusal()));
    assert_ne!(message, PAGE_CHANGED);
    assert_ne!(message, UNVERIFIED);
    assert!(browser.borrow().url.contains("seven.sliccy.ai"));
}

#[test]
fn a_page_that_keeps_changing_is_refused_after_exactly_three_attempts() {
    let (mut cdp, session, browser) = client(u32::MAX, None, false);
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, PAGE_CHANGED);
    assert_eq!(browser.borrow().inventories, 3);
}

#[test]
fn a_check_the_browser_does_not_answer_is_not_retried() {
    let (mut cdp, session, browser) = client(0, None, true);
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, UNVERIFIED);
    assert_eq!(browser.borrow().inventories, 1);
    assert_eq!(browser.borrow().trees, 1);
}

#[test]
fn a_reload_between_describe_node_checks_rechecks_the_new_document() {
    let (mut cdp, session, browser) = client(0, None, false);
    assert!(cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).is_ok());
    let first = browser.borrow().inventories;
    browser.borrow_mut().loader = "L-reload".into();
    browser.borrow_mut().url = "https://ordinary.test/reloaded".into();
    let frame = json!({
        "id": "T",
        "loaderId": "L-reload",
        "url": "https://ordinary.test/reloaded",
        "securityOrigin": "https://ordinary.test",
    });
    cdp.absorb(json!({
        "method": "Page.frameNavigated",
        "sessionId": session,
        "params": { "frame": frame, "type": "Navigation" },
    }));
    cdp.absorb(json!({
        "method": "Target.targetInfoChanged",
        "params": { "targetInfo": page("https://ordinary.test/reloaded") },
    }));
    assert!(cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).is_ok());
    assert!(browser.borrow().inventories > first);
    assert!(browser.borrow().url.ends_with("/reloaded"));
}
