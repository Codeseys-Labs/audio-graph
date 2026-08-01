# Credential Filesystem Policy Core Worktree State

Date: 2026-08-01

Seed: `audio-graph-dccd`

Parent Seed: `audio-graph-e241`

Branch: `work/audio-graph-dccd-filesystem-policy-core`

Base: `42ea2b693c8ae9cfaf8ee2407580f7762789b6e7`

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/dccd-filesystem-policy-core`

## Custody

- This clean worktree is the only write surface for the Seed.
- The main checkout and every other worktree remain custody-only.
- No Seed, adapter, persistence wrapper, path-resolution, file-I/O, Cargo/lock,
  AppState, IPC, frontend, generated, or workflow file belongs to this branch.
- No integration, remote operation, or push will be performed here.

## Accepted policy contract

Implement the common backend-private credential-filesystem decision core from
ADR-0035 and the accepted `audio-graph-c7b2` research contract:

- closed persistence-target, platform, filesystem-family, ternary observation,
  fault, and status types;
- a detector trait seam whose held-target representation remains adapter-owned;
- a pure evaluator with the normative denial precedence;
- target-specific, versioned build-time evidence-profile table shapes;
- separate journal and file-v2 evidence, with both compiled tables initially
  empty; and
- content-free formatting/serialization that cannot carry private locators,
  storage identities, provider identifiers, native codes, or native prose.

The first candidate classifications are only local fixed/internal
Windows/NTFS, macOS/APFS, and Linux/ext4. They remain unsupported in production
until reviewed evidence profiles are compiled into a later release.

## Confirmed test seams

- The principal seam is the pure evaluator over one closed
  `FilesystemObservation` or one closed detector fault.
- Deterministic tests may inject in-module fake evidence profiles; production
  evaluation can consult only the compiled profile tables.
- A fake detector may retain unsafe-looking source details internally, but only
  its closed observation/fault crosses the trait seam. Status JSON and debug
  output are asserted to remain closed and content-free.

## Owned files

- `src-tauri/src/credentials/filesystem_policy.rs` (new)
- declaration only in `src-tauri/src/credentials/mod.rs`
- this commit-state record

## Stop conditions

- Stop rather than add target-native OS calls, paths, handles, file operations,
  persistence wiring, adapters, dependencies, or lockfile changes.
- Stop rather than authorize any filesystem without accepted release evidence.
- Do not let an unknown observation become favorable or let an evidence profile
  bypass a prior runtime denial.
- Journal evidence must never authorize file-v2 confidentiality.

## Planned verification

Use `/tmp/audio-graph-target-dccd` as the isolated Cargo target. Run focused
policy tests, default and locked-cloud credential suites, accepted IPC contract
tests, locked cloud check, strict Clippy, Rust formatting, scoped diff checks,
generated credential-contract drift, and the repository hygiene fixture/scan.
Exact commands and results will be recorded in the implementation artifact.
