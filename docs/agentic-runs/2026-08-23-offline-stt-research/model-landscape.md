# Open ASR model landscape for streaming/low-latency local transcription (as of 2026-08-23)

## Verdict

The 2026 answer to "which local model for live meeting transcription" is **not** a Whisper variant and
**not** sherpa-onnx's zipformer. It is NVIDIA's **cache-aware streaming FastConformer-RNNT family** —
`nemotron-speech-streaming-en-0.6b` (English) and `nemotron-3.5-asr-streaming-0.6b` (multilingual) — run
through a **ggml/GGUF** runtime. Those two models are natively streaming (constant-memory encoder caches,
runtime-selectable 0/80/240/480/1040 ms lookahead), emit cased+punctuated text with **word- and token-level
timestamps**, hit **2.31 % WER on LibriSpeech test-clean in streaming mode** (byte-equal to NVIDIA's own
NeMo reference), fit in **~450–720 MB on disk at Q4_K_M/Q8_0**, and run **7–10× real time on a 2020-era
8-core laptop CPU** — on the same machine and same harness where `whisper-large-v3-turbo` runs at
**0.6–1.1× real time**, i.e. below real time. Two independent MIT-licensed C++/ggml runtimes now ship them
with CPU + CUDA + Vulkan + Metal + HIP backends and a **Rust binding on crates.io**
(`transcribe-cpp` 0.2.1, published 2026-08-20; features `cuda`/`vulkan`/`metal`/`rocm`/`dynamic-backends`),
which means AudioGraph's acceleration story for streaming ASR can reuse the exact build shape it already
uses for `whisper-rs` (ggml + cuda/vulkan cargo features) instead of inventing an ONNX-Runtime EP story.
Three caveats shape the plan: (1) the accuracy leaderboard crown belongs to **batch** audio-LLMs
(IBM Granite Speech 4.1 2B at 5.33 % mean WER, Apache-2.0) that cannot stream at all, so a two-model
architecture — streaming for the live ledger, batch for a post-hoc quality pass — is warranted rather than
avoidable; (2) **no** natively-streaming model reachable from Rust today has a hotword/keyterm biasing hook —
the only local biasing surfaces that exist are sherpa-onnx's Aho-Corasick contextual biasing (transducer +
`modified_beam_search` only) and Whisper's `initial_prompt`, so terminology seeding is net-new work whichever
model wins; (3) Voxtral Mini 4B Realtime (Apache-2.0, natively streaming, 240 ms–2.4 s configurable delay,
13 languages) is the most attractive *license*, but at 4 B params it measures **0.56–1.05× real time on a
mid laptop (CPU or Vulkan)** and emits **no timestamps** in the ggml port — it is a GPU-only quality tier,
not a default.

---

## 1. How to read the 2026 leaderboard (and why rank is not the deciding variable)

The Open ASR Leaderboard has stopped being a Whisper scoreboard. Public-track top-5 (Appen's snapshot,
observed 2026-04-30, still the ordering at the 2026-08-16 CodeSOTA snapshot):

| Rank | Model | Mean WER | License | Streaming? |
|---|---|---|---|---|
| 1 | `ibm-granite/granite-speech-4.1-2b` | 5.33 | Apache-2.0 | No |
| 2 | `CohereLabs/cohere-transcribe-03-2026` | 5.42 | Apache-2.0 | No |
| 3 | `ibm-granite/granite-4.0-1b-speech` | 5.52 | Apache-2.0 | No |
| 4 | `nvidia/canary-qwen-2.5b` | 5.63 | CC-BY-4.0 | No |
| 5 | `ibm-granite/granite-speech-3.3-8b` | 5.74 | Apache-2.0 | No |

Four facts to carry into the plan:

- **The top of the board is all batch, all Conformer-encoder + LLM-decoder.** The leaderboard paper
  (arXiv:2510.06961v4, 86 systems / 12 datasets, 27-Mar-2026 snapshot) states it directly: Conformer+
  transformer/LLM decoders win average WER; CTC and TDT decoders win RTFx. The best TDT model
  (`parakeet-tdt-0.6b-v2`, 6.05 WER) ranks 10th while running at **RTFx 3390 vs 418** for Canary-Qwen.
- **The top is compressed to <1 WER point, so license/streaming/hardware decide.** MarkTechPost's
  23-Jul-2026 roundup makes the same point and also shows the averages are **not comparable across rows**:
  ARK-ASR-3B and MOSS-Transcribe-preview-2B post lower headline numbers computed over **7** datasets
  (no TED-LIUM); recomputing Cohere on the same 7 moves it 5.42 → 5.84 and Granite 4.1 5.33 → 5.65.
- **Public scores overstate generalization on conversational audio.** Appen's private track (Australian/
  Canadian/Indian/American accents, scripted vs conversational) moves `zoom/scribe_v1` from #4 to #1
  (6.24 avg, Δ▲3) and demotes the public leader. For meeting audio, the AMI column matters more than the
  mean: Cohere Transcribe scores **AMI 8.13** against LibriSpeech-clean **1.25**.
- **Batch WER on a streaming model is a category error the board commits anyway.** Voxtral Realtime is
  listed at 7.68 and Kyutai STT 2.6B at 6.40 — both "worse than Whisper" by that number, which tells you
  nothing about their intended use. Treat those two numbers as floors, not rankings.

Sources: <https://huggingface.co/datasets/hf-audio/open-asr-leaderboard> ·
<https://arxiv.org/html/2510.06961v4> · <https://www.appen.com/blog/hugging-face-open-llm-leaderboard> ·
<https://www.marktechpost.com/2026/07/23/best-open-speech-recognition-asr-models-in-2026-wer-languages-latency-and-license-compared/>
· <https://codesota.com/speech-to-text> (updated 2026-08-16).

---

## 2. The natively-streaming tier (the only tier that matters for the live ledger)

### 2a. NVIDIA cache-aware streaming FastConformer-RNNT — the default candidate

Architecture: FastConformer encoder (8× depthwise-separable conv subsampling → 0.08 s/frame) trained with
*limited right context* and per-layer attention/conv caches, plus an RNN-T decoder. Unlike buffered
streaming there is **zero overlapping recompute**: each frame is processed once, caches carry state
(arXiv:2312.17279; NeMo docs). This is the architectural difference from every Whisper-chunking scheme.

| Model | Params | Langs | Lookahead settings | PnC | Timestamps | License |
|---|---|---|---|---|---|---|
| `nvidia/nemotron-speech-streaming-en-0.6b` | 600 M | en | 0 / 80 / 480 / 1040 ms | yes | token + word | NVIDIA Open Model License |
| `nvidia/nemotron-3.5-asr-streaming-0.6b` | 600 M | 32 locales (19 transcription-ready) | 0 / 240 / 480 / 1040 ms | yes | token + word | **OpenMDW-1.1** |
| `nvidia/parakeet_realtime_eou_120m-v1` | 120 M | en | 80–160 ms | **no** | via runtime | NVIDIA Open Model License |
| `nvidia/multitalker-parakeet-streaming-0.6b-v1` | 600 M | en | ~1.12 s chunks | yes | per-speaker text | NVIDIA Open Model License |

Verified accuracy, measured by the transcribe.cpp WER pipeline on the **full** LibriSpeech test-clean
(2620 utts), quant ladder included:

- `nemotron-speech-streaming-en-0.6b`: F32/F16/Q8_0 **2.31 %**, Q6_K 2.29 %, Q5_K_M 2.34 %, Q4_K_M 2.38 %.
  **Streaming** at R=13: F16 2.29 %, Q8_0 2.31 % — i.e. streaming == offline, and both match NVIDIA's
  NeMo cache-aware reference (2.31 %) and NVIDIA's self-reported 2.32 %.
- `nemotron-3.5-asr-streaming-0.6b`: F32 3.04 % / Q8_0 3.06 % / Q4_K_M ~3.1 % on LS-clean; FLEURS en 7.88–8.02
  (NVIDIA self-reports FLEURS en-US 7.91 % and an 8.84 % 19-locale macro-average). Language must be supplied
  per call (`--language en-US`) or `auto` (emits a `<lang-XX>` tag).
- `parakeet_realtime_eou_120m-v1`: parakeet.cpp measures **10.92 % WER on LibriSpeech test-clean** — matching
  NeMo exactly (agreement WER 0.0000), so this is the model, not the port. It is a voice-agent turn-taking
  model (emits an `<EOU>` token, no punctuation/caps) and its quality floor is far above the 0.6 B models.
  Use it for endpointing, not for the transcript. *(UNVERIFIED: whether that 10.92 figure reflects a
  decode-mode mismatch in the harness; NVIDIA publishes no comparable LS-clean number on the card.)*
- `multitalker-parakeet-streaming-0.6b-v1` is the sleeper: **streaming speaker-attributed ASR** via
  speaker-kernel injection driven by Sortformer, emitting `[Speaker N] text` per ~1.12 s chunk. `parakeet-rs`
  exposes it (`multitalker` feature); transcribe.cpp ports only its **single-speaker ASR path** so far.

Latency arithmetic that matters: at R=13 the lookahead is 1040 ms (worst case; average ≈ half), at R=6
480 ms, at R=1/0 80/0 ms. NVIDIA's own card claims ~17× more concurrent streams than buffered
Parakeet-RNNT-1.1B at the 80 ms setting (240 vs 14 on an H100).

Sources: <https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b> ·
<https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b> ·
<https://huggingface.co/nvidia/parakeet_realtime_eou_120m-v1> ·
<https://huggingface.co/nvidia/multitalker-parakeet-streaming-0.6b-v1> ·
<https://docs.nvidia.com/nemo-framework/user-guide/latest/nemotoolkit/asr/models.html> ·
transcribe.cpp per-model docs (linked in §6).

### 2b. Voxtral Mini 4B Realtime 2602 — best license, wrong size for CPU

`mistralai/Voxtral-Mini-4B-Realtime-2602`, **Apache-2.0**, released Feb 2026 (arXiv:2602.11298). Natively
streaming by construction: a ~970 M causal audio encoder (RMSNorm/SwiGLU/RoPE, sliding-window 750) + ~3.4 B
Ministral decoder emitting one text token per **80 ms** slot at 12.5 Hz, with **Ada-RMS-Norm delay
conditioning** so a *single* checkpoint runs at any delay that is a multiple of 80 ms (240 ms – 2.4 s).
At 480 ms it claims parity with offline Whisper. 13 languages. The paper explicitly benchmarks against
"DSM" (Kyutai) and "Nemotron Streaming" and claims it beats both at comparable latency, and notes DSM only
covers en/fr.

Verified numbers from the ggml port: LibriSpeech test-clean **2.07–2.09 % across the entire quant ladder**
(BF16 → Q4_K_M) — quantization is WER-neutral here — but the sizes are 8.87 GB BF16 → **2.83 GB Q4_K_M**,
and the throughput is the problem:

| Machine / backend | Q4_K_M, 11 s clip | Q4_K_M, 35 s clip |
|---|---|---|
| Apple M4 Max, Metal | 1.14 s (**9.7×**) | 3.91 s (9.0×) |
| Apple M4 Max, CPU | 4.69 s (2.3×) | 13.12 s (2.7×) |
| Ryzen 7 PRO 4750U, Vulkan (iGPU) | 10.97 s (**1.00×**) | 33.51 s (1.05×) |
| Ryzen 7 PRO 4750U, CPU | 13.80 s (0.80×) | 41.54 s (0.85×) |

Also: **no timestamps** in the port (grep of the model doc finds no timestamp/word/diarization mention), a
hard decoder-position wall at ~2.9 h of continuous audio, and streaming mode is **auto-language-detect only**.
Streaming transcripts are byte-equal to offline.

Sources: <https://huggingface.co/mistralai/Voxtral-Mini-4B-Realtime-2602> · <https://arxiv.org/html/2602.11298v2>
· <https://github.com/handy-computer/transcribe.cpp/blob/main/docs/models/voxtral-realtime.md>

### 2c. Kyutai STT (Delayed Streams Modeling) — right idea, wrong runtime for Windows

`kyutai/stt-1b-en_fr` (~1 B, en+fr, **0.5 s delay**, built-in **semantic VAD**) and `kyutai/stt-2.6b-en`
(~2.6 B, en, **2.5 s delay**). Weights **CC-BY-4.0**; code MIT (Python) / Apache-2.0 (Rust backend).
Architecture: Mimi audio tokens (12.5 Hz × 32 codebooks) into a decoder-only multistream transformer with the
text stream delayed — so streaming is intrinsic, batching is free (H100: 400 real-time streams; L40S: 64
connections at RTF 3×), and **word-level timestamps fall out** by subtracting the stream offset.
The semantic VAD predicts *content-and-intonation-aware* end-of-turn — genuinely better than Silero for
turn boundaries — but it is **only implemented in the Rust server**, not the other implementations.

Why it does not win here despite being the best conceptual fit:

- Weight footprint from the checkpoints: 1 B = 1.98 GB + 385 MB Mimi ≈ **2.4 GB bf16**; 2.6 B = 5.23 GB +
  Mimi ≈ **5.6 GB bf16**. No GGUF/int8 path; MLX variants exist (`kyutai/stt-2.6b-en-mlx`) for Apple only.
- The Rust path is `moshi` / `moshi-server` **0.6.4, last published 2025-10-01**, candle-based with only
  `cuda` and `metal` features. **No Vulkan, no DirectML, no ROCm** — on the primary Windows target that means
  NVIDIA-only GPU acceleration or CPU, and a 2.6 B autoregressive model at 12.5 Hz on CPU is not a live path.
- Batch leaderboard 6.40 avg WER / AMI ~12 for the 2.6 B (HF card eval results).
- English-only or en+fr; the 1 B (the low-latency one) is the weaker model.

Sources: <https://kyutai.org/stt/> · <https://github.com/kyutai-labs/delayed-streams-modeling/> ·
<https://huggingface.co/kyutai/stt-1b-en_fr> · <https://arxiv.org/html/2509.08753v1> ·
crates.io `moshi` / `moshi-server` 0.6.4.

### 2d. Moonshine Streaming — the honest CPU-fallback candidate, minus timestamps

`moonshine-ai/moonshine-streaming-{tiny,small,medium}` (Feb 2026), **MIT**, English-only. Ergodic encoder
with sliding-window attention over a 50 Hz **time-domain** frontend (no STFT), causal stride-2 convs.
Streaming contract in the ggml port: **240 ms cumulative encoder right-context, 80 ms feed cadence, 20 ms
emit unit** — the lowest-latency non-NVIDIA option.

| Variant | Params | Q8_0 GGUF | LS test-clean WER |
|---|---|---|---|
| `moonshine-streaming-tiny` | 34 M | **48 MB** | 4.52 % |
| `moonshine-streaming-small` | 123 M | 189 MB | 2.54 % |
| `moonshine-streaming-medium` | 245 M | 282 MB | 2.16 % |

Small at Q8_0 on Ryzen 7 4750U: **CPU 9–15× real time**, Vulkan 15–32×; on M4 Max CPU 51–63×.
Blockers: **no timestamps at all** ("does not emit timestamps"), English-only, no language detection,
and a 4096-token decode window ≈ **17 minutes** of speech per session.
The Handy desktop app (Rust/Tauri, same authors as the runtime) ships these as its ultra-fast tier with
`accuracy_score` 0.55/0.65/0.75 against Whisper's tiers.

Sources: <https://github.com/handy-computer/transcribe.cpp/blob/main/docs/models/moonshine-streaming.md> ·
<https://huggingface.co/moonshine-ai/moonshine-streaming-small> · <https://github.com/cjpais/Handy>

### 2e. Streaming zipformer (what AudioGraph ships today) — still fine, but the floor

`sherpa-onnx-streaming-zipformer-en-20M-2023-02-17` (the model wired into `asr/sherpa_streaming.rs`) is a
~20 M-param streaming zipformer transducer trained on LibriSpeech-960 only: **3.88 % WER on test-clean**
per k2's own docs, no punctuation/casing, no multilingual, English audiobooks domain. sherpa-onnx reports
RTF ≈ 0.062 (16× real time) at `num_threads=2` for the sibling `-en-2023-06-26` model. It *does* give
token-level timestamps in its result JSON, and it is the **one local engine with real hotword support**
(see §8). Newer icefall work (arXiv:2506.14434) unifies streaming/non-streaming zipformer and reaches
2.43 %/6.55 % test-clean/other with 256-frame right context, but no such checkpoint is packaged in
sherpa-onnx's release model zoo that I could verify.

Sources: <https://k2-fsa.github.io/sherpa/onnx/pretrained_models/online-transducer/zipformer-transducer-models.html>
· <https://k2-fsa.github.io/sherpa/ncnn/pretrained_models/zipformer-transucer-models.html> ·
<https://arxiv.org/pdf/2506.14434>

---

## 3. The chunked/pseudo-streaming tier — what it actually costs

**Whisper cannot stream.** It is a 30-second-window seq2seq model; every "streaming Whisper" is a policy on
top of repeated offline decodes. The reference implementation (ÚFAL `whisper_streaming`, IJCNLP 2023 demo)
uses **LocalAgreement-2**: emit a prefix only when *n* consecutive updates agree on it. Measured result on
ESIC European-Parliament speech, **on an A40 GPU**: **3.3 s average computationally-aware latency for
English** (4.4 s German, 4.8 s Czech), with WER ~2 % relative above offline. The paper's own limitation
section flags that cheaper hardware was untested. Simul-Whisper (Interspeech 2024) improves this to ~1–2×
the chunk length (0.5–2 s DAL) at 1 s chunks with 0.77 % absolute degradation, using cross-attention
truncation detection — but it is a research codebase, not a Rust-callable runtime.

Two structural costs, independent of hardware: (1) latency is **unpredictable** because emission waits for
prefix agreement, not for a clock; (2) you pay repeated full-window decode compute for every update. That
is precisely the "buffered streaming" waste the cache-aware FastConformer architecture eliminates.

Whisper checkpoints, Q8_0 GGUF size / LS-clean WER, measured by the same harness:
`tiny.en` 44 MB / 5.72 % · `base.en` 81 MB / 4.16 % · `small.en` 257 MB / 3.09 % · `medium` 793 MB / 2.64 % ·
`large-v3` 1.55 GB / 1.82 % · **`large-v3-turbo` 845 MB / 2.01 %**. `distil-whisper/distil-large-v3.5`
(MIT, English-only, ~756 M params derived from its 3.03 GB fp32 checkpoint) scores 7.21 leaderboard mean /
RTFx 202, i.e. *worse* than `parakeet-tdt-0.6b-v2` on both axes.

Same story for the batch-only NVIDIA/audio-LLM models: `parakeet-tdt-0.6b-v3` is explicitly "not a streaming
model" in every runtime, and sherpa-onnx issue #2918 (Dec 2025) documents the community failing to make it
stream — the reporter's pseudo-streaming "worked, but slow… slower along with the increasing of buffer
length", which is the buffered-recompute tax stated plainly. sherpa-onnx's own Android demos for
parakeet-tdt are named `simulated_streaming_asr`.

Sources: <https://aclanthology.org/2023.ijcnlp-demo.3.pdf> · <https://github.com/ufal/whisper_streaming> ·
<https://arxiv.org/html/2406.10052v1> · <https://github.com/k2-fsa/sherpa-onnx/issues/2918> ·
<https://github.com/handy-computer/transcribe.cpp/blob/main/docs/models/whisper.md>

---

## 4. The batch quality tier (for a post-hoc pass, not the live path)

| Model | Params | Langs | Q8_0 / Q4_K_M GGUF | LS-clean WER | Extras | License |
|---|---|---|---|---|---|---|
| `granite-speech-4.1-2b` | ~3 B fused | en,fr,de,es,pt,ja | 2.56 GB | 1.32 % | translation | Apache-2.0 |
| `granite-speech-4.1-2b-plus` | ~3 B | en,fr,de,es,pt | 2.35 GB | 1.50 % | **word timestamps** (`[T:N]` centisecond markers) | Apache-2.0 |
| `granite-speech-4.1-2b-nar` | ~3 B | en,fr,de,es,pt | 2.33 GB | **1.29 %** | non-autoregressive (single forward pass; RTFx ~1820 @H100 bs128) | Apache-2.0 |
| `cohere-transcribe-03-2026` | 2 B | 14 | 2.41 / **1.55 GB** | 1.25 % | long-form; **needs external VAD** | Apache-2.0 |
| `qwen3-asr-0.6b` | 600 M | 30 auto-detect | 811 MB | 2.11 % | 52 langs upstream; forced-alignment timestamps in 11 | Apache-2.0 |
| `qwen3-asr-1.7b` | 1.7 B | 30 auto-detect | 2.08 GB | 1.61 % | as above | Apache-2.0 |
| `MOSS-Transcribe-Diarize` | 0.9 B | en + zh | 987 / 617 MB | 1.93 % | **inline diarization** `[start][Sxx]text[end]` | Apache-2.0 |
| `canary-qwen-2.5b` | 2.5 B | en | (ported) | — (5.63 mean) | LLM mode (summarize/QA over transcript); AMI oversampled ⇒ verbatim-disfluency bias | CC-BY-4.0 |
| `Audio8/ARK-ASR-{0.6B,3B}` | 0.6/3 B | 19 | community GGUF/ONNX/MLX | — | headline mean scored on 7 sets, not 8 | Apache-2.0 |

`MOSS-Transcribe-Diarize` is worth a hard look for AudioGraph's **speaker-attribution repair** pass: one
model produces timestamps + speaker IDs + text in one decode, Apache-2.0, 617 MB at Q4_K_M. Cost: offline
only, and **~85 MB RAM per minute of audio** (≈2.5 GB for 30 min, ≈5 GB/hour). Q4_K_M shows tail failures
(6 empty outputs, 5 en→zh language drifts, 1 timestamp-repetition loop) — prefer Q5_K_M+.

Cohere Transcribe on a Ryzen 7 4750U: CPU 3–4× real time, Vulkan 8× at Q4_K_M — so even the batch
quality tier is affordable for a post-meeting pass on modest hardware.

Sources: <https://github.com/handy-computer/transcribe.cpp/tree/main/docs/models> (per-model docs, each with
a WER-vs-reference validation table) · <https://huggingface.co/ibm-granite/granite-speech-4.1-2b-nar> ·
<https://huggingface.co/CohereLabs/cohere-transcribe-03-2026> · <https://huggingface.co/OpenMOSS-Team/MOSS-Transcribe-Diarize>

---

## 5. What is actually callable from Rust today (this constrains model choice more than WER does)

| Runtime | Crate / version (verified crates.io) | Backends | Models | License |
|---|---|---|---|---|
| **transcribe.cpp** (handy-computer) | `transcribe-cpp` **0.2.1**, 2026-08-20 | features `cuda`, `vulkan`, `metal`, `rocm`, `openmp`, `dynamic-backends`, `shared`; tinyBLAS CPU | **20 families / 60+ variants**: Parakeet ×11, Nemotron streaming ×2, Moonshine + Moonshine-Streaming, Voxtral + Voxtral-Realtime, Whisper ×12, Canary/Canary-Qwen, Granite 4/4.1, Qwen3-ASR, Cohere, SenseVoice, FunASR-Nano, GigaAM, MOSS-Diarize, MedASR, Sortformer, Multitalker | MIT (ggml MIT) |
| **parakeet.cpp** (mudler/LocalAI) | no crate; flat C API `parakeet_capi.h`, "easy to embed from C, C++, Go, or **Rust**" | ggml: CPU, CUDA, Metal, Vulkan, HIP. **Prebuilt Windows x64 cpu/vulkan/cuda binaries** | 10 NVIDIA checkpoints incl. **`parakeet_realtime_eou_120m` (80 ms)** and `nemotron-3.5-asr-streaming-0.6b`; validated **WER 0 vs NeMo** on every published checkpoint | MIT |
| **parakeet-rs** | `parakeet-rs` **0.3.7**, 2026-07-28 (repo already pins `0.3`) | via `ort`: `cuda`, `directml`, `webgpu`, `tensorrt`, `migraphx`, `openvino`, `coreml`, `nnapi`, `load-dynamic` | Parakeet CTC/TDT, **EOU**, **Nemotron streaming (en + 3.5 multilingual, int8/int4)**, Unified, **Multitalker**, **Cohere Transcribe**, Sortformer v2/v2.1 | MIT OR Apache-2.0 |
| **sherpa-onnx** (in-tree today) | `sherpa-onnx` **1.13.5**, 2026-08-11 — features are **only** `default`, `shared`, `static` | **no GPU EP cargo feature exists** | streaming zipformer, offline parakeet-tdt v2/v3, Moonshine, SenseVoice, Whisper, diarization | Apache-2.0 |
| **whisper-rs** (in-tree today) | `whisper-rs` **0.16.0**, 2026-03-12 | `cuda`, `vulkan`, `metal`, `hipblas`, `intel-sycl`, `openblas` | Whisper only | MIT |
| **moshi** (Kyutai) | `moshi` / `moshi-server` **0.6.4**, 2025-10-01 | candle: `cuda`, `metal` only | Kyutai STT/TTS | Apache-2.0 (Rust) |

Two consequences worth stating plainly:

1. **The audit's open question is answered: the `sherpa-onnx` crate at 1.13.5 exposes no GPU feature at all**
   (`default`/`shared`/`static`). Its sibling binding `sherpa-rs` 0.6.8 does have `cuda`/`directml` — but it
   was last published **2025-10-05**. *(UNVERIFIED: whether a GPU-enabled `libsherpa-onnx` can be linked into
   the `sherpa-onnx` crate out-of-band via env vars / prebuilt libs — that's a runtime-angle question.)*
2. **ggml is already AudioGraph's acceleration substrate.** `whisper-rs?/cuda` and `whisper-rs?/vulkan` are
   already wired in `Cargo.toml`; `transcribe-cpp`'s `cuda`/`vulkan`/`metal`/`rocm` features are the same
   flags on the same tensor library. Note ggml has **no DirectML** backend — on Windows, Vulkan is the
   vendor-neutral GPU path (NVIDIA/AMD/Intel) and CUDA the fast path. DirectML only exists on the
   `ort`-based paths (`parakeet-rs`, `sherpa-rs`, `transcribe-rs`). Handy's own `Cargo.toml` documents
   *dropping* `ort-directml` because pyke's prebuilt ONNX Runtime is compiled with a global `/arch:AVX2`
   baseline that crashes at process startup on pre-Haswell CPUs — a concrete Windows-compat landmine for
   any ORT-based plan.

The API shape of `transcribe.h` is a near-exact match for AudioGraph's existing ledger semantics:
`transcribe_stream_begin/feed/finalize`, `transcribe_stream_get_text()` returning **`committed_text`
(append-only) + `tentative_text` (volatile suffix)**, a **monotonic `revision` counter**, `committed_changed`
/`tentative_changed` flags, a selectable commit policy (`ON_FINALIZE` vs agreement-based, with native-commit
families bypassing agreement), `TRANSCRIBE_TIMESTAMPS_{NONE,SEGMENT,WORD,TOKEN}` gated by
`max_timestamp_kind` (→ `TRANSCRIBE_ERR_UNSUPPORTED_TIMESTAMPS`), a diarize toggle with speaker-segment
accessors, and **`transcribe_model_backend()` which reports the actually-bound backend ("cpu"/"metal"/
"vulkan"/"cuda"/"ROCm") — i.e. a first-class CPU-fallback detector** for the `PipelineStatus`/`Degraded`
gap the audit flagged.

Sources: crates.io API for each crate (queried 2026-08-23) ·
<https://github.com/handy-computer/transcribe.cpp/blob/main/README.md> ·
<https://github.com/handy-computer/transcribe.cpp/blob/main/include/transcribe.h> ·
<https://github.com/mudler/parakeet.cpp/blob/master/README.md> · <https://crates.io/crates/parakeet-rs> ·
<https://github.com/cjpais/Handy/blob/main/src-tauri/Cargo.toml>

---

## 6. Measured throughput — same machines, same harness (this is the decision table)

transcribe.cpp publishes per-model benchmarks on two fixed machines. Speedup over real time in parentheses;
Ryzen 7 PRO 4750U is an 8-core/16-thread 2020 mobile part with Vega iGPU — a fair proxy for the *weakest*
Windows user AudioGraph will see.

**AMD Ryzen 7 PRO 4750U (Fedora 43)**

| Model | Quant | CPU (11 s clip) | CPU (35 s clip) | Vulkan (11 s) | Vulkan (35 s) |
|---|---|---|---|---|---|
| `moonshine-streaming-small` | Q8_0 | 735 ms (**15×**) | 4.00 s (9×) | 349 ms (32×) | 2.38 s (15×) |
| `nemotron-3.5-asr-streaming-0.6b` | Q4_K_M | 1.09 s (**10×**) | 4.17 s (8×) | 783 ms (14×) | 2.37 s (15×) |
| `nemotron-speech-streaming-en-0.6b` | Q4_K_M | 1.22 s (**9×**) | 4.76 s (7×) | 813 ms (14×) | 2.98 s (12×) |
| `parakeet-tdt-0.6b-v3` (batch) | Q4_K_M | 1.22 s (9×) | 4.78 s (7×) | 868 ms (13×) | 3.10 s (11×) |
| `cohere-transcribe-03-2026` (batch) | Q4_K_M | 2.90 s (4×) | 10.08 s (4×) | 1.33 s (8×) | 4.25 s (8×) |
| `whisper-large-v3-turbo` | Q4_K_M | 15.74 s (**0.7×**) | 32.22 s (1.1×) | 4.15 s (2.7×) | 8.88 s (4.0×) |
| `Voxtral-Mini-4B-Realtime` | Q4_K_M | 13.80 s (0.80×) | 41.54 s (0.85×) | 10.97 s (1.00×) | 33.51 s (1.05×) |

**Apple M4 Max** — `nemotron-speech-streaming-en-0.6b` Q4_K_M: Metal 73 ms (**151×**), CPU 330 ms (33×);
`parakeet-tdt-0.6b-v3`: Metal 149×, CPU 34×; `whisper-large-v3-turbo`: Metal 38×, **CPU 1.9×**;
`Voxtral-Mini-4B-Realtime`: Metal 9.7×, CPU 2.3×.

Corroborating CPU-only numbers from the independent parakeet.cpp harness (20-core x86, **8 threads**,
LibriSpeech test-clean, RTFx = audio-sec/proc-sec, vs NeMo PyTorch-CPU):
`tdt_ctc-110m` 81.1 → 91.5 (q8_0), peak RSS **563 MB**; `tdt-0.6b-v2` 32.4 → 34.7, RSS 2545 MB f32;
`rt-eou-120m-v1` 70.6 → 76.1, RSS 621 MB; and for `nemotron-3.5` on a Ryzen 9 9950X3D at 8 threads:
NeMo 12.2 vs parakeet.cpp **29.4 (f32) / 30.8 (q8_0)** with agreement WER 0.0000 — and, critically,
**the true cache-aware streaming path measures RTFx 3.80 on CPU**, i.e. ~3.8× real time for a 0.6 B
streaming model on a desktop CPU with many small chunked forward passes. Quant ladder averages there:
f16 = 57 % of f32 size at 1.70× NeMo, q8_0 = 37 % at 1.56×, q4_k = 26 % with ~1 pp agreement-WER cost.
parakeet.cpp also reports the 110 M Parakeet beating `whisper base.en` and being ~12× (GPU) / ~27× (CPU)
faster than `whisper.cpp` turbo at equal accuracy on its demo clip.

Sources: per-model `## Performance` tables under
<https://github.com/handy-computer/transcribe.cpp/tree/main/docs/models> ·
<https://github.com/mudler/parakeet.cpp/blob/master/benchmarks/BENCHMARK.md>

---

## 7. Word-level timestamp quality (needed for the transcript ledger + diarization alignment)

| Family | Native granularity | Notes |
|---|---|---|
| NVIDIA FastConformer transducers (Parakeet CTC/RNNT/TDT, Nemotron streaming) | **word + token**, 0.08 s/frame grid | parakeet.cpp matches NeMo's `transcribe(timestamps=True)` exactly: word offsets to 0.0 s, per-token/per-word confidence within 5e-6, `max_prob` confidence aggregated per word with NeMo's `min`. Best-in-class for our use. |
| Kyutai STT | **word** | Native; recover by subtracting the 0.5 s / 2.5 s stream offset. |
| Whisper | **segment** | Word-level requires DTW over cross-attention or external forced alignment; transcribe.cpp's whisper WER runs are "with segment timestamps enabled". |
| Zipformer transducer (sherpa-onnx) | **token** | Present in the result JSON today (`"timestamps":[0.68, 1.04, …]`). |
| Granite Speech 4.1 2B **-plus** | **word** | Parsed from the model's `[T:N]` centisecond markers; the non-plus and NAR variants have none. |
| MOSS-Transcribe-Diarize | **segment + speaker** | Emergent `[start][Sxx]text[end]` markers parsed into segment rows + speaker turns. |
| Qwen3-ASR | forced-alignment timestamps in 11 of 52 langs (upstream) | *UNVERIFIED whether the ggml port exposes them.* |
| Moonshine Streaming | **none** | Explicit: "does not emit timestamps". |
| Voxtral Realtime | **none in the ggml port** | One token per 80 ms slot means timing is *derivable in principle*; no runtime exposes it. *UNVERIFIED.* |

Also relevant: the streaming C API of **parakeet.cpp does not surface per-word timestamps for partial
results** (the C++ `StreamingSession::drain_words()` has them; the C API only exposes them offline via
`transcribe_path_json`). transcribe.cpp gates timestamp granularity per family via `max_timestamp_kind`, but
whether **word timestamps arrive during streaming feeds** (vs only at finalize) is *UNVERIFIED* and is a
question worth answering before committing, because AudioGraph's `AsrSpanRevisionPayload` wants them on
partials.

---

## 8. Terminology / hotword seeding — the honest state of local biasing

| Path | Biasing mechanism | Status |
|---|---|---|
| sherpa-onnx **transducer** + `modified_beam_search` | Aho-Corasick **contextual biasing** (hotwords file + boost score), inference-time, no retraining | **Works today.** Streaming zipformer: yes. Offline **NeMo transducers incl. TDT/Parakeet**: added by PR #3077, **merged 2026-02-05**, so present in ≥ v1.12.24 and therefore in the 1.13.x the repo pins. `greedy_search` does **not** support it. |
| Whisper `initial_prompt` | free-text prompt bias | Works, weak, drifts (transcribe.cpp advertises `TRANSCRIBE_FEATURE_INITIAL_PROMPT` for **whisper only**). |
| Granite Speech 4.1 2B | keyword-list biasing (upstream feature) | Secondary source only (MarkTechPost/LinkedIn analysis); **not exposed** by the ggml port. |
| Qwen3-ASR | `context` text prompt biasing (upstream) | *UNVERIFIED in any local runtime;* transcribe.cpp's INITIAL_PROMPT bit is whisper-only. |
| NVIDIA cache-aware streaming (Nemotron/EOU) | **none** | parakeet.cpp's only prompt surface is `stream_begin_lang(target_lang)`; deepwiki over the repo confirms "no explicit mention of hotword, general context-biasing, or external language model support beyond this prompt conditioning for language selection". Word boosting exists in **Riva/NIM**, not in the OSS checkpoints' greedy decode. |
| Kyutai STT | none documented | The DSM repo mentions an experimental prompt-sensitive feature; *UNVERIFIED* as a hotword surface. |

**This is the sharpest build-vs-adopt fork in the whole plan.** The best streaming models have no biasing
hook in any Rust-reachable runtime, and the only working local biasing lives in the engine family with the
weakest models. Three options, roughly ordered by cost:
(a) **post-hoc lexical repair** — fuzzy-match the terminology list against the committed transcript in the
ledger (cheap, no model work, fixes spelling not recognition);
(b) **keep a biased side-path** — run sherpa-onnx zipformer/offline-parakeet-TDT with `modified_beam_search`
+ hotwords for a re-decode of utterances containing seeded terms (moderate; two engines live);
(c) **implement shallow-fusion biasing in the transducer decode loop** of transcribe.cpp/parakeet.cpp —
both are MIT, both already have beam machinery (parakeet.cpp ships a TDT beam decoder with N-best and
raw/normalized scores), and sherpa-onnx's `ContextGraph` is a known-good reference implementation to port.
This is the option that actually gets biasing *on the streaming default*, and it is upstreamable rather
than forked. Estimate: a focused C++ change against a decoder that already exists, not a research project.

Sources: <https://k2-fsa.github.io/sherpa/onnx/hotwords/index.html> ·
<https://github.com/k2-fsa/sherpa-onnx/pull/3077> (merged 2026-02-05) ·
<https://github.com/k2-fsa/sherpa-onnx/issues/2753> · `transcribe.h` feature enum ·
deepwiki query over `mudler/parakeet.cpp`.

---

## 9. Licenses for bundling in a commercial-ish desktop app

Model weights are the constraint; all four candidate runtimes are MIT or Apache-2.0.

| License | Models | Practical read for a paid desktop app that **downloads** weights at runtime |
|---|---|---|
| **MIT** | Whisper (all), distil-whisper, **Moonshine + Moonshine-Streaming** | Cleanest. |
| **Apache-2.0** | **Voxtral Mini 4B Realtime**, Cohere Transcribe, Granite Speech 4/4.1 (all), Qwen3-ASR, MOSS-Transcribe-Diarize, ARK-ASR | Clean; patent grant included. |
| **CC-BY-4.0** | Parakeet CTC/RNNT/TDT (0.6b/1.1b, v2/v3), Canary family, Canary-Qwen, **Kyutai STT weights** | Commercial use permitted **with attribution**; a credits/About entry naming model + author + license + link is the compliance step. Note CC-BY is a poor fit for software and NVIDIA's own newer releases moved off it. |
| **NVIDIA Open Model License** | `nemotron-speech-streaming-en-0.6b`, `parakeet_realtime_eou_120m`, `multitalker-parakeet-streaming`, `parakeet-unified-en-0.6b`, **Sortformer v2.1** (already shipped in AudioGraph) | Permits commercial use of the model and derivatives under NVIDIA's terms; **needs an actual read by whoever signs off** — it carries trustworthy-AI/attribution obligations that CC-BY does not. AudioGraph already ships a Sortformer model under it, so the precedent exists. |
| **OpenMDW-1.1** | `nemotron-3.5-asr-streaming-0.6b` (the multilingual streaming default candidate) | Linux-Foundation "Open Model, Data and Weights" license — permissive, weights+artifacts scoped. **New enough that it deserves an explicit legal read** rather than an assumption. |

Nothing in the shortlist is non-commercial or research-only. The two files needing a lawyer's eye are the
**NVIDIA Open Model License** and **OpenMDW-1.1** — and both are avoidable if English-only is acceptable and
you pick `nemotron-speech-streaming-en-0.6b`… except that one is *also* NVIDIA-OML. The fully-permissive
streaming set is: **Moonshine-Streaming (MIT)** and **Voxtral Realtime (Apache-2.0)**.

---

## 10. Sizes matrix (on-disk, quantized, from published artifact tables)

| Model | Params | F16 | Q8_0 | Q4_K_M | Peak RAM signal | Streams? | Timestamps |
|---|---|---|---|---|---|---|---|
| `moonshine-streaming-tiny` | 34 M | 96 MB* | **48 MB** | n/s | tiny | native (240 ms RC) | none |
| `moonshine-streaming-small` | 123 M | 282 MB* | 189 MB | n/s | small | native | none |
| `moonshine-streaming-medium` | 245 M | 509 MB | 282 MB | n/s | small | native | none |
| `parakeet-tdt_ctc-110m` | 110 M | ~250 MB | ~165 MB | ~115 MB | **563 MB RSS f32** | no | word+token |
| `parakeet_realtime_eou_120m` | 120 M | ~275 MB | ~180 MB | ~125 MB | 621 MB RSS f32 | **native 80 ms** | word (offline API) |
| `nemotron-speech-streaming-en-0.6b` | 600 M | 1.16 GB | 696 MB | **453 MB** | ~2.3 GB f32 RSS | **native, 0/80/480/1040 ms** | word+token |
| `nemotron-3.5-asr-streaming-0.6b` | 600 M | 1.19 GB | 716 MB | **473 MB** | ~2.4 GB f32 RSS | **native, 0/240/480/1040 ms** | word+token |
| `parakeet-tdt-0.6b-v3` | 600 M | 1.26 GB | 740 MB | 502 MB | ~2.6 GB f32 RSS | no | token/word/sentence |
| `qwen3-asr-0.6b` | 600 M | — | 811 MB | — | KV-bound (65 k ctx) | no | upstream only |
| `whisper-large-v3-turbo` | 809 M | 1.6 GB | 845 MB | ~600 MB | — | no (chunk) | segment |
| `MOSS-Transcribe-Diarize` | 0.9 B | 1.83 GB | 987 MB | 617 MB | **+85 MB per audio-minute** | no | segment + speaker |
| `cohere-transcribe-03-2026` | 2 B | 4.11 GB | 2.41 GB | 1.55 GB | — | no | — |
| `granite-speech-4.1-2b(-plus)` | ~3 B fused | — | 2.56 / 2.35 GB | — | — | no | word (plus only) |
| `kyutai/stt-1b-en_fr` | 1 B | **2.36 GB bf16** (incl. 385 MB Mimi) | none | none | +KV | **native 0.5 s** + semantic VAD | word |
| `kyutai/stt-2.6b-en` | 2.6 B | **5.62 GB bf16** | none | none | +KV | native 2.5 s | word |
| `Voxtral-Mini-4B-Realtime` | 4 B | 8.88 GB | 4.73 GB | **2.83 GB** | sliding-window | **native 240 ms–2.4 s** | none |
| `diar_streaming_sortformer_4spk-v2.1` | 117 M | 237 MB | **139 MB** | withdrawn | small | native (diarization only) | speaker segments; **DER 14.6 % AMI-IHM** |

\* Estimated cells (all others are published artifact sizes): the Moonshine tiny/small F16 rows, the
`parakeet-tdt_ctc-110m` / `parakeet_realtime_eou_120m` quant rows, and `whisper-large-v3-turbo` Q4_K_M — all
derived by applying parakeet.cpp's measured average quant ratios (f16 = 57 %, q8_0 = 37 %, q4_k = 26 % of f32)
to the published F32 size. "n/s" = K-quants not shipped for that family. Peak-RAM cells marked "RSS" are
parakeet.cpp's measured `/usr/bin/time -v` peak RSS at f32; quantized RSS is lower.

---

## 11. Shortlist

**Default (live path):** `nvidia/nemotron-speech-streaming-en-0.6b` at **Q8_0 (696 MB)** if English-only is
acceptable, else `nvidia/nemotron-3.5-asr-streaming-0.6b` at **Q8_0 (716 MB)** for 19–32 locales at ~0.7 pp
LS-clean cost. Run via **`transcribe-cpp` 0.2.1** (Rust, MIT) with `vulkan` on Windows x64, `cuda` where the
toolkit is present, `metal` on macOS, plain CPU otherwise. Start at lookahead R=13 (1040 ms worst case,
matching offline WER), expose R=6/1 as a "lower latency, slightly worse" setting — the model was trained for
all of them, so this is a runtime knob, not a second download. Word+token timestamps and native PnC come
free; `transcribe_model_backend()` gives the degraded-mode signal the audit asked for.

**Quality tier (post-hoc pass over the finished utterance/meeting):** `ibm-granite/granite-speech-4.1-2b-nar`
(1.29 % LS-clean, Apache-2.0, single forward pass) for pure text quality, or **`cohere-transcribe-03-2026`
Q4_K_M (1.55 GB, 4× real time on a weak CPU, 8× on iGPU Vulkan)** for 14 languages, or
**`MOSS-Transcribe-Diarize` Q5_K_M (700 MB)** if the pass should re-derive speaker attribution and
timestamps together. All three are Apache-2.0 and all three run under the same runtime as the default.

**CPU-fallback tier:** `moonshine-streaming-small` **Q8_0 (189 MB, MIT, 9–15× real time on a 2020 laptop
CPU, 2.54 % LS-clean)** — but only if the ledger can tolerate **no timestamps** and English-only.
If timestamps are non-negotiable at the fallback tier (they probably are, for diarization alignment), the
fallback is instead the **same** Nemotron model at Q4_K_M (453 MB, still 7–10× real time on that machine),
and Moonshine becomes a "smallest possible download" option rather than a fallback.

**Endpointing/turn-taking (not transcript):** `parakeet_realtime_eou_120m-v1` via parakeet.cpp (80–160 ms,
emits `<EOU>`/`<EOB>`) — a strictly better end-of-utterance signal than silence-duration heuristics, at
125 MB. Do **not** use its text (10.9 % LS-clean, no punctuation).

**Explicitly not shortlisted, with reasons:** Whisper any size (chunked-only, 3.3 s LocalAgreement latency
on an A40, and large-v3-turbo is *below real time* on a weak CPU); parakeet-tdt-0.6b-v2/v3 as the live model
(batch-only in every runtime; sherpa-onnx #2918 documents the failed streaming attempts) — keep v3 as a
biasable re-decode path; Kyutai STT (best VAD story, but candle cuda/metal-only, no quantization, 2.4–5.6 GB,
en/fr); Voxtral Realtime as default (best license + delay knob, but 0.8–1.0× real time on a weak laptop and
no timestamps) — revisit as an opt-in GPU quality-streaming tier.

---

## 12. Implications for AudioGraph

1. **The engine swap and the acceleration epic are the same piece of work, and it is smaller than the audit
   feared.** Adopting `transcribe-cpp` gives a natively-streaming default *and* CPU/CUDA/Vulkan/Metal/ROCm in
   one dependency whose feature names mirror `whisper-rs`'s. The audit's finding that "no GPU acceleration is
   wired for the two engines that actually stream today" resolves by *replacing* those engines rather than by
   wiring EPs into `sherpa-onnx` (whose crate, verified at 1.13.5, has no GPU feature to wire).
2. **Windows GPU = Vulkan, not DirectML.** ggml has no DirectML backend; Vulkan covers NVIDIA/AMD/Intel and
   is what parakeet.cpp prebuilds for Windows x64 alongside CUDA. DirectML only exists on the `ort` paths, and
   Handy's Cargo.toml documents dropping `ort-directml` because pyke's prebuilt ORT crashes at startup on
   pre-Haswell CPUs. Recommend Vulkan as the default Windows GPU path with CUDA as an opt-in build and CPU as
   the always-present fallback — and note CUDA/cuDNN redistributables remain an unsolved packaging question
   (`tauri.conf.json` has no `resources`/`externalBin`), whereas **Vulkan needs only the GPU driver's loader**.
3. **The model-download infrastructure needs almost no change.** All candidate weights are single GGUF files
   on Hugging Face with stable `resolve/main/<name>-<QUANT>.gguf` URLs — a perfect fit for the existing
   bare-file `ModelDef` + `.download` temp file + size-verify + atomic-rename pattern in `models/mod.rs`.
   No archive extraction, no multi-file component directories (unlike the current sherpa `.tar.bz2` and
   Moonshine-ONNX component-dir shapes). Quant choice becomes a settings axis (Q4_K_M ↔ Q8_0 ↔ F16) with
   published WER deltas of ≤0.07 pp for the Nemotron family — i.e. a *safe* user-facing "smaller download"
   toggle.
4. **The trait seam question gets an answer from the C API.** `transcribe.h`'s
   `stream_begin/feed/finalize` + `committed_text`/`tentative_text` + monotonic `revision` +
   `committed_changed`/`tentative_changed` is a superset of what `AsrSpanRevisionPayload` needs, and its
   commit-policy enum (agreement vs on-finalize, with native-commit families bypassing agreement) is exactly
   the partial-vs-final distinction the ledger encodes. The Moonshine three-trait sketch
   (`Adapter`/`NativeRuntime`/`RuntimeLoader`) is the right shape — this runtime is what finally fills the
   `MoonshineUnavailableNativeLoader` hole, and (conveniently) it *also* runs Moonshine.
5. **Terminology seeding is net-new work regardless of model choice — plan it as its own ticket with a
   named strategy.** No natively-streaming model has a biasing hook in Rust today. Pick one of the three
   options in §8 explicitly; do not let "hotword parity with Deepgram keyterms" ride along as an implicit
   deliverable of the engine swap. The cheapest credible v1 is post-hoc lexical repair against the ledger;
   the strategically right one is shallow-fusion biasing contributed to the MIT transducer decoder.
6. **Diarization can converge on the same runtime.** transcribe.cpp ships Sortformer v2.1 as GGUF
   (Q8_0 139 MB, AMI-IHM DER 14.6 %) *and* the Multitalker speaker-attributed streaming ASR path is on its
   roadmap; `parakeet-rs` 0.3.7 already exposes Multitalker today via ORT. That is a path out of the current
   `compile_error!`-enforced mutual exclusion between parakeet-rs's and sherpa-onnx's ONNX Runtime linkage —
   one ggml runtime for ASR + diarization removes the conflict entirely. Note the ported Sortformer's
   K-quants were **withdrawn** because 4-bit weight error can deterministically flip a near-tie and *permute
   speaker labels mid-stream* — a direct warning against aggressive quantization on the diarization path.
7. **Two models, not one, is the correct architecture** and the leaderboard says so: the streaming/accuracy
   Pareto frontier genuinely forks (Conformer+LLM wins WER, transducers win latency), and the gap between
   `nemotron-streaming` and `granite-4.1-nar` on clean English is ~1 pp. Spend the live path on the
   transducer and the post-hoc pass on the audio-LLM; do not try to make one model do both.
8. **Benchmark on your own audio before locking the default.** Every WER number above is LibriSpeech
   test-clean or a leaderboard mean; the Appen private track demonstrates that public ordering does not
   survive contact with conversational, accented audio, and AMI columns run 5–8× higher than LibriSpeech for
   the same models. AudioGraph's actual workload is multi-speaker meeting audio over a mixer — the closest
   public proxy is AMI, and the closest real proxy is a recorded session from the app itself.

---

## Sources

**Leaderboards / surveys**
- Open ASR Leaderboard dataset: <https://huggingface.co/datasets/hf-audio/open-asr-leaderboard>
- Open ASR Leaderboard paper (86 systems, 12 datasets, 27-Mar-2026 snapshot): <https://arxiv.org/html/2510.06961v4>
- Leaderboard eval code (note "as of 24 July 2026" runbook): <https://github.com/huggingface/open_asr_leaderboard>
- Appen private-track announcement (public top-5 as of 2026-04-30): <https://www.appen.com/blog/hugging-face-open-llm-leaderboard>
- MarkTechPost 2026-07-23 roundup (dataset-subset incomparability; ARK/MOSS): <https://www.marktechpost.com/2026/07/23/best-open-speech-recognition-asr-models-in-2026-wer-languages-latency-and-license-compared/>
- CodeSOTA STT register, updated 2026-08-16: <https://codesota.com/speech-to-text>

**Models (primary cards)**
- <https://huggingface.co/nvidia/nemotron-speech-streaming-en-0.6b> · <https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b>
- <https://huggingface.co/nvidia/parakeet_realtime_eou_120m-v1> · <https://huggingface.co/nvidia/multitalker-parakeet-streaming-0.6b-v1>
- <https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2> · <https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3> · <https://huggingface.co/nvidia/canary-qwen-2.5b>
- <https://huggingface.co/mistralai/Voxtral-Mini-4B-Realtime-2602> · <https://arxiv.org/html/2602.11298v2>
- <https://huggingface.co/kyutai/stt-1b-en_fr> · <https://kyutai.org/stt/> · <https://arxiv.org/html/2509.08753v1> (DSM)
- <https://huggingface.co/moonshine-ai/moonshine-streaming-small> · <https://huggingface.co/ibm-granite/granite-speech-4.1-2b-nar>
- <https://huggingface.co/CohereLabs/cohere-transcribe-03-2026> · <https://huggingface.co/OpenMOSS-Team/MOSS-Transcribe-Diarize>
- <https://huggingface.co/Audio8/ARK-ASR-3B> · <https://huggingface.co/Qwen/Qwen3-ASR-1.7B> · <https://huggingface.co/openai/whisper-large-v3-turbo> · <https://huggingface.co/distil-whisper/distil-large-v3.5>
- Cache-aware streaming FastConformer: <https://arxiv.org/pdf/2312.17279> · <https://docs.nvidia.com/nemo-framework/user-guide/latest/nemotoolkit/asr/models.html>

**Runtimes (with measured tables)**
- transcribe.cpp: <https://github.com/handy-computer/transcribe.cpp> · README, `include/transcribe.h`, `docs/models/*`, `docs/extension-kinds.md`; GGUFs at <https://huggingface.co/handy-computer>
- Rust binding `transcribe-cpp` 0.2.1 (2026-08-20, MIT): crates.io
- parakeet.cpp (MIT, ggml, Windows cpu/vulkan/cuda prebuilts): <https://github.com/mudler/parakeet.cpp> · <https://github.com/mudler/parakeet.cpp/blob/master/benchmarks/BENCHMARK.md>
- `parakeet-rs` 0.3.7 (2026-07-28): <https://crates.io/crates/parakeet-rs> · <https://github.com/altunenes/parakeet-rs>
- sherpa-onnx: <https://github.com/k2-fsa/sherpa-onnx> · hotwords <https://k2-fsa.github.io/sherpa/onnx/hotwords/index.html> · PR #3077 (merged 2026-02-05) · issue #2918 (parakeet streaming) · issue #2753
- Handy (Rust/Tauri reference integration): <https://github.com/cjpais/Handy> — `src-tauri/Cargo.toml`, `src-tauri/src/managers/model.rs`
- Whisper streaming policy: <https://aclanthology.org/2023.ijcnlp-demo.3.pdf> · <https://github.com/ufal/whisper_streaming> · Simul-Whisper <https://arxiv.org/html/2406.10052v1>
- Streaming zipformer: <https://k2-fsa.github.io/sherpa/onnx/pretrained_models/online-transducer/zipformer-transducer-models.html> · <https://arxiv.org/pdf/2506.14434>

**Explicitly UNVERIFIED**
- Whether word timestamps are available on *partial* (pre-finalize) streaming results in transcribe.cpp
  (parakeet.cpp's streaming C API definitively does not expose them).
- Whether a GPU-enabled `libsherpa-onnx` can be linked through the `sherpa-onnx` 1.13.5 crate out-of-band.
- Whether Qwen3-ASR's upstream `context` biasing and Granite 4.1's keyword biasing are reachable through the
  ggml ports (the C API advertises `INITIAL_PROMPT` for whisper only).
- Whether hotwords work for *online* (cache-aware) NeMo transducer models in sherpa-onnx; PR #3077 covers the
  offline decoder.
- The `parakeet_realtime_eou_120m` 10.92 % LibriSpeech test-clean figure's decode mode (NeMo agrees exactly,
  so it is not a port artifact, but NVIDIA publishes no comparable number).
- ARK-ASR-3B / MOSS-Transcribe-preview-2B leaderboard placements (secondary source only; scored over 7 of
  the 8 English sets, so not comparable to the 8-set means quoted elsewhere).
- Moonshine-Streaming tiny/small F16 sizes in §10 (interpolated, not published).
