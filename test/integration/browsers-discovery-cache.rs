#![cfg(all(not(target_arch = "wasm32"), not(target_os = "wasi")))]

use cdp_client::discover::{
    attached_with, read_browsers_cache, version_url, BrowsersCache, CLI_MAX_STDOUT, CLI_TIMEOUT,
};
use serde_json::json;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn scratch() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "slicc-cdp-browsers-cache-int-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn fake_slicc(dir: &Path, script: &str) -> PathBuf {
    let path = dir.join("slicc");
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    path
}

fn serve_version(port_path: &str) -> (u16, std::thread::JoinHandle<()>, std::sync::mpsc::Sender<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = format!(r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}{port_path}"}}"#);
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let join = std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        loop {
            if stop_rx.try_recv().is_ok() {
                break;
            }
            match listener.accept() {
                Ok((mut sock, _)) => {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf);
                    let _ = sock.write_all(response.as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });
    (port, join, stop_tx)
}

struct EnvGuard {
    home: Option<std::ffi::OsString>,
    xdg: Option<std::ffi::OsString>,
}

impl EnvGuard {
    fn isolate(home: &Path) -> Self {
        let guard = Self {
            home: std::env::var_os("HOME"),
            xdg: std::env::var_os("XDG_CONFIG_HOME"),
        };
        std::env::set_var("HOME", home);
        std::env::remove_var("XDG_CONFIG_HOME");
        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match &self.xdg {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}

#[test]
fn two_commands_within_ttl_spawn_slicc_once() {
    let root = scratch();
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _env = EnvGuard::isolate(&home);
    let cache = root.join("browsers-discovery.json");
    let count = root.join("count");
    let (port, join, stop) = serve_version("/devtools/browser/live");
    let doc = json!({
        "schema": 1,
        "entries": [{
            "id": format!("cdp:127.0.0.1:{port}"),
            "kind": "endpoint",
            "tier": 1,
            "endpoint": {
                "cdp": format!("http://127.0.0.1:{port}"),
                "webSocketDebuggerUrl": format!("ws://127.0.0.1:{port}/devtools/browser/{port}"),
                "sevenOpen": true,
            }
        }]
    })
    .to_string()
    .replace('\'', "");
    let slicc = fake_slicc(
        &root,
        &format!(
            "#!/bin/sh\nprintf . >> '{}' || true\nprintf '%s' '{doc}'\n",
            count.display()
        ),
    );
    let mut probe = |url: &str| {
        let response = cdp_client::net::http_get(url)?;
        Ok::<String, String>(String::from_utf8_lossy(&response.body).into_owned())
    };
    let first =
        attached_with(Some(&slicc), Some(&cache), CLI_TIMEOUT, CLI_MAX_STDOUT, &mut probe).unwrap();
    let second =
        attached_with(Some(&slicc), Some(&cache), CLI_TIMEOUT, CLI_MAX_STDOUT, &mut probe).unwrap();
    assert_eq!(first.as_deref(), Some(version_url(port).as_str()));
    assert_eq!(second, first);
    assert_eq!(std::fs::read_to_string(&count).unwrap().len(), 1);
    let _ = stop.send(());
    let _ = join.join();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn hanging_slicc_costs_timeout_once_across_commands() {
    let root = scratch();
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _env = EnvGuard::isolate(&home);
    let cache = root.join("browsers-discovery.json");
    let count = root.join("count");
    let slicc = fake_slicc(
        &root,
        &format!(
            "#!/bin/sh\nprintf . >> '{}' || true\n/bin/sleep 5\n",
            count.display()
        ),
    );
    let timeout = Duration::from_millis(200);
    let started = std::time::Instant::now();
    let first =
        attached_with(Some(&slicc), Some(&cache), timeout, CLI_MAX_STDOUT, &mut |_| {
            Err("unused\n".to_string())
        })
        .unwrap();
    let first_elapsed = started.elapsed();
    assert_eq!(first, None);
    assert!(first_elapsed >= timeout);
    assert!(first_elapsed < Duration::from_millis(800));
    assert_eq!(
        read_browsers_cache(&cache, std::time::SystemTime::now()),
        BrowsersCache::Negative
    );
    let started = std::time::Instant::now();
    let second =
        attached_with(Some(&slicc), Some(&cache), timeout, CLI_MAX_STDOUT, &mut |_| {
            Err("unused\n".to_string())
        })
        .unwrap();
    assert_eq!(second, None);
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "second command still paid for slicc: {:?}",
        started.elapsed()
    );
    assert_eq!(std::fs::read_to_string(&count).unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}
