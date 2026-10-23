use crate::action::Report;
use crate::args::{Invocation, Output};
use crate::browser;
use crate::cdp::Cdp;
use crate::commands::{resolve_target, with_ref};
use crate::keys::{self, Key};
use crate::sanitize;
use crate::session::Session;
use crate::snapshot::RefRec;
use serde_json::{json, Value};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const COMMANDS: &[&str] = &["resize", "keydown", "keyup", "mousemove", "mousedown", "mouseup", "mousewheel", "drop"];
const BUTTONS: &[&str] = &["left", "right", "middle"];
const FRAME_OWNERS: &[&str] = &["iframe", "frame", "object", "embed", "fencedframe", "portal"];
const MAX_DROP_BYTES: u64 = 25 * 1024 * 1024;
const MAX_SIZE: f64 = 10_000_000.0;
const MAX_KEYS: usize = 64;
const GROUP: &str = "slicc-input";
const FOCUS: &str = "(() => { let e = document.activeElement; while (e && e.shadowRoot && e.shadowRoot.activeElement) e = e.shadowRoot.activeElement; return e; })()";
const INNER: &str = "[innerWidth, innerHeight]";
const DROP: &str = "function(payloads, data) { if (!this.isConnected || this.nodeType !== 1) return 'error:notconnected'; this.scrollIntoView({block:'center',inline:'center'}); const r = this.getBoundingClientRect(); const point = {x:r.x+r.width/2,y:r.y+r.height/2}; const dt = new DataTransfer(); for (const p of payloads) { const bytes = Uint8Array.from(atob(p.buffer), (c) => c.charCodeAt(0)); dt.items.add(new File([bytes], p.name, { type: p.mimeType, lastModified: p.lastModifiedMs })); } for (const entry of data) dt.setData(entry.mimeType, entry.value); const make = (type) => new DragEvent(type, { bubbles: true, cancelable: true, composed: true, clientX: point.x, clientY: point.y, dataTransfer: dt }); this.dispatchEvent(make('dragenter')); const over = make('dragover'); this.dispatchEvent(over); if (!over.defaultPrevented) { this.dispatchEvent(make('dragleave')); return 'not-accepted'; } this.dispatchEvent(make('drop')); return 'accepted'; }";
const NOT_ACCEPTED: &str = "Error: Drop target did not accept the drop \u{2014} its dragover handler did not call preventDefault()\n";
const BACKGROUND: &str = "Error: mousewheel could not scroll the background tab in place, and brings it to the front only when the tab that was in front can be switched back to. Run \"playwright-cli tab-select <index> --foreground\" first.\n";
const SYNTHETIC_WHEEL: &str = "function(x, y, dx, dy, alt, ctrl, meta, shift, buttons) { let target = document.elementFromPoint(x, y); while (target && target.shadowRoot) { const inner = target.shadowRoot.elementFromPoint(x, y); if (!inner || inner === target) break; target = inner; } const root = document.scrollingElement || document.documentElement; if (!target) target = root; if (/^(iframe|frame|object|embed|fencedframe|portal)$/i.test(target.localName)) return 'frame'; const event = new WheelEvent('wheel', { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, screenX: x, screenY: y, deltaX: dx, deltaY: dy, deltaMode: 0, altKey: alt, ctrlKey: ctrl, metaKey: meta, shiftKey: shift, buttons }); if (!target.dispatchEvent(event)) return 'prevented'; const parent = (node) => node.assignedSlot || node.parentElement || (node.getRootNode() instanceof ShadowRoot ? node.getRootNode().host : null); const room = (node, axis, delta) => { const style = getComputedStyle(node); if (!/(auto|scroll|overlay)/.test(axis === 'x' ? style.overflowX : style.overflowY)) return false; const at = axis === 'x' ? node.scrollLeft : node.scrollTop; const end = axis === 'x' ? node.scrollWidth - node.clientWidth : node.scrollHeight - node.clientHeight; return delta > 0 ? at < end - 0.5 : at > 0.5; }; const scroller = (axis, delta) => { if (!delta) return root; for (let node = target; node && node !== root; node = parent(node)) if (room(node, axis, delta)) return node; return root; }; const sx = scroller('x', dx); const sy = scroller('y', dy); if (sx === sy) sx.scrollBy({ left: dx, top: dy, behavior: 'instant' }); else { sx.scrollBy({ left: dx, top: 0, behavior: 'instant' }); sy.scrollBy({ left: 0, top: dy, behavior: 'instant' }); } return 'scrolled'; }";
const FOREGROUND_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
const WHEEL_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
const NO_PAYLOAD: &str = "Error: At least one of \"paths\" or \"data\" must be provided.\n";

pub fn is_command(command: &str) -> bool {
    COMMANDS.contains(&command)
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Pointer {
    x: f64,
    y: f64,
    last: String,
    buttons: Vec<String>,
    keys: Vec<String>,
    modifiers: Vec<String>,
}

impl Pointer {
    fn load(value: Option<&Value>) -> Pointer {
        let Some(value) = value else { return Pointer { last: "none".to_string(), ..Pointer::default() } };
        let number = |key: &str| value.get(key).and_then(Value::as_f64).filter(|n| n.is_finite()).unwrap_or(0.0);
        let names = |key: &str, allowed: &dyn Fn(&str) -> bool| -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for name in value.get(key).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                if allowed(name) && !out.iter().any(|known| known == name) && out.len() < MAX_KEYS {
                    out.push(name.to_string());
                }
            }
            out
        };
        let last = value.get("lastButton").and_then(Value::as_str).filter(|name| BUTTONS.contains(name)).unwrap_or("none");
        Pointer {
            x: number("x"),
            y: number("y"),
            last: last.to_string(),
            buttons: names("buttons", &|name| BUTTONS.contains(&name)),
            keys: names("keys", &|name| keys::lookup(name).is_some_and(|key| key.code == name)),
            modifiers: names("modifiers", &|name| keys::MODIFIERS.contains(&name)),
        }
    }

    fn save(&self) -> Value {
        json!({ "x": self.x, "y": self.y, "lastButton": self.last, "buttons": self.buttons, "keys": self.keys, "modifiers": self.modifiers })
    }

    fn buttons_mask(&self) -> i64 {
        BUTTONS.iter().enumerate().filter(|(_, name)| self.buttons.iter().any(|held| held == *name)).fold(0, |mask, (bit, _)| mask | 1 << bit)
    }

    fn modifiers_mask(&self) -> i64 {
        keys::mask(&self.modifiers)
    }
}

fn add(list: &mut Vec<String>, name: &str) {
    if !list.iter().any(|known| known == name) {
        list.push(name.to_string());
    }
}

fn missing(name: &str, kind: &str) -> String {
    let received = if kind == "number" { "'undefined'" } else { "undefined" };
    format!("error: '{name}' argument: expected {kind}, received {received}")
}

fn number(invocation: &Invocation, index: usize, name: &str) -> Result<f64, String> {
    let Some(text) = invocation.positionals.get(index) else {
        return Err(missing(name, "number"));
    };
    match crate::cookies::js_number(text) {
        Some(value) if value.is_finite() => Ok(if value == 0.0 { 0.0 } else { value }),
        Some(_) => Err(format!("error: '{name}' argument: expected number, received number")),
        None => Err(format!("error: '{name}' argument: expected number, received '{text}'")),
    }
}

fn numbers(invocation: &Invocation, names: [&str; 2]) -> Result<(f64, f64), String> {
    let first = number(invocation, 0, names[0]);
    let second = number(invocation, 1, names[1]);
    match (first, second) {
        (Ok(first), Ok(second)) => Ok((first, second)),
        (first, second) => {
            let lines: Vec<String> = [first.err(), second.err()].into_iter().flatten().collect();
            Err(format!("{}\n", lines.join("\n")))
        }
    }
}

fn sizes(invocation: &Invocation) -> Result<(u64, u64), String> {
    let (width, height) = numbers(invocation, ["w", "h"])?;
    let whole = |value: f64| value.fract() == 0.0 && (1.0..=MAX_SIZE).contains(&value);
    if !whole(width) || !whole(height) {
        return Err("Error: resize needs whole widths and heights from 1 to 10000000\n".to_string());
    }
    Ok((width as u64, height as u64))
}

fn button(invocation: &Invocation, command: &str) -> Result<Option<String>, String> {
    match invocation.positionals.first() {
        None => Ok(None),
        Some(name) if BUTTONS.contains(&name.as_str()) => Ok(Some(name.clone())),
        Some(_) => {
            let tool = if command == "mousedown" { "browser_mouse_down" } else { "browser_mouse_up" };
            Err(format!("Invalid arguments for tool \"{tool}\":\n\u{2716} Invalid option: expected one of \"left\"|\"right\"|\"middle\"\n  \u{2192} at button\n"))
        }
    }
}

fn key_name(invocation: &Invocation) -> Result<String, String> {
    let Some(key) = invocation.positionals.first() else {
        return Err(format!("{}\n", missing("key", "string")));
    };
    if keys::lookup(keys::smart(key, false)).is_none() {
        return Err(format!("Error: Unknown key: \"{key}\"\n"));
    }
    Ok(key.clone())
}

fn drop_data(invocation: &Invocation) -> Result<Vec<(String, String)>, String> {
    let mut data: Vec<(String, String)> = Vec::new();
    for entry in invocation.repeated("data") {
        let Some((mime, value)) = entry.split_once('=') else {
            return Err(format!("--data must be in \"mime/type=value\" format, got: {entry}\n"));
        };
        match data.iter_mut().find(|(known, _)| known == mime) {
            Some(found) => found.1 = value.to_string(),
            None => data.push((mime.to_string(), value.to_string())),
        }
    }
    Ok(data)
}

pub fn precheck(command: &str, invocation: &Invocation) -> Option<String> {
    let checked = match command {
        "keydown" | "keyup" => key_name(invocation).map(|_| ()),
        "mousemove" => numbers(invocation, ["x", "y"]).map(|_| ()),
        "mousewheel" => numbers(invocation, ["dx", "dy"]).map(|_| ()),
        "resize" => sizes(invocation).map(|_| ()),
        "mousedown" | "mouseup" => button(invocation, command).map(|_| ()),
        "drop" => {
            if invocation.positionals.is_empty() {
                Err(format!("{}\n", missing("target", "string")))
            } else {
                drop_data(invocation).and_then(|data| {
                    if data.is_empty() && invocation.repeated("path").is_empty() {
                        Err(NO_PAYLOAD.to_string())
                    } else {
                        Ok(())
                    }
                })
            }
        }
        _ => Ok(()),
    };
    checked.err()
}

pub fn run(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation, command: &str, report: &mut Report) -> Output {
    if command == "drop" {
        return drop(cdp, state, invocation, report);
    }
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let mut pointer = Pointer::load(state.input.get(&target));
    let result = act(cdp, &session, invocation, command, &mut pointer);
    let _ = cdp.call("Runtime.releaseObjectGroup", json!({ "objectGroup": GROUP }), Some(&session));
    let watched = browser::watch_detach(cdp, state, &target, &session);
    state.input.retain(|known, _| pages.iter().any(|page| &page.target_id == known));
    match result {
        Ok(text) => {
            state.input.insert(target, pointer.save());
            if let Err(message) = watched {
                return Output::err(message);
            }
            Output::ok(sanitize::terminal(&text))
        }
        Err(message) => Output::err(message),
    }
}

fn act(cdp: &mut Cdp, session: &str, invocation: &Invocation, command: &str, pointer: &mut Pointer) -> Result<String, String> {
    match command {
        "keydown" => key_down(cdp, session, &key_name(invocation)?, pointer),
        "keyup" => key_up(cdp, session, &key_name(invocation)?, pointer),
        "mousemove" => {
            let (x, y) = numbers(invocation, ["x", "y"])?;
            admit_point(cdp, session, x, y)?;
            let mut next = pointer.clone();
            next.x = x;
            next.y = y;
            mouse(cdp, session, "mouseMoved", &next, &next.last.clone(), None)?;
            *pointer = next;
            Ok(code(&format!("// Move mouse to ({}, {})\nawait page.mouse.move({}, {});", js(x), js(y), js(x), js(y))))
        }
        "mousedown" | "mouseup" => {
            let chosen = button(invocation, command)?;
            let name = chosen.clone().unwrap_or_else(|| "left".to_string());
            admit_point(cdp, session, pointer.x, pointer.y)?;
            let mut next = pointer.clone();
            let (kind, verb) = if command == "mousedown" {
                next.last = name.clone();
                add(&mut next.buttons, &name);
                ("mousePressed", "down")
            } else {
                next.last = "none".to_string();
                next.buttons.retain(|known| known != &name);
                ("mouseReleased", "up")
            };
            mouse(cdp, session, kind, &next, &name, Some(1))?;
            *pointer = next;
            let options = chosen.map(|name| format!("{{\n  button: {}\n}}", quoted(&name))).unwrap_or_default();
            Ok(code(&format!("// Press mouse {verb}\nawait page.mouse.{verb}({options});")))
        }
        "mousewheel" => {
            let (dx, dy) = numbers(invocation, ["dx", "dy"])?;
            admit_point(cdp, session, pointer.x, pointer.y)?;
            wheel(cdp, session, pointer, (dx, dy))?;
            Ok(code(&format!("// Scroll mouse wheel\nawait page.mouse.wheel({}, {});", js(dx), js(dy))))
        }
        _ => {
            let (width, height) = sizes(invocation)?;
            resize(cdp, session, width, height)?;
            Ok(code(&format!("await page.setViewportSize({{ width: {width}, height: {height} }});")))
        }
    }
}

fn visible(cdp: &mut Cdp, session: &str, waited: std::time::Duration) -> Result<bool, String> {
    let frame = main_frame(cdp, session)?;
    let world = cdp.call("Page.createIsolatedWorld", json!({ "frameId": frame }), Some(session))?;
    let context = world.get("executionContextId").cloned().ok_or_else(|| BACKGROUND.to_string())?;
    let deadline = std::time::Instant::now() + waited;
    loop {
        let state = cdp.call("Runtime.evaluate", json!({ "expression": "document.visibilityState", "contextId": context, "returnByValue": true }), Some(session))?;
        if state.pointer("/result/value") == Some(&json!("visible")) {
            return Ok(true);
        }
        if std::time::Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn synthetic_wheel(cdp: &mut Cdp, session: &str, pointer: &Pointer, (dx, dy): (f64, f64)) -> Result<bool, String> {
    let frame = main_frame(cdp, session)?;
    let world = cdp.call("Page.createIsolatedWorld", json!({ "frameId": frame }), Some(session))?;
    let Some(context) = world.get("executionContextId").cloned() else { return Ok(false) };
    let held = |name: &str| pointer.modifiers.iter().any(|known| known == name);
    let arguments = json!([pointer.x, pointer.y, dx, dy, held("Alt"), held("Control"), held("Meta"), held("Shift"), pointer.buttons_mask()]);
    let expression = format!("({SYNTHETIC_WHEEL})(...{arguments})");
    let scrolled = cdp.call("Runtime.evaluate", json!({ "expression": expression, "contextId": context, "returnByValue": true }), Some(session))?;
    match scrolled.pointer("/result/value").and_then(Value::as_str).filter(|_| scrolled.get("exceptionDetails").is_none()) {
        Some("scrolled" | "prevented") => Ok(true),
        Some("frame") => Err(format!("slicc: refused: mouse input at ({}, {}) would reach an embedded frame\n", js(pointer.x), js(pointer.y))),
        _ => Ok(false),
    }
}

fn trusted_wheel(cdp: &mut Cdp, session: &str, pointer: &Pointer, (dx, dy): (f64, f64)) -> Result<(), String> {
    let wheel = json!({ "type": "mouseWheel", "x": pointer.x, "y": pointer.y, "modifiers": pointer.modifiers_mask(), "deltaX": dx, "deltaY": dy });
    cdp.call_for("Input.dispatchMouseEvent", wheel, Some(session), WHEEL_WAIT).map_err(|message| if message == "Input.dispatchMouseEvent timed out\n" { BACKGROUND.to_string() } else { message })?;
    Ok(())
}

fn wheel(cdp: &mut Cdp, session: &str, pointer: &Pointer, delta: (f64, f64)) -> Result<(), String> {
    if visible(cdp, session, std::time::Duration::ZERO)? {
        return trusted_wheel(cdp, session, pointer, delta);
    }
    if synthetic_wheel(cdp, session, pointer, delta)? {
        return Ok(());
    }
    let front = cdp.bring_forward(session).map_err(|_| BACKGROUND.to_string())?;
    let result = visible(cdp, session, FOREGROUND_WAIT).and_then(|shown| if shown { trusted_wheel(cdp, session, pointer, delta) } else { Err(BACKGROUND.to_string()) });
    let restored = cdp.restore_front(front);
    result.and(restored)
}

fn mouse(cdp: &mut Cdp, session: &str, kind: &str, pointer: &Pointer, button: &str, clicks: Option<i64>) -> Result<(), String> {
    let mut params = json!({ "type": kind, "button": button, "buttons": pointer.buttons_mask(), "x": pointer.x, "y": pointer.y, "modifiers": pointer.modifiers_mask() });
    if let Some(clicks) = clicks {
        params["clickCount"] = json!(clicks);
    }
    if kind != "mouseReleased" {
        params["force"] = json!(if pointer.buttons.is_empty() { 0.0 } else { 0.5 });
    }
    cdp.call("Input.dispatchMouseEvent", params, Some(session))?;
    Ok(())
}

fn describe(name: &str, modifiers: &[String]) -> Result<Key, String> {
    let mut key = keys::lookup(name).ok_or_else(|| format!("Error: Unknown key: \"{name}\"\n"))?;
    if modifiers.iter().any(|held| held == "Shift") {
        if let Some(shifted) = key.shifted.take() {
            key = *shifted;
        }
    }
    let shift_only = modifiers.len() == 1 && modifiers[0] == "Shift";
    if !modifiers.is_empty() && !shift_only {
        key.text = String::new();
    }
    Ok(key)
}

fn mac(cdp: &mut Cdp) -> bool {
    cdp.call("Browser.getVersion", json!({}), None).ok().and_then(|version| version.get("userAgent").and_then(Value::as_str).map(|agent| agent.contains("Macintosh"))).unwrap_or(false)
}

fn key_down(cdp: &mut Cdp, session: &str, raw: &str, pointer: &mut Pointer) -> Result<String, String> {
    let mac = mac(cdp);
    let key = describe(keys::smart(raw, mac), &pointer.modifiers)?;
    let mut next = pointer.clone();
    let repeat = next.keys.iter().any(|known| known == &key.code);
    if !repeat && next.keys.len() >= MAX_KEYS {
        return Err(format!("Error: more than {MAX_KEYS} keys are held down\n"));
    }
    add(&mut next.keys, &key.code);
    if keys::MODIFIERS.contains(&key.key.as_str()) {
        add(&mut next.modifiers, &key.key);
    }
    admit_focus(cdp, session)?;
    let commands = if mac { keys::mac_commands(&key.code, &next.modifiers) } else { Vec::new() };
    cdp.call("Input.dispatchKeyEvent", json!({
        "type": if key.text.is_empty() { "rawKeyDown" } else { "keyDown" },
        "modifiers": next.modifiers_mask(),
        "windowsVirtualKeyCode": key.key_code,
        "code": key.code,
        "commands": commands,
        "key": key.key,
        "text": key.text,
        "unmodifiedText": key.text,
        "autoRepeat": repeat,
        "location": key.location,
        "isKeypad": key.location == keys::KEYPAD,
    }), Some(session))?;
    *pointer = next;
    Ok(code(&format!("await page.keyboard.down({});", quoted(raw))))
}

fn key_up(cdp: &mut Cdp, session: &str, raw: &str, pointer: &mut Pointer) -> Result<String, String> {
    let mac = raw == "ControlOrMeta" && mac(cdp);
    let key = describe(keys::smart(raw, mac), &pointer.modifiers)?;
    let mut next = pointer.clone();
    if keys::MODIFIERS.contains(&key.key.as_str()) {
        next.modifiers.retain(|held| held != &key.key);
    }
    next.keys.retain(|held| held != &key.code);
    admit_focus(cdp, session)?;
    cdp.call("Input.dispatchKeyEvent", json!({
        "type": "keyUp",
        "modifiers": next.modifiers_mask(),
        "key": key.key,
        "windowsVirtualKeyCode": key.key_code,
        "code": key.code,
        "location": key.location,
        "isKeypad": key.location == keys::KEYPAD,
    }), Some(session))?;
    *pointer = next;
    Ok(code(&format!("await page.keyboard.up({});", quoted(raw))))
}

fn main_frame(cdp: &mut Cdp, session: &str) -> Result<String, String> {
    let tree = cdp.call("Page.getFrameTree", json!({}), Some(session))?;
    tree.pointer("/frameTree/frame/id").and_then(Value::as_str).filter(|id| !id.is_empty()).map(str::to_string)
        .ok_or_else(|| format!("{}\n", cdp.guard().refusal()))
}

fn frame_owner(node: &Value) -> bool {
    let name = node.get("localName").or_else(|| node.get("nodeName")).and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    FRAME_OWNERS.contains(&name.as_str())
}

fn admit_point(cdp: &mut Cdp, session: &str, x: f64, y: f64) -> Result<(), String> {
    let refused = || format!("slicc: refused: mouse input at ({}, {}) would reach an embedded frame\n", js(x), js(y));
    let frame = main_frame(cdp, session)?;
    let metrics = cdp.call("Page.getLayoutMetrics", json!({}), Some(session))?;
    let scroll = |key: &str| metrics.pointer(&format!("/cssLayoutViewport/{key}")).and_then(Value::as_f64).filter(|value| value.is_finite());
    let (Some(left), Some(top)) = (scroll("pageX"), scroll("pageY")) else {
        return Err(refused());
    };
    let mut points: Vec<(i64, i64)> = Vec::new();
    for px in [(x + left).floor(), (x + left).ceil()] {
        for py in [(y + top).floor(), (y + top).ceil()] {
            if px.abs() <= i32::MAX as f64 && py.abs() <= i32::MAX as f64 && !points.contains(&(px as i64, py as i64)) {
                points.push((px as i64, py as i64));
            }
        }
    }
    for (px, py) in points {
        let hit = match cdp.call("DOM.getNodeForLocation", json!({ "x": px, "y": py, "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": false }), Some(session)) {
            Ok(hit) => hit,
            Err(message) if message.contains("No node found at given location") && !message.contains("slicc:") => continue,
            Err(message) => return Err(message),
        };
        let backend = hit.get("backendNodeId").and_then(Value::as_u64).ok_or_else(refused)?;
        if hit.get("frameId").and_then(Value::as_str) != Some(frame.as_str()) {
            return Err(refused());
        }
        let node = cdp.call("DOM.describeNode", json!({ "backendNodeId": backend }), Some(session))?;
        if node.get("node").is_none_or(frame_owner) {
            return Err(refused());
        }
    }
    Ok(())
}

fn admit_focus(cdp: &mut Cdp, session: &str) -> Result<(), String> {
    let refused = || "slicc: refused: keyboard focus is inside an embedded frame or a closed shadow root\n".to_string();
    let frame = main_frame(cdp, session)?;
    let world = cdp.call("Page.createIsolatedWorld", json!({ "frameId": frame }), Some(session))?;
    let context = world.get("executionContextId").cloned().ok_or_else(refused)?;
    let focused = cdp.call("Runtime.evaluate", json!({ "expression": FOCUS, "contextId": context, "returnByValue": false, "objectGroup": GROUP }), Some(session))?;
    if focused.get("exceptionDetails").is_some() {
        return Err(refused());
    }
    let remote = focused.get("result").cloned().unwrap_or(Value::Null);
    if remote.get("subtype").and_then(Value::as_str) == Some("null") {
        return Ok(());
    }
    let object = remote.get("objectId").cloned().filter(|_| remote.get("subtype").and_then(Value::as_str) == Some("node")).ok_or_else(refused)?;
    let node = cdp.call("DOM.describeNode", json!({ "objectId": object }), Some(session))?;
    let node = node.get("node").ok_or_else(refused)?;
    let closed = node.get("shadowRoots").and_then(Value::as_array).is_some_and(|roots| roots.iter().any(|root| root.get("shadowRootType").and_then(Value::as_str).is_none_or(|kind| !matches!(kind, "open" | "user-agent"))));
    if frame_owner(node) || closed {
        return Err(refused());
    }
    Ok(())
}

fn resize(cdp: &mut Cdp, session: &str, width: u64, height: u64) -> Result<(), String> {
    let window = cdp.call("Browser.getWindowForTarget", json!({}), Some(session))?;
    let id = window.get("windowId").and_then(Value::as_i64).ok_or("Error: Browser.getWindowForTarget returned no windowId\n".to_string())?;
    let outer = |key: &str| window.pointer(&format!("/bounds/{key}")).and_then(Value::as_f64);
    let frame = main_frame(cdp, session)?;
    let inner = browser::evaluate_in_frame(cdp, session, &frame, INNER)?;
    let viewport = |index: usize| inner.value.get(index).and_then(Value::as_f64);
    let inset = |outer: Option<f64>, inner: Option<f64>| match (outer, inner) {
        (Some(outer), Some(inner)) if outer.is_finite() && inner.is_finite() && outer > inner => (outer - inner).round() as u64,
        _ => 0,
    };
    let bounds = json!({ "width": width + inset(outer("width"), viewport(0)), "height": height + inset(outer("height"), viewport(1)) });
    cdp.call("Browser.setWindowBounds", json!({ "windowId": id, "bounds": bounds }), Some(session))?;
    Ok(())
}

struct DropFile {
    path: String,
    name: String,
    bytes: Vec<u8>,
    modified: f64,
}

fn cwd(invocation: &Invocation) -> PathBuf {
    #[cfg(target_arch = "wasm32")]
    let dir = cdp_client::state::wasm_cwd(|name| std::env::var(name).ok());
    #[cfg(not(target_arch = "wasm32"))]
    let dir = std::env::current_dir().ok();
    invocation.cwd.clone().or(dir).unwrap_or_else(|| PathBuf::from("/"))
}

fn absolute(base: &Path, path: &str) -> PathBuf {
    let joined = base.join(path);
    let mut out = PathBuf::new();
    for part in joined.components() {
        if part == Component::ParentDir {
            out.pop();
        } else {
            out.push(part.as_os_str());
        }
    }
    out
}

fn stat_error(path: &str, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        return format!("Error: ENOENT: no such file or directory, stat '{path}'\n");
    }
    format!("Error: {error}, stat '{path}'\n")
}

fn read_files(paths: &[&str], base: &Path) -> Result<Vec<DropFile>, String> {
    let mut found: Vec<(PathBuf, String, std::fs::Metadata)> = Vec::new();
    let mut directory = false;
    for path in paths {
        let resolved = absolute(base, path);
        let shown = resolved.to_string_lossy().into_owned();
        let meta = std::fs::metadata(&resolved).map_err(|error| stat_error(&shown, &error))?;
        if !meta.is_dir() && !meta.is_file() {
            return Err(format!("Error: {shown} is not a regular file\n"));
        }
        if meta.is_dir() {
            if directory {
                return Err("Error: Multiple directories are not supported\n".to_string());
            }
            directory = true;
        } else {
            found.push((resolved, shown, meta));
        }
    }
    if directory && !found.is_empty() {
        return Err("Error: File paths must be all files or a single directory\n".to_string());
    }
    if directory {
        return Err("Error: Dropping a directory is not supported \u{2014} pass individual files.\n".to_string());
    }
    let mut files = Vec::new();
    let mut total = 0u64;
    for (resolved, shown, meta) in found {
        let file = std::fs::File::open(&resolved).map_err(|error| format!("Error: {error}, open '{shown}'\n"))?;
        let mut bytes = Vec::new();
        file.take(MAX_DROP_BYTES + 1 - total).read_to_end(&mut bytes).map_err(|error| format!("Error: {error}, read '{shown}'\n"))?;
        total += bytes.len() as u64;
        if total > MAX_DROP_BYTES {
            return Err("Error: dropped files exceed the 25 MiB limit\n".to_string());
        }
        let modified = meta.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|since| since.as_secs_f64() * 1000.0).unwrap_or(0.0);
        let name = resolved.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        files.push(DropFile { path: shown, name, bytes, modified });
    }
    Ok(files)
}

fn drop(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation, report: &mut Report) -> Output {
    let id = invocation.positionals[0].clone();
    let data = drop_data(invocation).unwrap_or_default();
    let paths = invocation.repeated("path");
    let base = cwd(invocation);
    let output = with_ref(cdp, state, invocation, report, &id, |cdp, session, rec| {
        let files = read_files(&paths, &base)?;
        drop_on(cdp, session, rec, &files, &data)?;
        Ok(code(&format!("await page.locator({}).drop({});", quoted(&format!("aria-ref={id}")), drop_payload(&files, &data))))
    });
    if output.code == 0 {
        return Output::ok(sanitize::terminal(&output.stdout));
    }
    output
}

fn drop_on(cdp: &mut Cdp, session: &str, rec: &RefRec, files: &[DropFile], data: &[(String, String)]) -> Result<(), String> {
    let backend = rec.backend_node_id.ok_or_else(|| "element has no backend node\n".to_string())?;
    let resolved = cdp.call("DOM.resolveNode", json!({ "backendNodeId": backend, "objectGroup": GROUP }), Some(session))?;
    let object = resolved.pointer("/object/objectId").and_then(Value::as_str).ok_or_else(|| format!("could not resolve backend node {backend}\n"))?.to_string();
    let payloads: Vec<Value> = files.iter().map(|file| json!({ "name": file.name, "mimeType": browser::mime_from_name(&file.name), "buffer": crate::net::base64_encode(&file.bytes), "lastModifiedMs": file.modified })).collect();
    let entries: Vec<Value> = data.iter().map(|(mime, value)| json!({ "mimeType": mime, "value": value })).collect();
    let called = cdp.call("Runtime.callFunctionOn", json!({
        "objectId": object,
        "functionDeclaration": DROP,
        "arguments": [{ "value": payloads }, { "value": entries }],
        "returnByValue": true,
        "objectGroup": GROUP,
    }), Some(session));
    let _ = cdp.call("Runtime.releaseObjectGroup", json!({ "objectGroup": GROUP }), Some(session));
    let called = called?;
    if called.get("exceptionDetails").is_some() {
        return Err("Error: the drop target threw while handling the drop\n".to_string());
    }
    match called.pointer("/result/value").and_then(Value::as_str) {
        Some("accepted") => Ok(()),
        Some("not-accepted") => Err(NOT_ACCEPTED.to_string()),
        _ => Err("Error: Element is not attached to the DOM\n".to_string()),
    }
}

fn drop_payload(files: &[DropFile], data: &[(String, String)]) -> String {
    let mut tokens = Vec::new();
    if !data.is_empty() {
        let mut sorted: Vec<&(String, String)> = data.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let inner: Vec<String> = sorted.iter().map(|(mime, value)| format!("  {mime}: {}", quoted(value))).collect();
        tokens.push(format!("data: {{\n{}\n}}", inner.join(",\n")));
    }
    match files {
        [] => {}
        [file] => tokens.push(format!("files: {}", quoted(&file.path))),
        many => tokens.push(format!("files: [{}]", many.iter().map(|file| quoted(&file.path)).collect::<Vec<_>>().join(", "))),
    }
    format!("{{\n{}\n}}", tokens.iter().map(|token| format!("  {token}")).collect::<Vec<_>>().join(",\n"))
}

fn code(text: &str) -> String {
    format!("### Ran Playwright code\n```js\n{text}\n```\n")
}

fn quoted(text: &str) -> String {
    let json = Value::String(text.to_string()).to_string();
    let inner = json[1..json.len() - 1].replace("\\\"", "\"").replace('\'', "\\'");
    format!("'{inner}'")
}

fn js(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let scientific = format!("{value:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i64 = exponent.parse().unwrap_or(0);
    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let count = digits.len() as i64;
    let point = exponent + 1;
    let body = if count <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - count) as usize))
    } else if 0 < point && point <= 21 {
        format!("{}.{}", &digits[..point as usize], &digits[point as usize..])
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else {
        let rest = if count > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        let sign = if exponent < 0 { "-" } else { "+" };
        format!("{}{rest}e{sign}{}", &digits[..1], exponent.abs())
    };
    format!("{sign}{body}")
}
