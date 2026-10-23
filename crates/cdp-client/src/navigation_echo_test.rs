struct NavigationEchoTransport { inbox: VecDeque<String> }
impl Transport for NavigationEchoTransport {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let message: Value = serde_json::from_str(text).unwrap();
        if message["method"] == "Page.navigate" {
            self.inbox.push_back(json!({"method":"Network.requestWillBeSent","sessionId":message["sessionId"],"params":{"requestId":"LOADER","type":"Document","request":{"url":message["params"]["url"],"headers":{}}}}).to_string());
        }
        self.inbox.push_back(json!({"id":message["id"],"result":{"frameId":"FRAME","loaderId":"LOADER"}}).to_string());
        Ok(())
    }
    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { Ok(self.inbox.pop_front()) }
}

#[test]
fn accepted_ordinary_navigation_echo_is_byte_exact_but_unowned_urls_are_redacted() {
    let url="https://ordinary.invalid/page?to=https%3A%2F%2Fseven.sliccy.ai%2Fcanary%2Fowned";
    for queued in [false,true] {
        let mut cdp=Cdp::from_transport(Box::new(NavigationEchoTransport{inbox:VecDeque::new()}));
        if queued {
            let id=cdp.start_navigation(json!({"url":url}),Some("S")).unwrap();
            while cdp.poll_navigation(id).is_none() { assert!(cdp.pump(Instant::now()+Duration::from_secs(1)).unwrap()); }
        } else { cdp.call("Page.navigate",json!({"url":url}),Some("S")).unwrap(); }
        assert_eq!(cdp.events().iter().find(|event|event["method"]=="Network.requestWillBeSent").unwrap()["params"]["request"]["url"],url);
    }
    let mut cdp=url_backend("about:blank");
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"UNKNOWN","type":"Document","request":{"url":url,"headers":{}}}}));
    assert_ne!(cdp.events().last().unwrap()["params"]["request"]["url"],url);
    cdp.blocked.insert("S".into());let before=cdp.events.len();
    cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"PRIVATE","type":"Document","request":{"url":url,"headers":{}}}}));
    assert_eq!(cdp.events.len(),before);
}
