use crate::cdp::{Cdp, Transport};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
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
        "frame": { "id": "MAIN", "url": "https://example.test/", "securityOrigin": "https://example.test" },
        "childFrames": [
            { "frame": { "id": "SEVEN", "parentId": "MAIN", "url": "https://seven.sliccy.ai/missing", "securityOrigin": "https://seven.sliccy.ai" } },
            { "frame": { "id": "INHERITED", "parentId": "MAIN", "url": "about:blank", "securityOrigin": "https://vault.sliccy.ai" },
              "childFrames": [
                { "frame": { "id": "DEEP", "parentId": "INHERITED", "url": "about:srcdoc", "securityOrigin": "https://vault.sliccy.ai" } }
              ] },
            { "frame": { "id": "PLAIN", "parentId": "MAIN", "url": "https://example.test/plain", "urlFragment": "#top", "securityOrigin": "https://example.test" } }
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
    for frame in ["MAIN", "PLAIN"] {
        let tree = cdp.call("Accessibility.getFullAXTree", json!({ "frameId": frame }), Some("S")).unwrap();
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

struct Extension {
    sent: Rc<RefCell<Vec<Value>>>,
    incoming: VecDeque<Value>,
    tree: Value,
    tab_refused: Rc<Cell<bool>>,
    attached: usize,
}

impl Transport for Extension {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        self.sent.borrow_mut().push(message.clone());
        let id = message["id"].clone();
        let session = message["sessionId"].clone();
        let target = message["params"]["targetId"].as_str().unwrap_or("").to_string();
        let reply = match message["method"].as_str().unwrap() {
            "Target.attachToTarget" => {
                self.attached += 1;
                json!({ "id": id, "result": { "sessionId": if self.attached == 1 { "S".to_string() } else { format!("GUARD{}", self.attached) } } })
            }
            "Target.getTargetInfo" if target == "7" && !self.tab_refused.get() => json!({ "id": id, "result": {
                "targetInfo": { "targetId": "7", "type": "page", "url": "http://localhost:43117/driven.html", "title": "t" }
            }}),
            "Target.getTargetInfo" => json!({ "id": id, "error": { "message": format!("slicc: refused: Invalid targetId: {target} is not a tab") } }),
            "Page.getFrameTree" => json!({ "id": id, "sessionId": session, "result": self.tree.clone() }),
            _ => json!({ "id": id, "sessionId": session, "result": { "executionContextId": 5 } }),
        };
        self.incoming.push_back(reply);
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(self.incoming.pop_front().map(|value| value.to_string()))
    }
}

const MAIN_FRAME: &str = "A1B2C3D4E5F60718293A4B5C6D7E8F90";

fn extension_tree(main: &str) -> Value {
    json!({ "frameTree": {
        "frame": { "id": main, "url": "http://localhost:43117/driven.html", "securityOrigin": "http://localhost:43117" },
        "childFrames": [
            { "frame": { "id": "PLAIN", "parentId": main, "url": "http://localhost:43118/child.html", "securityOrigin": "http://localhost:43118" } },
            { "frame": { "id": "SEVENPORT", "parentId": main, "url": "http://localhost:5710/", "securityOrigin": "http://localhost:5710" } },
            { "frame": { "id": "SEVEN", "parentId": main, "url": "https://seven.sliccy.ai/", "securityOrigin": "https://seven.sliccy.ai" } },
            { "frame": { "id": "VAULT", "parentId": main, "url": "about:blank", "securityOrigin": "https://vault.sliccy.ai" } },
            { "frame": { "id": "OPAQUE", "parentId": main, "url": "about:blank", "securityOrigin": "null" } }
        ]
    }})
}

fn over_extension(main: &str) -> (Cdp, Rc<RefCell<Vec<Value>>>, Rc<Cell<bool>>) {
    let sent = Rc::new(RefCell::new(Vec::new()));
    let tab_refused = Rc::new(Cell::new(false));
    let mut cdp = Cdp::from_transport(Box::new(Extension { sent: sent.clone(), incoming: VecDeque::new(), tree: extension_tree(main), tab_refused: tab_refused.clone(), attached: 0 }));
    cdp.protect_origins(&["http://localhost:5710"]);
    assert_eq!(crate::cdp::attach(&mut cdp, "7").unwrap(), "S");
    (cdp, sent, tab_refused)
}

#[test]
fn a_refused_lookup_of_a_frame_id_that_is_not_the_sessions_tab_defers_to_the_frame_tree() {
    let reads = FRAME_READS.iter().filter(|(method, _)| *method != "Page.getResourceContent").map(|(method, params)| (*method, params(""))).chain([("Page.getResourceContent", json!({ "url": "http://localhost:43117/driven.html" }))]);
    for frame in [MAIN_FRAME, "PLAIN"] {
        for (method, mut params) in reads.clone() {
            params["frameId"] = json!(frame);
            let (mut cdp, sent, _) = over_extension(MAIN_FRAME);
            assert!(cdp.call(method, params, Some("S")).is_ok(), "{method} {frame}");
            assert_eq!(sent_with_frame(&sent, frame), [method], "{method} {frame}");
            assert!(!cdp.is_protected_target(frame), "{method} {frame}");
        }
    }
}

#[test]
fn frames_on_sevens_exact_origin_or_the_vault_stay_refused_over_the_extension() {
    for frame in ["SEVENPORT", "SEVEN", "VAULT", "OPAQUE", "GHOST"] {
        for (method, params) in FRAME_READS {
            let (mut cdp, sent, _) = over_extension(MAIN_FRAME);
            let refused = cdp.call(method, params(frame), Some("S")).unwrap_err();
            assert!(refused.starts_with("slicc: refused:"), "{method} {frame}: {refused}");
            assert!(sent_with_frame(&sent, frame).is_empty(), "{method} {frame}");
        }
    }
}

#[test]
fn a_refused_lookup_still_protects_the_sessions_own_target_and_sessionless_frames() {
    let (mut cdp, sent, tab_refused) = over_extension("7");
    tab_refused.set(true);
    let refused = cdp.call("Page.createIsolatedWorld", json!({ "frameId": "7" }), Some("S")).unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{refused}");
    assert!(cdp.is_protected_target("7"));
    assert!(sent_with_frame(&sent, "7").is_empty());
    let (mut cdp, sent, _) = over_extension(MAIN_FRAME);
    let refused = cdp.call("Page.createIsolatedWorld", json!({ "frameId": MAIN_FRAME }), None).unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{refused}");
    assert!(sent_with_frame(&sent, MAIN_FRAME).is_empty());
}
