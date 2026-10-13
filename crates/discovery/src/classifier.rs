use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Validity {
    Valid,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Classification {
    pub standard: String,
    pub status: Validity,
    pub summary: Value,
    pub reason: Option<String>,
}

type ClassifierFn = fn(body: &str) -> Classification;

pub struct ClassifierRegistry {
    classifiers: Vec<(String, ClassifierFn)>,
}

impl ClassifierRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            classifiers: Vec::new(),
        }
    }

    pub fn register(&mut self, standard: &str, f: ClassifierFn) {
        self.classifiers.push((standard.to_string(), f));
    }

    #[must_use]
    pub fn classify(&self, standard: &str, body: &str) -> Option<Classification> {
        let (_, f) = self.classifiers.iter().find(|(s, _)| s == standard)?;
        Some(f(body))
    }
}

impl Default for ClassifierRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_classifier(body: &str) -> Classification {
        match serde_json::from_str::<Value>(body) {
            Ok(v) => Classification {
                standard: "test".into(),
                status: Validity::Valid,
                summary: v,
                reason: None,
            },
            Err(e) => Classification {
                standard: "test".into(),
                status: Validity::Invalid,
                summary: Value::Null,
                reason: Some(e.to_string()),
            },
        }
    }

    #[test]
    fn valid_body() {
        let mut reg = ClassifierRegistry::new();
        reg.register("test", dummy_classifier);
        let c = reg.classify("test", r#"{"key":"val"}"#).unwrap();
        assert_eq!(c.status, Validity::Valid);
        assert!(c.reason.is_none());
        assert_eq!(c.summary["key"], "val");
    }

    #[test]
    fn invalid_body_reported() {
        let mut reg = ClassifierRegistry::new();
        reg.register("test", dummy_classifier);
        let c = reg.classify("test", "not json").unwrap();
        assert_eq!(c.status, Validity::Invalid);
        assert!(c.reason.is_some());
    }

    #[test]
    fn unknown_standard() {
        let reg = ClassifierRegistry::new();
        assert!(reg.classify("missing", "").is_none());
    }
}
