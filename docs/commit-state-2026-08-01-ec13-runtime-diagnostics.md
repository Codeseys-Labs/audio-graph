# audio-graph-ec13 runtime diagnostic contract

- Date: 2026-08-01
- Seed: `audio-graph-ec13`
- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/ec13-runtime-diagnostic-contract`
- Branch: `work/audio-graph-ec13-runtime-diagnostic-contract`
- Base: `b59c8c8ca0f931510642049d60f25ba4d6d36103`

## Scope and custody

This worktree was clean at intake. The slice owns the Rust IPC diagnostic
contract, its `ipc-contract` module export, the typed `AppError` conversion
boundary, colocated tests, and this document. It does not migrate provider,
credential, readiness, command, frontend, ASR, realtime, TTS, or LLM call
sites. Those remain owned by the dependent children of `audio-graph-87c9`.

No generated TypeScript projection is introduced here. The contract remains
Rust-owned for the later generated-projection child; no frontend projection is
load-bearing for this backend-only slice.

## Contract

- Credential failures embed the accepted e11c `CredentialError` tuple without
  translating it through a string.
- Non-credential failures use a closed runtime code and recovery vocabulary.
- Context contains only a closed operation, transport family, coarse response
  class, and saturating retry-delay bucket.
- Unknown native/provider sources are discarded at the `AppError` conversion
  boundary and map to the closed `internal` code.
- Legacy `AppError` variants and redaction helpers remain in place for
  dependent migrations; this slice adds a safe typed path without forcing
  those call-site changes.

## Verification

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check`
  passed with no output.
- Isolated-target, locked `audio-graph-ipc-contract` suite passed: 37 passed,
  0 failed.
- Isolated-target, locked cloud `error::tests` passed: 17 passed, 0 failed.
- Isolated-target, locked cloud `cargo check --lib --tests` passed.
- Isolated-target, locked cloud `cargo clippy --lib --tests -- -D warnings`
  passed.
- The public-field source guard found no free `String`, `usize`, or `u64`
  scalar in `runtime_diagnostic.rs` (expected `rg` exit 1 with no matches).
- `bun scripts/check-docs-secret-hygiene.mjs --fixture-self-test` passed.
- The normal docs/Seeds hygiene scan reported the accepted six-finding
  baseline from the 87c9 discovery: three `.seeds/issues.jsonl` entries, one
  provider architecture plan entry, and two credential-mechanism review
  entries. It reported no new finding in this slice.
- `bun run check:seeds-json-output` could not run its stress check because the
  global Seeds CLI lacks the repository's stdout retry patch. This worktree did
  not patch the global installation or run the write-capable preparation step.

The final `git diff --check`, footprint review, status, and commit hash are
recorded in the implementation artifact.

## Remaining release work

`audio-graph-87c9` remains release-blocking until its provider, credential,
readiness, frontend, logging, and cross-sink canary children migrate to this
contract and pass on the integrated snapshot.
