use cdp_client::protected::{Opener, Protected};

#[test]
fn shared_domain_premise_never_classifies_an_ordinary_origin_or_target() {
    let vectors = slicc_browsers::protected::vectors();
    let origins: Vec<&str> = vectors.origins.iter().map(String::as_str).collect();
    let guard = Protected::new(&origins);
    let cases: Vec<_> = vectors
        .urls
        .iter()
        .filter(|item| item.url.ends_with("/cdp-premise"))
        .collect();
    assert_eq!(cases.len(), 10);
    for item in cases {
        assert!(item.protected, "{}", item.url);
        assert!(guard.url(&item.url), "{}", item.url);
        assert!(guard.target(&item.url, false, Opener::None), "{}", item.url);
    }
}
