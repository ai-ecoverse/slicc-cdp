use super::Cdp;
#[cfg(not(target_os = "wasi"))]
use super::OPTIONAL_STEP;
use serde_json::{json, Value};

const MAX_TREE_DEPTH: usize = 64;
const MAX_TREE_NODES: usize = 4096;

fn tree_bounded(root: &Value) -> bool {
    let mut stack = vec![(root, 1usize)];
    let mut nodes = 0usize;
    while let Some((node, depth)) = stack.pop() {
        nodes += 1;
        if depth > MAX_TREE_DEPTH || nodes > MAX_TREE_NODES { return false; }
        for child in node.get("childFrames").and_then(Value::as_array).into_iter().flatten() { stack.push((child, depth + 1)); }
    }
    true
}

impl Cdp {
    fn document_route_attested(&mut self) -> bool {
        if self.route_attested { return true; }
        #[cfg(target_os = "wasi")]
        {
            self.route_attested = true;
            return true;
        }
        #[cfg(not(target_os = "wasi"))]
        {
            let version = self.raw_call("Browser.getVersion", json!({}), None, OPTIONAL_STEP);
            let attested = Self::document_version_attested(version);
            if attested { self.route_attested = true; }
            attested
        }
    }

    #[cfg(not(target_os = "wasi"))]
    fn document_version_attested(version: Result<Value, String>) -> bool {
        let Ok(Value::Object(version)) = version else { return true; };
        let slicc = match version.get("slicc") {
            None | Some(Value::Null) => return false,
            Some(Value::Object(slicc)) => slicc,
            Some(_) => return true,
        };
        match slicc.get("route") {
            None => false,
            Some(Value::String(_)) => true,
            Some(Value::Object(route)) => route.get("kind").is_some(),
            Some(_) => true,
        }
    }

    pub fn eval_attach_document(&mut self, session: &str, frame: &str) -> Result<Value, String> {
        let epoch = self.frame_epoch;
        let tree = self.call("Page.getFrameTree", json!({}), Some(session))?;
        if epoch != self.frame_epoch || !self.document_tree_verified(&tree, Some(frame)) || self.content_blocked(session) {
            return Err(self.refusal());
        }
        Self::document_frame(&tree["frameTree"], frame).cloned().ok_or_else(|| self.refusal())
    }

    pub fn eval_attach_cross_origin_available(&mut self) -> bool {
        !self.document_route_attested()
    }

    fn document_frame_refusal(&self, frame: &Value) -> Option<&'static str> {
        let text = |key: &str| frame.get(key).and_then(Value::as_str).unwrap_or("");
        if text("url").is_empty() { return Some("empty document URL"); }
        let Ok(url) = url::Url::parse(text("url")) else { return Some("unknown document URL"); };
        if self.guard.url(text("url")) || self.guard.url(text("securityOrigin")) { return Some("protected document"); }
        if !matches!(url.scheme(), "http" | "https") { return Some("non-HTTP document"); }
        if url.origin().ascii_serialization() != text("securityOrigin") { return Some("document origin mismatch"); }
        if text("id").is_empty() { return Some("missing frame id"); }
        if text("loaderId").is_empty() { return Some("missing document loader"); }
        if !text("unreachableUrl").is_empty() { return Some("unreachable document"); }
        None
    }

    fn document_frame_verified(&self, frame: &Value) -> bool {
        self.document_frame_refusal(frame).is_none()
    }

    fn document_tree_verified(&self, tree: &Value, frame: Option<&str>) -> bool {
        fn find(cdp: &Cdp, node: &Value, frame: Option<&str>) -> bool {
            let current = &node["frame"];
            let id = current.get("id").and_then(Value::as_str).unwrap_or("");
            if !cdp.document_frame_verified(current) || cdp.frame_protected(id) { return false; }
            if frame.is_none_or(|frame| frame == id) { return true; }
            node.get("childFrames").and_then(Value::as_array).is_some_and(|children| children.iter().any(|child| find(cdp, child, frame)))
        }
        tree.get("frameTree").is_some_and(|node| tree_bounded(node) && find(self, node, frame))
    }

    fn document_frame<'a>(node: &'a Value, frame: &str) -> Option<&'a Value> {
        if !tree_bounded(node) { return None; }
        Self::document_frame_walk(node, frame)
    }

    fn document_frame_walk<'a>(node: &'a Value, frame: &str) -> Option<&'a Value> {
        if node.pointer("/frame/id").and_then(Value::as_str) == Some(frame) { return node.get("frame"); }
        node.get("childFrames").and_then(Value::as_array)?.iter().find_map(|child| Self::document_frame_walk(child, frame))
    }
}

#[cfg(test)]
mod tests {
    include!("document_verification_test.rs");
}
