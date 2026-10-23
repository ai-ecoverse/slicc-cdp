use crate::spec::{self, Spec};

#[cfg(test)]
pub const UPSTREAM_PLAYWRIGHT_CLI: &str = "0.1.22";

pub fn global_help() -> String {
    let mut lines = vec![
        "playwright-cli - run playwright mcp commands from terminal".to_string(),
        String::new(),
        "Usage: playwright-cli <command> [args] [options]".to_string(),
        "Usage: playwright-cli --cdp=<url> <command> [args] [options]".to_string(),
        "Usage: playwright-cli -s=<session> <command> [args] [options]".to_string(),
        String::new(),
        "Connect, in order: --cdp <url> wins; else SLICC_CDP_URL; else a seven-profiles DevToolsActivePort; else a loopback endpoint this tool saved under the user config dir. A .playwright-cli/session.json in the working directory is ignored. Saved state never dials a non-loopback host. Host list GETs /json/version and /json/list on 127.0.0.1:9222-9229. Host open, when that port is absent or unreachable, attaches to the first of those ports whose /json/version answers, with or without a seven tab. A probe-derived session is reused while that browser answers, and every tab is shown except those on protected origins. Otherwise launch binds the first free port from 9222 to 9229, or lets Chrome pick one when all are busy. The wasm build still uses port 9222.".to_string(),
        "A ws:// or wss:// URL with a path is dialed directly. An http:// URL, a path-less ws:// URL, a port, or host[:port] is /json/version discovery. The debugger path is opaque.".to_string(),
        "--runtime <name> is appended to the query of the URL that is opened. It is not interpreted.".to_string(),
        "The host build launches a browser when no debugger is already connected, and the wasm build only opens a tab.".to_string(),
        String::new(),
        "Core:".to_string(),
        row("open [url]", "open the browser"),
        row("close", "close the browser"),
        row("goto <url>", "navigate to a url"),
        row("navigate <url>", "navigate to a url"),
        row("type <text>", "type text into editable element"),
        row("click <target> [button]", "perform click on a web page"),
        row("fill <target> <text>", "fill text into editable element"),
        row("snapshot [target]", "capture page snapshot to obtain element ref"),
        row("eval <func> [target]", "evaluate javascript expression on page or element"),
        row("dialog-accept [prompt]", "accept a dialog"),
        row("dialog-dismiss", "dismiss a dialog"),
        row("resize <w> <h>", "resize the browser window"),
        String::new(),
        "Keyboard:".to_string(),
        row("press <key>", "press a key on the keyboard, `a`, `arrowleft`"),
        row("keydown <key>", "press a key down on the keyboard"),
        row("keyup <key>", "press a key up on the keyboard"),
        String::new(),
        "Mouse:".to_string(),
        row("mousemove <x> <y>", "move mouse to a given position"),
        row("mousedown [button]", "press mouse down"),
        row("mouseup [button]", "press mouse up"),
        row("mousewheel <dx> <dy>", "scroll mouse wheel"),
        String::new(),
        "Pointer and form:".to_string(),
        row("hover <ref>", "hover over an element"),
        row("dblclick <ref> [button]", "double-click an element"),
        row("select <ref> <val>", "select a value in a <select> element"),
        row("check <ref>", "check a checkbox"),
        row("uncheck <ref>", "uncheck a checkbox"),
        row("upload <ref> <file...>", "upload files to a file input"),
        row("drag <startRef> <endRef>", "drag one element to another"),
        row("drop <target>", "drop files or data onto an element"),
        String::new(),
        "Navigation:".to_string(),
        row("go-back", "navigate to the previous page in history"),
        row("go-forward", "navigate to the next page in history"),
        row("reload", "reload the current page"),
        String::new(),
        "Save as:".to_string(),
        row("screenshot [target]", "screenshot of the current page or element"),
        row("pdf", "save the current page as a PDF"),
        String::new(),
        "Tabs:".to_string(),
        row("tab-list", "list all tabs"),
        row("tab-new [url]", "create a new tab"),
        row("tab-close [index]", "close a browser tab"),
        row("tab-select <index>", "select a browser tab"),
        String::new(),
        "Storage:".to_string(),
        row("state-load <filename>", "loads browser storage (authentication) state from a file"),
        row("state-save [filename]", "saves the current storage (authentication) state to a file"),
        row("cookie-list", "list all cookies (optionally filtered by domain/path)"),
        row("cookie-get <name>", "get a specific cookie by name"),
        row("cookie-set <name> <value>", "set a cookie with optional flags"),
        row("cookie-delete <name>", "delete a specific cookie"),
        row("cookie-clear", "clear all cookies"),
        row("localstorage-list", "list all localstorage key-value pairs"),
        row("localstorage-get <key>", "get a localstorage item by key"),
        "  localstorage-set <key> <value> set a localstorage item".to_string(),
        row("localstorage-delete <key>", "delete a localstorage item"),
        row("localstorage-clear", "clear all localstorage"),
        row("sessionstorage-list", "list all sessionstorage key-value pairs"),
        row("sessionstorage-get <key>", "get a sessionstorage item by key"),
        "  sessionstorage-set <key> <value> set a sessionstorage item".to_string(),
        row("sessionstorage-delete <key>", "delete a sessionstorage item"),
        row("sessionstorage-clear", "clear all sessionstorage"),
        String::new(),
        "Console and network:".to_string(),
        row("console [min-level]", "list console messages"),
        row("requests", "list network requests"),
        row("request <index>", "show one network request"),
        row("request-headers <index>", "print request headers"),
        row("request-body <index>", "print the request body"),
        row("response-headers <index>", "print response headers"),
        row("response-body <index>", "print the response body"),
        row("route <pattern>", "mock network requests matching a url pattern"),
        row("route-list", "list all active network routes"),
        row("unroute [pattern]", "remove routes matching a pattern (or all routes)"),
        String::new(),
        "Browsers:".to_string(),
        row(
            "list",
            "list connected browsers, debug ports, and launchables",
        ),
        row(
            "discover [--once <url>]",
            "watch navigations and report agent-facing web standards",
        ),
        String::new(),
        "WebMCP (SLICC):".to_string(),
        row("webmcp-polyfill", "hold a guarded WebMCP shim on a tab"),
        row("webmcp-list", "list WebMCP tools on the page"),
        row("webmcp-call <name> <json|->", "call a WebMCP tool on the page"),
        String::new(),
        "SLICC-only:".to_string(),
        row("frames", "list frame IDs for the current page"),
        String::new(),
        "Install:".to_string(),
        row("upgrade [--check]", "replace this binary with the latest release (alias: update)"),
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
        "a port or host[:port] (localhost, default 9222), a ws:// URL with a path, or an http URL (or path-less ws://) used for /json/version; loopback names try ::1 and 127.0.0.1",
    ));
    lines.push(row(
        "-s, --session <name>",
        "session name for per-agent tab isolation (env: PLAYWRIGHT_CLI_SESSION)",
    ));
    lines.push(row(
        "--runtime <name>",
        "query parameter on the URL that is opened",
    ));
    lines.push(row(
        "--full-urls",
        "print URLs without redacting query strings (may expose login tokens)",
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
        "upgrade" | "update" => crate::upgrade::usage(name),
        "open" => page(
            "open [url]",
            &[
                "Open the browser",
                "The host build launches a browser when no debugger is already connected, and the wasm build only opens a tab.",
                "Chromium uses --remote-debugging-port=0 and DevToolsActivePort in the shared seven-profiles directory.",
            ],
            &[("[url]", "the url to navigate to")],
            &[
                ("--browser", "host browser to launch. chrome-family names, plus chrome, chromium, and msedge. firefox and webkit are not launchable. an electron app name launches that app."),
                ("--config", "path to the configuration file, defaults to .playwright/cli.config.json"),
                ("--device", "emulate a specific device, for example \"iphone 15\"."),
                ("--headed", "run browser in headed mode"),
                ("--idle-timeout", "shut the session down after this many milliseconds without a command. defaults to one hour for headless browsers, never for headed ones. pass 0 to disable."),
                ("--mobile", "emulate a generic mobile device (pixel 10 for chromium, iphone 17 for webkit). mobile pages are usually lighter, which saves tokens."),
                ("--persistent", "use persistent browser profile"),
                ("--profile", "filesystem path that replaces the shared profile directory. it is not a profile name."),
            ],
        ),
        "close" => page("close ", &["Close the browser"], &[], &[]),
        "goto" => page(
            "goto <url>",
            &["Navigate to a URL"],
            &[("<url>", "the url to navigate to")],
            &[],
        ),
        "navigate" => page(
            "navigate <url>",
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
        "tab-select" => page(
            "tab-select <index>",
            &["Select a browser tab", "Only the saved current tab changes. The browser keeps its foreground tab."],
            &[("<index>", "tab index")],
            &[
                ("--foreground", "also bring the tab to the front"),
                ("--fg", "same as --foreground"),
            ],
        ),
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
                "Response bodies are not kept in the saved session, so this reports that the body was not saved.",
            ],
            &[("<index>", "1-based request number from requests")],
            &[
                ("--filename <path>", "write the text to a file"),
                ("--tab <targetId>", "tab to read"),
            ],
        ),
        "go-back" => page(
            "go-back",
            &["Navigate to the previous page in history"],
            &[],
            &[],
        ),
        "go-forward" => page(
            "go-forward",
            &["Navigate to the next page in history"],
            &[],
            &[],
        ),
        "reload" => page(
            "reload",
            &["Reload the current page"],
            &[],
            &[],
        ),
        "hover" => page(
            "hover <ref>",
            &["Hover over an element on the page"],
            &[
                ("<ref>", "exact target element reference from the page snapshot"),
            ],
            &[],
        ),
        "dblclick" => page(
            "dblclick <ref> [button]",
            &["Double-click an element on the page"],
            &[
                ("<ref>", "exact target element reference from the page snapshot"),
                ("[button]", "button to click, defaults to left"),
            ],
            &[("--modifiers", "modifier key to press (repeatable)")],
        ),
        "select" => page(
            "select <ref> <val>",
            &["Select a value in a <select> element"],
            &[
                ("<ref>", "exact target element reference from the page snapshot"),
                ("<val>", "value to select"),
            ],
            &[],
        ),
        "check" => page(
            "check <ref>",
            &["Check a checkbox element"],
            &[
                ("<ref>", "exact target element reference from the page snapshot"),
            ],
            &[],
        ),
        "uncheck" => page(
            "uncheck <ref>",
            &["Uncheck a checkbox element"],
            &[
                ("<ref>", "exact target element reference from the page snapshot"),
            ],
            &[],
        ),
        "upload" => page(
            "upload <ref> <file...>",
            &["Upload one or more files to a file input element"],
            &[
                ("<ref>", "exact target element reference from the page snapshot pointing to a file input"),
                ("<file...>", "one or more file paths to upload"),
            ],
            &[],
        ),
        "drag" => page(
            "drag <startRef> <endRef>",
            &["Drag one element to another"],
            &[
                ("<startRef>", "element reference for the drag source"),
                ("<endRef>", "element reference for the drop target"),
            ],
            &[],
        ),
        "resize" => page(
            "resize <w> <h>",
            &[
                "Resize the browser window",
                "",
                "Sets the window bounds so the viewport becomes <w> by <h> CSS pixels and stays that size after the command exits.",
                "Refused while a protected or unverified tab or frame is open in the same browser profile.",
            ],
            &[("<w>", "width of the browser window"), ("<h>", "height of the browser window")],
            &[],
        ),
        "keydown" | "keyup" => page(
            &format!("{name} <key>"),
            &[
                if name == "keydown" { "Press a key down on the keyboard" } else { "Press a key up on the keyboard" },
                "",
                "Held keys and modifiers are kept per tab in the saved session. Refused while focus is inside an iframe or a closed shadow root.",
            ],
            &[("<key>", "name of the key to press or a character to generate, such as `arrowleft` or `a`")],
            &[],
        ),
        "mousemove" => page(
            "mousemove <x> <y>",
            &["Move mouse to a given position", "", "The pointer position and held buttons are kept per tab in the saved session. Refused over an iframe or other embedded frame."],
            &[("<x>", "x coordinate"), ("<y>", "y coordinate")],
            &[],
        ),
        "mousedown" | "mouseup" => page(
            &format!("{name} [button]"),
            &[
                if name == "mousedown" { "Press mouse down" } else { "Press mouse up" },
                "",
                "Uses the position from the last mousemove on this tab. Refused over an iframe or other embedded frame.",
            ],
            &[("[button]", "button to press, defaults to left")],
            &[],
        ),
        "mousewheel" => page(
            "mousewheel <dx> <dy>",
            &["Scroll mouse wheel", "", "Scrolls at the position from the last mousemove on this tab. Refused over an iframe or other embedded frame."],
            &[("<dx>", "x delta"), ("<dy>", "y delta")],
            &[],
        ),
        "drop" => page(
            "drop <target>",
            &["Drop files or data onto an element", "", "Only the regular files named by --path are read, up to 25 MiB in total."],
            &[("<target>", "exact target element reference from the page snapshot, or a unique element selector")],
            &[
                ("--path", "absolute path to a file to drop onto the element (repeatable)"),
                ("--data", "data to drop in \"mime/type=value\" format, e.g. --data \"text/plain=hello\" (repeatable)"),
            ],
        ),
        "frames" => page(
            "frames",
            &[
                "List frame IDs for the current page",
                "",
                "Frame IDs are for use with --frame on snapshot and eval, not --tab.",
            ],
            &[],
            &[],
        ),
        "dialog-accept" => page(
            "dialog-accept [prompt]",
            &[
                "Accept a dialog",
                "",
                "Arm before triggering a dialog; waits up to 5 seconds for a verified ordinary main-frame dialog.",
                "Already-open dialogs need the foreground dialog holder (follow-up #529).",
            ],
            &[("[prompt]", "the text of the prompt in case of a prompt dialog.")],
            &[("--tab <targetId>", "tab with the dialog")],
        ),
        "dialog-dismiss" => page(
            "dialog-dismiss",
            &[
                "Dismiss a dialog",
                "",
                "Arm before triggering a dialog; waits up to 5 seconds for a verified ordinary main-frame dialog.",
                "Already-open dialogs need the foreground dialog holder (follow-up #529).",
            ],
            &[],
            &[("--tab <targetId>", "tab with the dialog")],
        ),
        "pdf" => page(
            "pdf",
            &["Save the current page as a PDF"],
            &[],
            &[
                ("--filename", "file name to save the PDF to. defaults to page-{timestamp}.pdf."),
            ],
        ),
        "route" => page(
            "route <pattern>",
            &[
                "Mock network requests matching a URL pattern",
                "",
                "Routes are saved with the session and apply while a later command is attached to a tab. Requests to seven's origins and the vault are never routed. Cookies are added from the browser's cookie jar after a route runs, so --remove-header cannot drop them.",
            ],
            &[("<pattern>", "url pattern to match (e.g., \"**/api/users\")")],
            &[
                ("--status", "http status code (default: 200)"),
                ("--body", "response body (text or json string)"),
                ("--content-type", "content-type header"),
                ("--header", "header to add in \"name: value\" format (repeatable)"),
                ("--remove-header", "comma-separated header names to remove"),
                ("--fulfill-file", "file whose bytes are the response body (slicc extension)"),
            ],
        ),
        "route-list" => "playwright-cli route-list \n\nList all active network routes\n\n".to_string(),
        "cookie-list" => page(
            "cookie-list ",
            &["List all cookies (optionally filtered by domain/path)"],
            &[],
            &[
                ("--domain", "filter cookies by domain"),
                ("--path", "filter cookies by path"),
            ],
        ),
        "cookie-get" => page(
            "cookie-get <name>",
            &["Get a specific cookie by name"],
            &[("<name>", "cookie name")],
            &[],
        ),
        "cookie-set" => page(
            "cookie-set <name> <value>",
            &["Set a cookie with optional flags"],
            &[("<name>", "cookie name"), ("<value>", "cookie value")],
            &[
                ("--domain", "cookie domain"),
                ("--path", "cookie path"),
                ("--expires", "cookie expiration as unix timestamp"),
                ("--httpOnly", "whether the cookie is http only"),
                ("--secure", "whether the cookie is secure"),
                ("--sameSite", "cookie samesite attribute (one of: Strict, Lax, None)"),
            ],
        ),
        "cookie-delete" => page(
            "cookie-delete <name>",
            &["Delete a specific cookie"],
            &[("<name>", "cookie name")],
            &[],
        ),
        "cookie-clear" => "playwright-cli cookie-clear \n\nClear all cookies\n\n".to_string(),
        "localstorage-list" => "playwright-cli localstorage-list \n\nList all localStorage key-value pairs\n\n".to_string(),
        "localstorage-get" => page("localstorage-get <key>", &["Get a localStorage item by key"], &[("<key>", "key to get")], &[]),
        "localstorage-set" => page(
            "localstorage-set <key> <value>",
            &["Set a localStorage item"],
            &[("<key>", "key to set"), ("<value>", "value to set")],
            &[],
        ),
        "localstorage-delete" => page("localstorage-delete <key>", &["Delete a localStorage item"], &[("<key>", "key to delete")], &[]),
        "localstorage-clear" => "playwright-cli localstorage-clear \n\nClear all localStorage\n\n".to_string(),
        "sessionstorage-list" => "playwright-cli sessionstorage-list \n\nList all sessionStorage key-value pairs\n\n".to_string(),
        "sessionstorage-get" => page("sessionstorage-get <key>", &["Get a sessionStorage item by key"], &[("<key>", "key to get")], &[]),
        "sessionstorage-set" => page(
            "sessionstorage-set <key> <value>",
            &["Set a sessionStorage item"],
            &[("<key>", "key to set"), ("<value>", "value to set")],
            &[],
        ),
        "sessionstorage-delete" => page("sessionstorage-delete <key>", &["Delete a sessionStorage item"], &[("<key>", "key to delete")], &[]),
        "sessionstorage-clear" => "playwright-cli sessionstorage-clear \n\nClear all sessionStorage\n\n".to_string(),
        "state-save" => page(
            "state-save [filename]",
            &["Saves the current storage (authentication) state to a file"],
            &[("[filename]", "file name to save the storage state to.")],
            &[],
        ),
        "state-load" => page(
            "state-load <filename>",
            &["Loads browser storage (authentication) state from a file"],
            &[("<filename>", "file name to load the storage state from.")],
            &[],
        ),
        "unroute" => page(
            "unroute [pattern]",
            &["Remove routes matching a pattern (or all routes)"],
            &[("[pattern]", "url pattern to unroute (omit to remove all)")],
            &[],
        ),
        "list" => page(
            "list",
            &[
                "List connected browsers, debug ports, and launchables.",
                "",
                "connected",
                "Browsers already attached. A line is `browser - <websocket url>`. Nothing connected is `none`.",
                "",
                "debug ports",
                "Host processes with a DevTools port. A line is `pid <pid> port <port> <executable>`. This section is `none` on wasm32-wasip1.",
                "",
                "launchable",
                "Apps the host can start. A line is `<name> <kind> <path>`. Kind is `browser` or `electron`. This section is `none` on wasm32-wasip1.",
                "",
                "Endpoint sources are `devtools-active-port`, `flag`, `process`, and `probe`.",
                "",
                "list does not launch a browser and does not open a websocket. An explicit --cdp or SLICC_CDP_URL whose HTTP discovery returns a non-2xx status prints that status and body on stderr and exits non-zero. Debug ports and launchables are still printed. On the host, list probes 127.0.0.1:9222-9229 with GET /json/version and GET /json/list. In --json, an endpoint's tabs lists every page except those on protected origins, and sevenTabs the seven ones. The wasm build still uses port 9222. A missing endpoint does not by itself exit non-zero.",
            ],
            &[],
            &[(
                "--json",
                "print one JSON document and no text sections",
            )],
        ),
        "discover" => page(
            "discover [--once <url>]",
            &[
                "Watch navigations and report agent-facing web standards",
                "",
                "Attaches to the running CDP session, watches every page target, and emits one event per finding. Runs until interrupted unless --once is given.",
                "",
                "Sinks:",
                "  stdout (default): NDJSON, one event per line.",
                "  --exec <cmd>: run <cmd> once per event with the event JSON on stdin",
                "    and SLICC_DISCOVER_STANDARD, SLICC_DISCOVER_ORIGIN, SLICC_DISCOVER_URL set.",
                "    One command at a time per origin. --exec-timeout (default 10s) kills",
                "    the whole process group on timeout. Not available in wasm.",
                "  --lick: write each event to ~/.slicc/discoveries/<hash>.json.",
                "    Default when SLICC_CDP_URL is set and no other sink is given.",
                "",
                "Opt-out: --off exits immediately. A file named discover.disabled in",
                "the user config dir (e.g. ~/.config/slicc/ or ~/Library/Application Support/slicc/)",
                "also disables the watcher; checked at start and on SIGHUP.",
            ],
            &[],
            &[
                ("--once <url>", "check one URL and exit instead of watching"),
                ("--tab <targetId>", "tab to use with --once"),
                ("--json", "emit JSON instead of human-readable text"),
                ("--no-probe", "skip well-known probe requests"),
                ("--probe robots", "also probe /robots.txt for Agentmap: lines"),
                ("--probe nlweb", "also probe /ask and POST an MCP initialize to /mcp for NLWeb"),
                ("--follow", "fetch nested ARD catalogs one level deep and follow link targets"),
                ("--exec <cmd>", "run a command for each event (not available in wasm)"),
                ("--exec-timeout <seconds>", "timeout for --exec commands (default: 10)"),
                ("--lick", "write events as slicc lick files under ~/.slicc/"),
                ("--off", "exit immediately without watching"),
            ],
        ),
        "webmcp-polyfill" => page(
            "webmcp-polyfill --tab <targetId>",
            &[
                "Hold the guarded WebMCP polyfill session until stdin or the tab closes.",
                "Native and page-defined modelContext APIs are not replaced.",
                "Protected and unverified documents never receive the shim.",
                "An already-open page may only register tools at load; --reload is opt-in.",
                "Prints one JSON status line without bridge identifiers.",
            ],
            &[],
            &[("--tab <targetId>", "required tab to hold"), ("--reload", "reload the tab once after arming the shim")],
        ),
        "webmcp-list" => page(
            "webmcp-list",
            &[
                "List WebMCP tools registered on the page",
                "",
                "Lists document.modelContext.getTools() from verified provider frames.",
                "Prefers native WebMCP; otherwise uses the guarded private bridge or page API.",
                "Same-origin frames are included. Cross-origin tools require --from-origin.",
                "The tools Permissions Policy and exposedTo are enforced before listing.",
                "Chrome includes declarative form[toolname] tools in getTools().",
                "An empty tool list succeeds (exit 0).",
                "If document.modelContext is absent, reports WebMCP unavailable (exit 2).",
                "Evaluation or connection failures exit 1.",
            ],
            &[],
            &[
                ("--tab <targetId>", "tab to query"),
                ("--json", "print the tool list as a JSON array"),
                ("--from-origin <origin>", "opt in to a cross-origin provider (repeatable)"),
            ],
        ),
        "webmcp-call" => page(
            "webmcp-call <name> <json-input|->",
            &[
                "Call a WebMCP tool on the page",
                "",
                "Runs document.modelContext.executeTool or the private guarded bridge.",
                "Only policy-admitted tools from the current verified document can be called.",
                "The result string is printed to stdout.",
                "",
                "Pass - as <json-input> to read the JSON from stdin (avoids exposing",
                "tool arguments in process listings).",
                "",
                "--allow-consequential is advisory: consequentialHint is declared by",
                "the page, so a hostile page can leave it off. Treat the flag as a UX",
                "guard, not a security boundary.",
                "",
                "Exit codes:",
                "  0  success",
                "  1  tool threw an error",
                "  2  unknown tool, invalid JSON input, no WebMCP support,",
                "     or consequential tool without --allow-consequential",
            ],
            &[
                ("<name>", "tool name"),
                ("<json-input|->", "JSON object passed to the tool, or - for stdin"),
            ],
            &[
                ("--tab <targetId>", "tab to run in"),
                ("--allow-consequential", "permit calling tools with consequentialHint"),
                ("--from-origin <origin>", "opt in to a cross-origin provider (repeatable)"),
                ("--frame <frameId>", "select a provider when tool names are ambiguous"),
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
            | ("tab-close", 0)
            | ("screenshot", 0)
            | ("console", 0)
            | ("dialog-accept", 0)
            | ("unroute", 0)
            | ("record", 0)
            | ("highlight", 0)
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn discover_help_lists_both_opt_in_probes() {
        let text = super::implemented_page("discover").unwrap();
        assert!(text.contains("--probe robots"));
        assert!(text.contains("--probe nlweb"));
    }

    #[test]
    fn response_body_help_says_bodies_are_not_saved() {
        let text = super::implemented_page("response-body").unwrap();
        assert!(text.contains("Response bodies are not kept in the saved session"));
    }
}
