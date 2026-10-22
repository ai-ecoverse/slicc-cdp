use cdp_client::protected::Protected;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const SCRIPT: &str = include_str!("aria-snapshot.js");

pub const ELEMENTS_KEY: &str = "__sliccAriaElements";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RefRec {
    pub backend_node_id: Option<u64>,
    pub selector: String,
    pub frame_id: Option<String>,
    pub label: Option<String>,
    pub target: Option<String>,
    pub owner: Option<u64>,
    pub page: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedRef {
    pub id: String,
    pub selector: String,
    pub backend_node_id: Option<u64>,
    pub content_frame: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    pub json: Vec<Value>,
    pub refs: Vec<CapturedRef>,
    pub iframe_depths: BTreeMap<String, i64>,
    pub base: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub focused: Option<bool>,
    pub depth: Option<i64>,
    pub boxes: bool,
}

pub fn ref_label(role: &str, name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let full = format!("{role} \"{}\"", escape_label(name));
    Some(full.chars().take(80).collect())
}

fn escape_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.replace('\\', "\\\\").replace('"', "\\\"").chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

pub fn expression(prefix: &str, options: Options) -> String {
    let options = json!({
        "prefix": prefix,
        "focused": options.focused,
        "depth": options.depth,
        "boxes": options.boxes,
        "key": ELEMENTS_KEY,
    });
    format!("{SCRIPT}\nsliccAriaSnapshot({options})")
}

pub fn parse_capture(value: &Value) -> Result<Capture, String> {
    let json = value
        .get("json")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "snapshot: the page returned no tree\n".to_string())?;
    let refs = value
        .get("refs")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(CapturedRef {
                        id: item.get("ref")?.as_str()?.to_string(),
                        selector: item
                            .get("selector")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        backend_node_id: None,
                        content_frame: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let base = value
        .get("base")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let iframe_depths = value
        .get("iframeDepths")
        .and_then(Value::as_object)
        .map(|depths| {
            depths
                .iter()
                .filter_map(|(id, depth)| Some((id.clone(), depth.as_i64()?)))
                .collect()
        })
        .unwrap_or_default();
    Ok(Capture { json, refs, iframe_depths, base })
}

pub fn labels(nodes: &[Value]) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    fn walk(node: &Value, out: &mut BTreeMap<String, Option<String>>) {
        if let Some(id) = node.get("ref").and_then(Value::as_str) {
            let role = node.get("role").and_then(Value::as_str).unwrap_or("");
            let name = node.get("name").and_then(Value::as_str).unwrap_or("");
            out.insert(id.to_string(), ref_label(role, name));
        }
        for child in node.get("children").and_then(Value::as_array).into_iter().flatten() {
            walk(child, out);
        }
    }
    for node in nodes {
        walk(node, &mut out);
    }
    out
}

pub fn find_mut<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
    for node in nodes {
        if node.get("ref").and_then(Value::as_str) == Some(id) {
            return Some(node);
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            if let Some(found) = find_mut(children, id) {
                return Some(found);
            }
        }
    }
    None
}

pub fn redact_links(nodes: &mut [Value], base: &str, guard: &Protected) {
    for node in nodes {
        if let Some(Value::String(url)) = node.get_mut("url") {
            *url = link(url, base, guard);
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            redact_links(children, base, guard);
        }
    }
}

pub fn link(raw: &str, base: &str, guard: &Protected) -> String {
    let absolute = cdp_client::sanitize::is_absolute_url(raw);
    let resolved = if absolute {
        raw.to_string()
    } else {
        cdp_client::sanitize::join_url(base, raw).unwrap_or_else(|| raw.to_string())
    };
    if guard.url_read_only(&resolved) || cdp_client::shield::redact_text(guard, &resolved).1 {
        return url::Url::parse(&resolved)
            .ok()
            .map(|parsed| parsed.origin().ascii_serialization())
            .filter(|origin| origin != "null")
            .unwrap_or_else(|| "about:blank".to_string());
    }
    crate::sanitize::href(if raw.starts_with("//") { &resolved } else { raw })
}

pub fn render(nodes: &[Value]) -> String {
    let mut lines = Vec::new();
    for node in nodes {
        visit(node, 0, &mut lines);
    }
    lines.join("\n")
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn visit_text(text: &str, depth: usize, lines: &mut Vec<String>) {
    lines.push(format!("{}- text: {}", indent(depth), yaml_value(text)));
}

fn visit(node: &Value, depth: usize, lines: &mut Vec<String>) {
    let text = node.get("text").and_then(Value::as_str);
    if node.get("role").and_then(Value::as_str) == Some("text") {
        visit_text(text.unwrap_or(""), depth, lines);
        return;
    }
    let key = format!("{}- {}", indent(depth), yaml_key(&create_key(node)));
    let props: Vec<(&str, &str)> = ["url", "placeholder"]
        .into_iter()
        .filter_map(|name| node.get(name).and_then(Value::as_str).map(|value| (name, value)))
        .collect();
    let children = node
        .get("children")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    match text {
        None if props.is_empty() && children.is_empty() => lines.push(key),
        Some(text) if props.is_empty() => lines.push(format!("{key}: {}", yaml_value(text))),
        _ => {
            lines.push(format!("{key}:"));
            for (name, value) in props {
                lines.push(format!("{}- /{name}: {}", indent(depth + 1), yaml_value(value)));
            }
            if let Some(text) = text {
                visit_text(text, depth + 1, lines);
                return;
            }
            for child in children {
                match child.as_str() {
                    Some(text) => visit_text(text, depth + 1, lines),
                    None => visit(child, depth + 1, lines),
                }
            }
        }
    }
}

fn create_key(node: &Value) -> String {
    let field = |name: &str| node.get(name).filter(|value| !value.is_null());
    let flag = |name: &str| field(name).and_then(Value::as_bool) == Some(true);
    let word = |name: &str| field(name).and_then(Value::as_str);
    let mut key = word("role").unwrap_or("").to_string();
    if let Some(name) = word("name").filter(|name| !name.is_empty()) {
        if name.encode_utf16().count() <= 900 {
            key.push(' ');
            key.push_str(&serde_json::to_string(name).unwrap_or_default());
        }
    }
    let tristate = |name: &str| {
        if word(name) == Some("mixed") {
            format!(" [{name}=mixed]")
        } else if flag(name) {
            format!(" [{name}]")
        } else {
            String::new()
        }
    };
    key.push_str(&tristate("checked"));
    for name in ["disabled", "expanded", "active"] {
        if flag(name) {
            key.push_str(&format!(" [{name}]"));
        }
    }
    match word("invalid") {
        Some(kind @ ("grammar" | "spelling")) => key.push_str(&format!(" [invalid={kind}]")),
        _ if flag("invalid") => key.push_str(" [invalid]"),
        _ => {}
    }
    if let Some(level) = field("level").and_then(Value::as_f64).filter(|level| *level != 0.0) {
        key.push_str(&format!(" [level={}]", field("level").map(Value::to_string).unwrap_or_else(|| level.to_string())));
    }
    key.push_str(&tristate("pressed"));
    if flag("selected") {
        key.push_str(" [selected]");
    }
    if flag("ariaHidden") {
        key.push_str(" [aria-hidden]");
    }
    if let Some(id) = word("ref").filter(|id| !id.is_empty()) {
        key.push_str(&format!(" [ref={id}]"));
        if word("cursor") == Some("pointer") {
            key.push_str(" [cursor=pointer]");
        }
    }
    if let Some(bounds) = field("box") {
        let part = |name: &str| bounds.get(name).map(Value::to_string).unwrap_or_default();
        key.push_str(&format!(
            " [box={},{},{},{}]",
            part("x"),
            part("y"),
            part("width"),
            part("height")
        ));
    }
    key
}

fn js_space(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n'
            | '\u{b}'
            | '\u{c}'
            | '\r'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

fn js_number(text: &str) -> bool {
    let text = text.trim_matches(js_space);
    if text.is_empty() || matches!(text, "Infinity" | "+Infinity" | "-Infinity") {
        return true;
    }
    for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
        let head = text.get(..2).map(str::to_ascii_lowercase);
        if head.as_deref() == Some(prefix) {
            let digits = &text[2..];
            return !digits.is_empty() && digits.chars().all(|ch| ch.is_digit(radix));
        }
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
        Some(at) => (&unsigned[..at], Some(&unsigned[at + 1..])),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.chars().all(|ch| ch.is_ascii_digit());
    if whole.is_empty() && fraction.is_empty() || !digits(whole) || !digits(fraction) {
        return false;
    }
    match exponent {
        None => true,
        Some(exponent) => {
            let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            !exponent.is_empty() && digits(exponent)
        }
    }
}

fn needs_quotes(text: &str) -> bool {
    let first = text.chars().next();
    let Some(first) = first else {
        return true;
    };
    let chars: Vec<char> = text.chars().collect();
    let last = chars[chars.len() - 1];
    if js_space(first) || js_space(last) {
        return true;
    }
    if chars.iter().any(|&ch| {
        matches!(ch as u32, 0x00..=0x08 | 0x0b | 0x0c | 0x0e..=0x1f | 0x7f..=0x9f)
    }) {
        return true;
    }
    if first == '-' {
        return true;
    }
    for (at, &ch) in chars.iter().enumerate() {
        let next = chars.get(at + 1).copied();
        if matches!(ch, '\n' | ':') && next.is_none_or(js_space) {
            return true;
        }
        if js_space(ch) && next == Some('#') {
            return true;
        }
        if matches!(ch, '\n' | '\r' | '{' | '}' | '`') {
            return true;
        }
    }
    if "&*],?!>|@\"'#%[".contains(first) {
        return true;
    }
    text == "~"
        || [".inf", ".nan"].contains(&text.strip_prefix(['+', '-']).unwrap_or(text).to_ascii_lowercase().as_str())
        || js_number(text)
        || ["y", "n", "yes", "no", "true", "false", "on", "off", "null"]
            .contains(&text.to_lowercase().as_str())
}

pub fn yaml_key(text: &str) -> String {
    if !needs_quotes(text) {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', "''"))
}

pub fn yaml_value(text: &str) -> String {
    if !needs_quotes(text) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if matches!(c as u32, 0x00..=0x1f | 0x7f..=0x9f) => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[derive(Debug, Clone, Default)]
pub struct FrameRec {
    pub frame_id: String,
    pub parent_frame_id: Option<String>,
    pub url: String,
    pub target: Option<String>,
}

pub fn flatten_frame_tree(tree: &Value) -> Vec<FrameRec> {
    let mut out = Vec::new();
    fn walk(node: &Value, parent: Option<&str>, out: &mut Vec<FrameRec>) {
        let Some(frame) = node.get("frame") else {
            return;
        };
        let Some(id) = frame.get("id").and_then(|v| v.as_str()) else {
            return;
        };
        out.push(FrameRec {
            frame_id: id.to_string(),
            parent_frame_id: parent.map(str::to_string),
            url: frame
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            target: None,
        });
        if let Some(children) = node.get("childFrames").and_then(|v| v.as_array()) {
            for child in children {
                walk(child, Some(id), out);
            }
        }
    }
    if tree.get("frame").is_some() {
        walk(tree, None, &mut out);
    } else if let Some(root) = tree.get("frameTree") {
        walk(root, None, &mut out);
    }
    out
}

pub fn child_options(options: Options, active: bool, iframe_depth: Option<i64>) -> Options {
    Options {
        focused: options.focused.map(|focused| focused && active),
        depth: options
            .depth
            .filter(|depth| *depth != 0)
            .map(|depth| depth - iframe_depth.unwrap_or(0) - 1),
        boxes: options.boxes,
    }
}

pub fn frame_prefix(frames: &[FrameRec], frame_id: &str, fallback: impl FnOnce() -> usize) -> String {
    let seq = frames
        .iter()
        .position(|frame| frame.frame_id == frame_id)
        .unwrap_or_else(fallback);
    if seq == 0 {
        String::new()
    } else {
        format!("f{seq}")
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/integration/fixtures/snapshot")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    #[test]
    fn renders_upstream_json_exactly_as_upstream_prints_it() {
        for name in ["example", "forms", "links", "frames"] {
            let json: Value = serde_json::from_str(&fixture(&format!("{name}.upstream.json"))).unwrap();
            let yaml = fixture(&format!("{name}.upstream.yml"));
            assert_eq!(render(json.as_array().unwrap()), yaml.trim_end_matches('\n'), "{name}");
        }
    }

    #[test]
    fn keys_carry_every_upstream_attribute() {
        let node = json!({
            "role": "checkbox", "name": "A", "checked": "mixed", "disabled": true, "expanded": true,
            "active": true, "invalid": "spelling", "level": 2, "pressed": true, "selected": true,
            "ariaHidden": true, "ref": "e1", "cursor": "pointer",
            "box": { "x": 1, "y": 2, "width": 3, "height": 4 }
        });
        assert_eq!(
            render(&[node]),
            "- checkbox \"A\" [checked=mixed] [disabled] [expanded] [active] [invalid=spelling] [level=2] [pressed] [selected] [aria-hidden] [ref=e1] [cursor=pointer] [box=1,2,3,4]"
        );
        let plain = json!({ "role": "button", "checked": true, "invalid": true, "pressed": "mixed", "level": 0, "ref": "" });
        assert_eq!(render(&[plain]), "- button [checked] [invalid] [pressed=mixed]");
        let long = json!({ "role": "img", "name": "x".repeat(901), "cursor": "pointer" });
        assert_eq!(render(&[long]), "- img");
    }

    #[test]
    fn text_children_props_and_top_level_text_render_like_upstream() {
        let nodes = [
            json!({ "role": "text", "text": "top" }),
            json!({ "role": "text" }),
            json!({ "role": "textbox", "placeholder": "Find", "text": "abc" }),
            json!({ "role": "link", "url": "/a", "children": ["one", { "role": "img", "name": "Logo" }] }),
        ];
        assert_eq!(
            render(&nodes),
            "- text: top\n- text: \"\"\n- textbox:\n  - /placeholder: Find\n  - text: abc\n- link:\n  - /url: /a\n  - text: one\n  - img \"Logo\""
        );
    }

    #[test]
    fn yaml_quoting_follows_upstream_rules() {
        for (text, quoted) in [
            ("plain words", false),
            ("", true),
            (" lead", true),
            ("trail\u{3000}", true),
            ("bell\u{7}", true),
            ("c1\u{85}x", true),
            ("-dash", true),
            ("key: value", true),
            ("ends:", true),
            ("a:b", false),
            ("line\nbreak", true),
            ("cr\rhere", true),
            ("x #tag", true),
            ("x#tag", false),
            ("&anchor", true),
            ("[list", true),
            ("brace{", true),
            ("tick`", true),
            ("Yes", true),
            ("null", true),
            ("12", true),
            ("-3.5e+2", true),
            (".5", true),
            ("5.", true),
            ("1e", false),
            ("e5", false),
            (".", false),
            ("0x1F", true),
            ("0b102", false),
            ("0o17", true),
            ("0x", false),
            ("+Infinity", true),
            ("1.2.3", false),
            ("1e+", false),
        ] {
            assert_eq!(needs_quotes(text), quoted, "{text:?}");
        }
        assert_eq!(yaml_value("a\\\"\u{8}\u{c}\n\r\t\u{1}\u{9f}é"), "\"a\\\\\\\"\\b\\f\\n\\r\\t\\x01\\x9fé\"");
        assert_eq!(yaml_value("fine"), "fine");
        assert_eq!(yaml_key("it's: x"), "'it''s: x'");
        assert_eq!(yaml_key("fine"), "fine");
    }

    #[test]
    fn links_are_redacted_like_every_other_url() {
        let guard = Protected::default();
        let base = "http://127.0.0.1:1/snapshot/links.html";
        crate::sanitize::set_full_urls(false);
        assert_eq!(link("https://seven.sliccy.ai/os/?k=SECRET#inbox", base, &guard), "https://seven.sliccy.ai");
        assert_eq!(link("//vault.sliccy.ai/v/item?x=SECRET", base, &guard), "http://vault.sliccy.ai");
        assert_eq!(link("/x?y=SECRET", "https://seven.sliccy.ai", &guard), "https://seven.sliccy.ai");
        assert_eq!(link("/next?page=3#top", base, &guard), "/next?page=\u{2026}#\u{2026}");
        assert_eq!(link("https://example.org/a?token=SECRET#frag", base, &guard), "https://example.org/a?token=\u{2026}#\u{2026}");
        assert_eq!(link("relative", "", &guard), "relative");
        assert_eq!(link("about:blank#x", base, &guard), "about:blank#\u{2026}");
        crate::sanitize::set_full_urls(true);
        assert_eq!(link("/next?page=3", base, &guard), "/next?page=3");
        assert_eq!(link("https://seven.sliccy.ai/os/?k=SECRET", base, &guard), "https://seven.sliccy.ai");
        crate::sanitize::set_full_urls(false);
        let mut nodes = vec![json!({ "role": "list", "children": ["x", { "role": "link", "url": "/a?b=SECRET" }] })];
        redact_links(&mut nodes, base, &guard);
        assert_eq!(nodes[0]["children"][1]["url"], "/a?b=\u{2026}");
    }

    #[test]
    fn captures_parse_and_index_their_refs() {
        assert!(parse_capture(&Value::Null).is_err());
        let capture = parse_capture(&json!({
            "json": [{ "role": "generic", "ref": "e1", "children": ["t", { "role": "button", "name": "Say \"hi\"\n", "ref": "e2" }] }],
            "refs": [{ "ref": "e1", "selector": ":root > :nth-child(2)" }, { "ref": "e2" }, { "selector": "x" }],
            "iframeDepths": { "e2": 1, "e3": "x" },
            "base": "http://h/"
        }))
        .unwrap();
        assert_eq!(capture.refs.len(), 2);
        assert_eq!(capture.refs[1].selector, "");
        assert_eq!(capture.base, "http://h/");
        assert_eq!(capture.iframe_depths, BTreeMap::from([("e2".to_string(), 1)]));
        let labels = labels(&capture.json);
        assert_eq!(labels["e1"], None);
        assert_eq!(labels["e2"].as_deref(), Some("button \"Say \\\"hi\\\"\\n\""));
        let mut nodes = capture.json.clone();
        assert_eq!(find_mut(&mut nodes, "e2").unwrap()["role"], "button");
        assert!(find_mut(&mut nodes, "e9").is_none());
        assert_eq!(parse_capture(&json!({ "json": [] })).unwrap().base, "");
    }

    #[test]
    fn labels_escape_and_cap_at_80() {
        assert_eq!(ref_label("button", ""), None);
        assert_eq!(ref_label("button", "a\tb\r\u{1b}").as_deref(), Some("button \"a\\tb\\r\\u{1b}\""));
        let label = ref_label("button", &"é".repeat(90)).unwrap();
        assert_eq!(label.chars().count(), 80);
    }

    #[test]
    fn frames_number_like_the_frame_tree_and_the_script_gets_its_options() {
        let frames = flatten_frame_tree(&json!({ "frameTree": {
            "frame": { "id": "M", "url": "http://h/" },
            "childFrames": [{ "frame": { "id": "C" } }, { "frame": {} }, { "nope": 1 }]
        }}));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].parent_frame_id.as_deref(), Some("M"));
        assert_eq!(frames[1].url, "");
        assert!(flatten_frame_tree(&json!({})).is_empty());
        assert_eq!(flatten_frame_tree(&json!({ "frame": { "id": "X" } })).len(), 1);
        assert_eq!(frame_prefix(&frames, "M", || 9), "");
        assert_eq!(frame_prefix(&frames, "C", || 9), "f1");
        assert_eq!(frame_prefix(&frames, "Z", || 9), "f9");
        let options = Options { focused: Some(false), depth: Some(3), boxes: true };
        let expression = expression("f1", options);
        assert!(expression.starts_with(SCRIPT));
        assert!(expression.ends_with(&format!(
            "\nsliccAriaSnapshot({{\"boxes\":true,\"depth\":3,\"focused\":false,\"key\":\"{ELEMENTS_KEY}\",\"prefix\":\"f1\"}})"
        )));
        assert_eq!(child_options(options, true, Some(1)), Options { focused: Some(false), depth: Some(1), boxes: true });
        let top = Options { focused: Some(true), depth: Some(0), boxes: false };
        assert_eq!(child_options(top, true, None), Options { focused: Some(true), depth: None, boxes: false });
        assert_eq!(child_options(Options::default(), false, Some(2)), Options::default());
    }
}
