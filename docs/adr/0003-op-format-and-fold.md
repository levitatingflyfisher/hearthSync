# ADR 0003: Op format and the fold

Status: accepted (v0, 2026-09-28)

## Context

Ruling Q9 picked dCBOR, SHA-256, Ed25519 and XChaCha20-Poly1305. Q2 made an
edit beat a concurrent delete; Q5 made LWW per field. Three implementations
(Rust kernel, Go relay, Dart tests) must produce the same bytes for the same op.

## Decision

**Format** (full spelling in `docs/reference/op-format.md`, pinned by `vectors/`):

- An op is a dCBOR map with integer keys 0–6: version, app, device, parents,
  `[millis, counter]`, body, signature. The signature covers the map without key 6.
  `id = SHA-256(signed bytes)`, computed only over bytes that are canonical dCBOR,
  so one op cannot have two spellings and two ids.
- A body is an array `[kind, ...]`: Put, Delete, Restore, SetAdd, SetRemove,
  Append, Enroll, Forget, Checkpoint.
- Values are closed: null, bool, i64, text, bytes. No floats (they do not hash
  identically across implementations) and no nesting.
- Id lists (parents, observed, cut) must be sorted ascending with no duplicates.
- Names are 1–128 bytes; an op is at most 64 KiB; at most 64 parents. dCBOR
  requires NFC text, so a non-NFC name is refused on decode.
- Unknown keys, extra array items and wrong types are rejected (closed schema).

**Fold.** State is a join of per-op effects, so any delivery order gives the same
state:

- Rows: an LWW register per `(table, row, field)`, ordered by `(hlc, op id)`; since
  ADR 0006 (OriginClock), by `(origin or hlc, hlc, op id)`, where only a re-issued
  edit carries an `origin`.
- Row deletes are observed-remove: a Delete lists the Put ids it saw, and a row is
  visible iff some Put is not covered by a live (unrestored) Delete. A concurrent
  edit was not observed, so it keeps the row visible (Q2). A tombstoned row keeps
  its field values, so Restore (Undo) brings it back whole, past the horizon too.
- Sets: add-wins OR-sets; the add's tag is its op id.
- Streams: grow-only, ordered by `(hlc, op id)`.

## Lens verdict

`lens-crdt-study`: every component is a monotonic semilattice (set unions, and a
max over a total order for LWW), and applying an op is a join, which discharges
the CvRDT obligation. The anomalies it names remain and are accepted: LWW loses
the losing concurrent write to one field ("some updates are inherently lost"),
and the OR-set approximates the sequential set (a concurrent re-add survives a
remove). The property tests check the obligation empirically: random delivery
orders with duplicates, batches in either order, and incremental fold versus a
full refold.

## Bounded nesting (kernel v1 stage 3b)

dcbor 0.25 decodes recursively, one stack frame per nesting level, with no limit.
65,000 bytes of `0x81` fit under `MAX_OP_BYTES` and abort the process on a 2 MiB
stack (the stage 3 probe); a WASM stack is smaller. Ops, envelopes, sync messages,
snapshots and stored records can all come from someone else, so no kernel code calls
the decoder directly: `cbor::decode` runs `cbor::nesting_ok` first, a loop over the
CBOR heads that checks structure and depth without recursing.

- **The limit is 16.** Depth counts levels of items (a lone scalar is 1). The deepest
  thing the kernel itself encodes, measured by instrumenting the decoder across the
  whole test suite, is 7 (in the horizon, persistence and property suites; every
  other suite stays at 5 or less).
  `tests/nesting.rs` asserts the kernel's own encodings stay at half the limit.
- **Refusal is the ordinary refusal of that path**: `not_canonical` for an op,
  `bad_seal` for an envelope, `BadMessage` for a sync message or snapshot, a
  `Persist` error for a stored record. Nothing new crosses the api.
- **One implementation.** The relay had written the guard first; it moved here and
  `relay::wire` re-exports it. The relay's own requests keep their tighter limit of 8.
- `tests/nesting.rs` is its own test binary, because a missing guard aborts the
  process: it feeds a 65,000-deep payload through every entry point on a 256 KiB
  thread stack, plus proptests at the limit's edge and over random bytes.

## Consequences

- Hierarchy hiding (a deleted container hides its items) and app schemas (V7)
  were not in v0; v1 adds both, outside the fold (ADR 0009).
- A Put with non-NFC field names made in Rust is normalised by the encoder; v1's
  bridge should normalise at the Dart boundary so the app sees what was signed.
