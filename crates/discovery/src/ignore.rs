pub struct IgnoreList {
    rules: Vec<IgnoreRule>,
}

struct IgnoreRule {
    standard: Option<String>,
    pattern: String,
}

impl IgnoreList {
    #[must_use]
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn parse(text: &str) -> Self {
        let mut rules = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = trimmed.splitn(2, char::is_whitespace).collect();
            if parts.len() == 2 {
                let standard = parts[0].trim();
                let glob = parts[1].trim().to_ascii_lowercase();
                if standard == "*" {
                    rules.push(IgnoreRule {
                        standard: None,
                        pattern: glob,
                    });
                } else {
                    rules.push(IgnoreRule {
                        standard: Some(standard.to_ascii_lowercase()),
                        pattern: glob,
                    });
                }
            } else {
                rules.push(IgnoreRule {
                    standard: None,
                    pattern: trimmed.to_ascii_lowercase(),
                });
            }
        }
        Self { rules }
    }

    #[must_use]
    pub fn is_ignored(&self, hostname: &str, standard: Option<&str>) -> bool {
        let host_lower = hostname.to_ascii_lowercase();
        for rule in &self.rules {
            if !glob_match(&rule.pattern, &host_lower) {
                continue;
            }
            match (&rule.standard, standard) {
                (None, _) => return true,
                (Some(rs), Some(s)) if rs == &s.to_ascii_lowercase() => return true,
                _ => {}
            }
        }
        false
    }
}

impl Default for IgnoreList {
    fn default() -> Self {
        Self::new()
    }
}

fn glob_match(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    glob_match_inner(&pat, &txt)
}

fn glob_match_inner(pat: &[char], txt: &[char]) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let mut star_pi = usize::MAX;
    let mut star_ti = 0;

    while ti < txt.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == txt[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if star_pi != usize::MAX {
            pi = star_pi + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }

    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }

    pi == pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_ignores_nothing() {
        let list = IgnoreList::new();
        assert!(!list.is_ignored("example.com", None));
    }

    #[test]
    fn exact_hostname() {
        let list = IgnoreList::parse("* example.com");
        assert!(list.is_ignored("example.com", None));
        assert!(!list.is_ignored("other.com", None));
    }

    #[test]
    fn wildcard_subdomain() {
        let list = IgnoreList::parse("* *.example.com");
        assert!(list.is_ignored("sub.example.com", None));
        assert!(!list.is_ignored("example.com", None));
    }

    #[test]
    fn case_insensitive() {
        let list = IgnoreList::parse("* Example.COM");
        assert!(list.is_ignored("EXAMPLE.com", None));
    }

    #[test]
    fn comments_and_blanks() {
        let list = IgnoreList::parse("# a comment\n\n* example.com\n");
        assert!(list.is_ignored("example.com", None));
    }

    #[test]
    fn standard_specific_rule() {
        let list = IgnoreList::parse("ard example.com");
        assert!(list.is_ignored("example.com", Some("ard")));
        assert!(!list.is_ignored("example.com", Some("other")));
        assert!(!list.is_ignored("example.com", None));
    }

    #[test]
    fn bare_hostname_ignores_all_standards() {
        let list = IgnoreList::parse("example.com");
        assert!(list.is_ignored("example.com", None));
        assert!(list.is_ignored("example.com", Some("ard")));
    }

    #[test]
    fn glob_match_star() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*.com", "example.com"));
        assert!(!glob_match("*.com", "example.org"));
    }

    #[test]
    fn glob_match_question() {
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "abbc"));
    }
}
