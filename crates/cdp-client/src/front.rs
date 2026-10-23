const FRONT_SKIP: &[&str] = &["service_worker", "shared_worker", "worker", "worklet", "auction_worklet", "shared_storage_worklet", "browser", "iframe"];
pub const FRONT_RESTORE: &str = "Error: the input landed, but the tab that was in front could not be brought back to the front.\n";

#[derive(Debug, Clone, PartialEq)]
pub struct Front {
    own: String,
    window: i64,
    restore: Option<(String, bool)>,
}

impl Cdp {
    pub fn bring_forward(&mut self, session: &str) -> Result<Front, String> {
        let own = self.front_own(session)?;
        let infos = self.front_infos()?;
        let text = |info: &Value, key: &str| info.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let mine = infos.iter().find(|info| text(info, "targetId") == own).cloned().ok_or_else(|| self.refusal())?;
        if text(&mine, "type") != "page" || self.info_protected(&mine, &infos) { return Err(self.refusal()) }
        let context = text(&mine, "browserContextId");
        let window = self.front_window(&own)?;
        if self.front_visible(&own) != Some(false) { return Err(self.refusal()) }
        let mut visible = Vec::new();
        let mut opaque = Vec::new();
        for info in &infos {
            let id = text(info, "targetId");
            let kind = text(info, "type");
            if id == own || FRONT_SKIP.contains(&kind.as_str()) || top_chrome_target(info) { continue }
            let other = text(info, "browserContextId");
            if !context.is_empty() && !other.is_empty() && other != context { continue }
            let open = !id.is_empty() && url::Url::parse(&text(info, "url")).is_ok() && !self.protected_targets.contains(&id) && !self.info_protected(info, &infos);
            let same = match self.front_window(&id) {
                Ok(found) => found == window,
                Err(_) if open => continue,
                Err(_) => return Err(self.refusal()),
            };
            if !open && !same { return Err(self.refusal()) }
            if !same { continue }
            match (open && kind == "page").then(|| self.front_visible(&id)).flatten() {
                Some(true) => visible.push(id),
                Some(false) => {}
                None => opaque.push(info.clone()),
            }
        }
        let restore = match (visible.as_slice(), opaque.as_slice()) {
            ([front], _) => (front.clone(), false),
            ([], [seven]) if text(seven, "type") == "page" && seven_page(&text(seven, "url")) => (text(seven, "targetId"), true),
            _ => return Err(self.refusal()),
        };
        self.front_own(session).ok().filter(|again| again == &own).ok_or_else(|| self.refusal())?;
        self.raw_call("Target.activateTarget", json!({ "targetId": own }), None, DOCUMENT_CHECK)?;
        Ok(Front { own, window, restore: Some(restore) })
    }

    pub fn restore_front(&mut self, front: Front) -> Result<(), String> {
        let Some((target, seven)) = front.restore else { return Ok(()) };
        let infos = self.front_infos().map_err(|_| FRONT_RESTORE.to_string())?;
        let text = |info: &Value, key: &str| info.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let info = infos.iter().find(|info| text(info, "targetId") == target).cloned().ok_or(FRONT_RESTORE.to_string())?;
        let allowed = if seven {
            text(&info, "type") == "page" && seven_page(&text(&info, "url"))
        } else {
            !self.protected_targets.contains(&target) && !self.info_protected(&info, &infos)
        };
        if !allowed || !infos.iter().any(|info| text(info, "targetId") == front.own) || self.front_window(&target).ok() != Some(front.window) {
            return Err(FRONT_RESTORE.to_string());
        }
        self.raw_call("Target.activateTarget", json!({ "targetId": target }), None, DOCUMENT_CHECK).map_err(|_| FRONT_RESTORE.to_string())?;
        Ok(())
    }

    fn front_own(&mut self, session: &str) -> Result<String, String> {
        let own = self.session_target(session).ok_or_else(|| self.refusal())?;
        if self.guard_sessions.contains(session) || self.blocked.contains(session) || self.protected_targets.contains(&own) {
            return Err(self.refusal());
        }
        self.document_state_for(&own, true, None).ok_or_else(|| self.refusal())?;
        if self.blocked.contains(session) { return Err(self.refusal()) }
        Ok(own)
    }

    fn front_infos(&mut self) -> Result<Vec<Value>, String> {
        let listed = self.raw_call("Target.getTargets", json!({}), None, DOCUMENT_CHECK)?;
        listed.get("targetInfos").and_then(Value::as_array).cloned().ok_or_else(|| self.refusal())
    }

    fn front_window(&mut self, target: &str) -> Result<i64, String> {
        let found = self.raw_call("Browser.getWindowForTarget", json!({ "targetId": target }), None, DOCUMENT_CHECK)?;
        let window = found.get("windowId").and_then(Value::as_i64).ok_or_else(|| self.refusal())?;
        if found.pointer("/bounds/windowState").and_then(Value::as_str) == Some("minimized") { return Err(self.refusal()) }
        Ok(window)
    }

    fn front_visible(&mut self, target: &str) -> Option<bool> {
        let attached = self.raw_call("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None, DOCUMENT_CHECK).ok()?;
        let session = attached.get("sessionId").and_then(Value::as_str).filter(|session| !session.is_empty())?.to_string();
        self.guard_sessions.insert(session.clone());
        let state = (|| {
            let world = self.raw_call("Page.createIsolatedWorld", json!({ "frameId": target, "worldName": "slicc-front" }), Some(&session), DOCUMENT_CHECK).ok()?;
            let context = world.get("executionContextId").cloned()?;
            let read = self.raw_call("Runtime.evaluate", json!({ "expression": "document.visibilityState", "contextId": context, "returnByValue": true }), Some(&session), DOCUMENT_CHECK).ok()?;
            read.pointer("/result/value").and_then(Value::as_str).map(|state| state == "visible")
        })();
        let _ = self.raw_call("Target.detachFromTarget", json!({ "sessionId": session }), None, DOCUMENT_CHECK);
        self.guard_sessions.remove(&session);
        state
    }
}

fn top_chrome_target(info: &Value) -> bool {
    info.get("type").and_then(Value::as_str) == Some("browser_ui")
        && info.get("url").and_then(Value::as_str).and_then(|text| url::Url::parse(text).ok())
            .is_some_and(|url| url.scheme() == "chrome" && url.host_str().is_some_and(|host| host.ends_with(".top-chrome")) && url.username().is_empty() && url.password().is_none())
}

fn seven_rules() -> &'static slicc_browsers::Seven {
    static SEVEN: std::sync::OnceLock<slicc_browsers::Seven> = std::sync::OnceLock::new();
    SEVEN.get_or_init(|| slicc_browsers::catalog().seven)
}

pub fn seven_page(text: &str) -> bool {
    let Ok(url) = url::Url::parse(text) else { return false };
    let rules = seven_rules();
    if url.scheme() != rules.origin_scheme || !url.username().is_empty() || url.password().is_some() || url.port().is_some() { return false }
    let Some(label) = url.host_str().and_then(|host| host.strip_suffix(".sliccy.ai")) else { return false };
    let bytes = label.as_bytes();
    !bytes.is_empty() && bytes.len() <= 63
        && bytes.iter().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && bytes[0] != b'-' && bytes[bytes.len() - 1] != b'-'
        && rules.is_seven_label(label)
}
