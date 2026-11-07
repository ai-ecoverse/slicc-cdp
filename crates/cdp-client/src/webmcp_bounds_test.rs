fn bounds_node(id: &str) -> Value {
    let mut node = frame("https://ordinary.test/")["frameTree"].clone();
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
    let (cdp, _state) = client();
    let deep = bounds_chain(MAX_TREE_DEPTH + 1);
    let deepest = format!("D{}", MAX_TREE_DEPTH + 1);
    assert!(!cdp.webmcp_tree_verified(&deep, None));
    assert!(!cdp.webmcp_tree_verified(&deep, Some(&deepest)));
    assert!(Cdp::webmcp_frame(&deep["frameTree"], &deepest).is_none());
    let mut loaders = BTreeMap::new();
    Cdp::webmcp_loaders(&deep["frameTree"], &mut loaders);
    assert!(loaders.is_empty());
    let wide = bounds_fan(MAX_TREE_NODES);
    assert!(!cdp.webmcp_tree_verified(&wide, None));
    assert!(Cdp::webmcp_frame(&wide["frameTree"], "W0").is_none());
    let ok = bounds_chain(MAX_TREE_DEPTH);
    assert!(cdp.webmcp_tree_verified(&ok, Some(&format!("D{MAX_TREE_DEPTH}"))));
    assert!(Cdp::webmcp_frame(&ok["frameTree"], &format!("D{MAX_TREE_DEPTH}")).is_some());
}

#[test]
fn an_ancestry_chain_past_the_depth_bound_is_not_walked() {
    let (mut cdp, _state) = client();
    for index in 0..(MAX_TREE_DEPTH + 8) {
        cdp.webmcp.parents.insert(format!("S{index}"), format!("S{}", index + 1));
    }
    assert_eq!(cdp.webmcp_trace_page("S0"), "unknown");
}
