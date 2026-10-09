use crate::cdp::{self, Cdp};
use crate::net::base64_decode;
use crate::snapshot::{self, AxNode, FrameRec, RefRec};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct PageInfo {
    pub target_id: String,
    pub url: String,
    pub title: String,
}

pub fn list_pages(cdp: &mut Cdp) -> Result<Vec<PageInfo>, String> {
    let result = cdp.call("Target.getTargets", json!({}), None)?;
    let infos = result
        .get("targetInfos")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut pages = Vec::new();
    for info in infos {
        if info.get("type").and_then(|v| v.as_str()) != Some("page") {
            continue;
        }
        let target_id = info
            .get("targetId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let url = info.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let title = info
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if is_internal(&url, &title) || target_id.is_empty() {
            continue;
        }
        pages.push(PageInfo {
            target_id,
            url,
            title,
        });
    }
    Ok(pages)
}

pub fn is_internal(url: &str, title: &str) -> bool {
    let url = url.trim();
    let title = title.trim();
    title == "Omnibox Popup"
        || url.starts_with("chrome://")
        || url.starts_with("chrome-search://")
        || url.starts_with("chrome-untrusted://")
        || url.starts_with("devtools://")
        || (url.is_empty() && title.to_ascii_lowercase().ends_with("popup"))
}

pub fn create_target(cdp: &mut Cdp, url: &str, foreground: bool) -> Result<String, String> {
    let result = cdp.call(
        "Target.createTarget",
        json!({ "url": url, "background": !foreground }),
        None,
    )?;
    let target_id = result
        .get("targetId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Target.createTarget returned no targetId\n".to_string())?
        .to_string();
    if foreground {
        let _ = cdp.call("Target.activateTarget", json!({ "targetId": target_id }), None);
    }
    let session = cdp::attach(cdp, &target_id)?;
    let _ = cdp.call("Page.enable", json!({}), Some(&session));
    cdp.wait_load(&session)?;
    cdp::detach(cdp, &session);
    Ok(target_id)
}

pub fn activate(cdp: &mut Cdp, target_id: &str) -> Result<(), String> {
    cdp.call(
        "Target.activateTarget",
        json!({ "targetId": target_id }),
        None,
    )?;
    Ok(())
}

pub fn close_target(cdp: &mut Cdp, target_id: &str) -> Result<(), String> {
    cdp.call("Target.closeTarget", json!({ "targetId": target_id }), None)?;
    Ok(())
}

pub fn navigate(cdp: &mut Cdp, target_id: &str, url: &str) -> Result<(), String> {
    let session = cdp::attach(cdp, target_id)?;
    let result = (|| {
        cdp.call("Page.enable", json!({}), Some(&session))?;
        let result = cdp.call("Page.navigate", json!({ "url": url }), Some(&session))?;
        if let Some(error) = result.get("errorText").and_then(|v| v.as_str()) {
            return Err(format!("navigate: {error}\n"));
        }
        cdp.wait_load(&session)?;
        Ok(())
    })();
    cdp::detach(cdp, &session);
    result
}

pub struct EvalValue {
    pub value: Value,
    pub defined: bool,
}

pub fn page_location(cdp: &mut Cdp, session: &str) -> Result<(String, String), String> {
    let evaluated = evaluate(
        cdp,
        session,
        "JSON.stringify({href:location.href,title:document.title})",
        false,
    )?;
    let text = match evaluated.value {
        Value::String(text) => text,
        other => other.to_string(),
    };
    let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let href = parsed
        .get("href")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let title = parsed
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok((href, title))
}

pub fn accessibility_tree(cdp: &mut Cdp, session: &str, frame_id: Option<&str>) -> Result<Vec<AxNode>, String> {
    let _ = cdp.call("Accessibility.enable", json!({}), Some(session));
    let params = if let Some(frame_id) = frame_id {
        json!({ "frameId": frame_id })
    } else {
        json!({})
    };
    let result = cdp.call("Accessibility.getFullAXTree", params, Some(session))?;
    let nodes = result
        .get("nodes")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(snapshot::ax_from_cdp(&nodes))
}

pub fn frame_tree(cdp: &mut Cdp, session: &str) -> Result<Vec<FrameRec>, String> {
    let result = cdp.call("Page.getFrameTree", json!({}), Some(session))?;
    Ok(snapshot::flatten_frame_tree(&result))
}

pub fn evaluate(cdp: &mut Cdp, session: &str, expression: &str, await_promise: bool) -> Result<EvalValue, String> {
    let result = cdp.call(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": await_promise,
        }),
        Some(session),
    )?;
    if let Some(details) = result.get("exceptionDetails") {
        return Err(exception_message(details));
    }
    let remote = result.get("result").cloned().unwrap_or(Value::Null);
    let defined = remote.get("type").and_then(|v| v.as_str()) != Some("undefined");
    let value = remote.get("value").cloned().unwrap_or(Value::Null);
    Ok(EvalValue { value, defined })
}

pub fn evaluate_in_frame(
    cdp: &mut Cdp,
    session: &str,
    frame_id: &str,
    expression: &str,
) -> Result<EvalValue, String> {
    let world = cdp.call(
        "Page.createIsolatedWorld",
        json!({ "frameId": frame_id, "grantUniveralAccess": true }),
        Some(session),
    )?;
    let context_id = world
        .get("executionContextId")
        .cloned()
        .ok_or_else(|| "Page.createIsolatedWorld returned no executionContextId\n".to_string())?;
    let result = cdp.call(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "contextId": context_id,
            "returnByValue": true,
            "awaitPromise": true,
        }),
        Some(session),
    )?;
    if let Some(details) = result.get("exceptionDetails") {
        return Err(exception_message(details));
    }
    let remote = result.get("result").cloned().unwrap_or(Value::Null);
    let defined = remote.get("type").and_then(|v| v.as_str()) != Some("undefined");
    let value = remote.get("value").cloned().unwrap_or(Value::Null);
    Ok(EvalValue { value, defined })
}

fn exception_message(details: &Value) -> String {
    let description = details
        .pointer("/exception/description")
        .and_then(|v| v.as_str())
        .or_else(|| details.get("text").and_then(|v| v.as_str()))
        .unwrap_or("evaluation failed");
    format!("{description}\n")
}

pub fn click_ref(
    cdp: &mut Cdp,
    session: &str,
    rec: &RefRec,
    button: &str,
    modifiers: i64,
) -> Result<(), String> {
    if let (Some(frame_id), selector) = (rec.frame_id.as_deref(), first_selector(&rec.selector)) {
        let expression = format!(
            "(function() {{ var el = document.querySelector({}); if (!el) throw new Error('Element not found'); el.scrollIntoView({{block:'center'}}); el.click(); }})()",
            serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into())
        );
        evaluate_in_frame(cdp, session, frame_id, &expression)?;
        return Ok(());
    }
    if let Some(backend) = rec.backend_node_id {
        click_backend(cdp, session, backend, button, modifiers)?;
        return Ok(());
    }
    if !rec.selector.is_empty() {
        let selector = first_selector(&rec.selector);
        let expression = format!(
            "(function() {{ var el = document.querySelector({}); if (!el) throw new Error('Element not found'); el.scrollIntoView({{block:'center'}}); var r = el.getBoundingClientRect(); return {{x:r.x+r.width/2,y:r.y+r.height/2,width:r.width,height:r.height}}; }})()",
            serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into())
        );
        let value = evaluate(cdp, session, &expression, false)?;
        dispatch_click(cdp, session, &value.value, button, modifiers)?;
        return Ok(());
    }
    Err("element has no backend node\n".to_string())
}

fn click_backend(
    cdp: &mut Cdp,
    session: &str,
    backend: u64,
    button: &str,
    modifiers: i64,
) -> Result<(), String> {
    let _ = cdp.call("DOM.enable", json!({}), Some(session));
    let _ = cdp.call("Runtime.enable", json!({}), Some(session));
    let resolved = cdp.call(
        "DOM.resolveNode",
        json!({ "backendNodeId": backend }),
        Some(session),
    )?;
    let object_id = resolved
        .pointer("/object/objectId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("could not resolve backend node {backend}\n"))?
        .to_string();
    let box_result = cdp.call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": "function() { this.scrollIntoView({block:'center',inline:'center'}); const r = this.getBoundingClientRect(); return {x:r.x,y:r.y,width:r.width,height:r.height}; }",
            "returnByValue": true,
        }),
        Some(session),
    )?;
    let value = box_result.pointer("/result/value").cloned().unwrap_or(Value::Null);
    let width = value.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let height = value.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
    if width == 0.0 || height == 0.0 {
        cdp.call(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function() { this.click(); }",
            }),
            Some(session),
        )?;
        return Ok(());
    }
    dispatch_click(cdp, session, &value, button, modifiers)
}

fn dispatch_click(
    cdp: &mut Cdp,
    session: &str,
    value: &Value,
    button: &str,
    modifiers: i64,
) -> Result<(), String> {
    let x = value.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = value.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let width = value.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let height = value.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let cx = if width > 0.0 { x + width / 2.0 } else { x };
    let cy = if height > 0.0 { y + height / 2.0 } else { y };
    for kind in ["mousePressed", "mouseReleased"] {
        cdp.call(
            "Input.dispatchMouseEvent",
            json!({
                "type": kind,
                "x": cx,
                "y": cy,
                "button": button,
                "clickCount": 1,
                "modifiers": modifiers,
            }),
            Some(session),
        )?;
    }
    Ok(())
}

pub fn fill_ref(cdp: &mut Cdp, session: &str, rec: &RefRec, text: &str) -> Result<(), String> {
    if let Some(frame_id) = rec.frame_id.as_deref() {
        let selector = first_selector(&rec.selector);
        let expression = format!(
            "(function() {{ var el = document.querySelector({}); if (!el) throw new Error('Element not found'); el.focus(); el.value = ''; el.value = {}; el.dispatchEvent(new Event('input', {{bubbles:true}})); el.dispatchEvent(new Event('change', {{bubbles:true}})); }})()",
            serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
        );
        evaluate_in_frame(cdp, session, frame_id, &expression)?;
        return Ok(());
    }
    click_ref(cdp, session, rec, "left", 0)?;
    if let Some(backend) = rec.backend_node_id {
        let _ = cdp.call("DOM.enable", json!({}), Some(session));
        let _ = cdp.call("Runtime.enable", json!({}), Some(session));
        let resolved = cdp.call(
            "DOM.resolveNode",
            json!({ "backendNodeId": backend }),
            Some(session),
        )?;
        if let Some(object_id) = resolved.pointer("/object/objectId").and_then(|v| v.as_str()) {
            let object_id = object_id.to_string();
            let _ = cdp.call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": CLEAR_FOCUSABLE,
                    "returnByValue": true,
                }),
                Some(session),
            );
            insert_text(cdp, session, text)?;
            let read = cdp.call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": READ_INPUT,
                    "returnByValue": true,
                }),
                Some(session),
            )?;
            let current = read
                .pointer("/result/value")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if current != text {
                cdp.call(
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": REACT_FILL,
                        "arguments": [{ "value": text }],
                        "returnByValue": true,
                    }),
                    Some(session),
                )?;
            }
            return Ok(());
        }
    }
    insert_text(cdp, session, text)
}

pub fn insert_text(cdp: &mut Cdp, session: &str, text: &str) -> Result<(), String> {
    cdp.call("Input.insertText", json!({ "text": text }), Some(session))?;
    Ok(())
}

pub fn press_key(cdp: &mut Cdp, session: &str, key: &str) -> Result<(), String> {
    let (key_name, code, vk, text, modifiers) = key_info(key);
    let mut down = json!({
        "type": "keyDown",
        "key": key_name,
        "code": code,
        "windowsVirtualKeyCode": vk,
        "modifiers": modifiers,
    });
    if let Some(text) = text {
        down["text"] = json!(text);
    }
    cdp.call("Input.dispatchKeyEvent", down, Some(session))?;
    cdp.call(
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyUp",
            "key": key_name,
            "code": code,
            "windowsVirtualKeyCode": vk,
            "modifiers": modifiers,
        }),
        Some(session),
    )?;
    Ok(())
}

pub fn screenshot(
    cdp: &mut Cdp,
    session: &str,
    full_page: bool,
    clip: Option<Value>,
) -> Result<Vec<u8>, String> {
    let mut params = json!({ "format": "png" });
    if full_page {
        params["captureBeyondViewport"] = json!(true);
    }
    if let Some(clip) = clip {
        params["clip"] = clip;
    }
    let result = cdp.call("Page.captureScreenshot", params, Some(session))?;
    let data = result
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "screenshot returned no data\n".to_string())?;
    base64_decode(data)
}

pub fn element_clip(cdp: &mut Cdp, session: &str, rec: &RefRec) -> Result<Value, String> {
    if let Some(backend) = rec.backend_node_id {
        let _ = cdp.call("DOM.enable", json!({}), Some(session));
        let _ = cdp.call("Runtime.enable", json!({}), Some(session));
        let resolved = cdp.call(
            "DOM.resolveNode",
            json!({ "backendNodeId": backend }),
            Some(session),
        )?;
        if let Some(object_id) = resolved.pointer("/object/objectId").and_then(|v| v.as_str()) {
            let box_result = cdp.call(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": "function() { this.scrollIntoView({block:'center'}); const r = this.getBoundingClientRect(); return {x:r.x+window.scrollX,y:r.y+window.scrollY,width:r.width,height:r.height,scale:1}; }",
                    "returnByValue": true,
                }),
                Some(session),
            )?;
            if let Some(value) = box_result.pointer("/result/value") {
                return Ok(value.clone());
            }
        }
    }
    Err("could not resolve element box\n".to_string())
}

pub fn limit_width(png: &[u8], max_width: u32) -> Result<Vec<u8>, String> {
    let image = image::load_from_memory(png).map_err(|err| format!("png: {err}\n"))?;
    if image.width() <= max_width {
        return Ok(png.to_vec());
    }
    let height = ((image.height() as u64 * max_width as u64) / image.width() as u64).max(1) as u32;
    let scaled = image.resize(max_width, height, image::imageops::FilterType::Triangle);
    let mut bytes = Vec::new();
    scaled
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|err| format!("png: {err}\n"))?;
    Ok(bytes)
}

fn first_selector(selector: &str) -> &str {
    selector.split(',').next().unwrap_or(selector).trim()
}

fn key_info(raw: &str) -> (String, String, i64, Option<String>, i64) {
    let mut modifiers = 0i64;
    let mut key = raw.to_string();
    if raw.contains('+') {
        let mut parts: Vec<&str> = raw.split('+').collect();
        if let Some(last) = parts.pop() {
            for part in parts {
                modifiers |= match part {
                    "Alt" | "Option" => 1,
                    "Control" | "Ctrl" => 2,
                    "Meta" | "Command" | "Cmd" => 4,
                    "Shift" => 8,
                    _ => 0,
                };
            }
            key = last.to_string();
        }
    }
    let (code, vk, text) = match key.as_str() {
        "Enter" => ("Enter".to_string(), 13, Some("\r".to_string())),
        "Tab" => ("Tab".to_string(), 9, Some("\t".to_string())),
        "Escape" | "Esc" => ("Escape".to_string(), 27, None),
        "Backspace" => ("Backspace".to_string(), 8, None),
        "Delete" => ("Delete".to_string(), 46, None),
        "ArrowLeft" => ("ArrowLeft".to_string(), 37, None),
        "ArrowUp" => ("ArrowUp".to_string(), 38, None),
        "ArrowRight" => ("ArrowRight".to_string(), 39, None),
        "ArrowDown" => ("ArrowDown".to_string(), 40, None),
        "Home" => ("Home".to_string(), 36, None),
        "End" => ("End".to_string(), 35, None),
        "PageUp" => ("PageUp".to_string(), 33, None),
        "PageDown" => ("PageDown".to_string(), 34, None),
        " " | "Space" => ("Space".to_string(), 32, Some(" ".to_string())),
        other if other.chars().count() == 1 => {
            let ch = other.chars().next().unwrap();
            let code = if ch.is_ascii_alphabetic() {
                format!("Key{}", ch.to_ascii_uppercase())
            } else if ch.is_ascii_digit() {
                format!("Digit{ch}")
            } else {
                other.to_string()
            };
            let vk = if ch.is_ascii() {
                ch.to_ascii_uppercase() as i64
            } else {
                0
            };
            (code, vk, Some(other.to_string()))
        }
        other => (other.to_string(), 0, None),
    };
    if key == "Esc" {
        key = "Escape".to_string();
    }
    if key == "Space" {
        key = " ".to_string();
    }
    (key, code, vk, text, modifiers)
}

const CLEAR_FOCUSABLE: &str = "function() { const emit = () => this.dispatchEvent(new Event('input', { bubbles: true })); if (this.isContentEditable) { this.textContent = ''; return true; } if ('value' in this) { this.value = ''; emit(); return true; } return false; }";
const READ_INPUT: &str = "function() { if (this.isContentEditable) return this.textContent || ''; if ('value' in this) return this.value; return ''; }";
const REACT_FILL: &str = "function(value) { const proto = this instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype; const desc = Object.getOwnPropertyDescriptor(proto, 'value'); if (desc && desc.set) desc.set.call(this, value); else this.value = value; this.dispatchEvent(new Event('input', { bubbles: true })); this.dispatchEvent(new Event('change', { bubbles: true })); }";

pub fn format_eval(result: &EvalValue) -> String {
    if !result.defined {
        return "undefined".to_string();
    }
    match &result.value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| "null".into()),
    }
}

pub fn lookup_ref<'a>(
    refs: &'a BTreeMap<String, RefRec>,
    id: &str,
) -> Result<&'a RefRec, String> {
    refs.get(id).ok_or_else(|| {
        let available = refs.keys().take(10).cloned().collect::<Vec<_>>().join(", ");
        format!("Unknown ref \"{id}\". Available: {available}\n")
    })
}
