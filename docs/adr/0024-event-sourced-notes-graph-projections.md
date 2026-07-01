# ADR-0024: Event-sourced transcript → notes/graph projections (supersedes ADR-0014)

## Status

Accepted 2026-06-30. **Supersedes [ADR-0014](0014-notes-synthesis.md)** (on-demand
notes synthesis). ADR-0014's "on-demand `synthesize_notes` action over
`build_graph_chat_context`" remains a manual, whole-conversation escape hatch
(`commands::synthesize_notes`, wired in `lib.rs`), but it is **no longer the
target architecture** for notes. The live notes/graph surface is now an
event-sourced projection of an immutable transcript event log. This was the
supersession that [ADR-0021](0021-storage-architecture.md) recorded as pending
("ADR-0014 … supersession-pending … via `0d1c`"); seed `ad44` (Event-sourced
transcript/notes/graph synthesis data model) landed the model, so it is now
defined and this ADR records it.

## Context

ADR-0014 made notes an **on-demand prose synthesis**: the user pressed
"Synthesize," and the backend ran one whole-conversation LLM call over
`build_graph_chat_context` + a transcript window. That model has three
structural limits the projection work was built to remove:

1. **No incrementality.** Each synthesis regenerates from scratch; there is no
   way to refine an earlier note when later transcript context corrects it,
   and nothing ties a note to the exact transcript revisions it was built from.
2. **No basis tracking.** Cloud ASR sends partial → final span revisions and
   diarization is remapped after the fact. On-demand synthesis cannot tell
   whether its inputs are still current, so a slow LLM call can silently land
   stale output over fresher state.
3. **Notes and graph drift apart.** The graph already had a temporal model
   (`graph/temporal.rs`, `valid_from`/`valid_until` edges); notes did not. They
   were "parallel views" only by convention, not by a shared event basis.

Seed `ad44` (now CLOSED) defined the durable data model that replaces this; the
runtime contracts now live in `src-tauri/src/projections.rs`,
`src-tauri/src/projection_scheduler.rs`, `src-tauri/src/projection_llm.rs`,
`src-tauri/src/projection_eval.rs`, and the persistence layer
(`src-tauri/src/persistence/mod.rs`). This ADR records that architecture and
retires ADR-0014 as the target for the live notes surface.

## Decision Drivers

- Notes and graph must be **deterministic, replayable projections** of one
  immutable source, so a crash/reload reproduces identical state.
- The system must **reject stale LLM output** by checking the exact transcript
  basis a patch was generated from before applying it.
- Output must be **incremental** (patch operations, not full regenerations) and
  cheap enough to run continuously during a live session.
- The graph's existing **temporal retcon** semantics (`valid_until`
  invalidation, merge/split) must extend to the new patch contract rather than
  being a separate code path.
- Scheduling must be **TTFT-aware** so concurrent transcript revisions coalesce
  into the in-flight job instead of spawning duplicate LLM calls.

## Considered Options

- **Option A — Event-sourced projection with basis-checked replayable patches.**
  Transcript span revisions are immutable events (`TranscriptEvent`); each LLM
  call is a `ProjectionJob` bound to an exact `ProjectionBasis`; the model
  returns a `ProjectionPatch` of `ProjectionOperation`s that materializers apply
  only if the basis still validates. Notes and graph share the basis, the patch
  log, and replay semantics.
- **Option B — Keep ADR-0014's on-demand whole-conversation synthesis** and add
  a debounce/auto-refresh. Cheap to build, but never incremental, never basis-
  checked, and cannot express graph retcon — the three limits above survive.
- **Option C — Continuous full re-synthesis** of notes + graph on every ledger
  tick. Always fresh but burns tokens, races extraction back-pressure, and has
  no way to reject a stale completion that lands after the basis moved.

## Decision Outcome

Chosen: **Option A.** It is the only option that makes notes and graph one
replayable projection of an immutable log, rejects stale completions by
construction, and reuses the graph's temporal retcon model. ADR-0014's manual
`synthesize_notes` survives as a user-triggered convenience but stops being the
architecture. Option B preserves exactly the limits this work removed; Option C
has no staleness defense and is cost-prohibitive.

### Architecture (grounded in code)

#### 1. Immutable transcript events

`projections::TranscriptEvent` is the durable, append-only source event: a
span-scoped revision identified by `span_id` (provider-neutral) with a
monotonic `revision_number`, `supersedes`, partial/final `stability`, and
turn/diarization provenance. It is built from the live
`AsrSpanRevisionPayload` (`From<AsrSpanRevisionPayload>`) and persisted as JSONL
via the projection/transcript event writer (`persistence::mod.rs`). Its `Debug`
redacts `text` (`REDACTED_DEBUG_VALUE`) so logs never leak content.

`TranscriptLedger` (and the parallel `SpeakerTimeline` for diarization)
**replays** these events: a later revision for a `span_id` replaces the earlier
one (partials collapse into their final), a stale revision is rejected
(`TranscriptLedgerError::StaleTranscriptRevision`), and a same-revision
disagreement is a `ConflictingTranscriptRevision`. The legacy
`TranscriptSegment` view is a read-only derivation
(`derive_legacy_transcript_segments`) — one segment per surviving span.

#### 2. ProjectionJob basis checks

`ProjectionBasis` captures the **exact** input a projection was built from: the
`span_revisions` (per-span `revision_number`), the `diarization_span_revisions`,
and a deterministic `transcript_hash` (`transcript_events_hash`, FNV-1a over
canonical fields). `TranscriptLedger::validate_basis_with_speaker_timeline`
returns a typed `ProjectionBasisStaleness` (`StaleSpanRevision`,
`MissingCurrentSpan`, `UnknownBasisSpan`, `TranscriptHashMismatch`, and the
`*Diarization*` variants) when the basis no longer matches the current ledgers.
A `ProjectionJob` (`id`, `session_id`, `kind`, `basis`, `priority`,
`queued_at_ms`) is the unit of in-flight work, and `projection_llm`'s
`projection_patch_prompt_messages` re-validates the basis before it will even
build the prompt.

#### 3. NoteOp / GraphOp patch contracts

A `ProjectionPatch` (`sequence`, `kind`, `llm_request_id`, `basis`,
`operations`, `confidence`, `provenance`, latency fields, `created_at_ms`)
carries a list of `ProjectionOperation`s — a single tagged enum (`schemars`
`JsonSchema`-derived) covering both surfaces:

- **Notes ops:** `UpsertNote`, `DeleteNote`, `ReorderNote` — stable note ids let
  a later patch refine an earlier note in place.
- **Graph ops:** `UpsertGraphNode`/`Edge`, `RemoveGraphNode`/`Edge`,
  `InvalidateGraphNode`/`Edge`, `StrengthenGraphEdge`/`WeakenGraphEdge`,
  `MergeGraphNodes`, `SplitGraphNode`.

Materializers enforce the kind boundary: `MaterializedNotes::apply_patch`
rejects graph ops in a notes patch and vice versa
(`ProjectionApplyError::UnsupportedOperation`), edges require active endpoints
(`MissingGraphNode`), weight deltas are clamped/validated
(`InvalidGraphEdgeWeightDelta`), and patches must advance the sequence
(`StaleSequence`). The model is told to return **only** the operations for its
kind and to omit trusted metadata (`sequence`/`basis`/`provenance` are stamped
by `trusted_projection_patch_from_model_json`, never by the model).

#### 4. Graph retcon engine

The materialized graph reuses the temporal model: `MaterializedGraphNode`/`Edge`
carry `valid_from_ms`/`valid_until_ms`, `InvalidateGraphNode` cascades to its
edges, `MergeGraphNodes` rewrites endpoints then invalidates duplicate active
edges (`invalidate_duplicate_active_edges`), and `SplitGraphNode` invalidates
the source and upserts ≥2 replacements. The LLM prompt explicitly instructs
"Prefer retcon operations over duplicate nodes when later transcript context
corrects earlier assumptions" (`projection_llm::projection_patch_prompt_messages`),
which mirrors `graph/temporal.rs`'s `invalidate_edge`/`valid_until` semantics
(ADR-0008) — the on-demand synthesis path had no equivalent.

#### 5. TTFT-aware scheduling

`projection_scheduler::ProjectionScheduler` owns deterministic queue semantics
per kind (notes + graph via `ProjectionSchedulers`). `observe_ledger` starts a
basis-bound job when the ledger changes; while a job is in flight, newer ledger
state is **coalesced** into the pending basis with a typed reason
(`PendingSpanThreshold` / `InFlightAgeThreshold` / `TtftWindow`) driven by a
`ttft_estimate_ms` that updates from observed generation latency
(`record_generation_result`). On completion, a stale basis is **discarded and a
repair job started** (`DiscardedStaleAndStartedRepair` /
`FailedStaleAndStartedRepair`), and an unchanged failed basis idles instead of
retrying forever.

#### 6. Replay semantics

`MaterializedProjectionState` (notes + graph) replays the accepted patch log:
`apply_validated_patch*` is the **live** path (validates basis before
accepting, so stale completions never become events), while
`apply_replayed_patch` / `replay_accepted_patches` trust the already-accepted
log because a later transcript span would make an earlier valid patch look stale
on replay. `replay_accepted_patches_with_transcript_history` interleaves the
sorted transcript events with patches by `created_at_ms` to produce a
`HistoricalProjectionReplay` with a `HistoricalProjectionValidationReport`
(per-patch `StaleBasis`/`TranscriptReplay` errors). Persistence wires this via
`replay_projection_state` (seeds `6f39`, `60ca`), and `projection_eval`'s
`run_offline_projection_replay` / `offline_projection_replay_fixture_catalog`
provide a no-network harness that drives the same ledger/scheduler/materializer
contracts with deterministic fixtures (no paid provider calls).

### Migration from on-demand markdown notes

- ADR-0014's `synthesize_notes` command stays as a **manual, whole-conversation
  prose escape hatch** (still registered in `lib.rs`); it is not removed, but it
  is no longer the live notes surface and produces ephemeral prose, not durable
  projected notes.
- The live notes surface migrates to `MaterializedNotes` populated by
  basis-checked `UpsertNote`/`DeleteNote`/`ReorderNote` patches, replayed from
  the durable projection event log on session load.
- Session-artifact migration of the transcript + projection event logs is
  tracked separately by seed `9c89` (Session artifact migration for transcript
  and projection events) and gated under ADR-0021's storage decision; this ADR
  does not change file layout.

### Consequences

- **Positive:** Notes and graph are one deterministic, replayable projection of
  an immutable transcript log; crash/reload reproduces identical state.
- **Positive:** Stale LLM output is rejected by construction (basis + sequence
  checks) before it can corrupt materialized state.
- **Positive:** Incremental patches + graph retcon replace full regeneration;
  earlier notes/nodes refine in place as later context arrives.
- **Positive:** Diarization remaps are first-class (provider-neutral `span_id`,
  opt-in diarization basis), not lost across a re-synthesis.
- **Negative:** Substantially more machinery than ADR-0014 (event log, two
  ledgers, scheduler, materializers, replay harness) — justified by the live,
  continuously-updating surface, and validated by the offline replay harness.
- **Neutral:** The on-demand `synthesize_notes` path remains for a one-shot
  whole-conversation prose summary; the two coexist intentionally.

## References

- Superseded: [ADR-0014](0014-notes-synthesis.md) (on-demand notes synthesis).
- Relates to: [ADR-0008](0008-conversation-ontology.md) (typed nodes/relations),
  [ADR-0021](0021-storage-architecture.md) (file-canonical event logs; recorded
  this supersession as pending), [ADR-0012](0012-turn-gated-incremental-prefill-llama-cpp.md)
  (turn-gated extraction), [ADR-0009](0009-design-token-system-and-theming.md)
  (not relevant).
- Code: projection contracts `src-tauri/src/projections.rs`
  (`TranscriptEvent`, `TranscriptLedger`, `SpeakerTimeline`, `ProjectionBasis`,
  `ProjectionJob`, `ProjectionPatch`, `ProjectionOperation`,
  `MaterializedNotes`/`MaterializedGraph`/`MaterializedProjectionState`,
  `transcript_events_hash`); scheduler `src-tauri/src/projection_scheduler.rs`
  (`ProjectionScheduler`, `ProjectionSchedulerDecision`, coalescing + repair);
  LLM patch schemas + prompts `src-tauri/src/projection_llm.rs`
  (`projection_patch_draft_json_schema`, `projection_patch_prompt_messages`,
  `projection_patch_repair_prompt_messages`,
  `trusted_projection_patch_from_model_json`); replay harness
  `src-tauri/src/projection_eval.rs` (`run_offline_projection_replay`,
  `offline_projection_replay_fixture_catalog`); persistence
  `src-tauri/src/persistence/mod.rs` (`append_projection_patch`,
  `load_projection_patches`, `replay_projection_state`); graph retcon
  `src-tauri/src/graph/temporal.rs` (`invalidate_edge`, `valid_from`/
  `valid_until`).
- Manual escape hatch: `commands::synthesize_notes` (`src-tauri/src/commands.rs`,
  registered in `src-tauri/src/lib.rs`).
- Seeds: `ad44` (closed — data model), `6f39`/`60ca` (closed — replay
  report/fixture), `9c89` (open — session artifact migration), `0d1c` (this
  supersession).
