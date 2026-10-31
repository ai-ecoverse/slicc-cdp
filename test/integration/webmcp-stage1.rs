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
    incomplete: bool,
    protected_world: bool,
    missing_instrumentation: bool,
    late_world: bool,
    raw_instrumentation: bool,
    conversion_pending: bool,
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
            "DOM.getDocument" if state.dom_refusal => Some("slicc: refused: protected frame"),
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
                state.tree.clone().unwrap_or_else(|| tree("L"))
            }
            "Runtime.enable" => {
                let loader = state
                    .tree
                    .as_ref()
                    .and_then(|tree| tree.pointer("/frameTree/frame/loaderId"))
                    .and_then(Value::as_str)
                    .unwrap_or("L")
                    .to_string();
                state
                    .incoming
                    .push_back(context(&session, &loader).to_string());
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
                for owner in state.sessions.keys().cloned().collect::<Vec<_>>() {
                    state.incoming.push_back(json!({"method":"Page.frameNavigated","sessionId":owner,"params":{"frame":tree("R")["frameTree"]["frame"]}}).to_string());
                    state.incoming.push_back(json!({"method":"Runtime.executionContextsCleared","sessionId":owner,"params":{}}).to_string());
                    state.incoming.push_back(context(&owner, "R").to_string());
                }
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
    let remove = state
        .calls
        .iter()
        .position(|call| call["method"] == "Debugger.removeBreakpoint")
        .unwrap();
    let resume = state
        .calls
        .iter()
        .position(|call| call["method"] == "Debugger.resume")
        .unwrap();
    assert!(install < remove && remove < resume);
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
fn source_and_call_frame_methods_remain_refused_at_a_published_pause() {
    let (mut cdp, state, session) = fixture();
    cdp.call("Debugger.enable", json!({}), Some(&session))
        .unwrap();
    cdp.call(
        "Debugger.setInstrumentationBreakpoint",
        json!({"instrumentation":"beforeScriptExecution"}),
        Some(&session),
    )
    .unwrap();
    cdp.call("Page.reload", json!({}), Some(&session)).unwrap();
    while !cdp
        .events()
        .iter()
        .any(|event| event["method"] == "Debugger.paused")
    {
        assert!(cdp.pump(Instant::now() + Duration::from_secs(1)).unwrap());
    }
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
        assert!(count(&state, "DOM.getDocument") >= 1);
        assert!(state
            .calls
            .iter()
            .filter(|call| call["method"].as_str().unwrap().starts_with("Debugger."))
            .all(|call| call["method"] == "Debugger.enable"));
    }
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
            state.dom_refusal = mode == "protected-read";
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
