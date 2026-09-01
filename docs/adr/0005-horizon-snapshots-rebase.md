# ADR 0005: Horizon, snapshots and the returning phone

Status: accepted (v0, 2026-09-28); amended by ADR 0006 (v0.1), which closes gaps 1, 2
and 4, and by ADR 0007 (v0.2)

## Context

Decision 1: snapshots plus a horizon, a phone offline past it resyncs from a
snapshot and its offline edits merge as one fresh change. Q3: 90 days by default,
Reckon and journals keep full history. Q4: a returning phone re-applies edits to
fields nobody else changed and lists the rest.

## Decision

- **Checkpoint** is an ordinary signed op whose `state_hash` commits to two things
  over before(itself): the canonical folded state and the index (clock order,
  checkpoint excluded), as `SHA-256(dCBOR [state bytes, [index entries]])`. The
  index has to be covered because V4 and V5 later trust its `enrolls`, `device`,
  `kind` and `target` fields; with only the state hashed, a provider could forge
  an enrolment in a snapshot. Nobody trusts a checkpoint blindly: compaction
  recomputes both and refuses a mismatch. *Amended by ADR 0007:* the hash now
  commits to the base state, the index and the set of kept op ids, so a snapshot
  provider cannot move a kept op into the base.
- **Compaction** (`Replica::compact(now)`): pick the newest checkpoint older than
  the horizon that descends from the current base, prune the bodies of its past,
  keep their **index entries** (id, parents, device, clock, kind, target), and keep
  the folded state there as the base. `Config::keep_full_history()` never prunes.
  *Amended by ADR 0006:* the bodies of ops excluded at the checkpoint and of the
  checkpoint author's own ops are kept and folded live (KeepBodies), the base state
  is the fold of the rest, and a checkpoint some Forget cuts out is never a base.
- **Snapshot**: the checkpoint op, the index of its past, and the canonical state.
  A replica that lacks a peer's base checkpoint gets one instead of ops. It checks
  the checkpoint's signature, that state and index hash to `state_hash`, that the
  author is enrolled in that state, and that the index is closed under parents.
  A replica forgotten in the snapshot's state wipes on adoption and reports it
  (`IngestReport::wiped`), exactly as if the Forget had arrived as an op.
  *Amended by ADR 0006:* a replica gets a snapshot when it lacks any checkpoint
  past the horizon the peer holds, pruned or not (AgePull). The snapshot also
  carries the kept bodies, and the receiver checks the base state plus the kept
  ops, with V6 and the shield over the index, against `state_hash`. A replica
  that is wiped, or that the snapshot forgets, never adopts: it wipes, keeps its
  log and hands over its own ops and Forgets (WipedPush).
- **Rebase** (local to sync): after adopting a snapshot, the replica's own ops the
  peer never saw are dropped and re-authored on the new heads. For a Put, each
  field is re-issued only if its current winner is the op that won that field in
  before(the old op); otherwise it goes to a review list with both values. A row
  deleted meanwhile goes to review. SetAdd, Append and a missing Enroll are
  re-issued (since ADR 0007, not an add or edit the replica's own state had already
  removed or deleted: that is listed); Delete, Restore and SetRemove are listed, never replayed.
  *Amended by ADR 0006:* a re-issue carries the original clock as `origin` and LWW
  orders it by that (OriginClock); lost Forgets are re-authored (Reforget);
  foreign unsynced ops are listed, not re-offered (ForeignReview); base-folded
  ops the snapshot lacks are rebased or listed (BaseRebase; every kind since
  ADR 0007).
- `reconcile` lets the side that lacks the other's base pull first, so its stale
  ops are rebased, never pushed as-is. *Amended by ADR 0006:* the same holds for
  any checkpoint past the horizon the peer holds, pruned or not (AgePull), and a
  wiped side hands over instead of adopting (WipedPush).

## Lens verdict

`lens-crdt-study`, asked whether pruning by a time horizon (not by stability) is
acceptable: pruning here is a commitment problem (§4.2), which "in first
approximation" needs unanimous agreement or a small stable core, and a lost
phone is exactly the replica that "crashes permanently". A horizon swaps that
agreement for a time bound. It is acceptable only because the pruned part is
replaced by a state in the same semilattice: a late op still joins into the
snapshot state, so pruned and unpruned replicas still converge. What the horizon
gives up is semantic, not convergence. The qualification that bears is "in first
approximation": the authors allow a cheaper core. Rivals: stability-based GC
with a known replica set (Wuu–Bernstein), consensus.

## Known gaps (v1 or the Alloy model)

Gaps 1, 2 and 4 were modelled in ADR 0006 and are closed in kernel v0.1: gap 1 by
AgePull and ForeignReview, gap 2 by Shield and KeepBodies, gap 4 by ForeignReview.
Gap 3 is open.

1. **A stale op through an unpruned third peer.** If a returning phone first
   syncs with a peer that has not pruned, its old edits deliver as ordinary ops
   and fold by plain LWW, with no review list. It still converges (the join
   argument above), but Q4's "never wins silently" is not guaranteed on that path.
2. **Forget behind a pruned checkpoint.** A Forget whose cut misses an op already
   folded into a pruned replica's base cannot exclude it there, while an unpruned
   replica does. The design accepts that a Forget cannot reach behind a checkpoint,
   but with concurrent checkpoints the replicas can disagree. This belongs in the
   Alloy model (Q12) before Forget ships.
3. **The index is never pruned.** About 100 bytes per op, forever. Fine at the
   design's rates (Lullaby ≈ 11,000 ops a year); v1 should fold old index entries
   into the snapshot.
4. **Foreign unsynced ops.** Ops by other devices that only the rebasing replica
   held are re-offered as they are; if their parents were its own rebased ops they
   stay pending.
