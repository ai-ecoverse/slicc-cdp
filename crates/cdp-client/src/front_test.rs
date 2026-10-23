struct FrontBrowser {
    targets: Vec<Value>,
    windows: std::collections::BTreeMap<String, Value>,
    front: String,
    sessions: std::collections::BTreeMap<String, String>,
    sent: Vec<Value>,
    refuse: bool,
}

struct FrontBackend {
    inbox: VecDeque<String>,
    browser: std::rc::Rc<std::cell::RefCell<FrontBrowser>>,
}

impl Transport for FrontBackend {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        let call: Value = serde_json::from_str(text).unwrap();
        let mut browser = self.browser.borrow_mut();
        browser.sent.push(call.clone());
        let params = &call["params"];
        let target = call["sessionId"].as_str().and_then(|session| browser.sessions.get(session).cloned()).unwrap_or_default();
        let result = match call["method"].as_str().unwrap() {
            "Target.attachToTarget" => {
                let id = params["targetId"].as_str().unwrap().to_string();
                let session = format!("{id}#{}", browser.sessions.len());
                browser.sessions.insert(session.clone(), id);
                json!({ "sessionId": session })
            }
            "Target.getTargetInfo" => json!({ "targetInfo": browser.targets.iter().find(|info| info["targetId"] == params["targetId"]).cloned().unwrap_or(Value::Null) }),
            "Target.getTargets" if browser.targets.is_empty() => json!({}),
            "Target.getTargets" => json!({ "targetInfos": browser.targets }),
            "Target.activateTarget" if browser.refuse => {
                self.inbox.push_back(json!({ "id": call["id"], "error": { "message": "refused" } }).to_string());
                return Ok(());
            }
            "Target.activateTarget" => {
                browser.front = params["targetId"].as_str().unwrap().to_string();
                json!({})
            }
            "Page.getFrameTree" => json!({ "frameTree": { "frame": { "id": target, "loaderId": "L", "url": "https://ordinary.test/", "securityOrigin": "https://ordinary.test" } } }),
            "Page.createIsolatedWorld" => json!({ "executionContextId": 9 }),
            "Runtime.evaluate" if params["expression"] == "document.visibilityState" => {
                json!({ "result": { "type": "string", "value": if browser.front == target { "visible" } else { "hidden" } } })
            }
            "Browser.getWindowForTarget" => match browser.windows.get(params["targetId"].as_str().unwrap_or("")) {
                Some(Value::Null) => {
                    self.inbox.push_back(json!({ "id": call["id"], "error": { "message": "No window" } }).to_string());
                    return Ok(());
                }
                Some(found) => found.clone(),
                None => json!({ "windowId": 1, "bounds": { "windowState": "normal" } }),
            },
            _ => json!({}),
        };
        self.inbox.push_back(json!({ "id": call["id"], "sessionId": call["sessionId"], "result": result }).to_string());
        Ok(())
    }

    fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
        Ok(self.inbox.pop_front())
    }
}

fn front_page(id: &str, url: &str) -> Value {
    json!({ "targetId": id, "type": "page", "url": url, "title": "", "browserContextId": "CTX" })
}

type FrontShared = std::rc::Rc<std::cell::RefCell<FrontBrowser>>;

fn front_client(extra: Vec<Value>, front: &str, windows: Vec<(&str, Value)>) -> (Cdp, String, FrontShared) {
    let mut targets = vec![front_page("T", "https://ordinary.test/")];
    targets.extend(extra);
    let browser = std::rc::Rc::new(std::cell::RefCell::new(FrontBrowser {
        targets,
        windows: windows.into_iter().map(|(id, value)| (id.to_string(), value)).collect(),
        front: front.to_string(),
        sessions: std::collections::BTreeMap::new(),
        sent: Vec::new(),
        refuse: false,
    }));
    let mut cdp = Cdp::from_transport(Box::new(FrontBackend { inbox: VecDeque::new(), browser: browser.clone() }));
    let session = super::attach(&mut cdp, "T").unwrap();
    browser.borrow_mut().sent.clear();
    (cdp, session, browser)
}

fn front_activated(browser: &FrontShared) -> Vec<String> {
    browser.borrow().sent.iter().filter(|call| call["method"] == "Target.activateTarget").map(|call| call["params"]["targetId"].as_str().unwrap().to_string()).collect()
}

fn front_window(id: i64, state: &str) -> Value {
    json!({ "windowId": id, "bounds": { "windowState": state } })
}

#[test]
fn bringing_a_tab_forward_restores_seven_only_when_seven_was_in_front() {
    let extra = vec![
        front_page("SEVEN", "https://seven.sliccy.ai/app"),
        front_page("BLANK", "about:blank"),
        front_page("ELSEWHERE", "https://vault.sliccy.ai/"),
        json!({ "targetId": "SW", "type": "service_worker", "url": "https://seven.sliccy.ai/sw.js", "browserContextId": "CTX" }),
        json!({ "targetId": "OMNIBOX", "type": "browser_ui", "url": "chrome://omnibox-popup.top-chrome/", "browserContextId": "CTX" }),
        json!({ "targetId": "PANEL", "type": "other", "url": "https://panel.test/", "browserContextId": "CTX" }),
        json!({ "targetId": "FRAME", "type": "iframe", "url": "https://vault.sliccy.ai/frame", "browserContextId": "CTX" }),
    ];
    let mut other = extra.clone();
    other[2]["browserContextId"] = json!("INCOGNITO");
    let (mut cdp, session, browser) = front_client(other, "SEVEN", vec![("PANEL", Value::Null), ("BLANK", front_window(1, "normal"))]);
    let front = cdp.bring_forward(&session).unwrap();
    assert_eq!(front_activated(&browser), ["T"]);
    assert_eq!(browser.borrow().front, "T");
    cdp.restore_front(front).unwrap();
    assert_eq!(front_activated(&browser), ["T", "SEVEN"]);
    assert_eq!(browser.borrow().front, "SEVEN");
}

#[test]
fn bringing_a_tab_alone_in_its_window_forward_switches_nothing_back() {
    let (mut cdp, session, browser) = front_client(vec![front_page("SEVEN", "https://seven.sliccy.ai/")], "NONE", vec![("SEVEN", front_window(3, "normal")), ("T", front_window(2, "normal"))]);
    assert!(cdp.bring_forward(&session).is_err());
    browser.borrow_mut().targets.remove(1);
    let front = cdp.bring_forward(&session).unwrap();
    cdp.restore_front(front).unwrap();
    assert_eq!(front_activated(&browser), ["T"]);
}

#[test]
fn bringing_a_tab_forward_restores_the_ordinary_tab_that_was_in_front() {
    let (mut cdp, session, browser) = front_client(vec![front_page("SEVEN", "https://seven.sliccy.ai/"), front_page("OTHER", "https://other.test/")], "OTHER", vec![]);
    let front = cdp.bring_forward(&session).unwrap();
    cdp.restore_front(front).unwrap();
    assert_eq!(front_activated(&browser), ["T", "OTHER"]);
}

#[test]
fn bringing_a_tab_forward_is_refused_when_the_front_tab_is_not_provably_seven_or_ordinary() {
    let seven = || front_page("SEVEN", "https://seven.sliccy.ai/");
    for (extra, front, windows) in [
        (vec![front_page("HIDDEN", "https://other.test/")], "NONE", vec![]),
        (vec![front_page("VAULT", "https://vault.sliccy.ai/")], "VAULT", vec![]),
        (vec![seven(), front_page("VAULT", "https://vault.sliccy.ai/")], "SEVEN", vec![]),
        (vec![seven()], "SEVEN", vec![("SEVEN", front_window(2, "normal"))]),
        (vec![seven()], "SEVEN", vec![("SEVEN", Value::Null)]),
        (vec![seven()], "SEVEN", vec![("T", front_window(1, "minimized"))]),
        (vec![seven()], "T", vec![]),
        (vec![front_page("SETTINGS", "chrome://settings/")], "SETTINGS", vec![]),
        (vec![seven()], "SEVEN", vec![("T", Value::Null)]),
        (vec![seven()], "SEVEN", vec![("T", json!({}))]),
        (vec![front_page("WWW", "https://www.sliccy.ai/")], "WWW", vec![]),
        (vec![front_page("PLAIN", "http://seven.sliccy.ai/")], "PLAIN", vec![]),
        (vec![front_page("PORT", "https://seven.sliccy.ai:8443/")], "PORT", vec![]),
        (vec![front_page("USER", "https://u@seven.sliccy.ai/")], "USER", vec![]),
        (vec![json!({ "targetId": "APP", "type": "other", "url": "https://seven.sliccy.ai/", "browserContextId": "CTX" })], "APP", vec![]),
        (vec![front_page("", "https://other.test/")], "NONE", vec![]),
    ] {
        let label = format!("{extra:?} {front} {windows:?}");
        let (mut cdp, session, browser) = front_client(extra, front, windows);
        let refused = cdp.bring_forward(&session).unwrap_err();
        assert!(refused.starts_with("slicc: refused:"), "{label}: {refused}");
        assert!(front_activated(&browser).is_empty(), "{label}");
    }
}

#[test]
fn bringing_a_tab_forward_is_refused_for_sessions_the_guard_does_not_admit() {
    let (mut cdp, session, browser) = front_client(vec![front_page("SEVEN", "https://seven.sliccy.ai/")], "SEVEN", vec![]);
    let guards: Vec<String> = cdp.guard_sessions.iter().cloned().collect();
    for session in guards.iter().map(String::as_str).chain(["NOBODY"]) {
        assert!(cdp.bring_forward(session).unwrap_err().starts_with("slicc: refused:"), "{session}");
    }
    cdp.blocked.insert(session.clone());
    assert!(cdp.bring_forward(&session).unwrap_err().starts_with("slicc: refused:"));
    cdp.blocked.remove(&session);
    cdp.protected_targets.insert("T".into());
    assert!(cdp.bring_forward(&session).unwrap_err().starts_with("slicc: refused:"));
    assert!(front_activated(&browser).is_empty());
}

#[test]
fn restoring_seven_is_refused_once_its_tab_changed() {
    for change in ["navigated", "moved", "closed", "own closed", "list", "activation"] {
        let (mut cdp, session, browser) = front_client(vec![front_page("SEVEN", "https://seven.sliccy.ai/")], "SEVEN", vec![]);
        let front = cdp.bring_forward(&session).unwrap();
        {
            let mut browser = browser.borrow_mut();
            match change {
                "navigated" => browser.targets[1]["url"] = json!("https://other.test/"),
                "moved" => { browser.windows.insert("SEVEN".into(), front_window(5, "normal")); }
                "closed" => { browser.targets.remove(1); }
                "own closed" => { browser.targets.remove(0); }
                "list" => browser.targets.clear(),
                _ => browser.refuse = true,
            }
        }
        assert_eq!(cdp.restore_front(front).unwrap_err(), super::FRONT_RESTORE, "{change}");
        let expected: &[&str] = if change == "activation" { &["T", "SEVEN"] } else { &["T"] };
        assert_eq!(front_activated(&browser), expected, "{change}");
    }
}

#[test]
fn restoring_an_ordinary_tab_is_refused_once_it_became_protected() {
    let (mut cdp, session, browser) = front_client(vec![front_page("OTHER", "https://other.test/")], "OTHER", vec![]);
    let front = cdp.bring_forward(&session).unwrap();
    browser.borrow_mut().targets[1]["url"] = json!("https://vault.sliccy.ai/");
    assert_eq!(cdp.restore_front(front).unwrap_err(), super::FRONT_RESTORE);
    assert_eq!(front_activated(&browser), ["T"]);
    let (mut cdp, _, browser) = front_client(vec![], "T", vec![]);
    cdp.restore_front(super::Front { own: "T".into(), window: 1, restore: None }).unwrap();
    assert!(front_activated(&browser).is_empty());
}

#[test]
fn seven_pages_are_only_seven_labels_on_https() {
    for url in ["https://seven.sliccy.ai/", "https://a-1.sliccy.ai/x?y#z", "https://seven.sliccy.ai:443/"] {
        assert!(super::seven_page(url), "{url}");
    }
    for url in ["", "nope", "https://sliccy.ai/", "https://.sliccy.ai/", "https://-a.sliccy.ai/", "https://a-.sliccy.ai/", "https://a.b.sliccy.ai/", "https://vault.sliccy.ai/", "https://vault-x.sliccy.ai/", "https://seven.sliccy.ai.evil/", "https://u:p@seven.sliccy.ai/", &format!("https://{}.sliccy.ai/", "a".repeat(64)), "https://SEVEN_.sliccy.ai/"] {
        assert!(!super::seven_page(url), "{url}");
    }
}
