use crate::net::base64_encode;

const REDIRECT_MARKER: &str = "P0_REDIRECT_SECRET_7F9A";

fn redirect_url(host: &str) -> String {
    let path = if host == "seven" { "auth/callback" } else { "oauth/mcp-callback" };
    format!("https://{host}.sliccy.ai/{path}?code={REDIRECT_MARKER}&state={REDIRECT_MARKER}")
}

fn redirect_urls(host: &str) -> Vec<String> {
    marker_forms().into_iter().flat_map(|marker| {
        let base = format!("https://{host}.sliccy.ai");
        [format!("{base}/auth/{marker}/callback"), format!("{base}/auth/callback?code={marker}&state={marker}"), format!("{base}/auth/callback#access_token={marker}")]
    }).collect()
}

fn marker_forms() -> Vec<String> {
    vec![
        REDIRECT_MARKER.to_string(),
        REDIRECT_MARKER.bytes().map(|byte| format!("%{byte:02X}")).collect(),
        REDIRECT_MARKER.bytes().map(|byte| format!("\\u{byte:04x}")).collect(),
        base64_encode(REDIRECT_MARKER.as_bytes()),
    ]
}

fn redirect_rows(url: &str) -> Vec<(String, Value)> {
    let mut rows = Vec::new();
    for name in ["Location", "location", "lOcAtIoN", "Content-Location", "Link", "Refresh"] {
        let value = match name {
            "Link" => format!("<{url}>; rel=preconnect"),
            "Refresh" => format!("0;url={url}"),
            _ => url.to_string(),
        };
        for (method, params) in [
            ("Network.responseReceivedExtraInfo", json!({"requestId":"R","statusCode":302,"headers":{name:&value}})),
            ("Network.requestWillBeSentExtraInfo", json!({"requestId":"R","headers":{name:&value},"associatedCookies":[]})),
            ("Network.requestWillBeSent", json!({"requestId":"R","request":{"url":"https://example.test/next"},"redirectResponse":{"url":"https://example.test/start","status":302,"headers":{name:&value}}})),
            ("Fetch.requestPaused", json!({"requestId":"F","networkId":"R","frameId":"FRAME","resourceType":"Document","request":{"url":"https://example.test/start","method":"GET","headers":{}},"responseStatusCode":302,"responseHeaders":[{"name":name,"value":&value}]})),
        ] {
            rows.push((format!("{method}/{name}"), json!({"method":method,"sessionId":"S","params":params})));
        }
        for field in ["headersText", "requestHeadersText"] {
            rows.push((format!("Network.responseReceivedExtraInfo/{field}/{name}"), json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"R","statusCode":302,"headers":{},field:format!("HTTP/1.1 302\r\n{name}: {value}\r\n\r\n")}})));
        }
    }
    for (method, params) in [
        ("Page.frameNavigated", json!({"frame":{"id":"FRAME","url":url,"securityOrigin":"https://seven.sliccy.ai"}})),
        ("Page.frameRequestedNavigation", json!({"frameId":"FRAME","url":url,"reason":"httpHeaderRefresh","disposition":"currentTab"})),
        ("Log.entryAdded", json!({"entry":{"source":"network","level":"error","text":format!("Redirect to {url}"),"timestamp":1}})),
        ("Audits.issueAdded", json!({"issue":{"code":"GenericIssue","details":{"genericIssueDetails":{"errorType":"Redirect","request":{"url":url}}}}})),
        ("Runtime.consoleAPICalled", json!({"type":"log","executionContextId":1,"timestamp":1,"args":[{"type":"string","value":format!("Redirect to {url}")}]})),
    ] {
        rows.push((method.into(), json!({"method":method,"sessionId":"S","params":params})));
    }
    rows
}

#[test]
fn protected_redirect_messages_never_disclose_markers() {
    let mut failures = Vec::new();
    for host in ["seven", "vault"] {
        for url in redirect_urls(host) {
        for (name, event) in redirect_rows(&url) {
            let mut cdp = url_backend("about:blank");
            cdp.requests.insert("R".into(), false);
            cdp.absorb(event);
            let visible = format!("{:?} {:?}", cdp.events(), cdp.paused);
            if marker_forms().iter().any(|marker| visible.contains(marker)) {
                failures.push(format!("LEAK {host} {name}: {visible}"));
            } else {
                println!("NO-LEAK {host} {name}");
            }
        }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

struct RedirectBodyBackend {
    inner: UrlBackend,
    body: String,
}

impl Transport for RedirectBodyBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        let result = match message["method"].as_str().unwrap() {
            "Network.getResponseBody" | "Fetch.getResponseBody" | "Page.getResourceContent" | "Page.searchInResource" | "CSS.getStyleSheetText" | "Debugger.getScriptSource" => Some(json!({"body":self.body,"text":self.body,"content":self.body,"scriptSource":self.body,"base64Encoded":self.body == base64_encode(REDIRECT_MARKER.as_bytes())})),
            "Network.loadNetworkResource" => Some(json!({"resource":{"success":true,"httpStatusCode":200,"stream":"BODY"}})),
            "IO.read" => Some(json!({"data":self.body,"base64Encoded":false,"eof":true})),
            _ => None,
        };
        if let Some(result) = result {
            self.inner.inbox.push_back(json!({"id":message["id"],"result":result}).to_string());
            Ok(())
        } else {
            self.inner.send_text(text)
        }
    }

    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
        self.inner.recv_text(deadline)
    }
}

fn redirect_body_backend(body: String) -> Cdp {
    Cdp::from_transport(Box::new(RedirectBodyBackend {
        inner: UrlBackend { url: "about:blank".into(), inbox: VecDeque::new() }, body,
    }))
}

#[test]
fn protected_redirect_body_methods_are_refused() {
    let mut failures = Vec::new();
    for host in ["seven", "vault"] {
        for (name, event) in redirect_rows(&redirect_url(host)).into_iter().filter(|(name, _)| name.starts_with("Network.") || name.starts_with("Fetch.")) {
            for method in ["Network.getResponseBody", "Fetch.getResponseBody"] {
                for body in marker_forms() {
                    let mut cdp = redirect_body_backend(body);
                    cdp.requests.insert("R".into(), false);
                    cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"S","params":{"requestId":"F","networkId":"R","frameId":"FRAME","resourceType":"Document","request":{"url":"https://example.test/start","method":"GET","headers":{}}}}));
                    cdp.absorb(event.clone());
                    let id = if method.starts_with("Fetch.") { "F" } else { "R" };
                    if let Ok(result) = cdp.call(method, json!({"requestId":id}), Some("S")) {
                        failures.push(format!("LEAK {host} {name} {method}: {result}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn page_load_network_resource_cannot_return_redirected_protected_body() {
    let mut cdp = redirect_body_backend(REDIRECT_MARKER.into());
    let loaded = cdp.call("Network.loadNetworkResource", json!({"frameId":"FRAME","url":"https://example.test/start","options":{"includeCredentials":true,"disableCache":true}}), Some("S"));
    assert!(loaded.is_err(), "page loadNetworkResource allowed: {loaded:?}");
}
