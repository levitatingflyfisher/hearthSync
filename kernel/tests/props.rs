//! Property tests: convergence, idempotence and commutativity over op sets that
//! real replicas produced (random edits, deletes, restores, set ops, appends,
//! checkpoints, partial syncs and forgets between three devices), and, over a short
//! horizon with compaction, snapshots and returns past it, that every replica's state
//! is the unpruned fold of what it delivered (ADR 0006's `ind_ideal`).

mod common;
use common::*;

use hearth_sync_kernel::op::Value;
use hearth_sync_kernel::replica::Replica;
use hearth_sync_kernel::store::OpStore;
use hearth_sync_kernel::sync::reconcile;
use hearth_sync_kernel::{sha256, Id};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// Fixed so the count is reportable and the run fits a 4 GiB memory cap in debug.
/// `PROPTEST_CASES` overrides it (and the horizon property's count), as CI does.
const CASES: u32 = 1000;

fn cases(default: u32) -> u32 {
    std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[derive(Clone, Debug)]
enum Step {
    Put { dev: usize, row: u8, field: u8, val: i8 },
    Delete { dev: usize, row: u8 },
    Restore { dev: usize, row: u8 },
    Add { dev: usize, elem: u8 },
    Remove { dev: usize, elem: u8 },
    Append { dev: usize, val: i8 },
    Checkpoint { dev: usize },
    Sync { a: usize, b: usize },
    Forget { by: usize, target: usize },
}

fn step() -> impl Strategy<Value = Step> {
    let dev = 0usize..3;
    prop_oneof![
        8 => (dev.clone(), 0u8..3, 0u8..3, any::<i8>()).prop_map(|(dev, row, field, val)| Step::Put { dev, row, field, val }),
        3 => (dev.clone(), 0u8..3).prop_map(|(dev, row)| Step::Delete { dev, row }),
        2 => (dev.clone(), 0u8..3).prop_map(|(dev, row)| Step::Restore { dev, row }),
        3 => (dev.clone(), 0u8..2).prop_map(|(dev, elem)| Step::Add { dev, elem }),
        2 => (dev.clone(), 0u8..2).prop_map(|(dev, elem)| Step::Remove { dev, elem }),
        2 => (dev.clone(), any::<i8>()).prop_map(|(dev, val)| Step::Append { dev, val }),
        1 => dev.clone().prop_map(|dev| Step::Checkpoint { dev }),
        5 => (dev.clone(), dev.clone()).prop_filter("distinct", |(a, b)| a != b).prop_map(|(a, b)| Step::Sync { a, b }),
        1 => (dev.clone(), dev).prop_map(|(by, target)| Step::Forget { by, target }),
    ]
}

struct Sim {
    peers: Vec<Replica>,
    now: u64,
}

fn pair(peers: &mut [Replica], a: usize, b: usize) -> (&mut Replica, &mut Replica) {
    let (lo, hi) = (a.min(b), a.max(b));
    let (l, h) = peers.split_at_mut(hi);
    if a < b {
        (&mut l[lo], &mut h[0])
    } else {
        (&mut h[0], &mut l[lo])
    }
}

fn run(steps: &[Step]) -> Sim {
    let mut sim = Sim { peers: (1..=3).map(|n| joined(n, T0)).collect(), now: T0 };
    for s in steps {
        sim.now += 1000;
        let now = sim.now;
        let p = &mut sim.peers;
        // Authoring can fail only on a wiped device (NoKeys); skip those steps.
        let _ = match *s {
            Step::Put { dev, row, field, val } => p[dev]
                .put("t", &format!("r{row}"), [(["a", "b", "c"][field as usize], Value::Int(val as i64))], now)
                .map(|_| ()),
            Step::Delete { dev, row } => p[dev].delete("t", &format!("r{row}"), now).map(|_| ()),
            Step::Restore { dev, row } => p[dev].restore("t", &format!("r{row}"), now).map(|_| ()),
            Step::Add { dev, elem } => p[dev].set_add("s", Value::Int(elem as i64), now).map(|_| ()),
            Step::Remove { dev, elem } => p[dev].set_remove("s", Value::Int(elem as i64), now).map(|_| ()),
            Step::Append { dev, val } => p[dev].append("log", Value::Int(val as i64), now).map(|_| ()),
            Step::Checkpoint { dev } => p[dev].checkpoint(now).map(|_| ()),
            Step::Sync { a, b } => {
                let (x, y) = pair(p, a, b);
                reconcile(x, y, now).map(|_| ())
            }
            Step::Forget { by, target } => {
                let d = p[target].device().unwrap();
                p[by].forget(d, now).map(|_| ())
            }
        };
    }
    sim
}

/// Every op any replica delivered, once each.
fn union(sim: &Sim) -> Vec<Vec<u8>> {
    let mut all: BTreeMap<Id, Vec<u8>> = BTreeMap::new();
    for p in &sim.peers {
        for b in p.log() {
            all.insert(sha256(&b), b);
        }
    }
    all.into_values().collect()
}

/// Deterministic shuffle (SplitMix64 + Fisher–Yates) so a failing seed replays.
fn shuffled(mut v: Vec<Vec<u8>>, seed: u64) -> Vec<Vec<u8>> {
    let mut x = seed;
    let mut next = || {
        x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = x;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..v.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
    v
}

fn deliver_all(ops: &[Vec<u8>], now: u64) -> Replica {
    let mut o = observer();
    let rep = o.ingest(ops, now);
    assert!(rep.rejected.is_empty(), "honest ops rejected: {:?}", rep.rejected);
    o
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(CASES), failure_persistence: None, ..ProptestConfig::default() })]

    /// Any delivery order, with duplicates, gives the same state, heads and exclusions,
    /// and that state equals a from-scratch refold.
    #[test]
    fn convergence_in_any_delivery_order(steps in prop::collection::vec(step(), 1..40), s1 in any::<u64>(), s2 in any::<u64>(), dup in any::<u64>()) {
        let sim = run(&steps);
        let all = union(&sim);
        let mut order1 = shuffled(all.clone(), s1);
        // Duplicate a random slice.
        let k = (dup as usize) % (all.len() + 1);
        order1.extend(shuffled(all.clone(), dup).into_iter().take(k));
        let x = deliver_all(&order1, sim.now);
        let y = deliver_all(&shuffled(all.clone(), s2), sim.now);
        prop_assert_eq!(x.pending_count(), 0);
        prop_assert_eq!(x.quarantined_count(), 0);
        prop_assert_eq!(x.store().len(), all.len(), "every generated op delivered");
        prop_assert_eq!(y.store().len(), all.len());
        prop_assert_eq!(x.state().encode(), y.state().encode());
        prop_assert_eq!(x.heads(), y.heads());
        prop_assert_eq!(x.excluded(), y.excluded());
        let (scratch, excl) = x.fold_from_scratch();
        prop_assert_eq!(scratch.encode(), x.state().encode(), "incremental fold == full refold");
        prop_assert_eq!(&excl, x.excluded());
    }

    /// After everyone syncs, the three replicas hold what an observer of the union holds.
    #[test]
    fn replicas_converge_after_full_sync(steps in prop::collection::vec(step(), 1..40)) {
        let mut sim = run(&steps);
        let all = union(&sim);
        for _ in 0..2 {
            for (a, b) in [(0, 1), (1, 2), (0, 2)] {
                sim.now += 1000;
                let (x, y) = pair(&mut sim.peers, a, b);
                reconcile(x, y, sim.now).unwrap();
            }
        }
        let o = deliver_all(&all, sim.now);
        for p in &sim.peers {
            prop_assert_eq!(p.store().len(), all.len());
            prop_assert_eq!(p.state().encode(), o.state().encode());
            prop_assert_eq!(p.heads(), o.heads());
        }
    }

    /// Ingesting everything again changes nothing.
    #[test]
    fn idempotence(steps in prop::collection::vec(step(), 1..40), seed in any::<u64>()) {
        let sim = run(&steps);
        let all = union(&sim);
        let mut x = deliver_all(&all, sim.now);
        let before = x.state().encode();
        let rep = x.ingest(shuffled(all.clone(), seed), sim.now);
        prop_assert_eq!(rep.duplicates, all.len());
        prop_assert!(rep.delivered.is_empty());
        prop_assert_eq!(x.state().encode(), before);
    }

    /// Two batches in either order give the same state (any split, not only causal ones:
    /// the second batch's ops wait for parents in the first).
    #[test]
    fn commutativity_of_batches(steps in prop::collection::vec(step(), 1..40), mask in any::<u64>()) {
        let sim = run(&steps);
        let all = union(&sim);
        let (p, q): (Vec<_>, Vec<_>) = all.iter().cloned().enumerate().partition(|(i, _)| mask >> (i % 64) & 1 == 1);
        let p: Vec<Vec<u8>> = p.into_iter().map(|(_, b)| b).collect();
        let q: Vec<Vec<u8>> = q.into_iter().map(|(_, b)| b).collect();
        let mut x = observer();
        x.ingest(&p, sim.now);
        x.ingest(&q, sim.now);
        let mut y = observer();
        y.ingest(&q, sim.now);
        y.ingest(&p, sim.now);
        prop_assert_eq!(x.store().len(), all.len());
        prop_assert_eq!(x.state().encode(), y.state().encode());
    }
}

// ---------------------------------------------------------------- the horizon

/// Cases for the horizon property: it refolds on every checkpoint once a Forget is
/// known, so it is dearer per case than the four above.
const HORIZON_CASES: u32 = 1000;
/// A short horizon so checkpoints age within a case: 20 steps of 1 s, or one Tick.
const H: u64 = 20_000;

#[derive(Clone, Debug)]
enum HStep {
    Put {
        dev: usize,
        row: u8,
        field: u8,
        val: i8,
    },
    Delete {
        dev: usize,
        row: u8,
    },
    Restore {
        dev: usize,
        row: u8,
    },
    Add {
        dev: usize,
        elem: u8,
    },
    Remove {
        dev: usize,
        elem: u8,
    },
    Append {
        dev: usize,
        val: i8,
    },
    Checkpoint {
        dev: usize,
    },
    Compact {
        dev: usize,
    },
    Sync {
        a: usize,
        b: usize,
    },
    Forget {
        by: usize,
        target: usize,
    },
    /// Time jumps past the horizon.
    Tick,
}

fn hstep() -> impl Strategy<Value = HStep> {
    let dev = 0usize..3;
    prop_oneof![
        8 => (dev.clone(), 0u8..2, 0u8..2, any::<i8>()).prop_map(|(dev, row, field, val)| HStep::Put { dev, row, field, val }),
        2 => (dev.clone(), 0u8..2).prop_map(|(dev, row)| HStep::Delete { dev, row }),
        1 => (dev.clone(), 0u8..2).prop_map(|(dev, row)| HStep::Restore { dev, row }),
        2 => (dev.clone(), 0u8..2).prop_map(|(dev, elem)| HStep::Add { dev, elem }),
        2 => (dev.clone(), 0u8..2).prop_map(|(dev, elem)| HStep::Remove { dev, elem }),
        2 => (dev.clone(), any::<i8>()).prop_map(|(dev, val)| HStep::Append { dev, val }),
        3 => dev.clone().prop_map(|dev| HStep::Checkpoint { dev }),
        3 => dev.clone().prop_map(|dev| HStep::Compact { dev }),
        6 => (dev.clone(), dev.clone()).prop_filter("distinct", |(a, b)| a != b).prop_map(|(a, b)| HStep::Sync { a, b }),
        1 => (dev.clone(), dev).prop_map(|(by, target)| HStep::Forget { by, target }),
        2 => Just(HStep::Tick),
    ]
}

/// Every body any replica ever held, collected after every step (pruning drops them).
fn archive(sim: &Sim, all: &mut BTreeMap<Id, Vec<u8>>) {
    for p in &sim.peers {
        for b in p.log() {
            all.entry(sha256(&b)).or_insert(b);
        }
    }
}

/// Ops a rebase re-issued or listed for review: accounted for, not lost.
fn account(rep: &hearth_sync_kernel::sync::AcceptReport, accounted: &mut BTreeSet<Id>) {
    use hearth_sync_kernel::sync::ReviewItem as R;
    if let Some(r) = &rep.rebase {
        accounted.extend(r.reissued.keys().copied());
        for i in &r.review {
            accounted.insert(match i {
                R::Field { op, .. } | R::RowDeleted { op, .. } | R::Op { op, .. } => *op,
                R::Foreign { op, .. } | R::Lost { op, .. } | R::Pruned { op, .. } => *op,
            });
        }
    }
}

/// A rebase never re-issues what the adopting replica's state had already undone:
/// an add whose tag a remove observed, or an edit a live delete observed (the OR-set
/// and observed-remove rules; an add or edit concurrent with the remove still wins).
fn check_no_resurrection(
    before: &hearth_sync_kernel::state::State,
    rep: &hearth_sync_kernel::sync::AcceptReport,
) -> Result<(), TestCaseError> {
    let Some(r) = &rep.rebase else { return Ok(()) };
    for old in r.reissued.keys() {
        let removed = before.sets.values().any(|e| e.adds.contains(old) && e.removed.contains(old));
        let deleted = before.rows.values().any(|row| {
            row.puts.contains(old) && row.deletes.iter().any(|(d, obs)| obs.contains(old) && !row.restored.contains(d))
        });
        prop_assert!(!removed && !deleted, "the rebase re-issued an op the replica had already undone");
    }
    Ok(())
}

/// One reconcile that never stalls (Fallback, ADR 0007), with its rebases accounted.
fn hsync(x: &mut Replica, y: &mut Replica, now: u64, accounted: &mut BTreeSet<Id>) -> Result<(), TestCaseError> {
    let (sx, sy) = (x.state().clone(), y.state().clone());
    match reconcile(x, y, now) {
        Ok((a, b)) => {
            check_no_resurrection(&sx, &a)?;
            check_no_resurrection(&sy, &b)?;
            account(&a, accounted);
            account(&b, accounted);
        }
        Err(e) => prop_assert!(false, "sync failed: {:?}", e),
    }
    Ok(())
}

fn hrun(steps: &[HStep], all: &mut BTreeMap<Id, Vec<u8>>, accounted: &mut BTreeSet<Id>) -> Result<Sim, TestCaseError> {
    let cfg = hearth_sync_kernel::replica::Config::horizon(H);
    let mut sim = Sim { peers: (1..=3).map(|n| joined_with(n, T0, cfg)).collect(), now: T0 };
    archive(&sim, all);
    for s in steps {
        sim.now += if matches!(s, HStep::Tick) { H } else { 1000 };
        let now = sim.now;
        let p = &mut sim.peers;
        // Authoring fails only on a wiped device.
        let _ = match *s {
            HStep::Put { dev, row, field, val } => p[dev]
                .put("t", &format!("r{row}"), [(["a", "b"][field as usize], Value::Int(val as i64))], now)
                .map(|_| ()),
            HStep::Delete { dev, row } => p[dev].delete("t", &format!("r{row}"), now).map(|_| ()),
            HStep::Restore { dev, row } => p[dev].restore("t", &format!("r{row}"), now).map(|_| ()),
            HStep::Add { dev, elem } => p[dev].set_add("s", Value::Int(elem as i64), now).map(|_| ()),
            HStep::Remove { dev, elem } => p[dev].set_remove("s", Value::Int(elem as i64), now).map(|_| ()),
            HStep::Append { dev, val } => p[dev].append("log", Value::Int(val as i64), now).map(|_| ()),
            HStep::Checkpoint { dev } => p[dev].checkpoint(now).map(|_| ()),
            HStep::Compact { dev } => p[dev].compact(now).map(|_| ()),
            HStep::Sync { a, b } => {
                let (x, y) = pair(p, a, b);
                hsync(x, y, now, accounted)?;
                Ok(())
            }
            HStep::Forget { by, target } => {
                let d = p[target].device().unwrap();
                p[by].forget(d, now).map(|_| ())
            }
            HStep::Tick => Ok(()),
        };
        archive(&sim, all);
    }
    Ok(sim)
}

/// ADR 0006's `ind_ideal`: each replica's state equals the fold an unpruned replica
/// computes from the same delivered set, and an incremental fold equals a refold.
/// Plus a proxy for (f), Forget durability.
fn check_ideal(sim: &Sim, all: &BTreeMap<Id, Vec<u8>>, accounted: &BTreeSet<Id>) -> Result<(), TestCaseError> {
    for (n, p) in sim.peers.iter().enumerate() {
        prop_assert!(p.rejected().is_empty(), "replica {} rejected {:?}", n, p.rejected());
        let ids = p.store().ids();
        let bodies: Vec<&Vec<u8>> = ids.iter().map(|id| all.get(id).expect("every op archived")).collect();
        let mut o = observer();
        let rep = o.ingest(bodies, sim.now);
        prop_assert!(rep.rejected.is_empty() && o.pending_count() == 0 && o.quarantined_count() == 0);
        prop_assert_eq!(o.store().len(), ids.len());
        prop_assert_eq!(o.state().encode(), p.state().encode(), "replica {} differs from the unpruned fold", n);
        prop_assert_eq!(p.fold_from_scratch().0.encode(), p.state().encode(), "replica {}: refold differs", n);
    }
    // ADR 0006 (f), as a proxy: a Forget once written is never lost from the household,
    // so every device any Forget named is still forgotten on some replica.
    for bytes in all.values() {
        let op = hearth_sync_kernel::op::decode(bytes).expect("archived ops decode");
        if let hearth_sync_kernel::op::Body::Forget { device, .. } = op.body {
            prop_assert!(sim.peers.iter().any(|p| p.state().forgotten().contains(&device)), "a Forget was lost");
        }
    }
    // Nothing vanishes (ADR 0006 (g), widened to every kind by ADR 0007): every op
    // ever written is still delivered on some replica, or was re-issued or listed.
    for (id, bytes) in all {
        let op = hearth_sync_kernel::op::decode(bytes).expect("archived ops decode");
        if op.body.kind() == hearth_sync_kernel::op::Kind::Checkpoint {
            continue;
        }
        let held = sim.peers.iter().any(|p| p.store().contains(id));
        prop_assert!(
            held || accounted.contains(id),
            "a {:?} vanished without a re-issue or a review item",
            op.body.kind()
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(HORIZON_CASES), failure_persistence: None, ..ProptestConfig::default() })]

    /// With checkpoints, compaction, Forgets, snapshots and returns past a short
    /// horizon: every replica's state is the unpruned fold of what it delivered, so
    /// replicas that delivered the same ops agree, before and after full syncs.
    #[test]
    fn horizon_state_is_the_unpruned_fold(steps in prop::collection::vec(hstep(), 1..40)) {
        let mut all = BTreeMap::new();
        let mut accounted = BTreeSet::new();
        let mut sim = hrun(&steps, &mut all, &mut accounted)?;
        check_ideal(&sim, &all, &accounted)?;
        for _ in 0..2 {
            for (a, b) in [(0, 1), (1, 2), (0, 2)] {
                sim.now += 1000;
                let (x, y) = pair(&mut sim.peers, a, b);
                hsync(x, y, sim.now, &mut accounted)?;
                archive(&sim, &mut all);
            }
        }
        check_ideal(&sim, &all, &accounted)?;
    }
}
