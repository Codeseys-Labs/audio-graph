# AudioGraph Wave 7C Session-semantics kernel plan

## Current state and custody

- Active Seed: `audio-graph-b887`, child of `audio-graph-7e81`.
- Exact execution base: `967cb4837b58592d180a3cdb22675d28e6101c36`.
- Branch: `work/audio-graph-b887-session-semantics-kernel-wave7c`.
- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/b887-session-semantics-kernel-wave7c`.
- Initial status: clean; no staged, unstaged, or untracked paths.
- Existing authority: `SessionArtifactManifestV1` is the typed Session inventory and
  `ManifestCasOutcome` distinguishes durably `Accepted`, exact
  `AlreadyCompleted`, rejection, and indeterminate durability.
- Dormancy boundary: no production manifest store has a filesystem
  qualification, so unqualified persistence cannot claim `Accepted`.

## Acceptance and public TDD seams

Work proceeds as independent vertical red/green slices through these agreed
public seams:

1. Historical missing `session_semantics_version` decodes as v1; only the
   monotonic v1-to-v2 transition is legal.
2. Logical floor advancement consumes the actual `ManifestCasOutcome` and is
   admitted only by an `Accepted` manifest or an exact `AlreadyCompleted`
   manifest. No generic receipt, boolean, or setter is exposed. Exact
   guard-ahead retry at v2 is idempotent.
3. A v2 transcript revision, hash-v2 Projection Basis, or hash-v2 projection
   patch observed under floor v1 produces its own typed, content-free
   corruption classification.
4. Checked Session open validates the persisted floor against reader support
   before invoking the supplied canonical/legacy content-reader closure.
5. New candidate manifest wire writes an explicit floor. A v2 manifest must
   inventory one `SessionProvenanceEvents` identity; unsupported values and
   floor regression fail closed.
6. Unqualified production manifest persistence continues to refuse
   `Accepted`; only the existing test-only qualification can establish the
   algorithmic acceptance proof.

Each slice starts with one failing test at its public seam, records the exact
RED output, then adds only enough implementation for GREEN. Focused
`session_semantics` and `session_artifact_manifest` tests and locked cloud
check/typecheck run throughout.

## Owned footprint

- `src-tauri/src/persistence/session_semantics.rs` (new)
- `src-tauri/src/persistence/mod.rs` (module declaration/helper surface only)
- `src-tauri/src/persistence/session_artifact_manifest.rs`
- this plan
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-b887-report.md`

No Seeds file, command/session/projection/canonical implementation, workflow,
dependency, generated artifact, frontend path, or other documentation is in
scope.

## Hard stops

- No fifth ADR-0037 canonical stream.
- No production `CanonicalFilesystemQualification` constructor.
- No v2 artifact writer, Projection Basis, or patch activation.
- No broad Review/load/export/delete/recovery migration.
- No predecessor binary canary or weakened Windows refusal.
- No durability claim synthesized from a caller-provided generic receipt.

If the existing manifest CAS outcome cannot carry honest acceptance evidence
at this footprint, implementation stops rather than widening scope.

## Verification and integration handoff

During implementation: focused Rust tests plus locked cloud check/typecheck.
At the final candidate: the serialized full cloud library suite, strict
Clippy, rustfmt, `bun run verify:fast`, all five repository contracts,
Betterleaks, docs/Seeds secret hygiene, diff/footprint checks, and a runtime-dark
search. Logical steps are committed with `audio-graph-b887` in each message.
The conductor owns rebase, squash, merge, push, Seed mutation, and worktree
cleanup.
