struct AncestorBrowser {
    infos: Vec<Value>,
    trees: std::collections::BTreeMap<String, Value>,
    sessions: std::collections::BTreeMap<String, String>,
    attaches: usize,
}

struct AncestorBackend {
    inbox: VecDeque<String>,
    browser: Rc<RefCell<AncestorBrowser>>,
}

impl Transport for AncestorBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let method = call["method"].as_str().unwrap();
        let target = call["params"]["targetId"].as_str().unwrap_or("");
        let session = call["sessionId"].as_str().unwrap_or("");
        let mut browser = self.browser.borrow_mut();
        let result = match method {
            "Target.getTargets" => json!({ "targetInfos": browser.infos }),
            "Target.getTargetInfo" => {
                match browser.infos.iter().find(|info| info["targetId"] == target).cloned() {
                    Some(info) => json!({ "targetInfo": info }),
                    None => {
                        self.inbox.push_back(json!({ "id": call["id"], "error": { "message": "No target with given id" } }).to_string());
                        return Ok(());
                    }
                }
            }
            "Target.attachToTarget" => {
                browser.attaches += 1;
                let session = format!("S{}-{target}", browser.attaches);
                browser.sessions.insert(session.clone(), target.to_string());
                json!({ "sessionId": session })
            }
            "Target.detachFromTarget" => {
                if let Some(child) = call["params"]["sessionId"].as_str() {
                    browser.sessions.remove(child);
                }
                json!({})
            }
            "Inspector.enable" | "Fetch.enable" | "Network.enable" | "Network.setBypassServiceWorker"
            | "Page.enable" | "Runtime.enable" | "Page.addScriptToEvaluateOnNewDocument" | "Page.navigate"
            | "Runtime.evaluate" | "Fetch.disable" | "Network.disable" | "Page.disable"
            | "Runtime.disable" | "Target.activateTarget" | "Target.setDiscoverTargets" => json!({}),
            "Page.getFrameTree" => {
                let owner = browser.sessions.get(session).cloned().unwrap_or_else(|| target.to_string());
                browser.trees.get(&owner).cloned().unwrap_or_else(|| json!({ "frameTree": { "frame": { "id": owner, "url": "about:blank", "securityOrigin": "://", "loaderId": "L" } } }))
            }
            _ => json!({}),
        };
        self.inbox.push_back(json!({ "id": call["id"], "sessionId": call.get("sessionId"), "result": result }).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    parent_url: &'static str,
    child_url: &'static str,
    parent_frame: &'static str,
    nested_middle: bool,
}

fn cases() -> [Case; 6] {
    [
        Case { name: "seven-oopif-https", parent_url: "https://seven.sliccy.ai/os/", child_url: "https://ordinary.test/child", parent_frame: "PARENT", nested_middle: false },
        Case { name: "seven-srcdoc", parent_url: "https://seven.sliccy.ai/os/", child_url: "about:srcdoc", parent_frame: "PARENT", nested_middle: false },
        Case { name: "seven-nested", parent_url: "https://seven.sliccy.ai/os/", child_url: "https://ordinary.test/nested", parent_frame: "MIDDLE", nested_middle: true },
        Case { name: "vault-oopif-https", parent_url: "https://vault.sliccy.ai/app/", child_url: "https://ordinary.test/child", parent_frame: "PARENT", nested_middle: false },
        Case { name: "vault-srcdoc", parent_url: "https://vault.sliccy.ai/app/", child_url: "about:srcdoc", parent_frame: "PARENT", nested_middle: false },
        Case { name: "vault-nested", parent_url: "https://vault.sliccy.ai/app/", child_url: "https://ordinary.test/nested", parent_frame: "MIDDLE", nested_middle: true },
    ]
}

fn fixture(case: Case) -> Cdp {
    let parent = json!({
        "targetId": "PARENT",
        "type": "page",
        "url": case.parent_url,
        "title": case.parent_url,
        "browserContextId": "CTX",
    });
    let child = json!({
        "targetId": "CHILD",
        "type": "iframe",
        "url": case.child_url,
        "title": "",
        "browserContextId": "CTX",
        "parentFrameId": case.parent_frame,
    });
    let ordinary = json!({
        "targetId": "ORDINARY",
        "type": "page",
        "url": "https://ordinary.test/",
        "title": "ordinary",
        "browserContextId": "CTX",
    });
    let ordinary_child = json!({
        "targetId": "ORDINARY-CHILD",
        "type": "iframe",
        "url": "https://foreign.test/cross",
        "title": "",
        "browserContextId": "CTX",
        "parentFrameId": "ORDINARY",
    });
    let mut trees = std::collections::BTreeMap::new();
    let origin = case.parent_url.trim_end_matches('/');
    let parent_tree = if case.nested_middle {
        json!({ "frameTree": {
            "frame": { "id": "PARENT", "url": case.parent_url, "securityOrigin": origin, "loaderId": "P" },
            "childFrames": [{ "frame": { "id": "MIDDLE", "parentId": "PARENT", "url": "about:blank", "securityOrigin": origin, "loaderId": "M" } }]
        }})
    } else {
        json!({ "frameTree": { "frame": { "id": "PARENT", "url": case.parent_url, "securityOrigin": origin, "loaderId": "P" } } })
    };
    trees.insert("PARENT".into(), parent_tree);
    trees.insert("CHILD".into(), json!({ "frameTree": { "frame": { "id": "CHILD", "parentId": case.parent_frame, "url": case.child_url, "securityOrigin": if case.child_url.starts_with("about:") { "://" } else { "https://ordinary.test" }, "loaderId": "C" } } }));
    trees.insert("ORDINARY".into(), json!({ "frameTree": { "frame": { "id": "ORDINARY", "url": "https://ordinary.test/", "securityOrigin": "https://ordinary.test", "loaderId": "O" } } }));
    trees.insert("ORDINARY-CHILD".into(), json!({ "frameTree": { "frame": { "id": "ORDINARY-CHILD", "parentId": "ORDINARY", "url": "https://foreign.test/cross", "securityOrigin": "https://foreign.test", "loaderId": "F" } } }));
    let browser = Rc::new(RefCell::new(AncestorBrowser {
        infos: vec![parent, child, ordinary, ordinary_child],
        trees,
        sessions: std::collections::BTreeMap::new(),
        attaches: 0,
    }));
    let mut cdp = Cdp::from_transport(Box::new(AncestorBackend { inbox: VecDeque::new(), browser }));
    if case.nested_middle {
        cdp.frame_parents.insert("MIDDLE".into(), "PARENT".into());
        cdp.frames.insert("MIDDLE".into(), vec!["about:blank".into(), origin.into()]);
        cdp.frames.insert("PARENT".into(), vec![case.parent_url.into(), origin.into()]);
        cdp.documents.insert("PARENT".into(), super::MainDocument {
            frame_id: "PARENT".into(),
            url: case.parent_url.into(),
            origin: origin.into(),
            unreachable_url: String::new(),
            navigating: false,
            navigation_url: String::new(),
            previous: false,
            probed: false,
        });
    }
    cdp
}

fn admitted(result: &Result<Value, String>) -> bool {
    result.is_ok()
}

fn probe(case: Case) -> String {
    let mut cdp = fixture(case);
    let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
    let listed_child = listed["targetInfos"].as_array().unwrap().iter().any(|info| info["targetId"] == "CHILD");
    let attach = cdp.call("Target.attachToTarget", json!({ "targetId": "CHILD", "flatten": true }), None);
    let session = attach.as_ref().ok().and_then(|value| value["sessionId"].as_str()).map(str::to_string);
    let eval = match &session {
        Some(session) => cdp.call("Runtime.evaluate", json!({ "expression": "1", "returnByValue": true }), Some(session)),
        None => Err("no-session".into()),
    };
    let navigate = match &session {
        Some(session) => cdp.call("Page.navigate", json!({ "url": "https://ordinary.test/hijack" }), Some(session)),
        None => Err("no-session".into()),
    };
    let script = match &session {
        Some(session) => cdp.call("Page.addScriptToEvaluateOnNewDocument", json!({ "source": "window.__pwned=1" }), Some(session)),
        None => Err("no-session".into()),
    };
    let fetch = match &session {
        Some(session) => cdp.call("Fetch.enable", json!({ "patterns": [{ "urlPattern": "*" }] }), Some(session)),
        None => Err("no-session".into()),
    };
    let network = match &session {
        Some(session) => cdp.call("Network.enable", json!({}), Some(session)),
        None => Err("no-session".into()),
    };
    let flag = |ok: bool| if ok { "ADMIT" } else { "REFUSE" };
    format!(
        "{} list={} attach={} eval={} nav={} script={} fetch={} net={}",
        case.name,
        flag(listed_child),
        flag(admitted(&attach)),
        flag(admitted(&eval)),
        flag(admitted(&navigate)),
        flag(admitted(&script)),
        flag(admitted(&fetch)),
        flag(admitted(&network)),
    )
}

#[test]
fn report_protected_parent_iframe_admission_matrix_on_current_head() {
    for case in cases() {
        eprintln!("{}", probe(case));
    }
}

#[test]
fn protected_parent_iframe_children_are_fully_refused() {
    let rows: Vec<_> = cases().into_iter().map(probe).collect();
    let mut ordinary = fixture(cases()[0]);
    assert!(ordinary.call("Target.getTargets", json!({}), None).unwrap()["targetInfos"].as_array().unwrap().iter().any(|info| info["targetId"] == "ORDINARY-CHILD"));
    let ordinary_attach = ordinary.call("Target.attachToTarget", json!({ "targetId": "ORDINARY-CHILD", "flatten": true }), None);
    assert!(ordinary_attach.is_ok(), "ordinary cross-site iframe under ordinary page must keep working: {ordinary_attach:?}");
    let session = ordinary_attach.unwrap()["sessionId"].as_str().unwrap().to_string();
    assert!(ordinary.call("Network.enable", json!({}), Some(&session)).is_ok());
    let expected: Vec<_> = cases().into_iter().map(|case| {
        format!("{} list=REFUSE attach=REFUSE eval=REFUSE nav=REFUSE script=REFUSE fetch=REFUSE net=REFUSE", case.name)
    }).collect();
    assert_eq!(rows, expected, "ancestor-protected iframe admission matrix");
}

#[test]
fn protected_parent_iframe_children_are_hidden_from_discovery_events() {
    for case in cases() {
        let mut cdp = fixture(case);
        let child = json!({
            "targetId": "CHILD",
            "type": "iframe",
            "url": case.child_url,
            "parentFrameId": case.parent_frame,
            "browserContextId": "CTX",
        });
        cdp.targets.insert("PARENT".into(), json!({
            "targetId": "PARENT",
            "type": "page",
            "url": case.parent_url,
            "browserContextId": "CTX",
        }));
        if case.nested_middle {
            cdp.frame_parents.insert("MIDDLE".into(), "PARENT".into());
            cdp.documents.insert("PARENT".into(), super::MainDocument {
                frame_id: "PARENT".into(),
                url: case.parent_url.into(),
                origin: case.parent_url.trim_end_matches('/').into(),
                unreachable_url: String::new(),
                navigating: false,
                navigation_url: String::new(),
                previous: false,
                probed: false,
            });
        }
        for method in ["Target.targetCreated", "Target.targetInfoChanged", "Target.attachedToTarget"] {
            let before = cdp.events().len();
            cdp.absorb(json!({ "method": method, "params": { "sessionId": "X", "targetInfo": child } }));
            let events = cdp.events();
            let delivered = events[before..].iter().any(|event| {
                event["method"] == method && event.pointer("/params/targetInfo/targetId") == Some(&json!("CHILD"))
            });
            assert!(!delivered, "{} still delivered {} for {}", case.name, method, case.child_url);
        }
        assert!(cdp.is_protected_target("CHILD"), "{} must latch protected", case.name);
        let refused = cdp.call("Target.activateTarget", json!({ "targetId": "CHILD" }), None).unwrap_err();
        assert!(refused.starts_with("slicc: refused:"), "{refused}");
        assert!(!refused.contains("ordinary.test"), "{refused}");
        assert!(!refused.contains("about:srcdoc"), "{refused}");
    }
}

fn logging_browser(infos: Vec<Value>) -> (Cdp, Rc<RefCell<AncestorBrowser>>, Rc<RefCell<Vec<Value>>>) {
    let sent = Rc::new(RefCell::new(Vec::new()));
    let browser = Rc::new(RefCell::new(AncestorBrowser {
        infos,
        trees: std::collections::BTreeMap::new(),
        sessions: std::collections::BTreeMap::new(),
        attaches: 0,
    }));
    let backend_sent = sent.clone();
    let backend_browser = browser.clone();
    struct LoggingBackend {
        inbox: VecDeque<String>,
        browser: Rc<RefCell<AncestorBrowser>>,
        sent: Rc<RefCell<Vec<Value>>>,
    }
    impl Transport for LoggingBackend {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            let call: Value = serde_json::from_str(text).unwrap();
            self.sent.borrow_mut().push(call.clone());
            let method = call["method"].as_str().unwrap();
            let target = call["params"]["targetId"].as_str().unwrap_or("");
            let session = call["sessionId"].as_str().unwrap_or("");
            let mut browser = self.browser.borrow_mut();
            let result = match method {
                "Target.getTargets" => json!({ "targetInfos": browser.infos }),
                "Target.getTargetInfo" => {
                    match browser.infos.iter().find(|info| info["targetId"] == target).cloned() {
                        Some(info) => json!({ "targetInfo": info }),
                        None => {
                            self.inbox.push_back(json!({ "id": call["id"], "error": { "message": "No target with given id" } }).to_string());
                            return Ok(());
                        }
                    }
                }
                "Target.attachToTarget" => {
                    browser.attaches += 1;
                    let session = format!("S{}-{target}", browser.attaches);
                    browser.sessions.insert(session.clone(), target.to_string());
                    json!({ "sessionId": session })
                }
                "Target.detachFromTarget" => {
                    if let Some(child) = call["params"]["sessionId"].as_str() {
                        browser.sessions.remove(child);
                    }
                    json!({})
                }
                "Inspector.enable" | "Fetch.enable" | "Network.enable" | "Network.setBypassServiceWorker"
                | "Page.enable" | "Runtime.enable" | "Page.addScriptToEvaluateOnNewDocument" | "Page.navigate"
                | "Runtime.evaluate" | "Fetch.disable" | "Network.disable" | "Page.disable"
                | "Runtime.disable" | "Target.activateTarget" | "Target.setDiscoverTargets"
                | "Fetch.fulfillRequest" | "Fetch.failRequest" | "Fetch.continueRequest" | "Page.getFrameTree" => json!({}),
                _ => json!({}),
            };
            let _ = session;
            self.inbox.push_back(json!({ "id": call["id"], "sessionId": call.get("sessionId"), "result": result }).to_string());
            Ok(())
        }
        fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
            Ok(self.inbox.pop_front())
        }
    }
    let cdp = Cdp::from_transport(Box::new(LoggingBackend {
        inbox: VecDeque::new(),
        browser: backend_browser,
        sent: backend_sent,
    }));
    (cdp, browser, sent)
}

#[test]
fn mid_stale_blank_frame_does_not_outrank_seven_iframe_target() {
    let page = json!({"targetId":"PAGE","type":"page","url":"https://ordinary.test/sheet","browserContextId":"CTX"});
    let mid = json!({"targetId":"MID","type":"iframe","url":"about:blank","parentFrameId":"PAGE","browserContextId":"CTX"});
    let child = json!({"targetId":"CHILD","type":"iframe","url":"https://sprinkle.test/frame","parentFrameId":"MID","browserContextId":"CTX"});
    let (mut cdp, browser, _) = logging_browser(vec![page.clone(), mid.clone(), child.clone()]);
    cdp.targets.insert("PAGE".into(), page);
    cdp.targets.insert("MID".into(), mid.clone());
    cdp.targets.insert("CHILD".into(), child.clone());
    cdp.frames.insert("MID".into(), vec!["about:blank".into()]);
    cdp.documents.insert("PAGE".into(), super::MainDocument {
        frame_id: "PAGE".into(),
        url: "https://ordinary.test/sheet".into(),
        origin: "https://ordinary.test".into(),
        unreachable_url: String::new(),
        navigating: false,
        navigation_url: String::new(),
        previous: false,
        probed: false,
    });
    let seven = json!({"targetId":"MID","type":"iframe","url":"https://seven.sliccy.ai/os/","parentFrameId":"PAGE","browserContextId":"CTX"});
    browser.borrow_mut().infos = vec![
        json!({"targetId":"PAGE","type":"page","url":"https://ordinary.test/sheet","browserContextId":"CTX"}),
        seven.clone(),
        child.clone(),
    ];
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":seven}}));
    let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
    assert!(!listed["targetInfos"].as_array().unwrap().iter().any(|info| info["targetId"] == "CHILD"), "CHILD must be hidden after MID navigates to seven: {listed}");
    let attach = cdp.call("Target.attachToTarget", json!({"targetId":"CHILD","flatten":true}), None);
    assert!(attach.is_err(), "CHILD attach must refuse: {attach:?}");
    assert!(cdp.is_protected_target("CHILD") || cdp.is_protected_target("MID"));
}

#[test]
fn nav_parent_to_seven_cuts_child_session_and_blocks_decider_fulfill() {
    let parent = json!({"targetId":"PARENT","type":"page","url":"https://ordinary.test/","browserContextId":"CTX"});
    let child = json!({"targetId":"CHILD","type":"iframe","url":"https://sprinkle.test/frame","parentFrameId":"PARENT","browserContextId":"CTX"});
    let (mut cdp, browser, sent) = logging_browser(vec![parent.clone(), child.clone()]);
    cdp.targets.insert("PARENT".into(), parent.clone());
    cdp.targets.insert("CHILD".into(), child.clone());
    cdp.documents.insert("PARENT".into(), super::MainDocument {
        frame_id: "PARENT".into(),
        url: "https://ordinary.test/".into(),
        origin: "https://ordinary.test".into(),
        unreachable_url: String::new(),
        navigating: false,
        navigation_url: String::new(),
        previous: false,
        probed: false,
    });
    let attached = cdp.call("Target.attachToTarget", json!({"targetId":"CHILD","flatten":true}), None).unwrap();
    let session = attached["sessionId"].as_str().unwrap().to_string();
    cdp.call("Network.enable", json!({}), Some(&session)).unwrap();
    cdp.call("Fetch.enable", json!({"patterns":[{"urlPattern":"*"}]}), Some(&session)).unwrap();
    cdp.set_decider(&session, Some(Box::new(|_| {
        crate::fetch::Decision::Fulfill(crate::fetch::Fulfill::html(200, "<script>parent.postMessage('pwned','*')</script>"))
    })));
    let seven = json!({"targetId":"PARENT","type":"page","url":"https://seven.sliccy.ai/os/","browserContextId":"CTX"});
    browser.borrow_mut().infos = vec![seven.clone(), child.clone()];
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":seven.clone()}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"PARENT-SESSION","params":{"frame":{"id":"PARENT","url":"https://seven.sliccy.ai/os/","securityOrigin":"https://seven.sliccy.ai","loaderId":"L2"}}}));
    assert!(cdp.is_protected_target("PARENT") || cdp.session_blocked(&session) || cdp.is_protected_target("CHILD"), "parent or child must latch after seven navigation");
    cdp.absorb(json!({
        "method":"Fetch.requestPaused",
        "sessionId":session,
        "params":{
            "requestId":"R1",
            "request":{"url":"https://sprinkle.test/frame","method":"GET","headers":{}},
            "resourceType":"Document",
            "frameId":"CHILD"
        }
    }));
    let fulfilled = sent.borrow().iter().any(|command| command["method"] == "Fetch.fulfillRequest");
    assert!(!fulfilled, "decider must not fulfill after parent became seven: {:?}", sent.borrow());
    assert!(cdp.call("Runtime.evaluate", json!({"expression":"1","returnByValue":true}), Some(&session)).is_err());
}

#[test]
fn worker_of_seven_iframe_is_hidden_and_refused() {
    let parent = json!({"targetId":"PARENT","type":"page","url":"https://seven.sliccy.ai/os/","browserContextId":"CTX"});
    let child = json!({"targetId":"CHILD","type":"iframe","url":"https://sprinkle.test/frame","parentFrameId":"PARENT","browserContextId":"CTX"});
    let worker = json!({"targetId":"WORKER","type":"worker","url":"https://sprinkle.test/worker.js","openerId":"CHILD","browserContextId":"CTX"});
    let (mut cdp, _, _) = logging_browser(vec![parent.clone(), child.clone(), worker.clone()]);
    cdp.targets.insert("PARENT".into(), parent);
    cdp.targets.insert("CHILD".into(), child);
    cdp.targets.insert("WORKER".into(), worker.clone());
    cdp.documents.insert("PARENT".into(), super::MainDocument {
        frame_id: "PARENT".into(),
        url: "https://seven.sliccy.ai/os/".into(),
        origin: "https://seven.sliccy.ai".into(),
        unreachable_url: String::new(),
        navigating: false,
        navigation_url: String::new(),
        previous: true,
        probed: false,
    });
    cdp.protected_targets.insert("PARENT".into());
    let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
    assert!(!listed["targetInfos"].as_array().unwrap().iter().any(|info| info["targetId"] == "WORKER"), "worker must be hidden: {listed}");
    let attach = cdp.call("Target.attachToTarget", json!({"targetId":"WORKER","flatten":true}), None);
    assert!(attach.is_err(), "worker attach must refuse: {attach:?}");
}

fn distinct_infos() -> Vec<Value> {
    vec![
        json!({"targetId":"PAGE","type":"page","url":"https://ordinary.test/","browserContextId":"CTX"}),
        json!({"targetId":"OOPIF","type":"iframe","url":"https://foreign.test/excel","parentFrameId":"FRAME","browserContextId":"CTX"}),
    ]
}

#[test]
fn distinct_frame_id_get_targets_first_is_reversible_after_placement() {
    let (mut cdp, browser, _) = logging_browser(distinct_infos());
    let first = cdp.call("Target.getTargets", json!({}), None).unwrap();
    assert!(!first["targetInfos"].as_array().unwrap().iter().any(|info| info["targetId"] == "OOPIF"), "unplaced OOPIF must hide: {first}");
    assert!(!cdp.is_protected_target("OOPIF"), "unresolved ancestor must not latch protected_targets");
    let page_attach = cdp.call("Target.attachToTarget", json!({"targetId":"PAGE","flatten":true}), None).unwrap();
    let page_session = page_attach["sessionId"].as_str().unwrap().to_string();
    browser.borrow_mut().trees.insert("PAGE".into(), json!({
        "frameTree":{"frame":{"id":"FRAME","url":"https://ordinary.test/","securityOrigin":"https://ordinary.test","loaderId":"L"}}
    }));
    let _ = cdp.call("Page.getFrameTree", json!({}), Some(&page_session));
    cdp.documents.insert("PAGE".into(), super::MainDocument {
        frame_id: "FRAME".into(),
        url: "https://ordinary.test/".into(),
        origin: "https://ordinary.test".into(),
        unreachable_url: String::new(),
        navigating: false,
        navigation_url: String::new(),
        previous: false,
        probed: false,
    });
    cdp.page_scope_owners.insert("FRAME".into(), "PAGE".into());
    let second = cdp.call("Target.getTargets", json!({}), None).unwrap();
    assert!(second["targetInfos"].as_array().unwrap().iter().any(|info| info["targetId"] == "OOPIF"), "placed OOPIF must list: {second}");
    let attach = cdp.call("Target.attachToTarget", json!({"targetId":"OOPIF","flatten":true}), None);
    assert!(attach.is_ok(), "placed OOPIF must attach: {attach:?}");
}

#[test]
fn distinct_frame_id_attach_first_keeps_excel_oopif_working() {
    let (mut cdp, browser, _) = logging_browser(distinct_infos());
    let page_attach = cdp.call("Target.attachToTarget", json!({"targetId":"PAGE","flatten":true}), None).unwrap();
    let page_session = page_attach["sessionId"].as_str().unwrap().to_string();
    browser.borrow_mut().trees.insert("PAGE".into(), json!({
        "frameTree":{"frame":{"id":"FRAME","url":"https://ordinary.test/","securityOrigin":"https://ordinary.test","loaderId":"L"}}
    }));
    let _ = cdp.call("Page.getFrameTree", json!({}), Some(&page_session));
    cdp.documents.insert("PAGE".into(), super::MainDocument {
        frame_id: "FRAME".into(),
        url: "https://ordinary.test/".into(),
        origin: "https://ordinary.test".into(),
        unreachable_url: String::new(),
        navigating: false,
        navigation_url: String::new(),
        previous: false,
        probed: false,
    });
    cdp.page_scope_owners.insert("FRAME".into(), "PAGE".into());
    let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
    assert!(listed["targetInfos"].as_array().unwrap().iter().any(|info| info["targetId"] == "OOPIF"), "{listed}");
    let attach = cdp.call("Target.attachToTarget", json!({"targetId":"OOPIF","flatten":true}), None).unwrap();
    let session = attach["sessionId"].as_str().unwrap().to_string();
    assert!(cdp.call("Network.enable", json!({}), Some(&session)).is_ok());
    assert!(cdp.call("Page.navigate", json!({"url":"https://foreign.test/next"}), Some(&session)).is_ok());
}

#[test]
fn mid_distinct_frame_id_cuts_child_when_middle_navigates_to_seven() {
    let page = json!({"targetId":"PAGE","type":"page","url":"https://ordinary.test/sheet","browserContextId":"CTX"});
    let mid = json!({"targetId":"MID","type":"iframe","url":"about:blank","parentFrameId":"FRAME","browserContextId":"CTX"});
    let child = json!({"targetId":"CHILD","type":"iframe","url":"https://sprinkle.test/frame","parentFrameId":"MIDFRAME","browserContextId":"CTX"});
    let (mut cdp, browser, sent) = logging_browser(vec![page.clone(), mid.clone(), child.clone()]);
    cdp.targets.insert("PAGE".into(), page);
    cdp.targets.insert("MID".into(), mid.clone());
    cdp.targets.insert("CHILD".into(), child.clone());
    cdp.documents.insert("PAGE".into(), super::MainDocument {
        frame_id: "FRAME".into(),
        url: "https://ordinary.test/sheet".into(),
        origin: "https://ordinary.test".into(),
        unreachable_url: String::new(),
        navigating: false,
        navigation_url: String::new(),
        previous: false,
        probed: false,
    });
    cdp.frame_parents.insert("MIDFRAME".into(), "FRAME".into());
    cdp.frames.insert("MIDFRAME".into(), vec!["about:blank".into()]);
    let attached = cdp.call("Target.attachToTarget", json!({"targetId":"CHILD","flatten":true}), None).unwrap();
    let session = attached["sessionId"].as_str().unwrap().to_string();
    cdp.call("Network.enable", json!({}), Some(&session)).unwrap();
    cdp.call("Fetch.enable", json!({"patterns":[{"urlPattern":"*"}]}), Some(&session)).unwrap();
    cdp.set_decider(&session, Some(Box::new(|_| {
        crate::fetch::Decision::Fulfill(crate::fetch::Fulfill::html(200, "<script>parent.postMessage('pwned','*')</script>"))
    })));
    let seven = json!({"targetId":"MID","type":"iframe","url":"https://seven.sliccy.ai/os/","parentFrameId":"FRAME","browserContextId":"CTX"});
    browser.borrow_mut().infos = vec![
        json!({"targetId":"PAGE","type":"page","url":"https://ordinary.test/sheet","browserContextId":"CTX"}),
        seven.clone(),
        child.clone(),
    ];
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":seven}}));
    assert!(cdp.is_protected_target("MID") || cdp.is_protected_target("CHILD"), "MID or CHILD must latch");
    assert!(cdp.session_blocked(&session) || cdp.is_protected_target("CHILD"), "child session must be cut");
    assert!(sent.borrow().iter().any(|command| command["method"] == "Target.detachFromTarget"), "must detach child");
    cdp.absorb(json!({
        "method":"Fetch.requestPaused",
        "sessionId":session,
        "params":{
            "requestId":"R1",
            "request":{"url":"https://sprinkle.test/frame","method":"GET","headers":{}},
            "resourceType":"Document",
            "frameId":"CHILD"
        }
    }));
    assert!(!sent.borrow().iter().any(|command| command["method"] == "Fetch.fulfillRequest"), "must not fulfill: {:?}", sent.borrow());
    assert!(cdp.call("Page.navigate", json!({"url":"https://sprinkle.test/hijack"}), Some(&session)).is_err());
}

#[test]
fn worker_attached_after_creator_forgotten_is_bound_and_cut() {
    let parent = json!({"targetId":"PARENT","type":"page","url":"https://ordinary.test/","browserContextId":"CTX"});
    let child = json!({"targetId":"CHILD","type":"iframe","url":"https://sprinkle.test/frame","parentFrameId":"PARENT","browserContextId":"CTX"});
    let (mut cdp, browser, sent) = logging_browser(vec![parent.clone(), child.clone()]);
    cdp.targets.insert("PARENT".into(), parent.clone());
    cdp.targets.insert("CHILD".into(), child.clone());
    cdp.documents.insert("PARENT".into(), super::MainDocument {
        frame_id: "PARENT".into(),
        url: "https://ordinary.test/".into(),
        origin: "https://ordinary.test".into(),
        unreachable_url: String::new(),
        navigating: false,
        navigation_url: String::new(),
        previous: false,
        probed: false,
    });
    let attached = cdp.call("Target.attachToTarget", json!({"targetId":"CHILD","flatten":true}), None).unwrap();
    let child_session = attached["sessionId"].as_str().unwrap().to_string();
    cdp.call("Network.enable", json!({}), Some(&child_session)).unwrap();
    let seven = json!({"targetId":"PARENT","type":"page","url":"https://seven.sliccy.ai/os/","browserContextId":"CTX"});
    browser.borrow_mut().infos = vec![seven.clone(), child.clone()];
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":seven}}));
    assert!(cdp.session_blocked(&child_session) || !cdp.session_targets.contains_key(&child_session) || cdp.is_protected_target("CHILD"));
    cdp.forget_session(&child_session);
    let worker = json!({"targetId":"WORKER","type":"worker","url":"https://sprinkle.test/worker.js","openerId":"CHILD","browserContextId":"CTX"});
    browser.borrow_mut().infos.push(worker.clone());
    cdp.absorb(json!({
        "method":"Target.attachedToTarget",
        "sessionId":child_session,
        "params":{"sessionId":"HAR-WORKER","targetInfo":worker,"waitingForDebugger":false}
    }));
    assert_eq!(cdp.session_targets.get("HAR-WORKER").map(String::as_str), Some("WORKER"));
    assert!(cdp.is_protected_target("WORKER") || cdp.session_blocked("HAR-WORKER"));
    assert!(cdp.call("Network.enable", json!({}), Some("HAR-WORKER")).is_err());
    cdp.set_decider("HAR-WORKER", Some(Box::new(|_| {
        crate::fetch::Decision::Fulfill(crate::fetch::Fulfill::html(200, "pwned"))
    })));
    cdp.absorb(json!({
        "method":"Fetch.requestPaused",
        "sessionId":"HAR-WORKER",
        "params":{
            "requestId":"RW",
            "request":{"url":"https://sprinkle.test/worker.js","method":"GET","headers":{}},
            "resourceType":"Document",
            "frameId":"WF"
        }
    }));
    assert!(!sent.borrow().iter().any(|command| command["method"] == "Fetch.fulfillRequest" && command["sessionId"] == "HAR-WORKER"), "{:?}", sent.borrow());
}

#[test]
fn ordinary_recommit_clears_descendant_latch() {
    let parent = json!({"targetId":"PARENT","type":"page","url":"https://ordinary.test/","browserContextId":"CTX"});
    let child = json!({"targetId":"CHILD","type":"iframe","url":"https://foreign.test/excel","parentFrameId":"PARENT","browserContextId":"CTX"});
    let (mut cdp, browser, _) = logging_browser(vec![parent.clone(), child.clone()]);
    cdp.targets.insert("PARENT".into(), parent.clone());
    cdp.targets.insert("CHILD".into(), child.clone());
    cdp.session_targets.insert("PS".into(), "PARENT".into());
    cdp.documents.insert("PARENT".into(), super::MainDocument {
        frame_id: "PARENT".into(),
        url: "https://ordinary.test/".into(),
        origin: "https://ordinary.test".into(),
        unreachable_url: String::new(),
        navigating: false,
        navigation_url: String::new(),
        previous: false,
        probed: false,
    });
    let _ = cdp.call("Target.attachToTarget", json!({"targetId":"CHILD","flatten":true}), None).unwrap();
    cdp.main_commit("PS", &json!({"id":"PARENT","url":"https://seven.sliccy.ai/os/","securityOrigin":"https://seven.sliccy.ai","loaderId":"L2"}));
    assert!(cdp.is_protected_target("CHILD"), "child latched under seven");
    let ordinary = json!({"targetId":"PARENT","type":"page","url":"https://ordinary.test/","browserContextId":"CTX"});
    browser.borrow_mut().infos = vec![ordinary.clone(), child.clone()];
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":ordinary}}));
    cdp.main_commit("PS", &json!({"id":"PARENT","url":"https://ordinary.test/","securityOrigin":"https://ordinary.test","loaderId":"L3"}));
    assert!(!cdp.is_protected_target("CHILD"), "child latch must clear after ordinary recommit");
    let attach = cdp.call("Target.attachToTarget", json!({"targetId":"CHILD","flatten":true}), None);
    assert!(attach.is_ok(), "child must attach again: {attach:?}");
}

#[test]
fn protected_subtree_fails_every_paused_resource_type() {
    let parent = json!({"targetId":"PARENT","type":"page","url":"https://ordinary.test/","browserContextId":"CTX"});
    let child = json!({"targetId":"CHILD","type":"iframe","url":"https://sprinkle.test/frame","parentFrameId":"PARENT","browserContextId":"CTX"});
    let (mut cdp, browser, sent) = logging_browser(vec![parent.clone(), child.clone()]);
    cdp.targets.insert("PARENT".into(), parent);
    cdp.targets.insert("CHILD".into(), child.clone());
    cdp.documents.insert("PARENT".into(), super::MainDocument {
        frame_id: "PARENT".into(),
        url: "https://ordinary.test/".into(),
        origin: "https://ordinary.test".into(),
        unreachable_url: String::new(),
        navigating: false,
        navigation_url: String::new(),
        previous: false,
        probed: false,
    });
    let attached = cdp.call("Target.attachToTarget", json!({"targetId":"CHILD","flatten":true}), None).unwrap();
    let session = attached["sessionId"].as_str().unwrap().to_string();
    cdp.call("Fetch.enable", json!({"patterns":[{"urlPattern":"*"}]}), Some(&session)).unwrap();
    let seven = json!({"targetId":"PARENT","type":"page","url":"https://seven.sliccy.ai/os/","browserContextId":"CTX"});
    browser.borrow_mut().infos = vec![seven.clone(), child];
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":seven}}));
    assert!(cdp.session_blocked(&session) || cdp.is_protected_target("CHILD"));
    for (request_id, resource_type) in [("R-DOC", "Document"), ("R-SCRIPT", "Script"), ("R-XHR", "XHR")] {
        cdp.absorb(json!({
            "method":"Fetch.requestPaused",
            "sessionId":session,
            "params":{
                "requestId":request_id,
                "request":{"url":"https://sprinkle.test/x","method":"GET","headers":{}},
                "resourceType":resource_type,
                "frameId":"CHILD"
            }
        }));
    }
    let sent_cmds = sent.borrow().clone();
    let fails: Vec<_> = sent_cmds.iter().filter(|command| command["method"] == "Fetch.failRequest").collect();
    let continues: Vec<_> = sent_cmds.iter().filter(|command| command["method"] == "Fetch.continueRequest").collect();
    assert!(fails.len() >= 3, "every pause must failRequest: fails={fails:?} continues={continues:?}");
    assert!(continues.is_empty(), "must not continueRequest on protected subtree: {continues:?}");
}
