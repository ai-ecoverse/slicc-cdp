#[test]
fn redirect_extra_info_pairing_and_ordinary_controls_remain_visible() {
    let url = redirect_url("seven");
    let ordinary_marker = "P0_ORDINARY_CONTROL_5D2B";
    let ordinary_url = format!("https://example.test/ordinary?control={ordinary_marker}");
    let mut cdp = redirect_body_backend(format!("ordinary body {ordinary_marker}"));
    cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"R","statusCode":302,"headers":{"Location":url,"X-Control":"ordinary"},"headersText":format!("HTTP/1.1 302\r\nLocation: {url}\r\nX-Control: ordinary\r\n")}}));
    assert!(cdp.events().is_empty());
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","request":{"url":"https://example.test/start"}}}));
    assert_eq!(cdp.events().len(), 2);
    assert_eq!(cdp.events()[1]["params"]["headers"]["Location"], "https://seven.sliccy.ai");
    assert_eq!(cdp.events()[1]["params"]["headers"]["X-Control"], "ordinary");
    assert!(cdp.call("Network.getResponseBody", json!({"requestId":"R"}), Some("S")).is_err());
    let headers = json!({"Location":ordinary_url,"Link":format!("<{ordinary_url}>; rel=preconnect"),"Refresh":format!("0;url={ordinary_url}")});
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"ORDINARY","request":{"url":ordinary_url},"redirectResponse":{"url":"https://example.test/start","status":302,"headers":headers}}}));
    let body = cdp.call("Network.getResponseBody", json!({"requestId":"ORDINARY"}), Some("S")).unwrap();
    assert!(body["body"].as_str().unwrap().contains(ordinary_marker));
    assert_eq!(cdp.events().last().unwrap()["params"]["redirectResponse"]["headers"], headers);
    let content = cdp.call("Page.getResourceContent", json!({"frameId":"F","url":ordinary_url}), Some("S")).unwrap();
    assert!(content["content"].as_str().unwrap().contains(ordinary_marker));
}

#[test]
fn redirect_resource_body_source_and_override_matrix() {
    let mut failures = Vec::new();
    for host in ["seven", "vault"] {
        for url in redirect_urls(host) {
            let mut cdp = redirect_body_backend(REDIRECT_MARKER.into());
            cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","request":{"url":"https://example.test/source"}}}));
            cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"R","statusCode":302,"headers":{"Location":url}}}));
            cdp.absorb(json!({"method":"Debugger.scriptParsed","sessionId":"S","params":{"scriptId":"JS","url":"https://example.test/source","executionContextId":1}}));
            cdp.absorb(json!({"method":"CSS.styleSheetAdded","sessionId":"S","params":{"header":{"styleSheetId":"CSS","sourceURL":"https://example.test/source"}}}));
            for (method, params) in [
                ("Network.replayXHR", json!({"requestId":"R"})),
                ("Page.getResourceContent", json!({"frameId":"FRAME","url":"https://example.test/source"})),
                ("Page.searchInResource", json!({"frameId":"FRAME","url":"https://example.test/source","query":"secret"})),
                ("CSS.getStyleSheetText", json!({"styleSheetId":"CSS"})),
                ("Debugger.getScriptSource", json!({"scriptId":"JS"})),
                ("Debugger.getWasmBytecode", json!({"scriptId":"JS"})),
                ("Debugger.searchInContent", json!({"scriptId":"JS","query":"secret"})),
                ("Page.captureSnapshot", json!({"format":"mhtml"})),
                ("Network.loadNetworkResource", json!({"frameId":"FRAME","url":"https://example.test/source","options":{"includeCredentials":true}})),
            ] {
                if let Ok(result) = cdp.call(method, params, Some("S")) {
                    failures.push(format!("ALLOWED {host} {method}: {result}"));
                }
            }
            for method in ["Fetch.continueRequest", "Fetch.continueResponse"] {
                cdp.hand("S", "F");
                cdp.requests.insert("F".into(), false);
                assert!(cdp.call(method, json!({"requestId":"F","url":url}), Some("S")).is_err(), "override allowed {method}");
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn redirect_error_page_and_nested_reply_strings_are_redacted() {
    let mut failures = Vec::new();
    for host in ["seven", "vault"] {
        for url in redirect_urls(host) {
            let encoded = url.bytes().map(|byte| format!("%{byte:02X}")).collect::<String>();
            for method in ["Page.getFrameTree", "Page.getResourceTree", "Page.getNavigationHistory", "DOMSnapshot.captureSnapshot", "Runtime.evaluate"] {
                let mut cdp = url_backend("about:blank");
                let reply = json!({"frameTree":{"frame":{"url":"chrome-error://chromewebdata/","unreachableUrl":url}},"frameResources":{"frame":{"unreachableUrl":url}},"entries":[{"url":url}],"strings":[format!("iframe failed: {url}"),format!("https://example.test/?next={encoded}")],"exceptionDetails":{"text":format!("failed {url}")}});
                let result = cdp.after(method, &json!({}), Some("S"), reply).unwrap();
                if marker_forms().iter().any(|marker| result.to_string().contains(marker)) {
                    failures.push(format!("LEAK {host} {method}: {result}"));
                }
            }
            for method in ["Page.frameNavigated", "Page.navigatedWithinDocument", "Page.frameStartedNavigating", "Page.frameRequestedNavigation"] {
                let mut cdp = url_backend("about:blank");
                cdp.absorb(json!({"method":method,"sessionId":"S","params":{"frame":{"id":"CHILD","parentId":"MAIN","url":"chrome-error://chromewebdata/","unreachableUrl":url},"url":"https://example.test/","unreachableUrl":url}}));
                if marker_forms().iter().any(|marker| format!("{:?}", cdp.events()).contains(marker)) {
                    failures.push(format!("LEAK {host} {method}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
