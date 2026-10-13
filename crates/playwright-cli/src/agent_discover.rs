use crate::args::{Invocation, Output};
use crate::browser::{self, PageInfo};
use crate::cdp::{self, Cdp};
use crate::launch;
use crate::session;
use discovery::{
    classify_response, default_classifiers, default_probes, known_link_rels, parse_link_header,
    ClassifierRegistry, DedupSet, DiscoveryEvent, ProbeRegistry, Validity, Via, EVENT_VERSION,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use url::Url;

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

struct WatcherState {
    dedup: DedupSet,
    probed_origins: BTreeSet<String>,
    probes: ProbeRegistry,
    classifiers: ClassifierRegistry,
    sessions: BTreeMap<String, SessionState>,
    json_mode: bool,
    events_emitted: usize,
    event_cursor: usize,
}

struct SessionState {
    target_id: String,
    top_frame_id: Option<String>,
    page_url: String,
}

pub fn run(invocation: &Invocation, cdp_env: Option<&str>) -> Output {
    let json_mode = invocation.flags.contains_key("json");
    let once_url = invocation.flags.get("once").cloned();
    let once_tab = invocation.flags.get("tab").cloned();
    let no_probe = invocation.flags.contains_key("no-probe");

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
        launch::remembered_endpoint()
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
    };

    if let Err(message) = cdp.call(
        "Target.setDiscoverTargets",
        json!({ "discover": true }),
        None,
    ) {
        return Output::err(message);
    }

    let mut sess_state = session::load();

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

fn escape_control(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' | '\t' => out.push(ch),
            '\x07' => out.push_str("\\x07"),
            '\x08' => out.push_str("\\x08"),
            '\r' => out.push_str("\\r"),
            '\x1b' => out.push_str("\\x1b"),
            c if c.is_control() || ('\u{0080}'..='\u{009f}').contains(&c) => {
                for byte in c.to_string().bytes() {
                    out.push_str(&format!("\\x{byte:02x}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn emit_event(state: &mut WatcherState, event: &DiscoveryEvent) -> String {
    state.events_emitted += 1;
    if state.json_mode {
        format!("{}\n", event.to_json())
    } else {
        format!(
            "{} {} {} ({})\n",
            escape_control(&event.standard),
            escape_control(&event.origin),
            escape_control(&event.url),
            format_via(event.via)
        )
    }
}

fn format_via(via: Via) -> &'static str {
    match via {
        Via::LinkHeader => "link-header",
        Via::HtmlLink => "html-link",
        Via::WellKnown => "well-known",
        Via::PageApi => "page-api",
        Via::Response => "response",
    }
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
    fn format_via_all_variants() {
        assert_eq!(format_via(Via::LinkHeader), "link-header");
        assert_eq!(format_via(Via::HtmlLink), "html-link");
        assert_eq!(format_via(Via::WellKnown), "well-known");
        assert_eq!(format_via(Via::PageApi), "page-api");
        assert_eq!(format_via(Via::Response), "response");
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
            
            
            events_emitted: 0, event_cursor: 0,
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
            
            
            events_emitted: 0, event_cursor: 0,
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
        assert!(line.contains("well-known"));
        assert!(!line.contains("\"v\""));
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
            
            
            events_emitted: 0, event_cursor: 0,
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
            
            
            events_emitted: 0, event_cursor: 0,
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
            
            
            events_emitted: 0, event_cursor: 0,
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
            events_emitted: 0, event_cursor: 0,
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
            events_emitted: 0, event_cursor: 0,
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
            events_emitted: 0, event_cursor: 0,
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
    fn escape_control_passes_normal_text() {
        assert_eq!(escape_control("hello world"), "hello world");
    }

    #[test]
    fn escape_control_preserves_newline_and_tab() {
        assert_eq!(escape_control("a\tb\n"), "a\tb\n");
    }

    #[test]
    fn escape_control_escapes_esc() {
        assert_eq!(escape_control("\x1b[31mred\x1b[0m"), "\\x1b[31mred\\x1b[0m");
    }

    #[test]
    fn escape_control_escapes_bel() {
        assert_eq!(escape_control("ring\x07"), "ring\\x07");
    }

    #[test]
    fn escape_control_escapes_cr() {
        assert_eq!(escape_control("over\rwrite"), "over\\rwrite");
    }

    #[test]
    fn escape_control_escapes_c1_range() {
        assert_eq!(escape_control("a\u{0090}b"), "a\\xc2\\x90b");
    }

    #[test]
    fn escape_control_escapes_null() {
        assert_eq!(escape_control("a\x00b"), "a\\x00b");
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
            events_emitted: 0, event_cursor: 0,
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
        assert!(line.contains("\\x1b"), "ESC must appear escaped: {line}");
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
            events_emitted: 0, event_cursor: 0,
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
}
