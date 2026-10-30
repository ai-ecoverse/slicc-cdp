use crate::parse::{unknown, Stop};
use crate::Output;
use cdp_client::upgrade::{self, Tool};

const TOOL: Tool = Tool {
    name: "curlwright",
    version: env!("CURLWRIGHT_VERSION"),
    package: "curlwright",
};

pub fn is_command(argv: &[String]) -> bool {
    matches!(argv.first().map(String::as_str), Some("upgrade" | "update"))
}

pub fn run(argv: &[String]) -> Output {
    let command = argv[0].as_str();
    let (mut check, mut force) = (false, false);
    for arg in &argv[1..] {
        match arg.as_str() {
            "--check" => check = true,
            "--force" => force = true,
            "-h" | "--help" => return Output::text(upgrade::usage(&TOOL, command)),
            other if other.starts_with('-') => return Output::from_stop(unknown(other)),
            other => {
                return Output::from_stop(Stop::usage(format!("curlwright {command}: unexpected argument {other}")));
            }
        }
    }
    let report = upgrade::run(&TOOL, command, check, force);
    Output {
        stdout: report.stdout.into_bytes(),
        stderr: report.stderr,
        code: report.code,
    }
}
