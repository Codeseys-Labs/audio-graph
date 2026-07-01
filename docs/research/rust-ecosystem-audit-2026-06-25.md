# Rust Ecosystem Audit

Date: 2026-06-25

Scope: Rust libraries and existing repo dependencies that can help AudioGraph's cross-platform audio capture, streaming STT, diarization, provider runtime, LLM streaming, credentials/config, and CI roadmap.

## Executive Takeaways

- The backend architecture is directionally right: keep capture, provider sockets, credentials, local ML, diarization, and timing in Rust/Tauri. React should remain configuration/control/display.
- The biggest opportunity is reuse and consolidation, not replacing the stack. `rsac`, `rubato`, `tokio-tungstenite`, `tokio-util`, `sherpa-onnx`, `parakeet-rs`, `reqwest` streaming, `serde_yaml`, `dirs`, and `zeroize` already cover much of the needed surface.
- Do not add provider-specific SDKs unless they remove real protocol risk. A shared internal transport/parser harness over existing primitives is more valuable than one-off wrappers.
- Config and credentials need an ADR. The current YAML split is product-useful, but `serde_yaml` maintenance status and plaintext local secret storage are operational risks.
- Local diarization/VAD/AEC needs measured bakeoffs on curated clips. The hard work is speaker identity stabilization, echo/noise behavior, and transcript/projection alignment, not finding a crate that claims diarization.

## Existing Capability To Exploit More

### Audio Capture And Resampling

Current repo capability:

- `rsac` is already the backend-owned capture/source abstraction with OS-specific path dependency features in `src-tauri/Cargo.toml`.
- `rubato` and `audioadapter-buffers` are already available and used by the 16 kHz mono ASR pipeline.
- `cpal` and `ringbuf` already power playback, with a dedicated audio thread to avoid Windows `cpal::Stream` sendability issues.

Gaps:

- `src-tauri/src/playback/mod.rs` still documents that playback has no output resampling, so 24 kHz TTS/native-S2S audio can pitch-shift on common 48 kHz devices.
- `src-tauri/src/asr/openai_realtime.rs` has a local linear 16 kHz to 24 kHz resampler. The comment makes a reasonable latency argument, but this should be explicitly compared against a shared provider-resampling helper before more providers clone local resamplers.

Decision:

- Keep `rsac` as the capture layer.
- Use `rubato` for playback/provider resampling where callback safety can be preserved.
- Filed `audio-graph-f53b` for playback output resampling.

Sources:

- `src-tauri/Cargo.toml`
- `src-tauri/src/audio/pipeline.rs`
- `src-tauri/src/playback/mod.rs`
- `src-tauri/src/asr/openai_realtime.rs`
- https://docs.rs/rubato
- https://docs.rs/cpal

## VAD, AEC, And Local Turn Detection

Candidate libraries:

- `earshot`: pure Rust VAD candidate, attractive because it avoids ONNX Runtime conflicts.
- `silero` / `fast-vad` / `wavekat-vad`: ONNX/model-backed VAD candidates; potentially stronger than simple RMS/ZCR, but packaging and ORT conflicts matter.
- `webrtc-audio-processing`: WebRTC AEC/NS/AGC path with native build complexity.
- `sonora`: pure-Rust WebRTC-audio-processing-inspired direction worth evaluating, but newer.

Decision:

- Do not pick a VAD/AEC crate by claims alone. Run a bakeoff on 16 kHz chunks and echo/playback-reference fixtures.
- Filed `audio-graph-0bdc`, blocking barge-in work, for VAD/AEC bakeoff.

Sources:

- https://docs.rs/earshot
- https://github.com/tonarino/webrtc-audio-processing
- https://github.com/dignifiedquire/sonora

## Diarization And Speaker Identity

Current repo capability:

- `parakeet-rs` Sortformer is already optional for streaming diarization.
- `sherpa-onnx` is already optional for unbounded offline/rolling clustering, speaker embeddings, and streaming ASR.
- AudioGraph already has speaker stabilization and revision logic in `src-tauri/src/diarization/stabilize.rs` and `src-tauri/src/diarization/worker.rs`.

Gaps:

- Provider streaming paths that lack native speaker labels sometimes construct `DiarizationInput` with empty audio, which limits local fallback quality.
- `parakeet-rs` and `sherpa-onnx`/ORT feature combinations are conflict-prone and need explicit CI matrix coverage.
- Additional diarization crates such as pyannote-style wrappers may help, but only after curated labeled clips exist.

Decision:

- Keep local diarization as backend-owned and revisioned.
- Favor `sherpa-onnx`/existing stabilization for unbounded speaker identity before adding another clustering crate.
- Add a later bakeoff only after curated labeled multi-speaker clips exist.

Sources:

- `src-tauri/src/diarization/mod.rs`
- `src-tauri/src/diarization/clustering.rs`
- `src-tauri/src/diarization/stabilize.rs`
- `src-tauri/src/diarization/worker.rs`
- https://docs.rs/sherpa-onnx
- https://docs.rs/parakeet-rs

## Provider Transport And Parser Runtime

Current repo capability:

- `tokio-tungstenite`, `futures-util`, `base64`, and `url` already support WebSocket providers.
- `reqwest` with `stream` plus the local `SseDecoder` already supports OpenAI-compatible streaming LLM providers.
- AWS Transcribe already uses AWS SDK infrastructure.

Gaps:

- Deepgram, AssemblyAI, OpenAI Realtime, Gemini, and TTS have similar reconnect/backoff/backlog/terminal semantics implemented separately.
- Soniox, Gladia, Speechmatics, and RevAI have parser/config skeletons but no shared live transport harness.
- Provider JSON parser coverage should be fixture-driven; no generic crate will encode provider-specific partial/final/revision semantics.

Decision:

- Build an internal provider transport/parser harness over existing primitives.
- Consider `insta` and `serde_path_to_error` as dev dependencies for snapshots and fixture diagnostics.
- Filed `audio-graph-d042`, blocking provider expansion, for reusable transport/parser work.

Sources:

- `src-tauri/src/asr/deepgram.rs`
- `src-tauri/src/asr/assemblyai.rs`
- `src-tauri/src/asr/openai_realtime.rs`
- `src-tauri/src/asr/soniox.rs`
- `src-tauri/src/asr/gladia.rs`
- `src-tauri/src/asr/speechmatics.rs`
- `src-tauri/src/asr/revai.rs`
- https://docs.rs/tokio-tungstenite
- https://docs.rs/eventsource-stream
- https://docs.rs/reqwest-eventsource
- https://docs.rs/insta
- https://docs.rs/serde_path_to_error

## LLM Streaming And Backpressure

Current repo capability:

- `tokio-util::CancellationToken` is already used and is the right cancellation primitive for streaming loops.
- LocalLlama now streams through the persistent actor and checks cancellation between generated tokens.
- `mistralrs` is already present and its Rust SDK exposes a streaming chat API.
- Bedrock should use `aws-sdk-bedrockruntime` rather than OpenAI-compatible fallback.

Gaps:

- `MistralRs` and `AwsBedrock` still return unsupported in the streaming dispatcher.
- Bedrock is not SSE/OpenAI-compatible; it needs a real ConverseStream adapter that reuses `aws_util`.
- Rate limits, queue caps, and cancellation acknowledgement should be provider-runtime concepts, not UI-only behavior.

Decision:

- Update existing Seeds instead of creating duplicates:
  - `audio-graph-919e`: MistralRs should use native `stream_chat_request`.
  - `audio-graph-2f4a`: Bedrock should adopt `aws-sdk-bedrockruntime`.
- Keep the projection scheduler bespoke. TTFT, basis freshness, stale-completion rejection, repair diffs, and graph/notes reconciliation are AudioGraph-specific.

Sources:

- `src-tauri/src/llm/streaming.rs`
- `src-tauri/src/llm/sse.rs`
- `src-tauri/src/llm/engine.rs`
- https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html
- https://github.com/EricLBuehler/mistral.rs
- https://docs.rs/aws-sdk-bedrockruntime
- https://docs.rs/governor
- https://docs.rs/tower

## Credentials And Config

Current repo capability:

- Non-secret settings and credentials are already split.
- `credentials.yaml` has owner-only writes, redaction tests, and `zeroize`.
- `dirs` is used for cross-platform config paths.

Gaps:

- Plaintext `credentials.yaml` is pragmatic for local dev, but not the best final product posture.
- `serde_yaml` is no longer maintained. Any migration must preserve existing user YAML semantics and legacy import behavior.
- Settings should continue loading saved keys for health/model discovery without echoing plaintext secrets to the UI.

Decision:

- Write an ADR before implementing. Evaluate `keyring`, Tauri Stronghold, `secrecy`, and YAML parser/layering replacements such as `figment` or `serde-saphyr`.
- Filed `audio-graph-1322`, blocking provider setup/settings completion, for the ADR and migration spike.

Sources:

- `src-tauri/src/credentials/mod.rs`
- `src-tauri/src/settings/mod.rs`
- https://docs.rs/keyring
- https://docs.rs/secrecy
- https://v2.tauri.app/plugin/stronghold/
- https://docs.rs/figment
- https://docs.rs/serde-saphyr
- https://github.com/dtolnay/serde-yaml

## CI, Dependency Hygiene, And Packaging

Current repo capability:

- GitHub/Blacksmith Linux/macOS/Windows jobs already cover important default/cloud paths.
- Release dry-run and cloud-only Tauri smoke coverage are already tracked elsewhere.

Gaps:

- Optional local-stack features are not all represented in visible CI.
- `src-tauri/Cargo.lock` contains duplicate versions for `reqwest`, `rubato`, `sysinfo`, and `tokio-tungstenite`; this is not automatically a bug, but it is a useful dependency-hygiene signal.
- Supply-chain tooling should be evaluated deliberately, not bolted into the dirty worktree.

Decision:

- Filed `audio-graph-fbf6`, blocking cross-platform CI readiness, for optional feature compile matrix coverage.
- Consider `cargo-nextest`, `cargo-deny`, and `cargo-vet` in a separate CI hardening pass once current workflow edits are on a clean branch.

Sources:

- `.github/workflows/ci.yml`
- `src-tauri/Cargo.lock`
- https://nexte.st/
- https://github.com/EmbarkStudios/cargo-deny
- https://github.com/mozilla/cargo-vet

## Seeds Filed Or Updated

Filed:

- `audio-graph-f53b`: Wire `rubato` output resampling into CPAL playback.
- `audio-graph-0bdc`: VAD and AEC crate bakeoff for local turn detection and barge-in.
- `audio-graph-1322`: Credential and YAML config backend migration ADR.
- `audio-graph-d042`: Reusable ASR provider transport and parser fixture harness.
- `audio-graph-fbf6`: Cross-platform optional Rust feature compile matrix.

Updated:

- `audio-graph-3818`: research audit parent.
- `audio-graph-919e`: MistralRs streaming adapter should use the native streaming API.
- `audio-graph-2f4a`: Bedrock streaming adapter should use `aws-sdk-bedrockruntime`.
- `audio-graph-ad1d`: provider roadmap now blocked by shared transport/parser harness.
- `audio-graph-c395`: cross-platform roadmap now blocked by optional feature matrix.

## Explicit Non-Goals

- Do not replace `rsac` with direct `cpal` capture.
- Do not add a generic graph/projection scheduler crate for TTFT-aware notes/graph diffs.
- Do not add source-separation crates until there is a product boundary and sidecar decision.
- Do not expose candidate STT providers as selectable until credentials, model/catalog, health, runtime parser, and cross-platform smoke evidence exist.
