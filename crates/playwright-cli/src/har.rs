use crate::args::{Invocation, Output};
use crate::browser;
use crate::cdp::{self, Cdp};
use crate::commands::resolve_target;
use crate::har_lease::{inspect, Lease};
use crate::route::Glob;
use crate::session::{self, Session};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const NONE: &str = "har-stop: no HAR recording in progress\n";
const ALREADY: &str = "har-start: a HAR recording is already in progress\n";

pub fn precheck(invocation: &Invocation) -> Option<String> {
    let command = invocation.command.as_deref()?;
    if !matches!(command, "har-start" | "record") { return None }
    if command == "har-start" && invocation.positionals.is_empty() { return Some("har-start requires <file>\n".into()) }
    for (name, choices) in [("content", ["omit", "embed"]), ("mode", ["full", "minimal"])] {
        if invocation.flags.get(name).is_some_and(|value| !choices.contains(&value.as_str())) {
            return Some(format!("har-start: --{name} must be {} or {}\n", choices[0], choices[1]));
        }
    }
    if let Some(pattern) = invocation.flags.get("filter").or_else(|| invocation.flags.get("url-filter")) {
        if pattern.is_empty() || pattern.contains("=>") || pattern.contains("function") || pattern.contains(';') || Glob::parse(pattern).is_err() {
            return Some("HAR filters accept only a URL glob, not arbitrary JavaScript; use --url-filter '**/api/**' (record: --filter)\n".into());
        }
    }
    if invocation.flags.get("include-credentials").is_some_and(|value| !matches!(value.as_str(), "true" | "false")) { return Some("har-start: --include-credentials must be true or false\n".into()) }
    if command == "record" {
        if let Some(url) = invocation.positionals.first() {
            if !ordinary(&cdp_client::protected::Protected::default(), url) {
                return Some("record: URL must be a verified ordinary http(s) URL\n".into());
            }
        }
    }
    None
}

struct Files {
    record: PathBuf,
    stop: PathBuf,
}

fn files(invocation: &Invocation) -> Result<Files, String> {
    let base = session::path_named(invocation.session.as_deref().filter(|name| !name.is_empty()), invocation.cwd.as_deref())
        .ok_or_else(|| "session: user config dir is not absolute\n".to_string())?;
    Ok(Files { record: base.with_extension("har.json"), stop: base.with_extension("har.stop") })
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_millis() as u64).unwrap_or(0)
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn atomic(path: &Path, value: &Value) -> Result<(), String> {
    atomic_with(path, value, session::write_private_file)
}

fn atomic_session(path: &Path, value: &Value) -> Result<(), String> {
    atomic_with(path, value, session::write_private)
}

fn atomic_with(path: &Path, value: &Value, write: fn(&Path, &str) -> Result<(), String>) -> Result<(), String> {
    let staged = path.with_extension(format!("{}.tmp", path.extension().and_then(|ext| ext.to_str()).unwrap_or("har")));
    write(&staged, &value.to_string())?;
    fs::rename(&staged, path).map_err(|err| format!("har: {err}\n"))
}

fn live(value: &Value) -> bool {
    value["state"] == "recording" && value["beat"].as_u64().is_some_and(|beat| now_ms().saturating_sub(beat) <= 5000)
}

fn clear(files: &Files) {
    let _ = fs::remove_file(&files.record);
    let _ = fs::remove_file(&files.stop);
}

pub fn stop(invocation: &Invocation) -> Output {
    let files = match files(invocation) { Ok(files) => files, Err(message) => return Output::err(message) };
    let found = read(&files.record);
    eprintln!("HAR stop record={} state={:?} beat={:?}", files.record.display(), found.as_ref().map(|value| &value["state"]), found.as_ref().map(|value| &value["beat"]));
    let Some(found) = found else { return Output::err(NONE) };
    if invocation.positionals.first().is_some_and(|id| found["recordingId"].as_str() != Some(id)) {
        return Output::err("stop-recording: recordingId does not belong to this session\n");
    }
    if found["state"] == "stopped" {
        clear(&files);
        return Output::ok(format!("{}\n", found));
    }
    if !live(&found) {
        let checked = inspect(&files.record);
        if !checked.alive {
            #[cfg(windows)]
            drop(checked);
            clear(&files);
            let _ = fs::remove_dir_all(files.record.with_extension("lock"));
            return Output::err(NONE);
        }
    }
    let owner = found["recordingId"].as_str().unwrap_or("");
    if let Err(message) = session::write_private(&files.stop, owner) { return Output::err(message) }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        std::thread::sleep(Duration::from_millis(50));
        match read(&files.record) {
            Some(value) if value["recordingId"] == owner && value["state"] == "stopped" => {
                clear(&files);
                return Output::ok(format!("{value}\n"));
            }
            Some(value) if value["recordingId"] == owner && value["state"] == "recording" => {
                let checked = inspect(&files.record);
                if !live(&value) && !checked.alive { clear(&files); return Output::err(NONE) }
            }
            _ => return Output::err(NONE),
        }
        if Instant::now() >= deadline { return Output::err("har-stop: the HAR holder did not answer\n") }
    }
}

pub fn hold(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation, endpoint: &str) -> Output {
    let files = match files(invocation) { Ok(files) => files, Err(message) => return Output::err(message) };
    if read(&files.record).is_some_and(|value| live(&value)) { return Output::err(ALREADY) }
    let alias = invocation.command.as_deref() == Some("record");
    let target = if alias {
        browser::create_target(cdp, state, "about:blank", true)
    } else {
        browser::list_pages(cdp).and_then(|pages| resolve_target(cdp, invocation, state, &pages, false))
    };
    let target = match target { Ok(target) => target, Err(message) => return Output::err(message) };
    let session = match attach(cdp, state, &target) { Ok(session) => session, Err(message) => return Output::err(message) };
    let id = format!("rec-{}-{}", now_ms(), &cdp_client::hash::sha256_hex(format!("{target}:{session}:{:?}", SystemTime::now()).as_bytes())[..12]);
    let path = if alias { PathBuf::from(format!("/recordings/{id}/recording.har")) } else { PathBuf::from(&invocation.positionals[0]) };
    let path = if path.is_absolute() { path } else { invocation.cwd.clone().or_else(|| std::env::current_dir().ok()).unwrap_or_default().join(path) };
    let mut holder = Holder {
        files, id, target: target.clone(), session: Some(session), path, main_frame: String::new(), seen: cdp.events().len(), paused: true,
        entries: Vec::new(), pending: BTreeMap::new(), failed: BTreeMap::new(), held: Vec::new(), denied: BTreeSet::new(),
        embed: invocation.flags.get("content").is_some_and(|value| value == "embed"),
        minimal: invocation.flags.get("mode").is_some_and(|value| value == "minimal"),
        credentials: invocation.flags.get("include-credentials").is_some_and(|value| value == "true"),
        filter: invocation.flags.get("filter").or_else(|| invocation.flags.get("url-filter")).and_then(|pattern| Glob::parse(pattern).ok()),
    };
    holder.paused = holder.verify(cdp).is_none();
    let _lease = match Lease::claim(&holder.files.record, read(&holder.files.record).is_some_and(|value| live(&value))) {
        Ok(lease) => lease,
        Err(message) => {
            cdp.set_decider(holder.session.as_deref().unwrap(), None);
            cdp::detach(cdp, holder.session.as_deref().unwrap());
            if alias {
                let _ = browser::close_target(cdp, &target);
                state.owned.retain(|owned| owned != &target);
                state.created_blanks.remove(&target);
            }
            return Output::err(message);
        }
    };
    let result = holder.run(cdp, state, endpoint, if alias { invocation.positionals.first().map(String::as_str) } else { None });
    if let Some(session) = &holder.session { cdp.set_decider(session, None); cdp::detach(cdp, session); }
    let mut latest = session::load_in(state.session_name.as_deref(), state.cwd.as_deref());
    latest.endpoint = state.endpoint.clone();
    latest.provenance = state.provenance.clone();
    latest.webmcp_connection = state.webmcp_connection.clone();
    latest.current = Some(target.clone());
    if alias {
        session::note_owned(&mut latest, &target);
        if let Some(proof) = state.created_blanks.get(&target) { latest.created_blanks.insert(target, proof.clone()); }
    }
    *state = latest;
    match result { Ok(()) => Output::ok(""), Err(message) => Output::err(message) }
}

fn attach(cdp: &mut Cdp, state: &Session, target: &str) -> Result<String, String> {
    let session = cdp::attach(cdp, target)?;
    if let Some(proof) = state.created_blanks.get(target) { cdp.restore_created_blank(target, proof); }
    for method in ["Page.enable", "Network.enable"] {
        if let Err(message) = cdp.call(method, json!({}), Some(&session)) { cdp::detach(cdp, &session); return Err(message) }
    }
    cdp.set_decider(&session, Some(Box::new(|paused| {
        if paused.status.is_some() { cdp_client::fetch::Decision::ContinueResponse(Default::default()) }
        else { cdp_client::fetch::Decision::default() }
    })));
    if let Err(message) = cdp.call("Fetch.enable", json!({"patterns":[{"urlPattern":"*","requestStage":"Response"}]}), Some(&session)) {
        cdp.set_decider(&session, None);
        cdp::detach(cdp, &session);
        return Err(message);
    }
    Ok(session)
}

struct Pending {
    request: Value,
    response: Option<Value>,
    extra_headers: Option<(u64, Value)>,
    ordinary_document: bool,
    recovering_document: bool,
    document_url: String,
    timestamp: f64,
    wall: f64,
    end: f64,
    loader: String,
}

struct Holder {
    files: Files,
    id: String,
    target: String,
    session: Option<String>,
    path: PathBuf,
    main_frame: String,
    seen: usize,
    paused: bool,
    entries: Vec<Value>,
    pending: BTreeMap<String, Pending>,
    failed: BTreeMap<String, Pending>,
    held: Vec<(String, Pending, bool)>,
    denied: BTreeSet<String>,
    embed: bool,
    minimal: bool,
    credentials: bool,
    filter: Option<Glob>,
}

impl Holder {
    fn status(&self, event: &str) -> Value {
        json!({"event":event,"recordingId":self.id,"tab":self.target,"path":self.path,"entries":self.entries.len()})
    }

    fn checkpoint(&self, cdp: &Cdp) -> Result<(), String> {
        let entries: Vec<_> = self.entries.iter().filter(|entry| ordinary(cdp.guard(), text(&entry["request"], "url")) && ordinary(cdp.guard(), text(&entry["response"], "_url"))).cloned().collect();
        atomic(&self.path, &json!({"log":{"version":"1.2","creator":{"name":"SLICC playwright-cli","version":env!("PLAYWRIGHT_CLI_VERSION")},"entries":entries}}))
    }

    fn beat(&self) -> Result<(), String> {
        atomic_session(&self.files.record, &json!({"state":"recording","recordingId":self.id,"beat":now_ms(),"path":self.path,"tab":self.target}))
    }

    fn run(&mut self, cdp: &mut Cdp, state: &Session, endpoint: &str, url: Option<&str>) -> Result<(), String> {
        self.beat()?;
        if let Err(message) = self.checkpoint(cdp) { clear(&self.files); return Err(message) }
        let mut input = crate::webmcp_polyfill::input()?;
        emit(self.status("ready"))?;
        if self.paused { emit(self.status("paused"))?; }
        let watched = (|| {
            if let Some(url) = url {
                cdp.call("Page.navigate", json!({"url":url}), self.session.as_deref())?;
            }
            self.watch(cdp, state, endpoint, &mut input)
        })();
        let reason = watched.as_deref().unwrap_or("error");
        let saved = self.checkpoint(cdp);
        let mut stopped = self.status("stopped");
        stopped["reason"] = json!(reason);
        stopped["state"] = json!("stopped");
        let stored = if reason == "stdin" { clear(&self.files); Ok(()) } else { atomic_session(&self.files.record, &stopped) };
        let _ = fs::remove_file(&self.files.stop);
        let emitted = emit(stopped);
        watched.and(saved).and(stored).and(emitted).map(|_| ())
    }

    fn verify(&mut self, cdp: &mut Cdp) -> Option<String> {
        let session = self.session.as_deref()?;
        if cdp.content_blocked(session) || cdp.is_protected_target(&self.target) { return None }
        let tree = cdp.call("Page.getFrameTree", json!({}), Some(session)).ok()?;
        let frame = tree.pointer("/frameTree/frame")?;
        self.main_frame = text(frame, "id").to_string();
        let known = ordinary(cdp.guard(), text(frame, "url")) && ordinary(cdp.guard(), text(frame, "securityOrigin"))
            && !text(frame, "id").is_empty() && !text(frame, "loaderId").is_empty() && text(frame, "unreachableUrl").is_empty();
        known.then(|| text(frame, "loaderId").to_string())
    }

    fn watch(&mut self, cdp: &mut Cdp, state: &Session, endpoint: &str, input: &mut crate::webmcp_polyfill::Input) -> Result<String, String> {
        let mut beat = Instant::now();
        let mut retry = Instant::now();
        loop {
            if self.absorb(cdp)? { return Ok("closed".into()) }
            if let Err(mpsc::TryRecvError::Disconnected) = input() { return self.finish_capture(cdp, "stdin") }
            if fs::read_to_string(&self.files.stop).is_ok_and(|id| id == self.id) { return self.finish_capture(cdp, "stop") }
            if beat.elapsed() >= Duration::from_secs(1) { self.beat()?; beat = Instant::now(); }
            if self.session.as_deref().is_some_and(|session| cdp.content_blocked(session) || cdp.is_protected_target(&self.target)) {
                if self.absorb(cdp)? { return Ok("closed".into()) }
                if let Some(session) = self.session.take() { cdp.set_decider(&session, None); cdp::detach(cdp, &session); }
                self.pending.clear();
                self.failed.clear();
                self.held.clear();
                self.checkpoint(cdp)?;
                emit(self.status("checkpoint"))?;
                self.pause()?;
            }
            if Instant::now() >= retry {
                if self.session.is_none() { self.reattach(cdp, state, endpoint)?; }
                if let Some(loader) = self.verify(cdp) {
                    if self.paused { self.paused = false; emit(self.status("resumed"))?; }
                    self.release(cdp, &loader, false)?;
                } else {
                    if self.absorb(cdp)? { return Ok("closed".into()) }
                    self.pending.retain(|_, pending| pending.recovering_document);
                    self.failed.retain(|_, pending| pending.recovering_document);
                    self.checkpoint(cdp)?;
                    self.pause()?;
                }
                retry = Instant::now() + Duration::from_millis(250);
            }
            for notice in cdp.take_notices() { eprint!("{notice}"); }
            cdp.pump(Instant::now() + Duration::from_millis(50))?;
        }
    }

    fn finish_capture(&mut self, cdp: &mut Cdp, reason: &str) -> Result<String, String> {
        drain(cdp);
        while self.seen < cdp.events().len() {
            if self.absorb(cdp)? { return Ok("closed".into()) }
        }
        Ok(reason.into())
    }

    fn reattach(&mut self, cdp: &mut Cdp, state: &Session, endpoint: &str) -> Result<(), String> {
        let previous = Rc::new(RefCell::new(Some(std::mem::replace(cdp, Cdp::connect(endpoint)?))));
        let pumped = Rc::clone(&previous);
        cdp.set_waiter(Some(Box::new(move |_: &Cdp| {
            if let Some(previous) = pumped.borrow_mut().as_mut() { drain(previous); }
        })));
        let attached = browser::list_pages(cdp).map(|pages| {
            if !pages.iter().any(|page| page.target_id == self.target && ordinary(cdp.guard(), &page.url)) { return None }
            attach(cdp, state, &self.target).ok()
        });
        cdp.set_waiter(None);
        let previous = previous.borrow_mut().take();
        match attached {
            Ok(Some(session)) => {
                self.session = Some(session);
                self.seen = cdp.events().len();
                Ok(())
            }
            attached => {
                if let Some(previous) = previous { *cdp = previous; }
                attached.map(|_| ())
            }
        }
    }

    fn pause(&mut self) -> Result<(), String> {
        if !self.paused { self.paused = true; emit(self.status("paused"))?; }
        Ok(())
    }

    fn absorb(&mut self, cdp: &mut Cdp) -> Result<bool, String> {
        let events = cdp.events()[self.seen.min(cdp.events().len())..].to_vec();
        self.seen = cdp.events().len();
        for event in events {
            if event["method"] == "Target.targetDestroyed" && event["params"]["targetId"] == self.target {
                return Ok(true);
            }
            if event["sessionId"].as_str() != self.session.as_deref() { continue }
            let params = &event["params"];
            let id = text(params, "requestId");
            match text(&event, "method") {
                "Page.frameNavigated" if params["frame"].get("parentId").is_none() => {
                    self.checkpoint(cdp)?;
                    emit(self.status("checkpoint"))?;
                    if let Some(loader) = self.verify(cdp) {
                        if self.paused { self.paused = false; emit(self.status("resumed"))?; }
                        self.release(cdp, &loader, true)?;
                    } else { self.pending.clear(); self.failed.clear(); self.held.clear(); self.pause()?; }
                }
                "Network.requestWillBeSent" => {
                    if let Some(response) = params.get("redirectResponse") {
                        if ordinary(cdp.guard(), text(response, "url")) {
                            if let Some(pending) = self.pending.get_mut(id) { pending.response = Some(response.clone()); pending.end = number(params, "timestamp"); }
                            self.finish(cdp, id, false)?;
                        } else { self.pending.remove(id); self.denied.insert(id.to_string()); }
                    }
                    let request = &params["request"];
                    if !ordinary(cdp.guard(), text(request, "url")) { self.pending.remove(id); self.denied.insert(id.to_string()); continue }
                    if self.denied.contains(id) { continue }
                    let recovering_document = self.paused && text(params, "type") == "Document" && !self.main_frame.is_empty() && text(params, "frameId") == self.main_frame;
                    if self.paused && !recovering_document { continue }
                    if !ordinary(cdp.guard(), text(params, "documentURL")) || text(params, "loaderId").is_empty() { continue }
                    if self.filter.as_ref().is_some_and(|filter| !filter.matches(text(request, "url"))) { continue }
                    self.pending.insert(id.to_string(), Pending { request: request.clone(), response: None, extra_headers: None, ordinary_document: !self.paused, recovering_document, document_url: text(params, "documentURL").to_string(), timestamp: number(params, "timestamp"), wall: number(params, "wallTime"), end: number(params, "timestamp"), loader: text(params, "loaderId").to_string() });
                }
                "Network.responseReceived" => {
                    let response = &params["response"];
                    if !ordinary(cdp.guard(), text(response, "url")) { self.pending.remove(id); self.denied.insert(id.to_string()); continue }
                    if let Some(pending) = self.pending.get_mut(id) {
                        let mut response = response.clone();
                        if let Some((status, extra)) = &pending.extra_headers {
                            if response["status"].as_u64() == Some(*status) || response["status"] == 200 && *status == 304 {
                                if let (Some(fields), Some(extra)) = (response["headers"].as_object_mut(), extra.as_object()) { fields.extend(extra.clone()); }
                            }
                        }
                        pending.response = Some(response);
                        pending.end = number(params, "timestamp");
                    }
                }
                "Network.responseReceivedExtraInfo" => {
                    let redirect = params["statusCode"].as_u64().is_some_and(|status| matches!(status, 301 | 302 | 303 | 307 | 308));
                    if redirect {
                        if let Some(mut pending) = self.failed.remove(id) {
                            pending.response = Some(json!({"url":pending.request["url"],"status":params["statusCode"],"headers":params["headers"]}));
                            self.pending.insert(id.to_string(), pending);
                            self.finish(cdp, id, false)?;
                            continue;
                        }
                    }
                    if let Some(pending) = self.pending.get_mut(id) {
                        if let Some(status) = params["statusCode"].as_u64().filter(|status| (100..600).contains(status)) {
                            pending.extra_headers = Some((status, params["headers"].clone()));
                            if let Some(response) = pending.response.as_mut() {
                                if response["status"].as_u64() == Some(status) || response["status"] == 200 && status == 304 {
                                    if let (Some(fields), Some(extra)) = (response["headers"].as_object_mut(), params["headers"].as_object()) { fields.extend(extra.clone()); }
                                }
                            }
                        }
                    }
                }
                "Fetch.requestPaused" if params["responseStatusCode"].as_u64().is_some_and(|status| matches!(status, 301 | 302 | 303 | 307 | 308)) => {
                    let network = text(params, "networkId");
                    if !self.pending.contains_key(network) {
                        if let Some(pending) = self.failed.remove(network) { self.pending.insert(network.to_string(), pending); }
                    }
                    let Some(pending) = self.pending.get_mut(network) else { continue };
                    let mut headers = serde_json::Map::new();
                    for header in params["responseHeaders"].as_array().into_iter().flatten() { headers.insert(text(header, "name").to_string(), header["value"].clone()); }
                    pending.response = Some(json!({"url":pending.request["url"],"status":params["responseStatusCode"],"statusText":params["responseStatusText"],"headers":headers}));
                    self.finish(cdp, network, false)?;
                }
                "Network.loadingFinished" => {
                    if let Some(pending) = self.pending.get_mut(id) { pending.end = number(params, "timestamp"); }
                    self.finish(cdp, id, true)?;
                }
                "Network.loadingFailed" => {
                    if self.pending.get(id).and_then(|pending| pending.response.as_ref()).is_some_and(|response| response["status"].as_u64().is_some_and(|status| matches!(status, 301 | 302 | 303 | 307 | 308))) {
                        self.finish(cdp, id, false)?;
                    } else if let Some(mut pending) = self.pending.remove(id) {
                        pending.end = number(params, "timestamp");
                        self.failed.insert(id.to_string(), pending);
                    }
                }
                _ => {}
            }
        }
        Ok(false)
    }

    fn release(&mut self, cdp: &mut Cdp, loader: &str, committed: bool) -> Result<(), String> {
        let (ready, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.held).into_iter().partition(|(_, pending, _)| pending.loader == loader);
        if !committed { self.held = rest; }
        for (id, pending, body) in ready { self.admit(cdp, &id, pending, body)?; }
        Ok(())
    }

    fn finish(&mut self, cdp: &mut Cdp, id: &str, body: bool) -> Result<(), String> {
        let Some(pending) = self.pending.remove(id) else { return Ok(()) };
        self.admit(cdp, id, pending, body)
    }

    fn admit(&mut self, cdp: &mut Cdp, id: &str, pending: Pending, body: bool) -> Result<(), String> {
        let Some(response) = &pending.response else { return Ok(()) };
        if !ordinary(cdp.guard(), &pending.document_url) || !ordinary(cdp.guard(), text(&pending.request, "url")) || !ordinary(cdp.guard(), text(response, "url")) { return Ok(()) }
        if !body && !response["status"].as_u64().is_some_and(|status| matches!(status, 301 | 302 | 303 | 307 | 308)) { return Ok(()) }
        let verified = self.verify(cdp).is_some_and(|loader| loader == pending.loader);
        if !pending.ordinary_document && !(pending.recovering_document && verified && !self.paused) {
            if pending.recovering_document { self.held.push((id.to_string(), pending, body)); }
            return Ok(())
        }
        let redirect = response["status"].as_u64().is_some_and(|status| matches!(status, 301 | 302 | 303 | 307 | 308));
        if self.paused && !redirect { return Ok(()) }
        let can_embed = self.embed && verified && !self.paused && self.session.as_deref().is_some_and(|session| cdp.ordinary_response_body(session, id));
        let content = if can_embed && body {
            cdp.call_for("Network.getResponseBody", json!({"requestId":id}), self.session.as_deref(), Duration::from_secs(2)).ok()
        } else { None };
        self.entries.push(entry(cdp, &pending, content.as_ref(), self.credentials, can_embed && pending.ordinary_document, self.minimal));
        Ok(())
    }
}

fn drain(cdp: &mut Cdp) {
    for _ in 0..64 {
        if !matches!(cdp.pump(Instant::now() + Duration::from_millis(1)), Ok(true)) { break }
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

fn number(value: &Value, key: &str) -> f64 {
    value[key].as_f64().filter(|value| value.is_finite()).unwrap_or(0.0)
}

fn ordinary(guard: &cdp_client::protected::Protected, value: &str) -> bool {
    !guard.url(value) && url::Url::parse(value).is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host())
}

fn headers(cdp: &Cdp, value: &Value, source: &str, credentials: bool) -> Vec<Value> {
    let mut value = value.clone();
    cdp.redact_redirect_carriers(&mut value, source);
    value.as_object().into_iter().flatten().filter(|(name, _)| credentials || !matches!(name.to_ascii_lowercase().as_str(), "cookie" | "set-cookie" | "authorization" | "proxy-authorization"))
        .flat_map(|(name, value)| value.as_str().into_iter().flat_map(move |value| value.split('\n').map(move |value| json!({"name":name,"value":value.trim_end_matches('\r')})))).collect()
}

fn entry(cdp: &Cdp, pending: &Pending, body: Option<&Value>, credentials: bool, embed: bool, minimal: bool) -> Value {
    let request = &pending.request;
    let response = pending.response.as_ref().unwrap();
    let url = text(request, "url");
    let request_headers = headers(cdp, &request["headers"], url, credentials);
    let response_headers = headers(cdp, &response["headers"], text(response, "url"), credentials);
    let location = response_headers.iter().find(|header| text(header, "name").eq_ignore_ascii_case("location")).map(|header| text(header, "value")).unwrap_or("");
    let duration = ((pending.end - pending.timestamp) * 1000.0).max(0.0);
    let size = number(response, "encodedDataLength").max(0.0);
    let query: Vec<Value> = url::Url::parse(url).ok().into_iter().flat_map(|url| url.query_pairs().map(|(name, value)| json!({"name":name,"value":value})).collect::<Vec<_>>()).collect();
    let mut content = json!({"size":size,"mimeType":response["mimeType"].as_str().unwrap_or("application/octet-stream")});
    if let Some(body) = body { content["text"] = json!(text(body, "body")); if body["base64Encoded"] == true { content["encoding"] = json!("base64"); } }
    let mut timings = json!({"blocked":-1,"dns":-1,"connect":-1,"ssl":-1,"send":0,"wait":if minimal { 0.0 } else { duration },"receive":0});
    if !minimal {
        if let Some(timing) = response.get("timing") {
            let phase = |start: &str, end: &str| match (timing[start].as_f64(), timing[end].as_f64()) {
                (Some(start), Some(end)) if start >= 0.0 && end >= start => end - start,
                _ => -1.0,
            };
            timings = json!({"blocked":0,"dns":phase("dnsStart","dnsEnd"),"connect":phase("connectStart","connectEnd"),"ssl":phase("sslStart","sslEnd"),"send":phase("sendStart","sendEnd").max(0.0),"wait":phase("sendEnd","receiveHeadersStart").max(0.0),"receive":(duration-number(timing,"receiveHeadersEnd")).max(0.0)});
        }
    }
    let mut entry = json!({
        "startedDateTime":date(pending.wall),"time":if minimal { 0.0 } else { duration },
        "request":{"method":request["method"].as_str().unwrap_or("GET"),"url":url,"httpVersion":"HTTP/1.1","cookies":[],"headers":request_headers,"queryString":query,"headersSize":-1,"bodySize":-1},
        "response":{"status":response["status"].as_u64().unwrap_or(0),"statusText":text(response,"statusText"),"httpVersion":response["protocol"].as_str().unwrap_or("HTTP/1.1"),"cookies":[],"headers":response_headers,"content":content,"redirectURL":location,"headersSize":-1,"bodySize":size,"_url":text(response,"url")},
        "cache":{},"timings":timings
    });
    if embed {
        if let Some(post) = request["postData"].as_str() {
            let mime = request_headers.iter().find(|header| text(header,"name").eq_ignore_ascii_case("content-type")).map(|header| text(header,"value")).unwrap_or("application/octet-stream");
            entry["request"]["postData"] = json!({"mimeType":mime,"text":post});
            entry["request"]["bodySize"] = json!(post.len());
        }
    }
    entry
}

fn date(seconds: f64) -> String {
    let millis = (seconds.max(0.0).min(253402300799.0) * 1000.0) as i64;
    let days = millis / 86400000;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z", millis / 3600000 % 24, millis / 60000 % 60, millis / 1000 % 60, millis % 1000)
}

fn emit(event: Value) -> Result<(), String> {
    let mut stdout = io::stdout();
    writeln!(stdout, "{event}").and_then(|_| stdout.flush()).map_err(|err| format!("har-start: {err}\n"))
}
