struct NestBrowser {
    infos: Vec<Value>,
    trees: std::collections::BTreeMap<String, Value>,
    sessions: std::collections::BTreeMap<String, String>,
    attaches: usize,
}

struct NestBackend {
    inbox: VecDeque<String>,
    browser: std::rc::Rc<std::cell::RefCell<NestBrowser>>,
}

#[derive(Clone, Copy)]
struct NestLevel {
    target: &'static str,
    kind: &'static str,
    frame: &'static str,
    parent: &'static str,
    url: &'static str,
}

fn nest_origin(url: &str) -> String {
    match url.find("://") {
        Some(at) => match url[at + 3..].find('/') {
            Some(end) => url[..at + 3 + end].to_string(),
            None => url.to_string(),
        },
        None => "://".into(),
    }
}

fn nest_context_id(browser: &NestBrowser, target: &str) -> usize {
    browser.infos.iter().position(|info| info["targetId"] == target).unwrap_or(0) + 20
}

impl Transport for NestBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let method = call["method"].as_str().unwrap();
        let session = call["sessionId"].as_str().unwrap_or("").to_string();
        let mut browser = self.browser.borrow_mut();
        let owner = browser.sessions.get(&session).cloned().unwrap_or_default();
        let result = match method {
            "Target.getTargets" => json!({ "targetInfos": browser.infos }),
            "Target.getTargetInfo" => {
                let wanted = call["params"]["targetId"].as_str().map(str::to_string).unwrap_or(owner.clone());
                match browser.infos.iter().find(|info| info["targetId"] == wanted.as_str()).cloned() {
                    Some(info) => json!({ "targetInfo": info }),
                    None => {
                        self.inbox.push_back(json!({ "id": call["id"], "sessionId": call.get("sessionId"), "error": { "message": "No target with given id" } }).to_string());
                        return Ok(());
                    }
                }
            }
            "Target.attachToTarget" => {
                let target = call["params"]["targetId"].as_str().unwrap_or("").to_string();
                browser.attaches += 1;
                let child = format!("S{}-{target}", browser.attaches);
                browser.sessions.insert(child.clone(), target.clone());
                let info = browser.infos.iter().find(|info| info["targetId"] == target.as_str()).cloned().unwrap_or(json!({}));
                self.inbox.push_back(json!({ "method": "Target.attachedToTarget", "params": { "sessionId": child, "targetInfo": info, "waitingForDebugger": false } }).to_string());
                json!({ "sessionId": child })
            }
            "Target.setAutoAttach" => {
                let root = browser.trees.get(&owner).and_then(|tree| tree["frameTree"]["frame"]["id"].as_str().map(str::to_string)).unwrap_or_default();
                let children: Vec<Value> = browser
                    .infos
                    .iter()
                    .filter(|info| info["type"] == "iframe" && info["parentFrameId"] == root.as_str())
                    .cloned()
                    .collect();
                for info in children {
                    browser.attaches += 1;
                    let target = info["targetId"].as_str().unwrap().to_string();
                    let child = format!("A{}-{target}", browser.attaches);
                    browser.sessions.insert(child.clone(), target);
                    self.inbox.push_back(json!({ "method": "Target.attachedToTarget", "sessionId": session, "params": { "sessionId": child, "targetInfo": info, "waitingForDebugger": false } }).to_string());
                }
                json!({})
            }
            "Target.detachFromTarget" => {
                if let Some(child) = call["params"]["sessionId"].as_str() {
                    browser.sessions.remove(child);
                }
                json!({})
            }
            "Page.getFrameTree" => browser.trees.get(&owner).cloned().unwrap_or_else(|| json!({ "frameTree": { "frame": { "id": owner, "url": "about:blank", "securityOrigin": "://", "loaderId": "L" } } })),
            "Page.getResourceTree" => {
                let tree = browser.trees.get(&owner).cloned().unwrap_or(json!({ "frameTree": { "frame": {} } }));
                json!({ "frameTree": { "frame": tree["frameTree"]["frame"], "resources": [] } })
            }
            "Runtime.enable" => {
                if let Some(frame) = browser.trees.get(&owner).map(|tree| tree["frameTree"]["frame"].clone()) {
                    let id = nest_context_id(&browser, &owner);
                    self.inbox.push_back(json!({
                        "method": "Runtime.executionContextCreated",
                        "sessionId": session,
                        "params": { "context": {
                            "id": id,
                            "uniqueId": format!("unique-{owner}"),
                            "origin": frame["securityOrigin"],
                            "name": "",
                            "auxData": { "frameId": frame["id"], "isDefault": true, "type": "default" }
                        } }
                    }).to_string());
                }
                json!({})
            }
            "Runtime.evaluate" => json!({ "result": { "type": "string", "value": "ok" } }),
            "DOM.getDocument" => json!({ "root": { "nodeId": 1, "backendNodeId": 1, "nodeName": "HTML" } }),
            "Page.captureScreenshot" => json!({ "data": "pixels" }),
            _ => json!({}),
        };
        self.inbox.push_back(json!({ "id": call["id"], "sessionId": call.get("sessionId"), "result": result }).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

const NEST_TOP: &str = "http://plain.guard.test/oopif-nest";
const NEST_MID: &str = "http://cross.other.test/oopif-mid";
const NEST_INNER: &str = "http://plain.guard.test/doc?n=inner";
const NEST_SEVEN: &str = "https://seven.sliccy.ai/os/";
const NEST_VAULT: &str = "https://vault.sliccy.ai/app/";

fn nest_level(target: &'static str, kind: &'static str, frame: &'static str, parent: &'static str, url: &'static str) -> NestLevel {
    NestLevel { target, kind, frame, parent, url }
}

fn nest_chain(top: &'static str, mid: &'static str, inner: &'static str) -> Vec<NestLevel> {
    vec![
        nest_level("TOP", "page", "TOP", "", top),
        nest_level("MID", "iframe", "MID", "TOP", mid),
        nest_level("INNER", "iframe", "INNER", "MID", inner),
    ]
}

fn nest_tree(level: &NestLevel) -> Value {
    let mut frame = json!({ "id": level.frame, "url": level.url, "securityOrigin": nest_origin(level.url), "loaderId": format!("L-{}", level.frame) });
    if !level.parent.is_empty() {
        frame["parentId"] = json!(level.parent);
    }
    json!({ "frameTree": { "frame": frame } })
}

fn nest_info(level: &NestLevel) -> Value {
    let mut info = json!({ "targetId": level.target, "type": level.kind, "url": level.url, "title": "", "attached": false, "browserContextId": "CTX" });
    if !level.parent.is_empty() {
        info["parentFrameId"] = json!(level.parent);
    }
    info
}

fn nest_browser(levels: &[NestLevel]) -> (Cdp, std::rc::Rc<std::cell::RefCell<NestBrowser>>) {
    let browser = std::rc::Rc::new(std::cell::RefCell::new(NestBrowser {
        infos: levels.iter().map(nest_info).collect(),
        trees: levels.iter().map(|level| (level.target.to_string(), nest_tree(level))).collect(),
        sessions: std::collections::BTreeMap::new(),
        attaches: 0,
    }));
    let cdp = Cdp::from_transport(Box::new(NestBackend { inbox: VecDeque::new(), browser: browser.clone() }));
    (cdp, browser)
}

fn nest_pump(cdp: &mut Cdp) {
    for _ in 0..8 {
        let _ = cdp.pump(Instant::now());
    }
}

fn nest_auto_attach(cdp: &mut Cdp, session: &str) {
    let _ = cdp.call("Target.setAutoAttach", json!({ "autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true }), Some(session));
    nest_pump(cdp);
}

fn nest_client_sessions(cdp: &Cdp, browser: &std::rc::Rc<std::cell::RefCell<NestBrowser>>, target: &str) -> Vec<String> {
    browser
        .borrow()
        .sessions
        .iter()
        .filter(|(session, owner)| owner.as_str() == target && !cdp.guard_sessions.contains(session.as_str()))
        .map(|(session, _)| session.clone())
        .collect()
}

fn nest_attach_all(cdp: &mut Cdp, browser: &std::rc::Rc<std::cell::RefCell<NestBrowser>>, levels: &[NestLevel]) {
    for level in levels {
        let attached = cdp.call("Target.attachToTarget", json!({ "targetId": level.target, "flatten": true }), None);
        nest_pump(cdp);
        if let Ok(attached) = attached {
            let session = attached["sessionId"].as_str().unwrap_or("").to_string();
            let _ = cdp.call("Page.enable", json!({}), Some(&session));
            nest_auto_attach(cdp, &session);
        }
    }
    for level in levels {
        for session in nest_client_sessions(cdp, browser, level.target) {
            let _ = cdp.call("Page.enable", json!({}), Some(&session));
            let _ = cdp.call("Runtime.enable", json!({}), Some(&session));
        }
    }
    nest_pump(cdp);
}

struct NestUse {
    sessions: usize,
    results: Vec<(String, Result<Value, String>)>,
}

fn nest_use(cdp: &mut Cdp, browser: &std::rc::Rc<std::cell::RefCell<NestBrowser>>, target: &str) -> NestUse {
    let context = nest_context_id(&browser.borrow(), target);
    let sessions = nest_client_sessions(cdp, browser, target);
    let mut results = Vec::new();
    for session in &sessions {
        results.push(("evaluate".into(), cdp.call("Runtime.evaluate", json!({ "expression": "location.href", "returnByValue": true, "contextId": context }), Some(session))));
        results.push(("unique".into(), cdp.call("Runtime.evaluate", json!({ "expression": "location.href", "returnByValue": true, "uniqueContextId": format!("unique-{target}") }), Some(session))));
        results.push(("plain".into(), cdp.call("Runtime.evaluate", json!({ "expression": "location.href", "returnByValue": true }), Some(session))));
        results.push(("screenshot".into(), cdp.call("Page.captureScreenshot", json!({}), Some(session))));
        results.push(("input".into(), cdp.call("Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": 1, "y": 1 }), Some(session))));
        results.push(("dom".into(), cdp.call("DOM.getDocument", json!({}), Some(session))));
    }
    NestUse { sessions: sessions.len(), results }
}

fn nest_all_refused(used: &NestUse, label: &str) {
    for (what, result) in &used.results {
        assert!(result.as_ref().is_err_and(|error| error.starts_with("slicc: refused:")), "{label}: {what} must refuse: {result:?}");
    }
}

fn nest_live_refused(used: &NestUse, label: &str) {
    assert!(used.sessions > 0, "{label}: the proof must be exercised on a live client session");
    nest_all_refused(used, label);
}

fn nest_set(browser: &std::rc::Rc<std::cell::RefCell<NestBrowser>>, level: NestLevel) {
    let mut browser = browser.borrow_mut();
    browser.trees.insert(level.target.into(), nest_tree(&level));
    let info = nest_info(&level);
    if let Some(slot) = browser.infos.iter_mut().find(|info| info["targetId"] == level.target) {
        *slot = info;
    }
}

fn nest_ordinary_attached() -> (Cdp, std::rc::Rc<std::cell::RefCell<NestBrowser>>, Vec<NestLevel>) {
    let levels = nest_chain(NEST_TOP, NEST_MID, NEST_INNER);
    let (mut cdp, browser) = nest_browser(&levels);
    nest_attach_all(&mut cdp, &browser, &levels);
    let before = nest_use(&mut cdp, &browser, "INNER");
    nest_all_admitted(&before, "ordinary chain before the change");
    (cdp, browser, levels)
}

fn nest_all_admitted(used: &NestUse, label: &str) {
    assert!(used.sessions > 0, "{label}: no client session reached the target");
    for (what, result) in &used.results {
        assert!(result.is_ok(), "{label}: {what} must work: {result:?}");
    }
}

#[test]
fn ordinary_nested_oopif_under_ordinary_page_stays_drivable() {
    let levels = nest_chain(NEST_TOP, NEST_MID, NEST_INNER);
    let (mut cdp, browser) = nest_browser(&levels);
    nest_attach_all(&mut cdp, &browser, &levels);
    let inner = nest_use(&mut cdp, &browser, "INNER");
    nest_all_admitted(&inner, "ordinary grandchild");
    let mid = nest_use(&mut cdp, &browser, "MID");
    nest_all_admitted(&mid, "ordinary mid");
}

#[test]
fn ordinary_nested_oopif_with_distinct_frame_ids_stays_drivable() {
    let levels = vec![
        nest_level("TOP", "page", "TOP", "", NEST_TOP),
        nest_level("MID", "iframe", "MIDF", "TOP", NEST_MID),
        nest_level("INNER", "iframe", "INNERF", "MIDF", NEST_INNER),
    ];
    let (mut cdp, browser) = nest_browser(&levels);
    nest_attach_all(&mut cdp, &browser, &levels);
    let inner = nest_use(&mut cdp, &browser, "INNER");
    nest_all_admitted(&inner, "distinct-id grandchild");
}

#[test]
fn nested_oopif_ordinary_three_level_oopif_chain_stays_drivable() {
    let levels = vec![
        nest_level("TOP", "page", "TOP", "", NEST_TOP),
        nest_level("MID", "iframe", "MID", "TOP", NEST_MID),
        nest_level("DEEP", "iframe", "DEEP", "MID", "http://deep.other.test/deep"),
        nest_level("INNER", "iframe", "INNER", "DEEP", NEST_INNER),
    ];
    let (mut cdp, browser) = nest_browser(&levels);
    nest_attach_all(&mut cdp, &browser, &levels);
    let inner = nest_use(&mut cdp, &browser, "INNER");
    nest_all_admitted(&inner, "three-level grandchild");
}

#[test]
fn nested_oopif_protected_mid_keeps_ordinary_grandchild_refused() {
    for mid in [NEST_SEVEN, NEST_VAULT] {
        let levels = nest_chain(NEST_TOP, mid, NEST_INNER);
        let (mut cdp, browser) = nest_browser(&levels);
        nest_attach_all(&mut cdp, &browser, &levels);
        let inner = nest_use(&mut cdp, &browser, "INNER");
        nest_all_refused(&inner, mid);
    }
}

#[test]
fn nested_oopif_protected_grandchild_under_ordinary_mid_stays_refused() {
    for inner_url in [NEST_SEVEN, NEST_VAULT] {
        let levels = nest_chain(NEST_TOP, NEST_MID, inner_url);
        let (mut cdp, browser) = nest_browser(&levels);
        nest_attach_all(&mut cdp, &browser, &levels);
        let inner = nest_use(&mut cdp, &browser, "INNER");
        nest_all_refused(&inner, inner_url);
    }
}

#[test]
fn nested_oopif_protected_top_keeps_ordinary_nested_chain_refused() {
    for top in [NEST_SEVEN, NEST_VAULT] {
        let levels = nest_chain(top, NEST_MID, NEST_INNER);
        let (mut cdp, browser) = nest_browser(&levels);
        nest_attach_all(&mut cdp, &browser, &levels);
        for target in ["MID", "INNER"] {
            let used = nest_use(&mut cdp, &browser, target);
            nest_all_refused(&used, top);
        }
    }
}

#[test]
fn nested_oopif_unguarded_mid_leaves_grandchild_unverified() {
    let levels = vec![
        nest_level("TOP", "page", "TOP", "", NEST_TOP),
        nest_level("MID", "iframe", "MIDF", "TOP", NEST_MID),
        nest_level("INNER", "iframe", "INNER", "MIDF", NEST_INNER),
    ];
    let (mut cdp, browser) = nest_browser(&levels);
    nest_attach_all(&mut cdp, &browser, &[levels[0], levels[2]]);
    let attached = cdp.call("Target.attachToTarget", json!({ "targetId": "INNER", "flatten": true }), None);
    nest_pump(&mut cdp);
    browser.borrow_mut().sessions.insert("LIVE-INNER".into(), "INNER".into());
    cdp.absorb(json!({ "method": "Target.attachedToTarget", "params": { "sessionId": "LIVE-INNER", "targetInfo": nest_info(&levels[2]), "waitingForDebugger": false } }));
    let _ = cdp.call("Runtime.enable", json!({}), Some("LIVE-INNER"));
    nest_pump(&mut cdp);
    assert!(!cdp.guards.contains_key("MID"), "mid must stay unguarded for this shape");
    cdp.page_scope_owners.insert("MIDF".into(), "TOP".into());
    let inner = nest_use(&mut cdp, &browser, "INNER");
    assert!(inner.sessions > 0 || attached.is_err());
    nest_all_refused(&inner, "unbound mid frame");
}

#[test]
fn nested_oopif_mid_guard_lost_with_stale_page_scope_owner_refuses() {
    let (mut cdp, browser, _) = nest_ordinary_attached();
    cdp.guards.remove("MID");
    cdp.page_scope_owners.insert("MID".into(), "TOP".into());
    let inner = nest_use(&mut cdp, &browser, "INNER");
    nest_live_refused(&inner, "mid without its own guard");
}

#[test]
fn nested_oopif_mid_root_that_does_not_match_the_parent_frame_refuses() {
    let (mut cdp, browser, _) = nest_ordinary_attached();
    browser.borrow_mut().trees.insert("MID".into(), nest_tree(&nest_level("MID", "iframe", "OTHER", "TOP", NEST_MID)));
    let inner = nest_use(&mut cdp, &browser, "INNER");
    nest_live_refused(&inner, "mid root moved");
}

#[test]
fn nested_oopif_mid_root_without_parent_refuses() {
    let (mut cdp, browser, _) = nest_ordinary_attached();
    browser.borrow_mut().trees.insert("MID".into(), nest_tree(&nest_level("MID", "iframe", "MID", "", NEST_MID)));
    let inner = nest_use(&mut cdp, &browser, "INNER");
    nest_live_refused(&inner, "mid root claims to be a top frame");
}

#[test]
fn nested_oopif_cyclic_iframe_ancestry_refuses() {
    let levels = vec![
        nest_level("TOP", "page", "TOP", "", NEST_TOP),
        nest_level("A", "iframe", "A", "B", NEST_MID),
        nest_level("B", "iframe", "B", "A", "http://loop.other.test/b"),
        nest_level("INNER", "iframe", "INNER", "A", NEST_INNER),
    ];
    let (mut cdp, browser) = nest_browser(&levels);
    nest_attach_all(&mut cdp, &browser, &levels);
    for target in ["INNER", "A", "B"] {
        let used = nest_use(&mut cdp, &browser, target);
        nest_all_refused(&used, target);
    }
    let (mut cdp, browser, _) = nest_ordinary_attached();
    nest_set(&browser, nest_level("MID", "iframe", "MID", "INNER", NEST_MID));
    for target in ["INNER", "MID"] {
        let used = nest_use(&mut cdp, &browser, target);
        nest_live_refused(&used, &format!("late cycle at {target}"));
    }
}

#[test]
fn nested_oopif_iframe_chain_deeper_than_the_bound_refuses() {
    const IDS: [&str; 10] = ["L1", "L2", "L3", "L4", "L5", "L6", "L7", "L8", "L9", "L10"];
    let mut levels = vec![nest_level("TOP", "page", "TOP", "", NEST_TOP)];
    let mut parent = "TOP";
    for id in IDS {
        levels.push(nest_level(id, "iframe", id, parent, NEST_MID));
        parent = id;
    }
    let (mut cdp, browser) = nest_browser(&levels);
    nest_attach_all(&mut cdp, &browser, &levels);
    let within = nest_use(&mut cdp, &browser, "L7");
    nest_all_admitted(&within, "depth within bound");
    for target in ["L8", "L10"] {
        let beyond = nest_use(&mut cdp, &browser, target);
        nest_live_refused(&beyond, &format!("depth beyond bound at {target}"));
    }
}

#[test]
fn nested_oopif_level_that_turns_protected_after_the_proof_voids_it() {
    for (target, kind, parent) in [("TOP", "page", ""), ("MID", "iframe", "TOP"), ("INNER", "iframe", "MID")] {
        for url in [NEST_SEVEN, NEST_VAULT, "chrome-extension://abcdefghijklmnopabcdefghijklmnop/frame.html"] {
            if target == "TOP" && url.starts_with("chrome-extension:") {
                continue;
            }
            let (mut cdp, browser, _) = nest_ordinary_attached();
            nest_set(&browser, nest_level(target, kind, target, parent, url));
            let silent = nest_use(&mut cdp, &browser, "INNER");
            nest_live_refused(&silent, &format!("{target} silently became {url}"));
        }
    }
}

#[test]
fn nested_oopif_mid_navigation_to_seven_cuts_the_grandchild() {
    let (mut cdp, browser, _) = nest_ordinary_attached();
    let seven = nest_level("MID", "iframe", "MID", "TOP", NEST_SEVEN);
    nest_set(&browser, seven);
    let mid_guard = cdp.guards.get("MID").cloned().unwrap();
    cdp.absorb(json!({ "method": "Target.targetInfoChanged", "params": { "targetInfo": nest_info(&seven) } }));
    cdp.absorb(json!({ "method": "Page.frameNavigated", "sessionId": mid_guard, "params": { "frame": nest_tree(&seven)["frameTree"]["frame"], "type": "Navigation" } }));
    nest_pump(&mut cdp);
    let announced = nest_use(&mut cdp, &browser, "INNER");
    nest_all_refused(&announced, "announced late protect");
    nest_set(&browser, nest_level("MID", "iframe", "MID", "TOP", NEST_MID));
    let back = nest_use(&mut cdp, &browser, "INNER");
    nest_all_refused(&back, "a cut grandchild stays cut when the mid looks ordinary again");
}
