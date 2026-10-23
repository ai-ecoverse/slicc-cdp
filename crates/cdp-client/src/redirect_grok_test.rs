#[test]
fn tainted_unseen_and_non_redirect_carriers_never_deliver_partial_headers() {
    let destination = "https://seven.sliccy.ai/private?code=GROK_DESTINATION_91#fragment";
    let headers = json!({"Location":destination,"Refresh":format!("0;url={destination}"),"Link":format!("<{destination}>;rel=prefetch"),"Cookie":"cookie=GROK_COOKIE_91","Set-Cookie":"cookie=GROK_COOKIE_91","X-Private":"GROK_OPAQUE_91"});
    for (method, params, seen) in [
        ("Network.requestWillBeSent", json!({"requestId":"R","request":{"url":"https://ordinary.invalid/next","method":"GET","headers":{"Cookie":"GROK_COOKIE_91"}},"redirectResponse":{"url":"https://ordinary.invalid/redirector","status":302,"headers":headers}}), false),
        ("Network.responseReceived", json!({"requestId":"R","response":{"url":"https://ordinary.invalid/redirector","status":200,"headers":headers}}), true),
        ("Network.responseReceivedExtraInfo", json!({"requestId":"R","statusCode":200,"headers":headers,"headersText":format!("HTTP/1.1 200 OK\r\nCookie: GROK_COOKIE_91\r\nLocation: {destination}\r\n\r\n")}), true),
    ] {
        let mut cdp = url_backend("about:blank");
        if seen {
            cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","type":"Document","request":{"url":"https://ordinary.invalid/redirector","method":"GET","headers":{}}}}));
        }
        cdp.absorb(json!({"method":method,"sessionId":"S","params":params}));
        let delivered = serde_json::to_string(cdp.events()).unwrap();
        for marker in ["GROK_DESTINATION_91", "GROK_COOKIE_91", "GROK_OPAQUE_91"] {
            assert!(!delivered.contains(marker), "{method} exposed {marker}: {delivered}");
        }
        assert_eq!(cdp.events().iter().filter(|event| event["method"] == "Network.loadingFailed").count(), usize::from(seen));
        cdp.absorb(json!({"method":"Network.loadingFinished","sessionId":"S","params":{"requestId":"R","timestamp":2}}));
        assert_eq!(cdp.events().iter().filter(|event| event["method"] == "Network.loadingFailed").count(), usize::from(seen));
        if seen { assert!(!cdp.events().iter().any(|event| event["method"] == "Network.loadingFinished")); }
    }
}

#[test]
fn redirector_override_aliases_are_refused_in_both_interception_apis() {
    for method in ["Fetch.continueRequest", "Network.continueInterceptedRequest"] {
        let mut cdp = redirect_body_backend("GROK_ORDINARY_CONTROL_27".into());
        fixture_target(&mut cdp, "S", "T");
        cdp.session_targets.insert("G".into(), "T".into());
        cdp.guard_sessions.insert("G".into());
        cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"G","params":{"requestId":"TAINT","request":{"url":"https://ordinary.invalid/redirector?seed=private#seed"}}}));
        cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"G","params":{"requestId":"TAINT","statusCode":302,"headers":{"Location":"https://seven.sliccy.ai/private"}}}));
        cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"S","params":{"requestId":"HOLD","networkId":"PLAIN","resourceType":"XHR","request":{"url":"https://ordinary.invalid/plain","method":"GET","headers":{}}}}));
        for url in ["https://ordinary.invalid/redirector?changed=1", "https://ordinary.invalid/redirector/", "https://user:pass@ordinary.invalid/redirector", "https://ORDINARY.INVALID:443/a/../redirector/?q=1#x", "blob:https://ordinary.invalid/redirector?blob=1", "://"] {
            let mut params = json!({"url":url});
            params[if method == "Fetch.continueRequest" { "requestId" } else { "interceptionId" }] = json!("HOLD");
            assert!(cdp.call(method, params, Some("S")).unwrap_err().starts_with("slicc: refused:"), "{method} {url}");
        }
        assert_eq!(cdp.call("Network.getResponseBody", json!({"requestId":"PLAIN"}), Some("S")).unwrap()["body"], "GROK_ORDINARY_CONTROL_27");
        assert_eq!(cdp.call("Page.getResourceContent", json!({"url":"https://ordinary.invalid/plain"}), Some("S")).unwrap()["content"], "GROK_ORDINARY_CONTROL_27");
        let mut params = json!({"url":"https://ordinary.invalid/plain?readable=1"});
        params[if method == "Fetch.continueRequest" { "requestId" } else { "interceptionId" }] = json!("HOLD");
        cdp.call(method, params, Some("S")).unwrap();
    }
}
