# Canonical durability wave implementation and validation

Date: 2026-07-10

Primary Seed: `audio-graph-b481`

Parent and successors: `audio-graph-90f3`, `audio-graph-6896`,
`audio-graph-8e73`

## Outcome

The bounded canonical-log v1 kernel now satisfies the commitment-stability and
uncertain-recovery-identity acceptance criteria. Two independent adversarial
review lanes approve the slice with no remaining P0, P1, or P2 finding inside
its declared scope.

This result does **not** authorize a runtime writer. Strict mixed-format reader
integration, parent-directory durability, manifest-first quarantine, file
identity, and fresh-process crash proof remain hard P0 successors.

## Implemented contracts

- Canonical v1 recursively sorts every JSON object before storage and hashing.
  Arrays retain order and scalar bytes remain owned by the pinned serializer.
- Tests deliberately enable `serde_json/preserve_order`, so removing the
  normalizer changes the golden commitment instead of passing accidentally on
  a `BTreeMap` feature graph.
- Framed v1 parsing rejects duplicate JSON object member names recursively
  before typed conversion, normalization, or semantic hash validation.
- Exact immutable fixtures freeze object-order behavior plus string escaping,
  Unicode, signed and unsigned integer limits, a finite fraction, payload hash,
  record hash, full frame bytes, and stream head.
- Each uncertain append retains its original byte length, semantic head,
  newline state, immutable frame, event commitment, and target sequence.
- Recovery mutates only a suffix proven to be a strict byte prefix of that
  immutable frame after reparsing and matching the exact original base.
- A complete matching event crosses a fresh flush and file-sync barrier before
  `AlreadyAccepted`. A zero-byte uncertainty retries without mutation.
- `Strict` mode never quarantines a partial or separator-only pending suffix.
  Mismatches remain poisoned, reject another event, and are repeatably
  byte-for-byte non-mutating.
- The locked file open now states `.truncate(false)` explicitly: opening a
  canonical stream retains all existing bytes.

## Dependency and reproducibility prerequisite

The application no longer resolves an ambient dirty sibling checkout for
`rsac`. Windows, macOS, and Linux target dependencies pin rsac v0.4.1 to:

`7956e6ef24a44672d502e72b0500efb27530e3b9`

The application lockfile is release-tracked and its verified SHA-256 is:

`C1248BDE7D41EB60D6F88F727DD796CAF050A2E3D02DB8403932801BF49288E5`

The v0.4.1 additive capture capability field is represented in the existing
capture fixture as `requires_user_consent: false`.

## Executed gates

Environment for Rust compilation:

```powershell
$env:CARGO_BUILD_JOBS = '1'
$env:CARGO_INCREMENTAL = '0'
$env:AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST = '1'
$env:RUSTFLAGS = '-C linker=lld-link'
```

Final-source evidence:

| Gate | Result |
|---|---|
| `cargo +1.95.0 test --locked --lib --no-default-features --features cloud persistence::canonical_log::tests -- --test-threads=1 --nocapture` | 23 passed, 0 failed, 0 ignored, 1,451 filtered; 0.29 s assertions after 2 m 53 s rebuild |
| `cargo +1.95.0 clippy --locked --lib --no-default-features --features cloud -- -D warnings` | passed; 22.59 s final rerun |
| `cargo +1.95.0 fmt --all -- --check` | passed |
| `cargo +1.95.0 metadata --locked --format-version 1 --no-deps` | passed |
| locked serde feature inspection | `serde_json/indexmap -> serde_json/preserve_order` present in the test graph |
| locked rsac feature inspection on Windows | only `rsac/feat_windows` selected |
| `git diff --check` | passed |
| ADR structure/index validation | 36 ADR files, 36 index rows; ADR-0035 and ADR-0036 accepted |

The full docs/Seeds secret scanner is not green in this older isolated
worktree: it reports six pre-existing findings in stale `.seeds` and historical
provider/credential documents. It reports no finding in this wave's run or ADR
files. The later defanged main-worktree state was intentionally not swept into
this bounded branch, so this is recorded as baseline evidence rather than
misrepresented as a passing gate.

The first strict Clippy run identified an ambiguous `OpenOptions` contract for
`create(true)`. The implementation was corrected with explicit
`truncate(false)`, after which both Clippy and the full 23-test kernel gate were
rerun on the final source.

## Adversarial review verdicts

- `implementation-review.md`: approved for the bounded b481 kernel and ADR
  slice; no remaining actionable P0/P1/P2 finding.
- `durability-implementation-review.md`: ready to close from the durability
  review perspective; the file-only receipt language remains honest.
- Discovery reports remain alongside these final reviews so the rejected
  runtime-adoption assumptions and downstream data-path map stay auditable.

## ADR state

ADR-0035 defines v1 recursive key ordering, duplicate-member rejection,
serializer/scalar ownership, feature-invariant fixtures, reblessing rules, and
the format-version boundary. ADR-0036 defines expected-base identity,
exact-prefix-only repair, complete-event reconciliation, strict-mode behavior,
poison transitions, and explicit durability deferrals.

The ADR set was committed as
`1d1c7cc157a3b4bd250119be6344ace29fca662e`. The reviewed kernel commit was
rebased directly on that ADR commit as
`7b0e5d003dcc23c971561c85fe5a5a57dc6920ed`, then re-gated on the integrated
tree. Nothing has been pushed.

## Queue reconciliation

- `audio-graph-b481` meets its reviewed acceptance on the integrated branch and
  is eligible to close. Its successors remain separate blockers.
- `audio-graph-6896` remains the next code wave: strict, non-mutating,
  mixed-format readers for transcript, projection, diarization, and movement.
- `audio-graph-8e73` remains the runtime durability gate: directory barriers,
  one-handle repair, durable manifest-first quarantine, file identity, and
  subprocess/power-loss proof.
- The typed-manifest, active Review isolation, movement ledger, golden data
  path, and worker-supervision Seeds were updated with the reader/runtime map.
- `audio-graph-90f3` remains open and runtime `Accepted` remains blocked.

## Rollback

There are still no runtime callers. Removing the module export, kernel file,
rsac prerequisite slice, and run artifacts reverts this wave without touching
user data. No canonical v1 runtime data has shipped.
