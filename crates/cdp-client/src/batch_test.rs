use crate::cdp::{Cdp, Transport, BATCH_LIMIT};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

const UNSEEN: &str =
    "slicc: refused: that execution context or object is not exposed to browser automation\n";

#[derive(Clone, Copy, PartialEq)]
enum Twist {
    None,
    Protected,
    Reverted,
    Error,
    Disconnect,
}

struct Page {
    sent: Rc<RefCell<Vec<Value>>>,
    incoming: VecDeque<Value>,
    objects: usize,
    twist: Twist,
    described: usize,
    closed: bool,
    url: String,
    sessions: Vec<String>,
}

impl Page {
    fn info(&self) -> Value {
        json!({"targetId":"T","type":"page","url":self.url,"title":"","attached":true,"browserContextId":"CTX"})
    }

    fn navigated(&mut self, loader: &str, url: &str) {
        let origin = url.trim_end_matches('/').to_string();
        for session in self.sessions.clone() {
            self.incoming.push_back(json!({"method":"Page.frameNavigated","sessionId":session,"params":{"type":"Navigation","frame":{"id":"T","loaderId":loader,"url":url,"securityOrigin":origin}}}));
        }
    }
}

impl Transport for Page {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        if self.closed { return Err("websocket closed 1006 \n".into()) }
        let message: Value = serde_json::from_str(text).unwrap();
        self.sent.borrow_mut().push(message.clone());
        let session = message["sessionId"].clone();
        let method = message["method"].as_str().unwrap();
        if method == "Runtime.enable" {
            self.incoming.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":1,"uniqueId":format!("world-{}", session.as_str().unwrap_or("")),"name":"","origin":"https://example.test","auxData":{"isDefault":true,"frameId":"T","type":"default"}}}}));
        }
        if method == "DOM.describeNode" {
            self.described += 1;
            if self.described == 5 {
                match self.twist {
                    Twist::Protected => {
                        self.url = "https://seven.sliccy.ai/".into();
                        let info = self.info();
                        self.incoming.push_back(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":info}}));
                    }
                    Twist::Reverted => {
                        self.navigated("L2", "https://other.test/");
                        self.navigated("L", "https://example.test/");
                    }
                    Twist::Error => {
                        self.incoming.push_back(json!({"id":message["id"],"sessionId":session,"error":{"code":-32000,"message":"Could not find node with given id"}}));
                        return Ok(());
                    }
                    Twist::Disconnect => {
                        self.closed = true;
                        return Ok(());
                    }
                    Twist::None => {}
                }
            }
        }
        let origin = self.url.trim_end_matches('/').to_string();
        let result = match method {
            "Target.attachToTarget" => {
                let id = format!("S{}", self.sessions.len() + 1);
                self.sessions.push(id.clone());
                json!({"sessionId":id})
            }
            "Target.getTargetInfo" => json!({"targetInfo":self.info()}),
            "Target.getTargets" => json!({"targetInfos":[self.info()]}),
            "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"T","loaderId":"L","url":self.url,"securityOrigin":origin}}}),
            "Page.getNavigationHistory" => json!({"currentIndex":0,"entries":[{"id":1,"url":self.url,"userTypedURL":self.url,"title":""}]}),
            "Runtime.evaluate" => json!({"result":{"type":"object","objectId":"elements"}}),
            "Runtime.getProperties" => json!({"result":(0..self.objects).map(|i| json!({"name":i.to_string(),"value":{"type":"object","subtype":"node","objectId":format!("node-{i}")}})).collect::<Vec<_>>()}),
            "DOM.describeNode" => json!({"node":{"nodeId":0,"backendNodeId":self.described,"nodeType":1,"nodeName":"BUTTON","localName":"button","nodeValue":""}}),
            _ => json!({}),
        };
        self.incoming.push_back(json!({"id":message["id"],"result":result,"sessionId":session}));
        Ok(())
    }

    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> {
        if let Some(value) = self.incoming.pop_front() { return Ok(Some(value.to_string())) }
        if self.closed { return Err("websocket closed 1006 \n".into()) }
        Ok(None)
    }
}

fn page(objects: usize, twist: Twist) -> (Cdp, Rc<RefCell<Vec<Value>>>, Vec<Value>, String) {
    let sent = Rc::new(RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(Page { sent: sent.clone(), incoming: VecDeque::new(), objects, twist, described: 0, closed: false, url: "https://example.test/".into(), sessions: Vec::new() }));
    let session = crate::cdp::attach(&mut cdp, "T").unwrap();
    cdp.call("Runtime.enable", json!({}), Some(&session)).unwrap();
    let handle = cdp.call("Runtime.evaluate", json!({"expression":"elements"}), Some(&session)).unwrap();
    let array = handle.pointer("/result/objectId").and_then(Value::as_str).unwrap().to_string();
    let properties = cdp.call("Runtime.getProperties", json!({"objectId":array,"ownProperties":true}), Some(&session)).unwrap();
    let batch = properties["result"].as_array().unwrap().iter().map(|item| json!({"objectId":item["value"]["objectId"]})).collect();
    sent.borrow_mut().clear();
    (cdp, sent, batch, session)
}

fn methods(sent: &Rc<RefCell<Vec<Value>>>) -> Vec<String> {
    sent.borrow().iter().filter_map(|message| message["method"].as_str()).filter(|method| matches!(*method, "DOM.describeNode" | "Target.getTargets")).map(str::to_string).collect()
}

fn inventories(sent: &Rc<RefCell<Vec<Value>>>) -> Vec<usize> {
    let mut runs = Vec::new();
    let mut count = 0;
    for method in methods(sent) {
        if method == "DOM.describeNode" { count += 1 } else if count > 0 { runs.push(count); count = 0 }
    }
    if count > 0 { runs.push(count) }
    runs
}

fn refused(result: Result<Vec<Value>, String>) -> String {
    let message = result.unwrap_err();
    assert!(message.starts_with("slicc: refused: "), "{message}");
    message
}

#[test]
fn a_batch_sends_every_call_between_one_check_before_and_one_after() {
    let (mut cdp, sent, batch, session) = page(20, Twist::None);
    let described = cdp.call_batch("DOM.describeNode", batch, Some(&session)).unwrap();
    assert_eq!(described.len(), 20);
    assert_eq!(described[19]["node"]["backendNodeId"], 20);
    assert_eq!(inventories(&sent), vec![20]);
    let list = methods(&sent);
    assert_eq!(list.first().map(String::as_str), Some("Target.getTargets"));
    assert_eq!(list.last().map(String::as_str), Some("Target.getTargets"));
}

#[test]
fn the_cap_is_inside_call_batch_and_each_chunk_gets_its_own_checks() {
    let (mut cdp, sent, batch, session) = page(BATCH_LIMIT, Twist::None);
    assert_eq!(cdp.call_batch("DOM.describeNode", batch, Some(&session)).unwrap().len(), BATCH_LIMIT);
    assert_eq!(inventories(&sent), vec![BATCH_LIMIT]);
    let (mut cdp, sent, batch, session) = page(BATCH_LIMIT + 1, Twist::None);
    assert_eq!(cdp.call_batch("DOM.describeNode", batch, Some(&session)).unwrap().len(), BATCH_LIMIT + 1);
    assert_eq!(inventories(&sent), vec![BATCH_LIMIT, 1]);
    let list = methods(&sent);
    let first = list.iter().position(|method| method == "DOM.describeNode").unwrap();
    let last = list.iter().rposition(|method| method == "DOM.describeNode").unwrap();
    assert_eq!(list[first..last].iter().filter(|method| *method == "Target.getTargets").count(), 2);
}

#[test]
fn a_target_that_turns_protected_mid_batch_refuses_the_whole_batch() {
    let (mut cdp, _, batch, session) = page(20, Twist::Protected);
    refused(cdp.call_batch("DOM.describeNode", batch, Some(&session)));
}

#[test]
fn a_navigation_that_reverts_before_the_post_check_refuses_the_whole_batch() {
    let (mut cdp, _, batch, session) = page(20, Twist::Reverted);
    refused(cdp.call_batch("DOM.describeNode", batch, Some(&session)));
}

#[test]
fn an_error_reply_mid_batch_fails_the_whole_batch_without_a_prefix() {
    let (mut cdp, _, batch, session) = page(20, Twist::Error);
    let message = cdp.call_batch("DOM.describeNode", batch, Some(&session)).unwrap_err();
    assert_eq!(message, "DOM.describeNode: Could not find node with given id\n");
}

#[test]
fn a_disconnect_mid_batch_fails_the_whole_batch() {
    let (mut cdp, _, batch, session) = page(20, Twist::Disconnect);
    let message = cdp.call_batch("DOM.describeNode", batch, Some(&session)).unwrap_err();
    assert!(message.contains("websocket closed"), "{message}");
}

#[test]
fn an_object_the_client_never_saw_refuses_the_batch_before_anything_is_sent() {
    let (mut cdp, sent, mut batch, session) = page(20, Twist::None);
    batch.push(json!({"objectId":"stale-object"}));
    assert_eq!(cdp.call_batch("DOM.describeNode", batch, Some(&session)).unwrap_err(), UNSEEN);
    assert!(methods(&sent).iter().all(|method| method != "DOM.describeNode"));
}
