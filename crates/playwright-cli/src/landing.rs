use crate::cdp::Cdp;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const SOURCE: &str = include_str!("landing.js");
const WORLD: &str = "slicc-landing";
const GROUP: &str = "slicc-landing";
const FOREGROUND_WAIT: Duration = Duration::from_secs(3);
const READ_WAIT: Duration = Duration::from_secs(3);
pub const NOT_EDITABLE: &str = "Error: type needs a focused editable element, and none is focused. Focus one first, for example with click or fill.\n";

pub struct Scope<'a> {
    pub session: &'a str,
    pub frame: Option<&'a str>,
    pub input: &'a str,
}

impl<'a> Scope<'a> {
    pub fn page(session: &'a str) -> Self {
        Scope { session, frame: None, input: session }
    }
}

pub enum Anchor<'a> {
    Backend(u64),
    Selector(&'a str),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Gesture<'a> {
    Click { button: &'a str, modifiers: i64 },
    Double { button: &'a str, modifiers: i64 },
    Hover,
}

impl Gesture<'_> {
    fn verb(&self) -> &'static str {
        match self {
            Gesture::Click { .. } => "click",
            Gesture::Double { .. } => "dblclick",
            Gesture::Hover => "hover",
        }
    }

    fn parts(&self) -> (&str, i64) {
        match self {
            Gesture::Click { button, modifiers } | Gesture::Double { button, modifiers } => (button, *modifiers),
            Gesture::Hover => ("none", 0),
        }
    }

    fn steps(&self) -> Vec<(&'static str, i64)> {
        let finish = match self.parts().0 {
            "right" => "contextmenu",
            "middle" => "auxclick",
            _ => "click",
        };
        let once = [("mousedown", 1), ("mouseup", 1), (finish, 1)];
        match self {
            Gesture::Hover => vec![("mousemove", 0)],
            Gesture::Click { .. } => once.to_vec(),
            Gesture::Double { .. } => {
                let mut steps = once.to_vec();
                steps.extend([("mousedown", 2), ("mouseup", 2), (finish, 2)]);
                if finish == "click" {
                    steps.push(("dblclick", 2));
                }
                steps
            }
        }
    }
}

pub struct Keyed<'k> {
    pub key: &'k str,
    pub code: &'k str,
    pub text: Option<&'k str>,
    pub modifiers: i64,
}

pub type Trusted<'t> = &'t mut dyn FnMut(&mut Cdp) -> Result<(), String>;

fn landing() -> &'static str {
    SOURCE.trim_end().trim_end_matches(';')
}

fn numeral(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { format!("{value}") }
}

pub fn frame_refusal(x: f64, y: f64) -> String {
    format!("slicc: refused: mouse input at ({}, {}) would reach an embedded frame\n", numeral(x), numeral(y))
}

pub fn background(verb: &str) -> String {
    format!("Error: {verb} could not reach the background tab in place, and brings it to the front only when the tab that was in front can be switched back to. Run \"playwright-cli tab-select <index> --foreground\" first.\n")
}

pub fn dropped(verb: &str) -> String {
    format!("Error: {verb} did not reach the page: Chrome dropped the input even with the tab in front.\n")
}

fn context(cdp: &mut Cdp, scope: &Scope) -> Result<Value, String> {
    let frame = match scope.frame {
        Some(frame) => frame.to_string(),
        None => {
            let tree = cdp.call("Page.getFrameTree", json!({}), Some(scope.session))?;
            tree.pointer("/frameTree/frame/id").and_then(Value::as_str).map(str::to_string).ok_or("Page.getFrameTree returned no main frame\n")?
        }
    };
    let world = cdp.call("Page.createIsolatedWorld", json!({ "frameId": frame, "worldName": WORLD }), Some(scope.session))?;
    world.get("executionContextId").cloned().ok_or_else(|| "Page.createIsolatedWorld returned no executionContextId\n".to_string())
}

fn settle(result: Value) -> Result<Value, String> {
    match result.get("exceptionDetails") {
        Some(details) => {
            let text = details.pointer("/exception/description").or_else(|| details.get("text")).and_then(Value::as_str).unwrap_or("evaluation failed");
            Err(format!("{}\n", crate::sanitize::browser_message(text)))
        }
        None => Ok(result.pointer("/result/value").cloned().unwrap_or(Value::Null)),
    }
}

fn run(cdp: &mut Cdp, scope: &Scope, context: &Value, mode: &str, args: Value, wait: Duration) -> Result<Value, String> {
    let expression = format!("{}({}, {args})", landing(), json!(mode));
    settle(cdp.call_for("Runtime.evaluate", json!({ "expression": expression, "contextId": context, "returnByValue": true }), Some(scope.session), wait)?)
}

fn mode(cdp: &mut Cdp, scope: &Scope, context: &Value, mode: &str, args: Value) -> Result<String, String> {
    Ok(run(cdp, scope, context, mode, args, Duration::from_secs(30))?.as_str().unwrap_or("").to_string())
}

fn pointer_on(cdp: &mut Cdp, scope: &Scope, context: &Value, anchor: &Anchor, args: Value) -> Result<String, String> {
    let value = match anchor {
        Anchor::Selector(selector) => {
            let mut args = args;
            args["selector"] = json!(selector);
            run(cdp, scope, context, "pointer", args, Duration::from_secs(30))?
        }
        Anchor::Backend(backend) => {
            let resolved = cdp.call("DOM.resolveNode", json!({ "backendNodeId": backend, "executionContextId": context, "objectGroup": GROUP }), Some(scope.session))?;
            let object = resolved.pointer("/object/objectId").and_then(Value::as_str).ok_or_else(|| format!("could not resolve backend node {backend}\n"))?.to_string();
            let called = cdp.call(
                "Runtime.callFunctionOn",
                json!({ "objectId": object, "functionDeclaration": landing(), "arguments": [{ "value": "pointer" }, { "value": args }], "returnByValue": true }),
                Some(scope.session),
            );
            let _ = cdp.call("Runtime.releaseObjectGroup", json!({ "objectGroup": GROUP }), Some(scope.session));
            settle(called?)?
        }
    };
    Ok(value.as_str().unwrap_or("").to_string())
}

fn landed(cdp: &mut Cdp, scope: &Scope, context: &Value, types: &[&str], trusted: Trusted) -> Result<bool, String> {
    if mode(cdp, scope, context, "arm", json!({ "types": types }))? != "armed" {
        return Err("Error: the input check could not start in the page.\n".to_string());
    }
    trusted(cdp)?;
    match run(cdp, scope, context, "read", json!({}), READ_WAIT) {
        Ok(Value::Array(seen)) => Ok(!seen.is_empty()),
        _ => Ok(true),
    }
}

fn visible(cdp: &mut Cdp, scope: &Scope, context: &Value, waited: Duration) -> bool {
    let deadline = Instant::now() + waited;
    loop {
        if mode(cdp, scope, context, "visible", json!({})).as_deref() == Ok("visible") {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn forward(cdp: &mut Cdp, scope: &Scope, context: &Value, verb: &str, types: Option<&[&str]>, trusted: Trusted) -> Result<(), String> {
    if visible(cdp, scope, context, Duration::ZERO) {
        return if types.is_none() { trusted(cdp) } else { Err(dropped(verb)) };
    }
    let front = cdp.bring_forward(scope.input).map_err(|_| background(verb))?;
    let result = if !visible(cdp, scope, context, FOREGROUND_WAIT) {
        Err(background(verb))
    } else if let Some(types) = types {
        landed(cdp, scope, context, types, trusted).and_then(|landed| if landed { Ok(()) } else { Err(dropped(verb)) })
    } else {
        trusted(cdp)
    };
    let restored = cdp.restore_front(front);
    result.and(restored)
}

fn types(steps: &[(&'static str, i64)]) -> Vec<&'static str> {
    let mut types: Vec<&'static str> = Vec::new();
    for (kind, _) in steps {
        if !types.contains(kind) {
            types.push(kind);
        }
    }
    types
}

pub fn pointer(cdp: &mut Cdp, scope: &Scope, anchor: &Anchor, point: (f64, f64), gesture: Gesture, trusted: Trusted) -> Result<(), String> {
    let context = context(cdp, scope)?;
    let steps = gesture.steps();
    let kinds = types(&steps);
    if landed(cdp, scope, &context, &kinds, trusted)? {
        return Ok(());
    }
    let (button, modifiers) = gesture.parts();
    let args = json!({
        "x": point.0,
        "y": point.1,
        "steps": steps.iter().map(|(kind, detail)| json!([kind, detail])).collect::<Vec<_>>(),
        "button": match button { "middle" => 1, "right" => 2, _ => 0 },
        "buttons": match button { "left" => 1, "right" => 2, "middle" => 4, _ => 0 },
        "modifiers": modifiers,
    });
    match pointer_on(cdp, scope, &context, anchor, args).as_deref() {
        Ok("done") => Ok(()),
        Ok("frame") => Err(frame_refusal(point.0, point.1)),
        _ => forward(cdp, scope, &context, gesture.verb(), Some(&kinds), trusted),
    }
}

pub fn text(cdp: &mut Cdp, session: &str, text: &str, trusted: Trusted) -> Result<(), String> {
    let scope = Scope::page(session);
    let context = context(cdp, &scope)?;
    match mode(cdp, &scope, &context, "focus", json!({}))?.as_str() {
        "frame" => return forward(cdp, &scope, &context, "type", None, trusted),
        "editable" => {}
        _ => return Err(NOT_EDITABLE.to_string()),
    }
    let kinds = ["beforeinput", "input"];
    if landed(cdp, &scope, &context, &kinds, trusted)? || mode(cdp, &scope, &context, "insert", json!({ "text": text }))? == "inserted" {
        return Ok(());
    }
    forward(cdp, &scope, &context, "type", Some(&kinds), trusted)
}

pub fn key(cdp: &mut Cdp, session: &str, keyed: &Keyed, trusted: Trusted) -> Result<(), String> {
    let scope = Scope::page(session);
    let context = context(cdp, &scope)?;
    if mode(cdp, &scope, &context, "focus", json!({}))? == "frame" {
        return forward(cdp, &scope, &context, "press", None, trusted);
    }
    let kinds = ["keydown"];
    let args = json!({ "key": keyed.key, "code": keyed.code, "text": keyed.text, "modifiers": keyed.modifiers });
    if landed(cdp, &scope, &context, &kinds, trusted)? || mode(cdp, &scope, &context, "key", args)? == "pressed" {
        return Ok(());
    }
    forward(cdp, &scope, &context, "press", Some(&kinds), trusted)
}
