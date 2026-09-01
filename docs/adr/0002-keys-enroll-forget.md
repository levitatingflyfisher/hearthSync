# ADR 0002: Device keys, the enroll key, and Forget

Status: accepted (v0, 2026-09-28)

## Context

Decision 2 keeps the shared BIP39 phrase as the household root and adds
"Forget this device". The operator's Q1 ruling: every device stores the 12
words, all holders are equal, and a forgotten device wipes its stored words and
derived keys when it next syncs, or at once if it forgets itself.

The dispatch described "a device key derived from the household root". The
design (§2.5) makes the device key random and chains its authority to the root
through an enrollment. v0 follows the design; this ADR records the reading.

## Decision

- **Device key**: a random Ed25519 key (32 bytes from the platform CSPRNG,
  passed in). Its public key is the device id. Deriving it from the seed would
  let every device, holding the same seed, forge every other device's writes.
- **Enroll key**: `HKDF-SHA256(seed64, salt = empty, info =
  "openhearth.<app>.enroll.v1")` used as an Ed25519 secret. This is the same
  construction and app-domain rule as `key_derivation.dart`. Its public key is
  the household's identity in the log; a replica rejects Enroll and Forget ops
  not signed by it (`bad_enroll_auth`).
- **Enroll**: `Enroll{device, label, auth}`. Because every device holds the
  phrase, a device can enroll itself; a device can also enroll another (pairing).
  An op is valid only if its device has an Enroll in before(op) (V4).
- **Forget**: `Forget{device, cut, auth}`. `cut` is the device's latest ops as
  the forgetter saw them. That device's ops outside before(cut) are **excluded**:
  kept in the DAG so honest descendants still deliver, left out of the fold.
- **Self-wipe**: a replica that delivers a Forget naming itself drops its signer
  and seed (both zeroised on drop) and can no longer author, by either path
  (`author`, or the bridge's `prepare`/`finish` with an external signature). The
  ingest report's `wiped` flag tells the app to delete the stored words; it is
  also set when the Forget arrives inside a snapshot. Forgetting yourself
  delivers the Forget locally first, so the wipe is immediate; the replica can
  still hand its log to a peer, keyless, so the Forget spreads.
- **Enroll and Forget are never excluded**, even when their author is later
  forgotten. Their authority is the enroll key, not the device.

## Lens verdicts

`lens-byzantine-crdts`, asked whether a forgotten device's late op should be
rejected (the dispatch's word) or excluded (V6). Summary of the verdict, in the
lens's terms (a paraphrase, not a quotation):

> Validity decided on anything outside before(u) can make correct replicas
> diverge. A post-cut op never has the Forget in its own past, and a replica
> that delivered it before the Forget arrived may already have honest children
> built on it. Rejecting it after the fact would drop those children on some
> replicas and not others. Exclusion is a deterministic function of the op and
> the delivered Forget set, and that set only grows, so it converges. The
> qualification that bears: before(u) is "one way" to get a consistent
> decision, and the alternative holds only while the criterion is monotonic.
> Exclusion by Forget is monotonic; making control-op validity depend on a
> later Forget would not be. Verdict: exclude, and never exclude Enroll/Forget.
> Rivals: consensus-ordered membership (MLS-style commits, Decision 3).

## Consequences

- "Rejected by others" in the dispatch is met as "excluded by others": the
  late op has no effect anywhere, and its bytes are kept.
- A lost phone that never syncs keeps the words until it does. Decision 2's
  "make a new phrase and re-pair" flow covers that rare case.
- A forgotten device that has not yet wiped still holds the phrase, so it can
  still sign Enroll or Forget ops. Accepted by Q1: anyone holding the words is
  equal. Only a new phrase ends that.
- A mistaken forget is repaired by pairing again with a new device key; the
  old device id stays forgotten.
- The Alloy model of Forget (ruling Q12) is ADR 0006 (`model/forget.als`). It
  found that v0's Forget diverges once pruning is involved, and that a
  self-Forget can be lost on adoption. Kernel v0.1 implements its changes.
- With the shield (ADR 0006, Ruling 1 option A), exclusion is no longer monotone
  in the delivered set: a checkpoint that lacks a Forget protects its past from it.
  It stays a deterministic function of the delivered set, so replicas still
  converge; validity (reject) is unchanged and still reads only before(u).
