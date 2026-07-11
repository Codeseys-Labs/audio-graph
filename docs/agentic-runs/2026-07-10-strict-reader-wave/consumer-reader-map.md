# Seed 6896 consumer and reader map

Date: 2026-07-10

Seed: `audio-graph-6896`

Scope: read-only discovery of data-movement, export, session timeline,
artifact-inventory, delete/purge, and command consumers. This report does not
authorize a runtime canonical writer or any reader-side repair.

## Snapshot and contract

- Worktree: `E:/CS/github/audio-graph-wt-6896-consumers`
- Branch: `codex/6896-consumer-map`
- Worktree HEAD: `104e9642b2db11ecec95cf26b4f8f1bb42268fd0`
- Integrated canonical kernel: `7b0e5d003dcc23c971561c85fe5a5a57dc6920ed`
- Reader rule: a canonical stream may be legacy-only or a valid legacy prefix
  followed by framed-v1 records. Once a framed record is seen, any malformed,
  unsupported, truncated, hash-invalid, or legacy-after-framed record is an
  error. Strict reads must neither quarantine nor truncate the source.
- Authority rule: a present, valid canonical event stream may legitimately
  decode to zero current items. Consumers must not equate that state with a
  missing canonical stream and silently fall back to a legacy derivative.

## Initial consumer inventory

The mapped consumer families are:

1. repository loaders for transcript, speaker/diarization, projection, and
   movement streams;
2. Tauri Review/session commands and projection-report commands;
3. session metadata recovery/statistics readers;
4. timeline assembly and graph-linkage consumers;
5. export and artifact-inventory enumeration;
6. soft-delete, permanent-delete, expiry-purge, and repository cleanup.

Detailed evidence and integration seams follow as each family is traced.

## Evidence notation and dirty-integration warning

- `C:` means the clean 6896 consumer-map worktree at `104e964`.
- `I:` means the user's broadly dirty integration checkout at
  `E:/CS/github/audio-graph`. Its relevant working-tree files are newer product
  slices that are not commits on `C:` and must not be overwritten.

The clean and integration copies differ for `persistence/mod.rs`,
`commands.rs`, `sessions/mod.rs`, `lib.rs`, the Zustand store, the session
browser, the data-route panel, and shared frontend types. `timeline.rs`,
`projection_data_movement.rs`, and `user_data.rs` are byte-identical. Any Act
patch built only against `C:` would regress accepted Review isolation,
canonical-empty projection authority, active-session delete guards,
retry-safe artifact deletion, expanded production artifact inventory, and
closed-world privacy copy already present in `I:`.

## Shared strict-reader seam

The kernel already supplies the only parser this wave should use:

- `load_canonical_stream` validates the stream context then reads and parses
  the whole file (`C:src-tauri/src/persistence/canonical_log.rs:540-575`).
- Passing `CanonicalTailRecovery::Strict` makes every structural failure return
  an error; the only mutation path is guarded by the distinct
  `QuarantineUnterminatedTail` branch
  (`C:src-tauri/src/persistence/canonical_log.rs:576-603`).
- The parser accepts whitespace rows in a legacy prefix, assigns only
  non-whitespace rows a sequence, and moves into framed mode permanently after
  the first v1 frame (`C:src-tauri/src/persistence/canonical_log.rs:1426-1519`).
- A legacy row after a frame is rejected
  (`C:src-tauri/src/persistence/canonical_log.rs:1523-1550`). Framed records
  validate declared length, envelope/session/stream/domain version, sequence,
  previous hash, payload hash, record hash, and duplicate-member-free JSON
  (`C:src-tauri/src/persistence/canonical_log.rs:1605-1685`).
- Typed decoding is performed only after structural validation and returns the
  payloads inside `CanonicalRecord<T>` plus the verified stream head
  (`C:src-tauri/src/persistence/canonical_log.rs:1347-1381`).

Add one file-backed adapter adjacent to `load_jsonl` in
`src-tauri/src/persistence/mod.rs`; do not teach generic `load_jsonl` to guess a
stream identity. The adapter should return a presence-bearing value, for
example:

```rust
enum CanonicalStreamPresence {
    Missing,
    Present,
}

struct StrictCanonicalStream<T> {
    presence: CanonicalStreamPresence,
    rows: Vec<T>,
    head: Option<CanonicalStreamHead>,
}
```

It should call `load_canonical_stream(..., CanonicalTailRecovery::Strict)` and
map only `Io { operation: Read, kind: NotFound }` to `Missing`. Permission,
sharing, and other I/O failures remain errors. This avoids a separate
`path.exists()`/open race, keeps a zero-byte or whitespace-only existing file
as `Present` with no rows, and exposes the verified head needed by the later
typed manifest without changing writer behavior.

The four file stream descriptors must be centralized rather than repeated at
call sites. No production constants currently freeze their exact `stream_id`
or outer `domain_schema_version`; choosing those strings is a durable wire
decision because framed readers reject mismatches
(`C:src-tauri/src/persistence/canonical_log.rs:1636-1649`). A bounded mapping
consistent with the current artifact domains is:

| Payload | Path | Proposed stream id | Outer schema |
|---|---|---|---|
| `TranscriptEvent` | `transcripts/<id>.events.jsonl` | `transcript_revisions` | `TranscriptLedger::SCHEMA_VERSION` (`1`) |
| `DiarizationSpanRevision` | `transcripts/<id>.speaker.jsonl` | `speaker_revisions` | `SpeakerTimeline::SCHEMA_VERSION` (`1`) |
| `ProjectionPatch` | `projections/<id>.events.jsonl` | `projection_patches` | `1` (new named constant) |
| `DataMovementEvent` | `ledgers/<id>.movements.jsonl` | `data_movement` | `DATA_MOVEMENT_SCHEMA_VERSION` (`1`) |

The conductor should either accept this mapping explicitly in the 6896 plan
or record a short decision before implementation. The kernel test-only
`"transcript"` id is not a production contract.

## Complete consumer map

### Repository boundary and replay helpers

`LocalMemoryRepository` exposes all four target readers as payload-only
`Vec<T>` results (`C:src-tauri/src/persistence/mod.rs:502-575`). Its transcript,
speaker, and projection replay helpers consume those methods directly and
already fail when domain replay rejects a stale/conflicting event
(`C:src-tauri/src/persistence/mod.rs:647-684`). Both the explicit-root and
user-data `FileMemoryRepository` branches currently route to `load_jsonl`
(`C:src-tauri/src/persistence/mod.rs:1172-1237`), while the free user-data
loaders do the same (`C:src-tauri/src/persistence/mod.rs:2640-2706`). These are
the primary integration seams: make both roots call the same strict adapter,
then have the compatibility `Vec<T>` methods project payloads from a verified
snapshot.

Do not route `SurrealMemoryRepository` through a file parser. It returns typed
repository records, is non-selectable for the MVP, and has no mixed JSONL
format. The richer presence/head API should either have an explicit
repository-record variant or remain a `FileMemoryRepository` capability until
the storage-adapter contract is deliberately expanded.

### Command, replay, Review, timeline, and export consumers

| Consumer | Current reads | Strict-reader behavior required |
|---|---|---|
| Projection replay report | Transcript and projection repository readers at `C:commands.rs:5918-5925`; then replay/evaluation at `:5927-6012` | Loader corruption must propagate before report construction. Do not turn a structural error into an empty replay or materialized-cache comparison. |
| Standalone transcript command | `C:commands.rs:6077-6102` reads only legacy segments and skips malformed rows. `I:commands.rs:6563-6608` correctly prefers transcript events, but uses `events.is_empty()` as the fallback test. | Preserve the `I:` ledger-first code, but fall back only when `presence == Missing`. A present-empty canonical stream returns an empty transcript and remains authoritative. A corrupt canonical stream errors; it never falls back to legacy. |
| Data-route command | `C:commands.rs:6116-6123` calls the free movement loader. `I:commands.rs:6611-6630` has corrected Unknown/privacy copy. | Decode legacy or mixed movement rows strictly and propagate corruption to the panel error state. Preserve the main-only closed-world copy. |
| Historical Review load | `C:commands.rs:6168-6305` reads all three event streams but then mutates the live graph, ledger, materializer, and schedulers at `:6249-6294`. `I:commands.rs:6655-6761` is the accepted read-only implementation and uses projection-file presence at `:6693-6740` so an existing empty canonical projection stream overrides an orphan cache. | Patch the `I:` implementation, not `C:`. Strict snapshots feed transcript/speaker/projection payloads; projection authority comes from snapshot `Present`, not a second existence check. Preserve no-live-state mutation and fail the entire Review load on framed corruption. |
| Session export bundle | Three repository event readers at `C:commands.rs:6325-6369`; the `I:` counterpart also derives transcript segments ledger-first at `I:commands.rs:6764-6824`. | Preserve `I:` semantics. Export valid legacy or mixed streams; fail the bundle on any framed corruption. Never substitute legacy transcript rows or derived caches after a present canonical stream fails. |
| Session timeline | Transcript and speaker repository readers at `C:commands.rs:6409-6444` / `I:commands.rs:6864-6899`, then the pure fold in `timeline.rs:104-186`. | The command gets verified payloads; either strict load failing aborts the fold. The pure timeline module needs no parser change and must remain non-mutating. |
| Projection replay trait helper | Transcript/projection loads at `C:persistence/mod.rs:673-684`. | Strict loader failures propagate; no cache fallback belongs in this helper. |
| Transcript and speaker replay trait helpers | `C:persistence/mod.rs:647-670`. | Missing produces an empty ledger/timeline only through an explicit compatibility projection; present-empty is still marked present in the richer result. |

Every frontend caller already has an error surface. The clean store catches
timeline, Review, export, and transcript command errors
(`C:src/store/index.ts:1670-1695`, `:2697-2768`, `:2845-2857`), and the
data-route panel enters an explicit error state
(`C:src/components/SessionDataRoutePanel.tsx:217-232`). Preserve the newer
`I:` historical-load generation guards and Live/Review lockout rather than
copying the clean store wholesale.

### Session recovery and statistics readers

Orphan recovery bypasses `LocalMemoryRepository`. It scans transcript,
transcript-event, speaker, and projection filenames
(`C:src-tauri/src/sessions/mod.rs:458-610`) and directly parses transcript and
speaker event lines for statistics. `transcript_event_stats` and
`diarization_event_speaker_count` skip each malformed row and continue
(`C:src-tauri/src/sessions/mod.rs:647-723`; unchanged semantically at
`I:sessions/mod.rs:820-895`). A framed suffix therefore becomes a series of
"malformed" rows and silently undercounts the recovered session. Worse,
`recovered_metadata` prefers a legacy transcript path whenever one exists and
only consults transcript events when it does not
(`C:sessions/mod.rs:781-830`; `I:sessions/mod.rs:954-1003`).

This reader must eventually consume the shared strict adapter, derive counts
from replayed current transcript/speaker state, and reject a corrupt candidate
without writing an index row. It overlaps the one-manifest recovery redesign
and the main checkout's substantial deletion/recovery changes, so it should
remain a required `audio-graph-be7c` follow-up unless the conductor explicitly
widens 6896 ownership to `sessions/mod.rs`. It must not be "fixed" by adding a
second frame parser.

### Artifact inventory, export parity, delete, and purge

The clean production inventory names only legacy transcript, the three event
streams, notes, and two graph files
(`C:src-tauri/src/sessions/mod.rs:403-447`). The explicit-root repository has a
different ten-descriptor list that additionally includes live-assist audit and
current files plus movement (`C:persistence/mod.rs:923-988`), proving test and
production drift.

The dirty integration checkout already expands the production inventory to
movement, scheduler, usage, live-assist, and atomic temp artifacts
(`I:sessions/mod.rs:466-513`) and implements active-session guards plus
artifact-first, index-last retry-safe deletion
(`I:commands.rs:6924-6964`; `I:sessions/mod.rs:335-399`, `:536-620`). Preserve
all of it. It still is a duplicated path list, not ADR-0027's typed manifest,
and it does not record canonical heads, schema/privacy class, or quarantine
receipts.

Deletion and purge should enumerate paths; they should not parse canonical
content first. Corruption must never cause fallback to a narrower inventory or
make sensitive residue invisible. Conversely, Review/export existence guards
must use the manifest/presence result and never infer authority from decoded
row count. Complete movement export parity, typed residual IPC, recovery parity,
and quarantine/head inventory belong to `audio-graph-be7c` and
`audio-graph-8e73`, not to a reader-only patch.

## Corruption, fallback, and mutation verdict

The current `load_jsonl` reader fails on a malformed line, so ordinary
repository calls do not deliberately fall back after corruption. The unsafe
fallbacks occur one layer above it:

1. `I:commands.rs:6563-6579` treats an empty decoded transcript event vector as
   absence and falls back to legacy rows. This loses existing-empty authority.
2. The clean Review implementation treats an empty projection vector as no
   canonical authority (`C:commands.rs:6208-6239`); the dirty integration copy
   correctly switched to file presence, but does so through a separate
   `exists()` probe. The strict snapshot should replace that probe.
3. Orphan recovery skips malformed event rows, produces partial/zero counts,
   and can then persist a recovered index entry. That is the only mapped path
   where a tolerant canonical-content read directly feeds mutation.
4. The legacy transcript derivative reader skips malformed legacy segment rows
   (`C:commands.rs:6083-6093`; `I:commands.rs:6580-6591`). That leniency is
   acceptable only after the canonical stream is proven `Missing`; it must
   never run after a canonical error or present-empty result.

No Review, replay, timeline, export, or movement reader may pass
`QuarantineUnterminatedTail`. Strict-reader tests must hash source bytes and
inventory siblings before and after every failure, proving no truncate,
quarantine, rename, temp file, or cache rewrite. Destructive repair remains
exclusive to a quiesced owner under `audio-graph-8e73`.

## Recommended implementation ownership and scope split

### In Seed 6896

1. Add `src-tauri/src/persistence/canonical_reader.rs` containing the four
   immutable descriptors, the presence-bearing result, the single generic
   strict adapter, payload projection helpers, and focused fixtures.
2. Expose it from `persistence/mod.rs`. Route both explicit-root and user-data
   `FileMemoryRepository` loaders through it. Keep `load_jsonl` for genuinely
   legacy/non-canonical logs such as promotion and live-assist audit streams.
3. Preserve the payload-only trait methods for callers that do not need
   authority, but add `FileMemoryRepository` snapshot methods so Review/export
   can consume presence and head without probing the filesystem twice.
4. Apply command changes against the newer `I:commands.rs`, preserving its
   read-only Review implementation, present-empty projection/cache behavior,
   and corrected export comments. Load transcript/speaker/projection snapshots
   once per command and reuse their rows; do not re-read transcript events to
   derive the legacy view and then again for the returned payload.
5. Route the projection report, standalone transcript command, movement
   command, Review load, export bundle, and timeline through the adapter.
6. For movement rows, additionally reject a payload whose `session_id` differs
   from the requested session or whose embedded `schema_version` differs from
   `DATA_MOVEMENT_SCHEMA_VERSION`. The outer frame validates framed context,
   but legacy movement rows carry these fields and otherwise could claim a
   different session/schema.
7. Add no runtime `CanonicalAppender`, writer-format selection, repair, index
   mutation, manifest rewrite, frontend response-shape change, or Surreal
   selection.

### Split out of Seed 6896

- `sessions/mod.rs` orphan scan/statistics and index reconstruction: update
  `audio-graph-be7c`. It needs the typed manifest as the candidate inventory
  and must preserve the dirty integration checkout's retry-safe deletion and
  malformed-index protections.
- Movement inclusion in portable export, typed missing/present/coverage status,
  and production producer completeness: `audio-graph-70a3` plus
  `audio-graph-51e0`.
- Quarantine receipts, directory barriers, exclusive repair, and subprocess
  crash proof: `audio-graph-8e73`.
- Fresh-process end-to-end proof across Review/replay/timeline/export/delete:
  `audio-graph-2add`.

### Integration hard stop

Do not mechanically cherry-pick a `commands.rs`, `sessions/mod.rs`,
`persistence/mod.rs`, `lib.rs`, store, or type file from `C:` over `I:`. The
clean branch is a canonical-kernel base, not the newest product tree. Either
first commit/integrate the main-only MVP hardening into a clean ancestor, or
construct the 6896 Act patch against an isolated copy of those exact main
working-tree files and review the complete merge-base footprint. An
adapter-only green branch does not satisfy the Seed because the command
consumers would still be unproven.

## Fixtures and tests

### Adapter fixtures

Use one shared fixture harness, with payload factories for all four domain
types. Test-only `CanonicalAppender` use is acceptable to build typed mixed
streams, while the kernel's exact byte/hash golden fixtures remain the
independent wire-format drift guard
(`C:canonical_log.rs:2047-2206`).

Required happy-path matrix:

- missing file;
- present zero-byte file;
- present whitespace-only file;
- one and multiple legacy records;
- legacy prefix plus one and multiple framed records;
- legacy final row without newline followed by an appender-inserted separator
  and valid frame;
- CRLF legacy prefix plus LF frames;
- every domain descriptor, asserting row order, internal
  `LegacyJsonl`/`FramedV1` provenance, and the final verified head;
- movement payload session/schema validation.

Required fail-closed matrix, each with before/after byte and directory-entry
equality:

- legacy after framed;
- wrong envelope session, stream id, and outer schema version;
- unsupported frame version, bad/oversized length, missing terminator;
- bad sequence, previous hash, payload hash, and record hash;
- duplicate envelope event id and duplicate JSON members;
- typed payload decode failure;
- newline-terminated interior corruption and unterminated final corruption;
- permission/sharing/read errors where deterministic on the host.

The full structural matrix can use one small representative payload because it
exercises the shared parser. Each of the four real payload types still needs
legacy-only and mixed-format typed-decode coverage, plus a wrong-descriptor
test proving the centralized stream mapping is enforced.

### Consumer fixtures

- standalone transcript: missing canonical falls back to legacy; present-empty
  canonical suppresses legacy; mixed canonical derives the latest duplicate-
  free transcript; corrupt canonical never falls back;
- Review: mixed transcript/speaker/projection rows hydrate the returned
  historical payload; present-empty projection clears orphan derived caches;
  any corrupt stream fails without mutating active state; retain the main-only
  A-then-B and Live/Review isolation assertions;
- replay report: mixed streams report verified event counts; corruption returns
  an error rather than a zero/empty report;
- timeline: mixed transcript and speaker streams fold latest-wins attribution;
  corruption aborts instead of returning a partial timeline;
- export: legacy-only and mixed streams serialize in order; corruption aborts;
  movement omission remains explicit until manifest export;
- data route: legacy-only and mixed movements load in append order; missing and
  present-empty remain internally distinct; corrupt/tampered movement enters
  the existing UI error path and never becomes an empty privacy claim.

## Recommended gates

Use the repository-pinned Windows gate environment already proven by b481:
`CARGO_BUILD_JOBS=1`, `CARGO_INCREMENTAL=0`,
`AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1`, and
`RUSTFLAGS=-C linker=lld-link`.

```text
cargo +1.95.0 fmt --all -- --check
cargo +1.95.0 metadata --locked --format-version 1 --no-deps
cargo +1.95.0 test --locked --lib --no-default-features --features cloud persistence::canonical_log::tests -- --test-threads=1
cargo +1.95.0 test --locked --lib --no-default-features --features cloud strict_reader_ -- --test-threads=1
cargo +1.95.0 test --locked --lib --no-default-features --features cloud load_session_ -- --test-threads=1
cargo +1.95.0 test --locked --lib --no-default-features --features cloud session_timeline_ -- --test-threads=1
cargo +1.95.0 test --locked --lib --no-default-features --features cloud session_export_ -- --test-threads=1
cargo +1.95.0 test --locked --lib --no-default-features --features cloud data_movement_ -- --test-threads=1
cargo +1.95.0 clippy --locked --lib --no-default-features --features cloud -- -D warnings
git diff --check
```

Name every new test with the `strict_reader_` prefix so the focused gate is
real. After integration, run the full locked cloud library suite because the
main-only command/session changes have broad state and lifecycle coverage. If
frontend response shapes stay unchanged, run the existing focused store,
SessionsBrowser, SessionDataRoutePanel, and timeline tests as regression proof;
no frontend rewrite is required in this slice.

Static review gates:

- no production call to `CanonicalAppender` outside `canonical_log.rs`;
- no consumer call using `QuarantineUnterminatedTail`;
- no `events.is_empty()` or row-count check used as canonical-file presence;
- no direct `serde_json::from_str` reader for the four target stream paths;
- no session command catches a strict canonical error and substitutes legacy,
  cache, or an empty success value.

## Proposed downstream Seed updates

The conductor should record these findings rather than leaving them only in
this report:

- `audio-graph-6896`: freeze the four stream-id/outer-schema mappings; require
  a presence-bearing snapshot; name the dirty-main integration hard stop and
  the movement payload session/schema check.
- `audio-graph-be7c`: orphan recovery must use the same strict descriptors,
  derive stats from validated replay, refuse to index a corrupt candidate, and
  carry stream presence/head/schema plus quarantine receipts in the one typed
  manifest. Preserve the main-only expanded inventory and retry-safe deletion.
- `audio-graph-70a3`: distinguish missing, present-empty, valid non-empty, and
  corrupt movement evidence; include the movement stream/head in export,
  delete, purge, and the future exhaustive-coverage marker contract.
- `audio-graph-51e0`: corruption/read failure is an explicit unavailable/error
  state, never an empty ledger; no negative egress claim may be inferred from
  missing or present-empty data without the versioned exhaustive marker.
- `audio-graph-1f71`: corrupt mixed-format Review must leave the active
  aggregate untouched; present-empty transcript/projection streams remain
  authoritative; preserve the already implemented read-only main slice.
- `audio-graph-0d72`: timeline construction aborts on transcript/speaker stream
  corruption and never renders a partial who-said-what timeline as complete.
- `audio-graph-8e73`: destructive repair is separate from all strict consumer
  reads and requires quiescence, one verified locked handle, manifest-first
  quarantine registration, and directory/subprocess durability proof.
- `audio-graph-2add`: the golden fixture must restart into strict mixed-format
  readers and assert Review, replay, timeline, manifest/export, delete, and
  zero-residual behavior from a fresh process.

## Unknowns that change the implementation plan

1. Exact production stream ids are not frozen anywhere; the proposed names
   above need conductor/ADR acceptance before fixtures become durable.
2. The current trait erases missing versus present-empty. Decide whether the
   richer result is a file-repository capability for this wave or a breaking
   trait-wide contract. Do not fake presence for Surreal.
3. Main-only MVP hardening is still uncommitted. Until it is preserved in an
   isolated integration snapshot, command-level Act work has an unacceptable
   regression footprint.
4. Orphan recovery both reads and writes. Including it in 6896 expands file
   ownership into the heaviest main-only session-safety slice; deferring it to
   be7c is the safer bounded decision, but runtime framed writers must remain
   blocked until that follow-up is complete.
