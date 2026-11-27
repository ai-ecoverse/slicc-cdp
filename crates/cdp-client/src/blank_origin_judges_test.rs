const LONE_ORIGIN_GUARDED: [&str; 3] = ["https://lease-pilot.sliccy.ai", "https://seven.sliccy.ai", "https://vault.sliccy.ai"];
const LONE_ORIGIN_ORDINARY: &str = "https://example.test";
const LONE_ORIGIN_OPAQUE: &str = "://";

#[derive(Default)]
struct LoneOriginTab {
    tree_origin: String,
    tree_loader: String,
    guard_context_origin: String,
    page_context: Option<(String, String)>,
    sessions: Vec<String>,
    wire: Vec<Value>,
    events: VecDeque<String>,
}

struct LoneOriginWire {
    inbox: VecDeque<String>,
    tab: std::rc::Rc<std::cell::RefCell<LoneOriginTab>>,
}

fn lone_origin_info() -> Value {
    json!({"targetId":"T","type":"page","url":"about:blank","title":"","attached":false,"canAccessOpener":false,"browserContextId":"C"})
}

fn lone_origin_frame(origin: &str, loader: &str) -> Value {
    json!({"id":"T","loaderId":loader,"url":"about:blank","securityOrigin":origin,"mimeType":"text/html"})
}

impl Transport for LoneOriginWire {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut tab = self.tab.borrow_mut();
        tab.wire.push(call.clone());
        let session = call["sessionId"].as_str().map(str::to_string);
        let reply = |result: Value| {
            let mut out = json!({"id":call["id"],"result":result});
            if let Some(session) = &session { out["sessionId"] = json!(session); }
            out.to_string()
        };
        let frame = lone_origin_frame(&tab.tree_origin, &tab.tree_loader);
        match call["method"].as_str().unwrap_or("") {
            "Target.getTargets" => self.inbox.push_back(reply(json!({"targetInfos":[lone_origin_info()]}))),
            "Target.getTargetInfo" => self.inbox.push_back(reply(json!({"targetInfo":lone_origin_info()}))),
            "Target.createTarget" => {
                self.inbox.push_back(json!({"method":"Target.targetCreated","params":{"targetInfo":lone_origin_info()}}).to_string());
                self.inbox.push_back(reply(json!({"targetId":"T"})));
            }
            "Target.attachToTarget" => {
                let id = format!("S{}", tab.sessions.len() + 1);
                tab.sessions.push(id.clone());
                self.inbox.push_back(json!({"method":"Target.attachedToTarget","params":{"sessionId":id,"targetInfo":lone_origin_info(),"waitingForDebugger":false}}).to_string());
                self.inbox.push_back(reply(json!({"sessionId":id})));
            }
            "Page.getFrameTree" => self.inbox.push_back(reply(json!({"frameTree":{"frame":frame}}))),
            "Page.getResourceTree" => self.inbox.push_back(reply(json!({"frameTree":{"frame":frame,"resources":[]}}))),
            "Runtime.enable" => {
                let owner = session.clone().unwrap_or_default();
                let origin = tab.page_context.as_ref().filter(|(page, _)| *page == owner).map(|(_, origin)| origin.clone()).unwrap_or_else(|| tab.guard_context_origin.clone());
                self.inbox.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":owner,"params":{"context":{"id":1,"uniqueId":format!("ctx-{owner}"),"origin":origin,"name":"","auxData":{"frameId":"T","isDefault":true,"type":"default"}}}}).to_string());
                self.inbox.push_back(reply(json!({})));
            }
            "Page.getNavigationHistory" => self.inbox.push_back(reply(json!({"currentIndex":0,"entries":[{"id":1,"url":"about:blank","userTypedURL":"about:blank","title":"","transitionType":"typed"}]}))),
            "DOM.getDocument" => self.inbox.push_back(reply(json!({"root":{"nodeId":1,"backendNodeId":1,"nodeName":"#document","documentURL":"about:blank"}}))),
            "Runtime.evaluate" | "Runtime.callFunctionOn" => self.inbox.push_back(reply(json!({"result":{"type":"string","value":"page-value"}}))),
            _ => self.inbox.push_back(reply(json!({}))),
        }
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front().or_else(|| self.tab.borrow_mut().events.pop_front()))
    }
}

struct LoneOriginPage {
    cdp: Cdp,
    tab: std::rc::Rc<std::cell::RefCell<LoneOriginTab>>,
    session: String,
    guard: String,
}

impl LoneOriginPage {
    fn created(tree_origin: &str, context_origin: &str) -> Self {
        let tab = std::rc::Rc::new(std::cell::RefCell::new(LoneOriginTab {
            tree_origin: tree_origin.into(),
            tree_loader: "L1".into(),
            guard_context_origin: context_origin.into(),
            ..Default::default()
        }));
        let mut cdp = Cdp::from_transport(Box::new(LoneOriginWire { inbox: VecDeque::new(), tab: tab.clone() }));
        cdp.call("Target.createTarget", json!({"url":"about:blank"}), None).unwrap();
        let attached = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
        let session = attached["sessionId"].as_str().unwrap().to_string();
        let guard = cdp.guards.get("T").cloned().unwrap();
        assert_ne!(session, guard);
        LoneOriginPage { cdp, tab, session, guard }
    }

    fn deliver(&mut self, events: Vec<Value>) {
        self.tab.borrow_mut().events.extend(events.iter().map(Value::to_string));
        while self.cdp.pump(Instant::now()).unwrap() {}
        assert!(self.tab.borrow().events.is_empty());
    }

    fn commit(&mut self, origin: &str, loader: &str) {
        let guard = self.guard.clone();
        self.deliver(vec![json!({"method":"Page.frameNavigated","sessionId":guard,"params":{"frame":lone_origin_frame(origin, loader),"type":"Navigation"}})]);
    }

    fn send(&mut self, method: &str, params: Value) -> (Result<Value, String>, bool) {
        let mark = self.tab.borrow().wire.len();
        let session = self.session.clone();
        let out = self.cdp.call(method, params.clone(), Some(&session));
        let sent = self.tab.borrow().wire[mark..].iter().any(|call| {
            call["method"] == method && call["sessionId"] == json!(session)
                && params.as_object().unwrap().iter().all(|(key, value)| call["params"].get(key) == Some(value))
        });
        (out, sent)
    }
}

fn lone_origin_content() -> Vec<(&'static str, Value)> {
    vec![
        ("Runtime.evaluate", json!({"expression":"document.title","returnByValue":true})),
        ("Page.captureScreenshot", json!({"format":"png"})),
        ("DOM.getDocument", json!({"depth":1})),
        ("Accessibility.getFullAXTree", json!({"depth":2})),
    ]
}

fn lone_origin_execution() -> Vec<(&'static str, Value)> {
    vec![
        ("Runtime.evaluate", json!({"expression":"document.title","returnByValue":true})),
        ("Runtime.callFunctionOn", json!({"functionDeclaration":"function () { return document.title }","returnByValue":true})),
    ]
}

fn lone_origin_row(commands: fn() -> Vec<(&'static str, Value)>, page: fn(&str) -> LoneOriginPage) {
    for origin in LONE_ORIGIN_GUARDED {
        for (method, params) in commands() {
            let mut opened = page(origin);
            let (out, sent) = opened.send(method, params);
            assert_eq!(out.err(), Some(opened.cdp.refusal()), "{origin} {method}");
            assert!(!sent, "{origin} {method}");
        }
    }
    for origin in [LONE_ORIGIN_ORDINARY, LONE_ORIGIN_OPAQUE] {
        for (method, params) in commands() {
            let mut opened = page(origin);
            let (out, sent) = opened.send(method, params);
            assert!(out.is_ok(), "{origin} {method} {out:?}");
            assert!(sent, "{origin} {method}");
        }
    }
}

#[test]
fn a_created_blank_whose_page_default_context_alone_reports_a_protected_origin_refuses_content() {
    lone_origin_row(lone_origin_content, |origin| {
        let mut opened = LoneOriginPage::created(LONE_ORIGIN_OPAQUE, LONE_ORIGIN_OPAQUE);
        let session = opened.session.clone();
        opened.tab.borrow_mut().page_context = Some((session.clone(), origin.to_string()));
        let _ = opened.cdp.call("Runtime.enable", json!({}), Some(&session));
        opened
    });
}

#[test]
fn a_created_blank_whose_main_frame_commit_alone_carried_a_protected_origin_refuses_content() {
    lone_origin_row(lone_origin_content, |origin| {
        let mut opened = LoneOriginPage::created(LONE_ORIGIN_OPAQUE, LONE_ORIGIN_OPAQUE);
        opened.commit(origin, "L2");
        opened.commit(LONE_ORIGIN_ORDINARY, "L3");
        let mut tab = opened.tab.borrow_mut();
        tab.tree_origin = LONE_ORIGIN_ORDINARY.into();
        tab.tree_loader = "L3".into();
        drop(tab);
        opened
    });
}

#[test]
fn a_created_blank_whose_live_frame_tree_alone_reports_a_protected_origin_refuses_content() {
    lone_origin_row(lone_origin_content, |origin| {
        let opened = LoneOriginPage::created(LONE_ORIGIN_OPAQUE, LONE_ORIGIN_OPAQUE);
        opened.tab.borrow_mut().tree_origin = origin.into();
        opened
    });
}

#[test]
fn a_created_blank_whose_default_context_proof_alone_holds_a_protected_origin_refuses_execution() {
    lone_origin_row(lone_origin_execution, |origin| {
        let mut opened = LoneOriginPage::created(LONE_ORIGIN_ORDINARY, LONE_ORIGIN_ORDINARY);
        let guard = opened.guard.clone();
        let mut held = 0;
        for ((owner, _), proof) in opened.cdp.context_proofs.iter_mut() {
            if *owner == guard && proof.default {
                proof.origin = origin.into();
                held += 1;
            }
        }
        assert_eq!(held, 1);
        opened
    });
}
