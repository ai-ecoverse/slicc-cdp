use super::execute;
use super::Output;
use cdp_client::net::base64_encode;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone)]
struct PageSpec {
    target_id: String,
    url: String,
    title: String,
    frames: Vec<(String, i64)>,
}

#[derive(Clone)]
struct Call {
    method: String,
    session_id: Option<String>,
    params: Value,
}

#[derive(Clone, Copy)]
enum SliccReply {
    Ack,
    NotFound,
    Silent,
}

struct Shared {
    log: Mutex<Vec<Call>>,
    discovery_path: Mutex<String>,
    ws_path: Mutex<String>,
    slicc: Mutex<SliccReply>,
}

struct Running {
    port: u16,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

fn one_page() -> Vec<PageSpec> {
    vec![PageSpec {
        target_id: "T1".to_string(),
        url: "https://app.example/home".to_string(),
        title: "App".to_string(),
        frames: vec![("MAIN".to_string(), 1), ("FCHILD".to_string(), 9)],
    }]
}

fn serve(pages: Vec<PageSpec>) -> Running {
    serve_mode(pages, SliccReply::Ack)
}

fn serve_mode(pages: Vec<PageSpec>, slicc: SliccReply) -> Running {
    let shared = Arc::new(Shared {
        log: Mutex::new(Vec::new()),
        discovery_path: Mutex::new(String::new()),
        ws_path: Mutex::new(String::new()),
        slicc: Mutex::new(slicc),
    });
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let shared_thread = Arc::clone(&shared);
    let stop_thread = Arc::clone(&stop);
    let join = thread::spawn(move || {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        tx.send(port).unwrap();
        while !stop_thread.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nodelay(true).ok();
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                    handle_conn(&mut stream, port, &pages, &shared_thread, &stop_thread);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });
    let port = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    Running { port, shared, stop, join: Some(join) }
}

fn handle_conn(stream: &mut std::net::TcpStream, port: u16, pages: &[PageSpec], shared: &Shared, stop: &AtomicBool) {
    let mut buf = Vec::new();
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos;
        }
        let mut tmp = [0u8; 2048];
        match stream.read(&mut tmp) {
            Ok(0) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut => {
                if stop.load(Ordering::SeqCst) || !buf.is_empty() {
                    return;
                }
                continue;
            }
            Err(_) => return,
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let first = head.lines().next().unwrap_or("");
    let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
    let upgrade = head.to_ascii_lowercase().contains("upgrade: websocket");
    if !upgrade {
        *shared.discovery_path.lock().unwrap() = path;
        let body = format!(r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/test"}}"#);
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(resp.as_bytes());
        return;
    }
    *shared.ws_path.lock().unwrap() = path;
    let _ = stream.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n");
    let mut pending = buf[header_end + 4..].to_vec();
    loop {
        while let Some((taken, payload)) = decode_frame(&pending) {
            pending.drain(..taken);
            if payload.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(&payload).to_string();
            for response in on_message(&text, pages, shared) {
                let _ = stream.write_all(&server_text(&response));
            }
        }
        let mut tmp = [0u8; 8192];
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => pending.extend_from_slice(&tmp[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            }
            Err(_) => break,
        }
    }
}

fn on_message(text: &str, pages: &[PageSpec], shared: &Shared) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let Some(id) = value.get("id").cloned() else {
        return Vec::new();
    };
    let method = value.get("method").and_then(|item| item.as_str()).unwrap_or("").to_string();
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    let session_id = value.get("sessionId").and_then(|item| item.as_str()).map(str::to_string);
    shared.log.lock().unwrap().push(Call { method: method.clone(), session_id: session_id.clone(), params: params.clone() });
    if method == "Slicc.action" {
        let mode = *shared.slicc.lock().unwrap();
        return match mode {
            SliccReply::Ack => vec![wrap(id, json!({}))],
            SliccReply::NotFound => vec![json!({"id": id, "error": {"message": "'Slicc.action' wasn't found"}}).to_string()],
            SliccReply::Silent => Vec::new(),
        };
    }
    match method.as_str() {
        "Target.getTargets" => vec![wrap(id, json!({
            "targetInfos": pages.iter().map(|page| json!({
                "targetId": page.target_id,
                "type": "page",
                "url": page.url,
                "title": page.title,
            })).collect::<Vec<_>>()
        }))],
        "Target.attachToTarget" => {
            let flatten = params.get("flatten").and_then(|item| item.as_bool()) == Some(true);
            let target = params.get("targetId").and_then(|item| item.as_str()).unwrap_or("");
            if !flatten {
                return vec![json!({"id": id, "error": {"message": "only flatten: true is supported"}}).to_string()];
            }
            if !pages.iter().any(|page| page.target_id == target) {
                return vec![json!({"id": id, "error": {"message": "No target with given id found"}}).to_string()];
            }
            vec![wrap(id, json!({ "sessionId": format!("s-{target}") }))]
        }
        "Target.detachFromTarget" => vec![wrap(id, json!({}))],
        "Runtime.enable" => {
            let session = session_id.unwrap_or_default();
            let target = session.strip_prefix("s-").unwrap_or("");
            let mut messages = Vec::new();
            if let Some(page) = pages.iter().find(|page| page.target_id == target) {
                for (frame_id, context_id) in &page.frames {
                    messages.push(json!({
                        "method": "Runtime.executionContextCreated",
                        "sessionId": session,
                        "params": {
                            "context": {
                                "id": context_id,
                                "origin": "https://app.example",
                                "auxData": {"isDefault": true, "type": "default", "frameId": frame_id}
                            }
                        }
                    }).to_string());
                }
            }
            messages.push(wrap(id, json!({})));
            messages
        }
        "Runtime.evaluate" => {
            if session_id.is_none() {
                return vec![json!({"id": id, "error": {"message": "missing sessionId"}}).to_string()];
            }
            let expr = params.get("expression").and_then(|item| item.as_str()).unwrap_or("");
            let context = params.get("contextId").and_then(|item| item.as_i64());
            vec![wrap(id, interpret(expr, context))]
        }
        _ => vec![wrap(id, json!({}))],
    }
}

fn wrap(id: Value, result: Value) -> String {
    json!({"id": id, "result": result}).to_string()
}

fn interpret(expr: &str, context: Option<i64>) -> Value {
    if expr.contains("AbortSignal.timeout") {
        return json!({
            "exceptionDetails": {
                "text": "TimeoutError: signal timed out",
                "exception": {"description": "TimeoutError: signal timed out"}
            }
        });
    }
    if expr.contains("/frame-child") {
        if context != Some(9) {
            return json!({
                "exceptionDetails": {
                    "text": "missing frame context",
                    "exception": {"description": "missing frame context"}
                }
            });
        }
        return ok_page(200, "OK", "https://app.example/frame-child", &[("content-type", "text/plain")], b"frame-ok");
    }
    if expr.contains("/api/me") {
        let body: &[u8] = if expr.contains("\"credentials\":\"include\"") { b"session=from-tab" } else { b"anonymous" };
        return ok_page(200, "OK", "https://app.example/api/me", &[("content-type", "text/plain")], body);
    }
    if expr.contains("/json-post") {
        let encoded = base64_encode(br#"{"a":1}"#);
        let good = expr.contains("\"method\":\"POST\"") && expr.contains("application/json") && expr.contains(&encoded);
        let body: &[u8] = if good { br#"{"ok":true}"# } else { b"bad-json" };
        return ok_page(200, "OK", "https://app.example/json-post", &[("content-type", "application/json")], body);
    }
    if expr.contains("/upload") {
        let encoded = base64_encode(b"hello-bytes");
        let good = expr.contains("up.bin") && expr.contains("application/octet-stream") && expr.contains(&encoded);
        let body: &[u8] = if good { b"uploaded" } else { b"bad-upload" };
        return ok_page(200, "OK", "https://app.example/upload", &[("content-type", "text/plain")], body);
    }
    if expr.contains("/show") {
        return ok_page(200, "OK", "https://app.example/show", &[("x-test", "yes")], b"hi");
    }
    if expr.contains("/only-head") {
        return ok_page(200, "OK", "https://app.example/only-head", &[("x-head", "1")], b"SECRET");
    }
    if expr.contains("/dump") {
        return ok_page(200, "OK", "https://app.example/dump", &[("x-dump", "1")], b"body-text");
    }
    if expr.contains("/code") {
        return ok_page(201, "Created", "https://app.example/code", &[], b"");
    }
    if expr.contains("/nope") {
        return ok_page(500, "Error", "https://app.example/nope", &[("content-type", "text/plain")], b"err");
    }
    if expr.contains("/picked") {
        return ok_page(200, "OK", "https://app.example/picked", &[("content-type", "text/plain")], b"picked");
    }
    ok_page(200, "OK", "https://app.example/", &[], b"ok")
}

fn ok_page(status: u16, status_text: &str, url: &str, headers: &[(&str, &str)], body: &[u8]) -> Value {
    let mut map = serde_json::Map::new();
    for (name, value) in headers {
        map.insert((*name).to_string(), json!(value));
    }
    json!({
        "result": {
            "type": "object",
            "value": {
                "ok": status < 400,
                "status": status,
                "statusText": status_text,
                "url": url,
                "redirected": false,
                "headers": map,
                "body": base64_encode(body),
                "bodyEncoding": "base64"
            }
        }
    })
}

fn decode_frame(buf: &[u8]) -> Option<(usize, Vec<u8>)> {
    if buf.len() < 2 {
        return None;
    }
    let b1 = buf[1];
    let masked = b1 & 0x80 != 0;
    let mut len = (b1 & 0x7f) as usize;
    let mut offset = 2usize;
    if len == 126 {
        if buf.len() < 4 { return None; }
        len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
        offset = 4;
    } else if len == 127 {
        if buf.len() < 10 { return None; }
        len = u64::from_be_bytes(buf[2..10].try_into().ok()?) as usize;
        offset = 10;
    }
    let mask = if masked {
        if buf.len() < offset + 4 { return None; }
        let mask = [buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3]];
        offset += 4;
        Some(mask)
    } else {
        None
    };
    if buf.len() < offset + len {
        return None;
    }
    let mut payload = buf[offset..offset + len].to_vec();
    if let Some(mask) = mask {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    let _ = buf[0];
    Some((offset + len, payload))
}

fn server_text(payload: &str) -> Vec<u8> {
    let bytes = payload.as_bytes();
    let mut out = vec![0x81];
    if bytes.len() < 126 {
        out.push(bytes.len() as u8);
    } else if bytes.len() <= u16::MAX as usize {
        out.push(126);
        out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    }
    out.extend_from_slice(bytes);
    out
}

fn run_at(port: u16, args: &[&str]) -> Output {
    let mut argv = vec!["--cdp".to_string(), format!("http://127.0.0.1:{port}")];
    argv.extend(args.iter().map(|arg| (*arg).to_string()));
    execute(&argv, None)
}

fn expr_of(shared: &Shared) -> String {
    shared.log.lock().unwrap().iter().rev().find(|call| call.method == "Runtime.evaluate").map(|call| {
        call.params.get("expression").and_then(|value| value.as_str()).unwrap_or("").to_string()
    }).unwrap_or_default()
}

#[test]
fn get_shows_tab_cookies_over_discovered_browser_socket() {
    let running = serve(one_page());
    let output = run_at(running.port, &["https://app.example/api/me"]);
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(output.as_text(), "session=from-tab");
    assert!(running.shared.discovery_path.lock().unwrap().contains("/json/version"));
    let ws = running.shared.ws_path.lock().unwrap().clone();
    assert!(ws.contains("/devtools/browser/"));
    assert!(!ws.contains("/devtools/page/"));
    let log = running.shared.log.lock().unwrap();
    let attach = log.iter().find(|call| call.method == "Target.attachToTarget").unwrap();
    assert_eq!(attach.params.get("flatten").and_then(|value| value.as_bool()), Some(true));
    assert!(attach.session_id.is_none());
    let eval = log.iter().find(|call| call.method == "Runtime.evaluate").unwrap();
    assert_eq!(eval.session_id.as_deref(), Some("s-T1"));
    assert!(eval.params.get("contextId").is_none());
}

#[test]
fn json_post_form_upload_headers_write_out_fail_and_timeout() {
    let running = serve(one_page());
    let json_out = run_at(running.port, &["--json", r#"{"a":1}"#, "https://app.example/json-post"]);
    assert_eq!(json_out.code, 0, "{}", json_out.stderr);
    assert_eq!(json_out.as_text(), r#"{"ok":true}"#);

    let path = std::env::temp_dir().join(format!("curlwright-upload-{}", std::process::id()));
    std::fs::write(&path, b"hello-bytes").unwrap();
    let upload = run_at(running.port, &["-F", &format!("up=@{};filename=up.bin", path.display()), "https://app.example/upload"]);
    let _ = std::fs::remove_file(&path);
    assert_eq!(upload.code, 0, "{}", upload.stderr);
    assert_eq!(upload.as_text(), "uploaded");

    let shown = run_at(running.port, &["-i", "https://app.example/show"]);
    assert_eq!(shown.code, 0, "{}", shown.stderr);
    assert!(shown.as_text().starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(shown.as_text().contains("x-test: yes\r\n"));
    assert!(shown.as_text().ends_with("\r\n\r\nhi"));

    let head = run_at(running.port, &["-I", "https://app.example/only-head"]);
    assert_eq!(head.code, 0, "{}", head.stderr);
    assert!(head.as_text().contains("HTTP/1.1 200 OK"));
    assert!(!head.as_text().contains("SECRET"));
    assert!(expr_of(&running.shared).contains("\"method\":\"HEAD\""));

    let dump_path = std::env::temp_dir().join(format!("curlwright-dump-{}", std::process::id()));
    let dumped = run_at(running.port, &["-D", dump_path.to_str().unwrap(), "https://app.example/dump"]);
    let dumped_file = std::fs::read_to_string(&dump_path).unwrap_or_default();
    let _ = std::fs::remove_file(&dump_path);
    assert_eq!(dumped.code, 0, "{}", dumped.stderr);
    assert_eq!(dumped.as_text(), "body-text");
    assert!(dumped_file.contains("HTTP/1.1 200 OK\r\n"));
    assert!(dumped_file.contains("x-dump: 1\r\n"));

    let code = run_at(running.port, &["-w", "%{http_code}", "https://app.example/code"]);
    assert_eq!(code.code, 0, "{}", code.stderr);
    assert_eq!(code.as_text(), "201");

    let failed = run_at(running.port, &["-f", "https://app.example/nope"]);
    assert_eq!(failed.code, 22, "{}", failed.stderr);
    assert_eq!(failed.as_text(), "");
    assert!(failed.stderr.contains("curlwright: (22) The requested URL returned error: 500"));

    let timed = run_at(running.port, &["-m", "1", "-w", "%{http_code}", "https://app.example/slow"]);
    assert_eq!(timed.code, 28, "{}", timed.stderr);
    assert!(timed.stderr.contains("curlwright: (28) Operation timed out"));
    assert_eq!(timed.as_text(), "0");
}

#[test]
fn tab_and_frame_selection() {
    let pages = vec![
        PageSpec { target_id: "A".into(), url: "https://app.example/one".into(), title: "One".into(), frames: vec![] },
        PageSpec { target_id: "B".into(), url: "https://app.example/two".into(), title: "Two".into(), frames: vec![("FCHILD".into(), 9)] },
    ];
    let running = serve(pages);
    let ambiguous = run_at(running.port, &["https://app.example/picked"]);
    assert_eq!(ambiguous.code, 2, "{}", ambiguous.stderr);
    assert!(ambiguous.stderr.contains("2 open tabs are on https://app.example"));
    assert!(ambiguous.stderr.contains("--tab=A"));
    assert!(ambiguous.stderr.contains("--tab=B"));
    assert!(running.shared.log.lock().unwrap().iter().all(|call| call.method != "Runtime.evaluate"));

    let picked = run_at(running.port, &["--tab", "B", "https://app.example/picked"]);
    assert_eq!(picked.code, 0, "{}", picked.stderr);
    assert_eq!(picked.as_text(), "picked");
    let session = running.shared.log.lock().unwrap().iter().rev().find(|call| call.method == "Runtime.evaluate").unwrap().session_id.clone();
    assert_eq!(session.as_deref(), Some("s-B"));

    let framed = run_at(running.port, &["--tab", "B", "--frame", "FCHILD", "https://app.example/frame-child"]);
    assert_eq!(framed.code, 0, "{}", framed.stderr);
    assert_eq!(framed.as_text(), "frame-ok");
    let eval = running.shared.log.lock().unwrap().iter().rev().find(|call| call.method == "Runtime.evaluate").unwrap().params.clone();
    assert_eq!(eval.get("contextId").and_then(|value| value.as_i64()), Some(9));
}

#[test]
fn a_single_foreign_tab_is_not_a_guess() {
    let pages = vec![PageSpec {
        target_id: "OTHER".into(),
        url: "https://other.example/dash".into(),
        title: "Other".into(),
        frames: vec![],
    }];
    let running = serve(pages);
    let output = run_at(running.port, &["https://app.example/api"]);
    assert_eq!(output.code, 2, "{}", output.stderr);
    assert!(output.stderr.contains("--tab=OTHER"));
    assert!(output.stderr.contains("https://other.example/dash"));
    assert!(running.shared.log.lock().unwrap().iter().all(|call| call.method != "Runtime.evaluate"));
}

#[test]
fn a_missing_action_reply_does_not_change_the_response() {
    let args = ["-H", "Authorization: secret-token", "-d", "secret-text", "https://app.example/api/me"];
    let started = std::time::Instant::now();
    let ack = serve_mode(one_page(), SliccReply::Ack);
    let missing = serve_mode(one_page(), SliccReply::NotFound);
    let silent = serve_mode(one_page(), SliccReply::Silent);
    let acknowledged = run_at(ack.port, &args);
    let rejected = run_at(missing.port, &args);
    let quiet = run_at(silent.port, &args);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert_eq!(acknowledged.code, 0, "{}", acknowledged.stderr);
    assert_eq!(acknowledged.stdout, rejected.stdout);
    assert_eq!(acknowledged.stderr, rejected.stderr);
    assert_eq!(acknowledged.code, rejected.code);
    assert_eq!(acknowledged.stdout, quiet.stdout);
    assert_eq!(acknowledged.stderr, quiet.stderr);
    assert_eq!(acknowledged.code, quiet.code);
    assert_eq!(acknowledged.as_text(), "session=from-tab");
    assert!(!acknowledged.stderr.contains("wasn't found"));
    for running in [&ack, &missing, &silent] {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while std::time::Instant::now() < deadline {
            let ready = running.shared.log.lock().unwrap().iter().filter(|call| call.method == "Slicc.action").count() >= 2;
            if ready {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let log = running.shared.log.lock().unwrap();
        let actions: Vec<&Call> = log.iter().filter(|call| call.method == "Slicc.action").collect();
        assert_eq!(actions.len(), 2);
        assert!(actions.iter().all(|call| call.session_id.is_none()));
        let start = &actions[0].params;
        assert_eq!(start["phase"], "start");
        assert_eq!(start["kind"], "request");
        assert_eq!(start["method"], "POST");
        assert_eq!(start["url"], "https://app.example/api/me");
        assert_eq!(start["tab"], "T1");
        let rendered = start.to_string();
        assert!(!rendered.contains("secret-text"));
        assert!(!rendered.contains("secret-token"));
        assert!(!rendered.contains("Authorization"));
        assert!(start.get("headers").is_none());
        assert!(start.get("body").is_none());
        assert_eq!(actions[1].params["phase"], "end");
        assert_eq!(actions[1].params["ok"], true);
        assert_eq!(actions[1].params["tab"], "T1");
        assert!(actions[1].params.get("error").is_none());
        let names: Vec<&str> = log.iter().map(|call| call.method.as_str()).collect();
        let resolved = names.iter().position(|name| *name == "Target.getTargets").unwrap();
        let began = names.iter().position(|name| *name == "Slicc.action").unwrap();
        let fetched = names.iter().position(|name| *name == "Runtime.evaluate").unwrap();
        let ended = names.iter().rposition(|name| *name == "Slicc.action").unwrap();
        assert!(resolved < began && began < fetched && fetched < ended);
        let eval = log.iter().find(|call| call.method == "Runtime.evaluate").unwrap();
        let expression = eval.params["expression"].as_str().unwrap_or("");
        assert!(expression.contains("c2VjcmV0LXRleHQ="));
        assert!(expression.contains("secret-token"));
    }
}

#[test]
fn a_verbose_failure_reports_the_error_line_not_the_trace() {
    let running = serve_mode(one_page(), SliccReply::NotFound);
    let output = run_at(
        running.port,
        &[
            "-v",
            "-f",
            "-H",
            "Authorization: secret-token",
            "https://app.example/nope",
        ],
    );
    assert_eq!(output.code, 22, "{}", output.stderr);
    assert!(output.stderr.starts_with("> GET https://app.example/nope\n"));
    assert!(output.stderr.contains("> Authorization: secret-token\n"));
    assert!(output.stderr.contains("curlwright: (22) The requested URL returned error: 500\n"));
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        let ready = running
            .shared
            .log
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.method == "Slicc.action")
            .count()
            >= 2;
        if ready {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let log = running.shared.log.lock().unwrap().clone();
    let actions: Vec<&Call> = log.iter().filter(|call| call.method == "Slicc.action").collect();
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[1].params["ok"], false);
    assert_eq!(actions[1].params["error"], "failed");
    let rendered = actions.iter().map(|call| call.params.to_string()).collect::<String>();
    assert!(!rendered.contains("secret-token"));
    assert!(!rendered.contains("Authorization"));
    assert!(!rendered.contains("> GET"));
}

#[test]
fn request_actions_keep_origin_and_path_only() {
    let secret = "https://user:pass@app.example/form?q=QUERYSECRET&tok=x#FRAGSECRET";
    let cases: [&[&str]; 2] = [&[secret], &["-d", "secret-body", secret]];
    for args in cases {
        let running = serve_mode(one_page(), SliccReply::Silent);
        let output = run_at(running.port, args);
        assert_eq!(output.code, 0, "{}", output.stderr);
        let actions = wait_actions(&running, 2);
        let start = &actions[0].params;
        assert_eq!(start["url"], "https://app.example/form");
        assert_eq!(actions[1].params["ok"], true);
        assert!(actions[1].params.get("error").is_none());
        let rendered = actions
            .iter()
            .map(|call| call.params.to_string())
            .collect::<String>();
        for needle in ["QUERYSECRET", "FRAGSECRET", "user:pass", "tok=x", "secret-body"] {
            assert!(!rendered.contains(needle), "{rendered}");
        }
        if args.len() == 1 {
            assert_eq!(start["method"], "GET");
        } else {
            assert_eq!(start["method"], "POST");
        }
    }
}

#[test]
fn a_foreign_tab_reports_not_allowed_without_the_secret_url() {
    let pages = vec![PageSpec {
        target_id: "OTHER".into(),
        url: "https://other.example/dash".into(),
        title: "Other".into(),
        frames: vec![],
    }];
    let running = serve_mode(pages, SliccReply::Silent);
    let output = run_at(
        running.port,
        &["https://user:pass@app.example/form?q=QUERYSECRET&tok=x#FRAGSECRET"],
    );
    assert_eq!(output.code, 2, "{}", output.stderr);
    assert!(output.stderr.contains("no open tab is on"));
    let actions = wait_actions(&running, 2);
    assert_eq!(actions[0].params["url"], "https://app.example/form");
    assert_eq!(actions[0].params["method"], "GET");
    assert_eq!(actions[1].params["ok"], false);
    assert_eq!(actions[1].params["error"], "not allowed");
    let rendered = actions
        .iter()
        .map(|call| call.params.to_string())
        .collect::<String>();
    for needle in ["QUERYSECRET", "FRAGSECRET", "user:pass", "tok=x"] {
        assert!(!rendered.contains(needle), "{rendered}");
    }
}

#[test]
fn a_scheme_relative_url_drops_userinfo_on_the_action() {
    let running = serve_mode(one_page(), SliccReply::Silent);
    let _output = run_at(
        running.port,
        &[
            "--tab",
            "T1",
            "//user:pass@app.example/form?q=QUERYSECRET&tok=x#FRAGSECRET",
        ],
    );
    let actions = wait_actions(&running, 2);
    assert_eq!(actions.len(), 2);
    assert_eq!(actions[0].params["url"], "//app.example/form");
    assert_eq!(actions[0].params["method"], "GET");
    let rendered = actions
        .iter()
        .map(|call| call.params.to_string())
        .collect::<String>();
    for needle in ["QUERYSECRET", "FRAGSECRET", "user:pass", "tok=x"] {
        assert!(!rendered.contains(needle), "{rendered}");
    }
}

fn wait_actions(running: &Running, count: usize) -> Vec<Call> {
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        let ready = running
            .shared
            .log
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.method == "Slicc.action")
            .count()
            >= count;
        if ready {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    running
        .shared
        .log
        .lock()
        .unwrap()
        .iter()
        .filter(|call| call.method == "Slicc.action")
        .cloned()
        .collect()
}
