use crate::body::{FormPart, ResolvedBody};
use crate::parse::{Options, Stop};
use serde_json::{Map, Value};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct PreparedRequest {
    pub url: String,
    pub method: String,
    pub headers: Map<String, Value>,
    pub credentials: String,
    pub referrer: Option<String>,
    pub timeout_ms: Option<u64>,
    pub body: RequestBody,
    pub upload_size: usize,
}

#[derive(Clone, Debug)]
pub enum RequestBody {
    None,
    Bytes(Vec<u8>),
    Form(Vec<FormPart>),
}

fn has_header(headers: &Map<String, Value>, name: &str) -> bool {
    headers.keys().any(|key| key.eq_ignore_ascii_case(name))
}

fn collect_headers(opts: &Options) -> (Map<String, Value>, HashSet<String>) {
    let mut headers = Map::new();
    let mut suppressed = HashSet::new();
    for header in &opts.headers {
        if header.name.is_empty() {
            continue;
        }
        if header.unset {
            suppressed.insert(header.name.to_ascii_lowercase());
            continue;
        }
        headers.insert(header.name.clone(), Value::String(header.value.clone()));
    }
    (headers, suppressed)
}

fn wants_default(headers: &Map<String, Value>, suppressed: &HashSet<String>, name: &str) -> bool {
    !has_header(headers, name) && !suppressed.contains(&name.to_ascii_lowercase())
}

fn range_header_value(range: &str) -> String {
    let unit = range.split_once('=').map(|(prefix, _)| prefix).unwrap_or("");
    if !unit.is_empty() && unit.chars().all(|ch| ch.is_ascii_alphabetic()) {
        range.to_string()
    } else {
        format!("bytes={range}")
    }
}

fn basic_auth(user: &str) -> String {
    format!("Basic {}", cdp_client::net::base64_encode(user.as_bytes()))
}

fn append_query(url: &str, query: &str) -> String {
    if query.is_empty() {
        return url.to_string();
    }
    let (base, fragment) = match url.find('#') {
        Some(index) => (&url[..index], &url[index..]),
        None => (url, ""),
    };
    let joiner = if base.contains('?') { '&' } else { '?' };
    format!("{base}{joiner}{query}{fragment}")
}

fn resolve_method(opts: &Options, has_body: bool) -> String {
    if let Some(method) = &opts.request_method {
        return method.clone();
    }
    if opts.head {
        return "HEAD".to_string();
    }
    if has_body { "POST".to_string() } else { "GET".to_string() }
}

fn bodiless(method: &str, has_body: bool, opts: &Options) -> Option<Stop> {
    let upper = method.to_ascii_uppercase();
    if !has_body || (upper != "GET" && upper != "HEAD") {
        return None;
    }
    let cause = match &opts.request_method {
        Some(method) => format!("-X {method}"),
        None => "-I".to_string(),
    };
    Some(Stop::usage(format!(
        "curlwright: {cause} cannot carry a request body — a page fetch rejects a {upper} with one. Drop the data, or use -G to send it as a query string."
    )))
}

fn timeout_ms(seconds: Option<f64>) -> Option<u64> {
    let seconds = seconds?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    let millis = (seconds * 1000.0).round();
    if millis <= 0.0 || millis > u64::MAX as f64 {
        return None;
    }
    Some(millis as u64)
}

pub fn prepare(opts: &Options, body: ResolvedBody) -> Result<PreparedRequest, Stop> {
    let Some(url) = opts.url.clone() else {
        return Err(Stop::usage("curlwright: no URL specified"));
    };
    let (mut headers, suppressed) = collect_headers(opts);
    let using_query = opts.get && body.bytes.is_some();
    let query = if using_query {
        body.bytes.as_ref().map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    } else {
        None
    };
    let effective_bytes = if using_query { None } else { body.bytes };
    let effective_type = if using_query { None } else { body.content_type };
    if opts.user.is_some() && wants_default(&headers, &suppressed, "authorization") {
        headers.insert("Authorization".to_string(), Value::String(basic_auth(opts.user.as_deref().unwrap_or(""))));
    }
    if let Some(range) = &opts.range {
        if wants_default(&headers, &suppressed, "range") {
            headers.insert("Range".to_string(), Value::String(range_header_value(range)));
        }
    }
    if let Some(content_type) = &effective_type {
        if wants_default(&headers, &suppressed, "content-type") {
            headers.insert("Content-Type".to_string(), Value::String(content_type.clone()));
        }
    }
    if let Some(accept) = &body.accept {
        if wants_default(&headers, &suppressed, "accept") {
            headers.insert("Accept".to_string(), Value::String(accept.clone()));
        }
    }
    let url = match query {
        Some(query) => append_query(&url, &query),
        None => url,
    };
    let request_body = if let Some(form) = body.form {
        RequestBody::Form(form)
    } else if let Some(bytes) = effective_bytes {
        RequestBody::Bytes(bytes)
    } else {
        RequestBody::None
    };
    let has_body = !matches!(request_body, RequestBody::None);
    let method = resolve_method(opts, has_body);
    if let Some(stop) = bodiless(&method, has_body, opts) {
        return Err(stop);
    }
    let upload_size = match &request_body {
        RequestBody::Bytes(bytes) => bytes.len(),
        _ => 0,
    };
    Ok(PreparedRequest {
        url,
        method,
        headers,
        credentials: opts.credentials.clone(),
        referrer: opts.referer.clone(),
        timeout_ms: timeout_ms(opts.max_time_seconds),
        body: request_body,
        upload_size,
    })
}

pub fn format_request_trace(request: &PreparedRequest) -> String {
    let mut lines = vec![format!("> {} {}", request.method, request.url)];
    for (name, value) in &request.headers {
        lines.push(format!("> {name}: {}", value.as_str().unwrap_or("")));
    }
    lines.push("> ".to_string());
    format!("{}\n", lines.join("\n"))
}

pub fn format_response_trace(status_line: &str, headers: &[(String, String)]) -> String {
    let mut lines = vec![format!("< {status_line}")];
    for (name, value) in headers {
        lines.push(format!("< {name}: {value}"));
    }
    lines.push("< ".to_string());
    format!("{}\n", lines.join("\n"))
}
