use serde_json::Value;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static ON: OnceLock<bool> = OnceLock::new();
static LINES: AtomicUsize = AtomicUsize::new(0);
const CAP: usize = 280;

pub fn on() -> bool {
    *ON.get_or_init(|| {
        START.get_or_init(Instant::now);
        std::env::var("SLICC_TRACE_HAR").is_ok_and(|value| value == "1")
    })
}

pub fn ms() -> u128 {
    START.get_or_init(Instant::now).elapsed().as_millis()
}

pub fn line(text: std::fmt::Arguments) {
    let count = LINES.fetch_add(1, Ordering::Relaxed);
    if count < CAP {
        let _ = writeln!(std::io::stderr(), "TRACE-HAR t={} {text}", ms());
    } else if count == CAP {
        let _ = writeln!(std::io::stderr(), "TRACE-HAR t={} cap reached", ms());
    }
}

#[macro_export]
macro_rules! trace_har {
    ($($arg:tt)*) => {
        if $crate::trace::on() {
            $crate::trace::line(format_args!($($arg)*));
        }
    };
}

pub fn fingerprint(value: &Value) -> (usize, u64) {
    let mut size = 0usize;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    walk(value, &mut size, &mut hash);
    (size, hash)
}

fn mix(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
}

fn walk(value: &Value, size: &mut usize, hash: &mut u64) {
    match value {
        Value::Null => mix(hash, b"n"),
        Value::Bool(flag) => mix(hash, if *flag { b"t" } else { b"f" }),
        Value::Number(number) => mix(hash, &number.as_f64().unwrap_or(0.0).to_bits().to_le_bytes()),
        Value::String(text) => {
            *size += text.len();
            mix(hash, b"s");
            mix(hash, text.as_bytes());
        }
        Value::Array(items) => {
            mix(hash, b"[");
            for item in items {
                walk(item, size, hash);
            }
            mix(hash, b"]");
        }
        Value::Object(map) => {
            mix(hash, b"{");
            for (key, item) in map {
                *size += key.len();
                mix(hash, key.as_bytes());
                walk(item, size, hash);
            }
            mix(hash, b"}");
        }
    }
}

pub fn path(url: &str) -> String {
    url::Url::parse(url).map_or_else(|_| format!("unparsed:{}", url.len()), |parsed| format!("{}:{}", parsed.scheme(), parsed.path()))
}

pub fn short(session: &str) -> &str {
    session.get(..8).unwrap_or(session)
}
