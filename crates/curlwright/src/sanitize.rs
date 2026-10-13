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
}
