use crate::cdp::Transport;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

struct FrameInspector {
    queue: VecDeque<String>,
    calls: Rc<RefCell<Vec<Value>>>,
    next: usize,
    sessions: BTreeMap<String, String>,
}

impl Transport for FrameInspector {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let command: Value = serde_json::from_str(text).unwrap();
        self.calls.borrow_mut().push(command.clone());
        let session = command["sessionId"].as_str().unwrap_or("");
        let target = self.sessions.get(session).map(String::as_str).unwrap_or("FRAME_TARGET");
        let frame = if target == "ROOT" {
            json!({"id":"PARENT","url":"https://ordinary.test/","securityOrigin":"https://ordinary.test","loaderId":"PARENT_LOADER"})
        } else {
            json!({"id":"FRAME_TARGET","parentId":"PARENT","url":"https://ordinary.test/","securityOrigin":"https://ordinary.test","loaderId":"LOADER"})
        };
        let root = json!({"targetId":"ROOT","type":"page","url":"https://ordinary.test/"});
        let child = json!({"targetId":"FRAME_TARGET","type":"iframe","parentFrameId":"PARENT","url":"https://ordinary.test/"});
        let result = match command["method"].as_str().unwrap() {
            "Target.getTargetInfo" => json!({"targetInfo":if command["params"]["targetId"] == "ROOT" {root} else {child}}),
            "Target.getTargets" => json!({"targetInfos":[root,child]}),
            "Target.attachToTarget" => {
                self.next += 1;
                let session = format!("S{}",self.next);
                self.sessions.insert(session.clone(),command["params"]["targetId"].as_str().unwrap().into());
                json!({"sessionId":session})
            },
            "Page.getFrameTree" => json!({"frameTree":{"frame":frame}}),
            "Runtime.enable" | "Page.createIsolatedWorld" => {
                let isolated = command["method"] == "Page.createIsolatedWorld";
                self.queue.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":if isolated {9} else {7},"uniqueId":format!("{}-{}-{}",if isolated {"isolated"} else {"main"},frame["id"].as_str().unwrap(),frame["loaderId"].as_str().unwrap()),"origin":"https://ordinary.test","auxData":{"frameId":frame["id"],"isDefault":!isolated}}}}).to_string());
                if isolated { json!({"executionContextId":9}) } else { json!({}) }
            }
            "Runtime.evaluate" => json!({"result":{"type":"boolean","value":true}}),
            _ => json!({}),
        };
        self.queue.push_back(json!({"id":command["id"],"result":result}).to_string());
        Ok(())
    }
    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { Ok(self.queue.pop_front()) }
}

#[test]
fn newly_attached_frame_requires_default_context_observation_before_scope_selection() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(FrameInspector { queue:VecDeque::new(), calls:calls.clone(), next:0, sessions:BTreeMap::new() }));
    let root = cdp.call("Target.attachToTarget",json!({"targetId":"ROOT","flatten":true}),None).unwrap();
    let frame = FrameRec { frame_id:"FRAME_TARGET".into(), parent_frame_id:Some("PARENT".into()), url:"https://ordinary.test/".into(), target:Some("FRAME_TARGET".into()) };
    let mut scopes=BTreeMap::new();
    let mut opened=Vec::new();
    build_scope(&mut cdp,&Session::default(),"ROOT",root["sessionId"].as_str().unwrap(),&frame,&mut scopes,&mut opened).unwrap();
    let scope=&scopes["FRAME_TARGET"];
    assert!(scope.main.as_deref().unwrap().starts_with("main-"));
    let calls=calls.borrow();
    let runtime=calls.iter().position(|call| call["method"] == "Runtime.enable" && call["sessionId"] == scope.session).unwrap();
    let isolated=calls.iter().position(|call| call["method"] == "Page.createIsolatedWorld" && call["sessionId"] == scope.session).unwrap();
    assert!(runtime<isolated);
}
