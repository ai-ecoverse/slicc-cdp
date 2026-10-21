use slicc_browsers::ProtectedOrigins;
use std::sync::OnceLock;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opener {
    None,
    Protected,
    Unprotected,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Protected {
    rules: ProtectedOrigins,
    extra: Vec<String>,
    extra_hosts: Vec<String>,
}

fn catalog_rules() -> &'static ProtectedOrigins {
    static RULES: OnceLock<ProtectedOrigins> = OnceLock::new();
    RULES.get_or_init(|| slicc_browsers::catalog().protected_origins)
}

pub fn refusal() -> &'static str {
    &catalog_rules().refusal
}

impl Default for Protected {
    fn default() -> Self {
        Self::new(&[])
    }
}

impl Protected {
    pub fn new(extra_origins: &[&str]) -> Self {
        Self::with_rules(catalog_rules().clone(), extra_origins)
    }

    pub fn with_rules(rules: ProtectedOrigins, extra_origins: &[&str]) -> Self {
        let parsed: Vec<Url> = extra_origins
            .iter()
            .filter_map(|text| parse(text))
            .collect();
        let extra = parsed.iter().filter_map(origin_of).collect();
        let extra_hosts = parsed
            .iter()
            .filter(|url| url.host_str().is_some_and(|host| !host.is_empty()))
            .map(host_of)
            .collect();
        Self {
            rules,
            extra,
            extra_hosts,
        }
    }

    pub fn refusal(&self) -> &str {
        &self.rules.refusal
    }

    pub fn refusal_for_url(&self, text: &str) -> &str {
        if self.rules.unparseable == "protected" && self.unparseable(text) {
            &self.rules.unparseable_refusal
        } else {
            self.refusal()
        }
    }

    pub fn unparseable(&self, text: &str) -> bool {
        !matches!(text, "" | "null" | "://") && self.unwrap(parse(text)).is_none()
    }

    pub fn url(&self, text: &str) -> bool {
        self.judge_url(text, false)
    }

    pub fn redaction_schemes(&self) -> impl Iterator<Item = &str> {
        self.rules.schemes.iter()
            .chain(&self.rules.protected_schemes)
            .chain(&self.rules.any_schemes)
            .chain(&self.rules.inner_schemes)
            .map(String::as_str)
            .chain(self.extra.iter().filter_map(|origin| origin.split_once(':').map(|(scheme, _)| scheme)))
    }

    pub fn url_read_only(&self, text: &str) -> bool {
        self.judge_url(text, true)
    }

    fn judge_url(&self, text: &str, read_only: bool) -> bool {
        let Some(url) = self.unwrap(parse(text)).map(|url| self.rewrite_about(url)) else {
            return !read_only && self.rules.unparseable == "protected" && !matches!(text, "" | "null" | "://");
        };
        let scheme = url.scheme();
        if self
            .rules
            .protected_schemes
            .iter()
            .any(|item| item == scheme)
        {
            let pair = format!("{scheme}://{}", host_of(&url));
            return !self.rules.scheme_exceptions.contains(&pair);
        }
        if self.rules.any_schemes.iter().any(|item| item == scheme) {
            return true;
        }
        if self.rules.schemes.iter().any(|item| item == scheme) {
            let host = host_of(&url);
            if self.rules.hosts.contains(&host) {
                return true;
            }
            if self.rules.domains.iter().any(|domain| under(&host, domain)) {
                return true;
            }
        }
        origin_of(&url).is_some_and(|origin| self.extra.contains(&origin))
    }

    pub fn target_type(&self, kind: &str) -> bool {
        self.rules.target_types.iter().any(|item| item == kind)
    }

    pub fn target(&self, text: &str, previous: bool, opener: Opener) -> bool {
        if self.inheriting(text) {
            return previous || matches!(opener, Opener::Protected | Opener::Unknown);
        }
        self.url(text)
    }

    pub fn cookie_domain(&self, domain: &str) -> bool {
        let mut domain = domain.to_ascii_lowercase();
        if let Some(rest) = domain.strip_prefix('.') {
            domain = rest.to_string();
        }
        if let Some(rest) = domain.strip_suffix('.') {
            domain = rest.to_string();
        }
        if domain.is_empty() {
            return false;
        }
        if self.rules.domains.iter().any(|item| under(&domain, item)) {
            return true;
        }
        self.rules
            .domains
            .iter()
            .chain(self.rules.hosts.iter())
            .chain(self.extra_hosts.iter())
            .any(|host| under(host, &domain))
    }

    pub fn cookie_scope(&self, domain: &str) -> bool {
        canonical_cookie_domain(domain).is_none_or(|host| self.cookie_domain(&host))
    }

    pub fn cookie_url(&self, text: &str) -> bool {
        let Some(url) = self.unwrap(parse(text)) else {
            return true;
        };
        let host = host_of(&url);
        if host.is_empty() || self.url(text) {
            return true;
        }
        if !matches!(url.host(), Some(url::Host::Domain(_))) {
            return self.cookie_domain(&host);
        }
        let labels: Vec<&str> = host.split('.').collect();
        (0..labels.len()).any(|at| self.cookie_scope(&labels[at..].join(".")))
    }

    pub fn storage_key(&self, key: &str) -> bool {
        let mut parts = key.split('^');
        let first = parts.next().unwrap_or("");
        if self.url_read_only(first) {
            return true;
        }
        parts
            .filter_map(|part| part.strip_prefix('0'))
            .any(|site| self.url_read_only(site))
    }

    pub fn pattern(&self, pattern: &str) -> bool {
        let lowered = pattern.to_ascii_lowercase();
        if self
            .rules
            .any_schemes
            .iter()
            .any(|scheme| lowered.starts_with(&format!("{scheme}:")))
        {
            return true;
        }
        let names: Vec<&String> = self
            .rules
            .domains
            .iter()
            .chain(self.rules.hosts.iter())
            .chain(self.extra_hosts.iter())
            .collect();
        if pattern_hosts(&lowered)
            .iter()
            .any(|host| names.iter().any(|name| host_reaches(host, name)))
        {
            return true;
        }
        let literal: String = pattern
            .chars()
            .take_while(|ch| !matches!(ch, '*' | '?' | '{' | '}' | '[' | ']' | '\\'))
            .collect();
        literal.contains("://") && self.url_read_only(&literal)
    }

    fn inheriting(&self, text: &str) -> bool {
        if text.is_empty() {
            return true;
        }
        let Some(url) = parse(text) else {
            return false;
        };
        url.scheme() == "about"
            && self
                .rules
                .inheriting
                .iter()
                .any(|item| *item == format!("about:{}", url.path().to_ascii_lowercase()))
    }

    fn rewrite_about(&self, url: Url) -> Url {
        if url.scheme() != "about"
            || self.rules.about_scheme.is_empty()
            || self.inheriting(url.as_str())
        {
            return url;
        }
        let rest = &url.as_str()[url.scheme().len() + 1..];
        parse(&format!("{}://{rest}", self.rules.about_scheme)).unwrap_or(url)
    }

    fn unwrap(&self, mut url: Option<Url>) -> Option<Url> {
        let mut depth = 0;
        while let Some(current) = url.as_ref() {
            if !self
                .rules
                .inner_schemes
                .iter()
                .any(|item| item == current.scheme())
            {
                break;
            }
            if depth >= 8 {
                return None;
            }
            let rest = current.as_str()[current.scheme().len() + 1..].to_string();
            url = parse(&rest);
            depth += 1;
        }
        url
    }
}

fn pattern_hosts(pattern: &str) -> Vec<String> {
    if let Some(index) = pattern.find("://") {
        let scheme = &pattern[..index];
        if !scheme.is_empty()
            && scheme.chars().all(|ch| {
                ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.' | '*' | '{' | '}' | ',')
            })
        {
            let rest = &pattern[index + 3..];
            let end = rest.find(['/', '#', '?']).unwrap_or(rest.len());
            return authority_host(&rest[..end]).into_iter().collect();
        }
    }
    let end = pattern.find(['#', '?']).unwrap_or(pattern.len());
    pattern[..end]
        .split('/')
        .filter_map(authority_host)
        .filter(|host| host.contains('.'))
        .collect()
}

fn authority_host(authority: &str) -> Option<String> {
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = match host.rfind(':') {
        Some(index)
            if !host.starts_with('[')
                && host[index + 1..]
                    .chars()
                    .all(|ch| ch.is_ascii_digit() || ch == '*') =>
        {
            &host[..index]
        }
        _ => host,
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty()
        || host
            .chars()
            .all(|ch| matches!(ch, '*' | '?' | '{' | '}' | ','))
    {
        return None;
    }
    Some(host.to_string())
}

fn host_reaches(glob: &str, name: &str) -> bool {
    let first: String = glob
        .split('.')
        .next()
        .unwrap_or("")
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .collect();
    let label = if first.is_empty() {
        "x".to_string()
    } else {
        first
    };
    [
        name.to_string(),
        format!("x.{name}"),
        format!("{label}.{name}"),
    ]
    .iter()
    .any(|candidate| host_glob(glob.as_bytes(), candidate.as_bytes()))
}

fn host_glob(glob: &[u8], text: &[u8]) -> bool {
    let Some((&first, rest)) = glob.split_first() else {
        return text.is_empty();
    };
    match first {
        b'*' => (0..=text.len()).any(|taken| host_glob(rest, &text[taken..])),
        b'{' => match rest.iter().position(|ch| *ch == b'}') {
            Some(close) => {
                (0..=text.len()).any(|taken| host_glob(&rest[close + 1..], &text[taken..]))
            }
            None => text.first() == Some(&b'{') && host_glob(rest, &text[1..]),
        },
        b'?' => !text.is_empty() && host_glob(rest, &text[1..]),
        b'\\' => match rest.split_first() {
            Some((&literal, after)) => {
                text.first() == Some(&literal) && host_glob(after, &text[1..])
            }
            None => text.is_empty(),
        },
        literal => text.first() == Some(&literal) && host_glob(rest, &text[1..]),
    }
}

pub fn canonical_cookie_domain(domain: &str) -> Option<String> {
    if domain.is_empty() || domain.chars().any(|ch| ch.is_whitespace() || matches!(ch, '/' | '\\' | '@' | '?' | '#')) {
        return None;
    }
    let normalized = domain.replace(['。', '．', '｡'], ".");
    let normalized = normalized.trim_start_matches('.');
    let url = parse(&format!("http://{normalized}/"))?;
    let host = host_of(&url).trim_matches('.').to_string();
    if matches!(url.host(), Some(url::Host::Domain(_))) && host.split('.').any(|label| {
        label.is_empty() || label.starts_with('-') || label.ends_with('-')
            || !label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }) {
        return None;
    }
    Some(host)
}

fn parse(text: &str) -> Option<Url> {
    Url::parse(text).ok()
}

fn host_of(url: &Url) -> String {
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    match host.strip_suffix('.') {
        Some(stripped) => stripped.to_string(),
        None => host,
    }
}

fn origin_of(url: &Url) -> Option<String> {
    url.host_str().filter(|host| !host.is_empty())?;
    let host = host_of(url);
    Some(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

fn under(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

#[cfg(test)]
mod tests {
    use super::{Opener, Protected};
    use slicc_browsers::protected::{
        vectors, OPENER_PROTECTED, OPENER_UNKNOWN, OPENER_UNPROTECTED,
    };

    fn guard() -> Protected {
        let vectors = vectors();
        let origins: Vec<&str> = vectors.origins.iter().map(String::as_str).collect();
        Protected::new(&origins)
    }

    #[test]
    fn unparseable_policy_and_refusal_are_catalog_driven() {
        let mut rules = slicc_browsers::catalog().protected_origins;
        rules.unparseable_refusal = "custom unparseable refusal".into();
        let guard = Protected::with_rules(rules.clone(), &[]);
        for text in ["https://exa%20mple.example/", "blob:invalid", "view-source:invalid"] {
            assert!(guard.unparseable(text));
            assert!(guard.url(text));
            assert!(!guard.url_read_only(text));
            assert!(!guard.storage_key(text));
            assert_eq!(guard.refusal_for_url(text), "custom unparseable refusal");
        }
        for text in ["", "null", "://"] {
            assert!(!guard.unparseable(text));
            assert!(!guard.url(text));
        }
        let at_limit = format!("{}https://example.test/", "view-source:".repeat(8));
        assert!(!guard.unparseable(&at_limit));
        assert!(!guard.url(&at_limit));
        let over_limit = format!("view-source:{at_limit}");
        assert!(guard.unparseable(&over_limit));
        assert!(guard.url(&over_limit));
        rules.unparseable = "not protected".into();
        let guard = Protected::with_rules(rules, &[]);
        assert!(!guard.url("https://exa%20mple.example/"));
        assert_eq!(guard.refusal_for_url("https://exa%20mple.example/"), guard.refusal());
        for url in ["http://xn--1ug.sliccy.ai/", "http://xn--a.sliccy.ai/"] {
            let guard = Protected::default();
            assert!(guard.url(url));
            assert!(guard.unparseable(url));
        }
    }

    #[test]
    fn privileged_rules_are_catalog_driven() {
        let mut rules = slicc_browsers::catalog().protected_origins;
        rules.protected_schemes = vec!["privileged".into()];
        rules.scheme_exceptions = vec!["privileged://newtab".into()];
        rules.about_scheme = "privileged".into();
        rules.target_types = vec!["custom_ui".into()];
        let guard = Protected::with_rules(rules.clone(), &[]);
        for url in ["privileged://settings", "about:settings", "view-source:about:settings", "blob:privileged://settings/id", "filesystem:privileged://settings/temporary/x"] {
            assert!(guard.url(url), "{url}");
        }
        for url in ["privileged://newtab/path", "about:newtab/path", "about:blank", "about:srcdoc", "chrome://settings"] {
            assert!(!guard.url(url), "{url}");
        }
        assert!(guard.target_type("custom_ui"));
        assert!(!guard.target_type("browser_ui"));
        rules.about_scheme.clear();
        assert!(!Protected::with_rules(rules, &[]).url("about:settings"));
    }

    #[test]
    fn every_url_vector_matches_slicc_browsers() {
        let guard = guard();
        for vector in vectors().urls {
            assert_eq!(guard.url(&vector.url), vector.protected, "{:?}", vector.url);
        }
    }

    #[test]
    fn every_target_vector_matches_slicc_browsers() {
        let guard = guard();
        for vector in vectors().targets {
            let opener = match vector.opener.as_deref() {
                None => Opener::None,
                Some(OPENER_PROTECTED) => Opener::Protected,
                Some(OPENER_UNPROTECTED) => Opener::Unprotected,
                Some(OPENER_UNKNOWN) => Opener::Unknown,
                Some(other) => panic!("unknown opener {other}"),
            };
            assert_eq!(
                guard.target_type(vector.kind.as_deref().unwrap_or(""))
                    || guard.target(&vector.url, vector.previous, opener),
                vector.protected,
                "{vector:?}"
            );
        }
    }

    #[test]
    fn every_cookie_and_storage_vector_matches_slicc_browsers() {
        let guard = guard();
        let all = vectors();
        for vector in all.cookie_domains {
            assert_eq!(
                guard.cookie_domain(&vector.domain),
                vector.protected,
                "{vector:?}"
            );
        }
        for vector in all.storage_keys {
            assert_eq!(
                guard.storage_key(&vector.key),
                vector.protected,
                "{vector:?}"
            );
        }
    }

    #[test]
    fn patterns_that_name_seven_or_the_vault_are_protected() {
        let guard = Protected::default();
        for pattern in [
            "https://vault.sliccy.ai/*",
            "**://*.sliccy.ai/**",
            "*sliccy.ai*",
            "https://SEVEN.SLICCY.AI/api/*",
            "https://www.sliccy.ai/auth/callback*",
            "devtools://*",
            "https://sliccy.ai/",
        ] {
            assert!(guard.pattern(pattern), "{pattern}");
        }
        for pattern in [
            "**://*.sliccy.ai/**",
            "**/*.sliccy.ai/**",
            "https://seven.sliccy.*/x",
            "https://user@vault.sliccy.ai:443/x",
            "https://vault.sliccy.ai./x",
            "*://{seven,other}.sliccy.ai/*",
        ] {
            assert!(guard.pattern(pattern), "{pattern}");
        }
        for pattern in [
            "*",
            "**/*",
            "**/api/users",
            "https://*/api",
            "https://example.com/*",
            "https://vault-sliccy.example/*",
            "https://notsliccy.ai/*",
            "https://vault.sliccy.ai.example/*",
            "https://vault-sliccy.ai/*",
            "**/x?next=https://seven.sliccy.ai/",
            "https://example.com/?next=https://seven.sliccy.ai/",
            "*/blocked*",
            "**/api/*/details",
        ] {
            assert!(!guard.pattern(pattern), "{pattern}");
        }
        assert_eq!(
            guard.refusal(),
            "slicc: refused: target is on seven's origin"
        );
    }
}
