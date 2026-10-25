use super::*;

pub(super) struct ObservedDocument {
    pub(super) frame: Value,
    pub(super) epoch: u64,
    pub(super) root: Option<(String, String)>,
}

fn ordinary_document(guard: &Protected, frame: &Value) -> bool {
    ["id", "loaderId"].iter().all(|key| frame.get(*key).and_then(Value::as_str).is_some_and(frame_text_known))
        && ["url", "securityOrigin"].iter().all(|key| frame.get(*key).and_then(Value::as_str).is_some_and(|text| frame_text_known(text) && !guard.url(text)))
        && !guard.error_page_url(frame.get("url").and_then(Value::as_str).unwrap_or(""))
        && frame.get("unreachableUrl").and_then(Value::as_str).is_none_or(|url| url.is_empty())
}

fn document_matches(guard: &Protected, observed: &Value, fresh: &Value) -> bool {
    let text = |frame: &Value, key: &str| frame.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    if !["id", "parentId", "loaderId", "securityOrigin", "unreachableUrl"].iter().all(|key| text(observed, key) == text(fresh, key)) { return false }
    let origin = |frame: &Value| url::Url::parse(&text(frame, "url")).ok().filter(|url| matches!(url.scheme(), "http" | "https")).map(|url| url.origin());
    for frame in [observed, fresh] {
        if ordinary_document(guard, frame) && origin(frame).is_some_and(|origin| Some(origin) != url::Url::parse(&text(frame, "securityOrigin")).ok().map(|url| url.origin())) { return false }
    }
    if ["url", "urlFragment"].iter().all(|key| text(observed, key) == text(fresh, key)) { return true }
    if !ordinary_document(guard, observed) || !ordinary_document(guard, fresh) { return false }
    let old = origin(observed);
    old.is_some() && old == origin(fresh) && old == url::Url::parse(&text(fresh, "securityOrigin")).ok().map(|url| url.origin())
}

impl Cdp {
    pub(super) fn observe_document_order(&mut self, method: &str, params: &Value, session: Option<&str>) {
        if method == "Page.navigatedWithinDocument" {
            let Some(target) = session.and_then(|session| self.session_target(session)) else { return };
            let id = params.get("frameId").and_then(Value::as_str).unwrap_or("");
            if id.is_empty() { return }
            let url = params.get("url").and_then(Value::as_str).unwrap_or("");
            let (url, fragment) = url.split_once('#').map_or((url, String::new()), |(url, fragment)| (url, format!("#{fragment}")));
            for ((owner, _, frame), observed) in &mut self.unchecked_documents {
                if owner == &target && frame == id {
                    observed.frame["url"] = json!(url);
                    observed.frame["urlFragment"] = json!(fragment);
                }
            }
            return;
        }
        if method != "Page.frameNavigated" { return }
        let Some(session) = session else { return };
        let Some(target) = self.session_target(session) else { return };
        if self.unobserved.contains(&target) || !self.guards.contains_key(&target) { return }
        let frame = params.get("frame").cloned().unwrap_or(Value::Null);
        let id = frame.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        let root = self.documents.get(&target).map(|doc| doc.frame_id.clone()).or_else(|| frame.get("parentId").is_none().then(|| id.clone())).and_then(|root| {
            let loader = if root == id { frame.get("loaderId").and_then(Value::as_str) }
                else { self.document_versions.get(&(target.clone(), root.clone())).map(String::as_str) };
            Some((root, loader.filter(|loader| frame_text_known(loader))?.to_string()))
        });
        self.unchecked_documents.insert((target, session.to_string(), id), ObservedDocument { frame, epoch:self.frame_epoch, root });
    }

    pub(super) fn note_frame_removal(&mut self, target: &str, id: &str) {
        let Some(root) = self.documents.get(target).map(|doc| doc.frame_id.clone()) else { return };
        let Some(loader) = self.document_versions.get(&(target.to_string(), root.clone())).filter(|loader| frame_text_known(loader)).cloned() else { return };
        self.removed_frames.insert((target.to_string(), id.to_string()), (root, loader));
    }

    pub(super) fn forget_frame_order(&mut self, target: &str, session: &str, id: &str, retired: bool) {
        let guard = &self.guard;
        self.unchecked_documents.retain(|(owner, peer, frame), observed| {
            owner != target || frame != id || peer != session && (!retired
                || ["url", "securityOrigin"].iter().any(|key| observed.frame.get(*key).and_then(Value::as_str).is_none_or(|text| !frame_text_known(text) || guard.url(text)))
                || guard.error_page_url(observed.frame.get("url").and_then(Value::as_str).unwrap_or(""))
                || observed.frame.get("unreachableUrl").and_then(Value::as_str).is_some_and(|url| !url.is_empty()))
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
            let mut matched = BTreeMap::<String, u64>::new();
            for ((owner, peer, id), observed) in &self.unchecked_documents {
                if owner == &target && !self.guard_sessions.contains(peer) && frames.get(id.as_str()).is_some_and(|fresh| ordinary_document(&self.guard, fresh) && document_matches(&self.guard, &observed.frame, fresh)) {
                    matched.entry(id.clone()).and_modify(|epoch| *epoch = (*epoch).max(observed.epoch)).or_insert(observed.epoch);
                }
            }
            let guard = &self.guard;
            let sessions = &self.guard_sessions;
            let main = &tree["frameTree"]["frame"];
            let main_id = main.get("id").and_then(Value::as_str).unwrap_or("");
            let main_loader = main.get("loaderId").and_then(Value::as_str).unwrap_or("");
            let main_epoch = ordinary_document(guard, main).then(|| matched.get(main_id).copied()).flatten();
            let removed = &self.removed_frames;
            self.unchecked_documents.retain(|(owner, peer, id), observed| {
                if owner == &target && !frames.contains_key(id.as_str()) && ordinary_document(guard, main) && ordinary_document(guard, &observed.frame)
                    && removed.get(&(owner.clone(), id.clone())).is_some_and(|root| observed.root.as_ref() == Some(root) && root.0 == main_id && root.1 == main_loader) { return false }
                if owner == &target && !frames.contains_key(id.as_str()) && ordinary_document(guard, &observed.frame)
                    && main_epoch.is_some_and(|epoch| epoch > observed.epoch)
                    && observed.root.as_ref().is_some_and(|(root, loader)| root == main_id && loader != main_loader) { return false }
                owner != &target || frames.get(id.as_str()).is_none_or(|fresh| {
                    (!document_matches(guard, &observed.frame, fresh) && !(sessions.contains(peer) && ordinary_document(guard, &observed.frame)
                        && matched.get(id).is_some_and(|epoch| *epoch > observed.epoch)))
                        || guard.error_page_url(fresh.get("url").and_then(Value::as_str).unwrap_or(""))
                        || fresh.get("unreachableUrl").and_then(Value::as_str).is_some_and(|url| !url.is_empty())
                })
            });
        }
        let mut verified = self.guard_sessions.contains(session);
        for ((owner, _, id), observed) in &self.unchecked_documents {
            if owner != &target { continue }
            let Some(fresh) = frames.get(id.as_str()) else {
                #[cfg(test)]
                {
                    self.runtime_probe.push(json!({"stage":"document-frame-missing","ordinary":ordinary_document(&self.guard, &observed.frame),"root_known":observed.root.is_some()}));
                    if std::env::var_os("CI").is_some() { eprintln!("document verification missing frame: ordinary={} root_known={}", ordinary_document(&self.guard, &observed.frame), observed.root.is_some()); }
                }
                return Err(self.refusal());
            };
            if !document_matches(&self.guard, &observed.frame, fresh) {
                #[cfg(test)]
                {
                    let fields = ["id","parentId","loaderId","url","urlFragment","securityOrigin","unreachableUrl"].iter().filter(|key| observed.frame.get(**key).and_then(Value::as_str).unwrap_or("") != fresh.get(**key).and_then(Value::as_str).unwrap_or("")).collect::<Vec<_>>();
                    self.runtime_probe.push(json!({"stage":"document-order-mismatch","guard":self.guard_sessions.contains(session),"fields":fields}));
                    if std::env::var_os("CI").is_some() { eprintln!("document verification mismatch: fields={fields:?}"); }
                }
                return Err(self.refusal());
            }
            let text = |key| fresh.get(key).and_then(Value::as_str).unwrap_or("");
            verified &= !self.unverified_error(text("url"), text("unreachableUrl"));
        }
        if verified { self.unchecked_documents.retain(|(owner, _, _), _| owner != &target); }
        Ok(())
    }
}
