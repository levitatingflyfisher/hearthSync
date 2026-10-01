//! The request handler: routing, the checks in the order the protocol fixes, and the
//! store updates. Pure apart from the store: time comes in as `now` (Unix millis), so the
//! conformance vectors and tests drive it deterministically. The HTTP layer reads the
//! clock and calls [`Relay::handle`].

use std::collections::HashMap;
use std::path::Path;

use dcbor::CBOR;
use hearth_sync_kernel::keys::{enroll_auth_msg, forget_auth_msg};
use hearth_sync_kernel::op::verify_strict;
use hearth_sync_kernel::seal::envelope_ref;
use hearth_sync_kernel::{sha256, DeviceId};

use crate::config::Config;
use crate::store::{DeviceRow, Store, StoreError, Tx};
use crate::wire::{self, channel_id, nesting_ok, Channel, Nonce, MAX_DEPTH};

/// What the handler answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: &'static str,
    /// The verb, for logs (never the channel or a device).
    pub verb: &'static str,
}

/// An error answer: a code from the protocol's table, sometimes with an extra item.
#[derive(Debug)]
enum Fail {
    Code(&'static str),
    Seq(u64),
    Epoch(u64),
    Store(StoreError),
}

impl From<StoreError> for Fail {
    fn from(e: StoreError) -> Self {
        Fail::Store(e)
    }
}

type R<T> = Result<T, Fail>;

fn status(code: &str) -> u16 {
    match code {
        "bad_request" | "channel_mismatch" | "bad_envelope" => 400,
        "stale" | "bad_signature" | "replay" => 401,
        "bad_auth" | "not_enrolled" | "forgotten" => 403,
        "no_snapshot" | "not_found" => 404,
        "method" => 405,
        "seq" | "epoch" => 409,
        "too_large" => 413,
        "rate_limited" => 429,
        "quota" => 507,
        _ => 500,
    }
}

/// The error body `["err", code]`.
pub fn error_body(code: &str) -> Vec<u8> {
    CBOR::from(vec![CBOR::from("err"), CBOR::from(code)]).to_cbor_data()
}

fn ok(items: Vec<CBOR>) -> Vec<u8> {
    let mut v = vec![CBOR::from("ok")];
    v.extend(items);
    CBOR::from(v).to_cbor_data()
}

/// A token bucket (docs/reference/relay-protocol.md, "Rate limits").
#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: u64,
    last: u64,
}

impl Bucket {
    fn take(&mut self, burst: u64, interval: u64, now: u64) -> bool {
        if now > self.last {
            let add = (now - self.last) / interval.max(1);
            self.tokens = burst.min(self.tokens.saturating_add(add));
            self.last = if self.tokens == burst { now } else { self.last + add * interval.max(1) };
        }
        if self.tokens == 0 {
            return false;
        }
        self.tokens -= 1;
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum BucketKey {
    Device(Channel, DeviceId),
    Channel(Channel),
    Enroll(Channel),
    Create,
}

/// Buckets are memory only; past this many, the ones that would be full by now are
/// dropped (forgetting them changes nothing).
const MAX_BUCKETS: usize = 100_000;

pub struct Relay {
    store: Store,
    cfg: Config,
    buckets: HashMap<BucketKey, Bucket>,
    /// Live read nonces and their expiry, per reader: at most `max_reader_nonces` each,
    /// so no reader or household can use up another's (no relay-wide cap). Memory only:
    /// the epoch is what makes them safe across a restart.
    nonces: HashMap<(Channel, DeviceId), Vec<(Nonce, u64)>>,
}

impl Relay {
    /// Open the relay over the database in `dir` (in memory when `None`). Every open
    /// is a start: the store's epoch moves on.
    pub fn open(dir: Option<&Path>, cfg: Config) -> Result<Relay, StoreError> {
        Ok(Relay { store: Store::open(dir)?, cfg, buckets: HashMap::new(), nonces: HashMap::new() })
    }

    /// As [`Relay::open`], without fsync: for tests and the differential harness only
    /// (the Go relay's `noSync`). A crash may lose committed requests.
    pub fn open_unsynced(dir: &Path, cfg: Config) -> Result<Relay, StoreError> {
        Ok(Relay { store: Store::open_with(Some(dir), false)?, cfg, buckets: HashMap::new(), nonces: HashMap::new() })
    }

    /// The epoch reads must name.
    pub fn epoch(&self) -> u64 {
        self.store.epoch()
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// SHA-256 of the canonical store dump (conformance).
    pub fn digest(&mut self) -> Result<[u8; 32], StoreError> {
        let tx = self.store.read()?;
        Ok(sha256(&tx.dump()?))
    }

    /// The server's sweep as of `now`: prune every channel (requests prune their own),
    /// expire idle channels and drop expired nonces. Returns the number of entries
    /// pruned.
    pub fn sweep(&mut self, now: u64) -> Result<u64, StoreError> {
        self.sweep_ahead(now, 0)
    }

    /// The sweep, with pruning and idle expiry run as of `now + ahead_ms` (nonces
    /// still expire as of `now`). For tests only: the HTTP layer's `--test-hooks`
    /// hook, so an expiry test needs neither a restart nor a fake clock.
    pub fn sweep_ahead(&mut self, now: u64, ahead_ms: u64) -> Result<u64, StoreError> {
        let later = now.saturating_add(ahead_ms);
        let tx = self.store.tx()?;
        let mut n = 0;
        for ch in tx.channel_ids()? {
            n += tx.prune(&ch, later, self.cfg.retain_ms)?;
        }
        tx.expire_idle(later, self.cfg.idle_ms)?;
        tx.commit()?;
        self.nonces.retain(|_, live| {
            live.retain(|(_, exp)| *exp >= now);
            !live.is_empty()
        });
        Ok(n)
    }

    /// Answer one request.
    pub fn handle(&mut self, method: &str, path: &str, body: &[u8], now: u64) -> Response {
        if path == "/healthz" && method == "GET" {
            return Response { status: 200, body: b"ok".to_vec(), content_type: "text/plain", verb: "healthz" };
        }
        let Some((ch, verb)) = route(path) else { return self.fail("-", "not_found") };
        if method != "POST" {
            return self.fail(verb, "method");
        }
        let res = match verb {
            "enroll" => self.enroll(&ch, body, now),
            "forget" => self.forget(&ch, body, now),
            "append" => self.append(&ch, body, now),
            "snapshot" => self.snapshot(&ch, body, now),
            "pull" => self.pull(&ch, body, now),
            "fetch_snapshot" => self.fetch_snapshot(&ch, body, now),
            _ => unreachable!("route() returns known verbs only"),
        };
        match res {
            Ok(body) => Response { status: 200, body, content_type: "application/cbor", verb },
            Err(Fail::Code(c)) => self.fail(verb, c),
            Err(Fail::Seq(n)) => conflict(verb, "seq", n),
            Err(Fail::Epoch(n)) => conflict(verb, "epoch", n),
            Err(Fail::Store(e)) => {
                crate::log::event("error", &[("event", "store"), ("verb", verb), ("error", &e.to_string())]);
                Response { status: 500, body: error_body("internal"), content_type: "application/cbor", verb }
            }
        }
    }

    fn fail(&self, verb: &'static str, code: &'static str) -> Response {
        Response { status: status(code), body: error_body(code), content_type: "application/cbor", verb }
    }

    // ------------------------------------------------------------ shared checks

    fn bucket(&mut self, key: BucketKey, burst: u64, interval: u64, now: u64) -> R<()> {
        if self.buckets.len() >= MAX_BUCKETS && !self.buckets.contains_key(&key) {
            // Drop buckets that would be full by now: forgetting them changes nothing.
            let cfg = self.cfg.clone();
            self.buckets.retain(|k, b| {
                let (burst, interval) = match k {
                    BucketKey::Device(..) => (cfg.device_burst, cfg.device_interval_ms),
                    BucketKey::Channel(_) => (cfg.channel_burst, cfg.channel_interval_ms),
                    BucketKey::Enroll(_) => (cfg.enroll_burst, cfg.enroll_interval_ms),
                    BucketKey::Create => (cfg.create_burst, cfg.create_interval_ms),
                };
                b.tokens + now.saturating_sub(b.last) / interval.max(1) < burst
            });
        }
        let b = self.buckets.entry(key).or_insert(Bucket { tokens: burst, last: now });
        if b.take(burst, interval, now) {
            Ok(())
        } else {
            Err(Fail::Code("rate_limited"))
        }
    }

    /// "Rate" in the protocol: the signer's device bucket, then the channel's (a
    /// channel refusal keeps the device token).
    fn device_rate(&mut self, ch: &Channel, d: &DeviceId, now: u64) -> R<()> {
        let (burst, interval) = (self.cfg.device_burst, self.cfg.device_interval_ms);
        self.bucket(BucketKey::Device(*ch, *d), burst, interval, now)?;
        let (burst, interval) = (self.cfg.channel_burst, self.cfg.channel_interval_ms);
        self.bucket(BucketKey::Channel(*ch), burst, interval, now)
    }

    fn epoch_ok(&self, epoch: u64) -> R<()> {
        if epoch != self.store.epoch() {
            return Err(Fail::Epoch(self.store.epoch()));
        }
        Ok(())
    }

    fn fresh(&self, ts: u64, now: u64) -> R<()> {
        let w = self.cfg.window_ms;
        if ts < now.saturating_sub(w) || ts > now.saturating_add(w) {
            return Err(Fail::Code("stale"));
        }
        Ok(())
    }

    fn nonce(&mut self, ch: &Channel, reader: &DeviceId, nonce: &Nonce, ts: u64, now: u64) -> R<()> {
        let (cap, exp) = (self.cfg.max_reader_nonces, ts.saturating_add(self.cfg.window_ms));
        let live = self.nonces.entry((*ch, *reader)).or_default();
        live.retain(|(_, e)| *e >= now);
        let res = if live.iter().any(|(n, _)| n == nonce) {
            Err(Fail::Code("replay"))
        } else if live.len() as u64 >= cap {
            // Refusing is safer than forgetting a live nonce; only this reader waits.
            Err(Fail::Code("rate_limited"))
        } else {
            live.push((*nonce, exp));
            Ok(())
        };
        if live.is_empty() {
            self.nonces.remove(&(*ch, *reader));
        }
        res
    }

    // ------------------------------------------------------------ verbs

    fn enroll(&mut self, ch: &Channel, body: &[u8], now: u64) -> R<Vec<u8>> {
        let r = wire::Enroll::parse(body).map_err(|_| Fail::Code("bad_request"))?;
        if channel_id(&r.app, &r.household) != *ch {
            return Err(Fail::Code("channel_mismatch"));
        }
        if !verify_strict(&r.household, &enroll_auth_msg(&r.app, &r.device, &r.label), &r.auth) {
            return Err(Fail::Code("bad_auth"));
        }
        let cfg = self.cfg.clone();
        let tx = self.store.tx()?;
        let chan = tx.channel(ch)?;
        let dev = if chan.is_some() { tx.device(ch, &r.device)? } else { None };
        if dev.as_ref().is_some_and(DeviceRow::forgotten) {
            return Err(Fail::Code("forgotten"));
        }
        if chan.is_none() && tx.channel_count()? >= cfg.max_channels {
            return Err(Fail::Code("quota"));
        }
        drop(tx);
        if chan.is_none() {
            self.bucket(BucketKey::Create, cfg.create_burst, cfg.create_interval_ms, now)?;
        }
        self.bucket(BucketKey::Enroll(*ch), cfg.enroll_burst, cfg.enroll_interval_ms, now)?;
        if let (Some(c), true) = (&chan, dev.as_ref().is_some_and(DeviceRow::enrolled)) {
            return Ok(ok(vec![CBOR::from(c.generation)]));
        }
        let tx = self.store.tx()?;
        if chan.is_some() && tx.device_count(ch)? >= cfg.max_devices {
            return Err(Fail::Code("quota"));
        }
        if chan.is_none() {
            tx.create_channel(ch, &r.app, &r.household, now)?;
        }
        tx.put_device(
            ch,
            &r.device,
            &DeviceRow { label: Some(r.label), auth: Some(r.auth.to_vec()), cut_seq: None, freeze_ord: None },
        )?;
        tx.touch(ch, now)?;
        let generation = tx.channel(ch)?.map(|c| c.generation).unwrap_or(0);
        tx.commit()?;
        Ok(ok(vec![CBOR::from(generation)]))
    }

    /// The signer's channel and record, if the signer is enrolled (forgotten or not).
    fn enrolled(&mut self, ch: &Channel, d: &DeviceId) -> R<(crate::store::ChannelRow, DeviceRow)> {
        let tx = self.store.read()?;
        let Some(c) = tx.channel(ch)? else { return Err(Fail::Code("not_enrolled")) };
        match tx.device(ch, d)? {
            Some(row) if row.enrolled() => Ok((c, row)),
            _ => Err(Fail::Code("not_enrolled")),
        }
    }

    /// A pull's reader: enrolled, or forgotten. A Forget can be recorded for a
    /// target that never enrolled in the channel (the household came back after an
    /// expiry and forgot it first), and that target must still read its Forget op.
    fn reader(&mut self, ch: &Channel, d: &DeviceId) -> R<(crate::store::ChannelRow, DeviceRow)> {
        let tx = self.store.read()?;
        let Some(c) = tx.channel(ch)? else { return Err(Fail::Code("not_enrolled")) };
        match tx.device(ch, d)? {
            Some(row) if row.enrolled() || row.forgotten() => Ok((c, row)),
            _ => Err(Fail::Code("not_enrolled")),
        }
    }

    fn signed(&self, signer: &DeviceId, msg: &[u8], sig: &[u8; 64]) -> R<()> {
        if !verify_strict(signer, msg, sig) {
            return Err(Fail::Code("bad_signature"));
        }
        Ok(())
    }

    fn forget(&mut self, ch: &Channel, body: &[u8], now: u64) -> R<Vec<u8>> {
        let r = wire::Forget::parse(body).map_err(|_| Fail::Code("bad_request"))?;
        let (c, _) = self.enrolled(ch, &r.poster)?;
        self.fresh(r.ts, now)?;
        self.signed(&r.poster, &r.signable(ch), &r.sig)?;
        if !verify_strict(&c.household, &forget_auth_msg(&c.app, &r.target, &r.cut), &r.auth) {
            return Err(Fail::Code("bad_auth"));
        }
        self.device_rate(ch, &r.poster, now)?;
        let tx = self.store.tx()?;
        let existing = tx.device(ch, &r.target)?;
        if existing.is_none() && tx.device_count(ch)? >= self.cfg.max_devices {
            return Err(Fail::Code("quota"));
        }
        let mut row = existing.unwrap_or_default();
        match row.cut_seq {
            None => {
                row.cut_seq = Some(r.cut_seq);
                row.freeze_ord = Some(c.next_ord - 1);
            }
            Some(s) => row.cut_seq = Some(s.min(r.cut_seq)),
        }
        tx.put_device(ch, &r.target, &row)?;
        tx.touch(ch, now)?;
        tx.commit()?;
        Ok(ok(vec![]))
    }

    fn append(&mut self, ch: &Channel, body: &[u8], now: u64) -> R<Vec<u8>> {
        let r = wire::Append::parse(body).map_err(|_| Fail::Code("bad_request"))?;
        if r.envelopes.len() as u64 > self.cfg.max_batch
            || r.envelopes.iter().any(|e| e.len() as u64 > self.cfg.max_envelope)
        {
            return Err(Fail::Code("too_large"));
        }
        let (_, dev) = self.enrolled(ch, &r.uploader)?;
        self.fresh(r.ts, now)?;
        self.signed(&r.uploader, &r.signable(ch), &r.sig)?;
        self.device_rate(ch, &r.uploader, now)?;
        if !r.envelopes.iter().all(|e| has_ref(e)) {
            return Err(Fail::Code("bad_envelope"));
        }
        let n = r.envelopes.len() as u64;
        // The batch's last seq; past u64::MAX it is above any cut.
        let top = r.first_seq.checked_add(n - 1);
        if dev.cut_seq.is_some_and(|cut| top.is_none_or(|t| t > cut)) {
            return Err(Fail::Code("forgotten"));
        }
        let tx = self.store.tx()?;
        let last = tx.last(ch, &r.uploader)?;
        if r.first_seq > last.saturating_add(1) {
            return Err(Fail::Seq(last));
        }
        let mut new: Vec<(u64, &[u8])> = Vec::new();
        for (k, env) in r.envelopes.iter().enumerate() {
            let seq = r.first_seq + k as u64;
            if seq <= last {
                if tx.entry(ch, &r.uploader, seq)?.is_some_and(|held| held != *env) {
                    return Err(Fail::Seq(last));
                }
            } else {
                new.push((seq, env));
            }
        }
        tx.prune(ch, now, self.cfg.retain_ms)?;
        let add: u64 = new.iter().map(|(_, e)| e.len() as u64).sum();
        let used = tx.channel(ch)?.map(|c| c.bytes).unwrap_or(0);
        quota(&self.cfg, &tx, used, add as i64)?;
        let last = if new.is_empty() { last } else { tx.append(ch, &r.uploader, &new, now)? };
        tx.touch(ch, now)?;
        tx.commit()?;
        Ok(ok(vec![CBOR::from(last)]))
    }

    fn snapshot(&mut self, ch: &Channel, body: &[u8], now: u64) -> R<Vec<u8>> {
        let r = wire::Snapshot::parse(body).map_err(|_| Fail::Code("bad_request"))?;
        if r.envelope.len() as u64 > self.cfg.max_snapshot || r.covers.len() as u64 > self.cfg.max_devices {
            return Err(Fail::Code("too_large"));
        }
        let (_, dev) = self.enrolled(ch, &r.device)?;
        self.fresh(r.ts, now)?;
        self.signed(&r.device, &r.signable(ch), &r.sig)?;
        if dev.forgotten() {
            return Err(Fail::Code("forgotten"));
        }
        self.device_rate(ch, &r.device, now)?;
        if !has_ref(&r.envelope) {
            return Err(Fail::Code("bad_envelope"));
        }
        let tx = self.store.tx()?;
        tx.prune(ch, now, self.cfg.retain_ms)?;
        let old = tx.snapshot(ch, &r.device)?.map(|(e, _)| e.len() as i64).unwrap_or(0);
        let used = tx.channel(ch)?.map(|c| c.bytes).unwrap_or(0);
        quota(&self.cfg, &tx, used, r.envelope.len() as i64 - old)?;
        tx.put_snapshot(ch, &r.device, &r.envelope, &r.covers, now)?;
        tx.touch(ch, now)?;
        tx.commit()?;
        Ok(ok(vec![]))
    }

    fn pull(&mut self, ch: &Channel, body: &[u8], now: u64) -> R<Vec<u8>> {
        let r = wire::Pull::parse(body).map_err(|_| Fail::Code("bad_request"))?;
        if r.cursors.len() as u64 > self.cfg.max_devices {
            return Err(Fail::Code("too_large"));
        }
        let (c, me) = self.reader(ch, &r.reader)?;
        self.fresh(r.ts, now)?;
        self.signed(&r.reader, &r.signable(ch), &r.sig)?;
        self.epoch_ok(r.epoch)?;
        self.nonce(ch, &r.reader, &r.nonce, r.ts, now)?;
        self.device_rate(ch, &r.reader, now)?;
        let freeze = me.freeze_ord.filter(|_| me.forgotten());
        let cursors: HashMap<DeviceId, u64> = r.cursors.iter().copied().collect();
        let (max_n, max_bytes) = (self.cfg.max_pull_entries, self.cfg.max_pull_bytes);
        let tx = self.store.read()?;
        let (mut n, mut size, mut more) = (0u64, 0u64, false);
        let mut logs = Vec::new();
        for (up, last) in tx.logs(ch)? {
            let first = tx.first_held(ch, &up)?.unwrap_or(last + 1);
            let mut got = Vec::new();
            if !more {
                tx.scan(ch, &up, cursors.get(&up).copied().unwrap_or(0), |seq, ord, env| {
                    if freeze.is_some_and(|f| ord > f) {
                        return true;
                    }
                    if n >= max_n || (n > 0 && size + env.len() as u64 > max_bytes) {
                        more = true;
                        return false;
                    }
                    n += 1;
                    size += env.len() as u64;
                    got.push(CBOR::from(vec![CBOR::from(seq), CBOR::to_byte_string(env)]));
                    true
                })?;
            }
            logs.push(CBOR::from(vec![CBOR::to_byte_string(up), CBOR::from(first), CBOR::from(got)]));
        }
        let snaps = if freeze.is_some() {
            Vec::new()
        } else {
            tx.snapshots(ch)?
                .into_iter()
                .map(|(d, env, covers)| {
                    let reference = envelope_ref(&env).unwrap_or([0; 32]);
                    CBOR::from(vec![
                        CBOR::to_byte_string(d),
                        CBOR::to_byte_string(reference),
                        wire::pairs_cbor(&covers),
                    ])
                })
                .collect()
        };
        Ok(ok(vec![CBOR::from(c.generation), CBOR::from(logs), CBOR::from(snaps), CBOR::from(more)]))
    }

    fn fetch_snapshot(&mut self, ch: &Channel, body: &[u8], now: u64) -> R<Vec<u8>> {
        let r = wire::FetchSnapshot::parse(body).map_err(|_| Fail::Code("bad_request"))?;
        let (_, me) = self.enrolled(ch, &r.reader)?;
        self.fresh(r.ts, now)?;
        self.signed(&r.reader, &r.signable(ch), &r.sig)?;
        self.epoch_ok(r.epoch)?;
        self.nonce(ch, &r.reader, &r.nonce, r.ts, now)?;
        if me.forgotten() {
            return Err(Fail::Code("forgotten"));
        }
        self.device_rate(ch, &r.reader, now)?;
        let tx = self.store.read()?;
        let Some((env, covers)) = tx.snapshot(ch, &r.device)? else { return Err(Fail::Code("no_snapshot")) };
        Ok(ok(vec![CBOR::to_byte_string(env), wire::pairs_cbor(&covers)]))
    }
}

/// A 409 answer that names a number: the log's last seq, or the relay's epoch.
fn conflict(verb: &'static str, code: &str, n: u64) -> Response {
    Response {
        status: 409,
        body: CBOR::from(vec![CBOR::from("err"), CBOR::from(code), CBOR::from(n)]).to_cbor_data(),
        content_type: "application/cbor",
        verb,
    }
}

/// The channel (holding `channel_bytes`) and the relay may grow by `add` bytes.
fn quota(cfg: &Config, tx: &Tx<'_>, channel_bytes: u64, add: i64) -> R<()> {
    let total = tx.total_bytes()?;
    let over = |used: u64, cap: u64| (used as i128 + add as i128) > cap as i128;
    if over(channel_bytes, cfg.channel_quota) || over(total, cfg.max_total_bytes) {
        return Err(Fail::Code("quota"));
    }
    Ok(())
}

/// A sealed envelope with a ref, checked without opening it. The nesting guard runs
/// first: the envelope parser recurses, and an envelope is attacker-chosen bytes.
fn has_ref(env: &[u8]) -> bool {
    nesting_ok(env, MAX_DEPTH) && envelope_ref(env).is_some()
}

/// `/v1/{channel}/{verb}` with a 64-digit lowercase hex channel and a known verb.
fn route(path: &str) -> Option<(Channel, &'static str)> {
    let rest = path.strip_prefix("/v1/")?;
    let (hex, verb) = rest.split_once('/')?;
    let verb = match verb {
        "enroll" => "enroll",
        "forget" => "forget",
        "append" => "append",
        "snapshot" => "snapshot",
        "pull" => "pull",
        "fetch_snapshot" => "fetch_snapshot",
        _ => return None,
    };
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    let mut ch = [0u8; 32];
    for (k, pair) in hex.as_bytes().chunks(2).enumerate() {
        let d = |b: u8| if b.is_ascii_digit() { b - b'0' } else { b - b'a' + 10 };
        ch[k] = d(pair[0]) << 4 | d(pair[1]);
    }
    Some((ch, verb))
}
