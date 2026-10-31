use crate::cdp::{self, Cdp};
use crate::session::Session;
use crate::webmcp_polyfill::bridge_name;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(250);
const QUIET: Duration = Duration::from_millis(250);
const LONGEST: Duration = Duration::from_secs(1);

type Key = (String, String, String, String);

#[derive(Default)]
struct Generation {
    seen: Option<String>,
    first: Option<Instant>,
    due: Option<Instant>,
    emitted: Option<Value>,
    world: Option<i64>,
}

pub(crate) struct Watch {
    tab: String,
    session: String,
    frame: String,
    armed: Option<String>,
    generations: BTreeMap<Key, Generation>,
    next: Instant,
    sequence: u64,
    changes: serde_json::Map<String, Value>,
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn key(generation: &Value) -> Key {
    (text(generation, "session").into(), text(generation, "frameId").into(), text(generation, "loaderId").into(), text(generation, "uniqueId").into())
}

fn initial(seen: &str) -> bool {
    matches!(seen, "" | "b0" | "n[]")
}

fn skipped(message: String) -> Result<Option<String>, String> {
    if cdp::transport_error(&message) { Err(message) } else { Ok(None) }
}

fn evaluate(cdp: &mut Cdp, session: &str, params: Value) -> Result<Option<String>, String> {
    match cdp.call("Runtime.evaluate", params, Some(session)) {
        Ok(result) if result.get("exceptionDetails").is_none() => Ok(result.pointer("/result/value").and_then(Value::as_str).map(str::to_string)),
        Ok(_) => Ok(None),
        Err(message) => skipped(message),
    }
}

fn probe(cdp: &mut Cdp, generation: &Value, state: &mut Generation) -> Result<Option<String>, String> {
    let session = text(generation, "session");
    let bridge = text(generation, "bridge");
    if !bridge_name(bridge) { return Ok(None); }
    let expression = format!("(() => {{ 'use strict'; try {{ if (typeof {bridge} === 'undefined' || !{bridge}) return ''; const listed = {bridge}.list(); return listed.error ? 'b!' : 'b' + listed.changes; }} catch {{ return 'b?'; }} }})()");
    let Some(main) = evaluate(cdp, session, json!({"expression":expression,"uniqueContextId":text(generation, "uniqueId"),"returnByValue":true}))? else { return Ok(None); };
    if !main.is_empty() { return Ok(Some(main)); }
    if state.world.is_none() {
        state.world = match cdp.call("Page.createIsolatedWorld", json!({"frameId":text(generation, "frameId"),"worldName":"slicc-webmcp"}), Some(session)) {
            Ok(world) => world.get("executionContextId").and_then(Value::as_i64),
            Err(message) => return skipped(message),
        };
    }
    let Some(world) = state.world else { return Ok(None); };
    let expression = "(async () => { 'use strict'; try { if (!('modelContext' in Document.prototype) || !document.modelContext) return ''; const listed = await document.modelContext.getTools(); const tools = []; for (const tool of listed) if (!tool.window || tool.window === window) tools.push([tool.name, tool.title, tool.description, tool.inputSchema, tool.annotations, tool.origin]); return 'n' + JSON.stringify(tools); } catch { return 'n?'; } })()";
    let native = evaluate(cdp, session, json!({"expression":expression,"contextId":world,"awaitPromise":true,"returnByValue":true}))?;
    if native.is_none() { state.world = None; }
    Ok(native)
}

impl Watch {
    pub(crate) fn new(tab: &str, session: &str, frame: &str) -> Self {
        Watch { tab: tab.into(), session: session.into(), frame: frame.into(), armed: None, generations: BTreeMap::new(), next: Instant::now(), sequence: 0, changes: serde_json::Map::new() }
    }

    pub(crate) fn empty(&self) -> bool {
        self.generations.values().all(|generation| generation.emitted.as_ref().and_then(Value::as_array).is_none_or(Vec::is_empty))
    }

    pub(crate) fn changes(&self) -> Value {
        Value::Object(self.changes.clone())
    }

    fn current(&self, cdp: &Cdp) -> (Vec<Value>, Option<String>) {
        let generations = cdp.webmcp_generations();
        let top = generations.iter().find(|generation| text(generation, "session") == self.session && text(generation, "frameId") == self.frame).map(|generation| text(generation, "loaderId").to_string());
        (generations, top)
    }

    fn lists(&self, cdp: &mut Cdp, state: &Session, generations: &[&Value]) -> Result<Option<Value>, String> {
        let from = generations.iter().map(|generation| text(generation, "origin").to_string()).collect::<BTreeSet<_>>().into_iter().collect::<Vec<_>>();
        match crate::webmcp_frames::frames(cdp, state, &self.tab, &self.session, &from) {
            Ok(frames) => Ok(Some(frames)),
            Err(message) if cdp::transport_error(&message) => Err(message),
            Err(message) => {
                if message != "webmcp: document changed\n" && !message.starts_with("slicc: refused:") { eprint!("webmcp-polyfill: tools-changed skipped: {}", crate::sanitize::browser_message(&message)); }
                Ok(None)
            }
        }
    }

    pub(crate) fn arm(&mut self, cdp: &mut Cdp, state: &Session) -> Result<(), String> {
        self.armed = None;
        self.generations.clear();
        self.changes.clear();
        let (generations, top) = self.current(cdp);
        let Some(top) = top else { return Ok(()); };
        for generation in &generations {
            let mut tracked = Generation::default();
            tracked.seen = Some(probe(cdp, generation, &mut tracked)?.unwrap_or_default());
            self.generations.insert(key(generation), tracked);
        }
        let frames = self.lists(cdp, state, &generations.iter().collect::<Vec<_>>())?;
        for generation in &generations {
            let listed = frames.as_ref().and_then(|frames| frames.get(text(generation, "frameId"))).filter(|frame| frame["loaderId"] == generation["loaderId"]);
            if let Some(tracked) = self.generations.get_mut(&key(generation)) { tracked.emitted = listed.map(|frame| frame["tools"].clone()); }
        }
        self.armed = Some(top);
        self.next = Instant::now() + POLL;
        Ok(())
    }

    fn moved(&mut self, top: Option<String>) -> bool {
        if top == self.armed { return false; }
        self.armed = None;
        self.generations.clear();
        self.changes.clear();
        true
    }

    pub(crate) fn tick(&mut self, cdp: &mut Cdp, state: &Session) -> Result<Vec<Value>, String> {
        let now = Instant::now();
        if now < self.next || self.armed.is_none() { return Ok(Vec::new()); }
        self.next = now + POLL;
        let (generations, top) = self.current(cdp);
        if self.moved(top) { return Ok(Vec::new()); }
        let keys = generations.iter().map(key).collect::<BTreeSet<_>>();
        self.generations.retain(|known, _| keys.contains(known));
        for generation in &generations {
            let tracked = self.generations.entry(key(generation)).or_insert_with(|| Generation { emitted: Some(json!([])), ..Generation::default() });
            let Some(seen) = probe(cdp, generation, tracked)? else { continue };
            let changed = tracked.seen.as_ref().map_or(!initial(&seen), |previous| *previous != seen);
            tracked.seen = Some(seen);
            if changed {
                let first = *tracked.first.get_or_insert(now);
                tracked.due = Some((now + QUIET).min(first + LONGEST));
            }
        }
        let now = Instant::now();
        let due = generations.iter().filter(|generation| self.generations.get(&key(generation)).and_then(|tracked| tracked.due).is_some_and(|due| due <= now)).collect::<Vec<_>>();
        if due.is_empty() { return Ok(Vec::new()); }
        let Some(frames) = self.lists(cdp, state, &generations.iter().collect::<Vec<_>>())? else { return Ok(Vec::new()); };
        for generation in &due {
            if let Some(tracked) = self.generations.get_mut(&key(generation)) { tracked.first = None; tracked.due = None; }
        }
        let (after, top) = self.current(cdp);
        if self.moved(top) { return Ok(Vec::new()); }
        let live = after.iter().map(key).collect::<BTreeSet<_>>();
        let mut events = Vec::new();
        for generation in due {
            let frame = text(generation, "frameId");
            let Some(listed) = frames.get(frame).filter(|listed| listed["loaderId"] == generation["loaderId"]) else { continue };
            let Some(tracked) = self.generations.get_mut(&key(generation)).filter(|_| live.contains(&key(generation))) else { continue };
            let mut tools = listed["tools"].clone();
            if tracked.emitted.as_ref() == Some(&tools) { continue; }
            tracked.emitted = Some(tools.clone());
            crate::webmcp::redact_output(&mut tools);
            self.sequence += 1;
            let event = json!({"event":"tools-changed","tab":self.tab,"target":generation["target"],"frameId":frame,"loaderId":generation["loaderId"],"origin":generation["origin"],"tools":tools});
            self.changes.insert(frame.to_string(), json!({"sequence":self.sequence,"topLoaderId":self.armed,"event":event}));
            events.push(event);
        }
        Ok(events)
    }
}
