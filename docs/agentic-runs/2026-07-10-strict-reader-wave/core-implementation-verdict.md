# Strict reader core implementation verdict

Date: 2026-07-10

Seed: `audio-graph-9fc4`

Integrated commit: `daf1ff6`

Verdict: accepted for the isolated reader-core slice. Runtime writer adoption,
consumer authority changes, session-index purity, and recovery remain outside
this commit.

## Implemented contract

- Centralized ADR-0037's four private stream descriptors and independent outer
  schema-v1 constants.
- Added one typed, presence-bearing strict reader that maps only
  `Io(Read, NotFound)` to `Missing`; a successfully opened empty/blank stream is
  `Present` with zero records.
- Added crate-scoped transcript, speaker, projection, and data-movement snapshot
  APIs while keeping the existing payload-only compatibility loaders.
- Routed explicit-root and default-root file loaders through resolve-only paths
  so missing canonical stream reads do not create the data root or directories.
- Validated data-movement payload session identity and pinned outer-v1 to
  embedded schema-v1 with compile-time and runtime drift guards.
- Kept Surreal rows outside file framing and added no production appender,
  quarantine, repair, or writer-format switch.

## Plan backflow

The first focused run exposed that `StdFileOps::read_log` converted filesystem
`NotFound` to empty bytes. That made a race-free `Missing` result impossible and
misclassified absent streams as present-empty. The authorized kernel backflow
now propagates redacted `Io(Read, NotFound)` and has a regression proving the
missing path and parent remain absent. `CanonicalAppender` open/create behavior
is separate and unchanged.

## Review and fix round

The adversarial review found two P1 issues and one P2 gap:

1. Public descriptors plus a public generic loader could bypass the durable
   registry. The descriptor and generic loader are now private; only four typed
   readers reach the parent module, and snapshot APIs are crate-scoped.
2. Movement outer-v1 followed the mutable current IPC schema constant. It now
   maps to an independently pinned embedded-v1 constant, with compile-time and
   test drift failures requiring an explicit migration.
3. Lower-level mixed tests did not prove repository path routing. A regression
   now writes distinct real payloads to all four actual repository paths and
   verifies present typed records and heads.

The reviewer approved the post-fix isolated merge. It did not run Cargo gates.

## Executed evidence

Authoritative Windows environment:

```text
CARGO_BUILD_JOBS=1
CARGO_INCREMENTAL=0
AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1
RUSTFLAGS=-C linker=lld-link
```

- `strict_reader_`: 8 passed, 0 failed, 1,474 filtered.
- `persistence::canonical_log::tests`: 24 passed, 0 failed, 1,458 filtered.
- Existing file-repository round trip: 1 passed, 0 failed.
- Existing `data_movement_ledger_` regressions: 5 passed, 0 failed.
- `cargo +1.95.0 fmt --all -- --check`: passed.
- Locked metadata: passed.
- Locked cloud library Clippy with `-D warnings`: passed.
- `git diff --check`: passed.
- `cloud,surrealdb-embedded` library compile check: passed. This is adapter
  compatibility evidence, not runtime enablement or repository conformance.

A conductor rerun without the manifest/`lld-link` environment produced Windows
`STATUS_ENTRYPOINT_NOT_FOUND` before assertions. Re-running the identical test
filters under the authoritative environment passed 8/8 and 24/24. This is
recorded as gate-wrapper/DevEx evidence rather than a code assertion failure.

## Remaining boundaries

- `audio-graph-9eee` must integrate presence-aware snapshots into the newer
  main-only Review, replay, timeline, export, and movement consumers. It must
  replace row-count fallback without overwriting existing safety hardening.
- Default-root nonempty routing is manually verified in the four free-function
  mappings but lacks the same end-to-end four-stream fixture; missing default
  roots have a non-creation regression.
- Production-domain golden frame/hash fixtures remain a follow-up; canonical-log
  v1 itself retains exact byte/hash fixtures.
- Orphan recovery remains `audio-graph-be7c`; directory/quarantine durability
  remains `audio-graph-8e73`; fresh-process proof remains `audio-graph-2add`.
- The broadly dirty main checkout remains untouched. Consumer integration must
  be hunk-based and preserve movement locking, malformed-index behavior,
  synchronous writer-open readiness, Review isolation, deletion safety,
  privacy truth, and frontend response ordering.
