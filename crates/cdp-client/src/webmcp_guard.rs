use super::{transport_error, Cdp, DOCUMENT_CHECK, OPTIONAL_STEP};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

const SHIM: &str = include_str!("webmcp-shim.js");
const SHIM_HASH: &str = "911b9c665b7bf8183de1d4eeebb4a96d72ddcfe7b6b222316998c595b410fe53";

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
    root_identity: Option<(String, Context)>,
    pending_children: BTreeMap<i64, (Context, (String, Context), Instant)>,
    child_done: BTreeSet<(String, String, String)>,
    admitted: usize,
    revoked: bool,
    revoked_at: Option<(&'static str, u32)>,
    revoked_reason: Option<&'static str>,
    pending_contexts: BTreeMap<i64, Instant>,
    pending_install: Option<Instant>,
    pending_frames: BTreeMap<String, (String, Instant)>,
    accepted: BTreeSet<(String, String, String)>,
    installing: BTreeSet<(String, String, String)>,
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
    holders: BTreeMap<String, String>,
    contexts: BTreeMap<(String, i64), Context>,
    scripts: BTreeMap<(String, String), i64>,
    evaluations: BTreeSet<(String, String)>,
    paused: BTreeSet<String>,
    handling_gate: bool,
    handling_children: bool,
    pending_gates: VecDeque<Value>,
    pub(super) deferred: BTreeSet<String>,
    pub(super) quarantined: BTreeSet<String>,
    parents: BTreeMap<String, String>,
    children: Vec<(String, String, String)>,
    samples: Vec<u128>,
    trace_sequence: u64,
    trace_lifecycle: BTreeMap<(String, &'static str), u64>,
    trace_requests: BTreeMap<(String, String), (String, u64)>,
    trace_fetch: BTreeMap<String, Value>,
    trace_roles: BTreeMap<String, &'static str>,
}

impl Webmcp {
    pub(super) fn forget(&mut self, session: &str) {
        self.arms.remove(session);
        self.holders.retain(|_, owner| owner != session);
        self.contexts.retain(|(owner, _), _| owner != session);
        self.scripts.retain(|(owner, _), _| owner != session);
        self.evaluations.retain(|(owner, _)| owner != session);
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
        if !self.webmcp_alive(session) { return Err(self.refusal()); }
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
            json!({"session":session,"bridge":arm.bridge,"target":self.session_target(session),"documents":arm.documents,"plain":arm.plain})
        }).collect()
    }

    pub fn webmcp_alive(&self, session: &str) -> bool {
        let session = self.webmcp_session(session);
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
        let pending = self.webmcp.arms.iter().filter(|(_, arm)| arm.plain && arm.admitted > 0 && !arm.revoked && (!arm.pending_contexts.is_empty() || !arm.pending_frames.is_empty())).map(|(session, _)| session.clone()).collect::<Vec<_>>();
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
            let reason = self.webmcp.arms.get(&session).map(|arm| if arm.pending_install.is_some_and(|deadline| deadline <= now) { "installer deadline" } else if arm.pending_contexts.values().any(|deadline| *deadline <= now) { "context deadline" } else { "frame deadline" }).unwrap_or("missing holder");
            self.webmcp_revoke_because(&session, reason);
            if !self.webmcp_plain(&session) { let _ = self.send("Debugger.resume", json!({}), Some(&session)); }
        }
        self.webmcp_install_children();
    }

    pub fn stop_webmcp(&mut self, session: &str) {
        let owner = self.webmcp_session(session).to_string();
        let session = owner.as_str();
        let attached = self.webmcp.holders.values().any(|holder| holder == session);
        let debug = self.webmcp.arms.get(session).is_some_and(|arm| !arm.plain);
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
        self.webmcp.arms.insert(session.to_string(), Arm { source, bridge: bridge.clone(), identifier: None, conversion: None, plain: false, root_identity: None, pending_children: BTreeMap::new(), child_done: BTreeSet::new(), admitted: 0, revoked: false, revoked_at: None, revoked_reason: None, pending_contexts: BTreeMap::new(), pending_install: None, pending_frames: BTreeMap::new(), accepted: BTreeSet::new(), installing: BTreeSet::new(), loaders, documents: BTreeMap::new() });
        self.call_for("Runtime.disable", json!({}), Some(session), DOCUMENT_CHECK)?;
        self.call_for("Runtime.enable", json!({}), Some(session), DOCUMENT_CHECK)?;
        self.call_for("Page.enable", json!({}), Some(session), DOCUMENT_CHECK)?;
        self.call_for("Network.enable", json!({}), Some(session), DOCUMENT_CHECK)?;
        let enabled = self.call_guarded_for("Debugger.enable", json!({}), Some(session), DOCUMENT_CHECK, true);
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
            match tree.get("frameTree").and_then(|node| tree_complete(self, node, false)) {
                Some(true) if self.webmcp_tree_verified(&tree, None) => return Ok(()),
                Some(false) => { let _ = self.pump(until.min(Instant::now() + Duration::from_millis(25)))?; }
                _ => return Err(self.refusal()),
            }
        }
    }

    fn webmcp_arm(&mut self, session: &str) -> Result<(), String> {
        if self.webmcp_plain(session) { return Ok(()); }
        let result = self.call_guarded_for("Debugger.setInstrumentationBreakpoint", json!({"instrumentation":"beforeScriptExecution"}), Some(session), DOCUMENT_CHECK, true)?;
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

    fn webmcp_evaluate(&mut self, session: &str, id: i64, context: &Context, root: (String, Context), until: Instant) -> Result<(), String> {
        let arm = self.webmcp.arms.get(session).filter(|arm| !arm.revoked).ok_or_else(|| self.refusal())?;
        if arm.plain && !self.worlds.fallback_safe(session) { return Err(self.refusal()); }
        if arm.accepted.contains(&(context.frame.clone(), context.loader.clone(), context.unique.clone())) { return Ok(()); }
        let source = arm.source.clone();
        if self.webmcp_root_document(session, until)? != root || context.unique.is_empty() || self.webmcp.contexts.get(&(session.to_string(), id)) != Some(context) { return Err(self.refusal()); }
        self.webmcp_check_context(session, id, context, until)?;
        if self.webmcp_plain(session) && !self.worlds.fallback_safe(session) { return Err(self.refusal()); }
        if self.webmcp.arms.get(session).is_some_and(|arm| arm.accepted.contains(&(context.frame.clone(), context.loader.clone(), context.unique.clone()))) { return Ok(()); }
        let realm = (context.frame.clone(), context.loader.clone(), context.unique.clone());
        self.webmcp.arms.get_mut(session).ok_or_else(|| "slicc: refused: WebMCP holder disappeared\n".to_string())?.installing.insert(realm.clone());
        let result = self.call_for("Runtime.evaluate", json!({"expression":source,"uniqueContextId":context.unique,"returnByValue":true,"silent":true}), Some(session), until.saturating_duration_since(Instant::now()));
        if let Some(arm) = self.webmcp.arms.get_mut(session) { arm.installing.remove(&realm); }
        let result = result?;
        if result.get("exceptionDetails").is_some() || self.webmcp_plain(session) && !self.worlds.fallback_safe(session) || self.webmcp_root_document(session, until)? != root { return Err(self.refusal()); }
        self.webmcp_check_context(session, id, context, until)
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
        if Instant::now() >= until || !self.webmcp_alive(session) || !self.webmcp_tree_verified(&tree, Some(&context.frame))
            || frame.get("loaderId").and_then(Value::as_str) != Some(&context.loader)
            || frame.get("securityOrigin").and_then(Value::as_str) != Some(&context.origin)
            || root.origin != context.origin || context.unique.is_empty() || self.webmcp.contexts.get(&(session.to_string(), id)) != Some(context)
            || !self.webmcp_plain(session) && self.worlds.reason(session, "Debugger.enable", &json!({})).is_some() { return Err(self.refusal()); }
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
        let accepted = self.webmcp_pause_context(value, session).is_some_and(|(_, context)| self.webmcp.arms.get(session).is_some_and(|arm| arm.accepted.contains(&(context.frame, context.loader, context.unique))));
        if evaluate || accepted {
            let _ = self.raw_call("Debugger.resume", json!({}), Some(session), DOCUMENT_CHECK);
            self.webmcp.paused.remove(session);
            return true;
        }
        let root_proof = self.webmcp_known_root(session);
        let converted = root_proof.and_then(|root| {
            let context = self.webmcp_pause_context(value, session).map(|(_, context)| context).unwrap_or_else(|| root.1.clone());
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
            if let Some(arm) = self.webmcp.arms.get_mut(session) { arm.conversion = None; arm.pending_install = None; }
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

    fn webmcp_queue_children(&mut self, session: &str) {
        let Some(target) = self.session_target(session) else { return; };
        let Some(frame) = self.documents.get(&target).map(|document| document.frame_id.clone()) else { return; };
        let Some(known) = self.webmcp.arms.get(session).filter(|arm| !arm.revoked && arm.conversion.is_none() && arm.pending_install.is_none()) else { return; };
        let Some(context) = self.webmcp.contexts.iter().find(|((owner, _), context)| owner == session && context.frame == frame
            && known.loaders.get(&frame) == Some(&context.loader) && !context.unique.is_empty()).map(|(_, context)| context.clone()) else { return; };
        let root = (target, context);
        let Some(arm) = self.webmcp.arms.get_mut(session) else { return; };
        if arm.plain && !self.worlds.fallback_safe(session) { return; }
        arm.root_identity = Some(root.clone());
        arm.pending_children.retain(|id, (context, known_root, _)| known_root == &root && self.webmcp.contexts.get(&(session.to_string(), *id)) == Some(context));
        for ((owner, id), context) in &self.webmcp.contexts {
            let realm = (context.frame.clone(), context.loader.clone(), context.unique.clone());
            if owner != session || context.frame == root.1.frame || context.origin != root.1.origin || context.loader.is_empty() || context.unique.is_empty()
                || arm.accepted.contains(&realm) || arm.child_done.contains(&realm) || arm.child_done.len() >= 1024 { continue; }
            if arm.pending_children.len() >= 1024 { break; }
            arm.pending_children.entry(*id).or_insert_with(|| (context.clone(), root.clone(), Instant::now() + DOCUMENT_CHECK));
        }
    }

    fn webmcp_install_children(&mut self) {
        if self.webmcp.handling_gate || self.webmcp.handling_children { return; }
        self.webmcp.handling_children = true;
        let sessions = self.webmcp.arms.keys().cloned().collect::<Vec<_>>();
        for session in sessions {
            self.webmcp_queue_children(&session);
            let pending = self.webmcp.arms.get(&session).filter(|arm| !arm.revoked && arm.conversion.is_none() && arm.pending_install.is_none()).map(|arm| arm.pending_children.clone()).unwrap_or_default();
            for (id, (context, root, until)) in pending {
                let realm = (context.frame.clone(), context.loader.clone(), context.unique.clone());
                if Instant::now() >= until {
                    if let Some(arm) = self.webmcp.arms.get_mut(&session) { arm.pending_children.remove(&id); arm.child_done.insert(realm); }
                    self.notices.push("webmcp-polyfill: a same-origin child placement did not settle before its installation deadline\n".into());
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
                    self.notices.push("webmcp-polyfill: a same-origin child could not verify its document; synchronous registration before installation is unavailable\n".into());
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
        let conversion = self.webmcp.arms.get_mut(session).and_then(|arm| arm.conversion.take());
        let started = conversion.as_ref().map_or_else(Instant::now, |(_, started, _)| *started);
        let until = conversion.as_ref().map_or(started + DOCUMENT_CHECK, |(_, _, until)| (*until).min(started + Duration::from_secs(1)));
        let verified = loop {
            let pending = conversion.as_ref().is_some_and(|(root, _, _)| self.webmcp_pending_child_pause(session, value, root, until));
            if pending {
                self.webmcp_trace_stage(session, "root-pause-held", "pending child placement");
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
        if let Some(arm) = self.webmcp.arms.get_mut(session) { arm.pending_install = None; }
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

    fn webmcp_adopt_child(&mut self, value: &Value) {
        let Some(child) = value.pointer("/params/sessionId").and_then(Value::as_str) else { return; };
        self.blocked.insert(child.to_string());
        let _ = self.send("Runtime.runIfWaitingForDebugger", json!({}), Some(child));
        let _ = self.send("Target.detachFromTarget", json!({"sessionId":child}), None);
    }

    pub(super) fn webmcp_cancel_queued(&mut self, session: &str) {
        let cancelled = self.webmcp.pending_gates.iter().filter(|value| value.get("sessionId").and_then(Value::as_str) == Some(session) || value.pointer("/params/sessionId").and_then(Value::as_str) == Some(session)).cloned().collect::<Vec<_>>();
        self.webmcp.pending_gates.retain(|value| value.get("sessionId").and_then(Value::as_str) != Some(session) && value.pointer("/params/sessionId").and_then(Value::as_str) != Some(session));
        for value in cancelled {
            let owner = value.get("sessionId").and_then(Value::as_str).unwrap_or("");
            if value.get("method").and_then(Value::as_str) == Some("Debugger.paused") {
                let _ = self.send("Debugger.resume", json!({}), Some(owner));
            } else if let Some(child) = value.pointer("/params/sessionId").and_then(Value::as_str) {
                self.webmcp_revoke(child);
                let _ = self.send("Runtime.runIfWaitingForDebugger", json!({}), Some(child));
                let _ = self.send("Debugger.resume", json!({}), Some(child));
                let _ = self.send("Target.detachFromTarget", json!({"sessionId":child}), None);
            }
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
                self.webmcp_adopt_child(&gate);
            } else {
                self.webmcp_handle_pause(&gate, owner);
            }
        }
        self.webmcp.handling_gate = false;
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
            if !visited.insert(root) { return "unknown"; }
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
            let frame = &value["params"]["frame"];
            let initial_child = (frame.get("parentId").is_some() || self.webmcp.parents.contains_key(session))
                && frame.get("url").and_then(Value::as_str).is_some_and(|url| url.is_empty() || url == "about:blank")
                && frame.get("unreachableUrl").and_then(Value::as_str).is_none_or(|url| url.is_empty());
            if let Some(reason) = self.webmcp_frame_refusal(frame).filter(|_| !initial_child) {
                self.webmcp_trace_navigation(session, frame);
                let role = if self.webmcp.parents.contains_key(session) { "OOPIF" } else if frame.get("parentId").is_some() { "subframe" } else { "top frame" };
                self.notices.push(format!("webmcp-polyfill: frame navigation refused ({role}, {reason})\n"));
                self.webmcp_revoke_because(session, "frame navigation refused");
            }
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
        if frame.get("parentId").is_some() { return; }
        if let (Some(id), Some(loader)) = (frame.get("id").and_then(Value::as_str), frame.get("loaderId").and_then(Value::as_str)) {
            let restored = value.pointer("/params/type").and_then(Value::as_str) == Some("BackForwardCacheRestore") && arm.accepted.iter().any(|(known, generation, _)| known == id && generation == loader);
            if restored { arm.pending_frames.remove(id); }
            else { arm.pending_frames.insert(id.to_string(), (loader.to_string(), Instant::now() + Duration::from_secs(2))); }
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
                    let loader = arm.loaders.get(frame).cloned().unwrap_or_default();
                    if root.as_deref() == Some(frame) && !arm.accepted.contains(&(frame.to_string(), loader.clone(), unique.to_string())) {
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
            self.webmcp.evaluations.retain(|(owner, _)| owner != session);
        }
        if method == "Debugger.scriptParsed" {
            if let (Some(script), Some(context)) = (value.pointer("/params/scriptId").and_then(Value::as_str), value.pointer("/params/executionContextId").and_then(Value::as_i64)) {
                self.webmcp.scripts.insert((session.to_string(), script.to_string()), context);
                self.webmcp.evaluations.remove(&(session.to_string(), script.to_string()));
                if value.pointer("/params/url").and_then(Value::as_str) == Some("") && value.pointer("/params/embedderName").and_then(Value::as_str) == Some("") {
                    self.webmcp.evaluations.insert((session.to_string(), script.to_string()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    include!("webmcp_guard_test.rs");
    include!("webmcp_plain_retry_test.rs");
}
