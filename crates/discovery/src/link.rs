use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub href: String,
    pub rel: String,
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub params: std::collections::BTreeMap<String, String>,
}

#[must_use]
pub fn parse_link_header(raw: &str, base: &Url) -> Vec<Link> {
    let mut links = Vec::new();
    for line in raw.split('\n') {
        parse_header_value(line.trim(), base, &mut links);
    }
    links
}

fn parse_header_value(input: &str, base: &Url, out: &mut Vec<Link>) {
    let mut remaining = input;
    while !remaining.is_empty() {
        remaining = remaining.trim_start();
        if remaining.is_empty() {
            break;
        }
        if !remaining.starts_with('<') {
            if let Some(pos) = remaining.find('<') {
                remaining = &remaining[pos..];
                continue;
            }
            break;
        }
        remaining = &remaining[1..];
        let close = match remaining.find('>') {
            Some(p) => p,
            None => break,
        };
        let raw_href = &remaining[..close];
        remaining = &remaining[close + 1..];

        let href = resolve_href(raw_href.trim(), base);
        let mut rel = None;
        let mut media_type = None;
        let mut title = None;
        let mut title_star_seen = false;
        let mut has_anchor = false;
        let mut extra_params = std::collections::BTreeMap::new();

        loop {
            remaining = remaining.trim_start();
            if remaining.starts_with(';') {
                remaining = remaining[1..].trim_start();
            } else {
                break;
            }

            let (key, value, rest) = parse_param(remaining);
            remaining = rest;

            match key.to_ascii_lowercase().as_str() {
                "rel" => {
                    if rel.is_none() {
                        rel = Some(value);
                    }
                }
                "type" => {
                    if media_type.is_none() {
                        media_type = Some(value);
                    }
                }
                "title" => {
                    if title.is_none() {
                        title = Some(value);
                    }
                }
                "title*" => {
                    if !title_star_seen {
                        if let Some(decoded) = decode_ext_value(&value) {
                            title_star_seen = true;
                            title = Some(decoded);
                        }
                    }
                }
                "anchor" => has_anchor = true,
                other => {
                    let norm = other.strip_suffix('*').unwrap_or(other);
                    if !extra_params.contains_key(norm) {
                        let decoded = if other.ends_with('*') {
                            decode_ext_value(&value).unwrap_or(value)
                        } else {
                            value
                        };
                        extra_params.insert(norm.to_string(), decoded);
                    }
                }
            }
        }

        if remaining.starts_with(',') {
            remaining = &remaining[1..];
        }

        if has_anchor {
            continue;
        }

        if let Some(rel_val) = rel {
            for r in rel_val.split_ascii_whitespace() {
                out.push(Link {
                    href: href.clone(),
                    rel: r.to_ascii_lowercase(),
                    media_type: media_type.clone(),
                    title: title.clone(),
                    params: extra_params.clone(),
                });
            }
        }
    }
}

fn resolve_href(raw: &str, base: &Url) -> String {
    match base.join(raw) {
        Ok(u) => u.to_string(),
        Err(_) => raw.to_string(),
    }
}

fn parse_param(input: &str) -> (String, String, &str) {
    let mut remaining = input;
    let key_end = remaining
        .find(['=', ';', ',', '<'])
        .unwrap_or(remaining.len());
    let key = remaining[..key_end].trim().to_string();
    remaining = &remaining[key_end..];

    if !remaining.starts_with('=') {
        return (key, String::new(), remaining);
    }
    remaining = remaining[1..].trim_start();

    if remaining.starts_with('"') {
        let (val, rest) = parse_quoted(remaining);
        (key, val, rest)
    } else {
        let end = remaining
            .find([';', ',', '<'])
            .unwrap_or(remaining.len());
        let val = remaining[..end].trim().to_string();
        (key, val, &remaining[end..])
    }
}

fn parse_quoted(input: &str) -> (String, &str) {
    let mut chars = input[1..].chars();
    let mut value = String::new();
    let mut consumed = 1;
    loop {
        match chars.next() {
            None => break,
            Some('"') => {
                consumed += 1;
                break;
            }
            Some('\\') => {
                consumed += 1;
                if let Some(c) = chars.next() {
                    value.push(c);
                    consumed += c.len_utf8();
                }
            }
            Some(c) => {
                value.push(c);
                consumed += c.len_utf8();
            }
        }
    }
    (value, &input[consumed..])
}

fn decode_ext_value(raw: &str) -> Option<String> {
    let apos1 = raw.find('\'')?;
    let rest = &raw[apos1 + 1..];
    let apos2 = rest.find('\'')?;
    let encoded = &rest[apos2 + 1..];

    let charset = raw[..apos1].to_ascii_uppercase();
    if charset != "UTF-8" {
        return None;
    }

    percent_decode(encoded)
}

fn percent_decode(input: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut chars = input.bytes();
    loop {
        match chars.next() {
            None => break,
            Some(b'%') => {
                let hi = chars.next()?;
                let lo = chars.next()?;
                let val = hex_val(hi)? << 4 | hex_val(lo)?;
                bytes.push(val);
            }
            Some(b) => bytes.push(b),
        }
    }
    String::from_utf8(bytes).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Url {
        Url::parse("https://example.com/page").unwrap()
    }

    #[test]
    fn simple_link() {
        let links = parse_link_header(r#"</.well-known/ard.json>; rel="ard"; type="application/json""#, &base());
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].href, "https://example.com/.well-known/ard.json");
        assert_eq!(links[0].rel, "ard");
        assert_eq!(links[0].media_type.as_deref(), Some("application/json"));
    }

    #[test]
    fn multiple_links_comma_separated() {
        let links = parse_link_header(
            r#"</a>; rel="first", </b>; rel="second""#,
            &base(),
        );
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].rel, "first");
        assert_eq!(links[1].rel, "second");
    }

    #[test]
    fn multiple_header_instances_newline() {
        let links = parse_link_header(
            "</a>; rel=\"one\"\n</b>; rel=\"two\"",
            &base(),
        );
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].rel, "one");
        assert_eq!(links[1].rel, "two");
    }

    #[test]
    fn quoted_comma_in_title() {
        let links = parse_link_header(
            r#"</x>; rel="thing"; title="a, b""#,
            &base(),
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].title.as_deref(), Some("a, b"));
    }

    #[test]
    fn ext_value_title_star() {
        let links = parse_link_header(
            "</x>; rel=\"thing\"; title*=UTF-8''caf%C3%A9",
            &base(),
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].title.as_deref(), Some("café"));
    }

    #[test]
    fn title_star_overrides_title() {
        let links = parse_link_header(
            "</x>; rel=\"thing\"; title=\"ascii\"; title*=UTF-8''caf%C3%A9",
            &base(),
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].title.as_deref(), Some("café"));
    }

    #[test]
    fn relative_href() {
        let b = Url::parse("https://example.com/dir/page").unwrap();
        let links = parse_link_header(r#"<../other>; rel="rel""#, &b);
        assert_eq!(links[0].href, "https://example.com/other");
    }

    #[test]
    fn absolute_href() {
        let links = parse_link_header(
            r#"<https://cdn.example.com/x>; rel="preload""#,
            &base(),
        );
        assert_eq!(links[0].href, "https://cdn.example.com/x");
    }

    #[test]
    fn empty_header() {
        let links = parse_link_header("", &base());
        assert!(links.is_empty());
    }

    #[test]
    fn no_rel_skipped() {
        let links = parse_link_header("</x>; type=\"text/plain\"", &base());
        assert!(links.is_empty());
    }

    #[test]
    fn multiple_rels() {
        let links = parse_link_header(r#"</x>; rel="a b""#, &base());
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].rel, "a");
        assert_eq!(links[1].rel, "b");
    }

    #[test]
    fn rel_case_insensitive() {
        let links = parse_link_header(r#"</x>; rel="ARD""#, &base());
        assert_eq!(links[0].rel, "ard");
    }

    #[test]
    fn duplicate_rel_first_wins() {
        let links = parse_link_header(
            r#"</x>; rel="first"; rel="second""#,
            &base(),
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].rel, "first");
    }

    #[test]
    fn duplicate_type_first_wins() {
        let links = parse_link_header(
            r#"</x>; rel="r"; type="text/plain"; type="application/json""#,
            &base(),
        );
        assert_eq!(links[0].media_type.as_deref(), Some("text/plain"));
    }

    #[test]
    fn duplicate_title_first_wins() {
        let links = parse_link_header(
            r#"</x>; rel="r"; title="first"; title="second""#,
            &base(),
        );
        assert_eq!(links[0].title.as_deref(), Some("first"));
    }

    #[test]
    fn whitespace_around_equals() {
        let links = parse_link_header(
            r#"</x>; rel = "ard"; type = "application/json""#,
            &base(),
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].rel, "ard");
        assert_eq!(links[0].media_type.as_deref(), Some("application/json"));
    }

    #[test]
    fn anchor_parameter_skips_link() {
        let links = parse_link_header(
            r#"</x>; rel="ard"; anchor="/other""#,
            &base(),
        );
        assert!(links.is_empty());
    }

    #[test]
    fn anchor_on_one_link_does_not_affect_others() {
        let links = parse_link_header(
            r#"</a>; rel="first"; anchor="/z", </b>; rel="second""#,
            &base(),
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].rel, "second");
    }

    #[test]
    fn duplicate_title_star_first_wins() {
        let links = parse_link_header(
            "</x>; rel=\"r\"; title*=UTF-8''first; title*=UTF-8''second",
            &base(),
        );
        assert_eq!(links[0].title.as_deref(), Some("first"));
    }
}
