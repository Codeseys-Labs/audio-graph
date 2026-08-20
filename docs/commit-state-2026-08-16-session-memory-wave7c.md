# Session Memory Wave 7C production qualification

Date: 2026-08-16

## Fixed execution state

- Active Seed: `audio-graph-cc9a`, child and final production-qualification
  prerequisite of `audio-graph-7e81`.
- Exact base: `d31b5f9695164452a6c353b8230097fd8f661119`.
- Branch: `work/audio-graph-cc9a-production-qualification-wave7c`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-production-qualification-wave7c`.
- Initial status: clean; no staged, unstaged, or untracked paths.
- One writer owns this worktree. The conductor retains integration, Seeds,
  review dispatch, push, merge, and cleanup authority.

## Accepted prior evidence

- The integrated native durability protocol evidence remains accepted at this
  base: Linux/ext4 passed 42/42 durability and 11/11 crash tests; macOS/APFS
  passed 13/13 durability and 11/11 crash tests.
- Windows NTFS probing completed, but the pinned DevCon helper failed the
  Microsoft Authenticode predicate before installation. No Windows durability
  or crash test executed, so Windows namespace mutation remains fail-closed.
- That evidence validates the existing file/parent-barrier protocol. It does
  not validate the new sysinfo-backed production qualification seam; cc9a must
  produce its own code-specific evidence.

## Owned scope and non-goals

Owned paths are limited to:

- `src-tauri/src/persistence/canonical_durability.rs`
- `src-tauri/src/persistence/session_artifact_manifest.rs`
- this commit-state document
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-plan.md`
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-cc9a-report.md`

No Seed, workflow, dependency or lockfile, frontend, generated contract,
`canonical_log.rs`, crash harness, Session writer/consumer, or other path may
change. No unsafe code, fifth stream, or `audio-graph-b887` runtime activation
is in scope.

## TDD and platform boundary

Implementation uses one public-behavior RED/GREEN slice at a time: production
filesystem qualification; qualification-bound guard acquisition; qualified
manifest initial CAS; and qualified manifest replacement/open-head retention.
The filesystem policy is exercised through deterministic injected inventory,
while production authority always comes from a fresh live sysinfo inventory.

Linux admits only writable, non-removable ext4. macOS admits only writable,
non-removable APFS. Unknown, remote, FUSE, tmpfs, read-only, removable,
unmatched, and identity-unavailable namespaces refuse. Windows and Other
return a typed unsupported-namespace error before mutation. The current Linux
fixture is ext4 and may provide a live code-specific qualification test;
macOS/APFS and Windows remain unexecuted locally.

## Verification, rollback, and handoff

Focused `canonical_durability`, `session_artifact_manifest`, and
`session_semantics` tests run throughout. The final candidate runs locked cloud
check, one serialized full cloud library suite, strict Clippy, rustfmt,
typecheck, `verify:fast`, all five contracts, Betterleaks, docs/Seeds secret
hygiene, and exact diff/footprint/runtime-dark checks.

Rollback is the three logical cc9a commits on this dedicated branch: planning,
implementation, then report evidence. The conductor can omit or revert this
branch without touching any other Wave 7C work. If safe live qualification
cannot remain root/mount/object-bound using the existing sysinfo dependency and
safe Rust, implementation stops instead of widening the footprint.
