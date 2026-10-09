mod args;
mod browser;
mod cdp;
mod commands;
mod connect;
mod help;
mod net;
mod session;
mod snapshot;
mod spec;

pub use args::Output;

pub fn execute(argv: &[String], cdp_env: Option<&str>) -> Output {
    let invocation = match args::parse(argv) {
        Ok(invocation) => invocation,
        Err(output) => return output,
    };
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
        assert!(output.stdout.contains("playwright-cli - run playwright mcp commands from terminal"));
        assert!(output.stdout.contains("Usage: playwright-cli <command> [args] [options]"));
    }

    #[test]
    fn help_lists_shared_commands() {
        let output = execute(&[], None);
        assert_eq!(output.code, 0);
        for name in ["open", "close", "goto", "snapshot", "click", "fill", "type", "press", "screenshot", "eval", "tab-list", "tab-new", "tab-select", "tab-close"] {
            assert!(output.stdout.contains(name), "{name}");
        }
        assert!(output.stdout.contains("Not implemented yet"));
        assert!(output.stdout.contains("console"));
        assert!(!output.stdout.contains("cdp.slicc.internal"));
        assert!(!output.stdout.contains("cdp.kernel.localhost"));
        assert!(!output.stdout.contains("/devtools/browser/slicc"));
    }
}
