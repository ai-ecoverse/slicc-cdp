mod action;
mod body;
mod page;
mod parse;
mod profiles;
mod request;
mod response;
mod sanitize;
mod sign;
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

fn connect_start(
    cdp: Option<&str>,
    env: Option<&str>,
) -> Result<cdp_client::connect::Start, String> {
    if host_endpoint_missing(cdp, env) {
        return profile_start(profiles::attached_port());
    }
    cdp_client::connect::choose_start(cdp, env)
}

fn profile_start(
    found: Result<Option<String>, String>,
) -> Result<cdp_client::connect::Start, String> {
    match found {
        Ok(Some(url)) => Ok(cdp_client::connect::Start::Discover(url)),
        Ok(None) => Err("curlwright: no debugger is connected\n".to_string()),
        Err(message) => Err(message),
    }
}

fn host_endpoint_missing(cdp: Option<&str>, env: Option<&str>) -> bool {
    if cfg!(target_arch = "wasm32") {
        return false;
    }
    trimmed_nonempty(cdp).is_none() && trimmed_nonempty(env).is_none()
}

fn trimmed_nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

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

fn destination(url: &str) -> String {
    url::Url::parse("https://relative.invalid/").unwrap().join(url)
        .map(|url| url.to_string()).unwrap_or_else(|_| url.to_string())
}

fn reaches_protected(url: &str) -> bool {
    cdp_client::protected::Protected::default().url(&destination(url))
}

fn redact_url(url: &str, full: bool) -> String {
    if full {
        url.to_string()
    } else {
        cdp_client::sanitize::redact_url(url)
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
    let signer = match signer_for(&opts) {
        Ok(signer) => signer,
        Err(stop) => return Output::from_stop(stop),
    };
    let body = match body::resolve_body(&opts) {
        Ok(body) => body,
        Err(stop) => return Output::from_stop(stop),
    };
    let mut request = match prepare(&opts, body) {
        Ok(request) => request,
        Err(stop) => return Output::from_stop(stop),
    };
    if reaches_protected(&request.url) {
        return Output::plain(
            format!("curlwright: {}\n", cdp_client::protected::Protected::default().refusal_for_url(&destination(&request.url))),
            1,
        );
    }
    if let Some(Err(message)) = signer.as_ref().map(|signer| signer.apply(&mut request)) {
        return Output::from_stop(Stop::usage(message.trim_end()));
    }
    let start = match connect_start(opts.cdp.as_deref(), cdp_env) {
        Ok(start) => start,
        Err(message) => return Output::plain(message, 1),
    };
    let mut cdp = match cdp_client::cdp::open(start, opts.runtime.as_deref()) {
        Ok(cdp) => cdp,
        Err(message) => return Output::plain(message, 1),
    };
    let mut report = action::Report::request(&request.method, &request.url);
    let (target, tab_page_url) = match tab::resolve_tab_with_url(&mut cdp, &request.url, opts.tab.as_deref(), opts.full_urls) {
        Ok(result) => result,
        Err(stop) => {
            let output = Output::from_stop(stop);
            report.finish(&mut cdp, output.code, annotation_stderr(&output.stderr));
            return noted(&mut cdp, output);
        }
    };
    let effective_base = tab::effective_url(&request.url, tab_page_url.as_deref())
        .unwrap_or_else(|| request.url.clone());
    report.tab(&target);
    report.begin(&mut cdp);
    let session = match page::attach(&mut cdp, &target) {
        Ok(session) => session,
        Err(message) => {
            let output = Output::plain(message, 1);
            report.finish(&mut cdp, output.code, annotation_stderr(&output.stderr));
            return noted(&mut cdp, output);
        }
    };
    let context = if let Some(frame) = opts.frame.as_deref().filter(|frame| !frame.is_empty()) {
        match page::frame_context(&mut cdp, &session, frame) {
            Ok(id) => Some(id),
            Err(stop) => {
                page::detach(&mut cdp, &session);
                let output = Output::from_stop(stop);
                report.finish(&mut cdp, output.code, annotation_stderr(&output.stderr));
                return noted(&mut cdp, output);
            }
        }
    } else {
        None
    };
    let trace = if opts.verbose { format_request_trace(&request, opts.full_urls, Some(&effective_base)) } else { String::new() };
    let started = Instant::now();
    let fetched = page::run_fetch(&mut cdp, &session, &request, context, opts.full_urls, signer.as_ref());
    let elapsed = started.elapsed().as_secs_f64();
    page::detach(&mut cdp, &session);
    let output = match fetched {
        Ok(result) => render(&opts, &request, &result, elapsed, &trace, &effective_base),
        Err(err) => render_failure(&opts, &request, &err, elapsed, &trace, &effective_base),
    };
    report.finish(&mut cdp, output.code, annotation_stderr(&output.stderr));
    noted(&mut cdp, output)
}

fn signer_for(opts: &Options) -> Result<Option<sign::Signer>, Stop> {
    let Some(path) = opts.sign_key.as_deref() else {
        if opts.signature_agent.is_some() || opts.signature_agent_legacy {
            return Err(Stop::usage("curlwright: --signature-agent needs --sign-key"));
        }
        return Ok(None);
    };
    if opts.signature_agent_legacy && opts.signature_agent.is_none() {
        return Err(Stop::usage("curlwright: --signature-agent-legacy needs --signature-agent"));
    }
    if let Some(header) = opts.headers.iter().find(|header| sign::HEADERS.iter().any(|name| name.eq_ignore_ascii_case(&header.name))) {
        return Err(Stop::usage(format!("curlwright: -H {} cannot be combined with --sign-key, which sets it", header.name)));
    }
    sign::load(path, opts.signature_agent.as_deref(), opts.signature_agent_legacy).map(Some)
}

fn noted(cdp: &mut cdp_client::cdp::Cdp, mut output: Output) -> Output {
    for notice in cdp.take_notices() {
        output.stderr.push_str(&notice);
    }
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

fn completed_write_out(request: &PreparedRequest, result: &FetchResult, elapsed: f64, exit_code: i32, full_urls: bool, effective_base: &str) -> WriteOut {
    WriteOut {
        url_effective: redact_url(if result.url.is_empty() { &request.url } else { &result.url }, full_urls),
        http_code: i64::from(result.status),
        content_type: content_type_of(&result.headers),
        size_download: result.body.len(),
        size_header: header_block_size(result.status, &result.status_text, &result.headers, true, None),
        size_upload: request.upload_size,
        method: request.method.clone(),
        num_redirects: if result.redirected { 1 } else { 0 },
        time_total_seconds: elapsed,
        exit_code,
        error_msg: String::new(),
        response_headers: result.headers.clone(),
        full_urls,
        base_url: effective_base.to_string(),
    }
}

fn failed_write_out(request: &PreparedRequest, elapsed: f64, exit_code: i32, error_msg: &str, full_urls: bool, effective_base: &str) -> WriteOut {
    WriteOut {
        url_effective: redact_url(&request.url, full_urls),
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
        full_urls,
        base_url: effective_base.to_string(),
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

fn render_failure(opts: &Options, request: &PreparedRequest, err: &str, elapsed: f64, trace: &str, effective_base: &str) -> Output {
    let (message, error_msg, code) = classify(err);
    let mut stdout = Vec::new();
    let mut messages = format!("{message}\n");
    append_write_out(opts, &mut stdout, &mut messages, &failed_write_out(request, elapsed, code, &error_msg, opts.full_urls, effective_base));
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

fn render(opts: &Options, request: &PreparedRequest, result: &FetchResult, elapsed: f64, trace: &str, effective_base: &str) -> Output {
    let mut stdout = Vec::new();
    let mut messages = String::new();
    let mut response_trace = String::new();
    let line = status_line(result.status, &result.status_text);
    if opts.verbose {
        response_trace = format_response_trace(&line, &result.headers, opts.full_urls, Some(&effective_base));
    }
    if opts.include || opts.head {
        stdout.extend_from_slice(
            format_header_block(result.status, &result.status_text, &result.headers, opts.full_urls, Some(&effective_base)).as_bytes(),
        );
    }
    let mut dump_code = 0;
    if let Some(path) = &opts.dump_header {
        let block = format_header_block(result.status, &result.status_text, &result.headers, opts.full_urls, Some(&effective_base));
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
    append_write_out(opts, &mut stdout, &mut messages, &completed_write_out(request, result, elapsed, code, opts.full_urls, effective_base));
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
    fn protected_urls_are_refused_before_any_tab_is_chosen() {
        for url in [
            "https://vault.sliccy.ai/unlock",
            "http://seven.sliccy.ai/x",
            "HTTPS://SEVEN.SLICCY.AI",
            "//vault.sliccy.ai/x",
            "\\\\vault.sliccy.ai\\x",
            "blob:https://vault.sliccy.ai/1",
        ] {
            let output = run(&["--cdp", "ws://127.0.0.1:1/devtools/browser/x", url]);
            assert_eq!(output.code, 1, "{url}");
            assert_eq!(
                output.stderr,
                "curlwright: slicc: refused: target is on seven's origin\n",
                "{url}"
            );
        }
        assert!(!super::reaches_protected("https://notsliccy.ai/"));
        assert!(!super::reaches_protected("/api/me"));
        assert!(!super::reaches_protected("https://app.example/?next=https://vault.sliccy.ai/"));
    }

    #[test]
    fn malformed_absolute_and_authority_urls_fail_closed() {
        for url in [
            "http://exa%20mple.sliccy.ai/",
            "http://exa mple.sliccy.ai/",
            "//exa%20mple.sliccy.ai/",
            "http://xn--1ug.sliccy.ai/",
            "http://xn--a.sliccy.ai/",
        ] {
            let output = run(&["--cdp", "ws://127.0.0.1:1/devtools/browser/x", url]);
            assert_eq!(output.code, 1, "{url}");
            assert_eq!(output.stderr, "curlwright: slicc: refused: slicc can't parse this URL, so it could be a protected page\n", "{url}");
        }
        for url in ["/api/me", "../api/me", "//app.example/api/me"] {
            assert!(!super::reaches_protected(url), "{url}");
        }
        for url in ["http://vault%2Esliccy.ai/", "http://vau%6Ct.sliccy.ai/", "http://VAULT.sliccy.ai/", "http://vault.sliccy.ai./", "http://seven.sliccy.ai./"] {
            assert!(super::reaches_protected(url), "{url}");
        }
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
            "-v, --verbose", "-f, --fail", "-m, --max-time", "--sign-key",
            "--signature-agent <url>", "--signature-agent-legacy",
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
    fn host_refuses_an_unnamed_debugger() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let missing = connect_start(None, None).unwrap_err();
            assert_eq!(missing, "curlwright: no debugger is connected\n");
            let blank = connect_start(Some(" "), Some("")).unwrap_err();
            assert_eq!(blank, "curlwright: no debugger is connected\n");
            assert_eq!(
                connect_start(Some("http://127.0.0.1:9225"), None).unwrap(),
                cdp_client::connect::Start::Discover(
                    "http://127.0.0.1:9225/json/version".to_string()
                )
            );
            let output = run(&["https://app.example/x"]);
            assert_eq!(output.code, 1);
            assert_eq!(output.stderr, "curlwright: no debugger is connected\n");
            assert!(!output.stderr.contains("9222"));
        }
        #[cfg(target_arch = "wasm32")]
        {
            assert_eq!(
                connect_start(None, None).unwrap(),
                cdp_client::connect::Start::Discover(
                    "http://127.0.0.1:9222/json/version".to_string()
                )
            );
        }
    }

    #[test]
    fn a_live_seven_profile_is_discovery() {
        let url = "http://127.0.0.1:9333/json/version".to_string();
        assert_eq!(
            profile_start(Ok(Some(url.clone()))).unwrap(),
            cdp_client::connect::Start::Discover(url)
        );
        let missing = profile_start(Ok(None)).unwrap_err();
        assert_eq!(missing, "curlwright: no debugger is connected\n");
        assert!(!missing.contains("9222"));
        assert_eq!(
            profile_start(Err("HTTP 500\nno\n".to_string())).unwrap_err(),
            "HTTP 500\nno\n"
        );
    }

    #[test]
    fn signing_flags_fail_before_connect_and_never_echo_the_key() {
        let dir = std::env::temp_dir().join(format!("curlwright-sign-flags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let secret = "n4Ni-HpISpVObnQMW0wOhCKROaIKqKtW_2ZYb2p9KcU";
        let good = dir.join("good.jwk");
        std::fs::write(&good, format!("{{\"kty\":\"OKP\",\"crv\":\"Ed25519\",\"d\":\"{secret}\"}}")).unwrap();
        let rsa = dir.join("rsa.jwk");
        std::fs::write(&rsa, format!("{{\"kty\":\"RSA\",\"d\":\"{secret}\"}}")).unwrap();
        let good = good.to_str().unwrap();
        let rsa = rsa.to_str().unwrap();
        let cdp = "ws://127.0.0.1:1/devtools/browser/x";
        let cases: [(Vec<&str>, i32, &str); 8] = [
            (vec!["--signature-agent", "https://agent.test", "https://app.example/"], 2, "--signature-agent needs --sign-key"),
            (vec!["--signature-agent-legacy", "https://app.example/"], 2, "--signature-agent needs --sign-key"),
            (vec!["--sign-key", good, "--signature-agent-legacy", "https://app.example/"], 2, "--signature-agent-legacy needs --signature-agent"),
            (vec!["--sign-key", good, "-H", "signature-input: x", "https://app.example/"], 2, "-H signature-input cannot be combined with --sign-key"),
            (vec!["--sign-key", good, "--signature-agent", "http://agent.test", "https://app.example/"], 2, "--signature-agent must be an https URL"),
            (vec!["--sign-key", rsa, "https://app.example/"], 2, "is not an Ed25519 key"),
            (vec!["--sign-key", "/no/such/curlwright-key.jwk", "https://app.example/"], 26, "cannot read --sign-key /no/such/curlwright-key.jwk"),
            (vec!["--sign-key", good, "/api/me"], 2, "cannot sign a request without an absolute http(s) URL"),
        ];
        for (args, code, message) in cases {
            let mut argv = vec!["--cdp", cdp];
            argv.extend(args);
            let output = run(&argv);
            assert_eq!(output.code, code, "{}", output.stderr);
            assert!(output.stderr.contains(message), "{}", output.stderr);
            assert!(!output.stderr.contains(secret), "{}", output.stderr);
            assert!(output.stdout.is_empty());
        }
        std::fs::remove_dir_all(&dir).unwrap();
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
