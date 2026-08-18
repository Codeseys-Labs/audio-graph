# §0 answers — the three cross-cutting questions

**Provenance.** The maintainer delegated all three of these to the agent on
2026-08-17 ("do what you think is best", three times, after the agent twice
flagged them as maintainer decisions). They are recorded here as **agent-made
decisions under delegation**, not maintainer-originated ones. The maintainer
authorized the decisions but has not independently reviewed the reasoning.

Each answer below matches the `DEFAULT:` the decision packet recommended. That
is not deference — the reasoning is restated and, where possible, grounded in
code or a measurement taken afterwards. Where an answer rests on judgement
rather than evidence, it says so.

These answers **constrain** `audio-graph-70c8`, `audio-graph-a668`, and
`audio-graph-21e9`. They do not resolve those tickets, which remain
`wayfinder:grilling`.

---

## Q0.1 — Authorize splitting ADR-0028:77-78? **YES.**

Recorded as its own record:
[ADR-0035](../../adr/0035-record-post-stop-finalization-failure-as-per-session-finalization-blocked.md),
which narrows ADR-0028's finalization arm rather than superseding it wholesale.
It needed an ADR because it changes what an accepted ADR says; the other two
questions do not.

Short reason: the undismissable guarantee ADR-0028 wrote was aimed at *local
durability* failure, and applying it to a remote provider's transient refusal
makes automatic cross-provider fallback the only survivable configuration — the
exact unauthorized egress `audio-graph-21e9` exists to delete.

---

## Q0.2 — Does an unmet evidence obligation, or an unconfirmed High-Impact Inference, hold the `Finalized` boundary? **NO.**

`Finalized` means *the machine did everything it could and recorded honestly what
it could not*. Unmet obligations and unconfirmed proposals are recorded as typed
gaps **inside** a Finalized Session, not as barriers to reaching it.

### Reasoning

`CONTEXT.md:82` says a High-Impact Inference "remains a proposal until the user
explicitly confirms it." That governs the **inference's** status, not the
**Session's**. Reading it as a Session-level barrier is a category error: it
would make `Finalized` user-input-gated, which contradicts the same document's
requirement that the Finalization Phase avoid a "fixed wait," and it would make
back-to-back capture self-defeating — a user recording all day would accumulate
Sessions that can never finalize without sitting down to confirm each one.

It is also consistent with two accepted ADRs rather than in tension with them.
ADR-0032 layers validation evidence *by claim*, and ADR-0034 requires exhaustive
evidence for negative claims. Both are shaped around **recording** the state of
evidence per item, not around blocking an aggregate boundary until every item is
satisfied.

### Why this is the less irreversible direction

Adding a barrier later is cheap: the typed gaps this answer requires are exactly
the data a future barrier would test. Shipping `Finalized` as user-gated and
discovering users have accumulated unfinalizable Sessions is expensive, and the
migration is worse than the original decision.

### What this obliges the tickets to do

- `audio-graph-70c8`: this answer is what licenses its recommended option's
  central property — Finalizing needs no wall-clock deadline and no human in the
  loop. That property is **derived from this answer**, so if this answer is
  reversed, `70c8`'s recommendation must be **re-derived, not patched**.
- `audio-graph-a668`: its Q3 is this same question. The typed-gap classes
  (Knowledge Gaps, Unavailable Evidence, unconfirmed proposals) must be
  expressive enough that a Finalized Session honestly reports what it lacks —
  otherwise this answer degrades into silently dropping obligations, which the
  repository's evidence rules forbid.
- Neither ticket may treat an unmet obligation as a `Finalization Blocked`
  reason. `Finalization Blocked` is for failures with a retry path (ADR-0035),
  not for evidence the user has simply not confirmed.

### How to reverse

Cheap until the typed-gap classes ship and Sessions carry Finalized records.
Reverse by adding an admission barrier at the final-refinement gate and
reopening `70c8`.

---

## Q0.3 — Is Original Session Audio retained on disk in this slice? **NO.**

Retained-audio-range Evidence Annotations are therefore an **optional
enrichment** in this slice, not a satisfiable requirement.

### Reasoning

The evidence is unambiguous: `SessionArtifactKind` has twelve members and no
audio kind (`src-tauri/src/persistence/mod.rs:370-383`), and no production code
writes session audio — the only writers are test fixtures
(`aec_vad_fixtures.rs`, `source_separation_fixtures.rs`). Adding retention is
therefore net-new work, not the documentation of existing behaviour.

The cost of saying yes is concentrated in one place. ADR-0027:96-101 makes the
typed manifest drive "load, export, backup, delete, purge, recovery, retention,
and usage," so a new member propagates into all eight. It also adds a
residual-failure class to `70c8`'s deletion path and a producer row to
ADR-0034's egress inventory — both in tickets that are already the wave's
critical path.

This is the one §0 answer that is genuinely a scope call rather than a
correctness argument. Audio retention is desirable; it is not desirable *now*,
inside the slice whose purpose is a trustworthy first path.

### How to reverse

Cheapest of the three. Add the manifest member, its eight lifecycle behaviours,
and a producer row to ADR-0034's inventory. Nothing decided here forecloses it,
and `a668`'s annotation schema should leave the audio-range shape *expressible*
even while nothing produces it.

---

## Sequencing constraint that survives all three answers

From `reconciliation.md`, and independent of the answers above: **implementation
order must not follow decision order.** `a668`'s stricter validator must ship
*after* `21e9`'s fallback removal. Today every validator rejection escalates the
repair prompt to the **next provider in the fallback chain**
(`src-tauri/src/llm/executor.rs:774-780`), authorized only by a privacy boolean
(`src-tauri/src/commands.rs:2026-2028`) with no ADR-0033 gate anywhere in
`executor.rs`. Tightening evidence first would turn this wave's hardening into
an unauthorized-egress amplifier.
