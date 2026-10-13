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
}
