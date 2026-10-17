use serde_json::{json, Map, Value};

use crate::classifier::{Classification, Validity};

const KNOWN_VERSIONS: &[&str] = &["2026-01-23", "2026-04-08", "2026-08-25"];
const TRANSPORTS: &[&str] = &["rest", "mcp", "a2a", "embedded"];
const LIST_CAP: usize = 50;
const URL_CAP: usize = 500;
const NAME_CAP: usize = 200;

fn invalid(reason: impl Into<String>) -> Classification {
    Classification {
        standard: "ucp".into(),
        status: Validity::Invalid,
        summary: Value::Null,
        reason: Some(reason.into()),
    }
}

fn is_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes.iter().enumerate().all(|(i, b)| match i {
            4 | 7 => *b == b'-',
            _ => b.is_ascii_digit(),
        })
}

fn capped(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn clean_url(text: &str) -> String {
    let stripped = match url::Url::parse(text) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        Err(_) => text.to_string(),
    };
    capped(&stripped, URL_CAP)
}

fn names_ap2(name: &str) -> bool {
    name.split(['.', '/']).any(|segment| segment.starts_with("ap2"))
}

fn check_service(name: &str, index: usize, entry: &Value) -> Result<(String, Option<String>), String> {
    let at = format!("services.{name}[{index}]");
    for key in ["version", "spec", "transport"] {
        if entry.get(key).and_then(Value::as_str).is_none() {
            return Err(format!("{at} is missing {key}"));
        }
    }
    let transport = entry["transport"].as_str().unwrap_or("").to_string();
    if !TRANSPORTS.contains(&transport.as_str()) {
        return Err(format!("{at} has unknown transport {}", capped(&transport, NAME_CAP)));
    }
    let endpoint = entry.get("endpoint").and_then(Value::as_str).map(clean_url);
    let schema = entry.get("schema").and_then(Value::as_str);
    if endpoint.is_none() && (transport != "embedded" || schema.is_none()) {
        return Err(format!("{at} has no endpoint"));
    }
    Ok((transport, endpoint))
}

fn named_entries(value: &Value) -> Vec<(&String, &Value)> {
    value.as_object().map(|map| map.iter().take(LIST_CAP).collect()).unwrap_or_default()
}

pub fn classify_ucp(body: &str) -> Classification {
    let value: Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(err) => return invalid(err.to_string()),
    };
    let Some(ucp) = value.get("ucp").filter(|v| v.is_object()) else {
        return invalid("missing ucp");
    };
    let version = match ucp.get("version").and_then(Value::as_str) {
        Some(version) if is_date(version) => version.to_string(),
        Some(_) => return invalid("ucp.version is not YYYY-MM-DD"),
        None => return invalid("missing ucp.version"),
    };
    let Some(services) = ucp.get("services").filter(|v| v.is_object()) else {
        return invalid("missing ucp.services");
    };
    let Some(handlers) = ucp.get("payment_handlers").filter(|v| v.is_object()) else {
        return invalid("missing ucp.payment_handlers");
    };
    let mut service_summary = Map::new();
    let mut mcp = Vec::new();
    let mut a2a = Vec::new();
    for (name, entries) in named_entries(services) {
        let Some(entries) = entries.as_array() else {
            return invalid(format!("services.{} is not a list", capped(name, NAME_CAP)));
        };
        let mut listed = Vec::new();
        for (index, entry) in entries.iter().take(LIST_CAP).enumerate() {
            let (transport, endpoint) = match check_service(&capped(name, NAME_CAP), index, entry) {
                Ok(found) => found,
                Err(reason) => return invalid(reason),
            };
            if let Some(url) = &endpoint {
                if transport == "mcp" && mcp.len() < LIST_CAP {
                    mcp.push(Value::String(url.clone()));
                }
                if transport == "a2a" && a2a.len() < LIST_CAP {
                    a2a.push(Value::String(url.clone()));
                }
            }
            listed.push(json!({ "transport": transport, "endpoint": endpoint }));
        }
        service_summary.insert(capped(name, NAME_CAP), Value::Array(listed));
    }
    let mut capabilities = Vec::new();
    let mut ap2 = false;
    for (name, entries) in named_entries(ucp.get("capabilities").unwrap_or(&Value::Null)) {
        ap2 |= names_ap2(name);
        let versions: Vec<&str> = entries
            .as_array()
            .map(|list| list.iter().filter_map(|e| e.get("version").and_then(Value::as_str)).collect())
            .unwrap_or_default();
        if versions.is_empty() {
            capabilities.push(Value::String(capped(name, NAME_CAP)));
        }
        for v in versions {
            if capabilities.len() < LIST_CAP {
                capabilities.push(Value::String(format!("{}@{}", capped(name, NAME_CAP), capped(v, NAME_CAP))));
            }
        }
    }
    let payment_handlers: Vec<Value> = named_entries(handlers)
        .into_iter()
        .map(|(name, _)| {
            ap2 |= names_ap2(name);
            Value::String(capped(name, NAME_CAP))
        })
        .collect();
    let has_keys = value.get("keys").and_then(Value::as_array).is_some_and(|keys| !keys.is_empty());
    let mut summary = json!({
        "version": version,
        "services": service_summary,
        "capabilities": capabilities,
        "paymentHandlers": payment_handlers,
        "hasKeys": has_keys,
        "ap2": ap2,
        "mcp": mcp,
        "a2a": a2a,
    });
    if !KNOWN_VERSIONS.contains(&version.as_str()) {
        summary["note"] = Value::String("unknown version".into());
    }
    Classification {
        standard: "ucp".into(),
        status: Validity::Valid,
        summary,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(ucp: Value) -> String {
        json!({ "ucp": ucp }).to_string()
    }

    #[test]
    fn a_profile_with_every_transport_validates_and_surfaces_mcp_a2a_and_ap2() {
        let body = json!({
            "ucp": {
                "version": "2026-08-25",
                "services": { "dev.ucp.shopping": [
                    { "version": "2026-08-25", "spec": "s", "transport": "rest", "endpoint": "https://u:p@shop.test/ucp" },
                    { "version": "2026-08-25", "spec": "s", "transport": "mcp", "endpoint": "https://shop.test/mcp" },
                    { "version": "2026-08-25", "spec": "s", "transport": "a2a", "endpoint": "https://shop.test/agent-card.json" },
                    { "version": "2026-08-25", "spec": "s", "transport": "embedded", "schema": "https://x.test/e.json" }
                ]},
                "capabilities": {
                    "dev.ucp.shopping.checkout": [{ "version": "2026-08-25" }],
                    "dev.ucp.common.payment.ap2_mandate": [{ "version": "2026-08-25" }],
                    "dev.example.flag": {}
                },
                "payment_handlers": { "com.example.card": [{ "id": "c" }] }
            },
            "keys": [{ "kty": "OKP", "x": "secret-ish" }]
        })
        .to_string();
        let c = classify_ucp(&body);
        assert_eq!(c.status, Validity::Valid, "{:?}", c.reason);
        let s = &c.summary;
        assert_eq!(s["version"], "2026-08-25");
        assert_eq!(s["services"]["dev.ucp.shopping"][0]["endpoint"], "https://shop.test/ucp");
        assert_eq!(s["services"]["dev.ucp.shopping"][3]["endpoint"], Value::Null);
        assert_eq!(s["mcp"], json!(["https://shop.test/mcp"]));
        assert_eq!(s["a2a"], json!(["https://shop.test/agent-card.json"]));
        assert_eq!(s["capabilities"], json!(["dev.example.flag", "dev.ucp.common.payment.ap2_mandate@2026-08-25", "dev.ucp.shopping.checkout@2026-08-25"]));
        assert_eq!(s["paymentHandlers"], json!(["com.example.card"]));
        assert_eq!(s["hasKeys"], true);
        assert_eq!(s["ap2"], true);
        assert!(s.get("note").is_none());
        assert!(!s.to_string().contains("secret-ish"));
    }

    #[test]
    fn an_unknown_version_is_valid_with_a_note_and_empty_registries_are_fine() {
        let c = classify_ucp(&profile(json!({ "version": "2027-01-01", "services": {}, "payment_handlers": { "dev.x.ap2_handler": [] } })));
        assert_eq!(c.status, Validity::Valid);
        assert_eq!(c.summary["note"], "unknown version");
        assert_eq!(c.summary["ap2"], true);
        assert_eq!(c.summary["hasKeys"], false);
    }

    #[test]
    fn a_broken_profile_is_invalid_with_a_reason() {
        let ok = json!({ "version": "2026-08-25", "spec": "s", "transport": "rest", "endpoint": "e" });
        let cases = [
            ("{", "EOF"),
            ("{}", "missing ucp"),
            (&profile(json!({ "services": {}, "payment_handlers": {} })), "missing ucp.version"),
            (&profile(json!({ "version": "v1", "services": {}, "payment_handlers": {} })), "ucp.version is not YYYY-MM-DD"),
            (&profile(json!({ "version": "2026-08-25", "payment_handlers": {} })), "missing ucp.services"),
            (&profile(json!({ "version": "2026-08-25", "services": {} })), "missing ucp.payment_handlers"),
            (&profile(json!({ "version": "2026-08-25", "services": { "a": {} }, "payment_handlers": {} })), "services.a is not a list"),
            (&profile(json!({ "version": "2026-08-25", "services": { "a": [{ "spec": "s", "transport": "rest" }] }, "payment_handlers": {} })), "services.a[0] is missing version"),
            (&profile(json!({ "version": "2026-08-25", "services": { "a": [ok, { "version": "v", "spec": "s", "transport": "smtp", "endpoint": "e" }] }, "payment_handlers": {} })), "services.a[1] has unknown transport smtp"),
            (&profile(json!({ "version": "2026-08-25", "services": { "a": [{ "version": "v", "spec": "s", "transport": "rest" }] }, "payment_handlers": {} })), "services.a[0] has no endpoint"),
            (&profile(json!({ "version": "2026-08-25", "services": { "a": [{ "version": "v", "spec": "s", "transport": "embedded" }] }, "payment_handlers": {} })), "services.a[0] has no endpoint"),
        ];
        for (body, reason) in cases {
            let c = classify_ucp(body);
            assert_eq!(c.status, Validity::Invalid, "{body}");
            assert!(c.reason.as_deref().unwrap_or("").contains(reason), "{body}: {:?}", c.reason);
        }
    }

    #[test]
    fn long_names_and_urls_are_capped_and_unparsable_urls_kept() {
        let long = "n".repeat(300);
        let body = profile(json!({
            "version": "2026-08-25",
            "services": { long.clone(): [{ "version": "v", "spec": "s", "transport": "mcp", "endpoint": format!("not a url {}", "é".repeat(400)) }] },
            "payment_handlers": {}
        }));
        let c = classify_ucp(&body);
        let endpoint = c.summary["mcp"][0].as_str().unwrap();
        assert!(endpoint.len() <= URL_CAP && endpoint.starts_with("not a url"));
        assert_eq!(c.summary["services"].as_object().unwrap().keys().next().unwrap().len(), NAME_CAP);
        assert!(is_date("2026-08-25") && !is_date("2026-8-25") && !is_date("2026x08-25"));
    }
}
