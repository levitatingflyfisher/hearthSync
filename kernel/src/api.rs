//! The app-facing surface (ADR 0011): what flutter_rust_bridge exposes to Dart, and
//! what glean may call directly.
//!
//! Everything here is synchronous (the web build is single-threaded WASM called with
//! `#[frb(sync)]`, per the spike) and crosses the boundary as plain types: byte
//! vectors, strings, integers, and simple structs and enums. No generics, lifetimes,
//! callbacks or database handles. One [`Kernel`] is one app's replica on this device.
//!
//! **Keys.** The household seed comes in once, at [`Kernel::open`]. The device's own
//! private key never enters the kernel: every call that writes returns
//! [`Step::Sign`] with the bytes to sign, and the app signs them with the platform
//! key store and calls [`Kernel::finish`] with the signature, until it gets
//! [`Step::Done`]. A local write needs one signature. Adopting a snapshot may need
//! several (the rebase re-issues ops, each on top of the last): the kernel runs the
//! flow on a copy of the replica and replays it from the start with the signatures
//! collected so far, so nothing changes until the flow completes. Ed25519 is
//! deterministic and the kernel is pure, so each replay asks for exactly the same
//! bytes up to the next new one.
//!
//! **Storage.** Every completed call returns an [`Outcome`] carrying the records to
//! write ([`Outcome::records`], ADR 0010) and the changes to the app's own tables
//! ([`Outcome::rows`], [`Outcome::sets`], [`Outcome::streams`]). Write both in one
//! transaction before sending anything the call produced.
//!
//! **Sync.** Messages are sealed (ADR 0008). A full sync between two devices, each
//! running its half, is the order `sync::reconcile` uses (ADR 0011 has the
//! choreography): each side sends [`Kernel::hello`]; each answers with
//! [`Kernel::request`], which says whether it needs a snapshot first; the provider
//! answers a request with [`Kernel::offer`] (or, if it is wiped, [`Kernel::handover`]);
//! the requester takes it with [`Kernel::accept`]. The relay path uses
//! [`Outcome::outgoing`], [`Kernel::ingest`], [`Kernel::snapshot`] and
//! [`Kernel::adopt_snapshot`].
//!
//! **The relay client** (docs/reference/relay-protocol.md). The kernel keeps what a
//! relay client must remember, in its own records, so it survives restarts and
//! snapshot adoptions: the own log's next seq ([`Kernel::relay_state`]), the
//! cursors of the last clean pull ([`Kernel::relay_pulled`]), the covers recorded
//! at each own checkpoint ([`Kernel::relay_snapshot`]), and the outbox: every op
//! this device holds that the relay is not known to hold ([`Kernel::relay_outbox`]),
//! its own writes and whatever it learned over the LAN, so an op no device ever
//! uploads still reaches relay-only devices. A sync round is: pull every log from
//! the cursors, [`Kernel::ingest`] it, [`Kernel::relay_pulled`]; then upload the
//! outbox in order and [`Kernel::relay_uploaded`] each acknowledged batch; then post
//! [`Kernel::relay_forgets`] and [`Kernel::relay_forget_posted`]. A Forget record
//! is handed out only once its Forget op is on the relay, because a forgotten device
//! reads the channel frozen at its record and must find the op there to wipe.
//! [`Kernel::relay_enrollment`] gives the fields of the relay's `enroll` verb.
//!
//! **Text.** Every name and text value from the app is NFC-normalised here, so the
//! app sees exactly what was signed (dCBOR refuses non-NFC text).

#![warn(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use dcbor::{CBORCase, CBOR};
use unicode_normalization::UnicodeNormalization;

use crate::keys::{DeviceSigner, HouseholdRoot};
use crate::op::{self, Body, Hlc, Reject, Unsigned};
use crate::persist::{self, Changeset, PersistError, TAG_APP};
use crate::replica::{IngestReport, KernelError, Replica};
use crate::seal::{envelope_ref, SealKeys, SealKind};
use crate::store::OpStore;
use crate::sync::{
    AcceptReport, Adoption, Hello, Installed, Offer, PrunedContent, RebaseReport, Request, ReviewItem, Snapshot,
};
use crate::{sha256, DeviceId, Id};

pub use crate::op::Value;
pub use crate::persist::Record;
pub use crate::schema::{Collection, ContainerDef, FieldDef, Merge, Schema, ValueType};

// ---------------------------------------------------------------- plain types

/// A field name and its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// The field's name (NFC).
    pub name: String,
    /// Its value.
    pub value: Value,
}

/// A local change the app asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Write {
    /// Set some fields of a row (creating it if new).
    Put {
        /// The table (a declared `Lww` collection).
        table: String,
        /// The row id, chosen by the app.
        row: String,
        /// The fields to set; at least one.
        fields: Vec<Field>,
    },
    /// Delete a row (it observes every edit this device has seen; a concurrent edit
    /// elsewhere keeps it).
    Delete {
        /// The table.
        table: String,
        /// The row id.
        row: String,
    },
    /// Undo every delete of a row.
    Restore {
        /// The table.
        table: String,
        /// The row id.
        row: String,
    },
    /// Add an element to an add-wins set.
    SetAdd {
        /// The set (a declared `AddWinsSet`).
        set: String,
        /// The element.
        element: Value,
    },
    /// Remove an element (every add this device has seen; a concurrent add
    /// elsewhere keeps it).
    SetRemove {
        /// The set.
        set: String,
        /// The element.
        element: Value,
    },
    /// Append a record to a stream.
    Append {
        /// The stream (a declared `AppendOnly`).
        stream: String,
        /// The record.
        record: Value,
    },
}

/// What [`Kernel::open`] needs.
#[derive(Clone, Debug)]
pub struct OpenArgs {
    /// The app domain (`lullaby`, `peckish`, ...).
    pub app: String,
    /// The 64-byte household seed (the 12 words), which every device stores (Q1).
    pub seed: Vec<u8>,
    /// This device's Ed25519 public key (32 bytes). Its private half stays in the
    /// platform key store.
    pub device: Vec<u8>,
    /// The app schema (ADR 0009); its horizon configures the replica.
    pub schema: Schema,
    /// Every record the app stored for this kernel; empty on first launch. If
    /// they were made under another schema, the first outcome
    /// ([`Kernel::flush`]) sets `replace_view` and carries the whole view.
    pub records: Vec<Record>,
    /// Unix millis.
    pub now: u64,
}

/// A record the app stored, as the kernel reads it back: see [`stored_info`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredInfo {
    /// The device the records belong to (32 bytes), if any.
    pub device: Option<Vec<u8>>,
    /// The device was forgotten and wiped: delete the words, but keep these records
    /// until [`sealed_handover`]'s ops have reached the relay; then delete them.
    pub wiped: bool,
}

/// The next thing a writing call needs.
// Unboxed on purpose: a plain enum crosses the bridge as it is.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Sign these bytes with the device key (Ed25519) and pass the signature to
    /// [`Kernel::finish`].
    Sign {
        /// The exact bytes to sign.
        signable: Vec<u8>,
    },
    /// The call is complete.
    Done(Outcome),
}

/// A row as the app should now show it. `visible == false` means hide it (deleted,
/// or under a deleted container); `fields` then is empty.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowChange {
    /// The table.
    pub table: String,
    /// The row id.
    pub row: String,
    /// Show the row (with `fields`), or hide it.
    pub visible: bool,
    /// Every declared field the row has, in name order.
    pub fields: Vec<Field>,
}

/// A set element appeared or went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetChange {
    /// The set.
    pub set: String,
    /// The element.
    pub element: Value,
    /// Whether it is now in the set.
    pub present: bool,
}

/// A stream record appeared (`present`) or left the fold (its author was forgotten
/// and the record falls outside the cut).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamChange {
    /// The stream.
    pub stream: String,
    /// The record's op id (32 bytes); streams are ordered by `(millis, counter, id)`.
    pub id: Vec<u8>,
    /// The record's clock: Unix millis.
    pub millis: u64,
    /// The record's clock: counter within the millisecond.
    pub counter: u32,
    /// The device that appended it (32 bytes).
    pub device: Vec<u8>,
    /// The record.
    pub record: Value,
    /// Whether it is now in the stream.
    pub present: bool,
}

/// An op this device wrote, sealed for the relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedOp {
    /// The op id (32 bytes), which the envelope also carries in clear.
    pub id: Vec<u8>,
    /// The sealed envelope (ADR 0008).
    pub sealed: Vec<u8>,
}

/// An op refused, with a stable code (`Reject::code`, or `bad_seal` for an
/// envelope that did not open, named by the ref it claims).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rejection {
    /// The op id (32 bytes).
    pub id: Vec<u8>,
    /// Why, as a stable code.
    pub code: String,
}

/// Something the rebase did not re-apply on its own, for the app's "review these
/// edits" list. Flat so it crosses the bridge as one struct; which fields are set
/// depends on `kind`:
///
/// - `field`: someone else changed `table/row/field` since `mine` was written;
///   `current` is what it holds now.
/// - `row_deleted`: the row was deleted since the edit.
/// - `op`: a delete, restore, set remove or Forget (or an add already removed) the
///   app may redo; `op_kind` names it.
/// - `foreign`: another device's op only this device held (one entry per field for
///   a Put).
/// - `lost`: another device's field value that the old base held and the snapshot
///   lacks.
/// - `pruned`: any other op the old base held and the snapshot lacks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewEntry {
    /// Stable key: pass it to [`Kernel::dismiss_review`].
    pub key: Vec<u8>,
    /// `field`, `row_deleted`, `op`, `foreign`, `lost` or `pruned` (see above).
    pub kind: String,
    /// The op the entry is about (32 bytes).
    pub op: Vec<u8>,
    /// That op's kind: `put`, `delete`, `restore`, `set_add`, `set_remove`,
    /// `append`, `enroll`, `forget` or `checkpoint`.
    pub op_kind: String,
    /// The op's author, when it is another device.
    pub device: Option<Vec<u8>>,
    /// The table, for a row op.
    pub table: Option<String>,
    /// The row, for a row op.
    pub row: Option<String>,
    /// The field, for a field entry.
    pub field: Option<String>,
    /// The value the op wrote (field value, set element or stream record).
    pub mine: Option<Value>,
    /// What the field holds now, for a `field` entry.
    pub current: Option<Value>,
    /// The set, for a set op.
    pub set: Option<String>,
    /// The stream, for an append.
    pub stream: Option<String>,
    /// The enrolled device, for a pruned Enroll.
    pub enrolled: Option<Vec<u8>>,
}

/// What one completed call did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Records to write, in order, in one transaction (ADR 0010).
    pub records: Vec<Record>,
    /// Delete every stored record before writing `records` (a snapshot replaced
    /// the whole log).
    pub reset_records: bool,
    /// Rows to upsert or hide in the app's tables.
    pub rows: Vec<RowChange>,
    /// Set elements that appeared or went.
    pub sets: Vec<SetChange>,
    /// Stream records that appeared or went.
    pub streams: Vec<StreamChange>,
    /// Ops this device wrote in the call, sealed, for the relay.
    pub outgoing: Vec<SealedOp>,
    /// The replica's heads after the call.
    pub heads: Vec<Vec<u8>>,
    /// Ops delivered by an ingest or an accept (not the device's own writes).
    pub delivered: Vec<Vec<u8>>,
    /// Delivered but left out of the fold (their author was forgotten).
    pub excluded: Vec<Vec<u8>>,
    /// Too far in the future: retried as time passes (V8).
    pub quarantined: Vec<Vec<u8>>,
    /// Using names this app version does not declare: retried after an upgrade (V7).
    pub held: Vec<Vec<u8>>,
    /// Waiting for a parent.
    pub pending: Vec<Vec<u8>>,
    /// Ops refused for good, with their codes.
    pub rejected: Vec<Rejection>,
    /// This device was forgotten and has wiped its keys: delete the stored words.
    pub wiped: bool,
    /// Ops a rebase re-issued, and the ops that re-issued them.
    pub reissued: Vec<Reissue>,
    /// `rows`, `sets` and `streams` hold the whole view: replace the app's tables
    /// with them (after an open with a changed schema, which can reveal names the
    /// old version did not show).
    pub replace_view: bool,
    /// Entries this call added to the review list.
    pub review_added: Vec<ReviewEntry>,
}

/// An op a rebase re-issued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reissue {
    /// The old op's id (32 bytes).
    pub old: Vec<u8>,
    /// The id of the op that stands for it now.
    pub new: Vec<u8>,
}

/// The whole view, for a first paint or after a schema change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewDump {
    /// Every visible row, all visible.
    pub rows: Vec<RowChange>,
    /// Every present element.
    pub sets: Vec<SetChange>,
    /// Every record.
    pub streams: Vec<StreamChange>,
}

/// A device enrolled in the household.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Its public key (32 bytes).
    pub device: Vec<u8>,
    /// Its label, as enrolled.
    pub label: String,
    /// Someone forgot it.
    pub forgotten: bool,
    /// It is this device.
    pub me: bool,
}

/// The replica's standing, for a sync-health screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// This device's public key.
    pub device: Option<Vec<u8>>,
    /// Forgotten and wiped.
    pub wiped: bool,
    /// This device is enrolled in the state.
    pub enrolled: bool,
    /// Ops waiting for a parent.
    pub pending: u64,
    /// Ops too far in the future (V8).
    pub quarantined: u64,
    /// Ops using undeclared names (V7).
    pub held: u64,
    /// Ops refused for good.
    pub rejected: u64,
    /// The schema registered at open differs from the one the records were made with.
    pub schema_changed: bool,
    /// A writing call is waiting for [`Kernel::finish`].
    pub awaiting_signature: bool,
}

/// What [`Kernel::request`] returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestOut {
    /// Send this to the provider.
    pub message: Vec<u8>,
    /// The provider holds an old checkpoint this device lacks: this device must
    /// take the provider's offer (a snapshot) before sending anything the other way.
    pub needs_snapshot: bool,
}

/// A position in a relay log: an uploader's device and a seq in its log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayCursor {
    /// The uploader (32 bytes).
    pub device: Vec<u8>,
    /// The last seq of its log taken into account.
    pub seq: u64,
}

/// Where this device stands with the relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayState {
    /// The relay channel's generation these positions belong to (0 until the relay
    /// has named one): see [`Kernel::relay_generation`].
    pub generation: u64,
    /// The seq the next upload to this device's own log starts at.
    pub next_seq: u64,
    /// How far each log was pulled and ingested (the `pull` verb's cursors).
    pub cursors: Vec<RelayCursor>,
}

/// The fields of the relay's `enroll` verb for one device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayEnrollment {
    /// The household's enroll public key (32 bytes).
    pub household: Vec<u8>,
    /// The enrolled device (32 bytes).
    pub device: Vec<u8>,
    /// Its label, as enrolled (the relay sees it: keep labels unrevealing).
    pub label: String,
    /// The household's signature over the enrolment (64 bytes).
    pub auth: Vec<u8>,
}

/// A Forget record for the relay's `forget` verb: the Forget op's own fields, and the
/// last seq of the target's log the cut covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayForget {
    /// The Forget op's id (32 bytes): pass it to [`Kernel::relay_forget_posted`].
    pub forget: Vec<u8>,
    /// The forgotten device (32 bytes).
    pub target: Vec<u8>,
    /// The cut: the target's op ids the Forget keeps (32 bytes each, sorted).
    pub cut: Vec<Vec<u8>>,
    /// The household's signature over the Forget (64 bytes).
    pub auth: Vec<u8>,
    /// The last seq of the target's relay log that the cut covers.
    pub cut_seq: u64,
}

/// This device's base as a sealed snapshot, with what it covers on the relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelaySnapshot {
    /// The sealed snapshot ([`Kernel::snapshot`]).
    pub sealed: Vec<u8>,
    /// For each uploader, every log entry up to this seq is behind the snapshot's
    /// checkpoint. Empty when the base is another device's checkpoint, or when
    /// something was still waiting at the checkpoint: the relay then prunes nothing
    /// on this snapshot's account.
    pub covers: Vec<RelayCursor>,
}

/// Why a call failed. Nothing changed; a flow waiting for a signature still waits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// A malformed argument (wrong key length, bad app domain, ...).
    BadArgument(String),
    /// The schema is not valid.
    Schema(String),
    /// A write uses a name the schema does not declare.
    Undeclared,
    /// The stored records could not be loaded.
    Persist(String),
    /// A sync message or snapshot envelope that did not open or parse.
    BadMessage,
    /// This device has no keys (it was forgotten and wiped).
    NoKeys,
    /// The clock is far behind the log: "your clock looks wrong".
    ClockBehind {
        /// The clock passed in.
        now: u64,
        /// The newest clock in the log.
        latest: u64,
    },
    /// The kernel refused the device's own op (the code says why).
    Rejected(String),
    /// A snapshot failed verification (the code says why).
    BadSnapshot(String),
    /// No snapshot could be built for a peer that needs one (defensive, ADR 0007).
    SnapshotUnavailable,
    /// The signature does not verify under the device key.
    BadSignature,
    /// `finish` with no flow waiting.
    NothingToFinish,
    /// Another writing call is waiting for its signature.
    AwaitingSignature,
    /// Relay cursors from a pull of another channel generation than the one the
    /// positions belong to ([`Kernel::relay_pulled`]): pull again.
    StaleGeneration,
}

impl From<KernelError> for ApiError {
    fn from(e: KernelError) -> Self {
        match e {
            KernelError::NoKeys => ApiError::NoKeys,
            KernelError::ClockBehind { now, latest } => ApiError::ClockBehind { now, latest },
            KernelError::Rejected(r) => ApiError::Rejected(r.code().into()),
            KernelError::CheckpointMismatch => ApiError::BadSnapshot("checkpoint_mismatch".into()),
            KernelError::BadSnapshot(r) => ApiError::BadSnapshot(r.code().into()),
            KernelError::SnapshotUnavailable => ApiError::SnapshotUnavailable,
            KernelError::Undeclared => ApiError::Undeclared,
        }
    }
}

/// Which device stored records belong to and whether it was wiped, read without
/// keys: call it before [`Kernel::open`] to know whether to fetch the words at all.
pub fn stored_info(records: Vec<Record>) -> Option<StoredInfo> {
    let recs: Vec<(Vec<u8>, Vec<u8>)> = records.into_iter().filter_map(|r| r.value.map(|v| (r.key, v))).collect();
    persist::stored_device(&recs).map(|(d, wiped)| StoredInfo { device: d.map(|d| d.to_vec()), wiped })
}

/// WipedPush after a restart (ADR 0006, ADR 0011): the sealed ops a wiped device
/// still owes the household, read from its stored records without keys.
///
/// A wiped device has dropped the words, so after a restart it cannot open a record
/// or a sync message. The op records still carry each op's index entry in clear and
/// its envelope as first sealed, and the envelope is deterministic (ADR 0008), so
/// the device can forward, byte for byte, what it first sent: every op it wrote and
/// the past of each of its Forgets, in clock order, minus pruned bodies and
/// checkpoints older than the schema's horizon at `now` (as [`Kernel::handover`]
/// leaves them out). This is for the relay path only: a LAN peer's request does not
/// open without keys, so the list is not trimmed to what the peer lacks, and a
/// receiver simply ignores what it already has. Pass the records the app stored and
/// the app's schema. No records, or records naming no device, give an empty list.
pub fn sealed_handover(records: Vec<Record>, schema: Schema, now: u64) -> Result<Vec<SealedOp>, ApiError> {
    let recs: Vec<(Vec<u8>, Vec<u8>)> = records.into_iter().filter_map(|r| r.value.map(|v| (r.key, v))).collect();
    let Some((Some(me), _)) = persist::stored_device(&recs) else { return Ok(Vec::new()) };
    let ops = persist::stored_sealed_ops(&recs).map_err(|e| ApiError::Persist(format!("{e:?}")))?;
    let index: BTreeMap<Id, &crate::store::IndexEntry> = ops.iter().map(|(id, e, _)| (*id, e)).collect();
    let mut set: BTreeSet<Id> = BTreeSet::new();
    let mut stack: Vec<Id> = Vec::new();
    for (id, e, _) in &ops {
        if e.device == me {
            set.insert(*id);
            if e.kind == op::Kind::Forget {
                stack.push(*id);
            }
        }
    }
    // The past of each Forget: everything reachable through parents that is stored.
    let mut walked: BTreeSet<Id> = stack.iter().copied().collect();
    while let Some(id) = stack.pop() {
        set.insert(id);
        for p in index.get(&id).map(|e| e.parents.as_slice()).unwrap_or_default() {
            if index.contains_key(p) && walked.insert(*p) {
                stack.push(*p);
            }
        }
    }
    let horizon = schema.config().horizon_ms;
    let old = |e: &crate::store::IndexEntry| {
        horizon.is_some_and(|h| e.kind == op::Kind::Checkpoint && e.hlc.millis.saturating_add(h) <= now)
    };
    let mut out: Vec<(Hlc, Id, Vec<u8>)> = ops
        .into_iter()
        .filter(|(id, e, _)| set.contains(id) && !old(e))
        .filter_map(|(id, e, sealed)| sealed.map(|s| (e.hlc, id, s)))
        .collect();
    out.sort();
    Ok(out.into_iter().map(|(_, id, sealed)| SealedOp { id: id.to_vec(), sealed }).collect())
}

// ---------------------------------------------------------------- the kernel

/// Signs from a memo of signatures the app supplied; a miss records the bytes it
/// needed (and signs with zeros, which the replica then rejects, ending the run).
struct MemoSigner {
    device: DeviceId,
    memo: Arc<Mutex<Memo>>,
}

#[derive(Default)]
struct Memo {
    sigs: BTreeMap<Vec<u8>, [u8; 64]>,
    missing: Option<Vec<u8>>,
}

impl DeviceSigner for MemoSigner {
    fn device(&self) -> DeviceId {
        self.device
    }
    fn sign(&self, msg: &[u8]) -> [u8; 64] {
        let mut m = self.memo.lock().expect("memo lock");
        if let Some(s) = m.sigs.get(msg) {
            return *s;
        }
        m.missing.get_or_insert_with(|| msg.to_vec());
        [0; 64]
    }
}

/// A writing call that authors inside the replica (possibly several ops).
#[derive(Clone, Debug)]
enum Call {
    EnrollSelf { label: String },
    Enroll { device: DeviceId, label: String },
    Forget { device: DeviceId },
    ForgetSelf,
    Checkpoint,
    Accept { offer: Offer },
}

enum Flow {
    /// A local write: one op, prepared on the replica itself.
    Write { unsigned: Unsigned, now: u64 },
    /// A call replayed on a copy until every signature it needs is known.
    Replay { call: Call, now: u64, sigs: BTreeMap<Vec<u8>, [u8; 64]>, awaiting: Vec<u8> },
    /// A snapshot adoption: installed once on a copy; only its rebase is replayed.
    Rebase { installed: Box<(Replica, Adoption)>, now: u64, sigs: BTreeMap<Vec<u8>, [u8; 64]>, awaiting: Vec<u8> },
}

/// The app's view: what the app has been told, to diff against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct View {
    rows: BTreeMap<(String, String), BTreeMap<String, Value>>,
    sets: BTreeSet<(String, Value)>,
    streams: BTreeMap<(String, Hlc, Id), (DeviceId, Value)>,
    /// Container links over every row (hidden ones included): child -> container,
    /// and container -> children, so hiding can follow a deleted container down.
    parent: BTreeMap<(String, String), (String, String)>,
    children: BTreeMap<(String, String), BTreeSet<(String, String)>>,
}

/// What the relay client must remember (see the module docs), kept in api records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct RelayLedger {
    /// The channel generation the positions below belong to (0: not known yet).
    generation: u64,
    /// The own log's last acknowledged seq.
    seq: u64,
    /// The last clean pull's cursors.
    cursors: BTreeMap<DeviceId, u64>,
    /// Covers recorded at own checkpoints, by checkpoint id.
    covers: BTreeMap<Id, BTreeMap<DeviceId, u64>>,
    /// Own Forgets whose record is not posted yet, by Forget op id.
    forgets: BTreeMap<Id, PendingForget>,
}

/// An own Forget whose record the relay has not acknowledged.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PendingForget {
    target: DeviceId,
    /// The cut's seq in the target's log (for a self-Forget, set when the Forget op
    /// is acknowledged).
    cut_seq: Option<u64>,
    /// The Forget op's signed bytes. Compaction may prune its body (another
    /// device's checkpoint backs it) before the record is posted; the record and,
    /// after a generation reset, the op itself are then rebuilt from this copy.
    op: Vec<u8>,
}

/// The api record holding the [`RelayLedger`]. Review keys are 33 bytes.
const RELAY_KEY: [u8; 2] = [TAG_APP, b'R'];
/// Outbox entries: `[TAG_APP, 'O', id]`, 34 bytes, one record per op.
const OUTBOX_TAG: u8 = b'O';

fn outbox_key(id: &Id) -> Vec<u8> {
    let mut k = vec![TAG_APP, OUTBOX_TAG];
    k.extend_from_slice(id);
    k
}

/// One app's replica on this device.
pub struct Kernel {
    replica: Replica,
    seal: SealKeys,
    device: DeviceId,
    view: View,
    review: BTreeMap<Vec<u8>, ReviewEntry>,
    review_dirty: BTreeSet<Vec<u8>>,
    schema_changed: bool,
    flow: Option<Flow>,
    /// The replica's fold generation the view was last brought up to date with.
    seen_gen: u64,
    /// Whether the app has been told this device is wiped.
    told_wiped: bool,
    /// The next outcome must hand the app the whole view to replace its tables.
    replace_view: bool,
    relay: RelayLedger,
    relay_dirty: bool,
    /// Ops this device holds that the relay is not known to hold.
    outbox: BTreeSet<Id>,
    outbox_dirty: BTreeSet<Id>,
    /// The call under way takes ops from the relay (ingest, adopt_snapshot): what
    /// it delivers is on the relay already.
    from_relay: bool,
    /// Ids of ops whose signatures [`Kernel::relay_verify`] checked, for the next
    /// [`Kernel::adopt_snapshot`]. Memory only, never persisted.
    verified: BTreeSet<Id>,
}

// flutter_rust_bridge keeps `Kernel` as an opaque handle, which must be shareable.
const _: fn() = || {
    fn opaque<T: Send + Sync + 'static>() {}
    opaque::<Kernel>();
};

fn nfc(s: &str) -> String {
    s.nfc().collect()
}

fn nfc_value(v: Value) -> Value {
    match v {
        Value::Text(s) => Value::Text(nfc(&s)),
        v => v,
    }
}

fn nfc_schema(s: Schema) -> Schema {
    let f = |f: FieldDef| FieldDef { name: nfc(&f.name), ..f };
    Schema {
        collections: s
            .collections
            .into_iter()
            .map(|c| Collection {
                name: nfc(&c.name),
                merge: match c.merge {
                    Merge::Lww { fields, container } => Merge::Lww {
                        fields: fields.into_iter().map(f).collect(),
                        container: container.map(|k| ContainerDef { field: nfc(&k.field), table: nfc(&k.table) }),
                    },
                    m => m,
                },
            })
            .collect(),
        ..s
    }
}

fn b32(v: &[u8], what: &str) -> Result<[u8; 32], ApiError> {
    v.try_into().map_err(|_| ApiError::BadArgument(format!("{what} must be 32 bytes")))
}

fn ids(v: &[Id]) -> Vec<Vec<u8>> {
    v.iter().map(|i| i.to_vec()).collect()
}

impl Kernel {
    /// Open this app's replica: from the stored records, or a new one when there
    /// are none. Retries held ops against `schema`. A new device then enrols with
    /// [`Kernel::enroll_self`]. The first [`Outcome`] (from [`Kernel::flush`])
    /// carries whatever load repaired.
    pub fn open(args: OpenArgs) -> Result<Kernel, ApiError> {
        if !crate::keys::app_domain_ok(&args.app) {
            return Err(ApiError::BadArgument("app domain".into()));
        }
        let seed: [u8; 64] =
            args.seed.as_slice().try_into().map_err(|_| ApiError::BadArgument("seed must be 64 bytes".into()))?;
        let device = b32(&args.device, "device")?;
        let root = HouseholdRoot::from_seed(seed);
        let seal = SealKeys::derive(&root, &args.app);
        let schema = nfc_schema(args.schema);
        schema.validate().map_err(|e| ApiError::Schema(format!("{e:?}")))?;
        let signer = || -> Box<dyn DeviceSigner> { Box::new(MemoSigner { device, memo: Arc::default() }) };
        let mut review = BTreeMap::new();
        let mut relay = RelayLedger::default();
        let mut outbox = BTreeSet::new();
        let recs: Vec<(Vec<u8>, Vec<u8>)> =
            args.records.into_iter().filter_map(|r| r.value.map(|v| (r.key, v))).collect();
        for (k, v) in &recs {
            if k.first() != Some(&TAG_APP) {
                continue;
            }
            if k.as_slice() == RELAY_KEY {
                relay = decode_relay(v).ok_or_else(|| ApiError::Persist("relay record".into()))?;
            } else if k.len() == 34 && k[1] == OUTBOX_TAG {
                outbox.insert(<Id>::try_from(&k[2..]).expect("34-byte key"));
            } else {
                let e = decode_review(k.clone(), v).ok_or_else(|| ApiError::Persist("review record".into()))?;
                review.insert(k.clone(), e);
            }
        }
        let (mut replica, schema_changed) = if recs.is_empty() {
            (Replica::new(&args.app, root, signer(), schema.config()), false)
        } else {
            let (r, _) = Replica::load(&seal, schema.config(), Some((root, signer())), recs, args.now)
                .map_err(|e: PersistError| ApiError::Persist(format!("{e:?}")))?;
            let changed = r.schema_fp != Some(schema.fingerprint());
            (r, changed)
        };
        replica.set_schema(schema, args.now);
        let mut k = Kernel {
            replica,
            seal,
            device,
            view: View::default(),
            review,
            review_dirty: BTreeSet::new(),
            schema_changed,
            flow: None,
            seen_gen: 0,
            told_wiped: false,
            replace_view: schema_changed,
            relay,
            relay_dirty: false,
            outbox,
            outbox_dirty: BTreeSet::new(),
            from_relay: false,
            verified: BTreeSet::new(),
        };
        k.seen_gen = k.replica.fold_generation();
        k.view = k.full_view();
        // The ops delivered while loading are in the view already.
        k.replica.take_delivered();
        k.replica.take_authored();
        Ok(k)
    }

    // ------------------------------------------------------------ writing

    /// A local change. Returns the bytes to sign; [`Kernel::finish`] completes it.
    pub fn write(&mut self, write: Write, now: u64) -> Result<Step, ApiError> {
        self.idle()?;
        let body = match write {
            Write::Put { table, row, fields } => {
                if fields.is_empty() {
                    return Err(ApiError::BadArgument("a put needs a field".into()));
                }
                Body::Put {
                    table: nfc(&table),
                    row: nfc(&row),
                    fields: fields.into_iter().map(|f| (nfc(&f.name), nfc_value(f.value))).collect(),
                    origin: None,
                }
            }
            Write::Delete { table, row } => {
                let (table, row) = (nfc(&table), nfc(&row));
                let observed = self
                    .replica
                    .state()
                    .rows
                    .get(&(table.clone(), row.clone()))
                    .map(|r| r.puts.iter().copied().collect())
                    .unwrap_or_default();
                Body::Delete { table, row, observed }
            }
            Write::Restore { table, row } => {
                let (table, row) = (nfc(&table), nfc(&row));
                let observed = self
                    .replica
                    .state()
                    .rows
                    .get(&(table.clone(), row.clone()))
                    .map(|r| r.deletes.keys().filter(|d| !r.restored.contains(*d)).copied().collect())
                    .unwrap_or_default();
                Body::Restore { table, row, observed }
            }
            Write::SetAdd { set, element } => Body::SetAdd { set: nfc(&set), element: nfc_value(element) },
            Write::SetRemove { set, element } => {
                let (set, element) = (nfc(&set), nfc_value(element));
                let observed = self
                    .replica
                    .state()
                    .sets
                    .get(&(set.clone(), element.clone()))
                    .map(|e| e.adds.iter().filter(|a| !e.removed.contains(*a)).copied().collect())
                    .unwrap_or_default();
                Body::SetRemove { set, element, observed }
            }
            Write::Append { stream, record } => Body::Append { stream: nfc(&stream), record: nfc_value(record) },
        };
        let unsigned = self.replica.prepare(body, now)?;
        let signable = unsigned.signable_bytes();
        self.flow = Some(Flow::Write { unsigned, now });
        Ok(Step::Sign { signable })
    }

    /// Enrol this device (every device holds the words, so it can sign for itself).
    pub fn enroll_self(&mut self, label: String, now: u64) -> Result<Step, ApiError> {
        self.start(Call::EnrollSelf { label: nfc(&label) }, now)
    }

    /// Enrol another device (pairing): its public key and a label.
    pub fn enroll_device(&mut self, device: Vec<u8>, label: String, now: u64) -> Result<Step, ApiError> {
        let device = b32(&device, "device")?;
        self.start(Call::Enroll { device, label: nfc(&label) }, now)
    }

    /// "Forget this device": its later ops stop counting on every device.
    pub fn forget_device(&mut self, device: Vec<u8>, now: u64) -> Result<Step, ApiError> {
        let device = b32(&device, "device")?;
        self.start(Call::Forget { device }, now)
    }

    /// Forget this device itself: it wipes its keys at once (the outcome says so).
    pub fn forget_self(&mut self, now: u64) -> Result<Step, ApiError> {
        self.start(Call::ForgetSelf, now)
    }

    /// Write a checkpoint (design §4: at most weekly, after 1,000 ops or 30 days).
    pub fn checkpoint(&mut self, now: u64) -> Result<Step, ApiError> {
        self.start(Call::Checkpoint, now)
    }

    /// Take a peer's offer (from [`Kernel::offer`] or [`Kernel::handover`]). A
    /// snapshot offer may ask for several signatures (the rebase).
    pub fn accept(&mut self, offer: Vec<u8>, now: u64) -> Result<Step, ApiError> {
        self.idle()?;
        let offer = self.open_msg(&offer).and_then(|m| decode_offer(&m)).ok_or(ApiError::BadMessage)?;
        match offer {
            Offer::Ops(ops) => {
                let ingest = self.replica.ingest(&ops, now);
                Ok(Step::Done(self.outcome(Some(ingest), None, Vec::new())))
            }
            offer => self.start(Call::Accept { offer }, now),
        }
    }

    /// Adopt a snapshot fetched from the relay ([`Kernel::snapshot`] on the device
    /// that made it), with the sealed ops the relay holds above it. Ops passed to
    /// [`Kernel::relay_verify`] first skip their signature check here.
    pub fn adopt_snapshot(&mut self, snapshot: Vec<u8>, ops: Vec<Vec<u8>>, now: u64) -> Result<Step, ApiError> {
        self.idle()?;
        let snap = open_snapshot(&self.seal, &snapshot).ok_or(ApiError::BadMessage);
        let snap = match snap {
            Ok(s) => s,
            Err(e) => {
                self.verified.clear();
                return Err(e);
            }
        };
        let (ops, _) = self.open_ops(ops);
        self.from_relay = true;
        let step = self.start(Call::Accept { offer: Offer::Snapshot { snapshot: snap, ops } }, now);
        // The install (the only ingest of an adoption) is done, whatever came of it.
        self.verified.clear();
        if step.is_err() {
            self.from_relay = false;
        }
        step
    }

    /// Open sealed ops from the relay and check their signatures, ahead of the
    /// [`Kernel::adopt_snapshot`] that will take them, so a long adoption can be
    /// split: call it on small batches (yielding between them on the web, where
    /// every call holds the UI thread), then adopt with all of them. The adoption
    /// then skips those signature checks, which are most of its per-op cost; it
    /// still runs every other validity rule. Returns how many opened and verified.
    /// Changes nothing stored; the next `adopt_snapshot` forgets the results.
    pub fn relay_verify(&mut self, sealed: Vec<Vec<u8>>) -> u64 {
        let (ops, _) = self.open_ops(sealed);
        let mut n = 0;
        for b in ops {
            if let Ok((_, id)) = op::Op::decode_verified(&b) {
                self.verified.insert(id);
                n += 1;
            }
        }
        n
    }

    /// Continue the waiting call with the device's signature over the bytes it asked
    /// for.
    pub fn finish(&mut self, signature: Vec<u8>) -> Result<Step, ApiError> {
        let step = self.finish_flow(signature);
        // A flow that failed for good leaves nothing waiting: the next call is not
        // an adoption from the relay, whatever this one was.
        if step.is_err() && self.flow.is_none() {
            self.from_relay = false;
        }
        step
    }

    fn finish_flow(&mut self, signature: Vec<u8>) -> Result<Step, ApiError> {
        let sig: [u8; 64] =
            signature.as_slice().try_into().map_err(|_| ApiError::BadArgument("signature must be 64 bytes".into()))?;
        match self.flow.take() {
            None => Err(ApiError::NothingToFinish),
            Some(Flow::Write { unsigned, now }) => {
                if !op::verify_strict(&self.device, &unsigned.signable_bytes(), &sig) {
                    self.flow = Some(Flow::Write { unsigned, now });
                    return Err(ApiError::BadSignature);
                }
                self.replica.finish(unsigned, sig, now)?;
                Ok(Step::Done(self.outcome(None, None, Vec::new())))
            }
            Some(Flow::Replay { call, now, mut sigs, awaiting }) => {
                if !op::verify_strict(&self.device, &awaiting, &sig) {
                    self.flow = Some(Flow::Replay { call, now, sigs, awaiting });
                    return Err(ApiError::BadSignature);
                }
                sigs.insert(awaiting, sig);
                self.replay(call, now, sigs)
            }
            Some(Flow::Rebase { installed, now, mut sigs, awaiting }) => {
                if !op::verify_strict(&self.device, &awaiting, &sig) {
                    self.flow = Some(Flow::Rebase { installed, now, sigs, awaiting });
                    return Err(ApiError::BadSignature);
                }
                sigs.insert(awaiting, sig);
                self.replay_rebase(installed, now, sigs)
            }
        }
    }

    /// Drop a call that is waiting for a signature. Nothing it did is kept.
    pub fn abandon(&mut self) {
        self.flow = None;
        self.from_relay = false;
    }

    fn idle(&self) -> Result<(), ApiError> {
        if self.flow.is_some() {
            return Err(ApiError::AwaitingSignature);
        }
        Ok(())
    }

    fn start(&mut self, call: Call, now: u64) -> Result<Step, ApiError> {
        self.idle()?;
        match call {
            Call::Accept { offer: Offer::Snapshot { snapshot, ops } } => {
                // Verify and install once, on a copy; only the rebase is replayed.
                let mut copy = self.replica.clone_with_signer(self.signer(Arc::default()));
                match copy.adopt_install(snapshot, ops, now, &self.verified)? {
                    Installed::Done(report) => Ok(Step::Done(self.commit(copy, Some(report)))),
                    Installed::Rebase(a) => self.replay_rebase(Box::new((copy, *a)), now, BTreeMap::new()),
                }
            }
            call => self.replay(call, now, BTreeMap::new()),
        }
    }

    fn signer(&self, memo: Arc<Mutex<Memo>>) -> Box<dyn DeviceSigner> {
        Box::new(MemoSigner { device: self.device, memo })
    }

    /// Run the rebase half of an adoption on a copy of the installed replica with
    /// the signatures known so far.
    fn replay_rebase(
        &mut self,
        installed: Box<(Replica, Adoption)>,
        now: u64,
        sigs: BTreeMap<Vec<u8>, [u8; 64]>,
    ) -> Result<Step, ApiError> {
        let memo = Arc::new(Mutex::new(Memo { sigs, missing: None }));
        let mut copy = installed.0.clone_with_signer(self.signer(memo.clone()));
        let result = copy.adopt_rebase(installed.1.clone(), now);
        let (sigs, missing) = {
            let mut m = memo.lock().expect("memo lock");
            (std::mem::take(&mut m.sigs), m.missing.take())
        };
        if let Some(awaiting) = missing {
            let signable = awaiting.clone();
            self.flow = Some(Flow::Rebase { installed, now, sigs, awaiting });
            return Ok(Step::Sign { signable });
        }
        let report = result?;
        Ok(Step::Done(self.commit(copy, Some(report))))
    }

    /// A completed flow's copy becomes the replica.
    fn commit(&mut self, copy: Replica, report: Option<AcceptReport>) -> Outcome {
        // The copy signs from a memo that is now spent; give it a fresh one.
        self.replica = copy.clone_with_signer(self.signer(Arc::default()));
        let (ingest, rebase) = match report {
            Some(AcceptReport { ingest, rebase }) => (Some(ingest), rebase),
            None => (None, None),
        };
        self.outcome(ingest, rebase, Vec::new())
    }

    /// Run `call` on a copy of the replica with the signatures known so far. If it
    /// needs one more, remember where it stands and ask; if it completes, the copy
    /// becomes the replica.
    fn replay(&mut self, call: Call, now: u64, sigs: BTreeMap<Vec<u8>, [u8; 64]>) -> Result<Step, ApiError> {
        let memo = Arc::new(Mutex::new(Memo { sigs, missing: None }));
        let mut copy = self.replica.clone_with_signer(Box::new(MemoSigner { device: self.device, memo: memo.clone() }));
        let result = run(&mut copy, &call, now);
        let (sigs, missing) = {
            let mut m = memo.lock().expect("memo lock");
            (std::mem::take(&mut m.sigs), m.missing.take())
        };
        if let Some(awaiting) = missing {
            let signable = awaiting.clone();
            self.flow = Some(Flow::Replay { call, now, sigs, awaiting });
            return Ok(Step::Sign { signable });
        }
        let report = result?;
        Ok(Step::Done(self.commit(copy, report)))
    }

    // ------------------------------------------------------------ taking ops in

    /// Ops from the relay (sealed), in any order, with duplicates.
    pub fn ingest(&mut self, sealed: Vec<Vec<u8>>, now: u64) -> Result<Outcome, ApiError> {
        self.idle()?;
        let (ops, bad) = self.open_ops(sealed);
        // The relay holds whatever it delivered: none of it is this device's to upload.
        for b in &ops {
            let id = sha256(b);
            if self.outbox.remove(&id) {
                self.outbox_dirty.insert(id);
            }
        }
        self.from_relay = true;
        let rep = self.replica.ingest(&ops, now);
        Ok(self.outcome(Some(rep), None, bad))
    }

    /// Prune op bodies behind the newest checkpoint older than the horizon.
    pub fn compact(&mut self, now: u64) -> Result<Outcome, ApiError> {
        self.idle()?;
        self.replica.compact(now)?;
        Ok(self.outcome(None, None, Vec::new()))
    }

    /// Whatever changed without a call of its own (load's repairs, right after open).
    pub fn flush(&mut self) -> Outcome {
        self.outcome(None, None, Vec::new())
    }

    fn open_ops(&self, sealed: Vec<Vec<u8>>) -> (Vec<Vec<u8>>, Vec<Rejection>) {
        let mut ops = Vec::new();
        let mut bad = Vec::new();
        for env in sealed {
            match self.seal.open_op(&env) {
                Ok((_, b)) => ops.push(b),
                Err(_) => bad.push(Rejection {
                    id: envelope_ref(&env).unwrap_or_else(|| sha256(&env)).to_vec(),
                    code: "bad_seal".into(),
                }),
            }
        }
        (ops, bad)
    }

    // ------------------------------------------------------------ sync messages

    /// This device's opening message: its heads and the old checkpoints it holds.
    pub fn hello(&self, now: u64) -> Vec<u8> {
        let h = self.replica.hello(now);
        let msg = CBOR::from(vec![
            CBOR::from(0u64),
            id_list(&h.heads),
            h.base.map(CBOR::to_byte_string).unwrap_or_else(CBOR::null),
            id_list(&h.old),
        ]);
        self.seal_msg(msg)
    }

    /// Answer a peer's hello: what this device has, and which of the peer's old
    /// checkpoints it lacks.
    pub fn request(&self, hello: Vec<u8>) -> Result<RequestOut, ApiError> {
        let m = self.open_msg(&hello).ok_or(ApiError::BadMessage)?;
        let h = decode_hello(&m).ok_or(ApiError::BadMessage)?;
        let r = self.replica.request(&h);
        let needs_snapshot = !r.lacks.is_empty();
        Ok(RequestOut { message: self.encode_request(&r), needs_snapshot })
    }

    /// The ops (or the snapshot and ops) a peer's request lacks.
    pub fn offer(&self, request: Vec<u8>) -> Result<Vec<u8>, ApiError> {
        let r = self.decode_request_msg(&request)?;
        let offer = self.replica.offer(&r)?;
        Ok(self.seal_msg(encode_offer(&offer)))
    }

    /// WipedPush (ADR 0006): what a wiped device hands a peer that holds an old
    /// checkpoint it lacks: its own ops and its Forgets' past, as an offer of ops.
    pub fn handover(&self, request: Vec<u8>, now: u64) -> Result<Vec<u8>, ApiError> {
        let r = self.decode_request_msg(&request)?;
        let ops = self.replica.handover(&r.heads, now);
        Ok(self.seal_msg(encode_offer(&Offer::Ops(ops))))
    }

    /// This device's base as a sealed snapshot for the relay, if it has pruned.
    pub fn snapshot(&self) -> Option<Vec<u8>> {
        let s = self.replica.snapshot()?;
        let cp = sha256(&s.checkpoint);
        Some(self.seal.seal(SealKind::Snapshot, Some(&cp), &s.encode()))
    }

    fn seal_msg(&self, msg: CBOR) -> Vec<u8> {
        self.seal.seal(SealKind::Msg, None, &msg.to_cbor_data())
    }

    fn open_msg(&self, env: &[u8]) -> Option<CBOR> {
        let (_, pt) = self.seal.open(SealKind::Msg, env).ok()?;
        crate::cbor::decode(&pt)
    }

    fn encode_request(&self, r: &Request) -> Vec<u8> {
        self.seal_msg(CBOR::from(vec![CBOR::from(1u64), id_list(&r.heads), id_list(&r.lacks)]))
    }

    fn decode_request_msg(&self, env: &[u8]) -> Result<Request, ApiError> {
        let m = self.open_msg(env).ok_or(ApiError::BadMessage)?;
        (|| {
            let a = arr(&m)?;
            if a.len() != 3 || uint(&a[0])? != 1 {
                return None;
            }
            Some(Request { heads: parse_ids(&a[1])?, lacks: parse_ids(&a[2])? })
        })()
        .ok_or(ApiError::BadMessage)
    }

    // ------------------------------------------------------------ the relay client

    /// The own log's next seq and the cursors to pull from.
    pub fn relay_state(&self) -> RelayState {
        RelayState {
            generation: self.relay.generation,
            next_seq: self.relay.seq + 1,
            cursors: cursors(&self.relay.cursors),
        }
    }

    /// The relay named its channel's generation (in its `enroll` and `pull` answers).
    /// The first one is recorded. A different one means the channel was expired and
    /// made again, so every position is from logs that are gone: the own log starts
    /// over at seq 1, the cursors and checkpoint covers are dropped, and every op this
    /// device still holds a body for joins the outbox (the relay lacks them all).
    /// Pull again before uploading, so what other devices already re-uploaded drops
    /// out. Forget records not yet posted stay, with their `cut_seq` re-based on the
    /// new logs: a self-Forget's is set again when its Forget op is acknowledged in
    /// the new own log; another target's becomes 0, this device's cursor on that
    /// target's new log (a later cursor could cover what the target uploaded after the
    /// Forget, which the cut does not hold). The Forget op is back in the outbox, so
    /// the record waits for it as before.
    pub fn relay_generation(&mut self, generation: u64) -> Result<Outcome, ApiError> {
        self.idle()?;
        if generation != self.relay.generation {
            if self.relay.generation != 0 {
                self.relay.seq = 0;
                self.relay.cursors.clear();
                self.relay.covers.clear();
                let me = self.device;
                for f in self.relay.forgets.values_mut() {
                    f.cut_seq = (f.target != me).then_some(0);
                }
                let store = self.replica.store();
                let mut held: Vec<Id> = store.ids().into_iter().filter(|id| store.body(id).is_some()).collect();
                // A pending Forget goes up again even when its body was pruned.
                held.extend(self.relay.forgets.keys().copied());
                for id in held {
                    if self.outbox.insert(id) {
                        self.outbox_dirty.insert(id);
                    }
                }
            }
            self.relay.generation = generation;
            self.relay_dirty = true;
        }
        Ok(self.outcome(None, None, Vec::new()))
    }

    /// The first `max` ops of the outbox: every op this device holds that the relay
    /// is not known to hold, sealed, in clock order (parents first), its own writes
    /// and what it learned over the LAN. Pull before uploading, so what the relay
    /// already has drops out. Upload in this order from [`RelayState::next_seq`] and
    /// acknowledge each batch with [`Kernel::relay_uploaded`]; then ask for the next
    /// page. Only the page is sealed, so after a generation reset (when everything
    /// held is queued again) no single call seals the whole log.
    pub fn relay_outbox(&self, max: u64) -> Vec<SealedOp> {
        let mut order: Vec<(Hlc, Id)> =
            self.outbox.iter().filter_map(|id| Some((self.replica.store().entry(id)?.hlc, *id))).collect();
        order.sort();
        order
            .into_iter()
            .filter_map(|(_, id)| Some(SealedOp { id: id.to_vec(), sealed: self.seal.seal_op(self.held_body(&id)?) }))
            .take(usize::try_from(max).unwrap_or(usize::MAX))
            .collect()
    }

    /// The relay acknowledged an append of these ops (by id, in the order sent) at
    /// `first_seq` onwards: they leave the outbox and the own log's seq moves on.
    pub fn relay_uploaded(&mut self, ids: Vec<Vec<u8>>, first_seq: u64) -> Result<Outcome, ApiError> {
        self.idle()?;
        if first_seq == 0 {
            return Err(ApiError::BadArgument("seqs start at 1".into()));
        }
        let ids: Vec<Id> = ids.iter().map(|i| b32(i, "op id")).collect::<Result<_, _>>()?;
        for (i, id) in ids.iter().enumerate() {
            let seq = first_seq.saturating_add(i as u64);
            if self.outbox.remove(id) {
                self.outbox_dirty.insert(*id);
            }
            // A self-Forget's cut ends where the Forget op itself landed.
            if let Some(f) = self.relay.forgets.get_mut(id) {
                if f.target == self.device && f.cut_seq.is_none() {
                    f.cut_seq = Some(seq);
                    self.relay_dirty = true;
                }
            }
            if seq > self.relay.seq {
                self.relay.seq = seq;
                self.relay_dirty = true;
            }
        }
        Ok(self.outcome(None, None, Vec::new()))
    }

    /// Every log was pulled up to these cursors and ingested. Call it after the
    /// ingest that took them; a later checkpoint records them as its covers. The
    /// own log's cursor also moves the own seq on: an append the relay stored
    /// whose answer was lost is learned here, so the next upload continues the log.
    /// `generation` is the one the pull answers named. Cursors from another
    /// generation than [`RelayState::generation`] name logs that are gone (or not
    /// yet known): they are refused with [`ApiError::StaleGeneration`] and nothing
    /// changes; call [`Kernel::relay_generation`] and pull again.
    pub fn relay_pulled(&mut self, generation: u64, cursors: Vec<RelayCursor>) -> Result<Outcome, ApiError> {
        self.idle()?;
        if generation != self.relay.generation {
            return Err(ApiError::StaleGeneration);
        }
        let cursors: Vec<(DeviceId, u64)> = cursors
            .into_iter()
            .map(|c| Ok((b32(&c.device, "cursor device")?, c.seq)))
            .collect::<Result<_, ApiError>>()?;
        for (d, seq) in cursors {
            if d == self.device && seq > self.relay.seq {
                self.relay.seq = seq;
                self.relay_dirty = true;
            }
            let cur = self.relay.cursors.entry(d).or_insert(0);
            if seq > *cur {
                *cur = seq;
                self.relay_dirty = true;
            }
        }
        Ok(self.outcome(None, None, Vec::new()))
    }

    /// This device's base as a sealed snapshot for the relay's `snapshot` verb, with
    /// the covers recorded when this device wrote that checkpoint.
    pub fn relay_snapshot(&self) -> Option<RelaySnapshot> {
        let sealed = self.snapshot()?;
        let covers =
            self.replica.base_checkpoint().and_then(|cp| self.relay.covers.get(&cp)).map(cursors).unwrap_or_default();
        Some(RelaySnapshot { sealed, covers })
    }

    /// The relay's `enroll` fields for an enrolled device (this one, or one it paired).
    pub fn relay_enrollment(&self, device: Vec<u8>) -> Result<RelayEnrollment, ApiError> {
        let d = b32(&device, "device")?;
        let (_, enroll, label) =
            self.replica.state().enrolled.get(&d).ok_or_else(|| ApiError::BadArgument("not enrolled".into()))?;
        let auth = match self.replica.body_of(enroll).map(|o| o.body) {
            Some(Body::Enroll { auth, .. }) => auth,
            _ => self.replica.enroll_auth(&d, label)?,
        };
        Ok(RelayEnrollment {
            household: self.seal.household().to_vec(),
            device,
            label: label.clone(),
            auth: auth.to_vec(),
        })
    }

    /// The Forget records this device owes the relay: one per Forget it wrote, once
    /// that Forget op is on the relay (acknowledged, or pulled back), and until
    /// [`Kernel::relay_forget_posted`].
    pub fn relay_forgets(&self) -> Vec<RelayForget> {
        self.relay
            .forgets
            .iter()
            .filter(|(id, _)| !self.outbox.contains(*id))
            .filter_map(|(id, f)| {
                let Body::Forget { device, cut, auth } = op::decode(&f.op).ok()?.body else { return None };
                Some(RelayForget {
                    forget: id.to_vec(),
                    target: device.to_vec(),
                    cut: ids(&cut),
                    auth: auth.to_vec(),
                    cut_seq: f.cut_seq?,
                })
            })
            .collect()
    }

    /// The signed bytes of a held op: its body, or the relay ledger's copy of a
    /// pending Forget whose body was pruned.
    fn held_body(&self, id: &Id) -> Option<&[u8]> {
        self.replica.store().body(id).or_else(|| self.relay.forgets.get(id).map(|f| f.op.as_slice()))
    }

    /// The relay recorded this Forget (`["ok"]` to its `forget` post).
    pub fn relay_forget_posted(&mut self, forget: Vec<u8>) -> Result<Outcome, ApiError> {
        self.idle()?;
        if self.relay.forgets.remove(&b32(&forget, "forget id")?).is_some() {
            self.relay_dirty = true;
        }
        Ok(self.outcome(None, None, Vec::new()))
    }

    /// Bring the relay ledger up to date with a call: what it wrote and what it
    /// delivered from elsewhere than the relay join the outbox; own Forgets await
    /// their record; an own checkpoint records its covers.
    fn track_relay(&mut self, authored: &[Id], delivered: &[Id]) {
        for id in authored.iter().chain(delivered) {
            if self.outbox.insert(*id) {
                self.outbox_dirty.insert(*id);
            }
        }
        let waiting = self.replica.pending_count() + self.replica.quarantined_count() + self.replica.held_count();
        for id in authored {
            match self.replica.body_of(id).map(|o| o.body) {
                Some(Body::Forget { device, .. }) => {
                    let cut = (device != self.device).then(|| self.relay.cursors.get(&device).copied().unwrap_or(0));
                    let op = self.replica.store().body(id).expect("an authored op has a body").to_vec();
                    self.relay.forgets.insert(*id, PendingForget { target: device, cut_seq: cut, op });
                    self.relay_dirty = true;
                }
                // Covers only after a pull that left nothing waiting (relay-protocol.md,
                // pruning): the relay must not drop an entry the snapshot lacks.
                Some(Body::Checkpoint { .. }) if waiting == 0 => {
                    let mut covers = self.relay.cursors.clone();
                    if self.relay.seq > 0 {
                        let own = covers.entry(self.device).or_insert(0);
                        *own = (*own).max(self.relay.seq);
                    }
                    if !covers.is_empty() {
                        self.relay.covers.insert(*id, covers);
                        self.relay_dirty = true;
                    }
                }
                _ => {}
            }
        }
        // Ops pruned or replaced by an adoption are gone; covers of checkpoints older
        // than the base will never be asked for.
        let store = self.replica.store();
        let gone: Vec<Id> = self
            .outbox
            .iter()
            .filter(|id| store.body(id).is_none() && !self.relay.forgets.contains_key(*id))
            .copied()
            .collect();
        for id in gone {
            self.outbox.remove(&id);
            self.outbox_dirty.insert(id);
        }
        if let Some(base) = self.replica.base_checkpoint().and_then(|b| store.entry(&b)).map(|e| e.hlc) {
            let before = self.relay.covers.len();
            self.relay.covers.retain(|cp, _| store.entry(cp).is_some_and(|e| e.hlc >= base));
            self.relay_dirty |= self.relay.covers.len() != before;
        }
    }

    /// The relay ledger's records for this outcome (all of them after a reset).
    fn relay_records(&mut self, reset: bool) -> Vec<Record> {
        let mut out = Vec::new();
        let dirty: Vec<Id> = std::mem::take(&mut self.outbox_dirty).into_iter().collect();
        let relay_dirty = std::mem::take(&mut self.relay_dirty);
        let entry = || CBOR::from(vec![CBOR::from(persist::RECORD_V)]).to_cbor_data();
        if reset {
            out.push(Record { key: RELAY_KEY.to_vec(), value: Some(encode_relay(&self.relay)) });
            out.extend(self.outbox.iter().map(|id| Record { key: outbox_key(id), value: Some(entry()) }));
            return out;
        }
        if relay_dirty {
            out.push(Record { key: RELAY_KEY.to_vec(), value: Some(encode_relay(&self.relay)) });
        }
        for id in dirty {
            out.push(Record { key: outbox_key(&id), value: self.outbox.contains(&id).then(entry) });
        }
        out
    }

    // ------------------------------------------------------------ reading

    /// The whole view as the app should show it, computed afresh from the state.
    pub fn view_all(&self) -> ViewDump {
        let v = self.full_view();
        ViewDump {
            rows: v.rows.iter().map(|((t, r), f)| row_change(t, r, Some(f))).collect(),
            sets: v.sets.iter().map(|(s, e)| SetChange { set: s.clone(), element: e.clone(), present: true }).collect(),
            streams: v.streams.iter().map(|(k, d)| stream_change(k, d, true)).collect(),
        }
    }

    /// The review list, oldest decisions first by key.
    pub fn review(&self) -> Vec<ReviewEntry> {
        self.review.values().cloned().collect()
    }

    /// Remove an entry from the review list (the app redid it, or the user let it
    /// go). The returned outcome carries the record deletion.
    pub fn dismiss_review(&mut self, key: Vec<u8>) -> Outcome {
        if self.review.remove(&key).is_some() {
            self.review_dirty.insert(key);
        }
        self.outcome(None, None, Vec::new())
    }

    /// Every device ever enrolled in the household, with its label.
    pub fn devices(&self) -> Vec<DeviceInfo> {
        let st = self.replica.state();
        let forgotten = st.forgotten();
        st.enrolled
            .iter()
            .map(|(d, (_, _, label))| DeviceInfo {
                device: d.to_vec(),
                label: label.clone(),
                forgotten: forgotten.contains(d),
                me: *d == self.device,
            })
            .collect()
    }

    /// The replica's heads (32-byte op ids).
    pub fn heads(&self) -> Vec<Vec<u8>> {
        ids(&self.replica.heads())
    }

    /// Counts and flags for a sync-health screen.
    pub fn status(&self) -> Status {
        Status {
            device: self.replica.device().map(|d| d.to_vec()),
            wiped: self.replica.is_wiped(),
            enrolled: self.replica.state().is_enrolled(&self.device),
            pending: self.replica.pending_count() as u64,
            quarantined: self.replica.quarantined_count() as u64,
            held: self.replica.held_count() as u64,
            rejected: self.replica.rejected().len() as u64,
            schema_changed: self.schema_changed,
            awaiting_signature: self.flow.is_some(),
        }
    }

    // ------------------------------------------------------------ outcome and view

    fn full_view(&self) -> View {
        let st = self.replica.state();
        let schema = self.replica.schema().expect("a kernel always has a schema");
        let mut v = View::default();
        for (t, r) in st.rows.keys() {
            if schema.table(t).is_none() {
                continue;
            }
            if let Some(f) = schema.view_row(st, t, r) {
                v.rows.insert((t.clone(), r.clone()), f);
            }
            if let Some(up) = schema.container_of(st, t, r) {
                v.children.entry(up.clone()).or_default().insert((t.clone(), r.clone()));
                v.parent.insert((t.clone(), r.clone()), up);
            }
        }
        for ((s, e), el) in &st.sets {
            if matches!(
                schema.collections.iter().find(|c| c.name == *s).map(|c| &c.merge),
                Some(Merge::AddWinsSet { .. })
            ) && el.present()
            {
                v.sets.insert((s.clone(), e.clone()));
            }
        }
        for (name, recs) in &st.streams {
            if !matches!(
                schema.collections.iter().find(|c| c.name == *name).map(|c| &c.merge),
                Some(Merge::AppendOnly { .. })
            ) {
                continue;
            }
            for ((h, i), (d, rec)) in recs {
                v.streams.insert((name.clone(), *h, *i), (*d, rec.clone()));
            }
        }
        v
    }

    /// Update the view for what changed and diff it against what the app was told.
    fn view_changes(&mut self, delivered: &[Id], whole: bool) -> (Vec<RowChange>, Vec<SetChange>, Vec<StreamChange>) {
        let (mut rows, mut sets, mut streams) = (Vec::new(), Vec::new(), Vec::new());
        if whole {
            let new = self.full_view();
            let old = std::mem::replace(&mut self.view, new);
            let keys: BTreeSet<&(String, String)> = old.rows.keys().chain(self.view.rows.keys()).collect();
            for k in keys {
                let (o, n) = (old.rows.get(k), self.view.rows.get(k));
                if o != n {
                    rows.push(row_change(&k.0, &k.1, n));
                }
            }
            for k in old.sets.symmetric_difference(&self.view.sets) {
                sets.push(SetChange { set: k.0.clone(), element: k.1.clone(), present: self.view.sets.contains(k) });
            }
            for (k, d) in &old.streams {
                if !self.view.streams.contains_key(k) {
                    streams.push(stream_change(k, d, false));
                }
            }
            for (k, d) in &self.view.streams {
                if !old.streams.contains_key(k) {
                    streams.push(stream_change(k, d, true));
                }
            }
            return (rows, sets, streams);
        }
        let schema = self.replica.schema().expect("a kernel always has a schema").clone();
        let st = self.replica.state();
        let mut touched: BTreeSet<(String, String)> = BTreeSet::new();
        let mut touched_sets: BTreeSet<(String, Value)> = BTreeSet::new();
        for id in delivered {
            let Some(b) = self.replica.store().body(id).map(<[u8]>::to_vec) else { continue };
            let Ok(o) = op::decode(&b) else { continue };
            match o.body {
                Body::Put { table, row, .. } | Body::Delete { table, row, .. } | Body::Restore { table, row, .. } => {
                    touched.insert((table, row));
                }
                Body::SetAdd { set, element } | Body::SetRemove { set, element, .. } => {
                    touched_sets.insert((set, element));
                }
                Body::Append { stream, .. } => {
                    let k = (stream.clone(), o.hlc, *id);
                    let declared = matches!(
                        schema.collections.iter().find(|c| c.name == stream).map(|c| &c.merge),
                        Some(Merge::AppendOnly { .. })
                    );
                    let rec = st.streams.get(&stream).and_then(|s| s.get(&(o.hlc, *id))).cloned();
                    if let (true, Some(d)) = (declared, rec) {
                        if let std::collections::btree_map::Entry::Vacant(slot) = self.view.streams.entry(k.clone()) {
                            streams.push(stream_change(&k, &d, true));
                            slot.insert(d);
                        }
                    }
                }
                _ => {}
            }
        }
        // Container links of touched rows may have moved; then follow every touched
        // row down to everything it contains.
        for k in &touched {
            if let Some(old) = self.view.parent.remove(k) {
                if let Some(c) = self.view.children.get_mut(&old) {
                    c.remove(k);
                }
            }
            if let Some(up) = schema.container_of(st, &k.0, &k.1) {
                self.view.children.entry(up.clone()).or_default().insert(k.clone());
                self.view.parent.insert(k.clone(), up);
            }
        }
        let mut all = touched.clone();
        let mut stack: Vec<(String, String)> = touched.into_iter().collect();
        while let Some(k) = stack.pop() {
            for c in self.view.children.get(&k).into_iter().flatten() {
                if all.insert(c.clone()) {
                    stack.push(c.clone());
                }
            }
        }
        for k in all {
            let n = schema.view_row(st, &k.0, &k.1);
            if self.view.rows.get(&k) != n.as_ref() {
                rows.push(row_change(&k.0, &k.1, n.as_ref()));
                match n {
                    Some(f) => self.view.rows.insert(k, f),
                    None => self.view.rows.remove(&k),
                };
            }
        }
        for k in touched_sets {
            let declared = matches!(
                schema.collections.iter().find(|c| c.name == k.0).map(|c| &c.merge),
                Some(Merge::AddWinsSet { .. })
            );
            let present = declared && st.sets.get(&k).is_some_and(|e| e.present());
            if present != self.view.sets.contains(&k) {
                sets.push(SetChange { set: k.0.clone(), element: k.1.clone(), present });
                if present {
                    self.view.sets.insert(k);
                } else {
                    self.view.sets.remove(&k);
                }
            }
        }
        (rows, sets, streams)
    }

    fn outcome(&mut self, ingest: Option<IngestReport>, rebase: Option<RebaseReport>, bad: Vec<Rejection>) -> Outcome {
        let delivered = self.replica.take_delivered();
        let whole = self.replica.fold_generation() != self.seen_gen;
        let (mut rows, mut sets, mut streams) = self.view_changes(&delivered, whole);
        self.seen_gen = self.replica.fold_generation();
        let replace_view = std::mem::take(&mut self.replace_view);
        if replace_view {
            let all = self.view_all();
            (rows, sets, streams) = (all.rows, all.sets, all.streams);
        }
        let wiped = self.replica.is_wiped() && !self.told_wiped;
        self.told_wiped = self.replica.is_wiped();
        let authored = self.replica.take_authored();
        let rep = ingest.unwrap_or_default();
        let from_relay = std::mem::take(&mut self.from_relay);
        self.track_relay(&authored, if from_relay { &[] } else { &rep.delivered });
        let outgoing = authored
            .iter()
            .filter_map(|id| self.replica.store().body(id).map(<[u8]>::to_vec).map(|b| (id, b)))
            .map(|(id, b)| SealedOp { id: id.to_vec(), sealed: self.seal.seal_op(&b) })
            .collect();
        let mut review_added = Vec::new();
        let mut reissued = Vec::new();
        if let Some(r) = &rebase {
            reissued = r.reissued.iter().map(|(a, b)| Reissue { old: a.to_vec(), new: b.to_vec() }).collect();
            for item in &r.review {
                for e in review_entries(item) {
                    self.review_dirty.insert(e.key.clone());
                    self.review.insert(e.key.clone(), e.clone());
                    review_added.push(e);
                }
            }
        }
        let Changeset { reset, mut records } = self.replica.take_changes(&self.seal);
        let review_keys: Vec<Vec<u8>> = if reset {
            self.review_dirty.clear();
            self.review.keys().cloned().collect()
        } else {
            std::mem::take(&mut self.review_dirty).into_iter().collect()
        };
        for k in review_keys {
            let value = self.review.get(&k).map(encode_review);
            records.push(Record { key: k, value });
        }
        records.extend(self.relay_records(reset));
        let mut rejected: Vec<Rejection> =
            rep.rejected.iter().map(|(i, r)| Rejection { id: i.to_vec(), code: r.code().into() }).collect();
        rejected.extend(bad);
        Outcome {
            records,
            reset_records: reset,
            rows,
            sets,
            streams,
            outgoing,
            heads: ids(&self.replica.heads()),
            delivered: ids(&rep.delivered),
            excluded: ids(&rep.excluded),
            quarantined: ids(&rep.quarantined),
            held: ids(&rep.held),
            pending: ids(&rep.pending),
            rejected,
            wiped,
            reissued,
            replace_view,
            review_added,
        }
    }
}

// ---------------------------------------------------------------- flows

fn run(r: &mut Replica, call: &Call, now: u64) -> Result<Option<AcceptReport>, KernelError> {
    match call {
        Call::EnrollSelf { label } => r.enroll_self(label, now).map(|_| None),
        Call::Enroll { device, label } => r.enroll(*device, label, now).map(|_| None),
        Call::Forget { device } => r.forget(*device, now).map(|_| None),
        Call::ForgetSelf => r.forget_self(now).map(|_| None),
        Call::Checkpoint => r.checkpoint(now).map(|_| None),
        Call::Accept { offer } => r.accept(offer.clone(), now).map(Some),
    }
}

// ---------------------------------------------------------------- message codec

fn arr(c: &CBOR) -> Option<&Vec<CBOR>> {
    match c.as_case() {
        CBORCase::Array(a) => Some(a),
        _ => None,
    }
}

fn uint(c: &CBOR) -> Option<u64> {
    match c.as_case() {
        CBORCase::Unsigned(n) => Some(*n),
        _ => None,
    }
}

fn bytes(c: &CBOR) -> Option<Vec<u8>> {
    match c.as_case() {
        CBORCase::ByteString(b) => Some(b.data().to_vec()),
        _ => None,
    }
}

fn text(c: &CBOR) -> Option<String> {
    match c.as_case() {
        CBORCase::Text(t) => Some(t.clone()),
        _ => None,
    }
}

fn id_list(v: &[Id]) -> CBOR {
    CBOR::from(v.iter().map(CBOR::to_byte_string).collect::<Vec<_>>())
}

fn parse_ids(c: &CBOR) -> Option<Vec<Id>> {
    arr(c)?.iter().map(|x| bytes(x)?.as_slice().try_into().ok()).collect()
}

fn decode_hello(m: &CBOR) -> Option<Hello> {
    let a = arr(m)?;
    if a.len() != 4 || uint(&a[0])? != 0 {
        return None;
    }
    let base = if a[2].is_null() { None } else { Some(bytes(&a[2])?.as_slice().try_into().ok()?) };
    Some(Hello { heads: parse_ids(&a[1])?, base, old: parse_ids(&a[3])? })
}

fn encode_offer(o: &Offer) -> CBOR {
    let list = |ops: &[Vec<u8>]| CBOR::from(ops.iter().map(CBOR::to_byte_string).collect::<Vec<_>>());
    match o {
        Offer::Ops(ops) => CBOR::from(vec![CBOR::from(2u64), CBOR::null(), list(ops)]),
        Offer::Snapshot { snapshot, ops } => {
            CBOR::from(vec![CBOR::from(2u64), CBOR::to_byte_string(snapshot.encode()), list(ops)])
        }
    }
}

fn decode_offer(m: &CBOR) -> Option<Offer> {
    let a = arr(m)?;
    if a.len() != 3 || uint(&a[0])? != 2 {
        return None;
    }
    let ops: Vec<Vec<u8>> = arr(&a[2])?.iter().map(bytes).collect::<Option<_>>()?;
    if a[1].is_null() {
        return Some(Offer::Ops(ops));
    }
    let snapshot = Snapshot::decode(&bytes(&a[1])?).ok()?;
    Some(Offer::Snapshot { snapshot, ops })
}

fn open_snapshot(seal: &SealKeys, env: &[u8]) -> Option<Snapshot> {
    let (reference, pt) = seal.open(SealKind::Snapshot, env).ok()?;
    let snap = Snapshot::decode(&pt).ok()?;
    (reference == Some(sha256(&snap.checkpoint))).then_some(snap)
}

// ---------------------------------------------------------------- view helpers

fn row_change(table: &str, row: &str, fields: Option<&BTreeMap<String, Value>>) -> RowChange {
    RowChange {
        table: table.to_string(),
        row: row.to_string(),
        visible: fields.is_some(),
        fields: fields.into_iter().flatten().map(|(n, v)| Field { name: n.clone(), value: v.clone() }).collect(),
    }
}

fn stream_change(k: &(String, Hlc, Id), d: &(DeviceId, Value), present: bool) -> StreamChange {
    StreamChange {
        stream: k.0.clone(),
        id: k.2.to_vec(),
        millis: k.1.millis,
        counter: k.1.counter,
        device: d.0.to_vec(),
        record: d.1.clone(),
        present,
    }
}

// ---------------------------------------------------------------- review entries

fn kind_name(k: op::Kind) -> &'static str {
    use op::Kind::*;
    match k {
        Put => "put",
        Delete => "delete",
        Restore => "restore",
        SetAdd => "set_add",
        SetRemove => "set_remove",
        Append => "append",
        Enroll => "enroll",
        Forget => "forget",
        Checkpoint => "checkpoint",
    }
}

fn entry(kind: &str, op: &Id, op_kind: op::Kind) -> ReviewEntry {
    ReviewEntry {
        key: Vec::new(),
        kind: kind.into(),
        op: op.to_vec(),
        op_kind: kind_name(op_kind).into(),
        device: None,
        table: None,
        row: None,
        field: None,
        mine: None,
        current: None,
        set: None,
        stream: None,
        enrolled: None,
    }
}

fn keyed(mut e: ReviewEntry) -> ReviewEntry {
    e.key.clear();
    let mut key = vec![TAG_APP];
    key.extend_from_slice(&sha256(&encode_review(&e)));
    e.key = key;
    e
}

/// The review entries for one rebase item.
fn review_entries(item: &ReviewItem) -> Vec<ReviewEntry> {
    use op::Kind;
    let at = |mut e: ReviewEntry, t: &str, r: &str| {
        e.table = Some(t.into());
        e.row = Some(r.into());
        e
    };
    let out = match item {
        ReviewItem::Field { op, table, row, field, mine, current } => {
            let mut e = at(entry("field", op, Kind::Put), table, row);
            e.field = Some(field.clone());
            e.mine = Some(mine.clone());
            e.current = current.clone();
            vec![e]
        }
        ReviewItem::RowDeleted { op, table, row } => vec![at(entry("row_deleted", op, Kind::Put), table, row)],
        ReviewItem::Op { op, kind } => vec![entry("op", op, *kind)],
        ReviewItem::Foreign { op, device, body } => {
            let base = || {
                let mut e = entry("foreign", op, body.kind());
                e.device = Some(device.to_vec());
                e
            };
            match body {
                Body::Put { table, row, fields, .. } => fields
                    .iter()
                    .map(|(f, v)| {
                        let mut e = at(base(), table, row);
                        e.field = Some(f.clone());
                        e.mine = Some(v.clone());
                        e
                    })
                    .collect(),
                Body::Delete { table, row, .. } | Body::Restore { table, row, .. } => vec![at(base(), table, row)],
                Body::SetAdd { set, element } | Body::SetRemove { set, element, .. } => {
                    let mut e = base();
                    e.set = Some(set.clone());
                    e.mine = Some(element.clone());
                    vec![e]
                }
                Body::Append { stream, record } => {
                    let mut e = base();
                    e.stream = Some(stream.clone());
                    e.mine = Some(record.clone());
                    vec![e]
                }
                Body::Enroll { device: d, .. } => {
                    let mut e = base();
                    e.enrolled = Some(d.to_vec());
                    vec![e]
                }
                Body::Forget { .. } | Body::Checkpoint { .. } => vec![base()],
            }
        }
        ReviewItem::Lost { op, device, table, row, field, value } => {
            let mut e = at(entry("lost", op, Kind::Put), table, row);
            e.device = Some(device.to_vec());
            e.field = Some(field.clone());
            e.mine = Some(value.clone());
            vec![e]
        }
        ReviewItem::Pruned { op, device, content } => {
            let kind = match content {
                PrunedContent::Put { .. } => Kind::Put,
                PrunedContent::Delete { .. } => Kind::Delete,
                PrunedContent::Restore { .. } => Kind::Restore,
                PrunedContent::SetAdd { .. } => Kind::SetAdd,
                PrunedContent::SetRemove { .. } => Kind::SetRemove,
                PrunedContent::Append { .. } => Kind::Append,
                PrunedContent::Enroll { .. } => Kind::Enroll,
                PrunedContent::Unknown { kind } => *kind,
            };
            let mut e = entry("pruned", op, kind);
            e.device = Some(device.to_vec());
            match content {
                PrunedContent::Put { table, row }
                | PrunedContent::Delete { table, row, .. }
                | PrunedContent::Restore { table, row } => e = at(e, table, row),
                PrunedContent::SetAdd { set, element } | PrunedContent::SetRemove { set, element } => {
                    e.set = Some(set.clone());
                    e.mine = Some(element.clone());
                }
                PrunedContent::Append { stream, record } => {
                    e.stream = Some(stream.clone());
                    e.mine = Some(record.clone());
                }
                PrunedContent::Enroll { device } => e.enrolled = Some(device.to_vec()),
                PrunedContent::Unknown { .. } => {}
            }
            vec![e]
        }
    };
    out.into_iter().map(keyed).collect()
}

/// A review entry as a record value (its key is the record key).
fn encode_review(e: &ReviewEntry) -> Vec<u8> {
    let s = |v: &Option<String>| v.as_ref().map(|t| CBOR::from(t.as_str())).unwrap_or_else(CBOR::null);
    let b = |v: &Option<Vec<u8>>| v.as_ref().map(CBOR::to_byte_string).unwrap_or_else(CBOR::null);
    // A present value is wrapped in a one-item array, so Some(null) differs from None.
    let val = |v: &Option<Value>| v.as_ref().map(|x| CBOR::from(vec![op::value_cbor(x)])).unwrap_or_else(CBOR::null);
    CBOR::from(vec![
        CBOR::from(persist::RECORD_V),
        CBOR::from(e.kind.as_str()),
        CBOR::to_byte_string(&e.op),
        CBOR::from(e.op_kind.as_str()),
        b(&e.device),
        s(&e.table),
        s(&e.row),
        s(&e.field),
        val(&e.mine),
        val(&e.current),
        s(&e.set),
        s(&e.stream),
        b(&e.enrolled),
    ])
    .to_cbor_data()
}

fn decode_review(key: Vec<u8>, v: &[u8]) -> Option<ReviewEntry> {
    let c = crate::cbor::decode(v)?;
    let a = arr(&c)?;
    if a.len() != 13 || uint(&a[0])? != persist::RECORD_V {
        return None;
    }
    let s = |c: &CBOR| if c.is_null() { Some(None) } else { text(c).map(Some) };
    let b = |c: &CBOR| if c.is_null() { Some(None) } else { bytes(c).map(Some) };
    let val = |c: &CBOR| -> Option<Option<Value>> {
        if c.is_null() {
            return Some(None);
        }
        let x = arr(c)?;
        (x.len() == 1).then(|| op::value(&x[0]).ok()).flatten().map(Some)
    };
    Some(ReviewEntry {
        key,
        kind: text(&a[1])?,
        op: bytes(&a[2])?,
        op_kind: text(&a[3])?,
        device: b(&a[4])?,
        table: s(&a[5])?,
        row: s(&a[6])?,
        field: s(&a[7])?,
        mine: val(&a[8])?,
        current: val(&a[9])?,
        set: s(&a[10])?,
        stream: s(&a[11])?,
        enrolled: b(&a[12])?,
    })
}

// Keep `Reject` in scope for the docs above that name its codes.
#[allow(dead_code)]
fn _codes(r: Reject) -> &'static str {
    r.code()
}

fn cursors(m: &BTreeMap<DeviceId, u64>) -> Vec<RelayCursor> {
    m.iter().map(|(d, s)| RelayCursor { device: d.to_vec(), seq: *s }).collect()
}

fn pairs(m: &BTreeMap<DeviceId, u64>) -> CBOR {
    CBOR::from(m.iter().map(|(d, s)| CBOR::from(vec![CBOR::to_byte_string(d), CBOR::from(*s)])).collect::<Vec<_>>())
}

fn parse_pairs(c: &CBOR) -> Option<BTreeMap<DeviceId, u64>> {
    arr(c)?
        .iter()
        .map(|p| {
            let p = arr(p)?;
            (p.len() == 2).then_some(())?;
            Some((bytes(&p[0])?.as_slice().try_into().ok()?, uint(&p[1])?))
        })
        .collect()
}

/// `[v, seq, cursors, [[checkpoint, covers]...], [[forget, target, cut_seq | null]...]]`.
fn encode_relay(r: &RelayLedger) -> Vec<u8> {
    CBOR::from(vec![
        CBOR::from(persist::RECORD_V),
        CBOR::from(r.generation),
        CBOR::from(r.seq),
        pairs(&r.cursors),
        CBOR::from(
            r.covers.iter().map(|(cp, c)| CBOR::from(vec![CBOR::to_byte_string(cp), pairs(c)])).collect::<Vec<_>>(),
        ),
        CBOR::from(
            r.forgets
                .iter()
                .map(|(id, f)| {
                    CBOR::from(vec![
                        CBOR::to_byte_string(id),
                        CBOR::to_byte_string(f.target),
                        f.cut_seq.map(CBOR::from).unwrap_or_else(CBOR::null),
                        CBOR::to_byte_string(&f.op),
                    ])
                })
                .collect::<Vec<_>>(),
        ),
    ])
    .to_cbor_data()
}

fn decode_relay(v: &[u8]) -> Option<RelayLedger> {
    let c = crate::cbor::decode(v)?;
    let a = arr(&c)?;
    if a.len() != 6 || uint(&a[0])? != persist::RECORD_V {
        return None;
    }
    let generation = uint(&a[1])?;
    let a = &a[1..];
    let b32o = |c: &CBOR| -> Option<[u8; 32]> { bytes(c)?.as_slice().try_into().ok() };
    let covers = arr(&a[3])?
        .iter()
        .map(|e| {
            let e = arr(e)?;
            (e.len() == 2).then_some(())?;
            Some((b32o(&e[0])?, parse_pairs(&e[1])?))
        })
        .collect::<Option<_>>()?;
    let forgets = arr(&a[4])?
        .iter()
        .map(|e| {
            let e = arr(e)?;
            (e.len() == 4).then_some(())?;
            let cut_seq = if e[2].is_null() { None } else { Some(uint(&e[2])?) };
            Some((b32o(&e[0])?, PendingForget { target: b32o(&e[1])?, cut_seq, op: bytes(&e[3])? }))
        })
        .collect::<Option<_>>()?;
    Some(RelayLedger { generation, seq: uint(&a[1])?, cursors: parse_pairs(&a[2])?, covers, forgets })
}
