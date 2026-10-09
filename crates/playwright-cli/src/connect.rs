#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    Direct(String),
    Discover(String),
}

pub fn choose_start(cdp: Option<&str>, env: Option<&str>) -> Result<Start, String> {
    if let Some(url) = nonempty(cdp) {
        return classify(url);
    }
    if let Some(url) = nonempty(env) {
        return classify(url);
    }
    Ok(Start::Discover(
        "http://127.0.0.1:9222/json/version".to_string(),
    ))
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn classify(url: &str) -> Result<Start, String> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("ws://") || lower.starts_with("wss://") {
        Ok(Start::Direct(url.to_string()))
    } else if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(Start::Discover(discovery_url(url)))
    } else {
        Err(format!(
            "unsupported CDP URL \"{url}\" (expected ws://, wss://, http://, or https://)\n"
        ))
    }
}

pub fn discovery_url(http_url: &str) -> String {
    if http_url.contains("/json/version") {
        return http_url.to_string();
    }
    let trimmed = http_url.trim_end_matches('/');
    format!("{trimmed}/json/version")
}

pub fn append_runtime(url: &str, runtime: Option<&str>) -> String {
    let Some(runtime) = nonempty(runtime) else {
        return url.to_string();
    };
    let enc = percent_encode(runtime);
    if url.contains('?') {
        format!("{url}&runtime={enc}")
    } else {
        format!("{url}?runtime={enc}")
    }
}

pub fn format_http_error(status: u16, body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.trim_end_matches('\0');
    if text.is_empty() {
        format!("HTTP {status}\n")
    } else if text.ends_with('\n') {
        format!("HTTP {status}\n{text}")
    } else {
        format!("HTTP {status}\n{text}\n")
    }
}

pub fn websocket_url_from_version(body: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|err| format!("json/version: {err}\n"))?;
    match value.get("webSocketDebuggerUrl").and_then(|v| v.as_str()) {
        Some(url) if !url.is_empty() => Ok(url.to_string()),
        _ => Err("json/version returned no webSocketDebuggerUrl\n".to_string()),
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_wins_over_env_and_discovery() {
        let flag = "ws://127.0.0.1:9333/devtools/browser/from-flag";
        let env = "ws://127.0.0.1:9222/devtools/browser/from-env";
        assert_eq!(
            choose_start(Some(flag), Some(env)).unwrap(),
            Start::Direct(flag.to_string())
        );
    }

    #[test]
    fn env_url_is_opaque() {
        let env = "ws://127.0.0.1:9222/devtools/browser/6f1c0a2e-1b4d-4e2a-9c77-0a1b2c3d4e5f";
        assert_eq!(
            choose_start(None, Some(env)).unwrap(),
            Start::Direct(env.to_string())
        );
    }

    #[test]
    fn unset_uses_json_version_discovery() {
        assert_eq!(
            choose_start(None, None).unwrap(),
            Start::Discover("http://127.0.0.1:9222/json/version".to_string())
        );
    }

    #[test]
    fn http_flag_is_discovery() {
        assert_eq!(
            choose_start(Some("http://127.0.0.1:9444"), None).unwrap(),
            Start::Discover("http://127.0.0.1:9444/json/version".to_string())
        );
    }

    #[test]
    fn does_not_invent_a_debugger_path() {
        let start = choose_start(None, None).unwrap();
        let text = format!("{start:?}");
        assert!(!text.contains("cdp.slicc.internal"));
        assert!(!text.contains("cdp.kernel.localhost"));
        assert!(!text.contains("/devtools/browser"));
    }

    #[test]
    fn runtime_query_appends() {
        assert_eq!(
            append_runtime("ws://127.0.0.1:9222/devtools/browser/abc", Some("follower")),
            "ws://127.0.0.1:9222/devtools/browser/abc?runtime=follower"
        );
        assert_eq!(
            append_runtime("ws://127.0.0.1:9222/devtools/browser/abc?x=1", Some("a b")),
            "ws://127.0.0.1:9222/devtools/browser/abc?x=1&runtime=a%20b"
        );
        assert_eq!(
            append_runtime("ws://127.0.0.1:9222/p", None),
            "ws://127.0.0.1:9222/p"
        );
    }

    #[test]
    fn http_error_prints_status_and_body() {
        assert_eq!(format_http_error(503, b"no host"), "HTTP 503\nno host\n");
        assert_eq!(format_http_error(404, b""), "HTTP 404\n");
        assert_eq!(format_http_error(404, b"missing\n"), "HTTP 404\nmissing\n");
    }

    #[test]
    fn version_body_uses_returned_socket_only() {
        let body = r#"{"webSocketDebuggerUrl":"ws://127.0.0.1:9222/devtools/browser/uuid"}"#;
        assert_eq!(
            websocket_url_from_version(body).unwrap(),
            "ws://127.0.0.1:9222/devtools/browser/uuid"
        );
        assert!(websocket_url_from_version("{}").is_err());
    }
}
