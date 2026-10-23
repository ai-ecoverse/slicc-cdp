use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct PersistentState {
    installed: bool,
    executions: usize,
    sent: Vec<Value>,
    inbox: VecDeque<String>,
}

struct PersistentBackend(Rc<RefCell<PersistentState>>);

impl Transport for PersistentBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let command: Value = serde_json::from_str(text).unwrap();
        let mut state = self.0.borrow_mut();
        state.sent.push(command.clone());
        match command["method"].as_str().unwrap_or("") {
            "Page.addScriptToEvaluateOnNewDocument" => state.installed = true,
            "Page.removeScriptToEvaluateOnNewDocument" | "Target.detachFromTarget" => state.installed = false,
            _ => {}
        }
        state.inbox.push_back(json!({"id":command["id"],"result":{"identifier":"INIT"}}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.0.borrow_mut().inbox.pop_front())
    }
}

fn persistent_client() -> (Cdp, Rc<RefCell<PersistentState>>) {
    let state = Rc::new(RefCell::new(PersistentState::default()));
    let mut cdp = Cdp::from_transport(Box::new(PersistentBackend(state.clone())));
    cdp.protect_origins(&["https://catalog.ordinary.test"]);
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":"https://ordinary.test/"}}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"https://ordinary.test/","securityOrigin":"https://ordinary.test"}}}));
    (cdp, state)
}

fn persistent_commit(cdp: &mut Cdp, state: &Rc<RefCell<PersistentState>>, url: &str, variant: &str) {
    {
        let mut backend = state.borrow_mut();
        if backend.installed {
            backend.executions += 1;
        }
    }
    let mut frame = json!({"id":"F","url":url,"securityOrigin":url});
    if variant == "child" {
        frame["id"] = json!("CHILD");
        frame["parentId"] = json!("F");
    }
    if variant == "error" {
        frame["url"] = json!("chrome-error://chromewebdata/");
        frame["unreachableUrl"] = json!(url);
    }
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":frame,"type":if variant == "bfcache" {"BackForwardCacheRestore"} else {"Navigation"}}}));
}

fn persistent_variant(variant: &str) {
    for url in ["https://seven.sliccy.ai/", "https://vault.sliccy.ai/", "https://catalog.ordinary.test/"] {
        let (mut cdp, state) = persistent_client();
        assert!(cdp.guard().url(url));
        let installed = cdp.call("Page.addScriptToEvaluateOnNewDocument", json!({"source":"globalThis.injected = true"}), Some("S"));
        if variant == "commit-before-attach" {
            cdp.session_targets.remove("S");
        }
        persistent_commit(&mut cdp, &state, url, variant);
        assert_eq!(state.borrow().executions, 0, "{variant}: persistent script executed before the guard observed the commit");
        assert!(installed.is_err(), "unguarded persistent script was admitted");
        assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Page.addScriptToEvaluateOnNewDocument"));
    }
}

#[test]
fn persistent_navigation_is_refused_before_install() { persistent_variant("navigation"); }

#[test]
fn persistent_history_is_refused_before_install() { persistent_variant("history"); }

#[test]
fn persistent_bfcache_is_refused_before_install() { persistent_variant("bfcache"); }

#[test]
fn persistent_child_is_refused_before_install() { persistent_variant("child"); }

#[test]
fn persistent_error_is_refused_before_install() { persistent_variant("error"); }

#[test]
fn persistent_commit_before_attach_is_refused_before_install() { persistent_variant("commit-before-attach"); }

#[test]
fn persistent_capabilities_are_refused_for_calls_and_notifications() {
    let capabilities = [
        ("Page.addScriptToEvaluateOnNewDocument", json!({"source":"globalThis.injected = true","runImmediately":true})),
        ("Page.addScriptToEvaluateOnLoad", json!({"scriptSource":"globalThis.injected = true"})),
        ("Page.setBypassCSP", json!({"enabled":true})),
        ("Page.setDownloadBehavior", json!({"behavior":"allow","downloadPath":"/tmp"})),
        ("Page.startScreencast", json!({})),
        ("Page.setWebLifecycleState", json!({"state":"frozen"})),
        ("Network.setExtraHTTPHeaders", json!({"headers":{"X-Control":"changed"}})),
        ("Network.setUserAgentOverride", json!({"userAgent":"changed"})),
        ("Network.setBypassServiceWorker", json!({"bypass":true})),
        ("Network.setCacheDisabled", json!({"cacheDisabled":true})),
        ("Network.emulateNetworkConditions", json!({"offline":true,"latency":0,"downloadThroughput":0,"uploadThroughput":0})),
        ("Emulation.setScriptExecutionDisabled", json!({"value":true})),
        ("Emulation.setVirtualTimePolicy", json!({"policy":"pause"})),
        ("Emulation.setGeolocationOverride", json!({"latitude":1,"longitude":1,"accuracy":1})),
        ("Input.setIgnoreInputEvents", json!({"ignore":true})),
        ("Security.setIgnoreCertificateErrors", json!({"ignore":true})),
        ("Debugger.setInstrumentationBreakpoint", json!({"instrumentation":"beforeScriptExecution"})),
        ("Debugger.setBreakpointByUrl", json!({"lineNumber":0,"urlRegex":".*"})),
        ("Runtime.setCustomObjectFormatterEnabled", json!({"enabled":true})),
        ("Overlay.setShowPaintRects", json!({"result":true})),
        ("Browser.setDownloadBehavior", json!({"behavior":"allow","downloadPath":"/tmp"})),
        ("Profiler.start", json!({})),
        ("HeapProfiler.startTrackingHeapObjects", json!({})),
        ("Target.setAutoAttach", json!({"autoAttach":true,"flatten":true,"waitForDebuggerOnStart":true})),
    ];
    for (method, params) in capabilities {
        let (mut cdp, state) = persistent_client();
        assert!(cdp.call(method, params.clone(), Some("S")).unwrap_err().starts_with("slicc: refused:"), "{method}");
        assert!(cdp.notify(method, params, Some("S")).unwrap_err().starts_with("slicc: refused:"), "{method}");
        assert!(!state.borrow().sent.iter().any(|command| command["method"] == method), "{method} reached the transport");
    }
}

#[test]
fn scoped_bindings_are_refused_even_in_a_verified_page_world() {
    let (mut cdp, state) = persistent_client();
    cdp.worlds.enable("S");
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"S","params":{"context":{"id":1,"uniqueId":"ordinary-context","name":"page","origin":"https://ordinary.test","auxData":{"isDefault":true,"frameId":"F"}}}}));
    for selector in [json!({"executionContextId":1}), json!({"executionContextName":"page"})] {
        let mut params = selector;
        params["name"] = json!("persistentBinding");
        assert!(cdp.call("Runtime.addBinding", params.clone(), Some("S")).unwrap_err().contains("persistent page capabilities"));
        assert!(cdp.notify("Runtime.addBinding", params, Some("S")).unwrap_err().contains("persistent page capabilities"));
    }
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Runtime.addBinding"));
}

fn network_state_params(offline: bool) -> Value {
    json!({"offline":offline,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1})
}

fn network_state_sent(state: &Rc<RefCell<PersistentState>>) -> Vec<Value> {
    state.borrow().sent.iter().filter(|command| command["method"] == "Network.emulateNetworkConditions").cloned().collect()
}

#[test]
fn network_state_admits_only_the_offline_toggle_on_an_ordinary_page_session() {
    for offline in [true, false] {
        let (mut cdp, state) = persistent_client();
        cdp.call("Network.emulateNetworkConditions", network_state_params(offline), Some("S")).unwrap();
        let sent = network_state_sent(&state);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["params"], network_state_params(offline));
        assert_eq!(sent[0]["sessionId"], "S");
    }
}

#[test]
fn network_state_refuses_every_other_shape_and_condition_method() {
    let shapes = [
        json!({"offline":true,"latency":0,"downloadThroughput":0,"uploadThroughput":0}),
        json!({"offline":true,"latency":5,"downloadThroughput":-1,"uploadThroughput":-1}),
        json!({"offline":true,"latency":0.0,"downloadThroughput":-1,"uploadThroughput":-1}),
        json!({"offline":true,"latency":0,"downloadThroughput":-1.0,"uploadThroughput":-1}),
        json!({"offline":"true","latency":0,"downloadThroughput":-1,"uploadThroughput":-1}),
        json!({"offline":null,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1}),
        json!({"offline":true,"latency":0,"downloadThroughput":-1}),
        json!({"latency":0,"downloadThroughput":-1,"uploadThroughput":-1,"connectionType":"none"}),
        json!({"offline":false,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1,"connectionType":"wifi"}),
        json!({"offline":false,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1,"packetLoss":100}),
        json!([]),
        json!(null),
    ];
    for params in shapes {
        let (mut cdp, state) = persistent_client();
        assert!(cdp.call("Network.emulateNetworkConditions", params.clone(), Some("S")).unwrap_err().contains("persistent page capabilities"), "{params}");
        assert!(cdp.notify("Network.emulateNetworkConditions", params.clone(), Some("S")).unwrap_err().contains("persistent page capabilities"), "{params}");
        assert!(network_state_sent(&state).is_empty(), "{params}");
    }
    for method in ["Network.emulateNetworkConditionsByRule", "Network.overrideNetworkState", "Network.setBlockedURLs", "Network.setBypassServiceWorker"] {
        let (mut cdp, state) = persistent_client();
        assert!(cdp.call(method, network_state_params(true), Some("S")).unwrap_err().starts_with("slicc: refused:"), "{method}");
        assert!(!state.borrow().sent.iter().any(|command| command["method"] == method), "{method}");
    }
}

#[test]
fn network_state_is_refused_off_an_ordinary_page_session() {
    let (mut cdp, state) = persistent_client();
    let method = "Network.emulateNetworkConditions";
    assert!(cdp.call(method, network_state_params(true), None).is_err());
    assert!(cdp.call("Network.emulateNetworkConditions", network_state_params(true), Some("UNKNOWN")).is_err());
    cdp.guard_sessions.insert("G".into());
    cdp.session_targets.insert("G".into(), "T".into());
    assert!(cdp.call("Network.emulateNetworkConditions", network_state_params(true), Some("G")).is_err());
    cdp.dialog_sessions.insert("D".into());
    cdp.session_targets.insert("D".into(), "T".into());
    assert!(cdp.call("Network.emulateNetworkConditions", network_state_params(true), Some("D")).is_err());
    for kind in ["iframe", "worker", "service_worker", "shared_worker", "other"] {
        let target = format!("T-{kind}");
        let session = format!("S-{kind}");
        cdp.session_targets.insert(session.clone(), target.clone());
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":target,"type":kind,"url":"https://ordinary.test/"}}}));
        assert!(cdp.call("Network.emulateNetworkConditions", network_state_params(true), Some(&session)).is_err(), "{kind}");
    }
    assert!(network_state_sent(&state).is_empty());
    for url in ["https://seven.sliccy.ai/", "https://vault.sliccy.ai/", "https://catalog.ordinary.test/"] {
        let (mut cdp, state) = persistent_client();
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":url}}}));
        assert!(cdp.call("Network.emulateNetworkConditions", network_state_params(true), Some("S")).is_err(), "{url}");
        assert!(network_state_sent(&state).is_empty(), "{url}");
    }
    for mark in ["protected", "abandoned", "blocked"] {
        let (mut cdp, state) = persistent_client();
        match mark {
            "protected" => { cdp.protected_targets.insert("T".into()); }
            "abandoned" => { cdp.abandoned.insert("T".into()); }
            _ => { cdp.blocked.insert("S".into()); }
        }
        assert!(cdp.call("Network.emulateNetworkConditions", network_state_params(false), Some("S")).is_err(), "{mark}");
        assert!(network_state_sent(&state).is_empty(), "{mark}");
    }
    let (mut cdp, state) = persistent_client();
    cdp.targets.remove("T");
    assert!(cdp.call("Network.emulateNetworkConditions", network_state_params(true), Some("S")).is_err());
    assert!(network_state_sent(&state).is_empty());
}

#[test]
fn network_state_cannot_reach_a_page_after_a_protected_commit() {
    for url in ["https://seven.sliccy.ai/", "https://vault.sliccy.ai/"] {
        let (mut cdp, state) = persistent_client();
        persistent_commit(&mut cdp, &state, url, "navigation");
        assert!(cdp.call("Network.emulateNetworkConditions", network_state_params(true), Some("S")).is_err(), "{url}");
        assert!(network_state_sent(&state).is_empty(), "{url}");
    }
}
