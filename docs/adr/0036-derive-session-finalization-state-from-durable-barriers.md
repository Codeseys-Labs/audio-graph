---
status: accepted
date: 2026-08-18
deciders: [maintainer]
consulted: [wayfinder 8873 frontier decision packet, docs/agentic-runs/2026-08-17-wayfinder-8873-frontier/]
relates: [ADR-0035, ADR-0028, ADR-0027, ADR-0029]
---

# ADR-0036: Derive Session Finalization State from Durable Barriers

> **Provenance.** The maintainer decided this on 2026-08-18 during the wayfinder
> grilling of ticket `audio-graph-70c8`, choosing among agent-prepared options in
> §1 of
> [`decision-packet.md`](../agentic-runs/2026-08-17-wayfinder-8873-frontier/decision-packet.md).
> The reasoning below restates that packet's case rather than adding new
> analysis; the maintainer reviewed the distilled trade-offs and caveats, not the
> full packet. See **More Information** for how to reverse it.

## Context and Problem Statement

Ticket `audio-graph-70c8` ("Complete the Session finalization state machine") has
to say what is *authoritative* for where a Session is between capture stop and
`Finalized Session Memory`: a persisted stage enum that the app writes as it
advances, or a set of predicates re-derived on demand from durable evidence. The
same decision has to say where capture returns to `Idle`, because `CONTEXT.md`
requires a second Session to be startable while a previous one is still
finalizing, and it has to say what bounds the wait, because `CONTEXT.md`
explicitly forbids the Finalization Phase from imposing a "fixed wait."

Several constraints are already bound and the answer has to fit inside them:

| Constraint | Source |
|---|---|
| Split-brain has no deterministic winner; dual authority rejected for storage | `docs/adr/0027-...md:145` |
| Derived state is disposable and head-vector-stamped; mismatch → rebuild, never authority arbitration | `docs/adr/0029-...md:59-62` |
| Coverage and failure state are **deliberately not persisted**; a restored in-flight job is demoted to pending | `src-tauri/src/projection_scheduler.rs:606-625, 845-857` |
| Every accepted restart rule in the 5e41 prototype is "re-derive from durable evidence," never "resume at stage N" | 5e41 §Admission state and crash reconciliation |
| `Finalization Blocked` must retain "an exact reason and a retry path"; the Finalization Phase must avoid a "fixed wait" | `CONTEXT.md` |
| The release-blocking validation tier is a deterministic offline fixture, and "a command may claim only what it asserts" | `docs/adr/0032-...md:63` |
| A failed lane on an unchanged basis returns `Idle` forever — no watchdog, no owner | `src-tauri/src/projection_scheduler.rs:355-357` |
| Session status today is a bare `"active" \| "complete" \| "crashed"` string | `src-tauri/src/sessions/mod.rs:62` |

Two of the packet's three cross-cutting §0 answers feed directly into this
decision. [ADR-0035](0035-record-post-stop-finalization-failure-as-per-session-finalization-blocked.md)
(Q0.1) established that a per-Session `Finalization Blocked` record exists at all
— without it a route-exhausted finalization would enter app-modal
`RecoveryRequired` and there would be nothing for this state machine to fail
into. Q0.2, recorded in
[`section-0-answers.md`](../agentic-runs/2026-08-17-wayfinder-8873-frontier/section-0-answers.md),
established that an unmet evidence obligation or an unconfirmed High-Impact
Inference does **not** hold the `Finalized` boundary. That answer is what
licenses a `Finalizing` phase with no wall-clock deadline and no human in the
loop; it is a premise of this decision, not a detail of it.

## Decision Drivers

- The repository has twice ruled against a second durable authority beside the
  canonical head vectors (ADR-0027, ADR-0029), and a persisted stage field is
  exactly that.
- Restart recovery must not be a distinct code path, because a single code path
  is what makes the ADR-0032 tier-3 deterministic offline proof affordable.
- Back-to-back capture must survive a slow or failing finalization of the
  previous Session: a remote refinement must never hold the microphone.
- `CONTEXT.md` forbids a fixed wait in the Finalization Phase, and requires
  `Finalization Blocked` to carry an exact reason and a retry path.
- A `Blocked` reason must not rot into a lie between the moment it is written and
  the moment it is shown or retried.
- Deletion of a Session must be unambiguous and immediate, with late results
  discarded (5e41's deletion fencing).
- Nothing here may make automatic cross-provider fallback load-bearing
  (ADR-0033's posture; the ungated fallback at `src-tauri/src/llm/executor.rs:664-699`
  that `audio-graph-21e9` exists to delete).

## Considered Options

- **A. Persisted stage machine** — one durable field per Session records which
  finalization stage it is in, with explicit transitions written as work
  advances.
- **B. Derived barrier reconciler** — finalization state is a set of predicates
  re-derived from durable canonical watermarks, a durable remote-attempt ledger,
  and per-Session `Finalization Blocked` records; nothing persists "progress"
  itself.
- **C. Stop-blocking serialized finalization** — stop does not return until
  finalization completes, so there is no concurrent finalization state to model.

## Decision Outcome

Chosen option: **"B. Derived barrier reconciler"**, because three independent
signals agree. The codebase already refuses to persist finalization progress
while accepted patches carry their own basis, so lane coverage is *already*
derived. The project has already ruled against dual authority twice. And none of
5e41's accepted restart rules is expressible as "resume at stage N" — they are
all re-derivations, which a stage enum would have to reconcile against anyway,
buying legibility rather than correctness.

The decided shape:

- **Exactly one wall-clock deadline exists in the whole machine**: a bounded
  **Local Durable Stop** barrier — final flush chunk or explicit discontinuity,
  provider close classified, pre-stop events `Accepted`, stop cut recorded with
  its head vector — expressed as a named p99-tuned constant in the style of
  `src-tauri/src/state.rs:1112-1128`. **Capture returns to `Idle` at that
  barrier**, so a second Session can start while the previous one finalizes.
- **`Finalizing` carries no deadline.** It is bounded by per-attempt budgets,
  per-lane budgets with backoff, and a **no-progress rule** that lands in
  `Finalization Blocked`. The timeout seam is: the route contract
  (`audio-graph-21e9`) owns the per-attempt deadline; this decision owns totals
  and the no-progress rule.
- **`Blocked` is re-derived before it is shown or retried**, so a reason that has
  since become satisfiable clears with zero cost and zero egress.
- **Restart recovery is normal progress on the same code path** — there is no
  resume-at-stage branch to write, test, or prove separately.
- **Cancellation lands as `Blocked{UserCancelled}`, never `Finalized`.**
- **Deletion always wins**, with late results discarded per 5e41's fencing.
- **Back-to-back capture is preserved by scoping the quiesce gate, not by
  weakening it.** Capture-scoped producers still quiesce, so
  `src-tauri/src/commands.rs:684-712` keeps its present meaning; the detached
  finalization owner registers in a separate per-epoch registry that
  `ensure_session_idle_for_rotation` never consults.

The accepted sub-question defaults, which belong to this one decision rather than
to records of their own:

1. **Required Projection Lanes for `Finalized`: notes required, graph
   recorded-but-not-required.** A Session reaches `Finalized` when the notes lane
   is covered; graph coverage is recorded but does not gate the boundary.
2. **Auto-retry of a `Blocked` refinement is permitted without asking for exactly
   two classes** — provably never-dispatched and provably Absent — and both facts
   are read from AudioGraph's **own** durable scheduler/attempt record (5e41's
   `DurableQueued` and `AbsentRetryAuthorized`), never inferred by the route
   layer, which cannot know whether the socket closed before or after the
   provider began work. The externally-uncertain class still requires explicit
   cost-and-egress authorization, as 5e41 already settled.
3. **User-initiated "cancel finalization" is offered**, lands as
   `Blocked{UserCancelled}`, stays retryable indefinitely, and Review does not
   nag about it.

### Consequences

- **Positive**: one authority. Finalization state cannot disagree with the
  canonical head vectors, because it is computed from them; there is no
  split-brain to arbitrate (ADR-0027:145).
- **Positive**: restart recovery is normal progress, so the ADR-0032 tier-3
  release-blocking offline fixture proves one code path instead of two. This is
  the single largest reason the option was chosen.
- **Positive**: `Blocked` cannot rot into a lie. Because the reason is
  re-derived before display or retry, a Blocked Session whose blocker has
  cleared resolves without a remote call and without egress.
- **Positive**: back-to-back capture works. The microphone is released at the
  Local Durable Stop barrier regardless of how slow a remote refinement is, and
  no fixed wait is introduced anywhere.
- **Positive**: adding a lane needs no manifest migration, because no persisted
  field enumerates lanes or stages.
- **Positive**: it produces the artifacts the other two frontier tickets consume
  — the state set, the `Finalization Blocked` reason taxonomy, and the per-lane
  coverage predicate — so `audio-graph-8873` planning, `3b48`, `1d92`, and `44c1`
  can proceed.
- **Negative — the spec is currently unimplementable.** The central exit barrier
  is phrased over events being `Accepted`, and the runtime does not provide
  `Accepted`: `ProjectionEventWriter::append` is a non-blocking `try_send` that
  returns "enqueued" (`src-tauri/src/persistence/mod.rs:2313-2320`), and the
  writer thread flushes its `BufWriter` only at shutdown (`:2435-2465`). This
  decision is therefore correct-but-unimplementable until the commit-boundary
  work in seeds `audio-graph-90f3` / `audio-graph-8e73` lands, and
  implementation of `audio-graph-70c8` is **blocked on them**.
- **Negative — the recommendation was MEDIUM confidence, for exactly that
  reason.** The choice is not being adopted as a high-confidence call; the
  missing durability primitive is the whole of the discount.
- **Negative — the no-deadline, no-human property is inherited, not
  self-standing.** It is derived from Q0.2's answer that `Finalized` is not
  user-input-gated (`section-0-answers.md`). If Q0.2 is ever reversed, this
  decision must be **re-derived, not patched**: `Finalized` becomes
  user-input-gated and the entire "no fixed wait" posture goes with it.
- **Negative — sub-question default 1 silently decides something in another
  ticket.** "Notes required, graph recorded-but-not-required" means a graph-lane
  absence claim has no coverage basis over the final transcript, so
  `audio-graph-a668`'s absence-claim class is inert for graph facts unless a668
  explicitly re-opens it. A lifecycle default is thereby choosing an option shape
  in an evidence ticket.
- **Negative — progress is computed, not glanced at.** Review cannot read a field
  to render a progress state; it must evaluate predicates. Absent a cached head
  vector, the predicates replay per query.
- **Negative — the cache that fixes that has an unresolved gate.** The intended
  mitigation is a rebuildable cached head vector built as an ADR-0029-class
  disposable derivative, but ADR-0029's Decision Outcome opens with seven
  preconditions for a "query-index proposal" (including a *measured* latency or
  memory budget). Whether a per-Session finalization cache falls inside that term
  is genuinely unresolved (reconciliation D4).
- **Negative — the re-derivation cost may be worse than budgeted.** If
  `audio-graph-a668` lands a per-item, evidence-resolving admission gate, then
  re-deriving before display is O(items × annotations) per load rather than
  O(streams), and a cached head vector does not cache item-level annotation
  resolution (reconciliation H6). Nobody has priced this.
- **Negative — this is "derived except where it cannot be."** The remote-attempt
  ledger is genuinely durable state, so the option does not fully escape the
  thing it criticises in option A; it minimises persisted state rather than
  eliminating it, and that ledger is what makes reversal cost a migration.
- **Negative — it specifies inside deferred and contested territory.** Retry
  progression was deferred to `audio-graph-3b48`, yet per-lane attempt budgets
  with backoff are specified here (reconciliation D6). Final refinement has at
  least four claimed owners — 5e41 deferred it to `3b48`, `audio-graph-fbca`
  claims "output budgets" and "coverage accounting", `21e9` claims per-route
  completion budgets, and this decision claims a barrier with attempt budgets —
  and the seam is undeclared (reconciliation C11). "Output budgets" and "coverage
  accounting" cannot be owned by three tickets at once.
- **Negative — a pinned route can make a Blocked Session permanently
  unsatisfiable.** The detached finalization owner dispatches refinement after
  rotation, possibly after an app update. ADR-0033:48-52 requires every
  content-bearing start to resolve its actual descriptor and reject one whose
  `ui_selectable` is false, and its carve-out (`:58-65`) covers only stop/cancel/
  drain/cleanup of an *active* session — not a new refinement start. A route
  pinned in a durable attempt record that later loses enablement yields a Blocked
  Session whose only retry path is rejected forever, and re-routing would itself
  need authorization under `21e9`'s "never silent" rule (reconciliation H4).
- **Negative — an unowned stall survives this decision.**
  `src-tauri/src/projection_scheduler.rs:355-357` returns `Idle` when
  `last_failed_basis == basis`, so a failed lane on an unchanged basis stalls
  forever today. The no-progress rule is specified here, but its owner is not.
- **Neutral**: ADR-0035's warning still applies — per-Session `Blocked` records
  accumulate quietly, so Review surfacing remains a `audio-graph-70c8`
  obligation, not something this decision discharges.

## Pros and Cons of the Options

### A. Persisted stage machine

- Good, because one field says where a Session is, which makes Review display
  trivial and makes transitions explicit and auditable.
- Good, because progress is glanceable and cheap to read — no predicate
  evaluation, no head-vector cache, no replay.
- Good, because it is the most conventional and most legible shape for anyone new
  to the code.
- Bad, because it installs a second durable authority next to the canonical head
  vectors — the exact pattern ADR-0027:145 rejected on the grounds that
  split-brain recovery has no deterministic winner.
- Bad, because load must reconcile the field against durable evidence anyway, so
  the field buys legibility rather than correctness, and the reconciliation code
  is the derived option's code plus the field.
- Bad, because it over-serializes work that is safely concurrent, forcing
  drain/flush overlap to be sequenced by the stage vocabulary.
- Bad, because every new Projection Lane is a manifest migration under ADR-0027.

### B. Derived barrier reconciler

- Good, because restart recovery *is* normal progress: one code path, which is
  what makes the ADR-0032 tier-3 offline proof affordable.
- Good, because `Blocked` cannot rot — it is re-derived before it is shown or
  retried, so a satisfiable Blocked clears with zero cost and zero egress.
- Good, because it permits safe drain/flush overlap instead of serializing it.
- Good, because no persisted stage or lane enumeration means no manifest
  migration per lane.
- Bad, because the predicates need a cached head vector or they replay per query,
  and that cache's status under ADR-0029's preconditions is unresolved.
- Bad, because progress is computed rather than glanced at, which costs Review
  clarity and costs debuggers a single field to look at.
- Bad, because the attempt ledger is still durable state — the option is "derived
  except where it cannot be," not purely derived.
- Bad, because it depends on ADR-0035 (Q0.1) existing; without a per-Session
  `Blocked` record it has nowhere to fail into.

### C. Stop-blocking serialized finalization

- Good, because it is the smallest delta — it is literally today's gate at
  `src-tauri/src/commands.rs:7148-7177`.
- Good, because no second failure taxonomy is needed: a failure that never
  escapes stop is a capture-lifecycle failure, which ADR-0028's existing
  undismissable `RecoveryRequired` already covers. The packet scored this as
  "ADR-0028 stays true exactly as written," which is now stale — ADR-0035 is
  accepted and has already narrowed ADR-0028's finalization arm, so choosing
  option C today additionally requires **reversing ADR-0035**, not merely leaving
  ADR-0028 untouched.
- Good, because there is no concurrent finalization state to model at all, so the
  whole question mostly disappears.
- Bad, because it violates the back-to-back capture requirement: a slow remote
  refinement holds the microphone hostage.
- Bad, because it pushes toward precisely the fixed wait `CONTEXT.md` forbids in
  the Finalization Phase.
- Bad, because a provider stall becomes a capture outage, which recreates the
  pressure toward automatic cross-provider fallback that ADR-0035 exists to
  relieve.

## More Information

- **Relationship to ADR-0035** (Record Post-Stop Finalization Failure as
  Per-Session Finalization Blocked): ADR-0035 is a precondition of this record.
  It created the per-Session `Finalization Blocked` state that this decision
  fails into and specifies the retry semantics of. ADR-0035 explicitly left the
  closed reason taxonomy, the retry path, and the Review surfacing to
  `audio-graph-70c8`; this record decides the state model those live in and hands
  the taxonomy itself to the ticket.
- **Relationship to ADR-0028** (Separate Capture Lifecycle from Foreground
  Workspace): unchanged by this record. ADR-0028's finalization arm was already
  narrowed by ADR-0035; this decision consumes that narrowing and adds nothing to
  it. ADR-0028's capture-lifecycle independence is what makes the return to
  `Idle` at the Local Durable Stop barrier legitimate.
- **Relationship to ADR-0027** (Adopt File-Canonical Durable Session Storage):
  this record is an application of ADR-0027's rejection of dual authority
  (`:145`) to the finalization lifecycle. The remote-attempt ledger it introduces
  is durable state under ADR-0027 and belongs in the typed manifest regime that
  ADR-0027:96-101 makes drive load, export, backup, delete, purge, recovery,
  retention, and usage.
- **Relationship to ADR-0029** (Gate Rebuildable Query Indexes on Measured
  Demand): the cached head vector this decision leans on must be a disposable,
  rebuildable derivative in ADR-0029's sense. Whether ADR-0029's seven
  preconditions for a query-index proposal also gate a per-Session finalization
  cache is unresolved and is flagged as a negative above rather than assumed away.
- **Relationship to ADR-0031, ADR-0032, ADR-0033**: ADR-0031's `Revised`
  classification supplies the basis semantics the coverage predicate reads;
  ADR-0032's tier-3 offline fixture is the beneficiary of the single-code-path
  property and its "a command may claim only what it asserts" rule bounds what
  `Finalized` may assert; ADR-0033's content-start gate is what makes the pinned-
  route hazard (H4) real.
- **Relationship to ADR-0034** (Require Exhaustive Evidence for Negative
  Data-Egress Claims): the per-lane coverage predicate decided here is **not**
  ADR-0034's named-and-versioned marker. ADR-0034's text is about a producer
  inventory for egress; the predicate here is deliberately disposable and
  unversioned. `audio-graph-a668` needs its own separately named and versioned
  transcript-coverage marker and may not borrow this one.
- **Downstream ownership**: `audio-graph-70c8` owns the concrete state set, the
  closed `Finalization Blocked` reason taxonomy (reconciling 5e41's
  `ExternalEffectUnknown` / `OutcomeUncertain` vocabulary with the route layer's
  retry classes), the per-lane coverage predicate, and the Review surfacing
  obligation inherited from ADR-0035. `audio-graph-3b48` owns backlog scheduling,
  lane reconciliation, and the retry progression whose budgets are described
  here. `audio-graph-1d92` owns prototyping `Finalizing` and
  `Finalization Blocked` in Review. `audio-graph-44c1` owns the trustworthy
  Session Memory acceptance contract that consumes the state set.
  `audio-graph-a668` and `audio-graph-21e9` consume the state set, the reason
  taxonomy, and the coverage predicate. The final-refinement seam against
  `audio-graph-fbca` remains uncut and is a maintainer decision.
- **How to reverse**: moderate cost, and rising. The remote-attempt ledger is
  durable state under ADR-0027, so moving to a persisted stage enum later is a
  manifest migration, not a code swap: existing Sessions carrying ledger entries
  and Blocked records must be migrated into whatever stage vocabulary replaces
  them. Reverse by superseding this ADR with one choosing option A or C and
  reopening `audio-graph-70c8`. Reversal is cheapest before the ledger ships.
  Separately, if Q0.2 is reversed this record must be re-derived rather than
  amended.
- **Sequencing constraints**: (1) implementation is blocked on
  `audio-graph-90f3` / `audio-graph-8e73` delivering a real `Accepted`
  acknowledgement; specifying ahead of them is fine, implementing is not.
  (2) Decision order across the frontier is `70c8` → `a668` → `21e9`, because
  this ticket produces what the other two consume. (3) Implementation order must
  **not** follow decision order for one pairing: `a668`'s stricter validator must
  ship *after* `21e9`'s fallback removal, because every validator rejection today
  escalates the repair prompt to the next provider in the chain
  (`src-tauri/src/llm/executor.rs:774-780`) authorized only by a privacy boolean
  (`src-tauri/src/commands.rs:2026-2028`), so tightening evidence first would
  turn this wave's hardening into an unauthorized-egress amplifier.
- **Not decided here**: the final-refinement ownership seam (reconciliation C11),
  whether a Blocked Session pinned to a route that lost `ui_selectable` may be
  re-routed (H4), and the owner of the unchanged-basis lane stall at
  `projection_scheduler.rs:355-357`.
