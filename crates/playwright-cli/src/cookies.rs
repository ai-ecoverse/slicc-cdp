use crate::args::{Invocation, Output};
use crate::browser::{self, PageInfo};
use crate::cdp::{self, Cdp};
use crate::commands::resolve_target;
use crate::sanitize;
use crate::session::Session;
use serde_json::{json, Value};

const COMMANDS: &[&str] = &["cookie-list", "cookie-get", "cookie-set", "cookie-delete", "cookie-clear"];
const MAX_EXPIRES: f64 = 253402300799.0;
const SAME_SITE: &[&str] = &["Strict", "Lax", "None"];
const NO_PAIR: &str = "Error: Cookie should have a url or a domain/path pair\n";
const BAD_EXPIRES: &str = "Error: Cookie should have a valid expires, only -1 or a positive number for the unix timestamp in seconds is allowed\n";

pub fn is_command(command: &str) -> bool {
    COMMANDS.contains(&command)
}

pub fn precheck(command: &str, invocation: &Invocation) -> Option<String> {
    if !is_command(command) {
        return None;
    }
    if command == "cookie-set" {
        if let Err(message) = set_options(invocation) {
            return Some(message);
        }
    }
    let spec = crate::spec::lookup(command)?;
    let missing: Vec<String> = spec
        .args
        .iter()
        .skip(invocation.positionals.len())
        .map(|name| format!("error: '{name}' argument: expected string, received undefined\n"))
        .collect();
    (!missing.is_empty()).then(|| missing.concat())
}

pub fn run(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation, command: &str) -> Output {
    match execute(cdp, state, invocation, command) {
        Ok(text) => Output::ok(sanitize::terminal(&text)),
        Err(message) => Output::err(message),
    }
}

fn execute(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation, command: &str) -> Result<String, String> {
    if matches!(command, "cookie-list" | "cookie-set") && invocation.flags.get("domain").is_some_and(|domain| cdp.guard().cookie_scope(domain)) {
        return Err(cdp::COOKIES.to_string());
    }
    let page = target(cdp, invocation, state)?;
    let mut set = None;
    if command == "cookie-set" {
        let domain = option(invocation, "domain").map(str::to_string).unwrap_or_else(|| hostname(&page.url));
        if domain.is_empty() {
            return Err(NO_PAIR.to_string());
        }
        if cdp.guard().cookie_scope(&domain) {
            return Err(cdp::COOKIES.to_string());
        }
        set = Some(domain);
    }
    let session = cdp::attach(cdp, &page.target_id)?;
    let result = match set {
        Some(domain) => cookie_set(cdp, &session, invocation, domain),
        None => with_jar(cdp, &session, invocation, command),
    };
    cdp::detach(cdp, &session);
    result
}

fn with_jar(cdp: &mut Cdp, session: &str, invocation: &Invocation, command: &str) -> Result<String, String> {
    let jar = cdp.call("Network.getAllCookies", json!({}), Some(session))?;
    let jar = jar.get("cookies").and_then(Value::as_array).cloned().unwrap_or_default();
    let name = invocation.positionals.first().map(String::as_str).unwrap_or("");
    match command {
        "cookie-list" => Ok(list(&jar, option(invocation, "domain"), option(invocation, "path"))),
        "cookie-get" => Ok(get(&jar, name)),
        "cookie-delete" => {
            for cookie in jar.iter().filter(|cookie| field(cookie, "name") == name) {
                delete(cdp, session, cookie)?;
            }
            Ok(code(&format!("await page.context().clearCookies({{ name: {} }});", quoted(name))))
        }
        _ => {
            for cookie in &jar {
                delete(cdp, session, cookie)?;
            }
            Ok(code("await page.context().clearCookies();"))
        }
    }
}

fn list(jar: &[Value], domain: Option<&str>, path: Option<&str>) -> String {
    let lines: Vec<String> = jar
        .iter()
        .filter(|cookie| domain.is_none_or(|domain| field(cookie, "domain").contains(domain)))
        .filter(|cookie| path.is_none_or(|path| field(cookie, "path").starts_with(path)))
        .map(|cookie| {
            format!(
                "{}={} (domain: {}, path: {})",
                field(cookie, "name"),
                field(cookie, "value"),
                field(cookie, "domain"),
                field(cookie, "path")
            )
        })
        .collect();
    if lines.is_empty() {
        return "No cookies found\n".to_string();
    }
    format!("{}\n", lines.join("\n"))
}

fn get(jar: &[Value], name: &str) -> String {
    let Some(cookie) = jar.iter().find(|cookie| field(cookie, "name") == name) else {
        return format!("Cookie '{name}' not found\n");
    };
    format!(
        "{}={} (domain: {}, path: {}, httpOnly: {}, secure: {}, sameSite: {})\n",
        field(cookie, "name"),
        field(cookie, "value"),
        field(cookie, "domain"),
        field(cookie, "path"),
        truth(cookie, "httpOnly"),
        truth(cookie, "secure"),
        cookie.get("sameSite").and_then(Value::as_str).unwrap_or("Lax")
    )
}

fn delete(cdp: &mut Cdp, session: &str, cookie: &Value) -> Result<(), String> {
    let mut params = json!({
        "name": field(cookie, "name"),
        "domain": field(cookie, "domain"),
        "path": field(cookie, "path"),
    });
    if let Some(key) = cookie.get("partitionKey") {
        params["partitionKey"] = key.clone();
    }
    cdp.call("Network.deleteCookies", params, Some(session))?;
    Ok(())
}

fn cookie_set(cdp: &mut Cdp, session: &str, invocation: &Invocation, domain: String) -> Result<String, String> {
    let mut fields = vec![
        ("name", json!(invocation.positionals[0])),
        ("value", json!(invocation.positionals[1])),
        ("domain", json!(domain)),
        ("path", json!(option(invocation, "path").unwrap_or("/"))),
    ];
    fields.extend(set_options(invocation)?);
    let params: serde_json::Map<String, Value> = fields.iter().map(|(key, value)| (key.to_string(), value.clone())).collect();
    let result = cdp.call("Network.setCookie", Value::Object(params), Some(session))?;
    if result.get("success").and_then(Value::as_bool) == Some(false) {
        return Err("Error: Invalid cookie fields\n".to_string());
    }
    let object: Vec<String> = fields.iter().map(|(key, value)| format!("\"{key}\":{}", js_value(value))).collect();
    Ok(code(&format!("await page.context().addCookies([{{{}}}]);", object.join(","))))
}

fn set_options(invocation: &Invocation) -> Result<Vec<(&'static str, Value)>, String> {
    let mut fields = Vec::new();
    let mut errors = String::new();
    let mut invalid = false;
    if let Some(text) = invocation.flags.get("expires") {
        match js_number(text) {
            Some(expires) => {
                invalid = expires != 0.0 && (expires < 0.0 && expires != -1.0 || expires > MAX_EXPIRES);
                fields.push(("expires", json!(expires)));
            }
            None => errors.push_str(&format!("error: '--expires' option: expected number, received '{text}'\n")),
        }
    }
    for name in ["httpOnly", "secure"] {
        if let Some(text) = invocation.flags.get(name) {
            fields.push((name, json!(text != "false")));
        }
    }
    if let Some(text) = invocation.flags.get("sameSite") {
        if SAME_SITE.contains(&text.as_str()) {
            fields.push(("sameSite", json!(text)));
        } else {
            errors.push_str("error: '--sameSite' option: Invalid option: expected one of \"Strict\"|\"Lax\"|\"None\"\n");
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    if invalid {
        return Err(BAD_EXPIRES.to_string());
    }
    Ok(fields)
}

fn js_number(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return Some(0.0);
    }
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        if text.len() > 2 && text[..2].eq_ignore_ascii_case(prefix) {
            return u64::from_str_radix(&text[2..], radix).ok().map(|value| value as f64);
        }
    }
    match text {
        "Infinity" | "+Infinity" => return Some(f64::INFINITY),
        "-Infinity" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    if !text.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.' | b'e' | b'E')) {
        return None;
    }
    text.parse::<f64>().ok()
}

fn js_value(value: &Value) -> String {
    match value.as_f64().filter(|_| value.is_number()) {
        Some(number) if number.fract() == 0.0 => format!("{}", number as i64),
        Some(number) => format!("{number}"),
        None => value.to_string(),
    }
}

fn quoted(text: &str) -> String {
    let json = Value::String(text.to_string()).to_string();
    let inner = json[1..json.len() - 1].replace("\\\"", "\"").replace('\'', "\\'");
    format!("'{inner}'")
}

fn code(line: &str) -> String {
    format!("### Ran Playwright code\n```js\n{line}\n```\n")
}

fn target(cdp: &mut Cdp, invocation: &Invocation, state: &mut Session) -> Result<PageInfo, String> {
    let pages = browser::list_pages(cdp)?;
    let id = match pages.first() {
        Some(page) if !invocation.flags.contains_key("tab") && state.current.is_none() => page.target_id.clone(),
        _ => resolve_target(cdp, invocation, state, &pages, false)?,
    };
    Ok(pages.into_iter().find(|page| page.target_id == id).unwrap_or(PageInfo { target_id: id, url: String::new(), title: String::new() }))
}

fn hostname(url: &str) -> String {
    url::Url::parse(url).ok().and_then(|url| url.host_str().map(str::to_string)).unwrap_or_default()
}

fn option<'a>(invocation: &'a Invocation, name: &str) -> Option<&'a str> {
    invocation.flags.get(name).map(String::as_str).filter(|value| !value.is_empty())
}

fn field<'a>(cookie: &'a Value, key: &str) -> &'a str {
    cookie.get(key).and_then(Value::as_str).unwrap_or("")
}

fn truth(cookie: &Value, key: &str) -> bool {
    cookie.get(key).and_then(Value::as_bool).unwrap_or(false)
}
