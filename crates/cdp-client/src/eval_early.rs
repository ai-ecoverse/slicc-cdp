use super::{Cdp, DOCUMENT_CHECK};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Early {
    pub(super) arms: BTreeMap<String, Arm>,
}

#[derive(Default)]
pub(super) struct Arm {
    armed: bool,
    identifier: String,
    contexts: BTreeMap<i64, Value>,
    scripts: BTreeMap<String, i64>,
    first_scripts: BTreeMap<String, String>,
    seen: BTreeSet<String>,
    pending: BTreeMap<String, Instant>,
    conversion: Option<(String, Instant)>,
    conversion_location: Option<Value>,
    paused: Option<(String, Instant)>,
    evaluating: Option<String>,
}

impl Cdp {
    pub fn eval_attach_early_arm(&mut self, session: &str) -> Result<(), String> {
        if self.eval_early.arms.contains_key(session) { return Err(self.refusal()); }
        self.eval_early.arms.insert(session.to_string(), Arm::default());
        let result = (|| {
            self.call("Runtime.enable", json!({}), Some(session))?;
            self.call_guarded_for("Debugger.enable", json!({}), Some(session), DOCUMENT_CHECK, true, false)?;
            let result = self.call_guarded_for("Debugger.setInstrumentationBreakpoint", json!({"instrumentation":"beforeScriptExecution"}), Some(session), DOCUMENT_CHECK, true, false)?;
            let identifier = result["breakpointId"].as_str().filter(|id| !id.is_empty()).ok_or_else(|| self.refusal())?.to_string();
            let arm = self.eval_early.arms.get_mut(session).ok_or_else(|| "eval-attach: early holder disappeared\n".to_string())?;
            arm.identifier = identifier;
            arm.seen.extend(arm.contexts.values().filter_map(|context| context["uniqueId"].as_str().map(str::to_string)));
            arm.armed = true;
            Ok(())
        })();
        if result.is_err() { self.eval_attach_early_stop(session); }
        result
    }

    pub fn eval_attach_early_evaluate(&mut self, session: &str, default: &str, params: Value) -> Result<Value, String> {
        if !self.eval_attach_early_paused(session, default) { return Err("eval-attach: late\n".into()); }
        self.eval_early.arms.get_mut(session).unwrap().evaluating = Some(default.to_string());
        let result = self.call("Runtime.evaluate", params, Some(session));
        let held = self.eval_early.arms.get(session).and_then(|arm| arm.paused.as_ref()).is_some_and(|(context, _)| context == default);
        if let Some(arm) = self.eval_early.arms.get_mut(session) { arm.evaluating = None; }
        if !held { return Err("eval-attach: late\n".into()); }
        result
    }

    pub(super) fn eval_attach_early_admit_execution(&self, method: &str, session: Option<&str>) -> Result<(), String> {
        if method == "Runtime.evaluate" {
            if let Some(session) = session {
                if let Some(default) = self.eval_early.arms.get(session).and_then(|arm| arm.evaluating.as_deref()) {
                    if !self.eval_attach_early_paused(session, default) { return Err("eval-attach: late\n".into()); }
                }
            }
        }
        Ok(())
    }

    pub fn eval_attach_early_paused(&self, session: &str, unique: &str) -> bool {
        self.eval_early.arms.get(session).and_then(|arm| arm.paused.as_ref()).is_some_and(|(context, until)| context == unique && Instant::now() < *until)
    }

    pub fn eval_attach_early_waiting(&self, session: &str, unique: &str) -> bool {
        self.eval_early.arms.get(session).is_some_and(|arm| !arm.seen.contains(unique) && arm.pending.get(unique).is_some_and(|until| !arm.first_scripts.contains_key(unique) || Instant::now() < *until) || arm.conversion.as_ref().is_some_and(|(context, until)| context == unique && Instant::now() < *until))
    }

    pub fn eval_attach_early_finish(&mut self, session: &str) {
        if self.eval_early.arms.get(session).is_some_and(|arm| arm.paused.is_some()) { self.eval_attach_early_resume(session); }
    }

    pub(super) fn eval_attach_early_sweep(&mut self) {
        let expired: Vec<_> = self.eval_early.arms.iter().filter(|(_, arm)| arm.paused.as_ref().or(arm.conversion.as_ref()).is_some_and(|(_, until)| Instant::now() >= *until)).map(|(session, _)| session.clone()).collect();
        for session in expired { self.eval_attach_early_resume(&session); }
    }

    fn eval_attach_early_resume(&mut self, session: &str) {
        if let Some(arm) = self.eval_early.arms.get_mut(session) {
            let paused = arm.paused.take().is_some();
            let converting = arm.conversion.take().is_some();
            arm.conversion_location = None;
            if paused || converting {
                let _ = self.raw_call("Debugger.resume", json!({}), Some(session), DOCUMENT_CHECK);
            }
        }
    }

    pub fn eval_attach_early_stop(&mut self, session: &str) {
        self.eval_attach_early_resume(session);
        if let Some(arm) = self.eval_early.arms.remove(session) {
            if !arm.identifier.is_empty() {
                let _ = self.raw_call("Debugger.removeBreakpoint", json!({"breakpointId":arm.identifier}), Some(session), DOCUMENT_CHECK);
            }
            let _ = self.raw_call("Debugger.disable", json!({}), Some(session), DOCUMENT_CHECK);
        }
    }

    pub(super) fn eval_attach_early_event(&mut self, value: &Value) -> bool {
        let session = value["sessionId"].as_str().unwrap_or("");
        let root = self.session_target(session).and_then(|target| self.documents.get(&target)).map(|document| document.frame_id.clone());
        let Some(arm) = self.eval_early.arms.get_mut(session) else { return false };
        match value["method"].as_str().unwrap_or("") {
            "Runtime.executionContextCreated" => {
                let context = &value["params"]["context"];
                if let Some(id) = context["id"].as_i64() {
                    if arm.contexts.len() < 4096 {
                        arm.contexts.insert(id, context.clone());
                        if arm.armed && arm.pending.len() < 4096 {
                            if let Some(unique) = context["uniqueId"].as_str() { arm.pending.insert(unique.to_string(), Instant::now() + Duration::from_secs(1)); }
                        }
                    }
                }
            }
            "Runtime.executionContextsCleared" => {
                arm.contexts.clear();
                arm.scripts.clear();
                arm.first_scripts.clear();
                arm.seen.clear();
                arm.pending.clear();
                arm.paused = None;
                arm.conversion = None;
                arm.conversion_location = None;
            }
            "Runtime.executionContextDestroyed" => {
                if let Some(id) = value["params"]["executionContextId"].as_i64() {
                    if let Some(context) = arm.contexts.remove(&id) {
                        if let Some(unique) = context["uniqueId"].as_str() {
                            arm.pending.remove(unique);
                            arm.first_scripts.remove(unique);
                        }
                    }
                    arm.scripts.retain(|_, context| *context != id);
                }
            }
            "Debugger.scriptParsed" => {
                if let (Some(script), Some(id)) = (value["params"]["scriptId"].as_str(), value["params"]["executionContextId"].as_i64()) {
                    if arm.scripts.len() < 4096 {
                        arm.scripts.insert(script.to_string(), id);
                        if let Some(unique) = arm.contexts.get(&id).and_then(|context| context["uniqueId"].as_str()) {
                            if arm.first_scripts.len() < 4096 && !arm.first_scripts.contains_key(unique) {
                                arm.first_scripts.insert(unique.to_string(), script.to_string());
                                if let Some(until) = arm.pending.get_mut(unique) { *until = Instant::now() + Duration::from_secs(1); }
                            }
                        }
                    }
                }
            }
            "Debugger.resumed" => { arm.paused = None; }
            "Debugger.paused" => {
                let script = value.pointer("/params/data/scriptId").or_else(|| value.pointer("/params/callFrames/0/location/scriptId")).and_then(Value::as_str);
                let context = script.and_then(|script| arm.scripts.get(script)).and_then(|id| arm.contexts.get(id));
                let unique = context.filter(|context| context["auxData"]["isDefault"] == true && context["auxData"]["frameId"].as_str() == root.as_deref()).and_then(|context| context["uniqueId"].as_str()).filter(|unique| !unique.is_empty()).map(str::to_string);
                if value["params"]["reason"] == "instrumentation" {
                    if let Some(unique) = unique.filter(|unique| arm.armed && arm.seen.len() < 4096 && arm.seen.insert(unique.clone()) && arm.pending.get(unique).is_some_and(|until| Instant::now() < *until) && arm.first_scripts.get(unique).map(String::as_str) == script) {
                        let until = Instant::now() + Duration::from_secs(1);
                        arm.conversion = Some((unique, until));
                        arm.conversion_location = value.pointer("/params/callFrames/0/location").cloned();
                        if self.raw_call("Debugger.pause", json!({}), Some(session), DOCUMENT_CHECK).is_err() {
                            if let Some(arm) = self.eval_early.arms.get_mut(session) { arm.conversion = None; }
                        }
                    }
                } else if let Some((expected, until)) = arm.conversion.take() {
                    let location = arm.conversion_location.take();
                    if unique.as_deref() == Some(&expected) && Instant::now() < until && location.is_some() && location.as_ref() == value.pointer("/params/callFrames/0/location") {
                        arm.paused = Some((expected, until));
                        return true;
                    }
                }
                if let Some(arm) = self.eval_early.arms.get_mut(session) { arm.paused = None; }
                let _ = self.raw_call("Debugger.resume", json!({}), Some(session), DOCUMENT_CHECK);
                return true;
            }
            _ => {}
        }
        false
    }
}
