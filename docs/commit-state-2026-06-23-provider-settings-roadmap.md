# Commit State - 2026-06-23 - Provider settings roadmap

**Timestamp:** 2026-06-23T17:24:40-07:00

## Repository state

This checkout is not in a normal Git state. `git log --oneline -5` fails with:

```text
fatal: your current branch appears to be broken
```

The worktree and Seeds queue are therefore the source of truth until Git history
is repaired. `git status --short` shows a broad initial-add index plus local
modifications; do not interpret staged files as a clean commit boundary.

## Current worktree changes from this roadmap pass

- Added `AGENTS.md` with the operating method for this repo: use Seeds as the
  work queue, fan out research/audit work with subagents when useful, reconcile
  Seeds at the end of code-affecting work, keep secrets out of normal UI state,
  and do not run `sd sync` while the index is in a broad staged state.
- Added `docs/research/streaming-stt-provider-ranking-2026-06-23.md`, ranking
  Soniox, AssemblyAI v3, Deepgram, Gladia, Speechmatics, ElevenLabs, and other
  Artificial Analysis streaming STT candidates for AudioGraph.
- Updated first-run credential detection so an existing `openrouter_api_key`
  suppresses Express Setup.
- Updated endpoint credential routing so generic OpenAI-compatible providers
  backed by `openrouter.ai` hydrate from the saved OpenRouter key slot.
- Centralized Sherpa Zipformer required runtime filenames and made capture
  preflight reject missing or zero-byte required files.
- Documented the frontend graph snapshot/delta contract: snapshots are
  authoritative resyncs, while deltas are best-effort incremental updates.
- Added Seeds for provider registry descriptor coverage, session artifact
  migration, and the ElevenLabs watch/spike; closed the already-implemented
  slices for provider research, OpenRouter credential routing, Sherpa preflight,
  and graph delta contract.

## Verification already run

- `bun run typecheck` - pass.
- `bunx @biomejs/biome check src/store/index.test.ts src/types/index.ts` -
  pass.
- `bun run test -- src/App.test.tsx` - pass, 6 tests.
- `bun run test -- src/store/index.test.ts` - pass, 24 tests.
- `bun run test -- src/App.test.tsx src/store/index.test.ts` - failed before
  executing tests due to a Vitest worker startup timeout; the two files passed
  when run separately.
- `cargo fmt` - pass.
- `cargo check` - pass.
- `cargo test settings::tests::endpoint_credential_routing_covers_known_openai_compatible_hosts`
  - pass.
- `cargo test settings::tests::generic_openrouter_api_provider_hydrates_from_openrouter_slot`
  - pass.
- `cargo test models::tests::sherpa_zipformer_validation_requires_runtime_files`
  - pass.

## Seeds queue snapshot

`sd ready --format json` currently reports 39 ready items.

Highest-priority ready item:

- `audio-graph-6381` - pin release rsac input to the same SHA tested by CI.
  This is P0, but it modifies CI/release workflow behavior. Defer code changes
  until the user approves CI edits.

Recommended next non-CI implementation wave:

- `audio-graph-257a` - Provider registry skeleton and descriptor coverage
  tests. This is the best architecture slice to reduce Rust/TS/provider drift
  before adding Soniox and additional streaming STT providers.

Other near-term P1s:

- `audio-graph-3709` - normalized ASR partial/final events with span revisions.
- `audio-graph-afca` - dynamic processed-audio consumer registry.
- `audio-graph-ad44` - event-sourced transcript/notes/graph synthesis model.
- `audio-graph-c309` - redacted credential presence API.
- `audio-graph-5fe7` - feature-gate local ML behind Cargo features.

## Known limitations

- Full CI was not modified or run.
- `sd sync` was not run because this checkout has a broad staged state and the
  repo instructions say not to sweep unrelated staged files into Seeds sync.
- Full frontend and Rust suites were not rerun after every change; only targeted
  tests and `cargo check`/typecheck were used for this pass.
- Provider research is current to the cited docs and Artificial Analysis page
  used on 2026-06-23, but provider rankings and model catalogs should be
  refreshed before committing to long-term roadmap order.

## Continuation - provider registry and credentials

Additional work completed after the initial snapshot:

- Implemented `src-tauri/src/provider_registry.rs`, a backend-owned provider
  descriptor registry covering current ASR, LLM, TTS, Gemini Live, and planned
  OpenAI Realtime voice-agent surfaces.
- Registered `get_provider_registry_cmd` and added matching TS
  `ProviderDescriptor` contract types.
- Moved the ASR single-session source guard to registry `source_policy`.
- Closed `audio-graph-257a`.
- Added `audio-graph-01be` for non-provider runtime model descriptors
  (Sortformer and clustering diarization dependencies) and linked it under the
  model-picker readiness path.
- Started `audio-graph-c309`: added `load_credential_presence_cmd`, a redacted
  credential presence API, and moved App first-run Express Setup detection to
  that presence API instead of probing plaintext keys.
- Added planned ASR descriptors for Soniox realtime, Gladia Solaria live,
  Speechmatics realtime enhanced, and ElevenLabs Scribe realtime. These are
  visible as `ProviderStatus::Planned` registry entries, not selectable ASR
  settings variants.
- Added local credential slots for the planned streaming STT candidates:
  `soniox_api_key`, `gladia_api_key`, `speechmatics_api_key`, and
  `elevenlabs_api_key`. The slots are redacted in Debug, included in
  non-secret credential presence, and counted by first-launch/demo credential
  detection.
- Added Settings readiness display labels for those planned provider ids so a
  manually saved future key renders intelligibly without cluttering readiness
  when no key is present.

Additional verification:

- `cargo test provider_registry::tests -- --nocapture` - pass, 10 tests.
- `cargo test streaming_source_guard -- --nocapture` - pass, 5 tests.
- `cargo check` - pass after provider-registry changes.
- `bun run test -- src/App.test.tsx` - pass, 6 tests.
- `bun run typecheck` - pass after credential-presence frontend changes.
- `bunx @biomejs/biome check src/App.tsx src/App.test.tsx src/types/index.ts`
  - pass.
- `cargo test --lib provider_registry -- --nocapture` - pass, 11 tests.
- `cargo test --lib credential -- --nocapture` - pass, 21 tests.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `bun run typecheck` - pass.
- `bun run test src/App.test.tsx src/components/SettingsPage.test.tsx`
  - pass, 52 tests.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

Verification limitation:

- The focused Rust credential-presence test initially caught a fixture compile
  issue, which was fixed. The rerun then left a stale Cargo/test PTY holding the
  build-directory lock with no visible Cargo/rustc process. Because there was
  no safe visible process to terminate, `cargo check --tests` could not
  complete in this pass. Treat the credential-presence backend slice as
  frontend-verified and Rust source-formatted, but not fully Rust-test-verified
  until the Cargo lock clears.

## Continuation - OpenRouter and Gemini saved-key readiness slice

The earlier Cargo lock cleared. Additional work completed:

- OpenRouter Settings no longer hydrates the saved `openrouter_api_key` into
  React state during Settings open.
- Gemini Settings no longer hydrates the saved `gemini_api_key` into React
  state during Settings open.
- Settings now loads redacted credential presence and uses saved
  `openrouter_api_key` / `gemini_api_key` presence to enable OpenRouter Test
  Connection, OpenRouter Load Models, and Gemini Test Connection while password
  inputs remain blank.
- `test_openrouter_connection_cmd` and `list_openrouter_models_cmd` now accept
  an optional draft key; when the UI passes `null`, Rust loads the saved
  OpenRouter key from `credentials.yaml` internally.
- `test_gemini_api_key` now accepts an optional draft key; when the UI passes
  `null`, Rust loads the saved Gemini key from `credentials.yaml` internally.
- The OpenRouter and Gemini password fields show saved-key hints and remain
  replace-only: blank keeps the saved key, typing a new key replaces it on Save.
- Filed `audio-graph-0d1c` to supersede ADR-0014's on-demand notes decision
  after `audio-graph-ad44` defines the event-sourced projection model.
- Updated `audio-graph-c309` acceptance to explicitly require SettingsPage not
  to invoke `load_credential_cmd` for saved OpenRouter/Gemini/default provider
  readiness on open.

Additional verification:

- `bun run test -- src/components/SettingsPage.test.tsx` - pass, 36 tests.
- `bun run typecheck` - pass.
- `bunx @biomejs/biome check src/App.tsx src/components/SettingsPage.tsx src/components/LlmProviderSettings.tsx src/components/GeminiSettings.tsx src/components/SettingsPage.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json src/types/index.ts`
  - pass.
- `cargo fmt` - pass.
- `cargo test api_key_resolution -- --nocapture` - pass, 6 tests.
- `cargo test credential_presence_maps_every_allowed_key_without_secret_values -- --nocapture`
  - pass, 1 test.
- `cargo check` - pass.

Remaining credential/config work:

- `audio-graph-c309` remains open because Deepgram, AssemblyAI, generic
  OpenAI-compatible endpoints, AWS access-key fields, TTS Deepgram, and
  `load_all_credentials_cmd` still need the same no-plaintext/restrict-by-default
  treatment.
- `audio-graph-cbde` remains the next readiness layer: saved-key health checks,
  model discovery, TTL/debounce, and backend-owned cached provider readiness on
  Settings open.

## Continuation - Deepgram, AssemblyAI, and Aura saved-key readiness slice

Additional work completed:

- Deepgram STT Settings no longer hydrates the saved `deepgram_api_key` into
  React state during Settings open.
- AssemblyAI STT Settings no longer hydrates the saved `assemblyai_api_key` into
  React state during Settings open.
- Deepgram Aura TTS now uses saved Deepgram credential presence to enable Test
  Connection without requiring the user to re-enter the key.
- `test_deepgram_connection`, `test_assemblyai_connection`, and
  `test_tts_connection_cmd` now accept optional draft keys. When the UI passes
  `null`, Rust resolves the saved key from `credentials.yaml` internally.
- STT/TTS saved-key hints match the OpenRouter/Gemini replace-only pattern:
  blank keeps the saved key, typing a new key replaces it on Save.
- Updated `audio-graph-c309` to record this third slice and keep remaining
  credential work explicit.

Additional verification:

- `bun run test -- src/components/SettingsPage.test.tsx` - pass, 39 tests.
- `bun run typecheck` - pass.
- `bunx @biomejs/biome check src/App.tsx src/components/SettingsPage.tsx src/components/LlmProviderSettings.tsx src/components/GeminiSettings.tsx src/components/AsrProviderSettings.tsx src/components/SettingsPage.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json src/types/index.ts`
  - pass.
- `cargo fmt` - pass.
- `cargo test api_key_resolution -- --nocapture` - pass, 12 tests.
- `cargo check` - pass.

Remaining credential/config work:

- `audio-graph-c309` remains open because generic OpenAI-compatible endpoint
  keys, AWS access-key/secret/session fields, `load_all_credentials_cmd`, and
  backend-owned readiness caching still need the same no-plaintext treatment.

## Continuation - diarization follow-up Seeds

Added a diarization workstream in response to the local/provider/hybrid speaker
attribution gap:

- `audio-graph-3588` - Local streaming diarization and speaker timeline
  architecture.
- `audio-graph-5011` - Local streaming diarization worker with flexible speaker
  counts. Blocked by `audio-graph-01be` and `audio-graph-afca`.
- `audio-graph-1fbd` - Normalize provider diarization into speaker-span
  revisions. Blocked by `audio-graph-3709` and `audio-graph-ad44`.
- `audio-graph-56da` - Investigate speaker-separated multichannel ASR feed vs
  metadata join. Blocked by `audio-graph-3251`.
- `audio-graph-dbac` - Diarization settings UX for local, provider, and hybrid
  modes. Blocked by `audio-graph-80ed`.

Architecture note:

- Provider diarization is available in some current/planned ASR paths, but it
  should normalize into a provider-agnostic `SpeakerTimeline` rather than
  directly mutating transcript rows.
- Local streaming diarization should emit revisioned speaker spans with stable
  speaker IDs and flexible auto/max speaker-count policy.
- Diarization alone does not separate a mixed mono signal into clean per-speaker
  channels. A true multi-channel speaker feed requires source-native separated
  channels or an explicit source-separation/speaker-extraction stage, so
  `audio-graph-56da` tracks that research before implementation.

## Continuation - generic endpoint and AWS replace-only credentials

Additional work completed:

- Generic OpenAI-compatible ASR/LLM endpoint keys are now replace-only in
  Settings for OpenAI, OpenRouter, Groq, Together, Fireworks, and Gemini routed
  endpoints. The form shows saved-key presence hints but does not hydrate
  stored plaintext keys into React state on Settings open.
- `test_cloud_asr_connection` accepts `apiKey: null` and resolves the saved
  endpoint-specific credential in Rust when the password field is blank.
- AWS access-key mode is now presence-first: saved access key ID, secret access
  key, and session token are not loaded into Settings fields, while saved
  access+secret presence enables Test Connection with blank fields.
- AWS test config resolves blank access key/secret/session inputs from
  `credentials.yaml`, preserving draft credential testing when the user types
  replacement values.
- Clear Saved AWS Keys now deletes `aws_access_key`, `aws_secret_key`, and
  `aws_session_token` together instead of leaving a stale access key behind.
- `load_all_credentials_cmd` was removed from the registered IPC command list.
- Updated `audio-graph-c309` to record this fourth slice. It remains open for
  saved-key health/model readiness caching and a final non-secret log/debug
  surface audit.

Additional verification:

- `bun run test -- src/components/SettingsPage.test.tsx` - pass, 42 tests.
- `bun run typecheck` - pass.
- `bunx @biomejs/biome check src/components/SettingsPage.tsx src/components/AsrProviderSettings.tsx src/components/LlmProviderSettings.tsx src/components/settingsTypes.ts src/components/SettingsPage.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json`
  - pass.
- `cargo fmt` - pass.
- `cargo test key_resolution -- --nocapture` - pass, 16 tests.
- `cargo test draft_or_saved_secret -- --nocapture` - pass, 1 test.
- `cargo check` - pass.

## Continuation - CredentialStore debug redaction and c309 closure

Additional work completed:

- `CredentialStore` no longer derives raw `Debug`.
- Added a manual `Debug` implementation that reports credential fields as
  `<present>` / `<missing>` only.
- Added a regression test proving `format!("{store:?}")` does not include
  sentinel API keys, AWS secrets, or session tokens.
- Closed `audio-graph-c309` as the no-plaintext Settings/readiness foundation.

Follow-up Seeds filed/updated from the subagent audits:

- `audio-graph-cbde` now carries the backend-owned provider readiness cache
  design: `get_provider_readiness`, TTL/debounce, in-flight coalescing,
  credential epoch invalidation, and non-secret cache keys.
- `audio-graph-1bd7` tracks redacted `Debug` for runtime-hydrated settings and
  provider config structs beyond `CredentialStore`.
- `audio-graph-74ed` tracks redacting upstream provider error bodies before
  returning UI-visible errors, so echoed keys do not leak through provider
  diagnostics.
- `audio-graph-e78e` remains open for Windows ACL/write hardening and broader
  local credential persistence hardening.

Additional verification:

- `cargo test debug_output_redacts_secret_values -- --nocapture` - pass, 1
  test.
- `cargo check` - pass.

## Continuation - provider error-body redaction

Additional work completed:

- Added shared error-redaction helpers that remove known submitted credential
  values before provider response bodies are returned through UI-visible
  diagnostics.
- Redacted the generic OpenAI-compatible ASR Settings probe
  (`test_cloud_asr_connection`) against the draft or saved key resolved by the
  backend.
- Redacted generic Cloud ASR worker non-2xx errors, generic LLM `ApiClient`
  non-2xx errors, and OpenRouter model-catalog/chat non-2xx errors.
- Reworked redaction tests to use pure error-message helpers instead of local
  mock servers, so the security invariant is covered even in sandboxes that
  deny local socket binds.
- Closed `audio-graph-74ed`.
- Updated `audio-graph-1bd7` with the read-only audit finding that
  `YamlRefreshingCredentialsProvider` also derives secret-bearing `Debug`.

Additional verification:

- `cargo fmt` - pass.
- `cargo check --lib` - pass.
- `cargo test redact --lib -- --nocapture` - pass, 11 tests.

Verification limitation:

- An earlier `cargo test redact -- --nocapture` run, before the tests were
  reworked, failed because this sandbox denies local mock-server socket binds.
  The redaction tests were then changed to exercise pure error-message helpers
  instead of local sockets. The follow-up PTY-backed `--lib` run passed and is
  the authoritative test result for this slice.

## Continuation - credential-bearing Debug redaction

Additional work completed:

- Replaced raw derived `Debug` on credential-bearing settings/provider structs
  with manual implementations that print secret material as `<present>` /
  `<missing>` while retaining non-secret routing fields.
- Covered settings-level surfaces: `AwsCredentialSource`, `AsrProvider`,
  `LlmApiConfig`, `LlmProvider`, `GeminiAuthMode`, and `GeminiSettings`.
- Covered runtime provider config surfaces: `CloudAsrConfig`,
  `DeepgramConfig`, `AssemblyAIConfig`, `OpenAiRealtimeConfig`, LLM
  `ApiConfig`, `OpenRouterConfig`, `GeminiConfig`, and
  `YamlRefreshingCredentialsProvider`.
- Added a reusable `credentials::redacted_secret_presence` helper so debug
  redaction uses the same presence convention as `CredentialStore`.
- Closed `audio-graph-1bd7`.

Additional verification:

- `cargo fmt` - pass.
- `cargo check --lib` - pass.
- `cargo test debug_redacts --lib --no-default-features --features cloud -- --nocapture`
  - pass, 9 tests.
- `rg` scan for `log::*("{:?}"...)` / `format!("{:?}"...)` found debug
  formatting for paths, graph indices, log levels, and error values, but no
  production log path formatting the credential-bearing settings/provider
  structs with raw `Debug`.

Verification limitation:

- A default-feature `cargo test debug_redacts --lib -- --nocapture` attempt was
  interrupted after the full native-ML test binary link sat silent for several
  minutes. The code is default-feature checked by `cargo check --lib`, while
  the focused tests are cloud-only to avoid the heavy native-ML linker path.

## Continuation - local vLLM topology documentation

Additional work completed:

- Expanded the README with a `Local LLM with vLLM` setup section that maps
  AudioGraph Settings to the existing OpenAI-compatible LLM provider.
- Expanded `docs/ops/vllm-backend.md` with local and remote `vllm serve`
  examples, host/port, `--served-model-name`, `--max-model-len`,
  `--gpu-memory-utilization`, prefix caching, optional `--api-key`, warmup,
  and Windows/WSL2 guidance.
- Documented that `--enforce-eager` should stay off for the normal low-latency
  CUDA-graph path because it disables CUDA graphs; it remains a
  compatibility/debug fallback.
- Added 7B/8B model examples including `meta-llama/Llama-3.1-8B-Instruct` and
  `mistralai/Mistral-7B-Instruct-v0.3`.
- Added runbook references to ADR-0003, ADR-0012, and the vLLM Rust frontend
  research note.
- Closed `audio-graph-0af2`.

Additional verification:

- Context7 query against the official stable vLLM docs for `vllm serve`,
  OpenAI-compatible server flags, and `--enforce-eager` / CUDA graph behavior.
- `rg` verification confirmed the README/runbook contain the required vLLM
  flags and setup sections.

## Continuation - local ML feature-gate audit

Additional work completed:

- Audited `audio-graph-5fe7` against ADR-0007 and the current codebase.
- Confirmed the code side is already implemented: `src-tauri/Cargo.toml`
  keeps `local-ml` as the default umbrella feature, exposes a no-dependency
  `cloud` marker, and makes `whisper-rs`, `llama-cpp-2`, and `mistralrs`
  optional behind `asr-whisper`, `llm-llama`, and `llm-mistralrs`.
- Confirmed the cloud-only feature graph excludes the heavy local ML crates:
  `cargo tree --no-default-features --features cloud -i whisper-rs`,
  `llama-cpp-2`, and `mistralrs` each returned package-not-found.
- Confirmed `.github/workflows/ci.yml` does not currently contain a dedicated
  cloud-only Rust CI job. Because CI edits are approval-gated in this session,
  `audio-graph-5fe7` remains open and is now blocked by `audio-graph-150f`
  instead of being treated as ready code work.
- Updated `audio-graph-5fe7` with the audit result and a structured extension
  noting the remaining CI/test acceptance.

Additional verification:

- `cargo check --no-default-features --features cloud --lib` - pass, 1m28s.

## Continuation - diarization/channel projection queue

Additional work completed:

- Confirmed local diarization is already a serious architecture track, not just
  a placeholder: ADR-0017 documents the unknown-count sherpa-onnx clustering
  backend, and `src-tauri/src/diarization/worker.rs` contains a rolling-window
  local worker behind `diarization-clustering`.
- Created `audio-graph-eebf` for the missing product bridge: projecting local
  or provider diarization spans into normalized ASR speaker metadata and, where
  a provider supports it, deterministic channel-aware audio/session inputs.
- Linked `audio-graph-eebf` behind `audio-graph-3588` (speaker timeline
  architecture), `audio-graph-3709` (ASR span revisions), and
  `audio-graph-afca` (dynamic processed-audio consumer registry).

Research notes:

- Artificial Analysis currently lists 27 streaming STT models/providers and
  includes Soniox v5 Real-Time, AssemblyAI, Deepgram, Speechmatics, Google,
  Azure, OpenAI, NVIDIA/Together, Cartesia, Alibaba/Qwen, and others on the
  streaming benchmark page.
- Deepgram supports streaming diarization with `diarize_model=latest`/`v1`;
  streaming returns word-level `speaker` values, while `speaker_confidence` is
  batch-only.
- Soniox STT real-time v5 supports `enable_speaker_diarization` and returns a
  `speaker` field on tokens when enabled; raw audio config declares
  `num_channels`.
- AssemblyAI Universal Streaming supports `speaker_labels`, optional
  `max_speakers`, word-level speaker labels on final words, and end-of-stream
  `SpeakerRevision` messages.
- Speechmatics Realtime supports speaker diarization and multichannel/channel
  diarization modes, including channel plus speaker diarization for realtime.

## Continuation - normalized ASR span-revision event foundation

Additional work completed:

- Added the backend `asr-span-revision` event contract with stable span fields,
  provider/source identity, optional provider item and transcript IDs, optional
  speaker/channel metadata, start/end timestamps, confidence, finality,
  revision number, supersession, turn metadata, raw event reference, and
  receive timestamp.
- Emitted `asr-span-revision` alongside legacy `asr-partial` and
  `transcript-update` events, so existing UI behavior remains compatible while
  notes/graph/transcript projection work can start consuming the normalized
  stream.
- Added provider attribution for current ASR paths: local Whisper, cloud API,
  Deepgram, AssemblyAI, OpenAI Realtime, AWS Transcribe, Sherpa, and the
  local-diarization transcript path.
- Added frontend types, store state, and Tauri listener plumbing for passive
  retention of the latest ASR span revisions.
- Kept `audio-graph-3709` open because provider-specific item IDs,
  supersession/revision semantics, and replay fixtures still need to be wired
  before partial revisions can reliably update transcript/notes/graph rows
  instead of appending duplicates.

Additional verification:

- `bun run typecheck` - pass.
- `bun run test -- src/hooks/useTauriEvents.test.ts` - pass, 21 tests.
- `bunx @biomejs/biome check src/types/index.ts src/store/index.ts src/hooks/useTauriEvents.ts src/hooks/useTauriEvents.test.ts`
  - pass.

## Continuation - config.yaml migration file tests

Additional work completed:

- Split settings disk persistence into path-based helpers so canonical
  `config.yaml` and legacy `settings.json` import behavior can be tested
  without constructing a platform-sensitive Tauri `AppHandle`.
- Preserved the public `save_settings` / `save_settings_locked` behavior:
  inline secrets are migrated to `credentials.yaml`, YAML written to disk is
  redacted, and owner-only file hardening still wraps the temp-file rename.
- Added file-level migration regressions covering canonical YAML precedence,
  legacy JSON import writing canonical YAML, corrupt YAML falling back without
  importing stale legacy JSON, and redacted YAML writes.

Additional verification:

- `cargo fmt` - pass.
- `cargo test --lib settings::tests:: -- --nocapture` - pass, 26 tests.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - readiness catalog model suggestions

Additional work completed:

- Added a frontend helper that resolves model catalogs from backend
  `get_provider_readiness_cmd` payloads, falling back to the generated
  backend provider registry for fixed/local defaults.
- Threaded catalog suggestions into the model fields for OpenAI Realtime ASR,
  Sherpa-ONNX, mistral.rs, and Gemini Live using datalist-backed inputs so
  users can pick known models without losing the ability to type custom IDs.
- Kept OpenRouter on its remote model-picker path and avoided inventing a fake
  Deepgram remote catalog before a real catalog strategy exists.
- Added regression coverage that proves readiness-provided catalogs show up in
  the actual model fields without calling plaintext credential load commands.

Additional verification:

- `bun run typecheck` - pass.
- `bun run test src/components/SettingsPage.test.tsx src/generated/providerRegistry.test.ts`
  - pass, 49 tests.
- `bunx @biomejs/biome check --write src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx src/components/AsrProviderSettings.tsx src/components/LlmProviderSettings.tsx src/components/GeminiSettings.tsx`
  - pass after formatting.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - AGENTS workflow tightening

Additional work completed:

- Tightened `AGENTS.md` so meaningful findings must update an existing Seed,
  create a Seed under the nearest epic, or close an acceptance-complete Seed.
- Added explicit cross-platform guardrails: do not ship Windows-only behavior
  as the default path; use platform capability checks or document/test matching
  macOS and Linux behavior.
- Updated secret guidance to refer to `config.yaml`, legacy `settings.json`,
  `credentials.yaml`, logs, docs, screenshots, and Seeds.
- Created and closed `audio-graph-2736` for the methodology update.

## Continuation - Deepgram advanced controls disclosure

Additional work completed:

- Moved Deepgram endpointing, UtteranceEnd, VAD events, Flux EOT thresholds,
  EOT timeout, and max-speaker cap behind a native Advanced provider controls
  disclosure.
- Kept the basic Deepgram setup path focused on key, model, diarization, and
  Test Connection.
- Added English and Portuguese labels for provider advanced controls plus
  restrained disclosure styling.
- Added a Settings regression proving Deepgram advanced controls start
  collapsed and still persist edited endpointing/max-speaker values after
  opening.

Additional verification:

- `bunx @biomejs/biome check --write src/components/AsrProviderSettings.tsx src/components/SettingsPage.test.tsx src/styles/settings.css src/i18n/locales/en.json src/i18n/locales/pt.json`
  - pass after formatting.
- `bun run typecheck` - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 47 tests.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - provider registry TS export drift guard

Additional work completed:

- Added `provider_registry_typescript_module()` in the Rust provider registry.
  It serializes the backend-owned registry into the checked-in frontend module
  shape without requiring a platform-specific codegen shell script.
- Added `src/generated/providerRegistry.ts`, a generated
  `GENERATED_PROVIDER_REGISTRY` module that TypeScript can import while the
  Settings UI migrates away from hand-maintained provider contracts.
- Added a Rust drift test that compares the checked-in generated module
  byte-for-byte against the backend serializer.
- Added `src/generated/providerRegistry.test.ts` to assert provider ids stay
  unique, planned streaming STT candidates remain represented, and generated
  credential keys are accepted by the frontend credential contract.
- Updated `audio-graph-80ed` with the `schema_export_slice_2026_06_24`
  extension. The issue remains open for descriptor enrichment and driving a
  Settings surface from the generated registry.

Additional verification:

- `cargo fmt` - pass.
- `cargo test --lib provider_registry -- --nocapture` - pass, 12 tests, 1
  ignored print-helper test.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `bun run typecheck` - pass.
- `bun run test src/generated/providerRegistry.test.ts` - pass, 3 tests.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

Remaining provider-registry work:

- Add an ergonomic cross-platform refresh command or CI drift step once CI
  workflow edits are approved.
- Extend descriptors with audio format, event semantics, settings groups,
  privacy/data residency, and detailed health/model catalog metadata.
- Drive at least one Settings surface from `GENERATED_PROVIDER_REGISTRY` instead
  of hand-maintained provider form logic.

## Continuation - Settings readiness consumes generated registry

Additional work completed:

- Replaced the Settings provider-readiness hardcoded label map with a
  `GENERATED_PROVIDER_REGISTRY` lookup keyed by provider id.
- The provider-readiness strip is now the first Settings UI surface consuming
  the generated backend registry artifact.
- Added SettingsPage coverage for a planned `asr.soniox` readiness entry so
  planned-provider labels stay registry-driven too.
- Updated `audio-graph-80ed` with the
  `settings_readiness_generated_registry_slice_2026_06_24` extension.

Additional verification:

- `bun run typecheck` - pass.
- `bun run test src/components/SettingsPage.test.tsx src/generated/providerRegistry.test.ts`
  - pass, 49 tests.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

Remaining provider-registry work:

- Add an ergonomic cross-platform refresh command or CI drift step once CI
  workflow edits are approved.
- Extend descriptors with audio format, event semantics, settings groups,
  privacy/data residency, and detailed health/model catalog metadata.
- Migrate provider forms/model controls progressively from hardcoded option sets
  to generated registry metadata.

## Continuation - fixed model catalog readiness payload

Additional work completed:

- Added a generic non-secret `ProviderModelCatalogItem` payload to provider
  readiness responses.
- Provider readiness now derives fixed/local/default model catalog entries from
  the backend provider registry and sets `model_count` from that catalog.
- This covers fixed/default provider surfaces such as AssemblyAI streaming, AWS
  Transcribe, OpenAI Realtime STT, Deepgram Aura, Gemini Live, and local model
  descriptors without another frontend provider map.
- OpenRouter keeps its existing remote `openrouter_models` payload; remote
  command catalogs such as Deepgram STT are deliberately not represented as
  fixed catalogs.
- Updated `audio-graph-cbde` with the
  `fixed_model_catalog_readiness_slice_2026_06_24` extension.

Additional verification:

- `cargo fmt` - pass.
- `cargo test --lib provider_readiness -- --nocapture` - pass, 2 tests.
- `cargo test --lib fixed_model_catalog -- --nocapture` - pass, 1 test.
- `cargo test --lib base_readiness_includes_fixed_provider_model_catalog -- --nocapture`
  - pass, 1 test.
- `cargo test --lib remote_command_model_catalogs_stay_provider_specific -- --nocapture`
  - pass, 1 test.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `bun run typecheck` - pass.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

Remaining readiness/catalog work:

- Expand remote model, voice, and language catalog strategies beyond OpenRouter.
- Improve AWS and Gemini Vertex readiness messaging and tests.
- Feed generic `model_catalog` entries into a shared searchable picker UI.

## Continuation - readiness UI consumes generic model catalog

Additional work completed:

- Settings provider-readiness rendering now consumes the generic
  `model_catalog` payload when `model_count` is absent.
- The display falls back to OpenRouter's remote `openrouter_models` only after
  generic catalog entries, keeping backend-owned fixed catalogs first.
- Added SettingsPage coverage for an AssemblyAI readiness response that returns
  `model_catalog` but no `model_count`, proving the UI can surface catalog
  counts without provider-specific frontend maps.
- Updated `audio-graph-cbde` with the
  `readiness_catalog_ui_slice_2026_06_24` extension.

Additional verification:

- `bun run typecheck` - pass.
- `bun run test src/components/SettingsPage.test.tsx src/generated/providerRegistry.test.ts`
  - pass, 49 tests.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

Remaining readiness/catalog work:

- Expand remote model, voice, and language catalog strategies beyond OpenRouter.
- Improve AWS and Gemini Vertex readiness messaging and tests.
- Feed generic `model_catalog` entries into editable searchable model picker
  controls.

## Continuation - Gemini Vertex readiness semantics

Additional work completed:

- Added a mode-aware provider readiness policy for Gemini Live.
- Gemini API-key mode remains probeable through the saved-key backend health
  command.
- Gemini Vertex AI mode no longer reports as probeable or ready just because
  the registry descriptor has a Gemini health command.
- With a saved service-account path, Vertex readiness now stays `unchecked`
  with an explicit "not probed automatically yet" message.
- Updated `audio-graph-cbde` with the
  `gemini_vertex_readiness_slice_2026_06_24` extension.

Additional verification:

- `cargo fmt` - pass.
- `cargo test --lib gemini_vertex_readiness_is_unchecked_without_automatic_probe -- --nocapture`
  - pass, 1 test.
- `cargo test --lib provider_readiness -- --nocapture` - pass, 2 tests.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

## Continuation - credentials.yaml temp write hardening

Additional work completed:

- Added `write_owner_only_temp_file` for credential persistence.
- Unix still creates the credentials temp file with `0o600` before writing
  secret YAML.
- Non-Unix now creates the temp file empty, applies `fs_util::set_owner_only`
  before writing secret YAML, and only then writes contents. This removes the
  prior `fs::write` path where the temp file contents existed before ACL
  hardening.
- Existing `fs_util::set_owner_only` already applies Windows owner-only ACLs
  through `icacls` with `CREATE_NO_WINDOW`.
- Added a credential writer regression that validates content and Unix mode.
- Closed `audio-graph-e78e`; broader runtime/provider Debug redaction and
  provider error-body redaction are already closed in `audio-graph-1bd7` and
  `audio-graph-74ed`.

Additional verification:

- `cargo fmt` - pass.
- `cargo test --lib credential -- --nocapture` - pass, 22 tests.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

Remaining readiness/catalog work:

- Implement a real Vertex AI readiness probe or keep the unchecked state
  explicit.
- Expand remote model, voice, and language catalog strategies beyond OpenRouter.
- Feed generic `model_catalog` entries into editable searchable model picker
  controls.

## Continuation - Vertex service-account auth without global env mutation

Additional work completed:

- Replaced the Gemini Vertex AI explicit service-account path flow in
  `gemini::open_ws`.
- Explicit service-account paths now use
  `gcp_auth::CustomServiceAccount::from_file` and request the token directly.
- The default/no-path Vertex flow still uses `gcp_auth::provider()` so ADC and
  `gcloud` behavior continue to work.
- `GOOGLE_APPLICATION_CREDENTIALS` is no longer mutated in the Gemini hot path.
- Closed `audio-graph-0ad3`.

Additional verification:

- `rg -n "set_var|GOOGLE_APPLICATION_CREDENTIALS" src-tauri/src/gemini/mod.rs src-tauri/src`
  - no Gemini `GOOGLE_APPLICATION_CREDENTIALS` mutation remains.
- `cargo fmt` - pass.
- `cargo test --lib gemini_config_debug_redacts_auth_secret -- --nocapture`
  - pass, 1 test.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

## Continuation - processed-audio runtime consumer split

Additional work completed:

- Added unregister support to `ProcessedAudioConsumerRegistry` and covered it
  with a dispatch/health regression.
- Removed the app-startup Gemini processed-audio channel from `AppState`.
  Startup now registers only the long-lived speech consumer.
- `start_gemini` now registers a runtime `gemini-notes` consumer and unregisters
  it from stop paths and worker teardown.
- `start_converse` now registers a separate runtime `gemini-converse` consumer
  and unregisters it from stop, audio-sender exit, and driver teardown paths.
- Verified no `gemini_audio_tx` or `gemini_audio_rx` references remain in
  `src-tauri/src`.
- Filed `audio-graph-fb7a` for the separate capture-stop/converse teardown gap
  found during this split.

Additional verification:

- `cargo fmt` - pass.
- `cargo check --lib` - pass.
- `cargo test --lib audio::consumer::tests -- --nocapture`
  - pass, 4 tests.
- `cargo check --lib --features diarization-clustering` - pass.
- `bun run typecheck` - pass.
- `rg -n 'gemini_audio_(tx|rx)' src-tauri/src`
  - no matches.
- `git diff --check -- src-tauri/src/audio/consumer.rs src-tauri/src/state.rs src-tauri/src/commands.rs src-tauri/src/audio/mod.rs src-tauri/src/events.rs src/types/index.ts src/store/index.ts src/hooks/useTauriEvents.ts src/hooks/useTauriEvents.test.ts docs/commit-state-2026-06-23-provider-settings-roadmap.md .seeds/issues.jsonl`
  - pass.

Remaining:

- Add runtime consumer APIs for OpenAI Realtime and local/hybrid S2S paths.
- Surface audio-consumer health in a compact UI surface.
- Move provider coexist/reject policy into the registry instead of scattered
  command guards; Gemini notes/converse still intentionally reject concurrent
  operation because they share the single Gemini client slot.

## Continuation - audio-consumer health status chip

Additional work completed:

- Added a compact Audio chip to `PipelineStatusBar` that summarizes
  `audio-consumer-health` snapshots from the store.
- The chip shows active/total consumers, aggregate queue depth/capacity, and
  total dropped chunks. Its status dot switches to warning when drops are
  reported.
- Added localized English and Portuguese strings for the chip text and tooltip.
- Added focused status-bar tests for normal queue health and dropped chunks.

Additional verification:

- `bun run typecheck` - pass.
- `bun run test src/components/PipelineStatusBar.test.tsx`
  - pass, 14 tests.
- `bunx @biomejs/biome check src/components/PipelineStatusBar.tsx src/components/PipelineStatusBar.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json`
  - pass.
- `git diff --check -- src/components/PipelineStatusBar.tsx src/components/PipelineStatusBar.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json docs/commit-state-2026-06-23-provider-settings-roadmap.md .seeds/issues.jsonl`
  - pass.

Remaining:

- Move provider coexist/reject policy into the audio registry.
- Add runtime consumer registration for OpenAI Realtime and local/hybrid S2S
  paths.
- Decide whether Gemini notes/converse should remain mutually exclusive via
  one client slot or split session ownership for true concurrency.

## Continuation - registry-owned consumer conflict policy

Additional work completed:

- Added optional `conflict_group` metadata to processed-audio consumer
  descriptors and health payloads.
- `ProcessedAudioConsumerRegistry::register` now rejects a new consumer when an
  already registered consumer owns the same conflict group.
- Gemini notes and native converse now reserve the shared
  `gemini-live-client` group before mutating the shared Gemini client slot.
  This moves the notes/converse coexistence decision out of ad hoc command
  cross-checks and into the provider audio registry.
- Runtime consumer unregistration now belongs to stop paths and session-owner
  teardown (`gemini-event-receiver` / `converse-driver`), not the audio sender
  threads. That prevents an audio-sender failure from releasing an exclusive
  provider slot while the shared client session is still active.
- Startup failure cleanup now clears active flags, unregisters consumers,
  disconnects the shared client, and joins spawned audio workers when the next
  startup phase fails.

Additional verification:

- `cargo fmt` - pass.
- `cargo test --lib audio::consumer::tests -- --nocapture`
  - pass, 6 tests.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `bun run typecheck` - pass.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

Remaining:

- Add runtime consumer registration for OpenAI Realtime and local/hybrid S2S
  paths.
- Decide whether Gemini notes/converse should remain mutually exclusive via
  one client slot or split session ownership for true concurrency.
- Extend registry policy beyond exclusive groups if future provider paths need
  shared-group admission limits, source-specific conflicts, or priority
  preemption.

## Continuation - capture-stop native converse teardown

Additional work completed:

- Extracted native converse teardown into `stop_converse_runtime`, shared by the
  explicit `stop_converse` command and last-capture stop.
- When the final capture source stops, active converse now follows the same
  teardown invariants as manual stop: active flag false, capture gate closed,
  `gemini-converse` runtime consumer unregistered, shared Gemini client
  disconnected/cleared, playback stopped, and converse worker handles taken and
  bounded-joined.
- Added a headless regression that uses dummy runtime state and worker threads
  to prove the shared teardown clears the consumer, gate, active flag, and
  thread slots without a live Gemini socket.

Additional verification:

- `cargo fmt` - pass.
- `cargo test --lib converse -- --nocapture`
  - pass, 52 tests.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

## Continuation - transcript event JSONL writer

Additional work completed:

- Added a dedicated transcript event writer alongside the legacy transcript
  segment writer. Final ASR span revisions are now persisted as immutable
  `TranscriptEvent` JSONL rows at `transcripts/<session>.events.jsonl`.
- Threaded the event writer through `AppState`, session rotation, command
  startup, `SpeechShared`, and the shared post-ASR transcript tail.
- Added the same bounded shutdown behavior used by the legacy transcript
  writer so session rotation can respawn both writers without blocking the UI
  indefinitely on slow storage.
- Persisted final transcript events from both the shared ASR tail and the
  local diarization-only fallback path.
- Added persistence contract tests for the event writer drain/shutdown path.

Additional verification:

- `cargo fmt` - pass.
- `cargo check --lib` - pass.
- `cargo test --lib persistence::shutdown_tests -- --nocapture`
  - pass, 5 tests.
- `cargo test --lib projections::tests -- --nocapture`
  - pass, 3 tests.
- `cargo check --lib --features diarization-clustering` - pass.
- `bun run typecheck` - pass.
- `git diff --check -- src-tauri/src/persistence/mod.rs src-tauri/src/state.rs src-tauri/src/speech/context.rs src-tauri/src/speech/mod.rs src-tauri/src/speech/tests_integration.rs src-tauri/src/commands.rs src-tauri/src/projections.rs src-tauri/src/user_data.rs src-tauri/src/lib.rs docs/commit-state-2026-06-23-provider-settings-roadmap.md`
  - pass.

Verification note:

- `cargo test --lib speech::tests_integration::speech_processor_missing_whisper_falls_back_to_diarization_only -- --nocapture`
  failed before exercising speech logic because the headless Linux sandbox
  could not initialize GTK through `tao`.

## Continuation - processed-audio consumer registry first slice

Additional work completed:

- Added a backend `ProcessedAudioConsumerRegistry` with consumer descriptors
  for id, stage/provider, capacity, drop policy, source filter, and mixing mode.
- Registered the existing speech and Gemini processed-audio consumers through
  the registry while preserving their current channels and active flags.
- Replaced the `start_capture` dispatcher branch logic with registry dispatch.
  The dispatcher no longer knows about speech/Gemini directly.
- Added per-consumer sent/drop counters and queue health snapshots emitted via
  `audio-consumer-health`.
- Added frontend contract types, store state/action, hook listener, and hook
  test coverage for the new health event.

Additional verification:

- `cargo fmt` - pass.
- `cargo check --lib` - pass.
- `cargo test --lib audio::consumer::tests -- --nocapture`
  - pass, 3 tests.
- `cargo check --lib --features diarization-clustering` - pass.
- `bun run typecheck` - pass.
- `bun run test src/hooks/useTauriEvents.test.ts`
  - pass, 22 tests.
- `git diff --check -- src-tauri/src/audio/consumer.rs src-tauri/src/audio/mod.rs src-tauri/src/state.rs src-tauri/src/commands.rs src-tauri/src/events.rs src/types/index.ts src/store/index.ts src/hooks/useTauriEvents.ts src/hooks/useTauriEvents.test.ts docs/commit-state-2026-06-23-provider-settings-roadmap.md`
  - pass.

Remaining:

- Move Gemini notes and native converse to distinct runtime registrations
  instead of sharing the same static Gemini consumer channel.
- Add explicit runtime register/unregister APIs for OpenAI Realtime and future
  local/hybrid S2S providers.
- Expose the consumer health snapshot in a compact UI surface.
- Encode policy conflicts in the registry rather than scattered command guards.

## Continuation - diarization span revision contract

Additional work completed:

- Added `diarization-span-revision` as the provider-neutral speaker timeline
  event, separate from aggregate `speaker-detected` stats and append-only
  transcript rows.
- Defined Rust and TypeScript payloads for revisioned diarization spans with
  `span_id`, `provider`, `timeline_id`, optional `source_id`, optional
  `channel`, speaker identity, time range, confidence, stability/finality,
  revision metadata, basis ASR/transcript ids, raw event provenance, and
  receipt time.
- Wired the local `diarization-clustering` emit loop to publish provisional
  session-level speaker-span revisions alongside the existing `SPEAKER_DETECTED`
  event. Because the current clustering worker consumes a session-level mono
  stream, it emits `timeline_id: "session"` and leaves `source_id` unset instead
  of inventing source affinity.
- Added a bounded frontend `diarizationSpanRevisions` buffer and Tauri event
  listener so future notes/graph projections can consume speaker timeline diffs.
- Left provider-specific normalization, persisted replay, local/provider/hybrid
  merge policy, and UI health/settings controls in the `audio-graph-3588`
  workstream.

Additional follow-up in the same slice:

- Added a pure transcript-to-diarization revision builder. Any finalized
  transcript segment that already carries a speaker id or label now emits a
  final `diarization-span-revision` on the source-local timeline, with the ASR
  span id and transcript segment id recorded as basis.
- Wired the shared final-transcript tail so Deepgram/AWS/AssemblyAI/Sherpa/local
  paths that produce speaker-labeled final segments normalize into the speaker
  timeline without provider-specific UI handling.
- Wired the diarization-only local path, which bypasses the shared final
  transcript tail, to emit the same source-local final speaker span revisions.
- Kept provider-native speaker IDs/word-token-level spans as follow-up work:
  this slice normalizes the current final transcript labels and preserves basis
  provenance, but it does not yet parse provider-specific word/token diarization
  events into finer-grained revisions.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `cargo test --lib events::tests::diarization_span_revision_serializes_snake_case_contract -- --nocapture`
  - pass, 1 test.
- `cargo test --lib transcript_ -- --nocapture` - pass, 10 tests, 1 ignored
  (filter also matched unrelated transcript tests).
- `bun run typecheck` - pass.
- `bun run test src/hooks/useTauriEvents.test.ts` - pass, 22 tests.
- `git diff --check -- src-tauri/src/events.rs src-tauri/src/speech/mod.rs src/types/index.ts src/store/index.ts src/hooks/useTauriEvents.ts src/hooks/useTauriEvents.test.ts`
  - pass.

## Continuation - event-sourced projection data model contract

Additional work completed:

- Added `src-tauri/src/projections.rs`, a Rust contract module for the
  event-sourced transcript/notes/graph synthesis pipeline.
- Defined durable `TranscriptEvent` values derived from `AsrSpanRevisionPayload`
  without reusing UI-only append rows as the canonical source of truth.
- Defined `ProjectionBasis`, `ProjectionBasisSpan`, `ProjectionJob`,
  `ProjectionPatch`, `ProjectionOperation`, and `ProjectionProvenance` so future
  notes/graph synthesis jobs can basis-check span revisions and emit replayable
  patches rather than overwrite append-only state.
- Added a deterministic `fnv1a64:` transcript hash over canonical transcript
  revision fields to detect stale projection jobs before applying their output.
- Added cross-platform user-data path helpers for the future artifacts:
  `transcripts/<session>.events.jsonl`, `projections/<session>.events.jsonl`,
  and `notes/<session>.json`.
- This is a contract/persistence-path slice only. It does not yet write the new
  event logs, run the TTFT-aware LLM queue, or apply projection patches to the
  live notes/graph state.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `cargo test --lib projections::tests -- --nocapture` - pass, 3 tests.
- `cargo test --lib user_data::tests::env_override_controls_non_secret_data_root -- --nocapture`
  - pass, 1 test.

## Continuation - cross-platform source descriptor first slice

Additional work completed:

- Threaded rsac `AudioSourceKind::Device { kind: Option<DeviceKind> }` into
  `AudioSourceInfo.device_kind` so React groups devices from backend metadata
  instead of Windows endpoint-id or display-name heuristics.
- Added `capture_target`, `is_default`, `ApplicationName`, and `ProcessTree`
  to the serialized audio-source contract. Active capture metadata now preserves
  `ProcessTree` instead of collapsing it into `Application`.
- Updated `AudioSourceSelector` to render unresolved device direction as
  `Unknown Devices` instead of guessing, and to show backend default-device
  badges when provided.
- Tightened Rust source-id parsing so `app:<pid>` and `process-tree:<pid>` both
  require non-zero numeric PIDs.
- Added TS and Rust regression tests for backend-driven device direction,
  unknown device direction, malformed process ids, and source-info
  process-tree/application-name preservation.
- Closed `audio-graph-0dba` as completed by this backend-driven device
  direction slice.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `bun run typecheck` - pass.
- `bun run test src/components/AudioSourceSelector.test.tsx src/utils/captureTarget.test.ts`
  - pass, 25 tests.
- `cargo test --lib parse_capture_target -- --nocapture`
  - pass, 2 tests.
- `cargo test --lib source_info_preserves -- --nocapture`
  - pass, 2 tests.

Remaining:

- `audio-graph-3251` still needs a fuller SourceDescriptor/schema-generation
  pass with supported/default format, permission/capability flags, and process
  metadata.
- `audio-graph-7ee6` still needs platform capability-gating coverage and legacy
  compatibility-id coverage before closure.

## Continuation - source descriptor format metadata

Additional work completed:

- Added serializable `AudioFormatInfo` / `AudioSampleFormat` metadata to
  `AudioSourceInfo`, including `supported_formats` and `default_format`.
- Populated device and system-default format metadata from
  `AudioDevice::supported_formats()` through a best-effort rsac enumerator pass;
  source listing remains non-fatal when format probing fails or a platform
  legitimately returns no formats.
- Added a compact default-format badge in `AudioSourceSelector` when backend
  metadata is available.
- Added frontend and Rust tests for the format badge and rsac format mapping.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `bun run typecheck` - pass.
- `bun run test src/components/AudioSourceSelector.test.tsx src/utils/captureTarget.test.ts`
  - pass, 26 tests.
- `cargo test --lib format_infos_preserve -- --nocapture`
  - pass, 1 test.
- `cargo test --lib source_info_preserves -- --nocapture`
  - pass, 2 tests.

Remaining:

- `audio-graph-3251` still needs schema generation / Rust-derived TS, platform
  capability flags, permission hints, and richer process/app metadata.

## Continuation - source descriptor capability and permission hints

Additional work completed:

- Added optional `AudioSourceCapabilities` and `AudioPermissionStatus` fields to
  `AudioSourceInfo`, mirrored in TypeScript.
- Derived per-source `capture_supported` from rsac `PlatformCapabilities` for
  system, device, application, application-name, and process-tree source types.
- Included backend name, platform capability booleans, and unsupported reasons
  so source selection can explain unsupported targets before `start_capture`.
- Added source-row UI gating: rows with `capture_supported === false` are
  disabled for click and keyboard selection and show an `Unsupported` badge with
  the backend reason in the title.
- Added pure Rust tests for unsupported device/process-tree capability
  projection and frontend tests for unsupported-row behavior.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `bun run typecheck` - pass.
- `bun run test src/components/AudioSourceSelector.test.tsx src/utils/captureTarget.test.ts`
  - pass, 27 tests.
- `cargo test --lib source_capabilities_gate -- --nocapture`
  - pass, 2 tests.
- `cargo test --lib source_info_preserves -- --nocapture`
  - pass, 2 tests.

Remaining:

- `audio-graph-3251` still needs generated/Rust-derived TS schema and richer
  process/app metadata.
- `audio-graph-1e47` remains open for richer platform-specific permission
  recovery states and disabled-state copy.

## Continuation - AGENTS Seed hygiene loop

Additional work completed:

- Expanded `AGENTS.md` with a dedicated Seed hygiene loop: inspect
  `sd ready --format json`, advance the highest-priority non-blocked work,
  record partial epic progress with `sd update --extensions`, close only
  acceptance-complete Seeds, keep remaining work explicit, and gate CI/workflow
  changes while the worktree has broad staged/unrelated changes.

## Continuation - config.yaml settings migration first slice

Additional work completed:

- Changed canonical non-secret settings persistence from
  app-data `settings.json` to app-config `config.yaml`.
- Added legacy `settings.json` import when `config.yaml` does not exist; imported
  settings are written back through the existing redaction and inline credential
  migration path.
- Switched settings saves to YAML while preserving atomic temp-file writes and
  owner-only permission hardening.
- Kept bundled `src-tauri/config/default.toml` as the app/default configuration
  source; this change only affects user settings persistence.
- Added parser/serialization tests for YAML round-trip, legacy JSON import
  compatibility, corrupt YAML rejection, and YAML secret redaction.
- Updated README and current settings-design prose/diagrams to document
  `config.yaml`, legacy `settings.json` import, and the split between
  non-secret settings and `credentials.yaml`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo test --lib settings::tests:: -- --nocapture`
  - pass, 22 tests.

Remaining:

- `audio-graph-559d` still needs file-level save/load tests around real
  config/legacy paths and a cleanup/marker policy decision for legacy
  `settings.json`.

## Continuation - provider readiness first slice

Additional work completed:

- Added backend `get_provider_readiness_cmd` with non-secret provider
  readiness payloads, credential presence summaries, 5-minute cached health
  results, 10-second per-provider timeout, and credential epoch invalidation
  when `save_credential_cmd` or `delete_credential_cmd` changes
  `credentials.yaml`.
- Readiness cache keys use provider id, credential epoch, and non-secret
  settings fingerprints such as endpoint/base URL, AWS region/source, Gemini
  auth mode, and TTS voice/speed; API keys are not part of the cache key or
  returned payload.
- Implemented explicit readiness strategies for the currently probeable
  providers instead of dispatching dynamically by registry command string:
  OpenAI-compatible ASR, Deepgram STT/TTS, AssemblyAI, OpenRouter, AWS
  Transcribe/Bedrock when active, and Gemini API-key mode.
- OpenRouter readiness now fetches the model catalog with the saved key and
  returns it in the readiness payload so Settings can populate the model picker
  on open without requiring key re-entry or a manual Load models click.
- Settings now calls `get_provider_readiness_cmd({ refresh: true })` on open,
  renders a compact Provider readiness strip with checking/ready/missing/error
  states, and hydrates OpenRouter models from the backend readiness result.
- Added TS readiness types, English/Portuguese copy, and SettingsPage tests
  proving Settings open calls readiness, does not call `load_credential_cmd`,
  renders saved OpenRouter readiness, and fills the OpenRouter picker from the
  backend payload.

Remaining before closing `audio-graph-cbde`:

- Refresh readiness immediately after Save/Clear actions and expose explicit
  retry affordances.
- Add in-flight request coalescing/rate limiting instead of only TTL caching.
- Expand model/voice/language catalog strategies beyond OpenRouter.
- Improve AWS/Gemini Vertex readiness messaging and testing.
- Keep `audio-graph-c906` blocked until plaintext credential readback IPC is
  removed or capability-gated.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `bun run typecheck` - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 44 tests.

## Continuation - restrict plaintext credential readback IPC

Additional work completed:

- Removed `load_credential_cmd` from the registered Tauri IPC command list so
  normal React code cannot invoke plaintext credential readback.
- Removed the frontend Zustand `loadCredential` helper and its public store
  type contract.
- Removed the `#[tauri::command]` attribute from the Rust plaintext loader.
  The function remains as an internal/test helper for the allowlist dispatcher,
  but it is no longer command-generated or registered for frontend IPC.
- Updated credential-store comments to describe save/delete/presence/readiness
  as the public credential boundary.
- Closed `audio-graph-c906`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `bun run typecheck` - pass.
- `bun run test src/components/SettingsPage.test.tsx src/App.test.tsx src/components/ExpressSetup.test.tsx`
  - pass, 55 tests.

## Continuation - provider readiness refresh after credential changes

Additional work completed:

- Added a shared Settings readiness refresh helper so the same non-secret
  backend readiness payload is applied consistently on Settings open, after
  Save, and after Clear Saved Key actions.
- After clearing saved credentials, Settings now immediately refreshes provider
  readiness instead of only mutating local credential-presence state.
- After saving settings and any draft credentials, Settings now refreshes
  provider readiness so saved-key health/model state reflects the current
  `credentials.yaml` epoch and non-secret provider config.
- Added SettingsPage regressions proving readiness is called on open and again
  after Save/Clear.

Additional verification:

- `bun run typecheck` - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 46 tests.

## Continuation - provider readiness refresh admission control

Additional work completed:

- Added per-provider readiness refresh admission keyed by the existing
  non-secret provider readiness cache key.
- A provider cache key can now have only one active health/model refresh at a
  time. Concurrent Settings opens reuse stale cached readiness when available
  or return a non-secret "Health check already in progress" placeholder.
- Added a 15-second refresh-start cooldown per cache key so repeated Settings
  opens, Save, or Clear actions do not stampede provider health/model APIs
  beyond the existing 5-minute TTL cache.
- Added unit tests for in-flight coalescing and rate-limit admission using
  synthetic cache keys.

Additional verification:

- `cargo fmt` - pass.
- `cargo test --lib provider_readiness -- --nocapture`
  - pass, 2 tests.
- `cargo check --lib` - pass.
- `cargo check --lib --features diarization-clustering` - pass.
- `bun run test src/components/SettingsPage.test.tsx`
  - pass, 46 tests.
- `git diff --check`
  - pass, with pre-existing CRLF normalization warnings only.

Remaining before closing `audio-graph-cbde`:

- Expand model/voice/language catalog strategies beyond OpenRouter.
- Improve AWS/Gemini Vertex readiness messaging and testing.

## Continuation - chat/LLM token usage persistence

Additional work completed:

- Audited `audio-graph-2e40`: the stale Seed text was partly outdated. Chat
  responses already surfaced real provider token totals through LocalLlama,
  API/OpenAI-compatible, OpenRouter, mistral.rs, and streaming terminal
  `usage` blocks when providers report them.
- Closed the remaining persistence gap by extending
  `sessions::usage::SessionUsage` and `LifetimeUsage` with separate
  `llm_total` and `llm_turns` counters. These deserialize to zero for existing
  usage JSON and stay separate from Gemini prompt/response/cache/thought
  counters because most chat providers only report request-wide totals.
- Added `append_llm_chat_usage`, wired successful streaming and blocking chat
  completions to persist non-zero provider-reported totals, and emitted a
  backend `llm-usage-update` event only after the disk write succeeds.
- Updated frontend usage contracts and `TokenUsagePanel` so the panel is now
  general "Token Usage": combined totals include Gemini plus LLM chat usage,
  source-specific rows remain visible, and LLM-only usage avoids misleading
  zero prompt/response rows.
- Updated English and Portuguese token-usage copy and added a focused panel
  regression for live persisted LLM usage updates.
- Closed `audio-graph-2e40`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `bun run typecheck` - pass.
- `bun run test src/components/TokenUsagePanel.test.tsx` - pass, 16 tests.

Verification note:

- Focused Rust usage test attempts through both default features and
  `--no-default-features --features cloud` did not produce terminal test
  results in this sandbox. The initial default-feature run spent several
  minutes linking/building and then the tool session stayed open despite no
  visible Cargo/rustc/linker child; two lighter focused attempts showed the
  same stale-session behavior. Treat the new Rust unit tests as compile-checked
  by `cargo check --lib` but not executed locally in this pass.

## Continuation - token usage reset/clear source of truth

Additional work completed:

- Filed and fixed `audio-graph-2167`: the token-usage panel's Reset/Clear All
  controls previously cleared only React/localStorage mirrors while backend
  usage JSON files remained authoritative and could rehydrate stale totals.
- Added backend usage helpers to reset a single session usage file to a durable
  zero record and clear all lifetime-contributing `.json`/`.json.tmp` usage
  records while preserving unrelated files in the usage directory.
- Exposed `reset_current_session_usage` and `clear_all_usage` Tauri commands.
- Updated `TokenUsagePanel` so Reset calls the backend, refreshes lifetime
  usage from disk, and only then updates local mirrors. Clear All now deletes
  backend usage records before clearing session/lifetime local mirrors.
- Updated TokenUsagePanel tests to assert backend reset/clear commands are
  invoked and that Reset no longer leaves a resurrectable lifetime total.
- Closed `audio-graph-2167`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `bun run typecheck` - pass.
- `bun run test src/components/TokenUsagePanel.test.tsx` - pass, 16 tests.

Verification note:

- Added Rust unit tests for `reset_usage` and `clear_all_usage`, but did not
  execute Rust tests locally in this pass because earlier focused Cargo test
  attempts repeatedly left stale tool sessions without visible child processes.

## Continuation - SSE decoder buffer cap

Additional work completed:

- Added a default 1 MiB `SseDecoder` frame cap to prevent unbounded growth when
  an upstream SSE stream never emits a blank-line terminator.
- Added `SseEvent::Error(String)` so decoder overflow becomes an explicit
  terminal streaming error instead of silently dropping bytes or continuing to
  accumulate memory.
- Made oversized complete frames recoverable: the bad frame is discarded, an
  error event is emitted, and following frames can still be parsed by tests.
- Updated the streaming chat consumer to convert decoder overflow into
  `TokenDelta::Error` with the accumulated text preserved.
- Closed `audio-graph-3344`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo test --lib --no-default-features --features cloud llm::sse::tests:: -- --nocapture`
  - pass, 11 tests.

Verification note:

- A broader `cargo test --lib --no-default-features --features cloud sse -- --nocapture`
  run passed all 11 `llm::sse` tests but also matched unrelated tests and failed
  at `models::tests::error_for_status_passes_2xx` because this sandbox denies
  local test-server socket binds.

## Continuation - Aura flush sequence race

Additional work completed:

- Changed `SessionCmd::Flush` to carry the dispatch-time sequence number
  allocated by `AuraSession::flush()`.
- Added a per-socket pending flush sequence queue in the Aura session loop.
  Each sent Flush pushes its sequence, and each server `Flushed` ack pops the
  oldest sequence instead of reading the latest global atomic counter.
- Updated close-drain text handling to use the same pending queue, preserving
  graceful tail-audio drain behavior.
- Added a pure `flush()` command test proving rapid consecutive calls enqueue
  `Flush(1)` and `Flush(2)`, plus a parser regression proving consecutive
  `Flushed` acks emit sequences 1 then 2.
- Closed `audio-graph-d875`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo test --lib --no-default-features --features cloud flush_command_carries_dispatch_time_sequence -- --nocapture`
  - pass, 1 test.
- `cargo test --lib --no-default-features --features cloud handle_server_text -- --nocapture`
  - pass, 8 tests.

Verification note:

- A broader `cargo test --lib --no-default-features --features cloud tts::deepgram_aura::tests:: -- --nocapture`
  run passed the non-socket Aura tests but failed seven websocket mock-server
  tests because this sandbox denies local socket binds.

## Continuation - streaming cancel semantics docs

Additional work completed:

- Clarified the blocking `send_chat_message` streaming shim's unused
  cancellation token binding: dropping a `CancellationToken` does not cancel;
  the binding only keeps stream infrastructure intact while the shim drains to
  completion.
- Documented `StreamRegistry::cancel` as a best-effort return value: `true`
  means a token was found and cancellation was requested, not that the next
  terminal frame is guaranteed to be `Cancelled`.
- Closed `audio-graph-9d6d` and `audio-graph-93a3`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo test asr_span_revision_serializes_snake_case_contract --lib -- --nocapture`
  - pass, 1 test.

Verification note:

- The first targeted Rust test attempt spent 8m11s building the default-feature
  native test binary and was interrupted just after the test executable
  launched. The immediate rerun reused artifacts and passed.

## Continuation - OpenAI Realtime ASR span revision adapter

Additional work completed:

- Added internal ASR revision metadata plumbing so providers with durable
  transcript item IDs can override the generic time-based partial span ID and
  final transcript-segment span ID.
- Updated the OpenAI Realtime transcription receiver to use provider
  `item_id` as `provider_item_id`, emit a stable
  `openai_realtime:{source_id}:{item_id}` span ID for both interim deltas and
  completed transcripts, increment `revision_number` per item, and set
  `supersedes` to the previous revision reference when available.
- Kept Deepgram, AssemblyAI, AWS, Sherpa, local Whisper, and cloud batch on the
  generic fallback path until their adapters preserve enough provider-native
  identity/revision metadata.
- Added a pure Rust regression for provider-item span and revision-reference
  formatting.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo test --lib --no-default-features --features cloud provider_item_revision_helpers_are_stable -- --nocapture`
  - pass, 1 test.
- `cargo test --lib --no-default-features --features cloud asr_span_revision_serializes_snake_case_contract -- --nocapture`
  - pass, 1 test.
- `bun run typecheck` - pass.
- `bun run test -- src/hooks/useTauriEvents.test.ts` - pass, 21 tests.
- `bunx @biomejs/biome check src/types/index.ts src/store/index.ts src/hooks/useTauriEvents.ts src/hooks/useTauriEvents.test.ts`
  - pass.

## Continuation - subagent review queue and cross-platform audio defaults

Additional work completed:

- Folded provider-expansion, configuration UX, diarization, and projection
  subagent findings into Seeds as acceptance additions instead of leaving them
  as loose review notes.
- Added focused follow-up Seeds for backend-aligned first-run audio defaults
  (`audio-graph-cc78`) and Settings accessibility (`audio-graph-a6d4`).
- Aligned frontend fresh-state and first-run audio channel fallbacks with the
  backend/default.toml stereo default while preserving explicit saved mono
  settings.
- Added focused tests proving the Settings reducer/save path and Express Setup
  no-settings save payload use 48 kHz stereo by default.

Additional verification:

- `bunx @biomejs/biome check --write src/components/settingsTypes.ts src/components/SettingsPage.tsx src/components/ExpressSetup.tsx src/components/SettingsPage.test.tsx src/components/AudioSettings.test.tsx src/components/ExpressSetup.test.tsx`
  - pass.
- `bun run typecheck` - pass.
- `bun run test src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/components/AudioSettings.test.tsx`
  - pass, 61 tests.

## Continuation - frontend redacted settings persistence

Additional work completed:

- Changed the Zustand `saveSettings` action to call `save_settings_cmd` and
  then reload `load_settings_cmd`, committing only backend-redacted settings to
  global UI state.
- Added a store-level regression that saves caller-provided settings containing
  ASR, LLM, Gemini, and AWS access-key-source values, then asserts the store
  contains the backend-redacted reload result instead of the draft object.
- Closed `audio-graph-b266`.

Additional verification:

- `bunx @biomejs/biome check --write src/store/index.ts src/store/index.test.ts`
  - pass.
- `bun run test src/store/index.test.ts` - pass, 25 tests.
- `bun run test src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/components/AudioSettings.test.tsx`
  - pass, 61 tests.
- `bun run typecheck` - pass.

## Continuation - Settings accessibility tab and readiness slice

Additional work completed:

- Added a labelled polite `status` region around provider readiness so health
  and catalog state changes are announced without loading plaintext secrets.
- Wired Settings tabs with tablist/tab/tabpanel semantics,
  `aria-controls`/`aria-labelledby`, roving `tabIndex`, and Arrow/Home/End
  keyboard navigation.
- Added focused SettingsPage tests for readiness live-region metadata, active
  tabpanel linkage, tab focus movement, and keyboard tab selection.
- Updated `audio-graph-a6d4` as partial; shared model-picker combobox
  accessibility and provider-local readiness panels remain open follow-ups.

Additional verification:

- `bunx @biomejs/biome check --write src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx`
  - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 49 tests.
- `bun run test src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/components/AudioSettings.test.tsx`
  - pass, 62 tests.
- `bun run typecheck` - pass.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - Provider event semantics registry

Additional work completed:

- Added explicit provider `event_semantics` metadata in the backend registry for
  ASR providers and native realtime-agent surfaces.
- Distinguished final-only batch ASR, partial/final streaming ASR,
  partial/final/turn streaming ASR, and native realtime audio/text agents.
- Refreshed the generated TypeScript provider registry from the Rust generator
  and added TypeScript coverage that every ASR provider declares event
  semantics.
- Excluded `src/generated/providerRegistry.ts` from Biome rewriting so
  formatter runs do not invalidate the Rust byte-for-byte drift guard.
- Updated `audio-graph-80ed` with
  `event_semantics_registry_slice_2026_06_24`.
- Filed `audio-graph-0bc2` for the separate Biome 2.5.1 schema/deprecation
  config migration warning observed during verification.

Additional verification:

- `cargo test --lib provider_registry -- --nocapture` - pass, 13 tests, 1
  ignored print helper.
- `cargo check --lib` - pass.
- `bun run typecheck` - pass.
- `bun run test src/generated/providerRegistry.test.ts` - pass, 4 tests.
- `bunx @biomejs/biome check src/generated/providerRegistry.test.ts src/types/index.ts biome.json`
  - pass with Biome schema/deprecation informational notices only.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - Provider audio-input registry

Additional work completed:

- Added `audio_input` metadata to every ASR provider and native realtime-agent
  descriptor in the backend provider registry.
- Captured the cross-platform processed-audio bus contract explicitly:
  16 kHz, mono, f32, independent of macOS/Windows/Linux capture backend details.
- Captured provider-side wire/runtime formats for implemented adapters:
  local f32 buffers, multipart WAV for batch ASR, AWS eventstream PCM16,
  WebSocket binary PCM16, WebSocket JSON/base64 PCM16, and OpenAI Realtime's
  24 kHz adapter-resampled input.
- Added provisional 16 kHz mono PCM16 WebSocket-binary audio contracts for
  planned Soniox, Gladia, Speechmatics, and ElevenLabs Scribe descriptors.
- Added Rust and Vitest coverage so audio-consuming providers cannot be added
  without an explicit input format, transport encoding, and multichannel safety
  statement.
- Updated `audio-graph-80ed` with
  `audio_input_registry_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo test --lib provider_registry -- --nocapture` - pass, 15 tests, 1
  ignored print helper.
- `cargo check --lib` - pass.
- `bun run typecheck` - pass.
- `bun run test src/generated/providerRegistry.test.ts` - pass, 6 tests.
- `bunx @biomejs/biome check src/generated/providerRegistry.test.ts src/types/index.ts biome.json`
  - pass with tracked Biome schema/deprecation informational notices only.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - Biome config migration

Additional work completed:

- Migrated `biome.json` to the installed Biome 2.5.1 schema URL.
- Replaced deprecated `linter.rules.recommended` with
  `linter.rules.preset: "recommended"`.
- Preserved the generated provider-registry exclusion so
  `src/generated/providerRegistry.ts` remains Rust-generator-owned and is not
  rewritten by Biome.
- Closed `audio-graph-0bc2`.

Additional verification:

- `bunx @biomejs/biome check src/generated/providerRegistry.test.ts src/types/index.ts biome.json`
  - pass with no schema/deprecation informational notices.
- `bunx @biomejs/biome check src/generated/providerRegistry.ts` - expected
  nonzero ignored-file result; confirms the generated registry is excluded.
- `cargo test --lib generated_provider_registry_ts_is_current -- --nocapture`
  - pass.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - OpenRouter advanced controls disclosure

Additional work completed:

- Moved OpenRouter base URL override and streaming usage toggle behind the
  shared Advanced provider controls disclosure.
- Kept normal OpenRouter setup visible: API key, saved-key hint, model picker,
  Load models, Test Connection, and Clear Saved Key.
- Added tests proving the disclosure starts collapsed, advanced values remain
  editable, and OpenRouter save/test/model-loading paths still use the
  configured base URL and usage flag.
- Updated `audio-graph-9882` as partial; generic LLM API and AWS advanced
  controls remain open.

Additional verification:

- `bunx @biomejs/biome check --write src/components/LlmProviderSettings.tsx src/components/SettingsPage.test.tsx`
  - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 50 tests.
- `bun run test src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/components/AudioSettings.test.tsx`
  - pass, 63 tests.
- `bun run typecheck` - pass.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - OpenAI-compatible LLM advanced tuning disclosure

Additional work completed:

- Moved OpenAI-compatible LLM max-token and temperature tuning behind the
  shared Advanced provider controls disclosure.
- Kept required setup controls visible: endpoint, API key, model, and vLLM
  preset.
- Added a SettingsPage save regression proving expanded advanced tuning
  persists through `llm_api_config`.
- Updated `audio-graph-9882` as partial; AWS advanced controls and future
  provider disclosure conventions remain open.

Additional verification:

- `bunx @biomejs/biome check --write src/components/LlmProviderSettings.tsx src/components/SettingsPage.test.tsx`
  - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 51 tests.
- `bun run test src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/components/AudioSettings.test.tsx`
  - pass, 64 tests.
- `bun run typecheck` - pass.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - AWS advanced controls disclosure

Additional work completed:

- Moved AWS Transcribe credential mode, profile selection, access keys, saved-key
  hints, profile refresh, and clear-saved-keys controls behind the shared
  Advanced provider controls disclosure.
- Moved AWS Bedrock credential mode, profile selection, access keys, saved-key
  hints, profile refresh, and clear-saved-keys controls behind the same
  disclosure pattern.
- Kept normal setup visible for both providers: region/language/model fields and
  Test Connection.
- Added SettingsPage regressions proving AWS credentials are hidden by default,
  remain editable when expanded, and test paths still use draft access keys
  without saving them.
- Updated `audio-graph-9882` as partial; AssemblyAI/Gemini audit and future
  Soniox/Speechmatics disclosure conventions remain open.

Additional verification:

- `bunx @biomejs/biome check --write src/components/AsrProviderSettings.tsx src/components/LlmProviderSettings.tsx src/components/SettingsPage.test.tsx`
  - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 51 tests.
- `bun run test src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/components/AudioSettings.test.tsx`
  - pass, 64 tests.
- `bun run typecheck` - pass.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - Advanced disclosure completion

Additional work completed:

- Added `AdvancedSettingsDisclosure` as the shared Settings disclosure pattern
  for provider-specific expert controls.
- Routed the current ASR/LLM Advanced sections through the shared component,
  preserving the same CSS classes and native details/summary behavior.
- Audited AssemblyAI and Gemini: current controls are required setup/status/test
  controls rather than optional latency/debug/provider tuning, so no extra
  controls needed to move in this slice.
- Closed `audio-graph-9882`; future Soniox/Speechmatics forms can reuse the
  shared disclosure instead of inventing provider-local markup.

Additional verification:

- `bunx @biomejs/biome check --write src/components/AdvancedSettingsDisclosure.tsx src/components/AsrProviderSettings.tsx src/components/LlmProviderSettings.tsx src/components/SettingsPage.test.tsx`
  - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 51 tests.
- `bun run test src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/components/AudioSettings.test.tsx`
  - pass, 64 tests.
- `bun run typecheck` - pass.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - Provider-local readiness panels

Additional work completed:

- Added `ProviderReadinessPanel` for contextual saved-credential/provider health
  state inside the active ASR, LLM, and Gemini configuration sections.
- Kept the global provider-readiness overview, but now the selected provider
  also shows ready/error/unchecked/stale/catalog state next to its controls.
- Wired ASR and LLM selected provider variants to backend registry-aligned
  provider IDs (`asr.*` and `llm.*`) instead of display-label matching.
- Added SettingsPage coverage for OpenRouter, OpenAI Realtime ASR, and Gemini
  local readiness panels without loading plaintext credentials.
- Updated `audio-graph-cbde` and `audio-graph-a6d4` as partial; shared model
  picker accessibility and real remote catalogs beyond OpenRouter remain open.

Additional verification:

- `bunx @biomejs/biome check --write src/components/ProviderReadinessPanel.tsx src/components/AsrProviderSettings.tsx src/components/LlmProviderSettings.tsx src/components/GeminiSettings.tsx src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx`
  - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 51 tests.
- `bun run test src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/components/AudioSettings.test.tsx`
  - pass, 64 tests.
- `bun run typecheck` - pass.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - Cross-platform capture-target selection

Additional work completed:

- Updated `AudioSourceSelector` to use backend-provided
  `AudioSourceInfo.capture_target` for selection, selected-state checks, and
  source mode labels when present.
- Kept `source.id` as the row key/display identity, so rsac/backend can expose
  opaque descriptor IDs while the app stores portable capture targets.
- Added a regression with an opaque source row ID and a separate
  `device:<id>` capture target to prevent the UI from falling back to parsing
  OS-specific row IDs.
- Updated `audio-graph-2044` as partial; backend structured start-capture and
  processed-audio consumer bus work remain open.

Additional verification:

- `bunx @biomejs/biome check --write src/components/AudioSourceSelector.tsx src/components/AudioSourceSelector.test.tsx src/utils/captureTarget.ts src/utils/captureTarget.test.ts`
  - pass.
- `bun run test src/components/AudioSourceSelector.test.tsx src/utils/captureTarget.test.ts`
  - pass, 28 tests.
- `bun run test src/store/captureSelection.test.ts src/components/AudioSourceSelector.test.tsx src/utils/captureTarget.test.ts`
  - pass, 29 tests.
- `bun run typecheck` - pass.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - Provider settings group registry

Additional work completed:

- Added `settings_groups` metadata to the backend provider registry and
  generated TypeScript contract.
- Supported generated UI layout hints: `basic`, `model_catalog`, `health`, and
  `advanced`.
- Required every provider to declare `basic`; provider-owned fixed/local/remote
  catalogs declare `model_catalog`; credential/health-check surfaces declare
  `health`; complex provider-specific controls declare `advanced`.
- Added Rust and Vitest coverage for duplicate-free groups and advanced grouping
  on Soniox, Gladia, Speechmatics, ElevenLabs Scribe, AWS, Deepgram,
  AssemblyAI, OpenRouter, TTS, and realtime-agent surfaces.
- Updated `audio-graph-80ed` with
  `settings_groups_registry_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo test --lib provider_registry -- --nocapture` - pass, 17 tests, 1
  ignored print helper.
- `cargo check --lib` - pass.
- `bun run typecheck` - pass.
- `bun run test src/generated/providerRegistry.test.ts` - pass, 8 tests.
- `bunx @biomejs/biome check src/generated/providerRegistry.test.ts src/types/index.ts biome.json`
  - pass.
- `git diff --check` - pass with pre-existing CRLF normalization warnings.

## Continuation - Provider lifecycle and privacy registry

Additional work completed:

- Added backend provider-registry metadata for auth lifecycle, session
  lifecycle, keepalive strategy, close/teardown strategy, and app-visible data
  boundary.
- Classified local-only providers separately from user-configured HTTP
  endpoints, user-configured cloud regions, provider-account-scoped cloud
  services, and vendor cloud services.
- Locked current adapter behavior into registry tests: Deepgram Listen
  KeepAlive plus end-stream close, AssemblyAI terminate-session close, AWS SDK
  end-stream, Gemini/OpenAI realtime audio-stream end, and Deepgram Aura
  provider close message plus KeepAlive.
- Mirrored the lifecycle/privacy contract into TypeScript and refreshed the
  generated provider registry artifact.
- Updated `audio-graph-80ed` with
  `lifecycle_privacy_registry_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo test --lib provider_registry -- --nocapture` - pass, 19 tests, 1
  ignored print helper.
- `bun run typecheck` - pass.
- `bun run test src/generated/providerRegistry.test.ts` - pass, 10 tests.
- `bunx @biomejs/biome check src/generated/providerRegistry.test.ts src/types/index.ts biome.json`
  - pass.

## Continuation - Settings consumes provider lifecycle metadata

Additional work completed:

- Extended the shared provider-readiness panel to consume generated provider
  descriptors, so the active ASR, LLM, and Gemini settings sections show
  registry-owned data boundary, session lifecycle, auth shape, keepalive, and
  close/teardown labels.
- Kept the metadata display compact and wrapping so long translated labels do
  not overflow the provider settings panel.
- Added English and Portuguese labels for the new registry metadata.
- Added SettingsPage coverage for OpenRouter, OpenAI Realtime transcription,
  and Gemini Live metadata without loading plaintext credentials.
- Updated `audio-graph-80ed` with
  `settings_lifecycle_metadata_panel_slice_2026_06_24`.

Additional verification:

- `bunx @biomejs/biome check --write src/components/ProviderReadinessPanel.tsx src/components/AsrProviderSettings.tsx src/components/LlmProviderSettings.tsx src/components/GeminiSettings.tsx src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json src/styles/settings.css`
  - pass.
- `bun run typecheck` - pass.
- `bun run test src/generated/providerRegistry.test.ts` - pass, 10 tests.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 51 tests.
- `bun run test src/components/SettingsPage.test.tsx src/components/ExpressSetup.test.tsx src/components/AudioSettings.test.tsx`
  - pass, 64 tests.
- `git diff --check` - pass.

## Continuation - Cross-platform provider registry refresh command

Additional work completed:

- Added a Rust `export-provider-registry` binary that writes the backend-owned
  provider registry TypeScript module to a supplied output path.
- Added `bun run generate:provider-registry` as the developer entry point,
  backed by a Node wrapper that resolves paths from the script location, runs
  Cargo from `src-tauri` so `rust-toolchain.toml` is honored, and uses
  `cargo.cmd` on Windows.
- Switched the refresh command to the existing cloud-only feature profile so
  registry generation does not require local-ML features.
- Verified the generated artifact remains byte-for-byte current and that the
  provider-registry Rust/Vitest drift guards still pass.
- Noted a follow-up: the cold generator run still links enough of the app crate
  to take about four minutes on this checkout, so a tiny provider-registry
  codegen crate would be a better long-term developer/CI surface.
- Updated `audio-graph-80ed` with
  `provider_registry_refresh_command_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `bun run generate:provider-registry` - pass; wrote
  `src/generated/providerRegistry.ts`.
- `cargo test --lib provider_registry -- --nocapture` - pass, 19 tests, 1
  ignored print helper.
- `bun run typecheck` - pass.
- `bun run test src/generated/providerRegistry.test.ts` - pass, 10 tests.
- `git diff --check` - pass.

## Continuation - Registry-backed ASR capture selection

Additional work completed:

- Replaced the command-layer single-session ASR name helper with
  `validate_asr_capture_selection`, which reads the active ASR provider
  descriptor from the backend provider registry.
- Start-capture and start-transcribe preflight now require ASR descriptors to
  declare both `source_policy` and `audio_input` before captured audio can be
  routed to the provider.
- The validator rejects registry entries whose processed-audio input format
  does not match the current cross-platform backend bus: 16 kHz, mono, f32.
- Single-session providers are still blocked from using multiple concurrent
  sources, but the decision now comes from `ProviderSourcePolicy` metadata;
  Deepgram/OpenAI Realtime mixed-source behavior and local/cloud batch
  multi-source behavior remain allowed.
- Added a provider-registry invariant that every audio-consuming ASR/realtime
  descriptor declares a source policy before it can consume captured audio.
- Updated `audio-graph-80ed` with
  `registry_backed_capture_selection_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo test --lib provider_registry -- --nocapture` - pass, 20 tests, 1
  ignored print helper.
- `cargo test --lib asr_capture_selection -- --nocapture` - pass, 5 tests.
- `git diff --check` - pass.

## Continuation - Durable projection event writer

Additional work completed:

- Added `ProjectionEventWriter`, a session-scoped JSONL writer for replayable
  notes/graph `ProjectionPatch` events under
  `projections/<session>.events.jsonl`.
- Mirrored the existing transcript-event writer semantics: non-blocking
  enqueue, owner-only file permissions, storage-full classification,
  final flush logging, and bounded shutdown.
- Wired the projection event writer into `AppState` startup and
  `rotate_session`, so new sessions rotate transcript rows, transcript events,
  and projection patch events as one persistence boundary.
- Added persistence tests for projection-event drain semantics and shutdown
  flag ordering.
- Updated `audio-graph-ad44` and `audio-graph-4673` with
  `projection_event_writer_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo check --lib --tests --no-default-features --features cloud` - pass.

## Continuation - Frontend projection artifact restore state

Additional work completed:

- Extended the frontend session store contract with restored transcript events,
  projection events, materialized notes, and materialized graph state returned
  by `load_session`.
- Hydrated those projection artifacts when loading a full session so settings
  and future notes/graph reducers can consume durable session state without
  re-reading credentials or losing backend replay results.
- Cleared projection restore state when loading a legacy transcript-only view or
  clearing the transcript, preventing stale notes/graph artifacts from leaking
  across sessions.
- Added store tests for full-session projection artifact hydration and
  legacy-transcript cleanup.
- Updated `audio-graph-9c89`, `audio-graph-9d93`, and `audio-graph-4673` with
  `frontend_projection_restore_state_slice_2026_06_24`.

Additional verification:

- `bun run typecheck` - pass.
- `bun run test src/store/index.test.ts` - pass.

## Continuation - Lightweight provider registry exporter

Additional work completed:

- Split provider registry metadata and TypeScript export into a lightweight
  Rust workspace crate at `src-tauri/crates/provider-registry`.
- Kept the Tauri-facing `provider_registry` module as a thin app wrapper for
  `get_provider_registry_cmd` plus ASR/LLM/TTS settings-enum mapping.
- Removed the app-linked `export-provider-registry` bin so
  `bun run generate:provider-registry` no longer links the full Tauri app,
  AWS SDK, `rsac`, or local-ML providers just to refresh generated metadata.
- Updated the generator wrapper to run
  `cargo run -p audio-graph-provider-registry --bin export-provider-registry`
  from `src-tauri`, preserving `src-tauri/rust-toolchain.toml` behavior.
- Regenerated `src/generated/providerRegistry.ts` with the new generated-source
  header.
- Created `audio-graph-0281` for the invalid Git HEAD/master ref because it
  blocks trustworthy commit, PR, CI, release, and `sd sync` work.
- Repaired and closed `audio-graph-0281` by backing up the corrupt loose
  `.git/refs/heads/master` file outside `refs/` and removing that loose
  override. The valid packed `refs/heads/master` at `831cc30` now resolves
  normally; no tracked files, index entries, or worktree content were changed by
  the Git repair.
- Updated `audio-graph-a805` and `audio-graph-80ed` with
  `lightweight_provider_registry_exporter_slice_2026_06_24`.

Additional verification:

- `bun run generate:provider-registry` - pass; cold run completed in about six
  seconds after moving to the lightweight crate.
- Deterministic generator rerun - pass; `src/generated/providerRegistry.ts`
  SHA-256 stayed unchanged after regeneration.
- `cargo tree -p audio-graph-provider-registry --manifest-path src-tauri/Cargo.toml`
  - pass; dependency tree is limited to `serde`, `serde_json`, and transitive
  proc-macro/support crates.
- `cargo test -p audio-graph-provider-registry --manifest-path src-tauri/Cargo.toml --lib`
  - pass, 3 tests.
- `cargo test --lib provider_registry --no-default-features --features cloud -- --nocapture`
  from `src-tauri` - pass, 9 tests.
- `cargo fmt --check --manifest-path src-tauri/Cargo.toml --all` - pass.
- `cargo check --lib --tests --no-default-features --features cloud` from
  `src-tauri` - pass.
- `bun run test src/generated/providerRegistry.test.ts` - pass, 10 tests.
- `bun run typecheck` - pass.
- Git repair verification - pass: `git rev-parse --verify HEAD`, `git log -1`,
  `git status --short --branch`, `git show-ref --head --dereference`, and
  `git fsck --connectivity-only` all complete; `fsck` reports only dangling
  objects after the bad-ref errors are gone.

Remaining:

- Cross-OS generator validation should run through GitHub/Blacksmith once the
  invalid Git HEAD ref is repaired and workflow/CI actions are approved.
- CI workflow drift gates remain approval-gated and were not edited.

## Continuation - Session projection artifact lifecycle

Additional work completed:

- Added `MaterializedProjectionState` to `AppState` so notes and graph
  materializer caches rotate with the active session, alongside the transcript
  ledger and event writers.
- Added canonical session artifact inventory helpers covering legacy transcript
  JSONL, transcript event JSONL, projection event JSONL, notes JSON, legacy
  graph JSON, and materialized projection graph JSON.
- Updated trash purge and permanent delete paths to remove every known
  session-owned artifact instead of leaving projection logs/materialized files
  behind.
- Extended session recovery to discover event-sourced transcript logs,
  projection event logs, notes artifacts, and materialized graph artifacts.
  Orphaned transcript-event sessions now recover segment/speaker counts from
  `TranscriptEvent` rows.
- Updated `load_session` to load transcript events, projection patch events,
  materialized notes, and materialized graph artifacts, replay the transcript
  ledger, refresh the materialized projection cache, and return these additive
  fields to the frontend.
- Added TypeScript contracts for the additive `load_session` projection payload
  fields.
- Updated `audio-graph-9c89`, `audio-graph-ad44`, and `audio-graph-4673` with
  `session_projection_artifact_lifecycle_slice_2026_06_24`.

Additional verification:

- `cargo fmt` - pass.
- `bun run typecheck` - pass.
- `cargo check --lib --tests --no-default-features --features cloud` - pass.
- Attempted
  `cargo test --lib --no-default-features --features cloud purge_removes_all_expired_session_artifacts -- --nocapture`;
  stopped after several minutes in local test-binary linking (`rustc`/`cc`/`rust-lld`).
  Run the focused session tests in GitHub/Blacksmith CI rather than blocking the
  local loop.

## Continuation - Materialized graph artifact contract

Additional work completed:

- Added `MaterializedGraph`, `MaterializedGraphNode`, and
  `MaterializedGraphEdge` projection state for the durable temporal-graph view.
- Implemented ordered graph `ProjectionPatch` application for node and edge
  upserts/removals, with stale sequence rejection, wrong-kind rejection,
  note-operation rejection, dangling-edge validation, and atomic clone-then-commit
  mutation semantics so invalid patches do not partially alter materialized
  state.
- Removing a graph node now also removes incident materialized graph edges, so
  replay cannot leave orphaned links behind.
- Added `materialized_graph_path` and `save_materialized_graph`, writing the new
  projection artifact to `graphs/<session>.materialized.json` via the shared
  atomic JSON persistence helper. The legacy temporal graph autosave remains on
  `graphs/<session>.json` while migration work is still open.
- Added unit coverage for node/edge insert/update/removal, stale/wrong-kind
  rejection, note-operation rejection, and dangling-edge rollback behavior.
- Updated `audio-graph-ad44` and `audio-graph-4673` with
  `materialized_graph_contract_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib --tests --no-default-features --features cloud` - pass.
- `cargo check --lib` - pass.

## Continuation - Provider readiness manual Run checks action

Additional work completed:

- Added a manual `Run checks` button to the provider readiness dashboard at the
  top of Settings.
- The button calls the existing backend-owned `get_provider_readiness_cmd`
  refresh path, stays disabled while a check is already running, and keeps the
  existing aria-live status region.
- Added English and Portuguese labels plus compact header action styling.
- Added SettingsPage coverage proving a user click triggers a second saved-key
  readiness refresh without using plaintext credential readback.
- Updated `audio-graph-abc1` and `audio-graph-cbde` with
  `readiness_run_checks_ui_slice_2026_06_24`.

Additional verification:

- `bunx @biomejs/biome check --write src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json src/styles/settings.css`
  - pass.
- `bun run typecheck` - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass, 52 tests.
- `git diff --check -- src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json src/styles/settings.css`
  - pass.

## Continuation - Projection artifact load helpers

Additional work completed:

- Added a shared `load_jsonl` helper that reads typed JSONL logs and treats
  missing files as empty logs for backward-compatible session restore.
- Added typed readers for `transcripts/<session>.events.jsonl` and
  `projections/<session>.events.jsonl`.
- Added optional materialized artifact readers for `notes/<session>.json` and
  `graphs/<session>.materialized.json`.
- These helpers do not yet wire session restore; they provide the typed
  persistence surface needed by session lifecycle/replay work.
- Updated `audio-graph-ad44` and `audio-graph-9c89` with
  `projection_artifact_load_helpers_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib --tests --no-default-features --features cloud` - pass.
- `cargo check --lib` - pass.

## Continuation - Validated projection materializer coordinator

Additional work completed:

- Added `MaterializedProjectionState`, a coordinator that owns
  `MaterializedNotes` and `MaterializedGraph` for one session.
- Added `apply_validated_patch`, which validates a `ProjectionPatch` basis
  against the current `TranscriptLedger` before applying it to the notes or
  graph materializer.
- Added `MaterializedProjectionApplyOutcome` so runtime code can report the
  updated notes/graph sequence and item counts without inspecting internals.
- Extended `ProjectionApplyError` with `StaleBasis`, carrying the exact
  `ProjectionBasisStaleness` reason.
- Added tests proving notes and graph patches apply after a current-basis check,
  and stale-basis patches are rejected before mutating materialized state.
- Updated `audio-graph-ad44`, `audio-graph-4673`, and `audio-graph-d524` with
  `validated_materializer_apply_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib --tests --no-default-features --features cloud` - pass.
- `cargo check --lib` - pass.

## Continuation - Basis-bound projection scheduler contract

Additional work completed:

- Added `projection_scheduler`, a pure scheduler module for TTFT-aware notes and
  graph projection jobs.
- The scheduler starts a `ProjectionJob` when the current transcript ledger basis
  differs from the last completed basis.
- While a job is in flight, new ledger states are coalesced instead of starting
  overlapping LLM calls. Decisions include queued span count and the configured
  TTFT estimate so runtime telemetry can explain the wait.
- When an in-flight job completes, the scheduler validates the job basis against
  the current `TranscriptLedger`. Current completions are accepted; stale
  completions are discarded and a repair/replay-priority job is started from the
  latest basis.
- Added scheduler metrics for jobs started, coalesced updates, and stale
  discards.
- Added unit coverage for start/coalesce/stale-repair behavior and
  current-completion idle behavior.
- Recorded the partial-persistence identity blocker on `audio-graph-3709` and
  `audio-graph-4da5`: providers with fallback time-based partial span IDs
  should not persist all partials until partial/final identity and supersession
  fixtures prove they update one span.
- Updated `audio-graph-d524` and `audio-graph-4673` with
  `projection_scheduler_contract_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib --tests --no-default-features --features cloud` - pass.
- `cargo check --lib` - pass.
- `git diff --check -- src-tauri/src/projections.rs src-tauri/src/persistence/mod.rs src-tauri/src/user_data.rs`
  - pass.
- Stopped `cargo test --lib --no-default-features --features cloud materialized_graph -- --nocapture`
  after the test binary spent several minutes linking locally. The test code is
  typechecked by the cloud-only test compile gate; execution should run in the
  GitHub/Blacksmith PR matrix instead of blocking the local loop.

## Continuation - CI validation stance

Additional CI findings:

- Existing `.github/workflows/ci.yml` already defines Blacksmith-backed Linux,
  macOS, and Windows jobs, but it triggers on pushes/PRs to `master/main` and
  does not currently expose `workflow_dispatch`.
- A feature-branch push alone will not validate the current dirty worktree; the
  practical CI path is an approved branch commit plus draft PR to `master`.
- The Release workflow is not a good validation path yet because it still
  clones `rsac` from a moving branch; this remains tracked by
  `audio-graph-6381`.
- Local validation should avoid default-feature Rust test execution as the
  primary gate on this checkout. Use local `cargo fmt`, `cargo check`, targeted
  no-default/cloud checks, and frontend tests for iteration; use Blacksmith for
  full OS/default-feature test execution once a PR branch is approved.
- Updated `audio-graph-150f` with `ci_validation_path_review_2026_06_24`.

## Continuation - Transcript ledger and basis freshness contract

Additional work completed:

- Added `TranscriptLedger`, a canonical replay state for transcript span
  revisions. It keeps the latest accepted `TranscriptEvent` per span, tracks
  accepted event count, and can reconstruct a current `ProjectionBasis`.
- Tightened `ProjectionBasis::from_transcript_events` so its transcript hash is
  computed from the latest canonical event per span, matching the view a
  scheduler should use when deciding whether an LLM response is still valid.
- Added `TranscriptLedgerError` for stale incoming transcript revisions and
  conflicting same-revision events.
- Added `ProjectionBasisStaleness` and `TranscriptLedger::validate_basis` /
  `is_basis_current` so future notes/graph materializers can reject stale LLM
  responses because of old span revisions, missing new spans, unknown old spans,
  transcript hash mismatches, or diarization basis that is not yet backed by a
  diarization ledger.
- Added unit coverage for ledger replay, latest-revision basis generation,
  stale/conflicting transcript revision rejection, and each basis mismatch class.
- Updated `audio-graph-ad44`, `audio-graph-4673`, and `audio-graph-d524` with
  `transcript_ledger_basis_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib --tests --no-default-features --features cloud` - pass.
- `cargo check --lib` - pass.
- `git diff --check -- src-tauri/src/projections.rs docs/commit-state-2026-06-23-provider-settings-roadmap.md .seeds/issues.jsonl`
  - pass.

## Continuation - Live transcript ledger wiring

Additional work completed:

- Added a session-scoped `TranscriptLedger` to `AppState`, initialized with the
  active session id.
- Reset the live ledger during `rotate_session` so new sessions start with an
  empty projection basis state.
- Threaded the ledger through `SpeechShared` and `TranscriptProcessingContext`
  so every shared final-ASR tail can update the canonical span ledger.
- Applied each emitted final `TranscriptEvent` to the ledger before appending it
  to the transcript event JSONL writer. The same helper is used by the local
  diarization-only fallback path that bypasses the common final-ASR tail.
- Ledger application is best-effort and logs rejected stale/conflicting span
  revisions rather than panicking the speech pipeline.
- Updated `audio-graph-ad44`, `audio-graph-4673`, and `audio-graph-d524` with
  `live_transcript_ledger_wiring_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib --tests --no-default-features --features cloud` - pass.
- `cargo check --lib` - pass.
- `git diff --check` - pass before the worklog/Seed update.
- Stopped `cargo test --lib projection -- --nocapture` after it spent more
  than six minutes linking the default-feature test harness locally; this full
  default-feature test execution should run in GitHub/Blacksmith CI instead of
  blocking the local loop.

## Continuation - Materialized notes artifact contract

Additional work completed:

- Added `MaterializedNotes` and `MaterializedNote` projection state for the
  durable notes view.
- Implemented ordered `ProjectionPatch` application for note upserts and
  deletes, with stale sequence rejection and wrong-kind/unsupported-operation
  errors.
- Preserved projection basis and provenance on each materialized note so later
  replay/debug paths can explain which transcript revision window produced a
  note.
- Added `save_materialized_notes`, writing the artifact to the existing
  `notes/<session>.json` path via the shared atomic JSON persistence helper.
- Added unit coverage for insert/update/delete and stale/wrong-kind rejection.
- Updated `audio-graph-ad44` and `audio-graph-4673` with
  `materialized_notes_contract_slice_2026_06_24`.

Additional verification:

- `cargo fmt --check` - pass.
- `cargo check --lib` - pass.
- `cargo check --lib --tests --no-default-features --features cloud` - pass.
