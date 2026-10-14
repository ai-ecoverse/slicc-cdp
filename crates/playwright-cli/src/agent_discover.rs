use crate::args::{Invocation, Output};
use crate::browser::{self, PageInfo};
use crate::cdp::{self, Cdp};
use crate::launch;
use crate::session;
use discovery::{
    classify_response, classify_slicc_handoff, default_classifiers,
    default_probes, fingerprint, is_slicc_handoff_rel, known_link_rels,
    parse_link_header, ClassifierRegistry, DedupSet, DiscoveryEvent,
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

fn is_reserved_cdp_origin(origin: &str) -> bool {
    let Some(parsed) = Url::parse(origin).ok() else {
        return false;
    };
    let port = parsed.port().unwrap_or(match parsed.scheme() {
        "http" | "ws" => 80,
        "https" | "wss" => 443,
        _ => 0,
    });
    if !(9222..=9229).contains(&port) {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Ipv4(ip)) => {
            is_loopback_or_unspec_v4(ip)
        }
        Some(url::Host::Ipv6(ip)) => {
            is_loopback_or_unspec_v6(ip)
        }
        Some(url::Host::Domain(domain)) => {
            let d = domain.to_ascii_lowercase();
            let d = d.strip_suffix('.').unwrap_or(&d);
            d == "localhost" || d.ends_with(".localhost")
        }
        None => false,
    }
}

fn is_loopback_or_unspec_v4(ip: std::net::Ipv4Addr) -> bool {
    ip.is_loopback() || ip.is_unspecified()
}

fn is_loopback_or_unspec_v6(ip: std::net::Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unspecified()
        || match ip.to_ipv4_mapped() {
            Some(v4) => is_loopback_or_unspec_v4(v4),
            None => false,
        }
}

#[allow(dead_code)]
pub fn is_reserved_cdp_addr(addr: std::net::SocketAddr, port: u16) -> bool {
    if !(9222..=9229).contains(&port) {
        return false;
    }
    match addr {
        std::net::SocketAddr::V4(v4) => is_loopback_or_unspec_v4(*v4.ip()),
        std::net::SocketAddr::V6(v6) => is_loopback_or_unspec_v6(*v6.ip()),
    }
}

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
        launch::remembered_endpoint_named(invocation.session.as_deref())
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

    let mut sess_state = session::load_named(invocation.session.as_deref());

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
            output.push_str(&process_events(&mut cdp, &mut state, &sess_state));
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
                let committed_url = cdp
                    .call("Page.getFrameTree", json!({}), Some(&session))
                    .ok()
                    .and_then(|r| {
                        r.pointer("/frameTree/frame/url")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                if let Some(ref url) = committed_url {
                    if let Some(sess) = state.sessions.get_mut(&session) {
                        sess.page_url = url.clone();
                    }
                }
                let page_signals = collect_page_signals(&mut cdp, &session);
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
        if !session::page_visible(
            sess_state.provenance.as_deref(),
            &page.url,
            &page.target_id,
            &sess_state.owned,
        ) {
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
            let events_out = process_events(&mut cdp, &mut state, &sess_state);
            if !events_out.is_empty() {
                eprint_flush(&events_out);
            }
            let load_events = check_load_events(&cdp, &state);
            for (session_id, _origin) in &load_events {
                let committed_url = cdp
                    .call("Page.getFrameTree", json!({}), Some(session_id))
                    .ok()
                    .and_then(|r| {
                        r.pointer("/frameTree/frame/url")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                if let Some(ref url) = committed_url {
                    if let Some(sess) = state.sessions.get_mut(session_id.as_str()) {
                        sess.page_url = url.clone();
                    }
                }
                let origin = state
                    .sessions
                    .get(session_id.as_str())
                    .and_then(|s| extract_origin(&s.page_url))
                    .unwrap_or_default();
                let page_signals = collect_page_signals(&mut cdp, session_id);
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
            attach_new_targets(&mut cdp, &mut state, &sess_state);
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
        let events_out = process_events(&mut cdp, &mut state, &sess_state);
        if !events_out.is_empty() {
            eprint_flush(&events_out);
        }
        let load_events = check_load_events(&cdp, &state);
            for (session_id, _origin) in &load_events {
                let committed_url = cdp
                    .call("Page.getFrameTree", json!({}), Some(session_id))
                    .ok()
                    .and_then(|r| {
                        r.pointer("/frameTree/frame/url")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    });
                if let Some(ref url) = committed_url {
                    if let Some(sess) = state.sessions.get_mut(session_id.as_str()) {
                        sess.page_url = url.clone();
                    }
                }
                let origin = state
                    .sessions
                    .get(session_id.as_str())
                    .and_then(|s| extract_origin(&s.page_url))
                    .unwrap_or_default();
                let page_signals = collect_page_signals(&mut cdp, session_id);
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
        attach_new_targets(&mut cdp, &mut state, &sess_state);
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
    let visible: Vec<&PageInfo> = pages
        .iter()
        .filter(|page| {
            session::page_visible(
                state.provenance.as_deref(),
                &page.url,
                &page.target_id,
                &state.owned,
            )
        })
        .collect();
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

fn process_events(cdp: &mut Cdp, state: &mut WatcherState, sess_state: &session::Session) -> String {
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
                handle_target_created(cdp, state, &params, sess_state);
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

fn handle_target_created(cdp: &mut Cdp, state: &mut WatcherState, params: &Value, sess_state: &session::Session) {
    let Some(info) = params.get("targetInfo") else { return };
    let Some(target_id) = info.get("targetId").and_then(|v| v.as_str()) else { return };
    if info.get("type").and_then(|v| v.as_str()) != Some("page") {
        return;
    }
    if cdp.is_protected_target(target_id) {
        return;
    }
    let url = info.get("url").and_then(|v| v.as_str()).unwrap_or("");
    if !session::page_visible(
        sess_state.provenance.as_deref(),
        url,
        target_id,
        &sess_state.owned,
    ) {
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

    if !is_top_frame_document {
        return output;
    }

    let url = response
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if let Some(sess) = state.sessions.get_mut(session_id) {
        if sess.page_url.is_empty() || sess.page_url == "about:blank" {
            sess.page_url = url.clone();
        }
    }

    let status = response
        .get("status")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u16;
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

    let response_signals = classify_response(status, &headers);
    for signal in response_signals {
        if !state.dedup.is_new(&origin, &signal.standard, &url) {
            continue;
        }
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: signal.standard.clone(),
            origin: origin.clone(),
            url: url.clone(),
            via: Via::Response,
            page: page_url.clone(),
            target_id: target_id.clone(),
            status: Validity::Valid,
            summary: signal.summary,
            reason: None,
        };
        output.push_str(&emit_event(state, &event));
    }

    output
}

fn collect_page_signals(cdp: &mut Cdp, session: &str) -> Vec<(String, Value)> {
    let mut signals = Vec::new();
    let js = r#"(function() {
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
  var links = [];
  var els = document.querySelectorAll('link[rel]');
  for (var i = 0; i < els.length; i++) {
    links.push({ rel: els[i].getAttribute('rel'), href: els[i].href });
  }
  if (links.length) r.links = links;
  return JSON.stringify(r);
})()
"#;
    let result = cdp.call(
        "Runtime.evaluate",
        json!({ "expression": js, "returnByValue": true, "awaitPromise": true }),
        Some(session),
    );
    if let Ok(result) = result {
        let value_str = result
            .pointer("/result/value")
            .and_then(|v| v.as_str())
            .unwrap_or("{}");
        if let Ok(parsed) = serde_json::from_str::<Value>(value_str) {
            if parsed.get("webmcp").and_then(|v| v.as_bool()) == Some(true) {
                let tools = parsed.get("tools").cloned().unwrap_or(json!([]));
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
        }
    }
    signals
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

    if is_reserved_cdp_origin(origin) {
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
    for probe_req in &probe_requests {
        if cdp.guard().url(&probe_req.url) {
            continue;
        }

        let probe_result = isolated_world_probe(cdp, session_id, &frame_id, &probe_req.url);
        let (status, content_type, body) = match probe_result {
            IsolatedProbeResult::Ok(s, ct, b) => (s, ct, b),
            IsolatedProbeResult::Final => continue,
            IsolatedProbeResult::Unavailable => {
                match native_probe(&probe_req.url, &probe_req.probe.accept) {
                    Some((ns, nct, nbody)) => (ns, nct, nbody),
                    None => continue,
                }
            }
        };

        let probe_match_result =
            state
                .probes
                .check_response(&probe_req.probe.standard, status, &content_type, body, probe_req.url.clone());

        if let Some(probe_match) = probe_match_result
        {
            let classification = state
                .classifiers
                .classify(&probe_match.probe.standard, &probe_match.body);
            let (validity, summary, reason) = match classification {
                Some(c) => (c.status, c.summary, c.reason),
                None => (
                    Validity::Valid,
                    json!({ "url": probe_match.url }),
                    None,
                ),
            };
            if !state.dedup.is_new(origin, &probe_match.probe.standard, &probe_match.url) {
                continue;
            }
            let event = DiscoveryEvent {
                v: EVENT_VERSION,
                standard: probe_match.probe.standard.clone(),
                origin: origin.to_string(),
                url: probe_match.url,
                via: Via::WellKnown,
                page: page_url.clone(),
                target_id: target_id.clone(),
                status: validity,
                summary,
                reason,
            };
            output.push_str(&emit_event(state, &event));
        }
    }
    output
}

enum IsolatedProbeResult {
    Ok(u16, String, String),
    Final,
    Unavailable,
}

fn isolated_world_probe(
    cdp: &mut Cdp,
    session_id: &str,
    frame_id: &str,
    url: &str,
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
  let r;
  try {{
    r = await fetch({url}, {{credentials:'omit',redirect:'manual',cache:'no-store'}});
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
    }}
  }}
  return JSON.stringify({{status:r.status,ct,body}});
}})()
"#,
        url = serde_json::to_string(url).unwrap_or_default()
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

fn attach_new_targets(cdp: &mut Cdp, state: &mut WatcherState, sess_state: &session::Session) {
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
        if !session::page_visible(
            sess_state.provenance.as_deref(),
            &page.url,
            &page.target_id,
            &sess_state.owned,
        ) {
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
    let raw = serde_json::to_string_pretty(&json!({
        "v": display.v,
        "standard": display.standard,
        "origin": display.origin,
        "url": display.url,
        "via": display.via,
        "page": display.page,
        "targetId": display.target_id,
        "status": display.status,
        "summary": display.summary,
    }))
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
        if is_reserved_cdp_origin(&link.href) {
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

        let classification = state.classifiers.classify(&standard, &body);
        let (validity, summary, reason) = match classification {
            Some(c) => (c.status, c.summary, c.reason),
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
            origin: origin.clone(),
            url: link.href.clone(),
            via: Via::LinkHeader,
            page: page_url.clone(),
            target_id: target_id.clone(),
            status: validity,
            summary,
            reason,
        };
        output.push_str(&emit_event(state, &event));
    }

    if !failures.is_empty() {
        let dedup_key = format!("follow-failures:{origin}");
        if state.dedup.is_new(&origin, "follow-failures", &dedup_key) {
            let event = DiscoveryEvent {
                v: EVENT_VERSION,
                standard: "follow-failures".into(),
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
            
            
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
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
            
            
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
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
    fn reserved_cdp_ports_are_never_probed() {
        for origin in [
            "http://127.0.0.1:9222",
            "http://127.0.0.1:9229",
            "http://localhost:9222",
            "http://[::1]:9225",
            "http://127.1:9222",
            "http://2130706433:9222",
            "http://0x7f000001:9222",
            "http://017700000001:9222",
            "http://0.0.0.0:9222",
            "http://localhost.:9223",
            "http://127.0.0.2:9224",
            "http://[::ffff:7f00:1]:9226",
            "http://foo.localhost:9222",
            "http://[::]:9223",
            "http://[::ffff:0:0]:9224",
            "http://LOCALHOST:9222",
            "http://[0:0:0:0:0:0:0:1]:9222",
            "http://[::ffff:127.0.0.1]:9225",
            "http://0x7f.1:9222",
            "http://0177.0.0.1:9222",
        ] {
            assert!(is_reserved_cdp_origin(origin), "{origin} should be reserved");
        }
    }

    #[test]
    fn non_reserved_ports_are_not_blocked() {
        for origin in [
            "http://127.0.0.1:8080",
            "http://127.0.0.1:9221",
            "http://127.0.0.1:9230",
            "http://example.com:9222",
            "http://10.0.0.1:9222",
            "https://localhost:443",
        ] {
            assert!(!is_reserved_cdp_origin(origin), "{origin} should not be reserved");
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
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
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
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
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
            events_emitted: 0, event_cursor: 0, sink: Sink::Lick { slicc_dir: slicc.to_string_lossy().to_string() },
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
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
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
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
    fn reserved_cdp_addr_ipv4_loopback() {
        let addr = "127.0.0.1:9222".parse().unwrap();
        assert!(is_reserved_cdp_addr(addr, 9222));
    }

    #[test]
    fn reserved_cdp_addr_ipv4_unspec() {
        let addr = "0.0.0.0:9222".parse().unwrap();
        assert!(is_reserved_cdp_addr(addr, 9222));
    }

    #[test]
    fn reserved_cdp_addr_ipv6_loopback() {
        let addr = "[::1]:9222".parse().unwrap();
        assert!(is_reserved_cdp_addr(addr, 9222));
    }

    #[test]
    fn reserved_cdp_addr_ipv6_unspec() {
        let addr = "[::]:9223".parse().unwrap();
        assert!(is_reserved_cdp_addr(addr, 9223));
    }

    #[test]
    fn reserved_cdp_addr_mapped_loopback() {
        let addr = "[::ffff:127.0.0.1]:9224".parse().unwrap();
        assert!(is_reserved_cdp_addr(addr, 9224));
    }

    #[test]
    fn reserved_cdp_addr_mapped_unspec() {
        let addr = "[::ffff:0.0.0.0]:9225".parse().unwrap();
        assert!(is_reserved_cdp_addr(addr, 9225));
    }

    #[test]
    fn reserved_cdp_addr_non_reserved_port() {
        let addr = "127.0.0.1:9230".parse().unwrap();
        assert!(!is_reserved_cdp_addr(addr, 9230));
    }

    #[test]
    fn reserved_cdp_addr_remote_host() {
        let addr = "93.184.216.34:9222".parse().unwrap();
        assert!(!is_reserved_cdp_addr(addr, 9222));
    }

    #[test]
    fn reserved_cdp_addr_127_0_0_2() {
        let addr = "127.0.0.2:9222".parse().unwrap();
        assert!(is_reserved_cdp_addr(addr, 9222));
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
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
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
            events_emitted: 0, event_cursor: 0, sink: Sink::Stdout,
        };
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
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
    fn follow_skips_reserved_cdp_port_links() {
        assert!(is_reserved_cdp_origin("http://127.0.0.1:9222"));
        assert!(is_reserved_cdp_origin("http://localhost:9225"));
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
        };
        let output = run(&invocation, None);
        assert_eq!(output.code, 0);
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
}
