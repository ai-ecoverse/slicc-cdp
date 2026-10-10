mod action;
mod body;
mod page;
mod parse;
mod request;
mod response;
mod tab;
mod write_out;

use body::resolve_path;
use page::FetchResult;
use parse::{parse, validate_headers, Options, Stop};
use request::{format_request_trace, format_response_trace, prepare, PreparedRequest};
use response::{format_header_block, header_block_size, looks_binary, remote_name, status_line, BINARY_OUTPUT_WARNING};
use std::time::Instant;
use write_out::{format_write_out, WriteOut};

const HELP: &str = include_str!("help.txt");

pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub code: i32,
}

impl Output {
    pub fn as_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    fn from_stop(stop: Stop) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: format!("{}\n", stop.message),
            code: stop.code,
        }
    }

    fn plain(stderr: String, code: i32) -> Self {
        Self { stdout: Vec::new(), stderr, code }
    }

    fn text(stdout: String) -> Self {
        Self { stdout: stdout.into_bytes(), stderr: String::new(), code: 0 }
    }
}

pub fn execute(argv: &[String], cdp_env: Option<&str>) -> Output {
    if argv.is_empty() {
        return Output::from_stop(Stop::usage(
            "curlwright: no URL specified\nRun 'curlwright --help' for usage.",
        ));
    }
    let opts = match parse(argv) {
        Ok(opts) => opts,
        Err(stop) => return Output::from_stop(stop),
    };
    if opts.version {
        return Output::text(format!("curlwright {}\n", env!("CURLWRIGHT_VERSION")));
    }
    if opts.help {
        return Output::text(HELP.to_string());
    }
    if opts.url.is_none() {
        return Output::from_stop(Stop::usage("curlwright: no URL specified"));
    }
    if let Err(stop) = validate_headers(&opts.headers) {
        return Output::from_stop(stop);
    }
    let body = match body::resolve_body(&opts) {
        Ok(body) => body,
        Err(stop) => return Output::from_stop(stop),
    };
    let request = match prepare(&opts, body) {
        Ok(request) => request,
        Err(stop) => return Output::from_stop(stop),
    };
    let start = match cdp_client::connect::choose_start(opts.cdp.as_deref(), cdp_env) {
        Ok(start) => start,
        Err(message) => return Output::plain(message, 1),
    };
    let mut cdp = match cdp_client::cdp::open(start, opts.runtime.as_deref()) {
        Ok(cdp) => cdp,
        Err(message) => return Output::plain(message, 1),
    };
    let mut report = action::Report::request(&request.method, &request.url);
    let target = match tab::resolve_tab(&mut cdp, &request.url, opts.tab.as_deref()) {
        Ok(target) => target,
        Err(stop) => {
            let output = Output::from_stop(stop);
            report.finish(&mut cdp, output.code, annotation_stderr(&output.stderr));
            return output;
        }
    };
    report.tab(&target);
    report.begin(&mut cdp);
    let session = match page::attach(&mut cdp, &target) {
        Ok(session) => session,
        Err(message) => {
            let output = Output::plain(message, 1);
            report.finish(&mut cdp, output.code, annotation_stderr(&output.stderr));
            return output;
        }
    };
    let context = if let Some(frame) = opts.frame.as_deref().filter(|frame| !frame.is_empty()) {
        match page::frame_context(&mut cdp, &session, frame) {
            Ok(id) => Some(id),
            Err(stop) => {
                page::detach(&mut cdp, &session);
                let output = Output::from_stop(stop);
                report.finish(&mut cdp, output.code, annotation_stderr(&output.stderr));
                return output;
            }
        }
    } else {
        None
    };
    let trace = if opts.verbose { format_request_trace(&request) } else { String::new() };
    let started = Instant::now();
    let fetched = page::run_fetch(&mut cdp, &session, &request, context);
    let elapsed = started.elapsed().as_secs_f64();
    page::detach(&mut cdp, &session);
    let output = match fetched {
        Ok(result) => render(&opts, &request, &result, elapsed, &trace),
        Err(err) => render_failure(&opts, &request, &err, elapsed, &trace),
    };
    report.finish(&mut cdp, output.code, annotation_stderr(&output.stderr));
    output
}

fn annotation_stderr(stderr: &str) -> &str {
    let mut rest = stderr;
    loop {
        let Some(line) = rest.lines().next() else {
            return "";
        };
        if !(line.starts_with("> ") || line.starts_with("< ")) {
            return rest;
        }
        rest = &rest[line.len()..];
        if let Some(stripped) = rest.strip_prefix("\r\n") {
            rest = stripped;
        } else if let Some(stripped) = rest.strip_prefix('\n') {
            rest = stripped;
        }
    }
}

fn content_type_of(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

fn completed_write_out(request: &PreparedRequest, result: &FetchResult, elapsed: f64, exit_code: i32) -> WriteOut {
    WriteOut {
        url_effective: if result.url.is_empty() { request.url.clone() } else { result.url.clone() },
        http_code: i64::from(result.status),
        content_type: content_type_of(&result.headers),
        size_download: result.body.len(),
        size_header: header_block_size(result.status, &result.status_text, &result.headers),
        size_upload: request.upload_size,
        method: request.method.clone(),
        num_redirects: if result.redirected { 1 } else { 0 },
        time_total_seconds: elapsed,
        exit_code,
        error_msg: String::new(),
        response_headers: result.headers.clone(),
    }
}

fn failed_write_out(request: &PreparedRequest, elapsed: f64, exit_code: i32, error_msg: &str) -> WriteOut {
    WriteOut {
        url_effective: request.url.clone(),
        http_code: 0,
        content_type: String::new(),
        size_download: 0,
        size_header: 0,
        size_upload: request.upload_size,
        method: request.method.clone(),
        num_redirects: 0,
        time_total_seconds: elapsed,
        exit_code,
        error_msg: error_msg.to_string(),
        response_headers: Vec::new(),
    }
}

fn append_write_out(opts: &Options, stdout: &mut Vec<u8>, messages: &mut String, ctx: &WriteOut) {
    let Some(format) = &opts.write_out else { return };
    let (text, warnings) = format_write_out(format, ctx);
    stdout.extend_from_slice(text.as_bytes());
    for warning in warnings {
        messages.push_str(&warning);
        messages.push('\n');
    }
}

fn classify(raw: &str) -> (String, String, i32) {
    let raw = raw.trim_end_matches(['\n', '\r']);
    let lower = raw.to_ascii_lowercase();
    if lower.contains("timeouterror")
        || lower.contains("timed out")
        || lower.contains("aborted")
    {
        let error_msg = "Operation timed out".to_string();
        return (format!("curlwright: (28) {error_msg}"), error_msg, 28);
    }
    if lower.contains("failed to fetch") || lower.contains("networkerror") || lower.contains("typeerror") {
        let error_msg = format!("Failed to fetch — {raw}");
        return (format!("curlwright: (7) {error_msg}"), error_msg, 7);
    }
    (format!("curlwright: {raw}"), raw.to_string(), 1)
}

fn render_failure(opts: &Options, request: &PreparedRequest, err: &str, elapsed: f64, trace: &str) -> Output {
    let (message, error_msg, code) = classify(err);
    let mut stdout = Vec::new();
    let mut messages = format!("{message}\n");
    append_write_out(opts, &mut stdout, &mut messages, &failed_write_out(request, elapsed, code, &error_msg));
    let quiet = opts.silent && !opts.show_error;
    Output {
        stdout,
        stderr: format!("{trace}{}", if quiet { "" } else { &messages }),
        code,
    }
}

enum Dest {
    ImpliedStdout,
    ForcedStdout,
    File(String),
    Error(String),
}

fn output_dest(opts: &Options, url: &str) -> Dest {
    if let Some(output) = &opts.output {
        if output == "-" {
            return Dest::ForcedStdout;
        }
        return Dest::File(output.clone());
    }
    if opts.remote_name {
        return match remote_name(url) {
            Some(name) => Dest::File(name),
            None => Dest::Error("curlwright: -O requires a URL with a file name in its path".to_string()),
        };
    }
    Dest::ImpliedStdout
}

fn write_file(target: &str, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(resolve_path(target), bytes)
        .map_err(|err| format!("curlwright: (23) failed writing {target}: {err}"))
}

fn emit_body(opts: &Options, messages: &mut String, stdout: &mut Vec<u8>, bytes: &[u8], url: &str) -> i32 {
    match output_dest(opts, url) {
        Dest::Error(error) => {
            messages.push_str(&error);
            messages.push('\n');
            2
        }
        Dest::File(target) => match write_file(&target, bytes) {
            Ok(()) => 0,
            Err(error) => {
                messages.push_str(&error);
                messages.push('\n');
                23
            }
        },
        Dest::ImpliedStdout if looks_binary(bytes) => {
            messages.push_str(BINARY_OUTPUT_WARNING);
            0
        }
        Dest::ImpliedStdout | Dest::ForcedStdout => {
            stdout.extend_from_slice(bytes);
            0
        }
    }
}

fn render(opts: &Options, request: &PreparedRequest, result: &FetchResult, elapsed: f64, trace: &str) -> Output {
    let mut stdout = Vec::new();
    let mut messages = String::new();
    let mut response_trace = String::new();
    let line = status_line(result.status, &result.status_text);
    if opts.verbose {
        response_trace = format_response_trace(&line, &result.headers);
    }
    if opts.include || opts.head {
        stdout.extend_from_slice(
            format_header_block(result.status, &result.status_text, &result.headers).as_bytes(),
        );
    }
    let mut dump_code = 0;
    if let Some(path) = &opts.dump_header {
        let block = format_header_block(result.status, &result.status_text, &result.headers);
        if let Err(error) = write_file(path, block.as_bytes()) {
            messages.push_str(&error);
            messages.push('\n');
            dump_code = 23;
        }
    }
    let mut code = 0;
    if opts.fail_on_error && result.status >= 400 {
        messages.push_str(&format!("curlwright: (22) The requested URL returned error: {}\n", result.status));
        code = 22;
    } else if !opts.head {
        code = emit_body(opts, &mut messages, &mut stdout, &result.body, &request.url);
    }
    if code == 0 {
        code = dump_code;
    }
    append_write_out(opts, &mut stdout, &mut messages, &completed_write_out(request, result, elapsed, code));
    let quiet = opts.silent && !opts.show_error;
    Output {
        stdout,
        stderr: format!("{trace}{response_trace}{}", if quiet { "" } else { &messages }),
        code,
    }
}

#[cfg(test)]
mod harness;

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Output {
        let argv: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        execute(&argv, None)
    }

    #[test]
    fn annotation_stderr_skips_the_verbose_trace() {
        let stderr = "> GET https://app.example/nope?token=secret\n> Authorization: secret-token\n> \n< HTTP/1.1 500 ERR\n< \ncurlwright: (22) The requested URL returned error: 500\n";
        assert_eq!(
            annotation_stderr(stderr),
            "curlwright: (22) The requested URL returned error: 500\n"
        );
        assert_eq!(
            annotation_stderr("curlwright: (7) Failed to fetch\n"),
            "curlwright: (7) Failed to fetch\n"
        );
        assert_eq!(annotation_stderr("> GET https://app.example/\n> \n"), "");
    }

    #[test]
    fn help_matches_slicc_text() {
        let output = run(&["--help"]);
        assert_eq!(output.code, 0);
        assert_eq!(output.stderr, "");
        assert_eq!(output.as_text(), HELP);
        for flag in [
            "-X, --request", "-H, --header", "-d, --data", "--data-raw", "--data-binary",
            "--data-urlencode", "--json", "-F, --form", "--form-string", "-G, --get",
            "-u, --user", "-e, --referer", "-r, --range", "--no-credentials", "--tab",
            "--frame", "-o, --output", "-O, --remote-name", "-i, --include", "-I, --head",
            "-D, --dump-header", "-w, --write-out", "-s, --silent", "-S, --show-error",
            "-v, --verbose", "-f, --fail", "-m, --max-time",
        ] {
            assert!(output.as_text().contains(flag), "{flag}");
        }
    }

    #[test]
    fn version_prints_package_version_without_a_url() {
        let long = run(&["--version"]);
        let short = run(&["-V"]);
        let expected = format!("curlwright {}\n", env!("CURLWRIGHT_VERSION"));
        assert_eq!(long.code, 0);
        assert_eq!(long.stderr, "");
        assert_eq!(long.as_text(), expected);
        assert_eq!(short.as_text(), expected);
        assert_eq!(short.code, 0);
    }

    #[test]
    fn dash_output_keeps_raw_bytes() {
        let opts = parse(&["-o".into(), "-".into(), "https://app.example/x".into()]).unwrap();
        let mut messages = String::new();
        let mut stdout = Vec::new();
        let bytes = [0xff, 0xfe, b'A'];
        let code = emit_body(&opts, &mut messages, &mut stdout, &bytes, "https://app.example/x");
        assert_eq!(code, 0);
        assert!(messages.is_empty());
        assert_eq!(stdout, bytes);
    }

    #[test]
    fn a_cdp_deadline_is_a_timeout() {
        let (_, _, code) = classify("Runtime.evaluate timed out\n");
        assert_eq!(code, 28);
    }

    #[test]
    fn usage_exits_before_connect() {
        let empty = run(&[]);
        assert_eq!(empty.code, 2);
        assert!(empty.stderr.contains("no URL specified"));
        let get_body = run(&["-X", "GET", "-d", "a=b", "https://app.example/x"]);
        assert_eq!(get_body.code, 2);
        assert!(get_body.stderr.contains("cannot carry a request body"));
        let head_body = run(&["-I", "-d", "a=b", "https://app.example/x"]);
        assert_eq!(head_body.code, 2);
        let mixed = run(&["-F", "a=b", "-d", "c", "https://app.example/x"]);
        assert_eq!(mixed.code, 2);
        assert!(mixed.stderr.contains("-F cannot be combined with -d"));
        let missing = run(&["-d", "@/no/such/curlwright-file", "https://app.example/x"]);
        assert_eq!(missing.code, 26);
        assert!(missing.stderr.contains("cannot read"));
    }
}
