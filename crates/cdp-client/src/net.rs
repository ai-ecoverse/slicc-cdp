use crate::connect::format_http_error;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use wasix_net::TcpStream;

const READ_SLICE: Duration = Duration::from_millis(200);

pub struct HttpResponse {
    pub body: Vec<u8>,
}

pub struct ProbeResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ProbeResponse {
    pub fn content_type(&self) -> &str {
        self.headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("")
    }
}

pub struct Ws {
    stream: Box<dyn ReadWrite>,
    buf: Vec<u8>,
}

trait ReadWrite: Read + Write {
    fn set_nonblocking(&mut self, on: bool) -> std::io::Result<()>;
    fn read_timeout(&self) -> std::io::Result<Option<Duration>>;
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()>;
}

impl ReadWrite for TcpStream {
    fn set_nonblocking(&mut self, on: bool) -> std::io::Result<()> {
        TcpStream::set_nonblocking(self, on)
    }

    fn read_timeout(&self) -> std::io::Result<Option<Duration>> {
        TcpStream::read_timeout(self)
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        TcpStream::set_read_timeout(self, timeout)
    }
}

#[cfg(not(target_os = "wasi"))]
impl ReadWrite for native_tls::TlsStream<TcpStream> {
    fn set_nonblocking(&mut self, on: bool) -> std::io::Result<()> {
        self.get_mut().set_nonblocking(on)
    }

    fn read_timeout(&self) -> std::io::Result<Option<Duration>> {
        self.get_ref().read_timeout()
    }

    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.get_ref().set_read_timeout(timeout)
    }
}

struct Parts {
    scheme: String,
    host: String,
    port: u16,
    path: String,
}

pub struct EndpointUrl {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

pub fn parse_endpoint(url: &str) -> Result<EndpointUrl, String> {
    let parts = parse_url(url)?;
    Ok(EndpointUrl {
        scheme: parts.scheme,
        host: parts.host,
        port: parts.port,
    })
}

pub fn http_get(url: &str) -> Result<HttpResponse, String> {
    let parts = parse_url(url)?;
    if parts.scheme != "http" && parts.scheme != "https" {
        return Err(format!("discovery URL must be http or https: {url}\n"));
    }
    let mut stream = dial(&parts)?;
    let host_header = host_header(&parts);
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\n\r\n",
        parts.path, host_header
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("CDP discovery failed for {url}: {err}\n"))?;
    let (status, _headers, body, _rest) = read_http(stream.as_mut())?;
    if !(200..300).contains(&status) {
        return Err(format_http_error(status, &body));
    }
    let _ = status;
    Ok(HttpResponse { body })
}

const PROBE_MAX_BODY: usize = 1024 * 1024;

#[cfg(not(target_os = "wasi"))]
pub fn http_probe(url: &str, accept: &str) -> Result<ProbeResponse, String> {
    probe_request("GET", url, &format!("Accept: {accept}\r\n"), "")
}

#[cfg(not(target_os = "wasi"))]
pub fn http_post_json(url: &str, body: &str) -> Result<ProbeResponse, String> {
    let headers = format!(
        "Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n",
        body.len()
    );
    probe_request("POST", url, &headers, body)
}

#[cfg(not(target_os = "wasi"))]
fn probe_request(method: &str, url: &str, headers: &str, body: &str) -> Result<ProbeResponse, String> {
    let parts = parse_url(url)?;
    if parts.scheme != "http" && parts.scheme != "https" {
        return Err(format!("probe URL must be http or https: {url}\n"));
    }
    refuse_real_debug_port(&parts);
    let addr = resolve(&parts.host, parts.port)?;
    refuse_real_debug_addr(addr);
    let deadline = Instant::now() + Duration::from_secs(5);
    let connect_timeout = Duration::from_secs(3).min(
        deadline.saturating_duration_since(Instant::now()),
    );
    let tcp = TcpStream::connect_timeout(&addr, connect_timeout)
        .map_err(|err| format!("probe connect failed for {url}: {err}\n"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|err| format!("probe timeout setup failed: {err}\n"))?;
    tcp.set_nodelay(true).ok();
    let mut stream: Box<dyn ReadWrite> = if parts.scheme == "https" {
        wrap_tls(&parts.host, tcp)?
    } else {
        Box::new(tcp)
    };
    let host_hdr = host_header(&parts);
    let request = format!(
        "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n{headers}\r\n{body}",
        parts.path, host_hdr
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("probe write failed for {url}: {err}\n"))?;
    if Instant::now() >= deadline {
        return Err(format!("probe deadline exceeded for {url}\n"));
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    tcp_set_read_deadline(stream.as_mut(), remaining);
    let (status, headers, body, _rest) = read_http_capped(stream.as_mut(), Some(PROBE_MAX_BODY), Some(deadline))?;
    Ok(ProbeResponse {
        status,
        headers,
        body,
    })
}

#[cfg(target_os = "wasi")]
pub fn http_probe(_url: &str, _accept: &str) -> Result<ProbeResponse, String> {
    Err("probes are not available in this build\n".to_string())
}

#[cfg(target_os = "wasi")]
pub fn http_post_json(_url: &str, _body: &str) -> Result<ProbeResponse, String> {
    Err("http_post_json not available on wasi\n".to_string())
}

pub fn ws_connect(url: &str) -> Result<Ws, String> {
    let parts = parse_url(url)?;
    if parts.scheme != "ws" && parts.scheme != "wss" {
        return Err(format!("WebSocket URL must be ws or wss: {url}\n"));
    }
    let mut stream = dial(&parts)?;
    let key = ws_key();
    let host_header = host_header(&parts);
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
        parts.path, host_header, key
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| connect_error(&parts, &err.to_string()))?;
    let (status, headers, body, rest) = read_http(stream.as_mut())?;
    if status != 101 {
        return Err(format_http_error(status, &body));
    }
    let accept = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("upgrade"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    if !accept.to_ascii_lowercase().contains("websocket") && !accept.is_empty() {
        return Err(format_http_error(status, &body));
    }
    Ok(Ws {
        stream,
        buf: rest,
    })
}

impl Ws {
    pub fn send_text(&mut self, text: &str) -> Result<(), String> {
        self.note_server_close()?;
        let frame = encode_client_frame(0x1, text.as_bytes(), mask_key(text.len()));
        self.stream
            .write_all(&frame)
            .map_err(|err| format!("websocket write failed: {err}\n"))?;
        self.stream
            .flush()
            .map_err(|err| format!("websocket write failed: {err}\n"))?;
        Ok(())
    }

    fn note_server_close(&mut self) -> Result<(), String> {
        if let Some(message) = buffered_close(&self.buf) {
            return Err(message);
        }
        let timeout = self.stream.read_timeout()
            .map_err(|err| format!("websocket timeout setup failed: {err}\n"))?;
        self.stream.set_nonblocking(true)
            .map_err(|err| format!("websocket nonblocking setup failed: {err}\n"))?;
        if let Err(err) = self.stream.set_read_timeout(None) {
            let _ = self.stream.set_nonblocking(false);
            return Err(format!("websocket timeout setup failed: {err}\n"));
        }
        let deadline = Instant::now() + Duration::from_millis(2);
        let mut drained = 0;
        let mut tmp = [0u8; 8192];
        let mut failed = None;
        while drained < 65536 && Instant::now() < deadline {
            match self.stream.read(&mut tmp) {
                Ok(0) => {
                    failed = Some(
                        buffered_close(&self.buf)
                            .unwrap_or_else(|| "websocket connection closed\n".to_string()),
                    );
                    break;
                }
                Ok(n) => {
                    self.buf.extend_from_slice(&tmp[..n]);
                    drained += n;
                    if let Some(message) = buffered_close(&self.buf) {
                        failed = Some(message);
                        break;
                    }
                }
                Err(err)
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::TimedOut
                        || err.kind() == std::io::ErrorKind::Interrupted =>
                {
                    break;
                }
                Err(err) => {
                    failed = Some(
                        buffered_close(&self.buf)
                            .unwrap_or_else(|| format!("websocket read failed: {err}\n")),
                    );
                    break;
                }
            }
        }
        let restored_timeout = self.stream.set_read_timeout(timeout);
        let restored_blocking = self.stream.set_nonblocking(false);
        restored_timeout.map_err(|err| format!("websocket timeout restore failed: {err}\n"))?;
        restored_blocking.map_err(|err| format!("websocket nonblocking restore failed: {err}\n"))?;
        if let Some(message) = failed {
            return Err(message);
        }
        if let Some(message) = buffered_close(&self.buf) {
            return Err(message);
        }
        Ok(())
    }

    pub fn recv_text(&mut self, deadline: std::time::Instant) -> Result<Option<String>, String> {
        let mut fragments: Vec<u8> = Vec::new();
        let mut kind = 0u8;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            self.stream
                .set_read_timeout(Some(remaining.clamp(Duration::from_millis(1), READ_SLICE)))
                .map_err(|err| format!("websocket timeout setup failed: {err}\n"))?;
            let frame = match next_frame(&mut self.buf, self.stream.as_mut())? {
                Some(frame) => frame,
                None => continue,
            };
            match frame.opcode {
                0x1 | 0x2 => {
                    kind = frame.opcode;
                    if frame.fin {
                        if frame.opcode == 0x2 {
                            return Err("unexpected binary websocket frame\n".to_string());
                        }
                        return Ok(Some(
                            String::from_utf8(frame.payload)
                                .map_err(|err| format!("websocket text: {err}\n"))?,
                        ));
                    }
                    fragments = frame.payload;
                }
                0x0 => {
                    fragments.extend_from_slice(&frame.payload);
                    if frame.fin {
                        if kind == 0x2 {
                            return Err("unexpected binary websocket frame\n".to_string());
                        }
                        return Ok(Some(
                            String::from_utf8(fragments)
                                .map_err(|err| format!("websocket text: {err}\n"))?,
                        ));
                    }
                }
                0x8 => return Err(close_message(&frame.payload)),
                0x9 => {
                    self.note_server_close()?;
                    let pong = encode_client_frame(0xA, &frame.payload, mask_key(frame.payload.len()));
                    self.stream
                        .write_all(&pong)
                        .map_err(|err| format!("websocket pong failed: {err}\n"))?;
                }
                0xA => {}
                other => return Err(format!("unsupported websocket opcode {other}\n")),
            }
        }
    }
}

struct Frame {
    opcode: u8,
    fin: bool,
    payload: Vec<u8>,
}

fn next_frame(buf: &mut Vec<u8>, stream: &mut dyn ReadWrite) -> Result<Option<Frame>, String> {
    if let Some((consumed, frame)) = try_decode(buf)? {
        buf.drain(..consumed);
        return Ok(Some(frame));
    }
    let mut tmp = [0u8; 8192];
    match stream.read(&mut tmp) {
        Ok(0) => Err("websocket connection closed\n".to_string()),
        Ok(n) => {
            buf.extend_from_slice(&tmp[..n]);
            if let Some((consumed, frame)) = try_decode(buf)? {
                buf.drain(..consumed);
                Ok(Some(frame))
            } else {
                Ok(None)
            }
        }
        Err(err)
            if err.kind() == std::io::ErrorKind::TimedOut
                || err.kind() == std::io::ErrorKind::WouldBlock =>
        {
            Ok(None)
        }
        Err(err) => Err(format!("websocket read failed: {err}\n")),
    }
}

fn try_decode(buf: &[u8]) -> Result<Option<(usize, Frame)>, String> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let b0 = buf[0];
    let b1 = buf[1];
    let fin = b0 & 0x80 != 0;
    let opcode = b0 & 0x0f;
    let masked = b1 & 0x80 != 0;
    let mut len = (b1 & 0x7f) as u64;
    let mut offset = 2usize;
    if len == 126 {
        if buf.len() < 4 {
            return Ok(None);
        }
        len = u16::from_be_bytes([buf[2], buf[3]]) as u64;
        offset = 4;
    } else if len == 127 {
        if buf.len() < 10 {
            return Ok(None);
        }
        len = u64::from_be_bytes(buf[2..10].try_into().unwrap());
        offset = 10;
    }
    let mask = if masked {
        if buf.len() < offset + 4 {
            return Ok(None);
        }
        let mask = [buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]];
        offset += 4;
        Some(mask)
    } else {
        None
    };
    let len_usize = usize::try_from(len).map_err(|_| "websocket frame too large\n".to_string())?;
    if buf.len() < offset + len_usize {
        return Ok(None);
    }
    let mut payload = buf[offset..offset + len_usize].to_vec();
    if let Some(mask) = mask {
        for (i, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[i % 4];
        }
    }
    Ok(Some((
        offset + len_usize,
        Frame {
            opcode,
            fin,
            payload,
        },
    )))
}

pub fn encode_client_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    for (i, byte) in payload.iter().enumerate() {
        out.push(byte ^ mask[i % 4]);
    }
    out
}

fn buffered_close(buf: &[u8]) -> Option<String> {
    let mut offset = 0;
    while offset < buf.len() {
        match try_decode(&buf[offset..]) {
            Ok(Some((consumed, frame))) => {
                if frame.opcode == 0x8 {
                    return Some(close_message(&frame.payload));
                }
                offset += consumed;
            }
            _ => return None,
        }
    }
    None
}

fn close_message(payload: &[u8]) -> String {
    if payload.len() < 2 {
        return "websocket closed\n".to_string();
    }
    let code = u16::from_be_bytes([payload[0], payload[1]]);
    let reason = String::from_utf8_lossy(&payload[2..]);
    if reason.is_empty() {
        format!("websocket closed {code}\n")
    } else {
        format!("websocket closed {code} {reason}\n")
    }
}

fn mask_key(seed: usize) -> [u8; 4] {
    let n = seed as u32 ^ 0xA5A5_5A5A;
    n.to_le_bytes()
}

fn ws_key() -> String {
    let ticks = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(1);
    let mut bytes = [0u8; 16];
    let mut x = ticks ^ 0x9E37_79B9_7F4A_7C15;
    for (i, byte) in bytes.iter_mut().enumerate() {
        x = x
            .wrapping_mul(0x6C62_272E_07BB_0142)
            .wrapping_add(i as u128 + 1);
        *byte = (x >> 11) as u8;
    }
    base64_encode(&bytes)
}

pub fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | data[i + 2] as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(TABLE[((n >> 6) & 63) as usize] as char);
        out.push(TABLE[(n & 63) as usize] as char);
        i += 3;
    }
    if i < data.len() {
        let rest = data.len() - i;
        let mut n = (data[i] as u32) << 16;
        if rest == 2 {
            n |= (data[i + 1] as u32) << 8;
        }
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        if rest == 2 {
            out.push(TABLE[((n >> 6) & 63) as usize] as char);
            out.push('=');
        } else {
            out.push('=');
            out.push('=');
        }
    }
    out
}

pub fn base64_decode(data: &str) -> Result<Vec<u8>, String> {
    fn val(byte: u8) -> Result<u8, String> {
        match byte {
            b'A'..=b'Z' => Ok(byte - b'A'),
            b'a'..=b'z' => Ok(byte - b'a' + 26),
            b'0'..=b'9' => Ok(byte - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err("invalid base64\n".to_string()),
        }
    }
    let bytes: Vec<u8> = data
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if bytes.len() % 4 != 0 {
        return Err("invalid base64\n".to_string());
    }
    let mut out = Vec::new();
    for chunk in bytes.chunks(4) {
        let pad = chunk.iter().filter(|b| **b == b'=').count();
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        let c = if chunk[2] == b'=' { 0 } else { val(chunk[2])? };
        let d = if chunk[3] == b'=' { 0 } else { val(chunk[3])? };
        let n = ((a as u32) << 18) | ((b as u32) << 12) | ((c as u32) << 6) | d as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

fn parse_url(url: &str) -> Result<Parts, String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("unsupported CDP URL \"{url}\"\n"))?;
    let scheme = scheme.to_ascii_lowercase();
    let cut = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..cut];
    let path: String = if cut == rest.len() || rest.as_bytes()[cut] == b'#' {
        "/".to_string()
    } else if rest.as_bytes()[cut] == b'/' {
        rest[cut..].to_string()
    } else {
        format!("/{}", &rest[cut..])
    };
    let hostport = match authority.rsplit_once('@') {
        Some((_, host)) => host,
        None => authority,
    };
    if hostport.is_empty() {
        return Err(format!("bad CDP URL \"{url}\"\n"));
    }
    let (host, port_text) = if let Some(host) = hostport.strip_prefix('[') {
        let end = host
            .find(']')
            .ok_or_else(|| format!("bad CDP URL \"{url}\"\n"))?;
        let host = &host[..end];
        let after = &hostport[end + 2..];
        if !after.is_empty() && !after.starts_with(':') {
            return Err(format!("bad CDP URL \"{url}\"\n"));
        }
        let port = after.strip_prefix(':');
        (host.to_string(), port.map(str::to_string))
    } else if let Some((host, port)) = hostport.rsplit_once(':') {
        if host.contains(':') {
            (hostport.to_string(), None)
        } else if port.is_empty() {
            return Err(format!("bad port in CDP URL \"{url}\"\n"));
        } else {
            (host.to_string(), Some(port.to_string()))
        }
    } else {
        (hostport.to_string(), None)
    };
    let default_port = match scheme.as_str() {
        "https" | "wss" => 443,
        _ => 80,
    };
    let port = match port_text {
        Some(text) => text
            .parse::<u16>()
            .map_err(|_| format!("bad port in CDP URL \"{url}\"\n"))?,
        None => default_port,
    };
    if host.is_empty() {
        return Err(format!("bad CDP URL \"{url}\"\n"));
    }
    let path = if path.is_empty() { "/".to_string() } else { path };
    Ok(Parts {
        scheme,
        host,
        port,
        path,
    })
}

fn host_header(parts: &Parts) -> String {
    let default = matches!(
        (parts.scheme.as_str(), parts.port),
        ("http" | "ws", 80) | ("https" | "wss", 443)
    );
    if default {
        parts.host.clone()
    } else if parts.host.contains(':') {
        format!("[{}]:{}", parts.host, parts.port)
    } else {
        format!("{}:{}", parts.host, parts.port)
    }
}

fn connect_error(parts: &Parts, err: &str) -> String {
    format!(
        "CDP connection to {}:{} failed: {err}. This CLI does not launch Chrome. Pass --cdp or set SLICC_CDP_URL.\n",
        parts.host, parts.port
    )
}

fn dial(parts: &Parts) -> Result<Box<dyn ReadWrite>, String> {
    refuse_real_debug_port(parts);
    let addr = resolve(&parts.host, parts.port)?;
    refuse_real_debug_addr(addr);
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|err| {
        connect_error(parts, &err.to_string())
    })?;
    tcp.set_read_timeout(Some(READ_SLICE))
        .map_err(|err| connect_error(parts, &err.to_string()))?;
    tcp.set_nodelay(true).ok();
    if parts.scheme == "https" || parts.scheme == "wss" {
        wrap_tls(&parts.host, tcp)
    } else {
        Ok(Box::new(tcp))
    }
}

fn refuse_real_debug_port(parts: &Parts) {
    if (cfg!(test) || crate::ports::testing()) && crate::ports::real_debug_port(&parts.host, parts.port) {
        panic!("test dialed {}:{}", parts.host, parts.port);
    }
}

fn refuse_real_debug_addr(addr: SocketAddr) {
    if (cfg!(test) || crate::ports::testing()) && crate::ports::real_debug_port(&addr.ip().to_string(), addr.port()) {
        panic!("test dialed {addr}");
    }
}

fn tcp_set_read_deadline(_stream: &mut dyn ReadWrite, _remaining: Duration) {}

fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    wasix_net::resolve(host, port)
        .map_err(|err| format!("could not resolve {host}: {err}\n"))?
        .into_iter()
        .next()
        .ok_or_else(|| format!("could not resolve {host}\n"))
}

#[cfg(not(target_os = "wasi"))]
fn wrap_tls(host: &str, tcp: TcpStream) -> Result<Box<dyn ReadWrite>, String> {
    let connector = native_tls::TlsConnector::new()
        .map_err(|err| format!("TLS setup failed: {err}\n"))?;
    let stream = connector
        .connect(host, tcp)
        .map_err(|err| format!("TLS handshake with {host} failed: {err}\n"))?;
    Ok(Box::new(stream))
}

#[cfg(target_os = "wasi")]
fn wrap_tls(_host: &str, _tcp: TcpStream) -> Result<Box<dyn ReadWrite>, String> {
    Err("wss and https are not available in this build\n".to_string())
}

fn read_http(stream: &mut dyn ReadWrite) -> Result<(u16, Vec<(String, String)>, Vec<u8>, Vec<u8>), String> {
    read_http_capped(stream, None, None)
}

fn read_http_capped(
    stream: &mut dyn ReadWrite,
    body_cap: Option<usize>,
    deadline: Option<Instant>,
) -> Result<(u16, Vec<(String, String)>, Vec<u8>, Vec<u8>), String> {
    let mut buf = Vec::new();
    let header_end = loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err("probe deadline exceeded\n".to_string());
        }
        if let Some(pos) = find_header_end(&buf) {
            break pos;
        }
        let mut tmp = [0u8; 4096];
        match stream.read(&mut tmp) {
            Ok(0) => {
                return Err("HTTP connection closed before headers\n".to_string());
            }
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(err)
                if err.kind() == std::io::ErrorKind::TimedOut
                    || err.kind() == std::io::ErrorKind::WouldBlock =>
            {
                if buf.is_empty() {
                    continue;
                }
                return Err("timed out reading HTTP headers\n".to_string());
            }
            Err(err) => return Err(format!("HTTP read failed: {err}\n")),
        }
        if buf.len() > 1024 * 1024 {
            return Err("HTTP headers too large\n".to_string());
        }
    };
    let header_bytes = buf[..header_end].to_vec();
    let mut rest = buf[header_end + 4..].to_vec();
    let header_text = String::from_utf8_lossy(&header_bytes);
    let mut lines = header_text.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|text| text.parse::<u16>().ok())
        .ok_or_else(|| format!("bad HTTP status line: {status_line}\n"))?;
    let mut headers = Vec::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    let content_length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok());
    let chunked = headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("transfer-encoding") && value.to_ascii_lowercase().contains("chunked")
    });
    let close = headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("connection") && value.eq_ignore_ascii_case("close")
    });
    if status == 101 && !chunked && content_length.is_none() {
        return Ok((status, headers, Vec::new(), rest));
    }
    let body = if chunked {
        read_chunked_capped(&mut rest, stream, body_cap, deadline)?
    } else if let Some(len) = content_length {
        if let Some(cap) = body_cap {
            if len > cap {
                return Err("HTTP body exceeds cap\n".to_string());
            }
        }
        let limit = body_cap.map(|c| c.min(len)).unwrap_or(len);
        while rest.len() < limit {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Err("probe deadline exceeded\n".to_string());
            }
            let mut tmp = [0u8; 4096];
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => rest.extend_from_slice(&tmp[..n]),
                Err(err)
                    if err.kind() == std::io::ErrorKind::TimedOut
                        || err.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        return Err("probe deadline exceeded\n".to_string());
                    }
                    continue;
                }
                Err(err) => return Err(format!("HTTP body read failed: {err}\n")),
            }
        }
        let body = rest[..limit.min(rest.len())].to_vec();
        let extra = if status == 101 {
            rest[limit.min(rest.len())..].to_vec()
        } else {
            Vec::new()
        };
        return Ok((status, headers, body, extra));
    } else {
        let close_cap = body_cap.unwrap_or(8 * 1024 * 1024);
        loop {
            if deadline.is_some_and(|d| Instant::now() >= d) {
                break;
            }
            let mut tmp = [0u8; 4096];
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => rest.extend_from_slice(&tmp[..n]),
                Err(err)
                    if err.kind() == std::io::ErrorKind::TimedOut
                        || err.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    break;
                }
                Err(err) => return Err(format!("HTTP body read failed: {err}\n")),
            }
            if rest.len() > close_cap {
                break;
            }
        }
        rest
    };
    let _ = close;
    let body = if let Some(cap) = body_cap {
        if body.len() > cap { body[..cap].to_vec() } else { body }
    } else {
        body
    };
    Ok((status, headers, body, Vec::new()))
}

#[allow(dead_code)]
fn read_chunked(rest: &mut Vec<u8>, stream: &mut dyn ReadWrite) -> Result<Vec<u8>, String> {
    read_chunked_capped(rest, stream, None, None)
}

fn read_chunked_capped(
    rest: &mut Vec<u8>,
    stream: &mut dyn ReadWrite,
    cap: Option<usize>,
    deadline: Option<Instant>,
) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Ok(body);
        }
        let line_end = loop {
            if let Some(pos) = rest.windows(2).position(|w| w == b"\r\n") {
                break pos;
            }
            let mut tmp = [0u8; 2048];
            match stream.read(&mut tmp) {
                Ok(0) => return Err("chunked body ended early\n".to_string()),
                Ok(n) => rest.extend_from_slice(&tmp[..n]),
                Err(err)
                    if err.kind() == std::io::ErrorKind::TimedOut
                        || err.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        return Ok(body);
                    }
                    continue;
                }
                Err(err) => return Err(format!("chunked read failed: {err}\n")),
            }
        };
        let line = String::from_utf8_lossy(&rest[..line_end]).to_string();
        rest.drain(..line_end + 2);
        let size_text = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| format!("bad chunk size {size_text}\n"))?;
        if size == 0 {
            return Ok(body);
        }
        while rest.len() < size + 2 {
            let mut tmp = [0u8; 4096];
            match stream.read(&mut tmp) {
                Ok(0) => return Err("chunked body ended early\n".to_string()),
                Ok(n) => rest.extend_from_slice(&tmp[..n]),
                Err(err)
                    if err.kind() == std::io::ErrorKind::TimedOut
                        || err.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        return Ok(body);
                    }
                    continue;
                }
                Err(err) => return Err(format!("chunked read failed: {err}\n")),
            }
        }
        body.extend_from_slice(&rest[..size]);
        rest.drain(..size + 2);
        if let Some(limit) = cap {
            if body.len() > limit {
                body.truncate(limit);
                return Ok(body);
            }
        }
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ReadyStream {
        state: std::rc::Rc<std::cell::RefCell<(usize, bool, Option<Duration>)>>,
        slow: bool,
        broken: bool,
    }

    impl Read for ReadyStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut state = self.state.borrow_mut();
            assert!(state.1);
            assert_eq!(state.2, None);
            state.0 += 1;
            if self.broken { return Err(std::io::ErrorKind::BrokenPipe.into()); }
            let n = if self.slow {
                std::thread::sleep(Duration::from_millis(1));
                4
            } else { buf.len() };
            for chunk in buf[..n].chunks_exact_mut(4) { chunk.copy_from_slice(b"\x81\x02{}"); }
            Ok(n)
        }
    }

    impl Write for ReadyStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let state = self.state.borrow();
            assert!(!state.1);
            assert_eq!(state.2, Some(Duration::from_millis(200)));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    impl ReadWrite for ReadyStream {
        fn set_nonblocking(&mut self, on: bool) -> std::io::Result<()> { self.state.borrow_mut().1 = on; Ok(()) }
        fn read_timeout(&self) -> std::io::Result<Option<Duration>> { Ok(self.state.borrow().2) }
        fn set_read_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> { self.state.borrow_mut().2 = timeout; Ok(()) }
    }

    #[test]
    fn pre_send_draining_is_bounded_and_restores_socket_options() {
        for (slow, broken) in [(false, false), (true, false), (false, true)] {
            let state = std::rc::Rc::new(std::cell::RefCell::new((0, false, Some(Duration::from_millis(200)))));
            let mut ws = Ws { stream: Box::new(ReadyStream { state: state.clone(), slow, broken }), buf: Vec::new() };
            let result = ws.send_text("{}");
            assert_eq!(result.is_err(), broken);
            let state = state.borrow();
            assert!(state.0 > 0);
            assert!(state.0 <= if slow { 2 } else { 8 });
            assert!(!state.1);
            assert_eq!(state.2, Some(Duration::from_millis(200)));
            assert!(ws.buf.len() <= 65536);
        }
    }

    struct SilentStream {
        timeout: Option<Duration>,
    }

    impl Read for SilentStream {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(self.timeout.unwrap_or(Duration::from_secs(5)));
            Err(std::io::ErrorKind::TimedOut.into())
        }
    }

    impl Write for SilentStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> { Ok(buf.len()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    impl ReadWrite for SilentStream {
        fn set_nonblocking(&mut self, _on: bool) -> std::io::Result<()> { Ok(()) }
        fn read_timeout(&self) -> std::io::Result<Option<Duration>> { Ok(self.timeout) }
        fn set_read_timeout(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
            if timeout == Some(Duration::ZERO) { return Err(std::io::ErrorKind::InvalidInput.into()); }
            self.timeout = timeout;
            Ok(())
        }
    }

    #[test]
    fn short_receive_deadline_is_not_stretched_by_the_socket_timeout() {
        let stream = SilentStream { timeout: Some(READ_SLICE) };
        let mut ws = Ws { stream: Box::new(stream), buf: Vec::new() };
        let started = Instant::now();
        assert_eq!(ws.recv_text(started + Duration::from_millis(5)).unwrap(), None);
        assert!(started.elapsed() < Duration::from_millis(100), "{:?}", started.elapsed());
        let started = Instant::now();
        assert_eq!(ws.recv_text(started + Duration::from_millis(450)).unwrap(), None);
        let spent = started.elapsed();
        assert!(spent >= Duration::from_millis(450) && spent < Duration::from_millis(700), "{spent:?}");
    }

    #[test]
    fn decodes_unmasked_text_frame() {
        let payload = b"hello";
        let mut frame = vec![0x81, payload.len() as u8];
        frame.extend_from_slice(payload);
        let parsed = try_decode(&frame).unwrap().unwrap();
        assert_eq!(parsed.1.payload, b"hello");
        assert!(parsed.1.fin);
        assert_eq!(parsed.1.opcode, 0x1);
    }

    #[test]
    fn client_frame_round_trip_mask() {
        let encoded = encode_client_frame(0x1, b"{}", [1, 2, 3, 4]);
        assert_eq!(encoded[0], 0x81);
        assert_eq!(encoded[1] & 0x80, 0x80);
        let parsed = try_decode(&encoded).unwrap().unwrap();
        assert_eq!(parsed.1.payload, b"{}");
    }

    #[test]
    fn base64_round_trip() {
        assert_eq!(base64_decode(&base64_encode(b"hi")).unwrap(), b"hi");
        assert_eq!(base64_decode(&base64_encode(&[0, 1, 2, 3, 4])).unwrap(), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn close_frame_reports_code_and_reason() {
        assert_eq!(
            close_message(b"\x03\xf3the CDP host is gone"),
            "websocket closed 1011 the CDP host is gone\n"
        );
        assert_eq!(
            close_message(b"\x03\xf1a message is over 268435456 bytes"),
            "websocket closed 1009 a message is over 268435456 bytes\n"
        );
        assert_eq!(close_message(&[]), "websocket closed\n");
        assert_eq!(close_message(&[0x03, 0xf3]), "websocket closed 1011\n");
    }

    #[test]
    fn tests_do_not_dial_reserved_debug_ports() {
        let err = std::panic::catch_unwind(|| {
            let _ = http_get("http://127.0.0.1:9222/json/version");
        });
        assert!(err.is_err());
        let err = std::panic::catch_unwind(|| {
            let _ = ws_connect("ws://localhost:9229/devtools/browser/abc");
        });
        assert!(err.is_err());
        for url in [
            "http://127.1:9223/json/version",
            "http://2130706433:9224/json/version",
            "http://[::ffff:127.0.0.1]:9225/json/version",
            "http://0.0.0.0:9226/json/version",
        ] {
            assert!(std::panic::catch_unwind(|| http_get(url)).is_err(), "{url}");
        }
    }

    #[test]
    fn parses_opaque_browser_path() {
        let parts = parse_url("ws://127.0.0.1:9222/devtools/browser/abc-def?x=1").unwrap();
        assert_eq!(parts.host, "127.0.0.1");
        assert_eq!(parts.port, 9222);
        assert_eq!(parts.path, "/devtools/browser/abc-def?x=1");
    }

    #[test]
    fn userinfo_is_not_part_of_the_dial_host() {
        let same = parse_url("http://evil.example@127.0.0.1:9222/json/version").unwrap();
        assert_eq!(same.scheme, "http");
        assert_eq!(same.host, "127.0.0.1");
        assert_eq!(same.port, 9222);
        assert_eq!(same.path, "/json/version");
        let other = parse_url("http://127.0.0.1:9222@evil.example/json/version").unwrap();
        assert_eq!(other.host, "evil.example");
        assert_eq!(other.port, 80);
        assert_eq!(other.path, "/json/version");
        let v6 = parse_url("http://user@[::1]:9333/json/version").unwrap();
        assert_eq!(v6.host, "::1");
        assert_eq!(v6.port, 9333);
        let query = parse_url("http://127.0.0.1:9333?x=1").unwrap();
        assert_eq!(query.host, "127.0.0.1");
        assert_eq!(query.port, 9333);
        assert_eq!(query.path, "/?x=1");
        let published = parse_endpoint("HTTP://evil.example@127.0.0.1:9222").unwrap();
        assert_eq!(published.scheme, "http");
        assert_eq!(published.host, "127.0.0.1");
        assert_eq!(published.port, 9222);
    }

    #[cfg(not(target_os = "wasi"))]
    fn accept_headers(listener: std::net::TcpListener) -> std::net::TcpStream {
        let (mut sock, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = sock.read(&mut tmp).unwrap();
            buf.extend_from_slice(&tmp[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        sock
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn http_get_uses_the_stream() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut sock = accept_headers(listener);
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .unwrap();
        });
        let body = http_get(&format!("http://{addr}/json/version")).unwrap();
        assert_eq!(body.body, b"{}");
        server.join().unwrap();
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn websocket_uses_the_stream() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut sock = accept_headers(listener);
            sock.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                .unwrap();
            sock.write_all(&[0x81, 5, b'h', b'e', b'l', b'l', b'o']).unwrap();
        });
        let mut ws = ws_connect(&format!("ws://{addr}/devtools/browser/abc")).unwrap();
        let text = ws
            .recv_text(std::time::Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert_eq!(text.as_deref(), Some("hello"));
        server.join().unwrap();
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn silent_websocket_returns_at_a_short_deadline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (done, wait) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let mut sock = accept_headers(listener);
            sock.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                .unwrap();
            let _ = wait.recv();
        });
        let mut ws = ws_connect(&format!("ws://{addr}/devtools/browser/abc")).unwrap();
        for _ in 0..3 {
            let started = Instant::now();
            assert_eq!(ws.recv_text(started + Duration::from_millis(5)).unwrap(), None);
            assert!(started.elapsed() < Duration::from_millis(100), "{:?}", started.elapsed());
        }
        done.send(()).unwrap();
        server.join().unwrap();
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn server_close_is_reported_and_not_answered() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut sock = accept_headers(listener);
            let reason = b"the CDP host is gone";
            let mut message = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n".to_vec();
            message.extend_from_slice(&[0x81, 5, b'h', b'e', b'l', b'l', b'o']);
            message.extend_from_slice(&[0x88, (2 + reason.len()) as u8, 0x03, 0xf3]);
            message.extend_from_slice(reason);
            sock.write_all(&message).unwrap();
            sock.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
            let mut tmp = [0u8; 64];
            match sock.read(&mut tmp) {
                Ok(n) => n,
                Err(err)
                    if err.kind() == std::io::ErrorKind::WouldBlock
                        || err.kind() == std::io::ErrorKind::TimedOut =>
                {
                    0
                }
                Err(_) => 0,
            }
        });
        let mut ws = ws_connect(&format!("ws://{addr}/devtools/browser/abc")).unwrap();
        let text = ws
            .recv_text(std::time::Instant::now() + Duration::from_secs(2))
            .unwrap();
        assert_eq!(text.as_deref(), Some("hello"));
        let err = ws.send_text("{}").unwrap_err();
        assert_eq!(err, "websocket closed 1011 the CDP host is gone\n");
        assert_eq!(server.join().unwrap(), 0);
    }

    #[test]
    fn probe_response_content_type() {
        let resp = ProbeResponse {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "application/json; charset=utf-8".into()),
            ],
            body: b"{}".to_vec(),
        };
        assert_eq!(resp.content_type(), "application/json; charset=utf-8");
    }

    #[test]
    fn probe_response_missing_content_type() {
        let resp = ProbeResponse {
            status: 200,
            headers: vec![],
            body: b"{}".to_vec(),
        };
        assert_eq!(resp.content_type(), "");
    }

    #[test]
    fn probe_html_200_never_matches_ard() {
        let resp = ProbeResponse {
            status: 200,
            headers: vec![("Content-Type".into(), "text/html".into())],
            body: b"<html>".to_vec(),
        };
        assert_eq!(resp.status, 200);
        assert_eq!(resp.content_type(), "text/html");
    }

    #[test]
    fn probe_404_never_matches_ard() {
        let resp = ProbeResponse {
            status: 404,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: b"{}".to_vec(),
        };
        assert_ne!(resp.status, 200);
    }

    #[test]
    fn probe_301_never_matches_ard() {
        let resp = ProbeResponse {
            status: 301,
            headers: vec![
                ("Location".into(), "https://example.com/.well-known/ard.json".into()),
                ("Content-Type".into(), "application/json".into()),
            ],
            body: Vec::new(),
        };
        assert_ne!(resp.status, 200);
    }

    #[test]
    fn probe_body_cap_constant() {
        assert_eq!(PROBE_MAX_BODY, 1024 * 1024);
    }

    #[test]
    fn probe_does_not_dial_reserved_ports() {
        let err = std::panic::catch_unwind(|| {
            let _ = http_probe("http://127.0.0.1:9222/.well-known/ard.json", "application/json");
        });
        assert!(err.is_err(), "probe must not dial 9222");
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn body_cap_content_length_truncates() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut sock = accept_headers(listener);
            let big = vec![b'X'; 5 * 1024 * 1024];
            sock.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    big.len()
                )
                .as_bytes(),
            )
            .unwrap();
            let _ = sock.write_all(&big);
        });
        let resp = http_probe(
            &format!("http://{addr}/.well-known/ard.json"),
            "application/json",
        );
        server.join().unwrap();
        match resp {
            Ok(r) => assert!(
                r.body.len() <= PROBE_MAX_BODY,
                "body should be capped at {PROBE_MAX_BODY}, got {}",
                r.body.len()
            ),
            Err(e) => assert!(
                e.contains("cap") || e.contains("exceeds"),
                "expected cap error, got: {e}"
            ),
        }
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn body_cap_chunked_truncates() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut sock = accept_headers(listener);
            sock.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
            let chunk = vec![b'Y'; 512 * 1024];
            for _ in 0..10 {
                let _ = sock.write_all(format!("{:x}\r\n", chunk.len()).as_bytes());
                let _ = sock.write_all(&chunk);
                let _ = sock.write_all(b"\r\n");
            }
            let _ = sock.write_all(b"0\r\n\r\n");
        });
        let resp = http_probe(
            &format!("http://{addr}/.well-known/ard.json"),
            "application/json",
        )
        .unwrap();
        assert!(
            resp.body.len() <= PROBE_MAX_BODY,
            "chunked body must be capped, got {}",
            resp.body.len()
        );
        server.join().unwrap();
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn a_json_post_sends_its_body_through_the_probe_path() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = Vec::new();
            let mut tmp = [0u8; 1024];
            while !String::from_utf8_lossy(&request).ends_with("{\"a\":1}") {
                let n = sock.read(&mut tmp).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&tmp[..n]);
            }
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
            String::from_utf8_lossy(&request).into_owned()
        });
        let resp = http_post_json(&format!("http://{addr}/mcp"), "{\"a\":1}").unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"{}");
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /mcp HTTP/1.1\r\n"), "{request}");
        assert!(request.contains("\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: 7\r\n\r\n{\"a\":1}"), "{request}");
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn a_chunked_reply_left_open_returns_what_arrived_by_the_deadline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let mut sock = accept_headers(listener);
            let event = b"data: {}\n\n";
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").unwrap();
            sock.write_all(format!("{:x}\r\n", event.len()).as_bytes()).unwrap();
            sock.write_all(event).unwrap();
            sock.write_all(b"\r\n").unwrap();
            let _ = done_rx.recv_timeout(Duration::from_secs(20));
        });
        let started = Instant::now();
        let resp = http_post_json(&format!("http://{addr}/mcp"), "{}").unwrap();
        assert!(started.elapsed() < Duration::from_secs(8));
        assert_eq!(resp.body, b"data: {}\n\n");
        done_tx.send(()).unwrap();
        server.join().unwrap();
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn a_chunk_cut_short_and_left_open_returns_by_the_deadline() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let mut sock = accept_headers(listener);
            sock.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n10\r\nabc").unwrap();
            let _ = done_rx.recv_timeout(Duration::from_secs(20));
        });
        let started = Instant::now();
        let resp = http_probe(&format!("http://{addr}/x"), "*/*").unwrap();
        assert!(started.elapsed() < Duration::from_secs(8));
        assert!(resp.body.is_empty());
        done_tx.send(()).unwrap();
        server.join().unwrap();
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn a_json_post_refuses_non_http_urls() {
        assert!(http_post_json("ftp://example.com/mcp", "{}").is_err());
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn body_cap_close_delimited_truncates() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut sock = accept_headers(listener);
            sock.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
            let chunk = vec![b'Z'; 256 * 1024];
            for _ in 0..20 {
                if sock.write_all(&chunk).is_err() {
                    break;
                }
            }
        });
        let resp = http_probe(
            &format!("http://{addr}/.well-known/ard.json"),
            "application/json",
        )
        .unwrap();
        assert!(
            resp.body.len() <= PROBE_MAX_BODY,
            "close-delimited body must be capped, got {}",
            resp.body.len()
        );
        server.join().unwrap();
    }

    #[cfg(not(target_os = "wasi"))]
    #[test]
    fn stalled_body_probe_returns_within_6s() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut sock = accept_headers(listener);
            sock.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
            let _ = sock.write_all(&[b'A'; 10]);
            std::thread::sleep(Duration::from_secs(30));
        });
        let start = std::time::Instant::now();
        let _ = http_probe(
            &format!("http://{addr}/.well-known/ard.json"),
            "application/json",
        );
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(7),
            "stalled probe must return within 7s, took {elapsed:?}"
        );
        drop(server);
    }
}
