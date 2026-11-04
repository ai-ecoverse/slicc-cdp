struct ExtensionFrameBrowser {
    present: bool,
    shown: bool,
    focused: bool,
    reveal_on_capture: bool,
    attach_on_capture: Option<bool>,
    defer_attach_probes: usize,
    visibility_error: bool,
    calls: Vec<Value>,
    attached: usize,
}

struct ExtensionFrameTransport {
    browser: Rc<RefCell<ExtensionFrameBrowser>>,
    inbox: VecDeque<String>,
}

fn extension_frame_page() -> Value {
    json!({"targetId":"T","type":"page","url":"https://ordinary.test/","title":"","browserContextId":"CTX"})
}

fn extension_frame_target() -> Value {
    json!({"targetId":"EXT","type":"iframe","parentFrameId":"T","url":"chrome-extension://fixture/frame.html?username=private","title":"private","browserContextId":"CTX"})
}

impl Transport for ExtensionFrameTransport {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut browser = self.browser.borrow_mut();
        browser.calls.push(call.clone());
        let result = match call["method"].as_str().unwrap() {
            "Target.attachToTarget" => {
                assert_eq!(call["params"]["targetId"], "T");
                browser.attached += 1;
                json!({"sessionId":format!("S{}", browser.attached)})
            }
            "Target.getTargetInfo" => json!({"targetInfo":if call["params"]["targetId"] == "EXT" { extension_frame_target() } else { extension_frame_page() }}),
            "Target.getTargets" => {
                let mut targets = vec![extension_frame_page()];
                if browser.present { targets.push(extension_frame_target()); }
                if browser.defer_attach_probes > 0 {
                    browser.defer_attach_probes -= 1;
                    if browser.defer_attach_probes == 0 { browser.present = true; }
                }
                json!({"targetInfos":targets})
            }
            "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"T","loaderId":"L","url":"https://ordinary.test/","securityOrigin":"https://ordinary.test"}}}),
            "Page.createIsolatedWorld" => json!({"executionContextId":9}),
            "DOM.getFrameOwner" => json!({"backendNodeId":17}),
            "DOM.resolveNode" => json!({"object":{"objectId":"owner"}}),
            "Runtime.callFunctionOn" if browser.visibility_error => json!({"exceptionDetails":{"text":"visibility failed"}}),
            "Runtime.callFunctionOn" => json!({"result":{"type":"boolean","value":browser.shown || browser.focused && call["params"]["functionDeclaration"].as_str().unwrap().contains("getRootNode")}}),
            "Runtime.evaluate" => json!({"result":{"type":"string","value":"complete"}}),
            "Page.captureScreenshot" => {
                if browser.reveal_on_capture {
                    browser.present = true;
                    browser.shown = true;
                }
                if let Some(shown) = browser.attach_on_capture {
                    browser.shown = shown;
                    browser.focused = false;
                    browser.defer_attach_probes = 1;
                }
                json!({"data":"private-pixels"})
            }
            _ => json!({}),
        };
        self.inbox.push_back(json!({"id":call["id"],"sessionId":call["sessionId"],"result":result}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn extension_frame_client(present: bool, shown: bool) -> (Cdp, String, Rc<RefCell<ExtensionFrameBrowser>>) {
    let browser = Rc::new(RefCell::new(ExtensionFrameBrowser {
        present, shown, focused:false, reveal_on_capture:false, attach_on_capture:None, defer_attach_probes:0, visibility_error:false, calls:Vec::new(), attached:0,
    }));
    let mut cdp = Cdp::from_transport(Box::new(ExtensionFrameTransport { browser:browser.clone(), inbox:VecDeque::new() }));
    let session = super::attach(&mut cdp, "T").unwrap();
    (cdp, session, browser)
}

#[test]
fn extension_frame_visibility_gates_capture_without_locking_page_reads() {
    for shown in [false, true] {
        let (mut cdp, session, browser) = extension_frame_client(true, shown);
        assert!(cdp.call("DOM.getDocument", json!({}), Some(&session)).is_ok());
        let picture = cdp.call("Page.captureScreenshot", json!({}), Some(&session));
        assert_eq!(picture.is_ok(), !shown);
        if shown { assert_eq!(picture.unwrap_err(), cdp.guard.extension_shown_refusal()); }
        assert_eq!(browser.borrow().calls.iter().any(|call| call["method"] == "Page.captureScreenshot"), !shown);
        assert_eq!(cdp.call("Page.printToPDF", json!({}), Some(&session)).unwrap_err(), cdp.guard.extension_present_refusal());
        assert!(!browser.borrow().calls.iter().any(|call| call["method"] == "Page.printToPDF"));
        for method in [
            "Input.dispatchKeyEvent", "Input.dispatchMouseEvent", "Input.dispatchTouchEvent",
            "Input.dispatchDragEvent", "Input.emulateTouchFromMouseEvent", "Input.insertText",
            "Input.imeSetComposition", "Input.synthesizePinchGesture",
            "Input.synthesizeScrollGesture", "Input.synthesizeTapGesture",
        ] {
            let answer = cdp.call(method, json!({}), Some(&session));
            assert_eq!(answer.is_ok(), !shown);
            if shown { assert_eq!(answer.unwrap_err(), cdp.guard.extension_shown_refusal()); }
            assert_eq!(browser.borrow().calls.iter().any(|call| call["method"] == method), !shown);
        }
    }
}

#[test]
fn extension_frame_added_or_revealed_during_capture_discards_the_pixels() {
    for present in [false, true] {
        let (mut cdp, session, browser) = extension_frame_client(present, false);
        browser.borrow_mut().reveal_on_capture = true;
        let message = cdp.call("Page.captureScreenshot", json!({}), Some(&session)).unwrap_err();
        assert_eq!(message, cdp.guard.extension_shown_refusal());
        assert!(browser.borrow().calls.iter().any(|call| call["method"] == "Page.captureScreenshot"));
        assert_eq!(cdp.extension_frames["T"], ["EXT"]);
        browser.borrow_mut().shown = false;
        browser.borrow_mut().reveal_on_capture = false;
        assert!(cdp.call("Page.captureScreenshot", json!({}), Some(&session)).is_ok());
    }
}

#[test]
fn extension_frame_attaching_shown_during_capture_is_refused_after_reply() {
    let (mut cdp, session, browser) = extension_frame_client(false, false);
    browser.borrow_mut().attach_on_capture = Some(true);
    let message = cdp.call("Page.captureScreenshot", json!({}), Some(&session)).unwrap_err();
    assert_eq!(message, cdp.guard.extension_shown_refusal());
    assert!(browser.borrow().calls.iter().any(|call| call["method"] == "Page.captureScreenshot"));
    assert_eq!(cdp.extension_frames["T"], ["EXT"]);
}

#[test]
fn extension_frame_attaching_hidden_during_capture_keeps_the_pixels() {
    let (mut cdp, session, browser) = extension_frame_client(false, false);
    browser.borrow_mut().attach_on_capture = Some(false);
    let picture = cdp.call("Page.captureScreenshot", json!({}), Some(&session)).unwrap();
    assert_eq!(picture["data"], "private-pixels");
    assert_eq!(cdp.extension_frames["T"], ["EXT"]);
}

#[test]
fn extension_frame_focus_is_rechecked_before_each_input() {
    let (mut cdp, session, browser) = extension_frame_client(true, false);
    for focused in [false, true, false] {
        browser.borrow_mut().focused = focused;
        browser.borrow_mut().calls.clear();
        for method in ["Input.dispatchKeyEvent", "Input.insertText", "Input.imeSetComposition", "Input.dispatchMouseEvent"] {
            let answer = cdp.call(method, json!({}), Some(&session));
            assert_eq!(answer.is_ok(), !focused);
            if focused { assert_eq!(answer.unwrap_err(), cdp.guard.extension_shown_refusal()); }
            assert_eq!(browser.borrow().calls.iter().any(|call| call["method"] == method), !focused);
        }
        let picture = cdp.call("Page.captureScreenshot", json!({}), Some(&session));
        assert_eq!(picture.is_ok(), !focused);
        if focused { assert_eq!(picture.unwrap_err(), cdp.guard.extension_shown_refusal()); }
    }
}

#[test]
fn extension_frame_visibility_failure_refuses_before_capture() {
    let (mut cdp, session, browser) = extension_frame_client(true, false);
    browser.borrow_mut().visibility_error = true;
    assert_eq!(cdp.call("Page.captureScreenshot", json!({}), Some(&session)).unwrap_err(), cdp.guard.extension_shown_refusal());
    assert!(!browser.borrow().calls.iter().any(|call| call["method"] == "Page.captureScreenshot"));
    assert_eq!(cdp.call("Input.insertText", json!({"text":"blocked"}), Some(&session)).unwrap_err(), cdp.guard.extension_shown_refusal());
    assert!(!browser.borrow().calls.iter().any(|call| call["method"] == "Input.insertText"));
}

#[test]
fn extension_frame_screencasts_are_refused_and_late_frames_are_stopped() {
    let (mut cdp, session, browser) = extension_frame_client(false, false);
    assert!(cdp.call("Page.startScreencast", json!({"format":"png"}), Some(&session)).unwrap_err().starts_with("slicc: refused:"));
    assert!(!browser.borrow().calls.iter().any(|call| call["method"] == "Page.startScreencast"));
    for (index, (present, shown)) in [(false, false), (true, false), (true, true)].into_iter().enumerate() {
        browser.borrow_mut().present = present;
        browser.borrow_mut().shown = shown;
        if index == 1 {
            cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":extension_frame_target()}}));
        }
        let before = browser.borrow().calls.iter().filter(|call| call["method"] == "Page.stopScreencast").count();
        let marker = format!("late-screencast-pixels-{index}");
        cdp.absorb(json!({"method":"Page.screencastFrame","sessionId":session,"params":{"sessionId":index,"data":marker,"metadata":{"deviceWidth":800,"deviceHeight":600}}}));
        assert!(!cdp.events().iter().any(|event| event["method"] == "Page.screencastFrame"));
        assert!(!format!("{:?}", cdp.events()).contains(&marker));
        let stopped = browser.borrow().calls.iter().filter(|call| call["method"] == "Page.stopScreencast" && call["sessionId"] == session).count();
        assert_eq!(stopped, before + 1);
    }
}

#[test]
fn extension_frame_stays_unattachable_and_redacted() {
    let (mut cdp, session, _) = extension_frame_client(true, false);
    assert!(cdp.call("Target.attachToTarget", json!({"targetId":"EXT","flatten":true}), None).is_err());
    assert!(cdp.call("Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true}), Some(&session)).is_err());
    for (method, params) in [("Target.getTargets", json!({})), ("Target.getTargetInfo", json!({"targetId":"EXT"}))] {
        let result = cdp.call(method, params, None).unwrap().to_string();
        assert!(!result.contains("private") && !result.contains("frame.html"), "{result}");
    }
}
