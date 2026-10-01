# ADR 0006: The Forget and horizon model

Status: accepted (2026-09-28). Kernel v0.1 implements all eight changes, with the two
project decisions recorded under **Decision** below. This ADR records the model, what it
found, the changes, and where the implementation departs from what was modelled.

## Context

Ruling Q12 asked for a small Alloy model of Forget before Forget ships. ADR 0005 left two
gaps for that model: a stale op reaching an unpruned peer (gap 1), and a Forget behind a
pruned checkpoint when checkpoints are concurrent (gap 2). Kernel v0's 42 tests and 4,000
proptest cases never prune while a Forget is in flight, so neither gap could show up there.

## The model

`model/forget.als` (Alloy 6.2.0, the official `org.alloytools.alloy.dist.jar`, sha256
`6b8c1cb5…edb78d`; the release publishes no checksum for the jar, so this is the hash of
the file as downloaded). The jar is a desk tool in the git-ignored `.tools/`, not a
dependency.

It is a partial model, aimed at the risky part:

- **Replicas** are devices. Each holds a down-closed set of delivered ids (index entries
  count, as `store.contains` counts them), an optional pruned base with its stored
  folded state, and a wiped flag.
- **Ops** are atoms with an author, parents and a kind: Put (one field), Checkpoint
  (which commits to its author's folded state), Forget (target and cut) and Enroll (V4).
  The HLC is a total order consistent with parents (V3). An author's parents are its
  heads, and its stamp comes after everything it has delivered.
- **The fold** is abstracted as the set of included puts. That is what `State` keeps per
  row: every put id, plus the LWW winner.
- **Events:** author; deliver a parent-closed batch from any peer that holds the bodies
  (so delivery happens in any order); compact at a checkpoint past the horizon, as
  `fold_at` does; adopt a snapshot; rebase per Q4. The v0 reconcile guard applies: a side
  that lacks the other's base pulls first. Time is only the growing set of checkpoints
  past the horizon.
- **Not modelled:**
  - signatures and hashes, assumed sound;
  - quarantine, which only delays;
  - Delete, Restore, sets and streams;
  - the relay;
  - pending ops, which the model drops where the kernel would park them.

Each proposed fix is a flag, so every property is checked twice: once against v0, once
against v0 with the fixes.

## Properties

| | Property |
|---|---|
| (a) | Convergence: replicas that have delivered the same ops have the same state, and so exclude the same data ops. (a′) Compaction never changes a replica's own state. |
| (b) | Once a replica holds a Forget, no op by its target outside before(cut) is in its state. (b′) is the same, except where a checkpoint that lacks the Forget covers the op (design §4: "a Forget cannot reach behind a checkpoint"). |
| (c) | A replica holding a Forget of itself is wiped. Forgetting yourself delivers the Forget locally, so the wipe happens in the same step. A wiped replica never authors (a guard). |
| (d) | Finality: nothing a replica has pruned into its base as included is excluded by a replica that knows at least as much. |
| (e1) | Q4, "fields nobody else changed". An edit is *past the horizon* for checkpoint C when C is old and no replica holds both the edit and C. When such an edit (or its re-issue) first meets C at a replica and becomes the field's winner there, the value it displaces must be one its author saw. Later propagation is ordinary concurrency. |
| (e2) | An old edit never beats, by LWW, a write that is newer by the clock and that it did not see. |
| (f) | A Forget, once written, is never lost from the household. It stays in some replica's log or rebase list, or it is re-issued. |
| (g) | A field's current winner is never lost silently. It stays in some replica, on a review or rebase list, or it is re-issued. |

## Counterexamples against v0

Each of these is a trace the solver found. The fix named for each is defined in the next
section.

1. **Compaction alone changes state (a′).** D0 forgets D2 before it has seen D2's put P.
   D1 checkpoints C over P without having seen the Forget. D0 receives P and C, and
   excludes P. D0 then compacts at C. `fold_at` applies only the Forgets in before(C),
   so P comes back into D0's state, with nothing new delivered. *Fix: Shield + KeepBodies.*
2. **Divergence (a).** D2 writes P, checkpoints C over it, and prunes at C. Meanwhile D0,
   not having seen P, forgets D2. The two exchange everything. Both now hold {P, C,
   Forget}. D0 excludes P; D2 has P folded into its base and keeps it. This is §3.4's
   "forgotten while away", where the device's own automatic checkpoint shields its edit.
   *Fix: Shield (a device's own checkpoint does not shield it) + KeepBodies.*
3. **An op excluded where it was committed (d).** The same shape, seen from the pruned
   side: D1 pruned P in as included, and D0 excludes P while knowing everything D1 knows.
   *Fix: Shield + KeepBodies.*
4. **A Forget that does not stick (b).** D2 forgets D1. D1 writes P. D0 checkpoints over
   P and prunes, then receives the Forget; P stays in its base. The fix makes every
   replica agree, but it agrees on keeping P, so (b) still fails **by design**. See
   Ruling 1.
5. **An old edit wins silently through an unpruned peer (e1, gap 1).** D1 checkpoints C,
   and C passes the horizon. D2 is away and writes P. Later, D1 writes P′ with an older
   clock than P. D2 syncs with D1, which never pruned, so it receives C and P′ as plain
   ops and nothing triggers a rebase. P beats P′, which D2 never saw, and no review list
   appears. *Fix: AgePull + ForeignReview.*
6. **A re-issued edit beats a newer one (e2).** D2 is away and writes P. D1 checkpoints
   C, which passes the horizon, and prunes. D2 adopts the snapshot and re-issues P with a
   fresh clock; nobody had changed the field, so the Q4 check passes. Meanwhile D1 writes
   P′, newer than P by the clock. The fresh clock lets the old edit beat it on every
   replica. This is exactly the failure §3.4 set out to prevent, reached through the
   rebase itself. *Fix: OriginClock.*
7. **A self-Forget is lost (f).** D2 forgets itself and wipes. It then syncs with a
   pruned peer and adopts the snapshot. The rebase drops its own unsynced Forget into the
   review list, and D2 has no keys left to redo it, so no one else ever learns of the
   Forget. *Fix: WipedPush + Reforget.*
8. **Concurrent checkpoints lose data silently (g).** D0 writes P, checkpoints C0 and
   prunes. D1 checkpoints C1 concurrently. Both pass the horizon. D0 lacks C1 and so
   adopts D1's snapshot. P was pruned into D0's base, so it is no longer an "unsynced op
   with a body", and it is not rebased. It ends up in no replica and on no list.
   *Fix: BaseRebase.*

While the fixes were being built, the model found three more ways to lose a Forget on
adoption. They are the reason Reforget covers every known Forget and WipedPush has its
limits:

- a forgetter that lacked an old checkpoint lost its Forget of another device to review;
- a Forget pruned into the forgetter's own base was lost on a concurrent snapshot;
- a replica forgotten *by* the snapshot was wiped mid-adoption, and lost a concurrent
  Forget it had written.

It also found two leaks in the first version of WipedPush:

- the handover carried a co-traveller's late edit;
- the handover delivered an old checkpoint as a plain op, which skipped the rebase.

## The kernel changes

Each item below is the smallest rule the model needed. All eight are implemented in
kernel v0.1 (see **Decision**).

1. **Shield (V6, fold_at, checkpoint hash).** A Forget F does not exclude an op u when a
   delivered checkpoint C has u in before(C) and F outside it, unless C was itself written
   by F's target outside the cut. This makes "a Forget cannot reach behind a checkpoint"
   a deterministic function of the delivered set, instead of an accident of who pruned.
   The exception keeps §3.4's "forgotten while away" row: an offline device's own
   automatic checkpoint does not protect its post-cut edits.

   As written, *any* concurrent checkpoint shields, including a young one. That is
   broader than §4, which accepts the loss only behind a *pruned* checkpoint, meaning one
   past the horizon. Option B, AgedShield, adds that condition. Ruling 1 weighs the two.
2. **KeepBodies (compaction, snapshots).** Compaction at C keeps the bodies of ops
   excluded at C, and of C's author's ops, and folds them live rather than into
   `base.state`. Snapshots carry these bodies. The rule behind this: a pruned base can
   always *add* an op later, because the fold commutes, but it can never *remove* one.
   So nothing covered and included may ever become excluded, and everything that might
   is kept as a body. The kept set is bounded by one device's ops per checkpoint interval
   plus the excluded ops.
3. **AgePull (reconcile).** Pull a snapshot and rebase whenever the peer holds a
   checkpoint past the horizon that you lack, whether or not the peer has pruned. An
   unpruned peer can build the snapshot at that checkpoint. v0 keys this on "the peer
   pruned", which is gap 1.
4. **ForeignReview (adopt).** Unsynced ops by other devices go to the review list, not
   back into the log as they are. Re-offering them was ADR 0005 gap 4, and it is another
   unreviewed path for late edits.
5. **OriginClock (rebase, fold).** A re-issued Put carries the original edit's HLC, and
   LWW orders it by that HLC. The op's own HLC still satisfies V3. The Q4 check already
   guarantees that the re-issue beats the value it saw, so the only thing that changes is
   that it can no longer beat a newer write.
6. **BaseRebase (adopt).** On adopting a snapshot, winning field values in the old base
   state whose ops the snapshot lacks are rebased (own) or listed (others'), like
   unsynced edits.
7. **Reforget (adopt).** Every Forget the replica knew and the snapshot lacks is
   re-authored on the new heads with a fresh cut. This covers its own Forgets, other
   devices' Forgets, and Forgets pruned into its base. It works because every device
   holds the phrase (Q1). It replaces "Forget is listed, never replayed".
8. **WipedPush (sync).** A replica that is wiped, or that a snapshot would wipe, never
   adopts. It wipes on what the snapshot tells it, keeps its log, and hands over as-is
   only its own ops and its own Forgets with their past. It never hands over an old
   checkpoint: the receiver must take that as a snapshot and rebase.

## Results with the fixes

Every "none" below means "no counterexample within this scope". None is a proof. The
scope is what one solve finished in under ten minutes on this box with SAT4J. The
small-scope hypothesis says most bugs have small counterexamples. It says *most*, and it
is a hypothesis.

| Check | v0 | v0 + fixes | Scope (devices / ops / trace length) |
|---|---|---|---|
| (a) convergence | counterexample 2 | none (A and B) | 3 / 4 / 7 |
| (a′) compaction neutral | counterexample 1 | none (A and B) | 3 / 4 / 7 |
| (b) strict Forget | counterexample 4 | **counterexample** under A and under B (by design, Ruling 1) | 3 / 4 / 7 |
| (b′) Forget except shielded | — | none (A and B) | 3 / 4 / 7 |
| (c) self-wipe | none | none | 3 / 4 / 8 (v0), 3 / 4 / 7 (fixed) |
| (d) finality | counterexample 3 | none (A and B) | 3 / 4 / 7 |
| (e1) Q4 seen | counterexample 5 | **counterexample** via WipedPush (Ruling 2) | 3 / 4 / 8 |
| (e1′) Q4 seen, except what a wiped device handed over | — | none | 3 / 4 / 8 |
| (e2) Q4 clock | counterexample 6 | none (it holds by construction once LWW uses the origin clock) | 3 / 4 / 9, no Forget |
| (f) Forget durable | counterexample 7 | none | 3 / 4 / 7 |
| (g) no silent loss | counterexample 8 | none | 3 / 4 / 8, no Forget |

Rows (c), (e1), (e2), (f) and (g) were checked under option A.

Enroll is left out of the scopes wherever V4 plays no part. With Enroll in, the v0 (c)
check was also clean at 3 / 6 / 9.

**Induction.** The convergence family is also checked by one-step induction. The
invariant has four parts:

- delivered sets are down-closed;
- every covered put is in `baseIncl` or kept as a body;
- the two sets are disjoint;
- every put in `baseIncl` is backed by a held checkpoint past the horizon, written by
  another device, in whose past the put was not excluded;
- bases are past the horizon.

With the fixes, under both option A and option B, three checks come back with no
counterexample at 3 devices and 7 ops:

- any step preserves the invariant;
- under the invariant, each replica's state equals the fold an unpruned replica would
  compute from the same delivered set;
- a replica that knows at least as much as another holds everything the other pruned in.

Convergence and finality follow from the second and third. The same invariant fails for
v0 in one step. A run shows the induction is not vacuous: a compaction that keeps some
bodies and folds others is reachable.

**Non-vacuity of the trace checks.** Runs with the fixes reach every mechanism:

- a shielded op;
- kept bodies under a base;
- a live excluded op under a base;
- adoption from an unpruned peer;
- a re-issue;
- a review item;
- a wiped handover.

## Rulings needed

1. **Which shield: any concurrent checkpoint (A), or only one past the horizon (B)?**
   Both are modelled. Both pass every convergence and finality check, including the
   induction. Under both, the strict (b) fails, and it has to fail somewhere: a Forget
   that always wins would need every pruned replica to un-fold an op, which means keeping
   every body.

   The case that separates them: the forgotten device's post-cut op reached a third
   device, and that device checkpointed over it, all before the Forget arrived there.
   - **A:** the op is never excluded. The Forget has no effect on it anywhere.
   - **B:** the op is excluded wherever the Forget arrives first. When that checkpoint
     passes the horizon, every replica re-includes it at the same moment. This is §4
     read literally; the `b` counterexample under B is exactly that reversal.

   A and B end in the same state once the checkpoint is old. B adds a stretch of up to
   90 days in which the op is hidden, followed by a surprise return. Neither can end
   with the op excluded without keeping every body.

   *Recommend A.* It gives the same final state without an edit coming back months
   later. B is §4's letter.

   The model's `Old` is global time. In the kernel, B needs a structural form, because
   the checkpoint hash and `fold_at` must not depend on local time. The form would be: a
   checkpoint shields once some delivered descendant op carries an HLC at least one
   horizon after it. That form is not modelled.
2. **A self-forgotten device's late edits.** A device that forgets itself while holding
   edits made past the horizon can either lose its Forget when it next syncs (v0, (f)),
   or hand its log over as-is (WipedPush). Under WipedPush those edits land by LWW with
   no review, because the one device that could review them has just wiped itself.
   *Recommend WipedPush:* the edits keep their original clocks, so they never beat a
   newer write ((e2) holds), and losing a Forget is worse than an unreviewed old edit.

## Decision

Both open questions were decided:

- **Ruling 1: shield option A.** Any concurrent checkpoint shields its past from Forgets
  it lacks, except one written by the forgotten device outside the cut. No age
  condition, so no structural "sealed" form is needed and an excluded op never comes
  back months later.
- **Ruling 2: WipedPush.** A self-forgotten (or snapshot-forgotten) device never adopts;
  it hands over its own ops and its Forgets' past as they are. Its late edits land
  without review; losing the Forget would be worse.

All eight changes landed together, as the model requires (a subset was never checked):

| Change | Where in the kernel |
|---|---|
| Shield | `replica::Exclusion` (V6 with the shield); a delivered checkpoint triggers a refold while any Forget is known |
| KeepBodies | `Replica::base_at` / `compact`; `Snapshot::kept`, checked by `verify` against the checkpoint hash |
| AgePull | `Hello::old`, `Request::lacks`, `Replica::offer` builds a snapshot at any held checkpoint descending from its base; `reconcile` pulls first |
| ForeignReview | `ReviewItem::Foreign` on adoption |
| OriginClock | `Body::Put::origin` (5th body item, strictly older than the op's clock); LWW orders by `(origin or clock, clock, id)` |
| WipedPush | `adopt` refuses when wiped or forgotten by the offer; `Replica::handover`; `reconcile` hands over |
| Reforget | `adopt` re-authors every known Forget the snapshot lacks, once per target |
| BaseRebase | `adopt` rebases own base-folded field winners the snapshot lacks, lists others' as `ReviewItem::Lost` |

Each counterexample is a named regression test in `kernel/tests/model_cx.rs` (`cx1_…`
to `cx8_…`), each watched failing on v0 first. A fifth property test runs the horizon
(checkpoints, compaction, Forgets, time jumps, syncs) and checks the induction's
`ind_ideal`: every replica's state is the unpruned fold of what it delivered, plus a
proxy for (f): every device an archived Forget names is still forgotten on some
replica. It fails on v0 and found two holes in the first implementation (a Forget kept as a body was
missing from the cut cache after a snapshot; an own enrolment lost from the base), both
now pinned by named tests.

### Where the implementation departs from the model

The model's clean results cover the model. These points are outside it, and the model
has not been re-run on them. *ADR 0007 (v0.2) closes items 1, 4 and 5 and reduces
item 6 to a lone device; the rest stand.*

1. **Stall.** When a side lacks an old checkpoint that the peer holds but cannot build a
   snapshot at (it is concurrent with the peer's base), `reconcile` returns
   `SnapshotUnavailable` and no ops move. (If an earlier offer in the same call forgot
   one side, that side has already wiped.) The model deadlocks the same way (no
   delivery, no adoption); the kernel makes it explicit. It clears once a checkpoint
   above both ages. Liveness is not modelled.
2. **BaseRebase's Q4 test reads the index.** A base-folded edit has no body, so "the
   field's winner is one the edit saw" is checked as "the current winner is in
   before(edit), or re-issues something that is", not by the model's `seenWin`.
3. **Re-enrolment.** A device whose own Enroll was folded into its base, and is missing
   from the snapshot it adopts, enrols itself again before re-issuing anything. The
   model's static V4 fact hid this case.
4. **Scope of the fold.** V6 and the shield apply to every kind except Enroll and Forget
   (the model has Puts and Checkpoints). BaseRebase covers row fields only: deletes,
   restores, set ops and appends folded into a base and missing from the snapshot still
   vanish without a review item. The model's (g) is about field winners only.
5. **The base/kept split is not committed.** The checkpoint hash covers the fold at the
   checkpoint, not which ops the provider kept as bodies. A dishonest provider could
   fold into the base an op that should stay a body, and a later Forget could then not
   exclude it on the receiver.
6. **KeepBodies cost.** A device compacting at its own checkpoint keeps all its own ops:
   a one-device household never prunes, and in general only ops behind another
   device's checkpoint are pruned.
7. **Reporting.** `IngestReport::excluded` lists ops newly excluded; an op a checkpoint
   brings back under its shield is not reported. Foreign checkpoints are dropped on
   adoption without a review item (they carry no data), and a foreign Forget is
   re-authored rather than listed.
8. **Pending ops** are parked, not dropped. **The relay path** is not modelled and has
   no AgePull yet. **"Old"** is each replica's own clock and `Config`, so every replica
   of an app must use the same horizon.

## Consequences

- Forget should not ship on v0. Counterexamples 1–3 are real divergence, found at three
  ops.
- The eight changes interact. Each was checked only together with all the others. A
  subset may reopen a counterexample, so implement them together or re-run the model on
  the subset.
- Each counterexample above is a named regression test in `kernel/tests/model_cx.rs`.
- Re-run the model after any change to V6, compaction, snapshots, rebase or reconcile.
