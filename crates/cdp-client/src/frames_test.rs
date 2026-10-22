use crate::cdp::{Cdp, Transport};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

struct Backend {
    sent: Rc<RefCell<Vec<Value>>>,
    incoming: VecDeque<Value>,
    tree: Option<Value>,
    broken: &'static str,
}

fn frame_tree() -> Value {
    json!({ "frameTree": {
        "frame": { "id": "MAIN", "loaderId": "LMAIN", "url": "https://example.test/", "securityOrigin": "https://example.test" },
        "childFrames": [
            { "frame": { "id": "SEVEN", "loaderId": "LSEVEN", "parentId": "MAIN", "url": "https://seven.sliccy.ai/missing", "securityOrigin": "https://seven.sliccy.ai" } },
            { "frame": { "id": "INHERITED", "loaderId": "LINHERITED", "parentId": "MAIN", "url": "about:blank", "securityOrigin": "https://vault.sliccy.ai" },
              "childFrames": [
                { "frame": { "id": "DEEP", "loaderId": "LDEEP", "parentId": "INHERITED", "url": "about:srcdoc", "securityOrigin": "https://vault.sliccy.ai" } }
              ] },
            { "frame": { "id": "PLAIN", "loaderId": "LPLAIN", "parentId": "MAIN", "url": "https://example.test/plain", "urlFragment": "#top", "securityOrigin": "https://example.test" } }
        ]
    }})
}

impl Transport for Backend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        if message["method"] == self.broken {
            return Err("websocket closed 1006".to_string());
        }
        self.sent.borrow_mut().push(message.clone());
        let id = message["id"].clone();
        let session = message["sessionId"].clone();
        let reply = match message["method"].as_str().unwrap() {
            "Page.getFrameTree" if self.tree.is_some() => json!({ "id": id, "sessionId": session, "result": self.tree.clone() }),
            "Page.getFrameTree" => json!({ "id": id, "sessionId": session, "error": { "message": "Page.getFrameTree wasn't found" } }),
            "Target.getTargetInfo" if message["params"]["targetId"] == "MAIN" => json!({ "id": id, "result": {
                "targetInfo": { "targetId": "MAIN", "type": "page", "url": "https://example.test/", "title": "t" }
            }}),
            "Target.getTargetInfo" => json!({ "id": id, "error": { "message": "No target with given id found" } }),
            "Target.attachToTarget" => {
                let attached = self.sent.borrow().iter().filter(|call| call["method"] == "Target.attachToTarget").count();
                json!({"id":id,"result":{"sessionId":if attached == 1 {"S"} else {"G"}}})
            }
            "Target.getTargets" => json!({"id":id,"result":{"targetInfos":[{"targetId":"MAIN","type":"page","url":"https://example.test/"}]}}),
            "Page.createIsolatedWorld" => json!({ "id": id, "sessionId": session, "result": { "executionContextId": 5 } }),
            _ => json!({ "id": id, "sessionId": session, "result": { "nodes": [{ "name": { "value": "PROTECTED-TEXT" } }] } }),
        };
        self.incoming.push_back(reply);
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(self.incoming.pop_front().map(|value| value.to_string()))
    }
}

fn client(tree: bool) -> (Cdp, Rc<RefCell<Vec<Value>>>) {
    broken_client(tree.then(frame_tree), "")
}

fn broken_client(tree: Option<Value>, broken: &'static str) -> (Cdp, Rc<RefCell<Vec<Value>>>) {
    let sent = Rc::new(RefCell::new(Vec::new()));
    let cdp = Cdp::from_transport(Box::new(Backend { sent: sent.clone(), incoming: VecDeque::new(), tree, broken }));
    (cdp, sent)
}

fn sent_with_frame(sent: &Rc<RefCell<Vec<Value>>>, frame: &str) -> Vec<String> {
    sent.borrow()
        .iter()
        .filter(|message| message["params"]["frameId"] == frame)
        .map(|message| message["method"].as_str().unwrap().to_string())
        .collect()
}

const FRAME_READS: [(&str, fn(&str) -> Value); 5] = [
    ("Accessibility.getFullAXTree", |frame| json!({ "frameId": frame })),
    ("Accessibility.getRootAXNode", |frame| json!({ "frameId": frame })),
    ("DOM.getFrameOwner", |frame| json!({ "frameId": frame })),
    ("Page.createIsolatedWorld", |frame| json!({ "frameId": frame, "grantUniveralAccess": true })),
    ("Page.getResourceContent", |frame| json!({ "frameId": frame, "url": "https://seven.sliccy.ai/missing" })),
];

#[test]
fn same_process_protected_child_frames_committed_before_attach_are_refused() {
    for frame in ["SEVEN", "INHERITED", "DEEP"] {
        for (method, params) in FRAME_READS {
            let (mut cdp, sent) = client(true);
            let refused = cdp.call(method, params(frame), Some("S")).unwrap_err();
            assert!(refused.starts_with("slicc: refused:"), "{method} {frame}: {refused}");
            assert!(sent_with_frame(&sent, frame).is_empty(), "{method} {frame} was sent");
        }
    }
}

#[test]
fn a_frame_tree_reply_is_recorded_for_later_frame_reads() {
    let (mut cdp, sent) = client(true);
    let tree = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(tree["frameTree"]["childFrames"][0]["frame"]["url"], "https://seven.sliccy.ai");
    let before = sent.borrow().len();
    assert!(cdp.call("Accessibility.getFullAXTree", json!({ "frameId": "SEVEN" }), Some("S")).is_err());
    assert_eq!(sent.borrow().len(), before);
}

#[test]
fn unknown_frames_fail_closed() {
    for tree in [Some(frame_tree()), Some(json!({})), None] {
        let (mut cdp, sent) = broken_client(tree.clone(), "");
        for (method, params) in FRAME_READS {
            assert!(cdp.call(method, params("GHOST"), Some("S")).is_err(), "{method}");
        }
        assert!(sent_with_frame(&sent, "GHOST").is_empty());
        let (mut cdp, sent) = broken_client(tree, "");
        assert!(cdp.call("Accessibility.getFullAXTree", json!({ "frameId": "GHOST" }), None).is_err());
        assert!(sent_with_frame(&sent, "GHOST").is_empty());
    }
}

#[test]
fn ordinary_frames_stay_readable() {
    let (mut cdp, sent) = client(true);
    cdp.call("Target.attachToTarget", json!({"targetId":"MAIN","flatten":true}), None).unwrap();
    for frame in ["MAIN", "PLAIN"] {
        let tree = cdp.call("Accessibility.getFullAXTree", json!({ "frameId": frame }), Some("S")).unwrap_or_else(|error| panic!("{frame}: {error}; {:?}", sent.borrow()));
        assert_eq!(tree["nodes"][0]["name"]["value"], "PROTECTED-TEXT");
        assert_eq!(sent_with_frame(&sent, frame), ["Accessibility.getFullAXTree"]);
    }
    let (mut cdp, _) = client(false);
    assert!(cdp.call("Page.createIsolatedWorld", json!({ "frameId": "MAIN" }), Some("S")).is_ok());
}

#[test]
fn a_dead_socket_while_checking_a_frame_is_reported_not_guessed() {
    for broken in ["Target.getTargetInfo", "Page.getFrameTree"] {
        let (mut cdp, sent) = broken_client(Some(frame_tree()), broken);
        let error = cdp.call("Accessibility.getFullAXTree", json!({ "frameId": "SEVEN" }), Some("S")).unwrap_err();
        assert!(error.starts_with("websocket closed"), "{broken}: {error}");
        assert!(sent_with_frame(&sent, "SEVEN").is_empty());
    }
}
