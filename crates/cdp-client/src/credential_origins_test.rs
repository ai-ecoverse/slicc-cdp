const IDP: &str = "https://ims-na1.adobelogin.com";
const IDP_STAGE: &str = "https://ims-na1-stg1.adobelogin.com";
const ADOBE_AUTH: &str = "https://auth.services.adobe.com";
const ADOBE_ID: &str = "https://adobeid-na1.services.adobe.com";

fn credential_refusal() -> String {
    format!("{}\n", crate::protected::Protected::default().credential_refusal())
}

struct CookieJarBackend { inbox: VecDeque<String>, jar: Value }
impl Transport for CookieJarBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        let result = match message["method"].as_str().unwrap() {
            "Network.getAllCookies" | "Network.getCookies" | "Storage.getCookies" | "Page.getCookies" => json!({"cookies":self.jar}),
            _ => json!({}),
        };
        self.inbox.push_back(json!({"id":message["id"],"result":result}).to_string());
        Ok(())
    }
    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { Ok(self.inbox.pop_front()) }
}

fn credential_jar() -> Value {
    json!([
        {"name":"host","value":"IDP_HOST_SECRET","domain":"ims-na1.adobelogin.com","path":"/"},
        {"name":"dotted","value":"IDP_DOTTED_SECRET","domain":".ims-na1.adobelogin.com","path":"/"},
        {"name":"parent","value":"IDP_PARENT_SECRET","domain":".adobelogin.com","path":"/"},
        {"name":"stage","value":"IDP_STAGE_SECRET","domain":"IMS-NA1-STG1.ADOBELOGIN.COM.","path":"/"},
        {"name":"chip","value":"IDP_PARTITION_SECRET","domain":"widget.example.test","path":"/","partitionKey":{"topLevelSite":"https://adobelogin.com","hasCrossSiteAncestor":true}},
        {"name":"auth","value":"ADOBE_AUTH_KEPT","domain":"auth.services.adobe.com","path":"/"},
        {"name":"adobeid","value":"ADOBE_ID_KEPT","domain":".adobeid-na1.services.adobe.com","path":"/"},
        {"name":"adobe","value":"ADOBE_PARENT_KEPT","domain":".adobe.com","path":"/"},
        {"name":"subhost","value":"SUBHOST_KEPT","domain":"x.ims-na1.adobelogin.com","path":"/"},
        {"name":"lookalike","value":"LOOKALIKE_KEPT","domain":"evil-adobelogin.com","path":"/"},
        {"name":"suffix","value":"SUFFIX_KEPT","domain":"adobelogin.com.evil.test","path":"/"},
        {"name":"ordinary","value":"ORDINARY_KEPT","domain":".example.test","path":"/"},
        {"name":"partitioned","value":"PARTITION_KEPT","domain":"widget.example.test","path":"/","partitionKey":{"topLevelSite":"https://example.test","hasCrossSiteAncestor":true}}
    ])
}

fn jar_backend() -> Cdp {
    let mut cdp = Cdp::from_transport(Box::new(CookieJarBackend { inbox: VecDeque::new(), jar: credential_jar() }));
    cdp.fixture_session("S");
    cdp
}

#[test]
fn credential_cookie_readers_drop_idp_cookies_on_every_route_and_keep_ordinary_ones() {
    for (method, params) in [
        ("Network.getAllCookies", json!({})),
        ("Network.getCookies", json!({})),
        ("Network.getCookies", json!({"urls":[format!("{IDP}/ims/authorize"), "https://example.test/"]})),
        ("Storage.getCookies", json!({})),
        ("Page.getCookies", json!({})),
    ] {
        let mut cdp = jar_backend();
        let result = cdp.call(method, params.clone(), Some("S")).unwrap();
        let text = result.to_string();
        for secret in ["IDP_HOST_SECRET", "IDP_DOTTED_SECRET", "IDP_PARENT_SECRET", "IDP_STAGE_SECRET", "IDP_PARTITION_SECRET"] {
            assert!(!text.contains(secret), "{method} {params} leaked {secret}: {text}");
        }
        for kept in ["SUBHOST_KEPT", "LOOKALIKE_KEPT", "SUFFIX_KEPT", "ORDINARY_KEPT", "PARTITION_KEPT", "ADOBE_AUTH_KEPT", "ADOBE_ID_KEPT", "ADOBE_PARENT_KEPT"] {
            assert!(text.contains(kept), "{method} {params} dropped {kept}: {text}");
        }
    }
}

#[test]
fn credential_cookie_writes_are_refused_with_the_catalog_refusal() {
    let refusal = credential_refusal();
    for (method, params) in [
        ("Network.setCookie", json!({"name":"x","value":"y","url":format!("{IDP}/")})),
        ("Network.setCookie", json!({"name":"x","value":"y","url":"http://ims-na1.adobelogin.com:8443/"})),
        ("Network.setCookie", json!({"name":"x","value":"y","domain":".adobelogin.com","path":"/"})),
        ("Network.setCookie", json!({"name":"x","value":"y","domain":"IMS-NA1.ADOBELOGIN.COM.","path":"/"})),
        ("Network.setCookie", json!({"name":"x","value":"y","url":"https://example.test/","partitionKey":{"topLevelSite":"https://adobelogin.com","hasCrossSiteAncestor":true}})),
        ("Network.setCookies", json!({"cookies":[{"name":"a","value":"b","url":"https://example.test/"},{"name":"x","value":"y","domain":"ims-na1-stg1.adobelogin.com"}]})),
        ("Network.deleteCookies", json!({"name":"x","url":format!("{IDP}/")})),
        ("Network.deleteCookies", json!({"name":"x","url":"https://auth.adobelogin.com/"})),
        ("Network.deleteCookies", json!({"name":"x","domain":"adobelogin.com"})),
        ("Network.deleteCookies", json!({"name":"x","domain":"example.test","partitionKey":{"topLevelSite":"https://adobelogin.com","hasCrossSiteAncestor":true}})),
        ("Page.deleteCookie", json!({"cookieName":"x","url":format!("{IDP_STAGE}/")})),
        ("Storage.setCookies", json!({"cookies":[{"name":"x","value":"y","domain":".adobelogin.com","path":"/"}]})),
    ] {
        let result = url_backend("about:blank").call(method, params.clone(), Some("S"));
        assert_eq!(result.unwrap_err(), refusal, "{method} {params}");
    }
    for (method, params) in [
        ("Network.setCookie", json!({"name":"x","value":"y","url":"https://example.test/"})),
        ("Network.setCookie", json!({"name":"x","value":"y","url":"https://x.ims-na1.adobelogin.com/"})),
        ("Network.setCookie", json!({"name":"x","value":"y","url":"https://auth.adobelogin.com/"})),
        ("Network.setCookie", json!({"name":"x","value":"y","domain":"x.ims-na1.adobelogin.com","path":"/"})),
        ("Network.setCookie", json!({"name":"x","value":"y","domain":"evil-adobelogin.com","path":"/"})),
        ("Network.setCookie", json!({"name":"x","value":"y","domain":"adobelogin.com.evil.test","path":"/"})),
        ("Network.setCookie", json!({"name":"x","value":"y","domain":".adobe.com","path":"/"})),
        ("Network.setCookie", json!({"name":"x","value":"y","url":format!("{ADOBE_AUTH}/")})),
        ("Network.deleteCookies", json!({"name":"x","url":format!("{ADOBE_ID}/")})),
        ("Network.deleteCookies", json!({"name":"x","domain":"example.test","partitionKey":{"topLevelSite":"https://example.test","hasCrossSiteAncestor":true}})),
    ] {
        assert!(url_backend("about:blank").call(method, params.clone(), Some("S")).is_ok(), "{method} {params}");
    }
    for method in ["Network.clearBrowserCookies", "Storage.clearCookies"] {
        assert!(url_backend("about:blank").call(method, json!({}), Some("S")).unwrap_err().starts_with("slicc: refused:"), "{method}");
    }
}

fn idp_request(id: &str, url: &str) -> Value {
    json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":id,"type":"XHR","documentURL":"https://example.test/","request":{"url":url,"method":"GET","headers":{"Cookie":"sid=IDP_REQUEST_SECRET"}}}})
}

#[test]
fn credential_request_and_response_cookie_headers_are_stripped_from_events() {
    for url in [format!("{IDP}/ims/check"), "http://ims-na1-stg1.adobelogin.com:8080/x".to_string(), "https://auth.adobelogin.com/x".to_string(), "https://x.ims-na1.adobelogin.com/x".to_string(), "wss://ims-na1.adobelogin.com/socket".to_string()] {
        let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
        cdp.absorb(idp_request("R", &url));
        cdp.absorb(json!({"method":"Network.requestWillBeSentExtraInfo","sessionId":"S","params":{"requestId":"R","headers":{"cookie":"sid=IDP_EXTRA_SECRET","X-Control":"kept"},
            "associatedCookies":[{"blockedReasons":[],"cookie":{"name":"sid","value":"IDP_ASSOCIATED_SECRET","domain":".adobelogin.com","path":"/"}}]}}));
        cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"R","type":"XHR","response":{"url":url,"status":200,
            "headers":{"Set-Cookie":"sid=IDP_RESPONSE_SECRET; Domain=.adobelogin.com","Content-Type":"text/plain"},
            "headersText":"HTTP/1.1 200 OK\r\nSet-Cookie: sid=IDP_TEXT_SECRET\r\nContent-Type: text/plain\r\n",
            "requestHeaders":{"Cookie":"sid=IDP_REQUEST_HEADERS_SECRET"},"requestHeadersText":"GET / HTTP/1.1\r\nCookie: sid=IDP_REQUEST_TEXT_SECRET\r\n"}}}));
        cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"R","statusCode":200,
            "headers":{"set-cookie":"sid=IDP_EXTRA_RESPONSE_SECRET"},"headersText":"HTTP/1.1 200 OK\nset-cookie: sid=IDP_EXTRA_TEXT_SECRET\n",
            "blockedCookies":[{"blockedReasons":["SameSiteLax"],"cookieLine":"sid=IDP_BLOCKED_SECRET"}],
            "exemptedCookies":[{"exemptionReason":"None","cookieLine":"sid=IDP_EXEMPT_SECRET","cookie":{"name":"sid","value":"IDP_EXEMPT_SECRET","domain":"ims-na1.adobelogin.com","path":"/"}}]}}));
        cdp.absorb(json!({"method":"Network.webSocketWillSendHandshakeRequest","sessionId":"S","params":{"requestId":"R","request":{"headers":{"Cookie":"sid=IDP_SOCKET_SECRET"}}}}));
        cdp.absorb(json!({"method":"Network.loadingFinished","sessionId":"S","params":{"requestId":"R","timestamp":1.0}}));
        let text = serde_json::to_string(cdp.events()).unwrap();
        assert!(!text.contains("_SECRET"), "{url}: {text}");
        for method in ["Network.requestWillBeSent", "Network.requestWillBeSentExtraInfo", "Network.responseReceived", "Network.responseReceivedExtraInfo", "Network.loadingFinished"] {
            assert_eq!(cdp.events().iter().filter(|event| event["method"] == method).count(), 1, "{url} {method}: {text}");
        }
        assert!(text.contains("\"X-Control\":\"kept\""), "{text}");
        assert!(text.contains(&url.replace('/', "\\/")) || text.contains(&url), "{url} must stay visible: {text}");
    }
}

#[test]
fn credential_redirect_hops_and_fetch_pauses_lose_their_cookie_headers() {
    let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","type":"Document","documentURL":"https://example.test/callback",
        "request":{"url":"https://example.test/callback","method":"GET","headers":{}},
        "redirectResponse":{"url":format!("{IDP}/ims/authorize"),"status":302,"headers":{"Set-Cookie":"sid=IDP_REDIRECT_SECRET; Domain=.adobelogin.com","Location":"https://example.test/callback"}}}}));
    cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"S","params":{"requestId":"P","resourceType":"XHR","request":{"url":format!("{IDP}/ims/check"),"method":"GET","headers":{"Cookie":"sid=IDP_PAUSED_SECRET","Accept":"*/*"}},
        "responseStatusCode":200,"responseHeaders":[{"name":"Set-Cookie","value":"sid=IDP_PAUSED_RESPONSE_SECRET"},{"name":"Content-Type","value":"text/plain"}]}}));
    let text = serde_json::to_string(cdp.events()).unwrap();
    assert!(!text.contains("_SECRET") && text.contains("Location"), "{text}");
    let paused = cdp.next_paused(Some("S"), Duration::ZERO).unwrap().expect("the IdP request stays drivable");
    assert_eq!(paused.url, format!("{IDP}/ims/check"));
    assert_eq!(paused.headers, vec![("Accept".to_string(), "*/*".to_string())]);
    assert_eq!(paused.response_headers, Some(vec![("Content-Type".to_string(), "text/plain".to_string())]));
    cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"S","params":{"requestId":"Q","resourceType":"XHR","request":{"url":"https://shop.example.test/api","method":"GET","headers":{"Accept":"*/*"}},
        "responseStatusCode":200,"responseHeaders":[{"name":"Set-Cookie","value":"sid=ORDINARY_PAUSED_KEPT"}]}}));
    let paused = cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap();
    assert_eq!(paused.response_headers, Some(vec![("Set-Cookie".to_string(), "sid=ORDINARY_PAUSED_KEPT".to_string())]));
    let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"O","type":"XHR","documentURL":"https://example.test/","request":{"url":"https://shop.example.test/api","method":"GET","headers":{"Cookie":"sid=ORDINARY_REQUEST_KEPT"}}}}));
    cdp.absorb(json!({"method":"Network.requestWillBeSentExtraInfo","sessionId":"S","params":{"requestId":"O","headers":{"Cookie":"sid=ORDINARY_EXTRA_KEPT"},"associatedCookies":[{"blockedReasons":[],"cookie":{"name":"sid","value":"ORDINARY_EXTRA_KEPT","domain":"shop.example.test","path":"/"}}]}}));
    cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"O","type":"XHR","response":{"url":"https://shop.example.test/api","status":200,"headers":{"Set-Cookie":"sid=ORDINARY_RESPONSE_KEPT"}}}}));
    let text = serde_json::to_string(cdp.events()).unwrap();
    for kept in ["ORDINARY_REQUEST_KEPT", "ORDINARY_EXTRA_KEPT", "ORDINARY_RESPONSE_KEPT"] {
        assert!(text.contains(kept), "{kept}: {text}");
    }
    assert!(!text.contains("[redacted]"), "{text}");
    for url in [format!("{ADOBE_AUTH}/signin"), format!("{ADOBE_ID}/ims"), "https://www.adobe.com/".to_string()] {
        let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
        cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"A","type":"XHR","documentURL":url,"request":{"url":url,"method":"GET","headers":{}}}}));
        cdp.absorb(json!({"method":"Network.requestWillBeSentExtraInfo","sessionId":"S","params":{"requestId":"A","headers":{"Cookie":"adobe=ADOBE_COOKIE_KEPT"},"associatedCookies":[{"blockedReasons":[],"cookie":{"name":"adobe","value":"ADOBE_COOKIE_KEPT","domain":".adobe.com","path":"/"}}]}}));
        cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"A","type":"XHR","response":{"url":url,"status":200,"headers":{"Set-Cookie":"adobe=ADOBE_SET_KEPT; Domain=.adobe.com"}}}}));
        let text = serde_json::to_string(cdp.events()).unwrap();
        assert!(text.contains("ADOBE_SET_KEPT") && text.matches("ADOBE_COOKIE_KEPT").count() == 2 && !text.contains("[redacted]"), "{url}: {text}");
    }
}

#[test]
fn partitioned_cookie_headers_under_an_idp_top_level_site_are_stripped() {
    let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"O","type":"XHR","documentURL":format!("{IDP}/"),"request":{"url":"https://widget.example.test/frame","method":"GET","headers":{}}}}));
    cdp.absorb(json!({"method":"Network.requestWillBeSentExtraInfo","sessionId":"S","params":{"requestId":"O","headers":{"Cookie":"chip=IDP_PARTITION_SECRET"},
        "associatedCookies":[{"blockedReasons":[],"cookie":{"name":"chip","value":"IDP_PARTITION_SECRET","domain":"widget.example.test","path":"/","partitionKey":{"topLevelSite":"https://adobelogin.com","hasCrossSiteAncestor":true}}}]}}));
    cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"O","statusCode":200,"headers":{"Set-Cookie":"chip=IDP_PARTITION_RESPONSE_SECRET; Partitioned"},"cookiePartitionKey":{"topLevelSite":"https://adobelogin.com","hasCrossSiteAncestor":true}}}));
    let text = serde_json::to_string(cdp.events()).unwrap();
    assert!(!text.contains("_SECRET"), "{text}");
    assert_eq!(cdp.events().len(), 3, "{text}");
}

#[test]
fn credential_storage_commands_are_refused_and_ordinary_storage_stays_readable() {
    let refusal = credential_refusal();
    let key = format!("{IDP}/");
    for (method, params) in [
        ("DOMStorage.getDOMStorageItems", json!({"storageId":{"securityOrigin":IDP,"isLocalStorage":true}})),
        ("DOMStorage.getDOMStorageItems", json!({"storageId":{"storageKey":key,"isLocalStorage":false}})),
        ("DOMStorage.getDOMStorageItems", json!({"storageId":{"storageKey":"https://widget.example.test/^0https://adobelogin.com","isLocalStorage":true}})),
        ("IndexedDB.requestDatabaseNames", json!({"storageKey":"https://widget.example.test/^0https://ims-na1.adobelogin.com^31"})),
        ("DOMStorage.setDOMStorageItem", json!({"storageId":{"securityOrigin":"https://IMS-NA1.ADOBELOGIN.COM.:8443","isLocalStorage":true},"key":"k","value":"v"})),
        ("DOMStorage.clear", json!({"storageId":{"securityOrigin":"http://ims-na1-stg1.adobelogin.com","isLocalStorage":true}})),
        ("IndexedDB.requestDatabaseNames", json!({"securityOrigin":IDP})),
        ("IndexedDB.requestDatabaseNames", json!({"storageKey":format!("{IDP_STAGE}/")})),
        ("IndexedDB.requestData", json!({"storageKey":key,"databaseName":"d","objectStoreName":"o","indexName":"","skipCount":0,"pageSize":10})),
        ("CacheStorage.requestCacheNames", json!({"securityOrigin":IDP})),
        ("CacheStorage.requestEntries", json!({"cacheId":format!("{IDP}/|cache"),"skipCount":0,"pageSize":10})),
        ("Storage.getUsageAndQuota", json!({"origin":IDP})),
        ("Storage.clearDataForOrigin", json!({"origin":IDP,"storageTypes":"cookies,local_storage,indexeddb,cache_storage"})),
        ("Storage.clearDataForOrigin", json!({"origin":IDP,"storageTypes":"all"})),
        ("Storage.clearDataForStorageKey", json!({"storageKey":key,"storageTypes":"all"})),
        ("Storage.getSharedStorageEntries", json!({"ownerOrigin":IDP})),
        ("Storage.getSharedStorageMetadata", json!({"ownerOrigin":IDP})),
        ("Storage.getInterestGroupDetails", json!({"ownerOrigin":IDP,"name":"g"})),
        ("Storage.trackCacheStorageForOrigin", json!({"origin":IDP})),
        ("Storage.trackIndexedDBForStorageKey", json!({"storageKey":key})),
        ("FileSystem.getDirectory", json!({"bucketFileSystemLocator":{"storageKey":key,"pathComponents":[]}})),
    ] {
        let result = url_backend("about:blank").call(method, params.clone(), Some("S"));
        assert_eq!(result.unwrap_err(), refusal, "{method} {params}");
    }
    for (method, params) in [
        ("Storage.getTrustTokens", json!({})),
        ("Storage.getAffectedUrlsForThirdPartyCookieMetadata", json!({"firstPartyUrl":format!("{IDP}/"),"thirdPartyUrls":[]})),
    ] {
        assert!(url_backend("about:blank").call(method, params.clone(), Some("S")).unwrap_err().starts_with("slicc: refused:"), "{method}");
    }
    for (method, params) in [
        ("DOMStorage.getDOMStorageItems", json!({"storageId":{"securityOrigin":"https://example.test","isLocalStorage":true}})),
        ("DOMStorage.getDOMStorageItems", json!({"storageId":{"securityOrigin":"https://x.ims-na1.adobelogin.com","isLocalStorage":true}})),
        ("IndexedDB.requestDatabaseNames", json!({"storageKey":"https://evil-adobelogin.com/"})),
        ("IndexedDB.requestDatabaseNames", json!({"storageKey":"https://adobelogin.com.evil.test/"})),
        ("CacheStorage.requestCacheNames", json!({"securityOrigin":"https://example.test"})),
        ("IndexedDB.requestDatabaseNames", json!({"storageKey":"https://widget.example.test/^0https://adobe.com"})),
        ("DOMStorage.getDOMStorageItems", json!({"storageId":{"securityOrigin":ADOBE_AUTH,"isLocalStorage":true}})),
        ("IndexedDB.requestDatabaseNames", json!({"storageKey":format!("{ADOBE_ID}/")})),
        ("Storage.getUsageAndQuota", json!({"origin":"https://example.test"})),
    ] {
        assert!(url_backend("about:blank").call(method, params.clone(), Some("S")).is_ok(), "{method} {params}");
    }
}

#[test]
fn credential_clear_data_for_origin_is_refused_and_never_reaches_chrome() {
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Recording {
        inbox: VecDeque<String>,
        sent: Rc<RefCell<Vec<String>>>,
    }

    impl Transport for Recording {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            let message: Value = serde_json::from_str(text).unwrap();
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                self.sent.borrow_mut().push(method.to_string());
            }
            self.inbox
                .push_back(json!({"id": message["id"], "result": {}}).to_string());
            Ok(())
        }

        fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
            Ok(self.inbox.pop_front())
        }
    }

    let sent = Rc::new(RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(Recording {
        inbox: VecDeque::new(),
        sent: Rc::clone(&sent),
    }));
    cdp.fixture_session("S");
    let refusal = credential_refusal();
    for params in [
        json!({"origin": IDP, "storageTypes": "cookies,local_storage,indexeddb,cache_storage"}),
        json!({"origin": IDP, "storageTypes": "all"}),
        json!({"origin": IDP_STAGE, "storageTypes": "all"}),
    ] {
        sent.borrow_mut().clear();
        assert_eq!(
            cdp.call("Storage.clearDataForOrigin", params.clone(), Some("S"))
                .unwrap_err(),
            refusal,
            "{params}"
        );
        assert!(
            !sent
                .borrow()
                .iter()
                .any(|method| method == "Storage.clearDataForOrigin"),
            "clearDataForOrigin must not reach Chrome for {params}: {:?}",
            sent.borrow()
        );
    }
}

#[test]
fn storage_events_naming_a_credential_origin_are_dropped() {
    let key = format!("{IDP}/");
    for event in [
        json!({"method":"DOMStorage.domStorageItemAdded","sessionId":"S","params":{"storageId":{"securityOrigin":IDP,"isLocalStorage":true},"key":"token","newValue":"IDP_STORAGE_SECRET"}}),
        json!({"method":"DOMStorage.domStorageItemUpdated","sessionId":"S","params":{"storageId":{"storageKey":key,"isLocalStorage":true},"key":"token","oldValue":"a","newValue":"IDP_STORAGE_SECRET"}}),
        json!({"method":"Storage.cacheStorageContentUpdated","params":{"origin":IDP,"storageKey":key,"bucketId":"b","cacheName":"IDP_STORAGE_SECRET"}}),
        json!({"method":"Storage.indexedDBContentUpdated","params":{"origin":IDP_STAGE,"storageKey":format!("{IDP_STAGE}/"),"bucketId":"b","databaseName":"IDP_STORAGE_SECRET","objectStoreName":"o"}}),
        json!({"method":"Storage.sharedStorageAccessed","params":{"accessTime":1,"type":"documentSet","mainFrameId":"F","ownerOrigin":IDP,"params":{"key":"IDP_STORAGE_SECRET"}}}),
        json!({"method":"Storage.indexedDBContentUpdated","params":{"origin":"https://widget.example.test","storageKey":"https://widget.example.test/^0https://adobelogin.com","bucketId":"b","databaseName":"IDP_STORAGE_SECRET","objectStoreName":"o"}}),
    ] {
        let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
        cdp.absorb(event.clone());
        assert!(!serde_json::to_string(cdp.events()).unwrap().contains("IDP_STORAGE_SECRET"), "{event}");
    }
    let ordinary = json!({"method":"Storage.cacheStorageContentUpdated","params":{"origin":"https://example.test","storageKey":"https://example.test/","bucketId":"b","cacheName":"ORDINARY_KEPT"}});
    let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
    cdp.absorb(ordinary.clone());
    assert_eq!(cdp.events(), &[ordinary]);
}

#[test]
fn audit_cookie_issues_naming_protected_or_credential_origins_are_dropped() {
    let issue = |details: Value| json!({"method":"Audits.issueAdded","sessionId":"S","params":{"issue":{"code":"CookieIssue","details":{"cookieIssueDetails":details}}}});
    for details in [
        json!({"request":{"requestId":"R","url":format!("{IDP}/ims/check")},"rawCookieLine":"sid=ISSUE_SECRET","cookieWarningReasons":[],"cookieExclusionReasons":[],"operation":"SetCookie"}),
        json!({"request":{"requestId":"R","url":"https://seven.sliccy.ai/api"},"rawCookieLine":"sid=ISSUE_SECRET","operation":"SetCookie"}),
        json!({"cookie":{"name":"sid","domain":".adobelogin.com","path":"/"},"rawCookieLine":"sid=ISSUE_SECRET","operation":"SetCookie"}),
        json!({"cookie":{"name":"sid","domain":"sliccy.ai","path":"/"},"rawCookieLine":"sid=ISSUE_SECRET","operation":"SetCookie"}),
        json!({"cookieUrl":format!("{IDP_STAGE}/"),"rawCookieLine":"sid=ISSUE_SECRET","operation":"SetCookie"}),
        json!({"cookieUrl":"https://vault.sliccy.ai/","rawCookieLine":"sid=ISSUE_SECRET","operation":"SetCookie"}),
        json!({"rawCookieLine":"sid=ISSUE_SECRET; Domain=.adobelogin.com","operation":"SetCookie"}),
        json!({"rawCookieLine":"sid=ISSUE_SECRET; Domain=sliccy.ai","operation":"SetCookie"}),
        json!({"rawCookieLine":"sid=ISSUE_SECRET","operation":"SetCookie"}),
        json!({"cookieUrl":"not a url","rawCookieLine":"sid=ISSUE_SECRET","operation":"SetCookie"}),
    ] {
        let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
        cdp.absorb(issue(details.clone()));
        assert!(cdp.events().is_empty(), "{details}: {:?}", cdp.events());
    }
    let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
    cdp.absorb(json!({"method":"Audits.issueAdded","sessionId":"S","params":{"issue":{"code":"DeprecationIssue","details":{"deprecationIssueDetails":{"sourceCodeLocation":{"url":"https://seven.sliccy.ai/app.js","lineNumber":1,"columnNumber":1},"type":"X"}}}}}));
    assert!(cdp.events().is_empty(), "{:?}", cdp.events());
    for details in [
        json!({"request":{"requestId":"R","url":"https://shop.example.test/api"},"rawCookieLine":"sid=ORDINARY_ISSUE","operation":"SetCookie"}),
        json!({"cookie":{"name":"sid","domain":"x.ims-na1.adobelogin.com","path":"/"},"operation":"ReadCookie"}),
        json!({"cookieUrl":"https://example.test/","rawCookieLine":"sid=ORDINARY_ISSUE","operation":"SetCookie"}),
        json!({"rawCookieLine":"sid=ORDINARY_ISSUE; Domain=example.test","operation":"SetCookie"}),
        json!({"rawCookieLine":"sid=ORDINARY_ISSUE; Domain=.adobe.com","operation":"SetCookie"}),
        json!({"cookieUrl":format!("{ADOBE_AUTH}/"),"rawCookieLine":"sid=ORDINARY_ISSUE","operation":"SetCookie"}),
    ] {
        let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
        cdp.absorb(issue(details.clone()));
        assert_eq!(cdp.events().len(), 1, "{details}");
    }
}

fn idp_page(url: &str) -> Cdp {
    let mut cdp = url_backend(url);
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":url,"title":"Sign in"}}}));
    let origin = url::Url::parse(url).unwrap().origin().ascii_serialization();
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":url,"securityOrigin":origin}}}));
    cdp
}

#[test]
fn credential_origin_pages_stay_drivable() {
    for url in ["https://example.test/login".to_string(), format!("{IDP}/ims/authorize/v2?client_id=public&state=s"), format!("{IDP_STAGE}/ims/authorize/v2"), "https://x.ims-na1.adobelogin.com/login".to_string(), format!("{ADOBE_AUTH}/en_US/index.html?callback=c"), format!("{ADOBE_ID}/ims")] {
        let mut cdp = idp_page(&url);
        assert!(!cdp.target_protected("T", false).unwrap(), "{url}");
        assert!(!cdp.session_blocked("S"), "{url}");
        let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
        assert_eq!(listed["targetInfos"][0]["url"], json!(url));
        let found = cdp.call("Target.getTargetInfo", json!({"targetId":"T"}), None).unwrap();
        assert_eq!(found["targetInfo"]["url"], json!(url));
        observe_document(&mut cdp);
        for (method, params) in [
            ("Runtime.evaluate", json!({"expression":"document.querySelector('#username').value = 'user'"})),
            ("DOM.getDocument", json!({})),
            ("Input.dispatchMouseEvent", json!({"type":"mousePressed","x":5,"y":5,"button":"left","clickCount":1})),
            ("Input.insertText", json!({"text":"user@example.test"})),
            ("Input.dispatchKeyEvent", json!({"type":"keyDown","key":"Enter"})),
            ("Page.captureScreenshot", json!({})),
            ("Page.reload", json!({})),
            ("Page.navigate", json!({"url":url})),
        ] {
            let result = cdp.call(method, params.clone(), Some("S"));
            assert!(result.is_ok(), "{url} {method}: {result:?}");
        }
        assert!(cdp.call("Target.createTarget", json!({"url":url}), None).is_ok(), "{url}");
        assert!(cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).is_ok(), "{url}");
        let mut seen = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
        seen.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"D","type":"Document","documentURL":url,"request":{"url":url,"method":"GET","headers":{}}}}));
        assert_eq!(seen.events().len(), 1, "{url}");
        assert_eq!(seen.events()[0]["params"]["request"]["url"], json!(url));
    }
    let guard = crate::protected::Protected::default();
    for url in [format!("{IDP}/ims/authorize?code=c"), format!("{IDP_STAGE}/"), format!("{ADOBE_AUTH}/"), format!("{ADOBE_ID}/")] {
        assert!(!guard.url(&url) && !guard.url_read_only(&url), "{url}");
        assert!(!guard.target(&url, false, crate::protected::Opener::None), "{url}");
        assert!(!guard.pattern(&format!("{url}*")), "{url}");
        assert!(!guard.redaction_host("ims-na1.adobelogin.com"));
        assert_eq!(crate::shield::redact_text(&guard, &url).0, url);
    }
}

fn ordinary_request(cdp: &mut Cdp, id: &str, url: &str) {
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":id,"type":"XHR","documentURL":"https://shop.example.test/","request":{"url":url,"method":"GET","headers":{}}}}));
}

fn line_entry(list: &str, line: &str) -> Value {
    match list {
        "associatedCookies" => json!({"blockedReasons":[],"cookieLine":line}),
        "blockedCookies" => json!({"blockedReasons":["SameSiteLax"],"cookieLine":line}),
        _ => json!({"exemptionReason":"None","cookieLine":line}),
    }
}

fn extra_lists(method: &str, list: &str, entries: Vec<Value>) -> Value {
    let mut params = json!({"requestId":"R","headers":{}});
    if method == "Network.responseReceivedExtraInfo" { params["statusCode"] = json!(200); }
    params[list] = Value::Array(entries);
    json!({"method":method,"sessionId":"S","params":params})
}

fn kept_lines() -> Vec<&'static str> {
    vec![
        "adobe=ADOBE_LINE_KEPT; Domain=.adobe.com; Path=/",
        "sid=ORDINARY_LINE_KEPT; Domain=shop.example.test",
        "plain=PLAIN_LINE_KEPT; Path=/",
        "quoted=QUOTED_LINE_KEPT; Domain=\"example.test\"",
        "repeated=REPEATED_LINE_KEPT; Domain=example.test; domain=shop.example.test",
    ]
}

#[test]
fn cookie_lines_in_every_extra_info_list_are_dropped_when_their_domain_covers_a_credential_or_protected_host() {
    let hidden = [
        "id=LINE_SECRET; Domain=.adobelogin.com; Path=/; Secure",
        "id=LINE_SECRET; Domain=\".adobelogin.com\"",
        "id=LINE_SECRET; DOMAIN=.ADOBELOGIN.COM",
        "id=LINE_SECRET; Domain=adobelogin.com.",
        "id=LINE_SECRET; Domain = ims-na1.adobelogin.com",
        "id=LINE_SECRET; Domain=example.test; Domain=.adobelogin.com",
        "id=LINE_SECRET; Domain=.adobelogin.com; Domain=example.test",
        "id=LINE_SECRET; Domain=sliccy.ai",
        "LINE_SECRET; Domain=example.test",
        "=LINE_SECRET; Domain=example.test",
        "id=LINE_SECRET\nother; Domain=example.test",
    ];
    for method in ["Network.requestWillBeSentExtraInfo", "Network.responseReceivedExtraInfo"] {
        for list in ["associatedCookies", "blockedCookies", "exemptedCookies"] {
            for line in hidden {
                let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
                ordinary_request(&mut cdp, "R", "https://shop.example.test/api");
                let mut entries: Vec<Value> = kept_lines().into_iter().map(|kept| line_entry(list, kept)).collect();
                entries.push(line_entry(list, line));
                cdp.absorb(extra_lists(method, list, entries));
                let text = serde_json::to_string(cdp.events()).unwrap();
                assert!(!text.contains("LINE_SECRET"), "{method} {list} {line:?}: {text}");
                let delivered = cdp.events().iter().find(|event| event["method"] == method).unwrap_or_else(|| panic!("{method} {list} {line:?}: {text}"));
                assert_eq!(delivered["params"][list].as_array().unwrap().len(), kept_lines().len(), "{method} {list} {line:?}: {text}");
            }
        }
    }
}

#[test]
fn cookie_objects_and_malformed_entries_in_every_extra_info_list_fail_closed() {
    for list in ["associatedCookies", "blockedCookies", "exemptedCookies"] {
        for entry in [
            json!({"blockedReasons":[],"cookie":{"name":"id","value":"ENTRY_SECRET","domain":".adobelogin.com","path":"/"}}),
            json!({"blockedReasons":[],"cookie":{"name":"id","value":"ENTRY_SECRET","domain":"seven.sliccy.ai","path":"/"}}),
            json!({"blockedReasons":[],"cookie":{"name":"id","value":"ENTRY_SECRET","domain":"widget.example.test","path":"/","partitionKey":{"topLevelSite":"https://adobelogin.com","hasCrossSiteAncestor":true}}}),
            json!({"blockedReasons":[],"cookie":{"name":"id","value":"ENTRY_SECRET","path":"/"}}),
            json!({"blockedReasons":[],"cookie":{"name":"id","value":"ENTRY_SECRET","domain":"shop.example.test"},"cookieLine":"id=ENTRY_SECRET; Domain=.adobelogin.com"}),
            json!({"blockedReasons":[],"cookieLine":["id=ENTRY_SECRET"]}),
            json!({"blockedReasons":[],"note":"ENTRY_SECRET"}),
        ] {
            let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
            ordinary_request(&mut cdp, "R", "https://shop.example.test/api");
            let kept = json!({"blockedReasons":[],"cookie":{"name":"sid","value":"ORDINARY_OBJECT_KEPT","domain":"shop.example.test","path":"/"}});
            cdp.absorb(extra_lists("Network.responseReceivedExtraInfo", list, vec![kept, entry.clone()]));
            let text = serde_json::to_string(cdp.events()).unwrap();
            assert!(!text.contains("ENTRY_SECRET") && text.contains("ORDINARY_OBJECT_KEPT"), "{list} {entry}: {text}");
        }
    }
}

#[test]
fn a_cookie_line_without_domain_is_judged_by_the_request_host() {
    for (url, kept) in [("https://shop.example.test/api", true), ("https://ims-na1.adobelogin.com/ims/check", false), ("https://seven.sliccy.ai/x", false)] {
        for list in ["associatedCookies", "blockedCookies", "exemptedCookies"] {
            let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
            ordinary_request(&mut cdp, "R", url);
            cdp.absorb(extra_lists("Network.responseReceivedExtraInfo", list, vec![line_entry(list, "host=HOST_ONLY_VALUE; Path=/")]));
            let text = serde_json::to_string(cdp.events()).unwrap();
            assert_eq!(text.contains("HOST_ONLY_VALUE"), kept, "{url} {list}: {text}");
        }
    }
    let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
    cdp.absorb(json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"UNKNOWN","statusCode":200,"headers":{},"blockedCookies":[line_entry("blockedCookies", "host=UNKNOWN_BASE_VALUE")]}}));
    assert!(!serde_json::to_string(cdp.events()).unwrap().contains("UNKNOWN_BASE_VALUE"));
}

#[test]
fn set_cookie_domain_attributes_are_parsed_quoted_dotted_and_repeated() {
    let guard = crate::protected::Protected::default();
    let base = "https://shop.example.test/api";
    for line in [
        "a=b; Domain=\".adobelogin.com\"",
        "a=b; DOMAIN=.ADOBELOGIN.COM.",
        "a=b; Domain=adobelogin.com.",
        "a=b; Domain=example.test; Domain=.adobelogin.com",
        "a=b; domain=sliccy.ai; Domain=example.test",
        "a=b; Domain=\"sliccy.ai\"",
        "a=b\n Domain=.adobelogin.com",
        "a=b;\n Domain=.adobelogin.com",
        "a=b\n DOMAIN=.adobelogin.com",
        "a=b;\nDomain=.adobelogin.com",
    ] {
        assert!(crate::shield::set_cookie_hidden(&guard, line, base), "{line}");
        assert!(crate::shield::cookie_line_hidden(&guard, line, base), "{line}");
    }
    for line in ["a=b; Domain=\"example.test\"", "a=b; Domain=.adobe.com", "a=b; Domain=example.test.; Domain=shop.example.test", "a=b; Path=/", "sid=; Max-Age=0; Path=/"] {
        assert!(!crate::shield::set_cookie_hidden(&guard, line, base), "{line}");
    }
}

#[test]
fn ordinary_empty_value_set_cookie_deletion_headers_stay_visible() {
    let guard = crate::protected::Protected::default();
    let deletion = "sid=; Max-Age=0; Path=/";
    let base = "https://shop.example.test/api";
    assert!(!crate::shield::set_cookie_hidden(&guard, deletion, base));
    assert!(crate::shield::cookie_line_hidden(&guard, deletion, base));
    for domain in [".adobelogin.com", ".sliccy.ai"] {
        let line = format!("sid=; Max-Age=0; Path=/; Domain={domain}");
        assert!(crate::shield::set_cookie_hidden(&guard, &line, base), "{line}");
        assert!(crate::shield::cookie_line_hidden(&guard, &line, base), "{line}");
    }
    let mut events = vec![
        json!({"method": "Network.requestWillBeSent", "params": {"requestId": "R", "request": {"url": base, "headers": {}}}}),
        json!({"method": "Network.responseReceived", "params": {"requestId": "R", "response": {"url": base, "status": 200, "headers": {"Set-Cookie": deletion}}}}),
        json!({"method": "Network.responseReceivedExtraInfo", "params": {"requestId": "R", "statusCode": 200, "headers": {"set-cookie": deletion}}}),
    ];
    crate::shield::redact_cookie_events(&guard, &mut events, &json!({"cookies": []}));
    let text = serde_json::to_string(&events).unwrap();
    assert!(text.contains("sid=; Max-Age=0; Path=/"), "{text}");
    assert!(!text.contains("[redacted]"), "{text}");
    let mut hidden_events = vec![
        json!({"method": "Network.responseReceived", "params": {"requestId": "H", "response": {"url": base, "status": 200, "headers": {"Set-Cookie": "sid=; Max-Age=0; Path=/; Domain=.adobelogin.com"}}}}),
        json!({"method": "Network.responseReceived", "params": {"requestId": "P", "response": {"url": base, "status": 200, "headers": {"Set-Cookie": "sid=; Max-Age=0; Path=/; Domain=.sliccy.ai"}}}}),
    ];
    crate::shield::redact_cookie_events(&guard, &mut hidden_events, &json!({"cookies": []}));
    let hidden = serde_json::to_string(&hidden_events).unwrap();
    assert_eq!(hidden.matches("[redacted]").count(), 2, "{hidden}");
    assert!(!hidden.contains("sid=;"), "{hidden}");
    let empty = |domain: &str| json!([{"name": "Set-Cookie", "value": format!("sid=; Max-Age=0; Domain={domain}")}]);
    let mut fulfill = url_backend("https://example.test/");
    fulfill.hand("S", "R");
    fulfill.requests.insert("R".into(), false);
    assert_eq!(
        fulfill.call("Fetch.fulfillRequest", json!({"requestId": "R", "responseCode": 200, "responseHeaders": empty(".adobelogin.com")}), Some("S")).unwrap_err(),
        credential_refusal()
    );
    let mut continued = url_backend("https://example.test/");
    continued.absorb(json!({"method": "Fetch.requestPaused", "sessionId": "S", "params": {
        "requestId": "R", "networkId": "N", "frameId": "F1", "resourceType": "XHR",
        "request": {"url": "https://example.test/api", "method": "GET", "headers": {}},
        "responseStatusCode": 200,
        "responseHeaders": [{"name": "Content-Type", "value": "text/plain"}]
    }}));
    assert!(continued.next_paused(Some("S"), Duration::ZERO).unwrap().is_some());
    assert_eq!(
        continued.call("Fetch.continueResponse", json!({"requestId": "R", "responseCode": 200, "responseHeaders": empty(".adobelogin.com")}), Some("S")).unwrap_err(),
        credential_refusal()
    );
    let raw = crate::net::base64_encode(b"HTTP/1.1 200 OK\r\nSet-Cookie: sid=; Max-Age=0; Domain=.adobelogin.com\r\n\r\n");
    let mut intercepted = url_backend("https://example.test/");
    intercepted.hand("S", "I1");
    intercepted.requests.insert("I1".into(), false);
    assert_eq!(
        intercepted.call("Network.continueInterceptedRequest", json!({"interceptionId": "I1", "rawResponse": raw}), Some("S")).unwrap_err(),
        credential_refusal()
    );
    let mut protected = url_backend("https://example.test/");
    protected.hand("S", "R");
    protected.requests.insert("R".into(), false);
    assert_eq!(
        protected.call("Fetch.fulfillRequest", json!({"requestId": "R", "responseCode": 200, "responseHeaders": empty(".sliccy.ai")}), Some("S")).unwrap_err(),
        super::COOKIES
    );
}

#[test]
fn credential_response_side_set_cookie_writes_through_interception_are_refused() {
    let refusal = credential_refusal();
    let plant = |domain: &str| format!("sid=PLANTED; Domain={domain}; HttpOnly");
    let headers = |domain: &str| json!([{"name": "Set-Cookie", "value": plant(domain)}]);
    let mut fulfill = url_backend("https://example.test/");
    fulfill.hand("S", "R");
    fulfill.requests.insert("R".into(), false);
    assert_eq!(
        fulfill.call("Fetch.fulfillRequest", json!({"requestId": "R", "responseCode": 200, "responseHeaders": headers(".adobelogin.com")}), Some("S")).unwrap_err(),
        refusal
    );
    let binary = crate::net::base64_encode(b"Set-Cookie: sid=PLANTED; Domain=.adobelogin.com\0Content-Type: text/plain");
    let mut fulfill_binary = url_backend("https://example.test/");
    fulfill_binary.hand("S", "R");
    fulfill_binary.requests.insert("R".into(), false);
    assert_eq!(
        fulfill_binary.call("Fetch.fulfillRequest", json!({"requestId": "R", "responseCode": 200, "binaryResponseHeaders": binary}), Some("S")).unwrap_err(),
        refusal
    );
    let mut continue_response = url_backend("https://example.test/");
    continue_response.absorb(json!({"method": "Fetch.requestPaused", "sessionId": "S", "params": {
        "requestId": "R", "networkId": "N", "frameId": "F1", "resourceType": "XHR",
        "request": {"url": "https://example.test/api", "method": "GET", "headers": {}},
        "responseStatusCode": 200,
        "responseHeaders": [{"name": "Content-Type", "value": "text/plain"}]
    }}));
    assert!(continue_response.next_paused(Some("S"), Duration::ZERO).unwrap().is_some());
    assert_eq!(
        continue_response.call("Fetch.continueResponse", json!({"requestId": "R", "responseCode": 200, "responseHeaders": headers(".adobelogin.com")}), Some("S")).unwrap_err(),
        refusal
    );
    let raw = crate::net::base64_encode(b"HTTP/1.1 200 OK\r\nSet-Cookie: sid=PLANTED; Domain=.adobelogin.com; HttpOnly\r\n\r\n");
    let mut intercepted = url_backend("https://example.test/");
    intercepted.hand("S", "I1");
    intercepted.requests.insert("I1".into(), false);
    assert_eq!(
        intercepted.call("Network.continueInterceptedRequest", json!({"interceptionId": "I1", "rawResponse": raw}), Some("S")).unwrap_err(),
        refusal
    );
}

#[test]
fn credential_response_side_set_cookie2_writes_through_interception_are_refused() {
    let refusal = credential_refusal();
    let plant = |domain: &str| format!("sid=PLANTED; Domain={domain}; HttpOnly");
    for domain in [".adobelogin.com", "ims-na1.adobelogin.com", "ims-na1-stg1.adobelogin.com"] {
        let line = plant(domain);
        for name in ["Set-Cookie2", "SET-COOKIE2", "set-Cookie2", " Set-Cookie2 "] {
            let mut fulfill = url_backend("https://example.test/");
            fulfill.hand("S", "R");
            fulfill.requests.insert("R".into(), false);
            assert_eq!(
                fulfill.call("Fetch.fulfillRequest", json!({"requestId": "R", "responseCode": 200, "responseHeaders": [{"name": name, "value": &line}]}), Some("S")).unwrap_err(),
                refusal,
                "fulfill array {name:?} {domain}"
            );
            let mut cont = url_backend("https://example.test/");
            cont.absorb(json!({"method": "Fetch.requestPaused", "sessionId": "S", "params": {
                "requestId": "R", "networkId": "N", "frameId": "F1", "resourceType": "XHR",
                "request": {"url": "https://example.test/api", "method": "GET", "headers": {}},
                "responseStatusCode": 200,
                "responseHeaders": [{"name": "Content-Type", "value": "text/plain"}]
            }}));
            assert!(cont.next_paused(Some("S"), Duration::ZERO).unwrap().is_some());
            assert_eq!(
                cont.call("Fetch.continueResponse", json!({"requestId": "R", "responseCode": 200, "responseHeaders": [{"name": name, "value": &line}]}), Some("S")).unwrap_err(),
                refusal,
                "continueResponse array {name:?} {domain}"
            );
        }
        let binary = crate::net::base64_encode(format!("Set-Cookie2: {line}\0Content-Type: text/plain").as_bytes());
        let mut fulfill_binary = url_backend("https://example.test/");
        fulfill_binary.hand("S", "R");
        fulfill_binary.requests.insert("R".into(), false);
        assert_eq!(
            fulfill_binary.call("Fetch.fulfillRequest", json!({"requestId": "R", "responseCode": 200, "binaryResponseHeaders": binary}), Some("S")).unwrap_err(),
            refusal,
            "fulfill binary {domain}"
        );
        let binary_mixed = crate::net::base64_encode(format!("SET-COOKIE2: {line}\0Content-Type: text/plain").as_bytes());
        let mut fulfill_binary_mixed = url_backend("https://example.test/");
        fulfill_binary_mixed.hand("S", "R");
        fulfill_binary_mixed.requests.insert("R".into(), false);
        assert_eq!(
            fulfill_binary_mixed.call("Fetch.fulfillRequest", json!({"requestId": "R", "responseCode": 200, "binaryResponseHeaders": binary_mixed}), Some("S")).unwrap_err(),
            refusal,
            "fulfill binary SET-COOKIE2 {domain}"
        );
        let raw = crate::net::base64_encode(format!("HTTP/1.1 200 OK\r\nSet-Cookie2: {line}\r\n\r\n").as_bytes());
        let mut intercepted = url_backend("https://example.test/");
        intercepted.hand("S", "I1");
        intercepted.requests.insert("I1".into(), false);
        assert_eq!(
            intercepted.call("Network.continueInterceptedRequest", json!({"interceptionId": "I1", "rawResponse": raw}), Some("S")).unwrap_err(),
            refusal,
            "raw {domain}"
        );
        let raw_mixed = crate::net::base64_encode(format!("HTTP/1.1 200 OK\r\nset-Cookie2: {line}\r\n\r\n").as_bytes());
        let mut intercepted_mixed = url_backend("https://example.test/");
        intercepted_mixed.hand("S", "I1");
        intercepted_mixed.requests.insert("I1".into(), false);
        assert_eq!(
            intercepted_mixed.call("Network.continueInterceptedRequest", json!({"interceptionId": "I1", "rawResponse": raw_mixed}), Some("S")).unwrap_err(),
            refusal,
            "raw set-Cookie2 {domain}"
        );
        let mut headers_map = url_backend("https://example.test/");
        headers_map.hand("S", "I1");
        headers_map.requests.insert("I1".into(), false);
        assert_eq!(
            headers_map.call("Network.continueInterceptedRequest", json!({"interceptionId": "I1", "headers": {"Set-Cookie2": &line}}), Some("S")).unwrap_err(),
            refusal,
            "headers map {domain}"
        );
        let mut headers_map_mixed = url_backend("https://example.test/");
        headers_map_mixed.hand("S", "I1");
        headers_map_mixed.requests.insert("I1".into(), false);
        assert_eq!(
            headers_map_mixed.call("Network.continueInterceptedRequest", json!({"interceptionId": "I1", "headers": {"SET-COOKIE2": &line}}), Some("S")).unwrap_err(),
            refusal,
            "headers map SET-COOKIE2 {domain}"
        );
    }
    for line in [
        "a=b; Domain=\".adobelogin.com\"",
        "a=b; DOMAIN=.ADOBELOGIN.COM.",
        "a=b; Domain=adobelogin.com.",
        "a=b; Domain=example.test; Domain=.adobelogin.com",
        "a=b; Domain=\"ims-na1.adobelogin.com\"",
        "a=b; DOMAIN=IMS-NA1-STG1.ADOBELOGIN.COM.",
        "a=b; Domain=example.test; Domain=ims-na1.adobelogin.com",
    ] {
        let mut fulfill = url_backend("https://example.test/");
        fulfill.hand("S", "R");
        fulfill.requests.insert("R".into(), false);
        assert_eq!(
            fulfill.call("Fetch.fulfillRequest", json!({"requestId": "R", "responseCode": 200, "responseHeaders": [{"name": "Set-Cookie2", "value": line}]}), Some("S")).unwrap_err(),
            refusal,
            "{line}"
        );
        let mut cont = url_backend("https://example.test/");
        cont.absorb(json!({"method": "Fetch.requestPaused", "sessionId": "S", "params": {
            "requestId": "R", "networkId": "N", "frameId": "F1", "resourceType": "XHR",
            "request": {"url": "https://example.test/api", "method": "GET", "headers": {}},
            "responseStatusCode": 200,
            "responseHeaders": [{"name": "Content-Type", "value": "text/plain"}]
        }}));
        assert!(cont.next_paused(Some("S"), Duration::ZERO).unwrap().is_some());
        assert_eq!(
            cont.call("Fetch.continueResponse", json!({"requestId": "R", "responseCode": 200, "responseHeaders": [{"name": "Set-Cookie2", "value": line}]}), Some("S")).unwrap_err(),
            refusal,
            "continueResponse {line}"
        );
        let raw = crate::net::base64_encode(format!("HTTP/1.1 200 OK\r\nSet-Cookie2: {line}\r\n\r\n").as_bytes());
        let mut intercepted = url_backend("https://example.test/");
        intercepted.hand("S", "I1");
        intercepted.requests.insert("I1".into(), false);
        assert_eq!(
            intercepted.call("Network.continueInterceptedRequest", json!({"interceptionId": "I1", "rawResponse": raw}), Some("S")).unwrap_err(),
            refusal,
            "raw {line}"
        );
    }
    for line in ["sid=ok; Domain=example.test; Path=/", "sid=ok; Domain=.adobe.com; Path=/"] {
        let mut fulfill = url_backend("https://example.test/");
        fulfill.hand("S", "R");
        fulfill.requests.insert("R".into(), false);
        assert!(fulfill.call("Fetch.fulfillRequest", json!({"requestId": "R", "responseCode": 200, "responseHeaders": [{"name": "Set-Cookie2", "value": line}]}), Some("S")).is_ok(), "{line}");
    }
    let mut deletion = url_backend("https://example.test/");
    deletion.absorb(json!({"method": "Fetch.requestPaused", "sessionId": "S", "params": {
        "requestId": "R", "networkId": "N", "frameId": "F1", "resourceType": "XHR",
        "request": {"url": "https://shop.example.test/api", "method": "GET", "headers": {}},
        "responseStatusCode": 200,
        "responseHeaders": [{"name": "Content-Type", "value": "text/plain"}]
    }}));
    assert!(deletion.next_paused(Some("S"), Duration::ZERO).unwrap().is_some());
    assert!(deletion.call("Fetch.fulfillRequest", json!({"requestId": "R", "responseCode": 200, "responseHeaders": [{"name": "Set-Cookie2", "value": "sid=; Max-Age=0; Path=/"}]}), Some("S")).is_ok());
}

#[test]
fn audit_raw_cookie_lines_follow_the_same_parse() {
    let issue = |details: Value| json!({"method":"Audits.issueAdded","sessionId":"S","params":{"issue":{"code":"CookieIssue","details":{"cookieIssueDetails":details}}}});
    let url = json!({"requestId":"R","url":"https://shop.example.test/api"});
    for line in ["RAW_SECRET", "=RAW_SECRET", "id=RAW_SECRET; Domain=\".adobelogin.com\"", "id=RAW_SECRET; Domain=example.test; DOMAIN=ADOBELOGIN.COM.", "id=RAW_SECRET\n Domain=.adobelogin.com", "id=RAW_SECRET;\n Domain=.adobelogin.com"] {
        let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
        cdp.absorb(issue(json!({"request":url,"rawCookieLine":line,"operation":"SetCookie"})));
        assert!(cdp.events().is_empty(), "{line}: {:?}", cdp.events());
    }
    for line in ["id=RAW_KEPT; Domain=\"example.test\"", "id=RAW_KEPT; Domain=.adobe.com"] {
        let mut cdp = Cdp::from_transport(Box::new(OrdinaryReviewTransport { inbox: VecDeque::new() }));
        cdp.absorb(issue(json!({"request":url,"rawCookieLine":line,"operation":"SetCookie"})));
        assert_eq!(cdp.events().len(), 1, "{line}");
    }
}

#[test]
fn cookies_seen_only_in_extra_info_still_scrub_console_and_header_values_on_the_next_cookie_read() {
    for (domain, secret) in [(".adobelogin.com", "TRACE_IDP_SECRET"), ("seven.sliccy.ai", "TRACE_SEVEN_SECRET")] {
        let mut cdp = Cdp::from_transport(Box::new(CookieJarBackend { inbox: VecDeque::new(), jar: json!([]) }));
        cdp.fixture_session("S");
        ordinary_request(&mut cdp, "R", "https://shop.example.test/api");
        cdp.absorb(json!({"method":"Network.requestWillBeSentExtraInfo","sessionId":"S","params":{"requestId":"R","headers":{},
            "associatedCookies":[{"blockedReasons":[],"cookie":{"name":"sid","value":secret,"domain":domain,"path":"/"}}]}}));
        cdp.absorb(json!({"method":"Runtime.consoleAPICalled","sessionId":"S","params":{"type":"log","executionContextId":1,"timestamp":1.0,"args":[{"type":"string","value":format!("token {secret}")}]}}));
        cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R2","type":"XHR","documentURL":"https://shop.example.test/","request":{"url":"https://shop.example.test/next","method":"GET","headers":{"Cookie":format!("sid={secret}")}}}}));
        let before = serde_json::to_string(cdp.events()).unwrap();
        assert!(before.contains(secret), "{domain}: the fixture must queue the value: {before}");
        cdp.call("Network.getAllCookies", json!({}), Some("S")).unwrap();
        let after = serde_json::to_string(cdp.events()).unwrap();
        assert!(!after.contains(secret), "{domain}: {after}");
        assert!(after.contains("https://shop.example.test/next") && after.contains("[redacted]"), "{domain}: {after}");
    }
}
