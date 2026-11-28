const LISTED: &str = "https://lease-pilot.sliccy.ai";
const LISTED_PATHS: [&str; 4] = [
    "https://lease-pilot.sliccy.ai/",
    "https://lease-pilot.sliccy.ai/os/x",
    "https://lease-pilot.sliccy.ai/auth/callback",
    "https://lease-pilot.sliccy.ai/os/x/y/z/deep/path/index.html",
];

fn listed_guard(kept_origins: Vec<String>) -> crate::protected::Protected {
    let mut rules = slicc_browsers::catalog().protected_origins;
    rules.kept_origins = kept_origins;
    crate::protected::Protected::with_rules(rules, &[])
}

fn refused_as_protected(cdp: &mut Cdp, sent: &std::rc::Rc<std::cell::RefCell<Vec<Value>>>, label: &str) {
    let expected = format!("{}\n", cdp.guard().refusal());
    let before = sent.borrow().len();
    let refused = cdp.call("Target.closeTarget", json!({"targetId":"T"}), None).unwrap_err();
    assert_eq!(refused, expected, "{label}");
    assert!(!sent.borrow()[before..].iter().any(|call| call["method"] == "Target.closeTarget"), "{label}");
}

#[test]
fn a_listed_origin_page_refuses_close_without_a_session_on_every_path() {
    for url in LISTED_PATHS {
        let (mut cdp, sent, _, _) = close_probe(url);
        assert!(cdp.guard().kept_origin(url), "{url}");
        refused_as_protected(&mut cdp, &sent, url);
        refused_as_protected(&mut cdp, &sent, &format!("{url} again"));
    }
}

#[test]
fn a_listed_origin_latch_holds_after_a_later_navigation() {
    let start = "https://example.test/start";
    let start_info = json!({"targetId":"T","type":"page","url":start,"title":"ordinary"});
    for url in LISTED_PATHS {
        let (mut cdp, sent, frame, frame_mode, _, _, _, info_queue) = close_probe_queue(start);
        cdp.session_targets.insert("S".into(), "T".into());
        cdp.guards.insert("T".into(), "S".into());
        cdp.guard_sessions.insert("S".into());
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":url,"title":"SECRETTITLE"}}}));
        assert!(cdp.kept_url_latch.contains("T"), "{url}");
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L1"}}}));
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":start_info.clone()}}));
        assert!(cdp.kept_url_latch.contains("T"), "{url} after navigation");
        info_queue.borrow_mut().push_back(start_info.clone());
        info_queue.borrow_mut().push_back(start_info.clone());
        *frame.borrow_mut() = json!({"id":"F","url":start,"securityOrigin":"https://example.test","loaderId":"L1"});
        *frame_mode.borrow_mut() = ProbeMode::Timeout;
        refused_as_protected(&mut cdp, &sent, url);
        assert!(cdp.kept_url_latch.contains("T"), "{url} after close");
    }
}

#[test]
fn a_listed_origin_page_is_not_a_closable_tab_and_its_verdict_is_kept() {
    for url in LISTED_PATHS {
        let (mut cdp, _, info, _) = close_probe(url);
        assert_eq!(cdp.closable_tab("T"), Some(false), "{url}");
        let info = info.borrow().clone();
        assert!(cdp.targets.verdicts(&info, &cdp.guard).kept, "{url}");
        assert!(super::kept_document(&cdp.guard, url), "{url}");
    }
    for url in ["https://seven.sliccy.ai/", "https://www.sliccy.ai/", "https://example.test/"] {
        let (mut cdp, _, info, _) = close_probe(url);
        assert_eq!(cdp.closable_tab("T"), Some(true), "{url}");
        let info = info.borrow().clone();
        assert!(!cdp.targets.verdicts(&info, &cdp.guard).kept, "{url}");
    }
}

#[test]
fn close_controls_beside_a_listed_origin_are_unchanged() {
    for url in ["https://seven.sliccy.ai/", "https://www.sliccy.ai/", "https://example.test/"] {
        let (mut cdp, sent, _, _) = close_probe(url);
        allow_close(&mut cdp, &sent, url);
    }
    for url in ["https://seven.sliccy.ai/os/x", "https://vault.sliccy.ai/"] {
        let (mut cdp, sent, _, _) = close_probe(url);
        refused_as_protected(&mut cdp, &sent, url);
    }
    for url in ["https://lease-pilot.sliccy.ai:8443/", "http://lease-pilot.sliccy.ai/", "https://a.lease-pilot.sliccy.ai/"] {
        let (mut cdp, sent, _, _) = close_probe(url);
        assert!(!cdp.guard().kept_origin(url), "{url}");
        assert!(!super::kept_document(&cdp.guard, url), "{url}");
        allow_close(&mut cdp, &sent, url);
    }
}

#[test]
fn kept_document_follows_the_catalog_list() {
    let listed = listed_guard(vec![LISTED.into()]);
    let cleared = listed_guard(Vec::new());
    for url in LISTED_PATHS {
        assert!(super::kept_document(&listed, url), "{url}");
        assert!(!super::kept_document(&cleared, url), "{url}");
    }
    for url in ["blob:https://lease-pilot.sliccy.ai/x", "view-source:https://lease-pilot.sliccy.ai/", "https://user:secret@lease-pilot.sliccy.ai/", "https://LEASE-PILOT.SLICCY.AI./"] {
        assert!(super::kept_document(&listed, url), "{url}");
        assert!(!super::kept_document(&cleared, url), "{url}");
    }
    assert!(super::kept_security_origin(&listed, "about:blank", LISTED));
    assert!(!super::kept_security_origin(&cleared, "about:blank", LISTED));
    let ignored = listed_guard(vec!["http://lease-pilot.sliccy.ai".into(), "https://lease-pilot.sliccy.ai:8443".into(), "not an origin".into()]);
    for url in LISTED_PATHS.iter().copied().chain(["http://lease-pilot.sliccy.ai/", "https://lease-pilot.sliccy.ai:8443/"]) {
        assert!(!super::kept_document(&ignored, url), "{url}");
    }
    assert!(!super::kept_security_origin(&ignored, "about:blank", LISTED));
}

#[test]
fn a_listed_origin_blank_is_kept_through_its_security_origin() {
    let guard = crate::protected::Protected::default();
    for url in ["about:blank", "about:srcdoc", ""] {
        assert!(super::kept_security_origin(&guard, url, LISTED), "{url:?}");
        assert!(guard.kept_target(url, Some(LISTED)), "{url:?}");
    }
    for opener in [None, Some("https://example.test/start"), Some("https://vault.sliccy.ai/app")] {
        let (mut cdp, sent, info, frame, _, _, _, _, _, opened_by) = close_probe_modes("about:blank");
        let mut blank = json!({"targetId":"T","type":"page","url":"about:blank","title":""});
        if let Some(url) = opener {
            *opened_by.borrow_mut() = Some(json!({"targetId":"O","type":"page","url":url,"title":"ordinary"}));
            blank["openerId"] = json!("O");
        }
        *info.borrow_mut() = blank;
        *frame.borrow_mut() = json!({"id":"F","url":"about:blank","securityOrigin":LISTED,"loaderId":"L"});
        refused_as_protected(&mut cdp, &sent, &format!("blank opener {opener:?}"));
    }
    let (mut cdp, sent, info, frame) = close_probe("about:blank");
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"type":"BackForwardCacheRestore","frame":{"id":"F","url":"about:blank","securityOrigin":LISTED,"loaderId":"L0"}}}));
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":"about:blank","title":""});
    *frame.borrow_mut() = json!({"id":"F","url":"about:blank","securityOrigin":LISTED,"loaderId":"L0"});
    refused_as_protected(&mut cdp, &sent, "restored blank");
}

#[test]
fn a_blank_without_a_listed_origin_is_not_kept_by_the_list() {
    let guard = crate::protected::Protected::default();
    for origin in ["://", "null", "https://lease-pilot.sliccy.ai:8443", "https://seven.sliccy.ai"] {
        assert!(!guard.kept_target("about:blank", Some(origin)), "{origin}");
        assert!(!super::kept_document(&guard, origin), "{origin}");
    }
    for origin in ["://", "null", "https://lease-pilot.sliccy.ai:8443"] {
        assert!(!super::kept_security_origin(&guard, "about:blank", origin), "{origin}");
    }
    assert!(super::kept_security_origin(&guard, "about:blank", "https://seven.sliccy.ai"));
    assert!(super::kept_security_origin(&listed_guard(Vec::new()), "about:blank", "https://seven.sliccy.ai"));
    assert!(!guard.kept_target("about:blank", None));
    assert!(!super::kept_document(&guard, "about:blank"));
    assert!(!super::kept_security_origin(&guard, "about:blank", ""));
    for origin in ["://", "https://lease-pilot.sliccy.ai:8443"] {
        let (mut cdp, sent, info, frame) = close_probe("about:blank");
        *info.borrow_mut() = json!({"targetId":"T","type":"page","url":"about:blank","title":""});
        *frame.borrow_mut() = json!({"id":"F","url":"about:blank","securityOrigin":origin,"loaderId":"L"});
        allow_close(&mut cdp, &sent, origin);
    }
}

#[test]
fn an_unparseable_close_url_stays_kept_and_an_empty_one_does_not() {
    let guard = crate::protected::Protected::default();
    let unparseable = "https://lease-pilot.sliccy.ai:99999/";
    assert!(!guard.kept_origin(unparseable));
    assert!(super::kept_document(&guard, unparseable));
    assert!(super::kept_document(&listed_guard(Vec::new()), unparseable));
    for text in ["", "null", "://"] {
        assert!(!super::kept_document(&guard, text), "{text:?}");
    }
    let (mut cdp, sent, _, _) = close_probe(unparseable);
    let before = sent.borrow().len();
    let refused = cdp.call("Target.closeTarget", json!({"targetId":"T"}), None).unwrap_err();
    assert!(refused.starts_with("slicc: refused:"), "{refused}");
    assert!(!sent.borrow()[before..].iter().any(|call| call["method"] == "Target.closeTarget"));
}

#[test]
fn wrappers_of_a_listed_origin_stay_kept() {
    let guard = crate::protected::Protected::default();
    for url in ["blob:https://lease-pilot.sliccy.ai/x", "view-source:https://lease-pilot.sliccy.ai/", "filesystem:https://lease-pilot.sliccy.ai/temporary/x"] {
        assert!(!guard.kept_origin(url), "{url}");
        assert!(super::kept_document(&guard, url), "{url}");
        let (mut cdp, sent, _, _) = close_probe(url);
        refused_as_protected(&mut cdp, &sent, url);
    }
}

#[test]
fn a_document_running_in_a_listed_origin_is_kept_by_its_security_origin() {
    let guard = crate::protected::Protected::default();
    assert!(!guard.kept_target("https://seven.sliccy.ai/", Some(LISTED)));
    assert!(super::kept_security_origin(&guard, "https://seven.sliccy.ai/", LISTED));
    assert!(super::kept_security_origin(&guard, "https://example.test/", LISTED));
    assert!(!super::kept_security_origin(&listed_guard(Vec::new()), "https://example.test/", LISTED));
}

#[test]
fn more_than_one_trailing_dot_on_a_listed_origin_is_not_kept() {
    let guard = crate::protected::Protected::default();
    for url in ["https://lease-pilot.sliccy.ai../", "https://lease-pilot.sliccy.ai.../", "https://lease-pilot.sliccy.ai../os/x", "blob:https://lease-pilot.sliccy.ai../x"] {
        assert!(!guard.kept_origin(url), "{url}");
        assert!(!super::kept_document(&guard, url), "{url}");
        assert!(!guard.url(url), "{url}");
        let (mut cdp, sent, _, _) = close_probe(url);
        allow_close(&mut cdp, &sent, url);
    }
    for url in ["https://lease-pilot.sliccy.ai./", "https://LEASE-PILOT.SLICCY.AI./os/x"] {
        assert!(guard.kept_origin(url), "{url}");
        assert!(super::kept_document(&guard, url), "{url}");
        assert!(guard.url(url), "{url}");
        let (mut cdp, sent, _, _) = close_probe(url);
        refused_as_protected(&mut cdp, &sent, url);
    }
    for url in ["https://vault.sliccy.ai../", "https://vault.sliccy.ai.../app", "https://seven.sliccy.ai../os/x", "https://seven.sliccy.ai.../os/"] {
        assert!(super::kept_document(&guard, url), "{url}");
        assert!(super::kept_document(&listed_guard(Vec::new()), url), "{url}");
    }
    for url in ["https://seven.sliccy.ai../", "https://www.sliccy.ai.../"] {
        assert!(!super::kept_document(&guard, url), "{url}");
    }
}

#[test]
fn a_blank_opened_by_a_listed_origin_page_is_kept_like_a_vault_opener() {
    for (opener, kept) in [
        ("https://lease-pilot.sliccy.ai/", true),
        ("https://lease-pilot.sliccy.ai/os/x/y/z/deep/path/index.html", true),
        ("https://example.test/start", false),
        ("https://vault.sliccy.ai/app", true),
    ] {
        let (mut cdp, sent, info, frame, _, _, _, _, _, opened_by) = close_probe_modes("about:blank");
        *opened_by.borrow_mut() = Some(json!({"targetId":"O","type":"page","url":opener,"title":"ordinary"}));
        *info.borrow_mut() = json!({"targetId":"T","type":"page","url":"about:blank","title":"","openerId":"O"});
        *frame.borrow_mut() = json!({"id":"F","url":"about:blank","securityOrigin":"://","loaderId":"L"});
        if kept {
            refused_as_protected(&mut cdp, &sent, opener);
        } else {
            allow_close(&mut cdp, &sent, opener);
        }
    }
    let (mut cdp, sent, info, frame, _, _, _, _, _, opened_by) = close_probe_modes("about:blank");
    cdp.guard = listed_guard(Vec::new());
    *opened_by.borrow_mut() = Some(json!({"targetId":"O","type":"page","url":"https://lease-pilot.sliccy.ai/","title":"ordinary"}));
    *info.borrow_mut() = json!({"targetId":"T","type":"page","url":"about:blank","title":"","openerId":"O"});
    *frame.borrow_mut() = json!({"id":"F","url":"about:blank","securityOrigin":"://","loaderId":"L"});
    allow_close(&mut cdp, &sent, "listed opener with the list cleared");
    for (url, origin, kept) in [
        ("https://lease-pilot.sliccy.ai/", LISTED, true),
        ("https://lease-pilot.sliccy.ai/auth/callback", "://", true),
        ("about:blank", LISTED, true),
        ("https://example.test/start", "https://example.test", false),
        ("https://vault.sliccy.ai/app", "https://vault.sliccy.ai", true),
    ] {
        let (mut cdp, sent, info, frame) = close_probe("about:blank");
        *info.borrow_mut() = json!({"targetId":"T","type":"page","url":"about:blank","title":"","openerFrameId":"OF"});
        *frame.borrow_mut() = json!({"id":"F","url":"about:blank","securityOrigin":"://","loaderId":"L"});
        cdp.frames.insert("OF".into(), vec![url.to_string(), origin.to_string()]);
        assert_eq!(cdp.frame_matches_kept_rule("OF"), kept, "{url} {origin}");
        if kept {
            refused_as_protected(&mut cdp, &sent, &format!("frame {url} {origin}"));
        } else {
            allow_close(&mut cdp, &sent, &format!("frame {url} {origin}"));
        }
    }
    let (cdp, _, _, _) = close_probe("about:blank");
    let mut cdp = cdp;
    cdp.guard = listed_guard(Vec::new());
    cdp.frames.insert("OF".into(), vec!["https://lease-pilot.sliccy.ai/".into(), LISTED.into()]);
    assert!(!cdp.frame_matches_kept_rule("OF"));
}
