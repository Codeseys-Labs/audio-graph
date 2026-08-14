# AudioGraph MVP storage audit

Date: 2026-07-09

Source-anchor note: unqualified line references in discovery findings refer to
HEAD `f97e19c`. Symbols and dated decision/implementation checkpoints are
authoritative for the current working slice.

Status: code audit complete; read-only historical load, serialized Review/Live,
two-phase capture/audit startup, ordered audio epoch reset, rotation fencing,
and contained retry-safe deletion implemented; crash-durable `Accepted`, one
typed manifest, mid-run worker supervision, and concurrent Review remain open

Owning Seeds:

- `audio-graph-5c57` — audit and decision
- `audio-graph-90f3` — authoritative durable commit protocol
- `audio-graph-1f71` — isolate historical review from the active session
- `audio-graph-4521` — atomic session lifecycle and writer rotation
- `audio-graph-9c89` — complete session lifecycle
- `audio-graph-be7c` — one typed artifact manifest
- `audio-graph-617e` — persist projection scheduler mutations
- `audio-graph-34be` — versioned envelopes and migrations
- `audio-graph-70a3` and `audio-graph-51e0` — complete data-movement evidence
- `audio-graph-2d22` — retention, quota, and compaction
- `audio-graph-21c4` — demand-gated derived query index

Decisions:

- [ADR-0027](../adr/0027-file-canonical-durable-session-store.md) accepts the
  file-canonical durability and artifact-lifecycle contract.
- [ADR-0029](../adr/0029-gate-rebuildable-query-indexes-on-measured-demand.md)
  accepts the measured-demand gate for disposable query indexes.

## Verdict

Keep file-backed storage as the only MVP store. Do not make SurrealDB
selectable or default.

The file architecture is the right MVP shape—human-readable canonical event
logs plus rebuildable projections—but the current runtime does not yet satisfy
that architecture's durability claims. One of the two discovery P0s has landed;
the remaining crash-consistency P0 still blocks trustworthy Accepted/Saved
claims:

1. **Open:** a UI-visible accepted event must have one authoritative durable
   commit.
2. **Landed:** historical Review reads and replays artifacts without mutating
   live `AppState`, its writers, or autosave ownership.

After those fixes, implement the accepted file-canonical contract and reserve
SurrealDB, SQLite, or another engine for a later disposable query index. A derived index is
justified only by a named cross-session feature that exceeds a measured
file-replay latency budget.

## Actual topology

| Artifact | Current role |
| --- | --- |
| `sessions.json` | Rebuildable session index/pointer, capped at 100 |
| `transcripts/<id>.events.jsonl` | Intended canonical transcript revision log |
| `transcripts/<id>.speaker.jsonl` | Intended canonical diarization timeline |
| `projections/<id>.events.jsonl` | Canonical-intent notes/graph patch stream; queue admission is not durable `Accepted` |
| `transcripts/<id>.jsonl` | Legacy final-transcript compatibility view, independently written |
| `notes/<id>.json` | Derived notes snapshot |
| `graphs/<id>.materialized.json` | Derived event-sourced graph snapshot |
| `graphs/<id>.json` | Separate non-replayable legacy graph used by timeline links and autosave |
| `projections/<id>.scheduler_queue.json` | Operational projection queue snapshot |
| `usage/<id>.json` | Token accounting |
| `live_assist/<id>.*` | Audit log and current snapshot |
| `ledgers/<id>.movements.jsonl` | Privacy and data-movement audit |
| SurrealDB | Partial feature-gated Mem (`kv-mem`) experiment; no runtime selection or repository parity |

Current flow:

```text
ASR revision
  -> bounded enqueue
  -> live TranscriptLedger advances
  -> buffered transcript event writer
  -> transcript JSONL

TranscriptLedger
  -> projection scheduler
  -> LLM patch
  -> bounded projection enqueue
  -> notes/graph snapshot save
  -> live materializer advances

Diarization
  -> live SpeakerTimeline and legacy graph retcon
  -> best-effort synchronous speaker JSONL append

Legacy extraction graph
  -> mutable TemporalKnowledgeGraph
  -> 30-second atomic JSON snapshot
```

Two graph generations remain live. The event-sourced projection graph is
replayable; the legacy mutable graph is not. Session lifecycle must either
retire the legacy path or explicitly inventory and reconcile it.

## P0 integrity findings and current status

### Accepted state is not crash-durable

The default transcript and projection writers use buffered `writeln!` and
primarily flush on shutdown. They do not synchronize every accepted event to
the event file:

- `src-tauri/src/persistence/mod.rs:1649-1697`
- `src-tauri/src/persistence/mod.rs:2027-2081`
- `src-tauri/src/persistence/mod.rs:2279-2338`

Queue acceptance can advance the live transcript ledger
(`speech/mod.rs:1641-1706`) and projection state
(`state.rs:526-575`). A power loss, process kill, ENOSPC, short write, or
delayed I/O error can therefore occur after the product has reported success.

The projection path has an additional split-brain sequence:

1. enqueue the canonical patch
2. save the materialized snapshot
3. verify session and commit memory

Discovery found that an enqueue followed by snapshot failure left live
`last_sequence` behind, so a retry could reuse the same sequence and poison
replay. The current slice repairs that focused logical split: once the canonical
writer accepts the patch, live materialized state advances even when the JSON
snapshot fails, and the result reports rebuildable cache lag. A fault test
injects first-snapshot failure, accepts sequence 2 next, and proves canonical
replay equals live state. This does **not** close the P0 because current writer
acceptance is still queue admission rather than the named crash-durable boundary;
event I/O can fail later.

Diarization applies live state first and persists best-effort
(`speech/mod.rs:422-467`), which belongs under the same commit protocol.

Required correction:

- one idempotency key and authoritative commit result
- durable canonical event success commits live state
- snapshot failure means recoverable cache lag, not logical event failure
- retry cannot reuse or duplicate a committed sequence
- missing writer, full queue, ENOSPC, short write, flush/sync failure, rotation,
  shutdown, and diarization obey the same rule

Tracked by `audio-graph-90f3`.

### Historical Review isolation — backend resolved, concurrent workspace open

At discovery HEAD `f97e19c`, `load_session` replaced the live legacy graph,
transcript ledger, materialized projections, and scheduler state without
rotating the active `session_id`, writers, or backend `speaker_timeline`
(`commands.rs:6174-6293` at that anchor).

The current slice removes the `State<AppState>`/`&AppState` argument from the
Tauri command and implementation. `load_session` now validates and replays the
requested historical artifacts solely into `LoadedSession`; it does not replace
the active legacy graph, transcript ledger, materialized projection state, or
schedulers. The frontend now rejects Review Open during capture/transcription
and clears every historical projection before starting live capture. This
serializes the two modes safely; it does not yet implement ADR-0028's concurrent
Review-while-Live workspace or session-scoped event envelopes.

Focused Rust tests cover:

- replaying notes/graph when derived artifacts are missing;
- preferring replayed historical payloads without mutating a deliberately
  seeded active materializer or transcript ledger;
- returning isolated payloads for sequential session A/session B Review; and
- persisted diarization present and absent cases without live-state binding.

Resume is intentionally not inferred from Review. A future Resume command must
atomically rotate the complete active-session tuple and remains open lifecycle
work.

Tracked by `audio-graph-1f71`.

## P1 correctness, privacy, and operability gaps

### Incomplete artifact lifecycle

The current slice expands the production inventory to 18 managed paths,
including usage, scheduler state, live-assist audit/current snapshots,
data-movement ledger, and atomic JSON temp residues. Soft and permanent delete
commands reject the active session. Permanent deletion and retention purge now
unlink artifacts first, remove the index entry last, and preserve/report the
index retry anchor when any residual remains; focused tests cover a partial
unlink and successful retry. Malformed `sessions.json` now blocks mutation
after a content-preserving backup. Destructive unlink accepts only the exact
target-session inventory, so a tampered in-root pointer cannot delete another
session or the index itself. Retention purge excludes the backend's current
session.

This is a safety improvement, not completion of the manifest contract.
Repository conformance and export still use a separate descriptor/load shape;
recovery does not consume the same inventory; export omits usage, scheduler,
live-assist audit, and data-movement records; and failure crosses Tauri as a
human-readable error rather than a typed residual payload.

Path containment is still check-then-unlink. A same-user race that replaces a
parent with a junction/symlink after validation needs a future
directory-handle/no-follow implementation. Stale legacy index paths now fail
safe and preserve the recovery anchor rather than gaining deletion authority.

Export, backup, delete, purge, recovery, storage accounting, and migration must
all consume one typed backend-owned artifact manifest. Destructive operations
must quiesce writers and return a residual manifest when anything remains.

Tracked by `audio-graph-be7c`.

### Scheduler queue is not crash-persisted

Queue state is saved during clean session rotation and directly by tests, not
after semantic enqueue, start, complete, or failure mutations. Historical
`load_session` is intentionally read-only and no longer restores a queue into
the active runtime; after a crash, persisted in-flight/pending work therefore
still lacks a production Resume/recovery owner.

The previously closed `audio-graph-617e` has been reopened.

### JSONL recovery is fail-stop

`load_jsonl` rejects the entire stream on any malformed line
(`persistence/mod.rs:2604-2636`). Append paths neither frame records nor
truncate or quarantine an incomplete final row. A torn final write followed by
another append can concatenate records and make the whole stream unreadable.

Recovery must preserve every valid prefix row, truncate and quarantine a torn
final record, and fail loudly on interior corruption.

Tracked under `audio-graph-90f3`.

### Schema migration is undefined

Canonical transcript, speaker, and projection events, session metadata, and
scheduler state lack durable envelope versions. Materialized snapshots declare
`schema_version`, but readers do not validate or migrate it. Export declares
bundle version 1 without an import/migration path.

Tracked by `audio-graph-34be`.

### Snapshots can override canonical logs

Discovery load preferred a snapshot at or ahead of replay sequence without
content or basis-hash equality. The current slice now chooses replay whenever
that canonical stream contains accepted content, including when a cache claims
an ahead sequence; canonical transcript revisions likewise produce Review rows
before the legacy segment file is considered. File existence now marks the
canonical-era projection authority even when the stream is empty. A crash-lost
empty log therefore materializes explicit empty notes/graph state and cannot
promote an orphan ahead cache into Review truth.

The full recovery contract remains open: each snapshot must record the canonical
stream head and basis hash, and load must rebuild or quarantine missing, behind,
ahead, or equal-sequence/different-content caches rather than merely ignoring
them for the response.

Tracked under `audio-graph-90f3`.

### Data route evidence is incomplete

The accepted data-movement scope includes capture, providers, artifact
write/load/export/delete, credentials, projection, and promotion. In production,
capture now contributes a synchronized first-source `CaptureStarted` before
audio release and one last-source `CaptureStopped` for explicit, fatal, and
clean-shutdown paths. Movement JSONL appends are serialized within the process,
so concurrent producers cannot interleave rows; JSONL order, not pre-lock wall
clock order, is the UI lifecycle authority. The projection LLM sink remains the
other production producer. ASR, TTS/realtime, load, export/delete, credentials,
and promotion still do not prove the accepted coverage matrix.

`audio-graph-70a3` and `audio-graph-51e0` have been reopened. The UI must
show an Unknown or Incomplete state until the backend emits an explicit,
versioned exhaustive-producer coverage marker. A closed capture validates only
the capture lifecycle; it cannot prove uninstrumented paths stayed local.
Positive off-device rows remain valid evidence of egress from a partial ledger.

### Retention silently orphans sessions

Registering session 101 removes the oldest entry from the index without deleting
or archiving its artifacts (`sessions/mod.rs:243-247`;
`persistence/mod.rs:1022-1024`). The UI asks for up to 200 sessions.

There is no quota, size view, compaction, or retention rule for non-trashed
data. Tracked by `audio-graph-2d22`.

## SurrealDB findings

The closed `audio-graph-2b2c` experiment is real and useful:

- SurrealKV and RocksDB build and link natively on Linux, macOS, and Windows
- a keyed experimental SurrealKV benchmark reached file-append parity
- RocksDB carried a larger binary and native dependency cost

This invalidates superseded ADR-0021's old statement that the engine gate had
no evidence. It does not validate the production adapter:

- it imports and constructs `Mem` only
- it stores opaque schemaless JSON
- it full-scans tables for sequence assignment and loads
- it lacks speaker and data-movement coverage
- no production caller selects it
- Cargo enables only `kv-mem`
- the keyed benchmark kept max sequence in memory; it did not prove
  transactional concurrent sequence allocation

Its deletion path also removes session metadata before child rows, so a later
child-table failure destroys the retry anchor. Before this adapter can become
selectable it must delete children first in one transaction, implement
diarization and data-movement methods instead of trait-default errors, and pass
the same artifact lifecycle/retry contract as the file store.

The integrated Rust 1.95 cloud library suite executes the current file-store,
malformed-index, replay, movement, and deletion regressions as part of 1,498
passing tests (zero failed, eight explicitly ignored). This proves the local
assertions that ran; it does not prove crash durability or packaged three-OS
filesystem behavior.

Engine packaging proof is not adapter durability, migration, backup, deletion,
or concurrency proof.

## Required MVP invariants

- One active-session tuple owns id, writers, transcript ledger, speaker
  timeline, buffers, both graph states, schedulers, and autosave.
- Canonical streams use per-stream monotonic heads plus stable event/causal ids;
  the session manifest and every derived snapshot record the complete head
  vector.
- A canonical session-provenance stream records lifecycle, redacted source kind
  and stable id, negotiated format, source/session clock mapping,
  discontinuities, drop summaries, and provider-route transitions.
- Historical Review is side-effect-free (implemented); Resume is separately
  transactional (open).
- A UI-visible Accepted event has a durable acknowledgement.
- Only explicitly Pending events may be lost.
- Canonical logs always win over derived snapshots.
- Every snapshot records its canonical stream head and basis hash.
- A torn final event preserves all valid prefix events.
- Interior corruption fails loudly and quarantines evidence.
- Every durable event and artifact has a version and idempotent migration.
- One typed manifest drives export, backup, delete, purge, recovery, and usage.
- Permanent deletion quiesces writers and reports every residual.
- Missing writers or repository failures never silently advance durable state.
- Optional indexes are disposable derivatives, never dual-authoritative.

## Options

### File event store as the MVP default — chosen

Benefits:

- human-readable and supportable
- portable across all three desktop targets
- minimal packaging and vendor risk
- natural fit for append-only revision history and recovery

Negative consequences:

- AudioGraph owns crash recovery, migration, retention, and compaction
- strict durability can add I/O latency
- indexed cross-session recall is deferred

### SurrealKV as the canonical default — rejected for MVP

Benefits:

- stronger transaction and query primitives are possible
- one engine could later serve document, graph, and search workloads

Negative consequences:

- production adapter and migration path do not exist
- current local memory becomes opaque
- backup, recovery, deletion, concurrency, and keyed sequencing are unproven
- additional vendor and on-disk-format risk

### File-canonical plus a rebuildable index — later preferred

Benefits:

- indexed recall without risking canonical user memory
- engine replacement remains a rebuild, not a data migration

Negative consequences:

- duplicates sensitive data
- requires deletion and retention parity
- introduces index lag and repair behavior
- increases bundle and operational complexity

Accepted ADR-0029 gates a derived index on a committed cross-session feature
that exceeds a measured file-replay UX budget. Any proposal to make a database
canonical would require a new ADR after full artifact conformance,
transactional keyed append, backup and corruption recovery, migration rollback,
realistic three-OS crash tests, and an explicit product decision to give up
canonical file readability.

## Verification matrix

- hard kill after enqueue, durable acknowledgement, snapshot write, rename, and
  directory synchronization
- ENOSPC, short-write, flush, sync, and rename fault injection
- missing, disconnected, or full writers
- stop and rotate while ASR, diarization, and projections complete
- torn final line versus corrupt interior line
- snapshot missing, behind, ahead, and equal-sequence/different-content
- focused historical-read tests: replay fallback, active-state preservation,
  and load A then B (landed)
- review A while capturing B, including autosave and shutdown integration
- transactional Resume tests if that feature is later added
- delete active, trashed, partially locked, and mixed-generation sessions
- verify zero residuals or an exact residual manifest
- export every typed artifact with old/current/future schema fixtures; import
  remains a separately scoped future feature
- delayed cross-stream replay and head-vector mismatch across transcript,
  speaker, projection, provenance, and movement streams
- eight-hour/high-partial-rate capture, more than 100 sessions, replay latency,
  queue depth, and disk growth
- for any future index: multi-instance append, unique sequence allocation,
  crash transaction, lock/WAL recovery, backup/restore, and full repository
  conformance

## Sequence

1. **Open:** land the canonical durable commit protocol.
2. **Landed:** make historical Review side-effect-free.
3. **Open:** unify the typed artifact manifest and destructive lifecycle.
4. **Open:** persist scheduler mutations and complete data-movement wiring.
5. **Open:** add schema envelopes, torn-tail recovery, and snapshot basis
   validation.
6. **Open:** define retention, quota, and capacity UX.
7. **Open:** measure real cross-session query demand before implementing a
   derived index.

## Clean-worktree implementation scope: 2026-07-09

The smallest independently mergeable `audio-graph-90f3` slice is a canonical-log
kernel plus compatibility readers, not an immediate rewrite of live writers:

- add `persistence/canonical_log.rs` and export it from `persistence/mod.rs`;
- define a versioned framed envelope, per-stream sequence/head, stable event id,
  causal/basis heads, previous-record hash, payload hash, named durability level,
  and durable commit receipt;
- read mixed legacy raw JSONL and framed rows deterministically;
- quarantine and truncate only an unterminated corrupt final tail;
- fail loudly without mutation for interior or newline-terminated corruption;
- poison an appender after any short-write, flush, or sync uncertainty; and
- inject file operations so short write, ENOSPC, flush, file sync, parent sync,
  truncate, and quarantine faults are deterministic tests.

The first slice switches the four canonical readers (transcript, projection,
speaker, and data movement) to compatibility decoding but deliberately does not
change runtime writer/live-state ordering. It is deliberately isolated to the
new module plus `persistence/mod.rs`, uses existing dependencies, and avoids
unrelated dependency and projection integration ranges.

The dependency-ordered runtime slices remain:

1. transcript Pending -> Accepted before live ledger/UI advance;
2. projection job-id commit before materializer advance, with snapshots becoming
   best-effort caches after Accepted;
3. diarization commit before timeline/graph retcon/UI advance;
4. complete head vectors and snapshot basis/hash validation;
5. provenance and movement on the same session commit aggregate; and
6. subprocess kill/reopen recovery on Windows, macOS, and Linux.

Failure semantics are explicit: a pre-write rejection is definitely retryable;
partial write/flush/sync uncertainty is `RecoveryRequired` and poisons the
writer; Accepted means the record crossed the declared filesystem durability
boundary; a later snapshot failure is cache lag, not logical event failure.

Per Wave 3 hygiene, the canonical-log implementation belongs in a flat clean
worktree after its accepted architecture prerequisites are integrated; this
audit intentionally scopes that storage rewrite without layering unrelated
changes into it.
