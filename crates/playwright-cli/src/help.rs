use crate::spec::{self, Spec};

#[cfg(test)]
pub const UPSTREAM_PLAYWRIGHT_CLI: &str = "0.1.22";

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
        "Core:".to_string(),
        row("open [url]", "open the browser"),
        row("close", "close the browser"),
        row("goto <url>", "navigate to a url"),
        row("type <text>", "type text into editable element"),
        row("click <target> [button]", "perform click on a web page"),
        row("fill <target> <text>", "fill text into editable element"),
        row("snapshot [target]", "capture page snapshot to obtain element ref"),
        row("eval <func> [target]", "evaluate javascript expression on page or element"),
        String::new(),
        "Keyboard:".to_string(),
        row("press <key>", "press a key on the keyboard, `a`, `arrowleft`"),
        String::new(),
        "Save as:".to_string(),
        row("screenshot [target]", "screenshot of the current page or element"),
        String::new(),
        "Tabs:".to_string(),
        row("tab-list", "list all tabs"),
        row("tab-new [url]", "create a new tab"),
        row("tab-close [index]", "close a browser tab"),
        row("tab-select <index>", "select a browser tab"),
        String::new(),
        "Console and network:".to_string(),
        row("console [min-level]", "list console messages"),
        row("requests", "list network requests"),
        row("request <index>", "show one network request"),
        row("request-headers <index>", "print request headers"),
        row("request-body <index>", "print the request body"),
        row("response-headers <index>", "print response headers"),
        row("response-body <index>", "print the response body"),
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
    lines.push(row(
        "--cdp <url>",
        "WebSocket URL, or an http URL used for /json/version",
    ));
    lines.push(row(
        "--runtime <name>",
        "query parameter on the URL that is opened",
    ));
    lines.push(row("-h, --help", "show help"));
    lines.push(row("-V, --version", "print the version"));
    lines.join("\n")
}

fn row(name: &str, text: &str) -> String {
    format!("  {name:<28}{text}")
}

pub fn command_help(spec: &Spec) -> String {
    if let Some(page) = implemented_page(spec.name) {
        return page;
    }
    let usage = usage_line(spec);
    let body = if !spec.implemented {
        "Not implemented yet."
    } else {
        ""
    };
    format!("playwright-cli {usage}\n\n{body}\n")
}

fn implemented_page(name: &str) -> Option<String> {
    let page = match name {
        "open" => page(
            "open [url]",
            &[
                "Open the browser",
                "",
                "This CLI does not launch a browser. It creates a tab on the attached endpoint.",
            ],
            &[("[url]", "the url to navigate to")],
            &[
                ("--browser", "not implemented"),
                ("--config", "not implemented"),
                ("--device", "not implemented"),
                ("--headed", "not implemented"),
                ("--idle-timeout", "not implemented"),
                ("--mobile", "not implemented"),
                ("--persistent", "not implemented"),
                ("--profile", "not implemented"),
                ("--foreground", "activate the new tab"),
                ("--fg", "activate the new tab"),
                ("--runtime <name>", "query parameter on the CDP URL"),
            ],
        ),
        "close" => page(
            "close ",
            &[
                "Close the browser",
                "",
                "This CLI does not stop a browser session. It closes the current tab.",
            ],
            &[],
            &[("--tab <targetId>", "tab to close (defaults to the current tab)")],
        ),
        "goto" => page(
            "goto <url>",
            &["Navigate to a URL"],
            &[("<url>", "the url to navigate to")],
            &[("--tab <targetId>", "tab to navigate")],
        ),
        "type" => page(
            "type <text>",
            &["Type text into editable element"],
            &[("<text>", "text to type into the element")],
            &[
                ("--submit", "whether to submit entered text (press enter after)"),
                ("--tab <targetId>", "tab to type into"),
            ],
        ),
        "click" => page(
            "click <target> [button]",
            &["Perform click on a web page"],
            &[
                (
                    "<target>",
                    "exact target element reference from the page snapshot, or a unique element selector",
                ),
                ("[button]", "button to click, defaults to left"),
            ],
            &[
                ("--modifiers", "modifier key to press (repeatable)"),
                ("--tab <targetId>", "tab to click"),
            ],
        ),
        "fill" => page(
            "fill <target> <text>",
            &["Fill text into editable element"],
            &[
                (
                    "<target>",
                    "exact target element reference from the page snapshot, or a unique element selector",
                ),
                ("<text>", "text to fill into the element"),
            ],
            &[
                ("--submit", "whether to submit entered text (press enter after)"),
                ("--tab <targetId>", "tab to fill"),
            ],
        ),
        "press" => page(
            "press <key>",
            &["Press a key on the keyboard, `a`, `ArrowLeft`"],
            &[(
                "<key>",
                "name of the key to press or a character to generate, such as `arrowleft` or `a`",
            )],
            &[("--tab <targetId>", "tab to press")],
        ),
        "snapshot" => page(
            "snapshot [target]",
            &["Capture page snapshot to obtain element ref"],
            &[("[target]", "not implemented")],
            &[
                (
                    "--filename",
                    "save snapshot to markdown file instead of returning it in the response.",
                ),
                ("--depth", "not implemented"),
                ("--boxes", "not implemented"),
                ("--frame <frameId>", "snapshot one frame"),
                ("--no-iframes", "skip child frames"),
                ("--tab <targetId>", "tab to snapshot"),
            ],
        ),
        "eval" => page(
            "eval <func> [target]",
            &["Evaluate JavaScript expression on page or element"],
            &[
                (
                    "<func>",
                    "() => { /* code */ } or (element) => { /* code */ } when element is provided",
                ),
                ("[target]", "not implemented"),
            ],
            &[
                (
                    "--filename",
                    "save evaluation result to a file instead of returning it in the response.",
                ),
                ("--output <path>", "write the result to a file when --filename is omitted"),
                ("--frame <frameId>", "evaluate in one frame"),
                ("--tab <targetId>", "tab to evaluate"),
            ],
        ),
        "screenshot" => page(
            "screenshot [target]",
            &["screenshot of the current page or element"],
            &[(
                "[target]",
                "exact target element reference from the page snapshot, or a unique element selector",
            )],
            &[
                (
                    "--filename",
                    "file name to save the screenshot to. defaults to screenshot-{timestamp}.png if not specified.",
                ),
                ("--type", "not implemented"),
                (
                    "--full-page",
                    "when true, takes a screenshot of the full scrollable page, instead of the currently visible viewport.",
                ),
                ("--hires", "not implemented"),
                ("--max-width <px>", "limit the image width in pixels"),
                ("--tab <targetId>", "tab to capture"),
            ],
        ),
        "tab-list" => "playwright-cli tab-list \n\nList all tabs\n\n".to_string(),
        "tab-new" => page(
            "tab-new [url]",
            &["Create a new tab"],
            &[(
                "[url]",
                "the url to navigate to in the new tab. if omitted, the new tab will be blank.",
            )],
            &[
                ("--foreground", "activate the new tab"),
                ("--fg", "activate the new tab"),
                ("--runtime <name>", "query parameter on the CDP URL"),
            ],
        ),
        "tab-select" => {
            "playwright-cli tab-select <index>\n\nSelect a browser tab\n\nArguments:\n  <index>                     tab index\n"
                .to_string()
        }
        "tab-close" => page(
            "tab-close [index]",
            &["Close a browser tab"],
            &[("[index]", "not implemented")],
            &[("--tab <targetId>", "tab to close (defaults to the current tab)")],
        ),
        "console" => page(
            "console [min-level]",
            &[
                "List console messages",
                "",
                "Levels: error, warning, info, debug. Defaults to info. Each level includes more severe levels.",
            ],
            &[("[min-level]", "minimum level to show")],
            &[
                ("--clear", "drop the stored console list for the tab"),
                ("--tab <targetId>", "tab to read"),
            ],
        ),
        "requests" => page(
            "requests",
            &["List network requests captured for the tab. Each line is numbered for request."],
            &[],
            &[
                ("--static", "include successful static resources"),
                ("--filter <text>", "substring of the request URL"),
                ("--clear", "drop the stored network list for the tab"),
                ("--tab <targetId>", "tab to read"),
            ],
        ),
        "request" => page(
            "request <index>",
            &["Show one network request by its number from requests."],
            &[("<index>", "1-based request number from requests")],
            &[
                ("--filename <path>", "write the text to a file"),
                ("--tab <targetId>", "tab to read"),
            ],
        ),
        "request-headers" => page(
            "request-headers <index>",
            &["Print request headers for one network request."],
            &[("<index>", "1-based request number from requests")],
            &[
                ("--filename <path>", "write the text to a file"),
                ("--tab <targetId>", "tab to read"),
            ],
        ),
        "request-body" => page(
            "request-body <index>",
            &["Print the request body for one network request."],
            &[("<index>", "1-based request number from requests")],
            &[
                ("--filename <path>", "write the text to a file"),
                ("--tab <targetId>", "tab to read"),
            ],
        ),
        "response-headers" => page(
            "response-headers <index>",
            &["Print response headers for one network request."],
            &[("<index>", "1-based request number from requests")],
            &[
                ("--filename <path>", "write the text to a file"),
                ("--tab <targetId>", "tab to read"),
            ],
        ),
        "response-body" => page(
            "response-body <index>",
            &[
                "Print the response body for one network request.",
                "Text bodies are printed. Binary bodies are saved and the path is printed.",
            ],
            &[("<index>", "1-based request number from requests")],
            &[
                ("--filename <path>", "write the text to a file"),
                ("--tab <targetId>", "tab to read"),
            ],
        ),
        _ => return None,
    };
    Some(page)
}

fn page(
    usage: &str,
    summary: &[&str],
    arguments: &[(&str, &str)],
    options: &[(&str, &str)],
) -> String {
    let mut lines = vec![format!("playwright-cli {usage}"), String::new()];
    lines.extend(summary.iter().map(|line| (*line).to_string()));
    if !arguments.is_empty() {
        lines.push(String::new());
        lines.push("Arguments:".to_string());
        for (name, text) in arguments {
            lines.push(row(name, text));
        }
    }
    if !options.is_empty() {
        if arguments.is_empty() {
            lines.push(String::new());
        }
        lines.push("Options:".to_string());
        for (name, text) in options {
            lines.push(row(name, text));
        }
    }
    format!("{}\n", lines.join("\n"))
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
        ("open", 0)
            | ("tab-new", 0)
            | ("screenshot", 0)
            | ("console", 0)
            | ("dialog-accept", 0)
            | ("unroute", 0)
            | ("record", 0)
            | ("highlight", 0)
    )
}
