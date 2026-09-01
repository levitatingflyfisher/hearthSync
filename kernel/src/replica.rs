//! A replica: one device's view of one app's household log.
//!
//! **Delivery and validity** (design §3.1). An op is delivered once all its parents
//! are delivered, then validated. Rules by class (`byzantine-crdts`):
//!
//! | Rule | Check | Class | Outcome |
//! |---|---|---|---|
//! | V1 | size, canonical dCBOR, closed schema | the op alone | reject |
//! | V2 | strict Ed25519 under `device` | the op alone | reject |
//! | V3 | `hlc` > every parent's | before(u) | reject |
//! | V4 | `device` enrolled in before(u) (or u enrolls itself); enroll auth by this household | before(u) | reject |
//! | V5 | observed / restored / cut ids are in before(u) and act on the same target | before(u) | reject |
//! | V6 | forgotten device, u not in before(forget.cut), and no **shield** | the delivered Forget and Checkpoint sets | **exclude**: kept in the DAG, left out of the fold |
//! | V8 | `hlc.millis` > now + `MAX_FUTURE_SKEW_MS` | local time | **quarantine** until time catches up |
//!
//! | V7 | a declared name misused (wrong kind or type) / an undeclared name | the op and the app schema | reject / **hold** until a schema declares it |
//!
//! A rejected op's descendants are rejected too (`ParentRejected`). V7 applies only
//! when a schema is registered ([`Replica::set_schema`], ADR 0009).
//!
//! **Shield** (ADR 0006, Ruling 1 option A). A Forget F does not exclude u when some
//! delivered checkpoint C has u in before(C) and F outside it, unless C was itself
//! written by F's target outside the cut. "A Forget cannot reach behind a checkpoint"
//! is then a function of the delivered set, not of who happened to prune. Exclusion is
//! no longer monotone: delivering a checkpoint can bring an excluded op back.
//!
//! **KeepBodies** (ADR 0006, backing rule ADR 0007). Compaction at C folds into the
//! base only the ops some checkpoint in upto(C) by another device backs (has in its
//! past without excluding), and keeps every other body, folding it live on top of
//! the base state. A pruned base can always add an op later but never remove one, so
//! everything that might still be excluded stays a body.

use std::collections::{BTreeMap, BTreeSet};

use crate::keys::{self, DeviceSigner, HouseholdRoot};
use crate::op::{self, Body, Hlc, Kind, Op, Reject, Unsigned, Value};
use crate::persist::{Journal, RecordKey};
use crate::schema::{Check, Schema};
use crate::state::State;
use crate::store::{IndexEntry, MemStore, OpStore};
use crate::{sha256, DeviceId, Id, DEFAULT_HORIZON_MS, MAX_FUTURE_SKEW_MS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Op bodies behind a checkpoint older than this may be pruned. `None` keeps
    /// full history (Reckon, journals: ruling Q3).
    pub horizon_ms: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Config { horizon_ms: Some(DEFAULT_HORIZON_MS) }
    }
}

impl Config {
    pub fn keep_full_history() -> Self {
        Config { horizon_ms: None }
    }
    pub fn horizon(ms: u64) -> Self {
        Config { horizon_ms: Some(ms) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelError {
    /// This replica holds no keys (an observer, or wiped).
    NoKeys,
    /// The local clock trails the newest delivered op by more than the skew bound:
    /// "your clock looks wrong" (design §3.2, Willow's creation-time guard).
    ClockBehind { now: u64, latest: u64 },
    /// The replica's own op failed validation (a kernel or caller bug).
    Rejected(Reject),
    /// A checkpoint's `state_hash` does not match the fold it claims to commit to.
    CheckpointMismatch,
    /// A snapshot failed verification.
    BadSnapshot(Reject),
    /// The op to author uses a name the registered schema does not declare (V7
    /// would hold it); a misused declared name is `Rejected(SchemaViolation)`.
    Undeclared,
    /// The requester lacks a checkpoint past the horizon that the provider holds, and
    /// the provider can build no snapshot at all. Since the Fallback (ADR 0007) a
    /// provider with a base always can, and one without a base can build one at
    /// every old checkpoint it holds, so this is defensive only.
    SnapshotUnavailable,
}

/// What one `ingest` call did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IngestReport {
    pub delivered: Vec<Id>,
    /// Delivered but left out of the fold (V6), including ops newly excluded by a Forget.
    pub excluded: Vec<Id>,
    pub quarantined: Vec<Id>,
    /// Held by V7: they use a name the app schema does not declare (yet).
    pub held: Vec<Id>,
    /// Waiting for parents.
    pub pending: Vec<Id>,
    pub rejected: Vec<(Id, Reject)>,
    pub duplicates: usize,
    /// This replica was forgotten by the ingested ops and has wiped its keys.
    pub wiped: bool,
}

struct Keys {
    signer: Box<dyn DeviceSigner>,
    root: HouseholdRoot,
}

/// The pruned part of the log: a checkpoint, everything in before(it), and the
/// folded state there, less the kept bodies.
#[derive(Clone, Debug)]
pub(crate) struct Base {
    pub checkpoint: Id,
    /// The checkpoint and before(it).
    pub covered: BTreeSet<Id>,
    /// Covered ops whose bodies are kept and folded live (KeepBodies): every op no
    /// checkpoint by another device backs (see [`split_at`]).
    pub keep: BTreeSet<Id>,
    /// The fold of `covered - keep`.
    pub state: State,
}

/// V6 with the shield, over one delivered set.
pub(crate) struct Exclusion {
    /// (Forget id, target, closure of its cut)
    forgets: Vec<(Id, DeviceId, BTreeSet<Id>)>,
    /// (Checkpoint id, author, closure of the checkpoint itself)
    checkpoints: Vec<(Id, DeviceId, BTreeSet<Id>)>,
}

impl Exclusion {
    /// `forgets` is every Forget known (base state and live bodies); `within`, if
    /// given, restricts the delivered set (fold at a checkpoint).
    pub(crate) fn new<S: OpStore>(
        store: &S,
        forgets: &BTreeMap<Id, (DeviceId, Vec<Id>)>,
        within: Option<&BTreeSet<Id>>,
    ) -> Self {
        let inside = |id: &Id| within.is_none_or(|w| w.contains(id));
        let forgets: Vec<_> =
            forgets.iter().filter(|(f, _)| inside(f)).map(|(f, (d, cut))| (*f, *d, closure_in(store, cut))).collect();
        // Shields matter only when something could be cut out.
        let checkpoints = if forgets.is_empty() {
            Vec::new()
        } else {
            store
                .ids()
                .into_iter()
                .filter(|id| inside(id))
                .filter_map(|id| {
                    let e = store.entry(&id)?;
                    (e.kind == Kind::Checkpoint).then(|| (id, e.device, closure_in(store, &[id])))
                })
                .collect()
        };
        Exclusion { forgets, checkpoints }
    }

    fn cut_out(f: &(Id, DeviceId, BTreeSet<Id>), device: &DeviceId, id: &Id) -> bool {
        f.1 == *device && !f.2.contains(id)
    }

    /// V6 with the shield. Enroll and Forget are never excluded.
    pub(crate) fn excluded(&self, id: &Id, device: &DeviceId, kind: Kind) -> bool {
        if matches!(kind, Kind::Enroll | Kind::Forget) {
            return false;
        }
        self.forgets.iter().any(|f| Self::cut_out(f, device, id) && !self.shielded(f, id))
    }

    /// A checkpoint that has u in its past and lacks F shields u from F, unless F's
    /// target wrote it outside the cut.
    fn shielded(&self, f: &(Id, DeviceId, BTreeSet<Id>), id: &Id) -> bool {
        self.checkpoints
            .iter()
            .any(|(c, author, cl)| c != id && cl.contains(id) && !cl.contains(&f.0) && !Self::cut_out(f, author, c))
    }
}

/// The base/kept split at a checkpoint by `author` whose past is `past` (KeepBodies
/// with the backing rule, ADR 0007). An op is folded into the base iff it is
/// **backed**: some checkpoint in upto(C), by a device other than the op's author,
/// has the op in its past and did not exclude it there. Such a checkpoint shields the
/// op from every later Forget, so no replica can ever exclude it. Everything else
/// (excluded ops, and ops only their own author's checkpoints cover) stays a body.
/// Checkpoints themselves are never kept. A function of before(C) alone, which is
/// what lets the checkpoint commit to it.
pub(crate) struct Split {
    pub base_state: State,
    pub keep: BTreeSet<Id>,
    /// The fold at C: base state plus the kept ops, with V6 and the shield.
    pub full: State,
}

pub(crate) fn split_at<S: OpStore>(store: &S, base: Option<&Base>, past: &BTreeSet<Id>, author: DeviceId) -> Split {
    let at = fold(store, base, Some(past));
    let checkpoints: Vec<(Id, DeviceId, BTreeSet<Id>)> = past
        .iter()
        .filter_map(|id| {
            let e = store.entry(id)?;
            (e.kind == Kind::Checkpoint).then(|| (*id, e.device, closure_in(store, &[*id])))
        })
        .collect();
    let mut excl_at: BTreeMap<Id, Exclusion> = BTreeMap::new();
    let mut base_state = base.map(|b| b.state.clone()).unwrap_or_default();
    let mut keep = BTreeSet::new();
    for (id, op) in &at.live {
        let kind = op.body.kind();
        // A checkpoint adds nothing to the state, and V6 and the shield read only its
        // index entry, so its body is never needed below a base. Leaving checkpoints
        // out of the split keeps it the same whether they are bodies or already
        // behind an older base.
        if kind == Kind::Checkpoint {
            continue;
        }
        // C itself backs what others wrote, unless it excluded it.
        let mut backed = op.device != author && !at.excluded.contains(id);
        for (c, dev, cl) in &checkpoints {
            if backed {
                break;
            }
            if *dev == op.device || c == id || !cl.contains(id) {
                continue;
            }
            let ex = excl_at.entry(*c).or_insert_with(|| Exclusion::new(store, &at.state.forgets, Some(cl)));
            backed = !ex.excluded(id, &op.device, kind);
        }
        if backed {
            debug_assert!(!at.excluded.contains(id), "a backed op is never excluded");
            base_state.apply(*id, op);
        } else {
            keep.insert(*id);
        }
    }
    Split { base_state, keep, full: at.state }
}

/// before(u) for the given parents: every op reachable through parent hashes, the
/// starting ids included.
pub(crate) fn closure_in<S: OpStore>(store: &S, start: &[Id]) -> BTreeSet<Id> {
    let mut seen = BTreeSet::new();
    let mut stack: Vec<Id> = start.to_vec();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if let Some(e) = store.entry(&id) {
            stack.extend(e.parents.iter().copied());
        }
    }
    seen
}

/// Whether every id in `targets` is in before(`parents`): a walk back from the parents
/// that never goes below the oldest target's clock. V3 makes clocks strictly increase
/// from parent to child (snapshot indexes are checked for it too), so every path from
/// the parents down to a target stays at or above that target's clock, and the walk
/// is exact while touching only the part of the past newer than the oldest target.
pub(crate) fn all_in_past<S: OpStore>(store: &S, parents: &[Id], targets: &[Id]) -> bool {
    let mut want: BTreeSet<Id> = targets.iter().copied().collect();
    let Some(floor) = targets.iter().map(|t| store.entry(t).map(|e| e.hlc)).min().flatten() else {
        return want.is_empty();
    };
    if targets.iter().any(|t| !store.contains(t)) {
        return false;
    }
    let mut seen = BTreeSet::new();
    let mut stack: Vec<Id> = parents.to_vec();
    while let Some(id) = stack.pop() {
        if want.is_empty() {
            break;
        }
        if !seen.insert(id) {
            continue;
        }
        want.remove(&id);
        if let Some(e) = store.entry(&id) {
            stack.extend(e.parents.iter().filter(|p| store.entry(p).is_some_and(|pe| pe.hlc >= floor)));
        }
    }
    want.is_empty()
}

/// The devices enrolled in before(u), for every delivered op u (u included), interned:
/// a household has a handful of devices, so there are few distinct sets. V4 reads it
/// from the op's parents instead of walking before(u). Derived from the index alone,
/// so it is rebuilt, never stored.
#[derive(Clone, Debug, Default)]
pub(crate) struct EnrollCache {
    sets: Vec<BTreeSet<DeviceId>>,
    intern: BTreeMap<BTreeSet<DeviceId>, u32>,
    at: BTreeMap<Id, u32>,
}

impl EnrollCache {
    /// Record a delivered op; its parents must already be recorded.
    pub(crate) fn note(&mut self, id: Id, e: &IndexEntry) {
        let mut set: BTreeSet<DeviceId> = e.enrolls.into_iter().collect();
        for p in &e.parents {
            if let Some(&i) = self.at.get(p) {
                set.extend(self.sets[i as usize].iter().copied());
            }
        }
        let i = match self.intern.get(&set) {
            Some(i) => *i,
            None => {
                let i = u32::try_from(self.sets.len()).expect("fewer than 2^32 distinct device sets");
                self.sets.push(set.clone());
                self.intern.insert(set, i);
                i
            }
        };
        self.at.insert(id, i);
    }

    /// V4: `device` has an Enroll in before(an op with these parents).
    pub(crate) fn enrolled_before(&self, parents: &[Id], device: &DeviceId) -> bool {
        parents.iter().any(|p| self.at.get(p).is_some_and(|&i| self.sets[i as usize].contains(device)))
    }

    /// The cache for a whole store, in clock order (parents first, by V3).
    pub(crate) fn rebuild<S: OpStore>(store: &S) -> Self {
        let mut v: Vec<(Hlc, Id)> =
            store.ids().into_iter().filter_map(|id| store.entry(&id).map(|e| (e.hlc, id))).collect();
        v.sort();
        let mut c = EnrollCache::default();
        for (_, id) in v {
            c.note(id, store.entry(&id).expect("indexed"));
        }
        c
    }
}

/// The one fold: the base state plus every live body (above the base, or kept), with
/// V6 and the shield over the delivered set (or over `within`, for the fold at a
/// checkpoint). Refold, compaction, snapshots and snapshot verification all use it.
pub(crate) struct Folded {
    pub state: State,
    pub excluded: BTreeSet<Id>,
    /// The live ops considered, in id order.
    pub live: Vec<(Id, Op)>,
}

pub(crate) fn fold<S: OpStore>(store: &S, base: Option<&Base>, within: Option<&BTreeSet<Id>>) -> Folded {
    let mut state = base.map(|b| b.state.clone()).unwrap_or_default();
    let mut live = Vec::new();
    for id in store.ids() {
        if within.is_some_and(|w| !w.contains(&id)) {
            continue;
        }
        if base.is_some_and(|b| b.covered.contains(&id) && !b.keep.contains(&id)) {
            continue;
        }
        let Some(body) = store.body(&id) else { continue };
        live.push((id, op::decode(body).expect("stored ops decode")));
    }
    let mut forgets = state.forgets.clone();
    for (id, op) in &live {
        if let Body::Forget { device, cut, .. } = &op.body {
            forgets.insert(*id, (*device, cut.clone()));
        }
    }
    let excl = Exclusion::new(store, &forgets, within);
    let mut excluded = BTreeSet::new();
    for (id, op) in &live {
        if excl.excluded(id, &op.device, op.body.kind()) {
            excluded.insert(*id);
        } else {
            state.apply(*id, op);
        }
    }
    Folded { state, excluded, live }
}

pub struct Replica<S: OpStore + Default = MemStore> {
    pub(crate) app: String,
    pub(crate) enroll_pk: [u8; 32],
    pub(crate) config: Config,
    pub(crate) store: S,
    pub(crate) pending: BTreeMap<Id, (Op, Vec<u8>)>,
    /// parent id -> pending children waiting on it
    pub(crate) waiting: BTreeMap<Id, BTreeSet<Id>>,
    pub(crate) quarantined: BTreeMap<Id, (Op, Vec<u8>)>,
    /// V7: ops using names the schema does not declare, retried on `set_schema`.
    pub(crate) held: BTreeMap<Id, (Op, Vec<u8>)>,
    schema: Option<Schema>,
    /// Fingerprint of the last schema registered (persisted, so a change shows).
    pub(crate) schema_fp: Option<[u8; 32]>,
    pub(crate) rejected: BTreeMap<Id, Reject>,
    excluded: BTreeSet<Id>,
    /// per Forget: (device, closure of its cut)
    cuts: Vec<(DeviceId, BTreeSet<Id>)>,
    pub(crate) state: State,
    pub(crate) base: Option<Base>,
    keys: Option<Keys>,
    pub(crate) me: Option<DeviceId>,
    pub(crate) wiped: bool,
    pub(crate) max_hlc: Hlc,
    /// Devices enrolled in before(u) for every delivered op (V4 without a walk).
    pub(crate) enrolls: EnrollCache,
    /// Which persisted records changed since the last `take_changes` (ADR 0010).
    pub(crate) journal: Journal,
    /// Bumped whenever the whole state may have changed at once (refold, install),
    /// so a view can tell an incremental change from a wholesale one.
    pub(crate) fold_gen: u64,
    /// Ids of ops this replica authored since the last `take_authored`.
    pub(crate) authored: Vec<Id>,
    /// Ids of ops delivered since the last `take_delivered`, authored or not.
    pub(crate) delivered_log: Vec<Id>,
}

impl<S: OpStore + Default> Replica<S> {
    /// A device replica. `root` is the household seed every device stores (Q1);
    /// `signer` is this device's own key.
    pub fn new(app: &str, root: HouseholdRoot, signer: Box<dyn DeviceSigner>, config: Config) -> Self {
        let enroll_pk = root.enroll_public(app);
        let me = signer.device();
        let mut r = Self::observer(app, enroll_pk, config);
        r.me = Some(me);
        r.keys = Some(Keys { signer, root });
        r
    }

    /// A keyless replica that validates and folds but cannot author (tests, relays,
    /// a future read-only viewer). Panics on an invalid app domain.
    pub fn observer(app: &str, enroll_pk: [u8; 32], config: Config) -> Self {
        assert!(keys::app_domain_ok(app), "invalid app domain {app:?}");
        Replica {
            app: app.to_string(),
            enroll_pk,
            config,
            store: S::default(),
            pending: BTreeMap::new(),
            waiting: BTreeMap::new(),
            quarantined: BTreeMap::new(),
            held: BTreeMap::new(),
            schema: None,
            schema_fp: None,
            rejected: BTreeMap::new(),
            excluded: BTreeSet::new(),
            cuts: Vec::new(),
            state: State::default(),
            base: None,
            keys: None,
            me: None,
            wiped: false,
            max_hlc: Hlc::default(),
            enrolls: EnrollCache::default(),
            journal: Journal::fresh(),
            fold_gen: 0,
            authored: Vec::new(),
            delivered_log: Vec::new(),
        }
    }

    // ------------------------------------------------------------ accessors

    pub fn app(&self) -> &str {
        &self.app
    }
    pub fn device(&self) -> Option<DeviceId> {
        self.me
    }
    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn heads(&self) -> Vec<Id> {
        self.store.heads()
    }
    pub fn store(&self) -> &S {
        &self.store
    }
    pub fn has_keys(&self) -> bool {
        self.keys.is_some()
    }
    /// True once this device has been forgotten and wiped its keys.
    pub fn is_wiped(&self) -> bool {
        self.wiped
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn quarantined_count(&self) -> usize {
        self.quarantined.len()
    }
    pub fn held_count(&self) -> usize {
        self.held.len()
    }
    /// Ids waiting for parents, quarantined (V8) and held (V7).
    pub fn pending_ids(&self) -> Vec<Id> {
        self.pending.keys().copied().collect()
    }
    pub fn quarantined_ids(&self) -> Vec<Id> {
        self.quarantined.keys().copied().collect()
    }
    pub fn held_ids(&self) -> Vec<Id> {
        self.held.keys().copied().collect()
    }
    /// Signed bytes of the ops held back from delivery: pending, quarantined, held.
    pub fn undelivered(&self) -> Vec<Vec<u8>> {
        let all = self.pending.values().chain(self.quarantined.values()).chain(self.held.values());
        all.map(|(_, b)| b.clone()).collect()
    }
    /// Counts refolds and snapshot installs: when it moves, any part of the state
    /// may have changed, not only what the delivered ops touched.
    pub fn fold_generation(&self) -> u64 {
        self.fold_gen
    }
    /// Ids of the ops this replica authored since the last call, oldest first: the
    /// ones to hand the relay.
    pub fn take_authored(&mut self) -> Vec<Id> {
        std::mem::take(&mut self.authored)
    }
    /// Ids of every op delivered since the last call (own or not), in delivery order.
    pub fn take_delivered(&mut self) -> Vec<Id> {
        std::mem::take(&mut self.delivered_log)
    }

    /// A copy of this replica that signs with `signer` (same device). The api runs
    /// flows that author several ops on a copy, so a flow waiting for a signature
    /// leaves the replica untouched.
    pub fn clone_with_signer(&self, signer: Box<dyn DeviceSigner>) -> Self
    where
        S: Clone,
    {
        assert_eq!(Some(signer.device()), self.me, "the same device");
        Replica {
            app: self.app.clone(),
            enroll_pk: self.enroll_pk,
            config: self.config,
            store: self.store.clone(),
            pending: self.pending.clone(),
            waiting: self.waiting.clone(),
            quarantined: self.quarantined.clone(),
            held: self.held.clone(),
            schema: self.schema.clone(),
            schema_fp: self.schema_fp,
            rejected: self.rejected.clone(),
            excluded: self.excluded.clone(),
            cuts: self.cuts.clone(),
            state: self.state.clone(),
            base: self.base.clone(),
            keys: self.keys.as_ref().map(|k| Keys { signer, root: k.root.clone() }),
            me: self.me,
            wiped: self.wiped,
            max_hlc: self.max_hlc,
            enrolls: self.enrolls.clone(),
            journal: self.journal.clone(),
            fold_gen: self.fold_gen,
            authored: self.authored.clone(),
            delivered_log: self.delivered_log.clone(),
        }
    }
    pub fn schema(&self) -> Option<&Schema> {
        self.schema.as_ref()
    }

    /// Register the app schema (V7) and retry every held op against it: a newer app
    /// version may declare the names they use. The caller validates the schema.
    pub fn set_schema(&mut self, schema: Schema, now: u64) -> IngestReport {
        self.schema_fp = Some(schema.fingerprint());
        self.schema = Some(schema);
        self.journal.mark(RecordKey::Meta);
        let mut rep = IngestReport::default();
        for (id, (op, bytes)) in std::mem::take(&mut self.held) {
            self.journal.mark(RecordKey::Held(id));
            self.admit(id, op, bytes, now, &mut rep);
        }
        rep
    }
    pub fn rejected(&self) -> &BTreeMap<Id, Reject> {
        &self.rejected
    }
    pub fn excluded(&self) -> &BTreeSet<Id> {
        &self.excluded
    }
    pub fn base_checkpoint(&self) -> Option<Id> {
        self.base.as_ref().map(|b| b.checkpoint)
    }
    /// Signed bytes of every delivered op this replica still holds a body for,
    /// in clock order (a topological order, by V3).
    pub fn log(&self) -> Vec<Vec<u8>> {
        self.ids_in_clock_order(self.store.ids())
            .into_iter()
            .filter_map(|id| self.store.body(&id).map(<[u8]>::to_vec))
            .collect()
    }

    pub(crate) fn ids_in_clock_order(&self, ids: impl IntoIterator<Item = Id>) -> Vec<Id> {
        let mut v: Vec<(Hlc, Id)> =
            ids.into_iter().filter_map(|id| self.store.entry(&id).map(|e| (e.hlc, id))).collect();
        v.sort();
        v.into_iter().map(|(_, id)| id).collect()
    }

    /// before(u) for the given parents: every op reachable through parent hashes,
    /// the starting ids included.
    pub(crate) fn closure(&self, start: &[Id]) -> BTreeSet<Id> {
        closure_in(&self.store, start)
    }

    // ------------------------------------------------------------ ingest

    /// Ingest signed ops in any order, with duplicates. Quarantined ops are retried
    /// first against the new `now`.
    pub fn ingest<B: AsRef<[u8]>>(&mut self, ops: impl IntoIterator<Item = B>, now: u64) -> IngestReport {
        let mut rep = IngestReport::default();
        for (id, (op, bytes)) in std::mem::take(&mut self.quarantined) {
            self.journal.mark(RecordKey::Quarantined(id));
            self.admit(id, op, bytes, now, &mut rep);
        }
        for bytes in ops {
            self.ingest_one(bytes.as_ref(), now, &mut rep);
        }
        rep
    }

    fn ingest_one(&mut self, bytes: &[u8], now: u64, rep: &mut IngestReport) {
        let id = sha256(bytes);
        if self.store.contains(&id)
            || self.pending.contains_key(&id)
            || self.quarantined.contains_key(&id)
            || self.held.contains_key(&id)
            || self.rejected.contains_key(&id)
        {
            rep.duplicates += 1;
            return;
        }
        match Op::decode_verified(bytes) {
            Err(r) => self.reject(id, r, rep),
            Ok((op, _)) if op.app != self.app => self.reject(id, Reject::WrongApp, rep),
            Ok((op, _)) => self.admit(id, op, bytes.to_vec(), now, rep),
        }
    }

    fn admit(&mut self, id: Id, op: Op, bytes: Vec<u8>, now: u64, rep: &mut IngestReport) {
        // Worklist instead of recursion: delivering one op can release a long chain.
        let mut work = vec![(id, op, bytes)];
        while let Some((id, op, bytes)) = work.pop() {
            if op.hlc.millis > now.saturating_add(MAX_FUTURE_SKEW_MS) {
                self.journal.mark(RecordKey::Quarantined(id));
                self.quarantined.insert(id, (op, bytes));
                rep.quarantined.push(id);
                continue;
            }
            if op.parents.iter().any(|p| self.rejected.contains_key(p)) {
                self.reject(id, Reject::ParentRejected, rep);
                continue;
            }
            let missing: Vec<Id> = op.parents.iter().filter(|p| !self.store.contains(p)).copied().collect();
            if !missing.is_empty() {
                for p in missing {
                    self.waiting.entry(p).or_default().insert(id);
                }
                self.journal.mark(RecordKey::Pending(id));
                self.pending.insert(id, (op, bytes));
                rep.pending.push(id);
                continue;
            }
            if let Err(r) = self.validate(&op) {
                self.reject(id, r, rep);
                continue;
            }
            // V7, after V1–V5: a misused declared name is invalid everywhere; an
            // undeclared one waits for an app version that declares it.
            match self.schema.as_ref().map_or(Check::Ok, |s| s.check(&op.body)) {
                Check::Violation => {
                    self.reject(id, Reject::SchemaViolation, rep);
                    continue;
                }
                Check::Unknown => {
                    self.journal.mark(RecordKey::Held(id));
                    self.held.insert(id, (op, bytes));
                    rep.held.push(id);
                    continue;
                }
                Check::Ok => {}
            }
            self.deliver(id, &op, Some(bytes), rep);
            for child in self.waiting.remove(&id).unwrap_or_default() {
                let ready =
                    self.pending.get(&child).is_some_and(|(c, _)| c.parents.iter().all(|p| self.store.contains(p)));
                if ready {
                    let (c, b) = self.pending.remove(&child).expect("pending child");
                    self.journal.mark(RecordKey::Pending(child));
                    work.push((child, c, b));
                }
            }
        }
    }

    fn reject(&mut self, id: Id, r: Reject, rep: &mut IngestReport) {
        let mut work = vec![(id, r)];
        while let Some((id, r)) = work.pop() {
            if self.rejected.insert(id, r).is_some() {
                continue;
            }
            self.journal.mark(RecordKey::Rejected(id));
            rep.rejected.push((id, r));
            for child in self.waiting.remove(&id).unwrap_or_default() {
                if self.pending.remove(&child).is_some() {
                    self.journal.mark(RecordKey::Pending(child));
                    work.push((child, Reject::ParentRejected));
                }
            }
        }
    }

    fn validate(&self, op: &Op) -> Result<(), Reject> {
        // V3
        for p in &op.parents {
            let e = self.store.entry(p).expect("parents delivered");
            if e.hlc >= op.hlc {
                return Err(Reject::ClockNotAfterParents);
            }
        }
        // V4: enroll / forget authority is the household enroll key.
        let auth_ok = match &op.body {
            Body::Enroll { device, label, auth } => {
                op::verify_strict(&self.enroll_pk, &keys::enroll_auth_msg(&self.app, device, label), auth)
            }
            Body::Forget { device, cut, auth } => {
                op::verify_strict(&self.enroll_pk, &keys::forget_auth_msg(&self.app, device, cut), auth)
            }
            _ => true,
        };
        if !auth_ok {
            return Err(Reject::BadEnrollAuth);
        }
        // V4, from the enrolment cache instead of a walk over before(u).
        let self_enroll = matches!(&op.body, Body::Enroll { device, .. } if *device == op.device);
        let enrolled = self.enrolls.enrolled_before(&op.parents, &op.device);
        // Differential oracle (debug builds): the v0 walk over all of before(u).
        #[cfg(debug_assertions)]
        {
            let walked = self
                .closure(&op.parents)
                .iter()
                .any(|id| self.store.entry(id).is_some_and(|e| e.enrolls == Some(op.device)));
            assert_eq!(enrolled, walked, "V4 cache disagrees with the walk over before(u)");
        }
        if !self_enroll && !enrolled {
            return Err(Reject::NotEnrolled);
        }
        // V5
        let check = |ids: &[Id], ok: &dyn Fn(&IndexEntry) -> bool| -> Result<(), Reject> {
            let good =
                ids.iter().all(|id| self.store.entry(id).is_some_and(ok)) && all_in_past(&self.store, &op.parents, ids);
            #[cfg(debug_assertions)]
            {
                let past = self.closure(&op.parents);
                let walked = ids.iter().all(|id| past.contains(id) && self.store.entry(id).is_some_and(ok));
                assert_eq!(good, walked, "V5's bounded walk disagrees with the walk over before(u)");
            }
            if good {
                Ok(())
            } else {
                Err(Reject::ObservedNotInPast)
            }
        };
        let target = op.body.target();
        match &op.body {
            Body::Delete { observed, .. } => check(observed, &|e| e.kind == Kind::Put && e.target == target)?,
            Body::Restore { observed, .. } => check(observed, &|e| e.kind == Kind::Delete && e.target == target)?,
            Body::SetRemove { observed, .. } => check(observed, &|e| e.kind == Kind::SetAdd && e.target == target)?,
            Body::Forget { device, cut, .. } => check(cut, &|e| e.device == *device)?,
            _ => {}
        }
        Ok(())
    }

    fn deliver(&mut self, id: Id, op: &Op, bytes: Option<Vec<u8>>, rep: &mut IngestReport) {
        let entry = IndexEntry::of(op);
        self.enrolls.note(id, &entry);
        self.store.insert(id, entry, bytes);
        self.journal.mark(RecordKey::Op(id));
        self.delivered_log.push(id);
        if op.hlc > self.max_hlc {
            // The creation-time guard's high-water mark outlives the ops that set it
            // (an adoption may drop them), so it is persisted with the meta record.
            self.max_hlc = op.hlc;
            self.journal.mark(RecordKey::Meta);
        }
        rep.delivered.push(id);
        match &op.body {
            Body::Forget { device, .. } => {
                self.state.apply(id, op);
                let before = self.excluded.clone();
                self.refold();
                rep.excluded.extend(self.excluded.difference(&before).copied());
                if Some(*device) == self.me && !self.wiped {
                    self.wipe();
                    rep.wiped = true;
                }
            }
            // A checkpoint can shield its past from Forgets it lacks, which may bring
            // excluded ops back (and it may itself be cut out).
            Body::Checkpoint { .. } if !self.state.forgets.is_empty() => {
                let before = self.excluded.clone();
                self.refold();
                rep.excluded.extend(self.excluded.difference(&before).copied());
            }
            _ => {
                if self.is_excluded(op.device, op.body.kind(), &id) {
                    self.excluded.insert(id);
                    rep.excluded.push(id);
                } else {
                    self.state.apply(id, op);
                }
            }
        }
    }

    /// V6 for a newly delivered op. Nothing delivered has it in its past yet, so no
    /// checkpoint can shield it. Enroll and Forget are never excluded: their authority
    /// is the enroll key, not the device, and excluding them would make validity
    /// non-monotonic (ADR 0002).
    fn is_excluded(&self, device: DeviceId, kind: Kind, id: &Id) -> bool {
        if matches!(kind, Kind::Enroll | Kind::Forget) {
            return false;
        }
        self.cuts.iter().any(|(d, closure)| *d == device && !closure.contains(id))
    }

    /// Plain V6, ignoring shields: some delivered Forget cuts this op out. An op like
    /// that is never a base (compaction, snapshots).
    pub(crate) fn cut_out(&self, id: &Id) -> bool {
        self.store.entry(id).is_some_and(|e| self.cuts.iter().any(|(d, c)| *d == e.device && !c.contains(id)))
    }

    /// Recompute the fold from the base plus every retained body. Called when a
    /// Forget arrives, since it can exclude ops already folded, and when a checkpoint
    /// arrives while a Forget is known, since it can shield them again.
    pub(crate) fn refold(&mut self) {
        let f = fold(&self.store, self.base.as_ref(), None);
        self.state = f.state;
        self.excluded = f.excluded;
        self.fold_gen += 1;
        // From the folded state: kept Forget bodies are not in the base state.
        self.cuts = self.state.forgets.values().map(|(d, cut)| (*d, self.closure(cut))).collect();
    }

    /// The fold recomputed from the base and every retained body, ignoring the
    /// incrementally maintained state. Tests use it to check that incremental
    /// application and a full refold agree.
    pub fn fold_from_scratch(&self) -> (State, BTreeSet<Id>) {
        let f = fold(&self.store, self.base.as_ref(), None);
        (f.state, f.excluded)
    }

    pub(crate) fn wipe(&mut self) {
        // Dropping the signer and root zeroises them (ed25519-dalek and Zeroizing).
        self.keys = None;
        self.wiped = true;
        self.journal.mark(RecordKey::Meta);
    }

    // ------------------------------------------------------------ authoring

    /// The clock for a new op: after everything delivered, and refusing to stamp
    /// when the local clock is far behind the log (design §3.2).
    pub fn next_hlc(&self, now: u64) -> Result<Hlc, KernelError> {
        let m = self.max_hlc;
        if now.saturating_add(MAX_FUTURE_SKEW_MS) < m.millis {
            return Err(KernelError::ClockBehind { now, latest: m.millis });
        }
        Ok(if now > m.millis {
            Hlc::new(now, 0)
        } else if m.counter == u32::MAX {
            Hlc::new(m.millis + 1, 0)
        } else {
            Hlc::new(m.millis, m.counter + 1)
        })
    }

    /// Step 1 of authoring: the unsigned op on the current heads. The platform signs
    /// `signable_bytes()`; step 2 is [`Replica::finish`].
    pub fn prepare(&self, body: Body, now: u64) -> Result<Unsigned, KernelError> {
        if self.wiped {
            return Err(KernelError::NoKeys);
        }
        let device = self.me.ok_or(KernelError::NoKeys)?;
        match self.schema.as_ref().map_or(Check::Ok, |s| s.check(&body)) {
            Check::Ok => {}
            Check::Unknown => return Err(KernelError::Undeclared),
            Check::Violation => return Err(KernelError::Rejected(Reject::SchemaViolation)),
        }
        Ok(Unsigned { app: self.app.clone(), device, parents: self.store.heads(), hlc: self.next_hlc(now)?, body })
    }

    /// Step 2: attach the signature and ingest locally. Returns the id and the bytes to send.
    pub fn finish(&mut self, unsigned: Unsigned, sig: [u8; 64], now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        if self.wiped {
            return Err(KernelError::NoKeys);
        }
        let bytes = unsigned.with_sig(sig).encode();
        let id = sha256(&bytes);
        let rep = self.ingest([&bytes], now);
        if let Some((_, r)) = rep.rejected.first() {
            return Err(KernelError::Rejected(*r));
        }
        self.authored.push(id);
        debug_assert!(rep.delivered.contains(&id), "own op must deliver: {rep:?}");
        Ok((id, bytes))
    }

    /// Prepare, sign with this device's key, finish.
    pub fn author(&mut self, body: Body, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        let u = self.prepare(body, now)?;
        let sig = self.keys.as_ref().ok_or(KernelError::NoKeys)?.signer.sign(&u.signable_bytes());
        self.finish(u, sig, now)
    }

    fn root(&self) -> Result<&HouseholdRoot, KernelError> {
        self.keys.as_ref().map(|k| &k.root).ok_or(KernelError::NoKeys)
    }

    /// The household's authorisation of `device` under `label`: the same bytes as in
    /// its Enroll op (Ed25519 is deterministic), for when that op's body is pruned.
    pub(crate) fn enroll_auth(&self, device: &DeviceId, label: &str) -> Result<[u8; 64], KernelError> {
        Ok(keys::sign_enroll(self.root()?, &self.app, device, label))
    }

    /// Enroll this device (the phrase is on every device, Q1, so it can sign for itself).
    pub fn enroll_self(&mut self, label: &str, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        let device = self.me.ok_or(KernelError::NoKeys)?;
        self.enroll(device, label, now)
    }

    /// Enroll another device (pairing): its public key and a label.
    pub fn enroll(&mut self, device: DeviceId, label: &str, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        let auth = keys::sign_enroll(self.root()?, &self.app, &device, label);
        self.author(Body::Enroll { device, label: label.to_string(), auth }, now)
    }

    pub fn put<'a>(
        &mut self,
        table: &str,
        row: &str,
        fields: impl IntoIterator<Item = (&'a str, Value)>,
        now: u64,
    ) -> Result<(Id, Vec<u8>), KernelError> {
        let fields = fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        self.author(Body::Put { table: table.into(), row: row.into(), fields, origin: None }, now)
    }

    /// Delete a row: observes every put to it delivered here (Decision 1: a delete
    /// is an op, never an absence).
    pub fn delete(&mut self, table: &str, row: &str, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        let observed = self
            .state
            .rows
            .get(&(table.to_string(), row.to_string()))
            .map(|r| r.puts.iter().copied().collect())
            .unwrap_or_default();
        self.author(Body::Delete { table: table.into(), row: row.into(), observed }, now)
    }

    /// Undo: restore every live delete of the row.
    pub fn restore(&mut self, table: &str, row: &str, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        let observed = self
            .state
            .rows
            .get(&(table.to_string(), row.to_string()))
            .map(|r| r.deletes.keys().filter(|d| !r.restored.contains(*d)).copied().collect())
            .unwrap_or_default();
        self.author(Body::Restore { table: table.into(), row: row.into(), observed }, now)
    }

    pub fn set_add(&mut self, set: &str, element: Value, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        self.author(Body::SetAdd { set: set.into(), element }, now)
    }

    pub fn set_remove(&mut self, set: &str, element: Value, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        let observed = self
            .state
            .sets
            .get(&(set.to_string(), element.clone()))
            .map(|e| e.adds.iter().filter(|a| !e.removed.contains(*a)).copied().collect())
            .unwrap_or_default();
        self.author(Body::SetRemove { set: set.into(), element, observed }, now)
    }

    pub fn append(&mut self, stream: &str, record: Value, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        self.author(Body::Append { stream: stream.into(), record }, now)
    }

    /// "Forget this device". The cut is `device`'s latest ops as seen here; its ops
    /// outside before(cut) leave the fold on every replica. Forgetting yourself
    /// wipes your keys as soon as the op is delivered locally, i.e. immediately.
    pub fn forget(&mut self, device: DeviceId, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        let theirs: Vec<Id> = self
            .store
            .ids()
            .into_iter()
            .filter(|id| self.store.entry(id).is_some_and(|e| e.device == device))
            .collect();
        let parents: Vec<Id> =
            theirs.iter().flat_map(|id| self.store.entry(id).map(|e| e.parents.clone()).unwrap_or_default()).collect();
        let covered = self.closure(&parents);
        let cut: Vec<Id> = theirs.into_iter().filter(|id| !covered.contains(id)).collect();
        let auth = keys::sign_forget(self.root()?, &self.app, &device, &cut);
        self.author(Body::Forget { device, cut, auth }, now)
    }

    pub fn forget_self(&mut self, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        let me = self.me.ok_or(KernelError::NoKeys)?;
        self.forget(me, now)
    }

    /// Commit to the base state, the index and the kept set a snapshot at this
    /// checkpoint will have (design §4; see [`checkpoint_hash`]).
    pub fn checkpoint(&mut self, now: u64) -> Result<(Id, Vec<u8>), KernelError> {
        let me = self.me.ok_or(KernelError::NoKeys)?;
        let past: BTreeSet<Id> = self.store.ids().into_iter().collect();
        let split = split_at(&self.store, self.base.as_ref(), &past, me);
        debug_assert_eq!(split.full, self.state, "the fold at the new checkpoint is the current state");
        let index = self.index_of(past);
        let state_hash = checkpoint_hash(&split.base_state.encode(), &index, &split.keep);
        self.author(Body::Checkpoint { state_hash }, now)
    }

    // ------------------------------------------------------------ horizon

    /// Whether checkpoint `id` is older than the horizon at `now`.
    pub(crate) fn is_old(&self, id: &Id, now: u64) -> bool {
        let Some(h) = self.config.horizon_ms else { return false };
        self.store.entry(id).is_some_and(|e| e.kind == Kind::Checkpoint && e.hlc.millis.saturating_add(h) <= now)
    }

    /// Checkpoints this replica can build a snapshot at: held with a body and
    /// descending from the current base (the base included).
    pub(crate) fn snapshot_candidates(&self) -> Vec<Id> {
        let base_cp = self.base.as_ref().map(|b| b.checkpoint);
        self.store
            .ids()
            .into_iter()
            .filter(|id| {
                self.store.entry(id).is_some_and(|e| e.kind == Kind::Checkpoint)
                    && self.store.body(id).is_some()
                    && base_cp.is_none_or(|b| b == *id || self.closure(&[*id]).contains(&b))
            })
            .collect()
    }

    /// Prune op bodies behind the newest checkpoint older than the horizon. Index
    /// entries stay, so before(u) is still computable, and so do the bodies
    /// KeepBodies names. Returns the new base checkpoint, or `None` if nothing is old
    /// enough (or full history is kept).
    pub fn compact(&mut self, now: u64) -> Result<Option<Id>, KernelError> {
        let base_cp = self.base.as_ref().map(|b| b.checkpoint);
        // A checkpoint some Forget cuts out is never a base.
        let best = self
            .snapshot_candidates()
            .into_iter()
            .filter(|id| Some(*id) != base_cp && self.is_old(id, now) && !self.cut_out(id))
            .map(|id| (self.store.entry(&id).expect("indexed").hlc, id))
            .max();
        let Some((_, cp)) = best else { return Ok(None) };
        let base = self.base_at(cp)?;
        for id in &base.covered {
            if *id != cp && !base.keep.contains(id) {
                self.store.prune_body(id);
                self.journal.mark(RecordKey::Op(*id));
            }
        }
        // An excluded op pruned under the base (only a checkpoint can be: excluded
        // ops keep their bodies) is no longer part of the fold, so it leaves the
        // excluded set too, as a refold, and a reload, would have it.
        self.excluded.retain(|id| !base.covered.contains(id) || base.keep.contains(id));
        self.base = Some(base);
        self.journal.mark(RecordKey::Base);
        debug_assert_eq!(
            self.fold_from_scratch(),
            (self.state.clone(), self.excluded.clone()),
            "compaction must not change the state"
        );
        Ok(Some(cp))
    }

    /// The base a compaction at `cp` would produce, checked against the checkpoint's
    /// hash. `cp` must be one of [`Replica::snapshot_candidates`].
    pub(crate) fn base_at(&self, cp: Id) -> Result<Base, KernelError> {
        let cp_op = op::decode(self.store.body(&cp).expect("candidate has a body")).expect("stored ops decode");
        let Body::Checkpoint { state_hash } = cp_op.body else { unreachable!("candidate is a checkpoint") };
        let mut down = self.closure(&[cp]);
        down.remove(&cp);
        let split = split_at(&self.store, self.base.as_ref(), &down, cp_op.device);
        let index = self.index_of(down.iter().copied());
        if checkpoint_hash(&split.base_state.encode(), &index, &split.keep) != state_hash {
            return Err(KernelError::CheckpointMismatch);
        }
        down.insert(cp);
        Ok(Base { checkpoint: cp, covered: down, keep: split.keep, state: split.base_state })
    }

    // ------------------------------------------------------------ used by sync

    pub(crate) fn body_of(&self, id: &Id) -> Option<Op> {
        self.store.body(id).map(|b| op::decode(b).expect("stored ops decode"))
    }

    /// Replace the log with a snapshot base (sync's past-horizon path).
    /// Index entries for `ids`, in clock order: the form a checkpoint commits to.
    pub(crate) fn index_of(&self, ids: impl IntoIterator<Item = Id>) -> Vec<(Id, IndexEntry)> {
        self.ids_in_clock_order(ids)
            .into_iter()
            .map(|id| (id, self.store.entry(&id).expect("indexed").clone()))
            .collect()
    }

    /// Forget pending ops (and whatever waits on them): used when adoption rebases an
    /// op that the offer listed but could not deliver.
    pub(crate) fn drop_pending(&mut self, ids: impl IntoIterator<Item = Id>) {
        let mut work: Vec<Id> = ids.into_iter().collect();
        while let Some(id) = work.pop() {
            if self.pending.remove(&id).is_some() {
                self.journal.mark(RecordKey::Pending(id));
                work.extend(self.waiting.remove(&id).unwrap_or_default());
            }
        }
        for children in self.waiting.values_mut() {
            children.retain(|c| self.pending.contains_key(c));
        }
        self.waiting.retain(|_, c| !c.is_empty());
    }

    /// Replace the log with a verified snapshot base (sync's past-horizon path).
    /// Returns whether this reset wiped the replica (it was forgotten in the base).
    pub(crate) fn install(&mut self, v: Verified<S>) -> bool {
        for id in v.store.ids() {
            self.max_hlc = self.max_hlc.max(v.store.entry(&id).expect("indexed").hlc);
        }
        self.store = v.store;
        self.enrolls = EnrollCache::rebuild(&self.store);
        // The whole log is replaced: the app rewrites every record in one transaction.
        self.journal.reset();
        self.pending.clear();
        self.waiting.clear();
        self.quarantined.clear();
        self.held.clear();
        self.rejected.clear();
        self.excluded.clear();
        self.state = v.base.state.clone();
        self.base = Some(v.base);
        self.refold();
        debug_assert_eq!(self.state, v.state, "the verified fold is the installed fold");
        let forgotten = self.me.is_some_and(|me| self.state.forgotten().contains(&me));
        if forgotten && !self.wiped {
            self.wipe();
            return true;
        }
        false
    }
}

/// A snapshot that passed verification: the store it describes (index entries, the
/// checkpoint and the kept bodies), its base, and the fold at the checkpoint.
pub(crate) struct Verified<S> {
    pub store: S,
    pub base: Base,
    pub state: State,
}

/// What a Checkpoint's `state_hash` commits to (v0.2, ADR 0007): the canonical
/// **base** state at the checkpoint, the index of everything before it (clock order,
/// checkpoint excluded), and the ids of the ops a snapshot there keeps as bodies.
/// The split between base and kept bodies is a function of before(C) (the backing
/// rule, [`split_at`]), so the author can commit to it and no snapshot provider can
/// move a kept op into the base, where a later Forget could no longer exclude it.
/// The index feeds V4/V5 later, so it must be as tamper-evident as the state.
pub fn checkpoint_hash(base_state: &[u8], index: &[(Id, IndexEntry)], kept: &BTreeSet<Id>) -> [u8; 32] {
    let entries: Vec<dcbor::CBOR> = index.iter().map(|(id, e)| e.to_cbor(id)).collect();
    let kept: Vec<dcbor::CBOR> = kept.iter().map(dcbor::CBOR::to_byte_string).collect();
    sha256(
        &dcbor::CBOR::from(vec![
            dcbor::CBOR::to_byte_string(base_state),
            dcbor::CBOR::from(entries),
            dcbor::CBOR::from(kept),
        ])
        .to_cbor_data(),
    )
}
