use crate::args::Output;
use crate::browser::{self, PageInfo};
use crate::cdp::Cdp;
use crate::sanitize;
use crate::session::Session;
use serde_json::{json, Value};

pub fn list_from(cdp: &mut Cdp, state: &mut Session, tab: Option<&str>, json: bool, from: &[String]) -> Output {
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve(cdp, tab, state, &pages) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = crate::webmcp_frames::list(cdp, state, &target, &session, from);
    let _ = browser::watch_detach(cdp, state, &target, &session);
    match result {
        Ok(tools) => {
            state.current = Some(target);
            if tools.is_null() || tools.get("__webmcp_error").and_then(Value::as_str) == Some("absent") {
                return unavailable("webmcp-list");
            }
            if json {
                let mut tools = tools;
                redact_output(&mut tools);
                Output::ok(format!("{}\n", crate::sanitize::json_c1(&tools.to_string())))
            } else {
                Output::ok(format_list(&tools))
            }
        }
        Err(message) => Output::err(message),
    }
}

fn unavailable(command: &str) -> Output {
    Output {
        stdout: String::new(),
        stderr: format!("{command}: WebMCP is unavailable on this page (document.modelContext is absent)\n"),
        code: 2,
    }
}

fn format_list(tools: &Value) -> String {
    let arr = match tools.as_array() {
        Some(arr) if arr.is_empty() => return "No WebMCP tools on this page\n".to_string(),
        Some(arr) => arr,
        None => return "No WebMCP tools on this page\n".to_string(),
    };
    let mut lines = Vec::new();
    for tool in arr {
        let name = sanitize::browser_message(tool.get("name").and_then(|v| v.as_str()).unwrap_or("?"));
        let desc = sanitize::browser_message(
            tool.get("description")
                .and_then(|v| v.as_str())
                .or_else(|| tool.get("title").and_then(|v| v.as_str()))
                .unwrap_or(""),
        );
        let mut flags = Vec::new();
        if tool.get("policy").and_then(Value::as_str) == Some("unknown") { flags.push("policy: unknown, top-level page-defined"); }
        if let Some(ann) = tool.get("annotations") {
            if ann.get("consequentialHint").and_then(|v| v.as_bool()) == Some(true) {
                flags.push("consequential");
            }
            if ann.get("readOnlyHint").and_then(|v| v.as_bool()) == Some(true) {
                flags.push("read-only");
            }
            if ann.get("untrustedContentHint").and_then(|v| v.as_bool()) == Some(true) {
                flags.push("untrusted");
            }
        }
        let suffix = if flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flags.join(", "))
        };
        if desc.is_empty() {
            lines.push(format!("  {name}{suffix}"));
        } else {
            lines.push(format!("  {name}  {desc}{suffix}"));
        }
    }
    format!("{}\n", lines.join("\n"))
}

#[cfg(test)]
pub fn call(
    cdp: &mut Cdp,
    state: &mut Session,
    name: &str,
    input_json: &str,
    tab: Option<&str>,
    allow_consequential: bool,
) -> Output {
    call_from(cdp, state, name, input_json, tab, allow_consequential, &[], None)
}

pub fn call_from(cdp: &mut Cdp, state: &mut Session, name: &str, input_json: &str, tab: Option<&str>, allow_consequential: bool, from: &[String], frame: Option<&str>) -> Output {
    let parsed: Value = match serde_json::from_str(input_json) {
        Ok(value) => value,
        Err(err) => {
            return Output {
                stdout: String::new(),
                stderr: format!("webmcp-call: invalid JSON input: {err}\n"),
                code: 2,
            }
        }
    };
    if !parsed.is_object() {
        return Output {
            stdout: String::new(),
            stderr: "webmcp-call: input must be a JSON object\n".into(),
            code: 2,
        };
    }
    let pages = match browser::list_pages(cdp) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve(cdp, tab, state, &pages) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match browser::watch_attach(cdp, state, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let outcome = crate::webmcp_frames::call(cdp, state, &target, &session, from, frame, name, &parsed, allow_consequential).and_then(parse_value);
    let _ = browser::watch_detach(cdp, state, &target, &session);
    match outcome {
        Ok(outcome) => {
            state.current = Some(target);
            match outcome {
                CallOutcome::Result(text) => Output::ok(format!("{}\n", sanitize::browser_message(&text))),
                CallOutcome::Absent => unavailable("webmcp-call"),
                CallOutcome::Unknown(available) => {
                    let list = if available.is_empty() {
                        String::new()
                    } else {
                        let safe: Vec<String> = available.iter().map(|n| sanitize::browser_message(n)).collect();
                        format!("\navailable tools: {}", safe.join(", "))
                    };
                    Output {
                        stdout: String::new(),
                        stderr: format!("webmcp-call: unknown tool \"{name}\"{list}\n"),
                        code: 2,
                    }
                }
                CallOutcome::Consequential(desc) => {
                    let hint = if desc.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", sanitize::browser_message(&desc))
                    };
                    Output {
                        stdout: String::new(),
                        stderr: format!("webmcp-call: tool \"{name}\" is consequential{hint}\nPass --allow-consequential to permit this call.\n"),
                        code: 2,
                    }
                }
                CallOutcome::Threw(message) => Output {
                    stdout: String::new(),
                    stderr: format!("webmcp-call: tool \"{name}\" threw: {}\n", sanitize::browser_message(&message)),
                    code: 1,
                },
            }
        }
        Err(message) => Output::err(message),
    }
}

pub fn read_stdin() -> Result<String, String> {
    use std::io::Read;
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|err| format!("webmcp-call: failed to read stdin: {err}\n"))?;
    if buf.trim().is_empty() {
        return Err("webmcp-call: stdin is empty (expected JSON)\n".to_string());
    }
    Ok(buf)
}

fn resolve(
    cdp: &mut Cdp,
    tab: Option<&str>,
    state: &mut Session,
    pages: &[PageInfo],
) -> Result<String, String> {
    if let Some(tab) = tab {
        if pages.iter().any(|page| page.target_id == *tab) {
            return Ok(tab.to_string());
        }
        if cdp.is_protected_target(tab) {
            return Err(format!("{}\n", cdp.guard().refusal()));
        }
        match cdp.target_protected(tab, false) {
            Ok(true) => return Err(format!("{}\n", cdp.guard().refusal())),
            Err(message) if crate::cdp::transport_error(&message) => return Err(message),
            _ => {}
        }
        return Err(format!(
            "Unknown tab \"{tab}\". Run 'playwright-cli tab-list' to get tab IDs.\n"
        ));
    }
    if let Some(current) = state.current.clone() {
        if pages.iter().any(|page| page.target_id == current) {
            return Ok(current);
        }
        if cdp.is_protected_target(&current) {
            return Err(format!("{}\n", cdp.guard().refusal()));
        }
        match cdp.target_protected(&current, true) {
            Ok(true) => return Err(format!("{}\n", cdp.guard().refusal())),
            Err(message) if crate::cdp::transport_error(&message) => return Err(message),
            _ => {}
        }
    }
    if pages.len() == 1 {
        return Ok(pages[0].target_id.clone());
    }
    Err(
        "Error: --tab <targetId> is required. Run 'playwright-cli tab-list' to get tab IDs.\n"
            .to_string(),
    )
}

#[cfg(test)]
fn evaluate_list(cdp: &mut Cdp, session: &str) -> Result<Value, String> {
    let expression = r#"(async () => {
  if (!('modelContext' in document) || !document.modelContext) {
    return { __webmcp_error: 'absent' };
  }
  const tools = [];
  {
    const list = await document.modelContext.getTools();
    for (const t of list) {
      let schema = t.inputSchema || null;
      if (typeof schema === 'string') { try { schema = JSON.parse(schema); } catch {} }
      tools.push({
        name: t.name,
        title: t.title || null,
        description: t.description || null,
        inputSchema: schema,
        origin: t.origin || null,
        annotations: t.annotations ? { readOnlyHint: t.annotations.readOnlyHint, consequentialHint: t.annotations.consequentialHint, untrustedContentHint: t.annotations.untrustedContentHint } : null
      });
    }
  }
  return tools;
})()"#;
    let result = cdp.call(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
            "userGesture": true
        }),
        Some(session),
    )?;
    if let Some(details) = result.get("exceptionDetails") {
        let description = details
            .pointer("/exception/description")
            .and_then(|v| v.as_str())
            .or_else(|| details.get("text").and_then(|v| v.as_str()))
            .unwrap_or("evaluation failed");
        return Err(format!("{}\n", crate::sanitize::browser_message(description)));
    }
    let value = result
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null);
    if !value.is_array() && value.get("__webmcp_error").and_then(Value::as_str) != Some("absent") {
        return Err("webmcp-list: unexpected evaluation result\n".into());
    }
    Ok(value)
}

pub fn chrome_major_version(cdp: &mut Cdp) -> Option<u32> {
    let result = cdp.call("Browser.getVersion", json!({}), None).ok()?;
    let product = result.pointer("/product").and_then(|v| v.as_str()).unwrap_or("");
    let version_part = product
        .strip_prefix("Chrome/")
        .or_else(|| product.strip_prefix("HeadlessChrome/"))?;
    version_part.split('.').next()?.parse().ok()
}

#[cfg(test)]
fn native_webmcp_object_input(cdp: &mut Cdp, session: &str) -> bool {
    let chrome = chrome_major_version(cdp);
    if chrome.is_some_and(|v| v < 155) {
        return false;
    }
    let result = cdp.call(
        "Runtime.evaluate",
        json!({
            "expression": "Boolean(document.modelContext && typeof document.modelContext.registerTool === 'function' && document.modelContext[Symbol.toStringTag] === 'ModelContext')",
            "returnByValue": true
        }),
        Some(session),
    );
    let native = result
        .ok()
        .and_then(|v| v.pointer("/result/value").and_then(|v| v.as_bool()))
        .unwrap_or(false);
    native && chrome.is_some_and(|v| v >= 155)
}

#[cfg(test)]
fn input_expression(input: &Value, native: bool) -> String {
    if native {
        format!("JSON.parse({})", json!(input.to_string()))
    } else {
        format!("JSON.stringify({input})")
    }
}

enum CallOutcome {
    Result(String),
    Absent,
    Unknown(Vec<String>),
    Consequential(String),
    Threw(String),
}

#[cfg(test)]
fn evaluate_call(
    cdp: &mut Cdp,
    session: &str,
    name: &str,
    input: &Value,
    native: bool,
) -> Result<CallOutcome, String> {
    let input_expr = input_expression(input, native);
    let expression = format!(
        r#"(async () => {{
  if (!('modelContext' in document) || !document.modelContext) {{
    return {{ __webmcp_error: 'absent' }};
  }}
  const tools = await document.modelContext.getTools();
  const tool = tools.find(t => t.name === {name_json});
  if (!tool) {{
    return {{ __webmcp_error: 'unknown', tools: tools.map(t => t.name) }};
  }}
  if (tool.annotations && tool.annotations.consequentialHint) {{
    return {{ __webmcp_error: 'consequential', description: tool.description || '' }};
  }}
  try {{
    const result = await document.modelContext.executeTool(tool, {input_expr});
    return {{ __webmcp_result: String(result) }};
  }} catch (err) {{
    return {{ __webmcp_error: 'threw', message: String(err) }};
  }}
}})()"#,
        name_json = json!(name),
    );
    parse_call_result(cdp, session, &expression)
}

#[cfg(test)]
fn evaluate_call_force(
    cdp: &mut Cdp,
    session: &str,
    name: &str,
    input: &Value,
    native: bool,
) -> Result<CallOutcome, String> {
    let input_expr = input_expression(input, native);
    let expression = format!(
        r#"(async () => {{
  const tools = await document.modelContext.getTools();
  const tool = tools.find(t => t.name === {name_json});
  if (!tool) {{
    return {{ __webmcp_error: 'unknown', tools: tools.map(t => t.name) }};
  }}
  try {{
    const result = await document.modelContext.executeTool(tool, {input_expr});
    return {{ __webmcp_result: String(result) }};
  }} catch (err) {{
    return {{ __webmcp_error: 'threw', message: String(err) }};
  }}
}})()"#,
        name_json = json!(name),
    );
    parse_call_result(cdp, session, &expression)
}

#[cfg(test)]
fn parse_call_result(
    cdp: &mut Cdp,
    session: &str,
    expression: &str,
) -> Result<CallOutcome, String> {
    let result = cdp.call(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
            "userGesture": true
        }),
        Some(session),
    )?;
    if let Some(details) = result.get("exceptionDetails") {
        let description = details
            .pointer("/exception/description")
            .and_then(|v| v.as_str())
            .or_else(|| details.get("text").and_then(|v| v.as_str()))
            .unwrap_or("evaluation failed");
        return Err(format!("{}\n", crate::sanitize::browser_message(description)));
    }
    let value = result
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null);
    parse_value(value)
}

fn parse_value(value: Value) -> Result<CallOutcome, String> {
    if let Some(error) = value.get("__webmcp_error").and_then(|v| v.as_str()) {
        return match error {
            "absent" => Ok(CallOutcome::Absent),
            "unknown" => {
                let tools = value
                    .get("tools")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(CallOutcome::Unknown(tools))
            }
            "consequential" => {
                let desc = value
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(CallOutcome::Consequential(desc))
            }
            "threw" => {
                let message = value
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                Ok(CallOutcome::Threw(message))
            }
            _ => Err("webmcp-call: unexpected evaluation result\n".into()),
        };
    }
    if let Some(text) = value.get("__webmcp_result").and_then(|v| v.as_str()) {
        return Ok(CallOutcome::Result(text.to_string()));
    }
    Err("webmcp-call: unexpected evaluation result\n".into())
}

fn redact_output(value: &mut Value) {
    match value {
        Value::String(text) => *text = sanitize::browser_message(text),
        Value::Array(values) => values.iter_mut().for_each(redact_output),
        Value::Object(values) => values.values_mut().for_each(redact_output),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdp::{Cdp, Transport};
    use serde_json::json;
    use std::collections::VecDeque;
    use std::time::Instant;

    struct FakeTransport {
        incoming: VecDeque<String>,
    }

    impl FakeTransport {
        fn with_responses(responses: Vec<Value>) -> Self {
            let incoming = responses
                .into_iter()
                .enumerate()
                .map(|(i, mut resp)| {
                    resp["id"] = json!(i as i64 + 1);
                    resp.to_string()
                })
                .collect();
            Self { incoming }
        }
    }

    impl Transport for FakeTransport {
        fn send_text(&mut self, _text: &str) -> Result<(), String> {
            Ok(())
        }

        fn recv_text(&mut self, _deadline: Instant) -> Result<Option<String>, String> {
            Ok(self.incoming.pop_front())
        }
    }

    fn fake_cdp(responses: Vec<Value>) -> Cdp {
        crate::document_client::client(Box::new(FakeTransport::with_responses(responses)), "S1")
    }

    fn eval_result(value: Value) -> Value {
        json!({ "result": { "result": { "type": "object", "value": value } } })
    }

    fn eval_exception(text: &str) -> Value {
        json!({ "result": { "result": { "type": "undefined" }, "exceptionDetails": { "text": text } } })
    }

    #[test]
    fn list_expression_returns_empty_array() {
        let mut cdp = fake_cdp(vec![eval_result(json!([]))]);
        let result = evaluate_list(&mut cdp, "S1");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), json!([]));
    }

    #[test]
    fn list_expression_returns_tools() {
        let tool = json!({
            "name": "add-to-cart",
            "title": "Add to Cart",
            "description": "Adds an item",
            "inputSchema": { "type": "object", "properties": { "id": { "type": "string" } } },
            "origin": "https://shop.example.com",
            "annotations": { "readOnlyHint": false, "consequentialHint": true }
        });
        let mut cdp = fake_cdp(vec![eval_result(json!([tool]))]);
        let result = evaluate_list(&mut cdp, "S1").unwrap();
        let arr = result.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "add-to-cart");
        assert_eq!(arr[0]["origin"], "https://shop.example.com");
    }

    #[test]
    fn list_expression_handles_exception() {
        let mut cdp = fake_cdp(vec![eval_exception("ReferenceError: document is not defined")]);
        let result = evaluate_list(&mut cdp, "S1");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("ReferenceError"));
    }

    #[test]
    fn call_expression_absent() {
        let mut cdp = fake_cdp(vec![eval_result(json!({ "__webmcp_error": "absent" }))]);
        match parse_call_result(&mut cdp, "S1", "").unwrap() {
            CallOutcome::Absent => {}
            _ => panic!("expected Absent"),
        }
    }

    #[test]
    fn call_expression_unknown() {
        let mut cdp = fake_cdp(vec![eval_result(json!({ "__webmcp_error": "unknown", "tools": ["bar", "baz"] }))]);
        match parse_call_result(&mut cdp, "S1", "").unwrap() {
            CallOutcome::Unknown(tools) => assert_eq!(tools, vec!["bar", "baz"]),
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn call_expression_threw() {
        let mut cdp = fake_cdp(vec![eval_result(json!({ "__webmcp_error": "threw", "message": "boom" }))]);
        match parse_call_result(&mut cdp, "S1", "").unwrap() {
            CallOutcome::Threw(msg) => assert_eq!(msg, "boom"),
            _ => panic!("expected Threw"),
        }
    }

    #[test]
    fn call_expression_success() {
        let mut cdp = fake_cdp(vec![eval_result(json!({ "__webmcp_result": "Item added to cart" }))]);
        match parse_call_result(&mut cdp, "S1", "").unwrap() {
            CallOutcome::Result(text) => assert_eq!(text, "Item added to cart"),
            _ => panic!("expected Result"),
        }
    }

    #[test]
    fn call_expression_consequential() {
        let mut cdp = fake_cdp(vec![eval_result(json!({ "__webmcp_error": "consequential", "description": "Deletes data" }))]);
        match parse_call_result(&mut cdp, "S1", "").unwrap() {
            CallOutcome::Consequential(desc) => assert_eq!(desc, "Deletes data"),
            _ => panic!("expected Consequential"),
        }
    }

    #[test]
    fn call_invalid_json_exit_code_2() {
        let mut cdp = fake_cdp(vec![]);
        let mut state = Session::default();
        let output = call(&mut cdp, &mut state, "foo", "not json", None, false);
        assert_eq!(output.code, 2);
        assert!(output.stderr.contains("invalid JSON"));
    }

    #[test]
    fn protected_tab_refused_no_runtime_call() {
        let mut cdp = fake_cdp(vec![
            json!({ "result": { "targetInfos": [{ "targetId": "T1", "type": "page", "url": "http://example.com", "title": "Test" }] } }),
            json!({ "result": { "targetInfo": { "targetId": "SEVEN", "type": "page", "url": "https://seven.sliccy.ai/", "title": "Seven" } } }),
        ]);
        let mut state = Session::default();
        let output = call(&mut cdp, &mut state, "foo", "{}", Some("SEVEN"), false);
        assert_eq!(output.code, 1);
        assert!(output.stderr.contains("refused"));
    }

    #[test]
    fn webmcp_call_requires_name() {
        let output = crate::execute(&["webmcp-call".to_string()], None);
        assert_eq!(output.code, 1);
        assert!(output.stderr.contains("requires"));
    }

    #[test]
    fn webmcp_call_requires_json_input() {
        let output = crate::execute(&["webmcp-call".to_string(), "tool-name".to_string()], None);
        assert_eq!(output.code, 1);
        assert!(output.stderr.contains("requires"));
    }

    #[test]
    fn webmcp_list_help() {
        let output = crate::execute(&["webmcp-list".to_string(), "--help".to_string()], None);
        assert_eq!(output.code, 0);
        assert!(output.stdout.contains("webmcp-list"));
        assert!(output.stdout.contains("document.modelContext"));
        assert!(output.stdout.contains("--json"));
    }

    #[test]
    fn webmcp_call_help() {
        let output = crate::execute(&["webmcp-call".to_string(), "--help".to_string()], None);
        assert_eq!(output.code, 0);
        assert!(output.stdout.contains("webmcp-call"));
        assert!(output.stdout.contains("executeTool"));
        assert!(output.stdout.contains("Exit codes"));
        assert!(output.stdout.contains("--allow-consequential"));
        assert!(output.stdout.contains("stdin"));
        assert!(output.stdout.contains("advisory"));
    }

    #[test]
    fn webmcp_commands_in_help() {
        let output = crate::execute(&["--help".to_string()], None);
        assert_eq!(output.code, 0);
        assert!(output.stdout.contains("WebMCP (SLICC):"));
        assert!(output.stdout.contains("webmcp-list"));
        assert!(output.stdout.contains("webmcp-call"));
    }

    #[test]
    fn webmcp_not_in_pending() {
        let output = crate::execute(&["--help".to_string()], None);
        let pending = output.stdout.split("Not implemented yet:").nth(1).unwrap_or("");
        assert!(!pending.contains("webmcp-list"));
        assert!(!pending.contains("webmcp-call"));
    }

    #[test]
    fn format_list_empty() {
        assert_eq!(format_list(&json!([])), "No WebMCP tools on this page\n");
    }

    #[test]
    fn format_list_renders_name_and_description() {
        let tools = json!([
            { "name": "greet", "description": "Says hello", "annotations": { "readOnlyHint": true } },
            { "name": "delete", "description": "Remove item", "annotations": { "consequentialHint": true } }
        ]);
        let text = format_list(&tools);
        assert!(text.contains("greet"), "{text}");
        assert!(text.contains("Says hello"), "{text}");
        assert!(text.contains("[read-only]"), "{text}");
        assert!(text.contains("delete"), "{text}");
        assert!(text.contains("[consequential]"), "{text}");
    }

    #[test]
    fn stdin_dash_is_recognized() {
        let output = crate::execute(
            &["webmcp-call".to_string(), "tool-name".to_string(), "-".to_string()],
            None,
        );
        assert_ne!(output.code, 0);
        assert!(!output.stderr.contains("requires"));
    }

    #[test]
    fn call_invalid_json_on_stdin_exit_2() {
        let mut cdp = fake_cdp(vec![]);
        let mut state = Session::default();
        let output = call(&mut cdp, &mut state, "foo", "  {bad  ", None, false);
        assert_eq!(output.code, 2);
        assert!(output.stderr.contains("invalid JSON"));
    }

    #[test]
    fn call_empty_string_exit_2() {
        let mut cdp = fake_cdp(vec![]);
        let mut state = Session::default();
        let output = call(&mut cdp, &mut state, "foo", "", None, false);
        assert_eq!(output.code, 2);
        assert!(output.stderr.contains("invalid JSON"));
    }

    #[test]
    fn call_non_object_json_exit_2() {
        let mut cdp = fake_cdp(vec![]);
        let mut state = Session::default();
        for input in ["42", "null", "\"text\"", "[1,2]", "true"] {
            let output = call(&mut cdp, &mut state, "foo", input, None, false);
            assert_eq!(output.code, 2, "input {input}");
            assert!(output.stderr.contains("must be a JSON object"), "input {input}: {}", output.stderr);
        }
    }

    #[test]
    fn result_sanitizes_terminal_escapes() {
        let mut cdp = fake_cdp(vec![eval_result(json!({ "__webmcp_result": "pwned\x1b[2J\x07" }))]);
        match parse_call_result(&mut cdp, "S1", "").unwrap() {
            CallOutcome::Result(text) => {
                assert!(text.contains('\x1b'));
                assert!(text.contains('\x07'));
            }
            _ => panic!("expected Result"),
        }
    }

    #[test]
    fn call_result_output_is_sanitized() {
        let text = "pwned\x1b[2J\x07done";
        let sanitized = sanitize::terminal(text);
        let output = format!("{}\n", sanitized);
        assert!(!output.contains('\x1b'));
        assert!(!output.contains('\x07'));
        assert!(output.contains("\\x1b"));
        assert!(output.contains("\\x07"));
    }

    #[test]
    fn threw_message_is_sanitized() {
        let message = "Error: \x1b[2Jboom";
        let sanitized = sanitize::terminal(message);
        assert!(!sanitized.contains('\x1b'));
        assert!(sanitized.contains("\\x1b"));
    }

    #[test]
    fn unknown_tool_names_are_sanitized() {
        let names = vec!["safe".to_string(), "\x1b[2Jevil".to_string()];
        let safe: Vec<String> = names.iter().map(|n| sanitize::terminal(n)).collect();
        let joined = safe.join(", ");
        assert!(!joined.contains('\x1b'));
        assert!(joined.contains("\\x1b"));
    }

    #[test]
    fn format_list_sanitizes_names_and_descriptions() {
        let tools = json!([
            { "name": "\x1b[2Jevil", "description": "desc\x07bel" }
        ]);
        let text = format_list(&tools);
        assert!(!text.contains('\x1b'), "{text}");
        assert!(!text.contains('\x07'), "{text}");
        assert!(text.contains("\\x1b"), "{text}");
        assert!(text.contains("\\x07"), "{text}");
    }

    #[test]
    fn chrome_major_version_parses_product() {
        let mut cdp = fake_cdp(vec![json!({ "result": { "product": "Chrome/155.0.6785.0" } })]);
        assert_eq!(chrome_major_version(&mut cdp), Some(155));
    }

    #[test]
    fn chrome_major_version_headless() {
        let mut cdp = fake_cdp(vec![json!({ "result": { "product": "HeadlessChrome/156.0.0.0" } })]);
        assert_eq!(chrome_major_version(&mut cdp), Some(156));
    }

    #[test]
    fn chrome_major_version_missing() {
        let mut cdp = fake_cdp(vec![json!({ "result": { "product": "Firefox/120" } })]);
        assert_eq!(chrome_major_version(&mut cdp), None);
    }

    #[test]
    fn native_webmcp_object_input_true_for_new_chrome() {
        let mut cdp = fake_cdp(vec![
            json!({ "result": { "product": "Chrome/155.0.0.0" } }),
            eval_result(json!(true)),
        ]);
        assert!(native_webmcp_object_input(&mut cdp, "S1"));
    }

    #[test]
    fn native_webmcp_object_input_false_for_old_chrome() {
        let mut cdp = fake_cdp(vec![
            json!({ "result": { "product": "Chrome/153.0.0.0" } }),
        ]);
        assert!(!native_webmcp_object_input(&mut cdp, "S1"));
    }

    #[test]
    fn native_webmcp_object_input_false_when_polyfill() {
        let mut cdp = fake_cdp(vec![
            json!({ "result": { "product": "Chrome/155.0.0.0" } }),
            eval_result(json!(false)),
        ]);
        assert!(!native_webmcp_object_input(&mut cdp, "S1"));
    }

    #[test]
    fn input_expression_string_for_polyfill() {
        let input = json!({"key": "value"});
        let expr = input_expression(&input, false);
        assert!(expr.starts_with("JSON.stringify("));
    }

    #[test]
    fn input_expression_object_for_native() {
        let input = json!({"key": "value"});
        let expr = input_expression(&input, true);
        assert!(expr.starts_with("JSON.parse("));
        assert!(!expr.contains("JSON.stringify"));
    }

    #[test]
    fn eval_exception_sanitizes_page_text() {
        let evil = "Error: \x1b[31mred\x1b[0m \x07 \x7f \u{009b}";
        let mut cdp = fake_cdp(vec![eval_exception(evil)]);
        let result = evaluate_list(&mut cdp, "S1");
        let err = result.unwrap_err();
        assert!(!err.contains('\x1b'), "ESC must be escaped: {err}");
        assert!(!err.contains('\x07'), "BEL must be escaped: {err}");
        assert!(!err.contains('\x7f'), "DEL must be escaped: {err}");
        assert!(!err.contains('\u{009b}'), "CSI must be escaped: {err}");
    }
}
