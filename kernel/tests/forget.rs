//! "Forget this device" (Decision 2, ADR 0002): ops a forgotten device
//! writes after the cut leave the fold everywhere; the forgotten device wipes its
//! keys when it receives the Forget, and at once when it forgets itself.

mod common;
use common::*;

use hearth_sync_kernel::replica::KernelError;
use hearth_sync_kernel::store::OpStore;
use hearth_sync_kernel::sync::reconcile;

#[test]
fn post_cut_ops_are_excluded_everywhere_and_pre_cut_ops_stay() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    b.put("feeds", "before", [("ml", v(1))], T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    reconcile(&mut b, &mut c, T0 + 2).unwrap();
    // B, not yet told, keeps writing; A forgets B without having seen that write.
    let (late, late_bytes) = b.put("feeds", "after", [("ml", v(2))], T0 + 3).unwrap();
    a.forget(b.device().unwrap(), T0 + 4).unwrap();
    // C gets B's late op first, then the Forget from A.
    c.ingest([&late_bytes], T0 + 5);
    assert!(c.state().row("feeds", "after").is_some(), "not forgotten yet");
    let rep = reconcile(&mut a, &mut c, T0 + 6).unwrap();
    assert!(rep.1.ingest.excluded.contains(&late), "{:?}", rep.1.ingest);
    for r in [&a, &c] {
        assert!(r.state().row("feeds", "before").is_some(), "pre-cut op still counts");
        assert!(r.state().row("feeds", "after").is_none(), "post-cut op excluded");
        assert!(r.store().contains(&late), "excluded, not rejected: kept in the DAG");
    }
    assert_eq!(a.state().encode(), c.state().encode());
}

#[test]
fn honest_op_built_on_an_excluded_op_still_delivers() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    reconcile(&mut a, &mut c, T0 + 1).unwrap();
    let (forget_id, forget) = {
        // C forgets B now, before B writes more.
        let r = c.forget(b.device().unwrap(), T0 + 2).unwrap();
        (r.0, r.1)
    };
    let (d, d_bytes) = b.put("feeds", "x", [("ml", v(5))], T0 + 3).unwrap();
    a.ingest([&d_bytes], T0 + 4);
    // A builds on B's op (its head), before hearing of the Forget.
    let (honest, _) = a.put("feeds", "y", [("ml", v(6))], T0 + 5).unwrap();
    a.ingest([&forget], T0 + 6);
    reconcile(&mut a, &mut c, T0 + 7).unwrap();
    for r in [&a, &c] {
        assert!(r.store().contains(&forget_id));
        assert!(r.excluded().contains(&d));
        assert!(r.store().contains(&honest), "honest child delivered");
        assert!(r.state().row("feeds", "y").is_some());
        assert!(r.state().row("feeds", "x").is_none());
    }
}

#[test]
fn forgotten_device_wipes_its_keys_when_the_forget_arrives() {
    let (mut a, mut b) = (joined(1, T0), joined(2, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    a.forget(b.device().unwrap(), T0 + 2).unwrap();
    assert!(b.has_keys());
    let (_, to_b) = reconcile(&mut a, &mut b, T0 + 3).unwrap();
    assert!(to_b.ingest.wiped);
    assert!(b.is_wiped() && !b.has_keys());
    assert_eq!(b.put("feeds", "r", [("ml", v(1))], T0 + 4).unwrap_err(), KernelError::NoKeys);
    assert_eq!(b.forget(a.device().unwrap(), T0 + 4).unwrap_err(), KernelError::NoKeys);
    // Data is not recalled: B still shows what it had.
    assert_eq!(b.state().enrolled.len(), 2);
}

#[test]
fn forgetting_yourself_wipes_immediately() {
    let (mut a, mut b) = (joined(1, T0), joined(2, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    let (fid, bytes) = b.forget_self(T0 + 2).unwrap();
    assert!(b.is_wiped() && !b.has_keys(), "wiped before any sync");
    // The Forget still reaches the others: B can hand over its log without keys.
    reconcile(&mut a, &mut b, T0 + 3).unwrap();
    assert!(a.store().contains(&fid));
    assert!(a.state().forgotten().contains(&b.device().unwrap()));
    let _ = bytes;
}

#[test]
fn concurrent_forgets_of_each_other_converge() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    reconcile(&mut b, &mut c, T0 + 1).unwrap();
    reconcile(&mut a, &mut c, T0 + 1).unwrap();
    a.put("feeds", "a", [("ml", v(1))], T0 + 2).unwrap();
    b.put("feeds", "b", [("ml", v(1))], T0 + 2).unwrap();
    // A and B forget each other at the same time.
    a.forget(b.device().unwrap(), T0 + 3).unwrap();
    b.forget(a.device().unwrap(), T0 + 3).unwrap();
    reconcile(&mut a, &mut c, T0 + 4).unwrap();
    reconcile(&mut b, &mut c, T0 + 4).unwrap();
    reconcile(&mut a, &mut c, T0 + 5).unwrap();
    // Both got wiped; the Forgets themselves are never excluded, so both apply.
    assert!(a.is_wiped() && b.is_wiped());
    assert_eq!(a.state().encode(), c.state().encode());
    assert_eq!(b.state().encode(), c.state().encode());
    assert_eq!(c.state().forgotten().len(), 2);
}

/// Forgotten while away past the horizon: the Forget arrives with a snapshot, not as
/// an op the device ingests, and the report must still say the device wiped (the app
/// deletes the stored words on that signal). WipedPush (ADR 0006): the device does
/// not adopt the snapshot; it wipes, keeps its log and hands its own ops over.
#[test]
fn forgotten_while_away_past_the_horizon_reports_the_wipe() {
    let (mut a, mut b) = (joined(1, T0), joined(2, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    b.put("feeds", "offline", [("ml", v(1))], T0 + 2 * DAY).unwrap();
    a.forget(b.device().unwrap(), T0 + 3 * DAY).unwrap();
    a.checkpoint(T0 + 4 * DAY).unwrap();
    a.compact(T0 + 100 * DAY).unwrap().unwrap();
    let (_, to_b) = reconcile(&mut a, &mut b, T0 + 101 * DAY).unwrap();
    assert!(to_b.rebase.is_none(), "a snapshot that forgets the device is never adopted");
    assert!(to_b.ingest.wiped, "{:?}", to_b.ingest);
    assert!(b.is_wiped() && !b.has_keys());
    assert!(a.state().row("feeds", "offline").is_none());
}

/// The two-step path the bridge will use must refuse after a wipe as well.
#[test]
fn a_wiped_replica_cannot_prepare_or_finish() {
    use hearth_sync_kernel::keys::{DeviceSigner, SoftSigner};
    use hearth_sync_kernel::op::Body;
    let mut b = joined(2, T0);
    let body = Body::Append { stream: "votes".into(), record: v(1) };
    let u = b.prepare(body.clone(), T0 + 1).unwrap();
    b.forget_self(T0 + 2).unwrap();
    assert_eq!(b.prepare(body, T0 + 3).unwrap_err(), KernelError::NoKeys);
    // A signature obtained before the wipe (e.g. from a platform keystore) is refused too.
    let sig = SoftSigner::from_secret([2; 32]).sign(&u.signable_bytes());
    assert_eq!(b.finish(u, sig, T0 + 3).unwrap_err(), KernelError::NoKeys);
}

/// Reforget (ADR 0006): a device that forgot another while away past the horizon
/// adopts the household's snapshot on return. Its Forget is not in the snapshot, so
/// it re-authors it on the new heads with a fresh cut instead of listing it.
#[test]
fn a_forget_written_while_away_is_reauthored_after_the_snapshot() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    reconcile(&mut a, &mut c, T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    a.checkpoint(T0 + 2).unwrap();
    assert!(a.compact(T0 + 100 * DAY).unwrap().is_some());
    let (old, _) = c.forget(b.device().unwrap(), T0 + 50 * DAY).unwrap();
    let (to_a, to_c) = reconcile(&mut a, &mut c, T0 + 101 * DAY).unwrap();
    let rb = to_c.rebase.expect("C took the snapshot path");
    assert!(rb.reissued.contains_key(&old), "{rb:?}");
    assert!(to_a.rebase.is_none());
    assert!(a.state().forgotten().contains(&b.device().unwrap()));
    reconcile(&mut a, &mut b, T0 + 102 * DAY).unwrap();
    assert!(b.is_wiped());
}

/// A adopted a snapshot whose Forget is a kept body, not part of the base state.
/// The Forget's cut must still apply to ops delivered afterwards (here, the
/// forgotten device's handover).
#[test]
fn a_forget_kept_as_a_body_in_a_snapshot_still_excludes_later_ops() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    b.forget(c.device().unwrap(), T0 + 1).unwrap();
    let (p, _) = c.put("feeds", "late", [("ml", v(1))], T0 + 2).unwrap();
    b.checkpoint(T0 + 3).unwrap();
    let now = T0 + 100 * DAY;
    let (to_a, _) = reconcile(&mut a, &mut b, now).unwrap();
    assert!(to_a.rebase.is_some(), "A adopted B's snapshot");
    let (_, to_c) = reconcile(&mut a, &mut c, now + 1).unwrap();
    assert!(to_c.ingest.wiped && a.store().contains(&p), "C wiped and handed P over");
    assert!(a.excluded().contains(&p));
    assert!(a.state().row("feeds", "late").is_none());
}
