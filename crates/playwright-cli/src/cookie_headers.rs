use crate::cdp::{self, Cdp};
use crate::trace::NetRequest;
use cdp_client::protected::Protected;
use serde_json::{json, Value};

pub const CURRENT_VALUES: &str = "Cookie values are from the current cookie jar, not necessarily from this request.\n";

fn field<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or("")
}

pub fn scopes(request: &NetRequest, jar: &Value) -> Vec<Value> {
    let cookies = jar.get("cookies").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut scopes = Vec::new();
    for (headers, response) in [(&request.request_headers, false), (&request.response_headers, true)] {
        for (header, text) in headers {
            if text == "[redacted]" || !header.eq_ignore_ascii_case(if response { "set-cookie" } else { "cookie" }) {
                continue;
            }
            let pairs: Vec<&str> = if response { text.lines().collect() } else { text.split(';').collect() };
            for pair in pairs {
                let Some((name, value)) = pair.split(';').next().unwrap_or_default().trim().split_once('=') else { continue; };
                let name = name.trim();
                let matching = cookies.iter().find(|cookie| field(cookie, "name") == name && field(cookie, "value") == value.trim());
                let (domain, path) = if let Some(cookie) = matching {
                    (field(cookie, "domain").to_string(), field(cookie, "path").to_string())
                } else if response {
                    let url = url::Url::parse(&request.url).ok();
                    let mut domain = url.as_ref().and_then(|url| url.host_str()).unwrap_or_default().to_string();
                    let mut path = url.as_ref().and_then(|url| url.path().rsplit_once('/').map(|(path, _)| path))
                        .filter(|path| !path.is_empty()).unwrap_or("/").to_string();
                    for attribute in pair.split(';').skip(1) {
                        if let Some((name, value)) = attribute.trim().split_once('=') {
                            if name.eq_ignore_ascii_case("domain") { domain = value.trim().to_string(); }
                            if name.eq_ignore_ascii_case("path") && value.trim().starts_with('/') { path = value.trim().to_string(); }
                        }
                    }
                    (domain, path)
                } else { continue; };
                if !Protected::default().cookie_scope(&domain) {
                    scopes.push(json!({ "header": if response { "set-cookie" } else { "cookie" }, "name": name, "domain": domain, "path": path }));
                }
            }
        }
    }
    scopes
}

pub fn saved_scopes(scopes: &[Value]) -> Vec<Value> {
    scopes.iter().filter(|scope| matches!(field(scope, "header"), "cookie" | "set-cookie") && !Protected::default().cookie_scope(field(scope, "domain")))
        .map(|scope| json!({ "header": field(scope, "header"), "name": field(scope, "name"), "domain": field(scope, "domain"), "path": field(scope, "path") })).collect()
}

pub fn hydrate(cdp: &mut Cdp, target: &str, request: &mut NetRequest, command: &str) -> Result<bool, String> {
    if request.cookie_scopes.is_empty() || !matches!(command, "request" | "request-headers" | "response-headers") {
        return Ok(false);
    }
    let session = cdp::attach(cdp, target)?;
    let result = cdp.call("Network.getAllCookies", json!({}), Some(&session));
    cdp::detach(cdp, &session);
    Ok(fill(request, &result?, command))
}

pub fn fill(request: &mut NetRequest, jar: &Value, command: &str) -> bool {
    let cookies = jar.get("cookies").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut restored = false;
    for (headers, response) in [(&mut request.request_headers, false), (&mut request.response_headers, true)] {
        if command == if response { "request-headers" } else { "response-headers" } { continue; }
        for (header, text) in headers {
            if !header.eq_ignore_ascii_case(if response { "set-cookie" } else { "cookie" }) { continue; }
            let mut values = Vec::new();
            for scope in request.cookie_scopes.iter().filter(|scope| field(scope, "header").eq_ignore_ascii_case(header)) {
                if Protected::default().cookie_scope(field(scope, "domain")) { continue; }
                let cookie = cookies.iter().find(|cookie| {
                    !Protected::default().cookie_scope(field(cookie, "domain"))
                        && field(cookie, "name") == field(scope, "name") && field(cookie, "path") == field(scope, "path")
                        && field(cookie, "domain") == field(scope, "domain")
                        && cookie.get("partitionKey").is_none()
                });
                let value = cookie.map(|cookie| field(cookie, "value")).unwrap_or("[redacted]");
                restored |= cookie.is_some();
                values.push(if response {
                    format!("{}={value}; Domain={}; Path={}", field(scope, "name"), field(scope, "domain"), field(scope, "path"))
                } else { format!("{}={value}", field(scope, "name")) });
            }
            *text = if values.is_empty() { "[redacted]".to_string() } else { values.join(if response { "\n" } else { "; " }) };
        }
    }
    restored
}
