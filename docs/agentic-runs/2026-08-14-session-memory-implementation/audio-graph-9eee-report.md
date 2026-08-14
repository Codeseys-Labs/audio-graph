# audio-graph-9eee strict snapshot consumer report

Date: 2026-08-14

Seed: `audio-graph-9eee`

Branch: `work/9eee-strict-snapshot-wave2`

Exact base: `b312dc39841d91be0bae40d91accde5f7b58b7d9`

Historical discovery: `24ab2b51e47a331875e81b5d3405cb808f2b9a9a`

Historical reviewed reader core: `daf1ff674a8e3f09086fa85da2662c156c50b844`

Historical verdict: `d733a1454b5debe879e74ce8012a8a308ca4ee46`

No historical commit was cherry-picked. The accepted reader/source concepts
were transplanted onto the current base and current command/session hardening
was preserved. No branch was pushed, no workflow was dispatched, no Seed was
closed, and `.seeds/issues.jsonl` was not modified.

## Outcome

The file repository now has one strict, presence-bearing canonical read
authority for transcript revisions, speaker revisions, projection patches, and
data-movement events. It accepts legacy JSONL and legacy-prefix/framed-v1
streams, preserves `Missing` versus `Present(empty)`, validates the ADR-0037
stream registry and movement payload context, and fails closed on structural,
context, version, hash, and payload errors.

The standalone transcript, projection replay report, historical Review load,
session export, session timeline, and movement/data-route command paths now
open each canonical stream once and reuse the resulting payloads/presence. A
present-empty transcript suppresses legacy rows; a present-empty projection
stream suppresses orphan materialized caches; canonical corruption cannot
trigger legacy/cache/empty-success fallback.

Nominal reads use resolve-only canonical/cache paths, index lookup, and the
complete current session artifact inventory. Missing reads do not create the
data root or stream directories, and malformed `sessions.json` is neither
backed up nor rewritten by Review/export/transcript inventory lookup.

## Historical semantics used

- ADR-0027 file-canonical authority, strict corruption handling, and read-only
  historical ownership.
- ADR-0028 separation of historical Review from the active backend aggregate.
- ADR-0035 key-canonical framed-v1 commitments and immutable integrity checks.
- ADR-0036 fail-closed strict tail behavior; reader paths do not quarantine or
  repair.
- ADR-0037 registry identifiers and independent outer schema versions:
  `transcript_revisions`, `speaker_revisions`, `projection_patches`, and
  `data_movement_events`, all version 1.
- The reviewed `daf1ff6` `NotFound` backflow: only a strict read-side
  `Read/NotFound` maps to `Missing`; an opened blank stream is `Present`.
- The reviewed private registry/generic loader shape and four crate-scoped
  typed file snapshot methods.

## Historical semantics rejected or deferred

- The old command/session files were not copied or mechanically overwritten.
- The historical mutating `load_session` implementation was rejected; current
  read-only Review isolation remains intact.
- No runtime `CanonicalAppender` construction, writer-format switch, recovery,
  quarantine, repair, cache authority, or Surreal file framing was enabled.
  Appender code exists only as the already-reviewed inert kernel and test
  fixture surface.
- Orphan recovery/statistics remain unchanged and outside this Seed.
- Deletion, purge, and recovery mutation paths retain their existing checked
  index and expanded inventory behavior. The new resolve-only inventory is a
  separate read seam.
- No frontend behavior or command response shape changed. Existing live/Review
  locks and stale-response guards were not edited.

## TDD evidence

The public seams were fixed before production edits: standalone transcript,
projection replay, historical Review, export, timeline, movement/data-route,
and resolve-only inventory/index behavior.

Initial command:

```text
CARGO_TARGET_DIR=.../src-tauri/target/9eee-wave2 cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud strict_reader_ --locked -- --nocapture --test-threads=1
```

Red result on exact base plus tests:

```text
running 6 tests
1 passed; 5 failed; 1506 filtered out

FAILED strict_reader_present_empty_transcript_stream_is_authoritative
FAILED strict_reader_loaded_session_reuses_one_present_empty_transcript_snapshot
FAILED strict_reader_export_reuses_one_present_empty_transcript_snapshot
FAILED strict_reader_nominal_missing_replay_does_not_create_data_root
FAILED strict_reader_nominal_transcript_read_does_not_back_up_malformed_index
ok     strict_reader_corrupt_canonical_transcript_blocks_legacy_fallback
```

Final green result:

```text
running 17 tests
test result: ok. 17 passed; 0 failed; 0 ignored; 1529 filtered out
```

The final filter includes the six command seams, reviewed canonical
kernel/reader regressions, resolve-only user-data and session inventory/index
tests, movement context validation, and the compatibility transcript helper's
present-empty authority regression.

## Files changed

- `src-tauri/src/persistence/canonical_log.rs`
  - reviewed canonical-log v1 kernel and tests;
  - preserves filesystem `NotFound` on read;
  - test-local type alias added to satisfy strict Clippy.
- `src-tauri/src/persistence/canonical_reader.rs`
  - private ADR-0037 registry;
  - `Missing | Present(CanonicalLogSnapshot<T>)` strict reads;
  - movement session/schema validation and focused fixtures.
- `src-tauri/src/persistence/mod.rs`
  - resolve-only repository routes and strict snapshot methods;
  - compatibility payload loaders now project only after strict validation;
  - materialized/live-assist reads no longer create directories;
  - transcript compatibility helper now uses presence, not row count.
- `src-tauri/src/user_data.rs`
  - resolve-only data-root, index, and four canonical stream path helpers.
- `src-tauri/src/sessions/mod.rs`
  - separate non-mutating read-only index lookup and complete resolve-only
    artifact inventory;
  - deletion/recovery/statistics implementations unchanged.
- `src-tauri/src/commands.rs`
  - one transcript snapshot reused by display events in Review/export;
  - strict speaker/projection/replay/timeline/movement reads;
  - projection presence replaces a second `exists()` probe;
  - focused command regressions.
- This report.

## Verification

All Rust commands used the stable worktree target:

```text
CARGO_TARGET_DIR=/home/codeseys/DevBox/audio-graph/.worktrees/9eee-strict-snapshot-wave2/src-tauri/target/9eee-wave2
```

### Focused reader and consumer gates

```text
strict_reader_: 17 passed, 0 failed
persistence::canonical_log::tests: 24 passed, 0 failed
commands::tests::load_session_: 9 passed, 0 failed
commands::tests::session_timeline_: 1 passed, 0 failed
export_session_bundle: 2 passed, 0 failed
commands::tests::projection_replay_report_: 5 passed, 0 failed
data_movement_ledger_: 5 passed, 0 failed
load_transcript_segments_: 3 passed, 0 failed
```

### Locked cloud check

```text
cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud --locked
Finished dev profile; exit 0 (final incremental rerun: 7.85s)
```

### Full direct cloud library suite

```text
cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud --locked -- --test-threads=1
test result: ok. 1538 passed; 0 failed; 8 ignored; finished in 38.19s
```

### Strict Clippy

```text
cargo +1.95.0 clippy --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud --locked -- -D warnings
Finished dev profile; exit 0 (final incremental rerun: 17.07s)
```

The first strict Clippy attempt identified three `type_complexity` warnings in
historical canonical-log test helpers. One test-local tuple alias fixed all
three; the command above is the green rerun.

### Format, generated contracts, and diff hygiene

```text
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
bun run verify:contracts
git diff --check
```

All exited 0. The four generated TypeScript contracts were current. The full
`verify:fast` frontend gate was not run because this worktree has no
`node_modules` and no command signature, IPC DTO, generated contract, or
frontend file changed; the dependency-free generated-contract gate was run
instead.

Static review also found no production `CanonicalAppender` construction or
`QuarantineUnterminatedTail` use outside the inert canonical kernel, no
row-count canonical presence decision, and no separate projection-stream
`exists()` authority probe.

## Review-fix round

One bounded review-fix round addressed two accepted read-path blockers on top
of implementation commit `a9b407217df638628bb09a1908f3c4625e228490`.

### Export metadata index purity

`session_export_bundle` still populated `metadata` through the older
`sessions::find_session` path. That path delegates to the checked index loader,
which backs up malformed `sessions.json` even though export is nominally
read-only. Export now uses `find_session_resolve_only`, consistent with its
artifact inventory and indexed path resolution.

The export regression snapshots the complete data-root tree as relative names,
directory entries, and file bytes before and after a successful legacy-session
export with malformed `sessions.json`. It proves exact tree equality and the
absence of any `.corrupt-*` backup.

### Malformed legacy transcript fail-closed behavior

When the canonical transcript stream is genuinely missing, the compatibility
legacy reader previously warned and skipped malformed JSONL rows, then returned
later valid rows as an incomplete success. `read_legacy_session_transcript`
now returns on the first malformed nonblank row with a content-redacted row
number and no source line, transcript text, or parser excerpt.

The same shared helper serves standalone transcript, historical Review, and
session export. Separate regressions prove all three public surfaces reject the
malformed-first/valid-second fixture and that the returned errors omit the
private malformed-row marker. Canonical missing versus present-empty semantics
are unchanged.

### Review-fix red and green evidence

Before either production edit:

```text
cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud strict_reader_review_fix_ --locked -- --nocapture --test-threads=1

running 5 tests
test result: FAILED. 0 passed; 5 failed; 0 ignored; 1546 filtered out
```

The failures were the export tree-purity assertion and incomplete-success
results from the shared helper, standalone transcript, Review, and export.

After the two fixes:

```text
running 5 tests
test result: ok. 5 passed; 0 failed; 0 ignored; 1546 filtered out
```

Surrounding focused reruns:

```text
strict_reader_: 22 passed, 0 failed
commands::tests::load_session_: 9 passed, 0 failed
export_session_bundle: 2 passed, 0 failed
load_transcript_segments_: 3 passed, 0 failed
```

Review-fix full gates:

```text
locked cloud check: exit 0; 10.49s
full direct cloud library suite: 1543 passed, 0 failed, 8 ignored; 38.32s
strict cloud Clippy -D warnings: exit 0; 18.80s
rustfmt check: exit 0
bun run verify:contracts: all four contracts current
docs/Seeds secret hygiene: 0 findings
git diff --check: exit 0
```

The full `verify:fast` frontend gate remained inapplicable for the same reason
as the implementation round: no frontend, command signature, IPC DTO, or
generated contract changed, and this isolated worktree has no `node_modules`.
No recovery, deletion, writer, frontend, Seed, or unrelated file was changed.

## Risks and remaining acceptance

- Linux local Rust evidence is complete for this bounded Seed. Packaged
  Windows/macOS mixed-format consumer proof remains part of the broader
  ADR-0032/release evidence, not this branch.
- Mixed legacy/framed payload decoding is exercised for all four domains at the
  shared strict reader, while command tests cover missing, present-empty,
  legacy, and corrupt behavior. There is not a separate framed happy-path
  fixture for every command because all commands consume the same typed reader
  methods.
- Speaker-aware historical projection-basis replay remains the dependent
  `audio-graph-edc8` work; this Seed does not change that decision.
- Orphan recovery/statistics and destructive repair remain assigned to their
  existing Seeds and were intentionally not changed.

Within the assigned local implementation scope, no known acceptance criterion
remains incomplete.
