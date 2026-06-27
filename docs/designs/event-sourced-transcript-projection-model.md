# Event-Sourced Transcript Projection Model

## Purpose

AudioGraph's durable meeting memory is event-sourced. Transcript span revisions
are the source events. Notes and temporal graph artifacts are derived
projections that can be replayed, repaired, or retconned when later transcript
revisions make earlier LLM output stale.

This document records the current migration boundary for `audio-graph-ad44`.
It is intentionally narrower than the product vision: it defines what is
already source-of-truth, which legacy artifacts still exist, and what must be
finished before the event-sourced model can be called closed.

## Source Of Truth

The authoritative inputs for synthesis are transcript event rows in:

- `transcripts/<session>.events.jsonl`
- `projections/<session>.events.jsonl`

`TranscriptEvent` rows are immutable span revisions. A row carries provider,
source, optional provider item, optional speaker/channel metadata, timing,
text, finality, revision number, supersession metadata, latency metadata, and
receipt time. The `TranscriptLedger` replays these rows into the latest accepted
span revision per `span_id`.

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
complete.

## Projection Basis Contract

`ProjectionBasis` currently contains:

- `span_revisions`: the exact transcript `span_id` and `revision_number` set
  used to build a projection job or patch.
- `diarization_span_revisions`: reserved for speaker timeline revisions.
- `transcript_hash`: a deterministic hash of the latest transcript events used
  by the basis.

Live application validates a patch basis against the current `TranscriptLedger`
before materializing it. Historical replay validates each patch against the
transcript history that existed at or before the patch timestamp, so an older
valid patch is not rejected only because later transcript rows arrived.

The diarization portion is not implemented yet. The ledger currently rejects a
non-empty `diarization_span_revisions` basis as unavailable. Speaker-aware notes
and graph projections therefore cannot be considered fully durable until the
speaker timeline has event rows, replay, and basis validation.

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
- Speaker/diarization timeline revisions participate in basis validation.
- Legacy `graphs/<session>.json` is documented as compatibility data, not the
  projection source of truth.
- Export/delete/session artifact workflows include transcript events,
  projection events, materialized notes, materialized graph, and legacy
  compatibility artifacts while the migration is active.
- Cross-platform CI verifies replay and load behavior on macOS, Windows, and
  Linux.

## Next Slices

The safest next implementation slices are:

- Add speaker timeline event rows and replay so `diarization_span_revisions`
  can be populated and validated.
- Add a projection-log durability test or repair path for materialized
  ahead-of-log artifacts.
- Update export/session artifact bundling to include the complete
  event-sourced artifact set.
- Add a migration note wherever legacy graph or transcript snapshots are still
  presented as canonical.
