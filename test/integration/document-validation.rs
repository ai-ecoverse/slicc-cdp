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
    next_worker_url: Option<String>,
    navigation_event: bool,
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
            "Target.getTargetInfo" => json!({"targetInfo":if target == "T" { backend.worker.clone().unwrap_or_else(|| json!({"targetId":"T","type":"page","url":"https://ordinary.example/"})) } else { json!({"targetId":target,"type":"page","url":"https://ordinary.example/"}) }}),
            "Target.attachToTarget" => {
                let session = format!("S{}", backend.sessions.len() + 1);
                backend.sessions.insert(session.clone(), target.into());
                json!({"sessionId":session})
            }
            "Page.getFrameTree" => backend.tree.clone(),
            "Runtime.enable" => {
                backend.inbox.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":call["sessionId"],"params":{"context":{"id":7,"uniqueId":"child-context","name":"child-world","origin":"https://ordinary.example","auxData":{"frameId":"CHILD","isDefault":false}}}}).to_string());
                json!({})
            }
            "Runtime.evaluate" => {
                if let Some(next) = backend.next_tree.take() { backend.tree = next; }
                if let Some(url) = backend.next_worker_url.take() { backend.worker.as_mut().unwrap()["url"] = json!(url); }
                if backend.navigation_event {
                    backend.inbox.push_back(json!({"method":"Page.navigatedWithinDocument","sessionId":call["sessionId"],"params":{"frameId":"F","url":"https://ordinary.example/"}}).to_string());
                }
                json!({"result":{"type":"string","value":"document-value"}})
            }
            "Page.printToPDF" => json!({"stream":"H"}),
            _ => json!({}),
        };
        backend.inbox.push_back(json!({"id":call["id"],"result":result}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(self.0.borrow_mut().inbox.pop_front())
    }
}

fn fixture() -> (Cdp, Rc<RefCell<Backend>>) {
    let backend = Rc::new(RefCell::new(Backend {
        worker:None, tree:tree("L1"), next_tree:None, next_worker_url:None, navigation_event:false,
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
