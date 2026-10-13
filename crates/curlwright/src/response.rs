pub fn status_line(status: u16, status_text: &str) -> String {
    if status_text.is_empty() {
        format!("HTTP/1.1 {status}")
    } else {
        format!("HTTP/1.1 {status} {status_text}")
    }
}

pub fn format_header_block(status: u16, status_text: &str, headers: &[(String, String)], full_urls: bool, base_url: Option<&str>) -> String {
    let mut lines = vec![status_line(status, status_text)];
    for (name, value) in headers {
        let display = if !full_urls && is_url_header(name) {
            cdp_client::sanitize::redact_header_url(value, base_url)
        } else {
            value.clone()
        };
        lines.push(format!("{name}: {display}"));
    }
    format!("{}\r\n\r\n", lines.join("\r\n"))
}

fn is_url_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "location" | "referer" | "content-location" | "refresh" | "link"
    )
}

pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.contains(&0) || std::str::from_utf8(bytes).is_err()
}

pub const BINARY_OUTPUT_WARNING: &str = "\
Warning: Binary output can mess up your terminal. Use \"--output -\" to tell\n\
Warning: curlwright to output it to your terminal anyway, or consider\n\
Warning: \"--output <FILE>\" to save to a file.\n";

pub fn remote_name(url: &str) -> Option<String> {
    let pathname = url_pathname(url).unwrap_or_else(|| url.split('?').next().unwrap_or(url).to_string());
    let trimmed = pathname.trim_end_matches('/');
    let segment = trimmed.rsplit('/').next().unwrap_or("");
    if segment.is_empty() { None } else { Some(segment.to_string()) }
}

fn url_pathname(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() || !scheme.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '+' | '-' | '.')) {
        return None;
    }
    let path = match rest.find('/') {
        Some(index) => &rest[index..],
        None => "/",
    };
    let path = path.split(['?', '#']).next().unwrap_or("/");
    Some(if path.is_empty() { "/".to_string() } else { path.to_string() })
}

pub fn header_block_size(status: u16, status_text: &str, headers: &[(String, String)], full_urls: bool, base_url: Option<&str>) -> usize {
    format_header_block(status, status_text, headers, full_urls, base_url).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_block_and_remote_name() {
        assert_eq!(status_line(200, "OK"), "HTTP/1.1 200 OK");
        assert_eq!(status_line(204, ""), "HTTP/1.1 204");
        let block = format_header_block(200, "OK", &[("content-type".into(), "text/plain".into())], true, None);
        assert_eq!(block, "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\n\r\n");
        assert_eq!(header_block_size(200, "OK", &[("content-type".into(), "text/plain".into())], true, None), block.len());
        assert_eq!(remote_name("https://a.example/files/report.csv?v=1").as_deref(), Some("report.csv"));
        assert_eq!(remote_name("https://a.example/files/").as_deref(), Some("files"));
        assert_eq!(remote_name("https://a.example/"), None);
        assert!(!looks_binary("héllo".as_bytes()));
        assert!(looks_binary(&[0x61, 0x00, 0x62]));
        assert!(BINARY_OUTPUT_WARNING.contains("Binary output can mess up your terminal"));
    }

    #[test]
    fn header_block_redacts_location_by_default() {
        let headers = vec![
            ("location".into(), "https://example.com/cb?code=SECRET".into()),
            ("content-type".into(), "text/html".into()),
        ];
        let block = format_header_block(302, "Found", &headers, false, Some("https://example.com/login"));
        assert!(!block.contains("SECRET"), "{block}");
        assert!(block.contains("code="), "{block}");
        assert!(block.contains("content-type: text/html"), "{block}");
    }

    #[test]
    fn header_block_shows_full_with_flag() {
        let headers = vec![
            ("location".into(), "https://example.com/cb?code=SECRET".into()),
        ];
        let block = format_header_block(302, "Found", &headers, true, None);
        assert!(block.contains("SECRET"), "{block}");
    }

    #[test]
    fn header_block_redacts_relative_location() {
        let headers = vec![
            ("location".into(), "/callback?code=SECRET".into()),
        ];
        let block = format_header_block(302, "Found", &headers, false, Some("https://example.com/login"));
        assert!(!block.contains("SECRET"), "{block}");
        assert!(block.contains("/callback?"), "{block}");
    }
}
