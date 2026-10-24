use crate::fetch::{self, Decision, Paused};
use crate::net::Ws;
use crate::protected::{Opener, Protected};
use crate::shield;
use crate::worlds::Worlds;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

#[path = "webmcp_guard.rs"]
mod webmcp_guard;
#[path = "page_scope.rs"]
mod page_scope;
#[path = "issued.rs"]
mod issued;
pub(crate) use issued::target_refusal;

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
const DOCUMENT_CHECK: Duration = Duration::from_secs(2);
const WAITER_SLICE: Duration = Duration::from_millis(5);
const AUTO_ATTACH: &str = "slicc: refused: auto-attach is not supported by slicc-cdp\n";
const FLATTEN: &str = "slicc: refused: attach with flatten: true\n";
const PERSISTENT: &str = "slicc: refused: persistent page capabilities are not supported by slicc-cdp\n";
pub const COOKIES: &str = "slicc: refused: cookies on seven's origin\n";
pub const STREAM: &str = "slicc: refused: slicc didn't hand out this stream, so it could be seven's\n";

struct CarrierTarget {
    origin: Option<String>,
    protected: bool,
    url: url::Url,
}

#[derive(Clone, Debug)]
struct DocumentState {
    frames: BTreeMap<String, Vec<String>>,
    worker: Option<(String, String)>,
    scope: Option<String>,
    epoch: u64,
    page_scope: Option<(String, u64, u64)>,
    runtime_scope_epoch: Option<u64>,
}

impl PartialEq for DocumentState {
    fn eq(&self, other: &Self) -> bool {
        self.scope == other.scope && self.epoch == other.epoch && self.worker == other.worker && self.frames.len() == other.frames.len()
            && self.frames.iter().all(|(id, frame)| other.frames.get(id).is_some_and(|current| {
                frame[0] == current[0] && frame[2..] == current[2..] && (!frame[0].is_empty() || frame[1] == current[1])
            }))
    }
}

#[derive(Clone)]
struct ContextProof {
    id: i64,
    frame: String,
    origin: String,
    name: String,
    loader: Option<String>,
    default: bool,
}

struct Navigation {
    params: Value,
    session: Option<String>,
    reply: Option<Value>,
    detached: bool,
}

struct MainDocument {
    frame_id: String,
    url: String,
    origin: String,
    unreachable_url: String,
    navigating: bool,
    navigation_url: String,
    previous: bool,
    probed: bool,
}

pub type Waiter = Box<dyn FnMut(&Cdp)>;

pub struct Cdp {
    ws: Box<dyn Transport>,
    next_id: i64,
    detaching: BTreeMap<i64, String>,
    detached_ordinary: BTreeSet<String>,
    navigations: BTreeMap<i64, Navigation>,
    events: Vec<Value>,
    dialog_sessions: BTreeSet<String>,
    dialogs: BTreeMap<String, Dialog>,
    dialog_documents: BTreeMap<String, DialogDocument>,
    dialog_epoch: u64,
    trace_cookies: Vec<Value>,
    guard: Protected,
    paused: VecDeque<Paused>,
    deciders: BTreeMap<String, Decider>,
    decided: BTreeMap<i64, (String, String)>,
    fetch_errors: Vec<String>,
    blocked: BTreeSet<String>,
    session_targets: BTreeMap<String, String>,
    protected_targets: BTreeSet<String>,
    guards: BTreeMap<String, String>,
    page_scope_sessions: BTreeMap<String, String>,
    checking_page_events: bool,
    pending_page_events: VecDeque<Value>,
    runtime_scope_epochs: BTreeMap<String, u64>,
    page_scope_owners: BTreeMap<String, String>,
    page_scope_epochs: BTreeMap<String, u64>,
    page_scope_unknown: u64,
    guard_sessions: BTreeSet<String>,
    guarding: BTreeSet<String>,
    probing: BTreeSet<String>,
    abandoned: BTreeSet<String>,
    unguarded_optional: BTreeMap<String, BTreeSet<String>>,
    notices: Vec<String>,
    discovering: bool,
    requests: BTreeMap<String, bool>,
    redirects: BTreeSet<String>,
    protected_body_requests: BTreeSet<String>,
    seen_network: BTreeMap<(String, String), String>,
    terminal_network: BTreeSet<(String, String)>,
    client_navigation_urls: BTreeMap<String, String>,
    request_aliases: BTreeMap<String, BTreeSet<String>>,
    request_resources: BTreeMap<String, BTreeSet<(String, String)>>,
    protected_request_pages: BTreeSet<(String, String)>,
    protected_resources: BTreeSet<(String, String)>,
    protected_target_resources: BTreeSet<(String, String)>,
    source_resources: BTreeMap<(String, String), String>,
    handed: BTreeSet<(String, String)>,
    answering: BTreeMap<i64, (String, String)>,
    held: BTreeMap<String, Vec<Value>>,
    streams: BTreeMap<String, (Option<String>, Option<DocumentState>)>,
    document_epochs: BTreeMap<String, u64>,
    document_versions: BTreeMap<(String, String), String>,
    created_blanks: BTreeMap<String, (String, String, i64)>,
    navigated_blanks: BTreeMap<String, (String, String, String)>,
    context_frames: BTreeMap<(String, String), String>,
    context_proofs: BTreeMap<(String, String), ContextProof>,
    runtime_sessions: BTreeMap<String, String>,
    created_contexts: BTreeSet<(String, i64)>,
    cached_documents: BTreeMap<String, DocumentState>,
    paused_documents: BTreeSet<(String, String)>,
    executing_sessions: BTreeSet<String>,
    contexts: BTreeSet<String>,
    targets: BTreeMap<String, Value>,
    worlds: Worlds,
    frames: BTreeMap<String, Vec<String>>,
    frame_parents: BTreeMap<String, String>,
    frame_decisions: BTreeMap<String, bool>,
    frame_sessions: BTreeMap<String, BTreeSet<String>>,
    frame_epoch: u64,
    frame_versions: BTreeMap<String, u64>,
    documents: BTreeMap<String, MainDocument>,
    unobserved: BTreeSet<String>,
    webmcp: webmcp_guard::Webmcp,
    waiting_calls: BTreeSet<i64>,
    call_replies: BTreeMap<i64, Value>,
    issued: issued::Issued,
    waiter: Option<Waiter>,
    waiter_due: Instant,
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
            detaching: BTreeMap::new(),
            detached_ordinary: BTreeSet::new(),
            navigations: BTreeMap::new(),
            events: Vec::new(),
            dialog_sessions: BTreeSet::new(),
            dialogs: BTreeMap::new(),
            dialog_documents: BTreeMap::new(),
            dialog_epoch: 0,
            trace_cookies: Vec::new(),
            guard: Protected::default(),
            paused: VecDeque::new(),
            deciders: BTreeMap::new(),
            decided: BTreeMap::new(),
            fetch_errors: Vec::new(),
            blocked: BTreeSet::new(),
            session_targets: BTreeMap::new(),
            protected_targets: BTreeSet::new(),
            guards: BTreeMap::new(),
            page_scope_sessions: BTreeMap::new(),
            checking_page_events: false,
            pending_page_events: VecDeque::new(),
            runtime_scope_epochs: BTreeMap::new(),
            page_scope_owners: BTreeMap::new(),
            page_scope_epochs: BTreeMap::new(),
            page_scope_unknown: 0,
            guard_sessions: BTreeSet::new(),
            guarding: BTreeSet::new(),
            probing: BTreeSet::new(),
            abandoned: BTreeSet::new(),
            unguarded_optional: BTreeMap::new(),
            notices: Vec::new(),
            discovering: false,
            requests: BTreeMap::new(),
            redirects: BTreeSet::new(),
            protected_body_requests: BTreeSet::new(),
            seen_network: BTreeMap::new(),
            terminal_network: BTreeSet::new(),
            client_navigation_urls: BTreeMap::new(),
            request_aliases: BTreeMap::new(),
            request_resources: BTreeMap::new(),
            protected_request_pages: BTreeSet::new(),
            protected_resources: BTreeSet::new(),
            protected_target_resources: BTreeSet::new(),
            source_resources: BTreeMap::new(),
            handed: BTreeSet::new(),
            answering: BTreeMap::new(),
            held: BTreeMap::new(),
            streams: BTreeMap::new(),
            document_epochs: BTreeMap::new(),
            document_versions: BTreeMap::new(),
            created_blanks: BTreeMap::new(),
            navigated_blanks: BTreeMap::new(),
            context_frames: BTreeMap::new(),
            context_proofs: BTreeMap::new(),
            runtime_sessions: BTreeMap::new(),
            created_contexts: BTreeSet::new(),
            cached_documents: BTreeMap::new(),
            paused_documents: BTreeSet::new(),
            executing_sessions: BTreeSet::new(),
            contexts: BTreeSet::new(),
            targets: BTreeMap::new(),
            worlds: Worlds::default(),
            frames: BTreeMap::new(),
            frame_parents: BTreeMap::new(),
            frame_decisions: BTreeMap::new(),
            frame_sessions: BTreeMap::new(),
            frame_epoch: 0,
            frame_versions: BTreeMap::new(),
            documents: BTreeMap::new(),
            unobserved: BTreeSet::new(),
            webmcp: webmcp_guard::Webmcp::default(),
            waiting_calls: BTreeSet::new(),
            call_replies: BTreeMap::new(),
            issued: issued::Issued::default(),
            waiter: None,
            waiter_due: Instant::now(),
            #[cfg(test)]
            paused_trail: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture_session(&mut self, session: &str) {
        struct Fixture {
            inner: Box<dyn Transport>,
            inbox: VecDeque<String>,
            worlds: BTreeMap<String, Vec<Value>>,
            pending_worlds: BTreeMap<i64, (String, Value)>,
            target_infos: Vec<Value>,
        }
        impl Transport for Fixture {
            fn send_text(&mut self, text: &str) -> Result<(), String> {
                let call: Value = serde_json::from_str(text).unwrap();
                if call["method"] == "Target.getTargets" && call["params"]["filter"] == json!([{}]) {
                    self.inbox.push_back(json!({"id":call["id"],"result":{"targetInfos":self.target_infos}}).to_string());
                    Ok(())
                } else if call["method"] == "Page.getFrameTree" && call["sessionId"].as_str().is_some_and(|session| session.starts_with("fixture-guard:") || session == "S" || session == "OTHER") {
                    self.inbox.push_back(json!({"id":call["id"],"result":{"frameTree":{"frame":{"id":"F","loaderId":"L","url":"https://example.test/","securityOrigin":"https://example.test"}}}}).to_string());
                    Ok(())
                } else if call["sessionId"].as_str().is_some_and(|session| session.starts_with("fixture-guard:")) && matches!(call["method"].as_str(), Some("Runtime.disable" | "Runtime.enable")) {
                    if call["method"] == "Runtime.enable" {
                        self.inbox.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":call["sessionId"],"params":{"context":{"id":1,"uniqueId":"fixture-context","origin":"https://example.test","name":"","auxData":{"frameId":"F","isDefault":true}}}}).to_string());
                        let owner = call["sessionId"].as_str().unwrap().strip_prefix("fixture-guard:").unwrap();
                        for context in self.worlds.get(owner).into_iter().flatten() {
                            self.inbox.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":call["sessionId"],"params":{"context":context}}).to_string());
                        }
                    }
                    self.inbox.push_back(json!({"id":call["id"],"result":{}}).to_string());
                    Ok(())
                } else {
                    if call["method"] == "Page.createIsolatedWorld" {
                        self.pending_worlds.insert(call["id"].as_i64().unwrap(), (call["sessionId"].as_str().unwrap().to_string(), call["params"].clone()));
                    }
                    self.inner.send_text(text)
                }
            }
            fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
                let text = match self.inbox.pop_front() { Some(text) => Some(text), None => self.inner.recv_text(deadline)? };
                if let Some(text) = &text {
                    let reply: Value = serde_json::from_str(text).unwrap();
                    if let Some((owner, params)) = reply["id"].as_i64().and_then(|id| self.pending_worlds.remove(&id)) {
                        if let Some(id) = reply.pointer("/result/executionContextId").and_then(Value::as_i64) {
                            self.worlds.entry(owner).or_default().push(json!({"id":id,"uniqueId":format!("fixture-world:{id}"),"origin":"","name":params["worldName"],"auxData":{"frameId":params["frameId"],"isDefault":false}}));
                        }
                    }
                }
                Ok(text)
            }
        }
        struct Empty;
        impl Transport for Empty {
            fn send_text(&mut self, _: &str) -> Result<(), String> { Ok(()) }
            fn recv_text(&mut self, _: Instant) -> Result<Option<String>, String> { Ok(None) }
        }
        let target = format!("fixture:{session}");
        let guard = format!("fixture-guard:{session}");
        self.targets.insert(target.clone(), json!({"targetId":target,"type":"page","url":"https://example.test/"}));
        self.session_targets.insert(session.into(), target.clone());
        self.guards.insert(target.clone(), guard.clone());
        self.guard_sessions.insert(guard);
        self.unobserved.insert(target);
        let inner = std::mem::replace(&mut self.ws, Box::new(Empty));
        self.ws = Box::new(Fixture { inner, inbox:VecDeque::new(), worlds:BTreeMap::new(), pending_worlds:BTreeMap::new(), target_infos:self.targets.values().cloned().collect() });
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

    pub fn content_blocked(&self, session_id: &str) -> bool {
        self.blocked.contains(session_id) || self.guard_sessions.contains(session_id)
            || self.session_target(session_id).and_then(|target| self.documents.get(&target)).is_some_and(|doc| self.sealed(doc))
    }

    #[cfg(test)]
    pub(crate) fn document_evidence(&self, target_id: &str) -> String {
        self.documents.get(target_id).map_or_else(|| "none".to_string(), |doc| {
            format!("url={} unreachable={} navigating={} previous={} probed={}", doc.url, doc.unreachable_url, doc.navigating, doc.previous, doc.probed)
        })
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
        if method == "Page.navigate" { self.remember_navigation(&params, session_id); }
        if method == "Target.createTarget" {
            return self.create_target(params, timeout);
        }
        let checks = self.prepare(method, &params, session_id)?;
        let pinned = self.pin_execution(method, &params, session_id, &checks)?;
        let object_pin = method == "Runtime.callFunctionOn" && params.get("objectId").is_some() && !checks.is_empty();
        let executed = pinned.is_some() && method != "Runtime.compileScript" || object_pin;
        let mut sent = params.clone();
        if let Some(unique) = &pinned {
            sent.as_object_mut().ok_or_else(|| self.refusal())?.remove("contextId");
            sent.as_object_mut().unwrap().remove("executionContextId");
            sent.as_object_mut().unwrap().remove("executionContextUniqueId");
            sent.as_object_mut().unwrap().remove("executionContextName");
            if method == "Runtime.compileScript" {
                let target = session_id.and_then(|session| self.session_target(session)).ok_or_else(|| self.refusal())?;
                let guard = self.guards.get(&target).or_else(|| self.runtime_sessions.get(&target)).ok_or_else(|| self.refusal())?;
                let proof = self.context_proofs.get(&(guard.clone(), unique.clone())).ok_or_else(|| self.refusal())?;
                sent["executionContextId"] = json!(proof.id);
            } else { sent["uniqueContextId"] = json!(unique); }
        }
        let (forwarded, sent) = self.fetch_command(method, sent, session_id);
        let compile_session = if method == "Runtime.compileScript" {
            session_id.and_then(|session| self.session_target(session)).and_then(|target| self.guards.get(&target).or_else(|| self.runtime_sessions.get(&target))).cloned()
        } else { None };
        let frame_epoch = self.frame_epoch;
        let result = match self.raw_call_execution(&forwarded, sent, compile_session.as_deref().or(session_id), timeout, executed) {
            Ok(result) => result,
            Err(message) => {
                if executed && !message.starts_with("slicc: refused:") && !transport_error(&message) { self.verify_checks(&checks, false, true)?; }
                return Err(message);
            }
        };
        if content_command(method, &params) == Some(true) && !executed {
            self.verify_checks(&checks, matches!(method, "IO.read" | "IO.close"), !method.starts_with("Input."))?;
        }
        if executed { self.verify_execution(&checks)?; }
        if let (true, Some(session), Some(id)) = (
            shield::answers_interception(method),
            session_id,
            shield::request_key(&params),
        ) {
            self.settle(session, &id);
        }
        if let Some(session) = session_id {
            if self.blocked.contains(session) && !harmless(method, &params) && !executed {
                return Err(self.refusal());
            }
        }
        if let Some(frame) = params.get("frameId").and_then(Value::as_str) {
            if self.frame_refused(frame, session_id)? {
                return Err(self.refusal());
            }
        }
        if crosses_frames(method, &params) && self.frame_tree_refused(session_id)? {
            return Err(self.refusal());
        }
        if (params.get("frameId").and_then(Value::as_str).is_some() || crosses_frames(method, &params)) && frame_epoch != self.frame_epoch {
            return Err(self.refusal());
        }
        let result = self.after(method, &params, session_id, result)?;
        self.bind_streams(method, session_id, &result, &checks);
        Ok(result)
    }

    pub fn start_navigation(&mut self, params: Value, session: Option<&str>) -> Result<i64, String> {
        let session = session.filter(|session| !session.is_empty());
        self.admit("Page.navigate", &params, session)?;
        self.remember_navigation(&params, session);
        let id = self.send("Page.navigate", params.clone(), session)?;
        self.navigations.insert(id, Navigation { params, session: session.map(str::to_string), reply: None, detached: false });
        Ok(id)
    }

    pub fn poll_navigation(&mut self, id: i64) -> Option<Result<Value, String>> {
        let navigation = self.navigations.get(&id)?;
        let refused = navigation.detached || navigation.session.as_deref().is_some_and(|session| self.blocked.contains(session) && !harmless("Page.navigate", &navigation.params));
        if refused {
            self.navigations.remove(&id);
            return Some(Err(self.refusal()));
        }
        navigation.reply.as_ref()?;
        let navigation = self.navigations.remove(&id).unwrap();
        let reply = navigation.reply.unwrap();
        if let Some(error) = reply.get("error") {
            let (message, _) = shield::redact_text(&self.guard, error.get("message").and_then(Value::as_str).unwrap_or("CDP error"));
            return Some(Err(format!("Page.navigate: {message}\n")));
        }
        Some(self.after("Page.navigate", &navigation.params, navigation.session.as_deref(), reply.get("result").cloned().unwrap_or(Value::Null)))
    }

    pub fn cancel_navigation(&mut self, id: i64) {
        self.navigations.remove(&id);
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
        if content_command(method, &params) == Some(true) {
            self.call(method, params, session_id)?;
            return Ok(());
        }
        self.prepare(method, &params, session_id)?;
        let answered = shield::answers_interception(method)
            .then(|| shield::request_key(&params))
            .flatten()
            .zip(session_id.map(str::to_string));
        let (forwarded, params) = self.fetch_command(method, params, session_id);
        let sent = self.send(&forwarded, params, session_id)?;
        if let Some((id, session)) = answered {
            self.answering.insert(sent, (session, id));
        }
        Ok(())
    }

    fn refusal(&self) -> String {
        format!("{}\n", self.guard.refusal())
    }

    fn admit(&mut self, method: &str, params: &Value, session_id: Option<&str>) -> Result<(), String> {
        if method == "Page.handleJavaScriptDialog" {
            if params.get("accept").and_then(Value::as_bool).is_none()
                || params.get("promptText").is_some_and(|text| !text.is_string())
                || !params.as_object().is_some_and(|object| object.keys().all(|key| matches!(key.as_str(), "accept" | "promptText"))) {
                return Err(self.refusal());
            }
            return self.admit_dialog(session_id.ok_or_else(|| self.refusal())?);
        }
        if session_id.is_some_and(|session| self.dialog_sessions.contains(session)) { return Err(self.refusal()) }
        if let Some(session) = session_id {
            if self.blocked.contains(session) && !harmless(method, params) {
                return Err(self.refusal());
            }
            if self.guard_sessions.contains(session) {
                return Err(self.refusal());
            }
            if self.session_target(session).is_some_and(|target| !self.refresh(&target)) {
                return Err(self.refusal());
            }
            if !recovery(method, params) && self.session_target(session).and_then(|target| self.documents.get(&target)).is_some_and(|doc| self.sealed(doc)) {
                return Err(self.refusal());
            }
            if !harmless(method, params) {
                if let Some(target) = self.session_target(session) {
                    if self.documents.get(&target).is_some_and(|doc| doc.navigating || self.guard.error_page_url(&doc.url) || !doc.unreachable_url.is_empty()) {
                        if self.target_protected(&target, false)? {
                            return Err(self.refusal());
                        }
                    } else if self.targets.get(&target).is_some_and(|info| self.info_protected(info, &[])) {
                        if self.target_protected(&target, false)? {
                            return Err(self.refusal());
                        }
                    }
                }
            }
        }
        if matches!(method, "Network.loadNetworkResource" | "Page.captureSnapshot") {
            return Err(self.refusal());
        }
        if !shield::body_method(method) && self.content_refused(method, params, session_id.unwrap_or("")) {
            return Err(self.refusal());
        }
        if crosses_frames(method, params) && self.frame_tree_refused(session_id)? {
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
        if method == "Browser.setWindowBounds" {
            self.admit_window(params, session_id)?;
        } else if shield::persistent_capability(method) && !self.network_state_admitted(method, params, session_id) {
            return Err(PERSISTENT.into());
        }
        if method == "Target.autoAttachRelated" || method == "Target.setAutoAttach"
            && params.get("autoAttach").and_then(|v| v.as_bool()) == Some(true)
        {
            return Err(AUTO_ATTACH.into());
        }
        if matches!(method, "Target.attachToTarget" | "Target.setAutoAttach")
            && params.get("flatten").and_then(|v| v.as_bool()) != Some(true)
        {
            return Err(FLATTEN.into());
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
            let entry = params.get("entryId").and_then(|v| v.as_i64()).ok_or_else(|| self.refusal())?;
            let history = self.raw_call(
                "Page.getNavigationHistory",
                json!({}),
                session_id,
                DOCUMENT_CHECK,
            ).map_err(|_| self.refusal())?;
            let url = history
                .get("entries")
                .and_then(|v| v.as_array())
                .and_then(|entries| {
                    entries
                        .iter()
                        .find(|item| item.get("id").and_then(|v| v.as_i64()) == Some(entry))
                })
                .and_then(|item| item.get("url").and_then(|v| v.as_str()))
                .filter(|url| !url.is_empty())
                .ok_or_else(|| self.refusal())?
                .to_string();
            if self.guard.url(&url) {
                return Err(format!("{}\n", self.guard.refusal_for_url(&url)));
            }
        }
        if let Some(frame) = params.get("frameId").and_then(Value::as_str) {
            if self.frame_refused(frame, session_id)? {
                let url = self.frames.get(frame).and_then(|known| known.first()).map(String::as_str).unwrap_or("");
                return Err(format!("{}\n", self.guard.refusal_for_url(url)));
            }
        }
        if !matches!(method, "Target.getTargetInfo" | "Target.detachFromTarget") {
            for key in ["targetId"] {
                if let Some(target) = params.get(key).and_then(|v| v.as_str()) {
                    let target = target.to_string();
                    if self.target_protected(&target, false)? {
                        let url = self.targets.get(&target).and_then(|info| info.get("url")).and_then(Value::as_str).unwrap_or("");
                        return Err(format!("{}\n", self.guard.refusal_for_url(url)));
                    }
                }
            }
        }
        let issued = self.client_issued(method, params, session_id)?;
        if shield::interception_method(method) {
            let id = shield::request_key(params).unwrap_or_default();
            let handed = session_id
                .is_some_and(|session| self.handed.contains(&(session.to_string(), id.clone())));
            if !handed || self.requests.get(&id) == Some(&true) {
                return Err(UNSEEN.into());
            }
        }
        if let Some(session) = session_id.filter(|_| issued) { self.issue(params, session)?; }
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
        if method == "Network.replayXHR" { return Err(self.refusal()) }
        if method == "IO.read" {
            let handle = params.get("handle").and_then(|v| v.as_str()).unwrap_or("");
            if !self.streams.contains_key(handle) {
                return Err(STREAM.into());
            }
        }
        Ok(())
    }

    pub fn settled_refusal(&self, method: &str, params: &Value, session_id: Option<&str>) -> Option<String> {
        let session_id = session_id.filter(|id| !id.is_empty());
        if method == "Page.handleJavaScriptDialog" || harmless(method, params) { return None }
        if session_id.is_some_and(|session| self.blocked.contains(session)) { return Some(self.refusal()) }
        let reason = if matches!(method, "Network.loadNetworkResource" | "Page.captureSnapshot") {
            self.refusal()
        } else if shield::persistent_capability(method) && method != "Browser.setWindowBounds" && (session_id.is_none() || !shield::network_state(method, params)) {
            PERSISTENT.to_string()
        } else if method == "Target.autoAttachRelated" || method == "Target.setAutoAttach" && params.get("autoAttach").and_then(Value::as_bool) == Some(true) {
            AUTO_ATTACH.to_string()
        } else if matches!(method, "Target.attachToTarget" | "Target.setAutoAttach") && params.get("flatten").and_then(Value::as_bool) != Some(true) {
            FLATTEN.to_string()
        } else {
            return None;
        };
        if session_id.is_some_and(|session| self.dialog_sessions.contains(session) || self.guard_sessions.contains(session))
            || self.content_refused(method, params, session_id.unwrap_or("")) {
            return Some(self.refusal());
        }
        if reason == self.refusal() { return Some(reason) }
        if Worlds::selectors(params) || Worlds::sensitive(method) { return None }
        if params.get("targetId").and_then(Value::as_str).is_some_and(|target| self.abandoned.contains(target)) { return Some(UNGUARDED.to_string()) }
        Some(shield::browser_wide(method, params, session_id, &self.contexts).unwrap_or(reason))
    }

    fn network_state_admitted(&self, method: &str, params: &Value, session_id: Option<&str>) -> bool {
        let Some(session) = session_id else { return false };
        shield::network_state(method, params)
            && !self.guard_sessions.contains(session)
            && !self.dialog_sessions.contains(session)
            && !self.blocked.contains(session)
            && self.session_target(session).is_some_and(|target| {
                !self.protected_targets.contains(&target)
                    && !self.abandoned.contains(&target)
                    && self.targets.get(&target).is_some_and(|info| {
                        info.get("type").and_then(Value::as_str) == Some("page") && !self.info_protected(info, &[])
                    })
            })
    }

    fn prepare(&mut self, method: &str, params: &Value, session: Option<&str>) -> Result<Vec<(String, DocumentState)>, String> {
        let content = content_command(method, params).ok_or_else(|| self.refusal())?;
        let mut checks = Vec::new();
        if !content { return Ok(checks) }
        let runtime = method.starts_with("Runtime.");
        let page_scope = if page_scope::content(method) && !runtime { Some(self.verify_page_scope(session)?) } else { None };
        if storage_content(method) && method != "Storage.getStorageKeyForFrame" {
            if !self.storage_scope_verified(params) { return Err(self.refusal()) }
            return Ok(checks);
        }
        if matches!(method, "Fetch.getResponseBody" | "Network.getResponseBody" | "Network.getRequestPostData")
            && session.and_then(|session| self.session_target(session)).is_some_and(|target| self.document_paused(&target))
            && shield::request_key(params).is_some_and(|id| self.request_state(&id) == Some(false)) { return Ok(checks) }
        if matches!(method, "IO.read" | "IO.close") {
            let saved = params.get("handle").and_then(Value::as_str).and_then(|handle| self.streams.get(handle)).cloned();
            let Some((Some(target), Some(expected))) = saved else { return Err(self.refusal()) };
            let mut current = self.document_state_for(&target, true, expected.scope.as_deref()).ok_or_else(|| self.refusal())?;
            if let Some(proof) = &expected.page_scope {
                let guard = self.guards.get(&target).cloned().ok_or_else(|| self.refusal())?;
                if &self.verify_page_scope(Some(&guard))? != proof { return Err(self.refusal()) }
                current.page_scope = Some(proof.clone());
            }
            if current != expected { return Err(self.refusal()) }
            checks.push((target, current));
        }
        if let Some(session) = session {
            let target = self.session_target(session).ok_or_else(|| self.refusal())?;
            if !checks.iter().any(|(known, _)| known == &target) {
                let selected_frame = params.get("frameId").and_then(Value::as_str).map(str::to_string).or_else(|| {
                    ["contextId", "executionContextId", "uniqueContextId", "executionContextUniqueId", "executionContextName"].into_iter().find_map(|key| {
                        let value = params.get(key)?;
                        let label = if matches!(key, "contextId" | "executionContextId") { "id" } else if key == "executionContextName" { "name" } else { "unique" };
                        let value = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                        self.context_frames.get(&(session.to_string(), format!("{label}:{value}"))).cloned()
                    })
                });
                let scope = if crosses_frames(method, params) || page_scope::content(method) && !runtime { None } else { Some(selected_frame.as_deref().unwrap_or("")) };
                let mut state = if self.document_paused(&target) {
                    if !matches!(method, "Runtime.evaluate" | "Runtime.callFunctionOn") { return Err(self.refusal()) }
                    self.cached_documents.get(&target).cloned().ok_or_else(|| self.refusal())?
                } else { self.document_state_for(&target, true, scope).ok_or_else(|| self.refusal())? };
                if self.blocked.contains(session) { return Err(self.refusal()) }
                for key in ["frameId", "contextId", "executionContextId", "uniqueContextId", "executionContextUniqueId", "executionContextName"] {
                    if let Some(value) = params.get(key) {
                        let frame = if key == "frameId" {
                            value.as_str().map(str::to_string)
                        } else {
                            let label = if matches!(key, "contextId" | "executionContextId") { "id" } else if key == "executionContextName" { "name" } else { "unique" };
                            let value = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
                            self.context_frames.get(&(session.to_string(), format!("{label}:{value}"))).cloned()
                        };
                        if frame.is_none_or(|frame| !state.frames.contains_key(&frame)) { return Err(self.refusal()) }
                    }
                }
                state.page_scope = if runtime { Some(self.verify_page_scope(Some(session))?) } else { page_scope };
                state.runtime_scope_epoch = state.page_scope.as_ref().filter(|_| runtime).map(|proof| self.runtime_scope_epochs.get(&proof.0).copied().unwrap_or(0));
                checks.push((target, state));
            }
        }
        if checks.is_empty() { return Err(self.refusal()) }
        Ok(checks)
    }

    fn document_paused(&self, target: &str) -> bool {
        self.paused_documents.iter().any(|(session, _)| self.session_target(session).as_deref() == Some(target))
    }

    fn pin_execution(&mut self, method: &str, params: &Value, session: Option<&str>, checks: &[(String, DocumentState)]) -> Result<Option<String>, String> {
        if !matches!(method, "Runtime.evaluate" | "Runtime.callFunctionOn" | "Runtime.compileScript") || checks.is_empty() { return Ok(None) }
        if method == "Runtime.callFunctionOn" && params.get("objectId").is_some() { return Ok(None) }
        let session = session.ok_or_else(|| self.refusal())?;
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let state = checks.iter().find(|(id, _)| id == &target).map(|(_, state)| state.clone()).ok_or_else(|| self.refusal())?;
        let guard = if let Some(guard) = self.guards.get(&target).or_else(|| self.runtime_sessions.get(&target)) {
            guard.clone()
        } else {
            if state.worker.is_none() { return Err(self.refusal()) }
            let result = self.raw_call("Target.attachToTarget", json!({"targetId":target,"flatten":true}), None, DOCUMENT_CHECK)?;
            let guard = result.get("sessionId").and_then(Value::as_str).ok_or_else(|| self.refusal())?.to_string();
            self.guard_sessions.insert(guard.clone());
            self.session_targets.insert(guard.clone(), target.clone());
            self.runtime_sessions.insert(target.clone(), guard.clone());
            guard
        };
        let choose = |proofs: &BTreeMap<(String, String), ContextProof>| {
            proofs.iter().find(|((owner, unique), proof)| {
                owner == &guard && if let Some(id) = params.get("contextId").or_else(|| params.get("executionContextId")).and_then(Value::as_i64) {
                    proof.id == id
                } else if let Some(id) = params.get("uniqueContextId").or_else(|| params.get("executionContextUniqueId")).and_then(Value::as_str) {
                    unique == id
                } else if let Some(name) = params.get("executionContextName").and_then(Value::as_str) {
                    proof.name == name
                } else { proof.default && (state.worker.is_some() || state.scope.as_deref() == Some(proof.frame.as_str()) || state.frames.get(&proof.frame).is_some_and(|evidence| evidence[4].is_empty())) }
            }).map(|((_, unique), proof)| (unique.clone(), proof.clone()))
        };
        let mut selected = choose(&self.context_proofs);
        let paused = self.document_paused(&target);
        let stable_context = selected.as_ref().is_some_and(|(_, proof)| state.worker.is_some()
            || state.frames.get(&proof.frame).is_some_and(|evidence| proof.loader.as_deref() == Some(evidence[0].as_str())));
        if !paused && !stable_context {
            let deadline = Instant::now() + DOCUMENT_CHECK;
            for _ in 0..2 {
                self.context_proofs.retain(|(owner, _), _| owner != &guard);
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() { return Err(self.refusal()) }
                self.raw_call("Runtime.disable", json!({}), Some(&guard), left)?;
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() { return Err(self.refusal()) }
                self.raw_call("Runtime.enable", json!({}), Some(&guard), left)?;
                let current = self.document_state_for(&target, true, state.scope.as_deref()).ok_or_else(|| self.refusal())?;
                let same = if state.worker.is_some() { state.worker == current.worker } else {
                    state.frames.iter().filter(|(_, evidence)| evidence[4].is_empty()).all(|(id, evidence)| current.frames.get(id).is_some_and(|now| evidence[0] == now[0] && evidence[2] == now[2]))
                };
                if !same || self.blocked.contains(session) { return Err(self.refusal()) }
                selected = choose(&self.context_proofs);
                if selected.is_some() { break }
            }
        }
        let (unique, proof) = selected.ok_or_else(|| self.refusal())?;
        let owned_world = !proof.default && self.created_contexts.contains(&(session.to_string(), proof.id));
        let owned_blank = proof.default && state.frames.get(&proof.frame).is_some_and(|evidence| evidence[1] == "about:blank"
            && self.owns_blank(&target, &proof.frame, &evidence[0]));
        if unique.is_empty() || proof.origin == "null" || proof.origin.is_empty() && !owned_world && !owned_blank || self.guard.url(&proof.origin) { return Err(self.refusal()) }
        if state.worker.is_none() {
            let evidence = state.frames.get(&proof.frame).ok_or_else(|| self.refusal())?;
            if proof.loader.as_deref() != Some(evidence[0].as_str()) { return Err(self.refusal()) }
        }
        Ok(Some(unique))
    }

    fn storage_scope_verified(&self, params: &Value) -> bool {
        let mut scopes = Vec::new();
        fn collect(value: &Value, scopes: &mut Vec<String>) -> bool {
            let Some(items) = value.as_object() else { return false };
            for (key, value) in items {
                if matches!(key.as_str(), "origin" | "securityOrigin" | "storageKey" | "cacheId") {
                    let Some(text) = value.as_str() else { return false };
                    scopes.push(if key == "cacheId" { text.split('|').next().unwrap_or("").to_string() } else { text.to_string() });
                } else if value.is_object() && !collect(value, scopes) { return false }
            }
            true
        }
        collect(params, &mut scopes) && !scopes.is_empty() && scopes.iter().all(|scope| {
            let mut parts = scope.split('^');
            let origin = parts.next().unwrap_or("");
            let valid = |text: &str| url::Url::parse(text).ok().is_some_and(|url| url.origin().ascii_serialization() != "null" && !self.guard.url(text));
            valid(origin) && !self.guard.storage_key(scope)
                && parts.filter_map(|part| part.strip_prefix('0')).all(valid)
        })
    }

    fn verify_checks(&mut self, checks: &[(String, DocumentState)], same_document: bool, stable_page: bool) -> Result<(), String> {
        for (target, expected) in checks {
            if let Some(proof) = &expected.page_scope {
                let session = self.guards.get(&proof.0).cloned().ok_or_else(|| self.refusal())?;
                let current = self.verify_page_scope(Some(&session))?;
                if stable_page && &current != proof { return Err(self.refusal()) }
            }
            let current = self.document_state_for(target, true, expected.scope.as_deref()).ok_or_else(|| self.refusal())?;
            if same_document && &current != expected { return Err(self.refusal()) }
        }
        Ok(())
    }

    fn verify_execution(&mut self, checks: &[(String, DocumentState)]) -> Result<(), String> {
        for (_, state) in checks {
            if let Some(proof) = &state.page_scope {
                let session = self.guards.get(&proof.0).cloned().ok_or_else(|| self.refusal())?;
                if self.session_target(&session).as_deref() != Some(&proof.0) { return Err(self.refusal()) }
                if self.page_scope_unknown != proof.2 || state.runtime_scope_epoch != Some(self.runtime_scope_epochs.get(&proof.0).copied().unwrap_or(0)) {
                    if self.verify_page_scope(Some(&session))?.0 != proof.0 { return Err(self.refusal()) }
                }
            }
        }
        let sealed = checks.iter().any(|(target, state)| {
            let owner = state.worker.as_ref().map_or(target.as_str(), |(_, owner)| owner.as_str());
            self.documents.get(owner).is_some_and(|doc| self.unverified_error(&doc.url, &doc.unreachable_url))
        });
        if sealed { Err(self.refusal()) } else { Ok(()) }
    }

    fn unverified_error(&self, url: &str, unreachable: &str) -> bool {
        let error = self.guard.error_page_url(url) || !unreachable.is_empty()
            || url.get(..13).is_some_and(|scheme| scheme.eq_ignore_ascii_case("chrome-error:"));
        error && (url::Url::parse(unreachable).is_err() || self.guard.url(unreachable))
    }

    fn bind_streams(&mut self, method: &str, session: Option<&str>, result: &Value, checks: &[(String, DocumentState)]) {
        let target = session.and_then(|session| self.session_target(session));
        let document = target.as_ref().and_then(|target| checks.iter().find(|(known, _)| known == target)).map(|(_, state)| state.clone());
        for stream in [result.get("stream"), result.pointer("/resource/stream")].into_iter().flatten().filter_map(Value::as_str) {
            self.streams.insert(stream.to_string(), (target.clone(), document.clone()));
        }
        if method == "IO.resolveBlob" {
            if let Some(uuid) = result.get("uuid").and_then(Value::as_str) {
                self.streams.insert(format!("blob:{uuid}"), (target, document));
            }
        }
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
            if let (Some(frame), Some(id)) = (params.get("frameId").and_then(Value::as_str), result.get("executionContextId")) {
                if let Some(id) = id.as_i64() { self.created_contexts.insert((session_id.unwrap_or("").to_string(), id)); }
                self.context_frames.insert((session_id.unwrap_or("").to_string(), format!("id:{id}")), frame.to_string());
            }
        }
        if method == "Runtime.compileScript" {
            self.worlds.script(session_id.unwrap_or(""), &result);
        }
        match method {
            "Page.navigate" if params.get("url").and_then(Value::as_str) == Some("about:blank")
                && params.get("frameId").is_none() && result.get("errorText").is_none()
                && result.get("isDownload").is_none_or(|download| download == &json!(false)) => {
                if let (Some(target), Some(frame), Some(loader)) = (
                    session_id.and_then(|session| self.session_target(session)),
                    result.get("frameId").and_then(Value::as_str).filter(|id| frame_text_known(id)),
                    result.get("loaderId").and_then(Value::as_str).filter(|id| frame_text_known(id)),
                ) {
                    if let Some(guard) = self.guards.get(&target).filter(|guard| !self.blocked.contains(*guard)) {
                        if !self.protected_targets.contains(&target) && !self.abandoned.contains(&target)
                            && self.documents.get(&target).is_some_and(|doc| !doc.previous && doc.frame_id == frame)
                            && self.targets.get(&target).and_then(|info| info.get("type")).and_then(Value::as_str) == Some("page") {
                            self.navigated_blanks.insert(target, (guard.clone(), frame.to_string(), loader.to_string()));
                        }
                    }
                }
            }
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
                    self.probe_error(info);
                }
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
                    self.targets.insert(target.clone(), info.clone());
                    self.probe_error(&info);
                    let protected = self.info_protected(&info, &[]);
                    if protected && !self.documents.contains_key(&target) {
                        self.protect_target(&target);
                    }
                    if protected || self.discovery_redacted(&info) {
                        result["targetInfo"] = shield::redacted(&info);
                    }
                }
            }
            _ if shield::cookie_reader(method) => {
                let mut cookies = result.get("cookies").and_then(Value::as_array).cloned().unwrap_or_default();
                cookies.extend(self.trace_cookies.iter().cloned());
                shield::redact_cookie_events(&self.guard, &mut self.events, &json!({ "cookies": cookies }));
                shield::drop_protected_cookies(&self.guard, &mut result);
            }
            _ => {}
        }
        shield::redact_strings(&self.guard, &mut result);
        Ok(result)
    }

    fn discovery_redacted(&self, info: &Value) -> bool {
        self.guard.error_redaction()
            && self.guard.url(info.get("url").and_then(Value::as_str).unwrap_or(""))
    }

    fn redact(&mut self, info: &Value, all: &[Value]) -> Value {
        let protected = self.info_protected(info, all);
        if protected {
            if let Some(target) = info.get("targetId").and_then(Value::as_str) {
                if !self.documents.contains_key(target) {
                    self.protect_target(target);
                }
            }
        }
        if protected || self.discovery_redacted(info) {
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
        if let Some(doc) = self.documents.get(id) {
            if doc.navigating {
                return doc.previous || self.guard.url(url) || doc.navigation_url.is_empty() || self.guard.url(&doc.navigation_url);
            }
            if (self.guard.error_page_url(&doc.url) || !doc.unreachable_url.is_empty()) && !self.masked_error(doc, url) {
                if !self.guard.error_document(&doc.url, &doc.origin)
                    || !(self.reported_unreachable(&doc.unreachable_url, url) || self.guard.error_page_url(url) && doc.url == url && !doc.unreachable_url.is_empty()) {
                    return true;
                }
                return self.guard.target_with_document(&doc.unreachable_url, doc.previous, Opener::None,
                    Some(&doc.url), Some(&doc.unreachable_url), doc.navigating);
            }
            if doc.previous {
                return true;
            }
        }
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
        self.judge_target(target_id, frame, true)
    }

    fn judge_target(&mut self, target_id: &str, frame: bool, refusal_protects: bool) -> Result<bool, String> {
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
            Err(message) if refusal_protects && message.starts_with("Target.getTargetInfo: slicc: refused:") => {
                self.protected_targets.insert(target_id.to_string());
                return Ok(true);
            }
            Err(_) if frame => return Ok(false),
            Err(message) => return Err(message),
        };
        if info.is_object() {
            self.targets.insert(target_id.to_string(), info.clone());
            if self.documents.get(target_id).is_some_and(|doc| doc.probed) {
                self.documents.remove(target_id);
            } else if !self.refresh(target_id) {
                return Ok(true);
            }
            self.probe_error(&info);
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
        let protected = if self.documents.contains_key(target_id) {
            self.info_protected(&info, &[])
        } else {
            self.guard.target_type(info.get("type").and_then(Value::as_str).unwrap_or(""))
                || self.guard.target(&url, false, opener)
        };
        if protected && !self.documents.contains_key(target_id) {
            self.protect_target(target_id);
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
        self.record_initial_blank(&target);
        if !url.eq_ignore_ascii_case("about:blank") {
            let guard = self.guards.get(&target).cloned();
            self.send("Page.navigate", json!({ "url": url }), guard.as_deref())?;
        }
        Ok(result)
    }

    fn record_initial_blank(&mut self, target: &str) {
        if let Some(recorded) = self.initial_blank_evidence(target, true) {
            self.created_blanks.insert(target.to_string(), recorded);
            let _ = self.document_state_for(target, true, Some(""));
        }
    }

    pub fn created_blank(&self, target: &str) -> Option<Value> {
        self.created_blanks.get(target).map(|(frame, loader, entry)| json!([frame, loader, entry]))
    }

    pub fn restore_created_blank(&mut self, target: &str, proof: &Value) -> bool {
        let expected = proof.as_array().filter(|values| values.len() == 3).and_then(|values| {
            Some((values[0].as_str()?.to_string(), values[1].as_str()?.to_string(), values[2].as_i64()?))
        });
        let Some(expected) = expected else { return false };
        if self.initial_blank_evidence(target, false).as_ref() != Some(&expected) { return false }
        self.created_blanks.insert(target.to_string(), expected);
        true
    }

    fn owns_blank(&self, target: &str, frame: &str, loader: &str) -> bool {
        self.created_blanks.get(target).is_some_and(|(id, version, _)| id == frame && version == loader)
            || self.navigated_blanks.get(target).is_some_and(|(guard, id, version)| self.guards.get(target) == Some(guard) && id == frame && version == loader)
    }

    fn initial_blank_evidence(&mut self, target: &str, empty: bool) -> Option<(String, String, i64)> {
        let guard = self.guards.get(target)?.clone();
        let result = self.raw_call("Target.getTargetInfo", json!({"targetId":target}), None, DOCUMENT_CHECK).ok()?;
        let info = result.get("targetInfo")?;
        if info.get("targetId").and_then(Value::as_str) != Some(target)
            || info.get("type").and_then(Value::as_str) != Some("page")
            || info.get("url").and_then(Value::as_str) != Some("about:blank")
            || info.get("openerId").is_some() || info.get("openerFrameId").is_some() { return None }
        let history = self.raw_call("Page.getNavigationHistory", json!({}), Some(&guard), DOCUMENT_CHECK).ok()?;
        let entries = history.get("entries")?.as_array()?;
        if history.get("currentIndex").and_then(Value::as_i64) != Some(0) || entries.len() != 1
            || entries[0].get("url").and_then(Value::as_str) != Some("about:blank") { return None }
        let entry = entries[0].get("id")?.as_i64()?;
        let state = self.document_state(target, false)?;
        if empty && state.frames.len() != 1 { return None }
        let (id, evidence) = state.frames.iter().find(|(_, evidence)| evidence[4].is_empty())?;
        if evidence[1] != "about:blank" || !evidence[3].is_empty()
            || matches!(evidence[2].as_str(), "" | "null") || self.guard.url(&evidence[2]) { return None }
        Some((id.clone(), evidence[0].clone(), entry))
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
            if method == "Runtime.enable" {
                let _ = self.document_state_for(target_id, false, Some(""));
                let _ = self.document_state_for(target_id, true, Some(""));
            }
            let optional = matches!(method, "Page.enable" | "Runtime.enable");
            let left = budget.saturating_duration_since(Instant::now());
            let span = if optional { left.min(OPTIONAL_STEP) } else { left };
            let answer = if span.is_zero() {
                Err(format!("{method} timed out\n"))
            } else {
                self.raw_call(method, params, Some(&session), span)
            };
            if method == "Page.enable" && answer.is_err() {
                self.unobserved.insert(target_id.to_string());
            }
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
        if self.documents.get(target_id).is_none_or(|doc| doc.probed) {
            let left = budget.saturating_duration_since(Instant::now());
            if !left.is_zero() {
                let epoch = self.frame_epoch;
                if let Ok(tree) = self.raw_exchange("Page.getFrameTree", json!({}), Some(&session), left.min(OPTIONAL_STEP), false) {
                    if self.tree_current(&tree, epoch) {
                        self.record_frames(&tree, &session);
                    }
                    if self.documents.get(target_id).is_none_or(|doc| doc.probed) {
                        if let Some(frame) = tree.pointer("/frameTree/frame").filter(|frame| self.main_frame_current(frame, epoch)) {
                            self.main_commit(&session, frame);
                        }
                    }
                }
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
        let newly_blocked = self.blocked.insert(session_id.to_string());
        if self.webmcp_is_paused(session_id) {
            self.webmcp.deferred.insert(session_id.to_string());
            self.webmcp_revoke(session_id);
            return;
        }
        if !newly_blocked && !self.webmcp.deferred.remove(session_id) {
            return;
        }
        self.webmcp_revoke(session_id);
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
        if !self.executing_sessions.contains(session_id) { self.detach_blocked(session_id); }
    }

    fn detach_blocked(&mut self, session_id: &str) {
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

    fn main_commit(&mut self, session: &str, frame: &Value) {
        let Some(target) = self.session_target(session) else { return };
        if self.guards.get(&target).is_some_and(|guard| guard != session) && !self.unobserved.contains(&target) {
            if self.guard.url(frame.get("url").and_then(Value::as_str).unwrap_or(""))
                || self.guard.url(frame.get("securityOrigin").and_then(Value::as_str).unwrap_or(""))
            {
                self.committed_protected(session);
            }
            return;
        }
        let Some(frame_id) = frame.get("id").and_then(Value::as_str) else { return };
        if let Some(loader) = frame.get("loaderId").and_then(Value::as_str).filter(|loader| !loader.is_empty()) {
            self.document_versions.insert((target.clone(), frame_id.to_string()), loader.to_string());
        }
        let url = format!("{}{}", frame.get("url").and_then(Value::as_str).unwrap_or(""), frame.get("urlFragment").and_then(Value::as_str).unwrap_or(""));
        let origin = frame.get("securityOrigin").and_then(Value::as_str).unwrap_or("").to_string();
        let unreachable_url = frame.get("unreachableUrl").and_then(Value::as_str).unwrap_or("").to_string();
        let reported = self.targets.get(&target).and_then(|info| info.get("url")).and_then(Value::as_str);
        let unknown = unreachable_url.is_empty() && reported.is_none_or(|reported| self.guard.url(reported));
        let previous = self.documents.get(&target).map(|doc| doc.previous).unwrap_or_else(|| {
            self.protected_targets.contains(&target) || unknown || self.guard.url(&unreachable_url)
        });
        let error = self.guard.error_page_url(&url) || !unreachable_url.is_empty();
        let protected = self.targets.get(&target).is_some_and(|info| self.guard.target_type(info.get("type").and_then(Value::as_str).unwrap_or(""))) || if error {
            previous || unknown || !self.guard.error_document(&url, &origin)
        } else {
            self.guard.target(&url, self.protected_targets.contains(&target), Opener::None) || self.guard.url(&origin)
        };
        self.documents.insert(target.clone(), MainDocument {
            frame_id: frame_id.to_string(), url, origin, unreachable_url, navigating: false, navigation_url: String::new(), previous: protected, probed: false,
        });
        if protected {
            self.protect_target(&target);
        } else if !self.abandoned.contains(&target) {
            self.protected_targets.remove(&target);
        }
    }

    pub fn recovering(&self, target_id: &str) -> bool {
        self.documents.get(target_id).is_some_and(|doc| doc.probed && !doc.previous)
    }

    fn document_state(&mut self, target: &str, content: bool) -> Option<DocumentState> {
        self.document_state_for(target, content, None)
    }

    fn document_state_for(&mut self, target: &str, content: bool, scope: Option<&str>) -> Option<DocumentState> {
        let Some(guard) = self.guards.get(target).cloned() else {
            if self.protected_targets.contains(target) { return None }
            let result = self.raw_call("Target.getTargetInfo", json!({"targetId":target}), None, DOCUMENT_CHECK).ok()?;
            let info = result.get("targetInfo")?;
            let text = |key| info.get(key).and_then(Value::as_str).unwrap_or("");
            if text("targetId") != target || !matches!(text("type"), "worker" | "service_worker" | "shared_worker")
                || url::Url::parse(text("url")).is_err() || self.guard.target(text("url"), false, Opener::Unknown) { return None }
            let owner = text("openerId");
            if !self.guards.contains_key(owner) { return None }
            let mut state = self.document_state(owner, true)?;
            state.worker = Some((text("url").to_string(), owner.to_string()));
            return Some(state);
        };
        let tree = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), DOCUMENT_CHECK).ok()?;
        if self.guards.get(target) != Some(&guard) { return None }
        let epoch = self.document_epochs.get(target).copied().unwrap_or(0);
        let node = tree.get("frameTree")?;
        let scoped;
        let node = if let Some(id) = scope {
            scoped = Self::scoped_document_tree(node, id)?;
            &scoped
        } else { node };
        let frame = node.get("frame")?;
        let parent = frame.get("parentId").and_then(Value::as_str);
        let mut frames = BTreeMap::new();
        if let Some(parent) = parent {
            let info = self.raw_call("Target.getTargetInfo", json!({"targetId":target}), None, DOCUMENT_CHECK).ok()?;
            let info = info.get("targetInfo")?;
            if info.get("targetId").and_then(Value::as_str) != Some(target) || info.get("type").and_then(Value::as_str) != Some("iframe")
                || info.get("parentFrameId").and_then(Value::as_str) != Some(parent) { return None }
            let owner = self.guards.keys().find(|owner| owner.as_str() != target && self.targets.get(*owner).and_then(|info| info.get("type")).and_then(Value::as_str) == Some("page")
                && self.documents.get(*owner).is_some_and(|doc| doc.frame_id == parent)).cloned()?;
            let parent_guard = self.guards.get(&owner)?.clone();
            let parent_tree = self.raw_call("Page.getFrameTree", json!({}), Some(&parent_guard), DOCUMENT_CHECK).ok()?;
            let parent_frame = parent_tree.pointer("/frameTree/frame")?;
            if parent_frame.get("id").and_then(Value::as_str) != Some(parent) || parent_frame.get("parentId").is_some() { return None }
            frames = self.document_state_for(&owner, true, Some(parent))?.frames;
        }
        if !self.collect_document_frames(node, &mut frames, content, parent) { return None }
        if content && frames.iter().any(|(id, evidence)| evidence[0].is_empty() && self.document_versions.contains_key(&(target.to_string(), id.clone()))) { return None }
        if content && self.guard.inheriting(frame.get("url").and_then(Value::as_str).unwrap_or(""))
            && frame.get("securityOrigin").and_then(Value::as_str) == Some("://")
        {
            let id = frame.get("id").and_then(Value::as_str)?;
            let loader = frame.get("loaderId").and_then(Value::as_str)?;
            if frame.get("url").and_then(Value::as_str) != Some("about:blank") || !self.owns_blank(target, id, loader) { return None }
            if self.navigated_blanks.contains_key(target) {
                let info = self.raw_call("Target.getTargetInfo", json!({"targetId":target}), None, DOCUMENT_CHECK).ok()?;
                let info = info.get("targetInfo")?;
                if info.get("targetId").and_then(Value::as_str) != Some(target) || info.get("type").and_then(Value::as_str) != Some("page")
                    || info.get("url").and_then(Value::as_str) != Some("about:blank") || info.get("openerId").is_some() || info.get("openerFrameId").is_some()
                    || frame.get("parentId").is_some() || self.guards.get(target) != Some(&guard) || !self.owns_blank(target, id, loader) { return None }
            }
        }
        self.main_commit(&guard, frame);
        if self.protected_targets.contains(target) { return None }
        for (id, evidence) in &frames {
            self.frames.insert(id.clone(), vec![evidence[1].clone(), evidence[2].clone()]);
            self.note_parent(id, &evidence[4]);
            if content { self.frame_decisions.insert(id.clone(), false); }
            if !evidence[0].is_empty() {
                self.document_versions.insert((target.to_string(), id.clone()), evidence[0].clone());
            }
        }
        let scope = if parent.is_some() && scope == Some("") { frame.get("id").and_then(Value::as_str) } else { scope };
        let state = DocumentState { frames, worker:None, scope:scope.map(str::to_string), epoch, page_scope:None, runtime_scope_epoch:None };
        if content { self.cached_documents.insert(target.to_string(), state.clone()); }
        Some(state)
    }

    fn scoped_document_tree(node: &Value, id: &str) -> Option<Value> {
        let frame = node.get("frame")?;
        if id.is_empty() || frame.get("id").and_then(Value::as_str) == Some(id) { return Some(json!({"frame":frame})) }
        node.get("childFrames")?.as_array()?.iter().find_map(|child| Self::scoped_document_tree(child, id))
            .map(|child| json!({"frame":frame,"childFrames":[child]}))
    }

    fn collect_document_frames(&self, node: &Value, frames: &mut BTreeMap<String, Vec<String>>, content: bool, parent: Option<&str>) -> bool {
        let Some(frame) = node.get("frame") else { return false };
        let origin = self.page_scope_origin(frame);
        let text = |key| if key == "securityOrigin" { origin.unwrap_or_else(|| frame.get(key).and_then(Value::as_str).unwrap_or("")) } else { frame.get(key).and_then(Value::as_str).unwrap_or("") };
        let root = parent.is_none();
        if ["id", "url"].iter().any(|key| matches!(text(*key), "" | "null" | "://"))
            || frame.get("parentId").is_some_and(|id| id.as_str() != parent) { return false }
        let initial_blank = text("url") == "about:blank" && frame.get("loaderId").and_then(Value::as_str) == Some("")
            && (root || parent.and_then(|id| frames.get(id)).is_some_and(|evidence| !self.guard.url(&evidence[1]) && !self.guard.url(&evidence[2])));
        let parent_allowed = !root && parent.and_then(|id| frames.get(id)).is_some_and(|evidence| !self.guard.url(&evidence[1]) && !self.guard.url(&evidence[2]));
        let inherited = parent_allowed && self.guard.inheriting(text("url")) && !matches!(text("securityOrigin"), "" | "null" | "://") && self.frame_decisions.get(text("id")) != Some(&true);
        let allowed_error = !root && self.guard.error_document(text("url"), text("securityOrigin"))
            && !text("unreachableUrl").is_empty() && self.frame_decisions.get(text("id")) == Some(&false);
        let data = (root || parent_allowed) && self.guard.data_document(text("url"));
        let opaque = self.guard.opaque_document(text("url"), root) || !root && initial_blank || inherited || allowed_error || data;
        if content && (text("loaderId") == "null" || text("loaderId").is_empty() && !initial_blank
            || matches!(text("securityOrigin"), "" | "null") || text("securityOrigin") == "://" && !opaque
            || !root && !allowed_error && (self.guard.url(text("url")) || self.guard.url(text("securityOrigin")))
            || self.unverified_error(text("url"), text("unreachableUrl"))) { return false }
        let evidence = vec![text("loaderId").to_string(), format!("{}{}", text("url"), text("urlFragment")), text("securityOrigin").to_string(), text("unreachableUrl").to_string(), parent.unwrap_or("").to_string()];
        if frames.insert(text("id").to_string(), evidence).is_some() { return false }
        match node.get("childFrames") {
            None => true,
            Some(children) => children.as_array().is_some_and(|children| children.iter().all(|child| self.collect_document_frames(child, frames, content, Some(text("id"))))),
        }
    }

    fn refresh(&mut self, target: &str) -> bool {
        let Some(doc) = self.documents.get(target) else { return true };
        if self.document_paused(target) { return true }
        if doc.navigating || doc.probed || !(self.guard.error_page_url(&doc.url) || !doc.unreachable_url.is_empty()) {
            return true;
        }
        let Some(guard) = self.guards.get(target).cloned() else { return true };
        let frame = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), OPTIONAL_STEP).ok().and_then(|tree| tree.pointer("/frameTree/frame").cloned());
        match frame {
            Some(frame) if self.guards.get(target) == Some(&guard) => {
                self.main_commit(&guard, &frame);
                true
            }
            _ => false,
        }
    }

    fn probe_candidate(&self, info: &Value) -> bool {
        let text = |key: &str| info.get(key).and_then(Value::as_str).unwrap_or("");
        let target = text("targetId");
        !target.is_empty() && text("type") == "page" && text("openerId").is_empty() && (self.guard.url(text("url")) || self.guard.error_page_url(text("url"))) && !self.guard.unparseable(text("url"))
            && !self.documents.contains_key(target) && !self.abandoned.contains(target) && !self.guards.contains_key(target)
    }

    fn probe_error(&mut self, info: &Value) {
        let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_string();
        let target = text(info, "targetId");
        let url = text(info, "url");
        if !self.probe_candidate(info) || self.protected_targets.contains(&target) {
            return;
        }
        self.probing.insert(target.clone());
        self.guarding.insert(target.clone());
        let attached = self.raw_call("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None, OPTIONAL_STEP);
        self.guarding.remove(&target);
        let Some(session) = attached.ok().map(|value| text(&value, "sessionId")).filter(|session| !session.is_empty()) else {
            self.probing.remove(&target);
            return;
        };
        self.guard_sessions.insert(session.clone());
        let frame = self.raw_call("Page.getFrameTree", json!({}), Some(&session), OPTIONAL_STEP).ok()
            .and_then(|tree| tree.pointer("/frameTree/frame").cloned())
            .filter(|frame| self.guard.error_document(&text(frame, "url"), &text(frame, "securityOrigin"))
                && (self.reported_unreachable(&text(frame, "unreachableUrl"), &url) || self.guard.error_page_url(&url) && text(frame, "url") == url));
        let history = match frame {
            Some(_) => self.raw_call("Page.getNavigationHistory", json!({}), Some(&session), OPTIONAL_STEP).ok(),
            None => None,
        };
        let _ = self.raw_call("Target.detachFromTarget", json!({ "sessionId": session }), None, OPTIONAL_STEP);
        self.probing.remove(&target);
        let (Some(frame), Some(history)) = (frame, history) else { return };
        let unreachable_url = text(&frame, "unreachableUrl");
        if self.documents.contains_key(&target) || !self.ordinary_history(&history, &unreachable_url) {
            return;
        }
        self.documents.insert(target, MainDocument {
            frame_id: text(&frame, "id"), url: text(&frame, "url"), origin: text(&frame, "securityOrigin"), unreachable_url,
            navigating: false, navigation_url: String::new(), previous: false, probed: true,
        });
    }

    fn reported_unreachable(&self, unreachable: &str, reported: &str) -> bool {
        !unreachable.is_empty()
            && (reported == unreachable
                || url::Url::parse(unreachable).is_ok_and(|parsed| {
                    let origin = parsed.origin().ascii_serialization();
                    reported == origin || reported.strip_suffix('/') == Some(origin.as_str())
                }))
    }

    fn ordinary_history(&self, history: &Value, url: &str) -> bool {
        let entries = history.get("entries").and_then(Value::as_array).cloned().unwrap_or_default();
        let current = history.get("currentIndex").and_then(Value::as_u64).and_then(|index| usize::try_from(index).ok()).unwrap_or(0);
        current >= 1
            && entries.get(current).is_some_and(|entry| {
                self.reported_unreachable(url, entry.get("url").and_then(Value::as_str).unwrap_or("")) && entry.get("transitionType").and_then(Value::as_str) != Some("reload")
            })
            && entries.iter().enumerate().all(|(index, entry)| {
                index == current || ["url", "userTypedURL"].iter().all(|key| !self.guard.url(entry.get(*key).and_then(Value::as_str).unwrap_or("")))
            })
    }

    fn sealed(&self, doc: &MainDocument) -> bool {
        (self.guard.error_page_url(&doc.url) || !doc.unreachable_url.is_empty())
            && (doc.unreachable_url.is_empty() || self.guard.url(&doc.unreachable_url))
    }

    fn masked_error(&self, doc: &MainDocument, reported: &str) -> bool {
        doc.unreachable_url.is_empty()
            && self.guard.error_document(&doc.url, &doc.origin)
            && !self.guard.url(reported)
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

    fn frame_tree_refused(&mut self, session_id: Option<&str>) -> Result<bool, String> {
        let Some(session) = session_id else {
            return Ok(true);
        };
        let tree = match self.raw_call("Page.getFrameTree", json!({}), Some(session), Duration::from_secs(10)) {
            Ok(tree) => tree,
            Err(message) if transport_error(&message) => return Err(message),
            Err(_) => return Ok(true),
        };
        self.verify_page_scope(Some(session))?;
        Ok(tree.get("frameTree").is_none_or(|node| self.tree_node_refused(node)))
    }

    fn tree_node_refused(&self, node: &Value) -> bool {
        let frame = node.get("frame").unwrap_or(&Value::Null);
        let unknown = ["id", "url", "securityOrigin"].iter().any(|key| {
            let text = if *key == "securityOrigin" { self.page_scope_origin(frame).or_else(|| frame.get(*key).and_then(Value::as_str)) } else { frame.get(*key).and_then(Value::as_str) };
            text.is_none_or(|text| !frame_text_known(text))
        });
        unknown || self.frame_protected(frame.get("id").and_then(Value::as_str).unwrap_or(""))
            || node.get("childFrames").is_some_and(|children| children.as_array().is_none_or(|children| children.iter().any(|child| self.tree_node_refused(child))))
    }

    fn frame_refused(&mut self, frame_id: &str, session_id: Option<&str>) -> Result<bool, String> {
        if self.frames.get(frame_id).is_some_and(|known| known.first().is_some_and(|url| self.guard.inheriting(url)) && !self.frame_verified(frame_id)) {
            self.verify_page_scope(session_id)?;
        }
        let known = self.frames.contains_key(frame_id);
        if self.frame_protected(frame_id) || known && !self.frame_verified(frame_id) {
            return Ok(true);
        }
        let other_target = session_id.and_then(|session| self.session_target(session)).is_some_and(|target| target != frame_id);
        let target_allowed = match self.judge_target(frame_id, false, !other_target) {
            Ok(true) => return Ok(true),
            Ok(false) => self.targets.get(frame_id).is_some_and(|info| info.get("targetId").and_then(Value::as_str) == Some(frame_id)
                && info.get("url").and_then(Value::as_str).is_some_and(frame_text_known)),
            Err(message) if transport_error(&message) => return Err(message),
            Err(_) => false,
        };
        let Some(session) = session_id else {
            return Ok(!target_allowed);
        };
        match self.raw_call("Page.getFrameTree", json!({}), Some(session), Duration::from_secs(10)) {
            Ok(_) => Ok(!self.frame_sessions.get(session).is_some_and(|frames| frames.contains(frame_id)) || !self.frame_verified(frame_id)),
            Err(message) if transport_error(&message) => Err(message),
            Err(message) if message.contains("slicc: refused:") => Ok(true),
            Err(_) => Ok(known || !target_allowed),
        }
    }

    fn frame_verified(&self, frame_id: &str) -> bool {
        self.frame_chain_verified(frame_id, 0)
    }

    fn frame_chain_verified(&self, frame_id: &str, depth: usize) -> bool {
        let Some(known) = self.frames.get(frame_id) else { return false };
        if depth > self.frames.len() || !frame_text_known(frame_id) || self.protected_targets.contains(frame_id)
            || known.len() < 2 || known.iter().any(|text| self.guard.url(text)) { return false }
        if known.iter().all(|text| frame_text_known(text)) { return true }
        frame_text_known(&known[0]) && self.guard.data_document(&known[0]) && known[1..].iter().all(|text| text == "://" || frame_text_known(text))
            && self.frame_parents.get(frame_id).is_none_or(|parent| self.frame_chain_verified(parent, depth + 1))
    }

    fn tree_current(&self, tree: &Value, epoch: u64) -> bool {
        epoch == self.frame_epoch || self.tree_matches_updates(tree, epoch)
    }

    fn main_frame_current(&self, frame: &Value, epoch: u64) -> bool {
        frame.get("id").and_then(Value::as_str).is_some_and(|id| self.frame_versions.get(id).is_none_or(|version| *version <= epoch))
    }

    fn tree_matches_updates(&self, tree: &Value, epoch: u64) -> bool {
        let mut frames = BTreeMap::new();
        if let Some(node) = tree.get("frameTree") {
            Self::collect_frames(node, &mut frames);
        }
        let updates = self.frame_versions.iter().filter(|(_, version)| **version > epoch).collect::<Vec<_>>();
        !updates.is_empty() && updates.iter().all(|(id, _)| self.frames.get(*id).is_some_and(|known| {
            frames.get(*id).is_some_and(|fresh| known.get(..2) == fresh.get(..2)
                && known.iter().skip(2).all(|origin| Some(origin) == fresh.get(1)))
        }))
    }

    fn record_frames(&mut self, tree: &Value, session: &str) {
        let mut frames = BTreeMap::new();
        if let Some(node) = tree.get("frameTree") {
            Self::collect_frames(node, &mut frames);
            let mut queue = vec![(node, None::<&str>)];
            while let Some((node, parent)) = queue.pop() {
                if let Some(frame) = node.get("frame") {
                    if let Some(id) = frame.get("id").and_then(Value::as_str) {
                        if let Some(known) = frames.get_mut(id) {
                            if let Some(origin) = self.page_scope_origin(frame) { known[1] = origin.to_string(); }
                            if let Some(parent) = parent { if frame.get("parentId").is_none() { known[2] = parent.to_string(); } }
                        }
                        for child in node.get("childFrames").and_then(Value::as_array).into_iter().flatten() { queue.push((child, Some(id))); }
                    }
                }
            }
        }
        let ids = frames.keys().cloned().collect::<BTreeSet<_>>();
        for id in self.frame_sessions.insert(session.to_string(), ids.clone()).unwrap_or_default().difference(&ids) {
            self.frames.remove(id);
            self.frame_parents.remove(id);
        }
        for (id, mut evidence) in frames {
            let parent = evidence.pop().unwrap_or_default();
            self.note_parent(&id, &parent);
            self.frames.insert(id, evidence);
        }
    }

    fn note_parent(&mut self, frame_id: &str, parent: &str) {
        if parent.is_empty() { self.frame_parents.remove(frame_id); } else { self.frame_parents.insert(frame_id.to_string(), parent.to_string()); }
    }

    fn collect_frames(node: &Value, frames: &mut BTreeMap<String, Vec<String>>) {
        let frame = node.get("frame").unwrap_or(&Value::Null);
        let text = |key: &str| frame.get(key).and_then(Value::as_str).unwrap_or("");
        if !text("id").is_empty() {
            let url = format!("{}{}", text("url"), text("urlFragment"));
            frames.insert(text("id").to_string(), vec![url, text("securityOrigin").to_string(), text("parentId").to_string()]);
        }
        for child in node.get("childFrames").and_then(Value::as_array).into_iter().flatten() {
            Self::collect_frames(child, frames);
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
        if let Some(doc) = self.documents.get_mut(target_id) {
            doc.previous = true;
        }
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
        if shield::body_method(method) && shield::request_key(params).is_some_and(|id| self.protected_body_requests.contains(&id)) { return true }
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

    fn protect_resource(&mut self, session: &str, url: &str) {
        let Some(url) = self.guard.resource_key(url) else { return };
        self.protected_resources.insert((session.to_string(), url.clone()));
        if let Some(target) = self.session_target(session) {
            self.protected_target_resources.insert((target, url));
        }
    }

    fn redirector_protected(&self, url: &str) -> bool {
        let Some(key) = self.guard.resource_key(url) else { return true };
        self.guard.url(url) || self.protected_resources.iter().any(|(_, resource)| resource == &key)
    }

    fn resource_protected(&self, session: &str, url: &str) -> bool {
        let Some(key) = self.guard.resource_key(url) else { return true };
        self.guard.url(url) || self.protected_resources.contains(&(session.to_string(), key.clone()))
            || self.session_target(session).is_some_and(|target| self.protected_target_resources.contains(&(target, key)))
    }

    fn protect_body_request(&mut self, id: &str) {
        let mut ids = self.request_aliases.get(id).cloned().unwrap_or_default();
        ids.insert(id.to_string());
        for id in ids {
            self.protected_body_requests.insert(id.clone());
            for (peer, url) in self.request_resources.get(&id).cloned().unwrap_or_default() { self.protect_resource(&peer, &url); }
        }
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
        self.page_scope_changed(&method, &params, session.as_deref());
        if let Some(target) = session.as_deref().and_then(|session| self.session_target(session)) {
            if method == "Page.frameNavigated" {
                let id = params.pointer("/frame/id").and_then(Value::as_str).unwrap_or("").to_string();
                let loader = params.pointer("/frame/loaderId").and_then(Value::as_str).unwrap_or("").to_string();
                let key = (target.clone(), id);
                if loader.is_empty() || self.document_versions.get(&key) != Some(&loader) {
                    if !loader.is_empty() { self.document_versions.insert(key, loader); }
                    *self.document_epochs.entry(target).or_default() += 1;
                }
            } else if method == "Page.frameDetached" {
                let id = params.get("frameId").and_then(Value::as_str).unwrap_or("").to_string();
                if self.document_versions.remove(&(target.clone(), id)).is_some() {
                    *self.document_epochs.entry(target).or_default() += 1;
                }
            }
        }
        if matches!(method.as_str(), "Runtime.executionContextDestroyed" | "Runtime.executionContextsCleared") {
            if let Some(session) = session.as_ref() {
                let target = self.session_target(session);
                let id = params.get("executionContextId").and_then(Value::as_i64);
                let unique = params.get("executionContextUniqueId").and_then(Value::as_str);
                let cleared = method == "Runtime.executionContextsCleared";
                let owners: BTreeSet<String> = self.context_proofs.keys().map(|(owner, _)| owner.clone()).chain(self.created_contexts.iter().map(|(owner, _)| owner.clone())).filter(|owner| owner == session || target.is_some() && self.session_target(owner) == target).collect();
                self.context_proofs.retain(|(owner, key), proof| !owners.contains(owner) || !(cleared || id == Some(proof.id) || unique == Some(key.as_str())));
                self.created_contexts.retain(|(owner, key)| !owners.contains(owner) || !(cleared || id == Some(*key)));
            }
        }
        if method == "Runtime.executionContextCreated" {
            if let (Some(session), Some(unique), Some(id)) = (session.as_ref(), params.pointer("/context/uniqueId").and_then(Value::as_str), params.pointer("/context/id").and_then(Value::as_i64)) {
                let frame = params.pointer("/context/auxData/frameId").and_then(Value::as_str).unwrap_or("").to_string();
                let loader = self.session_target(session).and_then(|target| self.document_versions.get(&(target, frame.clone())).cloned());
                self.context_proofs.insert((session.clone(), unique.to_string()), ContextProof {
                    id, frame, loader, origin:params.pointer("/context/origin").and_then(Value::as_str).unwrap_or("").to_string(),
                    name:params.pointer("/context/name").and_then(Value::as_str).unwrap_or("").to_string(),
                    default:params.pointer("/context/auxData/isDefault").and_then(Value::as_bool) == Some(true),
                });
            }
            if let Some(session) = session.as_ref() {
                let frame = params.pointer("/context/auxData/frameId").and_then(Value::as_str);
                for (label, key) in [("id", "id"), ("unique", "uniqueId"), ("name", "name")] {
                    if let Some(id) = params.pointer("/context").and_then(|context| context.get(key)) {
                        let id = id.as_str().map(str::to_string).unwrap_or_else(|| id.to_string());
                        let key = (session.clone(), format!("{label}:{id}"));
                        if let Some(frame) = frame { self.context_frames.insert(key, frame.to_string()); }
                        else { self.context_frames.remove(&key); }
                    }
                }
            }
        }
        if let Some(id) = params.get("requestId").and_then(Value::as_str) {
            if self.redirect_page_protected(value) {
                self.protected_request_pages.insert((session.clone().unwrap_or_default(), id.to_string()));
            }
        }
        if let Some(id) = params.get("requestId").and_then(|v| v.as_str()) {
            if let Some(url) = shield::request_url(&params) {
                let resource = (session.clone().unwrap_or_default(), url.clone());
                self.request_resources.entry(id.to_string()).or_default().insert(resource.clone());
                if self.redirects.contains(id) || self.protected_body_requests.contains(id) || self.guard.url(&url) {
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
            let authority = if method == "Network.requestWillBeSentExtraInfo" { params.clone() } else { params.get("request").cloned().unwrap_or(Value::Null) };
            if shield::scheme_less_protected(&self.guard, &authority) {
                self.requests.insert(id.to_string(), true);
                for (peer, url) in self.request_resources.get(id).cloned().unwrap_or_default() { self.protect_resource(&peer, &url); }
            }
            if self.protected_carrier(value, false) { self.protect_body_request(id); }
            if self.actual_redirect_carrier(value) { self.mark_redirect(id); }
        }
        if let Some((id, url)) = match method.as_str() {
            "Debugger.scriptParsed" | "Debugger.scriptFailedToParse" => params.get("scriptId").and_then(Value::as_str).zip(params.get("url").and_then(Value::as_str)),
            "CSS.styleSheetAdded" => params.pointer("/header/styleSheetId").and_then(Value::as_str).zip(params.pointer("/header/sourceURL").and_then(Value::as_str)),
            _ => None,
        } {
            self.source_resources.insert((session.clone().unwrap_or_default(), id.to_string()), url.to_string());
        }
        if matches!(method.as_str(), "Page.frameNavigated" | "Page.frameAttached" | "Page.frameDetached" | "Page.navigatedWithinDocument") {
            self.frame_epoch += 1;
        }
        match method.as_str() {
            "Page.frameAttached" | "Page.frameDetached" => {
                if method == "Page.frameAttached" && session.as_ref().is_some_and(|session| self.guard_sessions.contains(session)) {
                    if let (Some(id), Some(parent)) = (params.get("frameId").and_then(Value::as_str), params.get("parentFrameId").and_then(Value::as_str)) {
                        if let Some(protected) = self.frame_decisions.get(parent).copied() {
                            self.frame_decisions.entry(id.to_string()).or_insert(protected);
                        }
                    }
                }
                if let Some(id) = params.get("frameId").and_then(Value::as_str) {
                    self.frames.remove(id);
                    self.frame_parents.remove(id);
                    self.frame_versions.insert(id.to_string(), self.frame_epoch);
                    for frames in self.frame_sessions.values_mut() {
                        frames.remove(id);
                    }
                }
            }
            "Page.navigatedWithinDocument" => {
                if let Some(id) = params.get("frameId").and_then(Value::as_str) {
                    self.frame_versions.insert(id.to_string(), self.frame_epoch);
                    if let Some(known) = self.frames.get_mut(id).and_then(|known| known.first_mut()) {
                        *known = params.get("url").and_then(Value::as_str).unwrap_or("").to_string();
                    }
                }
            }
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
                    self.note_parent(id, frame.get("parentId").and_then(Value::as_str).unwrap_or(""));
                    if self.guard.url(&url) || self.guard.url(&origin) {
                        self.frame_decisions.insert(id.to_string(), true);
                    } else if !self.guard.error_page_url(&url) && !matches!(origin.as_str(), "" | "null" | "://") {
                        self.frame_decisions.insert(id.to_string(), false);
                    }
                    self.frame_versions.insert(id.to_string(), self.frame_epoch);
                    self.frame_sessions.entry(session.clone()).or_default().insert(id.to_string());
                }
                if frame.get("parentId").is_none() {
                    self.main_commit(&session, &frame);
                    if self.session_target(&session).is_none() && (self.guard.url(&url) || self.guard.url(&origin)) {
                        self.committed_protected(&session);
                    }
                }
            }
            "Page.frameStartedNavigating" => {
                if matches!(params.get("navigationType").and_then(Value::as_str), Some("sameDocument" | "historySameDocument")) {
                    return;
                }
                if let Some(target) = session.as_deref().and_then(|session| self.session_target(session)) {
                    if self.guards.get(&target).is_some_and(|guard| Some(guard) != session.as_ref()) && !self.unobserved.contains(&target) {
                        return;
                    }
                    if let Some(doc) = self.documents.get_mut(&target) {
                        if params.get("frameId").and_then(Value::as_str) == Some(&doc.frame_id) {
                            doc.navigating = true;
                            doc.navigation_url = params.get("url").and_then(Value::as_str).unwrap_or("").to_string();
                        }
                    }
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
                if self.info_protected(&info, &[]) && !self.documents.contains_key(&target) && !self.probing.contains(&target) && !self.probe_candidate(&info) {
                    self.protect_target(&target);
                }
                if self.info_protected(&info, &[]) || self.discovery_redacted(&info) {
                    value["params"]["targetInfo"] = shield::redacted(&info);
                } else if !self.documents.contains_key(&target) && !self.guard.target(url, false, Opener::Unknown)
                    && !self.abandoned.contains(&target)
                {
                    self.protected_targets.remove(&target);
                }
            }
            "Target.attachedToTarget" => {
                if let Some(info) = params.get("targetInfo") {
                    value["params"]["targetInfo"] = if info.get("targetId").and_then(Value::as_str).is_some_and(|target| self.probing.contains(target)) {
                        shield::redacted(info)
                    } else {
                        self.redact(info, &[])
                    };
                }
            }
            "Target.targetDestroyed" => {
                if let Some(target) = params.get("targetId").and_then(|v| v.as_str()) {
                    self.guards.remove(target);
                    self.targets.remove(target);
                    self.created_blanks.remove(target);
                    self.navigated_blanks.remove(target);
                    self.documents.remove(target);
                    self.unobserved.remove(target);
                }
            }
            "Target.detachedFromTarget" => {
                if let Some(child) = params.get("sessionId").and_then(|v| v.as_str()) {
                    self.forget_session(child);
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
        self.paused_documents.remove(&(session_id.to_string(), request_id.to_string()));
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
                if self.guard_sessions.contains(child) || self.blocked.contains(child)
                    && !(method == "Target.detachedFromTarget" && self.detached_ordinary.contains(child)) {
                    return true;
                }
            }
            return false;
        }
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        if let Some(id) = params.get("requestId").and_then(|v| v.as_str()) {
            if self.requests.get(id) == Some(&true) || self.redirect_page_protected(value) {
                return true;
            }
        }
        shield::loaded_urls_protected(&self.guard, &params)
    }

    fn fail_seen_network(&mut self, value: &Value) {
        let Some(id) = value.pointer("/params/networkId").and_then(Value::as_str).or_else(|| value.pointer("/params/requestId").and_then(Value::as_str)) else { return };
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
        if self.guard_sessions.contains(session) && self.request_state(id) != Some(true) { return }
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

    fn ordinary_redirect_url(&self, url: &str) -> bool {
        !self.guard.url(url) && !self.guard.url(&shield::web_socket_as_http(url)) && url::Url::parse(&shield::web_socket_as_http(url)).is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
    }

    fn remember_navigation(&mut self, params: &Value, session: Option<&str>) {
        if let Some((session, url)) = session.zip(params["url"].as_str()).filter(|(_, url)| self.ordinary_redirect_url(url)) {
            self.client_navigation_urls.insert(session.to_string(), url.to_string());
        }
    }

    fn ordinary_network_source(&self, session: &str, id: &str) -> Option<String> {
        let resources: Vec<_> = self.request_resources.get(id).into_iter().flatten().filter(|(peer, _)| peer == session).map(|(_, url)| url).collect();
        if resources.is_empty() || resources.iter().any(|url| !self.ordinary_redirect_url(url)) { return None }
        resources.first().map(|url| (*url).clone())
    }

    fn actual_redirect_carrier(&self, value: &Value) -> bool {
        self.protected_carrier(value, true)
    }

    fn protected_carrier(&self, value: &Value, navigation: bool) -> bool {
        let params = &value["params"];
        let method = value["method"].as_str().unwrap_or("");
        if navigation && method == "Network.requestWillBeSentExtraInfo" { return false }
        let (headers, status, source) = match method {
            "Network.requestWillBeSent" => (params.pointer("/redirectResponse/headers"), params.pointer("/redirectResponse/status"), params.pointer("/redirectResponse/url").and_then(Value::as_str).map(str::to_string)),
            "Network.responseReceived" => (params.pointer("/response/headers"), params.pointer("/response/status"), params.pointer("/response/url").and_then(Value::as_str).map(str::to_string)),
            "Fetch.requestPaused" => (params.get("responseHeaders"), params.get("responseStatusCode"), params.pointer("/request/url").and_then(Value::as_str).map(str::to_string)),
            "Network.responseReceivedExtraInfo" | "Network.requestWillBeSentExtraInfo" => (params.get("headers"), params.get("statusCode"), self.ordinary_network_source(value["sessionId"].as_str().unwrap_or(""), params["requestId"].as_str().unwrap_or(""))),
            _ => return false,
        };
        let source = source.as_deref().unwrap_or("https://ordinary.invalid/");
        let redirect = status.and_then(Value::as_u64).is_some_and(|status| matches!(status, 301 | 302 | 303 | 307 | 308));
        let sensitive = |target: &str| Self::carrier_targets(&self.guard, target, source).is_none_or(|targets| targets.iter().any(|target| target.protected && {
            let url = &target.url;
            !url.path().trim_matches('/').is_empty() || url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() || url.password().is_some()
        }));
        let protected = |name: &str, text: &str| {
            match name.to_ascii_lowercase().as_str() {
                "location" if navigation && redirect => Self::carrier_targets(&self.guard, text, source).is_none_or(|targets| targets.iter().any(|target| target.protected)),
                "location" | "content-location" if !navigation => sensitive(text),
                "refresh" => Self::redirect_header_value(&self.guard, name, text, source).is_none_or(|text| text.split_once(";url=").is_none_or(|(_, url)| self.guard.url(url))),
                "link" => text.split([',', '\n']).any(|part| part.trim().strip_prefix('<').and_then(|part| part.split_once('>')).is_some_and(|(target, _)| sensitive(target))),
                _ => false,
            }
        };
        let found = match headers {
            Some(Value::Object(headers)) => headers.iter().any(|(name, value)| value.as_str().is_some_and(|text| protected(name, text))),
            Some(Value::Array(headers)) => headers.iter().any(|field| field["name"].as_str().zip(field["value"].as_str()).is_some_and(|(name, text)| protected(name, text))),
            _ => false,
        };
        found || ["headersText", "requestHeadersText"].iter().filter(|field| !navigation || **field == "headersText").any(|field| params.get(*field).and_then(Value::as_str).is_some_and(|text| text.lines().filter_map(|line| line.split_once(':')).any(|(name, value)| protected(name.trim(), value.trim()))))
    }

    fn ordinary_late_extra_info(&self, value: &Value) -> bool {
        let session = value["sessionId"].as_str().unwrap_or("");
        let id = value["params"]["requestId"].as_str().unwrap_or("");
        self.requests.get(id) == Some(&false) && !self.redirects.contains(id) && !self.protected_body_requests.contains(id) && !self.guard_sessions.contains(session)
            && !self.redirect_page_protected(value) && !shield::loaded_urls_protected(&self.guard, &value["params"])
            && self.ordinary_network_source(session, id).is_some()
    }

    fn redirect_page_protected(&self, value: &Value) -> bool {
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let params = &value["params"];
        let hop = value["method"] == "Network.requestWillBeSent" && params["type"] == "Document" && params.pointer("/redirectResponse/url").and_then(Value::as_str).is_some_and(|url| self.ordinary_redirect_url(url)) && params.get("documentURL").is_some_and(|url| Some(url) == params.pointer("/request/url"));
        self.blocked.contains(session)
            || self.protected_request_pages.iter().any(|(peer, id)| id == params["requestId"].as_str().unwrap_or("") && (peer == session || self.session_target(session).is_some_and(|target| self.session_target(peer).as_ref() == Some(&target))))
            || self.session_target(session).is_some_and(|target| self.protected_targets.contains(&target) || self.documents.get(&target).is_some_and(|doc| self.guard.url(&doc.url) || self.guard.url(&doc.origin)))
            || params.get("frameId").and_then(Value::as_str).is_some_and(|frame| self.frame_protected(frame))
            || params.get("initiator").is_some_and(|initiator| shield::loaded_urls_protected(&self.guard, initiator))
            || !hop && params.get("documentURL").and_then(Value::as_str).is_some_and(|url| self.guard.url(url))
    }

    fn refresh_parts(text: &str) -> Option<(&str, Option<&str>)> {
        let text = text.trim_start_matches(|ch: char| ch.is_ascii_whitespace());
        let bytes = text.as_bytes();
        let mut at = bytes.iter().take_while(|byte| byte.is_ascii_digit()).count();
        if at == 0 { return None }
        if bytes.get(at) == Some(&b'.') {
            at += 1;
            while bytes.get(at).is_some_and(u8::is_ascii_digit) { at += 1; }
        }
        let delay = &text[..at];
        let tail = text[at..].trim_start_matches(|ch: char| ch.is_ascii_whitespace());
        if tail.is_empty() { return Some((delay, None)) }
        let mut target = tail.strip_prefix(';').or_else(|| tail.strip_prefix(','))?.trim_matches(|ch: char| ch.is_ascii_whitespace());
        if target.get(..3).is_some_and(|key| key.eq_ignore_ascii_case("url")) {
            if let Some(value) = target[3..].trim_start_matches(|ch: char| ch.is_ascii_whitespace()).strip_prefix('=') {
                target = value.trim_start_matches(|ch: char| ch.is_ascii_whitespace());
            }
        }
        if let Some(quote) = target.chars().next().filter(|ch| matches!(ch, '\'' | '"')) {
            target = &target[1..];
            target = target.split(quote).next().unwrap_or("");
        }
        target = target.trim_matches(|ch: char| ch.is_ascii_whitespace());
        Some((delay, (!target.is_empty()).then_some(target)))
    }

    pub fn ordinary_redirect_target(&self, source: &str, target: &str) -> Result<String, String> {
        target_refusal(&self.guard, source, target).map_err(|_| self.refusal())
    }

    fn carrier_targets(guard: &Protected, target: &str, source: &str) -> Option<Vec<CarrierTarget>> {
        let base = url::Url::parse(source).ok()?;
        let mut target = target.trim().to_string();
        if target.chars().any(char::is_control) { return None }
        let mut targets = Vec::new();
        for _ in 0..9 {
            let url = base.join(&target).ok()?;
            let keyed = guard.resource_key(&target).and_then(|key| url::Url::parse(&key).ok()).unwrap_or_else(|| url.clone());
            let origin = keyed.origin().ascii_serialization();
            let web = origin.starts_with("http://") || origin.starts_with("https://");
            let protected = if web { guard.url(&origin) } else { guard.url(keyed.as_str()) };
            targets.push(CarrierTarget { origin: web.then_some(origin), protected, url });
            let decoded = shield::decode_percent(&target);
            if decoded == target { return Some(targets) }
            target = decoded;
        }
        None
    }

    fn redirect_header_value(guard: &Protected, name: &str, text: &str, source: &str) -> Option<String> {
        if text.is_empty() || text.chars().any(|ch| ch.is_control() && !(name.eq_ignore_ascii_case("refresh") && ch == '\t')) { return None }
        let origin = |target: &str| {
            let targets = Self::carrier_targets(guard, target, source)?;
            targets.iter().find(|target| target.protected).unwrap_or(&targets[0]).origin.clone()
        };
        match name.to_ascii_lowercase().as_str() {
            "location" | "content-location" => origin(text),
            "refresh" => {
                let (delay, target) = Self::refresh_parts(text)?;
                Some(format!("{delay};url={}", origin(target.unwrap_or(source))?))
            }
            "link" => text.split(',').map(|part| {
                let (target, params) = part.trim().strip_prefix('<')?.split_once('>')?;
                let mut link = format!("<{}>", origin(target)?);
                for param in params.split(';') {
                    let Some((key, value)) = param.trim().split_once('=') else { continue };
                    let key = key.to_ascii_lowercase();
                    let value = value.trim();
                    let tokens = value.trim_matches('"');
                    if matches!(key.as_str(), "rel" | "rev") && !tokens.is_empty() && tokens.split_ascii_whitespace().all(|token| matches!(token, "preconnect" | "prefetch" | "preload" | "next" | "prev" | "stylesheet" | "canonical" | "alternate" | "noopener" | "noreferrer" | "icon" | "dns-prefetch" | "upskill" | "https://www.sliccy.ai/rel/upskill")) {
                        link.push_str(&format!("; {key}={value}"));
                    }
                }
                Some(link)
            }).collect::<Option<Vec<_>>>().map(|links| links.join(", ")),
            _ => None,
        }
    }

    pub fn ordinary_response_body(&self, session: &str, request_id: &str) -> bool {
        !self.content_blocked(session) && self.request_state(request_id) == Some(false)
            && !self.protected_body_requests.contains(request_id) && self.ordinary_network_source(session, request_id).is_some()
    }

    pub fn redact_redirect_carriers(&self, headers: &mut Value, source: &str) {
        let Some(fields) = headers.as_object_mut() else { return };
        fields.retain(|name, value| {
            let kind = name.to_ascii_lowercase();
            if !matches!(kind.as_str(), "location" | "content-location" | "refresh" | "link") { return true }
            let Some(text) = value.as_str() else { return false };
            let targets = match kind.as_str() {
                "refresh" => Self::refresh_parts(text).and_then(|(_, target)| Self::carrier_targets(&self.guard, target.unwrap_or(source), source)),
                "link" => text.split(',').map(|part| part.trim().strip_prefix('<').and_then(|part| part.split_once('>')).and_then(|(target, _)| Self::carrier_targets(&self.guard, target, source))).collect::<Option<Vec<_>>>().map(|parts| parts.into_iter().flatten().collect()),
                _ => Self::carrier_targets(&self.guard, text, source),
            };
            if targets.is_some_and(|targets| !targets.is_empty() && targets.iter().all(|target| !target.protected && matches!(target.url.scheme(), "http" | "https") && target.url.has_host())) { return true }
            match Self::redirect_header_value(&self.guard, name, text, source) {
                Some(safe) => { *value = json!(safe); true }
                None => false,
            }
        });
    }

    fn redirect_headers(headers: &mut Value, source: &str, guard: &Protected) {
        match headers {
            Value::Object(fields) => fields.retain(|name, value| {
                let safe = value.as_str().and_then(|text| Self::redirect_header_value(guard, name, text, source));
                if let Some(safe) = safe { *value = json!(safe); true } else { false }
            }),
            Value::Array(fields) => {
                fields.retain_mut(|field| {
                    let safe = field.get("name").and_then(Value::as_str).zip(field.get("value").and_then(Value::as_str)).and_then(|(name, text)| Self::redirect_header_value(guard, name, text, source));
                    if let Some(safe) = safe { field["value"] = json!(safe); true } else { false }
                });
                for field in fields { if let Some(fields) = field.as_object_mut() { fields.retain(|name, _| matches!(name.as_str(), "name" | "value")); } }
            }
            _ => *headers = json!({}),
        }
    }

    fn visible_redirect(&self, value: &mut Value) -> bool {
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
        if self.guard_sessions.contains(&session) || self.redirect_page_protected(value) { return false }
        let id = value.pointer("/params/requestId").and_then(Value::as_str).unwrap_or("").to_string();
        let method = value.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let params = &value["params"];
        let source = match method.as_str() {
            "Network.requestWillBeSent" => params.pointer("/redirectResponse/url").and_then(Value::as_str).map(str::to_string),
            "Network.responseReceived" => params.pointer("/response/url").and_then(Value::as_str).map(str::to_string),
            "Fetch.requestPaused" => params.pointer("/request/url").and_then(Value::as_str).map(str::to_string),
            "Network.responseReceivedExtraInfo" => self.ordinary_network_source(&session, &id),
            _ => return false,
        };
        let Some(source) = source.filter(|url| self.ordinary_redirect_url(url)) else { return false };
        let mut loaded = params.clone();
        if method == "Network.requestWillBeSent" {
            if let Some(fields) = loaded.as_object_mut() { fields.remove("request"); fields.remove("documentURL"); }
        }
        if shield::loaded_urls_protected(&self.guard, &loaded) { return false }
        if method == "Fetch.requestPaused" && Paused::from_event(value).is_none_or(|paused| self.initiated_by_protected(&paused)) { return false }
        let mut carriers = match method.as_str() {
            "Network.responseReceived" => params.pointer("/response/headers"),
            "Network.requestWillBeSent" => params.pointer("/redirectResponse/headers"),
            "Network.responseReceivedExtraInfo" => params.get("headers"),
            _ => params.get("responseHeaders"),
        }.cloned().unwrap_or(Value::Null);
        Self::redirect_headers(&mut carriers, &source, &self.guard);
        if !shield::redact_strings(&self.guard, &mut carriers) { return false }
        if method == "Network.responseReceivedExtraInfo" && !self.requests.contains_key(&id) { return false }
        let status = match method.as_str() {
            "Network.requestWillBeSent" => value.pointer("/params/redirectResponse/status"),
            "Network.responseReceived" => value.pointer("/params/response/status"),
            "Network.responseReceivedExtraInfo" => value.pointer("/params/statusCode"),
            "Fetch.requestPaused" => value.pointer("/params/responseStatusCode"),
            _ => return false,
        }.and_then(Value::as_u64);
        if !status.is_some_and(|status| matches!(status, 301 | 302 | 303 | 307 | 308)) || method == "Network.requestWillBeSent" && !self.seen_network.contains_key(&(session, id)) {
            return false;
        }
        let mut redacted = value.clone();
        if !shield::redact_strings(&self.guard, &mut redacted) { return false }
        let params = &mut redacted["params"];
        if method == "Network.responseReceivedExtraInfo" {
            if let Some(fields) = params.as_object_mut() { fields.retain(|key, _| matches!(key.as_str(), "requestId" | "statusCode" | "headers" | "headersText")); }
            Self::redirect_headers(&mut params["headers"], &source, &self.guard);
            if params.get("headersText").is_some() {
                let headers = params["headers"].as_object().map(|headers| headers.iter().map(|(name, value)| format!("{name}: {}\r\n", value.as_str().unwrap())).collect::<String>()).unwrap_or_default();
                params["headersText"] = json!(format!("HTTP/1.1 {}\r\n{headers}\r\n", status.unwrap()));
            }
        } else {
            let request = json!({"url":params.pointer("/request/url").cloned().unwrap_or(Value::Null),"method":params.pointer("/request/method").and_then(Value::as_str).filter(|method| matches!(*method, "GET" | "HEAD" | "POST" | "PUT" | "DELETE" | "CONNECT" | "OPTIONS" | "TRACE" | "PATCH")).unwrap_or("GET"),"headers":{}});
            if method == "Fetch.requestPaused" {
                if let Some(fields) = params.as_object_mut() { fields.retain(|key, _| matches!(key.as_str(), "requestId" | "networkId" | "frameId" | "resourceType" | "responseStatusCode" | "responseHeaders")); }
                Self::redirect_headers(&mut params["responseHeaders"], &source, &self.guard);
                params["responseStatusText"] = json!("");
                params["request"] = request;
            } else {
                let key = if method == "Network.requestWillBeSent" { "redirectResponse" } else { "response" };
                let mut response = params[key].clone();
                if let Some(fields) = response.as_object_mut() { fields.retain(|key, _| matches!(key.as_str(), "url" | "status" | "headers")); }
                response["statusText"] = json!("");
                Self::redirect_headers(&mut response["headers"], &source, &self.guard);
                if let Some(fields) = params.as_object_mut() { fields.retain(|key, _| matches!(key.as_str(), "requestId" | "loaderId" | "frameId" | "timestamp" | "wallTime" | "type")); }
                if method == "Network.requestWillBeSent" { params["request"] = request; }
                params[key] = response;
            }
        }
        *value = redacted;
        true
    }

    fn deliver(&mut self, value: Value) {
        self.deliver_event(value);
        if !self.checking_page_events {
            while let Some(event) = self.pending_page_events.pop_front() { self.deliver_event(event); }
        }
    }

    fn deliver_event(&mut self, mut value: Value) {
        self.watch(&mut value);
        if value.get("method").and_then(Value::as_str) == Some("Page.screencastFrame") {
            if let Some(session) = value.get("sessionId").and_then(Value::as_str) {
                let _ = self.send("Page.stopScreencast", json!({}), Some(session));
            }
            return;
        }
        if value.get("method").and_then(Value::as_str).is_some_and(page_scope::content_event) {
            let session = value.get("sessionId").and_then(Value::as_str);
            if session.is_some_and(|session| self.guard_sessions.contains(session)) { return }
            if self.checking_page_events { self.pending_page_events.push_back(value); return }
            self.checking_page_events = true;
            let verified = self.verify_page_scope(session).is_ok();
            self.checking_page_events = false;
            if !verified { return }
        }
        let key = (value.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string(), value.pointer("/params/requestId").and_then(Value::as_str).unwrap_or("").to_string());
        let terminal = self.terminal_network.contains(&key);
        let late_ordinary = terminal && matches!(value["method"].as_str(), Some("Network.responseReceivedExtraInfo" | "Network.requestWillBeSentExtraInfo")) && self.ordinary_late_extra_info(&value);
        if value.get("method").and_then(Value::as_str).is_some_and(|method| method.starts_with("Network.") && method != "Network.responseReceivedExtraInfo") && terminal && !late_ordinary { return }
        let guarded = self.blocked.contains(&key.0) || shield::loaded_urls_protected(&self.guard, &value["params"]) || self.hidden(&value) || self.redirects.contains(&key.1);
        if guarded && self.visible_redirect(&mut value) {
            let hop = value.get("method").and_then(Value::as_str) == Some("Network.requestWillBeSent");
            self.events.push(value.clone());
            if hop { self.fail_seen_network(&value); }
            return;
        }
        if terminal && !late_ordinary { return }
        if self.worlds.event(&value, &self.guard) {
            if value.get("method").and_then(Value::as_str) == Some("Debugger.paused") {
                let _ = self.send("Debugger.resume", json!({}), value.get("sessionId").and_then(Value::as_str));
            }
            return;
        }
        if guarded && !value.get("method").and_then(Value::as_str).unwrap_or("").starts_with("Target.") {
            self.fail_seen_network(&value);
            return;
        }
        self.adopt_guard_attach(&value);
        if self.blocked.contains(value.get("sessionId").and_then(Value::as_str).unwrap_or("")) || !value.get("method").and_then(Value::as_str).unwrap_or("").starts_with("Target.") && shield::loaded_urls_protected(&self.guard, &value["params"]) {
            self.fail_seen_network(&value);
            return;
        }
        let echoed_url = (value["method"] == "Network.requestWillBeSent" && value["params"]["type"] == "Document" && !guarded
            && !self.protected_body_requests.contains(&key.1) && self.client_navigation_urls.get(&key.0).is_some_and(|url| value.pointer("/params/request/url").and_then(Value::as_str) == Some(url.as_str()))).then(|| value["params"]["request"]["url"].clone());
        shield::redact_strings(&self.guard, &mut value);
        if let Some(url) = echoed_url { value["params"]["request"]["url"] = url; }
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
            if value.pointer("/params/resourceType").and_then(Value::as_str) == Some("Document") {
                self.paused_documents.insert((session.clone(), id.clone()));
            }
            self.hand(&session, id);
        }
        if method.ends_with("ExtraInfo") {
            if let Some(id) = id.clone() {
                if let Some(cookies) = value.pointer("/params/associatedCookies").and_then(Value::as_array) {
                    self.trace_cookies.extend(cookies.iter().filter_map(|item| item.get("cookie").cloned()));
                }
                shield::strip_protected_associated(&self.guard, &mut value);
                if self.requests.get(&id) != Some(&false) {
                    self.held.entry(id).or_default().push(value);
                    return;
                }
            }
        }
        if !terminal { self.visible_network(&value); }
        self.events.push(value);
        if let Some(id) = id {
            if self.requests.get(&id) == Some(&false) {
                if let Some(held) = self.held.remove(&id) {
                    for event in held { self.deliver(event); }
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
        self.raw_call_execution(method, params, session_id, timeout, false)
    }

    fn raw_call_execution(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
        pinned: bool,
    ) -> Result<Value, String> {
        let active = pinned.then_some(session_id).flatten();
        if let Some(session) = active { self.executing_sessions.insert(session.to_string()); }
        let result = self.raw_call_wait(method, params, session_id, timeout, pinned);
        if let Some(session) = active {
            self.executing_sessions.remove(session);
            if self.blocked.contains(session) { self.detach_blocked(session); }
        }
        result
    }

    fn raw_call_wait(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
        pinned: bool,
    ) -> Result<Value, String> {
        let frame_epoch = self.frame_epoch;
        let page = session_id.and_then(|session| self.session_target(session)).and_then(|target| self.page_scope_owner(&target))
            .map(|page| (page.clone(), self.page_scope_epochs.get(&page).copied().unwrap_or(0), self.page_scope_unknown));
        let result = self.raw_exchange(method, params, session_id, timeout, pinned)?;
        if method == "Page.getFrameTree" {
            let stable_page = page.is_some_and(|(page, epoch, unknown)| self.page_scope_epochs.get(&page).copied().unwrap_or(0) == epoch && self.page_scope_unknown == unknown);
            if !stable_page && !self.tree_current(&result, frame_epoch) {
                return Err(self.refusal());
            }
            self.record_frames(&result, session_id.unwrap_or(""));
        }
        Ok(result)
    }

    fn raw_exchange(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
        timeout: Duration,
        pinned: bool,
    ) -> Result<Value, String> {
        let id = self.send(method, params, session_id)?;
        self.waiting_calls.insert(id);
        let result = self.raw_reply(method, id, session_id, timeout, pinned);
        self.waiting_calls.remove(&id);
        self.call_replies.remove(&id);
        result
    }

    fn raw_reply(&mut self, method: &str, id: i64, session_id: Option<&str>, timeout: Duration, pinned: bool) -> Result<Value, String> {
        let deadline = Instant::now() + timeout;
        let was_blocked = session_id.is_some_and(|session| self.blocked.contains(session));
        loop {
            let value = match self.call_replies.remove(&id) {
                Some(value) => value,
                None => {
                    let text = match self.recv(deadline)? {
                        Some(text) => text,
                        None => return Err(if pinned && session_id.is_some_and(|session| self.blocked.contains(session)) { self.refusal() } else { format!("{method} timed out\n") }),
                    };
                    serde_json::from_str::<Value>(&text)
                        .map_err(|err| format!("CDP response was not JSON: {err}\n"))?
                }
            };
            if value.get("id").and_then(|item| item.as_i64()) == Some(id) {
                self.detach_reply(id, &value);
                if let Some(error) = value.get("error") {
                    if pinned && session_id.is_some_and(|session| self.blocked.contains(session)) { return Err(self.refusal()) }
                    let text = error
                        .get("message")
                        .and_then(|item| item.as_str())
                        .unwrap_or("CDP error");
                    let (text, _) = shield::redact_text(&self.guard, text);
                    return Err(format!("{method}: {text}\n"));
                }
                if method == "Target.attachToTarget" {
                    if let Some(session) = value.pointer("/result/sessionId").and_then(Value::as_str) {
                        self.blocked.remove(session);
                        self.detached_ordinary.remove(session);
                    }
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            let detached = value.get("method").and_then(|v| v.as_str())
                == Some("Target.detachedFromTarget")
                && value.pointer("/params/sessionId").and_then(|v| v.as_str()) == session_id;
            self.absorb(value);
            if let Some(session) = session_id {
                if detached || (!pinned && !was_blocked && self.blocked.contains(session)) {
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
        if method == "Target.detachFromTarget" {
            if let Some(session) = message.pointer("/params/sessionId").and_then(Value::as_str) {
                self.detaching.insert(id, session.to_string());
            }
        }
        Ok(id)
    }

    pub fn set_waiter(&mut self, waiter: Option<Waiter>) {
        self.waiter = waiter;
    }

    fn recv(&mut self, deadline: Instant) -> Result<Option<String>, String> {
        loop {
            let Some(mut waiter) = self.waiter.take() else { return self.ws.recv_text(deadline) };
            let now = Instant::now();
            if now >= self.waiter_due {
                waiter(self);
                self.waiter_due = Instant::now() + WAITER_SLICE;
            }
            self.waiter = Some(waiter);
            let until = deadline.min(self.waiter_due);
            if let Some(text) = self.ws.recv_text(until)? { return Ok(Some(text)) }
            if until == deadline { return Ok(None) }
        }
    }

    pub fn pump(&mut self, deadline: Instant) -> Result<bool, String> {
        self.webmcp_sweep();
        let result = match self.recv(deadline)? {
            Some(text) => {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    self.absorb(value);
                }
                Ok(true)
            }
            None => Ok(false),
        };
        self.webmcp_sweep();
        result
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

    fn detach_reply(&mut self, id: i64, value: &Value) {
        if let Some(session) = self.detaching.remove(&id) {
            if value.get("error").is_none() { self.forget_session(&session); }
        }
    }

    fn forget_session(&mut self, session: &str) {
        if !self.blocked.contains(session) && !self.guard_sessions.contains(session) {
            self.detached_ordinary.insert(session.to_string());
        }
        let target = self.session_target(session);
        if self.blocked.contains(session) {
            if let Some(target) = &target { self.protected_targets.insert(target.clone()); }
        }
        let guards: Vec<String> = self.guards.iter().filter(|(_, guard)| guard.as_str() == session).map(|(target, _)| target.clone()).collect();
        self.session_targets.remove(session);
        self.page_scope_sessions.retain(|_, owner| owner != session);
        self.guard_sessions.remove(session);
        self.blocked.insert(session.to_string());
        self.detaching.retain(|_, owner| owner != session);
        for target in guards {
            self.guards.remove(&target);
            self.documents.remove(&target);
            self.cached_documents.remove(&target);
            self.abandon(&target);
        }
        self.runtime_sessions.retain(|_, owner| owner != session);
        self.frame_sessions.remove(session);
        self.context_frames.retain(|(owner, _), _| owner != session);
        self.context_proofs.retain(|(owner, _), _| owner != session);
        self.created_contexts.retain(|(owner, _)| owner != session);
        self.executing_sessions.remove(session);
        self.dialog_sessions.remove(session);
        self.dialogs.remove(session);
        self.dialog_documents.remove(session);
        self.forget_paused(session);
        self.paused_documents.retain(|(owner, _)| owner != session);
        self.handed.retain(|(owner, _)| owner != session);
        self.answering.retain(|_, (owner, _)| owner != session);
        self.decided.retain(|_, (owner, _)| owner != session);
        self.held.remove(session);
        self.navigated_blanks.retain(|_, (owner, _, _)| owner != session);
        self.seen_network.retain(|(owner, _), _| owner != session);
        self.terminal_network.retain(|(owner, _)| owner != session);
        self.client_navigation_urls.remove(session);
        self.protected_request_pages.retain(|(owner, _)| owner != session);
        self.protected_resources.retain(|(owner, _)| owner != session);
        self.source_resources.retain(|(owner, _), _| owner != session);
        for resources in self.request_resources.values_mut() { resources.retain(|(owner, _)| owner != session); }
        for navigation in self.navigations.values_mut() {
            if navigation.session.as_deref() == Some(session) { navigation.detached = true; }
        }
        self.worlds.event(&json!({"method":"Target.detachedFromTarget","params":{"sessionId":session}}), &self.guard);
        self.issued.forget(session);
        self.webmcp.forget(session);
    }

    fn absorb(&mut self, mut value: Value) {
        if let Some(id) = value.get("id").and_then(|item| item.as_i64()) {
            self.detach_reply(id, &value);
            if self.waiting_calls.contains(&id) {
                self.call_replies.insert(id, value.clone());
            }
            if let Some(navigation) = self.navigations.get_mut(&id) {
                navigation.reply = Some(value);
                return;
            }
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
        self.observe_dialog(&value);
        if value.get("method").and_then(Value::as_str) == Some("Target.detachedFromTarget") {
            let session = value.pointer("/params/sessionId").and_then(Value::as_str);
            for navigation in self.navigations.values_mut() {
                if navigation.session.as_deref() == session { navigation.detached = true; }
            }
        }
        if self.webmcp_event(&value) {
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
        if value.get("method").and_then(Value::as_str) == Some("Fetch.authRequired") && self.guard_auth(&value) {
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
                if self.protected_body_requests.contains(network) { self.protect_body_request(network); }
            }
            if self.issued_hop(&value, &paused) { return; }
            let guard_only = self.guard_only(&paused);
            let authority_protected = shield::scheme_less_protected(&self.guard, &value["params"]["request"]);
            let redirect = self.actual_redirect_carrier(&value);
            if self.protected_carrier(&value, false) {
                self.protect_body_request(&paused.request_id);
                if let Some(network) = &paused.network_id { self.protect_body_request(network); self.protect_resource(&paused.session_id, &paused.url); }
            }
            let reaches = self.guard.url(&paused.full_url()) || redirect || authority_protected;
            let loaded_protected = self.guard.url(&paused.full_url()) || authority_protected;
            let protected = reaches;
            if loaded_protected {
                self.protect_resource(&paused.session_id, &paused.full_url());
            }
            if redirect {
                self.mark_redirect(&paused.request_id);
                if let Some(network) = &paused.network_id {
                    self.mark_redirect(network);
                    self.protect_resource(&paused.session_id, &paused.url);
                }
            }
            let initiated = self.initiated_by_protected(&paused);
            if self.redirect_page_protected(&value) || initiated {
                self.protected_request_pages.insert((paused.session_id.clone(), paused.request_id.clone()));
                if let Some(network) = &paused.network_id { self.protected_request_pages.insert((paused.session_id.clone(), network.clone())); }
            }
            #[cfg(test)]
            let observed_url = paused.url.clone();
            let ordinary_url = (!redirect && !loaded_protected && !initiated && !self.guard_sessions.contains(&paused.session_id) && !self.redirect_page_protected(&value)
                && paused.resource_type == "Document" && self.client_navigation_urls.get(&paused.session_id) == Some(&paused.url)
                && !self.protected_body_requests.contains(&paused.request_id)
                && self.ordinary_redirect_url(&paused.url)).then(|| paused.url.clone());
            shield::redact_strings(&self.guard, &mut value);
            if let Some(url) = ordinary_url { value["params"]["request"]["url"] = json!(url); }
            paused = Paused::from_event(&value).unwrap();
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
                    .push((paused.session_id.clone(), observed_url, reason));
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
                if redirect && !authority_protected && self.visible_redirect(&mut value) {
                    self.events.push(value);
                }
                self.answer_protected(&paused);
                return;
            }
            if self.blocked.contains(&paused.session_id) || initiated {
                self.fail_seen_network(&value);
                self.continue_plain(&paused.session_id, &paused.request_id);
                return;
            }
            if self.redirects.contains(&paused.request_id) && !redirect && !protected {
                self.continue_plain(&paused.session_id, &paused.request_id);
                return;
            }
            if guard_only {
                self.continue_plain(&paused.session_id, &paused.request_id);
                return;
            }
            if paused.resource_type == "Document" {
                self.paused_documents.insert((paused.session_id.clone(), paused.request_id.clone()));
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
        if self.defer_issued(&paused) { return }
        let Some(decider) = self.deciders.get_mut(&paused.session_id) else {
            self.paused.push_back(paused);
            return;
        };
        let decision = decider(&paused);
        let (method, params, issued) = match fetch::command(&self.guard, &paused, &decision)
            .and_then(|(method, params)| self.client_issued(method, &params, Some(&paused.session_id)).map(|issued| (method, params, issued))) {
            Ok(command) => command,
            Err(message) => {
                self.fetch_errors.push(message);
                let (method, params) = fetch::command(&self.guard, &paused, &Decision::default())
                    .expect("a plain continue is always allowed");
                (method, params, false)
            }
        };
        if issued {
            if let Err(message) = self.issue(&params, &paused.session_id) {
                self.fetch_errors.push(message);
                self.continue_plain(&paused.session_id, &paused.request_id);
                return;
            }
        }
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

fn frame_text_known(text: &str) -> bool {
    !matches!(text, "" | "null" | "://")
}

fn crosses_frames(method: &str, params: &Value) -> bool {
    matches!(method, "DOMSnapshot.captureSnapshot" | "DOMSnapshot.getSnapshot" | "DOM.getFlattenedDocument")
        || method.starts_with("DOM.") && (params.get("pierce").and_then(Value::as_bool) == Some(true)
            || params.get("depth").and_then(Value::as_i64).is_some_and(|depth| depth < 0 || depth > 1))
}

fn recovery(method: &str, params: &Value) -> bool {
    match method {
        "Page.enable" | "Page.disable" | "Page.navigate" | "Page.reload" | "Page.stopLoading" | "Page.navigateToHistoryEntry"
        | "Page.getNavigationHistory" | "Page.getFrameTree" | "Page.close" | "Runtime.enable" | "Runtime.disable"
        | "Runtime.runIfWaitingForDebugger" | "Network.enable" | "Network.disable" | "Log.enable" | "Log.disable"
        | "Fetch.enable" | "Fetch.disable" | "Fetch.continueRequest" | "Fetch.failRequest" | "Fetch.fulfillRequest"
        | "Fetch.continueResponse" | "Fetch.continueWithAuth" => true,
        "Runtime.evaluate" => {
            matches!(params.get("expression").and_then(Value::as_str), Some("document.readyState" | "0"))
                && params.as_object().is_some_and(|object| object.keys().all(|key| matches!(key.as_str(), "expression" | "returnByValue")))
        }
        _ => false,
    }
}

fn content_command(method: &str, params: &Value) -> Option<bool> {
    if method == "Runtime.evaluate"
        && matches!(params.get("expression").and_then(Value::as_str), Some("document.readyState" | "0" | "document.readyState === 'complete' ? location.href : ''"))
        && params.as_object().is_some_and(|object| object.keys().all(|key| matches!(key.as_str(), "expression" | "returnByValue")))
    { return Some(false) }
    if method == "Runtime.compileScript" && params.get("persistScript").and_then(Value::as_bool) != Some(false) { return None }
    if method == "Page.addScriptToEvaluateOnNewDocument" {
        return Some(params.get("runImmediately").is_some_and(|value| value != &json!(false)));
    }
    match method {
        "Debugger.setInstrumentationBreakpoint" | "Emulation.setVirtualTimePolicy" | "Overlay.setShowPaintRects"
        | "Page.addScriptToEvaluateOnLoad" | "Security.setIgnoreCertificateErrors" => None,
        "Runtime.evaluate" | "Runtime.callFunctionOn" | "Runtime.awaitPromise" | "Runtime.compileScript" | "Runtime.runScript"
        | "Runtime.getProperties" | "Runtime.getExceptionDetails" | "Runtime.queryObjects" | "Runtime.globalLexicalScopeNames" | "Runtime.getHeapUsage" | "Runtime.discardConsoleEntries"
        | "DOM.collectClassNamesFromSubtree" | "DOM.copyTo" | "DOM.describeNode" | "DOM.focus" | "DOM.getAttributes" | "DOM.getBoxModel" | "DOM.getContentQuads"
        | "DOM.getDocument" | "DOM.getFlattenedDocument" | "DOM.getNodesForSubtreeByStyle" | "DOM.getNodeForLocation" | "DOM.getOuterHTML" | "DOM.getRelayoutBoundary" | "DOM.getSearchResults"
        | "DOM.hideHighlight" | "DOM.highlightNode" | "DOM.highlightRect" | "DOM.markUndoableState" | "DOM.moveTo" | "DOM.performSearch" | "DOM.pushNodeByPathToFrontend"
        | "DOM.pushNodesByBackendIdsToFrontend" | "DOM.querySelector" | "DOM.querySelectorAll" | "DOM.redo" | "DOM.removeAttribute" | "DOM.removeNode" | "DOM.requestChildNodes" | "DOM.requestNode"
        | "DOM.resolveNode" | "DOM.setAttributeValue" | "DOM.setAttributesAsText" | "DOM.setFileInputFiles" | "DOM.setNodeName" | "DOM.setNodeValue" | "DOM.setOuterHTML" | "DOM.undo"
        | "DOM.getFrameOwner" | "DOM.getNodeStackTraces" | "DOM.getFileInfo" | "DOM.getDetachedDomNodes" | "DOM.getTopLayerElements" | "DOM.discardSearchResults" | "DOM.forceShowPopover"
        | "DOMSnapshot.captureSnapshot" | "DOMSnapshot.getSnapshot" | "DOMDebugger.getEventListeners"
        | "Input.dispatchDragEvent" | "Input.dispatchKeyEvent" | "Input.dispatchMouseEvent" | "Input.dispatchTouchEvent" | "Input.emulateTouchFromMouseEvent" | "Input.insertText"
        | "Input.synthesizePinchGesture" | "Input.synthesizeScrollGesture" | "Input.synthesizeTapGesture" | "Input.cancelDragging" | "Input.imeSetComposition"
        | "Accessibility.getFullAXTree" | "Accessibility.getPartialAXTree" | "Accessibility.getRootAXNode" | "Accessibility.getAXNodeAndAncestors" | "Accessibility.getChildAXNodes" | "Accessibility.queryAXTree"
        | "Page.captureScreenshot" | "Page.captureSnapshot" | "Page.printToPDF" | "Page.getResourceContent" | "Page.searchInResource" | "Page.getLayoutMetrics" | "Page.getPermissionsPolicyState" | "Page.setDocumentContent"
        | "Page.getAppManifest" | "Page.getInstallabilityErrors" | "Page.getAppId" | "Page.startScreencast" | "Page.generateTestReport"
        | "Network.getResponseBody" | "Network.getRequestPostData" | "Network.getResponseBodyForInterception" | "Network.searchInResponseBody" | "Network.loadNetworkResource"
        | "Network.streamResourceContent" | "Network.takeResponseBodyForInterceptionAsStream" | "Network.replayXHR" | "Audits.getEncodedResponse"
        | "Fetch.getResponseBody" | "Fetch.takeResponseBodyAsStream" | "IO.read" | "IO.close" | "IO.resolveBlob"
        | "Debugger.evaluateOnCallFrame" | "Debugger.getScriptSource" | "Debugger.searchInContent" | "Debugger.setScriptSource" | "Debugger.setVariableValue" | "Debugger.restartFrame" | "Debugger.stepInto" | "Debugger.stepOut" | "Debugger.stepOver"
        | "Debugger.getWasmBytecode" | "Debugger.disassembleWasmModule" | "Debugger.nextWasmDisassemblyChunk" | "Debugger.getPossibleBreakpoints" | "Debugger.pause"
        | "HeapProfiler.takeHeapSnapshot" | "HeapProfiler.getHeapObjectId" | "HeapProfiler.getObjectByHeapObjectId" | "HeapProfiler.getSamplingProfile" | "HeapProfiler.stopSampling" | "HeapProfiler.stopTrackingHeapObjects"
        | "DOMStorage.getDOMStorageItems" | "DOMStorage.setDOMStorageItem" | "DOMStorage.removeDOMStorageItem" | "DOMStorage.clear"
        | "Storage.clearDataForOrigin" | "Storage.clearDataForStorageKey" | "Storage.getUsageAndQuota" | "Storage.getStorageKeyForFrame"
        | "CacheStorage.requestCacheNames" | "CacheStorage.requestEntries" | "CacheStorage.requestCachedResponse" | "CacheStorage.deleteCache" | "CacheStorage.deleteEntry"
        | "IndexedDB.requestDatabaseNames" | "IndexedDB.requestDatabase" | "IndexedDB.requestData" | "IndexedDB.deleteDatabase" | "IndexedDB.deleteObjectStoreEntries" | "IndexedDB.clearObjectStore" | "IndexedDB.getMetadata"
        | "FileSystem.getDirectory"
        | "Target.sendMessageToTarget" | "Target.exposeDevToolsProtocol"
        | "Profiler.stop" | "Profiler.takePreciseCoverage" | "Profiler.getBestEffortCoverage" | "CSS.getStyleSheetText" | "CSS.getComputedStyleForNode" | "CSS.getMatchedStylesForNode" | "CSS.getInlineStylesForNode"
        | "CSS.setStyleSheetText" | "CSS.setStyleTexts" | "CSS.setEffectivePropertyValueForNode" | "CSS.getMediaQueries" | "CSS.getPlatformFontsForNode" => Some(true),
        "Page.enable" | "Page.disable" | "Page.navigate" | "Page.reload" | "Page.stopLoading" | "Page.navigateToHistoryEntry" | "Page.close" | "Page.bringToFront"
        | "Page.getFrameTree" | "Page.getResourceTree" | "Page.getNavigationHistory" | "Page.handleJavaScriptDialog" | "Page.createIsolatedWorld" | "Page.setLifecycleEventsEnabled" | "Page.removeScriptToEvaluateOnNewDocument"
        | "Page.setBypassCSP" | "Page.setWebLifecycleState" | "Page.setInterceptFileChooserDialog" | "Page.stopScreencast" | "Page.screencastFrameAck" | "Page.setDownloadBehavior" | "Page.getCookies" | "Page.deleteCookie"
        | "Runtime.enable" | "Runtime.disable" | "Runtime.runIfWaitingForDebugger" | "Runtime.addBinding" | "Runtime.removeBinding" | "Runtime.releaseObject" | "Runtime.releaseObjectGroup" | "Runtime.getIsolateId"
        | "Runtime.setAsyncCallStackDepth" | "Runtime.setCustomObjectFormatterEnabled" | "Runtime.setMaxCallStackSizeToCapture" | "Runtime.terminateExecution"
        | "DOM.enable" | "DOM.disable" | "DOM.setNodeStackTracesEnabled" | "DOMSnapshot.enable" | "DOMSnapshot.disable" | "Accessibility.enable" | "Accessibility.disable" | "CSS.enable" | "CSS.disable"
        | "Input.setIgnoreInputEvents" | "Input.setInterceptDrags" | "Fetch.enable" | "Fetch.disable" | "Fetch.continueRequest" | "Fetch.fulfillRequest" | "Fetch.failRequest" | "Fetch.continueWithAuth" | "Fetch.continueResponse"
        | "Network.enable" | "Network.disable" | "Network.setBypassServiceWorker" | "Network.setCacheDisabled" | "Network.setExtraHTTPHeaders" | "Network.setRequestInterception" | "Network.continueInterceptedRequest"
        | "Network.setUserAgentOverride" | "Network.setBlockedURLs" | "Network.emulateNetworkConditions" | "Network.emulateNetworkConditionsByRule" | "Network.overrideNetworkState"
        | "Network.getCookies" | "Network.getAllCookies" | "Network.setCookie" | "Network.setCookies" | "Network.deleteCookies" | "Network.clearBrowserCookies" | "Network.clearBrowserCache" | "Network.setAcceptedEncodings" | "Network.clearAcceptedEncodingsOverride"
        | "Target.attachToTarget" | "Target.attachToBrowserTarget" | "Target.detachFromTarget" | "Target.createTarget" | "Target.closeTarget" | "Target.activateTarget" | "Target.getTargets" | "Target.getTargetInfo"
        | "Target.setDiscoverTargets" | "Target.setAutoAttach" | "Target.autoAttachRelated" | "Target.createBrowserContext" | "Target.disposeBrowserContext" | "Target.getBrowserContexts"
        | "Browser.getVersion" | "Browser.close" | "Browser.getWindowForTarget" | "Browser.getWindowBounds" | "Browser.setWindowBounds" | "Browser.grantPermissions" | "Browser.resetPermissions" | "Browser.setPermission"
        | "Browser.setDownloadBehavior" | "Browser.cancelDownload" | "Browser.getBrowserCommandLine" | "Browser.getHistograms" | "Browser.getHistogram" | "Browser.executeBrowserCommand" | "Browser.setDockTile"
        | "Emulation.setCPUThrottlingRate" | "Emulation.setFocusEmulationEnabled" | "Emulation.setDeviceMetricsOverride" | "Emulation.clearDeviceMetricsOverride" | "Emulation.setTouchEmulationEnabled"
        | "Emulation.setUserAgentOverride" | "Emulation.setLocaleOverride" | "Emulation.setTimezoneOverride" | "Emulation.setEmulatedMedia" | "Emulation.setGeolocationOverride" | "Emulation.clearGeolocationOverride"
        | "Emulation.setScriptExecutionDisabled" | "Emulation.setEmulatedVisionDeficiency" | "Emulation.setEmitTouchEventsForMouse" | "Emulation.setScrollbarsHidden" | "Emulation.setDefaultBackgroundColorOverride"
        | "Debugger.enable" | "Debugger.disable" | "Debugger.resume" | "Debugger.setBreakpoint" | "Debugger.setBreakpointByUrl" | "Debugger.removeBreakpoint"
        | "Debugger.setBreakpointsActive" | "Debugger.setPauseOnExceptions" | "Debugger.setSkipAllPauses" | "Debugger.setAsyncCallStackDepth" | "Debugger.setBlackboxPatterns" | "Debugger.setBlackboxedRanges"
        | "Profiler.enable" | "Profiler.disable" | "Profiler.start" | "Profiler.startPreciseCoverage" | "Profiler.stopPreciseCoverage" | "Profiler.setSamplingInterval"
        | "HeapProfiler.enable" | "HeapProfiler.disable" | "HeapProfiler.startSampling" | "HeapProfiler.startTrackingHeapObjects" | "HeapProfiler.collectGarbage"
        | "Log.enable" | "Log.disable" | "Log.clear" | "Log.startViolationsReport" | "Log.stopViolationsReport" | "Audits.enable" | "Audits.disable"
        | "DOMStorage.enable" | "DOMStorage.disable"
        | "Storage.getCookies" | "Storage.setCookies" | "Storage.clearCookies"
        | "IndexedDB.enable" | "IndexedDB.disable" | "IndexedDB.releaseObject"
        | "ServiceWorker.enable" | "ServiceWorker.disable" | "ServiceWorker.unregister" | "ServiceWorker.startWorker" | "ServiceWorker.stopWorker" | "ServiceWorker.stopAllWorkers" | "ServiceWorker.updateRegistration"
        | "PWA.install" | "PWA.launch" | "PWA.uninstall" | "Extensions.loadUnpacked" | "Extensions.uninstall" | "Extensions.getExtensions" | "SystemInfo.getInfo" | "SystemInfo.getProcessInfo"
        | "Tracing.start" | "Tracing.end" | "Tracing.getCategories" | "Tracing.recordClockSyncMarker" | "Tracing.requestMemoryDump" | "Slicc.action" => Some(false),
        _ => None,
    }
}

fn storage_content(method: &str) -> bool {
    matches!(method, "Storage.getStorageKeyForFrame" | "FileSystem.getDirectory")
        || ["DOMStorage.", "IndexedDB.", "CacheStorage."].iter().any(|prefix| method.starts_with(prefix))
        || matches!(method, "Storage.clearDataForOrigin" | "Storage.clearDataForStorageKey" | "Storage.getUsageAndQuota")
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

include!("dialog.rs");
include!("front.rs");
include!("window.rs");

#[cfg(test)]
mod tests {
    use super::{first_stderr_line, probe_is_settled, ActionNote, Cdp, Transport};
    use serde_json::{json, Value};
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    include!("persistent_test.rs");
    include!("redirect_test.rs");
    include!("redirect_matrix_test.rs");
    include!("redirect_race_test.rs");
    include!("redirect_terminal_test.rs");
    include!("redirect_terminal_guard_test.rs");
    include!("redirect_c2_carrier_test.rs");
    include!("redirect_grok_test.rs");
    include!("redirect_scope_review_test.rs");
    include!("ordinary_network_review_test.rs");
    include!("pause_metadata_recovery_test.rs");
    include!("navigation_echo_test.rs");
    include!("refresh_review_test.rs");
    include!("refresh_forms_review_test.rs");
    include!("redirect_target_taint_test.rs");
    include!("redirect_resource_control_test.rs");
    include!("navigation_queue_test.rs");
    include!("redirect_fragment_test.rs");
    include!("redirect_review_test.rs");
    include!("redirect_relation_test.rs");
    include!("redirect_userinfo_test.rs");
    include!("guard_attach_tree_test.rs");
    include!("front_test.rs");
    include!("window_test.rs");
    include!("settled_test.rs");

    #[test]
    fn queued_raw_replies_keep_pinned_execution_and_guard_denial_semantics() {
        let mut cdp = pending(|_| Vec::new());
        cdp.blocked.insert("S".into());
        cdp.call_replies.insert(700, json!({"id":700,"error":{"message":"Cannot find context with specified id"}}));
        assert_eq!(cdp.raw_reply("Runtime.evaluate", 700, Some("S"), Duration::ZERO, true).unwrap_err(), cdp.refusal());
        cdp.call_replies.insert(701, json!({"id":701,"result":{"result":{"value":"ordinary-executed-value"}}}));
        assert_eq!(cdp.raw_reply("Runtime.evaluate", 701, Some("S"), Duration::ZERO, true).unwrap()["result"]["value"], "ordinary-executed-value");
        assert_eq!(cdp.raw_reply("Runtime.evaluate", 702, Some("S"), Duration::ZERO, true).unwrap_err(), cdp.refusal());
        assert!(cdp.call_replies.is_empty());
    }

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
        let mut cdp = Cdp::from_transport(Box::new(Pending {
            inbox: VecDeque::new(),
            answer,
        }));
        cdp.fixture_session("S");
        cdp
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

    fn error_page_client() -> Cdp {
        error_page_client_url("https://seven.sliccy.ai/path?secret#fragment")
    }

    fn error_page_client_url(url: &str) -> Cdp {
        let mut cdp = url_backend(url);
        cdp.session_targets.insert("S".into(), "T".into());
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":"https://example.test/"}}}));
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"https://example.test/","securityOrigin":"https://example.test"}}}));
        cdp
    }

    fn observe_document(cdp: &mut Cdp) {
        struct Observed {
            inner: Box<dyn Transport>,
            frame: Value,
            inbox: VecDeque<String>,
        }
        impl Transport for Observed {
            fn send_text(&mut self, text: &str) -> Result<(), String> {
                let call: Value = serde_json::from_str(text).unwrap();
                if call["method"] == "Target.getTargets" {
                    self.inbox.push_back(json!({"id":call["id"],"result":{"targetInfos":[{"targetId":"T","type":"page","url":self.frame["url"]}]}}).to_string());
                    Ok(())
                } else if call["method"] == "Page.getFrameTree" {
                    self.inbox.push_back(json!({"id":call["id"],"result":{"frameTree":{"frame":self.frame}}}).to_string());
                    Ok(())
                } else if call["method"] == "Runtime.enable" {
                    self.inbox.push_back(json!({"method":"Runtime.executionContextCreated","sessionId":call["sessionId"],"params":{"context":{"id":1,"uniqueId":"observed-context","origin":self.frame["securityOrigin"],"name":"","auxData":{"frameId":self.frame["id"],"isDefault":true}}}}).to_string());
                    self.inbox.push_back(json!({"id":call["id"],"result":{}}).to_string());
                    Ok(())
                } else {
                    self.inner.send_text(text)
                }
            }
            fn recv_text(&mut self, deadline: Instant) -> Result<Option<String>, String> {
                match self.inbox.pop_front() {
                    Some(text) => Ok(Some(text)),
                    None => self.inner.recv_text(deadline),
                }
            }
        }
        let doc = &cdp.documents["T"];
        let frame = json!({"id":doc.frame_id,"loaderId":"L","url":doc.url,"securityOrigin":doc.origin,"unreachableUrl":doc.unreachable_url});
        let inner = std::mem::replace(&mut cdp.ws, Box::new(Pending { inbox:VecDeque::new(), answer: |_| Vec::new() }));
        cdp.ws = Box::new(Observed { inner, frame, inbox:VecDeque::new() });
        cdp.guards.insert("T".into(), "G".into());
        cdp.guard_sessions.insert("G".into());
    }

    fn error_commit(cdp: &mut Cdp, unreachable: &str, origin: &str) {
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"chrome-error://chromewebdata/","securityOrigin":origin,"unreachableUrl":unreachable}}}));
    }

    #[test]
    fn protected_error_page_retains_allowed_recovery_and_redacts_discovery() {
        let url = "https://seven.sliccy.ai/path?secret#fragment";
        let mut cdp = error_page_client();
        error_commit(&mut cdp, url, "://");
        let info = json!({"targetId":"T","type":"page","url":url,"title":"SECRET"});
        for method in ["Target.targetCreated", "Target.targetInfoChanged", "Target.attachedToTarget"] {
            cdp.absorb(json!({"method":method,"params":{"sessionId":"A","targetInfo":info}}));
            let event = cdp.events().last().unwrap();
            assert_eq!(event["params"]["targetInfo"]["url"], "https://seven.sliccy.ai", "{method}");
            assert_eq!(event["params"]["targetInfo"]["title"], "https://seven.sliccy.ai", "{method}");
        }
        assert!(!cdp.target_protected("T", false).unwrap());
        assert!(!cdp.session_blocked("S"));
        let listed = cdp.call("Target.getTargets", json!({}), None).unwrap();
        assert_eq!(listed["targetInfos"][0]["url"], "https://seven.sliccy.ai");
        assert_eq!(listed["targetInfos"][0]["title"], "https://seven.sliccy.ai");
        let found = cdp.call("Target.getTargetInfo", json!({"targetId":"T"}), None).unwrap();
        assert_eq!(found["targetInfo"]["url"], "https://seven.sliccy.ai");
        assert_eq!(found["targetInfo"]["title"], "https://seven.sliccy.ai");
        observe_document(&mut cdp);
        for method in ["Page.reload", "Page.navigate", "Target.closeTarget"] {
            let (params, session) = if method == "Target.closeTarget" {
                (json!({"targetId":"T"}), None)
            } else {
                (json!({"url":"about:blank"}), Some("S"))
            };
            assert!(cdp.call(method, params, session).is_ok(), "{method}");
        }
        assert!(cdp.call("Page.navigate", json!({"url":url}), Some("S")).is_err());
    }

    #[test]
    fn protected_error_page_pending_report_does_not_change_previous_decision() {
        let mut cdp = error_page_client();
        cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":"S","params":{"frameId":"F","url":"https://seven.sliccy.ai/path?secret#fragment"}}));
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":"https://seven.sliccy.ai/path?secret#fragment"}}}));
        assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some("S")).is_err());
        error_commit(&mut cdp, "https://seven.sliccy.ai/path?secret#fragment", "://");
        assert!(!cdp.target_protected("T", false).unwrap());
        observe_document(&mut cdp);
        assert!(cdp.call("Page.reload", json!({}), Some("S")).is_ok());
    }

    #[test]
    fn protected_error_page_unknown_mismatch_origin_and_inflight_fail_closed() {
        let url = "https://seven.sliccy.ai/path?secret#fragment";
        for (unreachable, origin, navigating) in [(url, "://", true), ("", "://", false), ("https://seven.sliccy.ai/other", "://", false), (url, "null", false), (url, "https://example.test", false)] {
            let mut cdp = error_page_client();
            error_commit(&mut cdp, unreachable, origin);
            if navigating {
                cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":"S","params":{"frameId":"F","url":url}}));
            }
            assert!(cdp.target_protected("T", false).unwrap(), "{unreachable} {origin} {navigating}");
            assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some("S")).is_err());
        }
        let mut cdp = url_backend(url);
        cdp.session_targets.insert("S".into(), "T".into());
        error_commit(&mut cdp, url, "://");
        assert!(cdp.target_protected("T", false).unwrap());
    }

    #[test]
    fn protected_document_restore_and_error_page_sticky_lock() {
        let mut cdp = error_page_client();
        let url = "https://seven.sliccy.ai/path?secret#fragment";
        error_commit(&mut cdp, url, "://");
        assert!(!cdp.target_protected("T", false).unwrap());
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"type":"BackForwardCacheRestore","frame":{"id":"F","url":"https://seven.sliccy.ai/real-document","securityOrigin":"https://seven.sliccy.ai"}}}));
        assert!(cdp.is_protected_target("T"));
        error_commit(&mut cdp, url, "://");
        for method in ["Runtime.evaluate", "Page.reload", "Page.navigate", "Target.closeTarget", "Target.attachToTarget"] {
            let params = json!({"targetId":"T","url":"about:blank","flatten":true});
            assert!(cdp.call(method, params, Some("S")).is_err(), "{method}");
        }
    }

    #[test]
    fn protected_error_page_subframes_and_same_document_do_not_replace_evidence() {
        let mut cdp = error_page_client();
        let url = "https://seven.sliccy.ai/path?secret#fragment";
        error_commit(&mut cdp, url, "://");
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"CHILD","parentId":"F","url":"https://seven.sliccy.ai/child","securityOrigin":"https://seven.sliccy.ai"}}}));
        cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":"S","params":{"frameId":"F","navigationType":"sameDocument","url":url}}));
        assert!(!cdp.target_protected("T", false).unwrap());
        assert!(cdp.frame_protected("CHILD"));
        observe_document(&mut cdp);
        assert!(cdp.call("Page.reload", json!({}), Some("S")).is_ok());
    }

    #[test]
    fn protected_error_page_real_commit_and_guard_loss_fail_closed() {
        let url = "https://seven.sliccy.ai/path?secret#fragment";
        for origin in ["https://seven.sliccy.ai", "https://example.test"] {
            let mut cdp = error_page_client();
            error_commit(&mut cdp, url, "://");
            assert!(!cdp.target_protected("T", false).unwrap());
            cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":url,"securityOrigin":origin}}}));
            assert!(cdp.is_protected_target("T"));
            assert!(cdp.session_blocked("S"));
        }
        let mut cdp = error_page_client();
        error_commit(&mut cdp, url, "://");
        cdp.guards.insert("T".into(), "G".into());
        cdp.guard_sessions.insert("G".into());
        cdp.absorb(json!({"method":"Target.detachedFromTarget","params":{"sessionId":"G"}}));
        assert!(cdp.target_protected("T", false).unwrap());
        assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some("S")).is_err());
    }

    #[test]
    fn protected_error_page_ordinary_navigation_and_fresh_dns_error_remain_drivable() {
        let mut cdp = url_backend("https://example.test/next");
        cdp.session_targets.insert("S".into(), "T".into());
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"https://example.test/","securityOrigin":"https://example.test"}}}));
        cdp.absorb(json!({"method":"Page.frameStartedNavigating","sessionId":"S","params":{"frameId":"F","url":"https://example.test/next"}}));
        assert!(!cdp.target_protected("T", false).unwrap());
        observe_document(&mut cdp);
        assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.title"}), Some("S")).is_ok());
        let mut cdp = url_backend("http://ordinary.invalid/");
        cdp.session_targets.insert("S".into(), "T".into());
        error_commit(&mut cdp, "http://ordinary.invalid/", "://");
        assert!(!cdp.target_protected("T", false).unwrap());
        observe_document(&mut cdp);
        assert!(cdp.call("Page.navigate", json!({"url":"about:blank"}), Some("S")).is_ok());
    }

    #[test]
    fn protected_error_page_path_query_and_fragment_redact_url_and_title_everywhere() {
        for url in [
            "https://seven.sliccy.ai/path-secret",
            "https://seven.sliccy.ai/?query-secret=hidden",
            "https://seven.sliccy.ai/#fragment-secret-hidden",
            "https://seven.sliccy.ai/path-secret?query-secret=hidden#fragment-secret-hidden",
        ] {
            let mut cdp = error_page_client_url(url);
            error_commit(&mut cdp, url, "://");
            let info = json!({"targetId":"T","type":"page","url":url,"title":url});
            for method in ["Target.targetCreated", "Target.targetInfoChanged", "Target.attachedToTarget"] {
                cdp.absorb(json!({"method":method,"params":{"sessionId":"A","targetInfo":info}}));
                let event = cdp.events().last().unwrap();
                for key in ["url", "title"] {
                    assert_eq!(event["params"]["targetInfo"][key], "https://seven.sliccy.ai", "{method} {key} {url}");
                }
            }
            for method in ["Target.getTargets", "Target.getTargetInfo"] {
                let out = cdp.call(method, json!({"targetId":"T"}), None).unwrap();
                let info = if method == "Target.getTargets" { &out["targetInfos"][0] } else { &out["targetInfo"] };
                for key in ["url", "title"] {
                    assert_eq!(info[key], "https://seven.sliccy.ai", "{method} {key} {url}");
                }
            }
            assert!(!cdp.target_protected("T", false).unwrap());
            assert!(!cdp.session_blocked("S"));
        }
    }

    #[test]
    fn protected_seven_fragment_targets_and_url_changes_never_disclose_markers() {
        for url in [
            "https://seven.sliccy.ai/auth/callback#access_token=MARK",
            "https://seven.sliccy.ai/#tailscale=MARK1",
            "https://seven.sliccy.ai/#proxy=http%3A%2F%2F127.0.0.1%3A9%2F&key=MARK2",
        ] {
            for changed in [false, true] {
                let mut cdp = url_backend(url);
                if changed {
                    cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":"https://seven.sliccy.ai/","title":"clean"}}}));
                }
                let info = json!({"targetId":"T","type":"page","url":url,"title":url});
                for method in ["Target.targetCreated", "Target.targetInfoChanged", "Target.attachedToTarget"] {
                    let mark = cdp.events().len();
                    cdp.absorb(json!({"method":method,"params":{"sessionId":"A","targetInfo":info}}));
                    for event in &cdp.events()[mark..] {
                        assert!(!event.to_string().contains("MARK"), "{method} {changed} {event}");
                        for key in ["url", "title"] {
                            assert_eq!(event["params"]["targetInfo"][key], "https://seven.sliccy.ai", "{method} {key} {url}");
                        }
                    }
                }
                for method in ["Target.getTargets", "Target.getTargetInfo"] {
                    let params = if method == "Target.getTargets" { json!({}) } else { json!({"targetId":"T"}) };
                    let out = cdp.call(method, params, None).unwrap();
                    assert!(!out.to_string().contains("MARK"), "{method} {out}");
                    let info = if method == "Target.getTargets" { &out["targetInfos"][0] } else { &out["targetInfo"] };
                    for key in ["url", "title"] {
                        assert_eq!(info[key], "https://seven.sliccy.ai", "{method} {key} {url}");
                    }
                }
                cdp.absorb(json!({"method":"Target.detachedFromTarget","params":{"sessionId":"A","targetId":"T"}}));
                assert!(!cdp.events().last().unwrap().to_string().contains("MARK"));
                assert!(cdp.target_protected("T", false).unwrap());
            }
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

    #[test]
    fn allowed_blank_commit_refreshes_stale_protected_discovery_before_driving() {
        let mut cdp = url_backend("about:blank");
        cdp.session_targets.insert("S".into(), "T".into());
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"https://example.test/","securityOrigin":"https://example.test"}}}));
        error_commit(&mut cdp, "https://seven.sliccy.ai/doc#access_token=MARK", "://");
        cdp.absorb(json!({"method":"Target.targetInfoChanged","params":{"targetInfo":{"targetId":"T","type":"page","url":"https://seven.sliccy.ai/doc#access_token=MARK","title":"MARK"}}}));
        cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"about:blank","securityOrigin":"https://example.test"}}}));
        observe_document(&mut cdp);
        assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.readyState"}), Some("S")).is_ok());
        assert!(!cdp.session_blocked("S"));
        assert!(!cdp.is_protected_target("T"));
    }

    #[test]
    fn permitted_error_document_redacts_history_and_refuses_runtime_without_losing_ids() {
        struct HistoryResponses(UrlBackend);
        impl Transport for HistoryResponses {
            fn send_text(&mut self, text: &str) -> Result<(), String> {
                let call: Value = serde_json::from_str(text).unwrap();
                let protected = self.0.url.clone();
                let output = match call["method"].as_str().unwrap() {
                    "Page.getNavigationHistory" => Some(json!({"currentIndex":1,"entries":[
                        {"id":701,"url":"https://ordinary.example/ready","title":"ordinary control","userTypedURL":"https://ordinary.example/ready"},
                        {"id":702,"url":protected,"title":protected,"userTypedURL":protected}
                    ]})),
                    "Runtime.evaluate" => Some(json!({"result":{"type":"object","value":{"url":protected,"text":protected}}})),
                    _ => None,
                };
                if let Some(output) = output {
                    self.0.inbox.push_back(json!({"id":call["id"],"result":output}).to_string());
                    Ok(())
                } else {
                    self.0.send_text(text)
                }
            }
            fn recv_text(&mut self, until: Instant) -> Result<Option<String>, String> {
                self.0.recv_text(until)
            }
        }
        for attempted in [
            "https://seven.sliccy.ai/private/HISTORY_MARK?key=HISTORY_MARK#access_token=HISTORY_MARK",
            "https://vault.sliccy.ai/private/HISTORY_MARK?key=HISTORY_MARK#access_token=HISTORY_MARK",
        ] {
            let mut cdp = Cdp::from_transport(Box::new(HistoryResponses(UrlBackend { url:attempted.into(), inbox:VecDeque::new() })));
            cdp.session_targets.insert("S".into(), "T".into());
            cdp.absorb(json!({"method":"Page.frameNavigated","sessionId":"S","params":{"frame":{"id":"F","url":"https://ordinary.example/ready","securityOrigin":"https://ordinary.example"}}}));
            error_commit(&mut cdp, attempted, "://");
            assert!(!cdp.target_protected("T", false).unwrap());
            assert!(!cdp.session_blocked("S"));
            observe_document(&mut cdp);
            let origin = url::Url::parse(attempted).unwrap().origin().ascii_serialization();
            let history = cdp.call("Page.getNavigationHistory", json!({}), Some("S")).unwrap();
            assert_eq!(history["currentIndex"], 1);
            assert_eq!(history["entries"][0]["id"], 701);
            assert_eq!(history["entries"][0]["url"], "https://ordinary.example/ready");
            assert_eq!(history["entries"][0]["title"], "ordinary control");
            assert_eq!(history["entries"][1]["id"], 702);
            for key in ["url", "title", "userTypedURL"] {
                assert_eq!(history["entries"][1][key], origin, "{key}");
            }
            assert!(!history.to_string().contains("HISTORY_MARK"));
            let sent = cdp.next_id;
            for expression in ["location.href", "Array.from(document.documentElement.innerHTML, c => c.codePointAt(0))"] {
                assert!(cdp.call("Runtime.evaluate", json!({"expression":expression,"returnByValue":true}), Some("S")).is_err(), "{expression}");
            }
            assert_eq!(cdp.next_id, sent + 2);
            assert!(cdp.call("Page.reload", json!({}), Some("S")).is_ok());
            assert!(!cdp.session_blocked("S"));
        }
    }

    #[test]
    fn internal_guard_loss_refuses_direct_commands_even_with_stale_ordinary_discovery() {
        for error in [false, true] {
            let mut cdp = error_page_client();
            if error {
                error_commit(&mut cdp, "https://seven.sliccy.ai/path?secret#fragment", "://");
            }
            assert_eq!(cdp.targets["T"]["url"], "https://example.test/");
            assert!(!cdp.session_blocked("S"));
            cdp.guards.insert("T".into(), "G".into());
            cdp.guard_sessions.insert("G".into());
            cdp.absorb(json!({"method":"Target.detachedFromTarget","params":{"sessionId":"G","targetId":"T"}}));
            let sent = cdp.next_id;
            let result = cdp.call("Runtime.evaluate", json!({"expression":"location.href = 'https://seven.sliccy.ai/'"}), Some("S"));
            assert!(result.is_err(), "guard loss must refuse before any discovery refresh");
            assert_eq!(cdp.next_id, sent, "no Runtime or discovery command may reach the backend");
            assert!(cdp.session_blocked("S"));
            assert!(cdp.abandoned.contains("T"));
            assert!(cdp.is_protected_target("T"));
            assert!(cdp.call("Target.attachToTarget", json!({"targetId":"T","flatten":true}), None).is_err());
        }
    }

    #[test]
    fn caller_and_unknown_detach_do_not_abandon_a_healthy_guard() {
        let mut cdp = error_page_client_url("https://example.test/");
        cdp.guards.insert("T".into(), "G".into());
        cdp.guard_sessions.insert("G".into());
        for detached in ["UNKNOWN", "OTHER"] {
            cdp.session_targets.insert("OTHER".into(), "T".into());
            cdp.absorb(json!({"method":"Target.detachedFromTarget","params":{"sessionId":detached,"targetId":"T"}}));
            assert_eq!(cdp.guards["T"], "G");
            assert!(cdp.documents.contains_key("T"));
            assert!(!cdp.abandoned.contains("T"));
            assert!(!cdp.session_blocked("S"));
            observe_document(&mut cdp);
            assert!(cdp.call("Runtime.evaluate", json!({"expression":"document.readyState"}), Some("S")).is_ok());
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
        let mut cdp = Cdp::from_transport(Box::new(UrlBackend { url: url.into(), inbox: VecDeque::new() }));
        cdp.fixture_session("S");
        cdp
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
            assert!(cdp.call("Storage.getUsageAndQuota", json!({"origin":url}), None).is_err(), "{url}");
            assert!(cdp.call("Storage.clearDataForStorageKey", json!({"storageKey":format!("{url}^0{url}")}), None).is_err(), "{url}");
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
