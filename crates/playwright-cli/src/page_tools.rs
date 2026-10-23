use crate::action::Report;
use crate::args::{Invocation, Output};
use crate::browser;
use crate::cdp::{self, Cdp};
use crate::commands;
use crate::sanitize;
use crate::session::{self, Session};
use crate::snapshot::{FrameRec, RefRec};
use serde_json::{json, Value};

const INJECTED: &str = include_str!("../vendor/playwright-injected.js");
const ENTER_FRAME: &str = " >> internal:control=enter-frame >> ";
const MAX_SCRIPT: u64 = 16 * 1024 * 1024;
const HANDOFF_REL: &str = "https://www.sliccy.ai/rel/handoff";
const UPSKILL_REL: &str = "https://www.sliccy.ai/rel/upskill";

pub fn is_command(command: &str) -> bool {
    matches!(command, "eval-file" | "fetch" | "generate-locator" | "highlight")
}

pub fn precheck(command: &str, invocation: &Invocation) -> Option<Output> {
    let first = invocation.positionals.first();
    match command {
        "generate-locator" if first.is_none() => Some(Output::err(
            "error: 'target' argument: expected string, received undefined\n",
        )),
        "highlight" if first.is_none() && !hide(invocation) => Some(Output {
            stdout: "### Error\nInvalid arguments for tool \"browser_highlight\":\n\u{2716} Invalid input: expected string, received undefined\n  \u{2192} at target\n".to_string(),
            stderr: String::new(),
            code: 1,
        }),
        "eval-file" if first.is_none() => Some(Output::err("eval-file requires a file path\n")),
        "eval-file" if invocation.flags.contains_key("filename") && invocation.flags.contains_key("output") => Some(Output::err(
            "eval-file: --filename and --output are aliases \u{2014} pass one, not both\n",
        )),
        "fetch" => fetch_precheck(invocation).err().map(Output::err),
        _ => None,
    }
}

pub fn run(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation, report: &mut Report) -> Output {
    match invocation.command.as_deref().unwrap_or("") {
        "eval-file" => eval_file(cdp, state, invocation, report),
        "fetch" => fetch(cdp, state, invocation),
        "generate-locator" => match with_element(cdp, state, invocation, |cdp, page, scope, rec| locate(cdp, page, scope, rec).map(|(_, locator)| locator)) {
            Ok(locator) => Output::ok(format!("### Result\n{}\n", sanitize::terminal(&locator))),
            Err(message) => Output::err(message),
        },
        _ => highlight(cdp, state, invocation),
    }
}

pub fn network_state_set(invocation: &Invocation) -> Output {
    let offline = match invocation.positionals.first().map(String::as_str) {
        Some("offline") => true,
        Some("online") => false,
        _ => {
            return Output::err(
                "error: 'state' argument: Invalid option: expected one of \"online\"|\"offline\"\n",
            )
        }
    };
    let mut state = session::load_in(invocation.session.as_deref(), invocation.cwd.as_deref());
    state.offline = offline;
    if let Err(message) = session::save(&state) {
        return Output::err(message);
    }
    let name = if offline { "offline" } else { "online" };
    Output::ok(format!(
        "### Result\nNetwork is now {name}\n### Ran Playwright code\n```js\nawait page.context().setOffline({offline});\n```\n"
    ))
}

pub fn arm_network(cdp: &mut Cdp, session: &str, state: &Session) -> Result<(), String> {
    if !state.offline {
        return Ok(());
    }
    cdp.call(
        "Network.emulateNetworkConditions",
        json!({ "offline": true, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1 }),
        Some(session),
    )?;
    Ok(())
}

fn hide(invocation: &Invocation) -> bool {
    matches!(invocation.flags.get("hide").map(String::as_str), Some("true") | Some(""))
}

fn eval_file(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation, report: &mut Report) -> Output {
    let path = &invocation.positionals[0];
    let source = match read_script(path) {
        Ok(source) => source,
        Err(message) => return Output::err(message),
    };
    let text = match commands::evaluate_source(cdp, state, invocation, report, &source) {
        Ok(text) => text,
        Err(message) => return Output::err(message),
    };
    let Some(output) = invocation.flags.get("output").or_else(|| invocation.flags.get("filename")) else {
        return Output::ok(format!("{}\n", sanitize::browser_message(&text)));
    };
    match commands::write_text(output, &text) {
        Ok(()) => Output::ok(format!(
            "Result saved to {output} ({} KB)\n",
            (text.len() + 512) / 1024
        )),
        Err(message) => Output::err(message),
    }
}

fn read_script(path: &str) -> Result<String, String> {
    let shown = sanitize::terminal(path);
    let meta = std::fs::metadata(path).map_err(|err| format!("eval-file: cannot read {shown}: {err}\n"))?;
    if !meta.is_file() {
        return Err(format!("eval-file: cannot read {shown}: not a file\n"));
    }
    if meta.len() > MAX_SCRIPT {
        return Err(format!("eval-file: cannot read {shown}: larger than 16 MiB\n"));
    }
    std::fs::read_to_string(path).map_err(|err| format!("eval-file: cannot read {shown}: {err}\n"))
}

fn fetch_precheck(invocation: &Invocation) -> Result<(), String> {
    let Some(url) = invocation.positionals.first() else {
        return Err("fetch requires a URL\n".to_string());
    };
    if let Some(method) = invocation.flags.get("method") {
        if method.is_empty() || !method.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)) {
            return Err("fetch: --method must be an HTTP method token\n".to_string());
        }
    }
    let guard = cdp_client::protected::Protected::default();
    let destination = url::Url::parse("https://relative.invalid/")
        .ok()
        .and_then(|base| base.join(url).ok());
    let Some(destination) = destination.filter(|parsed| matches!(parsed.scheme(), "http" | "https")) else {
        return Err("fetch: only http and https URLs can be fetched\n".to_string());
    };
    if guard.url(destination.as_str()) {
        return Err(format!("{}\n", guard.refusal_for_url(destination.as_str())));
    }
    Ok(())
}

fn fetch(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let raw = &invocation.positionals[0];
    let method = invocation.flags.get("method").map(String::as_str).unwrap_or("GET");
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match commands::resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = fetch_in(cdp, &session, raw, method);
    let watched = browser::watch_detach(cdp, state, &target, &session);
    let (url, outcome) = match result {
        Ok(found) => found,
        Err(message) => return Output::err(message),
    };
    if let Err(message) = watched {
        return Output::err(message);
    }
    state.current = Some(target);
    let shown = sanitize::display_url(&url);
    let (payload, code) = match outcome {
        Ok(value) => (fetched(cdp, &url, &shown, &value), 0),
        Err(error) => (
            json!({ "url": shown, "links": [], "handoff": null, "error": error.trim_end() }),
            1,
        ),
    };
    let text = sanitize::json_c1(&serde_json::to_string_pretty(&payload).unwrap_or_default());
    Output { stdout: format!("{text}\n"), stderr: String::new(), code }
}

type Fetched = (String, Result<Value, String>);

fn fetch_in(cdp: &mut Cdp, session: &str, raw: &str, method: &str) -> Result<Fetched, String> {
    let resolve = format!("new URL({}, document.baseURI).href", Value::from(raw));
    let resolved = browser::evaluate(cdp, session, &resolve, false)?;
    let url = resolved
        .value
        .as_str()
        .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
        .ok_or_else(|| "fetch: only http and https URLs can be fetched\n".to_string())?
        .to_string();
    if cdp.guard().url(&url) {
        return Err(format!("{}\n", cdp.guard().refusal_for_url(&url)));
    }
    let script = format!(
        "(async () => {{ const r = await fetch({}, {{ method: {}, credentials: 'include', redirect: 'follow' }}); try {{ await r.body?.cancel(); }} catch {{}} return {{ status: r.status, url: r.url, redirected: r.redirected, link: r.headers.get('link') }}; }})()",
        Value::from(url.as_str()),
        Value::from(method)
    );
    let outcome = browser::evaluate(cdp, session, &script, true).map(|value| value.value);
    match outcome {
        Err(message) if message.starts_with("slicc: refused:") || cdp::transport_error(&message) => Err(message),
        other => Ok((url, other)),
    }
}

fn fetched(cdp: &Cdp, url: &str, shown: &str, value: &Value) -> Value {
    let guard = cdp.guard();
    let final_url = value.get("url").and_then(Value::as_str).filter(|text| !text.is_empty()).unwrap_or(url);
    let base = url::Url::parse(final_url).or_else(|_| url::Url::parse(url));
    let links = match (value.get("link").and_then(Value::as_str), base) {
        (Some(header), Ok(base)) => discovery::parse_link_header(header, &base),
        _ => Vec::new(),
    };
    let links: Vec<discovery::Link> = links
        .into_iter()
        .filter(|link| !guard.url(&link.href) && !guard.url_read_only(&link.href))
        .collect();
    let handoff = links.iter().find_map(|link| {
        let verb = match link.rel.as_str() {
            HANDOFF_REL => "handoff",
            UPSKILL_REL => "upskill",
            _ => return None,
        };
        let mut found = json!({ "verb": verb, "target": sanitize::display_url(&link.href) });
        if let Some(title) = link.title.as_deref().filter(|title| !title.is_empty()) {
            found["instruction"] = json!(sanitize::terminal(title));
        }
        Some(found)
    });
    let listed: Vec<Value> = links
        .iter()
        .map(|link| {
            let mut entry = json!({ "href": sanitize::display_url(&link.href), "rel": link.rel });
            if let Some(kind) = &link.media_type {
                entry["type"] = json!(kind);
            }
            if let Some(title) = &link.title {
                entry["title"] = json!(title);
            }
            if !link.params.is_empty() {
                entry["params"] = json!(link.params);
            }
            entry
        })
        .collect();
    json!({
        "url": shown,
        "status": value.get("status").cloned().unwrap_or(Value::Null),
        "redirected": value.get("redirected").and_then(Value::as_bool).unwrap_or(false),
        "links": listed,
        "handoff": handoff,
    })
}

fn with_element<T>(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    body: impl FnOnce(&mut Cdp, &str, &str, &RefRec) -> Result<T, String>,
) -> Result<T, String> {
    let id = invocation.positionals[0].as_str();
    let pages = browser::list_pages(cdp)?;
    let target = commands::resolve_target(cdp, invocation, state, &pages, false)?;
    let refs = state
        .snapshots
        .get(&target)
        .cloned()
        .ok_or_else(|| "No snapshot available. Run \"snapshot\" first.\n".to_string())?;
    let rec = browser::lookup_ref(&refs, id)?.clone();
    let session = browser::watch_attach(cdp, state, &target)?;
    let result = browser::ref_session(cdp, &session, &rec).and_then(|(rec, child)| {
        let result = body(cdp, &session, child.as_deref().unwrap_or(&session), &rec);
        if let Some(child) = child {
            cdp::detach(cdp, &child);
        }
        result
    });
    let watched = browser::watch_detach(cdp, state, &target, &session);
    let value = result?;
    watched?;
    state.current = Some(target);
    Ok(value)
}

fn main_frame(frames: &[FrameRec]) -> Result<String, String> {
    frames
        .iter()
        .find(|frame| frame.parent_frame_id.is_none())
        .map(|frame| frame.frame_id.clone())
        .ok_or_else(|| "the tab has no main frame\n".to_string())
}

fn backend_of(rec: &RefRec) -> Result<u64, String> {
    rec.backend_node_id.ok_or_else(|| "element has no backend node\n".to_string())
}

fn element_object(cdp: &mut Cdp, session: &str, frame_id: &str, backend: u64) -> Result<String, String> {
    let _ = cdp.call("DOM.enable", json!({}), Some(session));
    let world = cdp.call(
        "Page.createIsolatedWorld",
        json!({ "frameId": frame_id, "grantUniveralAccess": true }),
        Some(session),
    )?;
    let context = world
        .get("executionContextId")
        .cloned()
        .ok_or_else(|| "Page.createIsolatedWorld returned no executionContextId\n".to_string())?;
    let resolved = cdp.call(
        "DOM.resolveNode",
        json!({ "backendNodeId": backend, "executionContextId": context }),
        Some(session),
    )?;
    resolved
        .pointer("/object/objectId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("could not resolve backend node {backend}\n"))
}

fn call_on(cdp: &mut Cdp, session: &str, object: &str, declaration: &str, arguments: Value) -> Result<Value, String> {
    let result = cdp.call(
        "Runtime.callFunctionOn",
        json!({
            "objectId": object,
            "functionDeclaration": declaration,
            "arguments": arguments,
            "returnByValue": true,
        }),
        Some(session),
    )?;
    if let Some(details) = result.get("exceptionDetails") {
        let text = details
            .pointer("/exception/description")
            .and_then(Value::as_str)
            .or_else(|| details.get("text").and_then(Value::as_str))
            .unwrap_or("evaluation failed");
        return Err(format!("{}\n", sanitize::browser_message(text)));
    }
    Ok(result.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

fn generator() -> String {
    format!(
        "function(rest) {{ {INJECTED}; const injected = new sliccInjected.sliccInjectedScriptClass(window, {{ isUnderTest: false, sdkLanguage: 'javascript', testIdAttributeName: 'data-testid', stableRafCount: 1, browserName: 'chromium', shouldPrependErrorPrefix: false, isUtilityWorld: true, customEngines: [] }}); const own = injected.generateSelectorSimple(this); const selector = rest ? own + {} + rest : own; return [selector, injected.utils.asLocator('javascript', selector)]; }}",
        Value::from(ENTER_FRAME)
    )
}

fn generate(cdp: &mut Cdp, session: &str, frame_id: &str, backend: u64, rest: Option<&str>) -> Result<(String, String), String> {
    let object = element_object(cdp, session, frame_id, backend)?;
    let value = call_on(cdp, session, &object, &generator(), json!([{ "value": rest }]))?;
    let part = |index: usize| value.get(index).and_then(Value::as_str).filter(|text| !text.is_empty()).map(str::to_string);
    match (part(0), part(1)) {
        (Some(selector), Some(locator)) => Ok((selector, locator)),
        _ => Err("Unable to generate locator for this element\n".to_string()),
    }
}

fn frame_owner(cdp: &mut Cdp, session: &str, frame_id: &str) -> Result<u64, String> {
    let owner = cdp.call("DOM.getFrameOwner", json!({ "frameId": frame_id }), Some(session))?;
    owner
        .get("backendNodeId")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Unable to generate locator for this frame\n".to_string())
}

fn unsupported_frame() -> String {
    "generate-locator: frames inside out-of-process frames are not supported\n".to_string()
}

fn climb(cdp: &mut Cdp, session: &str, frames: &[FrameRec], mut frame: String, mut found: (String, String)) -> Result<(String, String), String> {
    loop {
        let here = frames.iter().find(|item| item.frame_id == frame).ok_or_else(unsupported_frame)?;
        let Some(parent) = here.parent_frame_id.clone() else {
            return Ok(found);
        };
        let above = frames.iter().find(|item| item.frame_id == parent).ok_or_else(unsupported_frame)?;
        if above.target.is_some() || here.target.is_some() {
            return Err(unsupported_frame());
        }
        let owner = frame_owner(cdp, session, &frame)?;
        found = generate(cdp, session, &parent, owner, Some(&found.0))?;
        frame = parent;
    }
}

fn locate(cdp: &mut Cdp, page: &str, scope: &str, rec: &RefRec) -> Result<(String, String), String> {
    let backend = backend_of(rec)?;
    let Some(remote) = rec.target.clone() else {
        let frames = browser::page_frames(cdp, page)?;
        let frame = match rec.frame_id.clone() {
            Some(frame) => frame,
            None => main_frame(&frames)?,
        };
        let found = generate(cdp, page, &frame, backend, None)?;
        return climb(cdp, page, &frames, frame, found);
    };
    let inner = browser::frame_tree(cdp, scope)?;
    let frame = rec.frame_id.clone().unwrap_or_else(|| remote.clone());
    let found = generate(cdp, scope, &frame, backend, None)?;
    let found = climb(cdp, scope, &inner, frame, found)?;
    let frames = browser::page_frames(cdp, page)?;
    let root = frames.iter().find(|item| item.frame_id == remote && item.target.is_some()).ok_or_else(unsupported_frame)?;
    let parent = root.parent_frame_id.clone().ok_or_else(unsupported_frame)?;
    if frames.iter().find(|item| item.frame_id == parent).is_none_or(|item| item.target.is_some()) {
        return Err(unsupported_frame());
    }
    let owner = rec.owner.ok_or_else(unsupported_frame)?;
    let found = generate(cdp, page, &parent, owner, Some(&found.0))?;
    climb(cdp, page, &frames, parent, found)
}

fn element_frame(cdp: &mut Cdp, page: &str, rec: &RefRec) -> Result<String, String> {
    if let Some(frame) = rec.frame_id.clone().or_else(|| rec.target.clone()) {
        return Ok(frame);
    }
    let frames = browser::frame_tree(cdp, page)?;
    main_frame(&frames)
}

const SHOW: &str = "function(style, key) { const element = this; const doc = element.ownerDocument; const root = doc.documentElement; if (!root || !element.isConnected) throw new Error('Element is not attached to the DOM'); let host = Array.from(root.children).find((node) => node.localName === 'x-slicc-highlight'); if (!host) { host = doc.createElement('x-slicc-highlight'); host.setAttribute('popover', 'manual'); host.style.cssText = 'position:fixed;inset:0;width:100%;height:100%;max-width:none;max-height:none;margin:0;padding:0;border:none;overflow:visible;pointer-events:none;background:transparent;display:block;z-index:2147483647'; host.attachShadow({ mode: 'open' }); root.appendChild(host); } try { host.hidePopover(); host.showPopover(); } catch {} for (const old of Array.from(host.shadowRoot.children)) if (old.getAttribute('data-key') === key) old.remove(); const box = doc.createElement('x-slicc-box'); box.setAttribute('data-key', key); box.style.cssText = 'position:fixed;box-sizing:border-box;pointer-events:none;background-color:#6fa8dc7f;' + (style ? ';' + style : ''); host.shadowRoot.appendChild(box); const place = () => { if (!box.isConnected) return; if (!element.isConnected) { box.remove(); return; } const rect = element.getBoundingClientRect(); box.style.left = rect.left + 'px'; box.style.top = rect.top + 'px'; box.style.width = rect.width + 'px'; box.style.height = rect.height + 'px'; requestAnimationFrame(place); }; place(); }";

const HIDE: &str = "function(key) { const root = this.ownerDocument.documentElement; if (!root) return; for (const host of Array.from(root.children).filter((node) => node.localName === 'x-slicc-highlight')) { if (!host.shadowRoot) continue; for (const box of Array.from(host.shadowRoot.children)) if (box.getAttribute('data-key') === key) box.remove(); if (!host.shadowRoot.children.length) host.remove(); } }";

const HIDE_ALL: &str = "(() => { const root = document.documentElement; if (!root) return; for (const host of Array.from(root.children).filter((node) => node.localName === 'x-slicc-highlight')) host.remove(); })()";

fn highlight(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let hiding = hide(invocation);
    if invocation.positionals.is_empty() {
        return match hide_all(cdp, state, invocation) {
            Ok(()) => Output::ok("### Result\nHid page highlight\n"),
            Err(message) => Output::err(message),
        };
    }
    let style = invocation.flags.get("style").cloned().unwrap_or_default();
    let shown = format!("locator('aria-ref={}')", sanitize::terminal(&invocation.positionals[0]));
    let result = with_element(cdp, state, invocation, |cdp, page, scope, rec| {
        let backend = backend_of(rec)?;
        let frame = element_frame(cdp, page, rec)?;
        let object = element_object(cdp, scope, &frame, backend)?;
        let key = backend.to_string();
        if hiding {
            call_on(cdp, scope, &object, HIDE, json!([{ "value": key }]))
        } else {
            call_on(cdp, scope, &object, SHOW, json!([{ "value": style }, { "value": key }]))
        }
    });
    match result {
        Ok(_) if hiding => Output::ok(format!("### Result\nHid highlight for {shown}\n")),
        Ok(_) => Output::ok(format!("### Result\nHighlighted {shown}\n")),
        Err(message) => Output::err(message),
    }
}

fn hide_all(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Result<(), String> {
    let pages = browser::list_pages(cdp)?;
    let target = commands::resolve_target(cdp, invocation, state, &pages, false)?;
    let session = browser::watch_attach(cdp, state, &target)?;
    let result = (|| {
        let frames = browser::page_frames(cdp, &session)?;
        for frame in &frames {
            if cdp.guard().url(&frame.url) || frame.url.is_empty() {
                continue;
            }
            let cleared = if frame.target.is_some() {
                browser::open_frame(cdp, frame).and_then(|(child, _)| {
                    let cleared = clear_frame(cdp, &child, &frame.frame_id);
                    cdp::detach(cdp, &child);
                    cleared
                })
            } else {
                clear_frame(cdp, &session, &frame.frame_id)
            };
            match cleared {
                Err(message) if frame.parent_frame_id.is_none() || !message.starts_with("slicc: refused:") => return Err(message),
                _ => {}
            }
        }
        Ok(())
    })();
    let watched = browser::watch_detach(cdp, state, &target, &session);
    result?;
    watched?;
    state.current = Some(target);
    Ok(())
}

fn clear_frame(cdp: &mut Cdp, session: &str, frame_id: &str) -> Result<(), String> {
    let world = cdp.call(
        "Page.createIsolatedWorld",
        json!({ "frameId": frame_id, "grantUniveralAccess": true }),
        Some(session),
    )?;
    let context = world
        .get("executionContextId")
        .cloned()
        .ok_or_else(|| "Page.createIsolatedWorld returned no executionContextId\n".to_string())?;
    let result = cdp.call(
        "Runtime.evaluate",
        json!({ "expression": HIDE_ALL, "contextId": context, "returnByValue": true }),
        Some(session),
    )?;
    if result.get("exceptionDetails").is_some() {
        return Err("highlight: could not remove the page highlight\n".to_string());
    }
    Ok(())
}
