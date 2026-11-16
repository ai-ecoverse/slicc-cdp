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
    let mut holder = Holder { scope, source, world, tabs: BTreeMap::new(), refused: BTreeMap::new(), key: format!("__slicc_{}", lease.id.replace('-', "_")), invocation, state, id: &lease.id };
    let result = holder.run(cdp, &lease);
    for tab in holder.tabs.values() { cdp::detach(cdp, &tab.session); }
    let reason = result.as_deref().unwrap_or("error");
    let finished = lease.finish(reason).and_then(emit);
    match result.and(finished.map(|_| String::new())) { Ok(_) => Output::ok(""), Err(message) => Output::err(message) }
}

struct Tab {
    session: String,
    seen: usize,
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
}

struct Holder<'a> {
    scope: Scope,
    source: String,
    world: &'a str,
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
        emit(json!({"event":"refused","attachmentId":self.id,"tab":target,"reason":reason}))
    }

    fn run(&mut self, cdp: &mut Cdp, lease: &AttachmentLease) -> Result<String, String> {
        let mut input = crate::webmcp_polyfill::input()?;
        emit(json!({"event":"ready","attachmentId":self.id,"world":self.world,"early":false}))?;
        let mut discovery = Instant::now() - Duration::from_millis(250);
        loop {
            if let Err(mpsc::TryRecvError::Disconnected) = input() { return Ok("stdin".into()) }
            if lease.stopping() { return Ok("detach".into()) }
            if discovery.elapsed() >= Duration::from_millis(250) {
                self.discover(cdp)?;
                discovery = Instant::now();
            }
            let targets: Vec<_> = self.tabs.keys().cloned().collect();
            for target in targets {
                let mut tab = self.tabs.remove(&target).unwrap();
                self.tick(cdp, &target, &mut tab)?;
                self.tabs.insert(target, tab);
            }
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
                for method in ["Page.enable", "Runtime.enable", "Network.enable"] {
                    if let Err(message) = cdp.call(method, json!({}), Some(&session)) { cdp::detach(cdp, &session); return Err(message) }
                }
                Ok(session)
            });
            match attached {
                Ok(session) => { self.tabs.insert(target, Tab { session, seen, contexts: BTreeMap::new(), attempted: BTreeMap::new(), installed: None }); }
                Err(_) => self.refuse(&target, "blocked", "guard")?,
            }
        }
        Ok(())
    }

    fn tick(&mut self, cdp: &mut Cdp, target: &str, tab: &mut Tab) -> Result<(), String> {
        absorb(cdp, tab);
        let tree = match cdp.call("Page.getFrameTree", json!({}), Some(&tab.session)) {
            Ok(tree) => tree,
            Err(_) => { tab.installed = None; return self.refuse(target, "blocked", "guard") }
        };
        let frame = &tree["frameTree"]["frame"];
        let frame_id = text(frame, "id");
        let loader = text(frame, "loaderId");
        let url = text(frame, "url");
        if frame_id.is_empty() || loader.is_empty() || frame.get("parentId").is_some() || !self.scope.matches(target, url) {
            tab.installed = None;
            return Ok(());
        }
        let default = tab.contexts.values().find(|context| context["auxData"]["isDefault"] == true && context["auxData"]["frameId"] == frame_id).cloned();
        let Some(default) = default else { return Ok(()) };
        let unique = text(&default, "uniqueId");
        if unique.is_empty() { return self.refuse(target, loader, "unverified-context") }
        if tab.installed.as_ref().is_some_and(|installed| installed.default != unique || installed.loader != loader) { tab.installed = None; }
        if !tab.attempted.contains_key(unique) {
            tab.attempted.insert(unique.to_string(), None);
            let context = if self.world == "isolated" {
                let created = cdp.call("Page.createIsolatedWorld", json!({"frameId":frame_id,"worldName":self.key}), Some(&tab.session));
                absorb(cdp, tab);
                created.ok().and_then(|created| tab.contexts.get(&created["executionContextId"].as_i64()?).cloned())
            } else { Some(default.clone()) };
            let Some(context) = context else { return self.refuse(target, unique, "unverified-context") };
            let context = text(&context, "uniqueId");
            if context.is_empty() { return self.refuse(target, unique, "unverified-context") }
            if self.world == "isolated" {
                let verified = cdp.call("Page.getFrameTree", json!({}), Some(&tab.session));
                if !verified.is_ok_and(|tree| tree["frameTree"]["frame"]["id"] == frame_id && tree["frameTree"]["frame"]["loaderId"] == loader && tree["frameTree"]["frame"]["url"] == url) {
                    return self.refuse(target, unique, "unverified-context");
                }
            }
            let mut report = Report::for_command("eval").with_intent("eval", self.invocation.intent.as_deref());
            report.tab(target);
            report.begin(cdp);
            let installed = evaluate(cdp, &tab.session, context, &self.install_source(url));
            let outcome = match &installed { Ok(Value::Bool(true)) => Output::ok(""), _ => Output::err("eval-attach: installation refused or script threw\n") };
            report.finish(cdp, &outcome);
            if !matches!(installed, Ok(Value::Bool(true))) { return self.refuse(target, unique, "installation") }
            tab.installed = Some(Installed { context: context.to_string(), default: unique.to_string(), frame: frame_id.to_string(), loader: loader.to_string() });
            tab.attempted.insert(unique.to_string(), tab.installed.clone());
            self.refused.remove(target);
            emit(json!({"event":"document-ready","attachmentId":self.id,"tab":target,"frameId":frame_id,"loaderId":loader,"url":url,"world":self.world,"early":false}))?;
        }
        tab.installed = tab.attempted.get(unique).cloned().flatten().filter(|installed| installed.loader == loader);
        let Some(installed) = &tab.installed else { return Ok(()) };
        let source = format!("globalThis[{}]({})", json!(self.key), json!(url.split('#').next().unwrap_or(url)));
        let drained = evaluate(cdp, &tab.session, &installed.context, &source);
        let values = match drained {
            Ok(value) => value,
            Err(_) => { tab.installed = None; tab.attempted.insert(unique.to_string(), None); return self.refuse(target, unique, "relay") }
        };
        if let Some(messages) = values.as_array() {
            for message in messages.iter().take(256).filter_map(Value::as_str).filter(|message| message.len() <= 65536) {
                if let Ok(payload) = serde_json::from_str::<Value>(message) {
                    emit(json!({"event":"message","attachmentId":self.id,"tab":target,"frameId":installed.frame,"loaderId":installed.loader,"payload":payload}))?;
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
    tab.seen = cdp.events().len();
}
