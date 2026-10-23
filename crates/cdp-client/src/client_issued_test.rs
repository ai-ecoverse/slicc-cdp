use crate::cdp::Transport;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

type Reply = Box<dyn FnMut(&Value) -> Vec<Value>>;

struct Wire {
    log: Rc<RefCell<Vec<Value>>>,
    inbox: VecDeque<String>,
    reply: Reply,
}

impl Transport for Wire {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let value: Value = serde_json::from_str(text).unwrap();
        self.log.borrow_mut().push(value.clone());
        for out in (self.reply)(&value) { self.inbox.push_back(out.to_string()); }
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn wire(reply: Reply) -> (Cdp, Rc<RefCell<Vec<Value>>>) {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut cdp = Cdp::from_transport(Box::new(Wire { log: log.clone(), inbox: VecDeque::new(), reply }));
    cdp.fixture_session("S");
    (cdp, log)
}

fn answer(message: &Value) -> Vec<Value> {
    vec![json!({ "id": message["id"], "result": {} })]
}

fn fetch_calls(log: &Rc<RefCell<Vec<Value>>>) -> Vec<(String, Value)> {
    let calls = log.borrow().iter().filter(|item| item["method"].as_str().is_some_and(|method| method.starts_with("Fetch.") || method == "Network.replayXHR"))
        .map(|item| (item["method"].as_str().unwrap().to_string(), item["params"].clone())).collect();
    log.borrow_mut().clear();
    calls
}

fn pause(id: &str, network: &str, url: &str, kind: &str) -> Value {
    json!({ "method": "Fetch.requestPaused", "sessionId": "S", "params": { "requestId": id, "networkId": network, "frameId": "F1", "resourceType": kind, "request": { "url": url, "method": "GET", "headers": {} } } })
}

fn hop(id: &str, network: &str, url: &str, from: &str) -> Value {
    let mut event = pause(id, network, url, "Fetch");
    event["params"]["redirectedRequestId"] = json!(from);
    event
}

fn respond(id: &str, network: &str, url: &str, status: u64, location: &str) -> Value {
    let mut event = pause(id, network, url, "Fetch");
    event["params"]["responseStatusCode"] = json!(status);
    event["params"]["responseHeaders"] = json!([{ "name": "Location", "value": location }]);
    event
}

fn auth(id: &str, url: &str) -> Value {
    json!({ "method": "Fetch.authRequired", "sessionId": "S", "params": { "requestId": id, "frameId": "F1", "resourceType": "Fetch", "request": { "url": url, "method": "GET", "headers": {} }, "authChallenge": { "source": "Server", "origin": url, "scheme": "basic", "realm": "r" } } })
}

fn armed(client: Value, auth: bool) -> (String, Value) {
    let mut patterns = client.as_array().cloned().unwrap_or_default();
    patterns.push(json!({ "urlPattern": "*", "requestStage": "Request" }));
    ("Fetch.enable".into(), json!({ "patterns": patterns, "handleAuthRequests": auth }))
}

fn call(method: &str, params: Value) -> (String, Value) {
    (method.into(), params)
}

fn failed(id: &str) -> (String, Value) {
    call("Fetch.failRequest", json!({ "requestId": id, "errorReason": "BlockedByClient" }))
}

fn plain(id: &str) -> (String, Value) {
    call("Fetch.continueRequest", json!({ "requestId": id }))
}

fn start(cdp: &mut Cdp, log: &Rc<RefCell<Vec<Value>>>, patterns: Value) {
    cdp.call("Fetch.enable", json!({ "patterns": patterns }), Some("S")).unwrap();
    cdp.absorb(pause("R1", "N1", "https://app.example/start", "Fetch"));
    assert_eq!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap().request_id, "R1");
    fetch_calls(log);
}

fn refused(result: Result<Value, String>) -> bool {
    result.is_err_and(|message| message.starts_with("slicc: refused:") && message != crate::cdp::UNSEEN)
}

fn protect_redirector(cdp: &mut Cdp) {
    cdp.absorb(json!({ "method": "Network.requestWillBeSent", "sessionId": "S", "params": { "requestId": "T", "request": { "url": "https://app.example/redirector" } } }));
    cdp.absorb(json!({ "method": "Network.responseReceivedExtraInfo", "sessionId": "S", "params": { "requestId": "T", "statusCode": 302, "headers": { "Location": "https://seven.sliccy.ai/private" } } }));
}

#[test]
fn an_overridden_request_has_every_hop_classified_before_it_is_sent() {
    let (mut cdp, log) = wire(Box::new(answer));
    let patterns = json!([{ "urlPattern": "https://app.example/start*" }]);
    start(&mut cdp, &log, patterns.clone());
    assert!(refused(cdp.call("Fetch.continueRequest", json!({ "requestId": "R1", "url": "https://seven.sliccy.ai/" }), Some("S"))));
    assert!(refused(cdp.call("Fetch.continueRequest", json!({ "requestId": "R1", "url": 7 }), Some("S"))));
    assert!(refused(cdp.call("Fetch.continueRequest", json!({ "requestId": "R1", "url": "https://user:pass@app.example/" }), Some("S"))));
    cdp.call("Fetch.continueRequest", json!({ "requestId": "R1", "url": "https://app.example/hop" }), Some("S")).unwrap();
    cdp.absorb(hop("R2", "N1", "https://app.example/next", "R1"));
    cdp.absorb(hop("R3", "N1", "https://app.example/start/again", "R2"));
    assert_eq!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap().request_id, "R3");
    cdp.call("Fetch.continueRequest", json!({ "requestId": "R3" }), Some("S")).unwrap();
    cdp.absorb(hop("R4", "N1", "https://seven.sliccy.ai/private", "R3"));
    cdp.absorb(hop("R5", "OTHER", "https://vault.sliccy.ai/private", "R2"));
    cdp.absorb(pause("R6", "N1", "https://vault.sliccy.ai/x", "Fetch"));
    assert!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().is_none());
    assert_eq!(fetch_calls(&log), [
        armed(patterns, false),
        call("Fetch.continueRequest", json!({ "requestId": "R1", "url": "https://app.example/hop" })),
        plain("R2"),
        plain("R3"),
        failed("R4"),
        failed("R5"),
        failed("R6"),
    ]);
    assert_eq!(cdp.call("Fetch.continueRequest", json!({ "requestId": "R4" }), Some("S")).unwrap_err(), crate::cdp::UNSEEN);
}

#[test]
fn a_tracked_redirect_response_is_classified_before_it_is_followed() {
    let (mut cdp, log) = wire(Box::new(answer));
    start(&mut cdp, &log, json!([{ "urlPattern": "*", "requestStage": "Response" }, { "urlPattern": "https://app.example/start*" }]));
    cdp.call("Fetch.continueRequest", json!({ "requestId": "R1", "url": "https://app.example/hop" }), Some("S")).unwrap();
    cdp.absorb(respond("R1", "N1", "https://app.example/hop", 200, "https://seven.sliccy.ai/"));
    assert_eq!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap().status, Some(200));
    cdp.absorb(respond("R1", "N1", "https://app.example/hop", 302, "/fine"));
    assert_eq!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap().status, Some(302));
    cdp.absorb(respond("R1", "N1", "https://app.example/hop", 302, "//vault.sliccy.ai/x"));
    assert!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().is_none());
    assert_eq!(fetch_calls(&log).last().unwrap(), &failed("R1"));
}

#[test]
fn a_fulfilled_redirect_has_its_location_and_later_hops_classified() {
    let (mut cdp, log) = wire(Box::new(answer));
    let patterns = json!([{ "urlPattern": "https://app.example/start*" }]);
    start(&mut cdp, &log, patterns.clone());
    let location = |value: &str| json!({ "requestId": "R1", "responseCode": 302, "responseHeaders": [{ "name": "Location", "value": value }] });
    for value in ["https://seven.sliccy.ai/", "//vault.sliccy.ai/x", "/a\u{1}b", "", "data:text/html,x"] {
        assert!(refused(cdp.call("Fetch.fulfillRequest", location(value), Some("S"))), "{value:?}");
    }
    let binary = crate::net::base64_encode(b"Location: https://seven.sliccy.ai/");
    assert!(refused(cdp.call("Fetch.fulfillRequest", json!({ "requestId": "R1", "responseCode": 307, "binaryResponseHeaders": binary }), Some("S"))));
    assert!(refused(cdp.call("Fetch.fulfillRequest", json!({ "requestId": "NOPE", "responseCode": 302, "responseHeaders": [{ "name": "Location", "value": "/a" }] }), Some("S"))));
    protect_redirector(&mut cdp);
    assert!(refused(cdp.call("Fetch.fulfillRequest", location("/redirector"), Some("S"))));
    cdp.call("Fetch.fulfillRequest", location("/next"), Some("S")).unwrap();
    cdp.absorb(hop("R2", "N1", "https://app.example/next", "R1"));
    cdp.absorb(hop("R3", "N1", "https://seven.sliccy.ai/", "R2"));
    assert_eq!(fetch_calls(&log), [armed(patterns, false), call("Fetch.fulfillRequest", location("/next")), plain("R2"), failed("R3")]);
    cdp.absorb(respond("R7", "N7", "https://app.example/start", 200, "x"));
    cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap();
    assert!(refused(cdp.call("Fetch.continueResponse", json!({ "requestId": "R7", "responseCode": 301, "responseHeaders": [{ "name": "location", "value": "https://vault.sliccy.ai/" }] }), Some("S"))));
    cdp.call("Fetch.continueResponse", json!({ "requestId": "R7", "responseCode": 200, "responseHeaders": [{ "name": "location", "value": "https://vault.sliccy.ai/" }] }), Some("S")).unwrap();
}

#[test]
fn network_interception_overrides_are_classified_too() {
    let (mut cdp, _log) = wire(Box::new(answer));
    let raw = crate::net::base64_encode(b"HTTP/1.1 302 Found\r\nLocation: https://seven.sliccy.ai/\r\n\r\n");
    assert!(refused(cdp.call("Network.continueInterceptedRequest", json!({ "interceptionId": "I1", "rawResponse": raw }), Some("S"))));
    assert!(refused(cdp.call("Network.continueInterceptedRequest", json!({ "interceptionId": "I1", "url": "https://vault.sliccy.ai/" }), Some("S"))));
    assert_eq!(cdp.call("Network.continueInterceptedRequest", json!({ "interceptionId": "I1", "url": "https://app.example/" }), Some("S")).unwrap_err(), crate::cdp::UNSEEN);
    assert_eq!(cdp.call("Network.continueInterceptedRequest", json!({ "interceptionId": "I1", "rawResponse": crate::net::base64_encode(b"HTTP/1.1 200 OK\r\n\r\n") }), Some("S")).unwrap_err(), crate::cdp::UNSEEN);
}

#[test]
fn continue_with_auth_classifies_the_redirect_that_follows() {
    let (mut cdp, log) = wire(Box::new(answer));
    let patterns = json!([{ "urlPattern": "https://app.example/*" }]);
    cdp.call("Fetch.enable", json!({ "patterns": patterns, "handleAuthRequests": true }), Some("S")).unwrap();
    cdp.absorb(auth("A1", "https://app.example/auth"));
    let credentials = json!({ "requestId": "A1", "authChallengeResponse": { "response": "ProvideCredentials", "username": "u", "password": "p" } });
    cdp.call("Fetch.continueWithAuth", credentials.clone(), Some("S")).unwrap();
    cdp.absorb(hop("A2", "NA", "https://vault.sliccy.ai/", "A1"));
    cdp.absorb(auth("A3", "https://app.example/again"));
    cdp.absorb(auth("A4", "https://elsewhere.example/"));
    let mut quiet = auth("A5", "https://elsewhere.example/");
    quiet["params"].as_object_mut().unwrap().remove("request");
    cdp.absorb(quiet);
    let auth_default = |id: &str| call("Fetch.continueWithAuth", json!({ "requestId": id, "authChallengeResponse": { "response": "Default" } }));
    let enabled = fetch_calls(&log);
    assert_eq!(enabled[0], call("Fetch.enable", json!({ "patterns": patterns, "handleAuthRequests": true })));
    assert_eq!(enabled[1..], [armed(patterns, true), call("Fetch.continueWithAuth", credentials), failed("A2"), auth_default("A4"), auth_default("A5")]);
    assert!(cdp.events().iter().any(|event| event["params"]["requestId"] == "A3"));
}

#[test]
fn replay_xhr_needs_a_verified_ordinary_original_and_classifies_every_hop() {
    let (mut cdp, log) = wire(Box::new(answer));
    let sent = |id: &str, kind: &str, url: &str| json!({ "method": "Network.requestWillBeSent", "sessionId": "S", "params": { "requestId": id, "type": kind, "request": { "url": url, "method": "GET", "headers": {} } } });
    let redirected = |id: &str, url: &str, from: &str| { let mut event = sent(id, "XHR", url); event["params"]["redirectResponse"] = json!({ "url": from, "status": 302, "headers": { "Location": url } }); event };
    cdp.absorb(sent("X1", "XHR", "https://app.example/flip"));
    cdp.absorb(sent("X1", "XHR", "https://app.example/flip"));
    cdp.absorb(redirected("X1", "https://app.example/landing", "https://app.example/flip"));
    cdp.absorb(sent("X2", "XHR", "https://app.example/taint"));
    cdp.absorb(redirected("X2", "https://vault.sliccy.ai/", "https://app.example/taint"));
    cdp.absorb(sent("X3", "Fetch", "https://app.example/fetch"));
    for id in ["X2", "X3"] {
        assert!(cdp.call("Network.replayXHR", json!({ "requestId": id }), Some("S")).unwrap_err().starts_with("slicc: refused:"), "{id}");
    }
    assert!(cdp.call("Network.replayXHR", json!({ "requestId": "X1" }), Some("")).unwrap_err().starts_with("slicc: refused:"));
    cdp.call("Network.replayXHR", json!({ "requestId": "X1" }), Some("S")).unwrap();
    cdp.absorb(pause("P1", "Y1", "https://app.example/flip", "XHR"));
    cdp.absorb(hop("P2", "Y1", "https://seven.sliccy.ai/private", "P1"));
    cdp.absorb(pause("P3", "Y3", "https://app.example/other", "XHR"));
    assert!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().is_none());
    assert_eq!(fetch_calls(&log), [armed(json!([]), false), call("Network.replayXHR", json!({ "requestId": "X1" })), plain("P1"), failed("P2"), plain("P3")]);
}

#[test]
fn an_armed_session_keeps_page_traffic_live_and_disable_releases_what_it_held() {
    let (mut cdp, log) = wire(Box::new(answer));
    cdp.notify("Fetch.enable", json!({ "handleAuthRequests": true }), Some("S")).unwrap();
    cdp.absorb(pause("R1", "N1", "https://app.example/start", "Fetch"));
    cdp.absorb(auth("A1", "https://app.example/auth"));
    cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap();
    cdp.call("Fetch.continueRequest", json!({ "requestId": "R1", "url": "https://app.example/hop" }), Some("S")).unwrap();
    cdp.absorb(pause("R2", "N2", "https://app.example/page", "Image"));
    cdp.absorb(auth("R2", "https://app.example/page"));
    cdp.absorb(pause("R2", "N2", "https://app.example/page", "Image"));
    let held = cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap();
    assert_eq!(held.request_id, "R2");
    cdp.paused.push_back(held);
    cdp.call("Fetch.disable", json!({}), Some("S")).unwrap();
    assert!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().is_none());
    cdp.absorb(pause("R3", "N3", "https://app.example/page", "Script"));
    cdp.notify("Fetch.enable", json!({ "patterns": "bogus" }), Some("S")).unwrap();
    cdp.absorb(pause("R4", "N4", "https://app.example/page", "Script"));
    cdp.call("Fetch.enable", json!({ "patterns": null }), Some("S")).unwrap();
    cdp.absorb(pause("R5", "N5", "https://app.example/page", "Script"));
    assert_eq!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap().request_id, "R5");
    cdp.call("Fetch.enable", json!({ "patterns": [{ "urlPattern": "*", "resourceType": "Document" }, { "urlPattern": "https://app.example/x\\*", "resourceType": "" }] }), Some("S")).unwrap();
    cdp.absorb(pause("R6", "N6", "https://app.example/x*", "Script"));
    cdp.absorb(pause("R7", "N7", "https://app.example/xy", "Script"));
    assert_eq!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().unwrap().request_id, "R6");
    assert!(cdp.next_paused(Some("S"), Duration::ZERO).unwrap().is_none());
    let auth_default = |id: &str| call("Fetch.continueWithAuth", json!({ "requestId": id, "authChallengeResponse": { "response": "Default" } }));
    let catch_all = json!({ "urlPattern": "*", "requestStage": "Request" });
    assert_eq!(fetch_calls(&log), [
        call("Fetch.enable", json!({ "handleAuthRequests": true })),
        armed(json!([catch_all]), true),
        call("Fetch.continueRequest", json!({ "requestId": "R1", "url": "https://app.example/hop" })),
        auth_default("A1"),
        plain("R2"),
        armed(json!([]), false),
        plain("R3"),
        armed(json!([]), false),
        plain("R4"),
        armed(json!([catch_all]), false),
        armed(json!([{ "urlPattern": "*", "resourceType": "Document" }, { "urlPattern": "https://app.example/x\\*", "resourceType": "" }]), false),
        plain("R7"),
    ]);
}

#[test]
fn disabling_an_unarmed_or_blocked_session_still_disables() {
    let (mut cdp, log) = wire(Box::new(answer));
    cdp.call("Fetch.disable", json!({}), Some("S")).unwrap();
    start(&mut cdp, &log, json!([{ "urlPattern": "*" }]));
    cdp.call("Fetch.continueRequest", json!({ "requestId": "R1", "url": "https://app.example/hop" }), Some("S")).unwrap();
    cdp.blocked.insert("S".into());
    let _ = cdp.call("Fetch.disable", json!({}), Some("S"));
    let calls = fetch_calls(&log);
    assert_eq!(calls.last().unwrap().0, "Fetch.disable");
    assert!(cdp.issued.armed.is_empty());
}

#[test]
fn a_failed_arm_refuses_the_client_issued_request() {
    let (mut cdp, log) = wire(Box::new(|message| {
        if message["method"] == "Fetch.enable" && message["params"]["handleAuthRequests"] == false {
            vec![json!({ "id": message["id"], "error": { "message": "no" } })]
        } else {
            answer(message)
        }
    }));
    start(&mut cdp, &log, json!([{ "urlPattern": "*" }]));
    assert!(refused(cdp.call("Fetch.continueRequest", json!({ "requestId": "R1", "url": "https://app.example/hop" }), Some("S"))));
    assert!(cdp.issued.armed.is_empty());
}

#[test]
fn route_style_deciders_follow_the_client_issued_rule() {
    let (mut cdp, log) = wire(Box::new(answer));
    protect_redirector(&mut cdp);
    cdp.call("Fetch.enable", json!({ "patterns": [{ "urlPattern": "*" }] }), Some("S")).unwrap();
    cdp.set_decider("S", Some(Box::new(|paused: &Paused| {
        let location = if paused.url.ends_with("/a") { "/next" } else if paused.url.ends_with("/b") { "/redirector" } else { "https://seven.sliccy.ai/" };
        crate::fetch::Decision::Fulfill(crate::fetch::Fulfill::new(302).header("Location", location))
    })));
    fetch_calls(&log);
    cdp.absorb(pause("D1", "M1", "https://app.example/a", "Fetch"));
    cdp.absorb(hop("D2", "M1", "https://vault.sliccy.ai/", "D1"));
    cdp.absorb(pause("D3", "M3", "https://app.example/b", "Fetch"));
    cdp.absorb(pause("D4", "M4", "https://app.example/c", "Fetch"));
    let calls = fetch_calls(&log);
    assert_eq!(calls[0], armed(json!([{ "urlPattern": "*" }]), false));
    assert_eq!(calls[1].0, "Fetch.fulfillRequest");
    assert_eq!(calls[1].1["requestId"], "D1");
    assert_eq!(calls[2..], [failed("D2"), plain("D3"), plain("D4")]);
    assert_eq!(cdp.take_fetch_errors().len(), 2);
}

#[test]
fn the_shared_classifier_keeps_its_curlwright_contract() {
    let (cdp, _log) = wire(Box::new(answer));
    assert_eq!(cdp.ordinary_redirect_target("https://app.example/a", "b?c#d").unwrap(), "https://app.example/b?c#d");
    assert_eq!(cdp.ordinary_redirect_target("https://app.example/a", "https://seven.sliccy.ai/").unwrap_err(), format!("{}\n", cdp.guard.refusal()));
}

#[test]
fn fetch_commands_refuse_unclassifiable_redirect_locations() {
    let guard = Protected::default();
    let paused = Paused { request_id: "R".into(), url: "https://app.example/".into(), status: Some(200), ..Paused::default() };
    let continued = |status: Option<u16>, location: &str| crate::fetch::command(&guard, &paused, &crate::fetch::Decision::ContinueResponse(crate::fetch::ResponseOverrides { status, headers: Some(vec![("Location".into(), location.into())]), ..Default::default() }));
    assert!(continued(Some(302), "http://[::1").is_err());
    assert!(continued(Some(302), "/fine").is_ok());
    assert!(continued(None, "/fine").is_ok());
    let fulfilled = crate::fetch::command(&guard, &paused, &crate::fetch::Decision::Fulfill(crate::fetch::Fulfill::new(301).header("location", "/a\u{1}")));
    assert!(fulfilled.unwrap_err().starts_with("slicc: refused"));
    let rewrite = crate::fetch::command(&guard, &paused, &crate::fetch::Decision::Continue(crate::fetch::Overrides { url: Some("blob:https://app.example/x".into()), ..Default::default() }));
    assert!(rewrite.unwrap_err().starts_with("slicc: refused"));
}
