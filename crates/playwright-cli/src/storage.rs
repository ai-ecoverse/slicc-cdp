use crate::args::{Invocation, Output};
use crate::browser;
use crate::cdp::{self, Cdp};
use crate::cookies;
use crate::sanitize;
use crate::session::Session;
use cdp_client::fetch::{self, Decision, Fulfill, Paused, Pattern};
use cdp_client::protected::Protected;
use cdp_client::shield;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const WEB: &[&str] = &[
    "localstorage-list", "localstorage-get", "localstorage-set", "localstorage-delete", "localstorage-clear",
    "sessionstorage-list", "sessionstorage-get", "sessionstorage-set", "sessionstorage-delete", "sessionstorage-clear",
];
const COVERAGE: &str = "__pwCoverage.";
const WORLD: &str = "slicc-storage";
const MAX_EXPIRES: f64 = 253402300799.0;
const BLANK_DOCUMENT: &str = "<html></html>";

const WEB_SCRIPT: &str = r#"(request) => {
  const origin = location.origin;
  const storage = request.kind === 'local' ? localStorage : sessionStorage;
  if (request.op === 'list') {
    const items = [];
    for (let i = 0; i < storage.length; i++) {
      const name = storage.key(i);
      if (name !== null)
        items.push({ name, value: storage.getItem(name) ?? '' });
    }
    return { origin, items };
  }
  if (request.op === 'get')
    return { origin, value: storage.getItem(request.key) };
  if (request.op === 'set')
    storage.setItem(request.key, request.value);
  else if (request.op === 'delete')
    storage.removeItem(request.key);
  else
    storage.clear();
  return { origin };
}"#;

const RESTORE_SCRIPT: &str = r#"async (expected, items) => {
  const origin = location.origin;
  if (origin !== expected)
    return { origin };
  const registrations = navigator.serviceWorker ? await navigator.serviceWorker.getRegistrations() : [];
  await Promise.all(registrations.map((registration) => registration.unregister().catch(() => {})));
  try {
    for (const database of (await indexedDB.databases?.()) || []) {
      if (database.name)
        indexedDB.deleteDatabase(database.name);
    }
  } catch (error) {
    throw new Error('Unable to restore IndexedDB: ' + error.message);
  }
  sessionStorage.clear();
  localStorage.clear();
  for (const { name, value } of items)
    localStorage.setItem(name, value);
  return { origin };
}"#;

pub fn is_command(command: &str) -> bool {
    WEB.contains(&command) || matches!(command, "state-save" | "state-load")
}

pub fn precheck(command: &str, invocation: &Invocation) -> Option<String> {
    if !is_command(command) || command == "state-save" {
        return None;
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
    let result = match command {
        "state-save" => save(cdp, state, invocation),
        "state-load" => load(cdp, invocation),
        _ => web(cdp, state, invocation, command),
    };
    match result {
        Ok(text) => Output::ok(sanitize::terminal(&text)),
        Err(message) => Output::err(message),
    }
}

fn web(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation, command: &str) -> Result<String, String> {
    let (area, op) = command.split_once('-').unwrap_or((command, ""));
    let (kind, label) = if area == "localstorage" { ("local", "localStorage") } else { ("session", "sessionStorage") };
    let key = invocation.positionals.first().map(String::as_str).unwrap_or("");
    let value = invocation.positionals.get(1).map(String::as_str).unwrap_or("");
    let page = cookies::target(cdp, invocation, state)?;
    let session = cdp::attach(cdp, &page.target_id)?;
    let request = json!({ "kind": kind, "op": op, "key": key, "value": value });
    let result = evaluate(cdp, &session, &format!("({WEB_SCRIPT})({request})"));
    cdp::detach(cdp, &session);
    let result = admitted(cdp.guard(), result?, None)?;
    Ok(match op {
        "list" => {
            let lines: Vec<String> = items(&result).into_iter().map(|(name, value)| format!("{name}={value}")).collect();
            if lines.is_empty() { format!("No {label} items found\n") } else { format!("{}\n", lines.join("\n")) }
        }
        "get" => match result.get("value").and_then(Value::as_str) {
            Some(found) => format!("{key}={found}\n"),
            None => format!("{label} key '{key}' not found\n"),
        },
        "set" => cookies::code(&format!("await page.{label}.setItem({}, {});", cookies::quoted(key), cookies::quoted(value))),
        "delete" => cookies::code(&format!("await page.{label}.removeItem({});", cookies::quoted(key))),
        _ => cookies::code(&format!("await page.{label}.clear();")),
    })
}

fn evaluate(cdp: &mut Cdp, session: &str, expression: &str) -> Result<Value, String> {
    let tree = cdp.call("Page.getFrameTree", json!({}), Some(session))?;
    let frame = tree.pointer("/frameTree/frame/id").and_then(Value::as_str).map(str::to_string);
    let frame = frame.ok_or_else(|| format!("{}\n", cdp.guard().refusal()))?;
    let world = cdp.call("Page.createIsolatedWorld", json!({ "frameId": frame, "worldName": WORLD }), Some(session))?;
    let context = world.get("executionContextId").cloned().ok_or_else(|| format!("{}\n", cdp.guard().refusal()))?;
    let result = cdp.call(
        "Runtime.evaluate",
        json!({ "expression": expression, "contextId": context, "returnByValue": true, "awaitPromise": true }),
        Some(session),
    )?;
    if let Some(details) = result.get("exceptionDetails") {
        let text = details.pointer("/exception/description").or_else(|| details.get("text")).and_then(Value::as_str);
        return Err(format!("Error: {}\n", sanitize::browser_message(text.unwrap_or("evaluation failed"))));
    }
    Ok(result.pointer("/result/value").cloned().unwrap_or(Value::Null))
}

fn admitted(guard: &Protected, result: Value, expected: Option<&str>) -> Result<Value, String> {
    let origin = result.get("origin").and_then(Value::as_str).unwrap_or("");
    if web_origin(guard, origin).as_deref() != Some(origin) || expected.is_some_and(|expected| expected != origin) {
        return Err(format!("{}\n", guard.refusal_for_url(origin)));
    }
    Ok(result)
}

fn web_origin(guard: &Protected, text: &str) -> Option<String> {
    let url = url::Url::parse(text).ok().filter(|url| matches!(url.scheme(), "http" | "https"))?;
    let origin = url.origin().ascii_serialization();
    (origin != "null" && !guard.url(text) && !guard.url(&origin) && !guard.storage_key(&origin)).then_some(origin)
}

fn items(result: &Value) -> Vec<(String, String)> {
    result.get("items").and_then(Value::as_array).into_iter().flatten().filter_map(|item| {
        Some((item.get("name")?.as_str()?.to_string(), item.get("value")?.as_str()?.to_string()))
    }).collect()
}

fn save(cdp: &mut Cdp, state: &mut Session, invocation: &Invocation) -> Result<String, String> {
    let page = cookies::target(cdp, invocation, state)?;
    let session = cdp::attach(cdp, &page.target_id)?;
    let jar = cdp.call("Network.getAllCookies", json!({}), Some(&session));
    cdp::detach(cdp, &session);
    let jar = jar?;
    let saved: Vec<Json> = jar.get("cookies").and_then(Value::as_array).into_iter().flatten()
        .filter(|cookie| !shield::cookie_protected(cdp.guard(), cookie))
        .map(playwright_cookie)
        .collect();
    let mut origins = Vec::new();
    let mut done = BTreeSet::new();
    for page in browser::list_pages(cdp)? {
        let Some(origin) = web_origin(cdp.guard(), &page.url) else { continue };
        if done.contains(&origin) {
            continue;
        }
        match collect(cdp, &page.target_id, &origin) {
            Ok(found) => {
                done.insert(origin.clone());
                if !found.is_empty() {
                    let entries = found.into_iter().map(|(name, value)| Json::Object(vec![("name", text(&name)), ("value", text(&value))])).collect();
                    origins.push(Json::Object(vec![("origin", text(&origin)), ("localStorage", Json::Array(entries))]));
                }
            }
            Err(message) if cdp::transport_error(&message) => return Err(message),
            Err(_) => {}
        }
    }
    let document = Json::Object(vec![("cookies", Json::Array(saved)), ("origins", Json::Array(origins))]);
    let path = match invocation.positionals.first().filter(|name| !name.is_empty()) {
        Some(name) => PathBuf::from(name),
        None => PathBuf::from(".playwright-cli").join(format!("storage-state-{}.json", stamp(now_millis()))),
    };
    write_private(&path, &document.pretty())?;
    let shown = shown(&path.display().to_string());
    Ok(format!(
        "### Result\n- [Storage state]({shown})\n{}",
        cookies::code(&format!("await page.context().storageState({{ path: {} }});", cookies::quoted(&shown)))
    ))
}

fn collect(cdp: &mut Cdp, target: &str, origin: &str) -> Result<Vec<(String, String)>, String> {
    let session = cdp::attach(cdp, target)?;
    let request = json!({ "kind": "local", "op": "list" });
    let result = evaluate(cdp, &session, &format!("({WEB_SCRIPT})({request})"));
    cdp::detach(cdp, &session);
    let result = admitted(cdp.guard(), result?, Some(origin))?;
    Ok(items(&result).into_iter().filter(|(name, _)| !name.starts_with(COVERAGE)).collect())
}

fn playwright_cookie(cookie: &Value) -> Json {
    let string = |key: &str| text(cookie.get(key).and_then(Value::as_str).unwrap_or(""));
    let truth = |key: &str| Json::Raw(cookie.get(key).and_then(Value::as_bool).unwrap_or(false).to_string());
    let mut fields = vec![
        ("name", string("name")),
        ("value", string("value")),
        ("domain", string("domain")),
        ("path", string("path")),
        ("expires", Json::Raw(cookies::js_value(cookie.get("expires").unwrap_or(&json!(-1))))),
        ("httpOnly", truth("httpOnly")),
        ("secure", truth("secure")),
        ("sameSite", text(cookie.get("sameSite").and_then(Value::as_str).unwrap_or("Lax"))),
    ];
    if let Some(key) = cookie.get("partitionKey").filter(|key| key.is_object()) {
        if let Some(ancestor) = key.get("hasCrossSiteAncestor").and_then(Value::as_bool) {
            fields.push(("_crHasCrossSiteAncestor", Json::Raw(ancestor.to_string())));
        }
        if let Some(site) = key.get("topLevelSite").and_then(Value::as_str) {
            fields.push(("partitionKey", text(site)));
        }
    }
    Json::Object(fields)
}

fn shown(path: &str) -> String {
    if path.starts_with('/') || path.starts_with('.') || path.contains('/') || path.contains('\\') {
        path.to_string()
    } else {
        format!("./{path}")
    }
}

fn write_private(path: &Path, contents: &str) -> Result<(), String> {
    let fail = |err: std::io::Error| format!("Error: {}: {err}\n", path.display());
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(fail)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).map_err(fail)?;
    #[cfg(unix)]
    file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600)).map_err(fail)?;
    file.write_all(contents.as_bytes()).map_err(fail)
}

fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as i64).unwrap_or(0)
}

fn stamp(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let rest = millis.rem_euclid(86_400_000);
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}-{:02}-{:02}-{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000
    )
}

struct Plan {
    cookies: Vec<Value>,
    origins: Vec<(String, Vec<(String, String)>)>,
}

fn load(cdp: &mut Cdp, invocation: &Invocation) -> Result<String, String> {
    let filename = invocation.positionals.first().cloned().unwrap_or_default();
    let unreadable = |err: String| format!("Error: Error reading storage state from {filename}:\n{err}\n");
    let contents = fs::read_to_string(&filename).map_err(|err| unreadable(err.to_string()))?;
    let parsed: Value = serde_json::from_str(&contents).map_err(|err| unreadable(err.to_string()))?;
    let plan = plan(cdp.guard(), &parsed)?;
    let created = cdp.call("Target.createTarget", json!({ "url": "about:blank", "background": true }), None)?;
    let target = created.get("targetId").and_then(Value::as_str).map(str::to_string)
        .ok_or_else(|| "Target.createTarget returned no targetId\n".to_string())?;
    let restored = match cdp::attach(cdp, &target) {
        Ok(session) => {
            let restored = restore(cdp, &session, &plan);
            if cdp.has_decider(&session) {
                fetch::disable(cdp, &session);
                cdp.set_decider(&session, None);
            }
            cdp::detach(cdp, &session);
            restored
        }
        Err(message) => Err(message),
    };
    let closed = browser::close_target(cdp, &target);
    restored?;
    closed?;
    Ok(format!(
        "### Result\nStorage state restored from {filename}\n{}",
        cookies::code(&format!("await page.context().setStorageState({});", cookies::quoted(&filename)))
    ))
}

fn restore(cdp: &mut Cdp, session: &str, plan: &Plan) -> Result<(), String> {
    let jar = cdp.call("Network.getAllCookies", json!({}), Some(session))?;
    for cookie in jar.get("cookies").and_then(Value::as_array).into_iter().flatten() {
        cookies::delete(cdp, session, cookie)?;
    }
    for cookie in &plan.cookies {
        let result = cdp.call("Network.setCookie", cookie.clone(), Some(session))?;
        if result.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(invalid("Invalid cookie fields"));
        }
    }
    if plan.origins.is_empty() {
        return Ok(());
    }
    cdp.call("Page.enable", json!({}), Some(session))?;
    let patterns: Vec<Pattern> = plan.origins.iter().map(|(origin, _)| Pattern::request(format!("{origin}/*"))).collect();
    fetch::enable(cdp, session, &patterns)?;
    cdp.set_decider(session, Some(Box::new(|_: &Paused| Decision::Fulfill(Fulfill::html(200, BLANK_DOCUMENT)))));
    for (origin, entries) in &plan.origins {
        let mark = cdp.events().len();
        let navigated = cdp.call("Page.navigate", json!({ "url": origin }), Some(session))?;
        if let Some(error) = navigated.get("errorText").and_then(Value::as_str).filter(|error| !error.is_empty()) {
            return Err(invalid(error));
        }
        cdp.wait_load_since(session, mark)?;
        let list: Vec<Value> = entries.iter().map(|(name, value)| json!({ "name": name, "value": value })).collect();
        let result = evaluate(cdp, session, &format!("({RESTORE_SCRIPT})({}, {})", json!(origin), Value::Array(list)))?;
        admitted(cdp.guard(), result, Some(origin))?;
    }
    Ok(())
}

fn invalid(message: &str) -> String {
    format!("Error: Error setting storage state:\n{message}\n")
}

fn plan(guard: &Protected, state: &Value) -> Result<Plan, String> {
    let object = state.as_object().ok_or_else(|| invalid("storageState: expected object"))?;
    if object.get("credentials").and_then(Value::as_array).is_some_and(|list| !list.is_empty()) {
        return Err(invalid("storageState.credentials: restoring credentials is not supported"));
    }
    let mut cookies = Vec::new();
    for (index, cookie) in list(object.get("cookies"), "storageState.cookies")?.iter().enumerate() {
        cookies.push(cookie_params(guard, cookie, &format!("storageState.cookies[{index}]"))?);
    }
    let mut origins = Vec::new();
    for (index, entry) in list(object.get("origins"), "storageState.origins")?.iter().enumerate() {
        let at = format!("storageState.origins[{index}]");
        let origin = entry.get("origin").and_then(Value::as_str).ok_or_else(|| invalid(&format!("{at}.origin: expected string")))?;
        for key in ["indexedDB", "opfs"] {
            if entry.get(key).is_some_and(|value| !value.is_null() && value.as_array().is_none_or(|list| !list.is_empty())) {
                return Err(invalid(&format!("{at}.{key}: restoring {key} is not supported")));
            }
        }
        let Some(normalized) = web_origin(guard, origin) else {
            return Err(format!("{}\n", guard.refusal_for_url(origin)));
        };
        let mut entries = Vec::new();
        for (item, pair) in list(entry.get("localStorage"), &format!("{at}.localStorage"))?.iter().enumerate() {
            let field = |key: &str| pair.get(key).and_then(Value::as_str).map(str::to_string)
                .ok_or_else(|| invalid(&format!("{at}.localStorage[{item}].{key}: expected string")));
            entries.push((field("name")?, field("value")?));
        }
        origins.push((normalized, entries));
    }
    Ok(Plan { cookies, origins })
}

fn list<'a>(value: Option<&'a Value>, at: &str) -> Result<&'a [Value], String> {
    match value {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(invalid(&format!("{at}: expected array"))),
    }
}

fn cookie_params(guard: &Protected, cookie: &Value, at: &str) -> Result<Value, String> {
    let string = |key: &str| match cookie.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(invalid(&format!("{at}.{key}: expected string"))),
    };
    let boolean = |key: &str| match cookie.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(flag)) => Ok(Some(*flag)),
        Some(_) => Err(invalid(&format!("{at}.{key}: expected boolean"))),
    };
    let name = string("name")?.ok_or_else(|| invalid(&format!("{at}.name: expected string")))?;
    let value = string("value")?.ok_or_else(|| invalid(&format!("{at}.value: expected string")))?;
    let url = string("url")?;
    let mut domain = string("domain")?;
    let mut path = string("path")?;
    let mut secure = boolean("secure")?;
    let http_only = boolean("httpOnly")?;
    let expires = match cookie.get("expires") {
        None | Some(Value::Null) => None,
        Some(number) => Some(number.as_f64().ok_or_else(|| invalid(&format!("{at}.expires: expected number")))?),
    };
    let same_site = string("sameSite")?;
    if same_site.as_deref().is_some_and(|text| !matches!(text, "Strict" | "Lax" | "None")) {
        return Err(invalid(&format!("{at}.sameSite: expected one of (Strict|Lax|None)")));
    }
    let partition = string("partitionKey")?;
    let ancestor = boolean("_crHasCrossSiteAncestor")?;
    if url.is_none() && (domain.is_none() || path.is_none()) {
        return Err(invalid("Cookie should have a url or a domain/path pair"));
    }
    if url.is_some() && domain.is_some() {
        return Err(invalid("Cookie should have either url or domain"));
    }
    if url.is_some() && path.is_some() {
        return Err(invalid("Cookie should have either url or path"));
    }
    if expires.is_some_and(|expires| expires != 0.0 && (expires < 0.0 && expires != -1.0 || expires > MAX_EXPIRES)) {
        return Err(invalid("Cookie should have a valid expires, only -1 or a positive number for the unix timestamp in seconds is allowed"));
    }
    if let Some(text) = &url {
        if text == "about:blank" {
            return Err(invalid(&format!("Blank page can not have cookie \"{name}\"")));
        }
        if text.starts_with("data:") {
            return Err(invalid(&format!("Data URL page can not have cookie \"{name}\"")));
        }
        if guard.url(text) {
            return Err(cdp::COOKIES.to_string());
        }
        let parsed = url::Url::parse(text).map_err(|err| invalid(&format!("Invalid URL: {err}")))?;
        let pathname = parsed.path();
        domain = Some(parsed.host_str().unwrap_or("").to_string());
        path = Some(pathname[..pathname.rfind('/').map_or(0, |at| at + 1)].to_string());
        secure = Some(parsed.scheme() == "https");
    }
    let mut params = json!({ "name": name, "value": value, "domain": domain, "path": path });
    if let Some(expires) = expires.filter(|expires| *expires != -1.0) {
        params["expires"] = json!(expires);
    }
    if let Some(flag) = http_only {
        params["httpOnly"] = json!(flag);
    }
    if let Some(flag) = secure {
        params["secure"] = json!(flag);
    }
    if let Some(text) = same_site {
        params["sameSite"] = json!(text);
    }
    if let Some(site) = partition {
        params["partitionKey"] = json!({ "topLevelSite": site, "hasCrossSiteAncestor": ancestor.unwrap_or(true) });
    }
    if shield::cookie_protected(guard, &params) {
        return Err(cdp::COOKIES.to_string());
    }
    Ok(params)
}

fn text(value: &str) -> Json {
    Json::Raw(Value::String(value.to_string()).to_string())
}

enum Json {
    Raw(String),
    Array(Vec<Json>),
    Object(Vec<(&'static str, Json)>),
}

impl Json {
    fn pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn write(&self, out: &mut String, depth: usize) {
        let inner = "  ".repeat(depth + 1);
        let close = "  ".repeat(depth);
        match self {
            Json::Raw(text) => out.push_str(text),
            Json::Array(items) if items.is_empty() => out.push_str("[]"),
            Json::Object(fields) if fields.is_empty() => out.push_str("{}"),
            Json::Array(items) => {
                out.push_str("[\n");
                for (index, item) in items.iter().enumerate() {
                    out.push_str(&inner);
                    item.write(out, depth + 1);
                    out.push_str(if index + 1 < items.len() { ",\n" } else { "\n" });
                }
                out.push_str(&close);
                out.push(']');
            }
            Json::Object(fields) => {
                out.push_str("{\n");
                for (index, (key, item)) in fields.iter().enumerate() {
                    out.push_str(&format!("{inner}\"{key}\": "));
                    item.write(out, depth + 1);
                    out.push_str(if index + 1 < fields.len() { ",\n" } else { "\n" });
                }
                out.push_str(&close);
                out.push('}');
            }
        }
    }
}
