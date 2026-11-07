use super::{transport_error, Cdp, DOCUMENT_CHECK, OPTIONAL_STEP};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

const SHIM: &str = include_str!("webmcp-shim.js");
const SHIM_HASH: &str = "911b9c665b7bf8183de1d4eeebb4a96d72ddcfe7b6b222316998c595b410fe53";
const DEFAULT_ID_CAP: usize = 4096;
const DEFAULT_PENDING_COUNT: usize = 4096;
const DEFAULT_PENDING_BYTES: usize = 64 << 20;
const MAX_TREE_DEPTH: usize = 64;
const MAX_TREE_NODES: usize = 4096;
const ID_CAP_ENV: &str = "SLICC_WEBMCP_ID_CAP";
const PENDING_COUNT_ENV: &str = "SLICC_WEBMCP_PENDING_CAP";
const PENDING_BYTES_ENV: &str = "SLICC_WEBMCP_PENDING_BYTES";

fn lower_only(default: usize, name: &str) -> usize {
    std::env::var(name).ok().and_then(|text| text.parse::<usize>().ok()).filter(|&n| n > 0).map(|n| n.min(default)).unwrap_or(default)
}

fn tree_bounded(root: &Value) -> bool {
    let mut stack = vec![(root, 1usize)];
    let mut nodes = 0usize;
    while let Some((node, depth)) = stack.pop() {
        nodes += 1;
        if depth > MAX_TREE_DEPTH || nodes > MAX_TREE_NODES { return false; }
        for child in node.get("childFrames").and_then(Value::as_array).into_iter().flatten() { stack.push((child, depth + 1)); }
    }
    true
}

fn gate_bytes(value: &Value) -> usize {
    value.to_string().len()
}

fn vetted_source(source: &str) -> Result<&str, String> {
    if crate::hash::sha256_hex(source.as_bytes()) != SHIM_HASH { return Err("slicc: refused: WebMCP shim integrity check failed\n".into()); }
    Ok(source)
}

fn private_bridge(fill: impl FnOnce(&mut [u8]) -> Result<(), getrandom::Error>) -> Result<String, String> {
    let mut bytes = [0; 16];
    fill(&mut bytes).map_err(|_| "slicc: refused: WebMCP bridge entropy unavailable\n")?;
    Ok(format!("__wmcp_{}", bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()))
}

struct Arm {
    source: String,
    bridge: String,
    identifier: Option<String>,
    conversion: Option<((String, Context), Instant, Instant)>,
    plain: bool,
    direct_child_attach: bool,
    root_identity: Option<(String, Context)>,
    pending_children: BTreeMap<i64, (Context, (String, Context), Instant)>,
    child_done: BTreeSet<(String, String, String)>,
    admitted: usize,
    revoked: bool,
    revoked_at: Option<(&'static str, u32)>,
    revoked_reason: Option<&'static str>,
    pending_contexts: BTreeMap<i64, Instant>,
    pending_install: Option<Instant>,
    pending_install_loader: Option<String>,
    pending_frames: BTreeMap<String, (String, Instant)>,
    accepted: BTreeSet<(String, String, String)>,
    installing: BTreeSet<(String, String, String)>,
    loaders: BTreeMap<String, String>,
    documents: BTreeMap<String, Value>,
    parked_origin: String,
    context_generation: u64,
    frame_accept_generation: BTreeMap<String, u64>,
}

#[derive(Clone, PartialEq)]
struct Context {
    frame: String,
    origin: String,
    unique: String,
    loader: String,
    generation: u64,
}

#[derive(Default)]
pub(super) struct Webmcp {
    arms: BTreeMap<String, Arm>,
    holders: BTreeMap<String, String>,
    contexts: BTreeMap<(String, i64), Context>,
    scripts: BTreeMap<(String, String), i64>,
    evaluations: BTreeSet<(String, String)>,
    pub(super) paused: BTreeSet<String>,
    handling_gate: bool,
    handling_children: bool,
    pending_gates: VecDeque<Value>,
    pending_gate_bytes: usize,
    id_cap_override: usize,
    pending_count_override: usize,
    pending_bytes_override: usize,
    pub(super) deferred: BTreeSet<String>,
    pub(super) quarantined: BTreeSet<String>,
    parents: BTreeMap<String, String>,
    children: Vec<(String, String, String)>,
    samples: Vec<u128>,
    requested_origins: BTreeSet<String>,
    route_attested: bool,
    buffering_attach: bool,
    pending_attaches: VecDeque<Value>,
    trace_sequence: u64,
    trace_lifecycle: BTreeMap<(String, &'static str), u64>,
    trace_requests: BTreeMap<(String, String), (String, u64)>,
    trace_fetch: BTreeMap<String, Value>,
    trace_roles: BTreeMap<String, &'static str>,
}

impl Webmcp {
    fn id_cap(&self) -> usize {
        match self.id_cap_override {
            0 => lower_only(DEFAULT_ID_CAP, ID_CAP_ENV),
            n => n.min(DEFAULT_ID_CAP).max(1),
        }
    }

    fn pending_count_cap(&self) -> usize {
        match self.pending_count_override {
            0 => lower_only(DEFAULT_PENDING_COUNT, PENDING_COUNT_ENV),
            n => n.min(DEFAULT_PENDING_COUNT).max(1),
        }
    }

    fn pending_bytes_cap(&self) -> usize {
        match self.pending_bytes_override {
            0 => lower_only(DEFAULT_PENDING_BYTES, PENDING_BYTES_ENV),
            n => n.min(DEFAULT_PENDING_BYTES).max(1),
        }
    }

    fn drop_pending(&mut self, value: &Value) {
        self.pending_gate_bytes = self.pending_gate_bytes.saturating_sub(gate_bytes(value));
    }

    fn session_script_count(&self, session: &str) -> usize {
        self.scripts.keys().filter(|(owner, _)| owner == session).count()
    }

    fn session_evaluation_count(&self, session: &str) -> usize {
        self.evaluations.iter().filter(|(owner, _)| owner == session).count()
    }

    fn forget_context_ids(scripts: &mut BTreeMap<(String, String), i64>, evaluations: &mut BTreeSet<(String, String)>, session: &str, context: i64) {
        let doomed = scripts.iter().filter(|((owner, _), id)| owner == session && **id == context).map(|((_, script), _)| script.clone()).collect::<Vec<_>>();
        for script in doomed {
            scripts.remove(&(session.to_string(), script.clone()));
            evaluations.remove(&(session.to_string(), script));
        }
    }

    pub(super) fn forget(&mut self, session: &str) {
        self.arms.remove(session);
        self.holders.retain(|_, owner| owner != session);
        self.contexts.retain(|(owner, _), _| owner != session);
        self.scripts.retain(|(owner, _), _| owner != session);
        self.evaluations.retain(|(owner, _)| owner != session);
        self.paused.remove(session);
        let mut dropped = 0;
        self.pending_gates.retain(|value| {
            let matches = value.get("sessionId").and_then(Value::as_str) == Some(session) || value.pointer("/params/sessionId").and_then(Value::as_str) == Some(session);
            if matches { dropped += gate_bytes(value); }
            !matches
        });
        self.pending_gate_bytes = self.pending_gate_bytes.saturating_sub(dropped);
        self.deferred.remove(session);
        self.quarantined.remove(session);
        self.parents.retain(|child, parent| child != session && parent != session);
        self.children.retain(|(_, owner, _)| owner != session);
    }
}

impl Cdp {
    pub fn start_webmcp(&mut self, session: &str) -> Result<String, String> {
        self.start_webmcp_for(session, false)
    }

    pub fn start_webmcp_for(&mut self, session: &str, reload: bool) -> Result<String, String> {
        let session = self.webmcp_session(session).to_string();
        let result = self.webmcp_start_once(&session, reload);
        if result.is_err() { self.stop_webmcp(&session); }
        else { self.webmcp_install_children(); }
        result
    }

    fn webmcp_start_once(&mut self, session: &str, reload: bool) -> Result<String, String> {
        if let Some(arm) = self.webmcp.arms.get(session).filter(|arm| !arm.revoked) { return Ok(arm.bridge.clone()); }
        self.admit("Runtime.enable", &json!({}), Some(session))?;
        let tree = self.webmcp_stable(session, Instant::now() + DOCUMENT_CHECK, |cdp, left| {
            let target = cdp.session_target(session).ok_or_else(|| cdp.refusal())?;
            let guard = cdp.guards.get(&target).cloned().ok_or_else(|| cdp.refusal())?;
            let tree = cdp.raw_call("Page.getFrameTree", json!({}), Some(&guard), left)?;
            if !cdp.webmcp_tree_verified(&tree, None) || cdp.content_blocked(session) || cdp.unobserved.contains(&target) {
                return Err(cdp.refusal());
            }
            Ok(tree)
        })?;
        let bridge = self.webmcp_install(session, &tree)?;
        if reload {
            let from = self.events.len();
            self.call_for("Page.reload", json!({}), Some(session), DOCUMENT_CHECK)?;
            if self.webmcp_plain(session) {
                self.webmcp_plain_reload(session, &tree, from).map_err(|message| format!("{message}webmcp-polyfill: other-extension fallback could not verify the reloaded document; tools are unavailable\n"))?;
            }
            let holder = self.webmcp_session(session).to_string();
            let session = holder.as_str();
            let until = Instant::now() + DOCUMENT_CHECK;
            while self.webmcp.arms.get(session).is_some_and(|arm| (arm.admitted == 0 || !arm.pending_contexts.is_empty()) && !arm.revoked) && Instant::now() < until {
                if !self.pump(until)? { break; }
            }
        } else { self.webmcp_install_current(session)?; }
        let holder = self.webmcp_session(session).to_string();
        let session = holder.as_str();
        if self.webmcp.arms.get(session).is_none_or(|arm| arm.revoked || arm.admitted == 0 || !arm.pending_contexts.is_empty()) {
            self.webmcp_revoke(session);
            return Err("slicc: refused: WebMCP installation could not verify the document; update the browser guard and client\n".into());
        }
        Ok(bridge)
    }

    pub fn webmcp_session<'a>(&'a self, session: &'a str) -> &'a str {
        self.webmcp.holders.get(session).map(String::as_str).unwrap_or(session)
    }

    pub fn webmcp_plain(&self, session: &str) -> bool {
        self.webmcp.arms.get(self.webmcp_session(session)).is_some_and(|arm| arm.plain)
    }

    fn webmcp_plain_reload(&mut self, session: &str, previous: &Value, from: usize) -> Result<(), String> {
        let until = Instant::now() + DOCUMENT_CHECK;
        let root = &previous["frameTree"]["frame"];
        while !self.events.iter().skip(from).any(|event| event["sessionId"] == session && event["method"] == "Page.frameNavigated"
            && event["params"]["frame"]["id"] == root["id"] && event["params"]["frame"]["loaderId"] != root["loaderId"] && event["params"]["frame"].get("parentId").is_none()) {
            if Instant::now() >= until || self.content_blocked(session) || !self.pump(until)? { return Err(self.refusal()); }
        }
        self.webmcp_plain_replay(session, until)?;
        while !self.events.iter().skip(from).any(|event| event["sessionId"] == session && event["method"] == "Page.loadEventFired") {
            if Instant::now() >= until || self.content_blocked(session) || !self.pump(until)? { return Err(self.refusal()); }
        }
        let holder = self.webmcp_session(session).to_string();
        self.webmcp_plain_proof(&holder)
    }

    fn webmcp_plain_replay(&mut self, session: &str, until: Instant) -> Result<(), String> {
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let attached = self.call_for("Target.attachToTarget", json!({"targetId":target,"flatten":true}), None, until.saturating_duration_since(Instant::now()))?;
        let fresh = attached.get("sessionId").and_then(Value::as_str).filter(|value| !value.is_empty()).ok_or_else(|| self.refusal())?.to_string();
        let result = (|| {
            let arm = self.webmcp.arms.remove(session).ok_or_else(|| self.refusal())?;
            self.webmcp.arms.insert(fresh.clone(), arm);
            self.webmcp_plain_enable(&fresh, until)?;
            self.webmcp_install_root(&fresh)?;
            self.call_for("Page.enable", json!({}), Some(&fresh), until.saturating_duration_since(Instant::now()))?;
            Ok(())
        })();
        if result.is_ok() {
            self.webmcp.holders.insert(session.to_string(), fresh);
            return result;
        }
        if let Some(arm) = self.webmcp.arms.remove(&fresh) {
            self.webmcp.arms.insert(session.to_string(), arm);
        }
        let _ = self.call_for("Target.detachFromTarget", json!({"sessionId":fresh}), None, until.saturating_duration_since(Instant::now()));
        result
    }

    fn webmcp_plain_enable(&mut self, session: &str, deadline: Instant) -> Result<(), String> {
        let deadline = deadline.min(Instant::now() + Duration::from_secs(1));
        loop {
            let result = self.call_for("Runtime.enable", json!({}), Some(session), deadline.saturating_duration_since(Instant::now()));
            let Err(message) = result else { return Ok(()); };
            let refusal = message.strip_prefix("Runtime.enable: ").unwrap_or(&message).starts_with("slicc: refused:");
            let left = deadline.saturating_duration_since(Instant::now());
            if !refusal || left.is_zero() { return Err(message); }
            std::thread::sleep(left.min(Duration::from_millis(100)));
            if Instant::now() >= deadline { return Err(message); }
        }
    }

    fn webmcp_stable<T>(&mut self, session: &str, deadline: Instant, mut attempt: impl FnMut(&mut Self, Duration) -> Result<T, String>) -> Result<T, String> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || self.blocked.contains(session) || self.webmcp.arms.get(session).is_some_and(|arm| arm.revoked) { return Err(self.refusal()); }
            let epoch = self.frame_epoch;
            let result = attempt(self, left);
            if result.as_ref().is_err_and(|message| epoch != self.frame_epoch && !transport_error(message)) { continue; }
            return result;
        }
    }

    pub fn webmcp_auto_attach(&mut self, session: &str) -> Result<(), String> {
        let session = self.webmcp_session(session).to_string();
        if !self.webmcp_alive(&session) { return Err(self.refusal()); }
        if self.webmcp_route_attested() {
            if let Some(arm) = self.webmcp.arms.get_mut(&session) { arm.direct_child_attach = false; }
            return Ok(());
        }
        if let Some(arm) = self.webmcp.arms.get_mut(&session) { arm.direct_child_attach = true; }
        self.webmcp.buffering_attach = true;
        let result = self.raw_call("Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":false,"flatten":true,"filter":[{"type":"iframe","exclude":false}]}), Some(&session), OPTIONAL_STEP);
        self.webmcp.buffering_attach = false;
        let pending = std::mem::take(&mut self.webmcp.pending_attaches);
        match result {
            Ok(_) => {
                let attested = self.webmcp.route_attested;
                if let Some(arm) = self.webmcp.arms.get_mut(&session) { arm.direct_child_attach = !attested; }
                for event in pending {
                    if attested {
                        if let Some(child) = event.pointer("/params/sessionId").and_then(Value::as_str) { self.webmcp_release_child(child); }
                    } else {
                        self.webmcp_adopt_child(&event);
                    }
                }
            }
            Err(_) => {
                if let Some(arm) = self.webmcp.arms.get_mut(&session) { arm.direct_child_attach = false; }
                for event in pending {
                    if let Some(child) = event.pointer("/params/sessionId").and_then(Value::as_str) {
                        self.webmcp_release_child(child);
                    }
                }
            }
        }
        Ok(())
    }

    fn webmcp_route_attested(&mut self) -> bool {
        if self.webmcp.route_attested { return true; }
        #[cfg(target_os = "wasi")]
        {
            self.webmcp.route_attested = true;
            return true;
        }
        #[cfg(not(target_os = "wasi"))]
        {
            let version = self.raw_call("Browser.getVersion", json!({}), None, OPTIONAL_STEP);
            let attested = Self::webmcp_version_attested(version);
            if attested { self.webmcp_route_hint(); }
            attested
        }
    }

    fn webmcp_version_attested(version: Result<Value, String>) -> bool {
        let Ok(Value::Object(version)) = version else { return true; };
        let slicc = match version.get("slicc") {
            None | Some(Value::Null) => return false,
            Some(Value::Object(slicc)) => slicc,
            Some(_) => return true,
        };
        match slicc.get("route") {
            None => false,
            Some(Value::String(_)) => true,
            Some(Value::Object(route)) => route.get("kind").is_some(),
            Some(_) => true,
        }
    }

    pub(super) fn webmcp_route_hint(&mut self) {
        self.webmcp.route_attested = true;
        let holders = self.webmcp.arms.iter().filter(|(_, arm)| arm.direct_child_attach).map(|(session, _)| session.clone()).collect::<Vec<_>>();
        for arm in self.webmcp.arms.values_mut() { arm.direct_child_attach = false; }
        for holder in holders { self.webmcp_detach_children(&holder); }
    }

    pub fn webmcp_request_origins(&mut self, origins: &[String]) {
        let mut added = false;
        for origin in origins {
            added |= self.webmcp.requested_origins.insert(origin.clone());
        }
        if added { self.webmcp_install_children(); self.webmcp_finish_awaiting_children(); }
    }

    pub fn webmcp_child_evidence(&self) -> bool {
        self.webmcp.arms.values().any(|arm| arm.direct_child_attach)
            || self.webmcp.parents.keys().any(|child| self.webmcp_alive(child))
            || self.webmcp.arms.iter().any(|(session, arm)| {
                let Some(root) = arm.root_identity.as_ref() else { return false; };
                self.webmcp.contexts.iter().any(|((owner, _), context)| {
                    owner == session && context.origin != root.1.origin && arm.accepted.contains(&(context.frame.clone(), context.loader.clone(), context.unique.clone()))
                })
            })
    }

    pub fn webmcp_set_route_attested(&mut self, attested: bool) {
        if attested { self.webmcp_route_hint(); } else { self.webmcp.route_attested = false; }
    }

    pub fn webmcp_route_attested_snapshot(&self) -> bool {
        self.webmcp.route_attested
    }

    pub fn take_webmcp_children(&mut self) -> Vec<(String, String, String)> {
        std::mem::take(&mut self.webmcp.children)
    }

    pub fn webmcp_pause_micros(&self) -> &[u128] {
        &self.webmcp.samples
    }

    pub fn webmcp_document(&mut self, session: &str, frame: &str) -> Result<Value, String> {
        self.webmcp_stable(session, Instant::now() + DOCUMENT_CHECK, |cdp, left| cdp.webmcp_document_once(session, frame, left))
    }

    fn webmcp_document_once(&mut self, session: &str, frame: &str, left: Duration) -> Result<Value, String> {
        if self.content_blocked(session) { return Err(self.refusal()); }
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let guard = self.guards.get(&target).cloned().ok_or_else(|| self.refusal())?;
        let epoch = self.frame_epoch;
        let tree = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), left.min(OPTIONAL_STEP))?;
        if epoch != self.frame_epoch || !self.webmcp_tree_verified(&tree, Some(frame)) || self.content_blocked(session) { return Err(self.refusal()); }
        let document = Self::webmcp_frame(&tree["frameTree"], frame).ok_or_else(|| self.refusal())?;
        Ok(json!({"frameId":frame,"loaderId":document["loaderId"],"origin":document["securityOrigin"],"parentFrameId":document.get("parentId")}))
    }

    pub fn webmcp_snapshot(&self) -> Vec<Value> {
        self.webmcp.arms.iter().filter(|(session, arm)| !arm.revoked && !self.blocked.contains(*session) && (self.webmcp.quarantined.contains(*session) || !self.content_blocked(session))).map(|(session, arm)| {
            let admitted_origins = self.webmcp.contexts.iter().filter(|((owner, _), context)| {
                owner == session && arm.accepted.contains(&(context.frame.clone(), context.loader.clone(), context.unique.clone()))
            }).map(|(_, context)| context.origin.clone()).collect::<BTreeSet<_>>();
            let pending_origins = if self.webmcp.quarantined.contains(session) && arm.admitted == 0 {
                self.webmcp_child_origin(session).into_iter().collect::<BTreeSet<_>>()
            } else {
                arm.pending_children.values().map(|(context, _, _)| context.origin.clone()).collect::<BTreeSet<_>>()
            };
            json!({"session":session,"bridge":arm.bridge,"target":self.session_target(session),"documents":arm.documents,"plain":arm.plain,"directChildAttach":arm.direct_child_attach,"admittedOrigins":admitted_origins,"pendingOrigins":pending_origins,"quarantined":self.webmcp.quarantined.contains(session)})
        }).collect()
    }

    pub fn webmcp_alive(&self, session: &str) -> bool {
        let session = self.webmcp_session(session);
        if !self.webmcp.arms.get(session).is_some_and(|arm| !arm.revoked) || self.blocked.contains(session) { return false; }
        if self.webmcp.quarantined.contains(session) {
            return self.session_target(session).and_then(|target| self.documents.get(&target)).is_none_or(|doc| !self.sealed(doc));
        }
        !self.content_blocked(session)
    }

    pub fn webmcp_settled(&self) -> bool {
        self.webmcp.arms.iter().all(|(session, arm)| {
            let child = self.webmcp.parents.contains_key(session) || self.webmcp.quarantined.contains(session);
            if child {
                return arm.revoked || self.blocked.contains(session)
                    || arm.pending_install.is_none() && arm.pending_contexts.is_empty() && arm.pending_frames.is_empty();
            }
            arm.revoked || self.blocked.contains(session) || self.content_blocked(session)
                || arm.pending_install.is_none() && arm.pending_contexts.is_empty() && arm.pending_frames.is_empty()
        })
    }

    pub fn webmcp_generations(&self) -> Vec<Value> {
        let mut generations = Vec::new();
        for (session, arm, target) in self.webmcp.arms.iter().filter_map(|(session, arm)| self.session_target(session).map(|target| (session, arm, target))) {
            let document = self.documents.get(&target).is_some_and(|doc| !doc.navigating && !doc.probed && !doc.previous && doc.unreachable_url.is_empty());
            let live = document && !arm.revoked && !self.webmcp.quarantined.contains(session) && !self.content_blocked(session) && !self.unobserved.contains(&target);
            let admitted = |context: &&Context| arm.loaders.get(&context.frame) == Some(&context.loader) && !arm.pending_frames.contains_key(&context.frame)
                && !self.frame_protected(&context.frame) && !self.guard.url(&context.origin) && arm.accepted.contains(&(context.frame.clone(), context.loader.clone(), context.unique.clone()));
            let contexts = self.webmcp.contexts.iter().filter(|((owner, _), _)| live && owner == session).map(|(_, context)| context).filter(admitted);
            generations.extend(contexts.map(|context| json!({"session":session,"target":target,"frameId":context.frame,"loaderId":context.loader,"uniqueId":context.unique,"origin":context.origin,"bridge":arm.bridge})));
        }
        generations
    }

    pub(super) fn webmcp_sweep(&mut self) {
        let pending = self.webmcp.arms.iter().filter(|(session, arm)| {
            !self.webmcp.parents.contains_key(*session) && !self.webmcp.quarantined.contains(*session)
                && arm.plain && arm.admitted > 0 && !arm.revoked && (!arm.pending_contexts.is_empty() || !arm.pending_frames.is_empty())
        }).map(|(session, _)| session.clone()).collect::<Vec<_>>();
        if !self.webmcp.handling_gate {
            self.webmcp.handling_gate = true;
            for session in pending {
                if self.webmcp.contexts.iter().any(|((owner, id), _)| owner == &session && self.webmcp.arms[&session].pending_contexts.contains_key(id)) && self.webmcp_install_current(&session).is_err() { self.webmcp_revoke(&session); }
            }
            self.webmcp.handling_gate = false;
        }
        let now = Instant::now();
        let stale = self.webmcp.arms.iter().filter(|(_, arm)| !arm.revoked && (arm.pending_install.is_some_and(|deadline| deadline <= now) || arm.pending_contexts.values().any(|deadline| *deadline <= now) || arm.pending_frames.values().any(|(_, deadline)| *deadline <= now))).map(|(session, _)| session.clone()).collect::<Vec<_>>();
        for session in stale {
            if self.webmcp.parents.contains_key(&session) || self.webmcp.quarantined.contains(&session) {
                self.webmcp_settle_child(&session, "a cross-origin child could not verify its document; synchronous registration before installation is unavailable");
                continue;
            }
            let reason = self.webmcp.arms.get(&session).map(|arm| if arm.pending_install.is_some_and(|deadline| deadline <= now) { "installer deadline" } else if arm.pending_contexts.values().any(|deadline| *deadline <= now) { "context deadline" } else { "frame deadline" }).unwrap_or("missing holder");
            self.webmcp_revoke_because(&session, reason);
            if !self.webmcp_plain(&session) { let _ = self.send("Debugger.resume", json!({}), Some(&session)); }
        }
        self.webmcp_install_children();
    }

    fn webmcp_settle_child(&mut self, child: &str, notice: &str) {
        if let Some(arm) = self.webmcp.arms.get_mut(child) {
            arm.pending_install = None;
            arm.pending_install_loader = None;
            arm.pending_contexts.clear();
            arm.pending_frames.clear();
        }
        if let Some(parent) = self.webmcp.parents.get(child).cloned() {
            if let Some(realm) = self.webmcp.contexts.iter().find(|((owner, _), _)| owner == child).map(|(_, context)| (context.frame.clone(), context.loader.clone(), context.unique.clone())) {
                if let Some(arm) = self.webmcp.arms.get_mut(&parent) {
                    if arm.child_done.len() < 1024 { arm.child_done.insert(realm); }
                }
            }
        }
        self.notices.push(format!("webmcp-polyfill: {notice}\n"));
        self.webmcp_revoke(child);
        let _ = self.raw_call("Target.detachFromTarget", json!({"sessionId":child}), None, DOCUMENT_CHECK);
        self.blocked.insert(child.to_string());
        self.webmcp.quarantined.remove(child);
        self.guard_sessions.remove(child);
    }

    pub fn stop_webmcp(&mut self, session: &str) {
        let owner = self.webmcp_session(session).to_string();
        let session = owner.as_str();
        let attached = self.webmcp.holders.values().any(|holder| holder == session);
        let debug = self.webmcp.arms.get(session).is_some_and(|arm| !arm.plain);
        self.webmcp_detach_parked_children(session);
        if self.webmcp.arms.contains_key(session) {
            let _ = self.raw_call("Target.setAutoAttach", json!({"autoAttach":false,"waitForDebuggerOnStart":false,"flatten":true,"filter":[{"type":"iframe","exclude":false}]}), Some(session), OPTIONAL_STEP);
        }
        self.webmcp_revoke(session);
        if debug {
            let _ = self.raw_call("Debugger.resume", json!({}), Some(session), DOCUMENT_CHECK);
            let _ = self.raw_call("Debugger.disable", json!({}), Some(session), DOCUMENT_CHECK);
        }
        self.webmcp.arms.remove(session);
        if attached {
            let _ = self.call_for("Target.detachFromTarget", json!({"sessionId":session}), None, DOCUMENT_CHECK);
            self.webmcp.holders.retain(|_, holder| holder != session);
        }
    }

    fn webmcp_install(&mut self, session: &str, seed: &Value) -> Result<String, String> {
        let shim = vetted_source(SHIM)?;
        let bridge = private_bridge(getrandom::fill)?;
        let source = format!("const {bridge} = {shim};");
        let mut loaders = BTreeMap::new();
        Self::webmcp_loaders(&seed["frameTree"], &mut loaders);
        self.webmcp.arms.insert(session.to_string(), Arm { source, bridge: bridge.clone(), identifier: None, conversion: None, plain: false, direct_child_attach: false, root_identity: None, pending_children: BTreeMap::new(), child_done: BTreeSet::new(), admitted: 0, revoked: false, revoked_at: None, revoked_reason: None, pending_contexts: BTreeMap::new(), pending_install: None, pending_install_loader: None, pending_frames: BTreeMap::new(), accepted: BTreeSet::new(), installing: BTreeSet::new(), loaders, documents: BTreeMap::new(), parked_origin: String::new(), context_generation: 0, frame_accept_generation: BTreeMap::new() });
        self.call_for("Runtime.disable", json!({}), Some(session), DOCUMENT_CHECK)?;
        self.call_for("Runtime.enable", json!({}), Some(session), DOCUMENT_CHECK)?;
        self.call_for("Page.enable", json!({}), Some(session), DOCUMENT_CHECK)?;
        self.call_for("Network.enable", json!({}), Some(session), DOCUMENT_CHECK)?;
        let enabled = self.call_guarded_for("Debugger.enable", json!({}), Some(session), DOCUMENT_CHECK, true, false);
        if let Err(message) = enabled {
            if Self::webmcp_foreign_refusal(&message) && self.webmcp_enter_plain(session, &message, false).is_ok() { return Ok(bridge); }
            self.webmcp_revoke(session);
            return Err(format!("{message}Runtime.evaluate: slicc: refused: WebMCP installation could not verify a usable guard and document; update the browser guard and client\n"));
        }
        if let Err(message) = self.webmcp_arm(session) { self.webmcp_enter_plain(session, &message, true)?; }
        Ok(bridge)
    }

    fn webmcp_enter_plain(&mut self, session: &str, message: &str, enabled: bool) -> Result<(), String> {
        if !Self::webmcp_foreign_refusal(message) || self.webmcp_plain(session) { return Err(format!("{message}Runtime.evaluate: slicc: refused: WebMCP installation could not verify a usable guard and document; update the browser guard and client\n")); }
        if enabled {
            let _ = self.raw_call("Debugger.disable", json!({}), Some(session), DOCUMENT_CHECK);
        }
        self.webmcp_plain_proof(session).map_err(|message| format!("{message}webmcp-polyfill: other-extension fallback could not verify the document\n"))?;
        let arm = self.webmcp.arms.get_mut(session).ok_or_else(|| "slicc: refused: WebMCP holder disappeared\n".to_string())?;
        arm.plain = true;
        arm.identifier = None;
        Ok(())
    }

    fn webmcp_foreign_refusal(message: &str) -> bool {
        let message = message.trim();
        let message = ["Debugger.enable: ", "Debugger.setInstrumentationBreakpoint: ", "Runtime.evaluate: "].into_iter().find_map(|prefix| message.strip_prefix(prefix)).unwrap_or(message);
        matches!(message, "slicc: refused: that execution context belongs to a browser extension" | "that execution context belongs to a browser extension" | "slicc: refused: a browser extension runs scripts in this page, and profiling it would expose them")
    }

    fn webmcp_plain_proof(&mut self, session: &str) -> Result<(), String> {
        let until = Instant::now() + DOCUMENT_CHECK;
        fn tree_complete(cdp: &Cdp, node: &Value, child: bool) -> Option<bool> {
            let frame = node.get("frame")?.as_object()?;
            if ["id", "loaderId", "securityOrigin"].iter().any(|key| frame.get(*key).and_then(Value::as_str).is_none_or(str::is_empty)) { return None; }
            let url = frame.get("url")?.as_str()?;
            if cdp.frame_protected(frame.get("id")?.as_str()?) || (!child && url.is_empty()) { return None; }
            let mut complete = !url.is_empty();
            if let Some(children) = node.get("childFrames") {
                for child in children.as_array()? { complete &= tree_complete(cdp, child, true)?; }
            }
            Some(complete)
        }
        loop {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() || !self.worlds.fallback_safe(session) || self.content_blocked(session) { return Err(self.refusal()); }
            let tree = self.call_for("Page.getFrameTree", json!({}), Some(session), left)?;
            if Instant::now() >= until || !self.worlds.fallback_safe(session) || self.content_blocked(session) { return Err(self.refusal()); }
            match tree.get("frameTree").filter(|node| tree_bounded(node)).and_then(|node| tree_complete(self, node, false)) {
                Some(true) if self.webmcp_tree_verified(&tree, None) => return Ok(()),
                Some(false) => { let _ = self.pump(until.min(Instant::now() + Duration::from_millis(25)))?; }
                _ => return Err(self.refusal()),
            }
        }
    }

    fn webmcp_arm(&mut self, session: &str) -> Result<(), String> {
        if self.webmcp_plain(session) { return Ok(()); }
        let result = self.call_guarded_for("Debugger.setInstrumentationBreakpoint", json!({"instrumentation":"beforeScriptExecution"}), Some(session), DOCUMENT_CHECK, true, false)?;
        let identifier = result.get("breakpointId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| "slicc: refused: WebMCP instrumentation is unavailable; update the browser guard and client\n".to_string())?;
        let arm = self.webmcp.arms.get_mut(session).ok_or_else(|| "slicc: refused: WebMCP holder disappeared\n".to_string())?;
        arm.identifier = Some(identifier.to_string());
        Ok(())
    }

    fn webmcp_install_current(&mut self, session: &str) -> Result<(), String> {
        if self.webmcp_plain(session) { self.webmcp_plain_proof(session)?; }
        self.webmcp_install_root(session)
    }

    fn webmcp_install_root(&mut self, session: &str) -> Result<(), String> {
        if self.webmcp_plain(session) { return self.webmcp_install_plain_root(session); }
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let guard = self.guards.get(&target).cloned().ok_or_else(|| self.refusal())?;
        let until = Instant::now() + DOCUMENT_CHECK;
        let tree = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), DOCUMENT_CHECK)?;
        if !self.webmcp_tree_verified(&tree, None) || self.content_blocked(session) { return Err(self.refusal()); }
        let frame = &tree["frameTree"]["frame"];
        let (id, context) = self.webmcp.contexts.iter().find(|((owner, _), context)| owner == session && frame.get("id").and_then(Value::as_str) == Some(&context.frame)
            && frame.get("loaderId").and_then(Value::as_str) == Some(&context.loader)).map(|((_, id), context)| (*id, context.clone())).ok_or_else(|| self.refusal())?;
        let root = self.webmcp_root_document(session, until)?;
        self.webmcp_evaluate(session, id, &context, root, until)?;
        self.webmcp_mark_admitted(session, id, &context, Instant::now())
    }

    fn webmcp_install_plain_root(&mut self, session: &str) -> Result<(), String> {
        let until = Instant::now() + DOCUMENT_CHECK;
        let root = self.webmcp_root_document(session, until)?;
        let context = root.1.clone();
        let id = self.webmcp.contexts.iter().find(|((owner, _), current)| owner == session && *current == &context).map(|((_, id), _)| *id).ok_or_else(|| self.refusal())?;
        if !self.worlds.fallback_safe(session) || self.document_paused(&root.0) { return Err(self.refusal()); }
        let arm = self.webmcp.arms.get(session).filter(|arm| !arm.revoked).ok_or_else(|| self.refusal())?;
        let realm = (context.frame.clone(), context.loader.clone(), context.unique.clone());
        if arm.accepted.contains(&realm) { return Ok(()); }
        let params = json!({"expression":arm.source,"uniqueContextId":context.unique,"returnByValue":true,"silent":true});
        self.admit("Runtime.evaluate", &params, Some(session))?;
        self.webmcp.arms.get_mut(session).ok_or_else(|| "slicc: refused: WebMCP holder disappeared\n".to_string())?.installing.insert(realm.clone());
        let result = self.raw_call_execution("Runtime.evaluate", params, Some(session), until.saturating_duration_since(Instant::now()), true);
        if let Some(arm) = self.webmcp.arms.get_mut(session) { arm.installing.remove(&realm); }
        let result = result?;
        if result.get("exceptionDetails").is_some() || !self.worlds.fallback_safe(session) || self.webmcp_root_document(session, until)? != root { return Err(self.refusal()); }
        self.webmcp_check_context(session, id, &context, until)?;
        if !self.worlds.fallback_safe(session) { return Err(self.refusal()); }
        self.webmcp_mark_admitted(session, id, &context, Instant::now())
    }

    fn webmcp_bound_root(&mut self, session: &str, until: Instant) -> Result<(String, Context), String> {
        if let Some(parent) = self.webmcp.parents.get(session).cloned() {
            self.webmcp_root_document(&parent, until)
        } else {
            self.webmcp_root_document(session, until)
        }
    }

    fn webmcp_evaluate(&mut self, session: &str, id: i64, context: &Context, root: (String, Context), until: Instant) -> Result<(), String> {
        let realm = (context.frame.clone(), context.loader.clone(), context.unique.clone());
        let arm = self.webmcp.arms.get(session).filter(|arm| !arm.revoked).ok_or_else(|| self.refusal())?;
        if arm.plain && !self.worlds.fallback_safe(session) { return Err(self.refusal()); }
        if arm.accepted.contains(&realm) || arm.installing.contains(&realm) { return Ok(()); }
        let source = arm.source.clone();
        let arm = self.webmcp.arms.get_mut(session).ok_or_else(|| "slicc: refused: WebMCP holder disappeared\n".to_string())?;
        if !arm.installing.insert(realm.clone()) { return Ok(()); }
        let outcome = (|| -> Result<(), String> {
            if self.webmcp_bound_root(session, until)? != root || context.unique.is_empty() || self.webmcp.contexts.get(&(session.to_string(), id)) != Some(context) { return Err(self.refusal()); }
            self.webmcp_check_context(session, id, context, until)?;
            if self.webmcp_plain(session) && !self.worlds.fallback_safe(session) { return Err(self.refusal()); }
            if self.webmcp.arms.get(session).is_some_and(|arm| arm.accepted.contains(&realm)) { return Ok(()); }
            let result = if self.webmcp.parents.contains_key(session) {
                self.raw_call("Runtime.evaluate", json!({"expression":source,"uniqueContextId":context.unique,"returnByValue":true,"silent":true}), Some(session), until.saturating_duration_since(Instant::now()))
            } else {
                self.call_for("Runtime.evaluate", json!({"expression":source,"uniqueContextId":context.unique,"returnByValue":true,"silent":true}), Some(session), until.saturating_duration_since(Instant::now()))
            }?;
            if result.get("exceptionDetails").is_some() || self.webmcp_plain(session) && !self.worlds.fallback_safe(session) || self.webmcp_bound_root(session, until)? != root { return Err(self.refusal()); }
            self.webmcp_check_context(session, id, context, until)
        })();
        if let Some(arm) = self.webmcp.arms.get_mut(session) { arm.installing.remove(&realm); }
        outcome
    }

    fn webmcp_known_root(&self, session: &str) -> Result<(String, Context), String> {
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let document = self.documents.get(&target).ok_or_else(|| self.refusal())?;
        let context = self.webmcp.contexts.iter().find(|((owner, _), context)| owner == session
            && context.frame == document.frame_id && self.webmcp.arms.get(session).and_then(|arm| arm.loaders.get(&context.frame)) == Some(&context.loader) && !context.unique.is_empty()).map(|(_, context)| context.clone()).ok_or_else(|| self.refusal())?;
        if !self.webmcp_alive(session) || self.worlds.reason(session, "Debugger.enable", &json!({})).is_some() { return Err(self.refusal()); }
        Ok((target, context))
    }

    fn webmcp_root_document(&mut self, session: &str, until: Instant) -> Result<(String, Context), String> {
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let guard = self.guards.get(&target).cloned().ok_or_else(|| self.refusal())?;
        let tree = self.webmcp_stable(session, until, |cdp, left| cdp.raw_call("Page.getFrameTree", json!({}), Some(&guard), left))?;
        if !self.webmcp_tree_verified(&tree, None) { return Err(self.refusal()); }
        self.main_commit(&guard, &tree["frameTree"]["frame"]);
        let frame = &tree["frameTree"]["frame"];
        let context = self.webmcp.contexts.iter().find(|((owner, _), context)| owner == session
            && frame.get("id").and_then(Value::as_str) == Some(&context.frame)
            && frame.get("loaderId").and_then(Value::as_str) == Some(&context.loader)
            && frame.get("securityOrigin").and_then(Value::as_str) == Some(&context.origin)
            && !context.unique.is_empty()).map(|(_, context)| context.clone()).ok_or_else(|| self.refusal())?;
        if Instant::now() >= until || !self.webmcp_alive(session) || self.unobserved.contains(&target)
            || self.documents.get(&target).is_none_or(|doc| doc.navigating || doc.probed || doc.previous || !doc.unreachable_url.is_empty())
            || !self.webmcp_plain(session) && self.worlds.reason(session, "Debugger.enable", &json!({})).is_some() { return Err(self.worlds.reason(session, "Debugger.enable", &json!({})).map(str::to_string).unwrap_or_else(|| self.refusal())); }
        Ok((target, context))
    }

    fn webmcp_check_context(&mut self, session: &str, id: i64, context: &Context, until: Instant) -> Result<(), String> {
        let (_, root) = self.webmcp_root_document(session, until)?;
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let guard = self.guards.get(&target).cloned().ok_or_else(|| self.refusal())?;
        let tree = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), until.saturating_duration_since(Instant::now()))?;
        let frame = Self::webmcp_frame(&tree["frameTree"], &context.frame).ok_or_else(|| self.refusal())?;
        let same_origin = root.origin == context.origin;
        let cross = !same_origin && self.webmcp.arms.get(session).is_some_and(|arm| arm.direct_child_attach);
        if Instant::now() >= until || !self.webmcp_alive(session) || !self.webmcp_tree_verified(&tree, Some(&context.frame))
            || frame.get("loaderId").and_then(Value::as_str) != Some(&context.loader)
            || frame.get("securityOrigin").and_then(Value::as_str) != Some(&context.origin)
            || !same_origin && !cross || context.unique.is_empty() || self.webmcp.contexts.get(&(session.to_string(), id)) != Some(context)
            || !self.webmcp_plain(session) && self.worlds.reason(session, "Debugger.enable", &json!({})).is_some() { return Err(self.refusal()); }
        if (cross || self.webmcp.parents.contains_key(session)) && (self.frame_protected(&context.frame) || self.guard.url(&context.origin) || self.guard.extension_frame(&context.origin)
            || frame.get("url").and_then(Value::as_str).is_some_and(|url| self.guard.extension_frame(url) || self.guard.url(url))
            || !self.webmcp_frame_verified(frame)) { return Err(self.refusal()); }
        Ok(())
    }

    pub(super) fn webmcp_is_paused(&self, session: &str) -> bool {
        self.webmcp.paused.contains(session)
    }

    #[track_caller]
    pub(super) fn webmcp_revoke(&mut self, session: &str) {
        self.webmcp_revoke_because(session, "guard or lifecycle refusal");
    }

    #[track_caller]
    fn webmcp_revoke_because(&mut self, session: &str, reason: &'static str) {
        let location = std::panic::Location::caller();
        if let Some(arm) = self.webmcp.arms.get_mut(session) {
            if arm.revoked_at.is_none() { arm.revoked_at = Some((location.file(), location.line())); arm.revoked_reason = Some(reason); }
            arm.revoked = true;
            arm.conversion = None;
            if let Some(identifier) = arm.identifier.take() {
                let _ = self.send("Debugger.removeBreakpoint", json!({"breakpointId":identifier}), Some(session));
            }
        }
        self.webmcp_detach_parked_children(session);
        self.webmcp_trace_stage(session, "revoke", reason);
    }

    pub fn webmcp_trace_result<T>(&self, session: &str, stage: &str, result: Result<T, String>) -> Result<T, String> {
        if result.is_err() { self.webmcp_trace_stage(session, "holder-wait-refused", stage); }
        result
    }

    pub fn webmcp_trace_stage(&self, session: &str, event: &str, reason: &str) {
        if std::env::var("SLICC_WEBMCP_TRACE").ok().as_deref() != Some("1") { return; }
        let arm = self.webmcp.arms.get(session);
        let location = arm.and_then(|arm| arm.revoked_at).map(|(file, line)| format!("{}:{line}", file.rsplit('/').next().unwrap_or("guard")));
        let child = self.webmcp.parents.contains_key(session);
        let trace = json!({"event":event,"reason":reason,"sessionRole":if child {"child"} else {"root"},"frameRole":self.webmcp.trace_roles.get(session).copied().unwrap_or(if child {"OOPIF"} else {"unknown"}),"sequence":self.webmcp.trace_sequence,"firstReason":arm.and_then(|arm| arm.revoked_reason),"firstLocation":location,"revoked":arm.is_some_and(|arm| arm.revoked),"blocked":self.blocked.contains(session),"contentBlocked":self.content_blocked(session),"pendingInstall":arm.is_some_and(|arm| arm.pending_install.is_some()),"pendingContexts":arm.map(|arm| arm.pending_contexts.len()),"pendingFrames":arm.map(|arm| arm.pending_frames.len()),"page":self.webmcp_trace_page(session)});
        eprintln!("webmcp-polyfill: {trace}");
    }

    fn webmcp_frame_refusal(&self, frame: &Value) -> Option<&'static str> {
        let text = |key: &str| frame.get(key).and_then(Value::as_str).unwrap_or("");
        if text("url").is_empty() { return Some("empty document URL"); }
        let Ok(url) = url::Url::parse(text("url")) else { return Some("unknown document URL"); };
        if self.guard.url(text("url")) || self.guard.url(text("securityOrigin")) { return Some("protected document"); }
        if !matches!(url.scheme(), "http" | "https") { return Some("non-HTTP document"); }
        if url.origin().ascii_serialization() != text("securityOrigin") { return Some("document origin mismatch"); }
        if text("id").is_empty() { return Some("missing frame id"); }
        if text("loaderId").is_empty() { return Some("missing document loader"); }
        if !text("unreachableUrl").is_empty() { return Some("unreachable document"); }
        None
    }

    fn webmcp_frame_verified(&self, frame: &Value) -> bool {
        self.webmcp_frame_refusal(frame).is_none()
    }

    fn webmcp_tree_verified(&self, tree: &Value, frame: Option<&str>) -> bool {
        fn find(cdp: &Cdp, node: &Value, frame: Option<&str>) -> bool {
            let current = &node["frame"];
            let id = current.get("id").and_then(Value::as_str).unwrap_or("");
            if !cdp.webmcp_frame_verified(current) || cdp.frame_protected(id) { return false; }
            if frame.is_none_or(|frame| frame == id) { return true; }
            node.get("childFrames").and_then(Value::as_array).is_some_and(|children| children.iter().any(|child| find(cdp, child, frame)))
        }
        tree.get("frameTree").is_some_and(|node| tree_bounded(node) && find(self, node, frame))
    }

    fn webmcp_loaders(node: &Value, out: &mut BTreeMap<String, String>) {
        if tree_bounded(node) { Self::webmcp_loaders_walk(node, out); }
    }

    fn webmcp_loaders_walk(node: &Value, out: &mut BTreeMap<String, String>) {
        if let (Some(id), Some(loader)) = (node.pointer("/frame/id").and_then(Value::as_str), node.pointer("/frame/loaderId").and_then(Value::as_str)) {
            if !loader.is_empty() { out.insert(id.to_string(), loader.to_string()); }
        }
        for child in node.get("childFrames").and_then(Value::as_array).into_iter().flatten() { Self::webmcp_loaders_walk(child, out); }
    }

    fn webmcp_frame<'a>(node: &'a Value, frame: &str) -> Option<&'a Value> {
        if !tree_bounded(node) { return None; }
        Self::webmcp_frame_walk(node, frame)
    }

    fn webmcp_frame_walk<'a>(node: &'a Value, frame: &str) -> Option<&'a Value> {
        if node.pointer("/frame/id").and_then(Value::as_str) == Some(frame) { return node.get("frame"); }
        node.get("childFrames").and_then(Value::as_array)?.iter().find_map(|child| Self::webmcp_frame_walk(child, frame))
    }

    fn webmcp_verify_pause(&mut self, session: &str, paused: &Value, deadline: Instant) -> Result<(String, (String, Context), i64, Context), String> {
        let root = self.webmcp_root_document(session, deadline)?;
        let call = paused.pointer("/params/callFrames/0").ok_or_else(|| self.refusal())?;
        if paused.pointer("/params/reason").and_then(Value::as_str) == Some("instrumentation") { return Err(self.refusal()); }
        let script = call.pointer("/location/scriptId").and_then(Value::as_str).ok_or_else(|| self.refusal())?;
        let context_id = *self.webmcp.scripts.get(&(session.to_string(), script.to_string())).ok_or_else(|| self.refusal())?;
        let context = self.webmcp.contexts.get(&(session.to_string(), context_id)).cloned().ok_or_else(|| self.refusal())?;
        if let Some(reason) = self.worlds.reason(session, "Debugger.enable", &json!({})) { return Err(reason.into()); }
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let guard = self.guards.get(&target).cloned().ok_or_else(|| self.refusal())?;
        let mut owner = session.to_string();
        let mut ancestors = BTreeSet::new();
        let mut ancestor_documents = Vec::new();
        while let Some(parent) = self.webmcp.parents.get(&owner).cloned() {
            if ancestors.len() >= MAX_TREE_DEPTH || !ancestors.insert(parent.clone()) || !self.webmcp_alive(&parent) {
                return Err(self.refusal());
            }
            let target = self.session_target(&parent).ok_or_else(|| self.refusal())?;
            let guard = self.guards.get(&target).cloned().ok_or_else(|| self.refusal())?;
            if self.documents.get(&target).is_none_or(|doc| doc.navigating || doc.probed || doc.previous) {
                return Err(self.refusal());
            }
            let ancestor = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), deadline.saturating_duration_since(Instant::now()))?;
            if !self.webmcp_tree_verified(&ancestor, None) { return Err(self.refusal()); }
            ancestor_documents.push((parent.clone(), guard, ancestor["frameTree"]["frame"].clone()));
            owner = parent;
        }
        let tree = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), deadline.saturating_duration_since(Instant::now()))?;
        if self.webmcp_tree_verified(&tree, Some(&context.frame)) {
            self.main_commit(&guard, &tree["frameTree"]["frame"]);
        }
        let blocked = self.blocked.contains(session) || !self.webmcp.quarantined.contains(session) && self.content_blocked(session);
        if !self.webmcp_tree_verified(&tree, Some(&context.frame)) || blocked || self.unobserved.contains(&target)
            || self.documents.get(&target).is_none_or(|doc| doc.navigating || doc.probed || doc.previous || !doc.unreachable_url.is_empty()) {
            return Err(self.refusal());
        }
        if root.1.origin != context.origin { return Err(self.refusal()); }
        let frame = Self::webmcp_frame(&tree["frameTree"], &context.frame).ok_or_else(|| self.refusal())?;
        if frame.get("securityOrigin").and_then(Value::as_str) != Some(&context.origin) || context.unique.is_empty()
            || context.loader.is_empty() || frame.get("loaderId").and_then(Value::as_str) != Some(&context.loader) {
            return Err(self.refusal());
        }
        let fresh = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), deadline.saturating_duration_since(Instant::now()))?;
        let next = Self::webmcp_frame(&fresh["frameTree"], &context.frame).ok_or_else(|| self.refusal())?;
        if !self.webmcp_tree_verified(&fresh, Some(&context.frame))
            || frame.get("loaderId") != next.get("loaderId") || frame.get("url") != next.get("url")
            || self.session_target(session).as_deref() != Some(&target) || self.guards.get(&target) != Some(&guard)
            || self.webmcp.contexts.get(&(session.to_string(), context_id)) != Some(&context) {
            return Err(self.refusal());
        }
        for (parent, guard, document) in ancestor_documents {
            let ancestor = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), deadline.saturating_duration_since(Instant::now()))?;
            let current = &ancestor["frameTree"]["frame"];
            if !self.webmcp_alive(&parent) || !self.webmcp_tree_verified(&ancestor, None) || current.get("loaderId") != document.get("loaderId") || current.get("url") != document.get("url") {
                return Err(self.refusal());
            }
        }
        if self.webmcp_root_document(session, deadline)? != root { return Err(self.refusal()); }
        let call_id = call.get("callFrameId").and_then(Value::as_str).ok_or_else(|| self.refusal())?.to_string();
        Ok((call_id, root, context_id, context))
    }

    fn webmcp_pause_context(&self, value: &Value, session: &str) -> Option<(i64, Context)> {
        let script = value.pointer("/params/data/scriptId").or_else(|| value.pointer("/params/callFrames/0/location/scriptId"))?.as_str()?;
        let id = *self.webmcp.scripts.get(&(session.to_string(), script.to_string()))?;
        Some((id, self.webmcp.contexts.get(&(session.to_string(), id))?.clone()))
    }

    fn webmcp_handle_instrumentation(&mut self, value: &Value, session: &str) -> bool {
        if value.pointer("/params/reason").and_then(Value::as_str) != Some("instrumentation") { return false; }
        let started = Instant::now();
        let until = started + DOCUMENT_CHECK;
        let script = value.pointer("/params/data/scriptId").or_else(|| value.pointer("/params/callFrames/0/location/scriptId")).and_then(Value::as_str);
        let evaluate = script.is_some_and(|script| self.webmcp.evaluations.contains(&(session.to_string(), script.to_string())));
        let pause_context = self.webmcp_pause_context(value, session);
        let accepted = pause_context.as_ref().is_some_and(|(_, context)| self.webmcp.arms.get(session).is_some_and(|arm| arm.accepted.contains(&(context.frame.clone(), context.loader.clone(), context.unique.clone()))));
        let Some((_, context)) = pause_context else {
            let _ = self.raw_call("Debugger.resume", json!({}), Some(session), DOCUMENT_CHECK);
            self.webmcp.paused.remove(session);
            return true;
        };
        if evaluate || accepted {
            let _ = self.raw_call("Debugger.resume", json!({}), Some(session), DOCUMENT_CHECK);
            self.webmcp.paused.remove(session);
            return true;
        }
        let root_proof = self.webmcp_known_root(session);
        let converted = root_proof.and_then(|root| {
            let arm = self.webmcp.arms.get_mut(session).ok_or_else(|| "slicc: refused: WebMCP holder disappeared\n".to_string())?;
            if arm.accepted.contains(&(context.frame.clone(), context.loader.clone(), context.unique.clone())) || context.origin != root.1.origin {
                return self.raw_call("Debugger.resume", json!({}), Some(session), until.saturating_duration_since(Instant::now()));
            }
            let owned = arm.identifier.is_some() && arm.conversion.is_none();
            if arm.plain || !owned { return Err(self.refusal()); }
            arm.conversion = Some((root.clone(), started, until));
            arm.pending_install = Some(until);
            self.raw_call("Debugger.pause", json!({}), Some(session), until.saturating_duration_since(Instant::now())).and_then(|_| {
                if Instant::now() >= until || self.webmcp_known_root(session)? != root { return Err(self.refusal()); }
                self.raw_call("Debugger.resume", json!({}), Some(session), until.saturating_duration_since(Instant::now()))
            })
        });
        if converted.is_err() {
            if let Some(arm) = self.webmcp.arms.get_mut(session) { arm.conversion = None; arm.pending_install = None; arm.pending_install_loader = None; }
            let _ = self.raw_call("Debugger.resume", json!({}), Some(session), DOCUMENT_CHECK);
        }
        self.webmcp.paused.remove(session);
        true
    }

    fn webmcp_pending_child_pause(&mut self, session: &str, paused: &Value, root: &(String, Context), until: Instant) -> bool {
        if Instant::now() >= until || self.blocked.contains(session) || self.unobserved.contains(&root.0)
            || self.worlds.reason(session, "Debugger.enable", &json!({})).is_some()
            || self.webmcp.arms.get(session).is_none_or(|arm| arm.revoked || arm.plain || arm.loaders.get(&root.1.frame) != Some(&root.1.loader))
            || self.webmcp_pause_context(paused, session).is_none_or(|(_, context)| context != root.1)
            || self.documents.get(&root.0).is_none_or(|doc| doc.frame_id != root.1.frame || doc.navigating || doc.probed || doc.previous || !doc.unreachable_url.is_empty()) { return false; }
        let Some(guard) = self.guards.get(&root.0).cloned() else { return false; };
        let Ok(tree) = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), until.saturating_duration_since(Instant::now())) else { return false; };
        if !tree_bounded(&tree["frameTree"]) { return false; }
        let frame = &tree["frameTree"]["frame"];
        if !self.webmcp_frame_verified(frame) || self.frame_protected(&root.1.frame)
            || frame.get("id").and_then(Value::as_str) != Some(&root.1.frame)
            || frame.get("loaderId").and_then(Value::as_str) != Some(&root.1.loader)
            || frame.get("securityOrigin").and_then(Value::as_str) != Some(&root.1.origin)
            || self.session_target(session).as_deref() != Some(&root.0)
            || self.webmcp.contexts.values().all(|context| context != &root.1) { return false; }
        fn pending(cdp: &Cdp, node: &Value, parent: &str) -> Option<bool> {
            let frame = node.get("frame")?;
            let text = |key| frame.get(key).and_then(Value::as_str).unwrap_or("");
            let id = text("id");
            if id.is_empty() || text("parentId") != parent || cdp.frame_protected(id)
                || !text("unreachableUrl").is_empty()
                || !matches!(text("securityOrigin"), "" | "null" | "://") && cdp.guard.url(text("securityOrigin")) { return None; }
            let provisional = matches!(text("url"), "" | "about:blank" | "about:srcdoc");
            if !provisional && cdp.guard.url(text("url")) { return None; }
            let unsettled = provisional || matches!(text("securityOrigin"), "" | "null" | "://") || text("loaderId").is_empty();
            if !unsettled && !cdp.webmcp_frame_verified(frame) { return None; }
            let mut result = unsettled;
            if let Some(children) = node.get("childFrames") {
                for child in children.as_array()? { result |= pending(cdp, child, id)?; }
            }
            Some(result)
        }
        let Some(children) = tree["frameTree"].get("childFrames").and_then(Value::as_array) else { return false; };
        let mut unsettled = false;
        for child in children {
            let Some(result) = pending(self, child, &root.1.frame) else { return false; };
            unsettled |= result;
        }
        unsettled && Instant::now() < until && self.worlds.reason(session, "Debugger.enable", &json!({})).is_none()
            && self.webmcp.arms.get(session).is_some_and(|arm| !arm.revoked && arm.loaders.get(&root.1.frame) == Some(&root.1.loader))
            && self.webmcp_pause_context(paused, session).is_some_and(|(_, context)| context == root.1)
    }

    fn webmcp_queue_children(&mut self, session: &str) -> bool {
        let Some(target) = self.session_target(session) else { return false; };
        if self.document_paused(&target) { return false; }
        let Some(frame) = self.documents.get(&target).filter(|document| !document.navigating && !document.probed && !document.previous && document.unreachable_url.is_empty()).map(|document| document.frame_id.clone()) else { return false; };
        let Some(known) = self.webmcp.arms.get(session).filter(|arm| !arm.revoked && arm.conversion.is_none() && arm.pending_install.is_none()) else { return false; };
        let Some(context) = self.webmcp.contexts.iter().find(|((owner, _), context)| owner == session && context.frame == frame
            && known.loaders.get(&frame) == Some(&context.loader) && !context.unique.is_empty()).map(|(_, context)| context.clone()) else { return false; };
        let root = (target, context);
        if known.root_identity.as_ref() != Some(&root) || !known.accepted.contains(&(root.1.frame.clone(), root.1.loader.clone(), root.1.unique.clone())) { return false; }
        let direct = known.direct_child_attach;
        let requested = self.webmcp.requested_origins.clone();
        if known.plain && !self.worlds.fallback_safe(session) { return false; }
        let candidates = self.webmcp.contexts.iter().filter_map(|((owner, id), context)| {
            if owner != session || context.frame == root.1.frame || context.loader.is_empty() || context.unique.is_empty() { return None; }
            let same = context.origin == root.1.origin;
            let cross = !same && direct && requested.contains(&context.origin);
            if !same && !cross { return None; }
            if !same && (self.frame_protected(&context.frame) || self.guard.url(&context.origin) || self.guard.extension_frame(&context.origin)) { return None; }
            Some((*id, context.clone()))
        }).collect::<Vec<_>>();
        let Some(arm) = self.webmcp.arms.get_mut(session) else { return false; };
        arm.pending_children.retain(|id, (context, known_root, _)| known_root == &root && self.webmcp.contexts.get(&(session.to_string(), *id)) == Some(context));
        for (id, context) in candidates {
            let realm = (context.frame.clone(), context.loader.clone(), context.unique.clone());
            if arm.accepted.contains(&realm) || arm.child_done.contains(&realm) || arm.child_done.len() >= 1024 { continue; }
            if arm.pending_children.len() >= 1024 { break; }
            arm.pending_children.entry(id).or_insert_with(|| (context, root.clone(), Instant::now() + DOCUMENT_CHECK));
        }
        true
    }

    pub fn webmcp_install_children(&mut self) {
        if self.webmcp.handling_gate || self.webmcp.handling_children { return; }
        self.webmcp.handling_children = true;
        let sessions = self.webmcp.arms.keys().cloned().collect::<Vec<_>>();
        for session in sessions {
            if !self.webmcp_queue_children(&session) { continue; }
            let pending = self.webmcp.arms.get(&session).filter(|arm| !arm.revoked && arm.conversion.is_none() && arm.pending_install.is_none()).map(|arm| arm.pending_children.clone()).unwrap_or_default();
            for (id, (context, root, until)) in pending {
                let realm = (context.frame.clone(), context.loader.clone(), context.unique.clone());
                let cross = context.origin != root.1.origin;
                if Instant::now() >= until {
                    if let Some(arm) = self.webmcp.arms.get_mut(&session) { arm.pending_children.remove(&id); arm.child_done.insert(realm); }
                    self.notices.push(if cross {
                        "webmcp-polyfill: a cross-origin child placement did not settle before its installation deadline\n".into()
                    } else {
                        "webmcp-polyfill: a same-origin child placement did not settle before its installation deadline\n".into()
                    });
                    continue;
                }
                if self.webmcp_root_document(&session, until).as_ref() != Ok(&root)
                    || self.call_for("Page.getFrameTree", json!({}), Some(&session), until.saturating_duration_since(Instant::now())).is_err()
                    || self.webmcp_check_context(&session, id, &context, until).is_err() { continue; }
                if self.webmcp.arms.get(&session).is_some_and(|arm| arm.accepted.contains(&realm)) {
                    if let Some(arm) = self.webmcp.arms.get_mut(&session) { arm.pending_children.remove(&id); }
                    continue;
                }
                if let Some(arm) = self.webmcp.arms.get_mut(&session) { arm.pending_children.remove(&id); arm.child_done.insert(realm.clone()); }
                if self.webmcp_evaluate(&session, id, &context, root, until).is_ok() {
                    if self.webmcp.arms.get(&session).is_some_and(|arm| !arm.accepted.contains(&realm)) {
                        let _ = self.webmcp_mark_admitted(&session, id, &context, Instant::now());
                    }
                } else {
                    self.notices.push(if cross {
                        "webmcp-polyfill: a cross-origin child could not verify its document; synchronous registration before installation is unavailable\n".into()
                    } else {
                        "webmcp-polyfill: a same-origin child could not verify its document; synchronous registration before installation is unavailable\n".into()
                    });
                }
            }
        }
        self.webmcp.handling_children = false;
    }

    fn webmcp_handle_pause(&mut self, value: &Value, session: &str) {
        self.webmcp.paused.insert(session.to_string());
        let installing = self.webmcp_pause_context(value, session).map(|(_, context)| context).or_else(|| self.webmcp_known_root(session).ok().map(|(_, context)| context))
            .is_some_and(|context| self.webmcp.arms.get(session).is_some_and(|arm| arm.installing.contains(&(context.frame, context.loader, context.unique))));
        if installing {
            let _ = self.raw_call("Debugger.resume", json!({}), Some(session), DOCUMENT_CHECK);
            self.webmcp.paused.remove(session);
            return;
        }
        if self.webmcp_handle_instrumentation(value, session) { return; }
        if self.webmcp_pause_context(value, session).is_none() {
            let _ = self.raw_call("Debugger.resume", json!({}), Some(session), DOCUMENT_CHECK);
            self.webmcp.paused.remove(session);
            return;
        }
        let conversion = self.webmcp.arms.get_mut(session).and_then(|arm| arm.conversion.take());
        let started = conversion.as_ref().map_or_else(Instant::now, |(_, started, _)| *started);
        let until = conversion.as_ref().map_or(started + DOCUMENT_CHECK, |(_, _, until)| (*until).min(started + Duration::from_secs(1)));
        let verified = loop {
            let pending = conversion.as_ref().is_some_and(|(root, _, _)| self.webmcp_pending_child_pause(session, value, root, until));
            if pending {
                if self.pump(until).ok() != Some(true) { break Err(self.refusal()); }
                continue;
            }
            break if Instant::now() >= until { Err(self.refusal()) }
                else { self.webmcp_stable(session, until, |cdp, _| cdp.webmcp_verify_pause(session, value, until)) }.and_then(|proof| {
                    if conversion.as_ref().is_some_and(|(root, _, _)| root != &proof.1) { return Err(self.refusal()); }
                    Ok(proof)
                });
        };
        let admitted = verified.and_then(|(_, root, id, context)| {
            self.webmcp_evaluate(session, id, &context, root.clone(), until)?;
            Ok((id, context))
        });
        let _ = self.raw_call("Debugger.resume", json!({}), Some(session), DOCUMENT_CHECK);
        self.webmcp.paused.remove(session);
        if let Some(arm) = self.webmcp.arms.get_mut(session) { arm.pending_install = None; arm.pending_install_loader = None; }
        if let Err(message) = admitted {
            if Self::webmcp_foreign_refusal(&message) {
                let _ = self.webmcp_enter_plain(session, &message, true).and_then(|_| self.webmcp_install_current(session));
            }
        } else if let Ok((id, context)) = admitted {
            let _ = self.webmcp_mark_admitted(session, id, &context, started);
        }
        if self.webmcp.deferred.contains(session) { self.block(session); }
    }

    fn webmcp_mark_admitted(&mut self, session: &str, id: i64, expected: &Context, started: Instant) -> Result<(), String> {
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let context = self.webmcp.contexts.get(&(session.to_string(), id)).filter(|context| *context == expected).cloned().ok_or_else(|| self.refusal())?;
        let root = self.documents.get(&target).is_some_and(|document| document.frame_id == context.frame);
        let arm = self.webmcp.arms.get_mut(session).ok_or_else(|| "slicc: refused: WebMCP holder disappeared\n".to_string())?;
        if root { arm.root_identity = Some((target.clone(), context.clone())); }
        arm.pending_install = None;
        arm.pending_install_loader = None;
        arm.pending_contexts.remove(&id);
        arm.pending_frames.remove(&context.frame);
        if arm.accepted.len() >= 1024 {
            arm.accepted.clear();
            arm.admitted = 0;
        }
        let realm = (context.frame.clone(), context.loader.clone(), context.unique.clone());
        if arm.accepted.insert(realm) {
            arm.frame_accept_generation.insert(context.frame.clone(), context.generation);
            self.webmcp.samples.push(started.elapsed().as_micros());
        }
        arm.admitted = arm.accepted.len();
        let bridge = arm.bridge.clone();
        if self.webmcp.quarantined.remove(session) {
            self.guard_sessions.remove(session);
            self.webmcp.children.push((target, session.to_string(), bridge));
        }
        Ok(())
    }

    fn webmcp_release_child(&mut self, child: &str) {
        let _ = self.raw_call("Target.detachFromTarget", json!({"sessionId":child}), None, DOCUMENT_CHECK);
        self.blocked.insert(child.to_string());
    }

    fn webmcp_child_arm(&mut self, session: &str, seed: &Value, parent_direct: bool) -> Result<String, String> {
        let shim = vetted_source(SHIM)?;
        let bridge = private_bridge(getrandom::fill)?;
        let source = format!("const {bridge} = {shim};");
        let mut loaders = BTreeMap::new();
        Self::webmcp_loaders(&seed["frameTree"], &mut loaders);
        self.webmcp.arms.insert(session.to_string(), Arm {
            source,
            bridge: bridge.clone(),
            identifier: None,
            conversion: None,
            plain: true,
            direct_child_attach: parent_direct,
            root_identity: None,
            pending_children: BTreeMap::new(),
            child_done: BTreeSet::new(),
            admitted: 0,
            revoked: false,
            revoked_at: None,
            revoked_reason: None,
            pending_contexts: BTreeMap::new(),
            pending_install: None,
            pending_install_loader: None,
            pending_frames: BTreeMap::new(),
            accepted: BTreeSet::new(),
            installing: BTreeSet::new(),
            loaders,
            documents: BTreeMap::new(),
            parked_origin: String::new(),
            context_generation: 0,
            frame_accept_generation: BTreeMap::new(),
        });
        self.raw_call("Network.enable", json!({}), Some(session), DOCUMENT_CHECK)?;
        self.raw_call("Runtime.disable", json!({}), Some(session), DOCUMENT_CHECK)?;
        self.raw_call("Runtime.enable", json!({}), Some(session), DOCUMENT_CHECK)?;
        self.worlds.enable(session);
        self.raw_call("Page.enable", json!({}), Some(session), DOCUMENT_CHECK)?;
        Ok(bridge)
    }

    fn webmcp_record_document_policy(&mut self, session: &str, frame: &str, loader: &str, origin: &str, policy: &str, source: &str) {
        if frame.is_empty() || loader.is_empty() || origin.is_empty() || origin == "null" { return; }
        let policy = if policy.len() > 16384 { "tools=()" } else { policy };
        if let Some(arm) = self.webmcp.arms.get_mut(session) {
            let key = format!("{frame}:{loader}");
            let stronger = arm.documents.get(&key).and_then(|document| document.get("source").and_then(Value::as_str)).is_some_and(|known| known == "network" || known == "native");
            if source == "fetched" && stronger { return; }
            arm.documents.insert(key, json!({"frameId":frame,"loaderId":loader,"origin":origin,"policy":policy,"source":source}));
        }
    }

    fn webmcp_policy_origin_usable(&self, origin: &str) -> bool {
        !origin.is_empty() && origin != "null" && !self.guard.url(origin) && !self.guard.extension_frame(origin)
    }

    fn webmcp_observe_child_policy(&mut self, child: &str, frame_id: &str, loader: &str, origin: &str, url: &str) {
        let key = format!("{frame_id}:{loader}");
        if self.webmcp.arms.get(child).is_some_and(|arm| {
            arm.documents.get(&key).and_then(|document| document.get("source").and_then(Value::as_str)).is_some_and(|source| source == "network" || source == "native")
        }) {
            return;
        }
        if url.is_empty() || self.guard.url(url) || !self.webmcp_policy_origin_usable(origin) { return; }
        if let Ok(result) = self.raw_call("Page.getPermissionsPolicyState", json!({"frameId":frame_id}), Some(child), DOCUMENT_CHECK) {
            if let Some(state) = result.get("states").and_then(Value::as_array).and_then(|states| states.iter().find(|state| state.get("feature").and_then(Value::as_str) == Some("tools"))) {
                let policy = if state.get("allowed").and_then(Value::as_bool) == Some(true) { "tools=*" } else { "tools=()" };
                self.webmcp_record_document_policy(child, frame_id, loader, origin, policy, "native");
                return;
            }
        }
        let bound_frame = frame_id.to_string();
        let bound_loader = loader.to_string();
        let bound_origin = origin.to_string();
        let Ok(result) = self.raw_call(
            "Network.loadNetworkResource",
            json!({"frameId":frame_id,"url":url,"options":{"disableCache":true,"includeCredentials":false}}),
            Some(child),
            DOCUMENT_CHECK,
        ) else { return; };
        if result.to_string().len() > 65536 { return; }
        if result.pointer("/resource/success").and_then(Value::as_bool) != Some(true) { return; }
        let Some(status) = result.pointer("/resource/httpStatusCode").and_then(Value::as_u64) else { return; };
        if !(200..300).contains(&status) { return; }
        let Some(headers) = result.pointer("/resource/headers").and_then(Value::as_object) else { return; };
        let Some(policy) = headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("permissions-policy")).and_then(|(_, value)| value.as_str()).filter(|policy| !policy.is_empty()) else { return; };
        let Ok(tree) = self.raw_call("Page.getFrameTree", json!({}), Some(child), OPTIONAL_STEP) else { return; };
        let frame = &tree["frameTree"]["frame"];
        if frame.get("id").and_then(Value::as_str) != Some(&bound_frame)
            || frame.get("loaderId").and_then(Value::as_str) != Some(&bound_loader)
            || frame.get("securityOrigin").and_then(Value::as_str) != Some(&bound_origin)
            || !self.webmcp_policy_origin_usable(&bound_origin)
        {
            return;
        }
        self.webmcp_record_document_policy(child, &bound_frame, &bound_loader, &bound_origin, policy, "fetched");
    }

    fn webmcp_parent_frame_id(&mut self, target: &str, until: Instant) -> Result<String, String> {
        if let Some(frame) = self.targets.get(target).and_then(|info| info.get("parentFrameId")).and_then(Value::as_str).filter(|frame| !frame.is_empty()) {
            return Ok(frame.to_string());
        }
        let left = until.saturating_duration_since(Instant::now()).min(OPTIONAL_STEP);
        let info = self.raw_call("Target.getTargetInfo", json!({"targetId":target}), None, left)?;
        let frame = info.pointer("/targetInfo/parentFrameId").and_then(Value::as_str).filter(|frame| !frame.is_empty()).ok_or_else(|| self.refusal())?.to_string();
        if let Some(Value::Object(stored)) = self.targets.get_mut(target) {
            stored.insert("parentFrameId".into(), json!(frame.clone()));
        } else if let Some(target_info) = info.get("targetInfo").cloned() {
            self.targets.insert(target.to_string(), target_info);
        }
        Ok(frame)
    }

    fn webmcp_parked_child(&self, session: &str) -> bool {
        self.webmcp.parents.contains_key(session)
            && self.webmcp.quarantined.contains(session)
            && self.webmcp.arms.get(session).is_some_and(|arm| !arm.revoked && arm.admitted == 0 && arm.pending_install.is_none())
    }

    fn webmcp_detach_parked_children(&mut self, parent: &str) {
        let parked = self.webmcp.parents.iter().filter(|(child, owner)| *owner == parent && self.webmcp_parked_child(child)).map(|(child, _)| child.clone()).collect::<Vec<_>>();
        for child in parked {
            self.webmcp_settle_child(&child, "a cross-origin child could not verify its document; synchronous registration before installation is unavailable");
        }
    }

    fn webmcp_detach_children(&mut self, parent: &str) {
        let children = self.webmcp.parents.iter().filter(|(child, owner)| *owner == parent && !self.blocked.contains(*child)).map(|(child, _)| child.clone()).collect::<Vec<_>>();
        for child in children {
            self.webmcp_settle_child(&child, "a cross-origin child could not verify its document; synchronous registration before installation is unavailable");
        }
    }

    fn webmcp_remember_child_origin(&mut self, child: &str, origin: &str) {
        if origin.is_empty() || origin == "null" { return; }
        if let Some(arm) = self.webmcp.arms.get_mut(child) {
            arm.parked_origin = origin.to_string();
        }
    }

    fn webmcp_child_origin(&self, child: &str) -> Option<String> {
        self.webmcp.contexts.iter().find(|((owner, _), context)| owner == child && !context.origin.is_empty() && context.origin != "null")
            .map(|(_, context)| context.origin.clone())
            .or_else(|| {
                self.session_target(child).and_then(|target| self.documents.get(&target)).and_then(|document| {
                    if !document.origin.is_empty() && document.origin != "null" {
                        return Some(document.origin.clone());
                    }
                    url::Url::parse(&document.url).ok().map(|parsed| parsed.origin().ascii_serialization())
                }).filter(|origin| !origin.is_empty() && origin != "null")
            })
            .or_else(|| {
                self.webmcp.arms.get(child).map(|arm| arm.parked_origin.clone()).filter(|origin| !origin.is_empty() && origin != "null")
            })
    }

    fn webmcp_pend_install(&mut self, child: &str, loader: &str) {
        if loader.is_empty() { return; }
        if let Some(arm) = self.webmcp.arms.get_mut(child) {
            if arm.pending_install.is_none() || arm.pending_install_loader.as_deref() != Some(loader) {
                arm.pending_install = Some(Instant::now() + DOCUMENT_CHECK);
                arm.pending_install_loader = Some(loader.to_string());
            }
        }
    }

    fn webmcp_adopt_install(&mut self, child: &str, parent: &str, until: Instant) -> Result<bool, String> {
        let target = self.session_target(child).ok_or_else(|| self.refusal())?;
        let tree = self.webmcp_stable(child, until, |cdp, left| cdp.raw_call("Page.getFrameTree", json!({}), Some(child), left.min(OPTIONAL_STEP)))?;
        if !self.webmcp_tree_verified(&tree, None) { return Ok(false); }
        self.main_commit(child, &tree["frameTree"]["frame"]);
        let frame = &tree["frameTree"]["frame"];
        let origin = frame.get("securityOrigin").and_then(Value::as_str).unwrap_or("").to_string();
        let loader = frame.get("loaderId").and_then(Value::as_str).unwrap_or("").to_string();
        let url = frame.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        if origin.is_empty() || origin == "null" || loader.is_empty() { return Ok(false); }
        self.webmcp_remember_child_origin(child, &origin);
        if self.guard.url(&origin) || self.guard.extension_frame(&origin)
            || self.frame_protected(frame.get("id").and_then(Value::as_str).unwrap_or(""))
            || frame.get("url").and_then(Value::as_str).is_some_and(|url| self.guard.extension_frame(url) || self.guard.url(url)) {
            return Err(self.refusal());
        }
        if !self.webmcp.requested_origins.contains(&origin) {
            if let Some(arm) = self.webmcp.arms.get_mut(child) {
                arm.pending_install = None;
                arm.pending_install_loader = None;
            }
            return Ok(false);
        }
        self.webmcp_pend_install(child, &loader);
        let parent_root = self.webmcp_root_document(parent, until)?;
        let frame_id = frame.get("id").and_then(Value::as_str).unwrap_or("");
        let last_accepted = self.webmcp.arms.get(child).and_then(|arm| arm.frame_accept_generation.get(frame_id).copied());
        let matched = self.webmcp.contexts.iter().filter(|((owner, _), context)| {
            owner == child
                && context.frame == frame_id
                && context.origin == origin
                && !context.unique.is_empty()
                && (context.loader.is_empty() || context.loader == loader)
        }).map(|(key, context)| (key.1, context.clone())).collect::<Vec<_>>();
        let accepted = |context: &Context| self.webmcp.arms.get(child).is_some_and(|arm| arm.accepted.contains(&(context.frame.clone(), if context.loader.is_empty() { loader.clone() } else { context.loader.clone() }, context.unique.clone())));
        let chosen = matched.iter().find(|(_, context)| accepted(context)).cloned()
            .or_else(|| matched.iter().filter(|(_, context)| last_accepted.is_none_or(|last| context.generation > last)).max_by_key(|(_, context)| context.generation).cloned());
        let Some((id, mut context)) = chosen else {
            self.webmcp_pend_install(child, &loader);
            return Ok(false);
        };
        context.loader = loader.clone();
        if let Some(stored) = self.webmcp.contexts.get_mut(&(child.to_string(), id)) {
            stored.loader = loader.clone();
        }
        self.webmcp_observe_child_policy(child, frame_id, &loader, &origin, &url);
        if let Some(arm) = self.webmcp.arms.get_mut(child) {
            arm.loaders.insert(frame_id.to_string(), context.loader.clone());
            arm.root_identity = Some((target.clone(), context.clone()));
            arm.pending_install = None;
            arm.pending_install_loader = None;
        }
        let realm = (context.frame.clone(), context.loader.clone(), context.unique.clone());
        if self.webmcp.arms.get(child).is_some_and(|arm| arm.accepted.contains(&realm)) {
            self.webmcp_mark_admitted(child, id, &context, Instant::now())?;
            return Ok(true);
        }
        if self.webmcp.arms.get(child).is_some_and(|arm| arm.installing.contains(&realm)) {
            return Ok(false);
        }
        self.webmcp_evaluate(child, id, &context, parent_root, until)?;
        self.webmcp_mark_admitted(child, id, &context, Instant::now())?;
        Ok(true)
    }

    fn webmcp_finish_awaiting_children(&mut self) {
        if self.webmcp.handling_children || self.webmcp.handling_gate { return; }
        self.webmcp.handling_children = true;
        let waiting = self.webmcp.quarantined.iter().filter(|child| {
            self.webmcp.parents.contains_key(*child)
                && self.webmcp.arms.get(*child).is_some_and(|arm| !arm.revoked && arm.admitted == 0 && arm.installing.is_empty())
                && self.webmcp_child_origin(child).is_some_and(|origin| self.webmcp.requested_origins.contains(&origin))
        }).cloned().collect::<Vec<_>>();
        for child in waiting {
            let Some(parent) = self.webmcp.parents.get(&child).cloned() else { continue; };
            let loader = self.session_target(&child).and_then(|target| self.documents.get(&target)).map(|document| document.frame_id.clone())
                .and_then(|frame| self.webmcp.arms.get(&child).and_then(|arm| arm.loaders.get(&frame).cloned())).unwrap_or_default();
            self.webmcp_pend_install(&child, &loader);
            let until = Instant::now() + DOCUMENT_CHECK;
            match self.webmcp_adopt_install(&child, &parent, until) {
                Ok(true) => {}
                Ok(false) => {}
                Err(_) => {
                    self.webmcp_settle_child(&child, "a cross-origin child could not verify its document; synchronous registration before installation is unavailable");
                }
            }
        }
        self.webmcp.handling_children = false;
    }

    fn webmcp_adopt_child(&mut self, value: &Value) {
        if self.webmcp.buffering_attach {
            self.webmcp.pending_attaches.push_back(value.clone());
            return;
        }
        let Some(child) = value.pointer("/params/sessionId").and_then(Value::as_str).map(str::to_string) else { return; };
        let parent = value.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
        let info = &value["params"]["targetInfo"];
        let target = info.get("targetId").and_then(Value::as_str).unwrap_or("").to_string();
        let url = info.get("url").and_then(Value::as_str).unwrap_or("").to_string();
        let direct = self.webmcp.arms.get(&parent).is_some_and(|arm| arm.direct_child_attach);
        if !direct || self.guard.extension_frame(&url) {
            self.webmcp_release_child(&child);
            return;
        }
        let provisional = url.is_empty() || url == "about:blank";
        let allowed = info.get("type").and_then(Value::as_str) == Some("iframe") && !target.is_empty()
            && (provisional || url::Url::parse(&url).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https")))
            && !self.guard.url(&url) && !self.info_protected(info, &[]) && !self.content_blocked(&parent);
        let prior = self.webmcp.parents.keys().filter(|session| {
            *session != &child && self.session_targets.get(*session) == Some(&target)
        }).cloned().collect::<Vec<_>>();
        for old in prior {
            self.webmcp_settle_child(&old, "a cross-origin child could not verify its document; synchronous registration before installation is unavailable");
        }
        self.session_targets.insert(child.clone(), target.clone());
        self.webmcp.quarantined.insert(child.clone());
        self.webmcp.parents.insert(child.clone(), parent.clone());
        self.targets.insert(target.clone(), info.clone());
        let outcome = (|| -> Result<(), String> {
            if !allowed { return Err(self.refusal()); }
            self.guards.entry(target.clone()).or_insert_with(|| child.clone());
            if self.is_protected_target(&target) { return Err(self.refusal()); }
            let until = Instant::now() + DOCUMENT_CHECK;
            self.webmcp_parent_frame_id(&target, until)?;
            let seed = self.webmcp_stable(&child, until, |cdp, left| cdp.raw_call("Page.getFrameTree", json!({}), Some(&child), left.min(OPTIONAL_STEP))).unwrap_or_else(|_| json!({"frameTree":{"frame":{"id":target,"url":url}}}));
            if seed.pointer("/frameTree/frame/url").and_then(Value::as_str).is_some_and(|frame_url| self.guard.extension_frame(frame_url)) {
                return Err(self.refusal());
            }
            let verified = self.webmcp_tree_verified(&seed, None);
            let parked = self.webmcp.quarantined.iter().filter(|session| {
                self.webmcp.parents.contains_key(*session)
                    && self.webmcp.arms.get(*session).is_none_or(|arm| !arm.revoked && arm.admitted == 0 && arm.pending_install.is_none())
            }).count();
            if parked > 1024 {
                return Err(self.refusal());
            }
            self.webmcp_child_arm(&child, &seed, direct)?;
            let parent_frame = self.webmcp.arms.get(&parent).and_then(|arm| arm.root_identity.as_ref()).map(|(_, context)| context.frame.clone())
                .or_else(|| self.session_target(&parent).and_then(|target| self.documents.get(&target)).map(|document| document.frame_id.clone()));
            let seed_frame = seed.pointer("/frameTree/frame/id").and_then(Value::as_str);
            let child_tree = seed_frame.is_some_and(|frame| parent_frame.as_deref() != Some(frame));
            if child_tree && verified {
                let _ = self.webmcp_adopt_install(&child, &parent, Instant::now() + OPTIONAL_STEP);
            } else if let Some(arm) = self.webmcp.arms.get_mut(&child) {
                arm.pending_install = None;
                arm.pending_install_loader = None;
            }
            if let Some(origin) = seed.pointer("/frameTree/frame/securityOrigin").and_then(Value::as_str) {
                self.webmcp_remember_child_origin(&child, origin);
            } else if !provisional {
                if let Ok(parsed) = url::Url::parse(&url) {
                    self.webmcp_remember_child_origin(&child, &parsed.origin().ascii_serialization());
                }
            }
            Ok(())
        })();
        if outcome.is_err() {
            self.webmcp_settle_child(&child, "a cross-origin child could not verify its document; synchronous registration before installation is unavailable");
        }
    }

    pub(super) fn webmcp_cancel_queued(&mut self, session: &str) {
        let cancelled = self.webmcp.pending_gates.iter().filter(|value| value.get("sessionId").and_then(Value::as_str) == Some(session) || value.pointer("/params/sessionId").and_then(Value::as_str) == Some(session)).cloned().collect::<Vec<_>>();
        let mut dropped = 0;
        self.webmcp.pending_gates.retain(|value| {
            let matches = value.get("sessionId").and_then(Value::as_str) == Some(session) || value.pointer("/params/sessionId").and_then(Value::as_str) == Some(session);
            if matches { dropped += gate_bytes(value); }
            !matches
        });
        self.webmcp.pending_gate_bytes = self.webmcp.pending_gate_bytes.saturating_sub(dropped);
        for value in cancelled {
            let owner = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
            if value.get("method").and_then(Value::as_str) == Some("Debugger.paused") {
                let _ = self.send("Debugger.resume", json!({}), Some(owner));
            } else if let Some(child) = value.pointer("/params/sessionId").and_then(Value::as_str) {
                self.webmcp_revoke(child);
                let _ = self.send("Debugger.resume", json!({}), Some(child));
                let _ = self.send("Target.detachFromTarget", json!({"sessionId":child}), None);
            }
        }
    }

    fn webmcp_refuse_gate(&mut self, value: &Value) {
        if value.get("method").and_then(Value::as_str) == Some("Debugger.paused") {
            let owner = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
            let _ = self.send("Debugger.resume", json!({}), Some(owner));
            self.webmcp.paused.remove(owner);
        } else if value.get("method").and_then(Value::as_str) == Some("Target.attachedToTarget") {
            self.webmcp_adopt_child(value);
        }
    }

    fn webmcp_gate_event(&mut self, value: &Value) {
        if self.webmcp.buffering_attach && value.get("method").and_then(Value::as_str) == Some("Target.attachedToTarget") {
            self.webmcp.pending_attaches.push_back(value.clone());
            return;
        }
        let paused = value.get("method").and_then(Value::as_str) == Some("Debugger.paused");
        if paused {
            if let Some(session) = value.get("sessionId").and_then(Value::as_str) { self.webmcp.paused.insert(session.to_string()); }
        }
        let bytes = gate_bytes(value);
        let count_cap = self.webmcp.pending_count_cap();
        let bytes_cap = self.webmcp.pending_bytes_cap();
        if self.webmcp.pending_gates.len() >= count_cap || self.webmcp.pending_gate_bytes.saturating_add(bytes) > bytes_cap {
            self.webmcp_refuse_gate(value);
            return;
        }
        self.webmcp.pending_gates.push_back(value.clone());
        self.webmcp.pending_gate_bytes = self.webmcp.pending_gate_bytes.saturating_add(bytes);
        if self.webmcp.handling_gate { return; }
        self.webmcp.handling_gate = true;
        while let Some(gate) = self.webmcp.pending_gates.pop_front() {
            self.webmcp.drop_pending(&gate);
            let owner = gate.get("sessionId").and_then(Value::as_str).unwrap_or("");
            if gate.get("method").and_then(Value::as_str) == Some("Target.attachedToTarget") {
                self.webmcp_adopt_child(&gate);
            } else {
                self.webmcp_handle_pause(&gate, owner);
            }
        }
        self.webmcp.handling_gate = false;
        self.webmcp_finish_awaiting_children();
        self.webmcp_install_children();
    }

    fn webmcp_trace_scheme(value: &str) -> &'static str {
        if value.is_empty() { return "empty"; }
        let Ok(url) = url::Url::parse(value) else { return "invalid"; };
        match (url.scheme(), url.path()) {
            ("about", "blank") => "about:blank",
            ("about", "srcdoc") => "about:srcdoc",
            ("about", _) => "about:other",
            ("chrome-error", _) => "chrome-error",
            ("http", _) => "http",
            ("https", _) => "https",
            ("data", _) => "data",
            ("blob", _) => "blob",
            ("file", _) => "file",
            ("chrome", _) => "chrome",
            ("chrome-extension", _) => "extension",
            _ => "other",
        }
    }

    fn webmcp_trace_frame(&self, frame: &Value) -> Value {
        let text = |key| frame.get(key).and_then(Value::as_str).unwrap_or("");
        json!({"scheme":Self::webmcp_trace_scheme(text("url")),"parentIdPresent":frame.get("parentId").is_some(),"unreachableUrlPresent":frame.get("unreachableUrl").is_some(),"unreachableUrlNonempty":!text("unreachableUrl").is_empty(),"loaderPresent":!text("loaderId").is_empty(),"refusal":self.webmcp_frame_refusal(frame)})
    }

    fn webmcp_trace_order(&self, target: &str) -> Value {
        let mut order = serde_json::Map::new();
        for method in ["attachedToTarget", "detachedFromTarget", "targetInfoChanged"] {
            order.insert(method.into(), json!(self.webmcp.trace_lifecycle.get(&(target.to_string(), method))));
        }
        Value::Object(order)
    }

    pub(super) fn webmcp_trace_fetch_command(&mut self, method: &str, params: &Value, session: Option<&str>) {
        let decision = match method { "Fetch.continueRequest" => "continue", "Fetch.failRequest" => "fail", "Fetch.fulfillRequest" => "fulfill", _ => return };
        let Some(request) = params.get("requestId").and_then(Value::as_str) else { return; };
        let Some((frame, sequence)) = self.webmcp.trace_requests.remove(&(session.unwrap_or("").to_string(), request.to_string())) else { return; };
        let reason = params.get("errorReason").and_then(Value::as_str).map(|reason| match reason { "BlockedByClient" => "BlockedByClient", "Aborted" => "Aborted", "TimedOut" => "TimedOut", _ => "other" });
        if let Some(record) = self.webmcp.trace_fetch.get_mut(&frame).filter(|record| record.get("pauseSequence").and_then(Value::as_u64) == Some(sequence)) { record["decision"] = json!(decision); record["errorReason"] = json!(reason); record["decisionSequence"] = json!(self.webmcp.trace_sequence); }
    }

    fn webmcp_trace_page(&self, session: &str) -> &'static str {
        let mut root = session;
        let mut visited = BTreeSet::new();
        while let Some(parent) = self.webmcp.parents.get(root) {
            if visited.len() >= MAX_TREE_DEPTH || !visited.insert(root) { return "unknown"; }
            root = parent;
        }
        let url = self.session_target(root).and_then(|target| self.documents.get(&target)).map(|document| document.url.as_str()).unwrap_or("");
        match url::Url::parse(url).ok().map(|url| url.path().to_string()).as_deref() { Some("/frames") => "frames", Some("/ordinary-frames") => "ordinary-frames", Some(_) => "other", None => "unknown" }
    }

    fn webmcp_trace_navigation(&mut self, session: &str, frame: &Value) {
        if std::env::var("SLICC_WEBMCP_TRACE").ok().as_deref() != Some("1") { return; }
        let sequence = self.webmcp.trace_sequence;
        let page = self.webmcp_trace_page(session);
        let target = self.session_target(session).unwrap_or_default();
        let kind = self.targets.get(&target).and_then(|info| info.get("type")).and_then(Value::as_str).unwrap_or("");
        let kind = match kind { "page" => "page", "iframe" => "iframe", "worker" => "worker", "" => "unknown", _ => "other" };
        let before = self.webmcp_trace_order(&target);
        let epoch = self.frame_epoch;
        let tree = self.guards.get(&target).cloned().and_then(|guard| self.raw_exchange("Page.getFrameTree", json!({}), Some(&guard), OPTIONAL_STEP, false).ok());
        let id = frame.get("id").and_then(Value::as_str).unwrap_or("");
        let current = tree.as_ref().and_then(|tree| tree.get("frameTree")).and_then(|node| Self::webmcp_frame(node, id));
        let current_metadata = current.map(|current| self.webmcp_trace_frame(current));
        let loader_equal = current.map(|current| current.get("loaderId").and_then(Value::as_str).filter(|loader| !loader.is_empty()) == frame.get("loaderId").and_then(Value::as_str).filter(|loader| !loader.is_empty()));
        let trace = json!({"event":"guard-navigation-trace","sequence":sequence,"targetType":kind,"eventFrame":self.webmcp_trace_frame(frame),"guardTreeAvailable":tree.is_some(),"frameInGuardTree":current.is_some(),"guardFrame":current_metadata,"loaderEqual":loader_equal,"frameEpochStable":epoch == self.frame_epoch,"lifecycleBefore":before,"lifecycleAfter":self.webmcp_trace_order(&target),"probeEndSequence":self.webmcp.trace_sequence,"documentFetch":self.webmcp.trace_fetch.get(id),"page":page,"pageAfter":self.webmcp_trace_page(session),"unreachableProtected":frame.get("unreachableUrl").and_then(Value::as_str).is_some_and(|url| !url.is_empty() && self.guard.url(url)),"targetURLProtected":self.targets.get(&target).and_then(|info| info.get("url")).and_then(Value::as_str).is_some_and(|url| self.guard.url(url))});
        self.notices.push(format!("webmcp-polyfill: {trace}\n"));
    }

    fn webmcp_trace_event(&mut self, value: &Value) {
        if std::env::var("SLICC_WEBMCP_TRACE").ok().as_deref() != Some("1") { return; }
        self.webmcp.trace_sequence += 1;
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        if method == "Page.frameNavigated" || method == "Debugger.paused" {
            let frame = if method == "Page.frameNavigated" { value.pointer("/params/frame/id").and_then(Value::as_str) } else { value.pointer("/params/callFrames/0/location/scriptId").and_then(Value::as_str).and_then(|script| self.webmcp.scripts.get(&(session.to_string(), script.to_string()))).and_then(|id| self.webmcp.contexts.get(&(session.to_string(), *id))).map(|context| context.frame.as_str()) };
            let top = self.session_target(session).and_then(|target| self.documents.get(&target)).map(|doc| doc.frame_id.as_str());
            let role = if self.webmcp.parents.contains_key(session) { "OOPIF" } else if frame.is_none() { "unknown" } else if frame == top { "top" } else { "subframe" };
            self.webmcp.trace_roles.insert(session.to_string(), role);
        }
        let lifecycle = match method { "Target.attachedToTarget" => Some("attachedToTarget"), "Target.detachedFromTarget" => Some("detachedFromTarget"), "Target.targetInfoChanged" => Some("targetInfoChanged"), _ => None };
        if method == "Fetch.requestPaused" && value.pointer("/params/resourceType").and_then(Value::as_str) == Some("Document") {
            if let (Some(request), Some(frame)) = (value.pointer("/params/requestId").and_then(Value::as_str), value.pointer("/params/frameId").and_then(Value::as_str)) {
                if self.webmcp.trace_requests.len() >= 1024 || self.webmcp.trace_fetch.len() >= 1024 { self.webmcp.trace_requests.clear(); self.webmcp.trace_fetch.clear(); }
                self.webmcp.trace_requests.insert((session.to_string(), request.to_string()), (frame.to_string(), self.webmcp.trace_sequence));
                let protected = value.pointer("/params/request/url").and_then(Value::as_str).is_some_and(|url| self.guard.url(url));
                self.webmcp.trace_fetch.insert(frame.to_string(), json!({"pauseSequence":self.webmcp.trace_sequence,"decision":"pending","destinationProtected":protected,"guardSession":self.guard_sessions.contains(session)}));
            }
        }
        if let Some(lifecycle) = lifecycle {
            let target = value.pointer("/params/targetInfo/targetId").or_else(|| value.pointer("/params/targetId")).and_then(Value::as_str).map(str::to_string)
                .or_else(|| value.pointer("/params/sessionId").and_then(Value::as_str).and_then(|session| self.session_target(session))).unwrap_or_default();
            if self.webmcp.trace_lifecycle.len() >= 1024 { self.webmcp.trace_lifecycle.clear(); }
            self.webmcp.trace_lifecycle.insert((target, lifecycle), self.webmcp.trace_sequence);
        }
    }

    pub(super) fn webmcp_event(&mut self, value: &Value) -> bool {
        self.webmcp_trace_event(value);
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        if self.webmcp_plain(session) && method.starts_with("Debugger.") { return true; }
        if method == "Target.attachedToTarget" && self.webmcp.arms.contains_key(session) {
            self.webmcp_gate_event(value);
            return true;
        }
        if matches!(method, "Page.frameDetached" | "Runtime.executionContextCreated" | "Runtime.executionContextDestroyed" | "Runtime.executionContextsCleared" | "Debugger.scriptParsed") {
            self.webmcp_context_event(value, session, method);
            if self.webmcp.parents.contains_key(session) {
                self.webmcp_finish_awaiting_children();
            }
            return false;
        }
        if !self.webmcp.arms.contains_key(session) { return false; }
        if method == "Network.responseReceived" && value.pointer("/params/type").and_then(Value::as_str) == Some("Document") {
            let params = &value["params"];
            let url = params.pointer("/response/url").and_then(Value::as_str).unwrap_or("");
            let frame = params.get("frameId").and_then(Value::as_str).unwrap_or("");
            let loader = params.get("loaderId").and_then(Value::as_str).unwrap_or("");
            if !frame.is_empty() && !loader.is_empty() && !self.guard.url(url) {
                let policy = params.pointer("/response/headers").and_then(Value::as_object).and_then(|headers| headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("permissions-policy"))).map(|(_, value)| value.as_str().unwrap_or("tools=()")).unwrap_or("");
                let origin = url::Url::parse(url).map(|url| url.origin().ascii_serialization()).unwrap_or_default();
                self.webmcp_record_document_policy(session, frame, loader, &origin, policy, "network");
            }
        }
        if method == "Page.frameNavigated" {
            let frame = &value["params"]["frame"];
            let child_frame = frame.get("parentId").is_some() || self.webmcp.parents.contains_key(session);
            let initial_child = child_frame
                && frame.get("url").and_then(Value::as_str).is_some_and(|url| url.is_empty() || url == "about:blank")
                && frame.get("unreachableUrl").and_then(Value::as_str).is_none_or(|url| url.is_empty());
            if let Some(reason) = self.webmcp_frame_refusal(frame).filter(|_| !initial_child) {
                self.webmcp_trace_navigation(session, frame);
                let role = if self.webmcp.parents.contains_key(session) { "OOPIF" } else if frame.get("parentId").is_some() { "subframe" } else { "top frame" };
                self.notices.push(format!("webmcp-polyfill: frame navigation refused ({role}, {reason})\n"));
                if !child_frame {
                    self.webmcp_revoke_because(session, "frame navigation refused");
                }
            }
            if self.webmcp.parents.contains_key(session) && frame.get("parentId").is_none() && !initial_child {
                self.main_commit(session, frame);
                if let Some(origin) = frame.get("securityOrigin").and_then(Value::as_str).filter(|origin| !origin.is_empty() && *origin != "null") {
                    self.webmcp_remember_child_origin(session, origin);
                } else if let Some(url) = frame.get("url").and_then(Value::as_str) {
                    if let Ok(parsed) = url::Url::parse(url) {
                        self.webmcp_remember_child_origin(session, &parsed.origin().ascii_serialization());
                    }
                }
            }
            self.webmcp_navigated(session, value);
            if self.webmcp.parents.contains_key(session) {
                self.webmcp_finish_awaiting_children();
            }
        }
        if method == "Debugger.paused" {
            self.webmcp_gate_event(value);
            return true;
        }
        false
    }

    fn webmcp_navigated(&mut self, session: &str, value: &Value) {
        let child = self.webmcp.parents.contains_key(session);
        let subframe = value.pointer("/params/frame/parentId").is_some();
        let frame_id = value.pointer("/params/frame/id").and_then(Value::as_str).unwrap_or("").to_string();
        let loader = value.pointer("/params/frame/loaderId").and_then(Value::as_str).unwrap_or("").to_string();
        let Some(arm) = self.webmcp.arms.get_mut(session) else { return; };
        Self::webmcp_loaders(&json!({"frame":&value["params"]["frame"]}), &mut arm.loaders);
        if !frame_id.is_empty() && !loader.is_empty() {
            arm.loaders.insert(frame_id.clone(), loader.clone());
        }
        let restored = value.pointer("/params/type").and_then(Value::as_str) == Some("BackForwardCacheRestore")
            && arm.accepted.iter().any(|(known, generation, _)| known == &frame_id && generation == &loader);
        if !child && !subframe && !frame_id.is_empty() && !loader.is_empty() {
            if restored { arm.pending_frames.remove(&frame_id); }
            else { arm.pending_frames.insert(frame_id.clone(), (loader.clone(), Instant::now() + Duration::from_secs(2))); }
        }
        if !frame_id.is_empty() && !loader.is_empty() {
            for ((owner, _), context) in &mut self.webmcp.contexts {
                if owner == session && context.frame == frame_id && context.loader.is_empty() {
                    context.loader = loader.clone();
                }
            }
        }
    }

    fn webmcp_context_event(&mut self, value: &Value, session: &str, method: &str) {
        let root = self.session_target(session).and_then(|target| self.documents.get(&target)).map(|document| document.frame_id.clone());
        let Some(arm) = self.webmcp.arms.get_mut(session) else { return; };
        if method == "Page.frameDetached" {
            if let Some(frame) = value.pointer("/params/frameId").and_then(Value::as_str) {
                arm.pending_frames.remove(frame);
                arm.loaders.remove(frame);
                for ((owner, id), context) in &self.webmcp.contexts { if owner == session && context.frame == frame { arm.pending_contexts.remove(id); } }
                self.webmcp.contexts.retain(|(owner, _), context| owner != session || context.frame != frame);
            }
        }
        if method == "Runtime.executionContextCreated" {
            let context = &value["params"]["context"];
            if context.pointer("/auxData/isDefault").and_then(Value::as_bool) == Some(true) {
                if let Some(id) = context.get("id").and_then(Value::as_i64) {
                    let frame = context.pointer("/auxData/frameId").and_then(Value::as_str).unwrap_or("");
                    let unique = context.get("uniqueId").and_then(Value::as_str).unwrap_or("");
                    let origin = context.get("origin").and_then(Value::as_str).unwrap_or("").to_string();
                    let child = self.webmcp.parents.contains_key(session);
                    let loader = if child { String::new() } else { arm.loaders.get(frame).cloned().unwrap_or_default() };
                    if root.as_deref() == Some(frame) && !arm.accepted.contains(&(frame.to_string(), loader.clone(), unique.to_string())) {
                        arm.pending_contexts.insert(id, Instant::now() + Duration::from_secs(2));
                    }
                    if child && !origin.is_empty() && origin != "null" {
                        arm.parked_origin = origin.clone();
                    }
                    self.webmcp.contexts.insert((session.to_string(), id), Context {
                        frame: frame.to_string(),
                        origin,
                        unique: unique.to_string(),
                        loader,
                        generation: arm.context_generation,
                    });
                }
            }
        }
        if method == "Runtime.executionContextDestroyed" {
            if let Some(id) = value.pointer("/params/executionContextId").and_then(Value::as_i64) {
                self.webmcp.contexts.remove(&(session.to_string(), id));
                arm.pending_contexts.remove(&id);
                Webmcp::forget_context_ids(&mut self.webmcp.scripts, &mut self.webmcp.evaluations, session, id);
            }
        }
        if method == "Runtime.executionContextsCleared" {
            arm.context_generation = arm.context_generation.saturating_add(1);
            arm.pending_contexts.clear();
            self.webmcp.contexts.retain(|(owner, _), _| owner != session);
            self.webmcp.scripts.retain(|(owner, _), _| owner != session);
            self.webmcp.evaluations.retain(|(owner, _)| owner != session);
        }
        if method == "Debugger.scriptParsed" {
            if let (Some(script), Some(context)) = (value.pointer("/params/scriptId").and_then(Value::as_str), value.pointer("/params/executionContextId").and_then(Value::as_i64)) {
                let key = (session.to_string(), script.to_string());
                let evaluate = value.pointer("/params/url").and_then(Value::as_str) == Some("") && value.pointer("/params/embedderName").and_then(Value::as_str) == Some("");
                let id_cap = self.webmcp.id_cap();
                let new_script = !self.webmcp.scripts.contains_key(&key);
                let new_evaluate = evaluate && !self.webmcp.evaluations.contains(&key);
                if (new_script && self.webmcp.session_script_count(session) >= id_cap) || (new_evaluate && self.webmcp.session_evaluation_count(session) >= id_cap) {
                    return;
                }
                self.webmcp.scripts.insert(key.clone(), context);
                self.webmcp.evaluations.remove(&key);
                if evaluate {
                    self.webmcp.evaluations.insert(key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    include!("webmcp_guard_test.rs");
    include!("webmcp_plain_retry_test.rs");
    include!("webmcp_bounds_test.rs");
}
