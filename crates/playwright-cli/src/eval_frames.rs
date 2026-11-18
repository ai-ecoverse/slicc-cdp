use crate::args::Invocation;
use crate::cdp::Cdp;
use serde_json::{json, Value};

pub(crate) const UNAVAILABLE: &str = "cross-origin child tools are unavailable";

pub(crate) fn origins(invocation: &Invocation) -> Result<(bool, Vec<String>), String> {
    let values = invocation.repeated("frames");
    let same = values.contains(&"same-origin");
    let explicit: Vec<_> = values.into_iter().filter(|value| *value != "same-origin").collect();
    let origins = crate::webmcp_frames::origins(&explicit).map_err(|message| message.replace("--from-origin", "--frames"))?;
    if origins.iter().any(|origin| url::Url::parse(origin).is_ok_and(|url| url.host_str().is_some_and(|host| host.contains('*')))) {
        return Err("--frames requires an http(s) origin without wildcards\n".into());
    }
    Ok((same, origins))
}

#[derive(Clone)]
pub(crate) struct Document {
    pub(crate) target: String,
    pub(crate) session: String,
    pub(crate) frame: Value,
}

impl Document {
    pub(crate) fn text(&self, key: &str) -> &str { self.frame[key].as_str().unwrap_or("") }

    pub(crate) fn verify(&self, cdp: &mut Cdp) -> Result<(), String> {
        let current = if self.frame.get("parentId").is_none() {
            cdp.call("Page.getFrameTree", json!({}), Some(&self.session))?["frameTree"]["frame"].clone()
        } else { cdp.eval_attach_document(&self.session, self.text("id"))? };
        if ["id", "loaderId", "url", "securityOrigin", "parentId", "unreachableUrl"].iter().any(|key| current[*key] != self.frame[*key]) {
            return Err("eval-attach: document changed\n".into());
        }
        Ok(())
    }

    pub(crate) fn evidence(&self, cdp: &mut Cdp, parent: Option<&Document>) -> Value {
        let policy = cdp.events().iter().rev().find(|event| event["sessionId"] == self.session && event["method"] == "Network.responseReceived"
            && event["params"]["type"] == "Document" && event["params"]["frameId"] == self.text("id") && event["params"]["loaderId"] == self.text("loaderId"))
            .and_then(|event| event["params"]["response"]["headers"].as_object()).map(|headers| {
                headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case("permissions-policy")).filter_map(|(_, value)| value.as_str()).collect::<Vec<_>>().join(",")
            }).map(|policy| if policy.len() > 16384 { "tools=()".to_string() } else { policy });
        let native = cdp.call("Page.getPermissionsPolicyState", json!({"frameId":self.text("id")}), Some(&self.session)).ok()
            .and_then(|value| value["states"].as_array()?.iter().find(|state| state["feature"] == "tools")?["allowed"].as_bool());
        let owner = parent.and_then(|parent| owner(cdp, parent, self.text("id")).ok()).unwrap_or(Value::Null);
        json!({"frameId":self.text("id"),"loaderId":self.text("loaderId"),"origin":self.text("securityOrigin"),"permissionsPolicy":policy,"toolsAllowed":native,"allow":owner["allow"],"srcOrigin":owner["srcOrigin"]})
    }
}

pub(crate) fn verify(cdp: &mut Cdp, chain: &[Document]) -> Result<(), String> {
    for (index, document) in chain.iter().enumerate() {
        document.verify(cdp)?;
        if index > 0 && document.text("parentId") != chain[index - 1].text("id") { return Err("eval-attach: unverified ancestor\n".into()); }
    }
    Ok(())
}

fn owner(cdp: &mut Cdp, parent: &Document, child: &str) -> Result<Value, String> {
    parent.verify(cdp)?;
    let world = cdp.call("Page.createIsolatedWorld", json!({"frameId":parent.text("id"),"worldName":"__slicc_eval_frame_evidence"}), Some(&parent.session))?;
    let owner = match cdp.call("DOM.getFrameOwner", json!({"frameId":child}), Some(&parent.session)) {
        Ok(owner) => owner,
        Err(_) => {
            let document = cdp.call("DOM.getDocument", json!({"depth":1,"pierce":false}), Some(&parent.session))?;
            let nodes = cdp.call("DOM.querySelectorAll", json!({"nodeId":document["root"]["nodeId"],"selector":"iframe,frame"}), Some(&parent.session))?;
            let ids = nodes["nodeIds"].as_array().filter(|ids| ids.len() <= 256).ok_or("eval-attach: unverified frame owner\n")?;
            let mut backend = None;
            for id in ids {
                let node = cdp.call("DOM.describeNode", json!({"nodeId":id,"depth":0,"pierce":false}), Some(&parent.session))?;
                if node["node"]["frameId"] == child {
                    if backend.is_some() { return Err("eval-attach: ambiguous frame owner\n".into()); }
                    backend = node["node"]["backendNodeId"].as_i64();
                }
            }
            json!({"backendNodeId":backend.ok_or("eval-attach: unverified frame owner\n")?})
        }
    };
    let object = cdp.call("DOM.resolveNode", json!({"backendNodeId":owner["backendNodeId"],"executionContextId":world["executionContextId"],"objectGroup":"slicc-eval-frame-evidence"}), Some(&parent.session))?;
    let result = cdp.call("Runtime.callFunctionOn", json!({"objectId":object["object"]["objectId"],"functionDeclaration":"function() { const allow = this.getAttribute('allow') || ''; return {allow:allow.length <= 16384 ? allow : null,srcOrigin:new URL(this.src,document.baseURI).origin}; }","returnByValue":true}), Some(&parent.session));
    let _ = cdp.call("Runtime.releaseObjectGroup", json!({"objectGroup":"slicc-eval-frame-evidence"}), Some(&parent.session));
    parent.verify(cdp)?;
    let result = result?;
    if result.get("exceptionDetails").is_some() { return Err("eval-attach: unverified frame owner\n".into()); }
    result.pointer("/result/value").cloned().ok_or_else(|| "eval-attach: unverified frame owner\n".into())
}

pub(crate) fn evaluate(cdp: &mut Cdp, invocation: &Invocation, target: &str, session: &str, expression: &str) -> Result<crate::browser::EvalValue, String> {
    let frame = invocation.flags.get("frame").ok_or("eval: --attachment requires --frame\n")?;
    let installed = crate::eval_lease::context(invocation, target, frame)?;
    let frames = installed["chain"].as_array().ok_or("eval: invalid attachment document\n")?;
    let mut opened = Vec::new();
    let result = (|| {
        let mut chain = Vec::new();
        let mut current_target = target.to_string();
        let mut current_session = session.to_string();
        for entry in frames {
            let owner = entry["target"].as_str().ok_or("eval: invalid attachment owner\n")?;
            if owner != current_target {
                let info = cdp.call("Target.getTargetInfo", json!({"targetId":owner}), None)?;
                if info["targetInfo"]["type"] != "iframe" || chain.last().is_none_or(|parent: &Document| info["targetInfo"]["parentFrameId"] != parent.text("id")) {
                    return Err("eval: attachment ancestor changed\n".into());
                }
                current_session = crate::cdp::attach(cdp, owner)?;
                opened.push(current_session.clone());
                current_target = owner.to_string();
            }
            chain.push(Document { session: current_session.clone(), target: owner.to_string(), frame: entry["frame"].clone() });
        }
        verify(cdp, &chain)?;
        let context = installed["context"].as_str().ok_or("eval: invalid attachment context\n")?;
        cdp.call("Runtime.enable", json!({}), Some(&current_session))?;
        let result = crate::browser::evaluate_called_context(cdp, &current_session, None, Some(context), expression)?;
        verify(cdp, &chain)?;
        Ok(result)
    })();
    for session in opened.into_iter().rev() { crate::cdp::detach(cdp, &session); }
    result
}
