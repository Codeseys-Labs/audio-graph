# Rust-native inference stacks for ASR — landscape as of 2026-08-23

**Angle:** the Rust inference-stack layer only (candle, whisper-rs/ggml, ort, sherpa-onnx, parakeet-rs, kyutai stt-rs/moshi, moonshine-rs, burn, tract; mistral.rs as a serving pattern). Model-quality comparisons and product/UX sequencing are other agents' lanes.

## Verdict

There is exactly one Rust-reachable inference stack in 2026 that covers **all four** AudioGraph requirements — true streaming ASR, streaming diarization, terminology/hotword seeding, and a Windows-first acceleration matrix that includes non-NVIDIA GPUs — and it is **ONNX Runtime, reached two different ways**: the `sherpa-onnx` C-API wrapper (k2-fsa's own crate, v1.13.5, released 2026-08-11) and the `ort` crate (v2.0.0-rc.13, ORT 1.28.0, released 2026-07-28) used directly or via `parakeet-rs` v0.3.7. Everything else has a disqualifying hole for this product: **candle 0.11.0 has no Vulkan, ROCm, or DirectML backend at all** (features are exactly `cuda`/`cudnn`/`metal`/`mkl`/`accelerate`/`nccl`) and no streaming Whisper; **whisper-rs 0.16 has the best ggml backend matrix for Windows (CUDA + Vulkan) but no cross-call encoder state**, so "streaming" is a sliding-window emulation; **kyutai's `stt-rs` is the most architecturally correct streaming design in Rust and is also the stalest** — an unpublished `kyutai-stt-rs` 0.1.0 pinning candle 0.9.1 / moshi 0.6.1 / hf-hub 0.4.3, in a repo last pushed 2026-01-26, CUDA+Metal only; **burn has the best backend story on paper (wgpu → Vulkan/DX12/Metal) and zero ASR models**; **`sherpa-rs` (thewh1teagle) is ARCHIVED** as of 2026-03-08, which is the single most important ecosystem fact here and the reason the "just use sherpa-rs" strawman is already dead. The decisive constraint is not model quality, it is **Windows GPU distribution physics**: on Windows the only zero-install acceleration paths are DirectML (`DirectML.dll`, 18.5 MB, DX12 present on every Win10 1903+ machine) and Vulkan (`vulkan-1.dll`, ships with every GPU driver). Both sherpa-onnx's and ort's CUDA archives ship a provider DLL (328.5 MB and 62.3 MB respectively) **and neither bundles cudart/cuBLAS/cuDNN** — a CUDA build forces an end-user CUDA toolkit install, which is not shippable in a bundled `.exe`. So the honest architecture is not "pick one crate": it is a **multi-backend ASR engine trait with an ORT-based streaming engine as the default and an EP ladder (DirectML → CUDA if present → CPU) plus a ggml/Vulkan whisper engine as the accuracy/offline-batch escape hatch**. That is closer to what AudioGraph already has than to a rewrite.

---

## Verification stamp

Every version/date/size below was pulled live on 2026-08-23 from `crates.io/api/v1`, the crates.io sparse index (`index.crates.io`, for authoritative feature lists), `api.github.com`, `codeberg.org/api/v1`, extracted `.crate` tarballs from `static.crates.io`, and HTTP `HEAD`/`GET` on release CDN URLs. Binary sizes are measured, not estimated. Items I could not verify are marked **UNVERIFIED**.

---

## 1. candle (Hugging Face) — 0.11.0

| Fact | Value |
|---|---|
| Latest | `candle-core` / `candle-nn` / `candle-transformers` **0.11.0**, published 2026-06-26 |
| Prior | 0.10.2 (2026-04-01), 0.9.2 (2026-01-24) |
| Pulse | repo pushed 2026-08-23 (today); 20,945 stars; 885 open issues; commits are almost entirely bugfixes (Metal correctness, CPU quant perf, FlashAttention) since the 0.11.0 version bump |
| License | MIT OR Apache-2.0 |
| Recent downloads | 2.57M (candle-core) |

**Backend matrix (from the crates.io index, `candle-core` 0.11.0 `features2`) — this is the load-bearing fact:**

```
accelerate, cuda, cudnn, metal, metal-debug-labels, mkl, nccl, ug
```

There is **no `vulkan`, no `wgpu`, no `rocm`/`hip`, no `directml`, no `sycl`, no `opencl`**. Confirmed independently by DeepWiki against the repo: "There is no mention of Vulkan, ROCm, or DirectML backends." For a Windows-primary desktop app this means candle accelerates **NVIDIA only** (CUDA), and on macOS via Metal. On a Windows machine with an AMD or Intel GPU, candle is a CPU library. `cuda` also pulls `cudarc` + `candle-kernels`, i.e. nvcc at build time.

**Whisper support.** `candle-transformers::models::whisper` has both `model.rs` and `quantized_model.rs`; the quantized path loads GGUF (`model-tiny-en-q80.gguf` in the WASM example) via quantized linear/embedding layers. Whisper is the **only** ASR architecture in `candle-transformers` — no Zipformer, Conformer, Parakeet, Moonshine, Kyutai-STT, or wav2vec2 in-tree. Decoding is fixed 30-second chunks (`N_SAMPLES = CHUNK_LENGTH * SAMPLE_RATE = 480_000`). `MultiHeadAttention` exposes a flushable KV cache, which is the raw material for streaming, but no streaming loop exists. So: **candle's streaming Whisper story in 2026 is "build it yourself."**

**Third-party pure-Rust Whisper on candle:** `whisper-candle-core` 0.1.2 (2026-07-10, 150 total downloads) — a from-scratch port claiming token-exact greedy/beam parity with the PyTorch reference and word timestamps within 0.1 s, with CPU/Metal/CUDA-behind-a-flag. Self-reported CPU speed is ~10–30× real-time on Apple Silicon. Adoption is effectively zero; treat as interesting, not adoptable.

**Distribution.** Pure Rust, so a CPU-only candle build is the cleanest possible Tauri story: no CMake, no C++ toolchain, no runtime dylibs, small binary. That is candle's genuine advantage and the reason it survives on this list at all.

**Also relevant:** `ort-candle` **0.4.1+0.11.0** (2026-08-18) exists — candle wrapped as an ONNX Runtime C-API-compatible backend for `ort` (see §3). Per ort's docs it supports CPU + WASM, with "CUDA … not available via `ort-candle` right now," and limited operator coverage.

---

## 2. whisper-rs (whisper.cpp bindings) — 0.16.0

| Fact | Value |
|---|---|
| Latest | `whisper-rs` **0.16.0** (2026-03-12), `whisper-rs-sys` **0.15.0** (2026-03-12) |
| Upstream | Codeberg `tazz4843/whisper-rs` (moved off GitHub); last commit **2026-03-14**; 67 stars there; not archived |
| Vendored whisper.cpp | **v1.8.3** (from the vendored `CMakeLists.txt`); upstream whisper.cpp is at **v1.9.3** (2026-08-20) — one minor version behind |
| License | **Unlicense** (public domain) — permissive but note it is *not* MIT/Apache, which some license scanners flag |
| Recent downloads | 573,733 |
| AudioGraph today | `whisper-rs = "0.16.0"` behind `asr-whisper`, with `cuda` / `vulkan` / `metal` wired |

**Feature matrix (crates.io index, exact):**

```
features:  coreml, cuda, hipblas, intel-sycl, metal, openblas, openmp, raw-api, _gpu, test-with-tiny-model
features2: vulkan (+_gpu, dep:libc), log_backend, tracing_backend
```

There is **no `directml`** — ggml has never had a DirectML backend, and does not need one because it has Vulkan. Cross-checking the vendored ggml tree, the backends *present in source* are: `ggml-cpu, ggml-blas, ggml-cuda, ggml-hip, ggml-metal, ggml-vulkan, ggml-sycl, ggml-opencl, ggml-webgpu, ggml-cann, ggml-musa, ggml-rpc, ggml-hexagon, ggml-zdnn, ggml-zendnn`. whisper-rs only exposes a subset as Cargo features; anything else requires passing raw `GGML_*` CMake defines (`build.rs` forwards env vars whose key starts with `GGML_`).

**Windows acceleration reality — the best in class here.** `build.rs` sets `GGML_VULKAN=ON`, resolves `VULKAN_SDK`, and links `vulkan-1`. At runtime that resolves against `vulkan-1.dll`, which ships with every modern GPU driver on Windows — **one build covers NVIDIA + AMD + Intel with no user-side SDK install**. The `cuda` feature instead emits `cargo:rustc-link-lib=cudart` + `cuda` and probes `CUDA_PATH` — nvcc required at build time, CUDA runtime DLLs required at run time. There is also a genuinely useful `whisper_rs::vulkan::list_devices()` returning `{id, name, vram.free, vram.total, buf_type}` per physical GPU — a ready-made hardware-probe primitive for a capability-detection UI.

**Streaming: no.** whisper.cpp has no cross-call encoder state. `WhisperContext::create_state()` gives you a per-decode state, not a carried one. Real-time is emulated: sliding window + `set_audio_ctx()` (truncate the 1500-frame encoder context to cut latency) + `set_single_segment(true)` + `set_tokens()`/`set_no_context()` to carry prompt continuity across windows. DeepWiki confirms the `whisper-stream` example re-transcribes every ~0.5 s. Latency floor is therefore encoder-bound, not chunk-bound, and re-encoding overlapping audio wastes compute — the exact thing cache-aware streaming avoids.

**Terminology seeding: good, three ways.** `set_initial_prompt(&str)` (plus whisper.cpp's `carry_initial_prompt` to prepend it to *every* window), `set_tokens(&[c_int])` for explicit prompt-token injection, and — notably — **GBNF grammar-constrained decoding** via `set_grammar(&[WhisperGrammarElement])` / `set_start_rule()` / `set_grammar_penalty(f32)`. Grammar constraints are a strictly stronger terminology mechanism than hotword score boosting for closed vocabularies (product names, ticket IDs, attendee names).

**Diarization: weak.** `set_tdrz_enable(true)` gives whisper.cpp's experimental *tinydiarize* `[SPEAKER_TURN]` markers — turn boundaries only, no speaker identity, requires a tdrz-specific model. Not a substitute for real diarization.

**Timestamps: good.** `set_token_timestamps(true)` plus DTW-based token timestamps (`DtwMode::{None, TopMost{n_top}, Custom{aheads}}`, `DtwModelPreset`), and `set_max_len(1)` + `set_split_on_word(true)` for word-level segmentation. `whisper_vad.rs` wraps whisper.cpp's built-in Silero VAD (`set_vad_model_path`, `set_vad_params`).

**Distribution.** `whisper-rs-sys` vendors the whole whisper.cpp + ggml source (1.76 MB `.crate`) and builds it with the `cmake` crate. That means **CMake + a C++ toolchain on every build machine**, plus the CUDA toolkit or Vulkan SDK per GPU feature. Compile time is the dominant cost. Statically linked output → no extra runtime DLLs beyond the driver-provided `vulkan-1.dll` / `cudart`. This is why AudioGraph's `ADR-0007` feature-gates local ML at all.

**Maintenance risk:** one maintainer, 5 months since last commit, one whisper.cpp minor behind, hosted on Codeberg (so no GitHub-native security advisory / Dependabot flow).

---

## 3. ort (ONNX Runtime bindings) — 2.0.0-rc.13

| Fact | Value |
|---|---|
| Latest | **2.0.0-rc.13** (2026-07-28), wrapping **ONNX Runtime 1.28.0** |
| Prior | rc.12 (2026-03-05), rc.11 (2026-01-07), rc.10 (2025-06-01) |
| Stable | **none** — still RC after ~4 years of rc.x. rc.13's default API level is `api-27`; `api-17` … `api-28` all selectable |
| Pulse | pushed 2026-08-19; 2,472 stars; **4 open issues** (unusually well-tended); commits 2026-08-18/19 |
| License | MIT OR Apache-2.0 |
| Recent downloads | 6.2M — by far the most-used ML runtime binding in Rust |

**Execution-provider Cargo features (crates.io index, exact, rc.13):**

```
acl, armnn, azure, cann, coreml, cuda, directml, migraphx, nnapi, nvrtx, onednn,
openvino, qnn, rknpu, rocm, tensorrt, tvm, vitis, vsinpu, webgpu, xnnpack
```
plus linking/dist controls: `download-binaries`, `copy-dylibs`, `load-dynamic`, `preload-dylibs`, `alternative-backend`, `pkg-config`, `lax-feature-matching`, `api-17..api-28`, `fetch-models`, `half`, `ndarray`, `num-complex`, `tls-{native,native-vendored,rustls,rustls-no-provider}`.
Default = `std, ndarray, tracing, download-binaries, tls-native, copy-dylibs, api-27`.

`nvrtx` = the **NVIDIA TensorRT-RTX** EP (consumer-RTX-targeted TensorRT), configurable with `device_id`, `cuda_graph`, `max_workspace`, `runtime_cache_path`.

### What actually ships as a prebuilt binary

This is the table nobody publishes; it comes from `ort-sys-2.0.0-rc.13/build/download/dist.tsv`, with sizes measured by `HEAD` against `cdn.pyke.io`:

| Target | Feature set in the prebuilt | Archive size |
|---|---|---|
| `x86_64-pc-windows-msvc` | `directml` | 29.6 MiB |
| `x86_64-pc-windows-msvc` | `webgpu` | 52.8 MiB |
| `x86_64-pc-windows-msvc` | `nvrtx, directml` | 31.1 MiB |
| `x86_64-pc-windows-msvc` | `cuda13, tensorrt, nvrtx, directml` | **59.2 MiB** |
| `aarch64-pc-windows-msvc` | `directml` | 31.1 MiB |
| `x86_64-unknown-linux-gnu` | *(none / CPU)* | 9.6 MiB |
| `x86_64-unknown-linux-gnu` | `webgpu` | 13.1 MiB |
| `x86_64-unknown-linux-gnu` | `nvrtx` | 10.0 MiB |
| `x86_64-unknown-linux-gnu` | `cuda13, tensorrt, nvrtx` | 39.1 MiB |
| `aarch64-unknown-linux-gnu` | *(none / CPU)* | 10.1 MiB |
| `aarch64-apple-darwin` | `coreml` | 8.8 MiB |
| `aarch64-apple-darwin` | `coreml, webgpu` | 11.4 MiB |
| `aarch64-apple-ios` / `-ios-sim` | `coreml` | 9.1 MiB each |
| `aarch64-linux-android` | `nnapi` | 10.0 MiB |

Five hard consequences:

1. **CUDA 13 only.** `resolve.rs` maps the `cuda`/`tensorrt` features to `cuda12` or `cuda13`, but the table has no `cuda12` row, and the code comments say so: *"couldn't determine CUDA version, guessing 13 … we only ship 13 for now."* ort's docs confirm: "binaries for CUDA ≥ 13.2 and targets cuDNN ≥ 9.23." Any prebuilt-CUDA ort build demands the user have CUDA 13.2+ / cuDNN 9.23+ on `PATH`.
2. **There is no plain-CPU Windows row.** A default Windows build resolves (0-intersection tie → `candidates.first()`) to the **DirectML** archive. So the DirectML EP is compiled into the ORT you get on Windows whether you asked for it or not; enabling the `directml` Cargo feature costs **zero extra download** — it just unlocks the Rust-side `ep::DirectML` binding.
3. **ROCm, OpenVINO, oneDNN, MIGraphX, QNN, CANN, ACL, ArmNN, TVM, RKNPU, Vitis, Azure have no prebuilt binaries.** Using them means building ONNX Runtime from source and pointing `ORT_LIB_PATH` at it. For a two-person-scale project, treat those EPs as non-existent.
4. **No Windows-ARM64 CUDA and no Linux-ARM64 GPU.** Windows-on-ARM gets DirectML only.
5. Measured contents of `x86_64-pc-windows-msvc+cuda13,tensorrt,nvrtx,directml` (I extracted it): `onnxruntime.lib` **363.9 MB** (static lib, links down), `onnxruntime_providers_cuda.dll` **62.3 MB**, `DirectML.dll` **18.5 MB**, `onnxruntime_providers_tensorrt.dll` 731 KB, `onnxruntime_providers_nv_tensorrt_rtx.dll` 689 KB, `onnxruntime_providers_shared.dll` 10.7 KB. **No cudart / cuBLAS / cuFFT / cuDNN DLLs.** DirectML costs +18.5 MB and nothing else. CUDA costs +62 MB *and* an external CUDA 13 install.

### EP selection and fallback semantics

The rc.13 API is `ort::ep::*` (renamed from `execution_providers`), registered in priority order:

```rust
let session = Session::builder()?
    .with_execution_providers([
        #[cfg(feature = "tensorrt")] ep::TensorRT::default().build(),
        #[cfg(feature = "cuda")]     ep::CUDA::default().build(),
        #[cfg(feature = "directml")] ep::DirectML::default().build(),
        #[cfg(feature = "coreml")]   ep::CoreML::default().build(),
    ])?
    .commit_from_file("model.onnx")?;
```

Semantics that matter for a shipped desktop app:
- Registration order **is** the fallback order; per-operator fallback walks down the list and lands on CPU.
- **`ort` silently falls back to CPU if every EP fails to register.** For live transcription that is the worst failure mode: no error, just 6× slower. `.error_on_failure()` after `.build()` makes registration failure fatal; `ExecutionProvider::register(&builder)` gives you manual per-EP error handling; `ExecutionProvider::is_available()` tells you whether the *linked* ORT was even compiled with that EP. A capability probe should use `is_available()` + a trial `register()` on a dummy session at startup and surface the chosen EP in the UI.
- Global EPs via `ort::init().with_execution_providers([...]).commit()` must be committed before any session; per-session EPs take precedence but do not replace the global set.
- CUDA and TensorRT EPs load as separate dylibs at runtime; DirectML and WebGPU "do not use this interface, but do require helper dylibs." `copy-dylibs` (default on) symlinks/copies them into `target/`; on Windows without Developer Mode it copies. For a Tauri bundle you must explicitly ship these next to the `.exe` (Tauri `resources` / `externalBin`).
- `load-dynamic` + `ort::init_from(path)` (or `ORT_DYLIB_PATH`) is the escape hatch that lets you **ship the app once and pick the ORT flavour at first run** — i.e. download the CUDA-enabled `onnxruntime.dll` post-install only for users who have an NVIDIA GPU, and default everyone else to the DirectML build. This is the single most useful ort feature for AudioGraph's problem and it is under-appreciated.

### Alternative backends (new in rc.12, live in rc.13)

`ort` can now be pointed at a non-ONNX-Runtime engine that implements the ORT C API. Published and current:
- **`ort-tract` 0.4.1+0.23** (2026-08-18) — over `tract` (`tract-onnx` 0.23.5, 2026-08-19). CPU + WASM. "Great operator support."
- **`ort-candle` 0.4.1+0.11.0** (2026-08-18) — over candle 0.11.0. CPU, WASM; CUDA "not available via `ort-candle` right now"; limited operator support but "most transformer models have good support."
- **`ort-web`** — full ORT in the browser (WebGL/WebGPU).

Wiring: add the backend crate, set `ort` to `default-features = false, features = ["std","ndarray","alternative-backend"]`, call `ort::set_api(ort_tract::api())` before any `ort` use. Docs label these **experimental**. Strategic value for AudioGraph: a *pure-Rust, no-C++, no-dylib* CPU fallback that reuses the exact same ONNX model files and the exact same engine code path — the cheapest possible answer to "the native ORT link broke on this machine."

---

## 4. sherpa-onnx (official k2-fsa Rust crate) — 1.13.5 · and sherpa-rs (ARCHIVED)

### The ecosystem fact that reframes this whole question

**`thewh1teagle/sherpa-rs` is archived.** GitHub reports `archived: true`, last push 2026-03-08, 312 stars, 32 open issues; last crates.io release **0.6.8 on 2025-10-05** (≈10.5 months stale). Its `cuda` / `directml` / `download-binaries` / `tts` features are frozen at whatever sherpa-onnx it pinned. Anyone planning around "sherpa-rs" in 2026 is planning around a dead crate.

The live crate is **k2-fsa's own** `sherpa-onnx` / `sherpa-onnx-sys`:

| Fact | Value |
|---|---|
| Latest | **1.13.5** (2026-08-11); 1.13.4 (2026-07-08), 1.13.3 (2026-06-16), 1.13.2 (2026-05-14) |
| Upstream release | sherpa-onnx **v1.13.6** (2026-08-18) — the crate trails the C++ release by ~1 week |
| Pulse | repo pushed 2026-08-18; 14,339 stars; releases roughly monthly-or-faster |
| License | Apache-2.0 |
| Recent downloads | 236,248 — ~3× sherpa-rs's lifetime total |
| AudioGraph today | `sherpa-onnx = "1.13"` behind `sherpa-streaming` and `diarization-clustering` |

**Feature set is only `{ default = ["static"], static, shared }`.** There are no GPU Cargo features. GPU is selected at **runtime** by a string: every model config carries `provider: Option<String>` defaulting to `"cpu"`. Accepted provider strings in the C++ core: `cpu, xnnpack, trt, cuda, directml, coreml, nnapi, spacemit`.

**The ceiling, precisely.** `sherpa-onnx-sys/build.rs` downloads a prebuilt archive from `github.com/k2-fsa/sherpa-onnx/releases/download/v{version}/...`, and `archive_name()` hard-codes **CPU-only** names:

```
linux-x64-static-lib / linux-aarch64-static-lib / osx-x64-static-lib / osx-arm64-static-lib
win-x64-static-MT-Release-lib
linux-x64-shared-lib / linux-aarch64-shared-cpu-lib / osx-*-shared-lib / win-x64-shared-MT-Release-lib
```

Static mode links a bundled CPU-only `onnxruntime` static lib (the `SHERPA_ONNX_STATIC_LIBS` list literally includes `"onnxruntime"`). So **out of the box the official Rust crate is CPU-only**, and setting `provider = "cuda"` on that build silently does nothing useful. Upstream CMake confirms the defaults: `option(SHERPA_ONNX_ENABLE_GPU ... OFF)` and `option(SHERPA_ONNX_ENABLE_DIRECTML ... OFF)`, both of which force `BUILD_SHARED_LIBS=ON` when enabled.

**The escape hatch is real and documented.** `resolve_lib_dir()` honours `SHERPA_ONNX_LIB_DIR` (a directory of prebuilt libs) and `SHERPA_ONNX_ARCHIVE_DIR` (a local mirror of the *expected-name* archive). With `default-features = false, features = ["shared"]`, `emit_shared_link_directives()` emits only `dylib=sherpa-onnx-c-api` + `dylib=onnxruntime`, and `copy_windows_runtime_dlls()` copies **every `.dll` in that dir** next to the built binary. So: extract a GPU archive, point `SHERPA_ONNX_LIB_DIR` at its `lib/`, set `provider = "cuda"`, done.

**What GPU archives upstream actually publishes** (v1.13.6, 283 assets): `cuda-12.x-cudnn-9.x-onnxruntime1.27.1-win-x64-cuda`, `cuda-13.x-cudnn-9.x-...-win-x64-cuda`, and the matching `linux-x64-gpu` pair, plus older `linux-aarch64-shared-gpu-onnxruntime-{1.11,1.16,1.18}`. **There is no DirectML archive.** DirectML with sherpa-onnx means compiling sherpa-onnx yourself with `SHERPA_ONNX_ENABLE_DIRECTML=1` — a real but non-trivial CI lane.

**Measured Windows distribution cost** (v1.13.6, compressed archive → key uncompressed members):

| Archive | Compressed | Notable contents |
|---|---|---|
| `win-x64-shared-MT-Release-lib` | **7.5 MiB** | CPU only |
| `win-x64-static-MT-Release-lib` | 114.7 MiB | static `.lib`s, link down |
| `cuda-12.x-cudnn-9.x-ort1.27.1-win-x64-cuda` | **358.2 MiB** | `onnxruntime_providers_cuda.dll` **328.5 MB**, `onnxruntime.dll` 15.8 MB, `sherpa-onnx-c-api.dll` 4.6 MB, `onnxruntime_providers_tensorrt.dll` 837 KB |

I enumerated all 55 entries in the CUDA archive: **no `cudart*.dll`, no `cublas*`, no `cudnn*`, no `nvinfer*`.** The archive name states the requirement (`cuda-12.x-cudnn-9.x`) and the app must satisfy it externally. A shipped CUDA sherpa build therefore means a ~330 MB DLL in the bundle *plus* a user-side CUDA/cuDNN install. That is not a consumer-app shape.

**Capability surface (Rust modules in the 1.13.5 crate):** `online_asr`, `offline_asr`, `offline_speaker_diarization`, `speaker_embedding`, `vad`, `kws`, `online_punctuation`, `offline_punctuation`, `online_speech_denoiser`, `offline_speech_denoiser`, `audio_tagging`, `spoken_language_identification`, `tts`, `resampler`, `wave`.

**Streaming ASR API** (`OnlineRecognizer`): `create_stream()`, `create_stream_with_hotwords(&str)`, `decode()`, `decode_multiple_streams(&[&OnlineStream])`, `is_ready()`, `is_endpoint()`, `reset()`, `get_result()`; `OnlineStream::{accept_waveform, input_finished, set_option, get_option, has_option}`. Model families: `transducer`, `paraformer`, `zipformer2_ctc`, `nemo_ctc`, `t_one_ctc`. Endpointing is rule-based (`enable_endpoint`, `rule1_min_trailing_silence`, `rule2_min_trailing_silence`, `rule3_min_utterance_length`). **Whisper is offline-only in sherpa-onnx** (`SherpaOnnxOfflineWhisperModelConfig`; the docs classify it under non-streaming ASR).

**Terminology / hotwords — best-in-class surface, with one sharp constraint.** `OnlineRecognizerConfig` exposes `hotwords_file`, `hotwords_score`, `hotwords_buf: Option<Vec<u8>>` (in-memory, no temp file), per-stream `create_stream_with_hotwords`, `blank_penalty`, `rule_fsts` / `rule_fars` (inverse text normalization), `ctc_fst_decoder_config { graph, max_active }` (HLG decoding graph), and `HomophoneReplacerConfig`. **The constraint: `hotwords_file`/`hotwords_score` require a transducer model with `decoding_method = "modified_beam_search"`** — sherpa-onnx logs an error if you set hotwords with any other decoding method. CTC models get contextual biasing only through an HLG graph, which you must build offline with the k2/OpenFst toolchain. AudioGraph's current `sherpa_streaming.rs` sets `decoding_method = "greedy_search"` and `provider = "cpu"`, i.e. it is on the branch where **hotwords silently do nothing**.

**Nemotron / parakeet-unified streaming** routes through the `transducer` field and is detected from model metadata `streaming_model_type == "nemo_parakeet_unified_streaming"` (impl `OnlineRecognizerTransducerNeMoParakeetUnifiedImpl`). Third-party field report (OpenWhispr, 2026-07-18): **Nemotron models require sherpa-onnx ≥ 1.13.4 or they decode badly without failing loudly** — a silent-correctness trap worth a pinned-version assertion. Whether `modified_beam_search` (and therefore hotwords) is supported by that specific unified-streaming impl is **UNVERIFIED**.

**Diarization: offline only.** `OfflineSpeakerDiarization` = pyannote segmentation + speaker embedding + `FastClusteringConfig { num_clusters: -1, threshold: 0.5 }`, `process(&[f32])` over a whole buffer, `sort_by_start_time()`. There is **no streaming diarization API** in sherpa-onnx. That is exactly why AudioGraph carries `parakeet-rs` for Sortformer alongside it.

---

## 5. parakeet-rs — 0.3.7 (the quiet frontrunner, already in the tree)

| Fact | Value |
|---|---|
| Latest | **0.3.7** (2026-07-28); 0.3.6 (2026-06-04), 0.3.5 (2026-04-17) — cadence ~6 weeks |
| Pulse | pushed 2026-07-28; 385 stars; **3 open issues** |
| License | MIT OR Apache-2.0 |
| Recent downloads | 45,874 |
| Depends on | `ort ^2.0.0-rc.13` (already the current rc) |
| AudioGraph today | `parakeet-rs = "0.3"` behind `diarization`, used **only** for Sortformer |

**Features (crates.io index, exact):** `cpu` (default), `cuda`, `tensorrt`, `directml`, `coreml`, `migraphx`, `openvino`, `webgpu`, `nnapi`, `load-dynamic`, `preload-dylibs`, `ort-defaults`, `api-24..api-28` (default `api-28`), `sortformer`, `multitalker`, `cohere`. Each maps 1:1 to the corresponding `ort/<ep>` feature, so **parakeet-rs inherits ort's entire EP matrix including DirectML and `load-dynamic`** — and the README states GPU support "auto-falls back to CPU if fails."

**Model coverage (all ONNX, weights on HF):** Parakeet CTC 0.6b (EN, punct+caps), Parakeet TDT 0.6b-v3 (25 languages, auto-detect), **Parakeet RealTime EOU 120M (streaming, 160 ms chunks, end-of-utterance detection)**, **Nemotron streaming 0.6B EN and Nemotron-3.5 multilingual (cache-aware, 560 ms chunks, `NemotronEncoderCache` carrying LSTM/transformer state; int8 and int4 community exports exist)**, **Multitalker streaming multi-speaker ASR 0.6B (speaker-attributed transcription; `LatencyMode::{Normal 1.12 s, Low 0.56 s, VeryLow 0.16 s, Ultra 0.08 s}`)**, Cohere Transcribe 03-2026 (offline, 14 languages), **Sortformer v2 / v2.1 streaming diarization (≤4 speakers, `diarize_chunk` / `diarize_chunk_raw` / `feed` with state preserved across calls and absolute timestamps)**. Token-level timestamps for CTC and TDT. CTC/TDT have a ~4–5 minute audio ceiling; streaming models don't.

**This is the only Rust crate on the list that does true cache-aware streaming ASR *and* streaming diarization *and* speaker-attributed streaming ASR, on an EP matrix that includes DirectML.** Its two real gaps: (a) **no hotwords / contextual biasing** anywhere in the API — confirmed by DeepWiki against the source; (b) it is a **single-maintainer 385-star crate** whose bus factor is 1, versus sherpa-onnx's 14k-star institutional project. Weights are partly mirrored on the maintainer's personal HF namespace (`altunenes/parakeet-rs`) and partly on `nvidia/*`, `onnx-community/*`, `istupakov/*`, `lokkju/*`, `smcleod/*` — a supply-chain surface worth pinning by revision + hash.

Independent corroboration of the streaming numbers, from a shipped product (OpenWhispr 1.7.6, 2026-07-18, using the same Nemotron models through sherpa-onnx rather than parakeet-rs): Nemotron EN streaming averages **6.93 % WER at 1.12 s chunks** on Open ASR Leaderboard sets vs **5.91 %** for NVIDIA's best English batch model; INT8 ONNX weights are **632 MB (EN) / 650 MB (multilingual)**; partials land "well under a second" behind the voice; runs real-time on a modern laptop **CPU** with no GPU, no Python, no CUDA. Language coverage is the trade: 15 transcription-ready languages vs Whisper's 99.

---

## 6. kyutai — `stt-rs`, `moshi` crate, delayed-streams-modeling

The most architecturally correct streaming STT design available in Rust, and the least maintained.

| Fact | Value |
|---|---|
| `moshi` crate | **0.6.4**, published **2025-10-01** — ~11 months stale. Pins `candle ^0.9.1` while candle is at 0.11.0. Features: `cuda`, `metal`, `flash-attn`. Recent downloads 12,261 |
| `kyutai-stt-rs` | **NOT PUBLISHED on crates.io.** Lives as `stt-rs/Cargo.toml` in `kyutai-labs/delayed-streams-modeling`, version `0.1.0`, edition 2024, deps `candle-core 0.9.1`, `candle-nn 0.9.1`, `candle-transformers 0.9.1`, `moshi 0.6.1`, `hf-hub 0.4.3`, `kaudio 0.2.1`, `sentencepiece 0.11.3`. Features: `cuda`, `cudnn`, `metal` — **nothing else** |
| DSM repo pulse | last push **2026-01-26** (~7 months); 3,017 stars; 36 open issues; **no GitHub releases** |
| moshi repo pulse | last push 2026-05-16; 10,925 stars; last release tag `rustymimi-0.2.2` (2024-09-22) |
| License | code Apache-2.0 (Rust backend) / MIT (Python, web client); **model weights CC-BY 4.0** |

**What it does right.** `stt-rs` processes **80 ms frames** with a genuinely streaming, delay-bounded architecture: `kyutai/stt-1b-en_fr` has a **0.5 s** structural delay, `kyutai/stt-2.6b-en` has **2.5 s**. It supports **semantic VAD** (`--vad`: end-of-turn prediction, e.g. P(2-second-horizon) > 0.5 on the 1b model) — a learned turn-taking signal, not an energy threshold, which is exactly what a live knowledge graph wants for utterance boundaries. It supports **word-level timestamps** (`--timestamps`, driven by `AsrMsg::Word` / `AsrMsg::EndWord`). It supports **GGUF-quantized weights** (the loader branches on a `.gguf` extension). `moshi-server` is a real Rust websocket streaming server with batched multi-stream serving — reported at **64 simultaneous connections at 3× real-time on an L40S**.

**Why it does not fit AudioGraph.** Device selection is CUDA → Metal → CPU, inheriting candle's backend matrix exactly: **no Vulkan, no DirectML, no ROCm ⇒ zero GPU acceleration on Windows AMD/Intel, and CPU-only for the median Windows user.** A 1B-parameter autoregressive audio LM decoding 80 ms frames on CPU is a very different compute budget from a 0.6B cache-aware FastConformer encoder — I have **no measured CPU real-time factor** for `stt-rs` and could not find one (**UNVERIFIED**; treat "runs on CPU" as unproven for live meeting transcription). Model adoption is thin: on HF the `-candle` variants (`kyutai/stt-1b-en_fr-candle`, `kyutai/stt-2.6b-en-candle`) have 9 and 3 likes and negligible downloads, while the `-trfs` (transformers) variants carry the traffic (16,387 and 1,057). No hotword/biasing mechanism is documented. Diarization is absent. And the whole thing is an unpublished path-dependency you would have to vendor and then maintain against a candle version two minors ahead of its pin.

**Verdict on kyutai:** the right *reference architecture* to steal (80 ms frames, bounded delay, semantic VAD as a first-class output, word timestamps from the model rather than from DTW), the wrong *dependency* to ship on Windows in 2026. Revisit if kyutai publishes `kyutai-stt-rs` to crates.io on candle ≥ 0.11 — or if someone exports the DSM models to ONNX, at which point they become an `ort` engine like everything else.

---

## 7. Moonshine Voice / moonshine-rs — the dark horse

| Fact | Value |
|---|---|
| Upstream | `moonshine-ai/moonshine`, **10,923 stars**, pushed **2026-08-23 (today)**, latest release **v0.1.3 (2026-08-18)**. C++ core over ONNX Runtime |
| Rust bindings | `moonshine-rs` **0.2.2** + `moonshine-sys` **0.2.2** (both 2026-08-14), by `ghchinoy` — **third-party, not official**. 193 / 272 total downloads. MIT OR Apache-2.0 |
| Native lib bundle | `moonshine-voice-windows-x86_64.tar.gz` **24.5 MiB**; linux-x64 13.8 MiB; macos-arm64 25.9 MiB; wasm 6.2 MiB. Static `.a`/`.lib` + `moonshine-c-api.h` + header-only `moonshine-cpp.h` |
| License | repo is **NOASSERTION** (mixed): code and **English streaming models MIT**; **other-language models under a non-commercial agreement** |
| AudioGraph today | `asr-moonshine` feature exists, off by default, `asr/moonshine.rs` present |

**Why it deserves a serious look.** The C API is purpose-built for exactly AudioGraph's UI problem. `moonshine_create_stream` / `start_stream` / `transcribe_add_audio_to_stream` / `transcribe_stream` returns a `transcript_t` of **lines**, each with text, timestamps, duration, speaker info, an **`is_updated` dirty flag** ("use this as a dirty flag to determine how to update your UI in a minimal way, touching only the elements that have changed") and an **`is_complete` flag** ("once `is_complete` is set to 1 for a line, its text and timing will never change again"). That is a *finalization contract in the ASR API* — the thing AudioGraph's live-transcript layer has had to invent (cf. the 2026-08-23 `fix(transcript): stop double-adding live rows for finalized utterances` commit). Multiple streams share one transcriber's model memory. Batteries included: mic capture, VAD, speaker ID/diarization, transcription, intent recognition, TTS (Kokoro/Piper), G2P — one API across Python/JS-WASM/iOS/Android/macOS/Linux/Windows/Pi.

**Quality claim, from the author (Pete Warden, 2026-02-13):** largest model is **245M params at 6.65 % WER on the OpenASR Leaderboard vs Whisper large-v3's 1.5B params at 7.44 %**. "Everything runs on the CPU with no NPU or GPU dependencies."

**Why it is not the default answer.** (a) The Rust binding is a 9-day-old third-party wrapper with ~200 downloads and a `0.2.x` version — the FFI surface, not the Rust crate, is the thing that's mature. (b) The upstream project is at **v0.1.3** and pre-1.0 by its own versioning, with a header-version-negotiation scheme (`MOONSHINE_HEADER_VERSION`) implying the API is still moving. (c) The **licensing is a genuine landmine for a commercial product**: English streaming models are MIT, everything else is non-commercial. (d) CPU-only by design, so it does not answer "leverage whatever acceleration hardware the user has" — though it also sidesteps the entire distribution problem. (e) The author himself concedes "our diarization has room for improvement."

---

## 8. burn — 0.21.0 stable / 0.22.0-pre.2

| Fact | Value |
|---|---|
| Latest | **0.22.0-pre.2** (2026-08-10); last stable **0.21.0** (2026-05-07) |
| Pulse | pushed 2026-08-21; 15,802 stars; 302 open issues |
| License | MIT OR Apache-2.0 |
| Recent downloads | 379,928 |

**Best backend matrix on this page, for a workload that doesn't exist yet.** CubeCL-JIT backends: `burn-wgpu` (WebGPU/**Vulkan**/Metal/**DirectX 12**; on Windows Vulkan + DX12 + OpenGL, with `Vulkan`/`Metal`/`WebGpu` aliases to lock the API at compile time), `burn-cuda`, `burn-rocm` (HIP), `burn-cpu`. Native: `burn-flex` (recommended pure-Rust CPU; `burn-ndarray` now **deprecated**), `burn-tch` (LibTorch), `burn-candle`. Post-training quantization to **8/4/2-bit**, per-tensor and per-block, dynamic and static; **no QAT**. ONNX import via **`burn-onnx`** (formerly `burn-import`) — codegen to native Rust, "limited set of ONNX operators," opset ≥ 16 recommended.

**Disqualifying gaps for ASR today.** No Whisper, Conformer, Zipformer, or any ASR architecture in burn or burn-models (`burn-dataset` has an `audio` feature for `SpeechCommandsDataset`, i.e. data loading only). No documented streaming/stateful autoregressive inference with KV cache. And the canonical community attempt, `Gadersd/whisper-burn`, has been dead since **2024-05-06** (357 stars, no commits in 2 years+). Adopting burn for AudioGraph means porting an ASR architecture *and* getting the operator coverage through `burn-onnx` *and* validating numerics — a multi-month project whose payoff is "Vulkan/DX12 acceleration," which ggml already gives you for free through whisper-rs.

**Watch item, not a 2026 bet.** If `burn-onnx` operator coverage ever reaches "loads a Zipformer/FastConformer encoder unmodified," burn becomes the only path to *pure-Rust, no-C++, Vulkan/DX12-accelerated* ASR on Windows. Nothing in the current release notes suggests that is close.

---

## 9. mistral.rs — 0.9.2 (pattern reference only)

| Fact | Value |
|---|---|
| GitHub | **v0.9.2** (2026-08-20), v0.9.1 (2026-08-14), v0.9.0 (2026-07-07); pushed today; 7,621 stars; 385 open issues; MIT |
| crates.io | `mistralrs` / `mistralrs-core` **0.8.1** (2026-04-02) — **crates.io trails GitHub by two minor versions** |
| AudioGraph today | `mistralrs = "0.8"` behind `llm-mistralrs` |

**What the pattern actually teaches — and it is partly a cautionary tale.** Backend selection is **compile-time Cargo features** (`cuda`, `cudnn`, `metal`, `mkl`, `accelerate`), not runtime. There is **no Vulkan, ROCm, DirectML, or OpenCL backend**; Windows AMD/Intel GPU users fall back to CPU (or are told to use WSL2 + CUDA). This is the same wall candle hits, because it *is* candle underneath. The distribution answer is to **ship N artifacts**: platform-specific Python wheels (`mistralrs-cuda`, `mistralrs-mkl`) and an `install.ps1` that probes the hardware and builds with matching features. There is also a `mistralrs doctor` command that reports GPU detection and compiled features — a good pattern to copy for a capability-probe UI.

The transferable lessons for AudioGraph: (1) **compile-time backend selection forces a build matrix, and a build matrix forces either N installers or a post-install download** — if you don't want N installers, you need runtime backend selection, which on this stack means `ort` + `load-dynamic`; (2) ship an explicit `doctor`/capability report so users and support can see which backend actually engaged; (3) crates.io being two minors behind the repo is normal for these projects and means git-rev pinning is sometimes unavoidable. Note also that mistral.rs's ASR story is multimodal-LLM-shaped (Voxtral, Phi4MM, MiniCpmO, Gemma4 audio input) routed through `/v1/chat/completions` — **`/v1/audio/transcriptions` is explicitly not exposed** — and audio transcription streaming is **UNVERIFIED**. It is not an ASR engine candidate.

---

## Windows acceleration: the distribution arithmetic

This table is the actual decision driver, and it is why "just enable CUDA" is not an option for a bundled `.exe`.

| Path | Extra bytes in the bundle | End-user prerequisite | Covers |
|---|---|---|---|
| ORT **DirectML** (`ort`/`parakeet-rs`, `directml` feature) | `DirectML.dll` **18.5 MB** (already inside the default Windows ORT download — +0 MB download) | **none** — DX12 on every Win10 1903+ | NVIDIA + AMD + Intel + Qualcomm |
| ggml **Vulkan** (`whisper-rs`, `vulkan` feature) | 0 (static link) | **none** — `vulkan-1.dll` ships with GPU drivers; Vulkan SDK needed only at *build* time | NVIDIA + AMD + Intel |
| ORT **WebGPU** (`ort`, `webgpu`) | archive 52.8 MiB vs 29.6 MiB | none (DX12/DX11 on Win) | all — but ort docs call it "experimental and may produce incorrect results/crashes" |
| ORT **CUDA** (`ort`, `cuda`) | `onnxruntime_providers_cuda.dll` **62.3 MB** | **CUDA ≥ 13.2 + cuDNN ≥ 9.23 installed and on PATH** | NVIDIA only |
| **sherpa-onnx CUDA** | `onnxruntime_providers_cuda.dll` **328.5 MB** + 15.8 MB `onnxruntime.dll` | **CUDA 12.x or 13.x + cuDNN 9.x** (no CUDA libs in the archive) | NVIDIA only |
| ORT **TensorRT / TensorRT-RTX** | 731 KB / 689 KB shims | full TensorRT (or TensorRT-RTX) runtime, not bundled | NVIDIA only |
| ggml **CUDA** (`whisper-rs`, `cuda`) | 0 (static, but huge object) | CUDA runtime (`cudart`); nvcc at build time | NVIDIA only |
| ORT **ROCm / OpenVINO / oneDNN / MIGraphX** | — | **build ONNX Runtime from source yourself** | not viable at this project's scale |
| `ort-tract` / `ort-candle` alt backends | 0 native (pure Rust) | none | CPU (+WASM) only |

Two corollaries. **First:** DirectML is the only GPU path that is simultaneously vendor-neutral, zero-install, and cheap — and on Windows you are already downloading the DirectML-enabled ORT whether you enable the feature or not. **Second:** if you want CUDA for the users who have it *without* punishing everyone else, the mechanism is `ort`'s `load-dynamic` + `ort::init_from(path)` — ship the DirectML build in the installer, and fetch a CUDA `onnxruntime.dll` set post-install only for machines that pass a CUDA probe. This is the one architectural move that turns a compile-time build matrix into a runtime decision.

---

## Comparative table

Legend: ● full / ◐ partial / ○ none. "Streaming" means *cross-chunk state carried inside the model* (cache-aware / delayed-streams), not sliding-window re-decode.

| | candle 0.11.0 | whisper-rs 0.16 | ort 2.0.0-rc.13 | sherpa-onnx 1.13.5 | parakeet-rs 0.3.7 | kyutai stt-rs | moonshine-rs 0.2.2 | burn 0.21/0.22-pre |
|---|---|---|---|---|---|---|---|---|
| Last release | 2026-06-26 | 2026-03-12 | 2026-07-28 | 2026-08-11 | 2026-07-28 | unpublished (repo 2026-01-26) | 2026-08-14 (upstream 2026-08-18) | 2026-05-07 / 2026-08-10 |
| Maintenance pulse | high (bugfix mode) | **low** (1 dev, 5 mo) | high | high | medium-high (1 dev) | **stalled** | upstream high, binding brand-new | high |
| Stars (upstream) | 20.9k | 67 (Codeberg) | 2.5k | 14.3k | 385 | 3.0k / 10.9k | 10.9k | 15.8k |
| True streaming ASR | ○ | ○ (sliding window) | n/a (runtime) | ● zipformer/nemo/paraformer CTC+transducer | ● EOU 160 ms, Nemotron 560 ms, Multitalker 80 ms–1.12 s | ● 80 ms frames, 0.5 s delay | ● stream API w/ `is_updated`/`is_complete` | ○ |
| Streaming diarization | ○ | ◐ tinydiarize turn markers | n/a | ○ (offline only) | ● Sortformer v2/v2.1, ≤4 spk, state carried | ○ | ◐ built-in, author says "room for improvement" | ○ |
| Hotwords / biasing | ○ | ● initial_prompt + tokens + **GBNF grammar** | n/a | ● hotwords file/buf/per-stream, HLG, ITN FSTs, homophones — **transducer + modified_beam_search only** | ○ | ○ | **UNVERIFIED** | ○ |
| Quantization | ● GGUF quantized whisper | ● full GGUF/ggml q-types | ● whatever the ONNX has (int8/int4) | ● int8 ONNX models | ● int8 (+ community int4) | ◐ GGUF path in loader | ● (ONNX) | ● PTQ 8/4/2-bit, no QAT |
| Windows NVIDIA | ● CUDA (user CUDA install) | ● CUDA / **Vulkan** | ● CUDA13/TRT/nvrtx/**DirectML** | ◐ CUDA via BYO 358 MiB archive | ● CUDA/TRT/**DirectML** | ● CUDA | ○ CPU | ● CUDA/Vulkan/DX12 |
| Windows AMD/Intel GPU | **○** | ● **Vulkan** | ● **DirectML** / WebGPU | ○ (DirectML = build from source) | ● **DirectML** | **○** | ○ CPU | ● Vulkan/DX12 |
| macOS | ● Metal/Accelerate | ● Metal + CoreML | ● CoreML (8.8 MiB) | ● CoreML provider string | ● CoreML (unstable) / WebGPU | ● Metal | ● CPU | ● Metal/wgpu |
| Bundle cost (Win) | 0 native | 0 (static) | +18.5 MB DML … +62 MB CUDA | 7.5 MiB CPU … 358 MiB CUDA | = ort | 0 native | 24.5 MiB static lib | 0 native |
| Build toolchain | Rust (+nvcc for CUDA) | **CMake + C++** (+SDKs) | none (prebuilt DL) | none (prebuilt DL) | none (via ort) | Rust (+nvcc) | prebuilt static lib | Rust |
| ASR models in-tree | Whisper only | Whisper only | any ONNX | zipformer/paraformer/nemo/whisper/moonshine/sensevoice/telespeech/… | parakeet/nemotron/multitalker/cohere/sortformer | kyutai stt-1b/2.6b | Moonshine family | **none** |
| License | MIT/Apache-2.0 | **Unlicense** | MIT/Apache-2.0 | Apache-2.0 | MIT/Apache-2.0 | Apache-2.0 code / **CC-BY-4.0 weights** | MIT/Apache (binding); **models MIT (en) / non-commercial (other langs)** | MIT/Apache-2.0 |

---

## What I would bet on

**Ranked, with reasons and rough effort.** "Effort" assumes one competent Rust dev already inside this codebase.

1. **`ort` 2.0.0-rc.13 as the single inference substrate, with an explicit EP ladder and `load-dynamic`.** This is the bet. It is the only crate on the page whose *runtime* covers NVIDIA + AMD + Intel + Apple + Qualcomm on the platforms AudioGraph ships, the only one with a documented fallback contract, and the only one that lets you defer the CUDA-vs-DirectML decision to first run instead of to `cargo build`. It is already a transitive dependency via `parakeet-rs`. Risks to own: still an RC after four years (API renames between rc's are real — `execution_providers` → `ep` happened); silent CPU fallback must be defeated with `.error_on_failure()` + `is_available()` probing; and having **two** ONNX Runtimes in one process (sherpa-onnx's bundled static ORT plus ort's) is precisely the link conflict AudioGraph's `Cargo.toml` already documents between the `diarization` and `diarization-clustering` features. *Effort: 1–2 weeks to build a probe + EP-ladder + engine trait.*

2. **`parakeet-rs` 0.3.7 as the streaming ASR + streaming diarization engine.** It is the only crate that satisfies streaming ASR, streaming speaker attribution, and a DirectML-capable EP matrix simultaneously, at 560 ms (Nemotron) or 160 ms (EOU) chunks with real encoder-state carry-over, on int8 weights that run real-time on a laptop CPU. AudioGraph already depends on it — currently for 1 of its ~8 capabilities. The honest caveats: bus factor 1, no hotwords, and weights spread across personal HF namespaces. Mitigation is boring: pin model revisions + hashes, and keep a second engine behind the trait. *Effort: 2–3 weeks to promote it from diarization-only to the primary ASR engine.*

3. **`sherpa-onnx` 1.13.5 as the terminology/hotword engine and the model-breadth engine.** Nothing else in Rust gives you `hotwords_buf` + per-stream hotwords + HLG graphs + ITN FSTs + homophone replacement, and nothing else gives you 15+ ASR model families, KWS, punctuation, denoise, VAD, language ID and TTS behind one Apache-2.0 C API from a 14k-star institutional project. Keep it, but **fix the configuration**: `decoding_method = "greedy_search"` in `sherpa_streaming.rs` means the hotword surface is inert. Switch to `modified_beam_search` on a transducer model to make terminology seeding real, and pin ≥ 1.13.4 if Nemotron models are ever loaded through it. Do **not** pursue sherpa-CUDA on Windows (358 MiB archive + user CUDA install), and treat sherpa-DirectML as a build-from-source project you probably shouldn't start. *Effort: days to fix the decoding method; weeks if you want a DirectML sherpa build.*

4. **`whisper-rs` 0.16 + Vulkan retained as the accuracy/offline-batch/multilingual escape hatch.** Vulkan is the cheapest real GPU win on Windows (zero bundle cost, zero user install, all three vendors), and Whisper's 99-language coverage plus GBNF grammar constraints are things the Parakeet/Nemotron family does not have. Do not try to make it stream. Use it for post-session re-transcription at higher quality, for languages outside the NVIDIA set, and for `initial_prompt`-driven terminology on batch passes. Accept the maintenance risk consciously: one dev, 5 months quiet, one whisper.cpp minor behind — and budget for the possibility of vendoring or switching to a fork. *Effort: near-zero, it's already wired.*

5. **`ort-tract` 0.4.1 (or `ort-candle`) as a pure-Rust CPU fallback behind the same engine trait.** Cheap insurance with a genuinely high payoff: same ONNX files, same engine code, zero C++ / zero dylibs, so "the native ORT failed to load on this user's machine" stops being an unrecoverable support ticket. Labelled experimental upstream; gate it behind a feature and a smoke test. *Effort: 3–5 days for a spike.*

6. **Moonshine Voice — spike it, don't commit yet.** The streaming C API's `is_updated`/`is_complete` line semantics map onto AudioGraph's live-transcript finalization problem better than anything else here, upstream is extremely active (pushed today), and 245M params at 6.65 % WER beating Whisper large-v3 is a real claim from a credible source. But the Rust binding is 9 days old with 200 downloads, upstream is v0.1.3, and the non-English model licensing is non-commercial. Spike the FFI directly against `moonshine-c-api.h` if the finalization contract is the thing you want; do not adopt `moonshine-rs` as a load-bearing dependency in 2026. *Effort: 1 week spike.*

7. **kyutai `stt-rs` / `moshi` — steal the design, skip the dependency.** 80 ms frames, a bounded 0.5 s structural delay, semantic VAD as a first-class model output, and model-native word timestamps are the right targets for a live-knowledge-graph pipeline; copy those as *requirements* for whatever engine you use. But an unpublished crate on candle 0.9.1 in a repo untouched since January, with no Vulkan/DirectML and therefore no GPU on most Windows machines, is not shippable. Reconsider if kyutai publishes to crates.io on candle ≥ 0.11, or if the DSM models get an ONNX export. *Effort to adopt today: 4–6 weeks and a permanent maintenance tax. Not recommended.*

8. **candle direct — no, for ASR.** No Vulkan/ROCm/DirectML, Whisper-only, 30-second chunks, no streaming loop. It stays in the tree as `mistralrs`'s substrate for LLM work, where CUDA-or-CPU is an acceptable answer. Not an ASR engine bet.

9. **burn — watch, don't build.** Best backend matrix (wgpu → Vulkan/DX12/Metal), real 8/4/2-bit PTQ, and **zero ASR models**; `whisper-burn` has been dead since 2024. Revisit only if `burn-onnx` operator coverage reaches "loads a FastConformer encoder unmodified." *Effort to adopt: months. Not now.*

10. **`sherpa-rs` — dead. Do not plan around it.** Archived 2026-03-08, last release 2025-10-05.

---

## Implications for AudioGraph

**1. The "smallest available subset" strawman is already false in this repo, and the real architecture is one step away.** `src-tauri/Cargo.toml` today carries `whisper-rs 0.16` (`asr-whisper`, with `cuda`/`vulkan`/`metal`), `sherpa-onnx 1.13` (`sherpa-streaming`, `diarization-clustering`), `parakeet-rs 0.3` (`diarization` → Sortformer only), `llama-cpp-2 0.1.139`, `mistralrs 0.8`, plus an `asr-moonshine` placeholder. That is *already* a multi-engine stack — it just isn't organised as one. The gap is not new dependencies; it is (a) an engine trait with a runtime capability probe, and (b) resolving the ORT link conflict that currently forces `diarization` and `diarization-clustering` to be mutually exclusive. `parakeet-rs`'s and `ort`'s `load-dynamic` feature is the mechanism that makes one-ORT-per-process tractable.

**2. Windows-primary means DirectML-first, and DirectML is free.** The single highest-leverage change on this page: on `x86_64-pc-windows-msvc`, ort's dist table has **no CPU-only row**, so the default download is already the DirectML-enabled ORT. Turning on `parakeet-rs`'s `directml` feature costs **+0 MB of download** and unlocks GPU inference for NVIDIA, AMD, Intel and Qualcomm users with **no CUDA toolkit, no driver install, no second installer**. Compare CUDA: +62 MB (ort) or +328 MB (sherpa) of provider DLL *plus* a mandatory user-side CUDA 13.2/cuDNN 9.23 (ort) or CUDA 12.x/cuDNN 9.x (sherpa) install, because neither archive bundles cudart/cuBLAS/cuDNN. Dev happens in WSL2 with an NVIDIA GPU, which will make CUDA look deceptively easy — it is easy *for the dev box only*.

**3. Terminology seeding has exactly two viable mechanisms, and the repo is currently on neither.** `asr/sherpa_streaming.rs` sets `decoding_method = Some("greedy_search")`, and sherpa-onnx only honours `hotwords_file`/`hotwords_score` under `modified_beam_search` on a transducer — so the hotword surface is present and inert. The two real paths: (a) sherpa-onnx transducer + `modified_beam_search` + `hotwords_buf` (in-memory, per-stream via `create_stream_with_hotwords`, so you can seed from meeting attendees and project vocabulary without touching disk), or (b) whisper-rs `set_initial_prompt` / `set_tokens` / GBNF `set_grammar` on batch re-transcription passes. `parakeet-rs` has **no** biasing API at all, which is the one thing that argues against making it the *only* ASR engine. This is a genuine architectural fork: the streaming-quality winner (parakeet-rs/Nemotron) and the terminology winner (sherpa-onnx transducer) are different crates.

**4. Diarization: keep both, for opposite reasons.** `parakeet-rs` Sortformer v2/v2.1 does **streaming** ≤4-speaker diarization with state carried across `diarize_chunk` calls — the only streaming diarizer in Rust, and the right fit for a live graph. sherpa-onnx's `OfflineSpeakerDiarization` (pyannote + embeddings + `FastClustering` with `num_clusters: -1`) is **offline but unbounded in speaker count** — which is exactly what ADR-0017 says it was adopted for. Those are complementary, not redundant: stream Sortformer live, run sherpa clustering at session finalization to reconcile identities beyond four speakers. Also worth noting: `parakeet-rs`'s **Multitalker** model does speaker-attributed streaming ASR in one pass with `LatencyMode` down to 80 ms, which could collapse two pipeline stages into one — worth a spike against the two-stage design.

**5. Model download should move from bespoke HTTP to `hf-hub` 1.0.** `src-tauri/src/models/mod.rs` is 1,398 lines of hand-rolled `reqwest::blocking` downloading against hardcoded URLs with `expected_size` ±1% checks and three ad-hoc layouts (bare file / `.tar.bz2` archive / multi-component directory). **`hf-hub` reached 1.0.0 on 2026-07-10** (7.1M recent downloads, Apache-2.0) and gives revision pinning, content hashing, resumable downloads, and the local cache layout for free. Every model this plan needs — Nemotron int8, Sortformer v2.1, parakeet TDT/CTC, zipformer, Whisper GGML, Moonshine — is on the Hub. That said: several `parakeet-rs` weights live in *personal* namespaces (`altunenes/parakeet-rs`, `lokkju/*`, `smcleod/*`, `istupakov/*`), so pin by revision SHA, not by `main`.

**6. Two silent-correctness traps to write assertions for now.** (a) **ort silently falls back to CPU** when every EP fails to register — for live transcription that manifests as unexplained latency, not an error. Defeat it with `.error_on_failure()` on the intended EP plus a startup probe that records the *actually engaged* EP and surfaces it (mistral.rs's `doctor` command is the pattern). (b) **Nemotron models decode badly, without failing, on sherpa-onnx < 1.13.4** — a shipped-product field report, not a hypothesis. If Nemotron weights ever load through sherpa-onnx, assert the runtime version. Both belong in the same family as this repo's existing "guard between phases or a dead implementer feeds null downstream" discipline: `it runs` and `it's correct` are separate tests.

**7. Build-vs-adopt, honestly.** *Adopt* the runtime (`ort`), the engines (`parakeet-rs`, `sherpa-onnx`, `whisper-rs`), and the download layer (`hf-hub`) — ~4–6 weeks total to get a probed, DirectML-accelerated, streaming, diarizing, hotword-capable pipeline out of parts that are already 70% in the tree. *Build* only the thin layer nothing supplies: the engine trait, the hardware capability probe and EP ladder, the post-install optional-CUDA fetch, and the partial/final reconciliation contract (which Moonshine's `is_updated`/`is_complete` shows is worth having as an explicit type). *Do not build* an inference engine — the two candidates for that (candle-based streaming à la kyutai, or burn + `burn-onnx`) each cost months and land you on a backend matrix that is *worse* on Windows than ort's is today.

**8. One CI/toolchain note.** `whisper-rs-sys` vendors and CMake-builds whisper.cpp + ggml on every build; ort and sherpa-onnx *download prebuilt* archives. Anything that shifts weight from whisper-rs toward ort-based engines shortens the Rust compile chain — directly relevant to this box's "max ONE Rust-compiling lane concurrently" constraint. Conversely, `ort`'s `download-binaries` (default on) means build-time network access and a `cdn.pyke.io` dependency; if that's unacceptable, `ORT_LIB_PATH` + a vendored archive is the offline path, and `sherpa-onnx` has the parallel `SHERPA_ONNX_ARCHIVE_DIR` / `SHERPA_ONNX_LIB_DIR` escape hatches.

---

## UNVERIFIED / open items

- Whether `modified_beam_search` (and therefore hotwords) is supported by sherpa-onnx's `OnlineRecognizerTransducerNeMoParakeetUnifiedImpl`. If not, the Nemotron-quality path and the hotword path cannot be the same engine.
- Whether the sherpa-onnx `win-x64-cuda` archive's 328.5 MB `onnxruntime_providers_cuda.dll` truly requires a user-side CUDA/cuDNN install. Strongly implied (archive name encodes `cuda-12.x-cudnn-9.x`; I enumerated all 55 archive entries and found no `cudart*`/`cublas*`/`cudnn*`), but not empirically link-tested — no Rust build was run per this run's constraints.
- Measured CPU real-time factor for kyutai `stt-rs` (`stt-1b-en_fr`, 80 ms frames). All published numbers are GPU (L40S). Without this, "kyutai on CPU" is unproven for live meeting transcription.
- Whether `moonshine-rs` 0.2.2 exposes the `is_updated` / `is_complete` line flags (the C API has them; the 9-day-old Rust wrapper's coverage was not inspected).
- Whether `parakeet-rs`'s DirectML path has been exercised on real AMD/Intel Windows hardware, or is only feature-wired. The README says GPU EPs "auto-fall back to CPU if fails," which would mask a non-working DirectML path as merely-slow.
- ort's exact linked-binary size delta in a Tauri release bundle (the 363.9 MB `onnxruntime.lib` is a static archive; the linked-in contribution is far smaller but was not measured).
- mistral.rs audio-transcription streaming behaviour (Voxtral through `/v1/chat/completions`).

## Primary sources

- crates.io API + sparse index: `https://crates.io/api/v1/crates/{candle-core,candle-transformers,whisper-rs,whisper-rs-sys,ort,ort-sys,sherpa-onnx,sherpa-onnx-sys,sherpa-rs,parakeet-rs,burn,moshi,mistralrs,moonshine-rs,hf-hub,tract-onnx,ort-tract,ort-candle}`; `https://index.crates.io/...` for authoritative `features`/`features2`.
- Extracted `.crate` tarballs from `static.crates.io`: `sherpa-onnx-sys-1.13.5/build.rs` (archive names, `resolve_lib_dir`, link directives), `sherpa-onnx-1.13.5/src/{lib,online_asr,offline_speaker_diarization}.rs`, `ort-sys-2.0.0-rc.13/build/download/{dist.tsv,resolve.rs}`, `whisper-rs-0.16.0/src/{whisper_params,vulkan,whisper_ctx}.rs`, `whisper-rs-sys-0.15.0/{build.rs,whisper.cpp/CMakeLists.txt}`.
- ort docs: `https://ort.pyke.io/perf/execution-providers`, `https://ort.pyke.io/setup/linking`, `https://ort.pyke.io/backends` (all "Last updated on July 28, 2026" / March 6, 2026).
- GitHub API: repo metadata, releases and commits for `huggingface/candle`, `pykeio/ort`, `k2-fsa/sherpa-onnx`, `thewh1teagle/sherpa-rs` (archived), `tracel-ai/burn`, `kyutai-labs/{delayed-streams-modeling,moshi}`, `EricLBuehler/mistral.rs`, `altunenes/parakeet-rs`, `ggml-org/whisper.cpp`, `moonshine-ai/moonshine`, `Gadersd/whisper-burn`. Codeberg API for `tazz4843/whisper-rs`.
- Measured downloads/HEADs: `cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/*`; `github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.6/*`.
- Raw upstream files: `raw.githubusercontent.com/kyutai-labs/delayed-streams-modeling/main/stt-rs/Cargo.toml`; `raw.githubusercontent.com/k2-fsa/sherpa-onnx/master/CMakeLists.txt`.
- HF Hub API: `huggingface.co/api/models?author=kyutai`, `?search=kyutai/stt`.
- DeepWiki (repo-grounded): huggingface/candle, kyutai-labs/delayed-streams-modeling, pykeio/ort, tracel-ai/burn, k2-fsa/sherpa-onnx (×2), altunenes/parakeet-rs, ggml-org/whisper.cpp, EricLBuehler/mistral.rs.
- `github.com/moonshine-ai/moonshine/blob/main/core/moonshine-c-api.h`; Pete Warden, "Announcing Moonshine Voice," `petewarden.com/2026/02/13/announcing-moonshine-voice/`.
- Field report with measured streaming numbers: "Streaming Speech-to-Text on CPU: Local & Live," `openwhispr.com/blog/local-streaming-speech-to-text`, 2026-07-18.
- AudioGraph repo (read-only): `src-tauri/Cargo.toml`, `src-tauri/src/asr/sherpa_streaming.rs`, `src-tauri/src/models/mod.rs`, `docs/adr/0007`, `docs/adr/0017`.
