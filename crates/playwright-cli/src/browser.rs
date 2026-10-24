use crate::cdp::{self, Cdp};
use crate::landing::{self, Anchor, Gesture, Scope};
use crate::net::base64_decode;
#[cfg(any(target_arch = "wasm32", test))]
use crate::net::base64_encode;
use crate::route;
use crate::session::{self, Session};
use crate::snapshot::{self, FrameRec, RefRec};
use crate::trace;
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
        if is_internal(&url, &title) && !cdp.recovering(&target_id) || target_id.is_empty() || cdp.is_protected_target(&target_id) {
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
        || cdp_client::protected::Protected::default().url(url)
        || url.starts_with("chrome-search://")
        || (url.is_empty() && title.to_ascii_lowercase().ends_with("popup"))
}

pub fn create_target(
    cdp: &mut Cdp,
    state: &mut Session,
    url: &str,
    foreground: bool,
) -> Result<String, String> {
    let blank = url.eq_ignore_ascii_case("about:blank");
    let result = cdp.call(
        "Target.createTarget",
        json!({
            "url": if blank { url } else { "about:blank" },
            "background": !foreground
        }),
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
    enable_page(cdp, &session)?;
    if let Err(message) = trace::enable(cdp, &session) {
        cdp::detach(cdp, &session);
        return Err(message);
    }
    if let Err(message) = route::arm(cdp, &target_id, &session, &state.routes) {
        cdp::detach(cdp, &session);
        return Err(message);
    }
    if let Err(message) = crate::page_tools::arm_network(cdp, &session, state) {
        route::disarm(cdp, &session);
        cdp::detach(cdp, &session);
        return Err(message);
    }
    let mut abandon = false;
    let loaded = if blank {
        cdp.wait_load(&session)
    } else {
        let mark = cdp.events().len();
        match cdp.call("Page.navigate", json!({ "url": url }), Some(&session)) {
            Ok(result) => {
                if let Some(error) = result
                    .get("errorText")
                    .and_then(|value| value.as_str())
                    .filter(|error| !error.is_empty())
                {
                    abandon = true;
                    Err(format!("navigate: {error}\n"))
                } else if navigation_commits(&result) {
                    cdp.wait_load_since(&session, mark)
                } else {
                    Ok(())
                }
            }
            Err(message) => {
                abandon = true;
                Err(message)
            }
        }
    };
    let traced = trace::absorb(cdp, &session);
    route::disarm(cdp, &session);
    cdp::detach(cdp, &session);
    if let Err(message) = loaded {
        if abandon {
            let closed = close_target(cdp, &target_id);
            return Err(closed.err().unwrap_or(message));
        }
        return Err(message);
    }
    let (console, requests) = traced?;
    session::push_trace(state, &target_id, console, requests);
    session::note_owned(state, &target_id);
    if let Some(proof) = cdp.created_blank(&target_id) { state.created_blanks.insert(target_id.clone(), proof); }
    Ok(target_id)
}

pub fn watch_attach(cdp: &mut Cdp, state: &Session, target_id: &str) -> Result<String, String> {
    let session = cdp::attach(cdp, target_id)?;
    if state.owned.iter().any(|owned| owned == target_id) {
        if let Some(proof) = state.created_blanks.get(target_id) { cdp.restore_created_blank(target_id, proof); }
    }
    if let Err(message) = trace::enable(cdp, &session) {
        cdp::detach(cdp, &session);
        return Err(message);
    }
    if let Err(message) = route::arm(cdp, target_id, &session, &state.routes) {
        cdp::detach(cdp, &session);
        return Err(message);
    }
    if let Err(message) = crate::page_tools::arm_network(cdp, &session, state) {
        route::disarm(cdp, &session);
        cdp::detach(cdp, &session);
        return Err(message);
    }
    Ok(session)
}

pub fn watch_detach(
    cdp: &mut Cdp,
    state: &mut Session,
    target_id: &str,
    session: &str,
) -> Result<(), String> {
    let traced = trace::absorb(cdp, session);
    route::disarm(cdp, session);
    cdp::detach(cdp, session);
    let (console, requests) = traced?;
    session::push_trace(state, target_id, console, requests);
    Ok(())
}

fn enable_page(cdp: &mut Cdp, session: &str) -> Result<(), String> {
    match cdp.call("Page.enable", json!({}), Some(session)) {
        Ok(_) => Ok(()),
        Err(err) if cdp_error_response("Page.enable", &err) => Ok(()),
        Err(err) => Err(err),
    }
}

fn cdp_error_response(method: &str, err: &str) -> bool {
    err.starts_with(&format!("{method}:"))
}

fn navigation_commits(result: &Value) -> bool {
    if result.get("isDownload").and_then(|value| value.as_bool()) == Some(true) {
        return false;
    }
    match result.get("loaderId").and_then(|value| value.as_str()) {
        Some(loader_id) if !loader_id.is_empty() => true,
        _ => false,
    }
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

pub fn navigate(
    cdp: &mut Cdp,
    state: &mut Session,
    target_id: &str,
    url: &str,
) -> Result<(), String> {
    let session = cdp::attach(cdp, target_id)?;
    let result = (|| {
        cdp.call("Page.enable", json!({}), Some(&session))?;
        trace::enable(cdp, &session)?;
        route::arm(cdp, target_id, &session, &state.routes)?;
        crate::page_tools::arm_network(cdp, &session, state)?;
        let result = cdp.call("Page.navigate", json!({ "url": url }), Some(&session))?;
        if let Some(error) = result.get("errorText").and_then(|v| v.as_str()) {
            return Err(format!("navigate: {error}\n"));
        }
        cdp.wait_load(&session)?;
        Ok(())
    })();
    let traced = trace::absorb(cdp, &session);
    route::disarm(cdp, &session);
    cdp::detach(cdp, &session);
    result?;
    let (console, requests) = traced?;
    session::push_trace(state, target_id, console, requests);
    Ok(())
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

pub fn aria_capture(
    cdp: &mut Cdp,
    session: &str,
    frame_id: &str,
    prefix: &str,
    options: snapshot::Options,
    default_context: bool,
) -> Result<snapshot::Capture, String> {
    let context_id = if default_context {
        Value::Null
    } else {
        let world = cdp.call(
            "Page.createIsolatedWorld",
            json!({ "frameId": frame_id, "grantUniveralAccess": true }),
            Some(session),
        )?;
        world.get("executionContextId").cloned()
            .ok_or_else(|| "Page.createIsolatedWorld returned no executionContextId\n".to_string())?
    };
    let mut params = json!({
        "expression": snapshot::expression(prefix, options),
        "returnByValue": true,
    });
    if !context_id.is_null() {
        params["contextId"] = context_id.clone();
    }
    let result = cdp.call("Runtime.evaluate", params, Some(session))?;
    if let Some(details) = result.get("exceptionDetails") {
        return Err(exception_message(details));
    }
    let mut capture = snapshot::parse_capture(result.pointer("/result/value").unwrap_or(&Value::Null))?;
    if capture.refs.is_empty() {
        return Ok(capture);
    }
    let described = describe_elements(cdp, session, &context_id, capture.refs.len());
    let _ = cdp.call(
        "Runtime.releaseObjectGroup",
        json!({ "objectGroup": ARIA_GROUP }),
        Some(session),
    );
    for (rec, (backend, frame)) in capture.refs.iter_mut().zip(described?) {
        rec.backend_node_id = backend;
        rec.content_frame = frame;
    }
    Ok(capture)
}

const ARIA_GROUP: &str = "slicc-aria-snapshot";

type Described = (Option<u64>, Option<String>);

fn describe_elements(
    cdp: &mut Cdp,
    session: &str,
    context_id: &Value,
    count: usize,
) -> Result<Vec<Described>, String> {
    let mut params = json!({
        "expression": format!("globalThis[{}]", Value::from(snapshot::ELEMENTS_KEY)),
        "objectGroup": ARIA_GROUP,
    });
    if !context_id.is_null() {
        params["contextId"] = context_id.clone();
    }
    let handle = cdp.call("Runtime.evaluate", params, Some(session))?;
    let array = handle
        .pointer("/result/objectId")
        .and_then(Value::as_str)
        .ok_or_else(|| "snapshot: the page returned no elements\n".to_string())?
        .to_string();
    let properties = cdp.call(
        "Runtime.getProperties",
        json!({ "objectId": array, "ownProperties": true }),
        Some(session),
    )?;
    let mut objects = vec![None; count];
    for item in properties.get("result").and_then(Value::as_array).into_iter().flatten() {
        let index = item.get("name").and_then(Value::as_str).and_then(|name| name.parse::<usize>().ok());
        let object = item.pointer("/value/objectId").and_then(Value::as_str);
        if let (Some(index), Some(object)) = (index.filter(|index| *index < count), object) {
            objects[index] = Some(object.to_string());
        }
    }
    let mut out = Vec::with_capacity(count);
    for object in objects {
        let Some(object) = object else {
            out.push((None, None));
            continue;
        };
        let node = cdp.call("DOM.describeNode", json!({ "objectId": object }), Some(session))?;
        out.push((
            node.pointer("/node/backendNodeId").and_then(Value::as_u64),
            node.pointer("/node/frameId").and_then(Value::as_str).map(str::to_string),
        ));
    }
    Ok(out)
}

const PENDING_FRAME_URL: &str = "(navigating)";

const FRAME_COMMIT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

pub fn frame_tree(cdp: &mut Cdp, session: &str) -> Result<Vec<FrameRec>, String> {
    let from = cdp.events().len();
    enable_page(cdp, session)?;
    let result = cdp.call("Page.getFrameTree", json!({}), Some(session))?;
    let mut frames = snapshot::flatten_frame_tree(&result);
    await_child_commits(cdp, session, from, &mut frames, FRAME_COMMIT_WAIT)?;
    Ok(frames)
}

pub fn page_frames(cdp: &mut Cdp, session: &str) -> Result<Vec<FrameRec>, String> {
    let mut frames = frame_tree(cdp, session)?;
    let listed = cdp.call("Target.getTargets", json!({}), None)?;
    let remote = listed
        .get("targetInfos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|info| info.get("type").and_then(Value::as_str) == Some("iframe"))
        .filter_map(|info| {
            let text = |key: &str| info.get(key).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_string);
            let id = text("targetId")?;
            Some(FrameRec { frame_id: id.clone(), parent_frame_id: Some(text("parentFrameId")?), url: text("url").unwrap_or_default(), target: Some(id) })
        })
        .collect();
    link_remote(&mut frames, remote);
    Ok(frames)
}

fn link_remote(frames: &mut Vec<FrameRec>, mut remote: Vec<FrameRec>) {
    loop {
        let (linked, rest): (Vec<FrameRec>, Vec<FrameRec>) = remote.into_iter().partition(|frame| {
            frames.iter().any(|known| frame.parent_frame_id.as_ref() == Some(&known.frame_id))
                && frames.iter().all(|known| known.frame_id != frame.frame_id)
        });
        if linked.is_empty() {
            return;
        }
        frames.extend(linked);
        remote = rest;
    }
}

fn remote_url(guard: &cdp_client::protected::Protected, url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https")) && !guard.url(url)
}

pub fn open_frame(cdp: &mut Cdp, frame: &FrameRec) -> Result<(String, Vec<FrameRec>), String> {
    let refusal = format!("{}\n", cdp.guard().refusal());
    let target = frame.target.as_deref().filter(|target| {
        *target == frame.frame_id && remote_url(cdp.guard(), &frame.url) && !cdp.is_protected_target(target)
    });
    let Some(target) = target else {
        return Err(refusal);
    };
    let session = cdp::attach(cdp, target)?;
    let frames = frame_tree(cdp, &session);
    match frames {
        Ok(frames) if frames.first().is_some_and(|root| root.frame_id == target && remote_url(cdp.guard(), &root.url)) => Ok((session, frames)),
        other => {
            cdp::detach(cdp, &session);
            Err(other.err().unwrap_or(refusal))
        }
    }
}

pub fn frame_session(cdp: &mut Cdp, session: &str, frame_id: &str) -> Result<Option<String>, String> {
    let frames = page_frames(cdp, session)?;
    match frames.into_iter().find(|frame| frame.frame_id == frame_id && frame.target.is_some()) {
        Some(frame) => open_frame(cdp, &frame).map(|(child, _)| Some(child)),
        None => Ok(None),
    }
}

pub fn ref_session(cdp: &mut Cdp, session: &str, rec: &RefRec) -> Result<(RefRec, Option<String>), String> {
    let Some(target) = rec.target.as_deref() else {
        return Ok((rec.clone(), None));
    };
    let child = frame_session(cdp, session, target)?
        .ok_or_else(|| format!("{}\n", cdp.guard().refusal()))?;
    Ok((RefRec { page: Some(session.to_string()), ..rec.clone() }, Some(child)))
}

fn await_child_commits(
    cdp: &mut Cdp,
    session: &str,
    from: usize,
    frames: &mut [FrameRec],
    timeout: std::time::Duration,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    for frame in frames
        .iter_mut()
        .filter(|frame| frame.parent_frame_id.is_some() && frame.url.is_empty())
    {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let id = frame.frame_id.clone();
        let event = cdp.wait_event(from, remaining, |event| {
            committed_frame_url(event, session, &id).is_some()
        })?;
        if let Some(url) = event.and_then(|event| committed_frame_url(&event, session, &id)) {
            frame.url = url;
        }
    }
    Ok(())
}

fn committed_frame_url(event: &Value, session: &str, frame_id: &str) -> Option<String> {
    if event.get("method").and_then(|v| v.as_str()) != Some("Page.frameNavigated")
        || event.get("sessionId").and_then(|v| v.as_str()) != Some(session)
    {
        return None;
    }
    let frame = event.pointer("/params/frame")?;
    if frame.get("id").and_then(|v| v.as_str()) != Some(frame_id) {
        return None;
    }
    frame
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|url| !url.is_empty())
        .map(str::to_string)
}

pub fn frame_display_url(url: &str) -> String {
    if url.is_empty() {
        PENDING_FRAME_URL.to_string()
    } else {
        crate::sanitize::display_url(url)
    }
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

const EVAL_GROUP: &str = "slicc-eval";

pub fn evaluate_called(
    cdp: &mut Cdp,
    session: &str,
    frame_id: Option<&str>,
    expression: &str,
) -> Result<EvalValue, String> {
    let mut params = json!({
        "expression": expression,
        "returnByValue": false,
        "awaitPromise": true,
        "objectGroup": EVAL_GROUP,
    });
    if let Some(frame_id) = frame_id {
        let world = cdp.call(
            "Page.createIsolatedWorld",
            json!({ "frameId": frame_id, "grantUniveralAccess": true }),
            Some(session),
        )?;
        params["contextId"] = world
            .get("executionContextId")
            .cloned()
            .ok_or("Page.createIsolatedWorld returned no executionContextId\n".to_string())?;
    }
    let source = expression.trim();
    if source.starts_with("function") || source.starts_with("async function") {
        let wrapped = format!("({source}\n)");
        let mut compile = json!({ "expression": wrapped, "sourceURL": "", "persistScript": false });
        if let Some(context) = params.get("contextId") {
            compile["executionContextId"] = context.clone();
        }
        let parsed = cdp.call("Runtime.compileScript", compile, Some(session))?;
        if parsed.get("exceptionDetails").is_none() {
            params["expression"] = json!(wrapped);
        }
    }
    let value = cdp
        .call("Runtime.evaluate", params, Some(session))
        .and_then(|result| settle_eval(cdp, session, &result));
    let _ = cdp.call("Runtime.releaseObjectGroup", json!({ "objectGroup": EVAL_GROUP }), Some(session));
    value
}

fn settle_eval(cdp: &mut Cdp, session: &str, result: &Value) -> Result<EvalValue, String> {
    if let Some(details) = result.get("exceptionDetails") {
        return Err(exception_message(details));
    }
    let remote = result.get("result").cloned().unwrap_or(Value::Null);
    let declaration = match remote.get("type").and_then(Value::as_str) {
        Some("function") => "function() { return this(); }",
        Some("object") => "function() { return this; }",
        _ => return Ok(by_value(&remote)),
    };
    let Some(object) = remote.get("objectId").cloned() else {
        return Ok(by_value(&remote));
    };
    let called = cdp.call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object,
            "functionDeclaration": declaration,
            "returnByValue": true,
            "awaitPromise": true,
            "objectGroup": EVAL_GROUP,
        }),
        Some(session),
    )?;
    if let Some(details) = called.get("exceptionDetails") {
        return Err(exception_message(details));
    }
    Ok(by_value(called.get("result").unwrap_or(&Value::Null)))
}

fn by_value(remote: &Value) -> EvalValue {
    EvalValue {
        value: remote.get("value").cloned().unwrap_or(Value::Null),
        defined: remote.get("type").and_then(Value::as_str) != Some("undefined"),
    }
}

fn exception_message(details: &Value) -> String {
    let description = details
        .pointer("/exception/description")
        .and_then(|v| v.as_str())
        .or_else(|| details.get("text").and_then(|v| v.as_str()))
        .unwrap_or("evaluation failed");
    format!("{}\n", crate::sanitize::browser_message(description))
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
        let point = center(&value.value);
        let gesture = Gesture::Click { button, modifiers };
        return landing::pointer(cdp, &Scope::page(session), &Anchor::Selector(selector), point, gesture, &mut |cdp| dispatch_click(cdp, session, point, button, modifiers));
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
    let point = center(&value);
    let gesture = Gesture::Click { button, modifiers };
    landing::pointer(cdp, &Scope::page(session), &Anchor::Backend(backend), point, gesture, &mut |cdp| dispatch_click(cdp, session, point, button, modifiers))
}

fn center(value: &Value) -> (f64, f64) {
    let x = value.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = value.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let width = value.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let height = value.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let cx = if width > 0.0 { x + width / 2.0 } else { x };
    let cy = if height > 0.0 { y + height / 2.0 } else { y };
    (cx, cy)
}

fn dispatch_click(
    cdp: &mut Cdp,
    session: &str,
    (cx, cy): (f64, f64),
    button: &str,
    modifiers: i64,
) -> Result<(), String> {
    for kind in ["mousePressed", "mouseReleased"] {
        cdp.call_for(
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
            landing::INPUT_WAIT,
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
    cdp.call_for("Input.insertText", json!({ "text": text }), Some(session), landing::INPUT_WAIT)?;
    Ok(())
}

pub fn type_text(cdp: &mut Cdp, session: &str, text: &str) -> Result<(), String> {
    landing::text(cdp, session, text, &mut |cdp| insert_text(cdp, session, text))
}

pub fn press_key(cdp: &mut Cdp, session: &str, key: &str) -> Result<(), String> {
    let (key_name, code, _, text, modifiers) = key_info(key);
    let keyed = landing::Keyed { key: &key_name, code: &code, text: text.as_deref(), modifiers };
    landing::key(cdp, session, &keyed, &mut |cdp| dispatch_key(cdp, session, key))
}

fn dispatch_key(cdp: &mut Cdp, session: &str, key: &str) -> Result<(), String> {
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
    cdp.call_for("Input.dispatchKeyEvent", down, Some(session), landing::INPUT_WAIT)?;
    cdp.call_for(
        "Input.dispatchKeyEvent",
        json!({
            "type": "keyUp",
            "key": key_name,
            "code": code,
            "windowsVirtualKeyCode": vk,
            "modifiers": modifiers,
        }),
        Some(session),
        landing::INPUT_WAIT,
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

const DIALOG_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

pub fn handle_dialog(
    cdp: &mut Cdp,
    session: &str,
    accept: bool,
    prompt: Option<&str>,
) -> Result<String, String> {
    cdp.wait_dialog(session, DIALOG_WAIT)?;
    let mut params = json!({ "accept": accept });
    if let Some(text) = prompt {
        params["promptText"] = json!(text);
    }
    cdp.call_for("Page.handleJavaScriptDialog", params, Some(session), DIALOG_WAIT)?;
    Ok(if accept { "Accepted the dialog\n" } else { "Dismissed the dialog\n" }.to_string())
}

pub fn format_eval(result: &EvalValue) -> String {
    if !result.defined {
        return "undefined".to_string();
    }
    match &result.value {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| "null".into()),
    }
}

pub fn dblclick_ref(
    cdp: &mut Cdp,
    session: &str,
    rec: &RefRec,
    button: &str,
    modifiers: i64,
) -> Result<(), String> {
    if let (Some(frame_id), selector) = (rec.frame_id.as_deref(), first_selector(&rec.selector)) {
        let expression = format!(
            "(function() {{ var el = document.querySelector({}); if (!el) throw new Error('Element not found'); el.scrollIntoView({{block:'center'}}); el.dispatchEvent(new MouseEvent('dblclick', {{ bubbles: true }})); }})()",
            serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into())
        );
        evaluate_in_frame(cdp, session, frame_id, &expression)?;
        return Ok(());
    }
    if let Some(backend) = rec.backend_node_id {
        dblclick_backend(cdp, session, backend, button, modifiers)?;
        return Ok(());
    }
    Err("element has no backend node\n".to_string())
}

fn dblclick_backend(
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
    let (cx, cy) = center(&value);
    let gesture = Gesture::Double { button, modifiers };
    landing::pointer(cdp, &Scope::page(session), &Anchor::Backend(backend), (cx, cy), gesture, &mut |cdp| {
        for (kind, count) in [("mousePressed", 1), ("mouseReleased", 1), ("mousePressed", 2), ("mouseReleased", 2)] {
            cdp.call_for(
                "Input.dispatchMouseEvent",
                json!({
                    "type": kind,
                    "x": cx,
                    "y": cy,
                    "button": button,
                    "clickCount": count,
                    "modifiers": modifiers,
                }),
                Some(session),
                landing::INPUT_WAIT,
            )?;
        }
        Ok(())
    })
}

pub fn hover_ref(
    cdp: &mut Cdp,
    session: &str,
    rec: &RefRec,
) -> Result<(), String> {
    if let Some(frame_id) = rec.frame_id.as_deref() {
        let selector = first_selector(&rec.selector);
        let frame_offset = ref_frame_offset(cdp, session, rec, frame_id)?;
        let expression = format!(
            "(function() {{ var el = document.querySelector({}); if (!el) throw new Error('Element not found'); el.scrollIntoView({{block:'center'}}); var r = el.getBoundingClientRect(); return {{x:r.x+r.width/2,y:r.y+r.height/2}}; }})()",
            serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into())
        );
        let pos = evaluate_in_frame(cdp, session, frame_id, &expression)?;
        let fx = pos.value.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let fy = pos.value.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let cx = frame_offset.0 + fx;
        let cy = frame_offset.1 + fy;
        let input = input_session(session, rec);
        let scope = Scope { session, frame: Some(frame_id), input };
        return landing::pointer(cdp, &scope, &Anchor::Selector(selector), (fx, fy), Gesture::Hover, &mut |cdp| move_mouse(cdp, input, (cx, cy)));
    }
    if let Some(backend) = rec.backend_node_id {
        hover_backend(cdp, session, backend)?;
        return Ok(());
    }
    Err("element has no backend node\n".to_string())
}

fn hover_backend(
    cdp: &mut Cdp,
    session: &str,
    backend: u64,
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
    let point = center(&value);
    landing::pointer(cdp, &Scope::page(session), &Anchor::Backend(backend), point, Gesture::Hover, &mut |cdp| move_mouse(cdp, session, point))
}

fn move_mouse(cdp: &mut Cdp, session: &str, (x, y): (f64, f64)) -> Result<(), String> {
    cdp.call_for("Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": x, "y": y }), Some(session), landing::INPUT_WAIT)?;
    Ok(())
}

pub fn select_ref(
    cdp: &mut Cdp,
    session: &str,
    rec: &RefRec,
    value: &str,
) -> Result<(), String> {
    if let (Some(frame_id), selector) = (rec.frame_id.as_deref(), first_selector(&rec.selector)) {
        let expression = format!(
            "(function() {{ var el = document.querySelector({}); if (!el) throw new Error('Element not found'); el.value = {}; el.dispatchEvent(new Event('input', {{ bubbles: true }})); el.dispatchEvent(new Event('change', {{ bubbles: true }})); }})()",
            serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
        );
        evaluate_in_frame(cdp, session, frame_id, &expression)?;
        return Ok(());
    }
    if let Some(backend) = rec.backend_node_id {
        select_backend(cdp, session, backend, value)?;
        return Ok(());
    }
    Err("element has no backend node\n".to_string())
}

fn select_backend(
    cdp: &mut Cdp,
    session: &str,
    backend: u64,
    value: &str,
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
    cdp.call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": "function(v) { this.value = v; this.dispatchEvent(new Event('input', { bubbles: true })); this.dispatchEvent(new Event('change', { bubbles: true })); }",
            "arguments": [{ "value": value }],
            "returnByValue": true,
        }),
        Some(session),
    )?;
    Ok(())
}

pub fn check_ref(
    cdp: &mut Cdp,
    session: &str,
    rec: &RefRec,
    checked: bool,
) -> Result<bool, String> {
    if let Some(frame_id) = rec.frame_id.as_deref() {
        let selector = first_selector(&rec.selector);
        let pre_expr = format!(
            "(function() {{ var el = document.querySelector({}); if (!el) throw new Error('Element not found'); return JSON.stringify({{ checked: el.checked, disabled: el.disabled || false }}); }})()",
            serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into())
        );
        let pre = evaluate_in_frame(cdp, session, frame_id, &pre_expr)?;
        let pre_text = match &pre.value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let pre_parsed: Value = serde_json::from_str(&pre_text).unwrap_or(Value::Null);
        if pre_parsed.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
            return Err("element is disabled\n".to_string());
        }
        if pre_parsed.get("checked").and_then(|v| v.as_bool()) == Some(checked) {
            return Ok(false);
        }
        let frame_offset = ref_frame_offset(cdp, session, rec, frame_id)?;
        let click_expr = format!(
            "(function() {{ var el = document.querySelector({}); if (!el) throw new Error('Element not found'); el.scrollIntoView({{block:'center'}}); var r = el.getBoundingClientRect(); return {{x:r.x+r.width/2,y:r.y+r.height/2}}; }})()",
            serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into())
        );
        let pos = evaluate_in_frame(cdp, session, frame_id, &click_expr)?;
        let fx = pos.value.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let fy = pos.value.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let point = (frame_offset.0 + fx, frame_offset.1 + fy);
        let input = input_session(session, rec);
        let scope = Scope { session, frame: Some(frame_id), input };
        landing::pointer(cdp, &scope, &Anchor::Selector(selector), (fx, fy), TOGGLE, &mut |cdp| dispatch_click(cdp, input, point, "left", 0))?;
        let post_expr = format!(
            "(function() {{ var el = document.querySelector({}); return el ? el.checked : null; }})()",
            serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".into())
        );
        toggled(cdp, checked, &mut |cdp| Ok(evaluate_in_frame(cdp, session, frame_id, &post_expr)?.value.as_bool()))?;
        return Ok(true);
    }
    if let Some(backend) = rec.backend_node_id {
        return check_backend(cdp, session, backend, checked);
    }
    Err("element has no backend node\n".to_string())
}

fn check_backend(
    cdp: &mut Cdp,
    session: &str,
    backend: u64,
    checked: bool,
) -> Result<bool, String> {
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
    let pre = cdp.call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": "function() { return JSON.stringify({ checked: this.checked, disabled: this.disabled || false }); }",
            "returnByValue": true,
        }),
        Some(session),
    )?;
    let pre_text = pre.pointer("/result/value").and_then(|v| v.as_str()).unwrap_or("{}").to_string();
    let pre_parsed: Value = serde_json::from_str(&pre_text).unwrap_or(Value::Null);
    if pre_parsed.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
        return Err("element is disabled\n".to_string());
    }
    if pre_parsed.get("checked").and_then(|v| v.as_bool()) == Some(checked) {
        return Ok(false);
    }
    let box_result = cdp.call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": "function() { this.scrollIntoView({block:'center',inline:'center'}); const r = this.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; }",
            "returnByValue": true,
        }),
        Some(session),
    )?;
    let value = box_result.pointer("/result/value").cloned().unwrap_or(Value::Null);
    let point = (
        value.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0),
        value.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0),
    );
    landing::pointer(cdp, &Scope::page(session), &Anchor::Backend(backend), point, TOGGLE, &mut |cdp| dispatch_click(cdp, session, point, "left", 0))?;
    toggled(cdp, checked, &mut |cdp| {
        let post = cdp.call(
            "Runtime.callFunctionOn",
            json!({
                "objectId": object_id,
                "functionDeclaration": "function() { return this.checked; }",
                "returnByValue": true,
            }),
            Some(session),
        )?;
        Ok(post.pointer("/result/value").and_then(|v| v.as_bool()))
    })?;
    Ok(true)
}

const TOGGLE: Gesture<'static> = Gesture::Click { button: "left", modifiers: 0 };
const TOGGLE_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

fn toggled(cdp: &mut Cdp, checked: bool, read: &mut dyn FnMut(&mut Cdp) -> Result<Option<bool>, String>) -> Result<(), String> {
    let deadline = std::time::Instant::now() + TOGGLE_WAIT;
    loop {
        if read(cdp)? == Some(checked) {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("check: element state did not change to {}\n", checked));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

pub fn drag_refs(
    cdp: &mut Cdp,
    session: &str,
    (start_session, start_rec): (&str, &RefRec),
    (end_session, end_rec): (&str, &RefRec),
) -> Result<(), String> {
    let start = element_center(cdp, start_session, start_rec)?;
    let end = element_center(cdp, end_session, end_rec)?;
    let step = |kind, point, buttons| landing::Step { kind, point, buttons, button: 0 };
    let motion = landing::Motion {
        steps: vec![step("move", start, 0), step("down", start, 1), step("move", end, 1), step("up", end, 0)],
        from: None,
        press: None,
        modifiers: 0,
    };
    let in_place = [start_rec, end_rec].iter().all(|rec| rec.frame_id.is_none() && rec.target.is_none());
    let scope = Scope { session: start_session, frame: start_rec.frame_id.as_deref(), input: session };
    landing::mouse(cdp, &scope, "drag", &motion, in_place, &mut |cdp| {
        for (kind, (x, y), button, buttons) in [("mouseMoved", start, "none", 0), ("mousePressed", start, "left", 1), ("mouseMoved", end, "left", 1), ("mouseReleased", end, "left", 0)] {
            let mut params = json!({ "type": kind, "x": x, "y": y, "button": button, "buttons": buttons });
            if kind != "mouseMoved" {
                params["clickCount"] = json!(1);
            }
            cdp.call_for("Input.dispatchMouseEvent", params, Some(session), landing::INPUT_WAIT)?;
        }
        Ok(())
    })
}

fn element_center(cdp: &mut Cdp, session: &str, rec: &RefRec) -> Result<(f64, f64), String> {
    let backend = rec
        .backend_node_id
        .ok_or_else(|| "element has no backend node\n".to_string())?;
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
            "functionDeclaration": "function() { this.scrollIntoView({block:'center',inline:'center'}); const r = this.getBoundingClientRect(); return {x:r.x+r.width/2,y:r.y+r.height/2}; }",
            "returnByValue": true,
        }),
        Some(session),
    )?;
    let value = box_result.pointer("/result/value").cloned().unwrap_or(Value::Null);
    let x = value.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = value.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let (ox, oy) = remote_offset(cdp, session, rec)?;
    Ok((x + ox, y + oy))
}

fn frame_element_offset(cdp: &mut Cdp, session: &str, frame_id: &str) -> Result<(f64, f64), String> {
    let result = cdp.call(
        "DOM.getFrameOwner",
        json!({ "frameId": frame_id }),
        Some(session),
    )?;
    let backend = result
        .get("backendNodeId")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "could not find frame owner element\n".to_string())?;
    owner_offset(cdp, session, backend)
}

fn ref_frame_offset(cdp: &mut Cdp, session: &str, rec: &RefRec, frame_id: &str) -> Result<(f64, f64), String> {
    if rec.target.is_none() {
        return frame_element_offset(cdp, session, frame_id);
    }
    remote_offset(cdp, session, rec)
}

fn remote_offset(cdp: &mut Cdp, session: &str, rec: &RefRec) -> Result<(f64, f64), String> {
    let (Some(target), Some(page)) = (rec.target.as_deref(), rec.page.as_deref()) else {
        return Ok((0.0, 0.0));
    };
    let owner = rec
        .owner
        .ok_or_else(|| "could not find frame owner element\n".to_string())?;
    let (x, y) = owner_offset(cdp, page, owner)?;
    match rec.frame_id.as_deref().filter(|frame| *frame != target) {
        Some(frame) => {
            let (fx, fy) = frame_element_offset(cdp, session, frame)?;
            Ok((x + fx, y + fy))
        }
        None => Ok((x, y)),
    }
}

fn input_session<'a>(session: &'a str, rec: &'a RefRec) -> &'a str {
    rec.page.as_deref().unwrap_or(session)
}

fn owner_offset(cdp: &mut Cdp, session: &str, backend: u64) -> Result<(f64, f64), String> {
    let _ = cdp.call("Runtime.enable", json!({}), Some(session));
    let resolved = cdp.call(
        "DOM.resolveNode",
        json!({ "backendNodeId": backend }),
        Some(session),
    )?;
    let object_id = resolved
        .pointer("/object/objectId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "could not resolve frame owner\n".to_string())?
        .to_string();
    let box_result = cdp.call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": "function() { var x = 0, y = 0, el = this; while (el) { var r = el.getBoundingClientRect(); var s = el.ownerDocument.defaultView.getComputedStyle(el); x += r.x + (parseFloat(s.borderLeftWidth)||0) + (parseFloat(s.paddingLeft)||0); y += r.y + (parseFloat(s.borderTopWidth)||0) + (parseFloat(s.paddingTop)||0); el = el.ownerDocument.defaultView.frameElement; } return {x:x,y:y}; }",
            "returnByValue": true,
        }),
        Some(session),
    )?;
    let value = box_result.pointer("/result/value").cloned().unwrap_or(Value::Null);
    let x = value.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let y = value.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    Ok((x, y))
}

#[cfg(any(target_arch = "wasm32", test))]
const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug)]
struct UploadEntry {
    name: String,
    mime: String,
    base64: String,
}

#[cfg(any(target_arch = "wasm32", test))]
fn upload_entry(path_str: &str, data: &[u8]) -> Result<UploadEntry, String> {
    if data.len() > MAX_UPLOAD_BYTES {
        return Err(format!(
            "upload: {path_str} is {} bytes, exceeding the 25 MiB limit\n",
            data.len()
        ));
    }
    let name = std::path::Path::new(path_str)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path_str)
        .to_string();
    let mime = mime_from_name(&name);
    Ok(UploadEntry {
        name,
        mime,
        base64: base64_encode(data),
    })
}

#[cfg(any(target_arch = "wasm32", test))]
fn upload_call_args(entries: &[UploadEntry]) -> (String, Vec<Value>) {
    let file_array_js = entries
        .iter()
        .enumerate()
        .map(|(i, _)| format!("files[{i}]"))
        .collect::<Vec<_>>()
        .join(", ");
    let func = format!(
        "function(...args) {{ \
            const dt = new DataTransfer(); \
            const files = []; \
            for (let i = 0; i < args.length; i += 3) {{ \
                const bytes = Uint8Array.from(atob(args[i+2]), c => c.charCodeAt(0)); \
                files.push(new File([bytes], args[i], {{ type: args[i+1] }})); \
            }} \
            for (const f of [{file_array_js}]) dt.items.add(f); \
            this.files = dt.files; \
            this.dispatchEvent(new Event('input', {{ bubbles: true }})); \
            this.dispatchEvent(new Event('change', {{ bubbles: true }})); \
        }}"
    );
    let mut arguments: Vec<Value> = Vec::new();
    for entry in entries {
        arguments.push(json!({ "value": entry.name }));
        arguments.push(json!({ "value": entry.mime }));
        arguments.push(json!({ "value": entry.base64 }));
    }
    (func, arguments)
}

#[cfg(target_arch = "wasm32")]
pub fn upload_files(
    cdp: &mut Cdp,
    session: &str,
    rec: &RefRec,
    files: &[String],
) -> Result<(), String> {
    let backend = rec
        .backend_node_id
        .ok_or_else(|| "element has no backend node\n".to_string())?;
    let mut entries: Vec<UploadEntry> = Vec::new();
    for path_str in files {
        let data = std::fs::read(path_str)
            .map_err(|err| format!("upload: cannot read {path_str}: {err}\n"))?;
        entries.push(upload_entry(path_str, &data)?);
    }
    let _ = cdp.call("DOM.enable", json!({}), Some(session));
    let _ = cdp.call("Runtime.enable", json!({}), Some(session));
    let frame_id = rec
        .frame_id
        .clone()
        .or_else(|| {
            cdp.call("Page.getFrameTree", json!({}), Some(session))
                .ok()
                .and_then(|r| {
                    r.pointer("/frameTree/frame/id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
        })
        .ok_or_else(|| "upload: could not determine frame id\n".to_string())?;
    let world = cdp.call(
        "Page.createIsolatedWorld",
        json!({ "frameId": frame_id, "grantUniveralAccess": true }),
        Some(session),
    )?;
    let context_id = world
        .get("executionContextId")
        .ok_or_else(|| "upload: isolated world returned no context\n".to_string())?;
    let resolved = cdp.call(
        "DOM.resolveNode",
        json!({ "backendNodeId": backend, "executionContextId": context_id }),
        Some(session),
    )?;
    let object_id = resolved
        .pointer("/object/objectId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("could not resolve backend node {backend}\n"))?
        .to_string();
    let (func, arguments) = upload_call_args(&entries);
    let result = cdp.call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object_id,
            "functionDeclaration": func,
            "arguments": arguments,
            "returnByValue": true,
        }),
        Some(session),
    )?;
    if let Some(details) = result.get("exceptionDetails") {
        return Err(format!("upload: JS error: {}\n", exception_message(details)));
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn upload_files(
    cdp: &mut Cdp,
    session: &str,
    rec: &RefRec,
    files: &[String],
) -> Result<(), String> {
    let backend = rec
        .backend_node_id
        .ok_or_else(|| "element has no backend node\n".to_string())?;
    cdp.call(
        "DOM.setFileInputFiles",
        json!({
            "files": files,
            "backendNodeId": backend,
        }),
        Some(session),
    )?;
    Ok(())
}

pub fn validate_upload_path(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if !p.exists() {
        return Err(format!("upload: file not found: {path}\n"));
    }
    if p.is_dir() {
        return Err(format!("upload: path is a directory: {path}\n"));
    }
    match std::fs::File::open(p) {
        Ok(_) => Ok(()),
        Err(err) => Err(format!("upload: cannot read {path}: {err}\n")),
    }
}

pub(crate) fn mime_from_name(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" => "application/javascript",
        "json" => "application/json",
        "xml" => "application/xml",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
    .to_string()
}

pub fn print_to_pdf(cdp: &mut Cdp, session: &str) -> Result<Vec<u8>, String> {
    let result = cdp.call("Page.printToPDF", json!({}), Some(session))?;
    let data = result
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "pdf returned no data\n".to_string())?;
    base64_decode(data)
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

#[cfg(test)]
mod tests {
    use super::{await_child_commits, cdp_error_response, committed_frame_url, frame_display_url, frame_tree, mime_from_name, navigation_commits, upload_call_args, upload_entry, validate_upload_path, MAX_UPLOAD_BYTES};
    use crate::cdp::{Cdp, Transport};
    use crate::snapshot::FrameRec;
    use serde_json::{json, Value};
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};

    struct Scripted {
        incoming: VecDeque<String>,
        sent: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    }

    impl Transport for Scripted {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            self.sent.borrow_mut().push(text.to_string());
            Ok(())
        }

        fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
            Ok(self.incoming.pop_front())
        }
    }

    fn scripted(messages: Vec<Value>) -> (Cdp, std::rc::Rc<std::cell::RefCell<Vec<String>>>) {
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let transport = Scripted {
            incoming: messages.into_iter().map(|m| m.to_string()).collect(),
            sent: sent.clone(),
        };
        (crate::document_client::client(Box::new(transport), "S"), sent)
    }

    fn navigated(session: &str, frame: &str, url: &str) -> Value {
        json!({ "method": "Page.frameNavigated", "sessionId": session, "params": { "frame": { "id": frame, "url": url } } })
    }

    fn tree(child_url: &str) -> Value {
        json!({ "frameTree": { "frame": { "id": "M", "url": "http://a/page" }, "childFrames": [ { "frame": { "id": "C", "parentId": "M", "url": child_url } } ] } })
    }

    #[test]
    fn frame_tree_enables_page_before_reading_a_committed_tree() {
        let (mut cdp, sent) = scripted(vec![
            json!({ "id": 1, "result": {} }),
            json!({ "id": 2, "result": tree("http://a/child.html") }),
        ]);
        let frames = frame_tree(&mut cdp, "S").unwrap();
        assert_eq!(frames[1].url, "http://a/child.html");
        let methods: Vec<String> = sent
            .borrow()
            .iter()
            .map(|text| serde_json::from_str::<Value>(text).unwrap()["method"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(methods, ["Page.enable", "Page.getFrameTree"]);
    }

    #[test]
    fn frame_tree_waits_for_an_uncommitted_child_to_navigate() {
        let (mut cdp, _) = scripted(vec![
            json!({ "id": 1, "result": {} }),
            json!({ "id": 2, "result": tree("") }),
            navigated("OTHER", "C", "http://a/wrong"),
            navigated("S", "M", "http://a/main"),
            navigated("S", "C", "http://a/child.html"),
        ]);
        let frames = frame_tree(&mut cdp, "S").unwrap();
        assert_eq!(frames[1].url, "http://a/child.html");
    }

    #[test]
    fn frame_tree_refuses_a_tree_racing_with_a_commit() {
        let (mut cdp, _) = scripted(vec![
            json!({ "id": 1, "result": {} }),
            navigated("S", "C", "http://a/child.html"),
            json!({ "id": 2, "result": tree("") }),
        ]);
        assert!(frame_tree(&mut cdp, "S").unwrap_err().starts_with("slicc: refused:"));
    }

    #[test]
    fn a_child_that_never_commits_stays_pending() {
        let (mut cdp, _) = scripted(vec![]);
        let mut frames = vec![
            FrameRec { frame_id: "M".into(), parent_frame_id: None, url: String::new(), target: None },
            FrameRec { frame_id: "C".into(), parent_frame_id: Some("M".into()), url: String::new(), target: None },
        ];
        let started = Instant::now();
        await_child_commits(&mut cdp, "S", 0, &mut frames, Duration::from_millis(50)).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(frames[1].url, "");
        assert_eq!(frame_display_url(&frames[1].url), "(navigating)");
    }

    #[test]
    fn frame_tree_surfaces_a_failed_tree_read() {
        let (mut cdp, _) = scripted(vec![
            json!({ "id": 1, "result": {} }),
            json!({ "id": 2, "error": { "message": "No frame" } }),
        ]);
        assert!(frame_tree(&mut cdp, "S").is_err());
    }

    #[test]
    fn only_a_matching_committed_navigation_counts() {
        assert_eq!(committed_frame_url(&navigated("S", "C", "http://a/x"), "S", "C").as_deref(), Some("http://a/x"));
        assert_eq!(committed_frame_url(&navigated("S", "C", ""), "S", "C"), None);
        assert_eq!(committed_frame_url(&navigated("S", "D", "http://a/x"), "S", "C"), None);
        assert_eq!(committed_frame_url(&navigated("T", "C", "http://a/x"), "S", "C"), None);
        assert_eq!(committed_frame_url(&json!({ "method": "Page.loadEventFired", "sessionId": "S" }), "S", "C"), None);
        assert_eq!(committed_frame_url(&json!({ "method": "Page.frameNavigated", "sessionId": "S" }), "S", "C"), None);
    }

    #[test]
    fn a_committed_frame_url_is_displayed() {
        assert_eq!(frame_display_url("about:blank"), "about:blank");
    }

    #[test]
    fn page_enable_ignores_only_a_cdp_error_response() {
        assert!(cdp_error_response("Page.enable", "Page.enable: 'Page.enable' wasn't found\n"));
        assert!(!cdp_error_response("Page.enable", "CDP response was not JSON: expected value\n"));
        assert!(!cdp_error_response("Page.enable", "websocket connection closed\n"));
        assert!(!cdp_error_response("Page.enable", "websocket write failed: broken pipe\n"));
        assert!(!cdp_error_response("Page.enable", "websocket read failed: reset\n"));
        assert!(!cdp_error_response("Page.enable", "Page.enable timed out\n"));
    }

    #[test]
    fn a_fragment_or_download_does_not_commit_a_document() {
        assert!(navigation_commits(&json!({ "loaderId": "ABC", "frameId": "F" })));
        assert!(!navigation_commits(&json!({ "loaderId": "", "frameId": "F" })));
        assert!(!navigation_commits(&json!({ "frameId": "F" })));
        assert!(!navigation_commits(
            &json!({ "loaderId": "ABC", "isDownload": true })
        ));
    }

    #[test]
    fn validate_upload_rejects_directory() {
        let dir = std::env::temp_dir().join(format!("upload-dir-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = validate_upload_path(dir.to_str().unwrap());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("directory"));
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn validate_upload_rejects_missing_file() {
        let result = validate_upload_path("/tmp/nonexistent-upload-test-file-xyz");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[cfg(unix)]
    #[test]
    fn validate_upload_rejects_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("upload-unreadable-{}", std::process::id()));
        std::fs::write(&path, b"secret").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = validate_upload_path(path.to_str().unwrap());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cannot read"));
    }

    #[test]
    fn validate_upload_accepts_regular_file() {
        let path = std::env::temp_dir().join(format!("upload-valid-{}", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        assert!(validate_upload_path(path.to_str().unwrap()).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn mime_from_name_common_types() {
        assert_eq!(mime_from_name("photo.png"), "image/png");
        assert_eq!(mime_from_name("page.html"), "text/html");
        assert_eq!(mime_from_name("data.json"), "application/json");
        assert_eq!(mime_from_name("archive.zip"), "application/zip");
        assert_eq!(mime_from_name("unknown.xyz"), "application/octet-stream");
        assert_eq!(mime_from_name("noext"), "application/octet-stream");
    }

    #[test]
    fn upload_entry_builds_name_type_base64() {
        let entry = upload_entry("/home/user/photo.png", b"PNG-DATA").unwrap();
        assert_eq!(entry.name, "photo.png");
        assert_eq!(entry.mime, "image/png");
        let decoded = crate::net::base64_decode(&entry.base64).unwrap();
        assert_eq!(decoded, b"PNG-DATA");
    }

    #[test]
    fn upload_entry_extracts_filename_from_path() {
        let entry = upload_entry("/a/b/c/report.csv", b"col1,col2").unwrap();
        assert_eq!(entry.name, "report.csv");
        assert_eq!(entry.mime, "text/csv");
    }

    #[test]
    fn upload_entry_rejects_oversized_data() {
        let big = vec![0u8; MAX_UPLOAD_BYTES + 1];
        let result = upload_entry("huge.bin", &big);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("25 MiB"), "{err}");
        assert!(err.contains("huge.bin"), "{err}");
    }

    #[test]
    fn upload_entry_accepts_exactly_max_size() {
        let data = vec![0u8; MAX_UPLOAD_BYTES];
        assert!(upload_entry("exact.bin", &data).is_ok());
    }

    #[test]
    fn upload_call_args_single_file() {
        let entry = upload_entry("hello.txt", b"world").unwrap();
        let (func, args) = upload_call_args(&[entry]);
        assert!(func.contains("DataTransfer"));
        assert!(func.contains("new File"));
        assert!(func.contains("dispatchEvent"));
        assert!(func.contains("files[0]"));
        assert!(!func.contains("files[1]"));
        assert_eq!(args.len(), 3);
        assert_eq!(args[0]["value"], "hello.txt");
        assert_eq!(args[1]["value"], "text/plain");
        let decoded = crate::net::base64_decode(args[2]["value"].as_str().unwrap()).unwrap();
        assert_eq!(decoded, b"world");
    }

    #[test]
    fn upload_call_args_multiple_files() {
        let e1 = upload_entry("a.png", b"img1").unwrap();
        let e2 = upload_entry("b.json", b"{}").unwrap();
        let (func, args) = upload_call_args(&[e1, e2]);
        assert!(func.contains("files[0]"));
        assert!(func.contains("files[1]"));
        assert_eq!(args.len(), 6);
        assert_eq!(args[0]["value"], "a.png");
        assert_eq!(args[1]["value"], "image/png");
        assert_eq!(args[3]["value"], "b.json");
        assert_eq!(args[4]["value"], "application/json");
    }
}
