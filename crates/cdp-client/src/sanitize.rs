pub fn redact_urls_in_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, url, after)) = find_next_url(rest) {
        result.push_str(before);
        result.push_str(&redact_url(url));
        rest = after;
    }
    result.push_str(rest);
    result
}

const URL_SCHEMES: &[&str] = &["https://", "http://", "wss://", "ws://"];

fn find_next_url(text: &str) -> Option<(&str, &str, &str)> {
    let mut best: Option<(usize, usize)> = None;
    for &scheme in URL_SCHEMES {
        for (_blob, start) in scheme_positions(text, scheme) {
            if best.is_none() || start < best.unwrap().0 {
                let authority_start = start + if text[start..].starts_with("blob:") {
                    5 + scheme.len()
                } else {
                    scheme.len()
                };
                best = Some((start, authority_start));
            }
        }
    }
    let (start, authority_start) = best?;
    if authority_start >= text.len() {
        return None;
    }
    let url_end = text[authority_start..]
        .find(|c: char| is_url_delimiter(c))
        .map(|i| authority_start + i)
        .unwrap_or(text.len());
    if url_end <= authority_start {
        return None;
    }
    let url = &text[start..url_end];
    Some((&text[..start], url, &text[url_end..]))
}

fn scheme_positions(text: &str, scheme: &str) -> Vec<(bool, usize)> {
    let mut hits = Vec::new();
    let mut search_from = 0;
    while let Some(pos) = text[search_from..].find(scheme) {
        let abs = search_from + pos;
        let blob = abs >= 5 && &text[abs - 5..abs] == "blob:";
        let start = if blob { abs - 5 } else { abs };
        hits.push((blob, start));
        search_from = abs + scheme.len();
    }
    hits
}

fn is_url_delimiter(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '"' | '\'' | '<' | '>' | '`' | '{' | '}' | '|' | '\\' | '^')
        || c.is_control()
}

pub fn redact_url(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    if raw.starts_with("data:") {
        return raw.to_string();
    }
    if raw.starts_with("blob:") {
        if let Some(inner) = raw.strip_prefix("blob:") {
            let redacted_inner = redact_url(inner);
            return format!("blob:{redacted_inner}");
        }
        return raw.to_string();
    }
    let Ok(mut parsed) = url::Url::parse(raw) else {
        return raw.to_string();
    };
    let has_userinfo = parsed.username() != "" || parsed.password().is_some();
    if has_userinfo {
        let _ = parsed.set_username("\u{2026}");
        let _ = parsed.set_password(Some("\u{2026}"));
    }
    let has_query = parsed.query().is_some_and(|q| !q.is_empty());
    let has_fragment = parsed.fragment().is_some_and(|f| !f.is_empty());
    if !has_query && !has_fragment && !has_userinfo {
        return raw.to_string();
    }
    if has_query {
        let raw_query = parsed.query().unwrap_or("");
        let redacted_query = redact_query_string(raw_query);
        parsed.set_query(Some(&redacted_query));
    }
    if has_fragment {
        parsed.set_fragment(Some("\u{2026}"));
    }
    parsed.to_string().replace("%E2%80%A6", "\u{2026}")
}

pub fn redact_query_string(query: &str) -> String {
    query
        .split('&')
        .map(|component| {
            if let Some((key, _)) = component.split_once('=') {
                format!("{key}=\u{2026}")
            } else {
                "\u{2026}".to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}

pub fn join_url(base: &str, relative: &str) -> Option<String> {
    let parsed = url::Url::parse(base).ok()?;
    parsed.join(relative).ok().map(|u| u.to_string())
}

pub fn is_absolute_url(raw: &str) -> bool {
    url::Url::parse(raw).is_ok()
}

pub fn redact_json_urls(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if url::Url::parse(s).is_ok() && !s.starts_with("data:") {
                serde_json::Value::String(redact_url(s))
            } else {
                value.clone()
            }
        }
        serde_json::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(redact_json_urls).collect())
        }
        serde_json::Value::Object(map) => {
            serde_json::Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), redact_json_urls(v)))
                    .collect(),
            )
        }
        _ => value.clone(),
    }
}

pub fn redact_header_url(value: &str, base: Option<&str>) -> String {
    if value.is_empty() {
        return value.to_string();
    }
    let trimmed = value.trim();
    if let Some((url_part, quoted)) = extract_refresh_url(trimmed) {
        let semi = trimmed.find(';').unwrap();
        let after_semi = &trimmed[semi + 1..];
        let after_trimmed = after_semi.trim_start();
        let prefix = &trimmed[..semi + 1 + (after_semi.len() - after_trimmed.len()) + 4];
        let redacted = redact_single_header_url(url_part, base);
        if quoted {
            return format!("{prefix}\"{redacted}\"");
        }
        return format!("{prefix}{redacted}");
    }
    if trimmed.starts_with('<') {
        return redact_link_header(trimmed, base);
    }
    redact_single_header_url(trimmed, base)
}

fn extract_refresh_url(value: &str) -> Option<(&str, bool)> {
    let semi = value.find(';')?;
    let after = value[semi + 1..].trim_start();
    let lower = after.to_ascii_lowercase();
    if lower.starts_with("url=") {
        let url_part = &after[4..];
        let trimmed = url_part.trim();
        if (trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
        {
            Some((&trimmed[1..trimmed.len() - 1], true))
        } else {
            Some((url_part, false))
        }
    } else {
        None
    }
}

fn redact_link_header(value: &str, base: Option<&str>) -> String {
    let mut result = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        result.push_str(&rest[..open + 1]);
        rest = &rest[open + 1..];
        if let Some(close) = rest.find('>') {
            let url_part = &rest[..close];
            result.push_str(&redact_single_header_url(url_part, base));
            result.push('>');
            rest = &rest[close + 1..];
        } else {
            result.push_str(&redact_single_header_url(rest, base));
            return result;
        }
    }
    result.push_str(rest);
    result
}

fn redact_single_header_url(raw: &str, base: Option<&str>) -> String {
    if raw.is_empty() {
        return raw.to_string();
    }
    if let Ok(parsed) = url::Url::parse(raw) {
        return redact_url(&parsed.to_string());
    }
    let Some(base_url) = base.and_then(|b| url::Url::parse(b).ok()) else {
        return raw.to_string();
    };
    let Ok(resolved) = base_url.join(raw) else {
        return raw.to_string();
    };
    let redacted = redact_url(&resolved.to_string());
    let Ok(redacted_parsed) = url::Url::parse(&redacted) else {
        return raw.to_string();
    };
    let Ok(base_origin) = url::Url::parse(&base_url.origin().ascii_serialization()) else {
        return raw.to_string();
    };
    if redacted_parsed.origin() == base_origin.origin() {
        let path = redacted_parsed.path();
        let query = redacted_parsed.query().map(|q| format!("?{q}")).unwrap_or_default();
        let fragment = redacted_parsed.fragment().map(|f| format!("#{f}")).unwrap_or_default();
        format!("{path}{query}{fragment}")
    } else {
        redacted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_query_unchanged() {
        assert_eq!(
            redact_url("https://example.com/path"),
            "https://example.com/path"
        );
    }

    #[test]
    fn query_values_redacted() {
        assert_eq!(
            redact_url("http://127.0.0.1:1455/success?id_token=eyJ...&state=x"),
            "http://127.0.0.1:1455/success?id_token=…&state=…"
        );
    }

    #[test]
    fn fragment_redacted() {
        assert_eq!(
            redact_url("http://example.com/page#access_token=SECRET"),
            "http://example.com/page#…"
        );
    }

    #[test]
    fn query_and_fragment() {
        assert_eq!(
            redact_url("http://127.0.0.1/success?id_token=SECRET#access_token=SECRET2"),
            "http://127.0.0.1/success?id_token=…#…"
        );
    }

    #[test]
    fn userinfo_redacted() {
        assert_eq!(
            redact_url("http://user:pass@example.com/path"),
            "http://…:…@example.com/path"
        );
    }

    #[test]
    fn empty_string() {
        assert_eq!(redact_url(""), "");
    }

    #[test]
    fn data_url_unchanged() {
        assert_eq!(
            redact_url("data:text/html,<h1>hi</h1>"),
            "data:text/html,<h1>hi</h1>"
        );
    }

    #[test]
    fn blob_url_without_query_unchanged() {
        assert_eq!(
            redact_url("blob:https://example.com/abc-123"),
            "blob:https://example.com/abc-123"
        );
    }

    #[test]
    fn blob_url_with_fragment_redacted() {
        let result = redact_url("blob:https://example.com/id#access_token=SECRET");
        assert!(!result.contains("SECRET"), "{result}");
        assert!(result.starts_with("blob:"), "{result}");
    }

    #[test]
    fn blob_url_userinfo_redacted() {
        let result = redact_url("blob:http://user:pass@example.com/path");
        assert!(!result.contains("user"), "{result}");
        assert!(!result.contains("pass"), "{result}");
        assert!(result.starts_with("blob:"), "{result}");
    }

    #[test]
    fn blob_opaque_origin_unchanged() {
        assert_eq!(
            redact_url("blob:null/abc-123"),
            "blob:null/abc-123"
        );
    }

    #[test]
    fn invalid_url_returned_as_is() {
        assert_eq!(redact_url("not a url"), "not a url");
    }

    #[test]
    fn empty_query_not_touched() {
        assert_eq!(
            redact_url("http://example.com/path?"),
            "http://example.com/path?"
        );
    }

    #[test]
    fn empty_fragment_not_touched() {
        assert_eq!(
            redact_url("http://example.com/path#"),
            "http://example.com/path#"
        );
    }

    #[test]
    fn preserves_scheme_host_port_path() {
        let redacted = redact_url("https://127.0.0.1:9333/callback?code=abc&state=xyz#tok=1");
        assert!(redacted.starts_with("https://127.0.0.1:9333/callback?"));
        assert!(redacted.contains("code="));
        assert!(redacted.contains("state="));
        assert!(!redacted.contains("abc"));
        assert!(!redacted.contains("xyz"));
        assert!(!redacted.contains("tok=1"));
    }

    #[test]
    fn bridge_token_redacted() {
        let redacted = redact_url("http://127.0.0.1:9222/?bridgeToken=secret123");
        assert!(!redacted.contains("secret123"));
        assert!(redacted.contains("bridgeToken="));
    }

    #[test]
    fn username_only_redacted() {
        let redacted = redact_url("http://admin@example.com/path");
        assert!(!redacted.contains("admin"));
        assert!(redacted.contains("…"));
    }

    #[test]
    fn about_blank_with_query_redacted() {
        let result = redact_url("ABOUT:blank?q=SECRET");
        assert!(!result.contains("SECRET"), "{result}");
    }

    #[test]
    fn non_url_paths_returned_as_is() {
        assert_eq!(redact_url("example.com/path"), "example.com/path");
    }

    #[test]
    fn javascript_url_no_query() {
        let result = redact_url("javascript:void('x')");
        assert_eq!(result, "javascript:void('x')");
    }

    #[test]
    fn about_srcdoc_no_query() {
        let result = redact_url("about:srcdoc,content");
        assert!(result.contains("srcdoc"), "{result}");
    }

    #[test]
    fn redact_json_urls_walks_objects_and_arrays() {
        let input = serde_json::json!({
            "href": "https://example.com/api?token=SECRET",
            "name": "test",
            "nested": { "url": "https://example.com/x?key=val" },
            "list": ["https://example.com/?a=1", "plain"],
            "count": 42
        });
        let redacted = redact_json_urls(&input);
        let href = redacted["href"].as_str().unwrap();
        assert!(!href.contains("SECRET"), "{href}");
        assert!(href.contains("token="), "{href}");
        let nested_url = redacted["nested"]["url"].as_str().unwrap();
        assert!(!nested_url.contains("val"), "{nested_url}");
        let list_first = redacted["list"][0].as_str().unwrap();
        assert!(!list_first.contains("=1") || list_first.contains("…"), "{list_first}");
        assert_eq!(redacted["list"][1], "plain");
        assert_eq!(redacted["name"], "test");
        assert_eq!(redacted["count"], 42);
    }

    #[test]
    fn redact_json_urls_ignores_non_http() {
        let input = serde_json::json!({ "path": "/relative?code=SECRET" });
        let redacted = redact_json_urls(&input);
        assert_eq!(redacted["path"], "/relative?code=SECRET");
    }

    #[test]
    fn redact_header_url_absolute() {
        let result = redact_header_url("https://example.com/cb?code=SECRET", None);
        assert!(!result.contains("SECRET"), "{result}");
        assert!(result.contains("code="), "{result}");
    }

    #[test]
    fn redact_header_url_relative_location() {
        let result = redact_header_url(
            "/callback?code=SECRET",
            Some("https://example.com/login"),
        );
        assert!(!result.contains("SECRET"), "{result}");
        assert!(result.starts_with("/callback?"), "{result}");
        assert!(result.contains("code="), "{result}");
    }

    #[test]
    fn redact_header_url_refresh() {
        let result = redact_header_url(
            "0; url=/callback?code=SECRET",
            Some("https://example.com/login"),
        );
        assert!(!result.contains("SECRET"), "{result}");
        assert!(result.starts_with("0; url="), "{result}");
    }

    #[test]
    fn redact_header_url_link() {
        let result = redact_header_url(
            "<https://example.com/next?token=SECRET>; rel=next",
            None,
        );
        assert!(!result.contains("SECRET"), "{result}");
        assert!(result.contains("; rel=next"), "{result}");
        assert!(result.contains("token="), "{result}");
    }

    #[test]
    fn redact_header_url_no_query_unchanged() {
        assert_eq!(
            redact_header_url("/plain", Some("https://example.com/")),
            "/plain"
        );
    }

    #[test]
    fn redact_header_url_empty() {
        assert_eq!(redact_header_url("", None), "");
    }

    #[test]
    fn bare_query_key_redacted() {
        let result = redact_url("https://example.com/continue?eyJhbGciOi");
        assert!(!result.contains("eyJhbGciOi"), "{result}");
        assert!(result.contains("…"), "{result}");
        assert!(!result.contains('='), "{result}");
    }

    #[test]
    fn bare_key_mixed_with_pairs() {
        let result = redact_url("https://example.com/?SECRET_TOKEN&state=x");
        assert!(!result.contains("SECRET_TOKEN"), "{result}");
        assert!(result.contains("state="), "{result}");
    }

    #[test]
    fn redact_urls_in_text_no_urls() {
        assert_eq!(redact_urls_in_text("no urls here"), "no urls here");
    }

    #[test]
    fn redact_urls_in_text_single_url() {
        let input = "TypeError: Failed to fetch https://u:PW@host.example.com/path";
        let result = redact_urls_in_text(input);
        assert!(!result.contains("PW"), "{result}");
        assert!(result.contains("host.example.com/path"), "{result}");
    }

    #[test]
    fn redact_urls_in_text_multiple_urls() {
        let input = "Redirect from https://a:b@one.com/x to https://c:d@two.com/y failed";
        let result = redact_urls_in_text(input);
        assert!(!result.contains("a:b"), "{result}");
        assert!(!result.contains("c:d"), "{result}");
        assert!(result.contains("one.com"), "{result}");
        assert!(result.contains("two.com"), "{result}");
    }

    #[test]
    fn redact_urls_in_text_query_redacted() {
        let input = "Failed: https://host.com/cb?token=SECRET end";
        let result = redact_urls_in_text(input);
        assert!(!result.contains("SECRET"), "{result}");
        assert!(result.contains("token="), "{result}");
    }

    #[test]
    fn redact_urls_in_text_blob() {
        let input = "Error at blob:https://u:p@host.com/abc end";
        let result = redact_urls_in_text(input);
        assert!(!result.contains("u:p"), "{result}");
        assert!(result.starts_with("Error at blob:"), "{result}");
    }

    #[test]
    fn redact_urls_in_text_ws_scheme() {
        let input = "Connection to ws://admin:secret@host:8080/ws failed";
        let result = redact_urls_in_text(input);
        assert!(!result.contains("secret"), "{result}");
        assert!(result.contains("host:8080/ws"), "{result}");
    }

    #[test]
    fn redact_urls_in_text_wss_scheme() {
        let input = "Connecting wss://user:pw@h.io/s";
        let result = redact_urls_in_text(input);
        assert!(!result.contains("pw"), "{result}");
    }

    #[test]
    fn redact_urls_in_text_preserves_surrounding() {
        let input = "[error] https://u:PW@h.com/x (details)";
        let result = redact_urls_in_text(input);
        assert!(result.starts_with("[error] "), "{result}");
        assert!(result.ends_with(" (details)"), "{result}");
        assert!(!result.contains("PW"), "{result}");
    }

    #[test]
    fn redact_urls_in_text_no_scheme_untouched() {
        let input = "user:pass@host.com/path";
        assert_eq!(redact_urls_in_text(input), input);
    }

    #[test]
    fn redact_urls_in_text_empty() {
        assert_eq!(redact_urls_in_text(""), "");
    }
}
