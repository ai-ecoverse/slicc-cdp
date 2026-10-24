use super::*;

const MAX_FRAMES: usize = 4096;
const MAX_TARGETS: usize = 256;

pub(super) fn content(method: &str) -> bool {
    ["DOM.", "CSS.", "DOMSnapshot.", "Accessibility.", "Input."].iter().any(|prefix| method.starts_with(prefix))
        || matches!(method, "Page.captureScreenshot" | "Page.printToPDF" | "Page.startScreencast")
}

impl Cdp {
    pub(super) fn verify_page_scope(&mut self, session: Option<&str>) -> Result<(), String> {
        let epoch = self.frame_epoch;
        let target = session.and_then(|session| self.session_target(session)).ok_or_else(|| self.refusal())?;
        let listed = self.raw_call("Target.getTargets", json!({"filter":[{}]}), None, DOCUMENT_CHECK).map_err(|_| self.refusal())?;
        let infos = listed.get("targetInfos").and_then(Value::as_array).ok_or_else(|| self.refusal())?;
        let selected = infos.iter().find(|info| info.get("targetId").and_then(Value::as_str) == Some(&target)).ok_or_else(|| self.refusal())?;
        if !matches!(selected.get("type").and_then(Value::as_str), Some("page" | "iframe")) { return Err(self.refusal()) }
        let context = selected.get("browserContextId").cloned();
        let has_iframes = infos.iter().any(|info| info.get("browserContextId") == context.as_ref() && info.get("type").and_then(Value::as_str) == Some("iframe"));
        let infos: Vec<Value> = infos.iter().filter(|info| info.get("browserContextId") == context.as_ref()
            && matches!(info.get("type").and_then(Value::as_str), Some("page" | "iframe"))
            && (has_iframes || info.get("targetId").and_then(Value::as_str) == Some(&target))).cloned().collect();
        if infos.len() > MAX_TARGETS { return Err(self.refusal()) }
        let mut ids = BTreeSet::new();
        if infos.iter().any(|info| info.get("targetId").and_then(Value::as_str).is_none_or(|id| !ids.insert(id))) { return Err(self.refusal()) }
        let mut nodes = BTreeMap::new();
        let mut roots = BTreeMap::new();
        let mut iframe_roots = Vec::new();
        let mut protected_iframes = Vec::new();
        let deadline = Instant::now() + DOCUMENT_CHECK;
        for info in &infos {
            let id = info.get("targetId").and_then(Value::as_str).filter(|id| frame_text_known(id)).ok_or_else(|| self.refusal())?;
            let url = info.get("url").and_then(Value::as_str).ok_or_else(|| self.refusal())?;
            let iframe = info.get("type").and_then(Value::as_str) == Some("iframe");
            let protected = !frame_text_known(url) || self.info_protected(info, &[]) || self.protected_targets.contains(id);
            if protected {
                let parent = if iframe { Some(info.get("parentFrameId").and_then(Value::as_str).filter(|id| frame_text_known(id)).ok_or_else(|| self.refusal())?) } else { None };
                nodes.entry(id.to_string()).or_insert_with(|| json!({"id":id,"parentId":parent,"url":url,"securityOrigin":url,"loaderId":"protected"}));
                if iframe { iframe_roots.push(id.to_string()); protected_iframes.push(id.to_string()); } else { roots.insert(id.to_string(), id.to_string()); }
                continue;
            }
            let tree = self.page_scope_tree(id, deadline)?;
            let frame = tree.pointer("/frameTree/frame").ok_or_else(|| self.refusal())?;
            let root = frame.get("id").and_then(Value::as_str).filter(|id| frame_text_known(id)).ok_or_else(|| self.refusal())?;
            if iframe {
                let parent = info.get("parentFrameId").and_then(Value::as_str).filter(|id| frame_text_known(id)).ok_or_else(|| self.refusal())?;
                if frame.get("parentId").and_then(Value::as_str) != Some(parent) || root != id { return Err(self.refusal()) }
                iframe_roots.push(root.to_string());
            } else {
                if frame.get("parentId").is_some() || roots.insert(root.to_string(), id.to_string()).is_some() { return Err(self.refusal()) }
            }
            Self::page_scope_nodes(tree.get("frameTree").unwrap(), None, &mut nodes).ok_or_else(|| self.refusal())?;
        }
        let owner = |id: &str| -> Option<String> {
            let mut current = id;
            let mut visited = BTreeSet::new();
            while visited.insert(current.to_string()) {
                if let Some(owner) = roots.get(current) { return Some(owner.clone()) }
                current = nodes.get(current)?.get("parentId")?.as_str()?;
            }
            None
        };
        for id in &iframe_roots { owner(id).ok_or_else(|| self.refusal())?; }
        let page = if selected.get("type").and_then(Value::as_str) == Some("page") { target.clone() } else { owner(&target).ok_or_else(|| self.refusal())? };
        if protected_iframes.iter().any(|id| owner(id).as_deref() == Some(&page)) { return Err(self.refusal()) }
        let root = roots.iter().find(|(_, owner)| *owner == &page).map(|(id, _)| id).ok_or_else(|| self.refusal())?;
        let tree = Self::page_scope_join(root, &nodes, &mut BTreeSet::new()).ok_or_else(|| self.refusal())?;
        let mut frames = BTreeMap::new();
        if epoch != self.frame_epoch || !self.collect_document_frames(&tree, &mut frames, true, None)
            || frames.values().any(|frame| self.guard.url(&frame[1]) || self.guard.url(&frame[2])) { return Err(self.refusal()) }
        Ok(())
    }

    fn page_scope_tree(&mut self, target: &str, deadline: Instant) -> Result<Value, String> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() { return Err(self.refusal()) }
        let session = match self.guards.get(target).or_else(|| self.page_scope_sessions.get(target)).cloned() {
            Some(session) => session,
            None => {
                let result = self.raw_call("Target.attachToTarget", json!({"targetId":target,"flatten":true}), None, left).map_err(|_| self.refusal())?;
                let session = result.get("sessionId").and_then(Value::as_str).filter(|id| !id.is_empty()).ok_or_else(|| self.refusal())?.to_string();
                self.guard_sessions.insert(session.clone());
                self.session_targets.insert(session.clone(), target.to_string());
                self.page_scope_sessions.insert(target.to_string(), session.clone());
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() || self.raw_call("Page.enable", json!({}), Some(&session), left).is_err() {
                    self.page_scope_sessions.remove(target);
                    let _ = self.send("Target.detachFromTarget", json!({"sessionId":session}), None);
                    self.guard_sessions.remove(&session);
                    self.session_targets.remove(&session);
                    return Err(self.refusal());
                }
                session
            }
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() { return Err(self.refusal()) }
        self.raw_call("Page.getFrameTree", json!({}), Some(&session), left).map_err(|_| self.refusal())
    }

    fn page_scope_nodes(node: &Value, parent: Option<&str>, nodes: &mut BTreeMap<String, Value>) -> Option<()> {
        let frame = node.get("frame")?;
        let id = frame.get("id")?.as_str().filter(|id| frame_text_known(id))?;
        if nodes.len() >= MAX_FRAMES || parent.is_some_and(|parent| frame.get("parentId").and_then(Value::as_str) != Some(parent)) { return None }
        if let Some(previous) = nodes.get(id) {
            if previous.get("parentId") != frame.get("parentId") { return None }
        }
        nodes.insert(id.to_string(), frame.clone());
        if let Some(children) = node.get("childFrames") {
            for child in children.as_array()? { Self::page_scope_nodes(child, Some(id), nodes)?; }
        }
        Some(())
    }

    fn page_scope_join(id: &str, nodes: &BTreeMap<String, Value>, visited: &mut BTreeSet<String>) -> Option<Value> {
        if !visited.insert(id.to_string()) { return None }
        let frame = nodes.get(id)?;
        let mut children = Vec::new();
        for (child, frame) in nodes {
            if frame.get("parentId").and_then(Value::as_str) == Some(id) { children.push(Self::page_scope_join(child, nodes, visited)?); }
        }
        Some(json!({"frame":frame,"childFrames":children}))
    }
}
