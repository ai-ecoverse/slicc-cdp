pub const BROWSER_METHODS: &[&str] = &[
    "Target.getTargets",
    "Target.getTargetInfo",
    "Target.setDiscoverTargets",
    "Target.createTarget",
    "Target.attachToTarget",
    "Target.detachFromTarget",
    "Target.closeTarget",
    "Target.activateTarget",
    "Browser.getVersion",
    "Slicc.action",
];

pub const BROWSER_EVENTS: &[&str] = &[
    "Target.attachedToTarget",
    "Target.targetCreated",
    "Target.targetInfoChanged",
    "Target.targetDestroyed",
    "Target.detachedFromTarget",
];

pub const SESSION_DOMAINS: &[&str] = &[
    "Page",
    "Runtime",
    "DOM",
    "Input",
    "Network",
    "Fetch",
    "Accessibility",
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    const CALLS: &[&str] = &["call(", "call_for(", "raw_call(", "send(", "notify("];
    const BROWSER_DOMAINS: &[&str] = &["Target", "Browser", "Slicc"];

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    fn module_name(tail: &str) -> &str {
        let len = tail
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(tail.len());
        &tail[..len]
    }

    fn test_only_modules(source: &str) -> Vec<String> {
        let marker = "#[cfg(test)]\nmod ";
        source
            .match_indices(marker)
            .map(|(at, _)| &source[at + marker.len()..])
            .filter(|tail| tail[module_name(tail).len()..].starts_with(';'))
            .map(|tail| module_name(tail).to_string())
            .collect()
    }

    fn production(source: &str) -> &str {
        let marker = "\n#[cfg(test)]\nmod ";
        for (at, _) in source.match_indices(marker) {
            let tail = &source[at + marker.len()..];
            if tail[module_name(tail).len()..]
                .trim_start()
                .starts_with('{')
            {
                return &source[..at];
            }
        }
        source
    }

    fn sources() -> Vec<(String, String)> {
        let mut out = Vec::new();
        let crates = fs::read_dir(root().join("crates")).unwrap();
        for krate in crates.flatten() {
            let src = krate.path().join("src");
            let mut skip = BTreeSet::new();
            for entry in ["lib.rs", "main.rs"] {
                if let Ok(text) = fs::read_to_string(src.join(entry)) {
                    for name in test_only_modules(&text) {
                        skip.insert(src.join(format!("{name}.rs")));
                    }
                }
            }
            let mut files = Vec::new();
            rust_files(&src, &mut files);
            for file in files {
                if skip.contains(&file) || file.ends_with("cdp-client/src/contract.rs") {
                    continue;
                }
                let text = fs::read_to_string(&file).unwrap();
                let name = file
                    .strip_prefix(root())
                    .unwrap_or(&file)
                    .display()
                    .to_string();
                out.push((name, production(&text).to_string()));
            }
        }
        out
    }

    fn skip_literal(bytes: &[u8], mut i: usize) -> usize {
        if bytes[i] == b'r' {
            let mut hashes = 0;
            i += 1;
            while bytes[i] == b'#' {
                hashes += 1;
                i += 1;
            }
            i += 1;
            loop {
                if bytes[i] == b'"' && bytes[i + 1..].iter().take(hashes).all(|b| *b == b'#') {
                    return i + 1 + hashes;
                }
                i += 1;
            }
        }
        i += 1;
        while bytes[i] != b'"' {
            if bytes[i] == b'\\' {
                i += 1;
            }
            i += 1;
        }
        i + 1
    }

    fn skip_char(bytes: &[u8], i: usize) -> usize {
        if bytes.get(i + 1) == Some(&b'\\') {
            return i
                + 3
                + bytes[i + 3..]
                    .iter()
                    .position(|b| *b == b'\'')
                    .map_or(0, |at| at + 1);
        }
        if bytes.get(i + 2) == Some(&b'\'') {
            return i + 3;
        }
        i + 1
    }

    fn raw_start(bytes: &[u8], i: usize) -> bool {
        bytes[i] == b'r'
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_'))
            && bytes[i + 1..]
                .iter()
                .find(|b| **b != b'#')
                .is_some_and(|b| *b == b'"')
    }

    fn arguments(source: &str, open: usize) -> Vec<String> {
        let bytes = source.as_bytes();
        let mut args = Vec::new();
        let mut depth = 0;
        let mut start = open + 1;
        let mut i = open;
        while i < bytes.len() {
            if bytes[i] == b'"' || raw_start(bytes, i) {
                i = skip_literal(bytes, i);
                continue;
            }
            if bytes[i] == b'\'' {
                i = skip_char(bytes, i);
                continue;
            }
            match bytes[i] {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        args.push(source[start..i].trim().to_string());
                        return args;
                    }
                }
                b',' if depth == 1 => {
                    args.push(source[start..i].trim().to_string());
                    start = i + 1;
                }
                _ => {}
            }
            i += 1;
        }
        args
    }

    fn line_of(source: &str, at: usize) -> usize {
        source[..at].matches('\n').count() + 1
    }

    fn sent() -> Vec<(String, bool, String)> {
        let mut found = Vec::new();
        for (file, source) in sources() {
            for call in CALLS {
                for (at, _) in source.match_indices(call) {
                    let before = source[..at].chars().next_back();
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let args = arguments(&source, at + call.len() - 1);
                    let Some(method) = args
                        .first()
                        .and_then(|arg| arg.strip_prefix('"'))
                        .and_then(|arg| arg.strip_suffix('"'))
                    else {
                        continue;
                    };
                    let sessionless = args.get(2).map(String::as_str) == Some("None");
                    let place = format!("{file}:{}", line_of(&source, at));
                    found.push((method.to_string(), sessionless, place));
                }
            }
        }
        found
    }

    fn event_name(name: &str) -> bool {
        let mut pieces = Vec::new();
        let mut from = 0;
        for (at, _) in name.match_indices(|c: char| c.is_ascii_uppercase()) {
            pieces.push(&name[from..at]);
            from = at;
        }
        pieces.push(&name[from..]);
        pieces
            .iter()
            .any(|piece| piece.len() > 2 && piece.ends_with("ed"))
    }

    fn literals(source: &str) -> Vec<(String, usize)> {
        let mut out = Vec::new();
        for (at, _) in source.match_indices('"') {
            let rest = &source[at + 1..];
            let Some(end) = rest.find('"') else { continue };
            let text = &rest[..end];
            let Some((domain, name)) = text.split_once('.') else {
                continue;
            };
            if domain
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_uppercase())
                && domain.chars().all(|c| c.is_ascii_alphabetic())
                && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && name.chars().all(|c| c.is_ascii_alphanumeric())
            {
                out.push((text.to_string(), at));
            }
        }
        out
    }

    fn handled_events() -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        for (_, source) in sources() {
            for (literal, _) in literals(&source) {
                let (domain, name) = literal.split_once('.').unwrap();
                if BROWSER_DOMAINS.contains(&domain) && event_name(name) {
                    found.insert(literal);
                }
            }
        }
        found
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn contract() -> String {
        let readme = fs::read_to_string(root().join("README.md")).unwrap();
        let contract = readme
            .split("\n## Backend contract\n")
            .nth(1)
            .expect("README has a Backend contract section");
        contract.split("\n## ").next().unwrap().to_string()
    }

    fn readme_section(heading: &str) -> Vec<String> {
        let contract = contract();
        let section = contract
            .split(&format!("\n### {heading}\n"))
            .nth(1)
            .unwrap_or_else(|| panic!("Backend contract has ### {heading}"));
        let section = section.split("\n### ").next().unwrap();
        section
            .lines()
            .filter_map(|line| line.strip_prefix("- `"))
            .map(|line| line.split('`').next().unwrap().to_string())
            .collect()
    }

    fn refused_by_shield() -> BTreeSet<String> {
        let source = fs::read_to_string(root().join("crates/cdp-client/src/shield.rs")).unwrap();
        let start = source.find("pub fn browser_wide(").unwrap();
        let body = &source[start..];
        let body = &body[..body.find("\n}\n").unwrap()];
        let mut found: BTreeSet<String> = literals(body)
            .into_iter()
            .map(|(literal, _)| literal)
            .collect();
        let marker = "method.starts_with(\"";
        for (at, _) in body.match_indices(marker) {
            let rest = &body[at + marker.len()..];
            found.insert(format!("{}*", &rest[..rest.find('"').unwrap()]));
        }
        found
    }

    #[test]
    fn sessionless_calls_match_browser_methods() {
        let found = sent();
        let methods: BTreeSet<String> = found
            .iter()
            .filter(|(_, sessionless, _)| *sessionless)
            .map(|(method, _, _)| method.clone())
            .collect();
        let missing: Vec<_> = found
            .iter()
            .filter(|(method, sessionless, _)| {
                *sessionless && !BROWSER_METHODS.contains(&method.as_str())
            })
            .map(|(method, _, place)| format!("{method} at {place}"))
            .collect();
        assert!(
            missing.is_empty(),
            "sessionless calls missing from BROWSER_METHODS: {missing:?}"
        );
        assert_eq!(
            methods,
            set(BROWSER_METHODS),
            "BROWSER_METHODS names a method no code sends without a session"
        );
    }

    #[test]
    fn session_calls_match_session_domains() {
        let domains: BTreeSet<String> = sent()
            .into_iter()
            .filter(|(_, sessionless, _)| !sessionless)
            .map(|(method, _, _)| method.split('.').next().unwrap().to_string())
            .filter(|domain| !BROWSER_DOMAINS.contains(&domain.as_str()))
            .collect();
        assert_eq!(domains, set(SESSION_DOMAINS));
    }

    #[test]
    fn handled_events_match_browser_events() {
        assert_eq!(handled_events(), set(BROWSER_EVENTS));
    }

    #[test]
    fn event_names_follow_the_cdp_convention() {
        for name in [
            "targetCreated",
            "attachedToTarget",
            "detachedFromTarget",
            "targetInfoChanged",
        ] {
            assert!(event_name(name), "{name}");
        }
        for name in [
            "attachToTarget",
            "setDiscoverTargets",
            "getTargetInfo",
            "createBrowserContext",
            "action",
        ] {
            assert!(!event_name(name), "{name}");
        }
    }

    #[test]
    fn readme_matches_the_contract() {
        assert_eq!(readme_section("Browser-level methods"), BROWSER_METHODS);
        assert_eq!(readme_section("Browser-level events"), BROWSER_EVENTS);
        assert_eq!(readme_section("Session-level domains"), SESSION_DOMAINS);
    }

    #[test]
    fn readme_lists_what_the_shield_refuses() {
        let listed: BTreeSet<String> = readme_section("Refused before sending")
            .into_iter()
            .collect();
        let shield = refused_by_shield();
        assert!(shield.contains("Network.clearBrowserCookies"));
        assert_eq!(
            listed
                .intersection(&shield)
                .cloned()
                .collect::<BTreeSet<_>>(),
            shield
        );
    }

    fn source_path(token: &str) -> bool {
        token.contains('/') && (token.ends_with(".rs") || token.ends_with(".js"))
    }

    fn item_body(text: &str, name: &str) -> Option<String> {
        let lines: Vec<&str> = text.lines().collect();
        let starts = |line: &str| {
            let line = line.trim_start();
            let line = line.strip_prefix("pub ").unwrap_or(line);
            let line = line.strip_prefix("async ").unwrap_or(line);
            ["fn ", "const ", "function "].iter().any(|kind| {
                line.strip_prefix(kind).is_some_and(|rest| {
                    rest.strip_prefix(name)
                        .is_some_and(|rest| rest.starts_with(['(', ':', ' ', '<']))
                })
            }) || line
                .strip_prefix(name)
                .is_some_and(|rest| rest.starts_with('('))
                && line.trim_end().ends_with('{')
        };
        let at = lines.iter().position(|line| starts(line))?;
        let indent = lines[at].len() - lines[at].trim_start().len();
        if lines[at].trim_end().ends_with(';') {
            return Some(lines[at].to_string());
        }
        let end = lines[at + 1..]
            .iter()
            .position(|line| {
                line.len() - line.trim_start().len() == indent && line.trim_start().starts_with('}')
            })
            .map_or(lines.len(), |end| at + 1 + end);
        Some(lines[at..=end.min(lines.len() - 1)].join("\n"))
    }

    #[test]
    fn readme_citations_point_at_their_code() {
        let contract = contract();
        let mut checked = 0;
        let mut listed = false;
        for line in contract.lines() {
            if let Some(heading) = line.strip_prefix("### ") {
                listed = heading.starts_with("Browser-level");
            }
            let mut names: Vec<String> = Vec::new();
            let head = line
                .strip_prefix("- `")
                .and_then(|rest| rest.split('`').next())
                .filter(|_| listed)
                .map(str::to_string);
            let tokens: Vec<&str> = line.split('`').skip(1).step_by(2).collect();
            let mut i = 0;
            while i < tokens.len() {
                let token = tokens[i];
                let item = tokens.get(i + 1).copied().filter(|item| {
                    !item.is_empty() && item.chars().all(|c| c.is_alphanumeric() || c == '_')
                });
                let (true, Some(item)) = (source_path(token), item) else {
                    names.extend(
                        token
                            .split(|c: char| !(c.is_alphanumeric() || c == '.' || c == '_'))
                            .filter(|name| name.len() > 3)
                            .map(str::to_string),
                    );
                    i += 1;
                    continue;
                };
                let text = fs::read_to_string(root().join(token))
                    .unwrap_or_else(|_| panic!("README cites {token}, which does not exist"));
                let body = item_body(&text, item).unwrap_or_else(|| {
                    panic!("README cites {item} in {token}, which has no such item")
                });
                let found = match &head {
                    Some(head) => body.contains(head.as_str()),
                    None => {
                        names.is_empty() || names.iter().any(|name| body.contains(name.as_str()))
                    }
                };
                assert!(
                    found,
                    "README cites {token} {item} for {:?}, but it holds none of them",
                    head.as_ref()
                        .map_or(names.clone(), |head| vec![head.clone()])
                );
                names.clear();
                checked += 1;
                i += 2;
            }
        }
        assert!(checked >= BROWSER_METHODS.len());
    }
}
