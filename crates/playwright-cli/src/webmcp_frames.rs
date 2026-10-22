use crate::browser;
use crate::snapshot::FrameRec;
use crate::cdp::{self, Cdp};
use crate::session::Session;
use crate::webmcp_polyfill::bridge_name;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

const PAGE_DEFINED: &str = "(() => { const api = document.modelContext; return !!api && !api.__slicc_webmcp_shim && (!('modelContext' in Document.prototype) || Object.prototype.hasOwnProperty.call(document, 'modelContext')); })()";

const CAP: &str = r#"const cap = (value, limit = 65536) => {
  if (typeof value !== 'string') return '';
  const length = value.length < limit ? value.length : limit;
  let out = '';
  for (let i = 0; i < length; i++) {
    const c = value[i];
    if (c >= '\ud800' && c <= '\udbff') {
      const next = value[i + 1];
      if (i + 1 < length && next >= '\udc00' && next <= '\udfff') { out += c + next; i++; }
      else out += '\ufffd';
    } else if (c >= '\udc00' && c <= '\udfff') out += '\ufffd';
    else out += c;
  }
  return out;
};
const quote = (value) => {
  let text = '"';
  const controls = '\u0000\u0001\u0002\u0003\u0004\u0005\u0006\u0007\u0008\u0009\u000a\u000b\u000c\u000d\u000e\u000f\u0010\u0011\u0012\u0013\u0014\u0015\u0016\u0017\u0018\u0019\u001a\u001b\u001c\u001d\u001e\u001f';
  const hex = '0123456789abcdef';
  for (let i = 0; i < value.length; i++) {
    const c = value[i];
    if (c === '"' || c === '\\') text += '\\' + c;
    else if (c < ' ') {
      for (let j = 0; j < 32; j++) if (c === controls[j]) text += '\\u00' + hex[j >> 4] + hex[j & 15];
    } else text += c;
  }
  return text + '"';
};
const pack = (value) => {
  const keys = ['kind','toolsJson','__webmcp_error','__webmcp_result','message','description'];
  let text = '{';
  for (let i = 0; i < keys.length; i++) {
    const key = keys[i];
    const item = value[key];
    if (typeof item !== 'string') continue;
    if (text.length > 1) text += ',';
    text += quote(key) + ':' + quote(cap(item));
  }
  if (text.length > 397312) return '{"__webmcp_error":"limit"}';
  return text + '}';
};
const meta = (t) => ({name:t.name,title:t.title || null,description:t.description || '',inputSchema:typeof t.inputSchema === 'string' ? JSON.parse(t.inputSchema) : t.inputSchema || null,exposedTo:t.exposedTo || [],annotations:{readOnlyHint:t.annotations?.readOnlyHint ?? t.annotations?.readOnly,consequentialHint:t.annotations?.consequentialHint ?? t.annotations?.consequential,untrustedContentHint:t.annotations?.untrustedContentHint ?? t.annotations?.untrustedContent}});
"#;

#[derive(Clone)]
struct Scope {
    session: String,
    frame: String,
    origin: String,
    loader: String,
    isolated: i64,
    isolated_unique: String,
    main: Option<String>,
    bridge: Option<String>,
    policy: Option<String>,
    path: Vec<usize>,
    page_defined: bool,
}

#[derive(Clone)]
struct Tool {
    value: Value,
    scope: Scope,
    kind: String,
    native_scope: Option<Scope>,
}

struct Collected {
    tools: Vec<Tool>,
    available: bool,
    frames: Vec<FrameRec>,
    scopes: BTreeMap<String, Scope>,
}

pub fn origins(values: &[&str]) -> Result<Vec<String>, String> {
    let mut result = BTreeSet::new();
    for value in values {
        let parsed = url::Url::parse(value).map_err(|_| "--from-origin requires an http(s) origin\n")?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() || !parsed.username().is_empty() || parsed.password().is_some() {
            return Err("--from-origin requires an http(s) origin without path, query, fragment or credentials\n".into());
        }
        result.insert(parsed.origin().ascii_serialization());
    }
    Ok(result.into_iter().collect())
}

pub fn list(cdp: &mut Cdp, state: &Session, target: &str, session: &str, from: &[String]) -> Result<Value, String> {
    with_tools(cdp, state, target, session, from, |_, collected| if collected.available { Ok(Value::Array(collected.tools.into_iter().map(|tool| tool.value).collect())) } else { Ok(Value::Null) })
}

pub fn call(cdp: &mut Cdp, state: &Session, target: &str, session: &str, from: &[String], frame: Option<&str>, name: &str, input: &Value, allow: bool) -> Result<Value, String> {
    with_tools(cdp, state, target, session, from, |cdp, collected| {
        if !collected.available { return Ok(json!({"__webmcp_error":"absent"})); }
        let tools = &collected.tools;
        let matches = tools.iter().filter(|tool| tool.value.get("name").and_then(Value::as_str) == Some(name) && frame.is_none_or(|frame| frame == tool.scope.frame)).collect::<Vec<_>>();
        if matches.is_empty() { return Ok(json!({"__webmcp_error":"unknown","tools":tools.iter().filter_map(|tool| tool.value.get("name")).collect::<Vec<_>>()})); }
        if matches.len() != 1 { return Err("webmcp-call: ambiguous tool name; select --frame <frameId>\n".into()); }
        let tool = matches[0];
        if !allow && tool.value.pointer("/annotations/consequentialHint").and_then(Value::as_bool) == Some(true) {
            return Ok(json!({"__webmcp_error":"consequential","description":tool.value["description"]}));
        }
        let native = tool.kind == "native";
        let object_input = crate::webmcp::chrome_major_version(cdp).is_some_and(|major| major >= 155);
        revalidate(cdp, &tool.scope, &collected, tool.kind != "native")?;
        let input_expression = if object_input { input.to_string() } else { format!("JSON.stringify({input})") };
        let bridge = tool.scope.bridge.as_deref().unwrap_or("undefined");
        let expression = if native {
            format!(r#"(async () => {{ 'use strict'; {CAP}
try {{
  const api = document.modelContext;
  if (!api) return pack({{__webmcp_error:'absent'}});
  let owner = window;
  for (const index of {path}) owner = owner.frames[index];
  const tools = await api.getTools({{fromOrigins:{from}}});
  const tool = tools.find(t => t.name === {name} && t.window === owner && t.origin === {origin});
  if (!tool) return pack({{__webmcp_error:'unknown'}});
  if (!{allow} && (tool.annotations?.consequentialHint || tool.annotations?.consequential)) return pack({{__webmcp_error:'consequential',description:cap(tool.description || '')}});
  const result = await api.executeTool(tool, {input});
  return pack({{__webmcp_result:cap(typeof result === 'string' ? result : JSON.stringify(result ?? null))}});
}} catch (error) {{ try {{ return pack({{__webmcp_error:'threw',message:cap(String(error))}}); }} catch {{ return '{{"__webmcp_error":"threw","message":"Tool invocation failed"}}'; }} }}
}})()"#, path=json!(tool.scope.path), from=json!(from), name=json!(name), origin=json!(tool.scope.origin), allow=json!(allow), input=input_expression)
        } else {
            format!(r#"(async () => {{ 'use strict'; {CAP}
try {{
  const bridge = typeof {bridge} === 'undefined' ? null : {bridge};
  if ({is_bridge} && bridge) {{
    const reply = await bridge.call({name}, {input}, {allow});
    if (reply.__webmcp_result !== undefined) reply.__webmcp_result = cap(reply.__webmcp_result);
    if (reply.message !== undefined) reply.message = cap(reply.message);
    return pack(reply);
  }}
  const api = document.modelContext;
  if (!api) return pack({{__webmcp_error:'absent'}});
  const tools = await api.getTools();
  let tool;
  for (let i = 0; i < tools.length; i++) if (tools[i].name === {name} && (!tools[i].window || tools[i].window === window)) {{ tool = tools[i]; break; }}
  if (!tool) return pack({{__webmcp_error:'unknown'}});
  if (!{allow} && (tool.annotations?.consequentialHint || tool.annotations?.consequential)) return pack({{__webmcp_error:'consequential',description:cap(tool.description || '')}});
  const input = {object_input} && Object.prototype.toString.call(api) === '[object ModelContext]' ? {input} : JSON.stringify({input});
  const result = await api.executeTool(tool, input);
  return pack({{__webmcp_result:cap(typeof result === 'string' ? result : JSON.stringify(result ?? null))}});
}} catch (error) {{ try {{ return pack({{__webmcp_error:'threw',message:cap(String(error))}}); }} catch {{ return '{{"__webmcp_error":"threw","message":"Tool invocation failed"}}'; }} }}
}})()"#, is_bridge=json!(tool.kind == "bridge"), name=json!(name), input=input, allow=json!(allow), object_input=json!(object_input))
        };
        let scope = tool.native_scope.as_ref().unwrap_or(&tool.scope);
        let child = cdp.webmcp_document(&tool.scope.session, &tool.scope.frame)?;
        if child.get("loaderId").and_then(Value::as_str) != Some(&tool.scope.loader) { return Err("webmcp: document changed\n".into()); }
        let evaluated = evaluate(cdp, scope, &expression, native).and_then(|value| {
            revalidate(cdp, &tool.scope, &collected, tool.kind != "native")?;
            Ok(value)
        });
        match evaluated {
            Err(message) if message == "webmcp: document changed\n" || message.starts_with("Runtime.evaluate:") && (message.contains("Execution context was destroyed") || message.contains("Cannot find context with specified id")) => Ok(json!({"__webmcp_result":"null"})),
            other => other,
        }
    })
}

fn with_tools(cdp: &mut Cdp, state: &Session, target: &str, session: &str, from: &[String], invoke: impl FnOnce(&mut Cdp, Collected) -> Result<Value, String>) -> Result<Value, String> {
    if from.iter().any(|origin| cdp.guard().url(origin)) { return Err(format!("{}\n", cdp.guard().refusal())); }
    let mut opened = Vec::new();
    let result = collect(cdp, state, target, session, from, &mut opened).and_then(|collected| invoke(cdp, collected));
    for child in opened { cdp::detach(cdp, &child); }
    result
}

fn collect(cdp: &mut Cdp, state: &Session, target: &str, session: &str, from: &[String], opened: &mut Vec<String>) -> Result<Collected, String> {
    let frames = browser::page_frames(cdp, session)?;
    let root = frames.first().ok_or_else(|| "webmcp: unverified frame tree\n".to_string())?;
    let mut scopes = BTreeMap::new();
    let mut owners = BTreeMap::new();
    build_scope(cdp, state, target, session, root, &mut scopes, opened)?;
    let top = scopes[&root.frame_id].origin.clone();
    for frame in &frames {
        if frame.frame_id == root.frame_id { continue; }
        let origin = url::Url::parse(&frame.url).map(|url| url.origin().ascii_serialization()).unwrap_or_default();
        if origin != top && !from.contains(&origin) { continue; }
        if cdp.guard().url(&frame.url) || !matches!(url::Url::parse(&frame.url).map(|url| url.scheme().to_string()).as_deref(), Ok("http" | "https")) { continue; }
        let chain = ancestor_chain(frame, &frames, &scopes)?;
        for item in chain.iter().rev() {
            if !scopes.contains_key(&item.frame_id) {
                if build_scope(cdp, state, target, session, item, &mut scopes, opened).is_err() { continue; }
            }
            owners.extend(link_owner(cdp, &mut scopes, item)?.map(|owner| (item.frame_id.clone(), owner)));
        }
    }
    let root_scope = scopes[&root.frame_id].clone();
    let native = evaluate(cdp, &root_scope, "'modelContext' in Document.prototype", true)?.as_bool() == Some(true);
    let available = evaluate(cdp, &root_scope, "document.modelContext != null", false)?.as_bool() == Some(true);
    let mut result = Vec::new();
    if native && available {
        let mut selected = Vec::new();
        for scope in scopes.values() {
            if (scope.origin == top || from.contains(&scope.origin)) && allowed(cdp, scope, &frames, &scopes, &owners, false)? {
                selected.push(json!({"frameId":scope.frame,"origin":scope.origin,"path":scope.path}));
            }
        }
        let expression = format!(r#"(async () => {{ 'use strict'; {CAP}
try {{
 const api = document.modelContext;
 if (!api) return pack({{toolsJson:'[]'}});
 const tools = await api.getTools({{fromOrigins:{from}}});
 const result = [];
 for (const t of tools) for (const frame of {frames}) {{
   let owner = window;
   for (const index of frame.path) owner = owner.frames[index];
   if (t.window === owner && t.origin === frame.origin) result.push({{...meta(t),frameId:frame.frameId,origin:frame.origin}});
 }}
 const text = JSON.stringify(result);
 return text.length > 65536 ? pack({{__webmcp_error:'limit'}}) : pack({{toolsJson:text}});
}} catch {{ return '{{"__webmcp_error":"threw","message":"WebMCP listing failed"}}'; }}
}})()"#, from=json!(from), frames=json!(selected));
        let value = evaluate(cdp, &root_scope, &expression, true)?;
        for value in decode_tools(&value)? {
            let Some(id) = value.get("frameId").and_then(Value::as_str) else { continue };
            let Some(scope) = scopes.get(id) else { continue };
            let mut value = value;
            value["policy"] = json!("allowed");
            result.push(Tool { value, scope: scope.clone(), kind: "native".into(), native_scope: Some(root_scope.clone()) });
        }
    }
    {
        for scope in scopes.values() {
            if result.iter().any(|tool| tool.scope.frame == scope.frame) { continue; }
            if scope.origin != top && !from.contains(&scope.origin) { continue; }
            if !allowed(cdp, scope, &frames, &scopes, &owners, true)? { continue; }
            let unknown = scope.page_defined && scope.policy.is_none() && scope.path.is_empty() && native_policy(cdp, scope)?.is_none();
            let bridge = scope.bridge.as_deref().unwrap_or("undefined");
            let expression = format!(r#"(async () => {{ 'use strict'; {CAP}
try {{
 const bridge = typeof {bridge} === 'undefined' ? null : {bridge};
 if (bridge) {{ const listed = bridge.list(); if (listed.error) return pack({{__webmcp_error:'limit'}}); if (!listed.replaced || JSON.parse(listed.toolsJson).length) return pack({{kind:'bridge',toolsJson:listed.toolsJson}}); }}
 const api = document.modelContext;
 if (!api) return pack({{kind:'none',toolsJson:'[]'}});
 const list = await api.getTools();
 const result = [];
 for (let i = 0; i < list.length; i++) if (!list[i].window || list[i].window === window) result.push(meta(list[i]));
 const text = JSON.stringify(result);
 return typeof text !== 'string' || text.length > 65536 ? pack({{__webmcp_error:'limit'}}) : pack({{kind:'page',toolsJson:text}});
}} catch {{ return '{{"__webmcp_error":"threw","message":"WebMCP listing failed"}}'; }}
}})()"#);
            let value = evaluate(cdp, scope, &expression, false)?;
            for mut tool in decode_tools(&value)? {
                if scope.origin != top && !exposed(&tool, &top) { continue; }
                if !tool.is_object() || tool.get("name").and_then(Value::as_str).is_none() { continue; }
                tool["origin"] = json!(scope.origin);
                tool["frameId"] = json!(scope.frame);
                tool["polyfilled"] = json!(value.get("kind").and_then(Value::as_str) == Some("bridge"));
                tool["policy"] = json!(if unknown { "unknown" } else { "allowed" });
                tool.as_object_mut().unwrap().remove("exposedTo");
                result.push(Tool { value: tool, scope: scope.clone(), kind: value.get("kind").and_then(Value::as_str).unwrap_or("page").to_string(), native_scope: None });
            }
        }
    }
    for scope in scopes.values() {
        let current = cdp.webmcp_document(&scope.session, &scope.frame)?;
        if current.get("loaderId").and_then(Value::as_str) != Some(&scope.loader) { return Err("webmcp: document changed\n".into()); }
    }
    result.sort_by(|a, b| a.value["name"].as_str().cmp(&b.value["name"].as_str()).then(a.scope.frame.cmp(&b.scope.frame)));
    let available = available || !result.is_empty();
    Ok(Collected { tools: result, available, frames, scopes })
}

fn revalidate(cdp: &mut Cdp, scope: &Scope, collected: &Collected, compatibility: bool) -> Result<(), String> {
    let mut current = scope;
    let mut seen = BTreeSet::new();
    let mut owners = BTreeMap::new();
    loop {
        if !seen.insert(current.frame.clone()) { return Err("webmcp: unverified ancestor chain\n".into()); }
        let document = cdp.webmcp_document(&current.session, &current.frame)?;
        if document.get("loaderId").and_then(Value::as_str) != Some(&current.loader) || document.get("origin").and_then(Value::as_str) != Some(&current.origin) {
            return Err("webmcp: document changed\n".into());
        }
        let Some(parent) = collected.frames.iter().find(|frame| frame.frame_id == current.frame).and_then(|frame| frame.parent_frame_id.as_deref()) else { break; };
        let parent = collected.scopes.get(parent).ok_or_else(|| "webmcp: unverified ancestor chain\n".to_string())?;
        let value = owner(cdp, parent, &current.frame)?;
        if value["index"].as_u64().and_then(|index| usize::try_from(index).ok()) != current.path.last().copied() { return Err("webmcp: document changed\n".into()); }
        owners.insert(current.frame.clone(), value);
        current = parent;
    }
    let mut approved = scope.clone();
    approved.page_defined = scope.page_defined && evaluate(cdp, scope, PAGE_DEFINED, false)?.as_bool() == Some(true);
    if !allowed(cdp, &approved, &collected.frames, &collected.scopes, &owners, compatibility)? { return Err("webmcp-call: tool permission changed\n".into()); }
    Ok(())
}

fn ancestor_chain(frame: &FrameRec, frames: &[FrameRec], scopes: &BTreeMap<String, Scope>) -> Result<Vec<FrameRec>, String> {
    let mut chain = vec![frame.clone()];
    let mut current = frame;
    let mut seen = BTreeSet::new();
    while let Some(parent) = current.parent_frame_id.as_deref() {
        if !seen.insert(parent.to_string()) { return Err("webmcp: unverified ancestor chain\n".into()); }
        if scopes.contains_key(parent) { break; }
        let next = frames.iter().find(|frame| frame.frame_id == parent).ok_or_else(|| "webmcp: unverified ancestor chain\n".to_string())?;
        chain.push(next.clone());
        current = next;
    }
    Ok(chain)
}

fn decode_tools(value: &Value) -> Result<Vec<Value>, String> {
    if value.get("__webmcp_error").and_then(Value::as_str) == Some("absent") { return Ok(Vec::new()); }
    if value.get("__webmcp_error").is_some() { return Err("webmcp-list: page result exceeds the limit or listing failed\n".into()); }
    let text = value.get("toolsJson").and_then(Value::as_str).filter(|text| text.len() <= 393216).ok_or_else(|| "webmcp-list: unexpected evaluation result\n".to_string())?;
    serde_json::from_str::<Value>(text).map_err(|_| "webmcp-list: invalid tool metadata\n".to_string())?.as_array().cloned().ok_or_else(|| "webmcp-list: invalid tool metadata\n".into())
}

fn evaluate(cdp: &mut Cdp, scope: &Scope, expression: &str, native: bool) -> Result<Value, String> {
    let before = cdp.webmcp_document(&scope.session, &scope.frame)?;
    if before.get("loaderId").and_then(Value::as_str) != Some(&scope.loader) || before.get("origin").and_then(Value::as_str) != Some(&scope.origin) { return Err("webmcp: document changed\n".into()); }
    let context = if native { &scope.isolated_unique } else { scope.main.as_ref().ok_or_else(|| "webmcp: unverified page context\n".to_string())? };
    let response = cdp.call("Runtime.evaluate", json!({"expression":expression,"uniqueContextId":context,"awaitPromise":true,"returnByValue":true,"userGesture":true}), Some(&scope.session))?;
    if let Some(details) = response.get("exceptionDetails") {
        let message = details.pointer("/exception/description").and_then(Value::as_str).or_else(|| details.get("text").and_then(Value::as_str)).unwrap_or("page evaluation failed");
        return Err(format!("webmcp: {}\n", crate::sanitize::browser_message(message)));
    }
    let after = cdp.webmcp_document(&scope.session, &scope.frame)?;
    if before != after { return Err("webmcp: document changed\n".into()); }
    let value = response.pointer("/result/value").cloned().unwrap_or(Value::Null);
    if let Some(text) = value.as_str() { serde_json::from_str(text).map_err(|_| "webmcp: unexpected evaluation result\n".into()) } else { Ok(value) }
}

fn build_scope(cdp: &mut Cdp, state: &Session, target: &str, session: &str, frame: &FrameRec, scopes: &mut BTreeMap<String, Scope>, opened: &mut Vec<String>) -> Result<(), String> {
    let session = if frame.target.is_some() {
        let (child, _) = browser::open_frame(cdp, frame)?;
        opened.push(child.clone());
        child
    } else { session.to_string() };
    let document = cdp.webmcp_document(&session, &frame.frame_id)?;
    cdp.call("Runtime.enable", json!({}), Some(&session))?;
    let world = cdp.call("Page.createIsolatedWorld", json!({"frameId":frame.frame_id,"worldName":"slicc-webmcp"}), Some(&session))?;
    let isolated = world.get("executionContextId").and_then(Value::as_i64).ok_or_else(|| "webmcp: unverified isolated context\n".to_string())?;
    let isolated_unique = active_context(cdp.events(), &session, &frame.frame_id, Some(isolated)).ok_or_else(|| "webmcp: unverified isolated context\n".to_string())?;
    let main = active_context(cdp.events(), &session, &frame.frame_id, None);
    if cdp.webmcp_document(&session, &frame.frame_id)? != document { return Err("webmcp: document changed\n".into()); }
    let frame_target = frame.target.as_deref().unwrap_or(target).to_string();
    let loader = document.get("loaderId").and_then(Value::as_str).unwrap_or("").to_string();
    let origin = document.get("origin").and_then(Value::as_str).unwrap_or("").to_string();
    let remembered = state.webmcp.get(target).filter(|entry| entry.get("connection").and_then(Value::as_str) == Some(&state.webmcp_connection)).and_then(|entry| entry.get("sessions")).and_then(Value::as_array).and_then(|entries| entries.iter().find(|entry| entry.get("target").and_then(Value::as_str) == Some(&frame_target)));
    let bridge = remembered.and_then(|entry| entry.get("bridge")).and_then(Value::as_str).filter(|name| bridge_name(name)).map(str::to_string);
    let policy = remembered.and_then(|entry| entry.get("documents")).and_then(|documents| documents.get(format!("{}:{}", frame.frame_id, loader))).filter(|entry| entry.get("origin").and_then(Value::as_str) == Some(&origin)).and_then(|entry| entry.get("policy")).and_then(Value::as_str).map(str::to_string);
    let mut scope = Scope { session, frame: frame.frame_id.clone(), origin, loader, isolated, isolated_unique, main, bridge, policy, path: Vec::new(), page_defined: false };
    scope.page_defined = evaluate(cdp, &scope, PAGE_DEFINED, false)?.as_bool() == Some(true);
    scopes.insert(frame.frame_id.clone(), scope);
    Ok(())
}

fn owner(cdp: &mut Cdp, parent: &Scope, child: &str) -> Result<Value, String> {
    let before = cdp.webmcp_document(&parent.session, &parent.frame)?;
    if before.get("loaderId").and_then(Value::as_str) != Some(&parent.loader) || before.get("origin").and_then(Value::as_str) != Some(&parent.origin) { return Err("webmcp: document changed\n".into()); }
    let document = cdp.call("DOM.getDocument", json!({"depth":1,"pierce":false}), Some(&parent.session))?;
    let nodes = cdp.call("DOM.querySelectorAll", json!({"nodeId":document["root"]["nodeId"],"selector":"iframe,frame"}), Some(&parent.session))?;
    let ids = nodes.get("nodeIds").and_then(Value::as_array).filter(|ids| ids.len() <= 256).ok_or_else(|| "webmcp: unverified frame owner\n".to_string())?;
    let mut backend = None;
    for id in ids {
        let described = cdp.call("DOM.describeNode", json!({"nodeId":id,"depth":0,"pierce":false}), Some(&parent.session))?;
        if described.pointer("/node/frameId").and_then(Value::as_str) == Some(child) {
            if backend.is_some() { return Err("webmcp: unverified frame owner\n".into()); }
            backend = Some(described.pointer("/node/backendNodeId").and_then(Value::as_i64).ok_or_else(|| "webmcp: unverified frame owner\n".to_string())?);
        }
    }
    let backend = backend.ok_or_else(|| "webmcp: unverified frame owner\n".to_string())?;
    let object = cdp.call("DOM.resolveNode", json!({"backendNodeId":backend,"executionContextId":parent.isolated}), Some(&parent.session))?;
    let result = cdp.call("Runtime.callFunctionOn", json!({"objectId":object["object"]["objectId"],"functionDeclaration":"function webmcp_owner() { 'use strict'; let index = -1; for (let i = 0; i < window.frames.length; i++) if (window.frames[i] === this.contentWindow) index = i; return {allow:this.getAttribute('allow') || '',srcOrigin:new URL(this.src,document.baseURI).origin,index}; }","returnByValue":true}), Some(&parent.session))?;
    if result.get("exceptionDetails").is_some() { return Err("webmcp: unverified frame owner\n".into()); }
    if cdp.webmcp_document(&parent.session, &parent.frame)? != before { return Err("webmcp: document changed\n".into()); }
    result.pointer("/result/value").cloned().ok_or_else(|| "webmcp: unverified frame owner\n".into())
}

fn link_owner(cdp: &mut Cdp, scopes: &mut BTreeMap<String, Scope>, frame: &FrameRec) -> Result<Option<Value>, String> {
    let Some(parent) = frame.parent_frame_id.as_ref().and_then(|id| scopes.get(id)).cloned() else { return Ok(None); };
    let owner = owner(cdp, &parent, &frame.frame_id)?;
    let scope = scopes.get_mut(&frame.frame_id).ok_or_else(|| "webmcp: unverified frame scope\n".to_string())?;
    scope.path = parent.path;
    scope.path.push(owner["index"].as_u64().ok_or_else(|| "webmcp: unverified frame owner\n".to_string())? as usize);
    Ok(Some(owner))
}

fn active_context(events: &[Value], session: &str, frame: &str, requested: Option<i64>) -> Option<String> {
    let mut destroyed = BTreeSet::new();
    for event in events.iter().rev().filter(|event| event.get("sessionId").and_then(Value::as_str) == Some(session)) {
        match event.get("method").and_then(Value::as_str) {
            Some("Runtime.executionContextsCleared") => break,
            Some("Runtime.executionContextDestroyed") => {
                if let Some(id) = event.pointer("/params/executionContextId").and_then(Value::as_i64) { destroyed.insert(id); }
            }
            Some("Runtime.executionContextCreated") => {
                let context = &event["params"]["context"];
                let id = context.get("id").and_then(Value::as_i64)?;
                if destroyed.contains(&id) || context.pointer("/auxData/frameId").and_then(Value::as_str) != Some(frame) { continue; }
                if requested.map_or_else(|| context.pointer("/auxData/isDefault").and_then(Value::as_bool) == Some(true), |requested| requested == id) {
                    return context.get("uniqueId").and_then(Value::as_str).filter(|unique| !unique.is_empty()).map(str::to_string);
                }
            }
            _ => {}
        }
    }
    None
}

fn native_policy(cdp: &mut Cdp, scope: &Scope) -> Result<Option<bool>, String> {
    let value = match cdp.call("Page.getPermissionsPolicyState", json!({"frameId":scope.frame}), Some(&scope.session)) {
        Ok(value) => value,
        Err(message) if message.starts_with("Page.getPermissionsPolicyState:") && (message.contains("wasn't found") || message.contains("Method not found")) => return Ok(None),
        Err(message) => return Err(message),
    };
    let states = value.get("states").and_then(Value::as_array).ok_or_else(|| "webmcp: unverified Permissions Policy response\n".to_string())?;
    match states.iter().find(|state| state.get("feature").and_then(Value::as_str) == Some("tools")) {
        Some(state) => state.get("allowed").and_then(Value::as_bool).map(Some).ok_or_else(|| "webmcp: unverified tools Permissions Policy state\n".to_string()),
        None => Ok(None),
    }
}

fn allowed(cdp: &mut Cdp, scope: &Scope, frames: &[FrameRec], scopes: &BTreeMap<String, Scope>, owners: &BTreeMap<String, Value>, compatibility: bool) -> Result<bool, String> {
    let native = native_policy(cdp, scope)?;
    if native == Some(false) { return Ok(false); }
    let mut current = scope;
    let mut seen = BTreeSet::new();
    loop {
        let current_native = if current.frame == scope.frame { native } else {
            native_policy(cdp, current)?
        };
        if current_native == Some(false) { return Ok(false); }
        if !seen.insert(current.frame.clone()) { return Ok(false); }
        match current.policy.as_deref() {
            Some(header) if !header_allows(header, &current.origin, &current.origin) => return Ok(false),
            None if current_native.is_none() && !(compatibility && current.frame == scope.frame && scope.page_defined && frames.iter().any(|frame| frame.frame_id == scope.frame && frame.parent_frame_id.is_none())) => return Ok(false),
            _ => {}
        }
        let Some(parent) = frames.iter().find(|frame| frame.frame_id == current.frame).and_then(|frame| frame.parent_frame_id.as_deref()) else { return Ok(true); };
        let Some(parent) = scopes.get(parent) else { return Ok(false) };
        let Some(owner) = owners.get(&current.frame) else { return Ok(false) };
        if !iframe_allows(owner.get("allow").and_then(Value::as_str).unwrap_or(""), &parent.origin, &current.origin, owner.get("srcOrigin").and_then(Value::as_str).unwrap_or("")) { return Ok(false); }
        if parent.policy.as_deref().is_some_and(|header| !header_allows(header, &parent.origin, &current.origin)) { return Ok(false); }
        current = parent;
    }
}

fn exposed(tool: &Value, top: &str) -> bool {
    tool.get("exposedTo").and_then(Value::as_array).is_some_and(|origins| origins.iter().any(|origin| origin.as_str().is_some_and(|origin| origin == "*" || origin == top)))
}

fn header_allows(header: &str, own: &str, origin: &str) -> bool {
    let entries = header.split(',').filter(|entry| entry.trim().starts_with("tools")).collect::<Vec<_>>();
    if entries.is_empty() { return true; }
    if entries.len() != 1 { return false; }
    let Some((name, value)) = entries[0].trim().split_once('=') else { return false };
    if name.trim() != "tools" { return false; }
    let value = value.trim();
    if value == "*" { return true; }
    let Some(values) = value.strip_prefix('(').and_then(|value| value.strip_suffix(')')) else { return false };
    let mut matched = false;
    for value in values.split_whitespace() {
        if value == "self" { matched |= own == origin; continue; }
        let Some(value) = value.strip_prefix('"').and_then(|value| value.strip_suffix('"')) else { return false };
        let Ok(parsed) = url::Url::parse(value) else { return false };
        if !matches!(parsed.scheme(), "http" | "https") || parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() || !parsed.username().is_empty() || parsed.password().is_some() { return false; }
        matched |= parsed.origin().ascii_serialization() == origin;
    }
    matched
}

fn iframe_allows(allow: &str, parent: &str, child: &str, src: &str) -> bool {
    let entries = allow.split(';').filter_map(|entry| {
        let values = entry.split_whitespace().collect::<Vec<_>>();
        (values.first() == Some(&"tools")).then_some(values)
    }).collect::<Vec<_>>();
    if entries.is_empty() { return parent == child; }
    if entries.len() != 1 { return false; }
    let values = &entries[0][1..];
    if values.is_empty() { return child == src; }
    if values.contains(&"'none'") { return false; }
    values.iter().any(|value| *value == "*" || *value == "'self'" && parent == child || *value == "'src'" && src == child || *value == child)
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("webmcp_frames_test.rs");

    #[test]
    fn permissions_policy_headers_and_iframe_delegation_fail_closed() {
        let top = "https://top.test";
        let child = "https://child.test";
        assert!(header_allows("", top, child));
        assert!(!header_allows("tools=()", top, top));
        assert!(header_allows("tools=(self)", top, top));
        assert!(!header_allows("tools=(self)", top, child));
        assert!(header_allows("tools=(\"https://child.test\")", top, child));
        assert!(!header_allows("tools=(self),tools=*", top, child));
        assert!(!header_allows("tools=broken", top, child));
        assert!(!iframe_allows("", top, child, child));
        assert!(iframe_allows("", top, top, top));
        assert!(iframe_allows("tools", top, child, child));
        assert!(!iframe_allows("tools", top, child, "https://redirect.test"));
        assert!(iframe_allows("tools *", top, child, "https://redirect.test"));
        assert!(!iframe_allows("tools 'none' *", top, child, child));
        assert!(!iframe_allows("tools;tools *", top, child, child));
    }

    #[test]
    fn exposed_to_and_cross_origin_opt_in_are_explicit() {
        assert!(!exposed(&json!({}), "https://top.test"));
        assert!(exposed(&json!({"exposedTo":["https://top.test"]}), "https://top.test"));
        assert!(!exposed(&json!({"exposedTo":["https://other.test"]}), "https://top.test"));
        assert!(origins(&["https://top.test/path?token=hidden"]).is_err());
        assert_eq!(origins(&["https://top.test", "https://top.test/"]).unwrap(), vec!["https://top.test"]);
    }
}
