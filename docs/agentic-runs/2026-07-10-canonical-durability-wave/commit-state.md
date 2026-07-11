# Canonical durability wave commit state

Date: 2026-07-10

## Frame

This bounded `agentic-sdlc-orchestrator` wave resumes P0 Seed
`audio-graph-90f3`. Its done condition is deliberately smaller than the full
Seed: execute the corrected canonical-log kernel tests on Windows, review the
stable kernel snapshot for correctness and durability gaps, persist every
actionable finding in Seeds, and leave an evidence-backed implementation plan
for the next runtime Pending-to-Accepted slice.

This wave does not claim that transcript, projection, or diarization runtime
writers have migrated to the new kernel.

## Repository state

- Main checkout: `E:/CS/github/audio-graph`
- Main branch and HEAD: `master` at
  `f97e19c251e4c227aade1289b2aba56e0d40ffca`
- Main checkout: broadly dirty with the prior integrated MVP-hardening work;
  it remains inspection- and Seeds-only for this wave.
- Wave worktree: `E:/CS/github/audio-graph-canonical-log`
- Wave branch and base: `codex-audiograph-canonical-log` from the same baseline
- Initial wave diff:
  - `src-tauri/src/persistence/mod.rs`: export `canonical_log`
  - `src-tauri/src/persistence/canonical_log.rs`: untracked 2,286-line kernel
- No staging, commit, push, workflow edit, or `sd sync` is authorized for the
  dirty main checkout.

## Queue state

- Active P0: `audio-graph-90f3` — canonical durability before live state.
- Downstream P0: `audio-graph-2add` — deterministic golden MVP data path,
  blocked by `audio-graph-90f3` and other correctness work.
- Ready P1: `audio-graph-2e97` — mid-capture pipeline/dispatcher supervision.
- MVP epic: `audio-graph-99eb` remains blocked and in progress.
- Provider expansion remains downstream of the MVP epic.

## Known verification at entry

- Twelve canonical-log tests passed before the final Windows handle change.
- The post-change 13-test run reached Rust code generation and was stopped
  before assertions; it is not evidence of a pass.
- The prior integrated checkout passed frontend, generated-contract, strict
  Clippy, Rust formatting, and 1,506-test Rust-library gates, but those results
  predate this isolated kernel and do not validate it.

## Wave bounds

- Native conductor plus at most three delegated discovery/review lanes.
- Each delegated worker writes only to its own artifact path/worktree.
- At most one review/fix round before an honest checkpoint.
- No runtime writer migration until the kernel verdict is green.
- No provider, CI/workflow, SurrealDB, or broad UI work in this wave.
