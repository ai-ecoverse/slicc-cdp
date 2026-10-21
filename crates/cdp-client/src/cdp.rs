use crate::fetch::{self, Decision, Paused};
use crate::net::Ws;
use crate::protected::{Opener, Protected};
use crate::shield;
use crate::worlds::Worlds;
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
    redirects: BTreeSet<String>,
    seen_network: BTreeMap<(String, String), String>,
    terminal_network: BTreeSet<(String, String)>,
    request_aliases: BTreeMap<String, BTreeSet<String>>,
    request_resources: BTreeMap<String, BTreeSet<(String, String)>>,
    protected_resources: BTreeSet<(String, String)>,
    protected_target_resources: BTreeSet<(String, String)>,
    source_resources: BTreeMap<(String, String), String>,
    handed: BTreeSet<(String, String)>,
    answering: BTreeMap<i64, (String, String)>,
    held: BTreeMap<String, Vec<Value>>,
    streams: BTreeSet<String>,
    contexts: BTreeSet<String>,
    targets: BTreeMap<String, Value>,
    worlds: Worlds,
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
            redirects: BTreeSet::new(),
            seen_network: BTreeMap::new(),
            terminal_network: BTreeSet::new(),
            request_aliases: BTreeMap::new(),
            request_resources: BTreeMap::new(),
            protected_resources: BTreeSet::new(),
            protected_target_resources: BTreeSet::new(),
            source_resources: BTreeMap::new(),
            handed: BTreeSet::new(),
            answering: BTreeMap::new(),
            held: BTreeMap::new(),
            streams: BTreeSet::new(),
            contexts: BTreeSet::new(),
            targets: BTreeMap::new(),
            worlds: Worlds::default(),
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
        if matches!(method, "Network.loadNetworkResource" | "Page.captureSnapshot") {
            return Err(self.refusal());
        }
        if !shield::body_method(method) && self.content_refused(method, params, session_id.unwrap_or("")) {
            return Err(self.refusal());
        }
        if Worlds::selectors(params) || Worlds::sensitive(method) {
            let session = session_id.unwrap_or("");
            if !self.worlds.enabled(session) && (Worlds::sensitive(method) || self.worlds.reason(session, method, params).is_some()) {
                self.raw_call("Runtime.enable", json!({}), session_id, Duration::from_secs(10))?;
                self.worlds.enable(session);
            }
            if let Some(reason) = self.worlds.reason(session, method, params) {
                return Err(reason.into());
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
        if method == "Target.autoAttachRelated" || method == "Target.setAutoAttach"
            && params.get("autoAttach").and_then(|v| v.as_bool()) == Some(true)
        {
            return Err("slicc: refused: auto-attach is not supported by slicc-cdp\n".into());
        }
        if matches!(method, "Target.attachToTarget" | "Target.setAutoAttach")
            && params.get("flatten").and_then(|v| v.as_bool()) != Some(true)
        {
            return Err("slicc: refused: attach with flatten: true\n".into());
        }
        if let Some(reason) = shield::command_url_refusal(&self.guard, params, method) {
            if reason == self.guard.refusal() && shield::cookie_method(method) {
                return Err(COOKIES.into());
            }
            return Err(format!("{reason}\n"));
        }
        if method.starts_with("PWA.") {
            for key in ["manifestId", "installUrlOrBundleUrl"] {
                if let Some(url) = params.get(key).and_then(Value::as_str) {
                    if self.guard.url(url) {
                        return Err(format!("{}\n", self.guard.refusal_for_url(url)));
                    }
                }
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
                return Err(format!("{}\n", self.guard.refusal_for_url(&url)));
            }
        }
        if !matches!(method, "Target.getTargetInfo" | "Target.detachFromTarget") {
            for key in ["targetId", "frameId"] {
                if let Some(target) = params.get(key).and_then(|v| v.as_str()) {
                    let target = target.to_string();
                    if (key == "frameId" && self.frame_protected(&target)) || self.target_protected(&target, key == "frameId")? {
                        let url = self.targets.get(&target).and_then(|info| info.get("url")).and_then(Value::as_str).unwrap_or("");
                        return Err(format!("{}\n", self.guard.refusal_for_url(url)));
                    }
                }
            }
        }
        if shield::interception_method(method) {
            let id = shield::request_key(params).unwrap_or_default();
            let handed = session_id
                .is_some_and(|session| self.handed.contains(&(session.to_string(), id.clone())));
            if !handed || self.requests.get(&id) == Some(&true) {
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
        if self.redirects.contains(id) { Some(true) } else { self.requests.get(id).copied() }
    }

    fn after(
        &mut self,
        method: &str,
        params: &Value,
        session_id: Option<&str>,
        mut result: Value,
    ) -> Result<Value, String> {
        if self.content_refused(method, params, session_id.unwrap_or("")) {
            return Err(self.refusal());
        }
        if Worlds::sensitive(method) {
            if let Some(reason) = self.worlds.reason(session_id.unwrap_or(""), method, params) {
                return Err(reason.into());
            }
        }
        if method == "Runtime.enable" {
            self.worlds.enable(session_id.unwrap_or(""));
        } else if method == "Runtime.disable" {
            self.worlds.disable(session_id.unwrap_or(""));
        }
        self.worlds.reply(session_id.unwrap_or(""), method, &result);
        if method == "Page.createIsolatedWorld" {
            self.worlds.created(session_id.unwrap_or(""), &result);
        }
        if method == "Runtime.compileScript" {
            self.worlds.script(session_id.unwrap_or(""), &result);
        }
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
                    let url = info.get("url").and_then(Value::as_str).unwrap_or("");
                    if !(self.guard.target_type(info.get("type").and_then(Value::as_str).unwrap_or(""))
                        || self.guard.unparseable(url) && self.guard.url(url)) {
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
        shield::redact_strings(&self.guard, &mut result);
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
            Err(message) if message.starts_with("Target.getTargetInfo: slicc: refused:") => {
                self.protected_targets.insert(target_id.to_string());
                return Ok(true);
            }
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
        let mark = self.events.len();
        let result = self.raw_call("Target.createTarget", blank, None, timeout)?;
        let target = result
            .get("targetId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if let Err(message) = self.touch(&target) {
            let closed = self
                .raw_call(
                    "Target.closeTarget",
                    json!({ "targetId": target }),
                    None,
                    Duration::from_secs(5),
                )
                .and_then(|result| {
                    if result.get("success").and_then(Value::as_bool) != Some(true) {
                        return Err("Target.closeTarget did not confirm success\n".into());
                    }
                    self.wait_destroyed(&target, mark, Duration::from_secs(2))
                });
            return Err(match closed {
                Ok(()) => message,
                Err(error) => format!("{message}slicc: refused: cleanup: {}\n", error.trim()),
            });
        }
        if !url.eq_ignore_ascii_case("about:blank") {
            let guard = self.guards.get(&target).cloned();
            self.send("Page.navigate", json!({ "url": url }), guard.as_deref())?;
        }
        Ok(result)
    }

    fn wait_destroyed(&mut self, target: &str, mark: usize, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let destroyed = |events: &[Value]| {
            events[mark..].iter().any(|event| {
                event.get("method").and_then(Value::as_str) == Some("Target.targetDestroyed")
                    && event.pointer("/params/targetId").and_then(Value::as_str) == Some(target)
            })
        };
        loop {
            if destroyed(&self.events) {
                return Ok(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(
                    "Target.targetDestroyed timed out and Target.getTargets did not confirm closure\n".into()
                );
            }
            let listed = self.raw_call("Target.getTargets", json!({}), None, left);
            let absent = listed
                .as_ref()
                .ok()
                .and_then(|result| result.get("targetInfos"))
                .and_then(Value::as_array)
                .is_some_and(|infos| {
                    infos.iter().all(|info| info.get("targetId").and_then(Value::as_str).is_some())
                        && !infos.iter().any(|info| info.get("targetId").and_then(Value::as_str) == Some(target))
                });
            if destroyed(&self.events) || absent {
                return Ok(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            self.pump(Instant::now() + left.min(Duration::from_millis(100)))?;
        }
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
        self.fail_seen_network(&json!({"sessionId":paused.session_id,"params":{"networkId":paused.network_id,"requestId":paused.request_id}}));
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

    fn content_refused(&self, method: &str, params: &Value, session: &str) -> bool {
        if shield::body_method(method) && shield::request_key(params).is_some_and(|id| self.request_state(&id) == Some(true)) {
            return true;
        }
        if matches!(method, "Page.getResourceContent" | "Page.searchInResource") {
            return self.resource_protected(session, params.get("url").and_then(Value::as_str).unwrap_or(""));
        }
        if matches!(method, "CSS.getStyleSheetText" | "Debugger.getScriptSource" | "Debugger.getWasmBytecode" | "Debugger.searchInContent" | "Debugger.disassembleWasmModule") {
            let id = params.get("styleSheetId").or_else(|| params.get("scriptId")).and_then(Value::as_str).unwrap_or("");
            return self.source_resources.get(&(session.to_string(), id.to_string())).is_none_or(|url| self.resource_protected(session, url));
        }
        false
    }

    fn resource_url(url: &str) -> String {
        url::Url::parse(url).map(|mut url| { url.set_fragment(None); url.to_string() }).unwrap_or_else(|_| url.to_string())
    }

    fn protect_resource(&mut self, session: &str, url: &str) {
        let url = Self::resource_url(url);
        self.protected_resources.insert((session.to_string(), url.clone()));
        if let Some(target) = self.session_target(session) {
            self.protected_target_resources.insert((target, url));
        }
    }

    fn resource_protected(&self, session: &str, url: &str) -> bool {
        let key = Self::resource_url(url);
        self.guard.url(url) || self.protected_resources.contains(&(session.to_string(), key.clone()))
            || self.session_target(session).is_some_and(|target| self.protected_target_resources.contains(&(target, key)))
    }

    fn mark_redirect(&mut self, id: &str) {
        let mut ids = self.request_aliases.get(id).cloned().unwrap_or_default();
        ids.insert(id.to_string());
        for id in ids {
            self.redirects.insert(id.clone());
            for (session, url) in self.request_resources.get(&id).cloned().unwrap_or_default() {
                self.protect_resource(&session, &url);
            }
        }
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
                let resource = (session.clone().unwrap_or_default(), url.clone());
                self.request_resources.entry(id.to_string()).or_default().insert(resource.clone());
                if self.redirects.contains(id) || self.guard.url(&url) {
                    self.protect_resource(&resource.0, &resource.1);
                }
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
        if let Some(id) = params.get("requestId").and_then(Value::as_str) {
            if shield::scheme_less_protected(&self.guard, &params) {
                self.requests.insert(id.to_string(), true);
                self.mark_redirect(id);
            }
            for key in ["response", "redirectResponse", "headers", "headersText", "requestHeadersText", "responseHeaders"] {
                if params.get(key).is_some_and(|item| shield::redact_strings(&self.guard, &mut json!({key:item}))) {
                    self.mark_redirect(id);
                }
            }
        }
        if let Some((id, url)) = match method.as_str() {
            "Debugger.scriptParsed" | "Debugger.scriptFailedToParse" => params.get("scriptId").and_then(Value::as_str).zip(params.get("url").and_then(Value::as_str)),
            "CSS.styleSheetAdded" => params.pointer("/header/styleSheetId").and_then(Value::as_str).zip(params.pointer("/header/sourceURL").and_then(Value::as_str)),
            _ => None,
        } {
            self.source_resources.insert((session.clone().unwrap_or_default(), id.to_string()), url.to_string());
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
            "Target.attachedToTarget" => {
                if let Some(info) = params.get("targetInfo") {
                    value["params"]["targetInfo"] = self.redact(info, &[]);
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
        self.request_state(id) == Some(true)
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
        if method == "Target.receivedMessageFromTarget" {
            return true;
        }
        if let Some(session) = value.get("sessionId").and_then(|v| v.as_str()) {
            if self.blocked.contains(session) || self.guard_sessions.contains(session) {
                return true;
            }
        }
        if method.starts_with("Target.") {
            let info = value.pointer("/params/targetInfo");
            let reported = info.and_then(|info| info.get("targetId")).and_then(Value::as_str)
                .and_then(|target| self.targets.get(target)).or(info);
            if let Some(url) = reported.and_then(|info| info.get("url")).and_then(Value::as_str) {
                if self.guard.unparseable(url) && self.guard.url(url) {
                    return true;
                }
            }
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
        shield::loaded_urls_protected(&self.guard, &params)
    }

    fn fail_seen_network(&mut self, value: &Value) {
        let Some(id) = value.pointer("/params/networkId").and_then(Value::as_str).or_else(|| value.pointer("/params/requestId").and_then(Value::as_str)) else { return };
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let target = self.session_target(session);
        let keys: Vec<_> = self.seen_network.keys().filter(|(peer, request)| request == id && (peer == session || target.is_some() && self.session_target(peer) == target)).cloned().collect();
        for key in keys {
            let kind = self.seen_network.remove(&key).unwrap();
            if self.terminal_network.insert(key.clone()) {
                self.events.push(json!({"method":"Network.loadingFailed","sessionId":key.0,"params":{"requestId":key.1,"timestamp":value.pointer("/params/timestamp").and_then(Value::as_f64).unwrap_or(0.0),"type":kind,"errorText":"net::ERR_BLOCKED_BY_CLIENT","canceled":true,"blockedReason":"inspector"}}));
            }
        }
    }

    fn visible_network(&mut self, value: &Value) {
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        if !method.starts_with("Network.") { return }
        let Some(id) = value.pointer("/params/requestId").and_then(Value::as_str) else { return };
        let key = (value.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string(), id.to_string());
        if matches!(method, "Network.loadingFinished" | "Network.loadingFailed") {
            self.seen_network.remove(&key);
            self.terminal_network.insert(key);
        } else {
            let kind = value.pointer("/params/type").and_then(Value::as_str).filter(|kind| matches!(*kind, "Document" | "Stylesheet" | "Image" | "Media" | "Font" | "Script" | "TextTrack" | "XHR" | "Fetch" | "Prefetch" | "EventSource" | "WebSocket" | "Manifest" | "SignedExchange" | "Ping" | "CSPViolationReport" | "Preflight" | "Other")).unwrap_or("Other");
            self.seen_network.entry(key).or_insert_with(|| kind.to_string());
        }
    }

    fn visible_redirect(&self, value: &mut Value) -> bool {
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
        let id = value.pointer("/params/requestId").and_then(Value::as_str).unwrap_or("").to_string();
        if value.get("method").and_then(Value::as_str) != Some("Network.requestWillBeSent") || !self.seen_network.contains_key(&(session, id)) || !value.pointer("/params/redirectResponse/status").and_then(Value::as_u64).is_some_and(|status| matches!(status, 301 | 302 | 303 | 307 | 308)) {
            return false;
        }
        let params = &mut value["params"];
        let request = json!({"url":params.pointer("/request/url").cloned().unwrap_or(Value::Null),"method":params.pointer("/request/method").and_then(Value::as_str).filter(|method| matches!(*method, "GET" | "HEAD" | "POST" | "PUT" | "DELETE" | "CONNECT" | "OPTIONS" | "TRACE" | "PATCH")).unwrap_or("GET"),"headers":{}});
        let mut response = params["redirectResponse"].clone();
        if let Some(fields) = response.as_object_mut() { fields.retain(|key, _| matches!(key.as_str(), "url" | "status" | "connectionReused" | "connectionId" | "encodedDataLength" | "fromDiskCache" | "fromServiceWorker" | "fromPrefetchCache" | "timing" | "responseTime")); }
        response["statusText"] = json!("");
        response["headers"] = json!({});
        let location = params.pointer("/redirectResponse/headers").and_then(Value::as_object).and_then(|headers| headers.iter().find(|(key, _)| key.eq_ignore_ascii_case("Location"))).map(|(_, location)| location.clone());
        if let Some(location) = location { response["headers"] = json!({"Location":location}); }
        if let Some(fields) = params.as_object_mut() { fields.retain(|key, _| matches!(key.as_str(), "requestId" | "loaderId" | "frameId" | "timestamp" | "wallTime" | "type")); }
        params["request"] = request;
        params["redirectResponse"] = response;
        shield::redact_strings(&self.guard, value);
        true
    }

    fn deliver(&mut self, mut value: Value) {
        self.watch(&mut value);
        let key = (value.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string(), value.pointer("/params/requestId").and_then(Value::as_str).unwrap_or("").to_string());
        if value.get("method").and_then(Value::as_str).is_some_and(|method| method.starts_with("Network.")) && self.terminal_network.contains(&key) { return }
        let guarded = self.blocked.contains(&key.0) || shield::loaded_urls_protected(&self.guard, &value["params"]) || self.hidden(&value) || self.redirects.contains(&key.1);
        if guarded && self.visible_redirect(&mut value) {
            self.events.push(value.clone());
            self.fail_seen_network(&value);
            return;
        }
        if self.worlds.event(&value, &self.guard) {
            if value.get("method").and_then(Value::as_str) == Some("Debugger.paused") {
                let _ = self.send("Debugger.resume", json!({}), value.get("sessionId").and_then(Value::as_str));
            }
            return;
        }
        self.adopt_guard_attach(&value);
        if self.blocked.contains(value.get("sessionId").and_then(Value::as_str).unwrap_or("")) || !value.get("method").and_then(Value::as_str).unwrap_or("").starts_with("Target.") && shield::loaded_urls_protected(&self.guard, &value["params"]) {
            self.fail_seen_network(&value);
            return;
        }
        let redacted = shield::redact_strings(&self.guard, &mut value);
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        let tainted = value.pointer("/params/requestId").and_then(Value::as_str).is_some_and(|id| self.redirects.contains(id));
        if tainted {
            let request = value.pointer("/params/request");
            let opaque_request = request.and_then(Value::as_object).is_some_and(|request| request.keys().any(|key| key.starts_with("postData") || key.to_ascii_lowercase().ends_with("fragment")));
            let safe_request = method == "Network.requestWillBeSent" && request.and_then(|request| request.get("url")).and_then(Value::as_str).is_some_and(|url| !self.guard.url(url)) && !opaque_request;
            if opaque_request || !redacted && !safe_request {
                self.fail_seen_network(&value);
                return;
            }
            if let Some(params) = value.get_mut("params").and_then(Value::as_object_mut) {
                params.retain(|key, _| key != "documentURL" && !key.to_ascii_lowercase().ends_with("fragment"));
            }
        }
        if self.hidden(&value) {
            self.fail_seen_network(&value);
            return;
        }
        let method = value.get("method").and_then(|v| v.as_str()).unwrap_or("");
        self.worlds.visible_event(value.get("sessionId").and_then(Value::as_str).unwrap_or(""), method, &value["params"]);
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
        self.visible_network(&value);
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
                    let (text, _) = shield::redact_text(&self.guard, text);
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

    fn absorb(&mut self, mut value: Value) {
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
                    let (text, _) = shield::redact_text(&self.guard, text);
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
            let Some(mut paused) = Paused::from_event(&value) else {
                return;
            };
            if let Some(network) = &paused.network_id {
                self.request_aliases.entry(network.clone()).or_default().insert(paused.request_id.clone());
                self.request_aliases.entry(paused.request_id.clone()).or_default().insert(network.clone());
                if self.redirects.contains(network) {
                    self.mark_redirect(network);
                }
            }
            let authority_protected = shield::scheme_less_protected(&self.guard, &value["params"]);
            let reaches = fetch::reaches_protected(&self.guard, &paused) || authority_protected;
            let protected = reaches;
            let mut headers = json!({"headers":paused.headers,"responseHeaders":paused.response_headers});
            let redirect = shield::redact_strings(&self.guard, &mut headers);
            if redirect || reaches {
                self.mark_redirect(&paused.request_id);
                if let Some(network) = &paused.network_id {
                    self.mark_redirect(network);
                    self.protect_resource(&paused.session_id, &paused.url);
                }
            }
            if redirect {
                shield::redact_strings(&self.guard, &mut value);
                paused = Paused::from_event(&value).unwrap();
            }
            let initiated = self.initiated_by_protected(&paused);
            let loaded_protected = self.guard.url(&paused.full_url()) || authority_protected;
            self.requests.insert(paused.request_id.clone(), loaded_protected || initiated);
            if let Some(network) = &paused.network_id {
                let entry = self.requests.entry(network.clone()).or_insert(false);
                *entry = *entry || loaded_protected || initiated;
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
                if reaches && paused.resource_type == "Document" {
                    self.answer_protected(&paused);
                } else {
                    self.continue_plain(&paused.session_id, &paused.request_id);
                }
                return;
            }
            if protected {
                if redirect && !authority_protected && !self.guard.url(&paused.full_url()) && !self.blocked.contains(&paused.session_id) {
                    if let Some(request) = value.pointer_mut("/params/request").and_then(Value::as_object_mut) {
                        request.retain(|key, _| !key.starts_with("postData") && !key.to_ascii_lowercase().ends_with("fragment"));
                    }
                    self.events.push(value);
                }
                self.answer_protected(&paused);
                return;
            }
            if self.blocked.contains(&paused.session_id) || initiated {
                self.continue_plain(&paused.session_id, &paused.request_id);
                return;
            }
            if self.redirects.contains(&paused.request_id) && !redirect && !protected {
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

    include!("redirect_test.rs");
    include!("redirect_matrix_test.rs");
    include!("redirect_race_test.rs");
    include!("redirect_terminal_test.rs");
    include!("redirect_fragment_test.rs");
    include!("redirect_review_test.rs");
    include!("redirect_relation_test.rs");

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

    struct Cleanup {
        inner: Pending,
        destroy_after: Option<Duration>,
        destroyed_at: Option<Instant>,
        closed: bool,
    }

    impl Transport for Cleanup {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            self.inner.send_text(text)?;
            let message: Value = serde_json::from_str(text).unwrap();
            if message["method"] == "Target.closeTarget" {
                self.closed = true;
                self.destroyed_at = self.destroy_after.map(|delay| Instant::now() + delay);
            }
            Ok(())
        }

        fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
            if let Some(message) = self.inner.inbox.pop_front() {
                return Ok(Some(message));
            }
            if let Some(at) = self.destroyed_at.take() {
                if at <= deadline {
                    std::thread::sleep(at.saturating_duration_since(Instant::now()));
                    return Ok(Some(json!({
                        "method": "Target.targetDestroyed", "params": { "targetId": "T" }
                    }).to_string()));
                }
                self.destroyed_at = Some(at);
            }
            if self.closed {
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            }
            Ok(None)
        }
    }

    fn target_fragment_markers() -> [&'static str; 5] {
        [
            "https://seven.sliccy.ai/#tailscale=MARK1",
            "https://seven.sliccy.ai/#proxy=http%3A%2F%2F127.0.0.1%3A9%2F&key=MARK2",
            "https://seven.sliccy.ai/auth/callback#access_token=MARK",
            "https://seven.sliccy.ai/path-secret?query-secret=MARK",
            "https://seven.sliccy.ai/path-secret?query-secret=MARK#access_token=MARK",
        ]
    }

    #[test]
    fn target_info_fragment_markers_are_redacted_in_responses_and_events() {
        for url in target_fragment_markers() {
            for changed in [false, true] {
                let mut cdp = url_backend(url);
                if changed {
                    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":"https://seven.sliccy.ai/","title":"clean"}}}));
                }
                let info = json!({"targetId":"T","type":"page","url":url,"title":url,"openerId":"OPENER","openerFrameId":"FRAME"});
                for method in ["Target.targetCreated", "Target.targetInfoChanged", "Target.attachedToTarget"] {
                    let mark = cdp.events().len();
                    cdp.absorb(json!({"method":method,"params":{"sessionId":"A","targetInfo":info}}));
                    let event = cdp.events()[mark..].iter().find(|event| event["method"] == method).unwrap();
                    for key in ["url", "title"] {
                        assert_eq!(event["params"]["targetInfo"][key], "https://seven.sliccy.ai", "{method} {key} {url} {changed}");
                    }
                    assert_eq!(event["params"]["targetInfo"]["openerId"], "OPENER");
                    assert_eq!(event["params"]["targetInfo"]["openerFrameId"], "FRAME");
                    assert!(!event.to_string().contains("MARK"));
                }
                for method in ["Target.getTargets", "Target.getTargetInfo"] {
                    let params = if method == "Target.getTargets" { json!({}) } else { json!({"targetId":"T"}) };
                    let out = cdp.call(method, params, None).unwrap();
                    let info = if method == "Target.getTargets" { &out["targetInfos"][0] } else { &out["targetInfo"] };
                    for key in ["url", "title"] {
                        assert_eq!(info[key], "https://seven.sliccy.ai", "{method} {key} {url}");
                    }
                    assert!(!out.to_string().contains("MARK"));
                }
                for event in [
                    json!({"method":"Target.detachedFromTarget","params":{"sessionId":"A","targetId":"T"}}),
                    json!({"method":"Target.detachedFromTarget","params":{"sessionId":"B"}}),
                    json!({"method":"Page.frameAttached","sessionId":"S","params":{"frameId":"CHILD","parentFrameId":"FRAME"}}),
                ] {
                    cdp.absorb(event);
                    assert!(!cdp.events().last().unwrap().to_string().contains("MARK"));
                }
            }
        }
    }

    #[test]
    fn target_info_nonflattened_and_browser_attach_paths_remain_refused() {
        for (method, params) in [
            ("Target.attachToTarget", json!({"targetId":"T","flatten":false})),
            ("Target.attachToBrowserTarget", json!({})),
            ("Target.setAutoAttach", json!({"autoAttach":true,"flatten":true})),
            ("Target.autoAttachRelated", json!({"targetId":"T","waitForDebuggerOnStart":false})),
            ("Target.sendMessageToTarget", json!({"targetId":"T","message":"MARK"})),
        ] {
            let mut cdp = url_backend("https://example.test/");
            assert!(cdp.call(method, params, None).is_err(), "{method}");
        }
    }

    #[test]
    fn target_info_wrapped_nonflattened_events_never_disclose_markers() {
        for url in target_fragment_markers() {
            let mut cdp = url_backend("https://example.test/");
            let mark = cdp.events().len();
            let nested = json!({"method":"Target.attachedToTarget","params":{"sessionId":"CHILD","targetInfo":{"targetId":"T","type":"page","url":url,"title":url}}});
            cdp.absorb(json!({"method":"Target.receivedMessageFromTarget","params":{"sessionId":"A","targetId":"T","message":nested.to_string()}}));
            assert_eq!(cdp.events().len(), mark, "{url}");
        }
    }

    const UNPARSEABLE: &str = "slicc: refused: slicc can't parse this URL, so it could be a protected page\n";

    fn malformed_urls() -> Vec<String> {
        let mut urls = [
            "http://exa%20mple.sliccy.ai/",
            "http://exa mple.sliccy.ai/",
            "http://\u{200d}.sliccy.ai/",
            "https://exa%20mple.example/",
            "http://xn--1ug.sliccy.ai/",
            "http://xn--a.sliccy.ai/",
            "view-source:http://exa%20mple.sliccy.ai/",
            "blob:http://exa%20mple.sliccy.ai/id",
            "filesystem:http://exa%20mple.sliccy.ai/temporary/x",
        ].map(str::to_string).to_vec();
        urls.push(format!("{}https://example.test/", "view-source:".repeat(9)));
        urls
    }

    struct UrlBackend {
        url: String,
        inbox: VecDeque<String>,
    }

    impl Transport for UrlBackend {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            let message: Value = serde_json::from_str(text).unwrap();
            let info = json!({"targetId":"T","type":"page","url":self.url,"title":"SECRET"});
            let result = match message["method"].as_str().unwrap() {
                "Target.getTargetInfo" => json!({"targetInfo":info}),
                "Target.getTargets" => json!({"targetInfos":[info]}),
                "Target.attachToTarget" => json!({"sessionId":"S"}),
                "Target.createTarget" => json!({"targetId":"T"}),
                "Page.getNavigationHistory" => json!({"entries":[{"id":1,"url":self.url}]}),
                _ => json!({}),
            };
            self.inbox.push_back(json!({"id":message["id"],"result":result}).to_string());
            Ok(())
        }

        fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
            Ok(self.inbox.pop_front())
        }
    }

    fn url_backend(url: &str) -> Cdp {
        Cdp::from_transport(Box::new(UrlBackend { url: url.into(), inbox: VecDeque::new() }))
    }

    #[test]
    fn issue_401_command_destinations_fail_closed() {
        let mut failures = Vec::new();
        for url in malformed_urls() {
            for method in ["Target.createTarget", "Page.navigate", "Network.loadNetworkResource", "Fetch.continueRequest", "Network.continueInterceptedRequest", "PWA.install", "PWA.launch"] {
                let mut cdp = url_backend("about:blank");
                cdp.hand("S", "R");
                cdp.requests.insert("R".into(), false);
                let params = if method.starts_with("PWA.") {
                    json!({"manifestId":url,"installUrlOrBundleUrl":url})
                } else {
                    json!({"url":url,"requestId":"R","interceptionId":"R"})
                };
                let result = cdp.call(method, params, Some("S"));
                if !result.as_ref().err().is_some_and(|error| error.starts_with("slicc: refused:")) {
                    failures.push(format!("{method} {url}: {result:?}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn issue_401_normalized_protected_hosts_are_guarded() {
        for url in [
            "http://vault%2Esliccy.ai/",
            "http://vau%6Ct.sliccy.ai/",
            "http://VAULT.sliccy.ai/",
            "http://vault.sliccy.ai./",
            "http://seven.sliccy.ai./",
        ] {
            for (method, params) in [
                ("Target.createTarget", json!({"url":url})),
                ("Page.navigate", json!({"url":url})),
                ("Network.setCookie", json!({"url":url,"name":"x","value":"y"})),
                ("Network.deleteCookies", json!({"url":url,"name":"x"})),
                ("Network.setCookies", json!({"cookies":[{"url":url,"name":"x","value":"y"}]})),
                ("Storage.setCookies", json!({"cookies":[{"url":url,"name":"x","value":"y"}]})),
            ] {
                assert!(url_backend("about:blank").call(method, params, Some("S")).unwrap_err().starts_with("slicc: refused:"), "{method} {url}");
            }
            let mut cdp = url_backend(url);
            let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
            assert_ne!(listed["targetInfos"][0]["url"], url);
            assert!(cdp.is_protected_target("T"));
            assert!(url_backend(url).call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).is_err());
        }
    }

    #[test]
    fn issue_401_refusal_text_is_exact() {
        for method in ["Page.navigate", "Target.createTarget", "Network.setCookie", "Network.getCookies"] {
            let url = "http://exa%20mple.sliccy.ai/";
            let params = if method == "Network.getCookies" { json!({"urls":[url]}) } else { json!({"url":url,"name":"x","value":"y"}) };
            assert_eq!(url_backend("about:blank").call(method, params, Some("S")).unwrap_err(), UNPARSEABLE, "{method}");
        }
    }

    #[test]
    fn issue_401_reported_targets_and_history_fail_closed() {
        for url in malformed_urls() {
            let mut cdp = url_backend(&url);
            let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
            assert!(listed["targetInfos"].as_array().unwrap().is_empty(), "{url}");
            assert!(cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).unwrap_err().starts_with("slicc: refused:"), "{url}");
            assert!(url_backend(&url).call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).is_err(), "{url}");
            assert!(url_backend(&url).call("Page.navigateToHistoryEntry", json!({"entryId":1}), Some("S")).is_err(), "{url}");
        }
    }

    #[test]
    fn cookie_readers_keep_filtering_parseable_mixed_url_lists() {
        assert!(url_backend("about:blank").call("Network.getCookies", json!({"urls":["https://seven.sliccy.ai/", "https://example.test/"]}), Some("S")).is_ok());
        assert_eq!(url_backend("about:blank").call("Network.getCookies", json!({"urls":["http://exa%20mple.sliccy.ai/", "https://example.test/"]}), Some("S")).unwrap_err(), UNPARSEABLE);
    }

    #[test]
    fn issue_401_cookie_command_urls_fail_closed() {
        let mut failures = Vec::new();
        for url in malformed_urls() {
            for (method, params) in [
                ("Network.setCookie", json!({"url":url,"name":"x","value":"y"})),
                ("Network.deleteCookies", json!({"url":url,"name":"x"})),
                ("Network.getCookies", json!({"urls":["https://example.test/",url]})),
                ("Network.setCookies", json!({"cookies":[{"url":url,"name":"x","value":"y"}]})),
                ("Storage.setCookies", json!({"cookies":[{"url":url,"name":"x","value":"y"}]})),
            ] {
                if url_backend("about:blank").call(method, params, Some("S")).is_ok() {
                    failures.push(format!("{method} {url}"));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    #[test]
    fn issue_401_loaded_urls_are_never_exposed() {
        for url in malformed_urls() {
            for event in [
                json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"https://example.test/","securityOrigin":url}}}),
                json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":url,"securityOrigin":"null"}}}),
                json!({"method":"Runtime.executionContextCreated","sessionId":"S","params":{"context":{"id":1,"origin":url,"auxData":{"isDefault":true}}}}),
                json!({"method":"Runtime.executionContextCreated","sessionId":"S","params":{"context":{"id":1,"origin":url,"auxData":{"isDefault":false}}}}),
                json!({"method":"Network.requestWillBeSent","sessionId":"S","params":{"requestId":"R","request":{"url":url}}}),
                json!({"method":"Network.webSocketCreated","sessionId":"S","params":{"requestId":"R","url":url.replacen("http:","ws:",1)}}),
                json!({"method":"ServiceWorker.workerVersionUpdated","params":{"versions":[{"scriptURL":url,"status":"activated"}]}}),
            ] {
                let mut cdp = url_backend("about:blank");
                cdp.absorb(event.clone());
                assert!(cdp.events().is_empty(), "{event}: {:?}", cdp.events());
                if event["method"] == "Page.frameNavigated" || event.pointer("/params/context/auxData/isDefault") == Some(&json!(true)) {
                    assert!(cdp.blocked.contains("S"), "{event}");
                }
            }
            for kind in ["page", "service_worker", "shared_worker", "iframe"] {
                let mut cdp = url_backend("about:blank");
                cdp.absorb(json!({"method":"Target.targetCreated","params":{"targetInfo":{"targetId":"T","type":kind,"url":url,"title":"SECRET"}}}));
                assert!(cdp.is_protected_target("T"), "{kind} {url}");
                assert!(cdp.events().is_empty(), "{kind} {url}");
            }
        }
        for origin in ["", "null", "://"] {
            let mut cdp = url_backend("about:blank");
            cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"about:blank","securityOrigin":origin}}}));
            assert!(!cdp.blocked.contains("S"));
            assert_eq!(cdp.events().len(), 1);
        }
    }

    #[test]
    fn issue_401_exemptions_and_read_only_keys_remain_open() {
        for url in ["", "null", "://"] {
            assert!(url_backend("about:blank").call("Target.createTarget", json!({"url":url}), None).is_ok(), "{url}");
            assert!(url_backend("about:blank").call("Page.navigate", json!({"url":url}), Some("S")).is_ok(), "{url}");
        }
        for url in malformed_urls() {
            let mut cdp = url_backend("about:blank");
            assert!(cdp.notify("Slicc.action", json!({"url":url,"phase":"start"}), None).is_ok(), "{url}");
            assert!(cdp.call("Storage.getUsageAndQuota", json!({"origin":url}), None).is_ok(), "{url}");
            assert!(cdp.call("Storage.clearDataForStorageKey", json!({"storageKey":format!("{url}^0{url}")}), None).is_ok(), "{url}");
        }
    }

    fn cleanup_answer(message: &Value) -> Vec<Value> {
        let result = match message["method"].as_str() {
            Some("Target.createTarget") => json!({ "targetId": "T" }),
            Some("Target.getTargetInfo") => json!({ "targetInfo": {
                "targetId": "T", "type": "page", "url": "about:blank"
            } }),
            Some("Target.getTargets") => json!({ "targetInfos": [
                { "targetId": "T", "type": "page", "url": "about:blank" }
            ] }),
            Some("Target.attachToTarget") => json!({ "sessionId": "S" }),
            Some("Network.setBypassServiceWorker") => return Vec::new(),
            Some("Target.closeTarget") => return vec![
                json!({ "method": "Target.targetDestroyed", "params": { "targetId": "OTHER" } }),
                json!({ "id": message["id"], "result": { "success": true } }),
            ],
            _ => json!({}),
        };
        vec![json!({ "id": message["id"], "result": result })]
    }

    fn cleanup(delay: Option<Duration>, answer: fn(&Value) -> Vec<Value>) -> Cdp {
        Cdp::from_transport(Box::new(Cleanup {
            inner: Pending { inbox: VecDeque::new(), answer },
            destroy_after: delay,
            destroyed_at: None,
            closed: false,
        }))
    }

    #[test]
    fn cleanup_waits_for_the_refused_targets_late_destruction() {
        let mut cdp = cleanup(Some(Duration::from_millis(40)), cleanup_answer);
        let error = cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None).unwrap_err();
        assert_eq!(error, "slicc: refused: slicc can't keep seven's origin out of this target: Network.setBypassServiceWorker timed out\n");
        assert!(cdp.events().iter().any(|event| event["method"] == "Target.targetDestroyed" && event["params"]["targetId"] == "T"));
        assert!(cdp.is_protected_target("T"));
    }

    #[test]
    fn cleanup_confirms_closure_without_browser_events() {
        let mut cdp = cleanup(None, |message| {
            if message["method"] == "Target.getTargets" {
                return vec![json!({ "id": message["id"], "result": { "targetInfos": [
                    { "targetId": "OTHER", "type": "page", "url": "about:blank" }
                ] } })];
            }
            cleanup_answer(message).into_iter().filter(|reply| reply.get("method").is_none()).collect()
        });
        let error = cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None).unwrap_err();
        assert_eq!(error, "slicc: refused: slicc can't keep seven's origin out of this target: Network.setBypassServiceWorker timed out\n");
        assert!(!cdp.events().iter().any(|event| event["method"] == "Target.targetDestroyed"));
        assert!(cdp.is_protected_target("T"));
    }

    #[test]
    fn cleanup_reports_a_missing_destruction_event_without_allowing_the_target() {
        let mut cdp = cleanup(None, cleanup_answer);
        let error = cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None).unwrap_err();
        assert!(error.starts_with("slicc: refused:"), "{error}");
        assert!(error.contains("Network.setBypassServiceWorker timed out"), "{error}");
        assert!(error.contains("Target.targetDestroyed timed out"), "{error}");
        assert!(cdp.is_protected_target("T"));
    }

    #[test]
    fn cleanup_reports_a_close_that_did_not_succeed() {
        let mut cdp = cleanup(None, |message| {
            let mut replies = cleanup_answer(message);
            if message["method"] == "Target.closeTarget" {
                replies.last_mut().unwrap()["result"] = json!({ "success": false });
            }
            replies
        });
        let error = cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None).unwrap_err();
        assert!(error.contains("Network.setBypassServiceWorker timed out"), "{error}");
        assert!(error.contains("cleanup: Target.closeTarget did not confirm success"), "{error}");
        assert!(cdp.is_protected_target("T"));
    }

    #[test]
    fn cleanup_does_not_accept_a_stale_destruction_event() {
        let mut cdp = cleanup(None, cleanup_answer);
        cdp.absorb(json!({ "method": "Target.targetDestroyed", "params": { "targetId": "T" } }));
        let error = cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None).unwrap_err();
        assert!(error.contains("Target.targetDestroyed timed out"), "{error}");
        assert!(cdp.is_protected_target("T"));
    }

    #[test]
    fn cleanup_accepts_destruction_before_the_close_reply() {
        let mut cdp = cleanup(None, |message| {
            let mut replies = cleanup_answer(message);
            if message["method"] == "Target.closeTarget" {
                replies.insert(0, json!({ "method": "Target.targetDestroyed", "params": { "targetId": "T" } }));
            }
            replies
        });
        let error = cdp.call("Target.createTarget", json!({ "url": "about:blank" }), None).unwrap_err();
        assert!(error.contains("Network.setBypassServiceWorker timed out"), "{error}");
        assert!(!error.contains("cleanup:"), "{error}");
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
