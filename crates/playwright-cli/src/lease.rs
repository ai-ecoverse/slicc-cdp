mod transport;
#[cfg(test)]
mod corpus_test;

use crate::args::Output;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use url::Url;

pub(super) const HELP: &str = "playwright-cli lease request --allow <https-origin> --for <duration> [--reason <text>]\nplaywright-cli lease request --from <lease> --add <exact-https-url> [--reason <text>]\nplaywright-cli --lease <handle> goto <https-url> | snapshot | click <ref> | fill <ref> <value>\nRequires a job-scoped lease adapter. No CDP discovery or automatic retry.\n";

pub(super) fn dispatch(argv: &[String]) -> Option<Output> {
    let marked = argv
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--lease" || arg.starts_with("--lease="));
    if !marked && argv.first().is_none_or(|arg| arg != "lease") {
        if !transport::configured()
            || argv.is_empty()
            || matches!(argv, [arg] if matches!(arg.as_str(), "--help" | "-h" | "--version" | "-V"))
        {
            return None;
        }
        return Some(unsupported());
    }
    if matches!(argv, [lease, help] if lease == "lease" && (help == "--help" || help == "-h")) {
        return Some(Output::ok(HELP));
    }
    Some(match Call::parse(argv) {
        Err(err) => {
            let lease = match argv {
                [flag, id, ..] if flag == "--lease" => Some(id.as_str()),
                [flag, ..] => flag.strip_prefix("--lease="),
                _ => None,
            };
            err.output(lease.filter(|id| identifier(id)), false)
        }
        Ok(call) => call
            .run()
            .unwrap_or_else(|err| err.output(call.params["lease"].as_str(), call.mutating())),
    })
}

pub(super) fn unsupported() -> Output {
    Error::Unsupported.output(None, false)
}

#[derive(Clone, Copy, Debug)]
enum Error {
    Arguments,
    Unsupported,
    InitialScope,
    Unavailable,
    UnknownOutcome,
    InvalidReply,
    Pending,
    Denied,
    Scope,
    Protected,
    Expired,
    Revoked,
    Retired,
    Stale,
}

impl Error {
    fn output(self, lease: Option<&str>, mutating: bool) -> Output {
        let (reason, message) = match self {
            Self::Arguments => ("E_ARGUMENT", "Invalid lease arguments. Run playwright-cli lease --help for usage."),
            Self::Unsupported => ("E_UNSUPPORTED", "This command or option is unavailable on a lease. No browser fallback."),
            Self::InitialScope => ("E_UNSUPPORTED", "Initial requests support one bare HTTPS origin without a slash. Exact URLs and path prefixes cannot be widened to an origin."),
            Self::Unavailable => ("E_UNAVAILABLE", "No job-scoped lease adapter is available."),
            Self::UnknownOutcome => ("E_OUTCOME_UNKNOWN", "No valid acknowledgement. The action may have taken effect; do not automatically retry."),
            Self::InvalidReply => ("E_PROTOCOL", "Invalid response from the job lease adapter."),
            Self::Pending => ("E_PENDING", "Waiting for owner approval. No access yet."),
            Self::Denied => ("E_DENIED", "The owner denied this request."),
            Self::Scope => ("E_SCOPE", "Outside the approved scope. Request a known exact URL with lease request --from and --add, or stop. Do not replay the action."),
            Self::Protected => ("E_SCOPE", "This browser surface cannot be requested or driven by a lease."),
            Self::Expired => ("E_EXPIRED", "This lease expired. Request a new lease for owner review."),
            Self::Revoked => ("E_REVOKED", "This lease was revoked. Stop using this handle."),
            Self::Retired => ("E_REPLACED", "This lease was replaced. Stop using this handle."),
            Self::Stale => ("E_STALE", "This reference is no longer valid. Take a new scoped snapshot."),
        };
        let mut error = json!({"reason":reason,"message":message});
        if let Some(lease) = lease {
            error["lease"] = json!(lease);
        }
        if mutating
            && matches!(
                self,
                Self::Scope | Self::Expired | Self::Revoked | Self::Retired | Self::UnknownOutcome
            )
        {
            error["outcome"] = json!("unknown");
        }
        Output::err(error.to_string())
    }

    fn remote(reply: &Value) -> Option<Self> {
        Some(match reply.pointer("/data/reason")?.as_str()? {
            "E_PENDING" => Self::Pending,
            "E_DENIED" => Self::Denied,
            "E_SCOPE" => Self::Scope,
            "E_EXPIRED" => Self::Expired,
            "E_REVOKED" => Self::Revoked,
            "E_REPLACED" => Self::Retired,
            "E_STALE" => Self::Stale,
            "E_UNSUPPORTED" => Self::Unsupported,
            "E_UNAVAILABLE" => Self::Unavailable,
            "E_OUTCOME_UNKNOWN" => Self::UnknownOutcome,
            _ => return None,
        })
    }
}

fn identifier(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

fn https_url(raw: &str) -> Result<Url, Error> {
    if raw.len() > 4096
        || !raw.is_ascii()
        || raw.match_indices('%').any(|(i, _)| {
            raw.as_bytes()
                .get(i + 1..i + 3)
                .is_none_or(|digits| !digits.iter().all(u8::is_ascii_hexdigit))
        })
        || raw
            .bytes()
            .any(|b| b.is_ascii_control() || b.is_ascii_whitespace() || b == b'\\')
    {
        return Err(Error::Arguments);
    }
    let url = Url::parse(raw).map_err(|_| Error::Arguments)?;
    let host = url.host_str().ok_or(Error::Arguments)?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'[' | b']' | b':'))
        || (raw != url.as_str() && raw != url.origin().ascii_serialization())
    {
        return Err(Error::Arguments);
    }
    let hard = cdp_client::protected::Protected::default();
    if hard.url(raw) || hard.credential_url(raw) {
        return Err(Error::Protected);
    }
    Ok(url)
}

fn duration(raw: &str) -> Option<u32> {
    let (number, multiplier) = if let Some(n) = raw.strip_suffix('h') {
        (n, 3600)
    } else if let Some(n) = raw.strip_suffix('m') {
        (n, 60)
    } else {
        (raw.strip_suffix('s')?, 1)
    };
    if !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    number
        .parse::<u32>()
        .ok()?
        .checked_mul(multiplier)
        .filter(|n| *n > 0)
}

struct Call {
    method: &'static str,
    params: Value,
}

impl Call {
    fn parse(args: &[String]) -> Result<Self, Error> {
        let (first, rest) = args.split_first().ok_or(Error::Arguments)?;
        if first == "lease" {
            let (sub, args) = rest.split_first().ok_or(Error::Arguments)?;
            if sub != "request" {
                return Err(Error::Unsupported);
            }
            return Self::request(args);
        }
        let (lease, args) = if first == "--lease" {
            let (id, tail) = rest.split_first().ok_or(Error::Arguments)?;
            if id.starts_with("--") {
                return Err(Error::Arguments);
            }
            (id.as_str(), tail)
        } else {
            (
                first.strip_prefix("--lease=").ok_or(Error::Unsupported)?,
                rest,
            )
        };
        if !identifier(lease) {
            return Err(Error::Arguments);
        }
        let (op, tail) = args.split_first().ok_or(Error::Arguments)?;
        let (opts, literal) = tail.split_at(
            tail.iter()
                .position(|arg| arg == "--")
                .unwrap_or(tail.len()),
        );
        if opts.iter().any(|arg| arg.starts_with('-')) {
            return Err(Error::Unsupported);
        }
        let positional: Vec<_> = opts
            .iter()
            .chain(literal.iter().skip(1))
            .map(String::as_str)
            .collect();
        let args = match (op.as_str(), positional.as_slice()) {
            ("snapshot", []) => json!({}),
            ("goto", [url]) => {
                https_url(url)?;
                json!({"url":url})
            }
            ("click", [r]) if identifier(r) => json!({"ref":r}),
            ("fill", [r, value]) if identifier(r) && value.len() <= 16384 => {
                json!({"ref":r,"value":value})
            }
            ("snapshot" | "goto" | "click" | "fill", _) => return Err(Error::Arguments),
            _ => return Err(Error::Unsupported),
        };
        Ok(Self {
            method: "Slicc.leaseAction",
            params: json!({"lease":lease,"op":op,"args":args}),
        })
    }

    fn request(args: &[String]) -> Result<Self, Error> {
        let mut fields = BTreeMap::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let (name, value) = match arg.split_once('=') {
                Some(pair) => pair,
                None => (
                    arg.as_str(),
                    iter.next()
                        .filter(|s| !s.starts_with("--"))
                        .ok_or(Error::Arguments)?
                        .as_str(),
                ),
            };
            if !matches!(name, "--allow" | "--for" | "--from" | "--add" | "--reason") {
                return Err(Error::Unsupported);
            }
            if fields.insert(name, value).is_some() {
                return Err(Error::Arguments);
            }
        }
        let mut params = if let Some(id) = fields.remove("--from") {
            let raw = fields.remove("--add").ok_or(Error::Arguments)?;
            if !identifier(id) {
                return Err(Error::Arguments);
            }
            let url = https_url(raw)?;
            let path = url.path().to_ascii_lowercase();
            if raw.contains('*') || path.contains("%2f") || path.contains("%5c") {
                return Err(Error::Arguments);
            }
            json!({"lease":id,"allow":[{"kind":"exact","url":url.as_str()}]})
        } else {
            let raw = fields.remove("--allow").ok_or(Error::Arguments)?;
            let origin = https_url(raw)?.origin().ascii_serialization();
            if raw != origin {
                return Err(Error::InitialScope);
            }
            let ttl = fields
                .remove("--for")
                .and_then(duration)
                .ok_or(Error::Arguments)?;
            json!({"allow":[{"kind":"origin","url":origin}],"ttlSeconds":ttl})
        };
        if let Some(reason) = fields.remove("--reason") {
            if reason.is_empty() || reason.len() > 512 || reason.chars().any(char::is_control) {
                return Err(Error::Arguments);
            }
            params["reason"] = json!(reason);
        }
        if !fields.is_empty() {
            return Err(Error::Arguments);
        }
        Ok(Self {
            method: "Slicc.leaseRequest",
            params,
        })
    }

    fn mutating(&self) -> bool {
        matches!(self.params["op"].as_str(), Some("click" | "fill" | "goto"))
    }

    fn run(&self) -> Result<Output, Error> {
        let uncertain = if self.mutating() {
            Error::UnknownOutcome
        } else {
            Error::InvalidReply
        };
        let wire = json!({"id":1,"method":self.method,"params":self.params}).to_string();
        let reply = transport::exchange(&wire, uncertain)?;
        let reply: Value = serde_json::from_slice(&reply).map_err(|_| uncertain)?;
        if reply["id"].as_u64() != Some(1)
            || reply.get("sessionId").is_some()
            || reply.get("method").is_some()
        {
            return Err(uncertain);
        }
        if let Some(err) = reply.get("error") {
            return Err(if reply.get("result").is_some() {
                uncertain
            } else {
                Error::remote(err).unwrap_or(uncertain)
            });
        }
        let result = &reply["result"];
        if let Some(op) = self.params["op"].as_str() {
            let ack = result["result"].as_object().ok_or(uncertain)?;
            if op == "snapshot" {
                let text = ack.get("text").and_then(Value::as_str).ok_or(uncertain)?;
                return Ok(Output::ok(format!(
                    "{}\n",
                    crate::sanitize::terminal(text).trim_end_matches('\n')
                )));
            }
            if !ack.is_empty() {
                return Err(uncertain);
            }
            return Ok(Output::ok("OK\n"));
        }
        let id = |key: &str| {
            result[key]
                .as_str()
                .filter(|id| identifier(id))
                .ok_or(Error::InvalidReply)
        };
        let request = id("request")?;
        let lease = id("lease")?;
        let mut text = format!("Request: {request}\nLease: {lease}\nPending owner review.\n");
        if self.params.get("lease").is_none() {
            let jkt = result["jkt"]
                .as_str()
                .filter(|jkt| {
                    jkt.len() == 43
                        && identifier(jkt)
                        && b"AEIMQUYcgkosw048".contains(&jkt.as_bytes()[42])
                })
                .ok_or(Error::InvalidReply)?;
            let origin = self.params["allow"][0]["url"]
                .as_str()
                .ok_or(Error::Arguments)?;
            let seconds = self.params["ttlSeconds"].as_u64().ok_or(Error::Arguments)?;
            let ttl = if seconds % 3600 == 0 {
                format!("{}h", seconds / 3600)
            } else if seconds % 60 == 0 {
                format!("{}m", seconds / 60)
            } else {
                format!("{seconds}s")
            };
            let scope = if origin.contains('[') {
                format!("'{origin}'")
            } else {
                origin.to_string()
            };
            text.push_str(&format!("Copy this line into the owner's terminal to review, not grant:\nslicc lease review --to jkt:{jkt} --allow {scope} --for {ttl}\n"));
        }
        Ok(Output::ok(text))
    }
}
