use super::*;
use crate::cdp::Transport;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Default)]
struct BackendState {
    inbox: VecDeque<String>,
    sent: Vec<Value>,
    source: String,
    tree: Value,
    current_frame: String,
    installed: bool,
    permitted: bool,
    api: bool,
    pause: bool,
    race: bool,
    forged: bool,
    fail_tree: bool,
    context_race: String,
}

struct Backend(Rc<RefCell<BackendState>>);

fn event(method: &str, params: Value) -> Value {
    json!({"method":method,"sessionId":"H","params":params})
}

fn paused() -> Value {
    event("Debugger.paused", json!({"callFrames":[{"callFrameId":"CALL","functionName":GATE,"location":{"scriptId":"SCRIPT","lineNumber":0,"columnNumber":0}}]}))
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
        let params = &command["params"];
        let result = match method {
            "Target.getTargets" => json!({"targetInfos":[{"targetId":"T","type":"page","url":state.tree["frameTree"]["frame"]["url"]}]}),
            "Page.getFrameTree" => {
                if state.fail_tree && state.installed {
                    state.inbox.push_back(json!({"id":command["id"],"error":{"message":"verification failed"}}).to_string());
                    return Ok(());
                }
                state.tree.clone()
            }
            "Runtime.enable" if command["sessionId"] == "H" => {
                let frame = state.current_frame.clone();
                state.inbox.push_back(context(&frame, "https://ordinary.test").to_string());
                json!({})
            }
            "Runtime.enable" => {
                state.inbox.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":command["sessionId"],"params":{"context":{"id":1,"uniqueId":"PRIVATE_CONTEXT","origin":"https://ordinary.test","name":"","auxData":{"frameId":"F","isDefault":true}}}}).to_string());
                json!({})
            }
            "Page.addScriptToEvaluateOnNewDocument" => {
                state.source = params["source"].as_str().unwrap().to_string();
                state.installed = true;
                if state.pause && params["runImmediately"] == true {
                    state.inbox.push_back(event("Debugger.scriptParsed", json!({"scriptId":"SCRIPT","executionContextId":7})).to_string());
                    state.inbox.push_back(paused().to_string());
                }
                json!({"identifier":"INIT"})
            }
            "Debugger.getScriptSource" => json!({"scriptSource":if state.forged { "function __slicc_webmcp_gate() { debugger; }" } else { &state.source }}),
            "Debugger.evaluateOnCallFrame" => {
                state.permitted = params["expression"].as_str().unwrap().ends_with("true");
                if state.context_race == "permit" && state.permitted {
                    let mut changed = context("F", "https://ordinary.test");
                    changed["params"]["context"]["uniqueId"] = json!("REUSED_CONTEXT");
                    state.inbox.push_back(changed.to_string());
                }
                if state.race && state.permitted {
                    state.tree = frame("https://seven.sliccy.ai/");
                    let changed = state.tree["frameTree"]["frame"].clone();
                    state.inbox.push_back(event("Page.frameNavigated", json!({"frame":changed})).to_string());
                }
                json!({"result":{"type":"boolean","value":state.permitted}})
            }
            "Debugger.resume" => {
                state.api = state.permitted;
                if state.context_race == "resume" && state.permitted {
                    let mut changed = context("F", "https://ordinary.test");
                    changed["params"]["context"]["uniqueId"] = json!("REUSED_CONTEXT");
                    state.inbox.push_back(changed.to_string());
                    state.api = false;
                }
                state.permitted = false;
                json!({})
            }
            "Page.removeScriptToEvaluateOnNewDocument" => {
                state.installed = false;
                json!({})
            }
            "Target.detachFromTarget" => {
                if state.permitted { state.api = true; }
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

#[test]
fn numeric_context_reuse_during_permission_or_resume_revokes_the_installer() {
    for stage in ["permit", "resume"] {
        let (mut cdp, state) = client();
        state.borrow_mut().context_race = stage.to_string();
        assert!(cdp.start_webmcp("H").is_err(), "{stage}");
        no_protected_api(&mut cdp, &state);
        assert!(cdp.webmcp_snapshot().is_empty());
    }
}

#[test]
fn quarantined_future_installer_does_not_run_in_the_existing_document() {
    for destination in ["https://ordinary.test/", "https://seven.sliccy.ai/"] {
        let (mut cdp, state) = client();
        state.borrow_mut().tree = frame("about:blank");
        cdp.webmcp.quarantined.insert("H".into());
        cdp.webmcp_install("H", true, None).unwrap();
        assert!(!state.borrow().api);
        assert!(cdp.webmcp_snapshot().is_empty());
        assert!(state.borrow().sent.iter().any(|call| call["method"] == "Page.addScriptToEvaluateOnNewDocument" && call["params"]["runImmediately"] == false));
        cdp.absorb(event("Page.frameNavigated", json!({"frame":frame(destination)["frameTree"]["frame"]})));
        enter_document(&mut cdp, &state, frame(destination), "F");
        if destination.contains("seven") { no_protected_api(&mut cdp, &state); }
        else { assert!(state.borrow().api); assert_eq!(cdp.take_webmcp_children().len(), 1); }
    }
}

#[test]
fn readiness_waits_until_a_quarantined_child_gate_is_decided() {
    for destination in ["https://ordinary.test/child", "https://seven.sliccy.ai/"] {
        let (mut cdp, state) = client();
        assert!(cdp.webmcp_settled());
        state.borrow_mut().tree = frame("about:blank");
        cdp.webmcp.quarantined.insert("H".into());
        cdp.webmcp_install("H", true, None).unwrap();
        assert!(!cdp.webmcp_settled());
        assert!(cdp.webmcp_snapshot().is_empty());
        cdp.absorb(event("Page.frameNavigated", json!({"frame":frame(destination)["frameTree"]["frame"]})));
        enter_document(&mut cdp, &state, frame(destination), "F");
        assert!(cdp.webmcp_settled(), "{destination}");
        assert_eq!(cdp.webmcp_snapshot().len(), usize::from(!destination.contains("seven")));
    }
}

#[test]
fn readiness_waits_for_pending_frame_generations_of_an_admitted_holder() {
    let (mut cdp, state) = client();
    cdp.start_webmcp("H").unwrap();
    assert!(cdp.webmcp_settled());
    let mut next = frame("https://ordinary.test/next");
    next["frameTree"]["frame"]["loaderId"] = json!("NEXT");
    cdp.absorb(event("Page.frameNavigated", json!({"frame":next["frameTree"]["frame"]})));
    assert!(!cdp.webmcp_settled());
    enter_document(&mut cdp, &state, next, "F");
    assert!(cdp.webmcp_alive("H"));
    assert!(cdp.webmcp_settled());
    let mut fresh = context("F", "https://ordinary.test");
    fresh["params"]["context"]["uniqueId"] = json!("FRESH");
    cdp.absorb(fresh);
    assert!(!cdp.webmcp_settled());
    cdp.guard_sessions.insert("H".into());
    assert!(cdp.webmcp_settled());
    cdp.guard_sessions.remove("H");
    cdp.block("H");
    assert!(cdp.webmcp_settled());
}

fn enter_document(cdp: &mut Cdp, state: &Rc<RefCell<BackendState>>, tree: Value, frame: &str) {
    {
        let mut backend = state.borrow_mut();
        backend.tree = tree;
        backend.api = false;
        backend.permitted = false;
        backend.current_frame = frame.to_string();
    }
    cdp.absorb(context(frame, "https://ordinary.test"));
    cdp.absorb(event("Debugger.scriptParsed", json!({"scriptId":"SCRIPT","executionContextId":7})));
    cdp.absorb(paused());
}

fn no_protected_api(cdp: &mut Cdp, state: &Rc<RefCell<BackendState>>) {
    assert!(!state.borrow().api, "shim executed in a document the guard did not verify");
    assert!(!state.borrow().installed, "init script was not removed");
    cdp.block("H");
    for expression in ["document.modelContext.getTools()", "document.modelContext.executeTool('secret', {})"] {
        assert!(cdp.call("Runtime.evaluate", json!({"expression":expression}), Some("H")).is_err());
    }
}

#[test]
fn vetted_webmcp_installer_admits_only_guard_verified_documents() {
    let (mut cdp, state) = client();
    let bridge = cdp.start_webmcp("H").unwrap();
    assert!(bridge.starts_with("__wmcp_"));
    assert_eq!(bridge.len(), 39);
    assert!(state.borrow().api);
    assert_eq!(cdp.webmcp_pause_micros().len(), 1);
    assert!(cdp.webmcp_alive("H"));
    cdp.webmcp_auto_attach("H").unwrap();
    assert_eq!(crate::hash::sha256_hex(SHIM.as_bytes()), SHIM_HASH);
    assert!(cdp.call("Page.addScriptToEvaluateOnNewDocument", json!({"source":"arbitrary"}), Some("H")).is_err());
    cdp.stop_webmcp("H");
    assert!(!state.borrow().installed);
    assert!(!cdp.webmcp_alive("H"));
}

#[test]
fn paused_child_installation_covers_the_existing_verified_document() {
    let (mut cdp, state) = client();
    let seed = state.borrow().tree.clone();
    cdp.webmcp_install("H", true, Some(&seed)).unwrap();
    assert!(state.borrow().api);
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Page.addScriptToEvaluateOnNewDocument" && command["params"]["runImmediately"] == true));
}

#[test]
fn initial_blank_child_remains_inert_and_replaces_the_removed_script_before_resume() {
    let (mut cdp, state) = client();
    cdp.start_webmcp("H").unwrap();
    let mut tree = frame("https://ordinary.test/");
    let child = json!({"id":"CHILD","parentId":"F","url":"about:blank","securityOrigin":"https://ordinary.test","loaderId":"BLANK"});
    tree["frameTree"]["childFrames"] = json!([{"frame":child}]);
    state.borrow_mut().tree = tree.clone();
    cdp.absorb(event("Page.frameNavigated", json!({"frame":child})));
    assert!(cdp.webmcp_alive("H"));
    enter_document(&mut cdp, &state, tree, "CHILD");
    assert!(!state.borrow().api);
    assert!(state.borrow().installed);
    assert!(cdp.webmcp_alive("H"));
    let backend = state.borrow();
    let removed = backend.sent.iter().rposition(|command| command["method"] == "Page.removeScriptToEvaluateOnNewDocument").unwrap();
    let replaced = backend.sent.iter().rposition(|command| command["method"] == "Page.addScriptToEvaluateOnNewDocument" && command["params"]["runImmediately"] == false).unwrap();
    let resumed = backend.sent.iter().rposition(|command| command["method"] == "Debugger.resume").unwrap();
    assert!(removed < replaced && replaced < resumed);
}

#[test]
fn an_unverified_blank_top_document_cannot_rearm() {
    let (mut cdp, state) = client();
    cdp.start_webmcp("H").unwrap();
    enter_document(&mut cdp, &state, frame("about:blank"), "F");
    no_protected_api(&mut cdp, &state);
}

#[test]
fn holder_navigation_and_history_cannot_inject_protected_documents() {
    for origin in ["https://seven.sliccy.ai/", "https://vault.sliccy.ai/", "https://catalog.test/"] {
        for _variant in ["navigation", "history"] {
            let (mut cdp, state) = client();
            cdp.start_webmcp("H").unwrap();
            enter_document(&mut cdp, &state, frame(origin), "F");
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
fn holder_protected_child_frame_never_passes_the_gate() {
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
fn holder_protected_oopif_stays_quarantined_and_receives_no_init_script() {
    let (mut cdp, state) = client();
    cdp.start_webmcp("H").unwrap();
    cdp.absorb(event("Target.attachedToTarget", json!({"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://seven.sliccy.ai/"},"waitingForDebugger":true})));
    assert!(cdp.take_webmcp_children().is_empty());
    assert!(cdp.session_blocked("CHILD"));
    assert!(!state.borrow().sent.iter().any(|command| command["sessionId"] == "CHILD" && command["method"] == "Page.addScriptToEvaluateOnNewDocument"));
    assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.modelContext.getTools()"}), Some("CHILD")).is_err());
    assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.modelContext.executeTool('secret', {})"}), Some("CHILD")).is_err());
}

#[test]
fn holder_protected_error_document_and_unverified_document_never_pass_the_gate() {
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
    no_protected_api(&mut cdp, &state);
}

#[test]
fn holder_generation_change_between_verification_and_resume_fails_closed() {
    let (mut cdp, state) = client();
    state.borrow_mut().race = true;
    assert!(cdp.start_webmcp("H").is_err());
    no_protected_api(&mut cdp, &state);
    let commands = &state.borrow().sent;
    let denied = commands.iter().position(|command| command["method"] == "Debugger.evaluateOnCallFrame" && command["params"]["expression"] == format!("{PERMIT} = false")).unwrap();
    let detach = commands.iter().position(|command| command["method"] == "Target.detachFromTarget").unwrap();
    assert!(denied < detach);
}

#[test]
fn holder_missing_pause_forged_gate_unknown_context_and_verification_error_fail_closed() {
    for variant in ["missing", "forged", "unknown", "error"] {
        let (mut cdp, state) = client();
        match variant {
            "missing" => state.borrow_mut().pause = false,
            "forged" => state.borrow_mut().forged = true,
            "unknown" => state.borrow_mut().current_frame = "UNKNOWN".into(),
            "error" => state.borrow_mut().fail_tree = true,
            _ => unreachable!(),
        }
        assert!(cdp.start_webmcp("H").is_err(), "{variant}");
        no_protected_api(&mut cdp, &state);
    }
}

#[test]
fn holder_future_document_missing_pause_is_removed_even_without_context_events() {
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
fn holder_known_ordinary_bfcache_restore_keeps_an_inert_installer() {
    let (mut cdp, state) = client();
    cdp.start_webmcp("H").unwrap();
    cdp.absorb(event("Runtime.executionContextsCleared", json!({})));
    cdp.absorb(event("Page.frameNavigated", json!({"type":"BackForwardCacheRestore","frame":state.borrow().tree["frameTree"]["frame"]})));
    cdp.absorb(context("F", "https://ordinary.test"));
    let arm = cdp.webmcp.arms.get("H").unwrap();
    assert!(arm.pending_frames.is_empty());
    assert!(arm.pending_contexts.is_empty());
    cdp.webmcp_sweep();
    assert!(cdp.webmcp_alive("H"));
}

#[test]
fn holder_changed_loader_and_context_destroyed_before_pause_are_denied() {
    for destroyed in [false, true] {
        let (mut cdp, state) = client();
        cdp.start_webmcp("H").unwrap();
        state.borrow_mut().api = false;
        state.borrow_mut().tree["frameTree"]["frame"]["loaderId"] = json!("NEXT");
        if destroyed { cdp.absorb(event("Runtime.executionContextDestroyed", json!({"executionContextId":7}))); }
        cdp.absorb(paused());
        no_protected_api(&mut cdp, &state);
    }
}

#[test]
fn holder_detached_frame_clears_its_missing_pause_deadline() {
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
