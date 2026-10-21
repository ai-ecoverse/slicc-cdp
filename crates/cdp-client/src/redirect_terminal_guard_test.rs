#[test]
fn redirect_terminal_ordinary_guard_observations_do_not_terminate_client_requests() {
    let mut cdp = url_backend("about:blank");
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.session_targets.insert("G".into(), "T".into());
    cdp.guard_sessions.insert("G".into());
    for session in ["S", "G"] {
        cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":session,"params":{"requestId":"ORDINARY-NET","type":"Document","request":{"url":"https://ordinary.invalid/control","method":"GET","headers":{}}}}));
    }
    cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"G","params":{"requestId":"ORDINARY-NET","response":{"url":"https://ordinary.invalid/control","status":200}}}));
    assert_eq!(cdp.events().iter().filter(|event| event["method"] == "Network.loadingFailed").count(), 0);
    assert_eq!(cdp.events().len(), 1);
    cdp.absorb(json!({"method":"Network.loadingFinished","sessionId":"S","params":{"requestId":"ORDINARY-NET","timestamp":3}}));
    assert_eq!(cdp.events().iter().filter(|event| event["method"] == "Network.loadingFinished").count(), 1);
}
