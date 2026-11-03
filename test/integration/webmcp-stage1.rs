use cdp_client::cdp::{Cdp, Transport};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    sessions: BTreeMap<String, String>,
    calls: Vec<Value>,
    incoming: VecDeque<String>,
    armed: Option<String>,
    tree: Option<Value>,
    changed: Option<Value>,
    published: bool,
    refusal: Option<&'static str>,
    dom_refusal: bool,
    tree_refusal: bool,
    pending_child: bool,
    pending_reads: usize,
    load_sent: bool,
    incomplete: bool,
    protected_world: bool,
    missing_instrumentation: bool,
    late_world: bool,
    raw_instrumentation: bool,
    conversion_pending: bool,
    child_churn: bool,
    replay_only: bool,
    load_after_install: bool,
    pause_during_install: bool,
    disable_after_reload_refused: bool,
    fresh_replay_missing: bool,
    status_original_refused: bool,
}

struct Backend(Rc<RefCell<State>>);

fn tree(loader: &str) -> Value {
    json!({"frameTree":{"frame":{"id":"F","loaderId":loader,"url":"https://ordinary.test/","securityOrigin":"https://ordinary.test"}}})
}

fn context(session: &str, loader: &str) -> Value {
    json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":1,"uniqueId":format!("main-{loader}"),"name":"","origin":"https://ordinary.test","auxData":{"frameId":"F","isDefault":true}}}})
}

impl Transport for Backend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut state = self.0.borrow_mut();
        state.calls.push(call.clone());
        let session = call["sessionId"].as_str().unwrap_or("").to_string();
        let method = call["method"].as_str().unwrap();
        let refusal = match method {
            "Debugger.enable" => state.refusal,
            "Runtime.disable" if state.disable_after_reload_refused && state.sessions.keys().next().is_some_and(|holder| holder == &session) && state.tree.as_ref().is_some_and(|tree| tree["frameTree"]["frame"]["loaderId"] == "R") => Some("slicc: refused: Runtime.disable is unavailable"),
            "Runtime.evaluate" if state.status_original_refused && call["params"]["expression"] == "typeof document.modelContext" && state.sessions.keys().next().is_some_and(|holder| holder == &session) => Some("slicc: refused: that execution context belongs to a browser extension"),
            "DOM.getDocument" if state.dom_refusal => Some("slicc: refused: protected frame"),
            "Page.getFrameTree" if state.tree_refusal => Some("slicc: refused: protected frame"),
            _ => None,
        };
        if let Some(message) = refusal {
            state.incoming.push_back(
                json!({"id":call["id"],"error":{"code":-32000,"message":message}}).to_string(),
            );
            return Ok(());
        }
        let result = match method {
            "Target.getTargets" => {
                json!({"targetInfos":[{"targetId":"T","type":"page","url":"https://ordinary.test/"}]})
            }
            "Target.getTargetInfo" => {
                json!({"targetInfo":{"targetId":"T","type":"page","url":"https://ordinary.test/"}})
            }
            "Target.attachToTarget" => {
                let session = format!("S{}", state.sessions.len() + 1);
                state.sessions.insert(session.clone(), "T".into());
                json!({"sessionId":session})
            }
            "Page.getFrameTree" => {
                if state.published {
                    if let Some(next) = state.changed.take() {
                        state.tree = Some(next.clone());
                        for owner in state.sessions.keys().cloned().collect::<Vec<_>>() {
                            state.incoming.push_back(json!({"method":"Page.frameNavigated","sessionId":owner,"params":{"frame":next["frameTree"]["frame"]}}).to_string());
                            state.incoming.push_back(json!({"method":"Runtime.executionContextsCleared","sessionId":owner,"params":{}}).to_string());
                        }
                    }
                }
                let mut current = state.tree.clone().unwrap_or_else(|| tree("L"));
                if state.incomplete { current["frameTree"]["childFrames"] = json!({}); }
                if state.pending_child && current["frameTree"]["frame"]["loaderId"] == "R" {
                    assert!(state.load_sent);
                    state.pending_reads += 1;
                    current["frameTree"]["childFrames"] = json!([{"frame":{"id":"C","loaderId":"CHILD","url":if state.pending_reads == 1 { "" } else { "https://ordinary.test/child" },"securityOrigin":"https://ordinary.test"}}]);
                }
                current
            }
            "Runtime.enable" => {
                let loader = state
                    .tree
                    .as_ref()
                    .and_then(|tree| tree.pointer("/frameTree/frame/loaderId"))
                    .and_then(Value::as_str)
                    .unwrap_or("L")
                    .to_string();
                if !state.fresh_replay_missing || state.tree.as_ref().is_none_or(|tree| tree["frameTree"]["frame"]["loaderId"] != "R") {
                    state.incoming.push_back(context(&session, &loader).to_string());
                }
                if state.protected_world {
                    state.incoming.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":2,"uniqueId":"protected-world","origin":"https://seven.sliccy.ai","auxData":{"frameId":"F","isDefault":false}}}}).to_string());
                }
                json!({})
            }
            "DOM.getDocument" => {
                if state.incomplete {
                    json!({"root":{"nodeId":1,"nodeType":9,"childNodeCount":1}})
                } else {
                    json!({"root":{"nodeId":1,"nodeType":9,"childNodeCount":1,"children":[{"nodeId":2,"nodeType":1,"nodeName":"IFRAME","frameId":"ORDINARY-CHILD","childNodeCount":0}]}})
                }
            }
            "Debugger.setInstrumentationBreakpoint" => {
                assert_eq!(call["params"]["instrumentation"], "beforeScriptExecution");
                state.armed = Some(session.clone());
                if state.late_world {
                    state.incoming.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":3,"uniqueId":"late-extension","origin":"chrome-extension://other","auxData":{"frameId":"F","isDefault":false}}}}).to_string());
                }
                if state.missing_instrumentation {
                    json!({})
                } else {
                    json!({"breakpointId":"INSTRUMENT"})
                }
            }
            "Debugger.pause" => {
                assert!(state.raw_instrumentation && state.armed.as_deref() == Some(&session));
                state.conversion_pending = true;
                json!({})
            }
            "Debugger.resume" if state.conversion_pending => {
                state.conversion_pending = false;
                state.incoming.push_back(json!({"method":"Debugger.paused","sessionId":session,"params":{"reason":"other","callFrames":[{"callFrameId":"CALL","functionName":"","location":{"scriptId":"SCRIPT","lineNumber":0,"columnNumber":0}}]}}).to_string());
                json!({})
            }
            "Debugger.disable" => {
                state.armed = None;
                json!({})
            }
            "Page.reload" => {
                state.tree = Some(tree("R"));
                if state.child_churn {
                    state.tree.as_mut().unwrap()["frameTree"]["childFrames"] = json!([{"frame":{"id":"C","parentId":"F","loaderId":"CHILD","url":"https://child.test/","securityOrigin":"https://child.test"}}]);
                }
                for owner in state.sessions.keys().cloned().collect::<Vec<_>>() {
                    state.incoming.push_back(json!({"method":"Page.frameNavigated","sessionId":owner,"params":{"frame":tree("R")["frameTree"]["frame"]}}).to_string());
                    state.incoming.push_back(json!({"method":"Runtime.executionContextsCleared","sessionId":owner,"params":{}}).to_string());
                    if !state.replay_only { state.incoming.push_back(context(&owner, "R").to_string()); }
                    if !state.load_after_install { state.incoming.push_back(json!({"method":"Page.loadEventFired","sessionId":owner,"params":{"timestamp":1}}).to_string()); }
                }
                state.load_sent = true;
                if let Some(owner) = state.armed.clone() {
                    state.incoming.push_back(json!({"method":"Debugger.scriptParsed","sessionId":owner,"params":{"scriptId":"SCRIPT","executionContextId":1,"url":"https://ordinary.test/first.js"}}).to_string());
                    let reason = if state.raw_instrumentation {
                        "instrumentation"
                    } else {
                        "other"
                    };
                    state.incoming.push_back(json!({"method":"Debugger.paused","sessionId":owner,"params":{"reason":reason,"callFrames":[{"callFrameId":"CALL","functionName":"","location":{"scriptId":"SCRIPT","lineNumber":0,"columnNumber":0}}]}}).to_string());
                }
                json!({})
            }
            "Runtime.evaluate" => {
                let loader = state
                    .tree
                    .as_ref()
                    .and_then(|tree| tree.pointer("/frameTree/frame/loaderId"))
                    .and_then(Value::as_str)
                    .unwrap_or("L");
                assert_eq!(call["params"]["uniqueContextId"], format!("main-{loader}"));
                if state.pause_during_install {
                    state.pause_during_install = false;
                    state.incoming.push_back(json!({"method":"Debugger.scriptParsed","sessionId":session,"params":{"scriptId":"INSTALL","executionContextId":1,"url":""}}).to_string());
                    state.incoming.push_back(json!({"method":"Debugger.paused","sessionId":session,"params":{"reason":"other","callFrames":[{"callFrameId":"INSTALL-CALL","location":{"scriptId":"INSTALL","lineNumber":0,"columnNumber":0}}]}}).to_string());
                }
                if state.load_after_install {
                    state.load_after_install = false;
                    for owner in state.sessions.keys().cloned().collect::<Vec<_>>() {
                        state.incoming.push_back(json!({"method":"Page.loadEventFired","sessionId":owner,"params":{"timestamp":1}}).to_string());
                    }
                }
                if state.child_churn {
                    let child = json!({"id":"C","parentId":"F","loaderId":"CHURNED","url":"https://child.test/","securityOrigin":"https://child.test"});
                    state.tree.as_mut().unwrap()["frameTree"]["childFrames"][0]["frame"] =
                        child.clone();
                    for owner in state.sessions.keys().cloned().collect::<Vec<_>>() {
                        state.incoming.push_back(json!({"method":"Page.frameNavigated","sessionId":owner,"params":{"frame":child}}).to_string());
                    }
                }
                json!({"result":{"type":"undefined"}})
            }
            _ => json!({}),
        };
        state
            .incoming
            .push_back(json!({"id":call["id"],"result":result}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        let mut state = self.0.borrow_mut();
        let text = state.incoming.pop_front();
        if text.as_ref().is_some_and(|text| {
            serde_json::from_str::<Value>(text).unwrap()["method"] == "Debugger.paused"
        }) {
            state.published = true;
        }
        Ok(text)
    }
}

struct EvaluateReadWire {
    base: Backend,
    waiting_read: bool,
    read_conversions: Rc<RefCell<usize>>,
}

impl Transport for EvaluateReadWire {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        if call["method"] == "Page.reload" {
            let mut state = self.base.0.borrow_mut();
            state.calls.push(call.clone());
            state.tree = Some(tree("R"));
            for session in state.sessions.keys().cloned().collect::<Vec<_>>() {
                state.incoming.push_back(json!({"method":"Page.frameNavigated","sessionId":session,"params":{"frame":tree("R")["frameTree"]["frame"]}}).to_string());
                state.incoming.push_back(json!({"method":"Runtime.executionContextsCleared","sessionId":session,"params":{}}).to_string());
                state.incoming.push_back(context(&session, "R").to_string());
            }
            let owner = state.armed.clone().unwrap();
            state.incoming.push_back(json!({"method":"Debugger.scriptParsed","sessionId":owner,"params":{"scriptId":"READ","executionContextId":1,"url":"","embedderName":""}}).to_string());
            state.incoming.push_back(json!({"method":"Debugger.paused","sessionId":owner,"params":{"reason":"instrumentation","data":{"scriptId":"READ"},"callFrames":[{"callFrameId":"READ-CALL","location":{"scriptId":"READ","lineNumber":0,"columnNumber":0}}]}}).to_string());
            state.incoming.push_back(json!({"id":call["id"],"result":{}}).to_string());
            self.waiting_read = true;
            return Ok(());
        }
        if call["method"] == "Debugger.pause" && self.waiting_read { *self.read_conversions.borrow_mut() += 1; }
        let page_script = call["method"] == "Debugger.resume" && self.waiting_read && !self.base.0.borrow().conversion_pending;
        self.base.send_text(text)?;
        if page_script {
            self.waiting_read = false;
            let mut state = self.base.0.borrow_mut();
            let owner = state.armed.clone().unwrap();
            state.incoming.push_back(json!({"method":"Debugger.scriptParsed","sessionId":owner,"params":{"scriptId":"SCRIPT","executionContextId":1,"url":"https://ordinary.test/first.js","embedderName":"https://ordinary.test/first.js"}}).to_string());
            state.incoming.push_back(json!({"method":"Debugger.paused","sessionId":owner,"params":{"reason":"instrumentation","data":{"scriptId":"SCRIPT"},"callFrames":[{"callFrameId":"CALL","location":{"scriptId":"SCRIPT","lineNumber":0,"columnNumber":0}}]}}).to_string());
        }
        Ok(())
    }

    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
        self.base.recv_text(deadline)
    }
}

#[test]
fn direct_owned_evaluate_pause_does_not_convert_before_the_page_script() {
    let state = Rc::new(RefCell::new(State { raw_instrumentation: true, ..Default::default() }));
    let read_conversions = Rc::new(RefCell::new(0));
    let mut cdp = Cdp::from_transport(Box::new(EvaluateReadWire { base: Backend(state.clone()), waiting_read: false, read_conversions: read_conversions.clone() }));
    let session = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap()["sessionId"].as_str().unwrap().to_string();
    cdp.start_webmcp_for(&session, true).unwrap();
    assert_eq!(*read_conversions.borrow(), 0);
    let wire = state.borrow();
    assert_eq!(count(&wire, "Debugger.pause"), 1);
    assert_eq!(count(&wire, "Runtime.evaluate"), 1);
    assert_eq!(count(&wire, "Page.reload"), 1);
    assert_eq!(count(&wire, "Debugger.removeBreakpoint"), 0);
}

fn fixture() -> (Cdp, Rc<RefCell<State>>, String) {
    let state = Rc::new(RefCell::new(State::default()));
    let mut cdp = Cdp::from_transport(Box::new(Backend(state.clone())));
    let session = cdp
        .call(
            "Target.attachToTarget",
            json!({"targetId":"T","flatten":true}),
            None,
        )
        .unwrap()["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    (cdp, state, session)
}

fn count(state: &State, method: &str) -> usize {
    state
        .calls
        .iter()
        .filter(|call| call["method"] == method)
        .count()
}

#[test]
fn installer_uses_only_instrumentation_and_main_world_evaluation() {
    let (mut cdp, state, session) = fixture();
    let result = cdp.start_webmcp_for(&session, true);
    let state = state.borrow();
    for forbidden in [
        "Page.addScriptToEvaluateOnNewDocument",
        "Runtime.addBinding",
        "Debugger.pause",
        "Debugger.getScriptSource",
        "Debugger.evaluateOnCallFrame",
    ] {
        assert_eq!(count(&state, forbidden), 0, "installer sent {forbidden}");
    }
    let bridge = result.unwrap();
    assert!(state.published);
    assert!(bridge.starts_with("__wmcp_"));
    assert_eq!(count(&state, "Page.reload"), 1);
    let install = state
        .calls
        .iter()
        .position(|call| {
            call["method"] == "Runtime.evaluate"
                && call["params"]["expression"]
                    .as_str()
                    .is_some_and(|source| source.starts_with(&format!("const {bridge} = ")))
        })
        .unwrap();
    assert_eq!(count(&state, "Debugger.removeBreakpoint"), 0);
    assert_eq!(count(&state, "Debugger.setInstrumentationBreakpoint"), 1);
    let resume = state
        .calls
        .iter()
        .position(|call| call["method"] == "Debugger.resume")
        .unwrap();
    assert!(install < resume);
}

#[test]
fn direct_chrome_instrumentation_is_converted_by_the_owned_installer_not_the_client() {
    let (mut cdp, state, session) = fixture();
    state.borrow_mut().raw_instrumentation = true;
    assert!(cdp
        .call("Debugger.pause", json!({}), Some(&session))
        .is_err());
    assert_eq!(count(&state.borrow(), "Debugger.pause"), 0);
    let bridge = cdp.start_webmcp_for(&session, true).unwrap();
    let state = state.borrow();
    assert_eq!(count(&state, "Debugger.pause"), 1);
    assert_eq!(count(&state, "Debugger.resume"), 2);
    let pause = state
        .calls
        .iter()
        .position(|call| call["method"] == "Debugger.pause")
        .unwrap();
    let resumed = state
        .calls
        .iter()
        .position(|call| call["method"] == "Debugger.resume")
        .unwrap();
    let installed = state
        .calls
        .iter()
        .position(|call| {
            call["method"] == "Runtime.evaluate"
                && call["params"]["expression"]
                    .as_str()
                    .is_some_and(|source| source.starts_with(&format!("const {bridge} = ")))
        })
        .unwrap();
    assert!(pause < resumed && resumed < installed);
    assert_eq!(count(&state, "Page.addScriptToEvaluateOnNewDocument"), 0);
    assert_eq!(count(&state, "Debugger.getScriptSource"), 0);
    assert_eq!(count(&state, "Debugger.evaluateOnCallFrame"), 0);
}

#[test]
fn root_install_survives_child_churn_and_unknown_scripts_while_instrumentation_stays_armed() {
    let (mut cdp, state, session) = fixture();
    {
        let mut backend = state.borrow_mut();
        backend.raw_instrumentation = true;
        backend.child_churn = true;
    }
    cdp.start_webmcp_for(&session, true).unwrap();
    assert!(cdp.webmcp_alive(&session));
    assert_eq!(count(&state.borrow(), "Runtime.evaluate"), 1);
    assert_eq!(
        count(&state.borrow(), "Debugger.setInstrumentationBreakpoint"),
        1
    );
    assert_eq!(count(&state.borrow(), "Debugger.removeBreakpoint"), 0);
    let pauses = count(&state.borrow(), "Debugger.pause");
    for reason in ["instrumentation", "other"] {
        state.borrow_mut().incoming.push_back(json!({"method":"Debugger.paused","sessionId":session,"params":{"reason":reason,"data":{"scriptId":"OTHER-PROCESS-EVALUATE"},"callFrames":[{"callFrameId":"UNKNOWN","location":{"scriptId":"OTHER-PROCESS-EVALUATE","lineNumber":0,"columnNumber":0}}]}}).to_string());
        assert!(cdp.pump(Instant::now() + Duration::from_secs(1)).unwrap());
        assert!(cdp.webmcp_alive(&session));
        assert_eq!(count(&state.borrow(), "Runtime.evaluate"), 1);
        assert_eq!(count(&state.borrow(), "Debugger.pause"), pauses);
        assert_eq!(
            count(&state.borrow(), "Debugger.setInstrumentationBreakpoint"),
            1
        );
        assert_eq!(count(&state.borrow(), "Debugger.removeBreakpoint"), 0);
        assert!(!cdp
            .events()
            .iter()
            .any(|event| event["method"] == "Debugger.paused"));
    }
    cdp.stop_webmcp(&session);
    assert_eq!(count(&state.borrow(), "Debugger.removeBreakpoint"), 1);
    assert_eq!(count(&state.borrow(), "Debugger.disable"), 1);
}

#[test]
fn late_install_never_reloads_and_repeated_start_is_a_noop() {
    let (mut cdp, state, session) = fixture();
    let first = cdp.start_webmcp(&session).unwrap();
    assert_eq!(cdp.start_webmcp(&session).unwrap(), first);
    let state = state.borrow();
    assert_eq!(count(&state, "Page.reload"), 0);
    assert_eq!(count(&state, "Runtime.evaluate"), 1);
    assert_eq!(count(&state, "Debugger.setInstrumentationBreakpoint"), 1);
    assert!(!state.published);
}

#[test]
fn document_change_after_publication_never_installs_in_the_new_document() {
    for protected in [false, true] {
        let (mut cdp, state, session) = fixture();
        let mut next = tree("NEW");
        if protected {
            next["frameTree"]["frame"]["url"] = json!("https://seven.sliccy.ai/");
            next["frameTree"]["frame"]["securityOrigin"] = json!("https://seven.sliccy.ai");
        }
        state.borrow_mut().changed = Some(next);
        assert!(cdp.start_webmcp_for(&session, true).is_err());
        assert!(state.borrow().published);
        assert_eq!(count(&state.borrow(), "Runtime.evaluate"), 0);
        assert!(cdp.webmcp_snapshot().is_empty());
    }
}

#[test]
fn generic_debugger_clients_refuse_persistent_calls_and_never_publish_unplaced_events() {
    let (mut cdp, state, session) = fixture();
    let before = state.borrow().calls.len();
    for (method, params) in [
        ("Debugger.enable", json!({})),
        ("Debugger.disable", json!({})),
        ("Debugger.resume", json!({})),
        ("Debugger.setInstrumentationBreakpoint", json!({"instrumentation":"beforeScriptExecution"})),
        ("Debugger.removeBreakpoint", json!({"breakpointId":"UNOWNED"})),
    ] {
        assert_eq!(cdp.call(method, params, Some(&session)).unwrap_err(), "slicc: refused: persistent page capabilities are not supported by slicc-cdp\n");
    }
    assert_eq!(state.borrow().calls.len(), before);
    state.borrow_mut().incoming.push_back(json!({"method":"Debugger.scriptParsed","sessionId":session,"params":{"scriptId":"NEVER-PLACED","executionContextId":404,"url":"","embedderName":"","hash":"reader-a-private-marker"}}).to_string());
    state.borrow_mut().incoming.push_back(json!({"method":"Debugger.paused","sessionId":session,"params":{"reason":"instrumentation","data":{"scriptId":"NEVER-PLACED"},"callFrames":[{"callFrameId":"1.404.1","location":{"scriptId":"NEVER-PLACED"},"scopeChain":[{"object":{"objectId":"1.404.2"}}],"this":{"objectId":"1.404.3"}}]}}).to_string());
    for _ in 0..2 { assert!(cdp.pump(Instant::now() + Duration::from_secs(1)).unwrap()); }
    assert!(cdp.events().iter().all(|event| !event["method"].as_str().unwrap_or("").starts_with("Debugger.")));
}

#[test]
fn holder_debugger_events_remain_private_while_installation_still_works() {
    let (mut cdp, state, session) = fixture();
    cdp.start_webmcp_for(&session, true).unwrap();
    assert!(state.borrow().published);
    assert_eq!(count(&state.borrow(), "Debugger.enable"), 1);
    assert_eq!(count(&state.borrow(), "Debugger.setInstrumentationBreakpoint"), 1);
    assert_eq!(count(&state.borrow(), "Runtime.evaluate"), 1);
    assert!(cdp.events().iter().all(|event| !event["method"].as_str().unwrap_or("").starts_with("Debugger.")));
    assert!(cdp.call("Debugger.resume", json!({}), Some(&session)).is_err());
}

#[test]
fn source_and_call_frame_methods_remain_refused_after_a_holder_pause() {
    let (mut cdp, state, session) = fixture();
    cdp.start_webmcp_for(&session, true).unwrap();
    assert!(state.borrow().published);
    assert!(cdp.events().iter().all(|event| !event["method"].as_str().unwrap_or("").starts_with("Debugger.")));
    for (method, params) in [
        ("Debugger.getScriptSource", json!({"scriptId":"SCRIPT"})),
        (
            "Debugger.evaluateOnCallFrame",
            json!({"callFrameId":"CALL","expression":"1"}),
        ),
    ] {
        assert!(cdp.call(method, params, Some(&session)).is_err());
        assert_eq!(count(&state.borrow(), method), 0);
    }
}

#[test]
fn plain_fallback_uses_complete_guarded_reads_and_only_the_requested_reload() {
    for reload in [false, true] {
        let (mut cdp, state, session) = fixture();
        state.borrow_mut().refusal =
            Some("slicc: refused: that execution context belongs to a browser extension");
        cdp.start_webmcp_for(&session, reload).unwrap();
        assert!(cdp.webmcp_plain(&session));
        cdp.stop_webmcp(&session);
        let state = state.borrow();
        assert_eq!(count(&state, "Page.reload"), usize::from(reload));
        assert_eq!(count(&state, "Runtime.evaluate"), 1);
        assert_eq!(count(&state, "DOM.getDocument"), 0);
        assert!(state
            .calls
            .iter()
            .filter(|call| call["method"].as_str().unwrap().starts_with("Debugger."))
            .all(|call| call["method"] == "Debugger.enable"));
    }
}

#[test]
fn plain_fallback_waits_for_loaded_children_without_a_pierced_dom_read() {
    let (mut cdp, state, session) = fixture();
    {
        let mut state = state.borrow_mut();
        state.refusal = Some("slicc: refused: that execution context belongs to a browser extension");
        state.dom_refusal = true;
        state.pending_child = true;
    }
    let started = Instant::now();
    cdp.start_webmcp_for(&session, true).unwrap();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(cdp.webmcp_plain(&session));
    let state = state.borrow();
    assert!(state.load_sent);
    assert!(state.pending_reads >= 2);
    assert_eq!(count(&state, "Page.reload"), 1);
    assert_eq!(count(&state, "DOM.getDocument"), 0);
    assert_eq!(count(&state, "Runtime.evaluate"), 1);
}

#[test]
fn a_world_arriving_after_enable_switches_once_without_a_second_reload() {
    for reload in [false, true] {
        let (mut cdp, state, session) = fixture();
        state.borrow_mut().late_world = true;
        let started = Instant::now();
        cdp.start_webmcp_for(&session, reload)
            .unwrap_or_else(|message| {
                panic!(
                    "{message}; sent {:?}",
                    state
                        .borrow()
                        .calls
                        .iter()
                        .map(|call| call["method"].as_str().unwrap())
                        .collect::<Vec<_>>()
                )
            });
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(cdp.webmcp_plain(&session));
        let bridge = cdp.start_webmcp_for(&session, reload).unwrap();
        assert!(bridge.starts_with("__wmcp_"));
        cdp.stop_webmcp(&session);
        let state = state.borrow();
        assert_eq!(count(&state, "Debugger.enable"), 1);
        assert_eq!(count(&state, "Debugger.disable"), 1);
        assert_eq!(count(&state, "Debugger.setInstrumentationBreakpoint"), 1);
        assert_eq!(count(&state, "Page.reload"), usize::from(reload));
        assert_eq!(count(&state, "Runtime.evaluate"), 1);
        let disabled = state
            .calls
            .iter()
            .position(|call| call["method"] == "Debugger.disable")
            .unwrap();
        assert!(state.calls[disabled + 1..]
            .iter()
            .all(|call| !call["method"].as_str().unwrap().starts_with("Debugger.")));
    }
}

#[test]
fn plain_reload_replays_the_committed_root_before_loading_children() {
    let (mut cdp, state, session) = fixture();
    {
        let mut wire = state.borrow_mut();
        wire.refusal = Some("slicc: refused: a browser extension runs scripts in this page, and profiling it would expose them");
        wire.replay_only = true;
        wire.load_after_install = true;
        wire.pending_child = true;
        wire.disable_after_reload_refused = true;
    }
    cdp.start_webmcp_for(&session, true).unwrap();
    assert!(cdp.webmcp_alive(&session));
    let wire = state.borrow();
    let reload = wire.calls.iter().position(|call| call["method"] == "Page.reload").unwrap();
    let after = &wire.calls[reload + 1..];
    assert_eq!(after.iter().filter(|call| call["method"] == "Runtime.disable" && call["sessionId"] == session).count(), 0);
    let fresh = after.iter().find(|call| call["method"] == "Runtime.enable").unwrap()["sessionId"].as_str().unwrap();
    assert_ne!(fresh, session);
    assert!(after.iter().any(|call| call["method"] == "Target.attachToTarget" && call["params"]["flatten"] == true));
    assert!(!after.iter().any(|call| call["method"] == "Target.detachFromTarget" && call["params"]["sessionId"] == fresh));
    assert!(after.iter().any(|call| call["method"] == "Runtime.evaluate" && call["sessionId"] == fresh));
    assert_eq!(count(&wire, "Runtime.evaluate"), 1);
    assert_eq!(count(&wire, "Page.reload"), 1);
    assert!(wire.pending_reads >= 2);
    assert!(after.iter().all(|call| !call["method"].as_str().unwrap().starts_with("Debugger.")));
}

#[test]
fn plain_replayed_root_installs_with_one_identity_read_and_no_execution_replay() {
    let (mut cdp, state, session) = fixture();
    {
        let mut wire = state.borrow_mut();
        wire.refusal = Some("slicc: refused: a browser extension runs scripts in this page, and profiling it would expose them");
        wire.replay_only = true;
        wire.load_after_install = true;
    }
    cdp.start_webmcp_for(&session, true).unwrap();
    let wire = state.borrow();
    let reload = wire.calls.iter().position(|call| call["method"] == "Page.reload").unwrap();
    let install = wire.calls.iter().position(|call| call["method"] == "Runtime.evaluate" && call["params"]["expression"].as_str().is_some_and(|source| source.starts_with("const __wmcp_"))).unwrap();
    let before = &wire.calls[reload + 1..install];
    let methods = before.iter().map(|call| call["method"].as_str().unwrap()).collect::<Vec<_>>();
    assert_eq!(before.iter().filter(|call| call["method"] == "Runtime.enable").count(), 1, "{methods:?}");
    assert_eq!(before.iter().filter(|call| call["method"] == "Page.getFrameTree").count(), 1, "{methods:?}");
    assert_eq!(before.iter().filter(|call| call["method"] == "Runtime.disable").count(), 0, "{methods:?}");
    assert_eq!(before.iter().filter(|call| call["method"] == "Target.getTargets").count(), 0, "{methods:?}");
    assert_eq!(count(&wire, "Runtime.evaluate"), 1);
    assert_eq!(count(&wire, "Page.reload"), 1);
    assert!(wire.calls[install + 1..].iter().any(|call| call["method"] == "Page.getFrameTree"));
    assert!(cdp.webmcp_alive(&session));
}

#[test]
fn plain_fallback_retains_the_installed_session_for_status_and_owner_cleanup() {
    let (mut cdp, state, original) = fixture();
    {
        let mut wire = state.borrow_mut();
        wire.refusal = Some("slicc: refused: a browser extension runs scripts in this page, and profiling it would expose them");
        wire.replay_only = true;
        wire.status_original_refused = true;
    }
    let bridge = cdp.start_webmcp_for(&original, true).unwrap();
    let fresh = state.borrow().calls.iter().find(|call| call["method"] == "Runtime.evaluate").unwrap()["sessionId"].as_str().unwrap().to_string();
    assert_ne!(fresh, original);
    assert_eq!(cdp.webmcp_snapshot()[0]["session"], fresh);
    assert!(!state.borrow().calls.iter().any(|call| call["method"] == "Target.detachFromTarget" && call["params"]["sessionId"] == fresh));
    cdp.call("Runtime.evaluate", json!({"expression":"typeof document.modelContext","uniqueContextId":"main-R","returnByValue":true}), Some(&fresh)).unwrap();
    assert_eq!(cdp.start_webmcp_for(&original, true).unwrap(), bridge);
    assert_eq!(count(&state.borrow(), "Page.reload"), 1);
    cdp.stop_webmcp(&original);
    assert!(cdp.webmcp_snapshot().is_empty());
    assert_eq!(state.borrow().calls.iter().filter(|call| call["method"] == "Target.detachFromTarget" && call["params"]["sessionId"] == fresh).count(), 1);
}

#[test]
fn missing_fresh_replay_stops_without_another_install_approach() {
    let (mut cdp, state, session) = fixture();
    {
        let mut wire = state.borrow_mut();
        wire.refusal = Some("slicc: refused: a browser extension runs scripts in this page, and profiling it would expose them");
        wire.replay_only = true;
        wire.fresh_replay_missing = true;
    }
    let message = cdp.start_webmcp_for(&session, true).unwrap_err();
    assert!(message.contains("other-extension fallback"));
    assert!(!cdp.webmcp_alive(&session));
    let wire = state.borrow();
    assert_eq!(count(&wire, "Runtime.evaluate"), 0);
    assert_eq!(count(&wire, "Page.reload"), 1);
    let reload = wire.calls.iter().position(|call| call["method"] == "Page.reload").unwrap();
    let after = &wire.calls[reload + 1..];
    assert_eq!(after.iter().filter(|call| call["method"] == "Target.attachToTarget").count(), 1);
    assert_eq!(after.iter().filter(|call| call["method"] == "Target.detachFromTarget").count(), 1);
    assert_eq!(after.iter().filter(|call| call["method"] == "Runtime.disable" && call["sessionId"] == session).count(), 0);
}

#[test]
fn a_pause_inside_the_plain_evaluate_of_an_armed_realm_never_installs_twice() {
    let (mut cdp, state, session) = fixture();
    state.borrow_mut().pause_during_install = true;
    cdp.start_webmcp_for(&session, false).unwrap();
    assert!(cdp.webmcp_alive(&session));
    let wire = state.borrow();
    assert_eq!(count(&wire, "Runtime.evaluate"), 1);
    assert_eq!(count(&wire, "Page.reload"), 0);
    assert_eq!(count(&wire, "Debugger.resume"), 1);
}

struct PendingPlacementWire {
    backend: Backend,
    next_child: Value,
    settle_at: Option<Instant>,
    held_since: Option<Instant>,
    awaiting: bool,
    delay: Duration,
    storm: bool,
    root_change: bool,
    foreign_world: bool,
    resumed_early: Rc<RefCell<bool>>,
}

fn pending_placement_child() -> Value {
    json!({"frame":{"id":"C","parentId":"F","loaderId":"","url":"","securityOrigin":"null"}})
}

impl Transport for PendingPlacementWire {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let command: Value = serde_json::from_str(text).unwrap();
        let conversion_resume = command["method"] == "Debugger.resume" && self.backend.0.borrow().conversion_pending;
        if command["method"] == "Debugger.resume" && !conversion_resume && self.awaiting {
            *self.resumed_early.borrow_mut() = true;
        }
        self.backend.send_text(text)?;
        if command["method"] == "Debugger.resume" && !conversion_resume {
            if let Some(started) = self.held_since {
                self.backend.0.borrow_mut().calls.last_mut().unwrap()["fixtureHeldMicros"] = json!(started.elapsed().as_micros());
            }
        }
        if command["method"] == "Page.reload" {
            self.backend.0.borrow_mut().tree.as_mut().unwrap()["frameTree"]["childFrames"] = json!([pending_placement_child()]);
        }
        if conversion_resume {
            self.awaiting = true;
            self.held_since = Some(Instant::now());
            self.settle_at = Some(Instant::now() + self.delay);
        }
        Ok(())
    }

    fn recv_text(&mut self, until: Instant) -> Result<Option<String>, String> {
        if let Some(message) = self.backend.recv_text(until)? { return Ok(Some(message)); }
        if self.awaiting {
            let at = self.settle_at.unwrap();
            let wake = if self.storm { (Instant::now() + Duration::from_millis(20)).min(at).min(until) } else { at.min(until) };
            std::thread::sleep(wake.saturating_duration_since(Instant::now()));
            if Instant::now() >= at {
                self.awaiting = false;
                let mut state = self.backend.0.borrow_mut();
                state.tree.as_mut().unwrap()["frameTree"]["childFrames"] = json!([self.next_child]);
                if self.root_change { state.tree.as_mut().unwrap()["frameTree"]["frame"]["loaderId"] = json!("REPLACED"); }
                for owner in state.sessions.keys().cloned().collect::<Vec<_>>() {
                    if self.root_change {
                        state.incoming.push_back(json!({"method":"Page.frameNavigated","sessionId":owner,"params":{"frame":tree("REPLACED")["frameTree"]["frame"]}}).to_string());
                    }
                    if self.foreign_world {
                        state.incoming.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":owner,"params":{"context":{"id":19,"uniqueId":"protected-world","origin":"https://seven.sliccy.ai","auxData":{"frameId":"F","isDefault":false,"type":"isolated"}}}}).to_string());
                    }
                    state.incoming.push_back(json!({"method":"Page.frameNavigated","sessionId":owner,"params":{"frame":self.next_child["frame"]}}).to_string());
                }
                drop(state);
                return self.backend.recv_text(until);
            }
            if self.storm && Instant::now() < until {
                let owner = self.backend.0.borrow().armed.clone().unwrap();
                return Ok(Some(json!({"method":"Page.frameNavigated","sessionId":owner,"params":{"frame":pending_placement_child()["frame"]}}).to_string()));
            }
        }
        Ok(None)
    }
}

fn pending_placement_client() -> (Cdp, Rc<RefCell<State>>, String, Rc<RefCell<bool>>) {
    pending_placement_client_with("ordinary")
}

fn pending_placement_client_with(mode: &str) -> (Cdp, Rc<RefCell<State>>, String, Rc<RefCell<bool>>) {
    let state = Rc::new(RefCell::new(State { raw_instrumentation: true, ..Default::default() }));
    let resumed = Rc::new(RefCell::new(false));
    let wire = PendingPlacementWire {
        backend: Backend(state.clone()),
        next_child: if mode == "protected" { json!({"frame":{"id":"C","parentId":"F","loaderId":"CHILD","url":"https://seven.sliccy.ai/child","securityOrigin":"https://seven.sliccy.ai"}}) } else { json!({"frame":{"id":"C","parentId":"F","loaderId":"CHILD","url":"https://ordinary.test/child","securityOrigin":"https://ordinary.test"}}) },
        settle_at: None,
        held_since: None,
        awaiting: false,
        delay: Duration::from_millis(if mode == "expiry" { 1400 } else { 20 }),
        storm: mode == "expiry",
        root_change: mode == "root-change",
        foreign_world: mode == "foreign",
        resumed_early: resumed.clone(),
    };
    let mut cdp = Cdp::from_transport(Box::new(wire));
    cdp.call("Target.getTargets", json!({}), None).unwrap();
    let attached = cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap();
    (cdp, state, attached["sessionId"].as_str().unwrap().to_string(), resumed)
}

#[test]
fn converted_root_pause_waits_for_pending_child_placement_before_startup_install() {
    let (mut cdp, state, session, resumed) = pending_placement_client();
    cdp.start_webmcp_for(&session, true).unwrap();
    assert!(!*resumed.borrow(), "the normal root pause is not lost before the child placement event");
    assert_eq!(count(&state.borrow(), "Runtime.evaluate"), 1);
    assert_eq!(count(&state.borrow(), "Debugger.pause"), 1);
    assert!(cdp.webmcp_alive(&session));
}

#[test]
fn converted_root_pause_preserves_a_live_holder_across_pending_child_placement() {
    let (mut cdp, state, session, resumed) = pending_placement_client();
    cdp.start_webmcp_for(&session, false).unwrap();
    {
        let mut wire = state.borrow_mut();
        wire.tree = Some(tree("FUTURE"));
        wire.tree.as_mut().unwrap()["frameTree"]["childFrames"] = json!([pending_placement_child()]);
        for owner in wire.sessions.keys().cloned().collect::<Vec<_>>() {
            wire.incoming.push_back(json!({"method":"Page.frameNavigated","sessionId":owner,"params":{"frame":tree("FUTURE")["frameTree"]["frame"]}}).to_string());
            wire.incoming.push_back(json!({"method":"Runtime.executionContextsCleared","sessionId":owner,"params":{}}).to_string());
            wire.incoming.push_back(context(&owner, "FUTURE").to_string());
        }
        wire.incoming.push_back(json!({"method":"Debugger.scriptParsed","sessionId":session,"params":{"scriptId":"SCRIPT","executionContextId":1,"url":"https://ordinary.test/first.js"}}).to_string());
        wire.incoming.push_back(json!({"method":"Debugger.paused","sessionId":session,"params":{"reason":"instrumentation","callFrames":[{"callFrameId":"CALL","location":{"scriptId":"SCRIPT","lineNumber":0,"columnNumber":0}}]}}).to_string());
    }
    while cdp.pump(Instant::now() + Duration::from_millis(50)).unwrap() {}
    assert!(!*resumed.borrow());
    assert_eq!(count(&state.borrow(), "Runtime.evaluate"), 2);
    assert!(cdp.webmcp_alive(&session));
}

#[test]
fn converted_root_pause_rescues_protected_foreign_replaced_and_expired_pending_children() {
    for mode in ["protected", "foreign", "root-change", "expiry"] {
        let (mut cdp, state, session, resumed) = pending_placement_client_with(mode);
        let start = Instant::now();
        assert!(cdp.start_webmcp_for(&session, true).is_err(), "{mode}");
        let spent = start.elapsed();
        assert_eq!(count(&state.borrow(), "Runtime.evaluate"), 0, "{mode}");
        assert!(!cdp.webmcp_alive(&session), "{mode}");
        assert!(count(&state.borrow(), "Debugger.resume") >= 2, "{mode}");
        if mode == "expiry" {
            assert!(*resumed.borrow(), "expiry resumes before the eventual ordinary child");
            let held = state.borrow().calls.iter().filter_map(|command| command["fixtureHeldMicros"].as_u64()).next().unwrap();
            assert!(held >= 900_000 && held < 1_200_000, "nonreset private-resume cap: {held}us");
            assert!(spent < Duration::from_millis(1800), "{spent:?}");
        } else { assert!(spent < Duration::from_millis(500), "{mode}: {spent:?}"); }
    }
}

#[test]
fn protected_incomplete_and_version_refusals_never_fall_back() {
    for mode in [
        "protected-read",
        "incomplete",
        "protected-world",
        "version",
        "instrumentation-version",
    ] {
        let (mut cdp, state, session) = fixture();
        {
            let mut state = state.borrow_mut();
            state.refusal = if mode == "version" {
                Some("slicc: refused: update the browser extension")
            } else if mode == "instrumentation-version" {
                None
            } else {
                Some("slicc: refused: that execution context belongs to a browser extension")
            };
            state.tree_refusal = mode == "protected-read";
            state.incomplete = mode == "incomplete";
            state.protected_world = mode == "protected-world";
            state.missing_instrumentation = mode == "instrumentation-version";
        }
        let message = cdp.start_webmcp_for(&session, true).unwrap_err();
        assert!(
            message.contains("refused") || message.contains("update"),
            "{mode}: {message}"
        );
        assert!(!cdp.webmcp_plain(&session), "{mode}");
        assert_eq!(count(&state.borrow(), "Page.reload"), 0, "{mode}");
        assert_eq!(count(&state.borrow(), "Runtime.evaluate"), 0, "{mode}");
    }
}
