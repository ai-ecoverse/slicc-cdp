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
