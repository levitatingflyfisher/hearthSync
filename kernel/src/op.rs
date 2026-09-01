//! The op: format, canonical encoding, id, signature, and the checks that depend on
//! the op alone (V1 size + canonical + closed schema, V2 signature).
//!
//! Wire shape (dCBOR map with integer keys, sorted, so key order is also wire order):
//! `{0: 1, 1: app, 2: device, 3: [parents], 4: [millis, counter], 5: body, 6: sig}`.
//! The signature covers the same map without key 6. `id = SHA-256(signed bytes)`.
//! `docs/reference/op-format.md` spells out every body; `vectors/` pins the bytes.

use std::collections::BTreeMap;

use dcbor::{CBORCase, Map, Simple, CBOR};
use ed25519_dalek::{Signature, VerifyingKey};

use crate::{sha256, DeviceId, Id, MAX_NAME_BYTES, MAX_OP_BYTES, MAX_PARENTS};

pub const FORMAT_V: u64 = 1;
const K_V: u64 = 0;
const K_APP: u64 = 1;
const K_DEVICE: u64 = 2;
const K_PARENTS: u64 = 3;
const K_HLC: u64 = 4;
const K_BODY: u64 = 5;
const K_SIG: u64 = 6;

/// Hybrid logical clock stamp. Ordered by `(millis, counter)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hlc {
    pub millis: u64,
    pub counter: u32,
}

impl Hlc {
    pub fn new(millis: u64, counter: u32) -> Self {
        Hlc { millis, counter }
    }
}

/// A field or element value. Closed set: no floats (they do not hash identically
/// across implementations), no nesting (an app that needs structure uses more fields).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Value {
    /// No value (clears a nullable field).
    Null,
    /// A boolean.
    Bool(bool),
    /// A signed 64-bit integer.
    Int(i64),
    /// NFC text.
    Text(String),
    /// Raw bytes.
    Bytes(Vec<u8>),
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Text(s.to_string())
    }
}
impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n)
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

/// What an op changes. See the design §2.3 and ADRs 0003 and 0005.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// Per-field LWW write to one row. `origin` is set only on a re-issued edit (the
    /// rebase, ADR 0006 OriginClock): the original edit's clock, strictly older than
    /// this op's own. LWW orders the write by `origin` when present, so a re-issue can
    /// never beat a newer write its original did not see.
    Put { table: String, row: String, fields: BTreeMap<String, Value>, origin: Option<Hlc> },
    /// Hide a row: every `Put` id listed must be in before(u). Edits not listed survive.
    Delete { table: String, row: String, observed: Vec<Id> },
    /// Undo: the listed `Delete` ids stop counting.
    Restore { table: String, row: String, observed: Vec<Id> },
    /// OR-set add; the add's tag is this op's id.
    SetAdd { set: String, element: Value },
    /// OR-set remove of the observed add tags. A concurrent add survives.
    SetRemove { set: String, element: Value, observed: Vec<Id> },
    /// Grow-only stream record.
    Append { stream: String, record: Value },
    /// Admit `device`; `auth` is the household enroll key's signature over
    /// `["oh-enroll/v1", app, device, label]`.
    Enroll { device: DeviceId, label: String, auth: [u8; 64] },
    /// Forget `device`: its ops outside before(cut) leave the fold everywhere.
    /// `auth` signs `["oh-forget/v1", app, device, cut]` with the enroll key.
    Forget { device: DeviceId, cut: Vec<Id>, auth: [u8; 64] },
    /// Commitment to the canonical folded state over before(this op).
    Checkpoint { state_hash: [u8; 32] },
}

/// Body kind tags, also stored in the op index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Put = 0,
    Delete = 1,
    Restore = 2,
    SetAdd = 3,
    SetRemove = 4,
    Append = 5,
    Enroll = 6,
    Forget = 7,
    Checkpoint = 8,
}

impl Kind {
    pub fn from_u64(n: u64) -> Option<Kind> {
        Some(match n {
            0 => Kind::Put,
            1 => Kind::Delete,
            2 => Kind::Restore,
            3 => Kind::SetAdd,
            4 => Kind::SetRemove,
            5 => Kind::Append,
            6 => Kind::Enroll,
            7 => Kind::Forget,
            8 => Kind::Checkpoint,
            _ => return None,
        })
    }
}

impl Body {
    pub fn kind(&self) -> Kind {
        match self {
            Body::Put { .. } => Kind::Put,
            Body::Delete { .. } => Kind::Delete,
            Body::Restore { .. } => Kind::Restore,
            Body::SetAdd { .. } => Kind::SetAdd,
            Body::SetRemove { .. } => Kind::SetRemove,
            Body::Append { .. } => Kind::Append,
            Body::Enroll { .. } => Kind::Enroll,
            Body::Forget { .. } => Kind::Forget,
            Body::Checkpoint { .. } => Kind::Checkpoint,
        }
    }

    /// The thing a body acts on, hashed, so V5 can check that an observed id acts on
    /// the same row or element without keeping bodies around.
    pub fn target(&self) -> Option<[u8; 32]> {
        match self {
            Body::Put { table, row, .. } | Body::Delete { table, row, .. } | Body::Restore { table, row, .. } => {
                Some(row_target(table, row))
            }
            Body::SetAdd { set, element } | Body::SetRemove { set, element, .. } => Some(element_target(set, element)),
            _ => None,
        }
    }
}

pub fn row_target(table: &str, row: &str) -> [u8; 32] {
    sha256(&CBOR::from(vec![CBOR::from("row"), CBOR::from(table), CBOR::from(row)]).to_cbor_data())
}

pub fn element_target(set: &str, element: &Value) -> [u8; 32] {
    sha256(&CBOR::from(vec![CBOR::from("set"), CBOR::from(set), value_cbor(element)]).to_cbor_data())
}

/// An op before signing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unsigned {
    pub app: String,
    pub device: DeviceId,
    pub parents: Vec<Id>,
    pub hlc: Hlc,
    pub body: Body,
}

/// A signed op.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op {
    pub app: String,
    pub device: DeviceId,
    /// Sorted ascending, no duplicates.
    pub parents: Vec<Id>,
    pub hlc: Hlc,
    pub body: Body,
    pub sig: [u8; 64],
}

/// Why an op was refused. The code strings are shared with the vectors and relays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Reject {
    /// V1: more than `MAX_OP_BYTES`.
    TooLarge,
    /// V1: not canonical dCBOR (unsorted keys, non-shortest ints, trailing bytes, non-NFC text...).
    NotCanonical,
    /// V1: canonical CBOR but not an op (unknown key, wrong type, bad limits, unsorted id list).
    Schema,
    /// The op belongs to another app's log.
    WrongApp,
    /// V2: signature does not verify (strict Ed25519) under `device`.
    BadSignature,
    /// V3: hlc not strictly greater than every parent's.
    ClockNotAfterParents,
    /// V4: no Enroll for this device in before(u).
    NotEnrolled,
    /// V4: an Enroll or Forget whose `auth` is not by this household's enroll key.
    BadEnrollAuth,
    /// V5: an observed / restored / cut id is not in before(u) or does not match.
    ObservedNotInPast,
    /// A parent was rejected, so this op can never be delivered.
    ParentRejected,
    /// V7: a name the app schema declares, used with the wrong kind or value type.
    SchemaViolation,
}

impl Reject {
    pub const ALL: [Reject; 11] = [
        Reject::TooLarge,
        Reject::NotCanonical,
        Reject::Schema,
        Reject::WrongApp,
        Reject::BadSignature,
        Reject::ClockNotAfterParents,
        Reject::NotEnrolled,
        Reject::BadEnrollAuth,
        Reject::ObservedNotInPast,
        Reject::ParentRejected,
        Reject::SchemaViolation,
    ];

    /// The inverse of [`Reject::code`].
    pub fn from_code(code: &str) -> Option<Reject> {
        Reject::ALL.into_iter().find(|r| r.code() == code)
    }

    pub fn code(&self) -> &'static str {
        match self {
            Reject::TooLarge => "too_large",
            Reject::NotCanonical => "not_canonical",
            Reject::Schema => "schema",
            Reject::WrongApp => "wrong_app",
            Reject::BadSignature => "bad_signature",
            Reject::ClockNotAfterParents => "clock_not_after_parents",
            Reject::NotEnrolled => "not_enrolled",
            Reject::BadEnrollAuth => "bad_enroll_auth",
            Reject::ObservedNotInPast => "observed_not_in_past",
            Reject::ParentRejected => "parent_rejected",
            Reject::SchemaViolation => "schema_violation",
        }
    }
}

// ---------------------------------------------------------------- encoding

pub(crate) fn value_cbor(v: &Value) -> CBOR {
    match v {
        Value::Null => CBOR::null(),
        Value::Bool(b) => CBOR::from(*b),
        Value::Int(n) => CBOR::from(*n),
        Value::Text(s) => CBOR::from(s.as_str()),
        Value::Bytes(b) => CBOR::to_byte_string(b),
    }
}

pub(crate) fn ids_cbor(ids: &[Id]) -> CBOR {
    let mut sorted = ids.to_vec();
    sorted.sort();
    sorted.dedup();
    CBOR::from(sorted.iter().map(CBOR::to_byte_string).collect::<Vec<_>>())
}

fn body_cbor(b: &Body) -> CBOR {
    let k = CBOR::from(b.kind() as u64);
    let items: Vec<CBOR> = match b {
        Body::Put { table, row, fields, origin } => {
            let mut m = Map::new();
            for (f, v) in fields {
                m.insert(f.as_str(), value_cbor(v));
            }
            let mut items = vec![k, table.as_str().into(), row.as_str().into(), m.into()];
            if let Some(o) = origin {
                items.push(hlc_cbor(o));
            }
            items
        }
        Body::Delete { table, row, observed } | Body::Restore { table, row, observed } => {
            vec![k, table.as_str().into(), row.as_str().into(), ids_cbor(observed)]
        }
        Body::SetAdd { set, element } => vec![k, set.as_str().into(), value_cbor(element)],
        Body::SetRemove { set, element, observed } => {
            vec![k, set.as_str().into(), value_cbor(element), ids_cbor(observed)]
        }
        Body::Append { stream, record } => vec![k, stream.as_str().into(), value_cbor(record)],
        Body::Enroll { device, label, auth } => {
            vec![k, CBOR::to_byte_string(device), label.as_str().into(), CBOR::to_byte_string(auth)]
        }
        Body::Forget { device, cut, auth } => {
            vec![k, CBOR::to_byte_string(device), ids_cbor(cut), CBOR::to_byte_string(auth)]
        }
        Body::Checkpoint { state_hash } => vec![k, CBOR::to_byte_string(state_hash)],
    };
    CBOR::from(items)
}

fn hlc_cbor(h: &Hlc) -> CBOR {
    CBOR::from(vec![CBOR::from(h.millis), CBOR::from(h.counter as u64)])
}

fn signable_map(u: &Unsigned) -> Map {
    let mut m = Map::new();
    m.insert(K_V, FORMAT_V);
    m.insert(K_APP, u.app.as_str());
    m.insert(K_DEVICE, CBOR::to_byte_string(u.device));
    m.insert(K_PARENTS, ids_cbor(&u.parents));
    m.insert(K_HLC, hlc_cbor(&u.hlc));
    m.insert(K_BODY, body_cbor(&u.body));
    m
}

impl Unsigned {
    /// The bytes the device signs.
    pub fn signable_bytes(&self) -> Vec<u8> {
        signable_map(self).cbor_data()
    }

    /// Attach a signature. Does not verify it; ingest does.
    pub fn with_sig(self, sig: [u8; 64]) -> Op {
        let mut parents = self.parents;
        parents.sort();
        parents.dedup();
        Op { app: self.app, device: self.device, parents, hlc: self.hlc, body: self.body, sig }
    }
}

impl Op {
    pub fn unsigned(&self) -> Unsigned {
        Unsigned {
            app: self.app.clone(),
            device: self.device,
            parents: self.parents.clone(),
            hlc: self.hlc,
            body: self.body.clone(),
        }
    }

    pub fn signable_bytes(&self) -> Vec<u8> {
        self.unsigned().signable_bytes()
    }

    /// Canonical signed bytes: what travels, and what the id hashes.
    pub fn encode(&self) -> Vec<u8> {
        let mut m = signable_map(&self.unsigned());
        m.insert(K_SIG, CBOR::to_byte_string(self.sig));
        m.cbor_data()
    }

    pub fn id(&self) -> Id {
        sha256(&self.encode())
    }

    /// V2: strict Ed25519 (rejects small-order keys and malleable signatures, so a
    /// Byzantine device cannot mint two ids for one op).
    pub fn signature_ok(&self) -> bool {
        verify_strict(&self.device, &self.signable_bytes(), &self.sig)
    }

    /// V1 + V2 on raw bytes: size, canonical dCBOR, closed schema, signature.
    /// Returns the op and its id.
    pub fn decode_verified(bytes: &[u8]) -> Result<(Op, Id), Reject> {
        let op = decode(bytes)?;
        if !op.signature_ok() {
            return Err(Reject::BadSignature);
        }
        Ok((op, sha256(bytes)))
    }
}

pub fn verify_strict(pk: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
    let Ok(vk) = VerifyingKey::from_bytes(pk) else {
        return false;
    };
    vk.verify_strict(msg, &Signature::from_bytes(sig)).is_ok()
}

// ---------------------------------------------------------------- decoding

type R<T> = Result<T, Reject>;

fn uint(c: &CBOR) -> R<u64> {
    match c.as_case() {
        CBORCase::Unsigned(n) => Ok(*n),
        _ => Err(Reject::Schema),
    }
}

fn text(c: &CBOR) -> R<String> {
    match c.as_case() {
        CBORCase::Text(s) => Ok(s.clone()),
        _ => Err(Reject::Schema),
    }
}

fn name(c: &CBOR) -> R<String> {
    let s = text(c)?;
    if s.is_empty() || s.len() > MAX_NAME_BYTES {
        return Err(Reject::Schema);
    }
    Ok(s)
}

fn bytes_n<const N: usize>(c: &CBOR) -> R<[u8; N]> {
    match c.as_case() {
        CBORCase::ByteString(b) => <[u8; N]>::try_from(b.data()).map_err(|_| Reject::Schema),
        _ => Err(Reject::Schema),
    }
}

fn array(c: &CBOR) -> R<&Vec<CBOR>> {
    match c.as_case() {
        CBORCase::Array(a) => Ok(a),
        _ => Err(Reject::Schema),
    }
}

/// A list of ids must be sorted ascending with no duplicates: one spelling per set.
fn id_list(c: &CBOR) -> R<Vec<Id>> {
    let ids = array(c)?.iter().map(bytes_n::<32>).collect::<R<Vec<Id>>>()?;
    if ids.windows(2).any(|w| w[0] >= w[1]) {
        return Err(Reject::Schema);
    }
    Ok(ids)
}

pub(crate) fn value(c: &CBOR) -> R<Value> {
    Ok(match c.as_case() {
        CBORCase::Simple(Simple::Null) => Value::Null,
        CBORCase::Simple(Simple::True) => Value::Bool(true),
        CBORCase::Simple(Simple::False) => Value::Bool(false),
        CBORCase::Unsigned(n) => Value::Int(i64::try_from(*n).map_err(|_| Reject::Schema)?),
        CBORCase::Negative(n) => {
            // CBOR negative n encodes -1 - n.
            let n = i64::try_from(*n).map_err(|_| Reject::Schema)?;
            Value::Int(-1 - n)
        }
        CBORCase::Text(s) => Value::Text(s.clone()),
        CBORCase::ByteString(b) => Value::Bytes(b.data().to_vec()),
        _ => return Err(Reject::Schema),
    })
}

fn hlc(c: &CBOR) -> R<Hlc> {
    let a = array(c)?;
    if a.len() != 2 {
        return Err(Reject::Schema);
    }
    Ok(Hlc::new(uint(&a[0])?, u32::try_from(uint(&a[1])?).map_err(|_| Reject::Schema)?))
}

fn body(c: &CBOR) -> R<Body> {
    let a = array(c)?;
    let kind = Kind::from_u64(uint(a.first().ok_or(Reject::Schema)?)?).ok_or(Reject::Schema)?;
    let want = match kind {
        Kind::Put | Kind::Delete | Kind::Restore | Kind::SetRemove | Kind::Enroll | Kind::Forget => 4,
        Kind::SetAdd | Kind::Append => 3,
        Kind::Checkpoint => 2,
    };
    // A Put may carry a 5th item, the origin clock of a re-issued edit.
    if a.len() != want && !(kind == Kind::Put && a.len() == 5) {
        return Err(Reject::Schema);
    }
    Ok(match kind {
        Kind::Put => {
            let CBORCase::Map(m) = a[3].as_case() else { return Err(Reject::Schema) };
            if m.is_empty() {
                return Err(Reject::Schema);
            }
            let mut fields = BTreeMap::new();
            for (k, v) in m.iter() {
                fields.insert(name(k)?, value(v)?);
            }
            let origin = a.get(4).map(hlc).transpose()?;
            Body::Put { table: name(&a[1])?, row: name(&a[2])?, fields, origin }
        }
        Kind::Delete => Body::Delete { table: name(&a[1])?, row: name(&a[2])?, observed: id_list(&a[3])? },
        Kind::Restore => Body::Restore { table: name(&a[1])?, row: name(&a[2])?, observed: id_list(&a[3])? },
        Kind::SetAdd => Body::SetAdd { set: name(&a[1])?, element: value(&a[2])? },
        Kind::SetRemove => Body::SetRemove { set: name(&a[1])?, element: value(&a[2])?, observed: id_list(&a[3])? },
        Kind::Append => Body::Append { stream: name(&a[1])?, record: value(&a[2])? },
        Kind::Enroll => Body::Enroll { device: bytes_n(&a[1])?, label: name(&a[2])?, auth: bytes_n(&a[3])? },
        Kind::Forget => Body::Forget { device: bytes_n(&a[1])?, cut: id_list(&a[2])?, auth: bytes_n(&a[3])? },
        Kind::Checkpoint => Body::Checkpoint { state_hash: bytes_n(&a[1])? },
    })
}

/// V1: size, canonical dCBOR, closed schema. Does not check the signature.
pub fn decode(bytes: &[u8]) -> R<Op> {
    if bytes.len() > MAX_OP_BYTES {
        return Err(Reject::TooLarge);
    }
    let cbor = crate::cbor::decode(bytes).ok_or(Reject::NotCanonical)?;
    // Belt and braces: re-encoding must reproduce the input byte for byte.
    if cbor.to_cbor_data() != bytes {
        return Err(Reject::NotCanonical);
    }
    let CBORCase::Map(m) = cbor.as_case() else { return Err(Reject::Schema) };
    if m.len() != 7 {
        return Err(Reject::Schema);
    }
    let mut got: [Option<&CBOR>; 7] = [None; 7];
    for (k, v) in m.iter() {
        let k = uint(k)? as usize;
        if k >= got.len() {
            return Err(Reject::Schema);
        }
        got[k] = Some(v);
    }
    let f = |k: u64| got[k as usize].ok_or(Reject::Schema);
    if uint(f(K_V)?)? != FORMAT_V {
        return Err(Reject::Schema);
    }
    let app = text(f(K_APP)?)?;
    if !crate::keys::app_domain_ok(&app) {
        return Err(Reject::Schema);
    }
    let parents = id_list(f(K_PARENTS)?)?;
    if parents.len() > MAX_PARENTS {
        return Err(Reject::Schema);
    }
    let hlc = hlc(f(K_HLC)?)?;
    let body = body(f(K_BODY)?)?;
    // A re-issue orders by its origin, so the origin must be strictly older than the
    // op itself: a device can only lower its own write's priority, never raise it.
    if let Body::Put { origin: Some(o), .. } = &body {
        if *o >= hlc {
            return Err(Reject::Schema);
        }
    }
    Ok(Op { app, device: bytes_n(f(K_DEVICE)?)?, parents, hlc, body, sig: bytes_n(f(K_SIG)?)? })
}

/// `id = SHA-256(bytes)`, only for bytes that are canonical dCBOR (the spike's rule).
pub fn op_id(bytes: &[u8]) -> Option<Id> {
    let cbor = crate::cbor::decode(bytes)?;
    (cbor.to_cbor_data() == bytes).then(|| sha256(bytes))
}
