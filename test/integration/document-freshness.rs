use cdp_client::cdp::{Cdp, Transport};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

const ORDINARY: &str = "https://ordinary.example/current";
const PROTECTED: &str = "https://seven.sliccy.ai/document";

struct Backend {
    current: String,
    sessions: Vec<String>,
    calls: Vec<Value>,
    inbox: VecDeque<String>,
    delayed: VecDeque<String>,
}

impl Backend {
    fn info(&self) -> Value {
        json!({"targetId":"T","type":"page","url":ORDINARY})
    }

    fn frame(&self) -> Value {
        json!({"id":"F","loaderId":"L","url":self.current,"securityOrigin":self.current.split("/document").next().unwrap()})
    }

    fn commit(&mut self, url: &str, delayed: bool) {
        self.current = url.into();
        for session in &self.sessions {
            let event = json!({"method":"Page.frameNavigated","sessionId":session,"params":{"type":"BackForwardCacheRestore","frame":self.frame()}}).to_string();
            if delayed {
                self.delayed.push_back(event);
            } else {
                self.inbox.push_back(event);
            }
        }
    }
}

struct Fake(Rc<RefCell<Backend>>);

impl Transport for Fake {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut backend = self.0.borrow_mut();
        backend.calls.push(call.clone());
        let result = match call["method"].as_str().unwrap() {
            "Target.getTargetInfo" => json!({"targetInfo":backend.info()}),
            "Target.attachToTarget" => {
                let session = format!("S{}", backend.sessions.len() + 1);
                backend.sessions.push(session.clone());
                json!({"sessionId":session})
            }
            "Page.getFrameTree" => json!({"frameTree":{"frame":backend.frame()}}),
            "Page.getNavigationHistory" => json!({"currentIndex":if backend.current == PROTECTED { 0 } else { 1 },"entries":[{"id":1,"url":PROTECTED},{"id":2,"url":ORDINARY}]}),
            "Runtime.evaluate" if call["params"]["expression"] == "history.back()" => {
                backend.commit(PROTECTED, true);
                json!({"result":{"type":"undefined"}})
            }
            "Runtime.evaluate" => json!({"result":{"type":"string","value":"document-value"}}),
            "DOM.getDocument" => json!({"root":{"nodeId":1,"nodeName":"document-value"}}),
            "Page.printToPDF" => json!({"stream":"H"}),
            _ => json!({}),
        };
        backend.inbox.push_back(json!({"id":call["id"],"result":result}).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.0.borrow_mut().inbox.pop_front())
    }
}

fn attach(cdp: &mut Cdp) -> String {
    cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap()["sessionId"].as_str().unwrap().into()
}

fn drain(cdp: &mut Cdp) {
    while cdp.pump(Instant::now()).unwrap() {}
}

fn commands() -> Vec<(&'static str, Value)> {
    vec![
        ("Runtime.evaluate", json!({"expression":"document.documentElement.innerHTML"})),
        ("DOM.getDocument", json!({"depth":-1,"pierce":true})),
        ("Input.dispatchKeyEvent", json!({"type":"keyDown","key":"Enter"})),
        ("Runtime.getHeapUsage", json!({})),
        ("DOMSnapshot.captureSnapshot", json!({"computedStyles":[]})),
        ("Accessibility.getFullAXTree", json!({})),
        ("Page.captureScreenshot", json!({})),
        ("Page.printToPDF", json!({})),
        ("Page.getLayoutMetrics", json!({})),
        ("Page.getResourceContent", json!({"frameId":"F","url":ORDINARY})),
        ("IO.read", json!({"handle":"H"})),
        ("IO.close", json!({"handle":"H"})),
    ]
}

#[test]
fn current_document_is_verified_before_content_commands_after_history_navigation() {
    for before_attach in [false, true] {
        for notify in [false, true] {
            for (method, params) in commands() {
                let backend = Rc::new(RefCell::new(Backend {
                    current: PROTECTED.into(), sessions: Vec::new(), calls: Vec::new(), inbox: VecDeque::new(), delayed: VecDeque::new(),
                }));
                backend.borrow_mut().commit(ORDINARY, false);
                let mut cdp = Cdp::from_transport(Box::new(Fake(backend.clone())));
                let mut session = attach(&mut cdp);
                if !before_attach {
                    assert!(cdp.call("Page.navigate", json!({"url":PROTECTED}), Some(&session)).is_err());
                    backend.borrow_mut().commit(PROTECTED, false);
                    drain(&mut cdp);
                    assert!(cdp.session_blocked(&session));
                    backend.borrow_mut().commit(ORDINARY, false);
                    drain(&mut cdp);
                    session = attach(&mut cdp);
                }
                cdp.call("Page.printToPDF", json!({"transferMode":"ReturnAsStream"}), Some(&session)).unwrap();
                let history_from = backend.borrow().calls.len();
                let history = cdp.call("Runtime.evaluate", json!({"expression":"history.back()"}), Some(&session));
                assert!(history.is_ok() || history.is_err_and(|message| message.starts_with("slicc: refused:")));
                assert_eq!(backend.borrow().current, PROTECTED);
                assert!(!backend.borrow().delayed.is_empty());
                let from = backend.borrow().calls.len();
                let command_session = (!method.starts_with("IO.")).then_some(session.as_str());
                let result = if notify {
                    cdp.notify(method, params, command_session).map(|_| json!({}))
                } else {
                    cdp.call(method, params, command_session)
                };
                {
                    let mut state = backend.borrow_mut();
                    while let Some(event) = state.delayed.pop_front() {
                        state.inbox.push_back(event);
                    }
                }
                drain(&mut cdp);
                assert!(result.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")), "{method}: before_attach={before_attach}, notify={notify}: {result:?}");
                assert!(!backend.borrow().calls[from..].iter().any(|call| call["method"] == method && call["sessionId"].as_str() == command_session), "the client command reached the current document");
                assert!(backend.borrow().calls[history_from..].iter().any(|call| call["method"] == "Page.getFrameTree" && call["sessionId"] != session), "the current document was not checked on the guard session");
            }
        }
    }
}
