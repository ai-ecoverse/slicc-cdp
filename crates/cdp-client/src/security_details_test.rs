const SCT_ORIGIN_VALUES: [&str; 3] = ["Embedded in certificate", "TLS extension", "OCSP response"];

struct SecurityDetailsTransport { inbox: VecDeque<String> }
impl Transport for SecurityDetailsTransport {
    fn send_text(&mut self, _: &str) -> Result<(), String> { Ok(()) }
    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { Ok(self.inbox.pop_front()) }
}

fn security_details_cdp() -> Cdp {
    Cdp::from_transport(Box::new(SecurityDetailsTransport { inbox: VecDeque::new() }))
}

fn timestamp(origin: &str) -> Value {
    json!({"status":"Verified","origin":origin,"logDescription":"Ordinary log","logId":"00","timestamp":1700000000000.0,"hashAlgorithm":"SHA-256","signatureAlgorithm":"ECDSA","signatureData":"00"})
}

fn security_details(origins: &[&str]) -> Value {
    json!({
        "protocol":"TLS 1.3","keyExchange":"","keyExchangeGroup":"X25519MLKEM768","cipher":"AES_128_GCM","certificateId":0,
        "subjectName":"*.ordinary.invalid","sanList":["*.ordinary.invalid","ordinary.invalid"],"issuer":"Ordinary CA",
        "validFrom":1700000000.0,"validTo":1800000000.0,
        "signedCertificateTimestampList":origins.iter().map(|origin| timestamp(origin)).collect::<Vec<_>>(),
        "certificateTransparencyCompliance":"compliant","encryptedClientHello":false
    })
}

fn sct_response(url: &str, details: Value) -> Value {
    json!({"response":{"url":url,"status":200,"statusText":"","headers":{"content-type":"application/json"},"mimeType":"application/json","encodedDataLength":120,"protocol":"h2","securityState":"secure","securityDetails":details}})
}

fn sct_request(id: &str, url: &str) -> Value {
    json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":id,"loaderId":"L","type":"Fetch","documentURL":"https://ordinary.invalid/page","request":{"url":url,"method":"GET","headers":{}}}})
}

fn sct_delivered(params: Value, request_url: &str) -> (bool, bool) {
    let mut cdp = security_details_cdp();
    cdp.absorb(sct_request("SCT", request_url));
    let mut params = params;
    params["requestId"] = json!("SCT");
    params["type"] = json!("Fetch");
    cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":params}));
    cdp.absorb(json!({"method":"Network.loadingFinished","sessionId":"S","params":{"requestId":"SCT","timestamp":1.0,"encodedDataLength":120}}));
    let count = |method: &str| cdp.events().iter().filter(|event| event["method"] == method && event["params"]["requestId"] == "SCT").count();
    (count("Network.responseReceived") == 1 && count("Network.loadingFinished") == 1, count("Network.loadingFailed") == 0)
}

#[test]
fn certificate_timestamp_origin_enum_values_are_not_scanned_as_urls() {
    let guard = crate::protected::Protected::default();
    for origin in SCT_ORIGIN_VALUES {
        assert!(guard.url(origin), "{origin}");
        assert!(!crate::shield::loaded_urls_protected(&guard, &sct_response("https://ordinary.invalid/api", security_details(&[origin]))), "{origin}");
    }
    assert!(!crate::shield::loaded_urls_protected(&guard, &sct_response("https://ordinary.invalid/api", security_details(&SCT_ORIGIN_VALUES))));
    assert!(!crate::shield::loaded_urls_protected(&guard, &json!({"redirectResponse":sct_response("https://ordinary.invalid/api", security_details(&SCT_ORIGIN_VALUES))["response"]})));
}

#[test]
fn certificate_timestamp_origin_outside_the_enum_keeps_the_url_rule() {
    let guard = crate::protected::Protected::default();
    for origin in ["https://seven.sliccy.ai", "https://vault.sliccy.ai/private?code=1", "chrome-extension://abcdefghijklmnopabcdefghijklmnop", "Embedded in certificate ", "embedded in certificate", "TLS Extension", "Unknown origin", "Embedded in certificate\nhttps://seven.sliccy.ai"] {
        assert!(crate::shield::loaded_urls_protected(&guard, &sct_response("https://ordinary.invalid/api", security_details(&[origin]))), "{origin:?}");
        assert!(crate::shield::loaded_urls_protected(&guard, &sct_response("https://ordinary.invalid/api", security_details(&["TLS extension", origin]))), "{origin:?}");
    }
}

#[test]
fn certificate_timestamp_enum_values_elsewhere_keep_the_url_rule() {
    let guard = crate::protected::Protected::default();
    for origin in SCT_ORIGIN_VALUES {
        for value in [
            json!({"origin":origin}),
            json!({"response":{"url":"https://ordinary.invalid/api","origin":origin}}),
            json!({"securityDetails":{"origin":origin}}),
            json!({"securityDetails":{"signedCertificateTimestampList":{"origin":origin}}}),
            json!({"securityDetails":{"signedCertificateTimestampList":[[{"origin":origin}]]}}),
            json!({"securityDetails":{"signedCertificateTimestampList":[{"nested":{"origin":origin}}]}}),
            json!({"securityDetails":{"other":{"signedCertificateTimestampList":[{"origin":origin}]}}}),
            json!({"signedCertificateTimestampList":[{"origin":origin}]}),
            json!({"securityDetails":{"signedCertificateTimestampList":[{"url":origin}]}}),
            json!({"securityDetails":{"signedCertificateTimestampList":[{"securityOrigin":origin}]}}),
            json!({"initiator":{"url":origin}}),
        ] {
            assert!(crate::shield::loaded_urls_protected(&guard, &value), "{value}");
        }
    }
}

#[test]
fn protected_urls_beside_certificate_timestamps_are_still_found() {
    let guard = crate::protected::Protected::default();
    let protected = "https://seven.sliccy.ai/private";
    let mut details_url = security_details(&SCT_ORIGIN_VALUES);
    details_url["origin"] = json!(protected);
    let mut timestamp_url = security_details(&SCT_ORIGIN_VALUES);
    timestamp_url["signedCertificateTimestampList"][1]["url"] = json!(protected);
    let mut nested = security_details(&SCT_ORIGIN_VALUES);
    nested["extra"] = json!({"securityOrigin":protected});
    for value in [
        sct_response(protected, security_details(&SCT_ORIGIN_VALUES)),
        sct_response("https://ordinary.invalid/api", details_url),
        sct_response("https://ordinary.invalid/api", timestamp_url),
        sct_response("https://ordinary.invalid/api", nested),
        json!({"redirectResponse":sct_response(protected, security_details(&SCT_ORIGIN_VALUES))["response"]}),
        json!({"response":sct_response("https://ordinary.invalid/api", security_details(&SCT_ORIGIN_VALUES))["response"],"initiator":{"url":protected}}),
    ] {
        assert!(crate::shield::loaded_urls_protected(&guard, &value), "{value}");
    }
}

#[test]
fn responses_on_a_new_tls_connection_are_delivered_with_their_terminal_event() {
    for origins in [&SCT_ORIGIN_VALUES[..1], &SCT_ORIGIN_VALUES[1..2], &SCT_ORIGIN_VALUES[2..], &SCT_ORIGIN_VALUES[..], &[][..]] {
        assert_eq!(sct_delivered(sct_response("https://ordinary.invalid/api", security_details(origins)), "https://ordinary.invalid/api"), (true, true), "{origins:?}");
    }
}

#[test]
fn protected_responses_on_a_new_tls_connection_stay_refused() {
    let protected = "https://seven.sliccy.ai/private";
    let ordinary = "https://ordinary.invalid/api";
    let mut timestamp_url = security_details(&SCT_ORIGIN_VALUES);
    timestamp_url["signedCertificateTimestampList"][0]["origin"] = json!(protected);
    let mut unknown = security_details(&SCT_ORIGIN_VALUES);
    unknown["signedCertificateTimestampList"][2]["origin"] = json!("Unknown origin");
    let mut details_url = security_details(&SCT_ORIGIN_VALUES);
    details_url["origin"] = json!(protected);
    let mut location = sct_response(ordinary, security_details(&SCT_ORIGIN_VALUES));
    location["response"]["headers"]["location"] = json!(protected);
    location["response"]["status"] = json!(302);
    for (params, request) in [
        (sct_response(protected, security_details(&SCT_ORIGIN_VALUES)), ordinary),
        (sct_response(ordinary, security_details(&SCT_ORIGIN_VALUES)), protected),
        (sct_response(ordinary, timestamp_url), ordinary),
        (sct_response(ordinary, unknown), ordinary),
        (sct_response(ordinary, details_url), ordinary),
        (location, ordinary),
    ] {
        let (delivered, _) = sct_delivered(params.clone(), request);
        assert!(!delivered, "{request} {params}");
    }
}

#[test]
fn a_page_load_mixing_new_and_reused_tls_connections_delivers_every_response() {
    let mut cdp = security_details_cdp();
    let fresh = 41;
    let reused = 29;
    for index in 0..fresh + reused {
        let id = format!("R{index}");
        let url = format!("https://ordinary.invalid/api/{index}");
        cdp.absorb(sct_request(&id, &url));
        let mut params = if index < fresh {
            sct_response(&url, security_details(&SCT_ORIGIN_VALUES[index % 2..index % 2 + 2]))
        } else {
            json!({"response":{"url":url,"status":200,"headers":{},"mimeType":"application/json","encodedDataLength":0,"protocol":"h2","securityState":"secure"}})
        };
        params["requestId"] = json!(id);
        params["type"] = json!("Fetch");
        cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":params}));
        cdp.absorb(json!({"method":"Network.loadingFinished","sessionId":"S","params":{"requestId":id,"timestamp":1.0,"encodedDataLength":120}}));
    }
    let count = |method: &str| cdp.events().iter().filter(|event| event["method"] == method).count();
    assert_eq!(count("Network.responseReceived"), fresh + reused);
    assert_eq!(count("Network.loadingFinished"), fresh + reused);
    assert_eq!(count("Network.loadingFailed"), 0);
}
