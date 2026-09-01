# ADR 0010: Persistence as opaque records the app stores

Status: accepted (kernel v1 stage 1, 2026-09-29)

## Context

ADR 0001 keeps I/O out of the kernel: Dart keeps persistence (Drift on native,
IndexedDB or OPFS on the web). v0's `OpStore` has only an in-memory
implementation, and pending, quarantined and rejected ops, the base and the wipe
flag lived only in `Replica` memory. The v0 report asks v1 to persist all of it,
to load the index once per session, and to cache enrolment and ancestry so that V4
and V5 stop walking the graph.

The web build is single-threaded WASM with synchronous calls (spike). It cannot
call back into IndexedDB, which is asynchronous. The kernel therefore cannot drive
the storage. The app has to.

## Decision

**The kernel emits and accepts records; the app stores them.** After each call,
`Replica::take_changes(keys)` returns a `Changeset`: an optional reset flag, then
records, each a key and either a value or a deletion. The app writes it into one
key-value table (Drift: `hearth_records(key BLOB PRIMARY KEY, value BLOB)`;
IndexedDB: one object store with ArrayBuffer keys). It does so in the same
transaction as the row changes it applies to its own tables. On launch,
`Replica::load(keys, config, device, records, now)` takes every record back.

The `Persist` trait (`apply`, `records`) names that contract. `MemPersist`
implements it for tests. The Dart side implements it over Drift and IndexedDB.

**Records** (`kernel/src/persist.rs` has the full table). A key is a tag byte, plus
an op id where the record belongs to one op. The value is dCBOR, and its first item
is the record version (1). Load refuses any other version.

| Record | Key | What it holds |
|---|---|---|
| meta | tag only | app, household, device, the wipe flag, the schema fingerprint, and the newest clock seen |
| base | tag only | checkpoint id, kept ids, and the base state (sealed) |
| op | tag + id | index entry, and the sealed body until it is pruned |
| pending, quarantined, held | tag + id | sealed op |
| rejected | tag + id | reject code |
| app | 0x20 + anything | reserved for the api (the review list) |

Some things are never stored, because they are cheap to rebuild once per session
from the index and bodies:

- the enrolment cache, which V4 uses (ADR 0004 v1 note);
- ancestry: V5's bounded walk needs only the index, so no cache is kept;
- `waiting`;
- the folded state, the exclusions and the cuts, rebuilt by one refold.

The wipe flag is stored and never derived. A replica wiped through WipedPush has no
Forget in its log that would say so. The newest clock is stored too, because the
creation-time guard's high-water mark can outlive the ops that set it: an adoption
may drop them. Without it, a reloaded replica would stamp different clocks from the
original.

**Write order and crash safety.**

1. The app applies each changeset in one transaction, in the order given, before
   it sends anything the same call produced.
2. A reset changeset (after a snapshot is adopted, the whole log is replaced)
   *must* be atomic: delete everything, then write.
3. Other changesets are also ordered so that any prefix loads:
   1. new op records, parents first;
   2. added pending, quarantined, held and rejected records;
   3. base;
   4. meta;
   5. pruned op bodies, and removals.

Load repairs every prefix of that order:

- a pending record whose op has delivered is dropped;
- a body that the base no longer keeps is pruned again;
- an op whose parent record is missing is re-admitted;
- a pending op whose parents have all arrived is re-admitted.

Everything load repairs is marked, so the next `take_changes` writes the repair.

## Evidence

`kernel/tests/persist.rs` has a property test (300 cases per run, over three
devices). Its steps cover edits, deletes, restores, set operations, appends,
checkpoints, compaction, syncs with snapshots, Forgets, writes the other devices'
schema does not declare (held), writes from a clock an hour ahead (quarantined)
and time jumps past a short horizon.

It checks:

- after any prefix of the history, the records written call by call equal a full
  rewrite of each replica, which shows the journal misses nothing;
- reloading from those records alone gives the same full records, state,
  exclusions, heads, pending, quarantined and held sets, wipe flag and keys;
- the rest of the history, run on the originals and on the reloaded replicas, ends
  identical;
- a changeset cut at a random point still loads, and once the loaded replica has
  the original's log again, it folds to the same state.

Named tests cover:

- a wipe learnt only from a snapshot, which survives a reload even when the app
  passes the words again;
- records refused under another household's keys, for another device, from a newer
  version, or with a flipped byte;
- compaction writing the base before the prunes.

Sealing is deterministic (ADR 0008), so "the same records" means byte for byte.

The property found a bug that predates v1. Adoption could fail half-way with
`ClockBehind` and lose unsynced edits. It is fixed separately (ADR 0007 v1 note).

## Consequences

- Load costs one pass over the index plus one refold. That is the "once per
  session" of design §6, measured in the kernel v1 stage 1 report.
- The meta record is rewritten whenever the newest clock moves, which is nearly
  every call. It is under 200 bytes.
- The app must not interleave a changeset with a later one. The kernel is called
  synchronously, so taking and writing one changeset per call keeps them in order.
- Records hold sealed bodies. After a wipe the seed is gone, so a restart cannot
  open them (ADR 0008).
