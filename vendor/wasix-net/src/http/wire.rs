//! HTTP/1.1 on the wire: the request head, the response head, and the
//! response body as a reader that hands its connection back for reuse.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What a connection runs over: a socket, or in tests a buffer.
pub(crate) trait Stream: Read + Write + Send + Sync + 'static {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

impl Stream for crate::TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        crate::TcpStream::set_read_timeout(self, timeout)
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        crate::TcpStream::set_write_timeout(self, timeout)
    }
}

pub(crate) type Conn = BufReader<Box<dyn Stream>>;

/// Idle kept-alive connections, by `proxy|host:port` or `direct|host:port`.
pub(crate) type Pool = Arc<Mutex<Vec<(String, Conn)>>>;
const MAX_IDLE: usize = 8;
const MAX_LINE: u64 = 64 * 1024;
const MAX_HEADERS: usize = 256;

pub(crate) fn valid_header(name: &str, value: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?={}".contains(&b))
        && !value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0)
}

/// The request head. `headers` are the caller's; Host, User-Agent, Accept
/// and Accept-Encoding get defaults the caller may override, and the
/// framing headers are always this client's.
pub(crate) fn request_head(
    method: &str,
    target: &str,
    host: &str,
    headers: &[(String, String)],
    user_agent: &str,
    body_len: Option<usize>,
) -> Vec<u8> {
    let has = |n: &str| headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(n));
    let mut head = format!("{method} {target} HTTP/1.1\r\n");
    if !has("host") {
        head.push_str(&format!("Host: {host}\r\n"));
    }
    if !has("user-agent") {
        head.push_str(&format!("User-Agent: {user_agent}\r\n"));
    }
    if !has("accept") {
        head.push_str("Accept: */*\r\n");
    }
    // The client does not decompress.
    if !has("accept-encoding") {
        head.push_str("Accept-Encoding: identity\r\n");
    }
    for (name, value) in headers {
        let framing = ["content-length", "transfer-encoding", "connection"];
        if framing.iter().any(|f| name.eq_ignore_ascii_case(f)) {
            continue;
        }
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(len) = body_len {
        head.push_str(&format!("Content-Length: {len}\r\n"));
    }
    head.push_str("\r\n");
    head.into_bytes()
}

pub(crate) fn protocol(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn read_line(r: &mut impl BufRead) -> io::Result<String> {
    let mut line = Vec::new();
    let n = r.take(MAX_LINE).read_until(b'\n', &mut line)?;
    if n == 0 {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    if line.last() != Some(&b'\n') {
        return Err(if n as u64 >= MAX_LINE {
            protocol("header line too long")
        } else {
            io::ErrorKind::UnexpectedEof.into()
        });
    }
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line.pop();
    }
    String::from_utf8(line).map_err(|_| protocol("non-UTF-8 header line"))
}

#[derive(Debug)]
pub(crate) struct Head {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub http10: bool,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn has_token(&self, name: &str, token: &str) -> bool {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .flat_map(|(_, v)| v.split(','))
            .any(|t| t.trim().eq_ignore_ascii_case(token))
    }

    pub fn keep_alive(&self) -> bool {
        if self.http10 {
            self.has_token("connection", "keep-alive")
        } else {
            !self.has_token("connection", "close")
        }
    }
}

/// The response head, after any interim (1xx) responses.
pub(crate) fn read_head(r: &mut impl BufRead) -> io::Result<Head> {
    loop {
        let line = read_line(r)?;
        let mut parts = line.splitn(3, ' ');
        let version = parts.next().unwrap_or_default();
        if !version.starts_with("HTTP/1.") {
            return Err(protocol(format!("bad status line `{line}`")));
        }
        let status = parts
            .next()
            .and_then(|c| c.parse::<u16>().ok())
            .filter(|c| (100..1000).contains(c))
            .ok_or_else(|| protocol(format!("bad status line `{line}`")))?;
        let reason = parts.next().unwrap_or_default().to_string();
        let mut headers: Vec<(String, String)> = Vec::new();
        loop {
            let line = read_line(r)?;
            if line.is_empty() {
                break;
            }
            if line.starts_with([' ', '\t']) {
                // obs-fold: a continuation of the previous value.
                let (_, value) = headers
                    .last_mut()
                    .ok_or_else(|| protocol("folded first header line"))?;
                value.push(' ');
                value.push_str(line.trim());
                continue;
            }
            let (name, value) = line
                .split_once(':')
                .ok_or_else(|| protocol(format!("bad header line `{line}`")))?;
            if !valid_header(name, value) {
                return Err(protocol(format!("bad header line `{line}`")));
            }
            if headers.len() == MAX_HEADERS {
                return Err(protocol("too many headers"));
            }
            headers.push((name.to_string(), value.trim().to_string()));
        }
        if (100..200).contains(&status) && status != 101 {
            continue;
        }
        return Ok(Head {
            status,
            reason,
            headers,
            http10: version == "HTTP/1.0",
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Framing {
    Empty,
    Length(u64),
    Chunked,
    Eof,
}

pub(crate) fn framing(head: &Head, method: &str) -> io::Result<Framing> {
    if method.eq_ignore_ascii_case("HEAD") || matches!(head.status, 101 | 204 | 304) {
        return Ok(Framing::Empty);
    }
    if let Some(te) = head.header("transfer-encoding") {
        let last = te.rsplit(',').next().unwrap_or_default().trim();
        return Ok(if last.eq_ignore_ascii_case("chunked") {
            Framing::Chunked
        } else {
            Framing::Eof
        });
    }
    let mut lengths = head
        .headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .flat_map(|(_, v)| v.split(','))
        .map(|v| v.trim().parse::<u64>());
    match lengths.next() {
        None => Ok(Framing::Eof),
        Some(Ok(n)) if lengths.all(|m| m.ok() == Some(n)) => Ok(Framing::Length(n)),
        _ => Err(protocol("bad content-length")),
    }
}

enum State {
    Done,
    Length(u64),
    /// Bytes left in the current chunk; `first` until the first size line.
    Chunk {
        left: u64,
        first: bool,
    },
    Eof,
}

/// A response body, read straight from its connection. Read to the end, a
/// kept-alive connection goes back to the agent's pool.
pub struct Body {
    conn: Option<Conn>,
    state: State,
    reuse: Option<(Pool, String)>,
    read_timeout: Option<Duration>,
    deadline: Option<Instant>,
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Body").finish_non_exhaustive()
    }
}

impl Body {
    pub(crate) fn new(
        conn: Conn,
        framing: Framing,
        reuse: Option<(Pool, String)>,
        read_timeout: Option<Duration>,
        deadline: Option<Instant>,
    ) -> Body {
        let state = match framing {
            Framing::Empty | Framing::Length(0) => State::Done,
            Framing::Length(n) => State::Length(n),
            Framing::Chunked => State::Chunk {
                left: 0,
                first: true,
            },
            Framing::Eof => State::Eof,
        };
        let reuse = if framing == Framing::Eof { None } else { reuse };
        let mut body = Body {
            conn: Some(conn),
            state,
            reuse,
            read_timeout,
            deadline,
        };
        if matches!(body.state, State::Done) {
            body.finish();
        }
        body
    }

    fn finish(&mut self) {
        self.state = State::Done;
        let conn = self.conn.take();
        if let (Some(conn), Some((pool, key))) = (conn, self.reuse.take()) {
            if conn.buffer().is_empty() {
                let mut idle = pool.lock().unwrap_or_else(|e| e.into_inner());
                if idle.len() == MAX_IDLE {
                    idle.remove(0);
                }
                idle.push((key, conn));
            }
        }
    }

    fn arm(&mut self) -> io::Result<()> {
        let timeout = match self.deadline {
            None => self.read_timeout,
            Some(deadline) => {
                let left = deadline
                    .checked_duration_since(Instant::now())
                    .filter(|d| !d.is_zero());
                let left = left.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::TimedOut, "the request timed out")
                })?;
                Some(self.read_timeout.map_or(left, |t| t.min(left)))
            }
        };
        if let Some(conn) = &self.conn {
            conn.get_ref().set_read_timeout(timeout)?;
        }
        Ok(())
    }
}

fn eof() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "the connection closed in the middle of the body",
    )
}

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || matches!(self.state, State::Done) {
            return Ok(0);
        }
        self.arm()?;
        let conn = self
            .conn
            .as_mut()
            .expect("a body that is not done has its connection");
        match self.state {
            State::Done => Ok(0),
            State::Eof => {
                let n = conn.read(buf)?;
                if n == 0 {
                    self.finish();
                }
                Ok(n)
            }
            State::Length(left) => {
                let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
                let n = conn.read(&mut buf[..want])?;
                if n == 0 {
                    return Err(eof());
                }
                self.state = State::Length(left - n as u64);
                if left == n as u64 {
                    self.finish();
                }
                Ok(n)
            }
            State::Chunk { mut left, first } => {
                if left == 0 {
                    if !first && !read_line(conn)?.is_empty() {
                        return Err(protocol("chunk not followed by CRLF"));
                    }
                    let line = read_line(conn)?;
                    let size = line.split(';').next().unwrap_or_default().trim();
                    left = u64::from_str_radix(size, 16)
                        .map_err(|_| protocol(format!("bad chunk size `{line}`")))?;
                    if left == 0 {
                        // Trailers, up to the empty line.
                        while !read_line(conn)?.is_empty() {}
                        self.finish();
                        return Ok(0);
                    }
                }
                let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
                let n = conn.read(&mut buf[..want])?;
                if n == 0 {
                    return Err(eof());
                }
                self.state = State::Chunk {
                    left: left - n as u64,
                    first: false,
                };
                Ok(n)
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Cursor;

    /// Canned response bytes in, request bytes out.
    pub struct Mem {
        pub input: Cursor<Vec<u8>>,
        pub output: Arc<Mutex<Vec<u8>>>,
    }

    impl Read for Mem {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Mem {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Stream for Mem {
        fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
            Ok(())
        }
        fn set_write_timeout(&self, _: Option<Duration>) -> io::Result<()> {
            Ok(())
        }
    }

    fn conn(bytes: &[u8]) -> Conn {
        let mem = Mem {
            input: Cursor::new(bytes.to_vec()),
            output: Default::default(),
        };
        BufReader::new(Box::new(mem) as Box<dyn Stream>)
    }

    fn respond(bytes: &[u8], method: &str) -> (Head, String, Pool) {
        let mut c = conn(bytes);
        let head = read_head(&mut c).unwrap();
        let framing = framing(&head, method).unwrap();
        let pool: Pool = Default::default();
        let reuse = head.keep_alive().then(|| (pool.clone(), "k".to_string()));
        let mut body = Body::new(c, framing, reuse, None, None);
        let mut text = String::new();
        body.read_to_string(&mut text).unwrap();
        (head, text, pool)
    }

    #[test]
    fn content_length_body_returns_the_connection() {
        let (head, body, pool) = respond(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nX-A: 1\r\n\r\nhello",
            "GET",
        );
        assert_eq!(
            (head.status, head.reason.as_str(), head.header("x-a")),
            (200, "OK", Some("1"))
        );
        assert_eq!(body, "hello");
        assert_eq!(pool.lock().unwrap().len(), 1);
    }

    #[test]
    fn chunked_body_with_extensions_and_trailers() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4;ext=1\r\nwiki\r\n5\r\npedia\r\n0\r\nX-T: 1\r\n\r\n";
        let (_, body, pool) = respond(raw, "GET");
        assert_eq!(body, "wikipedia");
        assert_eq!(pool.lock().unwrap().len(), 1);
    }

    #[test]
    fn eof_body_and_connection_close_are_not_reused() {
        let (_, body, pool) = respond(b"HTTP/1.1 200 OK\r\n\r\nto the end", "GET");
        assert_eq!(body, "to the end");
        assert!(pool.lock().unwrap().is_empty());
        let (_, _, pool) = respond(
            b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
            "GET",
        );
        assert!(pool.lock().unwrap().is_empty());
        let (_, _, pool) = respond(b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n", "GET");
        assert!(pool.lock().unwrap().is_empty());
    }

    #[test]
    fn interim_responses_head_and_no_content() {
        let (head, body, _) =
            respond(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 204 No Content\r\nX-B:  folded\r\n  more\r\n\r\n", "POST");
        assert_eq!(
            (head.status, body.as_str(), head.header("X-B")),
            (204, "", Some("folded more"))
        );
        let (_, body, _) = respond(b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\n", "HEAD");
        assert_eq!(body, "");
    }

    #[test]
    fn truncated_and_malformed_responses_fail() {
        let mut c = conn(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort");
        let head = read_head(&mut c).unwrap();
        let mut body = Body::new(c, framing(&head, "GET").unwrap(), None, None, None);
        let err = body.read_to_end(&mut Vec::new()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        assert!(read_head(&mut conn(b"SSH-2.0\r\n\r\n")).is_err());
        assert!(read_head(&mut conn(b"HTTP/1.1 200 OK\r\nno colon\r\n\r\n")).is_err());
        assert_eq!(
            read_head(&mut conn(b"")).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        let head = read_head(&mut conn(
            b"HTTP/1.1 200 OK\r\nContent-Length: 1, 2\r\n\r\n",
        ))
        .unwrap();
        assert!(framing(&head, "GET").is_err());
        let mut c = conn(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n");
        let head = read_head(&mut c).unwrap();
        let mut body = Body::new(c, framing(&head, "GET").unwrap(), None, None, None);
        assert!(body.read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn request_head_defaults_and_overrides() {
        let headers = vec![
            ("Accept".to_string(), "application/json".to_string()),
            ("Content-Length".to_string(), "999".to_string()),
            ("X-Key".to_string(), "v".to_string()),
        ];
        let head = request_head("POST", "http://h/p", "h", &headers, "ua/1", Some(3));
        assert_eq!(
            String::from_utf8(head).unwrap(),
            "POST http://h/p HTTP/1.1\r\nHost: h\r\nUser-Agent: ua/1\r\n\
             Accept-Encoding: identity\r\nAccept: application/json\r\nX-Key: v\r\n\
             Content-Length: 3\r\n\r\n"
        );
        assert!(valid_header("X-Ok", "a b"));
        assert!(!valid_header("X Bad", "v"));
        assert!(!valid_header("X-Bad", "a\r\nInjected: 1"));
    }
}
