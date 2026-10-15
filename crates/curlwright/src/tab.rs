use crate::parse::Stop;
use cdp_client::cdp::Cdp;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Page {
    pub target_id: String,
    pub url: String,
}

pub fn list_pages(cdp: &mut Cdp) -> Result<Vec<Page>, String> {
    let result = cdp.call("Target.getTargets", json!({}), None)?;
    let infos = result.get("targetInfos").and_then(|value| value.as_array()).cloned().unwrap_or_default();
    let mut pages = Vec::new();
    for info in infos {
        if info.get("type").and_then(|value| value.as_str()) != Some("page") {
            continue;
        }
        let target_id = info.get("targetId").and_then(|value| value.as_str()).unwrap_or("").to_string();
        let url = info.get("url").and_then(|value| value.as_str()).unwrap_or("").to_string();
        let title = info.get("title").and_then(|value| value.as_str()).unwrap_or("").to_string();
        if target_id.is_empty() || is_internal(&url, &title) || cdp.is_protected_target(&target_id) {
            continue;
        }
        pages.push(Page { target_id, url });
    }
    Ok(pages)
}

fn is_internal(url: &str, title: &str) -> bool {
    let url = url.trim();
    let title = title.trim();
    title == "Omnibox Popup"
        || url.starts_with("chrome://")
        || url.starts_with("chrome-search://")
        || url.starts_with("chrome-untrusted://")
        || url.starts_with("devtools://")
        || (url.is_empty() && title.to_ascii_lowercase().ends_with("popup"))
}

pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map(|(_, host)| host).unwrap_or(authority);
    if authority.is_empty() {
        return None;
    }
    let default_port = if scheme == "https" { 443 } else { 80 };
    let (host, port) = split_authority(authority, default_port)?;
    let host = host.to_ascii_lowercase();
    if port == default_port {
        Some(format!("{scheme}://{host}"))
    } else if host.contains(':') {
        Some(format!("{scheme}://[{host}]:{port}"))
    } else {
        Some(format!("{scheme}://{host}:{port}"))
    }
}

fn split_authority(authority: &str, default_port: u16) -> Option<(String, u16)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = rest[..end].to_string();
        let after = &rest[end + 1..];
        let port = if after.is_empty() {
            default_port
        } else {
            after.strip_prefix(':')?.parse().ok()?
        };
        return Some((host, port));
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        if !host.is_empty() && !host.contains(':') && port.chars().all(|ch| ch.is_ascii_digit()) && !port.is_empty() {
            return Some((host.to_string(), port.parse().ok()?));
        }
    }
    Some((authority.to_string(), default_port))
}

fn format_candidates(pages: &[&Page], full_urls: bool) -> String {
    pages.iter().map(|page| {
        let url = if full_urls { page.url.clone() } else { cdp_client::sanitize::redact_url(&page.url) };
        format!("  --tab={}  {}", page.target_id, url)
    }).collect::<Vec<_>>().join("\n")
}

fn session_current() -> Option<String> {
    let text = fs::read_to_string(session_file()?).ok()?;
    current_target(&text)
}

fn session_file() -> Option<PathBuf> {
    #[cfg(target_arch = "wasm32")]
    let cwd = cdp_client::state::wasm_cwd(|name| std::env::var(name).ok())?;
    #[cfg(not(target_arch = "wasm32"))]
    let cwd = std::env::current_dir().ok()?;
    let canonical = cwd.canonicalize().unwrap_or(cwd);
    session_path(&config_root()?, &canonical)
}

#[cfg(target_arch = "wasm32")]
fn config_root() -> Option<String> {
    cdp_client::state::wasm_config_root(|name| std::env::var(name).ok())
}

#[cfg(not(target_arch = "wasm32"))]
fn config_root() -> Option<String> {
    #[cfg(test)]
    {
        Some(
            std::env::temp_dir()
                .join("slicc-cdp-session-tests")
                .to_string_lossy()
                .into_owned(),
        )
    }
    #[cfg(not(test))]
    {
        Some(crate::profiles::user_config_dir())
    }
}

fn session_path(config_dir: &str, cwd: &std::path::Path) -> Option<PathBuf> {
    let root = std::path::Path::new(config_dir);
    if !root.is_absolute() {
        return None;
    }
    let key = cdp_client::hash::sha256_hex(cwd.as_os_str().as_encoded_bytes());
    Some(
        root.join("slicc")
            .join("playwright-cli")
            .join("sessions")
            .join(format!("{key}.json")),
    )
}

pub fn current_target(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    value.get("current").and_then(|item| item.as_str()).filter(|tab| !tab.is_empty()).map(str::to_string)
}

pub fn choose_tab(pages: &[Page], url: &str, explicit: Option<&str>, current: Option<&str>, full_urls: bool) -> Result<String, Stop> {
    if let Some(tab) = explicit.filter(|tab| !tab.is_empty()) {
        return Ok(tab.to_string());
    }
    if pages.is_empty() {
        return Err(Stop::usage("curlwright: no open tabs — open one with `playwright-cli open <url>`"));
    }
    if let Some(tab) = current.filter(|tab| pages.iter().any(|page| page.target_id == *tab)) {
        return Ok(tab.to_string());
    }
    let wanted = origin_of(url);
    let same: Vec<&Page> = match &wanted {
        Some(origin) => pages.iter().filter(|page| origin_of(&page.url).as_deref() == Some(origin.as_str())).collect(),
        None => Vec::new(),
    };
    if same.len() == 1 {
        return Ok(same[0].target_id.clone());
    }
    if same.len() > 1 {
        let origin = wanted.unwrap_or_default();
        return Err(Stop::usage(format!(
            "curlwright: {} open tabs are on {origin} — pass --tab to pick one.\n{}\n",
            same.len(),
            format_candidates(&same, full_urls)
        )));
    }
    let reason = match &wanted {
        Some(origin) => format!("no open tab is on {origin}"),
        None => "a relative URL needs an explicit tab".to_string(),
    };
    let refs: Vec<&Page> = pages.iter().collect();
    Err(Stop::usage(format!(
        "curlwright: {reason} — pass --tab.\n{}\n",
        format_candidates(&refs, full_urls)
    )))
}

pub fn effective_url(request_url: &str, page_url: Option<&str>) -> Option<String> {
    if cdp_client::sanitize::is_absolute_url(request_url) {
        return Some(request_url.to_string());
    }
    let base = page_url?;
    cdp_client::sanitize::join_url(base, request_url)
}

pub fn resolve_tab(cdp: &mut Cdp, url: &str, explicit: Option<&str>) -> Result<String, Stop> {
    let (target, _) = resolve_tab_with_url(cdp, url, explicit, false)?;
    Ok(target)
}

pub fn resolve_tab_with_url(cdp: &mut Cdp, url: &str, explicit: Option<&str>, full_urls: bool) -> Result<(String, Option<String>), Stop> {
    if let Some(tab) = explicit.filter(|tab| !tab.is_empty()) {
        let pages = list_pages(cdp).unwrap_or_default();
        let page_url = pages.iter().find(|p| p.target_id == tab).map(|p| p.url.clone());
        return Ok((tab.to_string(), page_url));
    }
    let pages = list_pages(cdp).map_err(|err| {
        Stop::new(format!("curlwright: cannot list tabs: {}", err.trim_end_matches('\n')), 2)
    })?;
    let target = choose_tab(&pages, url, None, session_current().as_deref(), full_urls)?;
    let page_url = pages.iter().find(|p| p.target_id == target).map(|p| p.url.clone());
    Ok((target, page_url))
}

#[cfg(test)]
mod tests {
    use super::{choose_tab, current_target, Page};

    fn page(id: &str, url: &str) -> Page {
        Page { target_id: id.to_string(), url: url.to_string() }
    }

    #[test]
    fn session_current_wins_after_an_explicit_tab() {
        let pages = vec![
            page("A", "https://app.example/home"),
            page("B", "https://other.example/"),
        ];
        let chosen = choose_tab(&pages, "https://app.example/api", Some("B"), Some("A"), false).unwrap();
        assert_eq!(chosen, "B");
        let current = choose_tab(&pages, "https://app.example/api", None, Some("B"), false).unwrap();
        assert_eq!(current, "B");
    }

    #[test]
    fn one_same_origin_tab_is_used_and_a_foreign_tab_is_not() {
        let foreign = vec![page("ONLY", "https://other.example/dash")];
        let missed = choose_tab(&foreign, "https://app.example/api", None, None, false).unwrap_err();
        assert_eq!(missed.code, 2);
        assert!(missed.message.contains("--tab=ONLY"));
        assert!(missed.message.contains("no open tab is on https://app.example"));
        let same = vec![page("ONLY", "https://app.example/home")];
        assert_eq!(choose_tab(&same, "https://app.example/api", None, None, false).unwrap(), "ONLY");
    }

    #[test]
    fn a_stale_session_falls_through_to_the_origin_tab() {
        let pages = vec![page("ONLY", "https://app.example/home")];
        assert_eq!(choose_tab(&pages, "https://app.example/api", None, Some("GONE"), false).unwrap(), "ONLY");
        assert_eq!(current_target(r#"{"current":"ABC"}"#).as_deref(), Some("ABC"));
        assert_eq!(current_target("{}"), None);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_cwd_session_file_does_not_supply_the_current_tab() {
        let cwd = std::env::current_dir().unwrap();
        let canonical = cwd.canonicalize().unwrap_or(cwd);
        let planted = canonical.join(".playwright-cli").join("session.json");
        let _ = std::fs::create_dir_all(planted.parent().unwrap());
        std::fs::write(
            &planted,
            r#"{"current":"PLANTED","endpoint":"https://evil.example/json/version"}"#,
        )
        .unwrap();
        let stored = super::session_path(&super::config_root().unwrap(), &canonical).unwrap();
        if let Some(parent) = stored.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(
            &stored,
            r#"{"current":"KEPT","endpoint":"https://evil.example/json/version"}"#,
        )
        .unwrap();
        assert_eq!(super::session_current().as_deref(), Some("KEPT"));
        assert_ne!(stored, planted);
        let _ = std::fs::remove_file(&planted);
        let _ = std::fs::remove_file(&stored);
        let _ = std::fs::remove_dir(planted.parent().unwrap());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_relative_config_dir_is_not_a_session_file() {
        assert!(super::session_path(".config", std::path::Path::new("work")).is_none());
        assert!(super::session_path("AppData\\Roaming", std::path::Path::new("work")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_directories_do_not_share_a_session_file() {
        use std::os::unix::ffi::OsStrExt;
        let left = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/work/\xff"));
        let right = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/work/\xfe"));
        assert_eq!(left.to_string_lossy(), right.to_string_lossy());
        let root = std::env::temp_dir();
        let root = root.to_str().unwrap();
        let a = super::session_path(root, left).unwrap();
        let b = super::session_path(root, right).unwrap();
        assert_ne!(a, b);
    }
}
