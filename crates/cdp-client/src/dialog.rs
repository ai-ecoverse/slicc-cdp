#[derive(Clone, PartialEq)]
struct Dialog {
    url: String,
    frame: String,
    epoch: u64,
}

pub const DIALOG_UNOBSERVED: &str = "Error: No dialog visible to this session. Dialogs already open before attachment require the foreground dialog holder (follow-up #529); arm dialog-accept or dialog-dismiss before triggering a dialog.\n";

impl Cdp {
    pub fn dialog_targets(&mut self) -> Result<Vec<String>, String> {
        let result = self.raw_call("Target.getTargets", json!({}), None, DOCUMENT_CHECK)?;
        let infos = result.get("targetInfos").and_then(Value::as_array).ok_or_else(|| self.refusal())?;
        Ok(infos.iter().filter(|info| {
            info.get("type").and_then(Value::as_str) == Some("page")
                && info.get("url").and_then(Value::as_str).is_some_and(|url| self.dialog_url(url))
                && !self.info_protected(info, infos)
        }).filter_map(|info| info.get("targetId").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_string)).collect())
    }

    pub fn attach_dialog(&mut self, target: &str) -> Result<String, String> {
        self.dialog_target(target)?;
        let result = self.raw_call("Target.attachToTarget", json!({"targetId":target,"flatten":true}), None, DOCUMENT_CHECK)?;
        let session = result.get("sessionId").and_then(Value::as_str).filter(|session| !session.is_empty())
            .ok_or("Target.attachToTarget returned no sessionId\n".to_string())?.to_string();
        self.session_targets.insert(session.clone(), target.to_string());
        self.dialog_sessions.insert(session.clone());
        let observed = (|| {
            let url = self.dialog_target(target)?;
            if self.dialog_history(&session)? != url { return Err(self.refusal()) }
            self.send("Page.enable", json!({}), Some(&session))?;
            Ok(())
        })();
        if let Err(message) = observed {
            let _ = self.raw_call("Target.detachFromTarget", json!({"sessionId":session}), None, DOCUMENT_CHECK);
            self.dialog_sessions.remove(&session);
            self.dialogs.remove(&session);
            self.session_targets.remove(&session);
            return Err(message);
        }
        Ok(session)
    }

    pub fn wait_dialog(&mut self, session: &str, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.blocked.contains(session) || !self.dialog_sessions.contains(session) {
                return Err(self.refusal());
            }
            if let Some(dialog) = self.dialogs.get(session) {
                let target = self.session_target(session).ok_or_else(|| self.refusal())?;
                if dialog.frame != target || !self.dialog_url(&dialog.url) { return Err(self.refusal()) }
                return Ok(());
            }
            if Instant::now() >= deadline || !self.pump(deadline)? {
                return Err(DIALOG_UNOBSERVED.into());
            }
        }
    }

    fn dialog_url(&self, text: &str) -> bool {
        url::Url::parse(text).is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
            && !self.guard.url(text)
    }

    fn dialog_target(&mut self, target: &str) -> Result<String, String> {
        if target.is_empty() || self.protected_targets.contains(target) || self.abandoned.contains(target) {
            return Err(self.refusal());
        }
        let result = self.raw_call("Target.getTargetInfo", json!({"targetId":target}), None, DOCUMENT_CHECK)?;
        let info = result.get("targetInfo").ok_or_else(|| self.refusal())?;
        let text = |key| info.get(key).and_then(Value::as_str).unwrap_or("");
        if text("targetId") != target || text("type") != "page" || !self.dialog_url(text("url"))
            || self.info_protected(info, &[]) { return Err(self.refusal()) }
        let url = text("url").to_string();
        self.targets.insert(target.to_string(), info.clone());
        Ok(url)
    }

    fn dialog_history(&mut self, session: &str) -> Result<String, String> {
        let history = self.raw_call("Page.getNavigationHistory", json!({}), Some(session), DOCUMENT_CHECK)?;
        let index = history.get("currentIndex").and_then(Value::as_u64)
            .and_then(|index| usize::try_from(index).ok()).ok_or_else(|| self.refusal())?;
        let url = history.get("entries").and_then(Value::as_array)
            .and_then(|entries| entries.get(index))
            .and_then(|entry| entry.get("url")).and_then(Value::as_str)
            .filter(|url| self.dialog_url(url)).ok_or_else(|| self.refusal())?;
        Ok(url.to_string())
    }

    fn admit_dialog(&mut self, session: &str) -> Result<(), String> {
        if self.blocked.contains(session) || self.guard_sessions.contains(session) { return Err(self.refusal()) }
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let dialog = self.dialogs.get(session).cloned().ok_or(DIALOG_UNOBSERVED.to_string())?;
        if dialog.frame != target || !self.dialog_url(&dialog.url) { return Err(self.refusal()) }
        let url = self.dialog_target(&target)?;
        let current = self.dialog_history(session)?;
        if current != dialog.url || url != current || self.dialogs.get(session) != Some(&dialog)
            || self.blocked.contains(session) || self.protected_targets.contains(&target) { return Err(self.refusal()) }
        self.dialogs.remove(session);
        Ok(())
    }

    fn observe_dialog(&mut self, event: &Value) {
        let method = event.get("method").and_then(Value::as_str).unwrap_or("");
        if method == "Target.detachedFromTarget" {
            if let Some(session) = event.pointer("/params/sessionId").and_then(Value::as_str) {
                self.dialog_sessions.remove(session);
                self.dialogs.remove(session);
            }
            return;
        }
        let Some(session) = event.get("sessionId").and_then(Value::as_str) else { return };
        match method {
            "Page.javascriptDialogOpening" if self.session_target(session).is_some() => {
                self.dialog_epoch += 1;
                self.dialogs.insert(session.to_string(), Dialog {
                    url: event.pointer("/params/url").and_then(Value::as_str).unwrap_or("").to_string(),
                    frame: event.pointer("/params/frameId").and_then(Value::as_str).unwrap_or("").to_string(),
                    epoch: self.dialog_epoch,
                });
            }
            "Page.javascriptDialogClosed" | "Page.frameStartedNavigating" | "Page.frameNavigated" => { self.dialogs.remove(session); }
            _ => {}
        }
    }
}
