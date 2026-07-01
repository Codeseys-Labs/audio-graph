# Moonshine Local STT Runtime Decision - 2026-06-25

## Decision

Use the Moonshine Voice native C API as AudioGraph's production Moonshine
runtime, behind a new optional `asr-moonshine` Cargo feature.

Do not use Python, Hugging Face Transformers, or a hand-rolled ONNX Runtime
implementation as the app runtime. Those remain useful for model download,
benchmarking, and reference behavior, but the desktop app should keep capture,
provider lifetime, PCM buffers, transcript normalization, and persistence in the
Rust backend.

## Why This Fits AudioGraph

AudioGraph's provider architecture already has the right seams:

- local providers are described by the backend provider registry;
- local runtime files are declared through model descriptors;
- compiled-heavy local engines are optional Cargo features;
- the processed audio bus already emits 16 kHz mono `f32` PCM for local
  providers;
- ASR providers normalize provider-specific revisions into transcript spans
  before notes and graph projection.

Moonshine's C API matches that shape. Its transcriber loads a model directory,
accepts 16 kHz float PCM, owns streaming state through explicit stream handles,
and returns transcript lines with stable line ids, completion flags, update
flags, timing, optional word timings, latency, and speaker metadata. That is
enough to map Moonshine lines into AudioGraph span revisions without moving the
pipeline into React or a Python sidecar.

## Runtime Options Considered

### Chosen: Moonshine Voice C API

Use upstream Moonshine's C/C++ core and bind it from Rust. The upstream project
documents a C interface for unsupported languages, CMake builds across
platforms, prebuilt libraries for supported systems, and model directories.
Older non-streaming English model directories use a smaller three-file layout,
but that is not the AudioGraph streaming contract.

The current English streaming model directories exposed by the upstream
downloader contain:

- `adapter.ort`
- `cross_kv.ort`
- `decoder_kv.ort`
- `decoder_kv_with_attention.ort`
- `encoder.ort`
- `frontend.ort`
- `streaming_config.json`
- `tokenizer.bin`

The Rust adapter should call the C API through a small unsafe wrapper and expose
a safe `MoonshineStreamingWorker` shaped like the existing Sherpa worker. The
first supported runtime target should be English streaming small or medium, with
architecture constants recorded in code and provider registry metadata.

Tradeoffs:

- Best fit for a Rust backend and Tauri packaging.
- Reuses upstream VAD, streaming cache, tokenizer, line tracking, and speaker
  metadata instead of duplicating them.
- Adds native binary packaging work and ONNX Runtime compatibility risk.
- Requires cross-platform CI proof before becoming a selectable default.

### Rejected As Runtime: Python / PyPI `moonshine-voice`

The Python package is useful for downloading model files and checking expected
behavior, but shipping Python as the runtime would add interpreter discovery,
venv management, process lifetime, IPC, logging, and platform packaging
failure modes. It would also split long-lived provider state outside the Rust
backend, against the repo guardrail that provider sockets/runtime state and PCM
live in Rust.

### Rejected As Runtime: Hugging Face Transformers

Transformers exposes Moonshine and Moonshine Streaming checkpoints and is a good
reference for model behavior. It is a poor production runtime for this app
because it brings Python/PyTorch/Transformers dependency weight, GPU/device
selection complexity, and a separate audio/transcript lifecycle.

### Deferred: Sherpa-ONNX Moonshine Packaging

Sherpa-ONNX is already a local ASR dependency pattern in AudioGraph, and there
are community Moonshine ONNX packages. Keep this as a fallback if native
Moonshine packaging proves too fragile. It should not be the first path because
the observed Moonshine package does not prove the full streaming line/update
semantics that Moonshine's own C API exposes.

### Rejected For First Slice: Direct Rust ONNX Runtime

A direct ORT implementation would avoid upstream binary bundles, but it would
also require reimplementing or binding Moonshine's tokenizer, VAD/endpointing,
stream cache, line identity, speaker metadata, and model-specific options. That
is higher risk than using the upstream C API first.

## Audio Contract

Input:

- AudioGraph processed audio: 16 kHz mono `f32` PCM.
- Moonshine API input: float PCM in the range `-1.0..1.0`, internally optimized
  for 16 kHz.
- No React audio path. `rsac` capture, resampling, source timing, queueing, and
  provider polling stay backend-owned.

Chunking:

- Feed natural processed-audio chunks into `moonshine_transcribe_add_audio_to_stream`.
- Poll `moonshine_transcribe_stream` on a configurable interval. Start with
  `500 ms` to match upstream defaults, then tune down only if telemetry shows
  acceptable CPU headroom.
- Do not call the transcription poll from the realtime audio callback. Keep it
  on the provider worker thread so the capture bus stays bounded.

Output mapping:

- `transcript_line_t.id` becomes `provider_item_id`.
- AudioGraph `span_id` should derive from provider id + session id + line id.
- `is_complete == 0` maps to a partial revision.
- `is_complete != 0` maps to a final revision/end-of-turn.
- `is_updated`, `is_new`, and `has_text_changed` gate whether AudioGraph emits
  a new span revision.
- `start_time` and `duration` map to transcript timing.
- `speaker_id` and `speaker_index` are provider-native speaker hints. They
  should enter the speaker timeline as provisional provider labels, not as a
  replacement for AudioGraph's normalized diarization workstream.
- `last_transcription_latency_ms` should feed provider latency telemetry.

## Model And Download Contract

Model descriptors should treat Moonshine models as directory models, not single
files. Required runtime files for English streaming models:

- `adapter.ort`
- `cross_kv.ort`
- `decoder_kv.ort`
- `decoder_kv_with_attention.ort`
- `encoder.ort`
- `frontend.ort`
- `streaming_config.json`
- `tokenizer.bin`

Initial model entries:

- `moonshine-small-streaming-en` as the default if packaging size is acceptable.
- `moonshine-medium-streaming-en` as the higher-quality preset.
- `moonshine-tiny-streaming-en` as the low-resource fallback if small is too
  heavy for CI or first-run download.

Downloader plan:

- The upstream Python downloader resolves English streaming components from
  `https://download.moonshine.ai/model/{tiny,small,medium}-streaming-en/quantized/{component}`.
- Product-ready implementation must use AudioGraph's Rust model downloader so
  model setup works on macOS, Windows, and Linux without Python.
- Do not auto-download on app start. Settings readiness should show missing
  model files with a deliberate download action.

## Cargo And Packaging Shape

Feature layout:

```toml
asr-moonshine = []
```

Implementation should not add Moonshine to `default` or `local-ml` until it has
cross-platform CI proof. `cloud` builds must compile without Moonshine headers,
native libraries, or model files.

Native library strategy:

- Add a `src-tauri/src/asr/moonshine.rs` safe wrapper around a small FFI layer.
- Add build-time discovery that accepts an explicit `MOONSHINE_ROOT` or a
  versioned repo-local/prebuilt bundle path.
- Prefer dynamic linking during the first spike to reduce static CRT and ORT
  conflict risk on Windows.
- Record the upstream Moonshine version and model architecture in provider
  readiness so support reports can identify the runtime.
- Cross-platform proof must include cloud/no-feature builds and
  `asr-moonshine` compile smoke on Linux, macOS, and Windows.

## Acceptance For The First Implementation Slice

First slice: provider skeleton and packaging probe, not live transcription.

Acceptance:

- `asr-moonshine` feature exists and is off by default.
- Provider registry has `asr.moonshine` with local-only privacy, local directory
  model requirements, `LocalStreamingRuntime`, partial/final semantics, and
  `asr-moonshine` as the required feature.
- `AsrProvider::Moonshine` exists but returns structured
  `provider_unavailable` when the feature is not compiled in.
- Model readiness reports missing Moonshine directory files without hardcoded
  frontend checks.
- A no-model compile smoke proves the wrapper and build script are shaped
  correctly on at least Linux locally, with macOS/Windows delegated to CI.
- No Settings UI branch makes Moonshine selectable until readiness and runtime
  probes exist.

Second slice: runtime worker and normalized transcript mapping.

Acceptance:

- Load transcriber from a model directory.
- Create/start/stop/free one stream per AudioGraph source policy.
- Feed 16 kHz `f32` PCM chunks from the processed-audio bus.
- Poll on a bounded interval and emit span revisions only for changed lines.
- Map complete lines to final transcript events and incomplete lines to partial
  transcript revisions.
- Unit tests use a fake FFI adapter so transcript mapping is deterministic
  without native libraries or model files.

Third slice: model download/readiness and cross-platform CI.

Acceptance:

- Rust downloader knows the exact Moonshine model URLs or has a documented
  versioned manifest generated from upstream.
- Settings/provider readiness reports feature missing, model missing, runtime
  load failure, and healthy local model states.
- Blacksmith/GitHub CI validates Linux, macOS, and Windows compile smokes
  before the provider is advertised as implemented.

## Sources Checked

- Moonshine Voice launch and platform/library claims:
  <https://huggingface.co/blog/UsefulSensors/announcing-moonshine-voice>
- Moonshine Voice GitHub documentation, C/C++ integration, model downloads, and
  C API:
  <https://github.com/moonshine-ai/moonshine>
- Moonshine C API header:
  <https://github.com/moonshine-ai/moonshine/blob/main/core/moonshine-c-api.h>
- Hugging Face Moonshine Streaming model card:
  <https://huggingface.co/UsefulSensors/moonshine-streaming-medium>
- Hugging Face Transformers Moonshine Streaming docs:
  <https://github.com/huggingface/transformers/blob/main/docs/source/en/model_doc/moonshine_streaming.md>
- Sherpa-ONNX documentation:
  <https://k2-fsa.github.io/sherpa/onnx/index.html>
- Sherpa-ONNX community Moonshine package:
  <https://huggingface.co/csukuangfj/sherpa-onnx-moonshine-tiny-en-int8>
- Moonshine v2 paper:
  <https://arxiv.org/abs/2602.12241>
