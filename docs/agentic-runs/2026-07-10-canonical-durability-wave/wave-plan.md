# Canonical durability wave plan

Date: 2026-07-10

Seeds: `audio-graph-90f3`, `audio-graph-b481`, `audio-graph-6896`,
`audio-graph-8e73`

## Adopted verdict

The candidate is a useful bounded research kernel, but it is unsafe to adopt as
the runtime authority. The current wave therefore fixes only two blocking
intra-kernel contracts in `audio-graph-b481`:

1. v1 payload commitments must be independent of JSON object insertion order
   and transitive `serde_json` feature unification; and
2. uncertain append recovery must prove the original base stream and exact
   attempted suffix before it mutates or retries.

No runtime writer or destructive public reader is enabled in this wave.

## Discovery evidence

- Correctness review: the current payload digest is not format-stable and
  length-only recovery can accept or destroy bytes that do not belong to the
  pending append.
- Durability review: new-file directory entries, quarantine registration,
  one-handle repair, cross-process ownership, and subprocess/power-loss proof
  remain P0 prerequisites.
- Runtime map: strict mixed-format readers must land before framed writers;
  transcript is the first eventual Pending-to-Accepted writer migration, then
  projection and diarization.

The three source reports are preserved in their review worktrees and will be
copied into this run directory during reconciliation.

## Workstream A — `audio-graph-b481` (this Act phase)

Owner: conductor in `E:/CS/github/audio-graph-canonical-log`

Files in scope:

- `src-tauri/src/persistence/canonical_log.rs`
- ADRs governing the two architectural choices
- this run directory

Implementation:

- recursively sort JSON object keys before v1 wire serialization and payload
  hashing;
- add immutable exact frame/hash fixtures so writer/reader drift cannot pass by
  sharing the same helper;
- retain expected base head and newline state in each `PendingAppend`;
- on uncertainty, reparse the original base, compare its semantic head/state,
  and require the observed suffix to be an exact prefix of the attempted frame;
- leave mismatched bases or foreign suffixes poisoned and byte-for-byte
  unchanged;
- cover the unterminated-legacy separator case and strict reopen after recovery;
- close the Windows test cleanup handle leak found by review.

Gates:

```powershell
Set-Location E:\CS\github\audio-graph-canonical-log\src-tauri
$env:CARGO_BUILD_JOBS = '1'
$env:CARGO_INCREMENTAL = '0'
$env:AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST = '1'
$env:RUSTFLAGS = '-C linker=lld-link'
cargo +1.95.0 fmt --all -- --check
cargo +1.95.0 test --locked --lib --no-default-features --features cloud persistence::canonical_log::tests -- --test-threads=1
cargo +1.95.0 clippy --locked --lib --no-default-features --features cloud -- -D warnings
```

The application manifest pins `rsac` v0.4.1 to
`7956e6ef24a44672d502e72b0500efb27530e3b9` for all three desktop targets, and
the application lockfile is release-tracked with SHA-256
`C1248BDE7D41EB60D6F88F727DD796CAF050A2E3D02DB8403932801BF49288E5`.
The prior dirty-sibling failure and the locked correction are recorded in
`audio-graph-bfa8`.

Rollback: the module has no runtime callers. Reverting its export and file
removes the entire code slice without changing any user data.

## Workstream B — `audio-graph-6896` (next wave)

Route transcript, projection, diarization, and movement reads through one
strict, non-mutating mixed legacy/framed adapter. Preserve missing-vs-existing-
empty authority and add corruption/cross-version fixtures before any framed
writer can create forward-only data.

## Workstream C — `audio-graph-8e73` (next durability wave)

Implement named file/directory barriers, one-handle locked recovery, a durable
manifest-first quarantine transaction, cross-process ownership checks, and
fresh-process crash gates. This remains a hard dependency of runtime
`Accepted`.

## Stop conditions

- At most one implementation/review fix round.
- Any failed exact golden fixture or strict reopen is a blocker.
- Any design that requires a runtime writer, public destructive recovery, or
  workflow change backflows to Plan instead of expanding this slice.
- `audio-graph-90f3` remains open even if `audio-graph-b481` closes.
