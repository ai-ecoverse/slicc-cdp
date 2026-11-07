struct PlainRetryBackend {
    inner: Backend,
    refusals: usize,
    attach_refused: bool,
}

impl Transport for PlainRetryBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let command: Value = serde_json::from_str(text).unwrap();
        let method = command["method"].as_str().unwrap();
        if method == "Debugger.enable" || method == "Target.attachToTarget" || method == "Page.reload" || method == "Runtime.enable" && command["sessionId"] == "FRESH" && self.refusals > 0 {
            let mut state = self.inner.0.borrow_mut();
            state.sent.push(command.clone());
            let reply = match method {
                "Debugger.enable" => json!({"error":{"code":-32000,"message":"slicc: refused: a browser extension runs scripts in this page, and profiling it would expose them"}}),
                "Target.attachToTarget" if self.attach_refused => json!({"error":{"code":-32000,"message":"slicc: refused: seven’s own pages and the vault are out of reach of browser automation"}}),
                "Target.attachToTarget" => json!({"result":{"sessionId":"FRESH"}}),
                "Runtime.enable" => {
                    self.refusals -= 1;
                    json!({"error":{"code":-32000,"message":"slicc: refused: seven’s own pages and the vault are out of reach of browser automation"}})
                }
                _ => {
                    state.tree["frameTree"]["frame"]["loaderId"] = json!("RELOADED");
                    state.inbox.push_back(event("Runtime.executionContextsCleared", json!({})).to_string());
                    let changed = state.tree["frameTree"]["frame"].clone();
                    state.inbox.push_back(event("Page.frameNavigated", json!({"frame":changed})).to_string());
                    state.inbox.push_back(event("Page.loadEventFired", json!({"timestamp":1})).to_string());
                    json!({"result":{}})
                }
            };
            let mut reply = reply;
            reply["id"] = command["id"].clone();
            if command.get("sessionId").is_some() { reply["sessionId"] = command["sessionId"].clone(); }
            state.inbox.push_back(reply.to_string());
            return Ok(());
        }
        self.inner.send_text(text)
    }

    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
        self.inner.recv_text(deadline)
    }
}

fn plain_retry_client(refusals: usize, attach_refused: bool) -> (Cdp, Rc<RefCell<BackendState>>) {
    let (mut cdp, state) = client();
    cdp.ws = Box::new(PlainRetryBackend { inner: Backend(state.clone()), refusals, attach_refused });
    (cdp, state)
}

fn plain_retry_attempts(state: &BackendState) -> Vec<&Value> {
    state.sent.iter().filter(|command| command["method"] == "Runtime.enable" && command["sessionId"] == "FRESH").collect()
}

#[test]
fn plain_fallback_retry_two_refusals_then_same_session_acceptance_installs_once() {
    let (mut cdp, state) = plain_retry_client(2, false);
    let start = Instant::now();
    let result = cdp.start_webmcp_for("H", true);
    assert!(result.is_ok(), "{result:?}");
    assert!(start.elapsed() >= Duration::from_millis(200));
    let state = state.borrow();
    assert_eq!(plain_retry_attempts(&state).len(), 3);
    assert!(state.api);
    assert_eq!(state.sent.iter().filter(|command| command["method"] == "Target.attachToTarget").count(), 1);
    assert!(!state.sent.iter().any(|command| command["method"] == "Target.detachFromTarget"));
    assert_eq!(state.sent.iter().filter(|command| command["method"] == "Page.reload").count(), 1);
    assert_eq!(state.sent.iter().filter(|command| command["method"] == "Runtime.evaluate" && command["params"]["expression"].as_str().is_some_and(webmcp_shim_install)).count(), 1);
    assert_eq!(cdp.webmcp_session("H"), "FRESH");
}

#[test]
fn plain_fallback_retry_persistent_refusal_is_bounded_with_clear_hint_and_no_install() {
    let (mut cdp, state) = plain_retry_client(usize::MAX, false);
    let start = Instant::now();
    let error = cdp.start_webmcp_for("H", true).unwrap_err();
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(950) && elapsed < Duration::from_millis(1600), "{elapsed:?} {error}");
    assert!(error.contains("other-extension fallback could not verify the reloaded document; tools are unavailable"), "{error}");
    let state = state.borrow();
    assert!((2..=10).contains(&plain_retry_attempts(&state).len()));
    assert!(!state.api);
    assert_eq!(state.sent.iter().filter(|command| command["method"] == "Target.attachToTarget").count(), 1);
    assert_eq!(state.sent.iter().filter(|command| command["method"] == "Target.detachFromTarget" && command["params"]["sessionId"] == "FRESH").count(), 1);
    assert!(!state.sent.iter().any(|command| command["method"] == "Runtime.evaluate" && command["sessionId"] == "FRESH"));
}

#[test]
fn plain_fallback_retry_attach_refusal_never_retries_or_installs() {
    let (mut cdp, state) = plain_retry_client(2, true);
    let start = Instant::now();
    let error = cdp.start_webmcp_for("H", true).unwrap_err();
    assert!(start.elapsed() < Duration::from_millis(500));
    assert!(error.contains("other-extension fallback could not verify the reloaded document; tools are unavailable"), "{error}");
    let state = state.borrow();
    assert_eq!(state.sent.iter().filter(|command| command["method"] == "Target.attachToTarget").count(), 1);
    assert!(plain_retry_attempts(&state).is_empty());
    assert!(!state.api);
    assert!(!state.sent.iter().any(|command| command["method"] == "Target.detachFromTarget"));
}
