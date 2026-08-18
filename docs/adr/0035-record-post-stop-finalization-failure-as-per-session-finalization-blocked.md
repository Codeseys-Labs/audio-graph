---
status: accepted
date: 2026-08-17
deciders: [agent, under explicit maintainer delegation 2026-08-17]
consulted: [wayfinder 8873 frontier decision briefs, docs/agentic-runs/2026-08-17-wayfinder-8873-frontier/]
narrows: ADR-0028
---

# ADR-0035: Record Post-Stop Finalization Failure as Per-Session Finalization Blocked

> **Provenance.** This decision was delegated to an agent by the maintainer on
> 2026-08-17 in response to question Q0.1 of
> [`decision-packet.md`](../agentic-runs/2026-08-17-wayfinder-8873-frontier/decision-packet.md).
> The maintainer authorized the decision but has not independently reviewed the
> reasoning below. See **More Information** for how to reverse it.

## Context and Problem Statement

[ADR-0028](0028-separate-capture-lifecycle-from-foreground-workspace.md) states
that `RecoveryRequired` "is reachable after canonical writer, drain, or
finalization failure," that it "cannot be cosmetically dismissed into a Saved or
healthy state," and that "an incomplete canonical drain **or finalization**
enters RecoveryRequired rather than reporting Review or Saved."

That single clause conflates two failures with very different owners. An
incomplete **local canonical drain** is capture-scoped: AudioGraph owns the
bytes, the writers, and the filesystem, so an unfinished drain genuinely means
the app's own durable state is in doubt. A **post-stop finalization** failure is
mostly external: bulk refinement calls a remote LLM route, so the dominant
failure is a provider rate limit, timeout, or outage on work that happens after
the microphone is already released and all captured bytes are durable.

Treating the second like the first makes one organization-scoped provider 429
escalate into an app-modal state that by ADR-0028's own words cannot be
dismissed. Under that constraint the only survivable configuration is a
non-empty automatic cross-provider fallback list — precisely the unauthorized
egress path that `executor.rs:664-699` implements today and that wayfinder
ticket `audio-graph-21e9` exists to remove. So keeping the clause as written is
in practice a decision to keep automatic cross-provider fallback, which no ADR
authorizes.

Ticket `audio-graph-70c8` cannot specify the finalization state machine until
this boundary is settled, and two of the three frontier briefs assumed
`Finalization Blocked` already existed as available infrastructure without
noticing it was contingent on this question.

## Decision Drivers

- A remote provider's transient refusal must not put the whole application into
  an undismissable recovery state.
- No decision may make automatic cross-provider fallback load-bearing, because
  no ADR authorizes that egress (ADR-0033 gates provider enablement at content
  start; `executor.rs` has no such gate).
- Back-to-back Session capture must survive a slow or failing finalization of a
  previous Session.
- ADR-0028's strength — that a real durability failure cannot be cosmetically
  dismissed — must be preserved for the case it was written about.
- `CONTEXT.md` requires `Finalization Blocked` to retain "an exact reason and a
  retry path" and the Finalization Phase to avoid a "fixed wait."

## Considered Options

- **A. Split by owner** — local drain failure stays capture-scoped
  `RecoveryRequired`; post-stop finalization failure becomes a per-Session
  `Finalization Blocked` record.
- **B. Keep ADR-0028 as written** — any finalization failure enters
  app-modal `RecoveryRequired`.
- **C. Split the state name only** — introduce `Finalization Blocked` but keep
  it app-modal and undismissable, identical in force to `RecoveryRequired`.

## Decision Outcome

Chosen option: **"A. Split by owner"**, because the failure that ADR-0028's
undismissable guarantee was written to protect is local durability, and applying
that same guarantee to a remote provider's transient refusal forces unauthorized
cross-provider egress to become the only working configuration.

### Consequences

- **Positive**: a provider outage degrades one Session's refinement instead of
  the application. Capture stays available.
- **Positive**: removes the pressure that makes automatic cross-provider
  fallback load-bearing, unblocking `audio-graph-21e9` to delete it.
- **Positive**: preserves ADR-0028's undismissable guarantee exactly where it
  belongs — local canonical drain and writer failure.
- **Positive**: unblocks `audio-graph-70c8` to specify the finalization state
  machine, and removes an unstated assumption from two frontier briefs.
- **Negative**: there are now **two** failure taxonomies to specify, surface,
  test, and keep consistent instead of one. `Finalization Blocked` must define
  its own closed reason set and retry path, and must re-establish its own
  non-dismissable property rather than inheriting ADR-0028's.
- **Negative**: per-Session Blocked records can accumulate silently across many
  Sessions. Unless Review surfaces them, this converts a loud app-modal failure
  into quiet per-Session debt — a real regression in visibility that the
  `audio-graph-70c8` design must answer.
- **Negative**: narrows an accepted ADR, so any reader of ADR-0028 line 78 must
  now also read this record to know the current rule.
- **Neutral**: `RecoveryRequired` keeps its full strength and its existing
  reachability from canonical writer and drain failure; only the finalization
  arm moves.

## Pros and Cons of the Options

### A. Split by owner

- Good, because the two failures have different owners, different blast radii,
  and different recovery actions.
- Good, because it keeps a transient remote refusal from being undismissable.
- Good, because it removes the structural pressure toward unauthorized
  cross-provider fallback.
- Bad, because it doubles the failure taxonomy and its test surface.
- Bad, because per-Session Blocked state is less visible than an app-modal
  state, so it can rot unnoticed if Review does not surface it.

### B. Keep ADR-0028 as written

- Good, because it is one rule, already accepted, with nothing new to specify.
- Good, because failures are maximally visible — the user cannot miss them.
- Bad, because one organization-scoped provider 429 takes the whole app modal.
- Bad, because it makes automatic cross-provider fallback the only survivable
  configuration, which contradicts ADR-0033's posture and blocks
  `audio-graph-21e9`.
- Bad, because it cannot satisfy the back-to-back capture requirement: a slow
  remote refinement holds the application hostage.

### C. Split the state name only

- Good, because it separates the two states for diagnosis and reporting.
- Good, because it retains maximum visibility.
- Bad, because it changes nothing that matters — an undismissable per-Session
  state has the same practical effect as the app-modal one, so the fallback
  pressure and the back-to-back capture violation both remain.
- Bad, because it adds a state and its taxonomy while buying no behavioural
  improvement, which is the worst trade of the three.

## More Information

- **Relationship to ADR-0028**: this record **narrows** ADR-0028's finalization
  clause (line 78) and the finalization arm of its `RecoveryRequired`
  reachability statement. Everything else in ADR-0028 — capture lifecycle,
  foreground workspace independence, passive readiness, egress scoping —
  remains in force and `accepted`. Following this repository's existing
  partial-supersession convention (see ADR-0003 / ADR-0006), ADR-0028 keeps its
  `accepted` status and carries a pointer here rather than being marked
  wholesale `superseded`.
- **Downstream**: `audio-graph-70c8` owns the closed `Finalization Blocked`
  reason taxonomy, its retry path, and the Review surfacing that answers the
  second negative consequence above. `audio-graph-21e9` owns removing the
  automatic cross-provider fallback this decision de-pressurizes.
- **How to reverse**: this decision is reversible while `Finalization Blocked`
  remains unimplemented — no production code reads it today. Supersede this ADR
  with one choosing option B or C, and reopen `audio-graph-70c8`. The cost rises
  sharply once the reason taxonomy ships and Sessions carry persisted Blocked
  records, because those records would then need migration.
- **Not decided here**: whether an unmet evidence obligation or unconfirmed
  High-Impact Inference holds the `Finalized` boundary (Q0.2), and whether
  Original Session Audio is retained (Q0.3). Both are recorded in
  `docs/agentic-runs/2026-08-17-wayfinder-8873-frontier/`.
