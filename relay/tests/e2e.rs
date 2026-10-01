//! End to end: kernel replicas, driven only through the kernel's `api` (as the Dart
//! side will drive them), that sync only through a relay instance in this process.
//! Each device does what a client does, with what the api's relay client hands it
//! (ADR 0011, "The relay client"): enrol with `relay_enrollment`, upload
//! `relay_outbox` from `relay_state().next_seq` and acknowledge it with
//! `relay_uploaded`, post each `relay_forgets` record, pull from the kept cursors and
//! record them with `relay_pulled`, and upload `relay_snapshot` with its covers.
//! Reads carry the relay's epoch, learned from its `epoch` answer; the channel's
//! generation, from `enroll` and `pull` answers, goes to `relay_generation`, and a
//! device the relay no longer knows (its channel expired) enrols again.
//!
//! Every scenario runs against the Rust relay in this process and, when
//! `HEARTH_GO_DRIVER` names a built `go-relay/difftest/godriver`, against the Go
//! relay behind it too (go-relay/README.md).

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use hearth_sync_kernel::api::{
    ApiError, Collection, Field, FieldDef, Kernel, Merge, OpenArgs, Outcome, RelayCursor, Schema, Step, Value,
    ValueType, Write,
};
use hearth_sync_kernel::keys::{DeviceSigner, HouseholdRoot, SoftSigner};
use hearth_sync_kernel::persist::{Changeset, MemPersist, Persist};
use hearth_sync_kernel::seal::{envelope_ref, SealKeys};
use hearth_sync_kernel::{DeviceId, Id};
use hearth_sync_relay::wire::{channel_id, client, Channel};
use hearth_sync_relay::{Config, Relay, Response};
use serde_json::{json, Value as Json};

const APP: &str = "lullaby";
const T0: u64 = 1_727_000_000_000;
/// The app's horizon in these tests (the kernel's default is 90 days).
const H: u64 = 20_000;

fn root() -> HouseholdRoot {
    HouseholdRoot::from_seed(core::array::from_fn(|i| i as u8))
}

fn schema() -> Schema {
    Schema {
        collections: vec![
            Collection {
                name: "feeds".into(),
                merge: Merge::Lww {
                    fields: vec![FieldDef { name: "ml".into(), ty: ValueType::Int, nullable: true }],
                    container: None,
                },
            },
            Collection { name: "log".into(), merge: Merge::AppendOnly { record: ValueType::Any } },
        ],
        horizon_ms: H,
        keep_full_history: false,
    }
}

fn channel() -> Channel {
    channel_id(APP, &root().enroll_public(APP))
}

fn code(r: &Response) -> String {
    if r.status == 200 {
        "ok".into()
    } else {
        client::error_code(&r.body).unwrap_or_default()
    }
}

fn b32(v: &[u8]) -> [u8; 32] {
    v.try_into().unwrap()
}

/// A relay under test, over a store on disk (so it can restart).
trait Side {
    fn handle(&mut self, path: &str, body: &[u8], now: u64) -> Response;
    fn sweep(&mut self, now: u64);
    /// Stop and start again over the same store: memory goes, the epoch moves on.
    fn restart(&mut self);
}

struct RustSide {
    relay: Option<Relay>,
    dir: PathBuf,
    cfg: Config,
}

impl Side for RustSide {
    fn handle(&mut self, path: &str, body: &[u8], now: u64) -> Response {
        self.relay.as_mut().unwrap().handle("POST", path, body, now)
    }
    fn sweep(&mut self, now: u64) {
        self.relay.as_mut().unwrap().sweep(now).unwrap();
    }
    fn restart(&mut self) {
        drop(self.relay.take());
        self.relay = Some(Relay::open_unsynced(&self.dir, self.cfg.clone()).unwrap());
    }
}

/// The Go relay behind godriver's line protocol.
struct GoSide {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl GoSide {
    fn call(&mut self, cmd: Json) -> Json {
        writeln!(self.stdin, "{cmd}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("godriver died on {cmd}"))
    }
}

impl Side for GoSide {
    fn handle(&mut self, path: &str, body: &[u8], now: u64) -> Response {
        let a = self.call(json!({"op": "req", "method": "POST", "path": path, "body": hex(body), "now": now}));
        let body = a["body"].as_str().unwrap();
        let body = (0..body.len()).step_by(2).map(|i| u8::from_str_radix(&body[i..i + 2], 16).unwrap()).collect();
        Response { status: a["status"].as_u64().unwrap() as u16, body, content_type: "application/cbor", verb: "-" }
    }
    fn sweep(&mut self, now: u64) {
        self.call(json!({"op": "sweep", "now": now}));
    }
    fn restart(&mut self) {
        self.call(json!({"op": "restart"}));
    }
}

impl Drop for GoSide {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The relays to run a scenario against: the Rust one, and the Go one when built.
/// `overrides` are protocol config keys applied to both.
fn sides(name: &str, overrides: &[(&str, u64)]) -> Vec<(&'static str, Box<dyn Side>)> {
    let mut cfg = Config { retain_ms: H + 5_000, ..Config::default() };
    for (k, v) in overrides {
        assert!(cfg.set(k, *v));
    }
    let dir = |side: &str| {
        let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("relay-e2e").join(side).join(name);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    };
    let d = dir("rust");
    let rust = RustSide { relay: Some(Relay::open_unsynced(&d, cfg.clone()).unwrap()), dir: d, cfg };
    let mut out: Vec<(&'static str, Box<dyn Side>)> = vec![("rust", Box::new(rust))];
    match std::env::var("HEARTH_GO_DRIVER") {
        Ok(driver) => {
            let mut child =
                Command::new(driver).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("run godriver");
            let (stdin, stdout) = (child.stdin.take().unwrap(), BufReader::new(child.stdout.take().unwrap()));
            let mut go = GoSide { child, stdin, stdout };
            let mut config = serde_json::Map::new();
            config.insert("retain_ms".into(), json!(H + 5_000));
            for (k, v) in overrides {
                config.insert((*k).into(), json!(v));
            }
            go.call(json!({"op": "reset", "config": config, "dir": dir("go").to_str().unwrap()}));
            out.push(("go", Box::new(go)));
        }
        Err(_) => eprintln!("{name}: HEARTH_GO_DRIVER not set, the Go relay is skipped"),
    }
    out
}

fn post(relay: &mut dyn Side, verb: &str, body: &[u8], now: u64) -> Response {
    relay.handle(&format!("/v1/{}/{verb}", hex(&channel())), body, now)
}

/// One device: its kernel, its platform key and the app's record store. Everything
/// the relay client keeps (seqs, cursors, outbox, Forget records, covers) is the
/// kernel's; the device itself remembers only the relay's epoch and a nonce counter.
struct Dev {
    k: Kernel,
    signer: SoftSigner,
    store: MemPersist,
    epoch: u64,
    nonce: u64,
}

impl Dev {
    fn id(&self) -> DeviceId {
        self.signer.device()
    }

    /// Open, enrol in the kernel and at the relay, and upload the enrolment.
    fn join(n: u8, relay: &mut dyn Side, now: u64) -> Dev {
        let signer = SoftSigner::from_secret([n; 32]);
        let k = Kernel::open(OpenArgs {
            app: APP.into(),
            seed: core::array::from_fn::<u8, 64, _>(|i| i as u8).to_vec(),
            device: signer.device().to_vec(),
            schema: schema(),
            records: vec![],
            now,
        })
        .unwrap();
        let mut d = Dev { k, signer, store: MemPersist::default(), epoch: 0, nonce: 0 };
        let o = d.k.flush();
        d.take(o);
        let s = d.k.enroll_self(format!("device {n}"), now);
        d.drive(s);
        assert_eq!(d.enrol(relay, now), "ok");
        assert_eq!(d.push(relay, now), "ok");
        d
    }

    /// Enrol this device at the relay, and hand the kernel the channel's generation.
    fn enrol(&mut self, relay: &mut dyn Side, now: u64) -> String {
        let e = self.k.relay_enrollment(self.id().to_vec()).unwrap();
        let body = client::enroll(APP, b32(&e.household), b32(&e.device), &e.label, e.auth.try_into().unwrap());
        let r = post(relay, "enroll", &body, now);
        if let Some(g) = client::parse_enroll(&r.body) {
            let o = self.k.relay_generation(g).unwrap();
            self.take(o);
        }
        code(&r)
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

    /// Upload the whole outbox in one append at the kernel's next seq, acknowledge it,
    /// then post every Forget record the kernel now hands out (only those whose Forget
    /// op is on the relay). Returns the append's answer.
    fn push(&mut self, relay: &mut dyn Side, now: u64) -> String {
        let out = self.k.relay_outbox(u64::MAX);
        let mut answer = "ok".to_string();
        if !out.is_empty() {
            let first = self.k.relay_state().next_seq;
            let body =
                client::append(&channel(), &self.signer, first, out.iter().map(|o| o.sealed.clone()).collect(), now);
            answer = code(&post(relay, "append", &body, now));
            if answer == "ok" {
                let o = self.k.relay_uploaded(out.into_iter().map(|o| o.id).collect(), first).unwrap();
                self.take(o);
            }
        }
        for f in self.k.relay_forgets() {
            let cut: Vec<Id> = f.cut.iter().map(|c| b32(c)).collect();
            let body = client::forget(
                &channel(),
                &self.signer,
                b32(&f.target),
                &cut,
                f.auth.try_into().unwrap(),
                f.cut_seq,
                now,
            );
            if code(&post(relay, "forget", &body, now)) == "ok" {
                let o = self.k.relay_forget_posted(f.forget).unwrap();
                self.take(o);
            }
        }
        answer
    }

    fn next_nonce(&mut self) -> [u8; 16] {
        self.nonce += 1;
        let mut n = [self.signer.device()[0]; 16];
        n[8..].copy_from_slice(&self.nonce.to_be_bytes());
        n
    }

    /// A read, signed under the epoch this device knows; on an `epoch` answer it learns
    /// the relay's and signs again (once).
    fn read(
        &mut self,
        relay: &mut dyn Side,
        verb: &str,
        now: u64,
        body: impl Fn(&SoftSigner, u64, [u8; 16]) -> Vec<u8>,
    ) -> Response {
        let mut enrolled = false;
        for _ in 0..3 {
            let nonce = self.next_nonce();
            let r = post(relay, verb, &body(&self.signer, self.epoch, nonce), now);
            match client::error_epoch(&r.body) {
                Some(e) if r.status == 409 => self.epoch = e,
                // The relay no longer knows this device: its channel expired. Enrol
                // again (once), which also names the channel's new generation.
                _ if code(&r) == "not_enrolled" && !enrolled && !self.k.status().wiped => {
                    enrolled = true;
                    assert_eq!(self.enrol(relay, now), "ok");
                }
                _ => return r,
            }
        }
        panic!("a read still refused after learning the epoch and enrolling");
    }

    /// Pull every page from the kernel's cursors. Returns the envelopes, whether some
    /// log was pruned past a cursor (the device then needs a snapshot), the last answer,
    /// and the cursors to record once the envelopes are ingested.
    fn pull_raw(
        &mut self,
        relay: &mut dyn Side,
        now: u64,
    ) -> (Vec<Vec<u8>>, bool, client::PullAnswer, Vec<RelayCursor>) {
        let mut cursors: BTreeMap<DeviceId, u64> =
            self.k.relay_state().cursors.into_iter().map(|c| (b32(&c.device), c.seq)).collect();
        let mut generation = self.k.relay_state().generation;
        let mut envs = Vec::new();
        let mut gap = false;
        loop {
            let at: Vec<(DeviceId, u64)> = cursors.iter().map(|(d, s)| (*d, *s)).collect();
            let r = self
                .read(relay, "pull", now, |s, epoch, nonce| client::pull(&channel(), s, epoch, nonce, at.clone(), now));
            assert_eq!(code(&r), "ok");
            let a = client::parse_pull(&r.body).unwrap();
            if self.k.relay_state().generation != generation {
                // The read enrolled again and learned a new generation: the cursors
                // it was signed with name logs that are gone. Pull again from the
                // kernel's reset positions (its answer names the new generation, so
                // relay_pulled would take the old cursors as seen in the new logs).
                generation = self.k.relay_state().generation;
                cursors.clear();
                envs.clear();
                gap = false;
                continue;
            }
            if a.generation != self.k.relay_state().generation {
                // A new channel: every position was in logs that are gone. Start over
                // from the kernel's reset positions and pull from the beginning.
                let o = self.k.relay_generation(a.generation).unwrap();
                self.take(o);
                generation = a.generation;
                cursors.clear();
                envs.clear();
                gap = false;
                continue;
            }
            for page in &a.logs {
                let cur = cursors.entry(page.uploader).or_insert(0);
                if page.first > *cur + 1 {
                    // Pruned past the cursor: the caller takes a snapshot for what is
                    // gone, so the cursor moves to what the relay still holds.
                    gap = true;
                    *cur = page.first - 1;
                }
                for (seq, env) in &page.entries {
                    *cur = *seq;
                    envs.push(env.clone());
                }
            }
            if !a.more {
                let cursors = cursors.into_iter().map(|(d, seq)| RelayCursor { device: d.to_vec(), seq }).collect();
                return (envs, gap, a, cursors);
            }
        }
    }

    fn pulled(&mut self, generation: u64, cursors: Vec<RelayCursor>) {
        let o = self.k.relay_pulled(generation, cursors).unwrap();
        self.take(o);
    }

    /// Pull, ingest, and record the cursors.
    fn pull(&mut self, relay: &mut dyn Side, now: u64) -> Outcome {
        let (envs, gap, answer, cursors) = self.pull_raw(relay, now);
        assert!(!gap, "unexpected gap");
        let o = self.k.ingest(envs, now).unwrap();
        let o = self.take(o);
        self.pulled(answer.generation, cursors);
        o
    }

    /// Checkpoint right after a clean pull and an upload, so the kernel records what
    /// the checkpoint's snapshot will cover; then upload the checkpoint.
    fn checkpoint(&mut self, relay: &mut dyn Side, now: u64) {
        let o = self.pull(relay, now);
        assert!(o.pending.is_empty() && o.quarantined.is_empty() && o.held.is_empty());
        assert_eq!(self.push(relay, now), "ok");
        let s = self.k.checkpoint(now);
        self.drive(s);
        assert_eq!(self.push(relay, now), "ok");
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn two_devices_converge_through_the_relay_alone() {
    for (_side, mut relay) in sides("two_devices_converge_through_the_relay_a", &[]) {
        let relay = relay.as_mut();
        let mut a = Dev::join(1, relay, T0);
        let mut b = Dev::join(2, relay, T0 + 1);
        a.put("r1", 120, T0 + 2);
        b.put("r2", 90, T0 + 3);
        assert_eq!(a.push(relay, T0 + 4), "ok");
        assert_eq!(b.push(relay, T0 + 4), "ok");
        b.pull(relay, T0 + 5);
        a.pull(relay, T0 + 5);
        // Concurrent edits to one field resolve the same way on both.
        a.put("r1", 130, T0 + 6);
        b.put("r1", 140, T0 + 6);
        a.push(relay, T0 + 7);
        b.push(relay, T0 + 7);
        a.pull(relay, T0 + 8);
        b.pull(relay, T0 + 8);
        assert_eq!(a.k.view_all(), b.k.view_all());
        assert_eq!(a.k.heads(), b.k.heads());
        assert_eq!(a.k.view_all().rows.len(), 2);
        assert!(
            a.k.relay_outbox(u64::MAX).is_empty() && b.k.relay_outbox(u64::MAX).is_empty(),
            "everything is on the relay"
        );
        assert_eq!(a.epoch, 1, "the first read learned the epoch");
        // An empty batch is malformed; a stranger cannot read.
        let body = client::append(&channel(), &a.signer, 1, vec![], T0 + 9);
        assert_eq!(code(&post(relay, "append", &body, T0 + 9)), "bad_request");
        let c = SoftSigner::from_secret([9; 32]);
        let body = client::pull(&channel(), &c, 1, [9; 16], vec![], T0 + 9);
        assert_eq!(code(&post(relay, "pull", &body, T0 + 9)), "not_enrolled");
    }
}

#[test]
fn a_relay_restart_moves_the_epoch_so_reads_retry_and_captured_reads_are_refused() {
    for (side, mut relay) in sides("restart", &[]) {
        let relay = relay.as_mut();
        let mut a = Dev::join(1, relay, T0);
        let mut b = Dev::join(2, relay, T0 + 1);
        a.put("r1", 1, T0 + 2);
        a.push(relay, T0 + 2);
        // A pull someone captured on the way.
        let captured = client::pull(&channel(), &b.signer, 1, [0xEE; 16], vec![], T0 + 3);
        assert_eq!(code(&post(relay, "pull", &captured, T0 + 3)), "ok");
        relay.restart();
        let r = post(relay, "pull", &captured, T0 + 4);
        assert_eq!((code(&r), client::error_epoch(&r.body)), ("epoch".into(), Some(2)), "{side}: no replay");
        // B still holds the old epoch: its read is refused once, then goes through.
        b.pull(relay, T0 + 5);
        assert_eq!(b.epoch, 2);
        assert_eq!(a.k.view_all(), b.k.view_all(), "{side}: the store survived the restart");
    }
}

#[test]
fn a_device_forgotten_through_the_relay_wipes_on_its_next_pull_and_its_writes_are_refused() {
    for (_side, mut relay) in sides("a_device_forgotten_through_the_relay_wip", &[]) {
        let relay = relay.as_mut();
        let mut a = Dev::join(1, relay, T0);
        let mut b = Dev::join(2, relay, T0 + 1);
        b.put("r1", 1, T0 + 2);
        b.push(relay, T0 + 2);
        a.pull(relay, T0 + 3);
        // A forgets B. The kernel hands out the record only once the Forget op is up, so
        // one push uploads the op and then posts the record, cut at what A had read of B.
        let s = a.k.forget_device(b.id().to_vec(), T0 + 4);
        a.drive(s);
        assert!(a.k.relay_forgets().is_empty(), "no record before the Forget op is uploaded");
        let forget_env = a.k.relay_outbox(u64::MAX).last().unwrap().sealed.clone();
        assert_eq!(a.push(relay, T0 + 4), "ok");
        assert!(a.k.relay_forgets().is_empty(), "the record was posted");
        // A writes after the Forget; B, frozen at the Forget, never sees it.
        a.put("secret", 7, T0 + 6);
        let secret = envelope_ref(&a.k.relay_outbox(u64::MAX).last().unwrap().sealed).unwrap();
        a.push(relay, T0 + 6);
        let (envs, _, _, _) = b.pull_raw(relay, T0 + 7);
        let o = b.k.ingest(envs.clone(), T0 + 7).unwrap();
        let o = b.take(o);
        assert!(o.wiped, "B wipes on the Forget it pulled");
        assert!(b.k.status().wiped);
        assert!(a.k.view_all().rows.iter().any(|r| r.row == "secret"));
        let keys = SealKeys::derive(&root(), APP);
        let refs: Vec<Id> = envs.iter().map(|e| keys.open_op(e).unwrap().0).collect();
        assert!(!refs.contains(&secret), "the frozen view holds nothing after the Forget");
        // A stolen, forgotten B still holds its device key: its writes are refused.
        let next = b.k.relay_state().next_seq;
        let body = client::append(&channel(), &b.signer, next, vec![forget_env.clone()], T0 + 8);
        assert_eq!(code(&post(relay, "append", &body, T0 + 8)), "forgotten");
        let snap = client::snapshot(&channel(), &b.signer, forget_env, vec![], T0 + 8);
        assert_eq!(code(&post(relay, "snapshot", &snap, T0 + 8)), "forgotten");
    }
}

#[test]
fn a_device_that_forgets_itself_hands_over_its_last_ops_then_posts_its_record() {
    for (_side, mut relay) in sides("a_device_that_forgets_itself_hands_over_", &[]) {
        let relay = relay.as_mut();
        let mut a = Dev::join(1, relay, T0);
        let mut c = Dev::join(3, relay, T0 + 1);
        c.pull(relay, T0 + 2);
        // Offline, C writes, then forgets itself: it wipes at once.
        c.put("offline", 5, T0 + 3);
        let s = c.k.forget_self(T0 + 4);
        let o = c.drive(s);
        assert!(o.wiped);
        assert!(c.k.relay_forgets().is_empty());
        let last = c.k.relay_outbox(u64::MAX).last().unwrap().sealed.clone();
        // Back online: the handover goes up, then the record, whose cut ends at the Forget
        // op's own seq (the record-first order is not reachable through the api).
        assert_eq!(c.push(relay, T0 + 5), "ok");
        assert!(c.k.relay_forgets().is_empty(), "the record was posted");
        a.pull(relay, T0 + 7);
        let devices = a.k.devices();
        assert!(devices.iter().any(|d| d.device == c.id().to_vec() && d.forgotten), "A learns C forgot itself");
        assert!(a.k.view_all().rows.iter().any(|r| r.row == "offline"), "C's offline edit arrives");
        // Nothing past the cut.
        let next = c.k.relay_state().next_seq;
        let body = client::append(&channel(), &c.signer, next, vec![last], T0 + 8);
        assert_eq!(code(&post(relay, "append", &body, T0 + 8)), "forgotten");
    }
}

#[test]
fn a_device_back_after_the_horizon_converges_from_a_snapshot_and_the_ops_above_it() {
    for (_side, mut relay) in sides("a_device_back_after_the_horizon_converge", &[]) {
        let relay = relay.as_mut();
        let mut a = Dev::join(1, relay, T0);
        let mut b = Dev::join(2, relay, T0 + 1);
        let mut d = Dev::join(4, relay, T0 + 2);
        a.pull(relay, T0 + 3);
        b.pull(relay, T0 + 3);
        d.pull(relay, T0 + 3);
        // D goes away. A and B keep writing and checkpointing.
        for i in 0..5 {
            a.put(&format!("a{i}"), i, T0 + 10 + i as u64);
            b.put(&format!("b{i}"), i, T0 + 10 + i as u64);
        }
        a.push(relay, T0 + 20);
        b.push(relay, T0 + 20);
        b.checkpoint(relay, T0 + 30);
        a.checkpoint(relay, T0 + 31);
        b.pull(relay, T0 + 32);
        a.pull(relay, T0 + 32);
        // Past the horizon: both compact; each uploads its base as a snapshot, with the
        // covers the kernel recorded if the base is its own checkpoint.
        let later = T0 + 32 + H + 1;
        let mut covering = 0;
        for dev in [&mut a, &mut b] {
            let o = dev.k.compact(later).unwrap();
            dev.take(o);
            let Some(snap) = dev.k.relay_snapshot() else { continue };
            covering += usize::from(!snap.covers.is_empty());
            let covers = snap.covers.iter().map(|c| (b32(&c.device), c.seq)).collect();
            let body = client::snapshot(&channel(), &dev.signer, snap.sealed, covers, later);
            assert_eq!(code(&post(relay, "snapshot", &body, later)), "ok");
        }
        assert!(covering > 0, "some device compacted behind its own checkpoint and covers entries");
        // Past the relay's retention, the next write prunes what the snapshots cover.
        let prune_at = T0 + 20 + H + 5_000 + 100;
        a.put("after", 1, prune_at);
        assert_eq!(a.push(relay, prune_at), "ok");
        b.pull(relay, prune_at);
        // D comes back: its cursors are behind what the relay holds.
        let back = prune_at + 10;
        let (envs, gap, answer, cursors) = d.pull_raw(relay, back);
        assert!(gap, "the relay pruned past D's cursors");
        let (who, _, _) = answer.snapshots.first().cloned().expect("a snapshot is listed");
        let r = d.read(relay, "fetch_snapshot", back, |s, epoch, nonce| {
            client::fetch_snapshot(&channel(), s, epoch, nonce, who, back)
        });
        assert_eq!(code(&r), "ok");
        let (snap, _) = client::parse_fetch(&r.body).unwrap();
        for batch in envs.chunks(32) {
            d.k.relay_verify(batch.to_vec());
        }
        let s = d.k.adopt_snapshot(snap, envs, back);
        d.drive(s);
        d.pulled(answer.generation, cursors);
        d.pull(relay, back + 1);
        assert_eq!(d.k.view_all(), a.k.view_all(), "D converges from the relay alone");
        // D's own writes flow again.
        d.put("d-back", 3, back + 2);
        assert_eq!(d.push(relay, back + 2), "ok");
        a.pull(relay, back + 3);
        assert_eq!(d.k.view_all(), a.k.view_all());
    }
}

#[test]
fn a_household_back_after_its_channel_expired_starts_over_by_itself() {
    const IDLE: u64 = 1_000_000;
    for (side, mut relay) in sides("expiry", &[("idle_ms", IDLE)]) {
        let relay = relay.as_mut();
        let mut a = Dev::join(1, relay, T0);
        let mut b = Dev::join(2, relay, T0 + 1);
        a.put("a1", 1, T0 + 2);
        b.put("b1", 2, T0 + 2);
        a.push(relay, T0 + 3);
        b.push(relay, T0 + 3);
        a.pull(relay, T0 + 4);
        b.pull(relay, T0 + 4);
        let first = a.k.relay_state().generation;
        assert!(first > 0, "{side}: the enrolment named the channel's generation");
        assert_eq!(b.k.relay_state().generation, first);
        // The household goes quiet; the channel expires at the sweep (the clock is ours).
        let back = T0 + 3 + IDLE;
        relay.sweep(back);
        // Offline meanwhile, B edited. Back online, each device finds itself unknown,
        // enrols again, learns the new generation, starts over and re-uploads.
        b.put("b-offline", 3, back);
        a.pull(relay, back);
        let state = a.k.relay_state();
        assert!(state.generation > first, "{side}: a new generation");
        assert_eq!(a.push(relay, back), "ok", "{side}: A's own log starts over at seq 1");
        b.pull(relay, back + 1);
        assert_eq!(b.push(relay, back + 1), "ok");
        a.pull(relay, back + 2);
        assert_eq!(a.k.view_all(), b.k.view_all(), "{side}: A and B converge again");
        assert!(a.k.view_all().rows.iter().any(|r| r.row == "b-offline"));
        // A device that never knew the old channel gets everything from the new one.
        let mut c = Dev::join(3, relay, back + 3);
        c.pull(relay, back + 4);
        assert_eq!(c.k.view_all(), a.k.view_all(), "{side}: a new device converges from the relay alone");
        // And writes keep flowing both ways.
        c.put("c1", 4, back + 5);
        c.push(relay, back + 5);
        a.pull(relay, back + 6);
        assert!(a.k.view_all().rows.iter().any(|r| r.row == "c1"));
    }
}
