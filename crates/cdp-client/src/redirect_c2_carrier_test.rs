fn c2_redirect_headers(url: &str) -> Value {
    json!({"Location":url,"Content-Location":url,"Refresh":format!("0;url={url}"),"Link":format!("<{url}>; rel=preconnect"),"X-Private":url,"X-Control":"ordinary","Set-Cookie":"C2_CARRIER_FAKE_SECRET=opaque"})
}

fn c2_assert_carriers(headers: &Value) {
    let expected = c2_redirect_headers("https://seven.sliccy.ai");
    for key in ["Location", "Content-Location", "Refresh", "Link"] {
        assert_eq!(headers[key], expected[key], "{key}");
    }
    assert_eq!(headers["X-Control"], "ordinary");
    assert_eq!(headers["X-Private"], "https://seven.sliccy.ai");
    assert!(headers.get("Set-Cookie").is_none());
}

#[test]
fn c2_origin_only_extra_info_survives_a_synthetic_terminal() {
    let mut cdp = url_backend("about:blank");
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"C2-NET","request":{"url":"https://ordinary.invalid/redirect","method":"GET","headers":{}}}}));
    let url = "https://seven.sliccy.ai/private?code=C2_CARRIER_FAKE_SECRET#private";
    cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"S","params":{"requestId":"C2-FETCH","networkId":"C2-NET","resourceType":"Document","request":{"url":"https://ordinary.invalid/redirect","method":"GET","headers":{}},"responseStatusCode":302,"responseHeaders":[{"name":"Location","value":url}]}}));
    assert_eq!(cdp.events().iter().filter(|event| event["method"] == "Network.loadingFailed").count(), 1);
    let mut text = "HTTP/1.1 302 Found\r\n".to_string();
    for (name, value) in c2_redirect_headers(url).as_object().unwrap() { text.push_str(&format!("{name}: {}\r\n", value.as_str().unwrap())); }
    cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"C2-NET","statusCode":302,"headers":c2_redirect_headers(url),"headersText":text,"blockedCookies":[{"cookie":{"value":"C2_CARRIER_FAKE_SECRET"}}]}}));
    let extra = cdp.events().iter().find(|event| event["method"] == "Network.responseReceivedExtraInfo").expect("late redacted extra info was withheld");
    c2_assert_carriers(&extra["params"]["headers"]);
    for name in ["Location", "Content-Location", "Refresh", "Link"] {
        let expected = c2_redirect_headers("https://seven.sliccy.ai");
        assert!(extra["params"]["headersText"].as_str().unwrap().contains(&format!("{name}: {}\r\n", expected[name].as_str().unwrap())));
    }
    assert!(!serde_json::to_string(cdp.events()).unwrap().contains("C2_CARRIER_FAKE_SECRET"));
    for method in ["Network.getResponseBody", "Network.getRequestPostData", "Network.streamResourceContent"] {
        assert!(cdp.call(method, json!({"requestId":"C2-NET"}), Some("S")).unwrap_err().starts_with("slicc: refused:"));
    }
}

#[test]
fn c2_every_redirect_response_carrier_retains_four_redacted_headers() {
    let url = "https://seven.sliccy.ai/private?code=C2_CARRIER_FAKE_SECRET#private";
    for (method, params) in [
        ("Network.requestWillBeSent", json!({"requestId":"R","request":{"url":url,"method":"GET","headers":{"Cookie":"C2_CARRIER_FAKE_SECRET"}},"redirectResponse":{"url":"https://ordinary.invalid/redirect","status":307,"headers":c2_redirect_headers(url)}})),
        ("Network.responseReceived", json!({"requestId":"R","response":{"url":"https://ordinary.invalid/redirect","status":303,"headers":c2_redirect_headers(url)}})),
        ("Fetch.requestPaused", json!({"requestId":"F","networkId":"R","resourceType":"Document","request":{"url":"https://ordinary.invalid/redirect","method":"GET","headers":{"Cookie":"C2_CARRIER_FAKE_SECRET"}},"responseStatusCode":308,"responseHeaders":c2_redirect_headers(url).as_object().unwrap().iter().map(|(name,value)| json!({"name":name,"value":value})).collect::<Vec<_>>()})),
    ] {
        let mut cdp = url_backend("about:blank");
        cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","request":{"url":"https://ordinary.invalid/redirect","method":"GET","headers":{}}}}));
        cdp.absorb(json!({"method":method,"sessionId":"S","params":params}));
        let event = cdp.events().iter().rev().find(|event| event["method"] == method).unwrap();
        let headers = match method {
            "Network.requestWillBeSent" => event["params"]["redirectResponse"]["headers"].clone(),
            "Network.responseReceived" => event["params"]["response"]["headers"].clone(),
            _ => Value::Object(event["params"]["responseHeaders"].as_array().unwrap().iter().map(|header| (header["name"].as_str().unwrap().to_string(), header["value"].clone())).collect()),
        };
        c2_assert_carriers(&headers);
        assert!(!serde_json::to_string(cdp.events()).unwrap().contains("C2_CARRIER_FAKE_SECRET"), "{method}");
    }
}

#[test]
fn c2_override_to_a_known_redirector_is_refused_without_blocking_plain_reads() {
    let mut cdp = redirect_body_backend("C2_ORDINARY_FAKE_CONTROL".into());
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.session_targets.insert("G".into(), "T".into());
    cdp.guard_sessions.insert("G".into());
    let redirector = "https://ordinary.invalid/redirect";
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"G","params":{"requestId":"GUARD-REDIRECT","request":{"url":redirector}}}));
    cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"G","params":{"requestId":"GUARD-REDIRECT","statusCode":302,"headers":{"Location":"https://seven.sliccy.ai/private?code=C2_OVERRIDE_FAKE_SECRET"}}}));
    cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"S","params":{"requestId":"HOLD","networkId":"PLAIN-NET","frameId":"FRAME","resourceType":"XHR","request":{"url":"https://ordinary.invalid/plain","method":"GET","headers":{}}}}));
    let overridden = cdp.call("Fetch.continueRequest", json!({"requestId":"HOLD","url":format!("{redirector}#another-fragment")}), Some("S"));
    assert!(overridden.unwrap_err().starts_with("slicc: refused:"));
    let body = cdp.call("Network.getResponseBody", json!({"requestId":"PLAIN-NET"}), Some("S")).unwrap();
    assert_eq!(body["body"], "C2_ORDINARY_FAKE_CONTROL");
    let resource = cdp.call("Page.getResourceContent", json!({"url":"https://ordinary.invalid/plain"}), Some("S")).unwrap();
    assert_eq!(resource["content"], "C2_ORDINARY_FAKE_CONTROL");
    assert!(cdp.call("Page.captureSnapshot", json!({"format":"mhtml"}), Some("S")).unwrap_err().starts_with("slicc: refused:"));
    cdp.call("Fetch.continueRequest", json!({"requestId":"HOLD","url":"https://ordinary.invalid/plain"}), Some("S")).unwrap();
}
