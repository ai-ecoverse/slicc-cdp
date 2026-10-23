#[test]
fn redirect_terminal_seen_request_is_redacted_and_finished_once() {
    let mut cdp = url_backend("about:blank");
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"TERMINAL","type":"Document","timestamp":1,"request":{"url":"https://ordinary.invalid/start","method":"GET","headers":{}}}}));
    let url = "https://seven.sliccy.ai/private?token=TERMINAL_FAKE_SECRET#fragment";
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"TERMINAL","type":"Document","timestamp":2,"documentURL":url,"request":{"url":url,"method":"GET","headers":{"Cookie":"TERMINAL_FAKE_SECRET"},"postData":"TERMINAL_FAKE_SECRET","urlFragment":"#TERMINAL_FAKE_SECRET"},"redirectResponse":{"url":"https://ordinary.invalid/start","status":302,"headers":{"Location":url}}}}));
    cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"TERMINAL","response":{"url":url,"status":200}}}));
    for method in ["Network.loadingFailed", "Network.loadingFinished"] {
        cdp.absorb(json!({"method":method,"sessionId":"S","params":{"requestId":"TERMINAL","timestamp":3}}));
    }
    let events = cdp.events();
    let redirects: Vec<_> = events.iter().filter(|event| event.pointer("/params/redirectResponse").is_some()).collect();
    assert_eq!(redirects.len(), 1);
    assert_eq!(redirects[0].pointer("/params/request/url").unwrap(), "https://seven.sliccy.ai");
    assert_eq!(redirects[0].pointer("/params/redirectResponse/headers/Location").unwrap(), "https://seven.sliccy.ai");
    let terminal: Vec<_> = events.iter().filter(|event| matches!(event["method"].as_str(), Some("Network.loadingFailed" | "Network.loadingFinished"))).collect();
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0]["params"]["errorText"], "net::ERR_BLOCKED_BY_CLIENT");
    assert_eq!(terminal[0]["params"]["blockedReason"], "inspector");
    let output = serde_json::to_string(events).unwrap();
    for secret in ["TERMINAL_FAKE_SECRET", "private?token", "#fragment", "postData", "urlFragment"] {
        assert!(!output.contains(secret), "{secret}: {output}");
    }
    for method in ["Network.getResponseBody", "Network.getRequestPostData", "Network.searchInResponseBody", "Network.streamResourceContent", "Audits.getEncodedResponse"] {
        assert!(cdp.call(method, json!({"requestId":"TERMINAL"}), Some("S")).unwrap_err().starts_with("slicc: refused:"));
    }
}

#[test]
fn redirect_terminal_fetch_network_alias_closes_seen_request_once() {
    let mut cdp = url_backend("about:blank");
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"NET","type":"Document","timestamp":1,"request":{"url":"https://ordinary.invalid/start","method":"GET","headers":{}}}}));
    cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"S","params":{"requestId":"FETCH","networkId":"NET","frameId":"FRAME","resourceType":"Document","request":{"url":"https://ordinary.invalid/start","method":"GET","headers":{}},"responseStatusCode":302,"responseHeaders":[{"name":"Location","value":"https://seven.sliccy.ai/private?token=FETCH_TERMINAL_SECRET"}]}}));
    cdp.absorb(json!({"method":"Network.loadingFailed","sessionId":"S","params":{"requestId":"NET","timestamp":2,"errorText":"net::ERR_BLOCKED_BY_CLIENT"}}));
    assert_eq!(cdp.events().iter().filter(|event| event["method"] == "Network.loadingFailed").count(), 1);
    assert!(!serde_json::to_string(cdp.events()).unwrap().contains("FETCH_TERMINAL_SECRET"));
    for id in ["NET", "FETCH"] {
        for method in ["Network.getResponseBody", "Network.getRequestPostData", "Fetch.getResponseBody", "Fetch.takeResponseBodyAsStream", "Network.getResponseBodyForInterception", "Network.takeResponseBodyForInterceptionAsStream", "Network.replayXHR", "Audits.getEncodedResponse"] {
            assert!(cdp.call(method, json!({"requestId":id,"interceptionId":id}), Some("S")).unwrap_err().starts_with("slicc: refused:"), "{method}/{id}");
        }
    }
}
