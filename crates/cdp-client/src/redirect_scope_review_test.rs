struct RedirectScopeTransport;
impl Transport for RedirectScopeTransport {
    fn send_text(&mut self, _: &str) -> Result<(), String> { Ok(()) }
    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { Ok(None) }
}

fn scope_request(cdp: &mut Cdp, id: &str) {
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":id,"type":"Document","request":{"url":"https://ordinary.invalid/redirect","method":"GET","headers":{}}}}));
}

#[test]
fn blocked_session_redirects_never_publish_carriers_or_opaque_headers() {
    for seen in [false, true] {
        let mut cdp = Cdp::from_transport(Box::new(RedirectScopeTransport));
        if seen { scope_request(&mut cdp, "N"); }
        cdp.blocked.insert("S".into());
        cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"S","params":{"requestId":"F","networkId":"N","frameId":"FR","resourceType":"XHR","request":{"url":"https://seven.sliccy.ai/oauth","method":"GET","headers":{}},"responseStatusCode":302,"responseHeaders":[{"name":"Location","value":"https://vault.sliccy.ai/callback"},{"name":"X-Private","value":"BLOCKED_SCOPE_OPAQUE"}]}}));
        cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"N","statusCode":302,"headers":{"location":"https://ordinary.invalid/cb?code=BLOCKED_SCOPE_LOCATION","x-csrf":"BLOCKED_SCOPE_CSRF"},"headersText":"HTTP/1.1 302\r\nlocation: https://seven.sliccy.ai/x\r\n\r\n"}}));
        cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"N","response":{"url":"https://seven.sliccy.ai/oauth","status":302,"headers":{"Location":"https://ordinary.invalid/cb?code=BLOCKED_SCOPE_LOCATION","X-Private":"BLOCKED_SCOPE_OPAQUE"}}}}));
        let events = cdp.events();
        assert!(!events.iter().any(|event| matches!(event["method"].as_str(), Some("Network.responseReceivedExtraInfo" | "Network.responseReceived" | "Fetch.requestPaused"))));
        assert_eq!(events.iter().filter(|event| event["method"] == "Network.loadingFailed").count(), usize::from(seen));
        let output = serde_json::to_string(events).unwrap();
        for secret in ["BLOCKED_SCOPE_OPAQUE", "BLOCKED_SCOPE_LOCATION", "BLOCKED_SCOPE_CSRF"] { assert!(!output.contains(secret)); }
    }
}

#[test]
fn protected_page_or_response_origin_redirects_never_publish_headers() {
    for page in [false, true] {
        for method in ["Network.responseReceived", "Network.requestWillBeSent", "Fetch.requestPaused"] {
            let mut cdp = Cdp::from_transport(Box::new(RedirectScopeTransport));
            if page { cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"FR","url":"https://seven.sliccy.ai/private","securityOrigin":"https://seven.sliccy.ai"}}})); }
            let source = if page { "https://ordinary.invalid/redirect" } else { "https://seven.sliccy.ai/private" };
            let params = match method {
                "Network.responseReceived" => json!({"requestId":"N","response":{"url":source,"status":302,"headers":{"Location":"https://vault.sliccy.ai/callback","X-Private-Token":"PROTECTED_SOURCE_RESPONSE"}}}),
                "Network.requestWillBeSent" => json!({"requestId":"N","request":{"url":"https://vault.sliccy.ai/callback","headers":{}},"redirectResponse":{"url":source,"status":302,"headers":{"Location":"https://vault.sliccy.ai/callback","X-Private-Token":"PROTECTED_SOURCE_RESPONSE"}}}),
                _ => json!({"requestId":"F","networkId":"N","frameId":"FR","resourceType":"Document","request":{"url":source,"method":"GET","headers":{}},"responseStatusCode":302,"responseHeaders":[{"name":"Location","value":"https://vault.sliccy.ai/callback"},{"name":"X-Private-Token","value":"PROTECTED_SOURCE_RESPONSE"}]}),
            };
            if method == "Network.requestWillBeSent" && !page { scope_request(&mut cdp, "N"); }
            let before = cdp.events.len();
            cdp.absorb(json!({"method":method,"sessionId":"S","params":params}));
            assert!(cdp.events()[before..].iter().all(|event| event["method"] == "Network.loadingFailed"), "{method} {page}");
            assert!(!serde_json::to_string(cdp.events()).unwrap().contains("PROTECTED_SOURCE_RESPONSE"));
        }
    }
}

#[test]
fn protected_request_page_and_held_blocked_events_cannot_reenter_delivery() {
    let mut cdp = Cdp::from_transport(Box::new(RedirectScopeTransport));
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"P","documentURL":"https://seven.sliccy.ai/private","request":{"url":"https://ordinary.invalid/hop","headers":{}}}}));
    cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"P","statusCode":302,"headers":{"Location":"https://vault.sliccy.ai/private","X-CSRF":"PRIVATE_PAGE_EXTRA_SECRET"}}}));
    assert!(cdp.events().is_empty());
    let mut cdp = Cdp::from_transport(Box::new(RedirectScopeTransport));
    cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"HELD","statusCode":302,"headers":{"Location":"https://ordinary.invalid/cb?secret=HELD_BLOCKED_SECRET"}}}));
    cdp.blocked.insert("S".into());
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"OTHER","params":{"requestId":"HELD","request":{"url":"https://ordinary.invalid/other","headers":{}}}}));
    assert!(!cdp.events().iter().any(|event| event["sessionId"] == "S"));
    assert!(!serde_json::to_string(cdp.events()).unwrap().contains("HELD_BLOCKED_SECRET"));
}

#[test]
fn redacted_redirect_hops_keep_only_four_carriers_and_rebuild_headers_text() {
    let headers = json!({"Location":"https://seven.sliccy.ai/private?code=ALLOWLIST_DESTINATION","Content-Location":"https://seven.sliccy.ai/private","Refresh":"0;url=https://seven.sliccy.ai/private","Link":"<https://seven.sliccy.ai/private>; rel=preconnect","X-CSRF":"ALLOWLIST_OPAQUE_SECRET","ETag":"ALLOWLIST_OPAQUE_SECRET","Content-Type":"ALLOWLIST_OPAQUE_SECRET","Set-Cookie":"ALLOWLIST_OPAQUE_SECRET"});
    for method in ["Network.responseReceivedExtraInfo", "Network.responseReceived", "Fetch.requestPaused"] {
        let mut cdp = Cdp::from_transport(Box::new(RedirectScopeTransport));
        scope_request(&mut cdp, "N");
        let params = match method {
            "Network.responseReceivedExtraInfo" => json!({"requestId":"N","statusCode":302,"headers":headers,"headersText":"HTTP/1.1 302\r\nX-CSRF: ALLOWLIST_OPAQUE_SECRET\r\nLocation: https://seven.sliccy.ai/private?code=ALLOWLIST_DESTINATION\r\n\r\n"}),
            "Network.responseReceived" => json!({"requestId":"N","response":{"url":"https://ordinary.invalid/redirect","status":302,"headers":headers}}),
            _ => json!({"requestId":"F","networkId":"N","resourceType":"Document","request":{"url":"https://ordinary.invalid/redirect","method":"GET","headers":{}},"responseStatusCode":302,"responseHeaders":headers.as_object().unwrap().iter().map(|(name,value)|json!({"name":name,"value":value,"extra":"ALLOWLIST_OPAQUE_SECRET"})).collect::<Vec<_>>()}),
        };
        cdp.absorb(json!({"method":method,"sessionId":"S","params":params}));
        let event = cdp.events().iter().find(|event| event["method"] == method).unwrap();
        let delivered = match method {
            "Network.responseReceivedExtraInfo" => event["params"]["headers"].clone(),
            "Network.responseReceived" => event["params"]["response"]["headers"].clone(),
            _ => Value::Object(event["params"]["responseHeaders"].as_array().unwrap().iter().map(|field|(field["name"].as_str().unwrap().into(),field["value"].clone())).collect()),
        };
        assert_eq!(delivered.as_object().unwrap().len(),4);
        assert_eq!(delivered["Location"],"https://seven.sliccy.ai");
        assert_eq!(delivered["Content-Location"],"https://seven.sliccy.ai");
        assert_eq!(delivered["Refresh"],"0;url=https://seven.sliccy.ai");
        assert_eq!(delivered["Link"],"<https://seven.sliccy.ai>; rel=preconnect");
        if method == "Network.responseReceivedExtraInfo" {
            for (name, value) in delivered.as_object().unwrap() { assert!(event["params"]["headersText"].as_str().unwrap().contains(&format!("{name}: {}\r\n",value.as_str().unwrap()))); }
        }
        let output=serde_json::to_string(cdp.events()).unwrap();
        assert!(!output.contains("ALLOWLIST_OPAQUE_SECRET"));
        assert!(!output.contains("ALLOWLIST_DESTINATION"));
    }
    let mut cdp = Cdp::from_transport(Box::new(RedirectScopeTransport));
    scope_request(&mut cdp,"CONTROL");
    let headers=json!({"Location":"https://ordinary.invalid/plain?positive=kept","X-Control":"ordinary-public-bytes","Content-Type":"text/html"});
    cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"CONTROL","response":{"url":"https://ordinary.invalid/redirect","status":302,"headers":headers}}}));
    assert_eq!(cdp.events().last().unwrap()["params"]["response"]["headers"],headers);
}
