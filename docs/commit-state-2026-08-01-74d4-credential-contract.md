# audio-graph-74d4 credential IPC contract state

Date: 2026-08-01

Seed: `audio-graph-74d4`

Branch: `work/audio-graph-74d4-credential-contract`

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/74d4-credential-contract`

Base: `b9bdd48d11144edbee475237028a775f6e0ba0b6`
(`work/audio-graph-cred-v2-integration`)

## Custody and scope

- The dedicated worktree was clean at intake and is the only write surface for
  this workstream.
- The slice owns the Rust credential IPC source of truth, its generated
  TypeScript projection and focused contract tests, plus this note.
- Native adapters, runtime workers, application state, Tauri commands/events,
  Settings UI, provider consumers, file-v2, workflows, and Seeds mutations are
  excluded.
- `sd sync`, pushes, and writes to the integration or primary checkout are
  intentionally excluded; the root conductor owns tracker and fan-in state.

## Contract seam and invariants

Tests exercise the public Rust serde boundary and generated TypeScript
projection. `CredentialSetRecordState::Unknown` is only a runtime/IPC
projection for pre-authority, opening, locked, or unavailable states; it is not
a valid persisted ready-journal record state. Versioned status, mutation,
diagnosis, and change-notification envelopes remain content-free. A change
notification contains only schema version, global epoch, and the mutation
receipt. No shared contract type can serialize or return a saved secret draft.

## Baseline and verification plan

The starting commit already contains the Rust-owned credential vocabulary,
redacted service status and error tuples, mutation receipts, deterministic
generator, and checked-in TypeScript output. Implementation proceeds test-first
with focused Rust and TypeScript contract tests, followed by the generated-file
drift check, typecheck, Rust format/check/Clippy, documentation secret-hygiene,
and `git diff --check`.

## Final verification

- The full `audio-graph-ipc-contract` suite passes: 40 passed, 0 failed.
- Rust format, all-target check, and strict all-target Clippy pass for the
  contract crate with Rust 1.95.0.
- The generated credential-contract drift check passes, the focused TypeScript
  contract suite passes 9 tests, and repository typecheck passes.
- Scoped Biome validation, the documentation hygiene fixture self-test, and
  `git diff --check` pass.
- The repository-wide documentation/Seeds scan still reports the same six
  pre-existing findings in unchanged tracker, plan, and review files. No
  finding is in this workstream's footprint.
