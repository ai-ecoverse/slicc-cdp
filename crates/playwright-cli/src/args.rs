use crate::help;
use crate::spec::{self, Spec};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct Invocation {
    pub command: Option<String>,
    pub positionals: Vec<String>,
    pub flags: BTreeMap<String, String>,
    pub multi: BTreeMap<String, Vec<String>>,
    pub cdp: Option<String>,
    pub runtime: Option<String>,
    pub session: Option<String>,
    pub full_urls: bool,
    pub help: bool,
    pub command_help: bool,
    pub version: bool,
}

impl Invocation {
    pub fn repeated(&self, name: &str) -> Vec<&str> {
        self.multi
            .get(name)
            .map(|values| values.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }
}

#[derive(Debug)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

impl Output {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: String::new(),
            code: 0,
        }
    }

    pub fn err(stderr: impl Into<String>) -> Self {
        let mut stderr = stderr.into();
        if !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        Self {
            stdout: String::new(),
            stderr,
            code: 1,
        }
    }
}

pub fn parse(args: &[String]) -> Result<Invocation, Output> {
    let mut rest = Vec::new();
    let mut cdp = None;
    let mut runtime = None;
    let mut session = None;
    let mut help = false;
    let mut version = false;
    let mut full_urls = false;
    let mut index = 0;
    while index < args.len() {
        let token = &args[index];
        if token == "--" {
            rest.extend(args[index + 1..].iter().cloned());
            break;
        }
        if token == "--version" || token == "-V" {
            version = true;
            index += 1;
            continue;
        }
        if token == "-h" || token == "--help" {
            help = true;
            index += 1;
            continue;
        }
        if token == "--full-urls" {
            full_urls = true;
            index += 1;
            continue;
        }
        if let Some(value) = inline_value(token, "--cdp") {
            cdp = Some(value);
            index += 1;
            continue;
        }
        if token == "--cdp" {
            index += 1;
            let value = args.get(index).cloned().ok_or_else(|| {
                Output::err("playwright-cli: --cdp requires a URL\n")
            })?;
            cdp = Some(value);
            index += 1;
            continue;
        }
        if let Some(value) = inline_value(token, "--runtime") {
            runtime = Some(value);
            index += 1;
            continue;
        }
        if token == "--runtime" {
            index += 1;
            let value = args.get(index).cloned().ok_or_else(|| {
                Output::err("playwright-cli: --runtime requires a name\n")
            })?;
            runtime = Some(value);
            index += 1;
            continue;
        }
        if let Some(value) = inline_value(token, "-s") {
            session = Some(value);
            index += 1;
            continue;
        }
        if token == "-s" {
            index += 1;
            let value = args.get(index).cloned().ok_or_else(|| {
                Output::err("playwright-cli: -s requires a session name\n")
            })?;
            session = Some(value);
            index += 1;
            continue;
        }
        if let Some(value) = inline_value(token, "--session") {
            session = Some(value);
            index += 1;
            continue;
        }
        if token == "--session" {
            index += 1;
            let value = args.get(index).cloned().ok_or_else(|| {
                Output::err("playwright-cli: --session requires a name\n")
            })?;
            session = Some(value);
            index += 1;
            continue;
        }
        rest.push(token.clone());
        index += 1;
    }
    let command = rest.first().cloned().filter(|token| !token.starts_with('-'));
    let after = if command.is_some() {
        rest.get(1..).unwrap_or(&[])
    } else {
        rest.as_slice()
    };
    if command.is_none() {
        if let Some(token) = after.first() {
            if token.starts_with('-') {
                return Err(Output {
                    stdout: String::new(),
                    stderr: format!("Unknown option: {token}\n\n"),
                    code: 1,
                });
            }
        }
        return Ok(Invocation {
            command: None,
            positionals: Vec::new(),
            flags: BTreeMap::new(),
            multi: BTreeMap::new(),
            cdp,
            runtime,
            session,
            full_urls,
            help: help || command.is_none(),
            command_help: false,
            version,
        });
    }
    let spec = spec::lookup(command.as_deref().unwrap());
    let mut positionals = Vec::new();
    let mut flags = BTreeMap::new();
    let mut multi: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut command_help = false;
    let mut index = 0;
    if spec.is_none() {
        command_help = after.iter().any(|token| token == "--help" || token == "-h");
        return Ok(Invocation {
            command,
            positionals,
            flags,
            multi,
            cdp,
            runtime,
            session,
            full_urls,
            help,
            command_help,
            version,
        });
    }
    while index < after.len() {
        let token = &after[index];
        if token == "--" {
            positionals.extend(after[index + 1..].iter().cloned());
            break;
        }
        if token == "--version" || token == "-V" {
            version = true;
            index += 1;
            continue;
        }
        if token == "-h" || token == "--help" {
            command_help = true;
            index += 1;
            continue;
        }
        if let Some(name) = token.strip_prefix("--") {
            let (name, inline) = match name.split_once('=') {
                Some((name, value)) => (name.to_string(), Some(value.to_string())),
                None => (name.to_string(), None),
            };
            if name.is_empty() {
                return Err(unknown_option(token, spec));
            }
            let is_bool = spec
                .and_then(|spec| spec::flag_is_bool(spec, &name))
                .ok_or_else(|| unknown_option(&format!("--{name}"), spec))?;
            let value = if is_bool {
                if let Some(value) = inline {
                    value
                } else {
                    "true".to_string()
                }
            } else if let Some(value) = inline {
                value
            } else {
                index += 1;
                after.get(index).cloned().ok_or_else(|| {
                    Output::err(format!("playwright-cli: --{name} requires a value\n"))
                })?
            };
            multi.entry(name.clone()).or_default().push(value.clone());
            flags.insert(name, value);
            index += 1;
            continue;
        }
        if token.starts_with('-') && token != "-" {
            return Err(unknown_option(token, spec));
        }
        positionals.push(token.clone());
        index += 1;
    }
    Ok(Invocation {
        command,
        positionals,
        flags,
        multi,
        cdp,
        runtime,
        session,
        full_urls,
        help,
        command_help,
        version,
    })
}

fn inline_value(token: &str, flag: &str) -> Option<String> {
    token
        .strip_prefix(flag)
        .and_then(|rest| rest.strip_prefix('='))
        .map(str::to_string)
}

fn unknown_option(token: &str, spec: Option<&Spec>) -> Output {
    let help_text = spec
        .map(help::command_help)
        .unwrap_or_else(help::global_help);
    Output {
        stdout: format!("\n{help_text}\n"),
        stderr: format!("Unknown option: {token}\n"),
        code: 1,
    }
}

pub fn validate(invocation: &Invocation, spec: &Spec) -> Result<(), Output> {
    if invocation.positionals.len() > spec.args.len() && !spec.variadic {
        let help_text = help::command_help(spec);
        return Err(Output {
            stdout: format!("\n{help_text}\n"),
            stderr: format!(
                "error: too many arguments: expected {}, received {}\n",
                spec.args.len(),
                invocation.positionals.len()
            ),
            code: 1,
        });
    }
    if let Some(message) = spec::ref_error(&spec.name, spec, &invocation.positionals) {
        return Err(Output::err(message));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_command_shape() {
        let invocation = parse(&["nope".into()]).unwrap();
        assert_eq!(invocation.command.as_deref(), Some("nope"));
        assert!(spec::lookup("nope").is_none());
    }

    #[test]
    fn cdp_and_runtime_are_global() {
        let invocation = parse(&[
            "--cdp".into(),
            "http://127.0.0.1:9333".into(),
            "goto".into(),
            "https://example.com".into(),
            "--runtime".into(),
            "follower".into(),
        ])
        .unwrap();
        assert_eq!(invocation.command.as_deref(), Some("goto"));
        assert_eq!(invocation.cdp.as_deref(), Some("http://127.0.0.1:9333"));
        assert_eq!(invocation.runtime.as_deref(), Some("follower"));
        assert_eq!(invocation.positionals, vec!["https://example.com"]);
    }

    #[test]
    fn too_many_arguments() {
        let invocation = parse(&["press".into(), "Enter".into(), "Shift".into()]).unwrap();
        let spec = spec::lookup("press").unwrap();
        let err = validate(&invocation, spec).unwrap_err();
        assert!(err.stderr.contains("error: too many arguments: expected 1, received 2"));
        assert!(err.stdout.contains("press"));
    }

    #[test]
    fn session_flag_long() {
        let invocation = parse(&[
            "--session".into(),
            "agent-1".into(),
            "snapshot".into(),
        ])
        .unwrap();
        assert_eq!(invocation.session.as_deref(), Some("agent-1"));
        assert_eq!(invocation.command.as_deref(), Some("snapshot"));
    }

    #[test]
    fn session_flag_short_inline() {
        let invocation = parse(&[
            "-s=my-session".into(),
            "goto".into(),
            "https://example.com".into(),
        ])
        .unwrap();
        assert_eq!(invocation.session.as_deref(), Some("my-session"));
        assert_eq!(invocation.command.as_deref(), Some("goto"));
    }

    #[test]
    fn session_flag_short_spaced() {
        let invocation = parse(&[
            "-s".into(),
            "my-session".into(),
            "snapshot".into(),
        ])
        .unwrap();
        assert_eq!(invocation.session.as_deref(), Some("my-session"));
        assert_eq!(invocation.command.as_deref(), Some("snapshot"));
    }

    #[test]
    fn session_flag_short_missing_value() {
        let err = parse(&["-s".into()]).unwrap_err();
        assert!(err.stderr.contains("-s requires a session name"));
    }

    #[test]
    fn session_flag_long_inline() {
        let invocation = parse(&[
            "--session=agent-2".into(),
            "tab-list".into(),
        ])
        .unwrap();
        assert_eq!(invocation.session.as_deref(), Some("agent-2"));
    }

    #[test]
    fn session_flag_missing_value() {
        let err = parse(&["--session".into()]).unwrap_err();
        assert!(err.stderr.contains("--session requires a name"));
    }

    #[test]
    fn session_absent_by_default() {
        let invocation = parse(&["snapshot".into()]).unwrap();
        assert!(invocation.session.is_none());
    }

    #[test]
    fn session_empty_value_is_accepted() {
        let invocation = parse(&["--session=".into(), "snapshot".into()]).unwrap();
        assert_eq!(invocation.session.as_deref(), Some(""));
    }
}
