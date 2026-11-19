const WINDOWLESS: &[&str] = &["service_worker", "shared_worker", "worker", "worklet", "auction_worklet", "shared_storage_worklet", "browser"];
const MAX_WINDOW: u64 = 10_000_000;
pub const WINDOW_SHAPE: &str = "slicc: refused: Browser.setWindowBounds takes only a windowId and a whole-number width and height\n";
pub const WINDOW_OWN: &str = "slicc: refused: Browser.setWindowBounds resizes only the window of the session's own tab\n";

impl Cdp {
    fn admit_window(&mut self, params: &Value, session: Option<&str>) -> Result<(), String> {
        let object = params.as_object().ok_or(WINDOW_SHAPE)?;
        let bounds = params.get("bounds").and_then(Value::as_object).ok_or(WINDOW_SHAPE)?;
        let size = |key: &str| bounds.get(key).and_then(Value::as_u64).is_some_and(|value| (1..=MAX_WINDOW).contains(&value));
        if !object.keys().all(|key| matches!(key.as_str(), "windowId" | "bounds"))
            || !bounds.keys().all(|key| matches!(key.as_str(), "width" | "height"))
            || !size("width") || !size("height") { return Err(WINDOW_SHAPE.into()) }
        let window = params.get("windowId").and_then(Value::as_i64).ok_or(WINDOW_SHAPE)?;
        let session = session.ok_or(WINDOW_OWN)?;
        let target = self.window_session(session)?;
        let owned = self.raw_call("Browser.getWindowForTarget", json!({}), Some(session), DOCUMENT_CHECK).map_err(|_| WINDOW_OWN.to_string())?;
        if owned.get("windowId").and_then(Value::as_i64) != Some(window) { return Err(WINDOW_OWN.into()) }
        if self.window_session(session)? != target { return Err(self.refusal()) }
        Ok(())
    }

    fn window_session(&self, session: &str) -> Result<String, String> {
        if self.guard_sessions.contains(session) { return Err(WINDOW_OWN.into()) }
        let target = self.session_target(session).ok_or(WINDOW_OWN)?;
        if self.blocked.contains(session) || self.protected_targets.contains(&target) { return Err(self.refusal()) }
        Ok(target)
    }
}

fn top_chrome(info: &Value) -> bool {
    info.get("type").and_then(Value::as_str) == Some("browser_ui")
        && info.get("url").and_then(Value::as_str).and_then(|text| url::Url::parse(text).ok())
            .is_some_and(|url| url.scheme() == "chrome" && url.host_str().is_some_and(|host| host.ends_with(".top-chrome")) && url.username().is_empty() && url.password().is_none())
}
