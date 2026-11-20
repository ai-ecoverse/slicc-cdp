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

    fn commit(&mut self, inbox: &mut VecDeque<String>, session: &str, deliver_events: bool) {
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
        inbox.push_back(
            json!({
                "method": toctou_method("Target", "targetInfoChanged"),
                "params": { "targetInfo": self.info.clone() },
            })
            .to_string(),
        );
        if deliver_events {
            inbox.push_back(
                json!({
                    "method": toctou_method("Page", "frameNavigated"),
                    "sessionId": session,
                    "params": { "frame": self.live.clone(), "type": "Navigation" },
                })
                .to_string(),
            );
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
            css.as_str(),
            mouse.as_str(),
            key.as_str(),
            insert.as_str(),
            net_body.as_str(),
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
            state.contexts.push(ToctouContext {
                id: 1,
                unique: format!("ctx-{session}"),
                objects: ["1".into()].into_iter().collect(),
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
                            "origin": "https://ordinary.example",
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
            self.inbox.push_back(reply_ok(json!({ "root": { "nodeId": 1, "nodeName": "HTML", "documentURL": url } })));
        } else if called == query || called == describe || called == resolve {
            if protected_doc {
                self.inbox.push_back(reply_ok(json!({ "nodeId": 2, "node": { "nodeId": 2, "nodeName": "BODY", "documentURL": format!("body {TOCTOU_MARK}") } })));
            } else {
                self.inbox.push_back(reply_ok(json!({ "nodeId": 2, "node": { "nodeId": 2, "nodeName": "BODY" } })));
            }
        } else if called == outer {
            if protected_doc {
                self.inbox.push_back(reply_ok(json!({ "outerHTML": format!("<html>{TOCTOU_MARK}</html>") })));
            } else {
                self.inbox.push_back(reply_ok(json!({ "outerHTML": "<html></html>" })));
            }
        } else if called == snap {
            if protected_doc {
                self.inbox.push_back(reply_ok(json!({ "documents": [{ "documentURL": format!("snap {TOCTOU_MARK}") }], "strings": [TOCTOU_MARK] })));
            } else {
                self.inbox.push_back(reply_ok(json!({ "documents": [], "strings": [] })));
            }
        } else if called == screenshot || called == pdf {
            self.inbox.push_back(reply_ok(json!({ "data": if protected_doc { format!("marker-{TOCTOU_MARK}") } else { "iVBORw0KGgo=".into() } })));
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
        } else if called == ax {
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
