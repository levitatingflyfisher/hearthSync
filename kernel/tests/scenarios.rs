//! The design's §5 acceptance scenarios.

mod common;
use common::*;

use hearth_sync_kernel::replica::Replica;
use hearth_sync_kernel::sync::{reconcile, Offer};

/// D1, ported from packages/sanctuary_auth_core/test/sync/stale_peer_repro_test.dart
/// (known red there): deletion inferred from absence dropped S on the second cycle.
///
/// Device A and device B share row r0. B publishes once (ops: create r0) and goes
/// silent. A creates S. A syncs against B's stale log, twice. S is present on A after
/// both cycles, and A's state after cycle 2 is byte-identical to after cycle 1.
#[test]
fn d1_row_created_on_a_survives_two_syncs_against_a_stale_peer_b() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    b.put("items", "r0", [("modifiedAt", v(1000))], T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    // B uploaded once and has not synced since: its log is frozen here.
    let b_stale_log = b.log();
    drop(b);

    a.put("items", "S", [("modifiedAt", v(2000))], T0 + 3).unwrap();

    let cycle = |a: &mut Replica, now| a.accept(Offer::Ops(b_stale_log.clone()), now).unwrap();
    cycle(&mut a, T0 + 4);
    assert!(a.state().row("items", "S").is_some(), "after cycle 1");
    let after_1 = a.state().encode();
    cycle(&mut a, T0 + 5);
    assert!(a.state().row("items", "S").is_some(), "after cycle 2: S must not vanish");
    assert_eq!(a.state().encode(), after_1, "cycle 2 changes nothing");
    assert!(a.state().row("items", "r0").is_some());
}

#[test]
fn edit_beats_a_concurrent_delete() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    a.put("feeds", "R", [("ml", v(100)), ("side", t("left"))], T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    // Concurrently: A deletes R, B edits one field of R.
    a.delete("feeds", "R", T0 + 3).unwrap();
    b.put("feeds", "R", [("ml", v(120))], T0 + 3).unwrap();
    reconcile(&mut a, &mut b, T0 + 4).unwrap();
    for r in [&a, &b] {
        let row = r.state().row("feeds", "R").expect("R visible with B's edit");
        assert_eq!(row["ml"], v(120));
        assert_eq!(row["side"], t("left"), "tombstone kept the other field");
    }
    assert_eq!(a.state().encode(), b.state().encode());
    // A later delete that has seen B's edit does stick, and Undo brings it all back.
    a.delete("feeds", "R", T0 + 5).unwrap();
    reconcile(&mut a, &mut b, T0 + 6).unwrap();
    assert!(b.state().row("feeds", "R").is_none());
    b.restore("feeds", "R", T0 + 7).unwrap();
    reconcile(&mut a, &mut b, T0 + 8).unwrap();
    assert_eq!(a.state().row("feeds", "R").unwrap()["ml"], v(120));
}

#[test]
fn or_set_milk_concurrent_readd_survives_a_remove() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    a.set_add("groceries", t("milk"), T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    a.set_remove("groceries", t("milk"), T0 + 3).unwrap();
    b.set_add("groceries", t("milk"), T0 + 3).unwrap();
    reconcile(&mut a, &mut b, T0 + 4).unwrap();
    assert!(a.state().set("groceries").contains(&t("milk")));
    assert_eq!(a.state().encode(), b.state().encode());
    // A remove that saw both adds empties it.
    b.set_remove("groceries", t("milk"), T0 + 5).unwrap();
    reconcile(&mut a, &mut b, T0 + 6).unwrap();
    assert!(a.state().set("groceries").is_empty());
}

#[test]
fn append_streams_keep_every_record_in_clock_order() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    a.append("votes", t("a1"), T0 + 1).unwrap();
    b.append("votes", t("b1"), T0 + 2).unwrap();
    a.append("votes", t("a2"), T0 + 3).unwrap();
    reconcile(&mut a, &mut b, T0 + 4).unwrap();
    let got: Vec<_> = a.state().stream("votes").into_iter().map(|(_, v)| v).collect();
    assert_eq!(got, vec![t("a1"), t("b1"), t("a2")]);
    assert_eq!(a.state().stream("votes"), b.state().stream("votes"));
}

/// A, B, C sync in every pairwise order while one of them only ever offers a
/// frozen (stale) log; afterwards everyone converges to identical heads and state.
#[test]
fn three_peers_with_stale_blobs_converge_in_every_order() {
    let pairs = [(0usize, 1usize), (0, 2), (1, 2)];
    let orders = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
    for stale in 0..3 {
        for order in orders {
            let mut peers: Vec<Replica> = (1..=3).map(|n| joined(n, T0)).collect();
            let mut now = T0 + 1;
            for (i, p) in peers.iter_mut().enumerate() {
                p.put("items", "shared", [("by", v(i as i64))], now).unwrap();
                p.put("items", &format!("own{i}"), [("n", v(1))], now).unwrap();
                now += 1;
            }
            let frozen = peers[stale].log();
            for &k in &order {
                let (x, y) = pairs[k];
                now += 1;
                if x == stale || y == stale {
                    let other = if x == stale { y } else { x };
                    peers[other].accept(Offer::Ops(frozen.clone()), now).unwrap();
                    // A deletion by absence would show here; nothing may vanish.
                    assert!(peers[other].state().row("items", &format!("own{other}")).is_some());
                } else {
                    let (lo, hi) = peers.split_at_mut(y);
                    reconcile(&mut lo[x], &mut hi[0], now).unwrap();
                }
            }
            // Finally everyone syncs for real.
            for _ in 0..2 {
                for (x, y) in pairs {
                    now += 1;
                    let (lo, hi) = peers.split_at_mut(y);
                    reconcile(&mut lo[x], &mut hi[0], now).unwrap();
                }
            }
            let s0 = peers[0].state().encode();
            for p in &peers {
                assert_eq!(p.heads(), peers[0].heads(), "stale={stale} order={order:?}");
                assert_eq!(p.state().encode(), s0, "stale={stale} order={order:?}");
                for i in 0..3 {
                    assert!(p.state().row("items", &format!("own{i}")).is_some());
                }
            }
        }
    }
}

/// A deletes R and syncs, then reinstalls (fresh keys, empty log) and syncs again:
/// R stays deleted, because the delete is an op, not a missing base row.
#[test]
fn reinstall_then_sync_does_not_resurrect() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    a.put("feeds", "R", [("ml", v(1))], T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    a.delete("feeds", "R", T0 + 3).unwrap();
    reconcile(&mut a, &mut b, T0 + 4).unwrap();
    drop(a);
    let mut a2 = joined(11, T0 + 5); // same household phrase, new device key
    reconcile(&mut a2, &mut b, T0 + 6).unwrap();
    assert!(a2.state().row("feeds", "R").is_none());
    assert!(b.state().row("feeds", "R").is_none());
    assert_eq!(a2.state().encode(), b.state().encode());
}
