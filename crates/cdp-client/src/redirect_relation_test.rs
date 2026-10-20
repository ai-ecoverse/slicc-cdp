#[test]
fn canonical_handoff_relation_is_kept_only_as_an_exact_link_parameter() {
    let relation = "https://www.sliccy.ai/rel/upskill";
    for name in ["Link", "link", "LINK"] {
        for param in ["rel", "rev"] {
            let value = format!("<https://github.com/o/r>; {param}=\"{relation}\"; branch=main; path=\"skills/foo\"");
            let mut cdp = url_backend("about:blank");
            let reply = json!({"headers":{name:value},"responseHeaders":[{"name":name,"value":value}],"headersText":format!("HTTP/1.1 200\r\n{name}: {value}\r\n"),"requestHeadersText":format!("{name}: {value}\r\n")});
            let result = cdp.after("Runtime.evaluate", &json!({}), Some("S"), reply.clone()).unwrap();
            assert_eq!(result, reply);
        }
    }
    for value in [
        format!("<https://github.com/o/r>; rel=\"{relation}?x\""),
        format!("<https://github.com/o/r>; rel=\"{relation}#y\""),
        format!("<{relation}>; rel=other"),
        format!("<https://github.com/o/r>; rel=\"{}\"", redirect_url("seven")),
        "<https://github.com/o/r>; rel=\"https://WWW.sliccy.ai/rel/upskill\"".to_string(),
    ] {
        let mut cdp = url_backend("about:blank");
        let reply = json!({"headers":{"Link":value},"responseHeaders":[{"name":"Link","value":value}],"headersText":format!("Link: {value}\r\n")});
        let result = cdp.after("Runtime.evaluate", &json!({}), Some("S"), reply).unwrap();
        assert_ne!(result["headers"]["Link"], value);
        assert!(!result.to_string().contains(relation));
        assert!(!result.to_string().contains(REDIRECT_MARKER));
    }
    let mixed = "</api>; rel=\"api-catalog\", </llms.txt>; rel=\"https://llmstxt.org/rel/llms-txt\", <https://github.com/o/r>; rel=\"https://www.sliccy.ai/rel/upskill\"; branch=main; path=\"skills/foo\"";
    let mut cdp = url_backend("about:blank");
    cdp.requests.insert("R".into(), false);
    cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"R","headers":{"link":mixed},"headersText":format!("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\nlink: {mixed}\r\n\r\n")}}));
    cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"R","response":{"url":"http://127.0.0.1:8080/","status":200,"headers":{"link":mixed}}}}));
    assert_eq!(cdp.events().len(), 2, "{:?}", cdp.events());
    assert_eq!(cdp.events()[1]["params"]["response"]["headers"]["link"], mixed);
    assert_eq!(cdp.request_state("R"), Some(false));
    let mut cdp = url_backend("about:blank");
    let reply = json!({"headers":{"Location":relation,"Refresh":format!("0;url={relation}")},"text":relation});
    let result = cdp.after("Runtime.evaluate", &json!({}), Some("S"), reply).unwrap();
    assert_eq!(result["headers"]["Location"], "https://www.sliccy.ai");
    assert_eq!(result["text"], "https://www.sliccy.ai");
}
