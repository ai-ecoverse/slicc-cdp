use crate::action::{self, Report};
use crate::args::{Invocation, Output};
use crate::browser::{self, PageInfo};
use crate::cdp::{self, Cdp};
use crate::connect;
use crate::cookies;
use crate::launch::{self, Started};
use crate::sanitize;
use crate::session::{self, Session};
use crate::snapshot::{self, RefRec};
use crate::trace::{self, Piece};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn run_command(invocation: &Invocation, env_url: Option<&str>) -> Output {
    let command = invocation.command.as_deref().unwrap_or("");
    if let Some(message) = trace::precheck(command, &invocation.positionals) {
        return Output::err(message);
    }
    if let Some(message) = cookies::precheck(command, invocation) {
        return Output::err(message);
    }
    if let Some(message) = launch::reject_wasm_flags(command, &invocation.flags) {
        return Output::err(message);
    }
    if let Some(message) = deferred_flag(invocation) {
        return Output::err(message);
    }
    if matches!(command, "open" | "tab-new" | "goto" | "navigate") {
        if let Some(url) = invocation.positionals.first() {
            if cdp_client::protected::Protected::default().unparseable(url)
                && matches!(url::Url::parse(url), Err(url::ParseError::RelativeUrlWithoutBase))
            {
                return Output::err(format!("playwright-cli {command}: URL needs a scheme; add https:// or http://\n"));
            }
        }
    }
    if command == "tab-close" {
        if let Some(index) = invocation.positionals.first() {
            if close_index(index).is_none() {
                return Output::err("tab-close index must be a positive integer\n");
            }
            if invocation.flags.contains_key("tab") {
                return Output::err("tab-close takes an index or --tab, not both\n");
            }
        }
    }
    let runtime = invocation.runtime.clone().or_else(|| {
        invocation
            .flags
            .get("runtime")
            .filter(|value| !value.is_empty())
            .cloned()
    });
    let explicit = launch::endpoint_is_explicit(invocation.cdp.as_deref(), env_url);
    let profile = if explicit {
        None
    } else {
        match launch::attached_port() {
            Ok(active) => active,
            Err(message) => return Output::err(message),
        }
    };
    let remembered = if explicit || profile.is_some() {
        None
    } else {
        launch::remembered_endpoint_named(invocation.session.as_deref(), invocation.cwd.as_deref())
    };
    let mut from_memory = remembered.is_some();
    let active = profile.clone().or(remembered);
    let mut started = None;
    let mut from_probe = false;
    let mut start = if active.is_none() && launch::should_launch(command, explicit) {
        match start_by_launching(command, &invocation.flags, &mut started) {
            Ok((start, probed)) => {
                from_probe = probed;
                start
            }
            Err(message) => return Output::err(message),
        }
    } else {
        match launch::start_after_explicit(invocation.cdp.as_deref(), env_url, active) {
            Ok(start) => start,
            Err(_) if from_memory && launch::should_launch(command, false) => {
                from_memory = false;
                match start_by_launching(command, &invocation.flags, &mut started) {
                    Ok((start, probed)) => {
                        from_probe = probed;
                        start
                    }
                    Err(message) => return Output::err(message),
                }
            }
            Err(message) => return Output::err(message),
        }
    };
    let mut opened = cdp::open(start.clone(), runtime.as_deref());
    if let Err(message) = &opened {
        if from_memory && launch::retry_open_without_endpoint(command, message) {
            from_memory = false;
            match start_by_launching(command, &invocation.flags, &mut started) {
                Ok((next, probed)) => {
                    from_probe = probed;
                    start = next;
                    opened = cdp::open(start.clone(), runtime.as_deref());
                }
                Err(next) => return Output::err(next),
            }
        }
    }
    let connected = match opened {
        Ok(cdp) => cdp,
        Err(message) => return Output::err(message),
    };
    let endpoint = match &start {
        connect::Start::Direct(url) | connect::Start::Discover(url) => url.clone(),
    };
    let mut cdp = connected;
    let mut state = session::load_in(invocation.session.as_deref(), invocation.cwd.as_deref());
    state.webmcp_connection = cdp_client::hash::sha256_hex(endpoint.as_bytes());
    let loaded_kind = state.provenance.clone();
    match saved_provenance(
        explicit,
        profile.is_some(),
        from_memory,
        from_probe,
        &endpoint,
        loaded_kind.as_deref(),
    ) {
        Some(kind) => {
            state.endpoint = Some(endpoint);
            state.provenance = Some(kind);
        }
        None if explicit || !cfg!(target_arch = "wasm32") => {
            state.endpoint = None;
            state.provenance = None;
        }
        None => {}
    }
    let mut report = action::Report::for_command(command);
    let result = match command {
        "open" | "tab-new" => cmd_open(&mut cdp, &mut state, invocation, &mut report, &mut started),
        "close" | "tab-close" => cmd_close(&mut cdp, &mut state, invocation, &mut report),
        "goto" | "navigate" => cmd_goto(&mut cdp, &mut state, invocation, &mut report),
        "snapshot" => cmd_snapshot(&mut cdp, &mut state, invocation, &mut report),
        "click" => cmd_click(&mut cdp, &mut state, invocation, &mut report),
        "fill" => cmd_fill(&mut cdp, &mut state, invocation, &mut report),
        "type" => cmd_type(&mut cdp, &mut state, invocation, &mut report),
        "press" => cmd_press(&mut cdp, &mut state, invocation, &mut report),
        "screenshot" => cmd_screenshot(&mut cdp, &mut state, invocation, &mut report),
        "eval" => cmd_eval(&mut cdp, &mut state, invocation, &mut report),
        "tab-list" => cmd_tab_list(&mut cdp, &state),
        "tab-select" => cmd_tab_select(&mut cdp, &mut state, invocation, &mut report),
        "console" => cmd_console(&mut cdp, &mut state, invocation),
        "requests" => cmd_requests(&mut cdp, &mut state, invocation),
        _ if trace::is_request_command(command) => {
            cmd_request(&mut cdp, &mut state, invocation, command)
        }
        "webmcp-polyfill" => crate::webmcp_polyfill::hold(&mut cdp, &mut state, invocation),
        "webmcp-list" => cmd_webmcp_list(&mut cdp, &mut state, invocation),
        _ if cookies::is_command(command) => cookies::run(&mut cdp, &mut state, invocation, command),
        "webmcp-call" => cmd_webmcp_call(&mut cdp, &mut state, invocation),
        "go-back" => cmd_go_back(&mut cdp, &mut state, invocation, &mut report),
        "go-forward" => cmd_go_forward(&mut cdp, &mut state, invocation, &mut report),
        "reload" => cmd_reload(&mut cdp, &mut state, invocation, &mut report),
        "hover" => cmd_hover(&mut cdp, &mut state, invocation, &mut report),
        "dblclick" => cmd_dblclick(&mut cdp, &mut state, invocation, &mut report),
        "select" => cmd_select(&mut cdp, &mut state, invocation, &mut report),
        "check" => cmd_check(&mut cdp, &mut state, invocation, &mut report),
        "uncheck" => cmd_uncheck(&mut cdp, &mut state, invocation, &mut report),
        "upload" => cmd_upload(&mut cdp, &mut state, invocation, &mut report),
        "drag" => cmd_drag(&mut cdp, &mut state, invocation, &mut report),
        "frames" => cmd_frames(&mut cdp, &mut state, invocation, &mut report),
        "pdf" => cmd_pdf(&mut cdp, &mut state, invocation, &mut report),
        "dialog-accept" => cmd_dialog(&mut cdp, &mut state, invocation, true),
        "dialog-dismiss" => cmd_dialog(&mut cdp, &mut state, invocation, false),
        _ => Output::err(format!("playwright-cli {command}: not implemented yet\n")),
    };
    let result = match session::save(&state) {
        Err(message) if cfg!(target_arch = "wasm32") => session::without_state(result, &message),
        Err(message) if result.code == 0 => Output::err(message),
        _ => result,
    };
    let mut result = result;
    for notice in cdp.take_notices() {
        result.stderr.push_str(&notice);
    }
    report.finish(&mut cdp, &result);
    result
}

fn start_by_launching(
    command: &str,
    flags: &BTreeMap<String, String>,
    started: &mut Option<Started>,
) -> Result<(connect::Start, bool), String> {
    let resolved = launch::resolve_without_endpoint(command, flags)?;
    let from_probe = resolved.from_probe;
    *started = resolved.started;
    Ok((connect::Start::Discover(resolved.discover), from_probe))
}

fn saved_provenance(
    explicit: bool,
    profile: bool,
    from_memory: bool,
    from_probe: bool,
    endpoint: &str,
    loaded: Option<&str>,
) -> Option<String> {
    if cfg!(target_arch = "wasm32") && !explicit {
        return None;
    }
    if !session::loopback_url(endpoint) {
        return None;
    }
    if explicit {
        return Some("explicit".to_string());
    }
    if profile {
        return Some("seven-profile".to_string());
    }
    if from_memory {
        return loaded
            .filter(|kind| session::known_provenance(kind))
            .map(str::to_string);
    }
    if from_probe {
        return Some("probe".to_string());
    }
    Some("seven-profile".to_string())
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

fn cmd_open(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
    started: &mut Option<Started>,
) -> Output {
    let url = invocation
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or("about:blank");
    let foreground =
        flag_true(&invocation.flags, "foreground") || flag_true(&invocation.flags, "fg");
    report.url(url);
    report.begin(cdp);
    match browser::create_target(cdp, state, url, foreground) {
        Ok(target_id) => {
            if let Some(started) = started.as_mut() {
                started.disarm();
            }
            report.tab(&target_id);
            state.current = Some(target_id.clone());
            Output::ok(format!("Opened {} in new tab [targetId: {target_id}]\n", sanitize::display_url(url)))
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_close(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let indexed = if invocation.command.as_deref() == Some("tab-close") {
        invocation.positionals.first().and_then(|text| close_index(text))
    } else {
        None
    };
    let target = match indexed {
        Some(index) => match pages.get(index - 1) {
            Some(page) => page.target_id.clone(),
            None => {
                let noun = if pages.len() == 1 { "tab" } else { "tabs" };
                return Output::err(format!(
                    "tab-close index {index} out of range ({} {noun} open)\n",
                    pages.len()
                ));
            }
        },
        None => match resolve_target(cdp, invocation, state, &pages, false) {
            Ok(target) => target,
            Err(message) => return Output::err(message),
        },
    };
    report.tab(&target);
    report.begin(cdp);
    match browser::close_target(cdp, &target) {
        Ok(()) => {
            state.snapshots.remove(&target);
            session::clear_console(state, &target);
            session::clear_requests(state, &target);
            if state.current.as_deref() == Some(target.as_str()) {
                state.current = None;
            }
            Output::ok(format!("Closed tab {target}\n"))
        }
        Err(message) => Output::err(message),
    }
}

fn close_index(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<usize>().ok().filter(|index| *index > 0)
}

fn cmd_goto(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let command = invocation.command.as_deref().unwrap_or("goto");
    let Some(url) = invocation.positionals.first() else {
        return Output::err(format!("{command} requires a URL\n"));
    };
    report.url(url.as_str());
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, true) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    report.begin(cdp);
    let existed = pages.iter().any(|page| page.target_id == target);
    let result = if existed {
        browser::navigate(cdp, state, &target, url)
    } else {
        Ok(())
    };
    match result {
        Ok(()) => {
            state.current = Some(target);
            state.snapshots.clear();
            Output::ok(format!("Navigated to {}\n", sanitize::display_url(url)))
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
            sanitize::display_url(&page.url),
            sanitize::browser_message(&page.title)
        ));
    }
    Output::ok(format!("{}\n", lines.join("\n")))
}

fn cmd_tab_select(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
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
    report.tab(&page.target_id);
    report.begin(cdp);
    let foreground =
        flag_true(&invocation.flags, "foreground") || flag_true(&invocation.flags, "fg");
    if foreground {
        if let Err(message) = browser::activate(cdp, &page.target_id) {
            return Output::err(message);
        }
    }
    state.current = Some(page.target_id.clone());
    Output::ok(format!(
        "Selected tab {index} [targetId: {}]\n",
        page.target_id
    ))
}

fn cmd_snapshot(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    report.begin(cdp);
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let built = build_snapshot(cdp, &session, invocation);
    let watched = browser::watch_detach(cdp, state, &target, &session);
    let (header, body, refs) = match built {
        Ok(value) => value,
        Err(message) => return Output::err(message),
    };
    if let Err(message) = watched {
        return Output::err(message);
    }
    state.current = Some(target.clone());
    state.snapshots.insert(target, refs);
    let mut lines = header;
    lines.push("### Snapshot".to_string());
    if let Some(filename) = invocation.flags.get("filename") {
        if let Err(message) = write_text(filename, &body) {
            return Output::err(message);
        }
        lines.push(format!("- [Snapshot]({})", sanitize::terminal(filename)));
    } else {
        lines.extend(["```yaml".to_string(), body, "```".to_string()]);
    }
    Output::ok(format!("{}\n", lines.join("\n")))
}

type Built = (Vec<String>, String, BTreeMap<String, RefRec>);

fn build_snapshot(cdp: &mut Cdp, session: &str, invocation: &Invocation) -> Result<Built, String> {
    let (url, title) = browser::page_location(cdp, session)?;
    let mut header = vec!["### Page".to_string(), format!("- Page URL: {}", sanitize::display_url(&url))];
    if !title.is_empty() {
        header.push(format!("- Page Title: {}", sanitize::browser_message(&title)));
    }
    let frames = browser::page_frames(cdp, session)?;
    let chosen = invocation.flags.get("frame");
    let root = match chosen {
        Some(frame_id) => frames.iter().find(|frame| frame.frame_id == *frame_id).ok_or_else(|| {
            let tab = invocation.flags.get("tab").map(String::as_str).unwrap_or("<targetId>");
            format!(
                "Unknown frame ID \"{frame_id}\" for tab {tab}. Run 'playwright-cli frames --tab={tab}' to list frame IDs.\n"
            )
        })?,
        None => frames
            .iter()
            .find(|frame| frame.parent_frame_id.is_none())
            .ok_or_else(|| "snapshot: the tab has no main frame\n".to_string())?,
    };
    let walk = Walk {
        frames: &frames,
        guard: cdp.guard().clone(),
        iframes: !flag_true(&invocation.flags, "no-iframes"),
        default_root: (chosen.is_none() && cdp.guard().error_page_url(&root.url) && !cdp.content_blocked(session)).then_some(root.frame_id.as_str()),
        extra: std::cell::Cell::new(0),
        seen: std::cell::RefCell::new([root.frame_id.clone()].into()),
    };
    let depth = match invocation.flags.get("depth") {
        Some(text) => Some(text.parse::<i64>().map_err(|_| {
            format!("snapshot: --depth must be a number, not \"{}\"\n", sanitize::terminal(text))
        })?),
        None => None,
    };
    let options = snapshot::Options {
        focused: if chosen.is_some() { None } else { Some(true) },
        depth,
        boxes: flag_true(&invocation.flags, "boxes"),
    };
    let mut refs = BTreeMap::new();
    let scope = Scope { session, frames: &frames, target: None, owner: None };
    let nodes = match root.target {
        Some(_) => walk.remote(cdp, &scope, &root.frame_id, None, options, &mut refs)?,
        None => walk.frame(cdp, &scope, &root.frame_id, options, root.parent_frame_id.is_some(), &mut refs)?,
    };
    Ok((header, sanitize::terminal(&snapshot::render(&nodes)), refs))
}

struct Scope<'a> {
    session: &'a str,
    frames: &'a [snapshot::FrameRec],
    target: Option<&'a str>,
    owner: Option<u64>,
}

struct Walk<'a> {
    frames: &'a [snapshot::FrameRec],
    guard: cdp_client::protected::Protected,
    iframes: bool,
    default_root: Option<&'a str>,
    extra: std::cell::Cell<usize>,
    seen: std::cell::RefCell<std::collections::BTreeSet<String>>,
}

impl Walk<'_> {
    fn frame(
        &self,
        cdp: &mut Cdp,
        scope: &Scope,
        frame_id: &str,
        options: snapshot::Options,
        child: bool,
        refs: &mut BTreeMap<String, RefRec>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let prefix = snapshot::frame_prefix(self.frames, frame_id, || {
            self.extra.set(self.extra.get() + 1);
            self.frames.len() + self.extra.get()
        });
        let capture = browser::aria_capture(cdp, scope.session, frame_id, &prefix, options, self.default_root == Some(frame_id))?;
        let mut nodes = capture.json;
        snapshot::redact_links(&mut nodes, &capture.base, &self.guard);
        let labels = snapshot::labels(&nodes);
        for rec in &capture.refs {
            refs.insert(
                rec.id.clone(),
                RefRec {
                    backend_node_id: rec.backend_node_id,
                    selector: rec.selector.clone(),
                    frame_id: child.then(|| frame_id.to_string()),
                    label: labels.get(&rec.id).cloned().flatten(),
                    target: scope.target.map(str::to_string),
                    owner: scope.owner,
                    page: None,
                },
            );
        }
        if !self.iframes {
            return Ok(nodes);
        }
        for rec in &capture.refs {
            let Some(content) = rec.content_frame.as_deref() else {
                continue;
            };
            let local = scope.frames.iter().find(|frame| frame.frame_id == content && frame.target.is_none());
            let remote = local.is_none() && self.frames.iter().any(|frame| frame.frame_id == content && frame.target.is_some());
            if local.is_some_and(|frame| self.guard.url(&frame.url)) {
                continue;
            }
            let Some(node) = snapshot::find_mut(&mut nodes, &rec.id) else {
                continue;
            };
            if node.get("role").and_then(serde_json::Value::as_str) != Some("iframe")
                || !self.seen.borrow_mut().insert(content.to_string())
            {
                continue;
            }
            let active = node.get("active").and_then(serde_json::Value::as_bool) == Some(true);
            let inner_options =
                snapshot::child_options(options, active, capture.iframe_depths.get(&rec.id).copied());
            let inner = if remote {
                self.remote(cdp, scope, content, rec.backend_node_id, inner_options, refs)
            } else {
                self.frame(cdp, scope, content, inner_options, true, refs)
            };
            if let Some(node) = snapshot::find_mut(&mut nodes, &rec.id) {
                if let Some(inner) = inner.ok().filter(|inner| !inner.is_empty()) {
                    node["children"] = serde_json::Value::Array(inner);
                }
            }
        }
        Ok(nodes)
    }

    fn remote(
        &self,
        cdp: &mut Cdp,
        scope: &Scope,
        frame_id: &str,
        owner: Option<u64>,
        options: snapshot::Options,
        refs: &mut BTreeMap<String, RefRec>,
    ) -> Result<Vec<serde_json::Value>, String> {
        let frame = self.frames.iter().find(|frame| frame.frame_id == frame_id && frame.target.is_some()).cloned().unwrap_or_default();
        let (session, frames) = browser::open_frame(cdp, &frame)?;
        let inner = Scope {
            session: &session,
            frames: &frames,
            target: Some(frame_id),
            owner: owner.filter(|_| scope.target.is_none()),
        };
        let nodes = self.frame(cdp, &inner, frame_id, options, true, refs);
        cdp::detach(cdp, &session);
        nodes
    }
}

fn cmd_click(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let Some(id) = invocation.positionals.first() else {
        return Output::err("click requires a ref (e.g. e5)\n");
    };
    let button = invocation
        .positionals
        .get(1)
        .map(String::as_str)
        .unwrap_or("left");
    if !matches!(button, "left" | "right" | "middle") {
        return Output::err(format!("unknown button \"{button}\"\n"));
    }
    let modifiers = modifiers_of(invocation.flags.get("modifiers").map(String::as_str));
    with_ref(cdp, state, invocation, report, id, |cdp, session, rec| {
        browser::click_ref(cdp, session, rec, button, modifiers)?;
        Ok(format!("Clicked {id}\n"))
    })
}

fn cmd_fill(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    if invocation.positionals.len() < 2 {
        return Output::err("fill requires <ref> <text>\n");
    }
    let id = &invocation.positionals[0];
    let text = invocation.positionals[1..].join(" ");
    report.length(text.chars().count());
    let submit = flag_true(&invocation.flags, "submit");
    with_ref(cdp, state, invocation, report, id, |cdp, session, rec| {
        browser::fill_ref(cdp, session, rec, &text)?;
        if submit {
            browser::press_key(cdp, session, "Enter")?;
        }
        Ok(format!("Filled {id} with: {text}\n"))
    })
}

fn cmd_type(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    if invocation.positionals.is_empty() {
        return Output::err("type requires text\n");
    }
    let text = invocation.positionals.join(" ");
    report.length(text.chars().count());
    let submit = flag_true(&invocation.flags, "submit");
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    report.begin(cdp);
    let session = match browser::watch_attach(cdp, state, &target) {
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
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(()) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
            state.snapshots.remove(&target);
            Output::ok(format!("Typed: {text}\n"))
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_press(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let Some(key) = invocation.positionals.first() else {
        return Output::err("press requires a key name\n");
    };
    report.key(key.as_str());
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    report.begin(cdp);
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = browser::press_key(cdp, &session, key);
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(()) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
            Output::ok(format!("Pressed {key}\n"))
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_screenshot(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    if let Some(id) = invocation.positionals.first() {
        if let Some(label) = session::stored_label(state, &target, id) {
            report.target(label);
        }
    }
    report.begin(cdp);
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = capture(cdp, state, invocation, &target, &session);
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(message) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
            Output::ok(message)
        }
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
    let full =
        flag_true(&invocation.flags, "fullPage") || flag_true(&invocation.flags, "full-page");
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

fn cmd_eval(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
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
    report.tab(&target);
    report.begin(cdp);
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result: Result<String, String> = (|| {
        let value = if let Some(frame_id) = invocation.flags.get("frame") {
            let child = browser::frame_session(cdp, &session, frame_id)?;
            let scope = child.as_deref().unwrap_or(&session);
            let value = browser::evaluate_called(cdp, scope, Some(frame_id), &expression).or_else(|err| {
                retry_await(err, &expression, |source| {
                    browser::evaluate_called(cdp, scope, Some(frame_id), source)
                })
            });
            if let Some(child) = child.as_deref() {
                cdp::detach(cdp, child);
            }
            value?
        } else {
            browser::evaluate_called(cdp, &session, None, &expression).or_else(|err| {
                retry_await(err, &expression, |source| {
                    browser::evaluate_called(cdp, &session, None, source)
                })
            })?
        };
        Ok(browser::format_eval(&value))
    })();
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(text) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
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
            Output::ok(format!("{}\n", sanitize::browser_message(&text)))
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
    report: &mut Report,
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
    report.tab(&target);
    if let Some(label) = session::stored_label(state, &target, id) {
        report.target(label);
    }
    report.begin(cdp);
    let refs = match state.snapshots.get(&target) {
        Some(refs) => refs.clone(),
        None => return Output::err("No snapshot available. Run \"snapshot\" first.\n"),
    };
    let rec = match browser::lookup_ref(&refs, id) {
        Ok(rec) => rec.clone(),
        Err(message) => return Output::err(message),
    };
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = browser::ref_session(cdp, &session, &rec).and_then(|(rec, child)| {
        let result = body(cdp, child.as_deref().unwrap_or(&session), &rec);
        if let Some(child) = child {
            cdp::detach(cdp, &child);
        }
        result
    });
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(text) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
            state.snapshots.remove(&target);
            Output::ok(text)
        }
        Err(message) => Output::err(message),
    }
}

pub(crate) fn resolve_target(
    cdp: &mut Cdp,
    invocation: &Invocation,
    state: &mut Session,
    pages: &[PageInfo],
    create_for_goto: bool,
) -> Result<String, String> {
    if let Some(tab) = invocation.flags.get("tab") {
        if pages.iter().any(|page| page.target_id == *tab) {
            return Ok(tab.clone());
        }
        if cdp.is_protected_target(tab) {
            return Err(format!("{}\n", cdp.guard().refusal()));
        }
        match cdp.target_protected(tab, false) {
            Ok(true) => return Err(format!("{}\n", cdp.guard().refusal())),
            Err(message) if crate::cdp::transport_error(&message) => return Err(message),
            _ => {}
        }
        return Err(format!(
            "Unknown tab \"{tab}\". Run 'playwright-cli tab-list' to get tab IDs.\n"
        ));
    }
    if let Some(current) = state.current.clone() {
        if pages.iter().any(|page| page.target_id == current) {
            return Ok(current);
        }
        if cdp.is_protected_target(&current) {
            return Err(format!("{}\n", cdp.guard().refusal()));
        }
        match cdp.target_protected(&current, true) {
            Ok(true) => return Err(format!("{}\n", cdp.guard().refusal())),
            Err(message) if crate::cdp::transport_error(&message) => return Err(message),
            _ => {}
        }
    }
    if pages.len() == 1 {
        return Ok(pages[0].target_id.clone());
    }
    if create_for_goto && pages.is_empty() {
        if let Some(url) = invocation.positionals.first() {
            return browser::create_target(cdp, state, url, false);
        }
    }
    Err(
        "Error: --tab <targetId> is required. Run 'playwright-cli tab-list' to get tab IDs.\n"
            .to_string(),
    )
}

fn cmd_console(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let target = match current_target(cdp, invocation, state) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    if flag_true(&invocation.flags, "clear") {
        session::clear_console(state, &target);
        return Output::ok("Console messages cleared\n");
    }
    let level = invocation
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or("info");
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    if cdp.content_blocked(&session) {
        cdp::detach(cdp, &session);
        return Output::err(format!("{}\n", cdp.guard().refusal()));
    }
    if let Err(message) = browser::watch_detach(cdp, state, &target, &session) {
        return Output::err(message);
    }
    let entries = state.console.get(&target).cloned().unwrap_or_default();
    match trace::render_console(&entries, level) {
        Ok(text) => Output::ok(format!("{text}\n")),
        Err(message) => Output::err(message),
    }
}

fn cmd_requests(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Output {
    let target = match current_target(cdp, invocation, state) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    if flag_true(&invocation.flags, "clear") {
        session::clear_requests(state, &target);
        return Output::ok("Network requests cleared\n");
    }
    let items = state.requests.get(&target).cloned().unwrap_or_default();
    let filter = invocation.flags.get("filter").map(String::as_str);
    match trace::render_requests(&items, flag_true(&invocation.flags, "static"), filter) {
        Ok(text) => Output::ok(format!("{text}\n")),
        Err(message) => Output::err(message),
    }
}

fn cmd_request(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    command: &str,
) -> Output {
    let Some(index) = invocation
        .positionals
        .first()
        .and_then(|text| trace::positive_index(text))
    else {
        return Output::err(format!("playwright-cli {command}: index is required\n"));
    };
    let target = match current_target(cdp, invocation, state) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let mut items = state.requests.get(&target).cloned().unwrap_or_default();
    let current_values = match items.get_mut(index - 1) {
        Some(request) => match crate::cookie_headers::hydrate(cdp, &target, request, command) {
            Ok(restored) => restored,
            Err(message) => return Output::err(message),
        },
        None => false,
    };
    let filename = invocation.flags.get("filename").map(String::as_str);
    let title = part_title(command);
    if command == "request" {
        let mut text = match trace::render_request(&items, index) {
            Ok(text) => text,
            Err(message) => return Output::err(message),
        };
        if current_values {
            text.push_str(&format!("\n\n{}", crate::cookie_headers::CURRENT_VALUES));
        }
        if filename.is_some() {
            let body = text.strip_prefix("### Result\n").unwrap_or(&text);
            return finish_text(false, title, body, filename);
        }
        return finish_text(true, title, &text, None);
    }
    match trace::render_part(&items, index, command) {
        Ok(Piece::Text(mut body)) => {
            if current_values {
                body.push_str(&format!("\n{}", crate::cookie_headers::CURRENT_VALUES));
            }
            finish_text(true, title, &body, filename)
        },
        Ok(Piece::Empty) => finish_text(true, title, "", filename),
        Ok(Piece::File { bytes, extension }) => finish_file(title, &bytes, &extension, filename),
        Err(message) => Output::err(message),
    }
}

fn current_target(
    cdp: &mut Cdp,
    invocation: &Invocation,
    state: &mut Session,
) -> Result<String, String> {
    let pages = browser::list_pages(cdp)?;
    resolve_target(cdp, invocation, state, &pages, false)
}

fn finish_text(raw: bool, title: &str, body: &str, filename: Option<&str>) -> Output {
    if let Some(path) = filename {
        if let Err(message) = write_text(path, body) {
            return Output::err(message);
        }
        let link = trace::printable_link(title, path);
        if raw {
            return Output::ok(format!("{link}\n"));
        }
        return Output::ok(format!("### Result\n{link}\n"));
    }
    if body.is_empty() {
        return Output::ok(String::new());
    }
    if body.ends_with('\n') {
        Output::ok(body.to_string())
    } else {
        Output::ok(format!("{body}\n"))
    }
}

fn finish_file(title: &str, bytes: &[u8], extension: &str, filename: Option<&str>) -> Output {
    let path = match filename {
        Some(path) => PathBuf::from(path),
        None => default_body_path(extension),
    };
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(err) = fs::create_dir_all(parent) {
                return Output::err(format!("{}: {err}\n", path.display()));
            }
        }
    }
    if let Err(err) = fs::write(&path, bytes) {
        return Output::err(format!("{}: {err}\n", path.display()));
    }
    let link = trace::printable_link(title, &path.display().to_string());
    Output::ok(format!("{link}\n"))
}

fn default_body_path(extension: &str) -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let name = format!("response-{millis}.{extension}");
    #[cfg(not(target_os = "wasi"))]
    {
        PathBuf::from("/tmp").join(name)
    }
    #[cfg(target_os = "wasi")]
    {
        PathBuf::from(".playwright-cli").join(name)
    }
}

fn part_title(command: &str) -> &'static str {
    match command {
        "request-headers" => "Request headers",
        "request-body" => "Request body",
        "response-headers" => "Response headers",
        "response-body" => "Response body",
        _ => "Request",
    }
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

fn cmd_webmcp_list(
    cdp: &mut Cdp,
    state: &mut session::Session,
    invocation: &Invocation,
) -> Output {
    let tab = invocation.flags.get("tab").map(String::as_str);
    let json = flag_true(&invocation.flags, "json");
    let from = match crate::webmcp_frames::origins(&invocation.repeated("from-origin")) {
        Ok(from) => from,
        Err(message) => return Output::err(message),
    };
    crate::webmcp::list_from(cdp, state, tab, json, &from)
}

fn cmd_webmcp_call(
    cdp: &mut Cdp,
    state: &mut session::Session,
    invocation: &Invocation,
) -> Output {
    let Some(name) = invocation.positionals.first() else {
        return Output::err("webmcp-call requires <name> and <json-input>\n");
    };
    let Some(raw) = invocation.positionals.get(1) else {
        return Output::err("webmcp-call requires <json-input>\n");
    };
    let input_json = if raw == "-" {
        match crate::webmcp::read_stdin() {
            Ok(text) => text,
            Err(message) => return Output::err(message),
        }
    } else {
        raw.clone()
    };
    let tab = invocation.flags.get("tab").map(String::as_str);
    let allow_consequential = flag_true(&invocation.flags, "allow-consequential");
    let from = match crate::webmcp_frames::origins(&invocation.repeated("from-origin")) {
        Ok(from) => from,
        Err(message) => return Output::err(message),
    };
    let frame = invocation.flags.get("frame").map(String::as_str);
    crate::webmcp::call_from(cdp, state, name, &input_json, tab, allow_consequential, &from, frame)
}


fn default_pdf_path() -> PathBuf {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = format!("page-{millis}.pdf");
    #[cfg(not(target_os = "wasi"))]
    {
        PathBuf::from("/tmp").join(name)
    }
    #[cfg(target_os = "wasi")]
    {
        PathBuf::from(".playwright-cli").join(name)
    }
}

fn cmd_dialog(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation, accept: bool) -> Output {
    let prompt = (accept && !invocation.positionals.is_empty()).then(|| invocation.positionals.join(" "));
    let target = match invocation.flags.get("tab").cloned().or_else(|| state.current.clone()) {
        Some(target) => target,
        None => match cdp.dialog_targets() {
            Ok(targets) if targets.len() == 1 => targets[0].clone(),
            Ok(_) => return Output::err("Error: --tab <targetId> is required. Run 'playwright-cli tab-list' to get tab IDs.\n"),
            Err(message) => return Output::err(message),
        },
    };
    let session = match cdp.attach_dialog(&target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = browser::handle_dialog(cdp, &session, accept, prompt.as_deref());
    cdp::detach(cdp, &session);
    match result {
        Ok(text) => {
            state.current = Some(target);
            Output::ok(text)
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_go_back(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    report.begin(cdp);
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = browser::evaluate(cdp, &session, "history.back()", false);
    if result.is_ok() {
        let _ = cdp.wait_load(&session);
    }
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(_) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
            state.snapshots.remove(&target);
            Output::ok("Navigated back\n")
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_go_forward(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    report.begin(cdp);
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = browser::evaluate(cdp, &session, "history.forward()", false);
    if result.is_ok() {
        let _ = cdp.wait_load(&session);
    }
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(_) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
            state.snapshots.remove(&target);
            Output::ok("Navigated forward\n")
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_reload(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    report.begin(cdp);
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = cdp.call("Page.reload", serde_json::json!({}), Some(&session));
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(_) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
            state.snapshots.remove(&target);
            Output::ok("Reloaded\n")
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_hover(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let Some(id) = invocation.positionals.first() else {
        return Output::err("hover requires a ref (e.g. e5)\n");
    };
    with_ref(cdp, state, invocation, report, id, |cdp, session, rec| {
        browser::hover_ref(cdp, session, rec)?;
        Ok(format!("Hovered {id}\n"))
    })
}

fn cmd_dblclick(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let Some(id) = invocation.positionals.first() else {
        return Output::err("dblclick requires a ref (e.g. e5)\n");
    };
    let button = invocation
        .positionals
        .get(1)
        .map(String::as_str)
        .unwrap_or("left");
    if !matches!(button, "left" | "right" | "middle") {
        return Output::err(format!("unknown button \"{button}\"\n"));
    }
    let modifiers = modifiers_of(invocation.flags.get("modifiers").map(String::as_str));
    with_ref(cdp, state, invocation, report, id, |cdp, session, rec| {
        browser::dblclick_ref(cdp, session, rec, button, modifiers)?;
        Ok(format!("Double-clicked {id}\n"))
    })
}

fn cmd_select(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    if invocation.positionals.len() < 2 {
        return Output::err("select requires <ref> <value>\n");
    }
    let id = &invocation.positionals[0];
    let value = invocation.positionals[1..].join(" ");
    with_ref(cdp, state, invocation, report, id, |cdp, session, rec| {
        browser::select_ref(cdp, session, rec, &value)?;
        Ok(format!("Selected \"{value}\" on {id}\n"))
    })
}

fn cmd_check(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let Some(id) = invocation.positionals.first() else {
        return Output::err("check requires a ref (e.g. e5)\n");
    };
    with_ref(cdp, state, invocation, report, id, |cdp, session, rec| {
        let toggled = browser::check_ref(cdp, session, rec, true)?;
        if toggled {
            Ok(format!("Checked {id}\n"))
        } else {
            Ok(format!("{id} already checked\n"))
        }
    })
}

fn cmd_uncheck(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let Some(id) = invocation.positionals.first() else {
        return Output::err("uncheck requires a ref (e.g. e5)\n");
    };
    with_ref(cdp, state, invocation, report, id, |cdp, session, rec| {
        let toggled = browser::check_ref(cdp, session, rec, false)?;
        if toggled {
            Ok(format!("Unchecked {id}\n"))
        } else {
            Ok(format!("{id} already unchecked\n"))
        }
    })
}

fn cmd_upload(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    if invocation.positionals.len() < 2 {
        return Output::err("upload requires <ref> <file>\n");
    }
    let id = &invocation.positionals[0];
    let files: Vec<String> = invocation.positionals[1..].to_vec();
    for file in &files {
        if let Err(message) = browser::validate_upload_path(file) {
            return Output::err(message);
        }
    }
    with_ref(cdp, state, invocation, report, id, |cdp, session, rec| {
        browser::upload_files(cdp, session, rec, &files)?;
        let count = files.len();
        let names: Vec<&str> = files
            .iter()
            .map(|f| {
                std::path::Path::new(f)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(f)
            })
            .collect();
        Ok(format!(
            "Uploaded {} file(s): {}\n",
            count,
            names.join(", ")
        ))
    })
}

fn cmd_drag(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    if invocation.positionals.len() < 2 {
        return Output::err("drag requires <startRef> <endRef>\n");
    }
    let start_id = &invocation.positionals[0];
    let end_id = &invocation.positionals[1];
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    report.begin(cdp);
    let refs = match state.snapshots.get(&target) {
        Some(refs) => refs.clone(),
        None => return Output::err("No snapshot available. Run \"snapshot\" first.\n"),
    };
    let start_rec = match browser::lookup_ref(&refs, start_id) {
        Ok(rec) => rec.clone(),
        Err(message) => return Output::err(message),
    };
    let end_rec = match browser::lookup_ref(&refs, end_id) {
        Ok(rec) => rec.clone(),
        Err(message) => return Output::err(message),
    };
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = drag_between(cdp, &session, &start_rec, &end_rec);
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(()) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
            state.snapshots.remove(&target);
            Output::ok(format!("Dragged {start_id} to {end_id}\n"))
        }
        Err(message) => Output::err(message),
    }
}

fn drag_between(cdp: &mut Cdp, session: &str, start: &RefRec, end: &RefRec) -> Result<(), String> {
    let mut children = Vec::new();
    let result = (|| {
        let (start, start_child) = browser::ref_session(cdp, session, start)?;
        children.extend(start_child.clone());
        let (end, end_child) = browser::ref_session(cdp, session, end)?;
        children.extend(end_child.clone());
        browser::drag_refs(
            cdp,
            session,
            (start_child.as_deref().unwrap_or(session), &start),
            (end_child.as_deref().unwrap_or(session), &end),
        )
    })();
    for child in children {
        cdp::detach(cdp, &child);
    }
    result
}

fn cmd_frames(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    report.begin(cdp);
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = browser::page_frames(cdp, &session);
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(frames) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
            let mut lines = Vec::new();
            for frame in &frames {
                let kind = if frame.parent_frame_id.is_some() {
                    "child"
                } else {
                    "main"
                };
                let parent = match &frame.parent_frame_id {
                    Some(pid) => format!(" parentFrameId={pid}"),
                    None => String::new(),
                };
                lines.push(format!(
                    "  [{kind}] frameId={}{parent} - {}",
                    frame.frame_id, browser::frame_display_url(&frame.url)
                ));
            }
            Output::ok(format!(
                "Frame IDs (use with --frame, never --tab):\n{}\n",
                lines.join("\n")
            ))
        }
        Err(message) => Output::err(message),
    }
}

fn cmd_pdf(
    cdp: &mut Cdp,
    state: &mut Session,
    invocation: &Invocation,
    report: &mut Report,
) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve_target(cdp, invocation, state, &pages, false) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    report.tab(&target);
    report.begin(cdp);
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = browser::print_to_pdf(cdp, &session);
    let watched = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(bytes) => {
            if let Err(message) = watched {
                return Output::err(message);
            }
            let path = invocation
                .flags
                .get("filename")
                .map(PathBuf::from)
                .unwrap_or_else(default_pdf_path);
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() {
                    if let Err(err) = fs::create_dir_all(parent) {
                        return Output::err(format!("pdf: {err}\n"));
                    }
                }
            }
            if let Err(err) = fs::write(&path, &bytes) {
                return Output::err(format!("pdf: {err}\n"));
            }
            Output::ok(format!("Saved PDF to {}\n", path.display()))
        }
        Err(message) => Output::err(message),
    }
}
