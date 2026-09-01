//! The shared conformance suite: every case in vectors/relay_v1.json, replayed against
//! the handler, must answer each step byte for byte and leave the store with the
//! case's digest. The Go relay runs the same file.

use hearth_sync_relay::{Config, Relay};
use serde_json::Value;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn config(v: &Value) -> Config {
    let mut c = Config::default();
    for (k, n) in v.as_object().unwrap() {
        assert!(c.set(k, n.as_u64().unwrap()), "unknown config key {k}");
    }
    c
}

#[test]
fn relay_v1_vectors() {
    let doc: Value = serde_json::from_str(include_str!("../../vectors/relay_v1.json")).unwrap();
    assert_eq!(doc["format"], "hearthSync relay conformance v1");
    let base = config(&doc["config"]);
    assert_eq!(base, Config::default(), "the vectors' defaults are the protocol's");
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 10, "the suite is not empty");
    let mut steps = 0;
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let mut cfg = base.clone();
        for (k, n) in case["config"].as_object().unwrap() {
            assert!(cfg.set(k, n.as_u64().unwrap()), "{name}: unknown config key {k}");
        }
        // A real store on disk, so a `restart` step really stops and reopens the relay.
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("relay-conformance").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut relay = Relay::open_unsynced(&dir, cfg.clone()).unwrap();
        for (i, s) in case["steps"].as_array().unwrap().iter().enumerate() {
            if s["sweep"].as_bool() == Some(true) {
                relay.sweep(s["now"].as_u64().unwrap()).unwrap();
                continue;
            }
            if s["restart"].as_bool() == Some(true) {
                drop(relay);
                relay = Relay::open_unsynced(&dir, cfg.clone()).unwrap();
                continue;
            }
            let r = relay.handle(
                s["method"].as_str().unwrap(),
                s["path"].as_str().unwrap(),
                &unhex(s["body"].as_str().unwrap()),
                s["now"].as_u64().unwrap(),
            );
            let want = s["response"].as_str().unwrap();
            assert_eq!(
                (r.status as u64, hex(&r.body)),
                (s["status"].as_u64().unwrap(), want.to_string()),
                "{name} step {i} (expects {})",
                s["expect"]
            );
            steps += 1;
        }
        assert_eq!(hex(&relay.digest().unwrap()), case["store_digest"].as_str().unwrap(), "{name}: store digest");
    }
    assert!(steps > 100, "{steps} steps");
}
