const CARRIER_MARK: &str = "FRAME_CARRIER_MARK";
const ORDINARY: &str = "https://ordinary.example/page";
const ORDINARY_ORIGIN: &str = "https://ordinary.example";
const VAULT: &str = "https://vault.sliccy.ai/private/FRAME_CARRIER_MARK";
const VAULT_ORIGIN: &str = "https://vault.sliccy.ai";

struct CarrierState {
    tree: Value,
    verify_tree: Option<Value>,
    resources: Value,
    history: Value,
    inbox: VecDeque<String>,
    tree_serves: usize,
    fail_tree_after: Option<usize>,
}

struct CarrierTransport {
    state: Rc<RefCell<CarrierState>>,
}

impl Transport for CarrierTransport {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut state = self.state.borrow_mut();
        if call["method"] == "Page.getFrameTree" {
            state.tree_serves += 1;
            if state.fail_tree_after == Some(state.tree_serves) {
                state.inbox.push_back(
                    json!({
                        "id": call["id"],
                        "sessionId": call.get("sessionId"),
                        "error": { "message": "Page.getFrameTree timed out" },
                    })
                    .to_string(),
                );
                return Ok(());
            }
        }
        let result = match call["method"].as_str().unwrap_or("") {
            "Page.getFrameTree" => {
                let tree = if state.tree_serves > 1 {
                    state
                        .verify_tree
                        .clone()
                        .unwrap_or_else(|| state.tree.clone())
                } else {
                    state.tree.clone()
                };
                json!({ "frameTree": tree })
            }
            "Page.getResourceTree" => {
                state.tree_serves += 1;
                json!({ "frameTree": state.resources.clone() })
            }
            "Page.getNavigationHistory" => state.history.clone(),
            _ => json!({}),
        };
        state.inbox.push_back(
            json!({
                "id": call["id"],
                "sessionId": call.get("sessionId"),
                "result": result,
            })
            .to_string(),
        );
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.state.borrow_mut().inbox.pop_front())
    }
}

fn frame(id: &str, url: &str, origin: &str, name: &str, loader: &str) -> Value {
    json!({
        "id": id,
        "loaderId": loader,
        "url": url,
        "securityOrigin": origin,
        "mimeType": "text/html",
        "name": name,
    })
}

fn ordinary_tree_with_named_iframe() -> Value {
    json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "", "L0"),
        "childFrames": [{
            "frame": frame("IFRAME", "https://ordinary.example/embed", ORDINARY_ORIGIN, "playwright-frame", "L1"),
        }],
    })
}

fn carrier_cdp(tree: Value) -> (Cdp, Rc<RefCell<CarrierState>>) {
    let resources = tree.clone();
    let state = Rc::new(RefCell::new(CarrierState {
        tree,
        verify_tree: None,
        resources,
        history: json!({
            "currentIndex": 0,
            "entries": [{
                "id": 1,
                "url": ORDINARY,
                "userTypedURL": ORDINARY,
                "title": "ordinary",
                "transitionType": "typed",
            }],
        }),
        inbox: VecDeque::new(),
        tree_serves: 0,
        fail_tree_after: None,
    }));
    let mut cdp = Cdp::from_transport(Box::new(CarrierTransport {
        state: state.clone(),
    }));
    cdp.session_targets.insert("S".into(), "T".into());
    cdp.absorb(json!({
        "method": "Target.targetInfoChanged",
        "params": {
            "targetInfo": {
                "targetId": "T",
                "type": "page",
                "url": ORDINARY,
                "title": "ordinary",
            }
        }
    }));
    cdp.absorb(json!({
        "method": "Page.frameNavigated",
        "sessionId": "S",
        "params": {
            "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "", "L0"),
        }
    }));
    (cdp, state)
}

fn assert_refused(result: Result<Value, String>) {
    assert!(
        result
            .as_ref()
            .err()
            .is_some_and(|error| error.starts_with("slicc: refused:")),
        "{result:?}"
    );
}

#[test]
fn frame_carrier_get_frame_tree_https_protected_refuses() {
    let tree = json!({
        "frame": frame("MAIN", VAULT, VAULT_ORIGIN, CARRIER_MARK, "L0"),
    });
    let (mut cdp, _) = carrier_cdp(tree);
    assert_refused(cdp.call("Page.getFrameTree", json!({}), Some("S")));
    assert!(cdp.is_protected_target("T"));
    assert!(cdp.session_blocked("S"));
}

#[test]
fn frame_carrier_get_resource_tree_https_protected_refuses() {
    let tree = json!({
        "frame": frame("MAIN", VAULT, VAULT_ORIGIN, CARRIER_MARK, "L0"),
        "resources": [{
            "url": VAULT,
            "type": "Document",
            "mimeType": "text/html",
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree.clone());
    state.borrow_mut().resources = tree;
    assert_refused(cdp.call("Page.getResourceTree", json!({}), Some("S")));
}

#[test]
fn frame_carrier_get_frame_tree_wrapper_refuses() {
    let wrapped = format!("view-source:{VAULT}");
    let tree = json!({
        "frame": frame("MAIN", &wrapped, "://", CARRIER_MARK, "L0"),
    });
    let (mut cdp, _) = carrier_cdp(tree);
    assert_refused(cdp.call("Page.getFrameTree", json!({}), Some("S")));
}

#[test]
fn frame_carrier_blank_child_of_protected_origin_refuses() {
    let tree = json!({
        "frame": frame("MAIN", VAULT, VAULT_ORIGIN, "", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "about:blank", VAULT_ORIGIN, CARRIER_MARK, "L1"),
        }],
    });
    let (mut cdp, _) = carrier_cdp(tree);
    assert_refused(cdp.call("Page.getFrameTree", json!({}), Some("S")));
}

#[test]
fn frame_carrier_opaque_origin_child_of_protected_document_refuses() {
    let tree = json!({
        "frame": frame("MAIN", VAULT, VAULT_ORIGIN, "", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "about:blank", "://", CARRIER_MARK, "L1"),
        }],
    });
    let (mut cdp, _) = carrier_cdp(tree);
    assert_refused(cdp.call("Page.getFrameTree", json!({}), Some("S")));
}

#[test]
fn frame_carrier_data_wrapper_refuses() {
    let wrapped = format!("data:text/html,{VAULT}");
    let tree = json!({
        "frame": frame("MAIN", &wrapped, "null", CARRIER_MARK, "L0"),
    });
    let (mut cdp, _) = carrier_cdp(tree);
    assert_refused(cdp.call("Page.getFrameTree", json!({}), Some("S")));
}

#[test]
fn frame_carrier_get_navigation_history_drops_non_url_on_protected_entry() {
    let (mut cdp, state) = carrier_cdp(ordinary_tree_with_named_iframe());
    state.borrow_mut().history = json!({
        "currentIndex": 0,
        "entries": [{
            "id": 1,
            "url": VAULT,
            "userTypedURL": VAULT,
            "title": CARRIER_MARK,
            "transitionType": CARRIER_MARK,
        }],
    });
    let result = cdp
        .call("Page.getNavigationHistory", json!({}), Some("S"))
        .unwrap();
    let entry = &result["entries"][0];
    assert_ne!(entry["url"], VAULT);
    assert_eq!(entry["title"], "");
    assert_eq!(entry["transitionType"], "");
    assert!(!serde_json::to_string(entry).unwrap().contains(CARRIER_MARK));
}

#[test]
fn frame_carrier_ordinary_named_iframe_keeps_names_with_zero_extra_reads() {
    let (mut cdp, state) = carrier_cdp(ordinary_tree_with_named_iframe());
    state.borrow_mut().tree_serves = 0;
    let result = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(
        result["frameTree"]["childFrames"][0]["frame"]["name"],
        "playwright-frame"
    );
    assert_eq!(state.borrow().tree_serves, 1);
    assert_eq!(cdp.frame_carrier_tree_reads, 0);
}

#[test]
fn frame_carrier_blank_child_with_parent_origin_keeps_name_with_zero_reads() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main-name", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "about:blank", ORDINARY_ORIGIN, "blank-name", "L1"),
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().tree_serves = 0;
    let result = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 0);
    assert_eq!(state.borrow().tree_serves, 1);
    assert_eq!(result["frameTree"]["frame"]["name"], "main-name");
    assert_eq!(result["frameTree"]["childFrames"][0]["frame"]["name"], "blank-name");
}

#[test]
fn frame_carrier_srcdoc_child_with_parent_origin_keeps_name_with_zero_reads() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main-name", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "about:srcdoc", ORDINARY_ORIGIN, "srcdoc-name", "L1"),
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().tree_serves = 0;
    let result = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 0);
    assert_eq!(state.borrow().tree_serves, 1);
    assert_eq!(result["frameTree"]["childFrames"][0]["frame"]["name"], "srcdoc-name");
}

#[test]
fn frame_carrier_opaque_origin_blank_child_measures_one_extra_read_and_blanks_name() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main-name", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "about:blank", "://", "sandboxed-blank", "L1"),
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().tree_serves = 0;
    let result = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 1);
    assert_eq!(state.borrow().tree_serves, 2);
    assert_eq!(result["frameTree"]["childFrames"][0]["frame"]["name"], "");
}

#[test]
fn frame_carrier_opaque_origin_non_blank_measures_one_extra_read() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main-name", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "https://ordinary.example/embed", "://", "opaque-name", "L1"),
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().tree_serves = 0;
    let result = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 1);
    assert_eq!(state.borrow().tree_serves, 2);
    assert_eq!(result["frameTree"]["childFrames"][0]["frame"]["name"], "");
}

#[test]
fn frame_carrier_event_wrapper_frame_with_name_is_dropped() {
    let (mut cdp, _) = carrier_cdp(ordinary_tree_with_named_iframe());
    cdp.events.clear();
    let wrapped = format!("view-source:{VAULT}");
    cdp.absorb(json!({
        "method": "Page.frameNavigated",
        "sessionId": "S",
        "params": {
            "frame": frame("CHILD", &wrapped, "://", CARRIER_MARK, "L9"),
        }
    }));
    assert!(cdp.events.iter().all(|event| {
        event.get("method").and_then(Value::as_str) != Some("Page.frameNavigated")
            || !serde_json::to_string(event).unwrap().contains(CARRIER_MARK)
    }));
    assert!(cdp.events.iter().all(|event| {
        !(event.get("method").and_then(Value::as_str) == Some("Page.frameNavigated")
            && event.pointer("/params/frame/url").and_then(Value::as_str) == Some(wrapped.as_str()))
    }));
}

#[test]
fn frame_carrier_event_protected_origin_with_ordinary_url_is_dropped() {
    let (mut cdp, _) = carrier_cdp(ordinary_tree_with_named_iframe());
    cdp.events.clear();
    cdp.absorb(json!({
        "method": "Page.frameNavigated",
        "sessionId": "S",
        "params": {
            "frame": frame("CHILD", "https://ordinary.example/looks-ordinary", VAULT_ORIGIN, CARRIER_MARK, "L9"),
        }
    }));
    assert!(cdp.events.iter().all(|event| {
        event.get("method").and_then(Value::as_str) != Some("Page.frameNavigated")
            || event.pointer("/params/frame/id").and_then(Value::as_str) != Some("CHILD")
    }));
}

#[test]
fn frame_carrier_event_data_text_plain_payload_is_forwarded_like_main() {
    let (mut cdp, _) = carrier_cdp(ordinary_tree_with_named_iframe());
    cdp.events.clear();
    let plain = format!("data:text/plain,{VAULT}");
    cdp.absorb(json!({
        "method": "Network.requestWillBeSent",
        "sessionId": "S",
        "params": {
            "requestId": "R-DATA",
            "type": "Other",
            "request": { "url": plain, "method": "GET", "headers": {} },
        }
    }));
    assert!(!Cdp::params_carrier_protected(
        &cdp.guard,
        &json!({
            "requestId": "R-DATA",
            "type": "Other",
            "request": { "url": plain, "method": "GET", "headers": {} },
        })
    ));
    assert!(cdp.events.iter().any(|event| {
        event.get("method").and_then(Value::as_str) == Some("Network.requestWillBeSent")
            && event.pointer("/params/requestId").and_then(Value::as_str) == Some("R-DATA")
    }));
}

#[test]
fn frame_carrier_event_frame_attached_under_protected_parent_stays_quiet() {
    let (mut cdp, _) = carrier_cdp(json!({
        "frame": frame("MAIN", VAULT, VAULT_ORIGIN, "", "L0"),
    }));
    let _ = cdp.call("Page.getFrameTree", json!({}), Some("S"));
    cdp.events.clear();
    cdp.absorb(json!({
        "method": "Page.frameAttached",
        "sessionId": "S",
        "params": {
            "frameId": "CHILD",
            "parentFrameId": "MAIN",
        }
    }));
    assert!(cdp
        .events
        .iter()
        .all(|event| event.get("method").and_then(Value::as_str) != Some("Page.frameAttached")));
}

#[test]
fn frame_carrier_nested_view_source_blob_refuses() {
    let wrapped = format!("view-source:blob:{VAULT}/11111111-1111-1111-1111-111111111111");
    let tree = json!({
        "frame": frame("MAIN", &wrapped, "://", CARRIER_MARK, "L0"),
    });
    let (mut cdp, _) = carrier_cdp(tree);
    assert_refused(cdp.call("Page.getFrameTree", json!({}), Some("S")));
}

#[test]
fn frame_carrier_userinfo_port_form_refuses() {
    let tree = json!({
        "frame": frame("MAIN", "https://user:pass@vault.sliccy.ai:443/private/FRAME_CARRIER_MARK", VAULT_ORIGIN, CARRIER_MARK, "L0"),
    });
    let (mut cdp, _) = carrier_cdp(tree);
    assert_refused(cdp.call("Page.getFrameTree", json!({}), Some("S")));
}

#[test]
fn frame_carrier_ordinary_page_with_protected_child_returns_without_latch() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main", "L0"),
        "childFrames": [
            {"frame": frame("PLAIN", "https://ordinary.example/embed", ORDINARY_ORIGIN, "playwright-frame", "L1")},
            {"frame": frame("PROT", VAULT, VAULT_ORIGIN, CARRIER_MARK, "L2")},
        ],
    });
    let (mut cdp, _) = carrier_cdp(tree);
    let listed = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(
        listed
            .pointer("/frameTree/childFrames/0/frame/name")
            .and_then(Value::as_str),
        Some("playwright-frame")
    );
    assert_eq!(
        listed
            .pointer("/frameTree/childFrames/1/frame/name")
            .and_then(Value::as_str),
        Some("")
    );
    assert!(!listed.to_string().contains(CARRIER_MARK));
    assert!(!cdp.is_protected_target("T"));
    assert!(!cdp.session_blocked("S"));
    assert!(cdp.call("Page.getFrameTree", json!({}), Some("S")).is_ok());
}

#[test]
fn frame_carrier_ordinary_page_with_protected_subresource_returns_without_latch() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main", "L0"),
        "resources": [
            {"url": "https://ordinary.example/app.js", "type": "Script", "mimeType": "text/javascript"},
            {"url": VAULT, "type": "Image", "mimeType": "image/png"},
        ],
    });
    let (mut cdp, state) = carrier_cdp(tree.clone());
    state.borrow_mut().resources = tree;
    let listed = cdp
        .call("Page.getResourceTree", json!({}), Some("S"))
        .unwrap();
    assert_eq!(
        listed
            .pointer("/frameTree/resources/0/mimeType")
            .and_then(Value::as_str),
        Some("text/javascript")
    );
    assert_eq!(
        listed
            .pointer("/frameTree/resources/1/mimeType")
            .and_then(Value::as_str),
        Some("")
    );
    assert!(!listed
        .pointer("/frameTree/resources/1")
        .unwrap()
        .to_string()
        .contains(CARRIER_MARK));
    assert!(!cdp.is_protected_target("T"));
    assert!(!cdp.session_blocked("S"));
}

#[test]
fn frame_carrier_verify_read_failure_blanks_names_without_refusal() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main-name", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "https://ordinary.example/embed", "://", "opaque-name", "L1"),
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().fail_tree_after = Some(2);
    for _ in 0..3 {
        let listed = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
        assert_eq!(
            listed
                .pointer("/frameTree/frame/name")
                .and_then(Value::as_str),
            Some("main-name")
        );
        assert_eq!(
            listed
                .pointer("/frameTree/childFrames/0/frame/name")
                .and_then(Value::as_str),
            Some("")
        );
        assert!(!cdp.is_protected_target("T"));
        assert!(!cdp.session_blocked("S"));
    }
    assert_eq!(cdp.frame_carrier_tree_reads, 1);
}

#[test]
fn frame_carrier_ordinary_data_documents_are_not_guard_url() {
    let guard = crate::protected::Protected::default();
    for text in [
        "data:text/html,<html><body>hello</body></html>",
        "data:text/plain,hello",
        "data:text/html;base64,PGh0bWw+PC9odG1sPg==",
        "data:application/json,{\"url\":\"https://vault.sliccy.ai/x\"}",
    ] {
        assert!(!guard.url(text), "{text}");
        assert!(!Cdp::carrier_data_payload_protected(&guard, text), "{text}");
    }
    let bare = format!("data:text/html,{VAULT}");
    assert!(!guard.url(&bare));
    assert!(Cdp::carrier_data_payload_protected(&guard, &bare));
}

#[test]
fn frame_carrier_ordinary_data_navigation_is_not_refused() {
    let (mut cdp, _) = carrier_cdp(ordinary_tree_with_named_iframe());
    for url in [
        "data:text/html,<html><body>ok</body></html>",
        "data:text/plain,hello",
        "data:text/html;base64,PGh0bWw+PC9odG1sPg==",
    ] {
        assert!(
            cdp.call("Page.navigate", json!({ "url": url }), Some("S"))
                .is_ok(),
            "{url}"
        );
    }
    assert!(cdp
        .call(
            "Page.navigate",
            json!({ "url": format!("data:text/html,{VAULT}") }),
            Some("S")
        )
        .is_ok());
}

#[test]
fn frame_carrier_get_frame_tree_protected_origin_ordinary_url_root_refuses() {
    let tree = json!({
        "frame": frame("MAIN", "https://ordinary.example/looks-ordinary", VAULT_ORIGIN, CARRIER_MARK, "L0"),
    });
    let (mut cdp, _) = carrier_cdp(tree);
    assert_refused(cdp.call("Page.getFrameTree", json!({}), Some("S")));
    assert!(cdp.is_protected_target("T"));
    assert!(cdp.session_blocked("S"));
}

#[test]
fn frame_carrier_get_frame_tree_protected_origin_ordinary_url_child_scrubs_in_place() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "https://ordinary.example/looks-ordinary", VAULT_ORIGIN, CARRIER_MARK, "L1"),
        }],
    });
    let (mut cdp, _) = carrier_cdp(tree);
    let listed = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(
        listed
            .pointer("/frameTree/frame/name")
            .and_then(Value::as_str),
        Some("main")
    );
    assert_eq!(
        listed
            .pointer("/frameTree/childFrames/0/frame/name")
            .and_then(Value::as_str),
        Some("")
    );
    assert!(!listed.to_string().contains(CARRIER_MARK));
    assert!(!cdp.is_protected_target("T"));
    assert!(!cdp.session_blocked("S"));
}

#[test]
fn frame_carrier_verify_race_blanks_stale_name_when_fresh_child_is_protected() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main-name", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "https://ordinary.example/embed", "://", CARRIER_MARK, "L1"),
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().verify_tree = Some(json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main-name", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", VAULT, VAULT_ORIGIN, "fresh-protected", "L1"),
        }],
    }));
    state.borrow_mut().tree_serves = 0;
    let listed = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 1);
    assert_eq!(
        listed
            .pointer("/frameTree/childFrames/0/frame/name")
            .and_then(Value::as_str),
        Some("")
    );
    assert!(!listed.to_string().contains(CARRIER_MARK));
    assert!(!cdp.is_protected_target("T"));
    assert!(!cdp.session_blocked("S"));
}

#[test]
fn frame_carrier_empty_fresh_tree_blanks_unclassified_names() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main-name", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "https://ordinary.example/embed", "://", CARRIER_MARK, "L1"),
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().verify_tree = Some(json!({}));
    state.borrow_mut().tree_serves = 0;
    let listed = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 1);
    assert_eq!(
        listed
            .pointer("/frameTree/childFrames/0/frame/name")
            .and_then(Value::as_str),
        Some("")
    );
    assert!(!listed.to_string().contains(CARRIER_MARK));
    assert!(!cdp.is_protected_target("T"));
}

#[test]
fn frame_carrier_child_without_mime_type_is_classified_and_blanked() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main-name", "L0"),
        "childFrames": [{
            "frame": {
                "id": "CHILD",
                "loaderId": "L1",
                "url": "about:blank",
                "securityOrigin": "://",
                "name": CARRIER_MARK,
            },
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().tree_serves = 0;
    let listed = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 1);
    assert_eq!(
        listed
            .pointer("/frameTree/childFrames/0/frame/name")
            .and_then(Value::as_str),
        Some("")
    );
    assert!(!listed.to_string().contains(CARRIER_MARK));
}

#[test]
fn frame_carrier_stock_inherited_blank_iframes_keep_names_with_zero_extra_reads() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main", "L0"),
        "childFrames": [
            {"frame": frame("B1", "about:blank", ORDINARY_ORIGIN, "one", "L1")},
            {"frame": frame("B2", "about:blank", ORDINARY_ORIGIN, "two", "L2")},
            {"frame": frame("B3", "about:srcdoc", ORDINARY_ORIGIN, "three", "L3")},
        ],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().tree_serves = 0;
    let listed = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 0, "inherited-origin blank/srcdoc must not verify-read");
    assert_eq!(state.borrow().tree_serves, 1);
    assert_eq!(listed["frameTree"]["childFrames"][0]["frame"]["name"], "one");
    assert_eq!(listed["frameTree"]["childFrames"][1]["frame"]["name"], "two");
    assert_eq!(listed["frameTree"]["childFrames"][2]["frame"]["name"], "three");
}

#[test]
fn frame_carrier_stock_repeated_calls_on_unchanged_page_reuse_verify_cache() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "https://ordinary.example/embed", "://", "opaque-name", "L1"),
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().tree_serves = 0;
    let first = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    let second = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 1, "second call on same generation must use cache");
    assert_eq!(state.borrow().tree_serves, 3);
    assert_eq!(first["frameTree"]["childFrames"][0]["frame"]["name"], "");
    assert_eq!(second["frameTree"]["childFrames"][0]["frame"]["name"], "");
}

#[test]
fn frame_carrier_verify_cache_invalidates_on_navigation() {
    let tree = json!({
        "frame": frame("MAIN", ORDINARY, ORDINARY_ORIGIN, "main", "L0"),
        "childFrames": [{
            "frame": frame("CHILD", "https://ordinary.example/embed", "://", "opaque-name", "L1"),
        }],
    });
    let (mut cdp, state) = carrier_cdp(tree);
    state.borrow_mut().tree_serves = 0;
    let _ = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 1);
    cdp.absorb(json!({
        "method": "Page.frameStartedNavigating",
        "sessionId": "S",
        "params": {
            "frameId": "MAIN",
            "url": "https://ordinary.example/next",
            "loaderId": "L9",
            "navigationType": "differentDocument",
        }
    }));
    let _ = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(cdp.frame_carrier_tree_reads, 2, "navigation must invalidate verify cache");
}

#[test]
fn frame_carrier_chrome_error_protected_unreachable_root_forwards_without_latch() {
    let error_frame = json!({
        "id": "MAIN",
        "loaderId": "L0",
        "url": "chrome-error://chromewebdata/",
        "securityOrigin": "://",
        "mimeType": "text/html",
        "name": CARRIER_MARK,
        "unreachableUrl": VAULT,
    });
    let tree = json!({ "frame": error_frame.clone() });
    let (mut cdp, state) = carrier_cdp(tree.clone());
    state.borrow_mut().resources = tree;
    cdp.absorb(json!({
        "method": "Page.frameNavigated",
        "sessionId": "S",
        "params": { "frame": error_frame },
    }));
    let listed = cdp.call("Page.getFrameTree", json!({}), Some("S")).unwrap();
    assert_eq!(listed.pointer("/frameTree/frame/name").and_then(Value::as_str), Some(""));
    assert_eq!(
        listed.pointer("/frameTree/frame/url").and_then(Value::as_str),
        Some("chrome-error://chromewebdata/")
    );
    assert!(
        listed.pointer("/frameTree/frame/unreachableUrl").is_some(),
        "main and fixed forward unreachableUrl (redacted); must not drop the key"
    );
    assert!(!listed.to_string().contains(CARRIER_MARK));
    assert!(!listed.to_string().contains("vault.sliccy.ai/private"));
    assert!(!cdp.session_blocked("S"));
    assert_refused(cdp.call("Page.getResourceTree", json!({}), Some("S")));
    assert!(!cdp.session_blocked("S"));
}

fn assert_history_non_url_cleared(entry: &Value) {
    assert_eq!(entry["title"], "");
    assert_eq!(entry["transitionType"], "");
    assert!(!serde_json::to_string(entry).unwrap().contains(CARRIER_MARK));
}

#[test]
fn frame_carrier_get_navigation_history_opaque_wrapper_clears_non_url() {
    let wrapped = format!("view-source:{VAULT}");
    let (mut cdp, state) = carrier_cdp(ordinary_tree_with_named_iframe());
    state.borrow_mut().history = json!({
        "currentIndex": 0,
        "entries": [{
            "id": 1,
            "url": wrapped,
            "userTypedURL": wrapped,
            "title": CARRIER_MARK,
            "transitionType": CARRIER_MARK,
        }],
    });
    let result = cdp
        .call("Page.getNavigationHistory", json!({}), Some("S"))
        .unwrap();
    assert_history_non_url_cleared(&result["entries"][0]);
}

#[test]
fn frame_carrier_get_navigation_history_filesystem_wrapper_clears_non_url() {
    let wrapped = format!("filesystem:{VAULT}");
    let (mut cdp, state) = carrier_cdp(ordinary_tree_with_named_iframe());
    state.borrow_mut().history = json!({
        "currentIndex": 0,
        "entries": [{
            "id": 1,
            "url": wrapped,
            "userTypedURL": wrapped,
            "title": CARRIER_MARK,
            "transitionType": CARRIER_MARK,
        }],
    });
    let result = cdp
        .call("Page.getNavigationHistory", json!({}), Some("S"))
        .unwrap();
    assert_history_non_url_cleared(&result["entries"][0]);
}

#[test]
fn frame_carrier_get_navigation_history_data_wrapper_clears_url_and_non_url() {
    let wrapped = format!("data:text/html,{VAULT}");
    let (mut cdp, state) = carrier_cdp(ordinary_tree_with_named_iframe());
    state.borrow_mut().history = json!({
        "currentIndex": 0,
        "entries": [{
            "id": 1,
            "url": wrapped,
            "userTypedURL": wrapped,
            "title": CARRIER_MARK,
            "transitionType": CARRIER_MARK,
        }],
    });
    let result = cdp
        .call("Page.getNavigationHistory", json!({}), Some("S"))
        .unwrap();
    let entry = &result["entries"][0];
    assert_history_non_url_cleared(entry);
    assert_ne!(
        entry["url"].as_str().unwrap_or(wrapped.as_str()),
        wrapped.as_str()
    );
    assert_ne!(
        entry["userTypedURL"].as_str().unwrap_or(wrapped.as_str()),
        wrapped.as_str()
    );
    assert!(!serde_json::to_string(entry)
        .unwrap()
        .contains("vault.sliccy.ai"));
}

#[test]
fn frame_carrier_get_navigation_history_https_clears_transition_and_title() {
    let (mut cdp, state) = carrier_cdp(ordinary_tree_with_named_iframe());
    state.borrow_mut().history = json!({
        "currentIndex": 0,
        "entries": [{
            "id": 1,
            "url": VAULT,
            "userTypedURL": VAULT,
            "title": CARRIER_MARK,
            "transitionType": CARRIER_MARK,
        }],
    });
    let result = cdp
        .call("Page.getNavigationHistory", json!({}), Some("S"))
        .unwrap();
    let entry = &result["entries"][0];
    assert_history_non_url_cleared(entry);
    assert_ne!(entry["url"], VAULT);
}

#[test]
fn frame_carrier_event_history_shaped_non_url_cleared_only_when_original_protected() {
    let (mut cdp, _) = carrier_cdp(ordinary_tree_with_named_iframe());
    cdp.events.clear();
    let wrapped = format!("view-source:{VAULT}");
    cdp.absorb(json!({
        "method": "Page.frameNavigated",
        "sessionId": "S",
        "params": {
            "frame": {
                "id": "CHILD",
                "loaderId": "L9",
                "url": wrapped,
                "securityOrigin": "://",
                "mimeType": "text/html",
                "name": CARRIER_MARK,
            }
        }
    }));
    assert!(cdp.events.iter().all(|event| {
        event.get("method").and_then(Value::as_str) != Some("Page.frameNavigated")
            || !serde_json::to_string(event).unwrap().contains(CARRIER_MARK)
    }));
}
