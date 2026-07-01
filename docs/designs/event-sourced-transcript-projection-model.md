# Event-Sourced Transcript Projection Model

## Purpose

AudioGraph's durable meeting memory is event-sourced. Transcript span revisions
are the source events. Notes and temporal graph artifacts are derived
projections that can be replayed, repaired, or retconned when later transcript
revisions make earlier LLM output stale.

This document records the current migration boundary for `audio-graph-ad44`.
It is intentionally narrower than the product vision: it defines what is
already source-of-truth, which legacy artifacts still exist, and what must be
finished before the event-sourced model can be called closed. The decision to
keep the file-canonical append-only event logs (rather than adopt an embedded
database) is ratified in
[ADR-0021](../adr/0021-storage-architecture.md); the notes-as-narrative
projection that consumes this basis is described in
[ADR-0014](../adr/0014-notes-synthesis.md).

## Source Of Truth

The authoritative inputs for synthesis are transcript event rows in:

- `transcripts/<session>.events.jsonl`
- `transcripts/<session>.speaker.jsonl`
- `projections/<session>.events.jsonl`

`TranscriptEvent` rows are immutable span revisions. A row carries provider,
source, optional provider item, optional speaker/channel metadata, timing,
text, finality, revision number, supersession metadata, latency metadata, and
receipt time (`src-tauri/src/projections.rs:33`). The `TranscriptLedger` replays
these rows into the latest accepted span revision per `span_id`, rejecting stale
or conflicting revisions (`src-tauri/src/projections.rs:562`).

`DiarizationSpanRevision` rows are the immutable speaker-timeline events
(`src-tauri/src/projections.rs:218`). A row carries the provider-neutral
`span_id` (the durable identity), the producing engine, the logical
`timeline_id`, optional source/channel provenance, the resolved
local/canonical speaker id and label, the raw provider speaker id (provenance
only — never identity), timing, an opt-in confidence, finality, a
`DiarizationEventStability` of `provisional`/`stable`/`final`, revision number,
supersession metadata, the ASR span/segment ids the attribution was built from,
and latency/receipt metadata. The `SpeakerTimeline` ledger replays these rows
into the latest accepted revision per `span_id` with the same
stale/conflict/supersede semantics as the transcript ledger — a `provisional`
attribution is replaced by the `stable`/`final` remap of the same `span_id`
(`src-tauri/src/projections.rs:339`).

`ProjectionPatch` rows are accepted notes or graph operations. A patch stores
its `ProjectionBasis`, kind, sequence, LLM request id, operations, confidence,
and provenance. Runtime code must basis-check a patch before accepting it into
the projection log.

Materialized artifacts are cacheable outputs:

- `notes/<session>.json`
- `graphs/<session>.materialized.json`

They are not the canonical source when the event logs can replay. Current
session-load behavior can hydrate returned and in-memory materialized state
from transcript events plus accepted projection patches. Rewriting missing or
stale materialized files, or quarantining artifacts that are ahead of the logs,
is still closure work for this migration.

## Legacy Artifacts

The legacy temporal graph autosave path still uses:

- `graphs/<session>.json`

During the migration this file can coexist with
`graphs/<session>.materialized.json`. Code and docs must not treat the legacy
graph snapshot as the authoritative temporal graph once projection event logs
exist for the session. Export, delete, and load workflows should include both
legacy artifacts and the event-sourced artifacts until the migration is
complete — the default artifact descriptor set already enumerates the legacy
transcript, transcript events, diarization events, projection events,
materialized notes, legacy graph, and materialized graph
(`src-tauri/src/persistence/mod.rs:844`).

The legacy transcript view (`transcripts/<session>.jsonl`, one
`TranscriptSegment` per line) is similarly derivable from the event log. The
read-only projection `derive_legacy_transcript_segments` collapses a
`TranscriptLedger` to one duplicate-free segment per surviving span in canonical
order (`src-tauri/src/projections.rs:693`), and
`load_transcript_segments_preferring_ledger` resolves the segment view by
replaying `<session>.events.jsonl` first and falling back to the legacy rows
only when the event log is empty (`src-tauri/src/persistence/mod.rs:2560`).
Neither path migrates or mutates either file. The command-layer session load
(`read_session_transcript`, `src-tauri/src/commands.rs:5781`) still reads the
legacy `<session>.jsonl` rows directly; switching it onto the
prefer-the-ledger reader is remaining migration work, not yet done.

## Projection Basis Contract

`ProjectionBasis` contains (`src-tauri/src/projections.rs:157`):

- `span_revisions`: the exact transcript `span_id` and `revision_number` set
  used to build a projection job or patch.
- `diarization_span_revisions`: the speaker-timeline `span_id`/`revision_number`
  set the projection consumed. Populated from
  `SpeakerTimeline::current_basis_spans` via
  `ProjectionBasis::from_transcript_events_and_speaker_spans`
  (`src-tauri/src/projections.rs:175`); a transcript-only projection passes an
  empty slice.
- `transcript_hash`: a deterministic hash of the latest transcript events used
  by the basis.

Live application validates a patch basis against the current `TranscriptLedger`
before materializing it (`src-tauri/src/projections.rs:602`). Historical replay
validates each patch against the transcript history that existed at or before
the patch timestamp, so an older valid patch is not rejected only because later
transcript rows arrived.

The diarization portion of basis validation has landed.
`TranscriptLedger::validate_basis_with_speaker_timeline` accepts an optional
`SpeakerTimeline` (`src-tauri/src/projections.rs:613`):

- With a timeline, `SpeakerTimeline::validate_diarization_basis` checks the
  cited diarization spans for full coverage and per-span staleness exactly the
  way transcript spans are checked, and reports
  `StaleDiarizationSpanRevision` / `MissingCurrentDiarizationSpan` /
  `UnknownDiarizationBasisSpan` (`src-tauri/src/projections.rs:446`).
- Without a timeline, a non-empty `diarization_span_revisions` basis is rejected
  as `DiarizationBasisUnavailable` (`src-tauri/src/projections.rs:621`), so a
  speaker-aware patch can never be applied against an absent timeline. A
  transcript-only patch (empty diarization basis) is unaffected — the
  diarization check is opt-in per projection
  (`src-tauri/src/projections.rs:456`).

`MaterializedProjectionState::apply_validated_patch_with_speaker_timeline`
threads the timeline through application so the speaker-aware path is
basis-checked before any notes/graph mutation
(`src-tauri/src/projections.rs:1905`).

What is still open is the *live emission* of diarization revisions: the runtime
does not yet append `DiarizationSpanRevision` rows during capture (no caller of
`Repository::append_diarization_span_revision` outside its default impl and
tests, `src-tauri/src/persistence/mod.rs:497`). Until the live diarization
pipeline writes the `speaker.jsonl` log, the timeline replays empty and
speaker-aware notes/graph projections cannot yet be exercised end to end.

## Runtime Flow

1. ASR providers emit span revisions.
2. The backend converts revisions into `TranscriptEvent` rows and records them
   in the live `TranscriptLedger`.
3. Final spans or end-of-turn markers wake projection scheduling.
4. Projection jobs capture the current basis and dispatch to the LLM patch
   generator.
5. Returned JSON patches are validated and repaired if needed.
6. Accepted patches are basis-checked, appended to the projection event log,
   applied to materialized notes or graph state, persisted as materialized
   artifacts, and emitted to the frontend.
7. Frontend reducers render materialized notes and graph updates as revisions,
   not append-only summaries.

This flow preserves dynamic retcon behavior: later transcript revisions can
invalidate pending LLM work, force a repair path, or produce a new patch that
updates prior notes and graph nodes instead of appending duplicate facts.

## Durability Caveat

The current writer APIs are non-blocking queues. Successful enqueue is not the
same as an fsync-complete durable append. The runtime has been hardened so the
transcript ledger should not advance when the transcript writer is unavailable
or the writer lock is poisoned, but projection materialization still has an
important crash window: a patch can be accepted by runtime state, materialized,
and then crash before the projection event writer flushes that patch.

Replay diagnostics can classify materialized artifacts that are ahead of the
accepted logs. Closure requires either:

- durable ordering that prevents materialized notes/graph from outrunning the
  projection event log, or
- explicit load-time repair/quarantine semantics for ahead artifacts with UI
  diagnostics that make the state non-silent.

Until then, materialized files are useful caches, but event-log durability is
not fully proven under crash timing.

## Migration Checklist

`audio-graph-ad44` can close only when all of the following are true:

- Transcript event logs include the span revisions needed for replay, including
  partial/final retcons that the projection scheduler can reason about.
- Projection event logs are the authoritative accepted patch stream for notes
  and graph.
- Materialized notes and graph can be deterministically rebuilt from event logs.
- Session load repairs missing or stale materialized files from replay.
- Ahead-of-log materialized artifacts are impossible by ordering or handled by
  explicit repair/quarantine.
- Speaker/diarization timeline revisions participate in basis validation. The
  durable contract is done — `DiarizationSpanRevision`, `SpeakerTimeline`
  replay, and `validate_diarization_basis`
  (`src-tauri/src/projections.rs:218`, `:339`, `:446`). The remaining gap is
  live emission: capture must write `DiarizationSpanRevision` rows so the
  timeline is non-empty on replay.
- Legacy `graphs/<session>.json` is documented as compatibility data, not the
  projection source of truth.
- Export/delete/session artifact workflows include transcript events,
  projection events, materialized notes, materialized graph, and legacy
  compatibility artifacts while the migration is active.
- Cross-platform CI verifies replay and load behavior on macOS, Windows, and
  Linux.

## Next Slices

The safest next implementation slices are:

- Wire live capture to append `DiarizationSpanRevision` rows to the
  `speaker.jsonl` log. The durable speaker-timeline contract (event rows,
  replay, basis validation) already landed
  (`src-tauri/src/projections.rs:218`); only the live producer side is missing,
  so `diarization_span_revisions` is currently populated only in replay/eval
  fixtures.
- Switch the command-layer session load onto
  `load_transcript_segments_preferring_ledger`
  (`src-tauri/src/persistence/mod.rs:2560`) so the UI reads the derived,
  duplicate-free ledger view instead of the raw legacy rows.
- Add a projection-log durability test or repair path for materialized
  ahead-of-log artifacts.
- Add a migration note wherever legacy graph or transcript snapshots are still
  presented as canonical.
