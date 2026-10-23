struct AttachBrowser {
    url: String,
    main: Value,
    during_read: Option<Value>,
    attaching: bool,
}

struct AttachTree {
    inbox: VecDeque<String>,
    browser: std::rc::Rc<std::cell::RefCell<AttachBrowser>>,
    guard: bool,
}

impl Transport for AttachTree {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut browser = self.browser.borrow_mut();
        let result = match call["method"].as_str().unwrap() {
            "Target.attachToTarget" => {
                let session = if self.guard { "G" } else { "U" };
                self.guard = true;
                json!({ "sessionId": session })
            }
            "Target.getTargetInfo" => json!({ "targetInfo": { "targetId": "T", "type": "page", "url": browser.url, "title": "" } }),
            "Page.getFrameTree" => {
                let event = if browser.attaching { browser.during_read.clone() } else { browser.during_read.take() };
                if let Some(event) = event {
                    self.inbox.push_back(event.to_string());
                }
                json!({ "frameTree": { "frame": browser.main } })
            }
            "Runtime.evaluate" => json!({ "result": { "type": "string", "value": "complete" } }),
            _ => json!({}),
        };
        self.inbox.push_back(json!({ "id": call["id"], "sessionId": call["sessionId"], "result": result }).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

const ATTACH_LANDING: &str = "http://127.0.0.1:8000/landing?n=window";
const ATTACH_SEVEN: &str = "http://seven.sliccy.ai/dump?n=main";

fn attach_during(during_read: Option<Value>) -> (Cdp, String, std::rc::Rc<std::cell::RefCell<AttachBrowser>>) {
    let browser = std::rc::Rc::new(std::cell::RefCell::new(AttachBrowser {
        url: ATTACH_LANDING.into(),
        main: json!({ "id": "T", "url": ATTACH_LANDING, "securityOrigin": "http://127.0.0.1:8000" }),
        during_read,
        attaching: true,
    }));
    let mut cdp = Cdp::from_transport(Box::new(AttachTree { inbox: VecDeque::new(), browser: browser.clone(), guard: false }));
    let session = super::attach(&mut cdp, "T").unwrap();
    browser.borrow_mut().attaching = false;
    browser.borrow_mut().during_read = None;
    assert_eq!(cdp.guards["T"], "G");
    (cdp, session, browser)
}

fn blocked_seven_navigation(cdp: &mut Cdp, browser: &std::cell::RefCell<AttachBrowser>) {
    let error = json!({ "id": "T", "url": "chrome-error://chromewebdata/", "securityOrigin": "://", "unreachableUrl": ATTACH_SEVEN });
    cdp.absorb(json!({ "method": "Page.frameStartedNavigating", "sessionId": "G", "params": { "frameId": "T", "url": ATTACH_SEVEN, "navigationType": "differentDocument" } }));
    cdp.absorb(json!({ "method": "Page.frameNavigated", "sessionId": "G", "params": { "frame": error, "type": "Navigation" } }));
    cdp.absorb(json!({ "method": "Target.targetInfoChanged", "params": { "targetInfo": { "targetId": "T", "type": "page", "url": ATTACH_SEVEN, "title": "seven.sliccy.ai" } } }));
    let mut browser = browser.borrow_mut();
    browser.url = ATTACH_SEVEN.into();
    browser.main = error;
}

fn ready(cdp: &mut Cdp, session: &str) -> Result<Value, String> {
    cdp.call("Runtime.evaluate", json!({ "expression": "document.readyState", "returnByValue": true }), Some(session))
}

#[test]
fn a_subframe_attached_while_the_guard_reads_the_tree_keeps_the_main_document() {
    for during_read in [None, Some(json!({ "method": "Page.frameAttached", "sessionId": "G", "params": { "frameId": "CHILD", "parentFrameId": "T" } }))] {
        let (mut cdp, session, browser) = attach_during(during_read.clone());
        assert_eq!(cdp.document_evidence("T"), format!("url={ATTACH_LANDING} unreachable= navigating=false previous=false probed=false"), "{during_read:?}");
        blocked_seven_navigation(&mut cdp, &browser);
        assert_eq!(ready(&mut cdp, &session).unwrap()["result"]["value"], "complete", "{during_read:?}");
        assert!(!cdp.session_blocked(&session), "{during_read:?}");
        assert!(!cdp.target_protected("T", false).unwrap(), "{during_read:?}");
        assert!(cdp.call("Runtime.evaluate", json!({ "expression": "document.cookie" }), Some(&session)).is_err());
        assert!(cdp.call("Page.navigate", json!({ "url": ATTACH_SEVEN }), Some(&session)).is_err());
        assert!(!cdp.session_blocked(&session));
    }
}

#[test]
fn a_main_frame_update_during_the_guard_tree_read_still_fails_closed() {
    let update = json!({ "method": "Page.navigatedWithinDocument", "sessionId": "G", "params": { "frameId": "T", "url": format!("{ATTACH_LANDING}#moved") } });
    let (mut cdp, session, browser) = attach_during(Some(update));
    assert_eq!(cdp.document_evidence("T"), "none");
    blocked_seven_navigation(&mut cdp, &browser);
    assert!(cdp.session_blocked(&session));
    assert!(cdp.target_protected("T", false).unwrap());
    assert!(ready(&mut cdp, &session).is_err());
}

#[test]
fn a_callers_tree_read_racing_a_subframe_attach_is_still_refused() {
    let (mut cdp, session, browser) = attach_during(None);
    browser.borrow_mut().during_read = Some(json!({ "method": "Page.frameAttached", "sessionId": "G", "params": { "frameId": "LATE", "parentFrameId": "T" } }));
    assert!(cdp.call("Page.getFrameTree", json!({}), Some(&session)).unwrap_err().starts_with("slicc: refused: "));
    assert!(cdp.call("Page.getFrameTree", json!({}), Some(&session)).is_ok());
    assert!(!cdp.session_blocked(&session));
}
