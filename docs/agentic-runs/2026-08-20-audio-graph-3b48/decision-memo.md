# Decision memo: projection backlog scheduling and lane reconciliation (audio-graph-3b48)

Date: 2026-08-20. Inputs: `constraints.md` and `code-reality.md` in this
directory, the seed text for `audio-graph-3b48` (plus `464c`, `9751`, `ab10`,
`caad` for scope), and three independently produced designs (crash-safety
angle, simplest-MVP angle, live-latency angle), each carrying a self-declared
worst weakness. This memo feeds a human decision. It does not close the seed
and it does not start implementation.

## The decision in one sentence

Choose the scheduling semantics for the projection backlog: how the notes and
graph lanes select and coalesce bases, track lane-local coverage, prioritize
live work against repair and catch-up work, bound concurrency, retry, flush at
stop, and re-derive their state on reopen, so that `audio-graph-fbca` and
`audio-graph-44c1` can build on named, testable states instead of the current
implicit behavior.

## The shared floor: what every design independently converged on

All three designs, written from three deliberately different angles, arrived
at the same answers on six points. These are therefore not options. Whichever
option below is chosen, it includes all of the following.

1. **The caad apply-gate fix, identically specified three times.**
   `apply_validated_patch_with_speaker_timeline_opt` (projections.rs:2295-2322)
   stops calling the two-way `validate_basis_with_speaker_timeline`
   (projections.rs:801-806) and matches `classify_basis_currency` directly:
   `Current` applies, `AppendOnlyStale` applies and returns the appended tail
   so the caller schedules exactly one coalesced Background follow-up,
   `Revised` errors and routes to Replay repair. This is ADR-0031 as written
   (MUST #2, #4); the code-reality audit proves the current gate collapses the
   three-way classifier back to two-way and is the root cause of caad's
   22-of-23 discard rate under continuous speech. No coalescing or backlog
   redesign fixes caad without this change. Telemetry must also split the
   currently indistinguishable `MissingCurrentSpan` log line into
   applied-append-only versus discarded-revised.

2. **Accepted canonical events are the only authority, and the dead queue
   read path stays dead.** All three designs refuse to make
   `SchedulerQueueState` load-bearing (delete it, demote it to
   diagnostics-never-read, or replace it with an explicitly disposable hint).
   Nothing reads it back today outside tests, so this codifies reality
   (MUST #12, #14, #16).

3. **Reconciliation on open is the ordinary code path.** Rebuild the ledger
   from accepted `transcript_revisions`, rebuild materialized state and the
   per-kind coverage head by replaying accepted `projection_patches`, then
   call the normal `observe_ledger` once. No persisted stage field, no resume
   branch, no resurrected in-flight job; a persisted in-flight record can at
   most demote to a pending basis (MUST #24). This also answers `9751`'s open
   "transactional Resume versus approved startup recovery" question in favor
   of startup recovery by re-derivation.

4. **A JoinHandle registry and a stop-time flush.** `spawn_projection_job`
   registers its handle; `stop_capture_impl` promotes the pending basis to a
   job and joins within a named budget; residual deficit survives as ordinary
   derivable work for the next open. Today projection threads are fully
   detached and the last pending basis is silently dropped at stop.

5. **The unowned stall (MUST #21) gets an owner.** The Idle-forever branch at
   projection_scheduler.rs:355-357 is replaced by a named attempt budget
   (all three designs picked 3 attempts, styled after the p99-tuned constant
   at state.rs:1112-1128); exhaustion emits a typed no-progress signal and
   hands off to ADR-0036's `Finalization Blocked`, never silent Idle.

6. **Explicit boundary statements.** Per-attempt deadline stays with
   `audio-graph-21e9`; per-lane totals and the `Finalized` verdict stay with
   ADR-0036; output budgets and coverage accounting for long-Session
   refinement stay with `audio-graph-fbca` (MUST #20, non-goals list). Review
   is not a lane; it remains the read-only `load_session` path (MUST #15).
   Coverage heads advance by the applied patch's basis coverage, never by the
   current ledger head, so no accepted event is silently claimed covered.

One shared risk rides along with item 1 and belongs to every option: once the
gate applies `AppendOnlyStale` patches, two sequential append-only patches can
be individually valid yet describe overlapping content. Deduplication is
delegated to the existing retcon mechanism (ADR-0024 §4), which is unproven at
this volume. The failure mode changes from caad's silent loss to visible,
retcon-correctable duplication. `44c1` needs a deterministic fixture for this,
and the maintainer needs to accept the trade (question 2 below).

## The real options

The three designs collapse into two genuinely different architectures. The
crash-safety and live-latency designs, despite different emphases, converge on
the same structural move: make `ProjectionPriority` load-bearing, split repair
and catch-up work into lanes separate from live work, and bound non-live
admission. They differ only in parameters (2 versus 3 max in flight, whether a
disposable snapshot hint is persisted, backoff schedules), so they merge into
one option here with the persistence choice called out as a sub-decision. The
simplest-MVP design genuinely differs: it declines to introduce lanes at all.

### Option A: derived heads, lane equals kind (simplest-MVP, hardened)

**Mechanism.** Keep the existing capacity-one structure verbatim:
`in_flight: Option<ProjectionJob>` plus `pending_basis: Option<ProjectionBasis>`
per `ProjectionKind` (projection_scheduler.rs:158-159). Define the backlog as
a derived triple per kind: `coverage_head` (re-seeded on open from the last
accepted `projection_patches` record for that kind) plus the two existing
slots. Lane equals kind: notes and graph are already structurally independent
schedulers, so a graph failure structurally cannot gate notes coverage
(MUST #22 falls out for free). `ProjectionPriority` stays a telemetry tag.
Retry is event-driven only: the attempt counter from shared-floor item 5 is
re-checked on the next final revision and at the stop-time flush; no timer or
backoff thread. Add the shared floor and nothing else. One hardening beyond
the design as submitted, taken from the other two designs because it closes
this option's self-declared worst weakness: on open, materialized state is
rebuilt strictly by replaying the accepted patch log, never seeded from a
materialized snapshot that has not been validated against that log. This
removes the double-apply window (a patch applied and snapshotted but never
Accepted would otherwise be invisible to the coverage head yet present in the
snapshot, and a regenerated patch would duplicate it).

**What it costs.** Coverage recomputation on open is a full replay of accepted
patches, O(session length), with no sanctioned cache (the ADR-0029 gate for
such a cache is explicitly unresolved, MUST #27). Provenance is kept only for
accepted patches: coalescing overwrites `pending_basis` wholesale, so there is
no record of which intermediate bases were skipped. `44c1` can assert "no
accepted patch is silently discarded" but not "every basis was considered."
If the product later needs the latter, or needs more than one projection
workload per kind concurrently (batch import, background re-projection), the
capacity-one slot is replaced rather than extended and this decision reopens.
A crash still loses one in-flight generation per kind (one LLM call), which is
unavoidable under accepted-only authority. A Replay repair and live work share
the single per-kind slot, so a repair occupies capacity that live notes could
use; with the caad fix in place repairs should be rare (genuine revisions
only), but this is asserted, not enforced.

**Which constraints it strains.** None of the MUSTs. It leans hardest on the
ticket's own framing: "lane reconciliation without erasing provenance" is
answered narrowly (provenance of accepted output, via the accepted log and
retcon ops) rather than broadly (provenance of scheduling intent). It also
gives the thinnest possible answer to "priority for live versus older
finalization work": priority is deliberately not load-bearing, and the
implicit cap of 2 in-flight calls is the whole quota story.

### Option B: structured lanes with load-bearing priority (crash-safety and live-latency, merged)

**Mechanism.** Lanes become execution classes orthogonal to kind, reusing the
existing `ProjectionPriority` vocabulary: Realtime maps to Live, Background to
Catchup, Replay to Repair. Per kind, the backlog becomes a real structure: a
capacity-one Live slot (unchanged coalescing), one coalesced append-only
catch-up gap, and a bounded, deduplicated repair queue. Coverage heads are
watermarks re-derived from the accepted patch log on every open, exactly as in
Option A. The scheduling rule is strict lane precedence: Live starts
unconditionally into an empty slot; Catchup and Repair are admitted only into
idle Live capacity, with a shared non-Live in-flight token so backlog work
never doubles LLM egress (named constants: max 2 to 3 total in flight).
Preemption is result-fencing via an attempt or fence epoch, not thread kill;
a fenced result returns its ticket to the backlog with attempts unchanged.
Timed backoff (named schedule, roughly 2s/8s/30s) drives retries between
speech events. Sub-decision inside this option: persist a versioned,
explicitly disposable `SchedulerBacklogSnapshot` hint (tmp plus rename, in the
`persistence/scheduler_queue.rs` file `464c` already plans) that may only
narrow work on open and is discarded on any mismatch, versus persist nothing
and pay full recomputation every open. Both sub-variants keep the accepted
log as sole authority.

**What it costs.** Net-new architecture with no existing code to preserve
(the code-reality audit confirms lanes do not exist today and priority is
cosmetic). More named states and constants for `44c1` to fixture, which is
both a benefit (explicit taxonomy) and a cost (more surface to prove,
including lane admission, fencing epochs, starvation windows, and snapshot
discard rules). The two parent designs each declared the same shaped weakness
from opposite ends: strict Live precedence means Catchup and Repair can starve
for an entire continuous-speech session and drain only at stop, so at
finalization this option can hand `fbca` one large unbounded catch-up gap,
pushing the long-Session context problem onto exactly the seam ADR-0036 flags
as contested (C11). Bounding that gap requires a starvation escape hatch
(force-admit one non-Live token after a named window), which trades away the
headline live-latency guarantee and is itself a product call. The
crash-safety variant additionally showed that without a sanctioned cache the
open-time replay cost is worst exactly where crashes hurt most, and its cache
sits behind the unresolved ADR-0029 gate.

**Which constraints it strains.** MUST #27 if the snapshot sub-variant is
read as a glanceable persisted coverage field (defensible as an ADR-0029-class
disposable hint, but the constraint explicitly records that gate as
unresolved, so it needs an explicit maintainer or ADR ruling rather than an
assumption). The SHOULD bias toward minimal persisted state and re-derivation.
The fbca seam (unbounded stop-time gap) under MUST #20's contested-ownership
warning. It also forces the `464c`/`9751` scope question immediately, in
whichever direction the sub-decision goes.

## Comparison against the binding constraints and the caad defect

- **caad (MUST #2, #4).** Identical in both options; the fix is the shared
  floor, lives at the apply gate, and is orthogonal to the lane model. Neither
  option fixes caad better than the other. Both inherit the shared
  duplication-versus-loss trade noted above.
- **Authority and streams (MUST #12-14, #16-18).** Both options treat only
  Accepted events as durable authority and add no fifth stream. Option A adds
  zero persisted state, trivially satisfying disposability. Option B's
  snapshot sub-variant must clear the ADR-0029 disposable-derivative bar and
  the unresolved MUST #27 gate; its no-snapshot sub-variant is equivalent to
  Option A on this axis.
- **No shared total order (MUST #13).** Both satisfied; neither invents a
  cross-lane sequence. Option B must additionally prove its per-kind lane
  precedence never implies a cross-kind order.
- **Retry ownership and the stall (MUST #20-21).** Both give the stall an
  owner with named constants and hand exhaustion to `Finalization Blocked`.
  Option A's retry is event-driven only (no retry during a long mid-session
  silence until stop); Option B retries on a timer. This is a real behavioral
  difference the maintainer should rule on (question 5).
- **Notes gate (MUST #22).** Both satisfy it structurally via independent
  per-kind schedulers.
- **One code path on open (MUST #24).** Both satisfied by construction; both
  answer `9751` as startup recovery, not transactional resume.
- **Computed coverage (MUST #27).** Option A complies by paying full replay.
  Option B's snapshot sub-variant is the only place either option touches the
  unresolved gate.
- **Downstream needs (fbca, 44c1).** Both hand fbca a defined stop-time state
  and the same per-lane coverage definition. Option B hands fbca a potentially
  unbounded coalesced gap under continuous speech, which strains the contested
  C11 seam; Option A's stop-time state is at most one pending basis per kind,
  which is smaller but carries no more provenance. `44c1` gets a smaller,
  fully derivable state taxonomy from Option A and a richer but larger one
  from Option B.
- **Neighbor tickets.** Option A demotes `464c`'s queue store to
  diagnostics-or-descriptor scope and effectively closes `9751`'s design
  question; Option B's snapshot sub-variant preserves a rescoped `464c`
  (disposable hint, not authority) and still closes `9751`'s question the same
  way. Either way `464c` cannot ship as "durable queue the scheduler reads
  back as authority"; that direction is dead under MUST #12/#24 regardless of
  the option chosen (question 4).

## Recommendation

**Adopt Option A: the derived-head design with lane equals kind, plus the
shared floor and the replay-on-open hardening.** It wins for three reasons.
First, every element it contains is already demanded by the binding
constraints or by a filed defect: the caad gate fix, the stall owner, the
stop-time flush, and accepted-only reconciliation are obligations, not
choices, and Option A is those obligations with nothing speculative attached.
Second, it is the only option that resolves 3b48 without touching either
unresolved gate the constraint set explicitly flags: the ADR-0029/MUST #27
cache question stays closed and the contested fbca C11 seam is left exactly
where ADR-0036 put it, whereas Option B must either open the cache gate (for
its snapshot) or lean on the C11 seam (with its stop-time gap). Third, the
migration is one-directional in Option A's favor: the lane model, load-bearing
priority, and a real queue can be added later behind the same derived coverage
heads when a concrete trigger arrives (more than 2 concurrent LLM calls, a
per-token budget cap, batch re-projection, or a `44c1` requirement for
scheduling-intent provenance), while unwinding Option B back to Option A would
mean deleting shipped persistence and fixtures. The honest price is recorded
above: no timer-driven retry, no intermediate-basis provenance, and an
asserted rather than enforced claim that repairs stay rare once caad is fixed.
Questions 1, 2, and 5 below are the places where a maintainer answer could
overturn this recommendation; if question 1 comes back "yes, every basis must
be provably considered," Option B should be chosen now instead of retrofitted.

## Open questions only the maintainer can answer

These are product calls, not facts; the code audit and constraint set cannot
settle them.

1. **Provenance depth.** Does the acceptance contract (`44c1`) or any product
   promise require asserting "every basis was considered," covering
   intermediate bases that coalescing overwrites, or is "no accepted patch is
   silently discarded" the full provenance obligation? The former forces
   Option B (a real queue) now; the latter permits Option A.
2. **Duplication tolerance.** With the caad fix, live notes can transiently
   show duplicated content across overlapping append-only patches until retcon
   corrects it, instead of silently losing patches as today. Is
   visible-then-corrected duplication acceptable live UX, or must the apply
   path be made keyed and idempotent before the gate fix ships?
3. **Reopen latency budget.** Coverage must be recomputed by replaying the
   accepted patch log on every open, O(session length), and the cache that
   would amortize it sits behind the unresolved ADR-0029 gate. What session
   length and open-time stall is acceptable before the maintainer would rather
   reopen ADR-0029 for a coverage cache?
4. **Fate of 464c and 9751.** Every design and this memo make the durable
   scheduler-queue read path permanently non-authoritative, and answer 9751's
   resume-versus-recovery question as startup recovery by re-derivation.
   Should `464c` be rescoped to the typed artifact descriptor (plus, at most,
   a disposable diagnostics snapshot) and `9751` reframed accordingly, or does
   the maintainer intend a future mode (multi-session, batch, expensive
   long-refinement jobs) where paying twice for lost in-flight work is
   unacceptable and durable in-flight state must return?
5. **Retry without speech.** Option A retries a failed lane only on the next
   final revision or at stop. If a user goes silent for twenty minutes with a
   failed notes lane, is it acceptable that nothing retries until stop, or
   does the product want timer-driven backoff mid-session (a point for
   Option B, or a small timer added to Option A)?
6. **Graph freshness during a live session.** Graph is
   recorded-but-not-required for `Finalized` (MUST #22). Under either option,
   graph work can lag notes work during continuous speech and may drain only
   at stop. Is arbitrary graph-lane lag during a live meeting acceptable, or
   is there a freshness expectation that would justify Option B's explicit
   catch-up lane with a starvation escape hatch?
