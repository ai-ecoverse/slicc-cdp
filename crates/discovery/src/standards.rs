use crate::classifier::{Classification, ClassifierRegistry, Validity};
use crate::probe::{Probe, ProbeRegistry};
use serde_json::Value;

pub fn known_link_rels() -> &'static [&'static str] {
    &[
        "ard",
        "ai-catalog",
        "llms-txt",
        "api-catalog",
        "ucp",
        "describedby",
        "service-desc",
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
    registry
}

pub fn default_classifiers() -> ClassifierRegistry {
    let mut registry = ClassifierRegistry::new();
    registry.register("ard", classify_ard);
    registry
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
    fn default_probes_has_ard() {
        let registry = default_probes();
        let probes = registry.probes_for_origin("https://example.com");
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].probe.standard, "ard");
        assert_eq!(probes[0].url, "https://example.com/.well-known/ard.json");
    }

    #[test]
    fn default_classifiers_has_ard() {
        let registry = default_classifiers();
        let result = registry.classify("ard", r#"{"name":"Test"}"#).unwrap();
        assert_eq!(result.status, Validity::Valid);
        assert_eq!(result.summary["name"], "Test");
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
}
