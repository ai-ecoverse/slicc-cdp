use cdp_client::cdp::{Cdp, Transport};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::time::Instant;

struct Scripted {
    inner: Box<dyn Transport>,
    session: String,
    attached: bool,
    incoming: VecDeque<String>,
    pending: VecDeque<Value>,
}

impl Transport for Scripted {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let method = call["method"].as_str().unwrap();
        let result = if call["sessionId"] == "fixture-guard" {
            Some(if method == "Page.getFrameTree" {
                let frame = |id: &str| json!({"id":id,"loaderId":"L","url":"https://example.test/","securityOrigin":"https://example.test"});
                let children = ["F1","MAIN","FRAME","FCHILD"].iter().map(|id| json!({"frame":frame(id)})).collect::<Vec<_>>();
                json!({"frameTree":{"frame":frame("F"),"childFrames":children}})
            } else {
                if method == "Runtime.enable" {
                    for id in 1..=64 {
                        self.incoming.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":"fixture-guard","params":{"context":{"id":id,"uniqueId":format!("fixture-context:{id}"),"origin":"https://example.test","name":"","auxData":{"frameId":"F","isDefault":id == 1}}}}).to_string());
                    }
                }
                json!({})
            })
        } else if method == "Target.getTargetInfo" && call["params"]["targetId"] == "fixture-page" {
            Some(json!({"targetInfo":{"targetId":"fixture-page","type":"page","url":"https://example.test/"}}))
        } else if method == "Target.attachToTarget" && call["params"]["targetId"] == "fixture-page" {
            let session = if self.attached { "fixture-guard" } else { self.session.as_str() };
            self.attached = true;
            Some(json!({"sessionId":session}))
        } else if method == "Target.setDiscoverTargets" { Some(json!({})) } else { None };
        match result {
            Some(result) => { self.incoming.push_back(json!({"id":call["id"],"result":result}).to_string()); Ok(()) }
            None => { self.pending.push_back(call["id"].clone()); self.inner.send_text(text) }
        }
    }

    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
        if let Some(text) = self.incoming.pop_front() { return Ok(Some(text)) }
        let Some(text) = self.inner.recv_text(deadline)? else { return Ok(None) };
        let mut message: Value = serde_json::from_str(&text).unwrap();
        if message.get("id").is_some() {
            if let Some(id) = self.pending.pop_front() { message["id"] = id; }
        }
        if message["method"] == "Runtime.executionContextCreated" && message.pointer("/params/context/auxData/frameId").is_none() {
            message["params"]["context"]["auxData"]["frameId"] = json!("F");
        }
        Ok(Some(message.to_string()))
    }
}

pub(crate) fn client(inner: Box<dyn Transport>, session: &str) -> Cdp {
    let mut cdp = Cdp::from_transport(Box::new(Scripted {
        inner, session:session.into(), attached:false, incoming:VecDeque::new(), pending:VecDeque::new(),
    }));
    cdp.call("Target.attachToTarget", json!({"targetId":"fixture-page","flatten":true}), None).unwrap();
    cdp
}
