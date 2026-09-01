# ADR 0004: Validity rules and the clock bound

Status: accepted (v0, 2026-09-28)

## Decision

An op is delivered once all its parents are, then validated:

| Rule | Check | Depends on | On failure |
|---|---|---|---|
| V1 | ≤ 64 KiB, canonical dCBOR, closed schema | the op | reject |
| V2 | strict Ed25519 under `device` | the op | reject |
| V3 | `hlc` strictly after every parent's | before(u) | reject |
| V4 | device enrolled in before(u) (or u enrolls itself); Enroll/Forget signed by this household's enroll key | before(u) | reject |
| V5 | observed / restored / cut ids in before(u), on the same row, element or device | before(u) | reject |
| V6 | author forgotten and u outside before(cut) | delivered Forgets | exclude |
| V7 | a declared name used with the wrong kind or type / an undeclared name (ADR 0009) | the op and the app schema | reject / hold |
| V8 | `hlc.millis` more than `MAX_FUTURE_SKEW_MS` (10 min) ahead of `now` | local time | quarantine |

A rejected op's descendants are rejected (`parent_rejected`); an op whose parent
exists nowhere stays pending and never delivers. V7 came in v1 (ADR 0009).

**Clock.** The DAG is the causal clock; the HLC only orders concurrent writes to
one field and gives history a human time. V3 forces an op's HLC above its
parents', so an ancestor never beats its descendant. `MAX_FUTURE_SKEW_MS` is 10
minutes. An op past it is quarantined, not rejected, and delivered once local
time reaches `hlc.millis − 10 min`; time only moves forward, so replicas
converge, and a phone whose clock is a year ahead parks its own ops instead of
winning every race. At creation time a replica refuses to stamp when its clock
trails the newest delivered op by more than the bound (`ClockBehind`).

## Lens verdicts

The clock bound was settled in the design (§3.2) with `lens-willow` (reject too-far
future entries, ten minutes; the widening for expiring capabilities never applies
since Decision 2 rejected expiring grants), `lens-crdt-study` (timestamps must be
consistent with causal order: V3) and `lens-byzantine-crdts` (a check on local
state must delay, not reject). v0 implements that verdict unchanged; the lenses
were not re-run for it.

## Consequences

- A test pins the quarantine edge to the millisecond, and another pins the
  creation-time guard.
- Rejections carry stable codes (`Reject::code`) shared with the vectors, so the
  relays and a sync-health screen can name them.
- *v1:* V4 and V5 no longer walk all of before(u). V4 reads a per-op set of the
  devices enrolled in its past (interned; rebuilt from the index, never stored). V5
  and the Forget cut walk back from the parents only down to the oldest id they look
  for, which is exact because V3 makes clocks rise from parent to child; a snapshot's
  index is checked for V3 for the same reason (`clock_not_after_parents`).
