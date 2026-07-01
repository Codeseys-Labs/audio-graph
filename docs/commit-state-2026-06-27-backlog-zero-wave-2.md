# Commit State - 2026-06-27 Backlog-Zero Wave 2

## Commit

- Branch: `master`
- HEAD: `831cc30101840db87bd2b502f2da749d65fe1c22`
- Latest commit: `831cc30 fix: address 6 real CodeRabbit findings from the PR review`
- Captured at: `2026-06-27T02:35:43Z`

## Worktree State

This checkout is still broadly dirty and should not be used for `sd sync`,
workflow edits, release edits, or broad generated-file changes without a clean
branch/worktree plan. `git status --short` currently reports 194 rows:

- 67 untracked paths
- 168 paths with unstaged modifications
- 76 paths with staged changes
- 0 conflicted paths

The dirty tree includes earlier parent-thread work across CI workflows, docs,
frontend, backend, generated provider registry files, ASR provider modules,
audio/source modules, projection modules, persistence, and Seeds. Current work
must stay narrowly scoped and must not revert unrelated edits.

## Queue State

Authoritative queue health:

- `sd doctor --json`: 12 pass, 0 warn, 0 fail
- Direct `.seeds/issues.jsonl` parse: 310 total Seeds
- Closed: 211
- Open: 99
- In progress: 0
- Ready/open-unblocked via `bun run sd:issues -- ready-all`: 59
- Blocked via `bun run sd:issues -- blocked`: 40

Open priority split:

- P1: 33
- P2: 46
- P3: 18
- P4: 2

Ready priority split:

- P1: 14
- P2: 27
- P3: 16
- P4: 2

Blocked priority split:

- P1: 19
- P2: 19
- P3: 2

## Just Completed Wave

Closed Seeds from the prior wave:

- `audio-graph-b841`: shared ASR WebSocket write guard across Soniox and
  Deepgram with blocked-policy fake-server coverage.
- `audio-graph-0f8e`: keychain-first credential docs/comments plus safe
  planned-provider credential routing behavior without plaintext readback.
- `audio-graph-c0cb`: `bun run sd:issues -- ready-all` for uncapped
  open/unblocked queue planning.
- `audio-graph-5f5e`: subagent integration manifest and clean-ref merge gate.

Verified in the prior wave:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --no-default-features --features cloud asr::soniox -- --nocapture --test-threads=1`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --no-default-features --features cloud asr::deepgram -- --nocapture --test-threads=1`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --no-default-features --features cloud asr::transport -- --nocapture --test-threads=1`
- `bun run test src/components/SettingsPage.test.tsx`
- `bunx @biomejs/biome@2.5.1 check src/App.tsx src/components/CredentialsManager.tsx src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx`
- `bun run typecheck`
- `bun run check:seeds-json-output`
- Scoped `git diff --check`

## Current Top Ready P1 Work

The ready queue is not purely implementation work; several P1s need clean
branch/remote evidence rather than local edits from this dirty checkout.

- `audio-graph-74b2`: Blacksmith Tauri build smoke matrix. Clean CI evidence
  branch required.
- `audio-graph-cbde`: saved-credential health checks and model discovery on
  Settings open. Frontend/backend Settings files are already hot.
- `audio-graph-ad44`: event-sourced transcript/notes/graph synthesis data
  model. This unlocks transcript ledger, session artifact migration, frontend
  retcon reducers, and speaker timeline replay.
- `audio-graph-afca`: dynamic processed-audio consumer registry. This unlocks
  provider audio policy, source/channel work, and diarization consumers.
- `audio-graph-2586`: release workflow to Blacksmith and pinned actions. Clean
  CI/release branch and approval-gated workflow edits required.
- `audio-graph-f0a3`: AssemblyAI Universal-3.5 Pro Realtime/v3.
- `audio-graph-0117`: Moonshine streaming worker and span-revision adapter.
- `audio-graph-0d58`: Blacksmith Moonshine compile matrix. Clean CI evidence
  branch required.
- `audio-graph-b05b`: diarization clustering feature compile/smoke matrix.
  Clean CI evidence branch required.
- `audio-graph-f53b`: rubato output resampling into CPAL playback.
- `audio-graph-d042`: reusable ASR provider transport and parser fixture
  harness, now unblocked by `audio-graph-b841`.
- `audio-graph-fbf6`: optional Rust feature compile matrix. Clean CI evidence
  branch required.

## Wave 2 Assumptions

- Do not run `sd sync` from this dirty checkout.
- Do not touch `.github/workflows/*` without explicit approval and a clean
  evidence branch/worktree.
- Do not use or print temporary provider API keys in commands, docs, Seeds, or
  subagent prompts.
- Prefer subagent work with disjoint write scopes and main-thread Seed
  reconciliation.
- Use `bun run sd:issues -- ready-all` for complete ready planning; `sd ready`
  is still useful but capped at 50 rows.

## Recommended Next Wave

The next local wave should avoid workflow files and hot generated registry
surfaces:

1. `audio-graph-d042` ASR harness scout/implementation slice: identify the
   smallest reusable session/parser fixture boundary after the Soniox/Deepgram
   write-guard extraction.
2. `audio-graph-ad44` projection data-model scout: inspect the existing
   projection modules and propose a narrow event-sourced model slice that
   unlocks transcript ledger and speaker timeline replay.
3. `audio-graph-afca` audio consumer bus implementation or scout: advance the
   dynamic processed-audio consumer registry without touching provider UI.
4. Review lane: audit for stale credential-storage wording and queue any
   remaining keychain-first documentation drift.

## Wave 2 Results

Captured after integration at `2026-06-27T03:02:00Z`.

Queue and tooling:

- `sd doctor --json`: 12 pass, 0 warn, 0 fail
- `bun run check:seeds-json-output`: passed local and global Seeds CLI JSON
  envelope checks
- `bun run --silent sd:issues -- ready-all`: 59 ready/open-unblocked Seeds
  after reconciliation
- Fixed and closed `audio-graph-8c46`: `scripts/sd-issues.mjs` now awaits
  stdout writes so large piped `ready-all` JSON is not truncated

Integrated slices:

- `audio-graph-d042`: added scripted test-only ASR WebSocket fake-server
  helper in `src-tauri/src/asr/ws_fixture.rs`; focused ASR fixture tests pass
  5/5. Parent remains open for production/session harness extraction,
  provider-test migration, AssemblyAI/OpenAI write guards, and Blacksmith
  evidence.
- `audio-graph-afca`: hardened `src-tauri/src/audio/consumer.rs` validation
  for bounded channels, source filters, provider/conflict labels, and
  `per_source` versus `mixed_mono`; focused consumer tests pass 16/16. Parent
  remains open for OpenAI Realtime/local S2S runtime registration, broader
  coexist policy, mixed-mono production routing, and UI health surfacing.
- `audio-graph-ad44`: added
  `docs/designs/event-sourced-transcript-projection-model.md` to document
  source-of-truth boundaries, replay contracts, legacy graph migration, and
  durability caveats. Parent remains open for durable speaker/diarization
  basis, ahead-of-log materialized artifact semantics, export bundling, and
  cross-platform validation.
- `audio-graph-cbde`: read-only settings scout confirmed Settings-open avoids
  plaintext loadback, but the Seed remains open for generic `llm.api`
  readiness/catalogs, language catalogs, OpenAI Realtime health probing, source
  fallback behavior, and tighter OpenAI-compatible key details.
- `audio-graph-0c08`: read-only keychain scout confirmed the backend is
  keychain-first and mostly complete locally; closure now waits on a gated
  real OS-keychain smoke and macOS/Windows/Linux evidence.
- `audio-graph-a3d8`: review findings now track stale credentials docs and
  readiness-source fallback risk.

Verification run in this wave:

- `timeout 600s env CARGO_TARGET_DIR=/tmp/audio-graph-main-d042-target cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph --lib --no-default-features --features cloud asr::ws_fixture -- --nocapture --test-threads=1`
- `timeout 900s env CARGO_TARGET_DIR=/tmp/audio-graph-main-afca-target cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph --lib --no-default-features --features cloud audio::consumer::tests -- --nocapture --test-threads=1`
- `rustfmt +1.95.0 --edition 2024 --check src-tauri/src/audio/consumer.rs src-tauri/src/asr/ws_fixture.rs`
- Scoped `git diff --check` on this wave's touched files

No live provider/API-key tests were run. No `sd sync` was run because the
checkout still contains broad unrelated staged and unstaged work.

## Wave 2 Follow-Up Integration

Captured after worker integration and reviewer fixes at `2026-06-27T03:32:00Z`.

Additional integrated slices:

- `audio-graph-0c08`: added an ignored,
  `AUDIO_GRAPH_RUN_OS_KEYCHAIN_SMOKE=1`-gated real OS keychain smoke in
  `src-tauri/src/credentials/mod.rs` with a per-run UUID
  service/account namespace. The smoke covers scoped OS save/load,
  non-destructive `credentials.yaml` import source labels, delete tombstones,
  and non-secret serialized presence/error payloads. Reviewer findings were
  fixed in the same file: stale credential temp files are removed before
  exclusive owner-only temp creation, fallback deletes now record tombstones
  that mask recovered keychain values, and credential/migration YAML writes now
  use `fs_util::try_set_owner_only` so Windows temp writes fail before secret
  bytes are written if ACL hardening fails.
- `audio-graph-d042`: migrated one AssemblyAI provider-local lifecycle test
  onto `ws_fixture::spawn_scripted_server`. Reviewer findings were fixed by
  queuing audio before the transcript receive wait, asserting final `Turn`
  fields, and asserting the client close frame after the `Terminate` JSON.

Verification added after the fixes:

- `rustfmt +1.95.0 --edition 2024 --check src-tauri/src/fs_util/mod.rs src-tauri/src/credentials/mod.rs src-tauri/src/asr/assemblyai.rs src-tauri/src/asr/ws_fixture.rs`
- `env CARGO_TARGET_DIR=/tmp/audio-graph-main-credentials-target cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph --lib --no-default-features --features cloud credentials::tests -- --nocapture --test-threads=1`
  passed 28 tests with 1 ignored gated OS smoke.
- `env CARGO_TARGET_DIR=/tmp/audio-graph-main-credentials-target cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph --lib --no-default-features --features cloud fs_util::tests -- --nocapture --test-threads=1`
  passed 2/2.
- `env CARGO_TARGET_DIR=/tmp/audio-graph-main-assemblyai-target cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph --lib --no-default-features --features cloud asr::assemblyai::tests::run_io_fake_server_writes_audio_reads_final_and_stops -- --nocapture --test-threads=1`
  passed.
- `env CARGO_TARGET_DIR=/tmp/audio-graph-main-ws-fixture-target cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph --lib --no-default-features --features cloud asr::ws_fixture -- --nocapture --test-threads=1`
  passed 5/5.
- `git diff --check -- src-tauri/src/fs_util/mod.rs src-tauri/src/credentials/mod.rs src-tauri/src/asr/assemblyai.rs src-tauri/src/asr/ws_fixture.rs docs/commit-state-2026-06-27-backlog-zero-wave-2.md .seeds/issues.jsonl`
  passed.

Remaining closure work:

- `audio-graph-0c08` remains open until the ignored OS-keychain smoke is run
  on clean macOS, Windows, and Linux Secret Service environments with redacted
  evidence. A Windows SID-native ACL hardening follow-up is also worth tracking
  for domain/AAD/service contexts; the current `USERNAME`-based `icacls` path
  fails closed before writing credential YAML secrets.
- `audio-graph-d042` remains open for production/session transport extraction,
  AssemblyAI/OpenAI write-guard coverage, and cross-platform Blacksmith
  validation.

## Wave 3 Notes

Captured during the next bounded subagent wave at `2026-06-27T04:15:00Z`.

Completed and recorded slices:

- `audio-graph-cbde`: fixed the provider readiness credential-source fallback
  in `src/components/ProviderReadinessPanel.tsx` so a present credential with
  no presence metadata renders the localized unknown source label instead of
  falsely implying `credentials.yaml`. Focused Vitest passed 14/14 and Biome
  passed for the two panel files.
- `audio-graph-d042`: migrated the Deepgram
  `run_io_fake_server_writes_audio_reads_results_and_stops` test onto the
  scripted WebSocket fixture in `src-tauri/src/asr/deepgram.rs`, then fixed the
  same timeout-race shape found in the AssemblyAI migration by queuing audio
  before waiting for transcript events. Focused Deepgram cargo test passed.
- `audio-graph-a6d4` / `audio-graph-cbde`: improved
  `ModelCatalogPicker` accessibility with stable described-by ids, listbox
  labeling, live no-results status, active-index clamping, Home/End behavior,
  custom-value retention, and a visible `.settings-input:focus-visible` ring.
  Focused Vitest passed 7/7 and Biome passed.
- `audio-graph-0bdc`: recorded VAD/AEC candidate research. The current
  recommendation is `earshot` first for pure-Rust VAD, `sonora` first for
  pure-Rust AEC/NS, and ORT-heavy candidates only behind optional
  feature/process isolation. Created `audio-graph-098b` for playback-reference
  echo fixtures.
- `audio-graph-5011` / `audio-graph-b05b`: recorded local diarization research:
  keep `sherpa-onnx` rolling-window clustering as the unbounded local speaker
  timeline path, with `parakeet-rs` Sortformer as optional low-latency max-4
  speaker mode pending compile/license/latency evidence.
- `audio-graph-dd19`, `audio-graph-b5f3`, and `audio-graph-bfcb`: recorded that
  source-separated lanes are derived artifacts, not source-native capture
  channels, and cannot satisfy provider `source_native` admission.
- `audio-graph-fd9f` / `audio-graph-c395`: recorded that `rsac` is currently a
  sibling path dependency and not Cargo-pinned; the smallest release-safe plan
  is a pinned git dependency at the workflow SHA, then clean Blacksmith matrix
  evidence.

Still intentionally not done from this checkout:

- No workflow edits or workflow dispatches were run because `.github/workflows`
  files are dirty and approval-gated.
- No `sd sync` was run because the checkout contains broad unrelated staged and
  unstaged work.
- No live provider API-key tests were run.

## Wave 3 UI Addendum

Captured after final integration checks at `2026-06-27T04:45:00Z`.

Additional completed and recorded slices:

- `audio-graph-a6d4` / `audio-graph-cbde`: narrowed
  `ProviderReadinessPanel` live announcements to the status block only, keeping
  verbose metadata outside the polite live region. Focused Vitest passed 15/15
  and Biome passed for the panel files.
- `audio-graph-cbde`: aligned `providerSetupModes` fallback semantics so
  readiness-only saved credential presence with no source metadata renders an
  unknown source instead of implying `credentials.yaml`, including the legacy
  boolean-presence path. Focused Vitest passed 16/16 and Biome passed.
- Verified that both Seed extensions landed:
  `readiness_live_region_and_setup_source_slice_2026_06_27` on
  `audio-graph-a6d4` and
  `setup_source_fallback_and_live_region_support_2026_06_27` on
  `audio-graph-cbde`.

Queue and review follow-through:

- Fixed and closed `audio-graph-a844`: `scripts/sd-issues.mjs` now treats
  downstream stdout `EPIPE` / broken-pipe failures as a benign closed consumer
  while still rejecting other stdout write errors. Regression checks covered
  normal `ready-all` JSON output, `ready-all | head -c 1`, `ready | head -c 1`,
  and `bun run check:seeds-json-output`.
- Reopened `audio-graph-3b9f`: a read-only privacy scout found residual
  socket-edge and projection-regression gaps after the prior closeout. The
  reopened Seed records OpenAI Realtime, AssemblyAI, Gemini Live, Deepgram Aura,
  and projection fallback follow-up scopes.
- Updated `audio-graph-be03`: Soniox is wired in state/save serialization and
  the generated registry, but remains absent from ASR Settings UI until the
  live-smoke evidence gate in `audio-graph-0b93` is satisfied.
- Updated `audio-graph-a6d4`: the settings accessibility review now tracks the
  noisy top-level readiness dashboard live region, `SecretCredentialControl`
  context labels/descriptions, and localized `ModelCatalogPicker` accessible
  labels as remaining slices.
- `audio-graph-a6d4`: implemented the isolated `SecretCredentialControl`
  accessibility slice. Single-key and AWS credential controls now expose stable
  status/hint descriptions to buttons and password inputs, with contextual
  action labels that do not include secret values. Focused Vitest passed 2/2
  and Biome passed for the credential-control files.
- `audio-graph-3b9f` / `audio-graph-d042`: implemented the AssemblyAI
  socket-edge guard slice. Runtime binary audio writes now pass through
  `AsrWsWriteGuard` as audio payloads; blocked policy exits with a redacted
  `PolicyBlocked` disconnect and a fake-server regression proves no client
  content frame is emitted. Focused AssemblyAI cargo tests passed 33/33 with
  the live smoke still ignored.
