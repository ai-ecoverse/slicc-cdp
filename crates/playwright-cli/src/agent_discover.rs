use crate::args::{Invocation, Output};
use crate::browser::{self, PageInfo};
use crate::cdp::{self, Cdp};
use crate::launch;
use crate::session;
use discovery::{
    classify_response, classify_slicc_handoff, default_classifiers,
    default_probes, fingerprint, is_slicc_handoff_rel, known_link_rels,
    parse_link_header, classify_ard_body, classify_ai_catalog_body,
    classify_poppy_body, registrable_domain, parse_robots_txt_agentmap,
    classify_nlweb_ask, classify_nlweb_mcp_init,
    ClassifierRegistry, DedupSet, DiscoveryEvent,
    ProbeRegistry, Validity, Via, EVENT_VERSION, LLMS_TXT_REL,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use url::Url;

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

#[allow(dead_code)]
const IO_READ_CAP: usize = 1024 * 1024;

#[derive(Clone)]
enum Sink {
    Stdout,
    Exec { cmd: String, timeout: Duration },
    Lick { slicc_dir: String },
}

fn slicc_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if home.is_empty() {
        return String::new();
    }
    format!("{home}/.slicc")
}

fn disabled_path() -> String {
    let config = discover_config_dir();
    if config.is_empty() {
        return String::new();
    }
    format!("{config}/slicc/discover.disabled")
}

fn discover_config_dir() -> String {
    #[cfg(test)]
    {
        std::env::temp_dir()
            .join("slicc-cdp-discover-tests")
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(all(not(test), not(target_arch = "wasm32")))]
    {
        launch::user_config_dir()
    }
    #[cfg(all(not(test), target_arch = "wasm32"))]
    {
        String::new()
    }
}

fn is_disabled() -> bool {
    let path = disabled_path();
    if path.is_empty() {
        return false;
    }
    std::path::Path::new(&path).exists()
}

struct WatcherState {
    dedup: DedupSet,
    probed_origins: BTreeSet<String>,
    probes: ProbeRegistry,
    classifiers: ClassifierRegistry,
    sessions: BTreeMap<String, SessionState>,
    json_mode: bool,
    follow_mode: bool,
    robots_probe: bool,
    nlweb_probe: bool,
    no_probe: bool,
    events_emitted: usize,
    event_cursor: usize,
    sink: Sink,
}

struct SessionState {
    target_id: String,
    top_frame_id: Option<String>,
    page_url: String,
}

pub fn run(invocation: &Invocation, cdp_env: Option<&str>) -> Output {
    if invocation.flags.contains_key("off") {
        return Output::ok("");
    }
    if is_disabled() {
        return Output {
            stdout: String::new(),
            stderr: "discover: disabled by discover.disabled\n".to_string(),
            code: 0,
        };
    }

    let json_mode = invocation.flags.contains_key("json");
    let once_url = invocation.flags.get("once").cloned();
    let once_tab = invocation.flags.get("tab").cloned();
    let no_probe = invocation.flags.contains_key("no-probe");
    let follow_mode = invocation.flags.contains_key("follow");
    let probes = invocation.repeated("probe");
    let robots_probe = probes.contains(&"robots");
    let nlweb_probe = probes.contains(&"nlweb");
    let exec_cmd = invocation.flags.get("exec").cloned();
    let lick_mode = invocation.flags.contains_key("lick");
    let exec_timeout = invocation
        .flags
        .get("exec-timeout")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(10);

    #[cfg(target_os = "wasi")]
    if exec_cmd.is_some() {
        return Output::err("discover: --exec is not available in wasm\n");
    }

    #[cfg(not(unix))]
    if exec_cmd.is_some() {
        return Output::err("discover: --exec is only available on unix\n");
    }

    let sink = if let Some(cmd) = exec_cmd {
        Sink::Exec {
            cmd,
            timeout: Duration::from_secs(exec_timeout),
        }
    } else if lick_mode || (cdp_env.is_some() && !json_mode) {
        let dir = slicc_dir();
        if dir.is_empty() {
            return Output::err("discover: HOME is not set, cannot use --lick\n");
        }
        ensure_lick_dirs(&dir);
        Sink::Lick { slicc_dir: dir }
    } else {
        Sink::Stdout
    };

    let explicit = launch::endpoint_is_explicit(invocation.cdp.as_deref(), cdp_env);
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
    let active = profile.or(remembered);
    let start = match launch::start_after_explicit(invocation.cdp.as_deref(), cdp_env, active) {
        Ok(start) => start,
        Err(message) => return Output::err(message),
    };
    let mut cdp = match cdp::open(start, invocation.runtime.as_deref()) {
        Ok(cdp) => cdp,
        Err(message) => return Output::err(message),
    };
    let mut state = WatcherState {
        dedup: DedupSet::new(),
        probed_origins: BTreeSet::new(),
        probes: default_probes(),
        classifiers: default_classifiers(),
        sessions: BTreeMap::new(),
        json_mode,
        follow_mode,
        robots_probe,
        nlweb_probe,
        no_probe,
        events_emitted: 0,
        event_cursor: 0,
        sink: sink.clone(),
    };

    if let Err(message) = cdp.call(
        "Target.setDiscoverTargets",
        json!({ "discover": true }),
        None,
    ) {
        return Output::err(message);
    }

    let mut sess_state = session::load_in(invocation.session.as_deref(), invocation.cwd.as_deref());

    if let Some(ref url) = once_url {
        let target = match resolve_once_tab(&mut cdp, &mut sess_state, once_tab.as_deref()) {
            Ok(target) => target,
            Err(message) => return Output::err(message),
        };
        let session = match cdp::attach(&mut cdp, &target) {
            Ok(session) => session,
            Err(message) => return Output::err(message),
        };
        if let Err(message) = setup_session(&mut cdp, &session) {
            cdp::detach(&mut cdp, &session);
            return Output::err(message);
        }
        let frame_tree = cdp
            .call("Page.getFrameTree", json!({}), Some(&session))
            .ok()
            .and_then(|result| {
                result
                    .pointer("/frameTree/frame/id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
            });
        state.sessions.insert(
            session.clone(),
            SessionState {
                target_id: target.clone(),
                top_frame_id: frame_tree,
                page_url: String::new(),
            },
        );
        if let Err(message) = cdp.call(
            "Page.navigate",
            json!({ "url": url }),
            Some(&session),
        ) {
            cdp::detach(&mut cdp, &session);
            return Output::err(message);
        }
        let mut output = String::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut loaded = false;
        let load_from = cdp.events().len();
        while Instant::now() < deadline {
            match cdp.pump(Instant::now() + Duration::from_millis(200)) {
                Ok(_) => {}
                Err(_) => break,
            }
            output.push_str(&process_events(&mut cdp, &mut state));
            if !loaded {
                for event in cdp.events()[load_from..].iter() {
                    if event.get("method").and_then(|v| v.as_str()) == Some("Page.loadEventFired")
                        && event
                            .get("sessionId")
                            .and_then(|v| v.as_str())
                            .map_or(false, |s| s == session)
                    {
                        loaded = true;
                    }
                }
            }
            if loaded {
                let frame_tree_result = cdp
                    .call("Page.getFrameTree", json!({}), Some(&session))
                    .ok();
                let committed_url = frame_tree_result
                    .as_ref()
                    .and_then(|r| {
                        r.pointer("/frameTree/frame/url")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                let committed_frame_id = frame_tree_result
                    .as_ref()
                    .and_then(|r| {
                        r.pointer("/frameTree/frame/id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                if let Some(ref url) = committed_url {
                    if let Some(sess) = state.sessions.get_mut(&session) {
                        sess.page_url = url.clone();
                    }
                }
                if let Some(ref fid) = committed_frame_id {
                    if let Some(sess) = state.sessions.get_mut(&session) {
                        sess.top_frame_id = Some(fid.clone());
                    }
                }
                let frame_id = state
                    .sessions
                    .get(&session)
                    .and_then(|s| s.top_frame_id.clone());
                let page_signals = collect_page_signals(
                    &mut cdp,
                    &session,
                    frame_id.as_deref(),
                );
                output.push_str(&emit_page_signals(
                    &mut state,
                    &session,
                    &page_signals,
                ));
                let sess = state.sessions.get(&session);
                let origin = sess
                    .and_then(|s| extract_origin(&s.page_url))
                    .unwrap_or_default();
                if !origin.is_empty() && !no_probe {
                    output.push_str(&run_probes(&mut cdp, &mut state, &session, &origin));
                }
                if state.follow_mode {
                    output.push_str(&follow_links(&mut cdp, &mut state, &session));
                }
                break;
            }
        }
        cdp::detach(&mut cdp, &session);
        let code = if state.events_emitted > 0 { 0 } else { 1 };
        return Output {
            stdout: output,
            stderr: String::new(),
            code,
        };
    }

    let pages = match browser::list_pages(&mut cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    for page in &pages {
        if cdp.is_protected_target(&page.target_id) {
            continue;
        }
        if let Ok(session) = cdp::attach(&mut cdp, &page.target_id) {
            if setup_session(&mut cdp, &session).is_ok() {
                let frame_tree = cdp
                    .call("Page.getFrameTree", json!({}), Some(&session))
                    .ok()
                    .and_then(|result| {
                        result
                            .pointer("/frameTree/frame/id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                state.sessions.insert(
                    session,
                    SessionState {
                        target_id: page.target_id.clone(),
                        top_frame_id: frame_tree,
                        page_url: page.url.clone(),
                    },
                );
            }
        }
    }

    #[cfg(unix)]
    {
        static SIGHUP_FLAG: AtomicBool = AtomicBool::new(false);
        unsafe {
            libc::signal(libc::SIGHUP, sighup_handler as libc::sighandler_t);
        }
        extern "C" fn sighup_handler(_: libc::c_int) {
            SIGHUP_FLAG.store(true, Ordering::SeqCst);
        }

        loop {
            if SIGHUP_FLAG.swap(false, Ordering::SeqCst) && is_disabled() {
                return Output {
                    stdout: String::new(),
                    stderr: "discover: disabled by discover.disabled (SIGHUP)\n".to_string(),
                    code: 0,
                };
            }
            match cdp.pump(Instant::now() + Duration::from_millis(500)) {
                Ok(_) => {}
                Err(message) => {
                    if message.contains("closed") || message.contains("EOF") {
                        let stderr = format!("discover: connection lost: {message}");
                        return Output {
                            stdout: String::new(),
                            stderr,
                            code: 1,
                        };
                    }
                    continue;
                }
            }
            let events_out = process_events(&mut cdp, &mut state);
            if !events_out.is_empty() {
                eprint_flush(&events_out);
            }
            let load_events = check_load_events(&cdp, &state);
            for (session_id, _origin) in &load_events {
                let frame_tree_result = cdp
                    .call("Page.getFrameTree", json!({}), Some(session_id))
                    .ok();
                let committed_url = frame_tree_result
                    .as_ref()
                    .and_then(|r| {
                        r.pointer("/frameTree/frame/url")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                let committed_frame_id = frame_tree_result
                    .as_ref()
                    .and_then(|r| {
                        r.pointer("/frameTree/frame/id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                if let Some(ref url) = committed_url {
                    if let Some(sess) = state.sessions.get_mut(session_id.as_str()) {
                        sess.page_url = url.clone();
                    }
                }
                if let Some(ref fid) = committed_frame_id {
                    if let Some(sess) = state.sessions.get_mut(session_id.as_str()) {
                        sess.top_frame_id = Some(fid.clone());
                    }
                }
                let origin = state
                    .sessions
                    .get(session_id.as_str())
                    .and_then(|s| extract_origin(&s.page_url))
                    .unwrap_or_default();
                let frame_id = state
                    .sessions
                    .get(session_id.as_str())
                    .and_then(|s| s.top_frame_id.clone());
                let page_signals = collect_page_signals(
                    &mut cdp,
                    session_id,
                    frame_id.as_deref(),
                );
                let signals_out = emit_page_signals(&mut state, session_id, &page_signals);
                if !signals_out.is_empty() {
                    eprint_flush(&signals_out);
                }
                if !no_probe && !origin.is_empty() {
                    let probes_out = run_probes(&mut cdp, &mut state, session_id, &origin);
                    if !probes_out.is_empty() {
                        eprint_flush(&probes_out);
                    }
                }
            }
            attach_new_targets(&mut cdp, &mut state);
        }
    }

    #[cfg(not(unix))]
    loop {
        match cdp.pump(Instant::now() + Duration::from_millis(500)) {
            Ok(_) => {}
            Err(message) => {
                if message.contains("closed") || message.contains("EOF") {
                    let stderr = format!("discover: connection lost: {message}");
                    return Output {
                        stdout: String::new(),
                        stderr,
                        code: 1,
                    };
                }
                continue;
            }
        }
        let events_out = process_events(&mut cdp, &mut state);
        if !events_out.is_empty() {
            eprint_flush(&events_out);
        }
        let load_events = check_load_events(&cdp, &state);
            for (session_id, _origin) in &load_events {
                let frame_tree_result = cdp
                    .call("Page.getFrameTree", json!({}), Some(session_id))
                    .ok();
                let committed_url = frame_tree_result
                    .as_ref()
                    .and_then(|r| {
                        r.pointer("/frameTree/frame/url")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                let committed_frame_id = frame_tree_result
                    .as_ref()
                    .and_then(|r| {
                        r.pointer("/frameTree/frame/id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                if let Some(ref url) = committed_url {
                    if let Some(sess) = state.sessions.get_mut(session_id.as_str()) {
                        sess.page_url = url.clone();
                    }
                }
                if let Some(ref fid) = committed_frame_id {
                    if let Some(sess) = state.sessions.get_mut(session_id.as_str()) {
                        sess.top_frame_id = Some(fid.clone());
                    }
                }
                let origin = state
                    .sessions
                    .get(session_id.as_str())
                    .and_then(|s| extract_origin(&s.page_url))
                    .unwrap_or_default();
                let frame_id = state
                    .sessions
                    .get(session_id.as_str())
                    .and_then(|s| s.top_frame_id.clone());
                let page_signals = collect_page_signals(
                    &mut cdp,
                    session_id,
                    frame_id.as_deref(),
                );
                let signals_out = emit_page_signals(&mut state, session_id, &page_signals);
                if !signals_out.is_empty() {
                    eprint_flush(&signals_out);
                }
                if !no_probe && !origin.is_empty() {
                    let probes_out = run_probes(&mut cdp, &mut state, session_id, &origin);
                    if !probes_out.is_empty() {
                        eprint_flush(&probes_out);
                    }
                }
            }
        attach_new_targets(&mut cdp, &mut state);
    }
}

fn eprint_flush(text: &str) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let _ = handle.write_all(text.as_bytes());
    let _ = handle.flush();
}

fn check_load_events(cdp: &Cdp, state: &WatcherState) -> Vec<(String, String)> {
    let mut results = Vec::new();
    for event in cdp.events().iter().rev().take(50) {
        if event.get("method").and_then(|v| v.as_str()) != Some("Page.loadEventFired") {
            continue;
        }
        let session_id = event
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if let Some(sess) = state.sessions.get(session_id) {
            let origin = extract_origin(&sess.page_url).unwrap_or_default();
            if !results.iter().any(|(sid, _): &(String, String)| sid == session_id) {
                results.push((session_id.to_string(), origin));
            }
        }
    }
    results
}

fn resolve_once_tab(
    cdp: &mut Cdp,
    state: &mut session::Session,
    tab: Option<&str>,
) -> Result<String, String> {
    if let Some(tab_id) = tab {
        return Ok(tab_id.to_string());
    }
    let pages = browser::list_pages(cdp)?;
    let visible: Vec<&PageInfo> = pages.iter().collect();
    match state.current.as_deref() {
        Some(current) if visible.iter().any(|p| p.target_id == current) => Ok(current.to_string()),
        _ => visible
            .first()
            .map(|p| p.target_id.clone())
            .ok_or_else(|| "discover: no tabs available\n".to_string()),
    }
}

fn setup_session(cdp: &mut Cdp, session: &str) -> Result<(), String> {
    cdp.call("Page.enable", json!({}), Some(session))?;
    cdp.call("Network.enable", json!({}), Some(session))?;
    cdp.call("Runtime.enable", json!({}), Some(session))?;
    Ok(())
}

fn process_events(cdp: &mut Cdp, state: &mut WatcherState) -> String {
    let mut output = String::new();
    let events: Vec<Value> = cdp.events()[state.event_cursor..].to_vec();
    state.event_cursor = cdp.events().len();
    for event in &events {
        let method = event
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let session_id = event
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let params = event.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "Page.frameNavigated" => {
                handle_frame_navigated(state, session_id, &params);
            }
            "Target.targetCreated" => {
                handle_target_created(cdp, state, &params);
            }
            "Target.targetInfoChanged" => {
                handle_target_info_changed(cdp, state, &params);
            }
            "Target.targetDestroyed" => {
                handle_target_destroyed(state, &params);
            }
            _ => {}
        }
    }
    for event in &events {
        let method = event
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let session_id = event
            .get("sessionId")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let params = event.get("params").cloned().unwrap_or(Value::Null);

        if method == "Network.responseReceived" {
            output.push_str(&handle_response(cdp, state, session_id, &params));
        }
    }
    output
}

fn handle_frame_navigated(state: &mut WatcherState, session_id: &str, params: &Value) {
    let frame = params.get("frame").unwrap_or(&Value::Null);
    let frame_id = frame.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let parent_id = frame.get("parentId").and_then(|v| v.as_str());
    let url = frame.get("url").and_then(|v| v.as_str()).unwrap_or("");

    if parent_id.is_none() {
        if let Some(sess) = state.sessions.get_mut(session_id) {
            sess.top_frame_id = Some(frame_id.to_string());
            sess.page_url = url.to_string();
        }
    }
}

fn handle_target_created(cdp: &mut Cdp, state: &mut WatcherState, params: &Value) {
    let Some(info) = params.get("targetInfo") else { return };
    let Some(target_id) = info.get("targetId").and_then(|v| v.as_str()) else { return };
    if info.get("type").and_then(|v| v.as_str()) != Some("page") {
        return;
    }
    if cdp.is_protected_target(target_id) {
        return;
    }
    if let Ok(session) = cdp::attach(cdp, target_id) {
        if setup_session(cdp, &session).is_ok() {
            let frame_tree = cdp
                .call("Page.getFrameTree", json!({}), Some(&session))
                .ok()
                .and_then(|result| {
                    result
                        .pointer("/frameTree/frame/id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                });
            state.sessions.insert(
                session,
                SessionState {
                    target_id: target_id.to_string(),
                    top_frame_id: frame_tree,
                    page_url: info
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                },
            );
        }
    }
}

fn handle_target_info_changed(cdp: &mut Cdp, state: &mut WatcherState, params: &Value) {
    let Some(info) = params.get("targetInfo") else { return };
    let target_id = info
        .get("targetId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let url = info.get("url").and_then(|v| v.as_str()).unwrap_or("");
    if cdp.is_protected_target(target_id) {
        let sessions_to_remove: Vec<String> = state
            .sessions
            .iter()
            .filter(|(_, s)| s.target_id == target_id)
            .map(|(sid, _)| sid.clone())
            .collect();
        for sid in sessions_to_remove {
            cdp::detach(cdp, &sid);
            state.sessions.remove(&sid);
        }
    } else {
        for (_, sess) in state.sessions.iter_mut() {
            if sess.target_id == target_id {
                sess.page_url = url.to_string();
            }
        }
    }
}

fn handle_target_destroyed(state: &mut WatcherState, params: &Value) {
    let Some(target_id) = params.get("targetId").and_then(|v| v.as_str()) else { return };
    let sessions_to_remove: Vec<String> = state
        .sessions
        .iter()
        .filter(|(_, s)| s.target_id == target_id)
        .map(|(sid, _)| sid.clone())
        .collect();
    for sid in sessions_to_remove {
        state.sessions.remove(&sid);
    }
}

fn filter_protected_auth_urls(cdp: &Cdp, summary: &Value) -> Value {
    let mut s = summary.clone();
    if let Some(obj) = s.as_object_mut() {
        if let Some(schemes) = obj.get_mut("schemes").and_then(|v| v.as_array_mut()) {
            for scheme in schemes.iter_mut() {
                if let Some(m) = scheme.as_object_mut() {
                    for key in &["resource_metadata", "as_uri"] {
                        let drop = m.get(*key)
                            .and_then(|v| v.as_str())
                            .map_or(false, |u| cdp.guard().url(u));
                        if drop {
                            m.remove(*key);
                        }
                    }
                }
            }
        }
        if let Some(links) = obj.get_mut("links").and_then(|v| v.as_array_mut()) {
            links.retain(|link| {
                link.get("href")
                    .and_then(|v| v.as_str())
                    .map_or(true, |u| !cdp.guard().url(u))
            });
        }
    }
    s
}

fn filter_protected_ucp(cdp: &Cdp, summary: &Value) -> Value {
    let mut s = summary.clone();
    let protected = |v: &Value| v.as_str().is_some_and(|u| cdp.guard().url(u));
    if let Some(obj) = s.as_object_mut() {
        for key in ["mcp", "a2a"] {
            if let Some(list) = obj.get_mut(key).and_then(|v| v.as_array_mut()) {
                list.retain(|u| !protected(u));
            }
        }
        if let Some(services) = obj.get_mut("services").and_then(|v| v.as_object_mut()) {
            for entries in services.values_mut() {
                if let Some(list) = entries.as_array_mut() {
                    list.retain(|entry| !entry.get("endpoint").is_some_and(|u| protected(u)));
                }
            }
        }
    }
    s
}

fn handle_response(
    cdp: &mut Cdp,
    state: &mut WatcherState,
    session_id: &str,
    params: &Value,
) -> String {
    let mut output = String::new();
    let response = params.get("response").unwrap_or(&Value::Null);
    let resource_type = params
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let frame_id = params
        .get("frameId")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let is_top_frame_document = resource_type == "Document"
        && state
            .sessions
            .get(session_id)
            .and_then(|s| s.top_frame_id.as_deref())
            .map_or(false, |top| top == frame_id);

    let url = response
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let status = response
        .get("status")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u16;

    if !is_top_frame_document && status != 401 {
        return output;
    }

    if cdp.guard().url(&url) {
        return output;
    }

    if is_top_frame_document {
        if let Some(sess) = state.sessions.get_mut(session_id) {
            if sess.page_url.is_empty() || sess.page_url == "about:blank" {
                sess.page_url = url.clone();
            }
        }
    }

    let headers_obj = response
        .get("headers")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let origin = match extract_origin(&url) {
        Some(o) => o,
        None => return output,
    };

    if cdp.guard().url(&url) {
        return output;
    }

    let sess = match state.sessions.get(session_id) {
        Some(s) => s,
        None => return output,
    };
    let target_id = sess.target_id.clone();
    let page_url = sess.page_url.clone();

    let headers: Vec<(String, String)> = headers_obj
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
        .collect();

    if is_top_frame_document {
        let link_header_raw: String = headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("link"))
            .map(|(_, val)| val.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        if !link_header_raw.is_empty() {
            if let Ok(base) = Url::parse(&url) {
                let links = parse_link_header(&link_header_raw, &base);
                for link in links {
                    if !known_link_rels().contains(&link.rel.as_str()) {
                        continue;
                    }
                    if cdp.guard().url(&link.href) {
                        continue;
                    }

                    if is_slicc_handoff_rel(&link.rel) {
                        let dedup_key = format!("slicc-handoff:{}", link.href);
                        if !state.dedup.is_new(&origin, "slicc-handoff", &dedup_key) {
                            continue;
                        }
                        if let Some(summary) = classify_slicc_handoff(&link) {
                            let event = DiscoveryEvent {
                                v: EVENT_VERSION,
                                standard: "slicc-handoff".into(),
                                aliases: None,
                                origin: origin.clone(),
                                url: link.href.clone(),
                                via: Via::LinkHeader,
                                page: page_url.clone(),
                                target_id: target_id.clone(),
                                status: Validity::Valid,
                                summary,
                                reason: None,
                            };
                            output.push_str(&emit_event(state, &event));
                        }
                        continue;
                    }

                    let standard = if link.rel == LLMS_TXT_REL {
                        "llms-txt".to_string()
                    } else {
                        link.rel.clone()
                    };

                    if !state.dedup.is_new(&origin, &standard, &link.href) {
                        continue;
                    }
                    let event = DiscoveryEvent {
                        v: EVENT_VERSION,
                        standard,
                        aliases: None,
                        origin: origin.clone(),
                        url: link.href.clone(),
                        via: Via::LinkHeader,
                        page: page_url.clone(),
                        target_id: target_id.clone(),
                        status: Validity::Valid,
                        summary: json!({ "rel": link.rel, "href": link.href, "type": link.media_type }),
                        reason: None,
                    };
                    output.push_str(&emit_event(state, &event));
                }
            }
        }
    }

    let response_signals = classify_response(status, &headers, Some(&url));
    for signal in response_signals {
        if !state.dedup.is_new(&origin, &signal.standard, &url) {
            continue;
        }
        let filtered_summary = filter_protected_auth_urls(cdp, &signal.summary);
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: signal.standard.clone(),
            aliases: None,
            origin: origin.clone(),
            url: url.clone(),
            via: Via::Response,
            page: page_url.clone(),
            target_id: target_id.clone(),
            status: Validity::Valid,
            summary: filtered_summary,
            reason: None,
        };
        output.push_str(&emit_event(state, &event));
        if signal.standard == "auth-md" && !state.no_probe && !state.probed_origins.contains(&origin) {
            output.push_str(&run_probes(cdp, state, session_id, &origin));
        }
    }

    output
}

fn collect_page_signals(cdp: &mut Cdp, session: &str, frame_id: Option<&str>) -> Vec<(String, Value)> {
    let mut signals = Vec::new();
    let js = r#"(function() {
  var r = {};
  var links = [];
  var els = document.querySelectorAll('link[rel]');
  for (var i = 0; i < els.length; i++) {
    links.push({ rel: els[i].getAttribute('rel'), href: els[i].href });
  }
  if (links.length) r.links = links;
  return JSON.stringify(r);
})()
"#;
    let webmcp_js = r#"(function() {
  var r = {};
  if (typeof document !== 'undefined' && document.modelContext) {
    r.webmcp = true;
    if (typeof document.modelContext.getTools === 'function') {
      try {
        var p = document.modelContext.getTools();
        if (p && typeof p.then === 'function') {
          return p.then(function(tools) {
            r.tools = tools.map(function(t) { return { name: t.name, description: t.description || '' }; });
            return JSON.stringify(r);
          });
        }
        r.tools = p.map(function(t) { return { name: t.name, description: t.description || '' }; });
      } catch(e) {}
    }
  }
  return JSON.stringify(r);
})()
"#;
    let parsed = eval_in_isolated_world(cdp, session, frame_id, js);
    let webmcp_parsed = eval_in_page_context(cdp, session, webmcp_js);
    if webmcp_parsed.get("webmcp").and_then(|v| v.as_bool()) == Some(true) {
        let tools = webmcp_parsed.get("tools").cloned().unwrap_or(json!([]));
        let has_tools = tools.as_array().map_or(false, |a| !a.is_empty());
        if has_tools {
            signals.push(("webmcp".to_string(), json!({ "tools": tools })));
        }
    }
    if let Some(links) = parsed.get("links").and_then(|v| v.as_array()) {
        for link in links {
            let rel = link
                .get("rel")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let href = link
                .get("href")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if known_link_rels().contains(&rel.as_str()) && !href.is_empty() && !cdp.guard().url(&href) {
                signals.push((
                    "html-link".to_string(),
                    json!({ "rel": rel, "href": href }),
                ));
            }
        }
    }
    signals
}

fn eval_in_isolated_world(
    cdp: &mut Cdp,
    session: &str,
    frame_id: Option<&str>,
    js: &str,
) -> Value {
    let fid = match frame_id {
        Some(f) => f.to_string(),
        None => {
            return eval_in_page_context(cdp, session, js);
        }
    };
    for _attempt in 0..2 {
        let world = match cdp.call(
            "Page.createIsolatedWorld",
            json!({ "frameId": fid, "worldName": "__slicc_discover", "grantUniveralAccess": true }),
            Some(session),
        ) {
            Ok(w) => w,
            Err(_) => continue,
        };
        let ctx_id = match world.get("executionContextId").and_then(|v| v.as_u64()) {
            Some(id) => id,
            None => continue,
        };
        let result = cdp.call(
            "Runtime.evaluate",
            json!({
                "expression": js,
                "contextId": ctx_id,
                "returnByValue": true,
                "awaitPromise": true
            }),
            Some(session),
        );
        match result {
            Ok(ref r) => {
                let err_msg = r
                    .pointer("/exceptionDetails/exception/description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if err_msg.contains("Cannot find context") || err_msg.contains("destroyed") {
                    continue;
                }
                let value_str = r
                    .pointer("/result/value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                return serde_json::from_str::<Value>(value_str)
                    .unwrap_or(Value::Object(serde_json::Map::new()));
            }
            Err(e) => {
                if e.contains("Cannot find context") || e.contains("destroyed") {
                    continue;
                }
                return Value::Object(serde_json::Map::new());
            }
        }
    }
    Value::Object(serde_json::Map::new())
}

fn eval_in_page_context(cdp: &mut Cdp, session: &str, js: &str) -> Value {
    let result = cdp.call(
        "Runtime.evaluate",
        json!({ "expression": js, "returnByValue": true, "awaitPromise": true }),
        Some(session),
    );
    match result {
        Ok(ref r) => {
            let value_str = r
                .pointer("/result/value")
                .and_then(|v| v.as_str())
                .unwrap_or("{}");
            serde_json::from_str::<Value>(value_str)
                .unwrap_or(Value::Object(serde_json::Map::new()))
        }
        Err(_) => Value::Object(serde_json::Map::new()),
    }
}

fn emit_page_signals(
    state: &mut WatcherState,
    session_id: &str,
    signals: &[(String, Value)],
) -> String {
    let mut output = String::new();
    let sess = match state.sessions.get(session_id) {
        Some(s) => s,
        None => return output,
    };
    let origin = extract_origin(&sess.page_url).unwrap_or_default();
    if origin.is_empty() {
        return output;
    }
    let page_url = sess.page_url.clone();
    let target_id = sess.target_id.clone();

    for (kind, summary) in signals {
        let (standard, via, dedup_url) = match kind.as_str() {
            "webmcp" => ("webmcp".to_string(), Via::PageApi, page_url.clone()),
            "html-link" => {
                let rel = summary
                    .get("rel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let href = summary
                    .get("href")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                (rel, Via::HtmlLink, href)
            }
            _ => continue,
        };
        if !state.dedup.is_new(&origin, &standard, &dedup_url) {
            continue;
        }
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard,
            aliases: None,
            origin: origin.clone(),
            url: dedup_url,
            via,
            page: page_url.clone(),
            target_id: target_id.clone(),
            status: Validity::Valid,
            summary: summary.clone(),
            reason: None,
        };
        output.push_str(&emit_event(state, &event));
    }
    output
}

fn run_probes(
    cdp: &mut Cdp,
    state: &mut WatcherState,
    session_id: &str,
    origin: &str,
) -> String {
    let mut output = String::new();
    if state.probed_origins.contains(origin) {
        return output;
    }
    state.probed_origins.insert(origin.to_string());

    if cdp.guard().url(origin) {
        return output;
    }

    let sess = match state.sessions.get(session_id) {
        Some(s) => s,
        None => return output,
    };
    let frame_id = match &sess.top_frame_id {
        Some(id) => id.clone(),
        None => return output,
    };
    let page_url = sess.page_url.clone();
    let target_id = sess.target_id.clone();

    let probe_requests = state.probes.probes_for_origin(origin);
    let mut ard_body: Option<String> = None;
    let mut ai_catalog_body: Option<String> = None;
    struct PendingProbe {
        standard: String,
        url: String,
        body: String,
    }
    let mut pending: Vec<PendingProbe> = Vec::new();
    for probe_req in &probe_requests {
        if cdp.guard().url(&probe_req.url) {
            continue;
        }

        let probe_result = isolated_world_probe(cdp, session_id, &frame_id, &probe_req.url);
        let (status, content_type, body) = match probe_result {
            IsolatedProbeResult::Ok(s, ct, b) => (s, ct, b),
            IsolatedProbeResult::Final => {
                if probe_req.probe.standard == "poppy" {
                    match poppy_redirect_probe(&probe_req.url, &probe_req.probe.accept, origin, cdp) {
                        Some((s, ct, b)) => (s, ct, b),
                        None => continue,
                    }
                } else {
                    continue
                }
            }
            IsolatedProbeResult::Unavailable => {
                if probe_req.probe.standard == "poppy" {
                    match poppy_redirect_probe(&probe_req.url, &probe_req.probe.accept, origin, cdp) {
                        Some((s, ct, b)) => (s, ct, b),
                        None => continue,
                    }
                } else {
                    match native_probe(&probe_req.url, &probe_req.probe.accept) {
                        Some((ns, nct, nbody)) => (ns, nct, nbody),
                        None => continue,
                    }
                }
            }
        };

        let probe_match_result =
            state
                .probes
                .check_response(&probe_req.probe.standard, status, &content_type, body, probe_req.url.clone());

        if let Some(probe_match) = probe_match_result
        {
            if probe_match.probe.standard == "ard" {
                ard_body = Some(probe_match.body.clone());
            } else if probe_match.probe.standard == "ai-catalog" {
                ai_catalog_body = Some(probe_match.body.clone());
            }
            pending.push(PendingProbe {
                standard: probe_match.probe.standard.clone(),
                url: probe_match.url,
                body: probe_match.body,
            });
        }
    }

    let alias_merge = match (&ard_body, &ai_catalog_body) {
        (Some(a), Some(b)) if a == b => true,
        _ => false,
    };

    for p in &pending {
        if alias_merge && p.standard == "ai-catalog" {
            continue;
        }
        let classification = if p.standard == "ard" {
            Some(classify_ard_body(&p.body, Some(origin)))
        } else if p.standard == "ai-catalog" {
            Some(classify_ai_catalog_body(&p.body, Some(origin)))
        } else if p.standard == "poppy" {
            Some(classify_poppy_body(&p.body, Some(origin)))
        } else {
            state.classifiers.classify(&p.standard, &p.body)
        };
        let (validity, summary, reason) = match classification {
            Some(c) => (c.status, c.summary, c.reason),
            None => (
                Validity::Valid,
                json!({ "url": p.url }),
                None,
            ),
        };
        let summary = if p.standard == "ucp" { filter_protected_ucp(cdp, &summary) } else { summary };
        if !state.dedup.is_new(origin, &p.standard, &p.url) {
            continue;
        }
        let aliases = if alias_merge && p.standard == "ard" {
            Some(vec!["ai-catalog".to_string()])
        } else {
            None
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: p.standard.clone(),
            aliases,
            origin: origin.to_string(),
            url: p.url.clone(),
            via: Via::WellKnown,
            page: page_url.clone(),
            target_id: target_id.clone(),
            status: validity,
            summary: summary.clone(),
            reason,
        };
        output.push_str(&emit_event(state, &event));
        if event.standard == "ard" || event.standard == "ai-catalog" {
            output.push_str(&emit_nlweb_from_catalog(state, origin, &page_url, &target_id, &summary));
        }
        if state.follow_mode && (event.standard == "ard" || event.standard == "ai-catalog") {
            output.push_str(&follow_ard_catalogs(
                cdp, state, session_id, origin, &frame_id, &page_url, &target_id, &summary,
            ));
        }
    }

    if state.robots_probe {
        let robots_url = format!("{origin}/robots.txt");
        if !cdp.guard().url(&robots_url) {
            let robots_result = isolated_world_probe(cdp, session_id, &frame_id, &robots_url);
            let (r_status, r_ct, r_body) = match robots_result {
                IsolatedProbeResult::Ok(s, ct, b) => (s, ct, b),
                IsolatedProbeResult::Final => (0, String::new(), String::new()),
                IsolatedProbeResult::Unavailable => {
                    match native_probe(&robots_url, "text/plain") {
                        Some((s, ct, b)) => (s, ct, b),
                        None => (0, String::new(), String::new()),
                    }
                }
            };
            if r_status == 200 && r_ct.starts_with("text/") {
                let agentmap_urls = parse_robots_txt_agentmap(&r_body);
                for am_url in agentmap_urls {
                    let resolved = match Url::parse(&am_url).or_else(|_| Url::parse(origin).and_then(|base| base.join(&am_url))) {
                        Ok(u) => u.to_string(),
                        Err(_) => continue,
                    };
                    if cdp.guard().url(&resolved) {
                        continue;
                    }
                    if !state.dedup.is_new(origin, "ard", &resolved) {
                        continue;
                    }
                    let am_result = isolated_world_probe(cdp, session_id, &frame_id, &resolved);
                    let (am_status, am_ct, am_body) = match am_result {
                        IsolatedProbeResult::Ok(s, ct, b) => (s, ct, b),
                        IsolatedProbeResult::Final => continue,
                        IsolatedProbeResult::Unavailable => {
                            match native_probe(&resolved, "application/json") {
                                Some((s, ct, b)) => (s, ct, b),
                                None => continue,
                            }
                        }
                    };
                    if am_status == 200 && is_json_content_type(&am_ct) {
                        let classification = classify_ard_body(&am_body, Some(origin));
                        let summary_clone = classification.summary.clone();
                        let event = DiscoveryEvent {
                            v: EVENT_VERSION,
                            standard: "ard".into(),
                            aliases: None,
                            origin: origin.to_string(),
                            url: resolved,
                            via: Via::WellKnown,
                            page: page_url.clone(),
                            target_id: target_id.clone(),
                            status: classification.status,
                            summary: classification.summary,
                            reason: classification.reason,
                        };
                        output.push_str(&emit_event(state, &event));
                        output.push_str(&emit_nlweb_from_catalog(state, origin, &page_url, &target_id, &summary_clone));
                        if state.follow_mode {
                            output.push_str(&follow_ard_catalogs(
                                cdp, state, session_id, origin, &frame_id, &page_url, &target_id, &summary_clone,
                            ));
                        }
                    }
                }
            }
        }
    }

    if state.nlweb_probe {
        output.push_str(&run_nlweb_probes(cdp, state, session_id, origin));
    }

    output
}

#[cfg(not(target_os = "wasi"))]
fn poppy_redirect_probe(
    url: &str,
    accept: &str,
    origin: &str,
    cdp: &Cdp,
) -> Option<(u16, String, String)> {
    let resp = cdp_client::net::http_probe(url, accept).ok()?;
    if resp.status == 200 {
        let ct = resp.content_type().to_string();
        let body = String::from_utf8_lossy(&resp.body).into_owned();
        return Some((resp.status, ct, body));
    }
    if !(300..400).contains(&resp.status) {
        return None;
    }
    let location = resp
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
        .map(|(_, value)| value.clone())?;
    let resolved = Url::parse(&location)
        .or_else(|_| Url::parse(url).and_then(|base| base.join(&location)))
        .ok()?;
    let resolved_str = resolved.to_string();
    let resolved_host = resolved.host_str()?.to_ascii_lowercase();
    let origin_parsed = Url::parse(origin).ok()?;
    let origin_host = origin_parsed.host_str()?.to_ascii_lowercase();
    let same_site = match (registrable_domain(&resolved_host), registrable_domain(&origin_host)) {
        (Some(r), Some(o)) => r == o,
        _ => resolved_host == origin_host,
    };
    if !same_site {
        return None;
    }
    if cdp.guard().url(&resolved_str) {
        return None;
    }
    let resp2 = cdp_client::net::http_probe(&resolved_str, accept).ok()?;
    let ct = resp2.content_type().to_string();
    let body = String::from_utf8_lossy(&resp2.body).into_owned();
    Some((resp2.status, ct, body))
}

#[cfg(target_os = "wasi")]
fn poppy_redirect_probe(
    _url: &str,
    _accept: &str,
    _origin: &str,
    _cdp: &Cdp,
) -> Option<(u16, String, String)> {
    None
}

enum IsolatedProbeResult {
    Ok(u16, String, String),
    Final,
    Unavailable,
}

#[derive(Default)]
struct FetchInit<'a> {
    method: Option<&'a str>,
    headers: &'a [(&'a str, &'a str)],
    body: Option<&'a str>,
    sse_until: Option<&'a str>,
}

const PLAIN_GET: FetchInit<'static> = FetchInit {
    method: None,
    headers: &[],
    body: None,
    sse_until: None,
};

fn fetch_options(init: &FetchInit) -> Value {
    let mut options = json!({ "credentials": "omit", "redirect": "manual", "cache": "no-store" });
    if let Some(method) = init.method {
        options["method"] = json!(method);
    }
    if !init.headers.is_empty() {
        let headers: serde_json::Map<String, Value> = init
            .headers
            .iter()
            .map(|(name, value)| (name.to_string(), json!(value)))
            .collect();
        options["headers"] = Value::Object(headers);
    }
    if let Some(body) = init.body {
        options["body"] = json!(body);
    }
    options
}

fn isolated_world_probe(
    cdp: &mut Cdp,
    session_id: &str,
    frame_id: &str,
    url: &str,
) -> IsolatedProbeResult {
    isolated_world_fetch(cdp, session_id, frame_id, url, &PLAIN_GET)
}

fn isolated_world_fetch(
    cdp: &mut Cdp,
    session_id: &str,
    frame_id: &str,
    url: &str,
    init: &FetchInit,
) -> IsolatedProbeResult {
    let world = match cdp.call(
        "Page.createIsolatedWorld",
        json!({ "frameId": frame_id, "grantUniveralAccess": true }),
        Some(session_id),
    ) {
        Ok(w) => w,
        Err(_) => return IsolatedProbeResult::Unavailable,
    };
    let execution_context_id = match world
        .get("executionContextId")
        .and_then(|v| v.as_u64())
    {
        Some(id) => id,
        None => return IsolatedProbeResult::Unavailable,
    };
    let js = format!(
        r#"(async () => {{
  const cap = 1048576;
  const sseUntil = {sse_until};
  const carries = (message) => message !== null && typeof message === 'object' && Object.hasOwn(message, sseUntil);
  const sseDone = (text) => text.replace(/\r\n?/g, '\n').split('\n\n').slice(0, -1).some((event) => {{
    const data = event.split('\n').filter((line) => line.startsWith('data:')).map((line) => line.slice(5).replace(/^ /, '')).join('\n');
    try {{
      const message = JSON.parse(data);
      return Array.isArray(message) ? message.some(carries) : carries(message);
    }} catch (e) {{ return false; }}
  }});
  let r;
  try {{
    r = await fetch({url}, {options});
  }} catch(e) {{ return JSON.stringify({{err:e.message}}); }}
  if (r.type === 'opaqueredirect') return JSON.stringify({{err:'redirect'}});
  const ct = r.headers.get('content-type') || '';
  let body = '';
  if (r.body) {{
    const reader = r.body.getReader();
    const dec = new TextDecoder();
    let total = 0;
    while (true) {{
      const {{done, value}} = await reader.read();
      if (done) break;
      total += value.byteLength;
      body += dec.decode(value, {{stream:true}});
      if (total > cap) {{ reader.cancel(); body = body.slice(0, cap); break; }}
      if (sseUntil && /text\/event-stream/i.test(ct) && sseDone(body)) {{ reader.cancel(); break; }}
    }}
  }}
  return JSON.stringify({{status:r.status,ct,body}});
}})()
"#,
        url = serde_json::to_string(url).unwrap_or_default(),
        options = fetch_options(init),
        sse_until = json!(init.sse_until)
    );
    let result = match cdp.call_for(
        "Runtime.evaluate",
        json!({
            "expression": js,
            "contextId": execution_context_id,
            "returnByValue": true,
            "awaitPromise": true
        }),
        Some(session_id),
        Duration::from_secs(5),
    ) {
        Ok(r) => r,
        Err(_) => return IsolatedProbeResult::Final,
    };
    let value_str = result
        .pointer("/result/value")
        .and_then(|v| v.as_str())
        .unwrap_or("{}");
    let parsed: Value = match serde_json::from_str(value_str) {
        Ok(v) => v,
        Err(_) => return IsolatedProbeResult::Final,
    };
    if parsed.get("err").is_some() {
        return IsolatedProbeResult::Final;
    }
    let status = parsed.get("status").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
    let ct = parsed
        .get("ct")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let body = parsed
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    IsolatedProbeResult::Ok(status, ct, body)
}

#[cfg(not(target_os = "wasi"))]
fn native_probe(url: &str, accept: &str) -> Option<(u16, String, String)> {
    let resp = cdp_client::net::http_probe(url, accept).ok()?;
    let ct = resp.content_type().to_string();
    let body = String::from_utf8_lossy(&resp.body).into_owned();
    Some((resp.status, ct, body))
}

#[cfg(target_os = "wasi")]
fn native_probe(_url: &str, _accept: &str) -> Option<(u16, String, String)> {
    None
}

fn attach_new_targets(cdp: &mut Cdp, state: &mut WatcherState) {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(_) => return,
    };
    let known_targets: BTreeSet<String> =
        state.sessions.values().map(|s| s.target_id.clone()).collect();
    for page in &pages {
        if known_targets.contains(&page.target_id) {
            continue;
        }
        if cdp.is_protected_target(&page.target_id) {
            continue;
        }
        if let Ok(session) = cdp::attach(cdp, &page.target_id) {
            if setup_session(cdp, &session).is_ok() {
                let frame_tree = cdp
                    .call("Page.getFrameTree", json!({}), Some(&session))
                    .ok()
                    .and_then(|result| {
                        result
                            .pointer("/frameTree/frame/id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                state.sessions.insert(
                    session,
                    SessionState {
                        target_id: page.target_id.clone(),
                        top_frame_id: frame_tree,
                        page_url: page.url.clone(),
                    },
                );
            }
        }
    }
}

fn strip_userinfo(url_str: &str) -> String {
    match Url::parse(url_str) {
        Ok(mut parsed) => {
            if parsed.username().is_empty() && parsed.password().is_none() {
                return url_str.to_string();
            }
            let _ = parsed.set_username("");
            let _ = parsed.set_password(None);
            parsed.to_string()
        }
        Err(_) => url_str.to_string(),
    }
}

fn sanitize_event_urls(event: &mut DiscoveryEvent) {
    event.url = strip_userinfo(&event.url);
    event.origin = strip_userinfo(&event.origin);
    event.page = strip_userinfo(&event.page);
    if let Some(obj) = event.summary.as_object_mut() {
        for key in ["url", "href"] {
            if let Some(val) = obj.get(key).and_then(|v| v.as_str()).map(|s| strip_userinfo(s)) {
                obj.insert(key.to_string(), Value::String(val));
            }
        }
    }
}

fn safe_json_str(s: &str) -> String {
    crate::sanitize::json_c1(s)
}

fn redact_event(event: &DiscoveryEvent) -> DiscoveryEvent {
    let mut redacted = event.clone();
    redacted.url = crate::sanitize::url(&redacted.url);
    redacted.page = crate::sanitize::url(&redacted.page);
    redacted.summary = cdp_client::sanitize::redact_json_urls(&redacted.summary);
    redacted
}
fn emit_event(state: &mut WatcherState, event: &DiscoveryEvent) -> String {
    let mut event = event.clone();
    sanitize_event_urls(&mut event);
    state.events_emitted += 1;
    let display = if crate::sanitize::full_urls_enabled() { event.clone() } else { redact_event(&event) };
    let json_line = format!("{}\n", safe_json_str(&display.to_json()));
    match &state.sink {
        Sink::Exec { cmd, timeout } => {
            run_exec_sink(cmd, *timeout, &display);
        }
        Sink::Lick { slicc_dir } => {
            write_lick_file(slicc_dir, &event, &display);
        }
        Sink::Stdout => {}
    }
    match &state.sink {
        Sink::Stdout => json_line,
        _ => {
            if state.json_mode {
                json_line
            } else {
                String::new()
            }
        }
    }
}

fn ensure_lick_dirs(slicc_dir: &str) {
    let discoveries = format!("{slicc_dir}/discoveries");
    let watches = format!("{slicc_dir}/watches");
    let _ = std::fs::create_dir_all(&discoveries);
    let _ = std::fs::create_dir_all(&watches);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for dir in [slicc_dir, &discoveries, &watches] {
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let watch_file = format!("{watches}/discover.json");
    if !std::path::Path::new(&watch_file).exists() {
        let watch_json = json!({
            "path": format!("{slicc_dir}/discoveries"),
            "glob": "*.json",
            "target": "cone",
            "message": "browser discovery found an agent-facing web standard",
            "debounce": 500,
            "triage": "a web page offers an agent-facing standard; decide whether to use it"
        });
        let content = serde_json::to_string_pretty(&watch_json).unwrap_or_default();
        atomic_write(&watch_file, content.as_bytes());
    }
}

fn atomic_write(path: &str, data: &[u8]) {
    let dir = std::path::Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    if dir.is_empty() {
        return;
    }
    let unique = format!(
        "{dir}/.tmp.{}.{}",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    );
    match std::fs::write(&unique, data) {
        Ok(()) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&unique, std::fs::Permissions::from_mode(0o600));
            }
            if let Err(e) = std::fs::rename(&unique, path) {
                eprintln!("discover: rename {unique} -> {path}: {e}");
                let _ = std::fs::remove_file(&unique);
            }
        }
        Err(e) => {
            eprintln!("discover: write {unique}: {e}");
            let _ = std::fs::remove_file(&unique);
        }
    }
}

fn write_lick_file(slicc_dir: &str, original: &DiscoveryEvent, display: &DiscoveryEvent) {
    let fp = fingerprint(&original.origin, &original.standard, &original.url);
    let hash = cdp_client::hash::sha256_hex(fp.as_bytes());
    let path = format!("{slicc_dir}/discoveries/{hash}.json");
    let mut obj = json!({
        "v": display.v,
        "standard": display.standard,
        "origin": display.origin,
        "url": display.url,
        "via": display.via,
        "page": display.page,
        "targetId": display.target_id,
        "status": display.status,
        "summary": display.summary,
    });
    if let Some(ref aliases) = display.aliases {
        obj["aliases"] = json!(aliases);
    }
    let raw = serde_json::to_string_pretty(&obj)
    .unwrap_or_default();
    let content = safe_json_str(&raw);
    atomic_write(&path, content.as_bytes());
}

#[cfg(unix)]
fn run_exec_sink(cmd: &str, timeout: Duration, event: &DiscoveryEvent) {
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let json = safe_json_str(&event.to_json());
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .env("SLICC_DISCOVER_STANDARD", &event.standard)
        .env("SLICC_DISCOVER_ORIGIN", &event.origin)
        .env("SLICC_DISCOVER_URL", &event.url);
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("discover: --exec failed to start: {e}");
            return;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(json.as_bytes());
        drop(stdin);
    }

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    eprintln!(
                        "discover: --exec exited with {}",
                        status.code().unwrap_or(-1)
                    );
                }
                return;
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_process_group(&child);
                    let _ = child.kill();
                    let _ = child.wait();
                    eprintln!("discover: --exec timed out after {}s", timeout.as_secs());
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                eprintln!("discover: --exec wait error: {e}");
                return;
            }
        }
    }
}

#[cfg(unix)]
fn kill_process_group(child: &std::process::Child) {
    let pid = child.id() as i32;
    unsafe {
        let ret = libc::kill(-pid, libc::SIGKILL);
        if ret != 0 {
            eprintln!(
                "discover: kill process group failed: {}",
                std::io::Error::last_os_error()
            );
        }
    }
}

#[cfg(not(unix))]
fn run_exec_sink(_cmd: &str, _timeout: Duration, _event: &DiscoveryEvent) {}

fn follow_links(
    cdp: &mut Cdp,
    state: &mut WatcherState,
    session_id: &str,
) -> String {
    let mut output = String::new();
    let sess = match state.sessions.get(session_id) {
        Some(s) => s,
        None => return output,
    };
    let frame_id = match &sess.top_frame_id {
        Some(id) => id.clone(),
        None => return output,
    };
    let page_url = sess.page_url.clone();
    let target_id = sess.target_id.clone();
    let origin = extract_origin(&page_url).unwrap_or_default();
    if origin.is_empty() {
        return output;
    }

    let link_header_raw = {
        let mut raw = String::new();
        for event in cdp.events().iter().rev().take(200) {
            let method = event.get("method").and_then(|v| v.as_str()).unwrap_or("");
            if method != "Network.responseReceived" {
                continue;
            }
            let sid = event.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
            if sid != session_id {
                continue;
            }
            let resp = event.get("params")
                .and_then(|p| p.get("response"));
            if let Some(resp) = resp {
                if let Some(headers) = resp.get("headers").and_then(|h| h.as_object()) {
                    for (name, val) in headers {
                        if name.eq_ignore_ascii_case("link") {
                            if !raw.is_empty() {
                                raw.push('\n');
                            }
                            raw.push_str(val.as_str().unwrap_or(""));
                        }
                    }
                }
            }
            break;
        }
        raw
    };

    if link_header_raw.is_empty() {
        return output;
    }

    let base = match Url::parse(&page_url) {
        Ok(b) => b,
        Err(_) => return output,
    };
    let links = parse_link_header(&link_header_raw, &base);
    let followable: Vec<_> = links.iter()
        .filter(|l| {
            let rels = known_link_rels();
            rels.contains(&l.rel.as_str()) && !is_slicc_handoff_rel(&l.rel)
        })
        .collect();

    let mut seen_rels = BTreeSet::new();
    let mut failures: Vec<Value> = Vec::new();

    for link in followable {
        let standard = if link.rel == LLMS_TXT_REL {
            "llms-txt".to_string()
        } else {
            link.rel.clone()
        };
        if !seen_rels.insert(standard.clone()) {
            continue;
        }
        if cdp.guard().url(&link.href) {
            continue;
        }

        let probe_result = isolated_world_probe(cdp, session_id, &frame_id, &link.href);
        let (status, content_type, body) = match probe_result {
            IsolatedProbeResult::Ok(s, ct, b) => (s, ct, b),
            IsolatedProbeResult::Final | IsolatedProbeResult::Unavailable => {
                failures.push(json!({
                    "rel": link.rel,
                    "href": link.href,
                    "error": "fetch failed"
                }));
                continue;
            }
        };

        if status < 200 || status >= 300 {
            failures.push(json!({
                "rel": link.rel,
                "href": link.href,
                "error": format!("HTTP {status}")
            }));
            continue;
        }

        let dedup_key = format!("follow:{}", link.href);
        if !state.dedup.is_new(&origin, &standard, &dedup_key) {
            continue;
        }

        let classification = if standard == "ard" {
            Some(classify_ard_body(&body, Some(&origin)))
        } else if standard == "ai-catalog" {
            Some(classify_ai_catalog_body(&body, Some(&origin)))
        } else {
            state.classifiers.classify(&standard, &body)
        };
        let (validity, summary, reason) = match classification {
            Some(c) => (c.status, c.summary.clone(), c.reason),
            None => (
                Validity::Valid,
                json!({
                    "rel": link.rel,
                    "href": link.href,
                    "type": link.media_type,
                    "contentType": content_type
                }),
                None,
            ),
        };

        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: standard.clone(),
            aliases: None,
            origin: origin.clone(),
            url: link.href.clone(),
            via: Via::LinkHeader,
            page: page_url.clone(),
            target_id: target_id.clone(),
            status: validity,
            summary: summary.clone(),
            reason,
        };
        output.push_str(&emit_event(state, &event));
        if standard == "ard" || standard == "ai-catalog" {
            output.push_str(&emit_nlweb_from_catalog(state, &origin, &page_url, &target_id, &summary));
        }
        if (standard == "ard" || standard == "ai-catalog") && state.follow_mode {
            output.push_str(&follow_ard_catalogs(
                cdp, state, session_id, &origin, &frame_id, &page_url, &target_id, &summary,
            ));
        }
    }

    if !failures.is_empty() {
        let dedup_key = format!("follow-failures:{origin}");
        if state.dedup.is_new(&origin, "follow-failures", &dedup_key) {
            let event = DiscoveryEvent {
                v: EVENT_VERSION,
                standard: "follow-failures".into(),
                aliases: None,
                origin: origin.clone(),
                url: page_url.clone(),
                via: Via::LinkHeader,
                page: page_url.clone(),
                target_id: target_id.clone(),
                status: Validity::Valid,
                summary: json!({ "failures": failures }),
                reason: None,
            };
            output.push_str(&emit_event(state, &event));
        }
    }

    output
}

fn emit_nlweb_from_catalog(
    state: &mut WatcherState,
    origin: &str,
    page_url: &str,
    target_id: &str,
    summary: &Value,
) -> String {
    let mut output = String::new();
    let nlweb = match discovery::detect_nlweb_entries(summary) {
        Some(s) => s,
        None => return output,
    };
    let ask_url = nlweb.get("askUrl").and_then(|v| v.as_str()).unwrap_or("");
    let mcp_url = nlweb.get("mcpUrl").and_then(|v| v.as_str()).unwrap_or("");
    let dedup_key = format!("nlweb-catalog:{}{}", ask_url, mcp_url);
    if !state.dedup.is_new(origin, "nlweb", &dedup_key) {
        return output;
    }
    let url = if !ask_url.is_empty() { ask_url.to_string() } else { mcp_url.to_string() };
    let event = DiscoveryEvent {
        v: EVENT_VERSION,
        standard: "nlweb".into(),
        aliases: None,
        origin: origin.to_string(),
        url,
        via: Via::Catalog,
        page: page_url.to_string(),
        target_id: target_id.to_string(),
        status: Validity::Valid,
        summary: nlweb,
        reason: None,
    };
    output.push_str(&emit_event(state, &event));
    output
}

fn run_nlweb_probes(
    cdp: &mut Cdp,
    state: &mut WatcherState,
    session_id: &str,
    origin: &str,
) -> String {
    let mut output = String::new();
    let dedup_key = format!("nlweb-probe:{origin}");
    if !state.dedup.is_new(origin, "nlweb-probe", &dedup_key) {
        return output;
    }
    if cdp.guard().url(origin) {
        return output;
    }
    let sess = match state.sessions.get(session_id) {
        Some(s) => s,
        None => return output,
    };
    let frame_id = match &sess.top_frame_id {
        Some(id) => id.clone(),
        None => return output,
    };
    let page_url = sess.page_url.clone();
    let target_id = sess.target_id.clone();
    let ask_url = format!("{}/ask", origin.trim_end_matches('/'));
    let ask_probe = format!("{ask_url}?query=nlweb&streaming=false");
    let ask_body = match isolated_world_fetch(cdp, session_id, &frame_id, &ask_probe, &NLWEB_ASK) {
        IsolatedProbeResult::Ok(status, ct, body) => json_ok_body(status, &ct, body),
        IsolatedProbeResult::Final => None,
        IsolatedProbeResult::Unavailable => native_probe(&ask_probe, "application/json")
            .and_then(|(status, ct, body)| json_ok_body(status, &ct, body)),
    };
    let ask_summary = ask_body.as_deref().and_then(classify_nlweb_ask);
    let mcp_url = format!("{}/mcp", origin.trim_end_matches('/'));
    let init_body = nlweb_mcp_init_body();
    let init = FetchInit {
        method: Some("POST"),
        headers: NLWEB_MCP_HEADERS,
        body: Some(&init_body),
        sse_until: Some("result"),
    };
    let mcp_body = match isolated_world_fetch(cdp, session_id, &frame_id, &mcp_url, &init) {
        IsolatedProbeResult::Ok(status, ct, body) => mcp_init_payload(status, &ct, body),
        IsolatedProbeResult::Final => None,
        IsolatedProbeResult::Unavailable => nlweb_mcp_probe_native(&mcp_url, &init_body),
    };
    let mcp_summary = mcp_body.as_deref().and_then(classify_nlweb_mcp_init);
    if ask_summary.is_none() && mcp_summary.is_none() {
        return output;
    }
    let mut map = serde_json::Map::new();
    if ask_summary.is_some() {
        map.insert("askUrl".into(), Value::String(ask_url.clone()));
    }
    if mcp_summary.is_some() {
        map.insert("mcpUrl".into(), Value::String(mcp_url.clone()));
    }
    if let Some(name) = ask_summary.as_ref().and_then(|ask| ask.get("serverName")) {
        map.insert("serverName".into(), name.clone());
    }
    if let Some(info) = mcp_summary.as_ref().and_then(|mcp| mcp.get("serverInfo")) {
        map.insert("serverInfo".into(), info.clone());
    }
    let event_url = if ask_summary.is_some() { ask_url } else { mcp_url };
    let event = DiscoveryEvent {
        v: EVENT_VERSION,
        standard: "nlweb".into(),
        aliases: None,
        origin: origin.to_string(),
        url: event_url,
        via: Via::WellKnown,
        page: page_url,
        target_id,
        status: Validity::Valid,
        summary: Value::Object(map),
        reason: None,
    };
    output.push_str(&emit_event(state, &event));
    output
}

const NLWEB_ASK: FetchInit<'static> = FetchInit {
    method: None,
    headers: &[("Accept", "application/json")],
    body: None,
    sse_until: None,
};

const NLWEB_MCP_HEADERS: &[(&str, &str)] = &[
    ("Content-Type", "application/json"),
    ("Accept", "application/json, text/event-stream"),
];

fn nlweb_mcp_init_body() -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "slicc-discover", "version": "0.1" }
        }
    })
    .to_string()
}

fn json_ok_body(status: u16, content_type: &str, body: String) -> Option<String> {
    (status == 200 && is_json_content_type(content_type)).then_some(body)
}

fn mcp_init_payload(status: u16, content_type: &str, body: String) -> Option<String> {
    let media = content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if media != "text/event-stream" {
        let body = json_ok_body(status, content_type, body)?;
        return initialize_response(&serde_json::from_str(&body).ok()?);
    }
    if status != 200 {
        return None;
    }
    body.strip_prefix('\u{feff}')
        .unwrap_or(&body)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split("\n\n")
        .map(|event| {
            event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|data| data.strip_prefix(' ').unwrap_or(data))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .find_map(|data| initialize_response(&serde_json::from_str::<Value>(&data).ok()?))
}

fn initialize_response(message: &Value) -> Option<String> {
    let found = match message {
        Value::Array(batch) => batch.iter().find(|item| item.get("result").is_some())?,
        single => single.get("result").map(|_| single)?,
    };
    Some(found.to_string())
}

#[cfg(not(target_os = "wasi"))]
fn nlweb_mcp_probe_native(url: &str, body: &str) -> Option<String> {
    let resp = cdp_client::net::http_post_json(url, body).ok()?;
    mcp_init_payload(
        resp.status,
        resp.content_type(),
        String::from_utf8_lossy(&resp.body).into_owned(),
    )
}

#[cfg(target_os = "wasi")]
fn nlweb_mcp_probe_native(_url: &str, _body: &str) -> Option<String> {
    None
}

fn follow_ard_catalogs(
    cdp: &mut Cdp,
    state: &mut WatcherState,
    session_id: &str,
    origin: &str,
    frame_id: &str,
    page_url: &str,
    target_id: &str,
    summary: &Value,
) -> String {
    let mut output = String::new();
    let catalogs = match summary.get("catalogs").and_then(|v| v.as_array()) {
        Some(arr) => arr.clone(),
        None => return output,
    };
    for catalog in &catalogs {
        let cat_url = match catalog.get("url").and_then(|v| v.as_str()) {
            Some(u) => u.to_string(),
            None => continue,
        };
        if cdp.guard().url(&cat_url) {
            continue;
        }
        let dedup_key = format!("follow-catalog:{cat_url}");
        if !state.dedup.is_new(origin, "ard-catalog", &dedup_key) {
            continue;
        }
        let probe_result = isolated_world_probe(cdp, session_id, frame_id, &cat_url);
        let (cat_status, cat_ct, cat_body) = match probe_result {
            IsolatedProbeResult::Ok(s, ct, b) => (s, ct, b),
            IsolatedProbeResult::Final => continue,
            IsolatedProbeResult::Unavailable => {
                match native_probe(&cat_url, "application/json") {
                    Some((s, ct, b)) => (s, ct, b),
                    None => continue,
                }
            }
        };
        if cat_status != 200 || !is_json_content_type(&cat_ct) {
            continue;
        }
        let cat_origin = extract_origin(&cat_url).unwrap_or_else(|| origin.to_string());
        let classification = classify_ard_body(&cat_body, Some(&cat_origin));
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: cat_origin,
            url: cat_url,
            via: Via::WellKnown,
            page: page_url.to_string(),
            target_id: target_id.to_string(),
            status: classification.status,
            summary: classification.summary,
            reason: classification.reason,
        };
        output.push_str(&emit_event(state, &event));
        output.push_str(&emit_nlweb_from_catalog(state, &event.origin, page_url, target_id, &event.summary));
    }
    output
}
fn is_json_content_type(ct: &str) -> bool {
    let lower = ct.to_ascii_lowercase();
    let media = lower.split(';').next().unwrap_or("").trim();
    media == "application/json" || media.ends_with("+json")
}

pub fn extract_origin(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let origin = parsed.origin();
    match origin {
        url::Origin::Tuple(scheme, host, port) => {
            let default_port = match scheme.as_str() {
                "http" | "ws" => 80,
                "https" | "wss" => 443,
                _ => 0,
            };
            if port == default_port {
                Some(format!("{scheme}://{host}"))
            } else {
                Some(format!("{scheme}://{host}:{port}"))
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_origin_https() {
        assert_eq!(
            extract_origin("https://example.com/page"),
            Some("https://example.com".into())
        );
    }

    #[test]
    fn extract_origin_http_with_port() {
        assert_eq!(
            extract_origin("http://localhost:8080/foo"),
            Some("http://localhost:8080".into())
        );
    }

    #[test]
    fn extract_origin_default_port_omitted() {
        assert_eq!(
            extract_origin("https://example.com:443/page"),
            Some("https://example.com".into())
        );
    }

    #[test]
    fn extract_origin_invalid() {
        assert_eq!(extract_origin("not a url"), None);
    }

    #[test]
    fn extract_origin_about_blank() {
        assert_eq!(extract_origin("about:blank"), None);
    }

    #[test]
    fn emit_event_json() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            
            
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "T1".into(),
            status: Validity::Valid,
            summary: json!({"name": "test"}),
            reason: None,
        };
        let line = emit_event(&mut state, &event);
        assert!(line.ends_with('\n'));
        assert!(line.contains("\"v\":1"));
        assert!(line.contains("\"standard\":\"ard\""));
        assert_eq!(state.events_emitted, 1);
    }

    #[test]
    fn emit_event_text() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: false,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            
            
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "T1".into(),
            status: Validity::Valid,
            summary: json!({"name": "test"}),
            reason: None,
        };
        let line = emit_event(&mut state, &event);
        assert!(line.contains("ard"));
        assert!(line.contains("\"v\":1"));
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn probed_origins_prevents_double_probe() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: false,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            
            
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.probed_origins.insert("https://example.com".to_string());
        assert!(state.probed_origins.contains("https://example.com"));
    }

    #[test]
    fn dedup_prevents_duplicate_events() {
        let mut dedup = DedupSet::new();
        assert!(dedup.is_new("https://example.com", "ard", "https://example.com/.well-known/ard.json"));
        assert!(!dedup.is_new("https://example.com", "ard", "https://example.com/.well-known/ard.json"));
        assert!(dedup.is_new("https://example.com", "webmcp", "https://example.com/"));
    }

    #[test]
    fn handle_frame_navigated_updates_session() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: false,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            
            
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "about:blank".to_string(),
            },
        );
        let params = json!({
            "frame": {
                "id": "F2",
                "url": "https://example.com/new"
            }
        });
        handle_frame_navigated(&mut state, "S1", &params);
        let sess = state.sessions.get("S1").unwrap();
        assert_eq!(sess.top_frame_id.as_deref(), Some("F2"));
        assert_eq!(sess.page_url, "https://example.com/new");
    }

    #[test]
    fn handle_frame_navigated_ignores_subframes() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: false,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            
            
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "frame": {
                "id": "F99",
                "url": "https://example.com/child",
                "parentId": "F1"
            }
        });
        handle_frame_navigated(&mut state, "S1", &params);
        let sess = state.sessions.get("S1").unwrap();
        assert_eq!(sess.top_frame_id.as_deref(), Some("F1"));
        assert_eq!(sess.page_url, "https://example.com/");
    }

    #[test]
    fn known_link_rels_are_recognized() {
        for rel in known_link_rels() {
            assert!(known_link_rels().contains(rel));
        }
    }

    #[test]
    fn extract_origin_strips_path_and_query() {
        assert_eq!(
            extract_origin("https://example.com:8443/path?q=1#h"),
            Some("https://example.com:8443".into())
        );
    }

    #[test]
    fn protected_origins_are_never_probed() {
        use cdp_client::protected::Protected;
        let guard = Protected::default();
        for origin in [
            "https://seven.sliccy.ai",
            "https://vault.sliccy.ai",
            "https://www.sliccy.ai",
            "https://sliccy.ai",
        ] {
            assert!(guard.url(origin), "expected {origin} to be protected");
        }
        for origin in [
            "https://example.com",
            "http://localhost:8080",
        ] {
            assert!(!guard.url(origin), "expected {origin} not to be protected");
        }
    }

    #[test]
    fn protected_link_hrefs_are_skipped() {
        use cdp_client::protected::Protected;
        let guard = Protected::default();
        assert!(guard.url("https://seven.sliccy.ai/.well-known/ard.json"));
        assert!(!guard.url("https://example.com/.well-known/ard.json"));
    }

    #[test]
    fn response_on_protected_origin_is_skipped() {
        use cdp_client::protected::Protected;
        let guard = Protected::default();
        assert!(guard.url("https://seven.sliccy.ai/page"));
        assert!(guard.url("https://vault.sliccy.ai/"));
    }

    #[test]
    fn handle_response_skips_non_document() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Script",
            "frameId": "F1",
            "response": {
                "url": "https://example.com/script.js",
                "status": 200,
                "headers": { "link": "</api>; rel=\"ard\"" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.is_empty(), "script resource should be skipped");
    }

    #[test]
    fn handle_response_emits_link_header_for_document() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Document",
            "frameId": "F1",
            "response": {
                "url": "https://example.com/page",
                "status": 200,
                "headers": { "link": "</api>; rel=\"api-catalog\"" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.contains("api-catalog"), "expected api-catalog event: {output}");
    }

    #[test]
    fn handle_response_402_emits_x402() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Document",
            "frameId": "F1",
            "response": {
                "url": "https://example.com/pay",
                "status": 402,
                "headers": { "x-payment": "lightning" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.contains("x402"), "expected x402 event: {output}");
    }

    struct FakeTransport;

    impl cdp_client::cdp::Transport for FakeTransport {
        fn send_text(&mut self, _text: &str) -> Result<(), String> {
            Ok(())
        }

        fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
            Ok(None)
        }
    }

    #[test]
    fn a_page_on_a_catalog_debug_port_is_discovered_like_any_other() {
        for origin in ["http://127.0.0.1:9222", "http://localhost:9229", "http://[::1]:9225"] {
            let mut state = WatcherState {
                dedup: DedupSet::new(),
                probed_origins: BTreeSet::new(),
                probes: default_probes(),
                classifiers: default_classifiers(),
                sessions: BTreeMap::new(),
                json_mode: true,
                follow_mode: false,
                robots_probe: false, nlweb_probe: false,
                no_probe: false,
                events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
            };
            state.sessions.insert(
                "S1".to_string(),
                SessionState {
                    target_id: "T1".to_string(),
                    top_frame_id: Some("F1".to_string()),
                    page_url: format!("{origin}/"),
                },
            );
            let params = json!({
                "type": "Document",
                "frameId": "F1",
                "response": {
                    "url": format!("{origin}/pay"),
                    "status": 402,
                    "headers": { "x-payment": "lightning", "link": format!("<{origin}/api>; rel=\"api-catalog\"") }
                }
            });
            let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
            let output = handle_response(&mut cdp, &mut state, "S1", &params);
            assert!(output.contains("x402"), "{origin}: {output}");
        }
    }

    #[test]
    fn io_read_cap_is_one_mib() {
        assert_eq!(IO_READ_CAP, 1024 * 1024);
    }

    #[test]
    fn probe_engine_rejects_html_200() {
        let probes = default_probes();
        let m = probes.check_response("ard", 200, "text/html", "<html>".into(), "u".into());
        assert!(m.is_none(), "HTML 200 must not match engine check_response");
    }

    #[test]
    fn probe_engine_rejects_404() {
        let probes = default_probes();
        let m = probes.check_response("ard", 404, "application/json", "{}".into(), "u".into());
        assert!(m.is_none(), "404 must not match engine check_response");
    }

    #[test]
    fn probe_engine_rejects_301() {
        let probes = default_probes();
        let m = probes.check_response("ard", 301, "application/json", "".into(), "u".into());
        assert!(m.is_none(), "301 must not match engine check_response");
    }

    #[test]
    fn emit_event_text_escapes_control_chars() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: false,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://evil.com".into(),
            url: "https://evil.com/\x1b[31mred\x1b[0m".into(),
            via: Via::LinkHeader,
            page: "https://evil.com/".into(),
            target_id: "T".into(),
            status: Validity::Valid,
            summary: json!({}),
            reason: None,
        };
        let line = emit_event(&mut state, &event);
        assert!(!line.contains('\x1b'), "ESC must not appear raw: {line}");
        assert!(line.starts_with('{'), "stdout sink must emit JSON");
    }

    #[test]
    fn emit_event_json_does_not_escape_control_chars() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://evil.com".into(),
            url: "https://evil.com/\x1b[31m".into(),
            via: Via::LinkHeader,
            page: "https://evil.com/".into(),
            target_id: "T".into(),
            status: Validity::Valid,
            summary: json!({}),
            reason: None,
        };
        let line = emit_event(&mut state, &event);
        assert!(line.starts_with('{'), "JSON mode must emit JSON");
    }

    #[test]
    fn off_flag_exits_immediately() {
        let mut flags = BTreeMap::new();
        flags.insert("off".to_string(), "true".to_string());
        let invocation = Invocation {
            command: Some("discover".to_string()),
            positionals: vec![],
            flags,
            multi: BTreeMap::new(),
            cdp: None,
            runtime: None,
            session: None,
            full_urls: false,
            help: false,
            command_help: false,
            version: false,
            cwd: None,
        };
        let output = run(&invocation, None);
        assert_eq!(output.code, 0);
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn disabled_file_is_checked() {
        let config = discover_config_dir();
        let slicc = std::path::Path::new(&config).join("slicc");
        std::fs::create_dir_all(&slicc).unwrap();
        let disabled = slicc.join("discover.disabled");
        std::fs::write(&disabled, "").unwrap();
        let result = is_disabled();
        let _ = std::fs::remove_file(&disabled);
        assert!(result);
    }

    #[test]
    fn disabled_file_absent_means_enabled() {
        let config = discover_config_dir();
        let disabled = std::path::Path::new(&config).join("slicc").join("discover.disabled");
        let _ = std::fs::remove_file(&disabled);
        let result = is_disabled();
        assert!(!result);
    }

    #[test]
    fn lick_file_uses_fingerprint_hash() {
        let tmp = std::env::temp_dir().join(format!("slicc-test-lick-{}", std::process::id()));
        let slicc = tmp.join(".slicc");
        std::fs::create_dir_all(slicc.join("discoveries")).unwrap();
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "T1".into(),
            status: Validity::Valid,
            summary: json!({"name": "test"}),
            reason: None,
        };
        write_lick_file(&slicc.to_string_lossy(), &event, &event);
        let fp = fingerprint("https://example.com", "ard", "https://example.com/.well-known/ard.json");
        let hash = cdp_client::hash::sha256_hex(fp.as_bytes());
        let path = slicc.join("discoveries").join(format!("{hash}.json"));
        assert!(path.exists(), "lick file must exist at {}", path.display());
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["standard"], "ard");
        assert_eq!(parsed["origin"], "https://example.com");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn lick_file_name_never_uses_event_fields() {
        let tmp = std::env::temp_dir().join(format!("slicc-test-hostile-lick-{}", std::process::id()));
        let slicc = tmp.join(".slicc");
        std::fs::create_dir_all(slicc.join("discoveries")).unwrap();
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://evil.com".into(),
            url: "https://evil.com/$(rm -rf /)".into(),
            via: Via::WellKnown,
            page: "https://evil.com/`whoami`".into(),
            target_id: "T1".into(),
            status: Validity::Valid,
            summary: json!({}),
            reason: None,
        };
        write_lick_file(&slicc.to_string_lossy(), &event, &event);
        let entries: Vec<_> = std::fs::read_dir(slicc.join("discoveries"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        let name = entries[0].file_name().to_string_lossy().to_string();
        assert!(name.ends_with(".json"));
        assert!(!name.contains("rm"));
        assert!(!name.contains("whoami"));
        assert!(!name.contains("$"));
        assert!(!name.contains("`"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn ensure_lick_dirs_creates_structure_and_watch() {
        let tmp = std::env::temp_dir().join(format!("slicc-test-ensure-{}", std::process::id()));
        let slicc = tmp.join(".slicc");
        ensure_lick_dirs(&slicc.to_string_lossy());
        assert!(slicc.join("discoveries").is_dir());
        assert!(slicc.join("watches").is_dir());
        let watch = slicc.join("watches").join("discover.json");
        assert!(watch.exists());
        let content = std::fs::read_to_string(&watch).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["glob"], "*.json");
        assert_eq!(parsed["target"], "cone");
        assert!(parsed["path"].as_str().unwrap().ends_with("/discoveries"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn atomic_write_creates_file() {
        let tmp = std::env::temp_dir().join(format!("slicc-test-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("test.json");
        atomic_write(&path.to_string_lossy(), b"hello");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
        let tmp_file = tmp.join(format!(".tmp.{}", std::process::id()));
        assert!(!tmp_file.exists(), "temp file must be cleaned up");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn emit_event_with_lick_sink_writes_file() {
        let tmp = std::env::temp_dir().join(format!("slicc-test-emit-lick-{}", std::process::id()));
        let slicc = tmp.join(".slicc");
        std::fs::create_dir_all(slicc.join("discoveries")).unwrap();
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: false,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Lick { slicc_dir: slicc.to_string_lossy().to_string() },
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "T1".into(),
            status: Validity::Valid,
            summary: json!({"name": "test"}),
            reason: None,
        };
        let output = emit_event(&mut state, &event);
        assert!(output.is_empty(), "lick sink should not output to stdout");
        assert_eq!(state.events_emitted, 1);
        let entries: Vec<_> = std::fs::read_dir(slicc.join("discoveries"))
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn exec_sink_hostile_url_not_in_command_line() {
        let tmp = std::env::temp_dir().join(format!("slicc-test-exec-hostile-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let record = tmp.join("stdin.json");
        let cmd = format!("cat > {}", record.display());
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://evil.com".into(),
            url: "https://evil.com/$(touch /tmp/pwned)\"'`backtick`".into(),
            via: Via::WellKnown,
            page: "https://evil.com/".into(),
            target_id: "T".into(),
            status: Validity::Valid,
            summary: json!({}),
            reason: None,
        };
        run_exec_sink(
            &cmd,
            Duration::from_secs(5),
            &event,
        );
        assert!(record.exists(), "script must receive stdin");
        let content = std::fs::read_to_string(&record).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["standard"], "ard");
        assert!(!std::path::Path::new("/tmp/pwned").exists(), "command injection must not happen");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn exec_sink_timeout_kills_process() {
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "T".into(),
            status: Validity::Valid,
            summary: json!({}),
            reason: None,
        };
        let start = Instant::now();
        run_exec_sink(
            "sleep 60",
            Duration::from_secs(1),
            &event,
        );
        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_secs(5), "timeout should kill quickly, took {elapsed:?}");
    }

    #[test]
    fn exec_sink_nonzero_exit_does_not_panic() {
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "T".into(),
            status: Validity::Valid,
            summary: json!({}),
            reason: None,
        };
        run_exec_sink(
            "exit 42",
            Duration::from_secs(5),
            &event,
        );
    }

    #[test]
    fn exec_sink_env_vars_are_set() {
        let tmp = std::env::temp_dir().join(format!("slicc-test-exec-env-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("env.txt");
        let cmd = format!(
            "echo \"$SLICC_DISCOVER_STANDARD\" > {} && echo \"$SLICC_DISCOVER_ORIGIN\" >> {} && echo \"$SLICC_DISCOVER_URL\" >> {}",
            out.display(),
            out.display(),
            out.display()
        );
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "T".into(),
            status: Validity::Valid,
            summary: json!({}),
            reason: None,
        };
        run_exec_sink(
            &cmd,
            Duration::from_secs(5),
            &event,
        );
        let content = std::fs::read_to_string(&out).unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines[0], "ard");
        assert_eq!(lines[1], "https://example.com");
        assert_eq!(lines[2], "https://example.com/.well-known/ard.json");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn exec_sink_command_with_arguments() {
        let tmp = std::env::temp_dir().join(format!("slicc-test-exec-args-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let record = tmp.join("args.json");
        let cmd = format!("cat > {} < /dev/stdin", record.display());
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "T".into(),
            status: Validity::Valid,
            summary: json!({"name": "test"}),
            reason: None,
        };
        run_exec_sink(&cmd, Duration::from_secs(5), &event);
        assert!(record.exists(), "command with arguments must work");
        let content = std::fs::read_to_string(&record).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["standard"], "ard");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn exec_sink_hostile_url_unexpanded_in_env() {
        let tmp = std::env::temp_dir().join(format!("slicc-test-exec-env-hostile-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let out = tmp.join("env.txt");
        let cmd = format!("echo \"$SLICC_DISCOVER_URL\" > {}", out.display());
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://evil.com".into(),
            url: "https://evil.com/$(touch /tmp/pwned2)\"'`backtick`".into(),
            via: Via::WellKnown,
            page: "https://evil.com/".into(),
            target_id: "T".into(),
            status: Validity::Valid,
            summary: json!({}),
            reason: None,
        };
        run_exec_sink(&cmd, Duration::from_secs(5), &event);
        assert!(!std::path::Path::new("/tmp/pwned2").exists(), "env var must not expand shell metacharacters");
        let content = std::fs::read_to_string(&out).unwrap();
        assert!(content.contains("$(touch /tmp/pwned2)"), "hostile URL must pass through as literal in env: {content}");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn stdout_sink_emits_ndjson() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "T1".into(),
            status: Validity::Valid,
            summary: json!({"name": "test"}),
            reason: None,
        };
        let line = emit_event(&mut state, &event);
        assert!(line.ends_with('\n'));
        let parsed: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["standard"], "ard");
    }

    #[test]
    fn watch_json_matches_slicc_agent_format() {
        let tmp = std::env::temp_dir().join(format!("slicc-test-watch-fmt-{}", std::process::id()));
        let slicc = tmp.join(".slicc");
        ensure_lick_dirs(&slicc.to_string_lossy());
        let watch = slicc.join("watches").join("discover.json");
        let content = std::fs::read_to_string(&watch).unwrap();
        let parsed: Value = serde_json::from_str(&content).unwrap();
        assert!(parsed.get("path").is_some());
        assert!(parsed.get("glob").is_some());
        assert!(parsed.get("target").is_some());
        assert!(parsed.get("message").is_some());
        assert!(parsed.get("debounce").is_some());
        assert!(parsed.get("triage").is_some());
        let obj = parsed.as_object().unwrap();
        for key in obj.keys() {
            assert!(
                ["path", "glob", "target", "message", "debounce", "triage"].contains(&key.as_str()),
                "unexpected key {key} in discover.json"
            );
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn strip_userinfo_removes_credentials() {
        assert_eq!(
            strip_userinfo("https://u:PW@host.com/path"),
            "https://host.com/path"
        );
    }

    #[test]
    fn strip_userinfo_preserves_clean_url() {
        assert_eq!(
            strip_userinfo("https://host.com/path"),
            "https://host.com/path"
        );
    }

    #[test]
    fn strip_userinfo_from_event_fields() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://user:pass@example.com".into(),
            url: "https://user:pass@example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://user:pass@example.com/".into(),
            target_id: "T".into(),
            status: Validity::Valid,
            summary: json!({ "href": "https://user:pass@example.com/api" }),
            reason: None,
        };
        let line = emit_event(&mut state, &event);
        assert!(!line.contains("user:pass"), "userinfo must be stripped: {line}");
        assert!(line.contains("example.com"));
        let parsed: Value = serde_json::from_str(line.trim()).unwrap();
        assert!(!parsed["origin"].as_str().unwrap().contains("user"));
        assert!(!parsed["page"].as_str().unwrap().contains("pass"));
        assert!(!parsed["summary"]["href"].as_str().unwrap().contains("user"));
    }

    #[test]
    fn safe_json_escapes_del_and_c1() {
        let input = "hello\x7fworld\u{009b}test";
        let json_str = serde_json::to_string(input).unwrap();
        let safe = safe_json_str(&json_str);
        assert!(!safe.contains('\x7f'), "DEL must be escaped");
        assert!(!safe.contains('\u{009b}'), "CSI must be escaped");
        assert!(safe.contains("\\u007f"), "DEL must be \\u007f: {safe}");
        assert!(safe.contains("\\u009b"), "CSI must be \\u009b: {safe}");
        let unescaped: String = serde_json::from_str(&safe).unwrap();
        assert_eq!(unescaped, input);
    }

    #[test]
    fn lick_event_json_escapes_c1() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/\x7f".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "T".into(),
            status: Validity::Valid,
            summary: json!({}),
            reason: None,
        };
        let line = emit_event(&mut state, &event);
        assert!(!line.contains('\x7f'), "DEL must be escaped in event JSON");
    }

    #[cfg(unix)]
    #[test]
    fn lick_dirs_have_mode_0700() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("slicc-test-mode-{}", std::process::id()));
        let slicc = tmp.join(".slicc");
        let old_umask = unsafe { libc::umask(0o000) };
        ensure_lick_dirs(&slicc.to_string_lossy());
        unsafe { libc::umask(old_umask); }
        let mode = std::fs::metadata(slicc.join("discoveries")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "discoveries dir mode must be 0700, got {mode:o}");
        let mode = std::fs::metadata(slicc.join("watches")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "watches dir mode must be 0700, got {mode:o}");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_file_has_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = std::env::temp_dir().join(format!("slicc-test-fmode-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("test.json");
        let old_umask = unsafe { libc::umask(0o000) };
        atomic_write(&path.to_string_lossy(), b"hello");
        unsafe { libc::umask(old_umask); }
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "file mode must be 0600, got {mode:o}");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn protected_html_link_hrefs_are_filtered() {
        let guard = cdp_client::protected::Protected::default();
        assert!(guard.url("https://vault.sliccy.ai/catalog.json"));
        assert!(guard.url("https://seven.sliccy.ai/.well-known/ard.json"));
        assert!(!guard.url("https://example.com/.well-known/ard.json"));
    }

    #[test]
    fn chrome_timeout_returns_final_not_unavailable() {
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let result = isolated_world_probe(&mut cdp, "S1", "F1", "http://example.com/.well-known/ard.json");
        assert!(
            !matches!(result, IsolatedProbeResult::Ok(_, _, _)),
            "a failing CDP must not return Ok"
        );
    }

    #[test]
    fn isolated_probe_final_does_not_fall_through() {
        let r = IsolatedProbeResult::Final;
        let fell_through = matches!(r, IsolatedProbeResult::Unavailable);
        assert!(!fell_through, "Final must not trigger the native fallback path");
    }

    #[test]
    fn link_header_slicc_handoff_emits_slicc_handoff_standard() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Document",
            "frameId": "F1",
            "response": {
                "url": "https://example.com/page",
                "status": 200,
                "headers": { "link": "<https://github.com/o/r>; rel=\"https://www.sliccy.ai/rel/upskill\"; branch=main; path=\"skills/foo\"" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.contains("slicc-handoff"), "expected slicc-handoff standard: {output}");
        let parsed: Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["standard"], "slicc-handoff");
        assert_eq!(parsed["summary"]["verb"], "upskill");
        assert_eq!(parsed["summary"]["branch"], "main");
        assert_eq!(parsed["summary"]["path"], "skills/foo");
    }

    #[test]
    fn link_header_handoff_rel_emits_handoff_verb() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Document",
            "frameId": "F1",
            "response": {
                "url": "https://example.com/page",
                "status": 200,
                "headers": { "link": "<>; rel=\"https://www.sliccy.ai/rel/handoff\"; title=\"Continue the signup flow\"" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.contains("slicc-handoff"), "expected slicc-handoff: {output}");
        let parsed: Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["summary"]["verb"], "handoff");
        assert_eq!(parsed["summary"]["instruction"], "Continue the signup flow");
    }

    #[test]
    fn link_header_llms_txt_rel_maps_to_llms_txt_standard() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Document",
            "frameId": "F1",
            "response": {
                "url": "https://example.com/page",
                "status": 200,
                "headers": { "link": "</llms.txt>; rel=\"https://llmstxt.org/rel/llms-txt\"" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.contains("llms-txt"), "expected llms-txt standard: {output}");
        let parsed: Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["standard"], "llms-txt");
    }

    #[test]
    fn link_header_rfc_rels_emit_with_summary() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let header = [
            "</.well-known/api-catalog>; rel=\"api-catalog\"",
            "</openapi.json>; rel=\"service-desc\"",
            "</status>; rel=\"status\"",
            "</meta>; rel=\"service-meta\"",
            "</doc>; rel=\"service-doc\"",
        ].join(", ");
        let params = json!({
            "type": "Document",
            "frameId": "F1",
            "response": {
                "url": "https://example.com/page",
                "status": 200,
                "headers": { "link": header }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        let events: Vec<Value> = output.trim().split('\n')
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(events.len(), 5);
        let standards: Vec<&str> = events.iter()
            .map(|e| e["standard"].as_str().unwrap())
            .collect();
        assert!(standards.contains(&"api-catalog"));
        assert!(standards.contains(&"service-desc"));
        assert!(standards.contains(&"status"));
        assert!(standards.contains(&"service-meta"));
        assert!(standards.contains(&"service-doc"));
        for e in &events {
            assert!(e["summary"]["rel"].is_string());
            assert!(e["summary"]["href"].is_string());
        }
    }

    #[test]
    fn link_header_protected_origin_silently_dropped() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Document",
            "frameId": "F1",
            "response": {
                "url": "https://example.com/page",
                "status": 200,
                "headers": { "link": "<https://vault.sliccy.ai/api>; rel=\"api-catalog\"" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.is_empty(), "protected origin link should be silently dropped: {output}");
    }

    #[test]
    fn follow_skips_protected_origin_links() {
        let guard = cdp_client::protected::Protected::default();
        assert!(guard.url("https://vault.sliccy.ai/api"));
        assert!(guard.url("https://seven.sliccy.ai/.well-known/api-catalog"));
    }

    #[test]
    fn link_header_upskill_unsafe_branch_dropped() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Document",
            "frameId": "F1",
            "response": {
                "url": "https://example.com/page",
                "status": 200,
                "headers": { "link": "<https://github.com/o/r>; rel=\"https://www.sliccy.ai/rel/upskill\"; branch=\"main;rm -rf /\"" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        let parsed: Value = serde_json::from_str(output.trim()).unwrap();
        assert!(parsed["summary"].get("branch").is_none(), "unsafe branch must be dropped");
    }

    #[test]
    fn llms_txt_probe_content_type_accepted() {
        let probes = default_probes();
        for ct in &["text/plain", "text/plain; charset=utf-8", "text/markdown"] {
            let m = probes.check_response("llms-txt", 200, ct, "# Title".into(), "u".into());
            assert!(m.is_some(), "content-type {ct} should match llms-txt probe");
        }
    }

    #[test]
    fn llms_txt_probe_html_rejected() {
        let probes = default_probes();
        let m = probes.check_response("llms-txt", 200, "text/html", "<html>".into(), "u".into());
        assert!(m.is_none());
    }

    #[test]
    fn api_catalog_probe_linkset_accepted() {
        let probes = default_probes();
        let m = probes.check_response("api-catalog", 200, "application/linkset+json", "{}".into(), "u".into());
        assert!(m.is_some());
    }

    #[test]
    fn api_catalog_probe_json_accepted() {
        let probes = default_probes();
        let m = probes.check_response("api-catalog", 200, "application/json", "{}".into(), "u".into());
        assert!(m.is_some());
    }

    #[test]
    fn api_catalog_probe_html_rejected() {
        let probes = default_probes();
        let m = probes.check_response("api-catalog", 200, "text/html", "<html>".into(), "u".into());
        assert!(m.is_none());
    }

    #[test]
    fn follow_mode_flag_parsed() {
        let mut flags = BTreeMap::new();
        flags.insert("off".to_string(), "true".to_string());
        flags.insert("follow".to_string(), "true".to_string());
        let invocation = Invocation {
            command: Some("discover".to_string()),
            positionals: vec![],
            flags,
            multi: BTreeMap::new(),
            cdp: None,
            runtime: None,
            session: None,
            full_urls: false,
            help: false,
            command_help: false,
            version: false,
            cwd: None,
        };
        let output = run(&invocation, None);
        assert_eq!(output.code, 0);
    }

    #[test]
    fn probe_robots_flag_parsed() {
        let mut flags = BTreeMap::new();
        flags.insert("off".to_string(), "true".to_string());
        flags.insert("probe".to_string(), "robots".to_string());
        let invocation = Invocation {
            command: Some("discover".to_string()),
            positionals: vec![],
            flags,
            multi: BTreeMap::new(),
            cdp: None,
            runtime: None,
            session: None,
            full_urls: false,
            help: false,
            command_help: false,
            version: false,
            cwd: None,
        };
        let output = run(&invocation, None);
        assert_eq!(output.code, 0);
    }

    #[test]
    fn is_json_content_type_matches() {
        assert!(is_json_content_type("application/json"));
        assert!(is_json_content_type("application/json; charset=utf-8"));
        assert!(is_json_content_type("application/ai-catalog+json"));
        assert!(is_json_content_type("application/ai-registry+json"));
        assert!(is_json_content_type("APPLICATION/JSON"));
    }

    #[test]
    fn is_json_content_type_rejects() {
        assert!(!is_json_content_type("text/html"));
        assert!(!is_json_content_type("text/plain"));
        assert!(!is_json_content_type("application/xml"));
    }

    #[test]
    fn link_header_multiple_rels_comma_separated() {
        let base = Url::parse("https://example.com/").unwrap();
        let links = parse_link_header(
            "</.well-known/api-catalog>; rel=\"api-catalog\", </openapi.json>; rel=\"service-desc\", </status>; rel=\"status\", </llms.txt>; rel=\"https://llmstxt.org/rel/llms-txt\"",
            &base,
        );
        assert_eq!(links.len(), 4);
        assert_eq!(links[0].rel, "api-catalog");
        assert_eq!(links[1].rel, "service-desc");
        assert_eq!(links[2].rel, "status");
        assert_eq!(links[3].rel, LLMS_TXT_REL);
    }

    #[test]
    fn link_header_params_branch_path() {
        let base = Url::parse("https://example.com/").unwrap();
        let links = parse_link_header(
            "<https://github.com/o/r>; rel=\"https://www.sliccy.ai/rel/upskill\"; branch=main; path=\"skills/foo\"",
            &base,
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].params.get("branch").map(String::as_str), Some("main"));
        assert_eq!(links[0].params.get("path").map(String::as_str), Some("skills/foo"));
    }

    #[test]
    fn link_header_ext_value_branch() {
        let base = Url::parse("https://example.com/").unwrap();
        let links = parse_link_header(
            "<https://github.com/o/r>; rel=\"https://www.sliccy.ai/rel/upskill\"; branch*=UTF-8''feature%2Ffix",
            &base,
        );
        assert_eq!(links[0].params.get("branch").map(String::as_str), Some("feature/fix"));
    }

    #[test]
    fn discover_links_p0_rels_recognized() {
        let rels = known_link_rels();
        for rel in &["api-catalog", "service-desc", "service-meta", "status", "service-doc", LLMS_TXT_REL] {
            assert!(rels.contains(rel), "missing {rel}");
        }
    }

    #[test]
    fn discover_links_first_occurrence_wins_dedup() {
        let mut dedup = DedupSet::new();
        assert!(dedup.is_new("https://example.com", "api-catalog", "https://example.com/a"));
        assert!(!dedup.is_new("https://example.com", "api-catalog", "https://example.com/a"));
        assert!(dedup.is_new("https://example.com", "api-catalog", "https://example.com/b"));
    }

    #[test]
    fn llmstxtignore_defaults_contain_expected_hosts() {
        let defaults = discovery::default_llmstxtignore_entries();
        assert!(defaults.contains(&"github.com"));
        assert!(defaults.contains(&"app.slack.com"));
    }

    #[test]
    fn handle_response_401_xhr_emits_auth_md() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "XHR",
            "frameId": "F1",
            "response": {
                "url": "https://api.example.com/data",
                "status": 401,
                "headers": { "www-authenticate": "Bearer realm=\"api\"" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.contains("auth-md"), "expected auth-md event from XHR 401: {output}");
        let parsed: Value = serde_json::from_str(output.trim_end().lines().next().unwrap()).unwrap();
        assert_eq!(parsed["standard"], "auth-md");
        assert_eq!(parsed["via"], "response");
    }

    #[test]
    fn handle_response_401_fetch_emits_auth_md() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Fetch",
            "frameId": "F1",
            "response": {
                "url": "https://api.example.com/resource",
                "status": 401,
                "headers": {
                    "www-authenticate": "Bearer realm=\"api\"",
                    "link": "</auth.md>; rel=\"auth-md\""
                }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.contains("auth-md"), "expected auth-md event from Fetch 401: {output}");
        let parsed: Value = serde_json::from_str(output.trim_end().lines().next().unwrap()).unwrap();
        assert_eq!(parsed["summary"]["status"], 401);
        assert!(parsed["summary"]["links"].as_array().unwrap().len() >= 1);
    }

    #[test]
    fn handle_response_non_401_xhr_still_skipped() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "XHR",
            "frameId": "F1",
            "response": {
                "url": "https://api.example.com/data",
                "status": 200,
                "headers": {}
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.is_empty(), "200 XHR should not emit events: {output}");
    }

    #[test]
    fn handle_response_401_never_leaks_request_headers() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "XHR",
            "frameId": "F1",
            "response": {
                "url": "https://api.example.com/data",
                "status": 401,
                "headers": {
                    "www-authenticate": "Bearer realm=\"api\""
                }
            },
            "requestHeaders": {
                "authorization": "Bearer SECRET_TOKEN",
                "cookie": "session=mysecret"
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(!output.contains("SECRET_TOKEN"), "must not leak Authorization header");
        assert!(!output.contains("mysecret"), "must not leak Cookie");
    }

    #[test]
    fn handle_response_401_document_emits_auth_md() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Document",
            "frameId": "F1",
            "response": {
                "url": "https://example.com/protected",
                "status": 401,
                "headers": {
                    "www-authenticate": "Bearer realm=\"site\", resource_metadata=\"https://example.com/.well-known/oauth-protected-resource\""
                }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.contains("auth-md"), "expected auth-md: {output}");
        let parsed: Value = serde_json::from_str(output.trim_end().lines().next().unwrap()).unwrap();
        let schemes = parsed["summary"]["schemes"].as_array().unwrap();
        assert!(schemes[0]["resource_metadata"].as_str().unwrap().contains("oauth-protected-resource"));
    }

    #[test]
    fn handle_response_401_filters_protected_auth_links() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "XHR",
            "frameId": "F1",
            "response": {
                "url": "https://api.example.com/data",
                "status": 401,
                "headers": {
                    "www-authenticate": "Bearer resource_metadata=\"https://vault.sliccy.ai/secret\"",
                    "link": "<https://seven.sliccy.ai/auth.md>; rel=\"auth-md\""
                }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.contains("auth-md"), "auth-md event expected: {output}");
        assert!(!output.contains("vault.sliccy.ai"), "protected resource_metadata must be filtered: {output}");
        assert!(!output.contains("seven.sliccy.ai"), "protected auth link must be filtered: {output}");
    }

    #[test]
    fn handle_response_401_from_protected_url_produces_nothing() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "XHR",
            "frameId": "F1",
            "response": {
                "url": "https://vault.sliccy.ai/secret",
                "status": 401,
                "headers": { "www-authenticate": "Bearer realm=\"vault\"" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.is_empty(), "401 from protected URL must produce no event: {output}");
        assert!(!state.probed_origins.contains("https://vault.sliccy.ai"), "must not probe protected origin");
    }

    #[test]
    fn handle_response_401_from_a_catalog_debug_port_is_reported() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: true,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        state.sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".to_string(),
                top_frame_id: Some("F1".to_string()),
                page_url: "https://example.com/".to_string(),
            },
        );
        let params = json!({
            "type": "Fetch",
            "frameId": "F1",
            "response": {
                "url": "http://127.0.0.1:9222/json",
                "status": 401,
                "headers": { "www-authenticate": "Bearer" }
            }
        });
        let mut cdp = Cdp::from_transport(Box::new(FakeTransport));
        let output = handle_response(&mut cdp, &mut state, "S1", &params);
        assert!(output.contains(r#""standard":"auth-md","origin":"http://127.0.0.1:9222""#), "{output}");
    }

    #[test]
    fn emit_nlweb_from_catalog_with_nlweb_entry() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let summary = json!({
            "name": "Test",
            "entries": [{
                "identifier": "urn:nlweb:search",
                "displayName": "NLWeb Search",
                "type": "application/nlweb+json",
                "url": "https://example.com/ask"
            }]
        });
        let output = emit_nlweb_from_catalog(
            &mut state,
            "https://example.com",
            "https://example.com/",
            "T1",
            &summary,
        );
        assert!(output.contains("nlweb"), "expected nlweb event: {output}");
        let parsed: Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(parsed["standard"], "nlweb");
        assert_eq!(parsed["via"], "catalog");
        assert!(parsed["summary"]["askUrl"].as_str().unwrap().contains("/ask"));
    }

    #[test]
    fn emit_nlweb_from_catalog_no_nlweb_entries() {
        let mut state = WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions: BTreeMap::new(),
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: false,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let summary = json!({
            "name": "Test",
            "entries": [{
                "identifier": "urn:example:api",
                "displayName": "Regular API",
                "type": "application/json",
                "url": "https://example.com/api"
            }]
        });
        let output = emit_nlweb_from_catalog(
            &mut state,
            "https://example.com",
            "https://example.com/",
            "T1",
            &summary,
        );
        assert!(output.is_empty(), "no nlweb entry should produce no event: {output}");
    }

    struct ScriptedTransport {
        incoming: std::collections::VecDeque<String>,
        sent: std::rc::Rc<std::cell::RefCell<Vec<Value>>>,
    }

    impl cdp_client::cdp::Transport for ScriptedTransport {
        fn send_text(&mut self, text: &str) -> Result<(), String> {
            self.sent.borrow_mut().push(serde_json::from_str(text).unwrap());
            Ok(())
        }

        fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
            Ok(self.incoming.pop_front())
        }
    }

    fn scripted_cdp(replies: Vec<Value>) -> (Cdp, std::rc::Rc<std::cell::RefCell<Vec<Value>>>) {
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let incoming = replies
            .into_iter()
            .enumerate()
            .map(|(i, result)| json!({ "id": i as i64 + 1, "result": result }).to_string())
            .collect();
        let transport = ScriptedTransport { incoming, sent: sent.clone() };
        (Cdp::from_transport(Box::new(transport)), sent)
    }

    fn fetched(status: u16, ct: &str, body: &str) -> Value {
        json!({ "result": { "type": "string", "value": json!({ "status": status, "ct": ct, "body": body }).to_string() } })
    }

    fn nlweb_state() -> WatcherState {
        let mut sessions = BTreeMap::new();
        sessions.insert(
            "S1".to_string(),
            SessionState {
                target_id: "T1".into(),
                top_frame_id: Some("F1".into()),
                page_url: "https://example.com/".into(),
            },
        );
        WatcherState {
            dedup: DedupSet::new(),
            probed_origins: BTreeSet::new(),
            probes: default_probes(),
            classifiers: default_classifiers(),
            sessions,
            json_mode: true,
            follow_mode: false,
            robots_probe: false, nlweb_probe: true,
            no_probe: false,
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        }
    }

    #[test]
    fn nlweb_probes_go_through_the_guarded_isolated_fetch() {
        let info = json!({ "targetInfo": { "targetId": "F1", "type": "page", "url": "https://example.com/" } });
        let world = json!({ "executionContextId": 7 });
        let (mut cdp, sent) = scripted_cdp(vec![
            info.clone(),
            world.clone(),
            fetched(200, "application/json", r#"{"name":"Shop","results":[]}"#),
            info,
            world,
            fetched(200, "application/json", r#"{"result":{"serverInfo":{"name":"nlweb-mcp","version":"1"}}}"#),
        ]);
        let mut state = nlweb_state();
        let output = run_nlweb_probes(&mut cdp, &mut state, "S1", "https://example.com");
        let event: Value = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(event["url"], "https://example.com/ask");
        assert_eq!(event["summary"]["askUrl"], "https://example.com/ask");
        assert_eq!(event["summary"]["mcpUrl"], "https://example.com/mcp");
        assert_eq!(event["summary"]["serverName"], "Shop");
        let expressions: Vec<String> = sent
            .borrow()
            .iter()
            .filter(|m| m["method"] == "Runtime.evaluate")
            .map(|m| m["params"]["expression"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(expressions.len(), 2);
        assert!(expressions[0].contains(r#""headers":{"Accept":"application/json"}"#), "{}", expressions[0]);
        assert!(!expressions[0].contains("POST"));
        assert!(expressions[1].contains(r#""method":"POST""#), "{}", expressions[1]);
        assert!(expressions[1].contains(r#""Content-Type":"application/json""#));
        assert!(expressions[1].contains(r#""Accept":"application/json, text/event-stream""#));
        assert!(expressions[0].contains("/ask?query=nlweb&streaming=false"));
        assert!(expressions[0].contains("const sseUntil = null;"));
        assert!(expressions[1].contains(r#"const sseUntil = "result";"#), "{}", expressions[1]);
        assert!(expressions[1].contains("initialize"));
        for expression in &expressions {
            assert!(expression.contains(r#""credentials":"omit""#));
            assert!(expression.contains(r#""redirect":"manual""#));
        }
    }

    #[test]
    fn a_refused_nlweb_fetch_does_not_fall_back_to_native() {
        let info = json!({ "targetInfo": { "targetId": "F1", "type": "page", "url": "http://127.0.0.1:1/" } });
        let world = json!({ "executionContextId": 7 });
        let refused = json!({ "result": { "type": "string", "value": r#"{"err":"redirect"}"# } });
        let (mut cdp, sent) = scripted_cdp(vec![info.clone(), world.clone(), refused.clone(), info, world, refused]);
        let mut state = nlweb_state();
        let output = run_nlweb_probes(&mut cdp, &mut state, "S1", "http://127.0.0.1:1");
        assert!(output.is_empty(), "{output}");
        let evaluations = sent.borrow().iter().filter(|m| m["method"] == "Runtime.evaluate").count();
        assert_eq!(evaluations, 2);
    }

    #[test]
    fn an_unavailable_isolated_world_falls_back_to_native() {
        let (mut cdp, _) = scripted_cdp(vec![]);
        let mut state = nlweb_state();
        let output = run_nlweb_probes(&mut cdp, &mut state, "S1", "http://127.0.0.1:1");
        assert!(output.is_empty(), "{output}");
    }

    fn serve_nlweb() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let catalog = json!({
            "name": "NLWeb Catalog",
            "entries": [{ "identifier": "urn:nlweb:search", "displayName": "NLWeb Search", "type": "application/nlweb+json", "url": format!("{origin}/ask") }]
        })
        .to_string();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 8192];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let line = request.lines().next().unwrap_or("").to_string();
                let mut content_type = "application/json";
                let (status, body) = if line.starts_with("GET /robots.txt ") {
                    content_type = "text/plain";
                    ("200 OK", "Agentmap: /agentmap.json\n".to_string())
                } else if line.starts_with("GET /.well-known/ard.json ") || line.starts_with("GET /agentmap.json ") {
                    ("200 OK", catalog.clone())
                } else if line.starts_with("GET /ask?query=nlweb&streaming=false ") {
                    ("200 OK", r#"{"name":"Shop","results":[]}"#.to_string())
                } else if line.starts_with("POST /mcp ") {
                    ("200 OK", r#"{"result":{"serverInfo":{"name":"NLWeb Server","version":"0.5"}}}"#.to_string())
                } else {
                    ("404 Not Found", "{}".to_string())
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        origin
    }

    #[test]
    fn native_probes_report_catalog_and_verified_nlweb_endpoints() {
        let origin = serve_nlweb();
        let (mut cdp, _) = scripted_cdp(vec![]);
        let mut state = nlweb_state();
        state.robots_probe = true;
        state.sessions.get_mut("S1").unwrap().page_url = format!("{origin}/");
        let output = run_probes(&mut cdp, &mut state, "S1", &origin);
        let events: Vec<Value> = output.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert!(events.iter().any(|e| e["standard"] == "ard" && e["url"].as_str().unwrap().ends_with("/agentmap.json")), "{output}");
        let nlweb: Vec<&Value> = events.iter().filter(|e| e["standard"] == "nlweb").collect();
        assert_eq!(nlweb.len(), 2, "{output}");
        assert_eq!(nlweb[0]["via"], "catalog");
        assert_eq!(nlweb[1]["summary"]["serverInfo"]["name"], "NLWeb Server");
        assert_eq!(nlweb[1]["summary"]["serverName"], "Shop");
    }

    #[test]
    fn a_catalog_link_header_reports_its_nlweb_entry() {
        let catalog = json!({
            "name": "NLWeb Catalog",
            "entries": [{ "identifier": "urn:nlweb:search", "displayName": "NLWeb Search", "type": "application/nlweb+json", "url": "https://example.com/ask" }]
        })
        .to_string();
        let link = json!({
            "method": "Network.responseReceived",
            "sessionId": "S1",
            "params": { "response": { "headers": { "Link": "</.well-known/ard.json>; rel=\"ard\"" } } }
        });
        let replies = [
            json!({ "targetInfo": { "targetId": "F1", "type": "page", "url": "https://example.com/" } }),
            json!({ "executionContextId": 7 }),
            fetched(200, "application/json", &catalog),
        ];
        let mut incoming: std::collections::VecDeque<String> = replies
            .iter()
            .enumerate()
            .map(|(i, result)| json!({ "id": i as i64 + 1, "result": result }).to_string())
            .collect();
        incoming.push_front(link.to_string());
        let sent = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut cdp = Cdp::from_transport(Box::new(ScriptedTransport { incoming, sent }));
        cdp.pump(Instant::now()).unwrap();
        let mut state = nlweb_state();
        let output = follow_links(&mut cdp, &mut state, "S1");
        assert!(output.lines().any(|l| l.contains(r#""standard":"nlweb""#) && l.contains(r#""via":"catalog""#)), "{output}");
    }

    #[test]
    fn a_followed_catalog_reports_its_nlweb_entry() {
        let child = json!({
            "name": "Child",
            "entries": [{ "identifier": "urn:nlweb:search", "displayName": "NLWeb Search", "type": "application/nlweb+json", "url": "https://example.com/ask" }]
        })
        .to_string();
        let (mut cdp, _) = scripted_cdp(vec![
            json!({ "targetInfo": { "targetId": "F1", "type": "page", "url": "https://example.com/" } }),
            json!({ "executionContextId": 7 }),
            fetched(200, "application/json", &child),
        ]);
        let mut state = nlweb_state();
        let parent = json!({ "catalogs": [{ "url": "https://example.com/child.json" }] });
        let output = follow_ard_catalogs(
            &mut cdp, &mut state, "S1", "https://example.com", "F1", "https://example.com/", "T1", &parent,
        );
        let nlweb: Vec<Value> = output
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|event| event["standard"] == "nlweb")
            .collect();
        assert_eq!(nlweb.len(), 1, "{output}");
        assert_eq!(nlweb[0]["summary"]["askUrl"], "https://example.com/ask");
    }

    #[test]
    fn nlweb_probes_run_once_and_skip_guarded_or_unknown_sessions() {
        let (mut cdp, _) = scripted_cdp(vec![]);
        let mut state = nlweb_state();
        assert!(run_nlweb_probes(&mut cdp, &mut state, "S1", "http://127.0.0.1:1").is_empty());
        assert!(run_nlweb_probes(&mut cdp, &mut state, "S1", "http://127.0.0.1:1").is_empty());
        assert!(run_nlweb_probes(&mut cdp, &mut state, "S1", "https://vault.sliccy.ai").is_empty());
        assert!(run_nlweb_probes(&mut cdp, &mut state, "NONE", "http://127.0.0.1:2").is_empty());
        state.sessions.get_mut("S1").unwrap().top_frame_id = None;
        assert!(run_nlweb_probes(&mut cdp, &mut state, "S1", "http://127.0.0.1:3").is_empty());
    }

    #[test]
    fn a_catalog_nlweb_event_is_emitted_once() {
        let mut state = nlweb_state();
        let summary = json!({ "entries": [{ "type": "application/nlweb+json", "url": "https://example.com/mcp" }] });
        let first = emit_nlweb_from_catalog(&mut state, "https://example.com", "https://example.com/", "T1", &summary);
        let event: Value = serde_json::from_str(first.trim()).unwrap();
        assert_eq!(event["url"], "https://example.com/mcp");
        assert!(emit_nlweb_from_catalog(&mut state, "https://example.com", "https://example.com/", "T1", &summary).is_empty());
    }

    #[test]
    fn an_sse_initialize_reply_yields_its_json_rpc_result() {
        let sse = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\r\n\r\nevent: message\r\ndata: {\"result\":\r\ndata:{\"serverInfo\":{\"name\":\"NLWeb\"}}}\r\n\r\n";
        let payload = mcp_init_payload(200, "text/event-stream; charset=utf-8", sse.into()).unwrap();
        assert_eq!(classify_nlweb_mcp_init(&payload).unwrap()["serverInfo"]["name"], "NLWeb");
        assert_eq!(mcp_init_payload(200, "text/event-stream", "data: nope\n\n".into()), None);
        let bare_cr = "event: message\rdata: {\"result\":{\"serverInfo\":{\"name\":\"NLWeb\"}}}\r\r";
        assert!(mcp_init_payload(200, "text/event-stream", bare_cr.into()).is_some());
        let bom = "\u{feff}data: {\"result\":{}}\n\n";
        assert_eq!(mcp_init_payload(200, "text/event-stream", bom.into()).as_deref(), Some("{\"result\":{}}"));
        let batched = "data: [{\"jsonrpc\":\"2.0\",\"method\":\"ping\"},{\"id\":1,\"result\":{\"serverInfo\":{\"name\":\"NLWeb\"}}}]\n\n";
        let payload = mcp_init_payload(200, "text/event-stream", batched.into()).unwrap();
        assert_eq!(classify_nlweb_mcp_init(&payload).unwrap()["serverInfo"]["name"], "NLWeb");
        assert_eq!(mcp_init_payload(200, "text/event-stream", "data: [{\"method\":\"ping\"}]\n\n".into()), None);
        assert_eq!(mcp_init_payload(406, "text/event-stream", sse.into()), None);
        assert_eq!(mcp_init_payload(200, "application/json", "{}".into()), None);
        assert_eq!(mcp_init_payload(200, "application/json", "nope".into()), None);
        assert_eq!(mcp_init_payload(200, "application/json", "[{\"result\":{}}]".into()).as_deref(), Some("{\"result\":{}}"));
    }

    #[test]
    fn a_non_json_nlweb_reply_is_not_verified() {
        assert_eq!(json_ok_body(200, "application/json; charset=utf-8", "{}".into()).as_deref(), Some("{}"));
        assert_eq!(json_ok_body(200, "text/html", "{}".into()), None);
        assert_eq!(json_ok_body(404, "application/json", "{}".into()), None);
    }

    #[test]
    fn a_plain_get_sends_only_the_guards() {
        assert_eq!(
            fetch_options(&PLAIN_GET),
            json!({ "credentials": "omit", "redirect": "manual", "cache": "no-store" })
        );
        let options = fetch_options(&FetchInit { method: Some("PUT"), headers: &[("X", "1")], body: Some("b"), sse_until: None });
        assert_eq!(options["method"], "PUT");
        assert_eq!(options["headers"], json!({ "X": "1" }));
        assert_eq!(options["body"], "b");
        assert_eq!(options["credentials"], "omit");
        assert!(FetchInit::default().method.is_none());
    }

    #[test]
    fn the_mcp_initialize_body_is_json_rpc() {
        let body: Value = serde_json::from_str(&nlweb_mcp_init_body()).unwrap();
        assert_eq!(body["method"], "initialize");
        assert_eq!(body["jsonrpc"], "2.0");
    }

    #[test]
    fn nlweb_probe_flag_parsed() {
        let mut flags = BTreeMap::new();
        flags.insert("off".to_string(), "true".to_string());
        flags.insert("probe".to_string(), "nlweb".to_string());
        let invocation = Invocation {
            command: Some("discover".to_string()),
            positionals: vec![],
            flags,
            multi: BTreeMap::new(),
            cdp: None,
            runtime: None,
            session: None,
            full_urls: false,
            help: false,
            command_help: false,
            version: false,
            cwd: None,
        };
        let output = run(&invocation, None);
        assert_eq!(output.code, 0);
    }

    #[test]
    fn repeated_probe_options_reach_the_connection_step() {
        let argv: Vec<String> = [
            "discover", "--json", "--cdp", "ws://127.0.0.1:1/devtools/browser/x",
            "--probe", "robots", "--probe", "nlweb",
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
        let invocation = crate::args::parse(&argv).unwrap();
        assert_eq!(invocation.repeated("probe"), ["robots", "nlweb"]);
        let output = run(&invocation, None);
        assert_ne!(output.code, 0);
    }

    #[test]
    fn a_ucp_summary_drops_protected_endpoints() {
        let cdp = Cdp::from_transport(Box::new(FakeTransport));
        let summary = json!({
            "mcp": ["https://vault.sliccy.ai/mcp", "https://shop.test/mcp"],
            "a2a": ["https://seven.sliccy.ai/card.json"],
            "services": { "s": [
                { "transport": "mcp", "endpoint": "https://vault.sliccy.ai/mcp" },
                { "transport": "embedded", "endpoint": null },
                { "transport": "rest", "endpoint": "https://shop.test/ucp" }
            ], "n": {} },
            "version": "2026-08-25"
        });
        let filtered = filter_protected_ucp(&cdp, &summary);
        assert_eq!(filtered["mcp"], json!(["https://shop.test/mcp"]));
        assert_eq!(filtered["a2a"], json!([]));
        assert_eq!(filtered["services"]["s"], json!([{ "transport": "embedded", "endpoint": null }, { "transport": "rest", "endpoint": "https://shop.test/ucp" }]));
        assert_eq!(filter_protected_ucp(&cdp, &json!(null)), json!(null));
    }
}
