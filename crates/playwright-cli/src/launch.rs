#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::collections::BTreeMap;

const DEFAULT_DISCOVERY: &str = "http://127.0.0.1:9222/json/version";

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnPlan {
    pub program: String,
    pub args: Vec<String>,
    pub port: u16,
    pub user_data_dir: String,
}

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
            discover: DEFAULT_DISCOVERY.to_string(),
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
    let mut launcher = HostLauncher;
    decide(
        command,
        browser,
        headed,
        profile,
        &mut probe_default,
        &mut launcher,
    )
}

struct HostLauncher;

impl Launcher for HostLauncher {
    fn launch(&mut self, request: &LaunchRequest) -> Result<Started, String> {
        launch_real(request)
    }
}

fn probe_default() -> Result<(), String> {
    #[cfg(test)]
    {
        return Err("no debugger is connected\n".to_string());
    }
    #[cfg(not(test))]
    {
        match crate::net::http_get(DEFAULT_DISCOVERY) {
            Ok(_) => Ok(()),
            Err(message) if message.starts_with("HTTP ") => Err(message),
            Err(_) => Err("no debugger is connected\n".to_string()),
        }
    }
}

fn launch_real(request: &LaunchRequest) -> Result<Started, String> {
    #[cfg(test)]
    {
        let _ = request;
        return Err("playwright-cli: browser launch is not available in tests\n".to_string());
    }
    #[cfg(all(not(test), target_arch = "wasm32"))]
    {
        return Err(format!(
            "playwright-cli {}: launching a browser is host-only\n",
            request.command
        ));
    }
    #[cfg(all(not(test), not(target_arch = "wasm32")))]
    {
        launch_host(request)
    }
}

pub fn plan_launch(
    request: &LaunchRequest,
    env: &HostEnv,
    port: u16,
    exists: &dyn Fn(&str) -> bool,
    mdfind: &dyn Fn(&str) -> Vec<String>,
    read_plist: &dyn Fn(&str) -> Option<String>,
) -> Result<SpawnPlan, String> {
    let command = request.command;
    if !port_allowed(port) {
        return Err(format!(
            "playwright-cli {command}: refusing debugger port {port}\n"
        ));
    }
    let (program, kind) = if let Some(name) = request.browser {
        resolve_named(command, name, env, exists, mdfind, read_plist)?
    } else {
        resolve_default(command, env, exists, mdfind, read_plist)?
    };
    let user_data_dir = user_data_dir(command, request.profile, env)?;
    let args = build_args(kind, request.headed, port, &user_data_dir);
    Ok(SpawnPlan {
        program,
        args,
        port,
        user_data_dir,
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

fn user_data_dir(command: &str, profile: Option<&str>, env: &HostEnv) -> Result<String, String> {
    if let Some(path) = profile {
        let path = path.trim();
        if path.is_empty() {
            return Err(format!(
                "playwright-cli {command}: --profile requires a path\n"
            ));
        }
        return Ok(path.to_string());
    }
    let temp = env.temp_dir.trim_end_matches(['/', '\\']);
    let name = format!("playwright-cli-{}", env.unique);
    if temp.contains('\\') {
        Ok(format!("{temp}\\{name}"))
    } else {
        Ok(format!("{temp}/{name}"))
    }
}

fn build_args(kind: Kind, headed: bool, port: u16, user_data_dir: &str) -> Vec<String> {
    let mut args = vec![
        format!("--remote-debugging-port={port}"),
        format!("--user-data-dir={user_data_dir}"),
    ];
    if kind == Kind::Chromium {
        args.push("--no-first-run".to_string());
        args.push("--no-default-browser-check".to_string());
        if !headed {
            args.push("--headless=new".to_string());
        }
    }
    args
}

fn not_found(command: &str, name: &str) -> String {
    format!("playwright-cli {command}: {name} was not found\n")
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn launch_host(request: &LaunchRequest) -> Result<Started, String> {
    let port = choose_port(bind_localhost)?;
    let env = current_env();
    let plan = plan_launch(
        request,
        &env,
        port,
        &file_exists,
        &spotlight,
        &read_plist_file,
    )?;
    if let Err(err) = std::fs::create_dir_all(&plan.user_data_dir) {
        return Err(format!(
            "playwright-cli {}: could not create the browser profile: {err}\n",
            request.command
        ));
    }
    let mut child = match spawn_child(&plan) {
        Ok(child) => child,
        Err(message) => {
            if request.profile.is_none() {
                let _ = std::fs::remove_dir_all(&plan.user_data_dir);
            }
            return Err(message);
        }
    };
    if let Err(message) = wait_ready(port, &mut child) {
        kill_child(&mut child);
        if request.profile.is_none() {
            let _ = std::fs::remove_dir_all(&plan.user_data_dir);
        }
        return Err(message);
    }
    let version_url = version_url(port);
    let owned = request.profile.is_none();
    let dir = plan.user_data_dir.clone();
    Ok(Started::running(
        version_url,
        Box::new(move || {
            kill_child(&mut child);
            if owned {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }),
    ))
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
    }
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

#[cfg(all(not(test), not(target_arch = "wasm32"), windows))]
fn kill_child(child: &mut std::process::Child) {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &child.id().to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(not(test), not(target_arch = "wasm32"), unix))]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
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
        assert_eq!(
            plan.args,
            vec![
                "--remote-debugging-port=9333".to_string(),
                "--user-data-dir=/tmp/playwright-cli-test".to_string(),
                "--no-first-run".to_string(),
                "--no-default-browser-check".to_string(),
                "--headless=new".to_string(),
            ]
        );
        assert!(!plan.args.iter().any(|arg| arg.contains("9222")));
        assert!(!plan.args.iter().any(|arg| arg == "--headless"));

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
        assert_eq!(
            plan.program,
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        );
        assert_eq!(plan.user_data_dir, "/tmp/my-profile");
        assert!(!plan.args.iter().any(|arg| arg.contains("headless")));
        assert!(plan.args.contains(&"--no-first-run".to_string()));
        assert!(plan
            .args
            .contains(&"--user-data-dir=/tmp/my-profile".to_string()));
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
        assert!(!plan.args.iter().any(|arg| arg.contains("headless")));
        assert!(plan.args.iter().any(|arg| arg == "--no-first-run"));
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
    }

    #[test]
    fn a_forbidden_port_is_refused_before_launch() {
        let host = env("linux");
        let request = LaunchRequest {
            command: "open",
            browser: Some("chrome"),
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
}
