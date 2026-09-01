//! Persistence without filesystem access (ADR 0010).
//!
//! The kernel never touches a file. After each call the app asks it for a
//! [`Changeset`]: opaque, versioned records, keyed by short byte strings, that the
//! app writes into its own database (a Drift table `hearth_records(key BLOB PRIMARY
//! KEY, value BLOB)` on native, an IndexedDB object store on the web) in the same
//! transaction as the row changes it applies. On the next launch the app hands every
//! record back to [`Replica::load`], which rebuilds the replica.
//!
//! **Records.** A key is one tag byte, followed by an op id where there is one:
//!
//! | Tag | Key | Value (dCBOR array, first item the record version) |
//! |---|---|---|
//! | 0x01 | meta | `[1, app, household, device or null, wiped, schema fingerprint or null, [newest clock]]` |
//! | 0x02 | base | `[1, checkpoint id, [kept ids], sealed base state]` |
//! | 0x10 | op + id | `[1, index entry, sealed op or null once pruned]` |
//! | 0x11 | pending + id | `[1, sealed op]` (waiting for a parent) |
//! | 0x12 | quarantined + id | `[1, sealed op]` (V8) |
//! | 0x13 | held + id | `[1, sealed op]` (V7) |
//! | 0x14 | rejected + id | `[1, reject code]` |
//! | 0x20 | app + any | reserved for the api layer (the review list) |
//!
//! Ops and the base state are sealed (ADR 0008); index entries are not, since the
//! app's own tables already hold the data in the clear. The enrolment cache, the
//! ancestry needed by V4–V6 and the folded state are rebuilt on load from the index
//! and the bodies, once per session, rather than stored.
//!
//! **Write order and crash safety.** Apply each changeset in one transaction, in
//! the order given, and before sending anything the same call produced. Both
//! Drift and IndexedDB transactions are atomic. A changeset with `reset` set (a
//! snapshot was adopted, which replaces the whole log) must be applied atomically:
//! delete every record, then write the new ones. Other changesets are also ordered
//! so that a prefix cut short by a crash still loads:
//!
//! 1. new op records, parents first (clock order);
//! 2. pending, quarantined, held and rejected records that were added;
//! 3. the base;
//! 4. meta;
//! 5. op records whose body was pruned, and removals of pending, quarantined and
//!    held records.
//!
//! Load tolerates every prefix of that order: a pending record whose op already
//! delivered is dropped, a body the base no longer needs is pruned again, and an
//! op whose parent's record is missing is parked as pending. The persistence tests
//! cut changesets at random points and check this.

use std::collections::{BTreeMap, BTreeSet};

use dcbor::{CBORCase, CBOR};

use crate::keys::{DeviceSigner, HouseholdRoot};
use crate::op::{self, Hlc, Reject};
use crate::replica::{closure_in, Base, Config, EnrollCache, IngestReport, Replica};
use crate::seal::{SealError, SealKeys, SealKind};
use crate::state::State;
use crate::store::{IndexEntry, OpStore};
use crate::{DeviceId, Id};

/// Record format version.
pub const RECORD_V: u64 = 1;

pub const TAG_META: u8 = 0x01;
pub const TAG_BASE: u8 = 0x02;
pub const TAG_OP: u8 = 0x10;
pub const TAG_PENDING: u8 = 0x11;
pub const TAG_QUARANTINED: u8 = 0x12;
pub const TAG_HELD: u8 = 0x13;
pub const TAG_REJECTED: u8 = 0x14;
/// Keys starting with this tag belong to the layer above the replica (the api).
pub const TAG_APP: u8 = 0x20;

/// A persisted record's key, as the replica tracks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RecordKey {
    Meta,
    Base,
    Op(Id),
    Pending(Id),
    Quarantined(Id),
    Held(Id),
    Rejected(Id),
}

impl RecordKey {
    pub fn bytes(&self) -> Vec<u8> {
        let (tag, id) = match self {
            RecordKey::Meta => (TAG_META, None),
            RecordKey::Base => (TAG_BASE, None),
            RecordKey::Op(i) => (TAG_OP, Some(i)),
            RecordKey::Pending(i) => (TAG_PENDING, Some(i)),
            RecordKey::Quarantined(i) => (TAG_QUARANTINED, Some(i)),
            RecordKey::Held(i) => (TAG_HELD, Some(i)),
            RecordKey::Rejected(i) => (TAG_REJECTED, Some(i)),
        };
        let mut k = vec![tag];
        if let Some(i) = id {
            k.extend_from_slice(i);
        }
        k
    }

    pub fn parse(key: &[u8]) -> Option<RecordKey> {
        let (&tag, rest) = key.split_first()?;
        let id = || <[u8; 32]>::try_from(rest).ok();
        Some(match tag {
            TAG_META if rest.is_empty() => RecordKey::Meta,
            TAG_BASE if rest.is_empty() => RecordKey::Base,
            TAG_OP => RecordKey::Op(id()?),
            TAG_PENDING => RecordKey::Pending(id()?),
            TAG_QUARANTINED => RecordKey::Quarantined(id()?),
            TAG_HELD => RecordKey::Held(id()?),
            TAG_REJECTED => RecordKey::Rejected(id()?),
            _ => return None,
        })
    }
}

/// One record to write (`value` set) or delete (`value` empty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// The record's key: a tag byte, then an op id where it belongs to one.
    pub key: Vec<u8>,
    /// The value to store, or `None` to delete the record.
    pub value: Option<Vec<u8>>,
}

/// What one call changed, in write order (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changeset {
    /// Delete every record first (a snapshot replaced the whole log).
    pub reset: bool,
    pub records: Vec<Record>,
}

impl Changeset {
    pub fn is_empty(&self) -> bool {
        !self.reset && self.records.is_empty()
    }
}

/// Where the app keeps the records. Implemented in Dart over Drift or IndexedDB; the
/// kernel ships [`MemPersist`] for tests and for the api's own round-trip test.
pub trait Persist {
    /// Apply one changeset atomically (reset first, then the records in order).
    fn apply(&mut self, changes: &Changeset);
    /// Every stored record, in any order.
    fn records(&self) -> Vec<(Vec<u8>, Vec<u8>)>;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemPersist {
    pub map: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl Persist for MemPersist {
    fn apply(&mut self, changes: &Changeset) {
        if changes.reset {
            self.map.clear();
        }
        for r in &changes.records {
            match &r.value {
                Some(v) => {
                    self.map.insert(r.key.clone(), v.clone());
                }
                None => {
                    self.map.remove(&r.key);
                }
            }
        }
    }
    fn records(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// Why stored records could not be loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistError {
    /// No meta record: nothing was ever stored (create a new replica instead).
    NoMeta,
    /// A record this version cannot read (wrong version, shape, or key).
    Malformed(Vec<u8>),
    /// A sealed record did not open: another household's keys, or tampering.
    Seal(Vec<u8>, SealError),
    /// The records belong to another app or household than the keys given.
    WrongHousehold,
    /// The records belong to another device than the signer given.
    WrongDevice,
    /// The base names a checkpoint whose record is missing.
    MissingBase,
}

/// Which records changed since the last [`Replica::take_changes`].
#[derive(Clone, Debug, Default)]
pub(crate) struct Journal {
    reset: bool,
    dirty: BTreeSet<RecordKey>,
}

impl Journal {
    /// A new replica: its meta record has never been written.
    pub(crate) fn fresh() -> Self {
        Journal { reset: false, dirty: BTreeSet::from([RecordKey::Meta]) }
    }
    pub(crate) fn mark(&mut self, k: RecordKey) {
        if !self.reset {
            self.dirty.insert(k);
        }
    }
    pub(crate) fn reset(&mut self) {
        self.reset = true;
        self.dirty.clear();
    }
}

// ---------------------------------------------------------------- encoding helpers

type R<T> = Result<T, ()>;

fn arr(c: &CBOR) -> R<&Vec<CBOR>> {
    match c.as_case() {
        CBORCase::Array(a) => Ok(a),
        _ => Err(()),
    }
}
fn bytes(c: &CBOR) -> R<Vec<u8>> {
    match c.as_case() {
        CBORCase::ByteString(b) => Ok(b.data().to_vec()),
        _ => Err(()),
    }
}
fn b32(c: &CBOR) -> R<[u8; 32]> {
    bytes(c)?.as_slice().try_into().map_err(|_| ())
}
fn text(c: &CBOR) -> R<String> {
    match c.as_case() {
        CBORCase::Text(s) => Ok(s.clone()),
        _ => Err(()),
    }
}
fn boolean(c: &CBOR) -> R<bool> {
    c.clone().try_into().map_err(|_| ())
}
fn opt32(c: &CBOR) -> R<Option<[u8; 32]>> {
    if c.is_null() {
        Ok(None)
    } else {
        b32(c).map(Some)
    }
}
fn opt_bytes(c: &CBOR) -> R<Option<Vec<u8>>> {
    if c.is_null() {
        Ok(None)
    } else {
        bytes(c).map(Some)
    }
}
fn null_or<T>(v: Option<T>, f: impl Fn(T) -> CBOR) -> CBOR {
    v.map(f).unwrap_or_else(CBOR::null)
}

/// Parse a record value: canonical dCBOR, an array of `len` items, version first.
fn parse_value(v: &[u8], len: usize) -> R<Vec<CBOR>> {
    let c = crate::cbor::decode(v).ok_or(())?;
    let a = arr(&c)?.clone();
    let version: u64 = a.first().cloned().ok_or(())?.try_into().map_err(|_| ())?;
    if a.len() != len || version != RECORD_V {
        return Err(());
    }
    Ok(a)
}

fn record(items: Vec<CBOR>) -> Vec<u8> {
    let mut all = vec![CBOR::from(RECORD_V)];
    all.extend(items);
    CBOR::from(all).to_cbor_data()
}

impl<S: OpStore + Default> Replica<S> {
    /// The value of one record as the replica stands now, or `None` if it no
    /// longer exists.
    fn record_value(&self, k: &RecordKey, keys: &SealKeys) -> Option<Vec<u8>> {
        let sealed = |bytes: &[u8]| CBOR::to_byte_string(keys.seal_op(bytes));
        match k {
            RecordKey::Meta => Some(record(vec![
                CBOR::from(self.app.as_str()),
                CBOR::to_byte_string(self.enroll_pk),
                null_or(self.me, CBOR::to_byte_string),
                CBOR::from(self.wiped),
                null_or(self.schema_fp, CBOR::to_byte_string),
                CBOR::from(vec![CBOR::from(self.max_hlc.millis), CBOR::from(self.max_hlc.counter as u64)]),
            ])),
            RecordKey::Base => self.base.as_ref().map(|b| {
                record(vec![
                    CBOR::to_byte_string(b.checkpoint),
                    CBOR::from(b.keep.iter().map(CBOR::to_byte_string).collect::<Vec<_>>()),
                    CBOR::to_byte_string(keys.seal(SealKind::Snapshot, Some(&b.checkpoint), &b.state.encode())),
                ])
            }),
            RecordKey::Op(id) => {
                self.store.entry(id).map(|e| record(vec![e.to_cbor(id), null_or(self.store.body(id), sealed)]))
            }
            RecordKey::Pending(id) => self.pending.get(id).map(|(_, b)| record(vec![sealed(b)])),
            RecordKey::Quarantined(id) => self.quarantined.get(id).map(|(_, b)| record(vec![sealed(b)])),
            RecordKey::Held(id) => self.held.get(id).map(|(_, b)| record(vec![sealed(b)])),
            RecordKey::Rejected(id) => self.rejected.get(id).map(|r| record(vec![CBOR::from(r.code())])),
        }
    }

    /// Every record key the replica would store now.
    fn all_keys(&self) -> Vec<RecordKey> {
        let mut v = vec![RecordKey::Meta];
        if self.base.is_some() {
            v.push(RecordKey::Base);
        }
        v.extend(self.store.ids().into_iter().map(RecordKey::Op));
        v.extend(self.pending.keys().map(|i| RecordKey::Pending(*i)));
        v.extend(self.quarantined.keys().map(|i| RecordKey::Quarantined(*i)));
        v.extend(self.held.keys().map(|i| RecordKey::Held(*i)));
        v.extend(self.rejected.keys().map(|i| RecordKey::Rejected(*i)));
        v
    }

    /// Order records for writing: see the module docs.
    fn ordered(&self, keys: Vec<RecordKey>, seal: &SealKeys) -> Vec<Record> {
        let mut new_ops: Vec<(Hlc, Id, Vec<u8>)> = Vec::new();
        let mut pruned_ops: Vec<(Hlc, Id, Vec<u8>)> = Vec::new();
        let (mut added, mut base, mut meta, mut removed) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for k in keys {
            let value = self.record_value(&k, seal);
            match (k, value) {
                (RecordKey::Op(id), Some(v)) => {
                    let hlc = self.store.entry(&id).expect("indexed").hlc;
                    if self.store.body(&id).is_some() {
                        new_ops.push((hlc, id, v));
                    } else {
                        pruned_ops.push((hlc, id, v));
                    }
                }
                (RecordKey::Base, v) => base.push(Record { key: k.bytes(), value: v }),
                (RecordKey::Meta, v) => meta.push(Record { key: k.bytes(), value: v }),
                (_, Some(v)) => added.push(Record { key: k.bytes(), value: Some(v) }),
                (_, None) => removed.push(Record { key: k.bytes(), value: None }),
            }
        }
        new_ops.sort();
        pruned_ops.sort();
        let op_rec = |(_, id, v): (Hlc, Id, Vec<u8>)| Record { key: RecordKey::Op(id).bytes(), value: Some(v) };
        let mut out: Vec<Record> = new_ops.into_iter().map(op_rec).collect();
        out.extend(added);
        out.extend(base);
        out.extend(meta);
        out.extend(pruned_ops.into_iter().map(op_rec));
        out.extend(removed);
        out
    }

    /// The records that changed since the last call, in write order. Ops and the
    /// base state are sealed with `keys`.
    pub fn take_changes(&mut self, keys: &SealKeys) -> Changeset {
        let j = std::mem::take(&mut self.journal);
        if j.reset {
            return Changeset { reset: true, records: self.ordered(self.all_keys(), keys) };
        }
        Changeset { reset: false, records: self.ordered(j.dirty.into_iter().collect(), keys) }
    }

    /// Every record, as a reset changeset: a full rewrite (and, since sealing is
    /// deterministic, a canonical picture of everything the replica would store).
    pub fn full_records(&self, keys: &SealKeys) -> Changeset {
        Changeset { reset: true, records: self.ordered(self.all_keys(), keys) }
    }

    /// Rebuild a replica from its stored records. `device` is the household root and
    /// this device's signer, if the device still has them; a replica stored as
    /// wiped stays keyless whatever is passed. Pending ops are re-admitted against
    /// `now` (a crash may have cut a changeset short), and anything load had to
    /// repair is left for the next [`Replica::take_changes`].
    pub fn load(
        keys: &SealKeys,
        config: Config,
        device: Option<(HouseholdRoot, Box<dyn DeviceSigner>)>,
        records: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
        now: u64,
    ) -> Result<(Self, IngestReport), PersistError> {
        // Meta first: which household, app and device the records are for, before
        // any sealed record is opened.
        let records: Vec<(Vec<u8>, Vec<u8>)> = records.into_iter().collect();
        let (app, household, me, wiped, schema_fp, top) = parse_meta(&records)?.ok_or(PersistError::NoMeta)?;
        if household != keys.household() || !crate::keys::app_domain_ok(&app) {
            return Err(PersistError::WrongHousehold);
        }
        let mut base_rec = None;
        let mut ops: Vec<(Hlc, Id, IndexEntry, Option<Vec<u8>>)> = Vec::new();
        let mut parked: Vec<(RecordKey, Vec<u8>)> = Vec::new();
        let mut rejected = BTreeMap::new();
        for (key, value) in records {
            let bad = || PersistError::Malformed(key.clone());
            let open = |c: &CBOR| -> Result<Vec<u8>, PersistError> {
                let env = bytes(c).map_err(|_| bad())?;
                keys.open_op(&env).map(|(_, b)| b).map_err(|e| PersistError::Seal(key.clone(), e))
            };
            let Some(k) = RecordKey::parse(&key) else {
                // Records of the layer above are not the replica's business.
                if key.first() == Some(&TAG_APP) {
                    continue;
                }
                return Err(bad());
            };
            match k {
                RecordKey::Meta => {}
                RecordKey::Base => {
                    let a = parse_value(&value, 4).map_err(|_| bad())?;
                    let cp = b32(&a[1]).map_err(|_| bad())?;
                    let keep: BTreeSet<Id> = arr(&a[2]).and_then(|v| v.iter().map(b32).collect()).map_err(|_| bad())?;
                    let env = bytes(&a[3]).map_err(|_| bad())?;
                    let (reference, st) =
                        keys.open(SealKind::Snapshot, &env).map_err(|e| PersistError::Seal(key.clone(), e))?;
                    if reference != Some(cp) {
                        return Err(bad());
                    }
                    let state = State::decode(&st).map_err(|_| bad())?;
                    base_rec = Some((cp, keep, state));
                }
                RecordKey::Op(id) => {
                    let a = parse_value(&value, 3).map_err(|_| bad())?;
                    let (eid, entry) = IndexEntry::from_cbor(&a[1]).map_err(|_| bad())?;
                    let body = match opt_bytes(&a[2]).map_err(|_| bad())? {
                        Some(_) => Some(open(&a[2])?),
                        None => None,
                    };
                    if eid != id || body.as_ref().is_some_and(|b| crate::sha256(b) != id) {
                        return Err(bad());
                    }
                    ops.push((entry.hlc, id, entry, body));
                }
                RecordKey::Pending(_) | RecordKey::Quarantined(_) | RecordKey::Held(_) => {
                    let a = parse_value(&value, 2).map_err(|_| bad())?;
                    parked.push((k, open(&a[1])?));
                }
                RecordKey::Rejected(id) => {
                    let a = parse_value(&value, 2).map_err(|_| bad())?;
                    let code = text(&a[1]).map_err(|_| bad())?;
                    rejected.insert(id, Reject::from_code(&code).ok_or_else(bad)?);
                }
            }
        }
        let mut r = match device {
            Some((root, signer)) if !wiped => {
                if root.enroll_public(&app) != household || Some(signer.device()) != me {
                    return Err(PersistError::WrongDevice);
                }
                Self::new(&app, root, signer, config)
            }
            _ => {
                let mut r = Self::observer(&app, household, config);
                r.me = me;
                r.wiped = wiped;
                r
            }
        };
        r.schema_fp = schema_fp;
        r.max_hlc = top;
        r.rejected = rejected;
        r.journal = Journal::default();

        // Ops in clock order, parents first. One whose parent's record is missing
        // (a crash between records) is parked as pending and re-admitted below.
        ops.sort_by_key(|a| (a.0, a.1));
        let mut repark: Vec<Vec<u8>> = Vec::new();
        for (_, id, entry, body) in ops {
            if entry.parents.iter().all(|p| r.store.contains(p)) {
                r.max_hlc = r.max_hlc.max(entry.hlc);
                r.store.insert(id, entry, body);
            } else if let Some(b) = body {
                r.journal.mark(RecordKey::Op(id));
                repark.push(b);
            }
        }
        r.enrolls = EnrollCache::rebuild(&r.store);
        if let Some((cp, keep, state)) = base_rec {
            if !r.store.contains(&cp) {
                return Err(PersistError::MissingBase);
            }
            let covered = closure_in(&r.store, &[cp]);
            // A prune cut short: drop the bodies the base does not keep.
            for id in &covered {
                if *id != cp && !keep.contains(id) && r.store.body(id).is_some() {
                    r.store.prune_body(id);
                    r.journal.mark(RecordKey::Op(*id));
                }
            }
            r.base = Some(Base { checkpoint: cp, covered, keep, state });
        }
        r.refold();

        for (k, bytes) in parked {
            let Ok(o) = op::decode(&bytes) else { continue };
            let id = crate::sha256(&bytes);
            match k {
                RecordKey::Quarantined(_) => {
                    r.quarantined.insert(id, (o, bytes));
                }
                RecordKey::Held(_) => {
                    r.held.insert(id, (o, bytes));
                }
                _ if r.store.contains(&id) => {
                    // Delivered already: a crash cut the changeset before the
                    // pending record's removal.
                    r.journal.mark(RecordKey::Pending(id));
                }
                _ if o.parents.iter().all(|p| r.store.contains(p)) => {
                    // Its parents arrived but it never delivered (a torn write):
                    // admit it again.
                    r.journal.mark(RecordKey::Pending(id));
                    repark.push(bytes);
                }
                _ => {
                    for p in o.parents.iter().filter(|p| !r.store.contains(p)) {
                        r.waiting.entry(*p).or_default().insert(id);
                    }
                    r.pending.insert(id, (o, bytes));
                }
            }
        }
        let rep = r.ingest(repark, now);
        Ok((r, rep))
    }
}

type Meta = (String, [u8; 32], Option<DeviceId>, bool, Option<[u8; 32]>, Hlc);

fn parse_meta(records: &[(Vec<u8>, Vec<u8>)]) -> Result<Option<Meta>, PersistError> {
    let Some((k, v)) = records.iter().find(|(k, _)| k.as_slice() == [TAG_META]) else { return Ok(None) };
    let parse = || -> R<Meta> {
        let a = parse_value(v, 7)?;
        let clock = arr(&a[6])?;
        let n = |c: &CBOR| -> R<u64> { c.clone().try_into().map_err(|_| ()) };
        if clock.len() != 2 {
            return Err(());
        }
        let top = Hlc::new(n(&clock[0])?, u32::try_from(n(&clock[1])?).map_err(|_| ())?);
        Ok((text(&a[1])?, b32(&a[2])?, opt32(&a[3])?, boolean(&a[4])?, opt32(&a[5])?, top))
    };
    parse().map(Some).map_err(|_| PersistError::Malformed(k.clone()))
}

/// The device a set of stored records belongs to, and whether it was wiped: what
/// the app needs to know before it fetches (or finds it has deleted) the keys.
pub fn stored_device(records: &[(Vec<u8>, Vec<u8>)]) -> Option<(Option<DeviceId>, bool)> {
    parse_meta(records).ok().flatten().map(|m| (m.2, m.3))
}

/// A stored op record as it is: id, index entry, sealed body (if not pruned).
pub type StoredOp = (Id, IndexEntry, Option<Vec<u8>>);

/// Every stored op record as it is, without opening anything: its id, index entry
/// (plain) and sealed body (`None` once pruned). What a wiped device, which has no
/// keys, can still forward (WipedPush after a restart, `api::sealed_handover`).
pub fn stored_sealed_ops(records: &[(Vec<u8>, Vec<u8>)]) -> Result<Vec<StoredOp>, PersistError> {
    let mut out = Vec::new();
    for (key, value) in records {
        let Some(RecordKey::Op(id)) = RecordKey::parse(key) else { continue };
        let bad = || PersistError::Malformed(key.clone());
        let a = parse_value(value, 3).map_err(|_| bad())?;
        let (eid, entry) = IndexEntry::from_cbor(&a[1]).map_err(|_| bad())?;
        if eid != id {
            return Err(bad());
        }
        out.push((id, entry, opt_bytes(&a[2]).map_err(|_| bad())?));
    }
    Ok(out)
}
