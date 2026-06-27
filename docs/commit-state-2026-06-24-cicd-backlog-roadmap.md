# Commit State - 2026-06-24 - CI/CD and backlog roadmap continuation

**Timestamp:** 2026-06-24T12:40:00-07:00  
**HEAD:** `831cc30` (`master`) - `fix: address 6 real CodeRabbit findings from the PR review`

## Purpose

This document is the current P1 state handoff for the active roadmap/backlog
push. It supersedes the stale parts of
`docs/commit-state-2026-06-23-provider-settings-roadmap.md`: Git history is no
longer broken, but the worktree remains broad and dirty. Treat current files and
Seeds as authoritative; do not infer a clean commit boundary from the index.

## HEAD commit

Latest committed change:

- `831cc30` fixed six CodeRabbit review findings.
- Files changed in HEAD: `src-tauri/src/commands.rs`,
  `src-tauri/src/converse/mod.rs`, `src-tauri/src/diarization/worker.rs`,
  `src-tauri/src/gemini/mod.rs`, `src-tauri/src/llm/api_client.rs`,
  `src-tauri/src/speech/mod.rs`.
- HEAD branch state reported by `git log -1 --decorate --oneline`:
  `831cc30 (HEAD -> master, origin/stack-5-bugfixes,
  origin/deep-work-loop-2026-05-31, stack-5-bugfixes,
  deep-work-loop-2026-05-31)`.

## Dirty worktree summary

The checkout has staged and unstaged work spanning CI, provider registry,
credentials/settings, ASR/TTS providers, S2S/converse, projection persistence,
audio capture, UX, localization, docs, and Seeds.

Do not run `sd sync` or create a broad commit from the current index without
first isolating ownership. Several files are `MM`, meaning they have both
staged pre-existing changes and later unstaged changes.

High-level diff size observed:

- Unstaged/tracked diff: 67 files, about 12,148 insertions and 2,091 deletions.
- Staged diff: 75 files, about 6,670 insertions and 669 deletions.
- Additional untracked files include the new provider registry/codegen path,
  projection pipeline modules, provider settings helpers, generated registry
  output, research docs, and this repo's `AGENTS.md`.

## Current CI/CD work

Changes completed in this continuation:

- `.github/workflows/release.yml`
  - Removed the moving `RSAC_REPO_REF=master` release input.
  - Added `workflow_dispatch.inputs.rsac_sha`, defaulting to the same pinned
    full SHA used by CI: `a2d3088b0ae8050d1ce79966298cc792c6694ec2`.
  - Validates the selected rsac SHA is a full 40-character commit hash.
  - Checks out `rsac` by detached SHA and records requested/actual revisions in
    the GitHub step summary.
  - Writes a per-platform rsac revision manifest for release metadata.
  - Moves create-release and build jobs to Blacksmith runner families.
  - Pins `actions/checkout`, `actions/github-script`, `dtolnay/rust-toolchain`,
    `oven-sh/setup-bun`, `tauri-apps/tauri-action`, `actions/upload-artifact`,
    and `ilammy/msvc-dev-cmd` by SHA.
  - Adds dry-run behavior that builds local release artifacts and uploads them
    to Actions artifacts without creating/uploading a GitHub Release.
  - Keeps release publish and standalone upload steps gated away from dry-run.
  - Removes the unpinned PipeWire PPA from the release Linux job.

- `.github/workflows/ci.yml`
  - Adds `workflow_dispatch`.
  - Adds a Blacksmith Linux/macOS/Windows cloud-only smoke matrix.
  - The matrix fetches pinned rsac, installs Bun, runs
    `cargo check --no-default-features --features cloud`,
    `cargo test --no-default-features --features cloud -- --test-threads=1`,
    and `bun run tauri build --no-bundle --ci -- --no-default-features
    --features cloud`.
  - Linux cloud-only tests run under `xvfb-run` for Tauri/GTK initialization.
  - Windows cloud-only keeps `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST` scoped to
    `cargo test`; the Tauri release compile smoke runs without the test
    manifest env var.

- `.github/actionlint.yaml`
  - Adds the custom Blacksmith runner labels so `actionlint` validates workflow
    syntax without suppressing unrelated checks.

Seeds touched by this CI/CD slice:

- Closed `audio-graph-6381`: release rsac no longer comes from a moving branch.
- Closed `audio-graph-150f`: PR 21 run `28126263189` passed the Blacksmith
  Linux/macOS/Windows cloud-only smoke matrix and the existing CI jobs.
- Closed `audio-graph-f98e`: Linux cloud-only cargo test uses `xvfb-run` and
  passed in PR 21 run `28125630762`.
- Closed `audio-graph-2b06`: Windows cloud-only Tauri compile smoke no longer
  inherits the test-manifest env and passed in PR 21 run `28126263189`.
- Closed `audio-graph-4078`: release dry-run Tauri version mismatch fixed;
  release run `28125549025` passed Linux/macOS/Windows.
- Updated `audio-graph-2586`: release Blacksmith/pinned-action/dry-run path is
  dry-run verified; non-dry test-tag publish remains.
- Updated `audio-graph-c395`: cross-platform CI/CD epic progress recorded.
- Updated `audio-graph-5b75`, `audio-graph-74b2`, `audio-graph-8eeb` with
  partial-progress notes for cloud-only CI, Tauri smoke, and PPA reduction.

## Current projection-runtime work

Changes completed after the CI/CD slice:

- `src-tauri/src/state.rs`
  - Added `AppState::apply_runtime_projection_patch`.
  - Requires the caller to pass the queued session id and queued
    `ProjectionBasis`, so LLM-returned patches cannot silently rewrite their
    basis or cross a session boundary.
  - Clones the current `TranscriptLedger` for stale-basis validation, applies
    patches to a cloned `MaterializedProjectionState`, persists the affected
    materialized artifact, enqueues the accepted `ProjectionPatch` event, then
    commits the cloned materialized state only if the active session is still
    the expected session.
  - Rejects rotation-in-progress, session mismatch, model basis mismatch,
    stale basis, artifact save failure, missing projection event writer, enqueue
    failure, and session changes during apply as structured runtime errors.
  - Added focused tests for accepted notes, accepted materialized graph, and
    stale-basis rejection with no state or disk mutation.

- `src-tauri/src/persistence/mod.rs`
  - `ProjectionEventWriter::append` now returns whether enqueue succeeded, so
    runtime apply can fail fast instead of claiming an accepted event was queued
    when the writer channel is closed.

- `src-tauri/src/projections.rs`
  - Added trusted replay for already-accepted `ProjectionPatch` logs via
    `MaterializedProjectionState::apply_replayed_patch` and
    `replay_accepted_patches`.
  - Replay intentionally does not validate old accepted patches against the
    final `TranscriptLedger`, because later transcript spans would make earlier
    accepted patches appear stale. Historical replay-basis validation remains a
    separate hardening item requiring transcript-event history and ordering.

- `src-tauri/src/commands.rs`
  - `load_session` now replays projection events and uses replayed notes/graph
    state when the materialized artifact is missing or behind the patch log.
  - Existing materialized artifacts still win when they are at least as current
    as replayed state.
  - Command tests now drain projection-event writer threads in addition to
    transcript writers.

Seeds touched by this projection slice:

- Updated `audio-graph-4673` with `runtime_projection_apply_slice_2026_06_24`.
  The epic remains open: TTFT scheduler dispatch, structured LLM patch parsing,
  historical replay-basis validation, frontend retcon reducers, and replay/eval
  metrics still remain.
- Updated `audio-graph-4673` with `projection_event_replay_slice_2026_06_24`
  for trusted session-restore replay from accepted patch events.

## Current ASR span-revision work

Changes completed after the projection slice:

- `src-tauri/src/speech/mod.rs`
  - Added deterministic start-time span ids for providers that do not expose a
    stable native result/item id.
  - Added deterministic sequence-based span ids for providers that need an
    app-owned session-local utterance identity.
  - Added shared helpers for advancing partial revisions and finalizing a span
    revision chain.
  - Deepgram interim transcripts now emit normalized ASR span revisions with a
    stable source/start span id, revision number, supersedes reference, and raw
    provider event reference.
  - Deepgram final transcripts now use the meta-aware transcript path with the
    same span id chain, so the final revision supersedes the latest partial
    instead of creating a separate UUID-only canonical span.
  - The current AssemblyAI legacy `PartialTranscript`/`FinalTranscript`
    receiver now creates one session-local turn span per active utterance.
    Partials and final transcripts share that span id, revision chain, turn id,
    and raw event references.
  - AWS Transcribe partial and final callbacks now preserve AWS `ResultId` as
    provider item identity. The speech processor uses that id for span identity,
    falls back to source/start identity when absent, and shares one revision map
    across AWS partial and final callbacks.
  - sherpa-onnx streaming now creates a source-local synthetic utterance id for
    partial and endpoint-final events. The first partial opens an active span,
    the endpoint final consumes it, and final-without-partial creates a one-shot
    span.
  - Removed the old generic `emit_asr_partial` fallback helper after all current
    streaming providers moved to metadata-aware partial emission.

- `src-tauri/src/asr/aws_transcribe.rs`
  - `AwsTranscribePartial` now includes `provider_item_id`.
  - Added `AwsTranscribeFinal`, which carries the final `TranscriptSegment` plus
    the provider item id for the result that produced it.
  - Unit tests assert `ResultId` survives both partial and final normalization.

- `src-tauri/src/projections.rs`
  - Added provider-shaped transcript ledger replay fixtures for OpenAI Realtime,
    Deepgram, current AssemblyAI legacy, AWS Transcribe, sherpa-onnx, and
    Soniox.
  - The fixtures prove a persisted partial revision plus final revision replays
    into one canonical latest span per utterance, not duplicate transcript spans.

- `src-tauri/src/asr/soniox.rs`
  - Added a parser-only Soniox realtime STT adapter. It does not open a
    WebSocket and does not expose Soniox in Settings yet.
  - Models Soniox v5 incremental token semantics: final tokens accumulate in
    one active app-owned turn span, non-final tokens replace the current tail,
    and `<end>`, `<fin>`, or `finished` close the turn.
  - Preserves consistent token speaker/language metadata, provider audio
    progress metadata, and provider error metadata such as error type and
    request id.
  - Inline tests cover partial-to-final supersession, mixed final/non-final
    token evolution, endpoint marker handling, finish handling, mixed speakers,
    empty responses, provider errors, and invalid JSON.

Subagent audit:

- Zeno completed a read-only AWS/Sherpa span-revision audit.
- AWS Transcribe has native `ResultId` available in SDK results, but the current
  callback payloads previously dropped it before `speech/mod.rs` could build
  span metadata. This is now fixed for the current AWS path.
- Sherpa/local streaming has no durable provider item id in the current wrapper
  and now uses an app-owned per-source utterance sequence fallback.
- Current implemented streaming providers now have stable partial-to-final span
  identity, and replay fixtures now prove those revisions collapse into one
  canonical span per provider. Remaining `audio-graph-3709` work is raw parser
  fixtures for new protocols/providers, AssemblyAI Universal Streaming v3
  `Turn`/`SpeakerRevision`, future Soniox, and speaker-timeline integration.

Seeds touched by this ASR slice:

- Updated `audio-graph-3709` with
  `deepgram_span_revision_identity_slice_2026_06_24`. The epic remains open for
  AssemblyAI, AWS, Sherpa/local, and parser/replay fixtures.
- Updated `audio-graph-3709` with
  `assemblyai_legacy_span_revision_identity_slice_2026_06_24`. The legacy
  AssemblyAI receiver path is stable, but Universal Streaming v3 `Turn` and
  `SpeakerRevision` parser support remains open.
- Updated `audio-graph-3709` with `aws_sherpa_span_revision_audit_2026_06_24`.
  After the AWS implementation slice, Sherpa/local is the remaining
  current-provider identity blocker.
- Updated `audio-graph-3709` with
  `aws_result_id_span_revision_identity_slice_2026_06_24`. AWS `ResultId`
  now drives partial/final span identity.
- Updated `audio-graph-3709` with
  `sherpa_sequence_span_revision_identity_slice_2026_06_24`. sherpa-onnx now
  uses source-local synthetic utterance identity.
- Updated `audio-graph-3709` with
  `provider_partial_replay_fixture_slice_2026_06_24`. Provider-shaped replay
  fixtures now prove current streaming partial/final chains do not duplicate
  canonical transcript spans.
- Updated `audio-graph-3709` with
  `soniox_parser_replay_fixture_slice_2026_06_24`. Soniox parser output now
  maps into the normalized ASR span-revision contract and the provider-shaped
  replay fixture.
- Updated `audio-graph-e35f` with
  `soniox_parser_fixture_slice_2026_06_24`. The parser-first slice is verified;
  live WebSocket transport, saved-key health/model discovery, Settings
  selection, and env-gated smoke remain open.
- Updated `audio-graph-ad1d` with
  `soniox_parser_first_progress_2026_06_24` so the streaming STT expansion
  roadmap distinguishes parser readiness from selectable provider readiness.
- Updated `audio-graph-4da5` partial-persistence blocker notes: OpenAI
  Realtime, Deepgram, the current AssemblyAI legacy receiver, and AWS Transcribe
  plus sherpa-onnx now have stable partial-to-final span identity. The blocker
  is now migration/downstream lifecycle work, not current-provider identity or
  basic replay proof.

## Current partial-ASR persistence work

Changes completed after the Soniox/parser slice:

- `src-tauri/src/speech/mod.rs`
  - `emit_asr_partial_with_meta` now records accepted partial ASR span revisions
    through the same canonical `TranscriptLedger` and `TranscriptEventWriter`
    path as final ASR span revisions.
  - Partial ASR revisions still do not write legacy `TranscriptSegment` rows,
    emit `TRANSCRIPT_UPDATE`, or trigger extraction.
  - `record_asr_span_revision_event` now returns ledger acceptance and appends
    to the transcript event writer only after the ledger accepts the revision.
  - Stale or conflicting ASR revisions are rejected before normalized ASR span
    events are emitted.
  - Shared final transcript emission now gates normalized ASR and diarization
    span events on ledger acceptance.
  - Follow-up cleanup moved canonical final ASR acceptance before legacy final
    transcript side effects. Stale/conflicting finals rejected by
    `TranscriptLedger` now return before legacy transcript row append,
    `TRANSCRIPT_UPDATE`, speaker/diarization emission, agent proposal, status
    update, extraction, or scheduler observation.
  - Local diarization final span revisions now use the shared canonical helper
    instead of duplicating ledger/writer logic.
  - Added focused unit tests for accepted partial-to-final revision progression
    and stale partial rejection.
  - Added writer-backed roundtrip coverage proving accepted partial/final ASR
    span revisions flush through `TranscriptEventWriter`, reload through
    `load_transcript_events`, rejected stale revisions do not append JSONL rows,
    and partial persistence does not create a legacy transcript segment file.

Seeds touched by this partial-ASR persistence slice:

- Updated `audio-graph-4673` with
  `partial_asr_revision_persistence_slice_2026_06_24`. The event-sourced
  notes/graph pipeline now has durable partial ASR revision input available for
  future TTFT-aware projection scheduling.
- Updated `audio-graph-4da5` with
  `partial_revision_persistence_slice_2026_06_24`. The transcript ledger work
  is now partially verified for live partial revision ingestion.
- Updated `audio-graph-4673` and `audio-graph-4da5` with the
  `partial_asr_revision_writer_roundtrip_slice_2026_06_24` /
  `partial_revision_writer_roundtrip_slice_2026_06_24` follow-up. Writer-backed
  JSONL roundtrip coverage is now verified; frontend reducers, migration rules,
  and CI remain open.
- Updated `audio-graph-4da5` and `audio-graph-4673` with
  `canonical_final_rejection_cleanup_2026_06_24`. Legacy final transcript
  behavior is now gated on canonical ledger acceptance; AppHandle-backed
  integration coverage for skipped UI/extraction side effects remains open.

## Current projection-scheduler observer work

Changes completed after the partial-ASR persistence slice:

- `src-tauri/src/projection_scheduler.rs`
  - Added a `ProjectionSchedulers` container that owns separate notes and graph
    scheduler instances for one session.
  - Added shared observation output so runtime code can observe both queues
    together without dispatching LLM calls yet.

- `src-tauri/src/state.rs`
  - `AppState` now owns session-scoped notes/graph projection schedulers.
  - Scheduler state is initialized with the active session and reset on
    `rotate_session` alongside `TranscriptLedger` and
    `MaterializedProjectionState`.
  - Added a focused rotation test proving scheduler jobs and in-flight state
    reset with the new session.

- `src-tauri/src/commands.rs`
  - Speech worker startup passes the scheduler handle through `SpeechShared`.
  - `load_session` resets scheduler state after restoring the loaded session's
    ledger/materialized projection state. Direct command-level test coverage is
    still a follow-up because there is not yet a lightweight `State` harness in
    the current test module.

- `src-tauri/src/speech/context.rs`,
  `src-tauri/src/speech/mod.rs`, and
  `src-tauri/src/speech/tests_integration.rs`
  - Speech workers now receive the scheduler handle.
  - Accepted canonical ASR span revisions observe the current ledger only when
    they are final or end-of-turn.
  - Accepted partial revisions remain durable ledger/event input but do not
    start projection jobs, preventing one LLM job per partial token.
  - Later eligible revisions coalesce while notes/graph jobs are in flight.
  - Legacy transcript extraction and `GRAPH_DELTA` behavior are unchanged in
    this slice.

Seeds touched by this projection-scheduler observer slice:

- Updated `audio-graph-d524` with
  `runtime_projection_scheduler_observer_slice_2026_06_24`. The TTFT queue now
  has live observer state; structured LLM patch generation and materializer
  dispatch remain open.
- Updated `audio-graph-4673` with
  `runtime_projection_scheduler_observer_route_2026_06_24` so the higher-level
  event-sourced notes/graph epic points to the verified scheduler-observer
  route.
- Updated `audio-graph-4da5` with
  `runtime_scheduler_observer_unblock_2026_06_24` to record that durable
  partial/final ASR revisions are now sufficient input for first-stage runtime
  scheduling.

## Current cloud-only local-ML availability work

Changes completed after the projection-scheduler observer slice:

- `src-tauri/src/error.rs`
  - Added structured `AppError::ProviderUnavailable` with a provider name and
    required recovery feature.
  - Added serialization/display coverage so the frontend receives a stable
    `provider_unavailable` code instead of parsing prose.

- `src-tauri/src/commands.rs`
  - Added command-layer provider availability helpers for compiled-out local
    providers.
  - Cloud-only `LocalWhisper` startup now returns `provider_unavailable` before
    local model-path checks, so a missing model file is not reported for a
    provider that is absent from the build.
  - `load_llm_model` now returns `provider_unavailable` for compiled-out
    `LocalLlama` before local model-path checks.
  - `start_streaming_chat` and `send_chat_message` reject compiled-out
    `LocalLlama` and `MistralRs` selections with structured errors instead of
    falling back to generic unsupported-provider prose.
  - Runtime transcription still only logs unavailable local LLM selections for
    extraction because extraction can fall back to another analyzer path.

- `src/types/index.ts`, `src/utils/errorToMessage.ts`, and
  `src/utils/errorToMessage.test.ts`
  - Added the typed frontend error payload and user-facing formatting for
    `provider_unavailable`.

- `README.md` and `docs/adr/0007-feature-gate-local-ml.md`
  - Documented default local-ML vs cloud-only build modes.
  - Documented that cloud-only builds omit `whisper-rs`, `llama-cpp-2`, and
    `mistralrs`, and that selecting a compiled-out local provider reports the
    feature needed to recover.

Seeds touched by this cloud-only availability slice:

- Updated `audio-graph-5fe7` with
  `cloud_only_provider_unavailable_slice_2026_06_24`. The structured local-ML
  provider-unavailable behavior is locally verified; clean GitHub/Blacksmith
  Linux/macOS/Windows cloud-only validation remains before closing the
  cross-platform acceptance.
- Updated `audio-graph-5b75` with
  `cloud_only_provider_unavailable_and_docs_slice_2026_06_24`. The docs and
  cloud-only command behavior now agree; clean-branch Blacksmith validation
  remains before closing the CI Seed.

## Current Seeds integrity and parsing repair

Changes completed after the cloud-only availability slice:

- Backed up `.seeds/issues.jsonl` to
  `/tmp/audio-graph-seeds-backups/issues.20260624T231850Z.jsonl` before repair.
  The backup has 123 lines and SHA-256
  `0dc952e21aa10e610dc129647ee13689ff9a9b028c406ad297b2b01fe417caf1`.
- Ran `sd doctor --fix` after `sd doctor` reported 20 bidirectional dependency
  mismatches. The fix added missing reverse dependency links; no Seed entries
  were removed.
- Created and closed `audio-graph-2e71` to record the repair evidence.
- Found a separate large-output parsing failure: direct pipes such as
  `sd ready --format json | jq` truncated large JSON output even though file
  redirection produced complete JSON.
- Patched the locally installed Seeds CLI helper at
  `/home/codeseys/.bun/install/global/node_modules/@os-eco/seeds-cli/src/output.ts`
  so JSON output retries partial and `EAGAIN` stdout writes until the full
  payload is emitted. This is a local tool patch, not a repo-tracked source
  change.
- Created `audio-graph-3926` as the durability follow-up to upstream this
  large JSON pipe-output fix or capture it in a versioned bootstrap path.

Seeds touched by this Seeds repair slice:

- Closed `audio-graph-2e71`: `sd doctor` is clean, `.seeds/issues.jsonl`
  parses, and the current provider-registry work extensions remained present.
- Updated `audio-graph-2e71` with
  `large_json_pipe_parse_repair_2026_06_24` after fixing direct
  `sd ready`/`sd blocked`/`sd list` JSON pipelines locally.
- Created `audio-graph-3926`: keep open until the Seeds CLI stdout fix is
  durable beyond this machine-local global package edit.

## Current provider registry required-feature and drift-check work

Changes completed after the Seeds repair slice:

- `src-tauri/crates/provider-registry/src/lib.rs`
  - Added `required_features` metadata to provider descriptors.
  - Local runtime providers now declare the Cargo features needed to make them
    selectable: `asr.local_whisper`, `asr.sherpa_onnx`, `llm.local_llama`, and
    `llm.mistralrs`.
  - Added registry coverage to prevent local providers from silently omitting
    their feature requirements.

- `src-tauri/src/commands.rs`
  - Added the compiled-out `SherpaOnnx` availability preflight so cloud-only
    builds return structured `provider_unavailable` with required feature
    `sherpa-streaming` instead of falling through to local setup errors.

- `src/generated/providerRegistry.ts`, `src/generated/providerRegistry.test.ts`,
  and `src/types/index.ts`
  - Regenerated the TypeScript provider registry with `required_features`.
  - Added frontend registry coverage for local runtime provider feature
    metadata.

- `src-tauri/crates/provider-registry/src/bin/export_provider_registry.rs`,
  `scripts/generate-provider-registry.mjs`, and `package.json`
  - Added `bun run check:provider-registry`, a non-mutating drift check for the
    generated registry. Check mode fails on schema/codegen drift without
    rewriting the generated file.

Seeds touched by this provider registry slice:

- Updated `audio-graph-80ed` with
  `provider_registry_required_features_and_check_slice_2026_06_24`.
- Updated `audio-graph-a805` with
  `provider_registry_check_command_slice_2026_06_24`.
- Updated `audio-graph-5fe7` with
  `provider_registry_required_features_2026_06_24`.

## Current provider registry closure work

Changes completed after the required-feature/drift-check slice:

- Closed `audio-graph-80ed` after a direct acceptance audit and an independent
  read-only subagent audit both found the provider registry acceptance met.
- Removed an obsolete disabled `#[cfg(any())]` duplicate app-test module from
  `src-tauri/crates/provider-registry/src/lib.rs`. The active lightweight crate
  tests and app-wrapper tests now define the real registry test surface.
- Clarified `audio-graph-a805`: an explicit `check:provider-registry` CI step
  is not required to close `audio-graph-80ed` because existing CI cargo tests
  already run the byte-for-byte generated TypeScript drift test. The explicit
  package script remains useful for local and future workflow ergonomics.
- Ran `sd doctor --fix` after closing `audio-graph-80ed`; Seeds preserves
  bidirectional dependency links even when the blocker is closed, while
  `sd ready` treats closed blockers correctly.

Seeds touched by this provider registry closure slice:

- Closed `audio-graph-80ed`: provider descriptors cover current
  ASR/LLM/TTS/Gemini/OpenAI Realtime surfaces, descriptor tests catch drift,
  generated TypeScript drives Settings, and CI schema drift is covered by
  existing cargo test jobs.
- Updated `audio-graph-80ed` with `closure_audit_2026_06_24`.
- Updated `audio-graph-a805` with `ci_drift_coverage_clarification_2026_06_24`.

## Current configuration readiness force-refresh work

Changes completed after the provider registry closure slice:

- `src-tauri/src/commands.rs`
  - `get_provider_readiness_cmd` now accepts optional `force`.
  - `force=true` bypasses the recent-check cooldown but still preserves
    in-flight coalescing, so a user-triggered recovery action can get a fresh
    provider result without creating overlapping checks.
  - Settings-open checks still use the cached/debounced path.

- `src/components/SettingsPage.tsx`
  - Manual `Run checks`, post-save readiness refresh, and clear-saved-key
    recovery refresh now call `get_provider_readiness_cmd` with
    `{ refresh: true, force: true }`.
  - The initial Settings-open readiness load still calls
    `{ refresh: true }`, preserving the debounced/cached behavior for opening
    the dialog.

- `src/components/SettingsPage.test.tsx`
  - Tests now pin the difference between Settings-open readiness and explicit
    user-triggered force refreshes.

Seeds touched by this configuration slice:

- Updated `audio-graph-1c2f` with
  `manual_provider_readiness_force_refresh_slice_2026_06_24`.

## Current structured projection-patch draft work

Changes completed after the configuration readiness force-refresh slice:

- `src-tauri/src/projection_llm.rs`
  - Added a backend-owned structured draft schema for notes/graph projection
    patches. The model output may contain only `operations` and optional
    `confidence`.
  - `ProjectionPatchDraft` uses `deny_unknown_fields`, so model-supplied
    trusted metadata such as `sequence`, `basis`, `llm_request_id`,
    `provenance`, `created_at_ms`, or session data is rejected instead of
    ignored.
  - Prompt construction validates the `ProjectionJob` basis against the
    current `TranscriptLedger` before building messages, then encodes the
    transcript basis window as JSON.
  - Trusted patch construction stamps `sequence`, `kind`, `basis`,
    `llm_request_id`, `provenance`, and `created_at_ms` from backend runtime
    context rather than model JSON.
  - Draft validation rejects malformed replacement prose, wrong-kind
    operations, blank required operation fields, invalid confidence, and
    out-of-range graph edge weights.

- `src-tauri/src/projections.rs`
  - `ProjectionOperation` now derives `schemars::JsonSchema` so the draft
    schema can expose the operation contract to JSON-mode/structured-output
    LLM clients.

- `src-tauri/src/llm/executor.rs`
  - Added an unused backend seam,
    `LlmExecutor::generate_projection_patch`, that can enqueue a
    `ProjectionJob` plus `TranscriptLedger` snapshot and return a trusted
    `ProjectionPatch`.
  - The seam reuses existing backend handles for OpenRouter, generic
    OpenAI-compatible API clients, local llama, and mistral.rs where available.
  - Live ASR scheduler observation is intentionally not wired to dispatch LLM
    calls in this slice.

- `src-tauri/src/lib.rs`
  - Registered the new `projection_llm` module.

Seeds touched by this structured-patch slice:

- Updated `audio-graph-4673` with
  `structured_projection_patch_draft_slice_2026_06_24`.
- Updated `audio-graph-d524` with
  `structured_projection_patch_draft_slice_2026_06_24`.
- Updated `audio-graph-e9b6` with
  `structured_projection_patch_draft_schema_slice_2026_06_24`.
- Removed stale dependency edges from `audio-graph-d524` to
  `audio-graph-ad44` and `audio-graph-4da5`. Those Seeds remain open for
  broader migration/replay/frontend/doc acceptance, but they no longer block
  TTFT queue work.
- Updated `audio-graph-d524`, `audio-graph-ad44`, and `audio-graph-4da5` with
  `blocker_reconciliation_2026_06_24` /
  `d524_blocker_reconciliation_2026_06_24` so the queue records why `d524` is
  now ready.

## Current projection scheduler telemetry work

Changes completed after the structured projection-patch draft slice:

- `src-tauri/src/projection_scheduler.rs`
  - Extended `ProjectionSchedulerMetrics` with completed-job count,
    repair-job count, follow-up-job count, and last/max job lag.
  - Added `ProjectionSchedulerTelemetry` and `ProjectionSchedulersTelemetry`
    snapshots, exposing each scheduler's projection kind, TTFT estimate,
    current in-flight job id/span count, pending coalesced span count, and
    counters.
  - `complete_in_flight` now records job lag before classifying current,
    stale, or repair completions.
  - Focused tests now assert coalescing, stale repair, completion lag, and
    in-flight telemetry state.

Seeds touched by this telemetry slice:

- Updated `audio-graph-d524` with
  `scheduler_telemetry_slice_2026_06_24`.
- Updated `audio-graph-3f24` with
  `scheduler_telemetry_foundation_slice_2026_06_24`.

## Verification run

Workflow/static verification:

- `ruby -e "require 'yaml'; %w[.github/workflows/ci.yml .github/workflows/release.yml .github/actionlint.yaml].each { |f| YAML.load_file(f); puts \"#{f} ok\" }"` - pass.
- `actionlint .github/workflows/ci.yml .github/workflows/release.yml` - pass.
- `git diff --check -- .github/workflows/ci.yml .github/workflows/release.yml .github/actionlint.yaml` - pass.
- `bun run tauri build --help` - confirmed local Tauri CLI exposes
  `--no-bundle`.
- Context7 Tauri v2 CLI docs confirmed `tauri build --no-bundle` and cargo
  runner args after `--`.
- PR 21 run `28126263189` - pass for frontend, lints, cargo audit, Rust
  Linux/macOS/Windows, and cloud-only Tauri smoke Linux/macOS/Windows.
- Release workflow dry-run `28125549025` - pass for create-release dry-run plus
  Linux/macOS/Windows local release artifact builds and artifact uploads.

Projection-runtime verification:

- `cargo test --lib runtime_projection_patch -- --nocapture` - pass
  (`3 passed; 0 failed`; local default-feature link took about 7m18s).
- `cargo test --lib materialized_projection_state_replays -- --nocapture` -
  pass.
- `cargo test --lib materialized_projection_restore_prefers -- --nocapture` -
  pass.
- `cargo check --lib --tests --no-default-features --features cloud` - pass.
- `cargo check --lib` - pass.
- `cargo fmt --check` - pass.
- `git diff --check -- src-tauri/src/projections.rs src-tauri/src/commands.rs src-tauri/src/state.rs src-tauri/src/persistence/mod.rs docs/commit-state-2026-06-24-cicd-backlog-roadmap.md .seeds/issues.jsonl` - pass.

ASR span-revision verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib provider_start_revision_helpers_chain_partial_to_final -- --nocapture` - pass.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib provider_sequence_revision_helpers_chain_partial_to_final -- --nocapture` - pass.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_sequence_revision_helpers_chain_partial_to_final -- --nocapture` - pass.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud aws_transcribe -- --nocapture` - pass.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud soniox -- --nocapture` - pass.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud transcript_ledger_replays_provider_partial_final_fixtures_without_duplicate_spans -- --nocapture` - pass.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud transcript_ledger_replays_latest_revisions_and_validates_current_basis -- --nocapture` - pass.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib tests_status -- --nocapture` - pass.
- `cargo +1.95.0 fmt --check --manifest-path src-tauri/Cargo.toml` - pass.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features 'cloud sherpa-streaming'` - pass.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` - pass.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib` - pass.
- `git diff --check -- src-tauri/src/asr/aws_transcribe.rs src-tauri/src/speech/mod.rs .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass.
- `git diff --check -- src-tauri/src/asr/mod.rs src-tauri/src/asr/soniox.rs src-tauri/src/projections.rs .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass.

Partial-ASR persistence verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud asr_partial_revision -- --nocapture` - pass
  (`2 passed; 0 failed`).
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud asr_partial_revision_recording -- --nocapture` - pass
  (`3 passed; 0 failed`).
- `cargo +1.95.0 fmt --check --manifest-path src-tauri/Cargo.toml` - pass.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` - pass.
- `git diff --check -- src-tauri/src/speech/mod.rs src-tauri/src/asr/mod.rs src-tauri/src/asr/soniox.rs src-tauri/src/projections.rs .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass.
- `git diff --check -- src-tauri/src/speech/mod.rs .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass.

Projection-scheduler observer verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_scheduler -- --nocapture` - pass
  (`4 passed; 0 failed`).
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud asr_partial_revision -- --nocapture` - pass
  (`3 passed; 0 failed`).
- `cargo +1.95.0 fmt --check --manifest-path src-tauri/Cargo.toml` - pass.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` - pass.
- `git diff --check -- src-tauri/src/projection_scheduler.rs src-tauri/src/state.rs src-tauri/src/speech/context.rs src-tauri/src/speech/mod.rs src-tauri/src/speech/tests_integration.rs src-tauri/src/commands.rs .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass.
- `git diff --check -- src-tauri/src/speech/mod.rs .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass after canonical-final rejection cleanup.

Cloud-only local-ML availability verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_unavailable -- --nocapture` - pass
  (`3 passed; 0 failed`).
- `bun run test src/utils/errorToMessage.test.ts` - pass (`6 passed`).
- `cargo +1.95.0 fmt --check --manifest-path src-tauri/Cargo.toml` - pass.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` - pass.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib` - pass.
- `bun run typecheck` - pass.
- `bunx @biomejs/biome check src/utils/errorToMessage.ts src/utils/errorToMessage.test.ts src/types/index.ts` - pass. This command printed
  `Saved lockfile`; `bun.lock` was already modified before this slice and was
  not reverted.
- `cargo +1.95.0 tree --manifest-path src-tauri/Cargo.toml --no-default-features --features cloud -i whisper-rs` - exit 101 with package-not-found, expected proof that the crate is absent in cloud-only resolution.
- `cargo +1.95.0 tree --manifest-path src-tauri/Cargo.toml --no-default-features --features cloud -i llama-cpp-2` - exit 101 with package-not-found, expected proof that the crate is absent in cloud-only resolution.
- `cargo +1.95.0 tree --manifest-path src-tauri/Cargo.toml --no-default-features --features cloud -i mistralrs` - exit 101 with package-not-found, expected proof that the crate is absent in cloud-only resolution.
- `git diff --check -- src-tauri/src/error.rs src-tauri/src/commands.rs src/types/index.ts src/utils/errorToMessage.ts src/utils/errorToMessage.test.ts README.md docs/adr/0007-feature-gate-local-ml.md .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass.

Seeds integrity and parsing verification:

- `sd doctor` before repair - reported 20 bidirectional dependency mismatches,
  11 passed, 1 warning, 0 failures.
- `sd doctor --fix` - pass; added missing bidirectional dependency links.
- `sd doctor` after repair and later Seed updates - pass (`12 passed, 0
  warning(s), 0 failure(s)`).
- `jq -c . .seeds/issues.jsonl >/dev/null` - pass.
- `sd ready --format json | jq '.count'` - pass (`26`) after the local Seeds
  CLI stdout retry patch, follow-up Seed creation, and `audio-graph-80ed`
  closure.
- `sd blocked --format json | jq '.count'` - pass (`34`).
- `sd list --format json | jq '.count'` - pass (`50`).
- Current `.seeds/issues.jsonl` line count is 125. The increase from the
  123-line backup is expected: `audio-graph-2e71` and `audio-graph-3926` were
  created after the backup.
- Current `.seeds/issues.jsonl` SHA-256:
  `ebabaff0936bbdfe00d057003d57314239e6e671db14f70626c1c3b6dfdfee46`.

Provider registry required-feature and drift-check verification:

- `bun run generate:provider-registry` - pass.
- `bun run check:provider-registry` - pass.
- Drift failure probe against a modified temporary registry file - pass; check
  mode exited nonzero and did not rewrite the file.
- `cargo +1.95.0 fmt --check --manifest-path src-tauri/Cargo.toml` - pass.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph-provider-registry -- --nocapture` - pass.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_registry -- --nocapture` - pass.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_unavailable -- --nocapture` - pass.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` - pass.
- `bun run typecheck` - pass.
- `bun run test src/generated/providerRegistry.test.ts` - pass.
- `bun run test src/generated/providerRegistry.test.ts src/components/providerRegistryHelpers.test.ts` - pass.
- `bunx @biomejs/biome check scripts/generate-provider-registry.mjs package.json src/generated/providerRegistry.test.ts src/types/index.ts` - pass. This
  command printed `Saved lockfile`; `bun.lock` was already modified before this
  slice and was not reverted.

Provider registry closure verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph-provider-registry -- --nocapture` - pass after dead disabled test-block cleanup.
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_registry -- --nocapture` - pass (`9 passed`).
- `bun run check:provider-registry` - pass.
- `cargo +1.95.0 fmt --check --manifest-path src-tauri/Cargo.toml` - pass.
- `bun run test src/generated/providerRegistry.test.ts src/components/providerRegistryHelpers.test.ts src/components/SettingsPage.test.tsx` - pass (`70 passed`).
- `bun run typecheck` - pass.
- `git diff --check -- .seeds/issues.jsonl src-tauri/crates/provider-registry/src/lib.rs docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass.
- `sd doctor` after closure and `sd doctor --fix` - pass (`12 passed, 0
  warning(s), 0 failure(s)`).

Configuration readiness force-refresh verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_readiness -- --nocapture` - pass (`3 passed`).
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` - pass.
- `cargo +1.95.0 fmt --check --manifest-path src-tauri/Cargo.toml` - pass.
- `bun run test src/components/SettingsPage.test.tsx` - pass (`53 passed`).
- `bun run typecheck` - pass.
- `bunx @biomejs/biome check src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx` - pass. This command printed `Saved lockfile`;
  `bun.lock` was already modified before this slice and was not reverted.
- `git diff --check -- src-tauri/src/commands.rs src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass.

Structured projection-patch draft verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_llm -- --nocapture` - pass
  (`7 passed`).
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud executor -- --nocapture` - pass
  (`15 passed`).
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` - pass.
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib` -
  pass.
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -
  pass.
- `git diff --check -- src-tauri/src/projection_llm.rs src-tauri/src/projections.rs src-tauri/src/lib.rs src-tauri/src/llm/executor.rs .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass before this doc update.

Projection scheduler telemetry verification:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_scheduler -- --nocapture` - pass
  (`4 passed`).
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud` - pass.
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check` -
  pass.
- `git diff --check -- src-tauri/src/projection_scheduler.rs .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md` - pass before this doc update.

Verification not yet run:

- Non-dry test tag release path.
- Full local Rust/frontend suites across the broad dirty worktree.
- `sd sync`, intentionally skipped because unrelated staged changes would be
  swept into a Seeds commit.

## Seeds queue snapshot

Current `sd ready` reports 26 ready issues. Top ready items:

1. `audio-graph-9279` - Moonshine model downloader readiness and
   cross-platform validation.
2. `audio-graph-0117` - Moonshine streaming worker and span-revision adapter.
3. `audio-graph-2586` - Move release workflow to Blacksmith and pinned actions.
4. `audio-graph-d524` - TTFT-aware LLM synthesis queue for notes and graph
   diffs.
5. `audio-graph-1c2f` - Configuration UX and credential health center.
6. `audio-graph-4673` - Streaming transcript to notes and temporal-graph diff
   pipeline.
7. `audio-graph-c395` - Cross-platform CI and Blacksmith release-readiness
   matrix.
8. `audio-graph-ad1d` - Provider registry and streaming STT expansion roadmap.
9. `audio-graph-5fe7` - Gate local ML behind cargo feature flags.
10. `audio-graph-74b2` - Blacksmith Tauri build smoke matrix.
11. `audio-graph-a805` - Split provider registry exporter into lightweight
    codegen path.
12. `audio-graph-dbac` - Diarization settings UX for local, provider, and
    hybrid modes.
13. `audio-graph-01be` - Extend registry model descriptors to diarization
    runtime dependencies.

Current `sd blocked` reports 33 blocked issues. Important blocked workstreams:

- `audio-graph-5b75` - Feature-gated cloud-only build in CI.
- `audio-graph-e35f` - Soniox realtime STT provider.
- `audio-graph-4da5` - Transcript revision ledger and canonical span
  projection.
- `audio-graph-5011` and related diarization Seeds - local/provider/hybrid
  speaker timeline path.

## Projection dispatch slice - 2026-06-25

Completed a backend runtime slice for `audio-graph-d524` /
`audio-graph-4673`:

- `ProjectionScheduler` now has explicit failure completion semantics:
  failed jobs clear `in_flight`, increment `failed_jobs`, record lag telemetry,
  and suppress same-basis retry loops until the transcript basis changes.
- Added `ProjectionRuntimeHandle`, a cloneable subset of `AppState` that
  background speech workers can use for transcript snapshots, per-kind sequence
  allocation, and checked runtime patch application.
- Final/end-of-turn ASR revisions now dispatch notes and graph jobs to
  `LlmExecutor::generate_projection_patch` on background threads. Partials
  still update the transcript ledger/event stream without starting projection
  LLM work. The dispatch happens after ledger and scheduler locks are released
  so provider work and spawn-failure cleanup cannot deadlock the ingest path.
- Generated patches are applied through runtime session/basis/materializer
  checks. Stale apply failures complete the scheduler job so repair jobs can
  start; generation and semantic apply failures use the new failure path so
  the queue does not stay stuck.
- Closed follow-up Seed `audio-graph-bfd2`: added a deterministic
  `ProjectionPatchGenerator` seam so production dispatch still uses
  `LlmExecutor`, while tests inject fake patch generation without provider
  credentials. The harness covers notes+graph success, generation failure
  clearing, stale apply repair, and partial suppression.
- Added backend diagnostics command `get_projection_runtime_status_cmd`,
  registered in Tauri, returning non-secret projection runtime status:
  transcript ledger counts, scheduler telemetry, materialized notes/graph
  counts and sequence numbers, and projection event writer availability.
  Frontend consumption is tracked by `audio-graph-f673`.

Verification run after this slice:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud runtime_projection_dispatch -- --nocapture --test-threads=1`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_runtime_status -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_scheduler -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud runtime_projection_scheduler_observes_finals_without_partial_job_churn -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud runtime_projection_patch -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_llm -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud executor -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `git diff --check --` touched projection files
- `sd doctor`
- `jq -c . .seeds/issues.jsonl`

## Projection runtime diagnostics UI slice

Changes completed after the backend projection runtime status command:

- Closed follow-up Seed `audio-graph-f673`.
- Added `ProjectionRuntimeStatusPanel` to the right rail. It calls
  `get_projection_runtime_status_cmd` on mount and manual refresh, then renders
  non-secret projection status:
  - transcript ledger event/span counts;
  - materialized notes count and sequence;
  - materialized graph node/edge counts and sequence;
  - notes and graph queue state, in-flight job ids/span counts, pending spans,
    TTFT, lag, completed/failed/stale counters, repairs, follow-ups, and
    coalescing;
  - projection event writer availability.
- Added frontend IPC types for `ProjectionRuntimeStatus`,
  `ProjectionSchedulersTelemetry`, `ProjectionSchedulerTelemetry`,
  `ProjectionSchedulerMetrics`, and `ProjectionMaterializedStatus`.
- Added English and Portuguese `projectionDiagnostics` locale strings.
- Updated `audio-graph-d524` and `audio-graph-4673` with
  `projection_runtime_status_ui_slice_2026_06_25`.

Verification for this UI slice:

- `bun run test src/components/ProjectionRuntimeStatusPanel.test.tsx`
- `bun run test src/App.test.tsx`
- `bun run typecheck`
- `bunx @biomejs/biome check src/components/ProjectionRuntimeStatusPanel.tsx src/components/ProjectionRuntimeStatusPanel.test.tsx src/App.tsx src/types/index.ts src/i18n/locales/en.json src/i18n/locales/pt.json`
- `jq empty src/i18n/locales/en.json src/i18n/locales/pt.json`
- `git diff --check -- src/components/ProjectionRuntimeStatusPanel.tsx src/components/ProjectionRuntimeStatusPanel.test.tsx src/App.tsx src/types/index.ts src/i18n/locales/en.json src/i18n/locales/pt.json .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md`
- `sd doctor`
- `jq -c . .seeds/issues.jsonl`

## Live projection events slice

Changes completed after the diagnostics UI slice:

- Closed follow-up Seed `audio-graph-9d1e`.
- Added live projection event names for accepted runtime patches and
  materialized notes/graph artifacts.
- Runtime projection dispatch now emits the accepted `ProjectionPatch` and the
  current materialized notes or graph snapshot only after
  `apply_runtime_projection_patch` succeeds. Generation failures, stale apply
  failures, semantic apply failures, and partial-only inputs emit no live
  projection materialization events.
- Added a projection runtime event sink seam so production emits Tauri events
  while tests record event counts without a frontend harness.
- Added `ProjectionRuntimeHandle::materialized_projection_snapshot` so emitted
  artifacts reflect committed runtime state after validation and persistence.
- React now subscribes to `projection-patch`, `materialized-notes-update`, and
  `materialized-graph-update`, appending live patch history and replacing the
  current materialized notes/graph state in the store.
- Updated `audio-graph-d524` and `audio-graph-4673` with
  `live_projection_events_slice_2026_06_25`.

Verification for this live event slice:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud runtime_projection_dispatch -- --nocapture --test-threads=1`
- `bun run test src/hooks/useTauriEvents.test.ts src/store/index.test.ts`
- `bun run typecheck`
- `bunx @biomejs/biome check src/hooks/useTauriEvents.ts src/hooks/useTauriEvents.test.ts src/store/index.ts src/store/index.test.ts src/types/index.ts`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `git diff --check -- src-tauri/src/events.rs src-tauri/src/state.rs src-tauri/src/speech/mod.rs src/hooks/useTauriEvents.ts src/hooks/useTauriEvents.test.ts src/store/index.ts src/store/index.test.ts src/types/index.ts .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md`
- `sd doctor`
- `jq -c . .seeds/issues.jsonl`

## Projection replay report command slice

Changes completed after the live projection events slice:

- Created and closed follow-up Seed `audio-graph-6f39`.
- Added `get_projection_replay_report_cmd`, a backend read-only replay/eval
  command for one session.
- The command validates the session id, loads transcript event JSONL,
  projection patch JSONL, materialized notes, and materialized graph artifacts
  through existing typed persistence helpers.
- It replays `TranscriptLedger` and accepted `ProjectionPatch` logs through
  `MaterializedProjectionState::replay_accepted_patches`, intentionally avoiding
  `load_session` and avoiding any `AppState` mutation.
- It returns non-secret report fields only: transcript/projection event counts,
  transcript span count, replayed notes/graph sequence and item counts,
  materialized artifact freshness (`missing`, `current`, `stale`, `ahead`),
  and structured replay error strings. It does not return transcript text, note
  bodies, graph labels/descriptions, or credentials.
- Added writer-backed tests for missing logs, valid replay parity, stale notes
  artifacts, semantic projection replay errors, and no app-state mutation.
- Updated a stale `LlmExecutor::generate_projection_patch` comment that still
  described the live dispatch path as future/unwired.
- Updated `audio-graph-d524` and `audio-graph-4673` with
  `projection_replay_report_command_slice_2026_06_25`.

Verification for this replay report slice:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_replay_report -- --nocapture --test-threads=1`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `git diff --check -- src-tauri/src/commands.rs src-tauri/src/lib.rs src-tauri/src/llm/executor.rs .seeds/issues.jsonl docs/commit-state-2026-06-24-cicd-backlog-roadmap.md`
- `sd doctor`
- `jq -c . .seeds/issues.jsonl`

## Ownership boundaries

Known current continuation-owned files:

- `.github/workflows/ci.yml`
- `.github/workflows/release.yml` for the rsac pin, Blacksmith, action pinning,
  dry-run, and release metadata changes. Note: this file also had pre-existing
  staged packaging edits before the latest CI/CD patch.
- `.github/actionlint.yaml`
- `.seeds/issues.jsonl` entries created or updated for CI/CD and this
  documentation Seed, plus `audio-graph-2e71` and `audio-graph-3926` for Seeds
  integrity and JSON pipe parsing, plus `audio-graph-80ed` closure and
  `audio-graph-a805` CI-drift clarification, plus structured projection-patch
  updates for `audio-graph-4673`, `audio-graph-d524`, `audio-graph-e9b6`,
  `audio-graph-ad44`, `audio-graph-4da5`, `audio-graph-bfd2`,
  `audio-graph-f673`, `audio-graph-9d1e`, and `audio-graph-6f39`.
  Later Settings UX metadata also records the provider-local recovery guidance
  partial on `audio-graph-80a0`, `audio-graph-abc1`, `audio-graph-1c2f`, and
  `audio-graph-c323`, plus the backend non-secret configuration readiness
  message slice on `audio-graph-cbde`, `audio-graph-1c2f`, and
  `audio-graph-abc1`.
- `docs/commit-state-2026-06-24-cicd-backlog-roadmap.md`
- `src-tauri/crates/provider-registry/src/lib.rs` for the disabled duplicate
  test-block cleanup.
- `src-tauri/src/projection_llm.rs`
- `src-tauri/src/projections.rs` for `ProjectionOperation` schema derivation.
- `src-tauri/src/llm/executor.rs` for the projection patch generation executor
  seam and live-dispatch comment correction.
- `src-tauri/src/lib.rs` for module registration.
- `src-tauri/src/projection_scheduler.rs` for projection queue telemetry
  counters, snapshots, and failure clearing semantics.
- `src-tauri/src/events.rs` for live projection event names.
- `src-tauri/src/state.rs` for `ProjectionRuntimeHandle`.
- `src-tauri/src/speech/mod.rs`, `src-tauri/src/speech/context.rs`,
  `src-tauri/src/speech/tests_integration.rs`, and `src-tauri/src/commands.rs`
  for runtime projection dispatch wiring, deterministic fake-dispatch tests, and
  the backend projection runtime status/replay report commands.
- `src/components/ProjectionRuntimeStatusPanel.tsx` and
  `src/components/ProjectionRuntimeStatusPanel.test.tsx` for the frontend
  projection diagnostics panel.
- `src/App.tsx`, `src/types/index.ts`, `src/i18n/locales/en.json`, and
  `src/i18n/locales/pt.json` for panel wiring, IPC types, and localized
  diagnostics copy. These files also contain earlier unrelated roadmap edits;
  inspect diffs before attributing every hunk to this slice.
- `src/hooks/useTauriEvents.ts`, `src/hooks/useTauriEvents.test.ts`,
  `src/store/index.ts`, and `src/store/index.test.ts` for live projection patch
  and materialized notes/graph event consumption. These files also contain
  earlier roadmap edits; inspect diffs before attributing every hunk to this
  slice.
- `src/components/ProviderReadinessPanel.tsx`,
  `src/components/ProviderReadinessPanel.test.tsx`,
  `src/components/SettingsPage.tsx`, `src/components/SettingsPage.test.tsx`,
  `src/styles/settings.css`, `src/i18n/locales/en.json`, and
  `src/i18n/locales/pt.json` for non-secret provider recovery guidance in the
  readiness dashboard and provider-local panels.
- `src/components/ProjectionRuntimeStatusPanel.tsx`,
  `src/components/ProjectionRuntimeStatusPanel.test.tsx`, and
  `src/types/index.ts` for the manual replay parity UI that invokes
  `get_projection_replay_report_cmd` and shows non-secret transcript/projection
  counts plus notes/graph artifact drift.
- `src-tauri/src/commands.rs` for provider readiness preflight messaging that
  reports missing non-secret setup, such as blank AWS profile names and
  incomplete Gemini Vertex project/location values, before network probes.

Known Seeds CLI output durability path:

- `/home/codeseys/.bun/install/global/node_modules/@os-eco/seeds-cli/src/output.ts`
  was patched so large `--format json` output reliably survives direct pipes.
- This repo now pins `@os-eco/seeds-cli@0.4.5` in dev dependencies and has a
  versioned check/fix wrapper. `bun run check:seeds-json-output` verifies the
  repo-local CLI first and parses `sd ready`, `sd blocked`, and `sd list` JSON
  output through that same package. `bun run prepare:seeds-json-output` applies
  the stdout retry patch when the repo-local or fallback global CLI is missing
  it.
- `audio-graph-3926` is closed because the fix is now captured in versioned
  repo tooling. Upstreaming the same `outputJson` retry behavior would let a
  future pinned Seeds CLI release make the prepare script a no-op.

Known earlier-roadmap files in the dirty tree include provider registry,
settings UX, credential readiness, projection contracts, audio capture, ASR/TTS
provider work, and S2S/converse work. Inspect diffs before editing those areas.

## Next recommended wave

1. Keep `audio-graph-2586` open until a non-dry release path from a test tag
   verifies publish-only artifacts and standalone upload steps.
2. Continue `audio-graph-8eeb`: the new cloud-only Linux job and release Linux
   job validate stock Ubuntu packages, but existing CI lint and full Rust Linux
   jobs still add the unpinned `pipewire-debian` PPA.
3. Continue `audio-graph-5fe7` / `audio-graph-5b75`: run the structured
   `provider_unavailable` cloud-only behavior through clean GitHub/Blacksmith
   Linux/macOS/Windows validation before closing the cross-platform acceptance.
4. Continue `audio-graph-d524` / `audio-graph-4673`: projection diagnostics UI
   and live projection patch/materialized-artifact frontend events are now
   closed via `audio-graph-f673` and `audio-graph-9d1e`, and backend replay
   reporting is closed via `audio-graph-6f39`; remaining projection work is
   historical replay-basis validation, an optional UI/export/eval consumer for
   the replay report, provider-backed smoke once saved LLM credential health is
   available, and clean cross-platform CI proof.
5. Continue Seeds hygiene after every slice. Do not close epics on static
   validation alone.
6. Continue `audio-graph-1c2f` / `audio-graph-80a0`: provider-local recovery
   guidance is now in the dashboard and provider panels, but the dedicated
   health-center details drawer/modal, intentional replace-key flow, broader
   model discovery, and product-mode/capability-card redesign remain open.
7. Continue `audio-graph-cbde`: missing non-secret setup now gets explicit
   unchecked readiness messages for AWS profile mode and Gemini Vertex. Remote
   model/voice/language catalogs beyond OpenRouter and any real Vertex health
   probe remain open.
8. Continue `audio-graph-3886`: Projection diagnostics now has a manual replay
   parity report consumer, but the full offline no-network replay/eval harness
   still needs deterministic fixtures and metrics for latency, coalescing,
   stale discards, graph churn, duplicate rate, correction accuracy, and token
   cost.
9. Continue `audio-graph-14e0` through new blocker `audio-graph-c4fd`: current
   Moonshine research shows multiple plausible runtime paths (UsefulSensors
   Python/Keras, Hugging Face Moonshine Streaming checkpoints, sherpa-onnx
   Moonshine ONNX packaging, or a direct Rust ONNX Runtime path). Decide the
   cross-platform runtime and model/download contract before adding code.

## Moonshine runtime architecture decision - 2026-06-25

Closed `audio-graph-c4fd`.

Decision:

- Use the Moonshine Voice native C API as the production runtime, behind a new
  optional `asr-moonshine` Cargo feature.
- Keep Python/PyPI `moonshine-voice` and Hugging Face Transformers as
  downloader/reference/benchmark tooling only, not the desktop app runtime.
- Keep sherpa-onnx Moonshine packaging as a fallback if native Moonshine
  packaging proves too brittle.
- Do not hand-roll a direct Rust ONNX Runtime implementation for the first
  slice because that would duplicate tokenizer, VAD, streaming cache, line
  identity, speaker metadata, and model-option behavior already exposed by the
  Moonshine C API.

Research/decision artifact:

- `docs/research/moonshine-local-stt-runtime-2026-06-25.md`

Key runtime contract:

- Streaming model directory required files: `adapter.ort`, `cross_kv.ort`,
  `decoder_kv.ort`, `decoder_kv_with_attention.ort`, `encoder.ort`,
  `frontend.ort`, `streaming_config.json`, and `tokenizer.bin`.
- Audio input stays backend-owned: 16 kHz mono `f32` PCM from the processed
  audio bus into the Moonshine stream API.
- Moonshine transcript line id becomes provider item identity.
- Incomplete lines emit partial span revisions; complete lines emit final /
  end-of-turn revisions; line update flags gate whether AudioGraph emits a new
  revision.
- Provider speaker ids are provisional provider hints, not a replacement for
  the normalized speaker timeline/diarization workstream.

New implementation Seeds created and linked under `audio-graph-14e0`:

- `audio-graph-a2dc` - Moonshine native provider skeleton and packaging probe.
- `audio-graph-0117` - Moonshine streaming worker and span-revision adapter.
- `audio-graph-9279` - Moonshine model downloader readiness and
  cross-platform validation.

Sources checked:

- Moonshine Voice launch/platform docs:
  `https://huggingface.co/blog/UsefulSensors/announcing-moonshine-voice`
- Moonshine Voice repo and C/C++ integration docs:
  `https://github.com/moonshine-ai/moonshine`
- Moonshine C API header:
  `https://github.com/moonshine-ai/moonshine/blob/main/core/moonshine-c-api.h`
- Hugging Face Moonshine Streaming model card:
  `https://huggingface.co/UsefulSensors/moonshine-streaming-medium`
- Transformers Moonshine Streaming docs:
  `https://github.com/huggingface/transformers/blob/main/docs/source/en/model_doc/moonshine_streaming.md`
- sherpa-onnx docs and community Moonshine package:
  `https://k2-fsa.github.io/sherpa/onnx/index.html`,
  `https://huggingface.co/csukuangfj/sherpa-onnx-moonshine-tiny-en-int8`
- Moonshine v2 paper:
  `https://arxiv.org/abs/2602.12241`

## Moonshine provider skeleton slice - 2026-06-25

Ready-to-close work on `audio-graph-a2dc`:

- `src-tauri/Cargo.toml` now declares optional feature `asr-moonshine`, off by
  default.
- The provider registry declares planned local provider `asr.moonshine` with
  local-only privacy, local streaming lifecycle, transcript partial/final/turn
  semantics, partial revision support, no credentials, streaming model
  directory component requirements (`adapter.ort`, `cross_kv.ort`,
  `decoder_kv.ort`, `decoder_kv_with_attention.ort`, `encoder.ort`,
  `frontend.ort`, `streaming_config.json`, and `tokenizer.bin`), and required
  feature metadata for `asr-moonshine`.
- Generated frontend registry output is current. The settings helper knows the
  `moonshine` settings variant, but `asr.moonshine` remains filtered out of
  selectable implemented providers while its descriptor status is `planned`.
- Backend settings can deserialize/persist the Moonshine ASR variant without
  secrets, and transcription preflight returns structured provider-unavailable
  errors instead of trying to load native headers/libs/models in cloud or
  no-feature builds.
- The speech processor has an explicit defensive fallback if a Moonshine
  provider reaches runtime before the worker exists; it does not silently route
  through another ASR provider.

Validation completed locally on Linux:

- `bun run generate:provider-registry`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph-provider-registry -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud compiled_out_moonshine -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud asr_capture_selection -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_registry -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud,asr-moonshine`
- `bun run test src/generated/providerRegistry.test.ts src/components/providerRegistryHelpers.test.ts`
- `bun run check:provider-registry`
- `bun run typecheck`
- `bunx @biomejs/biome check src/components/SettingsPage.tsx src/components/settingsTypes.ts src/components/providerRegistryHelpers.test.ts src/generated/providerRegistry.ts src/generated/providerRegistry.test.ts src/types/index.ts`

Remaining Moonshine work is intentionally split:

- `audio-graph-0117` owns the actual Moonshine streaming worker, native adapter,
  and span-revision mapping.
- `audio-graph-9279` owns model downloader/readiness UX plus macOS and Windows
  Blacksmith validation before `asr.moonshine` can become implemented.

## Moonshine model downloader/readiness partial slice - 2026-06-25

Partial progress on `audio-graph-9279`:

- The provider registry and generated frontend registry now use the current
  English streaming model IDs exposed by the upstream downloader:
  `moonshine-small-streaming-en`, `moonshine-medium-streaming-en`, and
  `moonshine-tiny-streaming-en`.
- Moonshine local model descriptors now validate the streaming component
  directory contract: `adapter.ort`, `cross_kv.ort`, `decoder_kv.ort`,
  `decoder_kv_with_attention.ort`, `encoder.ort`, `frontend.ort`,
  `streaming_config.json`, and `tokenizer.bin`.
- The Rust model manager can download component-directory models from
  `https://download.moonshine.ai/model/{model}/quantized/{component}` into a
  temporary directory, validate non-empty components, and atomically promote the
  complete model directory.
- Provider readiness now reports local model catalog state for planned local
  providers without making Moonshine selectable before the native runtime worker
  exists.
- The settings model table derives Moonshine readiness badges from each model's
  own validity instead of the older Whisper/LFM2/Sortformer filename heuristic.

Validation completed locally on Linux:

- `bun run generate:provider-registry`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph-provider-registry -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud moonshine -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_readiness -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud models -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_registry -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud,asr-moonshine`
- `bun run test src/generated/providerRegistry.test.ts src/components/providerRegistryHelpers.test.ts src/components/CredentialsManager.test.tsx src/components/ProviderReadinessPanel.test.tsx`
- `bun run typecheck`
- `bun run check:provider-registry`
- `bunx @biomejs/biome@2.5.1 check src/components/CredentialsManager.tsx src/components/CredentialsManager.test.tsx src/generated/providerRegistry.ts src/generated/providerRegistry.test.ts src/components/providerRegistryHelpers.test.ts`

Remaining before closing `audio-graph-9279`:

- Prove macOS, Windows, and Linux `asr-moonshine` compile through
  GitHub/Blacksmith CI.
- Wire native runtime load failure and healthy-runtime readiness once
  `audio-graph-0117` introduces the actual Moonshine worker.
- Keep `asr.moonshine` planned/unselectable until model readiness and runtime
  probes both pass.

## Moonshine worker mapping partial slice - 2026-06-25

Partial progress on `audio-graph-0117`:

- Added `src-tauri/src/asr/moonshine.rs` as a backend-only Moonshine streaming
  adapter seam. It defines the fakeable `MoonshineStreamingAdapter`, runtime
  config, transcript-line update model, worker shell, and mapping errors without
  requiring native Moonshine libraries or model files.
- Added `MoonshineSpanMapper` to convert Moonshine line updates into
  provider-neutral `AsrSpanRevisionPayload` values. Mapping preserves
  `line_id` as `provider_item_id`, uses stable `moonshine:{source}:{line_id}`
  span ids, chains revision numbers/supersedes, emits partial vs final stability
  correctly, marks completed lines as `end_of_turn`, and carries provider
  speaker hints as provisional metadata only.
- The mapper filters empty text, missing line ids, no-update polls, duplicate
  unchanged provider polls, and duplicate completed-line polls. A final update
  after a partial still emits even when text is unchanged because finality and
  end-of-turn changed.
- The existing runtime fallback remains in place: selecting Moonshine still
  falls back to diarization-only and `local_asr_provider_availability_error`
  still reports runtime unavailable until the native worker is actually wired.

Validation completed locally on Linux:

- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud moonshine -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud,asr-moonshine`

Remaining before closing `audio-graph-0117`:

- Implement the native Moonshine C API adapter behind `asr-moonshine`.
- Wire the worker into `speech/mod.rs` so processed 16 kHz mono PCM is fed to
  one stream per allowed source policy.
- Route accepted final revisions through the transcript/finalization tail
  without creating duplicate transcript rows or final diarization spans from
  provider speaker hints.
- Emit Moonshine transcription latency telemetry from line latency/runtime
  timing.

## TTFT projection queue closure - 2026-06-25

Closed `audio-graph-d524`.

Closure rationale:

- Final/end-of-turn ASR span revisions now dispatch basis-bound notes and graph
  `ProjectionJob` work through the runtime scheduler.
- Partial ASR churn updates the transcript ledger but does not trigger LLM calls.
- In-flight projection work coalesces newer transcript bases; stale or failed
  jobs are discarded, suppressed, or repaired against the current basis.
- Structured LLM patch drafts are parsed into trusted backend-stamped
  `ProjectionPatch` values with provenance and basis metadata.
- Accepted patches apply through basis-checked materialized notes/graph state,
  persist projection events, and emit live projection/materialized-state events.
- Runtime status and replay report commands expose non-secret queue/materializer
  diagnostics for UI and replay checks.

Validation completed locally on Linux:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_scheduler -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud runtime_projection_dispatch -- --nocapture --test-threads=1`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_runtime_status -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_llm -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_replay_report -- --nocapture --test-threads=1`
- `bun run test src/components/ProjectionRuntimeStatusPanel.test.tsx src/hooks/useTauriEvents.test.ts src/store/index.test.ts`
- `bun run typecheck`

Downstream work remains intentionally open:

- `audio-graph-d5a4` owns notes-specific versioned diff UX/history semantics.
- `audio-graph-6008` owns temporal graph retcon operations such as merge,
  split, invalidate, and temporal validity changes.
- `audio-graph-3f24` owns deeper latency/token-cost/backpressure diagnostics.

## Configuration UX Deepgram catalog slice - 2026-06-25

Partial progress on `audio-graph-1c2f`:

- Added a backend-owned Deepgram model catalog path using Deepgram's public
  `GET https://api.deepgram.com/v1/models` endpoint. The parser keeps
  streaming STT entries from the `stt` array, uses `canonical_name` as the
  selectable model id, and ignores TTS/non-streaming entries.
- `get_provider_readiness_cmd` now lets provider probes return a remote
  `model_catalog` alongside health state. Deepgram readiness with a saved key
  returns a catalog/model count without sending plaintext credentials to React.
- Registered `list_deepgram_models_cmd` and published it through the generated
  provider registry as `asr.deepgram.model_catalog_command`.
- The Deepgram ASR model field now uses the readiness catalog as a datalist
  while remaining editable for custom model ids.
- Provider-local recovery copy no longer tells users to "Run checks" for
  saved-key providers that have no automatic health command.

Reference:

- https://developers.deepgram.com/reference/manage/models/list

Validation completed locally on Linux:

- `bun run generate:provider-registry`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph-provider-registry -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud deepgram -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_readiness -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud`
- `bun run test src/components/SettingsPage.test.tsx src/components/ProviderReadinessPanel.test.tsx src/generated/providerRegistry.test.ts src/components/providerRegistryHelpers.test.ts`
- `bun run typecheck`
- `bun run check:provider-registry`
- `bunx @biomejs/biome@2.5.1 check src/components/SettingsPage.test.tsx src/components/ProviderReadinessPanel.test.tsx src/components/ProviderReadinessPanel.tsx src/components/AsrProviderSettings.tsx src/components/SettingsPage.tsx src/generated/providerRegistry.test.ts src/generated/providerRegistry.ts`

Remaining before closing `audio-graph-1c2f`:

- Continue model discovery beyond Deepgram/OpenRouter/fixed/local catalogs.
- Add dedicated replace-key/health-center detail flows.
- Complete the product-mode/capability-card Settings redesign tracked in
  `audio-graph-c323`.
- Prove the Settings health/catalog path in cross-platform GitHub/Blacksmith
  validation on a clean branch.

## Settings credentials diagnostics slice - 2026-06-25

Closed `audio-graph-6b8d`.

Closure rationale:

- Settings now preserves `CredentialFileError` failures from both
  `load_credential_presence_cmd` and `get_provider_readiness_cmd` and renders
  them as an explicit provider-readiness error instead of replacing the health
  center with the empty "no saved credentials" state.
- The error display uses `errorToMessage`, so it preserves structured backend
  errors while still keeping plaintext credentials out of React state.
- The readiness dashboard now shares the descriptor-aware recovery behavior
  used by provider-local readiness panels, so no-probe providers no longer get
  misleading "Run checks" recovery copy.

Validation completed locally on Linux:

- `bun run test src/components/SettingsPage.test.tsx src/components/ProviderReadinessPanel.test.tsx`
- `bun run typecheck`
- `bunx @biomejs/biome@2.5.1 check src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx src/components/ProviderReadinessPanel.tsx src/components/ProviderReadinessPanel.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json src/styles/settings.css`
- `jq empty src/i18n/locales/en.json src/i18n/locales/pt.json`

## Soniox saved-key catalog slice - 2026-06-25

Partial progress on `audio-graph-e35f` and `audio-graph-ad1d`:

- Added backend-owned Soniox saved-key readiness and real-time model discovery
  using Soniox's official `GET https://api.soniox.com/v1/models` endpoint with
  bearer auth.
- The parser keeps STT models whose `transcription_mode` is `real_time`, uses
  `id` as the selectable catalog id, and marks `stt-rt-v5` as the default.
- `get_provider_readiness_cmd` can now probe saved `soniox_api_key` credentials
  and return a remote model catalog without sending plaintext keys to React.
- The provider registry exposes `test_soniox_connection` and
  `list_soniox_models_cmd` for `asr.soniox`, while keeping Soniox
  `planned`/unselectable until live WebSocket transport lands.
- Readiness tests moved the "provider without a health command" fixture to
  Gladia, since Soniox now has a backend probe.

References:

- https://soniox.com/docs/api-reference/stt/get_models
- https://soniox.com/docs/stt/models
- https://soniox.com/docs/api-reference/stt/websocket-api

Validation completed locally on Linux:

- `bun run generate:provider-registry`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml -p audio-graph-provider-registry -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud soniox -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud provider_readiness -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud`
- `bun run test src/generated/providerRegistry.test.ts src/components/ProviderReadinessPanel.test.tsx src/components/SettingsPage.test.tsx src/components/providerRegistryHelpers.test.ts`
- `bun run typecheck`
- `bun run check:provider-registry`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `bunx @biomejs/biome@2.5.1 check src/generated/providerRegistry.ts src/generated/providerRegistry.test.ts src/components/ProviderReadinessPanel.test.tsx src/components/SettingsPage.test.tsx`

Remaining before closing `audio-graph-e35f`:

- Implement live Soniox WebSocket transport behind `soniox_api_key`.
- Map provider endpoint/finalization, language, speaker, and latency metadata
  through the normalized ASR span-revision runtime path.
- Keep Settings provider selection hidden until the live runtime and cleanup
  paths are proven.
- Add env-gated live smoke coverage once CI secrets and runner policy are
  available.

## Projection latency/token telemetry slice - 2026-06-25

Partial progress on `audio-graph-3f24` and the P1 projection epic
`audio-graph-4673`:

- Removed stale `audio-graph-d524` blocker metadata from `audio-graph-3f24`
  because `d524` is closed and `3f24` is actionable.
- Projection scheduler telemetry now reports live in-flight job age,
  generation latency, apply/materializer latency, accepted patch count,
  generation/apply failures, and provider-reported token totals.
- Projection LLM generation now returns `ProjectionPatchOutcome`, preserving
  `total_tokens` from API/OpenRouter/native/Mistral projection calls when the
  provider reports it.
- Runtime projection dispatch records non-secret aggregate metrics around LLM
  generation and materializer apply without exposing transcript text, note
  bodies, graph labels, or credentials.
- The existing Projection diagnostics panel renders the new aggregate age,
  generation, apply, token, patch, and failure metrics.

Validation completed locally on Linux:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_scheduler -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud runtime_projection_dispatch -- --nocapture --test-threads=1`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_runtime_status -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud executor -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `bun run test src/components/ProjectionRuntimeStatusPanel.test.tsx`
- `bun run typecheck`
- `jq empty src/i18n/locales/en.json src/i18n/locales/pt.json`
- `bunx @biomejs/biome@2.5.1 check src-tauri/src/projection_scheduler.rs src-tauri/src/llm/executor.rs src-tauri/src/llm/mod.rs src-tauri/src/speech/mod.rs src-tauri/src/commands.rs src/types/index.ts src/components/ProjectionRuntimeStatusPanel.tsx src/components/ProjectionRuntimeStatusPanel.test.tsx src/i18n/locales/en.json src/i18n/locales/pt.json`

Remaining before closing `audio-graph-3f24`:

- Wire latency/token telemetry into replay/eval artifacts and recorded-session
  reports.
- Distinguish capture/ASR delay from LLM projection delay in user-facing
  diagnostics.
- Add provider-backed env-gated projection smoke using saved credentials
  without logging transcript text or secrets.
- Prove the projection telemetry path in cross-platform GitHub/Blacksmith
  validation on a clean branch.

## Settings credential-health details slice - 2026-06-25

Closed focused child `audio-graph-646d` and advanced P1 configuration epic
`audio-graph-1c2f`:

- Settings now preserves non-secret `CredentialPresence` entries, including
  `source`, instead of reducing the saved credential map to booleans.
- The provider readiness dashboard and provider-local readiness panels share a
  compact Details disclosure with credential slot names, present/missing state,
  `credentials.yaml` vs missing source, last checked state, and catalog state.
- Details are rendered from backend-owned readiness/presence payloads only;
  plaintext credentials are still not loaded into React or displayed.
- Existing Run checks, save, clear-key, and recovery copy behavior remains
  unchanged.

Validation completed locally on Linux:

- `bun run test src/components/ProviderReadinessPanel.test.tsx src/components/SettingsPage.test.tsx`
- `bun run typecheck`
- `jq empty src/i18n/locales/en.json src/i18n/locales/pt.json`
- `bunx @biomejs/biome@2.5.1 check src/components/ProviderReadinessPanel.tsx src/components/ProviderReadinessPanel.test.tsx src/components/SettingsPage.tsx src/components/SettingsPage.test.tsx src/components/AsrProviderSettings.tsx src/components/LlmProviderSettings.tsx src/components/GeminiSettings.tsx src/i18n/locales/en.json src/i18n/locales/pt.json src/styles/settings.css`

Remaining before closing `audio-graph-1c2f`:

- Dedicated replace-key/health-center action flows.
- Product-mode/capability-card Settings redesign tracked in
  `audio-graph-c323`.
- More remote catalog discovery beyond the current implemented/fixed/cataloged
  providers.
- Cross-platform GitHub/Blacksmith validation on a clean branch.

## Historical projection replay-basis validation slice - 2026-06-25

Closed focused child `audio-graph-ff70` and advanced event-sourced data-model
Seed `audio-graph-ad44`:

- Accepted projection-log replay now has a historical validation path that
  advances a transcript ledger only through transcript events received at or
  before each patch timestamp.
- Older valid patches remain replayable after later transcript growth, while
  impossible patch bases are skipped and counted instead of being materialized.
- `load_session` uses the historical replay path for restored projection
  artifacts and logs non-secret warnings when invalid accepted patches are
  skipped.
- `get_projection_replay_report_cmd` now reports checked patch count and invalid
  historical-basis count, with projection replay errors summarized without
  transcript text, note bodies, graph labels, or credentials.
- The Projection diagnostics replay parity UI shows the invalid-basis count.

Validation completed locally on Linux:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud materialized_projection_history_validation -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_replay_report -- --nocapture --test-threads=1`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `bun run test src/components/ProjectionRuntimeStatusPanel.test.tsx`
- `bun run typecheck`
- `jq empty src/i18n/locales/en.json src/i18n/locales/pt.json`
- `bunx @biomejs/biome@2.5.1 check src/components/ProjectionRuntimeStatusPanel.tsx src/components/ProjectionRuntimeStatusPanel.test.tsx src/types/index.ts src/i18n/locales/en.json src/i18n/locales/pt.json`

Remaining before closing `audio-graph-ad44`:

- ADR/design migration boundary documentation.
- Crash/replay fixtures that exercise full session restore and artifact
  fallback behavior.
- Frontend live retcon reducers beyond materialized projection state delivery.
- Cross-platform GitHub/Blacksmith validation on a clean branch.

## Projection patch repair prompt retry slice - 2026-06-25

Closed focused child `audio-graph-7fb0` and advanced structured-schema Seed
`audio-graph-e9b6`:

- Removed stale `audio-graph-ad44` dependency from `audio-graph-e9b6`; the
  schema work now has the projection contracts, basis validation, and replay
  validation primitives it needs.
- Added `projection_patch_repair_prompt_messages`, a provider-agnostic repair
  prompt that includes the validation error, expected projection kind, schema,
  and compact invalid model output while continuing to forbid model-owned
  trusted metadata.
- Projection patch generation now retries at most once with the same successful
  backend attempt when first-pass JSON is invalid. If repair succeeds, token
  usage is summed across first and repair calls and the patch is stamped with
  `projection_patch_repair_v1`.
- If repair is invalid too, the executor fails without a third call and returns
  a structured validation summary rather than raw transcript text.

Validation completed locally on Linux:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_llm -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_patch_retries -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_patch_fails_after_one_repair_attempt -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`

Remaining before closing `audio-graph-e9b6`:

- Fixture coverage for duplicate entity avoidance, later-context correction,
  and notes item retcon behavior.
- Provider-specific structured-output optimization for vLLM/MistralRs or other
  local structured-output surfaces.
- Cross-platform GitHub/Blacksmith validation on a clean branch.

## Projection schema duplicate-id and retcon fixtures - 2026-06-25

Closed focused child `audio-graph-0e3a` and closed structured-schema Seed
`audio-graph-e9b6`:

- Projection draft validation now rejects duplicate operation identities inside
  one model draft before materialization. The duplicate check is namespaced by
  note, graph node, and graph edge identity, so same-patch duplicate nodes or
  conflicting note operations cannot slip through as accidental retcons.
- Added provider-agnostic fixtures proving later note context can retcon a
  stable note id through another `upsert_note` operation rather than replacement
  prose.
- Added provider-agnostic fixtures proving later graph context can update a
  stable graph node id through another `upsert_graph_node` operation.
- `audio-graph-e9b6` is now closed because its schema acceptance is met:
  malformed/stale/wrong-kind/duplicate operations are rejected, repair prompts
  exist, trusted metadata is backend-stamped, and fixture coverage exercises
  duplicate avoidance plus note/graph later-context corrections.
- Provider-specific structured-output optimization is split into a follow-up
  Seed instead of blocking the provider-agnostic schema contract.

Validation completed locally on Linux:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_llm -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`

Unblocked after closing `audio-graph-e9b6`:

- `audio-graph-d5a4` notes synthesis as versioned diffs with UI retcon support.
- `audio-graph-6008` temporal graph patch and retcon engine.

## NotesPanel materialized projection notes slice - 2026-06-25

Closed focused child `audio-graph-9ba8` and advanced notes retcon Seed
`audio-graph-d5a4`:

- The frontend `MaterializedNote` type now matches the backend projection
  artifact shape: stable id, title, body, tags, update sequence/time, basis, and
  provenance.
- `NotesPanel` now renders restored/live `materializedNotes` from the store
  before graph-derived fallback chips. Empty state treats materialized notes as
  real content.
- Live notes render with stable `data-note-id` keys, title/body/tags, and update
  sequence so later materialized updates replace the same note instead of
  appending duplicate prose.
- Existing synthesized Markdown and graph-derived fallback sections remain
  available.

Validation completed locally on Linux:

- `bun run test src/components/NotesPanel.test.tsx src/store/index.test.ts src/hooks/useTauriEvents.test.ts`
- `bun run typecheck`
- `jq empty src/i18n/locales/en.json src/i18n/locales/pt.json`
- `bunx @biomejs/biome@2.5.1 check src/components/NotesPanel.tsx src/components/NotesPanel.test.tsx src/types/index.ts src/store/index.test.ts src/hooks/useTauriEvents.test.ts src/i18n/locales/en.json src/i18n/locales/pt.json`

Remaining before closing `audio-graph-d5a4`:

- Explicit patch-history/correction indicators for user-visible retcons.
- Reorder semantics if product still wants ordered note sections beyond stable
  upsert/delete item lists.
- Replay/eval coverage for note stability across recorded sessions.
- Cross-platform GitHub/Blacksmith validation on a clean branch.

## NotesPanel projection patch history indicators - 2026-06-25

Closed focused child `audio-graph-980b` and advanced notes retcon Seed
`audio-graph-d5a4`:

- Frontend `ProjectionPatch.operations` now has a typed discriminated union for
  note and graph operations, which lets UI code reason about patch history
  without ad hoc `unknown` casts.
- `NotesPanel` derives per-note revision counts from notes projection patches
  and shows a compact correction indicator when a materialized note has multiple
  patch operations.
- Retconned notes still render as one stable list item keyed by note id; tests
  assert the old body disappears, the corrected body appears, and only one
  visible item remains for the stable id.

Validation completed locally on Linux:

- `bun run test src/components/NotesPanel.test.tsx src/store/index.test.ts src/hooks/useTauriEvents.test.ts`
- `bun run typecheck`
- `jq empty src/i18n/locales/en.json src/i18n/locales/pt.json`
- `bunx @biomejs/biome@2.5.1 check src/components/NotesPanel.tsx src/components/NotesPanel.test.tsx src/types/index.ts src/store/index.test.ts src/hooks/useTauriEvents.test.ts src/i18n/locales/en.json src/i18n/locales/pt.json`

Remaining before closing `audio-graph-d5a4`:

- Reorder semantics if product still wants ordered note sections beyond stable
  upsert/delete item lists.
- Replay/eval coverage for note stability across recorded sessions.
- Cross-platform GitHub/Blacksmith validation on a clean branch.

## Note reorder projection operation slice - 2026-06-25

Closed focused child `audio-graph-6347` and closed notes retcon Seed
`audio-graph-d5a4`:

- Added `reorder_note` to the shared projection operation contract.
- `MaterializedNotes` can now move a stable note id to the top or after another
  note id while preserving deterministic sequence and basis validation through
  the existing validated patch path.
- Invalid reorder requests fail before commit with explicit missing-note errors,
  preserving atomic apply semantics.
- Projection schema validation accepts `reorder_note` only for notes patches and
  includes it in notes prompt guidance.
- Frontend `ProjectionOperation` typing and NotesPanel revision counts include
  `reorder_note`.
- `audio-graph-d5a4` is now closed. Replay/eval remains tracked by downstream
  `audio-graph-3886`, and graph-specific retcons remain in `audio-graph-6008`.

Validation completed locally on Linux:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud materialized_notes -- --nocapture`
- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_llm -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`
- `bun run test src/components/NotesPanel.test.tsx src/store/index.test.ts src/hooks/useTauriEvents.test.ts`
- `bun run typecheck`
- `jq empty src/i18n/locales/en.json src/i18n/locales/pt.json`
- `bunx @biomejs/biome@2.5.1 check src/components/NotesPanel.tsx src/components/NotesPanel.test.tsx src/types/index.ts src/store/index.test.ts src/hooks/useTauriEvents.test.ts src/i18n/locales/en.json src/i18n/locales/pt.json`

Unblocked after closing `audio-graph-d5a4`:

- `audio-graph-3886` still waits on `audio-graph-6008`.
- `audio-graph-9d93` remains downstream integration work.

## Materialized graph confidence and temporal metadata slice - 2026-06-25

Closed focused child `audio-graph-b57a` and advanced graph retcon Seed
`audio-graph-6008`:

- Materialized graph nodes and edges now retain projection confidence,
  `valid_from_ms`, optional `valid_until_ms`, basis, and provenance.
- Upserted graph nodes/edges populate confidence from the accepted
  `ProjectionPatch.confidence` and temporal validity from `created_at_ms`.
- The new metadata fields deserialize with safe defaults so older materialized
  graph artifacts remain readable.
- This prepares later invalidate/merge/split graph operations to update
  temporal validity and confidence without changing the artifact shape again.

Validation completed locally on Linux:

- `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud materialized_graph -- --nocapture`
- `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --check`

Remaining before closing `audio-graph-6008`:

- Semantic graph retcon operations for invalidate/strengthen/weaken/merge/split.
- Frontend/event semantics for explicit graph retcon actions beyond materialized
  graph snapshot replacement.
- Recorded-session replay/eval fixtures that prove corrections do not duplicate
  entities or edges.
