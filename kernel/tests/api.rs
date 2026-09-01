//! Two devices driven purely through `api` (ADR 0011), as the Dart side will drive
//! them: the device key stays outside the kernel (a software signer stands in for
//! the platform key store), records go to a store the "app" owns, and every row, set
//! and stream change is applied to a mirror of the app's tables, which must always
//! equal a fresh view of the state.

mod common;
use common::{root, seed, APP, T0};

use hearth_sync_kernel::api::Schema;
use hearth_sync_kernel::api::{
    sealed_handover, stored_info, ApiError, Collection, ContainerDef, Field, FieldDef, Kernel, Merge, OpenArgs,
    Outcome, Record, Step, Value, ValueType, Write,
};
use hearth_sync_kernel::keys::{DeviceSigner, SoftSigner};
use hearth_sync_kernel::persist::{Changeset, MemPersist, Persist};
use std::collections::{BTreeMap, BTreeSet};

const H: u64 = 20_000;

fn schema(horizon_ms: u64) -> Schema {
    let f = |n: &str, ty| FieldDef { name: n.into(), ty, nullable: true };
    let t = |name: &str, fields, container: Option<(&str, &str)>| Collection {
        name: name.into(),
        merge: Merge::Lww {
            fields,
            container: container.map(|(f, t)| ContainerDef { field: f.into(), table: t.into() }),
        },
    };
    Schema {
        collections: vec![
            t("rooms", vec![f("name", ValueType::Text)], None),
            t("boxes", vec![f("name", ValueType::Text), f("room", ValueType::Text)], Some(("room", "rooms"))),
            t(
                "items",
                vec![f("name", ValueType::Text), f("box", ValueType::Text), f("value", ValueType::Int)],
                Some(("box", "boxes")),
            ),
            Collection { name: "tags".into(), merge: Merge::AddWinsSet { element: ValueType::Text } },
            Collection { name: "log".into(), merge: Merge::AppendOnly { record: ValueType::Any } },
        ],
        horizon_ms,
        keep_full_history: false,
    }
}

/// The app's copy of its tables, fed only by outcomes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Mirror {
    rows: BTreeMap<(String, String), Vec<Field>>,
    sets: BTreeSet<(String, Value)>,
    streams: BTreeMap<Vec<u8>, (String, Value)>,
}

impl Mirror {
    fn apply(&mut self, o: &Outcome) {
        if o.replace_view {
            *self = Mirror::default();
        }
        for r in &o.rows {
            let k = (r.table.clone(), r.row.clone());
            if r.visible {
                self.rows.insert(k, r.fields.clone());
            } else {
                self.rows.remove(&k);
            }
        }
        for s in &o.sets {
            if s.present {
                self.sets.insert((s.set.clone(), s.element.clone()));
            } else {
                self.sets.remove(&(s.set.clone(), s.element.clone()));
            }
        }
        for s in &o.streams {
            if s.present {
                self.streams.insert(s.id.clone(), (s.stream.clone(), s.record.clone()));
            } else {
                self.streams.remove(&s.id);
            }
        }
    }
}

/// One device: the kernel, the platform key, the app's record store and tables.
struct Dev {
    k: Kernel,
    n: u8,
    signer: SoftSigner,
    store: MemPersist,
    mirror: Mirror,
    /// Signatures asked for by the last call.
    signed: usize,
    /// Every sealed op this device handed out for the relay, and their ids.
    sent: Vec<(Vec<u8>, Vec<u8>)>,
    /// The schema the app version on this device registers.
    schema: Schema,
}

fn records(p: &MemPersist) -> Vec<Record> {
    p.records().into_iter().map(|(key, v)| Record { key, value: Some(v) }).collect()
}

impl Dev {
    fn new(n: u8, horizon: u64) -> Dev {
        let signer = SoftSigner::from_secret([n; 32]);
        let k = Kernel::open(OpenArgs {
            app: APP.into(),
            seed: seed().to_vec(),
            device: signer.device().to_vec(),
            schema: schema(horizon),
            records: vec![],
            now: T0,
        })
        .unwrap();
        let mut d = Dev {
            k,
            n,
            signer,
            store: MemPersist::default(),
            mirror: Mirror::default(),
            signed: 0,
            sent: Vec::new(),
            schema: schema(horizon),
        };
        let o = d.k.flush();
        d.take(o);
        let step = d.k.enroll_self(format!("device {n}"), T0);
        d.drive(step);
        d
    }

    /// What the app does with an outcome: one transaction for records and tables.
    fn take(&mut self, o: Outcome) -> Outcome {
        self.store.apply(&Changeset { reset: o.reset_records, records: o.records.clone() });
        self.mirror.apply(&o);
        self.sent.extend(o.outgoing.iter().map(|s| (s.id.clone(), s.sealed.clone())));
        self.check();
        o
    }

    fn check(&self) {
        let v = self.k.view_all();
        let mut fresh = Mirror::default();
        fresh.apply(&Outcome { rows: v.rows, sets: v.sets, streams: v.streams, ..Outcome::default() });
        assert_eq!(self.mirror, fresh, "device {}: the tables the outcomes built differ from the view", self.n);
    }

    /// Sign until done, as the platform would.
    fn drive(&mut self, mut step: Result<Step, ApiError>) -> Outcome {
        self.signed = 0;
        loop {
            match step.unwrap() {
                Step::Sign { signable } => {
                    self.signed += 1;
                    step = self.k.finish(self.signer.sign(&signable).to_vec());
                }
                Step::Done(o) => return self.take(o),
            }
        }
    }

    fn write(&mut self, w: Write, now: u64) -> Outcome {
        let s = self.k.write(w, now);
        self.drive(s)
    }

    fn put(&mut self, table: &str, row: &str, fields: &[(&str, Value)], now: u64) -> Outcome {
        let fields = fields.iter().map(|(n, v)| Field { name: n.to_string(), value: v.clone() }).collect();
        self.write(Write::Put { table: table.into(), row: row.into(), fields }, now)
    }

    fn row(&self, table: &str, row: &str) -> Option<BTreeMap<String, Value>> {
        self.mirror
            .rows
            .get(&(table.to_string(), row.to_string()))
            .map(|f| f.iter().map(|f| (f.name.clone(), f.value.clone())).collect())
    }

    /// Close and reopen from the app's stored records.
    fn reopen(&mut self, now: u64) {
        self.reopen_with(now, false)
    }

    fn reopen_with(&mut self, now: u64, expect_replace: bool) {
        let k = Kernel::open(OpenArgs {
            app: APP.into(),
            seed: seed().to_vec(),
            device: self.signer.device().to_vec(),
            schema: self.schema.clone(),
            records: records(&self.store),
            now,
        })
        .unwrap();
        self.k = k;
        let o = self.k.flush();
        assert_eq!(o.replace_view, expect_replace);
        self.take(o);
    }
}

/// A full two-way sync, each side running its half through `api` only, in the order
/// `sync::reconcile` uses: whoever lacks an old checkpoint pulls first (a snapshot),
/// a wiped side only hands over, then ops move both ways.
fn sync(a: &mut Dev, b: &mut Dev, now: u64) {
    let ra = a.k.request(b.k.hello(now)).unwrap();
    if ra.needs_snapshot && !a.k.status().wiped {
        let s = a.k.accept(b.k.offer(ra.message).unwrap(), now);
        a.drive(s);
    }
    let rb = b.k.request(a.k.hello(now)).unwrap();
    if rb.needs_snapshot && !b.k.status().wiped {
        let s = b.k.accept(a.k.offer(rb.message).unwrap(), now);
        b.drive(s);
    }
    let rb = b.k.request(a.k.hello(now)).unwrap();
    let ra = a.k.request(b.k.hello(now)).unwrap();
    if ra.needs_snapshot || rb.needs_snapshot {
        if ra.needs_snapshot {
            assert!(a.k.status().wiped, "only a wiped side may still lack a checkpoint");
            let s = b.k.accept(a.k.handover(rb.message.clone(), now).unwrap(), now);
            b.drive(s);
        }
        if rb.needs_snapshot {
            assert!(b.k.status().wiped, "only a wiped side may still lack a checkpoint");
            let s = a.k.accept(b.k.handover(ra.message, now).unwrap(), now);
            a.drive(s);
        }
        return;
    }
    let s = b.k.accept(a.k.offer(rb.message).unwrap(), now);
    b.drive(s);
    let ra = a.k.request(b.k.hello(now)).unwrap();
    let s = a.k.accept(b.k.offer(ra.message).unwrap(), now);
    a.drive(s);
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

#[test]
fn two_devices_sync_edits_deletes_containers_sets_and_streams_through_the_api() {
    let (mut a, mut b) = (Dev::new(1, 90 * 86_400_000), Dev::new(2, 90 * 86_400_000));
    let mut now = T0 + 1;
    sync(&mut a, &mut b, now);
    assert_eq!(a.k.devices().len(), 2);
    assert!(a.k.devices().iter().any(|d| d.me && d.label == "device 1"));

    a.put("rooms", "den", &[("name", text("Den"))], now);
    a.put("boxes", "b1", &[("name", text("Crate")), ("room", text("den"))], now + 1);
    a.put("items", "i1", &[("name", text("Lamp")), ("box", text("b1")), ("value", Value::Int(40))], now + 2);
    a.write(Write::SetAdd { set: "tags".into(), element: text("fragile") }, now + 3);
    let o = a.write(Write::Append { stream: "log".into(), record: text("moved in") }, now + 4);
    assert_eq!(a.signed, 1, "a local write needs one signature");
    assert_eq!(o.outgoing.len(), 1, "the op to hand the relay");
    now += 10;
    sync(&mut a, &mut b, now);
    assert_eq!(b.row("items", "i1").unwrap()["value"], Value::Int(40));
    assert!(b.mirror.sets.contains(&("tags".into(), text("fragile"))));
    assert_eq!(b.mirror.streams.len(), 1);

    // Edit beats a concurrent delete.
    a.write(Write::Delete { table: "items".into(), row: "i1".into() }, now + 1);
    b.put("items", "i1", &[("value", Value::Int(45))], now + 1);
    now += 10;
    sync(&mut a, &mut b, now);
    for d in [&a, &b] {
        assert_eq!(d.row("items", "i1").unwrap()["value"], Value::Int(45));
    }

    // Deleting the room hides its box and the item in the box, on both devices.
    b.write(Write::Delete { table: "rooms".into(), row: "den".into() }, now + 1);
    assert_eq!(b.row("items", "i1"), None, "hidden by the deleted room, at once");
    now += 10;
    sync(&mut a, &mut b, now);
    for d in [&a, &b] {
        assert_eq!(d.row("rooms", "den"), None);
        assert_eq!(d.row("boxes", "b1"), None);
        assert_eq!(d.row("items", "i1"), None);
    }
    // Undo brings the subtree back.
    a.write(Write::Restore { table: "rooms".into(), row: "den".into() }, now + 1);
    now += 10;
    sync(&mut a, &mut b, now);
    assert_eq!(b.row("items", "i1").unwrap()["name"], text("Lamp"));

    // OR-set: a remove and a concurrent re-add; the add wins.
    a.write(Write::SetRemove { set: "tags".into(), element: text("fragile") }, now + 1);
    b.write(Write::SetAdd { set: "tags".into(), element: text("fragile") }, now + 1);
    now += 10;
    sync(&mut a, &mut b, now);
    assert!(a.mirror.sets.contains(&("tags".into(), text("fragile"))));

    assert_eq!(a.mirror, b.mirror);
    assert_eq!(a.k.heads(), b.k.heads());

    // Close both and reopen from the records the "app" stored: same tables, and they
    // carry on syncing.
    a.reopen(now + 1);
    b.reopen(now + 1);
    assert_eq!(a.mirror, b.mirror);
    b.put("items", "i2", &[("name", text("Rug"))], now + 2);
    now += 10;
    sync(&mut a, &mut b, now);
    assert_eq!(a.row("items", "i2").unwrap()["name"], text("Rug"));
}

#[test]
fn names_and_text_are_normalised_to_nfc_at_the_boundary() {
    let (mut a, mut b) = (Dev::new(1, H), Dev::new(2, H));
    // "café" typed with a combining accent on one device and precomposed on the other.
    let decomposed = "cafe\u{301}";
    let composed = "caf\u{e9}";
    a.put("rooms", decomposed, &[("name", text(decomposed))], T0 + 1);
    b.put("rooms", composed, &[("name", text("Café"))], T0 + 2);
    sync(&mut a, &mut b, T0 + 3);
    for d in [&a, &b] {
        let rows: Vec<_> = d.mirror.rows.keys().filter(|(t, _)| t == "rooms").collect();
        assert_eq!(rows, vec![&("rooms".to_string(), composed.to_string())], "one row, spelled NFC");
        assert_eq!(d.row("rooms", composed).unwrap()["name"], text("Café"), "the later write wins the one field");
    }
    let s = a.k.write(Write::SetAdd { set: "tags".into(), element: text(decomposed) }, T0 + 4);
    let o = a.drive(s);
    assert_eq!(o.sets[0].element, text(composed));
}

#[test]
fn a_returning_device_rebases_through_several_signatures_and_keeps_its_review_list() {
    let (mut a, mut b) = (Dev::new(1, H), Dev::new(2, H));
    let mut now = T0 + 1;
    a.put("items", "i1", &[("name", text("Lamp"))], now);
    a.put("items", "i2", &[("name", text("Rug"))], now + 1);
    now += 10;
    sync(&mut a, &mut b, now);
    // B goes away and edits: one field nobody else touches, one A changes too, and
    // a set add.
    b.put("items", "i1", &[("name", text("Lamp (B)"))], now + 1);
    b.put("items", "i2", &[("name", text("Rug (B)"))], now + 2);
    b.write(Write::SetAdd { set: "tags".into(), element: text("new") }, now + 3);
    a.put("items", "i2", &[("name", text("Rug (A)"))], now + 4);
    let s = a.k.checkpoint(now + 5);
    a.drive(s);
    // A's checkpoint passes the horizon before B returns.
    now += H + 100;
    let rb = b.k.request(a.k.hello(now)).unwrap();
    assert!(rb.needs_snapshot, "B lacks A's old checkpoint");
    let s = b.k.accept(a.k.offer(rb.message).unwrap(), now);
    let o = b.drive(s);
    assert!(b.signed >= 2, "the rebase re-issued several ops, one signature each: {}", b.signed);
    assert_eq!(o.reissued.len(), 2, "i1's edit and the set add: {o:?}");
    assert!(o.reset_records, "the snapshot replaced the log");
    assert_eq!(o.review_added.len(), 1, "{:?}", o.review_added);
    let item = &o.review_added[0];
    assert_eq!((item.kind.as_str(), item.field.as_deref()), ("field", Some("name")));
    assert_eq!(item.mine, Some(text("Rug (B)")));
    assert_eq!(item.current, Some(text("Rug (A)")));
    assert_eq!(b.row("items", "i1").unwrap()["name"], text("Lamp (B)"));
    assert_eq!(b.row("items", "i2").unwrap()["name"], text("Rug (A)"));
    sync(&mut a, &mut b, now + 1);
    assert_eq!(a.mirror, b.mirror);

    // The review list survives a restart, and dismissing an entry deletes its record.
    b.reopen(now + 2);
    assert_eq!(b.k.review(), vec![item.clone()]);
    let o = b.k.dismiss_review(item.key.clone());
    assert!(o.records.iter().any(|r| r.key == item.key && r.value.is_none()));
    b.take(o);
    b.reopen(now + 3);
    assert!(b.k.review().is_empty());
}

#[test]
fn a_forgotten_device_learns_it_through_the_api_and_can_no_longer_write() {
    let (mut a, mut b) = (Dev::new(1, H), Dev::new(2, H));
    sync(&mut a, &mut b, T0 + 1);
    let bid = b.signer.device().to_vec();
    let s = a.k.forget_device(bid.clone(), T0 + 2);
    a.drive(s);
    assert!(a.k.devices().iter().any(|d| d.device == bid && d.forgotten));
    let rb = b.k.request(a.k.hello(T0 + 3)).unwrap();
    let s = b.k.accept(a.k.offer(rb.message).unwrap(), T0 + 3);
    let o = b.drive(s);
    assert!(o.wiped, "the outcome tells the app to delete the words");
    assert!(b.k.status().wiped);
    let err = b.k.write(Write::SetAdd { set: "tags".into(), element: text("x") }, T0 + 4).unwrap_err();
    assert_eq!(err, ApiError::NoKeys);
    // Without keys the stored records still say who they belonged to, and that the
    // device was wiped.
    let info = stored_info(records(&b.store)).unwrap();
    assert_eq!((info.device, info.wiped), (Some(bid), true));

    // Forgetting yourself wipes at once.
    let mut c = Dev::new(3, H);
    let s = c.k.forget_self(T0 + 5);
    let o = c.drive(s);
    assert!(o.wiped && c.k.status().wiped);
}

#[test]
fn the_relay_path_carries_sealed_ops_and_refuses_what_does_not_open() {
    let (mut a, mut b) = (Dev::new(1, H), Dev::new(2, H));
    a.put("rooms", "den", &[("name", text("Den"))], T0 + 1);
    a.write(Write::Append { stream: "log".into(), record: Value::Int(7) }, T0 + 2);
    // Everything A wrote, its enrolment included, as the relay would hold it.
    assert_eq!(a.sent.len(), 3);
    let mut up: Vec<Vec<u8>> = a.sent.iter().map(|(_, s)| s.clone()).collect();
    // A tampered envelope is refused as bad_seal, named by the id it claims.
    let n = up[1].len();
    up[1][n - 1] ^= 1;
    let o = b.k.ingest(up.clone(), T0 + 3).unwrap();
    let o = b.take(o);
    assert_eq!(o.rejected.len(), 1);
    assert_eq!((o.rejected[0].code.as_str(), &o.rejected[0].id), ("bad_seal", &a.sent[1].0));
    assert_eq!(b.row("rooms", "den"), None, "the put did not open");
    assert_eq!(b.mirror.streams.len(), 0, "the append waits for its parent");
    // The intact envelope arrives on a retry.
    let o = b.k.ingest(vec![a.sent[1].1.clone()], T0 + 4).unwrap();
    b.take(o);
    assert_eq!(b.row("rooms", "den").unwrap()["name"], text("Den"));
    assert_eq!(b.mirror.streams.len(), 1);
    // A sync message from another household does not open.
    let other = Kernel::open(OpenArgs {
        app: APP.into(),
        seed: vec![0xEE; 64],
        device: SoftSigner::from_secret([9; 32]).device().to_vec(),
        schema: schema(H),
        records: vec![],
        now: T0,
    })
    .unwrap();
    assert_eq!(b.k.request(other.hello(T0 + 5)).unwrap_err(), ApiError::BadMessage);
    let _ = root();
}

#[test]
fn the_api_refuses_bad_input_and_leaves_a_waiting_write_waiting() {
    let mut a = Dev::new(1, H);
    // A name the schema does not declare.
    let err = a.k.write(Write::Put { table: "photos".into(), row: "p".into(), fields: vec![] }, T0 + 1);
    assert!(matches!(err, Err(ApiError::BadArgument(_))), "{err:?}");
    let f = vec![Field { name: "x".into(), value: Value::Int(1) }];
    let err = a.k.write(Write::Put { table: "photos".into(), row: "p".into(), fields: f }, T0 + 1).unwrap_err();
    assert_eq!(err, ApiError::Undeclared);
    // A declared name misused.
    let f = vec![Field { name: "name".into(), value: Value::Int(1) }];
    let err = a.k.write(Write::Put { table: "rooms".into(), row: "r".into(), fields: f }, T0 + 1).unwrap_err();
    assert_eq!(err, ApiError::Rejected("schema_violation".into()));
    // A wrong signature is refused and the write keeps waiting for the right one.
    let Step::Sign { signable } = a.k.write(Write::SetAdd { set: "tags".into(), element: text("x") }, T0 + 2).unwrap()
    else {
        panic!("a write asks for a signature")
    };
    let wrong = SoftSigner::from_secret([9; 32]).sign(&signable);
    assert_eq!(a.k.finish(wrong.to_vec()).unwrap_err(), ApiError::BadSignature);
    assert!(a.k.status().awaiting_signature);
    assert_eq!(
        a.k.write(Write::SetAdd { set: "tags".into(), element: text("y") }, T0 + 3).unwrap_err(),
        ApiError::AwaitingSignature
    );
    let s = a.k.finish(a.signer.sign(&signable).to_vec());
    a.drive(s);
    assert!(a.mirror.sets.contains(&("tags".into(), text("x"))));
    assert_eq!(a.k.finish(vec![0; 64]).unwrap_err(), ApiError::NothingToFinish);
    assert_eq!(a.k.status().rejected, 0, "no junk op was recorded");
    // A schema that does not validate is refused at open.
    let mut bad = schema(H);
    bad.horizon_ms = 0;
    let err = Kernel::open(OpenArgs {
        app: APP.into(),
        seed: seed().to_vec(),
        device: a.signer.device().to_vec(),
        schema: bad,
        records: vec![],
        now: T0,
    });
    assert!(matches!(err, Err(ApiError::Schema(_))));
}

/// An upgrade seen through the api: a device on the old schema holds a newer
/// device's ops (V7), then the app updates and reopens with the new schema. The
/// first outcome replaces the app's tables with the whole view, which now shows
/// what the old version could not.
#[test]
fn reopening_with_a_newer_schema_releases_held_ops_and_replaces_the_view() {
    let (mut old, mut new) = (Dev::new(1, H), Dev::new(2, H));
    let mut v2 = schema(H);
    v2.collections.push(Collection {
        name: "photos".into(),
        merge: Merge::Lww {
            fields: vec![FieldDef { name: "item".into(), ty: ValueType::Text, nullable: false }],
            container: None,
        },
    });
    // Device 2 runs the new version.
    new.schema = v2.clone();
    old.schema = v2.clone();
    new.k = Kernel::open(OpenArgs {
        app: APP.into(),
        seed: seed().to_vec(),
        device: new.signer.device().to_vec(),
        schema: v2.clone(),
        records: records(&new.store),
        now: T0 + 1,
    })
    .unwrap();
    let o = new.k.flush();
    assert!(o.replace_view, "the schema changed under device 2");
    new.take(o);
    new.put("photos", "p1", &[("item", text("i1"))], T0 + 2);
    new.put("rooms", "den", &[("name", text("Den"))], T0 + 3);
    let rb = old.k.request(new.k.hello(T0 + 4)).unwrap();
    let s = old.k.accept(new.k.offer(rb.message).unwrap(), T0 + 4);
    let o = old.drive(s);
    assert_eq!(o.held.len(), 1, "the photo is held on the old version: {o:?}");
    assert_eq!(old.row("rooms", "den"), None, "and what was built on it waits");
    assert_eq!(old.k.status().held, 1);
    // The app updates and reopens with the new schema.
    old.k = Kernel::open(OpenArgs {
        app: APP.into(),
        seed: seed().to_vec(),
        device: old.signer.device().to_vec(),
        schema: v2,
        records: records(&old.store),
        now: T0 + 5,
    })
    .unwrap();
    assert!(old.k.status().schema_changed);
    let o = old.k.flush();
    assert!(o.replace_view);
    old.take(o);
    assert_eq!(old.row("photos", "p1").unwrap()["item"], text("i1"));
    assert_eq!(old.row("rooms", "den").unwrap()["name"], text("Den"));
    assert_eq!(old.k.status().held, 0);
    assert_eq!(old.mirror, new.mirror);
    // The next reopen, with the same schema, does not replace anything.
    old.reopen_with(T0 + 6, false);
}

#[test]
fn a_wiped_device_hands_over_its_own_sealed_ops_and_forget_after_a_restart_without_keys() {
    let (mut a, mut b) = (Dev::new(1, H), Dev::new(2, H));
    sync(&mut a, &mut b, T0 + 1);
    // A writes offline, then forgets itself before any of it reaches B.
    a.put("rooms", "den", &[("name", text("Den"))], T0 + 2);
    let s = a.k.forget_self(T0 + 3);
    let o = a.drive(s);
    assert!(o.wiped);
    let aid = a.signer.device().to_vec();
    // The app restarts: the words are gone, only the records are left.
    let recs = records(&a.store);
    assert!(stored_info(recs.clone()).unwrap().wiped);
    let out = sealed_handover(recs, a.schema.clone(), T0 + 4).unwrap();
    // Every envelope is the one the original session handed the relay, byte for byte
    // (the nonce is synthetic), so the relay never sees two envelopes for one op.
    // The Forget's past also carries B's ops that A had seen; B sent those.
    let sent: BTreeMap<Vec<u8>, Vec<u8>> = a.sent.iter().chain(b.sent.iter()).cloned().collect();
    let got: BTreeSet<Vec<u8>> = out.iter().map(|s| s.id.clone()).collect();
    assert!(a.sent.iter().all(|(id, _)| got.contains(id)), "every op A wrote is handed over");
    for s in &out {
        assert_eq!(sent.get(&s.id), Some(&s.sealed), "an envelope differs from the one first sent");
    }
    // B, which never saw the edit or the Forget, takes them from the relay.
    let o = b.k.ingest(out.iter().map(|s| s.sealed.clone()).collect(), T0 + 5).unwrap();
    let o = b.take(o);
    assert!(o.rejected.is_empty() && o.pending.is_empty(), "{o:?}");
    assert!(b.k.devices().iter().any(|d| d.device == aid && d.forgotten), "B learns A forgot itself");
    assert_eq!(b.row("rooms", "den").unwrap()["name"], text("Den"), "A's offline edit arrives");
    // A device that never synced with A at all gets A's enrolment too.
    let mut c = Dev::new(3, H);
    let recs = records(&a.store);
    let out = sealed_handover(recs, a.schema.clone(), T0 + 6).unwrap();
    let o = c.k.ingest(out.iter().map(|s| s.sealed.clone()).collect(), T0 + 6).unwrap();
    let o = c.take(o);
    // C's own household log does not have B's ops, but A's own ops and its Forget's
    // past (A's enrolment and whatever it had seen) are all there.
    assert!(o.pending.is_empty(), "the handover carries each op's past: {o:?}");
    assert!(c.k.devices().iter().any(|d| d.device == aid && d.forgotten));
}
