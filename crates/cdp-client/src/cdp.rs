use crate::fetch::{self, Decision, Paused};
use crate::net::Ws;
use crate::protected::{Opener, Protected};
use crate::shield;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

pub trait Transport {
    fn send_text(&mut self, text: &str) -> Result<(), String>;
    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String>;
}

impl Transport for Ws {
    fn send_text(&mut self, text: &str) -> Result<(), String> {
        Ws::send_text(self, text)
    }

    fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
        Ws::recv_text(self, deadline)
    }
}

pub type Decider = Box<dyn FnMut(&Paused) -> Decision>;

pub const UNSEEN: &str = "slicc: refused: slicc hasn't seen this request, so it could be seven's\n";
pub const UNGUARDED: &str = "slicc: refused: slicc can't keep seven's origin out of this target\n";
pub const GUARD_SETUP: Duration = Duration::from_secs(30);
const OPTIONAL_STEP: Duration = Duration::from_secs(10);
pub const COOKIES: &str = "slicc: refused: cookies on seven's origin\n";
pub const STREAM: &str = "slicc: refused: slicc didn't hand out this stream, so it could be seven's\n";

pub struct Cdp {
    ws: Box<dyn Transport>,
    next_id: i64,
    events: Vec<Value>,
    guard: Protected,
    paused: VecDeque<Paused>,
    deciders: BTreeMap<String, Decider>,
    decided: BTreeMap<i64, (String, String)>,
    fetch_errors: Vec<String>,
    blocked: BTreeSet<String>,
    session_targets: BTreeMap<String, String>,
    protected_targets: BTreeSet<String>,
    guards: BTreeMap<String, String>,
    guard_sessions: BTreeSet<String>,
    guarding: BTreeSet<String>,
    abandoned: BTreeSet<String>,
    unguarded_optional: BTreeMap<String, BTreeSet<String>>,
    notices: Vec<String>,
    discovering: bool,
    requests: BTreeMap<String, bool>,
    handed: BTreeSet<(String, String)>,
    answering: BTreeMap<i64, (String, String)>,
    held: BTreeMap<String, Vec<Value>>,
    streams: BTreeSet<String>,
    contexts: BTreeSet<String>,
    targets: BTreeMap<String, Value>,
    frames: BTreeMap<String, Vec<String>>,
    #[cfg(test)]
    pub(crate) paused_trail: Vec<(String, String, &'static str)>,
}

impl Cdp {
    pub fn connect(url: &str) -> Result<Self, String> {
        Ok(Self::from_transport(Box::new(crate::net::ws_connect(url)?)))
    }

    pub fn from_transport(ws: Box<dyn Transport>) -> Self {
        Self {
            ws,
            next_id: 0,
            events: Vec::new(),
            guard: Protected::default(),
            paused: VecDeque::new(),
            deciders: BTreeMap::new(),
            decided: BTreeMap::new(),
            fetch_errors: Vec::new(),
            blocked: BTreeSet::new(),
            session_targets: BTreeMap::new(),
            protected_targets: BTreeSet::new(),
            guards: BTreeMap::new(),
            guard_sessions: BTreeSet::new(),
            guarding: BTreeSet::new(),
            abandoned: BTreeSet::new(),
            unguarded_optional: BTreeMap::new(),
            notices: Vec::new(),
            discovering: false,
            requests: BTreeMap::new(),
            handed: BTreeSet::new(),
            answering: BTreeMap::new(),
            held: BTreeMap::new(),
            streams: BTreeSet::new(),
            contexts: BTreeSet::new(),
            targets: BTreeMap::new(),
            frames: BTreeMap::new(),
            #[cfg(test)]
            paused_trail: Vec::new(),
        }
    }

    pub fn events(&self) -> &[Value] {
        &self.events
    }

    pub fn guard(&self) -> &Protected {
        &self.guard
    }

    pub fn protect_origins(&mut self, extra_origins: &[&str]) {
        self.guard = Protected::new(extra_origins);
    }

    pub fn take_notices(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notices)
    }

    pub fn unguarded_optional(&self, target_id: &str) -> Vec<String> {
        self.unguarded_optional
            .get(target_id)
            .map(|steps| steps.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn session_blocked(&self, session_id: &str) -> bool {
        self.blocked.contains(session_id)
    }

    pub fn is_protected_target(&self, target_id: &str) -> bool {
        self.protected_targets.contains(target_id)
            || self
                .session_targets
                .iter()
                .any(|(session, target)| target == target_id && self.blocked.contains(session))
    }

    pub fn set_decider(&mut self, session_id: &str, decider: Option<Decider>) {
        match decider {
            Some(decider) => {
                self.deciders.insert(session_id.to_string(), decider);
                let queued: Vec<Paused> = self.paused.drain(..).collect();
                for paused in queued {
                    if paused.session_id == session_id {
                        self.decide_inline(paused);
                    } else {
                        self.paused.push_back(paused);
                    }
                }
            }
            None => {
                self.deciders.remove(session_id);
            }
        }
    }

    pub fn has_decider(&self, session_id: &str) -> bool {
        self.deciders.contains_key(session_id)
    }

    pub fn take_fetch_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.fetch_errors)
    }

    pub fn call(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, String> {
        self.call_for(method, params, session_id, Duration::from_secs(30))
    }

    pub fn call_for(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, String> {
        let session_id = session_id.filter(|id| !id.is_empty());
        self.admit(method, &params, session_id)?;
        if method == "Target.createTarget" {
            return self.create_target(params, timeout);
        }
        let result = self.raw_call(method, params.clone(), session_id, timeout)?;
        if let (true, Some(session), Some(id)) = (
            shield::answers_interception(method),
            session_id,
            shield::request_key(&params),
        ) {
            self.settle(session, &id);
        }
        if let Some(session) = session_id {
            if self.blocked.contains(session) && !harmless(method, &params) {
                return Err(self.refusal());
            }
        }
        self.after(method, &params, session_id, result)
    }

    pub fn notify(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<(), String> {
        let session_id = session_id.filter(|id| !id.is_empty());
        self.admit(method, &params, session_id)?;
        if matches!(method, "Target.createTarget" | "Target.attachToTarget") {
            return Err(format!("{method} must be a call, not a notify\n"));
        }
        let answered = shield::answers_interception(method)
            .then(|| shield::request_key(&params))
            .flatten()
            .zip(session_id.map(str::to_string));
        let sent = self.send(method, params, session_id)?;
        if let Some((id, session)) = answered {
            self.answering.insert(sent, (session, id));
        }
        Ok(())
    }

    fn refusal(&self) -> String {
        format!("{}\n", self.guard.refusal())
    }

    fn admit(&mut self, method: &str, params: &Value, session_id: Option<&str>) -> Result<(), String> {
        if let Some(session) = session_id {
            if self.blocked.contains(session) && !harmless(method, params) {
                return Err(self.refusal());
            }
            if self.guard_sessions.contains(session) {
                return Err(self.refusal());
            }
        }
        if params
            .get("targetId")
            .and_then(|v| v.as_str())
            .is_some_and(|target| self.abandoned.contains(target))
        {
            return Err(UNGUARDED.to_string());
        }
        if let Some(reason) = shield::browser_wide(method, params, session_id, &self.contexts) {
            return Err(reason);
        }
        if method == "Target.setAutoAttach"
            && params.get("autoAttach").and_then(|v| v.as_bool()) == Some(true)
        {
            return Err("slicc: refused: auto-attach is not supported by slicc-cdp\n".into());
        }
        if matches!(method, "Target.attachToTarget" | "Target.setAutoAttach")
            && params.get("flatten").and_then(|v| v.as_bool()) != Some(true)
        {
            return Err("slicc: refused: attach with flatten: true\n".into());
        }
        if let Some(url) = params.get("url").and_then(|v| v.as_str()) {
            if shield::cookie_method(method) {
                if shield::cookie_refused(&self.guard, params) {
                    return Err(COOKIES.into());
                }
            } else if self.guard.url(url) || self.guard.url(&shield::web_socket_as_http(url)) {
                return Err(self.refusal());
            }
        }
        if shield::cookie_method(method) && shield::cookie_refused(&self.guard, params) {
            return Err(COOKIES.into());
        }
        if shield::names_protected_storage(&self.guard, params) {
            return Err(self.refusal());
        }
        if method == "Page.navigateToHistoryEntry" {
            let entry = params.get("entryId").and_then(|v| v.as_i64());
            let history = self.raw_call(
                "Page.getNavigationHistory",
                json!({}),
                session_id,
                Duration::from_secs(10),
            )?;
            let url = history
                .get("entries")
                .and_then(|v| v.as_array())
                .and_then(|entries| {
                    entries
                        .iter()
                        .find(|item| item.get("id").and_then(|v| v.as_i64()) == entry)
                })
                .and_then(|item| item.get("url").and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string();
            if self.guard.url(&url) {
                return Err(self.refusal());
            }
        }
        if !matches!(method, "Target.getTargetInfo" | "Target.detachFromTarget") {
            for key in ["targetId", "frameId"] {
                if let Some(target) = params.get(key).and_then(|v| v.as_str()) {
                    let target = target.to_string();
                    if self.target_protected(&target, key == "frameId")? {
                        return Err(self.refusal());
                    }
                }
            }
        }
        if shield::interception_method(method) {
            let id = shield::request_key(params).unwrap_or_default();
            let handed = session_id
                .is_some_and(|session| self.handed.contains(&(session.to_string(), id.clone())));
            if !handed || self.request_state(&id) == Some(true) {
                return Err(UNSEEN.into());
            }
        }
        if shield::body_method(method) {
            let id = shield::request_key(params);
            match id.as_deref().and_then(|id| self.request_state(id)) {
                Some(true) => return Err(self.refusal()),
                Some(false) => {}
                None => {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    let mut known = None;
                    while known.is_none() && Instant::now() < deadline {
                        if !self.pump(deadline)? {
                            break;
                        }
                        known = id.as_deref().and_then(|id| self.request_state(id));
                    }
                    match known {
                        Some(false) => {}
                        Some(true) => return Err(self.refusal()),
                        None => return Err(UNSEEN.into()),
                    }
                }
            }
        }
        if method == "IO.read" {
            let handle = params.get("handle").and_then(|v| v.as_str()).unwrap_or("");
            if !self.streams.contains(handle) {
                return Err(STREAM.into());
            }
        }
        Ok(())
    }

    fn request_state(&self, id: &str) -> Option<bool> {
        self.requests.get(id).copied()
    }

    fn after(
        &mut self,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
        mut result: Value,
    ) -> Result<Value, String> {
        if let Some(stream) = result.get("stream").and_then(|v| v.as_str()) {
            self.streams.insert(stream.to_string());
        }
        if let Some(stream) = result.pointer("/resource/stream").and_then(|v| v.as_str()) {
            self.streams.insert(stream.to_string());
        }
        if method == "IO.resolveBlob" {
            if let Some(uuid) = result.get("uuid").and_then(|v| v.as_str()) {
                self.streams.insert(format!("blob:{uuid}"));
            }
        }
        match method {
            "Target.attachToTarget" => {
                let session = result
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let target = params
                    .get("targetId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if !session.is_empty() {
                    self.session_targets.insert(session.clone(), target.clone());
                    if let Err(message) = self.touch(&target) {
                        if transport_error(&message) {
                            self.session_targets.remove(&session);
                            return Err(message);
                        }
                        let detached = self.raw_call(
                            "Target.detachFromTarget",
                            json!({ "sessionId": session }),
                            None,
                            Duration::from_secs(5),
                        );
                        self.session_targets.remove(&session);
                        if let Err(closed) = detached {
                            if transport_error(&closed) {
                                return Err(closed);
                            }
                        }
                        return Err(message);
                    }
                    if self.protected_targets.contains(&target) {
                        self.block(&session);
                        return Err(self.refusal());
                    }
                }
            }
            "Target.createBrowserContext" => {
                if let Some(id) = result.get("browserContextId").and_then(|v| v.as_str()) {
                    self.contexts.insert(id.to_string());
                }
            }
            "Target.getTargets" => {
                let infos = result
                    .get("targetInfos")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                for info in &infos {
                    if let Some(id) = info.get("targetId").and_then(|v| v.as_str()) {
                        self.targets.insert(id.to_string(), info.clone());
                    }
                }
                let mut out = Vec::new();
                for info in &infos {
                    let redacted = self.redact(info, &infos);
                    if !self.guard.target_type(info.get("type").and_then(Value::as_str).unwrap_or("")) {
                        out.push(redacted);
                    }
                }
                result["targetInfos"] = Value::Array(out);
            }
            "Target.getTargetInfo" => {
                if let Some(info) = result.get("targetInfo").cloned() {
                    let target = info.get("targetId").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let protected = self.target_protected(&target, false).unwrap_or(true);
                    if protected {
                        result["targetInfo"] = shield::redacted(&info);
                    }
                }
            }
            _ if shield::cookie_reader(method) => {
                shield::drop_protected_cookies(&self.guard, &mut result);
            }
            _ => {}
        }
        let _ = session_id;
        Ok(result)
    }

    fn redact(&mut self, info: &Value, all: &[Value]) -> Value {
        if self.info_protected(info, all) {
            if let Some(id) = info.get("targetId").and_then(|v| v.as_str()) {
                self.protected_targets.insert(id.to_string());
            }
            shield::redacted(info)
        } else {
            info.clone()
        }
    }

    fn info_protected(&self, info: &Value, all: &[Value]) -> bool {
        if self.guard.target_type(info.get("type").and_then(Value::as_str).unwrap_or("")) {
            return true;
        }
        let url = info.get("url").and_then(|v| v.as_str()).unwrap_or("");
        let id = info.get("targetId").and_then(|v| v.as_str()).unwrap_or("");
        let previous = self.protected_targets.contains(id);
        let opener = match info
            .get("openerId")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
        {
            None => Opener::None,
            Some(opener) => match all
                .iter()
                .find(|item| item.get("targetId").and_then(|v| v.as_str()) == Some(opener))
            {
                Some(found) if found.get("targetId") == info.get("targetId") => Opener::Unknown,
                Some(found) if self.info_protected(found, &[]) => Opener::Protected,
                Some(_) => Opener::Unprotected,
                None if self.protected_targets.contains(opener) => Opener::Protected,
                None => match self.targets.get(opener) {
                    Some(found) if found.get("targetId") != info.get("targetId") => {
                        if self.info_protected(found, &[]) {
                            Opener::Protected
                        } else {
                            Opener::Unprotected
                        }
                    }
                    _ => Opener::Unknown,
                },
            },
        };
        self.guard.target(url, previous, opener)
    }

    pub fn target_protected(&mut self, target_id: &str, frame: bool) -> Result<bool, String> {
        if self.protected_targets.contains(target_id) {
            return Ok(true);
        }
        let info = match self.raw_call(
            "Target.getTargetInfo",
            json!({ "targetId": target_id }),
            None,
            Duration::from_secs(30),
        ) {
            Ok(info) => info.get("targetInfo").cloned().unwrap_or(Value::Null),
            Err(_) if frame => return Ok(false),
            Err(message) => return Err(message),
        };
        if info.is_object() {
            self.targets.insert(target_id.to_string(), info.clone());
        }
        let url = info.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let opener = match info
            .get("openerId")
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
            .map(str::to_string)
        {
            None => Opener::None,
            Some(id) if self.protected_targets.contains(&id) => Opener::Protected,
            Some(id) => match self.raw_call(
                "Target.getTargetInfo",
                json!({ "targetId": id }),
                None,
                Duration::from_secs(30),
            ) {
                Ok(found) => {
                    let opener_info = found.get("targetInfo").unwrap_or(&Value::Null);
                    let opener_url = opener_info.get("url").and_then(Value::as_str).unwrap_or("");
                    if self.guard.target_type(opener_info.get("type").and_then(Value::as_str).unwrap_or(""))
                        || self.guard.target(opener_url, false, Opener::Unknown)
                    {
                        Opener::Protected
                    } else {
                        Opener::Unprotected
                    }
                }
                Err(_) => Opener::Unknown,
            },
        };
        let protected = self.guard.target_type(info.get("type").and_then(Value::as_str).unwrap_or(""))
            || self.guard.target(&url, false, opener);
        if protected {
            self.protected_targets.insert(target_id.to_string());
        }
        Ok(protected)
    }

    fn create_target(&mut self, params: Value, timeout: Duration) -> Result<Value, String> {
        let url = params
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("about:blank")
            .to_string();
        let mut blank = params.clone();
        blank["url"] = json!("about:blank");
        let result = self.raw_call("Target.createTarget", blank, None, timeout)?;
        let target = result
            .get("targetId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if let Err(message) = self.touch(&target) {
            let _ = self.raw_call(
                "Target.closeTarget",
                json!({ "targetId": target }),
                None,
                Duration::from_secs(5),
            );
            return Err(message);
        }
        if !url.eq_ignore_ascii_case("about:blank") {
            let guard = self.guards.get(&target).cloned();
            self.send("Page.navigate", json!({ "url": url }), guard.as_deref())?;
        }
        Ok(result)
    }

    fn touch(&mut self, target_id: &str) -> Result<(), String> {
        if target_id.is_empty() || self.guards.contains_key(target_id) {
            return Ok(());
        }
        let kind = match self.targets.get(target_id) {
            Some(info) => info.get("type").and_then(|v| v.as_str()).map(str::to_string),
            None => self
                .raw_call(
                    "Target.getTargetInfo",
                    json!({ "targetId": target_id }),
                    None,
                    Duration::from_secs(10),
                )
                .ok()
                .and_then(|info| {
                    info.pointer("/targetInfo/type")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                }),
        };
        if kind
            .as_deref()
            .is_some_and(|kind| !matches!(kind, "page" | "iframe" | "background_page" | "webview" | "other"))
        {
            return Ok(());
        }
        let budget = Instant::now() + GUARD_SETUP;
        if !self.discovering {
            self.raw_call(
                "Target.setDiscoverTargets",
                json!({ "discover": true }),
                None,
                GUARD_SETUP,
            )
            .map_err(unguarded)?;
            self.discovering = true;
        }
        self.guarding.insert(target_id.to_string());
        let left = budget.saturating_duration_since(Instant::now());
        let attached = if left.is_zero() {
            Err("Target.attachToTarget timed out\n".to_string())
        } else {
            self.raw_call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                None,
                left,
            )
        };
        self.guarding.remove(target_id);
        let session = match attached
            .map(|value| value.get("sessionId").and_then(|v| v.as_str()).map(str::to_string))
        {
            Ok(Some(session)) => session,
            Ok(None) => {
                self.abandon(target_id);
                return Err(UNGUARDED.to_string());
            }
            Err(message) => {
                self.abandon(target_id);
                return Err(unguarded(message));
            }
        };
        self.guard_sessions.insert(session.clone());
        self.guards.insert(target_id.to_string(), session.clone());
        let setup = [
            (
                "Fetch.enable",
                json!({ "patterns": [{ "urlPattern": "*", "resourceType": "Document", "requestStage": "Request" }] }),
            ),
            ("Network.enable", json!({})),
            ("Network.setBypassServiceWorker", json!({ "bypass": true })),
            ("Page.enable", json!({})),
            ("Runtime.enable", json!({})),
        ];
        for (method, params) in setup {
            let optional = matches!(method, "Page.enable" | "Runtime.enable");
            let left = budget.saturating_duration_since(Instant::now());
            let span = if optional { left.min(OPTIONAL_STEP) } else { left };
            let answer = if span.is_zero() {
                Err(format!("{method} timed out\n"))
            } else {
                self.raw_call(method, params, Some(&session), span)
            };
            let failed = match answer {
                Ok(_) => None,
                Err(message) if optional && message == format!("{method} timed out\n") => {
                    if self
                        .unguarded_optional
                        .entry(target_id.to_string())
                        .or_default()
                        .insert(method.to_string())
                    {
                        self.notices
                            .push(format!("slicc: guard: {method} timed out; continuing\n"));
                    }
                    None
                }
                Err(message) if optional && message.starts_with(&format!("{method}:")) => None,
                Err(message) => Some(message),
            };
            if let Some(message) = failed {
                self.guards.remove(target_id);
                self.abandon(target_id);
                let _ = self.raw_call(
                    "Target.detachFromTarget",
                    json!({ "sessionId": session }),
                    None,
                    Duration::from_secs(5),
                );
                return Err(unguarded(message));
            }
        }
        Ok(())
    }

    fn abandon(&mut self, target_id: &str) {
        self.abandoned.insert(target_id.to_string());
        self.protect_target(target_id);
    }

    fn adopt_guard_attach(&mut self, value: &Value) {
        if value.get("method").and_then(|v| v.as_str()) != Some("Target.attachedToTarget") {
            return;
        }
        let target = value
            .pointer("/params/targetInfo/targetId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(session) = value.pointer("/params/sessionId").and_then(|v| v.as_str()) else {
            return;
        };
        if self.guarding.contains(target) {
            self.guard_sessions.insert(session.to_string());
        } else if self.abandoned.contains(target) {
            self.guard_sessions.insert(session.to_string());
            let _ = self.send(
                "Target.detachFromTarget",
                json!({ "sessionId": session }),
                None,
            );
        }
    }

    fn block(&mut self, session_id: &str) {
        if !self.blocked.insert(session_id.to_string()) {
            return;
        }
        if let Some(target) = self.session_targets.get(session_id) {
            self.protected_targets.insert(target.clone());
        }
        let queued: Vec<Paused> = self.paused.drain(..).collect();
        for paused in queued {
            if paused.session_id == session_id {
                self.answer_protected(&paused);
            } else {
                self.paused.push_back(paused);
            }
        }
        let _ = self.send("Fetch.disable", json!({}), Some(session_id));
        let _ = self.send(
            "Target.detachFromTarget",
            json!({ "sessionId": session_id }),
            None,
        );
    }

    fn session_target(&self, session_id: &str) -> Option<String> {
        self.session_targets.get(session_id).cloned().or_else(|| {
            self.guards
                .iter()
                .find(|(_, guard)| guard.as_str() == session_id)
                .map(|(target, _)| target.clone())
        })
    }

    fn committed_protected(&mut self, session_id: &str) {
        match self.session_target(session_id) {
            Some(target) => self.protect_target(&target),
            None => self.block(session_id),
        }
        if !self.guard_sessions.contains(session_id) {
            self.block(session_id);
        }
    }

    fn frame_protected(&self, frame_id: &str) -> bool {
        if frame_id.is_empty() {
            return false;
        }
        self.protected_targets.contains(frame_id)
            || self
                .frames
                .get(frame_id)
                .is_some_and(|known| known.iter().any(|url| self.guard.url(url)))
    }

    fn initiated_by_protected(&self, paused: &Paused) -> bool {
        if self.frame_protected(&paused.frame_id) {
            return true;
        }
        if self
            .session_target(&paused.session_id)
            .is_some_and(|target| self.protected_targets.contains(&target))
        {
            return true;
        }
        ["referer", "origin"].iter().any(|name| {
            paused
                .header(name)
                .is_some_and(|value| value != "null" && self.guard.url(value))
        })
    }

    fn protect_target(&mut self, target_id: &str) {
        self.protected_targets.insert(target_id.to_string());
        let sessions: Vec<String> = self
            .session_targets
            .iter()
            .filter(|(_, have)| have.as_str() == target_id)
            .map(|(session, _)| session.clone())
            .collect();
        for session in sessions {
            self.block(&session);
        }
    }

    fn answer_protected(&mut self, paused: &Paused) {
        self.settle(&paused.session_id, &paused.request_id);
        if paused.resource_type == "Document" {
            let _ = self.send(
                "Fetch.failRequest",
                json!({ "requestId": paused.request_id, "errorReason": "BlockedByClient" }),
                Some(&paused.session_id),
            );
        } else {
            self.continue_plain(&paused.session_id, &paused.request_id);
        }
    }

    fn continue_plain(&mut self, session_id: &str, request_id: &str) {
        self.settle(session_id, request_id);
        let _ = self.send(
            "Fetch.continueRequest",
            json!({ "requestId": request_id }),
            Some(session_id),
        );
    }

    fn watch(&mut self, value: &mut Value) {
        let method = value
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let session = value
            .get("sessionId")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        if let Some(id) = params.get("requestId").and_then(|v| v.as_str()) {
            if let Some(url) = shield::request_url(&params) {
                let initiator = ["documentURL"]
                    .iter()
                    .filter_map(|key| params.get(*key).and_then(|v| v.as_str()))
                    .chain(params.pointer("/initiator/url").and_then(|v| v.as_str()))
                    .any(|text| self.guard.url(text))
                    || params
                        .get("frameId")
                        .and_then(|v| v.as_str())
                        .is_some_and(|frame| self.frame_protected(frame));
                let protected = initiator
                    || self.guard.url(&url)
                    || self.guard.url(&shield::web_socket_as_http(&url));
                let entry = self.requests.entry(id.to_string()).or_insert(false);
                *entry = *entry || protected;
            }
        }
        match method.as_str() {
            "Page.frameNavigated" => {
                let Some(session) = session else { return };
                let frame = params.get("frame").cloned().unwrap_or(Value::Null);
                let url = format!(
                    "{}{}",
                    frame.get("url").and_then(|v| v.as_str()).unwrap_or(""),
                    frame.get("urlFragment").and_then(|v| v.as_str()).unwrap_or("")
                );
                let origin = frame
                    .get("securityOrigin")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if let Some(id) = frame.get("id").and_then(|v| v.as_str()) {
                    self.frames.insert(id.to_string(), vec![url.clone(), origin.clone()]);
                }
                if frame.get("parentId").is_none() && (self.guard.url(&url) || self.guard.url(&origin)) {
                    self.committed_protected(&session);
                }
            }
            "Runtime.executionContextCreated" => {
                let Some(session) = session else { return };
                let origin = params
                    .pointer("/context/origin")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let default = params
                    .pointer("/context/auxData/isDefault")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                if let Some(frame) = params.pointer("/context/auxData/frameId").and_then(|v| v.as_str()) {
                    if default {
                        self.frames
                            .entry(frame.to_string())
                            .or_default()
                            .push(origin.to_string());
                    }
                }
                if default && self.guard.url(origin) {
                    self.committed_protected(&session);
                }
            }
            "Target.targetCreated" | "Target.targetInfoChanged" => {
                let info = params.get("targetInfo").cloned().unwrap_or(Value::Null);
                let target = info
                    .get("targetId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let url = info.get("url").and_then(|v| v.as_str()).unwrap_or("");
                if !target.is_empty() {
                    self.targets.insert(target.clone(), info.clone());
                }
                if self.info_protected(&info, &[]) {
                    self.protect_target(&target);
                    value["params"]["targetInfo"] = shield::redacted(&info);
                } else if !self.guard.target(url, false, Opener::Unknown)
                    && !self.abandoned.contains(&target)
                {
                    self.protected_targets.remove(&target);
                }
            }
            "Target.targetDestroyed" => {
                if let Some(target) = params.get("targetId").and_then(|v| v.as_str()) {
                    self.guards.remove(target);
                    self.targets.remove(target);
                }
            }
            "Target.detachedFromTarget" => {
                if let Some(child) = params.get("sessionId").and_then(|v| v.as_str()) {
                    self.session_targets.remove(child);
                    self.guard_sessions.remove(child);
                    self.guards.retain(|_, session| session != child);
                }
            }
            _ => {}
        }
    }

    fn hand(&mut self, session_id: &str, request_id: &str) {
        self.handed
            .insert((session_id.to_string(), request_id.to_string()));
    }

    fn settle(&mut self, session_id: &str, request_id: &str) {
        self.handed
            .remove(&(session_id.to_string(), request_id.to_string()));
    }

    fn challenge_protected(&self, value: &Value) -> bool {
        let session = value.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        let id = params.get("requestId").and_then(|v| v.as_str()).unwrap_or("");
        let text = |pointer: &str| params.pointer(pointer).and_then(|v| v.as_str()).unwrap_or("");
        self.requests.get(id) == Some(&true)
            || self.blocked.contains(session)
            || self.guard_sessions.contains(session)
            || self.frame_protected(text("/frameId"))
            || self
                .session_target(session)
                .is_some_and(|target| self.protected_targets.contains(&target))
            || ["/request/url", "/authChallenge/origin"]
                .iter()
                .any(|pointer| self.guard.url(text(pointer)))
    }

    fn hidden(&self, value: &Value) -> bool {
        let method = value.get("method").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(session) = value.get("sessionId").and_then(|v| v.as_str()) {
            if self.blocked.contains(session) || self.guard_sessions.contains(session) {
                return true;
            }
        }
        if method.starts_with("Target.") {
            if let Some(child) = value.pointer("/params/sessionId").and_then(|v| v.as_str()) {
                if self.guard_sessions.contains(child) || self.blocked.contains(child) {
                    return true;
                }
            }
            return false;
        }
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        if let Some(id) = params.get("requestId").and_then(|v| v.as_str()) {
            if self.requests.get(id) == Some(&true) {
                return true;
            }
        }
        shield::mentions(&self.guard, &params)
    }

    fn deliver(&mut self, mut value: Value) {
        self.adopt_guard_attach(&value);
        self.watch(&mut value);
        if self.hidden(&value) {
            return;
        }
        let method = value.get("method").and_then(|v| v.as_str()).unwrap_or("");
        let id = value
            .pointer("/params/requestId")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let session = value.get("sessionId").and_then(|v| v.as_str());
        if let (true, Some(session), Some(id)) = (method.starts_with("Fetch."), session, &id) {
            let session = session.to_string();
            self.hand(&session, id);
        }
        if method.ends_with("ExtraInfo") {
            if let Some(id) = id.clone() {
                shield::strip_protected_associated(&self.guard, &mut value);
                if self.requests.get(&id) != Some(&false) {
                    self.held.entry(id).or_default().push(value);
                    return;
                }
            }
        }
        self.events.push(value);
        if let Some(id) = id {
            if self.requests.get(&id) == Some(&false) {
                if let Some(held) = self.held.remove(&id) {
                    self.events.extend(held);
                }
            }
        }
    }

    fn raw_call(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.send(method, params, session_id)?;
        let deadline = Instant::now() + timeout;
        let was_blocked = session_id.is_some_and(|session| self.blocked.contains(session));
        loop {
            let text = match self.ws.recv_text(deadline)? {
                Some(text) => text,
                None => return Err(format!("{method} timed out\n")),
            };
            let value: Value = serde_json::from_str(&text)
                .map_err(|err| format!("CDP response was not JSON: {err}\n"))?;
            if value.get("id").and_then(|item| item.as_i64()) == Some(id) {
                if let Some(error) = value.get("error") {
                    let text = error
                        .get("message")
                        .and_then(|item| item.as_str())
                        .unwrap_or("CDP error");
                    return Err(format!("{method}: {text}\n"));
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            let detached = value.get("method").and_then(|v| v.as_str())
                == Some("Target.detachedFromTarget")
                && value.pointer("/params/sessionId").and_then(|v| v.as_str()) == session_id;
            self.absorb(value);
            if let Some(session) = session_id {
                if detached || (!was_blocked && self.blocked.contains(session)) {
                    return Err(self.refusal());
                }
            }
        }
    }

    fn send(&mut self, method: &str, params: Value, session_id: Option<&str>) -> Result<i64, String> {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(session_id) = session_id {
            message["sessionId"] = json!(session_id);
        }
        self.ws.send_text(&message.to_string())?;
        Ok(id)
    }

    pub fn pump(&mut self, deadline: Instant) -> Result<bool, String> {
        match self.ws.recv_text(deadline)? {
            Some(text) => {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    self.absorb(value);
                }
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn next_paused(
        &mut self,
        session_id: Option<&str>,
        timeout: Duration,
    ) -> Result<Option<Paused>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let found = self
                .paused
                .iter()
                .position(|paused| session_id.is_none_or(|id| paused.session_id == id));
            if let Some(index) = found {
                return Ok(self.paused.remove(index));
            }
            if let Some(session) = session_id {
                if self.blocked.contains(session) {
                    return Err(self.refusal());
                }
            }
            if Instant::now() >= deadline || !self.pump(deadline)? {
                return Ok(None);
            }
        }
    }

    pub fn wait_event(
        &mut self,
        from: usize,
        timeout: Duration,
        mut matches: impl FnMut(&Value) -> bool,
    ) -> Result<Option<Value>, String> {
        let deadline = Instant::now() + timeout;
        let mut seen = from;
        loop {
            while seen < self.events.len() {
                if matches(&self.events[seen]) {
                    return Ok(Some(self.events[seen].clone()));
                }
                seen += 1;
            }
            if Instant::now() >= deadline || !self.pump(deadline)? {
                return Ok(None);
            }
        }
    }

    pub(crate) fn forget_paused(&mut self, session_id: &str) {
        self.paused.retain(|paused| paused.session_id != session_id);
        self.deciders.remove(session_id);
    }

    fn absorb(&mut self, value: Value) {
        if let Some(id) = value.get("id").and_then(|item| item.as_i64()) {
            if let Some((session, request)) = self.answering.remove(&id) {
                if value.get("error").is_none() {
                    self.settle(&session, &request);
                }
            }
            if let Some((session, request)) = self.decided.remove(&id) {
                if let Some(error) = value.get("error") {
                    let text = error
                        .get("message")
                        .and_then(|item| item.as_str())
                        .unwrap_or("CDP error");
                    self.fetch_errors.push(format!("Fetch: {text}\n"));
                    self.continue_plain(&session, &request);
                }
            }
            return;
        }
        if value.get("method").and_then(|item| item.as_str()) == Some("Fetch.authRequired")
            && self.challenge_protected(&value)
        {
            let id = value.pointer("/params/requestId").cloned().unwrap_or(Value::Null);
            if let Some(key) = id.as_str() {
                self.requests.insert(key.to_string(), true);
            }
            let session = value.get("sessionId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let _ = self.send(
                "Fetch.continueWithAuth",
                json!({ "requestId": id, "authChallengeResponse": { "response": "Default" } }),
                Some(&session),
            );
            return;
        }
        if value.get("method").and_then(|item| item.as_str()) == Some("Fetch.requestPaused") {
            let Some(paused) = Paused::from_event(&value) else {
                return;
            };
            let protected = fetch::reaches_protected(&self.guard, &paused);
            let initiated = self.initiated_by_protected(&paused);
            self.requests.insert(paused.request_id.clone(), protected || initiated);
            if let Some(network) = &paused.network_id {
                let entry = self.requests.entry(network.clone()).or_insert(false);
                *entry = *entry || protected || initiated;
            }
            #[cfg(test)]
            {
                let reason = if self.guard_sessions.contains(&paused.session_id) {
                    "guard session"
                } else if protected {
                    "reaches a protected origin"
                } else if self.blocked.contains(&paused.session_id) {
                    "blocked session"
                } else if initiated {
                    "initiated by protected"
                } else if self.deciders.contains_key(&paused.session_id) {
                    "decider"
                } else {
                    "queued"
                };
                self.paused_trail
                    .push((paused.session_id.clone(), paused.url.clone(), reason));
            }
            if self.guard_sessions.contains(&paused.session_id) {
                if protected && paused.resource_type == "Document" {
                    self.answer_protected(&paused);
                } else {
                    self.continue_plain(&paused.session_id, &paused.request_id);
                }
                return;
            }
            if protected {
                self.answer_protected(&paused);
                return;
            }
            if self.blocked.contains(&paused.session_id) || initiated {
                self.continue_plain(&paused.session_id, &paused.request_id);
                return;
            }
            self.hand(&paused.session_id, &paused.request_id);
            if self.deciders.contains_key(&paused.session_id) {
                self.decide_inline(paused);
            } else {
                self.paused.push_back(paused);
            }
            return;
        }
        if value.get("method").is_some() {
            self.deliver(value);
        }
    }

    fn decide_inline(&mut self, paused: Paused) {
        if self.blocked.contains(&paused.session_id) {
            self.answer_protected(&paused);
            return;
        }
        let Some(decider) = self.deciders.get_mut(&paused.session_id) else {
            self.paused.push_back(paused);
            return;
        };
        let decision = decider(&paused);
        let (method, params) = match fetch::command(&self.guard, &paused, &decision) {
            Ok(command) => command,
            Err(message) => {
                self.fetch_errors.push(message);
                fetch::command(&self.guard, &paused, &Decision::default())
                    .expect("a plain continue is always allowed")
            }
        };
        self.settle(&paused.session_id, &paused.request_id);
        if let Ok(id) = self.send(method, params, Some(&paused.session_id)) {
            self.decided
                .insert(id, (paused.session_id.clone(), paused.request_id.clone()));
        }
    }

    pub fn wait_load(&mut self, session_id: &str) -> Result<(), String> {
        if self.events.iter().any(|event| is_load(event, session_id)) {
            return Ok(());
        }
        let probe = self.call_for(
            "Runtime.evaluate",
            json!({
                "expression": "document.readyState",
                "returnByValue": true,
            }),
            Some(session_id),
            Duration::from_secs(1),
        );
        let settled = match &probe {
            Ok(result) => probe_is_settled(Ok(result
                .pointer("/result/value")
                .and_then(|value| value.as_str())))?,
            Err(err) => probe_is_settled(Err(err.as_str()))?,
        };
        if settled {
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if self.blocked.contains(session_id) {
                return Err(self.refusal());
            }
            if self.events.iter().any(|event| is_load(event, session_id)) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Ok(());
            }
            if !self.pump(deadline)? {
                return Ok(());
            }
        }
    }

    pub fn wait_load_since(&mut self, session_id: &str, from: usize) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if self.blocked.contains(session_id) {
                return Err(self.refusal());
            }
            if navigation_settled(&self.events, session_id, from) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Ok(());
            }
            if !self.pump(deadline)? {
                return Ok(());
            }
        }
    }
}

pub fn transport_error(message: &str) -> bool {
    message.starts_with("websocket ") || message.starts_with("CDP response was not JSON")
}

fn unguarded(message: String) -> String {
    if transport_error(&message) {
        message
    } else if let Some(step) = message.strip_suffix(" timed out\n") {
        format!("{}: {step} timed out\n", UNGUARDED.trim_end())
    } else {
        UNGUARDED.to_string()
    }
}

fn harmless(method: &str, params: &Value) -> bool {
    match method {
        "Fetch.disable" => true,
        "Fetch.continueRequest" => params
            .as_object()
            .is_some_and(|object| object.keys().all(|key| key == "requestId")),
        _ => false,
    }
}

fn probe_is_settled(outcome: Result<Option<&str>, &str>) -> Result<bool, String> {
    match outcome {
        Ok(Some("loading")) | Ok(Some("interactive")) => Ok(false),
        Ok(_) => Ok(true),
        Err(err)
            if err.starts_with("Runtime.evaluate timed out")
                || err.starts_with("Runtime.evaluate:") =>
        {
            Ok(true)
        }
        Err(err) => Err(err.to_string()),
    }
}

fn navigation_settled(events: &[Value], session_id: &str, from: usize) -> bool {
    let mut committed = false;
    for event in events.iter().skip(from) {
        if !committed && is_commit(event, session_id) {
            committed = true;
            continue;
        }
        if committed && is_load(event, session_id) {
            return true;
        }
    }
    false
}

fn is_commit(event: &Value, session_id: &str) -> bool {
    if event.get("sessionId").and_then(|value| value.as_str()) != Some(session_id) {
        return false;
    }
    if event.get("method").and_then(|value| value.as_str()) != Some("Page.frameNavigated") {
        return false;
    }
    let url = event
        .pointer("/params/frame/url")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    !url.is_empty() && !url.eq_ignore_ascii_case("about:blank")
}

fn is_load(event: &Value, session_id: &str) -> bool {
    if event.get("sessionId").and_then(|v| v.as_str()) != Some(session_id) {
        return false;
    }
    match event.get("method").and_then(|v| v.as_str()) {
        Some("Page.loadEventFired") => true,
        Some("Page.lifecycleEvent") => {
            event.pointer("/params/name").and_then(|v| v.as_str()) == Some("load")
        }
        _ => false,
    }
}

pub fn attach(cdp: &mut Cdp, target_id: &str) -> Result<String, String> {
    let result = cdp.call(
        "Target.attachToTarget",
        json!({ "targetId": target_id, "flatten": true }),
        None,
    )?;
    result
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| "Target.attachToTarget returned no sessionId\n".to_string())
}

pub fn detach(cdp: &mut Cdp, session_id: &str) {
    cdp.forget_paused(session_id);
    cdp.blocked.remove(session_id);
    cdp.session_targets.remove(session_id);
    let _ = cdp.call(
        "Target.detachFromTarget",
        json!({ "sessionId": session_id }),
        None,
    );
}

#[derive(Clone, Debug, Default)]
pub struct ActionNote {
    pub kind: String,
    pub tab: Option<String>,
    pub target: Option<String>,
    pub url: Option<String>,
    pub key: Option<String>,
    pub length: Option<u64>,
    pub method: Option<String>,
    pub agent: Option<String>,
}

impl ActionNote {
    pub fn start_params(&self) -> Value {
        let mut params = serde_json::Map::new();
        params.insert("phase".into(), json!("start"));
        params.insert("kind".into(), json!(self.kind));
        insert_text(&mut params, "tab", self.tab.as_deref());
        insert_text(&mut params, "target", self.target.as_deref());
        if let Some(url) = self.url.as_deref().and_then(public_url) {
            params.insert("url".into(), json!(url));
        }
        insert_text(&mut params, "key", self.key.as_deref());
        if let Some(length) = self.length {
            params.insert("length".into(), json!(length));
        }
        insert_text(&mut params, "method", self.method.as_deref());
        insert_text(&mut params, "agent", self.agent.as_deref());
        Value::Object(params)
    }

    pub fn end_params(&self, ok: bool, stderr: &str) -> Value {
        let mut params = serde_json::Map::new();
        params.insert("phase".into(), json!("end"));
        params.insert("ok".into(), json!(ok));
        if !ok {
            if let Some(error) = action_error(&self.kind, stderr) {
                params.insert("error".into(), json!(error));
            }
        }
        insert_text(&mut params, "tab", self.tab.as_deref());
        Value::Object(params)
    }
}

fn insert_text(params: &mut serde_json::Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        params.insert(key.to_string(), json!(value));
    }
}

pub fn first_stderr_line(stderr: &str) -> Option<&str> {
    stderr.lines().next().filter(|line| !line.is_empty())
}

fn public_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let without_fragment = raw.split_once('#').map(|(head, _)| head).unwrap_or(raw);
    let without_query = without_fragment
        .split_once('?')
        .map(|(head, _)| head)
        .unwrap_or(without_fragment);
    if without_query.is_empty() {
        return None;
    }
    if is_about_blank(without_query) {
        return Some("about:blank".to_string());
    }
    if let Some(rest) = without_query.strip_prefix("//") {
        let (host, path) = host_and_path(rest)?;
        return Some(format!("//{host}{path}"));
    }
    let (scheme, rest) = without_query.split_once(':')?;
    if !is_scheme(scheme) || !scheme_allowed(scheme) {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    let Some(authority) = rest.strip_prefix("//") else {
        return Some(format!("{scheme}:"));
    };
    let (host, path) = host_and_path(authority)?;
    Some(format!("{scheme}://{host}{path}"))
}

fn scheme_allowed(scheme: &str) -> bool {
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http"
            | "https"
            | "ws"
            | "wss"
            | "file"
            | "data"
            | "javascript"
            | "blob"
            | "about"
            | "chrome"
            | "chrome-extension"
            | "view-source"
            | "ftp"
    )
}

fn is_about_blank(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    scheme.eq_ignore_ascii_case("about") && rest.eq_ignore_ascii_case("blank")
}

fn is_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    match chars.next() {
        Some(ch) if ch.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '-' || ch == '.')
}

fn host_and_path(rest: &str) -> Option<(&str, &str)> {
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, ""),
    };
    let host = match authority.rfind('@') {
        Some(index) => &authority[index + 1..],
        None => authority,
    };
    if host.is_empty() { None } else { Some((host, path)) }
}

fn action_error(kind: &str, stderr: &str) -> Option<&'static str> {
    if kind == "eval" {
        return None;
    }
    let line = first_stderr_line(stderr).unwrap_or("").to_ascii_lowercase();
    if line.contains("timed out") {
        return Some("timeout");
    }
    if line.contains("websocket closed")
        || line.contains("websocket connection closed")
        || line.contains("websocket write failed")
        || line.contains("websocket read failed")
        || line.contains("connection reset")
        || line.contains("broken pipe")
    {
        return Some("connection lost");
    }
    if line.contains("no snapshot") || line.contains("likely stale") {
        return Some("no snapshot");
    }
    if line.contains("unknown ref")
        || line.contains("element not found")
        || line.contains("not an element ref")
        || line.contains("could not resolve")
        || line.contains("no backend node")
    {
        return Some("element not found");
    }
    if line.contains("no open tab is on") {
        return Some("not allowed");
    }
    if line.contains("unknown tab")
        || line.contains("--tab")
        || line.contains("no open tabs")
        || line.contains("explicit tab")
    {
        return Some("unknown tab");
    }
    if line.starts_with("navigate:") || line.contains("page.navigate") {
        return Some("navigation failed");
    }
    Some("failed")
}

pub fn slicc_agent() -> Option<String> {
    std::env::var("SLICC_AGENT")
        .ok()
        .filter(|value| !value.is_empty())
}

pub fn open(start: crate::connect::Start, runtime: Option<&str>) -> Result<Cdp, String> {
    let socket = match start {
        crate::connect::Start::Direct(url) => url,
        crate::connect::Start::Discover(url) => {
            let response = match crate::net::http_get(&url) {
                Ok(response) => response,
                Err(message) => {
                    if message.starts_with("HTTP ") || message.contains("does not launch Chrome") {
                        return Err(message);
                    }
                    return Err(format!(
                        "{message}This CLI does not launch Chrome. Pass --cdp or set SLICC_CDP_URL.\n"
                    ));
                }
            };
            let body = String::from_utf8_lossy(&response.body).to_string();
            crate::connect::websocket_url_from_version(&body)?
        }
    };
    let socket = crate::connect::append_runtime(&socket, runtime);
    Cdp::connect(&socket)
}

#[cfg(test)]
mod tests {
    use super::{first_stderr_line, probe_is_settled, ActionNote, Cdp, Transport};
    use serde_json::{json, Value};
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    struct Pending {
        inbox: VecDeque<String>,
        answer: fn(&Value) -> Vec<Value>,
    }

    impl Transport for Pending {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            let message: Value = serde_json::from_str(text).unwrap();
            for out in (self.answer)(&message) {
                self.inbox.push_back(out.to_string());
            }
            Ok(())
        }

        fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
            Ok(self.inbox.pop_front())
        }
    }

    fn pending(answer: fn(&Value) -> Vec<Value>) -> Cdp {
        Cdp::from_transport(Box::new(Pending {
            inbox: VecDeque::new(),
            answer,
        }))
    }

    #[test]
    fn privileged_target_types_and_extension_workers_are_guarded() {
        let mut cdp = pending(|message| {
            let info = match message["params"]["targetId"].as_str() {
                Some("UI") => json!({"targetId": "UI", "type": "browser_ui", "url": "https://example.test/"}),
                Some("W") => json!({"targetId": "W", "type": "service_worker", "url": "chrome-extension://abc/worker.js"}),
                Some("B") => json!({"targetId": "B", "type": "page", "url": "about:blank", "openerId": "UI"}),
                _ => json!({"targetId": "P", "type": "page", "url": "https://example.test/"}),
            };
            let result = match message["method"].as_str() {
                Some("Target.getTargetInfo") => json!({"targetInfo": info}),
                Some("Target.getTargets") => json!({"targetInfos": [
                    {"targetId": "UI", "type": "browser_ui", "url": "https://example.test/"},
                    {"targetId": "P", "type": "page", "url": "https://example.test/"}
                ]}),
                _ => json!({}),
            };
            vec![json!({"id": message["id"], "result": result})]
        });
        assert!(cdp.target_protected("B", false).unwrap());
        assert!(cdp.target_protected("UI", false).unwrap());
        assert!(cdp.target_protected("W", false).unwrap());
        assert!(!cdp.target_protected("P", false).unwrap());
        let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
        assert_eq!(listed["targetInfos"].as_array().unwrap().len(), 1);
        assert_eq!(listed["targetInfos"][0]["targetId"], "P");
        for kind in ["page", "service_worker", "background_page", "other", "iframe"] {
            assert!(cdp.info_protected(&json!({
                "targetId": "E", "type": kind, "url": "chrome-extension://abc/options.html"
            }), &[]));
        }
        assert!(cdp.info_protected(&json!({
            "targetId": "UI", "type": "browser_ui", "url": "about:blank"
        }), &[]));
    }

    fn evaluate(cdp: &mut Cdp) -> (Result<Value, String>, Duration) {
        let started = Instant::now();
        let result = cdp.call_for(
            "Runtime.evaluate",
            json!({ "expression": "1" }),
            Some("S"),
            Duration::from_secs(30),
        );
        (result, started.elapsed())
    }

    #[test]
    fn a_call_fails_with_the_refusal_once_the_guard_blocks_its_session() {
        let mut cdp = pending(|message| match message["method"].as_str() {
            Some("Runtime.evaluate") => vec![json!({
                "method": "Page.frameNavigated",
                "sessionId": "S",
                "params": { "frame": { "id": "F", "url": "https://seven.sliccy.ai/os/", "securityOrigin": "https://seven.sliccy.ai" } }
            })],
            _ => vec![],
        });
        let (result, spent) = evaluate(&mut cdp);
        assert_eq!(result.unwrap_err(), format!("{}\n", cdp.guard().refusal()));
        assert!(spent < Duration::from_secs(5), "{spent:?}");
        assert!(cdp.session_blocked("S"));
    }

    #[test]
    fn a_call_fails_with_the_refusal_once_its_session_detaches() {
        let mut cdp = pending(|_| {
            vec![
                json!({ "method": "Target.detachedFromTarget", "params": { "sessionId": "other" } }),
                json!({ "method": "Target.detachedFromTarget", "params": { "sessionId": "S" } }),
            ]
        });
        let (result, spent) = evaluate(&mut cdp);
        assert_eq!(result.unwrap_err(), format!("{}\n", cdp.guard().refusal()));
        assert!(spent < Duration::from_secs(5), "{spent:?}");
    }

    #[test]
    fn unrelated_events_do_not_end_a_call() {
        let mut cdp = pending(|message| {
            vec![
                json!({ "method": "Target.detachedFromTarget", "params": { "sessionId": "other" } }),
                json!({ "id": message["id"], "result": { "result": { "value": 1 } } }),
            ]
        });
        let (result, _) = evaluate(&mut cdp);
        assert_eq!(result.unwrap(), json!({ "result": { "value": 1 } }));
    }

    #[test]
    fn a_harmless_call_on_an_already_blocked_session_still_waits_for_its_reply() {
        let mut cdp = pending(|message| match message["method"].as_str() {
            Some("Runtime.evaluate") => vec![json!({
                "method": "Page.frameNavigated",
                "sessionId": "S",
                "params": { "frame": { "id": "F", "url": "https://seven.sliccy.ai/os/" } }
            })],
            Some("Fetch.disable") => vec![
                json!({ "method": "Page.lifecycleEvent", "sessionId": "S", "params": { "name": "load" } }),
                json!({ "id": message["id"], "result": {} }),
            ],
            _ => vec![],
        });
        let _ = evaluate(&mut cdp);
        assert!(cdp.session_blocked("S"));
        let disabled = cdp.call_for("Fetch.disable", json!({}), Some("S"), Duration::from_secs(30));
        assert_eq!(disabled.unwrap(), json!({}));
    }

    #[test]
    fn a_finished_document_does_not_wait_for_load() {
        assert_eq!(probe_is_settled(Ok(Some("complete"))).unwrap(), true);
        assert_eq!(probe_is_settled(Ok(None)).unwrap(), true);
        assert_eq!(
            probe_is_settled(Err("Runtime.evaluate timed out\n")).unwrap(),
            true
        );
        assert_eq!(
            probe_is_settled(Err("Runtime.evaluate: 'Runtime.evaluate' wasn't found\n")).unwrap(),
            true
        );
    }

    #[test]
    fn a_blank_load_does_not_settle_the_next_navigation() {
        let session = "sess";
        let blank_load = json!({
            "method": "Page.loadEventFired",
            "sessionId": session,
            "params": {}
        });
        let blank_frame = json!({
            "method": "Page.frameNavigated",
            "sessionId": session,
            "params": { "frame": { "url": "about:blank" } }
        });
        let commit = json!({
            "method": "Page.frameNavigated",
            "sessionId": session,
            "params": { "frame": { "url": "http://a.test/boot" } }
        });
        let load = json!({
            "method": "Page.loadEventFired",
            "sessionId": session,
            "params": {}
        });
        let raced = vec![blank_frame, blank_load.clone(), commit.clone()];
        assert!(!super::navigation_settled(&raced, session, 0));
        let ready = vec![blank_load.clone(), commit.clone(), load.clone()];
        assert!(super::navigation_settled(&ready, session, 0));
        let stale = vec![commit, load, blank_load];
        assert!(!super::navigation_settled(&stale, session, 2));
    }

    #[test]
    fn a_loading_document_still_waits() {
        assert_eq!(probe_is_settled(Ok(Some("loading"))).unwrap(), false);
        assert_eq!(probe_is_settled(Ok(Some("interactive"))).unwrap(), false);
    }

    #[test]
    fn a_closed_socket_is_not_treated_as_loaded() {
        for err in [
            "websocket closed 1011 the browser went away\n",
            "websocket connection closed\n",
            "websocket write failed: broken pipe\n",
            "websocket read failed: connection reset\n",
            "CDP response was not JSON: eof\n",
        ] {
            let reported = probe_is_settled(Err(err)).unwrap_err();
            assert_eq!(reported, err);
        }
    }

    #[test]
    fn action_start_and_end_keep_the_panel_fields() {
        let click = ActionNote {
            kind: "click".to_string(),
            tab: Some("1234".to_string()),
            target: Some("button \"Sign in\"".to_string()),
            agent: Some("cone:cone-1".to_string()),
            ..ActionNote::default()
        };
        assert_eq!(
            click.start_params(),
            json!({
                "phase": "start",
                "kind": "click",
                "tab": "1234",
                "target": "button \"Sign in\"",
                "agent": "cone:cone-1"
            })
        );
        let failed = click.end_params(
            false,
            "No snapshot available. Run \"snapshot\" first.\nignored\n",
        );
        assert_eq!(
            failed,
            json!({
                "phase": "end",
                "ok": false,
                "error": "no snapshot",
                "tab": "1234"
            })
        );
        assert!(failed.get("agent").is_none());
        assert!(failed.get("kind").is_none());
        assert!(failed.get("target").is_none());
        let open = ActionNote {
            kind: "open".to_string(),
            url: Some("http://a.test/".to_string()),
            ..ActionNote::default()
        };
        assert!(open.start_params().get("tab").is_none());
        assert_eq!(open.start_params()["url"], "http://a.test/");
        let mut opened = open.clone();
        opened.tab = Some("TAB1".to_string());
        assert_eq!(opened.end_params(true, "")["tab"], "TAB1");
        assert!(opened.end_params(true, "").get("error").is_none());
    }

    #[test]
    fn action_params_omit_typed_text_eval_source_and_request_secrets() {
        let secret = "secret-text";
        let fill = ActionNote {
            kind: "fill".to_string(),
            tab: Some("T".to_string()),
            target: Some("textbox \"Email\"".to_string()),
            length: Some(secret.chars().count() as u64),
            ..ActionNote::default()
        };
        let fill_params = fill.start_params();
        assert_eq!(fill_params["length"], 11);
        assert!(!fill_params.to_string().contains(secret));
        assert!(fill_params.get("value").is_none());
        let typed = "héllo";
        let type_note = ActionNote {
            kind: "type".to_string(),
            length: Some(typed.chars().count() as u64),
            ..ActionNote::default()
        };
        assert_eq!(type_note.start_params()["length"], 5);
        assert!(!type_note.start_params().to_string().contains(typed));
        let source = "document.cookie";
        let eval_note = ActionNote {
            kind: "eval".to_string(),
            tab: Some("T".to_string()),
            ..ActionNote::default()
        };
        let eval_params = eval_note.start_params();
        assert!(!eval_params.to_string().contains(source));
        assert!(eval_params.get("expression").is_none());
        let request = ActionNote {
            kind: "request".to_string(),
            tab: Some("T1".to_string()),
            method: Some("POST".to_string()),
            url: Some("https://app.example/api/me".to_string()),
            ..ActionNote::default()
        };
        let request_params = request.start_params();
        assert_eq!(
            request_params,
            json!({
                "phase": "start",
                "kind": "request",
                "tab": "T1",
                "method": "POST",
                "url": "https://app.example/api/me"
            })
        );
        assert!(!request_params.to_string().contains("Authorization"));
        assert!(!request_params.to_string().contains(secret));
        let press = ActionNote {
            kind: "press".to_string(),
            key: Some("Enter".to_string()),
            ..ActionNote::default()
        };
        assert_eq!(press.start_params()["key"], "Enter");
        assert!(ActionNote::default().start_params().get("agent").is_none());
        assert_eq!(first_stderr_line("only-line"), Some("only-line"));
        assert_eq!(first_stderr_line("\n"), None);
    }

    #[test]
    fn action_urls_and_errors_drop_page_secrets() {
        let secret = "https://user:pass@app.example/form?q=QUERYSECRET&tok=x#FRAGSECRET";
        let open = ActionNote {
            kind: "open".to_string(),
            url: Some(secret.to_string()),
            ..ActionNote::default()
        };
        let started = open.start_params();
        assert_eq!(started["url"], "https://app.example/form");
        assert_clean(&started);
        let goto = ActionNote {
            kind: "goto".to_string(),
            url: Some(secret.to_string()),
            ..ActionNote::default()
        };
        assert_eq!(goto.start_params()["url"], "https://app.example/form");
        let posted = ActionNote {
            kind: "request".to_string(),
            method: Some("POST".to_string()),
            url: Some(secret.to_string()),
            ..ActionNote::default()
        };
        let request = posted.start_params();
        assert_eq!(request["method"], "POST");
        assert_eq!(request["url"], "https://app.example/form");
        assert_clean(&request);
        let gotten = ActionNote {
            kind: "request".to_string(),
            method: Some("GET".to_string()),
            url: Some("http://user:p%40ss@[::1]:8443/a/b?x=1#y".to_string()),
            ..ActionNote::default()
        };
        assert_eq!(gotten.start_params()["url"], "http://[::1]:8443/a/b");
        let relative = ActionNote {
            kind: "request".to_string(),
            method: Some("GET".to_string()),
            url: Some("//user:pass@app.example/path?q=QUERYSECRET#FRAGSECRET".to_string()),
            ..ActionNote::default()
        };
        let relative_params = relative.start_params();
        assert_eq!(relative_params["url"], "//app.example/path");
        assert_clean(&relative_params);
        assert_eq!(
            ActionNote {
                kind: "open".to_string(),
                url: Some("about:blank".to_string()),
                ..ActionNote::default()
            }
            .start_params()["url"],
            "about:blank"
        );
        let opaque = [
            (
                "goto",
                "data:text/html,<title>DATASECRET</title>",
                Some("data:"),
                "DATASECRET",
            ),
            (
                "open",
                "data:text/html,DATASECRET2",
                Some("data:"),
                "DATASECRET2",
            ),
            (
                "goto",
                "javascript:void('JSSECRET')",
                Some("javascript:"),
                "JSSECRET",
            ),
            (
                "open",
                "blob:https://example.com/BLOBSECRET",
                Some("blob:"),
                "BLOBSECRET",
            ),
            ("goto", "about:blank?q=BLANKSECRET", Some("about:blank"), "BLANKSECRET"),
            ("open", "ABOUT:blank?q=BLANKSECRET", Some("about:blank"), "BLANKSECRET"),
            ("goto", "About:blank", Some("about:blank"), "BLANKSECRET"),
            (
                "open",
                "DATA:text/html,DATASECRET",
                Some("data:"),
                "DATASECRET",
            ),
            (
                "open",
                "about:srcdoc,ABOUTSECRET",
                Some("about:"),
                "ABOUTSECRET",
            ),
            ("goto", "file:/tmp/PATHSECRET", Some("file:"), "PATHSECRET"),
            (
                "open",
                "https:example.com/PATHSECRET",
                Some("https:"),
                "PATHSECRET",
            ),
            ("goto", "example.com/PATHSECRET", None, "PATHSECRET"),
            ("open", "not a url PATHSECRET", None, "PATHSECRET"),
            ("goto", "HOSTSECRET3:8080/path", None, "HOSTSECRET3"),
            ("open", "host:8080/x", None, "host:"),
            (
                "goto",
                "USERSECRET5:PASSSECRET5@127.0.0.1/form",
                None,
                "USERSECRET5",
            ),
            (
                "open",
                "USERSECRET5:PASSSECRET5@127.0.0.1/form",
                None,
                "PASSSECRET5",
            ),
            ("goto", "user:pw@host/x", None, "user:"),
            ("open", "foo:bar", None, "foo:"),
            ("goto", "foo://example.com/FOOSECRET", None, "FOOSECRET"),
            (
                "open",
                "view-source:https://example.com/VIEWSECRET",
                Some("view-source:"),
                "VIEWSECRET",
            ),
        ];
        for (kind, raw, expect, secret) in opaque {
            let note = ActionNote {
                kind: kind.to_string(),
                url: Some(raw.to_string()),
                ..ActionNote::default()
            };
            let params = note.start_params();
            match expect {
                Some(url) => assert_eq!(params["url"], url, "{raw}"),
                None => assert!(params.get("url").is_none(), "{raw} {params}"),
            }
            assert!(!params.to_string().contains(secret), "{raw} {params}");
        }
        let eval_failed = ActionNote {
            kind: "eval".to_string(),
            tab: Some("T".to_string()),
            ..ActionNote::default()
        };
        let eval_end = eval_failed.end_params(false, "Error: EVALSECRET-in-error\n");
        assert_eq!(eval_end["ok"], false);
        assert!(eval_end.get("error").is_none());
        assert_clean(&eval_end);
        let cases = [
            (
                "click",
                "Unknown ref \"e1\". Available: e2\n",
                "element not found",
            ),
            (
                "screenshot",
                "No snapshot available. Run \"snapshot\" first.\n",
                "no snapshot",
            ),
            (
                "screenshot",
                "screenshot: could not resolve element e1 to a visible box — the snapshot is likely stale\n",
                "no snapshot",
            ),
            (
                "click",
                "Error: --tab <targetId> is required. Run 'playwright-cli tab-list' to get tab IDs.\n",
                "unknown tab",
            ),
            (
                "goto",
                "navigate: refused https://app.example/form?q=QUERYSECRET#FRAGSECRET\n",
                "navigation failed",
            ),
            ("click", "Page.navigate timed out\n", "timeout"),
            (
                "open",
                "websocket closed 1011 the browser went away\n",
                "connection lost",
            ),
            (
                "request",
                "curlwright: no open tab is on https://app.example — pass --tab.\nhttps://other.example/dash?q=QUERYSECRET\n",
                "not allowed",
            ),
            (
                "request",
                "curlwright: (22) The requested URL returned error: 500\n",
                "failed",
            ),
        ];
        for (kind, stderr, code) in cases {
            let note = ActionNote {
                kind: kind.to_string(),
                ..ActionNote::default()
            };
            let ended = note.end_params(false, stderr);
            assert_eq!(ended["error"], code, "{stderr}");
            assert_clean(&ended);
        }
    }

    fn assert_clean(value: &serde_json::Value) {
        let rendered = value.to_string();
        for secret in [
            "QUERYSECRET",
            "FRAGSECRET",
            "EVALSECRET",
            "user:pass",
            "tok=x",
            "p%40ss",
        ] {
            assert!(!rendered.contains(secret), "{rendered}");
        }
    }
}
