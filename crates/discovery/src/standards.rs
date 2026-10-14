use crate::classifier::{Classification, ClassifierRegistry, Validity};
use crate::probe::{Probe, ProbeRegistry};
use serde_json::Value;

pub const HANDOFF_REL: &str = "https://www.sliccy.ai/rel/handoff";
pub const UPSKILL_REL: &str = "https://www.sliccy.ai/rel/upskill";
pub const LLMS_TXT_REL: &str = "https://llmstxt.org/rel/llms-txt";

const TITLE_CAP: usize = 200;
const HEADINGS_CAP: usize = 50;
const HEADING_LEN_CAP: usize = 200;

pub fn known_link_rels() -> &'static [&'static str] {
    &[
        "ard",
        "ai-catalog",
        "llms-txt",
        "api-catalog",
        "ucp",
        "describedby",
        "service-desc",
        "service-meta",
        "status",
        "service-doc",
        LLMS_TXT_REL,
        HANDOFF_REL,
        UPSKILL_REL,
    ]
}

pub fn default_probes() -> ProbeRegistry {
    let mut registry = ProbeRegistry::new();
    registry.register(Probe {
        standard: "ard".into(),
        path: "/.well-known/ard.json".into(),
        accept: "application/json".into(),
        content_type_prefix: "application/json".into(),
    });
    registry.register(Probe {
        standard: "api-catalog".into(),
        path: "/.well-known/api-catalog".into(),
        accept: "application/linkset+json, application/json;q=0.9".into(),
        content_type_prefix: "application/".into(),
    });
    registry.register(Probe {
        standard: "llms-txt".into(),
        path: "/llms.txt".into(),
        accept: "text/plain, text/markdown".into(),
        content_type_prefix: "text/".into(),
    });
    registry
}

pub fn default_classifiers() -> ClassifierRegistry {
    let mut registry = ClassifierRegistry::new();
    registry.register("ard", classify_ard);
    registry.register("api-catalog", classify_api_catalog);
    registry.register("llms-txt", classify_llms_txt);
    registry
}

pub fn default_llmstxtignore_entries() -> &'static [&'static str] {
    &["github.com", "app.slack.com"]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseSignal {
    pub standard: String,
    pub summary: Value,
}

pub fn classify_response(status: u16, headers: &[(String, String)]) -> Vec<ResponseSignal> {
    let mut signals = Vec::new();
    if status == 402 {
        let challenge = headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("x-payment") || name.eq_ignore_ascii_case("payment-required"))
            .map(|(_, value)| Value::String(value.clone()))
            .unwrap_or(Value::Null);
        signals.push(ResponseSignal {
            standard: "x402".into(),
            summary: serde_json::json!({ "status": 402, "challenge": challenge }),
        });
    }
    signals
}

fn classify_ard(body: &str) -> Classification {
    match serde_json::from_str::<Value>(body) {
        Ok(value) => {
            let name = value
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Classification {
                standard: "ard".into(),
                status: Validity::Valid,
                summary: serde_json::json!({ "name": name }),
                reason: None,
            }
        }
        Err(e) => Classification {
            standard: "ard".into(),
            status: Validity::Invalid,
            summary: Value::Null,
            reason: Some(e.to_string()),
        },
    }
}

fn classify_api_catalog(body: &str) -> Classification {
    match serde_json::from_str::<Value>(body) {
        Ok(value) => {
            let has_linkset = value.get("linkset").is_some();
            Classification {
                standard: "api-catalog".into(),
                status: Validity::Valid,
                summary: if has_linkset {
                    serde_json::json!({ "format": "linkset" })
                } else {
                    serde_json::json!({})
                },
                reason: None,
            }
        }
        Err(e) => Classification {
            standard: "api-catalog".into(),
            status: Validity::Invalid,
            summary: Value::Null,
            reason: Some(e.to_string()),
        },
    }
}

fn classify_llms_txt(body: &str) -> Classification {
    let summary = parse_llms_txt_summary(body);
    Classification {
        standard: "llms-txt".into(),
        status: Validity::Valid,
        summary,
        reason: None,
    }
}

pub fn parse_llms_txt_summary(body: &str) -> Value {
    let mut title: Option<String> = None;
    let mut blockquote: Option<String> = None;
    let mut headings: Vec<String> = Vec::new();
    let mut in_blockquote = false;
    let mut bq_lines: Vec<String> = Vec::new();

    for line in body.lines() {
        let trimmed = line.trim();

        if title.is_none() {
            if let Some(rest) = trimmed.strip_prefix("# ") {
                let t = rest.trim();
                if !t.is_empty() {
                    title = Some(truncate(t, TITLE_CAP));
                }
                continue;
            }
        }

        if trimmed.starts_with("> ") || trimmed == ">" {
            in_blockquote = true;
            let content = trimmed.strip_prefix("> ").unwrap_or("").trim();
            bq_lines.push(content.to_string());
            continue;
        }

        if in_blockquote {
            in_blockquote = false;
            if blockquote.is_none() {
                let joined = bq_lines.join(" ").trim().to_string();
                if !joined.is_empty() {
                    blockquote = Some(truncate(&joined, TITLE_CAP));
                }
            }
            bq_lines.clear();
        }

        if trimmed.starts_with("## ") && headings.len() < HEADINGS_CAP {
            let heading = trimmed.strip_prefix("## ").unwrap_or("").trim();
            if !heading.is_empty() {
                headings.push(truncate(heading, HEADING_LEN_CAP));
            }
        }
    }

    if in_blockquote && blockquote.is_none() {
        let joined = bq_lines.join(" ").trim().to_string();
        if !joined.is_empty() {
            blockquote = Some(truncate(&joined, TITLE_CAP));
        }
    }

    let mut map = serde_json::Map::new();
    if let Some(t) = title {
        map.insert("title".into(), Value::String(t));
    }
    if let Some(bq) = blockquote {
        map.insert("summary".into(), Value::String(bq));
    }
    if !headings.is_empty() {
        map.insert(
            "headings".into(),
            Value::Array(headings.into_iter().map(Value::String).collect()),
        );
    }
    Value::Object(map)
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        s[..end].to_string()
    }
}

pub fn is_slicc_handoff_rel(rel: &str) -> bool {
    rel == HANDOFF_REL || rel == UPSKILL_REL
}

pub fn classify_slicc_handoff(link: &crate::link::Link) -> Option<Value> {
    let verb = if link.rel == HANDOFF_REL {
        "handoff"
    } else if link.rel == UPSKILL_REL {
        "upskill"
    } else {
        return None;
    };

    let mut map = serde_json::Map::new();
    map.insert("verb".into(), Value::String(verb.into()));
    map.insert("rel".into(), Value::String(link.rel.clone()));
    map.insert("href".into(), Value::String(link.href.clone()));
    if let Some(ref t) = link.media_type {
        map.insert("type".into(), Value::String(t.clone()));
    }
    if let Some(ref title) = link.title {
        if !title.is_empty() {
            map.insert("instruction".into(), Value::String(truncate(title, TITLE_CAP)));
        }
    }

    if verb == "upskill" {
        if let Some(branch) = link.params.get("branch") {
            if is_safe_branch(branch) {
                map.insert("branch".into(), Value::String(branch.clone()));
            }
        }
        if let Some(path) = link.params.get("path") {
            let canon = canonicalise_path(path);
            if is_safe_path(&canon) {
                map.insert("path".into(), Value::String(canon));
            }
        }
    }

    Some(Value::Object(map))
}

fn canonicalise_path(raw: &str) -> String {
    let trimmed = raw.trim_end_matches('/');
    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with("/skill.md") {
        trimmed[..trimmed.len() - "/skill.md".len()].to_string()
    } else if lower == "skill.md" {
        String::new()
    } else {
        trimmed.to_string()
    }
}

fn is_safe_branch(value: &str) -> bool {
    if value.is_empty() || value.len() > 250 {
        return false;
    }
    if !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b)) {
        return false;
    }
    if value.starts_with('-') || value.starts_with('/') || value.ends_with('/') {
        return false;
    }
    if value.contains("..") || value.ends_with(".lock") {
        return false;
    }
    true
}

fn is_safe_path(value: &str) -> bool {
    if value.is_empty() || value.len() > 1024 {
        return false;
    }
    if !value.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b)) {
        return false;
    }
    if value.starts_with('-') || value.starts_with('/') {
        return false;
    }
    if value.contains("..") {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_link_rels_includes_ard() {
        assert!(known_link_rels().contains(&"ard"));
    }

    #[test]
    fn known_link_rels_includes_ai_catalog() {
        assert!(known_link_rels().contains(&"ai-catalog"));
    }

    #[test]
    fn known_link_rels_includes_rfc_rels() {
        for rel in &["service-desc", "service-meta", "status", "service-doc", "api-catalog"] {
            assert!(known_link_rels().contains(rel), "missing {rel}");
        }
    }

    #[test]
    fn known_link_rels_includes_llms_txt_rel() {
        assert!(known_link_rels().contains(&LLMS_TXT_REL));
    }

    #[test]
    fn known_link_rels_includes_slicc_rels() {
        assert!(known_link_rels().contains(&HANDOFF_REL));
        assert!(known_link_rels().contains(&UPSKILL_REL));
    }

    #[test]
    fn default_probes_has_ard() {
        let registry = default_probes();
        let probes = registry.probes_for_origin("https://example.com");
        assert!(probes.iter().any(|p| p.probe.standard == "ard"));
        assert!(probes.iter().any(|p| p.url == "https://example.com/.well-known/ard.json"));
    }

    #[test]
    fn default_probes_has_api_catalog() {
        let registry = default_probes();
        let probes = registry.probes_for_origin("https://example.com");
        let ac = probes.iter().find(|p| p.probe.standard == "api-catalog").unwrap();
        assert_eq!(ac.url, "https://example.com/.well-known/api-catalog");
    }

    #[test]
    fn default_probes_has_llms_txt() {
        let registry = default_probes();
        let probes = registry.probes_for_origin("https://example.com");
        let lt = probes.iter().find(|p| p.probe.standard == "llms-txt").unwrap();
        assert_eq!(lt.url, "https://example.com/llms.txt");
    }

    #[test]
    fn default_classifiers_has_ard() {
        let registry = default_classifiers();
        let result = registry.classify("ard", r#"{"name":"Test"}"#).unwrap();
        assert_eq!(result.status, Validity::Valid);
        assert_eq!(result.summary["name"], "Test");
    }

    #[test]
    fn default_classifiers_has_api_catalog() {
        let registry = default_classifiers();
        let result = registry.classify("api-catalog", r#"{"linkset":[{"anchor":"/"}]}"#).unwrap();
        assert_eq!(result.status, Validity::Valid);
        assert_eq!(result.summary["format"], "linkset");
    }

    #[test]
    fn api_catalog_invalid_json() {
        let registry = default_classifiers();
        let result = registry.classify("api-catalog", "not json").unwrap();
        assert_eq!(result.status, Validity::Invalid);
        assert!(result.reason.is_some());
    }

    #[test]
    fn default_classifiers_has_llms_txt() {
        let registry = default_classifiers();
        let result = registry.classify("llms-txt", "# Example\n> A summary\n## Section One\n## Section Two\n").unwrap();
        assert_eq!(result.status, Validity::Valid);
        assert_eq!(result.summary["title"], "Example");
        assert_eq!(result.summary["summary"], "A summary");
        let headings = result.summary["headings"].as_array().unwrap();
        assert_eq!(headings.len(), 2);
        assert_eq!(headings[0], "Section One");
        assert_eq!(headings[1], "Section Two");
    }

    #[test]
    fn classify_ard_invalid_json() {
        let registry = default_classifiers();
        let result = registry.classify("ard", "not json").unwrap();
        assert_eq!(result.status, Validity::Invalid);
        assert!(result.reason.is_some());
    }

    #[test]
    fn classify_response_402() {
        let headers = vec![
            ("Content-Type".into(), "text/html".into()),
            ("X-Payment".into(), "required".into()),
        ];
        let signals = classify_response(402, &headers);
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].standard, "x402");
        assert_eq!(signals[0].summary["status"], 402);
        assert_eq!(signals[0].summary["challenge"], "required");
    }

    #[test]
    fn classify_response_402_no_header() {
        let signals = classify_response(402, &[]);
        assert_eq!(signals.len(), 1);
        assert_eq!(signals[0].summary["challenge"], Value::Null);
    }

    #[test]
    fn classify_response_200_no_signal() {
        let signals = classify_response(200, &[]);
        assert!(signals.is_empty());
    }

    #[test]
    fn classify_response_401_no_signal() {
        let signals = classify_response(401, &[]);
        assert!(signals.is_empty());
    }

    #[test]
    fn llms_txt_summary_h1_only() {
        let summary = parse_llms_txt_summary("# My Site\nSome body text\n");
        assert_eq!(summary["title"], "My Site");
        assert!(summary.get("summary").is_none());
        assert!(summary.get("headings").is_none());
    }

    #[test]
    fn llms_txt_summary_blockquote() {
        let summary = parse_llms_txt_summary("# Title\n> This is the summary\n> continued\nBody\n");
        assert_eq!(summary["title"], "Title");
        assert_eq!(summary["summary"], "This is the summary continued");
    }

    #[test]
    fn llms_txt_summary_headings_capped() {
        let mut body = "# Title\n".to_string();
        for i in 0..60 {
            body.push_str(&format!("## Heading {i}\n"));
        }
        let summary = parse_llms_txt_summary(&body);
        let headings = summary["headings"].as_array().unwrap();
        assert_eq!(headings.len(), 50);
    }

    #[test]
    fn llms_txt_summary_title_truncated() {
        let long_title = format!("# {}", "x".repeat(300));
        let summary = parse_llms_txt_summary(&long_title);
        assert!(summary["title"].as_str().unwrap().len() <= 200);
    }

    #[test]
    fn llms_txt_body_not_in_summary() {
        let body = "# Title\n> Summary\n## Sec\nThis is body text that should not appear\n";
        let summary = parse_llms_txt_summary(body);
        let json = serde_json::to_string(&summary).unwrap();
        assert!(!json.contains("body text"));
        assert!(!json.contains("should not appear"));
    }

    #[test]
    fn llms_txt_empty_body() {
        let summary = parse_llms_txt_summary("");
        assert!(summary.as_object().unwrap().is_empty());
    }

    #[test]
    fn llmstxtignore_defaults() {
        let defaults = default_llmstxtignore_entries();
        assert!(defaults.contains(&"github.com"));
        assert!(defaults.contains(&"app.slack.com"));
    }

    #[test]
    fn slicc_handoff_upskill_rel() {
        use crate::link::Link;
        let link = Link {
            href: "https://github.com/o/r".into(),
            rel: UPSKILL_REL.to_string(),
            media_type: None,
            title: None,
            params: [
                ("branch".to_string(), "main".to_string()),
                ("path".to_string(), "skills/foo".to_string()),
            ].into_iter().collect(),
        };
        let summary = classify_slicc_handoff(&link).unwrap();
        assert_eq!(summary["verb"], "upskill");
        assert_eq!(summary["branch"], "main");
        assert_eq!(summary["path"], "skills/foo");
    }

    #[test]
    fn slicc_handoff_rel() {
        use crate::link::Link;
        let link = Link {
            href: "https://example.com/page".into(),
            rel: HANDOFF_REL.to_string(),
            media_type: None,
            title: Some("Continue the signup flow".into()),
            params: Default::default(),
        };
        let summary = classify_slicc_handoff(&link).unwrap();
        assert_eq!(summary["verb"], "handoff");
        assert_eq!(summary["instruction"], "Continue the signup flow");
        assert!(summary.get("branch").is_none());
    }

    #[test]
    fn slicc_handoff_ignores_branch_path() {
        use crate::link::Link;
        let link = Link {
            href: "https://example.com/page".into(),
            rel: HANDOFF_REL.to_string(),
            media_type: None,
            title: Some("x".into()),
            params: [
                ("branch".to_string(), "main".to_string()),
                ("path".to_string(), "skills/foo".to_string()),
            ].into_iter().collect(),
        };
        let summary = classify_slicc_handoff(&link).unwrap();
        assert!(summary.get("branch").is_none());
        assert!(summary.get("path").is_none());
    }

    #[test]
    fn slicc_upskill_strips_skill_md() {
        use crate::link::Link;
        let link = Link {
            href: "https://github.com/o/r".into(),
            rel: UPSKILL_REL.to_string(),
            media_type: None,
            title: None,
            params: [
                ("path".to_string(), "skills/foo/SKILL.md".to_string()),
            ].into_iter().collect(),
        };
        let summary = classify_slicc_handoff(&link).unwrap();
        assert_eq!(summary["path"], "skills/foo");
    }

    #[test]
    fn slicc_upskill_drops_unsafe_branch() {
        use crate::link::Link;
        for branch in &["main;rm -rf /", "main`whoami`", "main$(whoami)", "-rf", "main\necho PWNED"] {
            let link = Link {
                href: "https://github.com/o/r".into(),
                rel: UPSKILL_REL.to_string(),
                media_type: None,
                title: None,
                params: [("branch".to_string(), branch.to_string())].into_iter().collect(),
            };
            let summary = classify_slicc_handoff(&link).unwrap();
            assert!(summary.get("branch").is_none(), "branch {branch:?} should be dropped");
        }
    }

    #[test]
    fn slicc_upskill_drops_unsafe_path() {
        use crate::link::Link;
        for path in &["../etc/passwd", "/etc/passwd", "-rf", "skills/foo;rm", "skills/`id`", "skills/$(id)"] {
            let link = Link {
                href: "https://github.com/o/r".into(),
                rel: UPSKILL_REL.to_string(),
                media_type: None,
                title: None,
                params: [("path".to_string(), path.to_string())].into_iter().collect(),
            };
            let summary = classify_slicc_handoff(&link).unwrap();
            assert!(summary.get("path").is_none(), "path {path:?} should be dropped");
        }
    }

    #[test]
    fn slicc_upskill_keeps_safe_branch() {
        use crate::link::Link;
        let link = Link {
            href: "https://github.com/o/r".into(),
            rel: UPSKILL_REL.to_string(),
            media_type: None,
            title: None,
            params: [("branch".to_string(), "release/v1.2_hotfix-3".to_string())].into_iter().collect(),
        };
        let summary = classify_slicc_handoff(&link).unwrap();
        assert_eq!(summary["branch"], "release/v1.2_hotfix-3");
    }

    #[test]
    fn is_slicc_handoff_rel_recognises_both() {
        assert!(is_slicc_handoff_rel(HANDOFF_REL));
        assert!(is_slicc_handoff_rel(UPSKILL_REL));
        assert!(!is_slicc_handoff_rel("ard"));
    }

    #[test]
    fn slicc_upskill_drops_only_skill_md_path() {
        use crate::link::Link;
        let link = Link {
            href: "https://github.com/o/r".into(),
            rel: UPSKILL_REL.to_string(),
            media_type: None,
            title: None,
            params: [("path".to_string(), "SKILL.md".to_string())].into_iter().collect(),
        };
        let summary = classify_slicc_handoff(&link).unwrap();
        assert!(summary.get("path").is_none());
    }

    #[test]
    fn slicc_unrecognised_rel_returns_none() {
        use crate::link::Link;
        let link = Link {
            href: "https://example.com/".into(),
            rel: "next".into(),
            media_type: None,
            title: None,
            params: Default::default(),
        };
        assert!(classify_slicc_handoff(&link).is_none());
    }
}
