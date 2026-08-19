# Session Control Plane Wave 7C Decision State

Date: 2026-08-16

## Fixed custody

- Active Seed: `audio-graph-67a1`, P0 prerequisite of `audio-graph-7e81`.
- Exact base: `e64aa4a3aedb7e8839e1cb1e0e4cd01bd4e3de25`.
- Branch: `work/audio-graph-67a1-session-control-plane-wave7c`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/67a1-session-control-plane-wave7c`.
- Initial state: clean; no staged, unstaged, or untracked paths.
- One author owns this worktree. The conductor retains Seeds, review,
  integration, push, merge, and cleanup authority.

## Decision-only scope

This workstream owns exactly:

- `docs/adr/0044-keep-session-control-plane-in-the-flat-artifact-root.md`;
- the ADR-0044 row and link in `docs/adr/README.md`;
- this commit-state document; and
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-67a1-session-control-plane-plan.md`.

No production code, test, Seed, workflow, dependency, lockfile, generated
contract, frontend, build output, or other documentation is in scope. The
existing accepted ADRs are read-only evidence. No `sd sync` runs from this
worktree.

## Evidence at the base

- ADR-0027 requires one typed Session Artifact manifest to drive lifecycle
  parity and durable provenance.
- ADR-0041 requires a monotonic, durably Accepted v1-to-v2 Session semantics
  floor before v2 transcript, basis, or patch authority.
- ADR-0043 freezes four canonical event streams; a fifth stream requires
  explicit architectural backflow.
- The dormant manifest kernel hard-codes one manifest/temp pair at an explicit
  root, inventories a v2 provenance identity, and uses the canonical global
  lock.
- The durability substrate supplies shared/exclusive guards at one qualified
  root. Qualified Linux and macOS may install namespace-changing snapshots;
  Windows refuses them before mutation.
- Active Sessions ids are ASCII-safe and bounded to 128 bytes, while the
  dormant manifest wire separately accepts broader UTF-8 ids up to 255 bytes.
  Production control addressing uses only the narrower Sessions validator
  before path derivation; this does not redefine broad manifest wire validity.
- The dormant manifest and semantics kernels have no production caller and no
  persisted control-plane migration at this base.

These facts are current-code and accepted-record evidence at the exact base;
they are not implementation authorization.

## Proposed decision and human gate

ADR-0044 recommends per-Session manifest/provenance/temp filenames at the
existing qualified flat root, one bounded injective lowercase Base32 key, the
existing store-wide lock, and one immutable exact v1-to-v2 proof. The proof is
not a fifth canonical stream. Historical bootstrap must report Original
Session Audio from observed inventory without inventing a missing reason.
Windows remains read-capable and v2-mutation-refusing.

The ADR remains `proposed`. No production-code workstream may start until the
AudioGraph user and product owner explicitly accepts it. Acceptance is a
separate docs-only commit that atomically changes the ADR status, its date to
the actual acceptance date, and the README row's status/date. Acceptance
changes decision evidence only; it changes no production code or Seed and does
not authorize implementation or queue mutation.

## Verification and handoff

The documentation candidate runs Markdown whitespace/diff checks, an exact
four-path footprint check, an ADR index/link consistency check, relative-link
resolution, and `bun run verify:contracts`. The plan records the exact commands
and final results.

After human acceptance, the plan permits exactly two serial TDD workstreams:
first the shared persistence contract, then admission and lifecycle parity.
Both remain runtime-dark until their own scoped authorization, review, and
integration evidence exists.
