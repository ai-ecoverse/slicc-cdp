use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Probe {
    pub standard: String,
    pub path: String,
    pub accept: String,
    pub content_type_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRequest {
    pub probe: Probe,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeMatch {
    pub probe: Probe,
    pub url: String,
    pub body: String,
}

pub struct ProbeRegistry {
    probes: Vec<Probe>,
}

impl ProbeRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self { probes: Vec::new() }
    }

    pub fn register(&mut self, probe: Probe) {
        self.probes.push(probe);
    }

    #[must_use]
    pub fn probes_for_origin(&self, origin: &str) -> Vec<ProbeRequest> {
        self.probes
            .iter()
            .map(|p| {
                let url = format!("{}{}", origin.trim_end_matches('/'), p.path);
                ProbeRequest {
                    probe: p.clone(),
                    url,
                }
            })
            .collect()
    }

    #[must_use]
    pub fn check_response(
        &self,
        standard: &str,
        status: u16,
        content_type: &str,
        body: String,
        url: String,
    ) -> Option<ProbeMatch> {
        if status != 200 {
            return None;
        }
        let probe = self.probes.iter().find(|p| p.standard == standard)?;
        if is_html_content_type(content_type) {
            return None;
        }
        let ct_lower = content_type.to_ascii_lowercase();
        let prefix_lower = probe.content_type_prefix.to_ascii_lowercase();
        if !ct_lower.starts_with(&prefix_lower) {
            return None;
        }
        Some(ProbeMatch {
            probe: probe.clone(),
            url,
            body,
        })
    }
}

impl Default for ProbeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn is_html_content_type(ct: &str) -> bool {
    let lower = ct.to_ascii_lowercase();
    let base = lower.split(';').next().unwrap_or("").trim();
    base == "text/html" || base == "application/xhtml+xml"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> ProbeRegistry {
        let mut r = ProbeRegistry::new();
        r.register(Probe {
            standard: "ard".into(),
            path: "/.well-known/ard.json".into(),
            accept: "application/json".into(),
            content_type_prefix: "application/json".into(),
        });
        r.register(Probe {
            standard: "ai-catalog".into(),
            path: "/.well-known/ai-catalog.json".into(),
            accept: "application/json".into(),
            content_type_prefix: "application/json".into(),
        });
        r
    }

    #[test]
    fn probes_for_origin_builds_urls() {
        let r = registry();
        let probes = r.probes_for_origin("https://example.com");
        assert_eq!(probes.len(), 2);
        assert_eq!(probes[0].url, "https://example.com/.well-known/ard.json");
        assert_eq!(probes[0].probe.accept, "application/json");
        assert_eq!(probes[0].probe.standard, "ard");
    }

    #[test]
    fn trailing_slash_origin() {
        let r = registry();
        let probes = r.probes_for_origin("https://example.com/");
        assert_eq!(probes[0].url, "https://example.com/.well-known/ard.json");
    }

    #[test]
    fn probes_expose_accept_header() {
        let r = registry();
        let probes = r.probes_for_origin("https://example.com");
        for req in &probes {
            assert!(!req.probe.accept.is_empty());
        }
    }

    #[test]
    fn exact_200_json_matches() {
        let r = registry();
        let m = r.check_response("ard", 200, "application/json", "{}".into(), "https://example.com/.well-known/ard.json".into());
        assert!(m.is_some());
        let m = m.unwrap();
        assert_eq!(m.probe.standard, "ard");
        assert_eq!(m.body, "{}");
    }

    #[test]
    fn json_charset_matches() {
        let r = registry();
        let m = r.check_response("ard", 200, "application/json; charset=utf-8", "{}".into(), "u".into());
        assert!(m.is_some());
    }

    #[test]
    fn redirect_301_does_not_match() {
        let r = registry();
        let m = r.check_response("ard", 301, "application/json", "{}".into(), "u".into());
        assert!(m.is_none());
    }

    #[test]
    fn status_204_does_not_match() {
        let r = registry();
        let m = r.check_response("ard", 204, "application/json", "".into(), "u".into());
        assert!(m.is_none());
    }

    #[test]
    fn status_206_does_not_match() {
        let r = registry();
        let m = r.check_response("ard", 206, "application/json", "{}".into(), "u".into());
        assert!(m.is_none());
    }

    #[test]
    fn html_content_type_does_not_match() {
        let r = registry();
        let m = r.check_response("ard", 200, "text/html", "<html>".into(), "u".into());
        assert!(m.is_none());
    }

    #[test]
    fn xhtml_does_not_match() {
        let r = registry();
        let m = r.check_response("ard", 200, "application/xhtml+xml", "<html>".into(), "u".into());
        assert!(m.is_none());
    }

    #[test]
    fn wrong_content_type_does_not_match() {
        let r = registry();
        let m = r.check_response("ard", 200, "text/plain", "hello".into(), "u".into());
        assert!(m.is_none());
    }

    #[test]
    fn unknown_standard_returns_none() {
        let r = registry();
        let m = r.check_response("unknown", 200, "application/json", "{}".into(), "u".into());
        assert!(m.is_none());
    }
}
