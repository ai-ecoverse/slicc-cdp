use crate::connect::format_http_error;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

pub struct HttpResponse {
    pub body: Vec<u8>,
}

pub struct Ws {
    stream: Box<dyn ReadWrite>,
    buf: Vec<u8>,
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

struct Parts {
    scheme: String,
    host: String,
    port: u16,
    path: String,
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
        let frame = encode_client_frame(0x1, text.as_bytes(), mask_key(text.len()));
        self.stream
            .write_all(&frame)
            .map_err(|err| format!("websocket write failed: {err}\n"))?;
        self.stream
            .flush()
            .map_err(|err| format!("websocket write failed: {err}\n"))?;
        Ok(())
    }

    pub fn recv_text(&mut self, deadline: std::time::Instant) -> Result<Option<String>, String> {
        let mut fragments: Vec<u8> = Vec::new();
        let mut kind = 0u8;
        loop {
            if std::time::Instant::now() >= deadline {
                return Ok(None);
            }
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

fn base64_encode(data: &[u8]) -> String {
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
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (host, port_text) = if let Some(host) = authority.strip_prefix('[') {
        let end = host
            .find(']')
            .ok_or_else(|| format!("bad CDP URL \"{url}\"\n"))?;
        let host = &host[..end];
        let after = &authority[end + 2..];
        let port = after.strip_prefix(':');
        (host.to_string(), port.map(str::to_string))
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if host.contains(':') {
            (authority.to_string(), None)
        } else {
            (host.to_string(), Some(port.to_string()))
        }
    } else {
        (authority.to_string(), None)
    };
    let default_port = match scheme {
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
    let path = if path.is_empty() { "/".to_string() } else { path.to_string() };
    Ok(Parts {
        scheme: scheme.to_string(),
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
    let addr = resolve(&parts.host, parts.port)?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(10)).map_err(|err| {
        connect_error(parts, &err.to_string())
    })?;
    tcp.set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|err| connect_error(parts, &err.to_string()))?;
    tcp.set_nodelay(true).ok();
    if parts.scheme == "https" || parts.scheme == "wss" {
        wrap_tls(&parts.host, tcp)
    } else {
        Ok(Box::new(tcp))
    }
}

fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    let mut addrs = (host, port)
        .to_socket_addrs()
        .map_err(|err| format!("could not resolve {host}: {err}\n"))?;
    addrs
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
    let mut buf = Vec::new();
    let header_end = loop {
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
        read_chunked(&mut rest, stream)?
    } else if let Some(len) = content_length {
        while rest.len() < len {
            let mut tmp = [0u8; 4096];
            match stream.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => rest.extend_from_slice(&tmp[..n]),
                Err(err)
                    if err.kind() == std::io::ErrorKind::TimedOut
                        || err.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    continue;
                }
                Err(err) => return Err(format!("HTTP body read failed: {err}\n")),
            }
        }
        let body = rest[..len.min(rest.len())].to_vec();
        let extra = if status == 101 {
            rest[len.min(rest.len())..].to_vec()
        } else {
            Vec::new()
        };
        return Ok((status, headers, body, extra));
    } else {
        loop {
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
            if rest.len() > 8 * 1024 * 1024 {
                break;
            }
        }
        rest
    };
    let _ = close;
    Ok((status, headers, body, Vec::new()))
}

fn read_chunked(rest: &mut Vec<u8>, stream: &mut dyn ReadWrite) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
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
                    continue;
                }
                Err(err) => return Err(format!("chunked read failed: {err}\n")),
            }
        }
        body.extend_from_slice(&rest[..size]);
        rest.drain(..size + 2);
    }
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parses_opaque_browser_path() {
        let parts = parse_url("ws://127.0.0.1:9222/devtools/browser/abc-def?x=1").unwrap();
        assert_eq!(parts.host, "127.0.0.1");
        assert_eq!(parts.port, 9222);
        assert_eq!(parts.path, "/devtools/browser/abc-def?x=1");
    }
}
