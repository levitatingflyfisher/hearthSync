//! The Rust relay's handler behind a line protocol, for go-relay/difftest.
//!
//! Each stdin line is one JSON command; each gets one JSON line back:
//! - `{"op":"reset","config":{name: n...},"dir":path}` → a fresh relay over a new, unsynced SQLite
//!   file in `dir` (in memory without `dir`) → `{"ok":true}`
//! - `{"op":"restart"}` → drop the relay and open it again over the same file (memory is
//!   lost, the epoch moves on) → `{"digest":hex}`
//! - `{"op":"req","method":…,"path":…,"body":hex,"now":n}` → `{"status":n,"body":hex,"digest":hex}`
//! - `{"op":"sweep","now":n}` → `{"pruned":n,"digest":hex}`
//!
//! It is built in the dev profile, so arithmetic overflow in the relay panics (and the
//! harness reports the driver's death) rather than wrapping silently.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use hearth_sync_relay::{Config, Relay};
use serde_json::{json, Value};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex")).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut relay: Option<Relay> = None;
    let mut opened: Option<(Option<PathBuf>, Config)> = None;
    for line in stdin.lock().lines() {
        let line = line.expect("stdin");
        let cmd: Value = serde_json::from_str(&line).expect("json command");
        let answer = match cmd["op"].as_str().expect("op") {
            "reset" => {
                let mut cfg = Config::default();
                for (k, v) in cmd["config"].as_object().expect("config") {
                    assert!(cfg.set(k, v.as_u64().expect("u64")), "unknown config key {k}");
                }
                let dir = cmd["dir"].as_str().map(PathBuf::from);
                drop(relay.take());
                relay = Some(match &dir {
                    Some(d) => Relay::open_unsynced(d, cfg.clone()),
                    None => Relay::open(None, cfg.clone()),
                }
                .expect("open"));
                opened = Some((dir, cfg));
                json!({"ok": true})
            }
            "restart" => {
                let (dir, cfg) = opened.clone().expect("reset first");
                drop(relay.take());
                let r = relay.insert(Relay::open_unsynced(dir.as_deref().expect("a dir"), cfg).expect("reopen"));
                json!({"digest": hex(&r.digest().expect("digest"))})
            }
            "req" => {
                let r = relay.as_mut().expect("reset first");
                let resp = r.handle(
                    cmd["method"].as_str().expect("method"),
                    cmd["path"].as_str().expect("path"),
                    &unhex(cmd["body"].as_str().expect("body")),
                    cmd["now"].as_u64().expect("now"),
                );
                json!({"status": resp.status, "body": hex(&resp.body), "digest": hex(&r.digest().expect("digest"))})
            }
            "sweep" => {
                let r = relay.as_mut().expect("reset first");
                let n = r.sweep(cmd["now"].as_u64().expect("now")).expect("sweep");
                json!({"pruned": n, "digest": hex(&r.digest().expect("digest"))})
            }
            op => panic!("unknown op {op}"),
        };
        writeln!(out, "{answer}").expect("stdout");
        out.flush().expect("stdout");
    }
}
