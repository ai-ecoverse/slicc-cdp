fn plant(domain: &str) -> String {
    format!("sid=PLANTED; Domain={domain}; HttpOnly")
}

fn headers(domain: &str) -> Value {
    json!([{ "name": "Set-Cookie", "value": plant(domain) }])
}

fn handed(url: &str, id: &str) -> Cdp {
    let mut cdp = url_backend(url);
    cdp.hand("S", id);
    cdp.requests.insert(id.into(), false);
    cdp
}

fn response_paused(url: &str, id: &str) -> Cdp {
    let mut cdp = url_backend("https://example.test/");
    cdp.absorb(json!({
        "method": "Fetch.requestPaused",
        "sessionId": "S",
        "params": {
            "requestId": id,
            "networkId": "N",
            "frameId": "F1",
            "resourceType": "XHR",
            "request": { "url": url, "method": "GET", "headers": {} },
            "responseStatusCode": 200,
            "responseHeaders": [{ "name": "Content-Type", "value": "text/plain" }]
        }
    }));
    assert!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().is_some());
    cdp
}

#[test]
fn response_side_set_cookie_writes_through_interception_are_refused() {
    for domain in ["seven.sliccy.ai", "vault.sliccy.ai", ".sliccy.ai"] {
        let mut fulfill = handed("https://example.test/", "R");
        assert_eq!(
            fulfill
                .call(
                    "Fetch.fulfillRequest",
                    json!({ "requestId": "R", "responseCode": 200, "responseHeaders": headers(domain) }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "fulfill responseHeaders {domain}"
        );
        let binary = crate::net::base64_encode(format!("Set-Cookie: {}\0Content-Type: text/plain", plant(domain)).as_bytes());
        let mut fulfill_binary = handed("https://example.test/", "R");
        assert_eq!(
            fulfill_binary
                .call(
                    "Fetch.fulfillRequest",
                    json!({ "requestId": "R", "responseCode": 200, "binaryResponseHeaders": binary }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "fulfill binaryResponseHeaders {domain}"
        );
        let mut cont = response_paused("https://example.test/api", "R");
        assert_eq!(
            cont.call(
                "Fetch.continueResponse",
                json!({ "requestId": "R", "responseCode": 200, "responseHeaders": headers(domain) }),
                Some("S")
            )
            .unwrap_err(),
            super::COOKIES,
            "continueResponse {domain}"
        );
        let raw = crate::net::base64_encode(format!("HTTP/1.1 200 OK\r\nSet-Cookie: {}\r\n\r\n", plant(domain)).as_bytes());
        let mut intercepted = handed("https://example.test/", "I1");
        assert_eq!(
            intercepted
                .call(
                    "Network.continueInterceptedRequest",
                    json!({ "interceptionId": "I1", "rawResponse": raw }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "continueInterceptedRequest {domain}"
        );
    }
}

#[test]
fn response_side_set_cookie_quoted_mixed_case_trailing_dot_and_repeated_domains_are_refused() {
    let base = "https://shop.example.test/api";
    for line in [
        "a=b; Domain=\".sliccy.ai\"",
        "a=b; DOMAIN=.SLICCY.AI.",
        "a=b; Domain=sliccy.ai.",
        "a=b; Domain=example.test; Domain=.sliccy.ai",
        "a=b; domain=sliccy.ai; Domain=example.test",
        "a=b; Domain=\"seven.sliccy.ai\"",
        "a=b; DOMAIN=VAULT.SLICCY.AI.",
        "a=b; Domain=example.test; Domain=seven.sliccy.ai",
    ] {
        let mut fulfill = handed(base, "R");
        assert_eq!(
            fulfill
                .call(
                    "Fetch.fulfillRequest",
                    json!({ "requestId": "R", "responseCode": 200, "responseHeaders": [{ "name": "Set-Cookie", "value": line }] }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "{line}"
        );
    }
}

#[test]
fn response_side_set_cookie_unparseable_forms_fail_closed() {
    let mut bad_headers = handed("https://example.test/", "R");
    assert_eq!(
        bad_headers
            .call(
                "Fetch.fulfillRequest",
                json!({ "requestId": "R", "responseCode": 200, "responseHeaders": { "Set-Cookie": "sid=x; Domain=.sliccy.ai" } }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
    let mut missing_name = handed("https://example.test/", "R");
    assert_eq!(
        missing_name
            .call(
                "Fetch.fulfillRequest",
                json!({ "requestId": "R", "responseCode": 200, "responseHeaders": [{ "value": "sid=x; Domain=.sliccy.ai" }] }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
    let mut missing_value = handed("https://example.test/", "R");
    assert_eq!(
        missing_value
            .call(
                "Fetch.fulfillRequest",
                json!({ "requestId": "R", "responseCode": 200, "responseHeaders": [{ "name": "Set-Cookie", "value": 1 }] }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
    let mut bad_binary = handed("https://example.test/", "R");
    assert_eq!(
        bad_binary
            .call(
                "Fetch.fulfillRequest",
                json!({ "requestId": "R", "responseCode": 200, "binaryResponseHeaders": "!" }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
    let mut binary_not_string = handed("https://example.test/", "R");
    assert_eq!(
        binary_not_string
            .call(
                "Fetch.fulfillRequest",
                json!({ "requestId": "R", "responseCode": 200, "binaryResponseHeaders": 1 }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
    let mut bad_raw = handed("https://example.test/", "I1");
    assert_eq!(
        bad_raw
            .call(
                "Network.continueInterceptedRequest",
                json!({ "interceptionId": "I1", "rawResponse": "!" }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
    let mut raw_not_string = handed("https://example.test/", "I1");
    assert_eq!(
        raw_not_string
            .call(
                "Network.continueInterceptedRequest",
                json!({ "interceptionId": "I1", "rawResponse": 1 }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
    for line in [
        "nospace",
        "=value",
        "a=b\n Domain=.sliccy.ai",
        "a=b;\n Domain=.sliccy.ai",
        "a=b\n DOMAIN=.sliccy.ai",
        "a=b;\nDomain=.sliccy.ai",
    ] {
        let mut cdp = response_paused("https://shop.example.test/api", "R");
        assert_eq!(
            cdp.call(
                "Fetch.fulfillRequest",
                json!({ "requestId": "R", "responseCode": 200, "responseHeaders": [{ "name": "Set-Cookie", "value": line }] }),
                Some("S")
            )
            .unwrap_err(),
            super::COOKIES,
            "{line}"
        );
    }
}

#[test]
fn response_side_set_cookie_ordinary_controls_stay_allowed() {
    let mut ordinary_domain = handed("https://example.test/", "R");
    assert!(ordinary_domain
        .call(
            "Fetch.fulfillRequest",
            json!({
                "requestId": "R",
                "responseCode": 200,
                "responseHeaders": [{ "name": "Set-Cookie", "value": "sid=ok; Domain=example.test; Path=/" }]
            }),
            Some("S")
        )
        .is_ok());
    let mut no_domain = response_paused("https://shop.example.test/api", "R");
    assert!(no_domain
        .call(
            "Fetch.fulfillRequest",
            json!({
                "requestId": "R",
                "responseCode": 200,
                "responseHeaders": [{ "name": "Set-Cookie", "value": "sid=ok; Path=/" }]
            }),
            Some("S")
        )
        .is_ok());
    let mut deletion = response_paused("https://shop.example.test/api", "R");
    assert!(deletion
        .call(
            "Fetch.fulfillRequest",
            json!({
                "requestId": "R",
                "responseCode": 200,
                "responseHeaders": [{ "name": "Set-Cookie", "value": "sid=; Max-Age=0; Path=/" }]
            }),
            Some("S")
        )
        .is_ok());
    let mut no_cookie = handed("https://example.test/", "R");
    assert!(no_cookie
        .call(
            "Fetch.fulfillRequest",
            json!({
                "requestId": "R",
                "responseCode": 200,
                "responseHeaders": [{ "name": "Content-Type", "value": "text/plain" }]
            }),
            Some("S")
        )
        .is_ok());
    let mut continue_ok = response_paused("https://example.test/api", "R");
    assert!(continue_ok
        .call(
            "Fetch.continueResponse",
            json!({
                "requestId": "R",
                "responseCode": 200,
                "responseHeaders": [{ "name": "Content-Type", "value": "text/plain" }]
            }),
            Some("S")
        )
        .is_ok());
    let raw_ok = crate::net::base64_encode(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nX-Trace: 1\r\n\r\nbody Domain=.sliccy.ai planted",
    );
    let mut raw_body = handed("https://example.test/", "I1");
    assert!(raw_body
        .call(
            "Network.continueInterceptedRequest",
            json!({ "interceptionId": "I1", "rawResponse": raw_ok }),
            Some("S")
        )
        .is_ok());
    let mut raw_delete = response_paused("https://shop.example.test/api", "I1");
    let raw_delete_body = crate::net::base64_encode(b"HTTP/1.1 200 OK\r\nSet-Cookie: sid=; Max-Age=0\r\n\r\n");
    assert!(raw_delete
        .call(
            "Network.continueInterceptedRequest",
            json!({ "interceptionId": "I1", "rawResponse": raw_delete_body }),
            Some("S")
        )
        .is_ok());
    let mut multi = handed("https://example.test/", "R");
    assert!(multi
        .call(
            "Fetch.fulfillRequest",
            json!({
                "requestId": "R",
                "responseCode": 200,
                "responseHeaders": [
                    { "name": "Content-Type", "value": "text/plain" },
                    { "name": "X-Trace", "value": "1" },
                    { "name": "Cache-Control", "value": "no-store" }
                ]
            }),
            Some("S")
        )
        .is_ok());
}

#[test]
fn response_side_obs_fold_continuations_in_raw_and_binary_fail_closed() {
    for raw in [
        b"HTTP/1.1 200 OK\r\nSet-Cookie: a=b;\r\n Domain=.sliccy.ai\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nSet-Cookie: a=b;\r\n\tDomain=.sliccy.ai\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\nSet-Cookie: a=b;\n Domain=.sliccy.ai\n\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nSet-Cookie: a=b;\r\n Domain=.sliccy.ai; Expires=Wed, 21 Oct 2015 07:28:00 GMT\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nSet-Cookie: a=b;\r\n\tDomain=.sliccy.ai; Expires=Wed, 21 Oct 2015 07:28:00 GMT\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\nSet-Cookie: a=b;\n Domain=.sliccy.ai; Expires=Wed, 21 Oct 2015 07:28:00 GMT\n\n".as_slice(),
    ] {
        let mut cdp = response_paused("https://shop.example.test/api", "I1");
        assert_eq!(
            cdp.call(
                "Network.continueInterceptedRequest",
                json!({ "interceptionId": "I1", "rawResponse": crate::net::base64_encode(raw) }),
                Some("S")
            )
            .unwrap_err(),
            super::COOKIES,
            "{raw:?}"
        );
    }
    for binary in [
        b"Set-Cookie: a=b;\n Domain=.sliccy.ai\0Content-Type: text/plain".as_slice(),
        b"Set-Cookie: a=b;\n Domain=.sliccy.ai; Expires=Wed, 21 Oct 2015 07:28:00 GMT\0Content-Type: text/plain".as_slice(),
        b"Set-Cookie: a=b;\n\tDomain=.sliccy.ai; Expires=Wed, 21 Oct 2015 07:28:00 GMT\0Content-Type: text/plain".as_slice(),
    ] {
        let mut fulfill = response_paused("https://shop.example.test/api", "R");
        assert_eq!(
            fulfill
                .call(
                    "Fetch.fulfillRequest",
                    json!({ "requestId": "R", "responseCode": 200, "binaryResponseHeaders": crate::net::base64_encode(binary) }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "{binary:?}"
        );
    }
}

#[test]
fn response_side_raw_header_block_ends_at_lf_crlf() {
    let ordinary = crate::net::base64_encode(b"HTTP/1.1 200 OK\nContent-Type: text/plain\n\r\nhello");
    let mut ok = response_paused("https://shop.example.test/api", "I1");
    assert!(ok
        .call(
            "Network.continueInterceptedRequest",
            json!({ "interceptionId": "I1", "rawResponse": ordinary }),
            Some("S")
        )
        .is_ok());
    let planted = crate::net::base64_encode(
        b"HTTP/1.1 200 OK\nSet-Cookie: sid=PLANTED; Domain=.sliccy.ai\n\r\nhello",
    );
    let mut refuse = response_paused("https://shop.example.test/api", "I1");
    assert_eq!(
        refuse
            .call(
                "Network.continueInterceptedRequest",
                json!({ "interceptionId": "I1", "rawResponse": planted }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
    let body_plant = crate::net::base64_encode(
        b"HTTP/1.1 200 OK\nContent-Type: text/plain\n\r\nSet-Cookie: sid=PLANTED; Domain=.sliccy.ai",
    );
    let mut body_ok = response_paused("https://shop.example.test/api", "I1");
    assert!(body_ok
        .call(
            "Network.continueInterceptedRequest",
            json!({ "interceptionId": "I1", "rawResponse": body_plant }),
            Some("S")
        )
        .is_ok());
}

#[test]
fn response_side_bare_cr_and_status_line_smuggling_fail_closed() {
    for raw in [
        b"HTTP/1.1 200 OK\rSet-Cookie: sid=x; Domain=.sliccy.ai\r\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\rSet-Cookie: sid=x; Domain=.sliccy.ai\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\0X-Trace: 1\r\n\r\n".as_slice(),
        b"HTTP/1.1 200 OK Set-Cookie: sid=x; Domain=.sliccy.ai\r\nContent-Type: text/plain\r\n\r\n".as_slice(),
    ] {
        let mut cdp = response_paused("https://shop.example.test/api", "I1");
        assert_eq!(
            cdp.call(
                "Network.continueInterceptedRequest",
                json!({ "interceptionId": "I1", "rawResponse": crate::net::base64_encode(raw) }),
                Some("S")
            )
            .unwrap_err(),
            super::COOKIES,
            "{raw:?}"
        );
    }
    for binary in [
        b"Content-Type: text/plain\rSet-Cookie: sid=x; Domain=.sliccy.ai".as_slice(),
        b"Content-Type: text/plain\x01X-Trace: 1".as_slice(),
        b"Set-Cookie: sid=x; Domain=.sliccy.ai\r\nContent-Type: text/plain".as_slice(),
    ] {
        let mut cdp = response_paused("https://shop.example.test/api", "R");
        assert_eq!(
            cdp.call(
                "Fetch.fulfillRequest",
                json!({ "requestId": "R", "responseCode": 200, "binaryResponseHeaders": crate::net::base64_encode(binary) }),
                Some("S")
            )
            .unwrap_err(),
            super::COOKIES,
            "{binary:?}"
        );
    }
    let mut crlf = response_paused("https://shop.example.test/api", "I1");
    assert!(crlf
        .call(
            "Network.continueInterceptedRequest",
            json!({
                "interceptionId": "I1",
                "rawResponse": crate::net::base64_encode(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n")
            }),
            Some("S")
        )
        .is_ok());
    let mut lf_only = response_paused("https://shop.example.test/api", "I1");
    assert!(lf_only
        .call(
            "Network.continueInterceptedRequest",
            json!({
                "interceptionId": "I1",
                "rawResponse": crate::net::base64_encode(b"HTTP/1.1 200 OK\nContent-Type: text/plain\n\n")
            }),
            Some("S")
        )
        .is_ok());
    let mut deletion = response_paused("https://shop.example.test/api", "I1");
    assert!(deletion
        .call(
            "Network.continueInterceptedRequest",
            json!({
                "interceptionId": "I1",
                "rawResponse": crate::net::base64_encode(b"HTTP/1.1 200 OK\r\nSet-Cookie: sid=; Max-Age=0\r\n\r\n")
            }),
            Some("S")
        )
        .is_ok());
    let mut body_cr = response_paused("https://shop.example.test/api", "I1");
    assert!(body_cr
        .call(
            "Network.continueInterceptedRequest",
            json!({
                "interceptionId": "I1",
                "rawResponse": crate::net::base64_encode(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nline1\rline2")
            }),
            Some("S")
        )
        .is_ok());
}

#[test]
fn response_side_empty_set_cookie_line_is_refused() {
    let mut array = response_paused("https://shop.example.test/api", "R");
    assert_eq!(
        array
            .call(
                "Fetch.fulfillRequest",
                json!({
                    "requestId": "R",
                    "responseCode": 200,
                    "responseHeaders": [{ "name": "Set-Cookie", "value": "" }]
                }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
    let binary = crate::net::base64_encode(b"Set-Cookie:\0Content-Type: text/plain");
    let mut fulfill = response_paused("https://shop.example.test/api", "R");
    assert_eq!(
        fulfill
            .call(
                "Fetch.fulfillRequest",
                json!({ "requestId": "R", "responseCode": 200, "binaryResponseHeaders": binary }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
    let raw = crate::net::base64_encode(b"HTTP/1.1 200 OK\r\nSet-Cookie:\r\n\r\n");
    let mut intercepted = response_paused("https://shop.example.test/api", "I1");
    assert_eq!(
        intercepted
            .call(
                "Network.continueInterceptedRequest",
                json!({ "interceptionId": "I1", "rawResponse": raw }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES
    );
}

#[test]
fn response_side_array_header_smuggling_fail_closed() {
    for method in ["Fetch.fulfillRequest", "Fetch.continueResponse"] {
        let name_smuggle = json!([
            { "name": "X-Foo\r\nSet-Cookie", "value": "a=b; Domain=.sliccy.ai" },
            { "name": "Content-Type", "value": "text/plain" }
        ]);
        let value_smuggle = json!([
            { "name": "X-Foo", "value": "bar\r\nSet-Cookie: a=b; Domain=.sliccy.ai" },
            { "name": "Content-Type", "value": "text/plain" }
        ]);
        let nul_name = json!([{ "name": "X-Foo\0Set-Cookie", "value": "a=b; Domain=.sliccy.ai" }]);
        let nul_value = json!([{ "name": "X-Foo", "value": "bar\0Set-Cookie: a=b; Domain=.sliccy.ai" }]);
        for headers in [name_smuggle, value_smuggle, nul_name, nul_value] {
            let mut cdp = response_paused("https://shop.example.test/api", "R");
            assert_eq!(
                cdp.call(
                    method,
                    json!({ "requestId": "R", "responseCode": 200, "responseHeaders": headers }),
                    Some("S")
                )
                .unwrap_err(),
                super::COOKIES,
                "{method} {headers}"
            );
        }
    }
}

#[test]
fn response_side_set_cookie2_writes_through_interception_are_refused() {
    for domain in ["seven.sliccy.ai", "vault.sliccy.ai", ".sliccy.ai"] {
        let mut fulfill = handed("https://example.test/", "R");
        assert_eq!(
            fulfill
                .call(
                    "Fetch.fulfillRequest",
                    json!({
                        "requestId": "R",
                        "responseCode": 200,
                        "responseHeaders": [{ "name": "Set-Cookie2", "value": plant(domain) }]
                    }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "fulfill responseHeaders Set-Cookie2 {domain}"
        );
        let binary = crate::net::base64_encode(
            format!("Set-Cookie2: {}\0Content-Type: text/plain", plant(domain)).as_bytes(),
        );
        let mut fulfill_binary = handed("https://example.test/", "R");
        assert_eq!(
            fulfill_binary
                .call(
                    "Fetch.fulfillRequest",
                    json!({ "requestId": "R", "responseCode": 200, "binaryResponseHeaders": binary }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "fulfill binaryResponseHeaders Set-Cookie2 {domain}"
        );
        let mut cont = response_paused("https://example.test/api", "R");
        assert_eq!(
            cont.call(
                "Fetch.continueResponse",
                json!({
                    "requestId": "R",
                    "responseCode": 200,
                    "responseHeaders": [{ "name": "Set-Cookie2", "value": plant(domain) }]
                }),
                Some("S")
            )
            .unwrap_err(),
            super::COOKIES,
            "continueResponse Set-Cookie2 {domain}"
        );
        let raw = crate::net::base64_encode(
            format!("HTTP/1.1 200 OK\r\nSet-Cookie2: {}\r\n\r\n", plant(domain)).as_bytes(),
        );
        let mut intercepted = handed("https://example.test/", "I1");
        assert_eq!(
            intercepted
                .call(
                    "Network.continueInterceptedRequest",
                    json!({ "interceptionId": "I1", "rawResponse": raw }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "continueInterceptedRequest rawResponse Set-Cookie2 {domain}"
        );
        let mut headers_map = handed("https://example.test/", "I1");
        assert_eq!(
            headers_map
                .call(
                    "Network.continueInterceptedRequest",
                    json!({
                        "interceptionId": "I1",
                        "headers": { "Set-Cookie2": plant(domain) }
                    }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "continueInterceptedRequest headers Set-Cookie2 {domain}"
        );
    }
}

#[test]
fn response_side_set_cookie2_mixed_case_and_padded_names_are_refused() {
    let line = plant(".sliccy.ai");
    for name in ["SET-COOKIE2", "set-Cookie2", " Set-Cookie2 "] {
        let mut fulfill = handed("https://example.test/", "R");
        assert_eq!(
            fulfill
                .call(
                    "Fetch.fulfillRequest",
                    json!({
                        "requestId": "R",
                        "responseCode": 200,
                        "responseHeaders": [{ "name": name, "value": line }]
                    }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "array name {name:?}"
        );
    }
    let binary = crate::net::base64_encode(
        format!("SET-COOKIE2: {line}\0Content-Type: text/plain").as_bytes(),
    );
    let mut fulfill_binary = handed("https://example.test/", "R");
    assert_eq!(
        fulfill_binary
            .call(
                "Fetch.fulfillRequest",
                json!({ "requestId": "R", "responseCode": 200, "binaryResponseHeaders": binary }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES,
        "binary SET-COOKIE2"
    );
    let raw = crate::net::base64_encode(format!("HTTP/1.1 200 OK\r\nset-Cookie2: {line}\r\n\r\n").as_bytes());
    let mut intercepted = handed("https://example.test/", "I1");
    assert_eq!(
        intercepted
            .call(
                "Network.continueInterceptedRequest",
                json!({ "interceptionId": "I1", "rawResponse": raw }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES,
        "raw set-Cookie2"
    );
    let mut headers_map = handed("https://example.test/", "I1");
    assert_eq!(
        headers_map
            .call(
                "Network.continueInterceptedRequest",
                json!({
                    "interceptionId": "I1",
                    "headers": { "SET-COOKIE2": line }
                }),
                Some("S")
            )
            .unwrap_err(),
        super::COOKIES,
        "headers map SET-COOKIE2"
    );
}

#[test]
fn response_side_set_cookie2_unparseable_value_is_refused() {
    for line in ["nospace", "=value", ""] {
        let mut fulfill = handed("https://example.test/", "R");
        assert_eq!(
            fulfill
                .call(
                    "Fetch.fulfillRequest",
                    json!({
                        "requestId": "R",
                        "responseCode": 200,
                        "responseHeaders": [{ "name": "Set-Cookie2", "value": line }]
                    }),
                    Some("S")
                )
                .unwrap_err(),
            super::COOKIES,
            "unparseable Set-Cookie2 {line:?}"
        );
    }
}

#[test]
fn response_side_set_cookie2_ordinary_controls_stay_allowed() {
    let mut ordinary_domain = handed("https://example.test/", "R");
    assert!(ordinary_domain
        .call(
            "Fetch.fulfillRequest",
            json!({
                "requestId": "R",
                "responseCode": 200,
                "responseHeaders": [{ "name": "Set-Cookie2", "value": "sid=ok; Domain=example.test; Path=/" }]
            }),
            Some("S")
        )
        .is_ok());
    let mut no_domain = response_paused("https://shop.example.test/api", "R");
    assert!(no_domain
        .call(
            "Fetch.fulfillRequest",
            json!({
                "requestId": "R",
                "responseCode": 200,
                "responseHeaders": [{ "name": "Set-Cookie2", "value": "sid=ok; Path=/" }]
            }),
            Some("S")
        )
        .is_ok());
    let mut deletion = response_paused("https://shop.example.test/api", "R");
    assert!(deletion
        .call(
            "Fetch.fulfillRequest",
            json!({
                "requestId": "R",
                "responseCode": 200,
                "responseHeaders": [{ "name": "Set-Cookie2", "value": "sid=; Max-Age=0; Path=/" }]
            }),
            Some("S")
        )
        .is_ok());
    let binary = crate::net::base64_encode(b"Set-Cookie2: sid=ok; Domain=example.test\0Content-Type: text/plain");
    let mut fulfill_binary = handed("https://example.test/", "R");
    assert!(fulfill_binary
        .call(
            "Fetch.fulfillRequest",
            json!({ "requestId": "R", "responseCode": 200, "binaryResponseHeaders": binary }),
            Some("S")
        )
        .is_ok());
    let raw = crate::net::base64_encode(b"HTTP/1.1 200 OK\r\nSet-Cookie2: sid=; Max-Age=0\r\n\r\n");
    let mut intercepted = response_paused("https://shop.example.test/api", "I1");
    assert!(intercepted
        .call(
            "Network.continueInterceptedRequest",
            json!({ "interceptionId": "I1", "rawResponse": raw }),
            Some("S")
        )
        .is_ok());
    let mut headers_map = handed("https://example.test/", "I1");
    assert!(headers_map
        .call(
            "Network.continueInterceptedRequest",
            json!({
                "interceptionId": "I1",
                "headers": { "Set-Cookie2": "sid=ok; Domain=example.test; Path=/" }
            }),
            Some("S")
        )
        .is_ok());
}

#[test]
fn legacy_network_interception_body_readers_are_refused() {
    for method in [
        "Network.getResponseBodyForInterception",
        "Network.takeResponseBodyForInterceptionAsStream",
    ] {
        let err = url_backend("https://example.test/")
            .call(method, json!({ "interceptionId": "I1" }), Some("S"))
            .unwrap_err();
        assert!(err.starts_with("slicc: refused:"), "{method}: {err}");
        assert!(err.contains("Fetch"), "{method}: {err}");
    }
    let enable = url_backend("https://example.test/")
        .call("Network.setRequestInterception", json!({ "patterns": [] }), Some("S"))
        .unwrap_err();
    assert!(enable.starts_with("slicc: refused:"));
}
