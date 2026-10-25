use crate::body::FormPart;
use crate::parse::Stop;
use crate::request::{PreparedRequest, RequestBody};
use cdp_client::cdp::{self, Cdp};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const RESPONSE_HANDLING: &str = "const h = {};r.headers.forEach((v, k) => { h[k] = v; });const ct = r.headers.get('content-type') || '';const __rt = \"binary\";const __ctl = ct.toLowerCase();const __binPrefixes = ['image/','audio/','video/','application/octet-stream','application/pdf','application/protobuf','application/x-protobuf','application/wasm','application/zip'];const __isXml = __ctl.indexOf('+xml') !== -1 || __ctl.indexOf('application/xml') === 0 || __ctl.indexOf('text/xml') === 0;const __isBinary = __rt === 'binary' || (__rt !== 'text' && __rt !== 'json' && !__isXml && __binPrefixes.some((p) => __ctl.indexOf(p) === 0));const __meta = { ok: r.ok, status: r.status, statusText: r.statusText, url: r.url, redirected: !!r.redirected, headers: h };if (__isBinary) {const __u = new Uint8Array(await r.arrayBuffer());let __s = ''; const __cs = 0x8000;for (let __i = 0; __i < __u.length; __i += __cs) { __s += String.fromCharCode.apply(null, __u.subarray(__i, __i + __cs)); }return Object.assign(__meta, { body: btoa(__s), bodyEncoding: 'base64' });}const t = await r.text();let b;const __jsonWanted = __rt === 'json' || (__rt !== 'text' && ct.indexOf('application/json') !== -1);if (__jsonWanted) { if (!t) { b = null; } else { try { b = JSON.parse(t); } catch (e) { b = t; } } }else { b = t; }return Object.assign(__meta, { body: b });";

pub struct FetchResult {
    pub status: u16,
    pub status_text: String,
    pub url: String,
    pub redirected: bool,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub fn fetch_script(request: &PreparedRequest) -> String {
    let mut init = serde_json::Map::new();
    init.insert("redirect".into(), json!("manual"));
    init.insert("method".into(), json!(request.method));
    init.insert("credentials".into(), json!(request.credentials));
    init.insert("headers".into(), Value::Object(request.headers.clone()));
    if let Some(referrer) = &request.referrer {
        init.insert("referrer".into(), json!(referrer));
    }
    let init_json = serde_json::to_string(&Value::Object(init)).unwrap_or_else(|_| "{}".into());
    let url_json = serde_json::to_string(&request.url).unwrap_or_else(|_| "\"\"".into());
    let timeout = match request.timeout_ms {
        Some(ms) if ms > 0 => format!("__init.signal = AbortSignal.timeout({ms});"),
        _ => String::new(),
    };
    let mut script = String::new();
    script.push_str("(async () => {const __init = ");
    script.push_str(&init_json);
    script.push(';');
    script.push_str(&timeout);
    script.push_str(&reconstruct(&request.body));
    script.push_str("const __url = new URL(");
    script.push_str(&url_json);
    script.push_str(", location.href);__url.hash = '';const r = await fetch(__url.href, __init);if (r.type === 'opaqueredirect') return { manualRedirect: true, requestUrl: __url.href };");
    script.push_str(RESPONSE_HANDLING);
    script.push_str("})()");
    script
}

fn reconstruct(body: &RequestBody) -> String {
    let descriptor = match body {
        RequestBody::None => return String::new(),
        RequestBody::Bytes(bytes) => json!({
            "kind": "bytes",
            "data": cdp_client::net::base64_encode(bytes),
        }),
        RequestBody::Form(parts) => {
            let entries: Vec<Value> = parts.iter().map(form_entry).collect();
            json!({ "kind": "formdata", "entries": entries })
        }
    };
    let body_json = serde_json::to_string(&descriptor).unwrap_or_else(|_| "{}".into());
    format!(
        "const __b64 = (s) => {{ const bin = atob(s); const n = bin.length; const u = new Uint8Array(n); for (let i = 0; i < n; i++) u[i] = bin.charCodeAt(i); return u; }};const __body = {body_json};if (__body.kind === 'bytes') {{ __init.body = __b64(__body.data); }}else if (__body.kind === 'blob') {{ __init.body = new Blob([__b64(__body.data)], {{ type: __body.type }}); }}else if (__body.kind === 'formdata') {{ const __fd = new FormData(); for (const e of __body.entries) {{ if (e.file) {{ __fd.append(e.name, new Blob([__b64(e.file.data)], {{ type: e.file.type }}), e.file.filename); }} else {{ __fd.append(e.name, e.value); }} }} __init.body = __fd; }}"
    )
}

fn form_entry(part: &FormPart) -> Value {
    match part {
        FormPart::Text { name, value } => {
            let mut entry = serde_json::Map::new();
            entry.insert("name".into(), json!(name));
            entry.insert("value".into(), json!(value));
            Value::Object(entry)
        }
        FormPart::File { name, filename, mime, bytes } => {
            let mut file = serde_json::Map::new();
            file.insert("data".into(), json!(cdp_client::net::base64_encode(bytes)));
            file.insert("filename".into(), json!(filename));
            file.insert("type".into(), json!(mime));
            let mut entry = serde_json::Map::new();
            entry.insert("name".into(), json!(name));
            entry.insert("file".into(), Value::Object(file));
            Value::Object(entry)
        }
    }
}

pub fn frame_context(cdp: &mut Cdp, session: &str, frame_id: &str) -> Result<i64, Stop> {
    cdp.call("Runtime.enable", json!({}), Some(session)).map_err(|err| {
        Stop::new(format!("curlwright: {}", err.trim_end_matches('\n')), 1)
    })?;
    for event in cdp.events() {
        if event.get("sessionId").and_then(|value| value.as_str()) != Some(session) {
            continue;
        }
        if event.get("method").and_then(|value| value.as_str()) != Some("Runtime.executionContextCreated") {
            continue;
        }
        let Some(aux) = event.pointer("/params/context/auxData") else {
            continue;
        };
        if aux.get("frameId").and_then(|value| value.as_str()) != Some(frame_id) {
            continue;
        }
        if aux.get("isDefault").and_then(|value| value.as_bool()) != Some(true) {
            continue;
        }
        if let Some(id) = event.pointer("/params/context/id").and_then(|value| value.as_i64()) {
            return Ok(id);
        }
    }
    Err(Stop::usage(format!("curlwright: no frame {frame_id} in that tab")))
}

pub fn evaluate(
    cdp: &mut Cdp,
    session: &str,
    expression: &str,
    context_id: Option<i64>,
    timeout: Duration,
    full_urls: bool,
) -> Result<Value, String> {
    let mut params = serde_json::Map::new();
    params.insert("expression".into(), json!(expression));
    params.insert("returnByValue".into(), json!(true));
    params.insert("awaitPromise".into(), json!(true));
    if let Some(id) = context_id {
        params.insert("contextId".into(), json!(id));
    }
    let result = cdp.call_for("Runtime.evaluate", Value::Object(params), Some(session), timeout)?;
    if let Some(details) = result.get("exceptionDetails") {
        let description = details
            .pointer("/exception/description")
            .and_then(|value| value.as_str())
            .or_else(|| details.get("text").and_then(|value| value.as_str()))
            .unwrap_or("evaluation failed");
        return Err(format!("{}\n", crate::sanitize::browser_message(description, full_urls)));
    }
    Ok(result.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

pub fn parse_fetch_result(value: &Value) -> Result<FetchResult, String> {
    let status = value.get("status").and_then(|item| item.as_u64()).unwrap_or(0) as u16;
    let status_text = value.get("statusText").and_then(|item| item.as_str()).unwrap_or("").to_string();
    let url = value.get("url").and_then(|item| item.as_str()).unwrap_or("").to_string();
    let redirected = value.get("redirected").and_then(|item| item.as_bool()).unwrap_or(false);
    let mut headers = Vec::new();
    if let Some(map) = value.get("headers").and_then(|item| item.as_object()) {
        for (key, item) in map {
            let text = match item {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            headers.push((key.to_ascii_lowercase(), text));
        }
    }
    let encoding = value.get("bodyEncoding").and_then(|item| item.as_str());
    let body = match value.get("body") {
        Some(Value::String(text)) if encoding == Some("base64") => cdp_client::net::base64_decode(text)?,
        Some(Value::String(text)) => text.as_bytes().to_vec(),
        Some(Value::Null) | None => Vec::new(),
        Some(other) => other.to_string().into_bytes(),
    };
    Ok(FetchResult { status, status_text, url, redirected, headers, body })
}

pub fn run_fetch(
    cdp: &mut Cdp,
    session: &str,
    request: &PreparedRequest,
    context_id: Option<i64>,
    full_urls: bool,
    signer: Option<&crate::sign::Signer>,
) -> Result<FetchResult, String> {
    cdp.call("Network.enable", json!({}), Some(session))?;
    let timeout = match request.timeout_ms {
        Some(ms) if ms > 0 => Duration::from_millis(ms.saturating_add(1_000)),
        _ => Duration::from_secs(30),
    };
    let deadline = Instant::now() + timeout;
    let mut request = request.clone();
    let mut hop = 0;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() { return Err("Runtime.evaluate timed out\n".into()) }
        let from = cdp.events().len();
        let value = evaluate(cdp, session, &fetch_script(&request), context_id, left, full_urls)?;
        if value["manualRedirect"] != true {
            let mut result = parse_fetch_result(&value)?;
            result.redirected = hop > 0;
            return Ok(result);
        }
        if hop == 20 { return Err("curlwright: too many redirects\n".into()) }
        let source = value["requestUrl"].as_str().ok_or_else(|| cdp.guard().refusal().to_string())?;
        let (status, location) = redirect_metadata(&cdp.events()[from..], session, source, &request.method)
            .ok_or_else(|| cdp.guard().refusal().to_string())?;
        let next = cdp.ordinary_redirect_target(source, &location)?;
        rewrite_redirect(&mut request, source, &next, status);
        request.url = next;
        if let Some(signer) = signer {
            signer.apply(&mut request)?;
        }
        hop += 1;
    }
}

fn redirect_metadata(events: &[Value], session: &str, source: &str, method: &str) -> Option<(u16, String)> {
    let requests: Vec<&Value> = events.iter().filter(|event| {
        event["sessionId"] == session && event["method"] == "Network.requestWillBeSent"
            && event["params"].get("redirectResponse").is_none()
            && event["params"]["request"]["url"] == source
            && event["params"]["request"]["method"].as_str().is_some_and(|actual| actual.eq_ignore_ascii_case(method))
    }).collect();
    if requests.len() != 1 { return None }
    let id = requests[0]["params"]["requestId"].as_str()?;
    let responses: Vec<&Value> = events.iter().filter_map(|event| {
        if event["sessionId"] != session || event["params"]["requestId"] != id { return None }
        let response = match event["method"].as_str() {
            Some("Network.responseReceived") => &event["params"]["response"],
            Some("Network.requestWillBeSent") => &event["params"]["redirectResponse"],
            _ => return None,
        };
        (response["url"] == source).then_some(response)
    }).collect();
    if responses.len() != 1 { return None }
    let response = responses[0];
    let status = response["status"].as_u64()? as u16;
    if !matches!(status, 301 | 302 | 303 | 307 | 308) { return None }
    let locations: Vec<&str> = response["headers"].as_object()?.iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("location"))
        .filter_map(|(_, value)| value.as_str()).collect();
    if locations.len() != 1 || locations[0].is_empty() { return None }
    Some((status, locations[0].to_string()))
}

fn rewrite_redirect(request: &mut PreparedRequest, source: &str, next: &str, status: u16) {
    let post = request.method.eq_ignore_ascii_case("POST");
    let get_or_head = request.method.eq_ignore_ascii_case("GET") || request.method.eq_ignore_ascii_case("HEAD");
    if matches!(status, 301 | 302) && post || status == 303 && !get_or_head {
        request.method = "GET".into();
        request.body = RequestBody::None;
        request.headers.retain(|name, _| !matches!(name.to_ascii_lowercase().as_str(), "content-type" | "content-length" | "content-encoding" | "content-language" | "content-location"));
    }
    if url::Url::parse(source).ok().map(|url| url.origin()) != url::Url::parse(next).ok().map(|url| url.origin()) {
        request.headers.retain(|name, _| !matches!(name.to_ascii_lowercase().as_str(), "authorization" | "proxy-authorization" | "cookie"));
    }
}

pub fn attach(cdp: &mut Cdp, target_id: &str) -> Result<String, String> {
    cdp::attach(cdp, target_id)
}

pub fn detach(cdp: &mut Cdp, session: &str) {
    cdp::detach(cdp, session);
}
