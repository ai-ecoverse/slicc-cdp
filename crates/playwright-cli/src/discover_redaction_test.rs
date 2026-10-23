#[test]
fn protected_discovery_tabs_are_origin_only_even_with_full_urls() {
    crate::sanitize::set_full_urls(true);
    let input = json!([
        {"id":"PRIVATE","type":"page","url":"HTTPS://SEVEN.SLICCY.AI:443/private/DISCOVERY_PROTECTED_SECRET?code=DISCOVERY_PROTECTED_SECRET#DISCOVERY_PROTECTED_SECRET","title":"DISCOVERY_PROTECTED_SECRET title"},
        {"id":"QUERY","type":"page","url":"https://idp.example/authorize?redirect_uri=https://vault.sliccy.ai/oauth/mcp-callback","title":"ordinary login"},
        {"id":"DOC","type":"page","url":"https://example.test/docs?q=ORDINARY_DISCOVERY_CONTROL#visible","title":"ORDINARY_DISCOVERY_CONTROL"},
        {"id":"VAULT","type":"page","url":"https://vault.sliccy.ai/private?code=DISCOVERY_PROTECTED_SECRET#DISCOVERY_PROTECTED_SECRET","title":"DISCOVERY_PROTECTED_SECRET"}
    ]);
    let tabs = parse_tabs(&input.to_string(), &[]).unwrap();
    let seven: Vec<_> = tabs.iter().filter(|tab| tab.seven).collect();
    assert_eq!(seven.len(), 1);
    let protected: Value = serde_json::from_str(&tabs_json(&seven)).unwrap();
    assert_eq!(protected, json!([{"id":"PRIVATE","url":"https://seven.sliccy.ai","title":"https://seven.sliccy.ai"}]));
    let ordinary: Vec<_> = tabs.iter().filter(|tab| !tab.seven).collect();
    let open: Value = serde_json::from_str(&tabs_json(&ordinary)).unwrap();
    assert_eq!(open.as_array().unwrap().len(), 2);
    assert_eq!(open[0]["id"], "QUERY");
    assert_eq!(open[0]["url"], input[1]["url"]);
    assert_eq!(open[1]["url"], input[2]["url"]);
    assert!(!serde_json::to_string(&protected).unwrap().contains("DISCOVERY_PROTECTED_SECRET"));
    crate::sanitize::set_full_urls(false);
    let protected: Value = serde_json::from_str(&tabs_json(&seven)).unwrap();
    assert_eq!(protected[0]["url"], "https://seven.sliccy.ai");
    assert_eq!(protected[0]["title"], "https://seven.sliccy.ai");
}

#[test]
fn protected_discovery_extra_origin_never_exposes_private_metadata() {
    crate::sanitize::set_full_urls(true);
    let input = json!([
        {"id":"EXTRA","type":"page","url":"http://extra.example:8080/private?token=EXTRA_DISCOVERY_SECRET#EXTRA_DISCOVERY_SECRET","title":"EXTRA_DISCOVERY_SECRET"},
        {"id":"SAFE","type":"page","url":"https://ordinary.example/path?ordinary=1#ordinary","title":"ordinary"}
    ]);
    let tabs = parse_tabs(&input.to_string(), &["http://extra.example:8080"]).unwrap();
    assert!(tabs[0].seven);
    assert_eq!(tabs[0].url, "http://extra.example:8080");
    assert_eq!(tabs[0].title, "http://extra.example:8080");
    assert!(!tabs[1].seven);
    assert_eq!(tabs[1].url, input[1]["url"]);
    crate::sanitize::set_full_urls(false);
}
