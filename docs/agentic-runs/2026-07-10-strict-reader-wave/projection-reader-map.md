# Projection and Diarization Strict-Reader Map

Date: 2026-07-10

Seed: `audio-graph-6896`
Lane: projection/diarization read-only discovery

## Scope and verdict

This report maps how projection patches, materialized notes/graph snapshots, and
speaker revisions are read today; distinguishes authoritative files from
fallback caches; and defines the smallest strict mixed-format reader slice.

**Yes: the existing `canonical_log` typed reader can serve both existing
`ProjectionPatch` and `DiarizationSpanRevision` rows without changing
canonical-log v1.** Both payloads already implement `Deserialize`; legacy JSONL
rows are payload-shaped exactly as the typed reader expects, while framed rows
place that same JSON value under the v1 envelope. Use domain schema version `1`
for both streams, distinct stable stream ids, and `CanonicalTailRecovery::Strict`.

That verdict is intentionally narrower than "the whole read path is sound."
The repository adapter must preserve missing-versus-present-empty state, map
only `NotFound` to an absent stream, and propagate every integrity, context,
version, and payload-decode error. Projection materialization must remain a
replay from the event log; notes/graph JSON stays a derived cache. A separate
semantic gap remains: historical projection replay does not consume the
speaker timeline, so a persisted patch with a non-empty diarization basis is
currently rejected as `DiarizationBasisUnavailable`. Current runtime-generated
patches do not cite speaker bases, so this does not block the reader-only wave,
but speaker-aware projection must not ship until that replay path is extended.

The current checkout used for this lane is the canonical kernel branch based on
`f97e19c…`; the dirty main checkout contains newer, uncommitted product
hardening. References below use `K:` for this worktree and `M:` for
`E:/CS/github/audio-graph`. Main-only changes listed in the integration section
are mandatory preservation requirements, not optional suggestions.

## Evidence inventory

- ADR-0027 declares speaker revisions and accepted notes/graph patches canonical,
  requires session/stream/schema/sequence/integrity metadata, makes snapshots
  subordinate to canonical logs, and requires historical Review to stay
  read-only (`K:docs/adr/0027-file-canonical-durable-session-store.md:51-70,
  86-94`).
- File layout is already stable: projection patches are
  `projections/<session>.events.jsonl`, speaker revisions are
  `transcripts/<session>.speaker.jsonl`, notes are `notes/<session>.json`, and
  the projected graph is `graphs/<session>.materialized.json`
  (`K:src-tauri/src/persistence/mod.rs:816-862, 940-967`).
- The repository contract exposes projection/speaker append and load methods,
  materialized notes/graph snapshots, transcript/speaker replay, and projection
  replay (`K:src-tauri/src/persistence/mod.rs:509-539, 578-596, 647-699`).
- The file adapter still routes both event types through plain `append_jsonl` /
  `load_jsonl`; notes and graph use atomic single-JSON snapshots
  (`K:src-tauri/src/persistence/mod.rs:1179-1292`).
- `load_jsonl` maps a missing path to an empty vector, ignores blank rows, and
  fails on the first malformed typed row. It has no frame/context/version/hash
  checks (`K:src-tauri/src/persistence/mod.rs:2605-2637`).
- `canonical_log` accepts a deterministic legacy JSONL prefix followed by v1
  frames and rejects legacy rows after the first frame
  (`K:src-tauri/src/persistence/canonical_log.rs:1-23, 1426-1550`). Its typed
  strict loader verifies structure before payload decode and does not mutate
  unless the caller explicitly selects tail quarantine
  (`K:src-tauri/src/persistence/canonical_log.rs:540-603, 1347-1394`).
- Framed v1 validates format/envelope version, session, stream, domain schema,
  sequence, previous hash, payload hash, record hash, event ids, causal ids,
  basis heads, and duplicate event ids. Unknown `AGCL*` magic reports
  `UnsupportedFrameVersion` (`K:src-tauri/src/persistence/canonical_log.rs:140-183,
  1485-1508, 1535-1549, 1605-1700`).
- Surreal's feature-gated adapter has its own projection table reader but does
  not implement durable diarization methods, so the trait defaults fail loudly
  for speaker storage (`K:src-tauri/src/persistence/surreal.rs:33-46, 437-453`;
  `K:src-tauri/src/persistence/mod.rs:516-539`). The strict file reader must
  therefore be file-adapter-specific rather than silently changing database
  semantics.

## Current projection read path

1. `FileMemoryRepository::load_projection_patches` resolves the session path
   and calls plain `load_jsonl<ProjectionPatch>`
   (`K:src-tauri/src/persistence/mod.rs:1191-1195, 2680-2685`). The returned
   vector preserves append order but contains no transport metadata or head.
2. Repository replay loads transcript events and projection patches, then calls
   `replay_accepted_patches_with_transcript_history`
   (`K:src-tauri/src/persistence/mod.rs:673-684`). The replay sorts transcript
   events by receive/media time, advances a ledger through each patch timestamp,
   validates the patch basis, skips historically invalid bases with a report,
   and applies valid patches to the notes or graph materializer
   (`K:src-tauri/src/projections.rs:1970-2024`).
3. Materialization is deterministic and typed: notes/graph sequences must
   advance, kinds cannot cross, and invalid operations fail
   (`K:src-tauri/src/projections.rs:1207-1288, 1427-1469, 1926-1967`). The event
   log, not either JSON snapshot, is therefore the reconstructable source.
4. In the kernel-base command path, `load_session` replays only when the loaded
   projection vector is non-empty. If it is missing **or present but empty**, it
   selects materialized cache JSON instead. Replay errors are logged and the
   cache may still surface (`K:src-tauri/src/commands.rs:6134-6163,
   6168-6239`). This conflates absence with an explicit empty canonical state.
5. The dirty main checkout already fixes that authority error. It checks event
   **file existence**, replays even an empty file, returns an error for any
   invalid historical patch or replay failure, and never falls back to derived
   caches while canonical authority exists (`M:src-tauri/src/commands.rs:6633-6652,
   6666-6740`). Its regression proves a present empty canonical projection
   stream suppresses orphan caches (`M:src-tauri/src/commands.rs:10991-11030,
   12036-12088`). This main-only behavior must be ported/preserved before the
   strict reader is integrated.
6. Dirty main also changed Review to a read-only load: it no longer installs a
   historical transcript ledger, materialized projection state, graph, or
   scheduler into active `AppState` (`M:src-tauri/src/commands.rs:6655-6666,
   6741-6761`). Preserve this exactly; a strict reader must not regress active
   capture isolation.

### Projection fallback and error matrix

| Condition | Required result |
|---|---|
| Projection path absent | Pre-canonical session: return no canonical rows and allow the legacy materialized-cache compatibility path. |
| Projection path present, zero bytes or only ignored blank legacy rows | Canonical authority is explicitly empty; materialize empty notes/graph and ignore any cache, even an ahead cache. |
| Valid legacy-only JSONL | Decode every row as `ProjectionPatch`, replay normally, no file mutation. |
| Valid legacy prefix + framed-v1 suffix | Decode in one ordered vector; expose encoding/head for diagnostics and future manifest use. |
| Payload shape does not decode | Return `PayloadDecode { record_index }`; do not consult cache. |
| Bad frame/context/version/hash/sequence/interior JSON | Return the exact redacted canonical error; do not skip, repair, or consult cache. |
| Unterminated tail in this wave | Strict error and byte-for-byte preservation. Tail repair belongs to coordinated recovery, not Review/export. |
| Structurally valid rows but invalid materializer operation/basis | Fail or report at semantic replay; transport validity must not be mistaken for semantic acceptance. Dirty-main Review currently fails closed when its validation report is non-empty and must keep doing so. |

Materialized notes and graph are not append logs and must **not** be passed to
`load_canonical_stream`. They remain `load_json` caches. A future cache envelope
must carry the canonical projection head and input basis-head vector; that is a
manifest/snapshot wave, not a canonical-log v1 change.

## Current diarization read path

1. `DiarizationSpanRevision` is the persisted payload. It carries a stable
   provider-neutral `span_id`, provider/timeline/source provenance, resolved and
   provider speaker ids, time bounds, stability/finality, monotonically revised
   identity, supersedes/basis references, and optional latency fields
   (`K:src-tauri/src/projections.rs:273-324`). Optional latency fields already
   have serde defaults, preserving old-row compatibility.
2. The live speech path emits the frontend event first, applies the revision to
   the in-memory timeline/graph, and only afterward performs a synchronous,
   best-effort JSONL append. Append failure is logged and swallowed
   (`K:src-tauri/src/speech/mod.rs:382-467`). Thus the current writer is not an
   ADR-0027 Accepted boundary; a strict reader can validate what survived but
   cannot recover accepted-only-in-memory revisions.
3. The file reader maps both missing and present-empty speaker logs to an empty
   vector (`K:src-tauri/src/persistence/mod.rs:1210-1217, 2688-2697`). There is no
   alternate speaker snapshot today, so output is the same, but the strict
   adapter should still retain presence for manifest/head and future fallback
   decisions rather than discarding it.
4. `SpeakerTimeline::replay` applies rows in order. Later revisions replace the
   same provider-neutral span, an older revision fails as
   `StaleDiarizationRevision`, and a disagreeing same-revision row fails as
   `ConflictingDiarizationRevision`; the latest spans are deterministically
   media-time sorted (`K:src-tauri/src/projections.rs:418-499, 599-627`).
5. `load_session` returns the full append-ordered speaker event vector so the UI
   can hydrate latest-wins speaker attribution; `session_timeline` independently
   replays the rows backend-side before joining transcript spans and the legacy
   temporal graph (`K:src-tauri/src/commands.rs:6196-6205, 6296-6305,
   6421-6444`). Dirty-main tests preserve both full-log hydration and the empty
   old-session case (`M:src-tauri/src/commands.rs:11110-11195`).
6. A projection basis may cite diarization span revisions, and live validation
   can validate them when handed a `SpeakerTimeline`
   (`K:src-tauri/src/projections.rs:530-596, 700-725, 2035-2054`). Historical
   projection replay, however, calls transcript-only `validate_basis`; any
   non-empty speaker basis is therefore recorded as
   `DiarizationBasisUnavailable` and skipped
   (`K:src-tauri/src/projections.rs:1970-2021, 700-723`). This is the key semantic
   projection/diarization join gap.

The reader-only wave should decode and expose speaker rows and prove timeline
replay parity, but it should not alter live append ordering or claim durable
acceptance. Before speaker-bearing projection bases are enabled, add a separate
historical replay API that interleaves transcript **and diarization** revisions
through each patch timestamp and validates with
`validate_basis_with_speaker_timeline`.

## Canonical-log v1 compatibility

Recommended stable domain identifiers:

| Artifact | `stream_id` | `domain_schema_version` | Typed payload |
|---|---:|---:|---|
| Accepted notes/graph projection patches | `projection_events` | `1` | `ProjectionPatch` |
| Speaker span revisions | `diarization_events` | `1` | `DiarizationSpanRevision` |

The exact strings must be centralized once and treated as durable data. Do not
derive them from paths, Rust type names, or display labels.

Compatibility reasoning:

- Legacy rows are parsed as raw JSON values, recursively key-canonicalized for
  deterministic synthetic commitments, then decoded as `T`
  (`K:src-tauri/src/persistence/canonical_log.rs:1551-1602,
  1347-1370`). That is exactly the current row shape for both payloads.
- A v1 frame stores the same payload under the envelope. The reader verifies the
  caller-supplied session/stream/domain version before typed decode
  (`K:src-tauri/src/persistence/canonical_log.rs:1632-1700`). No payload wrapper
  or canonical-log format change is required.
- `ProjectionPatch` retains old-row compatibility through serde defaults on its
  three optional latency fields (`K:src-tauri/src/projections.rs:944-960`).
  `ProjectionBasis.summarized_through_revision` and the diarization latency
  fields also default when absent (`K:src-tauri/src/projections.rs:206-216,
  319-323`).
- The payload structs do not use `deny_unknown_fields`. Extra object fields are
  therefore tolerated by serde, while unknown enum variants and missing
  required fields fail `PayloadDecode`. That is acceptable for v1 compatibility
  but is not a substitute for explicit future domain-version dispatch.
- A framed domain version other than `1` must fail closed today as
  `DomainSchemaVersionMismatch`. Future v2 needs an explicit dispatcher or
  migration; weakening v1 to guess the payload schema would make rollback and
  integrity behavior ambiguous.
- Legacy rows contain no session/stream/domain envelope, so their context is
  necessarily supplied by the containing path/caller. The reader generates a
  context-bound synthetic identity, but it cannot prove that a copied legacy row
  originally belonged to that session. This is a legacy limitation for the
  typed manifest/migration wave, not a reason to alter v1 framing.

Use `CanonicalTailRecovery::Strict` everywhere in Review, replay, timeline, and
export. `QuarantineUnterminatedTail` mutates and explicitly requires writer
quiescence (`K:src-tauri/src/persistence/canonical_log.rs:540-543`); it belongs
only in a later recovery transaction with typed manifest inventory.

## Required fixtures and gates

### Checked-in byte fixtures

Create fixtures under `src-tauri/fixtures/canonical_streams/`; keep frame bytes
checked in so serializer or Cargo-feature drift cannot silently rewrite the
contract.

| Fixture | Exact assertion |
|---|---|
| `projection/legacy-only-v1.jsonl` | Two old-format patches (notes then graph, optional latency fields omitted) decode in order as `LegacyJsonl` and replay to expected notes/graph state. |
| `projection/legacy-prefix-framed-v1.log` | One legacy patch followed by one newline-committed v1 frame decodes with encodings `[LegacyJsonl, FramedV1]`, expected head/record hashes, and unchanged payloads. |
| `projection/present-empty.log` | Existing zero-byte path returns `PresentEmpty`; an orphan notes/graph cache is ignored by Review. |
| `projection/speaker-basis-legacy.jsonl` | A patch with non-empty `diarization_span_revisions` decodes without a v1 change; a focused test records the current `DiarizationBasisUnavailable` replay limitation until the speaker-aware replay Seed closes it. |
| `projection/payload-shape-invalid.log` | Structurally valid frame whose payload lacks a required patch field returns `PayloadDecode` at the exact record index. |
| `diarization/legacy-relabel-v1.jsonl` | Provisional rev1 plus stable rev2 for one span decode in append order and replay to exactly one latest span at rev2. |
| `diarization/legacy-prefix-framed-v1.log` | Legacy provisional row plus framed stable row decodes as mixed format and replays to the same timeline as the legacy-only equivalent. |
| `diarization/present-empty.log` | Existing empty file returns `PresentEmpty` and an empty timeline, distinct in metadata from `Missing`. |
| `diarization/optional-fields-old-row.jsonl` | Old row without latency fields decodes with `None`; a current framed row with latency fields preserves them. |
| `diarization/stale-revision.jsonl` | Transport decode succeeds; `SpeakerTimeline::replay` returns `StaleDiarizationRevision`. |
| `diarization/conflicting-revision.jsonl` | Transport decode succeeds; replay returns `ConflictingDiarizationRevision`. |

Use shared generated-corruption table tests over a valid projection and a valid
speaker frame for: unsupported `AGCL2` magic, envelope-version mismatch, wrong
session, wrong stream, wrong domain schema, sequence gap, previous-hash mismatch,
payload-hash mismatch, record-hash mismatch, duplicate event id, legacy after
framed, malformed interior JSON, frame length mismatch, and unterminated tail.
Each test must hash/read the file before and after and assert byte identity under
strict mode. Also prove that only `NotFound` maps to `Missing`; permission/read
errors propagate.

### Focused commands

From `src-tauri` with the pinned toolchain and serialized Windows build settings:

```powershell
$env:CARGO_BUILD_JOBS = '1'
$env:CARGO_INCREMENTAL = '0'
$env:AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST = '1'
$env:RUSTFLAGS = '-C linker=lld-link'

cargo +1.95.0 fmt --all -- --check
cargo +1.95.0 test --locked --lib --no-default-features --features cloud `
  persistence::canonical_log::tests -- --test-threads=1 --nocapture
cargo +1.95.0 test --locked --lib --no-default-features --features cloud `
  canonical_projection_reader_ -- --test-threads=1 --nocapture
cargo +1.95.0 test --locked --lib --no-default-features --features cloud `
  canonical_diarization_reader_ -- --test-threads=1 --nocapture
cargo +1.95.0 test --locked --lib --no-default-features --features cloud `
  load_session_ -- --test-threads=1 --nocapture
cargo +1.95.0 test --locked --lib --no-default-features --features cloud `
  export_session_bundle_ -- --test-threads=1 --nocapture
cargo +1.95.0 test --locked --lib --no-default-features --features cloud `
  session_timeline_ -- --test-threads=1 --nocapture
cargo +1.95.0 test --locked --lib --no-default-features --features cloud `
  speaker_timeline_ -- --test-threads=1 --nocapture
cargo +1.95.0 clippy --locked --lib --no-default-features --features cloud -- -D warnings
```

Then from the repository root run `bun run verify:fast` and `git diff --check`.
Add a source gate proving no production `CanonicalAppender` construction was
introduced in this wave. Run the byte-fixture reader matrix in Windows, macOS,
and Linux CI; it needs no audio hardware.

## Bounded implementation ownership

### Recommended owned slice

1. Add `src-tauri/src/persistence/canonical_streams.rs` as the narrow adapter.
   It should own the two stream specs, a generic strict file load, and a result
   that preserves `Missing` / `PresentEmpty` / `PresentRecords`, ordered typed
   records, stream head, and encoding metadata. It maps only `ErrorKind::NotFound`
   to `Missing`; all other `CanonicalLogError`s pass through in redacted form.
2. Touch `src-tauri/src/persistence/mod.rs` only at module wiring and the four
   file-adapter projection/diarization load seams (explicit-root and user-data
   free-function paths). Keep the public `Vec<T>` trait methods for compatibility;
   use a file-specific richer read where presence/head is needed. Do not route
   Surreal rows through file framing.
3. Add only the fixture files and focused tests described above. Existing
   Review, replay report, export, and timeline commands already call the
   repository boundary, so they inherit the decoder without per-command parsing.
4. If the integration branch does not yet contain dirty-main Review hardening,
   port the **specific** `commands.rs` hunks for read-only history, event-file
   existence authority, fail-closed replay, and cache suppression before
   declaring the slice complete. Do not replace `commands.rs` wholesale.
5. Do not change `canonical_log.rs`, `ProjectionPatch`,
   `DiarizationSpanRevision`, materialized snapshot schemas, any writer, speech
   dispatch, scheduler, or runtime appender ownership in this reader-only slice.

### Main-only overlap that must survive integration

The main checkout differs substantially from this worktree in every large
runtime file. Preserve at least these directly overlapping changes:

- `commands.rs`: canonical transcript preference; read-only `load_session`;
  projection-log **path existence** authority; fail-closed invalid replay; no
  derived-cache fallback; regression tests for explicit empty projection state
  and active-state isolation (`M:6563-6579, 6633-6761, 10955-11030,
  11203-11268, 12036-12088`).
- `projections.rs`: final/end-of-turn-only automatic projection bases and the
  shared Current/AppendOnly/Revised basis classifier with covered-subset hash and
  order checks (`M:703-910`). These alter replay/scheduler semantics around the
  same `ProjectionPatch` payload and cannot be overwritten by the older kernel
  copy.
- `state.rs`: live projection sequence advances after event **enqueue** even when
  derived snapshot save fails, plus `materialized_snapshot_saved` reporting
  (`M:src-tauri/src/state.rs:356-360, 482-635, 1539-1613`). Preserve the
  cache-as-derived behavior, but do not inherit the code comment's premature
  use of "accepted": the current writer returns enqueue success, not an ADR-0027
  durable receipt, so Pending-to-Accepted remains a later writer wave.
- `persistence/mod.rs`: process serialization for movement appends and writer
  target preflight before thread spawn (`M:233-240, 1797-1841, 2037,
  2283`). These are adjacent conflict zones in the same large module.
- `sessions/mod.rs`: complete artifact inventory (including JSON temp files,
  speaker/projection logs, scheduler/movement files) and containment-safe,
  artifacts-first deletion. Reader edits must not revert this inventory.
- Main currently has no `persistence/canonical_log.rs`; the kernel module is a
  branch-only addition. Integration therefore requires a three-way, semantic
  merge of kernel + dirty-main hardening, not copying either tree over the other.

### Explicit non-goals / follow-up

- No framed runtime writer, no appender construction, no repair/quarantine, and
  no Pending-to-Accepted claim.
- No snapshot-head or manifest schema; keep that under the typed manifest wave.
- No speaker-aware historical projection replay in this slice. Record a focused
  P0 follow-up before any runtime projection cites speaker bases: interleave
  transcript and speaker histories by event time, validate every patch with both
  ledgers, and prove current/append-only/revised speaker retcons across reload.
- No change to Surreal's authority. File logs remain canonical; Surreal stays a
  feature-gated repository/conformance target and its missing diarization parity
  remains explicit rather than being treated as an empty timeline.
