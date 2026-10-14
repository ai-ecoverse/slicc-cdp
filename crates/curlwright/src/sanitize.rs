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

pub fn browser_message(s: &str, full_urls: bool) -> String {
    if full_urls {
        terminal(s)
    } else {
        terminal(&cdp_client::sanitize::redact_urls_in_text(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_esc_and_del() {
        assert_eq!(terminal("\x1b[2J\x7f"), "\\x1b[2J\\x7f");
    }

    #[test]
    fn escapes_c1() {
        assert_eq!(terminal("\u{009b}"), "\\u{9b}");
    }

    #[test]
    fn preserves_normal_text() {
        assert_eq!(terminal("hello"), "hello");
    }

    #[test]
    fn browser_message_redacts_userinfo() {
        let input = "TypeError: Failed to fetch https://u:PW@host.com/x";
        let result = browser_message(input, false);
        assert!(!result.contains("PW"), "{result}");
        assert!(result.contains("host.com/x"), "{result}");
    }

    #[test]
    fn browser_message_full_urls_keeps_userinfo() {
        let input = "TypeError: Failed to fetch https://u:PW@host.com/x";
        let result = browser_message(input, true);
        assert!(result.contains("PW"), "{result}");
    }

    #[test]
    fn browser_message_also_escapes_terminal() {
        let input = "Error: \x1b[2J https://u:PW@host.com/x";
        let result = browser_message(input, false);
        assert!(!result.contains('\x1b'), "{result}");
        assert!(!result.contains("PW"), "{result}");
    }

    #[test]
    fn browser_message_no_urls_unchanged() {
        assert_eq!(browser_message("plain text", false), "plain text");
    }

    #[test]
    fn browser_message_redacts_query() {
        let input = "net::ERR_FAILED https://host.com/cb?token=SECRET";
        let result = browser_message(input, false);
        assert!(!result.contains("SECRET"), "{result}");
        assert!(result.contains("token="), "{result}");
    }
}
