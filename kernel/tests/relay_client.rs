//! What a relay client needs from the api (ADR 0011, "The relay client"), checked
//! against a stand-in relay that keeps one log per uploader, as the real one does
//! (docs/reference/relay-protocol.md). The relay itself is tested in `relay/`.
//!
//! - the plain enrolment and Forget fields the relay checks;
//! - the covers recorded at each own checkpoint;
//! - the outbox: every op this device holds that the relay is not known to hold,
//!   including ops learned over the LAN, so a relay-only device is not stuck;
//! - the Forget record handed out only once the Forget op is uploaded;
//! - all of it surviving a restart and a snapshot adoption.

mod common;
use common::{seed, APP, T0};

use hearth_sync_kernel::api::{
    ApiError, Collection, Field, FieldDef, Kernel, Merge, OpenArgs, Outcome, Record, RelayCursor, Schema, Step, Value,
    ValueType, Write,
};
use hearth_sync_kernel::keys::{enroll_auth_msg, forget_auth_msg, DeviceSigner, SoftSigner};
use hearth_sync_kernel::op;
use hearth_sync_kernel::persist::{Changeset, MemPersist, Persist};
use hearth_sync_kernel::seal::envelope_ref;
use hearth_sync_kernel::{DeviceId, Id};
use std::collections::BTreeMap;

const H: u64 = 20_000;

fn schema() -> Schema {
    Schema {
        collections: vec![Collection {
            name: "feeds".into(),
            merge: Merge::Lww {
                fields: vec![FieldDef { name: "ml".into(), ty: ValueType::Int, nullable: true }],
                container: None,
            },
        }],
        horizon_ms: H,
        keep_full_history: false,
    }
}

/// A stand-in relay: one numbered log per uploader, and the Forget records posted.
#[derive(Default)]
struct Relay {
    logs: BTreeMap<DeviceId, Vec<Vec<u8>>>,
    forgets: Vec<(Vec<u8>, u64)>,
}

struct Dev {
    k: Kernel,
    signer: SoftSigner,
    store: MemPersist,
}

fn records(p: &MemPersist) -> Vec<Record> {
    p.records().into_iter().map(|(key, v)| Record { key, value: Some(v) }).collect()
}

impl Dev {
    fn open(n: u8, records: Vec<Record>, now: u64) -> Kernel {
        Kernel::open(OpenArgs {
            app: APP.into(),
            seed: seed().to_vec(),
            device: SoftSigner::from_secret([n; 32]).device().to_vec(),
            schema: schema(),
            records,
            now,
        })
        .unwrap()
    }

    fn new(n: u8, now: u64) -> Dev {
        let mut d = Dev {
            k: Dev::open(n, vec![], now),
            signer: SoftSigner::from_secret([n; 32]),
            store: MemPersist::default(),
        };
        let o = d.k.flush();
        d.take(o);
        let s = d.k.enroll_self(format!("device {n}"), now);
        d.drive(s);
        d
    }

    fn id(&self) -> DeviceId {
        self.signer.device()
    }

    fn take(&mut self, o: Outcome) -> Outcome {
        self.store.apply(&Changeset { reset: o.reset_records, records: o.records.clone() });
        o
    }

    fn drive(&mut self, mut step: Result<Step, ApiError>) -> Outcome {
        loop {
            match step.unwrap() {
                Step::Sign { signable } => step = self.k.finish(self.signer.sign(&signable).to_vec()),
                Step::Done(o) => return self.take(o),
            }
        }
    }

    fn put(&mut self, row: &str, ml: i64, now: u64) {
        let s = self.k.write(
            Write::Put {
                table: "feeds".into(),
                row: row.into(),
                fields: vec![Field { name: "ml".into(), value: Value::Int(ml) }],
            },
            now,
        );
        self.drive(s);
    }

    /// Upload the whole outbox in one append, at the next seq, and acknowledge it.
    fn push(&mut self, relay: &mut Relay) -> Vec<Vec<u8>> {
        let out = self.k.relay_outbox();
        if out.is_empty() {
            return vec![];
        }
        let log = relay.logs.entry(self.id()).or_default();
        let first = self.k.relay_state().next_seq;
        assert_eq!(first, log.len() as u64 + 1, "the kernel tracks the own log's seq");
        log.extend(out.iter().map(|o| o.sealed.clone()));
        let o = self.k.relay_uploaded(out.iter().map(|o| o.id.clone()).collect(), first).unwrap();
        self.take(o);
        out.into_iter().map(|o| o.id).collect()
    }

    /// Pull every log from the cursors, ingest, and record the cursors.
    fn pull(&mut self, relay: &Relay, now: u64) -> Outcome {
        let cursors: BTreeMap<Vec<u8>, u64> =
            self.k.relay_state().cursors.into_iter().map(|c| (c.device, c.seq)).collect();
        let mut envs = Vec::new();
        let mut next = Vec::new();
        for (uploader, log) in &relay.logs {
            let from = cursors.get(uploader.as_slice()).copied().unwrap_or(0) as usize;
            envs.extend(log[from.min(log.len())..].iter().cloned());
            next.push(RelayCursor { device: uploader.to_vec(), seq: log.len() as u64 });
        }
        let o = self.k.ingest(envs, now).unwrap();
        let o = self.take(o);
        let p = self.k.relay_pulled(next).unwrap();
        self.take(p);
        o
    }

    /// A one-way LAN sync: this device takes what `peer` offers.
    fn lan_from(&mut self, peer: &Dev, now: u64) -> Outcome {
        let r = self.k.request(peer.k.hello(now)).unwrap();
        let s = self.k.accept(peer.k.offer(r.message).unwrap(), now);
        self.drive(s)
    }

    fn reopen(&mut self, n: u8, now: u64) {
        self.k = Dev::open(n, records(&self.store), now);
        let o = self.k.flush();
        self.take(o);
    }
}

fn ids_of(relay: &Relay, uploader: DeviceId) -> Vec<Id> {
    relay.logs.get(&uploader).into_iter().flatten().map(|e| envelope_ref(e).unwrap()).collect()
}

#[test]
fn the_enrolment_fields_verify_under_the_household() {
    let mut a = Dev::new(1, T0);
    let b = SoftSigner::from_secret([2; 32]).device();
    let s = a.k.enroll_device(b.to_vec(), "Kitchen tablet".into(), T0 + 1);
    a.drive(s);
    for (device, label) in [(a.id(), "device 1"), (b, "Kitchen tablet")] {
        let e = a.k.relay_enrollment(device.to_vec()).unwrap();
        assert_eq!(e.device, device.to_vec());
        assert_eq!(e.label, label);
        assert_eq!(e.household, common::root().enroll_public(APP).to_vec());
        let household: [u8; 32] = e.household.as_slice().try_into().unwrap();
        let auth: [u8; 64] = e.auth.as_slice().try_into().unwrap();
        assert!(op::verify_strict(&household, &enroll_auth_msg(APP, &device, label), &auth));
    }
    assert!(matches!(a.k.relay_enrollment(vec![9; 32]), Err(ApiError::BadArgument(_))));
}

#[test]
fn an_op_learned_over_the_lan_reaches_a_relay_only_device() {
    let mut relay = Relay::default();
    let mut a = Dev::new(1, T0);
    let mut b = Dev::new(2, T0);
    let mut c = Dev::new(3, T0);
    a.push(&mut relay);
    // B never talks to the relay: A learns B's enrolment and edit over the LAN only.
    b.put("lan", 5, T0 + 1);
    a.lan_from(&b, T0 + 2);
    let out: Vec<Vec<u8>> = a.k.relay_outbox().into_iter().map(|o| o.id).collect();
    assert_eq!(out.len(), 2, "B's enrolment and edit, learned over the LAN, are A's to forward");
    a.push(&mut relay);
    assert!(a.k.relay_outbox().is_empty(), "acknowledged uploads leave the outbox");
    c.push(&mut relay);
    c.pull(&relay, T0 + 3);
    a.pull(&relay, T0 + 3);
    assert!(c.k.view_all().rows.iter().any(|r| r.row == "lan"), "C, relay-only, has B's edit");
    assert_eq!(c.k.status().pending, 0);
    // What a pull delivers is on the relay already: never queued for upload.
    assert!(c.k.relay_outbox().is_empty());
    // An op the relay turns out to hold (pulled from someone else's log) leaves the
    // outbox without being uploaded.
    c.put("mine", 1, T0 + 4);
    a.lan_from(&c, T0 + 5);
    assert_eq!(a.k.relay_outbox().len(), 1);
    c.push(&mut relay);
    a.pull(&relay, T0 + 6);
    assert!(a.k.relay_outbox().is_empty(), "pulled from C's log, so not A's to upload");
}

#[test]
fn a_checkpoint_records_what_its_snapshot_covers_when_nothing_is_waiting() {
    let mut relay = Relay::default();
    let mut a = Dev::new(1, T0);
    let mut b = Dev::new(2, T0);
    a.push(&mut relay);
    b.put("b1", 1, T0 + 1);
    b.push(&mut relay);
    a.pull(&relay, T0 + 2);
    a.put("a1", 1, T0 + 3);
    a.push(&mut relay);
    let s = a.k.checkpoint(T0 + 4);
    a.drive(s);
    a.push(&mut relay);
    let o = a.k.compact(T0 + 4 + H + 1).unwrap();
    a.take(o);
    let snap = a.k.relay_snapshot().expect("compacted behind its own checkpoint");
    assert_eq!(Some(envelope_ref(&snap.sealed).unwrap()), a.k.snapshot().as_deref().and_then(envelope_ref));
    let covers: BTreeMap<Vec<u8>, u64> = snap.covers.into_iter().map(|c| (c.device, c.seq)).collect();
    // B's log up to what A pulled, and A's own log up to its last upload before the
    // checkpoint (the checkpoint op itself went up after).
    assert_eq!(covers.get(b.id().as_slice()), Some(&2));
    assert_eq!(covers.get(a.id().as_slice()), Some(&2));
    assert_eq!(covers.len(), 2);

    // With an op still waiting for its parent, the checkpoint records no covers: the
    // relay must not prune an entry the snapshot does not hold.
    let mut relay = Relay::default();
    let mut a = Dev::new(1, T0);
    let mut b = Dev::new(2, T0);
    b.put("b1", 1, T0 + 1);
    b.put("b1", 2, T0 + 2);
    b.push(&mut relay);
    relay.logs.get_mut(&b.id()).unwrap().remove(1); // A misses B's first edit
    a.pull(&relay, T0 + 3);
    assert_eq!(a.k.status().pending, 1);
    let s = a.k.checkpoint(T0 + 4);
    a.drive(s);
    let o = a.k.compact(T0 + 4 + H + 1).unwrap();
    a.take(o);
    let snap = a.k.relay_snapshot().expect("a base");
    assert!(snap.covers.is_empty());
}

#[test]
fn a_forget_record_is_handed_out_only_after_the_forget_op_is_uploaded() {
    let mut relay = Relay::default();
    let mut a = Dev::new(1, T0);
    let mut b = Dev::new(2, T0);
    a.push(&mut relay);
    b.put("b1", 1, T0 + 1);
    b.push(&mut relay);
    a.pull(&relay, T0 + 2);
    let cursor_b = 2;
    b.put("b2", 2, T0 + 3);
    b.push(&mut relay); // on the relay, but A has not pulled it
    let s = a.k.forget_device(b.id().to_vec(), T0 + 4);
    a.drive(s);
    assert!(a.k.relay_forgets().is_empty(), "the Forget op is not on the relay yet");
    let uploaded = a.push(&mut relay);
    let recs = a.k.relay_forgets();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    assert_eq!(uploaded.last(), Some(&r.forget), "the record names the uploaded Forget op");
    assert_eq!(r.target, b.id().to_vec());
    assert_eq!(r.cut_seq, cursor_b, "B's log as far as A had read it when it forgot B");
    let household = common::root().enroll_public(APP);
    let cut: Vec<Id> = r.cut.iter().map(|c| c.as_slice().try_into().unwrap()).collect();
    let auth: [u8; 64] = r.auth.as_slice().try_into().unwrap();
    assert!(op::verify_strict(&household, &forget_auth_msg(APP, &b.id(), &cut), &auth));
    relay.forgets.push((r.target.clone(), r.cut_seq));
    let o = a.k.relay_forget_posted(r.forget.clone()).unwrap();
    a.take(o);
    assert!(a.k.relay_forgets().is_empty());
    // It stays posted across a restart.
    a.reopen(1, T0 + 5);
    assert!(a.k.relay_forgets().is_empty());
}

#[test]
fn a_device_that_forgets_itself_can_still_upload_and_then_post_its_record() {
    let mut relay = Relay::default();
    let mut c = Dev::new(3, T0);
    c.push(&mut relay);
    c.put("offline", 5, T0 + 1);
    let s = c.k.forget_self(T0 + 2);
    let o = c.drive(s);
    assert!(o.wiped);
    assert!(c.k.relay_forgets().is_empty());
    let out = c.k.relay_outbox();
    assert_eq!(out.len(), 2, "the offline edit and the Forget, wiped or not");
    c.push(&mut relay);
    let recs = c.k.relay_forgets();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].target, c.id().to_vec());
    assert_eq!(recs[0].cut_seq, 3, "the seq the Forget op itself landed at");
    assert_eq!(ids_of(&relay, c.id())[2].to_vec(), recs[0].forget);
}

#[test]
fn the_relay_state_survives_a_restart_and_a_snapshot_adoption() {
    let mut relay = Relay::default();
    let mut a = Dev::new(1, T0);
    let mut b = Dev::new(2, T0);
    a.push(&mut relay);
    b.push(&mut relay);
    a.pull(&relay, T0 + 1);
    a.put("a1", 1, T0 + 2);
    a.push(&mut relay);
    let s = a.k.checkpoint(T0 + 3);
    a.drive(s);
    a.put("a2", 2, T0 + 4); // queued, not uploaded
    let s = a.k.forget_device(b.id().to_vec(), T0 + 5);
    a.drive(s);
    let o = a.k.compact(T0 + 3 + H + 1).unwrap();
    a.take(o);
    let before = (a.k.relay_state(), a.k.relay_outbox(), a.k.relay_snapshot());
    assert!(!before.1.is_empty() && before.2.as_ref().is_some_and(|s| !s.covers.is_empty()));
    a.reopen(1, T0 + 3 + H + 2);
    assert_eq!((a.k.relay_state(), a.k.relay_outbox(), a.k.relay_snapshot()), before);

    // D adopts A's snapshot over the LAN (a reset of every record): D's own relay
    // state, and the ops D holds that the relay lacks, come through the reset.
    let mut d = Dev::new(4, T0);
    d.push(&mut relay);
    d.pull(&relay, T0 + 1);
    d.put("d1", 1, T0 + 6); // queued
    let now = T0 + 3 + H + 3;
    let r = d.k.request(a.k.hello(now)).unwrap();
    assert!(r.needs_snapshot);
    let s = d.k.accept(a.k.offer(r.message).unwrap(), now);
    let o = d.drive(s);
    assert!(o.reset_records);
    let state = d.k.relay_state();
    assert_eq!(state.next_seq, 2);
    let out: Vec<Vec<u8>> = d.k.relay_outbox().into_iter().map(|o| o.id).collect();
    assert!(!out.is_empty(), "D's re-issued edit and what the offer brought above the snapshot");
    d.reopen(4, now + 1);
    assert_eq!(d.k.relay_state(), state);
    assert_eq!(d.k.relay_outbox().into_iter().map(|o| o.id).collect::<Vec<_>>(), out);
}

#[test]
fn a_new_relay_generation_resets_the_positions_and_queues_everything_held() {
    let mut relay = Relay::default();
    let mut a = Dev::new(1, T0);
    let mut b = Dev::new(2, T0);
    assert_eq!(a.k.relay_state().generation, 0, "not known before the relay says");
    let o = a.k.relay_generation(1).unwrap();
    a.take(o);
    a.push(&mut relay);
    b.put("b1", 1, T0 + 1);
    b.push(&mut relay);
    a.pull(&relay, T0 + 2);
    a.put("a1", 1, T0 + 3);
    a.push(&mut relay);
    let s = a.k.checkpoint(T0 + 4);
    a.drive(s);
    a.push(&mut relay);
    let o = a.k.compact(T0 + 4 + H + 1).unwrap();
    a.take(o);
    // The same generation again changes nothing.
    let before = (a.k.relay_state(), a.k.relay_outbox(), a.k.relay_snapshot());
    let o = a.k.relay_generation(1).unwrap();
    a.take(o);
    assert_eq!((a.k.relay_state(), a.k.relay_outbox(), a.k.relay_snapshot()), before);
    assert!(before.1.is_empty() && before.2.as_ref().is_some_and(|s| !s.covers.is_empty()));

    // The channel expired and was made again: every position is from the old one.
    let o = a.k.relay_generation(2).unwrap();
    a.take(o);
    let state = a.k.relay_state();
    assert_eq!((state.generation, state.next_seq, state.cursors.len()), (2, 1, 0));
    assert!(a.k.relay_snapshot().is_some_and(|s| s.covers.is_empty()), "old covers name old logs");
    let held = a.k.relay_outbox();
    assert!(held.len() >= 3, "everything A still holds a body for goes up again");
    // It survives a restart.
    a.reopen(1, T0 + 4 + H + 2);
    assert_eq!(a.k.relay_state(), state);
    assert_eq!(a.k.relay_outbox(), held);

    // Through the new, empty relay, C (relay only) converges from A's snapshot (A's
    // ops behind its base have no bodies left) and the ops A re-uploaded above it.
    let mut relay = Relay::default();
    a.push(&mut relay);
    let now = T0 + 4 + H + 3;
    let mut c = Dev::new(3, now);
    let above: Vec<Vec<u8>> = relay.logs.values().flatten().cloned().collect();
    let s = c.k.adopt_snapshot(a.k.relay_snapshot().unwrap().sealed, above, now);
    c.drive(s);
    c.push(&mut relay);
    c.pull(&relay, now);
    assert_eq!(c.k.view_all(), a.k.view_all());
    let rows = |d: &Dev| d.k.view_all().rows.iter().map(|r| r.row.clone()).collect::<Vec<_>>();
    assert!(rows(&c).contains(&"a1".to_string()) && rows(&c).contains(&"b1".to_string()));
}

#[test]
fn a_forget_record_not_yet_posted_takes_its_cut_seq_from_the_new_generation() {
    let mut relay = Relay::default();
    let mut a = Dev::new(1, T0);
    let mut b = Dev::new(2, T0);
    let o = a.k.relay_generation(1).unwrap();
    a.take(o);
    a.push(&mut relay);
    b.put("b1", 1, T0 + 1);
    b.push(&mut relay);
    a.pull(&relay, T0 + 2);
    let s = a.k.forget_device(b.id().to_vec(), T0 + 3);
    a.drive(s);
    a.push(&mut relay);
    assert_eq!(a.k.relay_forgets()[0].cut_seq, 2, "B's old log as far as A had read it");
    // The record never reached the relay, and the channel expired: B's old log is gone,
    // so seq 2 names nothing any more. A has read nothing of B's new log.
    let o = a.k.relay_generation(2).unwrap();
    a.take(o);
    assert!(a.k.relay_forgets().is_empty(), "the Forget op must go up again first");
    let mut relay = Relay::default();
    a.push(&mut relay);
    let recs = a.k.relay_forgets();
    assert_eq!(recs.len(), 1);
    assert_eq!(recs[0].cut_seq, 0, "A's cursor on B's log in the new generation");
    a.reopen(1, T0 + 4);
    assert_eq!(a.k.relay_forgets(), recs, "and it survives a restart");

    // A self-Forget's cut ends where its Forget op lands in the new log.
    let mut relay = Relay::default();
    let mut c = Dev::new(3, T0);
    let o = c.k.relay_generation(1).unwrap();
    c.take(o);
    c.push(&mut relay);
    c.put("c1", 1, T0 + 1);
    c.push(&mut relay);
    c.put("offline", 2, T0 + 2);
    let s = c.k.forget_self(T0 + 3);
    assert!(c.drive(s).wiped);
    c.push(&mut relay);
    assert_eq!(c.k.relay_forgets()[0].cut_seq, 4);
    let o = c.k.relay_generation(2).unwrap();
    c.take(o);
    assert!(c.k.relay_forgets().is_empty(), "the Forget op must go up again first");
    let mut relay = Relay::default();
    c.push(&mut relay);
    let recs = c.k.relay_forgets();
    let at = ids_of(&relay, c.id()).iter().position(|id| id.to_vec() == recs[0].forget).unwrap();
    assert_eq!(recs[0].cut_seq, at as u64 + 1, "the seq the Forget op landed at in the new log");
}

#[test]
fn an_append_whose_answer_was_lost_is_learned_from_the_own_log_on_the_next_pull() {
    let mut relay = Relay::default();
    let mut a = Dev::new(1, T0);
    a.push(&mut relay);
    a.put("a1", 1, T0 + 1);
    // The relay stored the batch, but its answer never arrived: no relay_uploaded.
    let out = a.k.relay_outbox();
    relay.logs.entry(a.id()).or_default().extend(out.iter().map(|o| o.sealed.clone()));
    // The next round pulls first, and finds its own entries in its own log.
    a.pull(&relay, T0 + 2);
    assert_eq!(a.k.relay_state().next_seq, 3, "the own log's cursor moves the own seq on");
    a.put("a2", 2, T0 + 3);
    a.push(&mut relay); // asserts the upload continues the log
    a.reopen(1, T0 + 4);
    assert_eq!(a.k.relay_state().next_seq, 4);
}
