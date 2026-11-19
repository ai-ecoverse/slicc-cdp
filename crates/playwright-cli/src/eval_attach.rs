use crate::action::Report;
use crate::args::{Invocation, Output};
use crate::browser;
use crate::cdp::{self, Cdp};
use crate::eval_lease::AttachmentLease;
use crate::eval_scope::Scope;
use crate::session::Session;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub(crate) fn precheck(invocation: &Invocation) -> Result<(), String> {
    if invocation.positionals.len() != 1 { return Err("eval-attach requires <file> containing a JavaScript body with send(payload)\n".into()) }
    Scope::parse(invocation)?;
    crate::eval_frames::origins(invocation)?;
    if invocation.flags.get("world").is_some_and(|world| !matches!(world.as_str(), "main" | "isolated")) {
        return Err("eval-attach: --world must be main or isolated\n".into());
    }
    Ok(())
}

fn emit(event: Value) -> Result<(), String> {
    println!("{}", crate::sanitize::json_c1(&event.to_string()));
    io::stdout().flush().map_err(|_| "eval-attach: failed to write event\n".into())
}

pub(crate) fn hold(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let scope = match Scope::parse(invocation) { Ok(scope) => scope, Err(message) => return Output::err(message) };
    let path = invocation.cwd.clone().unwrap_or_default().join(&invocation.positionals[0]);
    let source = match fs::read_to_string(path) { Ok(source) => source, Err(error) => return Output::err(format!("eval-attach: cannot read script: {error}\n")) };
    let lease = match AttachmentLease::claim(invocation) { Ok(lease) => lease, Err(message) => return Output::err(message) };
    let world = invocation.flags.get("world").map(String::as_str).unwrap_or("main");
    let early = invocation.flags.contains_key("early");
    let (same_origin, origins) = crate::eval_frames::origins(invocation).unwrap();
    let cross_origin = (same_origin || !origins.is_empty()) && cdp.eval_attach_cross_origin_available();
    let mut holder = Holder { lease: &lease, scope, source, world, early, same_origin, origins, cross_origin, active: BTreeMap::new(), observed: BTreeSet::new(), tabs: BTreeMap::new(), refused: BTreeMap::new(), key: format!("__slicc_{}", lease.id.replace('-', "_")), invocation, state, id: &lease.id };
    let result = holder.run(cdp, &lease);
    for tab in holder.tabs.values() {
        if early { cdp.eval_attach_early_stop(&tab.session); }
        stop_children(cdp, tab);
        cdp::detach(cdp, &tab.session);
    }
    let reason = result.as_deref().unwrap_or("error");
    let finished = lease.finish(reason).and_then(emit);
    match result.and(finished.map(|_| String::new())) { Ok(_) => Output::ok(""), Err(message) => Output::err(message) }
}

struct Tab {
    target: String,
    children: BTreeMap<String, Tab>,
    session: String,
    seen: usize,
    finish_early: bool,
    contexts: BTreeMap<i64, Value>,
    attempted: BTreeMap<String, Option<Installed>>,
    installed: Option<Installed>,
}

#[derive(Clone)]
struct Installed {
    context: String,
    default: String,
    frame: String,
    loader: String,
    record: Value,
}

struct Holder<'a> {
    lease: &'a AttachmentLease,
    scope: Scope,
    source: String,
    world: &'a str,
    early: bool,
    same_origin: bool,
    origins: Vec<String>,
    cross_origin: bool,
    active: BTreeMap<String, Value>,
    observed: BTreeSet<String>,
    tabs: BTreeMap<String, Tab>,
    refused: BTreeMap<String, String>,
    key: String,
    invocation: &'a Invocation,
    state: &'a Session,
    id: &'a str,
}

impl Holder<'_> {
    fn refuse(&mut self, target: &str, document: &str, reason: &str) -> Result<(), String> {
        if self.refused.get(target).is_some_and(|known| known == document || document == "blocked") { return Ok(()) }
        self.refused.insert(target.to_string(), document.to_string());
        emit(json!({"event":"refused","attachmentId":self.id,"tab":target,"frameId":target,"reason":reason}))
    }

    fn run(&mut self, cdp: &mut Cdp, lease: &AttachmentLease) -> Result<String, String> {
        let mut input = crate::holder_input::input()?;
        emit(json!({"event":"ready","attachmentId":self.id,"world":self.world,"early":self.early}))?;
        let mut discovery = Instant::now() - Duration::from_millis(250);
        loop {
            if let Err(mpsc::TryRecvError::Disconnected) = input() { return Ok("stdin".into()) }
            if lease.stopping() { return Ok("detach".into()) }
            if discovery.elapsed() >= Duration::from_millis(250) {
                self.discover(cdp)?;
                discovery = Instant::now();
            }
            self.observed.clear();
            let targets: Vec<_> = self.tabs.keys().cloned().collect();
            for target in targets {
                let mut tab = self.tabs.remove(&target).unwrap();
                let result = self.tick(cdp, &target, &mut tab);
                if self.early && tab.finish_early { cdp.eval_attach_early_finish(&tab.session); }
                tab.finish_early = false;
                self.tabs.insert(target, tab);
                result?;
            }
            self.active.retain(|key, _| self.observed.contains(key));
            lease.publish(self.active.values().cloned().collect())?;
            cdp.pump(Instant::now() + Duration::from_millis(100))?;
            for notice in cdp.take_notices() { eprint!("{notice}"); }
        }
    }

    fn discover(&mut self, cdp: &mut Cdp) -> Result<(), String> {
        let pages = browser::list_pages(cdp)?;
        let mut live: BTreeSet<_> = pages.iter().map(|page| page.target_id.clone()).collect();
        if let Scope::Tab(target) = &self.scope { live.insert(target.clone()); }
        let removed: Vec<_> = self.tabs.keys().filter(|target| !live.contains(*target) || cdp.is_protected_target(target)).cloned().collect();
        for target in removed {
            let tab = self.tabs.remove(&target).unwrap();
            if cdp.is_protected_target(&target) { self.refuse(&target, "blocked", "guard")?; }
            if self.early { cdp.eval_attach_early_stop(&tab.session); }
            stop_children(cdp, &tab);
            cdp::detach(cdp, &tab.session);
        }
        let candidates: Vec<_> = if let Scope::Tab(target) = &self.scope { vec![target.clone()] }
            else { pages.iter().filter(|page| self.scope.matches(&page.target_id, &page.url)).map(|page| page.target_id.clone()).collect() };
        for target in candidates {
            if self.tabs.contains_key(&target) { continue }
            let info = cdp.call("Target.getTargetInfo", json!({"targetId":target}), None);
            if !info.is_ok_and(|info| info["targetInfo"]["type"] == "page" && info["targetInfo"].get("parentFrameId").is_none()) {
                self.refuse(&target, "blocked", "guard")?;
                continue;
            }
            let seen = cdp.events().len();
            let attached = cdp::attach(cdp, &target).and_then(|session| {
                if self.state.owned.contains(&target) {
                    if let Some(proof) = self.state.created_blanks.get(&target) { cdp.restore_created_blank(&target, proof); }
                }
                if self.early {
                    if let Err(message) = cdp.eval_attach_early_arm(&session) { cdp::detach(cdp, &session); return Err(message) }
                }
                for method in ["Page.enable", "Runtime.enable", "Network.enable"] {
                    if let Err(message) = cdp.call(method, json!({}), Some(&session)) { cdp::detach(cdp, &session); return Err(message) }
                }
                Ok(session)
            });
            match attached {
                Ok(session) => { self.tabs.insert(target.clone(), Tab { target: target.clone(), children: BTreeMap::new(), session, seen, finish_early: false, contexts: BTreeMap::new(), attempted: BTreeMap::new(), installed: None }); }
                Err(_) => self.refuse(&target, "blocked", "guard")?,
            }
        }
        Ok(())
    }

    fn tick(&mut self, cdp: &mut Cdp, target: &str, tab: &mut Tab) -> Result<(), String> {
        absorb(cdp, tab);
        let tree = match cdp.call("Page.getFrameTree", json!({}), Some(&tab.session)) {
            Ok(tree) => tree,
            Err(_) => { tab.installed = None; tab.finish_early = true; return self.refuse(target, "blocked", "guard") }
        };
        absorb(cdp, tab);
        let frame = &tree["frameTree"]["frame"];
        let frame_id = text(frame, "id");
        let loader = text(frame, "loaderId");
        let url = text(frame, "url");
        if cdp.guard().url(url) || cdp.guard().url(text(frame, "unreachableUrl")) {
            tab.installed = None;
            tab.finish_early = true;
            return self.refuse(target, loader, "guard");
        }
        if frame_id.is_empty() || loader.is_empty() || frame.get("parentId").is_some() || !self.scope.matches(target, url) {
            tab.installed = None;
            tab.finish_early = true;
            return Ok(());
        }
        let infos = if self.same_origin || !self.origins.is_empty() {
            cdp.call("Target.getTargets", json!({}), None).ok().and_then(|value| value["targetInfos"].as_array().cloned()).unwrap_or_default()
        } else { Vec::new() };
        prune_children(cdp, tab, &infos);
        self.visit(cdp, target, tab, &tree["frameTree"], &[], &infos)
    }

    fn visit(&mut self, cdp: &mut Cdp, target: &str, tab: &mut Tab, node: &Value, ancestors: &[crate::eval_frames::Document], infos: &[Value]) -> Result<(), String> {
        if ancestors.len() >= 64 { return Ok(()) }
        absorb(cdp, tab);
        let frame = &node["frame"];
        let mut chain = ancestors.to_vec();
        chain.push(crate::eval_frames::Document { target: tab.target.clone(), session: tab.session.clone(), frame: frame.clone() });
        let child = !ancestors.is_empty();
        let origin = text(frame, "securityOrigin");
        let requested = !child || self.same_origin && origin == chain[0].text("securityOrigin") || self.origins.iter().any(|wanted| wanted == origin);
        let reason = if child && crate::eval_frames::verify(cdp, &chain).is_err() { Some("guard") }
            else if !requested { Some("origin-not-requested") }
            else if child && origin != chain[0].text("securityOrigin") && !self.cross_origin { Some(crate::eval_frames::UNAVAILABLE) }
            else { None };
        if let Some(reason) = reason {
            self.refuse_document(target, frame, &chain, text(frame, "loaderId"), reason)?;
            if reason == "guard" { return Ok(()) }
        } else {
            self.document(cdp, target, tab, frame, &chain)?;
        }
        if !child && self.early && tab.finish_early {
            cdp.eval_attach_early_finish(&tab.session);
            tab.finish_early = false;
        }
        if !self.same_origin && self.origins.is_empty() { return Ok(()) }
        let locals = node["childFrames"].as_array().cloned().unwrap_or_default();
        for node in &locals {
            if !infos.iter().any(|info| info["type"] == "iframe" && info["targetId"] == node["frame"]["id"] && info["parentFrameId"] == frame["id"]) {
                self.visit(cdp, target, tab, node, &chain, infos)?;
            }
        }
        let remote: Vec<_> = infos.iter().filter(|info| info["type"] == "iframe" && info["parentFrameId"] == frame["id"]).collect();
        for info in remote {
            let id = text(info, "targetId");
            if id.is_empty() || chain.iter().any(|document| document.text("id") == id) { continue }
            let origin = url::Url::parse(text(info, "url")).ok().map(|url| url.origin().ascii_serialization()).unwrap_or_default();
            let seed = json!({"id":id,"parentId":frame["id"],"securityOrigin":origin});
            if cdp.is_protected_target(id) || !browser::ordinary_frame_url(cdp.guard(), text(info, "url")) {
                self.refuse_document(target, &seed, &chain, "blocked", "guard")?;
                continue;
            }
            let requested = self.same_origin && origin == chain[0].text("securityOrigin") || self.origins.contains(&origin);
            if origin != chain[0].text("securityOrigin") && !self.cross_origin {
                self.refuse_document(target, &seed, &chain, "blocked", if requested { crate::eval_frames::UNAVAILABLE } else { "origin-not-requested" })?;
                continue;
            }
            if tab.children.len() >= 256 && !tab.children.contains_key(id) { self.refuse_document(target, &seed, &chain, "blocked", "guard")?; continue; }
            let opened = if let Some(child) = tab.children.remove(id) { Ok(child) } else {
                let seen = cdp.events().len();
                cdp::attach(cdp, id).and_then(|session| {
                    let enabled = ["Page.enable", "Runtime.enable", "Network.enable"].iter().try_for_each(|method| cdp.call(method, json!({}), Some(&session)).map(|_| ()));
                    if let Err(message) = enabled { cdp::detach(cdp, &session); return Err(message) }
                    Ok(Tab { target: id.to_string(), children: BTreeMap::new(), session, seen, finish_early: false, contexts: BTreeMap::new(), attempted: BTreeMap::new(), installed: None })
                })
            };
            match opened {
                Ok(mut child) => {
                    if let Ok(tree) = cdp.call("Page.getFrameTree", json!({}), Some(&child.session)) {
                        if tree["frameTree"]["frame"]["id"] == id && tree["frameTree"]["frame"]["parentId"] == frame["id"] {
                            self.visit(cdp, target, &mut child, &tree["frameTree"], &chain, infos)?;
                        } else { self.refuse_document(target, &seed, &chain, "blocked", "guard")?; }
                    } else { self.refuse_document(target, &seed, &chain, "blocked", "guard")?; }
                    tab.children.insert(id.to_string(), child);
                }
                Err(_) => self.refuse_document(target, &seed, &chain, "blocked", "guard")?,
            }
        }
        Ok(())
    }

    fn refuse_document(&mut self, target: &str, frame: &Value, chain: &[crate::eval_frames::Document], document: &str, reason: &str) -> Result<(), String> {
        let frame_id = text(frame, "id");
        let key = if chain.len() == 1 && frame.get("parentId").is_none() { target.to_string() } else { format!("{target}:{frame_id}") };
        if self.refused.get(&key).is_some_and(|known| known == document) { return Ok(()) }
        self.refused.insert(key, document.to_string());
        let mut event = json!({"event":"refused","attachmentId":self.id,"tab":target,"frameId":frame_id,"reason":reason});
        if chain.len() > 1 || frame.get("parentId").is_some() {
            event["parentFrameId"] = frame["parentId"].clone();
            event["origin"] = if reason == "guard" { Value::Null } else { frame["securityOrigin"].clone() };
        }
        emit(event)
    }

    fn event_fields(&self, cdp: &mut Cdp, mut event: Value, chain: &[crate::eval_frames::Document]) -> Result<Value, String> {
        if chain.len() > 1 {
            let evidence: Vec<_> = chain.iter().enumerate().map(|(index, document)| document.evidence(cdp, index.checked_sub(1).map(|index| &chain[index]))).collect();
            let child = evidence.last().unwrap();
            for key in ["origin", "permissionsPolicy", "toolsAllowed", "allow", "srcOrigin"] { event[key] = child[key].clone(); }
            event["parentFrameId"] = json!(chain[chain.len() - 2].text("id"));
            event["ancestors"] = json!(&evidence[..evidence.len() - 1]);
        }
        if chain.len() > 1 { crate::eval_frames::verify(cdp, chain)?; }
        Ok(event)
    }

    fn document(&mut self, cdp: &mut Cdp, target: &str, tab: &mut Tab, frame: &Value, chain: &[crate::eval_frames::Document]) -> Result<(), String> {
        let frame_id = text(frame, "id");
        let loader = text(frame, "loaderId");
        let url = text(frame, "url");
        let early = self.early && chain.len() == 1;
        let default = tab.contexts.values().find(|context| context["auxData"]["isDefault"] == true && context["auxData"]["frameId"] == frame_id).cloned();
        let Some(default) = default else { return Ok(()) };
        let unique = text(&default, "uniqueId");
        if unique.is_empty() { return self.refuse_document(target, frame, chain, loader, "unverified-context") }
        if tab.installed.as_ref().is_some_and(|installed| installed.default != unique || installed.loader != loader) { tab.installed = None; }
        if !tab.attempted.contains_key(unique) {
            if early {
                if cdp.eval_attach_early_waiting(&tab.session, unique) { return Ok(()) }
                tab.finish_early = cdp.eval_attach_early_paused(&tab.session, unique);
                if !cdp.eval_attach_early_paused(&tab.session, unique) {
                    tab.attempted.insert(unique.to_string(), None);
                    return self.refuse_document(target, frame, chain, unique, "late");
                }
            }
            tab.attempted.insert(unique.to_string(), None);
            let context = if self.world == "isolated" {
                let created = cdp.call("Page.createIsolatedWorld", json!({"frameId":frame_id,"worldName":self.key}), Some(&tab.session));
                absorb(cdp, tab);
                created.ok().and_then(|created| tab.contexts.get(&created["executionContextId"].as_i64()?).cloned())
            } else { Some(default.clone()) };
            let Some(context) = context else { return self.refuse_document(target, frame, chain, unique, "unverified-context") };
            let context = text(&context, "uniqueId");
            if context.is_empty() { return self.refuse_document(target, frame, chain, unique, "unverified-context") }
            if self.world == "isolated" {
                let verified = cdp.call("Page.getFrameTree", json!({}), Some(&tab.session));
                if !verified.is_ok_and(|tree| find_frame(&tree["frameTree"], frame_id).is_some_and(|current| ["id", "loaderId", "url"].iter().all(|key| current[*key] == frame[*key]))) || chain.len() > 1 && crate::eval_frames::verify(cdp, chain).is_err() {
                    return self.refuse_document(target, frame, chain, unique, "unverified-context");
                }
            }
            if early && !cdp.eval_attach_early_paused(&tab.session, unique) { return self.refuse_document(target, frame, chain, unique, "late") }
            if chain.len() > 1 && crate::eval_frames::verify(cdp, chain).is_err() { return self.refuse_document(target, frame, chain, unique, "guard") }
            let mut report = Report::for_command("eval").with_intent("eval", self.invocation.intent.as_deref());
            report.tab(target);
            report.begin(cdp);
            let installed = if early {
                cdp.eval_attach_early_evaluate(&tab.session, unique, json!({"expression":self.install_source(url),"uniqueContextId":context,"returnByValue":true})).and_then(evaluated)
            } else { evaluate(cdp, &tab.session, context, &self.install_source(url)) };
            let outcome = match &installed { Ok(Value::Bool(true)) => Output::ok(""), _ => Output::err("eval-attach: installation refused or script threw\n") };
            report.finish(cdp, &outcome);
            if !matches!(installed, Ok(Value::Bool(true))) {
                let reason = if installed.as_ref().err().is_some_and(|message| message == "eval-attach: late\n") { "late" } else { "installation" };
                return self.refuse_document(target, frame, chain, unique, reason);
            }
            if chain.len() > 1 && crate::eval_frames::verify(cdp, chain).is_err() { return self.refuse_document(target, frame, chain, unique, "guard") }
            let record = json!({"tab":target,"frameId":frame_id,"loaderId":loader,"context":context,"chain":chain.iter().map(|document| json!({"frame":document.frame,"target":document.target})).collect::<Vec<_>>()});
            tab.installed = Some(Installed { record, context: context.to_string(), default: unique.to_string(), frame: frame_id.to_string(), loader: loader.to_string() });
            tab.attempted.insert(unique.to_string(), tab.installed.clone());
            let refusal_key = if chain.len() == 1 { target.to_string() } else { format!("{target}:{frame_id}") };
            self.refused.remove(&refusal_key);
            self.observed.insert(format!("{target}:{frame_id}"));
            self.active.insert(format!("{target}:{frame_id}"), tab.installed.as_ref().unwrap().record.clone());
            self.lease.publish(self.active.values().cloned().collect())?;
            let event = json!({"event":"document-ready","attachmentId":self.id,"tab":target,"frameId":frame_id,"loaderId":loader,"url":url,"world":self.world,"early":early});
            match self.event_fields(cdp, event, chain) {
                Ok(event) => emit(event)?,
                Err(_) => return self.refuse_document(target, frame, chain, unique, "guard"),
            }
        }
        tab.installed = tab.attempted.get(unique).cloned().flatten().filter(|installed| installed.loader == loader);
        let Some(installed) = &tab.installed else { return Ok(()) };
        let source = format!("globalThis[{}]({})", json!(self.key), json!(url.split('#').next().unwrap_or(url)));
        if chain.len() > 1 && crate::eval_frames::verify(cdp, chain).is_err() { return self.refuse_document(target, frame, chain, unique, "guard") }
        let drained = evaluate(cdp, &tab.session, &installed.context, &source);
        let values = match drained {
            Ok(value) => value,
            Err(_) => { return self.refuse_document(target, frame, chain, unique, "relay") }
        };
        if chain.len() > 1 && crate::eval_frames::verify(cdp, chain).is_err() { return self.refuse_document(target, frame, chain, unique, "guard") }
        self.observed.insert(format!("{target}:{frame_id}"));
        self.active.insert(format!("{target}:{frame_id}"), installed.record.clone());
        if let Some(messages) = values.as_array() {
            for message in messages.iter().take(256).filter_map(Value::as_str).filter(|message| message.len() <= 65536) {
                if let Ok(payload) = serde_json::from_str::<Value>(message) {
                    let event = json!({"event":"message","attachmentId":self.id,"tab":target,"frameId":installed.frame,"loaderId":installed.loader,"payload":payload});
                    match self.event_fields(cdp, event, chain) {
                Ok(event) => emit(event)?,
                Err(_) => return self.refuse_document(target, frame, chain, unique, "guard"),
            }
                }
            }
        }
        Ok(())
    }

    fn install_source(&self, url: &str) -> String {
        format!("(() => {{ const queue = []; if (location.href.split('#')[0] !== {}) return false; const stringify = JSON.stringify; const push = Array.prototype.push.call.bind(Array.prototype.push); const splice = Array.prototype.splice.call.bind(Array.prototype.splice); globalThis[{}] = expected => location.href.split('#')[0] === expected ? splice(queue, 0, 256) : []; const send = payload => {{ const value = stringify(payload); if (typeof value === 'string' && value.length <= 65536 && queue.length < 256) push(queue, value); }}; (send => {{\n{}\n}})(send); return true; }})()", json!(url.split('#').next().unwrap_or(url)), json!(self.key), self.source)
    }
}

fn evaluate(cdp: &mut Cdp, session: &str, context: &str, source: &str) -> Result<Value, String> {
    let value = cdp.call("Runtime.evaluate", json!({"expression":source,"uniqueContextId":context,"returnByValue":true}), Some(session))?;
    evaluated(value)
}

fn evaluated(value: Value) -> Result<Value, String> {
    if value.get("exceptionDetails").is_some() { return Err("eval-attach: script threw\n".into()) }
    Ok(value["result"]["value"].clone())
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

fn absorb(cdp: &Cdp, tab: &mut Tab) {
    for event in &cdp.events()[tab.seen..] {
        if event["sessionId"] != tab.session { continue }
        match text(event, "method") {
            "Runtime.executionContextCreated" => {
                let context = &event["params"]["context"];
                if let Some(id) = context["id"].as_i64() { tab.contexts.insert(id, context.clone()); }
            }
            "Runtime.executionContextsCleared" => { tab.contexts.clear(); tab.installed = None; }
            "Runtime.executionContextDestroyed" => {
                if let Some(id) = event["params"]["executionContextId"].as_i64() { tab.contexts.remove(&id); }
            }
            _ => {}
        }
    }
    let live: BTreeSet<_> = tab.contexts.values().filter_map(|context| context["uniqueId"].as_str()).collect();
    tab.attempted.retain(|unique, _| live.contains(unique.as_str()));
    tab.seen = cdp.events().len();
}

fn stop_children(cdp: &mut Cdp, tab: &Tab) {
    for child in tab.children.values() {
        stop_children(cdp, child);
        cdp::detach(cdp, &child.session);
    }
}

fn prune_children(cdp: &mut Cdp, tab: &mut Tab, infos: &[Value]) {
    let removed: Vec<_> = tab.children.keys().filter(|id| !infos.iter().any(|info| info["targetId"] == id.as_str()) || cdp.is_protected_target(id)).cloned().collect();
    for id in removed {
        if let Some(child) = tab.children.remove(&id) {
            stop_children(cdp, &child);
            cdp::detach(cdp, &child.session);
        }
    }
    for child in tab.children.values_mut() { prune_children(cdp, child, infos); }
}

fn find_frame<'a>(node: &'a Value, id: &str) -> Option<&'a Value> {
    if node["frame"]["id"] == id { return node.get("frame"); }
    node["childFrames"].as_array()?.iter().find_map(|node| find_frame(node, id))
}
