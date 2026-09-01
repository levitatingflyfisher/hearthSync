//! Where delivered ops live. The kernel keeps an **index** entry for every delivered
//! op forever (id, parents, device, clock, kind, target) so before(u) stays
//! computable, and the op **body** (its signed bytes) until the horizon prunes it.
//!
//! v0 ships [`MemStore`]; v1's bridge backs the same trait with Drift (and OPFS on web).

use std::collections::{BTreeMap, BTreeSet};

use dcbor::{CBORCase, CBOR};

use crate::op::{Hlc, Kind, Reject};
use crate::{DeviceId, Id};

/// What the kernel remembers about a delivered op after its body is gone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    pub parents: Vec<Id>,
    pub device: DeviceId,
    pub hlc: Hlc,
    pub kind: Kind,
    /// Row or set-element target hash (see `Body::target`), for V5.
    pub target: Option<[u8; 32]>,
    /// For an Enroll: the device it admits, for V4.
    pub enrolls: Option<DeviceId>,
}

impl IndexEntry {
    /// The index entry a delivered op leaves behind.
    pub fn of(op: &crate::op::Op) -> IndexEntry {
        use crate::op::Body;
        IndexEntry {
            parents: op.parents.clone(),
            device: op.device,
            hlc: op.hlc,
            kind: op.body.kind(),
            target: op.body.target(),
            enrolls: match &op.body {
                Body::Enroll { device, .. } => Some(*device),
                _ => None,
            },
        }
    }

    pub(crate) fn to_cbor(&self, id: &Id) -> CBOR {
        CBOR::from(vec![
            CBOR::to_byte_string(id),
            crate::op::ids_cbor(&self.parents),
            CBOR::to_byte_string(self.device),
            CBOR::from(self.hlc.millis),
            CBOR::from(self.hlc.counter as u64),
            CBOR::from(self.kind as u64),
            self.target.map(CBOR::to_byte_string).unwrap_or_else(CBOR::null),
            self.enrolls.map(CBOR::to_byte_string).unwrap_or_else(CBOR::null),
        ])
    }

    pub(crate) fn from_cbor(c: &CBOR) -> Result<(Id, IndexEntry), Reject> {
        let CBORCase::Array(a) = c.as_case() else { return Err(Reject::Schema) };
        if a.len() != 8 {
            return Err(Reject::Schema);
        }
        let b32 = |c: &CBOR| -> Result<[u8; 32], Reject> {
            match c.as_case() {
                CBORCase::ByteString(b) => b.data().try_into().map_err(|_| Reject::Schema),
                _ => Err(Reject::Schema),
            }
        };
        let opt = |c: &CBOR| -> Result<Option<[u8; 32]>, Reject> {
            if c.is_null() {
                Ok(None)
            } else {
                b32(c).map(Some)
            }
        };
        let u = |c: &CBOR| -> Result<u64, Reject> {
            match c.as_case() {
                CBORCase::Unsigned(n) => Ok(*n),
                _ => Err(Reject::Schema),
            }
        };
        let CBORCase::Array(ps) = a[1].as_case() else { return Err(Reject::Schema) };
        Ok((
            b32(&a[0])?,
            IndexEntry {
                parents: ps.iter().map(b32).collect::<Result<_, _>>()?,
                device: b32(&a[2])?,
                hlc: Hlc::new(u(&a[3])?, u32::try_from(u(&a[4])?).map_err(|_| Reject::Schema)?),
                kind: Kind::from_u64(u(&a[5])?).ok_or(Reject::Schema)?,
                target: opt(&a[6])?,
                enrolls: opt(&a[7])?,
            },
        ))
    }
}

/// Storage for delivered ops. Implementations must keep `heads` equal to the
/// delivered ops that are no delivered op's parent.
pub trait OpStore {
    fn contains(&self, id: &Id) -> bool;
    fn entry(&self, id: &Id) -> Option<&IndexEntry>;
    /// Signed bytes, or `None` if never held or pruned.
    fn body(&self, id: &Id) -> Option<&[u8]>;
    /// Record a delivered op. Its parents must already be delivered.
    fn insert(&mut self, id: Id, entry: IndexEntry, body: Option<Vec<u8>>);
    /// Drop the body, keep the index entry.
    fn prune_body(&mut self, id: &Id);
    fn ids(&self) -> Vec<Id>;
    fn heads(&self) -> Vec<Id>;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug, Default)]
pub struct MemStore {
    index: BTreeMap<Id, IndexEntry>,
    bodies: BTreeMap<Id, Vec<u8>>,
    heads: BTreeSet<Id>,
}

impl OpStore for MemStore {
    fn contains(&self, id: &Id) -> bool {
        self.index.contains_key(id)
    }
    fn entry(&self, id: &Id) -> Option<&IndexEntry> {
        self.index.get(id)
    }
    fn body(&self, id: &Id) -> Option<&[u8]> {
        self.bodies.get(id).map(Vec::as_slice)
    }
    fn insert(&mut self, id: Id, entry: IndexEntry, body: Option<Vec<u8>>) {
        if self.index.contains_key(&id) {
            if let Some(b) = body {
                self.bodies.entry(id).or_insert(b);
            }
            return;
        }
        for p in &entry.parents {
            self.heads.remove(p);
        }
        // Parents are delivered first (snapshot indexes are inserted in clock order,
        // which V3 makes topological), so nothing delivered names a new op as parent.
        self.heads.insert(id);
        self.index.insert(id, entry);
        if let Some(b) = body {
            self.bodies.insert(id, b);
        }
    }
    fn prune_body(&mut self, id: &Id) {
        self.bodies.remove(id);
    }
    fn ids(&self) -> Vec<Id> {
        self.index.keys().copied().collect()
    }
    fn heads(&self) -> Vec<Id> {
        self.heads.iter().copied().collect()
    }
    fn len(&self) -> usize {
        self.index.len()
    }
}
