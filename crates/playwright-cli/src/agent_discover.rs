use crate::args::{Invocation, Output};
use crate::browser::{self, PageInfo};
use crate::cdp::{self, Cdp};
use crate::launch;
use crate::session;
use discovery::{
    classify_response, default_classifiers, default_probes, fingerprint, known_link_rels,
    parse_link_header, ClassifierRegistry, DedupSet, DiscoveryEvent, ProbeRegistry, Validity, Via,
    EVENT_VERSION,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use url::Url;

#[cfg(unix)]
use std::sync::atomic::{AtomicBool, Ordering};

const IO_READ_CAP: usize = 1024 * 1024;

fn is_reserved_cdp_origin(origin: &str) -> bool {
    let Some(parsed) = Url::parse(origin).ok() else {
        return false;
    };
    let host = parsed.host_str().unwrap_or("").to_ascii_lowercase();
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1" | "[::1]") {
        return false;
    }
    let port = parsed.port().unwrap_or(match parsed.scheme() {
        "http" | "ws" => 80,
        "https" | "wss" => 443,
        _ => 0,
    });
    (9222..=9229).contains(&port)
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
            for (session_id, origin) in &load_events {
                let page_signals = collect_page_signals(&mut cdp, session_id);
                let signals_out = emit_page_signals(&mut state, session_id, &page_signals);
                if !signals_out.is_empty() {
                    eprint_flush(&signals_out);
                }
                if !no_probe && !origin.is_empty() {
                    let probes_out = run_probes(&mut cdp, &mut state, session_id, origin);
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
        for (session_id, origin) in &load_events {
            let page_signals = collect_page_signals(&mut cdp, session_id);
            let signals_out = emit_page_signals(&mut state, session_id, &page_signals);
            if !signals_out.is_empty() {
                eprint_flush(&signals_out);
            }
            if !no_probe && !origin.is_empty() {
                let probes_out = run_probes(&mut cdp, &mut state, session_id, origin);
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
                if !state.dedup.is_new(&origin, &link.rel, &link.href) {
                    continue;
                }
                let event = DiscoveryEvent {
                    v: EVENT_VERSION,
                    standard: link.rel.clone(),
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
                    if known_link_rels().contains(&rel.as_str()) && !href.is_empty() {
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

        let probe_result = load_network_resource(cdp, session_id, &frame_id, &probe_req.url);
        let (status, content_type, body) = match probe_result {
            Some(r) => r,
            None => {
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

fn load_network_resource(
    cdp: &mut Cdp,
    session_id: &str,
    frame_id: &str,
    url: &str,
) -> Option<(u16, String, String)> {
    let result = cdp
        .call_for(
            "Network.loadNetworkResource",
            json!({
                "frameId": frame_id,
                "url": url,
                "options": {
                    "disableCache": false,
                    "includeCredentials": false
                }
            }),
            Some(session_id),
            Duration::from_secs(3),
        )
        .ok()?;
    let resource = result.get("resource")?;
    let success = resource.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
    if !success {
        return None;
    }
    if let Some(final_url) = resource.get("url").and_then(|v| v.as_str()) {
        if cdp.guard().url(final_url) {
            return None;
        }
    }
    let status = resource
        .get("httpStatusCode")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u16;
    let headers_obj = resource
        .get("headers")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let content_type = headers_obj
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .and_then(|(_, v)| v.as_str())
        .unwrap_or("")
        .to_string();

    let stream = resource
        .get("stream")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let body = match stream {
        Some(handle) => {
            let mut body = String::new();
            loop {
                let read_result = cdp.call(
                    "IO.read",
                    json!({ "handle": handle }),
                    Some(session_id),
                );
                match read_result {
                    Ok(chunk) => {
                        let data = chunk
                            .get("data")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let base64 = chunk
                            .get("base64Encoded")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        if base64 {
                            if let Ok(bytes) = cdp_client::net::base64_decode(data) {
                                body.push_str(&String::from_utf8_lossy(&bytes));
                            }
                        } else {
                            body.push_str(data);
                        }
                        if body.len() > IO_READ_CAP {
                            body.truncate(IO_READ_CAP);
                            let _ = cdp.call("IO.close", json!({ "handle": handle }), Some(session_id));
                            break;
                        }
                        let eof = chunk
                            .get("eof")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);
                        if eof {
                            let _ = cdp.call("IO.close", json!({ "handle": handle }), Some(session_id));
                            break;
                        }
                    }
                    Err(_) => {
                        let _ = cdp.call("IO.close", json!({ "handle": handle }), Some(session_id));
                        break;
                    }
                }
            }
            body
        }
        None => String::new(),
    };

    Some((status, content_type, body))
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

fn emit_event(state: &mut WatcherState, event: &DiscoveryEvent) -> String {
    state.events_emitted += 1;
    let json_line = format!("{}\n", event.to_json());
    match &state.sink {
        Sink::Exec { cmd, timeout } => {
            run_exec_sink(cmd, *timeout, event);
        }
        Sink::Lick { slicc_dir } => {
            write_lick_file(slicc_dir, event);
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

fn write_lick_file(slicc_dir: &str, event: &DiscoveryEvent) {
    let fp = fingerprint(&event.origin, &event.standard, &event.url);
    let hash = cdp_client::hash::sha256_hex(fp.as_bytes());
    let path = format!("{slicc_dir}/discoveries/{hash}.json");
    let content = serde_json::to_string_pretty(&json!({
        "v": event.v,
        "standard": event.standard,
        "origin": event.origin,
        "url": event.url,
        "via": event.via,
        "page": event.page,
        "targetId": event.target_id,
        "status": event.status,
        "summary": event.summary,
    }))
    .unwrap_or_default();
    atomic_write(&path, content.as_bytes());
}

#[cfg(unix)]
fn run_exec_sink(cmd: &str, timeout: Duration, event: &DiscoveryEvent) {
    use std::io::Write;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let json = event.to_json();
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
        write_lick_file(&slicc.to_string_lossy(), &event);
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
        write_lick_file(&slicc.to_string_lossy(), &event);
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
}
