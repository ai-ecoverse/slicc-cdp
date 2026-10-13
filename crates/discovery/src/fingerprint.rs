use std::collections::HashSet;

#[must_use]
pub fn fingerprint(origin: &str, standard: &str, url: &str) -> String {
    format!("{origin}\0{standard}\0{url}")
}

pub struct DedupSet {
    seen: HashSet<String>,
}

impl DedupSet {
    #[must_use]
    pub fn new() -> Self {
        Self {
            seen: HashSet::new(),
        }
    }

    pub fn is_new(&mut self, origin: &str, standard: &str, url: &str) -> bool {
        self.seen.insert(fingerprint(origin, standard, url))
    }
}

impl Default for DedupSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_format() {
        let fp = fingerprint("https://example.com", "ard", "https://example.com/.well-known/ard.json");
        assert_eq!(fp, "https://example.com\0ard\0https://example.com/.well-known/ard.json");
    }

    #[test]
    fn different_fields_differ() {
        let a = fingerprint("https://a.com", "ard", "/x");
        let b = fingerprint("https://b.com", "ard", "/x");
        assert_ne!(a, b);
    }

    #[test]
    fn dedup_first_is_new() {
        let mut ds = DedupSet::new();
        assert!(ds.is_new("https://example.com", "ard", "/x"));
    }

    #[test]
    fn dedup_second_is_not_new() {
        let mut ds = DedupSet::new();
        ds.is_new("https://example.com", "ard", "/x");
        assert!(!ds.is_new("https://example.com", "ard", "/x"));
    }

    #[test]
    fn dedup_different_standard_is_new() {
        let mut ds = DedupSet::new();
        ds.is_new("https://example.com", "ard", "/x");
        assert!(ds.is_new("https://example.com", "other", "/x"));
    }
}
