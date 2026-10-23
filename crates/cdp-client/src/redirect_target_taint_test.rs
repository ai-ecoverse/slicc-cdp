#[test]
fn known_redirector_taint_survives_a_fresh_target_and_session() {
    for method in ["Fetch.continueRequest", "Network.continueInterceptedRequest"] {
        let mut cdp = redirect_body_backend("NEW_TARGET_ORDINARY_CONTROL".into());
        cdp.session_targets.insert("A".into(), "OLD-TARGET".into());
        fixture_target(&mut cdp, "B", "NEW-TARGET");
        cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"A","params":{"requestId":"OLD-REDIRECT","request":{"url":"https://ordinary.invalid/redirector?original=1"}}}));
        cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"A","params":{"requestId":"OLD-REDIRECT","statusCode":302,"headers":{"Location":"https://seven.sliccy.ai/private"}}}));
        cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"B","params":{"requestId":"NEW-HOLD","networkId":"NEW-NET","resourceType":"XHR","request":{"url":"https://ordinary.invalid/ordinary","method":"GET","headers":{}}}}));
        assert!(cdp.redirector_protected("https://user@ORDINARY.invalid:443/redirector/?new=1#fragment"));
        assert!(!cdp.resource_protected("B", "https://ordinary.invalid/redirector?other=ordinary"));
        for url in ["https://ordinary.invalid/redirector", "https://ordinary.invalid/redirector?changed=1", "https://user:pass@ordinary.invalid/redirector/"] {
            let mut params = json!({"url":url});
            params[if method == "Fetch.continueRequest" { "requestId" } else { "interceptionId" }] = json!("NEW-HOLD");
            assert!(cdp.call(method, params, Some("B")).unwrap_err().starts_with("slicc: refused:"), "{method} {url}");
        }
        assert!(!cdp.resource_protected("B", "https://ordinary.invalid/ordinary?control=1"));
        assert_eq!(cdp.call("Network.getResponseBody", json!({"requestId":"NEW-NET"}), Some("B")).unwrap()["body"], "NEW_TARGET_ORDINARY_CONTROL");
    }
}
