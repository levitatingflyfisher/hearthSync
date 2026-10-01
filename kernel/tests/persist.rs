//! Persistence (ADR 0010): the records a replica emits rebuild it exactly, the
//! records it emitted call by call add up to a full rewrite, a changeset cut short
//! by a crash still loads, and the reloaded replica behaves as the original does.

mod common;
use common::*;

use hearth_sync_kernel::keys::{DeviceSigner, HouseholdRoot, SoftSigner};
use hearth_sync_kernel::op::Value;
use hearth_sync_kernel::persist::{stored_device, Changeset, MemPersist, Persist, PersistError, Record};
use hearth_sync_kernel::replica::{Config, Replica};
use hearth_sync_kernel::schema::{Collection, FieldDef, Merge, Schema, ValueType};
use hearth_sync_kernel::seal::SealKeys;
use hearth_sync_kernel::sync::reconcile;
use proptest::prelude::*;
use std::collections::BTreeMap;

/// `PROPTEST_CASES` overrides it, as CI does.
const CASES: u32 = 300;

fn cases() -> u32 {
    std::env::var("PROPTEST_CASES").ok().and_then(|v| v.parse().ok()).unwrap_or(CASES)
}
/// A short horizon so checkpoints age within a case.
const H: u64 = 20_000;

fn keys() -> SealKeys {
    SealKeys::derive(&root(), APP)
}

fn schema() -> Schema {
    let f = |n: &str| FieldDef { name: n.into(), ty: ValueType::Int, nullable: false };
    Schema {
        collections: vec![
            Collection { name: "t".into(), merge: Merge::Lww { fields: vec![f("a"), f("b")], container: None } },
            Collection { name: "s".into(), merge: Merge::AddWinsSet { element: ValueType::Int } },
            Collection { name: "log".into(), merge: Merge::AppendOnly { record: ValueType::Int } },
        ],
        horizon_ms: H,
        keep_full_history: false,
    }
}

fn signer(n: u8) -> Box<dyn DeviceSigner> {
    Box::new(SoftSigner::from_secret([n; 32]))
}

fn reload(p: &MemPersist, n: u8, with_schema: bool, now: u64) -> Replica {
    let (mut r, _) = Replica::load(&keys(), Config::horizon(H), Some((root(), signer(n))), p.records(), now)
        .unwrap_or_else(|e| panic!("device {n} failed to load: {e:?}"));
    if with_schema {
        r.set_schema(schema(), now);
    }
    r
}

fn as_map(c: &Changeset) -> BTreeMap<Vec<u8>, Vec<u8>> {
    c.records.iter().map(|r| (r.key.clone(), r.value.clone().expect("a full rewrite has no deletes"))).collect()
}

#[derive(Clone, Debug)]
enum Step {
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
    /// Device 2 (which registers no schema) writes a table the others do not
    /// declare: held there (V7).
    Undeclared {
        val: i8,
    },
    /// A device whose clock runs an hour ahead writes: quarantined elsewhere (V8).
    Future {
        dev: usize,
    },
    Tick,
}

fn step() -> impl Strategy<Value = Step> {
    let dev = 0usize..3;
    prop_oneof![
        8 => (dev.clone(), 0u8..2, 0u8..2, any::<i8>()).prop_map(|(dev, row, field, val)| Step::Put { dev, row, field, val }),
        2 => (dev.clone(), 0u8..2).prop_map(|(dev, row)| Step::Delete { dev, row }),
        1 => (dev.clone(), 0u8..2).prop_map(|(dev, row)| Step::Restore { dev, row }),
        2 => (dev.clone(), 0u8..2).prop_map(|(dev, elem)| Step::Add { dev, elem }),
        1 => (dev.clone(), 0u8..2).prop_map(|(dev, elem)| Step::Remove { dev, elem }),
        1 => (dev.clone(), any::<i8>()).prop_map(|(dev, val)| Step::Append { dev, val }),
        3 => dev.clone().prop_map(|dev| Step::Checkpoint { dev }),
        3 => dev.clone().prop_map(|dev| Step::Compact { dev }),
        6 => (dev.clone(), dev.clone()).prop_filter("distinct", |(a, b)| a != b).prop_map(|(a, b)| Step::Sync { a, b }),
        1 => (dev.clone(), dev.clone()).prop_map(|(by, target)| Step::Forget { by, target }),
        1 => any::<i8>().prop_map(|val| Step::Undeclared { val }),
        1 => dev.prop_map(|dev| Step::Future { dev }),
        2 => Just(Step::Tick),
    ]
}

struct World {
    peers: Vec<Replica>,
    stores: Vec<MemPersist>,
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

fn has_schema(n: usize) -> bool {
    n != 2
}

fn world() -> World {
    let mut peers: Vec<Replica> = (1..=3).map(|n| joined_with(n, T0, Config::horizon(H))).collect();
    for (i, p) in peers.iter_mut().enumerate() {
        if has_schema(i) {
            p.set_schema(schema(), T0);
        }
    }
    let mut w = World { peers, stores: vec![MemPersist::default(); 3], now: T0 };
    persist_all(&mut w);
    w
}

fn persist_all(w: &mut World) {
    let k = keys();
    for (p, s) in w.peers.iter_mut().zip(w.stores.iter_mut()) {
        s.apply(&p.take_changes(&k));
    }
}

/// One step; returns the changesets it produced (after persisting them).
fn apply(w: &mut World, s: &Step) -> Vec<Changeset> {
    w.now += if matches!(s, Step::Tick) { H } else { 1000 };
    let now = w.now;
    let p = &mut w.peers;
    // Authoring fails on a wiped device or a clock far behind; that is fine here.
    let _ = match *s {
        Step::Put { dev, row, field, val } => {
            p[dev].put("t", &format!("r{row}"), [(["a", "b"][field as usize], Value::Int(val as i64))], now).map(|_| ())
        }
        Step::Delete { dev, row } => p[dev].delete("t", &format!("r{row}"), now).map(|_| ()),
        Step::Restore { dev, row } => p[dev].restore("t", &format!("r{row}"), now).map(|_| ()),
        Step::Add { dev, elem } => p[dev].set_add("s", Value::Int(elem as i64), now).map(|_| ()),
        Step::Remove { dev, elem } => p[dev].set_remove("s", Value::Int(elem as i64), now).map(|_| ()),
        Step::Append { dev, val } => p[dev].append("log", Value::Int(val as i64), now).map(|_| ()),
        Step::Checkpoint { dev } => p[dev].checkpoint(now).map(|_| ()),
        Step::Compact { dev } => p[dev].compact(now).map(|_| ()),
        Step::Sync { a, b } => {
            let (x, y) = pair(p, a, b);
            reconcile(x, y, now).map(|_| ())
        }
        Step::Forget { by, target } => {
            let d = p[target].device().unwrap();
            p[by].forget(d, now).map(|_| ())
        }
        Step::Undeclared { val } => p[2].put("u", "r", [("x", Value::Int(val as i64))], now).map(|_| ()),
        Step::Future { dev } => p[dev].put("t", "r0", [("a", Value::Int(1))], now + 60 * 60 * 1000).map(|_| ()),
        Step::Tick => Ok(()),
    };
    let k = keys();
    let mut out = Vec::new();
    for (p, s) in w.peers.iter_mut().zip(w.stores.iter_mut()) {
        let c = p.take_changes(&k);
        s.apply(&c);
        out.push(c);
    }
    out
}

/// Everything a replica would store, and the state and exclusions it folds.
type Picture = (BTreeMap<Vec<u8>, Vec<u8>>, Vec<u8>, Vec<[u8; 32]>);

fn picture(r: &Replica) -> Picture {
    (as_map(&r.full_records(&keys())), r.state().encode(), r.excluded().iter().copied().collect())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), failure_persistence: None, ..ProptestConfig::default() })]

    /// Persist call by call, reload, and compare; then run the rest of the history
    /// on the originals and on the reloaded replicas, and compare again.
    #[test]
    fn persist_then_reload_gives_the_same_replica_and_the_same_future(
        steps in prop::collection::vec(step(), 1..50),
        cut in any::<prop::sample::Index>(),
        crash in any::<prop::sample::Index>(),
    ) {
        let k = cut.index(steps.len() + 1);
        let (head, tail) = steps.split_at(k);
        let mut w = world();
        for s in head {
            apply(&mut w, s);
        }
        // What was written call by call is exactly a full rewrite of the replica.
        for (n, (p, s)) in w.peers.iter().zip(&w.stores).enumerate() {
            prop_assert_eq!(&s.map, &as_map(&p.full_records(&keys())), "device {}: incremental records differ", n);
        }
        // Reload every replica from its records alone.
        let mut v = World {
            peers: (0..3).map(|i| reload(&w.stores[i], i as u8 + 1, has_schema(i), w.now)).collect(),
            stores: w.stores.clone(),
            now: w.now,
        };
        persist_all(&mut v);
        for n in 0..3 {
            prop_assert_eq!(picture(&w.peers[n]), picture(&v.peers[n]), "device {} reloaded differently", n);
            prop_assert_eq!(w.peers[n].is_wiped(), v.peers[n].is_wiped());
            prop_assert_eq!(w.peers[n].has_keys(), v.peers[n].has_keys());
            prop_assert_eq!(w.peers[n].heads(), v.peers[n].heads());
            prop_assert_eq!(w.peers[n].pending_ids(), v.peers[n].pending_ids());
            prop_assert_eq!(w.peers[n].quarantined_ids(), v.peers[n].quarantined_ids());
            prop_assert_eq!(w.peers[n].held_ids(), v.peers[n].held_ids());
        }
        // The same future on both.
        for (i, s) in tail.iter().enumerate() {
            let before = w.stores[0].clone();
            let changes = apply(&mut w, s);
            apply(&mut v, s);
            // A crash part-way through device 0's changeset: the prefix still loads,
            // and once it has the original's log again it folds to the same state.
            let c = &changes[0];
            if !c.reset && !c.records.is_empty() && i == crash.index(tail.len()) {
                let upto = crash.index(c.records.len() + 1);
                let mut torn = before.clone();
                torn.apply(&Changeset { reset: false, records: c.records[..upto].to_vec() });
                let mut r = reload(&torn, 1, true, w.now);
                // Compare at one later time against an intact copy (reloading is
                // exact, checked above), so quarantines release alike on both.
                let mut intact = reload(&w.stores[0], 1, true, w.now);
                let at = w.now + 2 * 60 * 60 * 1000;
                // A torn write may lose an op that was never delivered (it is in no
                // log); a peer sends it again, as the original's parked ops here.
                r.ingest(w.peers[0].log().into_iter().chain(w.peers[0].undelivered()), at);
                intact.ingest(Vec::<Vec<u8>>::new(), at);
                prop_assert_eq!(r.state().encode(), intact.state().encode(), "a torn write lost state");
            }
        }
        for n in 0..3 {
            prop_assert_eq!(picture(&w.peers[n]), picture(&v.peers[n]), "device {} diverged after reload", n);
            prop_assert_eq!(&w.stores[n].map, &v.stores[n].map);
        }
    }
}

#[test]
fn a_replica_wiped_by_a_snapshot_stays_wiped_after_a_reload() {
    let now = T0 + 100 * DAY;
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    reconcile(&mut a, &mut b, T0 + 1).unwrap();
    // A forgets B while B is away, checkpoints and prunes: B learns of it only
    // through a snapshot, so no Forget op ever sits in B's own log.
    a.forget(b.device().unwrap(), T0 + 2).unwrap();
    let mut c = joined(3, T0 + 3);
    reconcile(&mut a, &mut c, T0 + 4).unwrap();
    c.checkpoint(T0 + 5).unwrap();
    reconcile(&mut a, &mut c, T0 + 6).unwrap();
    a.compact(now).unwrap().unwrap();
    let (_, to_b) = reconcile(&mut a, &mut b, now).unwrap();
    assert!(to_b.ingest.wiped && b.is_wiped() && !b.has_keys());
    let mut p = MemPersist::default();
    p.apply(&b.take_changes(&keys()));
    assert_eq!(stored_device(&p.records()), Some((b.device(), true)));
    // The app still passes the words (it has not deleted them yet): no keys come back.
    let (r, _) = Replica::<hearth_sync_kernel::store::MemStore>::load(
        &keys(),
        Config::default(),
        Some((root(), signer(2))),
        p.records(),
        now,
    )
    .unwrap();
    assert!(r.is_wiped() && !r.has_keys());
    assert_eq!(r.device(), b.device());
}

#[test]
fn records_load_only_with_their_own_household_and_device() {
    let mut a = joined(1, T0);
    a.put("t", "r", [("a", v(1))], T0 + 1).unwrap();
    let mut p = MemPersist::default();
    p.apply(&a.take_changes(&keys()));
    let load = |k: &SealKeys, dev: Option<(HouseholdRoot, Box<dyn DeviceSigner>)>, recs: Vec<(Vec<u8>, Vec<u8>)>| {
        Replica::<hearth_sync_kernel::store::MemStore>::load(k, Config::default(), dev, recs, T0 + 2).map(|_| ())
    };
    assert_eq!(load(&keys(), Some((root(), signer(1))), p.records()), Ok(()));
    let other = SealKeys::derive(&HouseholdRoot::from_seed([0xEE; 64]), APP);
    assert_eq!(load(&other, None, p.records()), Err(PersistError::WrongHousehold));
    assert_eq!(load(&keys(), Some((root(), signer(9))), p.records()), Err(PersistError::WrongDevice));
    assert_eq!(load(&keys(), None, vec![]), Err(PersistError::NoMeta));
    // A flipped byte in a sealed op body is caught by the seal, not folded.
    let mut recs = p.records();
    let (_, v) = recs.iter_mut().find(|(k, _)| k[0] == hearth_sync_kernel::persist::TAG_OP).unwrap();
    let n = v.len();
    v[n - 1] ^= 1;
    let err = load(&keys(), None, recs).unwrap_err();
    assert!(matches!(err, PersistError::Seal(..) | PersistError::Malformed(_)), "{err:?}");
    // A record of a newer version is refused rather than misread.
    let mut recs = p.records();
    let (_, v) = recs.iter_mut().find(|(k, _)| k[0] == hearth_sync_kernel::persist::TAG_META).unwrap();
    v[1] = 0x02;
    assert!(matches!(load(&keys(), None, recs), Err(PersistError::Malformed(_))));
}

/// Compaction writes the base before it drops any body, so a crash between the two
/// leaves extra bodies (pruned again on load), never a base that lacks its ops.
#[test]
fn compaction_writes_the_base_before_the_prunes() {
    let mut a = joined(1, T0);
    let mut b = joined(2, T0);
    a.put("t", "r", [("a", v(1))], T0 + 1).unwrap();
    reconcile(&mut a, &mut b, T0 + 2).unwrap();
    b.checkpoint(T0 + 3).unwrap();
    reconcile(&mut a, &mut b, T0 + 4).unwrap();
    let mut p = MemPersist::default();
    p.apply(&a.take_changes(&keys()));
    a.compact(T0 + 4 + 90 * DAY).unwrap().unwrap();
    let c = a.take_changes(&keys());
    let tags: Vec<u8> = c.records.iter().map(|r: &Record| r.key[0]).collect();
    let base_at = tags.iter().position(|t| *t == hearth_sync_kernel::persist::TAG_BASE).expect("a base record");
    assert!(base_at < tags.len() - 1, "some body is pruned: {tags:?}");
    assert!(tags[base_at + 1..].iter().all(|t| *t == hearth_sync_kernel::persist::TAG_OP), "{tags:?}");
    // Cut right after the base.
    let mut torn = p.clone();
    torn.apply(&Changeset { reset: false, records: c.records[..=base_at].to_vec() });
    let (mut r, _) =
        Replica::<hearth_sync_kernel::store::MemStore>::load(&keys(), Config::default(), None, torn.records(), T0)
            .unwrap();
    assert_eq!(r.state().encode(), a.state().encode());
    assert_eq!(r.base_checkpoint(), a.base_checkpoint());
    // Load pruned the leftover bodies itself and says so in its next changeset.
    let repair = r.take_changes(&keys());
    torn.apply(&repair);
    p.apply(&c);
    assert_eq!(torn.map.get(&[hearth_sync_kernel::persist::TAG_META][..]), p.map.get(&[1u8][..]));
    assert_eq!(r.log(), a.log());
}
