use crate::session;
#[cfg(target_arch = "wasm32")]
use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const ALREADY: &str = "har-start: a HAR recording is already in progress\n";

pub(crate) struct OwnerCheck {
    pub(crate) alive: bool,
    #[cfg(not(target_arch = "wasm32"))]
    _lock: Option<fs::File>,
}

pub(crate) struct Lease {
    path: PathBuf,
    owner: String,
    #[cfg(not(target_arch = "wasm32"))]
    lock: fs::File,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn inspect(record: &Path) -> OwnerCheck {
    match fs::OpenOptions::new().read(true).write(true).open(record.with_extension("lock").join("owner")) {
        Ok(file) => match file.try_lock() {
            Ok(()) => OwnerCheck { alive: false, _lock: Some(file) },
            Err(_) => OwnerCheck { alive: true, _lock: None },
        },
        Err(error) => OwnerCheck { alive: error.kind() != io::ErrorKind::NotFound, _lock: None },
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
fn stat_identity(stat: &str, proc: &str, pid: Option<u32>) -> Option<String> {
    let actual: u32 = stat.split_whitespace().next()?.parse().ok()?;
    if actual == 0 || pid.is_some_and(|pid| pid != actual) { return None }
    let fields: Vec<_> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    if matches!(fields.first().copied(), Some("Z" | "X")) { return None }
    let start: u64 = fields.get(19)?.parse().ok()?;
    let boot: u64 = proc.lines().find_map(|line| line.strip_prefix("btime "))?.trim().parse().ok()?;
    Some(format!("{actual}:{start}:{boot}"))
}

#[cfg(target_arch = "wasm32")]
fn identity(pid: Option<u32>) -> Option<String> {
    let path = pid.map(|pid| format!("/proc/{pid}/stat")).unwrap_or_else(|| "/proc/self/stat".into());
    stat_identity(&fs::read_to_string(path).ok()?, &fs::read_to_string("/proc/stat").ok()?, pid)
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn inspect(record: &Path) -> OwnerCheck {
    let alive = (|| {
        let owner: Value = serde_json::from_str(&fs::read_to_string(record.with_extension("lock").join("owner")).ok()?).ok()?;
        let expected = owner["process"].as_str()?;
        let pid: u32 = expected.split(':').next()?.parse().ok()?;
        if identity(None)?.split(':').next()? == pid.to_string() { return Some(false) }
        Some(identity(Some(pid)).as_deref() == Some(expected))
    })().unwrap_or(false);
    OwnerCheck { alive }
}

impl Lease {
    pub(crate) fn claim(record: &Path, recent: bool) -> Result<Self, String> {
        let path = record.with_extension("lock");
        let parent = path.parent().ok_or_else(|| ALREADY.to_string())?;
        fs::create_dir_all(parent).map_err(|err| format!("har: {err}\n"))?;
        let stale = fs::metadata(&path).and_then(|metadata| metadata.modified()).ok()
            .and_then(|modified| modified.elapsed().ok()).is_some_and(|age| age >= Duration::from_secs(5));
        let checked = inspect(record);
        if stale && !recent && !checked.alive {
            #[cfg(windows)]
            drop(checked);
            fs::remove_dir_all(&path).map_err(|err| format!("har: {err}\n"))?;
        }
        match fs::create_dir(&path) {
            Ok(()) => {
                let token = cdp_client::hash::sha256_hex(format!("{:?}", SystemTime::now()).as_bytes());
                #[cfg(not(target_arch = "wasm32"))]
                let owner = token;
                #[cfg(target_arch = "wasm32")]
                let owner = serde_json::json!({"token":token,"process":identity(None)}).to_string();
                if let Err(message) = session::write_private(&path.join("owner"), &owner) {
                    let _ = fs::remove_dir_all(&path);
                    return Err(message);
                }
                #[cfg(not(target_arch = "wasm32"))]
                let opened = fs::OpenOptions::new().read(true).write(true).open(path.join("owner"))
                    .and_then(|file| file.try_lock().map(|()| file).map_err(|error| io::Error::other(error.to_string())));
                #[cfg(not(target_arch = "wasm32"))]
                let lock = match opened {
                    Ok(file) => file,
                    Err(error) => {
                        if fs::read_to_string(path.join("owner")).is_ok_and(|current| current == owner) { let _ = fs::remove_dir_all(&path); }
                        return Err(format!("har: {error}\n"));
                    }
                };
                Ok(Self { path, owner, #[cfg(not(target_arch = "wasm32"))] lock })
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(ALREADY.into()),
            Err(err) => Err(format!("har: {err}\n")),
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.lock.unlock();
        if fs::read_to_string(self.path.join("owner")).is_ok_and(|owner| owner == self.owner) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn process_identity_binds_the_pid_start_and_boot_and_rejects_dead_processes() {
        let mut fields = vec!["0"; 20];
        fields[0] = "S";
        fields[19] = "7";
        let stat = format!("42 (wasm worker) {}", fields.join(" "));
        assert_eq!(stat_identity(&stat, "btime 99\n", None).as_deref(), Some("42:7:99"));
        assert_eq!(stat_identity(&stat, "btime 99\n", Some(42)).as_deref(), Some("42:7:99"));
        assert!(stat_identity(&stat, "btime 99\n", Some(43)).is_none());
        assert!(stat_identity(&stat, "", None).is_none());
        assert!(stat_identity("0 (worker) S", "btime 99\n", None).is_none());
        assert!(stat_identity("42 worker S", "btime 99\n", None).is_none());
        for state in ["Z", "X"] {
            fields[0] = state;
            assert!(stat_identity(&format!("42 (worker) {}", fields.join(" ")), "btime 99\n", None).is_none());
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn native_lease_liveness_is_the_held_lock_not_the_heartbeat_age() {
        let root = std::env::temp_dir().join(format!("slicc-har-lease-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&root).unwrap();
        let record = root.join("record.har.json");
        let lease = Lease::claim(&record, false).unwrap();
        assert!(inspect(&record).alive);
        assert_eq!(Lease::claim(&record, false).err().unwrap(), ALREADY);
        drop(lease);
        assert!(!inspect(&record).alive);
        let lease = Lease::claim(&record, false).unwrap();
        drop(lease);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn ownership_changes_and_unknown_owner_handles_are_not_removed() {
        let root = std::env::temp_dir().join(format!("slicc-har-owner-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&root).unwrap();
        let record = root.join("record.har.json");
        let lease = Lease::claim(&record, false).unwrap();
        lease.lock.unlock().unwrap();
        fs::write(lease.path.join("owner"), "different owner").unwrap();
        let path = lease.path.clone();
        drop(lease);
        assert!(path.is_dir());
        fs::remove_file(path.join("owner")).unwrap();
        fs::create_dir(path.join("owner")).unwrap();
        assert!(inspect(&record).alive);
        fs::remove_dir_all(root).unwrap();
    }
}

