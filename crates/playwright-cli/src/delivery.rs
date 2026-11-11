use crate::browser;
use crate::cdp::Cdp;
use crate::landing::{self, Scope};
use crate::snapshot::FrameRec;
use serde_json::{json, Value};

const FRAME_OWNERS: &[&str] = &["iframe", "frame", "object", "embed", "fencedframe", "portal"];
const MAX_DEPTH: usize = 8;
const INPUT_GROUP: &str = "slicc-input";

pub struct Delivery {
    pub session: String,
    pub frame: String,
    pub input: String,
    pub offset: (f64, f64),
    pub untrusted: bool,
    detach: Vec<String>,
}

impl Delivery {
    pub fn scope(&self) -> Scope<'_> {
        Scope {
            session: &self.session,
            frame: Some(&self.frame),
            input: &self.input,
        }
    }

    pub fn local(&self, (x, y): (f64, f64)) -> (f64, f64) {
        (x - self.offset.0, y - self.offset.1)
    }

    pub fn shift_motion(&self, motion: &landing::Motion) -> landing::Motion {
        let map = |point: (f64, f64)| self.local(point);
        landing::Motion {
            steps: motion
                .steps
                .iter()
                .map(|step| landing::Step {
                    kind: step.kind,
                    point: map(step.point),
                    buttons: step.buttons,
                    button: step.button,
                })
                .collect(),
            from: motion.from.map(map),
            press: motion.press.map(map),
            modifiers: motion.modifiers,
        }
    }
}

pub fn release(cdp: &mut Cdp, delivery: Delivery) {
    detach_all(cdp, delivery.detach);
}

fn detach_all(cdp: &mut Cdp, detach: Vec<String>) {
    for session in detach.into_iter().rev() {
        crate::cdp::detach(cdp, &session);
    }
}

fn finish(cdp: &mut Cdp, detach: Vec<String>, result: Result<Delivery, String>) -> Result<Delivery, String> {
    match result {
        Ok(mut delivery) => {
            delivery.detach = detach;
            Ok(delivery)
        }
        Err(message) => {
            detach_all(cdp, detach);
            Err(message)
        }
    }
}

pub fn at_point(cdp: &mut Cdp, page: &str, x: f64, y: f64) -> Result<Delivery, String> {
    let refused = || landing::frame_refusal(x, y);
    let mut detach = Vec::new();
    let result = walk(cdp, page, x, y, &mut detach, refused);
    finish(cdp, detach, result)
}

pub fn at_focus(cdp: &mut Cdp, page: &str) -> Result<Delivery, String> {
    let refused = || landing::FOCUS_REFUSAL.to_string();
    struct Group<'a> {
        cdp: &'a mut Cdp,
        page: &'a str,
    }
    impl Drop for Group<'_> {
        fn drop(&mut self) {
            let _ = self.cdp.call(
                "Runtime.releaseObjectGroup",
                json!({ "objectGroup": INPUT_GROUP }),
                Some(self.page),
            );
        }
    }
    let group = Group { cdp, page };
    let cdp = &mut *group.cdp;
    let mut detach = Vec::new();
    let result = focus_walk(cdp, page, &mut detach, refused);
    finish(cdp, detach, result)
}

fn focus_walk(
    cdp: &mut Cdp,
    page: &str,
    detach: &mut Vec<String>,
    refused: impl Fn() -> String + Copy,
) -> Result<Delivery, String> {
    let mut session = page.to_string();
    let mut offset = (0.0, 0.0);
    let mut frame = main_frame(cdp, &session)?;
    for _ in 0..MAX_DEPTH {
        let world = cdp.call("Page.createIsolatedWorld", json!({ "frameId": frame }), Some(&session))?;
        let context = world.get("executionContextId").cloned().ok_or_else(refused)?;
        let focused = cdp.call(
            "Runtime.evaluate",
            json!({
                "expression": "(() => { let e = document.activeElement; while (e && e.shadowRoot && e.shadowRoot.activeElement) e = e.shadowRoot.activeElement; return e; })()",
                "contextId": context,
                "returnByValue": false,
                "objectGroup": INPUT_GROUP,
            }),
            Some(&session),
        )?;
        if focused.get("exceptionDetails").is_some() {
            return Err(refused());
        }
        let remote = focused.get("result").cloned().unwrap_or(Value::Null);
        if remote.get("subtype").and_then(Value::as_str) == Some("null") {
            return Ok(Delivery {
                session,
                frame,
                input: page.to_string(),
                offset,
                untrusted: false,
                detach: Vec::new(),
            });
        }
        let object = remote
            .get("objectId")
            .cloned()
            .filter(|_| remote.get("subtype").and_then(Value::as_str) == Some("node"))
            .ok_or_else(refused)?;
        let node = cdp.call("DOM.describeNode", json!({ "objectId": object }), Some(&session))?;
        let node = node.get("node").ok_or_else(refused)?;
        let closed = node.get("shadowRoots").and_then(Value::as_array).is_some_and(|roots| {
            roots.iter().any(|root| {
                root.get("shadowRootType")
                    .and_then(Value::as_str)
                    .is_none_or(|kind| !matches!(kind, "open" | "user-agent"))
            })
        });
        if closed {
            return Err(refused());
        }
        if !frame_owner(node) {
            return Ok(Delivery {
                session,
                frame,
                input: page.to_string(),
                offset,
                untrusted: false,
                detach: Vec::new(),
            });
        }
        let backend = node.get("backendNodeId").and_then(Value::as_u64).ok_or_else(refused)?;
        if open_owner(cdp, page, &mut session, &mut offset, detach, backend, node, &refused)? {
            frame = main_frame(cdp, &session)?;
            continue;
        }
        let content = content_frame(node)
            .map(str::to_string)
            .or_else(|| owner_frame(cdp, &session, backend).ok())
            .ok_or_else(refused)?;
        let found = locate(cdp, page, &session, &content)?;
        admit_frame(cdp, &found, &refused)?;
        frame = content;
    }
    Err(refused())
}

fn walk(
    cdp: &mut Cdp,
    page: &str,
    x: f64,
    y: f64,
    detach: &mut Vec<String>,
    refused: impl Fn() -> String + Copy,
) -> Result<Delivery, String> {
    let mut session = page.to_string();
    let mut offset = (0.0, 0.0);
    for _ in 0..MAX_DEPTH {
        let main = main_frame(cdp, &session)?;
        let (lx, ly) = (x - offset.0, y - offset.1);
        let hit = match node_at(cdp, &session, lx, ly)? {
            Some(hit) => hit,
            None => {
                return Ok(Delivery {
                    session,
                    frame: main,
                    input: page.to_string(),
                    offset,
                    untrusted: landing::untrusted(cdp, page)?,
                    detach: Vec::new(),
                });
            }
        };
        if hit.frame != main {
            let frame = locate(cdp, page, &session, &hit.frame).map_err(|_| refused())?;
            admit_frame(cdp, &frame, &refused)?;
            if frame.target.is_some() {
                let nested = browser::frame_offset(cdp, &session, &hit.frame).map_err(|_| refused())?;
                offset = (offset.0 + nested.0, offset.1 + nested.1);
                let (child, _) = browser::open_frame(cdp, &frame).map_err(|_| refused())?;
                detach.push(child.clone());
                session = child;
                continue;
            }
            let nested = browser::frame_offset(cdp, &session, &hit.frame).map_err(|_| refused())?;
            return Ok(Delivery {
                session,
                frame: hit.frame,
                input: page.to_string(),
                offset: (offset.0 + nested.0, offset.1 + nested.1),
                untrusted: false,
                detach: Vec::new(),
            });
        }
        let node = describe_backend(cdp, &session, hit.backend)?;
        if !frame_owner(&node) {
            return Ok(Delivery {
                session,
                frame: main,
                input: page.to_string(),
                offset,
                untrusted: false,
                detach: Vec::new(),
            });
        }
        if open_owner(cdp, page, &mut session, &mut offset, detach, hit.backend, &node, &refused)? {
            continue;
        }
        let content = content_frame(&node)
            .map(str::to_string)
            .or_else(|| owner_frame(cdp, &session, hit.backend).ok())
            .ok_or_else(refused)?;
        let frame = locate(cdp, page, &session, &content)?;
        admit_frame(cdp, &frame, &refused)?;
        return Ok(Delivery {
            session,
            frame: content,
            input: page.to_string(),
            offset,
            untrusted: false,
            detach: Vec::new(),
        });
    }
    Err(refused())
}

fn open_owner(
    cdp: &mut Cdp,
    page: &str,
    session: &mut String,
    offset: &mut (f64, f64),
    detach: &mut Vec<String>,
    backend: u64,
    node: &Value,
    refused: &impl Fn() -> String,
) -> Result<bool, String> {
    let content = content_frame(node)
        .map(str::to_string)
        .or_else(|| owner_frame(cdp, session, backend).ok())
        .ok_or_else(refused)?;
    let frame = locate(cdp, page, session, &content).map_err(|_| refused())?;
    let nested = browser::owner_point(cdp, session, backend).map_err(|_| refused())?;
    *offset = (offset.0 + nested.0, offset.1 + nested.1);
    if frame.target.is_none() {
        return Ok(false);
    }
    admit_frame(cdp, &frame, refused)?;
    let (child, _) = browser::open_frame(cdp, &frame).map_err(|_| refused())?;
    detach.push(child.clone());
    *session = child;
    Ok(true)
}

fn admit_frame(cdp: &mut Cdp, frame: &FrameRec, refused: &impl Fn() -> String) -> Result<(), String> {
    if let Some(target) = frame.target.as_deref() {
        if target == frame.frame_id
            && browser::ordinary_frame_url(cdp.guard(), &frame.url)
            && !cdp.is_protected_target(target)
        {
            Ok(())
        } else {
            Err(refused())
        }
    } else if browser::ordinary_frame_url(cdp.guard(), &frame.url) {
        Ok(())
    } else {
        Err(refused())
    }
}

fn locate(cdp: &mut Cdp, page: &str, session: &str, frame_id: &str) -> Result<FrameRec, String> {
    let mut frames = browser::page_frames(cdp, page)?;
    if session != page {
        frames.extend(browser::frame_tree(cdp, session)?);
    }
    frames
        .into_iter()
        .find(|frame| frame.frame_id == frame_id)
        .ok_or_else(|| cdp.unverified())
}

fn owner_frame(cdp: &mut Cdp, session: &str, backend: u64) -> Result<String, String> {
    let frames = browser::frame_tree(cdp, session)?;
    for frame in frames.into_iter().filter(|frame| frame.parent_frame_id.is_some()) {
        let owner = cdp.call("DOM.getFrameOwner", json!({ "frameId": frame.frame_id }), Some(session));
        if owner.ok().and_then(|value| value.get("backendNodeId").and_then(Value::as_u64)) == Some(backend) {
            return Ok(frame.frame_id);
        }
    }
    Err(cdp.unverified())
}

fn content_frame(node: &Value) -> Option<&str> {
    node.get("frameId").and_then(Value::as_str).filter(|id| !id.is_empty())
}

fn frame_owner(node: &Value) -> bool {
    let name = node
        .get("localName")
        .or_else(|| node.get("nodeName"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    FRAME_OWNERS.contains(&name.as_str())
}

fn describe_backend(cdp: &mut Cdp, session: &str, backend: u64) -> Result<Value, String> {
    let node = cdp.call("DOM.describeNode", json!({ "backendNodeId": backend }), Some(session))?;
    node.get("node").cloned().ok_or_else(|| cdp.unverified())
}

struct Hit {
    backend: u64,
    frame: String,
}

fn node_at(cdp: &mut Cdp, session: &str, x: f64, y: f64) -> Result<Option<Hit>, String> {
    let metrics = cdp.call("Page.getLayoutMetrics", json!({}), Some(session))?;
    let scroll = |key: &str| metrics.pointer(&format!("/cssLayoutViewport/{key}")).and_then(Value::as_f64).filter(|value| value.is_finite());
    let (Some(left), Some(top)) = (scroll("pageX"), scroll("pageY")) else {
        return Err(landing::frame_refusal(x, y));
    };
    let mut points: Vec<(i64, i64)> = Vec::new();
    for px in [(x + left).floor(), (x + left).ceil()] {
        for py in [(y + top).floor(), (y + top).ceil()] {
            if px.abs() <= i32::MAX as f64 && py.abs() <= i32::MAX as f64 && !points.contains(&(px as i64, py as i64)) {
                points.push((px as i64, py as i64));
            }
        }
    }
    let mut agreed: Option<Hit> = None;
    for (px, py) in points {
        let hit = match cdp.call(
            "DOM.getNodeForLocation",
            json!({ "x": px, "y": py, "includeUserAgentShadowDOM": false, "ignorePointerEventsNone": false }),
            Some(session),
        ) {
            Ok(hit) => hit,
            Err(message) if message.contains("No node found at given location") && !message.contains("slicc:") => continue,
            Err(message) if message.ends_with("wasn't found\n") && landing::untrusted(cdp, session)? => return Ok(None),
            Err(message) => return Err(message),
        };
        let backend = hit.get("backendNodeId").and_then(Value::as_u64).ok_or_else(|| landing::frame_refusal(x, y))?;
        let frame = hit
            .get("frameId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| landing::frame_refusal(x, y))?
            .to_string();
        let next = Hit { backend, frame };
        if let Some(prior) = &agreed {
            if prior.frame != next.frame || prior.backend != next.backend {
                return Err(landing::frame_refusal(x, y));
            }
        } else {
            agreed = Some(next);
        }
    }
    Ok(agreed)
}

fn main_frame(cdp: &mut Cdp, session: &str) -> Result<String, String> {
    let tree = cdp.call("Page.getFrameTree", json!({}), Some(session))?;
    tree.pointer("/frameTree/frame/id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or_else(|| cdp.unverified())
}

#[cfg(test)]
mod tests {
    use super::{content_frame, frame_owner};
    use serde_json::json;

    #[test]
    fn frame_owners_match_local_names() {
        assert!(frame_owner(&json!({ "localName": "iframe" })));
        assert!(frame_owner(&json!({ "nodeName": "FENCEDFRAME" })));
        assert!(!frame_owner(&json!({ "localName": "div" })));
    }

    #[test]
    fn content_frame_reads_node_frame_id() {
        assert_eq!(content_frame(&json!({ "frameId": "CHILD" })), Some("CHILD"));
        assert_eq!(content_frame(&json!({ "frameId": "" })), None);
        assert_eq!(content_frame(&json!({})), None);
    }
}
