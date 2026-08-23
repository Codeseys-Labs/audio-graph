# GPU / NPU acceleration from Rust on end-user machines — what a shipped Windows-first Tauri app can actually use (2026-08-23)

Research angle: acceleration backends × OS × GPU vendor → which Rust inference stack reaches them today, and whether an installer-distributed app can ship them. All crate versions, driver floors, and payload sizes below were re-verified against crates.io / NuGet / GitHub release APIs / vendor docs on 2026-08-23. Nothing here is from training memory unless explicitly marked UNVERIFIED.

---

## Verdict

For a Windows-first, installer-distributed Tauri app the real 2026 landscape is **two viable acceleration families, not one**, and the maintainer's instinct that "just sherpa-onnx" is too small is correct — but for a different reason than expected. Family 1 is **ggml/Vulkan** (`whisper-rs` + `llama-cpp-2`): one build, no vendor SDK on the user's machine, works on NVIDIA + AMD + Intel + Adreno, and lands within ~1–30 % of CUDA on the same GPU for Whisper-class encoders. Family 2 is **ONNX Runtime via the `ort` crate** (`parakeet-rs` today; `sherpa-*` less so), which is the *only* Rust path that reaches DirectML, TensorRT-RTX, OpenVINO, QNN (Snapdragon NPU), MIGraphX, and VitisAI — i.e. the only path to NPUs and to Microsoft's post-DirectML story. Vulkan is the correct default for latency-critical streaming ASR; ORT/DirectML is the correct universal floor and the only NPU on-ramp.

Three findings overturn common assumptions. **(a) `cuda-oxide` is dead** — 0.4.0, published 2021-06-16, last commit 2021-12-19, 37 stars, 21 489 lifetime downloads. `cudarc` (0.19.9, 2026-08-11, 2.87 M downloads/90d) is the only live Rust CUDA binding, and *neither matters for AudioGraph* because no Rust STT stack sits on top of either — CUDA reaches you through ggml's C++ CUDA backend or ORT's CUDA EP, not through Rust bindings. **(b) DirectML is not dead but is in "sustained engineering"**; Microsoft's forward path is **Windows ML**, whose EP catalog is FFI-reachable from Rust (`WinMLEpCatalog*` is a plain C API) and pairs with `ort`'s `Environment::register_ep_library` (`api-22`). **(c) ROCm on consumer Windows is no longer near-zero** — AMD's HIP SDK for Windows 7.2 (docs dated 2026-07-31) officially lists RDNA3 *and* RDNA4 desktop Radeons — but it is still unreachable from `whisper-rs`, whose `hipblas` feature is documented "Only available on linux."

The blocking architectural constraint is not backend availability: it is that **`whisper-rs-sys` statically links exactly one ggml backend per compiled binary** (`cargo:rustc-link-lib=static=ggml-cuda` / `=ggml-vulkan` / `=ggml-metal` / `=ggml-hip`), so today AudioGraph's `cuda` / `vulkan` Cargo features fork the *release artifact*, not a runtime decision. ggml upstream already solved this with `GGML_BACKEND_DL` + `GGML_CPU_ALL_VARIANTS` + `ggml_backend_score()`; `llama-cpp-2` exposes it as a `dynamic-backends` feature; `whisper-rs` does not. Closing that gap is the single highest-leverage piece of work in this whole space.

---

## 1. Rust CUDA bindings: `cudarc` vs `cuda-oxide` — and why the question is a decoy

| | `cudarc` | `cuda-oxide` |
|---|---|---|
| Latest version | **0.19.9**, published 2026-08-11 | **0.4.0**, published **2021-06-16** |
| Repo | `chelsea0x3b/cudarc` (renamed from `coreylowman/cudarc`), 1 211 ★, last push 2026-08-12 | `Protryon/cuda-oxide`, 37 ★, **last push 2021-12-19**, no license file |
| Downloads | 7 359 057 total / 2 871 494 recent | 21 489 total / 8 762 recent |
| Linking modes | `static-linking`, `dynamic-linking`, `dynamic-loading`, **`fallback-dynamic-loading` (in `default`)** | n/a |
| CUDA versions | feature-gated `cuda-11040` … `cuda-13030`, plus `cuda-version-from-build-system` | CUDA ~11 era |

Sources: crates.io API for [`cudarc`](https://crates.io/crates/cudarc) and [`cuda-oxide`](https://crates.io/crates/cuda-oxide); GitHub API for both repos.

**Assessment of `cuda-oxide`:** obscure and abandoned. Five years without a commit, zero CUDA 12/13 support, no license metadata on crates.io, and 8 762 recent downloads (≈0.3 % of `cudarc`'s). It should not appear in any AudioGraph plan except as a rejected option.

**Why `cudarc` also does not matter here.** `cudarc` is a *driver/runtime API* binding — it gives you `cuLaunchKernel`, cuBLAS, cuDNN handles. It is the right dependency if you are writing kernels (this is why `candle` and `mistral.rs` depend on it). It is the wrong dependency for STT, because you would be reimplementing Whisper/Conformer kernels. AudioGraph's CUDA access already flows through **ggml's C++ CUDA backend** (`whisper-rs?/cuda` → `GGML_CUDA=ON`) and could flow through **ORT's CUDA EP** (`ort/cuda`). `cudarc` is nevertheless worth knowing for one reason: its `dynamic-loading` / `fallback-dynamic-loading` design is the correct *pattern* for shipping CUDA — dlopen the driver, degrade if absent — and it is exactly what `whisper-rs-sys` does not do.

Note `cudarc`'s upstream GitHub org was **renamed** (`coreylowman` → `chelsea0x3b`); the old URL 301s. If any AudioGraph doc or lockfile pins a `coreylowman` git URL it will still resolve today via redirect but is a latent breakage.

---

## 2. Windows: DirectML → Windows ML is the real 2025-2026 story

### 2.1 DirectML's actual status (verified wording)

Microsoft Learn's DirectML pages and the ONNX Runtime DirectML EP page now all carry the same banner:

> "DirectML is in sustained engineering. DirectML continues to be supported, but new feature development has moved to Windows ML for Windows-based ONNX Runtime deployments. Windows ML provides the same ONNX Runtime APIs while dynamically selecting the best execution provider based on your hardware."
> — [learn.microsoft.com/windows/ai/directml/dml](https://learn.microsoft.com/en-us/windows/ai/directml/dml), [onnxruntime.ai DirectML EP](https://onnxruntime.ai/docs/execution-providers/DirectML-ExecutionProvider.html)

So: **not deprecated-and-removed, but frozen.** Corroborating signals: ORT issue [#25899](https://github.com/microsoft/onnxruntime/issues/25899) ("Update ONNX Runtime Documentation to Highlight WinML as the Preferred Windows Path… replacing DirectML, which is being deprecated", opened 2025-08-29) and issue [#23783](https://github.com/microsoft/onnxruntime/issues/23783) (2025-02, "Is DML being deprecated?" — DML never got the Adapters API). The DirectML EP is pinned to DirectML 1.15.2 and **ONNX opset 20 max**; DirectML itself last shipped feature level 6_4 (DirectML 1.15.x). `Microsoft.ML.OnnxRuntime.DirectML` on NuGet is still being serviced (1.24.4, 2026-03-17).

Practical read: DirectML remains the **guaranteed-present GPU floor** on Windows (any DirectX 12 GPU, in-box since Windows 10 1903, and it is one of Windows ML's two default "included" EPs alongside CPU). Do not architect *toward* it, but do keep it as the vendor-neutral fallback.

### 2.2 Windows ML — what it actually is, and it is Rust-reachable

Windows ML is Microsoft's supported copy of ONNX Runtime plus a **runtime EP catalog** that downloads vendor EPs on demand. Verified current EP table ([supported-execution-providers](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/supported-execution-providers)):

| Windows ML EP | Vendor | Current MSIX (as of docs) |
|---|---|---|
| `NvTensorRtRtx` | NVIDIA | 2.30.43.0, released 2026 7D |
| `MIGraphX` | AMD (GPU) | 1.8.57.0 / GPU EP 7.2.2606.20, 2026 6D |
| `VitisAI` | AMD (NPU) | 1.8.68.0, 2026 7D |
| `OpenVINO` | Intel (CPU/GPU/NPU) | 1.8.80.0 / OpenVINO 1.4.1, 2026 6D |
| `QNN` | Qualcomm (Snapdragon NPU) | 2.2451.48.0 / QAIRT 2.45.41, 2026 7D |
| `WebGPU` | — | experimental, **not in Windows ML 1.8.x** |

Requirements ([overview](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/overview)): x64 or ARM64; CPU + GPU-via-DirectML on any WASDK-supported Windows; **hardware-optimized EPs (NPU, vendor GPU) require Windows 11 24H2 / build 26100 or later.**

**The Rust path exists and is concrete:**

1. Windows ML exposes a **C API** — `WinMLEpCatalogCreate`, `WinMLEpCatalogEnumProviders`, `WinMLEpEnsureReady`, `WinMLEpGetLibraryPath` (header `WinMLEpCatalog.h`) — explicitly documented as the C/C++ alternative to the WinRT `ExecutionProviderCatalog`. That is bindgen-able from Rust with no WinRT projection.
2. `ort` 2.0.0-rc.12+ exposes **`Environment::register_ep_library(name, path)`** (gated on `api-22` + `std`, returns an `ExecutionProviderLibrary` handle with `.unregister()`), which is the Rust wrapper over ORT's `RegisterExecutionProviderLibrary`. Docs: [docs.rs/ort Environment](https://docs.rs/ort/latest/ort/environment/struct.Environment.html), [ort::ep::ExecutionProviderLibrary](https://docs.rs/ort/latest/ort/ep/struct.ExecutionProviderLibrary.html).
3. ORT's [plugin EP library](https://onnxruntime.ai/docs/execution-providers/plugin-ep-libraries/usage.html) machinery then gives you `GetEpDevices()`, `SessionOptionsAppendExecutionProvider_V2`, and `SessionOptionsSetEpSelectionPolicy(PREFER_NPU / PREFER_GPU)`. `ort` also wraps automatic selection as `SessionBuilder::with_auto_device` / `with_devices` (added in rc.12, requires ORT ≥ 1.22).

So the sequence `WinMLEpEnsureReady(ep)` → `WinMLEpGetLibraryPath` → `ort::Environment::register_ep_library` → `with_auto_device()` gets a Rust app TensorRT-RTX / OpenVINO / QNN / MIGraphX **without shipping a single vendor byte**. I found no existing Rust crate that does this; it is greenfield FFI work (~200–400 lines of bindgen + orchestration).

### 2.3 Version and packaging constraints (the sharp edges)

- **ORT version skew.** Windows ML **1.8.x ships ORT 1.23.x** (`1.8.2197`, 2026-05-12 → ORT `840c8d7` / 1.23.5). Windows ML **2.x** is newer (`Microsoft.WindowsAppSDK.ML` 2.1.74, 2026-07-17). `ort` 2.0.0-rc.13 ships against **ORT 1.28** but has `api-17` … `api-28` multiversioning, so pointing `ort` at Windows ML's `onnxruntime.dll` via `load-dynamic` requires capping the `api-*` feature to the WinML ORT (`api-23` for WinML 1.8.x). Source: [ONNX Runtime versions in Windows ML](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/onnx-versions).
- **Deployment mode.** Current docs say self-contained and framework-dependent both support **unpackaged (non-MSIX)** apps, and that **framework-dependent is not supported for C/C++** — i.e. a Rust/C-API consumer takes self-contained, which is **~41 MB** of Windows ML runtime binaries (`Microsoft.Windows.AI.MachineLearning.dll`, `onnxruntime.dll`, `onnxruntime_providers_shared.dll`, `DirectML.dll`). Source: [Install and deploy Windows ML](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/distributing-your-app). **Tension:** the Windows App SDK 1.8 release notes carry a known issue stating the opposite ("Windows ML requires framework-dependent deployment; self-containment deployment is not supported"). Treat this as **unresolved** — it needs a hands-on test on a Win11 24H2 box before any plan commits.
- **First-run cost.** `EnsureAndRegisterCertifiedAsync()` downloads EPs on first call and MS warns it "can take multiple seconds or even minutes depending on network speed." For an offline-STT feature this must be a background, resumable, user-visible step — never inline on the first transcription.
- **Bring-your-own EP** is the offline/air-gapped alternative, and MS states the cost plainly: "Each EP package adds approximately **80 MB or more** to your app package size… EP binaries must be included in your app package or installer — they are not downloaded at runtime." ([bring-your-own-eps](https://learn.microsoft.com/en-us/windows/ai/new-windows-ml/bring-your-own-eps))

---

## 3. Vulkan compute via ggml — the pragmatic universal path

**Maturity.** ggml's Vulkan backend shipped 2024 and has been the focus of sustained optimization work presented at **Vulkanised 2026** ("Vulkan Machine Learning in ggml/llama.cpp"): flash attention, op fusion, barrier deferral, with the speaker stating CUDA and Vulkan are "very similar" in functionality and that Vulkan is *faster* in some cases. Independent llama.cpp long-context benchmarks on the NVIDIA developer forum put Vulkan at ~85–95 % of CUDA prompt-processing throughput and within a few points on quality benches.

**Whisper-specific numbers (independent, 2026):**

| Source | Finding |
|---|---|
| [starwhisper.ai benchmark, 2026-07-04](https://starwhisper.ai/whisper-benchmark.html) (i9-13980HX + RTX 4090 Laptop, `jfk.wav`) | "Vulkan matches CUDA to within a few percent on the same GPU"; `small` = 0.83 s for 11 s audio (~13× RT); `large-v3` ~2.8× RT; **CPU-only `small` drops to ~0.5× RT**, `large-v3` ~0.1× RT |
| [snailtext.app, verified 2026-07-07](https://snailtext.app/blog/how-whisper-cpp-works) (production app shipping Vulkan on Windows) | "On a discrete RTX card, Vulkan is roughly 70-90 % the speed of CUDA"; RTX 4070 `large-v3` 30 s clip: **Vulkan 1.6 s vs CUDA 1.2 s** |

**Runtime dependency story — this is why Vulkan wins for shipping.** Per deepwiki over `ggml-org/whisper.cpp`: the Vulkan backend "does not explicitly require the Vulkan SDK to be installed on the end user's system… only the Vulkan runtime drivers provided by the GPU vendor are necessary." Every current NVIDIA/AMD/Intel Windows driver installs `vulkan-1.dll` + an ICD. Zero app-side redistributable. Contrast CUDA (§5).

**Build-time cost.** `GGML_VULKAN=ON` needs the Vulkan SDK (headers + `glslc`) on the *builder*. `whisper-rs`'s `BUILDING.md` documents MSVC/clang/CMake and a CUDA-toolkit recipe but **says nothing about Vulkan** — so the Windows CI leg for Vulkan is undocumented territory and should be scheduled as real work, not a flag flip.

**Also on the ggml side, and currently unused by AudioGraph:** ggml supports `GGML_BACKEND_DL` (backends as loadable `.dll` modules, discovered via executable dir / CWD / `GGML_BACKEND_DIR`, each exporting `ggml_backend_score()`; loader picks highest score) and `GGML_CPU_ALL_VARIANTS` (separate `ggml-cpu-sse42/avx/avx2/avx512` modules, requires `GGML_BACKEND_DL`). This is the one-installer/many-backends primitive. See §7.

---

## 4. ROCm on consumer Windows — better than assumed, still unreachable from Rust STT

**The assumption "near-zero" is now wrong at the platform layer.** AMD's official [HIP SDK for Windows 7.2 system requirements](https://rocm.docs.amd.com/projects/install-on-windows/en/latest/reference/system-requirements.html) (page dated **2026-07-31**) lists Runtime **and** HIP SDK support as ✅ for:

- RDNA4: RX 9070 XT / 9070 / 9070 GRE / 9060 XT / 9060 (gfx1200/1201) — also ROCm Debugger ✅
- RDNA3: RX 7900 XTX / 7900 XT / 7800 XT / 7700 XT / 7650 GRE / 7600 XT / 7600 (gfx1100/1101/1102)
- RDNA2 (RX 6000 series, gfx1030–1032): **❌ across the board**

OS support is **Windows 11 x86-64 22H2 only** (no Windows 10). Third-party reporting adds that ROCm 7.2.2 (CES 2026) unified the Windows and Linux release packages; ROCm itself is at 7.14.0 (2026-07-16, per Wikipedia's tracked release).

**But the Rust path is closed.** `whisper-rs` 0.16.0's own README: "`hipblas`: enable ROCm/hipBLAS support. **Only available on linux.**" `llama-cpp-2` 0.1.154 does expose a `rocm` feature (no OS caveat documented), and `ort` exposes `rocm` + `migraphx` features. So for AudioGraph's ASR stack specifically, AMD-on-Windows acceleration must come from **Vulkan** (works on all RDNA generations *and* RDNA2/GCN, unlike ROCm) or from **DirectML / MIGraphX via ORT**. Shipping ROCm would also mean redistributing the HIP runtime, which is a multi-hundred-MB per-gfx-target payload — strictly worse than Vulkan for a consumer installer.

**Verdict: correct to skip ROCm entirely.** Vulkan strictly dominates it for this app: broader hardware, broader OS, zero redistributable.

---

## 5. CUDA when shipping to users who never installed the toolkit

**It works, and the requirement is narrower than folklore suggests — but the payload is brutal.**

- Needed on the user's machine: an NVIDIA **driver** at or above the floor, plus three DLLs you redistribute next to your exe: `cudart64_13.dll`, `cublas64_13.dll`, `cublasLt64_13.dll` (CUDA 12 equivalents for a CUDA 12 build). Confirmed by LM-Kit.NET's shipping docs ("On Windows x64, no [toolkit needed]… you only need an up-to-date NVIDIA driver") and by StarWhisper's product notes ("bundles the necessary CUDA runtime files… Only the NVIDIA GPU driver needs to be present").
- **Driver floor (NVIDIA official):** CUDA **13.x → driver ≥ 580**; CUDA 12.x → ≥ 525 (and < 580 for minor-version compat, newer still OK via backward compat); CUDA 11.x → ≥ 450. Sources: [CUDA Toolkit release notes Table 2](https://docs.nvidia.com/cuda/cuda-toolkit-release-notes/index.html), [Minor Version Compatibility](https://docs.nvidia.com/deploy/cuda-compatibility/minor-version-compatibility.html). CUDA 13.1 notes tighten this to ≥ 590. Latest GeForce as of Aug 2026 is 610.88 WHQL, so 580+ is roughly "driver from Aug 2025 or later" — fine for gamers, not guaranteed on a locked-down work laptop.
- **Payload, verified:** the `nvidia-cublas-cu12` Windows wheel (which is just cuBLAS + cuBLASLt DLLs) is **553.2 MB**. Microsoft's own ORT release zips: `onnxruntime-win-x64-1.29.0.zip` = **79.6 MB**, `onnxruntime-win-x64-gpu_cuda13-1.29.0.zip` = **275.1 MB**, `gpu_cuda12` = **363.0 MB** (GitHub releases API, ORT 1.29.0, 2026-08-12).
- **Ecosystem drift to watch:** `ort` 2.0.0-rc.13 "only ships CUDA 13 binaries, as ONNX Runtime has deprecated CUDA 12." That silently raises the driver floor of any `ort`-CUDA build to 580+.
- `ort` gives you the shipping hook: **`preload-dylibs`** feature + `ort::util::preload_dylib`, documented as "override the path to EP dependency DLLs like `cudart64` if you wish to ship CUDA alongside your application."

**Verdict:** CUDA is worth having as an *opt-in, downloaded-on-demand* accelerator pack for NVIDIA users who want the last 10–30 %, never as the default installer payload. Half a gigabyte of cuBLAS to buy 1.2 s vs 1.6 s on a 30 s clip is a bad default trade for a live-meeting app.

---

## 6. macOS: Metal is settled; CoreML is a real win; MLX is not ready

- **Metal via `whisper-rs`/`llama-cpp-2`** is already how AudioGraph builds on macOS (`features = ["metal"]` in the `cfg(target_os = "macos")` block, with `GGML_METAL_EMBED_LIBRARY=ON` so the shader library is baked in — no runtime asset). Zero redistributable: Metal is the OS. Measured: M2 Air `small` 30 s clip = 1.1 s (snailtext, 2026-07); an independent M2 measurement puts CPU-only `small` at RTF ≈ 0.35 vs Metal 2.4 s for a 30 s clip — **~4.4× speedup**.
- **CoreML / Apple Neural Engine** (`whisper-rs`'s `coreml` feature, links `static=whisper.coreml`) accelerates the *encoder* only — historically ~6× on the encoder for `small` (1030 ms CPU → 174 ms ANE), with the decoder staying on Metal/CPU because ggml's KV-cache decoder beats CoreML. Cost: a **separate per-model `.mlmodelc` artifact** that must be converted and downloaded alongside each ggml model, doubling model-management surface. Community consensus is that CPU+ANE ≈ CPU+GPU for throughput, with ANE's real benefit being leaving the GPU free and lower power.
- **MLX from Rust is not shippable.** `mlx-rs` 0.25.3 was published **2025-12-16**; the repo moved orgs (`oxideai` → `oxiglade`), has 369 ★, **91 open issues**, and last push 2026-04-18. Latest tagged release predates this by 8 months. There is no MLX Whisper Rust binding in the ecosystem. Reject.
- **`candle`** (0.11.0, 2026-06-26) has Metal and CUDA backends and a Whisper example, but you would own the streaming decode loop, quantization, and VAD integration yourself — a from-scratch reimplementation of what whisper.cpp already ships. Reject for STT; keep on the radar only if AudioGraph ever needs custom models.
- **Latency tuning that beats backend choice on macOS:** dropping Whisper's `audio_ctx` from 1500 (30 s) to 750 (15 s) roughly **halves encoder latency**, since the encoder dominates cost and the full 30 s context is wasted on short streaming chunks. This is a free win for any streaming design and applies on every backend.

---

## 7. The core constraint: one binary can only hold one ggml backend today

Verified by reading `whisper-rs-sys/build.rs` on Codeberg (`whisper-rs` 0.16.0 / `whisper-rs-sys` 0.15.0):

```
if cfg!(feature = "cuda")   { config.define("GGML_CUDA",   "ON"); }
if cfg!(feature = "vulkan") { config.define("GGML_VULKAN", "ON"); }
if cfg!(feature = "hipblas"){ config.define("GGML_HIP",    "ON"); }
...
println!("cargo:rustc-link-lib=static=ggml-cuda");     // under feature = "cuda"
println!("cargo:rustc-link-lib=static=ggml-vulkan");   // under feature = "vulkan"
println!("cargo:rustc-link-lib=static=ggml-hip");      // under feature = "hipblas"
println!("cargo:rustc-link-lib=static=ggml-metal");    // under feature = "metal"
```

No `GGML_BACKEND_DL`, no `GGML_CPU_ALL_VARIANTS`, no `dynamic-backends` feature. Consequences for AudioGraph's current `[features] cuda = ["whisper-rs?/cuda", ...]` / `vulkan = [...]` design:

1. Each backend is a **separate release artifact**, so "pick the best backend at runtime" is impossible inside one installer.
2. The `cuda` build hard-links `cublas`/`cublasLt` import libs, so the exe **fails to start** on a machine without those DLLs — no graceful degradation, a loader error before `main()`.
3. The CPU variant is whatever baseline ISA the CI compiler chose; there is no AVX2-vs-AVX512 runtime dispatch.

By contrast `llama-cpp-2` 0.1.154 **does** ship `dynamic-backends` (plus `dynamic-link`, `system-ggml`), i.e. the LLM half of AudioGraph's stack could already do runtime backend selection while the ASR half cannot. That asymmetry is worth calling out in any plan.

Three ways out, in ascending cost:

- **(i) Sidecar processes.** Build `audio-graph-asr-{cpu,vulkan,cuda}.exe` as Tauri external binaries, probe, spawn the best one, talk over stdio/IPC. No upstream patching. Also isolates native crashes from the UI process. Cost: ~1–2 weeks, plus N× CI build legs and installer size.
- **(ii) Fork `whisper-rs-sys` to enable `GGML_BACKEND_DL` + `GGML_CPU_ALL_VARIANTS`.** Single exe + `ggml-vulkan.dll` / `ggml-cuda.dll` / `ggml-cpu-avx2.dll` / `ggml-cpu-avx512.dll` next to it; `ggml_backend_load_all()` + `ggml_backend_score()` does the selection for you. Cost: ~3–5 days of build.rs work + Windows CI validation, but you now own a fork of a crate whose GitHub repo is **archived** and whose canonical home is a Codeberg repo with 67 stars and 31 open issues — real bus-factor risk. Upstreaming it is the mitigation.
- **(iii) Move the GPU-critical path to `ort`.** See §8.

---

## 8. The `ort` / ONNX Runtime family — the only NPU-capable Rust path

`ort` 2.0.0-rc.13 (2026-07-28, wraps ORT 1.28, 2 472 ★, 16.4 M downloads) exposes EP features: `cuda`, `tensorrt`, `nvrtx` (TensorRT-RTX), `directml`, `rocm`, `migraphx`, `openvino`, `qnn`, `vitis`, `webgpu`, `coreml`, `nnapi`, `xnnpack`, `onednn`, `acl`, `armnn`, `cann`, `rknpu`, `vsinpu`, `tvm`, `azure`. Plus the shipping levers: `load-dynamic` (dlopen `onnxruntime.dll` at a runtime-chosen path — "the executable will not just completely fail to start if the binary couldn't be found"), `copy-dylibs`, `preload-dylibs`, `lax-feature-matching`, and `api-17`…`api-28` multiversioning.

Fallback semantics are built in: "`ort` will register all EPs specified, in order. If an EP does not support a certain operator in a graph, it will fall back to the next successfully registered EP, or to the CPU if all else fails," with `.error_on_failure()` when you want to *know*, and `ExecutionProvider::is_available()` to probe a custom-linked build. The documented idiom is literally TensorRT → CUDA → DirectML → CoreML → CPU.

**Which Rust ASR crates ride on this, verified:**

| Crate | Version / date | GPU reach | Notes |
|---|---|---|---|
| **`parakeet-rs`** | **0.3.7**, 2026-07-28 | `cuda`, `tensorrt`, `directml`, `migraphx`, `openvino`, `webgpu`, `coreml`, `nnapi`; `load-dynamic`, `preload-dylibs` | Streaming ASR (ParakeetEOU **160 ms** chunks, Nemotron **560 ms** chunks) **and** Sortformer streaming diarization (≤4 speakers, `diarize_chunk` keeps state). All GPU providers auto-fall-back to CPU on init failure. Already a dep in AudioGraph (`diarization` feature). |
| **`sherpa-onnx`** (official, `csukuangfj`) | **1.13.5**, 2026-08-11 | features are only `default` / `shared` / `static` — **no EP features** | `build.rs` downloads a matching prebuilt `-lib` archive from GitHub releases (or `SHERPA_ONNX_LIB_DIR`). The C API *does* expose `cuda`/`directml`/`trt`/`coreml`/`qnn`, but with no Cargo feature to select a GPU archive this is **effectively CPU-only from Rust**. This is what AudioGraph pins (`sherpa-onnx = "1.13"`). |
| **`sherpa-rs`** (`thewh1teagle`) | **0.6.8**, 2025-10-05 | `cuda`, `directml`, `download-binaries` | Has the GPU features the official crate lacks, but is ~10 months stale and low-traffic (19 731 recent downloads). |

**This reframes the sherpa question.** "Just use sherpa-onnx for everything" is not merely architecturally narrow — as currently pinned it is **CPU-only**, which for a live-meeting app is the difference between comfortable and marginal. Meanwhile `parakeet-rs`, already in the tree, is the one dependency that reaches DirectML *and* CUDA *and* OpenVINO from Rust with graceful degradation, and it does streaming ASR + streaming diarization in the same crate.

**The ORT link conflict is solvable.** `src-tauri/Cargo.toml` documents that `diarization` (parakeet-rs) and `diarization-clustering`/`sherpa-streaming` (sherpa-onnx) are "MUTUALLY EXCLUSIVE (ORT link conflict)". Root cause: both statically link their own ONNX Runtime. `ort`'s **`load-dynamic`** exists precisely to dissolve this — one dlopen'd `onnxruntime.dll` shared by all consumers. `parakeet-rs` already forwards `load-dynamic`; the blocker is that the official `sherpa-onnx` crate statically links its own copy. Worth a spike: `SHERPA_ONNX_LIB_DIR` + `shared` feature pointed at the same ORT that `ort` loads.

**Vendor caveats for ORT EPs.** DirectML EP is capped at **ONNX opset 20** and cannot use memory-pattern optimizations or parallel execution (must call `DisableMemPattern` + `SetSessionExecutionMode`). DirectML performance is competitive but not equal: an ORT-team-adjacent benchmark on an RTX 3070 measured DirectML 7.795 ms vs CUDA 9.052 ms vs TensorRT 5.797 ms on one model (DirectML *beat* CUDA there), while ORT issue [#14387](https://github.com/microsoft/onnxruntime/issues/14387) documents DirectML degrading badly when a cached session sees a changed batch shape — relevant to streaming, where chunk padding must be **fixed-shape**.

---

## 9. wgpu / WebGPU compute for ML — not a 2026 answer

`wgpu` 30.0.1 (2026-08-22, 9.6 M downloads/90d) is healthy as a *graphics* API but structurally behind for ML:

- `Features::EXPERIMENTAL_COOPERATIVE_MATRIX` "currently only supports **8x8 f32** matrices… Most Vulkan implementations (NVIDIA, AMD) primarily support f16 inputs at larger sizes (e.g. 16x16), so **Vulkan support may be limited**." (docs.rs `wgpu::Features`)
- `SHADER_F16` and `subgroups` exist as opt-in features, so portability requires runtime capability branching in WGSL.
- Packed int8 dot products (`dot4U8Packed` / `dot4I8Packed`) are a WGSL *language extension*, browser-polyfilled when absent — worth 1.7–2.9× over f16 when native, unpredictable when not.
- Evidence of what's achievable: `Beledarian/wgpu-llm` reaches 66 tok/s on an RTX 3090 (f16) and 32.8 tok/s on an Adreno X1-85 (int8) — impressive for a hand-written engine, and still nowhere near a production ASR stack. `burn-wgpu` exists; no production Whisper on it.
- ORT's `webgpu` EP is available in `ort` and `parakeet-rs`, but Windows ML documents it as **experimental, requiring experimental NuGet packages, not available in Windows ML 1.8.x.**

**Verdict:** interesting for Adreno/ARM64 Windows where nothing else exists; not a path to build on. Vulkan via ggml gets you the same hardware with 2 years more optimization behind it.

---

## 10. CPU fallback quality — the number that decides your model ceiling

CPU is the only universally-available backend, so its RTF sets the floor of the product.

| Config | Measurement | Source |
|---|---|---|
| i9-13980HX (8 threads), `small` | **~0.5× real-time** (slower than realtime) | starwhisper 2026-07 |
| i9-13980HX (8 threads), `tiny` / `base` | 5.6× / 2.4× RT | starwhisper 2026-07 |
| i9-13980HX, `large-v3` | ~0.1× RT (>2 min for an 11 s clip) | starwhisper 2026-07 |
| i5-1240P CPU-only, `base`, 30 s clip | 4.8 s | snailtext 2026-07 |
| "modern laptop CPU", `small` | ~1.5–2× RT | snailtext 2026-07 |
| RK3588 ARM64 (8 threads) | `tiny` RTF 0.10, `base` 0.16, `small` 0.47, `medium` 1.51 | turingpi 2026 |

**AVX-512 is worth much less than expected.** The canonical whisper.cpp measurement (discussion #589) shows AVX2 → AVX-512 buying essentially nothing on the same math library (oneMKL AVX2 1 m50 s vs AVX-512 1 m50 s for 13 min of audio); the big lever was the **BLAS library** (OpenBLAS AVX2 3 m06 s → oneMKL AVX2 1 m50 s) and **int8 quantization** (1 m25 s → 1 m17 s). Practical read: do not build an AVX-512 CPU variant expecting a win; do quantize (Q5_1 is the de-facto production default — `large-v3` at Q5_1 ≈ 1.2 GB) and do consider a BLAS/oneDNN path if CPU-only becomes a supported tier.

CPU floor conclusion: **`base`/`small` quantized is the CPU-only ceiling for live transcription**, and even `small` is marginal on a mainstream laptop. Anything above that requires a GPU backend. That is the strongest argument for making Vulkan the default rather than an option.

---

## 11. THE MATRIX — [backend × OS × vendor] → Rust stack, shipping feasibility, runtime deps

Shipping-feasibility scale: **A** = ship by default, **B** = ship as opt-in / downloaded pack, **C** = possible but a project of its own, **D** = don't.

| Backend | OS | Vendor / HW | Rust stack that reaches it TODAY | Ship-feasibility | Runtime dependency on user machine |
|---|---|---|---|---|---|
| **ggml Vulkan** | Win 10/11 x64 | NVIDIA, AMD (all gens incl. RDNA2/GCN), Intel Arc + iGPU | `whisper-rs 0.16 /vulkan`; `llama-cpp-2 /vulkan` | **A** | GPU driver's `vulkan-1.dll` + ICD only. **Nothing to redistribute.** |
| **ggml Vulkan** | Win 11 ARM64 | Qualcomm Adreno | same (UNVERIFIED whether whisper-rs builds clean for `aarch64-pc-windows-msvc`) | B | Adreno driver Vulkan ICD |
| **ggml Vulkan** | Linux | all | same | A | Mesa / proprietary ICD |
| **ggml CUDA** | Win/Linux x64 | NVIDIA only | `whisper-rs /cuda`, `llama-cpp-2 /cuda` | **B** (opt-in pack) | Driver **≥580** (CUDA 13) or **≥525** (CUDA 12) + you ship `cudart64_*`, `cublas64_*`, `cublasLt64_*` (**~553 MB** wheel-equivalent) |
| **ggml Metal** | macOS 11+ | Apple Silicon + Intel Macs w/ Metal | `whisper-rs /metal` (already enabled) | **A** | None — OS-provided; shaders embedded via `GGML_METAL_EMBED_LIBRARY` |
| **CoreML / ANE** | macOS | Apple Silicon | `whisper-rs /coreml` | B | None; but needs a per-model `.mlmodelc` companion artifact |
| **ggml HIP/ROCm** | **Linux only** | AMD RDNA3/4 + CDNA | `whisper-rs /hipblas` (README: "Only available on linux"); `llama-cpp-2 /rocm` | **D** | HIP runtime, per-gfx-target, hundreds of MB |
| ROCm platform on Windows | Win 11 22H2+ | RDNA3 + RDNA4 discrete only (RDNA2 ❌) | **no Rust ASR crate** | **D** | HIP SDK 7.2 install |
| **ORT DirectML** | Win 10 1903+ / 11 | **any DX12 GPU** — NVIDIA, AMD, Intel, Qualcomm | `ort /directml`; **`parakeet-rs /directml`**; `sherpa-rs /directml` (stale) | **A** | Ship `onnxruntime.dll` + `DirectML.dll`. ORT-DML NuGet = **12.5 MB**; opset ≤ **20**; must disable mem-pattern + parallel exec |
| **ORT CUDA** | Win/Linux x64 | NVIDIA | `ort /cuda`, `parakeet-rs /cuda` | B | ORT gpu_cuda13 zip **275 MB** (cuda12: **363 MB**); driver ≥580 for `ort` rc.13 |
| **ORT TensorRT-RTX (`nvrtx`)** | Win 11 24H2+ | NVIDIA RTX | `ort /nvrtx`, or via Windows ML EP catalog | **C** | Either an ~80 MB+ BYO EP package, or WinML catalog download |
| **ORT OpenVINO** | Win 11 24H2+ / Linux | Intel CPU / iGPU / Arc / **NPU** | `ort /openvino`, `parakeet-rs /openvino`, or WinML catalog | **C** | OpenVINO runtime (BYO ≈80 MB+) or WinML `OpenVINO` MSIX |
| **ORT QNN** | Win 11 24H2 ARM64 | Snapdragon X **NPU** | `ort /qnn`, or WinML catalog | **C** | QAIRT libs (BYO) or WinML `QNN` MSIX |
| **ORT MIGraphX / VitisAI** | Win 11 24H2+ | AMD GPU / AMD NPU | `ort /migraphx`, `/vitis`; `parakeet-rs /migraphx` | **C** | WinML MSIX or BYO |
| **ORT CoreML** | macOS | Apple | `ort /coreml`, `parakeet-rs /coreml` (crate marks unstable) | B | OS-provided |
| **ORT CPU (MLAS)** | all | all | `ort` default, `sherpa-onnx` (as pinned), `parakeet-rs /cpu` | **A** | `onnxruntime.dll` ≈ ORT win-x64 zip **79.6 MB** (single dll well under that) |
| **ggml CPU** | all | all | `whisper-rs` (no feature) | **A** | none |
| **wgpu / WebGPU compute** | all | all | `burn-wgpu`, hand-written; ORT `webgpu` EP | **D** for ASR | Driver only. Coop-matrix limited to 8×8 f32; no production ASR |
| **`cudarc` direct** | Win/Linux | NVIDIA | `cudarc 0.19.9` (used by `candle`, `mistral.rs`) | n/a for ASR | Driver only if `dynamic-loading`; you'd own the kernels |
| **`cuda-oxide`** | — | — | **abandoned 2021** | **D** | — |
| **`mlx-rs`** | macOS | Apple Silicon | `mlx-rs 0.25.3` (stale, 91 open issues, no Whisper binding) | **D** | — |

### Installer payload budget (all verified 2026-08-23)

| Item | Size | Source |
|---|---|---|
| Vulkan backend addition | **~0 MB** redistributable | driver-provided ICD |
| Metal backend addition | **~0 MB** | OS |
| `onnxruntime.dll` + DirectML (ORT-DML NuGet) | **12.5 MB** nupkg | api.nuget.org content-length |
| Windows ML self-contained runtime | **~41 MB** | MS `distributing-your-app` |
| Windows ML BYO EP, per EP | **≥80 MB** | MS `bring-your-own-eps` |
| ORT win-x64 CPU release zip | **79.6 MB** | GitHub releases, ORT 1.29.0 |
| ORT win-x64 CUDA 13 zip | **275.1 MB** | same |
| ORT win-x64 CUDA 12 zip | **363.0 MB** | same |
| cuBLAS + cuBLASLt for CUDA 12 (Windows) | **553.2 MB** | PyPI `nvidia-cublas-cu12` win_amd64 wheel |
| Whisper `small` Q5_1 ggml | ~250 MB (466 MiB fp16) | HF `ggerganov/whisper.cpp` |
| Whisper `large-v3` Q5_1 ggml | ~1.2 GB (2.9 GiB fp16) | same |

---

## 12. Hardware detection & graceful degradation strategy

Design goal: **one probe, cheap, cached, overridable, and never able to prevent the app from starting.** Rank candidates, try in order, record which one won.

### Probe primitives that exist today (no new heavy deps required on Windows)

| Signal | How, from Rust | Cost | Notes |
|---|---|---|---|
| GPU vendor + device ID + VRAM (Windows) | `IDXGIFactory6::EnumAdapterByGpuPreference` + `GetDesc3` via the **`windows` crate already in `src-tauri/Cargo.toml`** (0.62.2, Windows-target-only) | µs | Gives `VendorId` (0x10DE NVIDIA / 0x1002 AMD / 0x8086 Intel / 0x5143 Qualcomm), `DedicatedVideoMemory`, and whether the adapter is the Basic Render Driver (software) |
| DX12 feature level / DirectML availability | `D3D12CreateDevice` probe on the chosen adapter | ms | If this fails, DirectML is out; go CPU |
| Vulkan present + device props | `vulkan-1.dll` presence + `vkEnumeratePhysicalDevices`. `ash` 0.38.0+1.3.281 (last release **2024-04-01** — stable but unmaintained-looking) or raw `libloading` | ms | Prefer a raw `libloading` probe over adding `ash`: you only need "does an ICD with a discrete/integrated device exist" |
| NVIDIA driver version + VRAM | `nvml-wrapper` 0.12.1 (2026-03-30, 1.45 M recent dl) — dlopens `nvml.dll` | ms | **This is how you gate the CUDA pack**: read driver version, require ≥580 (CUDA 13) / ≥525 (CUDA 12) before offering it |
| ggml's own opinion | `ggml_backend_load_all()` then read `ggml_backend_score()` per registered backend | ms | Only available if you enable `GGML_BACKEND_DL` (see §7-ii). This is upstream's own scoring — prefer it over hand-rolled heuristics once available |
| ORT's own opinion | `ort` `GetEpDevices()` / `SessionBuilder::with_auto_device()` / `SetEpSelectionPolicy(PREFER_GPU\|PREFER_NPU)` | ms | Delegates ranking to ORT; the right default for the ORT family |
| Windows ML EP inventory | `WinMLEpCatalogEnumProviders` + each EP's `ReadyState` (`NotPresent` / `NotReady` / `Ready`) | ms | Gives you *what could be downloaded* vs *what is installed* — surface this as UI, not as a blocking step |
| CPU ISA | `std::arch::is_x86_feature_detected!("avx2")` / `"avx512f"`; core count via `sysinfo` 0.39.6 (already viable, 42.7 M recent dl) | ns | Per §10, AVX-512 barely matters — use core count to size `n_threads` instead |

### Selection ladder (Windows, streaming ASR)

```
0. user override from settings (always wins; persist the chosen tier)
1. NVIDIA + nvml driver >= 580 + CUDA pack already downloaded  -> ggml CUDA
2. any adapter with a working Vulkan ICD and >= ~2 GB VRAM     -> ggml Vulkan     <-- DEFAULT
3. DX12 device present                                          -> ORT DirectML   (parakeet-rs)
4. Win11 24H2 + WinML EP catalog offers a Ready NPU/vendor EP   -> ORT via register_ep_library
5. always                                                       -> ggml CPU, model capped at base/small Q5_1
```

macOS collapses to `Metal (+CoreML encoder if the .mlmodelc is present) -> CPU`. Linux: `CUDA if driver ok -> Vulkan -> CPU`.

### Degradation rules that matter for a live-meeting app

- **Never link-fail.** Rule out compile-time-linked GPU deps for anything that isn't guaranteed present. `ort`'s `load-dynamic` and `cudarc`'s `fallback-dynamic-loading` are the models: dlopen, and on failure log + drop a tier. A `cuda`-featured `whisper-rs` build violates this today (§7-2).
- **Probe once, cache with an invalidation key.** Cache `{selected_backend, adapter_luid, driver_version, app_version}`; re-probe when any changes. Driver updates are the common invalidation event.
- **Fail *down*, mid-session, without dropping audio.** A GPU device-lost (driver TDR, external GPU unplugged, laptop switching to iGPU) must demote to CPU with a smaller model and keep the transcript stream alive — the knowledge graph downstream cares more about continuity than WER. Budget for a warm CPU fallback context.
- **Make the tier visible and overridable.** Surface "Transcribing on: NVIDIA RTX 4070 (Vulkan)" plus a manual override. This is also your best diagnostic channel — misattributed backends are the top source of "it's slow" reports.
- **Fixed-shape chunks.** ORT DirectML degrades sharply when a cached session sees a new batch/shape (ORT #14387), and both DirectML and TensorRT prefer static shapes. Pad streaming chunks to a fixed frame count rather than feeding variable-length windows.
- **Warm-up on tier selection, not on first utterance.** TensorRT compiles engines on first run; DirectML does weight pre-processing at session creation. Do it during "starting session", not when someone starts talking.

---

## 13. Implications for AudioGraph

Grounded in the current tree: `src-tauri/Cargo.toml` pins `whisper-rs 0.16.0`, `llama-cpp-2 0.1.139` (lock resolves 0.1.151), `sherpa-onnx 1.13`, `parakeet-rs 0.3` (lock 0.3.6), `ort` 2.0.0-rc.12 transitively; `[features] cuda`/`vulkan` fan out to `whisper-rs?/…` + `llama-cpp-2?/…`; macOS block enables `metal` for both; `diarization` and `diarization-clustering`/`sherpa-streaming` are documented as mutually exclusive because of an ORT link conflict; `docs/ARCHITECTURE.md` lists Local Whisper as "No (batch)" for streaming with Deepgram as the MVP ASR.

1. **Make Vulkan the default Windows GPU backend, not an opt-in feature.** It is the only backend that is fast (within a few % to 30 % of CUDA on Whisper), vendor-neutral (NVIDIA + AMD all gens + Intel), and **zero-redistributable**. Today it sits behind a non-default `vulkan` feature with no documented Windows build recipe. Effort: ~1–2 days to add the Vulkan SDK to the Windows CI leg (`whisper-rs`'s `BUILDING.md` does not cover it — expect friction) + ~1 day for the probe. This is the highest value-per-day item on the list.
2. **Do not ship CUDA in the default installer.** ~553 MB of cuBLAS to move a 30 s `large-v3` clip from 1.6 s to 1.2 s is the wrong default, and the CUDA-13 driver floor (≥580) will strand users. Ship it as a downloadable "NVIDIA acceleration pack" gated on an `nvml-wrapper` driver-version check, delivered through the model-download machinery that `docs/MODEL_MANAGEMENT_DESIGN.md` already establishes (async `spawn_blocking` downloads, size verification, per-artifact concurrency guard). Effort: ~1 week once the multi-backend loading problem (item 3) is solved.
3. **The `whisper-rs-sys` static-link constraint is the real blocker; pick a resolution deliberately.** One binary per backend is incompatible with "detect and pick." Options and honest costs: **sidecar processes** ~1–2 weeks, no upstream risk, also buys native-crash isolation from the Tauri UI process; **fork `whisper-rs-sys` for `GGML_BACKEND_DL` + `GGML_CPU_ALL_VARIANTS`** ~3–5 days plus CI, gives the clean single-installer answer and free `ggml_backend_score()` ranking, but takes on a fork of an archived-on-GitHub / Codeberg-hosted crate (67 ★, 31 open issues) — mitigate by upstreaming. Note the asymmetry that `llama-cpp-2` already exposes `dynamic-backends`, so the LLM half could do runtime selection today.
4. **Correct the sherpa assumption before it hardens into a plan.** The pinned official `sherpa-onnx 1.13.5` has **no GPU EP Cargo features** — its build script fetches a prebuilt archive and there is no flag to ask for a CUDA/DirectML one. As pinned, the sherpa streaming-ASR and clustering-diarization paths are **CPU-only**. `sherpa-rs` (`thewh1teagle`) has `cuda`/`directml` but is 10 months stale. Either way, "sherpa for everything" buys the *narrowest* hardware story in the tree, which is the opposite of the maintainer's stated intent.
5. **`parakeet-rs` is the underrated asset — promote it.** Already a dependency, and it is the only crate in the tree that simultaneously does streaming ASR (160 ms / 560 ms chunk models), streaming diarization (Sortformer, `diarize_chunk` with retained state, ≤4 speakers), and reaches DirectML + CUDA + OpenVINO + MIGraphX + CoreML with documented automatic CPU fallback. For a live-meeting app whose ARCHITECTURE.md still lists local Whisper as batch-only, a `parakeet-rs` + DirectML path is the shortest route to *streaming* offline ASR with vendor-neutral GPU acceleration and a **12.5 MB** payload. Effort: ~1 week to wire as a first-class ASR provider behind the existing provider-registry abstraction.
6. **Kill the ORT link conflict with `load-dynamic`.** The documented `diarization` ⊥ `diarization-clustering` exclusion exists because two crates each statically link their own ONNX Runtime. `ort`'s `load-dynamic` (one dlopen'd `onnxruntime.dll`, path chosen at runtime) is the designed fix, and `parakeet-rs` already forwards the feature. Spike: point the official `sherpa-onnx` at the same runtime via `SHERPA_ONNX_LIB_DIR` + `shared`. ~2–3 days to prove, and it unlocks composing streaming ASR with unbounded-speaker clustering in one build.
7. **Treat Windows ML as a 2027 lane, scoped now.** It is the only route to NPUs (QNN on Snapdragon X, VitisAI on AMD, OpenVINO on Intel Lunar/Panther Lake) and to TensorRT-RTX — with **zero vendor bytes in your installer**, because Windows downloads the EPs. The Rust path is real (`WinMLEpCatalog*` C API → `ort::Environment::register_ep_library`, `api-22`) but greenfield: no crate does it, it needs Win11 24H2 hardware to test, `api-*` must be capped to WinML's ORT 1.23.x, and there is an **unresolved contradiction** in Microsoft's own docs about whether C/C++ self-contained deployment is supported. Effort: ~2–3 weeks for a working spike. Write the ADR now, build it after items 1–6.
8. **Reject outright, with reasons on the record:** `cuda-oxide` (abandoned 2021, 37 ★), `mlx-rs` (stale, no Whisper binding), `wgpu`/WebGPU compute for ASR (coop-matrix limited to 8×8 f32, no production stack), ROCm on any OS (`whisper-rs`'s `hipblas` is Linux-only, and Vulkan covers strictly more AMD hardware — including the RDNA2 cards AMD's own Windows HIP SDK marks ❌ — with zero redistributable).
9. **Cap the CPU tier honestly in the UI.** Verified CPU RTFs put `small` at ~0.5–2× real-time on mainstream laptops and `large-v3` at ~0.1×. Cap CPU-only users at `base`/`small` Q5_1, tune `n_threads` from core count (not from AVX-512 detection — §10 shows AVX2→AVX-512 buys ~nothing), and take the free ~50 % encoder-latency cut by lowering `audio_ctx` from 1500 to 750 for streaming chunks.
10. **Pin-hygiene items surfaced incidentally:** `whisper-rs`'s GitHub repo is **archived** (canonical home is Codeberg, 0.16.0 published 2026-03-12) — a supply-chain note worth an ADR line since AudioGraph's ASR depends on it. `cudarc`'s GitHub org was renamed `coreylowman` → `chelsea0x3b`. `ort` rc.13 dropped CUDA 12 binaries, so any future bump raises the NVIDIA driver floor to 580+. `ash` (the Vulkan binding) has not released since 2024-04-01 — irrelevant if you probe Vulkan with `libloading` instead.

---

## 14. UNVERIFIED / open questions

- **Exact x64 `DirectML.dll` size for DirectML 1.15.x.** `Microsoft.AI.DirectML` 1.15.4 nupkg is **202.3 MB** (verified via nuget.org content-length) but bundles x64 + x86 + ARM64 + debug variants. A 2021-era file listing showed a single x64 `DirectML.dll` at 13.4 MB; the 1.15.x figure is unverified and likely larger. `Microsoft.ML.OnnxRuntime.DirectML` 1.24.4 at **12.5 MB** is the better proxy for the ORT-side payload.
- **Windows ML self-contained deployment for C/C++.** Current `distributing-your-app` docs say self-contained supports unpackaged apps and that framework-dependent is *not* supported for C/C++; the Windows App SDK 1.8 release notes' known-issues list says Windows ML *requires* framework-dependent and self-contained is unsupported. Unresolved — needs an empirical test.
- **`whisper-rs` on `aarch64-pc-windows-msvc`** (Snapdragon X): no build evidence found either way; `BUILDING.md` covers only x64 MSVC/MSYS2 and Apple Silicon.
- **Whether the official `sherpa-onnx` crate's prebuilt archive can be swapped for a GPU build** via `SHERPA_ONNX_LIB_DIR` while keeping the Rust API. Deepwiki confirms the env-var override exists; that a CUDA/DirectML archive links cleanly against the crate's bindings is inferred, not verified.
- **Live-streaming Whisper quality on ORT DirectML.** All Whisper Vulkan-vs-CUDA numbers found are `whisper.cpp`; DirectML-vs-CUDA numbers found are non-ASR models (ResNet-class, Stable Diffusion UNet). A DirectML-vs-Vulkan Whisper head-to-head on the same Windows GPU is a genuine gap and worth measuring in-house before committing item 5 over item 1.
- **AMD MIGraphX EP maturity for encoder-decoder ASR.** Windows ML lists it as shipping (MSIX 1.8.57.0), and `ort`/`parakeet-rs` expose it, but no ASR benchmarks surfaced.
