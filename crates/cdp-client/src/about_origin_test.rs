#[test]
fn an_about_context_origin_reads_as_the_unknown_blank_origin_and_nothing_else_changes() {
    let created = |origin: &str| json!({"method":"Runtime.executionContextCreated","sessionId":"S","params":{"context":{"id":1,"origin":origin,"name":"","uniqueId":"U","auxData":{"isDefault":true,"frameId":"F"}}}}).to_string();
    let origin = |text: String| serde_json::from_str::<Value>(&text).unwrap().pointer("/params/context/origin").cloned().unwrap();
    assert_eq!(origin(super::opaque_blank_origin(created("about://"))), json!("://"));
    for kept in ["://", "", "null", "about://settings", "about:blank", "about:", "ABOUT://", "chrome://", "https://seven.sliccy.ai", "https://example.test"] {
        assert_eq!(origin(super::opaque_blank_origin(created(kept))), json!(kept), "{kept}");
    }
    for text in [
        json!({"method":"Page.frameNavigated","params":{"frame":{"id":"F","url":"about://","securityOrigin":"about://"}}}).to_string(),
        json!({"id":4,"result":{"context":{"origin":"about://"},"method":"Runtime.executionContextCreated"}}).to_string(),
        json!({"method":"Runtime.executionContextCreated","params":{"context":{"id":1,"origin":7,"note":"about://"}}}).to_string(),
        "not json Runtime.executionContextCreated about://".to_string(),
    ] {
        assert_eq!(super::opaque_blank_origin(text.clone()), text);
    }
    let guard = crate::protected::Protected::default();
    assert!(guard.url("about://") && !guard.url("://"));
}

#[test]
fn an_about_origin_on_a_worker_or_isolated_world_context_is_left_as_reported() {
    let context = |aux: Value| json!({"method":"Runtime.executionContextCreated","sessionId":"W","params":{"context":{"id":1,"origin":"about://","name":"","uniqueId":"U","auxData":aux}}}).to_string();
    for aux in [
        json!({"isDefault":true}),
        json!({"isDefault":true,"frameId":""}),
        json!({"isDefault":true,"type":"worker"}),
        json!({"isDefault":false,"type":"isolated","frameId":"F"}),
        json!({"frameId":"F"}),
        json!({"isDefault":"true","frameId":"F"}),
    ] {
        let text = context(aux.clone());
        assert_eq!(super::opaque_blank_origin(text.clone()), text, "{aux}");
    }
}

#[test]
fn received_contexts_keep_worker_and_isolated_about_origins_and_map_only_the_default_frame_one() {
    let mut cdp = pending(|_| [
        ("S", "default", json!({"isDefault":true,"type":"default","frameId":"F"})),
        ("S", "isolated", json!({"isDefault":false,"type":"isolated","frameId":"F"})),
        ("W", "worker", json!({"isDefault":true})),
    ].into_iter().map(|(session, unique, aux)| json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":1,"origin":"about://","name":"","uniqueId":unique,"auxData":aux}}})).collect());
    cdp.ws.send_text("{}").unwrap();
    while cdp.pump(Instant::now()).unwrap() {}
    let origin = |cdp: &Cdp, session: &str, unique: &str| cdp.context_proofs.get(&(session.to_string(), unique.to_string())).map(|proof| proof.origin.clone());
    assert_eq!(origin(&cdp, "S", "default").as_deref(), Some("://"));
    assert_eq!(origin(&cdp, "S", "isolated").as_deref(), Some("about://"));
    assert_eq!(origin(&cdp, "W", "worker").as_deref(), Some("about://"));
    assert!(cdp.guard.url("about://"));
}

#[test]
fn the_target_classifier_reads_only_an_about_context_origin_on_an_inheriting_document() {
    use crate::protected::Opener;
    let guard = crate::protected::Protected::default();
    let judged = |url: &str, opener: Opener, origin: Option<&str>| guard.target_with_document(url, false, opener, None, None, false, origin);
    for origin in ["ABOUT://", "about://x", "about:blank", "About:Srcdoc", "about:"] {
        assert!(judged("about:blank", Opener::None, Some(origin)), "{origin}");
        assert!(judged("", Opener::Unprotected, Some(origin)), "{origin}");
        assert!(!judged("https://example.test/", Opener::None, Some(origin)), "{origin}");
    }
    for origin in [None, Some("://"), Some("about://"), Some("https://example.test"), Some("xabout:blank"), Some("https://about.example")] {
        assert!(!judged("about:blank", Opener::None, origin), "{origin:?}");
        assert!(!judged("about:blank", Opener::Unprotected, origin), "{origin:?}");
        assert!(judged("about:blank", Opener::Protected, origin), "{origin:?}");
        assert!(judged("about:blank", Opener::Unknown, origin), "{origin:?}");
    }
}
