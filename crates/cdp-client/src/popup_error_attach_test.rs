const POPUP_SEVEN: &str = "https://seven.sliccy.ai/doc?n=popup-SECRET";
const POPUP_ERROR: &str = "chrome-error://chromewebdata/";
const POPUP_ORDINARY: &str = "http://ordinary.invalid/doc?n=popup";

fn name(domain: &str, member: &str) -> String {
    format!("{domain}.{member}")
}

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
        if let Ok(parsed) = url::Url::parse(unreachable) {
            if let Some(host) = parsed.host_str() {
                frame["name"] = json!(host);
            }
        }
    }
    frame
}

fn own_error_frame_named(loader: &str, unreachable: &str) -> Value {
    let mut frame = own_error_frame(loader, unreachable);
    if let Ok(parsed) = url::Url::parse(unreachable) {
        if let Some(host) = parsed.host_str() {
            frame["name"] = json!(host);
        }
    }
    frame
}

fn failed_host(unreachable: &str) -> String {
    url::Url::parse(unreachable).ok().and_then(|url| url.host_str().map(str::to_string)).unwrap_or_default()
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
        let method = call["method"].as_str().unwrap();
        let result = if method == name("Accessibility", "getFullAXTree") {
            json!({
                "nodes": [{
                    "nodeId": "1",
                    "ignored": false,
                    "role": {"type": "role", "value": "link"},
                    "name": {"type": "stringifiable", "value": "Seven"},
                    "properties": [{"name": "url", "value": {"type": "stringifiable", "value": "https://seven.sliccy.ai/path/SECRET"}}]
                }, {
                    "name": "Seven",
                    "url": "https://seven.sliccy.ai/path/SECRET",
                    "role": "link"
                }]
            })
        } else {
        match method {
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
            "Page.getFrameTree" if target != "O" => {
                let session = call["sessionId"].as_str().unwrap_or("");
                let (tree, frame_id, origin) = match state.frame.clone() {
                    PopupFrame::Frame(frame) => {
                        let id = frame.get("id").and_then(Value::as_str).unwrap_or("T").to_string();
                        let origin = frame.get("securityOrigin").and_then(Value::as_str).unwrap_or("https://example.test").to_string();
                        (json!({"frameTree":{"frame":frame}}), id, origin)
                    }
                    PopupFrame::Tree(tree) => {
                        let id = tree.pointer("/frameTree/frame/id").and_then(Value::as_str).unwrap_or("T").to_string();
                        let origin = tree.pointer("/frameTree/frame/securityOrigin").and_then(Value::as_str).unwrap_or("https://example.test").to_string();
                        (tree, id, origin)
                    }
                    PopupFrame::Error => {
                        self.inbox.push_back(json!({"id":call["id"],"sessionId":call["sessionId"],"error":{"code":-32000,"message":"Inspected target navigated or closed"}}).to_string());
                        return Ok(());
                    }
                    PopupFrame::Silent => return Ok(()),
                };
                if !session.is_empty() {
                    self.inbox.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":1,"uniqueId":format!("ctx-{session}"),"origin":origin,"name":"","auxData":{"frameId":frame_id,"isDefault":true}}}}).to_string());
                }
                tree
            }
            "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"O","loaderId":"LO","url":"https://example.test/opener","securityOrigin":"https://example.test","mimeType":"text/html"}}}),
            "Page.getNavigationHistory" => {
                let unreachable = match &state.frame {
                    PopupFrame::Frame(frame) => frame.get("unreachableUrl").and_then(Value::as_str).unwrap_or("").to_string(),
                    _ => String::new(),
                };
                let url = if !unreachable.is_empty() {
                    unreachable.clone()
                } else {
                    state.popup["url"].as_str().unwrap_or("about:blank").to_string()
                };
                let host = failed_host(&url);
                let title = if !host.is_empty() {
                    host
                } else {
                    state.popup["title"].as_str().unwrap_or(url.as_str()).to_string()
                };
                json!({"currentIndex":0,"entries":[{"id":1,"url":url,"userTypedURL":url,"title":title,"transitionType":"link"}]})
            }
            "Page.getResourceTree" => match state.frame.clone() {
                PopupFrame::Frame(frame) => json!({"frameTree":{"frame":frame,"resources":[]}}),
                PopupFrame::Tree(tree) => tree,
                _ => json!({"frameTree":{"frame":{"id":"T","url":"about:blank"},"resources":[]}}),
            }
            "Page.getResourceContent" => json!({"content":format!("body {}", state.popup),"base64Encoded":false}),
            "Page.captureScreenshot" => json!({"data":"iVBORw0KGgo="}),
            "Page.printToPDF" => json!({"data":"JVBERi0="}),
            "Network.getResponseBody" => json!({"body":format!("net {}", state.popup),"base64Encoded":false}),
            "DOM.getDocument" => json!({"root":{"nodeId":1,"nodeName":"HTML","documentURL":state.popup["url"].clone()}}),
            "DOM.describeNode" => {
                let name = match &state.frame {
                    PopupFrame::Frame(frame) => frame.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    PopupFrame::Tree(tree) => tree.pointer("/frameTree/childFrames/0/frame/name").or_else(|| tree.pointer("/frameTree/frame/name")).and_then(Value::as_str).unwrap_or("").to_string(),
                    _ => String::new(),
                };
                json!({"node":{"nodeId":2,"backendNodeId":2,"nodeName":"IFRAME","localName":"iframe","attributes":["name", name]}})
            }
            "Runtime.callFunctionOn" => json!({"result":{"type":"string","value":"document-read"}}),
            "Runtime.evaluate" => {
                let expr = call["params"]["expression"].as_str().unwrap_or("");
                let frame_name = match &state.frame {
                    PopupFrame::Frame(frame) => frame.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    PopupFrame::Tree(tree) => tree.pointer("/frameTree/childFrames/0/frame/name").or_else(|| tree.pointer("/frameTree/frame/name")).and_then(Value::as_str).unwrap_or("").to_string(),
                    _ => String::new(),
                };
                if expr.contains("window.name") {
                    json!({"result":{"type":"string","value":frame_name}})
                } else if expr.contains("nameAndUrl") {
                    json!({"result":{"type":"object","value":{"name":"Seven","url":"https://seven.sliccy.ai/path/SECRET"}}})
                } else if expr.contains("networkRequestLike") {
                    json!({"result":{"type":"object","value":{"name":"Seven","url":"https://seven.sliccy.ai/path/SECRET","method":"GET","headers":{}}}})
                } else {
                    json!({"result":{"type":"string","value":"document-read"}})
                }
            }
            _ => json!({}),
        }
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
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "ordinary.invalid");
        state.frame = PopupFrame::Frame(own_error_frame("L3", STALE_ORDINARY));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("L3", STALE_ORDINARY),"type":"Navigation"}}));
    assert!(!cdp.error_unproven(&own_info(POPUP_ERROR, "ordinary.invalid")));
    assert!(cdp.documents.get("T").is_some_and(|doc| !doc.proof_loader_id.is_empty()));
    assert!(cdp.client_targets.contains("T"));
    let _ = cdp.call("Target.detachFromTarget", json!({"sessionId":live}), None);
    cdp.absorb(json!({"method":"Target.targetDestroyed","params":{"targetId":"T"}}));
    assert!(!cdp.client_targets.contains("T"), "destroy must clear provenance");
    assert!(!cdp.documents.contains_key("T"), "destroy must clear proof documents and proof ids");
    let reused = own_info(POPUP_ERROR, "ordinary.invalid");
    {
        let mut state = state.borrow_mut();
        state.popup = reused.clone();
        state.frame = PopupFrame::Frame(own_error_frame("LR", STALE_ORDINARY));
        state.sessions.clear();
    }
    cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":reused.clone()}}));
    assert!(cdp.error_unproven(&reused), "reused id must not keep the prior ordinary proof");
    assert!(!cdp.client_targets.contains("T"), "reused id must not keep client provenance");
}

#[test]
fn a_protected_latch_survives_target_destruction_by_design() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let _ = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    cdp.protect_target("T");
    assert!(cdp.protected_targets.contains("T"));
    cdp.absorb(json!({"method":"Target.targetDestroyed","params":{"targetId":"T"}}));
    assert!(cdp.protected_targets.contains("T"), "reused id is not sticky-protected is accepted false: protection latch survives destroy");
    assert!(cdp.is_protected_target("T"), "reused id is not sticky-protected is accepted false: protection latch survives destroy");
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

#[test]
fn a_stale_proof_loader_id_after_loader_advance_stays_unproven_on_the_read_gate() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let _ = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "ordinary.invalid");
        state.frame = PopupFrame::Frame(own_error_frame("L1", STALE_ORDINARY));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("L1", STALE_ORDINARY),"type":"Navigation"}}));
    assert!(!cdp.error_unproven(&own_info(POPUP_ERROR, "ordinary.invalid")));
    let doc = cdp.documents.get_mut("T").expect("doc");
    assert_eq!(doc.proof_loader_id, "L1");
    doc.loader_id = "L2".into();
    assert!(cdp.error_unproven(&own_info(POPUP_ERROR, "ordinary.invalid")), "read-side document match must refuse a stale proof loader when unreachable still matches the title");
}

#[test]
fn an_abandoned_mark_survives_target_destruction_by_design() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    cdp.abandon("T");
    assert!(cdp.abandoned.contains("T"));
    assert!(cdp.protected_targets.contains("T"));
    cdp.probing.insert("T".into());
    cdp.session_targets.insert("STALE".into(), "T".into());
    let _ = cdp.call("Target.detachFromTarget", json!({"sessionId":live}), None);
    cdp.absorb(json!({"method":"Target.targetDestroyed","params":{"targetId":"T"}}));
    assert!(cdp.abandoned.contains("T"), "abandoned is a protection mark and survives destroy");
    assert!(cdp.protected_targets.contains("T"), "abandon also latches protected and that latch survives destroy");
    assert!(!cdp.probing.contains("T"), "probing is bookkeeping and clears on destroy");
    assert!(!cdp.session_targets.values().any(|owner| owner == "T"), "session_targets is bookkeeping and clears maps to the destroyed id");
    assert!(!cdp.client_targets.contains("T"), "provenance clears on destroy");
    let reused = own_info(POPUP_ERROR, &format!("seven.sliccy.ai/{STALE_MARKER}"));
    {
        let mut state = state.borrow_mut();
        state.popup = reused.clone();
        state.frame = PopupFrame::Frame(own_error_frame("LR", STALE_PROTECTED));
        state.sessions.clear();
    }
    cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":reused.clone()}}));
    assert!(!cdp.probe_candidate(&reused), "sticky abandoned blocks probe_candidate after id reuse by design");
}

#[test]
fn a_unicode_idn_title_versus_punycode_unreachable_stays_unproven_by_design() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let _ = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    let punycode = "http://xn--bcher-kva.test/doc";
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "xn--bcher-kva.test");
        state.frame = PopupFrame::Frame(own_error_frame("L1", punycode));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame("L1", punycode),"type":"Navigation"}}));
    assert!(!cdp.error_unproven(&own_info(POPUP_ERROR, "xn--bcher-kva.test")));
    assert!(cdp.error_unproven(&own_info(POPUP_ERROR, "bücher.test")), "unicode IDN title vs punycode unreachable is fail-closed over-scrub by design");
}

fn assert_no_failed_host(label: &str, value: &Value, host: &str, marker: &str) {
    let text = value.to_string();
    assert!(!text.contains(marker), "{label}: {text}");
    if let Some(name) = value.pointer("/frameTree/frame/name").or_else(|| value.pointer("/frame/name")).or_else(|| value.get("name")).and_then(Value::as_str) {
        assert!(name.is_empty() || name == "about:blank" || !name.eq_ignore_ascii_case(host) && !name.contains(host), "{label} frame.name={name}");
    }
    if let Some(title) = value.get("title").and_then(Value::as_str) {
        assert!(!title.eq_ignore_ascii_case(host), "{label} title bare host={title}");
        assert!(!title.contains(marker), "{label} title={title}");
    }
}

#[test]
fn an_attached_session_hides_frame_name_and_history_title_for_a_protected_failed_load() {
    let host = "seven.sliccy.ai";
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html","name":""
    })));
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, "ordinary.invalid");
        state.frame = PopupFrame::Frame(own_error_frame_named("LO", STALE_ORDINARY));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame_named("LO", STALE_ORDINARY),"type":"Navigation"}}));
    let ordinary = cdp.call("Page.getFrameTree", json!({}), Some(&live)).unwrap();
    assert_eq!(ordinary.pointer("/frameTree/frame/name").and_then(Value::as_str), Some("ordinary.invalid"));
    let ordinary_history = cdp.call("Page.getNavigationHistory", json!({}), Some(&live)).unwrap();
    assert_eq!(ordinary_history["entries"][0]["title"], "ordinary.invalid");
    let title = format!("{host}/path/{STALE_MARKER}");
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, &title);
        state.frame = PopupFrame::Frame(own_error_frame_named("LP", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_PROTECTED,"loaderId":"LP","navigationType":"differentDocument"}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame_named("LP", STALE_PROTECTED),"type":"Navigation"}}));
    let tree = cdp.call("Page.getFrameTree", json!({}), Some(&live)).unwrap();
    assert_no_failed_host("getFrameTree", &tree, host, STALE_MARKER);
    assert_ne!(tree.pointer("/frameTree/frame/name").and_then(Value::as_str).unwrap_or(host), host, "getFrameTree frame.name must not be the failed host");
    match cdp.call("Page.getResourceTree", json!({}), Some(&live)) {
        Ok(resources) => assert_no_failed_host("getResourceTree", &resources, host, STALE_MARKER),
        Err(error) => assert!(error.contains("seven") || error.contains("refused"), "getResourceTree must refuse or scrub: {error}"),
    }
    let history = cdp.call("Page.getNavigationHistory", json!({}), Some(&live)).unwrap();
    for entry in history["entries"].as_array().unwrap() {
        assert_no_failed_host("history-entry", entry, host, STALE_MARKER);
        assert!(!entry["title"].as_str().unwrap_or("").eq_ignore_ascii_case(host), "history title bare host");
    }
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":live,"params":{"frame":own_error_frame_named("LP", STALE_PROTECTED),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":live,"params":{"frameId":"T","url":STALE_PROTECTED,"loaderId":"LP","name":host,"navigationType":"differentDocument"}}));
    cdp.absorb(json!({"method":"Page.frameAttached","sessionId":live,"params":{"frameId":"C","parentFrameId":"T","stack":{"callFrames":[{"url":STALE_PROTECTED,"functionName":host}]}}}));
    for event in &cdp.events()[mark..] {
        assert_no_failed_host(&format!("event:{}", event["method"]), event, host, STALE_MARKER);
        if let Some(name) = event.pointer("/params/frame/name").and_then(Value::as_str) {
            assert!(name.is_empty() || name == "about:blank", "frameNavigated name={name}");
        }
    }
}

#[test]
fn a_stale_ordinary_proof_must_not_release_frame_name_or_history_title_on_a_later_protected_load() {
    let host = "seven.sliccy.ai";
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
        state.frame = PopupFrame::Frame(own_error_frame_named("L1", STALE_ORDINARY));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame_named("L1", STALE_ORDINARY),"type":"Navigation"}}));
    assert!(!cdp.error_unproven(&own_info(POPUP_ERROR, "ordinary.invalid")));
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, &format!("{host}/path/{STALE_MARKER}"));
        state.frame = PopupFrame::Frame(own_error_frame_named("L2", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_PROTECTED,"loaderId":"L2","navigationType":"differentDocument"}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame_named("L2", STALE_PROTECTED),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":live,"params":{"frame":own_error_frame_named("L2", STALE_PROTECTED),"type":"Navigation"}}));
    let tree = cdp.call("Page.getFrameTree", json!({}), Some(&live)).unwrap();
    assert_no_failed_host("stale-proof-tree", &tree, host, STALE_MARKER);
    assert_ne!(tree.pointer("/frameTree/frame/name").and_then(Value::as_str).unwrap_or(host), host);
    let history = cdp.call("Page.getNavigationHistory", json!({}), Some(&live)).unwrap();
    for entry in history["entries"].as_array().unwrap() {
        assert_no_failed_host("stale-proof-history", entry, host, STALE_MARKER);
    }
    assert!(cdp.call("Page.navigate", json!({"url":"https://example.test/away"}), Some(&live)).is_ok());
    {
        let mut state = state.borrow_mut();
        state.popup = own_info("https://example.test/away", "away");
        state.frame = PopupFrame::Frame(json!({"id":"T","loaderId":"L3","url":"https://example.test/away","securityOrigin":"https://example.test","mimeType":"text/html","name":""}));
        state.sessions.insert(live.clone(), "T".into());
    }
    let after = cdp.call("Page.getNavigationHistory", json!({}), Some(&live)).unwrap();
    assert!(!after.to_string().contains(STALE_MARKER), "{after}");
    assert!(!after["entries"].to_string().contains(host) || after["entries"].as_array().unwrap().iter().all(|entry| {
        !entry["title"].as_str().unwrap_or("").eq_ignore_ascii_case(host)
    }), "navigate-away must not leave a bare failed host title: {after}");
}

#[test]
fn an_attached_session_hides_other_failed_host_carriers_for_a_protected_failed_load() {
    let host = "seven.sliccy.ai";
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
        state.popup = own_info(POPUP_ERROR, &format!("{host}/path/{STALE_MARKER}"));
        state.frame = PopupFrame::Frame(own_error_frame_named("LP", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame_named("LP", STALE_PROTECTED),"type":"Navigation"}}));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":live,"params":{"context":{"id":1,"uniqueId":"ctx","origin":format!("https://{host}"),"name":host,"auxData":{"frameId":"T","isDefault":true}}}}));
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":live,"params":{"requestId":"R1","type":"Document","documentURL":STALE_PROTECTED,"request":{"url":STALE_PROTECTED},"frameId":"T"}}));
    cdp.absorb(json!({"method":"Network.responseReceived","sessionId":live,"params":{"requestId":"R1","type":"Document","response":{"url":STALE_PROTECTED,"status":0},"frameId":"T"}}));
    cdp.absorb(json!({"method":"Network.loadingFailed","sessionId":live,"params":{"requestId":"R1","errorText":format!("net::ERR_NAME_NOT_RESOLVED for https://{host}/path/{STALE_MARKER}"),"type":"Document","canceled":false}}));
    cdp.absorb(json!({"method":"Log.entryAdded","sessionId":live,"params":{"entry":{"source":"network","level":"error","text":format!("Failed to load https://{host}/path/{STALE_MARKER}"),"url":STALE_PROTECTED}}}));
    cdp.absorb(json!({"method":"Runtime.consoleAPICalled","sessionId":live,"params":{"type":"error","args":[{"type":"string","value":format!("net::ERR {host}/{STALE_MARKER}")}]}}));
    cdp.absorb(json!({"method":"Runtime.exceptionThrown","sessionId":live,"params":{"exceptionDetails":{"text":format!("Uncaught {host}/{STALE_MARKER}"),"exception":{"description":format!("Error: {STALE_PROTECTED}")}}}}));
    cdp.absorb(json!({"method":"Page.javascriptDialogOpening","sessionId":live,"params":{"url":STALE_PROTECTED,"message":format!("leave {host}?"),"type":"beforeunload","hasBrowserHandler":false}}));
    cdp.absorb(json!({"method":"Inspector.targetCrashed","sessionId":live,"params":{}}));
    for event in &cdp.events()[mark..] {
        let text = event.to_string();
        assert!(!text.contains(STALE_MARKER), "carrier {}: {event}", event["method"]);
        if let Some(name) = event.pointer("/params/context/name").and_then(Value::as_str) {
            assert!(!name.eq_ignore_ascii_case(host), "executionContext name={name}");
        }
    }
}

#[test]
fn an_ordinary_iframe_named_like_a_protected_host_keeps_its_frame_name() {
    let host_name = "seven.sliccy.ai";
    let start = own_info("https://example.test/start", "start");
    let tree = json!({
        "frameTree": {
            "frame": {"id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html","name":""},
            "childFrames": [{
                "frame": {"id":"IF","loaderId":"LIF","url":"https://example.test/inner","securityOrigin":"https://example.test","mimeType":"text/html","name":host_name}
            }]
        }
    });
    let state = own_state(start.clone(), PopupFrame::Tree(tree.clone()));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let listed = cdp.call("Page.getFrameTree", json!({}), Some(&live)).unwrap();
    assert_eq!(listed.pointer("/frameTree/childFrames/0/frame/name").and_then(Value::as_str), Some(host_name));
    assert_eq!(listed.pointer("/frameTree/childFrames/0/frame/url").and_then(Value::as_str), Some("https://example.test/inner"));
    let resources = cdp.call("Page.getResourceTree", json!({}), Some(&live)).unwrap();
    assert_eq!(resources.pointer("/frameTree/childFrames/0/frame/name").and_then(Value::as_str), Some(host_name));
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":live,"params":{"frame":{"id":"IF","loaderId":"LIF","url":"https://example.test/inner","securityOrigin":"https://example.test","mimeType":"text/html","name":host_name},"type":"Navigation"}}));
    let navigated = cdp.events()[mark..].iter().find(|event| event["method"] == "Page.frameNavigated").expect("frameNavigated");
    assert_eq!(navigated.pointer("/params/frame/name").and_then(Value::as_str), Some(host_name));
}

#[test]
fn an_ordinary_iframe_name_survives_next_to_a_protected_sibling_iframe() {
    let start = own_info("https://example.test/start", "start");
    let tree = json!({
        "frameTree": {
            "frame": {"id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html","name":""},
            "childFrames": [
                {"frame": {"id":"PLAIN","loaderId":"LP","url":"https://example.test/plain","securityOrigin":"https://example.test","mimeType":"text/html","name":"plain-frame"}},
                {"frame": {"id":"PROT","loaderId":"LR","url":"https://seven.sliccy.ai/path/STALE_PROOF_MARKER","securityOrigin":"https://seven.sliccy.ai","mimeType":"text/html","name":"seven.sliccy.ai"}}
            ]
        }
    });
    let state = own_state(start.clone(), PopupFrame::Tree(tree));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let listed = cdp.call("Page.getFrameTree", json!({}), Some(&live)).unwrap();
    let children = listed.pointer("/frameTree/childFrames").and_then(Value::as_array).expect("children");
    let plain = children.iter().find(|node| node.pointer("/frame/id") == Some(&json!("PLAIN"))).expect("plain");
    let protected = children.iter().find(|node| node.pointer("/frame/id") == Some(&json!("PROT"))).expect("protected");
    assert_eq!(plain.pointer("/frame/name").and_then(Value::as_str), Some("plain-frame"));
    assert_eq!(protected.pointer("/frame/name").and_then(Value::as_str), Some(""));
    assert!(!listed.to_string().contains("STALE_PROOF_MARKER"), "{listed}");
}

#[test]
fn window_name_and_dom_frame_name_on_an_ordinary_page_are_not_blanked() {
    let host_name = "seven.sliccy.ai";
    let start = own_info("https://example.test/start", "start");
    let frame = json!({"id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html","name":host_name});
    let state = own_state(start.clone(), PopupFrame::Frame(frame.clone()));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":frame.clone(),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":live,"params":{"frame":frame,"type":"Navigation"}}));
    let _ = cdp.call("Runtime.enable", json!({}), Some(&live));
    let evaluated = cdp.call("Runtime.evaluate", json!({"expression":"window.name"}), Some(&live)).unwrap();
    assert_eq!(evaluated.pointer("/result/value").and_then(Value::as_str), Some(host_name));
    let described = cdp.call("DOM.describeNode", json!({"nodeId":2}), Some(&live)).unwrap();
    let attrs = described.pointer("/node/attributes").and_then(Value::as_array).expect("attrs");
    assert!(attrs.windows(2).any(|pair| pair[0] == "name" && pair[1] == host_name), "{described}");
}


#[test]
fn a_guard_session_page_window_open_is_never_delivered() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    assert_ne!(guard, live);
    assert!(cdp.guard_sessions.contains(&guard));
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":name("Page", "windowOpen"),"sessionId":guard,
        "params":{"url":"https://idp.example/authorize?code=SECRETCODE","windowName":"vault-oauth","windowFeatures":"","userGesture":true}
    }));
    let leaked = cdp.events()[mark..].iter().any(|event| {
        event["method"].as_str() == Some(name("Page", "windowOpen").as_str())
            || event.to_string().contains("SECRETCODE")
            || event.to_string().contains("vault-oauth")
    });
    assert!(!leaked, "guard-session windowOpen must not reach the client: {:?}", &cdp.events()[mark..]);
}

#[test]
fn a_blocked_session_page_window_open_is_never_delivered() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    cdp.blocked.insert(live.clone());
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":name("Page", "windowOpen"),"sessionId":live,
        "params":{"url":"https://example.test/opened","windowName":"blocked-window","windowFeatures":"","userGesture":true}
    }));
    assert!(!cdp.events()[mark..].iter().any(|event| event["method"].as_str() == Some(name("Page", "windowOpen").as_str()) || event.to_string().contains("blocked-window")), "{:?}", &cdp.events()[mark..]);
}

#[test]
fn a_protected_url_page_window_open_on_an_own_session_is_never_delivered() {
    let host = "seven.sliccy.ai";
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":name("Page", "windowOpen"),"sessionId":live,
        "params":{"url":STALE_PROTECTED,"windowName":host,"windowFeatures":"noopener","userGesture":true}
    }));
    assert!(!cdp.events()[mark..].iter().any(|event| {
        event["method"].as_str() == Some(name("Page", "windowOpen").as_str())
            || event.to_string().contains(host)
            || event.to_string().contains(STALE_MARKER)
    }), "protected-url windowOpen must drop as on main: {:?}", &cdp.events()[mark..]);
}

#[test]
fn page_window_open_blanks_window_name_on_an_unproven_failed_load() {
    let host = "seven.sliccy.ai";
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Silent);
    let mut cdp = own_client_from(state.clone(), &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    {
        let mut state = state.borrow_mut();
        state.popup = own_info(POPUP_ERROR, host);
        state.frame = PopupFrame::Silent;
    }
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, host)}}));
    assert!(cdp.error_unproven(&own_info(POPUP_ERROR, host)));
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":name("Page", "windowOpen"),"sessionId":live,
        "params":{"url":POPUP_ERROR,"windowName":host,"windowFeatures":"","userGesture":true}
    }));
    let opened = cdp.events()[mark..].iter().find(|event| event["method"].as_str() == Some(name("Page", "windowOpen").as_str())).expect("windowOpen");
    assert_eq!(opened.pointer("/params/windowName").and_then(Value::as_str), Some(""));
    assert!(!opened.pointer("/params/windowName").and_then(Value::as_str).unwrap_or("").eq_ignore_ascii_case(host), "{opened}");
}

#[test]
fn page_window_open_keeps_an_ordinary_window_name() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":name("Page", "windowOpen"),"sessionId":live,
        "params":{"url":"https://example.test/opened","windowName":"my-window","windowFeatures":"noopener","userGesture":true}
    }));
    let opened = cdp.events()[mark..].iter().find(|event| event["method"].as_str() == Some(name("Page", "windowOpen").as_str())).expect("windowOpen");
    assert_eq!(opened.pointer("/params/windowName").and_then(Value::as_str), Some("my-window"));
    assert_eq!(opened.pointer("/params/url").and_then(Value::as_str), Some("https://example.test/opened"));
}

#[test]
fn settle_guard_never_delivers_guard_blocked_or_own_protected_attach() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let before = cdp.events().len();
    let reply = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = reply["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    for event in &cdp.events()[before..] {
        let session = event["params"]["sessionId"].as_str().unwrap_or("");
        if matches!(event["method"].as_str(), Some("Target.attachedToTarget") | Some("Target.detachedFromTarget")) {
            assert!(!cdp.guard_sessions.contains(session), "guard settle attach must not remain: {event}");
        }
    }
    assert!(cdp.events().iter().any(|event| event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == live));
    cdp.settling_attach.set(true);
    let mark = cdp.events().len();
    let settle_leaks = [
        json!({"method":name("Page", "windowOpen"),"sessionId":guard,"params":{"url":"https://idp.example/authorize?code=SETTLESECRET","windowName":"settle-leak","windowFeatures":"","userGesture":true}}),
        json!({"method":"Page.loadEventFired","sessionId":guard,"params":{"timestamp":1.0}}),
        json!({"method":"Network.requestWillBeSent","sessionId":guard,"params":{"requestId":"NR1","loaderId":"L0","documentURL":"https://idp.example/","request":{"url":"https://idp.example/authorize?code=SETTLESECRET","method":"GET","headers":{},"initialPriority":"VeryHigh","referrerPolicy":"no-referrer"},"timestamp":1.0,"wallTime":1.0,"initiator":{"type":"other"},"redirectHasExtraInfo":false,"type":"Document","hasExtraInfo":false}}),
        json!({"method":"Fetch.requestPaused","sessionId":guard,"params":{"requestId":"FR1","request":{"url":"https://idp.example/authorize?code=SETTLESECRET","method":"GET","headers":{},"initialPriority":"VeryHigh","referrerPolicy":"no-referrer"},"frameId":"T","resourceType":"Document"}}),
        json!({"method":"Runtime.consoleAPICalled","sessionId":guard,"params":{"type":"log","args":[{"type":"string","value":"SETTLESECRET"}],"executionContextId":1,"timestamp":1.0,"stackTrace":{"callFrames":[]}}}),
        json!({"method":"Target.targetCreated","sessionId":guard,"params":{"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai")}}),
        json!({"method":"Target.targetInfoChanged","sessionId":guard,"params":{"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai")}}),
    ];
    for event in settle_leaks {
        cdp.absorb(event);
    }
    assert!(!cdp.events()[mark..].iter().any(|event| {
        let method = event["method"].as_str().unwrap_or("");
        event.get("sessionId").and_then(Value::as_str) == Some(guard.as_str())
            || method == "Target.targetCreated" && event.get("sessionId").and_then(Value::as_str) == Some(guard.as_str())
            || method == "Target.targetInfoChanged" && event.get("sessionId").and_then(Value::as_str) == Some(guard.as_str())
            || event.to_string().contains("SETTLESECRET")
            || event.to_string().contains("settle-leak")
            || event.to_string().contains(STALE_MARKER)
    }), "settle_guard must not widen past attach/detach: {:?}", &cdp.events()[mark..]);
    cdp.blocked.insert(live.clone());
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.attachedToTarget","params":{"sessionId":live,"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai"),"waitingForDebugger":false}
    }));
    assert!(!cdp.events()[mark..].iter().any(|event| {
        event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == live
            || event.to_string().contains(STALE_MARKER)
    }), "blocked attach during settle must drop: {:?}", &cdp.events()[mark..]);
    cdp.blocked.remove(&live);
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.attachedToTarget","params":{"sessionId":live,"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai"),"waitingForDebugger":false}
    }));
    for event in &cdp.events()[mark..] {
        assert!(!event.to_string().contains(STALE_MARKER), "own protected attach during settle must not leak marker: {event}");
        if event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == live {
            let url = event.pointer("/params/targetInfo/url").and_then(Value::as_str).unwrap_or("");
            assert!(!url.contains(STALE_MARKER) && !url.contains("/path/"), "own protected attach must scrub path secrets: {event}");
        }
    }
    cdp.settling_attach.set(false);
}

#[test]
fn settling_attach_clears_on_error_nested_attach_and_panic() {
    let mut refused = popup_client(popup_info(POPUP_ERROR, "seven.sliccy.ai"), PopupFrame::Frame(popup_error_frame(POPUP_SEVEN)));
    assert!(refused.call("Target.attachToTarget", json!({"targetId":"P","flatten":true}), None).is_err());
    assert!(!refused.settling_attach.get(), "error path must clear settling_attach");

    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    cdp.settling_attach.set(true);
    let nested = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None);
    assert!(nested.is_ok(), "{nested:?}");
    assert!(!cdp.settling_attach.get(), "nested attach inside a settle window must clear settling_attach");
    let guard = cdp.guards.get("T").cloned().expect("guard");
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.attachedToTarget","params":{"sessionId":guard,"targetInfo":own_info("https://example.test/start", "start"),"waitingForDebugger":false}
    }));
    assert!(!cdp.events()[mark..].iter().any(|event| {
        event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == guard
    }), "cleared settle must not leave settle_guard sticky: {:?}", &cdp.events()[mark..]);

    cdp.settling_attach.set(true);
    let cell = std::ptr::from_ref(&cdp.settling_attach);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _clear = SettlingAttachClear(cell);
        panic!("settling-attach-panic");
    }));
    assert!(panicked.is_err());
    assert!(!cdp.settling_attach.get(), "SettlingAttachClear must clear on unwind");
}

#[test]
fn watch_scrub_error_alone_hides_unproven_chrome_error_when_redact_does_not() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let _ = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let info = json!({
        "targetId":"E","type":"page","url":POPUP_ERROR,"title":"ordinary.invalid","attached":false,"canAccessOpener":false
    });
    assert!(cdp.error_unproven(&info), "chrome-error without proof must be unproven");
    assert!(!cdp.info_protected(&info, &[]), "own chrome-error without protected url must not be info_protected (scrub_error-only lock)");
    assert!(!cdp.discovery_redacted(&info), "discovery redact must not already mask this shape");
    let mark = cdp.events().len();
    cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":info.clone()}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":info.clone()}}));
    cdp.absorb(json!({"method":"Target.attachedToTarget","params":{"sessionId":"E1","targetInfo":info,"waitingForDebugger":false}}));
    let mut seen = 0;
    for event in &cdp.events()[mark..] {
        if !matches!(event["method"].as_str(), Some("Target.targetCreated") | Some("Target.targetInfoChanged") | Some("Target.attachedToTarget")) {
            continue;
        }
        if event.pointer("/params/targetInfo/targetId").and_then(Value::as_str) != Some("E")
            && event["params"]["sessionId"] != "E1"
        {
            continue;
        }
        seen += 1;
        assert_eq!(event.pointer("/params/targetInfo/url").and_then(Value::as_str), Some("about:blank"), "watch scrub_error alone must placeholder: {event}");
        assert!(!event.to_string().contains("ordinary.invalid"), "{event}");
    }
    assert!(seen >= 2, "expected created/changed/attached deliveries, saw {seen}: {:?}", &cdp.events()[mark..]);
}

#[test]
fn target_created_scrub_skip_drops_guard_blocked_and_scrubs_own_protected() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.targetCreated","sessionId":guard,
        "params":{"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai")}
    }));
    assert!(!cdp.events()[mark..].iter().any(|event| {
        event["method"] == "Target.targetCreated" || event.to_string().contains(STALE_MARKER)
    }), "guard-session targetCreated must drop: {:?}", &cdp.events()[mark..]);
    cdp.blocked.insert(live.clone());
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.targetCreated","sessionId":live,
        "params":{"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai")}
    }));
    assert!(!cdp.events()[mark..].iter().any(|event| {
        event["method"] == "Target.targetCreated" || event.to_string().contains(STALE_MARKER)
    }), "blocked-session targetCreated must drop: {:?}", &cdp.events()[mark..]);
    cdp.blocked.remove(&live);
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.targetCreated",
        "params":{"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai")}
    }));
    for event in &cdp.events()[mark..] {
        if event["method"] == "Target.targetCreated" {
            assert!(!event.to_string().contains(STALE_MARKER), "own protected targetCreated must scrub: {event}");
            let url = event.pointer("/params/targetInfo/url").and_then(Value::as_str).unwrap_or("");
            assert!(url == "about:blank" || !url.contains(STALE_MARKER), "{event}");
        }
    }
}

#[test]
fn target_info_changed_scrub_skip_drops_guard_blocked_and_scrubs_own_protected() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.targetInfoChanged","sessionId":guard,
        "params":{"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai")}
    }));
    assert!(!cdp.events()[mark..].iter().any(|event| {
        event["method"] == "Target.targetInfoChanged" && event.get("sessionId").and_then(Value::as_str) == Some(guard.as_str())
            || event.to_string().contains(STALE_MARKER) && event.get("sessionId").and_then(Value::as_str) == Some(guard.as_str())
    }), "guard-session targetInfoChanged must drop: {:?}", &cdp.events()[mark..]);
    cdp.blocked.insert(live.clone());
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.targetInfoChanged","sessionId":live,
        "params":{"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai")}
    }));
    assert!(!cdp.events()[mark..].iter().any(|event| {
        event.get("sessionId").and_then(Value::as_str) == Some(live.as_str())
            && (event["method"] == "Target.targetInfoChanged" || event.to_string().contains(STALE_MARKER))
    }), "blocked-session targetInfoChanged must drop: {:?}", &cdp.events()[mark..]);
    cdp.blocked.remove(&live);
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.targetInfoChanged",
        "params":{"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai")}
    }));
    for event in &cdp.events()[mark..] {
        if event["method"] == "Target.targetInfoChanged" {
            assert!(!event.to_string().contains(STALE_MARKER), "own protected targetInfoChanged must scrub: {event}");
        }
    }
}

#[test]
fn target_attached_scrub_skip_drops_guard_blocked_and_scrubs_own_protected() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.attachedToTarget","params":{"sessionId":guard,"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai"),"waitingForDebugger":false}
    }));
    assert!(!cdp.events()[mark..].iter().any(|event| {
        event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == guard
            || event.to_string().contains(STALE_MARKER)
    }), "guard attachedToTarget must drop: {:?}", &cdp.events()[mark..]);
    cdp.blocked.insert(live.clone());
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.attachedToTarget","params":{"sessionId":live,"targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai"),"waitingForDebugger":false}
    }));
    assert!(!cdp.events()[mark..].iter().any(|event| {
        event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == live
            || event.to_string().contains(STALE_MARKER)
    }), "blocked attachedToTarget must drop: {:?}", &cdp.events()[mark..]);
    cdp.blocked.remove(&live);
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":"Target.attachedToTarget","params":{"sessionId":"OWN-SCRUB","targetInfo":own_info(STALE_PROTECTED, "seven.sliccy.ai"),"waitingForDebugger":false}
    }));
    for event in &cdp.events()[mark..] {
        if event["method"] == "Target.attachedToTarget" && event["params"]["sessionId"] == "OWN-SCRUB" {
            assert!(!event.to_string().contains(STALE_MARKER), "own protected attachedToTarget must scrub: {event}");
        } else {
            assert!(!event.to_string().contains(STALE_MARKER) || event["method"] != "Target.attachedToTarget", "{event}");
        }
    }
}

#[test]
fn an_accessibility_link_node_keeps_ordinary_text_next_to_a_protected_href() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let tree = cdp.call(&name("Accessibility", "getFullAXTree"), json!({}), Some(&live)).unwrap();
    assert_eq!(tree.pointer("/nodes/0/name/value").and_then(Value::as_str), Some("Seven"));
    assert_eq!(tree.pointer("/nodes/1/name").and_then(Value::as_str), Some("Seven"));
    assert!(!tree.to_string().contains("SECRET"), "{tree}");
}

#[test]
fn a_runtime_remote_object_with_name_and_protected_url_keeps_its_name() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let evaluated = cdp.call("Runtime.evaluate", json!({"expression":"nameAndUrl"}), Some(&live)).unwrap();
    assert_eq!(evaluated.pointer("/result/value/name").and_then(Value::as_str), Some("Seven"));
    assert!(!evaluated.to_string().contains("SECRET"), "{evaluated}");
}

#[test]
fn a_network_request_like_object_with_name_and_protected_url_keeps_its_name() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let evaluated = cdp.call("Runtime.evaluate", json!({"expression":"networkRequestLike"}), Some(&live)).unwrap();
    assert_eq!(evaluated.pointer("/result/value/name").and_then(Value::as_str), Some("Seven"));
    assert_eq!(evaluated.pointer("/result/value/method").and_then(Value::as_str), Some("GET"));
    assert!(!evaluated.to_string().contains("SECRET"), "{evaluated}");
}

#[test]
fn chrome_error_window_open_and_download_hide_after_a_protected_frame_while_target_info_stays_ordinary() {
    let host = "seven.sliccy.ai";
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
        state.frame = PopupFrame::Frame(own_error_frame_named("LP", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame_named("LP", STALE_PROTECTED),"type":"Navigation"}}));
    assert!(!cdp.error_unproven(&start), "targetInfo still ordinary");
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":name("Page", "windowOpen"),"sessionId":live,
        "params":{"url":POPUP_ERROR,"windowName":host,"windowFeatures":"","userGesture":true}
    }));
    cdp.absorb(json!({
        "method":name("Page", "downloadWillBegin"),"sessionId":live,
        "params":{"frameId":"T","guid":"G-ERR","url":POPUP_ERROR,"suggestedFilename":host}
    }));
    let opened = cdp.events()[mark..].iter().find(|event| event["method"].as_str() == Some(name("Page", "windowOpen").as_str())).expect("windowOpen");
    let download = cdp.events()[mark..].iter().find(|event| event["method"].as_str() == Some(name("Page", "downloadWillBegin").as_str())).expect("downloadWillBegin");
    assert_eq!(opened.pointer("/params/windowName").and_then(Value::as_str), Some(""));
    assert_eq!(download.pointer("/params/suggestedFilename").and_then(Value::as_str), Some(""));
    assert_no_failed_host("chrome-error-windowOpen", opened, host, STALE_MARKER);
    assert_no_failed_host("chrome-error-download", download, host, STALE_MARKER);
}

#[test]
fn chrome_error_window_open_and_download_hide_after_ordinary_proof_then_protected_load() {
    let host = "seven.sliccy.ai";
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
        state.frame = PopupFrame::Frame(own_error_frame_named("LO", STALE_ORDINARY));
    }
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, "ordinary.invalid")}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame_named("LO", STALE_ORDINARY),"type":"Navigation"}}));
    assert!(!cdp.error_unproven(&own_info(POPUP_ERROR, "ordinary.invalid")));
    {
        let mut state = state.borrow_mut();
        state.popup = own_info("https://example.test/start", "start");
        state.frame = PopupFrame::Frame(own_error_frame_named("LP", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":guard,"params":{"frameId":"T","url":STALE_PROTECTED,"loaderId":"LP","navigationType":"differentDocument"}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame_named("LP", STALE_PROTECTED),"type":"Navigation"}}));
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info("https://example.test/start", "start")}}));
    assert!(!cdp.error_unproven(&own_info("https://example.test/start", "start")));
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":name("Page", "windowOpen"),"sessionId":live,
        "params":{"url":POPUP_ERROR,"windowName":host,"windowFeatures":"","userGesture":true}
    }));
    cdp.absorb(json!({
        "method":name("Page", "downloadWillBegin"),"sessionId":live,
        "params":{"frameId":"T","guid":"G-RESTART","url":POPUP_ERROR,"suggestedFilename":host}
    }));
    let opened = cdp.events()[mark..].iter().find(|event| event["method"].as_str() == Some(name("Page", "windowOpen").as_str())).expect("windowOpen");
    let download = cdp.events()[mark..].iter().find(|event| event["method"].as_str() == Some(name("Page", "downloadWillBegin").as_str())).expect("downloadWillBegin");
    assert_eq!(opened.pointer("/params/windowName").and_then(Value::as_str), Some(""));
    assert_eq!(download.pointer("/params/suggestedFilename").and_then(Value::as_str), Some(""));
    assert_no_failed_host("restart-windowOpen", opened, host, STALE_MARKER);
    assert_no_failed_host("restart-download", download, host, STALE_MARKER);
}

#[test]
fn an_ordinary_download_keeps_its_suggested_filename() {
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Frame(json!({
        "id":"T","loaderId":"L0","url":"https://example.test/start","securityOrigin":"https://example.test","mimeType":"text/html"
    })));
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":name("Page", "downloadWillBegin"),"sessionId":live,
        "params":{"frameId":"T","guid":"G-ORD","url":"https://example.test/file.bin","suggestedFilename":"report.bin"}
    }));
    let download = cdp.events()[mark..].iter().find(|event| event["method"].as_str() == Some(name("Page", "downloadWillBegin").as_str())).expect("downloadWillBegin");
    assert_eq!(download.pointer("/params/suggestedFilename").and_then(Value::as_str), Some("report.bin"));
    assert_eq!(download.pointer("/params/url").and_then(Value::as_str), Some("https://example.test/file.bin"));
}

#[test]
fn document_unreachable_alone_hides_chrome_error_shaped_window_open_with_ordinary_event_url() {
    let host = "seven.sliccy.ai";
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
        state.frame = PopupFrame::Frame(own_error_frame_named("LP", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame_named("LP", STALE_PROTECTED),"type":"Navigation"}}));
    assert!(!cdp.error_unproven(&start));
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":name("Page", "windowOpen"),"sessionId":live,
        "params":{"url":"https://example.test/opened","unreachableUrl":STALE_ORDINARY,"windowName":host,"windowFeatures":"","userGesture":true}
    }));
    let opened = cdp.events()[mark..].iter().find(|event| event["method"].as_str() == Some(name("Page", "windowOpen").as_str())).expect("windowOpen");
    assert_eq!(opened.pointer("/params/windowName").and_then(Value::as_str), Some(""));
    assert!(!opened.pointer("/params/windowName").and_then(Value::as_str).unwrap_or("").eq_ignore_ascii_case(host));
}

#[test]
fn chrome_error_catch_all_hides_window_open_when_the_target_has_no_document() {
    let host = "seven.sliccy.ai";
    let start = own_info("https://example.test/start", "start");
    let state = own_state(start.clone(), PopupFrame::Silent);
    let mut cdp = own_client_from(state, &start);
    let first = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    let live = first["sessionId"].as_str().unwrap().to_string();
    assert!(!cdp.error_unproven(&start));
    assert!(cdp.documents.get("T").is_none());
    let mark = cdp.events().len();
    cdp.absorb(json!({
        "method":name("Page", "windowOpen"),"sessionId":live,
        "params":{"url":POPUP_ERROR,"windowName":host,"windowFeatures":"","userGesture":true}
    }));
    let opened = cdp.events()[mark..].iter().find(|event| event["method"].as_str() == Some(name("Page", "windowOpen").as_str())).expect("windowOpen");
    assert_eq!(opened.pointer("/params/windowName").and_then(Value::as_str), Some(""));
}

#[test]
fn name_label_carrier_sweep_on_a_protected_failed_load() {
    let host = "seven.sliccy.ai";
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
        state.popup = own_info(POPUP_ERROR, &format!("{host}/path/{STALE_MARKER}"));
        state.frame = PopupFrame::Frame(own_error_frame_named("LP", STALE_PROTECTED));
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":own_error_frame_named("LP", STALE_PROTECTED),"type":"Navigation"}}));
    let mark = cdp.events().len();
    let carriers = [
        json!({"method":name("Page", "windowOpen"),"sessionId":live,"params":{"url":POPUP_ERROR,"windowName":host,"windowFeatures":"","userGesture":true}}),
        json!({"method":"Page.frameRequestedNavigation","sessionId":live,"params":{"frameId":"T","url":STALE_PROTECTED,"reason":"scriptInitiated","disposition":"currentTab"}}),
        json!({"method":"Page.navigatedWithinDocument","sessionId":live,"params":{"frameId":"T","url":STALE_PROTECTED}}),
        json!({"method":name("Page", "fileChooserOpened"),"sessionId":live,"params":{"mode":"selectSingle","frameId":"T","backendNodeId":1}}),
        json!({"method":name("Page", "downloadWillBegin"),"sessionId":live,"params":{"frameId":"T","guid":"G1","url":STALE_PROTECTED,"suggestedFilename":host}}),
        json!({"method":name("Page", "downloadProgress"),"sessionId":live,"params":{"guid":"G1","totalBytes":1,"receivedBytes":0,"state":"inProgress"}}),
        json!({"method":"Page.screencastFrame","sessionId":live,"params":{"sessionId":1,"data":"AAAA","metadata":{"offsetTop":0,"pageScaleFactor":1,"deviceWidth":800,"deviceHeight":600,"scrollOffsetX":0,"scrollOffsetY":0,"timestamp":1.0}}}),
        json!({"method":"Page.lifecycleEvent","sessionId":live,"params":{"frameId":"T","loaderId":"LP","name":"DOMContentLoaded","timestamp":1.0}}),
        json!({"method":name("Page", "interstitialShown"),"sessionId":live,"params":{}}),
        json!({"method":name("Page", "interstitialHidden"),"sessionId":live,"params":{}}),
        json!({"method":"DOM.documentUpdated","sessionId":live,"params":{}}),
        json!({"method":"Network.requestWillBeSentExtraInfo","sessionId":live,"params":{"requestId":"R1","headers":{":authority":host,":path":format!("/path/{STALE_MARKER}")}}}),
        json!({"method":"Runtime.executionContextCreated","sessionId":live,"params":{"context":{"id":2,"uniqueId":"ctx2","origin":format!("https://{host}"),"name":host,"auxData":{"frameId":"T","isDefault":true,"name":host}}}}),
        json!({"method":name("Emulation", "virtualTimeBudgetExpired"),"sessionId":live,"params":{}}),
        json!({"method":name("Overlay", "inspectNodeRequested"),"sessionId":live,"params":{"backendNodeId":1}}),
        json!({"method":name("Performance", "metrics"),"sessionId":live,"params":{"title":host,"metrics":[{"name":"Nodes","value":1.0}]}}),
        json!({"method":"Target.targetInfoChanged","params":{"targetInfo":own_info(POPUP_ERROR, host)}}),
    ];
    for carrier in carriers {
        cdp.absorb(carrier);
    }
    for event in &cdp.events()[mark..] {
        let method = event["method"].as_str().unwrap_or("");
        if method == "Page.lifecycleEvent" {
            assert_eq!(event.pointer("/params/name").and_then(Value::as_str), Some("DOMContentLoaded"));
            continue;
        }
        if method == "Page.screencastFrame" {
            continue;
        }
        assert_no_failed_host(&format!("sweep:{method}"), event, host, STALE_MARKER);
        if let Some(name) = event.pointer("/params/windowName").and_then(Value::as_str) {
            assert!(name.is_empty(), "windowName={name}");
        }
        if let Some(name) = event.pointer("/params/suggestedFilename").and_then(Value::as_str) {
            assert!(name.is_empty(), "suggestedFilename={name}");
        }
        if let Some(title) = event.pointer("/params/targetInfo/title").and_then(Value::as_str) {
            assert!(!title.eq_ignore_ascii_case(host), "title={title}");
        }
        if let Some(title) = event.pointer("/params/title").and_then(Value::as_str) {
            assert!(!title.eq_ignore_ascii_case(host) && !title.contains(STALE_MARKER), "perf title={title}");
        }
    }
}
