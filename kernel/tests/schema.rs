//! App schemas (V7) and the container view (ADR 0009).

mod common;
use common::*;

use hearth_sync_kernel::op::{Reject, Value};
use hearth_sync_kernel::replica::{Config, KernelError, Replica};
use hearth_sync_kernel::schema::{Check, Collection, ContainerDef, FieldDef, Merge, Schema, SchemaError, ValueType};
use hearth_sync_kernel::sha256;
use hearth_sync_kernel::store::OpStore;
use hearth_sync_kernel::sync::reconcile;

fn field(name: &str, ty: ValueType, nullable: bool) -> FieldDef {
    FieldDef { name: name.into(), ty, nullable }
}

fn table(name: &str, fields: Vec<FieldDef>, container: Option<(&str, &str)>) -> Collection {
    Collection {
        name: name.into(),
        merge: Merge::Lww {
            fields,
            container: container.map(|(f, t)| ContainerDef { field: f.into(), table: t.into() }),
        },
    }
}

/// StillLife's shape: rooms hold containers hold items; plus a set and a stream.
fn v1() -> Schema {
    Schema {
        collections: vec![
            table("rooms", vec![field("name", ValueType::Text, false)], None),
            table(
                "boxes",
                vec![field("name", ValueType::Text, false), field("room", ValueType::Text, true)],
                Some(("room", "rooms")),
            ),
            table(
                "items",
                vec![
                    field("name", ValueType::Text, false),
                    field("box", ValueType::Text, true),
                    field("value", ValueType::Int, true),
                ],
                Some(("box", "boxes")),
            ),
            Collection { name: "tags".into(), merge: Merge::AddWinsSet { element: ValueType::Text } },
            Collection { name: "log".into(), merge: Merge::AppendOnly { record: ValueType::Any } },
        ],
        horizon_ms: 90 * DAY,
        keep_full_history: false,
    }
}

/// v2 adds a table and a field; nothing declared in v1 changes.
fn v2() -> Schema {
    let mut s = v1();
    s.collections.push(table("photos", vec![field("item", ValueType::Text, false)], None));
    if let Merge::Lww { fields, .. } = &mut s.collections[2].merge {
        fields.push(field("colour", ValueType::Text, true));
    }
    s
}

fn with(n: u8, schema: Schema) -> Replica {
    let mut r = joined_with(n, T0, schema.config());
    r.set_schema(schema, T0);
    r
}

#[test]
fn a_schema_is_checked_when_registered() {
    assert_eq!(v1().validate(), Ok(()));
    assert_eq!(v2().validate(), Ok(()));
    let mut dup = v1();
    dup.collections.push(Collection { name: "rooms".into(), merge: Merge::AppendOnly { record: ValueType::Any } });
    assert_eq!(dup.validate(), Err(SchemaError::Duplicate("rooms".into())));
    let mut bad = v1();
    bad.collections[1] = table("boxes", vec![field("room", ValueType::Int, true)], Some(("room", "rooms")));
    assert_eq!(bad.validate(), Err(SchemaError::BadContainer("boxes".into())));
    let mut bad = v1();
    bad.collections[1] = table("boxes", vec![field("room", ValueType::Text, true)], Some(("room", "tags")));
    assert_eq!(bad.validate(), Err(SchemaError::BadContainer("boxes".into())));
    let mut bad = v1();
    bad.collections[0].name = "e\u{301}".into(); // decomposed é: not NFC
    assert!(matches!(bad.validate(), Err(SchemaError::BadName(_))));
    let mut bad = v1();
    bad.horizon_ms = 0;
    assert_eq!(bad.validate(), Err(SchemaError::BadHorizon));
    bad.keep_full_history = true;
    assert_eq!(bad.validate(), Ok(()));
    assert_eq!(bad.config(), Config::keep_full_history());
    assert_eq!(v1().config(), Config::horizon(90 * DAY));
    assert_ne!(v1().fingerprint(), v2().fingerprint());
}

#[test]
fn a_declared_name_used_with_the_wrong_kind_or_type_is_rejected_everywhere_it_is_known() {
    // The writers have no schema, so they can write anything; the reader has v1.
    // One writer per op, so no bad op is another's parent.
    let mut r = with(2, v1());
    type Write = fn(&mut Replica) -> [u8; 32];
    let bad: [(&str, Write); 5] = [
        ("wrong type", |w| w.put("items", "i2", [("name", v(5))], T0 + 2).unwrap().0),
        ("null in a non-nullable field", |w| w.put("items", "i3", [("name", Value::Null)], T0 + 3).unwrap().0),
        ("set add to a table", |w| w.set_add("items", t("x"), T0 + 4).unwrap().0),
        ("wrong element type", |w| w.set_add("tags", v(1), T0 + 5).unwrap().0),
        ("append to a set", |w| w.append("tags", t("x"), T0 + 6).unwrap().0),
    ];
    for (i, (what, write)) in bad.iter().enumerate() {
        let mut w = joined(10 + i as u8, T0);
        let id = write(&mut w);
        let (child, _) = w.put("rooms", "r1", [("name", t("den"))], T0 + 7).unwrap();
        let rep = r.ingest(w.log(), T0 + 8);
        let why = |id| rep.rejected.iter().find(|(i, _)| *i == id).map(|(_, r)| *r);
        assert_eq!(why(id), Some(Reject::SchemaViolation), "{what}");
        // Everything built on a rejected op is rejected with it.
        assert_eq!(why(child), Some(Reject::ParentRejected), "{what}");
    }
    let mut w = joined(1, T0);
    let (ok, _) = w.put("items", "i1", [("name", t("lamp")), ("value", Value::Null)], T0 + 1).unwrap();
    let rep = r.ingest(w.log(), T0 + 8);
    assert!(r.store().contains(&ok), "{rep:?}");
    assert_eq!(Reject::SchemaViolation.code(), "schema_violation");
    // A replica refuses to author what its own schema forbids or does not know.
    let err = r.put("items", "i9", [("name", v(1))], T0 + 9).unwrap_err();
    assert_eq!(err, KernelError::Rejected(Reject::SchemaViolation));
    assert_eq!(r.put("photos", "p", [("item", t("i1"))], T0 + 9).unwrap_err(), KernelError::Undeclared);
}

/// Q: what happens when one partner upgrades first? The upgraded device's ops use a
/// table and a field the old version does not know. Rejecting them would be
/// permanent (rejections are recorded) and would take every later op of that device
/// with them. They are held instead, and deliver once the old device upgrades.
#[test]
fn an_undeclared_name_is_held_until_a_schema_declares_it_then_both_versions_converge() {
    let mut old = with(1, v1());
    let mut new = with(2, v2());
    reconcile(&mut old, &mut new, T0 + 1).unwrap();
    let (photo, _) = new.put("photos", "p1", [("item", t("i1"))], T0 + 2).unwrap();
    let (colour, _) = new.put("items", "i1", [("name", t("lamp")), ("colour", t("red"))], T0 + 3).unwrap();
    let (after, _) = new.put("rooms", "r1", [("name", t("den"))], T0 + 4).unwrap();
    let (_, to_old) = reconcile(&mut new, &mut old, T0 + 5).unwrap();
    assert_eq!(to_old.ingest.held, vec![photo], "{to_old:?}");
    assert!(to_old.ingest.rejected.is_empty());
    assert_eq!(old.held_count(), 1);
    // The ops built on it wait for it, like any op with a missing parent, whether
    // they are declared (`after`) or not (`colour`, held itself once it can deliver).
    assert!(to_old.ingest.pending.contains(&colour) && to_old.ingest.pending.contains(&after));
    assert!(!old.store().contains(&after));
    // A second sync does not re-deliver or reject them.
    let (_, again) = reconcile(&mut new, &mut old, T0 + 6).unwrap();
    assert!(again.ingest.rejected.is_empty() && again.ingest.delivered.is_empty(), "{again:?}");
    // The old device upgrades: the held ops deliver, and what waited on them follows.
    let rep = old.set_schema(v2(), T0 + 7);
    assert!(rep.delivered.contains(&photo) && rep.delivered.contains(&colour) && rep.delivered.contains(&after));
    assert_eq!(old.held_count(), 0);
    assert_eq!(old.state().encode(), new.state().encode());
    assert_eq!(old.heads(), new.heads());
}

#[test]
fn the_fold_is_the_same_whatever_the_schema() {
    // V7 only decides delivery; two replicas that delivered the same ops agree even
    // if one of them registered no schema at all.
    let mut a = with(1, v1());
    a.put("items", "i1", [("name", t("lamp")), ("box", t("b1"))], T0 + 1).unwrap();
    a.put("boxes", "b1", [("name", t("crate"))], T0 + 2).unwrap();
    a.delete("boxes", "b1", T0 + 3).unwrap();
    let mut o = observer();
    o.ingest(a.log(), T0 + 4);
    assert_eq!(o.state().encode(), a.state().encode());
    // The kernel's own row query knows nothing of containers; the view does.
    assert!(a.state().row("items", "i1").is_some());
    assert_eq!(v1().view_row(a.state(), "items", "i1"), None);
    assert_eq!(v1().check(&hearth_sync_kernel::op::decode(&a.log()[1]).unwrap().body), Check::Ok);
}

#[test]
fn deleting_a_container_hides_everything_under_it_and_undo_brings_it_back() {
    let s = v1();
    let mut a = with(1, s.clone());
    a.put("rooms", "den", [("name", t("Den"))], T0 + 1).unwrap();
    a.put("boxes", "b1", [("name", t("Crate")), ("room", t("den"))], T0 + 2).unwrap();
    a.put("items", "i1", [("name", t("Lamp")), ("box", t("b1")), ("value", v(40))], T0 + 3).unwrap();
    a.put("items", "i2", [("name", t("Rug")), ("box", Value::Null)], T0 + 4).unwrap();
    // An item whose box was never synced here is shown: a missing container hides nothing.
    a.put("items", "i3", [("name", t("Vase")), ("box", t("nowhere"))], T0 + 5).unwrap();
    let st = a.state();
    assert_eq!(s.view_row(st, "items", "i1").unwrap()["value"], v(40));
    a.delete("rooms", "den", T0 + 6).unwrap();
    let st = a.state();
    assert_eq!(s.view_row(st, "rooms", "den"), None);
    assert_eq!(s.view_row(st, "boxes", "b1"), None, "child of a deleted room");
    assert_eq!(s.view_row(st, "items", "i1"), None, "grandchild of a deleted room");
    assert!(s.view_row(st, "items", "i2").is_some(), "not in any box");
    assert!(s.view_row(st, "items", "i3").is_some());
    // The children's own state is untouched: Undo on the room shows them all again.
    a.restore("rooms", "den", T0 + 7).unwrap();
    assert!(s.view_row(a.state(), "items", "i1").is_some());
    // Moving an item out of the box and deleting the box leaves the item visible.
    a.put("items", "i1", [("box", Value::Null)], T0 + 8).unwrap();
    a.delete("boxes", "b1", T0 + 9).unwrap();
    assert!(s.view_row(a.state(), "items", "i1").is_some());
    // Undeclared fields are not in the view, though they are in the state.
    let mut w = joined(2, T0);
    w.put("items", "i7", [("name", t("Cup")), ("colour", t("red"))], T0 + 1).unwrap();
    assert_eq!(s.view_row(w.state(), "items", "i7").unwrap().len(), 1);
}

#[test]
fn a_container_cycle_does_not_loop() {
    let mut s = v1();
    // boxes may sit in boxes.
    s.collections[1] = table("boxes", vec![field("in", ValueType::Text, true)], Some(("in", "boxes")));
    let mut a = joined(1, T0);
    a.put("boxes", "x", [("in", t("y"))], T0 + 1).unwrap();
    a.put("boxes", "y", [("in", t("x"))], T0 + 2).unwrap();
    assert!(s.view_row(a.state(), "boxes", "x").is_some());
    a.delete("boxes", "y", T0 + 3).unwrap();
    assert_eq!(s.view_row(a.state(), "boxes", "x"), None);
    let _ = sha256(b"");
}
