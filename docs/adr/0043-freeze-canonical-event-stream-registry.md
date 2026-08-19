---
status: accepted
date: 2026-07-10
deciders: [AudioGraph maintainers]
---

# ADR-0043: Freeze the Canonical Event Stream Registry

## Context and Problem Statement

ADR-0027 makes transcript revisions, speaker revisions, accepted projection
patches, and data-movement events canonical session streams. ADR-0035 and
ADR-0036 freeze canonical-log v1 framing and recovery behavior, but no accepted
decision assigns production `stream_id` values or defines what the outer
`domain_schema_version` versions.

Those values are not display labels. Canonical-log v1 validates them during
read, includes the stream identity in legacy synthetic commitments, and
includes it in framed record commitments. Golden mixed-format fixtures and
future writers therefore need one registry before the first production reader
or writer hard-codes the values. The event-payload schema must also be able to
evolve independently from replay aggregates and materialized caches.

## Decision Drivers

- Give strict readers and future writers one stable, reviewable identity per
  canonical event domain.
- Keep event-payload versioning independent from ledger, timeline, and cache
  representation versions.
- Reject a framed record that claims the wrong stream or payload schema rather
  than guessing from a path or Rust type.
- Preserve legacy-prefix/framed-suffix replay with deterministic commitments.
- Make later migrations and multi-version dispatch explicit.

## Considered Options

- Use semantic event-domain IDs with independent outer schema constants.
- Reuse current path/API vocabulary and aggregate schema constants.
- Derive stream IDs and versions from Rust type names or artifact paths.
- Use one generic session-event stream and discriminate only inside payloads.

## Decision Outcome

Chosen option: "Use semantic event-domain IDs with independent outer schema
constants", because it gives persisted events stable identities while allowing
aggregate and cache representations to evolve without invalidating event
payloads.

The production registry is:

| Canonical payload | `stream_id` | Outer schema constant | Initial value |
|---|---|---|---:|
| `TranscriptEvent` | `transcript_revisions` | `TRANSCRIPT_REVISIONS_SCHEMA_VERSION` | 1 |
| `DiarizationSpanRevision` | `speaker_revisions` | `SPEAKER_REVISIONS_SCHEMA_VERSION` | 1 |
| `ProjectionPatch` | `projection_patches` | `PROJECTION_PATCHES_SCHEMA_VERSION` | 1 |
| `DataMovementEvent` | `data_movement_events` | `DATA_MOVEMENT_EVENTS_SCHEMA_VERSION` | 1 |

The registry is centralized in the file-canonical persistence adapter. IDs are
semantic and do not derive from file suffixes, command DTO field names, Rust
type names, or user-facing labels. The outer schema version describes the
serialized canonical event payload plus its replay contract; it must not alias
`TranscriptLedger::SCHEMA_VERSION`, `SpeakerTimeline::SCHEMA_VERSION`, or a
materialized notes/graph schema.

`DataMovementEvent` also carries an inner `schema_version`. For v1, a reader
validates that inner value against `DATA_MOVEMENT_SCHEMA_VERSION` and the
registry's supported outer mapping. A contradictory or unsupported pair fails
closed. Future event schemas require explicit version dispatch or migration;
readers do not reinterpret an unsupported version through a current Rust type.

The v1 payload structs continue their current Serde compatibility behavior:
unknown payload object members may be ignored by typed decoding, while missing
required fields or unknown enum variants fail payload decoding. Tightening
payload member policy later requires an explicit domain-schema decision and
fixtures; it does not silently redefine domain schema v1.

### Consequences

- **Positive**: Every strict reader and future writer shares one durable
  stream/schema mapping.
- **Positive**: Replay aggregate and materialized-cache schemas can change
  without changing canonical event identity.
- **Positive**: Wrong-domain frames fail before a consumer can fall back to a
  legacy derivative or cache.
- **Positive**: Golden fixtures can freeze exact cross-version bytes, hashes,
  and heads against production identifiers.
- **Negative**: The durable names differ from some current path and API terms,
  so developers must use the registry instead of guessing identifiers.
- **Negative**: Four independent outer version constants duplicate bookkeeping
  and require tests to prevent registry drift.
- **Negative**: Because a stream ID participates in commitments, a later rename
  requires an explicit migration or a new stream; it cannot be an alias-only
  refactor.
- **Neutral**: Existing legacy JSONL filenames do not change.

## Pros and Cons of the Options

### Use semantic event-domain IDs with independent outer schema constants

- Good, because names describe durable payload meaning rather than incidental
  code or path vocabulary.
- Good, because event schemas can evolve independently from replay aggregates
  and caches.
- Bad, because names and versions must be maintained in a dedicated registry.
- Bad, because current APIs use partly different terminology.

### Reuse current path/API vocabulary and aggregate schema constants

- Good, because it minimizes new constants and terminology.
- Good, because initial values already happen to be version 1.
- Bad, because path names such as `events.jsonl` do not identify a domain.
- Bad, because changing a ledger or materializer schema could accidentally
  invalidate or mislabel unchanged event payloads.

### Derive stream IDs and versions from Rust type names or artifact paths

- Good, because call sites do not manually select constants.
- Good, because the mapping appears self-updating during refactors.
- Bad, because a Rust rename or path migration would silently change persisted
  commitments.
- Bad, because alternative adapters and other languages may not share the same
  type or path vocabulary.

### Use one generic session-event stream and discriminate only inside payloads

- Good, because one ordered file could simplify discovery and global export.
- Good, because only one outer stream identity needs registration.
- Bad, because unrelated producers would share one contention and corruption
  domain.
- Bad, because it would replace ADR-0027's independent streams and per-stream
  heads with a new total-order design.

## More Information

- Governing storage decision: ADR-0027.
- Commitment and recovery contracts: ADR-0035 and ADR-0036.
- Reader implementation Seed: `audio-graph-6896`.
- Runtime writer adoption remains blocked by `audio-graph-8e73` and the other
  `audio-graph-90f3` durability gates.
