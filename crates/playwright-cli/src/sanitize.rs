use std::cell::Cell;

thread_local! {
    static FULL_URLS: Cell<bool> = const { Cell::new(false) };
}

pub fn set_full_urls(enabled: bool) {
    FULL_URLS.with(|cell| cell.set(enabled));
}

pub fn full_urls_enabled() -> bool {
    FULL_URLS.with(|cell| cell.get())
}

pub fn display_url(raw: &str) -> String {
    if full_urls_enabled() {
        terminal(raw)
    } else {
        terminal(&url(raw))
    }
}

pub fn browser_message(s: &str) -> String {
    if full_urls_enabled() {
        terminal(s)
    } else {
        terminal(&cdp_client::sanitize::redact_urls_in_text(s))
    }
}

pub fn url(raw: &str) -> String {
    cdp_client::sanitize::redact_url(raw)
}

pub fn terminal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\n' | '\t' => out.push(ch),
            '\x07' => out.push_str("\\x07"),
            '\r' => out.push_str("\\r"),
            '\x1b' => out.push_str("\\x1b"),
            '\x7f' => out.push_str("\\x7f"),
            c if c.is_control() && (c as u32) < 0x20 => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if (0x80..=0x9f).contains(&(c as u32)) => {
                out.push_str(&format!("\\u{{{:x}}}", c as u32));
            }
            _ => out.push(ch),
        }
    }
    out
}

pub fn json_c1(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp == 0x7f || (0x80..=0x9f).contains(&cp) {
            out.push_str(&format!("\\u{:04x}", cp));
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_plain_text() {
        assert_eq!(terminal("hello world"), "hello world");
    }

    #[test]
    fn preserves_newline_and_tab() {
        assert_eq!(terminal("a\tb\nc"), "a\tb\nc");
    }

    #[test]
    fn escapes_esc() {
        assert_eq!(terminal("\x1b[2J"), "\\x1b[2J");
    }

    #[test]
    fn escapes_bel() {
        assert_eq!(terminal("\x07"), "\\x07");
    }

    #[test]
    fn escapes_cr() {
        assert_eq!(terminal("\r"), "\\r");
    }

    #[test]
    fn escapes_del() {
        assert_eq!(terminal("\x7f"), "\\x7f");
    }

    #[test]
    fn escapes_null() {
        assert_eq!(terminal("\x00"), "\\x00");
    }

    #[test]
    fn escapes_c1_range() {
        assert_eq!(terminal("\u{0080}"), "\\u{80}");
        assert_eq!(terminal("\u{009f}"), "\\u{9f}");
    }

    #[test]
    fn does_not_escape_u00a0() {
        assert_eq!(terminal("\u{00a0}"), "\u{00a0}");
    }

    #[test]
    fn mixed_terminal_injection() {
        let input = "\x1b]0;pwned\x07\x1b[2J\x1b[Hfake prompt";
        let output = terminal(input);
        assert!(!output.contains('\x1b'));
        assert!(!output.contains('\x07'));
        assert!(output.contains("\\x1b"));
        assert!(output.contains("\\x07"));
    }

    #[test]
    fn escapes_set_title_sequence() {
        let input = "\x1b]2;evil title\x07";
        let output = terminal(input);
        assert_eq!(output, "\\x1b]2;evil title\\x07");
    }

    #[test]
    fn escapes_cursor_movement() {
        let input = "\x1b[A\x1b[K";
        let output = terminal(input);
        assert_eq!(output, "\\x1b[A\\x1b[K");
    }

    #[test]
    fn url_redacts_query_values() {
        set_full_urls(false);
        let redacted = url("http://127.0.0.1/success?id_token=SECRET&state=x");
        assert!(!redacted.contains("SECRET"));
        assert!(redacted.contains("id_token="));
        assert!(redacted.contains("state="));
    }

    #[test]
    fn url_redacts_fragment() {
        let redacted = url("http://example.com/page#access_token=SECRET");
        assert!(!redacted.contains("SECRET"));
    }

    #[test]
    fn url_redacts_userinfo() {
        let redacted = url("http://user:pass@example.com/");
        assert!(!redacted.contains("user"));
        assert!(!redacted.contains("pass"));
    }

    #[test]
    fn url_preserves_no_query() {
        assert_eq!(
            url("https://example.com/path"),
            "https://example.com/path"
        );
    }

    #[test]
    fn url_preserves_data_urls() {
        assert_eq!(
            url("data:text/html,<h1>hi</h1>"),
            "data:text/html,<h1>hi</h1>"
        );
    }

    #[test]
    fn url_preserves_blob_urls_without_query() {
        assert_eq!(
            url("blob:https://example.com/abc"),
            "blob:https://example.com/abc"
        );
    }

    #[test]
    fn url_returns_invalid_as_is() {
        assert_eq!(url("not a url"), "not a url");
    }

    #[test]
    fn display_url_redacts_by_default() {
        set_full_urls(false);
        let output = display_url("http://127.0.0.1/x?token=SECRET");
        assert!(!output.contains("SECRET"));
        assert!(output.contains("token="));
    }

    #[test]
    fn display_url_shows_full_when_enabled() {
        set_full_urls(true);
        let output = display_url("http://127.0.0.1/x?token=SECRET");
        assert!(output.contains("SECRET"));
        set_full_urls(false);
    }

    #[test]
    fn display_url_also_escapes_terminal() {
        set_full_urls(true);
        let output = display_url("http://example.com/\x1b[2J?x=1");
        assert!(!output.contains('\x1b'));
        assert!(output.contains("\\x1b"));
        set_full_urls(false);
    }

    #[test]
    fn json_c1_escapes_del() {
        assert_eq!(json_c1("a\x7fb"), "a\\u007fb");
    }

    #[test]
    fn json_c1_escapes_csi() {
        assert_eq!(json_c1("a\u{009b}b"), "a\\u009bb");
    }

    #[test]
    fn json_c1_preserves_normal_text() {
        assert_eq!(json_c1("hello world"), "hello world");
    }

    #[test]
    fn json_c1_roundtrips_through_serde() {
        let input = "hello\x7fworld\u{0080}end";
        let serialized = serde_json::to_string(input).unwrap();
        let safe = json_c1(&serialized);
        let parsed: String = serde_json::from_str(&safe).unwrap();
        assert_eq!(parsed, input);
    }

    #[test]
    fn browser_message_redacts_userinfo() {
        set_full_urls(false);
        let input = "TypeError: Failed to fetch https://u:PW@host.com/x";
        let result = browser_message(input);
        assert!(!result.contains("PW"), "{result}");
        assert!(result.contains("host.com/x"), "{result}");
    }

    #[test]
    fn browser_message_full_urls_keeps_userinfo() {
        set_full_urls(true);
        let result = browser_message("TypeError: Failed to fetch https://u:PW@host.com/x");
        assert!(result.contains("PW"), "{result}");
        set_full_urls(false);
    }

    #[test]
    fn browser_message_escapes_terminal() {
        set_full_urls(false);
        let input = "Error: \x1b[2J https://u:PW@h.com/x";
        let result = browser_message(input);
        assert!(!result.contains('\x1b'), "{result}");
        assert!(!result.contains("PW"), "{result}");
    }

    #[test]
    fn browser_message_no_urls() {
        set_full_urls(false);
        assert_eq!(browser_message("plain text"), "plain text");
    }
}
