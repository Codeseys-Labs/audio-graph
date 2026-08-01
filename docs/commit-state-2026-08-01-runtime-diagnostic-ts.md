# Runtime diagnostic TypeScript contract commit state

Date: 2026-08-01

Seed: `audio-graph-5996`

Branch: `work/audio-graph-5996-runtime-diagnostic-ts`

Base: `cec53bdddb2eb456a2f73feab419536bd869983d`

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/5996-runtime-diagnostic-ts`

## Scope

- Generate the closed `RuntimeDiagnostic` TypeScript vocabulary and wire DTOs
  from the accepted Rust IPC contract.
- Validate untrusted runtime-diagnostic payloads by reconstructing only closed
  fields and falling back to one fixed internal tuple.
- Expose safe localization keys and recovery metadata for later status,
  readiness, realtime, TTS, and LLM migrations.
- Do not migrate `StageStatus` or any provider call site in this slice.

## Worktree custody

- The repository's main checkout contains broad unrelated user WIP and remains
  custody-only for this mission.
- This clean, project-local worktree is the only implementation surface for
  `audio-graph-5996`.
- No workflow, credential-service core, adapter, migration, Settings, or
  provider-runtime files are in scope.

## Verification before review

- Rust runtime-diagnostic contract tests: 7 passed.
- Generated runtime-diagnostic drift check: passed.
- Generator launcher invariants: passed for all 6 launchers.
- Focused Vitest: 7 passed.
- TypeScript typecheck: passed.
- Scoped Biome, Rust formatting, and diff checks: passed.

Integration and Seed closure remain contingent on independent frozen-commit
review and re-gating on the credential-v2 integration branch.
