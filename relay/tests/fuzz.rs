//! Request fuzzing with proptest (cargo-fuzz needs a nightly toolchain this box does
//! not carry; ADR 0013). Three properties, over the handler as the HTTP layer calls it:
//!
//! 1. Any bytes to any path: no panic, and every answer is one the protocol lists.
//! 2. Any mutation of a valid signed request: never `200` unless the bytes are the
//!    original's (signatures and household auth hold under tampering).
//! 3. The nesting guard agrees with the dCBOR decoder on shallow input, and deep input
//!    (the decoder's stack hazard) is refused without decoding.
//!
//! `PROPTEST_CASES` scales the run (default 256 per property; the stage 3 report
//! records the bounded pass).

use hearth_sync_kernel::keys::{sign_enroll, DeviceSigner, HouseholdRoot, SoftSigner};
use hearth_sync_relay::relay::Response;
use hearth_sync_relay::wire::{self, channel_id, client, nesting_ok, Channel, MAX_DEPTH};
use hearth_sync_relay::{Config, Relay};
use proptest::prelude::*;

const APP: &str = "lullaby";
const NOW: u64 = 1_727_000_000_000;

fn root() -> HouseholdRoot {
    HouseholdRoot::from_seed([5; 64])
}

fn ch() -> Channel {
    channel_id(APP, &root().enroll_public(APP))
}

fn path(verb: &str) -> String {
    let h: String = ch().iter().map(|b| format!("{b:02x}")).collect();
    format!("/v1/{h}/{verb}")
}

const VERBS: [&str; 6] = ["enroll", "forget", "append", "snapshot", "pull", "fetch_snapshot"];
const CODES: [&str; 16] = [
    "bad_request",
    "channel_mismatch",
    "bad_envelope",
    "stale",
    "bad_signature",
    "replay",
    "bad_auth",
    "not_enrolled",
    "forgotten",
    "no_snapshot",
    "seq",
    "too_large",
    "rate_limited",
    "quota",
    "not_found",
    "method",
];

fn env(tag: u8) -> Vec<u8> {
    // A structurally valid sealed envelope: [1, 0, ref, nonce, ct].
    dcbor::CBOR::from(vec![
        dcbor::CBOR::from(1u64),
        dcbor::CBOR::from(0u64),
        dcbor::CBOR::to_byte_string([tag; 32]),
        dcbor::CBOR::to_byte_string([0u8; 24]),
        dcbor::CBOR::to_byte_string([tag; 40]),
    ])
    .to_cbor_data()
}

/// A relay with devices A (enrolled, has appended and snapshotted) and B (enrolled).
fn setup() -> (Relay, SoftSigner, SoftSigner) {
    let mut r = Relay::open(None, Config::default()).unwrap();
    let (a, b) = (SoftSigner::from_secret([1; 32]), SoftSigner::from_secret([2; 32]));
    for (d, l) in [(&a, "a"), (&b, "b")] {
        let body =
            client::enroll(APP, root().enroll_public(APP), d.device(), l, sign_enroll(&root(), APP, &d.device(), l));
        assert_eq!(r.handle("POST", &path("enroll"), &body, NOW).status, 200);
    }
    let body = client::append(&ch(), &a, 1, vec![env(1)], NOW);
    assert_eq!(r.handle("POST", &path("append"), &body, NOW).status, 200);
    let body = client::snapshot(&ch(), &a, env(2), vec![(a.device(), 1)], NOW);
    assert_eq!(r.handle("POST", &path("snapshot"), &body, NOW).status, 200);
    (r, a, b)
}

/// Valid requests of every verb (each answers 200 on a fresh setup).
fn valid(a: &SoftSigner, b: &SoftSigner) -> Vec<(&'static str, Vec<u8>)> {
    let c = SoftSigner::from_secret([3; 32]);
    let root = root();
    let auth = hearth_sync_kernel::keys::sign_forget(&root, APP, &b.device(), &[[9; 32]]);
    vec![
        (
            "enroll",
            client::enroll(APP, root.enroll_public(APP), c.device(), "c", sign_enroll(&root, APP, &c.device(), "c")),
        ),
        ("forget", client::forget(&ch(), a, b.device(), &[[9; 32]], auth, 0, NOW)),
        ("append", client::append(&ch(), a, 2, vec![env(3), env(4)], NOW)),
        ("snapshot", client::snapshot(&ch(), b, env(5), vec![(a.device(), 1)], NOW)),
        ("pull", client::pull(&ch(), b, 1, [7; 16], vec![(a.device(), 0)], NOW)),
        ("fetch_snapshot", client::fetch_snapshot(&ch(), b, 1, [8; 16], a.device(), NOW)),
    ]
}

fn well_formed(r: &Response) {
    if r.status == 200 {
        let c = dcbor::CBOR::try_from_data(&r.body).expect("a dCBOR answer");
        let dcbor::CBORCase::Array(items) = c.as_case() else { panic!("not an array") };
        assert_eq!(items[0], dcbor::CBOR::from("ok"));
    } else {
        let code = client::error_code(&r.body).expect("an error body");
        assert!(CODES.contains(&code.as_str()), "unlisted code {code}");
    }
}

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(256)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn any_bytes_to_any_verb_get_a_listed_answer(verb in 0usize..6, body in prop::collection::vec(any::<u8>(), 0..512)) {
        let (mut r, _, _) = setup();
        let resp = r.handle("POST", &path(VERBS[verb]), &body, NOW);
        well_formed(&resp);
    }

    #[test]
    fn cbor_shaped_junk_gets_a_listed_answer(verb in 0usize..6, body in cbor_value(4)) {
        let (mut r, _, _) = setup();
        let resp = r.handle("POST", &path(VERBS[verb]), &body.to_cbor_data(), NOW);
        well_formed(&resp);
        prop_assert_ne!(resp.status, 200);
    }

    #[test]
    fn a_tampered_signed_request_is_never_accepted(
        which in 0usize..6,
        edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>(), 0u8..3), 1..4),
    ) {
        let (mut r, a, b) = setup();
        let (verb, orig) = valid(&a, &b).swap_remove(which);
        let mut body = orig.clone();
        for (at, byte, op) in edits {
            if body.is_empty() { break; }
            let i = at.index(body.len());
            match op {
                0 => body[i] ^= byte | 1,
                1 => { body.insert(i, byte); }
                _ => { body.remove(i); }
            }
        }
        let resp = r.handle("POST", &path(verb), &body, NOW);
        well_formed(&resp);
        if body != orig {
            prop_assert_ne!(resp.status, 200, "{} accepted a tampered body", verb);
        }
    }

    #[test]
    fn the_nesting_guard_agrees_with_the_decoder(body in cbor_value(6)) {
        let data = body.to_cbor_data();
        // Every generated value nests at most 7 deep, so the guard must pass it.
        prop_assert!(nesting_ok(&data, MAX_DEPTH));
        prop_assert!(wire::decode(&data).is_ok());
    }

    #[test]
    fn the_nesting_guard_never_passes_what_the_decoder_rejects_for_structure(data in prop::collection::vec(any::<u8>(), 0..64)) {
        // Truncated or trailing input fails both.
        if nesting_ok(&data, MAX_DEPTH) {
            // Structure is fine; dCBOR may still refuse it (non-canonical ints, bad
            // text, floats), which is its job.
        } else {
            prop_assert!(wire::decode(&data).is_err());
        }
    }
}

fn cbor_value(depth: u32) -> impl Strategy<Value = dcbor::CBOR> {
    let leaf = prop_oneof![
        any::<u64>().prop_map(dcbor::CBOR::from),
        any::<i64>().prop_map(dcbor::CBOR::from),
        "[a-z]{0,8}".prop_map(|s| dcbor::CBOR::from(s.as_str())),
        prop::collection::vec(any::<u8>(), 0..40).prop_map(dcbor::CBOR::to_byte_string),
        Just(dcbor::CBOR::null()),
        any::<bool>().prop_map(dcbor::CBOR::from),
    ];
    leaf.prop_recursive(depth, 32, 6, |inner| prop::collection::vec(inner, 0..6).prop_map(dcbor::CBOR::from))
}

#[test]
fn every_valid_request_is_accepted_so_the_tamper_property_is_not_vacuous() {
    for which in 0..6 {
        let (mut r, a, b) = setup();
        let (verb, body) = valid(&a, &b).swap_remove(which);
        let resp = r.handle("POST", &path(verb), &body, NOW);
        assert_eq!(resp.status, 200, "{verb}: {:?}", client::error_code(&resp.body));
    }
}

#[test]
fn deep_nesting_is_refused_without_recursing() {
    let (mut r, a, _) = setup();
    // As a whole body, arrays and tags, up to the HTTP body limit's order of size.
    for byte in [0x81u8, 0xc1, 0xa1] {
        let body = vec![byte; 4 << 20];
        let resp = r.handle("POST", &path("pull"), &body, NOW);
        assert_eq!(client::error_code(&resp.body).as_deref(), Some("bad_request"));
    }
    // Inside a validly signed append: the envelope parser recurses too.
    let deep = vec![0x81u8; 60_000];
    let body = client::append(&ch(), &a, 2, vec![deep], NOW);
    let resp = r.handle("POST", &path("append"), &body, NOW);
    assert_eq!(client::error_code(&resp.body).as_deref(), Some("bad_envelope"));
    // A hostile relay's answers, parsed by a client.
    let deep = vec![0x81u8; 1 << 20];
    assert!(client::parse_pull(&deep).is_none() && client::parse_fetch(&deep).is_none());
    assert!(client::error_code(&deep).is_none());
    // And inside a snapshot.
    let body = client::snapshot(&ch(), &a, vec![0x81u8; 1 << 20], vec![], NOW);
    let resp = r.handle("POST", &path("snapshot"), &body, NOW);
    assert_eq!(client::error_code(&resp.body).as_deref(), Some("bad_envelope"));
}
