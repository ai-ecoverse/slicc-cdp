use crate::args::Output;
use crate::browser::{self, PageInfo};
use crate::cdp::{self, Cdp};
use crate::session::Session;
use serde_json::{json, Value};

pub fn list(cdp: &mut Cdp, state: &mut Session, tab: Option<&str>) -> Output {
    let pages = match visible_pages(cdp, state) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve(cdp, tab, state, &pages) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match cdp::attach(cdp, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = evaluate_list(cdp, &session);
    cdp::detach(cdp, &session);
    match result {
        Ok(tools) => {
            state.current = Some(target);
            Output::ok(format!("{}\n", tools))
        }
        Err(message) => Output::err(message),
    }
}

pub fn call(
    cdp: &mut Cdp,
    state: &mut Session,
    name: &str,
    input_json: &str,
    tab: Option<&str>,
) -> Output {
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
    let pages = match visible_pages(cdp, state) {
        Ok(pages) => pages,
        Err(message) => return Output::err(message),
    };
    let target = match resolve(cdp, tab, state, &pages) {
        Ok(target) => target,
        Err(message) => return Output::err(message),
    };
    let session = match cdp::attach(cdp, &target) {
        Ok(session) => session,
        Err(message) => return Output::err(message),
    };
    let result = evaluate_call(cdp, &session, name, &parsed);
    cdp::detach(cdp, &session);
    match result {
        Ok(outcome) => {
            state.current = Some(target);
            match outcome {
                CallOutcome::Result(text) => Output::ok(format!("{text}\n")),
                CallOutcome::Absent => Output {
                    stdout: String::new(),
                    stderr: "webmcp-call: page does not support WebMCP (document.modelContext is absent)\n".into(),
                    code: 2,
                },
                CallOutcome::Unknown(available) => {
                    let list = if available.is_empty() {
                        String::new()
                    } else {
                        format!("\navailable tools: {}", available.join(", "))
                    };
                    Output {
                        stdout: String::new(),
                        stderr: format!("webmcp-call: unknown tool \"{name}\"{list}\n"),
                        code: 2,
                    }
                }
                CallOutcome::Threw(message) => Output {
                    stdout: String::new(),
                    stderr: format!("webmcp-call: tool \"{name}\" threw: {message}\n"),
                    code: 1,
                },
            }
        }
        Err(message) => Output::err(message),
    }
}

fn visible_pages(cdp: &mut Cdp, state: &Session) -> Result<Vec<PageInfo>, String> {
    let pages = browser::list_pages(cdp)?;
    Ok(pages
        .into_iter()
        .filter(|page| {
            crate::session::page_visible(
                state.provenance.as_deref(),
                &page.url,
                &page.target_id,
                &state.owned,
            )
        })
        .collect())
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

fn evaluate_list(cdp: &mut Cdp, session: &str) -> Result<Value, String> {
    let expression = r#"(async () => {
  const tools = [];
  if ('modelContext' in document && document.modelContext) {
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
  for (const form of document.querySelectorAll('form[toolname]')) {
    const name = form.getAttribute('toolname');
    if (tools.some(t => t.name === name)) continue;
    const fields = {};
    for (const el of form.elements) {
      if (el.name) fields[el.name] = { type: 'string' };
    }
    tools.push({
      name: name,
      title: form.getAttribute('tooldescription') || name,
      description: form.getAttribute('tooldescription') || null,
      inputSchema: Object.keys(fields).length > 0 ? { type: 'object', properties: fields } : null,
      origin: location.origin,
      annotations: null,
      declarative: true
    });
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
        return Err(format!("{description}\n"));
    }
    let value = result
        .pointer("/result/value")
        .cloned()
        .unwrap_or(json!([]));
    Ok(value)
}

enum CallOutcome {
    Result(String),
    Absent,
    Unknown(Vec<String>),
    Threw(String),
}

fn evaluate_call(
    cdp: &mut Cdp,
    session: &str,
    name: &str,
    input: &Value,
) -> Result<CallOutcome, String> {
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
  try {{
    const input = {input_json};
    const result = await document.modelContext.executeTool(tool, typeof input === 'string' ? input : JSON.stringify(input));
    return {{ __webmcp_result: String(result) }};
  }} catch (err) {{
    return {{ __webmcp_error: 'threw', message: String(err) }};
  }}
}})()"#,
        name_json = json!(name),
        input_json = input
    );
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
        return Err(format!("{description}\n"));
    }
    let value = result
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null);
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
        Cdp::from_transport(Box::new(FakeTransport::with_responses(responses)))
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
        match evaluate_call(&mut cdp, "S1", "foo", &json!({})).unwrap() {
            CallOutcome::Absent => {}
            _ => panic!("expected Absent"),
        }
    }

    #[test]
    fn call_expression_unknown() {
        let mut cdp = fake_cdp(vec![eval_result(json!({ "__webmcp_error": "unknown", "tools": ["bar", "baz"] }))]);
        match evaluate_call(&mut cdp, "S1", "foo", &json!({})).unwrap() {
            CallOutcome::Unknown(tools) => assert_eq!(tools, vec!["bar", "baz"]),
            _ => panic!("expected Unknown"),
        }
    }

    #[test]
    fn call_expression_threw() {
        let mut cdp = fake_cdp(vec![eval_result(json!({ "__webmcp_error": "threw", "message": "boom" }))]);
        match evaluate_call(&mut cdp, "S1", "foo", &json!({})).unwrap() {
            CallOutcome::Threw(msg) => assert_eq!(msg, "boom"),
            _ => panic!("expected Threw"),
        }
    }

    #[test]
    fn call_expression_success() {
        let mut cdp = fake_cdp(vec![eval_result(json!({ "__webmcp_result": "Item added to cart" }))]);
        match evaluate_call(&mut cdp, "S1", "add-to-cart", &json!({"id": "123"})).unwrap() {
            CallOutcome::Result(text) => assert_eq!(text, "Item added to cart"),
            _ => panic!("expected Result"),
        }
    }

    #[test]
    fn call_invalid_json_exit_code_2() {
        let mut cdp = fake_cdp(vec![]);
        let mut state = Session::default();
        let output = call(&mut cdp, &mut state, "foo", "not json", None);
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
        let output = call(&mut cdp, &mut state, "foo", "{}", Some("SEVEN"));
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
}
