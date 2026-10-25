use super::*;

impl Cdp {
    pub(super) fn observe_document_order(&mut self, method: &str, params: &Value, session: Option<&str>) {
        if method != "Page.frameNavigated" { return }
        let Some(session) = session else { return };
        let Some(target) = self.session_target(session) else { return };
        if self.unobserved.contains(&target) || !self.guards.contains_key(&target) { return }
        let frame = params.get("frame").cloned().unwrap_or(Value::Null);
        let id = frame.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        self.unchecked_documents.insert((target, session.to_string(), id), frame);
    }

    pub(super) fn unchecked_document(&self, target: &str) -> bool {
        self.unchecked_documents.keys().any(|(owner, _, _)| owner == target)
    }

    pub(super) fn verify_document_order(&mut self, tree: &Value, session: Option<&str>) -> Result<(), String> {
        let Some(session) = session else { return Ok(()) };
        let Some(target) = self.session_target(session) else { return Ok(()) };
        if !self.unchecked_document(&target) { return Ok(()) }
        let mut frames = BTreeMap::new();
        let mut queue = vec![tree.get("frameTree").ok_or_else(|| self.refusal())?];
        while let Some(node) = queue.pop() {
            if frames.len() >= 4096 { return Err(self.refusal()) }
            let frame = node.get("frame").ok_or_else(|| self.refusal())?;
            let id = frame.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| self.refusal())?;
            if frames.insert(id, frame).is_some() { return Err(self.refusal()) }
            if let Some(children) = node.get("childFrames") {
                queue.extend(children.as_array().ok_or_else(|| self.refusal())?);
            }
        }
        let mut verified = self.guard_sessions.contains(session);
        for ((owner, _, id), observed) in &self.unchecked_documents {
            if owner != &target { continue }
            let fresh = frames.get(id.as_str()).ok_or_else(|| self.refusal())?;
            if ["id", "parentId", "loaderId", "url", "urlFragment", "securityOrigin", "unreachableUrl"].iter().any(|key| {
                observed.get(*key).and_then(Value::as_str).unwrap_or("") != fresh.get(*key).and_then(Value::as_str).unwrap_or("")
            }) { return Err(self.refusal()) }
            let text = |key| fresh.get(key).and_then(Value::as_str).unwrap_or("");
            verified &= !self.unverified_error(text("url"), text("unreachableUrl"));
        }
        if verified { self.unchecked_documents.retain(|(owner, _, _), _| owner != &target); }
        Ok(())
    }
}
