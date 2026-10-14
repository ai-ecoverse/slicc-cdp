use crate::classifier::{Classification, ClassifierRegistry, Validity};
use crate::probe::{Probe, ProbeRegistry};
use serde_json::Value;
use url::Url;

pub const HANDOFF_REL: &str = "https://www.sliccy.ai/rel/handoff";
pub const UPSKILL_REL: &str = "https://www.sliccy.ai/rel/upskill";
pub const LLMS_TXT_REL: &str = "https://llmstxt.org/rel/llms-txt";

const TITLE_CAP: usize = 200;
const HEADINGS_CAP: usize = 50;
const HEADING_LEN_CAP: usize = 200;

const ENTRY_TYPE_CATALOG: &str = "application/ai-catalog+json";
const ENTRY_TYPE_REGISTRY: &str = "application/ai-registry+json";

const ENTRIES_CAP: usize = 200;
const DISPLAY_NAME_CAP: usize = 200;
const IDENTIFIER_CAP: usize = 500;
const QUERIES_CAP: usize = 10;
const QUERY_LEN_CAP: usize = 200;
const CAPABILITIES_CAP: usize = 50;
const CAPABILITY_LEN_CAP: usize = 100;

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
        standard: "ai-catalog".into(),
        path: "/.well-known/ai-catalog.json".into(),
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
    registry.register(Probe {
        standard: "poppy".into(),
        path: "/.well-known/poppy.json".into(),
        accept: "application/json".into(),
        content_type_prefix: "application/json".into(),
    });
    registry
}

pub fn default_classifiers() -> ClassifierRegistry {
    let mut registry = ClassifierRegistry::new();
    registry.register("ard", classify_ard);
    registry.register("ai-catalog", classify_ai_catalog);
    registry.register("api-catalog", classify_api_catalog);
    registry.register("llms-txt", classify_llms_txt);
    registry.register("poppy", classify_poppy);
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
    classify_ard_with_origin(body, None)
}

fn classify_ard_with_origin(body: &str, origin: Option<&str>) -> Classification {
    let origin_domain = origin.and_then(|o| {
        Url::parse(o).ok().and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
    });
    match serde_json::from_str::<Value>(body) {
        Ok(value) => classify_ard_value(&value, origin_domain.as_deref()),
        Err(e) => Classification {
            standard: "ard".into(),
            status: Validity::Invalid,
            summary: Value::Null,
            reason: Some(e.to_string()),
        },
    }
}

pub fn classify_ard_body(body: &str, origin: Option<&str>) -> Classification {
    classify_ard_with_origin(body, origin)
}

fn classify_ai_catalog(body: &str) -> Classification {
    classify_ai_catalog_with_origin(body, None)
}

fn classify_ai_catalog_with_origin(body: &str, origin: Option<&str>) -> Classification {
    let origin_domain = origin.and_then(|o| {
        Url::parse(o).ok().and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
    });
    match serde_json::from_str::<Value>(body) {
        Ok(value) => {
            let mut c = classify_ard_value(&value, origin_domain.as_deref());
            c.standard = "ai-catalog".into();
            c
        }
        Err(e) => Classification {
            standard: "ai-catalog".into(),
            status: Validity::Invalid,
            summary: Value::Null,
            reason: Some(e.to_string()),
        },
    }
}

pub fn classify_ai_catalog_body(body: &str, origin: Option<&str>) -> Classification {
    classify_ai_catalog_with_origin(body, origin)
}

fn classify_ard_value(value: &Value, origin_domain: Option<&str>) -> Classification {
    let name = value
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let name = truncate(&name, TITLE_CAP);

    let entries = match value.get("entries").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            return Classification {
                standard: "ard".into(),
                status: Validity::Valid,
                summary: serde_json::json!({ "name": name, "entries": [], "warnings": ["no entries array"] }),
                reason: None,
            };
        }
    };

    let mut valid_entries: Vec<Value> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut type_groups: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut catalogs: Vec<Value> = Vec::new();

    for (idx, entry) in entries.iter().take(ENTRIES_CAP).enumerate() {
        let identifier = entry.get("identifier").and_then(|v| v.as_str()).unwrap_or("");
        let display_name = entry.get("displayName").and_then(|v| v.as_str()).unwrap_or("");
        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let has_url = entry.get("url").and_then(|v| v.as_str()).is_some();
        let has_data = entry.get("data").is_some();

        if identifier.is_empty() || display_name.is_empty() || entry_type.is_empty() {
            warnings.push(format!("entry {idx}: missing required field (identifier, displayName, or type)"));
            continue;
        }
        if !has_url && !has_data {
            warnings.push(format!("entry {idx}: missing url and data"));
            continue;
        }
        if has_url && has_data {
            warnings.push(format!("entry {idx}: has both url and data"));
            continue;
        }

        let urn_check = check_urn_publisher(identifier, origin_domain);
        let trust_check = check_trust_identity(entry, origin_domain);

        if let Some(ref warn) = urn_check {
            warnings.push(format!("entry {idx}: {warn}"));
            continue;
        }
        if let Some(ref warn) = trust_check {
            warnings.push(format!("entry {idx}: {warn}"));
        }

        *type_groups.entry(entry_type.to_string()).or_insert(0) += 1;

        if entry_type == ENTRY_TYPE_CATALOG || entry_type == ENTRY_TYPE_REGISTRY {
            let mut cat = serde_json::Map::new();
            cat.insert("identifier".into(), Value::String(truncate(identifier, IDENTIFIER_CAP)));
            cat.insert("displayName".into(), Value::String(truncate(display_name, DISPLAY_NAME_CAP)));
            cat.insert("type".into(), Value::String(entry_type.to_string()));
            if let Some(url) = entry.get("url").and_then(|v| v.as_str()) {
                cat.insert("url".into(), Value::String(url.to_string()));
            }
            catalogs.push(Value::Object(cat));
        }

        let mut summary_entry = serde_json::Map::new();
        summary_entry.insert("identifier".into(), Value::String(truncate(identifier, IDENTIFIER_CAP)));
        summary_entry.insert("displayName".into(), Value::String(truncate(display_name, DISPLAY_NAME_CAP)));
        summary_entry.insert("type".into(), Value::String(entry_type.to_string()));
        if let Some(url) = entry.get("url").and_then(|v| v.as_str()) {
            summary_entry.insert("url".into(), Value::String(url.to_string()));
        }
        if let Some(caps) = entry.get("capabilities").and_then(|v| v.as_array()) {
            let capped: Vec<Value> = caps.iter()
                .take(CAPABILITIES_CAP)
                .filter_map(|c| c.as_str().map(|s| Value::String(truncate(s, CAPABILITY_LEN_CAP))))
                .collect();
            if !capped.is_empty() {
                summary_entry.insert("capabilities".into(), Value::Array(capped));
            }
        }
        if let Some(queries) = entry.get("representativeQueries").and_then(|v| v.as_array()) {
            let capped: Vec<Value> = queries.iter()
                .take(QUERIES_CAP)
                .filter_map(|q| q.as_str().map(|s| Value::String(truncate(s, QUERY_LEN_CAP))))
                .collect();
            if !capped.is_empty() {
                summary_entry.insert("representativeQueries".into(), Value::Array(capped));
            }
        }
        valid_entries.push(Value::Object(summary_entry));
    }

    let mut summary = serde_json::Map::new();
    summary.insert("name".into(), Value::String(name));
    summary.insert("entries".into(), Value::Array(valid_entries));
    if !type_groups.is_empty() {
        let groups: serde_json::Map<String, Value> = type_groups
            .into_iter()
            .map(|(k, v)| (k, Value::Number(serde_json::Number::from(v))))
            .collect();
        summary.insert("types".into(), Value::Object(groups));
    }
    if !catalogs.is_empty() {
        summary.insert("catalogs".into(), Value::Array(catalogs));
    }
    if !warnings.is_empty() {
        summary.insert("warnings".into(), Value::Array(
            warnings.into_iter().map(Value::String).collect()
        ));
    }

    Classification {
        standard: "ard".into(),
        status: Validity::Valid,
        summary: Value::Object(summary),
        reason: None,
    }
}

pub fn registrable_domain(host: &str) -> Option<String> {
    let host = host.strip_suffix('.').unwrap_or(host);
    let bytes = host.as_bytes();
    let suffix = psl::suffix(bytes)?;
    if suffix.is_known() {
        let domain = psl::domain(bytes)?;
        Some(std::str::from_utf8(domain.as_bytes()).ok()?.to_ascii_lowercase())
    } else {
        None
    }
}

fn check_urn_publisher(identifier: &str, origin_domain: Option<&str>) -> Option<String> {
    let origin_domain = origin_domain?;
    let prefix = "urn:air:";
    if !identifier.starts_with(prefix) {
        return None;
    }
    let rest = &identifier[prefix.len()..];
    let publisher = rest.split(':').next().unwrap_or("");
    if publisher.is_empty() {
        return Some("URN has empty publisher".into());
    }
    let publisher_lower = publisher.to_ascii_lowercase();
    let publisher_reg = registrable_domain(&publisher_lower);
    let origin_reg = registrable_domain(origin_domain);
    match (publisher_reg, origin_reg) {
        (Some(p), Some(o)) if p == o => None,
        (Some(p), Some(o)) => Some(format!(
            "URN publisher {publisher} (registrable: {p}) does not match origin (registrable: {o})"
        )),
        _ => {
            if publisher_lower == origin_domain.to_ascii_lowercase() {
                None
            } else {
                Some(format!("URN publisher {publisher} does not match origin {origin_domain}"))
            }
        }
    }
}

fn check_trust_identity(entry: &Value, origin_domain: Option<&str>) -> Option<String> {
    let origin_domain = origin_domain?;
    let identity = entry
        .get("trustManifest")
        .and_then(|tm| tm.get("identity"))
        .and_then(|id| id.as_str())?;
    let identity_lower = identity.to_ascii_lowercase();
    let identity_reg = registrable_domain(&identity_lower);
    let origin_reg = registrable_domain(origin_domain);
    match (identity_reg, origin_reg) {
        (Some(i), Some(o)) if i == o => None,
        (Some(i), Some(o)) => Some(format!(
            "trustManifest.identity {identity} (registrable: {i}) does not match origin (registrable: {o})"
        )),
        _ => {
            if identity_lower == origin_domain.to_ascii_lowercase() {
                None
            } else {
                Some(format!("trustManifest.identity {identity} does not match origin {origin_domain}"))
            }
        }
    }
}

pub fn parse_robots_txt_agentmap(body: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Agentmap:") {
            let url = rest.trim();
            if !url.is_empty() {
                urls.push(url.to_string());
            }
        } else if let Some(rest) = trimmed.strip_prefix("agentmap:") {
            let url = rest.trim();
            if !url.is_empty() {
                urls.push(url.to_string());
            }
        } else {
            let lower = trimmed.to_ascii_lowercase();
            if let Some(rest_lower) = lower.strip_prefix("agentmap:") {
                let colon_pos = trimmed.find(':').unwrap();
                let url = trimmed[colon_pos + 1..].trim();
                let _ = rest_lower;
                if !url.is_empty() {
                    urls.push(url.to_string());
                }
            }
        }
    }
    urls
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

fn classify_poppy(body: &str) -> Classification {
    classify_poppy_with_origin(body, None)
}

fn classify_poppy_with_origin(body: &str, origin: Option<&str>) -> Classification {
    let origin_domain = origin.and_then(|o| {
        Url::parse(o).ok().and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
    });
    match serde_json::from_str::<Value>(body) {
        Ok(value) => classify_poppy_value(&value, origin_domain.as_deref()),
        Err(e) => Classification {
            standard: "poppy".into(),
            status: Validity::Invalid,
            summary: Value::Null,
            reason: Some(e.to_string()),
        },
    }
}

pub fn classify_poppy_body(body: &str, origin: Option<&str>) -> Classification {
    classify_poppy_with_origin(body, origin)
}

fn strip_www(host: &str) -> &str {
    host.strip_prefix("www.").unwrap_or(host)
}

fn classify_poppy_value(value: &Value, origin_domain: Option<&str>) -> Classification {
    let protocol_version = value.get("protocol_version").and_then(|v| v.as_str()).unwrap_or("");
    if protocol_version.is_empty() {
        return Classification {
            standard: "poppy".into(),
            status: Validity::Invalid,
            summary: Value::Null,
            reason: Some("missing protocol_version".into()),
        };
    }
    let parts: Vec<&str> = protocol_version.split('.').collect();
    if parts.len() != 2
        || parts[0].parse::<u32>().is_err()
        || parts[1].parse::<u32>().is_err()
    {
        return Classification {
            standard: "poppy".into(),
            status: Validity::Invalid,
            summary: Value::Null,
            reason: Some(format!("invalid protocol_version: {protocol_version}")),
        };
    }

    let org = value.get("organization");
    let org_domain = org
        .and_then(|o| o.get("domain"))
        .and_then(|d| d.as_str())
        .unwrap_or("");
    if let Some(host) = origin_domain {
        if !org_domain.is_empty() {
            let org_lower = org_domain.to_ascii_lowercase();
            let org_stripped = strip_www(&org_lower);
            let host_stripped = strip_www(host);
            let org_reg = registrable_domain(org_stripped);
            let host_reg = registrable_domain(host_stripped);
            let mismatch = match (org_reg, host_reg) {
                (Some(o), Some(h)) => o != h,
                _ => org_stripped != host_stripped,
            };
            if mismatch {
                return Classification {
                    standard: "poppy".into(),
                    status: Validity::Invalid,
                    summary: Value::Null,
                    reason: Some(format!(
                        "organization.domain {org_domain} does not match host {host}"
                    )),
                };
            }
        }
    }

    let has_agent = value.get("agent").is_some();
    let has_apis = value.get("apis").is_some();
    let has_web = value.get("web").is_some();
    if !has_agent && !has_apis && !has_web {
        return Classification {
            standard: "poppy".into(),
            status: Validity::Invalid,
            summary: Value::Null,
            reason: Some("requires at least one of agent, apis, web".into()),
        };
    }

    let has_browser_session = value
        .get("web")
        .and_then(|w| w.get("browser_session_endpoint"))
        .and_then(|v| v.as_str())
        .is_some();
    let needs_auth = has_agent || has_apis || has_browser_session;
    if needs_auth && value.get("auth").is_none() {
        return Classification {
            standard: "poppy".into(),
            status: Validity::Invalid,
            summary: Value::Null,
            reason: Some("auth required when agent, apis, or web.browser_session_endpoint is present".into()),
        };
    }

    let org_name = org
        .and_then(|o| o.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| truncate(s, TITLE_CAP));

    let issuer = value.get("issuer").and_then(|v| v.as_str()).map(str::to_string);

    let sign_in: Vec<String> = value
        .get("auth")
        .and_then(|a| a.get("sign_in_methods"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("type").and_then(|t| t.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let apis: Vec<Value> = value
        .get("apis")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let t = entry.get("type").and_then(|v| v.as_str())?;
                    let url = entry.get("url").and_then(|v| v.as_str())?;
                    Some(serde_json::json!({ "type": t, "url": url }))
                })
                .collect()
        })
        .unwrap_or_default();

    let agent_protocols: Vec<Value> = value
        .get("agent")
        .and_then(|a| a.get("protocols"))
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let t = entry.get("type").and_then(|v| v.as_str())?;
                    let ep = entry.get("endpoint").and_then(|v| v.as_str())?;
                    Some(serde_json::json!({ "type": t, "endpoint": ep }))
                })
                .collect()
        })
        .unwrap_or_default();

    let browser_session_endpoint = value
        .get("web")
        .and_then(|w| w.get("browser_session_endpoint"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let mut summary = serde_json::Map::new();
    summary.insert("protocolVersion".into(), Value::String(protocol_version.to_string()));
    if let Some(name) = org_name {
        let mut org_map = serde_json::Map::new();
        org_map.insert("name".into(), Value::String(name));
        org_map.insert("domain".into(), Value::String(org_domain.to_string()));
        summary.insert("organization".into(), Value::Object(org_map));
    } else if !org_domain.is_empty() {
        let mut org_map = serde_json::Map::new();
        org_map.insert("domain".into(), Value::String(org_domain.to_string()));
        summary.insert("organization".into(), Value::Object(org_map));
    }
    if let Some(iss) = issuer {
        summary.insert("issuer".into(), Value::String(iss));
    }
    if !sign_in.is_empty() {
        summary.insert("signIn".into(), Value::Array(
            sign_in.into_iter().map(Value::String).collect(),
        ));
    }
    if !apis.is_empty() {
        summary.insert("apis".into(), Value::Array(apis));
    }
    if !agent_protocols.is_empty() {
        summary.insert("agentProtocols".into(), Value::Array(agent_protocols));
    }
    if let Some(bse) = browser_session_endpoint {
        summary.insert("browserSessionEndpoint".into(), Value::String(bse));
    }

    Classification {
        standard: "poppy".into(),
        status: Validity::Valid,
        summary: Value::Object(summary),
        reason: None,
    }
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
    fn default_probes_has_ai_catalog() {
        let registry = default_probes();
        let probes = registry.probes_for_origin("https://example.com");
        let aic = probes.iter().find(|p| p.probe.standard == "ai-catalog").unwrap();
        assert_eq!(aic.url, "https://example.com/.well-known/ai-catalog.json");
    }

    #[test]
    fn default_probes_has_llms_txt() {
        let registry = default_probes();
        let probes = registry.probes_for_origin("https://example.com");
        let lt = probes.iter().find(|p| p.probe.standard == "llms-txt").unwrap();
        assert_eq!(lt.url, "https://example.com/llms.txt");
    }

    #[test]
    fn default_probes_has_poppy() {
        let registry = default_probes();
        let probes = registry.probes_for_origin("https://example.com");
        let p = probes.iter().find(|p| p.probe.standard == "poppy").unwrap();
        assert_eq!(p.url, "https://example.com/.well-known/poppy.json");
        assert_eq!(p.probe.accept, "application/json");
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
    fn default_classifiers_has_ai_catalog() {
        let registry = default_classifiers();
        let result = registry.classify("ai-catalog", r#"{"name":"Test Catalog","entries":[{"identifier":"urn:air:example.com:mcp","displayName":"MCP Server","type":"application/mcp-server-card+json","url":"https://example.com/mcp"}]}"#).unwrap();
        assert_eq!(result.status, Validity::Valid);
        assert_eq!(result.standard, "ai-catalog");
        assert_eq!(result.summary["name"], "Test Catalog");
    }

    #[test]
    fn ai_catalog_classifier_invalid_json() {
        let registry = default_classifiers();
        let result = registry.classify("ai-catalog", "not json").unwrap();
        assert_eq!(result.status, Validity::Invalid);
        assert_eq!(result.standard, "ai-catalog");
        assert!(result.reason.is_some());
    }

    #[test]
    fn ai_catalog_body_with_origin() {
        let body = r#"{"name":"Cat","entries":[{"identifier":"urn:air:example.com:agent","displayName":"Agent","type":"application/a2a-agent-card+json","url":"https://example.com/agent"},{"identifier":"urn:air:example.com:ds","displayName":"Dataset","type":"application/dataset+json","url":"https://example.com/ds"},{"identifier":"urn:air:evil.com:mcp","displayName":"Evil","type":"application/mcp-server-card+json","url":"https://evil.com/mcp"}]}"#;
        let c = classify_ai_catalog_body(body, Some("https://example.com"));
        assert_eq!(c.standard, "ai-catalog");
        assert_eq!(c.status, Validity::Valid);
        let entries = c.summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let warnings = c.summary["warnings"].as_array().unwrap();
        assert!(warnings.iter().any(|w| w.as_str().unwrap().contains("evil.com")));
        let types = c.summary["types"].as_object().unwrap();
        assert!(types.contains_key("application/a2a-agent-card+json"));
        assert!(types.contains_key("application/dataset+json"));
    }

    #[test]
    fn ai_catalog_nested_catalog_entries() {
        let body = r#"{"name":"Root","entries":[{"identifier":"urn:air:example.com:cat","displayName":"Sub","type":"application/ai-catalog+json","url":"https://example.com/sub.json"}]}"#;
        let c = classify_ai_catalog_body(body, Some("https://example.com"));
        assert_eq!(c.standard, "ai-catalog");
        let catalogs = c.summary["catalogs"].as_array().unwrap();
        assert_eq!(catalogs.len(), 1);
        assert_eq!(catalogs[0]["url"], "https://example.com/sub.json");
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
    fn default_classifiers_has_poppy() {
        let registry = default_classifiers();
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "name": "Test", "domain": "example.com" },
            "web": { "home": "/" }
        }).to_string();
        let result = registry.classify("poppy", &body).unwrap();
        assert_eq!(result.status, Validity::Valid);
        assert_eq!(result.summary["protocolVersion"], "0.1");
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

    #[test]
    fn ard_valid_entries_grouped_by_type() {
        let body = serde_json::json!({
            "name": "Acme Corp",
            "entries": [
                {
                    "identifier": "urn:air:acme.com:server:weather",
                    "displayName": "Weather Server",
                    "type": "application/mcp-server-card+json",
                    "url": "https://api.acme.com/mcp/weather.json",
                    "capabilities": ["WeatherTool"],
                    "representativeQueries": ["what is the weather"]
                },
                {
                    "identifier": "urn:air:acme.com:skill:pptx",
                    "displayName": "PPTX Creator",
                    "type": "application/agent-skills+md",
                    "url": "https://acme.com/skills/pptx"
                }
            ]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://acme.com"));
        assert_eq!(result.status, Validity::Valid);
        assert_eq!(result.summary["name"], "Acme Corp");
        let entries = result.summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["type"], "application/mcp-server-card+json");
        assert_eq!(entries[1]["type"], "application/agent-skills+md");
        let types = result.summary["types"].as_object().unwrap();
        assert_eq!(types["application/mcp-server-card+json"], 1);
        assert_eq!(types["application/agent-skills+md"], 1);
    }

    #[test]
    fn ard_squatted_urn_produces_warning() {
        let body = serde_json::json!({
            "name": "Evil",
            "entries": [
                {
                    "identifier": "urn:air:google.com:server:search",
                    "displayName": "Fake Google",
                    "type": "application/mcp-server-card+json",
                    "url": "https://evil.com/fake"
                },
                {
                    "identifier": "urn:air:evil.com:server:legit",
                    "displayName": "Legit Server",
                    "type": "application/mcp-server-card+json",
                    "url": "https://evil.com/legit"
                }
            ]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://evil.com"));
        assert_eq!(result.status, Validity::Valid);
        let entries = result.summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["displayName"], "Legit Server");
        let warnings = result.summary["warnings"].as_array().unwrap();
        assert!(warnings.iter().any(|w| w.as_str().unwrap().contains("google.com")));
    }

    #[test]
    fn ard_trust_identity_mismatch_warns() {
        let body = serde_json::json!({
            "name": "Test",
            "entries": [
                {
                    "identifier": "urn:air:example.com:server:api",
                    "displayName": "API",
                    "type": "application/mcp-server-card+json",
                    "url": "https://example.com/api",
                    "trustManifest": {
                        "identity": "other-corp.com"
                    }
                }
            ]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://example.com"));
        let warnings = result.summary["warnings"].as_array().unwrap();
        assert!(warnings.iter().any(|w| w.as_str().unwrap().contains("trustManifest.identity")));
        let entries = result.summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn ard_nested_catalogs_reported() {
        let body = serde_json::json!({
            "name": "Hub",
            "entries": [
                {
                    "identifier": "urn:air:hub.com:catalog:partners",
                    "displayName": "Partners Catalog",
                    "type": "application/ai-catalog+json",
                    "url": "https://hub.com/partners.json"
                },
                {
                    "identifier": "urn:air:hub.com:registry:search",
                    "displayName": "Search Registry",
                    "type": "application/ai-registry+json",
                    "url": "https://hub.com/search"
                }
            ]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://hub.com"));
        let catalogs = result.summary["catalogs"].as_array().unwrap();
        assert_eq!(catalogs.len(), 2);
        assert_eq!(catalogs[0]["type"], "application/ai-catalog+json");
        assert_eq!(catalogs[1]["type"], "application/ai-registry+json");
    }

    #[test]
    fn ard_missing_required_field_skipped() {
        let body = serde_json::json!({
            "name": "Bad",
            "entries": [
                {
                    "displayName": "No Identifier",
                    "type": "application/mcp-server-card+json",
                    "url": "https://bad.com/x"
                },
                {
                    "identifier": "urn:air:bad.com:x",
                    "type": "application/mcp-server-card+json",
                    "url": "https://bad.com/x"
                },
                {
                    "identifier": "urn:air:bad.com:y",
                    "displayName": "No URL or Data",
                    "type": "application/mcp-server-card+json"
                }
            ]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://bad.com"));
        let entries = result.summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 0);
        let warnings = result.summary["warnings"].as_array().unwrap();
        assert_eq!(warnings.len(), 3);
    }

    #[test]
    fn ard_both_url_and_data_rejected() {
        let body = serde_json::json!({
            "name": "Both",
            "entries": [{
                "identifier": "urn:air:both.com:x",
                "displayName": "Both Fields",
                "type": "application/mcp-server-card+json",
                "url": "https://both.com/x",
                "data": {}
            }]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://both.com"));
        let entries = result.summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn ard_no_entries_array_warns() {
        let body = r#"{"name":"Empty"}"#;
        let result = classify_ard_body(body, None);
        assert_eq!(result.status, Validity::Valid);
        assert_eq!(result.summary["name"], "Empty");
        let warnings = result.summary["warnings"].as_array().unwrap();
        assert!(warnings.iter().any(|w| w.as_str().unwrap().contains("no entries")));
    }

    #[test]
    fn ard_invalid_json() {
        let result = classify_ard_body("not json", None);
        assert_eq!(result.status, Validity::Invalid);
        assert!(result.reason.is_some());
    }

    #[test]
    fn ard_no_origin_skips_urn_check() {
        let body = serde_json::json!({
            "name": "No Origin",
            "entries": [{
                "identifier": "urn:air:google.com:server:search",
                "displayName": "Search",
                "type": "application/mcp-server-card+json",
                "url": "https://google.com/mcp"
            }]
        }).to_string();
        let result = classify_ard_body(&body, None);
        let entries = result.summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn ard_subdomain_urn_matches_registrable() {
        let body = serde_json::json!({
            "name": "Sub",
            "entries": [{
                "identifier": "urn:air:acme.com:server:api",
                "displayName": "API",
                "type": "application/mcp-server-card+json",
                "url": "https://api.acme.com/mcp"
            }]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://api.acme.com"));
        let entries = result.summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(result.summary.get("warnings").is_none());
    }

    #[test]
    fn ard_capabilities_and_queries_in_summary() {
        let body = serde_json::json!({
            "name": "Rich",
            "entries": [{
                "identifier": "urn:air:rich.com:server:api",
                "displayName": "API",
                "type": "application/mcp-server-card+json",
                "url": "https://rich.com/mcp",
                "capabilities": ["FooTool", "BarTool"],
                "representativeQueries": ["how do I foo", "bar something"]
            }]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://rich.com"));
        let entry = &result.summary["entries"][0];
        let caps = entry["capabilities"].as_array().unwrap();
        assert_eq!(caps.len(), 2);
        let queries = entry["representativeQueries"].as_array().unwrap();
        assert_eq!(queries.len(), 2);
    }

    #[test]
    fn ard_display_name_capped() {
        let long_name = "x".repeat(300);
        let body = serde_json::json!({
            "name": "Cap",
            "entries": [{
                "identifier": "urn:air:cap.com:x",
                "displayName": long_name,
                "type": "application/mcp-server-card+json",
                "url": "https://cap.com/x"
            }]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://cap.com"));
        let entry = &result.summary["entries"][0];
        assert!(entry["displayName"].as_str().unwrap().len() <= 200);
    }

    #[test]
    fn ard_data_entry_accepted() {
        let body = serde_json::json!({
            "name": "Inline",
            "entries": [{
                "identifier": "urn:air:inline.com:x",
                "displayName": "Inline",
                "type": "application/agent-skills+md",
                "data": "# My Skill\nSome content"
            }]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://inline.com"));
        let entries = result.summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn ard_fixture_mcp_skill_squatted() {
        let body = serde_json::json!({
            "name": "Fixture Site",
            "entries": [
                {
                    "identifier": "urn:air:fixture.com:server:weather",
                    "displayName": "Weather MCP",
                    "type": "application/mcp-server-card+json",
                    "url": "https://fixture.com/mcp/weather.json",
                    "capabilities": ["WeatherTool"]
                },
                {
                    "identifier": "urn:air:fixture.com:skill:pptx",
                    "displayName": "PPTX Skill",
                    "type": "application/agent-skills+md",
                    "url": "https://fixture.com/skills/pptx"
                },
                {
                    "identifier": "urn:air:google.com:server:search",
                    "displayName": "Squatted Google",
                    "type": "application/mcp-server-card+json",
                    "url": "https://fixture.com/fake-google"
                }
            ]
        }).to_string();
        let result = classify_ard_body(&body, Some("https://fixture.com"));
        assert_eq!(result.status, Validity::Valid);
        let entries = result.summary["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let types: Vec<&str> = entries.iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect();
        assert!(types.contains(&"application/mcp-server-card+json"));
        assert!(types.contains(&"application/agent-skills+md"));
        let warnings = result.summary["warnings"].as_array().unwrap();
        assert!(warnings.iter().any(|w| {
            let s = w.as_str().unwrap();
            s.contains("google.com") && s.contains("URN publisher")
        }));
    }

    #[test]
    fn robots_txt_agentmap_parses() {
        let robots = "User-agent: *\nDisallow: /private\nAgentmap: https://example.com/entries.json\n";
        let urls = parse_robots_txt_agentmap(robots);
        assert_eq!(urls, vec!["https://example.com/entries.json"]);
    }

    #[test]
    fn robots_txt_agentmap_case_insensitive() {
        let robots = "AGENTMAP: https://example.com/a.json\nagentmap: https://example.com/b.json\n";
        let urls = parse_robots_txt_agentmap(robots);
        assert_eq!(urls.len(), 2);
    }

    #[test]
    fn robots_txt_agentmap_empty() {
        let robots = "User-agent: *\nDisallow: /\n";
        let urls = parse_robots_txt_agentmap(robots);
        assert!(urls.is_empty());
    }

    #[test]
    fn robots_txt_agentmap_multiple() {
        let robots = "Agentmap: https://a.com/1.json\nAgentmap: https://b.com/2.json\n";
        let urls = parse_robots_txt_agentmap(robots);
        assert_eq!(urls.len(), 2);
    }

    #[test]
    fn registrable_domain_works() {
        assert_eq!(registrable_domain("www.example.com"), Some("example.com".into()));
        assert_eq!(registrable_domain("api.acme.co.uk"), Some("acme.co.uk".into()));
        assert_eq!(registrable_domain("example.com"), Some("example.com".into()));
    }

    #[test]
    fn registrable_domain_tld_only() {
        assert_eq!(registrable_domain("com"), None);
    }

    #[test]
    fn urn_check_cross_origin() {
        let warn = check_urn_publisher("urn:air:google.com:server:x", Some("evil.com"));
        assert!(warn.is_some());
    }

    #[test]
    fn urn_check_same_origin() {
        let warn = check_urn_publisher("urn:air:acme.com:server:x", Some("acme.com"));
        assert!(warn.is_none());
    }

    #[test]
    fn urn_check_subdomain_matches() {
        let warn = check_urn_publisher("urn:air:acme.com:server:x", Some("api.acme.com"));
        assert!(warn.is_none());
    }

    #[test]
    fn urn_check_no_origin() {
        let warn = check_urn_publisher("urn:air:google.com:server:x", None);
        assert!(warn.is_none());
    }

    #[test]
    fn urn_check_non_urn_identifier() {
        let warn = check_urn_publisher("some-other-id", Some("evil.com"));
        assert!(warn.is_none());
    }

    #[test]
    fn trust_identity_matching() {
        let entry = serde_json::json!({
            "trustManifest": { "identity": "example.com" }
        });
        let warn = check_trust_identity(&entry, Some("example.com"));
        assert!(warn.is_none());
    }

    #[test]
    fn trust_identity_mismatch() {
        let entry = serde_json::json!({
            "trustManifest": { "identity": "other.com" }
        });
        let warn = check_trust_identity(&entry, Some("example.com"));
        assert!(warn.is_some());
    }

    #[test]
    fn trust_identity_no_manifest() {
        let entry = serde_json::json!({});
        let warn = check_trust_identity(&entry, Some("example.com"));
        assert!(warn.is_none());
    }

    #[test]
    fn trust_identity_subdomain() {
        let entry = serde_json::json!({
            "trustManifest": { "identity": "api.acme.com" }
        });
        let warn = check_trust_identity(&entry, Some("www.acme.com"));
        assert!(warn.is_none());
    }

    #[test]
    fn poppy_valid_minimal() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "name": "Acme", "domain": "example.com" },
            "web": { "home": "https://example.com" }
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://example.com"));
        assert_eq!(result.status, Validity::Valid);
        assert_eq!(result.summary["protocolVersion"], "0.1");
        assert_eq!(result.summary["organization"]["name"], "Acme");
    }

    #[test]
    fn poppy_spec_example() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": {
                "name": "Acme Corp",
                "domain": "acme.com"
            },
            "issuer": "https://auth.acme.com",
            "auth": {
                "sign_in_methods": [
                    { "type": "direct" },
                    { "type": "device" }
                ]
            },
            "agent": {
                "protocols": [
                    { "type": "a2a", "endpoint": "https://api.acme.com/a2a" }
                ]
            },
            "apis": [
                { "type": "mcp", "url": "https://api.acme.com/mcp" },
                { "type": "rest", "url": "https://api.acme.com/v1" }
            ],
            "web": {
                "browser_session_endpoint": "https://acme.com/session"
            }
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://acme.com"));
        assert_eq!(result.status, Validity::Valid);
        assert_eq!(result.summary["protocolVersion"], "0.1");
        assert_eq!(result.summary["organization"]["name"], "Acme Corp");
        assert_eq!(result.summary["issuer"], "https://auth.acme.com");
        let sign_in = result.summary["signIn"].as_array().unwrap();
        assert_eq!(sign_in.len(), 2);
        let apis = result.summary["apis"].as_array().unwrap();
        assert_eq!(apis.len(), 2);
        assert_eq!(apis[0]["type"], "mcp");
        let protocols = result.summary["agentProtocols"].as_array().unwrap();
        assert_eq!(protocols.len(), 1);
        assert_eq!(protocols[0]["type"], "a2a");
        assert_eq!(result.summary["browserSessionEndpoint"], "https://acme.com/session");
    }

    #[test]
    fn poppy_missing_protocol_version() {
        let body = serde_json::json!({
            "organization": { "domain": "example.com" },
            "web": {}
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://example.com"));
        assert_eq!(result.status, Validity::Invalid);
        assert!(result.reason.unwrap().contains("protocol_version"));
    }

    #[test]
    fn poppy_invalid_protocol_version() {
        let body = serde_json::json!({
            "protocol_version": "1",
            "organization": { "domain": "example.com" },
            "web": {}
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://example.com"));
        assert_eq!(result.status, Validity::Invalid);
        assert!(result.reason.unwrap().contains("invalid protocol_version"));
    }

    #[test]
    fn poppy_domain_mismatch() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "domain": "evil.com" },
            "web": {}
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://example.com"));
        assert_eq!(result.status, Validity::Invalid);
        assert!(result.reason.unwrap().contains("does not match"));
    }

    #[test]
    fn poppy_domain_www_stripped() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "domain": "www.example.com" },
            "web": {}
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://example.com"));
        assert_eq!(result.status, Validity::Valid);
    }

    #[test]
    fn poppy_no_sections() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "domain": "example.com" }
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://example.com"));
        assert_eq!(result.status, Validity::Invalid);
        assert!(result.reason.unwrap().contains("at least one of"));
    }

    #[test]
    fn poppy_auth_required_with_agent() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "domain": "example.com" },
            "agent": { "protocols": [] }
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://example.com"));
        assert_eq!(result.status, Validity::Invalid);
        assert!(result.reason.unwrap().contains("auth required"));
    }

    #[test]
    fn poppy_auth_required_with_apis() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "domain": "example.com" },
            "apis": []
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://example.com"));
        assert_eq!(result.status, Validity::Invalid);
        assert!(result.reason.unwrap().contains("auth required"));
    }

    #[test]
    fn poppy_auth_required_with_browser_session() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "domain": "example.com" },
            "web": { "browser_session_endpoint": "https://example.com/session" }
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://example.com"));
        assert_eq!(result.status, Validity::Invalid);
        assert!(result.reason.unwrap().contains("auth required"));
    }

    #[test]
    fn poppy_web_only_no_auth_needed() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "domain": "example.com" },
            "web": { "home": "https://example.com" }
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://example.com"));
        assert_eq!(result.status, Validity::Valid);
    }

    #[test]
    fn poppy_invalid_json() {
        let result = classify_poppy_body("not json", Some("https://example.com"));
        assert_eq!(result.status, Validity::Invalid);
    }

    #[test]
    fn poppy_no_origin() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "domain": "any.com" },
            "web": {}
        }).to_string();
        let result = classify_poppy_body(&body, None);
        assert_eq!(result.status, Validity::Valid);
    }

    #[test]
    fn poppy_subdomain_matches() {
        let body = serde_json::json!({
            "protocol_version": "0.1",
            "organization": { "domain": "api.acme.com" },
            "web": {}
        }).to_string();
        let result = classify_poppy_body(&body, Some("https://www.acme.com"));
        assert_eq!(result.status, Validity::Valid);
    }
}
