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
            &["Open the browser"],
            &[("[url]", "the url to navigate to")],
            &[
                ("--browser", "browser or chrome channel to use, possible values: chrome, firefox, webkit, msedge."),
                ("--config", "path to the configuration file, defaults to .playwright/cli.config.json"),
                ("--device", "emulate a specific device, for example \"iphone 15\"."),
                ("--headed", "run browser in headed mode"),
                ("--idle-timeout", "shut the session down after this many milliseconds without a command. defaults to one hour for headless browsers, never for headed ones. pass 0 to disable."),
                ("--mobile", "emulate a generic mobile device (pixel 10 for chromium, iphone 17 for webkit). mobile pages are usually lighter, which saves tokens."),
                ("--persistent", "use persistent browser profile"),
                ("--profile", "path to a persistent user data directory."),
            ],
        ),
        "close" => page("close ", &["Close the browser"], &[], &[]),
        "goto" => page(
            "goto <url>",
            &["Navigate to a URL"],
            &[("<url>", "the url to navigate to")],
            &[],
        ),
        "type" => page(
            "type <text>",
            &["Type text into editable element"],
            &[("<text>", "text to type into the element")],
            &[("--submit", "whether to submit entered text (press enter after)")],
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
            &[("--modifiers", "modifier key to press (repeatable)")],
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
            &[("--submit", "whether to submit entered text (press enter after)")],
        ),
        "press" => page(
            "press <key>",
            &["Press a key on the keyboard, `a`, `ArrowLeft`"],
            &[(
                "<key>",
                "name of the key to press or a character to generate, such as `arrowleft` or `a`",
            )],
            &[],
        ),
        "snapshot" => page(
            "snapshot [target]",
            &["Capture page snapshot to obtain element ref"],
            &[("[target]", "element reference from the previous page snapshot, or a unique element selector for the root element to capture a partial snapshot instead of the whole page")],
            &[
                (
                    "--filename",
                    "save snapshot to markdown file instead of returning it in the response.",
                ),
                ("--depth", "limit snapshot depth, unlimited by default."),
                ("--boxes", "include each element's bounding box as [box=x,y,width,height] in the snapshot. coordinates are viewport-relative, in css pixels (element.getboundingclientrect)."),
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
                ("[target]", "exact target element reference from the page snapshot, or a unique element selector"),
            ],
            &[
                (
                    "--filename",
                    "save evaluation result to a file instead of returning it in the response.",
                ),
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
                    "file name to save the screenshot to. defaults to `page-{timestamp}.{png|jpeg|webp}` if not specified.",
                ),
                ("--type", "image format. if unset, inferred from the filename extension, otherwise png. (one of: png, jpeg, webp)"),
                (
                    "--full-page",
                    "when true, takes a screenshot of the full scrollable page, instead of the currently visible viewport.",
                ),
                ("--hires", "when true, captures a high-resolution screenshot using device pixels (accounts for the device pixel ratio), instead of css pixels."),
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
            &[],
        ),
        "tab-select" => {
            "playwright-cli tab-select <index>\n\nSelect a browser tab\n\nArguments:\n  <index>                     tab index\n"
                .to_string()
        }
        "tab-close" => page(
            "tab-close [index]",
            &["Close a browser tab"],
            &[("[index]", "tab index. if omitted, current tab is closed.")],
            &[],
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
