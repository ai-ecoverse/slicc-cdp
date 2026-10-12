#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use crate::args::{Invocation, Output};
use crate::cdp::Cdp;
use crate::connect::{self, Start};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

const WRAPPERS: &[&str] = &[
    "node",
    "npx",
    "tsx",
    "npm",
    "open",
    "bash",
    "zsh",
    "sh",
    "csh",
    "fish",
    "dash",
    "timeout",
    "env",
    "sudo",
    "caffeinate",
    "cmd",
    "powershell",
    "pwsh",
];

const CHROMIUM_BROWSERS: &[(&str, &str)] = &[
    ("com.google.Chrome", "Google Chrome"),
    ("com.google.Chrome.beta", "Google Chrome Beta"),
    ("com.google.Chrome.dev", "Google Chrome Dev"),
    ("com.google.Chrome.canary", "Chrome Canary"),
    ("com.google.chrome.for.testing", "Chrome for Testing"),
    ("com.brave.Browser", "Brave Browser"),
    ("com.brave.Browser.beta", "Brave Beta"),
    ("com.brave.Browser.nightly", "Brave Nightly"),
    ("com.microsoft.edgemac", "Microsoft Edge"),
    ("com.microsoft.edgemac.Beta", "Microsoft Edge Beta"),
    ("com.microsoft.edgemac.Dev", "Microsoft Edge Dev"),
    ("com.microsoft.edgemac.Canary", "Microsoft Edge Canary"),
    ("com.vivaldi.Vivaldi", "Vivaldi"),
    ("com.vivaldi.Vivaldi.snapshot", "Vivaldi Snapshot"),
    ("com.operasoftware.Opera", "Opera"),
    ("company.thebrowser.Browser", "Arc"),
    ("company.thebrowser.dia", "Dia"),
    ("com.openai.atlas", "ChatGPT Atlas"),
    ("org.chromium.Chromium", "Chromium"),
];

const ELECTRON_APPS: &[(&str, &str)] = &[
    ("com.microsoft.teams2", "Microsoft Teams"),
    ("com.microsoft.VSCode", "Visual Studio Code"),
    ("com.microsoft.VSCodeInsiders", "VS Code Insiders"),
    ("com.spotify.client", "Spotify"),
    ("com.tinyspeck.slackmacgap", "Slack"),
    ("com.hnc.Discord", "Discord"),
    ("com.todesktop.230313mzl4w4u92", "Cursor"),
    ("com.figma.Desktop", "Figma"),
    ("notion.id", "Notion"),
    ("com.obsproject.obs-studio", "OBS Studio"),
    ("com.1password.1password", "1Password"),
    ("us.zoom.xos", "Zoom"),
    ("com.linear", "Linear"),
    ("com.loom.desktop", "Loom"),
    ("md.obsidian", "Obsidian"),
    ("com.bitwarden.desktop", "Bitwarden"),
    ("com.todoist.mac.Todoist", "Todoist"),
    ("com.github.GitHubClient", "GitHub Desktop"),
    ("com.postmanlabs.mac", "Postman"),
    ("org.whispersystems.signal-desktop", "Signal"),
    ("com.tdesktop.Telegram", "Telegram Desktop"),
    ("com.logseq.logseq", "Logseq"),
    ("com.whatsonchain.WhatsOnChain", "WhatsOnChain"),
    ("io.trystorybook.app", "Storybook"),
    ("com.mongodb.compass", "MongoDB Compass"),
    ("com.insomnia.app", "Insomnia"),
    ("com.jetbrains.toolbox", "JetBrains Toolbox"),
    ("com.electron.replit", "Replit"),
    ("com.twitch.studio", "Twitch Studio"),
];

const BUNDLE_IDS: &[(&str, &str)] = &[
    ("com.google.Chrome", "chrome"),
    ("com.microsoft.edgemac", "edge"),
    ("com.brave.Browser", "brave"),
    ("com.operasoftware.Opera", "opera"),
    ("com.vivaldi.Vivaldi", "vivaldi"),
    ("company.thebrowser.Browser", "arc"),
    ("com.openai.atlas", "atlas"),
    ("company.thebrowser.dia", "dia"),
    ("com.google.Chrome.beta", "chrome-beta"),
    ("com.google.Chrome.dev", "chrome-dev"),
    ("com.google.Chrome.canary", "chrome-canary"),
    ("com.brave.Browser.beta", "brave-beta"),
    ("com.brave.Browser.nightly", "brave-nightly"),
    ("com.microsoft.edgemac.Beta", "edge-beta"),
    ("com.microsoft.edgemac.Dev", "edge-dev"),
    ("com.microsoft.edgemac.Canary", "edge-canary"),
    ("com.vivaldi.Vivaldi.snapshot", "vivaldi-snapshot"),
    ("com.google.chrome.for.testing", "chrome-for-testing"),
    ("org.chromium.Chromium", "chromium"),
];

const ACTIVE_PORT_FILE: &str = "DevToolsActivePort";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    #[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
    Macos,
    #[cfg_attr(not(any(test, target_os = "linux")), allow(dead_code))]
    Linux,
    #[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
    Windows,
}

pub struct LaunchQuery<'a> {
    pub os: HostOs,
    pub bundles: &'a dyn Fn(&str) -> Vec<String>,
    pub exists: &'a dyn Fn(&str) -> bool,
    pub chrome_path: Option<&'a str>,
    pub local_app_data: Option<&'a str>,
    pub program_files: Option<&'a str>,
    pub program_files_x86: Option<&'a str>,
}

pub struct Probe {
    pub websocket_url: String,
    pub targets: Result<Value, ()>,
}

pub fn run(invocation: &Invocation, cdp_env: Option<&str>) -> Output {
    #[cfg(not(target_arch = "wasm32"))]
    {
        run_host(invocation, cdp_env)
    }
    #[cfg(target_arch = "wasm32")]
    {
        run_guest(invocation, cdp_env)
    }
}

pub fn render(connected: &[String], ports: &[String], apps: &[String]) -> String {
    format!(
        "{}\n\n{}\n\n{}\n",
        section("connected", connected),
        section("debug ports", ports),
        section("launchable", apps)
    )
}

#[cfg(test)]
pub fn connected_lines_from_probe(probe: Result<Probe, ()>) -> Vec<String> {
    let Ok(probe) = probe else {
        return Vec::new();
    };
    match probe.targets {
        Ok(value) => browser_lines(&probe.websocket_url, &value),
        Err(()) => vec![format!("browser - {}", probe.websocket_url)],
    }
}

pub fn browser_lines(websocket_url: &str, targets: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(infos) = target_infos(targets) {
        for info in infos {
            if info.get("type").and_then(|value| value.as_str()) != Some("browser") {
                continue;
            }
            let target_id = info
                .get("targetId")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            if target_id.is_empty() {
                continue;
            }
            let url = info
                .get("url")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            lines.push(format!("browser {target_id} {url}"));
        }
    }
    if lines.is_empty() {
        lines.push(format!("browser - {websocket_url}"));
    }
    lines
}

#[cfg(test)]
pub fn debug_ports_unix(text: &str, self_pid: u32) -> Vec<String> {
    argv_hits_unix(text, self_pid)
        .into_iter()
        .filter(|hit| hit.port > 0)
        .map(|hit| argv_line(&hit))
        .collect()
}

fn argv_hits_unix(text: &str, self_pid: u32) -> Vec<ArgvHit> {
    let mut hits = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let Some(split) = line.find(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = line[..split].parse::<u32>() else {
            continue;
        };
        if pid == 0 || pid == self_pid {
            continue;
        }
        if let Some(hit) = argv_hit(pid, line[split..].trim()) {
            hits.push(hit);
        }
    }
    hits
}

#[cfg(test)]
pub fn debug_ports_windows(text: &str, self_pid: u32) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return Vec::new();
    };
    argv_hits_windows_value(&value, self_pid)
        .into_iter()
        .filter(|hit| hit.port > 0)
        .map(|hit| argv_line(&hit))
        .collect()
}

fn argv_hits_windows(text: &str, self_pid: u32) -> Vec<ArgvHit> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return Vec::new();
    };
    argv_hits_windows_value(&value, self_pid)
}

fn argv_hits_windows_value(value: &Value, self_pid: u32) -> Vec<ArgvHit> {
    let mut hits = Vec::new();
    match value {
        Value::Array(items) => {
            for entry in items {
                if let Some(hit) = windows_hit(entry, self_pid) {
                    hits.push(hit);
                }
            }
        }
        other => {
            if let Some(hit) = windows_hit(other, self_pid) {
                hits.push(hit);
            }
        }
    }
    hits
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_hit(entry: &Value, self_pid: u32) -> Option<ArgvHit> {
    let pid = entry.get("ProcessId").and_then(json_u32)?;
    if pid == 0 || pid == self_pid {
        return None;
    }
    let command = entry.get("CommandLine").and_then(|item| item.as_str())?;
    argv_hit(pid, command)
}

#[derive(Clone)]
struct FoundApp {
    name: String,
    kind: String,
    path: String,
    bundle_id: Option<String>,
}

#[cfg(test)]
pub fn launchables(query: &LaunchQuery<'_>) -> Vec<String> {
    let mut apps = found_apps(query);
    apps.sort_by(|left, right| left.name.cmp(&right.name).then(left.path.cmp(&right.path)));
    apps.into_iter()
        .map(|app| format!("{} {} {}", app.name, app.kind, app.path))
        .collect()
}

fn found_apps(query: &LaunchQuery<'_>) -> Vec<FoundApp> {
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    if query.os == HostOs::Macos {
        for (bundle_id, name) in CHROMIUM_BROWSERS {
            if let Some(path) = first_app(&(query.bundles)(bundle_id), query.exists) {
                push_app(&mut found, &mut seen, name, "browser", &path, Some(bundle_id));
            }
        }
        for (bundle_id, name) in ELECTRON_APPS {
            if let Some(path) = first_app(&(query.bundles)(bundle_id), query.exists) {
                push_app(&mut found, &mut seen, name, "electron", &path, Some(bundle_id));
            }
        }
    }
    for (name, path) in chrome_candidates(query) {
        if (query.exists)(&path) {
            push_app(&mut found, &mut seen, name, "browser", &path, None);
        }
    }
    if let Some(path) = query
        .chrome_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        if (query.exists)(path) {
            push_app(&mut found, &mut seen, "Chrome", "browser", path, None);
        }
    }
    found
}

fn section(title: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        format!("{title}\nnone")
    } else {
        format!("{title}\n{}", lines.join("\n"))
    }
}

fn target_infos(value: &Value) -> Option<&Vec<Value>> {
    value
        .get("targetInfos")
        .or_else(|| value.pointer("/result/targetInfos"))
        .and_then(|item| item.as_array())
}

struct ArgvHit {
    pid: u32,
    port: u32,
    executable: String,
    user_data_dir: Option<String>,
}

fn argv_hit(pid: u32, command: &str) -> Option<ArgvHit> {
    if wrapper_command(command) {
        return None;
    }
    let port = command_port(command)?;
    let executable = display_executable(command);
    if executable.is_empty() {
        return None;
    }
    Some(ArgvHit {
        pid,
        port,
        executable,
        user_data_dir: flag_value(command, "--user-data-dir").filter(|dir| !dir.is_empty()),
    })
}

#[cfg(test)]
fn argv_line(hit: &ArgvHit) -> String {
    format!("pid {} port {} {}", hit.pid, hit.port, hit.executable)
}

fn wrapper_command(command: &str) -> bool {
    let head = head_executable(command);
    if head.is_empty() {
        return false;
    }
    WRAPPERS
        .iter()
        .any(|name| basename_key(&head).eq_ignore_ascii_case(name))
}

fn head_executable(command: &str) -> String {
    let command = command.trim();
    if command.starts_with('"') || command.starts_with('\'') {
        return quoted_token(command);
    }
    command.split_whitespace().next().unwrap_or("").to_string()
}

fn display_executable(command: &str) -> String {
    let command = command.trim();
    if command.starts_with('"') || command.starts_with('\'') {
        let quoted = quoted_token(command);
        if !quoted.is_empty() {
            return quoted;
        }
    }
    let mut parts = Vec::new();
    for token in command.split_whitespace() {
        if token.starts_with('-') {
            break;
        }
        parts.push(token);
    }
    parts.join(" ")
}

fn quoted_token(command: &str) -> String {
    let mut chars = command.chars();
    let Some(quote) = chars.next() else {
        return String::new();
    };
    let mut out = String::new();
    for ch in chars {
        if ch == quote {
            break;
        }
        out.push(ch);
    }
    out
}

fn basename_key(executable: &str) -> String {
    let base = executable.rsplit(['/', '\\']).next().unwrap_or(executable);
    let lower = base.to_ascii_lowercase();
    lower
        .strip_suffix(".exe")
        .or_else(|| lower.strip_suffix(".cmd"))
        .or_else(|| lower.strip_suffix(".bat"))
        .unwrap_or(&lower)
        .to_string()
}

fn command_port(command: &str) -> Option<u32> {
    let digits = debugging_port(command)?;
    let port = digits.parse::<u32>().ok()?;
    if port > 65535 {
        None
    } else {
        Some(port)
    }
}

fn debugging_port(command: &str) -> Option<String> {
    let marker = "--remote-debugging-port";
    let mut rest = command;
    while let Some(index) = rest.find(marker) {
        let after = &rest[index + marker.len()..];
        if let Some(port) = port_digits(after) {
            return Some(port);
        }
        let mut chars = after.chars();
        chars.next();
        rest = chars.as_str();
    }
    None
}

fn port_digits(after: &str) -> Option<String> {
    let body = if let Some(rest) = after.strip_prefix('=') {
        rest
    } else if after.starts_with(|ch: char| ch.is_whitespace()) {
        after.trim_start()
    } else {
        return None;
    };
    let digits: String = body.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

fn flag_value(command: &str, marker: &str) -> Option<String> {
    let bytes = command.as_bytes();
    let mut start = 0;
    while let Some(rel) = command[start..].find(marker) {
        let index = start + rel;
        let before_ok = index == 0 || bytes[index - 1].is_ascii_whitespace();
        let after = index + marker.len();
        if !before_ok || after > command.len() {
            start = index + 1;
            continue;
        }
        let rest = &command[after..];
        if let Some(value) = rest.strip_prefix('=') {
            return Some(take_token(value));
        }
        if rest.starts_with(|ch: char| ch.is_whitespace()) {
            return Some(take_token(rest));
        }
        start = after;
    }
    None
}

fn take_token(text: &str) -> String {
    let text = text.trim_start();
    if text.starts_with('"') || text.starts_with('\'') {
        return quoted_token(text);
    }
    if text.starts_with("--") {
        return String::new();
    }
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            let after = text[index..].trim_start();
            if after.starts_with("--") {
                return text[..index].trim_end().to_string();
            }
        }
        index += 1;
    }
    text.trim_end().to_string()
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn json_u32(value: &Value) -> Option<u32> {
    if let Some(number) = value.as_u64() {
        return u32::try_from(number).ok();
    }
    if let Some(number) = value.as_i64() {
        return u32::try_from(number).ok();
    }
    value.as_str().and_then(|text| text.parse().ok())
}

fn first_app(paths: &[String], exists: &dyn Fn(&str) -> bool) -> Option<String> {
    for path in paths {
        let trimmed = path.trim().trim_end_matches(['/', '\\']);
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.to_ascii_lowercase().ends_with(".app") {
            continue;
        }
        if exists(trimmed) {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn push_app(
    found: &mut Vec<FoundApp>,
    seen: &mut BTreeSet<String>,
    name: &str,
    kind: &str,
    path: &str,
    bundle_id: Option<&str>,
) {
    if path.is_empty() || !seen.insert(path.to_string()) {
        return;
    }
    found.push(FoundApp {
        name: name.to_string(),
        kind: kind.to_string(),
        path: path.to_string(),
        bundle_id: bundle_id.map(str::to_string),
    });
}

fn chrome_candidates(query: &LaunchQuery<'_>) -> Vec<(&'static str, String)> {
    match query.os {
        HostOs::Macos => vec![
            (
                "Google Chrome",
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string(),
            ),
            (
                "Google Chrome Canary",
                "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary"
                    .to_string(),
            ),
            (
                "Chromium",
                "/Applications/Chromium.app/Contents/MacOS/Chromium".to_string(),
            ),
        ],
        HostOs::Linux => vec![
            ("Google Chrome", "/usr/bin/google-chrome".to_string()),
            ("Google Chrome", "/usr/bin/google-chrome-stable".to_string()),
            ("Chromium", "/usr/bin/chromium".to_string()),
            ("Chromium", "/usr/bin/chromium-browser".to_string()),
            ("Chromium", "/snap/bin/chromium".to_string()),
        ],
        HostOs::Windows => {
            let mut paths = Vec::new();
            if let Some(root) = nonempty(query.local_app_data) {
                paths.push((
                    "Google Chrome",
                    format!(r"{root}\Google\Chrome\Application\chrome.exe"),
                ));
            }
            if let Some(root) = nonempty(query.program_files) {
                paths.push((
                    "Google Chrome",
                    format!(r"{root}\Google\Chrome\Application\chrome.exe"),
                ));
            }
            if let Some(root) = nonempty(query.program_files_x86) {
                paths.push((
                    "Google Chrome",
                    format!(r"{root}\Google\Chrome\Application\chrome.exe"),
                ));
            }
            paths
        }
    }
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

#[derive(Clone)]
enum HttpResult {
    Response { status: u16, body: String },
    Transport,
}

struct ListIo<'a> {
    scan_host: bool,
    os: HostOs,
    home: Option<&'a str>,
    xdg_config_home: Option<&'a str>,
    appdata: Option<&'a str>,
    process_text: &'a str,
    windows_processes: bool,
    self_pid: u32,
    launch: LaunchQuery<'a>,
    http: &'a dyn Fn(&str) -> HttpResult,
    read: &'a dyn Fn(&str) -> Option<String>,
    list_dir: &'a dyn Fn(&str) -> Vec<String>,
    targets: &'a dyn Fn(&str) -> Result<Probe, ()>,
    flag_urls: &'a [&'a str],
    installed: Option<&'a [Value]>,
}

struct VersionDoc {
    websocket: String,
    product: String,
    product_name: String,
    version: Option<String>,
    user_agent: String,
}

struct TabDraft {
    id: String,
    url: String,
    title: String,
}

struct EndpointDraft {
    id: String,
    host: String,
    port: u32,
    source: &'static str,
    cdp: String,
    websocket: Option<String>,
    name: String,
    version: Option<String>,
    product: Option<String>,
    user_agent: Option<String>,
    browser: Option<String>,
    user_data_dir: Option<String>,
    profile: Option<String>,
    ws_only: bool,
    seven_open: bool,
    tabs: Vec<TabDraft>,
}

struct InstalledDraft {
    id: String,
    name: String,
    engine: &'static str,
    path: String,
    bundle_id: Option<String>,
    debug_support: &'static str,
}

struct ReportEntry {
    id: String,
    kind: &'static str,
    tier: u8,
    name: String,
    version: Option<String>,
    port: u32,
    tie: String,
    priority: usize,
    sort_name: String,
    endpoint: Option<EndpointDraft>,
    installed: Option<InstalledDraft>,
    installed_raw: Option<Value>,
}

fn assemble(invocation: &Invocation, cdp_env: Option<&str>, io: &ListIo<'_>) -> Output {
    let json = wants_json(invocation);
    let explicit = explicit_endpoint(invocation, cdp_env);
    let mut stderr = None;
    let mut dial = None;
    let mut drafts = Vec::new();
    let cache = RefCell::new(HashMap::<String, HttpResult>::new());
    let http = |url: &str| -> HttpResult {
        if let Some(hit) = cache.borrow().get(url).cloned() {
            return hit;
        }
        let hit = (io.http)(url);
        cache.borrow_mut().insert(url.to_string(), hit.clone());
        hit
    };
    match listed_start(invocation, cdp_env) {
        None => {}
        Some(Start::Direct(url)) => {
            let runtime = runtime_name(invocation);
            dial = Some(connect::append_runtime(&url, runtime.as_deref()));
            if let Some(draft) = flag_from_direct(&url, &http) {
                drafts.push(draft);
            }
        }
        Some(Start::Discover(url)) => {
            if explicit || !json {
                match http(&url) {
                    HttpResult::Response { status, body } if (200..300).contains(&status) => {
                        if let Ok(ws) = connect::websocket_url_from_version(&body) {
                            let runtime = runtime_name(invocation);
                            dial = Some(connect::append_runtime(&ws, runtime.as_deref()));
                            if explicit {
                                if let Some(draft) = flag_from_version(&url, &body, &http) {
                                    drafts.push(draft);
                                }
                            }
                        }
                    }
                    HttpResult::Response { status, body } => {
                        if explicit {
                            stderr = Some(connect::format_http_error(status, body.as_bytes()));
                        }
                    }
                    HttpResult::Transport => {}
                }
            }
        }
    }
    for url in io.flag_urls {
        push_flag_url(url, &mut drafts, &http);
    }
    let mut connected = if json {
        Vec::new()
    } else if let Some(url) = dial.as_deref() {
        match (io.targets)(url) {
            Ok(probe) => match probe.targets {
                Ok(value) => browser_lines(&probe.websocket_url, &value),
                Err(()) => vec![format!("browser - {}", probe.websocket_url)],
            },
            Err(()) => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let mut port_lines = Vec::new();
    let mut apps = Vec::new();
    if io.scan_host {
        for draft in profile_drafts(io, &http) {
            drafts.push(draft);
        }
        let hits = if io.windows_processes {
            argv_hits_windows(io.process_text, io.self_pid)
        } else {
            argv_hits_unix(io.process_text, io.self_pid)
        };
        for hit in hits {
            if let Some((port, draft)) = resolve_process(&hit, io.read, &http) {
                port_lines.push(format!("pid {} port {port} {}", hit.pid, hit.executable));
                drafts.push(draft);
            }
        }
        apps = found_apps(&io.launch);
    }
    let endpoints = merge_drafts(drafts);
    if !json {
        for endpoint in &endpoints {
            if endpoint.source != "devtools-active-port" {
                continue;
            }
            let Some(ws) = endpoint.websocket.as_deref().filter(|item| !item.is_empty()) else {
                continue;
            };
            let line = format!("browser - {ws}");
            if !connected.iter().any(|item| item == &line) {
                connected.push(line);
            }
        }
    }
    let mut entries = report_entries(&endpoints, &apps, io.installed);
    sort_entries(&mut entries);
    if json {
        return output_of(report_json(&entries), stderr);
    }
    output_of(render(&connected, &port_lines, &apps_to_lines(apps)), stderr)
}

fn wants_json(invocation: &Invocation) -> bool {
    match invocation.flags.get("json").map(String::as_str) {
        Some("false") | None => false,
        Some(_) => true,
    }
}

fn explicit_endpoint(invocation: &Invocation, cdp_env: Option<&str>) -> bool {
    nonempty(invocation.cdp.as_deref()).is_some() || nonempty(cdp_env).is_some()
}

fn listed_start(invocation: &Invocation, cdp_env: Option<&str>) -> Option<Start> {
    if !explicit_endpoint(invocation, cdp_env) && cfg!(not(target_arch = "wasm32")) {
        return None;
    }
    connect::choose_start(invocation.cdp.as_deref(), cdp_env).ok()
}

fn reserved_debug_port(port: u32) -> bool {
    (9222..=9229).contains(&port)
}

fn runtime_name(invocation: &Invocation) -> Option<String> {
    invocation.runtime.clone().or_else(|| {
        invocation
            .flags
            .get("runtime")
            .filter(|value| !value.is_empty())
            .cloned()
    })
}

fn output_of(stdout: String, stderr: Option<String>) -> Output {
    match stderr {
        Some(stderr) => Output {
            stdout,
            stderr,
            code: 1,
        },
        None => Output::ok(stdout),
    }
}

fn resolve_process(
    hit: &ArgvHit,
    read: &dyn Fn(&str) -> Option<String>,
    http: &dyn Fn(&str) -> HttpResult,
) -> Option<(u32, EndpointDraft)> {
    if hit.port == 0 {
        let dir = hit.user_data_dir.as_deref()?;
        if profiles_under_slicc(dir) {
            return None;
        }
        let text = read(&active_port_path(dir))?;
        let (port, ws_path) = parse_active_port(&text)?;
        if reserved_debug_port(port) {
            return Some((port, draft_bare(port, "process")));
        }
        let draft = draft_from_port(port, "process", Some(dir), None, None, Some(&ws_path), http)?;
        return Some((port, draft));
    }
    if reserved_debug_port(hit.port) {
        return Some((hit.port, draft_bare(hit.port, "process")));
    }
    let draft = draft_from_port(
        hit.port,
        "process",
        hit.user_data_dir.as_deref(),
        None,
        None,
        None,
        http,
    )?;
    Some((hit.port, draft))
}

fn profile_drafts(io: &ListIo<'_>, http: &dyn Fn(&str) -> HttpResult) -> Vec<EndpointDraft> {
    let mut out = Vec::new();
    for (id, dir) in user_data_dirs(io) {
        if let Some(draft) = draft_from_active_file(&dir, Some(&id), None, io, http) {
            out.push(draft);
        }
    }
    for root in profile_roots(io) {
        let mut names = (io.list_dir)(&root);
        names.sort();
        for name in names {
            if !profile_dir_name(&name) || six_profile_name(&name) {
                continue;
            }
            let dir = join_two(&root, &name);
            if let Some(draft) = draft_from_active_file(&dir, None, Some(&name), io, http) {
                out.push(draft);
            }
        }
    }
    out
}

fn draft_from_active_file(
    dir: &str,
    browser: Option<&str>,
    profile: Option<&str>,
    io: &ListIo<'_>,
    http: &dyn Fn(&str) -> HttpResult,
) -> Option<EndpointDraft> {
    let text = (io.read)(&active_port_path(dir))?;
    let (port, ws_path) = parse_active_port(&text)?;
    if reserved_debug_port(port) {
        return None;
    }
    draft_from_port(
        port,
        "devtools-active-port",
        Some(dir),
        browser,
        profile,
        Some(&ws_path),
        http,
    )
}

fn draft_from_port(
    port: u32,
    source: &'static str,
    dir: Option<&str>,
    browser: Option<&str>,
    profile: Option<&str>,
    ws_path: Option<&str>,
    http: &dyn Fn(&str) -> HttpResult,
) -> Option<EndpointDraft> {
    let base = format!("http://127.0.0.1:{port}");
    let version_url = format!("{base}/json/version");
    match http(&version_url) {
        HttpResult::Transport => None,
        HttpResult::Response { status, body } => {
            if (200..300).contains(&status) {
                if let Some(doc) = parse_version(&body) {
                    if let Some(path) = ws_path {
                        if !ws_path_matches(&doc.websocket, path) {
                            return None;
                        }
                    }
                    let tabs = fetch_tabs(&base, http);
                    let mut draft = draft_from_doc(&base, source, dir, browser, doc, tabs)?;
                    draft.profile = profile.map(str::to_string);
                    return Some(draft);
                }
            }
            ws_only_from_file(port, source, dir, browser, profile, ws_path)
        }
    }
}

fn ws_only_from_file(
    port: u32,
    source: &'static str,
    dir: Option<&str>,
    browser: Option<&str>,
    profile: Option<&str>,
    ws_path: Option<&str>,
) -> Option<EndpointDraft> {
    let path = ws_path?;
    let fallback = slicc_browsers::catalog().discovery.names.fallback.clone();
    let name = match browser {
        Some(id) => catalog_browser_name(id).unwrap_or(fallback),
        None => fallback,
    };
    let mut draft = draft_bare(port, source);
    draft.websocket = Some(format!("ws://127.0.0.1:{port}{path}"));
    draft.name = name;
    draft.user_data_dir = dir.map(str::to_string);
    draft.browser = browser.and_then(|id| catalog_browser_name(id).map(|_| id.to_string()));
    draft.profile = profile.map(str::to_string);
    draft.ws_only = true;
    Some(draft)
}

fn six_profile_name(name: &str) -> bool {
    let prefix = "browser-coding-agent-chrome";
    name.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn user_data_dirs(io: &ListIo<'_>) -> Vec<(String, String)> {
    let catalog = slicc_browsers::catalog();
    let mut dirs = Vec::new();
    for browser in &catalog.browsers {
        match io.os {
            HostOs::Macos => {
                if browser.macos.data_dir.is_empty() {
                    continue;
                }
                let config = config_dir(io, &catalog);
                if config.is_empty() {
                    continue;
                }
                dirs.push((
                    browser.id.clone(),
                    join_all(&config, &browser.macos.data_dir),
                ));
            }
            HostOs::Windows => {
                let Some(windows) = browser.windows.as_ref().filter(|item| !item.data_dir.is_empty())
                else {
                    continue;
                };
                let root = windows_root(io, &catalog, &windows.data_root);
                if root.is_empty() {
                    continue;
                }
                dirs.push((browser.id.clone(), join_all(&root, &windows.data_dir)));
            }
            HostOs::Linux => {
                let Some(linux) = &browser.linux else {
                    continue;
                };
                if !linux.config_dir.is_empty() {
                    let config = config_dir(io, &catalog);
                    if !config.is_empty() {
                        dirs.push((browser.id.clone(), join_all(&config, &linux.config_dir)));
                    }
                    if let Some(flatpak) = &linux.flatpak {
                        if let Some(home) = nonempty(io.home) {
                            let base = join_all(
                                home,
                                &[
                                    ".var".to_string(),
                                    "app".to_string(),
                                    flatpak.clone(),
                                    "config".to_string(),
                                ],
                            );
                            dirs.push((browser.id.clone(), join_all(&base, &linux.config_dir)));
                        }
                    }
                }
                if let Some(snap) = linux.snap.as_ref().filter(|_| !linux.snap_data_dir.is_empty())
                {
                    if let Some(home) = nonempty(io.home) {
                        let base = join_all(
                            home,
                            &["snap".to_string(), snap.clone(), "common".to_string()],
                        );
                        dirs.push((browser.id.clone(), join_all(&base, &linux.snap_data_dir)));
                    }
                }
            }
        }
    }
    dirs
}

fn config_dir(io: &ListIo<'_>, catalog: &slicc_browsers::Catalog) -> String {
    let (loc, absolute_only) = match io.os {
        HostOs::Macos => (&catalog.platforms.macos.config, false),
        HostOs::Windows => (&catalog.platforms.windows.config, false),
        HostOs::Linux => (&catalog.platforms.linux.config, true),
    };
    if let Some(name) = &loc.env {
        let value = match name.as_str() {
            "XDG_CONFIG_HOME" => io.xdg_config_home,
            "APPDATA" => io.appdata,
            _ => None,
        };
        if let Some(value) = nonempty(value) {
            if !absolute_only || is_absolute(value) {
                return value.to_string();
            }
        }
    }
    if let Some(abs) = &loc.absolute {
        return abs.clone();
    }
    match nonempty(io.home) {
        Some(home) => join_all(home, &loc.home),
        None => String::new(),
    }
}

fn windows_root(io: &ListIo<'_>, catalog: &slicc_browsers::Catalog, name: &str) -> String {
    let Some(loc) = catalog.platforms.windows.roots.get(name) else {
        return String::new();
    };
    let env_value = match loc.env.as_deref() {
        Some("LOCALAPPDATA") => io.launch.local_app_data,
        Some("PROGRAMFILES") => io.launch.program_files,
        Some("PROGRAMFILES(X86)") => io.launch.program_files_x86,
        Some("APPDATA") => io.appdata,
        _ => None,
    };
    if let Some(value) = nonempty(env_value) {
        return value.to_string();
    }
    if let Some(abs) = &loc.absolute {
        return abs.clone();
    }
    match nonempty(io.home) {
        Some(home) => join_all(home, &loc.home),
        None => String::new(),
    }
}

fn join_all(root: &str, parts: &[String]) -> String {
    let mut out = root.to_string();
    for part in parts {
        out = join_two(&out, part);
    }
    out
}

fn profile_roots(io: &ListIo<'_>) -> Vec<String> {
    let mut roots = Vec::new();
    match io.os {
        HostOs::Macos => {
            if let Some(home) = nonempty(io.home) {
                roots.push(format!(
                    "{home}/Library/Application Support/slicc/seven-profiles"
                ));
            }
        }
        HostOs::Windows => {
            if let Some(appdata) = nonempty(io.appdata) {
                roots.push(join_two(&join_two(appdata, "slicc"), "seven-profiles"));
            }
        }
        HostOs::Linux => {
            let config = linux_config(io.home, io.xdg_config_home);
            if !config.is_empty() {
                roots.push(format!("{config}/slicc/seven-profiles"));
            }
        }
    }
    if let Some(home) = nonempty(io.home) {
        let snap = join_two(home, "snap");
        let mut names = (io.list_dir)(&snap);
        names.sort();
        for name in names {
            if !dir_name_ok(&name) {
                continue;
            }
            let profiles = join_two(
                &join_two(&join_two(&join_two(&snap, &name), "common"), "slicc"),
                "seven-profiles",
            );
            roots.push(profiles);
        }
    }
    roots
}

fn linux_config(home: Option<&str>, xdg: Option<&str>) -> String {
    if let Some(xdg) = nonempty(xdg) {
        if is_absolute(xdg) {
            return xdg.to_string();
        }
    }
    match nonempty(home) {
        Some(home) => format!("{home}/.config"),
        None => String::new(),
    }
}

fn is_absolute(path: &str) -> bool {
    if path.starts_with('/') || path.starts_with('\\') {
        return true;
    }
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn dir_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
}

fn profile_dir_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 64 || name.contains("..") {
        return false;
    }
    let [first, rest @ ..] = bytes else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && rest
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn profiles_under_slicc(path: &str) -> bool {
    let mut slicc = false;
    for component in path.split(['/', '\\']) {
        if component.is_empty() {
            continue;
        }
        if slicc && component.eq_ignore_ascii_case("profiles") {
            return true;
        }
        slicc = component.eq_ignore_ascii_case("slicc");
    }
    false
}

fn join_two(root: &str, child: &str) -> String {
    let sep = if root.contains('\\') && !root.contains('/') {
        '\\'
    } else {
        '/'
    };
    format!("{}{sep}{child}", root.trim_end_matches(['/', '\\']))
}

fn active_port_path(dir: &str) -> String {
    join_two(dir, ACTIVE_PORT_FILE)
}

fn parse_active_port(text: &str) -> Option<(u32, String)> {
    let mut lines = text.split('\n');
    let port = lines.next()?.trim().parse::<u32>().ok()?;
    if port == 0 || port > 65535 {
        return None;
    }
    let path = lines.next()?.trim().to_string();
    if !path.starts_with('/') {
        return None;
    }
    Some((port, path))
}

fn fetch_tabs(base: &str, http: &dyn Fn(&str) -> HttpResult) -> Vec<TabDraft> {
    let HttpResult::Response { status, body } = http(&format!("{base}/json/list")) else {
        return Vec::new();
    };
    if !(200..300).contains(&status) {
        return Vec::new();
    }
    parse_tabs(&body).unwrap_or_default()
}

fn parse_version(body: &str) -> Option<VersionDoc> {
    let value: Value = serde_json::from_str(body).ok()?;
    let websocket = value
        .get("webSocketDebuggerUrl")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    if websocket.is_empty() {
        return None;
    }
    let product = value
        .get("Browser")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    let user_agent = value
        .get("User-Agent")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    let (product_name, version) = split_product(&product);
    Some(VersionDoc {
        websocket,
        product,
        product_name,
        version,
        user_agent,
    })
}

fn split_product(product: &str) -> (String, Option<String>) {
    let Some((name, rest)) = product.split_once('/') else {
        return (product.to_string(), None);
    };
    if name.is_empty() {
        return (product.to_string(), None);
    }
    let version = rest.split_whitespace().next().unwrap_or("");
    if version.is_empty() {
        (name.to_string(), None)
    } else {
        (name.to_string(), Some(version.to_string()))
    }
}

fn parse_tabs(body: &str) -> Option<Vec<TabDraft>> {
    let value: Value = serde_json::from_str(body).ok()?;
    let items = value.as_array()?;
    let mut tabs = Vec::new();
    for item in items {
        if item.get("type").and_then(|value| value.as_str()) != Some("page") {
            continue;
        }
        let url = item.get("url").and_then(|value| value.as_str()).unwrap_or("");
        if !seven_origin(url) {
            continue;
        }
        tabs.push(TabDraft {
            id: item
                .get("id")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
            url: url.to_string(),
            title: item
                .get("title")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    Some(tabs)
}

fn seven_origin(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = if let Some(stripped) = host.strip_prefix('[') {
        stripped
            .split_once(']')
            .map(|(name, _)| name)
            .unwrap_or(stripped)
    } else if let Some((name, port)) = host.rsplit_once(':') {
        if port != "443" {
            return false;
        }
        name
    } else {
        host
    };
    let host = host.to_ascii_lowercase();
    let Some(label) = host.strip_suffix(".sliccy.ai") else {
        return false;
    };
    if label.is_empty() || label == "www" || label.contains('.') {
        return false;
    }
    let mut chars = label.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    let mut last = first;
    for ch in chars {
        if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' {
            return false;
        }
        last = ch;
    }
    last != '-'
}

fn ws_path_matches(websocket: &str, path: &str) -> bool {
    let bare = websocket.split(['?', '#']).next().unwrap_or(websocket);
    let Some((_, rest)) = bare.split_once("://") else {
        return bare.ends_with(path);
    };
    let url_path = match rest.find('/') {
        Some(index) => &rest[index..],
        None => "",
    };
    url_path == path
}

enum FlagKind {
    Direct(String),
    Discover(String),
}

fn push_flag_url(raw: &str, drafts: &mut Vec<EndpointDraft>, http: &dyn Fn(&str) -> HttpResult) {
    let Some(kind) = classify_flag(raw) else {
        return;
    };
    match kind {
        FlagKind::Direct(url) => {
            if let Some(draft) = flag_from_direct(&url, http) {
                drafts.push(draft);
            }
        }
        FlagKind::Discover(url) => {
            let version_url = connect::discovery_url(&url);
            if let HttpResult::Response { status, body } = http(&version_url) {
                if (200..300).contains(&status) {
                    if let Some(draft) = flag_from_version(&version_url, &body, http) {
                        drafts.push(draft);
                    }
                }
            }
        }
    }
}

fn classify_flag(raw: &str) -> Option<FlagKind> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("ws://") || lower.starts_with("wss://") {
        return Some(FlagKind::Direct(trimmed.to_string()));
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(FlagKind::Discover(trimmed.to_string()));
    }
    if trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return Some(FlagKind::Discover(format!("http://127.0.0.1:{trimmed}")));
    }
    if host_port_text(trimmed) {
        return Some(FlagKind::Discover(format!("http://{trimmed}")));
    }
    None
}

fn host_port_text(text: &str) -> bool {
    if let Some(rest) = text.strip_prefix('[') {
        let Some((host, port)) = rest.split_once("]:") else {
            return false;
        };
        return !host.is_empty()
            && !port.is_empty()
            && port.bytes().all(|byte| byte.is_ascii_digit());
    }
    let Some((host, port)) = text.rsplit_once(':') else {
        return false;
    };
    !host.is_empty()
        && !host.contains('/')
        && !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
}

fn flag_from_direct(url: &str, http: &dyn Fn(&str) -> HttpResult) -> Option<EndpointDraft> {
    let base = ws_to_http_base(url)?;
    let version_url = format!("{base}/json/version");
    match http(&version_url) {
        HttpResult::Transport => None,
        HttpResult::Response { status, body } if (200..300).contains(&status) => {
            if let Some(doc) = parse_version(&body) {
                let tabs = fetch_tabs(&base, http);
                draft_from_doc(&base, "flag", None, None, doc, tabs)
            } else {
                ws_only_flag(url, &base)
            }
        }
        HttpResult::Response { .. } => ws_only_flag(url, &base),
    }
}

fn ws_only_flag(url: &str, base: &str) -> Option<EndpointDraft> {
    if !ws_scheme(url) {
        return None;
    }
    let (host, port) = host_port(base)?;
    let mut draft = draft_bare(port, "flag");
    draft.host = host.clone();
    draft.id = format!("cdp:{host}:{port}");
    draft.cdp = normalized_cdp(base).unwrap_or_else(|| base.to_string());
    draft.websocket = Some(url.to_string());
    draft.name = slicc_browsers::catalog().discovery.names.fallback.clone();
    draft.ws_only = true;
    Some(draft)
}

fn ws_scheme(url: &str) -> bool {
    let scheme = url.split("://").next().unwrap_or("");
    scheme.eq_ignore_ascii_case("ws") || scheme.eq_ignore_ascii_case("wss")
}

fn flag_from_version(
    discovery: &str,
    body: &str,
    http: &dyn Fn(&str) -> HttpResult,
) -> Option<EndpointDraft> {
    let doc = parse_version(body)?;
    let base = http_origin(discovery)?;
    let tabs = fetch_tabs(&base, http);
    draft_from_doc(&base, "flag", None, None, doc, tabs)
}

fn draft_from_doc(
    base: &str,
    source: &'static str,
    user_data_dir: Option<&str>,
    browser: Option<&str>,
    doc: VersionDoc,
    tabs: Vec<TabDraft>,
) -> Option<EndpointDraft> {
    let (host, port) = host_port(base)?;
    let name = endpoint_name(browser, &doc.product_name, &doc.user_agent);
    Some(EndpointDraft {
        id: format!("cdp:{host}:{port}"),
        host,
        port,
        source,
        cdp: normalized_cdp(base).unwrap_or_else(|| base.to_string()),
        websocket: Some(doc.websocket),
        name,
        version: doc.version,
        product: if doc.product.is_empty() {
            None
        } else {
            Some(doc.product)
        },
        user_agent: if doc.user_agent.is_empty() {
            None
        } else {
            Some(doc.user_agent)
        },
        browser: browser.and_then(|id| catalog_browser_name(id).map(|_| id.to_string())),
        user_data_dir: user_data_dir.map(str::to_string),
        profile: None,
        ws_only: false,
        seven_open: !tabs.is_empty(),
        tabs,
    })
}

fn draft_bare(port: u32, source: &'static str) -> EndpointDraft {
    EndpointDraft {
        id: format!("cdp:127.0.0.1:{port}"),
        host: "127.0.0.1".to_string(),
        port,
        source,
        cdp: format!("http://127.0.0.1:{port}"),
        websocket: None,
        name: "Chromium".to_string(),
        version: None,
        product: None,
        user_agent: None,
        browser: None,
        user_data_dir: None,
        profile: None,
        ws_only: false,
        seven_open: false,
        tabs: Vec::new(),
    }
}

fn endpoint_name(browser: Option<&str>, product_name: &str, user_agent: &str) -> String {
    let catalog = slicc_browsers::catalog();
    if let Some(id) = browser {
        if let Some(browser) = catalog.browser(id) {
            return browser.name.clone();
        }
    }
    let names = &catalog.discovery.names;
    if user_agent.contains(&names.electron_user_agent) {
        return names.electron.clone();
    }
    if product_name.is_empty() {
        return names.fallback.clone();
    }
    names
        .products
        .get(product_name)
        .cloned()
        .unwrap_or_else(|| product_name.to_string())
}

fn catalog_browser_name(id: &str) -> Option<String> {
    slicc_browsers::catalog()
        .browser(id)
        .map(|browser| browser.name.clone())
}

fn http_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{host}"))
}

fn ws_to_http_base(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let http_scheme = match scheme.to_ascii_lowercase().as_str() {
        "ws" | "http" => "http",
        "wss" | "https" => "https",
        _ => return None,
    };
    let host = rest.split(['/', '?', '#']).next()?;
    if !host.contains(':') {
        return None;
    }
    Some(format!("{http_scheme}://{host}"))
}

fn normalized_cdp(base: &str) -> Option<String> {
    let (host, port) = host_port(base)?;
    let scheme = base.split("://").next()?;
    Some(format!("{scheme}://{host}:{port}"))
}

fn host_port(base: &str) -> Option<(String, u32)> {
    let rest = base.split_once("://")?.1;
    let hostport = rest.split(['/', '?', '#']).next()?;
    let (host, port) = split_host_port(hostport)?;
    let port = port.parse::<u32>().ok()?;
    if port == 0 || port > 65535 {
        return None;
    }
    Some((normalize_host(host), port))
}

fn split_host_port(hostport: &str) -> Option<(&str, &str)> {
    if let Some(rest) = hostport.strip_prefix('[') {
        let (host, port) = rest.split_once("]:")?;
        return Some((host, port));
    }
    hostport.rsplit_once(':')
}

fn normalize_host(host: &str) -> String {
    let host = host.trim_matches(['[', ']']);
    if host.eq_ignore_ascii_case("localhost") || host == "::1" {
        "127.0.0.1".to_string()
    } else {
        host.to_ascii_lowercase()
    }
}

fn merge_drafts(drafts: Vec<EndpointDraft>) -> Vec<EndpointDraft> {
    let mut best: BTreeMap<String, EndpointDraft> = BTreeMap::new();
    for draft in drafts {
        if too_old(draft.user_agent.as_deref().unwrap_or("")) {
            continue;
        }
        match best.get(&draft.id) {
            Some(current) if source_rank(current.source) <= source_rank(draft.source) => {}
            _ => {
                best.insert(draft.id.clone(), draft);
            }
        }
    }
    best.into_values().collect()
}

fn source_rank(source: &str) -> u8 {
    match source {
        "devtools-active-port" => 0,
        "flag" => 1,
        _ => 2,
    }
}

fn too_old(user_agent: &str) -> bool {
    let min = slicc_browsers::catalog().min_chromium_major;
    chrome_major(user_agent).is_some_and(|major| major < min)
}

fn chrome_major(user_agent: &str) -> Option<u32> {
    let mut best: Option<(usize, u32)> = None;
    for marker in ["HeadlessChrome/", "Chromium/", "Chrome/"] {
        let Some(index) = user_agent.find(marker) else {
            continue;
        };
        let after = &user_agent[index + marker.len()..];
        let digits: String = after.chars().take_while(|ch| ch.is_ascii_digit()).collect();
        let Ok(major) = digits.parse::<u32>() else {
            continue;
        };
        if best.map(|(at, _)| index < at).unwrap_or(true) {
            best = Some((index, major));
        }
    }
    best.map(|(_, major)| major)
}

fn report_entries(
    endpoints: &[EndpointDraft],
    apps: &[FoundApp],
    installed: Option<&[Value]>,
) -> Vec<ReportEntry> {
    let catalog = slicc_browsers::catalog();
    let mut entries = Vec::new();
    for draft in endpoints {
        entries.push(ReportEntry {
            id: draft.id.clone(),
            kind: "endpoint",
            tier: if draft.seven_open {
                catalog.rank.tiers.seven_open
            } else {
                catalog.rank.tiers.endpoint
            },
            name: draft.name.clone(),
            version: draft.version.clone(),
            port: draft.port,
            tie: draft.host.clone(),
            priority: 0,
            sort_name: String::new(),
            endpoint: Some(EndpointDraft {
                id: draft.id.clone(),
                host: draft.host.clone(),
                port: draft.port,
                source: draft.source,
                cdp: draft.cdp.clone(),
                websocket: draft.websocket.clone(),
                name: draft.name.clone(),
                version: draft.version.clone(),
                product: draft.product.clone(),
                user_agent: draft.user_agent.clone(),
                browser: draft.browser.clone(),
                user_data_dir: draft.user_data_dir.clone(),
                profile: draft.profile.clone(),
                ws_only: draft.ws_only,
                seven_open: draft.seven_open,
                tabs: draft
                    .tabs
                    .iter()
                    .map(|tab| TabDraft {
                        id: tab.id.clone(),
                        url: tab.url.clone(),
                        title: tab.title.clone(),
                    })
                    .collect(),
            }),
            installed: None,
            installed_raw: None,
        });
    }
    if let Some(rows) = installed {
        for row in rows {
            if installed_too_old(&catalog, row) {
                continue;
            }
            entries.push(installed_entry(row, &catalog));
        }
        return entries;
    }
    for app in apps {
        let (id, engine, debug) = installed_identity(app);
        let tier = if engine == "electron" {
            5
        } else if id == "chrome" {
            3
        } else {
            4
        };
        entries.push(ReportEntry {
            id: id.clone(),
            kind: "installed",
            tier,
            name: app.name.clone(),
            version: None,
            port: 0,
            tie: app.path.clone(),
            priority: catalog.priority(&id),
            sort_name: app.name.to_ascii_lowercase(),
            endpoint: None,
            installed: Some(InstalledDraft {
                id,
                name: app.name.clone(),
                engine,
                path: app.path.clone(),
                bundle_id: app.bundle_id.clone(),
                debug_support: debug,
            }),
            installed_raw: None,
        });
    }
    entries
}

fn installed_entry(row: &Value, catalog: &slicc_browsers::Catalog) -> ReportEntry {
    let id = row
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let name = row
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let engine = row.get("engine").and_then(|value| value.as_str()).unwrap_or("");
    let path = row
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let version = row
        .get("version")
        .and_then(|value| value.as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    let tier = if engine == "electron" {
        catalog.rank.tiers.electron
    } else if id == catalog.rank.chrome_id {
        catalog.rank.tiers.chrome
    } else {
        catalog.rank.tiers.chromium
    };
    ReportEntry {
        id: id.clone(),
        kind: "installed",
        tier,
        name: name.clone(),
        version,
        port: 0,
        tie: path,
        priority: catalog.priority(&id),
        sort_name: name.to_ascii_lowercase(),
        endpoint: None,
        installed: None,
        installed_raw: Some(row.clone()),
    }
}

fn installed_too_old(catalog: &slicc_browsers::Catalog, row: &Value) -> bool {
    let Some(id) = row.get("id").and_then(|value| value.as_str()) else {
        return false;
    };
    let Some(browser) = catalog.browser(id) else {
        return false;
    };
    if !browser.chromium_version {
        return false;
    }
    let Some(version) = row.get("version").and_then(|value| value.as_str()) else {
        return false;
    };
    let digits: String = version
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    let Ok(major) = digits.parse::<u32>() else {
        return false;
    };
    major < catalog.min_chromium_major
}

fn installed_identity(app: &FoundApp) -> (String, &'static str, &'static str) {
    let debug = if app.name == "Claude" || app.name == "1Password" {
        "disabled"
    } else {
        "supported"
    };
    if app.kind == "electron" {
        return (format!("electron:{}", slug(&app.name)), "electron", debug);
    }
    if let Some(bundle) = app.bundle_id.as_deref() {
        if let Some(id) = bundle_id_to_slicc(bundle) {
            return (id.to_string(), "chromium", debug);
        }
    }
    let id = chromium_id_for_name(&app.name).unwrap_or("chromium");
    (id.to_string(), "chromium", debug)
}

fn bundle_id_to_slicc(bundle: &str) -> Option<&'static str> {
    BUNDLE_IDS
        .iter()
        .find(|(id, _)| *id == bundle)
        .map(|(_, slicc)| *slicc)
}

fn chromium_id_for_name(name: &str) -> Option<&'static str> {
    match name {
        "Google Chrome" | "Chrome" => Some("chrome"),
        "Google Chrome Beta" => Some("chrome-beta"),
        "Google Chrome Dev" => Some("chrome-dev"),
        "Google Chrome Canary" | "Chrome Canary" => Some("chrome-canary"),
        "Chrome for Testing" | "Google Chrome for Testing" => Some("chrome-for-testing"),
        "Brave Browser" => Some("brave"),
        "Brave Beta" | "Brave Browser Beta" => Some("brave-beta"),
        "Brave Nightly" | "Brave Browser Nightly" => Some("brave-nightly"),
        "Microsoft Edge" => Some("edge"),
        "Microsoft Edge Beta" => Some("edge-beta"),
        "Microsoft Edge Dev" => Some("edge-dev"),
        "Microsoft Edge Canary" => Some("edge-canary"),
        "Vivaldi" => Some("vivaldi"),
        "Vivaldi Snapshot" => Some("vivaldi-snapshot"),
        "Opera" => Some("opera"),
        "Arc" => Some("arc"),
        "Dia" => Some("dia"),
        "ChatGPT Atlas" => Some("atlas"),
        "Chromium" => Some("chromium"),
        _ => None,
    }
}

fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in name.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

fn sort_entries(entries: &mut [ReportEntry]) {
    entries.sort_by(|left, right| {
        left.tier.cmp(&right.tier).then_with(|| {
            match (left.kind, right.kind) {
                ("endpoint", "endpoint") => left
                    .tie
                    .cmp(&right.tie)
                    .then(left.port.cmp(&right.port))
                    .then(left.id.cmp(&right.id)),
                ("installed", "installed") => left
                    .priority
                    .cmp(&right.priority)
                    .then(left.sort_name.cmp(&right.sort_name))
                    .then(left.id.cmp(&right.id))
                    .then(left.tie.cmp(&right.tie)),
                _ => std::cmp::Ordering::Equal,
            }
        })
    });
}

fn apps_to_lines(mut apps: Vec<FoundApp>) -> Vec<String> {
    apps.sort_by(|left, right| left.name.cmp(&right.name).then(left.path.cmp(&right.path)));
    apps.into_iter()
        .map(|app| format!("{} {} {}", app.name, app.kind, app.path))
        .collect()
}

fn report_json(entries: &[ReportEntry]) -> String {
    let min = slicc_browsers::catalog().min_chromium_major;
    let mut body = format!("{{\"schema\":1,\"minChromium\":{min},\"entries\":[");
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            body.push(',');
        }
        body.push_str(&entry_json(entry));
    }
    body.push_str("]}\n");
    body
}

fn entry_json(entry: &ReportEntry) -> String {
    let mut out = String::from("{");
    let mut first = true;
    field(&mut out, &mut first, "id", &json_string(&entry.id));
    field(&mut out, &mut first, "kind", &json_string(entry.kind));
    field(&mut out, &mut first, "tier", &entry.tier.to_string());
    field(&mut out, &mut first, "name", &json_string(&entry.name));
    if let Some(version) = &entry.version {
        field(&mut out, &mut first, "version", &json_string(version));
    }
    if let Some(endpoint) = &entry.endpoint {
        field(&mut out, &mut first, "endpoint", &endpoint_json(endpoint));
    }
    if let Some(raw) = &entry.installed_raw {
        field(&mut out, &mut first, "installed", &raw.to_string());
    } else if let Some(installed) = &entry.installed {
        field(&mut out, &mut first, "installed", &installed_json(installed));
    }
    out.push('}');
    out
}

fn endpoint_json(endpoint: &EndpointDraft) -> String {
    let mut out = String::from("{");
    let mut first = true;
    field(&mut out, &mut first, "cdp", &json_string(&endpoint.cdp));
    field(
        &mut out,
        &mut first,
        "webSocketDebuggerUrl",
        &json_string(endpoint.websocket.as_deref().unwrap_or("")),
    );
    field(&mut out, &mut first, "name", &json_string(&endpoint.name));
    if let Some(version) = &endpoint.version {
        field(&mut out, &mut first, "version", &json_string(version));
    }
    if let Some(product) = &endpoint.product {
        field(&mut out, &mut first, "product", &json_string(product));
    }
    if let Some(user_agent) = &endpoint.user_agent {
        field(&mut out, &mut first, "userAgent", &json_string(user_agent));
    }
    field(&mut out, &mut first, "source", &json_string(endpoint.source));
    if let Some(browser) = &endpoint.browser {
        field(&mut out, &mut first, "browser", &json_string(browser));
    }
    if let Some(dir) = &endpoint.user_data_dir {
        field(&mut out, &mut first, "userDataDir", &json_string(dir));
    }
    if let Some(profile) = &endpoint.profile {
        field(&mut out, &mut first, "profile", &json_string(profile));
    }
    if endpoint.ws_only {
        field(&mut out, &mut first, "wsOnly", "true");
    }
    field(
        &mut out,
        &mut first,
        "sevenOpen",
        if endpoint.seven_open { "true" } else { "false" },
    );
    field(&mut out, &mut first, "sevenTabs", &tabs_json(&endpoint.tabs));
    out.push('}');
    out
}

fn installed_json(installed: &InstalledDraft) -> String {
    let mut out = String::from("{");
    let mut first = true;
    field(&mut out, &mut first, "id", &json_string(&installed.id));
    field(&mut out, &mut first, "name", &json_string(&installed.name));
    field(&mut out, &mut first, "engine", &json_string(installed.engine));
    field(&mut out, &mut first, "path", &json_string(&installed.path));
    field(
        &mut out,
        &mut first,
        "executable",
        &json_string(&installed.path),
    );
    if let Some(bundle) = &installed.bundle_id {
        field(&mut out, &mut first, "bundleId", &json_string(bundle));
    }
    field(
        &mut out,
        &mut first,
        "debugSupport",
        &json_string(installed.debug_support),
    );
    out.push('}');
    out
}

fn tabs_json(tabs: &[TabDraft]) -> String {
    let mut out = String::from("[");
    for (index, tab) in tabs.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('{');
        let mut first = true;
        field(&mut out, &mut first, "id", &json_string(&tab.id));
        field(&mut out, &mut first, "url", &json_string(&tab.url));
        field(&mut out, &mut first, "title", &json_string(&tab.title));
        out.push('}');
    }
    out.push(']');
    out
}

fn field(out: &mut String, first: &mut bool, key: &str, raw: &str) {
    if !*first {
        out.push(',');
    }
    *first = false;
    out.push('"');
    out.push_str(key);
    out.push_str("\":");
    out.push_str(raw);
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn run_host(invocation: &Invocation, cdp_env: Option<&str>) -> Output {
    let chrome_path = std::env::var("CHROME_PATH").ok();
    let local_app_data = std::env::var("LOCALAPPDATA").ok();
    let program_files = std::env::var("PROGRAMFILES").ok();
    let program_files_x86 = std::env::var("PROGRAMFILES(X86)").ok();
    let home = std::env::var("HOME")
        .ok()
        .or_else(|| std::env::var("USERPROFILE").ok());
    let xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let appdata = std::env::var("APPDATA").ok();
    let process_text = read_process_text();
    let os = host_os().unwrap_or(HostOs::Linux);
    let launch = LaunchQuery {
        os,
        bundles: &mdfind_bundle,
        exists: &path_exists,
        chrome_path: chrome_path.as_deref(),
        local_app_data: local_app_data.as_deref(),
        program_files: program_files.as_deref(),
        program_files_x86: program_files_x86.as_deref(),
    };
    let io = ListIo {
        scan_host: host_os().is_some(),
        os,
        home: home.as_deref(),
        xdg_config_home: xdg.as_deref(),
        appdata: appdata.as_deref(),
        process_text: &process_text,
        windows_processes: cfg!(target_os = "windows"),
        self_pid: std::process::id(),
        launch,
        http: &http_real,
        read: &read_real,
        list_dir: &list_dir_real,
        targets: &live_targets,
        flag_urls: &[],
        installed: None,
    };
    assemble(invocation, cdp_env, &io)
}

#[cfg(target_arch = "wasm32")]
fn run_guest(invocation: &Invocation, cdp_env: Option<&str>) -> Output {
    let launch = LaunchQuery {
        os: HostOs::Linux,
        bundles: &|_| Vec::new(),
        exists: &|_| false,
        chrome_path: None,
        local_app_data: None,
        program_files: None,
        program_files_x86: None,
    };
    let io = ListIo {
        scan_host: false,
        os: HostOs::Linux,
        home: None,
        xdg_config_home: None,
        appdata: None,
        process_text: "",
        windows_processes: false,
        self_pid: 0,
        launch,
        http: &http_real,
        read: &|_| None,
        list_dir: &|_| Vec::new(),
        targets: &live_targets,
        flag_urls: &[],
        installed: None,
    };
    assemble(invocation, cdp_env, &io)
}

fn live_targets(url: &str) -> Result<Probe, ()> {
    let mut cdp = Cdp::connect(url).map_err(|_| ())?;
    let targets = cdp
        .call("Target.getTargets", serde_json::json!({}), None)
        .map_err(|_| ());
    Ok(Probe {
        websocket_url: url.to_string(),
        targets,
    })
}

fn http_real(url: &str) -> HttpResult {
    match crate::net::http_get(url) {
        Ok(response) => HttpResult::Response {
            status: 200,
            body: String::from_utf8_lossy(&response.body).into_owned(),
        },
        Err(err) => match split_http_status(&err) {
            Some((status, body)) => HttpResult::Response { status, body },
            None => HttpResult::Transport,
        },
    }
}

fn split_http_status(err: &str) -> Option<(u16, String)> {
    let rest = err.strip_prefix("HTTP ")?;
    let (status, body) = rest.split_once('\n')?;
    let status = status.trim().parse().ok()?;
    let body = body.strip_suffix('\n').unwrap_or(body).to_string();
    Some((status, body))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_real(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn list_dir_real(path: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            names.push(name.to_string());
        }
    }
    names
}

#[cfg(all(not(target_arch = "wasm32"), target_os = "windows"))]
fn read_process_text() -> String {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_Process | Select-Object ProcessId, CommandLine | ConvertTo-Json -Compress",
        ])
        .output();
    let Ok(output) = output else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "windows")))]
fn read_process_text() -> String {
    let output = std::process::Command::new("ps")
        .args(["-ax", "-o", "pid=", "-o", "command="])
        .output();
    let Ok(output) = output else {
        return String::new();
    };
    if !output.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(all(not(target_arch = "wasm32"), target_os = "macos"))]
fn host_os() -> Option<HostOs> {
    Some(HostOs::Macos)
}

#[cfg(all(not(target_arch = "wasm32"), target_os = "linux"))]
fn host_os() -> Option<HostOs> {
    Some(HostOs::Linux)
}

#[cfg(all(not(target_arch = "wasm32"), target_os = "windows"))]
fn host_os() -> Option<HostOs> {
    Some(HostOs::Windows)
}

#[cfg(all(
    not(target_arch = "wasm32"),
    not(any(target_os = "macos", target_os = "linux", target_os = "windows"))
))]
fn host_os() -> Option<HostOs> {
    None
}

#[cfg(all(not(target_arch = "wasm32"), target_os = "macos"))]
fn mdfind_bundle(bundle_id: &str) -> Vec<String> {
    let output = std::process::Command::new("mdfind")
        .arg(format!("kMDItemCFBundleIdentifier == {bundle_id}"))
        .output()
        .ok();
    let Some(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "macos")))]
fn mdfind_bundle(bundle_id: &str) -> Vec<String> {
    let _ = bundle_id;
    Vec::new()
}

#[cfg(not(target_arch = "wasm32"))]
fn path_exists(path: &str) -> bool {
    std::path::Path::new(path).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_listing_keeps_every_section() {
        let text = render(&[], &[], &[]);
        assert_eq!(
            text,
            "connected\nnone\n\ndebug ports\nnone\n\nlaunchable\nnone\n"
        );
    }

    #[test]
    fn browser_targets_keep_target_order() {
        let targets = json!({
            "targetInfos": [
                {"type": "page", "targetId": "page-1", "url": "https://example.test/page"},
                {"type": "browser", "targetId": "browser-a", "url": ""},
                {"type": "iframe", "targetId": "frame-1", "url": "https://example.test/frame"},
                {"type": "browser", "targetId": "", "url": "about:blank"},
                {"type": "browser", "targetId": "browser-b", "url": "https://example.test/browser"}
            ]
        });
        let lines = browser_lines("ws://127.0.0.1:9333/devtools/browser/socket", &targets);
        assert_eq!(
            lines,
            vec![
                "browser browser-a ".to_string(),
                "browser browser-b https://example.test/browser".to_string(),
            ]
        );
    }

    #[test]
    fn an_open_socket_without_a_browser_target_names_the_socket() {
        let targets = json!({
            "result": {
                "targetInfos": [
                    {"type": "page", "targetId": "page-1", "url": "https://example.test"}
                ]
            }
        });
        let lines = browser_lines("ws://127.0.0.1:9333/devtools/browser/socket", &targets);
        assert_eq!(
            lines,
            vec!["browser - ws://127.0.0.1:9333/devtools/browser/socket".to_string()]
        );
    }

    #[test]
    fn a_failed_probe_leaves_the_other_sections() {
        let ports = vec!["pid 42 port 9333 /usr/bin/chromium".to_string()];
        let apps = vec!["Chromium browser /usr/bin/chromium".to_string()];
        let failed = render(&connected_lines_from_probe(Err(())), &ports, &apps);
        assert!(failed.starts_with("connected\nnone\n\n"));
        assert!(failed.contains("debug ports\npid 42 port 9333 /usr/bin/chromium\n"));
        assert!(failed.contains("launchable\nChromium browser /usr/bin/chromium\n"));
        assert_eq!(failed.matches("none").count(), 1);

        let socket = connected_lines_from_probe(Ok(Probe {
            websocket_url: "ws://127.0.0.1:9333/devtools/browser/socket".to_string(),
            targets: Err(()),
        }));
        let text = render(&socket, &ports, &[]);
        assert!(text.contains("browser - ws://127.0.0.1:9333/devtools/browser/socket"));
        assert!(text.contains("debug ports\npid 42 port 9333 /usr/bin/chromium"));
        assert!(text.contains("launchable\nnone"));
    }

    #[test]
    fn unix_ports_skip_wrappers_pid_zero_and_this_process() {
        let text = "\
   0 /usr/bin/chromium --remote-debugging-port=9333
  10 zsh -c '/usr/bin/chromium --remote-debugging-port=9333'
  11 /usr/local/bin/node server.js --remote-debugging-port=9444
  12 npx tsx app.ts --remote-debugging-port=9555
  13 /usr/bin/npm run start -- --remote-debugging-port=9666
  14 /usr/bin/env chromium --remote-debugging-port=9777
  15 /usr/bin/pwsh -Command '/usr/bin/chromium --remote-debugging-port=9111'
  16 powershell.exe -Command chrome --remote-debugging-port=9112
  17 cmd.exe /c chrome --remote-debugging-port=9113
  45 /usr/bin/chromium --remote-debugging-port=0 --user-data-dir=/tmp/p
  99 /usr/bin/chromium --remote-debugging-port=9888
  42 /usr/bin/chromium --remote-debugging-port 9333 --user-data-dir=/tmp/cdp
  43 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome --remote-debugging-port=9444 --user-data-dir=/tmp/chrome
  44 /bin/bash /tmp/start.sh
";
        let lines = debug_ports_unix(text, 99);
        assert_eq!(
            lines,
            vec![
                "pid 42 port 9333 /usr/bin/chromium".to_string(),
                "pid 43 port 9444 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn windows_ports_use_process_id_and_command_line() {
        let text = r#"[
            {"ProcessId": 0, "CommandLine": "chrome.exe --remote-debugging-port=1"},
            {"ProcessId": 8, "CommandLine": "node.exe app.js --remote-debugging-port=2"},
            {"ProcessId": 9, "CommandLine": "C:\\Windows\\System32\\cmd.exe /c chrome.exe --remote-debugging-port=3"},
            {"ProcessId": 15, "CommandLine": "cmd.exe /c chrome.exe --remote-debugging-port=4"},
            {"ProcessId": 16, "CommandLine": "powershell.exe -NoProfile chrome.exe --remote-debugging-port=5"},
            {"ProcessId": 17, "CommandLine": "C:\\Windows\\System32\\pwsh.exe -Command chrome.exe --remote-debugging-port=6"},
            {"ProcessId": 10, "CommandLine": "\"C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe\" --remote-debugging-port=9333"},
            {"ProcessId": 11, "CommandLine": null}
        ]"#;
        let lines = debug_ports_windows(text, 9);
        assert_eq!(
            lines,
            vec![
                "pid 10 port 9333 C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"
                    .to_string()
            ]
        );
        let one = r#"{"ProcessId":"12","CommandLine":"chrome.exe --remote-debugging-port=9334"}"#;
        assert_eq!(
            debug_ports_windows(one, 0),
            vec!["pid 12 port 9334 chrome.exe".to_string()]
        );
        assert!(debug_ports_windows("not json", 1).is_empty());
    }

    #[test]
    fn macos_launchables_use_bundle_hits_then_known_paths() {
        let apps = launchables(&LaunchQuery {
            os: HostOs::Macos,
            bundles: &|bundle_id| match bundle_id {
                "com.google.Chrome" => vec![
                    "/missing/Google Chrome.app".to_string(),
                    "/tmp/not-an-app".to_string(),
                    "/Applications/Google Chrome.app".to_string(),
                ],
                "com.tinyspeck.slackmacgap" => {
                    vec!["/Applications/Slack.app".to_string()]
                }
                "com.brave.Browser" => vec!["/Applications/Brave Browser.app".to_string()],
                _ => Vec::new(),
            },
            exists: &|path| {
                matches!(
                    path,
                    "/Applications/Google Chrome.app"
                        | "/Applications/Slack.app"
                        | "/Applications/Brave Browser.app"
                        | "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
                        | "/opt/chrome"
                )
            },
            chrome_path: Some("/opt/chrome"),
            local_app_data: None,
            program_files: None,
            program_files_x86: None,
        });
        assert_eq!(
            apps,
            vec![
                "Brave Browser browser /Applications/Brave Browser.app".to_string(),
                "Chrome browser /opt/chrome".to_string(),
                "Google Chrome browser /Applications/Google Chrome.app".to_string(),
                "Google Chrome browser /Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
                    .to_string(),
                "Slack electron /Applications/Slack.app".to_string(),
            ]
        );
    }

    #[test]
    fn every_known_bundle_is_listed_once() {
        let apps = launchables(&LaunchQuery {
            os: HostOs::Macos,
            bundles: &|bundle_id| vec![format!("/Applications/{bundle_id}.app")],
            exists: &|path| path.ends_with(".app"),
            chrome_path: None,
            local_app_data: None,
            program_files: None,
            program_files_x86: None,
        });
        assert_eq!(apps.len(), CHROMIUM_BROWSERS.len() + ELECTRON_APPS.len());
        assert!(apps
            .iter()
            .any(|line| line == "Google Chrome browser /Applications/com.google.Chrome.app"));
        assert!(apps
            .iter()
            .any(|line| line
                == "Signal electron /Applications/org.whispersystems.signal-desktop.app"));
        assert!(apps.iter().any(|line| line.contains(" electron ")));
        let names = app_names(&apps);
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn duplicate_paths_keep_the_first_name() {
        let apps = launchables(&LaunchQuery {
            os: HostOs::Macos,
            bundles: &|bundle_id| {
                if bundle_id == "com.google.Chrome" || bundle_id == "com.google.Chrome.beta" {
                    vec!["/Applications/Google Chrome.app".to_string()]
                } else {
                    Vec::new()
                }
            },
            exists: &|path| path == "/Applications/Google Chrome.app",
            chrome_path: Some("/Applications/Google Chrome.app"),
            local_app_data: None,
            program_files: None,
            program_files_x86: None,
        });
        assert_eq!(
            apps,
            vec!["Google Chrome browser /Applications/Google Chrome.app".to_string()]
        );
    }

    #[test]
    fn linux_and_windows_use_installed_chrome_paths() {
        let linux = launchables(&LaunchQuery {
            os: HostOs::Linux,
            bundles: &|_| panic!("mdfind is not used off macos"),
            exists: &|path| path == "/usr/bin/google-chrome" || path == "/usr/bin/chromium-browser",
            chrome_path: Some("/usr/bin/google-chrome"),
            local_app_data: None,
            program_files: None,
            program_files_x86: None,
        });
        assert_eq!(
            linux,
            vec![
                "Chromium browser /usr/bin/chromium-browser".to_string(),
                "Google Chrome browser /usr/bin/google-chrome".to_string(),
            ]
        );
        let windows = launchables(&LaunchQuery {
            os: HostOs::Windows,
            bundles: &|_| panic!("mdfind is not used off macos"),
            exists: &|path| path.ends_with("chrome.exe"),
            chrome_path: Some(r"D:\chrome\chrome.exe"),
            local_app_data: Some(r"C:\Users\me\AppData\Local"),
            program_files: Some(r"C:\Program Files"),
            program_files_x86: Some(""),
        });
        assert_eq!(
            windows,
            vec![
                r"Chrome browser D:\chrome\chrome.exe".to_string(),
                r"Google Chrome browser C:\Program Files\Google\Chrome\Application\chrome.exe"
                    .to_string(),
                r"Google Chrome browser C:\Users\me\AppData\Local\Google\Chrome\Application\chrome.exe"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn list_help_documents_the_three_sections() {
        let output = crate::execute(&["list".to_string(), "--help".to_string()], None);
        assert_eq!(output.code, 0);
        assert_eq!(output.stderr, "");
        assert!(output.stdout.contains("connected"));
        assert!(output.stdout.contains("debug ports"));
        assert!(output.stdout.contains("launchable"));
        assert!(output.stdout.contains("browser <targetId> <url>"));
        assert!(output.stdout.contains("pid <pid> port <port> <executable>"));
        assert!(output.stdout.contains("<name> <kind> <path>"));
        assert!(output.stdout.contains("--json"));
        assert!(output.stdout.contains("devtools-active-port"));
        assert!(output.stdout.contains("`flag`"));
        assert!(output.stdout.contains("`process`"));
        let flagged = crate::execute(
            &["list".to_string(), "--json".to_string(), "--help".to_string()],
            None,
        );
        assert_eq!(flagged.code, 0);
        assert!(flagged.stdout.contains("print one JSON document and no text sections"));
        let extra = crate::execute(
            &[
                "list".to_string(),
                "--json".to_string(),
                "extra".to_string(),
            ],
            None,
        );
        assert_eq!(extra.code, 1);
        assert!(extra.stderr.contains("too many arguments"));
        let global = crate::execute(&["--help".to_string()], None);
        assert!(global.stdout.contains("Browsers:"));
        let expected = format!(
            "  {:<28}{}",
            "list", "list connected browsers, debug ports, and launchables"
        );
        assert!(
            global.stdout.contains(&expected),
            "{expected}\n{}",
            global.stdout
        );
        assert!(global.stdout.contains(
            "The host build launches a browser when no debugger is already connected, and the wasm build only opens a tab.\n"
        ));
        assert!(global.stdout.contains(
            "The host never dials 127.0.0.1:9222-9229 unless --cdp or SLICC_CDP_URL names that URL."
        ));
        let pending = global.stdout.split("Not implemented yet:").nth(1).unwrap();
        let pending_names: Vec<&str> = pending
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .filter(|name| !name.is_empty() && !name.ends_with(':'))
            .collect();
        assert!(!pending_names.contains(&"list"));
    }

    #[test]
    fn list_rejects_arguments_before_any_discovery() {
        let output = crate::execute(&["list".to_string(), "extra".to_string()], None);
        assert_eq!(output.code, 1);
        assert!(output.stderr.contains("too many arguments"));
        let unknown = crate::execute(&["list".to_string(), "--launch".to_string()], None);
        assert_eq!(unknown.code, 1);
        assert!(unknown.stderr.contains("Unknown option"));
    }

    #[test]
    fn json_skips_the_default_port_and_explicit_http_errors_keep_the_other_sections() {
        let urls = RefCell::new(Vec::new());
        let http = |url: &str| -> HttpResult {
            urls.borrow_mut().push(url.to_string());
            if url == "http://127.0.0.1:9333/json/version" {
                return HttpResult::Response {
                    status: 503,
                    body: "broken".to_string(),
                };
            }
            if url == "http://127.0.0.1:9222/json/version" {
                return HttpResult::Response {
                    status: 404,
                    body: String::new(),
                };
            }
            HttpResult::Transport
        };
        let read = |_path: &str| None;
        let list_dir = |_path: &str| Vec::new();
        let empty = run_fake(
            &list_invocation(None, true),
            None,
            HostOs::Linux,
            None,
            None,
            None,
            "",
            false,
            false,
            &http,
            &read,
            &list_dir,
        );
        assert_eq!(empty.code, 0);
        assert_eq!(empty.stderr, "");
        assert_eq!(
            empty.stdout,
            "{\"schema\":1,\"minChromium\":116,\"entries\":[]}\n"
        );
        assert!(!empty.stdout.contains("connected"));
        assert!(urls.borrow().is_empty());

        urls.borrow_mut().clear();
        let missing = run_fake(
            &list_invocation(None, false),
            None,
            HostOs::Linux,
            None,
            None,
            None,
            "",
            false,
            false,
            &http,
            &read,
            &list_dir,
        );
        assert_eq!(missing.code, 0);
        assert_eq!(missing.stderr, "");
        assert!(missing.stdout.starts_with("connected\nnone\n\n"));
        assert!(urls.borrow().is_empty());

        urls.borrow_mut().clear();
        let reserved_processes = "  7 /usr/bin/chrome --remote-debugging-port=9222\n  8 /usr/bin/chromium --remote-debugging-port=0 --user-data-dir=/tmp/seven\n";
        let reserved_read = |path: &str| -> Option<String> {
            if path == "/tmp/seven/DevToolsActivePort" {
                Some("9228\n/devtools/browser/seven\n".to_string())
            } else if path == "/home/me/.config/slicc/seven-profiles/chrome/DevToolsActivePort" {
                Some("9223\n/devtools/browser/personal\n".to_string())
            } else {
                None
            }
        };
        let reserved_list = |path: &str| {
            if path == "/home/me/.config/slicc/seven-profiles" {
                vec!["chrome".to_string()]
            } else {
                Vec::new()
            }
        };
        let reserved = run_fake(
            &list_invocation(None, false),
            None,
            HostOs::Linux,
            Some("/home/me"),
            None,
            None,
            reserved_processes,
            false,
            false,
            &http,
            &reserved_read,
            &reserved_list,
        );
        assert_eq!(reserved.code, 0);
        assert_eq!(reserved.stderr, "");
        assert!(reserved.stdout.contains("pid 7 port 9222 /usr/bin/chrome\n"));
        assert!(reserved.stdout.contains("pid 8 port 9228 /usr/bin/chromium\n"));
        assert!(urls.borrow().iter().all(|url| {
            !url.contains(":9222")
                && !url.contains(":9223")
                && !url.contains(":9228")
                && !url.contains(":9229")
        }));

        urls.borrow_mut().clear();
        let processes = "  42 /usr/bin/chromium --remote-debugging-port=9444\n";
        let failed = run_fake(
            &list_invocation(Some("http://127.0.0.1:9333"), false),
            Some("http://127.0.0.1:1"),
            HostOs::Linux,
            None,
            None,
            None,
            processes,
            false,
            true,
            &http,
            &read,
            &list_dir,
        );
        assert_eq!(failed.code, 1);
        assert_eq!(failed.stderr, "HTTP 503\nbroken\n");
        assert!(failed.stdout.contains("connected\nnone\n"));
        assert!(failed.stdout.contains("debug ports\nnone\n"));
        assert!(failed
            .stdout
            .contains("launchable\nGoogle Chrome browser /usr/bin/google-chrome\n"));
        assert!(!urls.borrow().iter().any(|url| {
            url.contains(":9222")
                || url.contains(":9223")
                || url.contains(":9224")
                || url.contains(":9225")
                || url.contains(":9226")
                || url.contains(":9227")
                || url.contains(":9228")
                || url.contains(":9229")
        }));

        urls.borrow_mut().clear();
        let from_env = run_fake(
            &list_invocation(None, false),
            Some("http://127.0.0.1:9333"),
            HostOs::Linux,
            None,
            None,
            None,
            "",
            false,
            false,
            &http,
            &read,
            &list_dir,
        );
        assert_eq!(from_env.code, 1);
        assert_eq!(from_env.stderr, "HTTP 503\nbroken\n");
        assert!(from_env.stdout.contains("debug ports\nnone\n"));
        assert!(from_env.stdout.contains("launchable\nnone\n"));
    }

    #[test]
    fn devtools_active_port_uses_seven_profiles_and_drops_stale_files() {
        let reads = RefCell::new(Vec::new());
        let lists = RefCell::new(Vec::new());
        let http = |url: &str| -> HttpResult {
            let body = |port: u32, path: &str, major: u32, browser: &str| {
                format!(
                    "{{\"Browser\":\"{browser}/{major}.0.0.0\",\"User-Agent\":\"Mozilla/5.0 Chrome/{major}.0.0.0\",\"webSocketDebuggerUrl\":\"ws://127.0.0.1:{port}{path}\"}}"
                )
            };
            match url {
                "http://127.0.0.1:9333/json/version" => HttpResult::Response {
                    status: 200,
                    body: body(9333, "/devtools/browser/abc", 120, "Chrome"),
                },
                "http://127.0.0.1:9334/json/version" => HttpResult::Response {
                    status: 200,
                    body: body(9334, "/devtools/browser/edg", 121, "Edg"),
                },
                "http://127.0.0.1:9335/json/version" => HttpResult::Response {
                    status: 200,
                    body: body(9335, "/devtools/browser/other", 122, "Chrome"),
                },
                "http://127.0.0.1:9333/json/list" => HttpResult::Response {
                    status: 200,
                    body: r#"[{"type":"page","id":"T1","url":"https://app.sliccy.ai/home","title":"App"},{"type":"page","id":"W","url":"https://www.sliccy.ai/"},{"type":"page","id":"D","url":"https://a.b.sliccy.ai/"},{"type":"page","id":"H","url":"http://app.sliccy.ai/"},{"type":"iframe","id":"I","url":"https://app.sliccy.ai/frame"}]"#.to_string(),
                },
                "http://127.0.0.1:9334/json/list" => HttpResult::Response {
                    status: 500,
                    body: "no".to_string(),
                },
                _ => HttpResult::Transport,
            }
        };
        let read = |path: &str| -> Option<String> {
            reads.borrow_mut().push(path.to_string());
            match path {
                "/home/me/.config/slicc/seven-profiles/chrome/DevToolsActivePort" => {
                    Some("9333\n/devtools/browser/abc\n".to_string())
                }
                "/home/me/snap/slicc/common/slicc/seven-profiles/edge/DevToolsActivePort" => {
                    Some("9334\r\n/devtools/browser/edg\r\n".to_string())
                }
                "/tmp/stale/DevToolsActivePort" => {
                    Some("9335\n/devtools/browser/abc\n".to_string())
                }
                "/home/me/.config/slicc/profiles/chrome/DevToolsActivePort" => {
                    Some("9339\n/devtools/browser/six\n".to_string())
                }
                "/home/me/.config/slicc/seven-profiles/.migrated-from-profiles/DevToolsActivePort"
                | "/home/me/.config/slicc/seven-profiles/.hidden/DevToolsActivePort"
                | "/home/me/.config/slicc/seven-profiles/browser-coding-agent-chrome/DevToolsActivePort"
                | "/home/me/.config/slicc/seven-profiles/chrome..old/DevToolsActivePort"
                | "/home/me/.config/slicc/seven-profiles/has space/DevToolsActivePort" => {
                    Some("9777\n/devtools/browser/nope\n".to_string())
                }
                _ => None,
            }
        };
        let list_dir = |path: &str| -> Vec<String> {
            lists.borrow_mut().push(path.to_string());
            match path {
                "/home/me/.config/slicc/seven-profiles" => vec![
                    ".migrated-from-profiles".to_string(),
                    ".hidden".to_string(),
                    "browser-coding-agent-chrome".to_string(),
                    "chrome".to_string(),
                    "chrome..old".to_string(),
                    "has space".to_string(),
                ],
                "/home/me/snap" => vec!["slicc".to_string()],
                "/home/me/snap/slicc/common/slicc/seven-profiles" => vec!["edge".to_string()],
                _ => Vec::new(),
            }
        };
        let processes = "\
  7 /usr/bin/chromium --remote-debugging-port=0 --user-data-dir=/tmp/stale
  8 /usr/bin/chromium --remote-debugging-port=0
  9 /usr/bin/chromium --remote-debugging-port=9333 --user-data-dir=/tmp/also
";
        let output = run_fake(
            &list_invocation(None, true),
            None,
            HostOs::Linux,
            Some("/home/me"),
            Some("relative/config"),
            None,
            processes,
            false,
            false,
            &http,
            &read,
            &list_dir,
        );
        assert_eq!(output.code, 0);
        assert_eq!(output.stderr, "");
        assert!(!output.stdout.contains("connected"));
        assert!(lists
            .borrow()
            .iter()
            .all(|path| !path.contains("/slicc/profiles") && !path.contains("browser-coding-agent-chrome")));
        assert!(reads
            .borrow()
            .iter()
            .all(|path| !path.contains("/slicc/profiles")));
        assert!(reads.borrow().iter().all(|path| {
            !path.contains("/.migrated-from-profiles/")
                && !path.contains("/.hidden/")
                && !path.contains("browser-coding-agent-chrome")
                && !path.contains("chrome..old")
                && !path.contains("has space")
        }));
        assert!(!reads.borrow().iter().any(|path| path.contains("9339") || path.ends_with("/tmp/also/DevToolsActivePort")));
        let value: Value = serde_json::from_str(output.stdout.trim()).unwrap();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["minChromium"], 116);
        let entries = value["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["id"], "cdp:127.0.0.1:9333");
        assert_eq!(entries[0]["kind"], "endpoint");
        assert_eq!(entries[0]["tier"], 1);
        assert_eq!(entries[0]["endpoint"]["source"], "devtools-active-port");
        assert!(entries[0]["endpoint"].get("browser").is_none());
        assert_eq!(entries[0]["endpoint"]["profile"], "chrome");
        assert_eq!(entries[0]["endpoint"]["sevenOpen"], true);
        assert_eq!(entries[0]["endpoint"]["sevenTabs"].as_array().unwrap().len(), 1);
        assert_eq!(entries[0]["endpoint"]["sevenTabs"][0]["id"], "T1");
        assert_eq!(entries[0]["endpoint"]["name"], "Chrome");
        assert_eq!(entries[0]["name"], "Chrome");
        assert_eq!(entries[0]["version"], "120.0.0.0");
        assert_eq!(entries[1]["id"], "cdp:127.0.0.1:9334");
        assert_eq!(entries[1]["tier"], 2);
        assert_eq!(entries[1]["endpoint"]["source"], "devtools-active-port");
        assert!(entries[1]["endpoint"].get("browser").is_none());
        assert_eq!(entries[1]["endpoint"]["profile"], "edge");
        assert_eq!(entries[1]["endpoint"]["name"], "Microsoft Edge");
        assert_eq!(entries[1]["endpoint"]["sevenOpen"], false);
        assert!(entries[1]["endpoint"]["sevenTabs"].as_array().unwrap().is_empty());
        assert!(output.stdout.contains("\"webSocketDebuggerUrl\":\"ws://127.0.0.1:9333/devtools/browser/abc\""));

        let text = run_fake(
            &list_invocation(None, false),
            None,
            HostOs::Linux,
            Some("/home/me"),
            Some("/xdg"),
            None,
            processes,
            false,
            false,
            &http,
            &read,
            &list_dir,
        );
        assert_eq!(text.code, 0);
        assert!(text.stdout.contains("pid 9 port 9333 /usr/bin/chromium"));
        assert!(!text.stdout.contains("port 0"));
        assert!(!text.stdout.contains("9335"));
        assert!(lists.borrow().iter().any(|path| path == "/xdg/slicc/seven-profiles"));
    }

    #[test]
    fn json_entries_rank_like_slicc_browsers_and_skip_old_chrome() {
        let http = |url: &str| -> HttpResult {
            let doc = |port: u32, major: u32| {
                format!(
                    "{{\"Browser\":\"Chrome/{major}.0.0.0\",\"User-Agent\":\"Mozilla/5.0 Chrome/{major}.0.0.0\",\"webSocketDebuggerUrl\":\"ws://127.0.0.1:{port}/devtools/browser/live\"}}"
                )
            };
            match url {
                "http://127.0.0.1:9333/json/version" => HttpResult::Response {
                    status: 200,
                    body: doc(9333, 120),
                },
                "http://127.0.0.1:9444/json/version" => HttpResult::Response {
                    status: 200,
                    body: doc(9444, 115),
                },
                "http://127.0.0.1:9555/json/version"
                | "http://localhost:9555/json/version" => HttpResult::Response {
                    status: 200,
                    body: doc(9555, 116),
                },
                "http://127.0.0.1:9333/json/list" => HttpResult::Response {
                    status: 200,
                    body: r#"[{"type":"page","id":"T","url":"https://lab.sliccy.ai/x","title":"Lab"}]"#.to_string(),
                },
                "http://127.0.0.1:9555/json/list" => HttpResult::Transport,
                "http://127.0.0.1:9666/json/version" => HttpResult::Response {
                    status: 200,
                    body: r#"{"Browser":"HeadlessChrome/130.0.1","User-Agent":"Mozilla/5.0 HeadlessChrome/130.0.1","webSocketDebuggerUrl":"ws://127.0.0.1:9666/devtools/browser/h"}"#.to_string(),
                },
                _ => HttpResult::Transport,
            }
        };
        let read = |path: &str| -> Option<String> {
            if path == "/Users/me/Library/Application Support/slicc/seven-profiles/chrome/DevToolsActivePort" {
                Some("9333\n/devtools/browser/live\n".to_string())
            } else {
                None
            }
        };
        let list_dir = |path: &str| -> Vec<String> {
            if path == "/Users/me/Library/Application Support/slicc/seven-profiles" {
                vec!["chrome".to_string()]
            } else {
                Vec::new()
            }
        };
        let processes = "\
  42 /usr/bin/chromium --remote-debugging-port=9333
  43 /usr/bin/chromium --remote-debugging-port=9444
  44 /usr/bin/chromium --remote-debugging-port=9555
  45 /usr/bin/chromium --remote-debugging-port=9666
";
        let output = run_fake(
            &list_invocation(Some("http://localhost:9555"), true),
            None,
            HostOs::Macos,
            Some("/Users/me"),
            None,
            None,
            processes,
            false,
            true,
            &http,
            &read,
            &list_dir,
        );
        assert_eq!(output.code, 0);
        let value: Value = serde_json::from_str(output.stdout.trim()).unwrap();
        let ids: Vec<&str> = value["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![
                "cdp:127.0.0.1:9333",
                "cdp:127.0.0.1:9555",
                "cdp:127.0.0.1:9666",
                "chrome",
                "brave",
                "electron:1password",
                "electron:slack",
            ]
        );
        let entries = value["entries"].as_array().unwrap();
        assert_eq!(entries[0]["tier"], 1);
        assert_eq!(entries[0]["endpoint"]["source"], "devtools-active-port");
        assert_eq!(entries[1]["tier"], 2);
        assert_eq!(entries[1]["endpoint"]["source"], "flag");
        assert_eq!(entries[1]["endpoint"]["cdp"], "http://127.0.0.1:9555");
        assert_eq!(entries[1]["endpoint"]["sevenOpen"], false);
        assert!(entries[1]["endpoint"]["sevenTabs"].as_array().unwrap().is_empty());
        assert_eq!(entries[2]["endpoint"]["name"], "Chrome (headless)");
        assert_eq!(entries[2]["endpoint"]["source"], "process");
        assert_eq!(entries[2]["version"], "130.0.1");
        assert_eq!(entries[3]["tier"], 3);
        assert_eq!(entries[3]["installed"]["engine"], "chromium");
        assert_eq!(entries[3]["installed"]["debugSupport"], "supported");
        assert_eq!(
            entries[3]["installed"]["path"],
            "/Applications/Google Chrome.app"
        );
        assert!(entries[3].get("version").is_none());
        assert!(entries[3]["installed"].get("version").is_none());
        assert_eq!(entries[4]["tier"], 4);
        assert_eq!(entries[4]["installed"]["id"], "brave");
        assert_eq!(entries[5]["tier"], 5);
        assert_eq!(entries[5]["installed"]["debugSupport"], "disabled");
        assert_eq!(entries[6]["installed"]["engine"], "electron");
        assert_eq!(entries[6]["installed"]["id"], "electron:slack");
        let text = run_fake(
            &list_invocation(None, false),
            None,
            HostOs::Macos,
            Some("/Users/me"),
            None,
            None,
            processes,
            false,
            true,
            &http,
            &read,
            &list_dir,
        );
        assert!(text.stdout.contains("pid 43 port 9444 /usr/bin/chromium"));
        assert!(text.stdout.contains("connected\n"));
        assert!(text.stdout.contains("debug ports\n"));
        assert!(text.stdout.contains("launchable\n"));
    }

    #[test]
    fn windows_profile_root_is_appdata_seven_profiles() {
        let lists = RefCell::new(Vec::new());
        let http = |_url: &str| HttpResult::Transport;
        let read = |_path: &str| None;
        let list_dir = |path: &str| -> Vec<String> {
            lists.borrow_mut().push(path.to_string());
            Vec::new()
        };
        let output = run_fake(
            &list_invocation(None, true),
            None,
            HostOs::Windows,
            Some(r"C:\Users\me"),
            None,
            Some(r"C:\Users\me\AppData\Roaming"),
            "",
            true,
            false,
            &http,
            &read,
            &list_dir,
        );
        assert_eq!(output.code, 0);
        assert_eq!(
            output.stdout,
            "{\"schema\":1,\"minChromium\":116,\"entries\":[]}\n"
        );
        assert!(lists.borrow().iter().any(|path| {
            path == r"C:\Users\me\AppData\Roaming\slicc\seven-profiles"
        }));
        assert!(lists
            .borrow()
            .iter()
            .all(|path| !path.contains(r"\slicc\profiles")));
    }

    #[test]
    fn port_zero_skips_six_profiles_and_emits_an_empty_debugger_url() {
        let reads = RefCell::new(Vec::new());
        let http = |url: &str| -> HttpResult {
            let doc = |port: u32, path: &str| {
                format!(
                    "{{\"Browser\":\"Chrome/120.0.0.0\",\"User-Agent\":\"Mozilla/5.0 Chrome/120.0.0.0\",\"webSocketDebuggerUrl\":\"ws://127.0.0.1:{port}{path}\"}}"
                )
            };
            match url {
                "http://127.0.0.1:9333/json/version" => HttpResult::Response {
                    status: 200,
                    body: doc(9333, "/devtools/browser/live"),
                },
                "http://127.0.0.1:9334/json/version" => HttpResult::Response {
                    status: 200,
                    body: doc(9334, "/devtools/browser/brave"),
                },
                "http://127.0.0.1:9335/json/version" => HttpResult::Response {
                    status: 200,
                    body: doc(9335, "/devtools/browser/opt"),
                },
                "http://127.0.0.1:9336/json/version" => HttpResult::Response {
                    status: 200,
                    body: doc(9336, "/devtools/browser/prof"),
                },
                "http://127.0.0.1:9666/json/version" => HttpResult::Response {
                    status: 200,
                    body: doc(9666, "/devtools/browser/fixed"),
                },
                _ => HttpResult::Transport,
            }
        };
        let read = |path: &str| -> Option<String> {
            reads.borrow_mut().push(path.to_string());
            match path {
                "/tmp/live/DevToolsActivePort" => Some("9333\n/devtools/browser/live\n".to_string()),
                "/home/me/.config/slicc/seven-profiles/brave/DevToolsActivePort" => {
                    Some("9334\n/devtools/browser/brave\n".to_string())
                }
                "/opt/slicc-profiles/data/DevToolsActivePort" => {
                    Some("9335\n/devtools/browser/opt\n".to_string())
                }
                "/home/me/.config/slicc/seven-profiles/chrome/DevToolsActivePort" => {
                    Some("9336\n/devtools/browser/prof\n".to_string())
                }
                "/tmp/fixed-only/DevToolsActivePort" => {
                    Some("9404\n/devtools/browser/nope\n".to_string())
                }
                "/Users/me/Library/Application Support/Slicc/profiles/browser-coding-agent-chrome/DevToolsActivePort" => {
                    Some("9401\n/devtools/browser/six\n".to_string())
                }
                "/home/me/.config/slicc/profiles/chrome/DevToolsActivePort" => {
                    Some("9402\n/devtools/browser/six\n".to_string())
                }
                "C:\\Users\\me\\AppData\\Roaming\\SLICC\\Profiles\\chrome\\DevToolsActivePort" => {
                    Some("9403\n/devtools/browser/six\n".to_string())
                }
                _ => None,
            }
        };
        let list_dir = |path: &str| -> Vec<String> {
            if path == "/home/me/.config/slicc/seven-profiles" {
                vec!["chrome".to_string()]
            } else {
                Vec::new()
            }
        };
        let processes = "\
  21 /usr/bin/chromium --remote-debugging-port=0 --user-data-dir=\"/Users/me/Library/Application Support/Slicc/profiles/browser-coding-agent-chrome\"
  22 /usr/bin/chromium --remote-debugging-port=0 --user-data-dir=/home/me/.config/slicc/profiles/chrome
  23 /usr/bin/chromium --remote-debugging-port=0 --user-data-dir=C:\\Users\\me\\AppData\\Roaming\\SLICC\\Profiles\\chrome
  24 /usr/bin/chromium --remote-debugging-port=0 --user-data-dir=/tmp/live
  25 /usr/bin/chromium --remote-debugging-port=0 --user-data-dir=/home/me/.config/slicc/seven-profiles/brave
  26 /usr/bin/chromium --remote-debugging-port=0 --user-data-dir=/opt/slicc-profiles/data
  27 /usr/bin/chromium --remote-debugging-port=0 --user-data-dir=/tmp/missing
  28 /usr/bin/chromium --remote-debugging-port=9555 --user-data-dir=/home/me/.config/slicc/profiles/chrome
  29 /usr/bin/chromium --remote-debugging-port=9666 --user-data-dir=/tmp/fixed-only
";
        let output = run_fake(
            &list_invocation(None, true),
            None,
            HostOs::Linux,
            Some("/home/me"),
            Some("relative/config"),
            None,
            processes,
            false,
            false,
            &http,
            &read,
            &list_dir,
        );
        assert_eq!(output.code, 0);
        assert_eq!(output.stderr, "");
        let seen = reads.borrow();
        assert!(seen.iter().any(|path| path == "/tmp/live/DevToolsActivePort"));
        assert!(seen.iter().any(|path| {
            path == "/home/me/.config/slicc/seven-profiles/brave/DevToolsActivePort"
        }));
        assert!(seen
            .iter()
            .any(|path| path == "/opt/slicc-profiles/data/DevToolsActivePort"));
        assert!(seen.iter().any(|path| path == "/tmp/missing/DevToolsActivePort"));
        assert!(seen.iter().any(|path| {
            path == "/home/me/.config/slicc/seven-profiles/chrome/DevToolsActivePort"
        }));
        assert!(!seen.iter().any(|path| path == "/tmp/fixed-only/DevToolsActivePort"));
        assert!(seen.iter().all(|path| {
            let lower = path.to_ascii_lowercase();
            !lower.contains("slicc/profiles")
                && !lower.contains("slicc\\profiles")
                && !lower.contains("browser-coding-agent-chrome")
        }));
        drop(seen);
        let value: Value = serde_json::from_str(output.stdout.trim()).unwrap();
        let entries = value["entries"].as_array().unwrap();
        let ids: Vec<&str> = entries
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![
                "cdp:127.0.0.1:9333",
                "cdp:127.0.0.1:9334",
                "cdp:127.0.0.1:9335",
                "cdp:127.0.0.1:9336",
                "cdp:127.0.0.1:9666",
            ]
        );
        assert_eq!(entries[0]["endpoint"]["source"], "process");
        assert_eq!(
            entries[0]["endpoint"]["webSocketDebuggerUrl"],
            "ws://127.0.0.1:9333/devtools/browser/live"
        );
        assert_eq!(entries[1]["endpoint"]["source"], "process");
        assert_eq!(
            entries[1]["endpoint"]["userDataDir"],
            "/home/me/.config/slicc/seven-profiles/brave"
        );
        assert_eq!(entries[2]["endpoint"]["source"], "process");
        assert_eq!(entries[3]["endpoint"]["source"], "devtools-active-port");
        assert!(entries[3]["endpoint"].get("browser").is_none());
        assert_eq!(entries[3]["endpoint"]["profile"], "chrome");
        assert_eq!(entries[3]["name"], "Chrome");
        assert_eq!(entries[4]["endpoint"]["source"], "process");
        assert_eq!(
            entries[4]["endpoint"]["webSocketDebuggerUrl"],
            "ws://127.0.0.1:9666/devtools/browser/fixed"
        );
        assert!(!output.stdout.contains("9555"));
        assert!(!output.stdout.contains("9401"));
        assert!(!output.stdout.contains("9402"));
        assert!(!output.stdout.contains("9403"));
        assert!(!output.stdout.contains("browser-coding-agent-chrome"));

        let text = run_fake(
            &list_invocation(None, false),
            None,
            HostOs::Linux,
            Some("/home/me"),
            Some("relative/config"),
            None,
            processes,
            false,
            false,
            &http,
            &read,
            &list_dir,
        );
        assert_eq!(text.code, 0);
        assert!(text.stdout.contains("pid 24 port 9333 /usr/bin/chromium"));
        assert!(text.stdout.contains("pid 25 port 9334 /usr/bin/chromium"));
        assert!(!text.stdout.contains("9555"));
        assert!(text.stdout.contains("pid 29 port 9666 /usr/bin/chromium"));
        assert!(text.stdout.contains(
            "browser - ws://127.0.0.1:9336/devtools/browser/prof"
        ));
        assert!(!text.stdout.contains("port 0"));
        assert!(!text.stdout.contains("9401"));
        assert!(!text.stdout.contains("9402"));
        assert!(!text.stdout.contains("9403"));
    }

    fn list_invocation(cdp: Option<&str>, json: bool) -> Invocation {
        let mut flags = BTreeMap::new();
        if json {
            flags.insert("json".to_string(), "true".to_string());
        }
        Invocation {
            command: Some("list".to_string()),
            positionals: Vec::new(),
            flags,
            cdp: cdp.map(str::to_string),
            runtime: None,
            help: false,
            command_help: false,
            version: false,
        }
    }

    fn run_fake(
        invocation: &Invocation,
        cdp_env: Option<&str>,
        os: HostOs,
        home: Option<&str>,
        xdg: Option<&str>,
        appdata: Option<&str>,
        processes: &str,
        windows: bool,
        with_apps: bool,
        http: &dyn Fn(&str) -> HttpResult,
        read: &dyn Fn(&str) -> Option<String>,
        list_dir: &dyn Fn(&str) -> Vec<String>,
    ) -> Output {
        let launch = LaunchQuery {
            os,
            bundles: &|bundle_id| {
                if !with_apps {
                    return Vec::new();
                }
                match bundle_id {
                    "com.google.Chrome" => vec!["/Applications/Google Chrome.app".to_string()],
                    "com.brave.Browser" => vec!["/Applications/Brave Browser.app".to_string()],
                    "com.tinyspeck.slackmacgap" => vec!["/Applications/Slack.app".to_string()],
                    "com.1password.1password" => vec!["/Applications/1Password.app".to_string()],
                    _ => Vec::new(),
                }
            },
            exists: &|path| {
                with_apps
                    && matches!(
                        path,
                        "/usr/bin/google-chrome"
                            | "/Applications/Google Chrome.app"
                            | "/Applications/Brave Browser.app"
                            | "/Applications/Slack.app"
                            | "/Applications/1Password.app"
                    )
            },
            chrome_path: None,
            local_app_data: None,
            program_files: None,
            program_files_x86: None,
        };
        let io = ListIo {
            scan_host: true,
            os,
            home,
            xdg_config_home: xdg,
            appdata,
            process_text: processes,
            windows_processes: windows,
            self_pid: 1,
            launch,
            http,
            read,
            list_dir,
            targets: &|_| Err(()),
            flag_urls: &[],
            installed: None,
        };
        assemble(invocation, cdp_env, &io)
    }

    #[test]
    fn slicc_browsers_corpus_matches_list_json() {
        let catalog = slicc_browsers::catalog();
        let os = match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        };
        let host_os = match os {
            "darwin" => HostOs::Macos,
            "windows" => HostOs::Windows,
            _ => HostOs::Linux,
        };
        let home = if os == "windows" {
            r"C:\corpus-home"
        } else {
            "/corpus-home"
        };
        let supported = [
            slicc_browsers::corpus::NEED_USER_DATA_DIRS,
            slicc_browsers::corpus::NEED_PROFILES,
            slicc_browsers::corpus::NEED_FLAG,
            slicc_browsers::corpus::NEED_PROCESS,
            slicc_browsers::corpus::NEED_INSTALLED,
        ];
        let mut ran = 0;
        for case in slicc_browsers::corpus::corpus().cases {
            if !case.runs(os, &supported) {
                continue;
            }
            ran += 1;
            let getenv = |key: &str| case.input.env.get(key).cloned();
            let layout = slicc_browsers::corpus::Layout {
                os,
                home,
                env: &getenv,
                catalog: &catalog,
            };
            let file_map: BTreeMap<String, String> = case.files(&layout).into_iter().collect();
            let flags = case.cdp(&layout);
            let flag_refs: Vec<&str> = flags.iter().map(String::as_str).collect();
            let installed_rows = expand_corpus_values(&layout, &case.input.installed);
            let installed = if installed_rows.is_empty() {
                None
            } else {
                Some(installed_rows.as_slice())
            };
            let process_text = if os == "windows" {
                case.cim_output(&layout, 8675309)
            } else {
                case.ps_output(&layout, 8675309)
            };
            let http = |url: &str| corpus_http(&case, url);
            let read = |path: &str| file_map.get(path).cloned();
            let list_dir = |path: &str| corpus_children(&file_map, path);
            let launch = LaunchQuery {
                os: host_os,
                bundles: &|_| Vec::new(),
                exists: &|_| false,
                chrome_path: None,
                local_app_data: None,
                program_files: None,
                program_files_x86: None,
            };
            let io = ListIo {
                scan_host: true,
                os: host_os,
                home: Some(home),
                xdg_config_home: case.input.env.get("XDG_CONFIG_HOME").map(String::as_str),
                appdata: case.input.env.get("APPDATA").map(String::as_str),
                process_text: &process_text,
                windows_processes: os == "windows",
                self_pid: 8675309,
                launch,
                http: &http,
                read: &read,
                list_dir: &list_dir,
                targets: &|_| Err(()),
                flag_urls: &flag_refs,
                installed,
            };
            let output = assemble(&list_invocation(None, true), None, &io);
            assert_eq!(output.code, 0, "{}: {}", case.name, output.stderr);
            let actual: Value = serde_json::from_str(output.stdout.trim()).unwrap_or_else(|err| {
                panic!("{}: {err}\n{}", case.name, output.stdout);
            });
            assert_eq!(actual, case.expected(&layout), "{}", case.name);
        }
        assert!(ran > 0);
    }

    fn corpus_http(case: &slicc_browsers::corpus::Case, url: &str) -> HttpResult {
        let Some((_, rest)) = url.split_once("://") else {
            return HttpResult::Transport;
        };
        let (hostport, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, "/"),
        };
        let path = path.split(['?', '#']).next().unwrap_or(path);
        match case.http_get(hostport, path) {
            None => HttpResult::Transport,
            Some((status, body)) => HttpResult::Response { status, body },
        }
    }

    fn corpus_children(files: &BTreeMap<String, String>, dir: &str) -> Vec<String> {
        if dir.is_empty() {
            return Vec::new();
        }
        let sep = if dir.contains('\\') && !dir.contains('/') {
            '\\'
        } else {
            '/'
        };
        let prefix = format!("{}{sep}", dir.trim_end_matches(['/', '\\']));
        let mut names = BTreeSet::new();
        for path in files.keys() {
            let Some(rest) = path.strip_prefix(&prefix) else {
                continue;
            };
            let name = rest.split(['/', '\\']).next().unwrap_or("");
            if dir_name_ok(name) {
                names.insert(name.to_string());
            }
        }
        names.into_iter().collect()
    }

    fn expand_corpus_values(
        layout: &slicc_browsers::corpus::Layout<'_>,
        rows: &[Value],
    ) -> Vec<Value> {
        rows.iter()
            .map(|row| expand_corpus_value(layout, row))
            .collect()
    }

    fn expand_corpus_value(layout: &slicc_browsers::corpus::Layout<'_>, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(layout.expand(text)),
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| expand_corpus_value(layout, item))
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, item)| (key.clone(), expand_corpus_value(layout, item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn app_names(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                let kind = line
                    .rfind(" browser ")
                    .or_else(|| line.rfind(" electron "))
                    .unwrap();
                line[..kind].to_string()
            })
            .collect()
    }

    #[test]
    fn discovery_port_probe_field_is_not_consumed() {
        let field = ["probe", "_ports"].concat();
        let key = ["probe", "Ports"].concat();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut hits = Vec::new();
        scan_sources(&root, &field, &key, &mut hits);
        assert!(hits.is_empty(), "{hits:?}");
    }

    fn scan_sources(dir: &std::path::Path, field: &str, key: &str, hits: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if matches!(
                    name.as_ref(),
                    "target" | "node_modules" | ".git" | ".build" | "dist" | ".playwright-cli"
                ) {
                    continue;
                }
                scan_sources(&path, field, key, hits);
                continue;
            }
            let ext = path.extension().and_then(|item| item.to_str()).unwrap_or("");
            if !matches!(ext, "rs" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "toml") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.contains(field) || text.contains(key) {
                hits.push(path.display().to_string());
            }
        }
    }
}
