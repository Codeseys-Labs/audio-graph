# Commit State - 2026-06-25 - Deep Work Loop Checkpoint

**Timestamp:** 2026-06-25T19:55:00-07:00  
**HEAD:** `831cc30` (`master`) - `fix: address 6 real CodeRabbit findings from the PR review`

## Purpose

This checkpoint records the current orchestration state for the deep-work-loop
continuation. The checkout is intentionally broad and dirty; treat Seeds plus
the current working tree as the source of truth, not a clean commit boundary.

## Worktree State

`git status --short` shows broad staged and unstaged work across CI workflows,
release docs, provider registry/codegen, ASR/TTS providers, settings UX,
source recovery, projection runtime, sessions, localization, and Seeds.

Do not run `sd sync`, create a broad commit, or push from this checkout without
first isolating ownership into a clean branch/worktree. Several files are `MM`
and contain both pre-existing staged edits and later unstaged edits.

## Deep Work Loop Progress

Closed or integrated during this checkpoint:

- `audio-graph-c323` - provider setup UX redesign parent.
- `audio-graph-baf2` - provider setup card/readiness slice.
- `audio-graph-a1da` - Settings/Express native realtime state now follows
  `conversationMode` plus `converseEngine`.
- `audio-graph-b638` - provider capability cards now show provider audio,
  transport, platform, packaging, event, and health-probe contracts.
- `audio-graph-62ba` - source recovery now routes from Settings/Express into
  `AudioSourceSelector` with refresh, focus, clear-unavailable,
  clear-unsupported, and reselect affordances using backend source metadata.

Queue hygiene completed:

- `sd doctor` is clean after removing the stale `62ba -> 1c2f` dependency edge.
- `audio-graph-1c2f` remains open and is blocked by:
  - `audio-graph-cbde` for full saved-credential health/model-discovery
    acceptance.
  - `audio-graph-0162` for Linux/macOS/Windows provider setup validation
    evidence.
- `audio-graph-4673` remains open but is no longer a misleading ready epic. It
  is blocked by:
  - `audio-graph-ad44` for event-sourced data-model closeout, migration
    boundaries, and crash/replay fixture evidence.
  - `audio-graph-8e59` for env-gated provider-backed projection smoke with
    saved credentials and sanitized logs.
  - `audio-graph-c395` for clean Linux/macOS/Windows GitHub/Blacksmith
    validation.
- `audio-graph-dc5b` is closed after a docs-only Artificial Analysis
  streaming-STT watchlist backfill.
- `audio-graph-f8e0` is closed after adding provider-registry roadmap status,
  auth-schema metadata, xAI/NVIDIA docs-only descriptors, and Settings/readiness
  handling so `required_not_wired` descriptors do not render as "No credential
  required."
- `audio-graph-ad1d` remains open and is blocked by `audio-graph-b6a6`, which
  owns the remaining safe non-selectable watch descriptors for Cartesia,
  Inworld, Smallest.ai, Gradium, Mistral Voxtral, and Qwen/Alibaba.
- `audio-graph-b6a6` is now closed. The remaining Artificial Analysis
  watchlist providers have generated non-selectable roadmap descriptors with
  `required_not_wired` auth metadata: Inworld STT 1, Smallest.ai Pulse,
  Gradium STT, Mistral Voxtral realtime, Alibaba/Qwen3 ASR Flash, and
  Cartesia Ink-2.
- `audio-graph-ad1d` remains open, but no longer has schema/descriptor
  blockers. It still owns real provider runtime adapters, credential schemas,
  readiness probes, parser fixtures, and cross-platform validation before any
  watch candidate becomes selectable.
- `audio-graph-d042` is now filed from the Rust ecosystem audit and blocks
  `audio-graph-ad1d`. It owns the reusable ASR provider transport/parser
  harness so Soniox, AssemblyAI v3, Speechmatics, Gladia, RevAI, and similar
  providers do not keep copying WebSocket lifecycle code.
- `audio-graph-b373` is decomposed into backend streaming children:
  `audio-graph-e2b6`, `audio-graph-919e`, and `audio-graph-2f4a`.
- `audio-graph-1b50` is closed after adding the shared
  `StreamChatRequest`/backend-handle/source-metadata contract,
  provider-neutral `StreamUsage`, and shared terminal event semantics. The
  concrete streaming transports remain in the three adapter children above.
- `audio-graph-5958` is closed after replacing the LocalLlama compatibility
  wrapper with `EngineReq::StreamChat`, engine-owned `LlmStreamEvent`
  delta/done/cancel/error frames, and `CancellationToken` checks between
  generated tokens.
- `audio-graph-e2b6` is closed after wiring LocalLlama through the user-visible
  command path and persistent-context actor. `audio-graph-b373` remains open
  only for `audio-graph-919e` MistralRs streaming and `audio-graph-2f4a`
  Bedrock ConverseStream.
- `audio-graph-b05b` is open for Linux/macOS/Windows Blacksmith diarization
  validation. The local `diarization-clustering` compile/serialization break is
  fixed and verified locally.
- `audio-graph-1a60` is closed after adding the requested deep-work-loop method
  to `AGENTS.md`.
- `audio-graph-3818` is closed after the Rust ecosystem audit landed in
  `docs/research/rust-ecosystem-audit-2026-06-25.md`. Follow-up Seeds filed:
  `audio-graph-f53b` playback output resampling, `audio-graph-0bdc` VAD/AEC
  bakeoff, `audio-graph-1322` credential/config migration ADR,
  `audio-graph-d042` ASR provider transport/parser harness, and
  `audio-graph-fbf6` optional Rust feature matrix.
- `audio-graph-1322` is closed after adding proposed ADR-0019 for credential
  and config storage migration. Follow-up implementation Seeds filed:
  `audio-graph-799a` CredentialBackend facade/YAML adapter,
  `audio-graph-0c08` OS keychain backend and non-destructive import,
  `audio-graph-6ec7` ConfigCodec fixtures for `serde_yaml` replacement,
  `audio-graph-a3d8` keychain/fallback source labels and docs, and
  `audio-graph-e634` action-oriented credential controls. These are now
  attached to `audio-graph-1c2f` so the Settings epic remains blocked on real
  migration/UX work rather than the ADR alone.
- `audio-graph-6ec7` is closed after adding the first ADR-0019 implementation
  slice: a syntax-only `ConfigCodec` boundary in `settings/mod.rs` plus
  fixture-backed compatibility tests for current `config.yaml`, legacy
  `settings.json`, unknown-field tolerance, defaulted YAML fields, corrupt YAML,
  unknown provider types, and redaction. `serde_yaml` remains in use only behind
  the codec boundary for settings; parser replacement is now gated by fixtures
  instead of ad hoc round trips.
- `audio-graph-799a` is closed after adding the ADR-0019 credential facade
  slice: `CredentialBackend`, `YamlCredentialBackend`, store accessors for
  get/presence/count, compatibility wrappers for load/save/set/delete, command
  helper cleanup, and backend-backed AWS credential refresh. Direct
  `CredentialStore` YAML parse/serialize is now confined to
  `src-tauri/src/credentials/mod.rs`; OS keychain storage remains queued in
  `audio-graph-0c08`.
- `audio-graph-0c08` has a Linux-local implementation slice in place but remains
  open for Windows/macOS/Linux keychain validation. Added `keyring` v1-backed
  `KeychainCredentialBackend`, OS-keychain default backend selection,
  `credentials-state.yaml` migration tracking for migrated/deleted keys,
  source-aware credential presence/diagnostics, and malformed-YAML-preserving
  `YamlCredentialBackend` mutations. The automatic YAML fallback now filters
  keys that have been claimed by keychain or explicitly deleted, so stale
  plaintext values cannot resurrect a deleted credential unless the user
  explicitly forces the file backend for recovery/dev use. A review-found edge
  case was fixed so successful file-fallback writes clear stale migration
  tombstones for the written keys and remain visible on subsequent fallback
  reads.

## Verification Snapshot

Focused verification completed for the current Settings/source-recovery work:

- `bun run typecheck`
- `bun run test src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/components/providerSetupModes.test.ts src/components/AudioSourceSelector.test.tsx src/store/index.test.ts`
- `git diff --check -- src/types/index.ts src/store/index.ts src/store/index.test.ts src/components/providerSetupModes.ts src/components/providerSetupModes.test.ts src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx src/components/ExpressSetup.tsx src/components/ExpressSetup.test.tsx src/components/AudioSourceSelector.tsx src/components/AudioSourceSelector.test.tsx .seeds/issues.jsonl`
- `sd doctor`
- `bun run check:seeds-json-output`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud llm::streaming::tests`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib llm::streaming::tests::local_llama_stream_chat_emits_single_delta_and_done_from_backend_handle`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features llm-llama`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features llm-llama local_llama_stream -- --nocapture --test-threads=1`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud,diarization-clustering events::tests::diarization_span_revision_serializes_snake_case_contract`
- `bun run test src/components/providerRegistryHelpers.test.ts src/components/ProviderReadinessPanel.test.tsx src/components/SettingsPage.test.tsx`
- `bun run generate:provider-registry`
- `bun run check:provider-registry`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph-provider-registry -- --nocapture`
- `bun run test src/generated/providerRegistry.test.ts`
- `git diff --check -- docs/research/rust-ecosystem-audit-2026-06-25.md .seeds/issues.jsonl src-tauri/src/llm/engine.rs src-tauri/src/llm/streaming.rs`
- `sd doctor --json` after dependency cleanup returned 12 pass, 0 warn, 0 fail.
- `git diff --check -- docs/adr/0019-credential-and-config-storage.md docs/adr/README.md docs/commit-state-2026-06-25-deep-work-loop-checkpoint.md .seeds/issues.jsonl`
- `sd doctor --json` after ADR-0019 queue updates
- `bun run check:seeds-json-output` after ADR-0019 queue updates
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` after
  ConfigCodec fixture slice
- `git diff --check -- src-tauri/src/settings/mod.rs src-tauri/fixtures/settings`
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud config_codec -- --nocapture`
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud settings::tests -- --nocapture --test-threads=1`
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud credentials::tests -- --nocapture --test-threads=1` (16 passed after fallback tombstone fix)
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud credential_presence -- --nocapture --test-threads=1`
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud yaml_credentials_provider -- --nocapture --test-threads=1`
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud access_key_resolution -- --nocapture --test-threads=1`
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud settings::tests::runtime_credentials -- --nocapture --test-threads=1`
- `timeout 600s cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud credentials::tests -- --nocapture --test-threads=1`
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud credential_presence -- --nocapture --test-threads=1`
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud yaml_credentials_provider -- --nocapture --test-threads=1`
- `timeout 420s cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud settings::tests::runtime_credentials -- --nocapture --test-threads=1`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `git diff --check -- src-tauri/src/credentials/mod.rs src-tauri/src/commands.rs src-tauri/src/aws_util/mod.rs src-tauri/Cargo.toml src-tauri/Cargo.lock docs/adr/0019-credential-and-config-storage.md`

The focused frontend suite reported 177 passing tests for the source-recovery
slice. Some Rust checks were intentionally kept focused or deferred because
the dirty checkout has large pre-existing backend changes and prior cargo
invocations hit artifact-lock delays.

## Subagent Wave

Delegated wave completed and all returned subagents were closed:

- Configuration closure audit: read-only review of `audio-graph-1c2f`,
  `audio-graph-cbde`, and `audio-graph-0162`.
- Backend streaming context worker: bounded implementation/review for
  `audio-graph-1b50`, avoiding Settings/React/CI workflow ownership.
- Provider watchlist worker: completed docs-only backfill for
  `audio-graph-dc5b`; the follow-up schema/UI worker completed and closed
  `audio-graph-f8e0`; the descriptor worker completed and closed
  `audio-graph-b6a6`.
- Diarization review worker: read-only critique of local/provider/hybrid
  speaker timeline coverage and cross-platform risks; the main thread fixed the
  immediate `diarization-clustering` payload compile break.
- Streaming LLM contract worker: completed `audio-graph-1b50`; LocalLlama
  adapter and true actor-loop streaming are now closed in `audio-graph-e2b6`
  and `audio-graph-5958`.
- Rust ecosystem audit workers: completed read-only scans of existing repo
  dependencies and candidate crates. Findings were consolidated into
  `docs/research/rust-ecosystem-audit-2026-06-25.md` and the follow-up Seeds
  listed above.
- Credential/config audit workers: completed read-only backend and frontend
  audits for ADR-0019. Findings were consolidated into the ADR plus follow-up
  Seeds for the credential facade, keychain migration, config codec fixtures,
  source-label/UI docs, and action-oriented credential controls. Both returned
  subagents were closed.

The main thread owns integration, dependency cleanup, verification, and final
Seeds reconciliation.

## Architecture Reconciliation Notes

The transcript-to-notes/temporal-graph path is no longer append-only in the
current working tree. It has immutable transcript events, a revisioned
TranscriptLedger basis, TTFT-aware ProjectionScheduler primitives, structured
ProjectionPatch generation, validated notes/graph materializers, accepted patch
replay, frontend transcript/notes/graph reducers, and projection diagnostics.

The remaining architectural risk is validation and closure, not another queue
rewrite. The scheduler currently runs one basis-bound in-flight job per
projection kind, coalesces newer ledger state while waiting, adapts TTFT from
observed generation latency, and repairs stale completions. More speculative
parallelization should be driven by replay/eval evidence before adding
complexity.

The provider roadmap was refreshed against
`https://artificialanalysis.ai/speech-to-text/streaming` on 2026-06-25.
Closed follow-up `audio-graph-dc5b` classifies xAI Grok STT,
NVIDIA/Together Nemotron ASR, Inworld STT, Smallest.ai Pulse, Gradium STT,
Mistral Voxtral Realtime, Alibaba/Qwen3 ASR Flash Realtime, and Cartesia Ink-2
as watch or enterprise-watch providers. Closed follow-up `audio-graph-f8e0`
adds the registry schema and Settings safety handling. Closed follow-up
`audio-graph-b6a6` adds generated non-selectable descriptors for the remaining
watchlist providers. These descriptors are metadata only: they are not
selectable, do not advertise health checks, and must remain behind future
runtime/readiness/cross-platform proof.

The Rust ecosystem audit conclusion is conservative: keep backend ownership in
Rust/Tauri, exploit existing `rsac`, `rubato`, `tokio-tungstenite`,
`tokio-util`, `sherpa-onnx`, `parakeet-rs`, and `reqwest` streaming support
more deeply, and only add focused dependencies after bakeoffs or ADRs. Avoid
generic scheduler crates for TTFT-aware projection diffs, direct `cpal` capture
in place of `rsac`, and selectable provider entries without runtime, readiness,
credential, parser, and cross-platform proof.

ADR-0019 sets the credential/config migration direction: add a backend-owned
credential facade first, make OS-native keychain storage the production target,
keep `credentials.yaml` as an import/dev fallback, preserve saved-key
health/model discovery without plaintext IPC, and gate `serde_yaml` replacement
behind ConfigCodec compatibility fixtures. Stronghold remains a possible future
vault backend rather than the default path because the current product model
needs backend-owned native desktop credentials before app-managed vault UX.

The current `audio-graph-0c08` implementation intentionally keeps legacy
`credentials.yaml` as a manual recovery artifact, but keychain-owned and deleted
keys are tracked in non-secret migration state and filtered from automatic YAML
fallback. This resolves the stale-file resurrection risk found during review
while preserving explicit file-backend rollback for headless/dev/recovery use.
Remaining before `audio-graph-0c08` can close: explicit product policy/UX for
whether production may auto-save plaintext fallback when the OS keychain is
unavailable, per-key source labels such as `imported_file`, and GitHub/Blacksmith
Linux/macOS/Windows keychain validation.
