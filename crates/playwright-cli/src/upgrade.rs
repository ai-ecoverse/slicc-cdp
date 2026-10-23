#![cfg_attr(target_os = "wasi", allow(dead_code))]

use crate::args::{Invocation, Output};

pub fn usage(command: &str) -> String {
    format!(
        "playwright-cli {command} [--check] [--force]\n\nReplace this binary with the latest release of ai-ecoverse/slicc-cdp from GitHub\nThe release binary must match the release's SHA256SUMS and report the release version before it replaces this one.\nThe binary is only replaced when it is writable. A copy installed with cargo or inside an app bundle is not replaced.\n\nOptions:\n  --check                     only report whether a newer release exists\n  --force                     replace a development build, or reinstall the current release\n"
    )
}

#[cfg(target_os = "wasi")]
pub fn run(invocation: &Invocation) -> Output {
    let command = invocation.command.as_deref().unwrap_or("upgrade");
    Output::err(format!(
        "playwright-cli {command}: not available in the wasm build; the kernel updates it\n"
    ))
}

#[cfg(not(target_os = "wasi"))]
pub fn run(invocation: &Invocation) -> Output {
    let command = invocation.command.as_deref().unwrap_or("upgrade");
    let check = native::flag(invocation, "check");
    let force = native::flag(invocation, "force");
    match native::upgrade(check, force) {
        Ok(stdout) => Output::ok(stdout),
        Err(message) => Output::err(format!("playwright-cli {command}: {message}\n")),
    }
}

#[cfg(windows)]
pub fn remove_parked() {
    if let Ok(exe) = native::executable() {
        let _ = std::fs::remove_file(native::parked_path(&exe));
    }
}

fn is_release_version(version: &str) -> bool {
    let trimmed = normalize(version);
    !trimmed.is_empty()
        && trimmed
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn is_development(version: &str) -> bool {
    !is_release_version(version) || segments(version).iter().all(|part| *part == 0)
}

fn normalize(version: &str) -> &str {
    let trimmed = version.trim();
    trimmed.strip_prefix('v').unwrap_or(trimmed)
}

fn segments(version: &str) -> Vec<u64> {
    normalize(version)
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

fn is_newer(latest: &str, current: &str) -> bool {
    let (a, b) = (segments(latest), segments(current));
    for index in 0..a.len().max(b.len()) {
        let left = a.get(index).copied().unwrap_or(0);
        let right = b.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Report(String),
    Refuse(String),
    Replace,
}

fn plan(current: &str, latest: &str, check: bool, force: bool) -> Plan {
    let dev = is_development(current);
    let current = normalize(current);
    let newer = is_newer(latest, current);
    if check {
        return Plan::Report(if dev {
            format!("latest release: {latest} (this is a development build, {current})\n")
        } else if newer {
            format!("playwright-cli {latest} is available (you have {current}); run `playwright-cli upgrade`\n")
        } else {
            format!("playwright-cli {current} is up to date\n")
        });
    }
    if dev && !force {
        return Plan::Refuse(format!(
            "refusing to replace a development build ({current}) with release {latest}; pass --force to replace it"
        ));
    }
    if !dev && !force && !newer {
        return Plan::Report(format!("playwright-cli {current} is up to date\n"));
    }
    Plan::Replace
}

fn asset_name(os: &str, arch: &str) -> Option<String> {
    let os = match os {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "windows",
        _ => return None,
    };
    let arch = match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return None,
    };
    let ext = if os == "windows" { ".exe" } else { "" };
    Some(format!("playwright-cli-{os}-{arch}{ext}"))
}

fn parse_checksums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let sum = fields.next()?;
            let name = fields.next()?;
            if fields.next().is_some() {
                return None;
            }
            Some((name.trim_start_matches('*').to_string(), sum.to_ascii_lowercase()))
        })
        .collect()
}

fn managed_by(exe: &std::path::Path, cargo_bins: &[std::path::PathBuf]) -> Option<&'static str> {
    let parts: Vec<String> = exe
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    for (index, part) in parts.iter().enumerate() {
        if part.ends_with(".app") && parts.get(index + 1).is_some_and(|next| next == "Contents") {
            return Some("playwright-cli runs inside an app bundle; the app updates it");
        }
        if part == "node_modules" {
            return Some("playwright-cli runs from node_modules; update the package that installed it");
        }
    }
    let dir = exe.parent()?;
    if cargo_bins.iter().any(|bin| bin == dir) {
        return Some("playwright-cli was installed with cargo; upgrade it with `cargo install slicc-cdp`");
    }
    None
}

#[cfg(not(target_os = "wasi"))]
mod native {
    use super::*;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const REPO: &str = "ai-ecoverse/slicc-cdp";
    const DEFAULT_API: &str = "https://api.github.com";
    const API_ENV: &str = "SLICC_UPDATE_API_BASE";
    const SUMS: &str = "SHA256SUMS";
    const MAX_JSON: usize = 4 << 20;
    const MAX_SUMS: usize = 1 << 20;
    const MAX_BINARY: usize = 256 << 20;
    const MAX_REDIRECTS: usize = 5;
    const VERSION: &str = env!("PLAYWRIGHT_CLI_VERSION");

    pub fn flag(invocation: &Invocation, name: &str) -> bool {
        invocation
            .flags
            .get(name)
            .is_some_and(|value| value != "false")
    }

    struct Release {
        version: String,
        asset: String,
        asset_url: String,
        sums_url: String,
    }

    pub fn upgrade(check: bool, force: bool) -> Result<String, String> {
        let exe = executable().map_err(|err| format!("locating this binary: {err}"))?;
        if let Some(advice) = managed_by(&exe, &cargo_bins()) {
            return Err(advice.to_string());
        }
        let _ = std::fs::remove_file(parked_path(&exe));
        let release = latest()?;
        match plan(VERSION, &release.version, check, force) {
            Plan::Report(text) => return Ok(text),
            Plan::Refuse(text) => return Err(text),
            Plan::Replace => {}
        }
        writable(&exe)?;
        let current = normalize(VERSION);
        let mut out = format!("upgrading playwright-cli {current} to {}\n", release.version);
        apply(&release, &exe)?;
        out.push_str(&format!(
            "upgraded {} to playwright-cli {} (SHA256SUMS verified)\n",
            exe.display(),
            release.version
        ));
        Ok(out)
    }

    pub fn executable() -> std::io::Result<PathBuf> {
        let exe = std::env::current_exe()?;
        Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
    }

    pub fn parked_path(exe: &Path) -> PathBuf {
        let mut name = exe.as_os_str().to_os_string();
        name.push(".old");
        PathBuf::from(name)
    }

    fn staging_path(exe: &Path) -> PathBuf {
        let stem = exe
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "playwright-cli".to_string());
        let name = match exe.extension() {
            Some(ext) => format!("{stem}.new.{}", ext.to_string_lossy()),
            None => format!("{stem}.new"),
        };
        exe.with_file_name(name)
    }

    fn cargo_bins() -> Vec<PathBuf> {
        let mut bins = Vec::new();
        if let Some(home) = std::env::var_os("CARGO_HOME").filter(|home| !home.is_empty()) {
            bins.push(PathBuf::from(home).join("bin"));
        }
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .or_else(|| std::env::var_os("USERPROFILE").filter(|home| !home.is_empty()));
        if let Some(home) = home {
            bins.push(PathBuf::from(home).join(".cargo").join("bin"));
        }
        bins.into_iter()
            .map(|bin| std::fs::canonicalize(&bin).unwrap_or(bin))
            .collect()
    }

    fn api_base() -> String {
        std::env::var(API_ENV)
            .ok()
            .filter(|base| !base.is_empty())
            .unwrap_or_else(|| DEFAULT_API.to_string())
            .trim_end_matches('/')
            .to_string()
    }

    fn get(url: &str, accept: &str, cap: usize, total: Duration) -> Result<Vec<u8>, String> {
        let agent = format!("playwright-cli/{}", normalize(VERSION));
        let mut current = url.to_string();
        for _ in 0..=MAX_REDIRECTS {
            let mut headers = vec![("User-Agent", agent.as_str())];
            if !accept.is_empty() {
                headers.push(("Accept", accept));
            }
            let response = cdp_client::net::http_fetch(&current, &headers, cap, total)
                .map_err(|err| err.trim_end().to_string())?;
            match response.status {
                200 => return Ok(response.body),
                301 | 302 | 303 | 307 | 308 => {
                    let location = response
                        .headers
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case("location"))
                        .map(|(_, value)| value.clone())
                        .ok_or_else(|| format!("HTTP {} without a Location", response.status))?;
                    let base = url::Url::parse(&current).map_err(|err| err.to_string())?;
                    let next = base.join(&location).map_err(|err| format!("bad redirect: {err}"))?;
                    if base.scheme() == "https" && next.scheme() != "https" {
                        return Err(format!("refusing a redirect from https to {}", next.scheme()));
                    }
                    current = next.to_string();
                }
                status => return Err(format!("HTTP {status}")),
            }
        }
        Err(format!("more than {MAX_REDIRECTS} redirects"))
    }

    fn latest() -> Result<Release, String> {
        let url = format!("{}/repos/{REPO}/releases/latest", api_base());
        let body = get(&url, "application/vnd.github+json", MAX_JSON, Duration::from_secs(60))
            .map_err(|err| format!("asking GitHub for the latest release: {err}"))?;
        let json: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|err| format!("decoding the latest release: {err}"))?;
        let tag = json.get("tag_name").and_then(|tag| tag.as_str()).unwrap_or("");
        if !is_release_version(tag) {
            return Err(format!("the latest release has an unexpected tag {tag:?}"));
        }
        let version = normalize(tag).to_string();
        let asset = asset_name(std::env::consts::OS, std::env::consts::ARCH).ok_or_else(|| {
            format!(
                "no release binary for {} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        })?;
        let mut asset_url = None;
        let mut sums_url = None;
        for entry in json.get("assets").and_then(|assets| assets.as_array()).into_iter().flatten() {
            let name = entry.get("name").and_then(|name| name.as_str());
            let link = entry
                .get("browser_download_url")
                .and_then(|link| link.as_str())
                .map(str::to_string);
            if name == Some(asset.as_str()) {
                asset_url = link;
            } else if name == Some(SUMS) {
                sums_url = link;
            }
        }
        let asset_url = asset_url.ok_or_else(|| format!("release {version} has no {asset}"))?;
        let sums_url = sums_url.ok_or_else(|| format!("release {version} has no {SUMS}"))?;
        Ok(Release {
            version,
            asset,
            asset_url,
            sums_url,
        })
    }

    #[cfg(unix)]
    fn can_write(path: &Path) -> bool {
        use std::os::unix::ffi::OsStrExt;
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
    }

    #[cfg(not(unix))]
    fn can_write(path: &Path) -> bool {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {
                let probe = path.join(format!(".playwright-cli-upgrade-{}", std::process::id()));
                let created = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
                let _ = std::fs::remove_file(&probe);
                created
            }
            Ok(meta) => !meta.permissions().readonly(),
            Err(_) => false,
        }
    }

    fn writable(exe: &Path) -> Result<(), String> {
        let dir = exe.parent().unwrap_or_else(|| Path::new("."));
        if !can_write(exe) {
            return Err(format!(
                "{} is not writable; rerun with permission to write it, or reinstall with install.sh",
                exe.display()
            ));
        }
        if !can_write(dir) {
            return Err(format!(
                "{} is not writable, so {} can't be replaced; rerun with permission to write it, or reinstall with install.sh",
                dir.display(),
                exe.display()
            ));
        }
        Ok(())
    }

    fn apply(release: &Release, exe: &Path) -> Result<(), String> {
        let sums = get(&release.sums_url, "", MAX_SUMS, Duration::from_secs(60))
            .map_err(|err| format!("downloading {SUMS}: {err}"))?;
        let sums = String::from_utf8_lossy(&sums);
        let want = parse_checksums(&sums)
            .into_iter()
            .find(|(name, _)| *name == release.asset)
            .map(|(_, sum)| sum)
            .ok_or_else(|| format!("{SUMS} has no entry for {}", release.asset))?;
        let binary = get(&release.asset_url, "", MAX_BINARY, Duration::from_secs(600))
            .map_err(|err| format!("downloading {}: {err}", release.asset))?;
        if cdp_client::hash::sha256_hex(&binary) != want {
            return Err(format!("{} does not match its SHA-256 in {SUMS}", release.asset));
        }
        let staging = staging_path(exe);
        let result = stage(&staging, &binary)
            .and_then(|()| verify(&staging, &release.version))
            .and_then(|()| swap(&staging, exe));
        if result.is_err() {
            let _ = std::fs::remove_file(&staging);
        }
        result
    }

    fn stage(staging: &Path, binary: &[u8]) -> Result<(), String> {
        let _ = std::fs::remove_file(staging);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o755);
        }
        let mut file = options
            .open(staging)
            .map_err(|err| format!("writing {}: {err}", staging.display()))?;
        file.write_all(binary)
            .and_then(|()| file.sync_all())
            .map_err(|err| format!("writing {}: {err}", staging.display()))?;
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(staging, std::fs::Permissions::from_mode(0o755))
                .map_err(|err| format!("marking {} executable: {err}", staging.display()))?;
        }
        Ok(())
    }

    fn verify(staging: &Path, version: &str) -> Result<(), String> {
        let mut child = Command::new(staging)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| format!("the downloaded binary failed to run --version: {err}"))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("the downloaded binary did not answer --version in 30 seconds".to_string());
                }
                Err(err) => return Err(format!("the downloaded binary failed to run --version: {err}")),
            }
        }
        let output = child
            .wait_with_output()
            .map_err(|err| format!("the downloaded binary failed to run --version: {err}"))?;
        if !output.status.success() {
            return Err(format!("the downloaded binary failed to run --version ({})", output.status));
        }
        let got = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let want = format!("playwright-cli {version}");
        if got != want {
            return Err(format!("the downloaded binary reports {got:?}, expected {want:?}"));
        }
        Ok(())
    }

    fn swap(staging: &Path, exe: &Path) -> Result<(), String> {
        let parked = parked_path(exe);
        let park = cfg!(windows) && exe.exists();
        if park {
            let _ = std::fs::remove_file(&parked);
            std::fs::rename(exe, &parked)
                .map_err(|err| format!("moving the running binary aside: {err}"))?;
        }
        if let Err(err) = std::fs::rename(staging, exe) {
            if park {
                let _ = std::fs::rename(&parked, exe);
            }
            return Err(format!("replacing {}: {err}", exe.display()));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn staging_keeps_the_extension_last() {
            assert_eq!(
                staging_path(Path::new("/opt/bin/playwright-cli")),
                PathBuf::from("/opt/bin/playwright-cli.new")
            );
            assert_eq!(
                staging_path(Path::new("/opt/bin/playwright-cli.exe")),
                PathBuf::from("/opt/bin/playwright-cli.new.exe")
            );
            assert_eq!(
                parked_path(Path::new("/opt/bin/playwright-cli.exe")),
                PathBuf::from("/opt/bin/playwright-cli.exe.old")
            );
        }

        #[test]
        fn swap_replaces_the_target_and_leaves_nothing_behind() {
            let dir = std::env::temp_dir().join(format!("playwright-cli-swap-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let exe = dir.join("playwright-cli");
            std::fs::write(&exe, b"old").unwrap();
            let staging = staging_path(&exe);
            stage(&staging, b"new").unwrap();
            swap(&staging, &exe).unwrap();
            assert_eq!(std::fs::read(&exe).unwrap(), b"new");
            let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|entry| entry.unwrap().file_name()).collect();
            assert_eq!(left, vec![std::ffi::OsString::from("playwright-cli")]);
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("v2.0.0", "1.99.99"));
        assert!(is_newer("1.2.1", "1.2"));
        assert!(!is_newer("1.2.0", "1.2"));
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(!is_newer("1.2.3", "1.3.0"));
    }

    #[test]
    fn development_builds_are_recognised() {
        assert!(is_development("0.0.0"));
        assert!(is_development("1.2.3-dev"));
        assert!(is_development(""));
        assert!(!is_development("1.2.3"));
        assert!(!is_development("v0.1.0"));
        assert!(is_release_version("v1.2.3"));
        assert!(!is_release_version("v1..3"));
        assert!(!is_release_version("latest"));
    }

    #[test]
    fn plans_follow_the_version_and_flags() {
        let report = |text: &str| Plan::Report(format!("{text}\n"));
        assert_eq!(plan("1.0.0", "1.1.0", true, false), report("playwright-cli 1.1.0 is available (you have 1.0.0); run `playwright-cli upgrade`"));
        assert_eq!(plan("1.1.0", "1.1.0", true, false), report("playwright-cli 1.1.0 is up to date"));
        assert_eq!(plan("1.2.0", "1.1.0", true, true), report("playwright-cli 1.2.0 is up to date"));
        assert_eq!(plan("0.0.0", "1.1.0", true, false), report("latest release: 1.1.0 (this is a development build, 0.0.0)"));
        assert_eq!(plan("1.0.0", "1.1.0", false, false), Plan::Replace);
        assert_eq!(plan("1.1.0", "1.1.0", false, false), report("playwright-cli 1.1.0 is up to date"));
        assert_eq!(plan("1.2.0", "1.1.0", false, false), report("playwright-cli 1.2.0 is up to date"));
        assert_eq!(plan("1.1.0", "1.1.0", false, true), Plan::Replace);
        assert_eq!(
            plan("0.0.0", "1.1.0", false, false),
            Plan::Refuse("refusing to replace a development build (0.0.0) with release 1.1.0; pass --force to replace it".to_string())
        );
        assert_eq!(plan("0.0.0", "1.1.0", false, true), Plan::Replace);
    }

    #[test]
    fn asset_names_match_the_release() {
        assert_eq!(asset_name("macos", "aarch64").as_deref(), Some("playwright-cli-darwin-arm64"));
        assert_eq!(asset_name("macos", "x86_64").as_deref(), Some("playwright-cli-darwin-amd64"));
        assert_eq!(asset_name("linux", "aarch64").as_deref(), Some("playwright-cli-linux-arm64"));
        assert_eq!(asset_name("linux", "x86_64").as_deref(), Some("playwright-cli-linux-amd64"));
        assert_eq!(asset_name("windows", "x86_64").as_deref(), Some("playwright-cli-windows-amd64.exe"));
        assert_eq!(asset_name("windows", "aarch64").as_deref(), Some("playwright-cli-windows-arm64.exe"));
        assert_eq!(asset_name("freebsd", "x86_64"), None);
        assert_eq!(asset_name("linux", "riscv64"), None);
    }

    #[test]
    fn checksums_parse_like_sha256sum() {
        let sums = parse_checksums("ABCD  playwright-cli-linux-amd64\nef01 *curlwright-linux-amd64\nbad line here\n\n");
        assert_eq!(
            sums,
            vec![
                ("playwright-cli-linux-amd64".to_string(), "abcd".to_string()),
                ("curlwright-linux-amd64".to_string(), "ef01".to_string()),
            ]
        );
    }

    #[test]
    fn managed_installs_are_left_alone() {
        let cargo = vec![PathBuf::from("/home/me/.cargo/bin")];
        assert!(managed_by(Path::new("/home/me/.cargo/bin/playwright-cli"), &cargo)
            .unwrap()
            .contains("cargo install slicc-cdp"));
        assert!(managed_by(Path::new("/Applications/Sliccstart.app/Contents/Helpers/playwright-cli"), &cargo)
            .unwrap()
            .contains("app bundle"));
        assert!(managed_by(Path::new("/p/node_modules/.bin/playwright-cli"), &cargo)
            .unwrap()
            .contains("node_modules"));
        assert_eq!(managed_by(Path::new("/home/me/.local/bin/playwright-cli"), &cargo), None);
        assert_eq!(managed_by(Path::new("/home/me/.cargo/bin/sub/playwright-cli"), &cargo), None);
    }
}
