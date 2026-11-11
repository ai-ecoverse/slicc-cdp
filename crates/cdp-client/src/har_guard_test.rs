use super::*;
use crate::cdp::Transport;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

#[derive(Default)]
struct BackendState {
    inbox: VecDeque<String>,
    sent: Vec<Value>,
    refuse_auto_attach: bool,
    fail_network_enable: bool,
    early_attaches: Vec<Value>,
}

struct Backend(Rc<RefCell<BackendState>>);

impl Transport for Backend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let command: Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
        let mut state = self.0.borrow_mut();
        state.sent.push(command.clone());
        let method = command["method"].as_str().unwrap_or("");
        match method {
            "Target.setAutoAttach" => {
                let early = std::mem::take(&mut state.early_attaches);
                for attach in early {
                    state.inbox.push_back(attach.to_string());
                }
                let auto_attach = command["params"]["autoAttach"].as_bool() == Some(true);
                let filter = command["params"].get("filter").and_then(Value::as_array);
                if !auto_attach && filter.is_some_and(|items| !items.is_empty()) {
                    state.inbox.push_back(
                        json!({"id":command["id"],"error":{"message":"Target filter should be empty when disabling auto-attach"}})
                            .to_string(),
                    );
                    return Ok(());
                }
                if state.refuse_auto_attach {
                    state.inbox.push_back(
                        json!({"id":command["id"],"error":{"message":"slicc: refused: Target.setAutoAttach is not supported: attach to each target"}})
                            .to_string(),
                    );
                    return Ok(());
                }
            }
            "Network.enable" => {
                if state.fail_network_enable {
                    state.inbox.push_back(
                        json!({"id":command["id"],"error":{"message":"Network.enable failed"}})
                            .to_string(),
                    );
                    return Ok(());
                }
            }
            "Network.getResponseBody" => {
                state.inbox.push_back(
                    json!({"id":command["id"],"result":{"body":"worker-body","base64Encoded":false}})
                        .to_string(),
                );
                return Ok(());
            }
            "Runtime.runIfWaitingForDebugger" | "Target.detachFromTarget" => {}
            _ => {}
        }
        state.inbox.push_back(json!({"id":command["id"],"result":{}}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.0.borrow_mut().inbox.pop_front())
    }
}

fn client() -> (Cdp, Rc<RefCell<BackendState>>) {
    let state = Rc::new(RefCell::new(BackendState::default()));
    let mut cdp = Cdp::from_transport(Box::new(Backend(state.clone())));
    cdp.protect_origins(&["https://catalog.test"]);
    cdp.session_targets.insert("H".into(), "T".into());
    cdp.guards.insert("T".into(), "G".into());
    cdp.guard_sessions.insert("G".into());
    cdp.targets.insert(
        "T".into(),
        json!({"targetId":"T","type":"page","url":"https://ordinary.test/"}),
    );
    cdp.main_commit(
        "G",
        &json!({"id":"F","loaderId":"L","url":"https://ordinary.test/","securityOrigin":"https://ordinary.test"}),
    );
    (cdp, state)
}

fn worker_attach(owner: &str, session: &str, url: &str, waiting: bool) -> Value {
    json!({
        "method": "Target.attachedToTarget",
        "sessionId": owner,
        "params": {
            "sessionId": session,
            "targetInfo": {"targetId": format!("W-{session}"), "type": "worker", "url": url},
            "waitingForDebugger": waiting
        }
    })
}

fn methods_after(state: &Rc<RefCell<BackendState>>, before: usize) -> Vec<String> {
    state.borrow().sent[before..]
        .iter()
        .filter_map(|command| command["method"].as_str().map(str::to_string))
        .collect()
}

fn drain(cdp: &mut Cdp, state: &Rc<RefCell<BackendState>>) {
    while state.borrow().inbox.front().is_some_and(|message| {
        serde_json::from_str::<Value>(message).ok().is_some_and(|value| value.get("method").is_some())
    }) {
        let message = state.borrow_mut().inbox.pop_front().unwrap();
        let value = serde_json::from_str(&message).unwrap();
        cdp.absorb(value);
    }
}

#[test]
fn admitted_worker_enables_network_before_run_if_waiting() {
    let (mut cdp, state) = client();
    cdp.har_auto_attach("H").unwrap();
    assert!(cdp.har.armed.contains("H"));
    assert!(
        state.borrow().sent.iter().any(|command| {
            command["method"] == "Target.setAutoAttach"
                && command["params"]["waitForDebuggerOnStart"] == true
        }),
        "{:?}",
        state.borrow().sent
    );
    let before = state.borrow().sent.len();
    cdp.absorb(worker_attach(
        "H",
        "WORKER",
        "https://ordinary.test/worker.js",
        true,
    ));
    drain(&mut cdp, &state);
    assert!(cdp.har_owns("WORKER"));
    let methods = methods_after(&state, before);
    let enable = methods.iter().position(|method| method == "Network.enable");
    let resume = methods
        .iter()
        .position(|method| method == "Runtime.runIfWaitingForDebugger");
    assert!(enable.is_some(), "{methods:?}");
    assert!(resume.is_some(), "{methods:?}");
    assert!(enable.unwrap() < resume.unwrap(), "{methods:?}");
    assert!(
        !cdp.events().iter().any(|event| event["method"] == "Target.attachedToTarget"
            && event["params"]["sessionId"] == "WORKER"),
        "admitted worker attach must be consumed"
    );
}

#[test]
fn already_running_worker_gets_network_enable_without_resume() {
    let (mut cdp, state) = client();
    cdp.har_auto_attach("H").unwrap();
    let before = state.borrow().sent.len();
    cdp.absorb(worker_attach(
        "H",
        "RUNNING",
        "https://ordinary.test/worker.js",
        false,
    ));
    assert!(cdp.har_owns("RUNNING"));
    let methods = methods_after(&state, before);
    assert!(methods.iter().any(|method| method == "Network.enable"), "{methods:?}");
    assert!(
        !methods
            .iter()
            .any(|method| method == "Runtime.runIfWaitingForDebugger"),
        "{methods:?}"
    );
}

#[test]
fn early_attaches_during_set_auto_attach_are_consumed_and_judged() {
    let (mut cdp, state) = client();
    state.borrow_mut().early_attaches = vec![
        worker_attach("H", "EARLY", "https://ordinary.test/worker.js", false),
        worker_attach("H", "WAITING", "https://ordinary.test/other.js", true),
        worker_attach("H", "BAD", "https://seven.sliccy.ai/worker.js", true),
    ];
    let before = state.borrow().sent.len();
    cdp.har_auto_attach("H").unwrap();
    assert!(cdp.har.armed.contains("H"));
    assert!(cdp.har_owns("EARLY"));
    assert!(cdp.har_owns("WAITING"));
    assert!(!cdp.har_owns("BAD"));
    let methods = methods_after(&state, before);
    let early_enable = methods.iter().position(|method| method == "Network.enable");
    assert!(early_enable.is_some(), "{methods:?}");
    assert!(
        state.borrow().sent[before..].iter().any(|command| {
            command["method"] == "Network.enable" && command["sessionId"] == "EARLY"
        }),
        "{:?}",
        &state.borrow().sent[before..]
    );
    assert!(
        !state.borrow().sent[before..].iter().any(|command| {
            command["method"] == "Runtime.runIfWaitingForDebugger" && command["sessionId"] == "EARLY"
        }),
        "{:?}",
        &state.borrow().sent[before..]
    );
    let waiting_enable = state.borrow().sent[before..]
        .iter()
        .position(|command| command["method"] == "Network.enable" && command["sessionId"] == "WAITING");
    let waiting_resume = state.borrow().sent[before..]
        .iter()
        .position(|command| {
            command["method"] == "Runtime.runIfWaitingForDebugger" && command["sessionId"] == "WAITING"
        });
    assert!(waiting_enable.is_some() && waiting_resume.is_some(), "{methods:?}");
    assert!(waiting_enable.unwrap() < waiting_resume.unwrap(), "{methods:?}");
    assert!(
        state.borrow().sent[before..].iter().any(|command| {
            command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "BAD"
        }),
        "{:?}",
        &state.borrow().sent[before..]
    );
    assert!(
        !cdp.events().iter().any(|event| event["method"] == "Target.attachedToTarget"
            && matches!(
                event["params"]["sessionId"].as_str(),
                Some("EARLY" | "WAITING" | "BAD")
            )),
        "early attaches must be consumed"
    );
}

#[test]
fn refused_set_auto_attach_releases_early_workers_and_disarms() {
    let (mut cdp, state) = client();
    state.borrow_mut().refuse_auto_attach = true;
    state.borrow_mut().early_attaches = vec![
        worker_attach("H", "EARLY", "https://ordinary.test/worker.js", false),
        worker_attach("H", "WAITING", "https://ordinary.test/other.js", true),
    ];
    cdp.har_auto_attach("H").unwrap();
    assert!(!cdp.har.armed.contains("H"));
    assert!(!cdp.har_owns("EARLY"));
    assert!(!cdp.har_owns("WAITING"));
    assert!(
        state.borrow().sent.iter().any(|command| {
            command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "EARLY"
        })
    );
    assert!(
        state.borrow().sent.iter().any(|command| {
            command["method"] == "Target.detachFromTarget"
                && command["params"]["sessionId"] == "WAITING"
        })
    );
    let notices = cdp.take_notices().join("");
    assert!(notices.contains("dedicated worker traffic is not captured"), "{notices}");
}

#[test]
fn worker_of_protected_page_is_detached_without_enable_or_resume() {
    let (mut cdp, state) = client();
    cdp.har_auto_attach("H").unwrap();
    cdp.blocked.insert("H".into());
    let before = state.borrow().sent.len();
    cdp.absorb(worker_attach(
        "H",
        "WORKER",
        "https://ordinary.test/worker.js",
        true,
    ));
    assert!(!cdp.har_owns("WORKER"));
    let methods = methods_after(&state, before);
    assert!(!methods.iter().any(|method| method == "Network.enable"), "{methods:?}");
    assert!(
        !methods
            .iter()
            .any(|method| method == "Runtime.runIfWaitingForDebugger"),
        "{methods:?}"
    );
    assert!(
        state.borrow().sent[before..]
            .iter()
            .any(|command| command["method"] == "Target.detachFromTarget"
                && command["params"]["sessionId"] == "WORKER"),
        "{:?}",
        &state.borrow().sent[before..]
    );
}

#[test]
fn blob_worker_of_protected_page_is_detached_without_network_enable() {
    let (mut cdp, state) = client();
    cdp.har_auto_attach("H").unwrap();
    cdp.protected_targets.insert("T".into());
    let before = state.borrow().sent.len();
    cdp.absorb(worker_attach(
        "H",
        "BLOB",
        "blob:https://ordinary.test/1",
        true,
    ));
    assert!(!cdp.har_owns("BLOB"));
    let methods = methods_after(&state, before);
    assert!(!methods.iter().any(|method| method == "Network.enable"));
    assert!(
        !methods
            .iter()
            .any(|method| method == "Runtime.runIfWaitingForDebugger")
    );
    assert!(
        state.borrow().sent[before..]
            .iter()
            .any(|command| command["method"] == "Target.detachFromTarget"
                && command["params"]["sessionId"] == "BLOB")
    );
}

#[test]
fn blob_worker_of_ordinary_page_is_admitted() {
    let (mut cdp, state) = client();
    cdp.har_auto_attach("H").unwrap();
    let before = state.borrow().sent.len();
    cdp.absorb(worker_attach(
        "H",
        "BLOB",
        "blob:https://ordinary.test/1",
        true,
    ));
    assert!(cdp.har_owns("BLOB"));
    let methods = methods_after(&state, before);
    let enable = methods.iter().position(|method| method == "Network.enable");
    let resume = methods
        .iter()
        .position(|method| method == "Runtime.runIfWaitingForDebugger");
    assert!(enable.is_some() && resume.is_some() && enable.unwrap() < resume.unwrap(), "{methods:?}");
}

#[test]
fn failed_network_enable_still_releases_the_worker() {
    let (mut cdp, state) = client();
    state.borrow_mut().fail_network_enable = true;
    cdp.har_auto_attach("H").unwrap();
    let before = state.borrow().sent.len();
    cdp.absorb(worker_attach(
        "H",
        "WORKER",
        "https://ordinary.test/worker.js",
        true,
    ));
    assert!(!cdp.har_owns("WORKER"));
    let methods = methods_after(&state, before);
    assert!(methods.iter().any(|method| method == "Network.enable"), "{methods:?}");
    assert!(
        !methods
            .iter()
            .any(|method| method == "Runtime.runIfWaitingForDebugger"),
        "{methods:?}"
    );
    assert!(
        state.borrow().sent[before..]
            .iter()
            .any(|command| command["method"] == "Target.detachFromTarget"
                && command["params"]["sessionId"] == "WORKER"),
        "{:?}",
        &state.borrow().sent[before..]
    );
}

#[test]
fn attach_on_unarmed_session_is_not_consumed() {
    let (mut cdp, state) = client();
    cdp.session_targets.insert("OTHER".into(), "U".into());
    cdp.targets.insert(
        "U".into(),
        json!({"targetId":"U","type":"page","url":"https://ordinary.test/other"}),
    );
    cdp.har_auto_attach("H").unwrap();
    let before = cdp.events().len();
    cdp.absorb(worker_attach(
        "OTHER",
        "FOREIGN",
        "https://ordinary.test/worker.js",
        true,
    ));
    assert!(!cdp.har_owns("FOREIGN"));
    assert!(
        !state.borrow().sent.iter().any(|command| {
            command["method"] == "Network.enable" && command["sessionId"] == "FOREIGN"
        })
    );
    assert!(
        cdp.events()[before..]
            .iter()
            .any(|event| event["method"] == "Target.attachedToTarget"
                && event["params"]["sessionId"] == "FOREIGN"),
        "unarmed attach must reach the normal event path: {:?}",
        &cdp.events()[before..]
    );
}

#[test]
fn extension_style_refusal_emits_notice_and_leaves_recording_possible() {
    let (mut cdp, state) = client();
    state.borrow_mut().refuse_auto_attach = true;
    cdp.har_auto_attach("H").unwrap();
    assert!(!cdp.har.armed.contains("H"));
    let notices = cdp.take_notices().join("");
    assert!(notices.contains("dedicated worker traffic is not captured"), "{notices}");
    cdp.absorb(worker_attach(
        "H",
        "WORKER",
        "https://ordinary.test/worker.js",
        true,
    ));
    assert!(!cdp.har_owns("WORKER"));
}

#[test]
fn stop_releases_all_workers_and_disables_auto_attach() {
    let (mut cdp, state) = client();
    cdp.har_auto_attach("H").unwrap();
    cdp.absorb(worker_attach(
        "H",
        "WORKER",
        "https://ordinary.test/worker.js",
        true,
    ));
    cdp.absorb(worker_attach(
        "H",
        "WORKER-2",
        "https://ordinary.test/other.js",
        true,
    ));
    assert!(cdp.har_owns("WORKER"));
    assert!(cdp.har_owns("WORKER-2"));
    let before = state.borrow().sent.len();
    cdp.har_stop("H");
    assert!(!cdp.har_owns("WORKER"));
    assert!(!cdp.har_owns("WORKER-2"));
    assert!(!cdp.har.armed.contains("H"));
    let after = &state.borrow().sent[before..];
    assert!(
        after.iter().any(|command| {
            command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "WORKER"
        }),
        "{after:?}"
    );
    assert!(
        after.iter().any(|command| {
            command["method"] == "Target.detachFromTarget"
                && command["params"]["sessionId"] == "WORKER-2"
        }),
        "{after:?}"
    );
    assert!(
        after.iter().any(|command| {
            command["method"] == "Target.setAutoAttach"
                && command["params"]["autoAttach"] == false
                && command["sessionId"] == "H"
                && !command["params"]
                    .get("filter")
                    .and_then(Value::as_array)
                    .is_some_and(|filter| !filter.is_empty())
        }),
        "{after:?}"
    );
}

#[test]
fn after_stop_a_new_worker_is_never_stranded() {
    let (mut cdp, state) = client();
    cdp.har_auto_attach("H").unwrap();
    let before_stop = state.borrow().sent.len();
    cdp.har_stop("H");
    assert!(!cdp.har.armed.contains("H"));
    assert!(
        state.borrow().sent[before_stop..].iter().any(|command| {
            command["method"] == "Target.setAutoAttach"
                && command["params"]["autoAttach"] == false
                && !command["params"]
                    .get("filter")
                    .and_then(Value::as_array)
                    .is_some_and(|filter| !filter.is_empty())
        }),
        "{:?}",
        &state.borrow().sent[before_stop..]
    );
    let before = state.borrow().sent.len();
    cdp.absorb(worker_attach(
        "H",
        "LATE",
        "https://ordinary.test/worker.js",
        true,
    ));
    assert!(!cdp.har_owns("LATE"));
    let methods = methods_after(&state, before);
    assert!(!methods.iter().any(|method| method == "Network.enable"), "{methods:?}");
    assert!(
        !methods
            .iter()
            .any(|method| method == "Runtime.runIfWaitingForDebugger"),
        "{methods:?}"
    );
    assert!(
        !methods
            .iter()
            .any(|method| method == "Target.detachFromTarget"),
        "{methods:?}"
    );
}

#[test]
fn protected_blob_worker_of_ordinary_page_is_refused() {
    let (mut cdp, state) = client();
    cdp.har_auto_attach("H").unwrap();
    let before = state.borrow().sent.len();
    cdp.absorb(worker_attach(
        "H",
        "BLOB",
        "blob:https://lars.sliccy.ai/x",
        true,
    ));
    assert!(!cdp.har_owns("BLOB"));
    let methods = methods_after(&state, before);
    assert!(!methods.iter().any(|method| method == "Network.enable"), "{methods:?}");
    assert!(
        !methods
            .iter()
            .any(|method| method == "Runtime.runIfWaitingForDebugger"),
        "{methods:?}"
    );
    assert!(
        state.borrow().sent[before..]
            .iter()
            .any(|command| command["method"] == "Target.detachFromTarget"
                && command["params"]["sessionId"] == "BLOB"),
        "{:?}",
        &state.borrow().sent[before..]
    );
}

#[test]
fn unknown_worker_url_fails_closed() {
    let (mut cdp, state) = client();
    cdp.har_auto_attach("H").unwrap();
    let before = state.borrow().sent.len();
    cdp.absorb(worker_attach("H", "EMPTY", "", true));
    assert!(!cdp.har_owns("EMPTY"));
    let methods = methods_after(&state, before);
    assert!(!methods.iter().any(|method| method == "Network.enable"));
    assert!(
        !methods
            .iter()
            .any(|method| method == "Runtime.runIfWaitingForDebugger")
    );
}

#[test]
fn client_issued_auto_attach_remains_refused() {
    let (mut cdp, _state) = client();
    let refused = cdp.call(
        "Target.setAutoAttach",
        json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true,"filter":[{"type":"worker","exclude":false}]}),
        Some("H"),
    );
    assert!(refused.as_ref().is_err_and(|message| message.contains("auto-attach")), "{refused:?}");
}

fn admit_worker(cdp: &mut Cdp, state: &Rc<RefCell<BackendState>>, session: &str) {
    cdp.har_auto_attach("H").unwrap();
    cdp.absorb(worker_attach(
        "H",
        session,
        "https://ordinary.test/worker.js",
        true,
    ));
    drain(cdp, state);
    assert!(cdp.har_owns(session));
}

fn worker_fetch(cdp: &mut Cdp, session: &str, id: &str, url: &str) {
    worker_fetch_with(cdp, session, id, url, json!({}), None);
}

fn worker_fetch_with(
    cdp: &mut Cdp,
    session: &str,
    id: &str,
    url: &str,
    headers: Value,
    initiator: Option<Value>,
) {
    let mut params = json!({
        "requestId": id,
        "loaderId": "",
        "documentURL": "https://ordinary.test/worker.js",
        "type": "Fetch",
        "request": {"url": url, "method": "GET", "headers": headers}
    });
    if let Some(initiator) = initiator {
        params["initiator"] = initiator;
    }
    cdp.absorb(json!({
        "method": "Network.requestWillBeSent",
        "sessionId": session,
        "params": params
    }));
    cdp.absorb(json!({
        "method": "Network.responseReceived",
        "sessionId": session,
        "params": {
            "requestId": id,
            "response": {"url": url, "status": 200, "headers": {}, "mimeType": "text/plain"}
        }
    }));
    cdp.absorb(json!({
        "method": "Network.loadingFinished",
        "sessionId": session,
        "params": {"requestId": id, "timestamp": 1.2}
    }));
}

fn assert_worker_body_refused(cdp: &mut Cdp, session: &str, id: &str) {
    assert_eq!(cdp.request_state(id), Some(true), "{id}");
    assert!(!cdp.ordinary_response_body(session, id), "{id}");
    assert!(
        cdp.call("Network.getResponseBody", json!({"requestId": id}), Some(session))
            .unwrap_err()
            .starts_with("slicc: refused:"),
        "{id}"
    );
}

#[test]
fn ordinary_worker_fetch_body_is_readable() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    worker_fetch(&mut cdp, "WORKER", "W1", "https://ordinary.test/body");
    assert_eq!(cdp.request_state("W1"), Some(false));
    assert!(cdp.ordinary_response_body("WORKER", "W1"));
    assert_eq!(
        cdp.call("Network.getResponseBody", json!({"requestId":"W1"}), Some("WORKER"))
            .unwrap()["body"],
        "worker-body"
    );
}

#[test]
fn protected_worker_fetch_body_stays_refused() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    worker_fetch(&mut cdp, "WORKER", "W2", "https://catalog.test/secret");
    assert_eq!(cdp.request_state("W2"), Some(true));
    assert!(!cdp.ordinary_response_body("WORKER", "W2"));
    assert!(cdp
        .call("Network.getResponseBody", json!({"requestId":"W2"}), Some("WORKER"))
        .unwrap_err()
        .starts_with("slicc: refused:"));
}

#[test]
fn unrecorded_worker_request_body_stays_refused() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    assert_eq!(cdp.request_state("MISSING"), None);
    assert!(!cdp.ordinary_response_body("WORKER", "MISSING"));
    assert!(cdp
        .call(
            "Network.getResponseBody",
            json!({"requestId":"MISSING"}),
            Some("WORKER")
        )
        .unwrap_err()
        .starts_with("slicc: refused:"));
}

#[test]
fn worker_bodies_stop_after_creator_becomes_protected() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    worker_fetch(&mut cdp, "WORKER", "BEFORE", "https://ordinary.test/before");
    assert!(cdp.ordinary_response_body("WORKER", "BEFORE"));
    assert_eq!(
        cdp.call(
            "Network.getResponseBody",
            json!({"requestId":"BEFORE"}),
            Some("WORKER")
        )
        .unwrap()["body"],
        "worker-body"
    );
    cdp.protected_targets.insert("T".into());
    worker_fetch(&mut cdp, "WORKER", "AFTER", "https://ordinary.test/after");
    assert_eq!(cdp.request_state("AFTER"), Some(true));
    assert!(!cdp.ordinary_response_body("WORKER", "AFTER"));
    assert!(!cdp.ordinary_response_body("WORKER", "BEFORE"));
    assert!(cdp
        .call(
            "Network.getResponseBody",
            json!({"requestId":"AFTER"}),
            Some("WORKER")
        )
        .unwrap_err()
        .starts_with("slicc: refused:"));
}

#[test]
fn worker_referer_header_marks_request_protected() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    worker_fetch_with(
        &mut cdp,
        "WORKER",
        "REF",
        "https://ordinary.test/body",
        json!({"Referer": "https://seven.sliccy.ai/secret"}),
        None,
    );
    assert_worker_body_refused(&mut cdp, "WORKER", "REF");
}

#[test]
fn worker_origin_header_marks_request_protected() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    worker_fetch_with(
        &mut cdp,
        "WORKER",
        "ORG",
        "https://ordinary.test/body",
        json!({"Origin": "https://seven.sliccy.ai"}),
        None,
    );
    assert_worker_body_refused(&mut cdp, "WORKER", "ORG");
}

#[test]
fn worker_initiator_stack_marks_request_protected() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    worker_fetch_with(
        &mut cdp,
        "WORKER",
        "STACK",
        "https://ordinary.test/body",
        json!({}),
        Some(json!({
            "type": "script",
            "stack": {
                "callFrames": [{
                    "url": "https://seven.sliccy.ai/x.js",
                    "lineNumber": 0,
                    "columnNumber": 0
                }]
            }
        })),
    );
    assert_worker_body_refused(&mut cdp, "WORKER", "STACK");
}

#[test]
fn worker_extra_info_referer_and_origin_mark_request_protected() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    cdp.absorb(json!({
        "method": "Network.requestWillBeSent",
        "sessionId": "WORKER",
        "params": {
            "requestId": "EXTRA",
            "loaderId": "",
            "documentURL": "https://ordinary.test/worker.js",
            "type": "Fetch",
            "request": {"url": "https://ordinary.test/body", "method": "GET", "headers": {}}
        }
    }));
    assert_eq!(cdp.request_state("EXTRA"), Some(false));
    assert!(cdp.ordinary_response_body("WORKER", "EXTRA"));
    cdp.absorb(json!({
        "method": "Network.requestWillBeSentExtraInfo",
        "sessionId": "WORKER",
        "params": {
            "requestId": "EXTRA",
            "headers": {"Referer": "https://seven.sliccy.ai/secret"}
        }
    }));
    assert_eq!(cdp.request_state("EXTRA"), Some(true));
    assert!(!cdp.ordinary_response_body("WORKER", "EXTRA"));
    cdp.absorb(json!({
        "method": "Network.responseReceivedExtraInfo",
        "sessionId": "WORKER",
        "params": {
            "requestId": "EXTRA2",
            "statusCode": 200,
            "headers": {"Origin": "https://catalog.test/"}
        }
    }));
    cdp.absorb(json!({
        "method": "Network.requestWillBeSent",
        "sessionId": "WORKER",
        "params": {
            "requestId": "EXTRA2",
            "loaderId": "",
            "documentURL": "https://ordinary.test/worker.js",
            "type": "Fetch",
            "request": {"url": "https://ordinary.test/body2", "method": "GET", "headers": {}}
        }
    }));
    assert_worker_body_refused(&mut cdp, "WORKER", "EXTRA2");
    cdp.absorb(json!({
        "method": "Network.requestWillBeSent",
        "sessionId": "WORKER",
        "params": {
            "requestId": "EXTRA3",
            "loaderId": "",
            "documentURL": "https://ordinary.test/worker.js",
            "type": "Fetch",
            "request": {"url": "https://ordinary.test/body3", "method": "GET", "headers": {}}
        }
    }));
    cdp.absorb(json!({
        "method": "Network.responseReceived",
        "sessionId": "WORKER",
        "params": {
            "requestId": "EXTRA3",
            "response": {"url": "https://ordinary.test/body3", "status": 200, "headers": {}, "mimeType": "text/plain"}
        }
    }));
    assert_eq!(cdp.request_state("EXTRA3"), Some(false));
    cdp.absorb(json!({
        "method": "Network.responseReceivedExtraInfo",
        "sessionId": "WORKER",
        "params": {
            "requestId": "EXTRA3",
            "statusCode": 200,
            "headers": {"Origin": "https://catalog.test/"}
        }
    }));
    assert_eq!(cdp.request_state("EXTRA3"), Some(true));
    assert!(!cdp.ordinary_response_body("WORKER", "EXTRA3"));
}

#[test]
fn worker_redirect_response_referer_marks_request_protected() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    cdp.absorb(json!({
        "method": "Network.requestWillBeSent",
        "sessionId": "WORKER",
        "params": {
            "requestId": "REDIRRESP",
            "loaderId": "",
            "documentURL": "https://ordinary.test/worker.js",
            "type": "Fetch",
            "request": {"url": "https://ordinary.test/next", "method": "GET", "headers": {}},
            "redirectResponse": {
                "url": "https://ordinary.test/start",
                "status": 302,
                "headers": {"Referer": "https://seven.sliccy.ai/secret"}
            }
        }
    }));
    assert_worker_body_refused(&mut cdp, "WORKER", "REDIRRESP");
}

#[test]
fn worker_headers_text_referer_marks_request_protected() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    cdp.absorb(json!({
        "method": "Network.requestWillBeSent",
        "sessionId": "WORKER",
        "params": {
            "requestId": "TEXT",
            "loaderId": "",
            "documentURL": "https://ordinary.test/worker.js",
            "type": "Fetch",
            "request": {
                "url": "https://ordinary.test/body",
                "method": "GET",
                "headers": {},
                "headersText": "GET /body HTTP/1.1\r\nReferer: https://seven.sliccy.ai/secret\r\n\r\n"
            }
        }
    }));
    assert_worker_body_refused(&mut cdp, "WORKER", "TEXT");
}

#[test]
fn worker_extra_info_after_loading_finished_still_blocks_body() {
    let (mut cdp, state) = client();
    admit_worker(&mut cdp, &state, "WORKER");
    worker_fetch(&mut cdp, "WORKER", "LATE", "https://ordinary.test/body");
    assert_eq!(cdp.request_state("LATE"), Some(false));
    assert!(cdp.ordinary_response_body("WORKER", "LATE"));
    cdp.absorb(json!({
        "method": "Network.requestWillBeSentExtraInfo",
        "sessionId": "WORKER",
        "params": {
            "requestId": "LATE",
            "headers": {"Referer": "https://seven.sliccy.ai/secret"}
        }
    }));
    assert_eq!(cdp.request_state("LATE"), Some(true));
    assert!(!cdp.ordinary_response_body("WORKER", "LATE"));
    assert!(cdp
        .call(
            "Network.getResponseBody",
            json!({"requestId": "LATE"}),
            Some("WORKER")
        )
        .unwrap_err()
        .starts_with("slicc: refused:"));
}
