//! Structured logs: one JSON object per line on stderr. They never carry a channel id,
//! a device id or a client address (design §7.5): only verbs, statuses and timings.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o
}

/// Write `{"ts":…,"level":…,k:v…}` to stderr. Values are strings.
pub fn event(level: &str, fields: &[(&str, &str)]) {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let mut line = format!("{{\"ts\":{ts},\"level\":\"{}\"", escape(level));
    for (k, v) in fields {
        line.push_str(&format!(",\"{}\":\"{}\"", escape(k), escape(v)));
    }
    line.push_str("}\n");
    // A full or closed stderr must never take the relay down.
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}
