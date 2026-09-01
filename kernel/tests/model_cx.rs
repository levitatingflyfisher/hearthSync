//! The eight counterexamples the Alloy model (`model/forget.als`) found against kernel
//! v0, one regression test each, numbered as in ADR 0006. Each was written against v0
//! first and watched fail for the reason the model gave.
//!
//! The convergence family (cx1–cx4) delivers raw op bytes with `ingest`, which is the
//! unguarded primitive: the fold must be a function of the delivered set whatever path
//! the ops took. The sync family (cx5–cx8) goes through `reconcile`.

mod common;
use common::*;

use hearth_sync_kernel::replica::Replica;
use hearth_sync_kernel::store::OpStore;
use hearth_sync_kernel::sync::{reconcile, ReviewItem};

const HORIZON: u64 = 90 * DAY;

/// Everyone syncs with everyone.
fn mesh(rs: &mut [&mut Replica], now: u64) {
    for i in 0..rs.len() {
        for j in i + 1..rs.len() {
            let (l, h) = rs.split_at_mut(j);
            reconcile(l[i], h[0], now).unwrap();
        }
    }
}

fn ml(r: &Replica, row: &str) -> Option<i64> {
    r.state().row("feeds", row).and_then(|f| match f.get("ml") {
        Some(hearth_sync_kernel::op::Value::Int(n)) => Some(*n),
        _ => None,
    })
}

/// cx1 (a′), compaction alone changes state. D0 forgets D2 before seeing D2's put P.
/// D1 checkpoints C over P without the Forget. D0 receives P and C, then compacts at
/// C. Compaction must not change what D0's fold is.
#[test]
fn cx1_compaction_alone_never_changes_state() {
    let (mut d0, mut d1, mut d2) = (joined(1, T0), joined(2, T0), joined(3, T0));
    mesh(&mut [&mut d0, &mut d1, &mut d2], T0 + 1);
    d0.forget(d2.device().unwrap(), T0 + 2).unwrap();
    let (_, p) = d2.put("feeds", "p", [("ml", v(1))], T0 + 3).unwrap();
    d1.ingest([&p], T0 + 4);
    let (c, cb) = d1.checkpoint(T0 + 5).unwrap();
    d0.ingest([&p, &cb], T0 + 6);
    let before = d0.state().encode();
    assert_eq!(d0.compact(T0 + 5 + HORIZON).unwrap(), Some(c));
    assert_eq!(d0.state().encode(), before, "cx1: compaction changed the state");
    assert_eq!(d0.fold_from_scratch().0.encode(), before, "cx1: after compaction a refold computes another state");
}

/// cx2 (a), divergence: D2 writes P, checkpoints C over it and prunes at C. D0, not
/// having seen P, forgets D2. Once both hold the same ops they must agree.
#[test]
fn cx2_forgotten_device_pruning_under_its_own_checkpoint_does_not_diverge() {
    let (mut d0, mut d1, mut d2) = (joined(1, T0), joined(2, T0), joined(3, T0));
    mesh(&mut [&mut d0, &mut d1, &mut d2], T0 + 1);
    let (_, f) = d0.forget(d2.device().unwrap(), T0 + 2).unwrap();
    let (_, p) = d2.put("feeds", "p", [("ml", v(1))], T0 + 3).unwrap();
    let (c, cb) = d2.checkpoint(T0 + 4).unwrap();
    d1.ingest([&p, &cb], T0 + 5); // D1 keeps the bodies D2 is about to prune
    let now = T0 + 4 + HORIZON;
    assert_eq!(d2.compact(now).unwrap(), Some(c));
    d0.ingest([&p, &cb], now);
    let rep = d2.ingest([&f], now);
    assert!(rep.wiped);
    assert_eq!(d0.store().ids(), d2.store().ids(), "same delivered set");
    assert_eq!(d0.state().encode(), d2.state().encode(), "cx2: same ops, different states");
}

/// cx3 (d), finality: D1 prunes P into its base as included; D0 knows everything D1
/// knows and excludes P. Nothing pruned in as included may be excluded by a replica
/// that knows more, and once D1 learns the Forget the two agree.
#[test]
fn cx3_an_op_pruned_in_is_never_excluded_by_a_replica_that_knows_more() {
    let (mut d0, mut d1, mut d2) = (joined(1, T0), joined(2, T0), joined(3, T0));
    mesh(&mut [&mut d0, &mut d1, &mut d2], T0 + 1);
    let (_, f) = d0.forget(d2.device().unwrap(), T0 + 2).unwrap();
    let (_, p) = d2.put("feeds", "p", [("ml", v(1))], T0 + 3).unwrap();
    let (c, cb) = d2.checkpoint(T0 + 4).unwrap();
    d1.ingest([&p, &cb], T0 + 5);
    let now = T0 + 4 + HORIZON;
    assert_eq!(d1.compact(now).unwrap(), Some(c));
    d0.ingest([&p, &cb], now);
    assert!(d1.store().ids().iter().all(|id| d0.store().contains(id)), "D0 knows at least what D1 knows");
    for id in d1.store().ids() {
        let pruned_in = d1.store().body(&id).is_none() && !d1.excluded().contains(&id);
        assert!(!(pruned_in && d0.excluded().contains(&id)), "cx3: D1 pruned in an op D0 excludes");
    }
    d1.ingest([&f], now);
    assert_eq!(d0.state().encode(), d1.state().encode(), "cx3: same ops, different states");
}

/// cx4 (b, Ruling 1 option A): D2 forgets D1, D1 writes P, D0 checkpoints over P and
/// prunes, then receives the Forget. A concurrent checkpoint shields its past from a
/// Forget it lacks, so every replica keeps P (the strict (b) fails by design).
#[test]
fn cx4_a_concurrent_checkpoint_shields_its_past_everywhere() {
    let (mut d0, mut d1, mut d2) = (joined(1, T0), joined(2, T0), joined(3, T0));
    mesh(&mut [&mut d0, &mut d1, &mut d2], T0 + 1);
    let (_, f) = d2.forget(d1.device().unwrap(), T0 + 2).unwrap();
    let (_, p) = d1.put("feeds", "p", [("ml", v(1))], T0 + 3).unwrap();
    d0.ingest([&p], T0 + 4);
    let (c, cb) = d0.checkpoint(T0 + 5).unwrap();
    let now = T0 + 5 + HORIZON;
    assert_eq!(d0.compact(now).unwrap(), Some(c));
    d0.ingest([&f], now);
    d2.ingest([&p, &cb], now);
    assert_eq!(d0.store().ids(), d2.store().ids(), "same delivered set");
    assert_eq!(d0.state().encode(), d2.state().encode(), "cx4: pruned and unpruned replicas disagree");
    assert_eq!(ml(&d2, "p"), Some(1), "cx4: option A keeps P behind the concurrent checkpoint");
}

/// cx5 (e1, ADR 0005 gap 1): D2 is away past the horizon and writes P. D1 checkpoints C
/// (now old) but never prunes, and writes P′ with an older clock than P. When D2
/// returns through the unpruned D1, P must not beat P′ silently: it goes to review.
#[test]
fn cx5_an_old_edit_through_an_unpruned_peer_goes_to_review() {
    let (mut d1, mut d2) = (joined(1, T0), joined(2, T0));
    d1.put("feeds", "r", [("ml", v(0))], T0 + 1).unwrap();
    reconcile(&mut d1, &mut d2, T0 + 2).unwrap();
    d1.checkpoint(T0 + 2 * DAY).unwrap();
    d1.put("feeds", "r", [("ml", v(1))], T0 + 120 * DAY).unwrap(); // P′
    let (p, _) = d2.put("feeds", "r", [("ml", v(2))], T0 + 150 * DAY).unwrap(); // P, newer
    let (_, to_d2) = reconcile(&mut d1, &mut d2, T0 + 200 * DAY).unwrap();
    let listed = to_d2
        .rebase
        .as_ref()
        .is_some_and(|r| r.review.iter().any(|i| matches!(i, ReviewItem::Field { op, .. } if *op == p)));
    assert!(listed, "cx5: P was not put on the review list: {to_d2:?}");
    assert_eq!(ml(&d1, "r"), Some(1), "cx5: P won silently over P′, which it never saw");
    assert_eq!(ml(&d2, "r"), Some(1));
}

/// cx6 (e2): D2 is away and writes P. D1's checkpoint C passes the horizon and D1
/// prunes. D2 adopts the snapshot and re-issues P (nobody had changed the field). D3,
/// which already held C, wrote P′ newer than P. The re-issue's fresh clock must not
/// let the old edit beat P′.
#[test]
fn cx6_a_reissued_edit_never_beats_a_newer_write_it_did_not_see() {
    let (mut d1, mut d2, mut d3) = (joined(1, T0), joined(2, T0), joined(3, T0));
    d1.put("feeds", "r", [("ml", v(0))], T0 + 1).unwrap();
    mesh(&mut [&mut d1, &mut d2, &mut d3], T0 + 2);
    let (c, _) = d1.checkpoint(T0 + 2 * DAY).unwrap();
    reconcile(&mut d1, &mut d3, T0 + 3 * DAY).unwrap();
    d2.put("feeds", "r", [("ml", v(2))], T0 + 10 * DAY).unwrap(); // P
    d3.put("feeds", "r", [("ml", v(3))], T0 + 50 * DAY).unwrap(); // P′
    assert_eq!(d1.compact(T0 + 100 * DAY).unwrap(), Some(c));
    let (_, to_d2) = reconcile(&mut d1, &mut d2, T0 + 160 * DAY).unwrap();
    assert_eq!(to_d2.rebase.as_ref().map(|r| r.reissued.len()), Some(1), "P re-issued: {to_d2:?}");
    reconcile(&mut d1, &mut d3, T0 + 170 * DAY).unwrap();
    reconcile(&mut d1, &mut d2, T0 + 171 * DAY).unwrap();
    for r in [&d1, &d2, &d3] {
        assert_eq!(ml(r, "r"), Some(3), "cx6: the re-issue of P beat P′, which is newer");
    }
}

/// cx7 (f): D2 forgets itself and wipes, then syncs with a pruned peer. v0 adopted the
/// snapshot and dropped its own Forget to review, with no keys to redo it, so no one
/// else ever learnt of it. A Forget, once written, must reach the household.
#[test]
fn cx7_a_self_forget_is_never_lost() {
    let (mut d1, mut d2) = (joined(1, T0), joined(2, T0));
    reconcile(&mut d1, &mut d2, T0 + 1).unwrap();
    d1.checkpoint(T0 + 2).unwrap();
    assert!(d1.compact(T0 + 100 * DAY).unwrap().is_some());
    d2.forget_self(T0 + 50 * DAY).unwrap();
    assert!(d2.is_wiped());
    reconcile(&mut d1, &mut d2, T0 + 101 * DAY).unwrap();
    assert!(d1.state().forgotten().contains(&d2.device().unwrap()), "cx7: the self-Forget never reached D1");
}

/// cx8 (g): D0's edit P was pruned into D0's base under D2's checkpoint. D1 checkpointed
/// concurrently without P and pruned. D0 lacks D1's base and adopts D1's snapshot; P
/// was neither an unsynced op with a body nor in the snapshot, so it vanished.
#[test]
fn cx8_adopting_a_concurrent_snapshot_never_loses_a_pruned_edit_silently() {
    let (mut d0, mut d1, mut d2) = (joined(1, T0), joined(2, T0), joined(3, T0));
    mesh(&mut [&mut d0, &mut d1, &mut d2], T0 + 1);
    d0.put("feeds", "p", [("ml", v(7))], T0 + 2).unwrap(); // P
    reconcile(&mut d0, &mut d2, T0 + 3).unwrap();
    let (c2, _) = d2.checkpoint(T0 + 4).unwrap();
    reconcile(&mut d0, &mut d2, T0 + 5).unwrap();
    let (c1, _) = d1.checkpoint(T0 + 4).unwrap(); // concurrent, without P
    let now = T0 + 100 * DAY;
    assert_eq!(d0.compact(now).unwrap(), Some(c2));
    assert_eq!(d1.compact(now).unwrap(), Some(c1));
    let (to_d0, _) = reconcile(&mut d0, &mut d1, now + 1).unwrap();
    let listed = to_d0.rebase.as_ref().is_some_and(|r| !r.review.is_empty() || !r.reissued.is_empty());
    assert!(listed, "cx8: P is neither re-issued nor listed: {to_d0:?}");
    assert_eq!(ml(&d1, "p"), Some(7), "cx8: P's value was lost silently");
    assert_eq!(ml(&d0, "p"), Some(7));
}
