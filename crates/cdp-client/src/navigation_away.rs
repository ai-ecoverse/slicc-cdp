use super::{Cdp, DOCUMENT_CHECK};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const BIOS_REFUSAL: &str = "slicc: refused: seven’s own pages and the vault are out of reach of browser automation";

#[derive(Default)]
pub(super) struct Away {
    started: bool,
    frame: Option<Value>,
    loaded: bool,
    detached: bool,
    tainted: bool,
    accepted: bool,
    cutover: bool,
    delegated: bool,
    destination: String,
    requested_frames: BTreeSet<String>,
    old_loaders: BTreeSet<String>,
}

pub(super) struct Operation {
    target: String,
    ready: bool,
    tainted: bool,
}

impl Cdp {
    pub fn navigation_away_pending(&self) -> bool { self.away_operation.is_some() }

    pub fn finish_navigation_away(mut self) -> Result<String, String> {
        self.retire_private_sessions(Instant::now() + DOCUMENT_CHECK);
        self.guard_sessions.clear();
        let operation = self.away_operation.take().ok_or_else(|| self.away_failure())?;
        if !operation.ready || operation.tainted || self.connected().is_err() {
            self.protect_target(&operation.target);
            return Err(self.away_failure());
        }
        Ok(operation.target)
    }

    pub fn navigate_away(&mut self, target: &str, destination: &str) -> Result<bool, String> {
        self.connected()?;
        if self.guard.url(destination) { return Err(self.refusal()) }
        let Ok(parsed) = url::Url::parse(destination) else { return Ok(false) };
        let url = parsed.to_string();
        if !self.away_url(&url) { return Ok(false) }
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
        self.away_operation = Some(Operation { target: target.to_string(), ready: false, tainted: false });
        self.away_target = Some(target.to_string());
        let result = self.navigate_away_locked(target, &url);
        self.away_target = None;
        let operation = self.away_operation.as_mut().unwrap();
        operation.ready = result.is_ok();
        operation.tainted |= result.is_err();
        if result.is_err() { self.protect_target(target); }
        result.map(|_| true).map_err(|_| self.away_failure())
    }

    fn navigate_away_locked(&mut self, target: &str, url: &str) -> Result<(), String> {
        let attached = self.raw_call("Target.attachToTarget", json!({"targetId":target,"flatten":true}), None, DOCUMENT_CHECK)?;
        let session = attached.get("sessionId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| self.refusal())?.to_string();
        self.guard_sessions.insert(session.clone());
        let old_loaders = self.document_versions.iter().filter(|((owner, _), _)| owner == target).map(|(_, loader)| loader.clone()).collect();
        self.away_sessions.insert(session.clone(), Away { destination: self.guard.resource_key(url).unwrap(), old_loaders, ..Away::default() });
        let navigated = (|| {
            let delegated = match self.raw_exchange("Page.getFrameTree", json!({}), Some(&session), DOCUMENT_CHECK, false) {
                Ok(tree) if tree == json!({"frameTree":{"frame":{"id":"0","url":"chrome-error://chromewebdata/","securityOrigin":"://"}}}) => true,
                Ok(tree) => {
                    let root = tree.get("frameTree").ok_or_else(|| self.away_failure())?;
                    if root.pointer("/frame/parentId").is_some() || !self.away_error_tree(root) { return Err(self.away_failure()) }
                    if let Some(loader) = root.pointer("/frame/loaderId").and_then(Value::as_str) {
                        self.away_sessions.get_mut(&session).unwrap().old_loaders.insert(loader.to_string());
                    }
                    false
                }
                Err(message) if message == format!("Page.getFrameTree: {BIOS_REFUSAL}\n") => true,
                Err(_) => return Err(self.away_failure()),
            };
            self.away_sessions.get_mut(&session).unwrap().delegated = delegated;
            self.raw_call("Page.enable", json!({}), Some(&session), DOCUMENT_CHECK)?;
            if self.away_sessions[&session].tainted { return Err(self.away_failure()) }
            if !delegated {
                self.raw_call("Fetch.enable", json!({"patterns":[{"urlPattern":"*","resourceType":"Document","requestStage":"Request"}]}), Some(&session), DOCUMENT_CHECK)?;
            }
            self.away_sessions.get_mut(&session).unwrap().started = true;
            let result = self.raw_call("Page.navigate", json!({"url":url}), Some(&session), Duration::from_secs(30))?;
            if result.get("errorText").is_some() || result.get("isDownload").and_then(Value::as_bool) == Some(true) { return Err(self.refusal()) }
            let frame = result.get("frameId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| self.refusal())?;
            let loader = result.get("loaderId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| self.refusal())?;
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let away = &self.away_sessions[&session];
                if away.detached || away.tainted { return Err(self.away_failure()) }
                if let Some(committed) = &away.frame {
                    if committed.get("id").and_then(Value::as_str) == Some(frame)
                        && committed.get("loaderId").and_then(Value::as_str) == Some(loader) && away.loaded {
                        if !self.away_ordinary(committed) || !away.delegated && !away.requested_frames.contains(frame) { return Err(self.away_failure()) }
                        let frame = committed.clone();
                        self.away_sessions.get_mut(&session).unwrap().accepted = true;
                        return Ok(frame);
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
        self.away_sessions.get_mut(&session).unwrap().cutover = true;
        if self.away_sessions[&session].tainted || self.away_sessions[&session].frame.as_ref() != Some(&frame) { return Err(self.away_failure()) }
        let result = self.raw_call("Target.getTargetInfo", json!({"targetId":target}), None, DOCUMENT_CHECK)?;
        let info = result.get("targetInfo").ok_or_else(|| self.refusal())?;
        if info.get("targetId").and_then(Value::as_str) != Some(target) || info.get("type").and_then(Value::as_str) != Some("page")
            || !self.away_url(info.get("url").and_then(Value::as_str).unwrap_or("")) { return Err(self.refusal()) }
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
        if self.away_sessions.get(&session).is_some_and(|away| away.tainted || away.frame.as_ref() != Some(&frame)) { return Err(self.away_failure()) }
        Ok(())
    }

    fn away_url(&self, text: &str) -> bool {
        self.ordinary_redirect_url(text) && url::Url::parse(text).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
    }

    fn away_ordinary(&self, frame: &Value) -> bool {
        frame.get("parentId").is_none() && frame.get("unreachableUrl").is_none_or(|url| url.as_str() == Some(""))
            && self.away_url(frame.get("url").and_then(Value::as_str).unwrap_or(""))
            && self.away_url(frame.get("securityOrigin").and_then(Value::as_str).unwrap_or(""))
    }

    fn away_failure(&self) -> String { format!("{}\n", self.guard.refusal()) }

    fn away_error_tree(&self, node: &Value) -> bool {
        let Some(frame) = node.get("frame") else { return false };
        let text = |key| frame.get(key).and_then(Value::as_str).unwrap_or("");
        super::frame_text_known(text("id")) && text("id") != "0" && self.guard.error_document(text("url"), text("securityOrigin"))
            && !text("unreachableUrl").is_empty() && (self.guard.url(text("unreachableUrl")) || url::Url::parse(text("unreachableUrl")).is_err())
            && node.get("childFrames").is_none_or(|children| children.as_array().is_some_and(|children| children.iter().all(|child| self.away_error_tree(child))))
    }

    pub(super) fn away_event(&mut self, event: &Value) -> bool {
        let method = event.get("method").and_then(Value::as_str).unwrap_or("");
        let session = event.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let later_commit = method == "Page.frameNavigated" && event.pointer("/params/frame/parentId").is_none()
            && self.away_operation.as_ref().is_some_and(|operation| operation.ready && self.session_target(session).as_deref() == Some(&operation.target));
        if later_commit { self.away_operation.as_mut().unwrap().tainted = true; }
        if self.away_sessions.contains_key(session) {
            let root = event.pointer("/params/frame").filter(|frame| frame.get("parentId").is_none());
            let ordinary_frame = root.is_some_and(|frame| self.away_ordinary(frame));
            let error_frame = root.is_some_and(|frame| self.guard.error_document(frame.get("url").and_then(Value::as_str).unwrap_or(""), frame.get("securityOrigin").and_then(Value::as_str).unwrap_or("")));
            let request = (method == "Fetch.requestPaused").then(|| event.get("params")).flatten();
            let request_url = request.and_then(|params| params.pointer("/request/url")).and_then(Value::as_str).unwrap_or("");
            let request_ordinary = self.away_url(request_url);
            let request_key = self.guard.resource_key(request_url);
            let network_tainted = method == "Network.requestWillBeSent" && event.pointer("/params/type").and_then(Value::as_str) == Some("Document")
                && (!self.away_url(event.pointer("/params/request/url").and_then(Value::as_str).unwrap_or(""))
                    || event.pointer("/params/redirectResponse/url").is_some_and(|url| url.as_str().is_none_or(|url| !self.away_url(url))));
            let mut command = None;
            {
                let away = self.away_sessions.get_mut(session).unwrap();
                away.tainted |= away.cutover;
                if method == "Page.frameNavigated" {
                    if let Some(frame) = root {
                        let loader = frame.get("loaderId").and_then(Value::as_str).unwrap_or("");
                        if !away.started {
                            away.old_loaders.insert(loader.to_string());
                            away.tainted |= !error_frame;
                        } else {
                            away.tainted |= away.accepted || !ordinary_frame || event.pointer("/params/type").and_then(Value::as_str) != Some("Navigation") || away.old_loaders.contains(loader);
                            away.frame = Some(frame.clone());
                            away.loaded = false;
                        }
                    }
                }
                if away.started {
                    away.tainted |= network_tainted;
                    if method == "Page.frameStartedNavigating" {
                        away.tainted |= event.pointer("/params/navigationType").and_then(Value::as_str) != Some("differentDocument");
                    }
                    if let Some(params) = request.filter(|_| !away.delegated) {
                        let id = params.get("requestId").and_then(Value::as_str).unwrap_or("");
                        let frame = params.get("frameId").and_then(Value::as_str).unwrap_or("");
                        away.tainted |= !request_ordinary || id.is_empty() || frame.is_empty() || params.get("resourceType").and_then(Value::as_str) != Some("Document");
                        if !away.tainted && request_key.as_deref() == Some(away.destination.as_str()) { away.requested_frames.insert(frame.to_string()); }
                        if !id.is_empty() {
                            command = Some(if away.tainted { ("Fetch.failRequest", json!({"requestId":id,"errorReason":"BlockedByClient"})) }
                                else { ("Fetch.continueRequest", json!({"requestId":id})) });
                        }
                    }
                }
                if away.frame.is_some() && (method == "Page.loadEventFired" || method == "Page.lifecycleEvent" && event.pointer("/params/name").and_then(Value::as_str) == Some("load")) {
                    away.loaded = true;
                }
            }
            if let Some((method, params)) = command {
                if self.raw_call(method, params, Some(session), DOCUMENT_CHECK).is_err() { self.away_sessions.get_mut(session).unwrap().tainted = true; }
            }
            if let Some(operation) = &mut self.away_operation { operation.tainted |= self.away_sessions[session].tainted; }
            return true;
        }
        if method == "Target.detachedFromTarget" {
            if let Some(away) = event.pointer("/params/sessionId").and_then(Value::as_str).and_then(|session| self.away_sessions.get_mut(session)) {
                away.detached = true;
                if let Some(operation) = &mut self.away_operation { operation.tainted |= away.cutover; }
                return true;
            }
        }
        self.away_target.as_deref().is_some_and(|target| {
            event.pointer("/params/targetInfo/targetId").and_then(Value::as_str) == Some(target)
                || event.pointer("/params/targetId").and_then(Value::as_str) == Some(target)
        })
    }
}
