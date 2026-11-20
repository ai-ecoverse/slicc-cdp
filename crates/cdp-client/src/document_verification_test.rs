use super::*;
use super::super::Transport;
use std::time::Instant;

struct Unused;

impl Transport for Unused {
    fn send_text(&mut self, _: &str) -> Result<(), String> { panic!("unexpected transport call") }
    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { panic!("unexpected transport read") }
}

fn client() -> Cdp { Cdp::from_transport(Box::new(Unused)) }

fn bounds_node(id: &str) -> Value {
    let mut node = json!({"frame":{"id":id,"loaderId":"L","url":"https://ordinary.test/","securityOrigin":"https://ordinary.test"}});
    node["frame"]["id"] = json!(id);
    node
}

fn bounds_chain(depth: usize) -> Value {
    let mut node = bounds_node(&format!("D{depth}"));
    for level in (1..depth).rev() {
        let mut parent = bounds_node(&format!("D{level}"));
        parent["childFrames"] = json!([node]);
        node = parent;
    }
    json!({"frameTree":node})
}

fn bounds_fan(children: usize) -> Value {
    let mut root = bounds_node("ROOT");
    root["childFrames"] = Value::Array((0..children).map(|index| bounds_node(&format!("W{index}"))).collect());
    json!({"frameTree":root})
}

#[test]
fn tree_bounds_accept_the_limit_and_refuse_one_past_it() {
    assert!(tree_bounded(&bounds_chain(MAX_TREE_DEPTH)["frameTree"]));
    assert!(!tree_bounded(&bounds_chain(MAX_TREE_DEPTH + 1)["frameTree"]));
    assert!(tree_bounded(&bounds_fan(MAX_TREE_NODES - 1)["frameTree"]));
    assert!(!tree_bounded(&bounds_fan(MAX_TREE_NODES)["frameTree"]));
}

#[test]
fn deep_or_wide_frame_trees_are_refused_instead_of_walked() {
    let cdp = client();
    let deep = bounds_chain(MAX_TREE_DEPTH + 1);
    let deepest = format!("D{}", MAX_TREE_DEPTH + 1);
    assert!(!cdp.document_tree_verified(&deep, None));
    assert!(!cdp.document_tree_verified(&deep, Some(&deepest)));
    assert!(Cdp::document_frame(&deep["frameTree"], &deepest).is_none());
    let wide = bounds_fan(MAX_TREE_NODES);
    assert!(!cdp.document_tree_verified(&wide, None));
    assert!(Cdp::document_frame(&wide["frameTree"], "W0").is_none());
    let ok = bounds_chain(MAX_TREE_DEPTH);
    assert!(cdp.document_tree_verified(&ok, Some(&format!("D{MAX_TREE_DEPTH}"))));
    assert!(Cdp::document_frame(&ok["frameTree"], &format!("D{MAX_TREE_DEPTH}")).is_some());
}

#[test]
fn every_selected_document_and_ancestor_must_remain_ordinary_and_verified() {
    let mut cdp = client();
    cdp.protect_origins(&["https://catalog.ordinary.test"]);
    let ordinary = bounds_chain(3);
    assert!(cdp.document_tree_verified(&ordinary, Some("D3")));
    for path in ["/frameTree/frame", "/frameTree/childFrames/0/frame", "/frameTree/childFrames/0/childFrames/0/frame"] {
        for url in ["https://seven.sliccy.ai/", "https://vault.sliccy.ai/", "https://catalog.ordinary.test/", "chrome-extension://extension/page", "chrome-error://chromewebdata/"] {
            let mut tree = ordinary.clone();
            let frame = tree.pointer_mut(path).unwrap();
            frame["url"] = json!(url);
            frame["securityOrigin"] = json!(url::Url::parse(url).unwrap().origin().ascii_serialization());
            assert!(!cdp.document_tree_verified(&tree, Some("D3")), "{path}: {url}");
        }
        for (key, value) in [("url", ""), ("url", "unknown"), ("id", ""), ("loaderId", ""), ("securityOrigin", "https://different.test"), ("unreachableUrl", "https://ordinary.test/")] {
            let mut tree = ordinary.clone();
            tree.pointer_mut(path).unwrap()[key] = json!(value);
            assert!(!cdp.document_tree_verified(&tree, Some("D3")), "{path}: {key}");
        }
    }
    assert!(!cdp.document_tree_verified(&ordinary, Some("missing")));
}

#[test]
fn route_attestation_keeps_missing_failed_and_malformed_evidence_closed() {
    for version in [json!({"slicc":{"route":"link"}}), json!({"slicc":{"route":""}}), json!({"slicc":{"route":{"kind":"bios"}}}), json!({"slicc":{"route":false}}), json!({"slicc":false}), json!([])] {
        assert!(Cdp::document_version_attested(Ok(version.clone())), "{version}");
    }
    assert!(Cdp::document_version_attested(Err("unavailable".into())));
    for version in [json!({}), json!({"slicc":null}), json!({"slicc":{}}), json!({"slicc":{"route":{}}})] {
        assert!(!Cdp::document_version_attested(Ok(version.clone())), "{version}");
    }
}

const INSTALL: &str = "Page.addScriptToEvaluateOnNewDocument";

struct Wire {
    sent: std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
    inbox: std::collections::VecDeque<String>,
    during_tree: Vec<Value>,
}

impl Transport for Wire {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let command: Value = serde_json::from_str(text).unwrap();
        self.sent.borrow_mut().push(command.clone());
        let result = if command["method"] == "Page.getFrameTree" {
            for event in self.during_tree.drain(..) { self.inbox.push_back(event.to_string()); }
            attach_tree()
        } else if command["method"] == INSTALL { json!({"identifier":"1"}) } else { json!({}) };
        self.inbox.push_back(json!({"id":command["id"],"sessionId":command["sessionId"],"result":result}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { Ok(self.inbox.pop_front()) }
}

fn attach_child() -> Value {
    json!({"id":"CHILD","parentId":"ROOT","loaderId":"LCHILD","url":"https://ordinary.test/child","securityOrigin":"https://ordinary.test"})
}

fn attach_tree() -> Value {
    json!({"frameTree":{"frame":{"id":"ROOT","loaderId":"LROOT","url":"https://ordinary.test/","securityOrigin":"https://ordinary.test"},"childFrames":[{"frame":attach_child()}]}})
}

fn attach_client(during_tree: Vec<Value>) -> (Cdp, std::rc::Rc<std::cell::RefCell<Vec<Value>>>) {
    let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    (Cdp::from_transport(Box::new(Wire { sent: sent.clone(), inbox: Default::default(), during_tree })), sent)
}

fn child_committed_during_tree_read() -> Value {
    json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":attach_child()}})
}

fn seal(cdp: &mut Cdp) {
    cdp.session_targets.insert("S".into(), "PAGE".into());
    cdp.documents.insert("PAGE".into(), super::super::MainDocument {
        frame_id: "ROOT".into(),
        loader_id: String::new(),
        proof_frame_id: String::new(),
        proof_loader_id: String::new(),
        url: "chrome-error://chromewebdata/".into(),
        origin: "://".into(),
        unreachable_url: "https://vault.sliccy.ai/".into(),
        navigating: false,
        navigation_url: String::new(),
        previous: false,
        probed: false,
    });
}

fn attach_then_install(cdp: &mut Cdp) -> Result<Value, String> {
    let frame = cdp.eval_attach_document("S", "CHILD")?;
    cdp.raw_call(INSTALL, json!({"source":"globalThis.__slicc_eval_attach = true","runImmediately":false}), Some("S"), std::time::Duration::from_secs(1))?;
    Ok(frame)
}

fn methods(sent: &std::rc::Rc<std::cell::RefCell<Vec<Value>>>) -> Vec<String> {
    sent.borrow().iter().filter_map(|command| command["method"].as_str().map(str::to_string)).collect()
}

fn assert_refused_and_uninstalled(cdp: &mut Cdp, sent: &std::rc::Rc<std::cell::RefCell<Vec<Value>>>, case: &str) {
    let refused = attach_then_install(cdp).unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{case}: {refused}");
    let methods = methods(sent);
    assert!(methods.iter().all(|method| matches!(method.as_str(), "Page.getFrameTree" | "Target.getTargetInfo")), "{case}: {methods:?}");
    assert!(methods.iter().any(|method| method == "Page.getFrameTree"), "{case}: {methods:?}");
}

#[test]
fn eval_attach_refuses_when_the_frame_epoch_moves_during_the_tree_read() {
    let (mut cdp, sent) = attach_client(vec![child_committed_during_tree_read()]);
    assert!(!cdp.content_blocked("S"));
    assert_refused_and_uninstalled(&mut cdp, &sent, "epoch");
    assert!(!cdp.content_blocked("S"));
}

#[test]
fn eval_attach_refuses_a_content_blocked_target_with_a_stable_epoch() {
    let (mut cdp, sent) = attach_client(Vec::new());
    seal(&mut cdp);
    assert!(cdp.content_blocked("S") && !cdp.session_blocked("S"));
    assert_refused_and_uninstalled(&mut cdp, &sent, "sealed");
    for (case, block) in [("blocked", (|cdp: &mut Cdp| { cdp.blocked.insert("S".into()); }) as fn(&mut Cdp)), ("guard", |cdp: &mut Cdp| { cdp.guard_sessions.insert("S".into()); })] {
        let (mut cdp, sent) = attach_client(Vec::new());
        block(&mut cdp);
        assert!(cdp.content_blocked("S"), "{case}");
        let refused = attach_then_install(&mut cdp).unwrap_err();
        assert!(refused.starts_with("slicc: refused:"), "{case}: {refused}");
        assert!(!methods(&sent).iter().any(|method| method == INSTALL), "{case}");
    }
}

#[test]
fn eval_attach_refuses_when_the_epoch_moves_and_the_target_is_content_blocked() {
    let (mut cdp, sent) = attach_client(vec![child_committed_during_tree_read()]);
    seal(&mut cdp);
    assert_refused_and_uninstalled(&mut cdp, &sent, "both");
}

#[test]
fn eval_attach_installs_when_the_epoch_is_stable_and_the_target_is_not_blocked() {
    let (mut cdp, sent) = attach_client(Vec::new());
    let frame = attach_then_install(&mut cdp).unwrap();
    assert_eq!(frame, attach_child());
    assert_eq!(methods(&sent), ["Page.getFrameTree", INSTALL]);
}
