//! Checkpoints, the horizon, snapshots, and the returning past-horizon phone (Q3, Q4).

mod common;
use common::*;

use hearth_sync_kernel::replica::{Config, KernelError};
use hearth_sync_kernel::store::OpStore;
use hearth_sync_kernel::sync::{reconcile, Offer, PrunedContent, ReviewItem};
use hearth_sync_kernel::DEFAULT_HORIZON_MS;

#[test]
fn default_horizon_is_ninety_days() {
    assert_eq!(DEFAULT_HORIZON_MS, 90 * DAY);
    assert_eq!(Config::default().horizon_ms, Some(90 * DAY));
    assert_eq!(Config::keep_full_history().horizon_ms, None);
}

/// A's edits are pruned behind B's checkpoint. B's own ops stay bodies (KeepBodies,
/// ADR 0006): a later Forget of B could still exclude them, and a pruned base can
/// never un-fold an op.
#[test]
fn compaction_prunes_bodies_behind_an_old_checkpoint_and_keeps_the_state() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    a.delete("feeds", "r1", T0 + 2).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    let (cp, _) = b.checkpoint(T0 + 3).unwrap();
    reconcile(&mut a, &mut b, T0 + 3).unwrap();
    a.put("feeds", "r2", [("ml", v(2))], T0 + 4).unwrap();
    let before = a.state().encode();
    // Not old enough yet.
    assert_eq!(a.compact(T0 + 3 + 90 * DAY - 1).unwrap(), None);
    assert_eq!(a.compact(T0 + 3 + 90 * DAY).unwrap(), Some(cp));
    assert_eq!(a.state().encode(), before);
    assert_eq!(a.log().len(), 3, "checkpoint, B's enroll (kept) and r2 keep bodies; A's older ops are pruned");
    assert_eq!(a.store().len(), 6, "index entries stay");
    // Undo still works past the horizon: the tombstone kept the content.
    a.restore("feeds", "r1", T0 + 91 * DAY).unwrap();
    assert_eq!(a.state().row("feeds", "r1").unwrap()["ml"], v(1));
}

/// A device compacting at its own checkpoint keeps its own ops as bodies: until
/// another device's checkpoint covers them, a Forget of this device could still
/// exclude them (KeepBodies, ADR 0006). A one-device household never prunes.
#[test]
fn a_devices_own_checkpoint_keeps_its_own_ops() {
    let mut a = joined(1, T0);
    a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    let (cp, _) = a.checkpoint(T0 + 3).unwrap();
    assert_eq!(a.compact(T0 + 3 + 90 * DAY).unwrap(), Some(cp));
    assert_eq!(a.log().len(), 3, "enroll, put and checkpoint all kept");
}

#[test]
fn keep_full_history_never_prunes() {
    let mut a = joined_with(1, T0, Config::keep_full_history());
    a.put("forecasts", "f1", [("p", v(70))], T0 + 1).unwrap();
    a.checkpoint(T0 + 2).unwrap();
    assert_eq!(a.compact(T0 + 10 * 365 * DAY).unwrap(), None);
    assert_eq!(a.log().len(), 3);
}

#[test]
fn a_new_device_syncs_from_a_snapshot() {
    let mut a = joined(1, T0);
    a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    a.checkpoint(T0 + 2).unwrap();
    a.compact(T0 + 2 + 90 * DAY).unwrap().unwrap();
    a.put("feeds", "r2", [("ml", v(2))], T0 + 91 * DAY).unwrap();
    let mut n = joined(5, T0 + 92 * DAY);
    let (to_n, _) = reconcile(&mut n, &mut a, T0 + 92 * DAY).unwrap();
    assert!(to_n.rebase.is_some(), "took the snapshot path");
    assert!(n.state().row("feeds", "r1").is_some() && n.state().row("feeds", "r2").is_some());
    assert_eq!(n.state().encode(), a.state().encode());
    assert!(n.state().is_enrolled(&n.device().unwrap()), "its own enroll was re-issued");
}

#[test]
fn tampered_snapshot_is_refused() {
    let mut a = joined(1, T0);
    a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    a.checkpoint(T0 + 2).unwrap();
    a.compact(T0 + 2 + 90 * DAY).unwrap().unwrap();
    let mut snap = a.snapshot().unwrap();
    let n = snap.state.len();
    snap.state[n - 1] ^= 1;
    let mut o = observer();
    let err = o.accept(Offer::Snapshot { snapshot: snap, ops: vec![] }, T0 + 91 * DAY).unwrap_err();
    assert!(matches!(err, KernelError::BadSnapshot(_)), "{err:?}");
    assert!(o.store().is_empty(), "nothing adopted");
}

#[test]
fn checkpoint_with_a_wrong_hash_is_not_compacted_on() {
    use hearth_sync_kernel::op::Body;
    let mut a = joined(1, T0);
    a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    a.author(Body::Checkpoint { state_hash: [0; 32] }, T0 + 2).unwrap();
    assert_eq!(a.compact(T0 + 2 + 90 * DAY).unwrap_err(), KernelError::CheckpointMismatch);
    assert_eq!(a.log().len(), 3, "nothing pruned");
}

#[test]
fn snapshot_round_trips_through_bytes() {
    use hearth_sync_kernel::sync::Snapshot;
    let mut a = joined(1, T0);
    a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    a.checkpoint(T0 + 2).unwrap();
    a.compact(T0 + 2 + 90 * DAY).unwrap().unwrap();
    let s = a.snapshot().unwrap();
    assert_eq!(Snapshot::decode(&s.encode()).unwrap(), s);
}

/// Q4: B goes offline past the horizon. On return it adopts A's snapshot and
/// re-issues its edit to a field nobody else touched; its edit to a field A changed
/// in the meantime goes on the review list, so an old edit never wins silently.
#[test]
fn returning_phone_past_the_horizon_reapplies_untouched_fields_and_lists_the_rest() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    a.put("feeds", "r1", [("ml", v(100)), ("note", t("left")), ("side", t("L"))], T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();

    // B goes offline and edits two fields.
    b.put("feeds", "r1", [("ml", v(111))], T0 + 3 * DAY).unwrap();
    b.put("feeds", "r1", [("note", t("right"))], T0 + 4 * DAY).unwrap();
    b.put("feeds", "r1", [("side", t("R"))], T0 + 5 * DAY).unwrap();
    b.set_add("groceries", t("eggs"), T0 + 5 * DAY).unwrap();
    b.delete("feeds", "gone", T0 + 5 * DAY).unwrap();
    // Meanwhile A changes `note` and `side`, checkpoints, and time passes.
    a.put("feeds", "r1", [("note", t("both"))], T0 + 10 * DAY).unwrap();
    a.put("feeds", "r1", [("side", t("M"))], T0 + 10 * DAY + 1).unwrap();
    a.checkpoint(T0 + 11 * DAY).unwrap();
    assert!(a.compact(T0 + 102 * DAY).unwrap().is_some());

    let now = T0 + 200 * DAY;
    let (to_a, to_b) = reconcile(&mut a, &mut b, now).unwrap();
    let rb = to_b.rebase.expect("B took the snapshot path");
    assert!(to_a.rebase.is_none());

    // `ml` was untouched on A since B's last sync: re-issued, and it wins now.
    // `note` and `side` were changed by A: listed, A's values stand.
    for r in [&a, &b] {
        let row = r.state().row("feeds", "r1").unwrap();
        assert_eq!(row["ml"], v(111));
        assert_eq!(row["note"], t("both"));
        assert_eq!(row["side"], t("M"));
        assert!(r.state().set("groceries").contains(&t("eggs")));
    }
    let fields: Vec<&str> = rb
        .review
        .iter()
        .filter_map(|i| match i {
            ReviewItem::Field { field, .. } => Some(field.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(fields, vec!["note", "side"]);
    assert!(rb.review.iter().any(
        |i| matches!(i, ReviewItem::Field { mine, current, .. } if *mine == t("right") && *current == Some(t("both")))
    ));
    assert!(rb.review.iter().any(|i| matches!(i, ReviewItem::Op { .. })), "the delete is listed, not replayed");
    assert_eq!(a.state().encode(), b.state().encode());
    assert_eq!(a.heads(), b.heads());
}

#[test]
fn offline_less_than_the_horizon_merges_normally() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    a.put("feeds", "r1", [("ml", v(100))], T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    b.put("feeds", "r1", [("ml", v(111))], T0 + 3 * DAY).unwrap();
    a.checkpoint(T0 + 4 * DAY).unwrap();
    assert!(a.compact(T0 + 50 * DAY).unwrap().is_none());
    let (_, to_b) = reconcile(&mut a, &mut b, T0 + 50 * DAY).unwrap();
    assert!(to_b.rebase.is_none());
    assert_eq!(a.state().row("feeds", "r1").unwrap()["ml"], v(111));
}

/// The snapshot's index carries enrolments and targets that V4/V5 trust later, so
/// the checkpoint must commit to it: a forged `enrolls` must not admit a stranger.
#[test]
fn tampered_snapshot_index_is_refused() {
    use hearth_sync_kernel::keys::{DeviceSigner, SoftSigner};
    let mut a = joined(1, T0);
    a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    a.checkpoint(T0 + 2).unwrap();
    a.compact(T0 + 2 + 90 * DAY).unwrap().unwrap();
    let stranger = SoftSigner::from_secret([0x5A; 32]).device();
    let mut snap = a.snapshot().unwrap();
    let entry = snap.index.iter_mut().find(|(_, e)| e.enrolls.is_some()).expect("A's enroll is indexed");
    entry.1.enrolls = Some(stranger);
    entry.1.device = stranger;
    let mut o = observer();
    let err = o.accept(Offer::Snapshot { snapshot: snap, ops: vec![] }, T0 + 91 * DAY).unwrap_err();
    assert!(matches!(err, KernelError::BadSnapshot(_)), "{err:?}");
    assert!(o.store().is_empty(), "nothing adopted");

    // Any single changed index field is refused too.
    let mut snap = a.snapshot().unwrap();
    snap.index[0].1.hlc.counter += 1;
    let mut o = observer();
    assert!(o.accept(Offer::Snapshot { snapshot: snap, ops: vec![] }, T0 + 91 * DAY).is_err());
}

/// The kept bodies are part of what the checkpoint's hash checks: a snapshot that
/// drops one, or smuggles in an op its index does not name, is refused.
#[test]
fn snapshot_kept_bodies_are_verified() {
    let mut a = joined(1, T0);
    a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    a.checkpoint(T0 + 2).unwrap();
    a.compact(T0 + 2 + 90 * DAY).unwrap().unwrap();
    let good = a.snapshot().unwrap();
    assert!(!good.kept.is_empty(), "A's own ops are kept");
    let mut dropped = good.clone();
    dropped.kept.pop();
    assert!(observer().accept(Offer::Snapshot { snapshot: dropped, ops: vec![] }, T0 + 91 * DAY).is_err());
    let mut b = joined(2, T0);
    let (_, stranger) = b.put("feeds", "x", [("ml", v(9))], T0 + 3).unwrap();
    let mut smuggled = good.clone();
    smuggled.kept.push(stranger);
    assert!(observer().accept(Offer::Snapshot { snapshot: smuggled, ops: vec![] }, T0 + 91 * DAY).is_err());
    let mut o = observer();
    o.accept(Offer::Snapshot { snapshot: good, ops: vec![] }, T0 + 91 * DAY).unwrap();
    assert_eq!(o.state().encode(), a.state().encode());
}

/// ForeignReview (ADR 0006): an op by another device that only the returning phone
/// held is listed on adoption, not pushed on as it is: it may be a late edit too, and
/// its author rebases it when it returns itself.
#[test]
fn a_foreign_op_only_the_returning_phone_held_is_listed_not_pushed() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    reconcile(&mut a, &mut c, T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    a.checkpoint(T0 + 2).unwrap();
    let (x, _) = c.put("feeds", "x", [("ml", v(5))], T0 + 10 * DAY).unwrap();
    reconcile(&mut b, &mut c, T0 + 11 * DAY).unwrap();
    assert!(a.compact(T0 + 100 * DAY).unwrap().is_some());
    let (_, to_b) = reconcile(&mut a, &mut b, T0 + 101 * DAY).unwrap();
    let review = to_b.rebase.expect("snapshot path").review;
    assert!(review.iter().any(|i| matches!(i, ReviewItem::Foreign { op, .. } if *op == x)), "{review:?}");
    assert!(!a.store().contains(&x), "not pushed on as it is");
    assert_eq!(a.state().encode(), b.state().encode());
}

/// No stall (v0.2): B holds A's base but lacks an old checkpoint concurrent with it,
/// which A cannot build a snapshot at. A falls back to a snapshot at its own base,
/// which B already holds; B adopts it, so its edit meets the old checkpoint only
/// through the rebase, and the two converge.
#[test]
fn an_old_checkpoint_no_snapshot_covers_falls_back_to_the_providers_base() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    reconcile(&mut a, &mut c, T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    a.checkpoint(T0 + 2).unwrap();
    let (c2_id, c2) = c.checkpoint(T0 + 2).unwrap();
    reconcile(&mut a, &mut b, T0 + 3).unwrap();
    let (e, _) = b.put("feeds", "b", [("ml", v(8))], T0 + 50 * DAY).unwrap();
    assert!(a.compact(T0 + 100 * DAY).unwrap().is_some());
    a.ingest([&c2], T0 + 100 * DAY);
    let (to_b, _) = reconcile(&mut b, &mut a, T0 + 100 * DAY).expect("no stall");
    let rb = to_b.rebase.expect("B adopted a snapshot at A's base");
    assert!(rb.reissued.contains_key(&e), "B's edit went through the rebase: {rb:?}");
    assert!(b.store().contains(&c2_id));
    assert_eq!(a.state().encode(), b.state().encode());
    assert_eq!(a.state().row("feeds", "b").unwrap()["ml"], v(8));
}

/// The checkpoint hash commits to which ops a snapshot keeps as bodies and to the
/// base state itself (v0.2), so a provider cannot fold a kept op into the base,
/// where a later Forget could no longer exclude it.
#[test]
fn a_dishonest_provider_cannot_fold_a_kept_op_into_the_base() {
    use hearth_sync_kernel::op;
    use hearth_sync_kernel::state::State;
    let (mut a, mut b) = (joined(1, T0), joined(2, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    let (q, qb) = b.put("feeds", "q", [("ml", v(3))], T0 + 2).unwrap();
    b.checkpoint(T0 + 3).unwrap();
    let now = T0 + 3 + 90 * DAY;
    b.compact(now).unwrap().unwrap();
    let honest = b.snapshot().unwrap();
    assert!(honest.kept.contains(&qb), "only B's own checkpoint covers Q, so Q is kept");
    let mut folded = State::decode(&honest.state).unwrap();
    folded.apply(q, &op::decode(&qb).unwrap());
    let mut dropped = honest.clone();
    dropped.state = folded.encode();
    dropped.kept.retain(|k| *k != qb);
    let err = observer().accept(Offer::Snapshot { snapshot: dropped, ops: vec![] }, now);
    assert!(err.is_err(), "Q folded into the base, body dropped: accepted");
    let mut smuggled = honest.clone();
    smuggled.state = folded.encode();
    let err = observer().accept(Offer::Snapshot { snapshot: smuggled, ops: vec![] }, now);
    assert!(err.is_err(), "Q folded into the base, body still sent: accepted");
    let mut o = observer();
    o.accept(Offer::Snapshot { snapshot: honest, ops: vec![] }, now).unwrap();
    assert_eq!(o.state().encode(), b.state().encode());
}

/// A device's own ops are pruned once another device's checkpoint covers them, even
/// when it compacts at a later checkpoint of its own (v0.2 backing rule).
#[test]
fn own_ops_behind_another_devices_checkpoint_are_pruned_under_your_own() {
    let (mut a, mut b) = (joined(1, T0), joined(2, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    let (p, _) = a.put("feeds", "p", [("ml", v(1))], T0 + 2).unwrap();
    reconcile(&mut a, &mut b, T0 + 3).unwrap();
    b.checkpoint(T0 + 4).unwrap();
    reconcile(&mut a, &mut b, T0 + 5).unwrap();
    let (ca, _) = a.checkpoint(T0 + 6).unwrap();
    assert_eq!(a.compact(T0 + 6 + 90 * DAY).unwrap(), Some(ca));
    assert!(a.store().body(&p).is_none(), "B's checkpoint backs P, so A may prune it");
    assert_eq!(a.fold_from_scratch().0, *a.state());
}

/// BaseRebase (ADR 0006), another device's edit: C's edit was folded into A's base
/// under A's own checkpoint; B checkpointed concurrently without it. When A adopts
/// B's snapshot the value is listed, not lost silently.
#[test]
fn another_devices_edit_lost_from_the_base_on_adoption_is_listed() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    reconcile(&mut a, &mut c, T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    let (p, _) = c.put("feeds", "p", [("ml", v(4))], T0 + 2).unwrap();
    reconcile(&mut a, &mut c, T0 + 3).unwrap();
    a.checkpoint(T0 + 4).unwrap();
    b.checkpoint(T0 + 4).unwrap();
    let now = T0 + 100 * DAY;
    assert!(a.compact(now).unwrap().is_some());
    assert!(b.compact(now).unwrap().is_some());
    let (to_a, _) = reconcile(&mut a, &mut b, now + 1).unwrap();
    let review = to_a.rebase.expect("A took B's snapshot").review;
    assert!(
        review.iter().any(|i| matches!(i, ReviewItem::Lost { op, value, .. } if *op == p && *value == v(4))),
        "{review:?}"
    );
}

/// A's enrolment and edit were folded into its base under B's checkpoint; C's
/// concurrent checkpoint knows neither. Adopting C's snapshot, A must enrol itself
/// again before re-issuing anything (V4), since the enrolment has no body to rebase.
#[test]
fn adopting_a_snapshot_that_lacks_your_enrolment_re_enrols_you() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    a.put("feeds", "p", [("ml", v(6))], T0 + 1).unwrap();
    c.checkpoint(T0 + 2).unwrap();
    reconcile(&mut a, &mut b, T0 + 3).unwrap();
    b.checkpoint(T0 + 4).unwrap();
    let now = T0 + 100 * DAY;
    reconcile(&mut a, &mut b, now).unwrap();
    assert!(a.base_checkpoint().is_some(), "A adopted B's snapshot");
    let (to_a, _) = reconcile(&mut a, &mut c, now + 1).unwrap();
    assert!(to_a.rebase.is_some(), "A adopted C's snapshot");
    assert!(a.state().is_enrolled(&a.device().unwrap()));
    assert_eq!(c.state().row("feeds", "p").unwrap()["ml"], v(6), "A's edit re-issued");
}

/// Compacting at one of your own checkpoints and then at a later one: the later
/// checkpoint's committed split must not depend on whether the earlier checkpoint
/// is a body or already behind the base (checkpoints are never kept bodies).
#[test]
fn compacting_again_at_a_later_own_checkpoint_verifies() {
    let mut a = joined(1, T0);
    let (c1, _) = a.checkpoint(T0 + 1).unwrap();
    let (c2, _) = a.checkpoint(T0 + 2).unwrap();
    assert_eq!(a.compact(T0 + 1 + 90 * DAY).unwrap(), Some(c1));
    assert_eq!(a.compact(T0 + 2 + 90 * DAY).unwrap(), Some(c2));
}

/// BaseRebase for every kind (v0.2): A's own set add, append and delete were pruned
/// into A's base under C's checkpoint; B checkpointed concurrently without them. On
/// adopting B's snapshot, A re-issues the set add and the append and lists the delete.
#[test]
fn own_set_adds_appends_and_deletes_lost_from_the_base_are_reissued_or_listed() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    b.put("feeds", "r", [("ml", v(1))], T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    reconcile(&mut a, &mut c, T0 + 2).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    let (add, _) = a.set_add("groceries", t("eggs"), T0 + 3).unwrap();
    let (rec, _) = a.append("votes", t("yes"), T0 + 4).unwrap();
    let (del, _) = a.delete("feeds", "r", T0 + 5).unwrap();
    reconcile(&mut a, &mut c, T0 + 6).unwrap();
    c.checkpoint(T0 + 7).unwrap();
    reconcile(&mut a, &mut c, T0 + 8).unwrap();
    b.checkpoint(T0 + 7).unwrap(); // concurrent, without A's ops
    let now = T0 + 100 * DAY;
    assert!(a.compact(now).unwrap().is_some());
    assert!(b.compact(now).unwrap().is_some());
    assert!(a.store().body(&add).is_none(), "pruned into A's base");
    let (to_a, _) = reconcile(&mut a, &mut b, now + 1).unwrap();
    let rb = to_a.rebase.expect("A adopted B's snapshot");
    assert!(rb.reissued.contains_key(&add) && rb.reissued.contains_key(&rec), "{rb:?}");
    assert!(
        rb.review
            .iter()
            .any(|i| matches!(i, ReviewItem::Pruned { op, content: PrunedContent::Delete { .. }, .. } if *op == del)),
        "the delete is listed: {rb:?}"
    );
    assert!(b.state().set("groceries").contains(&t("eggs")));
    assert_eq!(b.state().stream("votes").len(), 1);
}

/// BaseRebase for every kind (v0.2), another device's ops: C's delete, set remove
/// and append were folded into A's base under A's checkpoint; when A adopts B's
/// concurrent snapshot they are listed, not lost silently.
#[test]
fn another_devices_deletes_removes_and_appends_lost_from_the_base_are_listed() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    let (add, _) = a.set_add("groceries", t("milk"), T0 + 1).unwrap();
    a.put("feeds", "r", [("ml", v(1))], T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    reconcile(&mut a, &mut c, T0 + 2).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    let (del, _) = c.delete("feeds", "r", T0 + 3).unwrap();
    let (rem, _) = c.set_remove("groceries", t("milk"), T0 + 4).unwrap();
    let (rec, _) = c.append("votes", t("no"), T0 + 5).unwrap();
    reconcile(&mut a, &mut c, T0 + 6).unwrap();
    a.checkpoint(T0 + 7).unwrap();
    b.checkpoint(T0 + 7).unwrap();
    let now = T0 + 100 * DAY;
    assert!(a.compact(now).unwrap().is_some());
    assert!(b.compact(now).unwrap().is_some());
    let (to_a, _) = reconcile(&mut a, &mut b, now + 1).unwrap();
    let review = to_a.rebase.expect("A adopted B's snapshot").review;
    let listed = |id: hearth_sync_kernel::Id| {
        review.iter().find_map(|i| match i {
            ReviewItem::Pruned { op, content, .. } if *op == id => Some(content.clone()),
            _ => None,
        })
    };
    assert!(matches!(listed(del), Some(PrunedContent::Delete { observed, .. }) if !observed.is_empty()), "{review:?}");
    assert_eq!(listed(rem), Some(PrunedContent::SetRemove { set: "groceries".into(), element: t("milk") }));
    assert_eq!(listed(rec), Some(PrunedContent::Append { stream: "votes".into(), record: t("no") }));
    let _ = add;
}

/// BaseRebase must respect what the user already undid: A added and then removed
/// "eggs", and edited and then deleted row x, all pruned into A's base under C's
/// checkpoint. Adopting B's concurrent snapshot must not bring either back; the ops
/// are listed instead.
#[test]
fn base_rebase_does_not_resurrect_what_was_removed_or_deleted() {
    let (mut a, mut b, mut c) = (joined(1, T0), joined(2, T0), joined(3, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    reconcile(&mut a, &mut c, T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    let (add, _) = a.set_add("groceries", t("eggs"), T0 + 2).unwrap();
    let (rem, _) = a.set_remove("groceries", t("eggs"), T0 + 3).unwrap();
    let (put, _) = a.put("feeds", "x", [("ml", v(4))], T0 + 4).unwrap();
    let (del, _) = a.delete("feeds", "x", T0 + 5).unwrap();
    reconcile(&mut a, &mut c, T0 + 6).unwrap();
    c.checkpoint(T0 + 7).unwrap();
    reconcile(&mut a, &mut c, T0 + 8).unwrap();
    b.checkpoint(T0 + 7).unwrap();
    let now = T0 + 100 * DAY;
    assert!(a.compact(now).unwrap().is_some());
    assert!(b.compact(now).unwrap().is_some());
    assert!(a.store().body(&add).is_none() && a.store().body(&put).is_none(), "pruned into A's base");
    let (to_a, _) = reconcile(&mut a, &mut b, now + 1).unwrap();
    let rb = to_a.rebase.expect("A adopted B's snapshot");
    for r in [&a, &b] {
        assert!(!r.state().set("groceries").contains(&t("eggs")), "a removed element came back: {rb:?}");
        assert!(r.state().row("feeds", "x").is_none(), "a deleted row came back: {rb:?}");
    }
    let listed = |id| rb.review.iter().any(|i| format!("{i:?}").contains(&format!("{id:?}")));
    for id in [add, rem, put, del] {
        assert!(listed(id), "{rb:?}");
    }
}

/// A provider cannot make the adopting replica skip the rebase of its own op by
/// listing it among the snapshot's ops while withholding its parent: "the peer
/// knows it" is decided by what actually delivered, not by what the offer claims.
#[test]
fn an_offer_that_lists_your_op_without_its_parent_does_not_skip_its_rebase() {
    let (mut a, mut b) = (joined(1, T0), joined(2, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    b.checkpoint(T0 + 2).unwrap();
    let now = T0 + 100 * DAY;
    assert!(b.compact(now).unwrap().is_some());
    a.put("feeds", "q", [("ml", v(1))], T0 + 10 * DAY).unwrap();
    let (p, pb) = a.put("feeds", "p", [("ml", v(2))], T0 + 11 * DAY).unwrap();
    let req = a.request(&b.hello(now));
    let Offer::Snapshot { snapshot, mut ops } = b.offer(&req).unwrap() else { panic!("snapshot path") };
    ops.push(pb); // claimed, but its parent is withheld
    let rep = a.accept(Offer::Snapshot { snapshot, ops }, now).unwrap();
    let rb = rep.rebase.expect("adopted");
    assert!(rb.reissued.contains_key(&p), "P was neither delivered nor rebased: {rb:?}");
    assert_eq!(a.pending_count(), 0);
    assert_eq!(a.state().row("feeds", "p").unwrap()["ml"], v(2));
}

/// The rebase of unsynced ops follows the OR-set's own rule: an add that a remove
/// observed (here the same device removed it later, offline) stays removed, and so
/// does an edit a later delete observed. Neither is re-issued; both are listed.
#[test]
fn an_offline_add_then_remove_does_not_come_back_through_the_rebase() {
    let (mut a, mut b) = (joined(1, T0), joined(2, T0));
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    b.checkpoint(T0 + 2).unwrap();
    let now = T0 + 100 * DAY;
    assert!(b.compact(now).unwrap().is_some());
    let (add, _) = a.set_add("groceries", t("eggs"), T0 + 10 * DAY).unwrap();
    a.set_remove("groceries", t("eggs"), T0 + 11 * DAY).unwrap();
    let (put, _) = a.put("feeds", "x", [("ml", v(4))], T0 + 12 * DAY).unwrap();
    a.delete("feeds", "x", T0 + 13 * DAY).unwrap();
    let (to_a, _) = reconcile(&mut a, &mut b, now + 1).unwrap();
    let rb = to_a.rebase.expect("A adopted B's snapshot");
    for r in [&a, &b] {
        assert!(!r.state().set("groceries").contains(&t("eggs")), "a removed element came back: {rb:?}");
        assert!(r.state().row("feeds", "x").is_none(), "a deleted row came back: {rb:?}");
    }
    assert!(!rb.reissued.contains_key(&add) && !rb.reissued.contains_key(&put), "{rb:?}");
    assert!(rb.review.iter().any(|i| i.op() == add) && rb.review.iter().any(|i| i.op() == put), "{rb:?}");
}

/// An add concurrent with another device's remove still wins after the rebase (the
/// remove never observed the add's tag), as the OR-set says.
#[test]
fn an_offline_add_concurrent_with_a_remove_still_wins_through_the_rebase() {
    let (mut a, mut b) = (joined(1, T0), joined(2, T0));
    b.set_add("groceries", t("eggs"), T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    let (add, _) = a.set_add("groceries", t("eggs"), T0 + 10 * DAY).unwrap();
    b.set_remove("groceries", t("eggs"), T0 + 3).unwrap();
    b.checkpoint(T0 + 4).unwrap();
    let now = T0 + 100 * DAY;
    assert!(b.compact(now).unwrap().is_some());
    let (to_a, _) = reconcile(&mut a, &mut b, now + 1).unwrap();
    assert!(to_a.rebase.expect("adopted").reissued.contains_key(&add));
    assert!(a.state().set("groceries").contains(&t("eggs")) && b.state().set("groceries").contains(&t("eggs")));
}

/// Ancestry checks (V5 and the Forget cut) walk back only as far as the oldest id
/// they look for, which is exact only while clocks rise from parent to child (V3).
/// A snapshot index is not re-validated op by op, so it is checked for that too:
/// even a checkpoint signed by an enrolled device cannot vouch for an index that
/// breaks V3.
#[test]
fn a_snapshot_index_whose_clocks_do_not_rise_is_refused() {
    use hearth_sync_kernel::keys::{DeviceSigner, SoftSigner};
    use hearth_sync_kernel::op::{Body, Op, Unsigned};
    use hearth_sync_kernel::replica::checkpoint_hash;
    let mut a = joined(1, T0);
    a.put("feeds", "r1", [("ml", v(1))], T0 + 1).unwrap();
    a.checkpoint(T0 + 2).unwrap();
    a.compact(T0 + 2 + 90 * DAY).unwrap().unwrap();
    let mut snap = a.snapshot().unwrap();
    // Push A's first op (its Enroll) above its child's clock, and re-sign a
    // checkpoint over the doctored index with A's own key.
    snap.index[0].1.hlc.millis = T0 + 50;
    let cp = hearth_sync_kernel::op::decode(&snap.checkpoint).unwrap();
    let kept: std::collections::BTreeSet<_> = snap.kept.iter().map(|b| hearth_sync_kernel::sha256(b)).collect();
    let state_hash = checkpoint_hash(&snap.state, &snap.index, &kept);
    let signer = SoftSigner::from_secret([1; 32]);
    let u = Unsigned {
        app: APP.into(),
        device: signer.device(),
        parents: cp.parents.clone(),
        hlc: cp.hlc,
        body: Body::Checkpoint { state_hash },
    };
    let op: Op = u.clone().with_sig(signer.sign(&u.signable_bytes()));
    snap.checkpoint = op.encode();
    snap.kept.clear();
    let mut o = observer();
    let err = o.accept(Offer::Snapshot { snapshot: snap, ops: vec![] }, T0 + 91 * DAY).unwrap_err();
    assert!(matches!(err, KernelError::BadSnapshot(hearth_sync_kernel::op::Reject::ClockNotAfterParents)), "{err:?}");
}

/// A device whose clock ran ahead wrote an edit, then its clock was put right. When
/// it next meets an old checkpoint it must rebase, and a rebase authors ops, which
/// the creation-time guard refuses while the log is more than the skew bound ahead
/// of the clock (`ClockBehind`). Found by the persistence property: adoption used to
/// install the snapshot first and fail half-way through the rebase, leaving the
/// edit in neither the log nor the review list, and the device unenrolled. It must
/// refuse before touching anything, and adopt once real time catches up.
#[test]
fn a_device_whose_clock_is_behind_its_log_refuses_a_snapshot_instead_of_losing_edits() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    let hour = 60 * 60 * 1000;
    let (edit, _) = b.put("feeds", "r1", [("ml", v(5))], T0 + hour).unwrap();
    let mut c = joined(3, T0);
    reconcile(&mut a, &mut c, T0 + 1).unwrap();
    c.checkpoint(T0 + 2).unwrap();
    reconcile(&mut a, &mut c, T0 + 3).unwrap();
    let now = T0 + 3 + 20_000;
    let cfg = Config::horizon(20_000);
    let mut a2 = hearth_sync_kernel::replica::Replica::<hearth_sync_kernel::store::MemStore>::observer(
        APP,
        root().enroll_public(APP),
        cfg,
    );
    a2.ingest(a.log(), T0 + 4);
    assert!(a2.compact(now).unwrap().is_some(), "an old checkpoint to adopt");
    // B's own clock now reads real time again, an hour behind its log.
    let err = reconcile(&mut b, &mut a2, now).unwrap_err();
    assert!(matches!(err, KernelError::ClockBehind { .. }), "{err:?}");
    assert!(b.store().contains(&edit), "the edit is still in B's log");
    assert_eq!(b.state().row("feeds", "r1").unwrap()["ml"], v(5));
    // Once real time passes B's log, the sync goes through and the edit is rebased.
    let (to_b, _) = reconcile(&mut b, &mut a2, T0 + hour + 1000).unwrap();
    let rebase = to_b.rebase.expect("B adopted the snapshot");
    assert!(rebase.reissued.contains_key(&edit), "{rebase:?}");
    assert_eq!(a2.state().row("feeds", "r1").unwrap()["ml"], v(5));
}

/// BaseRebase lists, never re-issues, an own edit that a live delete observed, even
/// when a later edit by another device keeps the row visible. v0.2.1 applied that
/// rule to unsynced ops; the base path still asked only whether the row was visible.
/// Found by the horizon property (about 1 case in 15,000), present since v0.2.1.
#[test]
fn base_rebase_does_not_reissue_an_edit_a_live_delete_observed_when_the_row_is_visible() {
    let cfg = Config::horizon(20_000);
    let mut p: Vec<hearth_sync_kernel::replica::Replica> = (1..=3).map(|n| joined_with(n, T0, cfg)).collect();
    let (edit, _) = p[1].put("t", "r0", [("a", v(0))], T0 + 1000).unwrap();
    p[1].delete("t", "r0", T0 + 2000).unwrap();
    {
        let (a, b) = p.split_at_mut(1);
        reconcile(&mut b[0], &mut a[0], T0 + 3000).unwrap();
    }
    p[0].checkpoint(T0 + 4000).unwrap();
    // Device 0 edits another field of the deleted row: the row is visible again,
    // but the delete still observes device 1's edit.
    p[0].put("t", "r0", [("b", v(0))], T0 + 5000).unwrap();
    p[2].checkpoint(T0 + 6000).unwrap();
    let now = T0 + 6000 + 20_000;
    {
        let (a, b) = p.split_at_mut(1);
        reconcile(&mut a[0], &mut b[0], now + 1000).unwrap();
    }
    // Device 1 adopted device 0's snapshot, so the edit and the delete are in its
    // base. Device 2's snapshot lacks both.
    let (_, rest) = p.split_at_mut(1);
    let (one, two) = rest.split_at_mut(1);
    let (to_one, _) = reconcile(&mut one[0], &mut two[0], now + 2000).unwrap();
    let rebase = to_one.rebase.expect("device 1 adopted device 2's snapshot");
    assert!(!rebase.reissued.contains_key(&edit), "an undone edit was re-issued: {rebase:?}");
    assert!(rebase.review.iter().any(|i| matches!(i, ReviewItem::RowDeleted { op, .. } if *op == edit)), "{rebase:?}");
}
