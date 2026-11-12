pub const MAX_CHARS: usize = 120;

pub fn choose(flag: Option<&str>, env: Option<&str>) -> Option<String> {
    match flag {
        Some(flag) => clean(flag),
        None => env.and_then(clean),
    }
}

pub fn clean(raw: &str) -> Option<String> {
    let kept: String = raw
        .chars()
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .filter(|ch| !ch.is_control() && !hidden(*ch))
        .collect();
    let joined = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = joined.chars().take(MAX_CHARS).collect();
    let cut = cut.trim_end();
    (!cut.is_empty()).then(|| cut.to_string())
}

fn hidden(ch: char) -> bool {
    matches!(
        ch as u32,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x115F..=0x1160
            | 0x180B..=0x180F
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFA0
            | 0xFFF9..=0xFFFB
            | 0xFFFE..=0xFFFF
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE0FFF
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_flag_wins_over_the_environment() {
        assert_eq!(choose(Some("Filling the order form"), Some("Reading mail")).as_deref(), Some("Filling the order form"));
        assert_eq!(choose(None, Some("Reading mail")).as_deref(), Some("Reading mail"));
        assert_eq!(choose(Some(""), Some("Reading mail")), None);
        assert_eq!(choose(Some("\u{200b}"), Some("Reading mail")), None);
        assert_eq!(choose(None, None), None);
        assert_eq!(choose(None, Some("  ")), None);
    }

    #[test]
    fn cleaning_drops_controls_and_format_characters_and_collapses_whitespace() {
        assert_eq!(clean("  Filling\tthe\n\norder\r\nform  ").as_deref(), Some("Filling the order form"));
        assert_eq!(clean("Filling\u{202e}mrof\u{202c} the form").as_deref(), Some("Fillingmrof the form"));
        assert_eq!(clean("Fil\u{200b}ling\u{200d} the\u{feff} \u{2066}form\u{2069}").as_deref(), Some("Filling the form"));
        assert_eq!(clean("Reading\u{2028}// injected\u{2029}code").as_deref(), Some("Reading // injected code"));
        assert_eq!(clean("Reading\u{0085}\u{0007}\u{001b}[31mred").as_deref(), Some("Reading [31mred"));
        assert_eq!(clean("Tag\u{e0041}\u{e0042}ged").as_deref(), Some("Tagged"));
        assert_eq!(clean("Écrire le café ✓").as_deref(), Some("Écrire le café ✓"));
        assert_eq!(clean("\u{200b}\u{200e}\n"), None);
    }

    #[test]
    fn cleaning_cuts_long_intents_on_character_boundaries() {
        let long = "é".repeat(500);
        let cut = clean(&long).unwrap();
        assert_eq!(cut.chars().count(), MAX_CHARS);
        let spaced = format!("{} tail", "a".repeat(MAX_CHARS - 1));
        assert_eq!(clean(&spaced).unwrap(), "a".repeat(MAX_CHARS - 1));
    }
}
