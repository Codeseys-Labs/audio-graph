# Strict reader wave commit state

Date: 2026-07-10

## Frame

This wave begins by integrating the reviewed `audio-graph-b481` kernel before
starting `audio-graph-6896`. The dependency is intentional: mixed-format
readers must consume the exact v1 contract that b481 freezes, and no runtime
writer may be added in this wave.

Done for the integration gate means:

- the accepted ADR commit and reviewed kernel are one linear branch;
- the exact integrated tree passes the locked canonical-log tests, strict
  Clippy, Rust formatting, metadata, and diff gates;
- `audio-graph-b481` closes only after those results are recorded;
- completed review/ADR/rsac worktrees are removed only after their unique
  changes or artifacts are proven preserved; and
- `audio-graph-6896` is re-read from the now-unblocked Seeds queue before Act.

## Repository state

- Main checkout: `E:/CS/github/audio-graph`
- Main branch/HEAD: `master` at
  `f97e19c251e4c227aade1289b2aba56e0d40ffca`
- Main caveat: broadly dirty user/integration checkout; no write-capable work,
  staging, commit, or `sd sync` occurs there except Seeds queue updates.
- Kernel worktree: `E:/CS/github/audio-graph-canonical-log`
- Kernel branch/base at wave start: `codex-audiograph-canonical-log` at
  `f97e19c251e4c227aade1289b2aba56e0d40ffca` with the reviewed b481 slice
  uncommitted.
- Accepted ADR branch: `codex/adr-canonical-log-v1` at
  `1d1c7cc157a3b4bd250119be6344ace29fca662e`.

## Cleanup inventory

- `audio-graph-wt-90f3-correctness`: no commits beyond the common base;
  report artifacts copied into the canonical durability run directory.
- `audio-graph-wt-90f3-durability`: no commits beyond the common base;
  report artifacts copied into the canonical durability run directory.
- `audio-graph-wt-90f3-integration`: no commits beyond the common base;
  runtime integration map copied into the canonical durability run directory.
- `audio-graph-rsac041`: detached at the common base; its exact lockfile and
  rsac v0.4.1 prerequisite are subsumed by the kernel worktree.
- `audio-graph-wt-adr-canonical-v1`: clean committed ADR branch; removable only
  after the kernel branch is based on its commit.
- Twenty-three already-missing historical worktree registrations are eligible
  for metadata pruning after live worktree reconciliation.

## Run bounds

- Native provider delegation; no CAO, cmux, or tmux dependency.
- At most three read-only discovery lanes for Seed 6896.
- At most one implementation lane and one adversarial fix round.
- No runtime `CanonicalAppender` caller, destructive reader repair, CI change,
  push, or force operation.
- Directory/manifest/subprocess durability remains owned by
  `audio-graph-8e73`.
