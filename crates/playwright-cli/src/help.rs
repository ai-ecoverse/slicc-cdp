use crate::spec::{self, Spec};

pub fn global_help() -> String {
    let mut lines = vec![
        "playwright-cli - run playwright mcp commands from terminal".to_string(),
        String::new(),
        "Usage: playwright-cli <command> [args] [options]".to_string(),
        "Usage: playwright-cli --cdp=<url> <command> [args] [options]".to_string(),
        String::new(),
        "Connect, in order: --cdp <url> wins; else SLICC_CDP_URL; else GET http://127.0.0.1:9222/json/version and dial webSocketDebuggerUrl.".to_string(),
        "ws:// and wss:// are dialed directly. An http:// URL is /json/version discovery. The debugger path is opaque.".to_string(),
        "--runtime <name> is appended to the query of the URL that is opened. It is not interpreted.".to_string(),
        "This CLI does not launch Chrome.".to_string(),
        String::new(),
        "Commands:".to_string(),
        row("open [url]", "create a tab on the CDP endpoint"),
        row("close", "close the current tab"),
        row("goto <url>", "navigate the current tab"),
        row("type <text>", "type text into the focused element"),
        row("click <ref> [button]", "click an element ref from the snapshot"),
        row("fill <ref> <text>", "fill an element ref"),
        row("press <key>", "press a key"),
        row("snapshot", "capture an ARIA snapshot with element refs"),
        row("eval <expression>", "evaluate JavaScript in the page"),
        row("screenshot [ref]", "capture a PNG of the page or an element"),
        row("tab-list", "list open tabs"),
        row("tab-new [url]", "create a tab"),
        row("tab-select <index>", "activate a tab by 1-based index"),
        row("tab-close", "close a tab"),
        String::new(),
        "Not implemented yet:".to_string(),
    ];
    let pending: Vec<&str> = spec::all()
        .iter()
        .filter(|spec| !spec.implemented)
        .map(|spec| spec.name)
        .collect();
    lines.push(format!("  {}", pending.join(", ")));
    lines.push(String::new());
    lines.push("Options:".to_string());
    lines.push(row("--cdp <url>", "WebSocket URL, or an http URL used for /json/version"));
    lines.push(row("--runtime <name>", "query parameter on the URL that is opened"));
    lines.push(row("-h, --help", "show help"));
    lines.join("\n")
}

fn row(name: &str, text: &str) -> String {
    format!("  {name:<28} {text}")
}

pub fn command_help(spec: &Spec) -> String {
    let usage = usage_line(spec);
    let body = match spec.name {
        "open" | "tab-new" => "Create a tab with Target.createTarget. Does not launch Chrome.\n--foreground, --fg    activate the new tab\n--runtime <name>      query parameter on the CDP URL",
        "close" | "tab-close" => "Close a tab with Target.closeTarget.\n--tab <targetId>      tab to close (defaults to the current tab)",
        "goto" => "Navigate the current tab with Page.navigate.\n--tab <targetId>",
        "type" => "Type text with Input.insertText.\n--submit              press Enter after typing\n--tab <targetId>",
        "click" => "Click a snapshot ref.\nclick <ref> [button]  button is left, right, or middle\n--modifiers <list>    Alt,Control,Meta,Shift\n--tab <targetId>",
        "fill" => "Fill a snapshot ref.\n--submit              press Enter after filling\n--tab <targetId>",
        "press" => "Press a key with Input.dispatchKeyEvent.\n--tab <targetId>",
        "snapshot" => "Print an ARIA snapshot. Refs look like e1, e2, and f1e5 inside iframes.\n--filename <path>     write the snapshot to a file\n--frame <frameId>     snapshot one frame\n--no-iframes          skip child frames\n--tab <targetId>",
        "eval" => "Evaluate JavaScript with Runtime.evaluate.\n--filename <path>     write the result to a file\n--output <path>       same, if --filename is omitted\n--frame <frameId>\n--tab <targetId>",
        "screenshot" => "Capture a PNG with Page.captureScreenshot.\n--filename <path>\n--full-page           capture beyond the viewport\n--max-width <px>\n--tab <targetId>",
        "tab-list" => "List actionable page targets from Target.getTargets.\nEach line starts with its 1-based index for tab-select.",
        "tab-select" => "Activate a tab with Target.activateTarget.\ntab-select <index>    index from tab-list",
        _ if !spec.implemented => "Not implemented yet.",
        _ => "",
    };
    format!("playwright-cli {usage}\n\n{body}\n")
}

fn usage_line(spec: &Spec) -> String {
    let mut parts = vec![spec.name.to_string()];
    for (index, arg) in spec.args.iter().enumerate() {
        let token = if spec.variadic && index + 1 == spec.args.len() {
            format!("<{arg}...>")
        } else if arg_optional(spec, index) {
            format!("[{arg}]")
        } else {
            format!("<{arg}>")
        };
        parts.push(token);
    }
    parts.join(" ")
}

fn arg_optional(spec: &Spec, index: usize) -> bool {
    matches!(
        (spec.name, index),
        ("open", 0) | ("tab-new", 0) | ("screenshot", 0) | ("dialog-accept", 0) | ("unroute", 0) | ("record", 0) | ("highlight", 0)
    )
}
