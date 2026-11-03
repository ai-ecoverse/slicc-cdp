mod native_origins_contract {
    use super::*;

    fn inspector(native: bool) -> (Cdp, Rc<RefCell<Vec<Value>>>) {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let cdp = Cdp::from_transport(Box::new(FrameInspector { queue: VecDeque::new(), calls: calls.clone(), next: 0, sessions: BTreeMap::new(), children: false, isolated: false, native }));
        (cdp, calls)
    }

    fn sent_origins(calls: &[Value]) -> Vec<Value> {
        calls.iter().filter_map(|command| {
            let expression = command["params"]["expression"].as_str()?;
            let rest = expression.split_once("api.getTools({fromOrigins:")?.1;
            serde_json::from_str(rest.split_once("});")?.0).ok()
        }).collect()
    }

    #[test]
    fn native_list_and_execute_pass_exact_requested_origins_without_polyfill_warning() {
        let (mut cdp, calls) = inspector(true);
        let mut state = Session::default();
        let from = vec!["https://ordinary-child.test".to_string(), "https://second-child.test".to_string()];
        let listed = crate::webmcp::list_from(&mut cdp, &mut state, Some("ROOT"), true, &from);
        assert_eq!(listed.code, 0, "{}", listed.stderr);
        assert!(listed.stdout.contains("add"));
        let called = crate::webmcp::call_from(&mut cdp, &mut state, "add", "{\"a\":2,\"b\":3}", Some("ROOT"), false, &from, None);
        assert_eq!(called.code, 0, "{}", called.stderr);
        assert_eq!(called.stdout.trim(), "5");
        let origins = sent_origins(&calls.borrow());
        assert_eq!(origins.len(), 3);
        assert!(origins.iter().all(|actual| actual == &json!(from)), "{origins:?}");
        assert!(!listed.stderr.contains("cross-origin child tools are unavailable"));
        assert!(!called.stderr.contains("cross-origin child tools are unavailable"));
    }

    #[test]
    fn polyfill_clears_requested_origins_but_preserves_root_and_clear_message() {
        let (mut cdp, calls) = inspector(false);
        let mut state = Session::default();
        let from = vec!["https://ordinary-child.test".to_string()];
        let listed = crate::webmcp::list_from(&mut cdp, &mut state, Some("ROOT"), true, &from);
        let called = crate::webmcp::call_from(&mut cdp, &mut state, "add", "{\"a\":2,\"b\":3}", Some("ROOT"), false, &from, None);
        assert_eq!(listed.code, 0);
        assert!(listed.stdout.contains("add"));
        assert_eq!(called.code, 0);
        assert_eq!(called.stdout.trim(), "5");
        assert!(listed.stderr.contains("cross-origin child tools are unavailable"));
        assert!(called.stderr.contains("cross-origin child tools are unavailable"));
        assert!(sent_origins(&calls.borrow()).is_empty());
        assert!(!calls.borrow().iter().any(|command| command["params"]["expression"].as_str().is_some_and(|expression| expression.contains(&from[0]))));
    }

    #[test]
    fn missing_requested_tool_reports_the_limit_only_after_polyfill_mode_detection() {
        for native in [false, true] {
            let (mut cdp, calls) = inspector(native);
            let mut state = Session::default();
            let from = vec!["https://ordinary-child.test".to_string()];
            let called = crate::webmcp::call_from(&mut cdp, &mut state, "foreign", "{}", Some("ROOT"), false, &from, None);
            assert_eq!(called.code, 2, "{}", called.stderr);
            assert_eq!(called.stdout, "");
            assert!(called.stderr.contains("unknown tool \"foreign\""));
            assert_eq!(called.stderr.contains("cross-origin child tools are unavailable"), !native);
            if !native {
                assert!(sent_origins(&calls.borrow()).is_empty());
                assert!(!calls.borrow().iter().any(|command| command["params"]["expression"].as_str().is_some_and(|expression| expression.contains(&from[0]))));
            }
        }
    }

    #[test]
    fn protected_origin_cli_refusal_has_no_polyfill_limit_message() {
        for native in [false, true] {
            let (mut cdp, calls) = inspector(native);
            let mut state = Session::default();
            let from = vec!["https://vault.sliccy.ai".to_string()];
            let listed = crate::webmcp::list_from(&mut cdp, &mut state, Some("ROOT"), true, &from);
            let called = crate::webmcp::call_from(&mut cdp, &mut state, "foreign", "{}", Some("ROOT"), false, &from, None);
            for output in [listed, called] {
                assert_ne!(output.code, 0);
                assert!(output.stderr.starts_with("slicc: refused:"), "{}", output.stderr);
                assert!(!output.stderr.contains("cross-origin child tools are unavailable"));
            }
            assert!(!calls.borrow().iter().any(|command| command["params"]["expression"] == "'modelContext' in Document.prototype"));
        }
    }

    #[test]
    fn protected_requested_origin_refuses_before_mode_detection_on_both_paths() {
        for native in [false, true] {
            let (mut cdp, calls) = inspector(native);
            let session = cdp.call("Target.attachToTarget", json!({"targetId":"ROOT","flatten":true}), None).unwrap()["sessionId"].as_str().unwrap().to_string();
            let before = calls.borrow().len();
            let from = vec!["https://ordinary-child.test".to_string(), "https://vault.sliccy.ai".to_string()];
            let state = Session::default();
            assert!(list(&mut cdp, &state, "ROOT", &session, &from).unwrap_err().contains("slicc: refused:"));
            assert_eq!(calls.borrow().len(), before);
            assert!(call(&mut cdp, &state, "ROOT", &session, &from, None, "add", &json!({"a":2,"b":3}), false).unwrap_err().contains("slicc: refused:"));
            assert_eq!(calls.borrow().len(), before);
        }
    }
}
