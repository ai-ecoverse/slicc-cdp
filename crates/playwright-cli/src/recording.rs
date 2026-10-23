use crate::args::{Invocation, Output};
use crate::browser;
use crate::cdp::Cdp;
use crate::commands::resolve_target;
use crate::sanitize;
use crate::session::{self, Session};
use crate::snapshot;
use serde_json::{json, Map, Value};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RECORDER: &str = include_str!("recorder.js");
const ARIA_HEAD: &str = "globalThis.sliccAriaSnapshot = (() => {";
const ARIA_TAIL: &str = "return sliccAriaSnapshot;\n})();";
const DRAIN: &str = "globalThis.__sliccRecording ? globalThis.__sliccRecording.next(500) : null";
const ALREADY: &str = "Error: Recording is already in progress.\n";
const NONE: &str = "Error: No recording in progress, use browser_start_recording to start one.\n";
const SILENT: &str = "Error: The recording holder did not answer.\n";
const CHANGED: &str = "recording-start: target changed or was refused by the guard\n";
const STALE_MS: u64 = 5000;
const BEAT: Duration = Duration::from_secs(1);
const STOP_WAIT: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_millis(250);
const SIGNAL: Duration = Duration::from_secs(5);
const MAX_ACTIONS: usize = 10000;
const MAX_TEXT: usize = 65536;

pub fn is_start(command: &str) -> bool {
    matches!(command, "recording-start" | "record")
}

pub fn is_stop(command: &str) -> bool {
    matches!(command, "recording-stop" | "stop-recording")
}

struct Files {
    record: PathBuf,
    stop: PathBuf,
}

fn files(name: Option<&str>, cwd: Option<&Path>) -> Result<Files, String> {
    let base = session::path_named(name.filter(|name| !name.is_empty()), cwd)
        .ok_or_else(|| "session: user config dir is not absolute\n".to_string())?;
    let stem = base.with_extension("");
    Ok(Files { record: stem.with_extension("recording.json"), stop: stem.with_extension("recording.stop") })
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn store(path: &Path, value: &Value) -> Result<(), String> {
    let staged = path.with_extension("json.tmp");
    session::write_private(&staged, &value.to_string())?;
    fs::rename(&staged, path).map_err(|err| format!("session: {err}\n"))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_millis() as u64).unwrap_or(0)
}

fn live(value: &Value) -> bool {
    value["state"] == "recording" && value["beat"].as_u64().is_some_and(|beat| now_ms().saturating_sub(beat) <= STALE_MS)
}

fn clear(files: &Files) {
    let _ = fs::remove_file(&files.record);
    let _ = fs::remove_file(&files.stop);
}

pub fn stop(invocation: &Invocation) -> Output {
    let files = match files(invocation.session.as_deref(), invocation.cwd.as_deref()) {
        Ok(files) => files,
        Err(message) => return Output::err(message),
    };
    let Some(found) = read(&files.record) else {
        let _ = fs::remove_file(&files.stop);
        return Output::err(NONE);
    };
    if found["state"] == "stopped" {
        clear(&files);
        return Output::ok(result(&found));
    }
    if !live(&found) {
        clear(&files);
        return Output::err(NONE);
    }
    let owner = found["owner"].as_str().unwrap_or("").to_string();
    if let Err(message) = session::write_private(&files.stop, &owner) {
        return Output::err(message);
    }
    let deadline = Instant::now() + STOP_WAIT;
    loop {
        std::thread::sleep(Duration::from_millis(50));
        match read(&files.record) {
            Some(value) if value["owner"] == owner.as_str() && value["state"] == "stopped" => {
                clear(&files);
                return Output::ok(result(&value));
            }
            Some(value) if !live(&value) => {
                clear(&files);
                return Output::err(NONE);
            }
            Some(_) => {}
            None => {
                let _ = fs::remove_file(&files.stop);
                return Output::err(NONE);
            }
        }
        if Instant::now() >= deadline {
            let _ = fs::remove_file(&files.stop);
            return Output::err(SILENT);
        }
    }
}

fn result(value: &Value) -> String {
    let actions: Vec<&str> = value["actions"].as_array().map(|items| items.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let text = if actions.is_empty() {
        "### Result\nRecording stopped. No actions were recorded.\n".to_string()
    } else {
        format!("### Result\nRecording stopped. Recorded actions:\n\n```js\n{}\n```\n", actions.join("\n"))
    };
    sanitize::terminal(&text)
}

pub fn hold(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    hold_with(cdp, state, invocation, crate::webmcp_polyfill::input)
}

fn hold_with(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation, input: impl FnOnce() -> Result<crate::webmcp_polyfill::Input, String>) -> Output {
    let files = match files(state.session_name.as_deref(), state.cwd.as_deref()) {
        Ok(files) => files,
        Err(message) => return Output::err(message),
    };
    if read(&files.record).is_some_and(|value| live(&value)) {
        return Output::err(ALREADY);
    }
    let target = match browser::list_pages(cdp).and_then(|pages| resolve_target(cdp, invocation, state, &pages, false)) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let owner = cdp_client::hash::sha256_hex(format!("{}:{target}:{session}", SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_nanos()).unwrap_or(0)).as_bytes())[..32].to_string();
    let mut holder = Holder { target: target.clone(), session: session.clone(), owner, files, actions: Vec::new(), last: None, seen: cdp.events().len() };
    let held = holder.run(cdp, input);
    let mut latest = session::load_in(state.session_name.as_deref(), state.cwd.as_deref());
    let _ = browser::watch_detach(cdp, &mut latest, &target, &session);
    latest.endpoint = state.endpoint.clone();
    latest.provenance = state.provenance.clone();
    latest.webmcp_connection = state.webmcp_connection.clone();
    latest.current = Some(target);
    *state = latest;
    match held {
        Ok(()) => Output::ok(""),
        Err(message) => Output::err(message),
    }
}

struct Recorded {
    name: String,
    locator: String,
    count: i64,
    code: String,
    at: Instant,
}

struct Holder {
    target: String,
    session: String,
    owner: String,
    files: Files,
    actions: Vec<Recorded>,
    last: Option<(String, String)>,
    seen: usize,
}

enum End {
    Input,
    Stop,
    Closed,
    Refused,
}

impl Holder {
    fn run(&mut self, cdp: &mut Cdp, input: impl FnOnce() -> Result<crate::webmcp_polyfill::Input, String>) -> Result<(), String> {
        let (first, loader) = self.install(cdp)?;
        let _ = browser::activate(cdp, &self.target);
        let mut input = input()?;
        self.beat()?;
        emit(json!({"event":"ready","tab":self.target,"loaderId":loader}))?;
        let mut context = Some(first);
        let end = self.watch(cdp, &mut input, &mut context);
        if matches!(end, Ok(End::Input | End::Stop)) {
            if let Some(id) = context.filter(|_| !cdp.content_blocked(&self.session)) {
                let _ = self.drain(cdp, id);
            }
        }
        let codes: Vec<String> = self.actions.iter().map(|action| action.code.clone()).collect();
        let reason = match &end {
            Ok(End::Input) => "stdin",
            Ok(End::Stop) => "stop",
            Ok(End::Closed) => "closed",
            Ok(End::Refused) => "refused",
            Err(_) => "error",
        };
        let stored = if matches!(end, Ok(End::Input)) {
            clear(&self.files);
            Ok(())
        } else {
            let written = store(&self.files.record, &json!({"owner":self.owner,"tab":self.target,"state":"stopped","reason":reason,"actions":codes}));
            let _ = fs::remove_file(&self.files.stop);
            written
        };
        let emitted = emit(json!({"event":"stopped","tab":self.target,"reason":reason,"actions":codes}));
        match end {
            Ok(End::Refused) => Err(CHANGED.to_string()),
            Ok(_) => stored.and(emitted),
            Err(message) => Err(message),
        }
    }

    fn watch(&mut self, cdp: &mut Cdp, input: &mut crate::webmcp_polyfill::Input, context: &mut Option<i64>) -> Result<End, String> {
        let mut paused = false;
        let mut beat = Instant::now();
        let mut retry = Instant::now();
        loop {
            if let Err(mpsc::TryRecvError::Disconnected) = input() {
                return Ok(End::Input);
            }
            if fs::read_to_string(&self.files.stop).is_ok_and(|owner| owner == self.owner) {
                return Ok(End::Stop);
            }
            if cdp.session_blocked(&self.session) || cdp.is_protected_target(&self.target) {
                return Ok(End::Refused);
            }
            if self.destroyed(cdp) {
                return Ok(End::Closed);
            }
            cdp.take_notices().iter().for_each(|notice| eprint!("{notice}"));
            if beat.elapsed() >= BEAT {
                self.beat()?;
                beat = Instant::now();
            }
            let blocked = cdp.content_blocked(&self.session);
            if let (Some(id), false) = (*context, blocked) {
                if !matches!(self.drain(cdp, id), Ok(true)) {
                    *context = None;
                }
                continue;
            }
            *context = None;
            if !blocked && Instant::now() >= retry {
                if let Ok((id, loader)) = self.install(cdp) {
                    *context = Some(id);
                    if paused {
                        paused = false;
                        emit(json!({"event":"resumed","tab":self.target,"loaderId":loader}))?;
                    }
                    continue;
                }
                retry = Instant::now() + RETRY;
            }
            if !paused {
                paused = true;
                emit(json!({"event":"paused","tab":self.target}))?;
            }
            cdp.pump(Instant::now() + Duration::from_millis(50))?;
        }
    }

    fn beat(&self) -> Result<(), String> {
        store(&self.files.record, &json!({"owner":self.owner,"tab":self.target,"state":"recording","beat":now_ms()}))
    }

    fn destroyed(&mut self, cdp: &Cdp) -> bool {
        let events = cdp.events();
        let from = self.seen.min(events.len());
        self.seen = events.len();
        events[from..].iter().any(|event| {
            matches!(event["method"].as_str(), Some("Target.targetDestroyed" | "Target.detachedFromTarget"))
                && (event.pointer("/params/targetId").and_then(Value::as_str) == Some(self.target.as_str())
                    || event.pointer("/params/sessionId").and_then(Value::as_str) == Some(self.session.as_str()))
        })
    }

    fn install(&mut self, cdp: &mut Cdp) -> Result<(i64, String), String> {
        let tree = cdp.call("Page.getFrameTree", json!({}), Some(&self.session))?;
        let frame = tree.pointer("/frameTree/frame").cloned().unwrap_or(Value::Null);
        let (Some(frame_id), Some(loader)) = (frame["id"].as_str(), frame["loaderId"].as_str()) else {
            return Err(format!("{}\n", cdp.guard().refusal()));
        };
        let world = cdp.call("Page.createIsolatedWorld", json!({"frameId":frame_id,"worldName":"slicc-recorder"}), Some(&self.session))?;
        let id = world["executionContextId"].as_i64().ok_or_else(|| format!("{}\n", cdp.guard().refusal()))?;
        let installed = cdp.call("Runtime.evaluate", json!({"expression":install_expression()?,"contextId":id,"returnByValue":true}), Some(&self.session))?;
        if installed.get("exceptionDetails").is_some() {
            return Err(format!("{}\n", cdp.guard().refusal()));
        }
        let href = installed.pointer("/result/value").and_then(Value::as_str).unwrap_or("").to_string();
        let previous = self.last.replace((loader.to_string(), href.clone()));
        if previous.is_some_and(|(before, _)| before != loader) {
            self.navigated(&href);
        }
        Ok((id, loader.to_string()))
    }

    fn navigated(&mut self, href: &str) {
        if href.is_empty() || href == "about:blank" {
            return;
        }
        let code = format!("await page.goto({});", quote(&sanitize::display_url(href)));
        let recent = self.actions.last().filter(|last| matches!(last.name.as_str(), "click" | "press" | "fill") && last.at.elapsed() <= SIGNAL);
        let repeated = self.actions.last().is_some_and(|last| last.name == "navigate" && last.code == code);
        if recent.is_none() && !repeated {
            self.push(Recorded { name: "navigate".into(), locator: String::new(), count: 0, code, at: Instant::now() });
        }
    }

    fn drain(&mut self, cdp: &mut Cdp, id: i64) -> Result<bool, String> {
        let drained = cdp.call("Runtime.evaluate", json!({"expression":DRAIN,"contextId":id,"returnByValue":true,"awaitPromise":true}), Some(&self.session))?;
        if drained.get("exceptionDetails").is_some() {
            return Ok(false);
        }
        let Some(items) = drained.pointer("/result/value").and_then(Value::as_array) else { return Ok(false) };
        for item in items {
            if let Some(action) = recorded(item) {
                self.push(action);
            }
        }
        Ok(true)
    }

    fn push(&mut self, action: Recorded) {
        if let Some(last) = self.actions.last_mut() {
            let fill = action.name == "fill" && last.name == "fill" && action.locator == last.locator;
            let click = action.name == "click" && last.name == "click" && action.locator == last.locator && action.count > last.count;
            if fill || click {
                *last = action;
                return;
            }
        }
        if self.actions.len() < MAX_ACTIONS {
            self.actions.push(action);
        }
    }
}

fn emit(event: Value) -> Result<(), String> {
    let mut stdout = io::stdout();
    writeln!(stdout, "{event}").and_then(|_| stdout.flush()).map_err(|_| "recording-start: failed to write status\n".to_string())
}

fn install_expression() -> Result<String, String> {
    let body = snapshot::SCRIPT
        .strip_prefix(ARIA_HEAD)
        .and_then(|rest| rest.trim_end().strip_suffix(ARIA_TAIL))
        .ok_or_else(|| "recording-start: the recorder script is unavailable\n".to_string())?;
    Ok(format!(
        "(() => {{ if (!globalThis.__sliccRecording) {{ globalThis.__sliccRecording = (() => {{\n{body}\n{RECORDER}\nreturn globalThis.sliccRecorder({{ getAriaRole, getElementAccessibleName, isElementHiddenForAria, beginAriaCaches, endAriaCaches, cssPath }});\n}})(); }} return globalThis.__sliccRecording.href(); }})()"
    ))
}

fn text(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| text.len() <= MAX_TEXT)
}

fn recorded(item: &Value) -> Option<Recorded> {
    let name = text(&item["name"])?;
    let locator = locator(&item["locator"])?;
    let count = item["clickCount"].as_i64().unwrap_or(1);
    let call = match name {
        "click" => click(item, count)?,
        "check" | "uncheck" => format!("{name}()"),
        "fill" => format!("fill({})", quote(text(&item["text"])?)),
        "press" => {
            let mut keys = modifier_names(item["modifiers"].as_i64().unwrap_or(0));
            keys.push(text(&item["key"])?.to_string());
            format!("press({})", quote(&keys.join("+")))
        }
        "select" => format!("selectOption({})", strings(&item["options"])?),
        "setInputFiles" => format!("setInputFiles({})", strings(&item["files"])?),
        _ => return None,
    };
    Some(Recorded { name: name.to_string(), code: format!("await page.{locator}.{call};"), locator, count, at: Instant::now() })
}

fn strings(value: &Value) -> Option<String> {
    let items = value.as_array()?;
    let quoted: Vec<String> = items.iter().map(|item| text(item).map(quote)).collect::<Option<_>>()?;
    Some(if quoted.len() == 1 { quoted[0].clone() } else { format!("[{}]", quoted.join(", ")) })
}

fn click(item: &Value, count: i64) -> Option<String> {
    let method = if count == 2 { "dblclick" } else { "click" };
    let mut options = Map::new();
    let button = text(&item["button"]).unwrap_or("left");
    if !matches!(button, "left" | "middle" | "right") {
        return None;
    }
    if button != "left" {
        options.insert("button".into(), json!(button));
    }
    if count > 2 {
        options.insert("clickCount".into(), json!(count));
    }
    let modifiers = modifier_names(item["modifiers"].as_i64().unwrap_or(0));
    if !modifiers.is_empty() {
        options.insert("modifiers".into(), json!(modifiers));
    }
    if let (Some(x), Some(y)) = (item.pointer("/position/x").and_then(Value::as_f64), item.pointer("/position/y").and_then(Value::as_f64)) {
        options.insert("position".into(), json!({"x":x,"y":y}));
    }
    if options.is_empty() {
        return Some(format!("{method}()"));
    }
    Some(format!("{method}({})", object(&Value::Object(options), 0)))
}

fn object(value: &Value, depth: usize) -> String {
    match value {
        Value::Object(map) => {
            let inner = "  ".repeat(depth + 1);
            let lines: Vec<String> = map.iter().map(|(key, item)| format!("{inner}{key}: {}", object(item, depth + 1))).collect();
            format!("{{\n{}\n{}}}", lines.join(",\n"), "  ".repeat(depth))
        }
        Value::Array(items) => format!("[{}]", items.iter().map(|item| object(item, depth)).collect::<Vec<_>>().join(", ")),
        Value::Number(number) => {
            let float = number.as_f64().unwrap_or_default();
            if float.fract() == 0.0 && float.abs() < 1e15 { format!("{}", float as i64) } else { format!("{float}") }
        }
        _ => quote(value.as_str().unwrap_or_default()),
    }
}

fn modifier_names(bits: i64) -> Vec<String> {
    let mut names = Vec::new();
    if bits & 1 != 0 {
        names.push("Alt".to_string());
    }
    if bits & 2 != 0 {
        names.push("ControlOrMeta".to_string());
    }
    if bits & 4 != 0 {
        names.push("ControlOrMeta".to_string());
    }
    if bits & 8 != 0 {
        names.push("Shift".to_string());
    }
    names
}

fn locator(value: &Value) -> Option<String> {
    let base = match text(&value["kind"])? {
        "role" => {
            let role = text(&value["role"]).filter(|role| !role.is_empty() && role.bytes().all(|byte| byte.is_ascii_lowercase()))?;
            match text(&value["name"]) {
                Some(name) if value["exact"] == true => format!("getByRole({}, {{ name: {}, exact: true }})", quote(role), quote(name)),
                Some(name) => format!("getByRole({}, {{ name: {} }})", quote(role), quote(name)),
                None => format!("getByRole({})", quote(role)),
            }
        }
        "testid" => format!("getByTestId({})", quote(text(&value["value"])?)),
        "placeholder" => format!("getByPlaceholder({})", quote(text(&value["value"])?)),
        "label" => format!("getByLabel({})", quote(text(&value["value"])?)),
        "text" => format!("getByText({})", quote(text(&value["value"])?)),
        "css" => format!("locator({})", quote(text(&value["value"]).filter(|css| !css.is_empty())?)),
        _ => return None,
    };
    Some(match value["nth"].as_u64() {
        Some(0) => format!("{base}.first()"),
        Some(index) => format!("{base}.nth({index})"),
        None => base,
    })
}

fn quote(text: &str) -> String {
    let json = Value::String(text.to_string()).to_string();
    let inner = json[1..json.len() - 1].replace("\\\"", "\"").replace('\'', "\\'");
    format!("'{inner}'")
}
