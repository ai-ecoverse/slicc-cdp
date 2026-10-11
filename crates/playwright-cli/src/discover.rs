#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use crate::args::{Invocation, Output};
use crate::cdp::Cdp;
use crate::connect::{self, Start};
use serde_json::Value;
use std::collections::BTreeSet;

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
    let connected = connected_lines_from_probe(probe_browser(invocation, cdp_env));
    let (ports, apps) = host_lines();
    Output::ok(render(&connected, &ports, &apps))
}

pub fn render(connected: &[String], ports: &[String], apps: &[String]) -> String {
    format!(
        "{}\n\n{}\n\n{}\n",
        section("connected", connected),
        section("debug ports", ports),
        section("launchable", apps)
    )
}

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

pub fn debug_ports_unix(text: &str, self_pid: u32) -> Vec<String> {
    let mut lines = Vec::new();
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
        let command = line[split..].trim();
        push_port(&mut lines, pid, command);
    }
    lines
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn debug_ports_windows(text: &str, self_pid: u32) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    match value {
        Value::Array(items) => {
            for entry in items {
                push_windows_port(&mut lines, &entry, self_pid);
            }
        }
        other => push_windows_port(&mut lines, &other, self_pid),
    }
    lines
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn push_windows_port(lines: &mut Vec<String>, entry: &Value, self_pid: u32) {
    let Some(pid) = entry.get("ProcessId").and_then(json_u32) else {
        return;
    };
    if pid == 0 || pid == self_pid {
        return;
    }
    let Some(command) = entry.get("CommandLine").and_then(|item| item.as_str()) else {
        return;
    };
    push_port(lines, pid, command);
}

pub fn launchables(query: &LaunchQuery<'_>) -> Vec<String> {
    let mut found: Vec<(String, String, String)> = Vec::new();
    let mut seen = BTreeSet::new();
    if query.os == HostOs::Macos {
        for (bundle_id, name) in CHROMIUM_BROWSERS {
            if let Some(path) = first_app(&(query.bundles)(bundle_id), query.exists) {
                push_app(&mut found, &mut seen, name, "browser", &path);
            }
        }
        for (bundle_id, name) in ELECTRON_APPS {
            if let Some(path) = first_app(&(query.bundles)(bundle_id), query.exists) {
                push_app(&mut found, &mut seen, name, "electron", &path);
            }
        }
    }
    for (name, path) in chrome_candidates(query) {
        if (query.exists)(&path) {
            push_app(&mut found, &mut seen, name, "browser", &path);
        }
    }
    if let Some(path) = query
        .chrome_path
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        if (query.exists)(path) {
            push_app(&mut found, &mut seen, "Chrome", "browser", path);
        }
    }
    found.sort_by(|left, right| left.0.cmp(&right.0).then(left.2.cmp(&right.2)));
    found
        .into_iter()
        .map(|(name, kind, path)| format!("{name} {kind} {path}"))
        .collect()
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

fn push_port(lines: &mut Vec<String>, pid: u32, command: &str) {
    if wrapper_command(command) {
        return;
    }
    let Some(port) = debugging_port(command) else {
        return;
    };
    let executable = display_executable(command);
    if executable.is_empty() {
        return;
    }
    lines.push(format!("pid {pid} port {port} {executable}"));
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
    found: &mut Vec<(String, String, String)>,
    seen: &mut BTreeSet<String>,
    name: &str,
    kind: &str,
    path: &str,
) {
    if path.is_empty() || !seen.insert(path.to_string()) {
        return;
    }
    found.push((name.to_string(), kind.to_string(), path.to_string()));
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

fn probe_browser(invocation: &Invocation, cdp_env: Option<&str>) -> Result<Probe, ()> {
    let start = connect::choose_start(invocation.cdp.as_deref(), cdp_env).map_err(|_| ())?;
    let runtime = invocation.runtime.clone().or_else(|| {
        invocation
            .flags
            .get("runtime")
            .filter(|value| !value.is_empty())
            .cloned()
    });
    let socket = resolve_socket(start, runtime.as_deref())?;
    let mut cdp = Cdp::connect(&socket).map_err(|_| ())?;
    let targets = cdp
        .call("Target.getTargets", serde_json::json!({}), None)
        .map_err(|_| ());
    Ok(Probe {
        websocket_url: socket,
        targets,
    })
}

fn resolve_socket(start: Start, runtime: Option<&str>) -> Result<String, ()> {
    let socket = match start {
        Start::Direct(url) => url,
        Start::Discover(url) => {
            let response = crate::net::http_get(&url).map_err(|_| ())?;
            let body = String::from_utf8_lossy(&response.body).to_string();
            connect::websocket_url_from_version(&body).map_err(|_| ())?
        }
    };
    Ok(connect::append_runtime(&socket, runtime))
}

#[cfg(not(target_arch = "wasm32"))]
fn host_lines() -> (Vec<String>, Vec<String>) {
    let ports = read_ports();
    let chrome_path = std::env::var("CHROME_PATH").ok();
    let local_app_data = std::env::var("LOCALAPPDATA").ok();
    let program_files = std::env::var("PROGRAMFILES").ok();
    let program_files_x86 = std::env::var("PROGRAMFILES(X86)").ok();
    let apps = match host_os() {
        Some(os) => launchables(&LaunchQuery {
            os,
            bundles: &mdfind_bundle,
            exists: &path_exists,
            chrome_path: chrome_path.as_deref(),
            local_app_data: local_app_data.as_deref(),
            program_files: program_files.as_deref(),
            program_files_x86: program_files_x86.as_deref(),
        }),
        None => Vec::new(),
    };
    (ports, apps)
}

#[cfg(target_arch = "wasm32")]
fn host_lines() -> (Vec<String>, Vec<String>) {
    (Vec::new(), Vec::new())
}

#[cfg(all(not(target_arch = "wasm32"), target_os = "windows"))]
fn read_ports() -> Vec<String> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_Process | Select-Object ProcessId, CommandLine | ConvertTo-Json -Compress",
        ])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    debug_ports_windows(&String::from_utf8_lossy(&output.stdout), std::process::id())
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "windows")))]
fn read_ports() -> Vec<String> {
    let output = std::process::Command::new("ps")
        .args(["-ax", "-o", "pid=", "-o", "command="])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    debug_ports_unix(&String::from_utf8_lossy(&output.stdout), std::process::id())
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
        assert!(global.stdout.contains("This CLI does not launch Chrome.\n"));
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
}
