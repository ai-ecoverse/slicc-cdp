fn toctou_method(domain: &str, member: &str) -> String {
    format!("{domain}.{member}")
}

const TOCTOU_MARK: &str = "TOCTOU_ATTACH_SECRET";

fn toctou_vault() -> String {
    format!("https://vault.sliccy.ai/private/{TOCTOU_MARK}")
}

fn toctou_seven() -> String {
    format!("https://seven.sliccy.ai/os/{TOCTOU_MARK}?token={TOCTOU_MARK}")
}

fn toctou_wrap(scheme: &str, inner: &str) -> String {
    format!("{scheme}:{inner}")
}

fn toctou_lives() -> Vec<String> {
    let mut lives = Vec::new();
    for inner in [toctou_vault(), toctou_seven()] {
        lives.push(inner.clone());
        lives.push(toctou_wrap("view-source", &inner));
        lives.push(toctou_wrap("blob", &format!("{inner}/11111111-1111-1111-1111-111111111111")));
        lives.push(toctou_wrap("filesystem", &format!("{inner}/temporary/a")));
    }
    lives
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ToctouEventTiming {
    Never,
    AfterFirstRead,
    AfterSecondRead,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ExistingLearn {
    EventFirst,
    ReplyBeforeLearn,
    NeverLearn,
    LearnDuring,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum ToctouNavEvents {
    All,
    None,
    AfterReply,
}

struct ToctouContext {
    id: i64,
    unique: String,
    objects: std::collections::BTreeSet<String>,
    promises: std::collections::BTreeSet<String>,
    post_commit: bool,
}

struct ToctouState {
    info: Value,
    live: Value,
    tree: Value,
    commits_on_forward: bool,
    committed: bool,
    event_timing: ToctouEventTiming,
    content_forwards: usize,
    content_backend_ok: usize,
    sessions: std::collections::BTreeMap<String, String>,
    attaches: usize,
    contexts: Vec<ToctouContext>,
    next_context: i64,
    survive_contexts: bool,
    nav_events: ToctouNavEvents,
    pending_nav_events: VecDeque<String>,
    node_epoch: u64,
    live_nodes: std::collections::BTreeSet<i64>,
    pdf_stream: Option<String>,
}

struct ToctouBrowser {
    inbox: VecDeque<String>,
    state: std::rc::Rc<std::cell::RefCell<ToctouState>>,
}

fn toctou_info(url: &str) -> Value {
    json!({
        "targetId": "T",
        "type": "page",
        "url": url,
        "title": if url == "about:blank" { "about:blank" } else { "vault" },
        "attached": false,
        "canAccessOpener": false,
    })
}

fn toctou_frame(url: &str, loader: &str) -> Value {
    let origin = if url == "about:blank" || url.starts_with("view-source:") || url.starts_with("blob:") || url.starts_with("filesystem:") {
        "://".into()
    } else {
        url::Url::parse(url)
            .ok()
            .map(|parsed| parsed.origin().ascii_serialization())
            .unwrap_or_else(|| "://".into())
    };
    json!({
        "id": "T",
        "loaderId": loader,
        "url": url,
        "securityOrigin": origin,
        "mimeType": "text/html",
    })
}

fn toctou_same_origin_reuse(from: &str, to: &str) -> bool {
    let Ok(from_url) = url::Url::parse(from) else {
        return false;
    };
    let Ok(to_url) = url::Url::parse(to) else {
        return false;
    };
    if from_url.scheme() == "about" || to_url.scheme() == "about" {
        return false;
    }
    from_url.origin() == to_url.origin()
        && matches!(from_url.scheme(), "http" | "https")
        && matches!(to_url.scheme(), "http" | "https")
        && from_url.origin().ascii_serialization() != "null"
}

impl ToctouState {
    fn live_context(&self, id: Option<i64>, unique: Option<&str>) -> Option<&ToctouContext> {
        self.contexts.iter().find(|context| {
            id.is_some_and(|id| context.id == id) || unique.is_some_and(|unique| context.unique == unique)
        })
    }

    fn live_object(&self, object_id: &str) -> Option<&ToctouContext> {
        self.contexts.iter().find(|context| context.objects.contains(object_id) || context.promises.contains(object_id))
    }

    fn commit(&mut self, inbox: &mut VecDeque<String>, session: &str, deliver_frame: bool) {
        if self.committed {
            self.tree = self.live.clone();
            self.info = toctou_info(self.live["url"].as_str().unwrap());
            return;
        }
        let from = self.tree["url"].as_str().unwrap_or("about:blank");
        let to = self.live["url"].as_str().unwrap();
        let reuse = self.survive_contexts || toctou_same_origin_reuse(from, to);
        self.tree = self.live.clone();
        self.info = toctou_info(to);
        self.committed = true;
        if !reuse {
            self.node_epoch += 1;
            self.live_nodes.clear();
            let destroyed: Vec<ToctouContext> = self.contexts.drain(..).collect();
            for context in destroyed {
                inbox.push_back(
                    json!({
                        "method": toctou_method("Runtime", "executionContextDestroyed"),
                        "sessionId": session,
                        "params": { "executionContextId": context.id, "executionContextUniqueId": context.unique },
                    })
                    .to_string(),
                );
            }
            self.next_context += 1;
            let id = self.next_context;
            let unique = format!("ctx-post-{id}");
            self.contexts.push(ToctouContext {
                id,
                unique: unique.clone(),
                objects: std::collections::BTreeSet::new(),
                promises: std::collections::BTreeSet::new(),
                post_commit: true,
            });
            inbox.push_back(
                json!({
                    "method": toctou_method("Runtime", "executionContextCreated"),
                    "sessionId": session,
                    "params": {
                        "context": {
                            "id": id,
                            "uniqueId": unique,
                            "origin": self.live["securityOrigin"].clone(),
                            "name": "",
                            "auxData": { "frameId": "T", "isDefault": true },
                        }
                    },
                })
                .to_string(),
            );
        } else {
            for context in &mut self.contexts {
                context.post_commit = true;
            }
        }
        let mut nav = VecDeque::new();
        nav.push_back(
            json!({
                "method": toctou_method("Target", "targetInfoChanged"),
                "params": { "targetInfo": self.info.clone() },
            })
            .to_string(),
        );
        if deliver_frame {
            let mut sessions: Vec<String> = self.sessions.keys().cloned().collect();
            if !sessions.iter().any(|have| have == session) {
                sessions.push(session.to_string());
            }
            for session_id in sessions {
                nav.push_back(
                    json!({
                        "method": toctou_method("Page", "frameNavigated"),
                        "sessionId": session_id,
                        "params": { "frame": self.live.clone(), "type": "Navigation" },
                    })
                    .to_string(),
                );
            }
        }
        match self.nav_events {
            ToctouNavEvents::None => {}
            ToctouNavEvents::AfterReply => self.pending_nav_events.append(&mut nav),
            ToctouNavEvents::All => inbox.append(&mut nav),
        }
    }

    fn flush_nav_events(&mut self, inbox: &mut VecDeque<String>) {
        while let Some(event) = self.pending_nav_events.pop_front() {
            inbox.push_back(event);
        }
    }
}

impl Transport for ToctouBrowser {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut state = self.state.borrow_mut();
        let called = call["method"].as_str().unwrap();
        let session = call["sessionId"].as_str().map(str::to_string);
        let reply_ok = |result: Value| {
            json!({
                "id": call["id"],
                "sessionId": call["sessionId"],
                "result": result,
            })
            .to_string()
        };
        let reply_err = |message: &str| {
            json!({
                "id": call["id"],
                "sessionId": call["sessionId"],
                "error": { "message": message },
            })
            .to_string()
        };
        let attach = toctou_method("Target", "attachToTarget");
        let detach = toctou_method("Target", "detachFromTarget");
        let get_info = toctou_method("Target", "getTargetInfo");
        let get_targets = toctou_method("Target", "getTargets");
        let set_discover = toctou_method("Target", "setDiscoverTargets");
        let frame_tree = toctou_method("Page", "getFrameTree");
        let enable_page = toctou_method("Page", "enable");
        let enable_runtime = toctou_method("Runtime", "enable");
        let enable_net = toctou_method("Network", "enable");
        let bypass = toctou_method("Network", "setBypassServiceWorker");
        let enable_fetch = toctou_method("Fetch", "enable");
        let enable_inspector = toctou_method("Inspector", "enable");
        let evaluate = toctou_method("Runtime", "evaluate");
        let call_fn = toctou_method("Runtime", "callFunctionOn");
        let await_promise = toctou_method("Runtime", "awaitPromise");
        let compile = toctou_method("Runtime", "compileScript");
        let run_script = toctou_method("Runtime", "runScript");
        let get_document = toctou_method("DOM", "getDocument");
        let query = toctou_method("DOM", "querySelector");
        let describe = toctou_method("DOM", "describeNode");
        let outer = toctou_method("DOM", "getOuterHTML");
        let resolve = toctou_method("DOM", "resolveNode");
        let snap = toctou_method("DOMSnapshot", "captureSnapshot");
        let screenshot = toctou_method("Page", "captureScreenshot");
        let page_snap = toctou_method("Page", "captureSnapshot");
        let pdf = toctou_method("Page", "printToPDF");
        let resource = toctou_method("Page", "getResourceContent");
        let resource_tree = toctou_method("Page", "getResourceTree");
        let ax = toctou_method("Accessibility", "getFullAXTree");
        let css = toctou_method("CSS", "getStyleSheetText");
        let mouse = toctou_method("Input", "dispatchMouseEvent");
        let key = toctou_method("Input", "dispatchKeyEvent");
        let insert = toctou_method("Input", "insertText");
        let net_body = toctou_method("Network", "getResponseBody");
        let get_props = toctou_method("Runtime", "getProperties");
        let nav_history = toctou_method("Page", "getNavigationHistory");
        let ax_query = toctou_method("Accessibility", "queryAXTree");
        let io_read = toctou_method("IO", "read");
        let content_like = [
            evaluate.as_str(),
            call_fn.as_str(),
            await_promise.as_str(),
            compile.as_str(),
            run_script.as_str(),
            get_document.as_str(),
            query.as_str(),
            describe.as_str(),
            outer.as_str(),
            resolve.as_str(),
            snap.as_str(),
            screenshot.as_str(),
            page_snap.as_str(),
            pdf.as_str(),
            resource.as_str(),
            resource_tree.as_str(),
            ax.as_str(),
            ax_query.as_str(),
            css.as_str(),
            mouse.as_str(),
            key.as_str(),
            insert.as_str(),
            net_body.as_str(),
            get_props.as_str(),
            io_read.as_str(),
        ]
        .contains(&called);
        if content_like && state.commits_on_forward {
            state.content_forwards += 1;
            let deliver = match state.event_timing {
                ToctouEventTiming::Never => false,
                ToctouEventTiming::AfterFirstRead => state.content_forwards == 1,
                ToctouEventTiming::AfterSecondRead => state.content_forwards == 2,
            };
            let session = session.clone().unwrap_or_default();
            state.commit(&mut self.inbox, &session, deliver);
        }
        let protected_doc = state.committed && state.tree["url"].as_str().is_some_and(|url| url.contains(TOCTOU_MARK) || url.contains("vault.sliccy.ai") || url.contains("seven.sliccy.ai"));
        let marker = || json!({ "result": { "type": "string", "value": format!("leaked-{TOCTOU_MARK}") } });
        let missing = "Cannot find context with specified id";
        let missing_node = "Could not find node with given id";
        if called == get_info {
            self.inbox.push_back(reply_ok(json!({ "targetInfo": state.info.clone() })));
        } else if called == get_targets {
            self.inbox.push_back(reply_ok(json!({ "targetInfos": [state.info.clone()] })));
        } else if called == set_discover {
            self.inbox.push_back(reply_ok(json!({})));
        } else if called == attach {
            state.attaches += 1;
            let session = format!("S{}", state.attaches);
            state.sessions.insert(session.clone(), "T".into());
            self.inbox.push_back(
                json!({
                    "method": toctou_method("Target", "attachedToTarget"),
                    "params": {
                        "sessionId": session,
                        "targetInfo": state.info.clone(),
                        "waitingForDebugger": false,
                    },
                })
                .to_string(),
            );
            self.inbox.push_back(reply_ok(json!({ "sessionId": session })));
        } else if called == detach {
            self.inbox.push_back(reply_ok(json!({})));
        } else if called == frame_tree {
            self.inbox.push_back(reply_ok(json!({ "frameTree": { "frame": state.tree.clone() } })));
        } else if called == enable_runtime {
            let session = call["sessionId"].as_str().unwrap_or("");
            state.next_context = 1;
            state.contexts.clear();
            state.live_nodes = [1].into_iter().collect();
            state.contexts.push(ToctouContext {
                id: 1,
                unique: format!("ctx-{session}"),
                objects: ["1".into(), "handle-1".into()].into_iter().collect(),
                promises: ["promise-1".into()].into_iter().collect(),
                post_commit: false,
            });
            self.inbox.push_back(
                json!({
                    "method": toctou_method("Runtime", "executionContextCreated"),
                    "sessionId": session,
                    "params": {
                        "context": {
                            "id": 1,
                            "uniqueId": format!("ctx-{session}"),
                            "origin": state.tree["securityOrigin"].clone(),
                            "name": "",
                            "auxData": { "frameId": "T", "isDefault": true },
                        }
                    },
                })
                .to_string(),
            );
            self.inbox.push_back(reply_ok(json!({})));
        } else if called == evaluate || called == compile || called == run_script {
            let id = call["params"].get("contextId").or_else(|| call["params"].get("executionContextId")).and_then(Value::as_i64);
            let unique = call["params"].get("uniqueContextId").or_else(|| call["params"].get("executionContextUniqueId")).and_then(Value::as_str);
            let context = state.live_context(id, unique);
            match context {
                None if id.is_some() || unique.is_some() => self.inbox.push_back(reply_err(missing)),
                None => self.inbox.push_back(reply_err(missing)),
                Some(context) if context.post_commit && protected_doc => {
                    state.content_backend_ok += 1;
                    self.inbox.push_back(reply_ok(marker()));
                }
                Some(_) if protected_doc && state.committed => self.inbox.push_back(reply_err(missing)),
                Some(_) => {
                    state.content_backend_ok += 1;
                    self.inbox.push_back(reply_ok(json!({ "result": { "type": "string", "value": "ordinary" } })));
                }
            }
        } else if called == call_fn {
            let object_id = call["params"].get("objectId").and_then(Value::as_str);
            let id = call["params"].get("contextId").or_else(|| call["params"].get("executionContextId")).and_then(Value::as_i64);
            let unique = call["params"].get("uniqueContextId").or_else(|| call["params"].get("executionContextUniqueId")).and_then(Value::as_str);
            let context = object_id.and_then(|object| state.live_object(object)).or_else(|| state.live_context(id, unique));
            match context {
                None => self.inbox.push_back(reply_err(missing)),
                Some(context) if context.post_commit && protected_doc => self.inbox.push_back(reply_ok(marker())),
                Some(_) if protected_doc && state.committed => self.inbox.push_back(reply_err(missing)),
                Some(_) => self.inbox.push_back(reply_ok(json!({ "result": { "type": "string", "value": "ordinary" } }))),
            }
        } else if called == await_promise {
            let promise = call["params"].get("promiseObjectId").and_then(Value::as_str).unwrap_or("");
            match state.live_object(promise) {
                None => self.inbox.push_back(reply_err(missing)),
                Some(context) if context.post_commit && protected_doc => self.inbox.push_back(reply_ok(marker())),
                Some(_) if protected_doc && state.committed => self.inbox.push_back(reply_err(missing)),
                Some(_) => self.inbox.push_back(reply_ok(json!({ "result": { "type": "string", "value": "ordinary" } }))),
            }
        } else if called == get_document {
            let url = state.tree["url"].clone();
            state.live_nodes.insert(1);
            self.inbox.push_back(reply_ok(json!({ "root": { "nodeId": 1, "nodeName": "HTML", "documentURL": url } })));
        } else if called == query || called == describe || called == resolve || called == outer {
            let node = call["params"].get("nodeId").and_then(Value::as_i64).unwrap_or(1);
            if !state.survive_contexts && state.committed && !state.live_nodes.contains(&node) {
                self.inbox.push_back(reply_err(missing_node));
            } else if called == outer {
                if protected_doc {
                    self.inbox.push_back(reply_ok(json!({ "outerHTML": format!("<html>{TOCTOU_MARK}</html>") })));
                } else {
                    self.inbox.push_back(reply_ok(json!({ "outerHTML": "<html></html>" })));
                }
            } else if protected_doc {
                state.live_nodes.insert(2);
                self.inbox.push_back(reply_ok(json!({ "nodeId": 2, "node": { "nodeId": 2, "nodeName": "BODY", "documentURL": format!("body {TOCTOU_MARK}") } })));
            } else {
                state.live_nodes.insert(2);
                self.inbox.push_back(reply_ok(json!({ "nodeId": 2, "node": { "nodeId": 2, "nodeName": "BODY" } })));
            }
        } else if called == snap {
            if protected_doc {
                self.inbox.push_back(reply_ok(json!({ "documents": [{ "documentURL": format!("snap {TOCTOU_MARK}") }], "strings": [TOCTOU_MARK] })));
            } else {
                self.inbox.push_back(reply_ok(json!({ "documents": [], "strings": [] })));
            }
        } else if called == screenshot {
            self.inbox.push_back(reply_ok(json!({ "data": if protected_doc { format!("marker-{TOCTOU_MARK}") } else { "iVBORw0KGgo=".into() } })));
        } else if called == pdf {
            if call["params"].get("transferMode").and_then(Value::as_str) == Some("ReturnAsStream") {
                let handle = format!("pdf-{}", state.content_forwards);
                state.pdf_stream = Some(handle.clone());
                self.inbox.push_back(reply_ok(json!({ "stream": handle })));
            } else {
                self.inbox.push_back(reply_ok(json!({ "data": if protected_doc { format!("marker-{TOCTOU_MARK}") } else { "iVBORw0KGgo=".into() } })));
            }
        } else if called == io_read {
            let handle = call["params"].get("handle").and_then(Value::as_str).unwrap_or("");
            if state.pdf_stream.as_deref() == Some(handle) {
                self.inbox.push_back(reply_ok(json!({
                    "data": if protected_doc { format!("pdf-{TOCTOU_MARK}") } else { "pdf-ordinary".into() },
                    "eof": true,
                    "base64Encoded": false,
                })));
            } else {
                self.inbox.push_back(reply_err("Invalid stream handle"));
            }
        } else if called == page_snap {
            if protected_doc {
                self.inbox.push_back(reply_ok(json!({ "document": format!("snapshot {TOCTOU_MARK}") })));
            } else {
                self.inbox.push_back(reply_ok(json!({ "document": "snapshot" })));
            }
        } else if called == resource {
            if protected_doc {
                self.inbox.push_back(reply_ok(json!({ "content": format!("body {TOCTOU_MARK}"), "base64Encoded": false })));
            } else {
                self.inbox.push_back(reply_ok(json!({ "content": "body", "base64Encoded": false })));
            }
        } else if called == resource_tree {
            self.inbox.push_back(reply_ok(json!({ "frameTree": { "frame": state.tree.clone(), "resources": [] } })));
        } else if called == nav_history {
            self.inbox.push_back(reply_ok(json!({
                "currentIndex": 0,
                "entries": [{ "id": 1, "url": state.tree["url"].clone(), "userTypedURL": state.tree["url"].clone(), "title": "", "transitionType": "typed" }],
            })));
        } else if called == ax || called == ax_query {
            if protected_doc {
                self.inbox.push_back(reply_ok(json!({ "nodes": [{ "name": { "value": format!("ax {TOCTOU_MARK}") } }] })));
            } else {
                self.inbox.push_back(reply_ok(json!({ "nodes": [] })));
            }
        } else if called == css {
            if protected_doc {
                self.inbox.push_back(reply_ok(json!({ "text": format!("css {TOCTOU_MARK}") })));
            } else {
                self.inbox.push_back(reply_ok(json!({ "text": "css" })));
            }
        } else if called == get_props {
            let object = call["params"].get("objectId").and_then(Value::as_str).unwrap_or("");
            match state.live_object(object) {
                None => self.inbox.push_back(reply_err(missing)),
                Some(context) if context.post_commit && protected_doc => {
                    self.inbox.push_back(reply_ok(json!({ "result": [{ "name": "secret", "value": { "type": "string", "value": format!("prop-{TOCTOU_MARK}") } }] })));
                }
                Some(_) if protected_doc && state.committed => self.inbox.push_back(reply_err(missing)),
                Some(_) => self.inbox.push_back(reply_ok(json!({ "result": [{ "name": "ok", "value": { "type": "string", "value": "ordinary" } }] }))),
            }
        } else if called == mouse || called == key || called == insert {
            self.inbox.push_back(reply_ok(json!({})));
        } else if called == net_body {
            if protected_doc {
                self.inbox.push_back(reply_ok(json!({ "body": format!("net {TOCTOU_MARK}"), "base64Encoded": false })));
            } else {
                self.inbox.push_back(reply_ok(json!({ "body": "net", "base64Encoded": false })));
            }
        } else if called == enable_page
            || called == enable_net
            || called == bypass
            || called == enable_fetch
            || called == enable_inspector
        {
            self.inbox.push_back(reply_ok(json!({})));
        } else {
            self.inbox.push_back(reply_ok(json!({})));
        }
        if state.nav_events == ToctouNavEvents::AfterReply && content_like {
            state.flush_nav_events(&mut self.inbox);
        }
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn toctou_client(live: &str, timing: ToctouEventTiming) -> (Cdp, std::rc::Rc<std::cell::RefCell<ToctouState>>) {
    let blank = toctou_frame("about:blank", "L0");
    let live_frame = toctou_frame(live, "L1");
    let announced = toctou_info("about:blank");
    let state = std::rc::Rc::new(std::cell::RefCell::new(ToctouState {
        info: announced.clone(),
        live: live_frame,
        tree: blank,
        commits_on_forward: false,
        committed: false,
        event_timing: timing,
        content_forwards: 0,
        content_backend_ok: 0,
        sessions: std::collections::BTreeMap::new(),
        attaches: 0,
        contexts: Vec::new(),
        next_context: 0,
        survive_contexts: false,
        nav_events: ToctouNavEvents::All,
        pending_nav_events: VecDeque::new(),
        node_epoch: 0,
        live_nodes: std::collections::BTreeSet::new(),
        pdf_stream: None,
    }));
    let mut cdp = Cdp::from_transport(Box::new(ToctouBrowser {
        inbox: VecDeque::new(),
        state: state.clone(),
    }));
    cdp.absorb(json!({
        "method": toctou_method("Target", "targetCreated"),
        "params": { "targetInfo": announced },
    }));
    (cdp, state)
}

fn toctou_commands() -> Vec<(&'static str, String, Value)> {
    vec![
        ("Runtime.evaluate", toctou_method("Runtime", "evaluate"), json!({ "expression": "document.body.innerText", "returnByValue": true })),
        ("Runtime.evaluate+contextId", toctou_method("Runtime", "evaluate"), json!({ "expression": "1", "contextId": 1, "returnByValue": true })),
        ("Runtime.callFunctionOn", toctou_method("Runtime", "callFunctionOn"), json!({ "functionDeclaration": "() => document.body.innerText", "objectId": "1", "returnByValue": true })),
        ("Runtime.awaitPromise", toctou_method("Runtime", "awaitPromise"), json!({ "promiseObjectId": "promise-1", "returnByValue": true })),
        ("Runtime.compileScript", toctou_method("Runtime", "compileScript"), json!({ "expression": "1", "sourceURL": "", "persistScript": true })),
        ("Runtime.runScript", toctou_method("Runtime", "runScript"), json!({ "scriptId": "1", "executionContextId": 1 })),
        ("DOM.getDocument", toctou_method("DOM", "getDocument"), json!({})),
        ("DOM.querySelector", toctou_method("DOM", "querySelector"), json!({ "nodeId": 1, "selector": "body" })),
        ("DOM.describeNode", toctou_method("DOM", "describeNode"), json!({ "nodeId": 1 })),
        ("DOM.getOuterHTML", toctou_method("DOM", "getOuterHTML"), json!({ "nodeId": 1 })),
        ("DOM.resolveNode", toctou_method("DOM", "resolveNode"), json!({ "nodeId": 1 })),
        ("DOMSnapshot.captureSnapshot", toctou_method("DOMSnapshot", "captureSnapshot"), json!({ "computedStyleWhitelist": [] })),
        ("Page.captureScreenshot", toctou_method("Page", "captureScreenshot"), json!({})),
        ("Page.captureSnapshot", toctou_method("Page", "captureSnapshot"), json!({})),
        ("Page.printToPDF", toctou_method("Page", "printToPDF"), json!({})),
        ("Page.getResourceContent", toctou_method("Page", "getResourceContent"), json!({ "frameId": "T", "url": toctou_vault() })),
        ("Page.getResourceTree", toctou_method("Page", "getResourceTree"), json!({})),
        ("Accessibility.getFullAXTree", toctou_method("Accessibility", "getFullAXTree"), json!({})),
        ("CSS.getStyleSheetText", toctou_method("CSS", "getStyleSheetText"), json!({ "styleSheetId": "1" })),
        ("Input.dispatchMouseEvent", toctou_method("Input", "dispatchMouseEvent"), json!({ "type": "mousePressed", "x": 1, "y": 1, "button": "left", "clickCount": 1 })),
        ("Input.dispatchKeyEvent", toctou_method("Input", "dispatchKeyEvent"), json!({ "type": "keyDown", "key": "a" })),
        ("Input.insertText", toctou_method("Input", "insertText"), json!({ "text": "x" })),
        ("Network.getResponseBody", toctou_method("Network", "getResponseBody"), json!({ "requestId": "R1" })),
    ]
}

#[test]
fn content_commands_never_return_protected_marker_when_navigation_commits_between_preread_and_forward() {
    let mut findings = Vec::new();
    let mut class_notes = std::collections::BTreeMap::<String, Vec<String>>::new();
    for live in toctou_lives() {
        for timing in [
            ToctouEventTiming::Never,
            ToctouEventTiming::AfterFirstRead,
            ToctouEventTiming::AfterSecondRead,
        ] {
            let (mut cdp, state) = toctou_client(&live, timing);
            let attach = cdp
                .call(
                    &toctou_method("Target", "attachToTarget"),
                    json!({ "targetId": "T", "flatten": true }),
                    None,
                )
                .unwrap_or_else(|err| panic!("ordinary blank attach must succeed before TOCTOU: {err}"));
            let session = attach["sessionId"].as_str().unwrap().to_string();
            {
                let mut state = state.borrow_mut();
                state.commits_on_forward = true;
                state.content_forwards = 0;
                state.committed = false;
            }
            for (label, command, params) in toctou_commands() {
                let out = cdp.call(&command, params.clone(), Some(&session));
                let text = match &out {
                    Ok(value) => value.to_string(),
                    Err(message) => message.clone(),
                };
                let entry = class_notes.entry(label.to_string()).or_default();
                if text.contains(TOCTOU_MARK) {
                    findings.push(format!("FINDING live={live} timing={timing:?} class={label} out={text}"));
                    entry.push(format!("marker live={live} timing={timing:?}"));
                } else if out.is_err() {
                    entry.push(format!("closed live={live} timing={timing:?} err={}", first_stderr_line(&text).unwrap_or(text.as_str())));
                } else {
                    entry.push(format!("ok-no-marker live={live} timing={timing:?}"));
                }
            }
        }
    }
    assert!(
        findings.is_empty(),
        "TOCTOU realistic-fake findings:\n{}\n\nclass notes:\n{}",
        findings.join("\n"),
        class_notes
            .into_iter()
            .map(|(class, notes)| format!("{class}: {}", notes.into_iter().take(3).collect::<Vec<_>>().join(" | ")))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn ordinary_to_ordinary_navigation_during_pinned_evaluate_is_not_over_blocked() {
    let ordinary_a = "https://ordinary.example/a";
    let ordinary_b = "https://ordinary.example/b";
    let blank = toctou_frame("about:blank", "L0");
    let live = toctou_frame(ordinary_b, "L1");
    let announced = toctou_info(ordinary_a);
    let state = std::rc::Rc::new(std::cell::RefCell::new(ToctouState {
        info: announced.clone(),
        live,
        tree: toctou_frame(ordinary_a, "L0"),
        commits_on_forward: false,
        committed: false,
        event_timing: ToctouEventTiming::Never,
        content_forwards: 0,
        content_backend_ok: 0,
        sessions: std::collections::BTreeMap::new(),
        attaches: 0,
        contexts: Vec::new(),
        next_context: 0,
        survive_contexts: false,
        nav_events: ToctouNavEvents::All,
        pending_nav_events: VecDeque::new(),
        node_epoch: 0,
        live_nodes: std::collections::BTreeSet::new(),
        pdf_stream: None,
    }));
    let mut cdp = Cdp::from_transport(Box::new(ToctouBrowser {
        inbox: VecDeque::new(),
        state: state.clone(),
    }));
    cdp.absorb(json!({
        "method": toctou_method("Target", "targetCreated"),
        "params": { "targetInfo": announced },
    }));
    {
        let mut state = state.borrow_mut();
        state.tree = toctou_frame(ordinary_a, "L0");
        let _ = blank;
    }
    let attach = cdp
        .call(
            &toctou_method("Target", "attachToTarget"),
            json!({ "targetId": "T", "flatten": true }),
            None,
        )
        .expect("ordinary attach");
    let session = attach["sessionId"].as_str().unwrap().to_string();
    {
        let mut state = state.borrow_mut();
        state.commits_on_forward = true;
        state.live = toctou_frame(ordinary_b, "L2");
    }
    let out = cdp.call(
        &toctou_method("Runtime", "evaluate"),
        json!({ "expression": "1", "returnByValue": true }),
        Some(&session),
    );
    assert!(
        out.as_ref().is_err_and(|message| message.contains("Cannot find context") || message.starts_with("slicc: refused:") == false)
            || out.as_ref().is_ok_and(|value| !value.to_string().contains(TOCTOU_MARK)),
        "ordinary→ordinary must match main (context destroyed or ordinary result), not a protected refusal: {out:?}"
    );
    assert!(!cdp.is_protected_target("T"), "ordinary→ordinary must not latch protected");
}

#[test]
fn mutation_post_reply_document_recheck_is_necessary_for_protected_commit() {
    let live = toctou_vault();
    let ordinary = "https://ordinary.example/";
    let (mut cdp, state) = toctou_client(&live, ToctouEventTiming::Never);
    {
        let mut state = state.borrow_mut();
        state.tree = toctou_frame(ordinary, "L0");
        state.info = toctou_info(ordinary);
    }
    cdp.absorb(json!({
        "method": toctou_method("Target", "targetInfoChanged"),
        "params": { "targetInfo": toctou_info(ordinary) },
    }));
    let attach = cdp
        .call(
            &toctou_method("Target", "attachToTarget"),
            json!({ "targetId": "T", "flatten": true }),
            None,
        )
        .expect("ordinary attach");
    let session = attach["sessionId"].as_str().unwrap().to_string();
    {
        let mut state = state.borrow_mut();
        state.commits_on_forward = true;
        state.survive_contexts = true;
        state.content_forwards = 0;
        state.content_backend_ok = 0;
        state.committed = false;
    }
    let out = cdp.call(
        &toctou_method("Runtime", "evaluate"),
        json!({ "expression": "1", "returnByValue": true }),
        Some(&session),
    );
    assert!(
        state.borrow().content_forwards >= 1,
        "evaluate must be forwarded to the backend so DiD is the sole refusal (got forwards={})",
        state.borrow().content_forwards
    );
    assert!(
        state.borrow().content_backend_ok >= 1,
        "backend must answer Ok before DiD refuses (got backend_ok={})",
        state.borrow().content_backend_ok
    );
    assert!(
        out.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")),
        "DiD recheck must refuse after a surviving context answers Ok(marker): {out:?}"
    );
    if let Err(message) = out {
        assert!(!message.contains(TOCTOU_MARK), "{message}");
    }
}

const EXISTING_ORDINARY: &str = "https://ordinary.example/existing";

fn existing_commands() -> Vec<(&'static str, String, Value)> {
    let mut commands = toctou_commands();
    commands.push(("Runtime.evaluate+awaitPromise", toctou_method("Runtime", "awaitPromise"), json!({ "promiseObjectId": "promise-1", "returnByValue": true })));
    commands.push(("Runtime.getProperties", toctou_method("Runtime", "getProperties"), json!({ "objectId": "handle-1" })));
    commands.push(("Page.printToPDF+stream", toctou_method("Page", "printToPDF"), json!({ "transferMode": "ReturnAsStream" })));
    commands.push(("IO.read+pdf", toctou_method("IO", "read"), json!({ "handle": "pdf-1" })));
    commands.push(("Accessibility.queryAXTree", toctou_method("Accessibility", "queryAXTree"), json!({ "nodeId": 1 })));
    commands.push(("Page.getFrameTree", toctou_method("Page", "getFrameTree"), json!({})));
    commands.push(("Page.getNavigationHistory", toctou_method("Page", "getNavigationHistory"), json!({})));
    commands
}

fn existing_client(live: &str) -> (Cdp, std::rc::Rc<std::cell::RefCell<ToctouState>>, String) {
    let ordinary = toctou_frame(EXISTING_ORDINARY, "L0");
    let live_frame = toctou_frame(live, "L1");
    let announced = toctou_info(EXISTING_ORDINARY);
    let state = std::rc::Rc::new(std::cell::RefCell::new(ToctouState {
        info: announced.clone(),
        live: live_frame,
        tree: ordinary.clone(),
        commits_on_forward: false,
        committed: false,
        event_timing: ToctouEventTiming::AfterFirstRead,
        content_forwards: 0,
        content_backend_ok: 0,
        sessions: std::collections::BTreeMap::new(),
        attaches: 0,
        contexts: Vec::new(),
        next_context: 0,
        survive_contexts: false,
        nav_events: ToctouNavEvents::All,
        pending_nav_events: VecDeque::new(),
        node_epoch: 0,
        live_nodes: std::collections::BTreeSet::new(),
        pdf_stream: None,
    }));
    let mut cdp = Cdp::from_transport(Box::new(ToctouBrowser {
        inbox: VecDeque::new(),
        state: state.clone(),
    }));
    cdp.absorb(json!({
        "method": toctou_method("Target", "targetCreated"),
        "params": { "targetInfo": announced },
    }));
    let attach = cdp
        .call(
            &toctou_method("Target", "attachToTarget"),
            json!({ "targetId": "T", "flatten": true }),
            None,
        )
        .unwrap_or_else(|err| panic!("ordinary existing attach must succeed: {err}"));
    let session = attach["sessionId"].as_str().unwrap().to_string();
    {
        let mut state = state.borrow_mut();
        state.live_nodes = [1].into_iter().collect();
        if let Some(context) = state.contexts.first_mut() {
            context.objects.insert("1".into());
            context.objects.insert("handle-1".into());
            context.promises.insert("promise-1".into());
        }
    }
    let proof = cdp
        .call(
            &toctou_method("Runtime", "evaluate"),
            json!({ "expression": "document.body.innerText", "returnByValue": true }),
            Some(&session),
        )
        .unwrap_or_else(|err| panic!("ordinary existing read must work: {err}"));
    assert!(
        !proof.to_string().contains(TOCTOU_MARK),
        "ordinary proof must not carry the marker: {proof}"
    );
    (cdp, state, session)
}

fn existing_arm(state: &std::rc::Rc<std::cell::RefCell<ToctouState>>, learn: ExistingLearn) {
    let mut state = state.borrow_mut();
    state.committed = false;
    state.content_forwards = 0;
    state.content_backend_ok = 0;
    state.pending_nav_events.clear();
    match learn {
        ExistingLearn::EventFirst => {
            state.commits_on_forward = false;
            state.nav_events = ToctouNavEvents::All;
            state.event_timing = ToctouEventTiming::AfterFirstRead;
        }
        ExistingLearn::ReplyBeforeLearn => {
            state.commits_on_forward = true;
            state.nav_events = ToctouNavEvents::AfterReply;
            state.event_timing = ToctouEventTiming::AfterFirstRead;
        }
        ExistingLearn::NeverLearn => {
            state.commits_on_forward = true;
            state.nav_events = ToctouNavEvents::None;
            state.event_timing = ToctouEventTiming::Never;
        }
        ExistingLearn::LearnDuring => {
            state.commits_on_forward = true;
            state.nav_events = ToctouNavEvents::All;
            state.event_timing = ToctouEventTiming::AfterFirstRead;
        }
    }
}

fn existing_deliver_event_first(cdp: &mut Cdp, state: &std::rc::Rc<std::cell::RefCell<ToctouState>>, session: &str) {
    let events = {
        let mut state = state.borrow_mut();
        let mut inbox = VecDeque::new();
        state.commit(&mut inbox, session, true);
        inbox
    };
    for event in events {
        if let Ok(value) = serde_json::from_str::<Value>(&event) {
            cdp.absorb(value);
        }
    }
}

fn existing_classify(
    label: &str,
    out: &Result<Value, String>,
    learn: ExistingLearn,
    forwarded: bool,
    backend_ok: bool,
    latched_before: bool,
    latched_after: bool,
) -> (&'static str, String) {
    let text = match out {
        Ok(value) => value.to_string(),
        Err(message) => message.clone(),
    };
    if matches!(label, "Page.getFrameTree" | "Page.getNavigationHistory" | "Page.getResourceTree") {
        if learn != ExistingLearn::EventFirst && text.contains(TOCTOU_MARK) {
            return ("LEAK", format!("LEAK(placeholder-url {text})"));
        }
        if text.starts_with("slicc: refused:") {
            let guard = if latched_before || latched_after && !latched_before {
                "hidden/latch"
            } else {
                "document-gate"
            };
            return ("REFUSED", format!("REFUSED({guard})"));
        }
        return ("PLACEHOLDER-ONLY", "PLACEHOLDER-ONLY".into());
    }
    if matches!(label, "Input.dispatchMouseEvent" | "Input.dispatchKeyEvent" | "Input.insertText") {
        if out.is_ok() {
            return ("PLACEHOLDER-ONLY", "PLACEHOLDER-ONLY(empty-input)".into());
        }
        if text.starts_with("slicc: refused:") {
            let guard = if latched_before || latched_after && !latched_before {
                "hidden/latch"
            } else {
                "document-gate"
            };
            return ("REFUSED", format!("REFUSED({guard})"));
        }
    }
    if text.contains(TOCTOU_MARK) {
        return ("LEAK", format!("LEAK({text})"));
    }
    if text.contains("Cannot find context") || text.contains("Could not find node") || text.contains("context was destroyed") {
        return ("CONTEXT-DESTROYED", "CONTEXT-DESTROYED".into());
    }
    if text.starts_with("slicc: refused:") {
        let guard = if latched_before {
            "hidden/latch"
        } else if latched_after && !latched_before {
            "hidden/latch"
        } else if backend_ok {
            "post-reply-recheck"
        } else if !forwarded {
            "document-gate"
        } else {
            "document-gate"
        };
        return ("REFUSED", format!("REFUSED({guard})"));
    }
    if out.is_ok() {
        return ("PLACEHOLDER-ONLY", "PLACEHOLDER-ONLY(ordinary-or-empty)".into());
    }
    ("REFUSED", format!("REFUSED(other:{})", first_stderr_line(&text).unwrap_or(text.as_str())))
}

fn existing_run_cell(live: &str, learn: ExistingLearn, label: &str, command: &str, params: Value) -> (String, bool, bool) {
    existing_run_cell_opts(live, learn, label, command, params, false)
}

fn existing_run_cell_opts(
    live: &str,
    learn: ExistingLearn,
    label: &str,
    command: &str,
    params: Value,
    survive_contexts: bool,
) -> (String, bool, bool) {
    let (mut cdp, state, session) = existing_client(live);
    existing_arm(&state, learn);
    if survive_contexts {
        state.borrow_mut().survive_contexts = true;
    }
    let before_forwards = state.borrow().content_forwards;
    let before_ok = state.borrow().content_backend_ok;
    if learn == ExistingLearn::EventFirst {
        existing_deliver_event_first(&mut cdp, &state, &session);
    }
    let latched_before = cdp.is_protected_target("T");
    if label == "IO.read+pdf" {
        {
            let mut state = state.borrow_mut();
            state.commits_on_forward = false;
        }
        let stream = cdp.call(
            &toctou_method("Page", "printToPDF"),
            json!({ "transferMode": "ReturnAsStream" }),
            Some(&session),
        );
        match stream {
            Ok(value) => {
                let handle = value.get("stream").and_then(Value::as_str).unwrap_or("pdf-1").to_string();
                existing_arm(&state, learn);
                if survive_contexts {
                    state.borrow_mut().survive_contexts = true;
                }
                if learn == ExistingLearn::EventFirst {
                    existing_deliver_event_first(&mut cdp, &state, &session);
                }
                let latched_before = cdp.is_protected_target("T");
                let before_ok = state.borrow().content_backend_ok;
                let mut params = params;
                params["handle"] = json!(handle);
                let out = cdp.call(command, params, Some(&session));
                let forwarded = state.borrow().content_forwards > before_forwards;
                let backend_ok = state.borrow().content_backend_ok > before_ok;
                let latched_after = cdp.is_protected_target("T");
                let (_, cell) = existing_classify(label, &out, learn, forwarded, backend_ok, latched_before, latched_after);
                return (cell, out.as_ref().is_ok_and(|value| value.to_string().contains(TOCTOU_MARK)), latched_after);
            }
            Err(message) => {
                let forwarded = state.borrow().content_forwards > before_forwards;
                let backend_ok = state.borrow().content_backend_ok > before_ok;
                let latched_after = cdp.is_protected_target("T");
                let out: Result<Value, String> = Err(message);
                let (_, cell) = existing_classify("Page.printToPDF+stream", &out, learn, forwarded, backend_ok, latched_before, latched_after);
                return (cell, false, latched_after);
            }
        }
    }
    let out = cdp.call(command, params, Some(&session));
    let forwarded = state.borrow().content_forwards > before_forwards;
    let backend_ok = state.borrow().content_backend_ok > before_ok;
    let latched_after = cdp.is_protected_target("T");
    let (_, cell) = existing_classify(label, &out, learn, forwarded, backend_ok, latched_before, latched_after);
    (cell, out.as_ref().is_ok_and(|value| value.to_string().contains(TOCTOU_MARK)), latched_after)
}

#[test]
fn existing_session_toctou_matrix_never_leaks_protected_marker() {
    let mut rows = Vec::new();
    let mut leaks = Vec::new();
    for live in toctou_lives() {
        for learn in [
            ExistingLearn::EventFirst,
            ExistingLearn::ReplyBeforeLearn,
            ExistingLearn::NeverLearn,
            ExistingLearn::LearnDuring,
        ] {
            for (label, command, params) in existing_commands() {
                let (cell, leaked, _) = existing_run_cell(&live, learn, label, &command, params);
                let row = format!("{learn:?}\t{label}\t{live}\t{cell}");
                if leaked || cell.starts_with("LEAK") {
                    leaks.push(row.clone());
                }
                rows.push(row);
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "existing-session TOCTOU LEAK cells:\n{}\n\nfull matrix ({} rows):\n{}",
        leaks.join("\n"),
        rows.len(),
        rows.iter().take(40).cloned().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn existing_session_toctou_matrix_summary_table() {
    let mut by_class: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> =
        std::collections::BTreeMap::new();
    let vault = toctou_vault();
    for learn in [
        ExistingLearn::EventFirst,
        ExistingLearn::ReplyBeforeLearn,
        ExistingLearn::NeverLearn,
        ExistingLearn::LearnDuring,
    ] {
        for (label, command, params) in existing_commands() {
            let (cell, _, _) = existing_run_cell(&vault, learn, label, &command, params);
            by_class
                .entry(label.to_string())
                .or_default()
                .insert(format!("{learn:?}"), cell);
        }
    }
    let mut table = String::from("class\\learn\tEventFirst\tReplyBeforeLearn\tNeverLearn\tLearnDuring\n");
    for (class, cells) in &by_class {
        table.push_str(class);
        for learn in ["EventFirst", "ReplyBeforeLearn", "NeverLearn", "LearnDuring"] {
            table.push('\t');
            table.push_str(cells.get(learn).map(String::as_str).unwrap_or("?"));
        }
        table.push('\n');
    }
    for live in toctou_lives() {
        if live == vault {
            continue;
        }
        for learn in [
            ExistingLearn::EventFirst,
            ExistingLearn::NeverLearn,
            ExistingLearn::LearnDuring,
        ] {
            let (cell, leaked, _) = existing_run_cell(
                &live,
                learn,
                "Runtime.evaluate",
                &toctou_method("Runtime", "evaluate"),
                json!({ "expression": "document.body.innerText", "returnByValue": true }),
            );
            assert!(!leaked, "wrapper live={live} learn={learn:?} leaked: {cell}");
            let (shot, leaked_shot, _) = existing_run_cell(
                &live,
                learn,
                "Page.captureScreenshot",
                &toctou_method("Page", "captureScreenshot"),
                json!({}),
            );
            assert!(!leaked_shot, "wrapper screenshot live={live} learn={learn:?} leaked: {shot}");
        }
    }
    assert!(
        table.contains("Runtime.evaluate"),
        "summary table must include Runtime.evaluate rows:\n{table}"
    );
    eprintln!("EXISTING_SESSION_TOCTOU_TABLE\n{table}");
}

#[test]
fn existing_session_ordinary_commit_reply_stays_allowed() {
    let ordinary_b = "https://ordinary.example/other";
    let (mut cdp, state, session) = existing_client(ordinary_b);
    {
        let mut state = state.borrow_mut();
        state.live = toctou_frame(ordinary_b, "L2");
        state.commits_on_forward = true;
        state.nav_events = ToctouNavEvents::All;
        state.event_timing = ToctouEventTiming::AfterFirstRead;
        state.committed = false;
    }
    let out = cdp.call(
        &toctou_method("Runtime", "evaluate"),
        json!({ "expression": "1", "returnByValue": true }),
        Some(&session),
    );
    assert!(
        out.as_ref().is_err_and(|message| message.contains("Cannot find context"))
            || out.as_ref().is_ok_and(|value| !value.to_string().contains(TOCTOU_MARK)),
        "ordinary→ordinary reply must stay allowed or context-destroyed, not marker: {out:?}"
    );
    assert!(!cdp.is_protected_target("T"));
}

#[test]
fn mutation_existing_survive_contexts_never_learn_evaluate_leaks_without_fresh_gate() {
    let live = toctou_vault();
    let (mut cdp, state, session) = existing_client(&live);
    {
        let mut state = state.borrow_mut();
        state.survive_contexts = true;
        state.commits_on_forward = true;
        state.nav_events = ToctouNavEvents::None;
        state.event_timing = ToctouEventTiming::Never;
        state.committed = false;
        state.content_forwards = 0;
        state.content_backend_ok = 0;
    }
    let out = cdp.call(
        &toctou_method("Runtime", "evaluate"),
        json!({ "expression": "1", "returnByValue": true }),
        Some(&session),
    );
    assert!(
        state.borrow().content_backend_ok >= 1,
        "mutation must reach the backend with a surviving context"
    );
    assert!(
        out.as_ref().is_ok_and(|value| value.to_string().contains(TOCTOU_MARK)),
        "never-learn + survive_contexts must LEAK under cached-only DiD (no nav event): {out:?}"
    );
}

#[test]
fn mutation_existing_survive_contexts_reply_before_learn_needs_did_recheck() {
    let live = toctou_vault();
    let (mut cdp, state, session) = existing_client(&live);
    {
        let mut state = state.borrow_mut();
        state.survive_contexts = true;
        state.commits_on_forward = true;
        state.nav_events = ToctouNavEvents::AfterReply;
        state.event_timing = ToctouEventTiming::AfterFirstRead;
        state.committed = false;
        state.content_forwards = 0;
        state.content_backend_ok = 0;
    }
    let out = cdp.call(
        &toctou_method("Runtime", "evaluate"),
        json!({ "expression": "1", "returnByValue": true }),
        Some(&session),
    );
    assert!(
        state.borrow().content_backend_ok >= 1,
        "backend must answer before DiD"
    );
    assert!(
        out.as_ref().is_ok_and(|value| value.to_string().contains(TOCTOU_MARK)),
        "reply-before-learn + survive_contexts must LEAK because DiD is cached-state only and events land after the reply: {out:?}"
    );
}

#[test]
fn mutation_existing_survive_contexts_learn_during_held_after_latch() {
    let live = toctou_vault();
    let (mut cdp, state, session) = existing_client(&live);
    {
        let mut state = state.borrow_mut();
        state.survive_contexts = true;
        state.commits_on_forward = true;
        state.nav_events = ToctouNavEvents::All;
        state.event_timing = ToctouEventTiming::AfterFirstRead;
        state.committed = false;
        state.content_forwards = 0;
        state.content_backend_ok = 0;
    }
    let out = cdp.call(
        &toctou_method("Runtime", "evaluate"),
        json!({ "expression": "1", "returnByValue": true }),
        Some(&session),
    );
    assert!(
        state.borrow().content_backend_ok >= 1,
        "backend must answer Ok(marker); Chrome-faithful would destroy the context instead"
    );
    assert!(
        cdp.is_protected_target("T"),
        "learn-during events must latch before reply admission"
    );
    assert!(
        out.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")),
        "learn-during with surviving contexts must refuse after latch (verify_execution before DiD): {out:?}"
    );
    if let Err(message) = out {
        assert!(!message.contains(TOCTOU_MARK), "{message}");
    }
}

#[test]
fn mutation_existing_survive_learn_during_without_latch_held_by_did() {
    let live = toctou_vault();
    let (mut cdp, state, session) = existing_client(&live);
    cdp.skip_protect = true;
    cdp.skip_verify_execution = true;
    {
        let mut state = state.borrow_mut();
        state.survive_contexts = true;
        state.commits_on_forward = true;
        state.nav_events = ToctouNavEvents::All;
        state.event_timing = ToctouEventTiming::AfterFirstRead;
        state.committed = false;
        state.content_forwards = 0;
        state.content_backend_ok = 0;
    }
    let out = cdp.call(
        &toctou_method("Runtime", "evaluate"),
        json!({ "expression": "1", "returnByValue": true }),
        Some(&session),
    );
    assert!(
        state.borrow().content_backend_ok >= 1,
        "backend must answer Ok(marker) before DiD"
    );
    assert!(
        !cdp.is_protected_target("T"),
        "skip_protect isolates DiD from latch/block"
    );
    assert!(
        out.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")),
        "without latch/verify_execution, LearnDuring+survive must be held by verify_execution_document_unchanged_or_ordinary: {out:?}"
    );
    if let Err(message) = out {
        assert!(!message.contains(TOCTOU_MARK), "{message}");
    }
}

#[test]
fn mutation_existing_survive_never_learn_screenshot_held_by_verify_checks() {
    let (cell, leaked, _) = existing_run_cell_opts(
        &toctou_vault(),
        ExistingLearn::NeverLearn,
        "Page.captureScreenshot",
        &toctou_method("Page", "captureScreenshot"),
        json!({}),
        true,
    );
    assert!(!leaked, "NeverLearn+survive screenshot must not return marker: {cell}");
    assert!(
        cell.starts_with("REFUSED("),
        "NeverLearn+survive screenshot must be held by post-reply verify_checks: {cell}"
    );
}

#[test]
fn mutation_existing_event_first_latches_protected_target() {
    let live = toctou_vault();
    let (mut cdp, state, session) = existing_client(&live);
    existing_deliver_event_first(&mut cdp, &state, &session);
    assert!(
        cdp.is_protected_target("T"),
        "EventFirst learn must latch protected_targets"
    );
    let out = cdp.call(
        &toctou_method("Page", "captureScreenshot"),
        json!({}),
        Some(&session),
    );
    assert!(
        out.as_ref().is_err_and(|message| message.starts_with("slicc: refused:")),
        "event-first must refuse screenshot via latch: {out:?}"
    );
    assert!(!out.as_ref().is_ok_and(|value| value.to_string().contains(TOCTOU_MARK)));
}


#[test]
fn existing_session_dump_matrix_for_mutation_harness() {
    let path = match std::env::var("SLICC_MUTATION_DUMP") {
        Ok(path) if !path.is_empty() => path,
        _ => return,
    };
    let survive = std::env::var("SLICC_MUTATION_SURVIVE").ok().as_deref() == Some("1");
    let vault = toctou_vault();
    let mut table = String::from("class\\learn\tEventFirst\tReplyBeforeLearn\tNeverLearn\tLearnDuring\n");
    let mut by_class: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> =
        std::collections::BTreeMap::new();
    let mut leaks = Vec::new();
    for learn in [
        ExistingLearn::EventFirst,
        ExistingLearn::ReplyBeforeLearn,
        ExistingLearn::NeverLearn,
        ExistingLearn::LearnDuring,
    ] {
        for (label, command, params) in existing_commands() {
            let (cell, leaked, _) = existing_run_cell_opts(&vault, learn, label, &command, params, survive);
            if leaked || cell.starts_with("LEAK") {
                leaks.push(format!("{learn:?}\t{label}\t{cell}"));
            }
            by_class
                .entry(label.to_string())
                .or_default()
                .insert(format!("{learn:?}"), cell);
        }
    }
    for (class, cells) in &by_class {
        table.push_str(class);
        for learn in ["EventFirst", "ReplyBeforeLearn", "NeverLearn", "LearnDuring"] {
            table.push('\t');
            table.push_str(cells.get(learn).map(String::as_str).unwrap_or("?"));
        }
        table.push('\n');
    }
    if !leaks.is_empty() {
        table.push_str("\nLEAK_ROWS\n");
        table.push_str(&leaks.join("\n"));
        table.push('\n');
    }
    std::fs::write(&path, &table).unwrap();
    eprintln!("MUTATION_DUMP wrote {path} leaks={}", leaks.len());
}

#[test]
fn lock_existing_event_first_screenshot_refuses_without_marker() {
    let (cell, leaked, _) = existing_run_cell(
        &toctou_vault(),
        ExistingLearn::EventFirst,
        "Page.captureScreenshot",
        &toctou_method("Page", "captureScreenshot"),
        json!({}),
    );
    assert!(!leaked, "EventFirst screenshot must not return marker: {cell}");
    assert!(cell.starts_with("REFUSED("), "EventFirst screenshot must refuse: {cell}");
}

#[test]
fn lock_existing_never_learn_screenshot_refuses_without_marker() {
    let (cell, leaked, _) = existing_run_cell(
        &toctou_vault(),
        ExistingLearn::NeverLearn,
        "Page.captureScreenshot",
        &toctou_method("Page", "captureScreenshot"),
        json!({}),
    );
    assert!(!leaked, "NeverLearn screenshot must not return marker: {cell}");
    assert!(cell.starts_with("REFUSED("), "NeverLearn screenshot must refuse: {cell}");
}

#[test]
fn lock_existing_learn_during_evaluate_refuses_without_marker() {
    let (cell, leaked, _) = existing_run_cell(
        &toctou_vault(),
        ExistingLearn::LearnDuring,
        "Runtime.evaluate",
        &toctou_method("Runtime", "evaluate"),
        json!({ "expression": "1", "returnByValue": true }),
    );
    assert!(!leaked, "LearnDuring evaluate must not return marker: {cell}");
    assert!(
        cell.starts_with("REFUSED(") || cell == "CONTEXT-DESTROYED",
        "LearnDuring evaluate must refuse or context-destroy: {cell}"
    );
}

#[test]
fn lock_existing_chrome_faithful_evaluate_backend_is_not_ok_marker() {
    let live = toctou_vault();
    for learn in [
        ExistingLearn::ReplyBeforeLearn,
        ExistingLearn::NeverLearn,
        ExistingLearn::LearnDuring,
    ] {
        let (mut cdp, state, session) = existing_client(&live);
        existing_arm(&state, learn);
        let before_ok = state.borrow().content_backend_ok;
        let out = cdp.call(
            &toctou_method("Runtime", "evaluate"),
            json!({ "expression": "1", "returnByValue": true }),
            Some(&session),
        );
        assert_eq!(
            state.borrow().content_backend_ok, before_ok,
            "Chrome-faithful evaluate under {learn:?} must not get Ok(marker) from the backend (got {out:?})"
        );
        assert!(
            out.as_ref().is_err_and(|message| message.starts_with("slicc: refused:") || message.contains("Cannot find context")),
            "Chrome-faithful evaluate under {learn:?} must refuse or context-destroy: {out:?}"
        );
        if let Err(message) = &out {
            assert!(!message.contains(TOCTOU_MARK), "{message}");
        }
    }
}
