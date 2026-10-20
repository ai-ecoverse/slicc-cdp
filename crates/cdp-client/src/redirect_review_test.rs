#[test]
fn review_redirect_chain_hides_scheme_less_secret_carriers() {
    let carriers = [
        json!({"method":"Network.requestWillBeSentExtraInfo","params":{"requestId":"R","headers":{},"requestHeadersText":format!("GET /auth/callback?code={REDIRECT_MARKER} HTTP/1.1\r\nHost: seven.sliccy.ai\r\n")}}),
        json!({"method":"Network.responseReceivedExtraInfo","params":{"requestId":"R","headers":{},"headersText":format!("HTTP/1.1 302\r\nLocation: /auth/callback?code={REDIRECT_MARKER}\r\nHost: seven.sliccy.ai\r\n")}}),
        json!({"method":"Network.requestWillBeSentExtraInfo","params":{"requestId":"R","headers":{":authority":"seven.sliccy.ai",":path":format!("/auth/callback?code={REDIRECT_MARKER}")}}}),
        json!({"method":"Network.responseReceivedExtraInfo","params":{"requestId":"R","headers":{"Host":"seven.sliccy.ai","X-Callback":format!("/auth/callback?code={REDIRECT_MARKER}")}}}),
        json!({"method":"Network.requestWillBeSent","params":{"requestId":"R","request":{"url":"https://example.test/next","urlFragment":format!("#access_token={REDIRECT_MARKER}"),"postData":format!("code={REDIRECT_MARKER}")}}}),
        json!({"method":"Fetch.requestPaused","params":{"requestId":"F","networkId":"R","resourceType":"XHR","request":{"url":"https://example.test/next","method":"POST","headers":{"Host":"seven.sliccy.ai"},"postData":format!("code={REDIRECT_MARKER}"),"urlFragment":format!("#access_token={REDIRECT_MARKER}")}}}),
    ];
    for mut event in carriers {
        let mut cdp = url_backend("about:blank");
        cdp.requests.insert("R".into(), false);
        cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"R","headers":{"Location":redirect_url("seven")},"statusCode":302}}));
        event["sessionId"] = json!("S");
        cdp.absorb(event.clone());
        assert!(!format!("{:?} {:?}", cdp.events(), cdp.paused).contains(REDIRECT_MARKER), "{event}: {:?} {:?}", cdp.events(), cdp.paused);
    }
    let mut cdp = url_backend("about:blank");
    cdp.protect_origins(&["http://client.example.test:8080"]);
    cdp.requests.insert("R".into(), false);
    cdp.absorb(json!({"method":"Network.requestWillBeSentExtraInfo","sessionId":"S","params":{"requestId":"R","headers":{":authority":"client.example.test:8080",":path":format!("/auth/callback?code={REDIRECT_MARKER}")}}}));
    assert!(cdp.events().is_empty());
}

#[test]
fn review_non_http_protected_schemes_cannot_leak() {
    for url in [
        format!("chrome://settings/passwords?secret={REDIRECT_MARKER}"),
        format!("chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/x?secret={REDIRECT_MARKER}"),
        format!("devtools://devtools/bundled/inspector.html?secret={REDIRECT_MARKER}"),
        format!("about:settings/passwords?secret={REDIRECT_MARKER}"),
        format!("view-source:https://seven.sliccy.ai/{REDIRECT_MARKER}"),
        format!("blob:https://seven.sliccy.ai/{REDIRECT_MARKER}"),
    ] {
        for (method, params) in [
            ("Runtime.consoleAPICalled", json!({"type":"log","executionContextId":1,"args":[{"type":"string","value":format!("message names {url}")}]})),
            ("Log.entryAdded", json!({"entry":{"text":format!("message names {url}"),"url":url}})),
        ] {
            let mut cdp = url_backend("about:blank");
            cdp.absorb(json!({"method":method,"sessionId":"S","params":params}));
            assert!(!format!("{:?}", cdp.events()).contains(REDIRECT_MARKER), "{method} {url}: {:?}", cdp.events());
        }
    }
    let mut cdp = url_backend("about:blank");
    cdp.protect_origins(&["ftp://client.example.test"]);
    cdp.absorb(json!({"method":"Runtime.consoleAPICalled","sessionId":"S","params":{"type":"log","executionContextId":1,"args":[{"type":"string","value":format!("message names ftp://client.example.test/{REDIRECT_MARKER}")}]}}));
    assert!(!format!("{:?}", cdp.events()).contains(REDIRECT_MARKER));
}

#[test]
fn review_nine_level_percent_nesting_fails_closed() {
    let mut encoded = redirect_url("seven").replace(':', "%3A").replace('/', "%2F").replace('?', "%3F").replace('=', "%3D").replace('&', "%26");
    for _ in 0..8 { encoded = encoded.replace('%', "%25"); }
    let mut cdp = url_backend("about:blank");
    let result = cdp.after("Runtime.evaluate", &json!({}), Some("S"), json!({"text":format!("https://example.test/?next={encoded}")})).unwrap();
    assert!(!result.to_string().contains(REDIRECT_MARKER), "{result}");
    assert!(result.to_string().contains("about"), "deep nesting must be replaced: {result}");
}
