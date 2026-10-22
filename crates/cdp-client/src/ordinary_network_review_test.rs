struct OrdinaryReviewTransport { inbox: VecDeque<String> }
impl Transport for OrdinaryReviewTransport {
    fn send_text(&mut self, _: &str) -> Result<(), String> { Ok(()) }
    fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { Ok(self.inbox.pop_front()) }
}

fn ordinary_review_request() -> Value {
    json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"ORD","type":"Document","documentURL":"https://ordinary.invalid/page","request":{"url":"https://ordinary.invalid/page","method":"GET","headers":{}}}})
}

#[test]
fn ordinary_successful_headers_may_mention_protected_origins_without_taint_or_failure() {
    for headers in [
        json!({"Content-Security-Policy":"frame-ancestors https://seven.sliccy.ai"}),
        json!({"Access-Control-Allow-Origin":"https://seven.sliccy.ai"}),
        json!({"X-Documentation":"https://seven.sliccy.ai/private?reference=PUBLIC_REFERENCE"}),
        json!({"Link":"<https://seven.sliccy.ai>; rel=preconnect"}),
        json!({"Content-Location":"https://seven.sliccy.ai/private"}),
        json!({"Location":"https://seven.sliccy.ai/private"}),
    ] {
        let mut cdp=Cdp::from_transport(Box::new(OrdinaryReviewTransport{inbox:VecDeque::new()}));
        cdp.absorb(ordinary_review_request());
        cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"ORD","type":"Document","response":{"url":"https://ordinary.invalid/page","status":200,"headers":headers}}}));
        cdp.absorb(json!({"method":"Network.loadingFinished","sessionId":"S","params":{"requestId":"ORD","timestamp":1.0}}));
        assert!(!cdp.redirects.contains("ORD"),"{headers}");
        assert_eq!(cdp.events().iter().filter(|event|event["method"]=="Network.responseReceived").count(),1,"{headers}");
        assert_eq!(cdp.events().iter().filter(|event|event["method"]=="Network.loadingFinished").count(),1);
        assert!(!cdp.events().iter().any(|event|event["method"]=="Network.loadingFailed"));
    }
}

#[test]
fn ordinary_late_request_and_response_extra_info_are_delivered_through_pump_without_reopening() {
    let response=json!({"method":"Network.responseReceivedExtraInfo","sessionId":"S","params":{"requestId":"ORD","statusCode":200,"headers":{"X-Control":"public-response"}}});
    let request=json!({"method":"Network.requestWillBeSentExtraInfo","sessionId":"S","params":{"requestId":"ORD","headers":{"X-Control":"public-request"}}});
    let finished=json!({"method":"Network.loadingFinished","sessionId":"S","params":{"requestId":"ORD","timestamp":1.0}});
    for terminal in [finished.clone(),json!({"method":"Network.loadingFailed","sessionId":"S","params":{"requestId":"ORD","errorText":"ordinary backend failure","timestamp":1.0}})] {
        let inbox=[ordinary_review_request(),terminal.clone(),response.clone(),request.clone(),terminal.clone()].into_iter().map(|event|event.to_string()).collect();
        let mut cdp=Cdp::from_transport(Box::new(OrdinaryReviewTransport{inbox}));
        while cdp.pump(Instant::now()+Duration::from_secs(1)).unwrap() {}
        assert_eq!(cdp.events().len(),4,"{:?}",cdp.events());
        assert_eq!(cdp.events()[2],response);
        assert_eq!(cdp.events()[3],request);
        assert!(cdp.seen_network.is_empty());
    }
    for missing in [true,false] {
        let mut cdp=Cdp::from_transport(Box::new(OrdinaryReviewTransport{inbox:VecDeque::new()}));
        if !missing {
            cdp.absorb(ordinary_review_request());
            cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"ORD","request":{"url":"https://seven.sliccy.ai/private","headers":{}}}}));
        }
        cdp.absorb(finished.clone());
        let before=cdp.events.len();
        cdp.absorb(response.clone());cdp.absorb(request.clone());
        assert_eq!(cdp.events.len(),before);
    }
}

#[test]
fn ordinary_mentions_keep_body_reads_while_private_resource_aliases_keep_only_body_refusal() {
    for (headers, alias) in [
        (json!({"Content-Security-Policy":"frame-ancestors https://seven.sliccy.ai"}), false),
        (json!({"Link":"<https://seven.sliccy.ai>; rel=preconnect"}), false),
        (json!({"X-Documentation":"https://seven.sliccy.ai/private?reference=PUBLIC_REFERENCE"}), false),
        (json!({"Content-Location":"https://seven.sliccy.ai/private?code=PRIVATE_ALIAS"}), true),
    ] {
        let mut cdp = redirect_body_backend("ORDINARY_BODY_READ_CONTROL".into());
        cdp.absorb(ordinary_review_request());
        cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"ORD","response":{"url":"https://ordinary.invalid/page","status":200,"headers":headers}}}));
        cdp.absorb(json!({"method":"Network.loadingFinished","sessionId":"S","params":{"requestId":"ORD"}}));
        assert!(!cdp.redirects.contains("ORD"));
        assert_eq!(cdp.events().last().unwrap()["method"], "Network.loadingFinished");
        let result = cdp.call("Network.getResponseBody", json!({"requestId":"ORD"}), Some("S"));
        if alias { assert!(result.unwrap_err().starts_with("slicc: refused:")); }
        else { assert_eq!(result.unwrap()["body"], "ORDINARY_BODY_READ_CONTROL"); }
    }
}

#[test]
fn real_protected_location_refresh_and_link_carriers_remain_tainted() {
    for (status,headers) in [
        (302,json!({"Location":"https://seven.sliccy.ai/private?code=REAL_CARRIER_SECRET"})),
        (302,json!({"Location":"//seven.sliccy.ai/private?code=REAL_CARRIER_SECRET"})),
        (200,json!({"Refresh":"0;url=https://seven.sliccy.ai/private?code=REAL_CARRIER_SECRET"})),
        (200,json!({"Link":"<https://seven.sliccy.ai/private?code=REAL_CARRIER_SECRET>; rel=preconnect"})),
    ] {
        let mut cdp=Cdp::from_transport(Box::new(OrdinaryReviewTransport{inbox:VecDeque::new()}));
        cdp.absorb(ordinary_review_request());
        cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"ORD","response":{"url":"https://ordinary.invalid/page","status":status,"headers":headers}}}));
        assert!(cdp.redirects.contains("ORD"),"{headers}");
        assert!(!serde_json::to_string(cdp.events()).unwrap().contains("REAL_CARRIER_SECRET"));
    }
}
