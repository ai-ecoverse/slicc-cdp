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
                let url = state.popup["url"].as_str().unwrap_or("about:blank");
                let title = state.popup["title"].as_str().unwrap_or(url);
                json!({"currentIndex":0,"entries":[{"id":1,"url":url,"userTypedURL":url,"title":title,"transitionType":"link"}]})
            }
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
fn guard_session_attach_events_in_the_settle_window_are_hidden_from_the_client() {
    let mut cdp = popup_client_in(popup_info(POPUP_ERROR, "ordinary.invalid"), PopupFrame::Frame(popup_error_frame(POPUP_ORDINARY)), PopupOrder::EventFirst);
    let reply = cdp.call("Target.attachToTarget", json!({"targetId":"P","flatten":true}), None).unwrap();
    assert_eq!(reply["sessionId"], "A1");
    for event in cdp.events() {
        let session = event["params"]["sessionId"].as_str().unwrap_or("");
        if matches!(event["method"].as_str(), Some("Target.attachedToTarget") | Some("Target.detachedFromTarget")) {
            assert!(!cdp.guard_sessions.contains(session), "{event}");
            assert_ne!(session, "", "{event}");
        }
    }
    assert!(cdp.events().iter().any(|event| event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == "A1"));
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
