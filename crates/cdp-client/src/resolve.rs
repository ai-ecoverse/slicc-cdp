use crate::connect::{choose_start, Start};

pub const AUTO: &str = "auto";

pub const CONNECT_ORDER: &str = "Connect, in order: --cdp <url> wins; else SLICC_CDP_URL; else the slicc browsers JSON listing when slicc is on PATH (prefer sevenOpen, then a slicc profile, then best tier; loopback ws only); else a live DevToolsActivePort under slicc seven-profiles, legacy profiles, and SLICC six profile dirs (loopback /json/version check, websocket pathname must match the file, newest live wins); else a loopback endpoint playwright-cli saved for this directory under the user config dir (curlwright reads it and never writes one); else the first of 127.0.0.1:9222-9229 whose /json/version answers. A .playwright-cli/session.json in the working directory is ignored. Saved state never dials a non-loopback host. The wasm build uses port 9222 and skips every step after SLICC_CDP_URL.";

pub const CDP_FORMS: &str = "--cdp and SLICC_CDP_URL take a port, host[:port], [ipv6][:port], ws://, wss://, http://, or https://. A ws:// or wss:// URL with a path is dialed directly. An http:// URL, a path-less ws:// URL, a port, or host[:port] is /json/version discovery. The debugger path is opaque. A webSocketDebuggerUrl from discovery is dialed only when it is ws:// or wss:// on this machine (for discovery on this machine) or on the host asked; anything else is refused before dialing. \"auto\" (or an empty value) skips that step, so --cdp auto ignores SLICC_CDP_URL.";

pub const CDP_REPORT: &str = "--verbose prints \"* CDP <endpoint> (via <rule>)\" to stderr before connecting. When nothing is found, the error lists every step tried and why it was passed over.";

pub fn help_block(indent: &str, width: usize) -> String {
    [CONNECT_ORDER, CDP_FORMS, CDP_REPORT]
        .iter()
        .map(|text| wrap(text, indent, width))
        .collect::<Vec<_>>()
        .join("\n")
}

fn wrap(text: &str, indent: &str, width: usize) -> String {
    let mut out = String::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && indent.len() + line.len() + 1 + word.len() > width {
            out.push_str(&format!("{indent}{line}\n"));
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push_str(&format!("{indent}{line}\n"));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Flag,
    Env,
    SliccBrowsers,
    ActivePort,
    Saved,
    Probe,
    Launch,
    Wasm,
}

impl Rule {
    pub fn label(self) -> &'static str {
        match self {
            Rule::Flag => "--cdp",
            Rule::Env => "SLICC_CDP_URL",
            Rule::SliccBrowsers => "slicc browsers",
            Rule::ActivePort => "DevToolsActivePort",
            Rule::Saved => "saved endpoint",
            Rule::Probe => "port probe",
            Rule::Launch => "launched browser",
            Rule::Wasm => "wasm default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub rule: Rule,
    pub note: String,
}

impl Step {
    pub fn new(rule: Rule, note: impl Into<String>) -> Self {
        Self { rule, note: note.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub start: Start,
    pub rule: Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub choice: Option<Choice>,
    pub trail: Vec<Step>,
}

pub trait Finder {
    fn attached(&mut self, trail: &mut Vec<Step>) -> Result<Option<(Rule, String)>, String>;
    fn saved(&mut self, trail: &mut Vec<Step>) -> Option<String>;
    fn probe(&mut self, trail: &mut Vec<Step>) -> Option<String>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listed {
    pub attached: Option<(Rule, String)>,
    pub saved: Option<String>,
    pub probe: Option<String>,
}

impl Finder for Listed {
    fn attached(&mut self, trail: &mut Vec<Step>) -> Result<Option<(Rule, String)>, String> {
        if self.attached.is_none() {
            trail.push(Step::new(Rule::SliccBrowsers, "slicc not on PATH, or it failed"));
            trail.push(Step::new(Rule::ActivePort, "no live file under slicc seven-profiles, profiles, or six profiles"));
        }
        Ok(self.attached.clone())
    }

    fn saved(&mut self, trail: &mut Vec<Step>) -> Option<String> {
        if self.saved.is_none() {
            trail.push(Step::new(Rule::Saved, "none for this directory"));
        }
        self.saved.clone()
    }

    fn probe(&mut self, trail: &mut Vec<Step>) -> Option<String> {
        if self.probe.is_none() {
            trail.push(Step::new(Rule::Probe, "nothing answered"));
        }
        self.probe.clone()
    }
}

pub fn explicit(value: Option<&str>) -> Option<&str> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty() && !text.eq_ignore_ascii_case(AUTO))
}

pub fn is_explicit(cdp: Option<&str>, env: Option<&str>) -> bool {
    explicit(cdp).is_some() || (!skips_env(cdp) && explicit(env).is_some())
}

fn skips_env(cdp: Option<&str>) -> bool {
    cdp.map(str::trim).is_some_and(|text| text.eq_ignore_ascii_case(AUTO))
}

pub fn explicit_start(cdp: Option<&str>, env: Option<&str>) -> Option<Result<Choice, String>> {
    if let Some(url) = explicit(cdp) {
        return Some(choose_start(Some(url), None).map(|start| Choice { start, rule: Rule::Flag }));
    }
    if skips_env(cdp) {
        return None;
    }
    explicit(env).map(|url| choose_start(Some(url), None).map(|start| Choice { start, rule: Rule::Env }))
}

pub fn resolve(cdp: Option<&str>, env: Option<&str>, finder: &mut dyn Finder) -> Result<Resolved, String> {
    resolve_for(cfg!(target_arch = "wasm32"), cdp, env, finder)
}

pub fn resolve_for(
    wasm: bool,
    cdp: Option<&str>,
    env: Option<&str>,
    finder: &mut dyn Finder,
) -> Result<Resolved, String> {
    let mut trail = Vec::new();
    if let Some(choice) = explicit_start(cdp, env) {
        return Ok(Resolved { choice: Some(choice?), trail });
    }
    trail.push(Step::new(Rule::Flag, absent(cdp, "not given")));
    let env_note = if skips_env(cdp) && explicit(env).is_some() {
        "set, skipped by --cdp auto".to_string()
    } else {
        absent(env, "not set")
    };
    trail.push(Step::new(Rule::Env, env_note));
    if wasm {
        let start = choose_start(None, None)?;
        return Ok(Resolved { choice: Some(Choice { start, rule: Rule::Wasm }), trail });
    }
    if let Some((rule, url)) = finder.attached(&mut trail)? {
        return Ok(Resolved { choice: Some(Choice { start: Start::Discover(url), rule }), trail });
    }
    if let Some(url) = finder.saved(&mut trail) {
        let start = choose_start(Some(&url), None)?;
        return Ok(Resolved { choice: Some(Choice { start, rule: Rule::Saved }), trail });
    }
    if let Some(url) = finder.probe(&mut trail) {
        return Ok(Resolved { choice: Some(Choice { start: Start::Discover(url), rule: Rule::Probe }), trail });
    }
    Ok(Resolved { choice: None, trail })
}

fn absent(value: Option<&str>, missing: &str) -> String {
    match value.map(str::trim) {
        Some(text) if text.eq_ignore_ascii_case(AUTO) => "auto".to_string(),
        _ => missing.to_string(),
    }
}

pub fn endpoint(start: &Start) -> &str {
    match start {
        Start::Direct(url) | Start::Discover(url) => url,
    }
}

pub fn describe(choice: &Choice, full_urls: bool) -> String {
    let url = endpoint(&choice.start);
    let shown = if full_urls { url.to_string() } else { crate::sanitize::redact_url(url) };
    format!("* CDP {shown} (via {})\n", choice.rule.label())
}

pub fn not_found(tool: &str, trail: &[Step]) -> String {
    let mut text = format!("{tool}: no CDP browser found. Tried, in order:\n");
    for step in trail {
        text.push_str(&format!("  {:<19} {}\n", step.rule.label(), step.note));
    }
    text.push_str("Pass --cdp <port|host:port|ws://...|http://...>, set SLICC_CDP_URL, or start a browser with --remote-debugging-port.\n");
    text
}

pub fn probe_ports(
    ports: &[u16],
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
    trail: &mut Vec<Step>,
) -> Option<String> {
    if let Some(url) = crate::ports::first_live(ports, probe) {
        return Some(url);
    }
    trail.push(Step::new(Rule::Probe, format!("nothing answered on 127.0.0.1:{}", port_range(ports))));
    None
}

pub fn port_range(ports: &[u16]) -> String {
    match (ports.first(), ports.last()) {
        (Some(first), Some(last)) if first == last => first.to_string(),
        (Some(first), Some(last)) => format!("{first}-{last}"),
        _ => "(no ports)".to_string(),
    }
}

pub fn known_provenance(text: &str) -> bool {
    matches!(text, "explicit" | "seven-profile" | "probe")
}

pub fn usable_endpoint(endpoint: Option<&str>, provenance: Option<&str>) -> Option<String> {
    let url = endpoint.filter(|text| !text.is_empty())?;
    if !known_provenance(provenance?) {
        return None;
    }
    if !loopback_url(url) {
        return None;
    }
    Some(url.to_string())
}

pub fn reuse_saved(endpoint: Option<&str>, provenance: Option<&str>, version_body: Option<&str>) -> Option<String> {
    let url = usable_endpoint(endpoint, provenance)?;
    if provenance == Some("probe") {
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return None;
        }
        crate::connect::websocket_url_from_version(version_body?).ok()?;
    }
    Some(url)
}

pub fn saved_from_json(
    text: Option<&str>,
    probe: &mut dyn FnMut(&str) -> Result<String, String>,
    trail: &mut Vec<Step>,
) -> Option<String> {
    let Some(text) = text else {
        trail.push(Step::new(Rule::Saved, "none for this directory"));
        return None;
    };
    let value: serde_json::Value = serde_json::from_str(text).unwrap_or(serde_json::Value::Null);
    let endpoint = value.get("endpoint").and_then(|item| item.as_str()).filter(|text| !text.is_empty());
    let provenance = value.get("provenance").and_then(|item| item.as_str());
    let Some(url) = usable_endpoint(endpoint, provenance) else {
        let note = if endpoint.is_some() { "saved endpoint is not loopback or has no known provenance" } else { "none for this directory" };
        trail.push(Step::new(Rule::Saved, note));
        return None;
    };
    let body = if provenance == Some("probe") { probe(&url).ok() } else { None };
    let reused = reuse_saved(Some(&url), provenance, body.as_deref());
    if reused.is_none() {
        trail.push(Step::new(Rule::Saved, format!("{} no longer answers", crate::sanitize::redact_url(&url))));
    }
    reused
}

pub fn loopback_url(url: &str) -> bool {
    let Ok(endpoint) = crate::net::parse_endpoint(url) else {
        return false;
    };
    matches!(endpoint.scheme.as_str(), "http" | "https" | "ws" | "wss") && crate::ports::local_host(&endpoint.host)
}

pub fn suggest<'a>(token: &str, known: &[&'a str]) -> Option<&'a str> {
    let (name, _) = token.split_once('=').unwrap_or((token, ""));
    if name.len() < 3 {
        return None;
    }
    known
        .iter()
        .filter(|candidate| **candidate != name)
        .map(|candidate| (distance(name, candidate), *candidate))
        .filter(|(score, candidate)| *score <= if candidate.len() > 5 { 2 } else { 1 })
        .min_by_key(|(score, _)| *score)
        .map(|(_, candidate)| candidate)
}

fn distance(left: &str, right: &str) -> usize {
    let a: Vec<char> = left.chars().collect();
    let b: Vec<char> = right.chars().collect();
    let mut table = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in table.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        table[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (table[i - 1][j] + 1).min(table[i][j - 1] + 1).min(table[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(table[i - 2][j - 2] + 1);
            }
            table[i][j] = best;
        }
    }
    table[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        slicc: Option<String>,
        active: Option<String>,
        saved: Option<String>,
        probe: Option<String>,
        calls: Vec<&'static str>,
    }

    impl Finder for Fake {
        fn attached(&mut self, trail: &mut Vec<Step>) -> Result<Option<(Rule, String)>, String> {
            self.calls.push("attached");
            if let Some(url) = self.slicc.clone() {
                return Ok(Some((Rule::SliccBrowsers, url)));
            }
            trail.push(Step::new(Rule::SliccBrowsers, "slicc not on PATH or failed"));
            if let Some(url) = self.active.clone() {
                return Ok(Some((Rule::ActivePort, url)));
            }
            trail.push(Step::new(Rule::ActivePort, "no live file"));
            Ok(None)
        }

        fn saved(&mut self, trail: &mut Vec<Step>) -> Option<String> {
            self.calls.push("saved");
            if self.saved.is_none() {
                trail.push(Step::new(Rule::Saved, "none for this directory"));
            }
            self.saved.clone()
        }

        fn probe(&mut self, trail: &mut Vec<Step>) -> Option<String> {
            self.calls.push("probe");
            if self.probe.is_none() {
                trail.push(Step::new(Rule::Probe, "nothing answered on 127.0.0.1:9300-9309"));
            }
            self.probe.clone()
        }
    }

    fn version(port: u16) -> String {
        format!("http://127.0.0.1:{port}/json/version")
    }

    fn all_running() -> Fake {
        Fake {
            slicc: Some(version(40101)),
            active: Some(version(40102)),
            saved: Some(version(40103)),
            probe: Some(version(40104)),
            calls: Vec::new(),
        }
    }

    fn picked(fake: &mut Fake, cdp: Option<&str>, env: Option<&str>) -> Choice {
        resolve_for(false, cdp, env, fake).unwrap().choice.unwrap()
    }

    #[test]
    fn with_every_source_live_the_order_decides() {
        let mut fake = all_running();
        let flag = picked(&mut fake, Some("40100"), Some("40199"));
        assert_eq!(flag, Choice { start: Start::Discover("http://localhost:40100/json/version".into()), rule: Rule::Flag });
        let env = picked(&mut fake, None, Some("ws://127.0.0.1:40199/devtools/browser/x"));
        assert_eq!(env, Choice { start: Start::Direct("ws://127.0.0.1:40199/devtools/browser/x".into()), rule: Rule::Env });
        assert!(fake.calls.is_empty());
        let mut fake = all_running();
        assert_eq!(picked(&mut fake, None, None), Choice { start: Start::Discover(version(40101)), rule: Rule::SliccBrowsers });
        fake.slicc = None;
        assert_eq!(picked(&mut fake, None, None).rule, Rule::ActivePort);
        fake.active = None;
        assert_eq!(picked(&mut fake, None, None), Choice { start: Start::Discover(version(40103)), rule: Rule::Saved });
        fake.saved = None;
        assert_eq!(picked(&mut fake, None, None), Choice { start: Start::Discover(version(40104)), rule: Rule::Probe });
        assert_eq!(fake.calls, ["attached", "attached", "attached", "saved", "attached", "saved", "probe"]);
    }

    #[test]
    fn auto_skips_the_explicit_steps() {
        let mut fake = all_running();
        assert_eq!(picked(&mut fake, Some("auto"), Some("40199")).rule, Rule::SliccBrowsers);
        assert_eq!(picked(&mut fake, Some(" AUTO "), None).rule, Rule::SliccBrowsers);
        assert_eq!(picked(&mut fake, None, Some("auto")).rule, Rule::SliccBrowsers);
        assert_eq!(picked(&mut fake, Some(""), Some("40199")).rule, Rule::Env);
        assert!(!is_explicit(Some("auto"), Some("40199")));
        assert!(is_explicit(None, Some("40199")));
        assert!(is_explicit(Some("9333"), None));
        assert!(!is_explicit(Some(" "), Some("auto")));
    }

    #[test]
    fn nothing_found_lists_every_step() {
        let mut fake = Fake::default();
        let resolved = resolve_for(false, Some("auto"), Some("40199"), &mut fake).unwrap();
        assert_eq!(resolved.choice, None);
        let text = not_found("curlwright", &resolved.trail);
        assert_eq!(
            text,
            "curlwright: no CDP browser found. Tried, in order:\n  --cdp               auto\n  SLICC_CDP_URL       set, skipped by --cdp auto\n  slicc browsers      slicc not on PATH or failed\n  DevToolsActivePort  no live file\n  saved endpoint      none for this directory\n  port probe          nothing answered on 127.0.0.1:9300-9309\nPass --cdp <port|host:port|ws://...|http://...>, set SLICC_CDP_URL, or start a browser with --remote-debugging-port.\n"
        );
        let plain = resolve_for(false, None, None, &mut Fake::default()).unwrap();
        assert_eq!(plain.trail[0], Step::new(Rule::Flag, "not given"));
        assert_eq!(plain.trail[1], Step::new(Rule::Env, "not set"));
    }

    #[test]
    fn wasm_stops_at_port_9222_without_looking_further() {
        let mut fake = all_running();
        let choice = resolve_for(true, None, None, &mut fake).unwrap().choice.unwrap();
        assert_eq!(choice, Choice { start: Start::Discover("http://127.0.0.1:9222/json/version".into()), rule: Rule::Wasm });
        assert!(fake.calls.is_empty());
        assert_eq!(resolve_for(true, Some("9333"), None, &mut fake).unwrap().choice.unwrap().rule, Rule::Flag);
    }

    #[test]
    fn a_bad_flag_is_an_error_and_zero_points_at_auto() {
        let err = resolve_for(false, Some("0"), None, &mut all_running()).unwrap_err();
        assert!(err.contains("unsupported CDP URL \"0\""), "{err}");
        assert!(err.contains("--cdp auto"), "{err}");
        assert!(resolve_for(false, None, Some("ftp://x"), &mut all_running()).is_err());
    }

    #[test]
    fn describe_names_the_endpoint_and_the_rule() {
        let choice = Choice { start: Start::Discover(version(9222)), rule: Rule::SliccBrowsers };
        assert_eq!(describe(&choice, false), "* CDP http://127.0.0.1:9222/json/version (via slicc browsers)\n");
        let secret = Choice { start: Start::Direct("wss://cdp.example/devtools/browser/x?token=hunter2".into()), rule: Rule::Env };
        assert!(!describe(&secret, false).contains("hunter2"));
        assert!(describe(&secret, true).contains("hunter2"));
    }

    #[test]
    fn saved_endpoints_stay_loopback_and_probe_ones_must_answer() {
        let alive = r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:40103/devtools/browser/b"}"#;
        let mut trail = Vec::new();
        let mut live = |_: &str| Ok(alive.to_string());
        let mut dead = |_: &str| Err("refused".to_string());
        let probe_json = r#"{"endpoint":"http://127.0.0.1:40103/json/version","provenance":"probe"}"#;
        assert_eq!(saved_from_json(Some(probe_json), &mut live, &mut trail).as_deref(), Some("http://127.0.0.1:40103/json/version"));
        assert!(trail.is_empty());
        assert_eq!(saved_from_json(Some(probe_json), &mut dead, &mut trail), None);
        assert_eq!(trail.pop().unwrap().note, "http://127.0.0.1:40103/json/version no longer answers");
        let remote = r#"{"endpoint":"http://evil.example:9222/json/version","provenance":"explicit"}"#;
        assert_eq!(saved_from_json(Some(remote), &mut live, &mut trail), None);
        assert_eq!(saved_from_json(None, &mut live, &mut trail), None);
        assert_eq!(saved_from_json(Some("{}"), &mut live, &mut trail), None);
        assert_eq!(trail.len(), 3);
        let explicit = r#"{"endpoint":"ws://127.0.0.1:40103/devtools/browser/b","provenance":"explicit"}"#;
        assert!(saved_from_json(Some(explicit), &mut dead, &mut trail).is_some());
        assert!(loopback_url("http://127.0.0.1:1/") && loopback_url("ws://localhost:2/x") && loopback_url("http://[::1]:3/"));
        assert!(!loopback_url("http://127.0.0.1.evil:1/") && !loopback_url("ftp://127.0.0.1/"));
        for odd in [
            "http://127.0.0.1@evil.example:1/",
            "http://127.0.0.1:80@evil.example/",
            "http://[::1]@evil.example/",
            "ws://localhost@evil.example:9300/devtools/browser/x",
            "http://127.0.0.1.:9300/json/version",
            "http://localhost.:9300/json/version",
            "http://127.0.0.1.evil.example:9300/",
            "http://0x7f.0.0.1:9300/",
            "http://evil.example/127.0.0.1",
            "http://evil.localhost:9300/json/version",
            "ws://foo.localhost:9300/devtools/browser/x",
            "http://127.000.0.1:9300/",
        ] {
            assert!(!loopback_url(odd), "{odd}");
            let saved = format!(r#"{{"endpoint":"{odd}","provenance":"explicit"}}"#);
            assert_eq!(saved_from_json(Some(&saved), &mut live, &mut trail), None, "{odd}");
        }
        assert!(loopback_url("http://user@127.0.0.1:9300/json/version"));
        for host in ["localhost", "LOCALHOST", "127.0.0.1", "127.8.9.10", "[::1]", "0.0.0.0", "[::]", "[::ffff:127.0.0.1]", "evil.localhost", "foo.localhost", "localhost.", "127.0.0.1.", "0x7f.0.0.1", "203.0.113.9"] {
            assert_eq!(
                loopback_url(&format!("http://{host}:9300/json/version")),
                crate::ports::local_host(host),
                "{host}"
            );
        }
    }

    #[test]
    fn misspelt_options_get_a_hint() {
        let known = ["--cdp", "--tab", "--frame", "--verbose", "--data-raw", "--data-binary"];
        assert_eq!(suggest("--cpd", &known), Some("--cdp"));
        assert_eq!(suggest("--cdpp", &known), Some("--cdp"));
        assert_eq!(suggest("--cpd=0", &known), Some("--cdp"));
        assert_eq!(suggest("--verbos", &known), Some("--verbose"));
        assert_eq!(suggest("--data-bianry", &known), Some("--data-binary"));
        assert_eq!(suggest("--zzz", &known), None);
        assert_eq!(suggest("--cdp", &known), None);
    }

    #[test]
    fn the_help_block_keeps_every_word_and_the_width() {
        let block = help_block("  ", 80);
        assert!(block.lines().all(|line| line.len() <= 80 && line.starts_with("  ") || line.is_empty()));
        let words = |text: &str| text.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        let joined = [CONNECT_ORDER, CDP_FORMS, CDP_REPORT].join(" ");
        assert_eq!(words(&block), words(&joined));
    }

    #[test]
    fn port_ranges_print_compactly() {
        assert_eq!(port_range(&[9222, 9223, 9229]), "9222-9229");
        assert_eq!(port_range(&[9300]), "9300");
        assert_eq!(port_range(&[]), "(no ports)");
    }
}
