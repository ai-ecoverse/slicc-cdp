fn cdp_method(domain: &str, member: &str) -> String {
    format!("{domain}.{member}")
}

struct InterimBrowser {
    infos: Vec<Value>,
    sessions: std::collections::BTreeMap<String, String>,
    trees: std::collections::BTreeMap<String, Value>,
    attaches: usize,
    sent: Vec<Value>,
}

struct InterimBackend {
    inbox: VecDeque<String>,
    browser: Rc<RefCell<InterimBrowser>>,
}

impl Transport for InterimBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut browser = self.browser.borrow_mut();
        browser.sent.push(call.clone());
        let called = call["method"].as_str().unwrap();
        let target = call["params"]["targetId"].as_str().unwrap_or("");
        let session = call["sessionId"].as_str().unwrap_or("");
        let result = if called == cdp_method("Target", "getTargets") {
            json!({ "targetInfos": browser.infos })
        } else if called == cdp_method("Target", "getTargetInfo") {
            match browser.infos.iter().find(|info| info["targetId"] == target).cloned() {
                Some(info) => json!({ "targetInfo": info }),
                None => {
                    self.inbox.push_back(
                        json!({ "id": call["id"], "error": { "message": "No target with given id" } }).to_string(),
                    );
                    return Ok(());
                }
            }
        } else if called == cdp_method("Target", "attachToTarget") {
            browser.attaches += 1;
            let session = format!("S{}-{target}", browser.attaches);
            browser.sessions.insert(session.clone(), target.to_string());
            json!({ "sessionId": session })
        } else if called == cdp_method("Page", "getFrameTree") {
            let owner = browser
                .sessions
                .get(session)
                .cloned()
                .unwrap_or_else(|| target.to_string());
            browser.trees.get(&owner).cloned().unwrap_or_else(|| {
                json!({ "frameTree": { "frame": { "id": owner, "url": "about:blank", "securityOrigin": "://", "loaderId": "L" } } })
            })
        } else if called == cdp_method("Page", "createIsolatedWorld") {
            json!({ "executionContextId": 7 })
        } else {
            json!({})
        };
        self.inbox.push_back(json!({ "id": call["id"], "sessionId": call.get("sessionId"), "result": result }).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn interim_client(infos: Vec<Value>) -> (Cdp, Rc<RefCell<InterimBrowser>>) {
    let browser = Rc::new(RefCell::new(InterimBrowser {
        infos,
        sessions: std::collections::BTreeMap::new(),
        trees: std::collections::BTreeMap::new(),
        attaches: 0,
        sent: Vec::new(),
    }));
    let cdp = Cdp::from_transport(Box::new(InterimBackend {
        inbox: VecDeque::new(),
        browser: browser.clone(),
    }));
    (cdp, browser)
}

fn listed_ids(listed: &Value) -> Vec<String> {
    listed["targetInfos"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|info| info["targetId"].as_str().map(str::to_string))
        .collect()
}

fn listed_info<'a>(listed: &'a Value, id: &str) -> Option<&'a Value> {
    listed["targetInfos"]
        .as_array()
        .unwrap()
        .iter()
        .find(|info| info["targetId"] == id)
}

#[test]
fn protected_tab_type_target_is_listed_redacted_and_attach_refused() {
    const MARK: &str = "TAB_LOCK_MARK";
    let url = format!("https://seven.sliccy.ai/os/{MARK}?q={MARK}#{MARK}");
    let page = json!({
        "targetId": "PAGE",
        "type": "page",
        "url": url,
        "title": format!("title-{MARK}"),
        "name": format!("name-{MARK}"),
        "browserContextId": "CTX",
    });
    let tab = json!({
        "targetId": "TAB",
        "type": "tab",
        "url": url,
        "title": format!("title-{MARK}"),
        "name": format!("name-{MARK}"),
        "browserContextId": "CTX",
    });
    let ordinary = json!({
        "targetId": "ORDINARY",
        "type": "page",
        "url": "https://ordinary.test/",
        "title": "ordinary",
        "browserContextId": "CTX",
    });
    let (mut cdp, _) = interim_client(vec![page.clone(), tab.clone(), ordinary]);
    let get_targets = cdp_method("Target", "getTargets");
    let get_info = cdp_method("Target", "getTargetInfo");
    let attach = cdp_method("Target", "attachToTarget");
    for method in [
        cdp_method("Target", "targetCreated"),
        cdp_method("Target", "targetInfoChanged"),
        cdp_method("Target", "attachedToTarget"),
    ] {
        let before = cdp.events().len();
        let mut params = json!({ "targetInfo": tab });
        if method == cdp_method("Target", "attachedToTarget") {
            params["sessionId"] = json!("A-TAB");
            params["waitingForDebugger"] = json!(false);
        }
        cdp.absorb(json!({ "method": method, "params": params }));
        for event in &cdp.events()[before..] {
            assert!(!event.to_string().contains(MARK), "{method}: {event}");
            if let Some(info) = event.pointer("/params/targetInfo") {
                assert_eq!(info["url"], "https://seven.sliccy.ai");
                assert_eq!(info["title"], "https://seven.sliccy.ai");
            }
        }
    }
    let listed = cdp
        .call(&get_targets, json!({ "filter": [{}] }), None)
        .unwrap();
    assert!(!listed.to_string().contains(MARK), "{listed}");
    let tab_info = listed_info(&listed, "TAB").expect("tab target must remain listed");
    assert_eq!(tab_info["url"], "https://seven.sliccy.ai");
    assert_eq!(tab_info["title"], "https://seven.sliccy.ai");
    let page_info = listed_info(&listed, "PAGE").expect("page target must remain listed");
    assert_eq!(page_info["url"], "https://seven.sliccy.ai");
    assert!(listed_info(&listed, "ORDINARY").is_some());
    let detail = cdp.call(&get_info, json!({ "targetId": "TAB" }), None).unwrap();
    assert!(!detail.to_string().contains(MARK), "{detail}");
    assert_eq!(detail["targetInfo"]["url"], "https://seven.sliccy.ai");
    let refused = cdp
        .call(&attach, json!({ "targetId": "TAB", "flatten": true }), None)
        .unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{refused}");
    assert!(!refused.contains(MARK), "{refused}");
    assert!(cdp.is_protected_target("TAB"));
}

#[test]
fn protected_oopif_raw_id_attach_is_refused_and_target_info_is_redacted() {
    const MARK: &str = "OOPIF_LOCK_MARK";
    let parent = json!({
        "targetId": "PARENT",
        "type": "page",
        "url": "https://ordinary.test/",
        "title": "ordinary",
        "browserContextId": "CTX",
    });
    let oopif = json!({
        "targetId": "OOPIF",
        "type": "iframe",
        "url": format!("https://seven.sliccy.ai/frame/{MARK}?q={MARK}#{MARK}"),
        "title": format!("title-{MARK}"),
        "name": format!("name-{MARK}"),
        "parentFrameId": "PARENT",
        "browserContextId": "CTX",
    });
    let (mut cdp, _) = interim_client(vec![parent, oopif.clone()]);
    let get_targets = cdp_method("Target", "getTargets");
    let get_info = cdp_method("Target", "getTargetInfo");
    let attach = cdp_method("Target", "attachToTarget");
    for method in [
        cdp_method("Target", "targetCreated"),
        cdp_method("Target", "targetInfoChanged"),
        cdp_method("Target", "attachedToTarget"),
    ] {
        let before = cdp.events().len();
        let mut params = json!({ "targetInfo": oopif });
        if method == cdp_method("Target", "attachedToTarget") {
            params["sessionId"] = json!("A-OOPIF");
            params["waitingForDebugger"] = json!(false);
        }
        cdp.absorb(json!({ "method": method, "params": params }));
        for event in &cdp.events()[before..] {
            assert!(!event.to_string().contains(MARK), "{method}: {event}");
            if let Some(info) = event.pointer("/params/targetInfo") {
                assert_eq!(info["url"], "https://seven.sliccy.ai");
                assert_eq!(info["title"], "https://seven.sliccy.ai");
            }
        }
    }
    assert!(cdp.is_protected_target("OOPIF"));
    let listed = cdp.call(&get_targets, json!({ "filter": [{}] }), None).unwrap();
    assert!(!listed.to_string().contains(MARK), "{listed}");
    let info = listed_info(&listed, "OOPIF").expect("protected OOPIF stays listed redacted");
    assert_eq!(info["url"], "https://seven.sliccy.ai");
    assert_eq!(info["title"], "https://seven.sliccy.ai");
    let detail = cdp.call(&get_info, json!({ "targetId": "OOPIF" }), None).unwrap();
    assert_eq!(detail["targetInfo"]["url"], "https://seven.sliccy.ai");
    assert!(!detail.to_string().contains(MARK), "{detail}");
    let refused = cdp
        .call(&attach, json!({ "targetId": "OOPIF", "flatten": true }), None)
        .unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{refused}");
    assert!(!refused.contains(MARK), "{refused}");
}

#[test]
fn iframe_of_protected_parent_is_hidden_and_attach_refused() {
    let parent = json!({
        "targetId": "PARENT",
        "type": "page",
        "url": "https://seven.sliccy.ai/os/",
        "title": "seven",
        "browserContextId": "CTX",
    });
    let child = json!({
        "targetId": "CHILD",
        "type": "iframe",
        "url": "https://ordinary.test/child",
        "title": "",
        "parentFrameId": "PARENT",
        "browserContextId": "CTX",
    });
    let (mut cdp, _) = interim_client(vec![parent.clone(), child.clone()]);
    cdp.targets.insert("PARENT".into(), parent);
    cdp.targets.insert("CHILD".into(), child.clone());
    cdp.documents.insert(
        "PARENT".into(),
        super::MainDocument {
            frame_id: "PARENT".into(),
            loader_id: String::new(),
            proof_frame_id: String::new(),
            proof_loader_id: String::new(),
            url: "https://seven.sliccy.ai/os/".into(),
            origin: "https://seven.sliccy.ai".into(),
            unreachable_url: String::new(),
            navigating: false,
            navigation_url: String::new(),
            previous: true,
            probed: false,
        },
    );
    cdp.protected_targets.insert("PARENT".into());
    let get_targets = cdp_method("Target", "getTargets");
    let attach = cdp_method("Target", "attachToTarget");
    let listed = cdp.call(&get_targets, json!({}), None).unwrap();
    assert!(
        !listed_ids(&listed).contains(&"CHILD".into()),
        "iframe under protected parent must be hidden: {listed}"
    );
    cdp.absorb(json!({
        "method": cdp_method("Target", "targetCreated"),
        "params": { "targetInfo": child }
    }));
    assert!(
        !cdp
            .events()
            .iter()
            .any(|event| event["method"] == cdp_method("Target", "targetCreated")
                && event["params"]["targetInfo"]["targetId"] == "CHILD"),
        "discovery must hide iframe-of-protected-parent"
    );
    assert!(cdp
        .call(&attach, json!({ "targetId": "CHILD", "flatten": true }), None)
        .unwrap_err()
        .starts_with("slicc: refused:"));
}

const WORKER_MARK: &str = "WORKER_LOCK_MARK";

fn marked_worker_url(origin_path: &str) -> String {
    format!("{origin_path}/{WORKER_MARK}.js?q={WORKER_MARK}#{WORKER_MARK}")
}

fn marked_worker_info(id: &str, kind: &str, url: &str, opener: Option<&str>) -> Value {
    let mut info = json!({
        "targetId": id,
        "type": kind,
        "url": url,
        "title": format!("title-{WORKER_MARK}"),
        "name": format!("name-{WORKER_MARK}"),
        "browserContextId": "CTX",
    });
    if let Some(opener) = opener {
        info["openerId"] = json!(opener);
    }
    info
}

fn assert_no_worker_mark(label: &str, value: &Value) {
    let text = value.to_string();
    assert!(
        !text.contains(WORKER_MARK),
        "{label}: discovery must not leak path/query/fragment/title/name marker: {text}"
    );
}

fn assert_origin_only_worker_info(label: &str, info: &Value, origin: &str) {
    assert_eq!(info["url"], origin, "{label} url");
    assert_eq!(info["title"], origin, "{label} title");
    assert_no_worker_mark(label, info);
}

fn assert_worker_discovery_redacted_no_mark(cdp: &mut Cdp, info: &Value, origin: &str) {
    let id = info["targetId"].as_str().unwrap();
    let get_targets = cdp_method("Target", "getTargets");
    let get_info = cdp_method("Target", "getTargetInfo");
    let attach = cdp_method("Target", "attachToTarget");
    for method in [
        cdp_method("Target", "targetCreated"),
        cdp_method("Target", "targetInfoChanged"),
        cdp_method("Target", "attachedToTarget"),
    ] {
        let before = cdp.events().len();
        let mut params = json!({ "targetInfo": info });
        if method == cdp_method("Target", "attachedToTarget") {
            params["sessionId"] = json!(format!("A-{id}"));
            params["waitingForDebugger"] = json!(false);
        }
        cdp.absorb(json!({ "method": method, "params": params }));
        for event in &cdp.events()[before..] {
            assert_no_worker_mark(&format!("{method} event"), event);
            if let Some(shown) = event.pointer("/params/targetInfo") {
                assert_origin_only_worker_info(&format!("{method} targetInfo"), shown, origin);
            }
        }
    }
    if let Some(listed) = listed_info(&cdp.call(&get_targets, json!({ "filter": [{}] }), None).unwrap(), id) {
        assert_origin_only_worker_info("getTargets", listed, origin);
    }
    match cdp.call(&get_info, json!({ "targetId": id }), None) {
        Ok(detail) => {
            assert_origin_only_worker_info("getTargetInfo", &detail["targetInfo"], origin);
            assert_no_worker_mark("getTargetInfo reply", &detail);
        }
        Err(error) => {
            assert!(error.starts_with("slicc: refused:"), "{error}");
            assert!(!error.contains(WORKER_MARK), "{error}");
        }
    }
    let refused = cdp
        .call(&attach, json!({ "targetId": id, "flatten": true }), None)
        .unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{refused}");
    assert!(!refused.contains(WORKER_MARK), "attach refusal must not leak marker: {refused}");
    assert!(cdp.is_protected_target(id), "{id} must latch");
}

#[test]
fn workers_of_a_protected_page_are_hidden_from_get_targets_but_redacted_in_discovery_events_by_design() {
    let page = json!({
        "targetId": "SEVEN",
        "type": "page",
        "url": "https://seven.sliccy.ai/os/",
        "title": "seven",
        "browserContextId": "CTX",
    });
    let workers = [
        marked_worker_info(
            "DEDICATED",
            "worker",
            &marked_worker_url("https://ordinary.test/path"),
            Some("SEVEN"),
        ),
        marked_worker_info(
            "SHARED",
            "shared_worker",
            &marked_worker_url("https://ordinary.test/shared"),
            Some("SEVEN"),
        ),
        marked_worker_info(
            "SERVICE",
            "service_worker",
            &marked_worker_url("https://ordinary.test/sw"),
            Some("SEVEN"),
        ),
    ];
    let mut infos = vec![page.clone()];
    infos.extend(workers.iter().cloned());
    let (mut cdp, _) = interim_client(infos);
    cdp.targets.insert("SEVEN".into(), page);
    for worker in &workers {
        let id = worker["targetId"].as_str().unwrap();
        cdp.targets.insert(id.into(), worker.clone());
    }
    cdp.protected_targets.insert("SEVEN".into());
    let get_targets = cdp_method("Target", "getTargets");
    let listed = cdp.call(&get_targets, json!({ "filter": [{}] }), None).unwrap();
    assert_no_worker_mark("getTargets listing", &listed);
    for worker in &workers {
        let id = worker["targetId"].as_str().unwrap();
        assert!(
            !listed_ids(&listed).contains(&id.into()),
            "{id} must be hidden from getTargets (opener-page branch): {listed}"
        );
    }
    for worker in &workers {
        assert_worker_discovery_redacted_no_mark(&mut cdp, worker, "https://ordinary.test");
        let listed_again = cdp.call(&get_targets, json!({ "filter": [{}] }), None).unwrap();
        assert!(
            !listed_ids(&listed_again).contains(&worker["targetId"].as_str().unwrap().into()),
            "getTargets must keep hiding after discovery events: {listed_again}"
        );
        assert_no_worker_mark("getTargets after events", &listed_again);
    }
}

#[test]
fn clean_protected_worker_urls_are_listed_or_evented_redacted_not_hidden_and_attach_refused() {
    for (id, kind, origin, path) in [
        ("SW", "service_worker", "https://seven.sliccy.ai", "sw"),
        ("SHARED", "shared_worker", "https://seven.sliccy.ai", "shared"),
        ("DEDICATED", "worker", "https://seven.sliccy.ai", "worker"),
    ] {
        let info = marked_worker_info(id, kind, &marked_worker_url(&format!("{origin}/{path}")), None);
        let ordinary = json!({
            "targetId": "ORDINARY",
            "type": "page",
            "url": "https://ordinary.test/",
            "title": "ordinary",
            "browserContextId": "CTX",
        });
        let (mut cdp, _) = interim_client(vec![ordinary, info.clone()]);
        let get_targets = cdp_method("Target", "getTargets");
        assert_worker_discovery_redacted_no_mark(&mut cdp, &info, origin);
        let listed = cdp.call(&get_targets, json!({ "filter": [{}] }), None).unwrap();
        let shown = listed_info(&listed, id).unwrap_or_else(|| panic!("{kind} clean protected URL is listed redacted by design, not hidden: {listed}"));
        assert_origin_only_worker_info(&format!("{kind} getTargets"), shown, origin);
        assert_no_worker_mark(&format!("{kind} getTargets"), &listed);
    }
}

struct HoldTransport {
    inbox: VecDeque<String>,
}

impl Transport for HoldTransport {
    fn send_text(&mut self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

#[test]
fn held_network_extra_info_is_delivered_after_request_will_be_sent_in_arrival_order() {
    let request_method = cdp_method("Network", "requestWillBeSent");
    let request_extra = cdp_method("Network", "requestWillBeSentExtraInfo");
    let response_extra = cdp_method("Network", "responseReceivedExtraInfo");
    for (label, cookie, associated) in [
        (
            "ordinary",
            "chip=ORDINARY_HELD",
            json!([]),
        ),
        (
            "credential",
            "sid=IDP_HELD_ORDER",
            json!([{
                "blockedReasons": [],
                "cookie": {
                    "name": "sid",
                    "value": "IDP_HELD_ORDER",
                    "domain": ".adobelogin.com",
                    "path": "/"
                }
            }]),
        ),
    ] {
        let mut cdp = Cdp::from_transport(Box::new(HoldTransport {
            inbox: VecDeque::new(),
        }));
        cdp.absorb(json!({
            "method": request_extra,
            "sessionId": "S",
            "params": {
                "requestId": "R",
                "headers": { "Cookie": cookie, "X-Order": "first" },
                "associatedCookies": associated
            }
        }));
        cdp.absorb(json!({
            "method": response_extra,
            "sessionId": "S",
            "params": {
                "requestId": "R",
                "statusCode": 200,
                "headers": { "X-Order": "second" }
            }
        }));
        assert!(
            cdp.events().is_empty(),
            "{label}: ExtraInfo held before requestWillBeSent (cdp.rs deliver_event held path)"
        );
        cdp.absorb(json!({
            "method": request_method,
            "sessionId": "S",
            "params": {
                "requestId": "R",
                "type": "XHR",
                "documentURL": "https://ordinary.invalid/page",
                "request": {
                    "url": "https://ordinary.invalid/api",
                    "method": "GET",
                    "headers": { "Accept": "*/*" }
                }
            }
        }));
        let methods: Vec<_> = cdp
            .events()
            .iter()
            .map(|event| event["method"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            methods,
            vec![request_method.clone(), request_extra.clone(), response_extra.clone()],
            "{label}: held ExtraInfo must follow requestWillBeSent in arrival order: {methods:?}"
        );
        assert_eq!(cdp.events()[1]["params"]["headers"]["X-Order"], "first");
        assert_eq!(cdp.events()[2]["params"]["headers"]["X-Order"], "second");
    }
}

#[test]
fn empty_source_add_script_and_display_emulation_calls_are_refused() {
    let state = Rc::new(RefCell::new(PersistentState::default()));
    let mut cdp = Cdp::from_transport(Box::new(PersistentBackend(state.clone())));
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.absorb(json!({
        "method": cdp_method("Target", "targetInfoChanged"),
        "params": { "targetInfo": { "targetId": "T", "type": "page", "url": "https://ordinary.test/" } }
    }));
    cdp.absorb(json!({
        "method": cdp_method("Page", "frameNavigated"),
        "sessionId": "S",
        "params": { "frame": { "id": "F", "url": "https://ordinary.test/", "securityOrigin": "https://ordinary.test" } }
    }));
    let empty_script = cdp_method("Page", "addScriptToEvaluateOnNewDocument");
    assert!(cdp
        .call(&empty_script, json!({ "source": "" }), Some("S"))
        .unwrap_err()
        .starts_with("slicc: refused:"));
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == empty_script));
    for (domain, member, params) in [
        ("Emulation", "setFocusEmulationEnabled", json!({ "enabled": true })),
        ("Emulation", "setEmulatedMedia", json!({ "media": "print" })),
        (
            "Emulation",
            "setDeviceMetricsOverride",
            json!({ "width": 800, "height": 600, "deviceScaleFactor": 1, "mobile": false }),
        ),
        ("Emulation", "setTouchEmulationEnabled", json!({ "enabled": true })),
        ("Emulation", "setUserAgentOverride", json!({ "userAgent": "" })),
    ] {
        let method = cdp_method(domain, member);
        assert!(
            cdp.call(&method, params.clone(), Some("S"))
                .unwrap_err()
                .starts_with("slicc: refused:"),
            "{method}"
        );
        assert!(
            cdp.notify(&method, params, Some("S"))
                .unwrap_err()
                .starts_with("slicc: refused:"),
            "{method} notify"
        );
        assert!(
            !state.borrow().sent.iter().any(|command| command["method"] == method),
            "{method} reached the transport"
        );
    }
}

#[test]
fn empty_isolated_world_on_protected_document_is_refused() {
    let (mut cdp, browser) = interim_client(vec![json!({
        "targetId": "MAIN",
        "type": "page",
        "url": "https://example.test/",
        "title": "t",
        "browserContextId": "CTX",
    })]);
    browser.borrow_mut().trees.insert(
        "MAIN".into(),
        json!({
            "frameTree": {
                "frame": {
                    "id": "MAIN",
                    "loaderId": "LMAIN",
                    "url": "https://example.test/",
                    "securityOrigin": "https://example.test"
                },
                "childFrames": [{
                    "frame": {
                        "id": "SEVEN",
                        "loaderId": "LSEVEN",
                        "parentId": "MAIN",
                        "url": "https://seven.sliccy.ai/missing",
                        "securityOrigin": "https://seven.sliccy.ai"
                    }
                }]
            }
        }),
    );
    let attach = cdp_method("Target", "attachToTarget");
    let method = cdp_method("Page", "createIsolatedWorld");
    let session = cdp
        .call(&attach, json!({ "targetId": "MAIN", "flatten": true }), None)
        .unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    let _ = cdp.call(&cdp_method("Page", "getFrameTree"), json!({}), Some(&session));
    let before = browser.borrow().sent.len();
    let refused = cdp
        .call(
            &method,
            json!({ "frameId": "SEVEN", "worldName": "" }),
            Some(&session),
        )
        .unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{refused}");
    assert!(
        !browser.borrow().sent[before..]
            .iter()
            .any(|command| command["method"] == method && command["params"]["frameId"] == "SEVEN"),
        "empty isolated world must not reach the transport"
    );
}

#[test]
fn create_isolated_world_on_main_frame_after_client_navigates_to_protected_is_refused() {
    let (mut cdp, browser) = interim_client(vec![json!({
        "targetId": "T",
        "type": "page",
        "url": "https://ordinary.test/",
        "title": "ordinary",
        "browserContextId": "CTX",
    })]);
    browser.borrow_mut().trees.insert(
        "T".into(),
        json!({
            "frameTree": {
                "frame": {
                    "id": "MAIN",
                    "url": "https://ordinary.test/",
                    "securityOrigin": "https://ordinary.test",
                    "loaderId": "L0"
                }
            }
        }),
    );
    let attach = cdp_method("Target", "attachToTarget");
    let create_world = cdp_method("Page", "createIsolatedWorld");
    let session = cdp
        .call(&attach, json!({ "targetId": "T", "flatten": true }), None)
        .unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    cdp.absorb(json!({
        "method": cdp_method("Page", "frameNavigated"),
        "sessionId": session,
        "params": {
            "frame": {
                "id": "MAIN",
                "loaderId": "L1",
                "url": "https://seven.sliccy.ai/os/?n=SECRET",
                "securityOrigin": "https://seven.sliccy.ai"
            }
        }
    }));
    browser.borrow_mut().infos = vec![json!({
        "targetId": "T",
        "type": "page",
        "url": "https://seven.sliccy.ai/os/?n=SECRET",
        "title": "seven",
        "browserContextId": "CTX",
    })];
    browser.borrow_mut().trees.insert(
        "T".into(),
        json!({
            "frameTree": {
                "frame": {
                    "id": "MAIN",
                    "loaderId": "L1",
                    "url": "https://seven.sliccy.ai/os/?n=SECRET",
                    "securityOrigin": "https://seven.sliccy.ai"
                }
            }
        }),
    );
    let refused = cdp
        .call(
            &create_world,
            json!({ "frameId": "MAIN", "worldName": "lock" }),
            Some(&session),
        )
        .unwrap_err();
    assert!(
        refused.starts_with("slicc: refused:"),
        "main-frame createIsolatedWorld after protected navigation must refuse: {refused}"
    );
    assert!(
        !browser
            .borrow()
            .sent
            .iter()
            .any(|command| command["method"] == create_world),
        "createIsolatedWorld must not reach the transport"
    );
}
