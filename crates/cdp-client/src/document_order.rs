use super::*;

fn document_matches(observed: &Value, fresh: &Value) -> bool {
    ["id", "parentId", "loaderId", "url", "urlFragment", "securityOrigin", "unreachableUrl"].iter().all(|key| {
        observed.get(*key).and_then(Value::as_str).unwrap_or("") == fresh.get(*key).and_then(Value::as_str).unwrap_or("")
    })
}

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

    pub(super) fn forget_frame_order(&mut self, target: &str, session: &str, id: &str, retired: bool) {
        let guard = &self.guard;
        self.unchecked_documents.retain(|(owner, peer, frame), observed| {
            owner != target || frame != id || peer != session && (!retired
                || ["url", "securityOrigin"].iter().any(|key| observed.get(*key).and_then(Value::as_str).is_none_or(|text| !frame_text_known(text) || guard.url(text)))
                || guard.error_page_url(observed.get("url").and_then(Value::as_str).unwrap_or(""))
                || observed.get("unreachableUrl").and_then(Value::as_str).is_some_and(|url| !url.is_empty()))
        });
    }

    pub(super) fn unchecked_document(&self, target: &str) -> bool {
        self.unchecked_documents.keys().any(|(owner, _, _)| owner == target)
            || self.targets.get(target).and_then(|info| info.get("url")).and_then(Value::as_str).is_some_and(|url| self.guard.error_page_url(url))
                && self.documents.get(target).is_none_or(|doc| !self.guard.error_page_url(&doc.url) || self.unverified_error(&doc.url, &doc.unreachable_url))
    }

    pub(super) fn verify_document_order(&mut self, tree: &Value, session: Option<&str>) -> Result<(), String> {
        let Some(session) = session else { return Ok(()) };
        let Some(target) = self.session_target(session) else { return Ok(()) };
        let reported_error = self.targets.get(&target).and_then(|info| info.get("url")).and_then(Value::as_str).is_some_and(|url| self.guard.error_page_url(url));
        if reported_error && tree.pointer("/frameTree/frame/url").and_then(Value::as_str).is_none_or(|url| !self.guard.error_page_url(url)) {
            let current = self.raw_call("Target.getTargetInfo", json!({"targetId":target}), None, DOCUMENT_CHECK).map_err(|_| self.refusal())?;
            let info = current.get("targetInfo").ok_or_else(|| self.refusal())?;
            if info.get("targetId").and_then(Value::as_str) != Some(target.as_str())
                || info.get("url").and_then(Value::as_str).is_none_or(|url| self.guard.error_page_url(url) || self.guard.url(url)) { return Err(self.refusal()) }
            self.targets.insert(target.clone(), info.clone());
        }
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
        if self.guard_sessions.contains(session) {
            let guard = &self.guard;
            self.unchecked_documents.retain(|(owner, _, id), observed| owner != &target || frames.get(id.as_str()).is_none_or(|fresh| {
                !document_matches(observed, fresh) || guard.error_page_url(fresh.get("url").and_then(Value::as_str).unwrap_or(""))
                    || fresh.get("unreachableUrl").and_then(Value::as_str).is_some_and(|url| !url.is_empty())
            }));
        }
        let mut verified = self.guard_sessions.contains(session);
        for ((owner, _, id), observed) in &self.unchecked_documents {
            if owner != &target { continue }
            let fresh = frames.get(id.as_str()).ok_or_else(|| self.refusal())?;
            if !document_matches(observed, fresh) { return Err(self.refusal()) }
            let text = |key| fresh.get(key).and_then(Value::as_str).unwrap_or("");
            verified &= !self.unverified_error(text("url"), text("unreachableUrl"));
        }
        if verified { self.unchecked_documents.retain(|(owner, _, _), _| owner != &target); }
        Ok(())
    }
}
