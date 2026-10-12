mod action;
mod args;
mod browser;
mod cdp;
mod commands;
mod connect;
mod discover;
mod help;
mod launch;
mod net;
mod session;
mod snapshot;
mod spec;
mod trace;

pub use args::Output;

pub fn execute(argv: &[String], cdp_env: Option<&str>) -> Output {
    let invocation = match args::parse(argv) {
        Ok(invocation) => invocation,
        Err(output) => return output,
    };
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
    if command == "list" {
        return discover::run(&invocation, cdp_env);
    }
    if !spec.implemented {
        return Output::err(format!("playwright-cli {command}: not implemented yet\n"));
    }
    commands::run_command(&invocation, cdp_env)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(pending.contains("dblclick"));
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
        assert!(stdout.contains(
            "Connect, in order: --cdp <url> wins; else SLICC_CDP_URL; else a seven-profiles DevToolsActivePort; else the endpoint in .playwright-cli/session.json. Host list, and open when that port is absent or unreachable, GET /json/version and /json/list on 127.0.0.1:9222-9229. A reused browser is written to that session file. A ws:// or wss:// value there is dialed directly. Host open and tab-new treat an unreachable value there as absent. Launch never binds those ports. The wasm build still uses port 9222.\n"
        ));
        assert!(stdout.contains(
            "ws:// and wss:// are dialed directly. An http:// URL is /json/version discovery. The debugger path is opaque.\n"
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
            "  type <text>                 type text into editable element",
            "  click <target> [button]     perform click on a web page",
            "  fill <target> <text>        fill text into editable element",
            "  snapshot [target]           capture page snapshot to obtain element ref",
            "  eval <func> [target]        evaluate javascript expression on page or element",
            "  press <key>                 press a key on the keyboard, `a`, `arrowleft`",
            "  screenshot [target]         screenshot of the current page or element",
            "  tab-list                    list all tabs",
            "  tab-new [url]               create a new tab",
            "  tab-close [index]           close a browser tab",
            "  tab-select <index>          select a browser tab",
        ] {
            assert!(stdout.contains(line), "{line}");
        }
        assert!(!stdout.contains("-s=<session>"));
        assert!(!stdout.contains("--session"));
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
        for name in ["dblclick", "go-back", "pdf"] {
            assert!(pending_names.contains(&name), "{name}");
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
            "playwright-cli tab-select <index>\n\nSelect a browser tab\n\nArguments:\n  <index>                     tab index\n"
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
        assert_eq!(
            execute(&["dblclick".into(), "--help".into()], None).stdout,
            "playwright-cli dblclick <ref> <button>\n\nNot implemented yet.\n"
        );

        for args in [
            vec!["open", "--persistent"],
            vec!["snapshot", "--depth", "1"],
            vec!["snapshot", "--boxes"],
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
        let tab_index = execute(&["tab-close".into(), "1".into()], None);
        assert_eq!(tab_index.code, 1);
        assert!(tab_index.stderr.contains("too many arguments"));
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
}
