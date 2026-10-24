use super::{transport_error, Cdp, DOCUMENT_CHECK, OPTIONAL_STEP};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

const SHIM: &str = include_str!("webmcp-shim.js");
const SHIM_HASH: &str = "911b9c665b7bf8183de1d4eeebb4a96d72ddcfe7b6b222316998c595b410fe53";
const GATE: &str = "__slicc_webmcp_gate";
const PERMIT: &str = "__slicc_webmcp_admitted";

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
    admitted: usize,
    revoked: bool,
    revoked_at: Option<(&'static str, u32)>,
    pending_contexts: BTreeMap<i64, Instant>,
    pending_install: Option<Instant>,
    pending_frames: BTreeMap<String, (String, Instant)>,
    accepted: BTreeSet<(String, String, String)>,
    loaders: BTreeMap<String, String>,
    documents: BTreeMap<String, Value>,
}

#[derive(Clone, PartialEq)]
struct Context {
    frame: String,
    origin: String,
    unique: String,
    loader: String,
}

#[derive(Default)]
pub(super) struct Webmcp {
    arms: BTreeMap<String, Arm>,
    contexts: BTreeMap<(String, i64), Context>,
    scripts: BTreeMap<(String, String), i64>,
    paused: BTreeSet<String>,
    handling_gate: bool,
    pending_gates: VecDeque<Value>,
    pub(super) deferred: BTreeSet<String>,
    pub(super) quarantined: BTreeSet<String>,
    parents: BTreeMap<String, String>,
    children: Vec<(String, String, String)>,
    samples: Vec<u128>,
}

impl Webmcp {
    pub(super) fn forget(&mut self, session: &str) {
        self.arms.remove(session);
        self.contexts.retain(|(owner, _), _| owner != session);
        self.scripts.retain(|(owner, _), _| owner != session);
        self.paused.remove(session);
        self.pending_gates.retain(|value| value.get("sessionId").and_then(Value::as_str) != Some(session) && value.pointer("/params/sessionId").and_then(Value::as_str) != Some(session));
        self.deferred.remove(session);
        self.quarantined.remove(session);
        self.parents.retain(|child, parent| child != session && parent != session);
        self.children.retain(|(_, owner, _)| owner != session);
    }
}

impl Cdp {
    pub fn start_webmcp(&mut self, session: &str) -> Result<String, String> {
        self.admit("Runtime.enable", &json!({}), Some(session))?;
        let tree = self.webmcp_stable(session, Instant::now() + DOCUMENT_CHECK, |cdp, left| {
            let target = cdp.session_target(session).ok_or_else(|| cdp.refusal())?;
            let guard = cdp.guards.get(&target).cloned().ok_or_else(|| cdp.refusal())?;
            let tree = cdp.raw_call("Page.getFrameTree", json!({}), Some(&guard), left.min(OPTIONAL_STEP))?;
            if !cdp.webmcp_tree_verified(&tree, None) || cdp.content_blocked(session) || cdp.unobserved.contains(&target) {
                return Err(cdp.refusal());
            }
            Ok(tree)
        })?;
        self.webmcp_install(session, false, Some(&tree))
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
        if !self.webmcp.arms.contains_key(session) || self.content_blocked(session) {
            return Err(self.refusal());
        }
        self.raw_call("Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true,"filter":[{"type":"iframe","exclude":false}]}), Some(session), OPTIONAL_STEP)?;
        Ok(())
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
        self.webmcp.arms.iter().filter(|(session, arm)| !arm.revoked && !self.webmcp.quarantined.contains(*session) && !self.content_blocked(session)).map(|(session, arm)| {
            json!({"session":session,"bridge":arm.bridge,"target":self.session_target(session),"documents":arm.documents})
        }).collect()
    }

    pub fn webmcp_alive(&self, session: &str) -> bool {
        self.webmcp.arms.get(session).is_some_and(|arm| !arm.revoked) && !self.content_blocked(session)
    }

    pub fn webmcp_settled(&self) -> bool {
        self.webmcp.arms.iter().all(|(session, arm)| {
            let quarantined = self.webmcp.quarantined.contains(session);
            arm.revoked || self.blocked.contains(session) || !quarantined && (self.content_blocked(session)
                || arm.pending_install.is_none() && arm.pending_contexts.is_empty() && arm.pending_frames.is_empty())
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
        let now = Instant::now();
        let stale = self.webmcp.arms.iter().filter(|(_, arm)| !arm.revoked && (arm.pending_install.is_some_and(|deadline| deadline <= now) || arm.pending_contexts.values().any(|deadline| *deadline <= now) || arm.pending_frames.values().any(|(_, deadline)| *deadline <= now))).map(|(session, _)| session.clone()).collect::<Vec<_>>();
        for session in stale {
            self.webmcp_revoke(&session);
            let _ = self.send("Debugger.resume", json!({}), Some(&session));
        }
    }

    pub fn stop_webmcp(&mut self, session: &str) {
        self.webmcp_revoke(session);
        let _ = self.raw_call("Target.setAutoAttach", json!({"autoAttach":false,"waitForDebuggerOnStart":false,"flatten":true}), Some(session), Duration::from_secs(2));
        let _ = self.raw_call("Debugger.resume", json!({}), Some(session), Duration::from_secs(2));
        self.webmcp.arms.remove(session);
    }

    fn webmcp_install(&mut self, session: &str, pending: bool, seed: Option<&Value>) -> Result<String, String> {
        let shim = vetted_source(SHIM)?;
        let bridge = private_bridge(getrandom::fill)?;
        let source = format!("const {bridge} = (function {GATE}() {{ 'use strict'; let {PERMIT} = false; debugger; if (!{PERMIT}) return; return {shim}; }})();");
        let mut loaders = BTreeMap::new();
        if let Some(tree) = seed { Self::webmcp_loaders(&tree["frameTree"], &mut loaders); }
        self.webmcp.arms.insert(session.to_string(), Arm { source: source.clone(), bridge: bridge.clone(), identifier: None, admitted: 0, revoked: false, revoked_at: None, pending_contexts: BTreeMap::new(), pending_install: pending.then(|| Instant::now() + Duration::from_secs(2)), pending_frames: BTreeMap::new(), accepted: BTreeSet::new(), loaders, documents: BTreeMap::new() });
        self.raw_call("Runtime.disable", json!({}), Some(session), OPTIONAL_STEP)?;
        self.raw_call("Runtime.enable", json!({}), Some(session), OPTIONAL_STEP)?;
        self.worlds.enable(session);
        self.raw_call("Page.enable", json!({}), Some(session), OPTIONAL_STEP)?;
        self.raw_call("Network.enable", json!({}), Some(session), OPTIONAL_STEP)?;
        self.raw_call("Debugger.enable", json!({}), Some(session), OPTIONAL_STEP)?;
        let added = self.raw_call("Page.addScriptToEvaluateOnNewDocument", json!({"source":source,"runImmediately":seed.is_some()}), Some(session), OPTIONAL_STEP);
        let identifier = added.ok().and_then(|result| result.get("identifier").and_then(Value::as_str).map(str::to_string));
        let Some(identifier) = identifier else {
            self.webmcp_revoke(session);
            self.block(session);
            return Err(self.refusal());
        };
        if let Some(arm) = self.webmcp.arms.get_mut(session) {
            arm.identifier = Some(identifier);
        }
        if !pending {
            let until = Instant::now() + Duration::from_secs(2);
            while self.webmcp.arms.get(session).is_some_and(|arm| (arm.admitted == 0 || !arm.pending_contexts.is_empty()) && !arm.revoked) && Instant::now() < until {
                if !self.pump(until)? { break; }
            }
            if self.webmcp.arms.get(session).is_none_or(|arm| arm.admitted == 0 || !arm.pending_contexts.is_empty() || arm.revoked) {
                self.webmcp_revoke(session);
                let _ = self.raw_call("Debugger.resume", json!({}), Some(session), Duration::from_secs(2));
                return Err(self.refusal());
            }
        }
        if self.webmcp.arms.get(session).is_some_and(|arm| arm.revoked) {
            self.webmcp_revoke(session);
            return Err(self.refusal());
        }
        Ok(bridge)
    }

    pub(super) fn webmcp_is_paused(&self, session: &str) -> bool {
        self.webmcp.paused.contains(session)
    }

    #[track_caller]
    pub(super) fn webmcp_revoke(&mut self, session: &str) {
        let location = std::panic::Location::caller();
        if let Some(arm) = self.webmcp.arms.get_mut(session) {
            if !arm.revoked { arm.revoked_at = Some((location.file(), location.line())); }
            arm.revoked = true;
            if let Some(identifier) = arm.identifier.take() {
                let _ = self.send("Page.removeScriptToEvaluateOnNewDocument", json!({"identifier":identifier}), Some(session));
            }
        }
    }

    fn webmcp_frame_verified(&self, frame: &Value) -> bool {
        let text = |key: &str| frame.get(key).and_then(Value::as_str).unwrap_or("");
        let web = url::Url::parse(text("url")).is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.origin().ascii_serialization() == text("securityOrigin"));
        web && !text("id").is_empty() && !text("loaderId").is_empty() && text("unreachableUrl").is_empty()
            && !self.guard.url(text("url")) && !self.guard.url(text("securityOrigin"))
    }

    fn webmcp_tree_verified(&self, tree: &Value, frame: Option<&str>) -> bool {
        fn find(cdp: &Cdp, node: &Value, frame: Option<&str>) -> bool {
            let current = &node["frame"];
            let id = current.get("id").and_then(Value::as_str).unwrap_or("");
            if !cdp.webmcp_frame_verified(current) || cdp.frame_protected(id) { return false; }
            if frame.is_none_or(|frame| frame == id) { return true; }
            node.get("childFrames").and_then(Value::as_array).is_some_and(|children| children.iter().any(|child| find(cdp, child, frame)))
        }
        tree.get("frameTree").is_some_and(|node| find(self, node, frame))
    }

    fn webmcp_loaders(node: &Value, out: &mut BTreeMap<String, String>) {
        if let (Some(id), Some(loader)) = (node.pointer("/frame/id").and_then(Value::as_str), node.pointer("/frame/loaderId").and_then(Value::as_str)) {
            if !loader.is_empty() { out.insert(id.to_string(), loader.to_string()); }
        }
        for child in node.get("childFrames").and_then(Value::as_array).into_iter().flatten() { Self::webmcp_loaders(child, out); }
    }

    fn webmcp_frame<'a>(node: &'a Value, frame: &str) -> Option<&'a Value> {
        if node.pointer("/frame/id").and_then(Value::as_str) == Some(frame) { return node.get("frame"); }
        node.get("childFrames").and_then(Value::as_array)?.iter().find_map(|child| Self::webmcp_frame(child, frame))
    }

    fn webmcp_verify_pause(&mut self, session: &str, paused: &Value, deadline: Instant) -> Result<(String, u64, i64, Context), String> {
        let call = paused.pointer("/params/callFrames/0").ok_or_else(|| self.refusal())?;
        if call.get("functionName").and_then(Value::as_str) != Some(GATE) { return Err(self.refusal()); }
        let script = call.pointer("/location/scriptId").and_then(Value::as_str).ok_or_else(|| self.refusal())?;
        let context_id = *self.webmcp.scripts.get(&(session.to_string(), script.to_string())).ok_or_else(|| self.refusal())?;
        let context = self.webmcp.contexts.get(&(session.to_string(), context_id)).cloned().ok_or_else(|| self.refusal())?;
        let source = self.raw_call("Debugger.getScriptSource", json!({"scriptId":script}), Some(session), deadline.saturating_duration_since(Instant::now()))?;
        if self.webmcp.arms.get(session).is_none_or(|arm| arm.revoked || source.get("scriptSource").and_then(Value::as_str) != Some(&arm.source)) {
            return Err(self.refusal());
        }
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let guard = self.guards.get(&target).cloned().ok_or_else(|| self.refusal())?;
        let epoch = self.frame_epoch;
        let mut owner = session.to_string();
        let mut ancestors = BTreeSet::new();
        let mut ancestor_documents = Vec::new();
        while let Some(parent) = self.webmcp.parents.get(&owner).cloned() {
            if !ancestors.insert(parent.clone()) || !self.webmcp_alive(&parent) {
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
        let frame = Self::webmcp_frame(&tree["frameTree"], &context.frame).ok_or_else(|| self.refusal())?;
        if frame.get("securityOrigin").and_then(Value::as_str) != Some(&context.origin) || context.unique.is_empty()
            || context.loader.is_empty() || frame.get("loaderId").and_then(Value::as_str) != Some(&context.loader) {
            return Err(self.refusal());
        }
        let fresh = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), deadline.saturating_duration_since(Instant::now()))?;
        let next = Self::webmcp_frame(&fresh["frameTree"], &context.frame).ok_or_else(|| self.refusal())?;
        if epoch != self.frame_epoch || !self.webmcp_tree_verified(&fresh, Some(&context.frame))
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
        if epoch != self.frame_epoch { return Err(self.refusal()); }
        let call_id = call.get("callFrameId").and_then(Value::as_str).ok_or_else(|| self.refusal())?.to_string();
        Ok((call_id, epoch, context_id, context))
    }

    fn webmcp_handle_pause(&mut self, value: &Value, session: &str) {
        let started = Instant::now();
        self.webmcp.paused.insert(session.to_string());
        let deadline = started + Duration::from_secs(2);
        let (verified, mut admitted) = loop {
            let epoch = self.frame_epoch;
            let (verified, admitted) = self.webmcp_admit_pause(value, session, deadline);
            let settled = admitted || epoch == self.frame_epoch || Instant::now() >= deadline || verified.as_ref().is_err_and(|message| transport_error(message));
            if settled || self.blocked.contains(session) || self.webmcp.arms.get(session).is_none_or(|arm| arm.revoked) { break (verified, admitted); }
        };
        if !admitted {
            let removable = self.webmcp.arms.get(session).is_some_and(|arm| arm.identifier.is_some());
            let previous = self.webmcp.arms.get(session).filter(|arm| arm.revoked).and_then(|arm| arm.revoked_at)
                .map(|(file, line)| format!(" installer revoked at {}:{line};", file.rsplit('/').next().unwrap_or("guard"))).unwrap_or_default();
            self.webmcp_revoke(session);
            let rearmed = if removable { self.webmcp_rearm_blank_child(session, value, deadline) } else { Err(self.refusal()) };
            if let Err(message) = rearmed {
                let stage = message.strip_prefix("slicc: refused: WebMCP blank-frame rearm ").map(str::trim).unwrap_or("unverified gate");
                self.notices.push(format!("webmcp-polyfill: document gate refused;{previous} blank-frame rearm {stage}\n"));
            }
        }
        let resumed = self.raw_call("Debugger.resume", json!({}), Some(session), Duration::from_secs(2));
        if resumed.is_err() {
            admitted = false;
            self.webmcp_revoke(session);
            if let Ok((call, _, _, _)) = &verified {
                let _ = self.raw_call("Debugger.evaluateOnCallFrame", json!({"callFrameId":call,"expression":format!("{PERMIT} = false"),"returnByValue":true,"silent":true}), Some(session), Duration::from_secs(2));
            }
            let _ = self.raw_call("Debugger.resume", json!({}), Some(session), Duration::from_secs(2));
        }
        self.webmcp.paused.remove(session);
        if let (true, Ok((_, _, id, context))) = (admitted, verified) {
            if self.webmcp_mark_admitted(session, id, &context, started).is_err() { self.webmcp_revoke(session); }
        }
        if self.webmcp.deferred.contains(session) { self.block(session); }
    }

    fn webmcp_admit_pause(&mut self, value: &Value, session: &str, deadline: Instant) -> (Result<(String, u64, i64, Context), String>, bool) {
        let verified = self.webmcp_verify_pause(session, value, deadline).and_then(|verified| {
            if self.webmcp.quarantined.contains(session) {
                self.raw_call("Target.setAutoAttach", json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true,"filter":[{"type":"iframe","exclude":false}]}), Some(session), deadline.saturating_duration_since(Instant::now()))?;
            }
            Ok(verified)
        });
        let mut admitted = false;
        if let Ok((call, epoch, id, context)) = &verified {
            let result = self.raw_call("Debugger.evaluateOnCallFrame", json!({"callFrameId":call,"expression":format!("{PERMIT} = true"),"returnByValue":true,"silent":true}), Some(session), deadline.saturating_duration_since(Instant::now()));
            let failure = result.as_ref().err().filter(|message| transport_error(message)).cloned();
            admitted = Instant::now() < deadline && result.is_ok_and(|result| result.get("exceptionDetails").is_none() && result.pointer("/result/value").and_then(Value::as_bool) == Some(true))
                && *epoch == self.frame_epoch && !self.blocked.contains(session)
                && (self.webmcp.quarantined.contains(session) || !self.content_blocked(session))
                && self.webmcp.contexts.get(&(session.to_string(), *id)) == Some(context)
                && self.webmcp.arms.get(session).is_some_and(|arm| !arm.revoked);
            if !admitted {
                let _ = self.raw_call("Debugger.evaluateOnCallFrame", json!({"callFrameId":call,"expression":format!("{PERMIT} = false"),"returnByValue":true,"silent":true}), Some(session), Duration::from_secs(2));
            }
            if let Some(message) = failure { return (Err(message), false); }
        }
        (verified, admitted)
    }

    fn webmcp_mark_admitted(&mut self, session: &str, id: i64, expected: &Context, started: Instant) -> Result<(), String> {
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let context = self.webmcp.contexts.get(&(session.to_string(), id)).filter(|context| *context == expected).cloned().ok_or_else(|| self.refusal())?;
        let arm = self.webmcp.arms.get_mut(session).ok_or_else(|| "slicc: refused: WebMCP holder disappeared\n".to_string())?;
        arm.admitted += 1;
        arm.pending_install = None;
        arm.pending_contexts.remove(&id);
        arm.pending_frames.remove(&context.frame);
        if arm.accepted.len() >= 1024 { arm.accepted.clear(); }
        arm.accepted.insert((context.frame, context.loader, context.unique));
        let bridge = arm.bridge.clone();
        self.webmcp.samples.push(started.elapsed().as_micros());
        if self.webmcp.quarantined.remove(session) {
            self.guard_sessions.remove(session);
            self.webmcp.children.push((target, session.to_string(), bridge));
        }
        Ok(())
    }

    fn webmcp_rearm_blank_child(&mut self, session: &str, paused: &Value, deadline: Instant) -> Result<(), String> {
        loop {
            let epoch = self.frame_epoch;
            let result = self.webmcp_rearm_blank_child_once(session, paused, deadline);
            if result.as_ref().is_err_and(|message| epoch != self.frame_epoch && !transport_error(message)) && Instant::now() < deadline && !self.content_blocked(session) { continue; }
            return result;
        }
    }

    fn webmcp_rearm_blank_child_once(&mut self, session: &str, paused: &Value, deadline: Instant) -> Result<(), String> {
        if self.content_blocked(session) || self.webmcp.quarantined.contains(session) { return Err(self.refusal()); }
        let call = paused.pointer("/params/callFrames/0").ok_or_else(|| self.refusal())?;
        if call.get("functionName").and_then(Value::as_str) != Some(GATE) { return Err(self.refusal()); }
        let script = call.pointer("/location/scriptId").and_then(Value::as_str).ok_or_else(|| self.refusal())?;
        let id = *self.webmcp.scripts.get(&(session.to_string(), script.to_string())).ok_or_else(|| self.refusal())?;
        let context = self.webmcp.contexts.get(&(session.to_string(), id)).cloned().ok_or_else(|| "slicc: refused: WebMCP blank-frame rearm lost context\n".to_string())?;
        let target = self.session_target(session).ok_or_else(|| self.refusal())?;
        let guard = self.guards.get(&target).cloned().ok_or_else(|| self.refusal())?;
        let epoch = self.frame_epoch;
        let tree = self.raw_call("Page.getFrameTree", json!({}), Some(&guard), deadline.saturating_duration_since(Instant::now()))?;
        let top = &tree["frameTree"]["frame"];
        let child = Self::webmcp_frame(&tree["frameTree"], &context.frame).ok_or_else(|| "slicc: refused: WebMCP blank-frame rearm lost child frame\n".to_string())?;
        let arm = self.webmcp.arms.get(session).ok_or_else(|| self.refusal())?;
        if epoch != self.frame_epoch || Instant::now() >= deadline { return Err("slicc: refused: WebMCP blank-frame rearm changed frame epoch\n".into()); }
        if !self.webmcp_tree_verified(&tree, None) || self.documents.get(&target).is_none_or(|document| document.navigating || document.previous || document.probed) {
            return Err("slicc: refused: WebMCP blank-frame rearm unverified parent document\n".into());
        }
        if child.get("url").and_then(Value::as_str) != Some("about:blank") || child.get("unreachableUrl").and_then(Value::as_str).is_some_and(|url| !url.is_empty())
            || child.get("parentId") != top.get("id") || child.get("securityOrigin") != top.get("securityOrigin")
            || child.get("securityOrigin").and_then(Value::as_str) != Some(&context.origin) || context.unique.is_empty() {
            return Err("slicc: refused: WebMCP blank-frame rearm unverified child document\n".into());
        }
        if !arm.accepted.iter().any(|(frame, loader, _)| Some(frame.as_str()) == top.get("id").and_then(Value::as_str) && Some(loader.as_str()) == top.get("loaderId").and_then(Value::as_str)) {
            return Err("slicc: refused: WebMCP blank-frame rearm parent gate not admitted\n".into());
        }
        let source = arm.source.clone();
        let result = self.raw_call("Page.addScriptToEvaluateOnNewDocument", json!({"source":source,"runImmediately":false}), Some(session), deadline.saturating_duration_since(Instant::now()))?;
        let identifier = result.get("identifier").and_then(Value::as_str).ok_or_else(|| self.refusal())?.to_string();
        let arm = self.webmcp.arms.get_mut(session).ok_or_else(|| "slicc: refused: WebMCP holder disappeared\n".to_string())?;
        arm.identifier = Some(identifier);
        if epoch != self.frame_epoch || Instant::now() >= deadline || self.blocked.contains(session) { self.webmcp_revoke(session); return Err(self.refusal()); }
        let arm = self.webmcp.arms.get_mut(session).unwrap();
        arm.revoked = false;
        arm.pending_contexts.remove(&id);
        arm.pending_frames.remove(&context.frame);
        Ok(())
    }

    fn webmcp_adopt_child(&mut self, value: &Value, parent: &str) {
        let Some(child) = value.pointer("/params/sessionId").and_then(Value::as_str) else { return };
        let info = &value["params"]["targetInfo"];
        let target = info.get("targetId").and_then(Value::as_str).unwrap_or("");
        let url = info.get("url").and_then(Value::as_str).unwrap_or("");
        let allowed = info.get("type").and_then(Value::as_str) == Some("iframe") && !target.is_empty()
            && (url.is_empty() || url == "about:blank" || url::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https")))
            && !self.guard.url(url) && !self.info_protected(info, &[]) && !self.content_blocked(parent);
        self.session_targets.insert(child.to_string(), target.to_string());
        self.guard_sessions.insert(child.to_string());
        self.webmcp.quarantined.insert(child.to_string());
        self.webmcp.parents.insert(child.to_string(), parent.to_string());
        self.targets.insert(target.to_string(), info.clone());
        let mut stage = "target";
        let installed = (|| -> Result<(), String> {
            if !allowed { return Err(self.refusal()); }
            stage = "guard setup";
            self.touch(target)?;
            stage = "protected target";
            if self.is_protected_target(target) { return Err(self.refusal()); }
            stage = "guard frame tree";
            let guard = self.guards.get(target).cloned().ok_or_else(|| self.refusal())?;
            let seed = self.webmcp_stable(child, Instant::now() + DOCUMENT_CHECK, |cdp, left| cdp.raw_call("Page.getFrameTree", json!({}), Some(&guard), left.min(OPTIONAL_STEP)))?;
            let verified = self.webmcp_tree_verified(&seed, None);
            let future_only = seed.pointer("/frameTree/frame/id").and_then(Value::as_str) == Some(target)
                && seed.pointer("/frameTree/frame/url").and_then(Value::as_str).is_some_and(|url| url.is_empty() || url == "about:blank")
                && seed.pointer("/frameTree/frame/unreachableUrl").and_then(Value::as_str).is_none_or(|url| url.is_empty())
                && self.webmcp_alive(parent);
            if !verified && !future_only { return Err(self.refusal()); }
            stage = "pinned installer";
            self.webmcp_install(child, true, verified.then_some(&seed))?;
            Ok(())
        })().is_ok();
        let _ = self.raw_call("Runtime.runIfWaitingForDebugger", json!({}), Some(child), Duration::from_secs(2));
        if !installed {
            self.notices.push(format!("webmcp-polyfill: child refused at {stage}\n"));
            self.webmcp_revoke(child);
            let _ = self.raw_call("Target.detachFromTarget", json!({"sessionId":child}), None, Duration::from_secs(2));
            self.blocked.insert(child.to_string());
        }
    }

    fn webmcp_gate_event(&mut self, value: &Value) {
        if value.get("method").and_then(Value::as_str) == Some("Debugger.paused") {
            if let Some(session) = value.get("sessionId").and_then(Value::as_str) { self.webmcp.paused.insert(session.to_string()); }
        }
        self.webmcp.pending_gates.push_back(value.clone());
        if self.webmcp.handling_gate { return; }
        self.webmcp.handling_gate = true;
        while let Some(gate) = self.webmcp.pending_gates.pop_front() {
            let owner = gate.get("sessionId").and_then(Value::as_str).unwrap_or("");
            if gate.get("method").and_then(Value::as_str) == Some("Target.attachedToTarget") {
                self.webmcp_adopt_child(&gate, owner);
            } else {
                self.webmcp_handle_pause(&gate, owner);
            }
        }
        self.webmcp.handling_gate = false;
    }

    pub(super) fn webmcp_event(&mut self, value: &Value) -> bool {
        let session = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        if method == "Target.attachedToTarget" && self.webmcp.arms.contains_key(session) {
            self.webmcp_gate_event(value);
            return true;
        }
        if matches!(method, "Page.frameDetached" | "Runtime.executionContextCreated" | "Runtime.executionContextDestroyed" | "Runtime.executionContextsCleared" | "Debugger.scriptParsed") {
            self.webmcp_context_event(value, session, method);
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
                let policy = if policy.len() > 16384 { "tools=()" } else { policy };
                let origin = url::Url::parse(url).map(|url| url.origin().ascii_serialization()).unwrap_or_default();
                if let Some(arm) = self.webmcp.arms.get_mut(session) {
                    arm.documents.insert(format!("{frame}:{loader}"), json!({"frameId":frame,"loaderId":loader,"origin":origin,"policy":policy}));
                }
            }
        }
        if method == "Page.frameNavigated" {
            let invalid = !self.webmcp_frame_verified(&value["params"]["frame"]);
            let frame = &value["params"]["frame"];
            let initial_child = frame.get("parentId").is_some() && frame.get("url").and_then(Value::as_str) == Some("about:blank") && frame.get("unreachableUrl").and_then(Value::as_str).is_none_or(|url| url.is_empty());
            if invalid && !initial_child { self.webmcp_revoke(session); }
            self.webmcp_navigated(session, value);
        }
        if method == "Debugger.paused" {
            self.webmcp_gate_event(value);
            return true;
        }
        false
    }

    fn webmcp_navigated(&mut self, session: &str, value: &Value) {
        let Some(arm) = self.webmcp.arms.get_mut(session) else { return; };
        let frame = &value["params"]["frame"];
        Self::webmcp_loaders(&json!({"frame":frame}), &mut arm.loaders);
        if let (Some(id), Some(loader)) = (frame.get("id").and_then(Value::as_str), frame.get("loaderId").and_then(Value::as_str)) {
            let restored = value.pointer("/params/type").and_then(Value::as_str) == Some("BackForwardCacheRestore") && arm.accepted.iter().any(|(known, generation, _)| known == id && generation == loader);
            if restored { arm.pending_frames.remove(id); }
            else { arm.pending_frames.insert(id.to_string(), (loader.to_string(), Instant::now() + Duration::from_secs(2))); }
        }
    }

    fn webmcp_context_event(&mut self, value: &Value, session: &str, method: &str) {
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
                    let loader = arm.loaders.get(frame).cloned().unwrap_or_default();
                    if !arm.accepted.contains(&(frame.to_string(), loader.clone(), unique.to_string())) {
                        arm.pending_contexts.insert(id, Instant::now() + Duration::from_secs(2));
                    }
                    self.webmcp.contexts.insert((session.to_string(), id), Context {
                        frame: frame.to_string(),
                        origin: context.get("origin").and_then(Value::as_str).unwrap_or("").to_string(),
                        unique: unique.to_string(),
                        loader,
                    });
                }
            }
        }
        if method == "Runtime.executionContextDestroyed" {
            if let Some(id) = value.pointer("/params/executionContextId").and_then(Value::as_i64) {
                self.webmcp.contexts.remove(&(session.to_string(), id));
                arm.pending_contexts.remove(&id);
            }
        }
        if method == "Runtime.executionContextsCleared" {
            arm.pending_contexts.clear();
            self.webmcp.contexts.retain(|(owner, _), _| owner != session);
            self.webmcp.scripts.retain(|(owner, _), _| owner != session);
        }
        if method == "Debugger.scriptParsed" {
            if let (Some(script), Some(context)) = (value.pointer("/params/scriptId").and_then(Value::as_str), value.pointer("/params/executionContextId").and_then(Value::as_i64)) {
                self.webmcp.scripts.insert((session.to_string(), script.to_string()), context);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    include!("webmcp_guard_test.rs");
}
