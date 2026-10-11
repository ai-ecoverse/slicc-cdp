#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

const ACTIVE_PORT_FILE: &str = "DevToolsActivePort";

struct HostEnv {
    platform: &'static str,
    home: String,
    app_data: Option<String>,
    xdg_config_home: Option<String>,
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

pub(crate) fn attached_port() -> Result<Option<String>, String> {
    #[cfg(any(test, target_arch = "wasm32"))]
    {
        Ok(None)
    }
    #[cfg(all(not(test), not(target_arch = "wasm32")))]
    {
        select_port(&current_env(), &DiskFs, &mut |url| http_body(url))
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

fn parse_active_port(text: &str) -> Option<(u16, String)> {
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
    cdp_client::connect::websocket_url_from_version(body)
        .ok()
        .is_some_and(|url| url.ends_with(path))
}

fn version_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/json/version")
}

fn collect_port_files<F: ScanFs>(env: &HostEnv, fs: &F) -> Vec<PortCandidate> {
    let windows = windows_host(env);
    let mut found = Vec::new();
    collect_profile_root(&config_seven_root(env), windows, fs, &mut found);
    let snap = join_parts(windows, &[env.home.clone(), "snap".to_string()]);
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
    let parts = path_parts(path);
    parts.windows(2).any(|window| {
        window[0].eq_ignore_ascii_case("slicc") && window[1].eq_ignore_ascii_case("profiles")
    }) || parts
        .iter()
        .any(|part| part.starts_with("browser-coding-agent-chrome"))
}

fn path_parts(path: &str) -> Vec<String> {
    path.replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn windows_host(env: &HostEnv) -> bool {
    env.platform == "windows"
}

fn child_path(dir: &str, name: &str, windows: bool) -> String {
    join_parts(windows, &[dir.to_string(), name.to_string()])
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

#[cfg(all(not(test), not(target_arch = "wasm32")))]
struct DiskFs;

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
        let micros = modified
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_micros();
        i64::try_from(micros).ok()
    }
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn current_env() -> HostEnv {
    HostEnv {
        platform: host_platform(),
        home: home_dir(),
        app_data: env_var("APPDATA"),
        xdg_config_home: env_var("XDG_CONFIG_HOME"),
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
fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[cfg(all(not(test), not(target_arch = "wasm32")))]
fn http_body(url: &str) -> Result<String, String> {
    let response = cdp_client::net::http_get(url)?;
    Ok(String::from_utf8_lossy(&response.body).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    fn env(platform: &'static str) -> HostEnv {
        HostEnv {
            platform,
            home: match platform {
                "macos" => "/Users/me".to_string(),
                "windows" => "C:\\Users\\me".to_string(),
                _ => "/home/me".to_string(),
            },
            app_data: None,
            xdg_config_home: None,
        }
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
    fn tests_do_not_scan_the_disk() {
        assert_eq!(attached_port().unwrap(), None);
    }

    #[test]
    fn a_live_9333_file_wins_and_reserved_ports_are_not_probed() {
        let host = env("macos");
        let root = config_seven_root(&host);
        assert_eq!(
            root,
            "/Users/me/Library/Application Support/slicc/seven-profiles"
        );
        let six = "/Users/me/Library/Application Support/Slicc/profiles";
        let mut fs = ClockFs::new();
        fs.dir(six, &["chrome"]);
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
            &port_file(&format!("{root}/browser-coding-agent-chrome"), false),
            20,
            "9555\n/devtools/browser/agent\n",
        );
        fs.file(
            &port_file(&format!("{root}/.migrated-from-profiles"), false),
            40,
            "9667\n/devtools/browser/marker\n",
        );
        fs.file(
            &port_file(&format!("{root}/.hidden"), false),
            39,
            "9668\n/devtools/browser/hidden\n",
        );
        fs.file(
            &port_file(&format!("{root}/chrome..old"), false),
            38,
            "9669\n/devtools/browser/dots\n",
        );
        fs.file(
            &port_file(&format!("{root}/has space"), false),
            37,
            "9670\n/devtools/browser/space\n",
        );
        fs.file(
            &port_file(&format!("{six}/chrome"), false),
            99,
            "9666\n/devtools/browser/six\n",
        );
        fs.dir(
            &root,
            &[
                ".migrated-from-profiles",
                ".hidden",
                "browser-coding-agent-chrome",
                "brave",
                "chrome",
                "chrome..old",
                "edge",
                "has space",
                "live",
            ],
        );
        let live = format!("{root}/live");
        fs.file(
            &port_file(&live, false),
            1,
            "9333\n/devtools/browser/old\n",
        );
        let mut seen = Vec::new();
        let found = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            assert_eq!(url, "http://127.0.0.1:9333/json/version");
            Ok(version_body(9333, "/devtools/browser/old"))
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9333/json/version"));
        assert_eq!(seen, vec!["http://127.0.0.1:9333/json/version".to_string()]);
        assert!(seen.iter().all(|url| !url.contains("9222")));
        assert!(seen.iter().all(|url| !url.contains("9223")));
        assert!(seen.iter().all(|url| !url.contains("9228")));
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
    fn a_mismatched_path_and_a_refused_port_fall_through() {
        let host = env("macos");
        let root = config_seven_root(&host);
        let mut fs = ClockFs::new();
        fs.dir(&root, &["chrome", "edge", "brave"]);
        fs.file(
            &port_file(&format!("{root}/edge"), false),
            5,
            "9444\n/devtools/browser/new\n",
        );
        fs.file(
            &port_file(&format!("{root}/brave"), false),
            4,
            "9555\n/devtools/browser/dead\n",
        );
        fs.file(
            &port_file(&format!("{root}/chrome"), false),
            1,
            "9333\n/devtools/browser/old\n",
        );
        let mut seen = Vec::new();
        let found = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            if url.contains(":9444/") {
                Ok(version_body(9444, "/devtools/browser/other"))
            } else if url.contains(":9555/") {
                Err("connection refused\n".to_string())
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
                "http://127.0.0.1:9555/json/version".to_string(),
                "http://127.0.0.1:9333/json/version".to_string(),
            ]
        );
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
    fn a_newer_snap_profile_wins_and_relative_xdg_is_ignored() {
        let mut host = env("linux");
        host.xdg_config_home = Some("relative".to_string());
        let config = config_seven_root(&host);
        assert_eq!(config, "/home/me/.config/slicc/seven-profiles");
        host.xdg_config_home = Some("/xdg".to_string());
        let config = config_seven_root(&host);
        assert_eq!(config, "/xdg/slicc/seven-profiles");
        let snap = format!("{}/snap", host.home);
        let snap_root = format!("{snap}/chromium/common/slicc/seven-profiles");
        let snap_six = format!("{snap}/chromium/common/slicc/profiles");
        let mut fs = ClockFs::new();
        fs.dir(&config, &["chrome"]);
        fs.dir(&snap, &["chromium"]);
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
    }

    #[test]
    fn a_windows_profile_port_is_found() {
        let mut host = env("windows");
        assert_eq!(
            config_seven_root(&host),
            "C:\\Users\\me\\AppData\\Roaming\\slicc\\seven-profiles"
        );
        host.app_data = Some("C:\\Users\\me\\AppData\\Roaming".to_string());
        let root = config_seven_root(&host);
        let mut fs = ClockFs::new();
        fs.dir(&root, &["chrome"]);
        let dir = join_parts(true, &[root.clone(), "chrome".to_string()]);
        fs.file(&port_file(&dir, true), 3, "9333\n/devtools/browser/win\n");
        let mut seen = Vec::new();
        let found = select_port(&host, &fs, &mut |url| {
            seen.push(url.to_string());
            assert_high_port(url);
            Ok(version_body(9333, "/devtools/browser/win"))
        })
        .unwrap();
        assert_eq!(found.as_deref(), Some("http://127.0.0.1:9333/json/version"));
        assert_eq!(seen, vec!["http://127.0.0.1:9333/json/version".to_string()]);
        assert_no_six(&fs.reads.borrow());
        assert_no_six(&fs.lists.borrow());
    }
}
