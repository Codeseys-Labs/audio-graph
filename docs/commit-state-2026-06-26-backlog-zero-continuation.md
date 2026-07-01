# Commit State - 2026-06-26 - Backlog Zero Continuation

**Timestamp:** 2026-06-26T01:45:00-07:00  
**HEAD:** `831cc30101840db87bd2b502f2da749d65fe1c22` (`master`) - `fix: address 6 real CodeRabbit findings from the PR review`

## Purpose

This checkpoint restarts the deep-work-loop-tiered continuation from current
evidence. The active goal is still to drive the roadmap/Seeds backlog to zero,
but this checkout is not a clean branch. Treat the current worktree and Seeds as
authoritative, and avoid broad sync/commit/push operations until ownership is
isolated into clean worktrees.

## Baseline Commit

The latest committed baseline is the May 31 CodeRabbit follow-up commit. It
fixed diarization worker timing/exit behavior, OpenAI Realtime commit cadence,
Gemini force-stop handle cleanup, Gemini audio IPC payload shape, and API key
trimming in the generic API client.

## Worktree State

`git status --short` shows broad staged and unstaged work across:

- GitHub CI/release workflows and release docs.
- Seeds state.
- Provider registry/codegen and generated frontend registry output.
- ASR providers, TTS, OpenAI Realtime, Gemini, Moonshine, Gladia, RevAI,
  Speechmatics, Soniox, and shared ASR fixtures.
- Credentials/config storage, provider readiness, Settings UX, i18n, and
  generated/provider panels.
- Processed audio consumer/PCM contracts, capture, playback, speech runtime,
  projection scheduler, transcript ledger, notes, and graph materialization.
- Research, ADR, review, and checkpoint docs.

Several files are `MM`, meaning staged edits predate later unstaged edits. Do
not run `sd sync`, create a broad commit, force-push, rewrite history, or edit
CI/release workflows from this dirty checkout without explicit approval or a
clean worktree plan.

## Seeds Snapshot

`sd stats --json` reports:

- Total Seeds: 236
- Active Seeds: 97 (`77 open`, `20 in_progress`)
- Closed Seeds: 139
- Blocked Seeds: 41
- Current ready count from `sd ready --format json`: 38
- Current blocked count from `sd blocked --format json`: 41

Top active lanes visible in Seeds labels and queue shape:

- Cross-platform release/CI/Blacksmith validation and rsac release hygiene.
- Config, credentials, keychain/fallback source labels, and provider readiness.
- Source/audio foundation: processed PCM, consumer bus, source selection, playback.
- Streaming STT provider platform and provider registry expansion.
- Transcript ledger, notes diffs, graph projection, replay parity, and memory repository.
- Diarization/speaker timeline and local/hybrid voice agent prerequisites.
- Native S2S sibling providers and local/hybrid S2S pipeline.
- Product/competitive roadmap: onboarding, live assist, memory workspace, trust, benchmark, calendar, and later collaboration/integration/sharing work.

## Recent Verified Work Before This Checkpoint

The latest local slices advanced `audio-graph-ad98` provider diagnostic
redaction:

- HTTP/model/readiness/provider body diagnostics now route through bounded safe
  excerpts.
- Streaming chat, TTS handshake, OpenRouter/generic API/Cloud ASR paths have
  targeted redaction coverage.
- WebSocket connect/read/protocol/close diagnostics for Deepgram ASR,
  AssemblyAI ASR, OpenAI Realtime ASR, and Deepgram Aura TTS now pass through
  key-aware redaction.
- Server-sent WebSocket Error/Warning diagnostics are redacted for current
  ASR/TTS handlers.

Focused verification recorded in `audio-graph-ad98` includes no-default cloud
Rust checks, redaction unit tests, parser regression tests, `rustfmt --check`,
`git diff --check`, `sd doctor --json`, and `bun run check:seeds-json-output`.
The Seed remains open for future native S2S wrapper coverage and cross-platform
runner evidence.

## Operating Assumptions

- Use Template A from `deep-work-loop-tiered`: the main orchestrator holds the
  outer frame/plan/verdict and launches bounded subagents only for independent
  scoped lanes.
- Use hyperresearch only when an item has real external unknowns. Current
  already-researched implementation slices should prefer local code and Seeds
  evidence.
- Do not edit CI/release workflows in this dirty checkout. For CI work, record a
  clean worktree/branch plan or wait for explicit approval.
- Before starting non-trivial code, update the relevant Seed. After each slice,
  update or close Seeds only when acceptance and verification evidence support it.
- Keep React as configuration/control/display; backend owns provider sockets,
  credentials, processed PCM, graph updates, and source timing.
- Secrets must not be written to config, docs, logs, screenshots, or Seeds.

## Next Queue Bias

Prefer a non-CI, tightly scoped ready Seed with clear ownership and local test
coverage. Current candidates include:

- `audio-graph-a3d8`: frontend/i18n/docs credential source labels, now unblocked
  by backend per-key source metadata.
- `audio-graph-5679`: LocalMemoryRepository trait and file-backed adapter, a
  dependency for SurrealDB and memory workspace work.
- `audio-graph-d4d2`: classify provider file paths and multi-field credentials
  in settings.
- `audio-graph-6cce`: evaluate secrecy wrappers for runtime provider credentials.

CI/release Seeds such as `audio-graph-2586` and `audio-graph-8eeb` remain
important but should be handled from a clean workflow-edit branch/worktree.

## Continuation Snapshot - 2026-06-26T05:30:52Z

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`.
The worktree is still broadly dirty across CI, docs, backend, frontend, Seeds,
and generated/provider files; do not commit, sync, or edit CI/release workflows
from this checkout without isolating the work first.

Current Seeds evidence:

- `sd stats --format json`: 242 total, 70 open, 23 in progress, 149 closed,
  44 blocked.
- `sd ready --format json`: 29 ready items.
- `sd blocked --format json`: 44 blocked items.

Recent completed slices since the previous checkpoint:

- Closed `audio-graph-053f` after adding Rust/TypeScript schemas and validation
  tests for promotion events, redaction snapshots, org knowledge items, and
  sync state.
- Closed `audio-graph-6165` after adding redaction snapshot fixtures and
  org-visible payload omission tests for promoted notes, graph facts,
  transcript spans, and live-assist cards.
- Advanced `audio-graph-c237` with a LibriSpeech-derived source-separation
  fixture manifest and offline WAV/manifest validation. It remains open because
  actual mono-ASR and diarization baseline results are still required.

Current non-CI implementation focus:

- `audio-graph-5679` is in progress. Finish the backend-owned
  `LocalMemoryRepository` trait and file-backed adapter before durable
  live-assist storage, promotion audit persistence, SurrealDB adapter work, and
  cross-session memory architecture.

## Continuation Snapshot - 2026-06-26T03:01:57-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree is
still broadly dirty, including staged and unstaged changes in CI/release docs,
Seeds, backend providers, credentials/config, persistence/projection runtime,
frontend Settings/status UI, generated provider files, and research/ADR docs.
Several files are still `MM`. Do not run `sd sync`, broad commits, force-pushes,
history rewrites, or CI/release edits from this checkout unless the work is first
isolated into a clean worktree/branch or explicitly approved.

Current Seeds evidence after the persistence queue wave:

- Total Seeds: 250.
- Closed Seeds: 160.
- Active Seeds: 90 (`67 open`, `23 in_progress`).
- Active priority split: `30 P1`, `43 P2`, `16 P3`, `1 P4`.
- `sd ready --format json`: 32 ready items.
- `sd blocked --format json`: 39 blocked items.

Recent verified closures since the prior checkpoint:

- Closed `audio-graph-f2b6` after adding repository-backed
  `TranscriptEventWriter` and `ProjectionEventWriter` constructors that keep
  file storage as default while allowing ASR/projection call sites to append
  through a selected `LocalMemoryRepository` in tests.
- Closed `audio-graph-ff32` after adding typed session artifact descriptors and
  repository-level delete/export semantics so DB-backed repositories do not
  pretend records are filesystem paths.
- Closed `audio-graph-3a09` after bounding transcript/projection writer queues,
  using non-blocking `try_send`, emitting redacted
  `persistence-queue-backpressure` state, preventing ASR ledger advancement when
  transcript events cannot be enqueued, and preventing materialized notes/graph
  saves when projection events cannot be enqueued.
- Closed `audio-graph-24dc` after wiring the frontend event bridge, redacted
  store state, localized pipeline status warning, and store/event/UI tests for
  persistence queue pressure.

Verification evidence for the latest slice:

- `cargo +1.95.0` focused backend tests and checks for repository writers,
  bounded queues, ASR ledger behavior, projection materialized-state behavior,
  shutdown behavior, local memory repository tests, and `cloud` plus
  `cloud,surrealdb-embedded` compile checks were recorded in the closed Seeds.
- `bun run test -- src/store/slices.test.ts src/hooks/useTauriEvents.test.ts src/components/PipelineStatusBar.test.tsx`
  passed (`68` tests).
- `bun run typecheck` passed.
- `bunx @biomejs/biome@2.5.1 check` passed for the touched frontend files.
- `git diff --check` passed for the touched backend/frontend files and
  `.seeds/issues.jsonl`.
- `sd doctor --json` passed all 12 checks.
- `bun run check:seeds-json-output` passed.

Current queue bias:

- `audio-graph-2586` is the top ready P1 but edits release workflow behavior; keep
  it approval-gated or move it to a clean workflow-edit branch/worktree.
- `audio-graph-2b2c` is the next ready item and remains the decisive blocker for
  making SurrealDB storage selectable/default: file-backed SurrealKV/RocksDB
  evidence must be gathered on Blacksmith across Linux, macOS, and Windows.
- Product/UX P2s ready behind that include onboarding/sample session, trust model,
  live-assist triggers, pre-briefs, benchmark suite, Cerebras readiness, xAI
  diarization caveat, and VAD/AEC bakeoff.

## Continuation Snapshot - 2026-06-26T03:51:37-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree
is still broadly dirty across CI/release workflows, docs, Seeds, backend ASR
providers, audio/capture/playback, credentials/config, persistence/projection,
frontend Settings/status surfaces, generated provider files, and research/ADR
docs. Several files are still `MM`, so do not run `sd sync`, create a broad
commit, force-push, rewrite history, or edit CI/release workflows from this
checkout unless the work is first isolated into a clean worktree/branch or
explicitly approved.

Current Seeds evidence:

- Active Seeds: 91 (`68 open`, `23 in_progress`).
- Active priority split: `31 P1`, `43 P2`, `16 P3`, `1 P4`.
- `sd ready --format json`: 33 ready items.
- `sd blocked --format json`: 40 blocked items.

Current top ready item:

- `audio-graph-5633`: fix AssemblyAI and OpenAI Realtime reconnect retry
  stale-socket loop. This was filed from the `audio-graph-d042` transport
  fake-server scout and now blocks the reusable ASR provider transport/parser
  harness. Acceptance requires provider-local fake-server or fake-connector
  tests for reconnect failure, cancel during backoff, and no stale-socket
  `run_io` detour, followed by focused ASR tests and cargo check.

## Continuation Snapshot - 2026-06-26T06:45:00-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree
is still broadly dirty and includes staged/unstaged changes across CI, docs,
backend providers, frontend, generated registry files, and Seeds. Continue to
avoid `sd sync`, broad commits, force pushes, history rewrites, or CI/release
workflow edits from this dirty checkout unless the work is isolated first.

Current Seeds evidence before the onboarding hardening wave:

- Total Seeds: 258.
- Closed Seeds: 169.
- Active Seeds: 89 (`64 open`, `25 in_progress`).
- `sd ready --format json`: 32 ready items.
- `sd blocked --format json`: 36 blocked items.
- `sd doctor --fix --json`: repaired one reciprocal dependency mismatch after
  the xAI diarization metadata Seed closure, then passed all 12 checks.
- `bun run check:seeds-json-output`: passed against both local and global Seeds
  CLI installations.

## Continuation Snapshot - 2026-06-26T07:43:30-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The latest
committed baseline changed `src-tauri/src/commands.rs`,
`src-tauri/src/converse/mod.rs`, `src-tauri/src/diarization/worker.rs`,
`src-tauri/src/gemini/mod.rs`, `src-tauri/src/llm/api_client.rs`, and
`src-tauri/src/speech/mod.rs` to address six CodeRabbit review findings.

The worktree remains broadly dirty, including staged and unstaged changes in
CI/release workflows, Seeds, docs, backend provider/audio/persistence modules,
frontend Settings/status surfaces, generated provider-registry output, and
research/ADR artifacts. Several files are still `MM`, so the current
orchestration rules remain unchanged:

- Do not run `sd sync` from this checkout.
- Do not create a broad commit, force-push, rewrite history, or edit CI/release
  workflow behavior from this dirty tree.
- Keep implementation slices tightly scoped and record evidence in Seeds before
  closing anything.

Current Seeds evidence:

- `sd stats --format json`: 261 total, 172 closed, 89 active (`63 open`,
  `26 in_progress`), 36 blocked.
- Active priority split from `sd list`: `29 P1`, `42 P2`, `17 P3`, `1 P4`.
- `sd ready --format json`: 31 ready items (`1 P1`, `14 P2`, `15 P3`, `1 P4`).
- `sd blocked --format json`: 36 blocked items (`15 P1`, `19 P2`, `2 P3`).

Current wave state:

- `audio-graph-94fc` is still `in_progress` and blocks the P1 configuration UX
  and credential health center epic (`audio-graph-1c2f`).
- The Cerebras slice has local implementation and verification evidence in the
  working tree, but it still needs a final review/Seed hygiene pass before
  closure.
- A review subagent for the Cerebras slice was shut down after timing out, so
  the main orchestrator must run the closeout review directly or spawn a fresh,
  tightly scoped reviewer.

Next queue bias:

- Finish or repair `audio-graph-94fc` before moving on, because it is a small
  active provider-readiness item and directly reduces the configuration epic's
  blocker set.
- Keep `audio-graph-2586` visible as the only ready P1, but treat workflow
  mutation/publish validation as approval-gated or clean-worktree work.
- After Cerebras closeout, re-read `sd ready` and prefer a non-CI P1/P2 item
  that unblocks credentials, source/audio foundation, provider harness,
  transcript/projection, or diarization.

## Continuation Snapshot - 2026-06-26T08:02:20-07:00

Closed `audio-graph-94fc` after the Cerebras provider-readiness wave. The
implementation keeps Cerebras as a first-class Settings/provider-registry
preset over the existing OpenAI-compatible `LlmProvider::Api` shape, with:

- Dedicated `cerebras_api_key` credential storage and redacted presence
  handling.
- Canonical `https://api.cerebras.ai/v1` endpoint routing in backend and
  frontend helpers.
- `llm.cerebras` provider-registry metadata, generated TypeScript registry
  output, fallback model catalog, health command, and model catalog command.
- Settings UI controls that show saved-key presence and model selection without
  plaintext saved-key loadback.
- Backend readiness/model discovery through the OpenAI-compatible `/models`
  path.
- Generic `llm.api` readiness preserved by routing required credentials from
  the active endpoint instead of requiring every OpenAI-compatible credential
  slot. Loopback OpenAI-compatible endpoints can remain unauthenticated.

Review loop result:

- A read-only Cerebras reviewer found one close-blocking issue: generic
  `llm.api` readiness fell back to all descriptor credential keys.
- The blocker was fixed in `src-tauri/src/commands.rs` and covered with focused
  tests.
- Two low review findings were also addressed in the same wave: fixed fallback
  catalogs for remote-command descriptors with explicit catalogs, and provider
  setup cards preserving a selected Cerebras model.

Verification evidence:

- `bun run test -- src/components/SettingsPage.test.tsx src/components/providerRegistryHelpers.test.ts src/components/providerSetupModes.test.ts src/generated/providerRegistry.test.ts src/App.test.tsx`
  passed: 5 files, 136 tests.
- `bun run typecheck` passed.
- `bun run check:provider-registry` passed.
- `bunx @biomejs/biome@2.5.1 check` on touched frontend files passed.
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph-provider-registry -- --nocapture`
  passed: 14 tests.
- Focused backend tests passed for endpoint routing, saved-key resolution,
  custom Cerebras default model parsing, LLM descriptor mapping, demo credential
  drift, generic LLM endpoint credential routing, loopback generic ASR
  credential behavior, and fixed fallback catalogs.
- `git diff --check` passed for touched Cerebras/frontend/doc/Seed files.
- `sd doctor --fix --json` repaired the post-close reciprocal dependency link
  and passed all 12 checks.
- `bun run check:seeds-json-output` passed.

Current Seeds evidence:

- `sd stats --format json`: 261 total, 173 closed, 88 active (`63 open`,
  `25 in_progress`), 36 blocked.
- `sd ready --format json`: 31 ready items (`1 P1`, `14 P2`, `15 P3`, `1 P4`).
- `sd blocked --format json`: 36 blocked items (`15 P1`, `19 P2`, `2 P3`).

Live-provider caveat:

- No live Cerebras API smoke was run because no API key was provided. Any live
  Cerebras smoke should be env-gated and must not log or persist secrets.

Next queue bias:

- `audio-graph-2586` remains the only ready P1, but workflow mutation/publish
  validation is still clean-worktree or explicit-approval work.
- The next practical non-CI wave should choose among ready P2s that unblock
  storage/memory (`audio-graph-2b2c`), trust/product architecture
  (`audio-graph-eeec`), provider setup UX (`audio-graph-0162`), projection
  smoke (`audio-graph-8e59`), or provider-registry codegen cleanup
  (`audio-graph-a805`).

Current loop focus:

- `audio-graph-75a1` remains in progress for time-to-first-note onboarding and
  sample session UX.
- The sample-preview review created child Seeds:
  - `audio-graph-b294`: sample session preview isolation and export boundaries.
  - `audio-graph-bab0`: allow one saved `openai_api_key` to satisfy runnable
    OpenAI-compatible ASR plus LLM onboarding readiness.
  - `audio-graph-4de9`: localize built-in sample session preview content.
- These child Seeds now block `audio-graph-75a1` until their acceptance criteria
  are verified.

Implementation assumptions for this wave:

- No external research is required for the selected slice; the known defects are
  local frontend state, export, i18n, and readiness-gating behavior.
- Keep fake sample data frontend-only unless the user explicitly saves it later.
- Tests must prove sample preview does not require backend persistence, capture,
  session-load, or graph-write commands.
- Keep ambiguous single-key provider combinations conservative; only the known
  OpenAI-compatible key path should satisfy both ASR and LLM readiness.

Current queue bias:

- Work `audio-graph-5633` first because it is the highest-priority non-CI
  ready item and directly unblocks the provider platform lane.
- Keep `audio-graph-2586` and other workflow/release Seeds approval-gated in
  this dirty checkout.
- Use local code evidence for this reconnect bug; no external research is
  needed unless provider protocol behavior itself becomes load-bearing.

## Continuation Snapshot - 2026-06-26T04:47:53-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree
is still broadly dirty across CI/release workflows, docs, Seeds, backend ASR
providers, audio/capture/playback, credentials/config, persistence/projection,
frontend Settings/status surfaces, generated provider files, and research/ADR
docs. Several files remain `MM`; do not run `sd sync`, create a broad commit,
force-push, rewrite history, or edit CI/release workflows from this checkout
unless the work is first isolated into a clean branch/worktree or explicitly
approved.

Current Seeds evidence:

- `sd stats --format json`: 255 total, 68 open, 23 in progress, 164 closed,
  39 blocked.
- Active Seeds from the ready/blocked pass: 91 active (`68 open`,
  `23 in_progress`).
- Active priority split: `30 P1`, `43 P2`, `17 P3`, `1 P4`.
- `sd ready --format json`: 33 ready items.
- `sd blocked --format json`: 39 blocked items.

Current non-CI implementation focus:

- Continue `audio-graph-d042`, the reusable ASR provider transport and parser
  fixture harness. Recent slices added parser fixtures, provider-native event
  fixtures, the shared reconnect ladder, and the shared test-only WebSocket
  fixture, but d042 remains open because successful reconnect lifecycle coverage
  and the production/session harness boundary are still incomplete.
- The next bounded wave is Deepgram successful reconnect coverage. Deepgram is
  the only current ASR provider that still calls `open_ws` directly from
  `session_task`, while AssemblyAI and OpenAI Realtime already have test-only
  reconnect opener seams. Add the smallest test-only seam needed for a
  deterministic successful-reconnect test; do not broaden production transport
  abstraction in this dirty checkout.

## Continuation Snapshot - 2026-06-26T06:21:21-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree
is still broadly dirty across CI/release workflows, docs, Seeds, backend
providers, audio/capture/playback, credentials/config, persistence/projection,
frontend Settings/status surfaces, generated provider files, and research/ADR
docs. Do not run `sd sync`, create a broad commit, force-push, rewrite history,
or edit CI/release workflows from this checkout unless the work is first
isolated into a clean branch/worktree or explicitly approved.

Current Seeds evidence:

- `sd stats --format json`: 258 total, 66 open, 24 in progress, 168 closed,
  36 blocked.
- `sd ready --format json`: 34 ready items.
- `sd blocked --format json`: 36 blocked items.

Current completed wave slice:

- Advanced `audio-graph-f0a3` with a verified backend AssemblyAI v3 runtime
  cutover. The live WebSocket now targets `wss://streaming.assemblyai.com/v3/ws`
  with `speech_model=universal-3-5-pro`, `sample_rate=16000`,
  `encoding=pcm_s16le`, optional `speaker_labels`, and saved-key
  `Authorization`. Audio frames are binary PCM16 LE, graceful shutdown sends the
  v3 `Terminate` control message, and v3 server messages are parsed in the
  speech receiver with source-aware `AssemblyAiV3Parser`.
- AssemblyAI v3 `Turn` revisions now flow through the shared
  `AsrSpanRevisionPayload` path and emit `turn-event` on end-of-turn.
  Unformatted final turns are downgraded to non-durable span revisions so the
  formatted final owns the transcript row, notes/graph projection, and live
  proposal side effects. `SpeakerRevision` now emits provider
  `diarization-span-revision` sideband events keyed by turn.
- Provider registry metadata now reports AssemblyAI as
  `universal-3-5-pro` + `web_socket_binary`, and generated TypeScript registry
  output is current.

Verification evidence for this wave:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph --lib --no-default-features --features cloud assemblyai -- --nocapture --test-threads=1`:
  27 passed.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`:
  passed.
- `rustfmt +1.95.0 --edition 2024 --check` on touched Rust files: passed.
- `bun run check:provider-registry`: passed.
- `bun run test -- src/generated/providerRegistry.test.ts src/components/SettingsPage.test.tsx src/components/providerSetupModes.test.ts`:
  112 passed.
- `bun run typecheck`: passed.
- `sd doctor --json`: 12 pass.

Remaining `audio-graph-f0a3` acceptance gaps:

- Run an env-gated real AssemblyAI v3 smoke with `ASSEMBLYAI_API_KEY` before
  claiming live provider readiness.
- Decide whether temporary-token support belongs in backend-owned AudioGraph or
  in a future browser-origin streaming mode.
- Add integration-level duplicate-final tests covering transcript buffer,
  projection scheduler, and agent proposal side effects.
- Persist or project provider diarization-span revisions beyond UI events once
  projection data models accept diarization bases.
- Update readiness copy to distinguish REST key validity from v3 streaming
  smoke evidence.

## Continuation Snapshot - 2026-06-26T08:16:12-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree
is still broadly dirty across CI/release workflows, docs, Seeds, backend
providers, audio/capture/playback, credentials/config, persistence/projection,
frontend Settings/status surfaces, generated provider files, and research/ADR
docs. Several files are still `MM`, so do not run `sd sync`, create a broad
commit, force-push, rewrite history, or touch CI/release workflows from this
checkout unless the work is isolated into a clean branch/worktree or explicitly
approved.

Current Seeds evidence:

- `sd stats --format json`: 267 total, 68 open, 25 in progress, 174 closed,
  36 blocked.
- Active Seeds: 93 (`68 open`, `25 in_progress`).
- Latest completed architecture slice: closed `audio-graph-eeec` after writing
  `docs/research/local-first-trust-model-2026-06-26.md` and creating the
  implementation follow-ups `audio-graph-70a3`, `audio-graph-51e0`,
  `audio-graph-d598`, `audio-graph-c282`, `audio-graph-baa6`, and
  `audio-graph-a32f`.
- Validation for the trust model slice: `git diff --check` on the new research
  doc and Seeds file, `sd doctor --json` with 12 passing checks, and
  `bun run check:seeds-json-output`.

Current queue bias:

- Work `audio-graph-d598` first because it is the highest-priority ready item
  that is not gated by CI/workflow approval. Its target is a backend-enforced
  local-only/cloud-disabled runtime policy so UI state is not the provider
  transfer boundary.
- Keep `audio-graph-2586` release publish verification ready but
  approval/clean-branch gated because it touches workflows and release
  behavior in a dirty checkout.
- Treat this as a bounded Template A deep-work-loop iteration: discover the
  current provider/settings/session seams, plan a minimal backend policy slice,
  implement with focused tests, then run Seeds and code gates before closing or
  recording remaining work.

## Continuation Snapshot - 2026-06-26T08:54:00-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree is
still broadly dirty, including CI/release workflow files, generated files,
backend provider/audio/projection/frontend surfaces, docs, and Seeds. Do not run
`sd sync` or make a broad commit from this checkout while unrelated staged and
unstaged work remains.

Completed since the previous snapshot:

- Closed `audio-graph-d598` after adding backend-enforced runtime privacy mode
  and session-content gates for ASR, LLM chat/notes/projection, cloud TTS
  speak-aloud, Gemini notes, and native converse.
- Added `privacy_mode` preservation through Settings and ExpressSetup without
  adding a visible UI control in this slice.
- Made LLM executor jobs policy-aware so restricted modes cannot silently fall
  back from local providers to OpenRouter/API cloud clients.
- Added follow-up `audio-graph-3b9f` for provider-internal socket/request egress
  guards as defense in depth.

Verification evidence for the runtime privacy slice:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`: passed.
- Focused Rust tests for `privacy_mode`,
  `provider_content_transfer_classification_treats_loopback_as_local`,
  `session_content_policy_blocks_cloud_but_not_loopback_or_local`, and
  `run_chat_restricted_policy_omits_cloud_attempts`: passed.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`:
  passed with the existing dead-code warning for
  `parse_openai_compatible_model_catalog`.
- `bun run typecheck`: passed.
- `bun run test -- src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/utils/errorToMessage.test.ts`:
  108 passed.
- `sd doctor --fix --json` repaired reciprocal dependency links; subsequent
  `sd doctor --json` passed all 12 checks.
- `bun run check:seeds-json-output` and `git diff --check -- .seeds/issues.jsonl`:
  passed.

Current Seeds evidence:

- `sd stats --format json`: 268 total, 68 open, 25 in progress, 175 closed,
  36 blocked.
- Active Seeds: 93 (`68 open`, `25 in_progress`).
- P1 active cut: 14 open P1 and 15 in-progress P1.
- `sd ready --format json`: 36 ready; the only ready P1 is
  `audio-graph-2586`, which remains clean-branch/CI-approval gated because it
  touches release workflows and publish behavior.

Next queue bias:

- Do not take `audio-graph-2586` from this dirty checkout without explicit
  workflow approval or a clean isolated worktree.
- Inspect the in-progress P1 lane and choose the highest-impact non-CI item that
  can be advanced with focused edits and tests.
- Candidate non-gated P1s include credential/keychain readiness
  (`audio-graph-0c08`, `audio-graph-cbde`), ASR provider platform
  (`audio-graph-d042`, `audio-graph-e35f`, `audio-graph-f0a3`), source/audio
  foundations (`audio-graph-afca`, `audio-graph-f53b`), and transcript/notes
  projection (`audio-graph-ad44`).

## Continuation Snapshot - 2026-06-26T09:16:00-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree is
still broadly dirty with unrelated parent-thread work, including CI/release
workflow changes, generated files, backend provider/audio/projection/frontend
surfaces, docs, and Seeds. Do not run `sd sync` or make a broad commit from this
checkout while unrelated changes remain.

Completed in this credentials/settings wave:

- Advanced `audio-graph-0c08` by making production credentials default to OS
  keychain without silent plaintext YAML fallback. Direct YAML/file backend and
  keychain-with-file-fallback are now explicit
  `AUDIO_GRAPH_CREDENTIAL_BACKEND` modes.
- Added an internal fakeable `KeychainStore` seam so keychain import/delete and
  tombstone behavior can be tested without prompting platform credential stores.
- Advanced `audio-graph-a3d8` by localizing `os_keychain`, `imported_file`,
  `file_fallback`, `credentials_yaml`, `missing`, `error`, and
  `keychain_unavailable` source labels in provider-local readiness and
  credential-health UI.
- Updated Settings save/clear flows to reload non-secret credential presence
  from Rust after credential writes instead of synthesizing a
  `credentials_yaml` source label in React.
- Updated README credential wording so production desktop builds use OS-native
  credential stores, while `credentials.yaml` is described as import/recovery
  and explicit headless/dev fallback.
- Updated Seeds `audio-graph-0c08`, `audio-graph-a3d8`, and
  `audio-graph-cbde` with verification and remaining work.

Verification evidence for this wave:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`: passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud credentials::tests -- --nocapture --test-threads=1`:
  23 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud credential_presence -- --nocapture --test-threads=1`:
  2 passed.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`:
  passed with the existing dead-code warning for
  `parse_openai_compatible_model_catalog`.
- `bun run test -- src/components/ProviderReadinessPanel.test.tsx src/components/SettingsPage.test.tsx src/components/providerRegistryHelpers.test.ts`:
  103 passed.
- `bun run typecheck`: passed.
- `bunx @biomejs/biome@2.5.1 check` on the touched Settings/readiness/i18n
  files: passed.
- `jq empty src/i18n/locales/en.json src/i18n/locales/pt.json`: passed.
- `git diff --check` on touched code/docs/Seeds files: passed.
- `sd doctor --json`: passed all 12 checks.
- `bun run check:seeds-json-output`: passed.

Remaining after this wave:

- `audio-graph-0c08` stays open for GitHub/Blacksmith Linux/macOS/Windows
  keychain compile/smoke evidence and for deciding whether
  `ProviderCredentialReadiness` itself should carry source/error metadata or the
  separate `CredentialPresence` path is final.
- `audio-graph-a3d8` stays open for broader docs refresh:
  `SETTINGS_DESIGN`, platform quickstarts, and related docs still need to
  describe `credentials.yaml` only as import/fallback/export/dev behavior.
- `audio-graph-cbde` stays open for typed voice/language catalogs, Gemini Vertex
  readiness policy/probe, and explicit cancellation of in-flight readiness calls.
- `audio-graph-2586` remains ready but clean-branch/CI-approval gated because it
  touches release workflows.

## Continuation Snapshot - 2026-06-26T09:34:00-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree is
still broadly dirty with unrelated parent-thread work. Do not run `sd sync`
from this checkout.

Completed in this provider-readiness/Soniox wave:

- Advanced `audio-graph-cbde` by adding typed non-secret readiness catalog slots:
  `voice_catalog` and `language_catalog`, alongside the existing
  `model_catalog` and `openrouter_models` payloads.
- Populated Deepgram Aura `voice_catalog` from the backend provider registry
  fixed Aura voice list. Existing `model_catalog` remains for compatibility, but
  Settings and provider-local readiness now prefer the typed voice catalog.
- Updated readiness/capability UI copy so Deepgram Aura reports voice catalogs
  as voices instead of models.
- Advanced `audio-graph-e35f` by sending `enable_endpoint_detection=true` in the
  Soniox realtime first config frame, matching the parser/runtime contract that
  relies on provider endpoint markers.
- Updated Seeds `audio-graph-cbde` and `audio-graph-e35f` with verification and
  remaining work.

Verification evidence for this wave:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`: passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_readiness -- --nocapture --test-threads=1`:
  3 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud base_readiness_exposes_deepgram_aura_voice_catalog -- --nocapture --test-threads=1`:
  1 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud soniox -- --nocapture --test-threads=1`:
  23 passed.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`:
  passed with the existing dead-code warning for
  `parse_openai_compatible_model_catalog`.
- `bun run test -- src/components/ProviderReadinessPanel.test.tsx src/components/SettingsPage.test.tsx src/components/providerRegistryHelpers.test.ts src/components/ModelCatalogPicker.test.tsx`:
  109 passed.
- `bun run typecheck`: passed.
- `bunx @biomejs/biome@2.5.1 check` on touched Settings/readiness/i18n files:
  passed.
- `bun run check:provider-registry`: provider registry is current.
- `sd doctor --json`: passed all 12 checks before Seed updates.
- `bun run check:seeds-json-output`: passed before Seed updates.
- `git diff --check` on touched slice files: passed.

Remaining after this wave:

- `audio-graph-cbde` stays open: `language_catalog` is modeled but no
  provider-specific language discovery is populated yet; Gemini Vertex readiness
  is still intentionally unchecked; readiness cancellation remains
  timeout/state-ignore based rather than explicit provider-call cancellation.
- `audio-graph-e35f` stays open: Soniox still needs fake-server reconnect
  lifecycle tests, env-gated live smoke with `SONIOX_API_KEY`, Settings controls,
  source-policy review, and registry promotion from planned to implemented.
- `audio-graph-2586` remains the only ready P1 and is still gated to a clean
  workflow-edit branch or explicit approval.

## Continuation Snapshot - 2026-06-26T09:42:05-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree is
still broadly dirty with unrelated parent-thread work. Do not run `sd sync`
from this checkout.

Completed in this Soniox reconnect wave:

- Added Soniox provider-local fake WebSocket reconnect-open failure coverage.
  The test proves failed reconnect attempts stay inside the reconnect ladder
  instead of re-entering `run_io` with stale closed socket halves.
- Added Soniox cancellation-during-backoff coverage inside that failure test:
  cancellation exits with `Disconnected`, leaves `connected=false`, starts no
  extra opener call, and emits no `Reconnected`.
- Added Soniox successful reconnect coverage. The test proves `Reconnecting ->
  Reconnected`, config-first replay on the fresh socket, transcript revision
  handling after reconnect, post-reconnect audio delivery, terminal empty binary
  frame on stop, pending chunk decrement, resumed `run_io`, and clean final
  `Disconnected`.
- Updated Seeds `audio-graph-e35f` and `audio-graph-d042` with verification and
  remaining work. Per review, this is Soniox provider/harness evidence, not a
  reason to close broader production transport extraction.

Verification evidence for this wave:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`: passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud soniox -- --nocapture --test-threads=1`:
  25 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud asr::reconnect -- --nocapture --test-threads=1`:
  2 passed.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`:
  passed with the existing dead-code warning for
  `parse_openai_compatible_model_catalog`.
- `git diff --check` on touched slice files: passed before Seed/doc update.

Remaining after this wave:

- `audio-graph-e35f` stays open for env-gated live Soniox smoke, source-policy
  review, Settings controls, registry promotion, and durable language/progress
  metadata decisions.
- `audio-graph-d042` stays open because the production/shared transport/session
  harness is still not extracted; provider modules still own
  `open_ws`/`run_io`/reconnect/init/terminal/close behavior.
- A full reconnect-exhaustion fake-server test is intentionally not added in
  this wave because current Tokio features lack `test-util` paused time and a
  real-time ladder test would add 18 seconds to the provider suite. The pure
  `asr::reconnect` unit tests continue to cover the give-up math.

## Continuation Snapshot - 2026-06-26T09:46:51-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree is
still broadly dirty with unrelated parent-thread work across CI, backend,
frontend, docs, and Seeds. Do not run `sd sync` from this checkout.

Selected wave:

- Advanced P1 Seed `audio-graph-cbde` rather than the ready workflow Seed
  `audio-graph-2586`, because workflow/CI edits remain approval-gated in this
  dirty checkout.
- Focused on the remaining explicit provider-readiness cancellation gap.

Completed in this readiness cancellation wave:

- Added request-scoped provider-readiness cancellation in Rust. Settings passes
  an opaque non-secret `requestId` to `get_provider_readiness_cmd`, and Rust
  registers a `CancellationToken` for that request.
- Added `cancel_provider_readiness_cmd`, wired through Tauri, so Settings
  cleanup can cancel the in-flight backend request instead of only ignoring late
  React state updates.
- Wrapped provider readiness probes in `tokio::select!` against cancellation
  and the existing timeout. Cancelled readiness returns unchecked with
  `checked_at=None`, so cancelled results are not written into the readiness
  cache.
- Added a drop guard for per-provider in-flight admission keys so cache-key
  coalescing state is cleaned up on success, timeout, cancellation, or dropped
  command futures.
- Updated Settings to cancel the current readiness request on Settings cleanup
  and cancel a previous active request before starting a replacement refresh.
- Added frontend coverage for request id propagation, unmount cancellation, and
  fresh request ids on manual rerun without calling `load_credential_cmd`.

Verification evidence for this wave:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_readiness -- --nocapture --test-threads=1`:
  7 passed.
- `bun run test -- src/components/SettingsPage.test.tsx`: 87 passed.
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`: passed.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`:
  passed with the existing dead-code warning for
  `parse_openai_compatible_model_catalog`.
- `bun run typecheck`: passed.
- `bunx @biomejs/biome@2.5.1 check src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx`:
  passed.

Remaining after this wave:

- `audio-graph-cbde` stays open because provider-specific `language_catalog`
  discovery is still not populated, Gemini Vertex readiness remains explicitly
  unchecked until a real probe or final no-probe policy is accepted, and broader
  cross-platform Settings/readiness validation still needs a clean branch or CI
  evidence.
- `audio-graph-1c2f` remains blocked by `cbde`, keychain/source-label work,
  redacted diagnostics, setup validation, and other configuration children.

## Continuation Snapshot - 2026-06-26T10:01:00-07:00

Current HEAD remains `831cc30101840db87bd2b502f2da749d65fe1c22` on `master`
(`fix: address 6 real CodeRabbit findings from the PR review`). The worktree is
still broadly dirty with unrelated parent-thread work. Do not run `sd sync`
from this checkout.

Selected wave:

- Advanced P1 Seed `audio-graph-e35f` with an env-gated live Soniox smoke
  scaffold.
- Skipped `audio-graph-a805` because its remaining acceptance is macOS/Windows
  remote validation or an approved workflow edit, not a local code slice.

External verification:

- Opened official Soniox docs on 2026-06-26. The docs list `stt-rt-v5` as an
  active real-time model, document `wss://stt-rt.soniox.com/transcribe-websocket`,
  show config with `"model": "stt-rt-v5"`, and describe binary audio streaming
  plus empty-frame finalization.
- Sources:
  - https://soniox.com/docs/api-reference/stt/websocket-api
  - https://soniox.com/docs/stt/models

Completed in this Soniox live-smoke scaffold wave:

- Added ignored test
  `live_smoke_soniox_websocket_accepts_config_audio_and_finish` in
  `src-tauri/src/asr/soniox.rs`.
- The test requires `SONIOX_API_KEY` and live network access, sends the normal
  config-first Soniox frame, streams one second of generated PCM16 silence,
  sends the terminal empty binary frame, parses provider responses through the
  existing Soniox parser, fails provider errors through the existing redaction
  path, and asserts a finished response.
- Default local/CI test runs do not need Soniox credentials and do not hit the
  network.
- Refreshed the Soniox module comment so it no longer says the module is
  parser-only now that provider-local live transport exists.

Verification evidence for this wave:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud soniox -- --nocapture --test-threads=1`:
  25 passed, 1 ignored.
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`: passed.
- `git diff --check -- src-tauri/src/asr/soniox.rs .seeds/issues.jsonl docs/commit-state-2026-06-26-backlog-zero-continuation.md`:
  passed before the final Seed/doc update.

Not run:

- The ignored live smoke itself was not executed because `SONIOX_API_KEY` is not
  available in the environment and this checkout should not require secrets for
  default verification.

Remaining after this wave:

- `audio-graph-e35f` stays open until the ignored live smoke is actually run
  against Soniox with `SONIOX_API_KEY`, source-policy review is complete,
  Settings controls are exposed safely, and `asr.soniox` is promoted from
  planned to implemented.
- `audio-graph-d042` still owns broader shared WebSocket provider harness
  extraction.
## Continuation Snapshot - 2026-06-26T10:19:07-07:00

### audio-graph-baa6 provider data-boundary metadata

- Added unknown-aware provider privacy/data-boundary metadata in `src-tauri/crates/provider-registry/src/lib.rs`: sent/returned/health-check data classes, cloud-transfer acknowledgement, retention/training/deletion status, optional policy URL, enterprise no-training status, data residency status, sensitive-error policy, and optional processor identity.
- Added per-stage privacy presets so ASR, LLM, TTS, and realtime-agent surfaces do not all collapse to one generic cloud boundary.
- Kept provider-specific retention/training/deletion facts as `unknown` unless sourced; added a follow-up Seed, `audio-graph-fee1`, for source-backed provider policy URL and processor matrix enrichment.
- Updated generated provider registry output, TypeScript types, Settings readiness panel display, i18n labels, and focused frontend tests.
- Closed `audio-graph-baa6` after verification.

Verification:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph-provider-registry --lib -- --nocapture` -> 15 passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_registry -- --nocapture --test-threads=1` -> 9 passed
- `bun run check:provider-registry`
- `bun run test -- src/generated/providerRegistry.test.ts src/components/ProviderReadinessPanel.test.tsx src/components/providerRegistryHelpers.test.ts src/components/providerSetupModes.test.ts` -> 48 passed
- `bun run typecheck`
- `bunx @biomejs/biome@2.5.1 check src/generated/providerRegistry.test.ts src/components/ProviderReadinessPanel.tsx src/components/ProviderReadinessPanel.test.tsx src/types/index.ts src/i18n/locales/en.json src/i18n/locales/pt.json`
- `jq empty src/i18n/locales/en.json src/i18n/locales/pt.json`
- `git diff --check -- src-tauri/crates/provider-registry/src/lib.rs src/generated/providerRegistry.ts src/generated/providerRegistry.test.ts src/types/index.ts src/components/ProviderReadinessPanel.tsx src/components/ProviderReadinessPanel.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json .seeds/issues.jsonl docs/commit-state-2026-06-26-backlog-zero-continuation.md`

## Continuation Snapshot - 2026-06-26T10:22:13-07:00

### audio-graph-c9ec AssemblyAI diarization caveat closeout

- Verified the current AssemblyAI integration is now v3 Universal-3.5 Pro Realtime rather than the old v2 JSON/base64 path.
- Evidence in current code: `wss://streaming.assemblyai.com/v3/ws`, `default_model=universal-3-5-pro`, binary `pcm_s16le` audio, `speaker_labels=true` when diarization is enabled, `Turn`/`SpeakerRevision` parser support, and `DiarizationSpanRevisionPayload` sideband emission from `SpeakerRevision`.
- Closed `audio-graph-c9ec`; remaining real-key live smoke/readiness wording work stays in `audio-graph-f0a3`.

Verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud assemblyai -- --nocapture --test-threads=1` -> 27 passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud asr::fixtures::assemblyai -- --nocapture --test-threads=1` -> 2 passed
- `bun run test -- src/generated/providerRegistry.test.ts` -> 16 passed

## Continuation Snapshot - 2026-06-26T10:29:23-07:00

### audio-graph-f0a3 AssemblyAI v3 live-smoke scaffold

- Added ignored live smoke
  `live_smoke_assemblyai_v3_websocket_accepts_binary_audio_and_terminates`
  in `src-tauri/src/asr/assemblyai.rs`.
- The test requires `ASSEMBLYAI_API_KEY` and live AssemblyAI v3 network access.
  It opens the production v3 WebSocket through `open_ws`, verifies `Begin`
  plus requested model echo, sends generated PCM16 silence as a binary frame,
  sends the v3 `{ "type": "Terminate" }` control message, keeps reading until
  `Termination`, and redacts provider diagnostics on every failure path.
- Default local/CI test runs do not require AssemblyAI credentials and do not
  hit the network because the smoke is `#[ignore]`.
- A read-only explorer confirmed there was no prior env-gated AssemblyAI live
  smoke and recommended adding it beside the existing fake-server AssemblyAI
  runtime tests.

Verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud assemblyai -- --nocapture --test-threads=1` -> 27 passed, 1 ignored
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed

Not run:

- The ignored live smoke itself was not executed because `ASSEMBLYAI_API_KEY`
  is not available in this environment and default verification must not
  require secrets.

Remaining after this wave:

- `audio-graph-f0a3` stays open until the ignored live smoke runs with a real
  `ASSEMBLYAI_API_KEY`, readiness copy distinguishes REST key validity from v3
  WebSocket smoke evidence, temporary-token/browser-origin scope is decided,
  partial/final side-effect dedup integration coverage is added, and provider
  diarization-span revisions have durable projection semantics where needed.

## Continuation Snapshot - 2026-06-26T10:39:25-07:00

### audio-graph-f0a3 AssemblyAI readiness copy caveat

- Updated `test_assemblyai_connection` in `src-tauri/src/commands.rs` so the
  readiness/test message says AssemblyAI account-key validity was checked
  through REST and that the v3 streaming socket smoke was not run.
- Updated `src/components/SettingsPage.test.tsx` so Settings readiness fixtures
  assert the caveat alongside the fixed Universal-3.5 Pro model catalog.
- This prevents the Settings provider readiness row from implying that the
  REST `/v2/transcript` account-key probe proves the v3 WebSocket transport is
  live-ready.
- Filed `audio-graph-6e1b` for the unrelated `parse_openai_compatible_model_catalog`
  dead-code warning surfaced by `cargo check`.

Verification:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed
- `bunx @biomejs/biome@2.5.1 check src/components/SettingsPage.test.tsx` -> passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud assemblyai_api_key_resolution -- --nocapture` -> 3 passed
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` -> passed with tracked warning `audio-graph-6e1b`
- `bun run test -- src/components/SettingsPage.test.tsx` -> 87 passed
- `bun run typecheck` -> passed

## Continuation Snapshot - 2026-06-26T10:44:00-07:00

### audio-graph-6e1b dead parser-helper warning

- Closed the warning Seed created during the AssemblyAI readiness work.
- Removed the unused `parse_openai_compatible_model_catalog` wrapper in
  `src-tauri/src/commands.rs`.
- Updated the parser test to call
  `parse_openai_compatible_model_catalog_with_default(..., Some("whisper-1"))`
  directly, matching the production fetch path.

Verification:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud openai_compatible_model_catalog -- --nocapture` -> 2 passed
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` -> passed with no warning output

## Continuation Snapshot - 2026-06-26T10:50:20-07:00

### audio-graph-f0a3 AssemblyAI unformatted-final side-effect dedup

- Fixed the AssemblyAI v3 unformatted-final path in `src-tauri/src/speech/mod.rs`.
- Extracted `normalize_assemblyai_v3_revision_for_side_effects` and made it
  clear `end_of_turn` when AssemblyAI emits an unformatted final turn.
- The extra `end_of_turn` clearing matters because the projection scheduler
  treats either finality or end-of-turn as eligible work. Without this, an
  unformatted AssemblyAI final could still start notes/graph projection before
  the formatted final, even though it no longer appended a transcript row.
- Added `assemblyai_unformatted_final_waits_for_formatted_final_side_effects`
  in `src-tauri/src/speech/tests_integration.rs`. It feeds partial,
  unformatted final, and formatted final revisions through the speech tail and
  proves only the formatted final appends a transcript row, starts one
  notes/graph projection each, and creates exactly one live-assist proposal.
- A read-only explorer independently confirmed this belongs in the speech
  integration layer and that clearing `end_of_turn` is required.

Verification:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud assemblyai_unformatted_final_waits_for_formatted_final_side_effects -- --nocapture --test-threads=1` -> 1 passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud assemblyai_v3_partial_final_revision_fixture_replays_through_ledger -- --nocapture --test-threads=1` -> 1 passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud runtime_projection_scheduler_observes_finals_without_partial_job_churn -- --nocapture --test-threads=1` -> 1 passed
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` -> passed

## Continuation Snapshot - 2026-06-26T11:12:25-07:00

### Wave 0 safe parallelization map

- HEAD remains `831cc30 fix: address 6 real CodeRabbit findings from the PR review`
  on `master`.
- The working tree is broadly dirty and includes staged plus unstaged overlap
  in many shared files. Do not use this checkout for broad commits, `sd sync`,
  workflow edits, generated-output refreshes, or parallel writes to global
  files without explicit ownership.
- Integrator-only/shared surfaces for now: `.seeds/issues.jsonl`,
  `.github/**`, `src-tauri/Cargo.toml`, `package.json`, lockfiles,
  `src-tauri/src/{commands.rs,state.rs,events.rs,lib.rs,settings/mod.rs}`,
  `src-tauri/src/speech/mod.rs`, generated files, store/types/i18n, and the
  main Settings components.
- Safer parallel lanes should use disjoint ownership:
  credential-specific backend/docs, individual provider files and fixtures,
  projection-specific modules, diarization/source-separation fixtures/docs,
  and new product/research docs. Subagents should return Seed proposals unless
  granted a narrow write set.
- Queue audit found no corrupt DAG: `sd doctor --json` passed 12 checks. It
  did create follow-up Seeds for stale closed blockers, stale `in_progress`
  triage, and duplicate-title closure hygiene: `audio-graph-f18c`,
  `audio-graph-1e4b`, and `audio-graph-d760`.

### audio-graph-3b9f ASR provider-client egress guards

- Added `ProviderContentEgressPolicy` in `src-tauri/src/asr/mod.rs`.
- Runtime privacy mode is now translated into that policy in
  `src-tauri/src/commands.rs` and carried by `SpeechConfig`.
- Threaded the policy into Deepgram, AssemblyAI, Soniox, and OpenAI Realtime
  transcription client configs.
- Each ASR client now rejects non-empty `send_audio` calls under blocked
  policy before channel initialization, conversion, base64 encoding, or
  queueing. Empty audio remains a no-op.
- Added direct provider-client tests proving blocked ASR audio egress errors
  mention the provider/mode, do not fall through to "Audio channel not
  initialized", and do not leak API keys, sample values, or transcript-like
  sentinel text.
- `audio-graph-3b9f` remains open for LLM blocking/streaming, projection and
  extraction prompt egress, Deepgram Aura TTS, Gemini Live, native S2S, and
  no-content readiness probe coverage.

Verification:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud blocked_policy -- --nocapture --test-threads=1` -> 12 passed
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` -> passed
- `sd doctor --json` -> 12 passed
- `bun run check:seeds-json-output` -> passed
- `jq empty .seeds/issues.jsonl` -> passed
- `git diff --check` on the ASR/privacy and Seeds files -> passed

### Wave 1A credential/settings audit

- Recorded the credential/settings saved-key audit on `audio-graph-a3d8` and
  `audio-graph-0c08`.
- Created and linked credential hardening follow-ups under `audio-graph-1c2f`:
  `audio-graph-9d0e` for the plaintext credential load helper,
  `audio-graph-de28` for a docs/Seeds secret hygiene scanner, and
  `audio-graph-25d9` for Gemini Vertex saved-credential readiness probe design.

### Wave 1B credential helper and docs/Seeds scanner

- Closed `audio-graph-9d0e`.
- Deleted the plaintext-returning `load_credential_cmd` helper from
  `src-tauri/src/commands.rs`.
- Kept the intended non-secret `load_credential_presence_cmd` path for Settings
  and provider readiness.
- Replaced the helper's old loadback tests with
  `plaintext_credential_loadback_is_not_registered_for_ipc`, which reads the
  Tauri command registration source and proves plaintext credential loadback is
  not registered while non-secret presence is.
- Added `scripts/check-docs-secret-hygiene.mjs` for `audio-graph-de28`. The
  scanner has a fixture self-test, masks secret-like snippets, and currently
  fails against the existing docs/Seeds baseline. `audio-graph-de28` stays open
  until those findings are cleaned or narrowly exempted.
- A read-only non-ASR privacy planner confirmed the remaining `audio-graph-3b9f`
  sequence: add text/prompt/json policy methods, then split LLM blocking,
  LLM streaming, TTS, and Gemini provider-client guards by file ownership before
  integrator-owned production wiring through shared command/state files.

Verification:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud plaintext_credential_loadback_is_not_registered_for_ipc -- --nocapture --test-threads=1` -> 1 passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud credential_presence -- --nocapture --test-threads=1` -> 2 passed
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` -> passed
- `node --check scripts/check-docs-secret-hygiene.mjs` -> passed
- `node scripts/check-docs-secret-hygiene.mjs --fixture-self-test` -> passed
- `bun scripts/check-docs-secret-hygiene.mjs --fixture-self-test` -> passed
- `node scripts/check-docs-secret-hygiene.mjs` -> failed as expected on current baseline findings with redacted snippets

### Wave 1C non-ASR provider-client egress guards

- Integrated bounded subagent slices for Deepgram Aura TTS, Gemini Live audio,
  and streaming LLM request egress.
- Added blocking LLM guards in `ApiClient::chat_completion_inner` and
  `OpenRouterClient::chat_completion_with_usage` before prompt/request
  construction.
- Added streaming LLM guard support through `StreamChatRequest`, blocking
  cloud Api/OpenRouter prompt/json egress before request body construction or
  network connection.
- Added Gemini Live non-empty audio egress checks before channel access,
  PCM/base64 conversion, or queueing.
- Added Deepgram Aura TTS text egress checks before `Speak` command queueing.
- Updated `audio-graph-3b9f` with the verified non-ASR slice and kept it open
  for production wiring, projection/extraction path tests, native S2S/converse
  audit, and future embedding/org-sync guards.

Verification:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud blocked_policy -- --nocapture --test-threads=1` -> 18 passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud blocked_content_egress_prevents_cloud_stream_request_send -- --nocapture --test-threads=1` -> 1 passed
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` -> passed warning-clean

### Sanitized live provider smoke

- Temporary live provider keys were used only through in-memory probes. No key
  material was written to repo files, config files, docs, or Seeds.
- AssemblyAI REST account-key probe returned HTTP 200. This is REST-only
  evidence and does not prove v3 WebSocket streaming readiness; `audio-graph-f0a3`
  remains open for the ignored v3 WebSocket live smoke.
- Deepgram `/v1/models` returned HTTP 200 and 408 catalog entries.
- OpenRouter `/models` returned HTTP 200 and the requested candidate models
  were present.
- OpenRouter `/chat/completions`, `/responses`, and `/messages` returned HTTP
  200 with `nvidia/nemotron-3-nano-30b-a3b:free`.
- Created `audio-graph-84f4` for OpenRouter accelerator routing and API-surface
  compatibility so provider-routing work for Cerebras/Groq/SambaNova-style
  accelerators does not stay chat-only.

Active read-only subagents launched after this slice:

- Privacy coverage auditor: audit remaining provider-client/session-content
  egress gaps and production wiring gaps under `audio-graph-3b9f`.
- OpenRouter provider architect: propose provider-routing/model-catalog/API
  surface architecture for accelerator-backed OpenRouter usage.
- Settings/credentials UX auditor: review saved-key readiness/model discovery
  and credential-source display gaps.
- Privacy patch code reviewer: review the current Wave 1 privacy patch set for
  regressions, default-allow production gaps, and missing tests.

### Wave 1D endpoint-aware egress policy

- Closed `audio-graph-131a`.
- Added `ProviderContentEgressPolicy::from_privacy_mode_and_transfer_requirement`.
- Threaded endpoint/provider transfer classification into production policy
  construction so LocalOnly can still allow loopback/local OpenAI-compatible
  LLM and generic ASR API endpoints while remote endpoints remain blocked.
- Updated speech ASR config to use `AsrProvider::requires_cloud_content_transfer`.
- Updated blocking and streaming LLM policy construction to use
  `LlmProvider::requires_cloud_content_transfer`.
- Updated speak-aloud policy construction to use
  `TtsProvider::requires_cloud_content_transfer`.
- Kept Gemini Live configs cloud-only for this policy because Gemini Live is a
  vendor-cloud native S2S path.
- Removed the closed child dependency from `audio-graph-3b9f` after `sd doctor`
  found the expected stale reciprocal link.

Verification:

- `rustfmt --edition 2024 src-tauri/src/asr/mod.rs src-tauri/src/commands.rs src-tauri/src/asr/cloud.rs` -> passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud endpoint_aware_content_egress_policy -- --nocapture --test-threads=1` -> 2 passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_content_egress_policy_allows_local_transfer_requirement -- --nocapture --test-threads=1` -> 1 passed
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed
- `git diff --check -- src-tauri/src/asr/mod.rs src-tauri/src/commands.rs src-tauri/src/asr/cloud.rs .seeds/issues.jsonl` -> passed
- `sd doctor --json` -> 12 passed
- `bun run check:seeds-json-output` -> ready/blocked/list parsed
- `jq empty .seeds/issues.jsonl` -> passed

Notes:

- A duplicate broad `cargo check` was stopped because parallel worker Rust jobs
  held the shared Cargo build lock. This slice has targeted policy tests and
  the broader cloud check had already passed before the endpoint-aware edit.
- The temporary live provider values should still be rotated after use. They
  were not written to repo files, but one PTY-based probe echoed stdin in local
  terminal output before the probe was switched back to sanitized evidence.

### Wave 1E fail-closed settings and Cloud/AWS ASR guard follow-through

- Advanced `audio-graph-a4b6`.
- Added `read_settings_for_session_content` in `src-tauri/src/commands.rs` and
  switched content-bearing command paths to fail closed when privacy settings
  cannot be read.
- Covered ASR start, streaming chat, blocking chat streaming shim, notes
  synthesis, Gemini Live notes mode, and native S2S converse.
- Updated `spawn_stream_task` to receive the already-read `AppSettings`
  snapshot so stream request policy and speak-aloud TTS policy do not re-read
  settings with a permissive default.
- Advanced `audio-graph-e604` with the LLM extraction parse-error redaction
  slice. API/OpenRouter entity extraction parse failures no longer return raw
  provider output.
- Advanced `audio-graph-8c0d` with low-level generic Cloud ASR and AWS
  Transcribe content-egress guards.
- The Cloud/AWS ASR low-level guard Seed remains open until the production
  `speech/mod.rs` runtime paths pass `SpeechConfig.provider_content_egress_policy`
  into the guarded configs. Worker Boyle owns that single-file follow-up.
- Updated `audio-graph-84f4` from a read-only OpenRouter routing architecture
  pass: keep OpenRouter as one adapter, model Cerebras/Groq/SambaNova as dynamic
  routing targets/endpoints, add a typed Rust/TS routing policy, add providers
  and endpoint catalog commands, and keep Chat Completions as the default API
  surface while treating Responses/Messages as opt-in follow-ups.

Verification:

- `rustfmt --edition 2024 --check src-tauri/src/commands.rs` -> passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud read_settings_for_session_content_fails_closed_on_poisoned_lock -- --nocapture --test-threads=1` -> 1 passed
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud extract_entities_redacts_provider_output_on_parse_failure -- --nocapture --test-threads=1` -> 2 passed

Active subagents after this slice:

- Settings/credentials UX auditor: still running.
- Boyle: production Cloud/AWS ASR content-egress wiring in
  `src-tauri/src/speech/mod.rs` only.

### Wave 1F subagent audit reconciliation and next implementation fan-out

- Closed `audio-graph-8c0d` after low-level Cloud/AWS ASR guards and production
  `SpeechConfig.provider_content_egress_policy` wiring were verified.
- Lagrange completed the Settings/credentials UX audit. It confirmed plaintext
  credential loadback is no longer registered and Settings-open saved-key
  readiness/model discovery is wired, then identified two follow-ups:
  frontend tests still tolerated `load_credential_cmd` mocks, and readiness
  source labels can fall back to stale `credentials_yaml` when presence and
  readiness payloads race.
- Created `audio-graph-dba3` for frontend test hardening against plaintext
  credential IPC reintroduction.
- Updated `audio-graph-a3d8` with the readiness-source payload gap and raised
  it to P1.
- Updated `audio-graph-cbde` with provider-readiness/model-catalog gaps for
  Gladia, Speechmatics, RevAI, ElevenLabs, Google, and Azure.
- Lovelace completed a read-only privacy review. It found no remaining focused
  command-level fail-open settings read for transcribe, chat/streaming, notes
  synthesis, Gemini Live notes, native S2S, or speak-aloud. Remaining risk is
  low-level default-allow provider construction plus raw diagnostics/logs.
- Updated `audio-graph-bf74` with the LLM default-allow/projection-eval finding.
- Updated `audio-graph-25aa` with the Aura/Gemini default-allow constructor
  finding.
- Updated `audio-graph-e604` with the runtime ASR transcript logs, LLM raw
  HTTP/SSE output, WebSocket diagnostics, and raw provider-event findings.
- Created `audio-graph-7db3` for explicit content-egress policy construction
  across provider clients.

Verification:

- `sd doctor --json` -> 12 passed
- `bun run check:seeds-json-output` -> ready/blocked/list parsed
- `git diff --check -- .seeds/issues.jsonl docs/commit-state-2026-06-26-backlog-zero-continuation.md` -> passed

Active subagents after this slice:

- Ampere: frontend plaintext credential IPC test hardening in
  `src/App.test.tsx`, `src/components/SettingsPage.test.tsx`, and
  `src/components/ExpressSetup.test.tsx`.
- Darwin: Aura TTS and Gemini Live production policy/default-allow verification.
- Sartre: LLM default-blocking policy slice in `src-tauri/src/llm/*` and
  `src-tauri/src/projection_eval.rs`.

### Wave 1G privacy blocker closures

- Closed `audio-graph-dba3`: frontend tests now hard-fail if
  `load_credential_cmd` is invoked, rather than returning `null` for removed
  plaintext saved-key loadback.
- Closed `audio-graph-25aa`: production Aura TTS and Gemini Live paths already
  receive runtime `ProviderContentEgressPolicy`; added a focused test proving
  blocked privacy modes block cloud TTS text and Gemini Live audio while
  no-content probes remain allowed.
- Closed `audio-graph-bf74`: LLM provider clients and streaming requests now
  default-block with `explicit_policy_required` unless an explicit runtime
  policy is supplied. Streaming request construction enforces content-egress
  policy before building cloud request bodies, and projection eval explicitly
  opts into allow for its env-gated smoke path.
- Closed `audio-graph-a4b6`: after follow-up review, no remaining focused
  command-level fail-open settings read was found for transcribe,
  chat/streaming, notes synthesis, Gemini Live notes, native S2S converse, or
  speak-aloud.
- Updated `audio-graph-7db3` with the completed LLM explicit-policy slice; it
  remains open for non-LLM provider-client constructor defaults.

Verification:

- `sd doctor --json` -> 12 passed
- `git diff --check -- .seeds/issues.jsonl docs/commit-state-2026-06-26-backlog-zero-continuation.md src-tauri/src/commands.rs src-tauri/src/llm/api_client.rs src-tauri/src/llm/openrouter.rs src-tauri/src/llm/stream_contract.rs src-tauri/src/llm/streaming.rs src-tauri/src/projection_eval.rs src/App.test.tsx src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx` -> passed

Remaining `audio-graph-3b9f` blockers:

- `audio-graph-7db3`: explicit content-egress policy for non-LLM provider
  construction.
- `audio-graph-e604`: remove remaining session content/raw provider output from
  logs, errors, and provider events.
- `audio-graph-03ec`: future-provider content-egress checklist for S2S and
  candidate providers.

### Wave 1H provider privacy guard closeout

- Closed `audio-graph-03ec`, `audio-graph-7db3`, `audio-graph-e604`, and parent
  `audio-graph-3b9f`.
- Added the provider addition content-egress checklist to the provider
  architecture docs and a matching `AGENTS.md` guardrail.
- Added provider-registry tests enforcing runtime/readiness data separation and
  keeping future content-egress providers non-`Implemented` until blocked-policy
  harnesses exist.
- Changed `ProviderContentEgressPolicy::default()` and the OpenAI Realtime
  default config to block with `explicit_policy_required`.
- Replaced remaining runtime diagnostic leaks with metadata-only diagnostics
  across Cloud ASR, AWS Transcribe, speech pipeline logs, TTS/Aura HTTP errors,
  AssemblyAI tests, OpenAI Realtime errors/close diagnostics, and
  `StreamChatRequest` debug formatting.

Verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud metadata_only -- --nocapture --test-threads=1` -> 9 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud handle_error_message -- --nocapture --test-threads=1` -> 1 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud stream_chat_request_debug_reports_shape_without_prompt_or_graph_content -- --nocapture --test-threads=1` -> 1 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud speech_error_diagnostic -- --nocapture --test-threads=1` -> 1 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_content_egress_policy -- --nocapture --test-threads=1` -> 3 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud aws_transcribe -- --nocapture --test-threads=1` -> 12 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud openai_realtime -- --nocapture --test-threads=1` -> 47 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features asr-whisper new_worker_starts_with_zero_segments_processed -- --nocapture --test-threads=1` -> 1 passed.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` -> passed.
- `cargo +1.95.0 test -p audio-graph-provider-registry` -> 17 passed.
- `bun scripts/check-docs-secret-hygiene.mjs --fixture-self-test` -> passed.
- `bun scripts/check-docs-secret-hygiene.mjs` -> failed with 29 existing
  baseline credential-presence findings; recorded under `audio-graph-de28`.
- `bun run check:seeds-json-output` -> ready 40, blocked 36, list 50 parsed for
  both the repo-pinned and global Seeds CLI installs.
- `sd doctor --json` -> 12 passed.
- `git diff --check -- <Wave 1H files>` -> passed.

`sd sync` was not run because this checkout remains broadly dirty.

### Wave 1I backlog-zero continuation frame

Current commit and worktree state:

- Branch: `master`.
- HEAD: `831cc30101840db87bd2b502f2da749d65fe1c22`
  (`fix: address 6 real CodeRabbit findings from the PR review`).
- The checkout is an integration worktree with broad modified and untracked
  files across CI/release workflows, docs, Seeds, backend Rust, frontend React,
  generated provider files, fixtures, and scripts. Continue to avoid broad
  commits, `sd sync`, history rewrites, or workflow edits from this dirty
  checkout.

Queue evidence:

- `.seeds/issues.jsonl`: 286 total Seeds, 189 closed, 97 active
  (`72 open`, `25 in_progress`).
- Active priority split: 30 P1, 44 P2, 21 P3, 2 P4.
- `bun run sd:issues -- ready`: 40 ready
  (`1 P1`, `18 P2`, `19 P3`, `2 P4`).
- `bun run sd:issues -- blocked`: 36 blocked
  (`16 P1`, `18 P2`, `2 P3`).
- The only ready P1 is `audio-graph-2586`, but it touches release workflow
  behavior. Keep it approval-gated in this dirty checkout per project rules and
  user constraints.

Active parallel workers:

- Plato: `audio-graph-de28` docs/Seeds secret hygiene scanner baseline cleanup.
- Euler: read-only `audio-graph-84f4` OpenRouter routing implementation-slice
  audit.
- Locke: read-only queue/DAG/stale in-progress audit.

Main-thread wave choice:

- Advance `audio-graph-de28` first because it is ready, non-CI, privacy
  blocking, and directly blocks the configuration UX epic.
- Prepare the next non-CI wave from Euler/Locke results rather than editing the
  workflow/release lane without approval.

### Wave 1I scanner and queue hygiene closeout

Closed Seeds:

- `audio-graph-de28`: docs/Seeds secret hygiene scanner baseline is now clean.
  The scanner still flags provider-shaped key patterns and affirmative
  live/local credential-state claims, but fixture and baseline scans pass
  without printing secret values.
- `audio-graph-f18c`: removed all active `blockedBy` edges pointing at closed
  Seeds.
- `audio-graph-1e4b`: moved stale `in_progress` Seeds back to `open` when the
  next action is partial, blocked, or pending remote evidence rather than
  actively owned.

Queue effects:

- Ready count changed from 40 before the scanner closeout to 48 after queue
  triage.
- Blocked count is 37 after queue triage.
- Remaining `in_progress` Seeds are current active lanes:
  `audio-graph-e35f`, `audio-graph-2044`, `audio-graph-cbde`,
  `audio-graph-ad44`, `audio-graph-afca`, `audio-graph-f0a3`,
  `audio-graph-d042`, `audio-graph-0c08`, `audio-graph-75a1`,
  `audio-graph-ad98`, `audio-graph-bfcb`, and `audio-graph-c237`.

Verification:

- `bun scripts/check-docs-secret-hygiene.mjs --fixture-self-test` -> passed.
- `bun scripts/check-docs-secret-hygiene.mjs` -> passed with 0 findings.
- Custom JSONL scan -> no active Seed has `blockedBy` entries pointing at
  closed Seeds.
- `sd doctor --json` -> 12 passed.
- `bun run check:seeds-json-output` -> ready/blocked/list parsed after scanner
  closeout.
- `git diff --check` on scanner/docs/Seeds/commit-state files -> passed before
  final queue-triage doc update.

Next active worker:

- Bacon: `audio-graph-d157` OpenRouter routing policy serializer parity in
  `src-tauri/src/llm/openrouter.rs` and `src-tauri/src/llm/streaming.rs`.

### Wave 1J OpenRouter serializer split

- Created child `audio-graph-d157` under `audio-graph-84f4` for a backend-only
  serializer parity slice.
- Implemented typed OpenRouter routing policy serialization shared by blocking
  and streaming request builders.
- Preserved legacy `provider_order` compatibility: if configured alone, it
  still serializes as `provider.order`.
- Empty routing now omits the `provider` object.
- Tests cover strict routing, privacy/ZDR, quantization, performance
  sort/preference shapes, max-price serialization, and blocking/streaming
  parity.
- Closed `audio-graph-d157`.
- Split the remaining `audio-graph-84f4` work into child Seeds:
  `audio-graph-b652` for provider/endpoint catalog commands,
  `audio-graph-a641` for Settings routing presets/advanced controls, and
  `audio-graph-70cf` for Responses/Messages API-surface decision. The parent
  remains open and blocked by those children.

Verification:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud llm::openrouter::tests -- --nocapture --test-threads=1` -> 20 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud llm::streaming::tests -- --nocapture --test-threads=1` -> 16 passed.
- `git diff --check -- src-tauri/src/llm/openrouter.rs src-tauri/src/llm/streaming.rs` -> passed.
- `sd doctor --json` -> 12 passed.
- `bun run check:seeds-json-output` -> ready 49, blocked 37, list parsed.

Next active workers:

- Pasteur: `audio-graph-f53b` playback resampling test-only hardening in
  `src-tauri/src/playback/tests.rs`.
- Sagan: `audio-graph-0117` Moonshine speech-processor helper/test slice in
  `src-tauri/src/speech/mod.rs` and, if needed, `src-tauri/src/asr/moonshine.rs`.

### Wave 1K playback and Moonshine runtime advancement

- Advanced `audio-graph-f53b` with test-only playback resampling hardening.
  Production playback code was not changed. The resampler reset test now covers
  both 24 kHz and 16 kHz sources, and AudioPlayer cancel/resume coverage proves
  partial 16 kHz rubato state is discarded before later flush/playback.
- Kept `audio-graph-f53b` open because closure still requires Linux/macOS/Windows
  Blacksmith playback-resampling evidence.
- Advanced `audio-graph-0117` with a testable Moonshine speech processor helper
  that consumes processed 16 kHz mono chunks, calls the fakeable streaming
  worker, polls pending updates on timeout, emits revisions through
  `emit_moonshine_span_revision`, and updates Moonshine latency/status.
- Kept `audio-graph-0117` open because native C API adapter loading, production
  runtime branch replacement, and cross-platform/provider-readiness evidence
  remain.

Verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud playback -- --nocapture --test-threads=1` -> 16 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud flushes_playback -- --nocapture --test-threads=1` -> 2 passed.
- `git diff --check -- src-tauri/src/playback/tests.rs` -> passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud moonshine -- --nocapture --test-threads=1` -> 18 passed.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud,asr-moonshine` -> passed.
- `git diff --check -- src-tauri/src/asr/moonshine.rs src-tauri/src/speech/mod.rs` -> passed.

### Wave 1L OpenRouter API-surface decision

- Closed `audio-graph-70cf`.
- Added `docs/research/openrouter-api-surfaces-2026-06-26.md`.
- Decision: keep OpenRouter Chat Completions as the production OpenRouter LLM
  surface. Defer Responses as beta and treat Anthropic Messages as
  compatibility-only; neither becomes selectable or an automatic readiness
  probe now.
- `audio-graph-84f4` remains open and blocked by `audio-graph-b652`
  provider/endpoint catalog commands and `audio-graph-a641` Settings routing
  presets/advanced controls.

Verification:

- `git diff --check -- .seeds/issues.jsonl docs/research/openrouter-api-surfaces-2026-06-26.md` -> passed.
- `sd doctor --json` -> 12 passed.

Next active worker:

- Socrates: `audio-graph-b652` backend-only OpenRouter provider/endpoint
  catalog commands in `src-tauri/src/llm/openrouter.rs`,
  `src-tauri/src/commands.rs`, and `src-tauri/src/lib.rs`.

### Wave 1M OpenRouter catalog commands

- Closed `audio-graph-b652`.
- Added backend-only saved-key OpenRouter provider and endpoint catalog commands:
  `list_openrouter_providers_cmd(base_url)` and
  `list_openrouter_model_endpoints_cmd(model_id, base_url)`.
- The new commands load `openrouter_api_key` from the backend credential store
  and do not accept plaintext `apiKey` parameters.
- Added permissive parsers for `/providers` and
  `/models/{author}/{slug}/endpoints`, including permissive endpoint status
  metadata.
- Added safe OpenRouter URL construction that rejects embedded credentials and
  encodes model-id path segments.
- `audio-graph-84f4` remains open and is now blocked only by `audio-graph-a641`
  for Settings routing presets and advanced controls.

Verification:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud llm::openrouter::tests -- --nocapture --test-threads=1` -> 26 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud commands::tests::openrouter -- --nocapture --test-threads=1` -> 5 passed.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud plaintext_credential_loadback_is_not_registered_for_ipc -- --nocapture --test-threads=1` -> 1 passed.
- `git diff --check -- src-tauri/src/llm/openrouter.rs src-tauri/src/commands.rs src-tauri/src/lib.rs` -> passed.
- `sd doctor --json` -> 12 passed.
- `bun run check:seeds-json-output` -> ready 47, blocked 37, list parsed.

### Wave 1N parallelization map and verifier pass

Read-only queue/merge-safety audit confirmed the current checkout is broad and
dirty, with `.seeds/issues.jsonl`, Settings, Rust provider plumbing, generated
provider registry output, and CI workflow files all acting as shared surfaces.
Implementation parallelism should stay bounded until the current slices are
integrated.

Safe parallelization map:

- Use one CI/Blacksmith evidence coordinator for `audio-graph-fbf6`,
  `audio-graph-b05b`, `audio-graph-0d58`, `audio-graph-f53b`, and only include
  `audio-graph-2586` / `audio-graph-74b2` from a clean branch or worktree.
- Use one Moonshine runtime owner for `audio-graph-0117`; do not split
  concurrent edits across `src-tauri/src/asr/*`, `src-tauri/src/speech/mod.rs`,
  `src-tauri/src/settings/mod.rs`, provider registry output, or
  `src-tauri/Cargo.toml`.
- Keep `audio-graph-a641` OpenRouter Settings routing serialized because it
  spans Rust settings serialization plus frontend Settings state, i18n, and
  tests. Backend routing persistence is in progress first; frontend writes wait
  for the backend shape.
- Keep `.seeds/issues.jsonl` coordinator-owned. Workers report recommended
  extension payloads, and the main thread applies Seed updates.
- Prefer read-only scouts for research/design lanes while shared code surfaces
  are hot.

Independent verifier pass on the current shared tree:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed.
- `CARGO_TARGET_DIR=/tmp/audio-graph-verifier-target cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --locked --lib --tests --no-default-features --features cloud` -> passed.
- `CARGO_TARGET_DIR=/tmp/audio-graph-verifier-target cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --locked --lib --no-default-features --features cloud llm::openrouter::tests -- --nocapture --test-threads=1` -> 26 passed.
- `CARGO_TARGET_DIR=/tmp/audio-graph-verifier-target cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --locked --lib --no-default-features --features cloud commands::tests::openrouter -- --nocapture --test-threads=1` -> 5 passed.

No new Seed was needed from the verifier pass.

Seed hygiene follow-up:

- Updated stale `in_progress` extension markers on `audio-graph-0117`,
  `audio-graph-226e`, and `audio-graph-dd19` to superseded statuses because
  later verified or research-decision extensions already describe the true
  current state.
- Kept all three Seeds open because each still has real remaining acceptance
  criteria or blockers.

Verification:

- `sd doctor --json` -> 12 passed.
- `bun run check:seeds-json-output` -> ready 47, blocked 37, list parsed.
- `git diff --check -- .seeds/issues.jsonl docs/commit-state-2026-06-26-backlog-zero-continuation.md` -> passed.

Provider-test scout follow-up:

- Existing live smoke coverage is no-content readiness plus scripted local
  cloud smoke. OpenRouter Settings and readiness should continue using
  saved-key `/models`, `/providers`, and endpoint catalog metadata by default.
- Content-bearing routed smoke should be explicit opt-in only, synthetic, and
  redacted; it must not become automatic readiness.
- Created and linked three follow-up Seeds under `audio-graph-84f4`:
  `audio-graph-61db` OpenRouter accelerator catalog view model,
  `audio-graph-8772` OpenRouter routed smoke harness, and
  `audio-graph-76bd` OpenRouter routed provider telemetry.
- Refined `audio-graph-a641` so its acceptance includes making the typed
  routing policy public in Rust/TypeScript settings and preserving legacy
  `provider_order` through hydrate/save.
- Verification after Seed creation: `sd doctor --json` -> 12 passed,
  `bun run check:seeds-json-output` -> ready 50, blocked 37, list parsed, and
  `git diff --check -- .seeds/issues.jsonl docs/commit-state-2026-06-26-backlog-zero-continuation.md` -> passed.

CI/Blacksmith evidence scout follow-up:

- Current workflow changes are still local to this dirty checkout. Existing
  pushed branch `origin/codex/cicd-blacksmith-smoke-20260624` is not enough to
  close optional/default/playback evidence because it predates the current
  optional-feature and default Tauri smoke matrix.
- Use a clean pushed ref containing the current workflow changes, for example
  `codex/cicd-remote-evidence-20260626`, before dispatching remote evidence.
- CI dispatch command once that ref exists:
  `gh workflow run ci.yml --ref codex/cicd-remote-evidence-20260626`.
- Release dry-run dispatch once that ref exists:
  `gh workflow run release.yml --ref codex/cicd-remote-evidence-20260626 -f dry_run=true -f rsac_sha=a2d3088b0ae8050d1ce79966298cc792c6694ec2`.
- Keep non-dry release proof approval-gated. Suggested tag shape:
  `v0.0.0-ci-release-smoke-20260626.1`.
- Known prior successful runs are still useful historical evidence but cannot
  close the current matrix cutline: AudioGraph CI run
  `28126263189` on `codex/cicd-blacksmith-smoke-20260624`, and Release run
  `28125549025` on the same branch.
- Updated `audio-graph-f53b`, `audio-graph-0d58`, `audio-graph-b05b`,
  `audio-graph-fbf6`, `audio-graph-74b2`, and `audio-graph-2586` with
  `remote_evidence_plan_2026_06_26` extensions so the exact clean-ref
  commands and remaining closure criteria are tracked in Seeds.
- Verification after Seed updates: `sd doctor --json` -> 12 passed and
  `bun run check:seeds-json-output` -> ready 50, blocked 37, list parsed.

OpenRouter backend routing verification:

- Applied the backend worker result to `audio-graph-a641`:
  Rust now persists root-level `openrouter_routing_policy`, keeps legacy
  `provider_order` as compatibility fallback, and prefers the rich policy for
  blocking plus app-path streaming OpenRouter requests.
- Targeted orchestrator verification:
  `rich_routing_policy_precedes_legacy_provider_order_and_serializes_false_fallbacks` -> passed,
  `openrouter_stream_request_prefers_synced_rich_policy_over_provider_order` -> passed,
  `openrouter_routing_policy_round_trips_without_serializing_api_key` -> passed.
- Frontend Settings implementation is delegated separately and must hydrate/save
  the root-level policy without plaintext credential readback.

OpenRouter Settings routing frontend verification:

- Applied the frontend worker result to `audio-graph-a641`:
  TypeScript settings now expose `OpenRouterRoutingPolicy` and
  `openrouter_routing_policy`; Settings hydrate/save preserves legacy
  `provider_order` unless a rich OpenRouter policy is selected; Advanced
  controls include balanced, low-latency, high-throughput/Nitro, privacy/ZDR,
  and strict accelerator presets.
- Strict accelerator mode serializes `order`/`only` and
  `allow_fallbacks=false`; saved-key model discovery remains on
  `list_openrouter_models_cmd` without plaintext credential loadback.
- Targeted frontend verification:
  `bun run test -- src/components/SettingsPage.test.tsx` -> 90 passed,
  `bun run typecheck` -> passed,
  `bunx @biomejs/biome@2.5.1 check` on touched frontend files -> passed,
  `git diff --check` on touched frontend/Seeds/commit-state files -> passed.
- `audio-graph-a641` now has verified backend and frontend extensions recorded
  in Seeds; wait for subagent closeout review before marking it closed.

OpenRouter Settings routing closeout:

- Closeout review initially found two real blockers: custom rich
  `openrouter_routing_policy` values could be inferred as partial preset
  matches and rewritten into narrower fixed policies, and low-latency thresholds
  were serialized as millisecond-like values instead of seconds.
- The frontend fix now treats saved rich policies as `custom` unless they
  exactly match a built-in preset shape, preserves custom policies unchanged on
  save, serializes low-latency thresholds as `{ p50: 0.75, p90: 2.0 }`, and
  adds low-latency/high-throughput/custom preservation coverage.
- Final verification:
  `bun run typecheck` -> passed,
  `bunx @biomejs/biome@2.5.1 check` on touched frontend/i18n/type files -> passed,
  `git diff --check` on touched frontend/Seeds/commit-state files -> passed,
  `bun run test -- --pool=threads --maxWorkers=1 src/components/SettingsPage.test.tsx` -> 93 passed on retry.
- One earlier Vitest attempt failed before running tests due a worker-start
  timeout; the isolated threaded retry passed.
- `audio-graph-a641` is closed. Parent `audio-graph-84f4` remains open and
  blocked by `audio-graph-61db`, `audio-graph-8772`, and `audio-graph-76bd`.

Moonshine native loader seam:

- `audio-graph-0117` advanced but remains open. The Moonshine worker added a
  fail-closed native runtime/loader/probe seam behind `asr-moonshine`, model
  directory validation against registered component files, fake native loader
  coverage, load-failure readiness coverage, and duplicate-final guardrails.
- Worker verification:
  `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -> passed,
  `CARGO_TARGET_DIR=/tmp/audio-graph-moonshine-target cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud moonshine -- --nocapture --test-threads=1` -> 20 passed,
  `CARGO_TARGET_DIR=/tmp/audio-graph-moonshine-target cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud,asr-moonshine` -> passed,
  `CARGO_TARGET_DIR=/tmp/audio-graph-moonshine-target cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud,asr-moonshine moonshine -- --nocapture --test-threads=1` -> 25 passed.
- Read-only review accepted the slice. Remaining blockers are real native
  Moonshine C API bindings/resource lifecycle, app-level readiness/selectability
  wiring, and Linux/macOS/Windows Blacksmith evidence.

Release workflow static cleanup:

- `audio-graph-2586` advanced but remains open. Release workflow/docs static
  cleanup removed stale runner/MSI wording, clarified dry-run versus non-dry
  behavior, kept Blacksmith runner language and pinned actions intact, and
  documented clean-ref dry-run plus approval-gated non-dry proof.
- Worker verification:
  `actionlint .github/workflows/release.yml` -> passed,
  `git diff --check -- .github/workflows/release.yml docs/RELEASE.md` -> passed,
  `git diff HEAD --check -- .github/workflows/release.yml docs/RELEASE.md` -> passed,
  stale runner/MSI scan -> no matches,
  pinned-action scan -> no tag-style `uses:` entries.
- Remaining closure criteria: clean-ref release dry-run evidence and explicit
  approval-gated non-dry draft artifact proof.

Optional local LLM feature matrix:

- `audio-graph-fbf6` advanced but remains open. The optional-feature Blacksmith
  matrix in `.github/workflows/ci.yml` now includes Linux/macOS/Windows rows for
  `cloud,llm-llama` with `local_llama_stream` and `cloud,llm-mistralrs` with
  `mistralrs_engine`.
- The optional-feature job remains skipped on `pull_request`, so these
  heavyweight local/runtime checks are available for scheduled/manual/mainline
  evidence without taxing every PR.
- Worker verification:
  `actionlint .github/workflows/ci.yml` -> passed,
  `git diff --check -- .github/workflows/ci.yml` -> passed,
  `timeout 900s cargo check --locked --no-default-features --features cloud,llm-llama` -> passed,
  `timeout 900s cargo test --locked -p audio-graph --lib --no-default-features --features cloud,llm-llama local_llama_stream -- --nocapture --test-threads=1` -> 6 passed,
  `timeout 900s cargo check --locked --no-default-features --features cloud,llm-mistralrs` -> passed,
  `timeout 900s cargo test --locked -p audio-graph --lib --no-default-features --features cloud,llm-mistralrs mistralrs_engine -- --nocapture --test-threads=1` -> 2 passed.
- Orchestrator verification:
  `actionlint .github/workflows/ci.yml` -> passed,
  `git diff --check -- .github/workflows/ci.yml .seeds/issues.jsonl docs/commit-state-2026-06-26-backlog-zero-continuation.md` -> passed,
  `rg` confirmed the six local LLM matrix rows.
- Remaining closure criteria: push a clean ref with the workflow change,
  dispatch `ci.yml`, and inspect/record the six Blacksmith optional-feature jobs
  for `llm-llama` and `llm-mistralrs` across Linux/macOS/Windows.
