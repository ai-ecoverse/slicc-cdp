#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Chromium,
    Electron,
}

#[derive(Clone, Copy)]
struct Known {
    names: &'static [&'static str],
    bundle_id: &'static str,
    apps: &'static [&'static str],
    linux: &'static [&'static str],
    windows: &'static [&'static str],
    kind: Kind,
}

const fn known(
    names: &'static [&'static str],
    bundle_id: &'static str,
    apps: &'static [&'static str],
    linux: &'static [&'static str],
    windows: &'static [&'static str],
    kind: Kind,
) -> Known {
    Known {
        names,
        bundle_id,
        apps,
        linux,
        windows,
        kind,
    }
}

const CHROMIUM: Kind = Kind::Chromium;
const ELECTRON: Kind = Kind::Electron;

static KNOWN: &[Known] = &[
    known(&["Google Chrome", "chrome"], "com.google.Chrome", &["Google Chrome"], &["/usr/bin/google-chrome", "/usr/bin/google-chrome-stable"], &["Google\\Chrome\\Application\\chrome.exe"], CHROMIUM),
    known(&["Google Chrome Beta"], "com.google.Chrome.beta", &["Google Chrome Beta"], &["/usr/bin/google-chrome-beta"], &["Google\\Chrome Beta\\Application\\chrome.exe"], CHROMIUM),
    known(&["Google Chrome Dev"], "com.google.Chrome.dev", &["Google Chrome Dev"], &["/usr/bin/google-chrome-unstable"], &["Google\\Chrome Dev\\Application\\chrome.exe"], CHROMIUM),
    known(&["Chrome Canary"], "com.google.Chrome.canary", &["Google Chrome Canary"], &["/usr/bin/google-chrome-canary"], &["Google\\Chrome SxS\\Application\\chrome.exe"], CHROMIUM),
    known(&["Chrome for Testing"], "com.google.chrome.for.testing", &["Google Chrome for Testing"], &[], &[], CHROMIUM),
    known(&["Brave Browser"], "com.brave.Browser", &["Brave Browser"], &["/usr/bin/brave-browser", "/usr/bin/brave"], &["BraveSoftware\\Brave-Browser\\Application\\brave.exe"], CHROMIUM),
    known(&["Brave Beta"], "com.brave.Browser.beta", &["Brave Browser Beta"], &["/usr/bin/brave-browser-beta"], &["BraveSoftware\\Brave-Browser-Beta\\Application\\brave.exe"], CHROMIUM),
    known(&["Brave Nightly"], "com.brave.Browser.nightly", &["Brave Browser Nightly"], &["/usr/bin/brave-browser-nightly"], &["BraveSoftware\\Brave-Browser-Nightly\\Application\\brave.exe"], CHROMIUM),
    known(&["Microsoft Edge", "msedge"], "com.microsoft.edgemac", &["Microsoft Edge"], &["/usr/bin/microsoft-edge", "/usr/bin/microsoft-edge-stable"], &["Microsoft\\Edge\\Application\\msedge.exe"], CHROMIUM),
    known(&["Microsoft Edge Beta"], "com.microsoft.edgemac.Beta", &["Microsoft Edge Beta"], &["/usr/bin/microsoft-edge-beta"], &["Microsoft\\Edge Beta\\Application\\msedge.exe"], CHROMIUM),
    known(&["Microsoft Edge Dev"], "com.microsoft.edgemac.Dev", &["Microsoft Edge Dev"], &["/usr/bin/microsoft-edge-dev"], &["Microsoft\\Edge Dev\\Application\\msedge.exe"], CHROMIUM),
    known(&["Microsoft Edge Canary"], "com.microsoft.edgemac.Canary", &["Microsoft Edge Canary"], &["/usr/bin/microsoft-edge-canary"], &["Microsoft\\Edge SxS\\Application\\msedge.exe"], CHROMIUM),
    known(&["Vivaldi"], "com.vivaldi.Vivaldi", &["Vivaldi"], &["/usr/bin/vivaldi", "/usr/bin/vivaldi-stable"], &["Vivaldi\\Application\\vivaldi.exe"], CHROMIUM),
    known(&["Vivaldi Snapshot"], "com.vivaldi.Vivaldi.snapshot", &["Vivaldi Snapshot"], &["/usr/bin/vivaldi-snapshot"], &[], CHROMIUM),
    known(&["Opera"], "com.operasoftware.Opera", &["Opera"], &["/usr/bin/opera"], &[], CHROMIUM),
    known(&["Arc"], "company.thebrowser.Browser", &["Arc"], &[], &[], CHROMIUM),
    known(&["Dia"], "company.thebrowser.dia", &["Dia"], &[], &[], CHROMIUM),
    known(&["ChatGPT Atlas"], "com.openai.atlas", &["ChatGPT Atlas"], &[], &[], CHROMIUM),
    known(&["Chromium", "chromium"], "org.chromium.Chromium", &["Chromium"], &["/usr/bin/chromium", "/usr/bin/chromium-browser", "/snap/bin/chromium"], &["Chromium\\Application\\chrome.exe"], CHROMIUM),
    known(&["Microsoft Teams"], "com.microsoft.teams2", &["Microsoft Teams"], &[], &[], ELECTRON),
    known(&["Visual Studio Code"], "com.microsoft.VSCode", &["Visual Studio Code"], &["/usr/bin/code"], &[], ELECTRON),
    known(&["VS Code Insiders"], "com.microsoft.VSCodeInsiders", &["VS Code Insiders"], &["/usr/bin/code-insiders"], &[], ELECTRON),
    known(&["Spotify"], "com.spotify.client", &["Spotify"], &["/usr/bin/spotify"], &[], ELECTRON),
    known(&["Slack"], "com.tinyspeck.slackmacgap", &["Slack"], &["/usr/bin/slack"], &[], ELECTRON),
    known(&["Discord"], "com.hnc.Discord", &["Discord"], &["/usr/bin/discord"], &[], ELECTRON),
    known(&["Cursor"], "com.todesktop.230313mzl4w4u92", &["Cursor"], &[], &[], ELECTRON),
    known(&["Figma"], "com.figma.Desktop", &["Figma"], &[], &[], ELECTRON),
    known(&["Notion"], "notion.id", &["Notion"], &[], &[], ELECTRON),
    known(&["OBS Studio"], "com.obsproject.obs-studio", &["OBS Studio"], &[], &[], ELECTRON),
    known(&["1Password"], "com.1password.1password", &["1Password"], &[], &[], ELECTRON),
    known(&["Zoom"], "us.zoom.xos", &["zoom.us"], &[], &[], ELECTRON),
    known(&["Linear"], "com.linear", &["Linear"], &[], &[], ELECTRON),
    known(&["Loom"], "com.loom.desktop", &["Loom"], &[], &[], ELECTRON),
    known(&["Obsidian"], "md.obsidian", &["Obsidian"], &[], &[], ELECTRON),
    known(&["Bitwarden"], "com.bitwarden.desktop", &["Bitwarden"], &[], &[], ELECTRON),
    known(&["Todoist"], "com.todoist.mac.Todoist", &["Todoist"], &[], &[], ELECTRON),
    known(&["GitHub Desktop"], "com.github.GitHubClient", &["GitHub Desktop"], &[], &[], ELECTRON),
    known(&["Postman"], "com.postmanlabs.mac", &["Postman"], &[], &[], ELECTRON),
    known(&["Signal"], "org.whispersystems.signal-desktop", &["Signal"], &[], &[], ELECTRON),
    known(&["Telegram Desktop"], "com.tdesktop.Telegram", &["Telegram"], &[], &[], ELECTRON),
    known(&["Logseq"], "com.logseq.logseq", &["Logseq"], &[], &[], ELECTRON),
    known(&["WhatsOnChain"], "com.whatsonchain.WhatsOnChain", &["WhatsOnChain"], &[], &[], ELECTRON),
    known(&["Storybook"], "io.trystorybook.app", &["Storybook"], &[], &[], ELECTRON),
    known(&["MongoDB Compass"], "com.mongodb.compass", &["MongoDB Compass"], &[], &[], ELECTRON),
    known(&["Insomnia"], "com.insomnia.app", &["Insomnia"], &[], &[], ELECTRON),
    known(&["JetBrains Toolbox"], "com.jetbrains.toolbox", &["JetBrains Toolbox"], &[], &[], ELECTRON),
    known(&["Replit"], "com.electron.replit", &["Replit"], &[], &[], ELECTRON),
    known(&["Twitch Studio"], "com.twitch.studio", &["Twitch Studio"], &[], &[], ELECTRON),
];

pub struct LaunchRequest<'a> {
    pub command: &'a str,
    pub browser: Option<&'a str>,
    pub headed: bool,
    pub profile: Option<&'a str>,
}

pub struct HostEnv {
    pub platform: &'static str,
    pub chrome_path: Option<String>,
    pub local_app_data: Option<String>,
    pub program_files: Option<String>,
    pub program_files_x86: Option<String>,
    pub temp_dir: String,
    pub unique: String,
    pub home: String,
    pub app_data: Option<String>,
    pub xdg_config_home: Option<String>,
    pub snap_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnPlan {
    pub program: String,
    pub args: Vec<String>,
    pub port: u16,
    pub user_data_dir: String,
    pub delete_on_failure: bool,
    pub via_open: bool,
}

const SEVEN_SITE: &str = "[*.]sliccy.ai";
const CHROME_EPOCH_MICROS: i64 = 11_644_473_600_000_000;
const SEVEN_FEATURES: &str = "LocalNetworkAccessChecks,LocalNetworkAccessChecksWebSockets,IntensiveWakeUpThrottling,HighEfficiencyModeAvailable,InfiniteTabsFreezing,InfiniteTabsFreezingOnMemoryPressure,CPUMeasurementInFreezingPolicy,MemoryMeasurementInFreezingPolicy,AllowDevtoolsConnectedDiscard";
const ACTIVE_PORT_FILE: &str = "DevToolsActivePort";

pub struct Started {
    pub version_url: String,
    stop: Option<Box<dyn FnOnce()>>,
}

impl Started {
    pub fn running(version_url: String, stop: Box<dyn FnOnce()>) -> Self {
        Self {
            version_url,
            stop: Some(stop),
        }
    }

    pub fn disarm(&mut self) {
        self.stop.take();
    }
}

impl Drop for Started {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

pub struct Resolved {
    pub discover: String,
    pub started: Option<Started>,
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter
            .debug_struct("Resolved")
            .field("discover", &self.discover)
            .field("started", &self.started.is_some())
            .finish()
    }
}

#[cfg(test)]
pub trait Launcher {
    fn launch(&mut self, request: &LaunchRequest) -> Result<Started, String>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum Poll {
    Ready,
    Again,
}

pub fn is_open_command(command: &str) -> bool {
    command == "open" || command == "tab-new"
}

pub fn should_launch(command: &str, explicit: bool) -> bool {
    cfg!(not(target_arch = "wasm32")) && is_open_command(command) && !explicit
}

pub fn endpoint_is_explicit(cdp: Option<&str>, env: Option<&str>) -> bool {
    nonempty(cdp) || nonempty(env)
}

pub fn start_after_explicit(
    cdp: Option<&str>,
    env: Option<&str>,
    active: Option<String>,
) -> Result<crate::connect::Start, String> {
    if endpoint_is_explicit(cdp, env) {
        return crate::connect::choose_start(cdp, env);
    }
    if let Some(url) = active {
        return Ok(crate::connect::Start::Discover(url));
    }
    if cfg!(target_arch = "wasm32") {
        return crate::connect::choose_start(None, None);
    }
    Err("playwright-cli: no debugger is connected\n".to_string())
}

pub fn attached_port() -> Result<Option<String>, String> {
    #[cfg(any(test, target_arch = "wasm32"))]
    {
        Ok(None)
    }
    #[cfg(all(not(test), not(target_arch = "wasm32")))]
    {
        let env = current_env();
        select_port(&env, &DiskFs, &mut |url| http_body(url))
    }
}

fn nonempty(value: Option<&str>) -> bool {
    value.map(str::trim).is_some_and(|text| !text.is_empty())
}

pub fn host_only_line(command: &str, names: &[String]) -> String {
    let verb = if names.len() == 1 { "is" } else { "are" };
    format!(
        "playwright-cli {command}: {} {verb} host-only\n",
        names.join(", ")
    )
}

pub fn reject_wasm_flags(command: &str, flags: &BTreeMap<String, String>) -> Option<String> {
    if !is_open_command(command) || !cfg!(target_arch = "wasm32") {
        return None;
    }
    let names: Vec<String> = ["browser", "headed", "profile"]
        .into_iter()
        .filter(|name| flags.contains_key(*name))
        .map(|name| format!("--{name}"))
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(host_only_line(command, &names))
    }
}

pub fn port_allowed(port: u16) -> bool {
    port >= 9333 && !matches!(port, 9222..=9229)
}

pub fn choose_port(mut bind: impl FnMut(u16) -> bool) -> Result<u16, String> {
    let mut port = 9333u32;
    while port <= u32::from(u16::MAX) {
        let candidate = port as u16;
        if port_allowed(candidate) && bind(candidate) {
            return Ok(candidate);
        }
        port += 1;
    }
    Err("playwright-cli: no free localhost port\n".to_string())
}

pub fn version_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/json/version")
}

pub fn mdfind_query(bundle_id: &str) -> String {
    format!("kMDItemCFBundleIdentifier == \"{bundle_id}\"")
}

pub fn executable_from_plist(plist: &str) -> Option<String> {
    let key = "<key>CFBundleExecutable</key>";
    let index = plist.find(key)?;
    let rest = &plist[index + key.len()..];
    let start = rest.find("<string>")? + "<string>".len();
    let end = rest[start..].find("</string>")? + start;
    let name = rest[start..end].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

pub fn poll_version(body: Result<String, String>, running: bool) -> Result<Poll, String> {
    if !running {
        return Err(
            "playwright-cli: browser exited before the debugger was ready\n".to_string(),
        );
    }
    match body {
        Ok(body) => {
            if crate::connect::websocket_url_from_version(&body).is_ok() {
                Ok(Poll::Ready)
            } else {
                Ok(Poll::Again)
            }
        }
        Err(message) if message.starts_with("HTTP ") => Err(message),
        Err(_) => Ok(Poll::Again),
    }
}

#[cfg(test)]
pub fn decide(
    command: &str,
    browser: Option<&str>,
    headed: bool,
    profile: Option<&str>,
    probe: &mut dyn FnMut() -> Result<(), String>,
    launcher: &mut dyn Launcher,
) -> Result<Resolved, String> {
    if let Some(name) = browser {
        validate_browser(command, name)?;
    }
    match probe() {
        Ok(()) => Ok(Resolved {
            discover: "http://127.0.0.1:9333/json/version".to_string(),
            started: None,
        }),
        Err(message) if message.starts_with("HTTP ") => Err(message),
        Err(_) => {
            let request = LaunchRequest {
                command,
                browser,
                headed,
                profile,
            };
            let started = launcher.launch(&request)?;
            let discover = started.version_url.clone();
            Ok(Resolved {
                discover,
                started: Some(started),
            })
        }
    }
}

pub fn resolve_without_endpoint(
    command: &str,
    flags: &BTreeMap<String, String>,
) -> Result<Resolved, String> {
    let browser = flags
        .get("browser")
        .map(String::as_str)
        .filter(|value| !value.is_empty());
    let profile_raw = flags.get("profile").map(String::as_str);
    if let Some(path) = profile_raw {
        if path.trim().is_empty() {
            return Err(format!(
                "playwright-cli {command}: --profile requires a path\n"
            ));
        }
    }
    let profile = profile_raw.map(str::trim).filter(|value| !value.is_empty());
    let headed = matches!(
        flags.get("headed").map(String::as_str),
        Some("true") | Some("")
    );
    if let Some(name) = browser {
        validate_browser(command, name)?;
    }
    let request = LaunchRequest {
        command,
        browser,
        headed,
        profile,
    };
    #[cfg(test)]
    {
        let _ = request;
        return Err("playwright-cli: browser launch is not available in tests\n".to_string());
    }
    #[cfg(not(test))]
    {
        host_open(&request)
    }
}

#[cfg(all(not(test), target_arch = "wasm32"))]
fn host_open(_request: &LaunchRequest) -> Result<Resolved, String> {
    Err("playwright-cli: browser launch is not available in this build\n".to_string())
}

pub fn plan_launch(
    request: &LaunchRequest,
    env: &HostEnv,
    electron_port: u16,
    exists: &dyn Fn(&str) -> bool,
    mdfind: &dyn Fn(&str) -> Vec<String>,
    read_plist: &dyn Fn(&str) -> Option<String>,
) -> Result<SpawnPlan, String> {
    let command = request.command;
    let (program, kind) = if let Some(name) = request.browser {
        resolve_named(command, name, env, exists, mdfind, read_plist)?
    } else {
        resolve_default(command, env, exists, mdfind, read_plist)?
    };
    let (user_data_dir, delete_on_failure) = data_dir(command, request, env, kind, &program)?;
    if kind == Kind::Electron && !port_allowed(electron_port) {
        return Err(format!(
            "playwright-cli {command}: refusing debugger port {electron_port}\n"
        ));
    }
    let browser_args = if kind == Kind::Chromium {
        chromium_args(&user_data_dir, request.headed)
    } else {
        electron_args(electron_port, &user_data_dir)
    };
    let (program, args, via_open) = if kind == Kind::Chromium
        && request.headed
        && env.platform == "macos"
    {
        if let Some(app) = bundle_app(&program) {
            let (program, args) = open_command(&app, &browser_args);
            (program, args, true)
        } else {
            (program, browser_args, false)
        }
    } else {
        (program, browser_args, false)
    };
    Ok(SpawnPlan {
        program,
        args,
        port: if kind == Kind::Electron {
            electron_port
        } else {
            0
        },
        user_data_dir,
        delete_on_failure,
        via_open,
    })
}

fn validate_browser(command: &str, name: &str) -> Result<&'static Known, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(format!(
            "playwright-cli {command}: --browser requires a name\n"
        ));
    }
    let key = trimmed.to_ascii_lowercase();
    if key == "firefox" || key == "webkit" {
        return Err(format!(
            "playwright-cli {command}: {key} is not launchable\n"
        ));
    }
    if key == "claude" || key == "1password" {
        let label = if key == "claude" { "Claude" } else { "1Password" };
        return Err(format!(
            "playwright-cli {command}: {label} blocks remote debugging\n"
        ));
    }
    find_known(trimmed).ok_or_else(|| {
        format!("playwright-cli {command}: unknown browser \"{trimmed}\"\n")
    })
}

fn find_known(name: &str) -> Option<&'static Known> {
    KNOWN.iter().find(|item| {
        item.names
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    })
}

fn resolve_default(
    command: &str,
    env: &HostEnv,
    exists: &dyn Fn(&str) -> bool,
    mdfind: &dyn Fn(&str) -> Vec<String>,
    read_plist: &dyn Fn(&str) -> Option<String>,
) -> Result<(String, Kind), String> {
    let mut candidates = Vec::new();
    if let Some(path) = &env.chrome_path {
        candidates.push(path.clone());
    }
    candidates.extend(installed_chrome_candidates(env));
    if let Some(program) = first_existing(&candidates, exists, read_plist) {
        return Ok((program, Kind::Chromium));
    }
    for bundle_id in [
        "com.google.Chrome",
        "com.google.Chrome.canary",
        "org.chromium.Chromium",
    ] {
        if let Some(program) = first_from_mdfind(bundle_id, mdfind, exists, read_plist) {
            return Ok((program, Kind::Chromium));
        }
    }
    Err(not_found(command, "chrome"))
}

fn resolve_named(
    command: &str,
    name: &str,
    env: &HostEnv,
    exists: &dyn Fn(&str) -> bool,
    mdfind: &dyn Fn(&str) -> Vec<String>,
    read_plist: &dyn Fn(&str) -> Option<String>,
) -> Result<(String, Kind), String> {
    let known = validate_browser(command, name)?;
    let mut candidates = Vec::new();
    if known.bundle_id == "com.google.Chrome" {
        if let Some(path) = &env.chrome_path {
            candidates.push(path.clone());
        }
    }
    match env.platform {
        "linux" => candidates.extend(known.linux.iter().map(|path| (*path).to_string())),
        "windows" => candidates.extend(windows_candidates(known.windows, env)),
        "macos" => {
            for app in known.apps {
                candidates.push(format!("/Applications/{app}.app"));
            }
        }
        _ => {}
    }
    if let Some(program) = first_existing(&candidates, exists, read_plist) {
        return Ok((program, known.kind));
    }
    if let Some(program) = first_from_mdfind(known.bundle_id, mdfind, exists, read_plist) {
        return Ok((program, known.kind));
    }
    Err(not_found(command, name.trim()))
}

fn installed_chrome_candidates(env: &HostEnv) -> Vec<String> {
    match env.platform {
        "macos" => vec![
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string(),
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary"
                .to_string(),
            "/Applications/Chromium.app/Contents/MacOS/Chromium".to_string(),
        ],
        "linux" => vec![
            "/usr/bin/google-chrome".to_string(),
            "/usr/bin/google-chrome-stable".to_string(),
            "/usr/bin/chromium".to_string(),
            "/usr/bin/chromium-browser".to_string(),
            "/snap/bin/chromium".to_string(),
        ],
        "windows" => windows_candidates(&["Google\\Chrome\\Application\\chrome.exe"], env),
        _ => Vec::new(),
    }
}

fn windows_candidates(relative: &[&str], env: &HostEnv) -> Vec<String> {
    let mut out = Vec::new();
    for root in [
        &env.local_app_data,
        &env.program_files,
        &env.program_files_x86,
    ]
    .into_iter()
    .flatten()
    {
        for path in relative {
            out.push(format!("{root}\\{path}"));
        }
    }
    out
}

fn first_existing(
    candidates: &[String],
    exists: &dyn Fn(&str) -> bool,
    read_plist: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    for candidate in candidates {
        if let Some(program) = resolve_path(candidate, exists, read_plist) {
            return Some(program);
        }
    }
    None
}

fn first_from_mdfind(
    bundle_id: &str,
    mdfind: &dyn Fn(&str) -> Vec<String>,
    exists: &dyn Fn(&str) -> bool,
    read_plist: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    for path in mdfind(bundle_id) {
        if let Some(program) = resolve_path(&path, exists, read_plist) {
            return Some(program);
        }
    }
    None
}

fn resolve_path(
    path: &str,
    exists: &dyn Fn(&str) -> bool,
    read_plist: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if path.ends_with(".app") {
        macos_executable(path, exists, read_plist)
    } else if exists(path) {
        Some(path.to_string())
    } else {
        None
    }
}

fn macos_executable(
    app: &str,
    exists: &dyn Fn(&str) -> bool,
    read_plist: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let plist_path = format!("{app}/Contents/Info.plist");
    if let Some(name) = read_plist(&plist_path) {
        let candidate = format!("{app}/Contents/MacOS/{name}");
        if exists(&candidate) {
            return Some(candidate);
        }
    }
    let bundle = app.rsplit(['/', '\\']).next().unwrap_or(app);
    let bundle = bundle.strip_suffix(".app").unwrap_or(bundle);
    let candidate = format!("{app}/Contents/MacOS/{bundle}");
    exists(&candidate).then(|| candidate)
}

fn profile_id(bundle_id: &str) -> &'static str {
    match bundle_id {
        "com.google.Chrome" => "chrome",
        "com.google.Chrome.beta" => "chrome-beta",
        "com.google.Chrome.dev" => "chrome-dev",
        "com.google.Chrome.canary" => "chrome-canary",
        "com.google.chrome.for.testing" => "chrome-for-testing",
        "com.brave.Browser" => "brave",
        "com.brave.Browser.beta" => "brave-beta",
        "com.brave.Browser.nightly" => "brave-nightly",
        "com.microsoft.edgemac" => "edge",
        "com.microsoft.edgemac.Beta" => "edge-beta",
        "com.microsoft.edgemac.Dev" => "edge-dev",
        "com.microsoft.edgemac.Canary" => "edge-canary",
        "com.vivaldi.Vivaldi" => "vivaldi",
        "com.vivaldi.Vivaldi.snapshot" => "vivaldi-snapshot",
        "com.operasoftware.Opera" => "opera",
        "company.thebrowser.Browser" => "arc",
        "company.thebrowser.dia" => "dia",
        "com.openai.atlas" => "atlas",
        "org.chromium.Chromium" => "chromium",
        _ => "chrome",
    }
}

fn browser_snap(program: &str) -> Option<String> {
    let name = program.strip_prefix("/snap/bin/")?;
    if name.is_empty() || name.contains('/') {
        None
    } else {
        Some(name.to_string())
    }
}

fn windows_host(env: &HostEnv) -> bool {
    env.platform == "windows"
}

fn join_parts(windows: bool, parts: &[String]) -> String {
    let sep = if windows { '\\' } else { '/' };
    let mut out = String::new();
    for (index, part) in parts.iter().enumerate() {
        let piece = if windows {
            part.replace('/', "\\")
        } else {
            part.clone()
        };
        let piece = if index == 0 {
            piece.trim_end_matches(sep).to_string()
        } else {
            piece.trim_matches(sep).to_string()
        };
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() && !out.ends_with(sep) {
            out.push(sep);
        }
        out.push_str(&piece);
    }
    out
}

fn config_dir(env: &HostEnv) -> String {
    match env.platform {
        "macos" => join_parts(
            false,
            &[
                env.home.clone(),
                "Library".to_string(),
                "Application Support".to_string(),
            ],
        ),
        "windows" => {
            if let Some(app_data) = &env.app_data {
                app_data.clone()
            } else {
                join_parts(
                    true,
                    &[
                        env.home.clone(),
                        "AppData".to_string(),
                        "Roaming".to_string(),
                    ],
                )
            }
        }
        _ => {
            if let Some(xdg) = &env.xdg_config_home {
                if xdg.starts_with('/') {
                    return xdg.clone();
                }
            }
            join_parts(false, &[env.home.clone(), ".config".to_string()])
        }
    }
}

fn snap_profiles_root(home: &str, snap: &str, windows: bool) -> String {
    join_parts(
        windows,
        &[
            home.to_string(),
            "snap".to_string(),
            snap.to_string(),
            "common".to_string(),
            "slicc".to_string(),
            "seven-profiles".to_string(),
        ],
    )
}

fn profiles_root(env: &HostEnv, browser_snap: Option<&str>) -> String {
    let windows = windows_host(env);
    if let Some(snap) = browser_snap {
        return snap_profiles_root(&env.home, snap, windows);
    }
    if let Some(snap) = &env.snap_name {
        return snap_profiles_root(&env.home, snap, windows);
    }
    join_parts(
        windows,
        &[
            config_dir(env),
            "slicc".to_string(),
            "seven-profiles".to_string(),
        ],
    )
}

fn path_parts(path: &str) -> Vec<String> {
    path.replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
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

fn is_six_profile(path: &str) -> bool {
    six_profile_path(path)
        || std::fs::canonicalize(path)
            .ok()
            .and_then(|full| full.to_str().map(six_profile_path))
            .unwrap_or(false)
}

fn six_profile_path(path: &str) -> bool {
    let parts = path_parts(path);
    let prefix = "browser-coding-agent-chrome";
    parts.windows(2).any(|window| {
        window[0].eq_ignore_ascii_case("slicc") && window[1].eq_ignore_ascii_case("profiles")
    }) || parts.iter().any(|part| {
        part.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })
}

fn under_seven_profiles(path: &str) -> bool {
    if is_six_profile(path) {
        return false;
    }
    path_parts(path)
        .windows(2)
        .any(|window| window[0] == "slicc" && window[1] == "seven-profiles")
}

fn temp_profile(env: &HostEnv) -> String {
    let temp = env.temp_dir.trim_end_matches(['/', '\\']);
    let name = format!("playwright-cli-{}", env.unique);
    if windows_host(env) || temp.contains('\\') {
        format!("{temp}\\{name}")
    } else {
        format!("{temp}/{name}")
    }
}

fn data_dir(
    command: &str,
    request: &LaunchRequest,
    env: &HostEnv,
    kind: Kind,
    program: &str,
) -> Result<(String, bool), String> {
    if let Some(path) = request.profile {
        let path = path.trim();
        if path.is_empty() {
            return Err(format!(
                "playwright-cli {command}: --profile requires a path\n"
            ));
        }
        if is_six_profile(path) {
            return Err(format!(
                "playwright-cli {command}: refusing profile {path}\n"
            ));
        }
        return Ok((path.to_string(), false));
    }
    if kind == Kind::Electron {
        return Ok((temp_profile(env), true));
    }
    let id = match request.browser.and_then(find_known) {
        Some(known) => profile_id(known.bundle_id),
        None => "chrome",
    };
    let snap = browser_snap(program);
    let root = profiles_root(env, snap.as_deref());
    if is_six_profile(&root) {
        return Err(format!(
            "playwright-cli {command}: refusing profile {root}\n"
        ));
    }
    Ok((
        join_parts(windows_host(env), &[root, id.to_string()]),
        false,
    ))
}

fn chromium_args(user_data_dir: &str, headed: bool) -> Vec<String> {
    let mut args = vec![
        "--remote-debugging-port=0".to_string(),
        format!("--user-data-dir={user_data_dir}"),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-background-timer-throttling".to_string(),
        "--disable-backgrounding-occluded-windows".to_string(),
        "--disable-renderer-backgrounding".to_string(),
        "--disable-background-tracing".to_string(),
        format!("--disable-features={SEVEN_FEATURES}"),
    ];
    if !headed {
        args.push("--headless=new".to_string());
    }
    args
}

fn electron_args(port: u16, user_data_dir: &str) -> Vec<String> {
    vec![
        format!("--remote-debugging-port={port}"),
        format!("--user-data-dir={user_data_dir}"),
    ]
}

fn bundle_app(program: &str) -> Option<String> {
    if program.ends_with(".app") {
        return Some(program.to_string());
    }
    let marker = program.find(".app/").or_else(|| program.find(".app\\"))?;
    Some(program[..marker + 4].to_string())
}

fn open_command(app: &str, browser_args: &[String]) -> (String, Vec<String>) {
    let mut args = vec![
        "-n".to_string(),
        "-a".to_string(),
        app.to_string(),
        "--args".to_string(),
    ];
    args.extend(browser_args.iter().cloned());
    ("/usr/bin/open".to_string(), args)
}

pub fn parse_active_port(text: &str) -> Option<(u16, String)> {
    let text = text.replace("\r\n", "\n");
    let mut lines = text.split('\n');
    let port: u16 = lines.next()?.trim().parse().ok()?;
    if port == 0 {
        return None;
    }
    let path = lines.next()?.trim();
    if !path.starts_with('/') {
        return None;
    }
    Some((port, path.to_string()))
}

fn version_accepts(body: &str, path: &str) -> bool {
    crate::connect::websocket_url_from_version(body)
        .ok()
        .is_some_and(|url| url.ends_with(path))
}

fn child_path(dir: &str, name: &str, windows: bool) -> String {
    join_parts(windows, &[dir.to_string(), name.to_string()])
}

fn lock_pid(target: &str) -> Option<u32> {
    let index = target.rfind('-')?;
    let pid: u32 = target[index + 1..].trim().parse().ok()?;
    if pid == 0 {
        None
    } else {
        Some(pid)
    }
}

trait ProfileFs {
    fn read(&self, path: &str) -> Option<String>;
    fn read_link(&self, path: &str) -> Option<String>;
    fn pid_alive(&self, pid: u32) -> bool;
    fn remove(&mut self, path: &str);
    fn write(&mut self, path: &str, body: &str) -> Result<(), String>;
}

fn profile_owner<F: ProfileFs>(dir: &str, windows: bool, fs: &F) -> Option<u32> {
    let target = fs.read_link(&child_path(dir, "SingletonLock", windows))?;
    let pid = lock_pid(&target)?;
    fs.pid_alive(pid).then_some(pid)
}

fn live_port<F: ProfileFs>(
    dir: &str,
    windows: bool,
    fs: &F,
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    let Some(text) = fs.read(&child_path(dir, ACTIVE_PORT_FILE, windows)) else {
        return Ok(None);
    };
    live_text(&text, probe)
}

fn live_text(
    text: &str,
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    let Some((port, path)) = parse_active_port(text) else {
        return Ok(None);
    };
    if (9222..=9229).contains(&port) {
        return Ok(None);
    }
    let url = version_url(port);
    match probe(&url) {
        Ok(body) if version_accepts(&body, &path) => Ok(Some(url)),
        Ok(_) => Ok(None),
        Err(message) if message.starts_with("HTTP ") => Err(message),
        Err(_) => Ok(None),
    }
}

trait ScanFs {
    fn list_dirs(&self, path: &str) -> Vec<String>;
    fn read_file(&self, path: &str) -> Option<String>;
    fn modified(&self, path: &str) -> Option<i64>;
}

struct PortCandidate {
    modified: i64,
    path: String,
    body: String,
}

fn config_seven_root(env: &HostEnv) -> String {
    join_parts(
        windows_host(env),
        &[
            config_dir(env),
            "slicc".to_string(),
            "seven-profiles".to_string(),
        ],
    )
}

fn collect_port_files<F: ScanFs>(env: &HostEnv, fs: &F) -> Vec<PortCandidate> {
    let windows = windows_host(env);
    let mut found = Vec::new();
    collect_profile_root(&config_seven_root(env), windows, fs, &mut found);
    let snap = join_parts(
        windows,
        &[env.home.clone(), "snap".to_string()],
    );
    for name in fs.list_dirs(&snap) {
        let root = join_parts(
            windows,
            &[
                snap.clone(),
                name,
                "common".to_string(),
                "slicc".to_string(),
                "seven-profiles".to_string(),
            ],
        );
        collect_profile_root(&root, windows, fs, &mut found);
    }
    found
}

fn collect_profile_root<F: ScanFs>(
    root: &str,
    windows: bool,
    fs: &F,
    found: &mut Vec<PortCandidate>,
) {
    if is_six_profile(root) {
        return;
    }
    for id in fs.list_dirs(root) {
        if !profile_dir_name(&id) {
            continue;
        }
        let dir = join_parts(windows, &[root.to_string(), id]);
        if is_six_profile(&dir) {
            continue;
        }
        let file = child_path(&dir, ACTIVE_PORT_FILE, windows);
        let Some(body) = fs.read_file(&file) else {
            continue;
        };
        let Some(modified) = fs.modified(&file) else {
            continue;
        };
        found.push(PortCandidate {
            modified,
            path: file,
            body,
        });
    }
}

fn select_port<F: ScanFs>(
    env: &HostEnv,
    fs: &F,
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    let mut found = collect_port_files(env, fs);
    found.sort_by(|left, right| {
        right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.path.cmp(&right.path))
    });
    for candidate in found {
        if let Some(url) = live_text(&candidate.body, probe)? {
            return Ok(Some(url));
        }
    }
    Ok(None)
}

pub fn seed_preferences(existing: Option<&str>, unix_micros: i64) -> Result<Option<String>, ()> {
    let mut prefs = Map::new();
    let existed = match existing {
        None => false,
        Some(text) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => {
                prefs = map;
                true
            }
            _ => return Err(()),
        },
    };
    let mut changed = false;
    if existed {
        changed = mark_clean_exit(&mut prefs);
    }
    changed = apply_seven_prefs(&mut prefs, unix_micros) || changed;
    if !changed {
        Ok(None)
    } else {
        Ok(Some(Value::Object(prefs).to_string()))
    }
}

fn set_pref(map: &mut Map<String, Value>, key: &str, value: Value) -> bool {
    if map.get(key) == Some(&value) {
        false
    } else {
        map.insert(key.to_string(), value);
        true
    }
}

fn with_child(
    map: &mut Map<String, Value>,
    key: &str,
    changed: &mut bool,
    update: impl FnOnce(&mut Map<String, Value>) -> bool,
) {
    let mut child = match map.remove(key) {
        Some(Value::Object(child)) => child,
        Some(_) => {
            *changed = true;
            Map::new()
        }
        None => {
            *changed = true;
            Map::new()
        }
    };
    if update(&mut child) {
        *changed = true;
    }
    map.insert(key.to_string(), Value::Object(child));
}

fn mark_clean_exit(prefs: &mut Map<String, Value>) -> bool {
    let mut changed = false;
    with_child(prefs, "profile", &mut changed, |profile| {
        let mut inner = false;
        inner |= set_pref(profile, "exit_type", Value::String("Normal".to_string()));
        inner |= set_pref(profile, "exited_cleanly", Value::Bool(true));
        inner
    });
    changed
}

fn apply_seven_prefs(prefs: &mut Map<String, Value>, unix_micros: i64) -> bool {
    let mut changed = false;
    changed |= set_pref(prefs, "tab_freezing_enabled", Value::Bool(false));
    with_child(prefs, "performance_tuning", &mut changed, |tuning| {
        let mut inner = false;
        with_child(tuning, "tab_freezing", &mut inner, |tab| {
            set_pref(tab, "enabled", Value::Bool(false))
        });
        with_child(tuning, "high_efficiency_mode", &mut inner, |mode| {
            set_pref(mode, "state", Value::Number(serde_json::Number::from(0u64)))
        });
        with_child(tuning, "tab_discarding", &mut inner, |discard| {
            let mut sites = false;
            sites |= merge_site_list(discard);
            with_child(discard, "exceptions_with_time", &mut sites, |times| {
                if times.contains_key(SEVEN_SITE) {
                    false
                } else {
                    times.insert(
                        SEVEN_SITE.to_string(),
                        Value::String(chrome_time(unix_micros)),
                    );
                    true
                }
            });
            sites
        });
        inner
    });
    changed
}

fn merge_site_list(discard: &mut Map<String, Value>) -> bool {
    let mut have = Vec::new();
    let mut changed = false;
    match discard.get("exceptions") {
        Some(Value::Array(list)) => {
            for item in list {
                if let Some(site) = item.as_str() {
                    if !have.iter().any(|kept: &String| kept == site) {
                        have.push(site.to_string());
                    } else {
                        changed = true;
                    }
                } else {
                    changed = true;
                }
            }
            if have.len() != list.len() {
                changed = true;
            }
        }
        _ => changed = true,
    }
    if !have.iter().any(|site| site == SEVEN_SITE) {
        have.push(SEVEN_SITE.to_string());
        changed = true;
    }
    if changed {
        discard.insert(
            "exceptions".to_string(),
            Value::Array(have.into_iter().map(Value::String).collect()),
        );
    }
    changed
}

fn chrome_time(unix_micros: i64) -> String {
    (unix_micros + CHROME_EPOCH_MICROS).to_string()
}

fn prepare_profile<F: ProfileFs>(
    command: &str,
    dir: &str,
    windows: bool,
    now_micros: i64,
    fs: &mut F,
) -> Result<(), String> {
    if let Some(pid) = profile_owner(dir, windows, fs) {
        return Err(format!(
            "playwright-cli {command}: profile is in use: {dir} is held by process {pid}\n"
        ));
    }
    if under_seven_profiles(dir) {
        let prefs_path = child_path(dir, "Default/Preferences", windows);
        let existing = fs.read(&prefs_path);
        match seed_preferences(existing.as_deref(), now_micros) {
            Err(()) => {
                return Err(format!(
                    "playwright-cli {command}: {prefs_path}: not a JSON object, left alone\n"
                ));
            }
            Ok(Some(body)) => fs.write(&prefs_path, &body)?,
            Ok(None) => {}
        }
    }
    for name in [
        ACTIVE_PORT_FILE,
        "SingletonLock",
        "SingletonSocket",
        "SingletonCookie",
    ] {
        fs.remove(&child_path(dir, name, windows));
    }
    Ok(())
}

enum Route {
    Attach { discover: String },
    Launch(SpawnPlan),
}

fn route<F: ProfileFs>(
    request: &LaunchRequest,
    env: &HostEnv,
    electron_port: u16,
    now_micros: i64,
    exists: &dyn Fn(&str) -> bool,
    mdfind: &dyn Fn(&str) -> Vec<String>,
    read_plist: &dyn Fn(&str) -> Option<String>,
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
    fs: &mut F,
) -> Result<Route, String> {
    let plan = plan_launch(
        request,
        env,
        electron_port,
        exists,
        mdfind,
        read_plist,
    )?;
    let windows = windows_host(env);
    if let Some(discover) = live_port(&plan.user_data_dir, windows, fs, probe)? {
        return Ok(Route::Attach { discover });
    }
    prepare_profile(request.command, &plan.user_data_dir, windows, now_micros, fs)?;
    Ok(Route::Launch(plan))
}

#[cfg(test)]
fn drive<F: ProfileFs>(
    request: &LaunchRequest,
    env: &HostEnv,
    electron_port: u16,
    now_micros: i64,
    exists: &dyn Fn(&str) -> bool,
    mdfind: &dyn Fn(&str) -> Vec<String>,
    read_plist: &dyn Fn(&str) -> Option<String>,
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
    fs: &mut F,
    spawn: &mut dyn FnMut(&SpawnPlan) -> Result<(), String>,
) -> Result<Resolved, String> {
    match route(
        request,
        env,
        electron_port,
        now_micros,
        exists,
        mdfind,
        read_plist,
        probe,
        fs,
    )? {
        Route::Attach { discover } => Ok(Resolved {
            discover,
            started: None,
        }),
        Route::Launch(plan) => {
            spawn(&plan)?;
            Ok(Resolved {
                discover: plan.user_data_dir,
                started: None,
            })
        }
    }
}

fn not_found(command: &str, name: &str) -> String {
    format!("playwright-cli {command}: {name} was not found\n")
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn host_open(request: &LaunchRequest) -> Result<Resolved, String> {
    let env = current_env();
    let electron = request
        .browser
        .and_then(find_known)
        .is_some_and(|known| known.kind == Kind::Electron);
    let electron_port = if electron {
        choose_port(bind_localhost)?
    } else {
        0
    };
    let mut fs = DiskFs;
    let now = unix_micros();
    match route(
        request,
        &env,
        electron_port,
        now,
        &file_exists,
        &spotlight,
        &read_plist_file,
        &mut |url| http_body(url),
        &mut fs,
    )? {
        Route::Attach { discover } => Ok(Resolved {
            discover,
            started: None,
        }),
        Route::Launch(plan) => launch_plan(request, &plan),
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn launch_plan(request: &LaunchRequest, plan: &SpawnPlan) -> Result<Resolved, String> {
    if let Err(err) = std::fs::create_dir_all(&plan.user_data_dir) {
        return Err(format!(
            "playwright-cli {}: could not create the browser profile: {err}\n",
            request.command
        ));
    }
    let mut child = match spawn_child(plan) {
        Ok(child) => child,
        Err(message) => {
            if plan.delete_on_failure {
                let _ = std::fs::remove_dir_all(&plan.user_data_dir);
            }
            return Err(message);
        }
    };
    let version_url = match wait_for_debugger(request.command, plan, &mut child) {
        Ok(url) => url,
        Err(message) => {
            stop_started(plan, &mut child);
            return Err(message);
        }
    };
    let owner = if plan.via_open {
        observed_owner(&plan.user_data_dir)
    } else {
        None
    };
    let via_open = plan.via_open;
    let delete = plan.delete_on_failure;
    let dir = plan.user_data_dir.clone();
    let started = Started::running(
        version_url,
        Box::new(move || {
            if via_open {
                if let Some(pid) = owner {
                    kill_pid(pid);
                }
                if matches!(child.try_wait(), Ok(None)) {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            } else {
                kill_child(&mut child);
            }
            if delete {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }),
    );
    let discover = started.version_url.clone();
    Ok(Resolved {
        discover,
        started: Some(started),
    })
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn wait_for_debugger(
    command: &str,
    plan: &SpawnPlan,
    child: &mut std::process::Child,
) -> Result<String, String> {
    if plan.port != 0 {
        wait_ready(plan.port, child)?;
        return Ok(version_url(plan.port));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let windows = cfg!(windows);
    loop {
        if child_failed(plan, child) {
            return Err(
                "playwright-cli: browser exited before the debugger was ready\n".to_string(),
            );
        }
        let port_path = child_path(&plan.user_data_dir, ACTIVE_PORT_FILE, windows);
        if let Some((port, path)) = std::fs::read_to_string(&port_path)
            .ok()
            .as_deref()
            .and_then(parse_active_port)
        {
            if (9222..=9229).contains(&port) {
                return Err(format!(
                    "playwright-cli {command}: refusing debugger port {port}\n"
                ));
            }
            let url = version_url(port);
            match http_body(&url) {
                Ok(body) if version_accepts(&body, &path) => return Ok(url),
                Ok(_) => {}
                Err(message) if message.starts_with("HTTP ") => return Err(message),
                Err(_) => {}
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(
                "playwright-cli: timed out waiting for the browser debugger\n".to_string(),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn child_failed(plan: &SpawnPlan, child: &mut std::process::Child) -> bool {
    match child.try_wait() {
        Ok(Some(status)) => !(plan.via_open && status.success()),
        Ok(None) => false,
        Err(_) => true,
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn stop_started(plan: &SpawnPlan, child: &mut std::process::Child) {
    if plan.via_open {
        if let Some(pid) = observed_owner(&plan.user_data_dir) {
            kill_pid(pid);
        }
        if matches!(child.try_wait(), Ok(None)) {
            let _ = child.kill();
            let _ = child.wait();
        }
    } else {
        kill_child(child);
    }
    if plan.delete_on_failure {
        let _ = std::fs::remove_dir_all(&plan.user_data_dir);
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn observed_owner(dir: &str) -> Option<u32> {
    let path = child_path(dir, "SingletonLock", cfg!(windows));
    let target = std::fs::read_link(path).ok()?;
    let pid = lock_pid(&target.to_string_lossy())?;
    pid_alive(pid).then_some(pid)
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
struct DiskFs;

#[cfg(all(not(test), not(target_arch = "wasm32")))]
impl ProfileFs for DiskFs {
    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn read_link(&self, path: &str) -> Option<String> {
        std::fs::read_link(path)
            .ok()
            .map(|target| target.to_string_lossy().into_owned())
    }

    fn pid_alive(&self, pid: u32) -> bool {
        pid_alive(pid)
    }

    fn remove(&mut self, path: &str) {
        let _ = std::fs::remove_file(path);
    }

    fn write(&mut self, path: &str, body: &str) -> Result<(), String> {
        let parent = std::path::Path::new(path).parent();
        if let Some(parent) = parent {
            std::fs::create_dir_all(parent).map_err(|err| {
                format!("playwright-cli: could not write preferences: {err}\n")
            })?;
        }
        let tmp = format!("{path}.slicc-tmp");
        std::fs::write(&tmp, body).map_err(|err| {
            format!("playwright-cli: could not write preferences: {err}\n")
        })?;
        std::fs::rename(&tmp, path).map_err(|err| {
            format!("playwright-cli: could not write preferences: {err}\n")
        })
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
impl ScanFs for DiskFs {
    fn list_dirs(&self, path: &str) -> Vec<String> {
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
        names.sort();
        names
    }

    fn read_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn modified(&self, path: &str) -> Option<i64> {
        let modified = std::fs::metadata(path).ok()?.modified().ok()?;
        let micros = modified.duration_since(std::time::UNIX_EPOCH).ok()?.as_micros();
        i64::try_from(micros).ok()
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn unix_micros() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_micros() as i64)
        .unwrap_or(0)
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn current_env() -> HostEnv {
    HostEnv {
        platform: host_platform(),
        chrome_path: env_var("CHROME_PATH"),
        local_app_data: env_var("LOCALAPPDATA"),
        program_files: env_var("PROGRAMFILES"),
        program_files_x86: env_var("PROGRAMFILES(X86)"),
        temp_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        unique: unique_name(),
        home: home_dir(),
        app_data: env_var("APPDATA"),
        xdg_config_home: env_var("XDG_CONFIG_HOME"),
        snap_name: env_var("SNAP_NAME"),
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn home_dir() -> String {
    if cfg!(windows) {
        if let Some(home) = env_var("USERPROFILE") {
            return home;
        }
    }
    if let Some(home) = env_var("HOME") {
        return home;
    }
    env_var("USERPROFILE").unwrap_or_default()
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "other"
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn unique_name() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn file_exists(path: &str) -> bool {
    std::path::Path::new(path).is_file()
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn read_plist_file(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    executable_from_plist(&text)
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn bind_localhost(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

#[cfg(all(not(test), not(target_arch = "wasm32"), target_os = "macos"))]
fn spotlight(bundle_id: &str) -> Vec<String> {
    let output = std::process::Command::new("mdfind")
        .arg(mdfind_query(bundle_id))
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8(output.stdout)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(all(not(test), not(target_arch = "wasm32"), not(target_os = "macos")))]
fn spotlight(_bundle_id: &str) -> Vec<String> {
    Vec::new()
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn wait_ready(port: u16, child: &mut std::process::Child) -> Result<(), String> {
    let url = version_url(port);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let running = matches!(child.try_wait(), Ok(None));
        let body = if running {
            http_body(&url)
        } else {
            Err("closed\n".to_string())
        };
        match poll_version(body, running) {
            Ok(Poll::Ready) => return Ok(()),
            Ok(Poll::Again) => {}
            Err(message) => return Err(message),
        }
        if std::time::Instant::now() >= deadline {
            return Err("playwright-cli: timed out waiting for the browser debugger\n".to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn http_body(url: &str) -> Result<String, String> {
    let response = crate::net::http_get(url)?;
    Ok(String::from_utf8_lossy(&response.body).into_owned())
}

#[cfg(all(not(test), not(target_arch = "wasm32"), unix))]
fn spawn_child(plan: &SpawnPlan) -> Result<std::process::Child, String> {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(&plan.program)
        .args(&plan.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|err| format!("playwright-cli: failed to launch browser: {err}\n"))
}

#[cfg(all(not(test), not(target_arch = "wasm32"), windows))]
fn spawn_child(plan: &SpawnPlan) -> Result<std::process::Child, String> {
    std::process::Command::new(&plan.program)
        .args(&plan.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|err| format!("playwright-cli: failed to launch browser: {err}\n"))
}

#[cfg(all(not(test), not(target_arch = "wasm32"), unix))]
fn kill_child(child: &mut std::process::Child) {
    let pid = child.id() as i32;
    unsafe {
        kill(-pid, 9);
    }
    let _ = child.wait();
}

#[cfg(all(not(test), not(target_arch = "wasm32"), unix))]
fn kill_pid(pid: u32) {
    if pid != 0 {
        unsafe {
            kill(pid as i32, 9);
        }
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32"), unix))]
fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let rc = unsafe { kill(pid as i32, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1)
}

#[cfg(all(not(test), not(target_arch = "wasm32"), windows))]
fn kill_child(child: &mut std::process::Child) {
    kill_pid(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(not(test), not(target_arch = "wasm32"), windows))]
fn kill_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(all(not(test), not(target_arch = "wasm32"), windows))]
fn pid_alive(pid: u32) -> bool {
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
    }
    if pid == 0 {
        return false;
    }
    let handle = unsafe { OpenProcess(0x1000, 0, pid) };
    if handle.is_null() {
        false
    } else {
        unsafe {
            CloseHandle(handle);
        }
        true
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32"), unix))]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::rc::Rc;

    fn env(platform: &'static str) -> HostEnv {
        HostEnv {
            platform,
            chrome_path: None,
            local_app_data: None,
            program_files: None,
            program_files_x86: None,
            temp_dir: "/tmp".to_string(),
            unique: "test".to_string(),
            home: match platform {
                "macos" => "/Users/me".to_string(),
                "windows" => "C:\\Users\\me".to_string(),
                _ => "/home/me".to_string(),
            },
            app_data: None,
            xdg_config_home: None,
            snap_name: None,
        }
    }

    fn no_plist(_path: &str) -> Option<String> {
        None
    }

    fn no_mdfind(_bundle: &str) -> Vec<String> {
        Vec::new()
    }

    struct Rec {
        calls: u32,
        headed: bool,
        browser: Option<String>,
        stop: Rc<Cell<bool>>,
    }

    impl Launcher for Rec {
        fn launch(&mut self, request: &LaunchRequest) -> Result<Started, String> {
            self.calls += 1;
            self.headed = request.headed;
            self.browser = request.browser.map(str::to_string);
            let stop = self.stop.clone();
            Ok(Started::running(
                "http://127.0.0.1:9333/json/version".to_string(),
                Box::new(move || stop.set(true)),
            ))
        }
    }

    fn recorder() -> (Rec, Rc<Cell<bool>>) {
        let stop = Rc::new(Cell::new(false));
        (
            Rec {
                calls: 0,
                headed: false,
                browser: None,
                stop: stop.clone(),
            },
            stop,
        )
    }

    #[test]
    fn host_only_line_names_the_flags() {
        assert_eq!(
            host_only_line("open", &["--headed".to_string()]),
            "playwright-cli open: --headed is host-only\n"
        );
        assert_eq!(
            host_only_line(
                "tab-new",
                &[
                    "--browser".to_string(),
                    "--headed".to_string(),
                    "--profile".to_string()
                ]
            ),
            "playwright-cli tab-new: --browser, --headed, --profile are host-only\n"
        );
    }

    #[test]
    fn host_flags_are_accepted_on_the_host_build() {
        let mut flags = BTreeMap::new();
        flags.insert("headed".to_string(), "true".to_string());
        assert!(reject_wasm_flags("open", &flags).is_none());
    }

    #[test]
    fn launch_only_open_without_an_endpoint() {
        assert!(should_launch("open", false));
        assert!(should_launch("tab-new", false));
        assert!(!should_launch("open", true));
        assert!(!should_launch("goto", false));
        assert!(endpoint_is_explicit(Some(" http://127.0.0.1:9333 "), None));
        assert!(endpoint_is_explicit(None, Some("ws://127.0.0.1:9333/devtools/browser/abc")));
        assert!(!endpoint_is_explicit(Some("  "), Some("")));
    }

    #[test]
    fn ports_stay_at_or_above_9333() {
        assert!(!port_allowed(9222));
        assert!(!port_allowed(9229));
        assert!(!port_allowed(9332));
        assert!(port_allowed(9333));
        assert!(port_allowed(65535));
        let mut seen = Vec::new();
        let port = choose_port(|port| {
            seen.push(port);
            port == 9334
        })
        .unwrap();
        assert_eq!(port, 9334);
        assert!(seen.iter().all(|port| port_allowed(*port)));
        assert!(!seen.iter().any(|port| (9222..=9229).contains(port)));
    }

    #[test]
    fn version_poll_uses_the_launched_port() {
        assert_eq!(version_url(9333), "http://127.0.0.1:9333/json/version");
        assert!(!version_url(9333).contains("9222"));
        let body = r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:9333/devtools/browser/abc"}"#;
        assert_eq!(
            poll_version(Ok(body.to_string()), true).unwrap(),
            Poll::Ready
        );
        assert_eq!(
            poll_version(Err("connection refused\n".to_string()), true).unwrap(),
            Poll::Again
        );
        assert!(poll_version(Err("HTTP 500\n".to_string()), true).is_err());
        assert!(poll_version(Ok(body.to_string()), false).is_err());
    }

    #[test]
    fn plist_and_mdfind_query() {
        let plist = "<key>CFBundleExecutable</key>\n<string>Google Chrome</string>";
        assert_eq!(
            executable_from_plist(plist).as_deref(),
            Some("Google Chrome")
        );
        assert_eq!(
            mdfind_query("com.google.Chrome"),
            "kMDItemCFBundleIdentifier == \"com.google.Chrome\""
        );
    }

    #[test]
    fn firefox_and_webkit_do_not_launch() {
        let (mut launcher, _) = recorder();
        let mut probes = 0;
        for name in ["firefox", "Firefox", "webkit", "WebKit"] {
            let err = decide(
                "open",
                Some(name),
                false,
                None,
                &mut || {
                    probes += 1;
                    Ok(())
                },
                &mut launcher,
            )
            .unwrap_err();
            let key = name.to_ascii_lowercase();
            assert_eq!(
                err,
                format!("playwright-cli open: {key} is not launchable\n")
            );
            assert!(!err.to_ascii_lowercase().contains("download"));
            assert!(!err.contains("9222"));
            assert_eq!(probes, 0);
            assert_eq!(launcher.calls, 0);
        }
        let err = decide(
            "open",
            Some("lynx"),
            false,
            None,
            &mut || Ok(()),
            &mut launcher,
        )
        .unwrap_err();
        assert_eq!(err, "playwright-cli open: unknown browser \"lynx\"\n");
    }

    #[test]
    fn an_existing_debugger_does_not_launch() {
        let (mut launcher, stop) = recorder();
        let mut probes = 0;
        let resolved = decide(
            "open",
            Some("chrome"),
            true,
            None,
            &mut || {
                probes += 1;
                Ok(())
            },
            &mut launcher,
        )
        .unwrap();
        assert_eq!(probes, 1);
        assert_eq!(launcher.calls, 0);
        assert!(resolved.started.is_none());
        assert!(!resolved.discover.contains("9222"));
        drop(resolved);
        assert!(!stop.get());
    }

    #[test]
    fn http_refusal_does_not_launch() {
        let (mut launcher, _) = recorder();
        let err = decide(
            "open",
            None,
            false,
            None,
            &mut || Err("HTTP 503\nno host\n".to_string()),
            &mut launcher,
        )
        .unwrap_err();
        assert_eq!(err, "HTTP 503\nno host\n");
        assert_eq!(launcher.calls, 0);
    }

    #[test]
    fn a_missing_debugger_uses_the_injected_launcher() {
        let (mut launcher, stop) = recorder();
        let resolved = decide(
            "open",
            None,
            true,
            Some("/tmp/profile"),
            &mut || Err("no debugger is connected\n".to_string()),
            &mut launcher,
        )
        .unwrap();
        assert_eq!(launcher.calls, 1);
        assert!(launcher.headed);
        assert_eq!(launcher.browser, None);
        assert_eq!(resolved.discover, "http://127.0.0.1:9333/json/version");
        assert!(!resolved.discover.contains("9222"));
        drop(resolved);
        assert!(stop.get());
    }

    #[test]
    fn disarm_leaves_the_launched_process_running() {
        let (mut launcher, stop) = recorder();
        let mut resolved = decide(
            "tab-new",
            Some("Slack"),
            false,
            None,
            &mut || Err("absent\n".to_string()),
            &mut launcher,
        )
        .unwrap();
        assert_eq!(launcher.browser.as_deref(), Some("Slack"));
        resolved.started.as_mut().unwrap().disarm();
        drop(resolved);
        assert!(!stop.get());
    }

    #[test]
    fn chrome_path_wins_then_installed_chrome() {
        let mut host = env("macos");
        host.chrome_path = Some("/custom/chrome".to_string());
        let request = LaunchRequest {
            command: "open",
            browser: None,
            headed: false,
            profile: None,
        };
        let queries = Rc::new(RefCell::new(Vec::new()));
        let seen = queries.clone();
        let plan = plan_launch(
            &request,
            &host,
            9333,
            &|path| path == "/custom/chrome",
            &|bundle| {
                seen.borrow_mut().push(bundle.to_string());
                Vec::new()
            },
            &no_plist,
        )
        .unwrap();
        assert_eq!(plan.program, "/custom/chrome");
        assert!(queries.borrow().is_empty());
        assert_eq!(plan.port, 0);
        assert!(!plan.via_open);
        assert!(!plan.delete_on_failure);
        assert_eq!(
            plan.user_data_dir,
            "/Users/me/Library/Application Support/slicc/seven-profiles/chrome"
        );
        assert_eq!(
            plan.args,
            vec![
                "--remote-debugging-port=0".to_string(),
                "--user-data-dir=/Users/me/Library/Application Support/slicc/seven-profiles/chrome"
                    .to_string(),
                "--no-first-run".to_string(),
                "--no-default-browser-check".to_string(),
                "--disable-background-timer-throttling".to_string(),
                "--disable-backgrounding-occluded-windows".to_string(),
                "--disable-renderer-backgrounding".to_string(),
                "--disable-background-tracing".to_string(),
                format!("--disable-features={SEVEN_FEATURES}"),
                "--headless=new".to_string(),
            ]
        );
        assert!(!plan.args.iter().any(|arg| arg.contains("9222")));
        assert!(!plan.args.iter().any(|arg| arg == "--headless"));
        assert!(!plan.user_data_dir.contains("/slicc/profiles"));

        host.chrome_path = Some("/missing".to_string());
        let plan = plan_launch(
            &request,
            &host,
            9333,
            &|path| path == "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            &no_mdfind,
            &no_plist,
        )
        .unwrap();
        assert_eq!(
            plan.program,
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        );
    }

    #[test]
    fn installed_chrome_order_matches_the_known_paths() {
        let mut host = env("linux");
        assert_eq!(
            installed_chrome_candidates(&host),
            vec![
                "/usr/bin/google-chrome",
                "/usr/bin/google-chrome-stable",
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser",
                "/snap/bin/chromium",
            ]
        );
        host.platform = "windows";
        host.local_app_data = Some("C:\\Users\\me\\AppData\\Local".to_string());
        host.program_files = Some("C:\\Program Files".to_string());
        host.program_files_x86 = Some("C:\\Program Files (x86)".to_string());
        assert_eq!(
            installed_chrome_candidates(&host),
            vec![
                "C:\\Users\\me\\AppData\\Local\\Google\\Chrome\\Application\\chrome.exe",
                "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
                "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
            ]
        );
    }

    #[test]
    fn default_launch_never_selects_an_electron_app() {
        let host = env("macos");
        let request = LaunchRequest {
            command: "open",
            browser: None,
            headed: false,
            profile: None,
        };
        let queries = Rc::new(RefCell::new(Vec::new()));
        let seen = queries.clone();
        let err = plan_launch(
            &request,
            &host,
            9333,
            &|path| path.contains("Slack"),
            &|bundle| {
                seen.borrow_mut().push(bundle.to_string());
                Vec::new()
            },
            &no_plist,
        )
        .unwrap_err();
        assert_eq!(err, "playwright-cli open: chrome was not found\n");
        assert_eq!(
            queries.borrow().as_slice(),
            [
                "com.google.Chrome",
                "com.google.Chrome.canary",
                "org.chromium.Chromium",
            ]
        );
    }

    #[test]
    fn mdfind_resolves_the_bundle_executable() {
        let host = env("macos");
        let request = LaunchRequest {
            command: "open",
            browser: None,
            headed: true,
            profile: Some("/tmp/my-profile"),
        };
        let plan = plan_launch(
            &request,
            &host,
            9400,
            &|path| path == "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            &|bundle| {
                if bundle == "com.google.Chrome" {
                    vec!["/Applications/Google Chrome.app".to_string()]
                } else {
                    Vec::new()
                }
            },
            &|path| {
                (path == "/Applications/Google Chrome.app/Contents/Info.plist")
                    .then(|| "Google Chrome".to_string())
            },
        )
        .unwrap();
        assert_eq!(plan.program, "/usr/bin/open");
        assert_eq!(
            &plan.args[..4],
            [
                "-n".to_string(),
                "-a".to_string(),
                "/Applications/Google Chrome.app".to_string(),
                "--args".to_string(),
            ]
        );
        assert!(!plan.args.iter().any(|arg| arg == "-W"));
        assert!(plan.via_open);
        assert_eq!(plan.user_data_dir, "/tmp/my-profile");
        assert!(!plan.args.iter().any(|arg| arg.contains("headless")));
        assert!(plan.args.contains(&"--no-first-run".to_string()));
        assert!(plan
            .args
            .contains(&"--user-data-dir=/tmp/my-profile".to_string()));
        assert!(plan.args.contains(&"--remote-debugging-port=0".to_string()));
    }

    #[test]
    fn headed_chrome_omits_headless_and_edge_uses_its_path() {
        let host = env("linux");
        let request = LaunchRequest {
            command: "open",
            browser: Some("msedge"),
            headed: true,
            profile: None,
        };
        let plan = plan_launch(
            &request,
            &host,
            9333,
            &|path| path == "/usr/bin/microsoft-edge",
            &no_mdfind,
            &no_plist,
        )
        .unwrap();
        assert_eq!(plan.program, "/usr/bin/microsoft-edge");
        assert!(!plan.via_open);
        assert!(!plan.args.iter().any(|arg| arg.contains("headless")));
        assert!(plan.args.iter().any(|arg| arg == "--no-first-run"));
        assert!(plan
            .args
            .contains(&"--remote-debugging-port=0".to_string()));
        assert_eq!(
            plan.user_data_dir,
            "/home/me/.config/slicc/seven-profiles/edge"
        );
    }

    #[test]
    fn an_electron_app_gets_only_the_debugging_args() {
        let host = env("macos");
        let request = LaunchRequest {
            command: "open",
            browser: Some("Slack"),
            headed: false,
            profile: None,
        };
        let queries = Rc::new(RefCell::new(Vec::new()));
        let seen = queries.clone();
        let plan = plan_launch(
            &request,
            &host,
            9333,
            &|path| path == "/Applications/Slack.app/Contents/MacOS/Slack",
            &|bundle| {
                seen.borrow_mut().push(bundle.to_string());
                vec!["/Applications/Slack.app".to_string()]
            },
            &|path| (path.ends_with("Info.plist")).then(|| "Slack".to_string()),
        )
        .unwrap();
        assert_eq!(plan.program, "/Applications/Slack.app/Contents/MacOS/Slack");
        assert_eq!(
            plan.args,
            vec![
                "--remote-debugging-port=9333".to_string(),
                "--user-data-dir=/tmp/playwright-cli-test".to_string(),
            ]
        );
        assert!(queries.borrow().is_empty());
        assert!(!plan.user_data_dir.contains("seven-profiles"));
        assert!(!plan.args.iter().any(|arg| arg.contains("disable-features")));
        assert!(!plan.args.iter().any(|arg| arg.contains("headless")));
        assert!(plan.delete_on_failure);
    }

    #[test]
    fn a_forbidden_port_is_refused_before_launch() {
        let host = env("linux");
        let request = LaunchRequest {
            command: "open",
            browser: Some("Slack"),
            headed: false,
            profile: None,
        };
        let err = plan_launch(
            &request,
            &host,
            9228,
            &|_| true,
            &no_mdfind,
            &no_plist,
        )
        .unwrap_err();
        assert!(err.contains("refusing debugger port"));
        assert!(!err.contains("9222"));
    }

    #[test]
    fn resolve_in_tests_does_not_spawn_or_dial() {
        let mut flags = BTreeMap::new();
        flags.insert("headed".to_string(), "true".to_string());
        let err = resolve_without_endpoint("open", &flags).unwrap_err();
        assert_eq!(
            err,
            "playwright-cli: browser launch is not available in tests\n"
        );
        assert!(!err.contains("9222"));
        flags.insert("browser".to_string(), "firefox".to_string());
        let err = resolve_without_endpoint("open", &flags).unwrap_err();
        assert_eq!(err, "playwright-cli open: firefox is not launchable\n");
    }

    fn installed(path: &str) -> bool {
        matches!(
            path,
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
                | "/usr/bin/google-chrome"
                | "/usr/bin/slack"
                | "/snap/bin/chromium"
                | "C:\\Users\\me\\AppData\\Local\\Google\\Chrome\\Application\\chrome.exe"
        )
    }

    #[derive(Default)]
    struct Mem {
        files: BTreeMap<String, String>,
        links: BTreeMap<String, String>,
        alive: Vec<u32>,
        removed: Vec<String>,
        writes: Vec<String>,
        reads: Vec<String>,
    }

    struct MemFs(RefCell<Mem>);

    impl MemFs {
        fn new() -> Self {
            Self(RefCell::new(Mem::default()))
        }
    }

    impl ProfileFs for MemFs {
        fn read(&self, path: &str) -> Option<String> {
            let mut mem = self.0.borrow_mut();
            mem.reads.push(path.to_string());
            mem.files.get(path).cloned()
        }

        fn read_link(&self, path: &str) -> Option<String> {
            let mut mem = self.0.borrow_mut();
            mem.reads.push(path.to_string());
            mem.links.get(path).cloned()
        }

        fn pid_alive(&self, pid: u32) -> bool {
            self.0.borrow().alive.contains(&pid)
        }

        fn remove(&mut self, path: &str) {
            let mut mem = self.0.borrow_mut();
            mem.removed.push(path.to_string());
            mem.files.remove(path);
            mem.links.remove(path);
        }

        fn write(&mut self, path: &str, body: &str) -> Result<(), String> {
            let mut mem = self.0.borrow_mut();
            mem.writes.push(path.to_string());
            mem.files.insert(path.to_string(), body.to_string());
            Ok(())
        }
    }

    fn open_request<'a>(
        browser: Option<&'a str>,
        headed: bool,
        profile: Option<&'a str>,
    ) -> LaunchRequest<'a> {
        LaunchRequest {
            command: "open",
            browser,
            headed,
            profile,
        }
    }

    fn run(
        request: &LaunchRequest,
        host: &HostEnv,
        port: u16,
        fs: &mut MemFs,
        probe: &mut dyn FnMut(&str) -> Result<String, String>,
        spawn: &mut dyn FnMut(&SpawnPlan) -> Result<(), String>,
    ) -> Result<Resolved, String> {
        drive(
            request,
            host,
            port,
            50,
            &installed,
            &no_mdfind,
            &no_plist,
            probe,
            fs,
            spawn,
        )
    }

    fn absent(url: &str) -> Result<String, String> {
        let _ = url;
        Err("no debugger is connected\n".to_string())
    }

    fn chrome_dir(host: &HostEnv) -> String {
        plan_launch(
            &open_request(None, false, None),
            host,
            0,
            &installed,
            &no_mdfind,
            &no_plist,
        )
        .unwrap()
        .user_data_dir
    }

    fn assert_no_six(paths: &[String]) {
        for path in paths {
            let parts: Vec<String> = path
                .replace('\\', "/")
                .split('/')
                .filter(|part| !part.is_empty())
                .map(|part| part.to_ascii_lowercase())
                .collect();
            assert!(
                !parts
                    .windows(2)
                    .any(|window| window[0] == "slicc" && window[1] == "profiles"),
                "{path}"
            );
            assert!(
                !path
                    .to_ascii_lowercase()
                    .contains("browser-coding-agent-chrome"),
                "{path}"
            );
        }
    }

    #[test]
    fn seven_profile_directories() {
        let macos = chrome_dir(&env("macos"));
        assert_eq!(
            macos,
            "/Users/me/Library/Application Support/slicc/seven-profiles/chrome"
        );
        let linux = chrome_dir(&env("linux"));
        assert_eq!(linux, "/home/me/.config/slicc/seven-profiles/chrome");
        let mut xdg = env("linux");
        xdg.xdg_config_home = Some("/srv/xdg".to_string());
        assert_eq!(
            chrome_dir(&xdg),
            "/srv/xdg/slicc/seven-profiles/chrome"
        );
        xdg.xdg_config_home = Some("relative/config".to_string());
        assert_eq!(
            chrome_dir(&xdg),
            "/home/me/.config/slicc/seven-profiles/chrome"
        );
        let mut snap = env("linux");
        snap.snap_name = Some("slicc".to_string());
        assert_eq!(
            chrome_dir(&snap),
            "/home/me/snap/slicc/common/slicc/seven-profiles/chrome"
        );
        let named = open_request(Some("chromium"), false, None);
        let plan = plan_launch(&named, &snap, 0, &installed, &no_mdfind, &no_plist).unwrap();
        assert_eq!(plan.program, "/snap/bin/chromium");
        assert_eq!(
            plan.user_data_dir,
            "/home/me/snap/chromium/common/slicc/seven-profiles/chromium"
        );
        let mut windows = env("windows");
        windows.local_app_data = Some("C:\\Users\\me\\AppData\\Local".to_string());
        windows.app_data = Some("C:\\Users\\me\\AppData\\Roaming".to_string());
        assert_eq!(
            chrome_dir(&windows),
            "C:\\Users\\me\\AppData\\Roaming\\slicc\\seven-profiles\\chrome"
        );
    }

    #[test]
    fn a_live_port_attaches_without_launch_or_9222() {
        let host = env("macos");
        let dir = chrome_dir(&host);
        let mut fs = MemFs::new();
        fs.0.borrow_mut().files.insert(
            format!("{dir}/{ACTIVE_PORT_FILE}"),
            "9333\n/devtools/browser/abc\n".to_string(),
        );
        let mut seen = Vec::new();
        let resolved = run(
            &open_request(None, false, None),
            &host,
            9333,
            &mut fs,
            &mut |url| {
                seen.push(url.to_string());
                assert!(!url.contains("9222"), "{url}");
                Ok(
                    r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:9333/devtools/browser/abc"}"#
                        .to_string(),
                )
            },
            &mut |_| panic!("spawned"),
        )
        .unwrap();
        assert!(resolved.started.is_none());
        assert_eq!(resolved.discover, "http://127.0.0.1:9333/json/version");
        assert_eq!(seen, vec!["http://127.0.0.1:9333/json/version".to_string()]);
        assert!(fs.0.borrow().removed.is_empty());
        assert!(fs.0.borrow().writes.is_empty());
        assert_no_six(&fs.0.borrow().reads);
    }

    #[test]
    fn a_stale_port_falls_through_and_the_websocket_path_must_match() {
        let host = env("linux");
        let dir = chrome_dir(&host);
        let mut fs = MemFs::new();
        fs.0.borrow_mut().files.insert(
            format!("{dir}/{ACTIVE_PORT_FILE}"),
            "9333\n/devtools/browser/abc\n".to_string(),
        );
        let mut seen = Vec::new();
        let mut spawned = None;
        let resolved = run(
            &open_request(None, false, None),
            &host,
            0,
            &mut fs,
            &mut |url| {
                seen.push(url.to_string());
                Ok(
                    r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:9333/devtools/browser/other"}"#
                        .to_string(),
                )
            },
            &mut |plan| {
                spawned = Some(plan.clone());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(seen, vec!["http://127.0.0.1:9333/json/version".to_string()]);
        let plan = spawned.unwrap();
        assert_eq!(plan.port, 0);
        assert_eq!(plan.user_data_dir, dir);
        assert_eq!(resolved.discover, dir);
        assert!(fs.0.borrow().writes.len() == 1);
        assert_no_six(&fs.0.borrow().reads);
    }

    #[test]
    fn a_forbidden_port_file_is_not_probed() {
        let host = env("linux");
        let dir = chrome_dir(&host);
        let mut fs = MemFs::new();
        fs.0.borrow_mut().files.insert(
            format!("{dir}/{ACTIVE_PORT_FILE}"),
            "9224\n/devtools/browser/abc\n".to_string(),
        );
        let mut seen = Vec::new();
        let resolved = run(
            &open_request(None, false, None),
            &host,
            0,
            &mut fs,
            &mut |url| {
                seen.push(url.to_string());
                panic!("probed {url}");
            },
            &mut |_| Ok(()),
        )
        .unwrap();
        assert!(seen.is_empty());
        assert_eq!(resolved.discover, dir);
        assert!(!resolved.discover.contains("9222"));
    }

    #[test]
    fn an_absent_port_file_launches_without_9222() {
        let host = env("linux");
        let mut fs = MemFs::new();
        let mut seen = Vec::new();
        let resolved = run(
            &open_request(None, false, None),
            &host,
            0,
            &mut fs,
            &mut |url| {
                seen.push(url.to_string());
                panic!("probed {url}");
            },
            &mut |_| Ok(()),
        )
        .unwrap();
        assert!(seen.is_empty());
        assert_eq!(resolved.discover, chrome_dir(&host));
        assert!(!resolved.discover.contains("9222"));
    }

    #[test]
    fn an_http_error_from_the_port_file_does_not_launch() {
        let host = env("linux");
        let dir = chrome_dir(&host);
        let mut fs = MemFs::new();
        fs.0.borrow_mut().files.insert(
            format!("{dir}/{ACTIVE_PORT_FILE}"),
            "9400\n/devtools/browser/abc\n".to_string(),
        );
        let err = run(
            &open_request(None, false, None),
            &host,
            0,
            &mut fs,
            &mut |_| Err("HTTP 500\nno\n".to_string()),
            &mut |_| panic!("spawned"),
        )
        .unwrap_err();
        assert_eq!(err, "HTTP 500\nno\n");
    }

    #[test]
    fn preferences_are_seeded_only_under_seven_profiles() {
        let created = seed_preferences(None, 5).unwrap().unwrap();
        let settled = seed_preferences(Some(&created), 99).unwrap().unwrap();
        assert!(seed_preferences(Some(&settled), 100).unwrap().is_none());
        let parsed: serde_json::Value = serde_json::from_str(&settled).unwrap();
        assert_eq!(parsed["tab_freezing_enabled"], serde_json::Value::Bool(false));
        assert_eq!(
            parsed["performance_tuning"]["high_efficiency_mode"]["state"],
            serde_json::Value::Number(serde_json::Number::from(0u64))
        );
        assert_eq!(parsed["profile"]["exit_type"], "Normal");
        assert_eq!(
            parsed["profile"]["exited_cleanly"],
            serde_json::Value::Bool(true)
        );
        let stamp = parsed["performance_tuning"]["tab_discarding"]["exceptions_with_time"]
            [SEVEN_SITE]
            .as_str()
            .unwrap();
        assert_eq!(stamp, chrome_time(5));
        assert!(seed_preferences(Some("[]"), 1).is_err());
        assert!(seed_preferences(Some("{"), 1).is_err());

        let host = env("macos");
        let dir = chrome_dir(&host);
        let prefs = format!("{dir}/Default/Preferences");
        let mut fs = MemFs::new();
        fs.0.borrow_mut().files.insert(prefs.clone(), "[]".to_string());
        let err = run(
            &open_request(None, false, None),
            &host,
            0,
            &mut fs,
            &mut absent,
            &mut |_| panic!("spawned"),
        )
        .unwrap_err();
        assert!(err.contains("not a JSON object, left alone"), "{err}");
        assert_eq!(fs.0.borrow().files.get(&prefs).map(String::as_str), Some("[]"));
        assert!(fs.0.borrow().removed.is_empty());

        fs.0.borrow_mut().files.insert(prefs.clone(), settled);
        let mut spawned = false;
        run(
            &open_request(None, false, None),
            &host,
            0,
            &mut fs,
            &mut absent,
            &mut |_| {
                spawned = true;
                Ok(())
            },
        )
        .unwrap();
        assert!(spawned);
        assert!(fs.0.borrow().writes.is_empty());
        assert_eq!(fs.0.borrow().removed.len(), 4);

        let mut outside = MemFs::new();
        run(
            &open_request(None, false, Some("/tmp/other")),
            &host,
            0,
            &mut outside,
            &mut absent,
            &mut |_| Ok(()),
        )
        .unwrap();
        assert!(outside.0.borrow().writes.is_empty());

        let inside = "/Users/me/Library/Application Support/slicc/seven-profiles/chrome";
        let mut explicit = MemFs::new();
        run(
            &open_request(None, false, Some(inside)),
            &host,
            0,
            &mut explicit,
            &mut absent,
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(explicit.0.borrow().writes.len(), 1);
        assert_no_six(&explicit.0.borrow().reads);
    }

    #[test]
    fn a_live_owner_blocks_launch_and_keeps_the_locks() {
        let host = env("linux");
        let dir = chrome_dir(&host);
        let mut fs = MemFs::new();
        fs.0
            .borrow_mut()
            .links
            .insert(format!("{dir}/SingletonLock"), "host-77".to_string());
        fs.0.borrow_mut().alive.push(77);
        let err = run(
            &open_request(None, false, None),
            &host,
            0,
            &mut fs,
            &mut absent,
            &mut |_| panic!("spawned"),
        )
        .unwrap_err();
        assert_eq!(
            err,
            format!("playwright-cli open: profile is in use: {dir} is held by process 77\n")
        );
        assert!(fs.0.borrow().removed.is_empty());
        assert!(fs.0.borrow().writes.is_empty());
    }

    #[test]
    fn six_profiles_are_refused_without_reading_them() {
        for path in [
            "/Users/me/Library/Application Support/slicc/profiles/chrome",
            "/Users/me/Library/Application Support/Slicc/profiles/chrome",
            "/Users/me/Library/Application Support/SLICC/Profiles/Default",
            "/home/me/.config/slicc/profiles/browser-coding-agent-chrome",
            "/tmp/browser-coding-agent-chrome-extra",
            "C:\\Users\\me\\AppData\\Roaming\\slicc\\profiles\\chrome",
            "C:\\Users\\me\\AppData\\Roaming\\Slicc\\profiles\\chrome",
        ] {
            let mut fs = MemFs::new();
            let err = run(
                &open_request(None, false, Some(path)),
                &env("linux"),
                0,
                &mut fs,
                &mut |_| panic!("probed"),
                &mut |_| panic!("spawned"),
            )
            .unwrap_err();
            assert!(err.contains("refusing profile"), "{err}");
            assert!(err.contains(path), "{err}");
            assert!(fs.0.borrow().reads.is_empty(), "{path}");
        }
    }

    #[test]
    fn claude_and_1password_are_refused_before_a_probe() {
        let (mut launcher, _) = recorder();
        let mut probes = 0;
        for (name, label) in [("claude", "Claude"), ("1Password", "1Password")] {
            let err = decide(
                "open",
                Some(name),
                false,
                None,
                &mut || {
                    probes += 1;
                    Ok(())
                },
                &mut launcher,
            )
            .unwrap_err();
            assert_eq!(
                err,
                format!("playwright-cli open: {label} blocks remote debugging\n")
            );
        }
        assert_eq!(probes, 0);
        assert_eq!(launcher.calls, 0);
    }

    #[test]
    fn headed_chromium_uses_open_without_waiting() {
        let host = env("macos");
        let request = open_request(None, true, None);
        let plan = plan_launch(&request, &host, 9333, &installed, &no_mdfind, &no_plist).unwrap();
        assert_eq!(plan.program, "/usr/bin/open");
        assert!(!plan.args.iter().any(|arg| arg == "-W"));
        assert!(!plan.args.iter().any(|arg| arg.contains("headless")));
        assert!(plan.via_open);
        let headless = plan_launch(
            &open_request(None, false, None),
            &host,
            9333,
            &installed,
            &no_mdfind,
            &no_plist,
        )
        .unwrap();
        assert_eq!(
            headless.program,
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        );
        assert!(headless.args.iter().any(|arg| arg == "--headless=new"));
        assert!(!headless.via_open);
    }

    #[test]
    fn a_capital_s_seven_profiles_path_is_not_refused() {
        let path = "/Users/me/Library/Application Support/Slicc/seven-profiles/chrome";
        let plan = plan_launch(
            &open_request(None, false, Some(path)),
            &env("macos"),
            0,
            &installed,
            &no_mdfind,
            &no_plist,
        )
        .unwrap();
        assert_eq!(plan.user_data_dir, path);
    }

    fn version_body(port: u16, path: &str) -> String {
        format!(r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}{path}"}}"#)
    }

    fn assert_high_port(url: &str) {
        let port: u16 = url
            .trim_start_matches("http://127.0.0.1:")
            .trim_end_matches("/json/version")
            .parse()
            .unwrap();
        assert!(port >= 9333, "{url}");
        assert!(!(9222..=9229).contains(&port), "{url}");
    }

    struct ClockFs {
        dirs: BTreeMap<String, Vec<String>>,
        files: BTreeMap<String, (i64, String)>,
        lists: RefCell<Vec<String>>,
        reads: RefCell<Vec<String>>,
    }

    impl ClockFs {
        fn new() -> Self {
            Self {
                dirs: BTreeMap::new(),
                files: BTreeMap::new(),
                lists: RefCell::new(Vec::new()),
                reads: RefCell::new(Vec::new()),
            }
        }

        fn dir(&mut self, path: &str, names: &[&str]) {
            self.dirs.insert(
                path.to_string(),
                names.iter().map(|name| (*name).to_string()).collect(),
            );
        }

        fn file(&mut self, path: &str, modified: i64, body: &str) {
            self.files
                .insert(path.to_string(), (modified, body.to_string()));
        }
    }

    impl ScanFs for ClockFs {
        fn list_dirs(&self, path: &str) -> Vec<String> {
            self.lists.borrow_mut().push(path.to_string());
            self.dirs.get(path).cloned().unwrap_or_default()
        }

        fn read_file(&self, path: &str) -> Option<String> {
            self.reads.borrow_mut().push(path.to_string());
            self.files.get(path).map(|(_, body)| body.clone())
        }

        fn modified(&self, path: &str) -> Option<i64> {
            self.files.get(path).map(|(modified, _)| *modified)
        }
    }

    fn port_file(dir: &str, windows: bool) -> String {
        child_path(dir, ACTIVE_PORT_FILE, windows)
    }

    #[test]
    fn the_newest_live_seven_profile_port_is_used() {
        let host = env("macos");
        let root = config_seven_root(&host);
        assert_eq!(
            root,
            "/Users/me/Library/Application Support/slicc/seven-profiles"
        );
        let six = "/Users/me/Library/Application Support/Slicc/profiles";
        let mut fs = ClockFs::new();
        fs.dir(
            &root,
            &[
                ".migrated-from-profiles",
                ".hidden",
                "browser-coding-agent-chrome",
                "chrome",
                "chrome..old",
                "edge",
                "has space",
            ],
        );
        fs.dir(six, &["chrome"]);
        fs.dir(
            "/Users/me/Library/Application Support/Slicc",
            &["profiles", "seven-profiles"],
        );
        fs.file(
            &port_file(&format!("{root}/chrome"), false),
            1,
            "9333\n/devtools/browser/old\n",
        );
        fs.file(
            &port_file(&format!("{root}/edge"), false),
            5,
            "9444\n/devtools/browser/new\n",
        );
        fs.file(
            &port_file(&format!("{root}/browser-coding-agent-chrome"), false),
            9,
            "9555\n/devtools/browser/agent\n",
        );
        fs.file(
            &port_file(&format!("{root}/.migrated-from-profiles"), false),
            50,
            "9667\n/devtools/browser/marker\n",
        );
        fs.file(
            &port_file(&format!("{root}/.hidden"), false),
            49,
            "9668\n/devtools/browser/hidden\n",
        );
        fs.file(
            &port_file(&format!("{root}/chrome..old"), false),
            48,
            "9669\n/devtools/browser/dots\n",
        );
        fs.file(
            &port_file(&format!("{root}/has space"), false),
            47,
            "9670\n/devtools/browser/space\n",
        );
        fs.file(
            &port_file(&format!("{six}/chrome"), false),
            99,
            "9666\n/devtools/browser/six\n",
        );
        let mut seen = Vec::new();
        let found = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            assert_eq!(url, "http://127.0.0.1:9444/json/version");
            Ok(version_body(9444, "/devtools/browser/new"))
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9444/json/version"));
        assert_eq!(seen, vec!["http://127.0.0.1:9444/json/version".to_string()]);
        let snap = format!("{}/snap", host.home);
        assert_eq!(fs.lists.borrow().as_slice(), [root.as_str(), snap.as_str()]);
        assert!(fs.reads.borrow().iter().any(|path| path.contains("/edge/")));
        assert!(fs.reads.borrow().iter().any(|path| path.contains("/chrome/")));
        assert_no_six(&fs.reads.borrow());
        assert_no_six(&fs.lists.borrow());
        assert!(fs.reads.borrow().iter().all(|path| {
            !path.contains("/.migrated-from-profiles/")
                && !path.contains("/.hidden/")
                && !path.contains("chrome..old")
                && !path.contains("has space")
        }));
    }

    #[test]
    fn a_newer_forbidden_port_is_not_probed() {
        let host = env("macos");
        let root = config_seven_root(&host);
        let mut fs = ClockFs::new();
        fs.dir(&root, &["brave", "brave-live", "chrome", "edge"]);
        fs.file(
            &port_file(&format!("{root}/chrome"), false),
            10,
            "9222\n/devtools/browser/default\n",
        );
        fs.file(
            &port_file(&format!("{root}/edge"), false),
            9,
            "9223\n/devtools/browser/nope\n",
        );
        fs.file(
            &port_file(&format!("{root}/brave"), false),
            8,
            "9228\n/devtools/browser/nope\n",
        );
        fs.file(
            &port_file(&format!("{root}/brave-live"), false),
            1,
            "9333\n/devtools/browser/old\n",
        );
        let mut seen = Vec::new();
        let found = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            Ok(version_body(9333, "/devtools/browser/old"))
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9333/json/version"));
        assert_eq!(seen, vec!["http://127.0.0.1:9333/json/version".to_string()]);
        assert!(seen.iter().all(|url| !url.contains("9222")));
        assert!(seen.iter().all(|url| !url.contains("9223")));
        assert!(seen.iter().all(|url| !url.contains("9228")));
    }

    #[test]
    fn a_mismatched_websocket_path_uses_the_older_live_file() {
        let host = env("macos");
        let root = config_seven_root(&host);
        let mut fs = ClockFs::new();
        fs.dir(&root, &["chrome", "edge"]);
        fs.file(
            &port_file(&format!("{root}/chrome"), false),
            1,
            "9333\n/devtools/browser/old\n",
        );
        fs.file(
            &port_file(&format!("{root}/edge"), false),
            5,
            "9444\n/devtools/browser/new\n",
        );
        let mut seen = Vec::new();
        let found = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            if url.contains(":9444/") {
                Ok(version_body(9444, "/devtools/browser/other"))
            } else {
                Ok(version_body(9333, "/devtools/browser/old"))
            }
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9333/json/version"));
        assert_eq!(
            seen,
            vec![
                "http://127.0.0.1:9444/json/version".to_string(),
                "http://127.0.0.1:9333/json/version".to_string(),
            ]
        );
    }

    #[test]
    fn a_dead_newer_port_uses_the_older_live_file() {
        let host = env("macos");
        let root = config_seven_root(&host);
        let mut fs = ClockFs::new();
        fs.dir(&root, &["chrome", "edge"]);
        fs.file(
            &port_file(&format!("{root}/chrome"), false),
            1,
            "9333\n/devtools/browser/old\n",
        );
        fs.file(
            &port_file(&format!("{root}/edge"), false),
            5,
            "9444\n/devtools/browser/new\n",
        );
        let mut seen = Vec::new();
        let found = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            if url.contains(":9444/") {
                Err("connection refused\n".to_string())
            } else {
                Ok(version_body(9333, "/devtools/browser/old"))
            }
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9333/json/version"));
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn an_http_error_from_the_newest_port_stops_the_scan() {
        let host = env("macos");
        let root = config_seven_root(&host);
        let mut fs = ClockFs::new();
        fs.dir(&root, &["chrome", "edge"]);
        fs.file(
            &port_file(&format!("{root}/chrome"), false),
            1,
            "9333\n/devtools/browser/old\n",
        );
        fs.file(
            &port_file(&format!("{root}/edge"), false),
            5,
            "9444\n/devtools/browser/new\n",
        );
        let mut seen = Vec::new();
        let err = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            Err("HTTP 500\nno\n".to_string())
        })
        .unwrap_err();
        assert_eq!(err, "HTTP 500\nno\n");
        assert_eq!(seen, vec!["http://127.0.0.1:9444/json/version".to_string()]);
    }

    #[test]
    fn an_equal_clock_tries_the_earlier_path_first() {
        let host = env("macos");
        let root = config_seven_root(&host);
        let mut fs = ClockFs::new();
        fs.dir(&root, &["chrome", "edge"]);
        fs.file(
            &port_file(&format!("{root}/chrome"), false),
            4,
            "9333\n/devtools/browser/old\n",
        );
        fs.file(
            &port_file(&format!("{root}/edge"), false),
            4,
            "9444\n/devtools/browser/new\n",
        );
        let mut seen = Vec::new();
        let found = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            if url.contains(":9333/") {
                Ok(version_body(9333, "/devtools/browser/other"))
            } else {
                Ok(version_body(9444, "/devtools/browser/new"))
            }
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9444/json/version"));
        assert_eq!(
            seen,
            vec![
                "http://127.0.0.1:9333/json/version".to_string(),
                "http://127.0.0.1:9444/json/version".to_string(),
            ]
        );
    }

    #[test]
    fn a_newer_snap_seven_profile_wins_without_reading_profiles() {
        let host = env("linux");
        let config = config_seven_root(&host);
        assert_eq!(config, "/home/me/.config/slicc/seven-profiles");
        let snap = format!("{}/snap", host.home);
        let snap_root = format!("{snap}/chromium/common/slicc/seven-profiles");
        let snap_six = format!("{snap}/chromium/common/slicc/profiles");
        let config_six = "/home/me/.config/slicc/profiles";
        let mut fs = ClockFs::new();
        fs.dir(&config, &["chrome"]);
        fs.dir(&snap, &["chromium"]);
        fs.dir(&snap_root, &["chromium"]);
        fs.dir(&snap_six, &["chromium"]);
        fs.dir(config_six, &["chrome"]);
        fs.dir("/home/me/.config/slicc", &["profiles", "seven-profiles"]);
        fs.dir(
            &format!("{snap}/chromium/common/slicc"),
            &["profiles", "seven-profiles"],
        );
        fs.file(
            &port_file(&format!("{config}/chrome"), false),
            1,
            "9333\n/devtools/browser/old\n",
        );
        fs.file(
            &port_file(&format!("{snap_root}/chromium"), false),
            9,
            "9555\n/devtools/browser/snap\n",
        );
        fs.file(
            &port_file(&format!("{snap_six}/chromium"), false),
            100,
            "9666\n/devtools/browser/six\n",
        );
        fs.file(
            &port_file(&format!("{config_six}/chrome"), false),
            100,
            "9777\n/devtools/browser/six\n",
        );
        let mut seen = Vec::new();
        let found = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            assert_eq!(url, "http://127.0.0.1:9555/json/version");
            Ok(version_body(9555, "/devtools/browser/snap"))
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9555/json/version"));
        assert_eq!(
            fs.lists.borrow().as_slice(),
            [config.as_str(), snap.as_str(), snap_root.as_str()]
        );
        assert_no_six(&fs.reads.borrow());
        assert_no_six(&fs.lists.borrow());
        assert!(fs
            .reads
            .borrow()
            .iter()
            .any(|path| path.contains("/seven-profiles/chromium/")));
    }

    #[test]
    fn a_windows_seven_profile_port_is_found_without_profiles() {
        let mut host = env("windows");
        host.app_data = Some("C:\\Users\\me\\AppData\\Roaming".to_string());
        let root = config_seven_root(&host);
        assert_eq!(
            root,
            "C:\\Users\\me\\AppData\\Roaming\\slicc\\seven-profiles"
        );
        let six = "C:\\Users\\me\\AppData\\Roaming\\Slicc\\profiles";
        let mut fs = ClockFs::new();
        fs.dir(&root, &["chrome"]);
        fs.dir(six, &["chrome"]);
        let dir = join_parts(true, &[root.clone(), "chrome".to_string()]);
        fs.file(
            &port_file(&dir, true),
            3,
            "9333\n/devtools/browser/win\n",
        );
        let six_dir = join_parts(true, &[six.to_string(), "chrome".to_string()]);
        fs.file(
            &port_file(&six_dir, true),
            9,
            "9444\n/devtools/browser/six\n",
        );
        let mut seen = Vec::new();
        let found = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            Ok(version_body(9333, "/devtools/browser/win"))
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9333/json/version"));
        assert_eq!(seen, vec!["http://127.0.0.1:9333/json/version".to_string()]);
        let snap = join_parts(true, &[host.home.clone(), "snap".to_string()]);
        assert_eq!(fs.lists.borrow().as_slice(), [root.as_str(), snap.as_str()]);
        assert_no_six(&fs.reads.borrow());
        assert_no_six(&fs.lists.borrow());
    }

    #[test]
    fn explicit_endpoints_win_and_tests_do_not_scan() {
        assert_eq!(attached_port().unwrap(), None);
        let active = "http://127.0.0.1:9444/json/version".to_string();
        assert_eq!(
            start_after_explicit(Some("http://127.0.0.1:9333"), None, Some(active.clone()))
                .unwrap(),
            crate::connect::Start::Discover("http://127.0.0.1:9333/json/version".to_string())
        );
        assert_eq!(
            start_after_explicit(None, Some("http://127.0.0.1:9334"), Some(active.clone()))
                .unwrap(),
            crate::connect::Start::Discover("http://127.0.0.1:9334/json/version".to_string())
        );
        assert_eq!(
            start_after_explicit(None, None, Some(active.clone())).unwrap(),
            crate::connect::Start::Discover(active)
        );
        #[cfg(not(target_arch = "wasm32"))]
        {
            let err = start_after_explicit(None, None, None).unwrap_err();
            assert_eq!(err, "playwright-cli: no debugger is connected\n");
            assert!(!err.contains("9222"));
        }
        #[cfg(target_arch = "wasm32")]
        {
            assert_eq!(
                start_after_explicit(None, None, None).unwrap(),
                crate::connect::Start::Discover(
                    "http://127.0.0.1:9222/json/version".to_string()
                )
            );
        }
    }
}
