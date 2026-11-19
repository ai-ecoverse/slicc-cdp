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
