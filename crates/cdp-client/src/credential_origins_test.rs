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
