use super::{PAGE_CHANGED, UNVERIFIED};

struct ScopeBrowser {
    url: String,
    origin: String,
    loader: String,
    unreachable: String,
    iframes: Vec<Value>,
    navigate_on_tree: u32,
    navigate_to: Option<(String, String, String)>,
    protect_url_after_nav: Option<String>,
    bump_unknown_on_tree: bool,
    drop_iframes_after: Option<u32>,
    tree_error_with_nav: bool,
    tree_error_with_unknown: bool,
    tree_error_with_protected_child: bool,
    silent_tree: bool,
    hang_tree_after: Option<u32>,
    inventory_delay: Option<Duration>,
    mid_tree_iframe: Option<Value>,
    mid_tree_pause_session: Option<String>,
    iframe_after_first_inventory: Option<Value>,
    user_session: String,
    trees: u32,
    inventories: u32,
    forwarded: Vec<String>,
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
            if let Some(delay) = browser.inventory_delay { std::thread::sleep(delay); }
            browser.inventories += 1;
            if let Some(url) = browser.protect_url_after_nav.clone().filter(|_| browser.inventories > 1) {
                browser.url = url;
                browser.origin = browser.url.rsplit_once('/').map(|(origin, _)| origin.to_string()).unwrap_or_else(|| browser.url.clone());
            }
            if browser.drop_iframes_after.is_some_and(|after| browser.inventories > after) {
                browser.iframes.clear();
            }
            if browser.inventories > 1 {
                if let Some(iframe) = browser.iframe_after_first_inventory.clone() {
                    if !browser.iframes.iter().any(|info| info.get("targetId") == iframe.get("targetId")) {
                        browser.iframes.push(iframe);
                    }
                }
            }
        }
        if matches!(
            method,
            "DOM.describeNode"
                | "Page.captureScreenshot"
                | "Runtime.evaluate"
                | "Accessibility.getFullAXTree"
                | "CSS.getComputedStyleForNode"
        ) {
            browser.forwarded.push(method.to_string());
        }
        if method == "Page.getFrameTree" {
            browser.trees += 1;
            if browser.hang_tree_after.is_some_and(|after| browser.trees > after) {
                return Ok(());
            }
            if browser.silent_tree {
                self.inbox.push_back(json!({ "id": call["id"], "sessionId": call["sessionId"], "error": { "message": "No frame" } }).to_string());
                return Ok(());
            }
            if browser.tree_error_with_unknown {
                browser.tree_error_with_unknown = false;
                self.inbox.push_back(json!({
                    "method": "Page.frameAttached",
                    "sessionId": "UNKNOWN",
                    "params": { "frameId": "UNKNOWN", "parentFrameId": "UNKNOWN_PARENT" },
                }).to_string());
                self.inbox.push_back(json!({ "id": call["id"], "sessionId": call["sessionId"], "error": { "message": "No frame" } }).to_string());
                return Ok(());
            }
            if browser.tree_error_with_nav {
                browser.tree_error_with_nav = false;
                browser.loader = format!("L{}", browser.trees);
                let frame = json!({
                    "id": "T",
                    "loaderId": browser.loader,
                    "url": browser.url,
                    "securityOrigin": browser.origin,
                });
                self.inbox.push_back(json!({
                    "method": "Page.frameNavigated",
                    "sessionId": call["sessionId"],
                    "params": { "frame": frame, "type": "Navigation" },
                }).to_string());
                self.inbox.push_back(json!({ "id": call["id"], "sessionId": call["sessionId"], "error": { "message": "No frame" } }).to_string());
                return Ok(());
            }
            if browser.tree_error_with_protected_child {
                browser.tree_error_with_protected_child = false;
                self.inbox.push_back(json!({
                    "method": "Page.frameNavigated",
                    "sessionId": call["sessionId"],
                    "params": {
                        "frame": {
                            "id": "C",
                            "parentId": "T",
                            "loaderId": "LC",
                            "url": "https://seven.sliccy.ai/secret",
                            "securityOrigin": "https://seven.sliccy.ai",
                        },
                        "type": "Navigation",
                    },
                }).to_string());
                self.inbox.push_back(json!({ "id": call["id"], "sessionId": call["sessionId"], "error": { "message": "No frame" } }).to_string());
                return Ok(());
            }
            if let Some(iframe) = browser.mid_tree_iframe.take() {
                self.inbox.push_back(json!({
                    "method": "Target.targetInfoChanged",
                    "params": { "targetInfo": iframe },
                }).to_string());
            }
            if let Some(session) = browser.mid_tree_pause_session.take() {
                self.inbox.push_back(json!({
                    "method": "Fetch.requestPaused",
                    "sessionId": session,
                    "params": {
                        "requestId": "MID-PAUSE",
                        "resourceType": "Document",
                        "request": { "url": browser.url, "method": "GET", "headers": {}, "initialPriority": "VeryHigh", "referrerPolicy": "no-referrer" },
                        "frameId": "T",
                        "networkId": "NET-MID-PAUSE",
                    },
                }).to_string());
            }
            if browser.bump_unknown_on_tree && browser.navigate_on_tree > 0 {
                browser.navigate_on_tree -= 1;
                self.inbox.push_back(json!({
                    "method": "Page.frameAttached",
                    "sessionId": "UNKNOWN",
                    "params": { "frameId": "UNKNOWN", "parentFrameId": "UNKNOWN_PARENT" },
                }).to_string());
            } else if browser.navigate_on_tree > 0 {
                browser.navigate_on_tree -= 1;
                let (url, origin, unreachable) = browser.navigate_to.clone().unwrap_or_else(|| {
                    let next = browser.trees;
                    (format!("https://ordinary.test/n{next}"), "https://ordinary.test".into(), String::new())
                });
                browser.loader = format!("L{}", browser.trees);
                browser.url = url.clone();
                browser.origin = origin.clone();
                browser.unreachable = unreachable.clone();
                let mut frame = json!({
                    "id": "T",
                    "loaderId": browser.loader,
                    "url": url,
                    "securityOrigin": origin,
                });
                if !unreachable.is_empty() {
                    frame["unreachableUrl"] = json!(unreachable);
                }
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
            "Target.getTargets" => {
                let mut infos = vec![page(&browser.url)];
                infos.extend(browser.iframes.iter().cloned());
                json!({ "targetInfos": infos })
            }
            "Page.getFrameTree" => json!({
                "frameTree": {
                    "frame": {
                        "id": "T",
                        "loaderId": browser.loader,
                        "url": browser.url,
                        "securityOrigin": browser.origin,
                        "unreachableUrl": browser.unreachable,
                    },
                },
            }),
            "Runtime.enable" => {
                self.inbox.push_back(json!({
                    "method": "Runtime.executionContextCreated",
                    "sessionId": call["sessionId"],
                    "params": {
                        "context": {
                            "id": 1,
                            "uniqueId": format!("context:{}", browser.loader),
                            "origin": browser.origin,
                            "name": "",
                            "auxData": { "frameId": "T", "isDefault": true },
                        },
                    },
                }).to_string());
                json!({})
            }
            "DOM.describeNode" => json!({ "node": { "nodeId": 1, "backendNodeId": 1, "nodeName": "HTML" } }),
            "DOM.getDocument" => json!({ "root": { "nodeId": 1, "backendNodeId": 1, "nodeName": "HTML" } }),
            "Page.captureScreenshot" => json!({ "data": "pixels" }),
            "Accessibility.getFullAXTree" => json!({ "nodes": [] }),
            "CSS.getComputedStyleForNode" => json!({ "computedStyle": [] }),
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

fn iframe(id: &str, url: &str) -> Value {
    json!({ "targetId": id, "type": "iframe", "parentFrameId": "T", "url": url, "title": "", "browserContextId": "CTX" })
}

fn iframe_parent(id: &str, parent: &str, url: &str) -> Value {
    json!({ "targetId": id, "type": "iframe", "parentFrameId": parent, "url": url, "title": "", "browserContextId": "CTX" })
}

fn client(navigate_on_tree: u32, navigate_to: Option<(&str, &str)>, silent_tree: bool) -> (Cdp, String, Rc<RefCell<ScopeBrowser>>) {
    configured(navigate_on_tree, navigate_to.map(|(url, origin)| (url, origin, "")), silent_tree, false, false, None, None, vec![])
}

fn configured(
    navigate_on_tree: u32,
    navigate_to: Option<(&str, &str, &str)>,
    silent_tree: bool,
    tree_error_with_nav: bool,
    bump_unknown_on_tree: bool,
    protect_url_after_nav: Option<&str>,
    drop_iframes_after: Option<u32>,
    iframes: Vec<Value>,
) -> (Cdp, String, Rc<RefCell<ScopeBrowser>>) {
    let browser = Rc::new(RefCell::new(ScopeBrowser {
        url: "https://ordinary.test/".into(),
        origin: "https://ordinary.test".into(),
        loader: "L0".into(),
        unreachable: String::new(),
        iframes: iframes.clone(),
        navigate_on_tree: 0,
        navigate_to: navigate_to.map(|(url, origin, unreachable)| (url.into(), origin.into(), unreachable.into())),
        protect_url_after_nav: None,
        bump_unknown_on_tree: false,
        drop_iframes_after: None,
        tree_error_with_nav: false,
        tree_error_with_unknown: false,
        tree_error_with_protected_child: false,
        silent_tree: false,
        hang_tree_after: None,
        inventory_delay: None,
        mid_tree_iframe: None,
        mid_tree_pause_session: None,
        iframe_after_first_inventory: None,
        user_session: String::new(),
        trees: 0,
        inventories: 0,
        forwarded: Vec::new(),
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
        browser.forwarded.clear();
        browser.navigate_on_tree = navigate_on_tree;
        browser.silent_tree = silent_tree;
        browser.tree_error_with_nav = tree_error_with_nav;
        browser.bump_unknown_on_tree = bump_unknown_on_tree;
        browser.protect_url_after_nav = protect_url_after_nav.map(str::to_string);
        browser.drop_iframes_after = drop_iframes_after;
        browser.iframes = iframes;
        browser.user_session = session.clone();
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
    let (mut cdp, session, browser) = configured(
        1, Some(("https://seven.sliccy.ai/app", "https://seven.sliccy.ai", "")), false, false, false, None, None, vec![],
    );
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, format!("{}\n", cdp.guard.refusal()));
    assert_ne!(message, PAGE_CHANGED);
    assert_ne!(message, UNVERIFIED);
    assert!(cdp.protected_targets.contains("T"));
    assert!(!browser.borrow().forwarded.iter().any(|method| method == "DOM.describeNode"));
}

#[test]
fn a_navigation_to_the_vault_during_the_check_keeps_the_protected_refusal() {
    let (mut cdp, session, browser) = configured(
        1, Some(("https://vault.sliccy.ai/box", "https://vault.sliccy.ai", "")), false, false, false, None, None, vec![],
    );
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, format!("{}\n", cdp.guard.refusal()));
    assert_ne!(message, PAGE_CHANGED);
    assert!(cdp.protected_targets.contains("T"));
    assert!(!browser.borrow().forwarded.iter().any(|method| method == "DOM.describeNode"));
}

#[test]
fn a_sealed_error_page_during_the_check_is_refused_and_not_forwarded() {
    for method in [
        "DOM.describeNode",
        "Page.captureScreenshot",
        "Runtime.evaluate",
        "Accessibility.getFullAXTree",
        "CSS.getComputedStyleForNode",
    ] {
        let (mut cdp, session, browser) = configured(
            1,
            Some(("chrome-error://chromewebdata/", "://", "")),
            false,
            false,
            false,
            None,
            None,
            vec![],
        );
        let params = if method == "Runtime.evaluate" {
            json!({ "expression": "1", "returnByValue": true })
        } else if method == "CSS.getComputedStyleForNode" {
            json!({ "nodeId": 1 })
        } else {
            json!({ "nodeId": 1 })
        };
        let message = cdp.call(method, params, Some(&session)).unwrap_err();
        assert!(message.starts_with("slicc: refused:"), "{method} {message}");
        assert_ne!(message, PAGE_CHANGED, "{method}");
        assert!(!browser.borrow().forwarded.iter().any(|sent| sent == method), "{method} {:?}", browser.borrow().forwarded);
    }
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
fn a_get_frame_tree_error_with_an_epoch_move_is_retried() {
    let (mut cdp, session, browser) = configured(0, None, false, true, false, None, None, vec![]);
    assert!(cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).is_ok());
    assert!(browser.borrow().inventories >= 2);
}

#[test]
fn a_get_frame_tree_error_overlapping_a_foreign_unknown_stays_unverified() {
    let (mut cdp, session, browser) = client(0, None, false);
    browser.borrow_mut().tree_error_with_unknown = true;
    browser.borrow_mut().inventories = 0;
    browser.borrow_mut().trees = 0;
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, UNVERIFIED);
    assert_eq!(browser.borrow().inventories, 1);
}

#[test]
fn a_runtime_evaluate_retries_a_main_frame_change() {
    let (mut cdp, session, browser) = client(0, None, false);
    assert!(cdp.call("Runtime.evaluate", json!({ "expression": "1", "returnByValue": true }), Some(&session)).is_ok());
    browser.borrow_mut().inventories = 0;
    browser.borrow_mut().forwarded.clear();
    browser.borrow_mut().navigate_on_tree = 1;
    browser.borrow_mut().navigate_to = Some(("https://ordinary.test/eval".into(), "https://ordinary.test".into(), String::new()));
    assert!(cdp.call("Runtime.evaluate", json!({ "expression": "1", "returnByValue": true }), Some(&session)).is_ok());
    assert!(browser.borrow().inventories >= 2);
    assert!(browser.borrow().url.ends_with("/eval"));
}

#[test]
fn a_closed_connection_is_not_retried_as_page_changed() {
    let (mut cdp, session, browser) = client(1, Some(("https://ordinary.test/x", "https://ordinary.test")), false);
    cdp.closed = Some("websocket closed\n".into());
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, "websocket closed\n");
    assert_ne!(message, PAGE_CHANGED);
    assert!(browser.borrow().inventories <= 1);
}

#[test]
fn a_paused_document_does_not_retry_the_page_check() {
    let (mut cdp, session, browser) = client(u32::MAX, None, false);
    cdp.paused_documents.insert((session.clone(), "REQ".into()));
    let start = Instant::now();
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, PAGE_CHANGED);
    assert_eq!(browser.borrow().inventories, 1);
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn an_empty_url_target_does_not_opt_into_page_check_retry() {
    let (mut cdp, session, browser) = client(u32::MAX, None, false);
    browser.borrow_mut().url = String::new();
    cdp.targets.insert("T".into(), page(""));
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, PAGE_CHANGED);
    assert_eq!(browser.borrow().inventories, 1);
    assert_eq!(browser.borrow().trees, 1);
}

#[test]
fn a_retry_off_call_matches_single_attempt_frame_tree_and_inventory_counts() {
    let setup = |retry_opt_in| {
        let (mut cdp, session, browser) = client(u32::MAX, None, false);
        browser.borrow_mut().url = String::new();
        cdp.targets.insert("T".into(), page(""));
        browser.borrow_mut().inventories = 0;
        browser.borrow_mut().trees = 0;
        let message = if retry_opt_in {
            cdp.call("Runtime.evaluate", json!({ "expression": "1", "returnByValue": true }), Some(&session))
        } else {
            cdp.call_for("Runtime.evaluate", json!({ "expression": "1", "returnByValue": true }), Some(&session), Duration::from_secs(30))
        }.unwrap_err();
        assert_eq!(message, PAGE_CHANGED);
        let counts = (browser.borrow().inventories, browser.borrow().trees);
        counts
    };
    let opted_out = setup(false);
    let blocked = setup(true);
    assert_eq!(blocked, opted_out);
    assert!(blocked.0 <= 2, "inventories={}", blocked.0);
    assert!(blocked.1 <= 2, "trees={}", blocked.1);
}

#[test]
fn a_protected_iframe_appearing_during_the_check_is_refused() {
    let (mut cdp, session, browser) = configured(0, None, false, false, false, None, None, vec![iframe("SEVEN", "https://seven.sliccy.ai/app")]);
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, format!("{}\n", cdp.guard.refusal()));
    assert!(!browser.borrow().forwarded.iter().any(|method| method == "DOM.describeNode"));
}

#[test]
fn an_unplaced_protected_iframe_observed_mid_check_still_refuses_on_retry() {
    let (mut cdp, session, browser) = client(0, None, false);
    {
        let mut browser = browser.borrow_mut();
        browser.inventories = 0;
        browser.trees = 0;
        browser.forwarded.clear();
        browser.mid_tree_iframe = Some(iframe("F", "https://seven.sliccy.ai/app"));
        browser.iframe_after_first_inventory = Some(iframe_parent("F", "MISSING", "https://seven.sliccy.ai/app"));
    }
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_ne!(message, PAGE_CHANGED);
    assert!(message.starts_with("slicc: refused:"), "{message}");
    assert!(browser.borrow().inventories >= 2, "inventories={}", browser.borrow().inventories);
    assert!(!browser.borrow().forwarded.iter().any(|method| method == "DOM.describeNode"));
}

#[test]
fn page_scope_prepare_retry_stays_off_for_holder_and_paused_paths() {
    let (cdp, session, _) = client(0, None, false);
    assert!(cdp.page_scope_prepare_retry_allowed("DOM.describeNode", Some(&session)));
    let mut cdp = cdp;
    cdp.webmcp.paused.insert(session.clone());
    assert!(!cdp.page_scope_prepare_retry_allowed("DOM.describeNode", Some(&session)));
    cdp.webmcp.paused.clear();
    cdp.paused_documents.insert((session.clone(), "REQ".into()));
    assert!(!cdp.page_scope_prepare_retry_allowed("DOM.describeNode", Some(&session)));
}

#[test]
fn page_scope_prepare_retry_ignores_leftover_records_but_not_error_urls() {
    let (mut cdp, session, _) = client(0, None, false);
    cdp.unchecked_documents.insert(
        ("T".into(), session.clone(), "C".into()),
        super::document_order::ObservedDocument {
            frame: json!({
                "id": "C",
                "parentId": "T",
                "loaderId": "LC",
                "url": "https://seven.sliccy.ai/secret",
                "securityOrigin": "https://seven.sliccy.ai",
            }),
            epoch: 0,
            root: Some(("T".into(), "L0".into())),
        },
    );
    assert!(cdp.page_scope_prepare_retry_allowed("DOM.describeNode", Some(&session)));
    cdp.targets.insert("T".into(), page("chrome-error://chromewebdata/"));
    assert!(!cdp.page_scope_prepare_retry_allowed("DOM.describeNode", Some(&session)));
}

#[test]
fn a_protected_child_observed_then_omitted_on_retry_is_refused() {
    let (mut cdp, session, browser) = client(0, None, false);
    browser.borrow_mut().tree_error_with_protected_child = true;
    browser.borrow_mut().inventories = 0;
    browser.borrow_mut().trees = 0;
    browser.borrow_mut().forwarded.clear();
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, format!("{}\n", cdp.guard.refusal()));
    assert_ne!(message, PAGE_CHANGED);
    assert!(browser.borrow().inventories >= 2, "inventories={}", browser.borrow().inventories);
    assert!(!browser.borrow().forwarded.iter().any(|method| method == "DOM.describeNode"));
}

#[test]
fn a_main_frame_navigation_record_is_matched_and_cleared_on_retry() {
    let (mut cdp, session, browser) = client(1, Some(("https://ordinary.test/done", "https://ordinary.test")), false);
    assert!(cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).is_ok());
    assert!(browser.borrow().inventories >= 2);
    assert!(browser.borrow().url.ends_with("/done"));
    assert!(!cdp.unchecked_documents.keys().any(|(owner, _, _)| owner == "T"));
}

#[test]
fn call_for_does_not_retry_a_changing_page() {
    let (mut cdp, session, browser) = client(u32::MAX, None, false);
    let start = Instant::now();
    let message = cdp.call_for("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session), Duration::from_secs(30)).unwrap_err();
    assert_eq!(message, PAGE_CHANGED);
    assert_eq!(browser.borrow().inventories, 1);
    assert!(start.elapsed() < Duration::from_secs(3));
}

#[test]
fn a_holder_pause_evaluate_runs_one_page_check_attempt() {
    let (mut cdp, session, browser) = client(u32::MAX, None, false);
    cdp.webmcp.paused.insert(session.clone());
    let start = Instant::now();
    let message = cdp.call_for(
        "DOM.describeNode",
        json!({ "nodeId": 1 }),
        Some(&session),
        Duration::from_secs(1),
    ).unwrap_err();
    assert_eq!(message, PAGE_CHANGED);
    assert_eq!(browser.borrow().inventories, 1);
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!browser.borrow().forwarded.iter().any(|method| method == "DOM.describeNode"));
}

#[test]
fn a_document_pause_that_begins_during_attempt_one_stops_further_attempts() {
    let (mut cdp, session, browser) = client(u32::MAX, None, false);
    browser.borrow_mut().mid_tree_pause_session = Some(session.clone());
    browser.borrow_mut().inventories = 0;
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert_eq!(message, PAGE_CHANGED);
    assert_eq!(browser.borrow().inventories, 1);
}

#[test]
fn a_sealed_retry_whose_refresh_never_answers_ends_within_the_overall_deadline() {
    let (mut cdp, session, browser) = configured(
        1,
        Some(("chrome-error://chromewebdata/", "://", "")),
        false,
        false,
        false,
        None,
        None,
        vec![],
    );
    browser.borrow_mut().hang_tree_after = Some(1);
    let start = Instant::now();
    let message = cdp.call("DOM.describeNode", json!({ "nodeId": 1 }), Some(&session)).unwrap_err();
    assert!(message.starts_with("slicc: refused:"), "{message}");
    assert_ne!(message, PAGE_CHANGED);
    assert!(start.elapsed() < Duration::from_secs(7), "{:?}", start.elapsed());
    assert!(!browser.borrow().forwarded.iter().any(|method| method == "DOM.describeNode"));
}

#[test]
fn a_caller_timeout_shorter_than_the_prepare_loop_bounds_the_check() {
    let (mut cdp, session, browser) = client(u32::MAX, None, false);
    browser.borrow_mut().inventory_delay = Some(Duration::from_millis(60));
    let start = Instant::now();
    let message = cdp.call_guarded_for(
        "DOM.describeNode",
        json!({ "nodeId": 1 }),
        Some(&session),
        Duration::from_millis(100),
        false,
        true,
    ).unwrap_err();
    assert!(message.starts_with("slicc: refused:") || message.contains("timed out"), "{message}");
    assert!(start.elapsed() < Duration::from_millis(800), "{:?}", start.elapsed());
    assert!(browser.borrow().inventories < 3, "inventories={}", browser.borrow().inventories);
}

#[test]
fn a_zero_deadline_slice_refuses_without_spending_the_retry_budget() {
    let (mut cdp, session, browser) = client(u32::MAX, None, false);
    let start = Instant::now();
    let message = cdp.call_guarded_for(
        "DOM.describeNode",
        json!({ "nodeId": 1 }),
        Some(&session),
        Duration::ZERO,
        false,
        true,
    ).unwrap_err();
    assert!(message.starts_with("slicc: refused:"), "{message}");
    assert_eq!(browser.borrow().inventories, 0);
    assert!(start.elapsed() < Duration::from_millis(500), "{:?}", start.elapsed());
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
