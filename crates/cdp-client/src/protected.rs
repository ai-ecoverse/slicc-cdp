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

    pub fn url(&self, text: &str) -> bool {
        let Some(url) = self.unwrap(parse(text)) else {
            return false;
        };
        let scheme = url.scheme();
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

    pub fn storage_key(&self, key: &str) -> bool {
        let mut parts = key.split('^');
        let first = parts.next().unwrap_or("");
        if self.url(first) {
            return true;
        }
        parts
            .filter_map(|part| part.strip_prefix('0'))
            .any(|site| self.url(site))
    }

    pub fn pattern(&self, pattern: &str) -> bool {
        let lowered = pattern.to_ascii_lowercase();
        let names = self.rules.domains.iter().chain(self.rules.hosts.iter());
        for name in names {
            if lowered.contains(name.as_str()) {
                return true;
            }
        }
        if self
            .rules
            .any_schemes
            .iter()
            .any(|scheme| lowered.starts_with(&format!("{scheme}:")))
        {
            return true;
        }
        let literal: String = pattern
            .chars()
            .take_while(|ch| !matches!(ch, '*' | '?' | '{' | '}' | '[' | ']' | '\\'))
            .collect();
        literal.contains("://") && self.url(&literal)
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

    fn unwrap(&self, mut url: Option<Url>) -> Option<Url> {
        let mut depth = 0;
        while let Some(current) = url.as_ref() {
            if depth >= 8
                || !self
                    .rules
                    .inner_schemes
                    .iter()
                    .any(|item| item == current.scheme())
            {
                break;
            }
            let rest = current.as_str()[current.scheme().len() + 1..].to_string();
            url = parse(&rest);
            depth += 1;
        }
        url
    }
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
                guard.target(&vector.url, vector.previous, opener),
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
            "*",
            "**/*",
            "**/api/users",
            "https://example.com/*",
            "https://vault-sliccy.example/*",
        ] {
            assert!(!guard.pattern(pattern), "{pattern}");
        }
        assert_eq!(
            guard.refusal(),
            "slicc: refused: target is on seven's origin"
        );
    }
}
