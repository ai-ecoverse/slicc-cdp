use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::classifier::Validity;

pub const EVENT_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Via {
    LinkHeader,
    HtmlLink,
    WellKnown,
    PageApi,
    Response,
    Catalog,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveryEvent {
    pub v: u8,
    pub standard: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    pub origin: String,
    pub url: String,
    pub via: Via,
    pub page: String,
    #[serde(rename = "targetId")]
    pub target_id: String,
    pub status: Validity,
    pub summary: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl DiscoveryEvent {
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_roundtrip() {
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://example.com".into(),
            url: "https://example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://example.com/shop".into(),
            target_id: "target1".into(),
            status: Validity::Valid,
            summary: serde_json::json!({"name": "test"}),
            reason: None,
        };
        let json = event.to_json();
        let parsed: DiscoveryEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, event);
    }

    #[test]
    fn via_serializes_as_kebab() {
        assert_eq!(
            serde_json::to_string(&Via::LinkHeader).unwrap(),
            r#""link-header""#
        );
        assert_eq!(
            serde_json::to_string(&Via::HtmlLink).unwrap(),
            r#""html-link""#
        );
        assert_eq!(
            serde_json::to_string(&Via::WellKnown).unwrap(),
            r#""well-known""#
        );
        assert_eq!(
            serde_json::to_string(&Via::PageApi).unwrap(),
            r#""page-api""#
        );
        assert_eq!(
            serde_json::to_string(&Via::Response).unwrap(),
            r#""response""#
        );
        assert_eq!(
            serde_json::to_string(&Via::Catalog).unwrap(),
            r#""catalog""#
        );
    }

    #[test]
    fn reason_omitted_when_none() {
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "test".into(),
            aliases: None,
            origin: "https://x.com".into(),
            url: "https://x.com/f".into(),
            via: Via::Response,
            page: "https://x.com/".into(),
            target_id: "t".into(),
            status: Validity::Valid,
            summary: serde_json::json!({}),
            reason: None,
        };
        let json = event.to_json();
        assert!(!json.contains("reason"));
    }

    #[test]
    fn reason_included_when_present() {
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "test".into(),
            aliases: None,
            origin: "https://x.com".into(),
            url: "https://x.com/f".into(),
            via: Via::Response,
            page: "https://x.com/".into(),
            target_id: "t".into(),
            status: Validity::Invalid,
            summary: serde_json::json!(null),
            reason: Some("bad json".into()),
        };
        let json = event.to_json();
        assert!(json.contains("\"reason\":\"bad json\""));
    }

    #[test]
    fn event_version_is_one() {
        assert_eq!(EVENT_VERSION, 1);
    }

    #[test]
    fn aliases_omitted_when_none() {
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: None,
            origin: "https://x.com".into(),
            url: "https://x.com/f".into(),
            via: Via::WellKnown,
            page: "https://x.com/".into(),
            target_id: "t".into(),
            status: Validity::Valid,
            summary: serde_json::json!({}),
            reason: None,
        };
        let json = event.to_json();
        assert!(!json.contains("aliases"));
    }

    #[test]
    fn aliases_included_when_present() {
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: Some(vec!["ai-catalog".into()]),
            origin: "https://x.com".into(),
            url: "https://x.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://x.com/".into(),
            target_id: "t".into(),
            status: Validity::Valid,
            summary: serde_json::json!({"name": "test"}),
            reason: None,
        };
        let json = event.to_json();
        assert!(json.contains("\"aliases\":[\"ai-catalog\"]"));
        let parsed: DiscoveryEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.aliases, Some(vec!["ai-catalog".to_string()]));
    }

    #[test]
    fn aliases_roundtrip() {
        let event = DiscoveryEvent {
            v: EVENT_VERSION,
            standard: "ard".into(),
            aliases: Some(vec!["ai-catalog".into()]),
            origin: "https://example.com".into(),
            url: "https://example.com/.well-known/ard.json".into(),
            via: Via::WellKnown,
            page: "https://example.com/".into(),
            target_id: "t".into(),
            status: Validity::Valid,
            summary: serde_json::json!({}),
            reason: None,
        };
        let json = event.to_json();
        let parsed: DiscoveryEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, event);
    }
}
