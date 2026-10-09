#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataKind {
    Ascii,
    Binary,
    Raw,
    UrlEncode,
    Json,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataSpec {
    pub kind: DataKind,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormSpec {
    pub value: String,
    pub literal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderSpec {
    pub name: String,
    pub value: String,
    pub unset: bool,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub url: Option<String>,
    pub request_method: Option<String>,
    pub headers: Vec<HeaderSpec>,
    pub data: Vec<DataSpec>,
    pub form: Vec<FormSpec>,
    pub get: bool,
    pub output: Option<String>,
    pub remote_name: bool,
    pub include: bool,
    pub head: bool,
    pub dump_header: Option<String>,
    pub write_out: Option<String>,
    pub silent: bool,
    pub show_error: bool,
    pub verbose: bool,
    pub fail_on_error: bool,
    pub max_time_seconds: Option<f64>,
    pub credentials: String,
    pub tab: Option<String>,
    pub frame: Option<String>,
    pub user: Option<String>,
    pub referer: Option<String>,
    pub range: Option<String>,
    pub help: bool,
    pub version: bool,
    pub cdp: Option<String>,
    pub runtime: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stop {
    pub message: String,
    pub code: i32,
}

impl Stop {
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: 2,
        }
    }

    pub fn new(message: impl Into<String>, code: i32) -> Self {
        Self {
            message: message.into(),
            code,
        }
    }
}

fn empty_options() -> Options {
    Options {
        url: None,
        request_method: None,
        headers: Vec::new(),
        data: Vec::new(),
        form: Vec::new(),
        get: false,
        output: None,
        remote_name: false,
        include: false,
        head: false,
        dump_header: None,
        write_out: None,
        silent: false,
        show_error: false,
        verbose: false,
        fail_on_error: false,
        max_time_seconds: None,
        credentials: "include".to_string(),
        tab: None,
        frame: None,
        user: None,
        referer: None,
        range: None,
        help: false,
        version: false,
        cdp: None,
        runtime: None,
    }
}

fn rejected_reason(name: &str) -> Option<&'static str> {
    Some(match name {
        "cert" | "cert-type" | "key" => {
            "client certificates are the browser profile's to present, not the shell's"
        }
        "cacert" | "capath" => "the browser decides which roots to trust",
        "insecure" => "the browser enforces TLS verification; a page cannot opt out",
        "proxy" | "proxy-user" => "proxying is configured on the browser, not per request",
        "interface" => "a page cannot choose an outgoing interface",
        "unix-socket" => "a page cannot dial a unix socket",
        "resolve" => "a page cannot override DNS resolution",
        "compressed" => "always on — the browser negotiates encoding itself",
        "cookie" => {
            "the tab's own cookies are already sent; set one with `playwright-cli cookie-set`"
        }
        "cookie-jar" => {
            "cookies live in the browser profile; read them with `playwright-cli cookies`"
        }
        "limit-rate" => "a page cannot throttle a fetch",
        "continue-at" => "a page fetch cannot resume; use `-H 'Range: bytes=N-'`",
        "connect-timeout" => "a page fetch cannot time the connect phase alone; use `--max-time`",
        "http1.1" | "http2" | "http3" => "the browser negotiates the HTTP version",
        "no-buffer" => "responses are read whole, never streamed to stdout",
        "trace-ascii" | "trace" => "no wire trace exists; use `playwright-cli network-requests`",
        "user-agent" => "User-Agent is a forbidden header name for fetch(); the tab sets it",
        _ => return None,
    })
}

fn rejected_short(letter: char) -> Option<&'static str> {
    Some(match letter {
        'k' => "insecure",
        'x' => "proxy",
        'b' => "cookie",
        'c' => "cookie-jar",
        'E' => "cert",
        'C' => "continue-at",
        'N' => "no-buffer",
        'A' => "user-agent",
        _ => return None,
    })
}

fn takes_value(name: &str) -> bool {
    matches!(
        name,
        "request"
            | "header"
            | "data"
            | "data-raw"
            | "data-binary"
            | "data-ascii"
            | "data-urlencode"
            | "json"
            | "form"
            | "form-string"
            | "output"
            | "dump-header"
            | "write-out"
            | "max-time"
            | "user"
            | "referer"
            | "range"
            | "url"
            | "tab"
            | "frame"
            | "cdp"
            | "runtime"
    )
}

fn is_bool(name: &str) -> bool {
    matches!(
        name,
        "silent"
            | "show-error"
            | "include"
            | "head"
            | "verbose"
            | "fail"
            | "location"
            | "remote-name"
            | "get"
            | "no-credentials"
            | "help"
            | "version"
    )
}

fn short_value(letter: char) -> Option<&'static str> {
    Some(match letter {
        'X' => "request",
        'H' => "header",
        'd' => "data",
        'F' => "form",
        'o' => "output",
        'D' => "dump-header",
        'w' => "write-out",
        'm' => "max-time",
        'u' => "user",
        'e' => "referer",
        'r' => "range",
        _ => return None,
    })
}

fn short_bool(letter: char) -> Option<&'static str> {
    Some(match letter {
        's' => "silent",
        'S' => "show-error",
        'i' => "include",
        'I' => "head",
        'v' => "verbose",
        'f' => "fail",
        'L' => "location",
        'O' => "remote-name",
        'G' => "get",
        'h' => "help",
        'V' => "version",
        _ => return None,
    })
}

fn rejection(flag: &str, canonical: &str) -> Stop {
    let reason = rejected_reason(canonical).unwrap_or("unsupported");
    Stop::usage(format!(
        "curlwright: option {flag} is not supported in a page context — {reason}"
    ))
}

fn unknown(flag: &str) -> Stop {
    Stop::usage(format!(
        "curlwright: unknown option {flag}\nRun 'curlwright --help' for the supported flags."
    ))
}

fn parse_header(raw: &str) -> HeaderSpec {
    match raw.find(':') {
        None => {
            if raw.ends_with(';') {
                HeaderSpec {
                    name: raw[..raw.len() - 1].trim().to_string(),
                    value: String::new(),
                    unset: false,
                }
            } else {
                HeaderSpec {
                    name: raw.trim().to_string(),
                    value: String::new(),
                    unset: false,
                }
            }
        }
        Some(colon) => {
            let name = raw[..colon].trim().to_string();
            let value = raw[colon + 1..].trim().to_string();
            let unset = value.is_empty();
            HeaderSpec { name, value, unset }
        }
    }
}

fn apply_max_time(opts: &mut Options, value: &str, token: &str) -> Result<(), Stop> {
    let seconds = value.trim().parse::<f64>().unwrap_or(f64::NAN);
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err(Stop::usage(format!(
            "curlwright: {token} expects a positive number of seconds"
        )));
    }
    opts.max_time_seconds = Some(seconds);
    Ok(())
}

fn apply_option(
    opts: &mut Options,
    name: &str,
    value: Option<&str>,
    token: &str,
) -> Result<(), Stop> {
    let owned = |value: Option<&str>| value.unwrap_or("").to_string();
    match name {
        "request" => opts.request_method = Some(owned(value)),
        "header" => opts.headers.push(parse_header(value.unwrap_or(""))),
        "data" | "data-ascii" => opts.data.push(DataSpec {
            kind: DataKind::Ascii,
            value: owned(value),
        }),
        "data-raw" => opts.data.push(DataSpec {
            kind: DataKind::Raw,
            value: owned(value),
        }),
        "data-binary" => opts.data.push(DataSpec {
            kind: DataKind::Binary,
            value: owned(value),
        }),
        "data-urlencode" => opts.data.push(DataSpec {
            kind: DataKind::UrlEncode,
            value: owned(value),
        }),
        "json" => opts.data.push(DataSpec {
            kind: DataKind::Json,
            value: owned(value),
        }),
        "form" => opts.form.push(FormSpec {
            value: owned(value),
            literal: false,
        }),
        "form-string" => opts.form.push(FormSpec {
            value: owned(value),
            literal: true,
        }),
        "output" => opts.output = Some(owned(value)),
        "dump-header" => opts.dump_header = Some(owned(value)),
        "write-out" => opts.write_out = Some(owned(value)),
        "max-time" => return apply_max_time(opts, value.unwrap_or(""), token),
        "user" => opts.user = Some(owned(value)),
        "referer" => opts.referer = Some(owned(value)),
        "range" => opts.range = Some(owned(value)),
        "url" => opts.url = Some(owned(value)),
        "tab" => opts.tab = Some(owned(value)),
        "frame" => opts.frame = Some(owned(value)),
        "cdp" => opts.cdp = Some(owned(value)),
        "runtime" => opts.runtime = Some(owned(value)),
        "silent" => opts.silent = true,
        "show-error" => opts.show_error = true,
        "include" => opts.include = true,
        "head" => opts.head = true,
        "verbose" => opts.verbose = true,
        "fail" => opts.fail_on_error = true,
        "location" => {}
        "remote-name" => opts.remote_name = true,
        "get" => opts.get = true,
        "no-credentials" => opts.credentials = "omit".to_string(),
        "help" => opts.help = true,
        "version" => opts.version = true,
        _ => return Err(unknown(token)),
    }
    Ok(())
}

fn consume_long(opts: &mut Options, args: &[String], index: usize) -> Result<usize, Stop> {
    let token = &args[index];
    let eq = token.find('=');
    let name = match eq {
        Some(eq) => token[2..eq].trim(),
        None => token[2..].trim(),
    };
    let inline = eq.map(|eq| &token[eq + 1..]);
    if let Some(reason_name) = rejected_reason(name).map(|_| name) {
        return Err(rejection(&format!("--{name}"), reason_name));
    }
    if takes_value(name) {
        let value = match inline {
            Some(value) => value,
            None => args
                .get(index + 1)
                .map(String::as_str)
                .ok_or_else(|| Stop::usage(format!("curlwright: option --{name} requires a value")))?,
        };
        apply_option(opts, name, Some(value), &format!("--{name}"))?;
        return Ok(if inline.is_none() { index + 2 } else { index + 1 });
    }
    if !is_bool(name) {
        return Err(unknown(&format!("--{name}")));
    }
    apply_option(opts, name, None, &format!("--{name}"))?;
    Ok(index + 1)
}

fn consume_short(opts: &mut Options, args: &[String], index: usize) -> Result<usize, Stop> {
    let token = &args[index];
    let chars: Vec<char> = token.chars().collect();
    let mut cursor = 1;
    while cursor < chars.len() {
        let letter = chars[cursor];
        if let Some(canonical) = rejected_short(letter) {
            return Err(rejection(&format!("-{letter}"), canonical));
        }
        if let Some(name) = short_value(letter) {
            let attached: String = chars[cursor + 1..].iter().collect();
            let (value, next) = if !attached.is_empty() {
                (attached, index + 1)
            } else {
                let value = args
                    .get(index + 1)
                    .cloned()
                    .ok_or_else(|| Stop::usage(format!("curlwright: option -{letter} requires a value")))?;
                (value, index + 2)
            };
            apply_option(opts, name, Some(&value), &format!("-{letter}"))?;
            return Ok(next);
        }
        let Some(name) = short_bool(letter) else {
            return Err(unknown(&format!("-{letter}")));
        };
        apply_option(opts, name, None, &format!("-{letter}"))?;
        cursor += 1;
    }
    Ok(index + 1)
}

fn consume_positional(opts: &mut Options, token: &str) -> Result<(), Stop> {
    if let Some(existing) = &opts.url {
        if existing != token {
            return Err(Stop::usage(format!(
                "curlwright: only one URL is supported (got \"{existing}\" and \"{token}\")"
            )));
        }
    }
    opts.url = Some(token.to_string());
    Ok(())
}

pub fn parse(args: &[String]) -> Result<Options, Stop> {
    let mut opts = empty_options();
    let mut index = 0;
    let mut options_ended = false;
    while index < args.len() {
        let token = &args[index];
        if !options_ended && token == "--" {
            options_ended = true;
            index += 1;
            continue;
        }
        if !options_ended && token.starts_with("--") {
            index = consume_long(&mut opts, args, index)?;
            continue;
        }
        if !options_ended && token.starts_with('-') && token.len() > 1 {
            index = consume_short(&mut opts, args, index)?;
            continue;
        }
        consume_positional(&mut opts, token)?;
        index += 1;
    }
    Ok(opts)
}

fn forbidden_reason(name: &str) -> Option<&'static str> {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("proxy-") {
        return Some("proxying is configured on the browser");
    }
    if lower.starts_with("sec-") {
        return Some("Sec- headers are reserved for the browser");
    }
    Some(match lower.as_str() {
        "accept-charset" => "the browser sets it",
        "accept-encoding" => "the browser negotiates encoding",
        "connection" => "the browser owns the connection",
        "content-length" => "derived from the body",
        "cookie" => "the tab's cookies are sent already; set one with `playwright-cli cookie-set`",
        "cookie2" => "the tab's cookies are sent already",
        "date" => "the browser sets it",
        "dnt" => "the browser sets it",
        "expect" => "a page fetch cannot send it",
        "host" => "derived from the URL",
        "keep-alive" => "the browser owns the connection",
        "origin" => "the tab's own origin is sent automatically",
        "referer" => "use -e/--referer, which sets the fetch referrer",
        "te" | "trailer" | "transfer-encoding" => "the browser owns transfer encoding",
        "upgrade" => "the browser owns the connection",
        "via" => "the browser sets it",
        _ => return None,
    })
}

pub fn validate_headers(headers: &[HeaderSpec]) -> Result<(), Stop> {
    for header in headers {
        if let Some(reason) = forbidden_reason(&header.name) {
            return Err(Stop::usage(format!(
                "curlwright: header \"{}\" cannot be set from a page — {reason}",
                header.name
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_string()).collect()
    }

    fn ok(args: &[&str]) -> Options {
        parse(&words(args)).unwrap()
    }

    fn err(args: &[&str]) -> Stop {
        parse(&words(args)).unwrap_err()
    }

    #[test]
    fn bundles_short_booleans_and_attached_values() {
        let opts = ok(&["-sSio", "/tmp/x", "https://a.example/"]);
        assert!(opts.silent && opts.show_error && opts.include);
        assert_eq!(opts.output.as_deref(), Some("/tmp/x"));
        assert_eq!(opts.url.as_deref(), Some("https://a.example/"));
        assert_eq!(ok(&["-XPOST", "https://a.example/"]).request_method.as_deref(), Some("POST"));
        assert_eq!(ok(&["--request=PUT", "https://a.example/"]).request_method.as_deref(), Some("PUT"));
    }

    #[test]
    fn keeps_repeatable_flags_and_header_forms() {
        let opts = ok(&[
            "-H", "A: 1", "-H", "B: 2", "-d", "x=1", "-d", "y=2", "https://a.example/",
        ]);
        assert_eq!(opts.headers.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert_eq!(opts.data.iter().map(|d| d.value.as_str()).collect::<Vec<_>>(), ["x=1", "y=2"]);
        let headers = ok(&["-H", "X-A: 1", "-H", "X-B;", "-H", "X-C:", "https://a.example/"]);
        assert_eq!(headers.headers[1].value, "");
        assert!(!headers.headers[1].unset);
        assert!(headers.headers[2].unset);
    }

    #[test]
    fn data_kinds_location_and_rejections() {
        let opts = ok(&[
            "--data-raw", "a", "--data-binary", "b", "--data-ascii", "c", "--data-urlencode", "d",
            "--json", "{}", "https://a.example/",
        ]);
        assert_eq!(
            opts.data.iter().map(|d| d.kind).collect::<Vec<_>>(),
            [DataKind::Raw, DataKind::Binary, DataKind::Ascii, DataKind::UrlEncode, DataKind::Json]
        );
        assert_eq!(ok(&["-L", "https://a.example/"]).url.as_deref(), Some("https://a.example/"));
        assert!(err(&["-H"]).message.contains("-H requires a value"));
        assert!(err(&["-m", "0", "https://a.example/"]).message.contains("positive number"));
        assert_eq!(ok(&["-m", "1.5", "https://a.example/"]).max_time_seconds, Some(1.5));
        assert!(err(&["--interface", "eth0"]).message.contains("outgoing interface"));
        assert!(err(&["-b", "a=1"]).message.contains("cookie-set"));
        assert!(err(&["-A", "curl/8"]).message.contains("forbidden header"));
        assert!(err(&["--unknown-thing"]).message.contains("unknown option --unknown-thing"));
        assert!(validate_headers(&[HeaderSpec { name: "Referer".into(), value: "x".into(), unset: false }]).unwrap_err().message.contains("-e/--referer"));
        assert!(validate_headers(&[HeaderSpec { name: "X-Fine".into(), value: "1".into(), unset: false }]).is_ok());
    }
}
