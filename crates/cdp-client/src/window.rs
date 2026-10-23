const WINDOWLESS: &[&str] = &["service_worker", "shared_worker", "worker", "worklet", "auction_worklet", "shared_storage_worklet", "browser"];
const MAX_WINDOW: u64 = 10_000_000;

impl Cdp {
    fn admit_window(&mut self, params: &Value, session: Option<&str>) -> Result<(), String> {
        let session = session.ok_or_else(|| self.refusal())?;
        let object = params.as_object().ok_or_else(|| self.refusal())?;
        let bounds = params.get("bounds").and_then(Value::as_object).ok_or_else(|| self.refusal())?;
        let size = |key: &str| bounds.get(key).and_then(Value::as_u64).is_some_and(|value| (1..=MAX_WINDOW).contains(&value));
        if !object.keys().all(|key| matches!(key.as_str(), "windowId" | "bounds"))
            || !bounds.keys().all(|key| matches!(key.as_str(), "width" | "height"))
            || !size("width") || !size("height") { return Err(self.refusal()) }
        let window = params.get("windowId").and_then(Value::as_i64).ok_or_else(|| self.refusal())?;
        let target = self.exclusive_profile(session)?;
        let owned = self.raw_call("Browser.getWindowForTarget", json!({}), Some(session), DOCUMENT_CHECK).map_err(|_| self.refusal())?;
        if owned.get("windowId").and_then(Value::as_i64) != Some(window) { return Err(self.refusal()) }
        if self.blocked.contains(session) || self.session_target(session).as_deref() != Some(target.as_str()) { return Err(self.refusal()) }
        Ok(())
    }

    pub fn admit_foreground(&mut self, session: &str) -> Result<String, String> {
        self.exclusive_profile(session)
    }

    fn exclusive_profile(&mut self, session: &str) -> Result<String, String> {
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        if self.guard_sessions.contains(session) || self.blocked.contains(session) || self.protected_targets.contains(&target) {
            return Err(self.refusal());
        }
        self.document_state_for(&target, true, None).ok_or_else(|| self.refusal())?;
        let listed = self.raw_call("Target.getTargets", json!({}), None, DOCUMENT_CHECK)?;
        let infos = listed.get("targetInfos").and_then(Value::as_array).cloned().ok_or_else(|| self.refusal())?;
        let text = |info: &Value, key: &str| info.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let own = infos.iter().find(|info| text(info, "targetId") == target).ok_or_else(|| self.refusal())?;
        if text(own, "type") != "page" || self.info_protected(own, &infos) { return Err(self.refusal()) }
        let context = text(own, "browserContextId");
        for info in &infos {
            if text(info, "targetId") == target || WINDOWLESS.contains(&text(info, "type").as_str()) || top_chrome(info) { continue }
            let other = text(info, "browserContextId");
            if !context.is_empty() && !other.is_empty() && other != context { continue }
            if text(info, "targetId").is_empty() || url::Url::parse(&text(info, "url")).is_err() || self.protected_targets.contains(&text(info, "targetId")) || self.info_protected(info, &infos) {
                return Err(self.refusal());
            }
        }
        if self.blocked.contains(session) || self.session_target(session).as_deref() != Some(target.as_str()) { return Err(self.refusal()) }
        Ok(target)
    }
}

fn top_chrome(info: &Value) -> bool {
    info.get("type").and_then(Value::as_str) == Some("browser_ui")
        && info.get("url").and_then(Value::as_str).and_then(|text| url::Url::parse(text).ok())
            .is_some_and(|url| url.scheme() == "chrome" && url.host_str().is_some_and(|host| host.ends_with(".top-chrome")) && url.username().is_empty() && url.password().is_none())
}
