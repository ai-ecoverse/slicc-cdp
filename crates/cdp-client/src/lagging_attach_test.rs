fn method(domain: &str, member: &str) -> String {
    format!("{domain}.{member}")
}

const LAG_MARK: &str = "LAG_ATTACH_SECRET";
const LAG_BLANK: &str = "about:blank";

fn lag_vault_https() -> String {
    format!("https://vault.sliccy.ai/private/{LAG_MARK}")
}

fn lag_seven_https() -> String {
    format!("https://seven.sliccy.ai/os/{LAG_MARK}?token={LAG_MARK}")
}

fn lag_wrap(scheme: &str, inner: &str) -> String {
    format!("{scheme}:{inner}")
}

fn lag_protected_lives() -> Vec<String> {
    let mut lives = Vec::new();
    for inner in [lag_vault_https(), lag_seven_https()] {
        lives.push(inner.clone());
        lives.push(lag_wrap("view-source", &inner));
        lives.push(lag_wrap("blob", &format!("{inner}/11111111-1111-1111-1111-111111111111")));
        lives.push(lag_wrap("filesystem", &format!("{inner}/temporary/a")));
    }
    lives
}

fn lag_origin(url: &str) -> String {
    if url == LAG_BLANK
        || url.starts_with("view-source:")
        || url.starts_with("blob:")
        || url.starts_with("filesystem:")
        || url.starts_with("chrome:")
        || url.starts_with("chrome-untrusted:")
        || url.starts_with("chrome-extension:")
    {
        return "://".into();
    }
    url::Url::parse(url)
        .ok()
        .map(|parsed| parsed.origin().ascii_serialization())
        .unwrap_or_else(|| "://".into())
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum LagInfoTiming {
    Never,
    BeforeAttach,
    AfterAttachReply,
    AfterFirstCommand,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum LagTreeMode {
    AlwaysLive,
    AlwaysBlank,
    SecondRead,
    BlankThenTimeout,
}

struct LagState {
    info: Value,
    live: Value,
    blank: Value,
    tree_mode: LagTreeMode,
    tree_reads: usize,
    sessions: std::collections::BTreeMap<String, String>,
    attaches: usize,
    client_attaches: usize,
    pending_info: Option<Value>,
    deliver_info_after_command: bool,
}

struct LagBrowser {
    inbox: VecDeque<String>,
    state: std::rc::Rc<std::cell::RefCell<LagState>>,
}

fn lag_info(url: &str) -> Value {
    json!({
        "targetId": "T",
        "type": "page",
        "url": url,
        "title": if url == LAG_BLANK { LAG_BLANK } else { "vault" },
        "attached": false,
        "canAccessOpener": false,
    })
}

fn lag_frame(url: &str, loader: &str) -> Value {
    json!({
        "id": "T",
        "loaderId": loader,
        "url": url,
        "securityOrigin": lag_origin(url),
        "mimeType": "text/html",
    })
}

impl Transport for LagBrowser {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut state = self.state.borrow_mut();
        let called = call["method"].as_str().unwrap();
        let target = call["params"]["targetId"]
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                call["sessionId"]
                    .as_str()
                    .and_then(|session| state.sessions.get(session).cloned())
            })
            .unwrap_or_else(|| "T".into());
        let reply = |result: Value| {
            json!({
                "id": call["id"],
                "sessionId": call["sessionId"],
                "result": result,
            })
            .to_string()
        };
        let attach = method("Target", "attachToTarget");
        let detach = method("Target", "detachFromTarget");
        let get_info = method("Target", "getTargetInfo");
        let get_targets = method("Target", "getTargets");
        let set_discover = method("Target", "setDiscoverTargets");
        let frame_tree = method("Page", "getFrameTree");
        let history = method("Page", "getNavigationHistory");
        let resource_tree = method("Page", "getResourceTree");
        let resource_content = method("Page", "getResourceContent");
        let capture = method("Page", "captureScreenshot");
        let snapshot = method("Page", "captureSnapshot");
        let print_pdf = method("Page", "printToPDF");
        let nav = method("Page", "navigate");
        let enable_page = method("Page", "enable");
        let enable_runtime = method("Runtime", "enable");
        let evaluate = method("Runtime", "evaluate");
        let call_fn = method("Runtime", "callFunctionOn");
        let dom_document = method("DOM", "getDocument");
        let net_body = method("Network", "getResponseBody");
        let enable_net = method("Network", "enable");
        let bypass = method("Network", "setBypassServiceWorker");
        let enable_fetch = method("Fetch", "enable");
        let enable_inspector = method("Inspector", "enable");
        let result = if called == get_info {
            json!({ "targetInfo": state.info.clone() })
        } else if called == get_targets {
            json!({ "targetInfos": [state.info.clone()] })
        } else if called == set_discover {
            json!({})
        } else if called == attach {
            state.attaches += 1;
            let session = format!("S{}", state.attaches);
            state.sessions.insert(session.clone(), target.clone());
            let client = state.attaches == 1;
            if client {
                state.client_attaches += 1;
            }
            let attached = json!({
                "method": method("Target", "attachedToTarget"),
                "params": {
                    "sessionId": session,
                    "targetInfo": state.info.clone(),
                    "waitingForDebugger": false,
                },
            })
            .to_string();
            self.inbox.push_back(attached);
            if let Some(info) = state.pending_info.take() {
                state.info = info.clone();
                self.inbox.push_back(
                    json!({
                        "method": method("Target", "targetInfoChanged"),
                        "params": { "targetInfo": info },
                    })
                    .to_string(),
                );
            }
            json!({ "sessionId": session })
        } else if called == detach {
            if let Some(session) = call["params"]["sessionId"].as_str() {
                if let Some(owner) = state.sessions.remove(session) {
                    self.inbox.push_back(reply(json!({})));
                    self.inbox.push_back(
                        json!({
                            "method": method("Target", "detachedFromTarget"),
                            "params": { "sessionId": session, "targetId": owner },
                        })
                        .to_string(),
                    );
                    return Ok(());
                }
            }
            json!({})
        } else if called == frame_tree {
            state.tree_reads += 1;
            if state.tree_mode == LagTreeMode::BlankThenTimeout && state.tree_reads >= 2 {
                self.inbox.push_back(
                    json!({
                        "id": call["id"],
                        "sessionId": call["sessionId"],
                        "error": { "message": "Page.getFrameTree timed out" },
                    })
                    .to_string(),
                );
                return Ok(());
            }
            let frame = match state.tree_mode {
                LagTreeMode::AlwaysLive => state.live.clone(),
                LagTreeMode::AlwaysBlank | LagTreeMode::BlankThenTimeout => state.blank.clone(),
                LagTreeMode::SecondRead if state.tree_reads >= 2 => state.live.clone(),
                LagTreeMode::SecondRead => state.blank.clone(),
            };
            json!({ "frameTree": { "frame": frame } })
        } else if called == history {
            let url = state.info["url"].as_str().unwrap_or(LAG_BLANK).to_string();
            json!({
                "currentIndex": 0,
                "entries": [{
                    "id": 1,
                    "url": url,
                    "userTypedURL": url,
                    "title": state.info["title"].clone(),
                    "transitionType": "link",
                }],
            })
        } else if called == resource_tree {
            json!({ "frameTree": { "frame": state.live.clone(), "resources": [] } })
        } else if called == resource_content {
            json!({ "content": format!("body {LAG_MARK}"), "base64Encoded": false })
        } else if called == capture || called == print_pdf {
            json!({ "data": "iVBORw0KGgo=" })
        } else if called == snapshot {
            json!({ "document": format!("snapshot {LAG_MARK}") })
        } else if called == evaluate || called == call_fn {
            if state.deliver_info_after_command {
                if let Some(info) = state.pending_info.take() {
                    state.info = info.clone();
                    self.inbox.push_back(
                        json!({
                            "method": method("Target", "targetInfoChanged"),
                            "params": { "targetInfo": info },
                        })
                        .to_string(),
                    );
                }
                state.deliver_info_after_command = false;
            }
            json!({ "result": { "type": "string", "value": format!("leaked-{LAG_MARK}") } })
        } else if called == dom_document {
            json!({
                "root": {
                    "nodeId": 1,
                    "nodeName": "HTML",
                    "documentURL": state.live["url"].clone(),
                },
            })
        } else if called == net_body {
            json!({ "body": format!("net {LAG_MARK}"), "base64Encoded": false })
        } else if called == enable_runtime {
            let session = call["sessionId"].as_str().unwrap_or("");
            let tree_url = match state.tree_mode {
                LagTreeMode::AlwaysLive => state.live.get("url").and_then(Value::as_str).unwrap_or(LAG_BLANK),
                LagTreeMode::AlwaysBlank | LagTreeMode::BlankThenTimeout => {
                    state.blank.get("url").and_then(Value::as_str).unwrap_or(LAG_BLANK)
                }
                LagTreeMode::SecondRead if state.tree_reads >= 2 => {
                    state.live.get("url").and_then(Value::as_str).unwrap_or(LAG_BLANK)
                }
                LagTreeMode::SecondRead => state.blank.get("url").and_then(Value::as_str).unwrap_or(LAG_BLANK),
            };
            let frame_id = state.blank.get("id").and_then(Value::as_str).unwrap_or("T");
            let origin = if tree_url.starts_with("chrome:")
                || tree_url.starts_with("chrome-untrusted:")
                || tree_url.starts_with("chrome-extension:")
            {
                tree_url.to_string()
            } else {
                url::Url::parse(tree_url)
                    .ok()
                    .map(|parsed| parsed.origin().ascii_serialization())
                    .filter(|text| text != "null")
                    .unwrap_or_else(|| "https://ordinary.example".into())
            };
            self.inbox.push_back(
                json!({
                    "method": method("Runtime", "executionContextCreated"),
                    "sessionId": session,
                    "params": {
                        "context": {
                            "id": 1,
                            "uniqueId": format!("ctx-{session}"),
                            "origin": origin,
                            "name": "",
                            "auxData": { "frameId": frame_id, "isDefault": true },
                        }
                    },
                })
                .to_string(),
            );
            json!({})
        } else if called == nav
            || called == enable_page
            || called == enable_net
            || called == bypass
            || called == enable_fetch
            || called == enable_inspector
        {
            json!({})
        } else {
            json!({})
        };
        self.inbox.push_back(reply(result));
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn lag_client(live_url: &str, tree_mode: LagTreeMode, timing: LagInfoTiming) -> (Cdp, std::rc::Rc<std::cell::RefCell<LagState>>) {
    let blank = lag_frame(LAG_BLANK, "L0");
    let live = lag_frame(live_url, "L1");
    let announced = lag_info(LAG_BLANK);
    let protected = lag_info(live_url);
    let (pending_info, deliver_info_after_command, info) = match timing {
        LagInfoTiming::Never => (None, false, announced.clone()),
        LagInfoTiming::BeforeAttach => (None, false, protected.clone()),
        LagInfoTiming::AfterAttachReply => (Some(protected.clone()), false, announced.clone()),
        LagInfoTiming::AfterFirstCommand => (Some(protected.clone()), true, announced.clone()),
    };
    let state = std::rc::Rc::new(std::cell::RefCell::new(LagState {
        info,
        live,
        blank,
        tree_mode,
        tree_reads: 0,
        sessions: std::collections::BTreeMap::new(),
        attaches: 0,
        client_attaches: 0,
        pending_info,
        deliver_info_after_command,
    }));
    let mut cdp = Cdp::from_transport(Box::new(LagBrowser {
        inbox: VecDeque::new(),
        state: state.clone(),
    }));
    cdp.absorb(json!({
        "method": method("Target", "targetCreated"),
        "params": { "targetInfo": announced },
    }));
    if timing == LagInfoTiming::BeforeAttach {
        state.borrow_mut().info = protected.clone();
        cdp.absorb(json!({
            "method": method("Target", "targetInfoChanged"),
            "params": { "targetInfo": protected },
        }));
    }
    (cdp, state)
}

fn lag_assert_no_secret(value: &Value, label: &str) {
    let text = value.to_string();
    assert!(!text.contains(LAG_MARK), "{label}: {text}");
}

fn lag_read_commands() -> Vec<(String, Value)> {
    vec![
        (method("Runtime", "evaluate"), json!({ "expression": "document.documentElement.outerHTML", "returnByValue": true })),
        (method("Runtime", "callFunctionOn"), json!({ "functionDeclaration": "() => document.body.innerText", "objectId": "1", "returnByValue": true })),
        (method("Page", "getFrameTree"), json!({})),
        (method("Page", "getNavigationHistory"), json!({})),
        (method("Page", "getResourceContent"), json!({ "frameId": "T", "url": lag_vault_https() })),
        (method("Page", "captureScreenshot"), json!({})),
        (method("Page", "captureSnapshot"), json!({})),
        (method("Page", "printToPDF"), json!({})),
        (method("DOM", "getDocument"), json!({})),
        (method("Network", "getResponseBody"), json!({ "requestId": "R1" })),
    ]
}

fn lag_attach_outcome(live: &str, tree_mode: LagTreeMode, timing: LagInfoTiming) -> Result<(), String> {
    let label = format!("live={live} tree={tree_mode:?} timing={timing:?}");
    let (mut cdp, state) = lag_client(live, tree_mode, timing);
    let attach = method("Target", "attachToTarget");
    let mark = cdp.events().len();
    let reply = cdp.call(&attach, json!({ "targetId": "T", "flatten": true }), None);
    if !reply.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")) {
        let mut leak = Vec::new();
        if let Ok(value) = &reply {
            if let Some(session) = value["sessionId"].as_str() {
                for (command, params) in lag_read_commands() {
                    match cdp.call(&command, params, Some(session)) {
                        Ok(body) if body.to_string().contains(LAG_MARK) => {
                            leak.push(format!("{command} leaked mark: {body}"));
                        }
                        Ok(body) => {
                            leak.push(format!("{command} Ok {body}"));
                        }
                        Err(message) if message.contains(LAG_MARK) => {
                            leak.push(format!("{command} error leaked mark: {message}"));
                        }
                        Err(message) => {
                            leak.push(format!("{command} Err {}", first_stderr_line(&message).unwrap_or(message.as_str())));
                        }
                    }
                }
                leak.push(format!(
                    "blocked={} protected={}",
                    cdp.session_blocked(session),
                    cdp.is_protected_target("T")
                ));
            }
        }
        return Err(format!("{label}: attach reply {reply:?}; {}", leak.join(" | ")));
    }
    if !cdp.is_protected_target("T") {
        return Err(format!("{label}: target not latched protected"));
    }
    for event in &cdp.events()[mark..] {
        if event["method"] == method("Target", "attachedToTarget") {
            let session = event["params"]["sessionId"].as_str().unwrap_or("");
            if !cdp.guard_sessions.contains(session) {
                return Err(format!("{label}: client-visible attachedToTarget: {event}"));
            }
        }
    }
    let _ = state;
    Ok(())
}

#[test]
fn attach_to_foreign_blank_with_live_vault_document_is_refused_when_target_info_lags() {
    let mut failures = Vec::new();
    for live in lag_protected_lives() {
        for tree_mode in [LagTreeMode::AlwaysLive, LagTreeMode::SecondRead] {
            for timing in [
                LagInfoTiming::Never,
                LagInfoTiming::AfterAttachReply,
                LagInfoTiming::AfterFirstCommand,
            ] {
                if let Err(message) = lag_attach_outcome(&live, tree_mode, timing) {
                    failures.push(message);
                }
            }
        }
    }
    assert!(failures.is_empty(), "lagging attach findings:\n{}", failures.join("\n"));
}

#[test]
fn attach_to_foreign_blank_is_refused_when_target_info_changed_arrives_first() {
    let mut failures = Vec::new();
    for live in lag_protected_lives() {
        for tree_mode in [LagTreeMode::AlwaysLive, LagTreeMode::AlwaysBlank, LagTreeMode::SecondRead] {
            if let Err(message) = lag_attach_outcome(&live, tree_mode, LagInfoTiming::BeforeAttach) {
                failures.push(message);
            }
        }
    }
    assert!(failures.is_empty(), "prompt lagging attach findings:\n{}", failures.join("\n"));
}

#[test]
fn attach_is_refused_when_reported_url_is_protected_while_frame_tree_stays_blank() {
    let mut failures = Vec::new();
    for live in lag_protected_lives() {
        for timing in [LagInfoTiming::BeforeAttach, LagInfoTiming::AfterAttachReply] {
            if let Err(message) = lag_attach_outcome(&live, LagTreeMode::AlwaysBlank, timing) {
                failures.push(message);
            }
        }
    }
    assert!(failures.is_empty(), "reported-url latch findings:\n{}", failures.join("\n"));
}

#[test]
fn mutation_rows_each_attach_gate_is_necessary() {
    let live = lag_wrap("view-source", &lag_vault_https());
    let (mut never_second, _) = lag_client(&live, LagTreeMode::SecondRead, LagInfoTiming::Never);
    let never_reply = never_second.call(
        &method("Target", "attachToTarget"),
        json!({ "targetId": "T", "flatten": true }),
        None,
    );
    assert!(
        never_reply.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")),
        "drop None-path latch would admit SecondRead+Never: {never_reply:?}"
    );
    assert!(
        never_second.is_protected_target("T"),
        "None-path latch must protect when reported url stays blank"
    );
    assert!(
        !never_second.reported_url_protected("T"),
        "SecondRead+Never must not depend on reported-url latch alone"
    );

    let (mut exclusive, _) = lag_client(&live, LagTreeMode::AlwaysBlank, LagInfoTiming::BeforeAttach);
    let exclusive_reply = exclusive.call(
        &method("Target", "attachToTarget"),
        json!({ "targetId": "T", "flatten": true }),
        None,
    );
    assert!(
        exclusive_reply.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")),
        "drop reported-url latch would admit placeable-blank+protected-info: {exclusive_reply:?}"
    );
    assert!(exclusive.is_protected_target("T"));
    assert!(
        exclusive.reported_url_protected("T"),
        "exclusive reported-url cell must still see the protected reported url"
    );
    assert!(
        exclusive.documents.get("T").is_none_or(|doc| !doc.previous || exclusive.is_protected_target("T")),
        "reported-url latch must be what keeps the placeable blank from proving"
    );

    let (mut ordinary, state) = lag_client(
        "https://ordinary.example/doc",
        LagTreeMode::AlwaysLive,
        LagInfoTiming::Never,
    );
    {
        let mut state = state.borrow_mut();
        state.info = lag_info("https://ordinary.example/doc");
        state.live = lag_frame("https://ordinary.example/doc", "L1");
        state.blank = lag_frame("https://ordinary.example/doc", "L1");
    }
    let ordinary_reply = ordinary
        .call(
            &method("Target", "attachToTarget"),
            json!({ "targetId": "T", "flatten": true }),
            None,
        )
        .expect("over-block ordinary would refuse a proven https document");
    let session = ordinary_reply["sessionId"].as_str().unwrap();
    assert!(!ordinary.is_protected_target("T"));
    assert!(!ordinary.session_blocked(session));
    assert!(ordinary.attach_document_proven("T"));
}

#[test]
fn ordinary_blank_and_https_foreign_tabs_still_attach() {
    for live in [
        LAG_BLANK.to_string(),
        "https://ordinary.example/doc".to_string(),
        "http://127.0.0.1:9/ok".to_string(),
    ] {
        let (mut cdp, state) = lag_client(&live, LagTreeMode::AlwaysLive, LagInfoTiming::Never);
        {
            let mut state = state.borrow_mut();
            state.info = lag_info(&live);
            state.live = lag_frame(&live, "L1");
            state.blank = lag_frame(&live, "L1");
        }
        let attach = method("Target", "attachToTarget");
        let reply = cdp
            .call(&attach, json!({ "targetId": "T", "flatten": true }), None)
            .unwrap_or_else(|err| panic!("ordinary attach refused for {live}: {err}"));
        let session = reply["sessionId"].as_str().unwrap();
        assert!(!cdp.is_protected_target("T"), "{live}");
        assert!(!cdp.session_blocked(session), "{live}");
    }
}

#[test]
fn chrome_scheme_exceptions_and_opaque_pages_still_attach() {
    for live in [
        "chrome://new-tab-page/",
        "chrome://new-tab-page",
        "chrome-untrusted://new-tab-page/",
    ] {
        assert!(!super::Protected::default().url(live), "matcher must admit {live}");
        assert!(super::Protected::default().opaque_document(live, true), "{live}");
        let (mut cdp, state) = lag_client(live, LagTreeMode::AlwaysLive, LagInfoTiming::Never);
        {
            let mut state = state.borrow_mut();
            state.info = lag_info(live);
            state.live = lag_frame(live, "L1");
            state.blank = lag_frame(live, "L1");
        }
        let reply = cdp
            .call(
                &method("Target", "attachToTarget"),
                json!({ "targetId": "T", "flatten": true }),
                None,
            )
            .unwrap_or_else(|err| panic!("chromeUrlsAttach control refused for {live}: {err}"));
        let session = reply["sessionId"].as_str().unwrap();
        assert!(!cdp.is_protected_target("T"), "{live}");
        assert!(!cdp.session_blocked(session), "{live}");
        let evaluate = cdp.call(
            &method("Runtime", "evaluate"),
            json!({ "expression": "location.href", "returnByValue": true }),
            Some(session),
        );
        assert!(evaluate.is_ok(), "{live}: {evaluate:?}");
    }
}

#[test]
fn protected_chrome_settings_extension_and_file_roots_refuse_and_latch() {
    for live in [
        "chrome://settings/",
        "chrome-extension://mhjfbmdgcfjbbpaeojofohoefgiehjai/index.html",
        "file:///",
    ] {
        assert!(super::Protected::default().url(live), "matcher must protect {live}");
        let (mut cdp, state) = lag_client(live, LagTreeMode::AlwaysLive, LagInfoTiming::Never);
        {
            let mut state = state.borrow_mut();
            state.info = lag_info(live);
            state.live = lag_frame(live, "L1");
            state.blank = lag_frame(live, "L1");
        }
        let reply = cdp.call(
            &method("Target", "attachToTarget"),
            json!({ "targetId": "T", "flatten": true }),
            None,
        );
        assert!(
            reply.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")),
            "{live}: {reply:?}"
        );
        assert!(cdp.is_protected_target("T"), "{live}");
    }
}

#[test]
fn blank_then_timeout_content_read_refuses_without_latching_and_retry_can_prove() {
    let live = lag_wrap("view-source", &lag_vault_https());
    let (mut cdp, state) = lag_client(&live, LagTreeMode::BlankThenTimeout, LagInfoTiming::Never);
    let first = cdp.call(
        &method("Target", "attachToTarget"),
        json!({ "targetId": "T", "flatten": true }),
        None,
    );
    assert!(
        first.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")),
        "S1 blank-then-timeout must refuse: {first:?}"
    );
    assert!(
        !cdp.is_protected_target("T"),
        "O2: no-proof timeout must not latch protected"
    );
    {
        let mut state = state.borrow_mut();
        state.tree_mode = LagTreeMode::AlwaysLive;
        state.tree_reads = 0;
        state.info = lag_info("https://ordinary.example/doc");
        state.live = lag_frame("https://ordinary.example/doc", "L9");
        state.blank = lag_frame("https://ordinary.example/doc", "L9");
        state.sessions.clear();
    }
    cdp.absorb(json!({
        "method": method("Target", "targetInfoChanged"),
        "params": { "targetInfo": lag_info("https://ordinary.example/doc") },
    }));
    let second = cdp
        .call(
            &method("Target", "attachToTarget"),
            json!({ "targetId": "T", "flatten": true }),
            None,
        )
        .expect("retry after no-proof refuse must admit a healthy ordinary tree");
    assert!(!cdp.is_protected_target("T"));
    assert!(!cdp.session_blocked(second["sessionId"].as_str().unwrap()));
}

#[test]
fn always_blank_with_never_timing_admits_only_when_blank_is_the_proven_document() {
    let (mut cdp, state) = lag_client(LAG_BLANK, LagTreeMode::AlwaysBlank, LagInfoTiming::Never);
    {
        let mut state = state.borrow_mut();
        state.info = lag_info(LAG_BLANK);
        state.live = lag_frame(LAG_BLANK, "L1");
        state.blank = lag_frame(LAG_BLANK, "L0");
    }
    let reply = cdp
        .call(
            &method("Target", "attachToTarget"),
            json!({ "targetId": "T", "flatten": true }),
            None,
        )
        .expect("AlwaysBlank+Never with blank reported must prove ordinary blank");
    assert!(!cdp.is_protected_target("T"));
    assert!(!cdp.session_blocked(reply["sessionId"].as_str().unwrap()));
}

#[test]
fn positive_proof_requirement_refuses_absence_of_protected_shapes() {
    let live = lag_wrap("view-source", &lag_vault_https());
    let (mut timeout, _) = lag_client(&live, LagTreeMode::BlankThenTimeout, LagInfoTiming::Never);
    let timeout_reply = timeout.call(
        &method("Target", "attachToTarget"),
        json!({ "targetId": "T", "flatten": true }),
        None,
    );
    assert!(timeout_reply.is_err(), "{timeout_reply:?}");
    assert!(!timeout.is_protected_target("T"));
    assert!(!timeout.attach_document_proven("T"));
    assert!(!timeout.attach_content_ok.contains("T"));

    let (mut empty, state) = lag_client(&live, LagTreeMode::AlwaysLive, LagInfoTiming::Never);
    {
        let mut state = state.borrow_mut();
        state.tree_mode = LagTreeMode::BlankThenTimeout;
        state.tree_reads = 1;
    }
    empty.attach_content_ok.clear();
    empty.documents.remove("T");
    assert!(!empty.attach_document_proven("T"), "empty documents is not positive proof");
}

#[test]
fn an_existing_blank_session_is_cut_when_the_document_navigates_to_vault() {
    for live in [lag_vault_https(), lag_wrap("view-source", &lag_vault_https())] {
        let blank = lag_frame(LAG_BLANK, "L0");
        let announced = lag_info(LAG_BLANK);
        let state = std::rc::Rc::new(std::cell::RefCell::new(LagState {
            info: announced.clone(),
            live: blank.clone(),
            blank: blank.clone(),
            tree_mode: LagTreeMode::AlwaysLive,
            tree_reads: 0,
            sessions: std::collections::BTreeMap::new(),
            attaches: 0,
            client_attaches: 0,
            pending_info: None,
            deliver_info_after_command: false,
        }));
        let mut cdp = Cdp::from_transport(Box::new(LagBrowser {
            inbox: VecDeque::new(),
            state: state.clone(),
        }));
        cdp.absorb(json!({
            "method": method("Target", "targetCreated"),
            "params": { "targetInfo": announced },
        }));
        let attach = method("Target", "attachToTarget");
        let evaluate = method("Runtime", "evaluate");
        let reply = cdp
            .call(&attach, json!({ "targetId": "T", "flatten": true }), None)
            .expect("blank attach");
        let session = reply["sessionId"].as_str().unwrap().to_string();
        assert!(!cdp.is_protected_target("T"));
        assert!(!cdp.session_blocked(&session));
        let guard = cdp.guards.get("T").cloned().expect("guard");
        let protected_info = lag_info(&live);
        let protected_frame = lag_frame(&live, "L2");
        {
            let mut state = state.borrow_mut();
            state.info = protected_info.clone();
            state.live = protected_frame.clone();
        }
        cdp.absorb(json!({
            "method": method("Page", "frameStartedNavigating"),
            "sessionId": guard,
            "params": {
                "frameId": "T",
                "url": live,
                "navigationType": "differentDocument",
            },
        }));
        cdp.absorb(json!({
            "method": method("Page", "frameNavigated"),
            "sessionId": guard,
            "params": { "frame": protected_frame, "type": "Navigation" },
        }));
        cdp.absorb(json!({
            "method": method("Target", "targetInfoChanged"),
            "params": { "targetInfo": protected_info },
        }));
        assert!(cdp.is_protected_target("T"), "live={live}");
        assert!(cdp.session_blocked(&session), "live={live}");
        let out = cdp.call(
            &evaluate,
            json!({ "expression": "document.body.innerHTML", "returnByValue": true }),
            Some(&session),
        );
        assert!(out.is_err(), "live={live}: {out:?}");
        if let Err(message) = out {
            assert!(!message.contains(LAG_MARK), "live={live}: {message}");
        }
        for (command, params) in lag_read_commands() {
            let out = cdp.call(&command, params, Some(&session));
            assert!(out.is_err(), "live={live}: {command} => {out:?}");
            if let Err(message) = out {
                assert!(!message.contains(LAG_MARK), "live={live}: {command}: {message}");
            }
        }
    }
}

#[test]
fn an_existing_blank_session_is_cut_when_only_frame_commit_shows_vault_before_target_info() {
    let live = lag_vault_https();
    let blank = lag_frame(LAG_BLANK, "L0");
    let announced = lag_info(LAG_BLANK);
    let state = std::rc::Rc::new(std::cell::RefCell::new(LagState {
        info: announced.clone(),
        live: blank.clone(),
        blank: blank.clone(),
        tree_mode: LagTreeMode::AlwaysLive,
        tree_reads: 0,
        sessions: std::collections::BTreeMap::new(),
        attaches: 0,
        client_attaches: 0,
        pending_info: None,
        deliver_info_after_command: false,
    }));
    let mut cdp = Cdp::from_transport(Box::new(LagBrowser {
        inbox: VecDeque::new(),
        state: state.clone(),
    }));
    cdp.absorb(json!({
        "method": method("Target", "targetCreated"),
        "params": { "targetInfo": announced },
    }));
    let attach = method("Target", "attachToTarget");
    let evaluate = method("Runtime", "evaluate");
    let reply = cdp
        .call(&attach, json!({ "targetId": "T", "flatten": true }), None)
        .expect("blank attach");
    let session = reply["sessionId"].as_str().unwrap().to_string();
    let guard = cdp.guards.get("T").cloned().expect("guard");
    let protected_frame = lag_frame(&live, "L2");
    {
        let mut state = state.borrow_mut();
        state.live = protected_frame.clone();
    }
    cdp.absorb(json!({
        "method": method("Page", "frameNavigated"),
        "sessionId": guard,
        "params": { "frame": protected_frame, "type": "Navigation" },
    }));
    assert!(cdp.is_protected_target("T"));
    assert!(cdp.session_blocked(&session));
    let out = cdp.call(
        &evaluate,
        json!({ "expression": "1", "returnByValue": true }),
        Some(&session),
    );
    assert!(out.is_err(), "{out:?}");
    lag_assert_no_secret(&json!(out.err()), "frame-commit-first");
}
