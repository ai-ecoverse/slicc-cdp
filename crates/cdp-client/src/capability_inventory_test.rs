#[test]
fn production_cdp_methods_have_a_lifetime_classification() {
    let document = set(&[
        "DOM.describeNode", "DOM.getDocument", "DOM.getFrameOwner", "DOM.getNodeForLocation", "DOM.querySelectorAll", "DOM.resolveNode", "DOM.setFileInputFiles",
        "Input.dispatchKeyEvent", "Input.dispatchMouseEvent", "Input.insertText",
        "Page.captureScreenshot", "Page.createIsolatedWorld", "Page.getFrameTree", "Page.getLayoutMetrics",
        "Page.getNavigationHistory", "Page.handleJavaScriptDialog", "Page.getPermissionsPolicyState", "Page.navigate", "Page.printToPDF", "Page.reload", "Page.stopScreencast",
        "Runtime.callFunctionOn", "Runtime.compileScript", "Runtime.evaluate", "Runtime.getProperties", "Runtime.releaseObjectGroup",
        "Debugger.resume",
    ]);
    let observation = set(&[
        "DOM.enable", "Network.enable", "Page.enable", "Runtime.enable", "Runtime.disable", "Target.setDiscoverTargets",
    ]);
    let request = set(&[
        "Fetch.continueRequest", "Fetch.continueResponse", "Fetch.continueWithAuth", "Fetch.failRequest",
        "Fetch.fulfillRequest", "Fetch.getResponseBody", "Network.getRequestPostData", "Network.getResponseBody",
    ]);
    let origin = set(&["Network.deleteCookies", "Network.getAllCookies", "Network.setCookie"]);
    let target = set(&[
        "Target.activateTarget", "Target.attachToTarget", "Target.closeTarget", "Target.createTarget",
        "Target.detachFromTarget", "Target.getTargetInfo", "Target.getTargets",
    ]);
    let internal = set(&["Browser.getVersion", "Slicc.action", "Fetch.enable", "Fetch.disable", "Network.setBypassServiceWorker"]);
    let vetted_shim = set(&["Debugger.enable", "Debugger.getScriptSource", "Debugger.evaluateOnCallFrame", "Page.addScriptToEvaluateOnNewDocument", "Page.removeScriptToEvaluateOnNewDocument", "Runtime.runIfWaitingForDebugger", "Target.setAutoAttach"]);
    let window = set(&["Browser.getWindowForTarget", "Browser.setWindowBounds"]);
    let network_state = set(&["Network.emulateNetworkConditions"]);
    let inventory = [&document, &observation, &request, &origin, &target, &internal, &vetted_shim, &window, &network_state];
    let mut methods: BTreeSet<String> = sent().into_iter()
        .filter(|(_, _, place)| !place.contains("_test.rs") && !place.contains("/raw.rs") && !place.contains("/race.rs"))
        .map(|(method, _, _)| method)
        .collect();
    for method in ["Fetch.fulfillRequest", "Fetch.continueResponse", "Network.setBypassServiceWorker"] {
        methods.insert(method.to_string());
    }
    for method in &methods {
        assert_eq!(inventory.iter().filter(|class| class.contains(method)).count(), 1, "unclassified or ambiguous method: {method}");
        if method == "Runtime.compileScript" {
            let source = fs::read_to_string(root().join("crates/playwright-cli/src/browser.rs")).unwrap();
            let compile = source.split("let mut compile =").nth(1).unwrap().split("let parsed =").next().unwrap();
            assert!(compile.contains("\"persistScript\": false"));
            assert!(!compile.contains("\"persistScript\": true"));
        }
        if crate::shield::persistent_capability(method) {
            if vetted_shim.contains(method) {
                let source = fs::read_to_string(root().join("crates/cdp-client/src/webmcp_guard.rs")).unwrap();
                assert!(source.contains("vetted_source(SHIM)?"));
                assert!(source.contains("sha256_hex(source.as_bytes()) != SHIM_HASH"));
                assert!(source.contains("__slicc_webmcp_admitted"));
                continue;
            }
            if window.contains(method) {
                let guard = fs::read_to_string(root().join("crates/cdp-client/src/cdp.rs")).unwrap();
                let source = fs::read_to_string(root().join("crates/cdp-client/src/window.rs")).unwrap();
                assert!(guard.contains("if method == \"Browser.setWindowBounds\" {\n            self.admit_window(params, session_id)?;"));
                assert!(source.contains("fn admit_window") && source.contains("self.info_protected(info, &infos)"));
                continue;
            }
            if network_state.contains(method) {
                let shield = fs::read_to_string(root().join("crates/cdp-client/src/shield.rs")).unwrap();
                let shape = shield.split("pub fn network_state(").nth(1).unwrap().split("pub fn persistent_capability(").next().unwrap();
                for part in ["object.len() == 4", "\"offline\").is_some_and(Value::is_boolean)", "\"latency\").and_then(Value::as_i64) == Some(0)", "\"downloadThroughput\").and_then(Value::as_i64) == Some(-1)", "\"uploadThroughput\").and_then(Value::as_i64) == Some(-1)"] {
                    assert!(shape.contains(part), "{part}");
                }
                let source = fs::read_to_string(root().join("crates/cdp-client/src/cdp.rs")).unwrap();
                assert!(source.contains("shield::persistent_capability(method) && !self.network_state_admitted(method, params, session_id)"));
                let admitted = source.split("fn network_state_admitted(").nth(1).unwrap().split("fn prepare(").next().unwrap();
                for part in ["!self.guard_sessions.contains(session)", "!self.blocked.contains(session)", "Some(\"page\")", "!self.info_protected(info, &[])"] {
                    assert!(admitted.contains(part), "{part}");
                }
                continue;
            }
            assert_eq!(method, "Network.setBypassServiceWorker", "CLI sends an unsafe persistent method: {method}");
            let source = fs::read_to_string(root().join("crates/cdp-client/src/cdp.rs")).unwrap();
            let setup = source.split("let setup = [").nth(1).unwrap().split("];" ).next().unwrap();
            assert!(setup.contains("Network.setBypassServiceWorker"));
            assert!(source.contains("self.raw_call(method, params, Some(&session), span)"));
        }
    }
    let classified: BTreeSet<String> = inventory.iter().flat_map(|class| class.iter().cloned()).collect();
    assert_eq!(methods, classified, "lifetime inventory must match every method production CLIs send");
}
