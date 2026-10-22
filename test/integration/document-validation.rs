use cdp_client::cdp::{Cdp, Transport};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::time::Instant;

struct Backend {
    worker: Option<Value>,
    tree: Value,
    next_tree: Option<Value>,
    owner_tree: Option<Value>,
    history: Option<Value>,
    deadlines: Vec<std::time::Duration>,
    block_tree: bool,
    next_worker_url: Option<String>,
    navigation_event: bool,
    event_loader: String,
    event_session: Option<String>,
    sessions: BTreeMap<String, String>,
    calls: Vec<Value>,
    inbox: VecDeque<String>,
}

fn tree(loader: &str) -> Value {
    json!({"frameTree":{"frame":{"id":"F","loaderId":loader,"url":"https://ordinary.example/","securityOrigin":"https://ordinary.example"}}})
}

struct Fake(Rc<RefCell<Backend>>);

impl Transport for Fake {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut backend = self.0.borrow_mut();
        backend.calls.push(call.clone());
        let target = call["params"]["targetId"].as_str().unwrap_or("T");
        let result = match call["method"].as_str().unwrap() {
            "Target.createTarget" => json!({"targetId":"T"}),
            "Target.getTargetInfo" => json!({"targetInfo":if target == "T" { backend.worker.clone().unwrap_or_else(|| json!({"targetId":"T","type":"page","url":backend.tree.pointer("/frameTree/frame/url").and_then(Value::as_str).unwrap_or("")})) } else { json!({"targetId":target,"type":"page","url":"https://ordinary.example/"}) }}),
            "Target.attachToTarget" => {
                let session = format!("S{}", backend.sessions.len() + 1);
                backend.sessions.insert(session.clone(), target.into());
                json!({"sessionId":session})
            }
            "Page.getFrameTree" => {
                if backend.block_tree { return Ok(()) }
                if backend.sessions.get(call["sessionId"].as_str().unwrap_or("")).is_some_and(|target| target == "OWNER") { backend.owner_tree.clone().unwrap_or_else(|| backend.tree.clone()) } else { backend.tree.clone() }
            },
            "Page.getNavigationHistory" => {
                let Some(history) = backend.history.clone() else { return Ok(()) };
                history
            }
            "Runtime.enable" => {
                backend.inbox.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":call["sessionId"],"params":{"context":{"id":7,"uniqueId":"child-context","name":"child-world","origin":"https://ordinary.example","auxData":{"frameId":"CHILD","isDefault":false}}}}).to_string());
                json!({})
            }
            "Runtime.evaluate" => {
                if let Some(next) = backend.next_tree.take() { backend.tree = next; }
                if let Some(url) = backend.next_worker_url.take() { backend.worker.as_mut().unwrap()["url"] = json!(url); }
                if backend.navigation_event {
                    let event = json!({"method":"Page.frameNavigated","sessionId":backend.event_session.clone().unwrap_or_else(|| call["sessionId"].as_str().unwrap().to_string()),"params":{"frame":{"id":"F","loaderId":backend.event_loader,"url":"https://ordinary.example/","securityOrigin":"https://ordinary.example"}}});
                    backend.inbox.push_back(event.to_string());
                }
                match call["params"]["expression"].as_str() {
                    Some("0") => json!({"result":{"type":"number","value":0}}),
                    Some("document.readyState") => json!({"result":{"type":"string","value":"complete"}}),
                    Some("document.readyState === 'complete' ? location.href : ''") => json!({"result":{"type":"string","value":backend.tree.pointer("/frameTree/frame/url").and_then(Value::as_str).unwrap_or("")}}),
                    _ => json!({"result":{"type":"string","value":"document-value"}}),
                }
            }
            "Page.printToPDF" => json!({"stream":"H"}),
            _ => json!({}),
        };
        backend.inbox.push_back(json!({"id":call["id"],"result":result}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
        let mut backend = self.0.borrow_mut();
        backend.deadlines.push(deadline.saturating_duration_since(Instant::now()));
        Ok(backend.inbox.pop_front())
    }
}

fn fixture() -> (Cdp, Rc<RefCell<Backend>>) {
    let backend = Rc::new(RefCell::new(Backend {
        worker:None, tree:tree("L1"), next_tree:None, owner_tree:None, history:Some(json!({"entries":[{"id":1,"url":"https://ordinary.example/"}]})), deadlines:Vec::new(), block_tree:false, next_worker_url:None, navigation_event:false,
        event_loader:"L2".into(), event_session:None,
        sessions:BTreeMap::new(), calls:Vec::new(), inbox:VecDeque::new(),
    }));
    (Cdp::from_transport(Box::new(Fake(backend.clone()))), backend)
}

fn attach(cdp: &mut Cdp, target: &str) -> String {
    cdp.call("Target.attachToTarget", json!({"targetId":target,"flatten":true}), None).unwrap()["sessionId"].as_str().unwrap().into()
}

fn refused(result: Result<Value, String>) {
    assert!(result.is_err_and(|message| message.starts_with("slicc: refused:")));
}

#[test]
fn fetch_answers_do_not_probe_a_paused_document() {
    for notify in [false, true] {
        for (method, params) in [
            ("Fetch.continueRequest", json!({"requestId":"R","method":"POST"})),
            ("Fetch.fulfillRequest", json!({"requestId":"R","responseCode":200,"body":"U2lnbmVkIGlu"})),
            ("Fetch.failRequest", json!({"requestId":"R","errorReason":"Aborted"})),
            ("Fetch.continueResponse", json!({"requestId":"R","responseCode":200})),
            ("Fetch.continueWithAuth", json!({"requestId":"R","authChallengeResponse":{"response":"Default"}})),
        ] {
            let (mut cdp, backend) = fixture();
            let session = attach(&mut cdp, "T");
            backend.borrow_mut().inbox.push_back(json!({"method":"Fetch.requestPaused","sessionId":session,"params":{"requestId":"R","request":{"url":"https://ordinary.example/","method":"GET"},"resourceType":"Document","responseStatusCode":200}}).to_string());
            cdp.pump(Instant::now() + std::time::Duration::from_secs(1)).unwrap();
            backend.borrow_mut().block_tree = true;
            let from = backend.borrow().calls.len();
            if notify { cdp.notify(method, params, Some(&session)).unwrap(); } else { cdp.call(method, params, Some(&session)).unwrap(); }
            assert!(backend.borrow().calls[from..].iter().any(|call| call["method"] == method));
            assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == "Page.getFrameTree"));
        }
    }
}

#[test]
fn ordinary_navigation_readiness_and_settings_do_not_require_frame_reads() {
    let (mut cdp, backend) = fixture();
    cdp.call("Target.createTarget", json!({"url":"https://ordinary.example/"}), None).unwrap();
    let session = attach(&mut cdp, "T");
    backend.borrow_mut().block_tree = true;
    let from = backend.borrow().calls.len();
    for (method, params) in [
        ("Page.navigate", json!({"url":"https://ordinary.example/next"})),
        ("Page.reload", json!({})), ("Page.enable", json!({})),
        ("Page.setLifecycleEventsEnabled", json!({"enabled":true})),
        ("Page.navigateToHistoryEntry", json!({"entryId":1})),
        ("Runtime.enable", json!({})), ("DOM.enable", json!({})),
        ("Network.enable", json!({})), ("Accessibility.enable", json!({})),
        ("Network.setExtraHTTPHeaders", json!({"headers":{"X-Test":"ordinary"}})),
        ("Emulation.setFocusEmulationEnabled", json!({"enabled":true})),
        ("Runtime.evaluate", json!({"expression":"document.readyState","returnByValue":true})),
        ("Runtime.evaluate", json!({"expression":"document.readyState === 'complete' ? location.href : ''","returnByValue":true})),
        ("Runtime.evaluate", json!({"expression":"0","returnByValue":true})),
        ("Page.addScriptToEvaluateOnNewDocument", json!({"source":"globalThis.ready=true"})),
        ("Target.activateTarget", json!({"targetId":"T"})),
    ] { cdp.call(method, params, Some(&session)).unwrap(); }
    assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == "Page.getFrameTree"));
    refused(cdp.call("DOM.getDocument", json!({}), Some(&session)));
}

#[test]
fn ordinary_new_tab_settings_and_basics_keep_content_checks() {
    let (mut cdp, backend) = fixture();
    backend.borrow_mut().tree = json!({"frameTree":{"frame":{"id":"F","loaderId":"L1","url":"chrome://new-tab-page/","securityOrigin":"://"},"childFrames":[{"frame":{"id":"CHILD","parentId":"F","loaderId":"","url":"about:blank","securityOrigin":"://"}}]}});
    let session = attach(&mut cdp, "T");
    let from = backend.borrow().calls.len();
    cdp.call("Page.setLifecycleEventsEnabled", json!({"enabled":true}), Some(&session)).unwrap();
    cdp.call("Runtime.evaluate", json!({"expression":"document.readyState"}), Some(&session)).unwrap();
    assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == "Page.getFrameTree"));
    cdp.call("Page.navigate", json!({"url":"https://ordinary.example/"}), Some(&session)).unwrap();
    backend.borrow_mut().tree = tree("L2");
    for method in ["Runtime.evaluate", "DOM.getDocument", "DOMSnapshot.captureSnapshot", "Input.dispatchMouseEvent", "Accessibility.getFullAXTree", "Page.captureScreenshot"] {
        let from = backend.borrow().calls.len();
        cdp.call(method, json!({"expression":"document.title"}), Some(&session)).unwrap();
        assert_eq!(backend.borrow().calls[from..].iter().filter(|call| call["method"] == "Page.getFrameTree").count(), 2);
    }
}

#[test]
fn unknown_commands_and_readiness_with_selectors_fail_closed() {
    let (mut cdp, backend) = fixture();
    let session = attach(&mut cdp, "T");
    backend.borrow_mut().block_tree = true;
    for method in ["Unknown.content", "Page.readFutureContent", "Runtime.readFutureContent"] {
        let from = backend.borrow().calls.len();
        refused(cdp.call(method, json!({}), Some(&session)));
        assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == method));
        refused(cdp.notify(method, json!({}), Some(&session)).map(|_| json!({})));
    }
    refused(cdp.call("Runtime.evaluate", json!({"expression":"document.readyState","awaitPromise":true}), Some(&session)));
    refused(cdp.call("Page.addScriptToEvaluateOnNewDocument", json!({"source":"globalThis.read=true","runImmediately":true}), Some(&session)));
    refused(cdp.call("DOM.getDocument", json!({}), None));
}

#[test]
fn worker_commands_require_current_origin_and_verified_ownership() {
    for kind in ["worker", "service_worker", "shared_worker"] {
        for (url, owner, owner_protected) in [
            ("https://ordinary.example/worker.js", "", false),
            ("https://seven.sliccy.ai/worker.js", "P", false),
            ("https://ordinary.example/worker.js", "P", true),
        ] {
            let (mut cdp, backend) = fixture();
            if !owner.is_empty() { attach(&mut cdp, "P"); }
            backend.borrow_mut().worker = Some(json!({"targetId":"T","type":kind,"url":url,"openerId":owner}));
            let session = match cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None) {
                Ok(result) => result["sessionId"].as_str().unwrap().to_string(),
                Err(message) => { assert!(message.starts_with("slicc: refused:")); continue; }
            };
            if owner_protected {
                backend.borrow_mut().tree["frameTree"]["frame"]["url"] = json!("https://seven.sliccy.ai/document");
                backend.borrow_mut().tree["frameTree"]["frame"]["securityOrigin"] = json!("https://seven.sliccy.ai");
            }
            let from = backend.borrow().calls.len();
            refused(cdp.call("Runtime.evaluate", json!({"expression":"self.origin"}), Some(&session)));
            assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == "Runtime.evaluate"));
        }
    }
}

#[test]
fn content_commands_require_verified_child_frames_and_contexts() {
    for child in [
        json!({"id":"CHILD","loaderId":"LC","url":"https://seven.sliccy.ai/","securityOrigin":"https://seven.sliccy.ai"}),
        json!({"id":"CHILD","loaderId":"LC","url":"https://ordinary.example/"}),
        json!({"id":"CHILD","url":"https://ordinary.example/","securityOrigin":"https://ordinary.example"}),
    ] {
        for params in [json!({"expression":"document.title"}), json!({"expression":"document.title","contextId":7})] {
            let (mut cdp, backend) = fixture();
            let session = attach(&mut cdp, "T");
            backend.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":child}]);
            let from = backend.borrow().calls.len();
            refused(cdp.call("Runtime.evaluate", params, Some(&session)));
            assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == "Runtime.evaluate"));
        }
    }
    let (mut cdp, _) = fixture();
    let session = attach(&mut cdp, "T");
    refused(cdp.call("Runtime.evaluate", json!({"expression":"document.title","contextId":7}), Some(&session)));
}

#[test]
fn document_changes_discard_content_responses_and_notification_results() {
    for notify in [false, true] {
        for event in [false, true] {
            let (mut cdp, backend) = fixture();
            let session = attach(&mut cdp, "T");
            if event { backend.borrow_mut().navigation_event = true; } else { backend.borrow_mut().next_tree = Some(tree("L2")); }
            let result = if notify {
                cdp.notify("Runtime.evaluate", json!({"expression":"document.title"}), Some(&session)).map(|_| json!({}))
            } else {
                cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some(&session))
            };
            refused(result);
        }
    }
    let (mut cdp, _) = fixture();
    refused(cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some("UNKNOWN")));
}

#[test]
fn same_generation_updates_and_other_targets_do_not_invalidate_content() {
    for shape in ["same-document", "duplicate", "other-target"] {
        let (mut cdp, backend) = fixture();
        let other = if shape == "other-target" { Some(attach(&mut cdp, "OTHER")) } else { None };
        let session = attach(&mut cdp, "T");
        if shape == "same-document" {
            let mut next = tree("L1");
            next["frameTree"]["frame"]["url"] = json!("https://ordinary.example/updated#fragment");
            backend.borrow_mut().next_tree = Some(next);
        } else {
            backend.borrow_mut().navigation_event = true;
            backend.borrow_mut().event_session = other;
            if shape == "duplicate" { backend.borrow_mut().event_loader = "L1".into(); }
        }
        let result = cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some(&session)).unwrap();
        assert_eq!(result["result"]["value"], "document-value");
    }
}

#[test]
fn catalog_exceptions_and_initial_blank_documents_have_verified_metadata() {
    for shape in ["blank", "newtab"] {
        let (mut cdp, backend) = fixture();
        if shape == "blank" {
            backend.borrow_mut().tree = json!({"frameTree":{"frame":{"id":"F","loaderId":"","url":"about:blank","securityOrigin":"://"}}});
        } else {
            backend.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":{"id":"CHILD","loaderId":"LC","url":"chrome-untrusted://new-tab-page/","securityOrigin":"://"}}]);
        }
        if shape == "blank" { cdp.call("Target.createTarget", json!({"url":"about:blank"}), None).unwrap(); }
        let session = attach(&mut cdp, "T");
        cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some(&session)).unwrap();
    }
}

#[test]
fn inherited_child_documents_require_verified_unprotected_origins() {
    for url in ["about:blank", "about:srcdoc", "data:text/html,child"] {
        for shape in ["ordinary", "protected-parent", "protected-origin", "unknown-origin", "initial-blank", "initial-protected-parent", "wrong-parent"] {
            let (mut cdp, backend) = fixture();
            let session = attach(&mut cdp, "T");
            let origin = match shape {
                "protected-origin" => "https://vault.sliccy.ai",
                "unknown-origin" => "://",
                _ => "https://ordinary.example",
            };
            backend.borrow_mut().tree["frameTree"]["childFrames"] = json!([{"frame":{"id":"CHILD","parentId":"F","loaderId":"LC","url":url,"securityOrigin":origin}}]);
            if shape.starts_with("initial-") {
                backend.borrow_mut().tree["frameTree"]["childFrames"][0]["frame"]["loaderId"] = json!("");
                backend.borrow_mut().tree["frameTree"]["childFrames"][0]["frame"]["securityOrigin"] = json!("://");
            }
            if shape == "wrong-parent" { backend.borrow_mut().tree["frameTree"]["childFrames"][0]["frame"]["parentId"] = json!("UNKNOWN"); }
            if shape == "protected-parent" || shape == "initial-protected-parent" {
                backend.borrow_mut().tree["frameTree"]["frame"]["url"] = json!("https://vault.sliccy.ai/document");
                backend.borrow_mut().tree["frameTree"]["frame"]["securityOrigin"] = json!("https://vault.sliccy.ai");
            }
            let from = backend.borrow().calls.len();
            let result = cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some(&session));
            if shape == "ordinary" || shape == "initial-blank" && url == "about:blank" { result.unwrap(); } else {
                refused(result);
                assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == "Runtime.evaluate"));
            }
        }
    }
}

#[test]
fn opaque_blank_roots_require_verified_creation_evidence() {
    for shape in ["unknown-opener", "unknown-frame", "known-generation", "missing-generation", "verified-opener", "protected-opener", "unknown-creator-frame"] {
        let (mut cdp, backend) = fixture();
        if matches!(shape, "verified-opener" | "protected-opener" | "unknown-creator-frame") { attach(&mut cdp, "OWNER"); }
        let session = attach(&mut cdp, "T");
        backend.borrow_mut().tree = json!({"frameTree":{"frame":{"id":"BLANK","loaderId":"","url":"about:blank","securityOrigin":"://"}}});
        match shape {
            "unknown-opener" => backend.borrow_mut().worker = Some(json!({"targetId":"T","type":"page","url":"about:blank","openerId":"UNKNOWN"})),
            "unknown-frame" => backend.borrow_mut().worker = Some(json!({"targetId":"T","type":"page","url":"about:blank","openerFrameId":"UNKNOWN"})),
            "known-generation" => backend.borrow_mut().tree["frameTree"]["frame"]["id"] = json!("F"),
            "missing-generation" => backend.borrow_mut().tree["frameTree"]["frame"].as_object_mut().unwrap().remove("loaderId").map(|_| ()).unwrap(),
            _ => {
                backend.borrow_mut().worker = Some(json!({"targetId":"T","type":"page","url":"about:blank","openerId":"OWNER"}));
                if shape == "unknown-creator-frame" { backend.borrow_mut().worker.as_mut().unwrap()["openerFrameId"] = json!("UNKNOWN"); }
                backend.borrow_mut().tree["frameTree"]["frame"]["loaderId"] = json!("LB");
                let mut owner = tree("L1");
                if shape == "protected-opener" {
                    owner["frameTree"]["frame"]["url"] = json!("https://vault.sliccy.ai/document");
                    owner["frameTree"]["frame"]["securityOrigin"] = json!("https://vault.sliccy.ai");
                }
                backend.borrow_mut().owner_tree = Some(owner);
                if shape == "verified-opener" { backend.borrow_mut().tree["frameTree"]["frame"]["securityOrigin"] = json!("https://ordinary.example"); }
            }
        }
        let from = backend.borrow().calls.len();
        let result = cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some(&session));
        if shape == "verified-opener" { result.unwrap(); } else {
            refused(result);
            assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == "Runtime.evaluate"));
        }
    }
}

#[test]
fn history_commands_refuse_unavailable_or_unknown_destination_evidence() {
    for shape in ["timeout", "missing", "empty", "protected", "ordinary", "invalid-id"] {
        let (mut cdp, backend) = fixture();
        let session = attach(&mut cdp, "T");
        backend.borrow_mut().history = match shape {
            "timeout" => None,
            "missing" => Some(json!({})),
            "empty" => Some(json!({"entries":[{"id":1,"url":""}]})),
            "protected" => Some(json!({"entries":[{"id":1,"url":"https://vault.sliccy.ai/document"}]})),
            _ => Some(json!({"entries":[{"id":1,"url":"https://ordinary.example/"}]})),
        };
        let from = backend.borrow().calls.len();
        let params = if shape == "invalid-id" { json!({}) } else { json!({"entryId":1}) };
        let result = cdp.call("Page.navigateToHistoryEntry", params, Some(&session));
        if shape == "ordinary" { result.unwrap(); } else {
            refused(result);
            assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == "Page.navigateToHistoryEntry"));
        }
        if shape == "timeout" { assert!(backend.borrow().deadlines.last().unwrap().as_millis() <= 2000); }
    }
}

#[test]
fn current_policy_is_still_checked_when_a_loader_id_is_unchanged() {
    let (mut cdp, backend) = fixture();
    let session = attach(&mut cdp, "T");
    let mut next = tree("L1");
    next["frameTree"]["frame"]["url"] = json!("https://seven.sliccy.ai/document");
    next["frameTree"]["frame"]["securityOrigin"] = json!("https://seven.sliccy.ai");
    backend.borrow_mut().next_tree = Some(next);
    refused(cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some(&session)));
}

#[test]
fn stream_commands_require_the_document_generation_that_opened_the_handle() {
    for method in ["IO.read", "IO.close"] {
        let (mut cdp, backend) = fixture();
        let session = attach(&mut cdp, "T");
        cdp.call("Page.printToPDF", json!({"transferMode":"ReturnAsStream"}), Some(&session)).unwrap();
        backend.borrow_mut().tree = tree("L2");
        let from = backend.borrow().calls.len();
        refused(cdp.call(method, json!({"handle":"H"}), None));
        assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == method));
    }
}
