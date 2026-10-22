#[test]
fn ordinary_pauses_keep_their_actual_request_url_without_disclosing_private_response_carriers() {
    let destination = "https://seven.sliccy.ai/canary/owned-navigation";
    let url = format!("https://ordinary.invalid/page?to={}&tag=owned", destination.bytes().map(|byte| format!("%{byte:02X}")).collect::<String>());
    let mut cdp = url_backend("about:blank");
    cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"S","params":{"requestId":"F","networkId":"N","resourceType":"Document","request":{"url":url,"method":"GET","headers":{"X-Reference":"https://vault.sliccy.ai/private?code=PAUSE_RESPONSE_SECRET"}},"responseStatusCode":200,"responseHeaders":[{"name":"X-Reference","value":"https://vault.sliccy.ai/private?code=PAUSE_RESPONSE_SECRET"}]}}));
    let paused = cdp.paused.front().unwrap();
    assert_eq!(paused.url, url);
    assert!(!format!("{:?}", paused).contains("PAUSE_RESPONSE_SECRET"));
    assert!(!cdp.redirects.contains("N"));
    assert!(!cdp.protected_body_requests.contains("N"));
}

#[test]
fn interception_diagnostics_record_the_observed_url_not_its_client_redaction() {
    let url = "https://seven.sliccy.ai/nowhere?secret=RAW_INTERCEPTION_PROOF";
    for guard in [false, true] {
        let mut cdp = url_backend("about:blank");
        if guard { cdp.guard_sessions.insert("S".into()); }
        cdp.absorb(json!({"method":"Fetch.requestPaused","sessionId":"S","params":{"requestId":"F","networkId":"N","resourceType":"Document","request":{"url":url,"method":"GET","headers":{}}}}));
        assert!(cdp.paused_trail.iter().any(|(session, observed, reason)| session == "S" && observed == url && matches!(*reason, "guard session" | "reaches a protected origin")));
        assert!(cdp.paused.is_empty());
        assert!(!serde_json::to_string(cdp.events()).unwrap().contains("RAW_INTERCEPTION_PROOF"));
    }
}
