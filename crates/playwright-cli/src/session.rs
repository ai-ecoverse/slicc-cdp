use crate::route::{self, Route};
use crate::snapshot::RefRec;
use crate::trace::{self, ConsoleEntry, NetRequest};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
#[cfg(unix)]
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct Session {
    pub current: Option<String>,
    pub endpoint: Option<String>,
    pub provenance: Option<String>,
    pub session_name: Option<String>,
    pub owned: Vec<String>,
    pub created_blanks: BTreeMap<String, Value>,
    pub snapshots: BTreeMap<String, BTreeMap<String, RefRec>>,
    pub console: BTreeMap<String, Vec<ConsoleEntry>>,
    pub requests: BTreeMap<String, Vec<NetRequest>>,
    pub routes: Vec<Route>,
    pub offline: bool,
    pub webmcp: BTreeMap<String, Value>,
    pub webmcp_connection: String,
    pub cwd: Option<PathBuf>,
    pub input: BTreeMap<String, Value>,
}

#[allow(dead_code)]
pub fn storage_path(cwd: &Path) -> Option<PathBuf> {
    storage_path_named(cwd, None)
}

pub fn storage_path_named(cwd: &Path, session_name: Option<&str>) -> Option<PathBuf> {
    let session_name = session_name.filter(|name| !name.is_empty());
    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    session_path(&config_root()?, &canonical, session_name)
}

pub fn session_path(config_dir: &str, cwd: &Path, session_name: Option<&str>) -> Option<PathBuf> {
    let root = Path::new(config_dir);
    if !root.is_absolute() {
        return None;
    }
    let key = match session_name.filter(|name| !name.is_empty()) {
        Some(name) => {
            let mut input = cwd.as_os_str().as_encoded_bytes().to_vec();
            input.push(0);
            input.extend_from_slice(name.as_bytes());
            cdp_client::hash::sha256_hex(&input)
        }
        None => cdp_client::hash::sha256_hex(cwd.as_os_str().as_encoded_bytes()),
    };
    Some(
        root.join("slicc")
            .join("playwright-cli")
            .join("sessions")
            .join(format!("{key}.json")),
    )
}

#[cfg(target_arch = "wasm32")]
fn config_root() -> Option<String> {
    cdp_client::state::wasm_config_root(|name| std::env::var(name).ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn config_root() -> Option<String> {
    #[cfg(test)]
    {
        Some(
            std::env::temp_dir()
                .join("slicc-cdp-session-tests")
                .to_string_lossy()
                .into_owned(),
        )
    }
    #[cfg(all(not(test), not(target_arch = "wasm32")))]
    {
        Some(crate::launch::user_config_dir())
    }
}

pub(crate) fn path_named(session_name: Option<&str>, cwd: Option<&Path>) -> Option<PathBuf> {
    if let Some(cwd) = cwd {
        return storage_path_named(cwd, session_name);
    }
    #[cfg(target_arch = "wasm32")]
    let cwd = cdp_client::state::wasm_cwd(|name| std::env::var(name).ok())?;
    #[cfg(not(target_arch = "wasm32"))]
    let cwd = std::env::current_dir().ok()?;
    storage_path_named(&cwd, session_name)
}

pub fn load_in(session_name: Option<&str>, cwd: Option<&Path>) -> Session {
    let session_name = session_name.filter(|name| !name.is_empty());
    let mut sess = path_named(session_name, cwd)
        .as_deref()
        .map(load_at)
        .unwrap_or_default();
    sess.cwd = cwd.map(Path::to_path_buf);
    if let Some(name) = session_name {
        if sess.session_name.as_deref() != Some(name) {
            sess.current = None;
        }
        sess.session_name = Some(name.to_string());
    } else if sess.session_name.is_some() {
        sess.current = None;
        sess.session_name = None;
    }
    sess
}

pub fn load_at(path: &Path) -> Session {
    let Ok(text) = fs::read_to_string(path) else {
        return Session::default();
    };
    let _ = private_file(path);
    decode(&text)
}

fn decode(text: &str) -> Session {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Session::default();
    };
    let current = value
        .get("current")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let endpoint = value
        .get("endpoint")
        .and_then(|v| v.as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let provenance = value
        .get("provenance")
        .and_then(|v| v.as_str())
        .filter(|text| known_provenance(text))
        .map(str::to_string);
    let session_name = value
        .get("sessionName")
        .and_then(|v| v.as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let mut owned = Vec::new();
    if let Some(items) = value.get("owned").and_then(|item| item.as_array()) {
        for item in items {
            if let Some(id) = item.as_str().filter(|id| !id.is_empty()) {
                if !owned.iter().any(|have: &String| have == id) {
                    owned.push(id.to_string());
                }
            }
        }
    }
    let mut snapshots = BTreeMap::new();
    if let Some(tabs) = value.get("snapshots").and_then(|v| v.as_object()) {
        for (target, refs) in tabs {
            let mut map = BTreeMap::new();
            if let Some(object) = refs.as_object() {
                for (id, rec) in object {
                    map.insert(
                        id.clone(),
                        RefRec {
                            backend_node_id: rec.get("backendNodeId").and_then(|v| v.as_u64()),
                            selector: rec
                                .get("selector")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            frame_id: rec
                                .get("frameId")
                                .and_then(|v| v.as_str())
                                .map(str::to_string),
                            label: rec
                                .get("label")
                                .and_then(|v| v.as_str())
                                .filter(|label| !label.is_empty())
                                .map(str::to_string),
                            target: rec.get("targetId").and_then(|v| v.as_str()).map(str::to_string),
                            owner: rec.get("ownerNodeId").and_then(|v| v.as_u64()),
                            page: None,
                        },
                    );
                }
            }
            snapshots.insert(target.clone(), map);
        }
    }
    Session {
        current,
        endpoint,
        provenance,
        session_name,
        owned,
        created_blanks: value.get("createdBlanks").and_then(Value::as_object).map(|items| items.iter().map(|(target, proof)| (target.clone(), proof.clone())).collect()).unwrap_or_default(),
        snapshots,
        console: decode_console(&value),
        requests: decode_requests(&value),
        routes: value
            .get("routes")
            .and_then(|item| item.as_array())
            .map(|items| items.iter().filter_map(route::decode).collect())
            .unwrap_or_default(),
        offline: value.get("networkOffline").and_then(Value::as_bool).unwrap_or(false),
        webmcp: value.get("webmcp").and_then(Value::as_object).map(|entries| entries.iter().map(|(target, value)| (target.clone(), value.clone())).collect()).unwrap_or_default(),
        webmcp_connection: String::new(),
        cwd: None,
        input: value.get("input").and_then(Value::as_object).map(|entries| entries.iter().filter(|(_, value)| value.is_object()).map(|(target, value)| (target.clone(), value.clone())).collect()).unwrap_or_default(),
    }
}

fn decode_console(value: &Value) -> BTreeMap<String, Vec<ConsoleEntry>> {
    let mut map = BTreeMap::new();
    let Some(tabs) = value.get("console").and_then(|item| item.as_object()) else {
        return map;
    };
    for (target, entries) in tabs {
        let list = entries
            .as_array()
            .map(|items| items.iter().filter_map(trace::console_from_json).collect())
            .unwrap_or_default();
        map.insert(target.clone(), list);
    }
    map
}

fn decode_requests(value: &Value) -> BTreeMap<String, Vec<NetRequest>> {
    let mut map = BTreeMap::new();
    let Some(tabs) = value.get("requests").and_then(|item| item.as_object()) else {
        return map;
    };
    for (target, entries) in tabs {
        let list = entries
            .as_array()
            .map(|items| items.iter().filter_map(trace::request_from_json).collect())
            .unwrap_or_default();
        map.insert(target.clone(), list);
    }
    map
}

fn encode(session: &Session) -> Value {
    let mut snapshots = serde_json::Map::new();
    for (target, refs) in &session.snapshots {
        let mut object = serde_json::Map::new();
        for (id, rec) in refs {
            object.insert(
                id.clone(),
                json!({
                    "backendNodeId": rec.backend_node_id,
                    "selector": rec.selector,
                    "frameId": rec.frame_id,
                    "label": rec.label,
                    "targetId": rec.target,
                    "ownerNodeId": rec.owner,
                }),
            );
        }
        snapshots.insert(target.clone(), Value::Object(object));
    }
    let mut value = json!({
        "current": session.current,
        "endpoint": session.endpoint,
        "provenance": session.provenance,
        "sessionName": session.session_name,
        "owned": session.owned,
        "createdBlanks": session.created_blanks,
        "snapshots": snapshots,
        "console": encode_lists(&session.console, stored_console),
        "requests": encode_lists(&session.requests, |request| Some(stored_request(request))),
        "routes": session.routes.iter().map(route::encode).collect::<Vec<_>>(),
        "webmcp": session.webmcp,
        "input": session.input,
    });
    if session.offline {
        value["networkOffline"] = json!(true);
    }
    value
}

const REDACTED: &str = "[redacted]";
const SECRET_HEADERS: &[&str] = &["token", "secret", "key", "auth", "session", "csrf"];
const SECRET_QUERY: &[&str] = &["token", "secret", "key", "auth", "code", "sig", "session", "password"];

fn names_secret(name: &str, words: &[&str]) -> bool {
    let name = name.to_ascii_lowercase();
    words.iter().any(|word| name.contains(word))
}

fn stored_headers(headers: &[(String, String)], base: &str) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let lowered = value.trim_start().to_ascii_lowercase();
            if names_secret(name, SECRET_HEADERS) {
                (name.clone(), REDACTED.to_string())
            } else if names_secret(name, &["cookie"]) {
                (name.clone(), REDACTED.to_string())
            } else if url_header(name) {
                (name.clone(), stored_header_url(name, value, base))
            } else if lowered.starts_with("http://") || lowered.starts_with("https://") {
                (name.clone(), stored_url(value))
            } else {
                (name.clone(), value.clone())
            }
        })
        .collect()
}

fn url_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "location" | "content-location" | "referer" | "refresh" | "link"
    )
}

fn stored_header_url(name: &str, value: &str, base: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "link" => stored_link(value, base),
        "refresh" => stored_refresh(value, base),
        _ => stored_ref(value.trim(), base),
    }
}

fn stored_link(value: &str, base: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..=open]);
        rest = &rest[open + 1..];
        let close = rest.find('>').unwrap_or(rest.len());
        out.push_str(&stored_ref(&rest[..close], base));
        rest = &rest[close..];
    }
    out.push_str(rest);
    out
}

fn refresh_target(value: &str) -> Option<usize> {
    let lowered = value.to_ascii_lowercase();
    let assigned = lowered.match_indices("url").find_map(|(at, _)| {
        let after = lowered[at + 3..].trim_start();
        let rest = after.strip_prefix('=')?;
        Some(value.len() - rest.trim_start().len())
    });
    assigned.or_else(|| {
        let after = value.find(';')? + 1;
        let start = value.len() - value[after..].trim_start().len();
        (start < value.len()).then_some(start)
    })
}

fn stored_refresh(value: &str, base: &str) -> String {
    let Some(at) = refresh_target(value) else {
        return value.to_string();
    };
    let (head, target) = value.split_at(at);
    let quote = target.chars().next().filter(|c| matches!(c, '"' | '\''));
    let Some(quote) = quote else {
        return format!("{head}{}", stored_ref(target.trim(), base));
    };
    let inner = target[1..].trim_end_matches(quote);
    format!("{head}{quote}{}{quote}", stored_ref(inner, base))
}

fn stored_ref(raw: &str, base: &str) -> String {
    let lowered = raw.to_ascii_lowercase();
    if lowered.starts_with("http://") || lowered.starts_with("https://") {
        return stored_url(raw);
    }
    let resolved = url::Url::parse(base)
        .and_then(|base| base.join(raw).map(|url| (base, url)))
        .ok()
        .filter(|(_, url)| matches!(url.scheme(), "http" | "https"));
    let Some((base, resolved)) = resolved else {
        return stored_url(raw);
    };
    let stored = stored_url(resolved.as_str());
    let origin = resolved.origin().ascii_serialization();
    if base.origin().ascii_serialization() != origin {
        return stored;
    }
    stored.strip_prefix(&origin).unwrap_or(&stored).to_string()
}

fn query_key(pair: &str) -> String {
    let key = pair.split('=').next().unwrap_or("");
    url::form_urlencoded::parse(key.as_bytes())
        .next()
        .map(|(key, _)| key.into_owned())
        .unwrap_or_default()
}

fn without_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_string(),
    }
}

fn stored_url(url: &str) -> String {
    let url = without_userinfo(url.split('#').next().unwrap_or(""));
    let url = url.as_str();
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let pairs: Vec<String> = query
        .split('&')
        .map(|pair| {
            if pair.contains('=') && names_secret(&query_key(pair), SECRET_QUERY) {
                format!("{}={REDACTED}", pair.split('=').next().unwrap_or(""))
            } else {
                pair.to_string()
            }
        })
        .collect();
    format!("{base}?{}", pairs.join("&"))
}

const CONSOLE_LIMIT: usize = 2048;

fn base64url(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'-' || c == b'_'
}

fn holds_jwt(text: &str) -> bool {
    let bytes = text.as_bytes();
    text.match_indices("eyJ").any(|(at, _)| {
        let mut i = at;
        for part in 0..2 {
            let start = i;
            while i < bytes.len() && base64url(bytes[i]) {
                i += 1;
            }
            if i - start < 8 || bytes.get(i) != Some(&b'.') {
                return false;
            }
            i += 1;
            if part == 1 {
                return i < bytes.len() && base64url(bytes[i]);
            }
        }
        false
    })
}

fn holds_secret_pair(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    lowered.match_indices(['=', ':']).any(|(at, _)| {
        let before = lowered[..at].trim_end_matches([' ', '"', '\'']);
        let start = before
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
            .map_or(0, |found| found + 1);
        let key = &before[start..];
        let value: String = lowered[at + 1..]
            .trim_start_matches([' ', '"', '\''])
            .chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, ';' | ',' | '&' | '"' | '\''))
            .collect();
        value.len() >= 8 && names_secret(key, SECRET_QUERY)
    })
}

fn holds_credential(lowered: &str, scheme: &str) -> bool {
    lowered.match_indices(scheme).any(|(at, _)| {
        lowered[at + scheme.len()..]
            .chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | ','))
            .count()
            >= 8
    })
}

const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("sk-", 16),
    ("sk_live_", 16),
    ("sk_test_", 16),
    ("rk_live_", 16),
    ("rk_test_", 16),
    ("pk_live_", 16),
    ("pk_test_", 16),
    ("whsec_", 16),
    ("AIza", 35),
    ("npm_", 30),
    ("hf_", 30),
    ("pypi-", 32),
    ("ghp_", 20),
    ("gho_", 20),
    ("ghu_", 20),
    ("ghs_", 20),
    ("ghr_", 20),
    ("github_pat_", 20),
    ("glpat-", 20),
    ("xoxa-", 10),
    ("xoxb-", 10),
    ("xoxp-", 10),
    ("xoxr-", 10),
];

fn token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
}

fn words(text: &str, part: fn(char) -> bool) -> impl Iterator<Item = &str> {
    text.split(move |c: char| !part(c)).filter(|word| !word.is_empty())
}

fn holds_prefixed_token(text: &str) -> bool {
    words(text, token_char).any(|word| {
        TOKEN_PREFIXES.iter().any(|(prefix, min)| {
            word.strip_prefix(prefix)
                .is_some_and(|rest| rest.len() >= *min && random_mix(rest))
        }) || aws_key(word)
    })
}

fn random_mix(rest: &str) -> bool {
    rest.bytes().any(|b| b.is_ascii_digit())
        || (rest.bytes().any(|b| b.is_ascii_uppercase()) && rest.bytes().any(|b| b.is_ascii_lowercase()))
}

fn aws_key(word: &str) -> bool {
    word.len() == 20
        && (word.starts_with("AKIA") || word.starts_with("ASIA"))
        && word[4..].bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

fn entropy(word: &str) -> f64 {
    let mut counts = [0usize; 256];
    for byte in word.bytes() {
        counts[byte as usize] += 1;
    }
    let len = word.len() as f64;
    counts
        .iter()
        .filter(|count| **count > 0)
        .map(|count| {
            let p = *count as f64 / len;
            -p * p.log2()
        })
        .sum()
}

fn high_entropy(word: &str) -> bool {
    let word = word.trim_end_matches('=');
    if word.len() < 32 {
        return false;
    }
    let digits = word.bytes().any(|b| b.is_ascii_digit());
    if word.bytes().all(|b| b.is_ascii_hexdigit()) {
        return digits && word.bytes().any(|b| b.is_ascii_alphabetic()) && entropy(word) >= 3.0;
    }
    digits
        && word.bytes().any(|b| b.is_ascii_uppercase())
        && word.bytes().any(|b| b.is_ascii_lowercase())
        && entropy(word) >= 4.0
        && mixes_classes(word)
}

fn mixes_classes(word: &str) -> bool {
    switches(word) * 20 >= (classes(word).len() - 1) * 7
}

fn switches(word: &str) -> usize {
    classes(word).windows(2).filter(|pair| pair[0] != pair[1]).count()
}

fn classes(word: &str) -> Vec<u8> {
    word
        .bytes()
        .filter_map(|b| match b {
            b'A'..=b'Z' => Some(0),
            b'a'..=b'z' => Some(1),
            b'0'..=b'9' => Some(2),
            _ => None,
        })
        .collect()
}

fn lowercase_secret(word: &str) -> bool {
    word.len() >= 32
        && !word.bytes().any(|b| b.is_ascii_uppercase())
        && word.bytes().any(|b| b.is_ascii_digit())
        && word.bytes().any(|b| b.is_ascii_lowercase())
        && entropy(word) >= 3.7
        && switches(word) * 20 >= (word.len() - 1) * 3
        && short_letter_runs(word)
}

fn short_letter_runs(word: &str) -> bool {
    let runs = words(word, |c| c.is_ascii_lowercase());
    let (count, letters) = runs.fold((0, 0), |(count, letters), run| (count + 1, letters + run.len()));
    letters <= count * 6
}

fn holds_high_entropy(text: &str) -> bool {
    words(text, |c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '-' | '_')).any(high_entropy)
        || words(text, |c| c.is_ascii_alphanumeric()).any(lowercase_secret)
}

fn secret_text(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    holds_prefixed_token(text)
        || holds_high_entropy(text)
        || holds_credential(&lowered, "bearer ")
        || holds_credential(&lowered, "basic ")
        || holds_jwt(text)
        || holds_secret_pair(text)
}

fn stored_console(entry: &ConsoleEntry) -> Option<Value> {
    if secret_text(&entry.text) || entry.cookie_value
        || entry.text.to_ascii_lowercase().contains("cookie") && entry.text.contains('=')
    {
        return None;
    }
    let mut text = entry.text.clone();
    if text.len() > CONSOLE_LIMIT {
        let mut cut = CONSOLE_LIMIT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("…[truncated]");
    }
    Some(trace::console_json(&ConsoleEntry {
        text,
        url: stored_url(&entry.url),
        ..entry.clone()
    }))
}

fn stored_request(request: &NetRequest) -> Value {
    trace::request_json(&NetRequest {
        url: stored_url(&request.url),
        request_headers: stored_headers(&request.request_headers, &request.url),
        response_headers: stored_headers(&request.response_headers, &request.url),
        request_body: None,
        response_body: None,
        response_base64: false,
        ..request.clone()
    })
}

fn encode_lists<T>(
    tabs: &BTreeMap<String, Vec<T>>,
    encode_one: fn(&T) -> Option<Value>,
) -> Value {
    let mut object = serde_json::Map::new();
    for (target, entries) in tabs {
        object.insert(
            target.clone(),
            Value::Array(entries.iter().filter_map(encode_one).collect()),
        );
    }
    Value::Object(object)
}

pub fn push_trace(
    session: &mut Session,
    target: &str,
    console: Vec<ConsoleEntry>,
    requests: Vec<NetRequest>,
) {
    if !console.is_empty() {
        let stored = session.console.entry(target.to_string()).or_default();
        for entry in console {
            if !entry.stamp.is_empty()
                && stored.iter().any(|have| {
                    (have.stamp == entry.stamp
                        && have.kind == entry.kind
                        && have.text == entry.text
                        && have.url == entry.url
                        && have.line == entry.line)
                        || stored_console(have).is_some_and(|saved| Some(saved) == stored_console(&entry))
                })
            {
                continue;
            }
            stored.push(entry);
        }
    }
    if !requests.is_empty() {
        session
            .requests
            .entry(target.to_string())
            .or_default()
            .extend(requests);
    }
}

pub fn clear_console(session: &mut Session, target: &str) {
    session.console.remove(target);
}

pub fn clear_requests(session: &mut Session, target: &str) {
    session.requests.remove(target);
}

pub fn stored_label<'a>(session: &'a Session, target: &str, id: &str) -> Option<&'a str> {
    session
        .snapshots
        .get(target)?
        .get(id)?
        .label
        .as_deref()
        .filter(|label| !label.is_empty())
}

pub fn save(session: &Session) -> Result<(), String> {
    store(session, false)
}

pub fn save_webmcp(session: &Session) -> Result<(), String> {
    store(session, true)
}

fn store(session: &Session, webmcp_writer: bool) -> Result<(), String> {
    let mut stored = session.clone();
    if usable_endpoint(stored.endpoint.as_deref(), stored.provenance.as_deref()).is_none() {
        stored.endpoint = None;
        stored.provenance = None;
    }
    let Some(file) = path_named(stored.session_name.as_deref(), stored.cwd.as_deref()) else {
        if cfg!(target_arch = "wasm32") {
            return Err("session: HOME is not set\n".into());
        }
        return Err("session: user config dir is not absolute\n".into());
    };
    if !webmcp_writer && file.exists() {
        stored.webmcp = load_at(&file).webmcp;
    }
    write_at(&file, &stored)
}

pub fn without_state(mut output: crate::args::Output, message: &str) -> crate::args::Output {
    let reason = message.trim_end().trim_start_matches("session: ");
    output.stderr.push_str(&format!(
        "playwright-cli: running without saved state ({reason})\n"
    ));
    output
}

pub fn write_at(path: &Path, session: &Session) -> Result<(), String> {
    let body = serde_json::to_string_pretty(&encode(session)).unwrap_or_else(|_| "{}".into());
    write_private(path, &body)
}

pub fn write_private(path: &Path, body: &str) -> Result<(), String> {
    write_with_dirs(path, body, true)
}

pub fn write_private_file(path: &Path, body: &str) -> Result<(), String> {
    write_with_dirs(path, body, false)
}

fn write_with_dirs(path: &Path, body: &str, private_dirs: bool) -> Result<(), String> {
    #[cfg(all(not(target_os = "wasi"), unix))]
    if path.is_file() { private_file(path)?; }
    path.file_name().ok_or_else(|| "session: invalid state file path\n".to_string())?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let clock = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    #[cfg(target_os = "wasi")]
    let process = fs::read_to_string("/proc/self/stat").unwrap_or_default();
    #[cfg(not(target_os = "wasi"))]
    let process = std::process::id().to_string();
    let token = cdp_client::hash::sha256_hex(format!("{process}:{clock}:{counter}").as_bytes());
    let staged = path.with_file_name(format!(".slicc-state-{token}.tmp"));
    stage_private(&staged, body, private_dirs)?;
    let result = fs::rename(&staged, path).map_err(|err| format!("session: {err}\n"));
    if result.is_err() { let _ = fs::remove_file(&staged); }
    result
}

fn stage_private(path: &Path, body: &str, private_dirs: bool) -> Result<(), String> {
    #[cfg(target_os = "wasi")]
    {
        let old = wasi_mode::set_umask(0o077);
        let written = wasi_write(path, body, private_dirs);
        if let Some(old) = old { wasi_mode::set_umask(old); }
        written
    }
    #[cfg(not(target_os = "wasi"))]
    {
        host_write(path, body, private_dirs)
    }
}

#[cfg(target_os = "wasi")]
fn wasi_write(path: &Path, body: &str, private_dirs: bool) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|err| format!("session: {err}\n"))?;
        if private_dirs {
            for item in [Some(dir), dir.parent()].into_iter().flatten() {
                if let Ok(handle) = fs::File::open(item) {
                    let _ = wasi_mode::chmod(handle.as_raw_fd(), 0o700);
                }
            }
        }
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| format!("session: {err}\n"))?;
    let written = wasi_mode::chmod(file.as_raw_fd(), 0o600).and_then(|()| {
        #[cfg(test)]
        PRIVATE_OPEN.with(|hook| { if let Some(opened) = hook.borrow().as_ref() { opened(path); } });
        std::io::Write::write_all(&mut file, body.as_bytes()).map_err(|err| format!("session: {err}\n"))
    });
    drop(file);
    if written.is_err() { let _ = fs::remove_file(path); }
    written
}

#[cfg(target_os = "wasi")]
mod wasi_mode {
    const ENOSYS: i32 = 52;

    #[link(wasm_import_module = "slicc_fs")]
    unsafe extern "C" {
        #[link_name = "fd_chmod"]
        fn slicc_fd_chmod(fd: i32, mode: i32) -> i32;
        #[link_name = "umask"]
        fn slicc_umask(mask: i32, old: *mut u32) -> i32;
    }

    pub fn chmod(fd: i32, mode: i32) -> Result<(), String> {
        match unsafe { slicc_fd_chmod(fd, mode) } {
            0 | ENOSYS => Ok(()),
            errno => Err(format!("session: chmod failed (wasi errno {errno})\n")),
        }
    }

    pub fn set_umask(mask: u32) -> Option<u32> {
        let mut old = 0u32;
        match unsafe { slicc_umask(mask as i32, &mut old) } {
            0 => Some(old),
            _ => None,
        }
    }
}

#[cfg(not(target_os = "wasi"))]
fn host_write(path: &Path, body: &str, private_dirs: bool) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|err| format!("session: {err}\n"))?;
        #[cfg(unix)]
        if private_dirs {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::Permissions::from_mode(0o700);
            let _ = fs::set_permissions(dir, mode.clone());
            if let Some(parent) = dir.parent() {
                let _ = fs::set_permissions(parent, mode);
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|err| format!("session: {err}\n"))?;
        let written = private_file(path).and_then(|()| {
            #[cfg(test)]
            PRIVATE_OPEN.with(|hook| { if let Some(opened) = hook.borrow().as_ref() { opened(path); } });
            file.write_all(body.as_bytes()).map_err(|err| format!("session: {err}\n"))
        });
        drop(file);
        if written.is_err() { let _ = fs::remove_file(path); }
        return written;
    }
    #[cfg(not(unix))]
    {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(path)
            .map_err(|err| format!("session: {err}\n"))?;
        #[cfg(test)]
        PRIVATE_OPEN.with(|hook| { if let Some(opened) = hook.borrow().as_ref() { opened(path); } });
        let written = file.write_all(body.as_bytes()).map_err(|err| format!("session: {err}\n"));
        drop(file);
        if written.is_err() { let _ = fs::remove_file(path); }
        written
    }
}

pub fn known_provenance(text: &str) -> bool {
    matches!(text, "explicit" | "seven-profile" | "probe")
}

pub fn note_owned(session: &mut Session, id: &str) {
    if id.is_empty() || session.owned.iter().any(|have| have == id) {
        return;
    }
    session.owned.push(id.to_string());
}

pub fn usable_endpoint(endpoint: Option<&str>, provenance: Option<&str>) -> Option<String> {
    let url = endpoint.filter(|text| !text.is_empty())?;
    if !known_provenance(provenance?) {
        return None;
    }
    if !loopback_url(url) {
        return None;
    }
    Some(url.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn reuse_saved(
    endpoint: Option<&str>,
    provenance: Option<&str>,
    version_body: Option<&str>,
) -> Option<String> {
    let url = usable_endpoint(endpoint, provenance)?;
    if provenance == Some("probe") {
        discovery_url(&url)?;
        crate::connect::websocket_url_from_version(version_body?).ok()?;
    }
    Some(url)
}

#[cfg(not(target_arch = "wasm32"))]
fn discovery_url(endpoint: &str) -> Option<&str> {
    let lower = endpoint.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://")).then_some(endpoint)
}

#[cfg(test)]
thread_local! {
    static FAIL_PRIVATE_FILE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PRIVATE_OPEN: std::cell::RefCell<Option<Box<dyn Fn(&Path)>>> = const { std::cell::RefCell::new(None) };
}

fn private_file(path: &Path) -> Result<(), String> {
    #[cfg(test)]
    if FAIL_PRIVATE_FILE.get() {
        return Err("session: Operation not permitted\n".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|err| format!("session: {err}\n"))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

pub fn loopback_url(url: &str) -> bool {
    let Ok(endpoint) = cdp_client::net::parse_endpoint(url) else {
        return false;
    };
    if !matches!(endpoint.scheme.as_str(), "http" | "https" | "ws" | "wss") {
        return false;
    }
    let host = endpoint.host.to_ascii_lowercase();
    host == "localhost" || host == "::1" || ipv4_loopback(&host)
}

fn ipv4_loopback(host: &str) -> bool {
    let mut parts = host.split('.');
    let Some(first) = parts.next().and_then(|text| text.parse::<u8>().ok()) else {
        return false;
    };
    let Some(_) = parts.next().and_then(|text| text.parse::<u8>().ok()) else {
        return false;
    };
    let Some(_) = parts.next().and_then(|text| text.parse::<u8>().ok()) else {
        return false;
    };
    let Some(_) = parts.next().and_then(|text| text.parse::<u8>().ok()) else {
        return false;
    };
    parts.next().is_none() && first == 127
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdp_client::hash::sha256_hex;

    include!("session_atomic_test.rs");

    #[test]
    fn an_old_session_without_label_still_loads() {
        let text = r#"{"current":"T","snapshots":{"T":{"e1":{"backendNodeId":4,"selector":"button","frameId":null}}}}"#;
        let session = decode(text);
        assert_eq!(session.current.as_deref(), Some("T"));
        let rec = &session.snapshots["T"]["e1"];
        assert_eq!(rec.selector, "button");
        assert_eq!(rec.backend_node_id, Some(4));
        assert!(rec.label.is_none());
        assert!(stored_label(&session, "T", "e1").is_none());
        assert!(stored_label(&session, "T", "e9").is_none());
        assert!(session.endpoint.is_none());
        assert!(session.provenance.is_none());
        assert!(session.owned.is_empty());
        assert!(session.console.is_empty());
        assert!(session.requests.is_empty());
    }

    #[test]
    fn an_endpoint_round_trips_and_a_blank_one_is_absent() {
        let session = Session {
            endpoint: Some("http://127.0.0.1:9333/json/version".to_string()),
            ..Session::default()
        };
        let again = decode(&encode(&session).to_string());
        assert_eq!(
            again.endpoint.as_deref(),
            Some("http://127.0.0.1:9333/json/version")
        );
        let blank = decode(r#"{"endpoint":"","current":null}"#);
        assert!(blank.endpoint.is_none());
    }

    #[test]
    fn a_label_round_trips_and_an_empty_one_is_absent() {
        let mut refs = BTreeMap::new();
        refs.insert(
            "e1".to_string(),
            RefRec {
                backend_node_id: Some(7),
                selector: "button".to_string(),
                frame_id: None,
                label: Some("button \"Sign in\"".to_string()),
                ..RefRec::default()
            },
        );
        refs.insert(
            "e2".to_string(),
            RefRec {
                backend_node_id: None,
                selector: "input".to_string(),
                frame_id: Some("F".to_string()),
                label: Some(String::new()),
                target: Some("F".to_string()),
                owner: Some(9),
                page: Some("S".to_string()),
            },
        );
        let session = Session {
            current: Some("T".to_string()),
            snapshots: BTreeMap::from([("T".to_string(), refs)]),
            ..Session::default()
        };
        let again = decode(&encode(&session).to_string());
        assert_eq!(stored_label(&again, "T", "e1"), Some("button \"Sign in\""));
        assert!(stored_label(&again, "T", "e2").is_none());
        assert_eq!(again.snapshots["T"]["e2"].frame_id.as_deref(), Some("F"));
        assert_eq!(again.snapshots["T"]["e2"].target.as_deref(), Some("F"));
        assert_eq!(again.snapshots["T"]["e2"].owner, Some(9));
        assert_eq!(again.snapshots["T"]["e2"].page, None);
        assert_eq!(again.snapshots["T"]["e1"].target, None);
    }

    #[test]
    fn console_entries_that_look_secret_are_not_saved() {
        let entry = |text: &str| ConsoleEntry {
            kind: "log".to_string(),
            text: text.to_string(),
            url: "https://example.test/app.js?sig=abc123#L4".to_string(),
            line: 1,
            stamp: String::new(),
            cookie_value: false,
        };
        let texts = [
            "Authorization: Bearer abcdefgh12345678",
            "creds Basic dXNlcjpwYXNzd29yZA==",
            "jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.c2lnbmF0dXJl",
            "document.cookie = sid=abc; theme=dark",
            "{\"access_token\":\"abcdefgh12345678\"}",
            "password=hunter2hunter2",
            "kept: cookie banner shown",
            "kept: the session ended",
            "kept: basic usage",
        ];
        let mut session = Session::default();
        push_trace(
            &mut session,
            "T",
            texts.iter().map(|text| entry(text)).collect(),
            Vec::new(),
        );
        push_trace(&mut session, "T", vec![entry(&"x".repeat(5000))], Vec::new());
        let written = encode(&session).to_string();
        let again = decode(&written);
        let kept: Vec<&str> = again.console["T"].iter().map(|entry| entry.text.as_str()).collect();
        assert_eq!(kept.len(), 4, "{kept:?}");
        assert_eq!(&kept[..3], ["kept: cookie banner shown", "kept: the session ended", "kept: basic usage"]);
        assert!(kept[3].ends_with("…[truncated]") && kept[3].len() < 2100);
        assert!(again.console["T"].iter().all(|entry| entry.url == "https://example.test/app.js?sig=[redacted]"));
        assert_eq!(session.console["T"].len(), 10);
    }

    #[test]
    fn console_and_requests_round_trip_with_secrets_redacted() {
        let mut session = Session::default();
        push_trace(
            &mut session,
            "T",
            vec![ConsoleEntry {
                kind: "log".to_string(),
                text: "hi".to_string(),
                url: "https://user:secret@example.test/a?q=1#h".to_string(),
                line: 4,
                stamp: "9.5".to_string(),
                cookie_value: false,
            }],
            vec![NetRequest {
                method: "GET".to_string(),
                url: "https://example.test/a?q=1&access_token=tok1&Code=c2&sig=s3#id_token=t4".to_string(),
                resource_type: "fetch".to_string(),
                status: Some(200),
                status_text: "OK".to_string(),
                failed: None,
                mime: "text/plain".to_string(),
                request_headers: vec![
                    ("cookie".to_string(), "session=secret".to_string()),
                    ("Authorization".to_string(), "Bearer abc".to_string()),
                    ("Proxy-Authorization".to_string(), "Basic def".to_string()),
                    ("X-CSRF-Token".to_string(), "csrf1".to_string()),
                    ("x-api-key".to_string(), "key1".to_string()),
                    ("accept".to_string(), "text/plain".to_string()),
                    ("Referer".to_string(), "https://example.test/?code=r5#x".to_string()),
                ],
                response_headers: vec![
                    ("Set-Cookie".to_string(), "a=b".to_string()),
                    ("x-session-id".to_string(), "sid1".to_string()),
                    ("content-type".to_string(), "text/plain".to_string()),
                    ("location".to_string(), "https://example.test/cb?state=1&code=l6".to_string()),
                ],
                request_body: Some("body".to_string()),
                response_body: Some("BODY7SECRET".to_string()),
                response_base64: true,
                cookie_scopes: Vec::new(),
            }],
        );
        let written = encode(&session).to_string();
        for secret in ["session=secret", "a=b", "Bearer abc", "Basic def", "csrf1", "key1", "sid1", "tok1", "c2", "s3", "t4", "r5", "l6", "\"body\"", "BODY7SECRET"] {
            assert!(!written.contains(secret), "{secret} was written: {written}");
        }
        assert_eq!(session.requests["T"][0].request_headers[0].1, "session=secret");
        let mut again = decode(&written);
        assert_eq!(again.console["T"][0].url, "https://example.test/a?q=1");
        assert_eq!(again.console["T"][0].stamp, "9.5");
        push_trace(
            &mut again,
            "T",
            vec![
                ConsoleEntry {
                    kind: "log".to_string(),
                    text: "hi".to_string(),
                    url: "https://user:secret@example.test/a?q=1#h".to_string(),
                    line: 4,
                    stamp: "9.5".to_string(),
                    cookie_value: false,
                },
                ConsoleEntry {
                    kind: "log".to_string(),
                    text: "hi".to_string(),
                    url: "https://user:secret@example.test/a?q=1#h".to_string(),
                    line: 4,
                    stamp: "9.75".to_string(),
                    cookie_value: false,
                },
            ],
            Vec::new(),
        );
        assert_eq!(again.console["T"].len(), 2);
        assert_eq!(again.console["T"][1].stamp, "9.75");
        push_trace(
            &mut again,
            "T",
            vec![ConsoleEntry {
                kind: "log".to_string(),
                text: "again".to_string(),
                url: String::new(),
                line: 0,
                stamp: String::new(),
                cookie_value: false,
            }],
            Vec::new(),
        );
        assert_eq!(again.console["T"].len(), 3);
        assert_eq!(
            again.requests["T"][0].url,
            "https://example.test/a?q=1&access_token=[redacted]&Code=[redacted]&sig=[redacted]"
        );
        let pair = |name: &str, value: &str| (name.to_string(), value.to_string());
        assert_eq!(
            again.requests["T"][0].request_headers,
            vec![
                pair("cookie", "[redacted]"),
                pair("Authorization", "[redacted]"),
                pair("Proxy-Authorization", "[redacted]"),
                pair("X-CSRF-Token", "[redacted]"),
                pair("x-api-key", "[redacted]"),
                pair("accept", "text/plain"),
                pair("Referer", "https://example.test/?code=[redacted]"),
            ]
        );
        assert_eq!(
            again.requests["T"][0].response_headers,
            vec![
                pair("Set-Cookie", "[redacted]"),
                pair("x-session-id", "[redacted]"),
                pair("content-type", "text/plain"),
                pair("location", "https://example.test/cb?state=1&code=[redacted]"),
            ]
        );
        assert_eq!(again.requests["T"][0].request_body, None);
        assert_eq!(again.requests["T"][0].response_body, None);
        assert!(!again.requests["T"][0].response_base64);
        assert_eq!(session.requests["T"][0].response_body.as_deref(), Some("BODY7SECRET"));
        clear_console(&mut again, "T");
        clear_requests(&mut again, "T");
        assert!(again.console.is_empty());
        assert!(again.requests.is_empty());
    }

    #[test]
    fn bare_tokens_and_high_entropy_strings_drop_the_console_entry() {
        let dropped = [
            "key sk-testPLNT0123456789abcdefXYZ",
            "sk-proj-PLNT_0123456789abcdefghij",
            "ghp_PLNT0123456789abcdefghijklmnopqrstuv",
            "gho_PLNT0123456789abcdefghij",
            "ghu_PLNT0123456789abcdefghij",
            "ghs_PLNT0123456789abcdefghij",
            "ghr_PLNT0123456789abcdefghij",
            "(github_pat_11PLNT0123456789_abcdefghijklmnopqrstuvwxyz)",
            "aws AKIAPLNT0123456789AB.",
            "ASIAPLNT0123456789AB",
            "stripe sk_live_PLNT0123456789abcdefgh",
            "sk_test_PLNT0123456789abcdefgh",
            "rk_live_PLNT0123456789abcdefgh",
            "rk_test_PLNT0123456789abcdefgh",
            "pk_live_PLNT0123456789abcdefgh",
            "pk_test_PLNT0123456789abcdefgh",
            "whsec_PLNT0123456789abcdefgh",
            "google AIzaSyPLNT0123456789abcdefghijklmnopqrs",
            "npm_PLNT0123456789abcdefghijklmnopqrstu",
            "hf_PLNTabcdefghijklmnopqrstuvwxyzABCDEFGH",
            "pypi-AgEIcHlwaS5vcmcPLNT0123456789abcdefghij",
            "key 7f3k9q2m8x1v4b6n0c5z7l2j9h4g1d8s6a3p",
            "lower+digit k7f3k9q2m8x1v4b6n0c5z7l2j9h4g1d8s6a3p0w2.",
            "glpat-PLNT0123456789abcdefgh",
            "xoxa-PLNT-0123456789",
            "xoxb-1234567890-PLNTabcdef",
            "xoxp-1234567890-PLNTabcdef",
            "xoxr-1234567890-PLNTabcdef",
            "hex 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "b64 Zk9xL3Rq2mWv8sPb7NcYd4hGe1RaKu6TfQi0Xo5Jz9==",
            "b64 Zk9xL3Rq+mWv8sPb/NcYd4hGe1RaKu6TfQi0Xo5Jz9",
        ];
        for text in dropped {
            assert!(secret_text(text), "{text}");
        }
        let kept = [
            "a plain sentence about the checkout page",
            "task-0123456789abcdefghijklmnop finished",
            "sk-the-very-long-css-class-name-here",
            "AKIA0123456789ABCDEFG",
            "sk_live_short123",
            "sk_live_the_very_long_setting_name_here",
            "AIzaShortKey123",
            "npm_install_the_dependencies_for_this_package_now",
            "hf_the_lowercase_hub_setting_without_digits_here",
            "pypi-the-package-index-mirror-configuration-name",
            "my-blog-post-2023-about-things-and-stuff-today",
            "assets/js/chunk-vendors/v2/index/main/app",
            "release-2024-10-17-build-1234-candidate-3-final",
            "user_profile_settings_page_v2_loaded_ok_today",
            "abc123abc123abc123abc123abc123abc123",
            "chapter1section2paragraph3sentence4wordsfive",
            "release1candidate2build3final4version5",
            "AKIAplnt0123456789ab",
            "ghp_short1",
            "xoxz-1234567890-PLNTabcdef",
            "id 123e4567-e89b-12d3-a456-426614174000",
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
            "0123456789012345678901234567890123456789",
            "src/components/HeaderNavigationMenu2023/index",
            "ReactDOMServerRenderingStream2Production",
            "webpackChunk_N_E_app_layout_tsx_0123456",
            "data-testid-ProductCard-Item-12345-Variant-B",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaAAAAAAAAAA1",
            "averyveryverylonglowercasewordwithdigits1234567890",
            "Zk9xL3Rq2mWv8sPb7NcYd4hGe1Ra==",
        ];
        for text in kept {
            assert!(!secret_text(text), "{text}");
        }
        assert_eq!(entropy("aaaa"), 0.0);
        assert_eq!(entropy("abcd"), 2.0);
    }

    #[test]
    fn relative_and_url_valued_headers_lose_their_secret_queries() {
        let base = "https://example.test/start?q=1";
        let pair = |name: &str, value: &str| (name.to_string(), value.to_string());
        let headers = vec![
            pair("Location", "/next?token=abc&page=2#frag"),
            pair("location", "next?code=X&access_token=Y"),
            pair("Content-Location", "?state=1&sig=Z"),
            pair("Location", "//other.test/cb?code=C2"),
            pair("Location", "//user:pw@example.test/cb?code=C3"),
            pair("Location", "/\\evil.test/cb?code=C4"),
            pair("Location", "HTTPS://example.test/cb?code=C5"),
            pair("Location", "javascript:alert(1)?token=C6"),
            pair("Link", "</a.css?token=L1>; rel=preload, <https://cdn.test/b?sig=L2>; rel=next, <broken?key=L3"),
            pair("Refresh", "5; url=/r?token=R1"),
            pair("Refresh", "0;URL='/r?code=R2'"),
            pair("Refresh", "5"),
            pair("Refresh", "0; url = /r?token=R5"),
            pair("Refresh", "0; /r?code=R6"),
            pair("Refresh", "0;"),
            pair("Refresh", "0; urlx /r?code=R7"),
            pair("Referer", "https://example.test/?code=R3"),
            pair("x-forwarded", "https://example.test/?code=R4"),
            pair("x-plain", "/keep?token=as-is"),
        ];
        let stored = stored_headers(&headers, base);
        let values: Vec<&str> = stored.iter().map(|(_, value)| value.as_str()).collect();
        assert_eq!(
            values,
            vec![
                "/next?token=[redacted]&page=2",
                "/next?code=[redacted]&access_token=[redacted]",
                "/start?state=1&sig=[redacted]",
                "https://other.test/cb?code=[redacted]",
                "/cb?code=[redacted]",
                "https://evil.test/cb?code=[redacted]",
                "HTTPS://example.test/cb?code=[redacted]",
                "javascript:alert(1)?token=[redacted]",
                "</a.css?token=[redacted]>; rel=preload, <https://cdn.test/b?sig=[redacted]>; rel=next, </broken?key=[redacted]",
                "5; url=/r?token=[redacted]",
                "0;URL='/r?code=[redacted]'",
                "5",
                "0; url = /r?token=[redacted]",
                "0; /r?code=[redacted]",
                "0;",
                "0; /urlx%20/r?code=[redacted]",
                "https://example.test/?code=[redacted]",
                "https://example.test/?code=[redacted]",
                "/keep?token=as-is",
            ]
        );
        assert_eq!(stored_ref("/x?token=T", "not a url"), "/x?token=[redacted]");
    }

    #[test]
    fn a_planted_cwd_session_is_not_the_saved_endpoint() {
        let cwd = std::env::temp_dir().join(format!(
            "slicc-planted-cwd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        ));
        let config = std::env::temp_dir().join(format!(
            "slicc-planted-config-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&cwd);
        fs::create_dir_all(cwd.join(".playwright-cli")).unwrap();
        fs::write(
            cwd.join(".playwright-cli").join("session.json"),
            r#"{"endpoint":"https://evil.example/json/version","provenance":"explicit","current":"personal"}"#,
        )
        .unwrap();
        let stored = session_path(config.to_str().unwrap(), &cwd, None).unwrap();
        assert_ne!(stored, cwd.join(".playwright-cli").join("session.json"));
        assert!(stored.starts_with(&config));
        assert!(!stored.starts_with(&cwd));
        assert!(load_at(&stored).endpoint.is_none());
        assert!(load_at(&cwd.join(".playwright-cli").join("session.json"))
            .endpoint
            .is_some());
        let _ = fs::remove_dir_all(&cwd);
        let _ = fs::remove_dir_all(&config);
    }

    #[test]
    fn saved_state_never_yields_a_remote_endpoint() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let cases = [
            "https://evil.example/json/version",
            "http://evil.example:8080/json/version",
            "wss://evil.example/devtools/browser/x",
            "ws://10.1.2.3:8999/devtools/browser/x",
            "ws://evil.example:8999/devtools/browser/x",
            "http://127.0.0.1@evil.example/json/version",
            "ftp://127.0.0.1:9333/json/version",
        ];
        for url in cases {
            assert!(usable_endpoint(Some(url), Some("explicit")).is_none(), "{url}");
            assert!(!loopback_url(url), "{url}");
        }
        assert_eq!(
            usable_endpoint(Some("http://127.0.0.1:9333/json/version"), Some("explicit")).as_deref(),
            Some("http://127.0.0.1:9333/json/version")
        );
        assert_eq!(
            usable_endpoint(Some("ws://localhost:9333/devtools/browser/abc"), Some("explicit")).as_deref(),
            Some("ws://localhost:9333/devtools/browser/abc")
        );
        assert_eq!(
            usable_endpoint(Some("http://[::1]:9333/json/version"), Some("seven-profile")).as_deref(),
            Some("http://[::1]:9333/json/version")
        );
        assert!(usable_endpoint(Some("http://127.0.0.1:9333/json/version"), None).is_none());
        assert!(usable_endpoint(Some("http://127.0.0.1:9333/json/version"), Some("planted")).is_none());
        assert!(loopback_url("http://127.8.8.8:9222/json/version"));
        assert!(!loopback_url("http://10.0.0.1/json/version"));
        let disguised = "http://evil.example@127.0.0.1:9222/json/version";
        let parsed = cdp_client::net::parse_endpoint(disguised).unwrap();
        assert_eq!(parsed.host, "127.0.0.1");
        assert_eq!(parsed.port, 9222);
        assert!(loopback_url(disguised));
        let remote = "http://127.0.0.1:9222@evil.example/json/version";
        let parsed = cdp_client::net::parse_endpoint(remote).unwrap();
        assert_eq!(parsed.host, "evil.example");
        assert!(!loopback_url(remote));
        assert!(usable_endpoint(Some(remote), Some("explicit")).is_none());
        assert!(loopback_url("http://user@[::1]:9333/json/version"));
        assert!(loopback_url("http://127.0.0.1:9333?x=1"));
        let alive = r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:9222/devtools/browser/personal"}"#;
        assert_eq!(
            reuse_saved(Some("http://127.0.0.1:9222/json/version"), Some("probe"), Some(alive)).as_deref(),
            Some("http://127.0.0.1:9222/json/version")
        );
        assert!(reuse_saved(Some("http://127.0.0.1:9222/json/version"), Some("probe"), Some("{}")).is_none());
        assert!(reuse_saved(Some("http://127.0.0.1:9222/json/version"), Some("probe"), None).is_none());
        assert!(reuse_saved(Some("ws://127.0.0.1:9222/devtools/browser/x"), Some("probe"), Some(alive)).is_none());
        assert!(reuse_saved(Some("https://evil.example/json/version"), Some("explicit"), None).is_none());
        assert_eq!(
            reuse_saved(Some("http://127.0.0.1:9333/json/version"), Some("explicit"), None).as_deref(),
            Some("http://127.0.0.1:9333/json/version")
        );
        assert_eq!(
            reuse_saved(Some("http://127.0.0.1:9333/json/version"), Some("seven-profile"), None).as_deref(),
            Some("http://127.0.0.1:9333/json/version")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_saved_session_is_private_to_the_user() {
        let root = std::env::temp_dir().join(format!("slicc-session-mode-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let path = session_path(root.to_str().unwrap(), Path::new("/work"), None).unwrap();
        let session = Session {
            endpoint: Some("http://127.0.0.1:9333/json/version".to_string()),
            provenance: Some("probe".to_string()),
            owned: vec!["OWN".to_string()],
            ..Session::default()
        };
        write_at(&path, &session).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let dir_mode = fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        let parent_mode = fs::metadata(path.parent().unwrap().parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
        assert_eq!(parent_mode, 0o700);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o666);
        write_at(&path, &session).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        let _ = load_at(&path);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let loaded = load_at(&path);
        assert_eq!(loaded.provenance.as_deref(), Some("probe"));
        assert_eq!(loaded.owned, vec!["OWN".to_string()]);
        let locked = session_path(root.to_str().unwrap(), Path::new("/work-locked"), None).unwrap();
        fs::write(&locked, "ORIGINAL\n").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o666)).unwrap();
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                super::FAIL_PRIVATE_FILE.set(false);
            }
        }
        super::FAIL_PRIVATE_FILE.set(true);
        let _reset = Reset;
        let err = write_at(&locked, &session).unwrap_err();
        assert_eq!(err, "session: Operation not permitted\n");
        assert_eq!(fs::read_to_string(&locked).unwrap(), "ORIGINAL\n");
        assert_eq!(fs::metadata(&locked).unwrap().permissions().mode() & 0o777, 0o666);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_relative_config_dir_is_not_a_session_path() {
        assert!(session_path(".config", Path::new("work"), None).is_none());
        assert!(session_path("AppData\\Roaming", Path::new("work"), None).is_none());
        assert!(session_path("", Path::new("work"), None).is_none());
        let stored = session_path(
            std::env::temp_dir().to_str().unwrap(),
            Path::new("/work"),
            None,
        )
        .unwrap();
        assert!(stored.is_absolute());
        let key = sha256_hex(Path::new("/work").as_os_str().as_encoded_bytes());
        assert!(stored.ends_with(format!("{key}.json")));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_directories_do_not_share_a_session_file() {
        use std::os::unix::ffi::OsStrExt;
        let left = Path::new(std::ffi::OsStr::from_bytes(b"/work/\xff"));
        let right = Path::new(std::ffi::OsStr::from_bytes(b"/work/\xfe"));
        assert_eq!(left.to_string_lossy(), right.to_string_lossy());
        let root = std::env::temp_dir();
        let root = root.to_str().unwrap();
        let a = session_path(root, left, None).unwrap();
        let b = session_path(root, right, None).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn named_session_uses_different_file_than_unnamed() {
        let root = std::env::temp_dir();
        let root = root.to_str().unwrap();
        let cwd = Path::new("/work");
        let unnamed = session_path(root, cwd, None).unwrap();
        let named = session_path(root, cwd, Some("agent-1")).unwrap();
        assert_ne!(unnamed, named);
        let other = session_path(root, cwd, Some("agent-2")).unwrap();
        assert_ne!(named, other);
    }

    #[test]
    fn empty_session_name_is_same_as_unnamed() {
        let root = std::env::temp_dir();
        let root = root.to_str().unwrap();
        let cwd = Path::new("/work");
        let unnamed = session_path(root, cwd, None).unwrap();
        let empty = session_path(root, cwd, Some("")).unwrap();
        assert_eq!(unnamed, empty);
    }

    #[test]
    fn session_name_round_trips() {
        let text = r#"{"current":"T","sessionName":"agent-1","snapshots":{}}"#;
        let session = decode(text);
        assert_eq!(session.session_name.as_deref(), Some("agent-1"));
        let again = decode(&encode(&session).to_string());
        assert_eq!(again.session_name.as_deref(), Some("agent-1"));
    }

    #[test]
    fn missing_session_name_decodes_as_none() {
        let text = r#"{"current":"T","snapshots":{}}"#;
        let session = decode(text);
        assert!(session.session_name.is_none());
    }

    #[test]
    fn two_named_sessions_keep_separate_current_tabs() {
        let unique = format!(
            "slicc-named-sess-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|t| t.as_nanos())
                .unwrap_or(0)
        );
        let cwd = std::env::temp_dir().join(&unique);
        fs::create_dir_all(&cwd).unwrap();
        let root = std::env::temp_dir()
            .join("slicc-cdp-session-tests")
            .to_string_lossy()
            .into_owned();
        let path_a = session_path(&root, &cwd, Some("agent-a")).unwrap();
        let path_b = session_path(&root, &cwd, Some("agent-b")).unwrap();
        assert_ne!(path_a, path_b);
        let sess_a = Session {
            current: Some("TAB_A".to_string()),
            session_name: Some("agent-a".to_string()),
            ..Session::default()
        };
        let sess_b = Session {
            current: Some("TAB_B".to_string()),
            session_name: Some("agent-b".to_string()),
            ..Session::default()
        };
        write_at(&path_a, &sess_a).unwrap();
        write_at(&path_b, &sess_b).unwrap();
        let loaded_a = load_at(&path_a);
        let loaded_b = load_at(&path_b);
        assert_eq!(loaded_a.current.as_deref(), Some("TAB_A"));
        assert_eq!(loaded_b.current.as_deref(), Some("TAB_B"));
        assert_eq!(loaded_a.session_name.as_deref(), Some("agent-a"));
        assert_eq!(loaded_b.session_name.as_deref(), Some("agent-b"));
        let _ = fs::remove_dir_all(&cwd);
        let _ = fs::remove_file(&path_a);
        let _ = fs::remove_file(&path_b);
    }

    #[test]
    fn unnamed_session_is_unchanged() {
        let unique = format!(
            "slicc-unnamed-sess-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|t| t.as_nanos())
                .unwrap_or(0)
        );
        let cwd = std::env::temp_dir().join(&unique);
        fs::create_dir_all(&cwd).unwrap();
        let root = std::env::temp_dir()
            .join("slicc-cdp-session-tests")
            .to_string_lossy()
            .into_owned();
        let path_unnamed = session_path(&root, &cwd, None).unwrap();
        let sess = Session {
            current: Some("TAB_X".to_string()),
            ..Session::default()
        };
        write_at(&path_unnamed, &sess).unwrap();
        let loaded = load_at(&path_unnamed);
        assert_eq!(loaded.current.as_deref(), Some("TAB_X"));
        assert!(loaded.session_name.is_none());
        let _ = fs::remove_dir_all(&cwd);
        let _ = fs::remove_file(&path_unnamed);
    }

    #[test]
    fn guard_ignores_current_from_different_session_name() {
        let text = r#"{"current":"T","sessionName":"agent-a","snapshots":{}}"#;
        let mut sess = decode(text);
        assert_eq!(sess.current.as_deref(), Some("T"));
        assert_eq!(sess.session_name.as_deref(), Some("agent-a"));
        if sess.session_name.as_deref() != Some("agent-b") {
            sess.current = None;
        }
        sess.session_name = Some("agent-b".to_string());
        assert!(sess.current.is_none());
    }

    #[test]
    fn guard_keeps_current_for_matching_session_name() {
        let text = r#"{"current":"T","sessionName":"agent-a","snapshots":{}}"#;
        let mut sess = decode(text);
        if sess.session_name.as_deref() != Some("agent-a") {
            sess.current = None;
        }
        assert_eq!(sess.current.as_deref(), Some("T"));
    }

    #[test]
    fn guard_unnamed_ignores_current_if_file_had_session_name() {
        let text = r#"{"current":"T","sessionName":"agent-a","snapshots":{}}"#;
        let mut sess = decode(text);
        if sess.session_name.is_some() {
            sess.current = None;
            sess.session_name = None;
        }
        assert!(sess.current.is_none());
        assert!(sess.session_name.is_none());
    }
}
