use crate::session;
use serde_json::{json, Value};
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

pub use cdp_client::intent::{choose, clean};

pub const RUNNING_MS: u64 = 60000;
pub const GRACE_MS: u64 = 1000;
const KEEP_MS: u64 = 600000;

const PAGE_COMMANDS: &[&str] = &[
    "open", "tab-new", "goto", "navigate", "go-back", "go-forward", "reload", "click", "dblclick", "fill", "type", "press",
    "keydown", "keyup", "hover", "select", "check", "uncheck", "upload", "drag", "drop", "mousemove", "mousedown", "mouseup",
    "mousewheel", "eval", "eval-file", "snapshot", "screenshot", "pdf", "resize", "highlight", "generate-locator", "fetch",
    "webmcp-call",
];

pub fn acts_on_page(command: &str) -> bool {
    PAGE_COMMANDS.contains(&command)
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_millis() as u64).unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    pub tab: String,
    pub intent: String,
    pub start: u64,
    pub end: Option<u64>,
}

impl Entry {
    fn until(&self) -> u64 {
        self.end.unwrap_or(self.start + RUNNING_MS)
    }
}

pub fn fresh_id(start: u64) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let clock = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    #[cfg(target_os = "wasi")]
    let process = fs::read_to_string("/proc/self/stat").unwrap_or_default();
    #[cfg(not(target_os = "wasi"))]
    let process = std::process::id().to_string();
    let token = cdp_client::hash::sha256_hex(format!("{process}:{clock}:{counter}").as_bytes());
    format!("{start}-{}", &token[..16])
}

pub fn note(entry: &Entry) {
    let Some(dir) = session::intents_dir() else { return };
    let body = json!({ "id": entry.id, "tab": entry.tab, "intent": entry.intent, "start": entry.start, "end": entry.end });
    let _ = session::write_private(&dir.join(format!("{}.json", entry.id)), &body.to_string());
    prune(&dir);
}

fn prune(dir: &std::path::Path) {
    let horizon = now_ms().saturating_sub(KEEP_MS);
    for item in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let stale = name.split('-').next().and_then(|start| start.parse::<u64>().ok()).is_some_and(|start| start < horizon);
        if stale && name.ends_with(".json") {
            let _ = fs::remove_file(item.path());
        }
    }
}

pub fn entries() -> Vec<Entry> {
    entries_since(0)
}

pub fn recent(since: u64) -> Vec<Entry> {
    entries_since(since.saturating_sub(RUNNING_MS + GRACE_MS))
}

fn entries_since(earliest: u64) -> Vec<Entry> {
    let Some(dir) = session::intents_dir() else { return Vec::new() };
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|item| {
            let name = item.file_name().to_string_lossy().into_owned();
            name.ends_with(".json") && name.split('-').next().and_then(|start| start.parse::<u64>().ok()).is_some_and(|start| start >= earliest)
        })
        .filter_map(|item| serde_json::from_str::<Value>(&fs::read_to_string(item.path()).ok()?).ok())
        .filter_map(|value| {
            Some(Entry {
                id: value["id"].as_str()?.to_string(),
                tab: value["tab"].as_str()?.to_string(),
                intent: clean(value["intent"].as_str()?).unwrap_or_default(),
                start: value["start"].as_u64()?,
                end: value["end"].as_u64(),
            })
        })
        .collect()
}

pub fn touched(entries: &[Entry], tab: &str, since: u64) -> bool {
    entries.iter().any(|entry| entry.tab == tab && !entry.intent.is_empty() && entry.until() + GRACE_MS >= since)
}

pub fn attribute<'a>(entries: &'a [Entry], tab: &str, at: u64) -> Option<&'a Entry> {
    let started: Vec<&Entry> = entries.iter().filter(|entry| entry.tab == tab && entry.start <= at).collect();
    let running: Vec<&Entry> = started.iter().copied().filter(|entry| at <= entry.until()).collect();
    let chosen = match running.iter().max_by_key(|entry| entry.start) {
        Some(first) => running.iter().all(|entry| entry.intent == first.intent).then_some(*first),
        None => started.into_iter().max_by_key(|entry| entry.start).filter(|entry| entry.end.is_some_and(|end| at <= end + GRACE_MS)),
    };
    chosen.filter(|entry| !entry.intent.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, tab: &str, intent: &str, start: u64, end: Option<u64>) -> Entry {
        Entry { id: id.into(), tab: tab.into(), intent: intent.into(), start, end }
    }

    #[test]
    fn page_commands_exclude_tab_bookkeeping() {
        for command in ["click", "fill", "type", "press", "select", "check", "goto", "open", "eval", "screenshot", "snapshot"] {
            assert!(acts_on_page(command), "{command}");
        }
        for command in ["tab-list", "tab-select", "tab-close", "close", "list", "console", "requests", "har-start", "recording-start", "dialog-accept"] {
            assert!(!acts_on_page(command), "{command}");
        }
    }

    #[test]
    fn attribution_prefers_the_running_command_then_a_short_grace() {
        let entries = vec![
            entry("a", "T", "Opening the page", 1000, Some(2000)),
            entry("b", "T", "Filling the form", 2200, Some(3000)),
            entry("c", "U", "Elsewhere", 0, Some(10000)),
        ];
        assert_eq!(attribute(&entries, "T", 500), None);
        assert_eq!(attribute(&entries, "T", 1500).map(|entry| entry.id.as_str()), Some("a"));
        assert_eq!(attribute(&entries, "T", 2100).map(|entry| entry.id.as_str()), Some("a"));
        assert_eq!(attribute(&entries, "T", 2300).map(|entry| entry.id.as_str()), Some("b"));
        assert_eq!(attribute(&entries, "T", 3000 + GRACE_MS).map(|entry| entry.id.as_str()), Some("b"));
        assert_eq!(attribute(&entries, "T", 3001 + GRACE_MS), None);
        assert_eq!(attribute(&entries, "V", 1500), None);
    }

    #[test]
    fn overlapping_commands_with_different_intents_are_not_attributed() {
        let entries = vec![entry("a", "T", "Reading", 1000, Some(5000)), entry("b", "T", "Writing", 2000, None)];
        assert_eq!(attribute(&entries, "T", 1500).map(|entry| entry.id.as_str()), Some("a"));
        assert_eq!(attribute(&entries, "T", 3000), None);
        assert_eq!(attribute(&entries, "T", 6000).map(|entry| entry.id.as_str()), Some("b"));
        assert_eq!(attribute(&entries, "T", 2000 + RUNNING_MS + 1), None);
        let same = vec![entry("a", "T", "Reading", 1000, Some(5000)), entry("b", "T", "Reading", 2000, Some(4000))];
        assert_eq!(attribute(&same, "T", 3000).map(|entry| entry.id.as_str()), Some("b"));
    }

    #[test]
    fn a_later_command_without_an_intent_ends_the_grace_window() {
        let entries = vec![entry("a", "T", "Placing the order", 1000, Some(2000)), entry("b", "T", "", 2200, Some(2400))];
        assert!(touched(&entries, "T", 2000 + GRACE_MS));
        assert!(!touched(&entries, "T", 2001 + GRACE_MS));
        assert!(!touched(&entries, "U", 1500));
        assert_eq!(attribute(&entries, "T", 2100).map(|entry| entry.id.as_str()), Some("a"));
        assert_eq!(attribute(&entries, "T", 2300), None);
        assert_eq!(attribute(&entries, "T", 2500), None);
        let mixed = vec![entry("a", "T", "Reading", 1000, Some(5000)), entry("b", "T", "", 2000, Some(3000))];
        assert_eq!(attribute(&mixed, "T", 2500), None);
    }

    #[test]
    fn the_ledger_round_trips_and_prunes_old_entries() {
        let dir = session::intents_dir().unwrap();
        let start = now_ms();
        let id = fresh_id(start);
        let mut written = entry(&id, "LEDGER-TAB", "Checking\u{202e} the ledger", start, None);
        note(&written);
        written.end = Some(start + 5);
        note(&written);
        let found = entries().into_iter().find(|item| item.id == id).unwrap();
        assert_eq!(found.intent, "Checking the ledger");
        assert_eq!(found.end, Some(start + 5));
        let old = fresh_id(start - KEEP_MS - 1);
        note(&entry(&old, "LEDGER-TAB", "Old", start - KEEP_MS - 1, Some(start - KEEP_MS)));
        note(&entry(&fresh_id(start), "LEDGER-TAB", "Pruning", start, None));
        assert!(!dir.join(format!("{old}.json")).exists());
        assert!(dir.join(format!("{id}.json")).exists());
    }
}
