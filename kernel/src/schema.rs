//! App schemas (V7) and the app's view of the state (ADR 0009).
//!
//! An app registers its collections, each with its merge: per-field LWW rows (with
//! observed-remove deletes), an add-wins set, or an append-only stream. A table may
//! name a **container**: a field holding the id of a row in another table, so
//! deleting the container hides its children (and theirs) in the view. The horizon,
//! or keeping full history, lives here too, since every replica of an app must agree
//! on it (ADR 0006, departure 8).
//!
//! **V7** runs after V1–V5, on the op alone plus the local schema:
//!
//! - A **declared** name used with the wrong kind (a Put to a set) or a value of the
//!   wrong type is **rejected** (`schema_violation`). Declared names keep their kind
//!   and type forever, so every app version that knows a name judges it alike.
//! - An **undeclared** name (table, field, set or stream) is **held**, like a
//!   quarantined op: a newer app version may declare it. The schema is local state
//!   (the app's version), and a rule that reads local state must delay, never reject
//!   (ADR 0004), or a partner one version behind would reject the upgraded device's
//!   ops, and everything built on them, for good. Held ops are retried whenever a
//!   schema is registered.
//!
//! The fold itself never reads the schema: two replicas with the same delivered set
//! have the same state whatever their app version. Only the view filters it.

#![warn(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};

use dcbor::CBOR;

use crate::op::{Body, Value};
use crate::replica::Config;
use crate::state::State;
use crate::{sha256, MAX_NAME_BYTES};

/// The type of a field, set element or stream record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ValueType {
    /// NFC text.
    Text,
    /// A signed 64-bit integer.
    Int,
    /// A boolean.
    Bool,
    /// Raw bytes.
    Bytes,
    /// Any value, null included.
    Any,
}

impl ValueType {
    fn accepts(self, v: &Value, nullable: bool) -> bool {
        match (self, v) {
            (ValueType::Any, _) => true,
            (_, Value::Null) => nullable,
            (ValueType::Text, Value::Text(_))
            | (ValueType::Int, Value::Int(_))
            | (ValueType::Bool, Value::Bool(_))
            | (ValueType::Bytes, Value::Bytes(_)) => true,
            _ => false,
        }
    }

    fn tag(self) -> u64 {
        self as u64
    }
}

/// One field of a table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDef {
    /// The field's name.
    pub name: String,
    /// The type of its values.
    pub ty: ValueType,
    /// Whether the field may be set to null (clearing it).
    pub nullable: bool,
}

/// A container relation: `field` (text) holds the id of a row of `table`. A row is
/// hidden in the view while any container above it is deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerDef {
    /// The text field that holds the container's row id.
    pub field: String,
    /// The table the container rows live in.
    pub table: String,
}

/// How a collection merges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Merge {
    /// Rows of fields, last writer wins per field; deletes are observed-remove (an
    /// edit beats a concurrent delete) and Undo restores.
    Lww {
        /// The declared fields.
        fields: Vec<FieldDef>,
        /// The container relation, if rows of this table live in another's.
        container: Option<ContainerDef>,
    },
    /// An add-wins OR-set of elements.
    AddWinsSet {
        /// The type of the elements.
        element: ValueType,
    },
    /// A grow-only stream of records.
    AppendOnly {
        /// The type of the records.
        record: ValueType,
    },
}

/// A named collection and how it merges. Names are unique across collections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collection {
    /// The collection's name (a table, set or stream).
    pub name: String,
    /// How it merges.
    pub merge: Merge,
}

/// An app's schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schema {
    /// Every collection the app declares.
    pub collections: Vec<Collection>,
    /// Op bodies behind a checkpoint older than this may be pruned.
    pub horizon_ms: u64,
    /// Never prune (Reckon, journals: ruling Q3). Overrides `horizon_ms`.
    pub keep_full_history: bool,
}

/// Why a schema is refused at registration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    /// A name is empty, longer than 128 bytes, or not NFC.
    BadName(String),
    /// Two collections, or two fields of one table, share a name.
    Duplicate(String),
    /// A container field is undeclared or not text, or its table is not a table.
    BadContainer(String),
    /// A horizon of zero with history not kept.
    BadHorizon,
}

/// V7's verdict on one op.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// The op fits the schema.
    Ok,
    /// Uses a name this schema does not declare: hold it.
    Unknown,
    /// Uses a declared name with the wrong kind or type: reject it.
    Violation,
}

impl Schema {
    /// Check the schema itself. Names must already be NFC (the api normalises them).
    pub fn validate(&self) -> Result<(), SchemaError> {
        let name_ok = |n: &str| !n.is_empty() && n.len() <= MAX_NAME_BYTES && unicode_normalization::is_nfc(n);
        let mut names = BTreeSet::new();
        for c in &self.collections {
            if !name_ok(&c.name) {
                return Err(SchemaError::BadName(c.name.clone()));
            }
            if !names.insert(c.name.as_str()) {
                return Err(SchemaError::Duplicate(c.name.clone()));
            }
            if let Merge::Lww { fields, container } = &c.merge {
                let mut fnames = BTreeSet::new();
                for f in fields {
                    if !name_ok(&f.name) {
                        return Err(SchemaError::BadName(f.name.clone()));
                    }
                    if !fnames.insert(f.name.as_str()) {
                        return Err(SchemaError::Duplicate(format!("{}.{}", c.name, f.name)));
                    }
                }
                if let Some(k) = container {
                    let field_ok = fields.iter().any(|f| f.name == k.field && f.ty == ValueType::Text);
                    let table_ok =
                        self.collections.iter().any(|t| t.name == k.table && matches!(t.merge, Merge::Lww { .. }));
                    if !field_ok || !table_ok {
                        return Err(SchemaError::BadContainer(c.name.clone()));
                    }
                }
            }
        }
        if self.horizon_ms == 0 && !self.keep_full_history {
            return Err(SchemaError::BadHorizon);
        }
        Ok(())
    }

    /// The replica configuration this schema implies.
    pub fn config(&self) -> Config {
        if self.keep_full_history {
            Config::keep_full_history()
        } else {
            Config::horizon(self.horizon_ms)
        }
    }

    fn collection(&self, name: &str) -> Option<&Merge> {
        self.collections.iter().find(|c| c.name == name).map(|c| &c.merge)
    }

    /// The fields of table `name`, if it is a declared table.
    pub fn table(&self, name: &str) -> Option<(&[FieldDef], Option<&ContainerDef>)> {
        match self.collection(name)? {
            Merge::Lww { fields, container } => Some((fields.as_slice(), container.as_ref())),
            _ => None,
        }
    }

    /// V7 on one body.
    pub fn check(&self, body: &Body) -> Check {
        let row_kind = |table: &str| match self.collection(table) {
            None => Err(Check::Unknown),
            Some(Merge::Lww { fields, .. }) => Ok(fields),
            Some(_) => Err(Check::Violation),
        };
        match body {
            Body::Put { table, fields, .. } => {
                let decl = match row_kind(table) {
                    Ok(d) => d,
                    Err(c) => return c,
                };
                let mut verdict = Check::Ok;
                for (name, v) in fields {
                    match decl.iter().find(|f| f.name == *name) {
                        None => verdict = Check::Unknown,
                        Some(f) if !f.ty.accepts(v, f.nullable) => return Check::Violation,
                        Some(_) => {}
                    }
                }
                verdict
            }
            Body::Delete { table, .. } | Body::Restore { table, .. } => match row_kind(table) {
                Ok(_) => Check::Ok,
                Err(c) => c,
            },
            Body::SetAdd { set, element } | Body::SetRemove { set, element, .. } => match self.collection(set) {
                None => Check::Unknown,
                Some(Merge::AddWinsSet { element: ty }) if ty.accepts(element, false) => Check::Ok,
                Some(_) => Check::Violation,
            },
            Body::Append { stream, record } => match self.collection(stream) {
                None => Check::Unknown,
                Some(Merge::AppendOnly { record: ty }) if ty.accepts(record, false) => Check::Ok,
                Some(_) => Check::Violation,
            },
            Body::Enroll { .. } | Body::Forget { .. } | Body::Checkpoint { .. } => Check::Ok,
        }
    }

    /// The container row of `(table, row)` in `state`, if its table has a container
    /// and the row names one.
    pub fn container_of(&self, state: &State, table: &str, row: &str) -> Option<(String, String)> {
        let (_, container) = self.table(table)?;
        let k = container?;
        let r = state.rows.get(&(table.to_string(), row.to_string()))?;
        match r.fields.get(&k.field) {
            Some((_, _, _, Value::Text(parent))) => Some((k.table.clone(), parent.clone())),
            _ => None,
        }
    }

    /// Whether the app sees row `(table, row)`: it is visible in the state, and no
    /// container above it exists and is deleted. A container that does not exist (not
    /// synced yet, or never written) hides nothing; a cycle stops the walk.
    pub fn row_visible(&self, state: &State, table: &str, row: &str) -> bool {
        let visible = |t: &str, r: &str| state.rows.get(&(t.to_string(), r.to_string())).map(|s| s.visible());
        if visible(table, row) != Some(true) {
            return false;
        }
        let mut seen = BTreeSet::from([(table.to_string(), row.to_string())]);
        let mut at = (table.to_string(), row.to_string());
        while let Some(up) = self.container_of(state, &at.0, &at.1) {
            if !seen.insert(up.clone()) {
                return true;
            }
            match visible(&up.0, &up.1) {
                Some(false) => return false,
                Some(true) => at = up,
                None => return true,
            }
        }
        true
    }

    /// The row as the app sees it: its declared fields, or `None` if it is hidden
    /// (deleted, under a deleted container, or in an undeclared table).
    pub fn view_row(&self, state: &State, table: &str, row: &str) -> Option<BTreeMap<String, Value>> {
        let (fields, _) = self.table(table)?;
        if !self.row_visible(state, table, row) {
            return None;
        }
        let r = state.rows.get(&(table.to_string(), row.to_string()))?;
        Some(
            fields
                .iter()
                .filter_map(|f| r.fields.get(&f.name).map(|(_, _, _, v)| (f.name.clone(), v.clone())))
                .collect(),
        )
    }

    /// Canonical bytes of the schema, so a change of schema can be noticed.
    pub fn encode(&self) -> Vec<u8> {
        let ty = |t: ValueType| CBOR::from(t.tag());
        let cols: Vec<CBOR> = self
            .collections
            .iter()
            .map(|c| {
                let merge = match &c.merge {
                    Merge::Lww { fields, container } => CBOR::from(vec![
                        CBOR::from(0u64),
                        CBOR::from(
                            fields
                                .iter()
                                .map(|f| {
                                    CBOR::from(vec![CBOR::from(f.name.as_str()), ty(f.ty), CBOR::from(f.nullable)])
                                })
                                .collect::<Vec<_>>(),
                        ),
                        container
                            .as_ref()
                            .map(|k| CBOR::from(vec![CBOR::from(k.field.as_str()), CBOR::from(k.table.as_str())]))
                            .unwrap_or_else(CBOR::null),
                    ]),
                    Merge::AddWinsSet { element } => CBOR::from(vec![CBOR::from(1u64), ty(*element)]),
                    Merge::AppendOnly { record } => CBOR::from(vec![CBOR::from(2u64), ty(*record)]),
                };
                CBOR::from(vec![CBOR::from(c.name.as_str()), merge])
            })
            .collect();
        CBOR::from(vec![CBOR::from(cols), CBOR::from(self.horizon_ms), CBOR::from(self.keep_full_history)])
            .to_cbor_data()
    }

    /// SHA-256 of [`Schema::encode`].
    pub fn fingerprint(&self) -> [u8; 32] {
        sha256(&self.encode())
    }
}
