//! Performance at 10^4 ops (and more, via `HS_PERF_N`). Ignored by default: run with
//! `cargo test --release -p hearth_sync_kernel --test perf -- --ignored --nocapture`.
//! Numbers go in the report, not in assertions: the only assertions are that the
//! replicas agree at the end.

mod common;
use common::*;

use hearth_sync_kernel::replica::Replica;
use hearth_sync_kernel::sync::reconcile;
use std::time::Instant;

fn n() -> usize {
    std::env::var("HS_PERF_N").ok().and_then(|s| s.parse().ok()).unwrap_or(10_000)
}

/// Author `count` ops on `r`: mostly edits over a rolling set of rows, some deletes
/// (each observes every put to its row, so V5 walks the past) and some set adds.
fn author(r: &mut Replica, count: usize, tag: &str, now: &mut u64) {
    let rows = (count / 10).max(1);
    for i in 0..count {
        *now += 1;
        let row = format!("{tag}{}", i % rows);
        match i % 10 {
            7 => r.delete("t", &row, *now).map(|_| ()),
            8 => r.set_add("s", t(&format!("{tag}{i}")), *now).map(|_| ()),
            _ => r.put("t", &row, [("f", v(i as i64)), ("g", t(tag))], *now).map(|_| ()),
        }
        .unwrap();
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

#[test]
#[ignore]
fn perf_author_ingest_and_sync() {
    let n = n();
    let mut now = T0;

    let mut a = joined(1, now);
    let t0 = Instant::now();
    author(&mut a, n, "a", &mut now);
    let author_ms = ms(t0);

    let log = a.log();
    let mut c = joined(3, now);
    let t0 = Instant::now();
    let rep = c.ingest(&log, now);
    let ingest_ms = ms(t0);
    assert_eq!(rep.rejected, vec![]);
    assert_eq!(c.state().table("t"), a.state().table("t"));

    // Two devices, n/2 concurrent ops each, then one full sync.
    let mut x = joined(1, T0);
    let mut y = joined(2, T0);
    reconcile(&mut x, &mut y, T0).unwrap();
    let mut now = T0;
    author(&mut x, n / 2, "x", &mut now);
    author(&mut y, n / 2, "y", &mut now);
    let t0 = Instant::now();
    reconcile(&mut x, &mut y, now).unwrap();
    let sync_ms = ms(t0);
    assert_eq!(x.state().encode(), y.state().encode());

    // Then an incremental round: 100 more each.
    author(&mut x, 100, "x2", &mut now);
    author(&mut y, 100, "y2", &mut now);
    let t0 = Instant::now();
    reconcile(&mut x, &mut y, now).unwrap();
    let inc_ms = ms(t0);
    assert_eq!(x.state().encode(), y.state().encode());

    println!(
        "PERF n={n}: author {author_ms:.0} ms ({:.1} us/op); ingest one batch {ingest_ms:.0} ms ({:.1} us/op); \
         full sync of 2x{} concurrent {sync_ms:.0} ms; incremental sync of 2x100 on top {inc_ms:.0} ms",
        author_ms * 1000.0 / n as f64,
        ingest_ms * 1000.0 / n as f64,
        n / 2,
    );
}

mod api_perf {
    use super::common::{seed, APP, T0};
    use super::{ms, n};
    use hearth_sync_kernel::api::{
        Collection, Field, FieldDef, Kernel, Merge, OpenArgs, Record, Step, Value, ValueType, Write,
    };
    use hearth_sync_kernel::keys::{DeviceSigner, SoftSigner};
    use hearth_sync_kernel::persist::{Changeset, MemPersist, Persist};
    use hearth_sync_kernel::schema::Schema;
    use std::time::Instant;

    fn schema(horizon_ms: u64) -> Schema {
        let f = |n: &str| FieldDef { name: n.into(), ty: ValueType::Any, nullable: true };
        Schema {
            collections: vec![
                Collection { name: "t".into(), merge: Merge::Lww { fields: vec![f("f"), f("g")], container: None } },
                Collection { name: "s".into(), merge: Merge::AddWinsSet { element: ValueType::Any } },
            ],
            horizon_ms,
            keep_full_history: false,
        }
    }

    struct Dev {
        k: Kernel,
        signer: SoftSigner,
        store: MemPersist,
        signatures: usize,
        sent: Vec<Vec<u8>>,
    }

    impl Dev {
        fn open(n: u8, horizon: u64, records: Vec<Record>, now: u64) -> (Kernel, SoftSigner) {
            let signer = SoftSigner::from_secret([n; 32]);
            let k = Kernel::open(OpenArgs {
                app: APP.into(),
                seed: seed().to_vec(),
                device: signer.device().to_vec(),
                schema: schema(horizon),
                records,
                now,
            })
            .unwrap();
            (k, signer)
        }
        fn new(n: u8, horizon: u64) -> Dev {
            let (k, signer) = Dev::open(n, horizon, vec![], T0);
            let mut d = Dev { k, signer, store: MemPersist::default(), signatures: 0, sent: Vec::new() };
            let s = d.k.enroll_self(format!("d{n}"), T0);
            d.drive(s);
            d
        }
        fn drive(&mut self, mut s: Result<Step, hearth_sync_kernel::api::ApiError>) {
            loop {
                match s.unwrap() {
                    Step::Sign { signable } => {
                        self.signatures += 1;
                        s = self.k.finish(self.signer.sign(&signable).to_vec());
                    }
                    Step::Done(o) => {
                        self.store.apply(&Changeset { reset: o.reset_records, records: o.records });
                        self.sent.extend(o.outgoing.into_iter().map(|s| s.sealed));
                        return;
                    }
                }
            }
        }
        fn put(&mut self, row: &str, i: i64, now: u64) {
            let fields = vec![Field { name: "f".into(), value: Value::Int(i) }];
            let s = self.k.write(Write::Put { table: "t".into(), row: row.into(), fields }, now);
            self.drive(s);
        }
    }

    #[test]
    #[ignore]
    fn perf_api_write_reload_relay_and_rebase() {
        let n = n();
        let mut a = Dev::new(1, 20_000);
        let mut now = T0;
        let t0 = Instant::now();
        for i in 0..n {
            now += 1;
            a.put(&format!("r{}", i % (n / 10).max(1)), i as i64, now);
        }
        let write_ms = ms(t0);
        let records: Vec<Record> =
            a.store.records().into_iter().map(|(key, v)| Record { key, value: Some(v) }).collect();
        let bytes: usize = a.store.map.iter().map(|(k, v)| k.len() + v.len()).sum();
        let t0 = Instant::now();
        let (reloaded, _) = Dev::open(1, 20_000, records, now);
        let reload_ms = ms(t0);
        assert_eq!(reloaded.heads(), a.k.heads());

        // A second device takes everything from the relay in one batch.
        let mut b = Dev::new(2, 20_000);
        let t0 = Instant::now();
        let o = b.k.ingest(a.sent.clone(), now).unwrap();
        let relay_ms = ms(t0);
        assert!(o.rejected.is_empty());
        assert_eq!(b.k.view_all(), a.k.view_all());

        // A returning device with m unsynced edits to untouched rows adopts a snapshot:
        // m re-issues, one signature each, replayed from the start every time.
        let m = 50;
        let mut c = Dev::new(3, 20_000);
        let s = c.k.accept(a.k.offer(c.k.request(a.k.hello(now)).unwrap().message).unwrap(), now);
        c.drive(s);
        let s = a.k.accept(c.k.offer(a.k.request(c.k.hello(now)).unwrap().message).unwrap(), now);
        a.drive(s);
        for i in 0..m {
            c.put(&format!("c{i}"), i, now + 1 + i as u64);
        }
        let s = a.k.checkpoint(now + 100);
        a.drive(s);
        let later = now + 100 + 20_001;
        let req = c.k.request(a.k.hello(later)).unwrap();
        assert!(req.needs_snapshot);
        let offer = a.k.offer(req.message).unwrap();
        c.signatures = 0;
        let t0 = Instant::now();
        let s = c.k.accept(offer, later);
        c.drive(s);
        let rebase_ms = ms(t0);
        println!(
            "PERF-API n={n}: write+sign+records {write_ms:.0} ms ({:.1} us/op); records {} KiB; \
             reload {reload_ms:.0} ms; relay ingest of {} sealed ops {relay_ms:.0} ms; \
             rebase of {m} edits: {} signatures, {rebase_ms:.0} ms",
            write_ms * 1000.0 / n as f64,
            bytes / 1024,
            a.sent.len(),
            c.signatures,
        );
    }
}
