use crate::args::{Invocation, Output};
use crate::browser::{self, PageInfo};
use crate::cdp::{self, Cdp};
use crate::connect::{self, Start};
use crate::session::{self, Session};
use crate::snapshot::{self, RefRec};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run_command(invocation: &Invocation, env_url: Option<&str>) -> Output {
    let start = match connect::choose_start(invocation.cdp.as_deref(), env_url) {
        Ok(start) => start,
        Err(message) => return Output::err(message),
    };
    let runtime = invocation.runtime.clone().or_else(|| {
        invocation
            .flags
            .get("runtime")
            .filter(|value| !value.is_empty())
            .cloned()
    });
    if let Some(message) = deferred_flag(invocation) {
        return Output::err(message);
    }
    let connected = match open_cdp(start, runtime.as_deref()) {
        Ok(cdp) => cdp,
        Err(message) => return Output::err(message),
    };
    let mut cdp = connected;
    let mut state = session::load();
    let command = invocation.command.as_deref().unwrap_or("");
    let result = match command {
        "open" | "tab-new" => cmd_open(&mut cdp, &mut state, invocation),
        "close" | "tab-close" => cmd_close(&mut cdp, &mut state, invocation),
        "goto" => cmd_goto(&mut cdp, &mut state, invocation),
        "snapshot" => cmd_snapshot(&mut cdp, &mut state, invocation),
        "click" => cmd_click(&mut cdp, &mut state, invocation),
        "fill" => cmd_fill(&mut cdp, &mut state, invocation),
        "type" => cmd_type(&mut cdp, &mut state, invocation),
        "press" => cmd_press(&mut cdp, &mut state, invocation),
        "screenshot" => cmd_screenshot(&mut cdp, &mut state, invocation),
        "eval" => cmd_eval(&mut cdp, &mut state, invocation),
        "tab-list" => cmd_tab_list(&mut cdp, &state),
        "tab-select" => cmd_tab_select(&mut cdp, &mut state, invocation),
        _ => Output::err(format!("playwright-cli {command}: not implemented yet\n")),
    };
    if let Err(message) = session::save(&state) {
        if result.code == 0 {
            return Output::err(message);
        }
    }
    result
}

fn deferred_flag(invocation: &Invocation) -> Option<String> {
    for name in [
        "discover",
        "teleport-start",
        "teleport-return",
        "timeout",
        "teleport-runtime",
    ] {
        if invocation.flags.contains_key(name) {
            let command = invocation.command.as_deref().unwrap_or("playwright-cli");
            return Some(format!(
                "playwright-cli {command}: --{name} is not implemented yet\n"
            ));
        }
    }
    None
}

fn open_cdp(start: Start, runtime: Option<&str>) -> Result<Cdp, String> {
    let socket = match start {
        Start::Direct(url) => url,
        Start::Discover(url) => {
            let response = match crate::net::http_get(&url) {
                Ok(response) => response,
                Err(message) => {
                    if message.starts_with("HTTP ") || message.contains("does not launch Chrome") {
                        return Err(message);
                    }
                    return Err(format!(
                        "{message}This CLI does not launch Chrome. Pass --cdp or set SLICC_CDP_URL.\n"
                    ));
                }
            };
            let body = String::from_utf8_lossy(&response.body).to_string();
            connect::websocket_url_from_version(&body)?
        }
    };
    let socket = connect::append_runtime(&socket, runtime);
    Cdp::connect(&socket)
}

fn cmd_open(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let url = invocation
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or("about:blank");
    let foreground = flag_true(&invocation.flags, "foreground") || flag_true(&invocation.flags, "fg");
    match browser::create_target(cdp, url, foreground) {
        Ok(target_id) => {
            state.current = Some(target_id.clone());
            Output::ok(format!("Opened {url} in new tab [targetId: {target_id}]\n"))
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_close(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    match browser::close_target(cdp, &target) {
        Ok(()) => {
            state.snapshots.remove(&target);
            if state.current.as_deref() == Some(target.as_str()) {
                state.current = None;
            }
            Output::ok(format!("Closed tab {target}\n"))
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_goto(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let Some(url) = invocation.positionals.first() else {
        return Output::err("goto requires a URL\n");
    };
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, true) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let existed = pages.iter().any(|page| page.target_id == target);
    let result = if existed {
        browser::navigate(cdp, &target, url)
    } else {
        Ok(())
    };
    match result {
        Ok(()) => {
            state.current = Some(target);
            state.snapshots.clear();
            Output::ok(format!("Navigated to {url}\n"))
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_tab_list(cdp: &mut Cdp, state: &Session) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    if pages.is_empty() {
        return Output::ok("No tabs open\n");
    }
    let mut lines = Vec::new();
    for (index, page) in pages.iter().enumerate() {
        let active = if state.current.as_deref() == Some(page.target_id.as_str()) {
            " (active)"
        } else {
            ""
        };
        lines.push(format!(
            "{}: [{}] {} \"{}\"{active}",
            index + 1,
            page.target_id,
            page.url,
            page.title
        ));
    }
    Output::ok(format!("{}\n", lines.join("\n")))
}

fn cmd_tab_select(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let Some(index_text) = invocation.positionals.first() else {
        return Output::err("tab-select requires a tab index\n");
    };
    if index_text.parse::<usize>().is_err() || index_text.starts_with('0') && index_text != "0" {
        if index_text.parse::<usize>().is_err() {
            return Output::err("tab-select index must be a positive integer\n");
        }
    }
    let Ok(index) = index_text.parse::<usize>() else {
        return Output::err("tab-select index must be a positive integer\n");
    };
    if index == 0 {
        return Output::err("tab-select index must be a positive integer\n");
    }
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let Some(page) = pages.get(index - 1) else {
        let noun = if pages.len() == 1 { "tab" } else { "tabs" };
        return Output::err(format!(
            "tab-select index {index} out of range ({} {noun} open)\n",
            pages.len()
        ));
    };
    match browser::activate(cdp, &page.target_id) {
        Ok(()) => {
            state.current = Some(page.target_id.clone());
            Output::ok(format!(
                "Selected tab {index} [targetId: {}]\n",
                page.target_id
            ))
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_snapshot(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match cdp::attach(cdp, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let built = build_snapshot(cdp, &session, invocation);
    cdp::detach(cdp, &session);
    let (text, refs) = match built {
        Ok(value) => value,
        Err(message) => return Output::err(message),
    };
    state.current = Some(target.clone());
    state.snapshots.insert(target, refs);
    if let Some(filename) = invocation.flags.get("filename") {
        return match write_text(filename, &text) {
            Ok(()) => Output::ok(format!("Snapshot saved to {filename}\n")),
            Err(message) => Output::err(message),
        };
    }
    Output::ok(format!("{text}\n"))
}

fn build_snapshot(
    cdp: &mut Cdp,
    session: &str,
    invocation: &Invocation,
) -> Result<(String, BTreeMap<String, RefRec>), String> {
    let (url, title) = browser::page_location(cdp, session)?;
    if let Some(frame_id) = invocation.flags.get("frame") {
        let frames = browser::frame_tree(cdp, session)?;
        if !frames.iter().any(|frame| frame.frame_id == *frame_id) {
            return Err(format!(
                "Unknown frame ID \"{frame_id}\" for tab {}. Run 'playwright-cli frames --tab={}' to list frame IDs.\n",
                invocation.flags.get("tab").map(String::as_str).unwrap_or("<targetId>"),
                invocation.flags.get("tab").map(String::as_str).unwrap_or("<targetId>")
            ));
        }
        let nodes = browser::accessibility_tree(cdp, session, Some(frame_id))?;
        let root = single_root(nodes);
        let (body, mut refs) = snapshot::render_tree(&root, "f1");
        snapshot::tag_frame_refs(&mut refs, frame_id, "f1");
        let text = format!("Page URL: {url}\nPage Title: {title}\n\n{body}");
        return Ok((text, refs));
    }
    let nodes = browser::accessibility_tree(cdp, session, None)?;
    let root = single_root(nodes);
    let (body, mut refs) = snapshot::render_tree(&root, "");
    let body = if flag_true(&invocation.flags, "no-iframes") {
        body
    } else {
        let frames = browser::frame_tree(cdp, session).unwrap_or_default();
        let (stitched, assigned) = snapshot::stitch_iframes(&body, &url, &frames, |frame, prefix| {
            let nodes = browser::accessibility_tree(cdp, session, Some(&frame.frame_id)).ok()?;
            let root = single_root(nodes);
            let (text, frame_refs) = snapshot::render_tree(&root, prefix);
            for (id, mut rec) in frame_refs {
                rec.frame_id = Some(frame.frame_id.clone());
                refs.insert(id, rec);
            }
            Some(text)
        });
        let _ = assigned;
        stitched
    };
    let text = format!("Page URL: {url}\nPage Title: {title}\n\n{body}");
    Ok((text, refs))
}

fn single_root(mut nodes: Vec<crate::snapshot::AxNode>) -> crate::snapshot::AxNode {
    if nodes.len() == 1 {
        return nodes.pop().unwrap();
    }
    crate::snapshot::AxNode {
        role: "RootWebArea".to_string(),
        name: String::new(),
        value: String::new(),
        backend_node_id: None,
        children: nodes,
    }
}

fn cmd_click(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let Some(id) = invocation.positionals.first() else {
        return Output::err("click requires a ref (e.g. e5)\n");
    };
    let button = invocation.positionals.get(1).map(String::as_str).unwrap_or("left");
    if !matches!(button, "left" | "right" | "middle") {
        return Output::err(format!("unknown button \"{button}\"\n"));
    }
    let modifiers = modifiers_of(invocation.flags.get("modifiers").map(String::as_str));
    with_ref(cdp, state, invocation, id, |cdp, session, rec| {
        browser::click_ref(cdp, session, rec, button, modifiers)?;
        Ok(format!("Clicked {id}\n"))
    })
}

fn cmd_fill(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    if invocation.positionals.len() < 2 {
        return Output::err("fill requires <ref> <text>\n");
    }
    let id = &invocation.positionals[0];
    let text = invocation.positionals[1..].join(" ");
    let submit = flag_true(&invocation.flags, "submit");
    with_ref(cdp, state, invocation, id, |cdp, session, rec| {
        browser::fill_ref(cdp, session, rec, &text)?;
        if submit {
            browser::press_key(cdp, session, "Enter")?;
        }
        Ok(format!("Filled {id} with: {text}\n"))
    })
}

fn cmd_type(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    if invocation.positionals.is_empty() {
        return Output::err("type requires text\n");
    }
    let text = invocation.positionals.join(" ");
    let submit = flag_true(&invocation.flags, "submit");
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match cdp::attach(cdp, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result: Result<(), String> = (|| {
        browser::insert_text(cdp, &session, &text)?;
        if submit {
            browser::press_key(cdp, &session, "Enter")?;
        }
        Ok(())
    })();
    cdp::detach(cdp, &session);
    match result {
        Ok(()) => {
            state.snapshots.remove(&target);
            Output::ok(format!("Typed: {text}\n"))
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_press(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let Some(key) = invocation.positionals.first() else {
        return Output::err("press requires a key name\n");
    };
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match cdp::attach(cdp, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = browser::press_key(cdp, &session, key);
    cdp::detach(cdp, &session);
    match result {
        Ok(()) => Output::ok(format!("Pressed {key}\n")),
        Err(message) => Output::err(message),
    }
}

fn cmd_screenshot(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match cdp::attach(cdp, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = capture(cdp, state, invocation, &target, &session);
    cdp::detach(cdp, &session);
    match result {
        Ok(message) => Output::ok(message),
        Err(message) => Output::err(message),
    }
}

fn capture(
    cdp: &mut Cdp,
    state: &Session,
    invocation: &Invocation,
    target: &str,
    session: &str,
) -> Result<String, String> {
    let mut clip = None;
    if let Some(id) = invocation.positionals.first() {
        let refs = state
            .snapshots
            .get(target)
            .ok_or_else(|| "No snapshot available. Run \"snapshot\" first.\n".to_string())?;
        let rec = browser::lookup_ref(refs, id)?;
        let value = browser::element_clip(cdp, session, rec)?;
        let width = value.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let height = value.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if width <= 0.0 || height <= 0.0 {
            return Err(format!(
                "screenshot: could not resolve element {id} to a visible box — the snapshot is likely stale (navigation, reload, or layout change). Re-run \"snapshot\" and retry with a fresh ref, or omit the ref to capture the viewport deliberately.\n"
            ));
        }
        clip = Some(value);
    }
    let full = flag_true(&invocation.flags, "fullPage") || flag_true(&invocation.flags, "full-page");
    let mut bytes = browser::screenshot(cdp, session, full, clip)?;
    if let Some(max) = invocation.flags.get("max-width") {
        let max = max
            .parse::<u32>()
            .map_err(|_| "screenshot: --max-width must be a positive integer\n".to_string())?;
        if max == 0 {
            return Err("screenshot: --max-width must be a positive integer\n".to_string());
        }
        bytes = browser::limit_width(&bytes, max)?;
    }
    let path = invocation
        .flags
        .get("filename")
        .map(PathBuf::from)
        .unwrap_or_else(default_screenshot_path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| format!("screenshot: {err}\n"))?;
        }
    }
    fs::write(&path, &bytes).map_err(|err| format!("screenshot: {err}\n"))?;
    let kb = bytes.len().div_ceil(1024);
    Ok(format!(
        "Screenshot saved to {} ({kb} KB)\n",
        path.display()
    ))
}

fn cmd_eval(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    if invocation.positionals.is_empty() {
        return Output::err("eval requires an expression\n");
    }
    let expression = invocation.positionals.join(" ");
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match cdp::attach(cdp, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result: Result<String, String> = (|| {
        let value = if let Some(frame_id) = invocation.flags.get("frame") {
            browser::evaluate_in_frame(cdp, &session, frame_id, &expression)
                .or_else(|err| retry_await(err, &expression, |source| {
                    browser::evaluate_in_frame(cdp, &session, frame_id, source)
                }))?
        } else {
            browser::evaluate(cdp, &session, &expression, true).or_else(|err| {
                retry_await(err, &expression, |source| {
                    browser::evaluate(cdp, &session, source, true)
                })
            })?
        };
        Ok(browser::format_eval(&value))
    })();
    cdp::detach(cdp, &session);
    match result {
        Ok(text) => {
            state.current = Some(target);
            if let Some(path) = invocation
                .flags
                .get("filename")
                .or_else(|| invocation.flags.get("output"))
            {
                return match write_text(path, &text) {
                    Ok(()) => Output::ok(format!("Result saved to {path}\n")),
                    Err(message) => Output::err(message),
                };
            }
            Output::ok(format!("{text}\n"))
        }
        Err(message) => Output::err(message),
    }
}

fn retry_await(
    err: String,
    source: &str,
    mut run: impl FnMut(&str) -> Result<browser::EvalValue, String>,
) -> Result<browser::EvalValue, String> {
    if !err.contains("SyntaxError") || (!source.contains("await") && !source.contains("return")) {
        return Err(err);
    }
    let wrapped = format!("(async () => (\n{source}\n))()");
    match run(&wrapped) {
        Ok(value) => Ok(value),
        Err(next) if next.contains("SyntaxError") => {
            let wrapped = format!("(async () => {{\n{source}\n}})()");
            match run(&wrapped) {
                Ok(value) => Ok(value),
                Err(stmt) if stmt.contains("SyntaxError") => Err(err),
                Err(stmt) => Err(stmt),
            }
        }
        Err(next) => Err(next),
    }
}

fn with_ref(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    id: &str,
    body: impl FnOnce(&mut Cdp, &str, &RefRec) -> Result<String, String>,
) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let refs = match state.snapshots.get(&target) {
        Some(refs) => refs.clone(),
        None => return Output::err("No snapshot available. Run \"snapshot\" first.\n"),
    };
    let rec = match browser::lookup_ref(&refs, id) {
        Ok(rec) => rec.clone(),
        Err(message) => return Output::err(message),
    };
    let session = match cdp::attach(cdp, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = body(cdp, &session, &rec);
    cdp::detach(cdp, &session);
    match result {
        Ok(text) => {
            state.snapshots.remove(&target);
            Output::ok(text)
        }
        Err(message) => Output::err(message),
    }
}

fn resolve_target(
    cdp: &mut Cdp,
    invocation: &Invocation,
    state: &Session,
    pages: &[PageInfo],
    create_for_goto: bool,
) -> Result<String, String> {
    if let Some(tab) = invocation.flags.get("tab") {
        if pages.iter().any(|page| page.target_id == *tab) {
            return Ok(tab.clone());
        }
        return Err(format!(
            "Unknown tab \"{tab}\". Run 'playwright-cli tab-list' to get tab IDs.\n"
        ));
    }
    if let Some(current) = state.current.as_ref() {
        if pages.iter().any(|page| &page.target_id == current) {
            return Ok(current.clone());
        }
    }
    if pages.len() == 1 {
        return Ok(pages[0].target_id.clone());
    }
    if create_for_goto && pages.is_empty() {
        if let Some(url) = invocation.positionals.first() {
            return browser::create_target(cdp, url, false);
        }
    }
    Err(
        "Error: --tab <targetId> is required. Run 'playwright-cli tab-list' to get tab IDs.\n"
            .to_string(),
    )
}

fn flag_true(flags: &BTreeMap<String, String>, name: &str) -> bool {
    matches!(flags.get(name).map(String::as_str), Some("true") | Some(""))
}

fn modifiers_of(flag: Option<&str>) -> i64 {
    let Some(flag) = flag else {
        return 0;
    };
    flag.split(',').fold(0, |acc, part| {
        acc | match part.trim() {
            "Alt" => 1,
            "Control" => 2,
            "Meta" => 4,
            "Shift" => 8,
            _ => 0,
        }
    })
}

fn write_text(path: &str, text: &str) -> Result<(), String> {
    if let Some(parent) = PathBuf::from(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| format!("{path}: {err}\n"))?;
        }
    }
    fs::write(path, text).map_err(|err| format!("{path}: {err}\n"))
}

fn default_screenshot_path() -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = format!("screenshot-{millis}.png");
    #[cfg(not(target_os = "wasi"))]
    {
        PathBuf::from("/tmp").join(name)
    }
    #[cfg(target_os = "wasi")]
    {
        PathBuf::from(".playwright-cli").join(name)
    }
}
