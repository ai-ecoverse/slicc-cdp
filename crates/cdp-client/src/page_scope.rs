use super::*;

const MAX_FRAMES: usize = 4096;
const MAX_TARGETS: usize = 256;

pub(super) fn content(method: &str) -> bool {
    ["Runtime.", "DOM.", "CSS.", "DOMSnapshot.", "Accessibility.", "Input."].iter().any(|prefix| method.starts_with(prefix))
        || matches!(method, "Page.captureScreenshot" | "Page.printToPDF" | "Page.startScreencast")
}

pub(super) fn content_event(method: &str) -> bool {
    content(method) && !matches!(method, "DOM.documentUpdated" | "Runtime.executionContextCreated" | "Runtime.executionContextDestroyed" | "Runtime.executionContextsCleared")
}

impl Cdp {
    pub(super) fn verify_page_scope(&mut self, session: Option<&str>) -> Result<(String, u64, u64), String> {
        self.verify_page_scope_state(session).map(|(proof, _)| proof)
    }

    pub(super) fn verify_page_scope_state(&mut self, session: Option<&str>) -> Result<((String, u64, u64), DocumentState), String> {
        self.page_scope_state(session, false, None)
    }

    pub(super) fn verify_runtime_page_scope_state(&mut self, session: Option<&str>) -> Result<((String, u64, u64), DocumentState), String> {
        self.page_scope_state(session, true, None)
    }

    pub(super) fn page_scope_state(&mut self, session: Option<&str>, refresh_main: bool, saved_deadline: Option<Instant>) -> Result<((String, u64, u64), DocumentState), String> {
        #[cfg(test)]
        self.runtime_prepare_probe("scope-selected-session", json!({"target_known":session.and_then(|session| self.session_target(session)).is_some()}));
        let mut target = session.and_then(|session| self.session_target(session)).ok_or_else(|| self.refusal())?;
        #[cfg(test)]
        self.runtime_prepare_probe("inventory-read", json!({}));
        let left = saved_deadline.map(|deadline| deadline.saturating_duration_since(Instant::now())).unwrap_or(DOCUMENT_CHECK);
        if left.is_zero() { return Err(self.refusal()) }
        let listed = self.raw_call("Target.getTargets", json!({"filter":[{}]}), None, left).map_err(|_| self.refusal())?;
        #[cfg(test)]
        self.runtime_prepare_probe("inventory-selected-presence", json!({"inventory_valid":listed.get("targetInfos").is_some_and(Value::is_array),"selected_present":listed.get("targetInfos").and_then(Value::as_array).is_some_and(|infos| infos.iter().any(|info| info.get("targetId").and_then(Value::as_str) == Some(&target)))}));
        let infos = listed.get("targetInfos").and_then(Value::as_array).ok_or_else(|| self.refusal())?;
        let mut selected = infos.iter().find(|info| info.get("targetId").and_then(Value::as_str) == Some(&target)).ok_or_else(|| self.refusal())?;
        if matches!(selected.get("type").and_then(Value::as_str), Some("worker" | "service_worker" | "shared_worker")) {
            target = selected.get("openerId").and_then(Value::as_str).filter(|owner| self.guards.contains_key(*owner)).ok_or_else(|| self.refusal())?.to_string();
            selected = infos.iter().find(|info| info.get("targetId").and_then(Value::as_str) == Some(&target)).ok_or_else(|| self.refusal())?;
        }
        #[cfg(test)]
        self.runtime_prepare_probe("inventory-selected-type", json!({"page":selected.get("type").and_then(Value::as_str) == Some("page"),"iframe":selected.get("type").and_then(Value::as_str) == Some("iframe")}));
        if !matches!(selected.get("type").and_then(Value::as_str), Some("page" | "iframe")) { return Err(self.refusal()) }
        let context = selected.get("browserContextId").cloned();
        let candidates: Vec<Value> = infos.iter().filter(|info| info.get("browserContextId") == context.as_ref()
            && matches!(info.get("type").and_then(Value::as_str), Some("page" | "iframe"))).cloned().collect();
        let epochs = self.page_scope_epochs.clone();
        let runtime_epochs = self.runtime_scope_epochs.clone();
        let guard = self.guards.get(&target).cloned();
        let unknown = self.page_scope_unknown;
        let deadline = saved_deadline.unwrap_or_else(|| Instant::now() + DOCUMENT_CHECK);
        #[cfg(test)]
        self.runtime_prepare_probe("inventory-completeness", json!({}));
        let (infos, mut trees) = self.page_scope_inventory(&target, selected, &candidates, deadline)?;
        #[cfg(test)]
        self.runtime_prepare_probe("inventory-uniqueness", json!({"within_limit":infos.len() <= MAX_TARGETS}));
        if infos.len() > MAX_TARGETS { return Err(self.refusal()) }
        let mut ids = BTreeSet::new();
        if infos.iter().any(|info| info.get("targetId").and_then(Value::as_str).is_none_or(|id| !ids.insert(id))) { return Err(self.refusal()) }
        for info in &infos {
            if let Some(id) = info.get("targetId").and_then(Value::as_str) { self.targets.insert(id.to_string(), info.clone()); }
        }
        let mut nodes = BTreeMap::new();
        let mut roots = BTreeMap::new();
        let mut target_roots = BTreeMap::new();
        let mut iframe_roots = Vec::new();
        let mut protected_iframes = Vec::new();
        for info in &infos {
            #[cfg(test)]
            self.runtime_prepare_probe("target-frame-placement", json!({"id_known":info.get("targetId").and_then(Value::as_str).is_some_and(frame_text_known),"url_present":info.get("url").is_some()}));
            let id = info.get("targetId").and_then(Value::as_str).filter(|id| frame_text_known(id)).ok_or_else(|| self.refusal())?;
            let url = info.get("url").and_then(Value::as_str).ok_or_else(|| self.refusal())?;
            let iframe = info.get("type").and_then(Value::as_str) == Some("iframe");
            let error_probe = self.guard.error_page_url(url) && self.guards.contains_key(id) && self.documents.get(id).is_some_and(|doc| !doc.previous);
            let protected = (!frame_text_known(url) && !self.page_scope_pending_page(info)) || !error_probe && self.info_protected(info, &[]) || self.protected_targets.contains(id);
            #[cfg(test)]
            self.runtime_prepare_probe("target-origin", json!({"protected":protected,"iframe":iframe}));
            if protected {
                let parent = if iframe { Some(info.get("parentFrameId").and_then(Value::as_str).filter(|id| frame_text_known(id)).ok_or_else(|| self.refusal())?) } else { None };
                nodes.entry(id.to_string()).or_insert_with(|| json!({"id":id,"parentId":parent,"url":url,"securityOrigin":url,"loaderId":"protected"}));
                if iframe { iframe_roots.push(id.to_string()); protected_iframes.push(id.to_string()); } else { roots.insert(id.to_string(), id.to_string()); }
                continue;
            }
            #[cfg(test)]
            self.runtime_prepare_probe("renderer-frame-placement", json!({"tree_present":trees.contains_key(id),"iframe":iframe}));
            let mut tree = trees.remove(id).ok_or_else(|| self.refusal())?;
            let frame = tree.pointer_mut("/frameTree/frame").ok_or_else(|| self.refusal())?;
            let root = frame.get("id").and_then(Value::as_str).filter(|id| frame_text_known(id)).ok_or_else(|| self.refusal())?.to_string();
            if iframe {
                let parent = info.get("parentFrameId").and_then(Value::as_str).filter(|id| frame_text_known(id)).ok_or_else(|| self.refusal())?;
                if frame.get("parentId").is_some_and(|id| id.as_str() != Some(parent)) { return Err(self.refusal()) }
                frame["parentId"] = json!(parent);
                iframe_roots.push(root.clone());
                if self.protected_targets.contains(id) { protected_iframes.push(root.clone()); }
            } else {
                if frame.get("parentId").is_some() || roots.insert(root.to_string(), id.to_string()).is_some() { return Err(self.refusal()) }
            }
            target_roots.insert(id.to_string(), root);
            #[cfg(test)]
            self.runtime_prepare_probe("renderer-nodes", json!({}));
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
        #[cfg(test)]
        self.runtime_prepare_probe("iframe-ancestry", json!({}));
        for id in &iframe_roots { owner(id).ok_or_else(|| self.refusal())?; }
        for id in nodes.keys() {
            if let Some(page) = owner(id) { self.page_scope_owners.insert(id.clone(), page); }
        }
        for (target, root) in &target_roots {
            if let Some(page) = owner(root) { self.page_scope_owners.insert(target.clone(), page); }
        }
        for info in &infos {
            let id = info.get("targetId").and_then(Value::as_str).unwrap();
            if info.get("type").and_then(Value::as_str) == Some("page") { self.page_scope_owners.insert(id.to_string(), id.to_string()); }
        }
        let page = if selected.get("type").and_then(Value::as_str) == Some("page") { target.clone() } else { owner(target_roots.get(&target).map(String::as_str).unwrap_or(&target)).ok_or_else(|| self.refusal())? };
        #[cfg(test)]
        self.runtime_prepare_probe("protected-frame-placement", json!({"protected_present":protected_iframes.iter().any(|id| owner(id).as_deref() == Some(&page))}));
        if protected_iframes.iter().any(|id| owner(id).as_deref() == Some(&page)) { return Err(self.refusal()) }
        #[cfg(test)]
        self.runtime_prepare_probe("root-placement", json!({}));
        let root = roots.iter().find(|(_, owner)| *owner == &page).map(|(id, _)| id).ok_or_else(|| self.refusal())?;
        #[cfg(test)]
        self.runtime_prepare_probe("frame-tree-join", json!({}));
        let tree = Self::page_scope_join(root, &nodes, &mut BTreeSet::new()).ok_or_else(|| self.refusal())?;
        let mut frames = BTreeMap::new();
        let epoch = epochs.get(&page).copied().unwrap_or(0);
        #[cfg(test)]
        self.runtime_prepare_probe("scope-stability", json!({"page_epoch_stable":epoch == self.page_scope_epochs.get(&page).copied().unwrap_or(0),"frame_epoch_stable":unknown == self.page_scope_unknown}));
        if epoch != self.page_scope_epochs.get(&page).copied().unwrap_or(0) || unknown != self.page_scope_unknown {
            if refresh_main && target == page && session.and_then(|session| self.session_target(session)).as_deref() == Some(page.as_str())
                && unknown == self.page_scope_unknown && runtime_epochs.get(&page).copied().unwrap_or(0) == self.runtime_scope_epochs.get(&page).copied().unwrap_or(0)
                && guard.is_some() && guard.as_ref() == self.guards.get(&page) && !self.protected_targets.contains(&page)
                && session.is_some_and(|session| !self.blocked.contains(session)) {
                return self.page_scope_state(session, false, Some(deadline));
            }
            return Err(self.refusal());
        }
        if !self.collect_document_frames(&tree, &mut frames, true, None) {
            #[cfg(test)]
            self.runtime_prepare_frames(&tree, &frames);
            return Err(self.refusal());
        }
        #[cfg(test)]
        self.runtime_prepare_probe("frame-origin", json!({}));
        if frames.iter().any(|(id, frame)| {
                let ordinary_error = self.guard.error_document(&frame[1], &frame[2]) && !self.unverified_error(&frame[1], &frame[3])
                    && if frame[4].is_empty() { self.documents.get(&page).is_some_and(|doc| !doc.previous) } else { self.frame_decisions.get(id) == Some(&false) };
                !ordinary_error && (self.guard.url(&frame[1]) || self.guard.url(&frame[2]))
            }) { return Err(self.refusal()) }
        for (id, evidence) in &frames {
            self.frames.insert(id.clone(), vec![evidence[1].clone(), evidence[2].clone()]);
            self.note_parent(id, &evidence[4]);
        }
        let proof = (page.clone(), epoch, unknown);
        let state = DocumentState { frames, worker:None, scope:None, epoch:self.document_epochs.get(&page).copied().unwrap_or(0), page_scope:Some(proof.clone()), runtime_scope_epoch:None };
        if target == page && !self.document_paused(&page) {
            for (id, evidence) in &state.frames {
                self.frame_decisions.insert(id.clone(), false);
                if !evidence[0].is_empty() { self.document_versions.insert((page.clone(), id.clone()), evidence[0].clone()); }
            }
            let mut cached = state.clone();
            cached.page_scope = None;
            self.cached_documents.insert(page, cached);
        }
        #[cfg(test)]
        self.runtime_prepare_probe("page-proof-complete", json!({"paused":self.document_paused(&proof.0)}));
        Ok((proof, state))
    }

    fn page_scope_pending_page(&self, info: &Value) -> bool {
        let id = info.get("targetId").and_then(Value::as_str).unwrap_or("");
        info.get("type").and_then(Value::as_str) == Some("page") && info.get("url").and_then(Value::as_str) == Some("")
            && self.guards.contains_key(id) && !self.protected_targets.contains(id)
            && self.documents.get(id).is_some_and(|doc| !doc.previous && frame_text_known(&doc.url) && !self.guard.url(&doc.url))
            && !self.info_protected(info, &[])
    }

    fn page_scope_inventory(&mut self, selected: &str, selected_info: &Value, candidates: &[Value], deadline: Instant) -> Result<(Vec<Value>, BTreeMap<String, Value>), String> {
        let pages = candidates.iter().filter(|info| info.get("type").and_then(Value::as_str) == Some("page")
            && (selected_info.get("type").and_then(Value::as_str) == Some("iframe") || info.get("targetId").and_then(Value::as_str) == Some(selected)));
        let mut found = None;
        for page in pages {
            let id = page.get("targetId").and_then(Value::as_str).ok_or_else(|| self.refusal())?;
            #[cfg(test)]
            self.runtime_prepare_probe("inventory-page-admission", json!({
                "url_known":page.get("url").and_then(Value::as_str).is_some_and(frame_text_known),
                "pending_admitted":self.page_scope_pending_page(page),"info_protected":self.info_protected(page, &[]),
                "target_protected":self.protected_targets.contains(id),"guard_present":self.guards.contains_key(id),
                "document_known":self.documents.contains_key(id),"previous_protected":self.documents.get(id).is_some_and(|doc| doc.previous)}));
            if (page.get("url").and_then(Value::as_str).is_none_or(|url| !frame_text_known(url)) && !self.page_scope_pending_page(page)) || self.info_protected(page, &[]) || self.protected_targets.contains(id) { continue }
            let tree = self.page_scope_tree(id, deadline)?;
            let root = tree.get("frameTree").ok_or_else(|| self.refusal())?;
            #[cfg(test)]
            self.runtime_prepare_probe("inventory-root-placement", json!({"parent_present":root.pointer("/frame/parentId").is_some()}));
            if root.pointer("/frame/parentId").is_some() { return Err(self.refusal()) }
            let mut nodes = BTreeMap::new();
            #[cfg(test)]
            self.runtime_prepare_probe("inventory-nodes", json!({}));
            Self::page_scope_nodes(root, None, &mut nodes).ok_or_else(|| self.refusal())?;
            let mut infos = vec![page.clone()];
            let mut trees = BTreeMap::from([(id.to_string(), tree)]);
            let mut pending = candidates.iter().filter(|info| info.get("type").and_then(Value::as_str) == Some("iframe")).cloned().collect::<Vec<_>>();
            loop {
                let from = infos.len();
                let mut remaining = Vec::new();
                for info in pending {
                    let target = info.get("targetId").and_then(Value::as_str).ok_or_else(|| self.refusal())?;
                    let parent = info.get("parentFrameId").and_then(Value::as_str);
                    if parent.is_none_or(|parent| !nodes.contains_key(parent)) { remaining.push(info); continue }
                    let parent = parent.unwrap();
                    if info.get("url").and_then(Value::as_str).is_none_or(|url| !frame_text_known(url)) || self.info_protected(&info, &[]) || self.protected_targets.contains(target) {
                        nodes.entry(target.to_string()).or_insert_with(|| json!({"id":target,"parentId":parent}));
                    } else {
                        let mut tree = self.page_scope_tree(target, deadline)?;
                        let frame = tree.pointer_mut("/frameTree/frame").ok_or_else(|| self.refusal())?;
                        if frame.get("parentId").is_some_and(|id| id.as_str() != Some(parent)) { return Err(self.refusal()) }
                        frame["parentId"] = json!(parent);
                        Self::page_scope_nodes(&tree["frameTree"], Some(parent), &mut nodes).ok_or_else(|| self.refusal())?;
                        trees.insert(target.to_string(), tree);
                    }
                    infos.push(info);
                    if infos.len() > MAX_TARGETS { return Err(self.refusal()) }
                }
                pending = remaining;
                if infos.len() == from { break }
            }
            #[cfg(test)]
            self.runtime_prepare_probe("inventory-unplaced-frames", json!({"unplaced_owned":pending.iter().any(|info| info.get("targetId").and_then(Value::as_str).is_some_and(|target| nodes.contains_key(target) || self.page_scope_owner(target).as_deref() == Some(id)))}));
            if pending.iter().any(|info| info.get("targetId").and_then(Value::as_str).is_some_and(|target| nodes.contains_key(target) || self.page_scope_owner(target).as_deref() == Some(id))) { return Err(self.refusal()) }
            if id == selected || infos.iter().any(|info| info.get("targetId").and_then(Value::as_str) == Some(selected)) {
                if found.is_some() { return Err(self.refusal()) }
                found = Some((infos, trees));
            }
        }
        #[cfg(test)]
        self.runtime_prepare_probe("inventory-complete", json!({"selected_found":found.is_some()}));
        found.ok_or_else(|| self.refusal())
    }

    fn page_scope_tree(&mut self, target: &str, deadline: Instant) -> Result<Value, String> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() { return Err(self.refusal()) }
        #[cfg(test)]
        self.runtime_prepare_probe("renderer-proof-session", json!({"guard_present":self.guards.contains_key(target),"metadata_session_present":self.page_scope_sessions.contains_key(target)}));
        let session = match self.guards.get(target).or_else(|| self.page_scope_sessions.get(target)).cloned() {
            Some(session) => session,
            None => {
                self.guarding.insert(target.to_string());
                let attached = self.raw_call("Target.attachToTarget", json!({"targetId":target,"flatten":true}), None, left);
                self.guarding.remove(target);
                let result = attached.map_err(|_| self.refusal())?;
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
        #[cfg(test)]
        self.runtime_prepare_probe("renderer-frame-read", json!({"budget_remaining":!left.is_zero()}));
        let mut tree = self.raw_call("Page.getFrameTree", json!({}), Some(&session), left).map_err(|_| self.refusal())?;
        self.verify_document_order(&tree, Some(&session))?;
        #[cfg(test)]
        self.runtime_prepare_probe("renderer-frame-versions", json!({}));
        if self.page_scope_versions(target, &tree)? {
            #[cfg(test)]
            self.runtime_prepare_probe("inherited-origin-replay", json!({}));
            self.context_proofs.retain(|(owner, _), _| owner != &session);
            for method in ["Runtime.disable", "Runtime.enable"] {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() { return Err(self.refusal()) }
                if self.raw_call(method, json!({}), Some(&session), left).is_err() {
                    if self.protected_targets.contains(target) { return Ok(tree) }
                    return Err(self.refusal());
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() { return Err(self.refusal()) }
            tree = self.raw_call("Page.getFrameTree", json!({}), Some(&session), left).map_err(|_| self.refusal())?;
            self.verify_document_order(&tree, Some(&session))?;
        }
        Ok(tree)
    }

    fn page_scope_nodes(node: &Value, parent: Option<&str>, nodes: &mut BTreeMap<String, Value>) -> Option<()> {
        let frame = node.get("frame")?;
        let id = frame.get("id")?.as_str().filter(|id| frame_text_known(id))?;
        if nodes.len() >= MAX_FRAMES || parent.is_some_and(|parent| frame.get("parentId").is_some_and(|id| id.as_str() != Some(parent))) { return None }
        let mut frame = frame.clone();
        if let Some(parent) = parent { frame["parentId"] = json!(parent); }
        if let Some(previous) = nodes.get(id) {
            if previous.get("parentId") != frame.get("parentId") { return None }
        }
        nodes.insert(id.to_string(), frame);
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

    fn page_scope_versions(&mut self, target: &str, tree: &Value) -> Result<bool, String> {
        let root = tree.get("frameTree").ok_or_else(|| self.refusal())?;
        let mut queue = vec![(root, false)];
        let mut inherited = false;
        let mut count = 0;
        while let Some((node, child)) = queue.pop() {
            count += 1;
            if count > MAX_FRAMES { return Err(self.refusal()) }
            let frame = node.get("frame").ok_or_else(|| self.refusal())?;
            let id = frame.get("id").and_then(Value::as_str).ok_or_else(|| self.refusal())?;
            if let Some(loader) = frame.get("loaderId").and_then(Value::as_str) {
                self.document_versions.insert((target.to_string(), id.to_string()), loader.to_string());
            }
            inherited |= (child || frame.get("parentId").is_some()) && frame.get("securityOrigin").and_then(Value::as_str) == Some("://")
                && self.guard.inheriting(frame.get("url").and_then(Value::as_str).unwrap_or("")) && self.page_scope_origin(frame).is_none();
            if let Some(children) = node.get("childFrames") {
                for child in children.as_array().ok_or_else(|| self.refusal())? { queue.push((child, true)); }
            }
        }
        Ok(inherited)
    }

    pub(super) fn page_scope_origin<'a>(&'a self, frame: &Value) -> Option<&'a str> {
        if frame.get("securityOrigin").and_then(Value::as_str) != Some("://")
            || !self.guard.inheriting(frame.get("url").and_then(Value::as_str).unwrap_or("")) { return None }
        let id = frame.get("id")?.as_str()?;
        let loader = frame.get("loaderId")?.as_str()?;
        self.context_proofs.iter().find(|((session, _), proof)| {
            self.guard_sessions.contains(session) && proof.default && proof.frame == id && proof.loader.as_deref() == Some(loader)
                && frame_text_known(&proof.origin)
        }).map(|(_, proof)| proof.origin.as_str())
    }

    pub(super) fn page_scope_owner(&self, id: &str) -> Option<String> {
        let mut current = id;
        let mut visited = BTreeSet::new();
        while visited.insert(current.to_string()) && visited.len() <= MAX_FRAMES {
            if let Some(page) = self.page_scope_owners.get(current) { return Some(page.clone()) }
            if let Some(info) = self.targets.get(current) {
                if info.get("type").and_then(Value::as_str) == Some("page") { return Some(current.to_string()) }
                if let Some(parent) = info.get("parentFrameId").and_then(Value::as_str) { current = parent; continue; }
            }
            current = self.frame_parents.get(current)?;
        }
        None
    }

    pub(super) fn page_scope_changed(&mut self, method: &str, params: &Value, session: Option<&str>) {
        let mut pages = BTreeSet::new();
        if matches!(method, "Page.frameNavigated" | "Page.frameAttached" | "Page.frameDetached" | "Page.navigatedWithinDocument") {
            if method == "Page.frameNavigated" {
                let frame = &params["frame"];
                let id = frame.get("id").and_then(Value::as_str).unwrap_or("");
                if self.frames.get(id).is_some_and(|known| known.first() == frame.get("url").and_then(Value::as_str).map(String::from).as_ref()
                    && known.get(1) == frame.get("securityOrigin").and_then(Value::as_str).map(String::from).as_ref())
                    && session.and_then(|session| self.session_target(session)).is_some_and(|target| self.document_versions.get(&(target, id.to_string())).map(String::as_str) == frame.get("loaderId").and_then(Value::as_str)) { return }
            }
            if let Some(page) = session.and_then(|session| self.session_target(session)).and_then(|target| self.page_scope_owner(&target)) { pages.insert(page); }
        } else if matches!(method, "Target.targetCreated" | "Target.targetInfoChanged" | "Target.targetDestroyed") {
            let info = params.get("targetInfo");
            let id = info.and_then(|info| info.get("targetId")).or_else(|| params.get("targetId")).and_then(Value::as_str).unwrap_or("");
            let previous = self.targets.get(id);
            if info.or(previous).and_then(|info| info.get("type")).and_then(Value::as_str) != Some("iframe") { return }
            if info.is_some_and(|info| previous.is_some_and(|previous| ["url", "parentFrameId"].iter().all(|key| previous.get(*key) == info.get(*key)))) { return }
            if let Some(page) = self.page_scope_owner(id) { pages.insert(page); }
            if let Some(page) = info.and_then(|info| info.get("parentFrameId")).and_then(Value::as_str).and_then(|parent| self.page_scope_owner(parent)) { pages.insert(page); }
            if pages.is_empty() && method == "Target.targetInfoChanged" && info.is_some_and(|info| previous.is_some_and(|previous| {
                info.get("parentFrameId").and_then(Value::as_str).is_some_and(frame_text_known) && previous.get("parentFrameId") == info.get("parentFrameId")
            })) { return }
        } else { return }
        if pages.is_empty() { self.page_scope_unknown += 1; }
        for page in pages {
            let frame = params.pointer("/frame/id").or_else(|| params.get("frameId")).and_then(Value::as_str);
            let main_navigation = matches!(method, "Page.frameNavigated" | "Page.navigatedWithinDocument") && self.documents.get(&page).is_some_and(|doc| frame == Some(doc.frame_id.as_str()));
            if !main_navigation { *self.runtime_scope_epochs.entry(page.clone()).or_default() += 1; }
            *self.page_scope_epochs.entry(page).or_default() += 1;
        }
    }
}
