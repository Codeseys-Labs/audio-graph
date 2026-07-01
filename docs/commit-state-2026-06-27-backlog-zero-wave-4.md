# Commit State: Backlog-Zero Wave 4

Captured on 2026-06-27 during the continued deep-work-loop-tiered run.

## Commit

- Branch: `master`
- HEAD: `831cc30101840db87bd2b502f2da749d65fe1c22`
- HEAD summary: `831cc30 fix: address 6 real CodeRabbit findings from the PR review`
- Worktree caveat: the checkout is broadly dirty before this wave. Latest
  `git status --short` counted 198 rows: 24 staged-only modifications, 49
  staged+unstaged modifications, 52 unstaged-only modifications, 1
  added+modified path, 2 staged additions, and 70 untracked paths.

## Queue State

- `sd doctor --json`: 12 pass, 0 warn, 0 fail.
- `bun run check:seeds-json-output`: passes; `sd ready`, `sd blocked`, and
  `sd list` JSON envelopes parse through the repo and global Seeds CLI
  installs.
- Ready queue from the Seeds JSON helper after the latest Spark reconciliation:
  50 ready Seeds.
- Blocked queue from the Seeds JSON helper after the latest Spark
  reconciliation: 42 blocked Seeds.
- Open queue from direct `.seeds/issues.jsonl` parse after creating
  `audio-graph-1d59` and `audio-graph-d262`: 104 open Seeds: P1 34, P2 49,
  P3 19, P4 2.

## Constraints

- Main orchestrator owns `.seeds/issues.jsonl`, commit-state docs, final
  integration, and queue hygiene.
- CI and release workflow edits remain approval-gated because
  `.github/workflows/ci.yml` and `.github/workflows/release.yml` are already
  dirty. This wave records CI plans in Seeds but does not edit or dispatch
  workflows.
- No `sd sync` while unrelated staged/unstaged work would be swept into a
  shared commit.
- No live provider/API-key tests are run unless a safe local credential path is
  confirmed and the command can avoid echoing secrets.

## Wave 4 Scope

The current iteration is a fix-oriented projection of the deep-work-loop
template: no new external research is load-bearing for the immediate work
because the relevant gaps are already evidenced in Seeds and code. If a worker
finds provider/API uncertainty that changes the design, that follow-up should
be filed as a Seed and routed through a research pass.

Launched bounded, disjoint subagent work:

- OpenAI Realtime ASR socket-edge guard worker:
  `src-tauri/src/asr/openai_realtime.rs` only. Goal is to advance
  `audio-graph-d042` and reopened `audio-graph-3b9f` by guarding
  content-bearing socket writes and adding blocked-policy fake-server
  coverage.
- Settings top-level readiness live-region worker:
  `src/components/SettingsPage.tsx` and
  `src/components/SettingsPage.test.tsx` only. Goal is to advance
  `audio-graph-a6d4` by replacing broad dashboard live-region semantics with
  a concise status summary.
- Projection privacy scout:
  read-only. Goal is to identify the smallest safe implementation slice for
  the remaining `audio-graph-3b9f` projection fallback regression.

## Wave 4 Results So Far

- Clean lane worktrees were created from
  `831cc30101840db87bd2b502f2da749d65fe1c22` for CI, settings/credentials,
  ASR provider guards, transcript ledger, audio/source bus, and diarization.
- `wt-asr-provider-guards` now contains the OpenAI Realtime ASR socket-edge
  guard slice in `src-tauri/src/asr/openai_realtime.rs`. Verification in that
  clean worktree passed `rustfmt`, `git diff --check`, and the focused
  `openai_realtime` cargo test filter with 31 tests passing.
- `wt-settings-creds` now contains the Settings provider-readiness live-region
  cleanup in `src/components/SettingsPage.tsx` and
  `src/components/SettingsPage.test.tsx`. Verification in that clean worktree
  passed the focused SettingsPage live-region tests, Biome, and
  `git diff --check`.
- `wt-ci-blacksmith` currently has a doc-only CI/Blacksmith plan. No workflow
  edits were made because CI/release workflow changes remain approval-gated.
- The transcript ledger audit found an important integration mismatch: clean
  HEAD still has legacy transcript/session behavior, while the dirty main
  checkout contains event/projection work from earlier waves. The event model
  should be integrated first before ledger/materializer/projection UI work.
- A Spark ASR privacy reviewer found the `wt-asr-provider-guards` OpenAI
  Realtime slice is not mergeable as a production privacy fix yet: the guard is
  tested through injected blocked guards, but the normal runtime path still
  constructs `AsrWsWriteGuard::allow`. The next slice must inject runtime
  policy into the normal connect/send path and prove blocked mode sends no
  content frames and does not reconnect.
- A Spark credentials verifier reran the backend credential suite with 28
  passing tests and 1 ignored smoke. The real OS keychain smoke failed in this
  Linux environment because no default secret store is configured, so
  `audio-graph-0c08` remains open for macOS, Windows, and Linux keychain
  evidence.
- A Spark ProcessTree contract scout validated frontend selector/mapping tests,
  the generated audio-source contract check, and the ipc-contract Rust test.
  That left only heavier backend runtime/filter verification open.
- A follow-up Spark ProcessTree backend verifier ran the focused Rust parser,
  resolver, source capability, and source-info filters. `audio-graph-7ee6`
  was closed after the TS, generated contract, ipc-contract, and Rust backend
  evidence all passed.
- A Spark provider-registry scout found `bun run check:provider-registry` and
  the lightweight Rust provider-registry crate tests pass locally with no
  generated-file drift. `audio-graph-a805` remains open for macOS/Windows
  remote evidence.
- A Spark playback reviewer reran the focused playback slice with Rust 1.95:
  `playback::tests::` passed 14/14. `audio-graph-f53b` remains open because
  its closure boundary is clean Linux/macOS/Windows CI playback regression
  evidence, not another local unit-test patch.
- A Spark consumer-registry verifier completed the previously interrupted
  focused test pass: audio consumer, pipeline, backpressure, source descriptor,
  capture target, ASR capture selection, and converse teardown filters all
  passed. `audio-graph-afca` remains open because the Seed description still
  includes production OpenAI Realtime/local-hybrid S2S coexist/reject wiring and
  command-layer `start_capture`/`stop_capture` lifecycle coverage. Created
  `audio-graph-1d59` for that command-layer test gap and linked it as an
  `afca` blocker.
- A Spark Settings worker completed the safe `ProviderReadinessPanel` source
  label fix: present credentials without presence-source metadata now render
  the localized unknown source label instead of implying `credentials.yaml`.
  Focused ProviderReadinessPanel tests passed 15/15. `audio-graph-cbde` remains
  open for generic `llm.api` readiness/catalog support and provider catalog
  policy.
- A read-only LLM readiness planner found generic `llm.api` already has
  endpoint-aware saved-key routing and shared OpenAI-compatible `/models`
  helpers, while Cerebras and OpenRouter are largely first-class already. The
  missing slice is first-class `llm.api` health/catalog commands, provider
  descriptor hooks, readiness admission, and cache fingerprinting. Created
  `audio-graph-d262` as a P1 child blocking `audio-graph-1c2f` and
  `audio-graph-cbde`; OpenRouter accelerator endpoint UI stays on
  `audio-graph-61db` / `audio-graph-84f4`.
- A clean `wt-settings-config` worktree was created for `audio-graph-559d`.
  Its schema-version patch is not mergeable yet: review found the status is
  computed but not acted on by runtime load paths, file-level migration tests
  are missing, and the Rust/TypeScript settings contract must be synchronized
  or the field must be hidden intentionally.
- The `wt-asr-provider-guards` OpenAI Realtime slice now reaches the speech
  runtime constructor and is merge-ready only as a partial privacy guard slice.
  It still uses a default-allow hook rather than true runtime `PrivacyMode`,
  and needs explicit blocked commit/reconnect-policy redaction tests before
  full closure.

## Open Integration Work

- Do not squash/merge the ASR provider guard worktree as full privacy closure.
  It is merge-ready only as a partial guard slice; runtime `PrivacyMode`
  sourcing and blocked commit/reconnect-policy tests remain open.
- Do not squash/merge the Settings worktree slice as-is. It is useful reference
  evidence, but it is based on older plaintext credential hydration logic;
  only the live-region pattern should be manually ported onto the newer
  presence-only settings flow.
- `audio-graph-3b9f`, `audio-graph-d042`, `audio-graph-a6d4`, and
  `audio-graph-cbde` were updated with clean-worktree evidence, but remain open.
- `audio-graph-7ee6` is now closed; `audio-graph-0c08` remains open for
  cross-platform keychain smoke.
- `audio-graph-559d` remains open because the current clean-worktree
  schema-version patch is partial evidence only.
- `audio-graph-a805` remains open for remote macOS/Windows provider-registry
  check evidence.
- `audio-graph-f53b` remains open for clean Linux/macOS/Windows CI playback
  regression evidence.
- `audio-graph-afca` remains open for production non-Gemini runtime registration
  wiring/policy plus child `audio-graph-1d59` command-layer capture lifecycle
  tests.
- `audio-graph-d262` is the next clean-worktree slice for generic
  OpenAI-compatible `llm.api` Settings-open readiness/model discovery.
- `audio-graph-cbde` and `audio-graph-1c2f` are blocked by `audio-graph-d262`.
- `sd sync` remains unsafe in this checkout because broad unrelated changes are
  still staged/unstaged together.

## Prior Wave Carried Forward

The previous wave left `audio-graph-3b9f` open after reopening residual
socket-edge privacy gaps. AssemblyAI was completed as a partial slice:
runtime binary audio writes now pass through `AsrWsWriteGuard`, and focused
AssemblyAI tests passed 33/33 with the live smoke still ignored.
