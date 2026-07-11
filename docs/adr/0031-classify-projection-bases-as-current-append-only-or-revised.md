---
status: accepted
date: 2026-07-09
deciders: [AudioGraph maintainers]
---

# ADR-0031: Classify Projection Bases as Current, Append-Only, or Revised

## Context and Problem Statement

Notes and graph projections run asynchronously from a revisioned transcript
ledger. Continuous speech often appends new spans while an LLM completion is in
flight. Treating every completion whose basis is not the newest ledger as stale
throws away useful progressive notes, but accepting a completion after a
covered span was revised can apply output derived from false text.

The previous implementation direction checked ledger growth before proving the
hash of the subset covered by the completion. A mismatched hash could therefore
be misclassified as harmless append-only staleness. The projection scheduler
needs one shared classifier with explicit follow-up behavior.

## Decision Drivers

- Produce useful automatic notes during continuous speech.
- Never accept output whose covered transcript subset changed.
- Distinguish appended speech from revision, reorder, deletion, or hash
  corruption.
- Give apply, success, failure, and scheduler paths one classification.
- Keep follow-up bounded and preserve cancellation and retry behavior.
- Do not equate in-memory apply or queue enqueue with durable acceptance.

## Considered Options

- Discard every completion whose basis is not the latest ledger
- Accept valid append-only prefixes and schedule a background follow-up
- Pin or throttle transcript processing until each projection finishes

## Decision Outcome

Chosen option: "Accept valid append-only prefixes and schedule a background
follow-up", because a completion based on an unchanged prefix remains true and
useful even when later speech has arrived.

Every completion basis records the ordered covered transcript identities and
revisions, covered count, and hash. The classifier follows this order:

1. select exactly the current-ledger subset covered by the basis;
2. hash that covered subset before examining any extra spans;
3. compare the subset hash and ordered identities/revisions with the basis;
4. classify any missing, reordered, deleted, revised, or hash-mismatched covered
   span as **Revised**;
5. if the covered subset matches and the ledger has no extra spans, classify it
   as **Current**;
6. if the covered subset matches and the ledger has only later appended spans,
   classify it as **AppendOnly**.

Automatic projection bases contain only final, end-of-turn, or otherwise stable
transcript revisions. Provisional ASR revisions remain canonical transcript
events, but an appended provisional span neither makes a final-only projection
basis stale nor starts a follow-up job. Historical bases that explicitly covered
partials retain full-ledger validation for replay compatibility.

Every completion, failure, and telemetry update is matched against the exact
projection job id, session id, scheduler session, and ledger session before it
can mutate scheduler state. A late worker from a rotated or superseded session
is ignored with an observable counter; completion by projection kind alone is
not permitted.

`Current` output follows the normal apply path. `AppendOnly` output may
follow the same semantic apply path and schedules one coalesced Background
follow-up for the appended spans. `Revised` output never mutates notes or
graph state and schedules Replay repair when the scheduler policy permits it.
The same classifier governs successful and failed completions so a stale
failure cannot consume or reschedule the wrong job.

`validate_basis` delegates to this classifier or is mechanically proven to
produce the same result. No independent second interpretation of basis currency
is allowed.

Classification authorizes semantic applicability, not durability. Under
ADR-0027, a Current or AppendOnly projection becomes visible as durable state
only after its canonical projection event is Accepted. Queue enqueue, writer
send, or materialized snapshot save is insufficient. A snapshot lag after
Accepted is derived-cache lag and replay must reproduce the same state.

### Consequences

- **Positive**: Continuous speech can yield useful progressive notes before a
  pause.
- **Positive**: Hash mismatch and transcript revision cannot hide behind later
  append-only growth.
- **Positive**: One classifier aligns apply and scheduler follow-up behavior.
- **Negative**: AppendOnly output can briefly omit the newest speech until its
  coalesced follow-up completes.
- **Negative**: Covered-prefix hashing and ordered identity comparison add work
  to every completion.
- **Negative**: Durable runtime integration remains blocked until ADR-0027's
  Accepted event boundary exists.
- **Neutral**: This decision governs basis currency only; broader prompt
  efficiency and notes retcon design remain proposed in ADR-0025.

## Pros and Cons of the Options

### Discard every completion whose basis is not the latest ledger

- Good, because no applied completion omits newer speech.
- Good, because the apply rule is simple.
- Bad, because continuous speech can starve notes indefinitely.
- Bad, because expensive valid work over an unchanged prefix is discarded.

### Accept valid append-only prefixes and schedule a background follow-up

- Good, because unchanged-prefix output remains semantically valid.
- Good, because users receive progressive notes during long speech.
- Good, because revised covered text is still rejected.
- Bad, because a temporary partial view is visible by design.
- Bad, because follow-up coalescing and replay tests are required.

### Pin or throttle transcript processing until each projection finishes

- Good, because a projection can always target the newest fixed basis.
- Good, because scheduler currency becomes simpler.
- Bad, because transcript ingestion or projection cadence becomes coupled to
  provider latency.
- Bad, because slow or failed LLM calls can stall the live product.

## More Information

This decision governs `audio-graph-caad` and supersedes only the basis-currency
portion previously discussed inside proposed ADR-0025; ADR-0024 remains the
accepted event-sourced projection foundation.

Required tests cover Current, AppendOnly, and Revised for success and failure;
same identities with the wrong hash; appended spans after a valid prefix;
reorder, deletion, and revision; coalesced Background versus Replay repair;
session rotation, wrong job identity, ledger-session mismatch, appended
provisional spans, and unchanged metrics for ignored workers; durable event
acknowledgement before visibility; and reload/replay equality after an accepted
AppendOnly patch.

Research:
`docs/research/mvp-projection-correctness-2026-07-09.md`.
