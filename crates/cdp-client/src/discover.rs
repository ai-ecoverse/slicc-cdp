use serde_json::Value;
use std::time::Duration;

pub const CLI_TIMEOUT: Duration = Duration::from_millis(1500);
pub const CLI_MAX_STDOUT: usize = 1_000_000;
pub const POSITIVE_CACHE_TTL: Duration = Duration::from_secs(5);
pub const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(30);
const ACTIVE_PORT_FILE: &str = "DevToolsActivePort";
const CACHE_FILE: &str = "browsers-discovery.json";
const CACHE_SCHEMA: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEnv {
    pub platform: &'static str,
    pub home: String,
    pub app_data: Option<String>,
    pub local_app_data: Option<String>,
    pub xdg_config_home: Option<String>,
    pub xdg_state_home: Option<String>,
}

pub trait ScanFs {
    fn list_dirs(&self, path: &str) -> Vec<String>;
    fn read_file(&self, path: &str) -> Option<String>;
    fn modified(&self, path: &str) -> Option<i64>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PortCandidate {
    modified: i64,
    path: String,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliCandidate {
    url: String,
    port: u16,
    seven_open: bool,
    slicc_profile: bool,
    tier: i64,
}

pub fn accept_loopback_ws(url: &str, expected_port: u16) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "ws" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    if !loopback_host(host) {
        return false;
    }
    let port = parsed.port().unwrap_or(80);
    port == expected_port
}

pub fn is_slicc_user_data_dir(dir: &str) -> bool {
    if dir.is_empty() {
        return false;
    }
    let normalized = dir.replace('\\', "/");
    normalized.contains("/slicc/seven-profiles/")
        || normalized.contains("/slicc/profiles/")
        || normalized.contains("/Slicc/profiles/")
        || normalized.ends_with("/slicc/seven-profiles")
        || normalized.ends_with("/slicc/profiles")
        || normalized.ends_with("/Slicc/profiles")
}

pub fn slicc_profile_roots(env: &HostEnv) -> Vec<String> {
    let windows = env.platform == "windows";
    let config = config_dir(env);
    let mut roots = vec![
        join_parts(windows, &[config.clone(), "slicc".to_string(), "seven-profiles".to_string()]),
        join_parts(windows, &[config, "slicc".to_string(), "profiles".to_string()]),
        six_profiles_root(env),
    ];
    let snap = join_parts(windows, &[env.home.clone(), "snap".to_string()]);
    roots.push(snap);
    roots
}

pub fn pick_cli_endpoint(entries: &[Value]) -> Option<String> {
    let mut candidates = endpoint_candidates(entries);
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|left, right| {
        right
            .seven_open
            .cmp(&left.seven_open)
            .then_with(|| right.slicc_profile.cmp(&left.slicc_profile))
            .then_with(|| left.tier.cmp(&right.tier))
    });
    let best = &candidates[0];
    Some(version_url(best.port))
}

pub fn parse_browsers_json(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    let object = value.as_object()?;
    if object.get("schema").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let entries = object.get("entries")?.as_array()?;
    pick_cli_endpoint(entries)
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

pub fn version_accepts(body: &str, expected_path: &str, expected_port: u16) -> bool {
    let Ok(url) = crate::connect::websocket_url_from_version(body) else {
        return false;
    };
    if !accept_loopback_ws(&url, expected_port) {
        return false;
    }
    websocket_pathname(&url) == Some(expected_path)
}

pub fn version_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/json/version")
}

pub fn from_devtools_active_port<F: ScanFs>(
    env: &HostEnv,
    fs: &F,
    roots: &[String],
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    let mut found = collect_port_files(env, fs, roots);
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

pub fn resolve_attached<F: ScanFs>(
    env: &HostEnv,
    fs: &F,
    roots: &[String],
    slicc_json: Option<&str>,
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    if let Some(text) = slicc_json {
        if let Some(url) = parse_browsers_json(text) {
            return Ok(Some(url));
        }
    }
    from_devtools_active_port(env, fs, roots, probe)
}

#[cfg(any(target_arch = "wasm32", target_os = "wasi"))]
pub fn attached(
    _probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    Ok(None)
}

pub fn accept_cached_version_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "http" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    if !loopback_host(host) {
        return false;
    }
    if parsed.path() != "/json/version" {
        return false;
    }
    parsed.port().is_some()
}

pub fn browsers_discovery_cache_path(env: &HostEnv) -> String {
    let windows = env.platform == "windows";
    join_parts(
        windows,
        &[
            config_dir(env),
            "slicc".to_string(),
            "playwright-cli".to_string(),
            CACHE_FILE.to_string(),
        ],
    )
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
pub fn attached(
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    let env = current_env();
    let cache = browsers_discovery_cache_path(&env);
    attached_with(None, Some(std::path::Path::new(&cache)), CLI_TIMEOUT, CLI_MAX_STDOUT, probe)
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
pub fn attached_with(
    program: Option<&std::path::Path>,
    cache_path: Option<&std::path::Path>,
    timeout: Duration,
    max_stdout: usize,
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    if let Some(path) = cache_path {
        match read_browsers_cache(path, std::time::SystemTime::now()) {
            BrowsersCache::Positive(url) if accept_cached_version_url(&url) => {
                if cached_endpoint_live(&url, probe) {
                    return Ok(Some(url));
                }
                let _ = std::fs::remove_file(path);
            }
            BrowsersCache::Negative => {
                let env = current_env();
                let roots = slicc_profile_roots(&env);
                return from_devtools_active_port(&env, &DiskFs, &roots, probe);
            }
            BrowsersCache::Positive(_) | BrowsersCache::Miss => {}
        }
    }
    match run_slicc_browsers(program, timeout, max_stdout) {
        Some(url) => {
            if let Some(path) = cache_path {
                if accept_cached_version_url(&url) {
                    let _ = write_browsers_cache(path, Some(&url));
                }
            }
            return Ok(Some(url));
        }
        None => {
            if let Some(path) = cache_path {
                let _ = write_browsers_cache(path, None);
            }
        }
    }
    let env = current_env();
    let roots = slicc_profile_roots(&env);
    from_devtools_active_port(&env, &DiskFs, &roots, probe)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowsersCache {
    Miss,
    Positive(String),
    Negative,
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
pub fn read_browsers_cache(
    path: &std::path::Path,
    now: std::time::SystemTime,
) -> BrowsersCache {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(_) => return BrowsersCache::Miss,
    };
    if !cache_file_trusted(&meta, now) {
        return BrowsersCache::Miss;
    }
    let modified = match meta.modified() {
        Ok(modified) => modified,
        Err(_) => return BrowsersCache::Miss,
    };
    if modified > now {
        return BrowsersCache::Miss;
    }
    let age = match now.duration_since(modified) {
        Ok(age) => age,
        Err(_) => return BrowsersCache::Miss,
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return BrowsersCache::Miss,
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return BrowsersCache::Miss,
    };
    let object = match value.as_object() {
        Some(object) => object,
        None => return BrowsersCache::Miss,
    };
    if object.get("schema").and_then(Value::as_u64) != Some(CACHE_SCHEMA) {
        return BrowsersCache::Miss;
    }
    match object.get("url") {
        Some(Value::Null) => {
            if age > NEGATIVE_CACHE_TTL {
                return BrowsersCache::Miss;
            }
            BrowsersCache::Negative
        }
        Some(Value::String(url)) => {
            if age > POSITIVE_CACHE_TTL {
                return BrowsersCache::Miss;
            }
            if !accept_cached_version_url(url) {
                return BrowsersCache::Miss;
            }
            BrowsersCache::Positive(url.clone())
        }
        _ => BrowsersCache::Miss,
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
pub fn write_browsers_cache(path: &std::path::Path, url: Option<&str>) -> Result<(), String> {
    if let Some(url) = url {
        if !accept_cached_version_url(url) {
            return Err("cache: refusing non-loopback version URL\n".to_string());
        }
    }
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|err| format!("cache: {err}\n"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::Permissions::from_mode(0o700);
            let _ = std::fs::set_permissions(dir, mode.clone());
            if let Some(parent) = dir.parent() {
                let _ = std::fs::set_permissions(parent, mode);
            }
        }
    }
    let body = match url {
        Some(url) => format!(
            "{{\"schema\":{CACHE_SCHEMA},\"url\":{}}}",
            serde_json::to_string(url).unwrap_or_else(|_| "null".to_string())
        ),
        None => format!("{{\"schema\":{CACHE_SCHEMA},\"url\":null}}"),
    };
    let staged = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    write_private_file(&staged, &body)?;
    match std::fs::rename(&staged, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&staged);
            Err(format!("cache: {err}\n"))
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
fn write_private_file(path: &std::path::Path, body: &str) -> Result<(), String> {
    use std::io::Write;
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|err| format!("cache: {err}\n"))?;
        let written = file
            .write_all(body.as_bytes())
            .map_err(|err| format!("cache: {err}\n"));
        drop(file);
        if written.is_err() {
            let _ = std::fs::remove_file(path);
        }
        written
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, body).map_err(|err| format!("cache: {err}\n"))
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
fn cache_file_trusted(meta: &std::fs::Metadata, now: std::time::SystemTime) -> bool {
    let Ok(modified) = meta.modified() else {
        return false;
    };
    if modified > now {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return false;
        }
        let uid = unsafe { libc::geteuid() };
        if meta.uid() != uid {
            return false;
        }
    }
    true
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
fn cached_endpoint_live(
    url: &str,
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> bool {
    let Ok(body) = probe(url) else {
        return false;
    };
    let Ok(ws) = crate::connect::websocket_url_from_version(&body) else {
        return false;
    };
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(port) = parsed.port() else {
        return false;
    };
    accept_loopback_ws(&ws, port)
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
pub fn run_slicc_browsers(
    program: Option<&std::path::Path>,
    timeout: Duration,
    max_stdout: usize,
) -> Option<String> {
    use std::io::Read;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;

    fn kill_and_reap(child: &mut Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    fn wait_deadline(child: &mut Child, deadline: Instant) -> Option<std::process::ExitStatus> {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() >= deadline => {
                    kill_and_reap(child);
                    return None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => {
                    kill_and_reap(child);
                    return None;
                }
            }
        }
    }

    let mut command = match program {
        Some(path) => Command::new(path),
        None => Command::new("slicc"),
    };
    command
        .args(["browsers", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return None,
    };
    let deadline = Instant::now() + timeout;
    let mut stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            kill_and_reap(&mut child);
            return None;
        }
    };
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, ()>>();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => {
                    let _ = tx.send(Ok(buf));
                    return;
                }
                Ok(n) => {
                    if buf.len() + n > max_stdout {
                        let _ = tx.send(Err(()));
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                Err(_) => {
                    let _ = tx.send(Err(()));
                    return;
                }
            }
        }
    });
    let remaining = deadline.saturating_duration_since(Instant::now());
    let buf = match rx.recv_timeout(remaining) {
        Ok(Ok(buf)) => buf,
        Ok(Err(())) | Err(_) => {
            kill_and_reap(&mut child);
            return None;
        }
    };
    let status = wait_deadline(&mut child, deadline)?;
    if !status.success() || buf.len() > max_stdout {
        return None;
    }
    let text = String::from_utf8_lossy(&buf);
    parse_browsers_json(&text)
}

fn endpoint_candidates(entries: &[Value]) -> Vec<CliCandidate> {
    let mut out = Vec::new();
    for entry in entries {
        let Some(object) = entry.as_object() else {
            continue;
        };
        if object.get("kind").and_then(Value::as_str) != Some("endpoint") {
            continue;
        }
        let Some(endpoint) = object.get("endpoint").and_then(Value::as_object) else {
            continue;
        };
        let Some(ws) = endpoint.get("webSocketDebuggerUrl").and_then(Value::as_str) else {
            continue;
        };
        let Some(port) = port_from_cdp(endpoint.get("cdp").and_then(Value::as_str)) else {
            continue;
        };
        if !accept_loopback_ws(ws, port) {
            continue;
        }
        let tier = object
            .get("tier")
            .and_then(Value::as_i64)
            .unwrap_or(99);
        let profile = endpoint
            .get("profile")
            .and_then(Value::as_str)
            .is_some_and(|name| !name.is_empty());
        let user_data = endpoint
            .get("userDataDir")
            .and_then(Value::as_str)
            .is_some_and(is_slicc_user_data_dir);
        out.push(CliCandidate {
            url: ws.to_string(),
            port,
            seven_open: endpoint.get("sevenOpen") == Some(&Value::Bool(true)),
            slicc_profile: profile || user_data,
            tier,
        });
    }
    out
}

fn port_from_cdp(cdp: Option<&str>) -> Option<u16> {
    let cdp = cdp?;
    let parsed = url::Url::parse(cdp).ok()?;
    let host = parsed.host_str()?;
    if !loopback_host(host) {
        return None;
    }
    Some(match parsed.port() {
        Some(port) => port,
        None if parsed.scheme() == "https" => 443,
        None => 80,
    })
}

fn loopback_host(host: &str) -> bool {
    let name = host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    name == "127.0.0.1" || name == "localhost" || name == "::1"
}

fn websocket_pathname(url: &str) -> Option<&str> {
    let bare = url.split(['?', '#']).next().unwrap_or(url);
    let (_, rest) = bare.split_once("://")?;
    match rest.find('/') {
        Some(index) => Some(&rest[index..]),
        None => Some(""),
    }
}

fn live_text(
    text: &str,
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    let Some((port, path)) = parse_active_port(text) else {
        return Ok(None);
    };
    let url = version_url(port);
    match probe(&url) {
        Ok(body) if version_accepts(&body, &path, port) => Ok(Some(url)),
        Ok(_) => Ok(None),
        Err(message) if message.starts_with("HTTP ") => Err(message),
        Err(_) => Ok(None),
    }
}

fn collect_port_files<F: ScanFs>(
    env: &HostEnv,
    fs: &F,
    roots: &[String],
) -> Vec<PortCandidate> {
    let windows = env.platform == "windows";
    let mut found = Vec::new();
    for root in roots {
        if root.ends_with("snap") || root.ends_with("snap\\") || root.ends_with("snap/") {
            for name in fs.list_dirs(root) {
                if !profile_dir_name(&name) {
                    continue;
                }
                let profile_root = join_parts(
                    windows,
                    &[
                        root.clone(),
                        name,
                        "common".to_string(),
                        "slicc".to_string(),
                        "seven-profiles".to_string(),
                    ],
                );
                collect_profile_root(&profile_root, windows, fs, &mut found);
            }
            continue;
        }
        collect_profile_root(root, windows, fs, &mut found);
    }
    found
}

fn collect_profile_root<F: ScanFs>(
    root: &str,
    windows: bool,
    fs: &F,
    found: &mut Vec<PortCandidate>,
) {
    for id in fs.list_dirs(root) {
        if !profile_dir_name(&id) {
            continue;
        }
        let dir = join_parts(windows, &[root.to_string(), id]);
        let file = join_parts(windows, &[dir, ACTIVE_PORT_FILE.to_string()]);
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

fn six_profiles_root(env: &HostEnv) -> String {
    match env.platform {
        "macos" => join_parts(
            false,
            &[
                env.home.clone(),
                "Library".to_string(),
                "Application Support".to_string(),
                "Slicc".to_string(),
                "profiles".to_string(),
            ],
        ),
        "windows" => {
            let local = if let Some(local) = &env.local_app_data {
                local.clone()
            } else {
                join_parts(
                    true,
                    &[
                        env.home.clone(),
                        "AppData".to_string(),
                        "Local".to_string(),
                    ],
                )
            };
            join_parts(true, &[local, "Slicc".to_string(), "profiles".to_string()])
        }
        _ => {
            if let Some(xdg) = &env.xdg_state_home {
                if xdg.starts_with('/') {
                    return join_parts(
                        false,
                        &[xdg.clone(), "slicc".to_string(), "profiles".to_string()],
                    );
                }
            }
            join_parts(
                false,
                &[
                    env.home.clone(),
                    ".local".to_string(),
                    "state".to_string(),
                    "slicc".to_string(),
                    "profiles".to_string(),
                ],
            )
        }
    }
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

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
fn current_env() -> HostEnv {
    HostEnv {
        platform: host_platform(),
        home: home_dir(),
        app_data: env_var("APPDATA"),
        local_app_data: env_var("LOCALAPPDATA"),
        xdg_config_home: env_var("XDG_CONFIG_HOME"),
        xdg_state_home: env_var("XDG_STATE_HOME"),
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
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

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
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

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
struct DiskFs;

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
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
        let micros = modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_micros();
        i64::try_from(micros).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn env(platform: &'static str) -> HostEnv {
        HostEnv {
            platform,
            home: match platform {
                "macos" => "/Users/me".to_string(),
                "windows" => "C:\\Users\\me".to_string(),
                _ => "/home/me".to_string(),
            },
            app_data: None,
            local_app_data: None,
            xdg_config_home: None,
            xdg_state_home: None,
        }
    }

    fn endpoint(
        port: u16,
        tier: i64,
        seven_open: bool,
        user_data_dir: Option<&str>,
        host: &str,
    ) -> Value {
        let mut endpoint = json!({
            "cdp": format!("http://{host}:{port}"),
            "webSocketDebuggerUrl": format!("ws://{host}:{port}/devtools/browser/{port}"),
            "sevenOpen": seven_open,
        });
        if let Some(dir) = user_data_dir {
            endpoint
                .as_object_mut()
                .unwrap()
                .insert("userDataDir".into(), json!(dir));
        }
        json!({
            "id": format!("cdp:{host}:{port}"),
            "kind": "endpoint",
            "tier": tier,
            "endpoint": endpoint,
        })
    }

    struct MemFs {
        dirs: BTreeMap<String, Vec<String>>,
        files: BTreeMap<String, (i64, String)>,
        lists: RefCell<Vec<String>>,
        reads: RefCell<Vec<String>>,
    }

    impl MemFs {
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

    impl ScanFs for MemFs {
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

    fn version_body(port: u16, path: &str) -> String {
        format!(r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}{path}"}}"#)
    }

    fn port_file(dir: &str, windows: bool) -> String {
        join_parts(windows, &[dir.to_string(), ACTIVE_PORT_FILE.to_string()])
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

    fn serve_version(port_path: &str) -> (u16, std::thread::JoinHandle<()>, std::sync::mpsc::Sender<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = version_body(port, port_path);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
        let join = std::thread::spawn(move || {
            listener.set_nonblocking(true).unwrap();
            loop {
                if stop_rx.try_recv().is_ok() {
                    break;
                }
                match listener.accept() {
                    Ok((mut sock, _)) => {
                        let mut buf = [0u8; 1024];
                        let _ = sock.read(&mut buf);
                        let _ = sock.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        (port, join, stop_tx)
    }

    #[test]
    fn accept_loopback_ws_rejects_non_ws_non_loopback_and_port_mismatch() {
        assert!(accept_loopback_ws("ws://127.0.0.1:9333/devtools/browser/x", 9333));
        assert!(accept_loopback_ws("ws://localhost:9333/x", 9333));
        assert!(accept_loopback_ws("ws://[::1]:9333/x", 9333));
        assert!(!accept_loopback_ws("wss://127.0.0.1:9333/x", 9333));
        assert!(!accept_loopback_ws("ws://10.0.0.2:9333/x", 9333));
        assert!(!accept_loopback_ws("ws://127.0.0.1:9334/x", 9333));
    }

    #[test]
    fn slicc_browsers_prefers_seven_open_then_profile_then_lowest_tier() {
        let seven = endpoint(9401, 2, true, Some("/tmp/other"), "127.0.0.1");
        let profile = endpoint(
            9402,
            2,
            false,
            Some("/Users/me/Library/Application Support/slicc/seven-profiles/chrome"),
            "127.0.0.1",
        );
        let plain_high = endpoint(9403, 1, false, Some("/tmp/plain"), "127.0.0.1");
        let plain_low = endpoint(9404, 2, false, Some("/tmp/plain2"), "127.0.0.1");
        let remote = endpoint(9405, 1, true, Some("/tmp/remote"), "10.0.0.9");
        let installed = json!({
            "id": "chrome",
            "kind": "installed",
            "tier": 3,
        });
        assert_eq!(
            pick_cli_endpoint(&[plain_high.clone(), seven, profile.clone(), remote.clone(), installed]),
            Some(version_url(9401))
        );
        assert_eq!(
            pick_cli_endpoint(&[plain_high.clone(), profile, plain_low.clone(), remote]),
            Some(version_url(9402))
        );
        assert_eq!(
            pick_cli_endpoint(&[plain_low, plain_high]),
            Some(version_url(9403))
        );
    }

    #[test]
    fn non_loopback_ws_from_slicc_is_rejected() {
        let remote = endpoint(9405, 1, true, Some("/tmp/remote"), "10.0.0.9");
        assert_eq!(pick_cli_endpoint(&[remote]), None);
        assert_eq!(
            parse_browsers_json(r#"{"schema":1,"entries":[]}"#),
            None
        );
        assert_eq!(parse_browsers_json("not-json"), None);
        assert_eq!(
            parse_browsers_json(r#"{"schema":2,"entries":[]}"#),
            None
        );
    }

    #[test]
    fn profile_roots_cover_seven_legacy_and_six() {
        let darwin = slicc_profile_roots(&env("macos"));
        assert_eq!(
            &darwin[..3],
            &[
                "/Users/me/Library/Application Support/slicc/seven-profiles".to_string(),
                "/Users/me/Library/Application Support/slicc/profiles".to_string(),
                "/Users/me/Library/Application Support/Slicc/profiles".to_string(),
            ]
        );
        let mut linux = env("linux");
        linux.xdg_config_home = Some("/home/me/.config".to_string());
        linux.xdg_state_home = Some("/home/me/.local/state".to_string());
        let linux_roots = slicc_profile_roots(&linux);
        assert_eq!(
            &linux_roots[..3],
            &[
                "/home/me/.config/slicc/seven-profiles".to_string(),
                "/home/me/.config/slicc/profiles".to_string(),
                "/home/me/.local/state/slicc/profiles".to_string(),
            ]
        );
        let mut windows = env("windows");
        windows.app_data = Some("C:\\Users\\me\\AppData\\Roaming".to_string());
        windows.local_app_data = Some("C:\\Users\\me\\AppData\\Local".to_string());
        let windows_roots = slicc_profile_roots(&windows);
        assert!(windows_roots[0].ends_with("slicc\\seven-profiles"));
        assert!(windows_roots[1].ends_with("slicc\\profiles"));
        assert!(windows_roots[2].ends_with("Slicc\\profiles"));
    }

    #[test]
    fn newest_live_devtools_wins_and_stale_or_identity_mismatch_is_ignored() {
        let host = env("macos");
        let seven = "/Users/me/Library/Application Support/slicc/seven-profiles";
        let six = "/Users/me/Library/Application Support/Slicc/profiles";
        let mut fs = MemFs::new();
        fs.dir(seven, &["stale", "mismatch", "live"]);
        fs.dir(six, &["old"]);
        fs.file(
            &format!("{seven}/stale/{ACTIVE_PORT_FILE}"),
            30_000,
            "1\n/devtools/browser/1\n",
        );
        fs.file(
            &format!("{seven}/mismatch/{ACTIVE_PORT_FILE}"),
            20_000,
            "9400\n/devtools/browser/retired\n",
        );
        fs.file(
            &format!("{seven}/live/{ACTIVE_PORT_FILE}"),
            10_000,
            "9401\n/devtools/browser/9401\n",
        );
        fs.file(
            &format!("{six}/old/{ACTIVE_PORT_FILE}"),
            5_000,
            "9401\n/devtools/browser/9401\n",
        );
        let roots = vec![seven.to_string(), six.to_string()];
        let mut seen = Vec::new();
        let found = from_devtools_active_port(&host, &fs, &roots, &mut |url| {
            seen.push(url.to_string());
            match url {
                "http://127.0.0.1:1/json/version" => Err("refused\n".to_string()),
                "http://127.0.0.1:9400/json/version" => Ok(version_body(9400, "/devtools/browser/other")),
                "http://127.0.0.1:9401/json/version" => Ok(version_body(9401, "/devtools/browser/9401")),
                other => panic!("unexpected probe {other}"),
            }
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9401/json/version"));
        assert!(seen.contains(&"http://127.0.0.1:1/json/version".to_string()));
        assert!(seen.contains(&"http://127.0.0.1:9400/json/version".to_string()));
        assert!(version_accepts(
            &version_body(9400, "/devtools/browser/retired"),
            "/devtools/browser/retired",
            9400
        ));
        assert!(!version_accepts(
            &version_body(9400, "/devtools/browser/other"),
            "/devtools/browser/retired",
            9400
        ));
        assert!(!version_accepts(
            &format!(r#"{{"webSocketDebuggerUrl":"ws://8.8.8.8:9400/devtools/browser/retired"}}"#),
            "/devtools/browser/retired",
            9400
        ));
    }

    #[test]
    fn resolve_prefers_slicc_json_over_devtools_and_skips_probe_when_json_wins() {
        let host = env("linux");
        let root = "/home/me/.config/slicc/seven-profiles";
        let mut fs = MemFs::new();
        fs.dir(root, &["chrome"]);
        fs.file(
            &format!("{root}/chrome/{ACTIVE_PORT_FILE}"),
            1,
            "9500\n/devtools/browser/9500\n",
        );
        let entries = json!([endpoint(9401, 2, true, Some("/tmp/x"), "127.0.0.1")]);
        let doc = json!({"schema": 1, "entries": entries}).to_string();
        let mut probed = false;
        let found = resolve_attached(&host, &fs, &[root.to_string()], Some(&doc), &mut |_| {
            probed = true;
            Err("unused\n".to_string())
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9401/json/version"));
        assert!(!probed);
        let found = resolve_attached(&host, &fs, &[root.to_string()], Some("nope"), &mut |url| {
            assert_eq!(url, "http://127.0.0.1:9500/json/version");
            Ok(version_body(9500, "/devtools/browser/9500"))
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9500/json/version"));
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    #[test]
    fn fake_slicc_on_path_and_devtools_fallthrough() {
        let _guard = env_lock().lock().unwrap();
        let scratch = std::env::temp_dir().join(format!(
            "slicc-cdp-discover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let bin = scratch.join("bin");
        let home = scratch.join("home");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let (port, join, stop) = serve_version(&format!("/devtools/browser/live"));
        let profile = home
            .join("Library")
            .join("Application Support")
            .join("slicc")
            .join("seven-profiles")
            .join("chrome");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profile.join(ACTIVE_PORT_FILE),
            format!("{port}\n/devtools/browser/live\n"),
        )
        .unwrap();

        let good = bin.join("slicc");
        let doc = json!({
            "schema": 1,
            "entries": [endpoint(port, 1, true, Some("/tmp/x"), "127.0.0.1")]
        })
        .to_string()
        .replace('\'', "");
        std::fs::write(
            &good,
            format!("#!/bin/sh\nprintf '%s' '{doc}'\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&good).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&good, perms).unwrap();
        }

        let from_cli = run_slicc_browsers(Some(&good), CLI_TIMEOUT, CLI_MAX_STDOUT);
        assert_eq!(from_cli.as_deref(), Some(version_url(port).as_str()));

        let slow = bin.join("slow-slicc");
        std::fs::write(&slow, "#!/bin/sh\n/bin/sleep 5\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&slow).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&slow, perms).unwrap();
        }
        assert_eq!(
            run_slicc_browsers(Some(&slow), Duration::from_millis(200), CLI_MAX_STDOUT),
            None
        );

        #[cfg(unix)]
        {
            let closed = bin.join("closed-stdout-slicc");
            std::fs::write(&closed, "#!/bin/sh\nexec 1>&-\n/bin/sleep 3\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&closed).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&closed, perms).unwrap();
            let started = std::time::Instant::now();
            assert_eq!(
                run_slicc_browsers(Some(&closed), Duration::from_millis(200), CLI_MAX_STDOUT),
                None
            );
            assert!(
                started.elapsed() < Duration::from_millis(800),
                "closed-stdout hang exceeded margin: {:?}",
                started.elapsed()
            );
        }

        let bad = bin.join("bad-slicc");
        std::fs::write(&bad, "#!/bin/sh\nprintf not-json\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&bad).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&bad, perms).unwrap();
        }
        assert_eq!(
            run_slicc_browsers(Some(&bad), CLI_TIMEOUT, CLI_MAX_STDOUT),
            None
        );

        let boom = bin.join("boom-slicc");
        std::fs::write(&boom, "#!/bin/sh\nexit 3\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&boom).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&boom, perms).unwrap();
        }
        assert_eq!(
            run_slicc_browsers(Some(&boom), CLI_TIMEOUT, CLI_MAX_STDOUT),
            None
        );

        let host = HostEnv {
            platform: "macos",
            home: home.to_string_lossy().into_owned(),
            app_data: None,
            local_app_data: None,
            xdg_config_home: None,
            xdg_state_home: None,
        };
        let roots = slicc_profile_roots(&host);
        let found = from_devtools_active_port(&host, &DiskFs, &roots, &mut |url| {
            let response = crate::net::http_get(url)?;
            Ok(String::from_utf8_lossy(&response.body).into_owned())
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some(version_url(port).as_str()));

        let _ = stop.send(());
        let _ = join.join();
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn an_http_error_from_the_newest_port_stops_the_scan() {
        let host = env("macos");
        let root = "/Users/me/Library/Application Support/slicc/seven-profiles";
        let mut fs = MemFs::new();
        fs.dir(root, &["chrome", "edge"]);
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
        let err = from_devtools_active_port(&host, &fs, &[root.to_string()], &mut |url| {
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
        let root = "/Users/me/Library/Application Support/slicc/seven-profiles";
        let mut fs = MemFs::new();
        fs.dir(root, &["chrome", "edge"]);
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
        let found = from_devtools_active_port(&host, &fs, &[root.to_string()], &mut |url| {
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
        let mut host = env("linux");
        host.xdg_config_home = Some("/home/me/.config".to_string());
        host.xdg_state_home = Some("/home/me/.local/state".to_string());
        let roots = slicc_profile_roots(&host);
        let config = "/home/me/.config/slicc/seven-profiles";
        let legacy = "/home/me/.config/slicc/profiles";
        let six = "/home/me/.local/state/slicc/profiles";
        let snap = "/home/me/snap";
        let snap_root = format!("{snap}/chromium/common/slicc/seven-profiles");
        let snap_six = format!("{snap}/chromium/common/slicc/profiles");
        assert_eq!(roots[0], config);
        assert_eq!(roots[1], legacy);
        assert_eq!(roots[2], six);
        assert_eq!(roots[3], snap);
        let mut fs = MemFs::new();
        fs.dir(config, &["chrome"]);
        fs.dir(legacy, &["chrome"]);
        fs.dir(six, &["chrome"]);
        fs.dir(snap, &["chromium", ".hidden", "has space"]);
        fs.dir(&snap_root, &["chromium"]);
        fs.dir(&snap_six, &["chromium"]);
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
            &port_file(&format!("{legacy}/chrome"), false),
            100,
            "9777\n/devtools/browser/legacy\n",
        );
        fs.file(
            &port_file(&format!("{six}/chrome"), false),
            100,
            "9888\n/devtools/browser/six\n",
        );
        let mut seen = Vec::new();
        let found = from_devtools_active_port(&host, &fs, &roots, &mut |url| {
            seen.push(url.to_string());
            match url {
                "http://127.0.0.1:9777/json/version" | "http://127.0.0.1:9888/json/version" => {
                    Err("connection refused\n".to_string())
                }
                "http://127.0.0.1:9555/json/version" => {
                    assert_high_port(url);
                    Ok(version_body(9555, "/devtools/browser/snap"))
                }
                other => panic!("unexpected probe {other}"),
            }
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9555/json/version"));
        assert!(fs.lists.borrow().iter().any(|path| path == snap));
        assert!(fs.lists.borrow().iter().any(|path| path == &snap_root));
        assert!(!fs.lists.borrow().iter().any(|path| path == &snap_six));
        assert!(!fs
            .reads
            .borrow()
            .iter()
            .any(|path| path.contains("/snap/chromium/common/slicc/profiles/")));
        assert!(fs
            .reads
            .borrow()
            .iter()
            .any(|path| path.contains("/seven-profiles/chromium/")));
        assert!(!fs.lists.borrow().iter().any(|path| {
            path.contains("/snap/.hidden/") || path.contains("/snap/has space/")
        }));
    }

    #[test]
    fn a_windows_seven_profile_port_is_found_without_profiles() {
        let mut host = env("windows");
        host.app_data = Some("C:\\Users\\me\\AppData\\Roaming".to_string());
        host.local_app_data = Some("C:\\Users\\me\\AppData\\Local".to_string());
        let roots = slicc_profile_roots(&host);
        let root = "C:\\Users\\me\\AppData\\Roaming\\slicc\\seven-profiles";
        let legacy = "C:\\Users\\me\\AppData\\Roaming\\slicc\\profiles";
        let six = "C:\\Users\\me\\AppData\\Local\\Slicc\\profiles";
        assert_eq!(roots[0], root);
        assert_eq!(roots[1], legacy);
        assert_eq!(roots[2], six);
        let mut fs = MemFs::new();
        fs.dir(root, &["chrome"]);
        fs.dir(legacy, &["chrome"]);
        fs.dir(six, &["chrome"]);
        let dir = join_parts(true, &[root.to_string(), "chrome".to_string()]);
        fs.file(&port_file(&dir, true), 3, "9333\n/devtools/browser/win\n");
        let six_dir = join_parts(true, &[six.to_string(), "chrome".to_string()]);
        fs.file(
            &port_file(&six_dir, true),
            9,
            "9444\n/devtools/browser/six\n",
        );
        let mut seen = Vec::new();
        let found = from_devtools_active_port(&host, &fs, &roots, &mut |url| {
            seen.push(url.to_string());
            match url {
                "http://127.0.0.1:9444/json/version" => Err("connection refused\n".to_string()),
                "http://127.0.0.1:9333/json/version" => {
                    assert_high_port(url);
                    Ok(version_body(9333, "/devtools/browser/win"))
                }
                other => panic!("unexpected probe {other}"),
            }
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9333/json/version"));
        assert!(seen.contains(&"http://127.0.0.1:9444/json/version".to_string()));
        assert!(seen.contains(&"http://127.0.0.1:9333/json/version".to_string()));
        assert!(fs.reads.borrow().iter().any(|path| path.contains("seven-profiles")));
        assert!(fs.reads.borrow().iter().any(|path| path.contains("Slicc\\profiles")));
    }

    #[cfg(any(target_arch = "wasm32", target_os = "wasi"))]
    #[test]
    fn wasm_attached_is_none() {
        let found = attached(&mut |_| Err("no\n".to_string())).unwrap();
        assert_eq!(found, None);
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    #[test]
    fn missing_slicc_falls_through_without_panic() {
        let missing = std::env::temp_dir().join(format!(
            "no-such-slicc-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert_eq!(
            run_slicc_browsers(Some(&missing), CLI_TIMEOUT, CLI_MAX_STDOUT),
            None
        );
    }

    #[test]
    fn accept_cached_version_url_rejects_remote_and_non_version_paths() {
        assert!(accept_cached_version_url("http://127.0.0.1:9401/json/version"));
        assert!(accept_cached_version_url("http://localhost:9401/json/version"));
        assert!(!accept_cached_version_url("http://10.0.0.9:9401/json/version"));
        assert!(!accept_cached_version_url("https://127.0.0.1:9401/json/version"));
        assert!(!accept_cached_version_url("ws://127.0.0.1:9401/devtools/browser/x"));
        assert!(!accept_cached_version_url("http://127.0.0.1/json/version"));
        assert!(!accept_cached_version_url("http://127.0.0.1:9401/json/list"));
    }

    #[test]
    fn browsers_discovery_cache_path_sits_beside_sessions() {
        let darwin = browsers_discovery_cache_path(&env("macos"));
        assert_eq!(
            darwin,
            "/Users/me/Library/Application Support/slicc/playwright-cli/browsers-discovery.json"
        );
        let mut linux = env("linux");
        linux.xdg_config_home = Some("/home/me/.config".to_string());
        assert_eq!(
            browsers_discovery_cache_path(&linux),
            "/home/me/.config/slicc/playwright-cli/browsers-discovery.json"
        );
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    fn cache_scratch() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "slicc-cdp-browsers-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    fn make_fake_slicc(dir: &std::path::Path, name: &str, script: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    #[test]
    fn browsers_cache_positive_ttl_expires_and_negative_outlives_positive() {
        let scratch = cache_scratch();
        let cache = scratch.join(CACHE_FILE);
        let url = "http://127.0.0.1:9401/json/version";
        write_browsers_cache(&cache, Some(url)).unwrap();
        let now = std::fs::metadata(&cache).unwrap().modified().unwrap();
        assert_eq!(
            read_browsers_cache(&cache, now + Duration::from_millis(100)),
            BrowsersCache::Positive(url.to_string())
        );
        assert_eq!(
            read_browsers_cache(&cache, now + POSITIVE_CACHE_TTL + Duration::from_millis(1)),
            BrowsersCache::Miss
        );
        write_browsers_cache(&cache, None).unwrap();
        let now = std::fs::metadata(&cache).unwrap().modified().unwrap();
        assert_eq!(
            read_browsers_cache(&cache, now + POSITIVE_CACHE_TTL + Duration::from_secs(1)),
            BrowsersCache::Negative
        );
        assert_eq!(
            read_browsers_cache(&cache, now + NEGATIVE_CACHE_TTL + Duration::from_millis(1)),
            BrowsersCache::Miss
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    #[test]
    fn browsers_cache_ignores_future_mtime_wrong_mode_and_remote_url() {
        let scratch = cache_scratch();
        let cache = scratch.join(CACHE_FILE);
        write_browsers_cache(&cache, Some("http://127.0.0.1:9401/json/version")).unwrap();
        let now = std::time::SystemTime::now();
        let future = now + Duration::from_secs(60);
        let past = now - Duration::from_secs(1);
        filetime_set(&cache, future);
        assert_eq!(read_browsers_cache(&cache, now), BrowsersCache::Miss);
        filetime_set(&cache, past);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert_eq!(read_browsers_cache(&cache, now), BrowsersCache::Miss);
            std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(
            write_browsers_cache(&cache, Some("http://10.0.0.9:9401/json/version"))
                .unwrap_err()
                .as_str(),
            "cache: refusing non-loopback version URL\n"
        );
        std::fs::write(
            &cache,
            r#"{"schema":1,"url":"http://10.0.0.9:9401/json/version"}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        filetime_set(&cache, past);
        assert_eq!(read_browsers_cache(&cache, now), BrowsersCache::Miss);
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    fn filetime_set(path: &std::path::Path, when: std::time::SystemTime) {
        let file = std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap();
        file.set_modified(when).unwrap();
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    struct EnvGuard {
        home: Option<std::ffi::OsString>,
        xdg: Option<std::ffi::OsString>,
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    impl EnvGuard {
        fn isolate(home: &std::path::Path) -> Self {
            let guard = Self {
                home: std::env::var_os("HOME"),
                xdg: std::env::var_os("XDG_CONFIG_HOME"),
            };
            std::env::set_var("HOME", home);
            std::env::remove_var("XDG_CONFIG_HOME");
            guard
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.home {
                Some(value) => std::env::set_var("HOME", value),
                None => std::env::remove_var("HOME"),
            }
            match &self.xdg {
                Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    #[test]
    fn stale_cached_endpoint_is_dropped_and_slicc_runs_once_more() {
        let _guard = env_lock().lock().unwrap();
        let scratch = cache_scratch();
        let home = scratch.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let _env = EnvGuard::isolate(&home);
        let cache = scratch.join(CACHE_FILE);
        let count = scratch.join("count");
        let (port, join, stop) = serve_version("/devtools/browser/live");
        let doc = json!({
            "schema": 1,
            "entries": [endpoint(port, 1, true, Some("/tmp/x"), "127.0.0.1")]
        })
        .to_string()
        .replace('\'', "");
        let slicc = make_fake_slicc(
            &scratch,
            "slicc",
            &format!(
                "#!/bin/sh\nprintf . >> '{}' || true\nprintf '%s' '{doc}'\n",
                count.display()
            ),
        );
        write_browsers_cache(&cache, Some("http://127.0.0.1:1/json/version")).unwrap();
        let found = attached_with(
            Some(&slicc),
            Some(&cache),
            CLI_TIMEOUT,
            CLI_MAX_STDOUT,
            &mut |url| {
                if url == "http://127.0.0.1:1/json/version" {
                    return Err("connection refused\n".to_string());
                }
                let response = crate::net::http_get(url)?;
                Ok(String::from_utf8_lossy(&response.body).into_owned())
            },
        )
        .unwrap();
        assert_eq!(found.as_deref(), Some(version_url(port).as_str()));
        let calls = std::fs::read_to_string(&count).unwrap().len();
        assert_eq!(calls, 1);
        assert_eq!(
            read_browsers_cache(&cache, std::time::SystemTime::now()),
            BrowsersCache::Positive(version_url(port))
        );
        let _ = stop.send(());
        let _ = join.join();
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    #[test]
    fn two_attached_calls_within_positive_ttl_spawn_slicc_once() {
        let _guard = env_lock().lock().unwrap();
        let scratch = cache_scratch();
        let home = scratch.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let _env = EnvGuard::isolate(&home);
        let cache = scratch.join(CACHE_FILE);
        let count = scratch.join("count");
        let (port, join, stop) = serve_version("/devtools/browser/live");
        let doc = json!({
            "schema": 1,
            "entries": [endpoint(port, 1, true, Some("/tmp/x"), "127.0.0.1")]
        })
        .to_string()
        .replace('\'', "");
        let slicc = make_fake_slicc(
            &scratch,
            "slicc",
            &format!(
                "#!/bin/sh\nprintf . >> '{}' || true\nprintf '%s' '{doc}'\n",
                count.display()
            ),
        );
        let mut probe = |url: &str| {
            let response = crate::net::http_get(url)?;
            Ok(String::from_utf8_lossy(&response.body).into_owned())
        };
        let first = attached_with(
            Some(&slicc),
            Some(&cache),
            CLI_TIMEOUT,
            CLI_MAX_STDOUT,
            &mut probe,
        )
        .unwrap();
        let second = attached_with(
            Some(&slicc),
            Some(&cache),
            CLI_TIMEOUT,
            CLI_MAX_STDOUT,
            &mut probe,
        )
        .unwrap();
        assert_eq!(first.as_deref(), Some(version_url(port).as_str()));
        assert_eq!(second, first);
        assert_eq!(std::fs::read_to_string(&count).unwrap().len(), 1);
        let _ = stop.send(());
        let _ = join.join();
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
    #[test]
    fn hanging_slicc_is_cached_negative_so_second_call_skips_timeout() {
        let _guard = env_lock().lock().unwrap();
        let scratch = cache_scratch();
        let home = scratch.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let _env = EnvGuard::isolate(&home);
        let cache = scratch.join(CACHE_FILE);
        let count = scratch.join("count");
        let slicc = make_fake_slicc(
            &scratch,
            "slicc",
            &format!(
                "#!/bin/sh\nprintf . >> '{}' || true\n/bin/sleep 5\n",
                count.display()
            ),
        );
        let timeout = Duration::from_millis(200);
        let started = std::time::Instant::now();
        let first = attached_with(Some(&slicc), Some(&cache), timeout, CLI_MAX_STDOUT, &mut |_| {
            Err("unused\n".to_string())
        })
        .unwrap();
        let first_elapsed = started.elapsed();
        assert_eq!(first, None);
        assert!(first_elapsed >= timeout);
        assert!(first_elapsed < Duration::from_millis(800));
        assert_eq!(
            read_browsers_cache(&cache, std::time::SystemTime::now()),
            BrowsersCache::Negative
        );
        let started = std::time::Instant::now();
        let second = attached_with(Some(&slicc), Some(&cache), timeout, CLI_MAX_STDOUT, &mut |_| {
            Err("unused\n".to_string())
        })
        .unwrap();
        assert_eq!(second, None);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "negative cache still waited {:?}",
            started.elapsed()
        );
        assert_eq!(std::fs::read_to_string(&count).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&scratch);
    }
}
