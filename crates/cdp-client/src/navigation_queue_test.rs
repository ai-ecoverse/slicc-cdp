struct NavigationQueueBackend {
    inbox: VecDeque<String>,
    flight: Option<(Value, String)>,
    guard: bool,
}

impl Transport for NavigationQueueBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let command: Value = serde_json::from_str(text).unwrap();
        let method = command["method"].as_str().unwrap();
        if matches!(method, "Page.navigate" | "Runtime.evaluate") {
            self.flight = Some((command["id"].clone(), method.to_string()));
            self.inbox.push_back(json!({"method":"Fetch.requestPaused","sessionId":if self.guard {"G"} else {"S"},"params":{"requestId":"PAUSE","networkId":"NET","resourceType":"Document","request":{"url":"https://ordinary.invalid/queue","method":"GET","headers":{}}}}).to_string());
        } else {
            if method == "Fetch.continueRequest" {
                let (id, method) = self.flight.take().unwrap();
                let result = if method == "Page.navigate" { json!({"frameId":"ACTUAL-FRAME","loaderId":"ACTUAL-LOADER"}) } else { json!({"result":{"type":"string","value":"completed-fetch"}}) };
                self.inbox.push_back(json!({"id":id,"result":result}).to_string());
            }
            self.inbox.push_back(json!({"id":command["id"],"result":{}}).to_string());
        }
        Ok(())
    }
    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { Ok(self.inbox.pop_front()) }
}

fn navigation_queue_backend(guard: bool) -> Cdp {
    Cdp::from_transport(Box::new(NavigationQueueBackend { inbox: VecDeque::new(), flight: None, guard }))
}

#[test]
fn navigation_manual_fetch_queue_returns_the_real_response_without_deadlock() {
    let mut cdp = navigation_queue_backend(false);
    let id = cdp.start_navigation(json!({"url":"https://ordinary.invalid/queue"}), Some("S")).unwrap();
    assert!(cdp.poll_navigation(id).is_none());
    cdp.pump(Instant::now()).unwrap();
    let paused = cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap();
    assert_eq!(paused.network_id.as_deref(), Some("NET"));
    cdp.call("Fetch.continueRequest", json!({"requestId":paused.request_id}), Some("S")).unwrap();
    let result = cdp.poll_navigation(id).unwrap().unwrap();
    assert_eq!(result["frameId"], "ACTUAL-FRAME");
    assert_eq!(result["loaderId"], "ACTUAL-LOADER");
    assert!(cdp.poll_navigation(id).is_none());
}

#[test]
fn navigation_playwright_route_decider_cannot_queue_a_dependent_pause() {
    let mut cdp = navigation_queue_backend(false);
    cdp.set_decider("S", Some(Box::new(|_| crate::fetch::Decision::default())));
    let result = cdp.call_for("Page.navigate", json!({"url":"https://ordinary.invalid/queue"}), Some("S"), Duration::from_millis(100)).unwrap();
    assert_eq!(result["frameId"], "ACTUAL-FRAME");
    assert!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().is_none());
}

#[test]
fn navigation_curlwright_guard_pause_is_answered_inside_synchronous_evaluation() {
    let mut cdp = navigation_queue_backend(true);
    cdp.guard_sessions.insert("G".into());
    let result = cdp.call_for("Runtime.evaluate", json!({"expression":"void 0","returnByValue":true}), Some("S"), Duration::from_millis(100)).unwrap();
    assert_eq!(result["result"]["value"], "completed-fetch");
    assert!(cdp.next_paused(None, Duration::ZERO).unwrap().is_none());
}
