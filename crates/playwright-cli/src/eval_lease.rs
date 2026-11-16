use crate::args::{Invocation, Output};
use crate::har_lease::{inspect, Lease};
use crate::session;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) struct AttachmentLease {
    pub(crate) id: String,
    record: PathBuf,
    stop: PathBuf,
    _lease: Lease,
}

fn paths(invocation: &Invocation) -> Result<(PathBuf, PathBuf), String> {
    let base = session::path_named(invocation.session.as_deref().filter(|name| !name.is_empty()), invocation.cwd.as_deref())
        .ok_or_else(|| "session: user config dir is not absolute\n".to_string())?;
    Ok((base.with_extension("eval.json"), base.with_extension("eval.stop")))
}

fn read(path: &PathBuf) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

fn completion(record: &std::path::Path, id: &str) -> PathBuf {
    record.with_extension(format!("{}.done", cdp_client::hash::sha256_hex(id.as_bytes())))
}

impl AttachmentLease {
    pub(crate) fn claim(invocation: &Invocation) -> Result<Self, String> {
        let (record, stop) = paths(invocation)?;
        let lease = Lease::claim(&record, false).map_err(|message| if message.contains("already in progress") { "eval-attach: an attachment is already held in this session\n".to_string() } else { message.replace("har:", "eval-attach:") })?;
        let seed = format!("{:?}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default());
        let id = format!("eval-{}", &cdp_client::hash::sha256_hex(seed.as_bytes())[..24]);
        session::write_private(&record, &json!({"attachmentId":id,"state":"attached"}).to_string())?;
        Ok(Self { id, record, stop, _lease: lease })
    }

    pub(crate) fn stopping(&self) -> bool {
        fs::read_to_string(&self.stop).is_ok_and(|id| id == self.id)
    }

    pub(crate) fn finish(&self, reason: &str) -> Result<Value, String> {
        let event = json!({"event":"stopped","attachmentId":self.id,"reason":reason});
        if reason == "detach" { session::write_private(&completion(&self.record, &self.id), &event.to_string())?; }
        else { let _ = fs::remove_file(&self.record); }
        if self.stopping() { let _ = fs::remove_file(&self.stop); }
        Ok(event)
    }
}

pub(crate) fn detach(invocation: &Invocation) -> Output {
    let (record, stop) = match paths(invocation) { Ok(paths) => paths, Err(message) => return Output::err(message) };
    let Some(found) = read(&record) else { return Output::err("eval-detach: no attachment in this session\n") };
    let Some(id) = found["attachmentId"].as_str() else { return Output::err("eval-detach: invalid attachment lease\n") };
    if invocation.positionals.first().is_some_and(|expected| expected != id) { return Output::err("eval-detach: attachmentId does not belong to this session\n") }
    if let Err(message) = session::write_private(&stop, id) { return Output::err(message) }
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(done) = read(&completion(&record, id)).filter(|value| value["attachmentId"] == id && value["event"] == "stopped") {
            let _ = fs::remove_file(completion(&record, id));
            if read(&record).is_some_and(|current| current["attachmentId"] == id) { let _ = fs::remove_file(&record); }
            let _ = fs::remove_file(&stop);
            return Output::ok(format!("{done}\n"));
        }
        if !inspect(&record).alive { return Output::err("eval-detach: attachment holder is no longer running\n") }
        if Instant::now() >= deadline { return Output::err("eval-detach: holder did not answer\n") }
        std::thread::sleep(Duration::from_millis(50));
    }
}
