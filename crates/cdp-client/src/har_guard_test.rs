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
                && command["params"]["filter"]
                    .as_array()
                    .is_some_and(|filter| filter.iter().any(|item| item["type"] == "worker"))
        }),
        "{after:?}"
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
