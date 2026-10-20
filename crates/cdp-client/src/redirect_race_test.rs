#[test]
fn response_body_is_rechecked_after_late_redirect_metadata() {
    let mut cdp = pending(|message| {
        vec![
            json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"R","statusCode":302,"headers":{"Location":redirect_url("seven")}}}),
            json!({"id":message["id"],"result":{"body":base64_encode(REDIRECT_MARKER.as_bytes()),"base64Encoded":true}}),
        ]
    });
    cdp.requests.insert("R".into(), false);
    assert!(cdp.call("Network.getResponseBody", json!({"requestId":"R"}), Some("S")).is_err());
}

#[test]
fn ordinary_source_reads_remain_readable() {
    let mut cdp = redirect_body_backend("ordinary source".into());
    cdp.absorb(json!({"method":"Runtime.executionContextCreated","sessionId":"S","params":{"context":{"id":1,"origin":"https://example.test","auxData":{"isDefault":true}}}}));
    cdp.absorb(json!({"method":"Debugger.scriptParsed","sessionId":"S","params":{"scriptId":"JS","url":"https://example.test/source","executionContextId":1}}));
    cdp.absorb(json!({"method":"CSS.styleSheetAdded","sessionId":"S","params":{"header":{"styleSheetId":"CSS","sourceURL":"https://example.test/source"}}}));
    for (method, params) in [
        ("CSS.getStyleSheetText", json!({"styleSheetId":"CSS"})),
        ("Debugger.getScriptSource", json!({"scriptId":"JS"})),
        ("Page.getResourceContent", json!({"frameId":"FRAME","url":"https://example.test/source"})),
    ] {
        let result = cdp.call(method, params, Some("S")).unwrap();
        assert_eq!(result["text"], "ordinary source");
    }
}

#[test]
fn multiply_encoded_nested_urls_and_json_slashes_are_redacted() {
    for host in ["seven", "vault"] {
        let url = redirect_url(host);
        let mut encoded = url.bytes().map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => char::from(byte).to_string(),
            _ => format!("%{byte:02X}"),
        }).collect::<String>();
        for _ in 0..4 {
            let input = format!("https://example.test/?next={encoded}&control=ordinary");
            let mut cdp = url_backend("about:blank");
            let result = cdp.after("Runtime.evaluate", &json!({}), Some("S"), json!({"text":input})).unwrap();
            let output = result["text"].as_str().unwrap();
            assert!(!output.contains(REDIRECT_MARKER), "{output}");
            assert!(output.contains("&control=ordinary"));
            encoded = encoded.replace('%', "%25");
        }
        let input = url.replace('/', "\\/");
        let mut cdp = url_backend("about:blank");
        let result = cdp.after("Runtime.evaluate", &json!({}), Some("S"), json!({"text":input})).unwrap();
        assert_eq!(result["text"], format!("https://{host}.sliccy.ai"));
    }
}
