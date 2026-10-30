use crate::args::{Invocation, Output};
use cdp_client::upgrade::{self, Tool};

pub const TOOL: Tool = Tool {
    name: "playwright-cli",
    version: env!("PLAYWRIGHT_CLI_VERSION"),
    package: "slicc-cdp",
};

fn flag(invocation: &Invocation, name: &str) -> bool {
    invocation
        .flags
        .get(name)
        .is_some_and(|value| value != "false")
}

pub fn run(invocation: &Invocation) -> Output {
    let command = invocation.command.as_deref().unwrap_or("upgrade");
    let report = upgrade::run(&TOOL, command, flag(invocation, "check"), flag(invocation, "force"));
    Output {
        stdout: report.stdout,
        stderr: report.stderr,
        code: report.code,
    }
}

#[cfg(windows)]
pub fn remove_parked() {
    upgrade::remove_parked();
}
