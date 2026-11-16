use crate::args::Invocation;
use cdp_client::protected::Protected;
use url::Url;

pub(crate) enum Scope {
    Tab(String),
    Host(String),
    Match(Vec<String>),
}

impl Scope {
    pub(crate) fn parse(invocation: &Invocation) -> Result<Self, String> {
        let tab = invocation.flags.get("tab");
        let host = invocation.flags.get("host");
        let patterns = invocation.repeated("match");
        if usize::from(tab.is_some()) + usize::from(host.is_some()) + usize::from(!patterns.is_empty()) != 1 {
            return Err("eval-attach requires exactly one of --tab, --host, or --match (repeatable)\n".into());
        }
        if let Some(tab) = tab {
            if tab.is_empty() { return Err("eval-attach: --tab must not be empty\n".into()) }
            return Ok(Self::Tab(tab.clone()));
        }
        let guard = Protected::default();
        if let Some(host) = host {
            let host = host.to_ascii_lowercase();
            let name = host.strip_prefix("*.").unwrap_or(&host);
            let canonical = cdp_client::protected::canonical_cookie_domain(name);
            if canonical.as_deref() != Some(name) || name.starts_with('.') || name.contains(':') {
                return Err("eval-attach: --host must be an exact host or *.example.com\n".into());
            }
            if guard.pattern(&format!("https://{host}/*")) { return Err("eval-attach: scope names a protected origin\n".into()) }
            return Ok(Self::Host(host));
        }
        for pattern in &patterns {
            if guard.pattern(pattern) { return Err("eval-attach: scope names a protected origin\n".into()) }
            let Some((scheme, rest)) = pattern.split_once("://") else { return Err("eval-attach: --match requires scheme://host/path with * wildcards\n".into()) };
            let Some((host, _)) = rest.split_once('/') else { return Err("eval-attach: --match requires a path\n".into()) };
            let name = host.strip_prefix("*.").unwrap_or(host);
            if !matches!(scheme, "*" | "http" | "https") || host != "*" && cdp_client::protected::canonical_cookie_domain(name).as_deref() != Some(name)
                || name.starts_with('.') || name.contains(':') || pattern.contains(['#', '\\', '{', '}']) {
                return Err("eval-attach: invalid --match; use http, https or *, an exact host, * or *.example.com, and a path glob\n".into());
            }
        }
        Ok(Self::Match(patterns.into_iter().map(str::to_string).collect()))
    }

    pub(crate) fn matches(&self, tab: &str, text: &str) -> bool {
        if let Self::Tab(id) = self { return id == tab }
        let Ok(url) = Url::parse(text) else { return false };
        if !matches!(url.scheme(), "http" | "https") { return false }
        let host = url.host_str().unwrap_or("").trim_end_matches('.');
        match self {
            Self::Host(pattern) => host_matches(pattern, host),
            Self::Match(patterns) => patterns.iter().any(|pattern| {
                let (scheme, rest) = pattern.split_once("://").unwrap();
                let (pattern_host, path) = rest.split_once('/').unwrap();
                if scheme != "*" && scheme != url.scheme() || !host_matches(pattern_host, host) { return false }
                wildcard(&format!("/{path}"), &format!("{}{}", url.path(), url.query().map(|q| format!("?{q}")).unwrap_or_default()))
            }),
            Self::Tab(_) => false,
        }
    }
}

fn host_matches(pattern: &str, host: &str) -> bool {
    pattern == "*" || pattern == host || pattern.strip_prefix("*.").is_some_and(|name| host == name || host.ends_with(&format!(".{name}")))
}

fn wildcard(pattern: &str, text: &str) -> bool {
    let mut rest = text;
    let parts: Vec<_> = pattern.split('*').collect();
    if !rest.starts_with(parts[0]) { return false }
    rest = &rest[parts[0].len()..];
    for (index, part) in parts.iter().enumerate().skip(1) {
        if index == parts.len() - 1 { return rest.ends_with(part) }
        let Some(at) = rest.find(part) else { return false };
        rest = &rest[at + part.len()..];
    }
    rest.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_match_hosts_and_paths_without_crossing_authorities() {
        for (flags, hits, misses) in [
            (vec!["--match", "https://example.com/search?q=*"], vec!["https://example.com/search?q=a#fragment"], vec!["https://example.com/search?other=a"]),
            (vec!["--host", "*.example.com"], vec!["https://example.com/a", "https://x.example.com/a"], vec!["https://example.com.evil/a", "https://other.test/"]),
            (vec!["--match", "*://*.example.com/a*", "--match", "https://other.test/*"], vec!["https://x.example.com/a/b", "http://example.com/a?q=1", "https://other.test/x"], vec!["https://example.com/b", "http://other.test/x"]),
        ] {
            let args = [vec!["eval-attach", "shim.js"], flags].concat().into_iter().map(str::to_string).collect::<Vec<_>>();
            let scope = Scope::parse(&crate::args::parse(&args).unwrap()).unwrap();
            for url in hits { assert!(scope.matches("TAB", url), "{url}"); }
            for url in misses { assert!(!scope.matches("TAB", url), "{url}"); }
        }
    }

    #[test]
    fn protected_and_invalid_scopes_fail_precheck() {
        for flags in [vec!["--host", "*.sliccy.ai"], vec!["--match", "https://vault.sliccy.ai/*"], vec!["--tab", "T", "--host", "example.com"], vec!["--host", "a/b"], vec!["--match", "https://exa*mple.com/*"]] {
            let args = [vec!["eval-attach", "shim.js"], flags].concat().into_iter().map(str::to_string).collect::<Vec<_>>();
            assert!(Scope::parse(&crate::args::parse(&args).unwrap()).is_err());
        }
    }
}
