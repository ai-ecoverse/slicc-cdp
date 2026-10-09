use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxNode {
    pub role: String,
    pub name: String,
    pub value: String,
    pub backend_node_id: Option<u64>,
    pub children: Vec<AxNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefRec {
    pub backend_node_id: Option<u64>,
    pub selector: String,
    pub frame_id: Option<String>,
}

pub fn render_tree(node: &AxNode, frame_prefix: &str) -> (String, BTreeMap<String, RefRec>) {
    let mut refs = BTreeMap::new();
    let mut counter = 0u32;
    let lines = render_node(node, frame_prefix, "", &mut counter, &mut refs);
    (lines.join("\n"), refs)
}

fn render_node(
    node: &AxNode,
    frame_prefix: &str,
    indent: &str,
    counter: &mut u32,
    refs: &mut BTreeMap<String, RefRec>,
) -> Vec<String> {
    let role = node.role.to_ascii_lowercase();
    let name = node.name.clone();
    let value = node.value.clone();
    let mut ref_id = String::new();
    if node_needs_ref(&role, &name) {
        *counter += 1;
        ref_id = format!("{frame_prefix}e{counter}");
        refs.insert(
            ref_id.clone(),
            RefRec {
                backend_node_id: node.backend_node_id,
                selector: build_ref_selector(&role, &name),
                frame_id: None,
            },
        );
    }
    let mut line = format!("{indent}- {role}");
    if !name.is_empty() {
        line.push_str(&format!(" \"{}\"", escape_yaml(&name)));
    }
    if !ref_id.is_empty() {
        line.push_str(&format!(" [ref={ref_id}]"));
    }
    if !value.is_empty() {
        line.push_str(&format!(": \"{}\"", escape_yaml(&value)));
    }
    let mut lines = vec![line];
    let child_indent = format!("{indent}  ");
    for child in &node.children {
        lines.extend(render_node(
            child,
            frame_prefix,
            &child_indent,
            counter,
            refs,
        ));
    }
    lines
}

fn node_needs_ref(role: &str, name: &str) -> bool {
    if matches!(role, "none" | "presentation" | "generic" | "rootwebarea") {
        return false;
    }
    !name.is_empty() || matches!(role, "textbox" | "button" | "link" | "checkbox" | "radio")
}

fn build_ref_selector(role: &str, name: &str) -> String {
    let escaped = escape_css(name);
    if role == "button" && !name.is_empty() {
        return [
            format!("button[aria-label=\"{escaped}\"]"),
            format!("button[title=\"{escaped}\"]"),
            format!("[role=\"button\"][aria-label=\"{escaped}\"]"),
            format!("[role=\"button\"][title=\"{escaped}\"]"),
            format!("input[type=\"button\"][value=\"{escaped}\"]"),
            format!("input[type=\"submit\"][value=\"{escaped}\"]"),
            format!("input[type=\"reset\"][value=\"{escaped}\"]"),
        ]
        .join(", ");
    }
    if role == "link" && !name.is_empty() {
        return format!(
            "a[aria-label=\"{escaped}\"], a[title=\"{escaped}\"], [role=\"link\"][aria-label=\"{escaped}\"], [role=\"link\"][title=\"{escaped}\"]"
        );
    }
    if role == "textbox" {
        if name.is_empty() {
            return "input, textarea, [contenteditable]".to_string();
        }
        return format!(
            "input[aria-label=\"{escaped}\"], textarea[aria-label=\"{escaped}\"], [contenteditable][aria-label=\"{escaped}\"], input[placeholder=\"{escaped}\"], textarea[placeholder=\"{escaped}\"], [contenteditable][placeholder=\"{escaped}\"], input[title=\"{escaped}\"], textarea[title=\"{escaped}\"], [contenteditable][title=\"{escaped}\"]"
        );
    }
    if role == "checkbox" {
        return "input[type=\"checkbox\"]".to_string();
    }
    if role == "radio" {
        return "input[type=\"radio\"]".to_string();
    }
    if !name.is_empty() {
        return format!("[aria-label=\"{escaped}\"], [title=\"{escaped}\"]");
    }
    format!("[role=\"{role}\"]")
}

pub fn escape_yaml(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn escape_css(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn text_of(value: &serde_json::Value, fallback: &str) -> String {
    match value {
        serde_json::Value::Null => fallback.to_string(),
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Bool(flag) => flag.to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| fallback.to_string()),
    }
}

pub fn ax_from_cdp(nodes: &[serde_json::Value]) -> Vec<AxNode> {
    use std::collections::HashMap;
    let mut by_id: HashMap<String, &serde_json::Value> = HashMap::new();
    for node in nodes {
        if let Some(id) = node.get("nodeId").and_then(|v| v.as_str()) {
            by_id.insert(id.to_string(), node);
        }
    }
    let roots: Vec<String> = nodes
        .iter()
        .filter(|node| node.get("parentId").and_then(|v| v.as_str()).is_none())
        .filter_map(|node| node.get("nodeId").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    fn child_ids<'a>(node: &'a serde_json::Value) -> Vec<&'a str> {
        node.get("childIds")
            .and_then(|v| v.as_array())
            .map(|ids| ids.iter().filter_map(|id| id.as_str()).collect())
            .unwrap_or_default()
    }
    fn build(
        id: &str,
        by_id: &HashMap<String, &serde_json::Value>,
    ) -> Vec<AxNode> {
        let Some(node) = by_id.get(id) else {
            return Vec::new();
        };
        let mut children = Vec::new();
        for child in child_ids(node) {
            children.extend(build(child, by_id));
        }
        if node.get("ignored").and_then(|v| v.as_bool()).unwrap_or(false) {
            return children;
        }
        let role = node
            .get("role")
            .map(|v| text_of(v.get("value").unwrap_or(v), "unknown"))
            .unwrap_or_else(|| "unknown".to_string());
        let name = node
            .get("name")
            .map(|v| text_of(v.get("value").unwrap_or(v), ""))
            .unwrap_or_default();
        let value = node
            .get("value")
            .map(|v| text_of(v.get("value").unwrap_or(v), ""))
            .unwrap_or_default();
        let backend_node_id = node.get("backendDOMNodeId").and_then(|v| v.as_u64());
        vec![AxNode {
            role,
            name,
            value,
            backend_node_id,
            children,
        }]
    }
    roots
        .iter()
        .flat_map(|id| build(id, &by_id))
        .collect()
}

#[derive(Debug, Clone)]
pub struct FrameRec {
    pub frame_id: String,
    pub parent_frame_id: Option<String>,
    pub url: String,
}

pub fn flatten_frame_tree(tree: &serde_json::Value) -> Vec<FrameRec> {
    let mut out = Vec::new();
    fn walk(node: &serde_json::Value, parent: Option<&str>, out: &mut Vec<FrameRec>) {
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

pub fn stitch_iframes(
    content: &str,
    base_url: &str,
    frames: &[FrameRec],
    mut render_frame: impl FnMut(&FrameRec, &str) -> Option<String>,
) -> (String, Vec<(String, String)>) {
    let children: Vec<&FrameRec> = frames
        .iter()
        .filter(|frame| frame.parent_frame_id.is_some())
        .collect();
    if children.is_empty() {
        return (content.to_string(), Vec::new());
    }
    let mut stitched = Vec::new();
    let mut matched = Vec::new();
    let mut frame_index = 0u32;
    let mut assigned = Vec::new();
    for line in content.split('\n') {
        stitched.push(line.to_string());
        let Some(indent) = iframe_indent(line) else {
            continue;
        };
        let Some(src) = iframe_src(line) else {
            continue;
        };
        let Some(frame) = children.iter().copied().find(|frame| {
            !matched.iter().any(|id: &String| id == &frame.frame_id)
                && urls_match(&src, &frame.url, base_url)
        }) else {
            continue;
        };
        matched.push(frame.frame_id.clone());
        frame_index += 1;
        let prefix = format!("f{frame_index}");
        if let Some(text) = render_frame(frame, &prefix) {
            for child_line in text.split('\n') {
                if child_line.is_empty() {
                    continue;
                }
                stitched.push(format!("{indent}  {child_line}"));
            }
            assigned.push((prefix, frame.frame_id.clone()));
        }
    }
    (stitched.join("\n"), assigned)
}

fn iframe_indent(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("- iframe") {
        return None;
    }
    let rest = &trimmed["- iframe".len()..];
    if rest.is_empty() || rest.starts_with(' ') || rest.starts_with(':') {
        let indent_len = line.len() - trimmed.len();
        Some(line[..indent_len].to_string())
    } else {
        None
    }
}

fn iframe_src(line: &str) -> Option<String> {
    let marker = ": \"";
    let start = line.rfind(marker)? + marker.len();
    let end = line[start..].rfind('"')? + start;
    Some(unescape_yaml(&line[start..end]))
}

fn unescape_yaml(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn urls_match(src: &str, frame_url: &str, base: &str) -> bool {
    normalize_url(src, Some(base))
        .zip(normalize_url(frame_url, None))
        .map(|(left, right)| left == right)
        .unwrap_or(false)
        || src == frame_url
}

fn normalize_url(raw: &str, base: Option<&str>) -> Option<String> {
    let absolute = if raw.contains("://") {
        raw.to_string()
    } else if let Some(base) = base {
        join_url(base, raw)?
    } else {
        return None;
    };
    let (scheme_host, rest) = split_origin(&absolute)?;
    let rest = rest.split('#').next().unwrap_or(rest);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let path = path.trim_end_matches('/');
    if query.is_empty() {
        Some(format!("{scheme_host}{path}"))
    } else {
        Some(format!("{scheme_host}{path}?{query}"))
    }
}

fn split_origin(url: &str) -> Option<(&str, &str)> {
    let idx = url.find("://")?;
    let after = idx + 3;
    let slash = url[after..].find('/').map(|pos| after + pos).unwrap_or(url.len());
    Some((&url[..slash], &url[slash..]))
}

fn join_url(base: &str, raw: &str) -> Option<String> {
    if raw.starts_with('/') {
        let (origin, _) = split_origin(base)?;
        return Some(format!("{origin}{raw}"));
    }
    let (origin, path) = split_origin(base)?;
    let path = path.split('?').next().unwrap_or(path);
    let path = path.split('#').next().unwrap_or(path);
    let dir = match path.rfind('/') {
        Some(pos) => &path[..=pos],
        None => "/",
    };
    Some(format!("{origin}{dir}{raw}"))
}

pub fn tag_frame_refs(refs: &mut BTreeMap<String, RefRec>, frame_id: &str, prefix: &str) {
    for (id, rec) in refs.iter_mut() {
        if id.starts_with(prefix) {
            rec.frame_id = Some(frame_id.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, name: &str, children: Vec<AxNode>) -> AxNode {
        AxNode {
            role: role.to_string(),
            name: name.to_string(),
            value: String::new(),
            backend_node_id: None,
            children,
        }
    }

    #[test]
    fn refs_are_e1_e2_and_skip_generic() {
        let tree = node(
            "RootWebArea",
            "Example",
            vec![
                node("generic", "", vec![node("button", "Submit", vec![])]),
                node("link", "Home", vec![]),
                node("textbox", "Search", vec![]),
            ],
        );
        let (text, refs) = render_tree(&tree, "");
        assert!(text.contains("button \"Submit\" [ref=e1]"));
        assert!(text.contains("link \"Home\" [ref=e2]"));
        assert!(text.contains("textbox \"Search\" [ref=e3]"));
        assert!(refs.contains_key("e1"));
        assert!(!text.contains("[ref=e0]"));
    }

    #[test]
    fn iframe_prefix_reaches_f1e5() {
        let mut children = Vec::new();
        for name in ["A", "B", "C", "D", "E"] {
            children.push(node("button", name, vec![]));
        }
        let tree = node("RootWebArea", "Frame", children);
        let (text, refs) = render_tree(&tree, "f1");
        assert!(text.contains("[ref=f1e5]"));
        assert!(refs.contains_key("f1e5"));
        assert!(!refs.contains_key("e5"));
    }

    #[test]
    fn non_string_value_renders() {
        let tree = AxNode {
            role: "textbox".to_string(),
            name: "{\"label\":\"Message\"}".to_string(),
            value: "0".to_string(),
            backend_node_id: Some(44),
            children: vec![],
        };
        let (text, refs) = render_tree(&tree, "");
        assert!(text.contains("textbox"));
        assert!(text.contains("Message"));
        assert!(text.contains("[ref=e1]"));
        assert!(text.contains(": \"0\""));
        assert_eq!(refs["e1"].backend_node_id, Some(44));
    }

    #[test]
    fn stitch_inserts_child_frame_under_iframe_line() {
        let content = "- RootWebArea \"Page\"\n  - iframe \"Ad\": \"https://cdn.example/ad\"";
        let frames = vec![
            FrameRec {
                frame_id: "main".to_string(),
                parent_frame_id: None,
                url: "https://example.com/".to_string(),
            },
            FrameRec {
                frame_id: "child".to_string(),
                parent_frame_id: Some("main".to_string()),
                url: "https://cdn.example/ad".to_string(),
            },
        ];
        let (text, assigned) = stitch_iframes(content, "https://example.com/", &frames, |_, prefix| {
            Some(format!("- button \"Go\" [ref={prefix}e5]"))
        });
        assert!(text.contains("[ref=f1e5]"));
        assert_eq!(assigned, vec![("f1".to_string(), "child".to_string())]);
    }
}
