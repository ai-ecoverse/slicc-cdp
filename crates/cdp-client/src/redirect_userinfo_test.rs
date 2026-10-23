const USERINFO_MARKER: &str = "USERINFO_PRIVATE_520";

fn userinfo_targets(host: &str) -> Vec<String> {
    ["%2F", "%2f", "%5C", "%5c", "%3F", "%3f", "%23", "%252F", "%252f", "%255C", "%255c", "%253F", "%253f", "%2523", "%25252F"].iter()
        .map(|code| format!("https://evil.test{code}@{host}/private?code={USERINFO_MARKER}#{USERINFO_MARKER}")).collect()
}

fn userinfo_carriers(url: &str) -> Vec<(&'static str, String)> {
    vec![("Refresh", format!("0;url={url}")), ("Location", url.to_string()), ("Content-Location", url.to_string()), ("Link", format!("<{url}>; rel=preload"))]
}

fn userinfo_response(cdp: &mut Cdp, method: &str, name: &str, value: &str, status: u64, source: &str) {
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","request":{"url":source,"headers":{}}}}));
    let params = match method {
        "Network.responseReceived" => json!({"requestId":"R","response":{"url":source,"status":status,"headers":{name:value}}}),
        "Network.responseReceivedExtraInfo" => json!({"requestId":"R","statusCode":status,"headers":{name:value},"headersText":format!("HTTP/1.1 {status}\r\n{name}: {value}\r\n\r\n")}),
        _ => json!({"requestId":"F","networkId":"R","resourceType":"Document","request":{"url":source,"headers":{},"method":"GET"},"responseStatusCode":status,"responseHeaders":[{"name":name,"value":value}]}),
    };
    cdp.absorb(json!({"method":method,"sessionId":"S","params":params}));
}

#[test]
fn percent_encoded_userinfo_carriers_refuse_protected_bodies() {
    let mut failures = Vec::new();
    for host in ["seven.sliccy.ai", "vault.sliccy.ai"] {
        for url in userinfo_targets(host) {
            for (name, value) in userinfo_carriers(&url) {
                for method in ["Network.responseReceived", "Network.responseReceivedExtraInfo", "Fetch.requestPaused"] {
                    let mut cdp = redirect_body_backend("PRIVATE_REDIRECT_BODY".into());
                    userinfo_response(&mut cdp, method, name, &value, 200, "https://example.test/start");
                    if !cdp.call("Network.getResponseBody", json!({"requestId":"R"}), Some("S")).is_err_and(|err| err.starts_with("slicc: refused:")) {
                        failures.push(format!("body {method} {name}: {value}"));
                    }
                    if method == "Fetch.requestPaused" && !cdp.call("Fetch.getResponseBody", json!({"requestId":"F"}), Some("S")).is_err_and(|err| err.starts_with("slicc: refused:")) {
                        failures.push(format!("fetch body {name}: {value}"));
                    }
                    if format!("{:?} {:?}", cdp.events(), cdp.paused).contains(USERINFO_MARKER) {
                        failures.push(format!("visible {method} {name}: {value}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn percent_encoded_userinfo_redirects_are_redacted_to_the_protected_origin() {
    let mut failures = Vec::new();
    for url in userinfo_targets("seven.sliccy.ai") {
        let headers = Value::Object(userinfo_carriers(&url).into_iter().map(|(name, value)| (name.to_string(), json!(value))).collect());
        for (method, params) in [
            ("Network.requestWillBeSent", json!({"requestId":"R","request":{"url":url,"method":"GET","headers":{}},"redirectResponse":{"url":"https://ordinary.invalid/redirect","status":302,"headers":headers}})),
            ("Network.responseReceived", json!({"requestId":"R","response":{"url":"https://ordinary.invalid/redirect","status":302,"headers":headers}})),
            ("Fetch.requestPaused", json!({"requestId":"F","networkId":"R","resourceType":"Document","request":{"url":"https://ordinary.invalid/redirect","method":"GET","headers":{}},"responseStatusCode":302,"responseHeaders":headers.as_object().unwrap().iter().map(|(name, value)| json!({"name":name,"value":value})).collect::<Vec<_>>()})),
        ] {
            let mut cdp = url_backend("about:blank");
            cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","request":{"url":"https://ordinary.invalid/redirect","method":"GET","headers":{}}}}));
            cdp.absorb(json!({"method":method,"sessionId":"S","params":params}));
            let Some(event) = cdp.events().iter().rev().find(|event| event["method"] == method) else {
                failures.push(format!("missing {method}: {url}"));
                continue;
            };
            let delivered = match method {
                "Network.requestWillBeSent" => event["params"]["redirectResponse"]["headers"].clone(),
                "Network.responseReceived" => event["params"]["response"]["headers"].clone(),
                _ => Value::Object(event["params"]["responseHeaders"].as_array().unwrap().iter().map(|header| (header["name"].as_str().unwrap().to_string(), header["value"].clone())).collect()),
            };
            let expected = json!({"Refresh":"0;url=https://seven.sliccy.ai","Location":"https://seven.sliccy.ai","Content-Location":"https://seven.sliccy.ai","Link":"<https://seven.sliccy.ai>; rel=preload"});
            if delivered != expected { failures.push(format!("headers {method} {url}: {delivered}")); }
            if format!("{:?} {:?}", cdp.events(), cdp.paused).contains(USERINFO_MARKER) { failures.push(format!("visible {method}: {url}")); }
            if !cdp.call("Network.getResponseBody", json!({"requestId":"R"}), Some("S")).is_err_and(|err| err.starts_with("slicc: refused:")) { failures.push(format!("body {method}: {url}")); }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn percent_encoded_userinfo_urls_are_redacted_in_event_text() {
    let mut failures = Vec::new();
    for url in userinfo_targets("seven.sliccy.ai") {
        let encoded: String = url.bytes().map(|byte| if byte.is_ascii_alphanumeric() || byte == b'.' { char::from(byte).to_string() } else { format!("%{byte:02X}") }).collect();
        for text in [format!("see {url} now"), format!("next=https://ordinary.invalid/?to={encoded}")] {
            let cdp = url_backend("about:blank");
            let (redacted, protected) = crate::shield::redact_text(&cdp.guard, &text);
            if !protected || redacted.contains(USERINFO_MARKER) { failures.push(format!("{text} -> {redacted}")); }
            let mut cdp = url_backend("about:blank");
            cdp.absorb(json!({"method":"Log.entryAdded","sessionId":"S","params":{"entry":{"source":"network","level":"info","text":text,"url":"https://ordinary.invalid/"}}}));
            if serde_json::to_string(cdp.events()).unwrap().contains(USERINFO_MARKER) { failures.push(format!("event {text}")); }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn ordinary_percent_encoded_userinfo_carriers_remain_readable() {
    let mut failures = Vec::new();
    for url in ["https://evil.test%2F@ordinary.invalid/end?control=visible", "https://evil.test%5c@ordinary.invalid/end", "https://ordinary.invalid/a%2Fb%3Fc%23d?next=%252F", "/relative%2F@seven.sliccy.ai/end?control=visible", "https://ordinary.invalid/?next=https%3A%2F%2Fexample.test%2F"] {
        for (name, value) in userinfo_carriers(url) {
            for method in ["Network.responseReceived", "Network.responseReceivedExtraInfo", "Fetch.requestPaused"] {
                let mut cdp = redirect_body_backend("ORDINARY_USERINFO_BODY".into());
                userinfo_response(&mut cdp, method, name, &value, 200, "https://ordinary.invalid/start");
                if cdp.call("Network.getResponseBody", json!({"requestId":"R"}), Some("S")).ok().is_none_or(|body| body["body"] != "ORDINARY_USERINFO_BODY") {
                    failures.push(format!("body {method} {name}: {value}"));
                }
                if method == "Network.responseReceived" {
                    cdp.absorb(json!({"method":"Network.loadingFinished","sessionId":"S","params":{"requestId":"R"}}));
                    if cdp.events().iter().any(|event| event["method"] == "Network.loadingFailed") { failures.push(format!("failed {name}: {value}")); }
                    if cdp.events().iter().find(|event| event["method"] == method).is_none_or(|event| event["params"]["response"]["headers"][name] != value.as_str()) {
                        failures.push(format!("header {name}: {value}"));
                    }
                }
            }
        }
        let mut cdp = url_backend("about:blank");
        userinfo_response(&mut cdp, "Fetch.requestPaused", "Location", url, 302, "https://ordinary.invalid/redirect");
        if cdp.paused.front().is_none_or(|paused| paused.request_id != "F") { failures.push(format!("redirect {url}")); }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn unresolved_userinfo_percent_encoding_fails_closed() {
    let nested = format!("https://ordinary.invalid/%{}2Fend", "25".repeat(12));
    for (name, value) in userinfo_carriers(&nested) {
        let mut cdp = redirect_body_backend("ORDINARY_USERINFO_BODY".into());
        userinfo_response(&mut cdp, "Network.responseReceived", name, &value, 200, "https://ordinary.invalid/start");
        assert!(cdp.call("Network.getResponseBody", json!({"requestId":"R"}), Some("S")).unwrap_err().starts_with("slicc: refused:"), "{name}: {value}");
    }
    let mut cdp = url_backend("about:blank");
    userinfo_response(&mut cdp, "Fetch.requestPaused", "Location", &nested, 302, "https://ordinary.invalid/redirect");
    assert!(cdp.paused.is_empty());
}
