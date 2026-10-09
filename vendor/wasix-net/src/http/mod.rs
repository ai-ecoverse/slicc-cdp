//! A small blocking HTTP/1.1 client.
//!
//! ```no_run
//! let agent = wasix_net::http::Agent::new();
//! let response = agent.get("https://example.com/").header("Accept", "text/html").call()?;
//! println!("{} {}", response.status(), response.into_string()?);
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! - **Proxy**: by default the proxy comes from the environment:
//!   `https_proxy` / `http_proxy` (either case, then `all_proxy`) unless
//!   the host is in `no_proxy`. Requests to it go in absolute form
//!   (`GET https://example.com/ HTTP/1.1`), so the proxy does any TLS.
//! - **No TLS**: an `https://` URL without a proxy is an error
//!   ([`Error::TlsUnsupported`]), as are `https://` and SOCKS proxies.
//! - **Bodies**: requests send a buffered body with a Content-Length.
//!   Responses come chunked, with a length, or up to the end of the
//!   connection, and are read as they arrive ([`Response::into_reader`]).
//! - **Redirects**: followed (10 by default). 301, 302 and 303 turn
//!   anything but GET and HEAD into a body-less GET; 307 and 308 repeat
//!   the request with its body. Authorization and Cookie headers are
//!   dropped when a redirect leaves the host.
//! - **Connections** are kept alive and reused, and a request that finds
//!   its reused connection closed is sent once more on a fresh one.
//! - Every status is a response, not an error.

mod proxy;
mod url;
mod wire;

use std::fmt;
use std::io::{self, BufReader, Read, Write};
use std::time::{Duration, Instant};

pub use wire::Body;

use proxy::ProxyAddr;
use url::Url;
use wire::{Conn, Pool, Stream};

/// Why a request failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The URL could not be parsed.
    BadUrl(String),
    /// A scheme other than `http` or `https`.
    UnsupportedScheme(String),
    /// An `https://` URL with no proxy to send it through: this client has
    /// no TLS of its own.
    TlsUnsupported(String),
    /// A method this client does not send (CONNECT).
    UnsupportedMethod(String),
    /// The proxy setting could not be used.
    BadProxy(String),
    /// A header name or value that cannot be sent.
    BadHeader(String),
    /// The connection to the server (or proxy) failed.
    Connect { addr: String, source: io::Error },
    /// Sending the request or reading the response head failed; a timeout
    /// is an `io::ErrorKind::TimedOut`.
    Io(io::Error),
    /// The response was not HTTP/1.x as this client understands it.
    Protocol(String),
    /// More redirects than the agent follows.
    TooManyRedirects(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadUrl(why) => write!(f, "invalid URL {why}"),
            Error::UnsupportedScheme(url) => {
                write!(f, "unsupported URL scheme in `{url}`: only http and https")
            }
            Error::TlsUnsupported(url) => write!(
                f,
                "cannot fetch `{url}` without a proxy: this client has no TLS, so https goes \
                 through the proxy that https_proxy names"
            ),
            Error::UnsupportedMethod(m) => write!(f, "the {m} method is not supported"),
            Error::BadProxy(why) => write!(f, "unusable proxy {why}"),
            Error::BadHeader(name) => write!(f, "invalid header `{name}`"),
            Error::Connect { addr, source } => write!(f, "cannot connect to {addr}: {source}"),
            Error::Io(e) => write!(f, "{e}"),
            Error::Protocol(why) => write!(f, "malformed response: {why}"),
            Error::TooManyRedirects(url) => write!(f, "too many redirects fetching `{url}`"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Connect { source, .. } | Error::Io(source) => Some(source),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        if e.kind() == io::ErrorKind::InvalidData {
            Error::Protocol(e.to_string())
        } else {
            Error::Io(e)
        }
    }
}

#[derive(Debug, Clone)]
enum ProxyMode {
    Env,
    Off,
    Fixed(String),
}

#[derive(Debug, Clone)]
struct Config {
    proxy: ProxyMode,
    timeout: Option<Duration>,
    timeout_connect: Option<Duration>,
    timeout_read: Option<Duration>,
    timeout_write: Option<Duration>,
    redirects: u32,
    user_agent: String,
}

/// Builds an [`Agent`].
#[derive(Debug, Clone)]
pub struct AgentBuilder {
    config: Config,
}

impl Default for AgentBuilder {
    fn default() -> AgentBuilder {
        AgentBuilder::new()
    }
}

impl AgentBuilder {
    pub fn new() -> AgentBuilder {
        AgentBuilder {
            config: Config {
                proxy: ProxyMode::Env,
                timeout: None,
                timeout_connect: None,
                timeout_read: None,
                timeout_write: None,
                redirects: 10,
                user_agent: concat!("wasix-net/", env!("CARGO_PKG_VERSION")).to_string(),
            },
        }
    }

    /// Whether to take the proxy from the environment (the default) or to
    /// connect directly.
    pub fn proxy_from_env(mut self, on: bool) -> AgentBuilder {
        self.config.proxy = if on { ProxyMode::Env } else { ProxyMode::Off };
        self
    }

    /// Send every request through this `http://host:port` proxy,
    /// whatever the environment says.
    pub fn proxy(mut self, url: &str) -> AgentBuilder {
        self.config.proxy = ProxyMode::Fixed(url.to_string());
        self
    }

    /// The whole request, redirects and body included.
    pub fn timeout(mut self, timeout: Duration) -> AgentBuilder {
        self.config.timeout = Some(timeout);
        self
    }

    pub fn timeout_connect(mut self, timeout: Duration) -> AgentBuilder {
        self.config.timeout_connect = Some(timeout);
        self
    }

    /// Each read from the connection.
    pub fn timeout_read(mut self, timeout: Duration) -> AgentBuilder {
        self.config.timeout_read = Some(timeout);
        self
    }

    /// Each write to the connection.
    pub fn timeout_write(mut self, timeout: Duration) -> AgentBuilder {
        self.config.timeout_write = Some(timeout);
        self
    }

    /// How many redirects to follow; 0 returns redirects as responses.
    pub fn redirects(mut self, n: u32) -> AgentBuilder {
        self.config.redirects = n;
        self
    }

    pub fn user_agent(mut self, user_agent: &str) -> AgentBuilder {
        self.config.user_agent = user_agent.to_string();
        self
    }

    pub fn build(self) -> Agent {
        Agent {
            config: self.config,
            pool: Pool::default(),
        }
    }
}

/// Sends requests, keeping connections alive between them. Clones share
/// the connections.
#[derive(Clone)]
pub struct Agent {
    config: Config,
    pool: Pool,
}

impl fmt::Debug for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agent")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Default for Agent {
    fn default() -> Agent {
        Agent::new()
    }
}

impl Agent {
    pub fn new() -> Agent {
        AgentBuilder::new().build()
    }

    pub fn builder() -> AgentBuilder {
        AgentBuilder::new()
    }

    pub fn request(&self, method: &str, url: &str) -> Request {
        Request {
            agent: self.clone(),
            method: method.to_ascii_uppercase(),
            url: url.to_string(),
            headers: Vec::new(),
            timeout: None,
        }
    }

    pub fn get(&self, url: &str) -> Request {
        self.request("GET", url)
    }

    pub fn head(&self, url: &str) -> Request {
        self.request("HEAD", url)
    }

    pub fn post(&self, url: &str) -> Request {
        self.request("POST", url)
    }

    pub fn put(&self, url: &str) -> Request {
        self.request("PUT", url)
    }

    pub fn patch(&self, url: &str) -> Request {
        self.request("PATCH", url)
    }

    pub fn delete(&self, url: &str) -> Request {
        self.request("DELETE", url)
    }

    fn proxy_for(&self, url: &Url) -> Result<Option<ProxyAddr>, Error> {
        match &self.config.proxy {
            ProxyMode::Off => Ok(None),
            ProxyMode::Fixed(p) => ProxyAddr::parse(p).map(Some),
            ProxyMode::Env => proxy::from_env(url),
        }
    }

    fn connect(&self, host: &str, port: u16, deadline: Option<Instant>) -> Result<Conn, Error> {
        let addr = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        let failed = |source: io::Error| Error::Connect {
            addr: addr.clone(),
            source,
        };
        let timeout = match (self.config.timeout_connect, remaining(deadline)?) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let mut last = None;
        for a in crate::resolve(host, port).map_err(failed)? {
            let stream = match timeout {
                Some(t) => crate::TcpStream::connect_timeout(&a, t),
                None => crate::TcpStream::connect(a),
            };
            match stream {
                Ok(s) => return Ok(BufReader::new(Box::new(s) as Box<dyn Stream>)),
                Err(e) => last = Some(e),
            }
        }
        Err(failed(last.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "the name resolved to no address")
        })))
    }

    fn take_idle(&self, key: &str) -> Option<Conn> {
        let mut idle = self.pool.lock().unwrap_or_else(|e| e.into_inner());
        let i = idle.iter().rposition(|(k, _)| k == key)?;
        Some(idle.remove(i).1)
    }

    /// One request and its response head, without following redirects.
    fn exchange(
        &self,
        method: &str,
        url: &Url,
        headers: &[(String, String)],
        body: Option<&[u8]>,
        deadline: Option<Instant>,
    ) -> Result<Response, Error> {
        let (target, host, port, key) = match self.proxy_for(url)? {
            Some(p) => {
                let key = format!("proxy|{}:{}", p.host, p.port);
                (url.absolute(), p.host, p.port, key)
            }
            None if url.scheme == "https" => return Err(Error::TlsUnsupported(url.absolute())),
            None => {
                let key = format!("direct|{}:{}", url.host, url.port_or_default());
                (
                    url.path.clone(),
                    url.host.clone(),
                    url.port_or_default(),
                    key,
                )
            }
        };
        let body_len = match body {
            Some(b) => Some(b.len()),
            None if matches!(method, "POST" | "PUT" | "PATCH") => Some(0),
            None => None,
        };
        let head = wire::request_head(
            method,
            &target,
            &url.authority(),
            headers,
            &self.config.user_agent,
            body_len,
        );
        let mut idle = self.take_idle(&key);
        loop {
            let reused = idle.is_some();
            let mut conn = match idle.take() {
                Some(conn) => conn,
                None => self.connect(&host, port, deadline)?,
            };
            let sent = (|| -> io::Result<wire::Head> {
                let stream = conn.get_mut();
                stream.set_write_timeout(self.config.timeout_write)?;
                stream.write_all(&head)?;
                stream.write_all(body.unwrap_or_default())?;
                stream.flush()?;
                let read_timeout = match (self.config.timeout_read, remaining(deadline)?) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                conn.get_ref().set_read_timeout(read_timeout)?;
                wire::read_head(&mut conn)
            })();
            let head = match sent {
                Ok(head) => head,
                // A kept-alive connection the server closed: once more, fresh.
                Err(e) if reused && stale(&e) => continue,
                Err(e) => return Err(e.into()),
            };
            let framing = wire::framing(&head, method)?;
            let reuse = head.keep_alive().then(|| (self.pool.clone(), key.clone()));
            let body = Body::new(conn, framing, reuse, self.config.timeout_read, deadline);
            return Ok(Response {
                status: head.status,
                reason: head.reason,
                headers: head.headers,
                url: url.absolute(),
                body,
            });
        }
    }

    fn run(
        &self,
        method: &str,
        url: &str,
        mut headers: Vec<(String, String)>,
        body: Option<&[u8]>,
        timeout: Option<Duration>,
    ) -> Result<Response, Error> {
        if method.eq_ignore_ascii_case("CONNECT") {
            return Err(Error::UnsupportedMethod("CONNECT".into()));
        }
        if !method.bytes().all(|b| b.is_ascii_alphabetic()) || method.is_empty() {
            return Err(Error::UnsupportedMethod(method.to_string()));
        }
        if let Some((name, _)) = headers.iter().find(|(n, v)| !wire::valid_header(n, v)) {
            return Err(Error::BadHeader(name.clone()));
        }
        let deadline = timeout.or(self.config.timeout).map(|t| Instant::now() + t);
        let mut url = Url::parse(url)?;
        let mut method = method.to_string();
        let mut body = body;
        let mut followed = 0;
        loop {
            if url.scheme != "http" && url.scheme != "https" {
                return Err(Error::UnsupportedScheme(url.absolute()));
            }
            let response = self.exchange(&method, &url, &headers, body, deadline)?;
            let location = match response.status {
                301 | 302 | 303 | 307 | 308 if self.config.redirects > 0 => {
                    response.header("location").map(str::to_string)
                }
                _ => None,
            };
            let Some(location) = location else {
                return Ok(response);
            };
            if followed == self.config.redirects {
                return Err(Error::TooManyRedirects(url.absolute()));
            }
            followed += 1;
            if matches!(response.status, 301..=303) && method != "GET" && method != "HEAD" {
                method = "GET".into();
                body = None;
                headers.retain(|(n, _)| !n.eq_ignore_ascii_case("content-type"));
            }
            // Read a small rest of the body so the connection can be reused.
            let mut rest = response.into_reader().take(64 * 1024);
            let _ = io::copy(&mut rest, &mut io::sink());
            let next = url.join(&location)?;
            if next.host != url.host {
                headers.retain(|(n, _)| {
                    !n.eq_ignore_ascii_case("authorization") && !n.eq_ignore_ascii_case("cookie")
                });
            }
            url = next;
        }
    }
}

fn remaining(deadline: Option<Instant>) -> io::Result<Option<Duration>> {
    match deadline {
        None => Ok(None),
        Some(d) => match d.checked_duration_since(Instant::now()) {
            Some(left) if !left.is_zero() => Ok(Some(left)),
            _ => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the request timed out",
            )),
        },
    }
}

fn stale(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
}

/// A request being built; [`call`](Request::call) or one of the `send`
/// methods sends it.
#[derive(Debug, Clone)]
pub struct Request {
    agent: Agent,
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    timeout: Option<Duration>,
}

impl Request {
    /// Adds a header; one the request already has is replaced.
    pub fn header(mut self, name: &str, value: &str) -> Request {
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// The whole request, overriding the agent's timeout.
    pub fn timeout(mut self, timeout: Duration) -> Request {
        self.timeout = Some(timeout);
        self
    }

    pub fn method(&self) -> &str {
        &self.method
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// The value of a header set on this request.
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Sends the request without a body.
    pub fn call(self) -> Result<Response, Error> {
        self.agent
            .run(&self.method, &self.url, self.headers, None, self.timeout)
    }

    /// Sends the request with `body`.
    pub fn send(self, body: &[u8]) -> Result<Response, Error> {
        self.agent.run(
            &self.method,
            &self.url,
            self.headers,
            Some(body),
            self.timeout,
        )
    }

    /// Sends `value` as JSON, with `Content-Type: application/json` unless
    /// the request has a content type.
    #[cfg(feature = "json")]
    pub fn send_json<T: serde::Serialize + ?Sized>(self, value: &T) -> Result<Response, Error> {
        let body = serde_json::to_vec(value)
            .map_err(|e| Error::Io(io::Error::new(io::ErrorKind::InvalidInput, e)))?;
        let request = if self.get_header("content-type").is_some() {
            self
        } else {
            self.header("Content-Type", "application/json")
        };
        request.send(&body)
    }
}

/// A response, its head read and its body still on the connection.
#[derive(Debug)]
pub struct Response {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    url: String,
    body: Body,
}

impl Response {
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The reason phrase of the status line.
    pub fn status_text(&self) -> &str {
        &self.reason
    }

    /// The first value of a header, by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Every value of a header.
    pub fn all(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// Header names and values, in the order they came.
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The URL this response came from, after redirects.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The body as it arrives.
    pub fn into_reader(self) -> Body {
        self.body
    }

    pub fn into_bytes(self) -> io::Result<Vec<u8>> {
        let mut body = self.body;
        let mut out = Vec::new();
        body.read_to_end(&mut out)?;
        Ok(out)
    }

    pub fn into_string(self) -> io::Result<String> {
        String::from_utf8(self.into_bytes()?)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    #[cfg(feature = "json")]
    pub fn into_json<T: serde::de::DeserializeOwned>(self) -> io::Result<T> {
        serde_json::from_reader(io::BufReader::new(self.body))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_without_a_proxy_is_refused_before_connecting() {
        let agent = Agent::builder().proxy_from_env(false).build();
        let err = agent.get("https://example.invalid/").call().unwrap_err();
        assert!(matches!(err, Error::TlsUnsupported(_)), "{err}");
        assert!(err.to_string().contains("https_proxy"));
        let err = agent
            .request("CONNECT", "http://example.invalid/")
            .call()
            .unwrap_err();
        assert!(matches!(err, Error::UnsupportedMethod(_)));
        let err = agent.get("ftp://example.invalid/").call().unwrap_err();
        assert!(matches!(err, Error::UnsupportedScheme(_)));
        let err = agent
            .get("http://h/")
            .header("X", "a\nb")
            .call()
            .unwrap_err();
        assert!(matches!(err, Error::BadHeader(_)));
        let err = Agent::builder()
            .proxy("socks5://127.0.0.1:1")
            .build()
            .get("http://h/")
            .call();
        assert!(matches!(err.unwrap_err(), Error::BadProxy(_)));
    }
}
