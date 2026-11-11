use super::*;
use crate::cdp::Transport;
use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;

fn webmcp_shim_install(source: &str) -> bool {
    source.starts_with("const __wmcp_")
}

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
    attach_on_auto_attach: bool,
    buffer_child_lifecycle_on_auto_attach: bool,
    refuse_auto_attach: bool,
    route: Option<String>,
    child_policy: Option<String>,
    load_network_headers: Option<Value>,
    native_policy: Option<bool>,
    version_mode: Option<String>,
    oopif_tree: Option<Value>,
    get_target_parent: Option<String>,
    omit_get_target_parent: bool,
    suppress_runtime_child: bool,
    iframe_targets: Option<Vec<Value>>,
    chrome_shaped_lifecycle: bool,
    enabled_page: BTreeSet<String>,
    enabled_runtime: BTreeSet<String>,
    deferred_lifecycle: VecDeque<String>,
}

impl BackendState {
    fn listed_iframes(&self) -> Vec<Value> {
        if let Some(targets) = &self.iframe_targets { return targets.clone(); }
        let Some(tree) = &self.oopif_tree else { return Vec::new(); };
        let url = tree["frameTree"]["frame"]["url"].as_str().filter(|url| url.starts_with("http")).unwrap_or("https://foreign.test/cross");
        let mut info = json!({"targetId":"IFRAME","type":"iframe","url":url});
        if !self.omit_get_target_parent {
            info["parentFrameId"] = json!(self.get_target_parent.clone().unwrap_or_else(|| "F".into()));
        }
        vec![info]
    }

    fn child_session(session: &str) -> bool {
        session.starts_with("CHILD") || session == "TOUCH-IFRAME"
    }

    fn push_lifecycle(&mut self, session: &str, event: Value) {
        let method = event.get("method").and_then(Value::as_str).unwrap_or("");
        let page = method.starts_with("Page.");
        let runtime = method.starts_with("Runtime.");
        let withheld = self.chrome_shaped_lifecycle
            && Self::child_session(session)
            && ((page && !self.enabled_page.contains(session)) || (runtime && !self.enabled_runtime.contains(session)));
        if withheld {
            self.deferred_lifecycle.push_back(event.to_string());
        } else {
            self.inbox.push_back(event.to_string());
        }
    }

    fn flush_lifecycle(&mut self, session: &str) {
        if !self.chrome_shaped_lifecycle { return; }
        let mut kept = VecDeque::new();
        while let Some(text) = self.deferred_lifecycle.pop_front() {
            let Ok(event) = serde_json::from_str::<Value>(&text) else { continue; };
            let owner = event.get("sessionId").and_then(Value::as_str).unwrap_or("");
            let method = event.get("method").and_then(Value::as_str).unwrap_or("");
            let ready = owner == session && (
                (method.starts_with("Page.") && self.enabled_page.contains(session))
                    || (method.starts_with("Runtime.") && self.enabled_runtime.contains(session))
            );
            if ready {
                self.inbox.push_back(text);
            } else {
                kept.push_back(text);
            }
        }
        self.deferred_lifecycle = kept;
    }
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
    event_on("H", method, params)
}

fn event_on(session: &str, method: &str, params: Value) -> Value {
    json!({"method":method,"sessionId":session,"params":params})
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
            "Target.getTargets" => {
                let url = state.tree["frameTree"]["frame"]["url"].clone();
                let mut infos = vec![json!({"targetId":"T","type":"page","url":url}), json!({"targetId":"T2","type":"page","url":url})];
                if command["params"].get("filter").is_none() { infos.extend(state.listed_iframes()); }
                json!({"targetInfos":infos})
            }
            "Page.getFrameTree" => {
                if state.fail_tree && state.installed {
                    state.inbox.push_back(json!({"id":command["id"],"error":{"message":"verification failed"}}).to_string());
                    return Ok(());
                }
                let session = command["sessionId"].as_str().unwrap_or("");
                if session.starts_with("CHILD") || session == "TOUCH-IFRAME" {
                    if let Some(tree) = state.oopif_tree.clone() { tree } else { state.tree.clone() }
                } else {
                    state.tree.clone()
                }
            }
            "Target.attachToTarget" => {
                let target = command["params"]["targetId"].as_str().unwrap_or("");
                json!({"sessionId": format!("TOUCH-{target}")})
            }
            "Page.enable" => {
                let session = command["sessionId"].as_str().unwrap_or("H");
                state.enabled_page.insert(session.to_string());
                if state.chrome_shaped_lifecycle && BackendState::child_session(session) {
                    if let Some(tree) = state.oopif_tree.clone() {
                        let frame = tree["frameTree"]["frame"].clone();
                        let url = frame.get("url").and_then(Value::as_str).unwrap_or("");
                        if url::Url::parse(url).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https")) {
                            state.push_lifecycle(session, json!({"method":"Page.frameNavigated","sessionId":session,"params":{"frame":frame}}));
                        }
                    }
                    state.flush_lifecycle(session);
                }
                json!({})
            }
            "Runtime.enable" => {
                let session = command["sessionId"].as_str().unwrap_or("H");
                state.enabled_runtime.insert(session.to_string());
                if session == "CHILD" || session == "CHILD-NEW" || session == "CHILD-OLD" || session == "TOUCH-IFRAME" {
                    if !state.suppress_runtime_child {
                        if let Some(child) = state.runtime_child.clone().or_else(|| {
                            state.oopif_tree.as_ref().map(|tree| {
                                let frame = &tree["frameTree"]["frame"];
                                json!({"id":9,"uniqueId":"CROSS-CTX","origin":frame["securityOrigin"],"auxData":{"frameId":frame["id"],"isDefault":true}})
                            })
                        }) {
                            state.push_lifecycle(session, json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":child}}));
                        }
                    }
                    state.flush_lifecycle(session);
                    json!({})
                } else {
                    let mut created = context(&state.current_frame, "https://ordinary.test");
                    created["sessionId"] = command["sessionId"].clone();
                    state.inbox.push_back(created.to_string());
                    let child = state.runtime_child.clone().or_else(|| state.initial_child.then(|| json!({"id":8,"uniqueId":"CHILD-CONTEXT","origin":state.tree["frameTree"]["childFrames"][0]["frame"]["securityOrigin"],"auxData":{"frameId":"C","isDefault":true}})));
                    if let Some(child) = child {
                        state.inbox.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":command["sessionId"],"params":{"context":child}}).to_string());
                    }
                    json!({})
                }
            }
            "Debugger.setInstrumentationBreakpoint" => {
                state.installed = true;
                json!({"breakpointId":"INSTRUMENT"})
            }
            "Page.reload" => {
                if state.pause {
                    let session = command["sessionId"].as_str().unwrap_or("H");
                    state.inbox.push_back(event_on(session, "Debugger.scriptParsed", json!({"scriptId":"SCRIPT","executionContextId":7})).to_string());
                    let mut pause = event_on(session, "Debugger.paused", json!({"reason":"other","callFrames":[{"callFrameId":"CALL","functionName":"","location":{"scriptId":"SCRIPT","lineNumber":0,"columnNumber":0}}]}));
                    if state.unconverted { pause["params"]["reason"] = json!("instrumentation"); }
                    state.inbox.push_back(pause.to_string());
                }
                json!({})
            }
            "Runtime.evaluate" if command["params"]["expression"].as_str().is_some_and(webmcp_shim_install) => {
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
            "Page.getPermissionsPolicyState" => match state.native_policy {
                Some(allowed) => json!({"states":[{"feature":"tools","allowed":allowed}]}),
                None => {
                    state.inbox.push_back(json!({"id":command["id"],"error":{"message":"Page.getPermissionsPolicyState wasn't found"}}).to_string());
                    return Ok(());
                }
            },
            "Network.loadNetworkResource" => match state.load_network_headers.as_ref() {
                Some(Value::Null) => json!({"resource":{"success":true}}),
                Some(headers) => {
                    let status = headers.get("__status").and_then(Value::as_u64).unwrap_or(200);
                    let mut clean = headers.clone();
                    if let Some(object) = clean.as_object_mut() { object.remove("__status"); }
                    json!({"resource":{"success":true,"httpStatusCode":status,"headers":clean}})
                }
                None => json!({"resource":{"success":true,"httpStatusCode":200,"headers":{"permissions-policy":state.child_policy.clone().unwrap_or_else(|| "tools=*".into())}}}),
            },
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
            "Target.setAutoAttach" => {
                let auto_attach = command["params"]["autoAttach"].as_bool() == Some(true);
                let filter = command["params"].get("filter").and_then(Value::as_array);
                if !auto_attach && filter.is_some_and(|items| !items.is_empty()) {
                    state.inbox.push_back(json!({"id":command["id"],"error":{"message":"Target filter should be empty when disabling auto-attach"}}).to_string());
                    return Ok(());
                }
                if state.refuse_auto_attach {
                    state.inbox.push_back(json!({"id":command["id"],"error":{"message":"slicc: refused: Target.setAutoAttach is not supported: attach to each target"}}).to_string());
                    return Ok(());
                }
                if state.attach_on_auto_attach {
                    let parent = command["sessionId"].as_str().unwrap_or("H");
                    state.inbox.push_back(json!({"method":"Target.attachedToTarget","sessionId":parent,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}).to_string());
                }
                if state.buffer_child_lifecycle_on_auto_attach {
                    let parent = command["sessionId"].as_str().unwrap_or("H");
                    let frame = state.oopif_tree.as_ref().map(|tree| tree["frameTree"]["frame"].clone()).unwrap_or_else(|| json!({"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}));
                    state.inbox.push_back(json!({"method":"Target.attachedToTarget","sessionId":parent,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":frame["url"].as_str().unwrap_or("https://foreign.test/cross")},"waitingForDebugger":false}}).to_string());
                    state.push_lifecycle("CHILD", json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":frame.clone()}}));
                    state.push_lifecycle("CHILD", json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":frame["securityOrigin"],"auxData":{"frameId":frame["id"],"isDefault":true}}}}));
                }
                json!({})
            }
            "Browser.getVersion" => {
                match state.version_mode.as_deref() {
                    Some("timeout") => {
                        state.inbox.push_back(json!({"id":command["id"],"error":{"message":"Browser.getVersion timed out"}}).to_string());
                        return Ok(());
                    }
                    Some("error") => {
                        state.inbox.push_back(json!({"id":command["id"],"error":{"message":"Backend unavailable"}}).to_string());
                        return Ok(());
                    }
                    Some("empty") => json!({"product":"Chrome/156.0.0.0","slicc":{"route":""}}),
                    Some("number") => json!({"product":"Chrome/156.0.0.0","slicc":{"route":1}}),
                    Some("array") => json!({"product":"Chrome/156.0.0.0","slicc":{"route":[]}}),
                    Some("nonobject") => json!("Chrome/156.0.0.0"),
                    _ => {
                        if let Some(route) = state.route.clone() {
                            json!({"product":"Chrome/156.0.0.0","slicc":{"route":route}})
                        } else {
                            json!({"product":"Chrome/156.0.0.0"})
                        }
                    }
                }
            }
            "Target.getTargetInfo" => {
                let id = command["params"]["targetId"].as_str().unwrap_or("IFRAME");
                let mut info = json!({"targetId":id,"type":"iframe","url":"https://foreign.test/cross"});
                if !state.omit_get_target_parent {
                    let parent = state.get_target_parent.clone().unwrap_or_else(|| "F".into());
                    info["parentFrameId"] = json!(parent);
                }
                json!({"targetInfo":info})
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
    let installed = || state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)).cloned().collect::<Vec<_>>();
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
        if protected {
            assert!(cdp.webmcp_alive("H"), "protected subframe navigation must not revoke the root holder");
            assert!(!cdp.webmcp.arms["H"].revoked);
        }
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
    let before = state.borrow().sent.len();
    let mut tree = frame("https://ordinary.test/");
    let mut child = frame("https://seven.sliccy.ai/")["frameTree"].clone();
    child["frame"]["id"] = json!("CHILD");
    child["frame"]["parentId"] = json!("F");
    tree["frameTree"]["childFrames"] = json!([child]);
    enter_document(&mut cdp, &state, tree, "CHILD");
    assert!(!state.borrow().api, "shim executed in a document the guard did not verify");
    assert!(!cdp.webmcp.arms["H"].revoked, "protected subframe navigation must not revoke the root");
    assert!(!state.borrow().sent[before..].iter().any(|command| {
        command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
    }));
    cdp.block("H");
    for expression in ["document.modelContext.getTools()", "document.modelContext.executeTool('secret', {})"] {
        assert!(cdp.call("Runtime.evaluate", json!({"expression":expression}), Some("H")).is_err());
    }
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

fn no_debugger_published(cdp: &Cdp) {
    assert!(cdp.events().iter().all(|event| !event["method"].as_str().unwrap_or("").starts_with("Debugger.")), "{:?}", cdp.events());
}

#[test]
fn script_id_cap_refuses_new_ids_resumes_nothing_published_and_stays_bounded() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    let before = state.borrow().sent.iter().filter(|command| command["method"] == "Debugger.resume").count();
    cdp.webmcp.id_cap_override = 3;
    for i in 0..8 {
        cdp.absorb(event("Debugger.scriptParsed", json!({"scriptId":format!("S{i}"),"executionContextId":7,"url":format!("https://ordinary.test/{i}.js")})));
    }
    assert!(cdp.webmcp.session_script_count("H") <= 3, "{}", cdp.webmcp.session_script_count("H"));
    assert!(cdp.webmcp.session_evaluation_count("H") <= 3, "{}", cdp.webmcp.session_evaluation_count("H"));
    let events = cdp.events().len();
    cdp.absorb(event("Debugger.scriptParsed", json!({"scriptId":"OVERFLOW","executionContextId":7,"url":"https://ordinary.test/overflow.js"})));
    cdp.absorb(paused_with("OVERFLOW"));
    assert!(cdp.webmcp.session_script_count("H") <= 3, "{}", cdp.webmcp.session_script_count("H"));
    assert!(state.borrow().sent.iter().filter(|command| command["method"] == "Debugger.resume").count() > before);
    assert_eq!(cdp.events().len(), events);
    no_debugger_published(&cdp);
    assert!(!cdp.webmcp_is_paused("H"));
}

fn paused_with(script: &str) -> Value {
    event("Debugger.paused", json!({"reason":"other","callFrames":[{"callFrameId":"CALL","functionName":"","location":{"scriptId":script,"lineNumber":0,"columnNumber":0}}]}))
}

#[test]
fn pending_gates_count_cap_resumes_privately_publishes_nothing_and_stays_bounded() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    cdp.webmcp.pending_count_override = 2;
    cdp.webmcp.handling_gate = true;
    let before = state.borrow().sent.iter().filter(|command| command["method"] == "Debugger.resume").count();
    let events = cdp.events().len();
    for i in 0..5 {
        cdp.absorb(paused_with(&format!("SCRIPT-{i}")));
    }
    assert!(cdp.webmcp.pending_gates.len() <= 2, "{}", cdp.webmcp.pending_gates.len());
    assert!(cdp.webmcp.pending_gate_bytes <= cdp.webmcp.pending_bytes_cap());
    assert!(state.borrow().sent.iter().filter(|command| command["method"] == "Debugger.resume").count() > before);
    assert_eq!(cdp.events().len(), events);
    no_debugger_published(&cdp);
    cdp.webmcp.handling_gate = false;
    cdp.webmcp_cancel_queued("H");
    assert!(cdp.webmcp.pending_gates.is_empty());
    assert_eq!(cdp.webmcp.pending_gate_bytes, 0);
}

#[test]
fn pending_gates_byte_cap_resumes_privately_publishes_nothing_and_stays_bounded() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    let sample = paused_with("SCRIPT");
    let sample_bytes = sample.to_string().len();
    cdp.webmcp.pending_bytes_override = sample_bytes.saturating_mul(2).max(1);
    cdp.webmcp.pending_count_override = 64;
    cdp.webmcp.handling_gate = true;
    let before = state.borrow().sent.iter().filter(|command| command["method"] == "Debugger.resume").count();
    let events = cdp.events().len();
    for i in 0..6 {
        let mut pause = paused_with(&format!("SCRIPT-{i}"));
        pause["params"]["callFrames"][0]["functionName"] = json!("x".repeat(32));
        cdp.absorb(pause);
    }
    assert!(cdp.webmcp.pending_gates.len() <= cdp.webmcp.pending_count_cap());
    assert!(cdp.webmcp.pending_gate_bytes <= cdp.webmcp.pending_bytes_cap(), "{} > {}", cdp.webmcp.pending_gate_bytes, cdp.webmcp.pending_bytes_cap());
    assert!(state.borrow().sent.iter().filter(|command| command["method"] == "Debugger.resume").count() > before);
    assert_eq!(cdp.events().len(), events);
    no_debugger_published(&cdp);
    cdp.webmcp.handling_gate = false;
    cdp.stop_webmcp("H");
}

#[test]
fn saturated_id_cap_instrumentation_for_unknown_script_resumes_without_install_or_publish() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    cdp.webmcp.arms.get_mut("H").unwrap().accepted.clear();
    cdp.webmcp.id_cap_override = 2;
    for i in 0..6 {
        cdp.absorb(event("Debugger.scriptParsed", json!({"scriptId":format!("FLOOD-{i}"),"executionContextId":7,"url":format!("https://ordinary.test/flood-{i}.js")})));
    }
    assert_eq!(cdp.webmcp.session_script_count("H"), 2);
    assert!(!cdp.webmcp.scripts.contains_key(&("H".into(), "UNRECORDED".into())));
    let resumes = state.borrow().sent.iter().filter(|command| command["method"] == "Debugger.resume").count();
    let pauses = state.borrow().sent.iter().filter(|command| command["method"] == "Debugger.pause").count();
    let installs = state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)).count();
    let api = state.borrow().api;
    let events = cdp.events().len();
    let mut pause = paused_with("UNRECORDED");
    pause["params"]["reason"] = json!("instrumentation");
    pause["params"]["data"] = json!({"scriptId":"UNRECORDED"});
    cdp.absorb(pause);
    assert!(state.borrow().sent.iter().filter(|command| command["method"] == "Debugger.resume").count() > resumes);
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Debugger.pause").count(), pauses);
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)).count(), installs);
    assert_eq!(state.borrow().api, api);
    assert!(cdp.webmcp.arms["H"].conversion.is_none());
    assert!(cdp.webmcp.arms["H"].accepted.is_empty());
    assert_eq!(cdp.events().len(), events);
    no_debugger_published(&cdp);
    assert!(!cdp.webmcp_is_paused("H"));
}

#[test]
fn flooding_session_does_not_starve_another_holders_root_install() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    cdp.webmcp.id_cap_override = 2;
    for i in 0..8 {
        cdp.absorb(event("Debugger.scriptParsed", json!({"scriptId":format!("FLOOD-{i}"),"executionContextId":7,"url":format!("https://ordinary.test/flood-{i}.js")})));
    }
    assert_eq!(cdp.webmcp.session_script_count("H"), 2);
    assert!(cdp.webmcp.scripts.len() >= 2);
    cdp.session_targets.insert("H2".into(), "T2".into());
    cdp.guards.insert("T2".into(), "G2".into());
    cdp.guard_sessions.insert("G2".into());
    cdp.targets.insert("T2".into(), json!({"targetId":"T2","type":"page","url":"https://ordinary.test/"}));
    cdp.main_commit("G2", &state.borrow().tree["frameTree"]["frame"]);
    let installs_before = state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)).count();
    assert!(cdp.start_webmcp_for("H2", true).is_ok());
    assert!(cdp.webmcp.arms.get("H2").is_some_and(|arm| !arm.revoked && arm.admitted > 0 && !arm.accepted.is_empty()));
    assert!(cdp.webmcp.session_script_count("H2") > 0);
    assert!(cdp.webmcp.session_script_count("H2") <= 2);
    assert!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["sessionId"] == "H2" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)).count() > 0 || state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)).count() > installs_before);
    assert!(!cdp.webmcp.arms["H2"].accepted.is_empty());
}

#[test]
fn destroying_a_context_frees_its_script_ids_for_later_records() {
    let (mut cdp, _) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    cdp.webmcp.id_cap_override = 2;
    for i in 0..6 {
        cdp.absorb(event("Debugger.scriptParsed", json!({"scriptId":format!("OLD-{i}"),"executionContextId":7,"url":format!("https://ordinary.test/old-{i}.js")})));
    }
    assert_eq!(cdp.webmcp.session_script_count("H"), 2);
    assert!(!cdp.webmcp.scripts.contains_key(&("H".into(), "FRESH".into())));
    cdp.absorb(event("Runtime.executionContextDestroyed", json!({"executionContextId":7})));
    assert_eq!(cdp.webmcp.session_script_count("H"), 0);
    assert!(cdp.webmcp.evaluations.iter().all(|(owner, _)| owner != "H"));
    cdp.absorb(event("Runtime.executionContextCreated", json!({"context":{"id":9,"uniqueId":"NEXT","origin":"https://ordinary.test","auxData":{"frameId":"F","isDefault":true}}})));
    cdp.absorb(event("Debugger.scriptParsed", json!({"scriptId":"FRESH","executionContextId":9,"url":"https://ordinary.test/fresh.js"})));
    assert_eq!(cdp.webmcp.scripts.get(&("H".into(), "FRESH".into())), Some(&9));
    assert_eq!(cdp.webmcp.session_script_count("H"), 1);
}

#[test]
fn lower_only_overrides_never_raise_the_stage1_defaults() {
    assert_eq!(lower_only(DEFAULT_ID_CAP, "SLICC_WEBMCP_MISSING_CAP_ENV"), DEFAULT_ID_CAP);
    assert_eq!(DEFAULT_ID_CAP, 4096);
    assert_eq!(DEFAULT_PENDING_COUNT, 4096);
    assert_eq!(DEFAULT_PENDING_BYTES, 64 << 20);
    let (mut cdp, _) = client();
    cdp.webmcp.id_cap_override = DEFAULT_ID_CAP.saturating_mul(4);
    cdp.webmcp.pending_count_override = DEFAULT_PENDING_COUNT.saturating_mul(4);
    cdp.webmcp.pending_bytes_override = DEFAULT_PENDING_BYTES.saturating_mul(4);
    assert_eq!(cdp.webmcp.id_cap(), DEFAULT_ID_CAP);
    assert_eq!(cdp.webmcp.pending_count_cap(), DEFAULT_PENDING_COUNT);
    assert_eq!(cdp.webmcp.pending_bytes_cap(), DEFAULT_PENDING_BYTES);
    cdp.webmcp.id_cap_override = 7;
    cdp.webmcp.pending_count_override = 5;
    cdp.webmcp.pending_bytes_override = 11;
    assert_eq!(cdp.webmcp.id_cap(), 7);
    assert_eq!(cdp.webmcp.pending_count_cap(), 5);
    assert_eq!(cdp.webmcp.pending_bytes_cap(), 11);
}

#[test]
fn stop_webmcp_disables_auto_attach_without_a_filter() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    cdp.webmcp_auto_attach("H").unwrap();
    let before = state.borrow().sent.len();
    cdp.stop_webmcp("H");
    assert!(
        state.borrow().sent[before..].iter().any(|command| {
            command["method"] == "Target.setAutoAttach"
                && command["params"]["autoAttach"] == false
                && command["params"]["waitForDebuggerOnStart"] == false
                && command["params"]["flatten"] == true
                && command["sessionId"] == "H"
                && command["params"].get("filter").is_none()
        }),
        "{:?}",
        &state.borrow().sent[before..]
    );
}

#[test]
fn direct_auto_attach_marks_the_arm_and_refused_or_attested_routes_stay_detach_only() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    cdp.webmcp_auto_attach("H").unwrap();
    assert!(cdp.webmcp.arms["H"].direct_child_attach);
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Target.setAutoAttach" && command["params"]["waitForDebuggerOnStart"] == false));
    cdp.absorb(event("Target.attachedToTarget", json!({"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/"},"waitingForDebugger":false})));
    assert!(cdp.webmcp_child_evidence() || cdp.webmcp.parents.contains_key("CHILD") || cdp.session_blocked("CHILD"));

    let (mut guarded, state) = client();
    guarded.start_webmcp_for("H", true).unwrap();
    guarded.webmcp_set_route_attested(true);
    guarded.webmcp_auto_attach("H").unwrap();
    assert!(!guarded.webmcp.arms["H"].direct_child_attach);
    let before = state.borrow().sent.len();
    guarded.absorb(event("Target.attachedToTarget", json!({"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/"},"waitingForDebugger":false})));
    assert!(guarded.session_blocked("CHILD"));
    assert!(state.borrow().sent[before..].iter().any(|command| command["method"] == "Target.detachFromTarget"));
    assert!(!state.borrow().sent[before..].iter().any(|command| command["method"] == "Runtime.runIfWaitingForDebugger"));
    assert!(guarded.take_webmcp_children().is_empty());
}

#[test]
fn in_process_cross_origin_installs_only_after_requested_origin_and_keeps_same_origin_equality() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    cdp.webmcp_auto_attach("H").unwrap();
    state.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":{"id":"C","parentId":"F","loaderId":"CROSS","url":"https://foreign.test/child","securityOrigin":"https://foreign.test"}}]);
    cdp.absorb(event("Page.frameNavigated", json!({"frame":state.borrow().tree["frameTree"]["childFrames"][0]["frame"]})));
    let context = json!({"id":8,"uniqueId":"CROSS-CONTEXT","origin":"https://foreign.test","auxData":{"frameId":"C","isDefault":true}});
    state.borrow_mut().runtime_child = Some(context.clone());
    cdp.absorb(event("Runtime.executionContextCreated", json!({"context":context})));
    cdp.webmcp_install_children();
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "CROSS-CONTEXT"));
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "CROSS-CONTEXT").count(), 1);
    assert!(cdp.webmcp_generations().iter().any(|entry| entry["frameId"] == "C" && entry["origin"] == "https://foreign.test"));
    cdp.webmcp_install_children();
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "CROSS-CONTEXT").count(), 1);

    let (mut same, state) = client();
    same.start_webmcp_for("H", true).unwrap();
    same.webmcp_auto_attach("H").unwrap();
    state.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":{"id":"C","parentId":"F","loaderId":"CHILD","url":"https://ordinary.test/child","securityOrigin":"https://ordinary.test"}}]);
    same.absorb(event("Page.frameNavigated", json!({"frame":state.borrow().tree["frameTree"]["childFrames"][0]["frame"]})));
    child_context(&mut same, &state, 8, "CHILD-CONTEXT");
    same.webmcp_install_children();
    assert_eq!(state.borrow().sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["uniqueContextId"] == "CHILD-CONTEXT").count(), 1);
}

#[test]
fn extension_frame_and_protected_oopif_never_install_on_direct() {
    for url in ["chrome-extension://abcd/frame.html", "https://seven.sliccy.ai/"] {
        let (mut cdp, state) = client();
        cdp.start_webmcp_for("H", true).unwrap();
        cdp.webmcp_auto_attach("H").unwrap();
        cdp.webmcp_request_origins(&["https://foreign.test".into(), "https://seven.sliccy.ai".into()]);
        cdp.absorb(event("Target.attachedToTarget", json!({"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":url},"waitingForDebugger":true})));
        assert!(cdp.take_webmcp_children().is_empty());
        assert!(cdp.session_blocked("CHILD"));
        assert!(!state.borrow().sent.iter().any(|command| command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)));
    }
}

#[test]
fn pending_child_cap_drops_the_1025th_cross_origin_candidate() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    cdp.webmcp_auto_attach("H").unwrap();
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    let mut children = Vec::new();
    for i in 0..1025 {
        let id = format!("C{i}");
        children.push(json!({"frame":{"id":id,"parentId":"F","loaderId":format!("L{i}"),"url":"https://foreign.test/child","securityOrigin":"https://foreign.test"}}));
    }
    state.borrow_mut().tree["frameTree"]["childFrames"] = json!(children);
    for i in 0..1025 {
        let frame = format!("C{i}");
        cdp.absorb(event("Page.frameNavigated", json!({"frame":{"id":frame.clone(),"parentId":"F","loaderId":format!("L{i}"),"url":"https://foreign.test/child","securityOrigin":"https://foreign.test"}})));
        cdp.absorb(event("Runtime.executionContextCreated", json!({"context":{"id":100 + i as i64,"uniqueId":format!("U{i}"),"origin":"https://foreign.test","auxData":{"frameId":frame,"isDefault":true}}})));
    }
    cdp.webmcp_install_children();
    assert!(cdp.webmcp.arms["H"].pending_children.len() <= 1024);
    assert!(cdp.webmcp.arms["H"].child_done.len() <= 1024 || cdp.webmcp.arms["H"].pending_children.len() + cdp.webmcp.arms["H"].accepted.len() <= 2048);
}

#[test]
fn existing_oopifs_are_adopted_only_after_set_auto_attach_succeeds() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    let before = state.borrow().sent.len();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"EARLY","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/"},"waitingForDebugger":true}}));
    assert!(!cdp.session_blocked("EARLY"));
    assert!(cdp.webmcp.recorded.is_empty(), "attaches before direct_child_attach are never consumed");
    assert!(!cdp.webmcp.parents.contains_key("EARLY") && !cdp.webmcp.arms.contains_key("EARLY"), "no adoption before setAutoAttach succeeds");
    assert!(!state.borrow().sent[before..].iter().any(|command| matches!(command["method"].as_str(), Some("Target.attachToTarget" | "Target.getTargets"))));
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.webmcp.attach_grace = Some(Duration::ZERO);
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    let before = state.borrow().sent.len();
    cdp.webmcp_auto_attach("H").unwrap();
    assert!(cdp.webmcp.arms[&holder].direct_child_attach);
    assert!(state.borrow().sent[before..].iter().any(|command| command["method"] == "Target.setAutoAttach"));
    assert!(state.borrow().sent[before..].iter().any(|command| command["method"] == "Target.attachToTarget" && command["params"]["targetId"] == "IFRAME"));
    assert!(cdp.webmcp.parents.contains_key("TOUCH-IFRAME"), "reconcile attaches after setAutoAttach succeeds");
}

#[test]
fn refused_auto_attach_or_route_hint_keeps_cross_origin_off() {
    let (mut refused, state) = client();
    refused.start_webmcp_for("H", true).unwrap();
    state.borrow_mut().refuse_auto_attach = true;
    refused.webmcp_auto_attach("H").unwrap();
    assert!(!refused.webmcp.arms["H"].direct_child_attach);
    refused.absorb(event("Target.attachedToTarget", json!({"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/"},"waitingForDebugger":true})));
    assert!(refused.session_blocked("CHILD"));
    assert!(refused.take_webmcp_children().is_empty());

    for route in ["link", "extension", "devtools", "unknown-guard"] {
        let (mut cdp, state) = client();
        cdp.start_webmcp_for("H", true).unwrap();
        state.borrow_mut().route = Some(route.into());
        cdp.webmcp_auto_attach("H").unwrap();
        assert!(!cdp.webmcp.arms["H"].direct_child_attach, "{route}");
        cdp.absorb(event("Target.attachedToTarget", json!({"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/"},"waitingForDebugger":true})));
        assert!(cdp.session_blocked("CHILD"), "{route}");
        assert!(cdp.take_webmcp_children().is_empty(), "{route}");
    }
}

#[test]
fn cross_origin_child_deadline_or_failure_leaves_the_root_holder_ready() {
    let (mut cdp, state) = client();
    cdp.start_webmcp_for("H", true).unwrap();
    cdp.webmcp_auto_attach("H").unwrap();
    assert!(cdp.webmcp.arms["H"].direct_child_attach);
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(event("Target.attachedToTarget", json!({"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/"},"waitingForDebugger":true})));
    assert!(cdp.webmcp_alive("H"));
    assert!(!cdp.webmcp.arms["H"].revoked);
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.pending_install = Some(Instant::now() - Duration::from_secs(1));
        arm.pending_contexts.insert(1, Instant::now() - Duration::from_secs(1));
        arm.pending_frames.insert("IFRAME".into(), ("L".into(), Instant::now() - Duration::from_secs(1)));
    }
    cdp.webmcp_sweep();
    assert!(cdp.webmcp_alive("H"));
    assert!(!cdp.webmcp.arms["H"].revoked);
    assert!(cdp.webmcp.arms.get("CHILD").is_none_or(|arm| arm.revoked) || cdp.session_blocked("CHILD"));
    assert!(cdp.webmcp_settled() || cdp.webmcp.arms.get("CHILD").is_none_or(|arm| arm.revoked));
    let notices = cdp.take_notices().join("");
    assert!(notices.contains("cross-origin child") || cdp.session_blocked("CHILD") || cdp.webmcp.arms.get("CHILD").is_none_or(|arm| arm.revoked));
    assert!(!state.borrow().sent.iter().any(|command| command["sessionId"] == "H" && command["method"] == "Debugger.disable"));
}

#[test]
fn rematching_the_same_child_realm_does_not_raise_admitted() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    let admitted = cdp.webmcp.arms.get("CHILD").map(|arm| arm.admitted).unwrap_or(0);
    assert!(admitted >= 1);
    let child_evals = state.borrow().sent.iter().filter(|command| {
        command["sessionId"] == "CHILD"
            && command["method"] == "Runtime.evaluate"
            && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
    }).count();
    assert_eq!(child_evals, 1, "shim Runtime.evaluate once per realm");
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    assert_eq!(cdp.webmcp.arms.get("CHILD").map(|arm| arm.admitted), Some(admitted), "admitted counts realms, not evaluates");
    assert_eq!(
        state.borrow().sent.iter().filter(|command| {
            command["sessionId"] == "CHILD"
                && command["method"] == "Runtime.evaluate"
                && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
        }).count(),
        1,
        "rematch must not send another shim evaluate"
    );
}

fn reinstall_child_client() -> (Cdp, Rc<RefCell<BackendState>>, String) {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/a","securityOrigin":"https://foreign.test","loaderId":"L1"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/a"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.accepted.contains(&("FRAME".into(), "L1".into(), "CROSS-CTX".into()))));
    (cdp, state, holder)
}

fn child_shim_installs(state: &Rc<RefCell<BackendState>>, from: usize) -> Vec<Value> {
    state.borrow().sent[from..].iter().filter(|command| command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)).cloned().collect()
}

#[test]
fn context_generation_advances_only_on_contexts_cleared() {
    let (mut cdp, _state, _holder) = reinstall_child_client();
    assert_eq!(cdp.webmcp.arms["CHILD"].context_generation, 0);
    assert_eq!(cdp.webmcp.contexts.iter().find(|((owner, _), context)| owner == "CHILD" && context.unique == "CROSS-CTX").map(|(_, context)| context.generation), Some(0));
    assert_eq!(cdp.webmcp.arms["CHILD"].frame_accept_generation.get("FRAME"), Some(&0));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/b","securityOrigin":"https://foreign.test","loaderId":"L2"}}}));
    assert_eq!(cdp.webmcp.arms["CHILD"].context_generation, 0);
    cdp.absorb(json!({"method":"Runtime.executionContextsCleared","sessionId":"CHILD","params":{}}));
    assert_eq!(cdp.webmcp.arms["CHILD"].context_generation, 1);
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":8,"uniqueId":"NEW-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    assert_eq!(cdp.webmcp.contexts[&("CHILD".into(), 8)].generation, 1);
}

#[test]
fn post_navigation_reinstall_requires_newer_generation() {
    for order in ["cleared-created-navigated", "navigated-cleared-created", "created-navigated-cleared", "navigated-created-cleared"] {
        let (mut cdp, state, holder) = reinstall_child_client();
        state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/b","securityOrigin":"https://foreign.test","loaderId":"L2"}}}));
        let before = state.borrow().sent.len();
        match order {
            "cleared-created-navigated" => {
                cdp.absorb(json!({"method":"Runtime.executionContextsCleared","sessionId":"CHILD","params":{}}));
                cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":8,"uniqueId":"NEW-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
                cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/b","securityOrigin":"https://foreign.test","loaderId":"L2"}}}));
            }
            "navigated-cleared-created" => {
                cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/b","securityOrigin":"https://foreign.test","loaderId":"L2"}}}));
                cdp.absorb(json!({"method":"Runtime.executionContextsCleared","sessionId":"CHILD","params":{}}));
                cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":8,"uniqueId":"NEW-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
            }
            "created-navigated-cleared" => {
                cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":8,"uniqueId":"NEW-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
                cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/b","securityOrigin":"https://foreign.test","loaderId":"L2"}}}));
                assert!({ let _ = cdp.webmcp_adopt_install("CHILD", &holder, Instant::now() + DOCUMENT_CHECK); child_shim_installs(&state, before).iter().all(|command| command["params"]["uniqueContextId"] != "NEW-CTX") }, "same generation must wait: {order}");
                cdp.absorb(json!({"method":"Runtime.executionContextsCleared","sessionId":"CHILD","params":{}}));
                cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"AFTER-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
            }
            "navigated-created-cleared" => {
                cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/b","securityOrigin":"https://foreign.test","loaderId":"L2"}}}));
                cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":8,"uniqueId":"NEW-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
                assert!({ let _ = cdp.webmcp_adopt_install("CHILD", &holder, Instant::now() + DOCUMENT_CHECK); child_shim_installs(&state, before).iter().all(|command| command["params"]["uniqueContextId"] != "NEW-CTX") }, "same generation must wait: {order}");
                cdp.absorb(json!({"method":"Runtime.executionContextsCleared","sessionId":"CHILD","params":{}}));
                cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"AFTER-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
            }
            _ => unreachable!(),
        }
        let _ = cdp.webmcp_adopt_install("CHILD", &holder, Instant::now() + DOCUMENT_CHECK);
        let installs = child_shim_installs(&state, before);
        let expected = if order.ends_with("cleared") { "AFTER-CTX" } else { "NEW-CTX" };
        assert!(installs.iter().any(|command| command["params"]["uniqueContextId"] == expected), "{order}: {installs:?}");
        assert!(installs.iter().all(|command| command["params"]["uniqueContextId"] != "CROSS-CTX"), "{order}: {installs:?}");
        assert!(cdp.webmcp.contexts.values().all(|context| context.unique != "CROSS-CTX" || context.loader != "L2"), "{order}");
    }
}

#[test]
fn adopt_install_and_finish_awaiting_evaluate_the_same_realm_once() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.admitted == 1));
    let shim_evals = || state.borrow().sent.iter().filter(|command| {
        command["sessionId"] == "CHILD"
            && command["method"] == "Runtime.evaluate"
            && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
    }).count();
    assert_eq!(shim_evals(), 1);
    assert!(cdp.webmcp_adopt_install("CHILD", &holder, Instant::now() + DOCUMENT_CHECK).unwrap());
    cdp.webmcp_finish_awaiting_children();
    assert_eq!(shim_evals(), 1, "adopt_install then finish_awaiting must not re-send the shim");
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.admitted = 0;
        arm.accepted.clear();
        arm.pending_install = None;
        arm.installing.insert(("FRAME".into(), "CROSS".into(), "CROSS-CTX".into()));
    }
    cdp.webmcp.quarantined.insert("CHILD".into());
    cdp.webmcp_finish_awaiting_children();
    assert_eq!(shim_evals(), 1, "in-flight installing blocks finish_awaiting from selecting the child");
    assert_eq!(cdp.webmcp_adopt_install("CHILD", &holder, Instant::now() + DOCUMENT_CHECK).ok(), Some(false));
    assert_eq!(shim_evals(), 1, "in-flight installing makes adopt_install a no-op send");
    if let Some(arm) = cdp.webmcp.arms.get_mut("CHILD") {
        arm.installing.clear();
        arm.accepted.insert(("FRAME".into(), "CROSS".into(), "CROSS-CTX".into()));
        arm.admitted = 0;
        arm.pending_install = None;
    }
    cdp.webmcp.quarantined.insert("CHILD".into());
    assert!(cdp.webmcp_adopt_install("CHILD", &holder, Instant::now() + DOCUMENT_CHECK).unwrap());
    cdp.webmcp_finish_awaiting_children();
    assert_eq!(shim_evals(), 1, "accepted realm stays at one evaluate after both paths run");
    assert_eq!(cdp.webmcp.arms.get("CHILD").map(|arm| arm.admitted), Some(1));
}

#[test]
fn child_arm_enables_network_before_page_and_observes_policy_before_shim() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    let before = state.borrow().sent.len();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    let child_sent = state.borrow().sent[before..].iter().filter(|command| command["sessionId"] == "CHILD").cloned().collect::<Vec<_>>();
    let network = child_sent.iter().position(|command| command["method"] == "Network.enable");
    let page = child_sent.iter().position(|command| command["method"] == "Page.enable");
    assert!(network.is_some_and(|network| page.is_some_and(|page| network < page)), "Network.enable must precede Page.enable on the child: {child_sent:?}");
    assert!(
        child_sent.iter().any(|command| command["method"] == "Network.loadNetworkResource"),
        "policy must come from browser-side loadNetworkResource, not page fetch: {child_sent:?}"
    );
    assert!(
        !child_sent.iter().any(|command| command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(|source| source.contains("fetch(document.URL)") || source.contains("permissions-policy"))),
        "page-world fetch must not observe policy: {child_sent:?}"
    );
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    assert!(
        cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.documents.get("FRAME:CROSS").is_some_and(|document| document.get("policy").and_then(Value::as_str) == Some("tools=*"))),
        "already-loaded children must observe Permissions-Policy before listing: {:?}",
        cdp.webmcp.arms.get("CHILD").map(|arm| &arm.documents)
    );
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.admitted > 0));
}

#[test]
fn start_time_requested_origin_installs_when_default_context_appears() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"about:blank"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.revoked && arm.admitted == 0));
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    let before = state.borrow().sent.len();
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    assert!(
        cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.admitted > 0)
            || state.borrow().sent[before..].iter().any(|command| command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install))
            || state.borrow().sent.iter().any(|command| command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)),
        "start-time --from-origin must install once the default context appears"
    );
    assert!(cdp.webmcp_alive(&holder));
}

#[test]
fn unverified_child_parks_until_first_commit_then_installs() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    assert!(cdp.webmcp.arms[&holder].direct_child_attach);
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    let before = state.borrow().sent.len();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"about:blank"},"waitingForDebugger":false}}));
    assert!(!state.borrow().sent[before..].iter().any(|command| command["method"] == "Runtime.runIfWaitingForDebugger"));
    assert!(!cdp.content_blocked("CHILD"));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.revoked && arm.pending_install.is_some()), "unverified blank child must park with a promote-or-settle deadline");
    cdp.webmcp_sweep();
    assert!(!cdp.session_blocked("CHILD"), "parked blank child must survive a sweep before its deadline");
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    assert!(
        cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.admitted > 0)
            || state.borrow().sent.iter().any(|command| command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)),
        "first real commit must make a parked blank child eligible"
    );
    assert!(cdp.webmcp_alive(&holder));
    assert!(!cdp.webmcp.arms[&holder].revoked);
}

#[test]
fn oopif_private_auto_attach_installs_without_content_blocking_the_root() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    assert!(cdp.webmcp.arms[&holder].direct_child_attach);
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    assert!(cdp.webmcp.arms[&holder].admitted > 0);
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"about:blank"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp_alive(&holder), "root must stay ready while adopting an OOPIF");
    assert!(!cdp.webmcp.arms[&holder].revoked);
    assert!(cdp.webmcp.arms[&holder].pending_contexts.is_empty(), "root pendingContexts must not count the child: {:?}", cdp.webmcp.arms[&holder].pending_contexts);
    assert!(cdp.webmcp.arms[&holder].pending_frames.is_empty(), "root pendingFrames must not count the child: {:?}", cdp.webmcp.arms[&holder].pending_frames);
    assert!(!cdp.content_blocked("CHILD"), "OOPIF must not be content-blocked before install");
    assert!(!cdp.session_blocked("CHILD"), "OOPIF must not settle as blocked before install");
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Runtime.runIfWaitingForDebugger"), "late Option A must not hold the OOPIF");
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.revoked && arm.pending_install.is_some()), "blank OOPIF must park with a promote-or-settle deadline");
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":holder,"params":{"frame":{"id":"FRAME","parentId":"F","url":"https://foreign.test/cross","securityOrigin":"","loaderId":""}}}));
    assert!(!cdp.webmcp.arms[&holder].revoked, "parent-session iframe navigation must not revoke the root");
    assert!(cdp.webmcp.arms[&holder].pending_frames.is_empty(), "parent-session iframe navigation must not add root pending frames");
    assert!(cdp.webmcp.arms[&holder].pending_contexts.is_empty(), "parent-session iframe navigation must not add root pending contexts");
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    assert!(!cdp.session_blocked("CHILD"), "parked child must not settle on its first real commit");
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    assert!(!cdp.content_blocked("CHILD"));
    assert!(
        cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.admitted > 0)
            || state.borrow().sent.iter().any(|command| command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)),
        "requested-origin OOPIF must install after commit on the child session"
    );
    assert!(cdp.webmcp_alive(&holder));
    assert!(!cdp.webmcp.arms[&holder].revoked);
    assert!(cdp.webmcp.arms[&holder].pending_contexts.is_empty());
    assert!(cdp.webmcp.arms[&holder].pending_frames.is_empty());

    let (mut late, state) = client();
    let verified = json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}});
    state.borrow_mut().oopif_tree = Some(verified.clone());
    late.start_webmcp_for("H", true).unwrap();
    let holder = late.webmcp_session("H").to_string();
    late.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = late.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    let before = state.borrow().sent.len();
    late.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    assert!(late.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.revoked && arm.pending_install.is_none()), "verified unrequested child must park with no deadline");
    assert!(late.webmcp.quarantined.contains("CHILD"));
    assert!(
        late.webmcp_snapshot().iter().any(|session| {
            session["session"] == "CHILD" && session["pendingOrigins"].as_array().is_some_and(|origins| origins.iter().any(|origin| origin == "https://foreign.test"))
        }),
        "parked child origin must be visible to pendingOrigins"
    );
    assert!(
        !state.borrow().sent[before..].iter().any(|command| {
            command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
        }),
        "shim must not evaluate before --from-origin"
    );
    late.webmcp_sweep();
    assert!(!late.session_blocked("CHILD"), "parked child must survive a sweep with no deadline");
    assert!(late.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.revoked && arm.pending_install.is_none()));
    let before = state.borrow().sent.len();
    late.webmcp_request_origins(&["https://other.test".into()]);
    assert!(
        !state.borrow().sent[before..].iter().any(|command| {
            command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
        }),
        "unrelated requested origin must not install a parked child"
    );
    assert!(!late.session_blocked("CHILD"));
    assert!(late.webmcp.arms.get("CHILD").is_some_and(|arm| arm.pending_install.is_none()));
    let before = state.borrow().sent.len();
    late.webmcp_request_origins(&["https://foreign.test".into()]);
    let child_evals = state.borrow().sent[before..].iter().filter(|command| {
        command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
    }).count();
    assert_eq!(child_evals, 1, "exactly one child shim evaluate after the origin is requested");
    assert!(!late.session_blocked(&holder));
    assert!(late.webmcp_alive(&holder));

    let (mut rooted, state) = client();
    state.borrow_mut().oopif_tree = Some(verified.clone());
    rooted.start_webmcp_for("H", true).unwrap();
    let holder = rooted.webmcp_session("H").to_string();
    rooted.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = rooted.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    rooted.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    assert!(rooted.webmcp.arms.get("CHILD").is_some_and(|arm| arm.pending_install.is_none()));
    rooted.absorb(json!({"method":"Page.frameNavigated","sessionId":holder,"params":{"frame":{"id":"F","url":"https://ordinary.test/next","securityOrigin":"https://ordinary.test","loaderId":"NEXT"}}}));
    assert!(!rooted.session_blocked("CHILD") && rooted.webmcp.parents.contains_key("CHILD"), "ordinary root navigation must keep parked children of the new document");
    assert!(!state.borrow().sent.iter().any(|command| command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "CHILD"));

    let (mut buffered, state) = client();
    state.borrow_mut().oopif_tree = Some(verified);
    state.borrow_mut().buffer_child_lifecycle_on_auto_attach = true;
    buffered.start_webmcp_for("H", true).unwrap();
    let holder = buffered.webmcp_session("H").to_string();
    if let Some(arm) = buffered.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    buffered.webmcp_auto_attach(&holder).unwrap();
    assert!(buffered.webmcp.parents.contains_key("CHILD") || buffered.webmcp.arms.contains_key("CHILD"));
    let before = state.borrow().sent.len();
    buffered.webmcp_request_origins(&["https://foreign.test".into()]);
    assert!(
        state.borrow().sent[before..].iter().any(|command| {
            command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
        }),
        "replayed attach must still install after the origin is requested"
    );
    assert!(!buffered.session_blocked(&holder));

    let (mut missing, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    state.borrow_mut().runtime_child = Some(json!({"id":9,"uniqueId":"","origin":"https://foreign.test","auxData":{"frameId":"OTHER","isDefault":true}}));
    missing.start_webmcp_for("H", true).unwrap();
    let holder = missing.webmcp_session("H").to_string();
    missing.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = missing.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    missing.webmcp_request_origins(&["https://foreign.test".into()]);
    missing.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    assert!(missing.webmcp.arms.get("CHILD").is_some_and(|arm| arm.pending_install.is_some()), "context miss must restore the install deadline");
    if let Some(arm) = missing.webmcp.arms.get_mut("CHILD") {
        arm.pending_install = Some(Instant::now() - Duration::from_secs(1));
    }
    missing.webmcp_sweep();
    assert!(missing.session_blocked("CHILD"));
    let notices = missing.take_notices().join("");
    assert!(notices.contains("cross-origin child"), "{notices}");
    assert!(missing.webmcp_alive(&holder));
    assert!(!missing.webmcp.arms[&holder].revoked);
}

#[test]
fn private_oopif_attach_does_not_touch_protect_or_guard_the_root() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    let before = state.borrow().sent.len();
    let root_blocked = cdp.session_blocked(&holder);
    let root_guarded = cdp.guard_sessions.contains(&holder);
    let root_content = cdp.content_blocked(&holder);
    let root_protected = cdp.protected_targets.contains("T");
    let page_document = cdp.documents.get("T").map(|document| document.url.clone());
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"about:blank"},"waitingForDebugger":false}}));
    let after_attach = state.borrow().sent[before..].to_vec();
    assert!(
        !after_attach.iter().any(|command| {
            command["method"] == "Target.attachToTarget" && command["params"]["targetId"] == "T"
        }),
        "private auto-attach must not touch the page target: {after_attach:?}"
    );
    assert_eq!(cdp.session_blocked(&holder), root_blocked, "adopt must not block the root session");
    assert_eq!(cdp.guard_sessions.contains(&holder), root_guarded, "adopt must not place the root in guard_sessions");
    assert_eq!(cdp.content_blocked(&holder), root_content, "adopt must not content-block the root");
    assert_eq!(cdp.protected_targets.contains("T"), root_protected, "adopt must not protect_target the page");
    assert_eq!(cdp.documents.get("T").map(|document| document.url.clone()), page_document, "adopt must not clear the page document (page:unknown)");
    assert_eq!(cdp.guards.get("IFRAME").map(String::as_str), Some("CHILD"), "adopt registers the OOPIF as its own guard slot");
    assert!(!cdp.guard_sessions.contains("CHILD"), "OOPIF guard slot is not a Fetch guard_sessions entry");
    assert!(cdp.webmcp.parents.contains_key("CHILD"));
    assert!(!cdp.session_blocked(&holder));
    assert!(!cdp.content_blocked(&holder));
    assert!(cdp.webmcp_alive(&holder));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.pending_install.is_some()), "blank child parks with a promote-or-settle deadline");
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":holder,"params":{"frame":{"id":"F","url":"https://ordinary.test/next","securityOrigin":"https://ordinary.test","loaderId":"NEXT"}}}));
    assert!(!cdp.session_blocked("CHILD"), "ordinary root navigation must not settle a parked child that attached before frameNavigated");
    assert!(cdp.webmcp.parents.contains_key("CHILD"), "parked child remains parented across ordinary root navigation");
    assert!(!cdp.session_blocked(&holder), "root stays unblocked after ordinary navigation");
    assert!(!cdp.content_blocked(&holder), "root stays content-unblocked after ordinary navigation");
    assert!(!cdp.guard_sessions.contains(&holder));
    assert!(cdp.documents.get("T").is_some(), "page document remains; root page label stays known");
    assert_eq!(cdp.guards.get("IFRAME").map(String::as_str), Some("CHILD"), "OOPIF guard slot survives ordinary root navigation");
    assert!(!cdp.protected_targets.contains("T") || root_protected);
}

#[test]
fn parked_child_attached_before_root_navigation_survives_and_installs_once() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"about:blank","securityOrigin":"","loaderId":""}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"about:blank","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| !arm.revoked && arm.admitted == 0 && arm.pending_install.is_some()));
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":holder,"params":{"frame":{"id":"F","url":"https://ordinary.test/next","securityOrigin":"https://ordinary.test","loaderId":"NEXT"}}}));
    assert!(!cdp.session_blocked("CHILD"), "attach-before-navigation parked child must survive root frameNavigated");
    assert!(cdp.webmcp.parents.contains_key("CHILD"));
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    let before = state.borrow().sent.len();
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"CHILD","params":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    let child_evals = state.borrow().sent[before..].iter().filter(|command| {
        command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
    }).count();
    assert_eq!(child_evals, 1, "surviving parked child installs exactly once after request");
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.admitted > 0));
    assert!(cdp.webmcp_alive(&holder));
}

#[test]
fn adopt_install_parks_instead_of_settling_when_parent_root_is_mid_navigation() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.admitted == 0));
    let parent_contexts = cdp.webmcp.contexts.iter().filter(|((owner, _), _)| owner == &holder).map(|(key, context)| (key.clone(), context.clone())).collect::<Vec<_>>();
    cdp.webmcp.contexts.retain(|(owner, _), _| owner != &holder);
    cdp.webmcp.requested_origins.insert("https://foreign.test".into());
    let before = state.borrow().sent.len();
    assert_eq!(cdp.webmcp_adopt_install("CHILD", &holder, Instant::now() + Duration::from_secs(1)), Ok(false));
    assert!(!cdp.session_blocked("CHILD"), "unstable parent root must park the child, not settle it");
    assert!(cdp.webmcp.parents.contains_key("CHILD"));
    assert!(
        !state.borrow().sent[before..].iter().any(|command| {
            command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
        }),
        "no evaluate while the parent root cannot be verified"
    );
    for (key, context) in parent_contexts {
        cdp.webmcp.contexts.insert(key, context);
    }
    let before = state.borrow().sent.len();
    assert_eq!(cdp.webmcp_adopt_install("CHILD", &holder, Instant::now() + Duration::from_secs(1)), Ok(true));
    let child_evals = state.borrow().sent[before..].iter().filter(|command| {
        command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
    }).count();
    assert_eq!(child_evals, 1, "child installs once the parent root is stable again");
    assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.admitted > 0));
}

#[test]
fn protected_root_navigation_never_evaluates_a_parked_child() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.parents.contains_key("CHILD") || cdp.webmcp.arms.contains_key("CHILD"));
    let before = state.borrow().sent.len();
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":holder,"params":{"frame":{"id":"F","url":"https://catalog.test/vault","securityOrigin":"https://catalog.test","loaderId":"PROTECTED"}}}));
    assert!(cdp.webmcp.arms.get(&holder).is_some_and(|arm| arm.revoked), "protected root navigation revokes the holder");
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.webmcp_finish_awaiting_children();
    assert!(
        !state.borrow().sent[before..].iter().any(|command| {
            command["sessionId"] == "CHILD" && command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)
        }),
        "parked child must never evaluate after the root navigates to a protected document"
    );
}

#[test]
fn missing_parent_frame_id_from_attach_and_get_target_info_fails_closed_without_domains() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    state.borrow_mut().omit_get_target_parent = true;
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    let before = state.borrow().sent.len();
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    let child_sent = state.borrow().sent[before..].iter().filter(|command| command["sessionId"] == "CHILD").cloned().collect::<Vec<_>>();
    assert!(cdp.session_blocked("CHILD") || !cdp.webmcp.parents.contains_key("CHILD"));
    assert!(!child_sent.iter().any(|command| matches!(command["method"].as_str(), Some("Network.enable" | "Runtime.enable" | "Page.enable" | "Runtime.evaluate"))), "{child_sent:?}");
}

#[test]
fn attach_parent_frame_id_or_get_target_info_parent_frame_id_allows_arm() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.arms.get("CHILD").is_some());
    assert_eq!(cdp.targets["IFRAME"].get("parentFrameId").and_then(Value::as_str), Some("F"));
}

#[test]
fn real_chrome_oopif_frame_id_equals_target_id_and_unit_keeps_differing_ids() {
    for (frame_id, target_id) in [("IFRAME", "IFRAME"), ("FRAME", "IFRAME")] {
        let (mut cdp, state) = client();
        state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":frame_id,"url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
        cdp.start_webmcp_for("H", true).unwrap();
        let holder = cdp.webmcp_session("H").to_string();
        cdp.webmcp_auto_attach(&holder).unwrap();
        if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
            arm.pending_contexts.clear();
            arm.pending_frames.clear();
            arm.pending_install = None;
        }
        cdp.webmcp_request_origins(&["https://foreign.test".into()]);
        cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":target_id,"type":"iframe","url":"https://foreign.test/cross","parentFrameId":"F"},"waitingForDebugger":false}}));
        cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":frame_id,"isDefault":true}}}}));
        assert!(cdp.webmcp.arms.get("CHILD").is_some_and(|arm| arm.admitted > 0), "real Chrome uses F==T; unit also keeps F!=T. F={frame_id} T={target_id}");
    }
}

#[test]
fn real_chrome_root_get_frame_tree_lists_only_local_frames_never_oopif() {
    let (_cdp, state) = client();
    assert_eq!(state.borrow().tree.pointer("/frameTree/frame/id").and_then(Value::as_str), Some("F"));
    assert!(state.borrow().tree.pointer("/frameTree/childFrames").and_then(Value::as_array).is_none_or(|children| children.is_empty()), "root Page.getFrameTree lists only local frames, never the OOPIF");
}
fn policy_child(configure: impl FnOnce(&mut BackendState)) -> Cdp {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    configure(&mut state.borrow_mut());
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    if let Some(arm) = cdp.webmcp.arms.get_mut(&holder) {
        arm.pending_contexts.clear();
        arm.pending_frames.clear();
        arm.pending_install = None;
    }
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    cdp
}

fn policy_document(cdp: &Cdp) -> Option<Value> {
    cdp.webmcp.arms.get("CHILD").and_then(|arm| arm.documents.get("FRAME:CROSS").cloned())
}

#[test]
fn load_network_resource_is_credentialless_and_stores_fetched_source() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"CHILD","params":{"context":{"id":9,"uniqueId":"CROSS-CTX","origin":"https://foreign.test","auxData":{"frameId":"FRAME","isDefault":true}}}}));
    let load = state.borrow().sent.iter().find(|command| command["method"] == "Network.loadNetworkResource").cloned().expect("loadNetworkResource");
    assert_eq!(load["params"]["options"]["includeCredentials"], false);
    assert_eq!(load["params"]["options"]["disableCache"], true);
    let document = policy_document(&cdp).expect("policy recorded");
    assert_eq!(document["source"], "fetched");
    assert_eq!(document["policy"], "tools=*");
}

#[test]
fn native_policy_state_is_tried_before_load_network_resource() {
    let cdp = policy_child(|state| state.native_policy = Some(false));
    let document = policy_document(&cdp).expect("policy recorded");
    assert_eq!(document["source"], "native");
    assert_eq!(document["policy"], "tools=()");
}

#[test]
fn fetched_policy_is_unknown_when_missing_failed_non_2xx_or_empty() {
    for headers in [Value::Null, json!({"content-type":"text/html"}), json!({"__status":404,"permissions-policy":"tools=*"}), json!({"permissions-policy":""})] {
        let cdp = policy_child(|state| state.load_network_headers = Some(headers.clone()));
        assert!(policy_document(&cdp).is_none(), "headers {headers} must leave policy unknown");
    }
}

#[test]
fn oversized_fetched_policy_is_denied() {
    let long = format!("tools=*, {}", "a".repeat(20000));
    let cdp = policy_child(|state| state.child_policy = Some(long));
    assert_eq!(policy_document(&cdp).expect("policy recorded")["policy"], "tools=()");
}

#[test]
fn network_document_response_overwrites_fetched_policy_but_not_the_reverse() {
    let mut cdp = policy_child(|_| {});
    assert_eq!(policy_document(&cdp).expect("fetched")["source"], "fetched");
    cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"CHILD","params":{"frameId":"FRAME","loaderId":"CROSS","type":"Document","response":{"url":"https://foreign.test/cross","headers":{"permissions-policy":"tools=()"}}}}));
    let document = policy_document(&cdp).expect("network");
    assert_eq!(document["source"], "network");
    assert_eq!(document["policy"], "tools=()");
    cdp.webmcp_record_document_policy("CHILD", "FRAME", "CROSS", "https://foreign.test", "tools=*", "fetched");
    assert_eq!(policy_document(&cdp).expect("kept")["policy"], "tools=()");
}

#[test]
fn fetched_policy_is_dropped_when_committed_origin_changed_during_fetch() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://other.test/cross","securityOrigin":"https://other.test","loaderId":"CROSS"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    cdp.webmcp_observe_child_policy("CHILD", "FRAME", "CROSS", "https://foreign.test", "https://foreign.test/cross");
    assert!(policy_document(&cdp).is_none());
}

#[test]
fn get_version_failure_keeps_cross_origin_off() {
    for mode in ["timeout", "error", "empty", "number", "array", "nonobject"] {
        let (mut cdp, state) = client();
        cdp.start_webmcp_for("H", true).unwrap();
        state.borrow_mut().version_mode = Some(mode.into());
        let before = state.borrow().sent.len();
        cdp.webmcp_auto_attach("H").unwrap();
        assert!(!cdp.webmcp.arms["H"].direct_child_attach, "{mode}");
        assert!(cdp.webmcp_route_attested_snapshot(), "{mode}");
        assert!(!state.borrow().sent[before..].iter().any(|command| command["method"] == "Target.setAutoAttach"), "{mode}");
        cdp.absorb(event("Target.attachedToTarget", json!({"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/"},"waitingForDebugger":true})));
        assert!(cdp.session_blocked("CHILD"), "{mode}");
    }
}

#[test]
fn later_route_hint_clears_direct_attach_and_detaches_children() {
    let (mut cdp, state) = client();
    state.borrow_mut().oopif_tree = Some(json!({"frameTree":{"frame":{"id":"FRAME","url":"https://foreign.test/cross","securityOrigin":"https://foreign.test","loaderId":"CROSS"}}}));
    cdp.start_webmcp_for("H", true).unwrap();
    let holder = cdp.webmcp_session("H").to_string();
    cdp.webmcp_auto_attach(&holder).unwrap();
    cdp.webmcp_request_origins(&["https://foreign.test".into()]);
    cdp.absorb(json!({"method":"Target.attachedToTarget","sessionId":holder,"params":{"sessionId":"CHILD","targetInfo":{"targetId":"IFRAME","type":"iframe","url":"https://foreign.test/cross"},"waitingForDebugger":false}}));
    assert!(cdp.webmcp.arms[&holder].direct_child_attach);
    cdp.webmcp_set_route_attested(true);
    assert!(!cdp.webmcp.arms[&holder].direct_child_attach);
    assert!(cdp.session_blocked("CHILD") || !cdp.webmcp.parents.contains_key("CHILD"));
    assert!(state.borrow().sent.iter().any(|command| command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "CHILD"));
}

