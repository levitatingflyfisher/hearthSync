//! Sealing (ADR 0008) against `vectors/seal_v1.json`, built by PyNaCl (libsodium's
//! XChaCha20-Poly1305) and pyca: no expected byte here comes from the kernel. Then
//! the tamper matrix: every part of an envelope, and every binding in its AAD.

mod common;
use common::*;

use hearth_sync_kernel::keys::HouseholdRoot;
use hearth_sync_kernel::seal::{envelope_ref, SealError, SealKeys, SealKind};
use hearth_sync_kernel::sha256;
use serde_json::Value as J;

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn load() -> J {
    let path = format!("{}/../vectors/seal_v1.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn kind(s: &str) -> SealKind {
    match s {
        "op" => SealKind::Op,
        "snapshot" => SealKind::Snapshot,
        _ => SealKind::Msg,
    }
}

fn keys() -> SealKeys {
    SealKeys::derive(&root(), APP)
}

#[test]
fn every_case_seals_byte_for_byte_and_opens() {
    let v = load();
    let derived = keys();
    assert_eq!(derived.household().to_vec(), unhex(v["household"].as_str().unwrap()));
    let raw = SealKeys::from_parts(
        APP,
        unhex(v["household"].as_str().unwrap()).try_into().unwrap(),
        unhex(v["seal_key"].as_str().unwrap()).try_into().unwrap(),
        unhex(v["nonce_key"].as_str().unwrap()).try_into().unwrap(),
    );
    for c in v["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let k = kind(c["kind"].as_str().unwrap());
        let reference: Option<[u8; 32]> = c["ref"].as_str().map(|r| unhex(r).try_into().unwrap());
        let pt = unhex(c["plaintext"].as_str().unwrap());
        let env = unhex(c["envelope"].as_str().unwrap());
        // The keys derived from the seed are the vector's keys: same envelope.
        assert_eq!(derived.seal(k, reference.as_ref(), &pt), env, "{name}: derived keys");
        assert_eq!(raw.seal(k, reference.as_ref(), &pt), env, "{name}: raw keys");
        assert_eq!(derived.open(k, &env), Ok((reference, pt.clone())), "{name}: opens");
        assert_eq!(envelope_ref(&env), reference, "{name}: ref in clear");
        if k == SealKind::Op {
            assert_eq!(derived.open_op(&env), Ok((sha256(&pt), pt)), "{name}: op id checked");
        }
    }
}

#[test]
fn a_changed_byte_anywhere_in_an_envelope_is_refused() {
    let k = keys();
    let op = b"stand-in for signed op bytes".to_vec();
    let env = k.seal_op(&op);
    let n = env.len();
    // Every single-bit flip, in the header, the ref, the nonce, the ciphertext or
    // the tag, is either malformed or inauthentic: never a different plaintext.
    for i in 0..n {
        for bit in [0x01u8, 0x80] {
            let mut t = env.clone();
            t[i] ^= bit;
            let got = k.open_op(&t);
            assert!(got.is_err(), "flip at byte {i} of {n} opened: {got:?}");
        }
    }
    // The tag (the last 16 bytes) and the ciphertext fail authentication.
    let mut t = env.clone();
    t[n - 1] ^= 1;
    assert_eq!(k.open_op(&t), Err(SealError::Unauthentic));
    let mut t = env.clone();
    t[n - 17] ^= 1;
    assert_eq!(k.open_op(&t), Err(SealError::Unauthentic));
    // Trailing bytes make it non-canonical.
    let mut t = env.clone();
    t.push(0);
    assert_eq!(k.open_op(&t), Err(SealError::Malformed));
}

#[test]
fn an_envelope_is_bound_to_its_household_app_ref_and_kind() {
    let k = keys();
    let op = b"signed op bytes".to_vec();
    let env = k.seal_op(&op);
    // Another household (another seed) cannot open it.
    let other = SealKeys::derive(&HouseholdRoot::from_seed([0xEE; 64]), APP);
    assert_eq!(other.open_op(&env), Err(SealError::Unauthentic));
    // Another app of the same household cannot either.
    let peckish = SealKeys::derive(&root(), "peckish");
    assert_eq!(peckish.open_op(&env), Err(SealError::Unauthentic));
    // An op envelope offered as a snapshot or a message is refused by kind.
    assert_eq!(k.open(SealKind::Snapshot, &env), Err(SealError::WrongKind));
    assert_eq!(k.open(SealKind::Msg, &env), Err(SealError::WrongKind));
    // Relabelling the kind byte to pass that check breaks the AAD instead.
    let snap = k.seal(SealKind::Snapshot, Some(&sha256(&op)), &op);
    let mut relabelled = snap.clone();
    let pos = relabelled.iter().position(|b| *b == 0x01).unwrap(); // version, then kind 1
    assert_eq!(relabelled[pos + 1], 0x01);
    relabelled[pos + 1] = 0x00;
    assert_eq!(k.open_op(&relabelled), Err(SealError::Unauthentic));
    // An op sealed under an id that is not its hash is refused after opening.
    let lying = k.seal(SealKind::Op, Some(&[7; 32]), &op);
    assert_eq!(k.open_op(&lying), Err(SealError::IdMismatch));
}
