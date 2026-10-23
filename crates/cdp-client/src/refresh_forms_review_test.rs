fn refresh_review_body_denied(value: String) {
    let mut cdp=redirect_body_backend("PRIVATE_REDIRECT_BODY".into());
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","request":{"url":"https://example.test/start","headers":{}}}}));
    cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"R","response":{"url":"https://example.test/start","status":200,"headers":{"Refresh":value}}}}));
    assert!(cdp.call("Network.getResponseBody",json!({"requestId":"R"}),Some("S")).unwrap_err().starts_with("slicc: refused:"));
}

#[test]
fn review_refresh_fractional_delay_refuses_the_private_body() {
    refresh_review_body_denied("0.5; url=https://seven.sliccy.ai/private?code=PRIV".into());
}

#[test]
fn review_refresh_bare_url_refuses_the_private_body() {
    refresh_review_body_denied("0; https://seven.sliccy.ai/private?code=PRIV".into());
}

#[test]
fn review_refresh_trailing_decimal_and_quoted_url_refuses_the_private_body() {
    refresh_review_body_denied("1.; URL='https://seven.sliccy.ai/private?code=PRIV'".into());
}
