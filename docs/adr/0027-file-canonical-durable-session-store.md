---
status: accepted
date: 2026-07-09
deciders: [AudioGraph maintainers]
supersedes: ADR-0021
---

# ADR-0027: Adopt File-Canonical Durable Session Storage

## Context and Problem Statement

AudioGraph is a local-first desktop memory product. Transcript and speaker
revisions, notes, graph patches, source provenance, privacy routing, and
operational state must survive capture failure, process termination, restart,
review, export, and deletion on Windows, macOS, and Linux.

ADR-0021 selected file-canonical event logs but described the default buffered
writers as though each accepted event were already synchronized. In the current
runtime, live or materialized state can advance before its canonical event is
durable, a snapshot can outrun its basis log, historical load can cross active
session ownership, and lifecycle operations do not share one complete artifact
inventory. This ADR supersedes ADR-0021 and defines the authoritative session
storage contract.

Whether and when to add a disposable query index is a separate decision in
ADR-0029.

## Decision Drivers

- Never acknowledge user memory that can still be silently lost.
- Preserve portable, inspectable canonical data.
- Keep capture I/O bounded while reporting durability honestly.
- Replay transcript, speaker, projection, provenance, and privacy streams
  deterministically.
- Reconstruct cross-stream causality without inventing one global sequence.
- Prevent review, export, delete, and autosave from crossing session ownership.
- Recover a torn final record while failing loudly on interior corruption.
- Support idempotent schema migration and rollback.
- Prove complete deletion or report exact residual artifacts.
- Use explicit Windows, macOS, and Linux durability primitives.

## Considered Options

- File-canonical event storage with an explicit durable commit protocol
- SurrealKV as the canonical session store
- Dual-authoritative files and SurrealKV
- Keep the current buffered file runtime unchanged

## Decision Outcome

Chosen option: "File-canonical event storage with an explicit durable commit
protocol", because it preserves AudioGraph's local, inspectable memory while
giving acknowledgement, replay, recovery, and deletion one testable authority.

Canonical streams include session lifecycle and provenance, transcript
revisions, speaker revisions, accepted notes and graph projection patches, and
privacy/data-movement events.

Every canonical record carries a schema version, session id, stream id,
per-stream monotonic sequence, stable event or idempotency id, causal event ids,
the basis head vector where applicable, and integrity framing. Independent
streams never claim a shared total order. The canonical session manifest stores
the complete per-stream head vector, and every derived snapshot stores that
vector plus basis hashes.

Bounded enqueue creates a `Pending` record. A record becomes `Accepted` only
after its framed canonical bytes and the filesystem metadata required for replay
cross the declared durability boundary. Live durable state advances from
Accepted events. A snapshot failure is derived-cache lag, not logical event
failure, and snapshots never override canonical logs.

The implementation may use per-event synchronization or a measured short
group-commit window. Both must preserve the Pending/Accepted distinction.
Existing-file data is synchronized before Accepted. Newly created or atomically
replaced files also persist their parent-directory entry where the platform
provides that primitive. If one platform cannot provide an equivalent
guarantee, the backend exposes a weaker named durability level; the UI may not
label it with the same Saved state.

A canonical session-provenance stream records lifecycle transitions, redacted
source kind and stable id, negotiated format, source/session clock mappings,
discontinuities, layer-specific drop summaries, and provider-route transitions.
`sessions.json` and other indexes are rebuildable from canonical session
manifests and streams.

One active-session aggregate owns the session id, writers, transcript ledger,
speaker timeline, graph states, schedulers, buffers, and autosave target.
Historical review is read-only with respect to that aggregate. Resume is a
separate transactional lifecycle operation.

A torn final record preserves its valid prefix and quarantines the tail.
Interior corruption fails loudly and preserves evidence. Legacy transcript and
graph files remain compatibility derivatives until a versioned, idempotent
migration retires them.

One typed artifact manifest, including schema version and privacy class, drives
load, export, backup, delete, purge, recovery, retention, and usage. Permanent
deletion first quiesces active writers, removes every managed artifact, removes
the rebuildable index entry last, and returns an exact residual manifest if any
removal fails. No global tombstone survives by default; a user may explicitly
export a content-free deletion receipt outside the managed store.

### Consequences

- **Positive**: Saved and Accepted acquire one fault-injectable meaning.
- **Positive**: Canonical data remains portable, inspectable, and authoritative
  over every snapshot or index.
- **Positive**: Per-stream heads and causal ids make delayed cross-stream replay
  deterministic.
- **Positive**: Historical review cannot steal live writer or autosave
  ownership.
- **Negative**: Strict acknowledgement can delay visible final transcript or
  note updates.
- **Negative**: AudioGraph must maintain framing, synchronization, recovery,
  migration, compaction, and retention primitives normally supplied by a
  database.
- **Negative**: Legacy compatibility and three-OS filesystem fault testing add
  substantial delivery cost.
- **Neutral**: The repository trait remains useful, but its conformance surface
  expands beyond replay equality.

## Pros and Cons of the Options

### File-canonical event storage with an explicit durable commit protocol

- Good, because user memory remains human-readable and supportable.
- Good, because logs, head vectors, and hashes provide deterministic authority.
- Good, because derived state can always be discarded and rebuilt.
- Bad, because the application owns complex durability and recovery code.
- Bad, because synchronization pressure must be measured and surfaced.

### SurrealKV as the canonical session store

- Good, because database transactions could simplify atomic keyed updates.
- Good, because later query workloads could use one engine.
- Bad, because the current adapter is in-memory, schemaless, and full-scan.
- Bad, because backup, migration, deletion, corruption recovery, and concurrent
  sequencing are not production-proven.
- Bad, because canonical user memory becomes opaque and vendor-format-dependent.

### Dual-authoritative files and SurrealKV

- Good, because indexed queries would be immediately available.
- Good, because two representations can look like redundancy.
- Bad, because split-brain recovery has no deterministic winner.
- Bad, because acknowledgement and deletion span two failure domains.
- Bad, because duplicated sensitive state increases privacy and retention risk.

### Keep the current buffered file runtime unchanged

- Good, because it has no immediate migration cost.
- Good, because queue acceptance has low apparent latency.
- Bad, because acknowledged memory can be lost.
- Bad, because snapshots can become durable ahead of canonical events.
- Bad, because historical load and lifecycle operations can cross session
  ownership.

## More Information

This decision supersedes ADR-0021. Query-index policy is ADR-0029; projection
events build on ADR-0024; session timeline semantics build on ADR-0026.

Validation must inject failures at enqueue, write, flush, sync, snapshot,
rename, directory sync, rotation, and shutdown; cover ENOSPC, short writes,
locked files, missing writers, and full queues; replay torn tails and interior
corruption; compare behind, ahead, and equal-head/different-content snapshots;
exercise review A while capture B remains live; prove typed artifact
export/delete/purge parity; and run packaged restart and recovery tests on all
three desktop operating systems.

Research: `docs/research/mvp-storage-audit-2026-07-09.md`.
Implementation: `audio-graph-90f3`, `audio-graph-1f71`,
`audio-graph-be7c`, and `audio-graph-9c89`.
