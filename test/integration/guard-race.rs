use cdp_client::cdp::{self as client, Cdp};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn live(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

fn attached(cdp: &Cdp) -> Vec<(String, String)> {
    cdp.events()
        .iter()
        .filter(|event| event["method"] == json!("Target.attachedToTarget"))
        .map(|event| {
            (
                event
                    .pointer("/params/targetInfo/targetId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                event
                    .pointer("/params/sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        })
        .collect()
}

fn pump_for(cdp: &mut Cdp, span: Duration) {
    let until = Instant::now() + span;
    while Instant::now() < until {
        let _ = cdp.pump(Instant::now() + Duration::from_millis(100));
    }
}

fn refused(result: Result<Value, String>, what: &str) {
    match result {
        Ok(value) => panic!("{what} was allowed: {value}"),
        Err(message) => assert!(message.starts_with("slicc: refused: "), "{what}: {message}"),
    }
}

fn page_ids(cdp: &mut Cdp) -> Vec<String> {
    cdp.call("Target.getTargets", json!({}), None).unwrap()["targetInfos"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|info| info["type"] == json!("page"))
        .filter_map(|info| info["targetId"].as_str().map(str::to_string))
        .collect()
}

fn timed_out_guard(ws: &str) -> (Cdp, String) {
    let mut cdp = Cdp::connect(ws).unwrap();
    let before = page_ids(&mut cdp);
    let created = cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None);
    refused(created, "createTarget whose guard attach timed out");
    let mut abandoned: Vec<String> = Vec::new();
    let until = Instant::now() + Duration::from_secs(5);
    while abandoned.is_empty() && Instant::now() < until {
        abandoned = page_ids(&mut cdp)
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        let _ = cdp.pump(Instant::now() + Duration::from_millis(100));
    }
    pump_for(&mut cdp, Duration::from_secs(3));
    (cdp, abandoned.into_iter().next().unwrap_or_default())
}

#[test]
#[ignore = "needs SLICC_RACE_PASS_WS from test/integration/guard-attach-race.test.mjs"]
fn live_a_guarded_create_or_attach_surfaces_only_the_callers_session() {
    let mut cdp = Cdp::connect(&live("SLICC_RACE_PASS_WS")).unwrap();
    let target = cdp
        .call("Target.createTarget", json!({ "url": "about:blank" }), None)
        .unwrap()["targetId"]
        .as_str()
        .unwrap()
        .to_string();
    pump_for(&mut cdp, Duration::from_millis(500));
    assert_eq!(
        attached(&cdp),
        Vec::new(),
        "createTarget surfaced an attach"
    );
    let session = client::attach(&mut cdp, &target).unwrap();
    pump_for(&mut cdp, Duration::from_millis(500));
    assert_eq!(attached(&cdp), vec![(target.clone(), session)]);
    cdp.call("Target.closeTarget", json!({ "targetId": target }), None)
        .unwrap();
}

#[test]
#[ignore = "needs SLICC_RACE_DELAY_WS from test/integration/guard-attach-race.test.mjs"]
fn live_b_a_timed_out_guard_attach_leaves_no_session_and_its_target_refused() {
    let (mut cdp, target) = timed_out_guard(&live("SLICC_RACE_DELAY_WS"));
    assert_eq!(
        attached(&cdp),
        Vec::new(),
        "the late guard attach reached the caller"
    );
    if !target.is_empty() {
        refused(
            client::attach(&mut cdp, &target).map(Value::String),
            "attach to the abandoned target",
        );
    }
}

#[test]
#[ignore = "needs SLICC_RACE_NOCLOSE_WS from test/integration/guard-attach-race.test.mjs"]
fn live_c_a_timed_out_guard_attach_with_a_failed_close_stays_refused() {
    let (mut cdp, target) = timed_out_guard(&live("SLICC_RACE_NOCLOSE_WS"));
    assert!(
        !target.is_empty(),
        "closeTarget was forced to fail, so the target must still exist"
    );
    let leaked = attached(&cdp);
    for (_, session) in &leaked {
        refused(
            cdp.call_for(
                "Runtime.evaluate",
                json!({ "expression": "location.href", "returnByValue": true }),
                Some(session),
                Duration::from_secs(5),
            ),
            "Runtime.evaluate on a surfaced guard session",
        );
    }
    assert_eq!(
        leaked,
        Vec::new(),
        "the late guard attach reached the caller"
    );
    refused(
        client::attach(&mut cdp, &target).map(Value::String),
        "attach to the unguarded target",
    );
    refused(
        cdp.call("Target.activateTarget", json!({ "targetId": target }), None),
        "activateTarget on the unguarded target",
    );
}
