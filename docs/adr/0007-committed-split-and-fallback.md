# ADR 0007: Committed split, fallback pull, every-kind rebase

Status: accepted (2026-09-28). Kernel v0.2. It amends ADRs 0005 and 0006.

## Context

Kernel v0.1 (ADR 0006) left four concerns that this ADR settles:

1. The checkpoint hash did not commit to which ops a snapshot keeps as bodies, so a
   dishonest provider could fold a kept op into the base, where a later Forget could no
   longer exclude it.
2. BaseRebase covered row-field winners only. Deletes, restores, set ops, appends, and
   edits that had lost every field could vanish on adoption.
3. AgePull could stall (`SnapshotUnavailable`) for up to a horizon.
4. A device compacting at its own checkpoint kept all its own ops, so a one-device
   household never pruned.

## Decision

**The backing rule, and a checkpoint that commits to it.** An op in before(C) is folded
into the base at C if and only if it is **backed**: some checkpoint in upto(C), written
by a device other than the op's author, has the op in its past and did not exclude it
there. Every other op stays a kept body. Checkpoints themselves are never kept: they add
nothing to the state, and V6 and the shield read only their index entry.

This is the model's induction invariant turned into the rule. It is a function of
before(C) alone, so the checkpoint's author can compute it. `state_hash` now commits to
three things:

- the **base state**;
- the **index**;
- the **kept ids**.

It is `SHA-256(dCBOR [base state bytes, [index entries], [kept ids]])`. Compaction
recomputes it. A snapshot receiver checks it over exactly what it received, so a
provider can neither move, drop nor add a kept body. As before, the checkpoint's author
is trusted.

**Fallback pull.** When the requester lacks an old checkpoint that the provider holds
but cannot build a snapshot at (because it is concurrent with the provider's base), the
provider sends a snapshot at its own base, even though the requester already holds it.
Adopting that snapshot still gives the requester everything the provider holds, and
sends the requester's own ops through the rebase, which is all AgePull was for. Nothing
in the model required the adopted checkpoint to be new to the requester. A provider
without a base can build a snapshot at every old checkpoint it holds, so
`SnapshotUnavailable` is now defensive only.

**BaseRebase for every kind.** On adoption, every op folded into the old base that the
snapshot lacks is enumerated from the index, and each has an outcome:

- Field winners take the ADR 0006 path.
- The replica's own set adds and appends are re-issued, as unsynced ones already are.
- Deletes, restores and set removes are listed, never replayed, also as before.
- Other devices' ops are listed.
- Forgets are re-authored (Reforget); every lost Forget of a target maps to the one
  re-authored Forget.
- The device's own Enroll maps to the enrolment that now stands.

The new `ReviewItem::Pruned` carries what the old base state still says about the op:

- the row;
- the observed puts;
- the set element;
- the record;
- the enrolled device.

**One-device pruning is left as it is (Ruling 1).** With the backing rule, a device's
own ops are pruned as soon as another device's checkpoint covers them, even when the
device compacts at a later checkpoint of its own; v0.1 kept them. A lone device still
never prunes its own ops, for these reasons:

- Any phrase holder can enrol a new device at any time and forget this one with a cut
  that misses its latest ops.
- Ruling 1's exception says a forgotten device's own checkpoint does not shield its
  post-cut edits, so every replica must then exclude them, and a pruned op cannot be
  excluded.
- Neither "no other enrolled device" nor "every device has acknowledged" can be known,
  because enrolment is open to the phrase at any time.
- The only acknowledgement that settles the question is another device's checkpoint,
  and that is exactly the backing rule.

Pruning more would need Ruling 1 reopened: for example, letting a device's own
checkpoint shield its ops when no other device was enrolled in before(C). But that is
exactly the "forgotten while away" case the exception exists for.

**Nothing a provider claims skips a rebase.** The ops an offer lists above the snapshot
count as "the peer has them" only once they have actually delivered. Before this rule,
a provider could list one of the requester's ops and withhold its parent, and the op
was then neither delivered nor rebased.

**Nothing the user undid comes back.** An own set add whose tag the old state had
already removed, and an own edit to a row the old state had deleted, are listed, never
re-issued. The same rule applies to unsynced ops as well as base-folded ones. It follows
the OR-set and observed-remove semantics: an add or edit that a remove or delete
*observed* stays undone (re-issuing it would mint a tag the remove no longer observes).
An add or edit *concurrent* with the remove was not observed, so it is re-issued and
still wins.

## The model

`model/forget.als` gains two flags:

- **`Backing`**: `keepAt` becomes "every candidate not backed at c".
- **`Fallback`**: `adopt` and `learnWipe` may use a checkpoint the replica already
  holds when `syncGuard` fails.

With both flags and every v0.1 fix on, none of the following checks found a
counterexample within its scope:

| Check | Scope (devices / ops / states) |
|---|---|
| (a) convergence | 3 / 4 / 7 |
| (a′) compaction neutral | 3 / 4 / 7 |
| (b′) Forget except shielded | 3 / 4 / 7 |
| (d) finality | 3 / 4 / 7 |
| (f) Forget durable | 3 / 4 / 7 |
| (e1′) Q4, except what a wiped device handed over | 3 / 4 / 8 |
| (g) no silent loss | 3 / 4 / 8, no Forget |
| induction: step, ideal and final | 3 devices / 7 ops |

Both flags are reachable (`reach_fallback`, and `reach_backed` at 9 states). The v0.1
induction checks now pin both flags off, so their recorded results still reproduce.

All runs used Alloy 6.2.0 under a 4 GiB memory cap: glucose for the trace checks and
`ind_step2` (198 s; SAT4J did not finish in 570 s), and SAT4J for `ind_ideal2` and
`ind_final2`. Every clean result means "no counterexample within this scope", not a
proof.

**Not modelled:** BaseRebase for kinds other than Put (the model has only Puts), and
the `Pruned` content.

## Consequences

- The checkpoint hash and the snapshot's `state` changed meaning: the base state, not
  the fold. Checkpoints written by v0.1 do not verify (there are no users).
- The horizon property now also writes restores, set removes and appends. It checks
  that every op ever written, checkpoints aside, is still delivered on some replica or
  was re-issued or listed, and that no sync fails. That is how it found a real bug in
  the first version of the committed split: a checkpoint body kept at authoring was
  behind the base on a later compaction, which then failed with `CheckpointMismatch`.
- Items from ADR 0006's list of departures that still stand:
  - 2: BaseRebase's Q4 test reads the index.
  - 3: re-enrolment.
  - 7: reporting.
  - 8: pending ops, the relay and per-replica "old".

  Item 1 (the stall) and item 5 (the uncommitted split) are closed. Item 4 (BaseRebase's
  scope) is closed. Item 6 (the KeepBodies cost) is reduced to the lone-device case.
- *v1:* adoption now checks, before it touches anything, that it will be able to
  author: a device whose log runs more than the skew bound ahead of its clock (its
  clock ran ahead, then was put right) gets `ClockBehind` and keeps its log as it is.
  Before, it installed the snapshot, then failed in the rebase, and its unsynced
  edits and its own enrolment were in neither the log nor the review list. Found by
  the persistence property (ADR 0010), pinned by
  `a_device_whose_clock_is_behind_its_log_refuses_a_snapshot_instead_of_losing_edits`.
- *v1:* BaseRebase now applies the v0.2.1 rule too: an own base-folded edit that a
  live delete observed is listed as `RowDeleted`, never re-issued, even when a later
  edit by another device keeps the row visible. The base path used to ask only
  whether the row was visible. The horizon property hit this about once in 15,000
  cases, and it reproduced at v0.2.1. Pinned by
  `base_rebase_does_not_reissue_an_edit_a_live_delete_observed_when_the_row_is_visible`.
