#[test]
fn production_cdp_methods_have_a_lifetime_classification() {
    let document = set(&[
        "DOM.describeNode", "DOM.getFrameOwner", "DOM.resolveNode", "DOM.setFileInputFiles",
        "Input.dispatchKeyEvent", "Input.dispatchMouseEvent", "Input.insertText",
        "Page.captureScreenshot", "Page.createIsolatedWorld", "Page.getFrameTree",
        "Page.getNavigationHistory", "Page.navigate", "Page.printToPDF", "Page.reload",
        "Runtime.callFunctionOn", "Runtime.evaluate", "Runtime.getProperties", "Runtime.releaseObjectGroup",
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
    let inventory = [&document, &observation, &request, &origin, &target, &internal, &vetted_shim];
    let mut methods: BTreeSet<String> = sent().into_iter()
        .filter(|(_, _, place)| !place.contains("_test.rs") && !place.contains("/raw.rs") && !place.contains("/race.rs"))
        .map(|(method, _, _)| method)
        .collect();
    for method in ["Fetch.fulfillRequest", "Fetch.continueResponse", "Network.setBypassServiceWorker"] {
        methods.insert(method.to_string());
    }
    for method in &methods {
        assert_eq!(inventory.iter().filter(|class| class.contains(method)).count(), 1, "unclassified or ambiguous method: {method}");
        if crate::shield::persistent_capability(method) {
            if vetted_shim.contains(method) {
                let source = fs::read_to_string(root().join("crates/cdp-client/src/webmcp_guard.rs")).unwrap();
                assert!(source.contains("sha256_hex(SHIM.as_bytes()) != SHIM_HASH"));
                assert!(source.contains("__slicc_webmcp_admitted"));
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
