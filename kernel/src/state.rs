//! The fold. App state is a pure function of the delivered, non-excluded op set.
//!
//! Every component is a join-semilattice (set unions, and a max for LWW fields), and
//! applying an op only ever joins, so applying ops in any order gives the same state:
//! `crdt-study`'s CvRDT obligation, discharged per component (ADR 0003).
//!
//! - **Rows**: per-field LWW registers ordered by `(hlc, id)`. A row is visible iff
//!   some `Put` to it is not observed by any live (unrestored) `Delete`: a concurrent
//!   edit was not observed, so **edit beats delete** (Q2). Tombstoned rows keep their
//!   last field values, so a `Restore` (Undo) brings them back whole.
//! - **Sets**: add-wins OR-sets. An element is present iff one of its add tags is not
//!   removed.
//! - **Streams**: grow-only, ordered by `(hlc, id)`.
//! - **Enrolled / forgets**: the household membership record.

use std::collections::{BTreeMap, BTreeSet};

use dcbor::{CBORCase, Map, CBOR};

use crate::op::{self, Body, Hlc, Op, Reject, Value};
use crate::{sha256, DeviceId, Id};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowState {
    /// field -> (LWW clock, own clock, winning op id, value). The LWW clock is the
    /// op's origin if it is a re-issue, else its own clock; LWW orders by all three
    /// (ADR 0006 OriginClock).
    pub fields: BTreeMap<String, (Hlc, Hlc, Id, Value)>,
    pub puts: BTreeSet<Id>,
    /// delete op id -> the put ids it observed
    pub deletes: BTreeMap<Id, BTreeSet<Id>>,
    /// delete ids undone by a Restore
    pub restored: BTreeSet<Id>,
}

impl RowState {
    pub fn visible(&self) -> bool {
        self.puts
            .iter()
            .any(|p| !self.deletes.iter().any(|(d, observed)| !self.restored.contains(d) && observed.contains(p)))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ElementState {
    pub adds: BTreeSet<Id>,
    pub removed: BTreeSet<Id>,
}

impl ElementState {
    pub fn present(&self) -> bool {
        self.adds.iter().any(|a| !self.removed.contains(a))
    }
}

/// A stream's records: (hlc, op id) -> (author device, record).
pub type Stream = BTreeMap<(Hlc, Id), (DeviceId, Value)>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub rows: BTreeMap<(String, String), RowState>,
    pub sets: BTreeMap<(String, Value), ElementState>,
    pub streams: BTreeMap<String, Stream>,
    /// device -> (hlc, enroll op id, label); the label is LWW so it stays order-free.
    pub enrolled: BTreeMap<DeviceId, (Hlc, Id, String)>,
    /// forget op id -> (device, cut)
    pub forgets: BTreeMap<Id, (DeviceId, Vec<Id>)>,
}

impl State {
    /// Join one delivered, non-excluded op into the state.
    pub fn apply(&mut self, id: Id, op: &Op) {
        match &op.body {
            Body::Put { table, row, fields, origin } => {
                let r = self.rows.entry((table.clone(), row.clone())).or_default();
                r.puts.insert(id);
                let lww = origin.unwrap_or(op.hlc);
                for (f, v) in fields {
                    let better = r.fields.get(f).is_none_or(|(l, h, i, _)| (lww, op.hlc, id) > (*l, *h, *i));
                    if better {
                        r.fields.insert(f.clone(), (lww, op.hlc, id, v.clone()));
                    }
                }
            }
            Body::Delete { table, row, observed } => {
                let r = self.rows.entry((table.clone(), row.clone())).or_default();
                r.deletes.insert(id, observed.iter().copied().collect());
            }
            Body::Restore { table, row, observed } => {
                let r = self.rows.entry((table.clone(), row.clone())).or_default();
                r.restored.extend(observed.iter().copied());
            }
            Body::SetAdd { set, element } => {
                self.sets.entry((set.clone(), element.clone())).or_default().adds.insert(id);
            }
            Body::SetRemove { set, element, observed } => {
                self.sets.entry((set.clone(), element.clone())).or_default().removed.extend(observed.iter().copied());
            }
            Body::Append { stream, record } => {
                self.streams.entry(stream.clone()).or_default().insert((op.hlc, id), (op.device, record.clone()));
            }
            Body::Enroll { device, label, .. } => {
                let better = self.enrolled.get(device).is_none_or(|(h, i, _)| (op.hlc, id) > (*h, *i));
                if better {
                    self.enrolled.insert(*device, (op.hlc, id, label.clone()));
                }
            }
            Body::Forget { device, cut, .. } => {
                self.forgets.insert(id, (*device, cut.clone()));
            }
            Body::Checkpoint { .. } => {}
        }
    }

    /// Least upper bound of two states (used to lay a snapshot under later ops).
    pub fn join(&mut self, other: &State) {
        for (k, o) in &other.rows {
            let r = self.rows.entry(k.clone()).or_default();
            for (f, (l, h, i, v)) in &o.fields {
                if r.fields.get(f).is_none_or(|(l2, h2, i2, _)| (*l, *h, *i) > (*l2, *h2, *i2)) {
                    r.fields.insert(f.clone(), (*l, *h, *i, v.clone()));
                }
            }
            r.puts.extend(o.puts.iter().copied());
            for (d, obs) in &o.deletes {
                r.deletes.entry(*d).or_default().extend(obs.iter().copied());
            }
            r.restored.extend(o.restored.iter().copied());
        }
        for (k, o) in &other.sets {
            let e = self.sets.entry(k.clone()).or_default();
            e.adds.extend(o.adds.iter().copied());
            e.removed.extend(o.removed.iter().copied());
        }
        for (k, o) in &other.streams {
            self.streams.entry(k.clone()).or_default().extend(o.iter().map(|(a, b)| (*a, b.clone())));
        }
        for (d, (h, i, l)) in &other.enrolled {
            if self.enrolled.get(d).is_none_or(|(h2, i2, _)| (*h, *i) > (*h2, *i2)) {
                self.enrolled.insert(*d, (*h, *i, l.clone()));
            }
        }
        self.forgets.extend(other.forgets.iter().map(|(a, b)| (*a, b.clone())));
    }

    // ------------------------------------------------------------ queries

    /// Visible rows of `table`: row id -> field -> value.
    pub fn table(&self, table: &str) -> BTreeMap<String, BTreeMap<String, Value>> {
        self.rows
            .iter()
            .filter(|((t, _), r)| t == table && r.visible())
            .map(|((_, row), r)| {
                (row.clone(), r.fields.iter().map(|(f, (_, _, _, v))| (f.clone(), v.clone())).collect())
            })
            .collect()
    }

    /// A visible row's fields, or `None` if missing or deleted.
    pub fn row(&self, table: &str, row: &str) -> Option<BTreeMap<String, Value>> {
        let r = self.rows.get(&(table.to_string(), row.to_string()))?;
        r.visible().then(|| r.fields.iter().map(|(f, (_, _, _, v))| (f.clone(), v.clone())).collect())
    }

    /// Present elements of an OR-set.
    pub fn set(&self, set: &str) -> BTreeSet<Value> {
        self.sets.iter().filter(|((s, _), e)| s == set && e.present()).map(|((_, v), _)| v.clone()).collect()
    }

    /// A stream's records in `(hlc, id)` order, with their authoring device.
    pub fn stream(&self, stream: &str) -> Vec<(DeviceId, Value)> {
        self.streams.get(stream).map(|s| s.values().cloned().collect()).unwrap_or_default()
    }

    pub fn is_enrolled(&self, device: &DeviceId) -> bool {
        self.enrolled.contains_key(device)
    }

    pub fn forgotten(&self) -> BTreeSet<DeviceId> {
        self.forgets.values().map(|(d, _)| *d).collect()
    }

    // ------------------------------------------------------------ canonical encoding

    /// Canonical dCBOR of the whole CRDT state (tombstones and tags included).
    /// Two replicas with the same delivered set produce the same bytes.
    pub fn encode(&self) -> Vec<u8> {
        let ids = |s: &BTreeSet<Id>| CBOR::from(s.iter().map(CBOR::to_byte_string).collect::<Vec<_>>());
        let hlc = |h: &Hlc| CBOR::from(vec![CBOR::from(h.millis), CBOR::from(h.counter as u64)]);
        let rows: Vec<CBOR> = self
            .rows
            .iter()
            .map(|((t, r), s)| {
                let mut fields = Map::new();
                for (f, (l, h, i, v)) in &s.fields {
                    fields.insert(f.as_str(), vec![hlc(l), hlc(h), CBOR::to_byte_string(i), op::value_cbor(v)]);
                }
                let deletes: Vec<CBOR> =
                    s.deletes.iter().map(|(d, obs)| CBOR::from(vec![CBOR::to_byte_string(d), ids(obs)])).collect();
                CBOR::from(vec![
                    CBOR::from(t.as_str()),
                    CBOR::from(r.as_str()),
                    fields.into(),
                    ids(&s.puts),
                    CBOR::from(deletes),
                    ids(&s.restored),
                ])
            })
            .collect();
        let sets: Vec<CBOR> = self
            .sets
            .iter()
            .map(|((s, v), e)| {
                CBOR::from(vec![CBOR::from(s.as_str()), op::value_cbor(v), ids(&e.adds), ids(&e.removed)])
            })
            .collect();
        let streams: Vec<CBOR> = self
            .streams
            .iter()
            .map(|(name, recs)| {
                let recs: Vec<CBOR> = recs
                    .iter()
                    .map(|((h, i), (d, v))| {
                        CBOR::from(vec![hlc(h), CBOR::to_byte_string(i), CBOR::to_byte_string(d), op::value_cbor(v)])
                    })
                    .collect();
                CBOR::from(vec![CBOR::from(name.as_str()), CBOR::from(recs)])
            })
            .collect();
        let enrolled: Vec<CBOR> = self
            .enrolled
            .iter()
            .map(|(d, (h, i, l))| {
                CBOR::from(vec![CBOR::to_byte_string(d), hlc(h), CBOR::to_byte_string(i), CBOR::from(l.as_str())])
            })
            .collect();
        let forgets: Vec<CBOR> = self
            .forgets
            .iter()
            .map(|(i, (d, cut))| CBOR::from(vec![CBOR::to_byte_string(i), CBOR::to_byte_string(d), op::ids_cbor(cut)]))
            .collect();
        let mut m = Map::new();
        m.insert(0u64, rows);
        m.insert(1u64, sets);
        m.insert(2u64, streams);
        m.insert(3u64, enrolled);
        m.insert(4u64, forgets);
        m.cbor_data()
    }

    pub fn hash(&self) -> [u8; 32] {
        sha256(&self.encode())
    }

    /// Inverse of [`State::encode`]; refuses anything that does not re-encode identically.
    pub fn decode(bytes: &[u8]) -> Result<State, Reject> {
        let cbor = crate::cbor::decode(bytes).ok_or(Reject::NotCanonical)?;
        let CBORCase::Map(m) = cbor.as_case() else { return Err(Reject::Schema) };
        let arr = |c: &CBOR| -> Result<Vec<CBOR>, Reject> {
            match c.as_case() {
                CBORCase::Array(a) => Ok(a.clone()),
                _ => Err(Reject::Schema),
            }
        };
        let get = |k: u64| -> Result<Vec<CBOR>, Reject> { arr(&m.get::<u64, CBOR>(k).ok_or(Reject::Schema)?) };
        let b32 = |c: &CBOR| -> Result<[u8; 32], Reject> {
            match c.as_case() {
                CBORCase::ByteString(b) => b.data().try_into().map_err(|_| Reject::Schema),
                _ => Err(Reject::Schema),
            }
        };
        let idset = |c: &CBOR| -> Result<BTreeSet<Id>, Reject> { arr(c)?.iter().map(b32).collect() };
        let idvec = |c: &CBOR| -> Result<Vec<Id>, Reject> { arr(c)?.iter().map(b32).collect() };
        let txt = |c: &CBOR| -> Result<String, Reject> {
            match c.as_case() {
                CBORCase::Text(s) => Ok(s.clone()),
                _ => Err(Reject::Schema),
            }
        };
        let u = |c: &CBOR| -> Result<u64, Reject> {
            match c.as_case() {
                CBORCase::Unsigned(n) => Ok(*n),
                _ => Err(Reject::Schema),
            }
        };
        let hlc = |c: &CBOR| -> Result<Hlc, Reject> {
            let a = arr(c)?;
            if a.len() != 2 {
                return Err(Reject::Schema);
            }
            Ok(Hlc::new(u(&a[0])?, u32::try_from(u(&a[1])?).map_err(|_| Reject::Schema)?))
        };
        let n = |a: &Vec<CBOR>, len: usize| if a.len() == len { Ok(()) } else { Err(Reject::Schema) };

        let mut s = State::default();
        for r in get(0)? {
            let a = arr(&r)?;
            n(&a, 6)?;
            let mut row = RowState::default();
            let CBORCase::Map(fm) = a[2].as_case() else { return Err(Reject::Schema) };
            for (k, v) in fm.iter() {
                let fa = arr(v)?;
                n(&fa, 4)?;
                row.fields.insert(txt(k)?, (hlc(&fa[0])?, hlc(&fa[1])?, b32(&fa[2])?, op::value(&fa[3])?));
            }
            row.puts = idset(&a[3])?;
            for d in arr(&a[4])? {
                let da = arr(&d)?;
                n(&da, 2)?;
                row.deletes.insert(b32(&da[0])?, idset(&da[1])?);
            }
            row.restored = idset(&a[5])?;
            s.rows.insert((txt(&a[0])?, txt(&a[1])?), row);
        }
        for e in get(1)? {
            let a = arr(&e)?;
            n(&a, 4)?;
            s.sets
                .insert((txt(&a[0])?, op::value(&a[1])?), ElementState { adds: idset(&a[2])?, removed: idset(&a[3])? });
        }
        for st in get(2)? {
            let a = arr(&st)?;
            n(&a, 2)?;
            let mut recs = BTreeMap::new();
            for rec in arr(&a[1])? {
                let ra = arr(&rec)?;
                n(&ra, 4)?;
                recs.insert((hlc(&ra[0])?, b32(&ra[1])?), (b32(&ra[2])?, op::value(&ra[3])?));
            }
            s.streams.insert(txt(&a[0])?, recs);
        }
        for en in get(3)? {
            let a = arr(&en)?;
            n(&a, 4)?;
            s.enrolled.insert(b32(&a[0])?, (hlc(&a[1])?, b32(&a[2])?, txt(&a[3])?));
        }
        for f in get(4)? {
            let a = arr(&f)?;
            n(&a, 3)?;
            s.forgets.insert(b32(&a[0])?, (b32(&a[1])?, idvec(&a[2])?));
        }
        if s.encode() != bytes {
            return Err(Reject::NotCanonical);
        }
        Ok(s)
    }
}
