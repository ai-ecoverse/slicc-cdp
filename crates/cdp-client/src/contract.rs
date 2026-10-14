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

    fn test_only_modules(source: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut rest = source;
        while let Some(at) = rest.find("#[cfg(test)]\nmod ") {
            let tail = &rest[at + "#[cfg(test)]\nmod ".len()..];
            let name: String = tail
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if tail[name.len()..].starts_with(';') {
                names.push(name);
            }
            rest = tail;
        }
        names
    }

    fn production(source: &str) -> &str {
        let mut rest = source;
        let mut offset = 0;
        while let Some(at) = rest.find("\n#[cfg(test)]\nmod ") {
            let tail = &rest[at + "\n#[cfg(test)]\nmod ".len()..];
            let name_len = tail
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .count();
            if tail[name_len..].trim_start().starts_with('{') {
                return &source[..offset + at];
            }
            offset += at + 1;
            rest = &rest[at + 1..];
        }
        source
    }

    fn sources() -> Vec<(PathBuf, String)> {
        let mut out = Vec::new();
        let Ok(crates) = fs::read_dir(root().join("crates")) else {
            return out;
        };
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
                if skip.contains(&file) {
                    continue;
                }
                let text = fs::read_to_string(&file).unwrap();
                out.push((file, production(&text).to_string()));
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
            return i + 3 + bytes[i + 3..].iter().position(|b| *b == b'\'').map_or(0, |at| at + 1);
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

    fn sessionless_methods() -> BTreeSet<(String, String)> {
        let mut found = BTreeSet::new();
        for (file, source) in sources() {
            for call in CALLS {
                for (at, _) in source.match_indices(call) {
                    let before = source[..at].chars().next_back();
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    let args = arguments(&source, at + call.len() - 1);
                    let Some(method) = args.first().and_then(|arg| arg.strip_prefix('"')) else {
                        continue;
                    };
                    if args.get(2).map(String::as_str) == Some("None") {
                        let method = method.trim_end_matches('"').to_string();
                        found.insert((method, file.display().to_string()));
                    }
                }
            }
        }
        found
    }

    fn readme_section(heading: &str) -> Vec<String> {
        let readme = fs::read_to_string(root().join("README.md")).unwrap();
        let contract = readme
            .split("\n## Backend contract\n")
            .nth(1)
            .expect("README has a Backend contract section");
        let contract = contract.split("\n## ").next().unwrap();
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

    #[test]
    fn sessionless_calls_are_in_the_contract() {
        let found = sessionless_methods();
        assert!(found.iter().any(|(method, _)| method == "Target.attachToTarget"));
        let extra: Vec<_> = found
            .iter()
            .filter(|(method, _)| !BROWSER_METHODS.contains(&method.as_str()))
            .collect();
        assert!(extra.is_empty(), "sessionless methods missing from BROWSER_METHODS: {extra:?}");
    }

    #[test]
    fn every_contract_method_is_sent() {
        let found: BTreeSet<String> = sessionless_methods().into_iter().map(|(method, _)| method).collect();
        for method in BROWSER_METHODS {
            assert!(found.contains(*method), "{method} is in BROWSER_METHODS but never sent without a session");
        }
    }

    #[test]
    fn every_contract_event_is_handled() {
        let sources = sources();
        for event in BROWSER_EVENTS {
            let literal = format!("\"{event}\"");
            assert!(
                sources.iter().any(|(_, source)| source.contains(&literal)),
                "{event} is in BROWSER_EVENTS but no production code matches it"
            );
        }
    }

    #[test]
    fn readme_matches_the_contract() {
        assert_eq!(readme_section("Browser-level methods"), BROWSER_METHODS);
        assert_eq!(readme_section("Browser-level events"), BROWSER_EVENTS);
        assert_eq!(readme_section("Session-level domains"), SESSION_DOMAINS);
    }
}
