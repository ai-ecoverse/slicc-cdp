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
    children: bool,
    isolated: bool,
    native: bool,
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
            "Target.getTargets" => json!({"targetInfos":if self.children {vec![root,child]} else {vec![root]}}),
            "Target.attachToTarget" => {
                self.next += 1;
                let session = format!("S{}",self.next);
                self.sessions.insert(session.clone(),command["params"]["targetId"].as_str().unwrap().into());
                json!({"sessionId":session})
            },
            "Page.getPermissionsPolicyState" => json!({"states":[{"feature":"tools","allowed":true}]}),
            "Page.getFrameTree" => json!({"frameTree":{"frame":frame}}),
            "Runtime.enable" | "Page.createIsolatedWorld" => {
                let isolated = command["method"] == "Page.createIsolatedWorld";
                self.queue.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":if isolated {9} else {7},"uniqueId":format!("{}-{}-{}",if isolated {"isolated"} else {"main"},frame["id"].as_str().unwrap(),frame["loaderId"].as_str().unwrap()),"origin":"https://ordinary.test","auxData":{"frameId":frame["id"],"isDefault":!isolated}}}}).to_string());
                if isolated { self.isolated = true; }
                if !isolated && self.isolated {
                    self.queue.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":session,"params":{"context":{"id":9,"uniqueId":format!("isolated-{}-{}",frame["id"].as_str().unwrap(),frame["loaderId"].as_str().unwrap()),"origin":"https://ordinary.test","auxData":{"frameId":frame["id"],"isDefault":false}}}}).to_string());
                }
                if isolated { json!({"executionContextId":9}) } else { json!({}) }
            }
            "Runtime.evaluate" => {
                let expression = command["params"]["expression"].as_str().unwrap_or("");
                let value = if expression.contains("api.executeTool") {
                    json!({"__webmcp_result":"5"})
                } else if expression.contains("toolsJson") {
                    json!({"toolsJson":json!([{"name":"add","origin":"https://ordinary.test","frameId":"PARENT","inputSchema":{"type":"object"},"annotations":{}}]).to_string()})
                } else if expression == "'modelContext' in Document.prototype" {
                    json!(self.native)
                } else {
                    json!(!expression.contains("__slicc_webmcp_shim"))
                };
                json!({"result":{"value":value}})
            },
            _ => json!({}),
        };
        self.queue.push_back(json!({"id":command["id"],"result":result}).to_string());
        Ok(())
    }
    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { Ok(self.queue.pop_front()) }
}

#[test]
fn requested_cross_origin_tools_warn_but_preserve_root_listing_and_calling() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(FrameInspector { queue:VecDeque::new(), calls:calls.clone(), next:0, sessions:BTreeMap::new(), children:false, isolated:false, native:false }));
    let session = cdp.call("Target.attachToTarget", json!({"targetId":"ROOT","flatten":true}), None).unwrap()["sessionId"].as_str().unwrap().to_string();
    let state = Session::default();
    let from = vec!["https://ordinary-child.test".to_string()];
    let (tools, native) = list(&mut cdp, &state, "ROOT", &session, &from).unwrap();
    assert!(!native);
    assert!(tools.as_array().unwrap().iter().any(|tool| tool["name"] == "add"));
    let (answer, native) = call(&mut cdp, &state, "ROOT", &session, &from, None, "add", &json!({"a":2,"b":3}), false).unwrap();
    assert!(!native);
    assert_eq!(answer["__webmcp_result"], "5");
    assert!(!calls.borrow().iter().any(|command| command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(|expression| expression.contains("ordinary-child.test"))));
    let before = calls.borrow().len();
    let protected = list(&mut cdp, &state, "ROOT", &session, &["https://vault.sliccy.ai".into()]);
    assert!(protected.unwrap_err().contains("slicc: refused:"));
    assert_eq!(calls.borrow().len(), before);
}

#[test]
fn only_explicit_client_origin_requests_return_the_warning_with_root_tools() {
    for from in [Vec::new(), vec!["https://ordinary-child.test".to_string()]] {
        let calls = Rc::new(RefCell::new(Vec::new()));
        let mut cdp = Cdp::from_transport(Box::new(FrameInspector { queue:VecDeque::new(), calls:calls.clone(), next:0, sessions:BTreeMap::new(), children:false, isolated:false, native:false }));
        let mut state = Session::default();
        let listed = crate::webmcp::list_from(&mut cdp, &mut state, Some("ROOT"), true, &from);
        assert_eq!(listed.code, 0, "{}", listed.stderr);
        assert!(listed.stdout.contains("add"));
        assert_eq!(listed.stderr.contains("cross-origin child tools are unavailable"), !from.is_empty());
        let called = crate::webmcp::call_from(&mut cdp, &mut state, "add", "{\"a\":2,\"b\":3}", Some("ROOT"), false, &from, None);
        assert_eq!(called.code, 0, "{}", called.stderr);
        assert_eq!(called.stdout.trim(), "5");
        assert_eq!(called.stderr.contains("cross-origin child tools are unavailable"), !from.is_empty());
    }
}

#[test]
fn newly_attached_frame_requires_default_context_observation_before_scope_selection() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(FrameInspector { queue:VecDeque::new(), calls:calls.clone(), next:0, sessions:BTreeMap::new(), children:true, isolated:false, native:false }));
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
