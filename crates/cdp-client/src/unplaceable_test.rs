use super::{Cdp, Transport, UNPLACEABLE};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::Instant;

const SEVEN: &str = "slicc: refused: target is on seven's origin\n";

struct Crest {
    inbox: VecDeque<String>,
    children: Vec<Value>,
    attached: usize,
}

impl Transport for Crest {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let result = match call["method"].as_str().unwrap() {
            "Target.attachToTarget" => {
                self.attached += 1;
                json!({ "sessionId": format!("S{}", self.attached) })
            }
            "Target.getTargetInfo" => json!({ "targetInfo": page() }),
            "Target.getTargets" => json!({ "targetInfos": [page()] }),
            "Page.getFrameTree" => json!({ "frameTree": {
                "frame": { "id": "T", "loaderId": "L", "url": "https://ordinary.test/", "securityOrigin": "https://ordinary.test" },
                "childFrames": self.children,
            } }),
            "Page.createIsolatedWorld" => json!({ "executionContextId": 9 }),
            "Runtime.evaluate" => json!({ "result": { "type": "string", "value": "hidden" } }),
            "Browser.getWindowForTarget" => json!({ "windowId": 1, "bounds": { "windowState": "normal" } }),
            _ => json!({}),
        };
        self.inbox.push_back(json!({ "id": call["id"], "sessionId": call["sessionId"], "result": result }).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn page() -> Value {
    json!({ "targetId": "T", "type": "page", "url": "https://ordinary.test/", "title": "", "browserContextId": "CTX" })
}

fn child(id: &str, url: &str, origin: &str) -> Value {
    json!({ "frame": { "id": id, "parentId": "T", "loaderId": format!("L{id}"), "url": url, "securityOrigin": origin } })
}

fn sandboxed() -> Value {
    child("SANDBOX", "about:blank", "null")
}

fn seven() -> Value {
    child("SEVEN", "https://seven.sliccy.ai/app", "https://seven.sliccy.ai")
}

fn ordinary() -> Value {
    child("PLAIN", "https://ordinary.test/child", "https://ordinary.test")
}

fn crest(children: Vec<Value>) -> (Cdp, String) {
    let mut cdp = Cdp::from_transport(Box::new(Crest { inbox: VecDeque::new(), children, attached: 0 }));
    let session = crate::cdp::attach(&mut cdp, "T").unwrap();
    (cdp, session)
}

fn scoped(children: Vec<Value>) -> Result<Value, String> {
    let (mut cdp, session) = crest(children);
    cdp.call("DOM.getDocument", json!({}), Some(&session))
}

fn forward(children: Vec<Value>) -> Result<(), String> {
    let (mut cdp, session) = crest(children);
    cdp.bring_forward(&session).map(|_| ())
}

fn resize(children: Vec<Value>) -> Result<Value, String> {
    let (mut cdp, session) = crest(children);
    cdp.call("Browser.setWindowBounds", json!({ "windowId": 1, "bounds": { "width": 800, "height": 600 } }), Some(&session))
}

#[test]
fn a_sandboxed_blank_frame_is_refused_as_unplaceable_not_as_sevens() {
    assert_eq!(scoped(vec![ordinary(), sandboxed()]).unwrap_err(), UNPLACEABLE);
    assert_eq!(forward(vec![ordinary(), sandboxed()]).unwrap_err(), UNPLACEABLE);
    assert_eq!(resize(vec![ordinary(), sandboxed()]).unwrap_err(), UNPLACEABLE);
    let srcdoc = child("SRCDOC", "about:srcdoc", "null");
    assert_eq!(scoped(vec![srcdoc.clone(), sandboxed()]).unwrap_err(), UNPLACEABLE);
    assert_eq!(forward(vec![srcdoc]).unwrap_err(), UNPLACEABLE);
    assert!(UNPLACEABLE.starts_with("slicc: refused: ") && !UNPLACEABLE.contains("seven"));
}

#[test]
fn an_empty_or_unproven_opaque_origin_is_unplaceable_too() {
    for origin in ["", "://"] {
        let frame = child("ODD", "https://ordinary.test/odd", origin);
        assert_eq!(scoped(vec![frame.clone()]).unwrap_err(), UNPLACEABLE, "{origin}");
        assert_eq!(forward(vec![frame]).unwrap_err(), UNPLACEABLE, "{origin}");
    }
}

#[test]
fn a_seven_frame_keeps_the_seven_text_wherever_it_sits_beside_an_unplaceable_one() {
    for children in [vec![seven()], vec![sandboxed(), seven()], vec![seven(), sandboxed()]] {
        assert_eq!(scoped(children.clone()).unwrap_err(), SEVEN);
        assert_eq!(forward(children.clone()).unwrap_err(), SEVEN);
        assert_eq!(resize(children).unwrap_err(), SEVEN);
    }
    let mut nested = sandboxed();
    let mut inner = seven();
    inner["frame"]["parentId"] = json!("SANDBOX");
    nested["childFrames"] = json!([inner]);
    assert_eq!(scoped(vec![nested.clone()]).unwrap_err(), SEVEN);
    assert_eq!(forward(vec![nested]).unwrap_err(), SEVEN);
    let unplaceable_seven = child("SEVENNULL", "https://seven.sliccy.ai/app", "null");
    assert_eq!(scoped(vec![unplaceable_seven.clone()]).unwrap_err(), SEVEN);
    assert_eq!(forward(vec![unplaceable_seven]).unwrap_err(), SEVEN);
}

#[test]
fn other_frame_faults_keep_the_old_text_and_ordinary_pages_stay_admitted() {
    let mut unloaded = ordinary();
    unloaded["frame"]["loaderId"] = json!("null");
    assert_eq!(scoped(vec![unloaded.clone()]).unwrap_err(), SEVEN);
    assert_eq!(forward(vec![unloaded]).unwrap_err(), SEVEN);
    assert!(scoped(vec![ordinary()]).is_ok());
    assert!(forward(vec![ordinary()]).is_ok());
    assert!(resize(vec![ordinary()]).is_ok());
}


#[test]
fn a_closed_connection_still_names_itself_over_an_unplaceable_frame() {
    let (mut cdp, _) = crest(vec![sandboxed()]);
    cdp.unplaceable.set(true);
    assert_eq!(cdp.document_refusal(), UNPLACEABLE);
    cdp.closed = Some("websocket closed 1006\n".to_string());
    assert_eq!(cdp.document_refusal(), "websocket closed 1006\n");
}
