# ADR-0025: STT→LLM context efficiency + diff-based note/graph retroactive updates (extends ADR-0024)

## Status

Proposed 2026-07-04. **Extends [ADR-0024](0024-event-sourced-notes-graph-projections.md)**
(event-sourced notes/graph projections). ADR-0024 established the immutable
transcript log, basis-checked `ProjectionPatch`, the `ProjectionOperation` enum,
and the graph retcon engine. This ADR records the decisions for two things
ADR-0024 left open: (1) how the projection prompt is *fed* (today the whole
transcript is re-sent every tick), and (2) that the graph's supersede-not-delete
retcon model is **not yet mirrored on the notes surface** (notes are still
whole-body-replaced). It also records a privacy decision the efficiency work
forces. Full design + citations: `docs/plans/2026-07-04-stt-llm-context-efficiency-design.md`
(research workflow `w2jr8fyzs`). Tracked by epic `d7bb` (9 child seeds).

This ADR is **Proposed**, not Accepted — it is filed for review alongside the
design doc; no code has changed. It becomes Accepted when the first vertical
slice (below) lands.

## Context

ADR-0024 made notes and graph replayable projections of an immutable transcript
log, with basis-checked incremental patches and a temporal graph retcon engine
(`valid_until` invalidate, merge, split). Three gaps remain, surfaced by a
codebase-grounded research pass:

1. **The projection prompt re-sends the full transcript every tick.**
   `basis_events()` / `format_transcript_events_json()`
   (`projection_llm.rs:509-540`) feed the entire transcript on each call, so
   token cost grows O(n²) over a session even though the scheduler already
   computes the per-tick delta (`basis_revision_delta_count`,
   `projection_scheduler.rs:282`). No rolling summary, no delta-only feed, no
   prompt-cache reuse — even though the transcript ledger is already append-only
   (the exact precondition prompt caching needs).

2. **Notes are the weaker retcon surface.** `UpsertNote` full-replaces the note
   body (`projections.rs:1203-1227`) — the silent-overwrite vector. The graph
   has bitemporal `valid_from/until_ms`, `InvalidateGraphNode/Edge`,
   `MergeGraphNodes`, `SplitGraphNode`; notes have none of it, no sub-note
   atoms, and hard `DeleteNote`. And the live `SpeakerLabelRemap` retcon
   producer (`projections.rs:349-461` → `graph.supersede_entity`,
   `speech/mod.rs:365`) has **no notes-side consumer** — a speaker
   re-attribution fixes the graph but not the notes.

3. **The LLM is not taught patch-in-place vs. record-a-supersession.** The
   prompt says "prefer retcon operations over duplicate nodes" (ADR-0024 §4) but
   does not distinguish *correcting a fact* (revision) from *the world changed*
   (supersession across real-world time), nor type facts as static vs.
   dynamic/temporal — so it cannot decide when a `valid_until` invalidation is
   warranted vs. an in-place `Upsert`.

## Decision Drivers

- Never re-send the whole transcript: token cost must be bounded per tick, not
  growing with session length.
- The staleness guarantee from ADR-0024 must survive: a rolling summary must not
  let a slow completion land stale, so "summarized-through revision" must be
  part of `ProjectionBasis`.
- Notes must gain the same supersede-not-delete, bitemporal, provenance-
  preserving semantics the graph already has — by **extending the existing
  `ProjectionOperation` enum + JSONL log + basis gate**, not a parallel path.
- A failed patch must be a **visible signal**, not a silent corruption: prose
  edits are content-anchored search/replace that the materializer refuses on a
  bad anchor.
- **Privacy is not optional:** the efficiency layer creates *new* off-device
  artifacts (rolling summary, vendor-cached prefix) that must be recorded in the
  session data-movement ledger (ADR-0023 / seed `70a3`) and gated behind the
  cloud-transfer policy.
- Prefer the laziest sufficient mechanism: reuse the shipped retcon engine; add
  no CRDT / no full bitemporal store where a supersede-edge + `valid_until`
  already suffices.

## Considered Options

- **Option A — Extend the existing retcon substrate (chosen).** Feed a rolling
  summary + delta + stable-prefix cache; add notes ops that mirror the graph
  ops (sub-note blocks, search/replace, soft-invalidate); teach the prompt the
  patch-vs-supersede rule + fact typing; ledger the new flows. All of it slots
  into the ADR-0024 enum / log / basis / scheduler.
- **Option B — Keep full-transcript feed + whole-note replace, add only a
  debounce.** Cheapest, but leaves the O(n²) cost and the silent-overwrite
  vector in place — the two problems this ADR exists to remove.
- **Option C — Introduce a dedicated document CRDT + a full bitemporal triple
  store** for notes/KG. Maximally general, but a large new substrate duplicating
  what the graph retcon engine already does; rejected as over-engineering for a
  single-writer projection.

## Decision Outcome

Chosen: **Option A.** The retcon substrate ADR-0024 built is the
supersede-not-delete, bitemporal, provenance-preserving model the diff-knowledge
research prescribes — already shipping, for the graph. The work is therefore
mostly (a) feed it efficiently and (b) extend it to notes, plus teaching the LLM
the vocabulary it already has. Option B preserves both defects; Option C rebuilds
what exists.

### Architecture (grounded in code)

#### 1. Context efficiency (extends ADR-0024 §2 basis, §5 scheduling)

- **Windowed basis + incremental rolling summary.** `basis_events()` feeds a
  maintained rolling summary of older turns + the last K verbatim turns + the
  delta since last patch. The summary is folded **incrementally** (only the turn
  leaving the hot buffer; never re-summarized from scratch — avoids
  recursive-summary "Telephone" drift). "Summarized-through revision R" is stored
  on/beside `ProjectionBasis` and R goes into the basis, so
  `validate_basis` keeps coalescing/repair correct and a slow completion still
  cannot land stale. (`projection_llm.rs:509-540`, `projections.rs:160`.)
- **Delta-only feed + pinned typed facts.** Send current notes/graph state +
  spans since last patch (the scheduler already computes the delta). Pin
  must-never-lose facts from the graph snapshot at the prompt top rather than
  trusting the prose summarizer (which the research shows inverts negations and
  drops rejection reasons). The KG *is* the structured state.
- **Stable-prefix prompt caching.** Order the prompt static→dynamic
  `[system+schema]→[pinned facts]→[summary]→[append-only transcript]→[per-tick
  metadata]` and set a `cache_control`/`cachePoint` breakpoint on the last stable
  block (`openrouter.rs`, `api_client.rs`, `bedrock.rs`), gated on the already-
  parsed catalog capability (`supports_implicit_caching`/`input_cache_read`). A
  `prompt_cache_key` is scoped per **(session, resolved-provider)**: the executor
  has a provider fallback chain (`llm/executor.rs`), so a mid-session failover
  lands a cold cache by design, and a summary/prefix computed for one vendor's
  tokenizer is meaningless to another. Caching is a best-effort per-provider
  property, not a session-wide guarantee.

#### 2. Notes gain the graph's retcon semantics (extends ADR-0024 §3)

New `ProjectionOperation` variants, mirroring existing graph ops one-for-one:

- **Sub-note blocks (the one piece with no existing analog).**
  `MaterializedNote` gets addressable `Vec<NoteBlock { block_id, text,
  valid_from_ms, valid_until_ms }>`; ops `UpsertNoteBlock` / `InvalidateNoteBlock`.
  A note becomes a bitemporal collection of claims — the same shape as graph
  nodes — so a claim is superseded (hidden, auditable), not overwritten.
- **Search/replace prose patch.** `ReplaceNoteText { note_id, block_id, search,
  replace }` applied by exact/expanding-unique anchor. The materializer
  **rejects** a non-matching anchor (a signal, per ADR-0024's
  `ProjectionApplyError` pattern), never applies blind. Line-number edits are
  rejected as a design choice (LLMs are unreliable at them).
- **Note-level soft-invalidate.** `MaterializedNote` gets `valid_from_ms`/
  `valid_until_ms` + `InvalidateNote { id }` (near-drop-in copy of
  `InvalidateGraphNode`), replacing hard `DeleteNote` for supersession.

All slot into the existing enum, the JSONL log (`state.rs:438-560`), the basis
validation, and the frontend reducer shape (`store/index.ts:253-317`).

#### 3. Patch-in-place vs. record-a-supersession (extends ADR-0024 §4)

The decision reduces to contradiction-type + confidence + **fact type**:

| Concept | Existing op (already ships) | When |
| --- | --- | --- |
| Patch-in-place / revision | `UpsertGraphEdge` (replace-by-id), `Strengthen/WeakenGraphEdge` | correction/refinement of the same fact |
| Supersede / update (bitemporal) | `InvalidateGraphEdge` (`valid_until_ms`) + new `UpsertGraphEdge` | contradiction across real-world times |
| Entity merge / split | `MergeGraphNodes` / `SplitGraphNode` | identity resolution |
| Hidden-but-auditable | `valid_until_ms` filtered from snapshot/delta | all invalidations (soft-delete) |

The work is ~90% prompt: extend `operation_guidance` (`projection_llm.rs:195-202`)
to classify STATIC vs DYNAMIC/TEMPORAL facts, invalidate+new-edge only dynamic
facts on a temporally-overlapping contradiction (scoped to semantically-similar
edges), upsert-replace for same-time corrections. Add a `validate_basis` guard
that **rejects an `Invalidate` on a STATIC fact** (over-retraction guard). An
optional `superseded_by` provenance edge makes retractions traversable if the UI
wants a "what replaced this" view.

The `SpeakerLabelRemap` producer gets a **notes-side consumer**
(`ReattributeNoteSpeaker`) so the same signal that retcons the graph also
re-attributes notes, through the same log + basis gate. Diarization span
revisions are persisted on the live path (a confirmed one-call gap:
`append_diarization_span_revision` exists but is test-only,
`speech/mod.rs:378-427`) so retcons are replayable.

#### 4. Privacy: ledger the new remote-LLM flows (extends ADR-0023 / seed 70a3)

The rolling summary (transcript-derived, `DataClass::Notes`) and the pinned-fact
block (graph-derived, `DataClass::GraphContext`) are **new off-device
artifacts**, and `cache_control` persists the cached prefix on the vendor for the
TTL (a durable off-device copy, not just in-flight). Therefore: emit a
`DataMovementEvent` per projection LLM call
(`ProviderCallStarted/Succeeded/Failed`) via `DataMovementLedgerBuilder`, tagging
`DataClass::Prompts` (hash/size only) + `TranscriptText`; ledger the summary and
pinned-fact block as derived artifacts; record the vendor-side prefix
persistence; and **gate the whole context-efficiency path behind the same
`MovementPolicy` that governs cloud transfer**, so a session pinned to local-only
providers never writes a summary/prefix to a remote cache. The ledger seam is
redaction-safe by construction (no field carries a raw prompt body).

### Phased plan

**First vertical slice (becomes the trigger to move this ADR to Accepted):**
rolling-summary + delta feed + stable-prefix caching **for the Notes scheduler
only**, shipped *with* the §4 data-movement ledgering. Touches `projection_llm.rs`
+ one request builder + `ProjectionBasis`; no new STT work; immediately bounds the
highest-frequency call path's token cost. Measure `tokens_used`
(`executor.rs:638`) before/after.

Then: notes retcon ops (§2) → patch-vs-supersede prompt + fact typing (§3) →
speaker-remap-into-notes fan-out → TurnUnit assembly / eager-EOT (STT batching) →
(optional) app-owned endpointer and sentence sub-segmentation (both need a model
download; lowest ROI, deferrable).

### Consequences

- **Positive:** Per-tick LLM cost bounded (rolling summary + delta + cache-read)
  instead of O(n²); notes gain the graph's supersede-not-delete auditability;
  the LLM stops silently clobbering notes (anchored patches fail loud).
- **Positive:** One retcon substrate for notes + graph; the speaker remap fans to
  both. No new store, no CRDT.
- **Positive:** New remote data flows are ledgered and policy-gated — the
  efficiency work cannot quietly widen the privacy surface.
- **Negative:** Rolling-summary drift is a real failure mode (mitigated by
  incremental folding + pinned typed facts, not eliminated); sub-note block
  granularity is genuinely new code, not a copy.
- **Neutral:** Prompt caching is best-effort per provider; a fallback hop or a
  turn gap beyond the vendor TTL is an expected cold cache, not a bug.

## References

- Extends: [ADR-0024](0024-event-sourced-notes-graph-projections.md) (event-sourced
  projections; the enum/log/basis/scheduler this builds on),
  [ADR-0023](0023-anonymous-analytics-sentry-integration.md) &
  [ADR-0017](0017-unbounded-speaker-diarization.md) (diarization),
  [ADR-0018](0018-converse-turn-state-machine-and-half-duplex.md) (turn FSM),
  [ADR-0008](0008-conversation-ontology.md) (typed nodes/relations; retcon).
- Design + citations: `docs/plans/2026-07-04-stt-llm-context-efficiency-design.md`
  and `docs/plans/research-notes-2026-07-04-stt-llm/` (4 codebase ground notes +
  3 external research notes).
- Code seams: `projection_llm.rs` (prompt/basis/guidance), `projections.rs`
  (ops, materializers, `SpeakerLabelRemap`), `projection_scheduler.rs` (delta),
  `llm/executor.rs` (fallback chain, `tokens_used`), `openrouter.rs`/
  `api_client.rs`/`bedrock.rs` (cache_control), `speech/mod.rs` (diarization
  dispatch, AudioAccumulator seam), `persistence/data_movement.rs` +
  `crates/ipc-contract/src/session_data_movement.rs` (ledger).
- Epic: `d7bb` (STT→LLM context efficiency + diff-based note/KG maintenance),
  child seeds `72d5` (2g ledger), `18ee` (2c rolling summary), `d77e` (2d cache),
  `4796` (2e note patching), `6c9a` (2f patch-vs-supersede), `719d`/`262a` (2b
  diarization persist + notes fan-out), `1f56`/`5b1b`/`8edd` (2a STT batching).
