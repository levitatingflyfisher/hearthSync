//! The bridge over `hearth_sync_kernel::api` (ADR 0012).
//!
//! The kernel's api is already bridge-shaped (ADR 0011): synchronous, plain types.
//! This module only tells flutter_rust_bridge about it. Every plain type is a
//! `#[frb(mirror)]` of the kernel's own type, so there is no conversion layer to
//! drift; the one opaque handle, [`HearthKernel`], wraps `Kernel` and forwards each
//! call unchanged. Every function is `#[frb(sync)]`: the web build is
//! single-threaded WASM with no worker pool (spike).

use flutter_rust_bridge::frb;

pub use hearth_sync_kernel::api::{
    ApiError, Collection, ContainerDef, DeviceInfo, Field, FieldDef, Kernel, Merge, OpenArgs, Outcome, Record, Reissue,
    Rejection, RelayCursor, RelayEnrollment, RelayForget, RelaySnapshot, RelayState, RequestOut, ReviewEntry,
    RowChange, Schema, SealedOp, SetChange, Status, Step, StoredInfo, StreamChange, ValueType, ViewDump, Write,
};
// frb reads a type named `Value` as serde_json's, so the kernel's crosses as
// `KernelValue` (the same type under another name).
pub use hearth_sync_kernel::api::Value as KernelValue;

// ------------------------------------------------------------------ mirrors

#[frb(mirror(KernelValue))]
pub enum _KernelValue {
    Null,
    Bool(bool),
    Int(i64),
    Text(String),
    Bytes(Vec<u8>),
}

#[frb(mirror(ValueType))]
pub enum _ValueType {
    Text,
    Int,
    Bool,
    Bytes,
    Any,
}

#[frb(mirror(FieldDef))]
pub struct _FieldDef {
    pub name: String,
    pub ty: ValueType,
    pub nullable: bool,
}

#[frb(mirror(ContainerDef))]
pub struct _ContainerDef {
    pub field: String,
    pub table: String,
}

#[frb(mirror(Merge))]
pub enum _Merge {
    Lww { fields: Vec<FieldDef>, container: Option<ContainerDef> },
    AddWinsSet { element: ValueType },
    AppendOnly { record: ValueType },
}

#[frb(mirror(Collection))]
pub struct _Collection {
    pub name: String,
    pub merge: Merge,
}

#[frb(mirror(Schema))]
pub struct _Schema {
    pub collections: Vec<Collection>,
    pub horizon_ms: u64,
    pub keep_full_history: bool,
}

#[frb(mirror(Field))]
pub struct _Field {
    pub name: String,
    pub value: KernelValue,
}

#[frb(mirror(Write))]
pub enum _Write {
    Put { table: String, row: String, fields: Vec<Field> },
    Delete { table: String, row: String },
    Restore { table: String, row: String },
    SetAdd { set: String, element: KernelValue },
    SetRemove { set: String, element: KernelValue },
    Append { stream: String, record: KernelValue },
}

#[frb(mirror(OpenArgs))]
pub struct _OpenArgs {
    pub app: String,
    pub seed: Vec<u8>,
    pub device: Vec<u8>,
    pub schema: Schema,
    pub records: Vec<Record>,
    pub now: u64,
}

#[frb(mirror(Record))]
pub struct _Record {
    pub key: Vec<u8>,
    pub value: Option<Vec<u8>>,
}

#[frb(mirror(StoredInfo))]
pub struct _StoredInfo {
    pub device: Option<Vec<u8>>,
    pub wiped: bool,
}

#[frb(mirror(Step))]
pub enum _Step {
    Sign { signable: Vec<u8> },
    Done(Outcome),
}

#[frb(mirror(RowChange))]
pub struct _RowChange {
    pub table: String,
    pub row: String,
    pub visible: bool,
    pub fields: Vec<Field>,
}

#[frb(mirror(SetChange))]
pub struct _SetChange {
    pub set: String,
    pub element: KernelValue,
    pub present: bool,
}

#[frb(mirror(StreamChange))]
pub struct _StreamChange {
    pub stream: String,
    pub id: Vec<u8>,
    pub millis: u64,
    pub counter: u32,
    pub device: Vec<u8>,
    pub record: KernelValue,
    pub present: bool,
}

#[frb(mirror(SealedOp))]
pub struct _SealedOp {
    pub id: Vec<u8>,
    pub sealed: Vec<u8>,
}

#[frb(mirror(Rejection))]
pub struct _Rejection {
    pub id: Vec<u8>,
    pub code: String,
}

#[frb(mirror(ReviewEntry))]
pub struct _ReviewEntry {
    pub key: Vec<u8>,
    pub kind: String,
    pub op: Vec<u8>,
    pub op_kind: String,
    pub device: Option<Vec<u8>>,
    pub table: Option<String>,
    pub row: Option<String>,
    pub field: Option<String>,
    pub mine: Option<KernelValue>,
    pub current: Option<KernelValue>,
    pub set: Option<String>,
    pub stream: Option<String>,
    pub enrolled: Option<Vec<u8>>,
}

#[frb(mirror(Outcome))]
pub struct _Outcome {
    pub records: Vec<Record>,
    pub reset_records: bool,
    pub rows: Vec<RowChange>,
    pub sets: Vec<SetChange>,
    pub streams: Vec<StreamChange>,
    pub outgoing: Vec<SealedOp>,
    pub heads: Vec<Vec<u8>>,
    pub delivered: Vec<Vec<u8>>,
    pub excluded: Vec<Vec<u8>>,
    pub quarantined: Vec<Vec<u8>>,
    pub held: Vec<Vec<u8>>,
    pub pending: Vec<Vec<u8>>,
    pub rejected: Vec<Rejection>,
    pub wiped: bool,
    pub reissued: Vec<Reissue>,
    pub replace_view: bool,
    pub review_added: Vec<ReviewEntry>,
}

#[frb(mirror(Reissue))]
pub struct _Reissue {
    pub old: Vec<u8>,
    pub new: Vec<u8>,
}

#[frb(mirror(ViewDump))]
pub struct _ViewDump {
    pub rows: Vec<RowChange>,
    pub sets: Vec<SetChange>,
    pub streams: Vec<StreamChange>,
}

#[frb(mirror(DeviceInfo))]
pub struct _DeviceInfo {
    pub device: Vec<u8>,
    pub label: String,
    pub forgotten: bool,
    pub me: bool,
}

#[frb(mirror(Status))]
pub struct _Status {
    pub device: Option<Vec<u8>>,
    pub wiped: bool,
    pub enrolled: bool,
    pub pending: u64,
    pub quarantined: u64,
    pub held: u64,
    pub rejected: u64,
    pub schema_changed: bool,
    pub awaiting_signature: bool,
}

#[frb(mirror(RequestOut))]
pub struct _RequestOut {
    pub message: Vec<u8>,
    pub needs_snapshot: bool,
}

#[frb(mirror(RelayCursor))]
pub struct _RelayCursor {
    pub device: Vec<u8>,
    pub seq: u64,
}

#[frb(mirror(RelayState))]
pub struct _RelayState {
    pub generation: u64,
    pub next_seq: u64,
    pub cursors: Vec<RelayCursor>,
}

#[frb(mirror(RelayEnrollment))]
pub struct _RelayEnrollment {
    pub household: Vec<u8>,
    pub device: Vec<u8>,
    pub label: String,
    pub auth: Vec<u8>,
}

#[frb(mirror(RelayForget))]
pub struct _RelayForget {
    pub forget: Vec<u8>,
    pub target: Vec<u8>,
    pub cut: Vec<Vec<u8>>,
    pub auth: Vec<u8>,
    pub cut_seq: u64,
}

#[frb(mirror(RelaySnapshot))]
pub struct _RelaySnapshot {
    pub sealed: Vec<u8>,
    pub covers: Vec<RelayCursor>,
}

#[frb(mirror(ApiError))]
pub enum _ApiError {
    BadArgument(String),
    Schema(String),
    Undeclared,
    Persist(String),
    BadMessage,
    NoKeys,
    ClockBehind { now: u64, latest: u64 },
    Rejected(String),
    BadSnapshot(String),
    SnapshotUnavailable,
    BadSignature,
    NothingToFinish,
    AwaitingSignature,
}

// ------------------------------------------------------------------ free functions

/// Which device stored records belong to and whether it was wiped, without keys.
#[frb(sync)]
pub fn stored_info(records: Vec<Record>) -> Option<StoredInfo> {
    hearth_sync_kernel::api::stored_info(records)
}

/// WipedPush after a restart: a wiped device's own sealed ops and its Forgets'
/// past, read from its records without keys, for the relay.
#[frb(sync)]
pub fn sealed_handover(records: Vec<Record>, schema: Schema, now: u64) -> Result<Vec<SealedOp>, ApiError> {
    hearth_sync_kernel::api::sealed_handover(records, schema, now)
}

// ------------------------------------------------------------------ the handle

/// One app's replica on this device (`hearth_sync_kernel::api::Kernel`).
#[frb(opaque)]
pub struct HearthKernel {
    k: Kernel,
}

impl HearthKernel {
    /// See `Kernel::open`.
    #[frb(sync)]
    pub fn open(args: OpenArgs) -> Result<HearthKernel, ApiError> {
        Kernel::open(args).map(|k| HearthKernel { k })
    }

    /// See `Kernel::write`.
    #[frb(sync)]
    pub fn write(&mut self, write: Write, now: u64) -> Result<Step, ApiError> {
        self.k.write(write, now)
    }

    /// See `Kernel::enroll_self`.
    #[frb(sync)]
    pub fn enroll_self(&mut self, label: String, now: u64) -> Result<Step, ApiError> {
        self.k.enroll_self(label, now)
    }

    /// See `Kernel::enroll_device`.
    #[frb(sync)]
    pub fn enroll_device(&mut self, device: Vec<u8>, label: String, now: u64) -> Result<Step, ApiError> {
        self.k.enroll_device(device, label, now)
    }

    /// See `Kernel::forget_device`.
    #[frb(sync)]
    pub fn forget_device(&mut self, device: Vec<u8>, now: u64) -> Result<Step, ApiError> {
        self.k.forget_device(device, now)
    }

    /// See `Kernel::forget_self`.
    #[frb(sync)]
    pub fn forget_self(&mut self, now: u64) -> Result<Step, ApiError> {
        self.k.forget_self(now)
    }

    /// See `Kernel::checkpoint`.
    #[frb(sync)]
    pub fn checkpoint(&mut self, now: u64) -> Result<Step, ApiError> {
        self.k.checkpoint(now)
    }

    /// See `Kernel::accept`.
    #[frb(sync)]
    pub fn accept(&mut self, offer: Vec<u8>, now: u64) -> Result<Step, ApiError> {
        self.k.accept(offer, now)
    }

    /// See `Kernel::adopt_snapshot`.
    #[frb(sync)]
    pub fn adopt_snapshot(&mut self, snapshot: Vec<u8>, ops: Vec<Vec<u8>>, now: u64) -> Result<Step, ApiError> {
        self.k.adopt_snapshot(snapshot, ops, now)
    }

    /// See `Kernel::finish`.
    #[frb(sync)]
    pub fn finish(&mut self, signature: Vec<u8>) -> Result<Step, ApiError> {
        self.k.finish(signature)
    }

    /// See `Kernel::abandon`.
    #[frb(sync)]
    pub fn abandon(&mut self) {
        self.k.abandon()
    }

    /// See `Kernel::ingest`.
    #[frb(sync)]
    pub fn ingest(&mut self, sealed: Vec<Vec<u8>>, now: u64) -> Result<Outcome, ApiError> {
        self.k.ingest(sealed, now)
    }

    /// See `Kernel::compact`.
    #[frb(sync)]
    pub fn compact(&mut self, now: u64) -> Result<Outcome, ApiError> {
        self.k.compact(now)
    }

    /// See `Kernel::flush`.
    #[frb(sync)]
    pub fn flush(&mut self) -> Outcome {
        self.k.flush()
    }

    /// See `Kernel::hello`.
    #[frb(sync)]
    pub fn hello(&self, now: u64) -> Vec<u8> {
        self.k.hello(now)
    }

    /// See `Kernel::request`.
    #[frb(sync)]
    pub fn request(&self, hello: Vec<u8>) -> Result<RequestOut, ApiError> {
        self.k.request(hello)
    }

    /// See `Kernel::offer`.
    #[frb(sync)]
    pub fn offer(&self, request: Vec<u8>) -> Result<Vec<u8>, ApiError> {
        self.k.offer(request)
    }

    /// See `Kernel::handover`.
    #[frb(sync)]
    pub fn handover(&self, request: Vec<u8>, now: u64) -> Result<Vec<u8>, ApiError> {
        self.k.handover(request, now)
    }

    /// See `Kernel::snapshot`.
    #[frb(sync)]
    pub fn snapshot(&self) -> Option<Vec<u8>> {
        self.k.snapshot()
    }

    /// See `Kernel::relay_state`.
    #[frb(sync)]
    pub fn relay_state(&self) -> RelayState {
        self.k.relay_state()
    }

    /// See `Kernel::relay_outbox`.
    #[frb(sync)]
    pub fn relay_outbox(&self) -> Vec<SealedOp> {
        self.k.relay_outbox()
    }

    /// See `Kernel::relay_uploaded`.
    #[frb(sync)]
    pub fn relay_uploaded(&mut self, ids: Vec<Vec<u8>>, first_seq: u64) -> Result<Outcome, ApiError> {
        self.k.relay_uploaded(ids, first_seq)
    }

    /// See `Kernel::relay_pulled`.
    #[frb(sync)]
    pub fn relay_pulled(&mut self, cursors: Vec<RelayCursor>) -> Result<Outcome, ApiError> {
        self.k.relay_pulled(cursors)
    }

    /// See `Kernel::relay_generation`.
    #[frb(sync)]
    pub fn relay_generation(&mut self, generation: u64) -> Result<Outcome, ApiError> {
        self.k.relay_generation(generation)
    }

    /// See `Kernel::relay_snapshot`.
    #[frb(sync)]
    pub fn relay_snapshot(&self) -> Option<RelaySnapshot> {
        self.k.relay_snapshot()
    }

    /// See `Kernel::relay_enrollment`.
    #[frb(sync)]
    pub fn relay_enrollment(&self, device: Vec<u8>) -> Result<RelayEnrollment, ApiError> {
        self.k.relay_enrollment(device)
    }

    /// See `Kernel::relay_forgets`.
    #[frb(sync)]
    pub fn relay_forgets(&self) -> Vec<RelayForget> {
        self.k.relay_forgets()
    }

    /// See `Kernel::relay_forget_posted`.
    #[frb(sync)]
    pub fn relay_forget_posted(&mut self, forget: Vec<u8>) -> Result<Outcome, ApiError> {
        self.k.relay_forget_posted(forget)
    }

    /// See `Kernel::view_all`.
    #[frb(sync)]
    pub fn view_all(&self) -> ViewDump {
        self.k.view_all()
    }

    /// See `Kernel::review`.
    #[frb(sync)]
    pub fn review(&self) -> Vec<ReviewEntry> {
        self.k.review()
    }

    /// See `Kernel::dismiss_review`.
    #[frb(sync)]
    pub fn dismiss_review(&mut self, key: Vec<u8>) -> Outcome {
        self.k.dismiss_review(key)
    }

    /// See `Kernel::devices`.
    #[frb(sync)]
    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.k.devices()
    }

    /// See `Kernel::heads`.
    #[frb(sync)]
    pub fn heads(&self) -> Vec<Vec<u8>> {
        self.k.heads()
    }

    /// See `Kernel::status`.
    #[frb(sync)]
    pub fn status(&self) -> Status {
        self.k.status()
    }
}

#[frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}
