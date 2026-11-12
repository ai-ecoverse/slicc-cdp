use crate::browser;
use crate::cdp::{self, Cdp};
use crate::session;
use serde_json::{json, Value};
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_CHARS: usize = 120;
pub const LINGER_MS: u64 = 1500;
pub const RUNNING_MS: u64 = 60000;
pub const GRACE_MS: u64 = 1000;
const KEEP_MS: u64 = 600000;
const CALL: Duration = Duration::from_millis(500);
const WORLD: &str = "slicc-intent";

const PAGE_COMMANDS: &[&str] = &[
    "open", "tab-new", "goto", "navigate", "go-back", "go-forward", "reload", "click", "dblclick", "fill", "type", "press",
    "keydown", "keyup", "hover", "select", "check", "uncheck", "upload", "drag", "drop", "mousemove", "mousedown", "mouseup",
    "mousewheel", "eval", "eval-file", "snapshot", "screenshot", "pdf", "resize", "highlight", "generate-locator", "fetch",
    "webmcp-call",
];

const SHOW: &str = "function(text, ms) { const doc = document; const root = doc.documentElement; if (!root) return false; for (const old of Array.from(root.children)) if (old.localName === 'x-slicc-intent') old.remove(); const host = doc.createElement('x-slicc-intent'); host.setAttribute('popover', 'manual'); host.setAttribute('aria-hidden', 'true'); host.setAttribute('inert', ''); host.style.cssText = 'position:fixed!important;inset:0!important;width:100%!important;height:100%!important;max-width:none!important;max-height:none!important;margin:0!important;padding:0!important;border:none!important;overflow:visible!important;pointer-events:none!important;background:transparent!important;display:block!important;visibility:visible!important;opacity:1!important;transform:none!important;z-index:2147483647!important'; let shadow; try { shadow = host.attachShadow({ mode: 'closed' }); } catch { return false; } const label = doc.createElement('div'); label.dir = 'auto'; label.textContent = text; label.style.cssText = 'all:initial;position:absolute;left:50%;bottom:16px;transform:translateX(-50%);box-sizing:border-box;max-width:calc(100% - 32px);padding:6px 14px;border:1px solid #6fa8dc;border-radius:999px;background:rgba(24,24,27,0.88);color:#fff;font:500 13px/1.4 system-ui,-apple-system,\"Segoe UI\",sans-serif;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;unicode-bidi:isolate;box-shadow:0 2px 8px rgba(0,0,0,0.25);pointer-events:none;user-select:none'; shadow.appendChild(label); root.appendChild(host); try { host.showPopover(); } catch {} setTimeout(() => host.remove(), ms); return true; }";

const HIDE: &str = "function() { const root = document.documentElement; if (!root) return false; for (const old of Array.from(root.children)) if (old.localName === 'x-slicc-intent') old.remove(); return true; }";

pub fn acts_on_page(command: &str) -> bool {
    PAGE_COMMANDS.contains(&command)
}

pub fn captures(command: &str) -> bool {
    matches!(command, "screenshot" | "pdf")
}

pub fn choose(flag: Option<&str>, env: Option<&str>) -> Option<String> {
    match flag {
        Some(flag) => clean(flag),
        None => env.and_then(clean),
    }
}

pub fn clean(raw: &str) -> Option<String> {
    let kept: String = raw
        .chars()
        .map(|ch| if ch.is_whitespace() { ' ' } else { ch })
        .filter(|ch| !ch.is_control() && !hidden(*ch))
        .collect();
    let joined = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = joined.chars().take(MAX_CHARS).collect();
    let cut = cut.trim_end();
    (!cut.is_empty()).then(|| cut.to_string())
}

fn hidden(ch: char) -> bool {
    matches!(
        ch as u32,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x115F..=0x1160
            | 0x180B..=0x180F
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x206F
            | 0x3164
            | 0xFE00..=0xFE0F
            | 0xFEFF
            | 0xFFA0
            | 0xFFF9..=0xFFFB
            | 0xFFFE..=0xFFFF
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE0FFF
    )
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|time| time.as_millis() as u64).unwrap_or(0)
}

pub fn show(cdp: &mut Cdp, tab: &str, text: &str, ms: u64) {
    let _ = on_tab(cdp, tab, SHOW, json!([{ "value": text }, { "value": ms }]));
}

pub fn clear(cdp: &mut Cdp, session: &str, tab: &str) {
    let now = now_ms();
    if showing(&recent(now), tab, now) {
        let _ = apply(cdp, session, HIDE, json!([]));
    }
}

fn showing(entries: &[Entry], tab: &str, now: u64) -> bool {
    entries.iter().any(|entry| entry.tab == tab && !entry.intent.is_empty() && entry.until() + LINGER_MS + GRACE_MS >= now)
}

fn on_tab(cdp: &mut Cdp, tab: &str, declaration: &str, arguments: Value) -> Result<(), String> {
    if cdp.is_protected_target(tab) {
        return Err(format!("{}\n", cdp.guard().refusal()));
    }
    let session = cdp::attach(cdp, tab)?;
    let result = apply(cdp, &session, declaration, arguments);
    cdp::detach(cdp, &session);
    result
}

fn apply(cdp: &mut Cdp, session: &str, declaration: &str, arguments: Value) -> Result<(), String> {
    let refusal = format!("{}\n", cdp.guard().refusal());
    let document = main_document(cdp, session).ok_or_else(|| refusal.clone())?;
    let world = cdp.call_for("Page.createIsolatedWorld", json!({ "frameId": document.0, "worldName": WORLD }), Some(session), CALL)?;
    let context = world.get("executionContextId").cloned().ok_or_else(|| refusal.clone())?;
    if main_document(cdp, session).as_ref() != Some(&document) {
        return Err(refusal);
    }
    let params = json!({ "executionContextId": context, "functionDeclaration": declaration, "arguments": arguments, "returnByValue": true });
    cdp.call_for("Runtime.callFunctionOn", params, Some(session), CALL).map(|_| ())
}

fn main_document(cdp: &mut Cdp, session: &str) -> Option<(String, String)> {
    if cdp.content_blocked(session) {
        return None;
    }
    let tree = cdp.call_for("Page.getFrameTree", json!({}), Some(session), CALL).ok()?;
    let frame = tree.pointer("/frameTree/frame")?;
    let text = |key: &str| frame.get(key).and_then(Value::as_str).unwrap_or("").to_string();
    let (id, loader) = (text("id"), text("loaderId"));
    let ordinary = !id.is_empty() && !loader.is_empty() && text("unreachableUrl").is_empty() && browser::ordinary_frame_url(cdp.guard(), &text("url"));
    ordinary.then_some((id, loader))
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
    fn an_explicit_flag_wins_over_the_environment() {
        assert_eq!(choose(Some("Filling the order form"), Some("Reading mail")).as_deref(), Some("Filling the order form"));
        assert_eq!(choose(None, Some("Reading mail")).as_deref(), Some("Reading mail"));
        assert_eq!(choose(Some(""), Some("Reading mail")), None);
        assert_eq!(choose(Some("\u{200b}"), Some("Reading mail")), None);
        assert_eq!(choose(None, None), None);
        assert_eq!(choose(None, Some("  ")), None);
    }

    #[test]
    fn cleaning_drops_controls_and_format_characters_and_collapses_whitespace() {
        assert_eq!(clean("  Filling\tthe\n\norder\r\nform  ").as_deref(), Some("Filling the order form"));
        assert_eq!(clean("Filling\u{202e}mrof\u{202c} the form").as_deref(), Some("Fillingmrof the form"));
        assert_eq!(clean("Fil\u{200b}ling\u{200d} the\u{feff} \u{2066}form\u{2069}").as_deref(), Some("Filling the form"));
        assert_eq!(clean("Reading\u{2028}// injected\u{2029}code").as_deref(), Some("Reading // injected code"));
        assert_eq!(clean("Reading\u{0085}\u{0007}\u{001b}[31mred").as_deref(), Some("Reading [31mred"));
        assert_eq!(clean("Tag\u{e0041}\u{e0042}ged").as_deref(), Some("Tagged"));
        assert_eq!(clean("Écrire le café ✓").as_deref(), Some("Écrire le café ✓"));
        assert_eq!(clean("\u{200b}\u{200e}\n"), None);
    }

    #[test]
    fn cleaning_cuts_long_intents_on_character_boundaries() {
        let long = "é".repeat(500);
        let cut = clean(&long).unwrap();
        assert_eq!(cut.chars().count(), MAX_CHARS);
        let spaced = format!("{} tail", "a".repeat(MAX_CHARS - 1));
        assert_eq!(clean(&spaced).unwrap(), "a".repeat(MAX_CHARS - 1));
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
        assert!(showing(&entries, "T", 2000 + LINGER_MS + GRACE_MS));
        assert!(!showing(&entries, "T", 2001 + LINGER_MS + GRACE_MS));
        assert!(!showing(&entries, "U", 1500));
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
