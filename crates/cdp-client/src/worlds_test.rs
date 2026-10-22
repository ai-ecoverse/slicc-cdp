use crate::cdp::{Cdp, Transport};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

const UNSEEN: &str =
    "slicc: refused: that execution context or object is not exposed to browser automation\n";
const REFUSAL: &str = "slicc: refused: that execution context belongs to a browser extension\n";

struct Backend {
    sent: Rc<RefCell<Vec<Value>>>,
    incoming: VecDeque<Value>,
}

impl Transport for Backend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        self.sent.borrow_mut().push(message.clone());
        let session = message["sessionId"].clone();
        let method = message["method"].as_str().unwrap();
        if method == "Runtime.enable" {
            for (id, origin, default) in [
                (1, "https://example.test", true),
                (7, "chrome-extension://fixture", false),
            ] {
                self.incoming.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":id,"uniqueId":format!("world-{id}"),"name":format!("world-{id}"),"origin":origin,"auxData":{"isDefault":default,"frameId":"F","type":if default { "default" } else { "isolated" }}}}}));
            }
            for method in [
                "Runtime.consoleAPICalled",
                "Runtime.exceptionThrown",
                "Runtime.bindingCalled",
            ] {
                self.incoming.push_back(json!({"method":method,"sessionId":session,"params":{"executionContextId":7,"exceptionDetails":{"executionContextId":7},"args":[{"objectId":"extension-object","description":"EXTENSION-SECRET"}]}}));
            }
        }
        if method == "Runtime.enable" {
            self.incoming.push_back(json!({"method":"Debugger.scriptParsed","sessionId":session,"params":{"executionContextId":7,"scriptId":"extension-script","url":"EXTENSION-SECRET"}}));
            self.incoming.push_back(json!({"method":"Debugger.paused","sessionId":session,"params":{"callFrames":[{"callFrameId":"extension-call","location":{"scriptId":"extension-script"}}]}}));
            self.incoming.push_back(json!({"method":"HeapProfiler.addHeapSnapshotChunk","sessionId":session,"params":{"chunk":"EXTENSION-SECRET"}}));
        }
        if method == "Runtime.evaluate" && message["params"]["expression"] == "destroy-hidden" {
            self.incoming.push_back(json!({"method":"Runtime.executionContextDestroyed","sessionId":session,"params":{"executionContextId":7,"executionContextUniqueId":"world-7"}}));
        }
        if method == "Runtime.evaluate" && message["params"]["expression"] == "console" {
            self.incoming.push_back(json!({"method":"Runtime.consoleAPICalled","sessionId":session,"params":{"executionContextId":1,"args":[{"type":"object","objectId":"console-object"}]}}));
            self.incoming.push_back(json!({"method":"Runtime.exceptionThrown","sessionId":session,"params":{"exceptionDetails":{"executionContextId":1,"exception":{"type":"object","objectId":"exception-object"}}}}));
        }
        let result = match method {
            "Network.getCookies" => {
                json!({"cookies":[{"name":"poison","domain":"example.test","type":"object","objectId":"forged-object"}]})
            }
            "Runtime.evaluate" if message["params"]["expression"] == "spoof" => {
                json!({"result":{"type":"object","value":{"type":"object","objectId":"extension-object"}}})
            }
            "Runtime.evaluate" | "Runtime.callFunctionOn" => {
                json!({"result":{"type":"object","objectId":"page-object","value":"PAGE-MARK"}})
            }
            "Runtime.getProperties" => {
                json!({"result":[{"name":"child","value":{"type":"object","objectId":"child-object"}},{"name":"getter","get":{"type":"function","objectId":"getter-object"}}]})
            }
            "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"F","url":"https://example.test/","securityOrigin":"https://example.test"}}}),
            "Page.createIsolatedWorld" => json!({"executionContextId":11}),
            "DOM.resolveNode" => json!({"object":{"type":"object","objectId":"node-object"}}),
            "Runtime.compileScript" => json!({"scriptId":"page-script"}),
            _ => json!({}),
        };
        self.incoming
            .push_back(json!({"id":message["id"],"result":result,"sessionId":session}));
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(self.incoming.pop_front().map(|value| value.to_string()))
    }
}

fn client() -> (Cdp, Rc<RefCell<Vec<Value>>>) {
    let sent = Rc::new(RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(Backend {
        sent: sent.clone(),
        incoming: VecDeque::new(),
    }));
    cdp.fixture_session("S");
    cdp.fixture_session("OTHER");
    (cdp, sent)
}

#[test]
fn object_aliases_refuse_hidden_worlds_before_call_or_notify() {
    for (method, key) in [
        ("Runtime.awaitPromise", "promiseObjectId"),
        ("Runtime.getExceptionDetails", "errorObjectId"),
    ] {
        let (mut cdp, sent) = client();
        cdp.call("Runtime.enable", json!({}), Some("S")).unwrap();
        let params = json!({key: "extension-object"});
        let count = sent.borrow().len();
        assert_eq!(
            cdp.call(method, params.clone(), Some("S")).unwrap_err(),
            REFUSAL
        );
        assert_eq!(cdp.notify(method, params, Some("S")).unwrap_err(), REFUSAL);
        assert_eq!(sent.borrow().len(), count);
        assert!(!cdp.session_blocked("S"));
    }
}

#[test]
fn guessed_object_aliases_cannot_skip_context_discovery() {
    for (method, key) in [
        ("Runtime.awaitPromise", "promiseObjectId"),
        ("Runtime.getExceptionDetails", "errorObjectId"),
    ] {
        for (id, reason) in [("extension-object", REFUSAL), ("unknown-object", UNSEEN)] {
            let (mut cdp, sent) = client();
            assert_eq!(
                cdp.call(method, json!({key: id}), Some("S")).unwrap_err(),
                reason
            );
            assert!(sent
                .borrow()
                .iter()
                .all(|message| message["method"] != method));
        }
    }
}

#[test]
fn visible_object_aliases_are_session_scoped_and_need_no_new_preflight() {
    let (mut cdp, sent) = client();
    cdp.call("Runtime.enable", json!({}), Some("S")).unwrap();
    cdp.call(
        "Runtime.evaluate",
        json!({"expression":"document"}),
        Some("S"),
    )
    .unwrap();
    for (method, key) in [
        ("Runtime.awaitPromise", "promiseObjectId"),
        ("Runtime.getExceptionDetails", "errorObjectId"),
    ] {
        cdp.call(method, json!({key: "page-object"}), Some("S"))
            .unwrap();
    }
    assert_eq!(
        sent.borrow()
            .iter()
            .filter(|message| message["method"] == "Runtime.enable")
            .count(),
        1
    );
    for (method, key) in [
        ("Runtime.awaitPromise", "promiseObjectId"),
        ("Runtime.getExceptionDetails", "errorObjectId"),
    ] {
        assert_eq!(
            cdp.call(method, json!({key: "page-object"}), Some("OTHER"))
                .unwrap_err(),
            UNSEEN
        );
    }
}

#[test]
fn extension_worlds_are_hidden_without_blocking_the_page() {
    let (mut cdp, _) = client();
    cdp.call("Runtime.enable", json!({}), Some("S")).unwrap();
    assert!(!cdp.session_blocked("S"));
    assert_eq!(cdp.events().len(), 1, "{:?}", cdp.events());
    assert_eq!(
        cdp.events()[0].pointer("/params/context/id"),
        Some(&json!(1))
    );
    let result = cdp
        .call(
            "Runtime.evaluate",
            json!({"expression":"document.title","contextId":1}),
            Some("S"),
        )
        .unwrap();
    assert_eq!(result.pointer("/result/value"), Some(&json!("PAGE-MARK")));
    cdp.call(
        "Runtime.getProperties",
        json!({"objectId":"page-object"}),
        Some("S"),
    )
    .unwrap();
}

#[test]
fn extension_context_selectors_and_objects_never_reach_the_backend() {
    let (mut cdp, sent) = client();
    cdp.call("Runtime.enable", json!({}), Some("S")).unwrap();
    for (method, params) in [
        (
            "Runtime.addBinding",
            json!({"name":"extensionBinding","executionContextName":"world-7"}),
        ),
        ("Runtime.addBinding", json!({"name":"unscopedBinding"})),
        (
            "Runtime.queryObjects",
            json!({"prototypeObjectId":"extension-object"}),
        ),
        (
            "Debugger.evaluateOnCallFrame",
            json!({"callFrameId":"extension-call","expression":"chrome.runtime.id"}),
        ),
        (
            "Debugger.setVariableValue",
            json!({"callFrameId":"extension-call","scopeNumber":0,"variableName":"x","newValue":{"value":1}}),
        ),
        (
            "Debugger.restartFrame",
            json!({"callFrameId":"extension-call"}),
        ),
        ("Profiler.start", json!({})),
        (
            "HeapProfiler.getHeapObjectId",
            json!({"objectId":"extension-object"}),
        ),
        (
            "Runtime.evaluate",
            json!({"expression":"chrome.runtime.id","contextId":7}),
        ),
        (
            "Runtime.evaluate",
            json!({"expression":"chrome.runtime.id","uniqueContextId":"world-7"}),
        ),
        (
            "Runtime.callFunctionOn",
            json!({"functionDeclaration":"function(){return chrome.runtime.id}","executionContextId":7}),
        ),
        (
            "Runtime.compileScript",
            json!({"expression":"chrome.runtime.id","executionContextId":7}),
        ),
        (
            "DOM.resolveNode",
            json!({"nodeId":1,"executionContextId":7}),
        ),
        ("Runtime.runScript", json!({"scriptId":"extension-script"})),
        (
            "Runtime.getProperties",
            json!({"objectId":"extension-object"}),
        ),
        (
            "Runtime.callFunctionOn",
            json!({"objectId":"extension-object","functionDeclaration":"function(){return this}"}),
        ),
        (
            "Runtime.callFunctionOn",
            json!({"executionContextId":1,"arguments":[{"objectId":"extension-object"}],"functionDeclaration":"function(x){return x}"}),
        ),
    ] {
        let count = sent.borrow().len();
        assert_eq!(
            cdp.call(method, params, Some("S")).unwrap_err(),
            REFUSAL,
            "{method}"
        );
        assert_eq!(sent.borrow().len(), count, "{method} reached backend");
    }
}

#[test]
fn all_visible_remote_handles_remain_usable() {
    let (mut cdp, sent) = client();
    cdp.call("Runtime.enable", json!({}), Some("S")).unwrap();
    cdp.call(
        "Runtime.evaluate",
        json!({"expression":"console"}),
        Some("S"),
    )
    .unwrap();
    cdp.call("DOM.resolveNode", json!({"nodeId":1}), Some("S"))
        .unwrap();
    cdp.call(
        "Runtime.getProperties",
        json!({"objectId":"page-object"}),
        Some("S"),
    )
    .unwrap();
    for id in [
        "child-object",
        "getter-object",
        "node-object",
        "console-object",
        "exception-object",
    ] {
        cdp.call("Runtime.getProperties", json!({"objectId":id}), Some("S"))
            .unwrap();
    }
    cdp.call(
        "Runtime.compileScript",
        json!({"expression":"1","executionContextId":1}),
        Some("S"),
    )
    .unwrap();
    cdp.call(
        "Runtime.runScript",
        json!({"scriptId":"page-script","executionContextId":1}),
        Some("S"),
    )
    .unwrap();
    assert_eq!(
        sent.borrow()
            .iter()
            .filter(|message| message["method"] == "Runtime.enable")
            .count(),
        1
    );
}

#[test]
fn created_page_worlds_need_no_runtime_preflight() {
    let (mut cdp, sent) = client();
    cdp.call(
        "Page.createIsolatedWorld",
        json!({"frameId":"F","worldName":"page-world"}),
        Some("S"),
    )
    .unwrap();
    cdp.call(
        "Runtime.evaluate",
        json!({"expression":"document.title","contextId":11}),
        Some("S"),
    )
    .unwrap();
    assert!(sent
        .borrow()
        .iter()
        .all(|message| message["method"] != "Runtime.enable"));
}

#[test]
fn by_value_payloads_cannot_plant_remote_object_handles() {
    let (mut cdp, _) = client();
    cdp.call("Runtime.enable", json!({}), Some("S")).unwrap();
    cdp.call(
        "Runtime.evaluate",
        json!({"expression":"spoof","returnByValue":true}),
        Some("S"),
    )
    .unwrap();
    assert_eq!(
        cdp.call(
            "Runtime.getProperties",
            json!({"objectId":"extension-object"}),
            Some("S")
        )
        .unwrap_err(),
        REFUSAL
    );
}

#[test]
fn non_runtime_payloads_cannot_allowlist_object_handles() {
    let (mut cdp, _) = client();
    cdp.call("Runtime.enable", json!({}), Some("S")).unwrap();
    cdp.call("Network.getCookies", json!({}), Some("S"))
        .unwrap();
    assert_eq!(
        cdp.call(
            "Runtime.getProperties",
            json!({"objectId":"forged-object"}),
            Some("S")
        )
        .unwrap_err(),
        UNSEEN
    );
}

#[test]
fn destroying_a_hidden_world_keeps_page_handles_but_not_stale_selectors() {
    let (mut cdp, _) = client();
    cdp.call("Runtime.enable", json!({}), Some("S")).unwrap();
    cdp.call(
        "Runtime.evaluate",
        json!({"expression":"destroy-hidden"}),
        Some("S"),
    )
    .unwrap();
    cdp.call(
        "Runtime.getProperties",
        json!({"objectId":"page-object"}),
        Some("S"),
    )
    .unwrap();
    assert_eq!(
        cdp.call("Runtime.evaluate", json!({"contextId":7}), Some("S"))
            .unwrap_err(),
        UNSEEN
    );
    assert_eq!(
        cdp.call(
            "Runtime.addBinding",
            json!({"name":"late","executionContextName":"world-7"}),
            Some("S")
        )
        .unwrap_err(),
        UNSEEN
    );
    assert_eq!(
        cdp.call("Profiler.stop", json!({}), Some("S")).unwrap_err(),
        REFUSAL
    );
}

#[test]
fn unknown_selectors_report_the_actual_reason() {
    let (mut cdp, _) = client();
    for params in [
        json!({"contextId":95}),
        json!({"uniqueContextId":"unknown"}),
        json!({"objectId":"unknown"}),
        json!({"executionContextName":"unknown"}),
    ] {
        assert_eq!(
            cdp.call("Runtime.evaluate", params, Some("S")).unwrap_err(),
            UNSEEN
        );
    }
}

#[test]
fn guessed_contexts_cannot_skip_runtime_enable_or_cross_sessions() {
    for params in [
        json!({"contextId":7}),
        json!({"uniqueContextId":"world-7"}),
        json!({"objectId":"extension-object"}),
    ] {
        let (mut cdp, sent) = client();
        assert_eq!(
            cdp.call("Runtime.evaluate", params, Some("S")).unwrap_err(),
            REFUSAL
        );
        assert!(sent
            .borrow()
            .iter()
            .all(|message| message["method"] != "Runtime.evaluate"));
    }
    let (mut cdp, _) = client();
    cdp.call("Runtime.enable", json!({}), Some("S")).unwrap();
    cdp.call(
        "Runtime.evaluate",
        json!({"expression":"document"}),
        Some("S"),
    )
    .unwrap();
    assert_eq!(
        cdp.call(
            "Runtime.getProperties",
            json!({"objectId":"page-object"}),
            Some("OTHER")
        )
        .unwrap_err(),
        UNSEEN
    );
}
