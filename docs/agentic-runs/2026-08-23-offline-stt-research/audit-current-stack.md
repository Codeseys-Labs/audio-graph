# Audit: AudioGraph's current speech stack (as of 2026-08-23, commit-local, d0a1e0a-era)

## Verdict

AudioGraph's speech stack is **not a plugin architecture** — it is a `settings::AsrProvider`
enum with one hand-written free function per provider (deepgram, aws_transcribe, sherpa_onnx,
openai_realtime, whisper, ...) inside a single 14,865-line `src-tauri/src/speech/mod.rs`, each
duplicating its own diarization-worker construction, revision bookkeeping, and projection
dispatch. There is no `trait AsrProviderRuntime` to implement. A new local engine integrates by
(1) adding an `AsrProvider` enum variant + settings fields, (2) writing a new
`run_<engine>_speech_processor` function that maps engine output into the two provider-neutral
event structs (`AsrSpanRevisionPayload`, `TurnEventPayload`), (3) adding a `provider-registry`
`ProviderDescriptor` entry, and (4) wiring model download/validation in `models/mod.rs`. Today's
**only real streaming local ASR path is CPU-only sherpa-onnx** (`sherpa-onnx = { version = "1.13",
optional = true }` — no `cuda`/`directml`/`coreml` Cargo feature requested); Whisper (the
*default* local engine) is **batch, not streaming** (`event_semantics:
TranscriptFinalOnly`, `supports_streaming: false` in the registry); the only GPU acceleration
wired anywhere today is `whisper-rs?/cuda` + `llama-cpp-2?/cuda` and `?/vulkan`, both **opt-in
Cargo features not in `default`**, so the shipped Windows `.exe` — built with `default-features`
— runs Whisper and Llama on CPU unless a maintainer explicitly rebuilds with `--features cuda`.
Diarization has three backends (Simple heuristic / Sortformer-via-parakeet-rs / sherpa-onnx
clustering) that are mutually exclusive at compile time and silently fall back to the weakest
(Simple) with only an `info`/`warn` log line — no readiness-state signal exists for this today
(`StageStatus` has no `Degraded` variant), which is exactly the seed 586b failure mode. All local
models are downloaded post-install into `app_data_dir()/models` over plain `reqwest::blocking`;
nothing is bundled as a Tauri `resource` (there is no `resources`/`externalBin` key in
`tauri.conf.json` at all) — so installer size is unaffected by model choice, but GPU runtime
DLLs/toolkits (CUDA, cuDNN, DirectML) are **not vendored anywhere** and would need a separate
packaging story.

---

## 1. Provider selection & config schema — `src-tauri/src/settings/mod.rs`

`AsrProvider` is a `#[serde(tag = "type")]` enum (`settings/mod.rs:176-294`), one variant per
provider, each carrying its own provider-specific fields inline (no shared sub-trait):

- `LocalWhisper` (default) — no fields.
- `Api { endpoint, api_key, model }` — generic OpenAI-compatible batch ASR.
- `AwsTranscribe { region, language_code, credential_source, enable_diarization }`
- `DeepgramStreaming { api_key, model, enable_diarization, endpointing_ms, utterance_end_ms, vad_events, eot_threshold, eager_eot_threshold, eot_timeout_ms, max_speakers, keyterms }` — the most fully-specified provider; `keyterms: Vec<String>` (settings/mod.rs:226-236) is the terminology/hotword mechanism today, **Deepgram-only**, connection-time-only (no mid-session growth), no Settings UI yet (config-file only).
- `AssemblyAI { api_key, enable_diarization }`
- `Soniox { api_key, model, enable_diarization, enable_language_identification, language_hints, max_speakers }`
- `SherpaOnnx { model_dir, enable_endpoint_detection }`
- `Moonshine { model_dir, enable_speaker_hints }` — **present in the config enum but backed by zero implementation** (see §4).
- `OpenAiRealtimeTranscription { api_key, model, language }`

`AsrProvider::runtime_provider_id()` (settings/mod.rs:435-447) and
`requires_cloud_content_transfer()` (settings/mod.rs:449-461) are the only cross-provider methods
— string-id lookup and a privacy-boundary bool, both hand-maintained `match` arms, not derived
from a trait. **Gladia, Speechmatics, Rev.ai are NOT in this enum** despite having client modules
under `src-tauri/src/asr/{gladia,speechmatics,revai}.rs` and credential-store keys
(`settings/mod.rs:2208,2235` — `revai_api_key`); they exist only as
`provider-registry` catalog entries with `status: ProviderStatus::Planned`
(`crates/provider-registry/src/lib.rs:2374,2404,2464`).

The whole `AppSettings` struct (which embeds `AsrProvider`, `DiarizationSettings`, etc.) exports a
JSON Schema via `schemars::schema_for!(AppSettings)` (settings/mod.rs:1359) — this is the seam a
generated-forms Settings UI reads; every new provider field must derive `schemars::JsonSchema` to
show up there.

### Diarization policy schema

`DiarizationMode` (settings/mod.rs:1002-1006): `Off | Provider (default) | Local | Hybrid`.
`DiarizationSpeakerCount` (mod.rs:1013-1019): `Auto (default) | Fixed | Unbounded`.
`DiarizationSettings { mode, speaker_count, max_speakers }` (mod.rs:1023-1026) is the **global**
policy layer; `DiarizationSettings::provider_diarization_enabled(provider_requested)` (mod.rs:1032-1040)
gates whether a *provider's own* `enable_diarization` flag is honored (only under
`Provider`/`Hybrid`). Critically: **this global policy only governs whether a cloud provider's
native diarization is trusted — it has no effect on whether the separate, always-constructed
local `DiarizationWorker` (Simple/Sortformer/Clustering) also runs.** See §3.

`AsrProvider::apply_diarization_policy` (referenced at settings/mod.rs:1106, override struct at
1069-1103) already has a precedent for the "silent override" problem 586b is about: it logs a
`log::warn!` line whenever the global policy overrides a per-provider `enable_diarization`/
`max_speakers`. **The Sortformer-asset-missing fallback in `make_diarization_config` does not use
this pattern** — it logs at `info` level only and never surfaces through `PipelineStatus`.

---

## 2. Streaming event contract — `src-tauri/src/events.rs`

There is exactly one provider-neutral contract every provider (cloud or local) must eventually
normalize into. Two structs:

**`AsrSpanRevisionPayload`** (events.rs:234-274) — the transcript-span contract:
```rust
pub struct AsrSpanRevisionPayload {
    pub span_id: String, pub provider: String, pub source_id: String,
    pub provider_item_id: Option<String>, pub transcript_segment_id: Option<String>,
    pub speaker_id: Option<String>, pub speaker_label: Option<String>, pub channel: Option<String>,
    pub text: String, pub start_time: f64, pub end_time: f64, pub confidence: f32,
    pub is_final: bool, pub stability: AsrSpanStability,   // Partial | Final
    pub revision_number: u64, pub supersedes: Option<String>,
    pub turn_id: Option<String>, pub end_of_turn: bool,
    pub raw_event_ref: Option<String>,
    pub capture_latency_ms: Option<u64>, pub asr_latency_ms: Option<u64>,
    pub received_at_ms: u64,
}
```
`AsrSpanStability` (events.rs:228-231) is just `Partial | Final` — interim/final is a 2-state
enum, not a richer stability ladder. `revision_number` + `supersedes` is how a later, more-final
hypothesis replaces an earlier interim one for the *same* `span_id` (see `TranscriptLedger` in
`projections.rs`, which rejects stale/conflicting revisions).

**`TurnEventPayload`** (events.rs:344-361) — turn-boundary contract, `TurnEventKind` (events.rs:332-342):
`SpeechStarted | SpeechFinal | UtteranceEnd | EagerEndOfTurn | EndOfTurn | TurnResumed |
LocalWindow`. `LocalWindow` exists specifically for local/rolling-window backends that don't have
a real turn concept (e.g. the clustering diarizer). `end_of_turn: bool` also appears directly on
`AsrSpanRevisionPayload` (events.rs:266) as a convenience flag.

**`DiarizationSpanRevisionPayload`** (events.rs:290-329) — separate provider-neutral speaker
contract with its own 3-state stability enum `DiarizationSpanStability` (events.rs:279-287):
`Provisional | Stable | Final`, plus `basis_asr_span_ids`/`basis_transcript_segment_ids` linking a
speaker-span revision back to the transcript spans it labels. This is emitted by
`emit_diarization_span_revision` (speech/mod.rs, `DiarizationEventSink` trait at line 170) — the
**one** real `trait` in the diarization emission path, but it is a narrow event-sink
abstraction (`fn emit_diarization_span_revision(&self, payload)`), not an engine/runtime trait.

A new local engine's job is to produce `AsrSpanRevisionPayload`/`TurnEventPayload`/
`DiarizationSpanRevisionPayload` values with correct `stability`/`revision_number`/`supersedes`
semantics — there is no adapter layer that does this for you; every existing provider function
(deepgram, aws, openai_realtime, sherpa) hand-builds these structs inline.

---

## 3. Provider dispatch — no trait, one free function per provider (`speech/mod.rs`)

`speech/mod.rs` is 14,865 lines. It has **no provider trait**. Instead there is one
`run_<x>_speech_processor` / `run_<x>_event_receiver` function per provider, called from a
dispatcher (in `commands.rs`, start-transcription command) that matches on `AsrProvider`. Grep of
`make_diarization_config` call sites shows the pattern repeats across at least 7 provider paths:

| Line | Function | Provider |
|---|---|---|
| 4935 | `run_speech_processor` (Whisper worker body) | `local_whisper` |
| 5219 | `run_speech_processor_diarization_only` | fallback when ASR itself fails to load |
| 5622 | (sherpa-adjacent path) | local |
| 6444 | `run_deepgram_event_receiver` | `deepgram` (cloud, native diarization) |
| 7846 | `run_openai_realtime_event_receiver` | `openai_realtime` (cloud, no native speaker labels) |
| 8094 | `run_aws_transcribe_speech_processor` | `aws_transcribe` (cloud, native diarization) |
| 8351 | (sherpa streaming processor) | `sherpa_onnx` (local) |

**Key finding — this is the mechanism behind seed `audio-graph-586b`:** every one of these
functions unconditionally calls `make_diarization_config(&config.models_dir)`
(speech/mod.rs:1174-1218) and constructs a local `DiarizationWorker`, **regardless of the global
`DiarizationSettings.mode`.** For a provider with native diarization (Deepgram, AWS), the local
worker is only *invoked* as a fallback when the provider segment arrives with no speaker label
yet (`run_deepgram_event_receiver` body: `let final_segment = if segment.speaker_label.is_some()
{ segment } else { ...diarization_worker.process_input(input)... }`). So the failure chain is:
(a) provider segment lacks a label for some reason → local fallback activates silently, then (b)
`make_diarization_config` finds the Sortformer ONNX file missing on disk → downgrades to `Simple`
heuristic backend, **also silently** (`log::info!`, speech/mod.rs:1211-1216: *"Sortformer model
not found... using Simple diarization backend. Download via Settings → Models..."*). Neither hop
touches `PipelineStatus.diarization` (a `StageStatus` with only `Idle | Running | Error` variants
— events.rs:190-200 — there is structurally no `Degraded` state to report into), so the frontend
has no way to know diarization silently downgraded to signal-heuristics.

`make_diarization_config` itself (speech/mod.rs:1174-1218) picks backend by **file existence
only**, in this precedence: `diarization-clustering` feature + both clustering model files present
→ `Clustering`; else Sortformer file present → `Sortformer`; else → `Simple` (`DiarizationConfig::default()`).
It never reads `DiarizationSettings.mode` at all — mode governs only whether a *provider's*
`enable_diarization` is honored, not which local backend gets built.

---

## 4. Local ASR engines that exist today

### 4a. Whisper (`asr/mod.rs`) — default, batch, no streaming

`AsrWorker::transcribe_segment` (asr/mod.rs:289-371) is `whisper-rs` batch inference per
~2-second `SpeechSegment`: `FullParams::new(SamplingStrategy::Greedy)`, `n_threads: 4` (default,
`AsrConfig::default()` at mod.rs:244-254), `set_single_segment(false)`, `set_no_context(true)`.
Confidence is a "rough proxy": `1.0 - whisper_seg.no_speech_probability()` (mod.rs:342). No word
timestamps beyond whisper's own segment-level start/end. `speaker_id`/`speaker_label` are always
`None` at this layer — "filled by diarization later" (mod.rs:349-350). Registry confirms this is
**not streaming**: `event_semantics: TranscriptFinalOnly`, `supports_streaming: false`,
`supports_partial_revisions: false` (provider-registry/src/lib.rs:2102,2110-2111). This is the
single biggest gap against the stated latency requirement — the *default* local engine is batch.

### 4b. Sherpa-ONNX streaming (`asr/sherpa_streaming.rs`) — CPU-only, minimal API surface

`SherpaStreamingWorker` (sherpa_streaming.rs:26-142) wraps `sherpa_onnx::OnlineRecognizer`
(Zipformer transducer). Construction hardcodes `provider = Some("cpu")` (line 83) and
`num_threads = 2` (line 82) — **no GPU path exists in this file at all**, and the crate
dependency itself (`Cargo.toml:268`, `sherpa-onnx = { version = "1.13", optional = true }`) has
zero features enabled (no `cuda`, `directml`, `coreml`), matching the CPU-only claim.
`process_chunk(&mut self, samples: &[f32]) -> Option<(String, bool)>` (line 111) is the *entire*
output contract: text + `is_endpoint` bool. No confidence, no word timestamps, no interim-vs-final
distinction beyond the endpoint boundary, no speaker hint. Whatever maps this into
`AsrSpanRevisionPayload` (in the speech/mod.rs sherpa processor at ~line 8351) has to synthesize
everything else. Required model files are hardcoded literals: `encoder-epoch-99-avg-1.onnx`,
`decoder-epoch-99-avg-1.onnx`, `joiner-epoch-99-avg-1.onnx`, `tokens.txt` (lines 42-56), checked
via `Path::exists()` before construction, erroring (not falling back) if any is missing.

### 4c. Moonshine (`asr/moonshine.rs`) — the closest thing to a trait, but unimplemented

This module is explicitly documented as a seam, not a working engine: *"This module is
intentionally backend-only and native-library-free for the first slice: it defines the fakeable
contract... The later native runtime can implement `MoonshineStreamingAdapter`... without changing
the transcript ledger semantics tested here"* (moonshine.rs:1-7). Three traits:
- `MoonshineStreamingAdapter { start, accept_pcm, poll_updates, stop }` (moonshine.rs:155-164) —
  the fakeable/testable contract.
- `MoonshineNativeRuntime { runtime_version, accept_pcm, poll_updates, stop }` (moonshine.rs:236-245),
  gated on `#[cfg(feature = "asr-moonshine")]`.
- `MoonshineNativeRuntimeLoader { load(&self, config) -> Result<Box<dyn MoonshineNativeRuntime>, _> }`
  (moonshine.rs:248-253) — a loader-injection seam for testing.

But `asr-moonshine = []` in Cargo.toml (line 68) — **zero dependencies**, and the only
`MoonshineNativeRuntimeLoader` impl shipped is `MoonshineUnavailableNativeLoader`, whose `load()`
always returns `Err("Moonshine native C API adapter is not linked in this build")`
(moonshine.rs:259-269). Confirmed in the registry: `asr.moonshine` has `status:
ProviderStatus::Planned` (provider-registry/src/lib.rs:2274) even though it already has a full
`AsrProvider::Moonshine{model_dir, enable_speaker_hints}` settings variant and a
`ProviderDescriptor` with `supports_streaming: true, supports_partial_revisions: true,
supports_diarization: true` declared as *target* capabilities. This is the pattern a new
Rust-native local engine should probably follow for its seam shape (fakeable trait +
native-runtime trait + loader-injection stub), but note the native runtime itself has never been
built here — there's no working example of the native side in this codebase.

### 4d. Diarization backends (`diarization/mod.rs`, `diarization/clustering.rs`, `diarization/worker.rs`)

Three mutually-adjusted backends behind one `DiarizationBackend` enum (diarization/mod.rs:97-115):
- `Simple` (default) — pure-Rust RMS energy + zero-crossing-rate + mean-abs-deviation fingerprint,
  "always available; works as a fallback" (mod.rs:8-10). This is the phantom-speaker source seed
  586b measured (6 labels on a 3-voice session).
- `Sortformer { model_path }` — NVIDIA Sortformer v2 ONNX via `parakeet-rs` (`features =
  ["sortformer"], default-features = false` — Cargo.toml:263-266), max 4 speakers
  (`SORTFORMER_MAX_SPEAKERS`, mod.rs:58), gated behind the `diarization` Cargo feature. If the
  feature is off or the model fails to load, `DiarizationWorker::new` downgrades to `Simple` and
  **resets the whole config to `DiarizationConfig::default()`** (mod.rs:277-343) — logged at
  `log::warn!` this time (mod.rs:296,313) unlike the `make_diarization_config` info-level miss.
- `Clustering { segmentation_model, embedding_model, threshold }` — unbounded speaker count via
  sherpa-onnx `OfflineSpeakerDiarization` (pyannote segmentation + 3D-Speaker/TitaNet embedding +
  `FastClusteringConfig`), behind the `diarization-clustering` feature. **Mutually exclusive with
  `Sortformer`/`parakeet-rs` at compile time** — a `compile_error!` guard in `lib.rs` fails the
  build if both features are enabled simultaneously (ORT link conflict between parakeet-rs's and
  sherpa-onnx's own ONNX Runtime linkage). This is the one backend with a genuinely different
  runtime shape: it runs on a **dedicated thread with its own rolling window**
  (`LiveDiarizationWorker`, worker.rs), fed via a lock-free SPSC `ringbuf`, rather than being
  driven inline per-utterance like Simple/Sortformer.

All three normalize into the provider-neutral `SpeakerTimeline` / `DiarizationSpanRevision` ledger
in `projections.rs` (ADR-0017, "Provider-neutral SpeakerTimeline ledger", landed 2026-06-28) — this
part of the diarization seam *is* clean: whatever new local engine emits, it should target
`DiarizationSpanRevisionPayload` with the right `stability`/`basis_*_ids` fields, same as every
existing backend.

---

## 5. Acceleration reality check (Cargo.toml) — CPU-only by default everywhere except the two gated engines

```toml
default = ["local-ml"]
local-ml = ["asr-whisper", "llm-llama", "llm-mistralrs"]
cuda    = ["whisper-rs?/cuda", "llama-cpp-2?/cuda"]      # NOT in default
vulkan  = ["whisper-rs?/vulkan", "llama-cpp-2?/vulkan"]  # NOT in default
whisper-rs  = { version = "0.16.0", optional = true, features = ["metal"] }   # macOS-target block, line 126
llama-cpp-2 = { version = "0.1.139", optional = true, features = ["metal"] }  # macOS-target block, line 127
sherpa-onnx = { version = "1.13", optional = true }                            # zero features — CPU ORT
parakeet-rs = { version = "0.3", features = ["sortformer"], default-features = false, optional = true }
mistralrs   = { version = "0.8", optional = true }                             # CPU features (comment: "Metal GPU acceleration requires full Xcode... use default (CPU) features here")
```

Facts:
- `cuda`/`vulkan` are **weak-ref opt-in Cargo features**, not part of `default`. The shipped
  Windows `.exe` (built without an explicit `--features cuda`) runs Whisper/Llama on CPU.
  ADR-0007 confirms this is deliberate (Option B: `default = ["local-ml"]` for build-time
  parity, not runtime acceleration).
- `sherpa-onnx` and `parakeet-rs` (the two engines actually used for **streaming** ASR and
  neural diarization) have **no GPU feature requested at all** — not even opt-in. sherpa-onnx's
  upstream crate does expose CUDA/DirectML/CoreML/NNAPI onnxruntime execution providers, but
  nothing in this repo's Cargo.toml turns them on. **This is the actual current-state gap the
  "multi-backend acceleration" epic (1dd6) has to close** — it's not a matter of flipping an
  existing flag, it's wiring new ones for two crates that don't have them wired at all today.
  UNVERIFIED (would need crates.io/docs.rs check, out of scope for a read-only file audit): exact
  feature names sherpa-onnx 1.13 exposes for `directml`/`coreml`/`cuda` — flagging for the
  research angle to confirm against docs.rs, not verified here.
- No DirectML, ROCm, or MLX/Metal feature strings appear anywhere in `Cargo.toml` for any local
  engine except macOS-target-block Metal for whisper-rs/llama-cpp-2 (unconditionally on for macOS
  builds, lines 126-127 — always compiled with Metal on macOS, not user-toggleable).
- Runtime CUDA/cuDNN/DirectML **redistributables are not bundled**: `tauri.conf.json`'s `bundle`
  block (lines 27-35) has no `resources` or `externalBin` key, and NSIS is the only Windows
  target (`"targets": ["app", "dmg", "nsis", "appimage", "deb"]`). If a future GPU build links
  dynamically against CUDA/cuDNN, the installer does not currently vendor those DLLs — this is a
  packaging design question the new-engine plan must answer explicitly, not an existing solved
  seam.

---

## 6. Model download & packaging (`src-tauri/src/models/mod.rs`)

Models are **never bundled** in the Tauri installer. `get_models_dir(app)` (models/mod.rs:369-379)
resolves to `<app_data_dir>/models`, created at runtime if missing. All downloads go through a
shared blocking `reqwest::blocking::Client` with connect/read timeouts (`build_download_client`,
mod.rs:41-46, guarding against a P4-class hang-forever bug). Three download shapes exist
(`ModelDef` enum implied by the comment at mod.rs:63-68, not re-read in full — UNVERIFIED exact
struct name beyond what's quoted):
1. **Bare file** — direct download to `filename`, size-verified within 1% tolerance
   (`verify_model_file`, mod.rs:389-404). Used for Whisper ggml files, the LFM2 extraction LLM,
   the Sortformer ONNX, and the TitaNet embedding model.
2. **Archive** — `.tar.bz2` extraction (`tar` + `bzip2` crates, Cargo.toml deps), used for the
   Sherpa Zipformer streaming model and the pyannote segmentation-3.0 model. Verified by
   `verify_archive_dir`: every required file present, a regular file, and non-empty
   (mod.rs:413-420) — explicitly guards against a truncated/interrupted unpack reporting ready.
3. **Component directory** — used by Moonshine's planned models (`component_required_files`,
   referenced in `model_exists_and_is_valid`, mod.rs:422-434), same non-empty-file verification.

Known model URLs (`models/mod.rs`, all hardcoded string literals):
- Whisper: `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{tiny,base,small,medium,large-v3}.en.bin` (or `-v3` for large)
- LLM extraction model: `https://huggingface.co/LiquidAI/LFM2-350M-Extract-GGUF/resolve/main/LFM2-350M-Extract-Q4_K_M.gguf`
- Sortformer: `https://huggingface.co/altunenes/parakeet-rs/resolve/main/diar_streaming_sortformer_4spk-v2.onnx`
- Sherpa Zipformer 20M: `https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17.tar.bz2`
- Moonshine tiny/small/medium streaming-en: `https://download.moonshine.ai/model/{tiny,small,medium}-streaming-en/quantized`
- Pyannote segmentation-3.0: `https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2`
- TitaNet embedding: `https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx`

Progress is reported via a `model-download-progress` Tauri event carrying `DownloadProgress`
(mod.rs:329-342: `model_id, model_name, bytes_downloaded, total_bytes, elapsed_ms, percent,
status`), `total_bytes: 0` meaning "unknown" (no `Content-Length`). `ModelReadiness` (mod.rs:346-351)
is a 3-state enum: `Ready | NotDownloaded | Invalid` — this exists and is exposed via the
`get_model_status` Tauri command (commands.rs:4453-4454 → `models::get_model_status`), but as
shown in §3, **the diarization backend-selection code path (`make_diarization_config`) does not
call `get_model_status` at all** — it does its own ad-hoc `Path::exists()` check, disconnected
from the readiness type the frontend already has a command to read. This is a concrete, cheap
seam gap: the fix for 586b's readiness-notice ask could reuse `get_model_status`/`ModelReadiness`
rather than inventing a new signal.

Download-in-progress writes to a sibling `.download` temp file, then atomically renames onto the
target path on success (mod.rs:605-611) — a stale `.download` from an interrupted process is
deleted before starting (mod.rs:611-614). This pattern (temp file + verify + atomic rename) is the
one a new engine's model fetcher should reuse rather than reinvent.

---

## 7. Implications for AudioGraph (offline STT v2 planning)

1. **There is no trait to slot a new engine behind.** Any "Rust-native local model serving with
   multi-backend acceleration" plan must either (a) write yet another
   `run_<engine>_speech_processor` free function following the existing copy-paste pattern (fast,
   consistent with today, but extends a 14.8k-line file and duplicates diarization-fallback
   wiring), or (b) introduce a real `trait AsrEngineRuntime` and refactor at least the local
   engines (Whisper, Sherpa, the new engine) behind it — which is a larger, cross-cutting change
   touching `speech/mod.rs`, `settings/mod.rs`'s `AsrProvider` enum, and the `provider-registry`
   crate. The Moonshine module's three-trait shape
   (`MoonshineStreamingAdapter`/`MoonshineNativeRuntime`/`MoonshineNativeRuntimeLoader`) is the
   best in-repo precedent for what a real trait boundary would look like, but it has never been
   exercised by a working native runtime — treat it as an unvalidated sketch, not a proven pattern.

2. **The default local engine (Whisper) is batch, not streaming** — `event_semantics:
   TranscriptFinalOnly`. If the new engine is meant to *replace* Whisper as the default local
   path (matching the "streaming beats batch accuracy" latency priority), that is a bigger swap
   than "add a new provider option" — it changes what `local_whisper`/`ui_selectable` defaults
   mean product-wide, and the `provider_registry.rs` MVP-selectability gating
   (`provider_id_is_mvp_selectable`) is a separate axis from `ProviderStatus` that would need a
   deliberate decision either way (ADR-0033 precedent).

3. **No GPU acceleration is wired for the two engines that actually stream today**
   (sherpa-onnx, parakeet-rs). The "leverage whatever acceleration the user has" goal is not a
   matter of flipping existing flags for the ASR/diarization local path — CUDA/Vulkan feature
   wiring exists only for whisper-rs/llama-cpp-2 (batch ASR + LLM), and even those are opt-in,
   not default. A new engine's acceleration story starts from zero for streaming ASR and
   diarization specifically.

4. **Two silent-fallback layers stack for diarization** (provider-didn't-label → local Simple/
   Sortformer/Clustering selection by file-existence → Simple heuristic if the neural model is
   missing), neither of which is visible in `PipelineStatus` (`StageStatus` has no `Degraded`
   variant) despite the app already having a `ModelReadiness`/`get_model_status` command that
   could report it. Any new engine inherits this blind spot unless the readiness-surfacing part
   of 586b's fix lands first (per the seed's own stated sequencing) or the new engine's plan
   explicitly wires its own model-readiness into `PipelineStatus`.

5. **Terminology/hotword seeding today is Deepgram-only** (`keyterms: Vec<String>` on
   `DeepgramStreaming`, connection-time-only, no UI). A local engine wanting hotword/keyterm
   biasing (mentioned as a target feature-parity item) has no existing local analog to extend —
   sherpa-onnx's `OnlineRecognizerConfig` and Whisper's `FullParams` are both unused for
   vocabulary biasing in this codebase today; this would be new integration work against
   whichever local engine wins, not a matter of copying an existing local pattern.

6. **Models are never bundled**, so installer-size pressure from a new engine's weights is a
   non-issue for the base installer, but (a) the download-progress/verification infrastructure
   in `models/mod.rs` is reusable and should be extended rather than replaced, and (b) if the new
   engine needs GPU runtime redistributables (CUDA/cuDNN DLLs, DirectML), there is currently no
   packaging mechanism for that at all (`tauri.conf.json` has no `resources`/`externalBin`) —
   this is new plumbing, not an extension of an existing seam.

7. **Provider-registry (`crates/provider-registry/src/lib.rs`, 4,499 lines) is the most complete,
   most future-proofed part of the stack** — its `ProviderDescriptor` taxonomy (transport,
   credential keys, required Cargo features, local-model requirements, event semantics, STT
   fidelity, audio format/attribution, lifecycle, privacy) already has fields for everything a
   new local engine needs to declare (`LocalModelRequirement { model_id, kind: File|Directory,
   required_files }`, `ModelCatalogPolicy::LocalFiles`, `ProviderTransport::Local`,
   `ProviderEventSemantics::TranscriptPartialFinalTurns`). Architects should treat this crate as
   the canonical place to declare the new engine's capabilities, and the `ProviderStatus::Planned`
   → `Implemented` promotion (as already modeled for Moonshine/Gladia/Speechmatics/RevAI) as the
   existing, working "half-wired provider" pattern to follow during a phased rollout.
