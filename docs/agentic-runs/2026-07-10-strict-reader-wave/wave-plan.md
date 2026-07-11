# Strict mixed-format reader wave plan

Date: 2026-07-10

Seeds: `audio-graph-b481`, `audio-graph-6896`, `audio-graph-8e73`,
`audio-graph-90f3`

## Phase 1: integrate and clean

1. Commit the already-reviewed b481 kernel and locked rsac prerequisite without
   expanding its scope.
2. Rebase that commit onto accepted ADR commit `1d1c7cc`.
3. Re-run all b481 gates on the integrated tree and close b481 only if green.
4. Hash/normalize-compare copied reviewer artifacts and rsac prerequisite
   files, then remove completed live worktrees through `git worktree remove`.
5. Prune only registrations whose filesystem worktrees are already missing.

## Phase 2: discover Seed 6896

Run three bounded read-only lanes:

- transcript/session Review readers and missing-versus-existing-empty
  authority;
- projection/diarization readers and corruption/version fixtures; and
- movement/export/timeline consumers plus integration seams.

Every lane writes a report under this run directory in its own worktree. The
conductor synthesizes one file-ownership plan and updates Seeds before code.

## Phase 3: bounded Act and review

Prefer one shared strict, non-mutating compatibility adapter rather than four
one-off parsers. The first implementation slice must prove legacy-only,
legacy-prefix/framed-suffix, and corrupt framed inputs without adding a runtime
writer or destructive recovery. Re-run affected repository/command tests plus
the canonical kernel gate, then obtain an independent snapshot review.

## Stop conditions

- Backflow to Plan if existing repository payloads cannot be represented by
  the current typed canonical reader without changing v1.
- Stop before Act if transcript/projection/diarization/movement ownership
  overlaps too broadly for one bounded commit; split the Seed and preserve the
  dependency order instead.
- Any mutation during strict read, fallback after framed corruption, or loss of
  existing-empty authority is a blocker.
- Runtime writer adoption and directory/manifest durability remain out of
  scope regardless of unit-test status.
