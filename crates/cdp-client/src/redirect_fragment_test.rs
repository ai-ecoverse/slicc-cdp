#[test]
fn protected_url_sibling_fragments_and_document_url_are_removed() {
    for host in ["seven", "vault"] {
        for marker in marker_forms() {
            let mut cdp = url_backend("about:blank");
            let reply = json!({"request":{"url":format!("https://{host}.sliccy.ai/auth/callback"),"urlFragment":format!("#access_token={marker}"),"alternateFragment":marker,"documentURL":format!("https://example.test/?token={marker}")}});
            let result = cdp.after("Runtime.evaluate", &json!({}), Some("S"), reply).unwrap();
            assert_eq!(result["request"]["url"], format!("https://{host}.sliccy.ai"));
            for key in ["urlFragment", "alternateFragment", "documentURL"] {
                assert!(result["request"].get(key).is_none(), "{key}: {result}");
            }
            cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","request":{"url":format!("https://{host}.sliccy.ai/auth/callback"),"urlFragment":format!("#access_token={marker}")},"documentURL":format!("https://{host}.sliccy.ai/auth/callback")}}));
            assert!(!format!("{:?}", cdp.events()).contains(&marker));
        }
    }
}
