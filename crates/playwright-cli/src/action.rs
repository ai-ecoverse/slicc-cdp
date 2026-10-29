use crate::args::Output;
use crate::cdp::{self, Cdp};

pub struct Report {
    note: cdp::ActionNote,
    started: bool,
    ended: bool,
    active: bool,
}

impl Report {
    pub fn for_command(command: &str) -> Self {
        let kind = match command {
            "open" | "tab-new" => "open",
            "goto" | "navigate" => "goto",
            "close" | "tab-close" => "close",
            "tab-select" => "select",
            "snapshot" => "snapshot",
            "screenshot" => "screenshot",
            "eval" => "eval",
            "click" => "click",
            "fill" => "fill",
            "type" => "type",
            "press" => "press",
            _ => {
                return Self {
                    note: cdp::ActionNote::default(),
                    started: false,
                    ended: false,
                    active: false,
                };
            }
        };
        Self {
            note: cdp::ActionNote {
                kind: kind.to_string(),
                agent: cdp::slicc_agent(),
                ..cdp::ActionNote::default()
            },
            started: false,
            ended: false,
            active: true,
        }
    }

    #[cfg(test)]
    pub fn kind(&self) -> Option<&str> {
        if self.active {
            Some(self.note.kind.as_str())
        } else {
            None
        }
    }

    pub fn tab(&mut self, tab: impl Into<String>) {
        self.note.tab = Some(tab.into());
    }

    pub fn url(&mut self, url: impl Into<String>) {
        self.note.url = Some(url.into());
    }

    pub fn target(&mut self, label: &str) {
        if !label.is_empty() {
            self.note.target = Some(label.to_string());
        }
    }

    pub fn key(&mut self, key: impl Into<String>) {
        self.note.key = Some(key.into());
    }

    pub fn length(&mut self, count: usize) {
        self.note.length = Some(count as u64);
    }

    pub fn begin(&mut self, cdp: &mut Cdp) {
        if !self.active || self.started || self.ended {
            return;
        }
        self.started = true;
        let _ = cdp.notify("Slicc.action", self.note.start_params(), None);
    }

    pub fn finish(&mut self, cdp: &mut Cdp, output: &Output) {
        if !self.active || self.ended {
            return;
        }
        if !self.started {
            self.begin(cdp);
        }
        self.ended = true;
        let _ = cdp.notify(
            "Slicc.action",
            self.note.end_params(output.code == 0, &output.stderr),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_kinds_match_the_panel() {
        assert_eq!(Report::for_command("open").kind(), Some("open"));
        assert_eq!(Report::for_command("tab-new").kind(), Some("open"));
        assert_eq!(Report::for_command("goto").kind(), Some("goto"));
        assert_eq!(Report::for_command("close").kind(), Some("close"));
        assert_eq!(Report::for_command("tab-close").kind(), Some("close"));
        assert_eq!(Report::for_command("tab-select").kind(), Some("select"));
        assert_eq!(Report::for_command("snapshot").kind(), Some("snapshot"));
        assert_eq!(Report::for_command("screenshot").kind(), Some("screenshot"));
        assert_eq!(Report::for_command("eval").kind(), Some("eval"));
        assert_eq!(Report::for_command("click").kind(), Some("click"));
        assert_eq!(Report::for_command("fill").kind(), Some("fill"));
        assert_eq!(Report::for_command("type").kind(), Some("type"));
        assert_eq!(Report::for_command("press").kind(), Some("press"));
        assert_eq!(Report::for_command("tab-list").kind(), None);
        assert_eq!(Report::for_command("nope").kind(), None);
        for name in [
            "console",
            "requests",
            "request",
            "request-headers",
            "request-body",
            "response-headers",
            "response-body",
        ] {
            assert_eq!(Report::for_command(name).kind(), None, "{name}");
        }
    }

    #[cfg(not(target_os = "wasi"))]
    mod peer {
        use super::*;
        use serde_json::{json, Value};
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::path::Path;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};
        use std::thread::{self, JoinHandle};
        use std::time::{Duration, Instant};

        #[derive(Clone, Copy)]
        enum SliccReply {
            NotFound,
            Silent,
            Takeover,
        }

        const TAKEOVER: &str = "another page took over the local proxy’s browser connection (superseded-by-new-cdp-client)";

        struct Peer {
            port: u16,
            log: Arc<Mutex<Vec<Value>>>,
            stop: Arc<AtomicBool>,
            join: Option<JoinHandle<()>>,
        }

        impl Peer {
            fn start(mode: SliccReply, pages: bool) -> Self {
                let log = Arc::new(Mutex::new(Vec::new()));
                let stop = Arc::new(AtomicBool::new(false));
                let (tx, rx) = std::sync::mpsc::channel();
                let log_thread = Arc::clone(&log);
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
                                let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                                handle_conn(
                                    &mut stream,
                                    port,
                                    mode,
                                    pages,
                                    &log_thread,
                                    &stop_thread,
                                );
                            }
                            Err(err) if timed_out(&err) => thread::sleep(Duration::from_millis(5)),
                            Err(_) => break,
                        }
                    }
                });
                let port = rx.recv_timeout(Duration::from_secs(2)).unwrap();
                Self {
                    port,
                    log,
                    stop,
                    join: Some(join),
                }
            }

            fn wait_actions(&self, count: usize) {
                let deadline = Instant::now() + Duration::from_secs(1);
                while Instant::now() < deadline {
                    if actions(&self.log.lock().unwrap()).len() >= count {
                        return;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }

        impl Drop for Peer {
            fn drop(&mut self) {
                self.stop.store(true, Ordering::SeqCst);
                if let Some(handle) = self.join.take() {
                    let _ = handle.join();
                }
            }
        }

        fn handle_conn(
            stream: &mut std::net::TcpStream,
            port: u16,
            mode: SliccReply,
            pages: bool,
            log: &Mutex<Vec<Value>>,
            stop: &AtomicBool,
        ) {
            let mut buf = Vec::new();
            let header_end = loop {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                if let Some(pos) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
                    break pos;
                }
                let mut tmp = [0u8; 4096];
                match stream.read(&mut tmp) {
                    Ok(0) => return,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    Err(err) if timed_out(&err) => continue,
                    Err(_) => return,
                }
            };
            let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
            if !head.to_ascii_lowercase().contains("upgrade: websocket") {
                let body = format!(
                    r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/test"}}"#
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                return;
            }
            let _ = stream.write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n",
            );
            let mut pending = buf[header_end + 4..].to_vec();
            loop {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                while let Some((taken, payload)) = decode_frame(&pending) {
                    pending.drain(..taken);
                    if payload.is_empty() {
                        continue;
                    }
                    let text = String::from_utf8_lossy(&payload);
                    if let Some(frames) = on_message(&text, mode, pages, log) {
                        for frame in frames {
                            if frame == "\u{0}close" {
                                let reason = TAKEOVER.as_bytes();
                                let mut close = vec![0x88, (2 + reason.len()) as u8, 0x03, 0xf3];
                                close.extend_from_slice(reason);
                                let _ = stream.write_all(&close);
                                let deadline = Instant::now() + Duration::from_secs(3);
                                let mut sink = [0u8; 4096];
                                while Instant::now() < deadline && !stop.load(Ordering::SeqCst) {
                                    match stream.read(&mut sink) {
                                        Ok(0) => break,
                                        Ok(_) => {}
                                        Err(err) if timed_out(&err) => {}
                                        Err(_) => break,
                                    }
                                }
                                return;
                            }
                            let _ = stream.write_all(&server_text(&frame));
                        }
                    }
                }
                let mut tmp = [0u8; 8192];
                match stream.read(&mut tmp) {
                    Ok(0) => return,
                    Ok(n) => pending.extend_from_slice(&tmp[..n]),
                    Err(err) if timed_out(&err) => continue,
                    Err(_) => return,
                }
            }
        }

        fn on_message(
            text: &str,
            mode: SliccReply,
            pages: bool,
            log: &Mutex<Vec<Value>>,
        ) -> Option<Vec<String>> {
            let value: Value = serde_json::from_str(text).ok()?;
            let id = value.get("id").cloned()?;
            log.lock().unwrap().push(value.clone());
            let method = value
                .get("method")
                .and_then(|item| item.as_str())
                .unwrap_or("");
            let expression = value
                .pointer("/params/expression")
                .and_then(|item| item.as_str())
                .unwrap_or("");
            if expression.contains("TAKEOVER")
                || (matches!(mode, SliccReply::Takeover) && method == "Target.setDiscoverTargets")
            {
                return Some(vec!["\u{0}close".to_string()]);
            }
            if method == "Slicc.action" {
                return match mode {
                    SliccReply::Silent | SliccReply::Takeover => None,
                    SliccReply::NotFound => Some(vec![
                        json!({"id": id, "error": {"message": "'Slicc.action' wasn't found"}})
                            .to_string(),
                    ]),
                };
            }
            let params = value.get("params").cloned().unwrap_or(Value::Null);
            let result = match method {
                "Target.getTargets" if pages => json!({
                    "targetInfos": [{
                        "targetId": "TAB1",
                        "type": "page",
                        "url": "https://app.example/",
                        "title": "App"
                    }]
                }),
                "Target.getTargets" => json!({ "targetInfos": [] }),
                "Target.createTarget" => json!({ "targetId": "TAB1" }),
                "Target.attachToTarget" => {
                    let calls = log.lock().unwrap();
                    let previous = calls
                        .len()
                        .checked_sub(2)
                        .and_then(|index| calls.get(index))
                        .and_then(|call| call.get("method"))
                        .and_then(|item| item.as_str())
                        .unwrap_or("")
                        .to_string();
                    if previous == "Target.setDiscoverTargets" {
                        json!({ "sessionId": "guard" })
                    } else {
                        json!({ "sessionId": "sess" })
                    }
                }
                "Page.getFrameTree" => json!({"frameTree":{"frame":{"id":"F","loaderId":"L","url":"https://app.example/","securityOrigin":"https://app.example"}}}),
                "DOM.resolveNode" => json!({ "object": { "objectId": "obj-1" } }),
                "Runtime.evaluate" => {
                    let expr = params
                        .get("expression")
                        .and_then(|item| item.as_str())
                        .unwrap_or("");
                    if expr.contains("EVALSECRET-in-error") {
                        json!({
                            "result": { "type": "object", "subtype": "error" },
                            "exceptionDetails": {
                                "text": "Uncaught",
                                "exception": { "description": "Error: EVALSECRET-in-error" }
                            }
                        })
                    } else if expr == "document.readyState" {
                        json!({ "result": { "type": "string", "value": "complete" } })
                    } else {
                        json!({ "result": { "type": "string", "value": "ok" } })
                    }
                }
                _ => json!({}),
            };
            let mut frames = vec![json!({"id": id, "result": result}).to_string()];
            if method == "Runtime.enable" {
                frames.insert(0, json!({"method":"Runtime.executionContextCreated","sessionId":value["sessionId"],"params":{"context":{"id":1,"uniqueId":"peer-context","origin":"https://app.example","name":"","auxData":{"isDefault":true,"frameId":"F"}}}}).to_string());
            }
            if method == "Page.navigate" {
                let url = params
                    .get("url")
                    .and_then(|item| item.as_str())
                    .unwrap_or("http://navigated.test/");
                frames.push(
                    json!({
                        "method": "Page.frameNavigated",
                        "params": { "frame": { "id": "frame", "url": url } },
                        "sessionId": "sess"
                    })
                    .to_string(),
                );
                frames.push(
                    json!({
                        "method": "Page.loadEventFired",
                        "params": {},
                        "sessionId": "sess"
                    })
                    .to_string(),
                );
            }
            Some(frames)
        }

        fn timed_out(err: &std::io::Error) -> bool {
            matches!(
                err.kind(),
                std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted
            )
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
                if buf.len() < 4 {
                    return None;
                }
                len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
                offset = 4;
            } else if len == 127 {
                if buf.len() < 10 {
                    return None;
                }
                len = u64::from_be_bytes(buf[2..10].try_into().ok()?) as usize;
                offset = 10;
            }
            let mask = if masked {
                if buf.len() < offset + 4 {
                    return None;
                }
                let mask = [
                    buf[offset],
                    buf[offset + 1],
                    buf[offset + 2],
                    buf[offset + 3],
                ];
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
            Some((offset + len, payload))
        }

        fn server_text(payload: &str) -> Vec<u8> {
            let bytes = payload.as_bytes();
            let mut out = vec![0x81];
            if bytes.len() < 126 {
                out.push(bytes.len() as u8);
            } else {
                out.push(126);
                out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
            }
            out.extend_from_slice(bytes);
            out
        }

        fn run(dir: &Path, port: u16, args: &[&str]) -> Output {
            let mut argv = vec!["--cdp".to_string(), format!("http://127.0.0.1:{port}")];
            argv.extend(args.iter().copied().map(str::to_string));
            crate::execute_in(&argv, None, None, Some(dir))
        }

        fn actions(log: &[Value]) -> Vec<&Value> {
            log.iter()
                .filter(|message| {
                    message.get("method").and_then(|item| item.as_str()) == Some("Slicc.action")
                })
                .collect()
        }

        fn assert_agent(params: &Value) {
            match crate::cdp::slicc_agent() {
                Some(agent) => assert_eq!(params["agent"], agent),
                None => assert!(params.get("agent").is_none()),
            }
        }

        fn assert_pair(left: &Output, right: &Output, started: Instant) {
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "notification changed timing: {:?}",
                started.elapsed()
            );
            assert_eq!(left.stdout, right.stdout);
            assert_eq!(left.stderr, right.stderr);
            assert_eq!(left.code, right.code);
            assert!(!left.stderr.contains("wasn't found"));
            assert!(!right.stdout.contains("Slicc.action"));
        }

        fn method_names(log: &[Value]) -> Vec<&str> {
            log.iter()
                .filter_map(|message| message.get("method").and_then(|item| item.as_str()))
                .collect()
        }

        #[test]
        fn a_missing_action_reply_does_not_change_open() {
            let root =
                std::env::temp_dir().join(format!("slicc-action-open-{}", std::process::id()));
            let missing_dir = root.join("missing");
            let silent_dir = root.join("silent");
            std::fs::create_dir_all(&missing_dir).unwrap();
            std::fs::create_dir_all(&silent_dir).unwrap();
            let missing = Peer::start(SliccReply::NotFound, true);
            let silent = Peer::start(SliccReply::Silent, true);
            let started = Instant::now();
            let left = {
                run(&missing_dir, missing.port, &["open", "http://a.test/"])
            };
            let right = {
                run(&silent_dir, silent.port, &["open", "http://a.test/"])
            };
            assert_pair(&left, &right, started);
            assert_eq!(left.code, 0);
            assert_eq!(left.stderr, "");
            assert_eq!(
                left.stdout,
                "Opened http://a.test/ in new tab [targetId: TAB1]\n"
            );
            missing.wait_actions(2);
            silent.wait_actions(2);
            let missing_log = missing.log.lock().unwrap().clone();
            let silent_log = silent.log.lock().unwrap().clone();
            for log in [missing_log, silent_log] {
                let names = method_names(&log);
                let start = names
                    .iter()
                    .position(|name| *name == "Slicc.action")
                    .unwrap();
                let create = names
                    .iter()
                    .position(|name| *name == "Target.createTarget")
                    .unwrap();
                let end = names
                    .iter()
                    .rposition(|name| *name == "Slicc.action")
                    .unwrap();
                assert!(start < create && create < end);
                let noted = actions(&log);
                assert_eq!(noted.len(), 2);
                assert!(noted
                    .iter()
                    .all(|message| message.get("sessionId").is_none()));
                assert_ne!(noted[0]["id"], noted[1]["id"]);
                assert_eq!(noted[0]["params"]["phase"], "start");
                assert_eq!(noted[0]["params"]["kind"], "open");
                assert_eq!(noted[0]["params"]["url"], "http://a.test/");
                assert!(noted[0]["params"].get("tab").is_none());
                assert_agent(&noted[0]["params"]);
                assert_eq!(noted[1]["params"]["phase"], "end");
                assert_eq!(noted[1]["params"]["ok"], true);
                assert_eq!(noted[1]["params"]["tab"], "TAB1");
                assert!(noted[1]["params"].get("error").is_none());
                assert!(noted[1]["params"].get("kind").is_none());
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn tab_list_sends_nothing_and_a_failed_click_keeps_its_stderr() {
            let root =
                std::env::temp_dir().join(format!("slicc-action-fail-{}", std::process::id()));
            let missing_dir = root.join("missing");
            let silent_dir = root.join("silent");
            std::fs::create_dir_all(&missing_dir).unwrap();
            std::fs::create_dir_all(&silent_dir).unwrap();
            let listed = Peer::start(SliccReply::NotFound, true);
            let listed_out = {
                run(&missing_dir, listed.port, &["tab-list"])
            };
            assert_eq!(listed_out.code, 0, "{}", listed_out.stderr);
            assert!(listed_out.stdout.contains("[TAB1]"));
            thread::sleep(Duration::from_millis(50));
            let listed_log = listed.log.lock().unwrap().clone();
            assert!(actions(&listed_log).is_empty());
            let missing = Peer::start(SliccReply::NotFound, false);
            let silent = Peer::start(SliccReply::Silent, false);
            let started = Instant::now();
            let left = {
                run(&missing_dir, missing.port, &["click", "e1"])
            };
            let right = {
                run(&silent_dir, silent.port, &["click", "e1"])
            };
            assert_pair(&left, &right, started);
            let expected = "Error: --tab <targetId> is required. Run 'playwright-cli tab-list' to get tab IDs.\n";
            assert_eq!(left.stderr, expected);
            assert_eq!(left.stdout, "");
            assert_eq!(left.code, 1);
            missing.wait_actions(2);
            silent.wait_actions(2);
            let missing_log = missing.log.lock().unwrap().clone();
            let silent_log = silent.log.lock().unwrap().clone();
            for log in [missing_log, silent_log] {
                let noted = actions(&log);
                assert_eq!(
                    noted.len(),
                    2,
                    "{}",
                    serde_json::to_string(&log).unwrap_or_default()
                );
                assert_eq!(noted[0]["params"]["kind"], "click");
                assert!(noted[0]["params"].get("tab").is_none());
                assert!(noted[0]["params"].get("target").is_none());
                assert_eq!(noted[1]["params"]["ok"], false);
                assert_eq!(noted[1]["params"]["error"], "unknown tab");
                assert!(!noted[1].to_string().contains("targetId"));
                let names = method_names(&log);
                let listed = names
                    .iter()
                    .position(|name| *name == "Target.getTargets")
                    .unwrap();
                let start = names
                    .iter()
                    .position(|name| *name == "Slicc.action")
                    .unwrap();
                assert!(listed < start);
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        fn seed(dir: &Path, label: Option<&str>) {
            let label = match label {
                Some(label) => format!(",\"label\":{}", serde_json::to_string(label).unwrap()),
                None => String::new(),
            };
            let body = format!(
                r#"{{"current":"TAB1","snapshots":{{"TAB1":{{"e1":{{"backendNodeId":7,"selector":"input","frameId":null{label}}}}}}}}}"#
            );
            let session = crate::session::storage_path(dir).unwrap();
            if let Some(parent) = session.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(session, body).unwrap();
        }

        #[test]
        fn fill_reports_length_and_label_without_the_text() {
            let root =
                std::env::temp_dir().join(format!("slicc-action-fill-{}", std::process::id()));
            let missing_dir = root.join("missing");
            let silent_dir = root.join("silent");
            let old_dir = root.join("old");
            std::fs::create_dir_all(&missing_dir).unwrap();
            std::fs::create_dir_all(&silent_dir).unwrap();
            std::fs::create_dir_all(&old_dir).unwrap();
            seed(&missing_dir, Some("textbox \"Email\""));
            seed(&silent_dir, Some("textbox \"Email\""));
            seed(&old_dir, None);
            let missing = Peer::start(SliccReply::NotFound, true);
            let silent = Peer::start(SliccReply::Silent, true);
            let started = Instant::now();
            let left = {
                run(&missing_dir, missing.port, &["fill", "e1", "secret-text"])
            };
            let right = {
                run(&silent_dir, silent.port, &["fill", "e1", "secret-text"])
            };
            assert_pair(&left, &right, started);
            assert_eq!(left.code, 0, "{}", left.stderr);
            assert_eq!(left.stdout, "Filled e1 with: secret-text\n");
            missing.wait_actions(2);
            silent.wait_actions(2);
            let missing_log = missing.log.lock().unwrap().clone();
            let silent_log = silent.log.lock().unwrap().clone();
            for log in [missing_log, silent_log] {
                let noted = actions(&log);
                assert_eq!(noted.len(), 2);
                let start = &noted[0]["params"];
                assert_eq!(start["kind"], "fill");
                assert_eq!(start["tab"], "TAB1");
                assert_eq!(start["target"], "textbox \"Email\"");
                assert_eq!(start["length"], 11);
                assert!(!start.to_string().contains("secret-text"));
                assert!(start.get("value").is_none());
                assert_agent(start);
                assert_eq!(noted[1]["params"]["ok"], true);
                assert_eq!(noted[1]["params"]["tab"], "TAB1");
                let names = method_names(&log);
                let start_at = names
                    .iter()
                    .position(|name| *name == "Slicc.action")
                    .unwrap();
                let insert = names
                    .iter()
                    .position(|name| *name == "Input.insertText")
                    .unwrap();
                let end_at = names
                    .iter()
                    .rposition(|name| *name == "Slicc.action")
                    .unwrap();
                assert!(start_at < insert && insert < end_at);
                let inserted = log
                    .iter()
                    .find(|message| message["method"] == "Input.insertText")
                    .unwrap();
                assert_eq!(inserted["params"]["text"], "secret-text");
            }
            let old = Peer::start(SliccReply::NotFound, true);
            let old_out = {
                run(&old_dir, old.port, &["fill", "e1", "secret-text"])
            };
            assert_eq!(old_out.stdout, left.stdout);
            assert_eq!(old_out.code, 0, "{}", old_out.stderr);
            old.wait_actions(2);
            let old_log = old.log.lock().unwrap().clone();
            let noted = actions(&old_log);
            assert!(noted[0]["params"].get("target").is_none());
            assert!(!noted[0]["params"].to_string().contains("secret-text"));
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn a_takeover_prints_the_close_frame_not_a_guard_refusal() {
            let root =
                std::env::temp_dir().join(format!("slicc-action-takeover-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let expected = format!("websocket closed 1011 {TAKEOVER}\n");
            let held = Peer::start(SliccReply::NotFound, true);
            let output = run(&root, held.port, &["eval", "new Promise(() => { TAKEOVER })"]);
            assert_eq!(output.code, 1);
            assert_eq!(output.stderr, expected);
            let guarding = Peer::start(SliccReply::Takeover, true);
            let output = run(&root, guarding.port, &["eval", "1"]);
            assert_eq!(output.code, 1);
            assert_eq!(output.stderr, expected);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn eval_does_not_send_the_expression() {
            let root =
                std::env::temp_dir().join(format!("slicc-action-eval-{}", std::process::id()));
            let missing_dir = root.join("missing");
            let silent_dir = root.join("silent");
            std::fs::create_dir_all(&missing_dir).unwrap();
            std::fs::create_dir_all(&silent_dir).unwrap();
            let missing = Peer::start(SliccReply::NotFound, true);
            let silent = Peer::start(SliccReply::Silent, true);
            let started = Instant::now();
            let left = {
                run(&missing_dir, missing.port, &["eval", "secret-source"])
            };
            let right = {
                run(&silent_dir, silent.port, &["eval", "secret-source"])
            };
            assert_pair(&left, &right, started);
            assert_eq!(left.code, 0, "{}", left.stderr);
            assert_eq!(left.stdout, "ok\n");
            missing.wait_actions(2);
            silent.wait_actions(2);
            let missing_log = missing.log.lock().unwrap().clone();
            let silent_log = silent.log.lock().unwrap().clone();
            for log in [missing_log, silent_log] {
                let noted = actions(&log);
                assert_eq!(noted.len(), 2);
                assert_eq!(noted[0]["params"]["kind"], "eval");
                assert_eq!(noted[0]["params"]["tab"], "TAB1");
                assert!(!noted[0].to_string().contains("secret-source"));
                assert!(noted[0]["params"].get("expression").is_none());
                assert_eq!(noted[1]["params"]["ok"], true);
                let names = method_names(&log);
                let start = names
                    .iter()
                    .position(|name| *name == "Slicc.action")
                    .unwrap();
                let eval = names
                    .iter()
                    .position(|name| *name == "Runtime.evaluate")
                    .unwrap();
                assert!(start < eval);
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn annotation_frames_omit_query_fragment_userinfo_and_eval_text() {
            let root = std::env::temp_dir().join(format!(
                "slicc-action-secrets-{}",
                std::process::id()
            ));
            let secret = "http://user:pass@a.test/form?q=QUERYSECRET&tok=x#FRAGSECRET";
            let open_dir = root.join("open");
            let goto_dir = root.join("goto");
            let eval_dir = root.join("eval");
            std::fs::create_dir_all(&open_dir).unwrap();
            std::fs::create_dir_all(&goto_dir).unwrap();
            std::fs::create_dir_all(&eval_dir).unwrap();
            let opened_peer = Peer::start(SliccReply::Silent, false);
            let opened = {
                run(&open_dir, opened_peer.port, &["open", secret])
            };
            assert_eq!(opened.code, 0, "{}", opened.stderr);
            assert!(!opened.stdout.contains("QUERYSECRET"));
            assert!(!opened.stdout.contains("FRAGSECRET"));
            assert!(opened.stdout.contains("q="));
            opened_peer.wait_actions(2);
            let opened_log = opened_peer.log.lock().unwrap().clone();
            let opened_actions = actions(&opened_log);
            assert_eq!(opened_actions[0]["params"]["url"], "http://a.test/form");
            assert_frames_clean(&opened_actions);
            let went_peer = Peer::start(SliccReply::Silent, true);
            let went = {
                run(&goto_dir, went_peer.port, &["goto", secret])
            };
            assert_eq!(went.code, 0, "{}", went.stderr);
            assert!(!went.stdout.contains("QUERYSECRET"));
            went_peer.wait_actions(2);
            let went_log = went_peer.log.lock().unwrap().clone();
            let went_actions = actions(&went_log);
            assert_eq!(went_actions[0]["params"]["kind"], "goto");
            assert_eq!(went_actions[0]["params"]["url"], "http://a.test/form");
            assert_frames_clean(&went_actions);
            let failed_peer = Peer::start(SliccReply::Silent, true);
            let failed = {
                run(&eval_dir, 
                    failed_peer.port,
                    &["eval", "throw new Error('EVALSECRET-in-error')"],
                )
            };
            assert_ne!(failed.code, 0);
            assert!(failed.stderr.contains("EVALSECRET-in-error"));
            failed_peer.wait_actions(2);
            let failed_log = failed_peer.log.lock().unwrap().clone();
            let failed_actions = actions(&failed_log);
            assert_eq!(failed_actions[0]["params"]["kind"], "eval");
            assert_eq!(failed_actions[1]["params"]["ok"], false);
            assert!(failed_actions[1]["params"].get("error").is_none());
            assert_frames_clean(&failed_actions);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn opaque_urls_keep_only_the_scheme_on_the_action() {
            let root = std::env::temp_dir().join(format!(
                "slicc-action-opaque-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let cases = [
                (
                    "open",
                    "data:text/html,<title>DATASECRET</title>",
                    Some("data:"),
                    "DATASECRET",
                ),
                (
                    "goto",
                    "data:text/html,DATASECRET2",
                    Some("data:"),
                    "DATASECRET2",
                ),
                (
                    "open",
                    "javascript:void('JSSECRET')",
                    Some("javascript:"),
                    "JSSECRET",
                ),
                (
                    "goto",
                    "blob:https://example.com/BLOBSECRET",
                    Some("blob:"),
                    "BLOBSECRET",
                ),
                ("open", "about:blank", Some("about:blank"), ""),
                ("open", "", None, ""),
                ("open", "null", None, ""),
                ("goto", "://", None, ""),
                (
                    "goto",
                    "ABOUT:blank?q=BLANKSECRET",
                    Some("about:blank"),
                    "BLANKSECRET",
                ),
                (
                    "goto",
                    "about:blank#ABOUTSECRET",
                    Some("about:blank"),
                    "ABOUTSECRET",
                ),
                ("goto", "HOSTSECRET3:8080/path", None, "HOSTSECRET3"),
                (
                    "open",
                    "USERSECRET5:PASSSECRET5@127.0.0.1/form",
                    None,
                    "USERSECRET5",
                ),
                ("goto", "foo:bar", None, "foo:"),
            ];
            let peer = Peer::start(SliccReply::Silent, true);
            let mut seen = 0usize;
            for (command, url, expect, secret) in cases {
                let output = {
                    run(&root, peer.port, &[command, url])
                };
                assert_eq!(output.code, 0, "{command} {url} {}", output.stderr);
                if !secret.is_empty() {
                    let lower_stdout = output.stdout.to_ascii_lowercase();
                    let lower_secret = secret.to_ascii_lowercase();
                    if !url.contains('?') && !url.contains('#') {
                        assert!(lower_stdout.contains(&lower_secret), "{command} {url}");
                    }
                }
                seen += 2;
                peer.wait_actions(seen);
                let log = peer.log.lock().unwrap().clone();
                let noted = actions(&log);
                assert_eq!(noted.len(), seen, "{command} {url}");
                let start = &noted[seen - 2]["params"];
                match expect {
                    Some(sent) => assert_eq!(start["url"], sent, "{url}"),
                    None => assert!(start.get("url").is_none(), "{url} {start}"),
                }
                if !secret.is_empty() {
                    assert!(!start.to_string().contains(secret), "{url} {start}");
                }
                assert!(!start.to_string().contains("PASSSECRET5"), "{url} {start}");
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn malformed_absolute_destinations_still_use_the_catalog_refusal() {
            let root = std::env::temp_dir().join(format!("slicc-action-malformed-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let peer = Peer::start(SliccReply::Silent, true);
            for command in ["open", "tab-new", "goto", "navigate"] {
                for url in ["http://exa%20mple.sliccy.ai/", "http://xn--1ug.sliccy.ai/", "http://xn--a.sliccy.ai/", "view-source:http://exa%20mple.sliccy.ai/"] {
                    let output = run(&root, peer.port, &[command, url]);
                    assert_eq!(output.code, 1, "{command} {url}");
                    assert_eq!(output.stderr, format!("{}\n", slicc_browsers::catalog().protected_origins.unparseable_refusal), "{command} {url}");
                    assert!(!output.stderr.contains("add https://"));
                }
            }
            let _ = std::fs::remove_dir_all(&root);
        }

        fn assert_frames_clean(noted: &[&Value]) {
            let rendered = noted
                .iter()
                .map(|message| message.to_string())
                .collect::<String>();
            for secret in ["QUERYSECRET", "FRAGSECRET", "user:pass", "tok=x", "EVALSECRET"] {
                assert!(!rendered.contains(secret), "{rendered}");
            }
        }

        #[test]
        fn upload_rejects_directory() {
            let root = std::env::temp_dir().join(format!("slicc-upload-dir-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            seed(&root, None);
            let peer = Peer::start(SliccReply::Silent, true);
            let dir_path = std::env::temp_dir().join(format!("slicc-upload-target-dir-{}", std::process::id()));
            std::fs::create_dir_all(&dir_path).unwrap();
            let output = {
                run(&root, peer.port, &["upload", "e1", dir_path.to_str().unwrap()])
            };
            assert_ne!(output.code, 0);
            assert!(output.stderr.contains("directory"), "{}", output.stderr);
            let _ = std::fs::remove_dir_all(&root);
            let _ = std::fs::remove_dir_all(&dir_path);
        }

        #[cfg(unix)]
        #[test]
        fn upload_rejects_unreadable_file() {
            use std::os::unix::fs::PermissionsExt;
            let root = std::env::temp_dir().join(format!("slicc-upload-unread-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            seed(&root, None);
            let peer = Peer::start(SliccReply::Silent, true);
            let file_path = std::env::temp_dir().join(format!("slicc-upload-unreadable-{}", std::process::id()));
            std::fs::write(&file_path, b"secret").unwrap();
            std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o000)).unwrap();
            let output = {
                run(&root, peer.port, &["upload", "e1", file_path.to_str().unwrap()])
            };
            std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let _ = std::fs::remove_file(&file_path);
            assert_ne!(output.code, 0);
            assert!(output.stderr.contains("cannot read"), "{}", output.stderr);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn upload_rejects_missing_file() {
            let root = std::env::temp_dir().join(format!("slicc-upload-miss-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            seed(&root, None);
            let peer = Peer::start(SliccReply::Silent, true);
            let output = {
                run(&root, peer.port, &["upload", "e1", "/tmp/nonexistent-slicc-upload-xyz"])
            };
            assert_ne!(output.code, 0);
            assert!(output.stderr.contains("not found"), "{}", output.stderr);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn upload_succeeds_with_valid_file() {
            let root = std::env::temp_dir().join(format!("slicc-upload-ok-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            seed(&root, None);
            let peer = Peer::start(SliccReply::Silent, true);
            let file_path = std::env::temp_dir().join(format!("slicc-upload-valid-{}.txt", std::process::id()));
            std::fs::write(&file_path, b"hello world").unwrap();
            let output = {
                run(&root, peer.port, &["upload", "e1", file_path.to_str().unwrap()])
            };
            assert_eq!(output.code, 0, "{}", output.stderr);
            assert!(output.stdout.contains("Uploaded 1 file(s)"), "{}", output.stdout);
            let log = peer.log.lock().unwrap().clone();
            let set_file = log.iter().find(|call| {
                call.get("method").and_then(|v| v.as_str()) == Some("DOM.setFileInputFiles")
            });
            assert!(set_file.is_some(), "expected DOM.setFileInputFiles call");
            let _ = std::fs::remove_file(&file_path);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn navigate_works_like_goto() {
            let root = std::env::temp_dir().join(format!("slicc-navigate-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let peer = Peer::start(SliccReply::Silent, true);
            let output = {
                run(&root, peer.port, &["navigate", "http://example.com/test"])
            };
            assert_eq!(output.code, 0, "{}", output.stderr);
            assert!(output.stdout.contains("Navigated to"), "{}", output.stdout);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn navigate_requires_url() {
            let root = std::env::temp_dir().join(format!("slicc-nav-nourl-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let peer = Peer::start(SliccReply::Silent, true);
            let output = {
                run(&root, peer.port, &["navigate"])
            };
            assert_eq!(output.code, 1);
            assert!(output.stderr.contains("navigate requires a URL"), "{}", output.stderr);
            let _ = std::fs::remove_dir_all(&root);
        }

        #[test]
        fn navigate_refuses_teleport_flags() {
            let root = std::env::temp_dir().join(format!("slicc-nav-teleport-{}", std::process::id()));
            std::fs::create_dir_all(&root).unwrap();
            let peer = Peer::start(SliccReply::Silent, true);
            for flag in ["--teleport-start=x", "--teleport-return=x", "--teleport-runtime=x"] {
                let output = {
                    run(&root, peer.port, &["navigate", flag, "http://example.com/"])
                };
                assert_eq!(output.code, 1, "{flag}");
                assert!(output.stderr.contains("not implemented yet"), "{flag}: {}", output.stderr);
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}
