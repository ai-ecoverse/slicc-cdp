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
            "goto" => "goto",
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
        cdp.notify("Slicc.action", self.note.start_params(), None);
    }

    pub fn finish(&mut self, cdp: &mut Cdp, output: &Output) {
        if !self.active || self.ended {
            return;
        }
        if !self.started {
            self.begin(cdp);
        }
        self.ended = true;
        cdp.notify(
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
    }

    #[cfg(not(target_os = "wasi"))]
    mod peer {
        use super::*;
        use serde_json::{json, Value};
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};
        use std::thread::{self, JoinHandle};
        use std::time::{Duration, Instant};

        #[derive(Clone, Copy)]
        enum SliccReply {
            NotFound,
            Silent,
        }

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
                    if let Some(response) = on_message(&text, mode, pages, log) {
                        let _ = stream.write_all(&server_text(&response));
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
        ) -> Option<String> {
            let value: Value = serde_json::from_str(text).ok()?;
            let id = value.get("id").cloned()?;
            log.lock().unwrap().push(value.clone());
            let method = value
                .get("method")
                .and_then(|item| item.as_str())
                .unwrap_or("");
            if method == "Slicc.action" {
                return match mode {
                    SliccReply::Silent => None,
                    SliccReply::NotFound => Some(
                        json!({"id": id, "error": {"message": "'Slicc.action' wasn't found"}})
                            .to_string(),
                    ),
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
                "Target.attachToTarget" => json!({ "sessionId": "sess" }),
                "DOM.resolveNode" => json!({ "object": { "objectId": "obj-1" } }),
                "Runtime.evaluate" => {
                    let expr = params
                        .get("expression")
                        .and_then(|item| item.as_str())
                        .unwrap_or("");
                    if expr == "document.readyState" {
                        json!({ "result": { "type": "string", "value": "complete" } })
                    } else {
                        json!({ "result": { "type": "string", "value": "ok" } })
                    }
                }
                _ => json!({}),
            };
            Some(json!({"id": id, "result": result}).to_string())
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

        struct DirGuard {
            prev: PathBuf,
        }

        impl DirGuard {
            fn enter(dir: &Path) -> Self {
                let prev = std::env::current_dir().unwrap();
                std::env::set_current_dir(dir).unwrap();
                Self { prev }
            }
        }

        impl Drop for DirGuard {
            fn drop(&mut self) {
                let _ = std::env::set_current_dir(&self.prev);
            }
        }

        static CWD: Mutex<()> = Mutex::new(());

        fn run(port: u16, args: &[&str]) -> Output {
            let mut argv = vec!["--cdp".to_string(), format!("http://127.0.0.1:{port}")];
            argv.extend(args.iter().copied().map(str::to_string));
            crate::execute(&argv, None)
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
            let _lock = CWD.lock().unwrap();
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
                let _dir = DirGuard::enter(&missing_dir);
                run(missing.port, &["open", "http://a.test/"])
            };
            let right = {
                let _dir = DirGuard::enter(&silent_dir);
                run(silent.port, &["open", "http://a.test/"])
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
            let _lock = CWD.lock().unwrap();
            let root =
                std::env::temp_dir().join(format!("slicc-action-fail-{}", std::process::id()));
            let missing_dir = root.join("missing");
            let silent_dir = root.join("silent");
            std::fs::create_dir_all(&missing_dir).unwrap();
            std::fs::create_dir_all(&silent_dir).unwrap();
            let listed = Peer::start(SliccReply::NotFound, true);
            let listed_out = {
                let _dir = DirGuard::enter(&missing_dir);
                run(listed.port, &["tab-list"])
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
                let _dir = DirGuard::enter(&missing_dir);
                run(missing.port, &["click", "e1"])
            };
            let right = {
                let _dir = DirGuard::enter(&silent_dir);
                run(silent.port, &["click", "e1"])
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
                assert_eq!(noted[1]["params"]["error"], expected.trim_end());
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
            let session = dir.join(".playwright-cli");
            std::fs::create_dir_all(&session).unwrap();
            std::fs::write(session.join("session.json"), body).unwrap();
        }

        #[test]
        fn fill_reports_length_and_label_without_the_text() {
            let _lock = CWD.lock().unwrap();
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
                let _dir = DirGuard::enter(&missing_dir);
                run(missing.port, &["fill", "e1", "secret-text"])
            };
            let right = {
                let _dir = DirGuard::enter(&silent_dir);
                run(silent.port, &["fill", "e1", "secret-text"])
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
                let _dir = DirGuard::enter(&old_dir);
                run(old.port, &["fill", "e1", "secret-text"])
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
        fn eval_does_not_send_the_expression() {
            let _lock = CWD.lock().unwrap();
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
                let _dir = DirGuard::enter(&missing_dir);
                run(missing.port, &["eval", "secret-source"])
            };
            let right = {
                let _dir = DirGuard::enter(&silent_dir);
                run(silent.port, &["eval", "secret-source"])
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
    }
}
