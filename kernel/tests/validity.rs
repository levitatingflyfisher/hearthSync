//! Validity rules V1–V5 and V8 against hand-built hostile ops.

mod common;
use common::*;

use hearth_sync_kernel::keys::{self, DeviceSigner, HouseholdRoot, SoftSigner};
use hearth_sync_kernel::op::{Body, Hlc, Reject, Unsigned, Value};
use hearth_sync_kernel::replica::KernelError;
use hearth_sync_kernel::store::OpStore;
use hearth_sync_kernel::{sha256, MAX_FUTURE_SKEW_MS, MAX_OP_BYTES};

fn signed(signer: &SoftSigner, parents: Vec<[u8; 32]>, hlc: Hlc, body: Body) -> Vec<u8> {
    let u = Unsigned { app: APP.into(), device: signer.device(), parents, hlc, body };
    let sig = signer.sign(&u.signable_bytes());
    u.with_sig(sig).encode()
}

fn put(field: &str, n: i64) -> Body {
    Body::Put {
        table: "feeds".into(),
        row: "r1".into(),
        fields: [(field.to_string(), Value::Int(n))].into(),
        origin: None,
    }
}

fn reason(r: &hearth_sync_kernel::replica::IngestReport, bytes: &[u8]) -> Option<Reject> {
    r.rejected.iter().find(|(i, _)| *i == sha256(bytes)).map(|(_, why)| *why)
}

#[test]
fn forged_signature_is_rejected() {
    let mut a = joined(1, T0);
    let (id, _) = a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    let mallory = SoftSigner::from_secret([9; 32]);
    // Claims to be device 1 but is signed by another key.
    let u = Unsigned {
        app: APP.into(),
        device: a.device().unwrap(),
        parents: vec![id],
        hlc: Hlc::new(T0 + 2, 0),
        body: put("ml", 666),
    };
    let bytes = u.clone().with_sig(mallory.sign(&u.signable_bytes())).encode();
    let rep = a.ingest([&bytes], T0 + 3);
    assert_eq!(reason(&rep, &bytes), Some(Reject::BadSignature));
    assert_eq!(a.state().row("feeds", "r1").unwrap()["ml"], v(1));
}

#[test]
fn unenrolled_device_is_rejected_and_its_descendants_never_deliver() {
    let mut a = joined(1, T0);
    let (id, _) = a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    let stranger = SoftSigner::from_secret([7; 32]);
    let s1 = signed(&stranger, vec![id], Hlc::new(T0 + 2, 0), put("ml", 2));
    let s2 = signed(&stranger, vec![sha256(&s1)], Hlc::new(T0 + 3, 0), put("ml", 3));
    // Child first: it waits, then is rejected when its parent is.
    let rep = a.ingest([&s2, &s1], T0 + 4);
    assert_eq!(reason(&rep, &s1), Some(Reject::NotEnrolled));
    assert_eq!(reason(&rep, &s2), Some(Reject::ParentRejected));
    assert_eq!(a.pending_count(), 0);
    assert_eq!(a.state().row("feeds", "r1").unwrap()["ml"], v(1));
}

#[test]
fn enroll_signed_by_another_household_is_rejected() {
    let mut a = joined(1, T0);
    let stranger = SoftSigner::from_secret([7; 32]);
    let other = HouseholdRoot::from_seed([0xEE; 64]);
    let auth = keys::sign_enroll(&other, APP, &stranger.device(), "sneaky");
    let bytes = signed(
        &stranger,
        a.heads(),
        Hlc::new(T0 + 5, 0),
        Body::Enroll { device: stranger.device(), label: "sneaky".into(), auth },
    );
    let rep = a.ingest([&bytes], T0 + 6);
    assert_eq!(reason(&rep, &bytes), Some(Reject::BadEnrollAuth));
    assert!(!a.state().is_enrolled(&stranger.device()));
}

#[test]
fn clock_must_move_past_parents() {
    let mut a = joined(1, T0);
    let (id, _) = a.put("feeds", "r1", [("ml", v(1))], T0 + 10).unwrap();
    let signer = SoftSigner::from_secret([1; 32]);
    let same = signed(&signer, vec![id], Hlc::new(T0 + 10, 0), put("ml", 2));
    let earlier = signed(&signer, vec![id], Hlc::new(T0 + 5, 9), put("ml", 3));
    let rep = a.ingest([&same, &earlier], T0 + 20);
    assert_eq!(reason(&rep, &same), Some(Reject::ClockNotAfterParents));
    assert_eq!(reason(&rep, &earlier), Some(Reject::ClockNotAfterParents));
}

#[test]
fn delete_must_observe_puts_from_its_own_past_on_the_same_row() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    let (pa, _) = a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    let (pb, bb) = b.put("feeds", "r1", [("ml", v(2))], T0 + 1).unwrap();
    let (p_other_row, _) = a.put("feeds", "r2", [("ml", v(3))], T0 + 2).unwrap();
    let signer = SoftSigner::from_secret([1; 32]);
    // pb is not in before(u): A never saw it when "deleting".
    let blind = signed(
        &signer,
        a.heads(),
        Hlc::new(T0 + 3, 0),
        Body::Delete { table: "feeds".into(), row: "r1".into(), observed: vec![pa, pb] },
    );
    let wrong_row = signed(
        &signer,
        a.heads(),
        Hlc::new(T0 + 3, 1),
        Body::Delete { table: "feeds".into(), row: "r1".into(), observed: vec![p_other_row] },
    );
    a.ingest([&bb], T0 + 3);
    let rep = a.ingest([&blind, &wrong_row], T0 + 4);
    assert_eq!(reason(&rep, &blind), Some(Reject::ObservedNotInPast));
    assert_eq!(reason(&rep, &wrong_row), Some(Reject::ObservedNotInPast));
}

#[test]
fn non_canonical_and_oversize_bytes_are_rejected() {
    let mut a = joined(1, T0);
    let (_, bytes) = a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    let mut b = observer();
    // Trailing byte.
    let mut trailing = bytes.clone();
    trailing.push(0);
    // Oversize: a single text value past MAX_OP_BYTES.
    let big = a.put("feeds", "r2", [("note", Value::Text("x".repeat(MAX_OP_BYTES)))], T0 + 2);
    assert!(matches!(big, Err(KernelError::Rejected(Reject::TooLarge))), "{big:?}");
    let rep = b.ingest([&trailing], T0 + 3);
    assert_eq!(reason(&rep, &trailing), Some(Reject::NotCanonical));
}

#[test]
fn dangling_parent_never_delivers() {
    let mut a = joined(1, T0);
    let signer = SoftSigner::from_secret([1; 32]);
    let nowhere = [0xAB; 32];
    let bytes = signed(&signer, vec![nowhere], Hlc::new(T0 + 5, 0), put("ml", 1));
    let rep = a.ingest([&bytes], T0 + 6);
    assert!(rep.delivered.is_empty() && rep.rejected.is_empty());
    assert_eq!(a.pending_count(), 1);
    assert!(a.state().row("feeds", "r1").is_none());
}

#[test]
fn future_clock_is_quarantined_until_local_time_catches_up() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    hearth_sync_kernel::sync::reconcile(&mut a, &mut b, T0).unwrap();
    let hour = 60 * 60 * 1000;
    // B's clock runs an hour fast.
    let (id, bytes) = b.put("feeds", "r1", [("ml", v(9))], T0 + hour).unwrap();
    let rep = a.ingest([&bytes], T0 + 1000);
    assert_eq!(rep.quarantined, vec![id]);
    assert!(a.state().row("feeds", "r1").is_none());
    // Still too early just inside the bound's edge.
    let rep = a.ingest(Vec::<Vec<u8>>::new(), T0 + hour - MAX_FUTURE_SKEW_MS - 1);
    assert!(rep.delivered.is_empty());
    let rep = a.ingest(Vec::<Vec<u8>>::new(), T0 + hour - MAX_FUTURE_SKEW_MS);
    assert_eq!(rep.delivered, vec![id]);
    assert_eq!(a.state().row("feeds", "r1").unwrap()["ml"], v(9));
}

#[test]
fn a_clock_far_behind_the_log_refuses_to_stamp() {
    let mut a = joined(1, T0 + 60 * 60 * 1000);
    let err = a.put("feeds", "r1", [("ml", v(1))], T0).unwrap_err();
    assert!(matches!(err, KernelError::ClockBehind { .. }), "{err:?}");
    // Within the bound it stamps just after the log instead.
    assert!(a.put("feeds", "r1", [("ml", v(1))], T0 + 60 * 60 * 1000 - MAX_FUTURE_SKEW_MS).is_ok());
}

#[test]
fn duplicates_are_idempotent() {
    let mut a = joined(1, T0);
    a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    let mut o = observer();
    o.ingest(a.log(), T0 + 2);
    let before = o.state().encode();
    let rep = o.ingest(a.log().into_iter().chain(a.log()), T0 + 3);
    assert_eq!(rep.duplicates, 2 * a.log().len());
    assert_eq!(o.state().encode(), before);
    assert_eq!(o.store().len(), a.store().len());
}

/// V5 walks back from the parents only as far as the oldest observed id. Guard: an
/// observed put far back along a long chain of another device's ops is found, and a
/// put that is older by the clock but not an ancestor is still refused.
#[test]
fn observed_ids_are_found_deep_in_the_past_and_refused_when_only_older() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    let (p, _) = a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    // B writes a put to r1 that A never sees: older than everything A writes next.
    let (q, _) = b.put("feeds", "r1", [("ml", v(7))], T0 + 2).unwrap();
    for i in 0..200 {
        a.put("feeds", &format!("x{i}"), [("ml", v(i))], T0 + 10 + i as u64).unwrap();
    }
    // A deletes r1 observing P (deep in its past): valid everywhere.
    let (_, del) = a.delete("feeds", "r1", T0 + 300).unwrap();
    let mut o = observer();
    let rep = o.ingest(a.log().iter().chain(b.log().iter()), T0 + 301);
    assert!(rep.rejected.is_empty(), "{rep:?}");
    assert!(o.store().contains(&sha256(&del)));
    // A delete by A that claims to have observed B's Q, which A never delivered.
    let heads = a.heads();
    let signer = SoftSigner::from_secret([1; 32]);
    let forged = signed(
        &signer,
        heads,
        Hlc::new(T0 + 400, 0),
        Body::Delete { table: "feeds".into(), row: "r1".into(), observed: vec![p.min(q), p.max(q)] },
    );
    let rep = o.ingest([&forged], T0 + 401);
    assert_eq!(reason(&rep, &forged), Some(Reject::ObservedNotInPast));
}
