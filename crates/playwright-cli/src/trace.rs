use crate::cdp::Cdp;
use crate::net::base64_decode;
use crate::sanitize;
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleEntry {
    pub kind: String,
    pub text: String,
    pub url: String,
    pub line: u64,
    pub stamp: String,
    pub cookie_value: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetRequest {
    pub method: String,
    pub url: String,
    pub resource_type: String,
    pub status: Option<u16>,
    pub status_text: String,
    pub failed: Option<String>,
    pub mime: String,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Vec<(String, String)>,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
    pub response_base64: bool,
    pub cookie_scopes: Vec<Value>,
}

#[derive(Debug)]
pub enum Piece {
    Text(String),
    File { bytes: Vec<u8>, extension: String },
    Empty,
}

struct RawRequest {
    request_id: String,
    has_post_data: bool,
    done: bool,
    item: NetRequest,
}

pub fn precheck(command: &str, positionals: &[String]) -> Option<String> {
    if command == "console" {
        if let Some(level) = positionals.first() {
            if !valid_level(level) {
                return Some(format!(
                    "playwright-cli console: unknown min-level \"{level}\" (expected error, warning, info, or debug)\n"
                ));
            }
        }
    }
    if is_request_command(command) {
        let Some(index) = positionals.first() else {
            return Some(format!("playwright-cli {command}: index is required\n"));
        };
        if positive_index(index).is_none() {
            return Some(format!(
                "playwright-cli {command}: index must be a positive integer\n"
            ));
        }
    }
    None
}

pub fn is_request_command(command: &str) -> bool {
    matches!(
        command,
        "request" | "request-headers" | "request-body" | "response-headers" | "response-body"
    )
}

pub fn positive_index(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = text.parse::<usize>().ok()?;
    if value == 0 {
        None
    } else {
        Some(value)
    }
}

pub fn valid_level(level: &str) -> bool {
    matches!(level, "error" | "warning" | "info" | "debug")
}

pub fn enable(cdp: &mut Cdp, session: &str) -> Result<(), String> {
    tolerate(cdp, "Runtime.enable", session)?;
    tolerate(cdp, "Network.enable", session)?;
    Ok(())
}

fn tolerate(cdp: &mut Cdp, method: &str, session: &str) -> Result<(), String> {
    match cdp.call(method, json!({}), Some(session)) {
        Ok(_) => Ok(()),
        Err(err) if err.starts_with(&format!("{method}:")) => Ok(()),
        Err(err) => Err(err),
    }
}

pub fn absorb(cdp: &mut Cdp, session: &str) -> Result<(Vec<ConsoleEntry>, Vec<NetRequest>), String> {
    flush(cdp, session)?;
    let jar = cdp.call("Network.getAllCookies", json!({}), Some(session))?;
    let events = cdp.events().to_vec();
    let (mut console, mut raws) = from_events(&events, session);
    let mut cookies = jar.get("cookies").and_then(Value::as_array).cloned().unwrap_or_default();
    for event in &events {
        if let Some(associated) = event.pointer("/params/associatedCookies").and_then(Value::as_array) {
            cookies.extend(associated.iter().filter_map(|item| item.get("cookie").cloned()));
        }
    }
    let scope_jar = json!({ "cookies": cookies });
    for entry in &mut console {
        entry.cookie_value = cookies.iter().filter_map(|cookie| cookie.get("value").and_then(Value::as_str))
            .any(|value| !value.is_empty() && entry.text.contains(value));
    }
    for raw in &mut raws {
        raw.item.cookie_scopes = crate::cookie_headers::scopes(&raw.item, &scope_jar);
    }
    fetch_bodies(cdp, session, &mut raws)?;
    let requests = raws.into_iter().map(|raw| raw.item).collect();
    Ok((console, requests))
}

fn flush(cdp: &mut Cdp, session: &str) -> Result<(), String> {
    match cdp.call_for(
        "Runtime.evaluate",
        json!({ "expression": "0", "returnByValue": true }),
        Some(session),
        Duration::from_millis(300),
    ) {
        Ok(_) => Ok(()),
        Err(err)
            if err.starts_with("Runtime.evaluate:")
                || err.starts_with("Runtime.evaluate timed out") =>
        {
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn fetch_bodies(cdp: &mut Cdp, session: &str, raws: &mut [RawRequest]) -> Result<(), String> {
    for raw in raws {
        if raw.item.request_body.is_none() && raw.has_post_data {
            match cdp.call_for(
                "Network.getRequestPostData",
                json!({ "requestId": raw.request_id }),
                Some(session),
                Duration::from_secs(2),
            ) {
                Ok(result) => {
                    if let Some(text) = result.get("postData").and_then(|value| value.as_str()) {
                        raw.item.request_body = Some(text.to_string());
                    }
                }
                Err(err) if soft("Network.getRequestPostData", &err) => {}
                Err(err) => return Err(err),
            }
        }
        if raw.done && raw.item.response_body.is_none() && can_have_response_body(&raw.item) {
            match cdp.call_for(
                "Network.getResponseBody",
                json!({ "requestId": raw.request_id }),
                Some(session),
                Duration::from_secs(2),
            ) {
                Ok(result) => store_body(&mut raw.item, &result),
                Err(err) if soft("Network.getResponseBody", &err) => {}
                Err(err) => return Err(err),
            }
        }
    }
    Ok(())
}

fn soft(method: &str, err: &str) -> bool {
    err.starts_with(&format!("{method}:")) || err.starts_with(&format!("{method} timed out"))
}

fn store_body(item: &mut NetRequest, result: &Value) {
    let Some(body) = result.get("body").and_then(|value| value.as_str()) else {
        return;
    };
    if body.len() > 1_000_000 {
        return;
    }
    let encoded = result
        .get("base64Encoded")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    item.response_body = Some(body.to_string());
    item.response_base64 = encoded;
}

fn from_events(events: &[Value], session: &str) -> (Vec<ConsoleEntry>, Vec<RawRequest>) {
    let mut console = Vec::new();
    let mut raws = Vec::new();
    for event in events {
        if event.get("sessionId").and_then(|value| value.as_str()) != Some(session) {
            continue;
        }
        let method = event.get("method").and_then(|value| value.as_str()).unwrap_or("");
        let params = event.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "Runtime.consoleAPICalled" => {
                if let Some(entry) = console_from(&params) {
                    console.push(entry);
                }
            }
            "Network.requestWillBeSent" => on_request(&mut raws, &params),
            "Network.responseReceived" => on_response(&mut raws, &params),
            "Network.loadingFinished" => {
                if let Some(raw) = latest(&mut raws, request_id(&params).as_deref()) {
                    raw.done = true;
                }
            }
            "Network.loadingFailed" => {
                if let Some(raw) = latest(&mut raws, request_id(&params).as_deref()) {
                    raw.done = true;
                    let text = params
                        .get("errorText")
                        .and_then(|value| value.as_str())
                        .unwrap_or("Unknown error");
                    raw.item.failed = Some(text.to_string());
                }
            }
            "Network.requestWillBeSentExtraInfo" => {
                if let Some(raw) = latest(&mut raws, request_id(&params).as_deref()) {
                    let headers = headers_of(params.get("headers"));
                    if !headers.is_empty() {
                        raw.item.request_headers = headers;
                    }
                }
            }
            "Network.responseReceivedExtraInfo" => {
                if let Some(raw) = latest(&mut raws, request_id(&params).as_deref()) {
                    let headers = headers_of(params.get("headers"));
                    if !headers.is_empty() {
                        raw.item.response_headers = headers;
                    }
                }
            }
            _ => {}
        }
    }
    (console, raws)
}

fn console_from(params: &Value) -> Option<ConsoleEntry> {
    if params.get("executionContextId").and_then(|value| value.as_u64()) == Some(0) {
        return None;
    }
    let kind = params
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("log")
        .to_string();
    let frame = params.pointer("/stackTrace/callFrames/0");
    let url = frame
        .and_then(|value| value.get("url"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let line = frame
        .and_then(|value| value.get("lineNumber"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    Some(ConsoleEntry {
        kind,
        text: console_text(params),
        url,
        line,
        stamp: timestamp_text(params.get("timestamp")),
        cookie_value: false,
    })
}

fn timestamp_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Number(number)) => {
            let text = number.to_string();
            if text == "0" {
                String::new()
            } else {
                text
            }
        }
        _ => String::new(),
    }
}

fn console_text(params: &Value) -> String {
    params
        .get("args")
        .and_then(|value| value.as_array())
        .map(|args| {
            args.iter()
                .map(arg_preview)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn arg_preview(arg: &Value) -> String {
    if let Some(value) = arg.get("value") {
        return match value {
            Value::String(text) => text.clone(),
            Value::Null => "null".to_string(),
            other => other.to_string(),
        };
    }
    if let Some(value) = arg.get("unserializableValue").and_then(|value| value.as_str()) {
        return value.to_string();
    }
    if let Some(value) = arg.get("description").and_then(|value| value.as_str()) {
        return value.to_string();
    }
    String::new()
}

fn on_request(raws: &mut Vec<RawRequest>, params: &Value) {
    let request_id = request_id(params).unwrap_or_default();
    if let Some(redirect) = params.get("redirectResponse") {
        if let Some(raw) = latest(raws, Some(request_id.as_str())) {
            apply_response(raw, redirect);
            raw.done = true;
        }
    }
    let request = params.get("request").cloned().unwrap_or(Value::Null);
    let url = request
        .get("url")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let method = request
        .get("method")
        .and_then(|value| value.as_str())
        .unwrap_or("GET")
        .to_string();
    let post = request
        .get("postData")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let has_post_data = post.is_some()
        || request
            .get("hasPostData")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
    let resource_type = params
        .get("type")
        .and_then(|value| value.as_str())
        .map(resource_type)
        .unwrap_or_else(|| "other".to_string());
    raws.push(RawRequest {
        request_id,
        has_post_data,
        done: false,
        item: NetRequest {
            method,
            url,
            resource_type,
            status: None,
            status_text: String::new(),
            failed: None,
            mime: String::new(),
            request_headers: headers_of(request.get("headers")),
            response_headers: Vec::new(),
            request_body: post,
            response_body: None,
            response_base64: false,
            cookie_scopes: Vec::new(),
        },
    });
}

fn on_response(raws: &mut Vec<RawRequest>, params: &Value) {
    let Some(raw) = latest(raws, request_id(params).as_deref()) else {
        return;
    };
    if let Some(kind) = params.get("type").and_then(|value| value.as_str()) {
        raw.item.resource_type = resource_type(kind);
    }
    if let Some(response) = params.get("response") {
        apply_response(raw, response);
    }
}

fn apply_response(raw: &mut RawRequest, response: &Value) {
    if let Some(status) = response.get("status").and_then(|value| value.as_u64()) {
        let status = status as u16;
        let given = response
            .get("statusText")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        raw.item.status = Some(status);
        raw.item.status_text = status_text(status, given);
    }
    if let Some(mime) = response.get("mimeType").and_then(|value| value.as_str()) {
        if !mime.is_empty() {
            raw.item.mime = mime.to_string();
        }
    }
    let headers = headers_of(response.get("headers"));
    if !headers.is_empty() {
        raw.item.response_headers = headers;
        if raw.item.mime.is_empty() {
            if let Some((_, value)) = raw
                .item
                .response_headers
                .iter()
                .find(|(name, _)| name == "content-type")
            {
                raw.item.mime = value.clone();
            }
        }
    }
}

fn latest<'a>(raws: &'a mut [RawRequest], request_id: Option<&str>) -> Option<&'a mut RawRequest> {
    let request_id = request_id?;
    raws.iter_mut()
        .rev()
        .find(|raw| raw.request_id == request_id)
}

fn request_id(params: &Value) -> Option<String> {
    params
        .get("requestId")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn headers_of(value: Option<&Value>) -> Vec<(String, String)> {
    let Some(value) = value else {
        return Vec::new();
    };
    if let Some(object) = value.as_object() {
        return object
            .iter()
            .map(|(name, item)| (name.to_ascii_lowercase(), scalar(item)))
            .collect();
    }
    let Some(array) = value.as_array() else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(|value| value.as_str())?;
            if name.is_empty() {
                return None;
            }
            let text = item
                .get("value")
                .map(scalar)
                .unwrap_or_default();
            Some((name.to_ascii_lowercase(), text))
        })
        .collect()
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn resource_type(raw: &str) -> String {
    match raw {
        "Document" => "document",
        "Stylesheet" => "stylesheet",
        "Image" => "image",
        "Media" => "media",
        "Font" => "font",
        "Script" => "script",
        "TextTrack" => "texttrack",
        "XHR" => "xhr",
        "Fetch" => "fetch",
        "EventSource" => "eventsource",
        "WebSocket" => "websocket",
        "Manifest" => "manifest",
        "Ping" => "ping",
        "CSPViolationReport" => "cspreport",
        "Prefetch" | "SignedExchange" | "Preflight" | "FedCM" | "Other" | "" => "other",
        other => return other.to_ascii_lowercase(),
    }
    .to_string()
}

fn status_text(code: u16, given: &str) -> String {
    if !given.is_empty() {
        return given.to_string();
    }
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "",
    }
    .to_string()
}

pub fn render_console(entries: &[ConsoleEntry], min_level: &str) -> Result<String, String> {
    if !valid_level(min_level) {
        return Err(format!(
            "playwright-cli console: unknown min-level \"{min_level}\" (expected error, warning, info, or debug)\n"
        ));
    }
    if entries.is_empty() {
        return Err("No console messages\n".to_string());
    }
    let total = entries.len();
    let errors = entries.iter().filter(|entry| entry.kind == "error").count();
    let warnings = entries
        .iter()
        .filter(|entry| entry.kind == "warning")
        .count();
    let shown: Vec<&ConsoleEntry> = entries
        .iter()
        .filter(|entry| include_message(min_level, &entry.kind))
        .collect();
    let mut lines = vec![format!(
        "Total messages: {total} (Errors: {errors}, Warnings: {warnings})"
    )];
    if shown.len() != total {
        lines.push(format!(
            "Returning {} messages for level \"{min_level}\"",
            shown.len()
        ));
    }
    lines.push(String::new());
    for entry in shown {
        lines.push(format!(
            "[{}] {} @ {}:{}",
            entry.kind.to_ascii_uppercase(),
            sanitize::browser_message(&entry.text),
            sanitize::display_url(&entry.url),
            entry.line
        ));
    }
    Ok(format!("### Result\n{}", lines.join("\n")))
}

fn include_message(threshold: &str, kind: &str) -> bool {
    level_rank(&message_level(kind)) <= level_rank(threshold)
}

fn message_level(kind: &str) -> &'static str {
    match kind {
        "assert" | "error" => "error",
        "warning" => "warning",
        "clear" | "debug" | "endGroup" | "profile" | "profileEnd" | "startGroup"
        | "startGroupCollapsed" | "trace" => "debug",
        _ => "info",
    }
}

fn level_rank(level: &str) -> usize {
    match level {
        "error" => 0,
        "warning" => 1,
        "debug" => 3,
        _ => 2,
    }
}

pub fn render_requests(
    items: &[NetRequest],
    include_static: bool,
    filter: Option<&str>,
) -> Result<String, String> {
    if items.is_empty() {
        return Err("No network requests\n".to_string());
    }
    let mut lines = Vec::new();
    let mut hidden = 0usize;
    for (offset, request) in items.iter().enumerate() {
        if !include_static && !is_fetch(request) && is_successful(request) {
            hidden += 1;
            continue;
        }
        if let Some(filter) = filter.filter(|text| !text.is_empty()) {
            if !request.url.contains(filter) {
                continue;
            }
        }
        lines.push(format!(
            "{}. {}",
            offset + 1,
            render_request_line(request)
        ));
    }
    if hidden > 0 {
        let plural = if hidden == 1 { "" } else { "s" };
        let pronoun = if hidden == 1 { "it" } else { "them" };
        lines.push(format!(
            "\nNote: {hidden} static request{plural} not shown, run with --static option to see {pronoun}."
        ));
    }
    Ok(format!("### Result\n{}", lines.join("\n")))
}

fn render_request_line(request: &NetRequest) -> String {
    let mut line = format!(
        "[{}] {}",
        request.method.to_ascii_uppercase(),
        sanitize::display_url(&truncate_data_url(&request.url))
    );
    if let Some(status) = request.status {
        if request.status_text.is_empty() {
            line.push_str(&format!(" => [{status}]"));
        } else {
            line.push_str(&format!(" => [{status}] {}", sanitize::browser_message(&request.status_text)));
        }
    } else if let Some(failed) = &request.failed {
        line.push_str(&format!(" => [FAILED] {}", sanitize::browser_message(failed)));
    }
    line
}

pub fn render_request(items: &[NetRequest], index: usize) -> Result<String, String> {
    let request = lookup(items, index)?;
    Ok(format!(
        "### Result\n{}",
        render_details(index, request)
    ))
}

fn render_details(index: usize, request: &NetRequest) -> String {
    let mut lines = vec![
        format!(
            "#{index} [{}] {}",
            request.method.to_ascii_uppercase(),
            sanitize::display_url(&truncate_data_url(&request.url))
        ),
        String::new(),
        "  General".to_string(),
    ];
    if let Some(status) = request.status {
        let text = if request.status_text.is_empty() {
            format!("[{status}]")
        } else {
            format!("[{status}] {}", sanitize::browser_message(&request.status_text))
        };
        lines.push(general("status", &text));
    } else if let Some(failed) = &request.failed {
        lines.push(general("status", &format!("[FAILED] {}", sanitize::browser_message(failed))));
    }
    lines.push(general("type", &request.resource_type));
    if !request.mime.is_empty() {
        let mime = request.mime.split(';').next().unwrap_or("").trim();
        lines.push(general("mimeType", mime));
    }
    append_headers(&mut lines, "Request headers", &request.request_headers, Some(&request.url));
    append_headers(&mut lines, "Response headers", &request.response_headers, Some(&request.url));
    let mut hints = Vec::new();
    if request
        .request_body
        .as_ref()
        .is_some_and(|body| !body.is_empty())
    {
        hints.push(format!(
            "Run `request-body {index}` to read the request body."
        ));
    }
    if request.response_body.is_some() {
        hints.push(format!(
            "Run `response-body {index}` to read the response body."
        ));
    } else if expects_body(request) {
        hints.push(UNSAVED_BODY.to_string());
    }
    if !hints.is_empty() {
        lines.push(String::new());
        lines.extend(hints);
    }
    lines.join("\n")
}

fn general(label: &str, value: &str) -> String {
    let spaces = 10usize.saturating_sub(label.len());
    format!("    {label}:{}{value}", " ".repeat(spaces))
}

fn is_url_header(name: &str) -> bool {
    matches!(
        name,
        "location" | "referer" | "content-location" | "refresh" | "link"
    )
}

fn display_header_value(name: &str, value: &str, base_url: Option<&str>) -> String {
    if is_url_header(name) {
        if crate::sanitize::full_urls_enabled() {
            sanitize::terminal(value)
        } else {
            sanitize::terminal(&cdp_client::sanitize::redact_header_url(value, base_url))
        }
    } else {
        sanitize::terminal(value)
    }
}

fn append_headers(lines: &mut Vec<String>, title: &str, headers: &[(String, String)], base_url: Option<&str>) {
    if headers.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(format!("  {title}"));
    for (name, value) in headers {
        lines.push(format!("    {}: {}", sanitize::terminal(name), display_header_value(name, value, base_url)));
    }
}

const UNSAVED_BODY: &str = "The response body is not kept in the saved session.";

pub fn render_part(items: &[NetRequest], index: usize, part: &str) -> Result<Piece, String> {
    let request = lookup(items, index)?;
    match part {
        "request-headers" => Ok(text_piece(&render_header_block(&request.request_headers, Some(&request.url)))),
        "request-body" => Ok(request
            .request_body
            .as_ref()
            .map(|body| text_piece(body))
            .unwrap_or(Piece::Empty)),
        "response-headers" => Ok(text_piece(&render_header_block(&request.response_headers, Some(&request.url)))),
        "response-body" if request.response_body.is_none() && expects_body(request) => {
            Err(format!("playwright-cli response-body: request #{index}: {UNSAVED_BODY}\n"))
        }
        "response-body" => Ok(response_piece(request)),
        _ => Err(format!("playwright-cli: unknown request part \"{part}\"\n")),
    }
}

fn text_piece(text: &str) -> Piece {
    if text.is_empty() {
        Piece::Empty
    } else {
        Piece::Text(text.to_string())
    }
}

fn render_header_block(headers: &[(String, String)], base_url: Option<&str>) -> String {
    headers
        .iter()
        .map(|(name, value)| format!("{}: {}", sanitize::terminal(name), display_header_value(name, value, base_url)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn response_piece(request: &NetRequest) -> Piece {
    let Some(body) = request.response_body.as_deref() else {
        return Piece::Empty;
    };
    if body.is_empty() {
        return Piece::Empty;
    }
    let textual = is_textual_mime(&request.mime);
    if request.response_base64 {
        let bytes = match base64_decode(body) {
            Ok(bytes) => bytes,
            Err(_) => return Piece::Empty,
        };
        if textual {
            if let Ok(text) = String::from_utf8(bytes.clone()) {
                return Piece::Text(text);
            }
        }
        return Piece::File {
            bytes,
            extension: extension_for_mime(&request.mime),
        };
    }
    if textual {
        return Piece::Text(body.to_string());
    }
    Piece::File {
        bytes: body.as_bytes().to_vec(),
        extension: extension_for_mime(&request.mime),
    }
}

fn lookup(items: &[NetRequest], index: usize) -> Result<&NetRequest, String> {
    if index == 0 {
        return Err(missing_index(index));
    }
    items
        .get(index - 1)
        .ok_or_else(|| missing_index(index))
}

fn missing_index(index: usize) -> String {
    format!(
        "Request #{index} not found. Use playwright-cli requests to see available indexes.\n"
    )
}

fn is_fetch(request: &NetRequest) -> bool {
    matches!(request.resource_type.as_str(), "fetch" | "xhr")
}

fn is_successful(request: &NetRequest) -> bool {
    if request.failed.is_some() {
        return false;
    }
    matches!(request.status, Some(status) if status < 400)
}

fn expects_body(request: &NetRequest) -> bool {
    !request.method.eq_ignore_ascii_case("HEAD") && can_have_response_body(request)
}

fn can_have_response_body(request: &NetRequest) -> bool {
    match request.status {
        Some(status) => status != 204 && status != 304 && !(100..200).contains(&status),
        None => false,
    }
}

pub fn is_textual_mime(mime: &str) -> bool {
    let main = mime.split(';').next().unwrap_or("").trim();
    if main.starts_with("text/") {
        return true;
    }
    main == "application/json"
        || main.ends_with("+json")
        || main == "application/xml"
        || main == "text/xml"
        || main.ends_with("+xml")
        || main == "application/javascript"
        || main == "application/x-javascript"
        || main == "application/ecmascript"
        || main == "application/graphql"
        || main == "application/x-www-form-urlencoded"
        || main == "image/svg+xml"
}

pub fn extension_for_mime(mime: &str) -> String {
    let subtype = mime
        .split(';')
        .next()
        .unwrap_or("")
        .split('/')
        .nth(1)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if subtype.is_empty() {
        return "bin".to_string();
    }
    let tail = subtype.split('+').next_back().unwrap_or(&subtype);
    let name = match tail {
        "plain" => "txt".to_string(),
        "javascript" | "ecmascript" => "js".to_string(),
        "jpeg" => "jpg".to_string(),
        other => other
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect::<String>(),
    };
    if name.is_empty() {
        "bin".to_string()
    } else {
        name
    }
}

fn truncate_data_url(url: &str) -> String {
    let Some(rest) = url.strip_prefix("data:") else {
        return url.to_string();
    };
    let Some(index) = rest.find(',') else {
        return url.to_string();
    };
    format!("{}…", &url[.."data:".len() + index + 1])
}

pub fn printable_link(title: &str, path: &str) -> String {
    let display = if path.starts_with('/') || path.starts_with('.') || path.contains('/') || path.contains('\\')
    {
        path.to_string()
    } else {
        format!("./{path}")
    };
    format!("- [{title}]({display})")
}

pub fn console_json(entry: &ConsoleEntry) -> Value {
    let mut object = json!({
        "type": entry.kind,
        "text": entry.text,
        "url": entry.url,
        "line": entry.line,
    });
    if let Some(stamp) = stamp_json(&entry.stamp) {
        object["stamp"] = stamp;
    }
    object
}

fn stamp_json(stamp: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(stamp).ok()?;
    if value.is_number() {
        Some(value)
    } else {
        None
    }
}

pub fn console_from_json(value: &Value) -> Option<ConsoleEntry> {
    Some(ConsoleEntry {
        kind: value.get("type")?.as_str()?.to_string(),
        text: value.get("text").and_then(|item| item.as_str()).unwrap_or("").to_string(),
        url: value.get("url").and_then(|item| item.as_str()).unwrap_or("").to_string(),
        line: value.get("line").and_then(|item| item.as_u64()).unwrap_or(0),
        stamp: timestamp_text(value.get("stamp")),
        cookie_value: false,
    })
}

pub fn request_json(request: &NetRequest) -> Value {
    json!({
        "method": request.method,
        "url": request.url,
        "type": request.resource_type,
        "status": request.status,
        "statusText": request.status_text,
        "failed": request.failed,
        "mime": request.mime,
        "requestHeaders": pairs_json(&request.request_headers),
        "responseHeaders": pairs_json(&request.response_headers),
        "requestBody": request.request_body,
        "responseBody": request.response_body,
        "responseBase64": request.response_base64,
        "cookieScopes": crate::cookie_headers::saved_scopes(&request.cookie_scopes),
    })
}

fn pairs_json(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(name, value)| json!([name, value]))
            .collect(),
    )
}

pub fn request_from_json(value: &Value) -> Option<NetRequest> {
    Some(NetRequest {
        method: value.get("method")?.as_str()?.to_string(),
        url: value.get("url")?.as_str()?.to_string(),
        resource_type: value
            .get("type")
            .and_then(|item| item.as_str())
            .unwrap_or("other")
            .to_string(),
        status: value
            .get("status")
            .and_then(|item| item.as_u64())
            .map(|status| status as u16),
        status_text: value
            .get("statusText")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string(),
        failed: value
            .get("failed")
            .and_then(|item| item.as_str())
            .map(str::to_string),
        mime: value
            .get("mime")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string(),
        request_headers: pairs_from(value.get("requestHeaders")),
        response_headers: pairs_from(value.get("responseHeaders")),
        cookie_scopes: crate::cookie_headers::saved_scopes(value.get("cookieScopes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default()),
        request_body: value
            .get("requestBody")
            .and_then(|item| item.as_str())
            .map(str::to_string),
        response_body: value
            .get("responseBody")
            .and_then(|item| item.as_str())
            .map(str::to_string),
        response_base64: value
            .get("responseBase64")
            .and_then(|item| item.as_bool())
            .unwrap_or(false),
    })
}

fn pairs_from(value: Option<&Value>) -> Vec<(String, String)> {
    let Some(array) = value.and_then(|item| item.as_array()) else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|item| {
            let pair = item.as_array()?;
            Some((
                pair.first()?.as_str()?.to_string(),
                pair.get(1)?.as_str().unwrap_or("").to_string(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_stamp_comes_from_the_event_timestamp() {
        let events = vec![json!({
            "method": "Runtime.consoleAPICalled",
            "sessionId": "S",
            "params": {
                "type": "error",
                "executionContextId": 1,
                "timestamp": 1700000000000.5,
                "args": [{"type": "string", "value": "once"}]
            }
        })];
        let (console, _) = from_events(&events, "S");
        assert_eq!(console.len(), 1);
        assert_eq!(console[0].text, "once");
        assert_eq!(console[0].stamp, "1700000000000.5");
    }

    fn sample_events() -> Vec<Value> {
        vec![
            json!({
                "method": "Runtime.consoleAPICalled",
                "sessionId": "other",
                "params": {
                    "type": "log",
                    "executionContextId": 1,
                    "args": [{"type": "string", "value": "elsewhere"}]
                }
            }),
            json!({
                "method": "Runtime.consoleAPICalled",
                "sessionId": "S",
                "params": {
                    "type": "debug",
                    "executionContextId": 1,
                    "args": [{"type": "string", "value": "hidden-debug"}],
                    "stackTrace": {"callFrames": [{"url": "http://127.0.0.1/page", "lineNumber": 1}]}
                }
            }),
            json!({
                "method": "Runtime.consoleAPICalled",
                "sessionId": "S",
                "params": {
                    "type": "error",
                    "executionContextId": 1,
                    "args": [{"type": "string", "value": "slicc-console-marker"}],
                    "stackTrace": {"callFrames": [{"url": "http://user:pw@127.0.0.1/page?token=SECRET#frag", "lineNumber": 4}]}
                }
            }),
            json!({
                "method": "Runtime.consoleAPICalled",
                "sessionId": "S",
                "params": {"type": "log", "executionContextId": 0, "args": [{"type": "string", "value": "internal"}]}
            }),
            json!({
                "method": "Network.requestWillBeSent",
                "sessionId": "S",
                "params": {
                    "requestId": "doc",
                    "type": "Document",
                    "request": {
                        "url": "http://127.0.0.1/page?token=SECRET#frag",
                        "method": "GET",
                        "headers": {"Accept": "text/html"}
                    }
                }
            }),
            json!({
                "method": "Network.responseReceived",
                "sessionId": "S",
                "params": {
                    "requestId": "doc",
                    "type": "Document",
                    "response": {
                        "status": 200,
                        "statusText": "OK",
                        "mimeType": "text/html",
                        "headers": {"Content-Type": "text/html"}
                    }
                }
            }),
            json!({
                "method": "Network.loadingFinished",
                "sessionId": "S",
                "params": {"requestId": "doc"}
            }),
            json!({
                "method": "Network.requestWillBeSent",
                "sessionId": "S",
                "params": {
                    "requestId": "api",
                    "type": "Fetch",
                    "request": {
                        "url": "http://127.0.0.1/api/me?x=1",
                        "method": "POST",
                        "headers": {"Accept": "*/*"},
                        "postData": "{\"a\":1}"
                    }
                }
            }),
            json!({
                "method": "Network.requestWillBeSentExtraInfo",
                "sessionId": "S",
                "params": {
                    "requestId": "api",
                    "headers": [{"name": "Cookie", "value": "session=from-tab"}, {"name": "Accept", "value": "*/*"}]
                }
            }),
            json!({
                "method": "Network.responseReceived",
                "sessionId": "S",
                "params": {
                    "requestId": "api",
                    "type": "Fetch",
                    "response": {
                        "status": 200,
                        "statusText": "OK",
                        "mimeType": "text/plain",
                        "headers": {"Content-Type": "text/plain", "Set-Cookie": "session=from-tab"}
                    }
                }
            }),
            json!({
                "method": "Network.loadingFailed",
                "sessionId": "S",
                "params": {"requestId": "missing", "errorText": "net::ERR_FAILED"}
            }),
        ]
    }

    fn requests_of(events: &[Value]) -> Vec<NetRequest> {
        from_events(events, "S").1.into_iter().map(|raw| raw.item).collect()
    }

    #[test]
    fn console_keeps_the_query_and_filters_levels() {
        crate::sanitize::set_full_urls(false);
        let (console, _) = from_events(&sample_events(), "S");
        assert_eq!(console.len(), 2);
        let listed = render_console(&console, "info").unwrap();
        assert!(listed.starts_with("### Result\n"));
        assert!(listed.contains("Total messages: 2 (Errors: 1, Warnings: 0)"));
        assert!(listed.contains("Returning 1 messages for level \"info\""));
        assert!(listed.contains("[ERROR] slicc-console-marker"));
        assert!(!listed.contains("SECRET"));
        assert!(listed.contains("token="));
        assert!(!listed.contains("hidden-debug"));
        assert!(!listed.contains("elsewhere"));
        assert!(!listed.contains("internal"));
        let errors = render_console(&console, "error").unwrap();
        assert!(errors.contains("[ERROR] slicc-console-marker"));
        assert!(render_console(&[], "info").unwrap_err().contains("No console messages"));
        assert!(render_console(&console, "verbose").is_err());
    }

    #[test]
    fn requests_number_the_full_list_and_skip_static() {
        crate::sanitize::set_full_urls(false);
        let items = requests_of(&sample_events());
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].request_body.as_deref(), Some("{\"a\":1}"));
        assert_eq!(
            items[1].request_headers,
            vec![
                ("cookie".to_string(), "session=from-tab".to_string()),
                ("accept".to_string(), "*/*".to_string()),
            ]
        );
        let listed = render_requests(&items, false, None).unwrap();
        assert!(listed.contains("[POST]"));
        assert!(listed.contains("/api/me"));
        assert!(!listed.contains("token=SECRET"));
        assert!(!listed.contains("/page?"));
        assert!(listed.contains("1 static request not shown"));
        let filtered = render_requests(&items, true, Some("/api/me")).unwrap();
        assert!(filtered.contains("2. [POST]"));
        assert!(!filtered.contains("/page?"));
        let all = render_requests(&items, true, None).unwrap();
        assert!(all.contains("1. [GET]"));
        assert!(all.contains("/page?"));
        assert!(!all.contains("token=SECRET"));
        assert!(render_requests(&[], false, None)
            .unwrap_err()
            .contains("No network requests"));
    }

    #[test]
    fn request_parts_match_the_index_and_a_miss_is_an_error() {
        crate::sanitize::set_full_urls(false);
        let items = requests_of(&sample_events());
        let details = render_request(&items, 2).unwrap();
        assert!(details.contains("#2 [POST]"));
        assert!(details.contains("/api/me"));
        assert!(details.contains("    status:    [200] OK"));
        assert!(details.contains("    type:      fetch"));
        assert!(details.contains("    cookie: session=from-tab"));
        assert!(details.contains("    set-cookie: session=from-tab"));
        assert!(details.contains("Run `request-body 2`"));
        assert!(!details.contains("Run `response-body 2`"));
        assert!(details.contains("The response body is not kept in the saved session."));
        match render_part(&items, 2, "request-headers").unwrap() {
            Piece::Text(text) => {
                assert!(text.contains("cookie: session=from-tab"));
                assert!(!text.contains("###"));
            }
            other => panic!("{other:?}"),
        }
        match render_part(&items, 2, "request-body").unwrap() {
            Piece::Text(text) => assert_eq!(text, "{\"a\":1}"),
            other => panic!("{other:?}"),
        }
        let unsaved = render_part(&items, 2, "response-body").unwrap_err();
        assert_eq!(
            unsaved,
            "playwright-cli response-body: request #2: The response body is not kept in the saved session.\n"
        );
        let mut kept = items.clone();
        kept[1].response_body = Some(String::new());
        assert!(render_request(&kept, 2).unwrap().contains("Run `response-body 2`"));
        assert!(matches!(render_part(&kept, 2, "response-body").unwrap(), Piece::Empty));
        kept[1].response_body = None;
        kept[1].method = "head".to_string();
        let head = render_request(&kept, 2).unwrap();
        assert!(!head.contains("response-body") && !head.contains("not kept"));
        assert!(matches!(render_part(&kept, 2, "response-body").unwrap(), Piece::Empty));
        kept[1].method = "GET".to_string();
        kept[1].status = Some(204);
        let empty = render_request(&kept, 2).unwrap();
        assert!(!empty.contains("response-body") && !empty.contains("not kept"));
        assert!(matches!(render_part(&kept, 2, "response-body").unwrap(), Piece::Empty));
        let err = render_request(&items, 9).unwrap_err();
        assert!(err.contains("Request #9 not found"));
        assert!(precheck("request", &[]).unwrap().contains("index is required"));
        assert!(precheck("response-body", &["0".into()])
            .unwrap()
            .contains("positive integer"));
        assert!(precheck("console", &["verbose".into()])
            .unwrap()
            .contains("min-level"));
        assert!(precheck("console", &[]).is_none());
    }

    #[test]
    fn a_binary_body_is_a_file_and_text_stays_text() {
        let mut request = requests_of(&sample_events())[1].clone();
        request.response_body = Some("session=from-tab".to_string());
        request.response_base64 = false;
        match response_piece(&request) {
            Piece::Text(text) => assert_eq!(text, "session=from-tab"),
            other => panic!("{other:?}"),
        }
        request.mime = "image/png".to_string();
        request.response_base64 = true;
        request.response_body = Some(crate::net::base64_encode(&[0x89, 0x50, 0x4e, 0x47]));
        match response_piece(&request) {
            Piece::File { bytes, extension } => {
                assert_eq!(bytes, vec![0x89, 0x50, 0x4e, 0x47]);
                assert_eq!(extension, "png");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(printable_link("Response body", "body.txt"), "- [Response body](./body.txt)");
        assert_eq!(truncate_data_url("data:text/plain,hello"), "data:text/plain,…");
    }

    #[test]
    fn a_failed_script_is_not_hidden() {
        let items = vec![NetRequest {
            method: "GET".to_string(),
            url: "http://127.0.0.1/app.js".to_string(),
            resource_type: "script".to_string(),
            status: None,
            status_text: String::new(),
            failed: Some("net::ERR_FAILED".to_string()),
            mime: String::new(),
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            cookie_scopes: Vec::new(),
            request_body: None,
            response_body: None,
            response_base64: false,
        }];
        let listed = render_requests(&items, false, None).unwrap();
        assert!(listed.contains("[FAILED] net::ERR_FAILED"));
        assert!(!listed.contains("static request"));
    }

    #[test]
    fn console_sanitizes_terminal_escapes() {
        let entries = vec![ConsoleEntry {
            kind: "log".to_string(),
            text: "\x1b[2Jevil message".to_string(),
            url: "http://example.com/\x07bell".to_string(),
            line: 1,
            stamp: String::new(),
            cookie_value: false,
        }];
        let text = render_console(&entries, "info").unwrap();
        assert!(!text.contains('\x1b'), "{text}");
        assert!(!text.contains('\x07'), "{text}");
        assert!(text.contains("\\x1b"), "{text}");
    }

    #[test]
    fn request_line_sanitizes_url_and_status_text() {
        let items = vec![NetRequest {
            method: "GET".to_string(),
            url: "http://evil.com/\x1b[2J".to_string(),
            resource_type: "fetch".to_string(),
            status: Some(200),
            status_text: "OK\x1b[Hpwned".to_string(),
            failed: None,
            mime: String::new(),
            request_headers: Vec::new(),
            response_headers: Vec::new(),
            cookie_scopes: Vec::new(),
            request_body: None,
            response_body: None,
            response_base64: false,
        }];
        let text = render_requests(&items, true, None).unwrap();
        assert!(!text.contains('\x1b'), "{text}");
        assert!(text.contains("\\x1b"), "{text}");
    }

    #[test]
    fn request_details_sanitizes_headers() {
        let items = vec![NetRequest {
            method: "GET".to_string(),
            url: "http://example.com/".to_string(),
            resource_type: "fetch".to_string(),
            status: Some(200),
            status_text: String::new(),
            failed: None,
            mime: String::new(),
            request_headers: vec![("X-Evil".to_string(), "\x1b[2Jvalue".to_string())],
            response_headers: Vec::new(),
            cookie_scopes: Vec::new(),
            request_body: None,
            response_body: None,
            response_base64: false,
        }];
        let text = render_request(&items, 1).unwrap();
        assert!(!text.contains('\x1b'), "{text}");
        assert!(text.contains("\\x1b"), "{text}");
    }
}
