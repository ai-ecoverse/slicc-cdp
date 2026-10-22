#[test]
fn ordinary_resource_reads_in_a_fresh_target_do_not_inherit_another_targets_body_taint() {
    let mut cdp = redirect_body_backend("ORDINARY_FRESH_RESOURCE_CONTROL".into());
    cdp.session_targets.insert("A".into(), "OLD".into());
    cdp.session_targets.insert("B".into(), "NEW".into());
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"A","params":{"requestId":"R","request":{"url":"https://normal.invalid/redirect?kind=link"}}}));
    cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"A","params":{"requestId":"R","statusCode":302,"headers":{"Location":"/ordinary","Link":"<https://seven.sliccy.ai/private>; rel=prefetch"}}}));
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"A","params":{"requestId":"R","request":{"url":"https://normal.invalid/ordinary"},"redirectResponse":{"url":"https://normal.invalid/redirect?kind=link","status":302,"headers":{"Link":"<https://seven.sliccy.ai/private>; rel=prefetch"}}}}));
    cdp.hand("B", "HOLD");
    cdp.requests.insert("HOLD".into(), false);
    assert!(cdp.call("Fetch.continueRequest", json!({"requestId":"HOLD","url":"https://normal.invalid/redirect?kind=other"}), Some("B")).unwrap_err().starts_with("slicc: refused:"));
    assert!(cdp.resource_protected("A", "https://normal.invalid/ordinary"));
    assert!(!cdp.resource_protected("B", "https://normal.invalid/ordinary?control=readable"));
    assert!(cdp.call("Page.getResourceContent", json!({"url":"https://normal.invalid/ordinary"}), Some("A")).unwrap_err().starts_with("slicc: refused:"));
    assert_eq!(cdp.call("Page.getResourceContent", json!({"url":"https://normal.invalid/ordinary?control=readable"}), Some("B")).unwrap()["content"], "ORDINARY_FRESH_RESOURCE_CONTROL");
}
