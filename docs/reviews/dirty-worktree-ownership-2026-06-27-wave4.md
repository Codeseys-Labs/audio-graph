# Dirty Worktree Ownership Map: Wave 4

Captured on 2026-06-27 for `audio-graph-bc1c`.

## Summary

Current checkout is intentionally broad and mixed. Latest `git status --short`
counted 198 dirty rows: 24 staged-only modifications, 49 staged+unstaged
modifications, 52 unstaged-only modifications, 1 added+modified path, 2 staged
additions, and 70 untracked paths. Parallel implementation must stay scoped
or use clean worktrees.

## Main-Thread Owned

- `.seeds/issues.jsonl`
- `docs/commit-state-*.md`
- `docs/reviews/dirty-worktree-ownership-*.md`

Reason: Seeds and run-state docs are the orchestrator's source of truth. Do
not let subagents edit these concurrently.

## Clean-Worktree Or Approval-Gated

- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `.github/actionlint.yaml`
- `package.json`, `bun.lock`, `biome.json`
- `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`
- `src/generated/`, `src/types/index.ts`

Reason: these affect CI/release, generated contracts, package resolution, or
cross-platform build behavior. Edit from a clean branch/worktree with explicit
merge sequencing.

## Sequence One Owner At A Time

- ASR core: `src-tauri/src/asr/mod.rs`, `deepgram.rs`, `assemblyai.rs`,
  `openai_realtime.rs`, `aws_transcribe.rs`, plus untracked provider modules.
- Runtime orchestration: `src-tauri/src/commands.rs`, `state.rs`,
  `settings/mod.rs`, `speech/mod.rs`, `speech/tests_integration.rs`.
- LLM/projection: `src-tauri/src/llm/*`, `src-tauri/src/projection_llm.rs`,
  `src-tauri/src/persistence/mod.rs`, `src-tauri/src/sessions/*`.
- Audio/source: `src-tauri/src/audio/capture.rs`, `pipeline.rs`, `mod.rs`,
  untracked `consumer.rs` and `pcm.rs`.
- Settings UI: `src/components/SettingsPage.tsx`,
  `src/components/SettingsPage.test.tsx`, `src/i18n/locales/en.json`,
  `src/i18n/locales/pt.json`.

Reason: these files already have mixed staged+unstaged state or broad
cross-module contracts. Assign exactly one worker per file group.

## Safe Micro-Slice Candidates

- Isolated new or narrow component files such as `SecretCredentialControl`,
  `ProviderReadinessPanel`, `providerSetupModes`, and `ModelCatalogPicker`,
  provided the worker does not touch `SettingsPage` or locale files.
- Provider-local test/guard work in a single provider file, one provider at a
  time, with main-thread Seed reconciliation after focused cargo tests.
- New docs/research artifacts under unique filenames.

## Merge Order Recommendation

1. Queue/tooling and commit-state docs.
2. Credentials/config persistence hardening.
3. ASR fixture/transport provider slices, one provider at a time.
4. Settings UI micro-slices, avoiding locale merges until component tests pass.
5. Transcript/projection persistence slices after commands/persistence owners
   are reconciled.
6. CI/workflow/package-lock changes from a clean worktree only.

## Current Wave Assignments

- `wt-asr-provider-guards` at `/mnt/e/cs/github/wt-asr-provider-guards`,
  branch `lane/wt-asr-provider-guards`: provider privacy and ASR harness work.
- `wt-settings-creds` at `/mnt/e/cs/github/wt-settings-creds`, branch
  `lane/wt-settings-creds`: settings/credentials UX and backend credential
  work.
- `wt-settings-config` at `/mnt/e/cs/github/wt-settings-config`, branch
  `lane/wt-settings-config`: backend-only `config.yaml` / legacy
  `settings.json` migration work for `audio-graph-559d`.
- `wt-ci-blacksmith` at `/mnt/e/cs/github/wt-ci-blacksmith`, branch
  `lane/wt-ci-blacksmith`: CI/Blacksmith planning and later approval-gated
  workflow edits.
- `wt-transcript-ledger` at `/mnt/e/cs/github/wt-transcript-ledger`, branch
  `lane/wt-transcript-ledger`: event model, transcript ledger, notes/graph
  diff projection work.
- `wt-audio-source-bus` at `/mnt/e/cs/github/wt-audio-source-bus`, branch
  `lane/wt-audio-source-bus`: processed audio consumer bus, source descriptors,
  and provider source policy.
- `wt-diarization` at `/mnt/e/cs/github/wt-diarization`, branch
  `lane/wt-diarization`: diarization compile matrix, speaker timeline schema,
  and provider/local diarization normalization.

All six lane worktrees were created from
`831cc30101840db87bd2b502f2da749d65fe1c22` and verified clean with
`git status --short`.

## Dirty Main Checkout Slices To Extract

- `src-tauri/src/asr/openai_realtime.rs`: OpenAI Realtime socket-edge guard
  slice was implemented and tested in the dirty main checkout, but the file was
  already `MM`. Extract only the guard hunks into `wt-asr-provider-guards`
  before any merge.
- `src/components/SettingsPage.tsx` and
  `src/components/SettingsPage.test.tsx`: top-level readiness live-region
  cleanup was implemented and focused-tested in the dirty main checkout, but
  both files were already `MM`. Extract only that accessibility slice into
  `wt-settings-creds` and rerun tests there before merge.

## Clean Worktree Slices Awaiting Review

- `wt-asr-provider-guards`: `src-tauri/src/asr/openai_realtime.rs` contains
  the clean OpenAI Realtime socket-edge guard slice. It is verified with
  `rustfmt`, `git diff --check`, and focused `openai_realtime` Rust tests.
  A fresh Spark privacy reviewer found a blocker: the guard is tested through
  injected blocked guards, but the normal runtime path still constructs
  `AsrWsWriteGuard::allow`, so app privacy policy cannot block content egress
  in production. Do not merge this as privacy closure until runtime policy
  injection and normal-path blocked-policy tests land.
- `wt-settings-creds`: `src/components/SettingsPage.tsx` and
  `src/components/SettingsPage.test.tsx` contain the clean provider-readiness
  live-region cleanup. It is verified with focused SettingsPage tests, Biome,
  and `git diff --check`. Read-only review found the worktree is based on
  older plaintext credential hydration logic; do not squash-merge it as-is.
  Manually port only the live-region pattern onto the newer dirty-main
  presence-only readiness flow.
- `wt-transcript-ledger`: `src-tauri/src/projections.rs`, `src-tauri/src/lib.rs`,
  and `src-tauri/src/user_data.rs` contain a clean contract-only event model
  slice. Do not squash-merge this over the main checkout as-is: the main
  checkout already has a richer untracked `src-tauri/src/projections.rs`
  implementation. Treat this as sequencing evidence and reconcile by
  preserving a superset of the richer main implementation. Read-only review
  also found the clean worktree uses a different artifact layout from the
  current `ad44` design. The redaction-safe `Debug` idea is tracked separately
  as `audio-graph-9338`.
- `wt-audio-source-bus`: do not implement new source/audio code here yet. A
  read-only scout found this clean worktree is behind the dirty-main
  source/audio baseline and lacks current `src-tauri/crates`, generated
  contracts, and `src-tauri/src/audio/consumer.rs`. The safe immediate action is
  `audio-graph-bfcb` verification only. After source/audio baseline
  integration, the next code slice should stay registry-only under
  `src-tauri/crates/provider-registry/src/lib.rs` for `audio-graph-a2ff`.
- `wt-settings-config`: clean backend-only worktree for
  `audio-graph-559d`. It should touch `src-tauri/src/settings/mod.rs` only and
  avoid SettingsPage, provider settings components, i18n, docs, workflows,
  package files, and credential backend files.

## Spark Wave Integration Notes

- `ProviderReadinessPanel.tsx` and `ProviderReadinessPanel.test.tsx` were a
  successful narrow Settings micro-slice: blank or absent credential source
  metadata now renders as unknown for present credentials, while missing
  credentials still render missing. Keep future saved-key UI work in this
  component unless it truly needs `SettingsPage` orchestration.
- `src-tauri/src/commands.rs` remains high-collision. The new
  `audio-graph-1d59` command-layer capture lifecycle test Seed should have a
  single owner and should not be mixed with provider readiness, CI, or ASR
  provider edits.
- `audio-graph-afca` should not be closed from registry unit tests alone. Its
  acceptance text still includes production OpenAI Realtime/local-hybrid S2S
  coexist/reject wiring and per-consumer health surfacing, so the next owner
  must either implement those paths or split remaining acceptance explicitly
  into child Seeds before closure.
- `audio-graph-f53b` has enough local playback resampling evidence. Its next
  owner should collect clean Linux/macOS/Windows CI playback regression logs,
  not add more local playback unit tests unless CI reveals a platform-specific
  failure.
- `audio-graph-d262` is the generic OpenAI-compatible `llm.api` saved-key
  readiness/catalog slice. It crosses `commands.rs`, `settings/mod.rs`,
  provider-registry descriptors, generated TypeScript, and Settings UI files,
  so it must run in a clean worktree with backend contract work first and
  frontend wiring second. Keep it separate from OpenRouter accelerator UI,
  which is already tracked by `audio-graph-61db` and `audio-graph-84f4`.
