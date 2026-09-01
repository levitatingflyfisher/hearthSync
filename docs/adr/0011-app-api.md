# ADR 0011: The app-facing api

Status: accepted (kernel v1 stage 1, 2026-09-29)

## Context

Design §6 sketches the bridge surface (`seal_op`/`finish_op`, `ingest`, `checkpoint`,
`reconcile_plan`). The spike binds us to three things:

- flutter_rust_bridge v2;
- stable, single-threaded WASM on the web, because the threaded build needs
  COOP/COEP, which GitHub Pages cannot send;
- every web-facing call marked `#[frb(sync)]`.

The v0 report lists what v1's surface needs:

- prepare/finish, so the device key stays in the platform key store;
- `ingest` returning row changes;
- sync messages as bytes;
- snapshots;
- the review list;
- the wipe signal;
- NFC at the boundary.

## Decision

`kernel/src/api.rs` holds one opaque type, `Kernel` (one app's replica on this
device), plus plain structs and enums. Every call is synchronous. Everything that
crosses the boundary is a byte vector, a string, an integer, or a simple struct or
enum. There are no tuples, generics, lifetimes, callbacks or handles, so the file
can be handed to the frb codegen as it stands. Two compile-time checks back that
up: `Kernel` must be `Send + Sync + 'static` (frb's opaque handles need it), and
`api` and `schema` compile under `missing_docs`.

**Opening.** `Kernel::open(OpenArgs { app, seed, device, schema, records, now })`
takes:

- the seed, which every device stores (Q1);
- the device's *public* key;
- the app schema (ADR 0009);
- every record the app stored (ADR 0010), or none on first launch.

`stored_info(records)` reads, without keys, whose records they are and whether the
device was wiped, so that the app knows whether to fetch the words at all.
`sealed_handover(records, schema, now)` is what a wiped device does after a
restart, when it has no keys: it returns, from the records alone, the envelopes
of its own ops and of its Forgets' past, exactly as it first sealed them, for the
relay (WipedPush without keys; a LAN request cannot be opened, so this is the
relay path only). The app deletes the records once they have been handed on.

**Keys stay platform-side, with no callbacks.** A call that writes returns
`Step::Sign { signable }`. The app signs with the platform key store and calls
`finish(signature)` until it gets `Step::Done(outcome)`.

- A local `write` needs one signature. The op is prepared on the replica, and the
  signature is checked against the device key before anything is recorded, so a
  wrong signature leaves the write waiting and records no junk op.
- Enrolment, Forget, checkpoints and above all snapshot adoption author inside the
  kernel. Adoption can re-issue many ops, each on top of the last, so each op's
  bytes depend on the previous signature. These calls run on a copy of the
  replica, with a signer that answers from the signatures collected so far. On the
  first miss, the copy is dropped and the kernel asks for that signature. The next
  `finish` replays the call from the start.
  - Ed25519 signatures are deterministic, and the kernel is pure, so every replay
    asks for the same bytes up to the new one.
  - The replica changes only when the call completes.
  - The Alloy-checked adoption code runs in the same order. It is split at the one
    point where authoring begins: `adopt_install` verifies, installs and ingests,
    and `adopt_rebase` re-enrols, rebases and re-forgets. The api runs the first
    half once on a copy, keeps that copy while the flow waits, and replays only the
    second half on a fresh clone of it.
  - A flow of N signatures costs one install plus N + 1 rebase runs, each on a
    clone of the replica. At 10⁴ ops, a 50-edit rebase took 52.5 s when every run
    replayed the whole adoption, and takes 1.7 s with the split.

**Outcomes.** Every completed call returns an `Outcome`:

- the records to write, and whether to reset first (ADR 0010);
- the changes to the app's tables:
  - `RowChange`: the full declared row, or `visible: false` to hide it;
  - `SetChange`;
  - `StreamChange`, which can also remove a record that a Forget took out of the
    fold;
- the sealed ops this device wrote, for the relay;
- heads;
- delivered, excluded, quarantined, held and pending ids;
- rejections with stable codes, plus `bad_seal`;
- the wipe flag (set once, when the device learns it is wiped);
- re-issued ids;
- new review entries.

When `open` finds records made under another schema, the first outcome
(`flush()`) sets `replace_view` and carries the whole view, so the app replaces
its tables. A newer schema can release held ops and reveal names the old version
did not show.

The row changes are computed incrementally from the ops delivered in the call,
following container links down to every child. After a refold (a Forget, or a
checkpoint while Forgets are known) or a snapshot install, the whole view is
diffed. `view_all()` recomputes the view from scratch for a first paint.

**Review list.** `ReviewEntry` is one flat struct for every kind of rebase item
(`field`, `row_deleted`, `op`, `foreign`, `lost`, `pruned`). Entries are persisted
as records under the api's reserved tag and listed by `review()`.
`dismiss_review(key)` removes one.

**Sync.** Messages are sealed as a whole (ADR 0008) and carry ops inside. The
choreography is `sync::reconcile`, split between the two devices:

1. Each side sends `hello(now)`, and each answers the other's hello with
   `request(hello)`. The answer says whether the requester `needs_snapshot`.
2. A side that needs a snapshot (and is not wiped) takes `offer(request)` with
   `accept` first. Then the other side does the same if it needs one.
3. Both sides make fresh requests. If a side still lacks a checkpoint, it must be
   wiped: it sends `handover(request)` instead of an offer, and nothing else moves.
4. Otherwise ops move both ways: `accept(offer(request))`, once in each direction.

The relay path uses the outcome's sealed `outgoing` ops, `ingest(sealed ops)`,
`snapshot()` (the base, sealed) and `adopt_snapshot(sealed snapshot, sealed
ops)`.

**NFC.** Every table, row, field, set, stream and label name, every text value and
every set element is NFC-normalised on the way in. The app therefore sees exactly
what was signed, and two spellings of "café" are one row.

### The relay client (stage 3b)

The relay (ADR 0013) needs, from a client, fields and positions the kernel is best
placed to keep, so the api keeps them, in its own records (a ledger record at
`[TAG_APP, 'R']` and one record per outbox entry at `[TAG_APP, 'O', id]`, re-emitted
on a reset like the review list), never in `Replica`:

- **Positions.** `relay_state()` gives the own log's next seq and the pull cursors.
  `relay_uploaded(ids, first_seq)` records an acknowledged append;
  `relay_pulled(cursors)` records a pull, after the ingest that took it. The own
  log's cursor also raises the own seq: an append the relay stored but whose
  answer was lost (a time-out) would otherwise leave every later upload answered
  `seq`. The pulled-back ops left the outbox on ingest.
- **Generations.** The relay names its channel's generation in every `enroll` and
  `pull` answer; it changes when an idle channel is expired and made again (ADR 0013).
  `relay_generation(g)` records the first one. A different one resets the positions
  (own log from seq 1, no cursors, no covers) and puts every op the device still holds
  a body for back in the outbox; the client pulls again, then uploads. Ops behind the
  device's base have no body left: a device that needs them adopts a snapshot, which
  the device uploads with empty covers. `relay_state().generation` is 0 until known.
  Forget records not yet posted survive the reset with their `cut_seq` re-based: a
  self-Forget's is set again when its Forget op (back in the outbox) is acknowledged
  in the new own log, and another target's becomes 0, the device's cursor on the
  target's new log. A later cursor could cover entries the target uploaded after the
  Forget, which the cut does not hold; 0 only means the relay refuses more of the
  target's own uploads, and the author's outbox forwards the cut's ops instead.
- **The outbox.** `relay_outbox()` is every op this device holds that the relay is
  not known to hold, sealed, in clock order: what it wrote, and what an `accept`
  delivered (LAN offers, WipedPush handovers, the ops above a LAN snapshot, but not
  the snapshot's own base). What `ingest` or `adopt_snapshot` delivered came from
  the relay and never joins; an op `ingest` opens leaves the outbox, since the relay
  evidently holds it. This is what lets an op learned only over the LAN reach
  relay-only devices. Acknowledged ops leave; pruned or replaced ones drop out.
- **Covers.** An own checkpoint records the pull cursors, plus the own log up to its
  last acknowledged seq, as that checkpoint's covers, only if nothing is pending,
  quarantined or held at that moment (the protocol's pruning rule).
  `relay_snapshot()` returns the sealed base with the covers of its checkpoint, or
  none if the base is another device's checkpoint.
- **Enrolment.** `relay_enrollment(device)` gives the household key, device, label
  and auth of the `enroll` verb, from the Enroll op's body or, once pruned, by
  re-signing (Ed25519 is deterministic, so the bytes are the op's).
- **Forget, and its ordering.** Each Forget this device writes is remembered with
  its `cut_seq`: the target's pull cursor when the Forget was written, or, for a
  self-Forget, the seq the Forget op itself lands at. `relay_forgets()` hands a
  record out only once its Forget op has left the outbox (acknowledged, or pulled
  back), because a forgotten device reads the channel frozen at its record and must
  find the Forget op there to wipe. `relay_forget_posted(forget)` ends it. The
  rule "upload the Forget op before posting the record" is therefore enforced by
  the api, not left to each app.

Limits, accepted for v1:

- An op that arrives pending from the relay and is admitted by a later LAN
  `accept` joins the outbox and may be uploaded twice (the relay stores the copy in
  this device's log). An op pending from the LAN and admitted by a later relay
  `ingest` does not join; LAN offers are closed under parents, so this needs a
  withheld parent.
- A LAN sync before a pull queues ops the relay already has; pulling first drops
  them. Duplicates cost relay bytes, not correctness.
- A self-forgotten device that restarts before posting its record cannot post it
  later: the Forget body is sealed and the words are gone. Its handover ops still
  reach the relay (`sealed_handover`), and every peer excludes it on the Forget op;
  only the relay's freeze of that device is missing, and the device is wiped.
- `cut_seq` is taken from the pull cursor. If the Forget's author holds some of the
  target's ops from the LAN that the target uploads later, above the cursor, the
  relay refuses those uploads; the author's own outbox forwards them instead.

## Evidence

`kernel/tests/api.rs` drives two devices, and a third for self-Forget, only through
`api`, with a software signer standing in for the key store. After every call, the
test applies every row, set and stream change to a mirror of the app's tables and
checks it against a fresh `view_all()`. That covers refolds, snapshot installs and
container hiding.

The scenarios are:

- edits;
- edit against a concurrent delete;
- a deleted room hiding its box and item, and Undo bringing them back;
- an OR-set re-add;
- streams;
- a restart from the stored records;
- an upgrade: held ops on the old schema, released on reopen, with the view
  replaced;
- NFC;
- a past-horizon return that re-issues through several signatures and persists its
  review list across restarts;
- Forget, of another device and of oneself;
- the relay path with a tampered envelope;
- another household's message;
- refused input.

## Consequences

- Replaying the rebase costs O(N) clones of the replica for a rare flow, about
  33 ms each at 10⁴ ops. If long rebases turn out to be common, the next step is a
  copy-on-write store, not restructuring the rebase.
- The frb codegen has not been run over this file yet; that is stage 2, with the
  Dart implementation of `Persist`.
- `Kernel` holds the seed-derived sealing keys for its lifetime, so a device wiped
  mid-session can still seal its WipedPush handover (ADR 0008).
