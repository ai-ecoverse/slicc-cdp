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

fn default_context_marks(origin: &str, aux: Value) -> bool {
    let created = json!({"method":"Runtime.executionContextCreated","sessionId":"S","params":{"context":{"id":1,"origin":origin,"name":"","uniqueId":"U","auxData":aux}}});
    let mut cdp = Cdp::from_transport(Box::new(Pending { inbox: VecDeque::from([created.to_string()]), answer: |_| Vec::new() }));
    cdp.fixture_session("S");
    while cdp.pump(Instant::now()).unwrap() {}
    cdp.is_protected_target("fixture:S")
}

#[test]
fn a_default_frame_context_whose_origin_is_an_about_value_other_than_about_slash_slash_marks_the_tab_protected() {
    let frame = || json!({"isDefault":true,"type":"default","frameId":"F"});
    for origin in ["ABOUT://", "about://x", "about:blank", "ABOUT:BLANK", "About:srcdoc", "about:"] {
        assert!(default_context_marks(origin, frame()), "{origin}");
    }
    for origin in ["about://", "://", "https://example.test", "xabout:blank", "https://about.example", "", "null"] {
        assert!(!default_context_marks(origin, frame()), "{origin}");
    }
}

#[test]
fn an_about_origin_on_a_worker_isolated_world_or_frameless_context_marks_only_what_it_marked_before() {
    let guard = crate::protected::Protected::default();
    for aux in [
        json!({"isDefault":true}),
        json!({"isDefault":true,"frameId":""}),
        json!({"isDefault":true,"type":"worker"}),
        json!({"isDefault":false,"type":"isolated","frameId":"F"}),
        json!({"frameId":"F"}),
        json!({"isDefault":"true","frameId":"F"}),
        json!({"isDefault":1,"frameId":"F"}),
        json!({"isDefault":true,"frameId":7}),
    ] {
        let before = aux.get("isDefault").and_then(Value::as_bool).unwrap_or(true);
        for origin in ["about:blank", "ABOUT:BLANK", "ABOUT://", "about://x", "about://"] {
            assert_eq!(default_context_marks(origin, aux.clone()), before && guard.url(origin), "{aux} {origin}");
        }
        assert!(!default_context_marks("about:blank", aux.clone()), "{aux}");
    }
}

#[test]
fn the_about_rule_and_the_about_slash_slash_rewrite_read_the_same_default_frame_context() {
    for aux in [
        json!({"isDefault":true,"frameId":"F"}),
        json!({"isDefault":true,"type":"default","frameId":"F"}),
        json!({"isDefault":true}),
        json!({"isDefault":true,"frameId":""}),
        json!({"isDefault":true,"frameId":7}),
        json!({"isDefault":false,"frameId":"F"}),
        json!({"isDefault":"true","frameId":"F"}),
        json!({"frameId":"F"}),
        json!({}),
    ] {
        let context = json!({"id":1,"origin":"about://","name":"","uniqueId":"U","auxData":aux});
        let text = json!({"method":"Runtime.executionContextCreated","sessionId":"S","params":{"context":context}}).to_string();
        assert_eq!(super::opaque_blank_origin(text.clone()) != text, super::default_frame_context(Some(&context)), "{aux}");
    }
    assert!(!super::default_frame_context(None));
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
