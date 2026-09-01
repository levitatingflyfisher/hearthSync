/*
 * forget.als — a small-scope model of hearthSync's Forget, checkpoint and horizon rules
 * (kernel v0, ADRs 0002, 0004, 0005; design §2.5, §3.4, §4; ruling Q12).
 *
 * What is modelled (partial on purpose, aimed at the risky part):
 *   - replicas = devices; each holds a down-closed delivered set (index entries included,
 *     as `store.contains` sees them), an optional pruned base, and a wiped flag;
 *   - ops are static atoms with an author, parents and a kind; HLC is a total order on
 *     ops consistent with parents (V3); authoring puts parents = heads (so before(op) is
 *     exactly what the author had delivered) and stamps after everything delivered;
 *   - Put (one field; the fold is abstracted as the set of included puts, which is what
 *     State keeps per row: every put id plus the LWW winner), Checkpoint (commits to the
 *     author's folded state), Forget (target + cut), Enroll (V4);
 *   - delivery in any order from any peer that still holds the body, with the v0
 *     reconcile guard (a side lacking the other's base must pull a snapshot first);
 *   - compaction at an old checkpoint (fold_at: base state + ops in before(C), applying only
 *     the Forgets in before(C), hash-checked), snapshot adoption and the Q4 rebase
 *     (re-issue a field edit only if its winner is unchanged, else review; foreign unsynced
 *     ops re-offered as they are; own non-Put ops dropped to review);
 *   - time only as the monotone set Old of checkpoints past the horizon.
 * Not modelled: signatures and hashing (assumed sound), quarantine (V8 only delays),
 * Delete/Restore/sets/streams, the relay, pending ops (dropped instead of parked).
 *
 * The proposed fixes are switched on by the presence of the lone flag sigs below, so every
 * assertion is checked twice: against kernel v0 and against v0 plus the fixes. The
 * convergence family is also checked by one-step induction over an invariant (end of file),
 * which reaches a larger op scope than the trace checks.
 * A clean check means "no counterexample within the stated scope", never "proved".
 * Results, scopes and the counterexamples: docs/adr/0006-forget-and-horizon-model.md; the v0.2
 * flags (Backing, Fallback) and their checks: docs/adr/0007-committed-split-and-fallback.md.
 * Run (Alloy 6.2, the official dist jar):
 *   java -jar alloy.jar exec -s sat4j -t text -o <outdir> -c <command> model/forget.als
 */
module forget

open util/ordering[Op] as H

sig Device {
  var delivered: set Op,     -- ids this replica has (bodies or index entries)
  var base: lone Checkpoint, -- pruned base checkpoint
  var baseIncl: set Put,     -- puts folded into the base state (stored at compaction/adoption)
  var keep: set Put,         -- FIX KeepBodies: covered ops whose bodies are kept and folded live
  var toRebase: set Op,      -- own unsynced puts (with the fixes: lost base winners, lost Forgets) awaiting rebase
  var review: set Op         -- the rebase review list
}
some sig Genesis in Device {} -- enrolled before the modelled window
sig Field {}

abstract sig Op {
  author: one Device,
  parents: set Op,
  anc: set Op        -- before(op), materialised once: anc = ^parents
}
sig Put extends Op { field: one Field, reissueOf: lone Put, var lateOf: set Checkpoint }
sig Checkpoint extends Op { commits: set Put } -- state_hash abstracted: the included puts
sig Forget extends Op { target: one Device, cut: set Op, refOf: lone Forget }
sig Enroll extends Op { enrolls: one Device }

var sig Made in Op {}          -- ops authored so far
var sig Old in Checkpoint {}   -- checkpoints older than the horizon (time passes)
var sig Wiped in Device {}
var sig Late in Put {}         -- past-horizon edits (see lateUpdate)
var sig Pick in Op {}          -- unconstrained per step: the batch a sync step moves or a tick ages

-- fix flags -------------------------------------------------------------------------
lone sig Shield {}        -- G2a: a checkpoint shields its past from Forgets it lacks
lone sig AgedShield {}    -- G2a, option B: only a checkpoint past the horizon shields
lone sig KeepBodies {}    -- G2b: compaction keeps bodies of excluded ops and of the checkpointer's ops
lone sig AgePull {}       -- E1a: pull-first on any checkpoint past the horizon, pruned or not
lone sig ForeignReview {} -- E1b: foreign unsynced ops go to review on adoption, never re-offered as-is
lone sig OriginClock {}   -- E2:  a re-issued edit keeps its original clock for LWW
lone sig WipedPush {}     -- F:   a wiped (or about-to-be-wiped) replica never adopts; it hands its log over as-is
lone sig BaseRebase {}    -- G:   adoption also rebases winning values in the old base state the peer lacks
lone sig Reforget {}      -- F2:  adoption re-authors every Forget it knew (own or not, pruned or not) that the snapshot lacks
-- v0.2 (ADR 0007)
lone sig Backing {}       -- KeepBodies made a function of upto(c): fold into the base exactly the ops backed by a
                          -- checkpoint in upto(c) by another device in whose past they were not excluded; keep the rest
lone sig Fallback {}      -- a replica that lacks an old checkpoint the peer holds may adopt a snapshot at a checkpoint
                          -- it already holds (the provider's base), so AgePull never stalls

pred v0 { no Induct and no AgedShield and no Shield and no KeepBodies and no AgePull and no ForeignReview and no OriginClock and no WipedPush
  and no BaseRebase and no Reforget and no Backing and no Fallback }
pred fixG2 { some Shield and some KeepBodies }
pred fixE { some AgePull and some ForeignReview and some OriginClock }
pred fixes { fixG2 and fixE and some WipedPush and some BaseRebase and some Reforget }
pred fixAll { no Induct and no AgedShield and fixes and no Backing and no Fallback }
pred fixAllB { no Induct and some AgedShield and fixes and no Backing and no Fallback }
pred fixAll2 { no Induct and no AgedShield and fixes and some Backing and some Fallback }

-- static structure ------------------------------------------------------------------
fact ancestry { anc = ^parents }
fun past[o: Op]: set Op { o.anc }
fun upto[o: set Op]: set Op { o + o.anc }
pred conc[a, b: Op] { a != b and a not in past[b] and b not in past[a] }

fact V3 { all o: Op, p: o.parents | H/lt[p, o] }
fact V4 {
  all o: Op - Enroll | o.author in Genesis or some e: Enroll & past[o] | e.enrolls = o.author
  all e: Enroll | e.author in Genesis or e.enrolls = e.author
                  or some e2: Enroll & past[e] | e2.enrolls = e.author
}
fact V5 { all f: Forget | f.cut in past[f] and f.cut.author in f.target }
fact reissueShape {
  all n: Put | some n.reissueOf implies {
    n.reissueOf.field = n.field and n.reissueOf.author = n.author
    no n.reissueOf.reissueOf and H/lt[n.reissueOf, n]
  }
}

-- exclusion (V6) --------------------------------------------------------------------
-- plain V6: F's target wrote u and u is not in before(cut). Enroll and Forget are exempt.
pred cutOut[F: Forget, u: Op] {
  u in Put + Checkpoint and u.author = F.target and u not in upto[F.cut]
}
-- FIX Shield: a checkpoint C shields u from F when u is in before(C) and F is not, unless C
-- itself was written by F's target outside the cut (a forgotten device cannot shield its
-- own post-cut ops with its own automatic checkpoint). Variant AgedShield (Ruling 1, option
-- B) lets only a checkpoint past the horizon shield: exactly the design's "cannot reach
-- behind a pruned checkpoint", at the price of an excluded op returning when C ages.
pred shielded[F: Forget, u: Op, D: set Op] {
  some Shield and some c: Checkpoint & D & (some AgedShield => Old else Checkpoint) |
    u in past[c] and F not in upto[c] and not cutOut[F, c]
}
-- u is excluded given the delivered set D
pred excl[u: Op, D: set Op] { some F: Forget & D | cutOut[F, u] and not shielded[F, u, D] }

fun covered[r: Device]: set Op { upto[r.base] }
fun held[r: Device]: set Op { (r.delivered - covered[r]) + r.base + r.keep }
fun live[r: Device]: set Put { Put & ((r.delivered - covered[r]) + r.keep) }
-- the folded state (as included puts): the stored base plus a live fold of retained bodies
fun state[r: Device]: set Put { r.baseIncl + { u: live[r] | not excl[u, r.delivered] } }

-- fold_at: the state at checkpoint c from p's base, applying only what is in before(c)
fun cand[p: Device, c: Checkpoint]: set Put { Put & (p.keep + (upto[c] - covered[p])) }
fun exAt[p: Device, c: Checkpoint]: set Put { { u: cand[p, c] | excl[u, upto[c]] } }
fun foldAt[p: Device, c: Checkpoint]: set Put { p.baseIncl + (cand[p, c] - exAt[p, c]) }
-- backed at c: some checkpoint in upto(c), by another device, has u in its past and did not exclude it
pred backed[u: Op, c: Checkpoint] {
  some c2: Checkpoint & upto[c] | u in past[c2] and u.author != c2.author and not excl[u, upto[c2]]
}
fun keepAt[p: Device, c: Checkpoint]: set Put {
  some KeepBodies => (some Backing => { u: cand[p, c] | not backed[u, c] }
                      else exAt[p, c] + (cand[p, c] & c.author.~author)) else none
}

-- LWW ------------------------------------------------------------------------------
fun origin[o: Put]: Put { some o.reissueOf => o.reissueOf else o }
fun key[o: Put]: Put { some OriginClock => origin[o] else o }
pred beats[w, q: Put] { H/lt[key[q], key[w]] or (key[q] = key[w] and H/lt[q, w]) }
fun winner[S: set Put, f: Field]: lone Put {
  { w: S & field.f | all q: (S & field.f) - w | beats[w, q] }
}
fun wins[r: Device]: set Put { { w: state[r] | w = winner[state[r], w.field] } }
-- what an edit saw as the field's winner when written (winners_seen)
fun seenWin[o: Put]: lone Put { H/max[past[o] & Put & field.(o.field)] }

-- events ---------------------------------------------------------------------------
pred init {
  no lateOf and no delivered and no base and no baseIncl and no keep and no toRebase and no review
  no Made and no Old and no Wiped and no Late
}

pred sameRest { base' = base and baseIncl' = baseIncl and keep' = keep and review' = review }

fun heads[D: set Op]: set Op { D - D.anc }
fun wipeOf[r: Device, D: set Op]: set Device { (some F: Forget & D | F.target = r) => r else none }

pred canAuthor[r: Device, o: Op] {
  r not in Wiped and no r.toRebase
  o not in Made and o.author = r
  o.parents = heads[r.delivered]
  all d: r.delivered | H/lt[d, o]
}
pred addOwn[r: Device, o: Op] {
  Made' = Made + o
  delivered' = delivered + r -> o
  toRebase' = toRebase and Old' = Old and sameRest
  Wiped' = Wiped + wipeOf[r, o]   -- forgetting yourself wipes on local delivery (c)
}
pred write[r: Device, o: Put] { canAuthor[r, o] and no o.reissueOf and addOwn[r, o] }
pred checkpoint[r: Device, c: Checkpoint] { canAuthor[r, c] and c.commits = state[r] and addOwn[r, c] }
pred forget[r: Device, f: Forget] {
  canAuthor[r, f] and no f.refOf and f.cut = r.delivered & f.target.~author and addOwn[r, f]
}
-- FIX Reforget: the device holds the phrase (Q1), so it re-signs the Forget with a cut
-- recomputed on the new heads
pred reforget[r: Device, o, n: Forget] {
  some Reforget and o in r.toRebase and r not in Wiped
  n not in Made and n.refOf = o and n.author = r and n.target = o.target
  n.cut = r.delivered & n.target.~author
  n.parents = heads[r.delivered] and (all d: r.delivered | H/lt[d, n])
  Made' = Made + n and delivered' = delivered + r -> n
  toRebase' = toRebase - r -> o
  Old' = Old and sameRest
  Wiped' = Wiped + wipeOf[r, n]
}
pred enroll[r: Device, e: Enroll] { canAuthor[r, e] and addOwn[r, e] }

pred syncGuard[r, p: Device] {
  -- reconcile: a side that lacks the other's base pulls a snapshot first
  (no p.base or p.base in r.delivered) and (no r.base or r.base in p.delivered)
  -- FIX AgePull: the same for any checkpoint past the horizon, pruned or not
  some AgePull implies ((Old & p.delivered) in r.delivered and (Old & r.delivered) in p.delivered)
}
-- one sync step: r takes any parent-closed batch X of bodies p holds (any order overall)
-- FIX WipedPush: what a wiped replica may hand over as-is: its own ops, and its Forgets
-- with their causal past (so the Forgets can be delivered)
fun handover[p: Device]: set Op { p.delivered & (p.~author + upto[Forget & p.~author]) }
pred deliver[r, p: Device, X: set Op] {
  r != p and no r.toRebase and no p.toRebase
  some X and X in held[p] - r.delivered and X.parents in r.delivered + X
  -- (a handover never brings an old checkpoint: the receiver must take that as a snapshot and rebase)
  syncGuard[r, p] or (some WipedPush and p in Wiped and X in handover[p] and no (X & Old))
  delivered' = delivered + r -> X
  Made' = Made and Old' = Old and toRebase' = toRebase and sameRest
  Wiped' = Wiped + wipeOf[r, X]
}
pred compact[r: Device, c: Checkpoint] {
  no r.toRebase
  c in Old & held[r] and c != r.base
  no F: Forget & r.delivered | cutOut[F, c]  -- an excluded checkpoint is never a base
  no r.base or r.base in past[c]
  foldAt[r, c] = c.commits                   -- CheckpointMismatch otherwise
  base' = (base - r -> univ) + r -> c
  baseIncl' = (baseIncl - r -> univ) + r -> (foldAt[r, c] - keepAt[r, c])
  keep' = (keep - r -> univ) + r -> keepAt[r, c]
  delivered' = delivered and Made' = Made and Old' = Old and Wiped' = Wiped
  toRebase' = toRebase and review' = review
}
-- adopt a snapshot at c from p, then (in later steps) rebase
pred adopt[r, p: Device, c: Checkpoint] {
  r != p and no r.toRebase and no p.toRebase
  some WipedPush implies (r not in Wiped and r not in wipeOf[r, p.delivered])
  c not in r.delivered or (some Fallback and not syncGuard[r, p])
  c = p.base or (some AgePull and c in Old & held[p] and (no p.base or p.base in past[c]))
  foldAt[p, c] = c.commits                    -- snapshot verification
  let D0 = p.delivered,
      uns = (r.delivered - D0) & held[r],
      own = uns & r.~author,
      foreign = uns - own,
      fok = { f: foreign | past[f] - D0 in foreign and no ForeignReview },
      lost = (some BaseRebase => (r.baseIncl & wins[r]) - D0 else none),
      rf = (some Reforget => (Forget & r.delivered) - D0 else none) {
    delivered' = (delivered - r -> univ) + r -> (D0 + fok)
    base' = (base - r -> univ) + r -> c
    baseIncl' = (baseIncl - r -> univ) + r -> (foldAt[p, c] - keepAt[p, c])
    keep' = (keep - r -> univ) + r -> keepAt[p, c]
    toRebase' = (toRebase - r -> univ) + r -> ((own & Put) + (lost & r.~author) + rf)
    review' = (review - r -> univ) + r -> (r.review + (own - Put - rf)
                                            + (foreign - fok) + (lost - r.~author))
    Wiped' = Wiped + wipeOf[r, D0 + fok]
  }
  Made' = Made and Old' = Old
}
-- FIX WipedPush: a replica that would be forgotten by the snapshot wipes from it but keeps
-- its own log (so the Forgets and ops only it holds can still be handed over)
pred learnWipe[r, p: Device, c: Checkpoint] {
  some WipedPush and r != p and no r.toRebase and no p.toRebase and r not in Wiped
  (c not in r.delivered or (some Fallback and not syncGuard[r, p])) and r in wipeOf[r, p.delivered]
  c = p.base or (some AgePull and c in Old & held[p] and (no p.base or p.base in past[c]))
  Wiped' = Wiped + r
  delivered' = delivered and Made' = Made and Old' = Old and toRebase' = toRebase and sameRest
}
fun expected[r: Device, o: Put]: lone Put {
  let sw = seenWin[o], ms = Made & reissueOf.sw & r.~author | some ms => ms else sw
}
pred q4ok[r: Device, o: Put] { winner[state[r], o.field] = expected[r, o] }
pred reissue[r: Device, o, n: Put] {
  o in r.toRebase and r not in Wiped and q4ok[r, o]
  n not in Made and n.reissueOf = o and n.author = r
  n.parents = heads[r.delivered] and (all d: r.delivered | H/lt[d, n])
  Made' = Made + n and delivered' = delivered + r -> n
  toRebase' = toRebase - r -> o
  Old' = Old and Wiped' = Wiped and sameRest
}
pred toReview[r: Device, o: Op] {
  o in r.toRebase and (r in Wiped or (o in Put and not q4ok[r, o]))
  toRebase' = toRebase - r -> o and review' = review + r -> o
  delivered' = delivered and Made' = Made and Old' = Old and Wiped' = Wiped
  base' = base and baseIncl' = baseIncl and keep' = keep
}
pred tick { some Pick and Pick in (Made & Checkpoint) - Old and (Pick.anc & Checkpoint) in Old + Pick
  Old' = Old + Pick
  delivered' = delivered and Made' = Made and Wiped' = Wiped and toRebase' = toRebase and sameRest }
pred stutter { delivered' = delivered and Made' = Made and Old' = Old and Wiped' = Wiped
  toRebase' = toRebase and sameRest }

-- an edit is late (past the horizon) once some checkpoint concurrent with it is old while
-- no replica holds both: its author was away past the horizon when it came back
pred lateUpdate {
  lateOf' = lateOf + { o: Put & Made', c: Old' | conc[c, o] and no r: Device | c + o in r.delivered' }
  Late = lateOf.Checkpoint
}

pred step {
  stutter or tick or some r: Device | {
    (some o: Put | write[r, o]) or (some c: Checkpoint | checkpoint[r, c])
    or (some f: Forget | forget[r, f]) or (some e: Enroll | enroll[r, e])
    or (some p: Device | deliver[r, p, Pick])
    or (some c: Checkpoint | compact[r, c])
    or (some p: Device, c: Checkpoint | adopt[r, p, c])
    or (some p: Device, c: Checkpoint | learnWipe[r, p, c])
    or (some o, n: Put | reissue[r, o, n]) or (some o: Op | toReview[r, o])
    or (some o, n: Forget | reforget[r, o, n]) }
}
-- Traces start from init, except in the one-step induction checks (flag Induct), which
-- start from any state satisfying the invariant.
lone sig Induct {}
fact traces { no Induct implies (init and always step) }

-- properties -----------------------------------------------------------------------
-- (a) convergence: same delivered ops => same state (hence same excluded data ops)
pred convergent { always all r1, r2: Device | r1.delivered = r2.delivered implies state[r1] = state[r2] }
-- (a') compaction never changes a replica's own state
pred compactionNeutral { always all r: Device, c: Checkpoint | compact[r, c] implies (state[r])' = state[r] }
-- (b) strict: once r has a Forget, no op of the target outside the cut is in r's state
pred forgetExcludes { always all r: Device, F: Forget & r.delivered | no u: state[r] | cutOut[F, u] }
-- (c) forgetting yourself (or receiving your own Forget) wipes at once; wiped never author
pred selfWipe { always all r: Device | (some F: Forget & r.delivered | F.target = r) implies r in Wiped }
-- (d) finality: nothing a replica pruned in as included is excluded by a replica that knows at least as much
pred finality { always all r1, r2: Device | no u: r1.baseIncl |
  r1.delivered in r2.delivered and u in r2.delivered - state[r2] }
-- (e1) Q4 at re-entry: when a past-horizon edit (or its re-issue) first meets the old
-- checkpoint it missed, at a replica where it becomes the field's winner, the value it
-- displaces is one its author saw ("fields nobody else changed"). Later propagation of that
-- op is ordinary concurrency and is judged by (e2).
pred q4Seen { always all r: Device, w: Put, c: Old & r.delivered | let S = state[r] & field.(w.field) |
  (w = winner[S, w.field] and c in origin[w].lateOf
    and before (no r2: Device | c + w in r2.delivered))  -- quantified atoms: `before` sees the same ops
  implies (let ru = winner[S - w, w.field] | no ru or origin[ru] in upto[origin[w]]) }
-- (e2) an old edit never beats, by LWW, a write newer (by clock) than itself that it did not see
pred q4Clock { always all r: Device, w: Put | let S = state[r] & field.(w.field) |
  (w = winner[S, w.field] and origin[w] in Late) implies
    no q: S - w | H/lt[origin[w], origin[q]] and origin[q] not in upto[origin[w]] }
-- (f) a Forget, once authored, is never lost from the household
pred forgetDurable { always all F: Forget | (some r: Device | F in r.delivered)
  implies always ((some r: Device | F in r.delivered + r.toRebase) or some (Made & refOf.F)) }
-- (g) a value that is a field's winner somewhere is never lost silently in one step: some
-- replica still has it, or it sits on a review/rebase list, or it was re-issued
pred noSilentLoss { always all u: Put | (some r: Device | u in wins[r]) implies
  after ((some r: Device | u in r.delivered + r.review + r.toRebase) or some (Made & reissueOf.u)) }
-- (b') (b) with the design's one exception made explicit: a checkpoint that the Forget did
-- not see freezes its past ("a Forget cannot reach behind a checkpoint", design §4)
pred forgetExcludesUnlessShielded { always all r: Device, F: Forget & r.delivered |
  all u: state[r] | cutOut[F, u] implies shielded[F, u, r.delivered] }
-- (e1') (e1) except for what a wiped device handed over as-is (its own edits and its Forgets' past)
pred q4SeenExceptWiped { always all r: Device, w: Put, c: Old & r.delivered | let S = state[r] & field.(w.field) |
  (w = winner[S, w.field] and c in origin[w].lateOf
    and before (no r2: Device | c + w in r2.delivered)
    and origin[w] not in (Wiped.~author + upto[Forget & Wiped.~author]))
  implies (let ru = winner[S - w, w.field] | no ru or origin[ru] in upto[origin[w]]) }

-- checks: v0 ------------------------------------------------------------------------
-- Late is bookkeeping for (e) only; it is constrained only where (e) is checked.
check a_v0  { v0 implies convergent }  for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
check a2_v0 { v0 implies compactionNeutral }  for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
check b_v0  { v0 implies forgetExcludes }  for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
check c_v0  { v0 implies selfWipe }  for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
check d_v0  { v0 implies finality }  for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
check e1_v0 { (v0 and always lateUpdate) implies q4Seen }  for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
check e2_v0 { (v0 and always lateUpdate) implies q4Clock }  for 3 but 4 Op, 1 Field, 0 Forget, 0 Enroll, 9..9 steps
check f_v0  { v0 implies forgetDurable }  for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
check g_v0  { v0 implies noSilentLoss }  for 3 but 4 Op, 1 Field, 0 Forget, 0 Enroll, 8..8 steps

-- checks: v0 + all proposed fixes ---------------------------------------------------
-- These are UNSAT (no counterexample) checks, far dearer than finding one: the scope is what
-- one solve finishes in under ten minutes on this box (SAT4J). Enroll is left out where V4
-- plays no part; the horizon checks leave out Forget. Traces of exactly N states include
-- every shorter trace, since stuttering is always allowed.
check a_fix   { fixAll implies convergent }        for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check a2_fix  { fixAll implies compactionNeutral } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check b_fix   { fixAll implies forgetExcludes }    for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check b2_fix  { fixAll implies forgetExcludesUnlessShielded } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check c_fix   { fixAll implies selfWipe }          for 3 but 4 Op, 1 Field, 7..7 steps
check d_fix   { fixAll implies finality }          for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check e1_fix  { (fixAll and always lateUpdate) implies q4Seen }  for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
check e1w_fix { (fixAll and always lateUpdate) implies q4SeenExceptWiped } for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
check e2_fix  { (fixAll and always lateUpdate) implies q4Clock } for 3 but 4 Op, 1 Field, 0 Forget, 0 Enroll, 9..9 steps
check f_fix   { fixAll implies forgetDurable }     for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check g_fix   { fixAll implies noSilentLoss }      for 3 but 4 Op, 1 Field, 0 Forget, 0 Enroll, 8..8 steps

-- non-vacuity: the fixed model still reaches every mechanism the checks depend on
run reach_shield  { fixAll and eventually (some r: Device, F: Forget & r.delivered, u: state[r] | cutOut[F, u]) } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
run reach_keep    { fixAll and eventually (some r: Device | some r.base and some r.keep) } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
run reach_exclude { fixAll and eventually (some r: Device | some r.base and some u: live[r] | excl[u, r.delivered]) } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
run reach_ageadopt { fixAll and eventually (some r, p: Device, c: Checkpoint | no p.base and adopt[r, p, c]) } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
run reach_reissue { fixAll and always lateUpdate and eventually (some n: Made & Put | some n.reissueOf) } for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
run reach_review  { fixAll and eventually (some review & Device -> Put) } for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
run reach_wipedpush { fixAll and eventually (some r, p: Device | deliver[r, p, Pick] and not syncGuard[r, p]) } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps

-- induction (fixed model): an invariant that makes each replica's state equal the fold an
-- unpruned replica with the same delivered set would compute, checked to hold after any
-- one step from any state satisfying it. Two states only, so the op scope can be larger.
fun ideal[D: set Op]: set Put { { u: Put & D | not excl[u, D] } }
pred inv {
  Made.anc in Made
  Old in Made and Old.anc & Checkpoint in Old
  all r: Device {
    r.delivered in Made and r.delivered.anc in r.delivered
    r.base in r.delivered & Old
    r.baseIncl + r.keep in Put & covered[r]
    no r.baseIncl & r.keep
    Put & covered[r] in r.baseIncl + r.keep
    -- each op folded into a base is backed by a checkpoint the replica holds, written by
    -- someone else, in whose past the op was not excluded
    all u: r.baseIncl | some c: Checkpoint & r.delivered & Old |
      u in past[c] and u.author != c.author and not excl[u, upto[c]]
  }
}
check ind_step  { (some Induct and no AgedShield and fixes and no Backing and no Fallback and inv and step) implies after inv } for 3 but 7 Op, 1 Field, 2..2 steps
run ind_reach   { some Induct and no AgedShield and fixes and no Backing and no Fallback and inv and step and after inv and
  (some r: Device, c: Checkpoint | compact[r, c] and some keepAt[r, c] and some r.baseIncl') } for 3 but 7 Op, 1 Field, 2..2 steps
check ind_ideal { (some Induct and no AgedShield and fixes and no Backing and no Fallback and inv) implies all r: Device | state[r] = ideal[r.delivered] } for 3 but 7 Op, 1 Field, 1..1 steps
check ind_final { (some Induct and no AgedShield and fixes and no Backing and no Fallback and inv) implies all r1, r2: Device |
  r1.delivered in r2.delivered implies r1.baseIncl in state[r2] } for 3 but 7 Op, 1 Field, 1..1 steps
-- the same invariant fails for v0 (compaction drops excluded ops and re-includes cut-out ones)
check ind_step_v0 { (some Induct and no AgedShield and no Shield and no KeepBodies and inv and step) implies after inv } for 3 but 5 Op, 1 Field, 2..2 steps

-- Ruling 1, option B (AgedShield): the convergence family again, by traces and induction
check a_fixB  { fixAllB implies convergent }        for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check a2_fixB { fixAllB implies compactionNeutral } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check b_fixB  { fixAllB implies forgetExcludes }    for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check b2_fixB { fixAllB implies forgetExcludesUnlessShielded } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check d_fixB  { fixAllB implies finality }          for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check ind_stepB  { (some Induct and some AgedShield and fixes and no Backing and no Fallback and inv and step) implies after inv } for 3 but 7 Op, 1 Field, 2..2 steps
check ind_idealB { (some Induct and some AgedShield and fixes and no Backing and no Fallback and inv) implies all r: Device | state[r] = ideal[r.delivered] } for 3 but 7 Op, 1 Field, 1..1 steps
check ind_finalB { (some Induct and some AgedShield and fixes and no Backing and no Fallback and inv) implies all r1, r2: Device |
  r1.delivered in r2.delivered implies r1.baseIncl in state[r2] } for 3 but 7 Op, 1 Field, 1..1 steps

-- v0.2 (ADR 0007): the fixes plus Backing and Fallback ------------------------------
check a_fix2   { fixAll2 implies convergent }        for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check a2_fix2  { fixAll2 implies compactionNeutral } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check b2_fix2  { fixAll2 implies forgetExcludesUnlessShielded } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check d_fix2   { fixAll2 implies finality }          for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check e1w_fix2 { (fixAll2 and always lateUpdate) implies q4SeenExceptWiped } for 3 but 4 Op, 1 Field, 0 Enroll, 8..8 steps
check f_fix2   { fixAll2 implies forgetDurable }     for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
check g_fix2   { fixAll2 implies noSilentLoss }      for 3 but 4 Op, 1 Field, 0 Forget, 0 Enroll, 8..8 steps
run reach_fallback { fixAll2 and eventually (some r, p: Device, c: Checkpoint & r.delivered | adopt[r, p, c]) } for 3 but 4 Op, 1 Field, 0 Enroll, 7..7 steps
run reach_backed   { fixAll2 and eventually (some r: Device | some r.base and some (r.baseIncl & r.base.author.~author)) } for 3 but 4 Op, 1 Field, 0 Enroll, 0 Forget, 9..9 steps
check ind_step2  { (some Induct and no AgedShield and fixes and some Backing and some Fallback and inv and step) implies after inv } for 3 but 7 Op, 1 Field, 2..2 steps
check ind_ideal2 { (some Induct and no AgedShield and fixes and some Backing and some Fallback and inv) implies all r: Device | state[r] = ideal[r.delivered] } for 3 but 7 Op, 1 Field, 1..1 steps
check ind_final2 { (some Induct and no AgedShield and fixes and some Backing and some Fallback and inv) implies all r1, r2: Device |
  r1.delivered in r2.delivered implies r1.baseIncl in state[r2] } for 3 but 7 Op, 1 Field, 1..1 steps
