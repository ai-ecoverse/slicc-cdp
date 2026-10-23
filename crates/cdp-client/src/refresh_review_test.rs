#[test]
fn browser_accepted_refresh_forms_refuse_private_bodies_in_every_response_carrier() {
    let destination = "https://seven.sliccy.ai/auth/callback?code=REFRESH_REVIEW_PRIVATE&state=REFRESH_REVIEW_PRIVATE";
    for value in [format!("0;url={destination}"),format!("0.5; url={destination}"),format!("0; {destination}"),format!("1.; URL='{destination}'"),format!(" 02.75 , URL = \"{destination}\""),format!("bad;url={destination}"),"0;url=https://[".to_string()] {
        for method in ["Network.responseReceived","Network.responseReceivedExtraInfo","Fetch.requestPaused"] {
            let mut cdp=redirect_body_backend("PRIVATE_REDIRECT_BODY".into());
            cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","request":{"url":"https://example.test/start","headers":{}}}}));
            let params=match method {
                "Network.responseReceived"=>json!({"requestId":"R","response":{"url":"https://example.test/start","status":200,"headers":{"Refresh":value}}}),
                "Network.responseReceivedExtraInfo"=>json!({"requestId":"R","statusCode":200,"headers":{"Refresh":value},"headersText":format!("HTTP/1.1 200\r\nRefresh: {value}\r\n\r\n")}),
                _=>json!({"requestId":"F","networkId":"R","resourceType":"Document","request":{"url":"https://example.test/start","headers":{},"method":"GET"},"responseStatusCode":200,"responseHeaders":[{"name":"Refresh","value":value}]}),
            };
            cdp.absorb(json!({"method":method,"sessionId":"S","params":params}));
            assert!(cdp.call("Network.getResponseBody",json!({"requestId":"R"}),Some("S")).unwrap_err().starts_with("slicc: refused:"),"{method}: {value}");
            if method=="Fetch.requestPaused" {assert!(cdp.call("Fetch.getResponseBody",json!({"requestId":"F"}),Some("S")).unwrap_err().starts_with("slicc: refused:"));}
            let visible=format!("{:?} {:?}",cdp.events(),cdp.paused);
            assert!(!visible.contains("REFRESH_REVIEW_PRIVATE"));
        }
    }
}

#[test]
fn ordinary_refresh_forms_remain_readable_and_complete_without_a_synthetic_failure() {
    for value in ["0;url=https://ordinary.invalid/end?control=visible","0.5; url=/ordinary?control=visible","0; https://ordinary.invalid/end","1.; URL='https://ordinary.invalid/end'"," 2.5 , URL = \"/end\"","0","1.5;"] {
        let mut cdp=redirect_body_backend("ORDINARY_REFRESH_BODY".into());
        cdp.absorb(json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","request":{"url":"https://ordinary.invalid/start","headers":{}}}}));
        cdp.absorb(json!({"method":"Network.responseReceived","sessionId":"S","params":{"requestId":"R","response":{"url":"https://ordinary.invalid/start","status":200,"headers":{"Refresh":value,"X-Control":"ordinary"}}}}));
        assert_eq!(cdp.call("Network.getResponseBody",json!({"requestId":"R"}),Some("S")).unwrap()["body"],"ORDINARY_REFRESH_BODY","{value}");
        cdp.absorb(json!({"method":"Network.loadingFinished","sessionId":"S","params":{"requestId":"R"}}));
        assert!(!cdp.events().iter().any(|event|event["method"]=="Network.loadingFailed"));
        assert_eq!(cdp.events().iter().find(|event|event["method"]=="Network.responseReceived").unwrap()["params"]["response"]["headers"]["Refresh"],value);
    }
}
