# ADR 0009: App schemas (V7) and the container view

Status: accepted (kernel v1 stage 1, 2026-09-29)

## Context

Design §3.1 lists V7, "body matches the app schema (known table, known field types)",
as deterministic on the op, with the verdict reject. Design §2.4 has a delete of a
container hide every descendant in the view (StillLife's room → container → item).
Ruling Q3 sets the horizon per app, and ADR 0006 (departure 8) notes that every
replica of an app must use the same one.

The dispatch asks for a registered schema with tables, fields, types and a merge per
collection, container relations, and the horizon with the keep-full-history flag.

## Decision

**The schema.** An app registers a list of collections. Each collection has one
merge:

- `Lww`: rows of typed fields, last writer wins per field. Deletes are
  observed-remove (an edit beats a concurrent delete) and Undo restores. A table may
  also name a **container**: a text field that holds the id of a row in another
  table.
- `AddWinsSet`: elements of one type.
- `AppendOnly`: records of one type.

Types are text, int, bool, bytes or any. A field may be nullable, so that setting it
to null clears it. The schema also carries `horizon_ms` and `keep_full_history`, and
the replica's `Config` comes from them.

Registration checks the schema itself:

- names are 1–128 bytes of NFC;
- collection names are unique, and so are field names within a table;
- a container field is a declared text field, and it points at a table;
- the horizon is non-zero unless full history is kept.

**V7 has two verdicts, not one.**

- **A declared name used wrongly is rejected** (`schema_violation`). Examples: a Put
  to a set, a value of the wrong type, a null in a field that is not nullable.
- **An undeclared name is held.** This covers a table, field, set or stream the
  schema does not declare. A held op is parked like a quarantined one, and ops built
  on it wait for it. Every held op is retried whenever a schema is registered.

The design's single "reject" is wrong for undeclared names. The schema is local
state: it is the app version installed on this device. ADR 0004's rule is that a
check reading local state must delay, not reject, and V8 is its clock-shaped
instance. Consider two partners, one of whom has upgraded. If an undeclared name
were rejected, the older device would record a rejection of every op that uses the
new table. It would also reject, as `parent_rejected`, every later op built on those
ops. That rejection is permanent, so upgrading the old device later would not undo
it. Holding delays instead, and the two converge once both run the new version.

Reject stays sound for declared names under one rule: **a declared name never
changes kind or type, and is never removed.** Schema evolution is additive. With
that rule, every app version that knows a name judges an op on it the same way.

**The fold never reads the schema.** V7 decides only whether an op is delivered.
Two replicas with the same delivered set have the same state, whatever schema they
registered: the properties and the Alloy model are untouched.

**The view.** The app sees a projection of the state (`Schema::view_row`):

- only declared tables and fields;
- only rows that are visible;
- only rows with no deleted container above them.

Hiding is a view rule, not a fold rule, so a child keeps its own state. Undo on the
container brings the whole subtree back, and an item moved out of a box before the
box is deleted stays visible. Two edge cases:

- a container that does not exist yet (not synced, or never written) hides nothing;
- a cycle of containers ends the walk instead of looping.

## Consequences

- An app must keep every declared name's kind and type for good. Renaming means
  declaring a new name.
- A misbehaving device can write an undeclared name with a type that a later
  version declares differently. Old versions deliver the op, the new version
  rejects it, and the two disagree on validity. This needs a device that writes
  names its own app does not declare, which the kernel refuses to author
  (`KernelError::Undeclared`). ADR 0006 already accepts a Byzantine device with the
  phrase as out of reach.
- Held ops live in the replica, and in the persisted records (ADR 0010), until a
  schema declares their names.
- A snapshot's kept bodies are not run through V7. They are behind a checkpoint
  the household vouched for, and the view filters whatever they carry.
