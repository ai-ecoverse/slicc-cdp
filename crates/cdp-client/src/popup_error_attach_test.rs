const POPUP_SEVEN: &str = "https://seven.sliccy.ai/doc?n=popup-SECRET";
const POPUP_ERROR: &str = "chrome-error://chromewebdata/";
const POPUP_ORDINARY: &str = "http://ordinary.invalid/doc?n=popup";

#[derive(Clone)]
enum PopupFrame {
    Frame(Value),
    Tree(Value),
    Error,
    Silent,
}

#[derive(Clone, Copy, PartialEq)]
enum PopupOrder {
    EventFirst,
    ReplyFirst,
    DetachFirst,
}

struct PopupState {
    popup: Value,
    frame: PopupFrame,
    order: PopupOrder,
    sessions: std::collections::BTreeMap<String, String>,
    attached: usize,
}

thread_local! {
    static POPUP_BROWSER: std::cell::RefCell<Option<std::rc::Rc<std::cell::RefCell<PopupState>>>> = const { std::cell::RefCell::new(None) };
}

fn popup_client_attaches() -> usize {
    POPUP_BROWSER.with(|state| state.borrow().as_ref().map_or(0, |state| state.borrow().attached))
}

struct PopupBrowser {
    inbox: VecDeque<String>,
    state: std::rc::Rc<std::cell::RefCell<PopupState>>,
}

fn popup_opener() -> Value {
    json!({"targetId":"O","type":"page","url":"https://example.test/opener","title":"opener","attached":true})
}

fn popup_info(url: &str, title: &str) -> Value {
    json!({"targetId":"P","type":"page","url":url,"title":title,"openerId":"O","openerFrameId":"O","attached":false,"canAccessOpener":false})
}

fn popup_error_frame(unreachable: &str) -> Value {
    let mut frame = json!({"id":"P","loaderId":"LP","url":POPUP_ERROR,"securityOrigin":"://","mimeType":"text/html"});
    if !unreachable.is_empty() {
        frame["unreachableUrl"] = json!(unreachable);
    }
    frame
}

impl Transport for PopupBrowser {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut state = self.state.borrow_mut();
        let target = call["params"]["targetId"].as_str().map(str::to_string)
            .or_else(|| call["sessionId"].as_str().and_then(|session| state.sessions.get(session).cloned()))
            .unwrap_or_default();
        let info = if target == "O" { popup_opener() } else { state.popup.clone() };
        let reply = |result: Value| json!({"id":call["id"],"sessionId":call["sessionId"],"result":result}).to_string();
        let result = match call["method"].as_str().unwrap() {
            "Target.getTargetInfo" => json!({"targetInfo":info}),
            "Target.getTargets" => json!({"targetInfos":[popup_opener(), state.popup.clone()]}),
            "Target.attachToTarget" => {
                state.attached += 1;
                let session = format!("A{}", state.attached);
                state.sessions.insert(session.clone(), target.clone());
                let attached = json!({"method":"Target.attachedToTarget","params":{"sessionId":session,"targetInfo":info,"waitingForDebugger":false}}).to_string();
                let detached = json!({"method":"Target.detachedFromTarget","params":{"sessionId":session,"targetId":target}}).to_string();
                let client = state.attached == 1;
                match state.order {
                    PopupOrder::ReplyFirst if client => {
                        self.inbox.push_back(reply(json!({"sessionId":session})));
                        self.inbox.push_back(attached);
                        return Ok(());
                    }
                    PopupOrder::DetachFirst if client => {
                        self.inbox.push_back(attached);
                        self.inbox.push_back(detached);
                    }
                    _ => self.inbox.push_back(attached),
                }
                json!({"sessionId":session})
            }
            "Target.detachFromTarget" => {
                if let Some(session) = call["params"]["sessionId"].as_str() {
                    if let Some(owner) = state.sessions.remove(session) {
                        self.inbox.push_back(reply(json!({})));
                        self.inbox.push_back(json!({"method":"Target.detachedFromTarget","params":{"sessionId":session,"targetId":owner}}).to_string());
                        return Ok(());
                    }
                }
                json!({})
            }
            "Page.getFrameTree" if target != "O" => match state.frame.clone() {
                PopupFrame::Frame(frame) => json!({"frameTree":{"frame":frame}}),
                PopupFrame::Tree(tree) => tree,
                PopupFrame::Error => {
                    self.inbox.push_back(json!({"id":call["id"],"sessionId":call["sessionId"],"error":{"code":-32000,"message":"Inspected target navigated or closed"}}).to_string());
                    return Ok(());
                }
                PopupFrame::Silent => return Ok(()),
            },
            "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"O","loaderId":"LO","url":"https://example.test/opener","securityOrigin":"https://example.test","mimeType":"text/html"}}}),
            "Page.getNavigationHistory" => {
                let unreachable = match &state.frame {
                    PopupFrame::Frame(frame) => frame.get("unreachableUrl").and_then(Value::as_str).unwrap_or("").to_string(),
                    _ => String::new(),
                };
                let url = if !unreachable.is_empty() {
                    unreachable
                } else {
                    state.popup["url"].as_str().unwrap_or("about:blank").to_string()
                };
                let title = state.popup["title"].as_str().unwrap_or(url.as_str()).to_string();
                json!({"currentIndex":0,"entries":[{"id":1,"url":url,"userTypedURL":url,"title":title,"transitionType":"link"}]})
            }
            "Page.getResourceContent" => json!({"content":format!("body {}", state.popup),"base64Encoded":false}),
            "Page.captureScreenshot" => json!({"data":"iVBORw0KGgo="}),
            "Page.printToPDF" => json!({"data":"JVBERi0="}),
            "Network.getResponseBody" => json!({"body":format!("net {}", state.popup),"base64Encoded":false}),
            "DOM.getDocument" => json!({"root":{"nodeId":1,"nodeName":"HTML","documentURL":state.popup["url"].clone()}}),
            "Runtime.callFunctionOn" => json!({"result":{"type":"string","value":"document-read"}}),
            "Runtime.evaluate" => json!({"result":{"type":"string","value":"document-read"}}),
            _ => json!({}),
        };
        self.inbox.push_back(reply(result));
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn popup_client_in(popup: Value, frame: PopupFrame, order: PopupOrder) -> Cdp {
    let state = std::rc::Rc::new(std::cell::RefCell::new(PopupState {
        popup, frame, order, sessions: std::collections::BTreeMap::new(), attached: 0,
    }));
    POPUP_BROWSER.with(|shared| *shared.borrow_mut() = Some(state.clone()));
    let mut cdp = Cdp::from_transport(Box::new(PopupBrowser { inbox: VecDeque::new(), state }));
    cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":popup_opener()}}));
    cdp
}

fn popup_client(popup: Value, frame: PopupFrame) -> Cdp {
    popup_client_in(popup, frame, PopupOrder::EventFirst)
}

fn popup_session_events(cdp: &Cdp, session: &str) -> Vec<String> {
    cdp.events().iter()
        .filter(|event| event["params"]["sessionId"] == session && event["method"].as_str().is_some_and(|method| method.starts_with("Target.")))
        .map(|event| event["method"].as_str().unwrap().to_string())
        .collect()
}

fn popup_target_infos(cdp: &Cdp) -> Vec<Value> {
    cdp.events().iter()
        .filter(|event| event["params"]["targetInfo"]["targetId"] == "P")
        .map(|event| event["params"]["targetInfo"].clone())
        .collect()
}

fn popup_attach_refused(cdp: &mut Cdp, label: &str) {
    let before = popup_client_attaches();
    let reply = cdp.call("Target.attachToTarget", json!({"targetId":"P","flatten":true}), None);
    let forwarded = popup_client_attaches() > before;
    assert!(reply.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")), "{label}: {reply:?}");
    let _ = cdp.call("Target.getTargets", json!({}), None);
    assert!(popup_session_events(cdp, "A1").is_empty(), "{label}: {:?}", popup_session_events(cdp, "A1"));
    for (method, params) in [
        ("Runtime.evaluate", json!({"expression":"location.href"})),
        ("Page.navigate", json!({"url":"https://example.test/next"})),
        ("Page.getNavigationHistory", json!({})),
        ("Page.getFrameTree", json!({})),
    ] {
        if forwarded {
            let out = cdp.call(method, params, Some("A1"));
            assert!(out.is_err(), "{label}: {method} {out:?}");
        }
    }
    for info in popup_target_infos(cdp) {
        assert!(!info.to_string().contains("SECRET"), "{label}: {info}");
        if info["url"].as_str().is_some_and(|url| !url.starts_with("https://seven.sliccy.ai")) {
            assert!(!info.to_string().contains("sliccy"), "{label}: {info}");
        }
    }
}

#[test]
fn an_error_page_popup_with_a_protected_failed_url_is_refused_and_never_announced() {
    for title in [POPUP_ERROR, "seven.sliccy.ai"] {
        for order in [PopupOrder::EventFirst, PopupOrder::ReplyFirst, PopupOrder::DetachFirst] {
            let mut cdp = popup_client_in(popup_info(POPUP_ERROR, title), PopupFrame::Frame(popup_error_frame(POPUP_SEVEN)), order);
            popup_attach_refused(&mut cdp, &format!("title={title}"));
            assert!(cdp.is_protected_target("P"));
        }
    }
}

#[test]
fn an_error_page_popup_without_proof_of_its_failed_url_is_refused() {
    for (name, frame) in [
        ("error", PopupFrame::Error),
        ("empty-unreachable", PopupFrame::Frame(popup_error_frame(""))),
        ("unparseable-unreachable", PopupFrame::Frame(popup_error_frame("not a url"))),
        ("no-frame", PopupFrame::Tree(json!({"frameTree":{}}))),
        ("malformed", PopupFrame::Tree(json!({"frameTree":{"frame":"chrome-error://chromewebdata/"}}))),
        ("no-id", PopupFrame::Tree(json!({"frameTree":{"frame":{"url":POPUP_ERROR,"securityOrigin":"://","unreachableUrl":POPUP_ORDINARY}}}))),
    ] {
        for order in [PopupOrder::EventFirst, PopupOrder::ReplyFirst, PopupOrder::DetachFirst] {
            let mut cdp = popup_client_in(popup_info(POPUP_ERROR, POPUP_ERROR), frame.clone(), order);
            popup_attach_refused(&mut cdp, name);
        }
    }
}

#[test]
fn an_error_page_popup_whose_frame_read_times_out_is_refused() {
    let mut cdp = popup_client(popup_info(POPUP_ERROR, "seven.sliccy.ai"), PopupFrame::Silent);
    popup_attach_refused(&mut cdp, "timeout");
}

#[test]
fn an_error_page_popup_latched_on_its_protected_url_stays_refused() {
    for title in [POPUP_ERROR, "seven.sliccy.ai"] {
        for documented in [false, true] {
            let mut cdp = popup_client(popup_info(POPUP_ERROR, title), PopupFrame::Frame(popup_error_frame(POPUP_SEVEN)));
            cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":popup_info("about:blank","about:blank")}}));
            cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":popup_info(POPUP_SEVEN, POPUP_SEVEN)}}));
            assert!(cdp.is_protected_target("P"));
            if documented {
                cdp.session_targets.insert("W".into(), "P".into());
                cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"W","params":{"frame":popup_error_frame(POPUP_SEVEN),"type":"Navigation"}}));
                cdp.session_targets.remove("W");
            }
            cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":popup_info(POPUP_ERROR, title)}}));
            assert!(cdp.is_protected_target("P"), "latched title={title} documented={documented}");
            popup_attach_refused(&mut cdp, &format!("latched title={title} documented={documented}"));
        }
    }
}

#[test]
fn an_error_page_popup_with_an_ordinary_failed_url_attaches_in_order_with_its_title() {
    for order in [PopupOrder::EventFirst, PopupOrder::ReplyFirst, PopupOrder::DetachFirst] {
        let mut cdp = popup_client_in(popup_info(POPUP_ERROR, "ordinary.invalid"), PopupFrame::Frame(popup_error_frame(POPUP_ORDINARY)), order);
        let reply = cdp.call("Target.attachToTarget", json!({"targetId":"P","flatten":true}), None).unwrap();
        assert_eq!(reply["sessionId"], "A1");
        let _ = cdp.call("Target.getTargets", json!({}), None);
        let expected: Vec<String> = if order == PopupOrder::DetachFirst {
            vec!["Target.attachedToTarget".into(), "Target.detachedFromTarget".into()]
        } else {
            vec!["Target.attachedToTarget".into()]
        };
        assert_eq!(popup_session_events(&cdp, "A1"), expected);
        let attached = cdp.events().iter().find(|event| event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == "A1").unwrap();
        assert_eq!(attached["params"]["targetInfo"]["url"], POPUP_ERROR);
        assert_eq!(attached["params"]["targetInfo"]["title"], "ordinary.invalid");
        assert_eq!(attached["params"]["targetInfo"]["openerId"], "O");
        if order != PopupOrder::DetachFirst {
            assert!(!cdp.session_blocked("A1"));
            assert!(cdp.call("Page.navigate", json!({"url":"https://example.test/next"}), Some("A1")).is_ok());
        }
    }
}

#[test]
fn ordinary_error_page_discovery_keeps_its_url_and_title_once_proven() {
    let mut cdp = popup_client(popup_info(POPUP_ERROR, "ordinary.invalid"), PopupFrame::Frame(popup_error_frame(POPUP_ORDINARY)));
    cdp.call("Target.attachToTarget", json!({"targetId":"P","flatten":true}), None).unwrap();
    let mark = cdp.events().len();
    for method in ["Target.targetCreated", "Target.targetInfoChanged"] {
        cdp.absorb(json!({"method":method,"params":{"targetInfo":popup_info(POPUP_ERROR, "ordinary.invalid")}}));
    }
    for event in &cdp.events()[mark..] {
        assert_eq!(event["params"]["targetInfo"]["url"], POPUP_ERROR, "{event}");
        assert_eq!(event["params"]["targetInfo"]["title"], "ordinary.invalid", "{event}");
    }
    let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
    let popup = listed["targetInfos"].as_array().unwrap().iter().find(|info| info["targetId"] == "P").unwrap().clone();
    assert_eq!(popup["title"], "ordinary.invalid");
    let found = cdp.call("Target.getTargetInfo", json!({"targetId":"P"}), None).unwrap();
    assert_eq!(found["targetInfo"]["title"], "ordinary.invalid");
}

#[test]
fn error_page_discovery_with_a_protected_or_unproven_failed_url_shows_the_placeholder() {
    for documented in [false, true] {
        for title in [POPUP_ERROR, "seven.sliccy.ai", POPUP_SEVEN] {
            let label = format!("documented={documented} title={title}");
            let mut cdp = popup_client(popup_info(POPUP_ERROR, title), PopupFrame::Frame(popup_error_frame(POPUP_SEVEN)));
            let mark = cdp.events().len();
            cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":popup_info("about:blank","about:blank")}}));
            cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":popup_info(POPUP_SEVEN, POPUP_SEVEN)}}));
            if documented {
                cdp.session_targets.insert("W".into(), "P".into());
                cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"W","params":{"frame":popup_error_frame(POPUP_SEVEN),"type":"Navigation"}}));
                cdp.session_targets.remove("W");
            }
            let changed = cdp.events().len();
            for method in ["Target.targetInfoChanged", "Target.targetCreated", "Target.attachedToTarget"] {
                cdp.absorb(json!({"method":method,"params":{"sessionId":"X","targetInfo":popup_info(POPUP_ERROR, title)}}));
            }
            for event in &cdp.events()[mark..] {
                assert!(!event.to_string().contains("SECRET"), "{label}: {event}");
            }
            let shown: Vec<&Value> = cdp.events()[changed..].iter().filter(|event| event["params"]["targetInfo"]["targetId"] == "P").collect();
            assert!(!shown.is_empty(), "{label}");
            for event in shown {
                assert_eq!(event["params"]["targetInfo"]["url"], "about:blank", "{label}: {event}");
                assert_eq!(event["params"]["targetInfo"]["title"], "about:blank", "{label}: {event}");
            }
            let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
            for info in listed["targetInfos"].as_array().unwrap().iter().filter(|info| info["targetId"] == "P") {
                assert!(!info.to_string().contains("sliccy"), "{label}: {info}");
            }
            if let Ok(found) = cdp.call("Target.getTargetInfo", json!({"targetId":"P"}), None) {
                assert!(!found.to_string().contains("sliccy"), "{label}: {found}");
            }
        }
    }
}

#[test]
fn a_created_target_aimed_at_a_protected_url_is_refused_before_any_attach() {
    let mut cdp = popup_client(popup_info(POPUP_ERROR, POPUP_ERROR), PopupFrame::Frame(popup_error_frame(POPUP_SEVEN)));
    assert!(cdp.call("Target.createTarget", json!({"url":POPUP_SEVEN}), None).is_err());
    assert!(cdp.events().iter().all(|event| event["method"] != "Target.attachedToTarget"));
}

fn own_info(url: &str, title: &str) -> Value {
    json!({"targetId":"T","type":"page","url":url,"title":title,"attached":false,"canAccessOpener":false})
}

fn own_state(info: Value, frame: PopupFrame) -> std::rc::Rc<std::cell::RefCell<PopupState>> {
    let state = std::rc::Rc::new(std::cell::RefCell::new(PopupState {
        popup: info, frame, order: PopupOrder::EventFirst, sessions: std::collections::BTreeMap::new(), attached: 0,
    }));
    POPUP_BROWSER.with(|shared| *shared.borrow_mut() = Some(state.clone()));
    state
}

fn own_client_from(state: std::rc::Rc<std::cell::RefCell<PopupState>>, info: &Value) -> Cdp {
    let mut cdp = Cdp::from_transport(Box::new(PopupBrowser { inbox: VecDeque::new(), state }));
    cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":info}}));
    cdp
}

fn assert_recovered(cdp: &mut Cdp, session: &str, label: &str) {
    let announced = cdp.events().iter().rev().find(|event| event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == session).unwrap();
    assert_eq!(announced["params"]["targetInfo"]["url"], "about:blank", "{label}: {announced}");
    assert_eq!(announced["params"]["targetInfo"]["title"], "about:blank", "{label}: {announced}");
    assert!(!announced.to_string().contains("sliccy"), "{label}: {announced}");
    assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.body.innerHTML"}), Some(session)).is_err(), "{label}");
    assert!(cdp.call("Page.navigate", json!({"url":"https://example.test/recovered"}), Some(session)).is_ok(), "{label}");
}

#[test]
fn an_own_tab_error_page_with_a_protected_failed_url_stays_attachable() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    assert!(cdp.call("Page.navigate", json!({"url":"https://example.test/next"}), Some(&live)).is_ok());
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "seven.sliccy.ai");
        state.frame = PopupFrame::Frame(popup_error_frame(POPUP_SEVEN));
    }
    let guard = cdp.guards.get("T").cloned().expect("guard");
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "seven.sliccy.ai")}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":popup_error_frame(POPUP_SEVEN),"type":"Navigation"}}));
    assert!(!cdp.is_protected_target("T"));
    assert!(cdp.call("Runtime.evaluate", json!({"expression":"1"}), Some(&live)).is_err());
    let _ = cdp.call("Target.detachFromTarget", json!({"sessionId":live}), None);
    let reply = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    assert_recovered(&mut cdp, reply["sessionId"].as_str().unwrap(), "protected-unreachable");
}

#[test]
fn an_own_tab_error_page_with_an_empty_failed_url_stays_attachable_across_processes() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut first = own_client_from(state.clone(), &start);
    assert!(first.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).is_ok());
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "seven.sliccy.ai");
        state.frame = PopupFrame::Frame(popup_error_frame(""));
        state.sessions.clear();
    }
    let error = own_info(POPUP_ERROR, "seven.sliccy.ai");
    let mut fresh = own_client_from(state, &error);
    let reply = fresh.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    assert_recovered(&mut fresh, reply["sessionId"].as_str().unwrap(), "empty-unreachable");
}

#[test]
fn a_fresh_attach_after_a_post_attach_chrome_error_target_info_recovers_an_own_tab() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut live = own_client_from(state.clone(), &start);
    let first = live.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let session = first["sessionId"].as_str().unwrap().to_string();
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "seven.sliccy.ai");
        state.frame = PopupFrame::Frame(popup_error_frame(""));
    }
    let guard = live.guards.get("T").cloned().expect("guard");
    live.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "seven.sliccy.ai")}}));
    live.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":popup_error_frame(""),"type":"Navigation"}}));
    assert!(live.call("Runtime.evaluate", json!({"expression":"1"}), Some(&session)).is_err());
    {
        let mut state = state.borrow_mut();
        state.sessions.clear();
    }
    let error = own_info(POPUP_ERROR, "seven.sliccy.ai");
    let mut fresh = own_client_from(state, &error);
    let reply = fresh.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    assert_recovered(&mut fresh, reply["sessionId"].as_str().unwrap(), "post-attach-chrome-error");
}

#[test]
fn an_error_page_iframe_is_not_held_by_the_popup_error_attach_gate() {
    let iframe = json!({"targetId":"IF","type":"iframe","url":"chrome-error://private/","title":"private","attached":false});
    let popup = popup_info("chrome-error://private/", "private");
    let cdp = Cdp::from_transport(Box::new(PopupBrowser {
        inbox: VecDeque::new(),
        state: own_state(iframe.clone(), PopupFrame::Frame(json!({"id":"IF","loaderId":"LIF","url":"chrome-error://private/","securityOrigin":"://","mimeType":"text/html"}))),
    }));
    assert!(!cdp.error_popup_hold(&iframe));
    assert!(cdp.error_unproven(&iframe));
    assert!(cdp.error_popup_hold(&popup));
}

#[test]
fn a_blank_popup_attach_does_not_grant_provenance_for_a_later_protected_error() {
    let blank = popup_info("about:blank", "about:blank");
    let state = own_state(blank.clone(), PopupFrame::Frame(json!({
        "id":"P","loaderId":"LP","url":"about:blank","securityOrigin":"://","mimeType":"text/html"
    })));
    let mut cdp = Cdp::from_transport(Box::new(PopupBrowser { inbox: VecDeque::new(), state: state.clone() }));
    cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":popup_opener()}}));
    cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":blank}}));
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"P","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    assert!(!cdp.client_targets.contains("P"));
    assert!(cdp.call("Page.navigate", json!({"url":"https://example.test/next"}), Some(&live)).is_ok());
    assert!(!cdp.client_targets.contains("P"), "popup-shaped navigate must not grant provenance");
    {
        let mut state = state.borrow_mut();
        state.popup = popup_info(POPUP_ERROR, "seven.sliccy.ai");
        state.frame = PopupFrame::Frame(popup_error_frame(POPUP_SEVEN));
    }
    let guard = cdp.guards.get("P").cloned().expect("guard");
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":popup_info(POPUP_ERROR, "seven.sliccy.ai")}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":popup_error_frame(POPUP_SEVEN),"type":"Navigation"}}));
    assert!(!cdp.is_protected_target("P"));
    let _ = cdp.call("Target.detachFromTarget", json!({"sessionId":live}), None);
    while cdp.pump(Instant::now() + Duration::from_millis(20)).unwrap_or(false) {}
    let mark = cdp.events().len();
    let reply = cdp.call("Target.attachToTarget", json!({"targetId":"P","flatten":true}), None);
    assert!(reply.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")), "blank-popup-no-provenance: {reply:?}");
    for event in &cdp.events()[mark..] {
        if matches!(event["method"].as_str(), Some("Target.attachedToTarget") | Some("Target.detachedFromTarget")) {
            let session = event["params"]["sessionId"].as_str().unwrap_or("");
            assert!(cdp.guard_sessions.contains(session), "blank-popup-no-provenance client-visible session event: {event}");
        }
    }
}

#[test]
fn a_noopener_or_user_tab_error_page_is_attachable_like_an_own_tab_with_origin_only_surfaces() {
    const MARKER: &str = "NOOPENER_SECRET_MARKER";
    let failed = format!("https://seven.sliccy.ai/path/{MARKER}?q={MARKER}#{MARKER}");
    let origin = "https://seven.sliccy.ai";
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let session = first["sessionId"].as_str().unwrap().to_string();
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, &failed);
        state.frame = PopupFrame::Frame(popup_error_frame(&failed));
    }
    let guard = cdp.guards.get("T").cloned().expect("guard");
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &failed)}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":popup_error_frame(&failed),"type":"Navigation"}}));
    assert!(!cdp.is_protected_target("T"));
    let assert_origin_only = |name: &str, value: &Value| {
        let text = value.to_string();
        assert!(!text.contains(MARKER), "{name}: {text}");
        for key in ["url", "userTypedURL", "title", "unreachableUrl", "documentURL"] {
            if let Some(seen) = value.pointer(&format!("/{key}")).or_else(|| value.pointer(&format!("/frameTree/frame/{key}"))).or_else(|| value.pointer(&format!("/targetInfo/{key}"))).and_then(Value::as_str) {
                if seen.contains("seven.sliccy.ai") {
                    assert_eq!(seen, origin, "{name}.{key}");
                }
            }
        }
    };
    assert_origin_only("Page.getFrameTree", &cdp.call("Page.getFrameTree", json!({}), Some(&session)).unwrap());
    let history = cdp.call("Page.getNavigationHistory", json!({}), Some(&session)).unwrap();
    assert_origin_only("Page.getNavigationHistory", &history);
    for entry in history["entries"].as_array().unwrap() {
        assert_origin_only("history-entry", entry);
    }
    for (method, params) in [
        ("Runtime.evaluate", json!({"expression":"document.documentElement.innerHTML"})),
        ("Runtime.callFunctionOn", json!({"functionDeclaration":"() => document.body.innerText","returnByValue":true})),
        ("DOM.getDocument", json!({})),
        ("Page.getResourceContent", json!({"frameId":"T","url":failed})),
        ("Page.captureSnapshot", json!({})),
        ("Page.printToPDF", json!({})),
        ("Page.captureScreenshot", json!({})),
        ("Network.getResponseBody", json!({"requestId":"R1"})),
    ] {
        assert!(cdp.call(method, params, Some(&session)).is_err(), "{method}");
    }
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":session,"params":{"frame":popup_error_frame(&failed),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Page.lifecycleEvent","sessionId":session,"params":{"name":"load","frameId":"T","loaderId":"LP","timestamp":1.0}}));
    cdp.absorb(json!({"method":"Network.responseReceived","sessionId":session,"params":{"requestId":"R1","type":"Document","response":{"url":failed,"status":200,"headers":{}}}}));
    for event in &cdp.events()[mark..] {
        assert_origin_only(&format!("event:{}", event["method"]), event);
        assert!(!event.to_string().contains(MARKER), "{event}");
    }
    assert_origin_only("Target.getTargets", &cdp.call("Target.getTargets", json!({}), None).unwrap());
    assert_origin_only("Target.getTargetInfo", &cdp.call("Target.getTargetInfo", json!({"targetId":"T"}), None).unwrap());
    let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
    let tab = listed["targetInfos"].as_array().unwrap().iter().find(|info| info["targetId"] == "T").unwrap();
    assert_eq!(tab["url"], "about:blank");
    assert_eq!(tab["title"], "about:blank");
    assert!(cdp.call("Page.navigate", json!({"url":"https://example.test/away"}), Some(&session)).is_ok());
    {
        let mut state = state.borrow_mut();
        state.popup = own_info("https://example.test/away", "away");
        state.frame = PopupFrame::Frame(json!({"id":"T","loaderId":"L1","url":"https://example.test/away","securityOrigin":"https://example.test","mimeType":"text/html"}));
    }
    let after = cdp.call("Page.getNavigationHistory", json!({}), Some(&session)).unwrap();
    assert!(!after.to_string().contains(MARKER), "{after}");
    assert!(!after.to_string().contains("sliccy"), "{after}");
}

#[test]
fn guard_session_attach_events_in_the_settle_window_are_hidden_from_the_client() {
    let mut cdp = popup_client_in(popup_info(POPUP_ERROR, "ordinary.invalid"), PopupFrame::Frame(popup_error_frame(POPUP_ORDINARY)), PopupOrder::EventFirst);
    let before = cdp.events().len();
    let reply = cdp.call("Target.attachToTarget", json!({"targetId":"P","flatten":true}), None).unwrap();
    assert_eq!(reply["sessionId"], "A1");
    assert!(cdp.guard_sessions.len() >= 1, "touch must create a guard session");
    for event in &cdp.events()[before..] {
        let session = event["params"]["sessionId"].as_str().unwrap_or("");
        if matches!(event["method"].as_str(), Some("Target.attachedToTarget") | Some("Target.detachedFromTarget")) {
            assert!(!cdp.guard_sessions.contains(session), "settle must drop guard session event {event}");
        }
    }
    assert!(cdp.events().iter().any(|event| event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == "A1"));
}

#[test]
fn error_page_scrub_on_deliver_hides_the_failed_url() {
    let mut cdp = popup_client(popup_info(POPUP_ERROR, "seven.sliccy.ai"), PopupFrame::Frame(popup_error_frame(POPUP_SEVEN)));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Inspector.targetCrashed","params":{"targetInfo":popup_info(POPUP_ERROR, "seven.sliccy.ai")}}));
    let shown = cdp.events()[mark..].iter().find(|event| event["method"] == "Inspector.targetCrashed").expect("delivered");
    assert_eq!(shown["params"]["targetInfo"]["url"], "about:blank");
    assert!(!shown.to_string().contains("SECRET"));
}

#[test]
fn error_page_scrub_on_created_and_changed_hides_the_failed_url() {
    let mut cdp = popup_client(popup_info(POPUP_ERROR, "seven.sliccy.ai"), PopupFrame::Frame(popup_error_frame(POPUP_SEVEN)));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":popup_info(POPUP_ERROR, "seven.sliccy.ai")}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":popup_info(POPUP_ERROR, POPUP_SEVEN)}}));
    for event in &cdp.events()[mark..] {
        assert_eq!(event["params"]["targetInfo"]["url"], "about:blank", "{event}");
        assert!(!event.to_string().contains("SECRET"), "{event}");
    }
}

#[test]
fn error_page_scrub_on_attached_to_target_hides_the_failed_url() {
    let mut cdp = popup_client(popup_info(POPUP_ERROR, "seven.sliccy.ai"), PopupFrame::Frame(popup_error_frame(POPUP_SEVEN)));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.attachedToTarget","params":{"sessionId":"X","targetInfo":popup_info(POPUP_ERROR, "seven.sliccy.ai"),"waitingForDebugger":false}}));
    let shown = cdp.events()[mark..].iter().find(|event| event["method"] == "Target.attachedToTarget").expect("delivered");
    assert_eq!(shown["params"]["targetInfo"]["url"], "about:blank");
    assert!(!shown.to_string().contains("SECRET"));
}

#[test]
fn a_slicc_hide_target_event_is_never_delivered() {
    let mut cdp = popup_client(popup_opener(), PopupFrame::Frame(json!({
        "id":"O","loaderId":"LO","url":"https://example.test/opener","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    cdp.protect_target("O");
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":{
        "targetId":"IF","type":"iframe","url":"chrome-error://private/","title":"private","openerId":"O","parentFrameId":"O","attached":false
    }}}));
    assert!(cdp.events()[mark..].iter().all(|event| event["params"]["targetInfo"]["targetId"] != "IF"), "{:?}", &cdp.events()[mark..]);
}

const STALE_MARKER: &str = "STALE_PROOF_MARKER";
const STALE_PROTECTED: &str = "https://seven.sliccy.ai/path/STALE_PROOF_MARKER?q=STALE_PROOF_MARKER#STALE_PROOF_MARKER";
const STALE_PROTECTED_B: &str = "https://vault.sliccy.ai/path/STALE_PROOF_MARKER?q=STALE_PROOF_MARKER#STALE_PROOF_MARKER";
const STALE_ORDINARY: &str = "http://ordinary.invalid/doc?n=STALE_PROOF_MARKER";

fn own_error_frame(loader: &str, unreachable: &str) -> Value {
    let mut frame = json!({"id":"T","loaderId":loader,"url":POPUP_ERROR,"securityOrigin":"://","mimeType":"text/html"});
    if !unreachable.is_empty() {
        frame["unreachableUrl"] = json!(unreachable);
    }
    frame
}

fn assert_client_surfaces_hide_failed(cdp: &mut Cdp, target: &str, label: &str) {
    for event in cdp.events() {
        let text = event.to_string();
        assert!(!text.contains(STALE_MARKER), "{label}: {event}");
        let info = &event["params"]["targetInfo"];
        if info["targetId"] == target {
            let title = info["title"].as_str().unwrap_or("");
            let url = info["url"].as_str().unwrap_or("");
            assert!(!title.contains("sliccy"), "{label}: {event}");
            assert!(!url.contains("sliccy"), "{label}: {event}");
            if title.contains("sliccy") || url.contains("sliccy") || text.contains(STALE_MARKER) {
                assert_eq!(info["url"], "about:blank", "{label}: {event}");
                assert_eq!(info["title"], "about:blank", "{label}: {event}");
            }
        }
    }
    if let Ok(listed) = cdp.call("Target.getTargets", json!({}), None) {
        for info in listed["targetInfos"].as_array().unwrap().iter().filter(|info| info["targetId"] == target) {
            assert!(!info.to_string().contains(STALE_MARKER), "{label}: {info}");
            assert!(!info.to_string().contains("sliccy"), "{label}: {info}");
            assert_eq!(info["url"], "about:blank", "{label}: {info}");
            assert_eq!(info["title"], "about:blank", "{label}: {info}");
        }
    }
    if let Ok(found) = cdp.call("Target.getTargetInfo", json!({"targetId":target}), None) {
        assert!(!found.to_string().contains(STALE_MARKER), "{label}: {found}");
        assert!(!found.to_string().contains("sliccy"), "{label}: {found}");
    }
}

fn assert_surface_clean(value: &Value, label: &str) {
    let text = value.to_string();
    assert!(!text.contains(STALE_MARKER), "{label}: {text}");
    assert!(!text.contains("sliccy"), "{label}: {text}");
}

#[test]
fn a_second_protected_failed_load_is_scrubbed_after_an_ordinary_error_proof() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "ordinary.invalid");
        state.frame = PopupFrame::Frame(popup_error_frame(STALE_ORDINARY));
    }
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "ordinary.invalid")}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":popup_error_frame(STALE_ORDINARY),"type":"Navigation"}}));
    assert!(!cdp.error_unproven(&own_info(POPUP_ERROR, "ordinary.invalid")));
    let mark = cdp.events().len();
    let protected_title = format!("seven.sliccy.ai/path/{STALE_MARKER}");
    let second = own_info(POPUP_ERROR, &protected_title);
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":second.clone()}}));
    let before = cdp.events()[mark..].iter().find(|event| event["method"] == "Target.targetInfoChanged").expect("changed before proof");
    assert_eq!(before["params"]["targetInfo"]["url"], "about:blank", "stale ordinary proof must not release the second title: {before}");
    assert_eq!(before["params"]["targetInfo"]["title"], "about:blank", "{before}");
    assert_surface_clean(before, "before-frame-refresh");
    {
        let mut state = state.borrow_mut();
        state.popup = second.clone();
        state.frame = PopupFrame::Frame(popup_error_frame(STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":popup_error_frame(STALE_PROTECTED),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":second}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":live,"params":{"frame":popup_error_frame(STALE_PROTECTED),"type":"Navigation"}}));
    assert_client_surfaces_hide_failed(&mut cdp, "T", "ordinary-then-protected");
}

#[test]
fn an_own_tab_latch_that_stays_after_an_ordinary_failed_load_over_blocks() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    cdp.protect_target("T");
    assert!(cdp.is_protected_target("T"));
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "ordinary.invalid");
        state.frame = PopupFrame::Frame(own_error_frame("L1", STALE_ORDINARY));
    }
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_ORDINARY,"loaderId":"L1","navigationType":"differentDocument"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "ordinary.invalid")}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("L1", STALE_ORDINARY),"type":"Navigation"}}));
    assert!(!cdp.protected_targets.contains("T"), "availability: ordinary failed-load proof must clear an own-tab latch");
    assert!(!cdp.is_protected_target("T"), "availability: cleared latch must not keep the live session blocked");
    assert!(!cdp.blocked.contains(&live), "availability: live session must be released with the latch");
    assert!(!cdp.error_unproven(&own_info(POPUP_ERROR, "ordinary.invalid")));
}

#[test]
fn a_popup_latch_that_clears_on_a_protected_failed_load_leaks() {
    let mut cdp = popup_client(popup_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}")), PopupFrame::Frame(popup_error_frame(STALE_PROTECTED)));
    popup_attach_refused(&mut cdp, "popup-latch-seed");
    assert!(cdp.is_protected_target("P"));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":popup_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}"))}}));
    cdp.session_targets.insert("W".into(), "P".into());
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"W","params":{"frame":popup_error_frame(STALE_PROTECTED),"type":"Navigation"}}));
    cdp.session_targets.remove("W");
    assert!(cdp.is_protected_target("P"), "leak: a protected failed load must not clear a popup latch");
    popup_attach_refused(&mut cdp, "popup-latch-must-hold");
}

#[test]
fn a_protected_then_ordinary_failed_load_clears_own_latch_and_keeps_popup_latch() {
    {
        let start = own_info("https://example.test/start", "start");
        let state = own_state(start.clone(), PopupFrame::Frame(json!({
            "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
        })));
        let mut cdp = own_client_from(state.clone(), &start);
        let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
        let live = first["sessionId"].as_str().unwrap().to_string();
        let guard = cdp.guards.get("T").cloned().expect("guard");
        {
            let mut state = state.borrow_mut();
            state.popup = own_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}"));
            state.frame = PopupFrame::Frame(popup_error_frame(STALE_PROTECTED));
        }
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}"))}}));
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":popup_error_frame(STALE_PROTECTED),"type":"Navigation"}}));
        assert_client_surfaces_hide_failed(&mut cdp, "T", "own-protected");
        {
            let mut state = state.borrow_mut();
            state.popup = own_info(POPUP_ERROR, "ordinary.invalid");
            state.frame = PopupFrame::Frame(popup_error_frame(STALE_ORDINARY));
        }
        let mark = cdp.events().len();
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "ordinary.invalid")}}));
        let pending = cdp.events()[mark..].iter().find(|event| event["method"] == "Target.targetInfoChanged").expect("ordinary pending");
        assert_eq!(pending["params"]["targetInfo"]["url"], "about:blank", "ordinary title only after proof: {pending}");
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":popup_error_frame(STALE_ORDINARY),"type":"Navigation"}}));
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "ordinary.invalid")}}));
        assert!(!cdp.is_protected_target("T"), "own tab must clear latch after ordinary proof");
        let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
        let tab = listed["targetInfos"].as_array().unwrap().iter().find(|info| info["targetId"] == "T").unwrap();
        assert_eq!(tab["title"], "ordinary.invalid");
        assert_eq!(tab["url"], POPUP_ERROR);
        assert!(cdp.call("Page.navigate", json!({"url":"https://example.test/away"}), Some(&live)).is_ok());
    }
    {
        let mut cdp = popup_client(popup_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}")), PopupFrame::Frame(popup_error_frame(STALE_PROTECTED)));
        popup_attach_refused(&mut cdp, "popup-protected-first");
        assert!(cdp.is_protected_target("P"));
        {
            let shared = POPUP_BROWSER.with(|state| state.borrow().as_ref().map(|state| state.clone()));
            if let Some(state) = shared {
                let mut state = state.borrow_mut();
                state.popup = popup_info(POPUP_ERROR, "ordinary.invalid");
                state.frame = PopupFrame::Frame(popup_error_frame(STALE_ORDINARY));
            }
        }
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":popup_info(POPUP_ERROR, "ordinary.invalid")}}));
        assert!(cdp.is_protected_target("P"), "popup latch must survive an ordinary chrome-error title");
        popup_attach_refused(&mut cdp, "popup-still-latched");
    }
}

#[test]
fn a_second_failed_load_arriving_before_the_first_proof_stays_scrubbed() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Silent);
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let first_info = own_info(POPUP_ERROR, "ordinary.invalid");
    let second_info = own_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}"));
    {
        let mut state = state.borrow_mut();
        state.popup = first_info.clone();
        state.frame = PopupFrame::Silent;
    }
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":first_info}}));
    {
        let mut state = state.borrow_mut();
        state.popup = second_info.clone();
        state.frame = PopupFrame::Frame(popup_error_frame(STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":second_info.clone()}}));
    for event in &cdp.events()[mark..] {
        if event["params"]["targetInfo"]["targetId"] == "T" {
            assert_eq!(event["params"]["targetInfo"]["url"], "about:blank", "{event}");
            assert_surface_clean(event, "back-to-back");
        }
    }
    let guard = cdp.guards.get("T").cloned().expect("guard");
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":popup_error_frame(STALE_PROTECTED),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":second_info}}));
    cdp.absorb(json!({"method":"Page.lifecycleEvent","sessionId":live,"params":{"name":"load","frameId":"T","loaderId":"LP","timestamp":1.0}}));
    assert_client_surfaces_hide_failed(&mut cdp, "T", "back-to-back-after");
}

#[test]
fn a_stale_frame_tree_proof_must_not_apply_after_a_newer_navigation() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_ORDINARY,"loaderId":"LA","navigationType":"differentDocument"}}));
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_PROTECTED,"loaderId":"LB","navigationType":"differentDocument"}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":{
        "id":"T","loaderId":"LA","url":POPUP_ERROR,"securityOrigin":"://","mimeType":"text/html","unreachableUrl":STALE_ORDINARY
    },"type":"Navigation"}}));
    let stale = own_info(POPUP_ERROR, "ordinary.invalid");
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":stale}}));
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}"));
        state.frame = PopupFrame::Frame(popup_error_frame(STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":{
        "id":"T","loaderId":"LB","url":POPUP_ERROR,"securityOrigin":"://","mimeType":"text/html","unreachableUrl":STALE_PROTECTED
    },"type":"Navigation"}}));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}"))}}));
    for event in &cdp.events()[mark..] {
        assert_surface_clean(event, "stale-frame-tree");
        if event["params"]["targetInfo"]["targetId"] == "T" {
            assert_eq!(event["params"]["targetInfo"]["url"], "about:blank", "{event}");
        }
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":live,"params":{"frame":popup_error_frame(STALE_PROTECTED),"type":"Navigation"}}));
    assert_client_surfaces_hide_failed(&mut cdp, "T", "stale-frame-tree-surfaces");
}

#[test]
fn a_new_document_and_reused_target_id_do_not_keep_stale_proof_or_provenance() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    assert!(cdp.client_targets.contains("T"));
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "ordinary.invalid");
        state.frame = PopupFrame::Frame(popup_error_frame(STALE_ORDINARY));
    }
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "ordinary.invalid")}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":{
        "id":"T","loaderId":"L1","url":POPUP_ERROR,"securityOrigin":"://","mimeType":"text/html","unreachableUrl":STALE_ORDINARY
    },"type":"Navigation"}}));
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}"));
        state.frame = PopupFrame::Frame(json!({
            "id":"T","loaderId":"L2","url":POPUP_ERROR,"securityOrigin":"://","mimeType":"text/html","unreachableUrl":STALE_PROTECTED
        }));
    }
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_PROTECTED,"loaderId":"L2","navigationType":"differentDocument"}}));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}"))}}));
    for event in &cdp.events()[mark..] {
        assert_surface_clean(event, "new-loader");
        if event["params"]["targetInfo"]["targetId"] == "T" {
            assert_eq!(event["params"]["targetInfo"]["url"], "about:blank", "{event}");
        }
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":{
        "id":"T","loaderId":"L2","url":POPUP_ERROR,"securityOrigin":"://","mimeType":"text/html","unreachableUrl":STALE_PROTECTED
    },"type":"Navigation"}}));
    assert_client_surfaces_hide_failed(&mut cdp, "T", "new-loader-after");
    let _ = cdp.call("Target.detachFromTarget", json!({"sessionId":live}), None);
    cdp.protect_target("T");
    assert!(cdp.protected_targets.contains("T"));
    cdp.absorb(json!({"method":"Target.targetDestroyed","params":{"targetId":"T"}}));
    assert!(!cdp.client_targets.contains("T"), "destroy must clear provenance");
    assert!(!cdp.protected_targets.contains("T"), "destroy must clear the protected latch");
    assert!(!cdp.is_protected_target("T"), "destroy must clear the protected latch");
    assert!(!cdp.documents.contains_key("T"));
    let reused = own_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}"));
    {
        let mut state = state.borrow_mut();
        state.popup = reused.clone();
        state.frame = PopupFrame::Frame(popup_error_frame(STALE_PROTECTED));
        state.sessions.clear();
    }
    cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":reused.clone()}}));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":reused}}));
    for event in &cdp.events()[mark..] {
        assert_surface_clean(event, "id-reuse");
        if event["params"]["targetInfo"]["targetId"] == "T" {
            assert_eq!(event["params"]["targetInfo"]["url"], "about:blank", "{event}");
        }
    }
}

#[test]
fn two_different_protected_hosts_in_a_row_are_each_scrubbed_on_their_own_document() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    let first_title = format!("seven.sliccy.ai/path/{STALE_MARKER}");
    let second_title = format!("vault.sliccy.ai/path/{STALE_MARKER}");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, &first_title);
        state.frame = PopupFrame::Frame(own_error_frame("LA", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_PROTECTED,"loaderId":"LA","navigationType":"differentDocument"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &first_title)}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("LA", STALE_PROTECTED),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &first_title)}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":live,"params":{"frame":own_error_frame("LA", STALE_PROTECTED),"type":"Navigation"}}));
    assert_client_surfaces_hide_failed(&mut cdp, "T", "protected-host-a");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, &second_title);
        state.frame = PopupFrame::Frame(own_error_frame("LB", STALE_PROTECTED_B));
    }
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_PROTECTED_B,"loaderId":"LB","navigationType":"differentDocument"}}));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &second_title)}}));
    for event in &cdp.events()[mark..] {
        assert_surface_clean(event, "protected-host-b-before");
        if event["params"]["targetInfo"]["targetId"] == "T" {
            assert_eq!(event["params"]["targetInfo"]["url"], "about:blank", "{event}");
        }
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("LB", STALE_PROTECTED_B),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &second_title)}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":live,"params":{"frame":own_error_frame("LB", STALE_PROTECTED_B),"type":"Navigation"}}));
    assert_client_surfaces_hide_failed(&mut cdp, "T", "protected-host-b");
}

#[test]
fn the_same_protected_host_on_a_second_load_is_judged_on_its_own_document() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    let title = format!("seven.sliccy.ai/path/{STALE_MARKER}");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, &title);
        state.frame = PopupFrame::Frame(own_error_frame("L1", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("L1", STALE_PROTECTED),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &title)}}));
    assert_client_surfaces_hide_failed(&mut cdp, "T", "protected-load-1");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, &title);
        state.frame = PopupFrame::Frame(own_error_frame("L2", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_PROTECTED,"loaderId":"L2","navigationType":"differentDocument"}}));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &title)}}));
    for event in &cdp.events()[mark..] {
        assert_surface_clean(event, "protected-load-2-before");
        if event["params"]["targetInfo"]["targetId"] == "T" {
            assert_eq!(event["params"]["targetInfo"]["url"], "about:blank", "{event}");
        }
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("L2", STALE_PROTECTED),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, &title)}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":live,"params":{"frame":own_error_frame("L2", STALE_PROTECTED),"type":"Navigation"}}));
    assert_client_surfaces_hide_failed(&mut cdp, "T", "protected-load-2");
}

#[test]
fn the_same_ordinary_host_on_a_second_load_after_a_protected_one_needs_its_own_proof() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, &format!("seven.sliccy.ai/path/{STALE_MARKER}"));
        state.frame = PopupFrame::Frame(own_error_frame("LP", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("LP", STALE_PROTECTED),"type":"Navigation"}}));
    assert_client_surfaces_hide_failed(&mut cdp, "T", "protected-before-ordinary");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "ordinary.invalid");
        state.frame = PopupFrame::Frame(own_error_frame("L1", STALE_ORDINARY));
    }
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_ORDINARY,"loaderId":"L1","navigationType":"differentDocument"}}));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "ordinary.invalid")}}));
    let pending = cdp.events()[mark..].iter().find(|event| event["method"] == "Target.targetInfoChanged").expect("ordinary pending");
    assert_eq!(pending["params"]["targetInfo"]["url"], "about:blank", "ordinary title only after its own proof: {pending}");
    assert_surface_clean(pending, "ordinary-before-proof");
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("L1", STALE_ORDINARY),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "ordinary.invalid")}}));
    assert!(!cdp.error_unproven(&own_info(POPUP_ERROR, "ordinary.invalid")));
    let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
    let tab = listed["targetInfos"].as_array().unwrap().iter().find(|info| info["targetId"] == "T").unwrap();
    assert_eq!(tab["title"], "ordinary.invalid");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "ordinary.invalid");
        state.frame = PopupFrame::Frame(own_error_frame("L2", STALE_ORDINARY));
    }
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_ORDINARY,"loaderId":"L2","navigationType":"differentDocument"}}));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "ordinary.invalid")}}));
    let again = cdp.events()[mark..].iter().find(|event| event["method"] == "Target.targetInfoChanged").expect("ordinary second");
    assert_eq!(again["params"]["targetInfo"]["url"], "about:blank", "stale ordinary proof must not release the second load: {again}");
    assert_surface_clean(again, "ordinary-second-before");
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("L2", STALE_ORDINARY),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "ordinary.invalid")}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":live,"params":{"frame":own_error_frame("L2", STALE_ORDINARY),"type":"Navigation"}}));
    let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
    let tab = listed["targetInfos"].as_array().unwrap().iter().find(|info| info["targetId"] == "T").unwrap();
    assert_eq!(tab["title"], "ordinary.invalid");
    assert_eq!(tab["url"], POPUP_ERROR);
}
