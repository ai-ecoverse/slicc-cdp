use crate::parse::Stop;
use cdp_client::cdp::Cdp;
use serde_json::{json, Value};
use std::fs;

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
        if target_id.is_empty() || is_internal(&url, &title) {
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

fn format_candidates(pages: &[&Page]) -> String {
    pages.iter().map(|page| format!("  --tab={}  {}", page.target_id, page.url)).collect::<Vec<_>>().join("\n")
}

fn session_current() -> Option<String> {
    let text = fs::read_to_string(".playwright-cli/session.json").ok()?;
    current_target(&text)
}

pub fn current_target(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    value.get("current").and_then(|item| item.as_str()).filter(|tab| !tab.is_empty()).map(str::to_string)
}

pub fn choose_tab(pages: &[Page], url: &str, explicit: Option<&str>, current: Option<&str>) -> Result<String, Stop> {
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
            format_candidates(&same)
        )));
    }
    let reason = match &wanted {
        Some(origin) => format!("no open tab is on {origin}"),
        None => "a relative URL needs an explicit tab".to_string(),
    };
    let refs: Vec<&Page> = pages.iter().collect();
    Err(Stop::usage(format!(
        "curlwright: {reason} — pass --tab.\n{}\n",
        format_candidates(&refs)
    )))
}

pub fn resolve_tab(cdp: &mut Cdp, url: &str, explicit: Option<&str>) -> Result<String, Stop> {
    if let Some(tab) = explicit.filter(|tab| !tab.is_empty()) {
        return Ok(tab.to_string());
    }
    let pages = list_pages(cdp).map_err(|err| {
        Stop::new(format!("curlwright: cannot list tabs: {}", err.trim_end_matches('\n')), 2)
    })?;
    choose_tab(&pages, url, None, session_current().as_deref())
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
        let chosen = choose_tab(&pages, "https://app.example/api", Some("B"), Some("A")).unwrap();
        assert_eq!(chosen, "B");
        let current = choose_tab(&pages, "https://app.example/api", None, Some("B")).unwrap();
        assert_eq!(current, "B");
    }

    #[test]
    fn one_same_origin_tab_is_used_and_a_foreign_tab_is_not() {
        let foreign = vec![page("ONLY", "https://other.example/dash")];
        let missed = choose_tab(&foreign, "https://app.example/api", None, None).unwrap_err();
        assert_eq!(missed.code, 2);
        assert!(missed.message.contains("--tab=ONLY"));
        assert!(missed.message.contains("no open tab is on https://app.example"));
        let same = vec![page("ONLY", "https://app.example/home")];
        assert_eq!(choose_tab(&same, "https://app.example/api", None, None).unwrap(), "ONLY");
    }

    #[test]
    fn a_stale_session_falls_through_to_the_origin_tab() {
        let pages = vec![page("ONLY", "https://app.example/home")];
        assert_eq!(choose_tab(&pages, "https://app.example/api", None, Some("GONE")).unwrap(), "ONLY");
        assert_eq!(current_target(r#"{"current":"ABC"}"#).as_deref(), Some("ABC"));
        assert_eq!(current_target("{}"), None);
    }
}
