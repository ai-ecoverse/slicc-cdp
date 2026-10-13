use crate::args::{Invocation, Output};
use crate::cdp::Cdp;
use crate::net::{base64_decode, base64_encode};
use crate::session::{self, Session};
use cdp_client::fetch::{self, Decision, Fulfill, Overrides, Pattern, Paused};
use cdp_client::protected::Protected;
use serde_json::{json, Value};

const MAX_FILE: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Route {
    pub pattern: String,
    pub status: Option<u16>,
    pub body: Option<Vec<u8>>,
    pub body_file: Option<String>,
    pub content_type: Option<String>,
    pub add_headers: Vec<(String, String)>,
    pub remove_headers: Vec<String>,
}

impl Route {
    fn fulfils(&self) -> bool {
        self.body.is_some() || self.status.is_some()
    }

    pub fn matches(&self, url: &str) -> bool {
        Glob::parse(&resolve(&self.pattern))
            .map(|glob| glob.matches(url))
            .unwrap_or(false)
    }

    pub fn decision(&self, paused: &Paused) -> Decision {
        if self.fulfils() {
            let body = self.body.clone().unwrap_or_default();
            let mut response = Fulfill::new(self.status.unwrap_or(200));
            let content_type = self.content_type.clone().or_else(|| {
                self.body_file
                    .as_deref()
                    .and_then(mime_for)
                    .map(str::to_string)
            });
            if let Some(kind) = content_type {
                response = response.header("Content-Type", kind);
            }
            for (name, value) in &self.add_headers {
                response
                    .headers
                    .retain(|(have, _)| !have.eq_ignore_ascii_case(name));
                response = response.header(name.clone(), value.clone());
            }
            if !response
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            {
                response = response.header("Content-Length", body.len().to_string());
            }
            return Decision::Fulfill(response.body(body));
        }
        if self.add_headers.is_empty() && self.remove_headers.is_empty() {
            return Decision::default();
        }
        let mut headers = paused.headers.clone();
        for (name, value) in &self.add_headers {
            headers.retain(|(have, _)| !have.eq_ignore_ascii_case(name));
            headers.push((name.clone(), value.clone()));
        }
        headers.retain(|(have, _)| {
            !self
                .remove_headers
                .iter()
                .any(|name| name.eq_ignore_ascii_case(have))
        });
        Decision::Continue(Overrides {
            headers: Some(headers),
            ..Overrides::default()
        })
    }

    fn describe(&self) -> String {
        let mut details = Vec::new();
        if let Some(status) = self.status {
            details.push(format!("status={status}"));
        }
        if let Some(file) = &self.body_file {
            details.push(format!("file={file}"));
        } else if let Some(body) = &self.body {
            let text = String::from_utf8_lossy(body);
            let shown: String = if text.chars().count() > 50 {
                format!("{}...", text.chars().take(50).collect::<String>())
            } else {
                text.to_string()
            };
            details.push(format!("body={shown}"));
        }
        if let Some(kind) = &self.content_type {
            details.push(format!("contentType={kind}"));
        }
        if !self.add_headers.is_empty() {
            let pairs: Vec<String> = self
                .add_headers
                .iter()
                .map(|(name, value)| format!("{}:{}", json!(name), json!(value)))
                .collect();
            details.push(format!("addHeaders={{{}}}", pairs.join(",")));
        }
        if !self.remove_headers.is_empty() {
            details.push(format!("removeHeaders={}", self.remove_headers.join(",")));
        }
        if details.is_empty() {
            self.pattern.clone()
        } else {
            format!("{} ({})", self.pattern, details.join(", "))
        }
    }
}

pub fn decide(routes: &[Route], paused: &Paused) -> Decision {
    routes
        .iter()
        .rev()
        .find(|route| route.matches(&paused.url))
        .map(|route| route.decision(paused))
        .unwrap_or_default()
}

pub fn is_route_command(command: &str) -> bool {
    matches!(command, "route" | "route-list" | "unroute")
}

pub fn run(invocation: &Invocation) -> Output {
    let command = invocation.command.as_deref().unwrap_or("");
    let mut state = session::load();
    let output = match command {
        "route" => add(&mut state, invocation),
        "route-list" => return list(&state),
        "unroute" => remove(&mut state, invocation),
        _ => return Output::err(format!("playwright-cli {command}: not implemented yet\n")),
    };
    if output.code != 0 {
        return output;
    }
    match session::save(&state) {
        Ok(()) => output,
        Err(message) => Output::err(message),
    }
}

fn add(state: &mut Session, invocation: &Invocation) -> Output {
    match parse(invocation) {
        Ok(route) => {
            let pattern = route.pattern.clone();
            state.routes.push(route);
            Output::ok(format!("Route added for pattern: {pattern}\n"))
        }
        Err(message) => Output::err(message),
    }
}

pub fn parse(invocation: &Invocation) -> Result<Route, String> {
    let Some(pattern) = invocation
        .positionals
        .first()
        .filter(|text| !text.is_empty())
    else {
        return Err("playwright-cli route: pattern is required\n".into());
    };
    let guard = Protected::default();
    if guard.pattern(pattern) {
        return Err(format!("{}\n", guard.refusal()));
    }
    if let Err(message) = Glob::parse(&resolve(pattern)) {
        return Err(format!("playwright-cli route: {message}\n"));
    }
    let flags = &invocation.flags;
    let status = match flags.get("status") {
        None => None,
        Some(text) => match text.trim().parse::<u16>() {
            Ok(code) if (100..=599).contains(&code) => Some(code),
            _ => {
                return Err(format!(
                    "playwright-cli route: --status must be an HTTP status code, got \"{text}\"\n"
                ))
            }
        },
    };
    let mut body = flags.get("body").map(|text| text.as_bytes().to_vec());
    let body_file = flags.get("fulfill-file").cloned();
    if let Some(path) = &body_file {
        if body.is_some() {
            return Err("playwright-cli route: pass --body or --fulfill-file, not both\n".into());
        }
        body = Some(read_file(path)?);
    }
    let mut add_headers: Vec<(String, String)> = Vec::new();
    for raw in invocation.repeated("header") {
        let Some(colon) = raw.find(':').filter(|index| *index > 0) else {
            return Err(format!(
                "Invalid header \"{raw}\": expected \"Name: Value\"\n"
            ));
        };
        let name = raw[..colon].trim().to_string();
        let value = raw[colon + 1..].trim().to_string();
        if name.is_empty() {
            return Err(format!(
                "Invalid header \"{raw}\": expected \"Name: Value\"\n"
            ));
        }
        add_headers.retain(|(have, _)| !have.eq_ignore_ascii_case(&name));
        add_headers.push((name, value));
    }
    let remove_headers = flags
        .get("remove-header")
        .map(|text| {
            text.split(',')
                .map(|name| name.trim().to_ascii_lowercase())
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Ok(Route {
        pattern: pattern.clone(),
        status,
        body,
        body_file,
        content_type: flags.get("content-type").cloned(),
        add_headers,
        remove_headers,
    })
}

fn read_file(path: &str) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path)
        .map_err(|err| format!("playwright-cli route: --fulfill-file {path}: {err}\n"))?;
    if !meta.is_file() {
        return Err(format!(
            "playwright-cli route: --fulfill-file {path}: not a file\n"
        ));
    }
    if meta.len() > MAX_FILE {
        return Err(format!(
            "playwright-cli route: --fulfill-file {path}: larger than 16 MiB\n"
        ));
    }
    std::fs::read(path)
        .map_err(|err| format!("playwright-cli route: --fulfill-file {path}: {err}\n"))
}

fn list(state: &Session) -> Output {
    if state.routes.is_empty() {
        return Output::ok("No active routes\n");
    }
    let lines: Vec<String> = state
        .routes
        .iter()
        .enumerate()
        .map(|(index, route)| format!("{}. {}", index + 1, route.describe()))
        .collect();
    Output::ok(format!("{}\n", lines.join("\n")))
}

fn remove(state: &mut Session, invocation: &Invocation) -> Output {
    match invocation.positionals.first() {
        Some(pattern) => {
            let before = state.routes.len();
            state.routes.retain(|route| &route.pattern != pattern);
            let removed = before - state.routes.len();
            Output::ok(format!(
                "Removed {removed} route(s) for pattern: {pattern}\n"
            ))
        }
        None => {
            let removed = state.routes.len();
            state.routes.clear();
            Output::ok(format!("Removed all {removed} route(s)\n"))
        }
    }
}

pub fn arm(
    cdp: &mut Cdp,
    target_id: &str,
    session_id: &str,
    routes: &[Route],
) -> Result<bool, String> {
    if routes.is_empty() {
        return Ok(false);
    }
    if fetch::target_protected(cdp, target_id)? {
        return Ok(false);
    }
    let mut patterns: Vec<Pattern> = Vec::new();
    for route in routes {
        let pattern = Pattern::request(cdp_pattern(&resolve(&route.pattern)));
        if !patterns.contains(&pattern) {
            patterns.push(pattern);
        }
    }
    fetch::enable(cdp, session_id, &patterns)?;
    let owned = routes.to_vec();
    cdp.set_decider(
        session_id,
        Some(Box::new(move |paused: &Paused| decide(&owned, paused))),
    );
    Ok(true)
}

pub fn disarm(cdp: &mut Cdp, session_id: &str) {
    if cdp.has_decider(session_id) {
        fetch::disable(cdp, session_id);
    }
}

pub fn encode(route: &Route) -> Value {
    json!({
        "pattern": route.pattern,
        "status": route.status,
        "body": route.body.as_deref().map(base64_encode),
        "bodyFile": route.body_file,
        "contentType": route.content_type,
        "addHeaders": route
            .add_headers
            .iter()
            .map(|(name, value)| json!([name, value]))
            .collect::<Vec<_>>(),
        "removeHeaders": route.remove_headers,
    })
}

pub fn decode(value: &Value) -> Option<Route> {
    let pattern = value.get("pattern")?.as_str()?.to_string();
    if pattern.is_empty() || Protected::default().pattern(&pattern) {
        return None;
    }
    let text = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
    Some(Route {
        pattern,
        status: value
            .get("status")
            .and_then(|v| v.as_u64())
            .and_then(|code| u16::try_from(code).ok()),
        body: text("body").and_then(|data| base64_decode(&data).ok()),
        body_file: text("bodyFile"),
        content_type: text("contentType"),
        add_headers: value
            .get("addHeaders")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|pair| {
                        Some((
                            pair.get(0)?.as_str()?.to_string(),
                            pair.get(1)?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        remove_headers: value
            .get("removeHeaders")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn mime_for(path: &str) -> Option<&'static str> {
    let extension = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match extension.as_str() {
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "txt" => "text/plain",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        _ => return None,
    })
}

fn resolve(pattern: &str) -> String {
    if pattern.starts_with('*') {
        return pattern.to_string();
    }
    let Some((scheme, rest)) = pattern.split_once("://") else {
        return pattern.to_string();
    };
    if !rest.is_empty() && !rest.contains('/') && !rest.contains(['*', '?', '{', '}', '\\']) {
        return format!(
            "{}://{}/",
            scheme.to_ascii_lowercase(),
            rest.to_ascii_lowercase()
        );
    }
    pattern.to_string()
}

fn cdp_pattern(glob: &str) -> String {
    let chars: Vec<char> = glob.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    let wild = |out: &mut String| {
        if !out.ends_with('*') || out.ends_with("\\*") {
            out.push('*');
        }
    };
    while index < chars.len() {
        let ch = chars[index];
        if ch == '\\' && index + 1 < chars.len() {
            push_literal(&mut out, chars[index + 1]);
            index += 2;
            continue;
        }
        match ch {
            '*' => wild(&mut out),
            '{' => {
                wild(&mut out);
                while index < chars.len() && chars[index] != '}' {
                    index += 1;
                }
            }
            other => push_literal(&mut out, other),
        }
        index += 1;
    }
    out
}

fn push_literal(out: &mut String, ch: char) {
    if matches!(ch, '*' | '?' | '\\') {
        out.push('\\');
    }
    out.push(ch);
}

#[derive(Clone, Debug)]
enum Tok {
    Lit(char),
    Star,
    Any,
    AnyDir,
    OptDir,
    Group(Vec<Vec<Tok>>),
}

struct Glob {
    tokens: Vec<Tok>,
}

impl Glob {
    fn parse(glob: &str) -> Result<Self, String> {
        let chars: Vec<char> = glob.chars().collect();
        let mut top: Vec<Tok> = Vec::new();
        let mut group: Option<Vec<Vec<Tok>>> = None;
        let mut index = 0;
        let invalid = |why: &str| format!("Invalid glob pattern {}: {why}", json!(glob));
        while index < chars.len() {
            let ch = chars[index];
            let mut token = None;
            if ch == '\\' && index + 1 < chars.len() {
                index += 1;
                token = Some(Tok::Lit(chars[index]));
            } else if ch == '*' {
                let before = if index > 0 {
                    Some(chars[index - 1])
                } else {
                    None
                };
                let mut stars = 1;
                while index + 1 < chars.len() && chars[index + 1] == '*' {
                    stars += 1;
                    index += 1;
                }
                token = Some(if stars > 1 {
                    if index + 1 < chars.len() && chars[index + 1] == '/' {
                        index += 1;
                        if before == Some('/') {
                            Tok::OptDir
                        } else {
                            Tok::AnyDir
                        }
                    } else {
                        Tok::Any
                    }
                } else {
                    Tok::Star
                });
            } else if ch == '{' {
                if group.is_some() {
                    return Err(invalid("nested '{' is not supported"));
                }
                group = Some(vec![Vec::new()]);
            } else if ch == '}' {
                let Some(alternatives) = group.take() else {
                    return Err(invalid("unmatched '}'"));
                };
                top.push(Tok::Group(alternatives));
            } else if ch == ',' && group.is_some() {
                if let Some(alternatives) = group.as_mut() {
                    alternatives.push(Vec::new());
                }
            } else {
                token = Some(Tok::Lit(ch));
            }
            if let Some(token) = token {
                match group
                    .as_mut()
                    .and_then(|alternatives| alternatives.last_mut())
                {
                    Some(current) => current.push(token),
                    None => top.push(token),
                }
            }
            index += 1;
        }
        if group.is_some() {
            return Err(invalid("unmatched '{'"));
        }
        Ok(Self { tokens: top })
    }

    fn matches(&self, text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        matches_at(&self.tokens, &chars)
    }
}

fn matches_at(tokens: &[Tok], text: &[char]) -> bool {
    let Some((first, rest)) = tokens.split_first() else {
        return text.is_empty();
    };
    match first {
        Tok::Lit(ch) => text.first() == Some(ch) && matches_at(rest, &text[1..]),
        Tok::Star => {
            let mut taken = 0;
            loop {
                if matches_at(rest, &text[taken..]) {
                    return true;
                }
                if taken >= text.len() || text[taken] == '/' {
                    return false;
                }
                taken += 1;
            }
        }
        Tok::Any => (0..=text.len()).any(|taken| matches_at(rest, &text[taken..])),
        Tok::AnyDir => (0..text.len())
            .filter(|index| text[*index] == '/')
            .any(|index| matches_at(rest, &text[index + 1..])),
        Tok::OptDir => {
            matches_at(rest, text)
                || (1..text.len())
                    .filter(|index| text[*index] == '/')
                    .any(|index| matches_at(rest, &text[index + 1..]))
        }
        Tok::Group(alternatives) => alternatives.iter().any(|alternative| {
            let mut joined = alternative.clone();
            joined.extend_from_slice(rest);
            matches_at(&joined, text)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args;

    fn glob(pattern: &str, url: &str) -> bool {
        Route {
            pattern: pattern.to_string(),
            ..Route::default()
        }
        .matches(url)
    }

    fn invocation(argv: &[&str]) -> Invocation {
        let owned: Vec<String> = argv.iter().map(|item| item.to_string()).collect();
        args::parse(&owned).unwrap()
    }

    fn paused(url: &str) -> Paused {
        Paused {
            session_id: "S".into(),
            request_id: "R".into(),
            method: "GET".into(),
            url: url.into(),
            headers: vec![
                ("Accept".into(), "*/*".into()),
                ("Cookie".into(), "a=b".into()),
                ("Authorization".into(), "Bearer x".into()),
            ],
            ..Paused::default()
        }
    }

    #[test]
    fn route_list_and_unroute_print_what_upstream_prints() {
        let mut state = Session::default();
        assert_eq!(list(&state).stdout, "No active routes\n");
        let added = add(&mut state, &invocation(&["route", "**/a", "--status=404"]));
        assert_eq!(added.stdout, "Route added for pattern: **/a\n");
        add(&mut state, &invocation(&["route", "**/b", "--body", &"x".repeat(60)]));
        add(&mut state, &invocation(&["route", "**/a"]));
        assert_eq!(
            list(&state).stdout,
            format!("1. **/a (status=404)\n2. **/b (body={}...)\n3. **/a\n", "x".repeat(50))
        );
        let refused = add(&mut state, &invocation(&["route", "https://vault.sliccy.ai/*"]));
        assert_eq!(refused.code, 1);
        assert_eq!(state.routes.len(), 3);
        assert_eq!(
            remove(&mut state, &invocation(&["unroute", "**/a"])).stdout,
            "Removed 2 route(s) for pattern: **/a\n"
        );
        assert_eq!(
            remove(&mut state, &invocation(&["unroute", "**/zzz"])).stdout,
            "Removed 0 route(s) for pattern: **/zzz\n"
        );
        assert_eq!(remove(&mut state, &invocation(&["unroute"])).stdout, "Removed all 1 route(s)\n");
        assert!(state.routes.is_empty());
    }

    #[test]
    fn globs_follow_playwright() {
        assert!(glob("**/api/users", "https://app.example/api/users"));
        assert!(!glob("**/api/users", "https://app.example/api/users/1"));
        assert!(glob("**/api/*/details", "https://a.test/api/7/details"));
        assert!(!glob("**/api/*/details", "https://a.test/api/7/8/details"));
        assert!(glob("**/*.{png,jpg,jpeg}", "https://a.test/i/x.jpeg"));
        assert!(!glob("**/*.{png,jpg,jpeg}", "https://a.test/i/x.gif"));
        assert!(glob("**/search?q=*", "https://a.test/search?q=cats"));
        assert!(!glob("**/search?q=*", "https://a.test/searchXq=cats"));
        assert!(glob("**/*", "https://a.test/"));
        assert!(glob("**", "https://a.test/x/y"));
        assert!(glob("https://a.test/**", "https://a.test/x/y"));
        assert!(glob("https://a.test/**/z", "https://a.test/z"));
        assert!(glob("https://a.test/**/z", "https://a.test/x/y/z"));
        assert!(glob("https://a.test", "https://a.test/"));
        assert!(glob("https://a.test/\\*", "https://a.test/*"));
        assert!(!glob("https://a.test/\\*", "https://a.test/x"));
        assert!(!glob("*.jpg", "https://a.test/x.jpg"));
        assert!(Glob::parse("{a,{b}}").is_err());
        assert!(Glob::parse("a}").is_err());
        assert!(Glob::parse("{a").is_err());
    }

    #[test]
    fn cdp_patterns_cover_their_globs() {
        assert_eq!(cdp_pattern("**/api/users"), "*/api/users");
        assert_eq!(cdp_pattern("**/*.{png,jpg}"), "*/*.*");
        assert_eq!(cdp_pattern("**/search?q=*"), "*/search\\?q=*");
        assert_eq!(cdp_pattern("https://a.test/\\*"), "https://a.test/\\*");
    }

    #[test]
    fn route_parses_upstream_flags() {
        let route = parse(&invocation(&[
            "route",
            "**/api/data",
            "--body={\"ok\":true}",
            "--content-type=application/json",
            "--header=X-Custom: value",
            "--header",
            "X-Other:  two ",
            "--remove-header=Cookie, authorization",
            "--status=201",
        ]))
        .unwrap();
        assert_eq!(route.status, Some(201));
        assert_eq!(route.body.as_deref(), Some(&b"{\"ok\":true}"[..]));
        assert_eq!(
            route.add_headers,
            [
                ("X-Custom".to_string(), "value".to_string()),
                ("X-Other".to_string(), "two".to_string())
            ]
        );
        assert_eq!(route.remove_headers, ["cookie", "authorization"]);
        assert_eq!(
            route.describe(),
            "**/api/data (status=201, body={\"ok\":true}, contentType=application/json, addHeaders={\"X-Custom\":\"value\",\"X-Other\":\"two\"}, removeHeaders=cookie,authorization)"
        );
        let bad = parse(&invocation(&["route", "**/x", "--header=novalue"])).unwrap_err();
        assert_eq!(
            bad,
            "Invalid header \"novalue\": expected \"Name: Value\"\n"
        );
        assert!(parse(&invocation(&["route", "**/x", "--status=abc"])).is_err());
        assert!(parse(&invocation(&["route", "**/x", "--status=99"])).is_err());
        assert!(parse(&invocation(&["route", "{a"]))
            .unwrap_err()
            .contains("Invalid glob"));
        assert!(parse(&invocation(&["route"])).is_err());
    }

    #[test]
    fn route_refuses_seven_and_the_vault() {
        for pattern in [
            "https://vault.sliccy.ai/**",
            "https://lars.sliccy.ai/api/*",
            "**/*.sliccy.ai/**",
            "*sliccy.ai*",
            "https://www.sliccy.ai/auth/callback*",
        ] {
            let refused = parse(&invocation(&["route", pattern, "--status=200"])).unwrap_err();
            assert_eq!(
                refused, "slicc: refused: target is on seven's origin\n",
                "{pattern}"
            );
        }
        let stored = json!({ "pattern": "https://vault.sliccy.ai/*", "status": 200 });
        assert!(decode(&stored).is_none());
    }

    #[test]
    fn a_fulfil_route_answers_with_status_body_and_headers() {
        let route = parse(&invocation(&[
            "route",
            "**/api/users",
            "--body=[1]",
            "--content-type=application/json",
            "--header=X-Mock: yes",
        ]))
        .unwrap();
        let Decision::Fulfill(response) = route.decision(&paused("https://a.test/api/users"))
        else {
            panic!("expected a fulfil");
        };
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"[1]");
        assert_eq!(
            response.headers,
            [
                ("Content-Type".to_string(), "application/json".to_string()),
                ("X-Mock".to_string(), "yes".to_string()),
                ("Content-Length".to_string(), "3".to_string()),
            ]
        );
        let status_only = parse(&invocation(&["route", "**/*.jpg", "--status=404"])).unwrap();
        let Decision::Fulfill(response) = status_only.decision(&paused("https://a.test/x.jpg"))
        else {
            panic!("expected a fulfil");
        };
        assert_eq!(response.status, 404);
        assert!(response.body.is_empty());
    }

    #[test]
    fn a_header_route_continues_with_edited_request_headers() {
        let route = parse(&invocation(&[
            "route",
            "**/*",
            "--remove-header=cookie,authorization",
            "--header=X-Added: 1",
        ]))
        .unwrap();
        let Decision::Continue(overrides) = route.decision(&paused("https://a.test/")) else {
            panic!("expected a continue");
        };
        assert_eq!(
            overrides.headers.unwrap(),
            [
                ("Accept".to_string(), "*/*".to_string()),
                ("X-Added".to_string(), "1".to_string())
            ]
        );
        assert!(overrides.url.is_none());
    }

    #[test]
    fn the_newest_matching_route_wins_and_others_continue() {
        let older = parse(&invocation(&["route", "**/api/*", "--status=500"])).unwrap();
        let newer = parse(&invocation(&["route", "**/api/users", "--status=201"])).unwrap();
        let routes = vec![older, newer];
        let status = |url: &str| match decide(&routes, &paused(url)) {
            Decision::Fulfill(response) => Some(response.status),
            _ => None,
        };
        assert_eq!(status("https://a.test/api/users"), Some(201));
        assert_eq!(status("https://a.test/api/other"), Some(500));
        assert_eq!(status("https://a.test/page"), None);
        assert_eq!(
            decide(&routes, &paused("https://a.test/page")),
            Decision::default()
        );
    }

    #[test]
    fn fulfill_file_reads_bytes_and_guesses_the_type() {
        let dir = std::env::temp_dir().join(format!("slicc-route-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("page.html");
        std::fs::write(&file, b"<h1>mock</h1>").unwrap();
        let path = file.to_string_lossy().to_string();
        let route = parse(&invocation(&["route", "**/x", "--fulfill-file", &path])).unwrap();
        assert_eq!(route.describe(), format!("**/x (file={path})"));
        let Decision::Fulfill(response) = route.decision(&paused("https://a.test/x")) else {
            panic!("expected a fulfil");
        };
        assert_eq!(response.body, b"<h1>mock</h1>");
        assert_eq!(
            response.headers[0],
            ("Content-Type".to_string(), "text/html".to_string())
        );
        assert!(parse(&invocation(&[
            "route",
            "**/x",
            "--fulfill-file",
            &path,
            "--body=x"
        ]))
        .is_err());
        let missing = dir.join("missing.html").to_string_lossy().to_string();
        assert!(parse(&invocation(&["route", "**/x", "--fulfill-file", &missing])).is_err());
        let round = decode(&encode(&route)).unwrap();
        assert_eq!(round, route);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
