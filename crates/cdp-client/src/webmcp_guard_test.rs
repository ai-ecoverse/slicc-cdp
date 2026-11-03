use super::*;
use crate::cdp::Transport;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Default)]
struct BackendState {
    inbox: VecDeque<String>,
    sent: Vec<Value>,
    tree: Value,
    current_frame: String,
    installed: bool,
    api: bool,
    pause: bool,
    race: bool,
    unconverted: bool,
    fail_tree: bool,
    context_race: String,
    initial_child: bool,
    child_loader_race: bool,
    runtime_child: Option<Value>,
}

struct Backend(Rc<RefCell<BackendState>>);

struct SettlingBackend {
    inner: Backend,
    due: Option<Instant>,
    refuse: bool,
}

impl Transport for SettlingBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let command: Value = serde_json::from_str(text).unwrap();
        self.inner.send_text(text)?;
        if command["method"] == "Runtime.enable" && command["sessionId"] == "H" {
            self.due = Some(Instant::now() + Duration::from_secs(1));
            if self.refuse {
                let mut state = self.inner.0.borrow_mut();
                state.inbox.clear();
                state.inbox.push_back(json!({"id":command["id"],"error":{"code":-32000,"message":"slicc: refused: seven’s own pages and the vault are out of reach of browser automation"}}).to_string());
            }
        }
        Ok(())
    }

    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
        if let Some(due) = self.due.take() {
            std::thread::sleep(due.min(deadline).saturating_duration_since(Instant::now()));
            if Instant::now() < due {
                self.due = Some(due);
                return Ok(None);
            }
        }
        self.inner.recv_text(deadline)
    }
}

#[test]
fn one_second_settling_reply_preserves_rust_document_check_and_terminal_refusal() {
    for refuse in [false, true] {
        let (mut cdp, state) = client();
        cdp.ws = Box::new(SettlingBackend { inner: Backend(state.clone()), due: None, refuse });
        let start = Instant::now();
        let result = cdp.start_webmcp_for("H", true);
        let spent = start.elapsed();
        assert!(spent >= Duration::from_secs(1) && spent < Duration::from_millis(1800), "{spent:?} {result:?}");
        if refuse {
            let error = result.unwrap_err();
            assert!(error.contains("out of reach"), "{error}");
            assert!(!error.contains("timed out"), "{error}");
            assert!(!state.borrow().api);
            assert!(!state.borrow().sent.iter().any(|command| matches!(command["method"].as_str(), Some("Debugger.enable" | "Debugger.setInstrumentationBreakpoint" | "Page.reload"))));
            assert!(state.borrow().sent.iter().filter_map(|command| command["method"].as_str()).filter(|method| method.starts_with("Debugger.")).all(|method| matches!(method, "Debugger.resume" | "Debugger.disable")));
        } else {
            assert!(result.is_ok(), "{result:?}");
            assert!(state.borrow().api);
        }
    }
}

fn event(method: &str, params: Value) -> Value {
    json!({"method":method,"sessionId":"H","params":params})
}

fn paused() -> Value {
    event("Debugger.paused", json!({"reason":"other","callFrames":[{"callFrameId":"CALL","functionName":"","location":{"scriptId":"SCRIPT","lineNumber":0,"columnNumber":0}}]}))
}

fn context(frame: &str, origin: &str) -> Value {
    event("Runtime.executionContextCreated", json!({"context":{"id":7,"uniqueId":"CONTEXT","origin":origin,"auxData":{"frameId":frame,"isDefault":true}}}))
}

fn frame(url: &str) -> Value {
    let origin = url::Url::parse(url).map(|url| url.origin().ascii_serialization()).unwrap_or_else(|_| "null".into());
    json!({"frameTree":{"frame":{"id":"F","url":url,"securityOrigin":origin,"loaderId":"LOADER"}}})
}

impl Transport for Backend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let command: Value = serde_json::from_str(text).unwrap();
        let mut state = self.0.borrow_mut();
        state.sent.push(command.clone());
        let method = command["method"].as_str().unwrap();
        let result = match method {
            "Target.getTargets" => json!({"targetInfos":[{"targetId":"T","type":"page","url":state.tree["frameTree"]["frame"]["url"]}]}),
            "Page.getFrameTree" => {
                if state.fail_tree && state.installed {
                    state.inbox.push_back(json!({"id":command["id"],"error":{"message":"verification failed"}}).to_string());
                    return Ok(());
                }
                state.tree.clone()
            }
            "Runtime.enable" => {
                let mut created = context(&state.current_frame, "https://ordinary.test");
                created["sessionId"] = command["sessionId"].clone();
                state.inbox.push_back(created.to_string());
                let child = state.runtime_child.clone().or_else(|| state.initial_child.then(|| json!({"id":8,"uniqueId":"CHILD-CONTEXT","origin":state.tree["frameTree"]["childFrames"][0]["frame"]["securityOrigin"],"auxData":{"frameId":"C","isDefault":true}})));
                if let Some(child) = child {
                    state.inbox.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":command["sessionId"],"params":{"context":child}}).to_string());
                }
                json!({})
            }
            "Debugger.setInstrumentationBreakpoint" => {
                state.installed = true;
                json!({"breakpointId":"INSTRUMENT"})
            }
            "Page.reload" => {
                if state.pause {
                    state.inbox.push_back(event("Debugger.scriptParsed", json!({"scriptId":"SCRIPT","executionContextId":7})).to_string());
                    let mut pause = paused();
                    if state.unconverted { pause["params"]["reason"] = json!("instrumentation"); }
                    state.inbox.push_back(pause.to_string());
                }
                json!({})
            }
            "Runtime.evaluate" if command["params"]["expression"].as_str().is_some_and(|source| source.starts_with("const __wmcp_")) => {
                if state.race {
                    state.tree = frame("https://seven.sliccy.ai/");
                    let changed = state.tree["frameTree"]["frame"].clone();
                    state.inbox.push_back(event("Page.frameNavigated", json!({"frame":changed})).to_string());
                    state.inbox.push_back(json!({"id":command["id"],"error":{"message":"Cannot find context with specified id"}}).to_string());
                    return Ok(());
                }
                state.api = true;
                if state.child_loader_race && command["params"]["uniqueContextId"] == "CONTEXT" {
                    state.tree["frameTree"]["childFrames"][0]["frame"]["loaderId"] = json!("REPLACED-CHILD");
                }
                if state.context_race == "child-evaluate" && command["params"]["uniqueContextId"] == "CHILD-CONTEXT" {
                    state.inbox.push_back(event("Runtime.executionContextCreated", json!({"context":{"id":8,"uniqueId":"REPLACED-CHILD-CONTEXT","origin":"https://ordinary.test","auxData":{"frameId":"C","isDefault":true}}})).to_string());
                }
                if state.context_race == "evaluate" {
                    let mut changed = context("F", "https://ordinary.test");
                    changed["params"]["context"]["uniqueId"] = json!("REUSED_CONTEXT");
                    state.inbox.push_back(changed.to_string());
                    state.api = false;
                }
                json!({"result":{"type":"undefined"}})
            }
            "Runtime.evaluate" if command["params"]["expression"] == "document.readyState" => json!({"result":{"type":"string","value":"complete"}}),
            "Runtime.evaluate" => json!({"result":{"type":"number","value":1}}),
            "Debugger.resume" => {
                if state.context_race == "resume" && state.api {
                    let mut changed = context("F", "https://ordinary.test");
                    changed["params"]["context"]["uniqueId"] = json!("REUSED_CONTEXT");
                    state.inbox.push_back(changed.to_string());
                    state.api = false;
                }
                json!({})
            }
            "Debugger.removeBreakpoint" | "Debugger.disable" | "Target.detachFromTarget" => {
                state.installed = false;
                json!({})
            }
            _ => json!({}),
        };
        state.inbox.push_back(json!({"id":command["id"],"result":result}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.0.borrow_mut().inbox.pop_front())
    }
}

fn client() -> (Cdp, Rc<RefCell<BackendState>>) {
    let state = Rc::new(RefCell::new(BackendState { tree: frame("https://ordinary.test/"), current_frame: "F".into(), pause: true, ..Default::default() }));
    let mut cdp = Cdp::from_transport(Box::new(Backend(state.clone())));
    cdp.protect_origins(&["https://catalog.test"]);
    cdp.session_targets.insert("H".into(), "T".into());
    cdp.guards.insert("T".into(), "G".into());
    cdp.guard_sessions.insert("G".into());
    cdp.targets.insert("T".into(), json!({"targetId":"T","type":"page","url":"https://ordinary.test/"}));
    cdp.main_commit("G", &state.borrow().tree["frameTree"]["frame"]);
    (cdp, state)
}

fn enter_document(cdp: &mut Cdp, state: &Rc<RefCell<BackendState>>, tree: Value, frame: &str) {
    {
        let mut backend = state.borrow_mut();
        backend.tree = tree;
        backend.api = false;
        backend.current_frame = frame.to_string();
    }
    let changed = {
        let backend = state.borrow();
        Cdp::webmcp_frame(&backend.tree["frameTree"], frame).cloned()
    };
    if let Some(changed) = changed { cdp.absorb(event("Page.frameNavigated", json!({"frame":changed}))); }
    cdp.absorb(context(frame, "https://ordinary.test"));
    cdp.absorb(event("Debugger.scriptParsed", json!({"scriptId":"SCRIPT","executionContextId":7})));
    cdp.absorb(paused());
}

fn no_protected_api(cdp: &mut Cdp, state: &Rc<RefCell<BackendState>>) {
    assert!(!state.borrow().api, "shim executed in a document the guard did not verify");
    assert!(!state.borrow().installed, "instrumentation was not removed");
    cdp.block("H");
    for expression in ["document.modelContext.getTools()", "document.modelContext.executeTool('secret', {})"] {
        assert!(cdp.call("Runtime.evaluate", json!({"expression":expression}), Some("H")).is_err());
    }
}

#[test]
fn numeric_context_reuse_during_installation_or_resume_revokes_the_installer() {
    for stage in ["evaluate", "resume"] {
        let (mut cdp, state) = client();
        state.borrow_mut().context_race = stage.to_string();
        assert!(cdp.start_webmcp_for("H", true).is_err(), "{stage}");
        no_protected_api(&mut cdp, &state);
        assert!(cdp.webmcp_snapshot().is_empty());
    }
}

#[test]
fn source_and_call_frame_methods_remain_refused_at_a_holder_held_pause() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    cdp.webmcp.handling_gate = true;
    cdp.absorb(paused());
    assert!(cdp.webmcp_is_paused("H"));
    assert_eq!(cdp.webmcp.pending_gates.len(), 1);
    let count = state.borrow().sent.len();
    for (method, params) in [
        ("Debugger.getScriptSource", json!({"scriptId":"SCRIPT"})),
        ("Debugger.evaluateOnCallFrame", json!({"callFrameId":"CALL","expression":"1"})),
    ] {
        assert!(cdp.call(method, params, Some("H")).is_err());
        assert_eq!(state.borrow().sent.len(), count);
    }
    assert!(cdp.events().iter().all(|event| !event["method"].as_str().unwrap_or("").starts_with("Debugger.")));
    cdp.webmcp.handling_gate = false;
    cdp.stop_webmcp("H");
}

fn child_context(cdp: &mut Cdp, state: &Rc<RefCell<BackendState>>, id: i64, unique: &str) {
    let context = json!({"id":id,"uniqueId":unique,"origin":"https://ordinary.test","auxData":{"frameId":"C","isDefault":true}});
    state.borrow_mut().runtime_child = Some(context.clone());
    cdp.absorb(event("Runtime.executionContextCreated", json!({"context":context})));
}

#[test]
fn initial_same_origin_child_installs_on_a_guarded_root_pause_once_without_child_debugger_publication() {
    let (mut cdp, state) = client();
    state.borrow_mut().initial_child = true;
    state.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":{"id":"C","parentId":"F","loaderId":"CHILD","url":"https://ordinary.test/child","securityOrigin":"https://ordinary.test"}}]);
    cdp.start_webmcp_for("H", true).unwrap();
    let installed = || state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(|source| source.starts_with("const __wmcp_"))).cloned().collect::<Vec<_>>();
    assert_eq!(installed().len(), 2);
    assert!(installed().iter().any(|command| command["params"]["uniqueContextId"] == "CHILD-CONTEXT"));
    assert_eq!(cdp.webmcp_generations().len(), 2);
    cdp.absorb(paused());
    assert_eq!(installed().len(), 2, "a second pause never redeclares either realm's bridge");
    assert!(cdp.events().iter().all(|event| !event["method"].as_str().unwrap_or("").starts_with("Debugger.")));
    assert!(!state.borrow().sent.iter().any(|command| matches!(command["method"].as_str(), Some("Debugger.pause" | "Debugger.evaluateOnCallFrame"))));
}

#[test]
fn initial_child_installation_rejects_foreign_unplaced_protected_and_replaced_child_or_root_identity() {
    for mode in ["foreign", "unplaced", "protected", "child-loader", "child-unique", "root"] {
        let (mut cdp, state) = client();
        state.borrow_mut().initial_child = true;
        let origin = match mode { "foreign" => "https://foreign.test", "protected" => "https://seven.sliccy.ai", "unplaced" => "null", _ => "https://ordinary.test" };
        let url = if mode == "unplaced" { "about:blank".into() } else { format!("{origin}/child") };
        state.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":{"id":"C","parentId":"F","loaderId":if mode == "unplaced" { "" } else { "CHILD" },"url":url,"securityOrigin":origin}}]);
        state.borrow_mut().child_loader_race = mode == "child-loader";
        state.borrow_mut().race = mode == "root";
        if mode == "child-unique" { state.borrow_mut().context_race = "child-evaluate".into(); }
        let result = cdp.start_webmcp_for("H", true);
        if matches!(mode, "foreign" | "child-loader" | "child-unique") { assert!(result.is_ok(), "{mode}: {result:?}"); }
        else { assert!(result.is_err(), "{mode}"); }
        let child_installs = state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "CHILD-CONTEXT").count();
        assert_eq!(child_installs, usize::from(mode == "child-unique"), "{mode}");
        assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "REPLACED-CHILD-CONTEXT"));
        assert!(!cdp.webmcp_generations().iter().any(|entry| entry["frameId"] == "C"), "{mode}");
    }
}

#[test]
fn a_same_origin_child_created_after_the_root_was_accepted_installs_on_placement() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    state.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":{"id":"C","parentId":"F","loaderId":"CHILD","url":"https://ordinary.test/child","securityOrigin":"https://ordinary.test"}}]);
    cdp.absorb(event("Page.frameNavigated", json!({"frame":state.borrow().tree["frameTree"]["childFrames"][0]["frame"]})));
    child_context(&mut cdp, &state, 8, "CHILD-CONTEXT");
    cdp.webmcp_install_children();
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "CHILD-CONTEXT").count(), 1);
    assert_eq!(cdp.webmcp_generations().len(), 2);
    cdp.webmcp_install_children();
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "CHILD-CONTEXT").count(), 1);
}

struct DirectChildPauseWire {
    inner: Backend,
    converting: bool,
}

impl Transport for DirectChildPauseWire {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let command: Value = serde_json::from_str(text).unwrap();
        self.inner.send_text(text)?;
        if command["method"] == "Debugger.pause" { self.converting = true; }
        else if command["method"] == "Debugger.resume" && self.converting {
            self.converting = false;
            let mut state = self.inner.0.borrow_mut();
            let reply = state.inbox.pop_back().unwrap();
            let mut normal = paused();
            normal["params"]["callFrames"][0]["location"]["scriptId"] = json!("CHILD-SCRIPT");
            state.inbox.push_back(normal.to_string());
            state.inbox.push_back(reply);
        }
        Ok(())
    }
    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> { self.inner.recv_text(deadline) }
}

#[test]
fn direct_child_runtime_path_is_a_noop_after_the_owned_pause_path_installed_the_realm() {
    let (mut cdp, state) = client();
    let child = json!({"id":"C","parentId":"F","loaderId":"CHILD","url":"https://ordinary.test/child","securityOrigin":"https://ordinary.test"});
    state.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":child}]);
    state.borrow_mut().initial_child = true;
    cdp.webmcp.handling_children = true;
    cdp.start_webmcp_for("H", false).unwrap();
    cdp.call_for("Page.getFrameTree", json!({}), Some("H"), DOCUMENT_CHECK).unwrap();
    cdp.ws = Box::new(DirectChildPauseWire { inner: Backend(state.clone()), converting: false });
    cdp.absorb(event("Debugger.scriptParsed", json!({"scriptId":"CHILD-SCRIPT","executionContextId":8,"url":"https://ordinary.test/child.js"})));
    cdp.absorb(event("Debugger.paused", json!({"reason":"instrumentation","data":{"scriptId":"CHILD-SCRIPT"},"callFrames":[{"callFrameId":"CHILD-CALL","location":{"scriptId":"CHILD-SCRIPT","lineNumber":0,"columnNumber":0}}]})));
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Debugger.pause"));
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "CHILD-CONTEXT").count(), 1);
    let samples = cdp.webmcp_pause_micros().len();
    cdp.webmcp.handling_children = false;
    cdp.webmcp_install_children();
    cdp.webmcp_install_children();
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "CHILD-CONTEXT").count(), 1);
    assert_eq!(cdp.webmcp_pause_micros().len(), samples);
    assert_eq!(cdp.webmcp_generations().iter().filter(|entry| entry["frameId"] == "C").count(), 1);
}

#[test]
fn child_about_blank_context_is_replaced_and_only_the_final_document_receives_the_shim() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    let blank = json!({"id":"C","parentId":"F","loaderId":"BLANK","url":"about:blank","securityOrigin":"https://ordinary.test"});
    state.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":blank}]);
    cdp.absorb(event("Page.frameNavigated", json!({"frame":blank})));
    child_context(&mut cdp, &state, 8, "BLANK-CONTEXT");
    cdp.webmcp_install_children();
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "BLANK-CONTEXT"));
    let placed = json!({"id":"C","parentId":"F","loaderId":"FINAL","url":"https://ordinary.test/child","securityOrigin":"https://ordinary.test"});
    state.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":placed}]);
    cdp.absorb(event("Runtime.executionContextDestroyed", json!({"executionContextId":8})));
    cdp.absorb(event("Page.frameNavigated", json!({"frame":placed})));
    child_context(&mut cdp, &state, 9, "FINAL-CONTEXT");
    cdp.webmcp_install_children();
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "FINAL-CONTEXT").count(), 1);
    assert!(cdp.webmcp_generations().iter().any(|entry| entry["frameId"] == "C" && entry["loaderId"] == "FINAL" && entry["uniqueId"] == "FINAL-CONTEXT"));
    cdp.webmcp_install_children();
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "FINAL-CONTEXT").count(), 1);
}

#[test]
fn child_context_created_before_live_placement_waits_and_never_installs_in_unplaced_or_protected_content() {
    for protected in [false, true] {
        let (mut cdp, state) = client();
        cdp.start_webmcp_for("H", true).unwrap();
        state.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":{"id":"C","parentId":"F","loaderId":"","url":"","securityOrigin":"null"}}]);
        let announced = json!({"id":"C","parentId":"F","loaderId":"CHILD","url":"https://ordinary.test/child","securityOrigin":"https://ordinary.test"});
        cdp.absorb(event("Page.frameNavigated", json!({"frame":announced})));
        child_context(&mut cdp, &state, 8, "PENDING-CHILD");
        cdp.webmcp_install_children();
        assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "PENDING-CHILD"));
        assert!(cdp.webmcp.arms["H"].pending_children.contains_key(&8));
        let final_frame = if protected { json!({"id":"C","parentId":"F","loaderId":"PROTECTED","url":"https://seven.sliccy.ai/child","securityOrigin":"https://seven.sliccy.ai"}) } else { announced };
        state.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":final_frame}]);
        cdp.absorb(event("Page.frameNavigated", json!({"frame":final_frame})));
        cdp.webmcp_install_children();
        assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "PENDING-CHILD").count(), usize::from(!protected));
        if protected { assert!(!cdp.webmcp_alive("H")); }
    }
}

#[test]
fn vetted_webmcp_installer_admits_only_guard_verified_documents() {
    let (mut cdp, state) = client();
    let bridge = cdp.start_webmcp_for("H", true).unwrap();
    assert!(bridge.starts_with("__wmcp_"));
    assert_eq!(bridge.len(), 39);
    assert!(state.borrow().api);
    assert_eq!(cdp.webmcp_pause_micros().len(), 1);
    assert!(cdp.webmcp_alive("H"));
    cdp.webmcp_auto_attach("H").unwrap();
    assert_eq!(crate::hash::sha256_hex(SHIM.as_bytes()), SHIM_HASH);
    for (method, params) in [("Page.addScriptToEvaluateOnNewDocument", json!({"source":"arbitrary"})), ("Runtime.addBinding", json!({"name":"arbitrary"})), ("Debugger.getScriptSource", json!({"scriptId":"SCRIPT"})), ("Debugger.evaluateOnCallFrame", json!({"callFrameId":"CALL","expression":"1"}))] {
        assert!(cdp.call(method, params, Some("H")).is_err());
        assert!(!state.borrow().sent.iter().any(|command| command["method"] == method));
    }
    cdp.stop_webmcp("H");
    assert!(!state.borrow().installed);
    assert!(!cdp.webmcp_alive("H"));
}

#[test]
fn late_install_does_not_reload_or_wait_for_a_script() {
    let (mut cdp, state) = client();
    state.borrow_mut().pause = false;
    cdp.start_webmcp("H").unwrap();
    assert!(state.borrow().api);
    assert!(state.borrow().sent.iter().all(|command| command["method"] != "Page.reload"));
}

#[test]
fn unverified_blank_documents_never_install_or_rearm() {
    for destination in ["about:blank", "chrome-error://chromewebdata/"] {
        let (mut cdp, state) = client();
        cdp.start_webmcp("H").unwrap();
        enter_document(&mut cdp, &state, frame(destination), "F");
        no_protected_api(&mut cdp, &state);
    }
}

#[test]
fn holder_navigation_history_and_protected_bfcache_cannot_inject_protected_documents() {
    for origin in ["https://seven.sliccy.ai/", "https://vault.sliccy.ai/", "https://catalog.test/"] {
        for variant in ["navigation", "history", "bfcache"] {
            let (mut cdp, state) = client();
            cdp.start_webmcp("H").unwrap();
            let tree = frame(origin);
            state.borrow_mut().api = false;
            cdp.absorb(event("Page.frameNavigated", json!({"type":if variant == "bfcache" {"BackForwardCacheRestore"} else {"Navigation"},"frame":tree["frameTree"]["frame"]})));
            enter_document(&mut cdp, &state, tree, "F");
            no_protected_api(&mut cdp, &state);
        }
    }
}

#[test]
fn holder_protected_bfcache_restore_has_no_shim_and_no_tool_access() {
    let (mut cdp, state) = client();
    cdp.start_webmcp("H").unwrap();
    state.borrow_mut().api = false;
    let tree = frame("https://seven.sliccy.ai/");
    cdp.absorb(event("Page.frameNavigated", json!({"type":"BackForwardCacheRestore","frame":tree["frameTree"]["frame"]})));
    no_protected_api(&mut cdp, &state);
}

#[test]
fn refused_blank_installer_never_arms_for_a_future_protected_document() {
    let (mut cdp, state) = client();
    state.borrow_mut().tree = frame("about:blank");
    assert!(cdp.start_webmcp("H").is_err());
    assert!(!state.borrow().api);
    assert!(cdp.webmcp_snapshot().is_empty());
    assert!(!state.borrow().sent.iter().any(|call| call["method"] == "Page.addScriptToEvaluateOnNewDocument"));
    enter_document(&mut cdp, &state, frame("https://seven.sliccy.ai/"), "F");
    no_protected_api(&mut cdp, &state);
    assert!(cdp.webmcp_snapshot().is_empty());
}

#[test]
fn holder_protected_child_frame_never_installs() {
    let (mut cdp, state) = client();
    cdp.start_webmcp("H").unwrap();
    let mut tree = frame("https://ordinary.test/");
    let mut child = frame("https://seven.sliccy.ai/")["frameTree"].clone();
    child["frame"]["id"] = json!("CHILD");
    child["frame"]["parentId"] = json!("F");
    tree["frameTree"]["childFrames"] = json!([child]);
    enter_document(&mut cdp, &state, tree, "CHILD");
    no_protected_api(&mut cdp, &state);
}

#[test]
fn holder_protected_oopif_is_detached_without_injection_or_tool_access() {
    let (mut cdp, state) = client();
    cdp.start_webmcp("H").unwrap();
    cdp.absorb(event("Target.attachedToTarget", json!({"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://seven.sliccy.ai/"},"waitingForDebugger":true})));
    assert!(cdp.take_webmcp_children().is_empty());
    assert!(cdp.session_blocked("CHILD"));
    assert!(!state.borrow().sent.iter().any(|command| command["sessionId"] == "CHILD" && command["method"] == "Page.addScriptToEvaluateOnNewDocument"));
    assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.modelContext.getTools()"}), Some("CHILD")).is_err());
    assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.modelContext.executeTool('secret', {})"}), Some("CHILD")).is_err());
    assert!(!state.borrow().sent.iter().any(|command| command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate"));
    assert!(!cdp.take_notices().iter().any(|notice| notice.contains("child tools are unavailable")));
}

#[test]
fn holder_protected_error_document_and_unverified_document_never_install() {
    for url in ["chrome-error://chromewebdata/", "about:blank"] {
        let (mut cdp, state) = client();
        cdp.start_webmcp("H").unwrap();
        let mut tree = frame(url);
        tree["frameTree"]["frame"]["securityOrigin"] = json!("null");
        tree["frameTree"]["frame"]["unreachableUrl"] = json!("https://vault.sliccy.ai/");
        enter_document(&mut cdp, &state, tree, "F");
        no_protected_api(&mut cdp, &state);
    }
}

#[test]
fn holder_commit_before_attach_refuses_installation() {
    let (mut cdp, state) = client();
    state.borrow_mut().tree = frame("https://seven.sliccy.ai/");
    assert!(cdp.start_webmcp("H").is_err());
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Page.addScriptToEvaluateOnNewDocument"));
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Runtime.evaluate"));
    no_protected_api(&mut cdp, &state);
}

#[test]
fn holder_context_loss_at_dispatch_cannot_install_in_a_new_protected_document() {
    let (mut cdp, state) = client();
    state.borrow_mut().race = true;
    assert!(cdp.start_webmcp("H").is_err());
    no_protected_api(&mut cdp, &state);
}

#[test]
fn missing_normal_pause_unconverted_pause_unknown_context_and_verification_error_fail_closed() {
    for variant in ["missing", "unconverted", "unknown", "error"] {
        let (mut cdp, state) = client();
        match variant {
            "missing" => state.borrow_mut().pause = false,
            "unconverted" => state.borrow_mut().unconverted = true,
            "unknown" => state.borrow_mut().current_frame = "UNKNOWN".into(),
            "error" => state.borrow_mut().fail_tree = true,
            _ => unreachable!(),
        }
        assert!(cdp.start_webmcp_for("H", true).is_err(), "{variant}");
        no_protected_api(&mut cdp, &state);
    }
}

#[test]
fn future_document_missing_pause_is_removed_even_without_context_events() {
    let (mut cdp, state) = client();
    cdp.start_webmcp("H").unwrap();
    let mut next = frame("https://ordinary.test/next");
    next["frameTree"]["frame"]["loaderId"] = json!("NEXT");
    cdp.absorb(event("Page.frameNavigated", json!({"frame":next["frameTree"]["frame"]})));
    cdp.webmcp.arms.get_mut("H").unwrap().pending_frames.get_mut("F").unwrap().1 = Instant::now();
    cdp.webmcp_sweep();
    assert!(!cdp.webmcp_alive("H"));
    assert!(!state.borrow().installed);
}

#[test]
fn known_ordinary_bfcache_restore_retains_only_the_same_accepted_realm() {
    let (mut cdp, _) = client();
    cdp.start_webmcp("H").unwrap();
    cdp.absorb(event("Runtime.executionContextsCleared", json!({})));
    cdp.absorb(event("Page.frameNavigated", json!({"type":"BackForwardCacheRestore","frame":frame("https://ordinary.test/")["frameTree"]["frame"]})));
    cdp.absorb(context("F", "https://ordinary.test"));
    let arm = cdp.webmcp.arms.get("H").unwrap();
    assert!(arm.pending_frames.is_empty());
    assert!(arm.pending_contexts.is_empty());
    cdp.webmcp_sweep();
    assert!(cdp.webmcp_alive("H"));
}

#[test]
fn changed_loader_and_context_destroyed_before_pause_are_denied() {
    for destroyed in [false, true] {
        let (mut cdp, state) = client();
        cdp.start_webmcp("H").unwrap();
        state.borrow_mut().api = false;
        state.borrow_mut().tree["frameTree"]["frame"]["loaderId"] = json!("NEXT");
        if destroyed { cdp.absorb(event("Runtime.executionContextDestroyed", json!({"executionContextId":7}))); }
        cdp.absorb(paused());
        assert!(!state.borrow().api, "stale root context must not get another install");
        cdp.stop_webmcp("H");
        no_protected_api(&mut cdp, &state);
    }
}

#[test]
fn readiness_waits_for_pending_top_frame_generations() {
    let (mut cdp, _) = client();
    cdp.start_webmcp("H").unwrap();
    assert!(cdp.webmcp_settled());
    let mut next = frame("https://ordinary.test/next");
    next["frameTree"]["frame"]["loaderId"] = json!("NEXT");
    cdp.absorb(event("Page.frameNavigated", json!({"frame":next["frameTree"]["frame"]})));
    assert!(!cdp.webmcp_settled());
    cdp.block("H");
    assert!(cdp.webmcp_settled());
}

#[test]
fn detached_frame_clears_its_contexts_without_affecting_the_top_generation() {
    let (mut cdp, _) = client();
    cdp.start_webmcp("H").unwrap();
    cdp.absorb(context("REMOVED", "https://ordinary.test"));
    cdp.absorb(event("Page.frameNavigated", json!({"frame":{"id":"REMOVED","loaderId":"CHILD","url":"https://ordinary.test/frame","securityOrigin":"https://ordinary.test","parentId":"F"}})));
    cdp.absorb(event("Page.frameDetached", json!({"frameId":"REMOVED"})));
    assert!(cdp.webmcp.arms["H"].pending_frames.is_empty());
    assert!(!cdp.webmcp.contexts.values().any(|context| context.frame == "REMOVED"));
}

#[test]
fn ordinary_documents_without_holder_pay_no_debugger_or_init_overhead() {
    let (mut cdp, state) = client();
    cdp.call("Runtime.evaluate", json!({"expression":"1"}), Some("H")).unwrap();
    assert!(state.borrow().sent.iter().all(|command| !command["method"].as_str().unwrap().starts_with("Debugger.") && command["method"] != "Page.addScriptToEvaluateOnNewDocument"));
}
