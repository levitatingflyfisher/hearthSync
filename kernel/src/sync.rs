//! Reconciliation between two replicas (design §3.3, §3.4).
//!
//! 1. Each side says [`Hello`]: its heads, its base checkpoint (if it pruned), and the
//!    checkpoints past the horizon it holds.
//! 2. The requester answers with a [`Request`]: its heads, and which of the
//!    provider's old checkpoints (base included) it lacks.
//! 3. The provider makes an [`Offer`]: the ops the requester lacks (every op not in
//!    before(requester's known heads)), or, if the requester lacks any old checkpoint,
//!    a [`Snapshot`] at one of them (or, if it can build none, at its own base:
//!    Fallback, ADR 0007) plus every op the provider holds above it. The
//!    provider need not have pruned (AgePull, ADR 0006): what matters is that the
//!    requester's edits never meet an old checkpoint except through a rebase.
//! 4. The requester ingests. After a snapshot it **rebases** its own unsynced ops:
//!    a field edit is re-issued on the new heads only if that field's winner is
//!    unchanged since the op was written; everything else goes on a review list (Q4).
//!    The re-issue carries the original edit's clock as `origin`, and LWW orders it by
//!    that clock (ADR 0006 OriginClock). Adoption also rebases or lists field winners
//!    in its old base that the snapshot lacks (BaseRebase), re-enrols the device if
//!    its enrolment was among them, lists other devices' unsynced ops
//!    (ForeignReview), and re-authors every Forget it knew that the snapshot lacks
//!    (Reforget). A replica that is wiped, or that the snapshot forgets, never adopts;
//!    [`reconcile`] has it hand over its own ops and Forgets instead (WipedPush).
//!
//! In-process in v0: the messages are plain structs. The LAN codec and relay path
//! are v1 (they will carry the same messages, sealed).

use std::collections::{BTreeMap, BTreeSet};

use dcbor::{CBORCase, CBOR};

use crate::keys;
use crate::op::{self, Body, Kind, Op, Reject, Value};
use crate::replica::{checkpoint_hash, fold, Base, IngestReport, KernelError, Replica, Verified};
use crate::state::State;
use crate::store::{IndexEntry, OpStore};
use crate::{DeviceId, Id};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    pub heads: Vec<Id>,
    pub base: Option<Id>,
    /// Checkpoints past the horizon this replica holds: its base, and every old
    /// checkpoint it still has a body for (the rest are behind the base).
    pub old: Vec<Id>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub heads: Vec<Id>,
    /// The provider's old checkpoints the requester lacks. Non-empty means the
    /// requester must take a snapshot before any op moves either way (AgePull).
    pub lacks: Vec<Id>,
}

/// The canonical state at a checkpoint, with the index of everything before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The signed Checkpoint op.
    pub checkpoint: Vec<u8>,
    /// Index entries for before(checkpoint), checkpoint excluded.
    pub index: Vec<(Id, IndexEntry)>,
    /// `State::encode()` of the base: the fold of the backed ops in before(checkpoint).
    pub state: Vec<u8>,
    /// Signed bytes of the kept ops (KeepBodies), in clock order: every op in
    /// before(checkpoint) that no checkpoint by another device backs. Folding them
    /// live on top of `state` gives the fold at the checkpoint.
    /// `checkpoint_hash(state, index, kept ids)` must equal the checkpoint's
    /// `state_hash`.
    pub kept: Vec<Vec<u8>>,
}

impl Snapshot {
    pub fn encode(&self) -> Vec<u8> {
        CBOR::from(vec![
            CBOR::to_byte_string(&self.checkpoint),
            CBOR::from(self.index.iter().map(|(id, e)| e.to_cbor(id)).collect::<Vec<_>>()),
            CBOR::to_byte_string(&self.state),
            CBOR::from(self.kept.iter().map(CBOR::to_byte_string).collect::<Vec<_>>()),
        ])
        .to_cbor_data()
    }

    pub fn decode(bytes: &[u8]) -> Result<Snapshot, Reject> {
        let c = crate::cbor::decode(bytes).ok_or(Reject::NotCanonical)?;
        let CBORCase::Array(a) = c.as_case() else { return Err(Reject::Schema) };
        if a.len() != 4 {
            return Err(Reject::Schema);
        }
        let bs = |c: &CBOR| match c.as_case() {
            CBORCase::ByteString(b) => Ok(b.data().to_vec()),
            _ => Err(Reject::Schema),
        };
        let CBORCase::Array(idx) = a[1].as_case() else { return Err(Reject::Schema) };
        let CBORCase::Array(kept) = a[3].as_case() else { return Err(Reject::Schema) };
        Ok(Snapshot {
            checkpoint: bs(&a[0])?,
            index: idx.iter().map(IndexEntry::from_cbor).collect::<Result<_, _>>()?,
            state: bs(&a[2])?,
            kept: kept.iter().map(bs).collect::<Result<_, _>>()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offer {
    Ops(Vec<Vec<u8>>),
    Snapshot { snapshot: Snapshot, ops: Vec<Vec<u8>> },
}

/// Something the returning device held that was not re-applied automatically.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewItem {
    /// Someone else changed this field since the edit was written.
    Field { op: Id, table: String, row: String, field: String, mine: Value, current: Option<Value> },
    /// The row was deleted since the edit was written.
    RowDeleted { op: Id, table: String, row: String },
    /// A delete, restore, set-remove or forget, or a set add that a remove had already
    /// undone: the app decides whether to redo it.
    Op { op: Id, kind: Kind },
    /// An op by another device that only this replica held (ForeignReview, ADR 0006):
    /// it is listed, never pushed on as it is, since it may be a late edit too.
    Foreign { op: Id, device: DeviceId, body: Body },
    /// A field value another device wrote that was folded into our old base and that
    /// the adopted snapshot lacks (BaseRebase, ADR 0006).
    Lost { op: Id, device: DeviceId, table: String, row: String, field: String, value: Value },
    /// Any other op folded into our old base that the adopted snapshot lacks and that
    /// was not re-issued (BaseRebase for every kind, ADR 0007): what the old base
    /// state still said about it. Nothing pruned vanishes without a re-issue or one of
    /// these (or a `Lost` / `Field` / `RowDeleted` item for a Put's fields).
    Pruned { op: Id, device: DeviceId, content: PrunedContent },
}

impl ReviewItem {
    /// The op this item is about.
    pub fn op(&self) -> Id {
        match self {
            ReviewItem::Field { op, .. }
            | ReviewItem::RowDeleted { op, .. }
            | ReviewItem::Op { op, .. }
            | ReviewItem::Foreign { op, .. }
            | ReviewItem::Lost { op, .. }
            | ReviewItem::Pruned { op, .. } => *op,
        }
    }
}

/// What the old base state recorded for a pruned op that the rebase could not
/// re-issue. The body is gone; this is all that is left of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrunedContent {
    /// An edit that no longer won any field (it may still have kept the row visible).
    Put {
        table: String,
        row: String,
    },
    Delete {
        table: String,
        row: String,
        observed: Vec<Id>,
    },
    Restore {
        table: String,
        row: String,
    },
    SetAdd {
        set: String,
        element: Value,
    },
    SetRemove {
        set: String,
        element: Value,
    },
    Append {
        stream: String,
        record: Value,
    },
    Enroll {
        device: DeviceId,
    },
    /// The base state no longer says anything about it.
    Unknown {
        kind: Kind,
    },
}

/// Where adoption stands between installing the snapshot and rebasing.
pub(crate) enum Installed {
    /// Nothing to rebase (the replica was wiped, or is wiped by the snapshot).
    Done(AcceptReport),
    /// Installed; the rebase is still to run.
    Rebase(Box<Adoption>),
}

/// What the rebase must redo, worked out before the snapshot was installed.
#[derive(Clone)]
pub(crate) struct Adoption {
    own: Vec<(Id, Op, BTreeMap<String, Option<Id>>)>,
    undone: BTreeSet<Id>,
    reforget: Vec<(Id, DeviceId)>,
    lost: LostOps,
    my_enroll: Option<(Id, String)>,
    report: RebaseReport,
    ingest: IngestReport,
}

/// What of our old base the snapshot lacks and the rebase must redo.
#[derive(Clone, Default)]
struct LostOps {
    /// Our own field winners.
    edits: Vec<LostEdit>,
    /// Our own set adds and appends, re-issued as they were.
    reissue: Vec<(Id, Body)>,
    /// Our own Enroll ops (renewed by the re-enrolment).
    own_enrolls: Vec<Id>,
}

/// One of our own edits folded into the old base that the snapshot lacks.
#[derive(Clone)]
struct LostEdit {
    op: Id,
    table: String,
    row: String,
    /// The fields it still won, with their values.
    fields: BTreeMap<String, Value>,
    /// Its LWW clock (its origin if it was itself a re-issue).
    origin: op::Hlc,
    /// before(op), from the index.
    seen: BTreeSet<Id>,
    /// The row was deleted in the old state: the edit is listed, never re-issued.
    hidden: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RebaseReport {
    /// old op id -> re-issued op id
    pub reissued: BTreeMap<Id, Id>,
    pub review: Vec<ReviewItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AcceptReport {
    pub ingest: IngestReport,
    pub rebase: Option<RebaseReport>,
}

impl<S: OpStore + Default> Replica<S> {
    /// `now` decides which checkpoints are past the horizon.
    pub fn hello(&self, now: u64) -> Hello {
        let base = self.base_checkpoint();
        let mut old: BTreeSet<Id> = base.into_iter().collect();
        old.extend(self.store.ids().into_iter().filter(|id| self.store.body(id).is_some() && self.is_old(id, now)));
        Hello { heads: self.heads(), base, old: old.into_iter().collect() }
    }

    pub fn request(&self, provider: &Hello) -> Request {
        Request {
            heads: self.heads(),
            lacks: provider.old.iter().filter(|c| !self.store.contains(c)).copied().collect(),
        }
    }

    /// The ops the requester lacks, or, if it lacks one of our old checkpoints, a
    /// snapshot at the newest of those we can build one at.
    pub fn offer(&self, req: &Request) -> Result<Offer, KernelError> {
        let known: Vec<Id> = req.heads.iter().filter(|h| self.store.contains(h)).copied().collect();
        let theirs = self.closure(&known);
        if !req.lacks.is_empty() {
            // The newest lacked checkpoint we can build a snapshot at; failing that
            // (it is concurrent with our base), our base itself, even though the
            // requester holds it (Fallback, ADR 0007): adopting it still hands over
            // everything we hold, the lacked checkpoint included, and rebases the
            // requester's own ops, so they meet it only through the rebase.
            let cp = self
                .snapshot_candidates()
                .into_iter()
                .filter(|c| req.lacks.contains(c))
                .map(|c| (self.store.entry(&c).expect("indexed").hlc, c))
                .max()
                .map(|(_, c)| c)
                .or(self.base_checkpoint())
                .ok_or(KernelError::SnapshotUnavailable)?;
            let base = match &self.base {
                Some(b) if b.checkpoint == cp => b.clone(),
                _ => self.base_at(cp)?,
            };
            let ops = self
                .ids_in_clock_order(self.store.ids().into_iter().filter(|id| !base.covered.contains(id)))
                .into_iter()
                .filter_map(|id| self.store.body(&id).map(<[u8]>::to_vec))
                .collect();
            return Ok(Offer::Snapshot { snapshot: self.snapshot_of(&base), ops });
        }
        // Pruned bodies are all behind our base, which the requester holds.
        let ops = self
            .ids_in_clock_order(self.store.ids().into_iter().filter(|id| !theirs.contains(id)))
            .into_iter()
            .filter_map(|id| self.store.body(&id).map(<[u8]>::to_vec))
            .collect();
        Ok(Offer::Ops(ops))
    }

    /// Our base as a snapshot, if we have pruned.
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.base.as_ref().map(|b| self.snapshot_of(b))
    }

    fn snapshot_of(&self, base: &Base) -> Snapshot {
        let body = |id: Id| self.store.body(&id).expect("base checkpoint and kept bodies are held").to_vec();
        Snapshot {
            checkpoint: body(base.checkpoint),
            index: self.index_of(base.covered.iter().copied().filter(|id| *id != base.checkpoint)),
            state: base.state.encode(),
            kept: self.ids_in_clock_order(base.keep.iter().copied()).into_iter().map(body).collect(),
        }
    }

    /// Check a snapshot before touching anything: the checkpoint's signature and app,
    /// the kept bodies against the index, the index closed under parents, and the
    /// fold at the checkpoint (base state plus the kept ops, with V6 and the shield
    /// over the index) against `state_hash`.
    fn verify(&self, snap: &Snapshot) -> Result<(Verified<S>, Op, Id), KernelError> {
        let bad = KernelError::BadSnapshot;
        let (cp_op, cp_id) = Op::decode_verified(&snap.checkpoint).map_err(bad)?;
        if cp_op.app != self.app {
            return Err(bad(Reject::WrongApp));
        }
        let Body::Checkpoint { state_hash } = cp_op.body else { return Err(bad(Reject::Schema)) };
        let base_state = State::decode(&snap.state).map_err(bad)?;
        let index: BTreeMap<Id, IndexEntry> = snap.index.iter().cloned().collect();
        let closed =
            index.values().flat_map(|e| e.parents.iter()).chain(cp_op.parents.iter()).all(|p| index.contains_key(p));
        if !closed || index.len() != snap.index.len() || index.contains_key(&cp_id) {
            return Err(bad(Reject::Schema));
        }
        // V3 over the index: ancestry checks (`all_in_past`) rely on clocks rising
        // from parent to child, so a snapshot must not break that either.
        let rises = |parents: &[Id], hlc: op::Hlc| parents.iter().all(|p| index.get(p).is_some_and(|pe| pe.hlc < hlc));
        if !index.values().all(|e| rises(&e.parents, e.hlc)) || !rises(&cp_op.parents, cp_op.hlc) {
            return Err(bad(Reject::ClockNotAfterParents));
        }
        let mut kept = BTreeMap::new();
        for b in &snap.kept {
            let (op, id) = Op::decode_verified(b).map_err(bad)?;
            if op.app != self.app || index.get(&id) != Some(&IndexEntry::of(&op)) {
                return Err(bad(Reject::Schema));
            }
            kept.insert(id, b.clone());
        }
        let mut store = S::default();
        let mut sorted: Vec<(Id, IndexEntry)> = index.into_iter().collect();
        sorted.sort_by_key(|(id, e)| (e.hlc, *id));
        for (id, e) in sorted {
            let body = kept.get(&id).cloned();
            store.insert(id, e, body);
        }
        store.insert(cp_id, IndexEntry::of(&cp_op), Some(snap.checkpoint.clone()));
        let mut covered: BTreeSet<Id> = store.ids().into_iter().collect();
        covered.insert(cp_id);
        let keep: BTreeSet<Id> = kept.keys().copied().collect();
        // The checkpoint commits to the base state, the index (V4/V5 will trust it) and
        // the kept set, so a provider can neither move a kept op into the base nor drop
        // or add a body.
        if checkpoint_hash(&snap.state, &snap.index, &keep) != state_hash {
            return Err(bad(Reject::NotCanonical));
        }
        let base = Base { checkpoint: cp_id, covered, keep, state: base_state };
        let at = fold(&store, Some(&base), None);
        if !at.state.is_enrolled(&cp_op.device) {
            return Err(bad(Reject::NotEnrolled));
        }
        Ok((Verified { store, base, state: at.state }, cp_op, cp_id))
    }

    /// WipedPush (ADR 0006): what a wiped replica may hand over as it is when the peer
    /// holds an old checkpoint it lacks: its own ops and its own Forgets with their
    /// past, never a checkpoint past the horizon (the receiver must take that as a
    /// snapshot and rebase). `heads` are the receiver's.
    pub fn handover(&self, heads: &[Id], now: u64) -> Vec<Vec<u8>> {
        let Some(me) = self.me else { return Vec::new() };
        let known: Vec<Id> = heads.iter().filter(|h| self.store.contains(h)).copied().collect();
        let theirs = self.closure(&known);
        let mine: Vec<Id> =
            self.store.ids().into_iter().filter(|id| self.store.entry(id).is_some_and(|e| e.device == me)).collect();
        let forgets: Vec<Id> =
            mine.iter().filter(|id| self.store.entry(id).is_some_and(|e| e.kind == Kind::Forget)).copied().collect();
        let mut set: BTreeSet<Id> = mine.into_iter().collect();
        set.extend(self.closure(&forgets));
        self.ids_in_clock_order(set.into_iter().filter(|id| !theirs.contains(id) && !self.is_old(id, now)))
            .into_iter()
            .filter_map(|id| self.store.body(&id).map(<[u8]>::to_vec))
            .collect()
    }

    /// A Forget of this device that the enroll key really signed (a wipe trigger must
    /// come from verified input only).
    fn forgets_me(&self, bytes: &[u8]) -> bool {
        let Some(me) = self.me else { return false };
        let Ok((op, _)) = Op::decode_verified(bytes) else { return false };
        match &op.body {
            Body::Forget { device, cut, auth } => {
                op.app == self.app
                    && *device == me
                    && op::verify_strict(&self.enroll_pk, &keys::forget_auth_msg(&self.app, device, cut), auth)
            }
            _ => false,
        }
    }

    pub fn accept(&mut self, offer: Offer, now: u64) -> Result<AcceptReport, KernelError> {
        match offer {
            Offer::Ops(ops) => Ok(AcceptReport { ingest: self.ingest(&ops, now), rebase: None }),
            Offer::Snapshot { snapshot, ops } => self.adopt(snapshot, ops, now),
        }
    }

    fn adopt(&mut self, snap: Snapshot, ops: Vec<Vec<u8>>, now: u64) -> Result<AcceptReport, KernelError> {
        match self.adopt_install(snap, ops, now)? {
            Installed::Done(report) => Ok(report),
            Installed::Rebase(a) => self.adopt_rebase(*a, now),
        }
    }

    /// Adoption, first half: verify, install and ingest, and work out what the
    /// rebase must redo. Authors nothing, so the api can run it once and replay only
    /// the second half while it collects signatures (ADR 0011).
    pub(crate) fn adopt_install(
        &mut self,
        snap: Snapshot,
        ops: Vec<Vec<u8>>,
        now: u64,
    ) -> Result<Installed, KernelError> {
        // Verify before touching anything.
        let (verified, _, cp_id) = self.verify(&snap)?;
        // WipedPush: a replica that is wiped, or that this snapshot would wipe, never
        // adopts. It wipes on what the snapshot tells it and keeps its log, so the
        // ops and Forgets only it holds can still be handed over.
        let forgotten = self.me.is_some_and(|me| verified.state.forgotten().contains(&me))
            || ops.iter().any(|o| self.forgets_me(o));
        if self.is_wiped() || forgotten {
            let wiped = !self.is_wiped();
            if wiped {
                self.wipe();
            }
            return Ok(Installed::Done(AcceptReport {
                ingest: IngestReport { wiped, ..Default::default() },
                rebase: None,
            }));
        }
        // Adoption authors once the snapshot is in (rebase, Reforget, re-enrolment),
        // and the creation-time guard refuses to stamp while the log runs more than
        // the skew bound ahead of the clock. Refuse now, before touching anything:
        // failing half-way would leave unsynced edits in neither log nor review.
        if self.has_keys() {
            let top = verified
                .store
                .ids()
                .iter()
                .filter_map(|id| verified.store.entry(id).map(|e| e.hlc))
                .fold(self.max_hlc, std::cmp::max);
            if now.saturating_add(crate::MAX_FUTURE_SKEW_MS) < top.millis {
                return Err(KernelError::ClockBehind { now, latest: top.millis });
            }
        }
        let index_ids: BTreeSet<Id> = snap.index.iter().map(|(id, _)| *id).collect();

        // What the snapshot itself vouches for (its index is hash-checked). The ops
        // offered above it count as known only once they have actually delivered
        // (checked after ingest below): a provider could list our op and withhold its
        // parent, and the op would then be neither delivered nor rebased.
        let mut peer_known = index_ids.clone();
        peer_known.insert(cp_id);

        // What we wrote that the peer never saw, and what each edit saw when written.
        let unsynced: Vec<Id> = self.ids_in_clock_order(
            self.store.ids().into_iter().filter(|id| !peer_known.contains(id) && self.store.body(id).is_some()),
        );
        let mut own = Vec::new();
        let mut undone = BTreeSet::new();
        let mut report = RebaseReport::default();
        // Reforget: every Forget this replica knew that the snapshot lacks (its own or
        // not, a body or pruned into its base) is re-authored on the new heads.
        let mut reforget: Vec<(Id, DeviceId)> =
            self.state.forgets.iter().filter(|(f, _)| !peer_known.contains(*f)).map(|(f, (d, _))| (*f, *d)).collect();
        let mut lost = self.lost_base_ops(&peer_known, &mut report);
        // Our own enrolment may have been pruned into the base too, with no body to
        // rebase; without it nothing we re-issue would be valid (V4).
        let my_enroll = self.me.and_then(|me| self.state.enrolled.get(&me)).map(|(_, id, label)| (*id, label.clone()));
        for id in unsynced {
            let op = self.body_of(&id).expect("has body");
            if op.body.kind() == Kind::Forget {
                continue;
            }
            if Some(op.device) == self.me {
                if self.undone(&id) {
                    undone.insert(id);
                }
                let seen = self.winners_seen(&op);
                own.push((id, op, seen));
            } else if op.body.kind() != Kind::Checkpoint {
                // ForeignReview: listed, never re-offered as it is.
                report.review.push(ReviewItem::Foreign { op: id, device: op.device, body: op.body });
            }
        }

        let wiped = self.install(verified);
        debug_assert!(!wiped, "a snapshot that forgets us is never adopted");
        let mut ingest = self.ingest(&ops, now);
        ingest.wiped |= wiped;
        // Whatever the offer delivered, the peer has: no rebase, no review item.
        {
            let got = |id: &Id| self.store.contains(id);
            own.retain(|(id, _, _)| !got(id));
            report.review.retain(|i| !got(&i.op()));
            reforget.retain(|(id, _)| !got(id));
            lost.edits.retain(|l| !got(&l.op));
            lost.reissue.retain(|(id, _)| !got(id));
            lost.own_enrolls.retain(|id| !got(id));
        }
        // Our own ops the offer listed but that could not deliver (a parent withheld)
        // are rebased below; they must not also wait for that parent forever.
        self.drop_pending(own.iter().map(|(id, _, _)| *id));
        Ok(Installed::Rebase(Box::new(Adoption { own, undone, reforget, lost, my_enroll, report, ingest })))
    }

    /// Adoption, second half: re-enrol, rebase, re-issue and re-forget, authoring on
    /// the adopted heads.
    pub(crate) fn adopt_rebase(&mut self, a: Adoption, now: u64) -> Result<AcceptReport, KernelError> {
        let Adoption { own, undone, reforget, lost, my_enroll, mut report, ingest } = a;
        let mut own_enrolls = lost.own_enrolls;
        if let Some((old, _)) = &my_enroll {
            if !self.store.contains(old) {
                own_enrolls.push(*old);
            }
        }
        if let (Some(me), Some((_, label))) = (self.me, my_enroll) {
            if self.has_keys() && !self.state.is_enrolled(&me) {
                self.enroll_self(&label, now)?;
            }
            // Every own Enroll the snapshot lacks is superseded by the enrolment that
            // now stands (the snapshot's, or the renewed one).
            if let Some((_, current, _)) = self.state.enrolled.get(&me) {
                for old in own_enrolls {
                    report.reissued.insert(old, *current);
                }
            }
        }
        self.rebase_lost(lost.edits, now, &mut report)?;
        self.reissue_lost(lost.reissue, now, &mut report)?;
        self.rebase(own, &undone, now, &mut report)?;
        self.reforget(reforget, now, &mut report)?;
        Ok(AcceptReport { ingest, rebase: Some(report) })
    }

    /// For a Put: per field, the winning op id in before(op) (in the old log), by the
    /// same LWW order the fold uses.
    fn winners_seen(&self, op: &Op) -> BTreeMap<String, Option<Id>> {
        let Body::Put { table, row, fields, .. } = &op.body else { return BTreeMap::new() };
        let target = op::row_target(table, row);
        let past = self.closure(&op.parents);
        type Key = (op::Hlc, op::Hlc, Id);
        let mut best: BTreeMap<String, Option<Key>> = fields.keys().map(|f| (f.clone(), None)).collect();
        // The part of the past that was pruned lives in our base state.
        if let Some(b) = &self.base {
            if past.contains(&b.checkpoint) {
                if let Some(r) = b.state.rows.get(&(table.clone(), row.clone())) {
                    for (f, slot) in best.iter_mut() {
                        *slot = r.fields.get(f).map(|(l, h, i, _)| (*l, *h, *i));
                    }
                }
            }
        }
        for id in &past {
            let Some(e) = self.store.entry(id) else { continue };
            if e.kind != Kind::Put || e.target != Some(target) || self.excluded().contains(id) {
                continue;
            }
            let Some(p) = self.body_of(id) else { continue };
            let Body::Put { fields: pf, origin, .. } = &p.body else { continue };
            let key = (origin.unwrap_or(p.hlc), p.hlc, *id);
            for (f, slot) in best.iter_mut() {
                if pf.contains_key(f) && slot.is_none_or(|s| key > s) {
                    *slot = Some(key);
                }
            }
        }
        best.into_iter().map(|(f, s)| (f, s.map(|(_, _, i)| i))).collect()
    }

    /// BaseRebase (ADR 0006, every kind since ADR 0007): ops folded into our old base
    /// (pruned, no body) that the snapshot lacks would vanish on adoption. Our own
    /// field winners go back for a rebase and our own set adds and appends are
    /// re-issued; everything else is listed with what the base state still says about
    /// it. Forgets are re-authored (Reforget) and our own enrolment is renewed
    /// separately; checkpoints carry no data.
    fn lost_base_ops(&self, peer_known: &BTreeSet<Id>, report: &mut RebaseReport) -> LostOps {
        let mut lost = LostOps::default();
        let Some(base) = &self.base else { return lost };
        let gone = |id: &Id| {
            base.covered.contains(id) && !base.keep.contains(id) && *id != base.checkpoint && !peer_known.contains(id)
        };
        let mut own: BTreeMap<Id, LostEdit> = BTreeMap::new();
        let mut winners = BTreeSet::new();
        for ((table, row), r) in &self.state.rows {
            for (field, (lww, _, w, value)) in &r.fields {
                if !gone(w) {
                    continue;
                }
                winners.insert(*w);
                let e = self.store.entry(w).expect("covered ops are indexed");
                if Some(e.device) == self.me {
                    let edit = own.entry(*w).or_insert_with(|| LostEdit {
                        op: *w,
                        table: table.clone(),
                        row: row.clone(),
                        fields: BTreeMap::new(),
                        origin: *lww,
                        seen: self.closure(&e.parents),
                        // Undone if the row is deleted, or if a live delete observed
                        // this very edit (another edit may keep the row visible): by
                        // observed-remove it stays undone, as for unsynced ops.
                        hidden: !r.visible()
                            || r.deletes.iter().any(|(d, obs)| obs.contains(w) && !r.restored.contains(d)),
                    });
                    edit.fields.insert(field.clone(), value.clone());
                } else {
                    report.review.push(ReviewItem::Lost {
                        op: *w,
                        device: e.device,
                        table: table.clone(),
                        row: row.clone(),
                        field: field.clone(),
                        value: value.clone(),
                    });
                }
            }
        }
        lost.edits = own.into_values().collect();
        lost.edits.sort_by_key(|l| (l.origin, l.op));
        let st = &self.state;
        let row_of =
            |target: Option<[u8; 32]>| st.rows.keys().find(|(t, r)| Some(op::row_target(t, r)) == target).cloned();
        let element_of = |target: Option<[u8; 32]>| {
            st.sets.keys().find(|(set, el)| Some(op::element_target(set, el)) == target).cloned()
        };
        for id in self.ids_in_clock_order(base.covered.iter().copied().filter(|id| gone(id) && !winners.contains(id))) {
            let e = self.store.entry(&id).expect("covered ops are indexed");
            let mine = Some(e.device) == self.me;
            let content = match e.kind {
                Kind::Checkpoint | Kind::Forget => continue,
                Kind::Enroll if mine => {
                    lost.own_enrolls.push(id);
                    continue;
                }
                Kind::Enroll => PrunedContent::Enroll { device: e.enrolls.expect("an Enroll names its device") },
                Kind::Put => match row_of(e.target) {
                    Some((table, row)) => PrunedContent::Put { table, row },
                    None => PrunedContent::Unknown { kind: e.kind },
                },
                Kind::Delete => {
                    let found = st.rows.iter().find_map(|((t, r), rs)| {
                        rs.deletes.get(&id).map(|obs| (t.clone(), r.clone(), obs.iter().copied().collect()))
                    });
                    match found {
                        Some((table, row, observed)) => PrunedContent::Delete { table, row, observed },
                        None => PrunedContent::Unknown { kind: e.kind },
                    }
                }
                Kind::Restore => match row_of(e.target) {
                    Some((table, row)) => PrunedContent::Restore { table, row },
                    None => PrunedContent::Unknown { kind: e.kind },
                },
                Kind::SetAdd => match st.sets.iter().find(|(_, el)| el.adds.contains(&id)) {
                    Some(((set, element), el)) => {
                        // A tag already removed is not brought back: listed instead.
                        if mine && !el.removed.contains(&id) {
                            lost.reissue.push((id, Body::SetAdd { set: set.clone(), element: element.clone() }));
                            continue;
                        }
                        PrunedContent::SetAdd { set: set.clone(), element: element.clone() }
                    }
                    None => PrunedContent::Unknown { kind: e.kind },
                },
                Kind::SetRemove => match element_of(e.target) {
                    Some((set, element)) => PrunedContent::SetRemove { set, element },
                    None => PrunedContent::Unknown { kind: e.kind },
                },
                Kind::Append => {
                    let found = st.streams.iter().find_map(|(name, recs)| {
                        recs.iter().find(|((_, i), _)| *i == id).map(|(_, (_, v))| (name.clone(), v.clone()))
                    });
                    match found {
                        Some((stream, record)) => {
                            if mine {
                                lost.reissue
                                    .push((id, Body::Append { stream: stream.clone(), record: record.clone() }));
                                continue;
                            }
                            PrunedContent::Append { stream, record }
                        }
                        None => PrunedContent::Unknown { kind: e.kind },
                    }
                }
            };
            report.review.push(ReviewItem::Pruned { op: id, device: e.device, content });
        }
        lost
    }

    /// Re-issue our own lost set adds and appends, as the rebase does for unsynced
    /// ones; a keyless replica lists them.
    fn reissue_lost(
        &mut self,
        bodies: Vec<(Id, Body)>,
        now: u64,
        report: &mut RebaseReport,
    ) -> Result<(), KernelError> {
        for (old, body) in bodies {
            if self.has_keys() {
                let (new, _) = self.author(body, now)?;
                report.reissued.insert(old, new);
            } else {
                let device = self.me.unwrap_or_default();
                let content = match body {
                    Body::SetAdd { set, element } => PrunedContent::SetAdd { set, element },
                    Body::Append { stream, record } => PrunedContent::Append { stream, record },
                    b => PrunedContent::Unknown { kind: b.kind() },
                };
                report.review.push(ReviewItem::Pruned { op: old, device, content });
            }
        }
        Ok(())
    }

    /// Re-issue our own lost base winners where the field's current winner is one the
    /// edit saw (it is in before(edit), or re-issues something that is), and list the
    /// rest. The bodies are gone, so "saw" is read off the index, not off the edit's
    /// own winners (a deviation from the model's `seenWin`, recorded in ADR 0006).
    fn rebase_lost(&mut self, lost: Vec<LostEdit>, now: u64, report: &mut RebaseReport) -> Result<(), KernelError> {
        for l in lost {
            let key = (l.table.clone(), l.row.clone());
            // Deleted then (in the old state, whose delete may be lost too) or now.
            if l.hidden || self.state.rows.get(&key).is_some_and(|r| !r.visible()) {
                report.review.push(ReviewItem::RowDeleted { op: l.op, table: l.table, row: l.row });
                continue;
            }
            let mut keep = BTreeMap::new();
            for (f, v) in l.fields {
                let current = self.state.rows.get(&key).and_then(|r| r.fields.get(&f));
                let seen = current.is_none_or(|(_, _, i, _)| {
                    l.seen.contains(i) || report.reissued.iter().any(|(old, new)| new == i && l.seen.contains(old))
                });
                if seen && self.has_keys() {
                    keep.insert(f, v);
                } else {
                    report.review.push(ReviewItem::Field {
                        op: l.op,
                        table: l.table.clone(),
                        row: l.row.clone(),
                        field: f,
                        mine: v,
                        current: current.map(|(_, _, _, v)| v.clone()),
                    });
                }
            }
            if !keep.is_empty() {
                let body = Body::Put { table: l.table, row: l.row, fields: keep, origin: Some(l.origin) };
                let (new, _) = self.author(body, now)?;
                report.reissued.insert(l.op, new);
            }
        }
        Ok(())
    }

    /// Re-author each lost Forget, once per target, with a fresh cut (every lost
    /// Forget of that target maps to the one re-authored). The phrase is on
    /// every device (Q1), so any keyed replica can; a keyless one lists them.
    fn reforget(&mut self, lost: Vec<(Id, DeviceId)>, now: u64, report: &mut RebaseReport) -> Result<(), KernelError> {
        let mut done: BTreeMap<DeviceId, Id> = BTreeMap::new();
        for (old, target) in lost {
            if !self.has_keys() {
                report.review.push(ReviewItem::Op { op: old, kind: Kind::Forget });
                continue;
            }
            let new = match done.get(&target) {
                Some(new) => *new,
                None => {
                    let (new, _) = self.forget(target, now)?;
                    done.insert(target, new);
                    new
                }
            };
            report.reissued.insert(old, new);
        }
        Ok(())
    }

    /// Whether our current state has already undone this op: an add whose tag some
    /// remove observed, or an edit some live delete observed. By the OR-set and
    /// observed-remove rules such an op stays undone; one the remove or delete did not
    /// observe (it was concurrent) still wins, and is not "undone" here.
    fn undone(&self, id: &Id) -> bool {
        self.state.sets.values().any(|e| e.adds.contains(id) && e.removed.contains(id))
            || self.state.rows.values().any(|r| {
                r.puts.contains(id) && r.deletes.iter().any(|(d, obs)| obs.contains(id) && !r.restored.contains(d))
            })
    }

    /// `undone` (computed on the old state) are listed, never re-issued: re-issuing
    /// them would give them a fresh tag or id that the listed remove or delete no
    /// longer observes, bringing back what the user removed.
    fn rebase(
        &mut self,
        own: Vec<(Id, Op, BTreeMap<String, Option<Id>>)>,
        undone: &BTreeSet<Id>,
        now: u64,
        report: &mut RebaseReport,
    ) -> Result<(), KernelError> {
        for (old, op, seen) in own {
            if !self.has_keys() {
                report.review.push(ReviewItem::Op { op: old, kind: op.body.kind() });
                continue;
            }
            match &op.body {
                Body::Put { table, row, fields, origin } => {
                    let key = (table.clone(), row.clone());
                    if undone.contains(&old) || self.state.rows.get(&key).is_some_and(|r| !r.visible()) {
                        report.review.push(ReviewItem::RowDeleted { op: old, table: table.clone(), row: row.clone() });
                        continue;
                    }
                    let mut keep = BTreeMap::new();
                    for (f, v) in fields {
                        let expected =
                            seen.get(f).copied().flatten().map(|w| report.reissued.get(&w).copied().unwrap_or(w));
                        let current = self.state.rows.get(&key).and_then(|r| r.fields.get(f));
                        if current.map(|(_, _, i, _)| *i) == expected {
                            keep.insert(f.clone(), v.clone());
                        } else {
                            report.review.push(ReviewItem::Field {
                                op: old,
                                table: table.clone(),
                                row: row.clone(),
                                field: f.clone(),
                                mine: v.clone(),
                                current: current.map(|(_, _, _, v)| v.clone()),
                            });
                        }
                    }
                    if !keep.is_empty() {
                        // OriginClock: the re-issue orders by the original edit's clock.
                        let origin = Some(origin.unwrap_or(op.hlc));
                        let body = Body::Put { table: table.clone(), row: row.clone(), fields: keep, origin };
                        let (new, _) = self.author(body, now)?;
                        report.reissued.insert(old, new);
                    }
                }
                Body::Enroll { device, .. } => {
                    if !self.state.is_enrolled(device) {
                        let (new, _) = self.author(op.body.clone(), now)?;
                        report.reissued.insert(old, new);
                    } else if let Some((_, current, _)) = self.state.enrolled.get(device) {
                        // Superseded by the enrolment that stands.
                        report.reissued.insert(old, *current);
                    }
                }
                Body::SetAdd { .. } if undone.contains(&old) => {
                    report.review.push(ReviewItem::Op { op: old, kind: Kind::SetAdd });
                }
                Body::SetAdd { .. } | Body::Append { .. } => {
                    let (new, _) = self.author(op.body.clone(), now)?;
                    report.reissued.insert(old, new);
                }
                Body::Checkpoint { .. } => {}
                _ => report.review.push(ReviewItem::Op { op: old, kind: op.body.kind() }),
            }
        }
        Ok(())
    }
}

/// Full two-way sync between two in-process replicas. A side that lacks any
/// checkpoint past the horizon that the other holds pulls a snapshot first and
/// rebases (AgePull), so its stale ops are never pushed as they are and never meet
/// an old checkpoint except through the rebase.
pub fn reconcile<S: OpStore + Default, T: OpStore + Default>(
    a: &mut Replica<S>,
    b: &mut Replica<T>,
    now: u64,
) -> Result<(AcceptReport, AcceptReport), KernelError> {
    let mut to_a = AcceptReport::default();
    let mut to_b = AcceptReport::default();
    // A wiped side never adopts (WipedPush), so it does not ask for a snapshot.
    let ra = a.request(&b.hello(now));
    if !ra.lacks.is_empty() && !a.is_wiped() {
        merge(&mut to_a, a.accept(b.offer(&ra)?, now)?);
    }
    let rb = b.request(&a.hello(now));
    if !rb.lacks.is_empty() && !b.is_wiped() {
        merge(&mut to_b, b.accept(a.offer(&rb)?, now)?);
    }
    let rb = b.request(&a.hello(now));
    let ra = a.request(&b.hello(now));
    if !ra.lacks.is_empty() || !rb.lacks.is_empty() {
        // WipedPush: a wiped side that still lacks an old checkpoint only hands over
        // its own ops and its Forgets' past; nothing else moves either way.
        for (lacks, wiped) in [(!ra.lacks.is_empty(), a.is_wiped()), (!rb.lacks.is_empty(), b.is_wiped())] {
            if lacks && !wiped {
                return Err(KernelError::SnapshotUnavailable);
            }
        }
        if !ra.lacks.is_empty() {
            to_b.ingest = merged(to_b.ingest, b.ingest(a.handover(&rb.heads, now), now));
        }
        if !rb.lacks.is_empty() {
            to_a.ingest = merged(to_a.ingest, a.ingest(b.handover(&ra.heads, now), now));
        }
        return Ok((to_a, to_b));
    }
    merge(&mut to_b, b.accept(a.offer(&rb)?, now)?);
    let ra = a.request(&b.hello(now));
    merge(&mut to_a, a.accept(b.offer(&ra)?, now)?);
    Ok((to_a, to_b))
}

fn merge(into: &mut AcceptReport, later: AcceptReport) {
    into.ingest = merged(std::mem::take(&mut into.ingest), later.ingest);
    if into.rebase.is_none() {
        into.rebase = later.rebase;
    }
}

fn merged(mut i: IngestReport, mut l: IngestReport) -> IngestReport {
    i.delivered.append(&mut l.delivered);
    i.excluded.append(&mut l.excluded);
    i.rejected.append(&mut l.rejected);
    i.quarantined.append(&mut l.quarantined);
    i.held.append(&mut l.held);
    i.pending.append(&mut l.pending);
    i.duplicates += l.duplicates;
    i.wiped |= l.wiped;
    i
}
