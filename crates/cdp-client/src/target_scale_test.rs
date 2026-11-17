fn hundreds_of_targets() -> Vec<Value> {
    let mut infos = Vec::new();
    for page in 0..60 {
        infos.push(json!({ "targetId": format!("PAGE{page}"), "type": "page", "url": format!("https://site{page}.test/path?q={page}"), "title": format!("site {page}"), "attached": false, "browserContextId": "CTX" }));
    }
    for frame in 0..600 {
        infos.push(json!({ "targetId": format!("FRAME{frame}"), "type": "iframe", "url": format!("https://ads{frame}.test/frame?id={frame}"), "title": "", "attached": false, "browserContextId": "CTX", "parentFrameId": format!("PAGE{}", frame % 60) }));
    }
    for vault in 0..8 {
        infos.push(json!({ "targetId": format!("VAULT{vault}"), "type": "iframe", "url": format!("https://vault.sliccy.ai/app/{vault}"), "title": "vault", "attached": false, "browserContextId": "CTX", "parentFrameId": format!("PAGE{vault}") }));
        infos.push(json!({ "targetId": format!("INNER{vault}"), "type": "iframe", "url": format!("https://inner{vault}.test/"), "title": "", "attached": false, "browserContextId": "CTX", "parentFrameId": format!("VAULT{vault}") }));
        infos.push(json!({ "targetId": format!("VWORKER{vault}"), "type": "worker", "url": format!("https://inner{vault}.test/worker.js"), "title": "", "attached": false, "browserContextId": "CTX", "openerFrameId": format!("VAULT{vault}") }));
    }
    for worker in 0..40 {
        infos.push(json!({ "targetId": format!("SW{worker}"), "type": "service_worker", "url": format!("https://site{worker}.test/sw.js"), "title": "", "attached": false, "browserContextId": "CTX" }));
    }
    infos
}

fn redacted_in_discovery(id: &str) -> bool {
    id.starts_with("VAULT")
}

fn hidden_from_discovery(id: &str) -> bool {
    id.starts_with("INNER") || id.starts_with("VWORKER")
}

fn refused_lookup(id: &str) -> bool {
    id.starts_with("INNER")
}

#[test]
fn guard_keeps_up_with_hundreds_of_targets() {
    let infos = hundreds_of_targets();
    assert_eq!(infos.len(), 724);
    let browser = Rc::new(RefCell::new(AncestorBrowser {
        infos: infos.clone(),
        trees: std::collections::BTreeMap::new(),
        sessions: std::collections::BTreeMap::new(),
        attaches: 0,
        detach_failures: 0,
    }));
    let mut cdp = Cdp::from_transport(Box::new(AncestorBackend { inbox: VecDeque::new(), browser }));

    let started = Instant::now();
    for info in &infos {
        cdp.absorb(json!({ "method": "Target.targetCreated", "params": { "targetInfo": info } }));
        cdp.absorb(json!({ "method": "Target.targetInfoChanged", "params": { "targetInfo": info } }));
    }
    let discovery = started.elapsed();
    let started = Instant::now();
    let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
    let listing = started.elapsed();
    let started = Instant::now();
    let answers: Vec<(String, Result<Value, String>)> = infos
        .iter()
        .map(|info| {
            let id = info["targetId"].as_str().unwrap().to_string();
            let answer = cdp.call("Target.getTargetInfo", json!({ "targetId": id }), None);
            (id, answer)
        })
        .collect();
    let lookups = started.elapsed();
    eprintln!("{} targets: discovery {discovery:?}, getTargets {listing:?}, getTargetInfo {lookups:?}", infos.len());
    assert!(discovery < Duration::from_secs(4), "absorbing {} discovery events took {discovery:?}", infos.len() * 2);
    assert!(listing < Duration::from_secs(2), "Target.getTargets over {} targets took {listing:?}", infos.len());
    assert!(lookups < Duration::from_secs(2), "{} Target.getTargetInfo calls took {lookups:?}", infos.len());

    let shown: std::collections::BTreeSet<String> = listed["targetInfos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|info| info["targetId"].as_str().unwrap().to_string())
        .collect();
    let expected: std::collections::BTreeSet<String> = infos
        .iter()
        .map(|info| info["targetId"].as_str().unwrap().to_string())
        .filter(|id| !hidden_from_discovery(id))
        .collect();
    assert_eq!(expected.len(), 708);
    assert!(shown == expected, "listed {} of {} targets", shown.len(), expected.len());
    for info in listed["targetInfos"].as_array().unwrap() {
        let id = info["targetId"].as_str().unwrap();
        let original = infos.iter().find(|item| item["targetId"] == id).unwrap();
        assert_eq!(info["url"] == original["url"], !redacted_in_discovery(id), "{id}: {}", info["url"]);
    }
    for (id, answer) in &answers {
        if refused_lookup(id) {
            assert!(answer.is_err(), "{id} was answered: {answer:?}");
            continue;
        }
        let answer = answer.as_ref().unwrap_or_else(|error| panic!("{id} was refused: {error}"));
        let original = infos.iter().find(|item| item["targetId"] == id.as_str()).unwrap();
        let redacted = redacted_in_discovery(id) || hidden_from_discovery(id);
        assert_eq!(answer["targetInfo"]["targetId"], id.as_str());
        assert_eq!(answer["targetInfo"]["url"] == original["url"], !redacted, "{id}: {}", answer["targetInfo"]["url"]);
    }
    for vault in 0..8 {
        for id in [format!("VAULT{vault}"), format!("INNER{vault}"), format!("VWORKER{vault}")] {
            assert!(cdp.is_protected_target(&id), "{id}");
        }
    }
    let events = serde_json::to_string(cdp.events()).unwrap();
    assert!(!events.contains("vault.sliccy.ai/app"));
    assert!(!events.contains("/worker.js"));
    assert!(!events.contains("\"INNER"));
}
