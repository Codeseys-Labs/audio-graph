# Transcript/session strict-reader map

Status: complete (read-only discovery lane for Seed `audio-graph-6896`)

## Scope and constraints

This lane maps transcript/session Review, replay, timeline, and export readers. It does not modify runtime code, Seeds, or git state. The target is the smallest non-mutating adapter that can read a legacy JSONL prefix followed by canonical framed-v1 rows, rejects corruption instead of silently discarding it, and preserves the product's missing-versus-existing-empty authority rules.

## Preliminary map

- Legacy display transcript: `transcripts/<session_id>.jsonl`, payload `state::TranscriptSegment`.
- Durable transcript revision ledger: `transcripts/<session_id>.events.jsonl`, payload `projections::TranscriptEvent`.
- On this older branch, session hydration (`load_session`) reads both, replays the revision ledger, then mutates live backend aggregates. Main has already replaced that unsafe behavior with a read-only historical payload, which integration must preserve.
- Timeline and replay-report commands consume repository-loaded transcript revision events; session bundle export consumes both legacy display rows and revision events.

## Critical baseline warning: integrate the main working-tree slice, not this branch in isolation

The discovery branch is based on `104e964` (kernel commit `7b0e5d0`), but the main checkout contains broad uncommitted MVP hardening that is newer than this branch. The relevant differences are not cosmetic:

- Main makes historical `load_session` read-only by removing its `AppState` argument and never rebinding the live graph, transcript ledger, projection state, or scheduler (`E:/CS/github/audio-graph/src-tauri/src/commands.rs:6655-6762`). This branch still mutates those live aggregates (`src-tauri/src/commands.rs:6166-6305`). The strict-reader integration must preserve the main behavior.
- Main already derives Review/export transcript segments from a non-empty transcript event log before falling back to the legacy transcript (`E:/CS/github/audio-graph/src-tauri/src/commands.rs:6563-6591`, `:6783-6823`). This branch still reads the legacy transcript directly and silently skips malformed rows (`src-tauri/src/commands.rs:6077-6094`, `:6325-6369`). The new adapter should complete, not regress, the main migration.
- Main treats projection-log **file existence**, including an existing empty file, as canonical authority and refuses to promote orphan materialized caches (`E:/CS/github/audio-graph/src-tauri/src/commands.rs:6693-6740`; regression `:10991-11030`). This branch decides from `projection_events.is_empty()` and may select caches (`src-tauri/src/commands.rs:6208-6239`).
- Main frontend Review blocks historical loads while capture/transcription is live and rejects stale/out-of-order historical responses (`E:/CS/github/audio-graph/src/store/index.ts:2895-2992`; regressions `src/store/index.test.ts:850-982`). `SessionsBrowser` exposes that lock and only closes after a successful load (`E:/CS/github/audio-graph/src/components/SessionsBrowser.tsx:142-203`, `:347-350`, `:434-447`). These files differ from this branch and must not be overwritten.
- Main's session inventory covers movement, scheduler, usage, live-assist, and atomic-write temp residues and adds path-containment/deletion retry safety (`E:/CS/github/audio-graph/src-tauri/src/sessions/mod.rs:429-534`). This branch's inventory ends at the materialized graph (`src-tauri/src/sessions/mod.rs:403-447`). Preserve main when testing session existence/export/delete behavior.
- Main `persistence/mod.rs`, `projections.rs`, and `state.rs` also contain unrelated but load-bearing writer pre-open, movement serialization, projection-basis, redacted-debug, capture, and rotation changes. In particular, main does not yet contain `pub mod canonical_log`, while this branch does. Do not copy whole files in either direction; integrate the kernel and reader changes hunk-by-hunk onto the MVP-hardened tree.

`timeline.rs`, `user_data.rs`, `LiveTranscript.tsx`, and `SeekTimeline.tsx` are byte-identical between the two trees at discovery time. The other files named above are not.

## Current on-disk types and authority

| Artifact | Current path and payload | Intended role | Current read behavior |
|---|---|---|---|
| Legacy transcript display view | `transcripts/<id>.jsonl`; `state::TranscriptSegment` (`state.rs:41-52`) | Compatibility/derived view, not the event-sourced authority | Branch command reads line-by-line and skips malformed rows (`commands.rs:6077-6094`). Main prefers non-empty event rows, then uses the same lossy legacy fallback (`E:/CS/github/audio-graph/src-tauri/src/commands.rs:6563-6591`). |
| Transcript revision stream | `transcripts/<id>.events.jsonl` (`persistence/mod.rs:809-814`); `projections::TranscriptEvent` (`projections.rs:34-69`) | Authoritative transcript history used by Review, projection basis, timeline, and export | `load_jsonl` is strict per nonblank line but collapses missing and existing-empty to the same `Vec::new()` (`persistence/mod.rs:2605-2637`); repository routing discards path/head metadata (`:1160-1176`). |
| Transcript read model | No file; `TranscriptLedger` then `Vec<TranscriptSegment>` | Latest accepted revision per `span_id`, ordered deterministically | Replay rejects stale/conflicting revisions (`projections.rs:629-694`) and derivation is duplicate-free (`:816-848`). The existing `load_transcript_segments_preferring_ledger` is production-unused on this branch and chooses by row non-emptiness, not file presence (`persistence/mod.rs:2648-2678`). |

`TranscriptEvent` contains `span_id`, provider/source identity, optional provider/segment/speaker/channel metadata, raw transcript text, start/end/confidence, finality/stability, revision/supersession, turn/end-of-turn, optional raw-event reference and latency fields, and receipt time (`projections.rs:34-69`). Its `Debug` redacts only `text` on this branch (`:73-99`); main carries additional redaction hardening elsewhere in `projections.rs` and must remain the payload definition used by the adapter.

The typed canonical kernel can represent both payloads without changing framing: `load_canonical_stream<T>` validates legacy-prefix/framed-suffix structure, context, sequence/hash chain, and typed payloads, returning full `CanonicalRecord<T>` rows plus a stream head (`canonical_log.rs:83-118`, `:540-603`). It rejects legacy rows after the first framed record and validates framed context/version/hash fields (`:1426-1519`, `:1523-1699`). Strict mode is the only allowed mode in this wave.

## Missing versus existing-empty: the rule the adapter must retain

The generic `load_jsonl` and repository trait currently return only `Vec<T>`, so callers cannot distinguish an absent stream from an explicitly empty stream (`persistence/mod.rs:456-514`, `:2605-2646`). That loss is already unsafe for projection caches and main works around it by probing the projection path separately.

The shared reader result should therefore carry at least:

```text
present: bool
records: Vec<CanonicalRecord<T>>
head: Option<CanonicalStreamHead>
```

An `Io(Read, NotFound)` from the canonical loader maps to `present=false`; a successful zero-byte/blank-only load maps to `present=true`, `records=[]`, `head=None`. Other I/O and every parse/context/hash/schema failure are errors. This avoids a preflight `exists()` race and prevents a corruption error from being reinterpreted as a reason to try a legacy fallback.

Consumer policy:

- Transcript event stream missing: load the indexed legacy transcript compatibility file.
- Transcript event stream present with zero events: return an explicit empty transcript. Do **not** promote legacy derived rows; doing so would repeat the orphan-cache bug main fixed for projections.
- Transcript event stream present with events: replay once, derive segments, and return the same event snapshot to `LoadedSession`/export so the view and revision badges cannot disagree.
- Projection stream present (including empty): preserve main's canonical-cache authority rule.
- Diarization/movement currently render missing and empty similarly, but the shared result must retain `present` for manifest, export, privacy completeness, and later recovery decisions.

This intentionally supersedes the old comment in `load_transcript_segments_preferring_ledger` that an empty event file means “no event log” (`persistence/mod.rs:2662-2663`). With ADR-0027 file-canonical authority, file presence is a durable generation signal; if backward compatibility needs a different rule, it requires an explicit migration marker rather than `Vec::is_empty()`.

## Non-mutating-reader blocker in current path resolution

The canonical kernel's `Strict` mode does not quarantine or truncate, but the surrounding path resolution currently mutates the filesystem:

- `user_data::data_root()` and every directory helper call `create_dir_all` (`user_data.rs:24-34`, `:64-92`).
- `FileMemoryRepository::with_data_root` path helpers also call `ensure_dir` (`persistence/mod.rs:729-799`).
- High-level read commands call `sessions::session_artifact_paths_for_id`, whose default paths invoke those create-on-read helpers before checking existence (`commands.rs:6177-6179`, `sessions/mod.rs:403-447`; the same architectural issue remains in main's expanded inventory).
- `find_session` uses the lenient session-index loader; resolving its path creates the root, and a malformed `sessions.json` is copied to a `*.corrupt-*` backup even for read-only callers (`sessions/mod.rs:102-168`, `:376-379`).

Therefore “strict decoder did not truncate” is not enough to claim a non-mutating read. The Act slice needs resolve-only root/path/inventory/index APIs, or a read-only artifact locator whose construction never creates directories or quarantines the session index. A required regression should start with a nonexistent data root, invoke every missing-stream Review/replay/export reader, and prove the root is still nonexistent afterward. A second fixture should provide a malformed index and prove strict stream reads do not create an index backup.

## Transcript/session consumer call graph

### Standalone transcript load

`load_session_transcript` is registered as IPC (`src-tauri/src/lib.rs:578-581`). The frontend store exposes `loadSessionTranscript` (`src/store/index.ts:2697-2726`), but no non-test production component calls that action in this tree. It remains a public command and must route through the same transcript snapshot helper rather than a separate parser. Main additionally validates that the session owns at least one artifact before returning (`E:/CS/github/audio-graph/src-tauri/src/commands.rs:6594-6608`).

### Historical Review load

`SessionsBrowser.handleLoad` calls store `loadSession`, selects the transcript panel, and closes only on success (`src/components/SessionsBrowser.tsx:195-199`; main improves the success guard at `E:/CS/github/audio-graph/src/components/SessionsBrowser.tsx:198-203`). The store invokes `load_session`, installs the returned transcript/event/projection payloads, records `loadedSessionId`, and starts a separate timeline fold (`src/store/index.ts:2728-2767`). `LiveTranscript` renders the segment view, joins persisted diarization, and uses `sessionTranscriptEvents` for revision badges (`src/components/LiveTranscript.tsx:47-67`, `:153-167`).

On the backend, main's read-only `load_session_impl` loads the segment view, transcript events, diarization, projection events/caches, live assist, and legacy graph, then validates transcript replay without installing it into live runtime state (`E:/CS/github/audio-graph/src-tauri/src/commands.rs:6665-6761`). The transcript stream must be read once and shared between view derivation, projection-history replay, validation, and the returned DTO.

### Timeline

After a successful Review load, `loadSessionTimeline` invokes `build_session_timeline_cmd`; it has a stale-session guard and renders an explicit empty/error state without blanking the transcript (`src/store/index.ts:1670-1700`, `:2760-2763`). The backend loads transcript and diarization streams, replays `TranscriptLedger`/`SpeakerTimeline`, loads the legacy live graph, and folds `TimelineEntry` rows (`commands.rs:6386-6456`; main equivalent `E:/CS/github/audio-graph/src-tauri/src/commands.rs:6841-6911`). The fold uses latest transcript spans, trusted latest-wins speakers, and graph edge provenance (`timeline.rs:1-40`, `:104-185`).

### Projection replay diagnostics

`ProjectionRuntimeStatusPanel` invokes `get_projection_replay_report_cmd` for the active runtime session (`src/components/ProjectionRuntimeStatusPanel.tsx:657-672`). The backend loads transcript events as the basis history before projection patches/caches (`commands.rs:5918-5957`). This is a read-only diagnostic but can target a live writer; once framed writers exist it needs the actor/shared-snapshot coordination owned by Seed `audio-graph-1f71`. The reader wave must not add destructive recovery to make this path “work.”

### Export

`SessionsBrowser` invokes `export_session_bundle` and downloads the typed JSON object (`src/components/SessionsBrowser.tsx:229-239`, store `src/store/index.ts:2845-2858`). The backend v1 bundle includes legacy/derived transcript segments and raw transcript revision rows, plus sibling artifacts (`commands.rs:6308-6384`; main `E:/CS/github/audio-graph/src-tauri/src/commands.rs:6764-6839`). The same transcript stream snapshot must drive both `transcript` and `transcript_events`; a second read can otherwise produce a self-inconsistent export. Canonical corruption must abort export, never fall back or omit the bad stream.

## Smallest safe transcript adapter integration

1. Add one shared reader module, rather than parser branches in commands. Give it versioned stream descriptors (`stream_id`, `domain_schema_version`) and a `CanonicalStreamRead<T>` result retaining presence, records, and head. It must always call `CanonicalTailRecovery::Strict`.
2. Add resolve-only path construction. Do not call `data_root()`, `transcripts_dir()`, repository `ensure_dir`, or a create-on-read artifact inventory from this reader.
3. Add a transcript-specific snapshot helper that:
   - loads `TranscriptEvent` rows through the shared adapter;
   - if present, replays once and derives `TranscriptSegment`s;
   - if absent, strictly loads the indexed legacy `TranscriptSegment` path (blank lines compatible, malformed rows fatal);
   - returns segments plus the exact event-stream snapshot for reuse.
4. Route main's read-only `load_session`, standalone transcript load, projection replay report, timeline, and bundle export through the repository/shared adapter. `load_session` and export should not re-read transcript events after deriving the display view.
5. Keep command DTOs unchanged in this slice if needed, but retain heads/presence internally so the typed manifest work can consume them. Do not construct `CanonicalAppender`, enable a framed writer, quarantine a tail, or change snapshot authority beyond applying the already-required presence rule.

The stream ids and domain-schema constants must be durable shared constants used later by writers. No accepted ADR currently fixes their exact spellings. Suggested values are `transcript_events` and domain schema `1`, but the conductor should record the chosen cross-stream registry before fixtures hard-code hashes. Do not reuse `TranscriptLedger::SCHEMA_VERSION` implicitly: that constant versions the materialized ledger aggregate, while canonical `domain_schema_version` versions persisted `TranscriptEvent` payloads.

Do not widen `LocalMemoryRepository::load_transcript_events -> Vec<TranscriptEvent>` in this slice unless every adapter is intentionally migrated. `SurrealMemoryRepository` implements that trait with repository rows (`persistence/surreal.rs:430-450`) and has no file-presence/head meaning. A File-specific strict-read extension can retain canonical metadata while the legacy trait remains for noncanonical/derived adapters; the conductor can migrate the repository contract later with an explicit head model.

## Error, schema, and privacy details

- Map canonical failures with the kernel's content-redacted `Display`, not raw file bytes or payload `Debug` (`canonical_log.rs:163-215`). Do not include a raw user path in the frontend error.
- `TranscriptSegment` derives `Debug` and exposes `text` (`state.rs:41-52`). A wrapper containing `CanonicalRecord<TranscriptSegment>` must not derive/log content-bearing `Debug`; errors and telemetry should carry only stream kind, record index, corruption reason, and presence/head metadata.
- The framed envelope and kernel test payload deny unknown fields, but production `TranscriptEvent` does not (`canonical_log.rs:307-317`, test type `:1961-1968`; `projections.rs:34-69`). Consequently an unknown payload member under declared domain schema v1 is currently ignored during typed decode. Before freezing domain-v1 fixtures, decide whether additive unknown fields are valid forward compatibility or a schema violation. If strict v1 requires rejection, use versioned persisted DTOs with `deny_unknown_fields`; do not casually annotate heavily edited main domain types.
- A later domain payload version must dispatch to a different decoder. Asking the v1 reader to reinterpret a v2 envelope correctly fails `DomainSchemaVersionMismatch`; no fallback is allowed.

## Required transcript fixture matrix

| Fixture | Expected result |
|---|---|
| Missing event file; valid indexed legacy transcript | `Missing` event stream; strict legacy rows returned unchanged; neither file nor directories created/rewritten. |
| Existing zero-byte or blank-only event file plus non-empty legacy transcript | `Present(empty)`; transcript is explicitly empty; legacy rows are not promoted. |
| Legacy-only event JSONL | Present rows in append order, `LegacyJsonl` encodings, deterministic synthetic head; source bytes unchanged. |
| Legacy prefix followed by one or more valid AGCL1 frames | All payloads in order; framed head returned; derived segment view collapses revisions exactly once. |
| Framed-only stream | Payloads/head returned and replayed. |
| Valid final legacy row without newline; CRLF rows; whitespace-only rows | Preserve current compatibility: final legacy row accepted, CRLF accepted, blank rows ignored and do not consume sequence. |
| Malformed newline-terminated legacy row (middle or tail) | `InvalidJson`; fail closed; no “skip malformed line”; no legacy-display fallback. |
| Valid framed row followed by legacy JSON | `LegacyRecordAfterFramedRecord`; fail closed. |
| Unterminated/truncated framed tail in Strict mode | `MissingFrameTerminator`/`InvalidFrame`; file bytes, length, and mtime unchanged; no quarantine receipt/file. |
| AGCL2/future magic; wrong envelope format version | `UnsupportedFrameVersion` / `EnvelopeVersionMismatch`; no fallback. |
| Wrong session, stream, or domain schema | Matching context error; no fallback. |
| Bad frame length, sequence, previous hash, payload hash, record hash, duplicate event id | Matching structured corruption; no partial rows returned. |
| Payload missing/wrong required field | `PayloadDecode` at the exact record index. Pin the chosen unknown-field policy separately. |
| Corrupt event stream plus a valid legacy transcript | Corruption returned; legacy content never leaks into Review/export. |
| Same mixed fixture through standalone load, `load_session`, timeline, replay report, and export | Every consumer sees the same append order; Review `transcript` equals derivation of returned `transcript_events`; export is internally self-consistent. |
| Nonexistent data root; malformed `sessions.json` | All strict read surfaces leave the tree unchanged: no mkdir, index backup, tail quarantine, temp file, or write. |

For every error fixture, snapshot the complete test tree (relative paths + file SHA-256 + lengths) before/after. Checking only the target log misses the current create-on-read/index-backup mutations.

## Gates

1. Focused Rust tests for the new reader module and transcript snapshot helper, serialized where environment-root guards are used.
2. Existing canonical kernel suite (`persistence::canonical_log::tests`) remains 23/23 green under the locked cloud feature gate.
3. Main-only command regressions remain green: existing-empty canonical projection authority (`commands.rs:10991-11030`), isolated historical payloads (`:11428-11462`), timeline reload speaker retcon, projection replay report, full bundle export, and missing-session rejection.
4. Existing transcript ledger/derivation and legacy-fallback tests (`persistence/mod.rs:5231-5398`, `projections.rs:816-848`), updated so file presence—not row count—selects authority.
5. Frontend main regressions remain green even if no TS source changes: `src/store/index.test.ts` historical-live lock, stale-response ordering, capture-start invalidation (`E:/CS/github/audio-graph/src/store/index.test.ts:850-982`), plus `SessionsBrowser` success/locked behavior.
6. `cargo +1.95.0 fmt --all -- --check`, locked metadata, strict library Clippy `-D warnings`, `git diff --check`, and the main-tree `bun run verify:fast` after integration.
7. Static no-writer gate: no new production `CanonicalAppender` reference outside the kernel; no `QuarantineUnterminatedTail` at a Review/replay/export call site.
8. Multi-OS CI eventually runs the strict fixtures on Windows, macOS, and Linux. This local wave must not claim cross-platform proof from Windows alone.

## Unknowns that would change the plan

1. **Domain registry:** exact stream ids and domain schema constants are not fixed. Golden mixed frames must wait for one shared registry decision.
2. **Payload unknown fields:** reject under v1 (versioned strict DTOs) versus ignore for forward compatibility. The former adds payload-type work; the latter must document that typed export omits unknown members while disk/head integrity remains intact.
3. **Empty transcript authority:** this report recommends present-empty event logs override legacy rows, matching file-canonical/projection authority. If product compatibility requires fallback, it needs an explicit pre-canonical generation marker; row emptiness alone is unsafe.
4. **Non-mutating scope:** if Seed 6896 means only “canonical target log bytes are unchanged,” resolve-only index/inventory work could split out. If it means the user-observable read command is non-mutating—as the wave stop condition reads—create-on-read directories and malformed-index backup are blockers in this slice.
5. **Live/current reads:** projection diagnostics and export can read a live session. No canonical writer is enabled in this wave, so the adapter can land, but framed writer adoption remains blocked until Seed `audio-graph-1f71` supplies an actor snapshot/shared-lock protocol.
6. **Indexed legacy paths:** historical metadata may name a non-default transcript path (`sessions/mod.rs:382-393`). Preserve that fallback unless a separate migration validates and rewrites it.

## Bounded file-ownership proposal

The smallest low-conflict split is:

- **Shared reader owner:** new `src-tauri/src/persistence/canonical_reader.rs` (typed strict adapter, stream registry, presence/head type, fixture matrix) plus only the module/export and narrow loader hooks in `persistence/mod.rs`. This owner integrates main's movement lock/writer-preopen changes rather than replacing the file.
- **Read-only locator owner:** `user_data.rs` and `sessions/mod.rs` resolve-only root/path/index/inventory APIs and whole-tree non-mutation tests. This is a distinct commit/Seed child if the conductor chooses the narrow log-bytes interpretation; otherwise it is a prerequisite commit in 6896. Start from main because its session inventory/deletion changes are newer.
- **Transcript/session owner:** main-first `commands.rs` changes limited to one-read transcript snapshot reuse across standalone load, read-only `load_session`, replay report, timeline, and export, with command regressions. Do not touch `state.rs` or revert main's no-`AppState` Review command.
- **Conductor-owned integration seam:** reconcile projection/diarization/movement lane routing into the shared adapter after their reports arrive. Do not let parallel agents edit `persistence/mod.rs` or `commands.rs` concurrently.

No frontend production change is required for the decoder itself. Treat main `src/store/index.ts`, `src/components/SessionsBrowser.tsx`, `src/types/index.ts`, and their tests as preservation gates, not files to copy from this older worktree.

## Final verdict

The transcript payloads fit the canonical kernel, and a strict reader-only rollout is viable, but Act should not begin by swapping `load_jsonl` blindly. First preserve the main-only Review isolation/authority slice, add a typed `Missing | Present(snapshot)` file reader and resolve-only locator, then route each consumer through one transcript snapshot. The two blockers are loss of existing-empty authority and filesystem mutation during nominally strict reads. Runtime appender adoption remains out of scope.
