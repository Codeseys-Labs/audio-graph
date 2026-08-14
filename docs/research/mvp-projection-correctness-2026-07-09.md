# MVP notes and graph projection correctness review

Date: 2026-07-09

Status: classifier/scheduler hardening implemented; durable progressive fold remains blocked

Owning Seeds: `audio-graph-caad`, `audio-graph-10ac`,
`audio-graph-ab10`, and `audio-graph-99eb`

Handoff basis:
`docs/backlog/handoff-2026-07-08-caad-10ac-wave.md`

Source-anchor note: unqualified line references in the discovery findings refer
to HEAD `f97e19c`. Symbols and the dated implementation checkpoint are
authoritative for the current working slice.

Decision:
[ADR-0031](../adr/0031-classify-projection-bases-as-current-append-only-or-revised.md)
accepts the focused `caad` policy reviewed here.

## Executive verdict

- The reviewed `caad` implementation at `200a87a` was not merge-ready. This
  wave replaced it with one covered-subset-first classifier, chronological
  append detection, symmetric success/failure policy, exact job/session
  completion correlation, and projection-eligible partial filtering.
- The `10ac` graph failure has a small, testable repair: a graph-specific
  output budget floor, correct nullable-envelope handling, bounded retry
  classification, and sanitized routing telemetry.
- The reviewed branch also lacked immutable job/session correlation; this wave
  now rejects late workers before they can consume a rotated session's active
  job or mutate its metrics.

### 2026-07-09 implementation checkpoint

Implemented and focused-tested:

- one `Current | AppendOnlyStale | Revised` classifier, with `validate_basis`
  delegating to it;
- covered-subset hash validation before considering extra spans;
- chronological rather than lexical span-id tail detection;
- Background follow-up for AppendOnly success/failure and Replay repair for
  Revised success/failure;
- exact job id, job session, scheduler session, and ledger session matching
  before completion/failure or telemetry mutation;
- an observable ignored-superseded-completion counter; and
- final/end-of-turn projection bases, with unrelated appended partials excluded
  from LLM follow-up scheduling while historical partial-bearing bases remain
  replay-compatible.

Still blocked and intentionally not claimed complete:

- live AppendOnly output is still rejected by the legacy materializer apply
  gate, so the useful prefix is not visible before the Background follow-up;
- projection persistence exposes enqueue rather than an Accepted acknowledgement;
- UI materialized events can emit before the canonical writer proves acceptance;
  and
- reload/replay equality for an Accepted AppendOnly patch cannot be proven until
  `audio-graph-90f3` supplies the crash-consistent commit boundary required by
  ADR-0027.

## `caad`: continuous speech notes

### Intended policy

If a projection basis is stale only because new transcript spans were appended,
the patch still describes a valid prefix of the session. Apply it progressively
and schedule a background follow-up.

If any covered span was revised, removed, or otherwise changed, reject the patch
and schedule immediate replay repair.

This lets useful notes appear during continuous speech without allowing a patch
based on invalidated text to mutate the durable materialization.

### What the reviewed branch got right

- completion and failure gates are symmetric in
  `projection_scheduler.rs:323-460`
- revised bases trigger immediate Replay repair
- append-only bases trigger Background follow-up
- single in-flight scheduling and materializer sequence guards prevent
  same-session double apply
- focused tests cover append-only and revised completion/failure

### Reviewed `200a87a` findings and current status

#### Durable progressive apply remains blocked

The reviewed branch attempted to make an append-only patch immediately visible
and changed a state test from rejection toward enqueue/materialization. The
current pure slice intentionally does not do that: `state.rs` and the legacy
materializer still reject the stale patch because queue admission is not an
ADR-0027 Accepted durable commit.

The remaining runtime test must prove canonical event acceptance first, then
materialized visibility and reload/replay equality. That work stays blocked on
`audio-graph-90f3`; the 125 passing projection-filtered tests do not claim the
durable fold.

#### Wrong-hash classification was resolved in the pure slice

The contract classifies a basis hash mismatch with otherwise matching span
revisions as Revised with `TranscriptHashMismatch`
(`projections.rs:827-833`).

The reviewed branch treated any hash difference as `AppendOnlyStale`, including
an identical span set carrying a forged or corrupt hash. The integrated
classifier now hashes the exact covered subset before considering appended
spans and returns `Revised(TranscriptHashMismatch)` on mismatch.

Correct order:

1. select the current-ledger subset covered by the basis
2. hash that subset
3. compare it with `basis.transcript_hash`
4. classify a mismatch as Revised
5. only then check whether the current ledger has additional appended spans

#### One classifier now owns both paths

The reviewed branch described `validate_basis` as a thin wrapper while retaining
an independent implementation. The integrated slice makes `validate_basis`
delegate to the ADR-0031 classifier. Proposed ADR-0025 no longer owns this
focused basis-currency decision.

### Regression-test status

Passing now:

- same span ids and revisions plus wrong basis hash produces
  `Revised(TranscriptHashMismatch)`;
- deletion, revision, reorder, non-tail insertion, chronological append, and
  appended-partial cases classify and schedule correctly; and
- stale success/failure completion cannot consume a newer job or mutate its
  metrics.

Still required after `audio-graph-90f3`:

- a runtime append-only patch becomes visible only after its canonical event is
  Accepted;
- runtime revised basis rejects without persistence; and
- accepted append-only patch reload and replay produce identical materialized
  state.

## `10ac`: graph projection decode failures

### Confirmed facts

- Production OpenRouter configuration defaults to 512 output tokens
  (`commands.rs:901-905`).
- Every request currently uses that shared value unchanged
  (`llm/openrouter.rs:485-512`).
- Graph projections have no request-specific override.
- `ChoiceMessage.content` is a required Rust `String`
  (`openrouter.rs:761-764`), while the documented response contract permits
  `string | null`.
- `response.json()` plus `is_decode()`
  (`openrouter.rs:1721-1737,1890-1901`) conflates malformed or truncated
  response bodies with syntactically valid envelopes whose content is null.
- Schema-cached projection calls discard routing telemetry
  (`openrouter.rs:1607-1617`).
- Projection success records the configured provider and requested model, not
  the selected provider and served model
  (`executor.rs:846-850,989-1002`).
- The graph strict schema already requires `type` for all ten variants
  (`projection_llm.rs:305-322,353-401`).

OpenRouter documents both nullable content and reasoning tokens as output
budget:

- [response schema](https://openrouter.ai/docs/api/reference/overview)
- [reasoning token guidance](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens)
- [error envelope contract](https://openrouter.ai/docs/api/reference/errors-and-debugging)

The 512-token default is unquestionably too small for many reasoning-model
graph responses. Existing logs cannot prove that every observed decode failure
was a null-content envelope or that every missing discriminator came from
budget exhaustion. The response body was consumed before safe shape telemetry
could preserve that distinction. Instrument the distinction and prove it in
Round 4.

### Smallest safe fix

1. Add a per-request output-token override in request construction; do not
   mutate shared client configuration.
2. In the OpenRouter projection path, use
   `max(configured_max_tokens, 2048)` for Graph in both JSON Schema and JSON
   fallback modes.
3. Keep Notes, extraction, blocking chat, and streaming chat at the configured
   value.
4. Read a successful response body as text and record only sanitized metadata:
   body byte count, request id, JSON shape category, selected provider, served
   model, finish reason, and token counters.
5. Parse completion content as `Option<String>`.
6. Classify null or empty content as terminal
   `missing_completion_content`, not a serde retry.
7. Retain bounded retry for connect, timeout, body-read, retryable HTTP, and
   malformed or incomplete JSON failures.
8. Treat a syntactically valid but incompatible envelope as terminal.
9. Handle typed embedded provider errors without logging raw messages,
   metadata, content, or reasoning.
10. Preserve public wrappers and add telemetry-returning private projection
    variants.
11. Carry sanitized route, finish reason, and optional reasoning-token count
    into projection diagnostics without changing durable provenance semantics.

### Regression matrix

| Area | Required assertions |
| --- | --- |
| Budget | Graph 512 becomes 2048; Graph 4096 stays 4096; schema and fallback match; other consumers unchanged |
| Envelope | String succeeds; null/absent content yields a clean terminal error; empty choices are terminal; reasoning is tolerated |
| Retry | 408/409/429/5xx, connect, timeout, body read, and malformed JSON retain bounded retry; valid shape mismatch does not |
| Errors | Typed error and `finish_reason=error` classify without leaking body or message |
| Telemetry | Success/failure expose sanitized route, request id, finish reason, and body length; no content fields |
| Consumers | Blocking chat, history, extraction, schema projection, and fallback are covered; streaming remains unchanged |
| Schema | All graph variants require `type`; valid multi-op graph parses; missing discriminator rejects |

## Cross-session completion race: success/failure fixed, recovery still open

The reviewed implementation finished by projection kind only, allowing an
old-session worker to consume a new same-kind in-flight job after rotation.
The current slice carries job id and session id into success/failure and
telemetry, checks scheduler plus ledger session, and records an ignored
superseded completion without mutating the active job.

Required contract:

- thread the same identity through explicit cancellation and shutdown;
- prove simultaneous Notes/Graph behavior; and
- prove persisted scheduler recovery preserves correlation.

Tracked by `audio-graph-ab10`.

### Final adversarial correction: same-session replacement

The first identity fix still allowed two gaps: scheduler reset reused the local
`session/kind/index` identifier, and a worker continued into materialization even
when its generation telemetry was rejected as superseded. The corrected runtime
contract now:

- preserves each projection kind's monotonic launch counter across in-process
  reset, so a reset cannot recreate an old identity for the same session while
  offline replay remains deterministic;
- treats generation-ownership rejection as a terminal discard before apply;
- rechecks job/kind/session ownership while holding the scheduler lock through
  the runtime apply decision; and
- records the rejected completion through the content-free superseded decision
  path without consuming the replacement job.

Regression coverage resets and relaunches a Notes generation in the same session
from inside the old worker, then proves the old patch emits no projection event,
does not materialize, does not charge replacement metrics, and leaves the new job
active. Explicit cancellation, shutdown, and persisted recovery correlation stay
open under `audio-graph-ab10`.

## Automated verification

```powershell
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo +1.95.0 clippy --manifest-path src-tauri/Cargo.toml --lib --tests `
  --no-default-features --features cloud --locked -- -D warnings
$env:AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST = "1"
cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph --lib `
  --no-default-features --features cloud --locked projection `
  -- --nocapture --test-threads=1
```

OpenRouter fixtures must cover the budget, envelope, retry, error,
telemetry, consumer, and schema matrix above.

## Round 4 manual gate

Use continuous speech long enough to keep transcript appends ahead of
projection generation.

Pass only if:

- useful notes become visible before the speaker pauses
- no graph response-decode or missing-`type` failures occur
- selected provider, served model, finish reason, and reasoning-token telemetry
  are present and content-free
- append-only progressive notes are followed by a background update
- revised spans trigger repair without applying invalid patches
- persisted transcript/projection JSONL and materialized notes/graph reload and
  replay identically

## Implementation order

1. Completed: repair the pure `caad` classifier/scheduler policy under
   ADR-0031 while leaving durability-gated runtime apply unchanged.
2. Completed for success/failure: add job/session-correlated completion;
   cancellation and persisted recovery remain in `audio-graph-ab10`.
3. Implement the graph-specific OpenRouter budget and response decoder in
   `audio-graph-10ac`.
4. Run full automated gates, then implement the Accepted durable fold after
   `audio-graph-90f3`.
5. Run Round 4.
6. Close `caad` and `10ac` only after the manual and replay evidence passes.
