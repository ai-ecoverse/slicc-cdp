use serde_json::Value;
use std::time::Duration;

pub const CLI_TIMEOUT: Duration = Duration::from_millis(1500);
pub const CLI_MAX_STDOUT: usize = 1_000_000;
const ACTIVE_PORT_FILE: &str = "DevToolsActivePort";

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

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
pub fn attached(
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<Option<String>, String> {
    if let Some(url) = run_slicc_browsers(None, CLI_TIMEOUT, CLI_MAX_STDOUT) {
        return Ok(Some(url));
    }
    let env = current_env();
    let roots = slicc_profile_roots(&env);
    from_devtools_active_port(&env, &DiskFs, &roots, probe)
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]
pub fn run_slicc_browsers(
    program: Option<&std::path::Path>,
    timeout: Duration,
    max_stdout: usize,
) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

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
    let mut stdout = child.stdout.take()?;
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
    let buf = match rx.recv_timeout(timeout) {
        Ok(Ok(buf)) => buf,
        Ok(Err(())) | Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let status = child.wait().ok()?;
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
        reads: RefCell<Vec<String>>,
    }

    impl MemFs {
        fn new() -> Self {
            Self {
                dirs: BTreeMap::new(),
                files: BTreeMap::new(),
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
}
