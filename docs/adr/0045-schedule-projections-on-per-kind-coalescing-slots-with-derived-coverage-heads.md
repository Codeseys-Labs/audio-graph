---
status: accepted
date: 2026-08-20
deciders:
  - "AudioGraph maintainer (accepted 2026-08-20 via grilling, six sub-decisions)"
drafter: "Claude agent (non-decider)"
refines: ADR-0024, ADR-0027, ADR-0031
relates-to: ADR-0029, ADR-0035, ADR-0036, ADR-0042
---

# ADR-0045: Schedule Projections on Per-Kind Coalescing Slots with Derived Coverage Heads

## Context and Problem Statement

Projections (the LLM jobs that turn accepted transcript revisions into notes
and graph patches, ADR-0024) arrive faster than they complete during
continuous speech. The scheduler needs a backlog discipline: what happens to
projection work that cannot run yet, what survives a crash, and how the
system knows on reopen which transcript state each projection kind has
covered. The live defect `audio-graph-caad` (notes patches discarded as
stale during continuous speech) and the stalled-lane gap at the projection
scheduler's retry seam (owner unassigned in ADR-0036) are both symptoms of
this discipline being undefined.

The fork: a durable on-disk queue that records every intended projection
(with catch-up lanes and crash-consistent enqueue writes), or no durable
scheduler state at all, with everything derived from the canonical accepted
patch log (ADR-0027) on open.

## Decision Drivers

- No accepted patch may be silently discarded; the caad defect must be fixed
  structurally, not patched around.
- ADR-0027 already durably records every applied projection; a second
  durable store describing intentions creates a second source of truth that
  can disagree with the first after a crash.
- ADR-0029 gates rebuildable indexes and caches on measured demand, not
  anticipated demand.
- The live notes lane is the product's companion surface during capture;
  graph freshness is not (ADR-0030's surface split).
- The scheduler must hand un-recoverable failures to ADR-0035/ADR-0036's
  Finalization Blocked rather than inventing a parallel failure state.

## Considered Options

- **Derived heads + coalescing slots** — one capacity-one slot per
  `ProjectionKind`; newer work replaces (coalesces into) the pending slot;
  per-kind coverage heads re-derived on open from the last accepted
  `projection_patches` record; materialized state rebuilt from the accepted
  patch log.
- **Durable queue + catch-up lane** — every intended projection is a durable
  enqueue before it may coalesce; an explicit background lane drains backlog
  with a starvation escape hatch.
- **Status quo** — implicit scheduling in the dispatch path, the caad
  discard, and no defined crash story.

## Decision Outcome

Chosen option: "Derived heads + coalescing slots", because every element it
adds is already required by a binding constraint or a filed defect, and it
adds no durable state that can contradict the canonical log. The shared
hardening floor ships with it: the caad apply-gate fix (match basis-currency
classification three ways so `AppendOnlyStale` applies with one coalesced
follow-up), a JoinHandle registry with stop-time flush, and a named
3-attempt counter that hands a stalled lane to Finalization Blocked.

The maintainer resolved the six open sub-decisions as follows (grilling,
2026-08-20; decision memo at
`docs/agentic-runs/2026-08-20-audio-graph-3b48/decision-memo.md`):

1. **Provenance depth.** The obligation is "no accepted patch is silently
   discarded". Coalescing may overwrite intermediate bases without a durable
   trace; the ledger proves every applied patch and its basis, never the
   set of bases considered.
2. **Duplication over loss.** The caad fix ships accepting transient visible
   duplication in live notes; the reconciling follow-up projection is the
   lane's mandatory next item, so a duplicate cannot outlive the next tick.
3. **Retry shape.** One deferred retry (~60 s) after a lane failure, then
   purely event-driven (next final revision or stop). Persistent failures
   surface as Finalization Blocked at stop; no backoff loops.
4. **Graph freshness.** Graph-lane lag during live capture is unbounded by
   design; the lane must provably drain at stop, and Review renders
   oldest-pending-since so the lag is visible (per the 1d92 prototype
   findings).
5. **Reopen latency.** Replay-on-open is accepted under an explicit budget:
   p95 under ~2 s for a 2-hour session, proven by a replay benchmark on a
   long-session fixture that ships with the implementation. ADR-0029 reopens
   for a coverage cache only on a measured breach of that budget.
6. **Durable in-flight state.** Seeds `audio-graph-464c` and
   `audio-graph-9751` are rescoped: the typed artifact descriptor (plus at
   most a disposable diagnostics snapshot) survives; crash resume is
   recovery-by-re-derivation, and the cost of a crash is one repeated LLM
   call. Durable in-flight state returns only when a real batch or
   long-refinement mode exists.

### Consequences

- **Positive**: one source of truth for what happened (the accepted patch
  log); the caad silent loss becomes a visible, self-correcting state; the
  crash story is "re-derive and re-run", idempotent by construction; lanes,
  priorities, and a real queue can be layered onto the same derived heads
  later without unwinding anything.
- **Negative**: superseded intermediate bases leave no durable trace, so an
  audit can prove what was applied but not what was considered; reopening a
  session costs O(session length) replay inside a budget rather than O(1);
  the graph view can lag arbitrarily during continuous speech; a crash
  mid-projection repays one LLM call.
- **Neutral**: `464c`/`9751` narrow; the coverage-cache question stays closed
  under ADR-0029 until the benchmark measures a breach.

## Pros and Cons of the Options

### Derived heads + coalescing slots

- Good, because scheduler state cannot disagree with the canonical log after
  a crash — it *is* the canonical log, re-read.
- Good, because coalescing is the natural backpressure for continuous
  speech; the newest basis subsumes the older ones.
- Bad, because consideration (as opposed to application) is unrecorded, and
  reopen pays replay time proportional to session length.

### Durable queue + catch-up lane

- Good, because every intention survives a crash and graph work drains
  continuously.
- Bad, because every enqueue becomes a durability decision on the hot path,
  the queue's read path must be reconciled against the patch log on every
  recovery, and unwinding it later means deleting shipped persistence.
- Bad, because it touches the contested fbca C11 seam and the stop-time
  catch-up gap before either has an owner.

### Status quo

- Good, because it is zero work.
- Bad, because caad keeps silently discarding user content and the crash
  story stays undefined.

## More Information

Decision memo: `docs/agentic-runs/2026-08-20-audio-graph-3b48/decision-memo.md`
(code-reality and constraint artifacts in the same directory). Seed:
`audio-graph-3b48`; consumers: `audio-graph-fbca`, `audio-graph-44c1`.
Retry-counter handoff: ADR-0035/ADR-0036. Surface split: ADR-0030.
