mod action;
mod agent_discover;
mod args;
mod browser;
mod cdp;
mod commands;
mod connect;
mod cookie_headers;
mod cookies;
mod delivery;
mod discover;
mod dialog_holder;
#[cfg(test)]
#[path = "../../../test/integration/document-client.rs"]
mod document_client;
mod eval_attach;
mod eval_frames;
mod eval_lease;
mod eval_scope;
mod har;
mod har_lease;
mod help;
mod input;
mod intent;
mod keys;
mod landing;
mod launch;
mod net;
mod page_tools;
mod recording;
mod route;
mod sanitize;
mod session;
mod snapshot;
mod spec;
mod storage;
mod trace;
mod upgrade;
mod holder_input;

pub use args::Output;

#[cfg(windows)]
pub fn remove_parked_binary() {
    upgrade::remove_parked();
}

pub fn execute(argv: &[String], cdp_env: Option<&str>) -> Output {
    execute_with_session(argv, cdp_env, None)
}

pub fn execute_with_session(argv: &[String], cdp_env: Option<&str>, session_env: Option<&str>) -> Output {
    execute_in(argv, cdp_env, session_env, None)
}

pub fn execute_in(
    argv: &[String],
    cdp_env: Option<&str>,
    session_env: Option<&str>,
    cwd: Option<&std::path::Path>,
) -> Output {
    let invocation = match args::parse(argv) {
        Ok(mut invocation) => {
            invocation.cwd = cwd.map(std::path::Path::to_path_buf);
            if invocation.session.is_none() {
                invocation.session = session_env.map(str::to_string);
            }
            invocation.intent = intent::choose(invocation.intent.as_deref(), std::env::var("SLICC_INTENT").ok().as_deref());
            invocation
        }
        Err(output) => return output,
    };
    sanitize::set_full_urls(invocation.full_urls);
    if invocation.version {
        return Output::ok(format!(
            "playwright-cli {}\n",
            env!("PLAYWRIGHT_CLI_VERSION")
        ));
    }
    if invocation.help && invocation.command.is_none() {
        return Output::ok(format!("{}\n", help::global_help()));
    }
    let Some(command) = invocation.command.clone() else {
        return Output::ok(format!("{}\n", help::global_help()));
    };
    let Some(spec) = spec::lookup(&command) else {
        if invocation.help || invocation.command_help {
            return Output::ok(format!("{}\n", help::global_help()));
        }
        return Output {
            stdout: format!("{}\n", help::global_help()),
            stderr: format!("Unknown command: {command}\n\n"),
            code: 1,
        };
    };
    if invocation.help || invocation.command_help {
        return Output::ok(help::command_help(spec));
    }
    if let Err(output) = args::validate(&invocation, spec) {
        return output;
    }
    if command == "upgrade" || command == "update" {
        return upgrade::run(&invocation);
    }
    if command == "list" {
        return discover::run(&invocation, cdp_env);
    }
    if command == "discover" {
        return agent_discover::run(&invocation, cdp_env);
    }
    if !spec.implemented {
        return Output::err(format!("playwright-cli {command}: not implemented yet\n"));
    }
    if route::is_route_command(&command) {
        return route::run(&invocation, cdp_env);
    }
    if command == "network-state-set" {
        return page_tools::network_state_set(&invocation, cdp_env);
    }
    if command == "eval-detach" { return eval_lease::detach(&invocation); }
    if command == "eval-attach" {
        if let Err(message) = eval_attach::precheck(&invocation) { return Output::err(message); }
    }
    if let Some(message) = har::precheck(&invocation) {
        return Output::err(message);
    }
    if matches!(command.as_str(), "har-stop" | "stop-recording") {
        return har::stop(&invocation);
    }
    if recording::is_stop(&command) {
        return recording::stop(&invocation);
    }
    commands::run_command(&invocation, cdp_env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_less_navigation_gets_a_hint_before_endpoint_validation() {
        for command in ["open", "tab-new", "goto", "navigate"] {
            for url in ["example.com", "example.com/PATHSECRET?q=QUERYSECRET#FRAGSECRET", "//example.com/path", "/relative/path", "\texample.com"] {
                let output = execute(&["--cdp".into(), "ftp://127.0.0.1:1/invalid".into(), command.into(), url.into()], None);
                assert_eq!(output.code, 1, "{command} {url}");
                assert_eq!(output.stdout, "");
                assert_eq!(output.stderr, format!("playwright-cli {command}: URL needs a scheme; add https:// or http://\n"));
                for secret in ["PATHSECRET", "QUERYSECRET", "FRAGSECRET"] {
                    assert!(!output.stderr.contains(secret));
                }
                assert!(!output.stderr.contains("protected page"));
            }
        }
    }

    fn discovery_answering(socket: impl Fn(u16) -> String + Send + 'static) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = hits.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(16) {
                let Ok(mut stream) = stream else { return };
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                if !String::from_utf8_lossy(&buf[..n]).starts_with("GET /json/version") {
                    seen.fetch_add(1000, std::sync::atomic::Ordering::SeqCst);
                    continue;
                }
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let body = format!(r#"{{"webSocketDebuggerUrl":"{}"}}"#, socket(port));
                let _ = stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
            }
        });
        (format!("http://127.0.0.1:{port}/json/version"), hits)
    }

    #[test]
    fn every_rule_ends_in_the_checked_dial_and_a_foreign_socket_is_refused() {
        use cdp_client::resolve::{Listed, Rule};
        let (foreign, foreign_hits) = discovery_answering(|_| "ws://203.0.113.9:9223/devtools/browser/x".to_string());
        let empty = Listed::default();
        let routes: Vec<(Option<&str>, Option<&str>, Listed, &str)> = vec![
            (Some(foreign.as_str()), None, empty.clone(), "via --cdp"),
            (None, Some(foreign.as_str()), empty.clone(), "via SLICC_CDP_URL"),
            (None, None, Listed { attached: Some((Rule::SliccBrowsers, foreign.clone())), ..Listed::default() }, "via slicc browsers"),
            (None, None, Listed { attached: Some((Rule::ActivePort, foreign.clone())), ..Listed::default() }, "via DevToolsActivePort"),
            (None, None, Listed { saved: Some(foreign.clone()), ..Listed::default() }, "via saved endpoint"),
            (None, None, Listed { probe: Some(foreign.clone()), ..Listed::default() }, "via port probe"),
        ];
        let count = routes.len();
        for (cdp, env, finder, via) in routes {
            launch::TEST_FINDER.with(|cell| *cell.borrow_mut() = Some(finder));
            let mut argv = vec!["--verbose".to_string()];
            if let Some(cdp) = cdp {
                argv.extend(["--cdp".to_string(), cdp.to_string()]);
            }
            argv.push("tab-list".to_string());
            let output = execute(&argv, env);
            launch::TEST_FINDER.with(|cell| *cell.borrow_mut() = None);
            assert_eq!(output.code, 1, "{via}: {}", output.stderr);
            assert!(output.stderr.contains(via), "{via}: {}", output.stderr);
            assert!(output.stderr.contains("returned ws://203.0.113.9:9223/devtools/browser/x, a host other than this machine; refusing to dial it"), "{via}: {}", output.stderr);
        }
        assert_eq!(foreign_hits.load(std::sync::atomic::Ordering::SeqCst), count);
        let dead = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://127.0.0.1:{}/json/version", listener.local_addr().unwrap().port())
        };
        launch::TEST_FINDER.with(|cell| *cell.borrow_mut() = Some(Listed { saved: Some(dead.clone()), ..Listed::default() }));
        let output = execute(&["tab-list".to_string()], None);
        launch::TEST_FINDER.with(|cell| *cell.borrow_mut() = None);
        assert_eq!(output.code, 1);
        assert!(output.stderr.ends_with(&format!("playwright-cli: {dead} is the endpoint playwright-cli saved for this directory, and it did not answer. Pass --cdp <port|url> to choose another browser.\n")), "{}", output.stderr);
        let (named, named_hits) = discovery_answering(|port| format!("ws://evil.localhost:{port}/devtools/browser/x"));
        launch::TEST_FINDER.with(|cell| *cell.borrow_mut() = Some(Listed { probe: Some(named), ..Listed::default() }));
        let output = execute(&["tab-list".to_string()], None);
        launch::TEST_FINDER.with(|cell| *cell.borrow_mut() = None);
        assert_eq!(output.code, 1);
        assert!(output.stderr.contains("a host other than this machine; refusing to dial it"), "{}", output.stderr);
        assert_eq!(named_hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        let (local, local_hits) = discovery_answering(|port| format!("ws://[::1]:{port}/devtools/browser/x"));
        launch::TEST_FINDER.with(|cell| *cell.borrow_mut() = Some(Listed { probe: Some(local), ..Listed::default() }));
        let output = execute(&["tab-list".to_string()], None);
        launch::TEST_FINDER.with(|cell| *cell.borrow_mut() = None);
        assert_eq!(output.code, 1);
        assert!(!output.stderr.contains("refusing to dial"), "{}", output.stderr);
        assert_eq!(local_hits.load(std::sync::atomic::Ordering::SeqCst), 1001);
    }

    #[test]
    fn connect_is_explained_and_typos_get_a_hint() {
        let typo = execute(&["--cpd".into(), "0".into(), "snapshot".into()], None);
        assert_eq!(typo.code, 1);
        assert_eq!(typo.stderr, "Unknown option: --cpd\nDid you mean --cdp?\n\n");
        let inner = execute(&["snapshot".into(), "--cpd=0".into()], None);
        assert!(inner.stderr.starts_with("Unknown option: --cpd\nDid you mean --cdp?\n"), "{}", inner.stderr);
        let zero = execute(&["--cdp".into(), "0".into(), "snapshot".into()], None);
        assert!(zero.stderr.contains("use --cdp auto to search"), "{}", zero.stderr);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let missing = execute(&["--cdp".into(), "auto".into(), "snapshot".into()], Some("9444"));
            assert_eq!(missing.code, 1);
            assert!(missing.stderr.starts_with(
                "playwright-cli: no CDP browser found. Tried, in order:\n  --cdp               auto\n  SLICC_CDP_URL       set, skipped by --cdp auto\n  saved endpoint      none usable for this directory\n"
            ), "{}", missing.stderr);
            let open = execute(&["--cdp=auto".into(), "--verbose".into(), "open".into()], None);
            assert!(open.stderr.contains("browser launch is not available in tests"), "{}", open.stderr);
        }
        let verbose = execute(&["--verbose".into(), "--cdp".into(), "ws://127.0.0.1:1/devtools/browser/x".into(), "snapshot".into()], None);
        assert_eq!(verbose.code, 1);
        assert!(verbose.stderr.starts_with("* CDP ws://127.0.0.1:1/devtools/browser/x (via --cdp)\n"), "{}", verbose.stderr);
        let quiet = execute(&["--cdp".into(), "ws://127.0.0.1:1/devtools/browser/x".into(), "snapshot".into()], None);
        assert!(!quiet.stderr.contains("* CDP"));
    }

    #[test]
    fn unknown_command_matches_upstream_usage_error() {
        let output = execute(&["nope".to_string()], None);
        assert_eq!(output.code, 1);
        assert_eq!(output.stderr, "Unknown command: nope\n\n");
        assert!(output
            .stdout
            .contains("playwright-cli - run playwright mcp commands from terminal"));
        assert!(output
            .stdout
            .contains("Usage: playwright-cli <command> [args] [options]"));
    }

    #[test]
    fn help_lists_shared_commands() {
        let output = execute(&[], None);
        assert_eq!(output.code, 0);
        for name in [
            "open",
            "close",
            "goto",
            "snapshot",
            "click",
            "fill",
            "type",
            "press",
            "screenshot",
            "eval",
            "tab-list",
            "tab-new",
            "tab-select",
            "tab-close",
        ] {
            assert!(output.stdout.contains(name), "{name}");
        }
        assert!(output.stdout.contains("Not implemented yet"));
        assert!(output.stdout.contains("console"));
        assert!(output.stdout.contains("requests"));
        let pending = output
            .stdout
            .split("Not implemented yet:")
            .nth(1)
            .unwrap_or("");
        assert!(!pending.contains("console"));
        assert!(!pending.contains("dblclick"));
        assert!(!pending.contains("dialog-accept"));
        assert!(!pending.contains("dialog-dismiss"));
        assert!(!pending.contains("resize"));
        assert!(!output.stdout.contains("cdp.slicc.internal"));
        assert!(!output.stdout.contains("cdp.kernel.localhost"));
        assert!(!output.stdout.contains("/devtools/browser/slicc"));
    }

    #[test]
    fn help_matches_upstream_playwright_cli_0_1_22() {
        assert_eq!(help::UPSTREAM_PLAYWRIGHT_CLI, "0.1.22");
        let global = execute(&["--help".into()], None);
        assert_eq!(global.code, 0);
        assert_eq!(global.stderr, "");
        let stdout = &global.stdout;
        assert!(stdout.starts_with(
            "playwright-cli - run playwright mcp commands from terminal\n\nUsage: playwright-cli <command> [args] [options]\n"
        ));
        assert!(stdout.contains("Usage: playwright-cli --cdp=<url> <command> [args] [options]\n"));
        for shared in [
            cdp_client::resolve::CONNECT_ORDER,
            cdp_client::resolve::CDP_FORMS,
            cdp_client::resolve::CDP_REPORT,
        ] {
            assert!(stdout.contains(&format!("{shared}\n")), "{shared}");
        }
        assert!(stdout.contains(
            "Host list GETs /json/version and /json/list on 127.0.0.1:9222-9229. Host open, when nothing is found, launches a browser on the first free port from 9222 to 9229, or lets Chrome pick one when all are busy; a probe-derived session is reused while that browser answers, and every tab is shown except those on protected origins. Other commands exit 1 instead of launching.\n"
        ));
        assert!(stdout.contains(
            "--runtime <name> is appended to the query of the URL that is opened. It is not interpreted.\n"
        ));
        assert!(stdout.contains(
            "The host build launches a browser when no debugger is already connected, and the wasm build only opens a tab.\n"
        ));
        for line in [
            "  open [url]                  open the browser",
            "  close                       close the browser",
            "  goto <url>                  navigate to a url",
            "  navigate <url>              navigate to a url",
            "  type <text>                 type text into editable element",
            "  click <target> [button]     perform click on a web page",
            "  fill <target> <text>        fill text into editable element",
            "  snapshot [target]           capture page snapshot to obtain element ref",
            "  eval <func> [target]        evaluate javascript expression on page or element",
            "  dialog-accept [prompt]      accept a dialog",
            "  dialog-dismiss              dismiss a dialog",
            "  press <key>                 press a key on the keyboard, `a`, `arrowleft`",
            "  screenshot [target]         screenshot of the current page or element",
            "  tab-list                    list all tabs",
            "  tab-new [url]               create a new tab",
            "  tab-close [index]           close a browser tab",
            "  tab-select <index>          select a browser tab",
        ] {
            assert!(stdout.contains(line), "{line}");
        }
        assert!(stdout.contains("-s=<session>"));
        assert!(stdout.contains("--session"));
        assert!(stdout.contains("PLAYWRIGHT_CLI_SESSION"));
        assert!(!stdout.contains("--headed"));
        assert!(!stdout.contains("--browser"));
        assert!(!stdout.contains("install-browser"));
        assert!(!stdout.contains("--json"));
        assert!(!stdout.contains("--raw"));
        let pending = stdout.split("Not implemented yet:").nth(1).unwrap();
        let pending_names: Vec<&str> = pending
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .filter(|name| !name.is_empty() && !name.starts_with("--") && !name.ends_with(':'))
            .collect();
        for name in [
            "open",
            "close",
            "goto",
            "navigate",
            "snapshot",
            "click",
            "fill",
            "type",
            "press",
            "screenshot",
            "eval",
            "tab-list",
            "tab-new",
            "tab-select",
            "tab-close",
        ] {
            assert!(!pending_names.contains(&name), "{name} listed as pending");
        }
        for line in [
            "  console [min-level]         list console messages",
            "  requests                    list network requests",
            "  request <index>             show one network request",
            "  request-headers <index>     print request headers",
            "  request-body <index>        print the request body",
            "  response-headers <index>    print response headers",
            "  response-body <index>       print the response body",
        ] {
            assert!(stdout.contains(line), "{line}");
        }
        for name in [
            "console",
            "requests",
            "request",
            "request-headers",
            "request-body",
            "response-headers",
            "response-body",
        ] {
            assert!(!pending_names.contains(&name), "{name} listed as pending");
        }
        for name in ["resize", "keydown", "keyup", "mousemove", "mousedown", "mouseup", "mousewheel", "drop"] {
            assert!(!pending_names.contains(&name), "{name}");
        }

        assert_page(
            "open",
            "playwright-cli open [url]\n\nOpen the browser\n",
            &[
                "  [url]                       the url to navigate to",
                "The host build launches a browser when no debugger is already connected, and the wasm build only opens a tab.",
                "Chromium uses --remote-debugging-port=0 and DevToolsActivePort in the shared seven-profiles directory.",
                "  --browser                   host browser to launch. chrome-family names, plus chrome, chromium, and msedge. firefox and webkit are not launchable. an electron app name launches that app.",
                "  --config                    path to the configuration file, defaults to .playwright/cli.config.json",
                "  --device                    emulate a specific device, for example \"iphone 15\".",
                "  --headed                    run browser in headed mode",
                "  --idle-timeout              shut the session down after this many milliseconds without a command. defaults to one hour for headless browsers, never for headed ones. pass 0 to disable.",
                "  --mobile                    emulate a generic mobile device (pixel 10 for chromium, iphone 17 for webkit). mobile pages are usually lighter, which saves tokens.",
                "  --persistent                use persistent browser profile",
                "  --profile                   filesystem path that replaces the shared profile directory. it is not a profile name.",
            ],
            &[
                "--foreground",
                "--fg",
                "--runtime",
            ],
        );
        assert_page(
            "dialog-accept",
            "playwright-cli dialog-accept [prompt]\n\nAccept a dialog\n",
            &[
                "  [prompt]                    the text of the prompt in case of a prompt dialog.",
                "Arm before triggering a dialog; waits up to 5 seconds for a verified ordinary main-frame dialog.",
                "Already-open dialogs need the foreground dialog holder (follow-up #529).",
            ],
            &["Not implemented"],
        );
        assert_page(
            "dialog-dismiss",
            "playwright-cli dialog-dismiss\n\nDismiss a dialog\n",
            &[
                "Arm before triggering a dialog; waits up to 5 seconds for a verified ordinary main-frame dialog.",
                "Already-open dialogs need the foreground dialog holder (follow-up #529).",
            ],
            &["Not implemented"],
        );
        assert_page(
            "close",
            "playwright-cli close \n\nClose the browser\n",
            &[],
            &["-s="],
        );
        assert_page(
            "goto",
            "playwright-cli goto <url>\n\nNavigate to a URL\n\nArguments:\n  <url>                       the url to navigate to",
            &[],
            &[],
        );
        assert_page(
            "type",
            "playwright-cli type <text>\n\nType text into editable element\n\nArguments:\n  <text>                      text to type into the element\nOptions:\n  --submit                    whether to submit entered text (press enter after)",
            &[],
            &[],
        );
        assert_page(
            "click",
            "playwright-cli click <target> [button]\n\nPerform click on a web page\n\nArguments:\n  <target>                    exact target element reference from the page snapshot, or a unique element selector\n  [button]                    button to click, defaults to left\nOptions:\n  --modifiers                 modifier key to press (repeatable)",
            &[],
            &[],
        );
        assert_page(
            "fill",
            "playwright-cli fill <target> <text>\n\nFill text into editable element\n\nArguments:\n  <target>                    exact target element reference from the page snapshot, or a unique element selector\n  <text>                      text to fill into the element\nOptions:\n  --submit                    whether to submit entered text (press enter after)",
            &[],
            &[],
        );
        assert_page(
            "press",
            "playwright-cli press <key>\n\nPress a key on the keyboard, `a`, `ArrowLeft`\n\nArguments:\n  <key>                       name of the key to press or a character to generate, such as `arrowleft` or `a`",
            &[],
            &[],
        );
        assert_page(
            "snapshot",
            "playwright-cli snapshot [target]\n\nCapture page snapshot to obtain element ref\n",
            &[
                "  [target]                    element reference from the previous page snapshot, or a unique element selector for the root element to capture a partial snapshot instead of the whole page",
                "  --filename                  save snapshot to markdown file instead of returning it in the response.",
                "  --depth                     limit snapshot depth, unlimited by default.",
                "  --boxes                     include each element's bounding box as [box=x,y,width,height] in the snapshot. coordinates are viewport-relative, in css pixels (element.getboundingclientrect).",
            ],
            &["--frame", "--no-iframes", "--tab"],
        );
        assert_page(
            "eval",
            "playwright-cli eval <func> [target]\n\nEvaluate JavaScript expression on page or element\n\nArguments:\n  <func>                      () => { /* code */ } or (element) => { /* code */ } when element is provided",
            &[
                "  [target]                    exact target element reference from the page snapshot, or a unique element selector",
                "  --filename                  save evaluation result to a file instead of returning it in the response.",
            ],
            &["--output", "--frame", "--tab"],
        );
        assert_page(
            "screenshot",
            "playwright-cli screenshot [target]\n\nscreenshot of the current page or element\n\nArguments:\n  [target]                    exact target element reference from the page snapshot, or a unique element selector",
            &[
                "  --filename                  file name to save the screenshot to. defaults to `page-{timestamp}.{png|jpeg|webp}` if not specified.",
                "  --type                      image format. if unset, inferred from the filename extension, otherwise png. (one of: png, jpeg, webp)",
                "  --full-page                 when true, takes a screenshot of the full scrollable page, instead of the currently visible viewport.",
                "  --hires                     when true, captures a high-resolution screenshot using device pixels (accounts for the device pixel ratio), instead of css pixels.",
            ],
            &["--max-width", "--tab"],
        );
        let tab_list = execute(&["tab-list".into(), "--help".into()], None);
        assert_eq!(tab_list.code, 0);
        assert_eq!(
            tab_list.stdout,
            "playwright-cli tab-list \n\nList all tabs\n\n"
        );
        assert_page(
            "tab-new",
            "playwright-cli tab-new [url]\n\nCreate a new tab\n\nArguments:\n  [url]                       the url to navigate to in the new tab. if omitted, the new tab will be blank.",
            &[],
            &["--headed", "--browser", "--foreground", "--fg", "--runtime"],
        );
        let tab_select = execute(&["tab-select".into(), "--help".into()], None);
        assert_eq!(tab_select.code, 0);
        assert_eq!(
            tab_select.stdout,
            "playwright-cli tab-select <index>\n\nSelect a browser tab\nOnly the saved current tab changes. The browser keeps its foreground tab.\n\nArguments:\n  <index>                     tab index\nOptions:\n  --foreground                also bring the tab to the front\n  --fg                        same as --foreground\n"
        );
        assert_page(
            "tab-close",
            "playwright-cli tab-close [index]\n\nClose a browser tab\n",
            &["  [index]                     tab index. if omitted, current tab is closed."],
            &[],
        );

        assert_page(
            "console",
            "playwright-cli console [min-level]\n\nList console messages\n",
            &[
                "Levels: error, warning, info, debug. Defaults to info. Each level includes more severe levels.",
                "  [min-level]                 minimum level to show",
                "  --clear                     drop the stored console list for the tab",
                "  --tab <targetId>            tab to read",
            ],
            &["Not implemented yet"],
        );
        assert_page(
            "requests",
            "playwright-cli requests\n\nList network requests captured for the tab. Each line is numbered for request.\n",
            &[
                "  --static                    include successful static resources",
                "  --filter <text>             substring of the request URL",
                "  --clear                     drop the stored network list for the tab",
                "  --tab <targetId>            tab to read",
            ],
            &["Not implemented yet"],
        );
        assert_page(
            "request",
            "playwright-cli request <index>\n\nShow one network request by its number from requests.\n",
            &[
                "  <index>                     1-based request number from requests",
                "  --filename <path>           write the text to a file",
            ],
            &["Not implemented yet"],
        );
        assert_page(
            "route",
            "playwright-cli route <pattern>\n\nMock network requests matching a URL pattern\n",
            &[
                "  <pattern>                   url pattern to match (e.g., \"**/api/users\")",
                "  --status                    http status code (default: 200)",
                "  --body                      response body (text or json string)",
                "  --content-type              content-type header",
                "  --header                    header to add in \"name: value\" format (repeatable)",
                "  --remove-header             comma-separated header names to remove",
                "  --fulfill-file              file whose bytes are the response body (slicc extension)",
            ],
            &["Not implemented yet", "--tab"],
        );
        assert_eq!(
            execute(&["route-list".into(), "--help".into()], None).stdout,
            "playwright-cli route-list \n\nList all active network routes\n\n"
        );
        assert_eq!(
            execute(&["unroute".into(), "--help".into()], None).stdout,
            "playwright-cli unroute [pattern]\n\nRemove routes matching a pattern (or all routes)\n\nArguments:\n  [pattern]                   url pattern to unroute (omit to remove all)\n"
        );
        for line in [
            "  route <pattern>             mock network requests matching a url pattern",
            "  route-list                  list all active network routes",
            "  unroute [pattern]           remove routes matching a pattern (or all routes)",
        ] {
            assert!(stdout.contains(line), "{line}");
        }
        for name in ["route", "route-list", "unroute"] {
            assert!(!pending_names.contains(&name), "{name} listed as pending");
        }
        assert_eq!(
            execute(&["dblclick".into(), "--help".into()], None).stdout,
            "playwright-cli dblclick <ref> [button]\n\nDouble-click an element on the page\n\nArguments:\n  <ref>                       exact target element reference from the page snapshot\n  [button]                    button to click, defaults to left\nOptions:\n  --modifiers                 modifier key to press (repeatable)\n"
        );

        for args in [
            vec!["open", "--persistent"],
            vec!["screenshot", "--hires"],
            vec!["screenshot", "--type", "jpeg"],
        ] {
            let owned: Vec<String> = args.iter().map(|part| (*part).to_string()).collect();
            let output = execute(&owned, None);
            assert_eq!(output.code, 1, "{args:?}");
            assert!(
                output.stderr.contains("Unknown option"),
                "{args:?} {}",
                output.stderr
            );
            assert!(!output.stderr.contains("9222"), "{args:?}");
        }
        for args in [
            vec!["open", "--headed"],
            vec!["open", "--browser", "chrome"],
            vec!["open", "--profile", "/tmp/playwright-cli-profile"],
            vec!["tab-new", "--headed"],
        ] {
            let owned: Vec<String> = args.iter().map(|part| (*part).to_string()).collect();
            let output = execute(&owned, None);
            assert_eq!(output.code, 1, "{args:?} {}", output.stderr);
            assert!(
                !output.stderr.contains("Unknown option"),
                "{args:?} {}",
                output.stderr
            );
            assert!(!output.stderr.contains("9222"), "{args:?} {}", output.stderr);
        }
        let firefox = execute(
            &["open".into(), "--browser".into(), "firefox".into()],
            None,
        );
        assert_eq!(firefox.code, 1);
        assert_eq!(
            firefox.stderr,
            "playwright-cli open: firefox is not launchable\n"
        );
        assert!(!firefox.stderr.contains("9222"));
        let missing_file = execute(&["eval".into(), "--filename".into()], None);
        assert!(missing_file.stderr.contains("--filename"));
        let snapshot_target = execute(&["snapshot".into(), "e1".into()], None);
        assert_eq!(snapshot_target.code, 1);
        assert!(snapshot_target.stderr.contains("too many arguments"));
        assert!(snapshot_target.stdout.contains("[target]"));
        assert!(snapshot_target
            .stdout
            .contains("partial snapshot instead of the whole page"));
        let tab_extra = execute(&["tab-close".into(), "1".into(), "2".into()], None);
        assert_eq!(tab_extra.code, 1);
        assert!(tab_extra.stderr.contains("too many arguments"));
        for bad in ["0", "x", "1.5", "01x"] {
            let tab_index = execute(&["tab-close".into(), bad.into()], None);
            assert_eq!(tab_index.code, 1, "{bad}");
            assert_eq!(tab_index.stderr, "tab-close index must be a positive integer\n", "{bad}");
        }
        let both = execute(&["tab-close".into(), "1".into(), "--tab".into(), "T".into()], None);
        assert_eq!(both.stderr, "tab-close takes an index or --tab, not both\n");
        let foreground = super::args::parse(&["open".into(), "--foreground".into()]).unwrap();
        assert_eq!(
            foreground.flags.get("foreground").map(String::as_str),
            Some("true")
        );
        let full_page = super::args::parse(&["screenshot".into(), "--full-page".into()]).unwrap();
        assert_eq!(
            full_page.flags.get("full-page").map(String::as_str),
            Some("true")
        );
    }

    fn assert_page(command: &str, prefix: &str, lines: &[&str], absent: &[&str]) {
        let output = execute(&[command.to_string(), "--help".to_string()], None);
        assert_eq!(output.code, 0, "{command}");
        assert_eq!(output.stderr, "", "{command}");
        assert!(
            output.stdout.starts_with(prefix),
            "{command}\n--- stdout ---\n{}\n--- prefix ---\n{prefix}",
            output.stdout
        );
        for line in lines {
            assert!(
                output.stdout.contains(line),
                "{command} missing {line}\n{}",
                output.stdout
            );
        }
        for phrase in absent {
            assert!(
                !output.stdout.contains(phrase),
                "{command} contains {phrase}"
            );
        }
    }

    #[test]
    fn version_prints_package_version() {
        let output = execute(&["--version".to_string()], None);
        assert_eq!(output.code, 0);
        assert_eq!(output.stderr, "");
        assert_eq!(
            output.stdout,
            format!("playwright-cli {}\n", env!("PLAYWRIGHT_CLI_VERSION"))
        );
        let short = execute(&["-V".to_string()], None);
        assert_eq!(short.stdout, output.stdout);
        assert_eq!(short.code, 0);
    }

    #[test]
    fn console_rejects_an_unknown_level_before_connect() {
        let output = execute(&["console".to_string(), "verbose".to_string()], None);
        assert_eq!(output.code, 1);
        assert!(output.stderr.contains("min-level"));
        assert!(output.stdout.is_empty());
    }

    #[test]
    fn request_requires_a_positive_index_before_connect() {
        let missing = execute(&["request".to_string()], None);
        assert_eq!(missing.code, 1);
        assert!(missing.stderr.contains("index"));
        let zero = execute(&["response-body".to_string(), "0".to_string()], None);
        assert_eq!(zero.code, 1);
        assert!(zero.stderr.contains("positive"));
    }

    #[test]
    fn basics_batch_help_pages() {
        for command in [
            "go-back",
            "go-forward",
            "reload",
            "hover",
            "dblclick",
            "select",
            "check",
            "uncheck",
            "upload",
            "drag",
            "frames",
            "pdf",
            "navigate",
        ] {
            let output = execute(&[command.to_string(), "--help".to_string()], None);
            assert_eq!(output.code, 0, "{command}");
            assert_eq!(output.stderr, "", "{command}");
            assert!(
                output.stdout.starts_with(&format!("playwright-cli {command}")),
                "{command}: {}",
                output.stdout
            );
            assert!(
                !output.stdout.contains("Not implemented yet"),
                "{command}"
            );
        }
    }

    #[test]
    fn basics_batch_are_implemented_in_spec() {
        for command in [
            "go-back",
            "go-forward",
            "reload",
            "hover",
            "dblclick",
            "select",
            "check",
            "uncheck",
            "upload",
            "drag",
            "frames",
            "pdf",
            "navigate",
        ] {
            let spec = spec::lookup(command);
            assert!(spec.is_some(), "{command} not in spec");
            assert!(spec.unwrap().implemented, "{command} not implemented");
        }
    }

    #[test]
    fn basics_batch_not_in_pending_list() {
        let output = execute(&[], None);
        let pending = output
            .stdout
            .split("Not implemented yet:")
            .nth(1)
            .unwrap_or("");
        for command in [
            "go-back",
            "go-forward",
            "reload",
            "hover",
            "dblclick",
            "select",
            "check",
            "uncheck",
            "upload",
            "drag",
            "frames",
            "pdf",
            "navigate",
        ] {
            assert!(
                !pending.contains(command),
                "{command} still listed as pending"
            );
        }
    }

    #[test]
    fn basics_batch_global_help_sections() {
        let output = execute(&["--help".to_string()], None);
        assert_eq!(output.code, 0);
        for line in [
            "  go-back",
            "  go-forward",
            "  reload",
            "  hover <ref>",
            "  dblclick <ref> [button]",
            "  select <ref> <val>",
            "  check <ref>",
            "  uncheck <ref>",
            "  upload <ref> <file...>",
            "  navigate <url>",
            "  drag <startRef> <endRef>",
            "  pdf",
            "  frames",
        ] {
            assert!(output.stdout.contains(line), "missing: {line}");
        }
        assert!(output.stdout.contains("Pointer and form:"));
        assert!(output.stdout.contains("Navigation:"));
        assert!(output.stdout.contains("SLICC-only:"));
    }

    #[test]
    fn navigate_help_page() {
        let output = execute(&["navigate".to_string(), "--help".to_string()], None);
        assert_eq!(output.code, 0);
        assert!(output.stdout.starts_with("playwright-cli navigate <url>"), "{}", output.stdout);
        assert!(output.stdout.contains("Navigate to a URL"), "{}", output.stdout);
    }

    #[test]
    fn dblclick_rejects_bad_ref_before_connect() {
        let output = execute(&["dblclick".to_string(), "/tmp/bad".to_string()], None);
        assert_eq!(output.code, 1);
        assert!(
            output.stderr.contains("not an element ref"),
            "{}",
            output.stderr
        );
    }

    #[test]
    fn drag_rejects_bad_ref_before_connect() {
        let output = execute(
            &[
                "drag".to_string(),
                "not-a-ref".to_string(),
                "e2".to_string(),
            ],
            None,
        );
        assert_eq!(output.code, 1);
        assert!(
            output.stderr.contains("not an element ref"),
            "{}",
            output.stderr
        );
    }

    #[test]
    fn hover_rejects_bad_ref_before_connect() {
        let output = execute(&["hover".to_string(), "not-a-ref".to_string()], None);
        assert_eq!(output.code, 1);
        assert!(
            output.stderr.contains("not an element ref"),
            "{}",
            output.stderr
        );
    }

    #[test]
    fn check_rejects_bad_ref_before_connect() {
        let output = execute(&["check".to_string(), "button".to_string()], None);
        assert_eq!(output.code, 1);
        assert!(
            output.stderr.contains("not an element ref"),
            "{}",
            output.stderr
        );
    }

    #[test]
    fn uncheck_rejects_bad_ref_before_connect() {
        let output = execute(&["uncheck".to_string(), "button".to_string()], None);
        assert_eq!(output.code, 1);
        assert!(
            output.stderr.contains("not an element ref"),
            "{}",
            output.stderr
        );
    }

    #[test]
    fn dblclick_too_many_args() {
        let output = execute(
            &[
                "dblclick".to_string(),
                "e1".to_string(),
                "left".to_string(),
                "extra".to_string(),
            ],
            None,
        );
        assert_eq!(output.code, 1);
        assert!(output.stderr.contains("too many arguments"));
    }

    #[test]
    fn session_flag_overrides_env() {
        let output = execute_with_session(
            &["-s=flag-session".to_string(), "--help".to_string()],
            None,
            Some("env-session"),
        );
        assert_eq!(output.code, 0);
    }

    #[test]
    fn session_env_is_used_when_no_flag() {
        let inv = args::parse(&["snapshot".to_string()]).unwrap();
        assert!(inv.session.is_none());
        let output = execute_with_session(
            &["--help".to_string()],
            None,
            Some("from-env"),
        );
        assert_eq!(output.code, 0);
    }
}
