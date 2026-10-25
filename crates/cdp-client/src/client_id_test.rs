struct ClientIdFixture(std::sync::Arc<std::sync::Mutex<Vec<Value>>>);

impl Transport for ClientIdFixture {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(serde_json::from_str(text).unwrap());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> { Ok(None) }
}

#[test]
fn client_message_id_boundary_is_sent_once_then_exhaustion_never_wraps() {
    let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(ClientIdFixture(sent.clone())));
    cdp.next_id = (1 << 30) - 2;
    assert_eq!(cdp.send("Browser.getVersion", json!({}), None).unwrap(), (1 << 30) - 1);
    assert_eq!(sent.lock().unwrap()[0]["id"], (1 << 30) - 1);
    assert_eq!(cdp.send("Browser.getVersion", json!({}), None).unwrap_err(), "CDP client message ID range exhausted\n");
    assert_eq!(cdp.next_id, (1 << 30) - 1);
    assert_eq!(cdp.send("Browser.getVersion", json!({}), None).unwrap_err(), "CDP client message ID range exhausted\n");
    assert_eq!(sent.lock().unwrap().len(), 1);
}

#[test]
fn client_message_ids_already_outside_the_range_fail_before_transport_or_overflow() {
    let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(ClientIdFixture(sent.clone())));
    for next in [1 << 30, i64::MAX] {
        cdp.next_id = next;
        assert_eq!(cdp.send("Browser.getVersion", json!({}), None).unwrap_err(), "CDP client message ID range exhausted\n");
        assert_eq!(cdp.next_id, next);
    }
    assert!(sent.lock().unwrap().is_empty());
}
