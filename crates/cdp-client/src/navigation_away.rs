use super::{Cdp, DOCUMENT_CHECK};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Away {
    started: bool,
    frame: Option<Value>,
    loaded: bool,
    detached: bool,
}

impl Cdp {
    pub fn navigate_away(&mut self, target: &str, destination: &str) -> Result<bool, String> {
        self.connected()?;
        if self.guard.url(destination) { return Err(self.refusal()) }
        let Ok(parsed) = url::Url::parse(destination) else { return Ok(false) };
        let url = parsed.to_string();
        if !matches!(parsed.scheme(), "http" | "https") || !self.ordinary_redirect_url(&url) { return Ok(false) }
        let result = match self.raw_call("Target.getTargetInfo", json!({"targetId":target}), None, DOCUMENT_CHECK) {
            Ok(result) => result,
            Err(message) if super::transport_error(&message) => return Err(message),
            Err(_) => return Ok(false),
        };
        let Some(info) = result.get("targetInfo") else { return Ok(false) };
        if info.get("targetId").and_then(Value::as_str) != Some(target) || info.get("type").and_then(Value::as_str) != Some("page") {
            return Ok(false);
        }
        let current = info.get("url").and_then(Value::as_str).unwrap_or("");
        if !self.info_protected(info, &[]) && !self.guard.error_page_url(current)
            && !self.documents.get(target).is_some_and(|doc| self.sealed(doc)) { return Ok(false) }
        self.targets.insert(target.to_string(), info.clone());
        self.protect_target(target);
        self.away_target = Some(target.to_string());
        let result = self.navigate_away_locked(target, &url);
        self.away_target = None;
        if result.is_err() { self.protect_target(target); }
        result.map(|_| true)
    }

    fn navigate_away_locked(&mut self, target: &str, url: &str) -> Result<(), String> {
        let attached = self.raw_call("Target.attachToTarget", json!({"targetId":target,"flatten":true}), None, DOCUMENT_CHECK)?;
        let session = attached.get("sessionId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| self.refusal())?.to_string();
        self.guard_sessions.insert(session.clone());
        self.away_sessions.insert(session.clone(), Away::default());
        let navigated = (|| {
            self.raw_call("Page.enable", json!({}), Some(&session), DOCUMENT_CHECK)?;
            self.away_sessions.get_mut(&session).unwrap().started = true;
            let result = self.raw_call("Page.navigate", json!({"url":url}), Some(&session), Duration::from_secs(30))?;
            if result.get("errorText").is_some() || result.get("isDownload").and_then(Value::as_bool) == Some(true) { return Err(self.refusal()) }
            let frame = result.get("frameId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| self.refusal())?;
            let loader = result.get("loaderId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| self.refusal())?;
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let away = &self.away_sessions[&session];
                if away.detached { return Err(self.refusal()) }
                if let Some(committed) = &away.frame {
                    if committed.get("id").and_then(Value::as_str) == Some(frame)
                        && committed.get("loaderId").and_then(Value::as_str) == Some(loader) && away.loaded {
                        if !self.away_ordinary(committed) { return Err(self.refusal()) }
                        return Ok(committed.clone());
                    }
                }
                if Instant::now() >= deadline || !self.pump(deadline)? { return Err(self.refusal()) }
            }
        })();
        let detached = self.connected().and_then(|_| self.raw_call("Target.detachFromTarget", json!({"sessionId":session}), None, DOCUMENT_CHECK));
        self.guard_sessions.remove(&session);
        self.blocked.insert(session.clone());
        let frame = navigated?;
        detached?;
        let result = self.raw_call("Target.getTargetInfo", json!({"targetId":target}), None, DOCUMENT_CHECK)?;
        let info = result.get("targetInfo").ok_or_else(|| self.refusal())?;
        if info.get("targetId").and_then(Value::as_str) != Some(target) || info.get("type").and_then(Value::as_str) != Some("page")
            || !self.ordinary_redirect_url(info.get("url").and_then(Value::as_str).unwrap_or("")) { return Err(self.refusal()) }
        self.targets.insert(target.to_string(), info.clone());
        if let Some(guard) = self.guards.remove(target) {
            self.raw_call("Target.detachFromTarget", json!({"sessionId":guard}), None, DOCUMENT_CHECK)?;
        }
        self.touch(target)?;
        let document = self.document_state(target, true).ok_or_else(|| self.refusal())?;
        let evidence = document.frames.get(frame.get("id").and_then(Value::as_str).unwrap_or("")).ok_or_else(|| self.refusal())?;
        if evidence[0] != frame.get("loaderId").and_then(Value::as_str).unwrap_or("")
            || evidence[1] != format!("{}{}", frame.get("url").and_then(Value::as_str).unwrap_or(""), frame.get("urlFragment").and_then(Value::as_str).unwrap_or("")) {
            return Err(self.refusal());
        }
        let guard = self.guards.get(target).cloned().ok_or_else(|| self.refusal())?;
        self.verify_page_scope(Some(&guard))?;
        Ok(())
    }

    fn away_ordinary(&self, frame: &Value) -> bool {
        frame.get("parentId").is_none() && frame.get("unreachableUrl").is_none_or(|url| url.as_str() == Some(""))
            && url::Url::parse(frame.get("url").and_then(Value::as_str).unwrap_or("")).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
            && self.ordinary_redirect_url(frame.get("url").and_then(Value::as_str).unwrap_or(""))
            && self.ordinary_redirect_url(frame.get("securityOrigin").and_then(Value::as_str).unwrap_or(""))
    }

    pub(super) fn away_event(&mut self, event: &Value) -> bool {
        let method = event.get("method").and_then(Value::as_str).unwrap_or("");
        let session = event.get("sessionId").and_then(Value::as_str).unwrap_or("");
        if let Some(away) = self.away_sessions.get_mut(session) {
            if away.started && method == "Page.frameNavigated" {
                if let Some(frame) = event.pointer("/params/frame").filter(|frame| frame.get("parentId").is_none()) {
                    away.frame = Some(frame.clone());
                    away.loaded = false;
                }
            }
            if away.frame.is_some() && (method == "Page.loadEventFired" || method == "Page.lifecycleEvent" && event.pointer("/params/name").and_then(Value::as_str) == Some("load")) {
                away.loaded = true;
            }
            return true;
        }
        if method == "Target.detachedFromTarget" {
            if let Some(away) = event.pointer("/params/sessionId").and_then(Value::as_str).and_then(|session| self.away_sessions.get_mut(session)) {
                away.detached = true;
                return true;
            }
        }
        self.away_target.as_deref().is_some_and(|target| {
            event.pointer("/params/targetInfo/targetId").and_then(Value::as_str) == Some(target)
                || event.pointer("/params/targetId").and_then(Value::as_str) == Some(target)
        })
    }
}
