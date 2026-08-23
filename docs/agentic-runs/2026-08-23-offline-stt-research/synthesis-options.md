# Offline STT v2 — Architecture Options Memo (synthesis of 8-investigator fan-out, 2026-08-23)

Inputs: `audit-current-stack.md`, `realtime-stt-deep-dive.md`, `rust-inference-stacks.md`,
`gpu-backend-landscape.md`, `model-landscape.md`, `diarization-local.md`,
`terminology-biasing.md`, `model-distribution.md` (same directory). All version/size/latency
claims below are sourced from those artifacts, which verified them live on 2026-08-23;
UNVERIFIED items are carried forward as such.

---

## 1. Executive verdict

Bet on **Architecture C: a capability-matrix multi-engine design behind a real
`AsrEngineRuntime` trait**, with NVIDIA's cache-aware streaming FastConformer family
(`nemotron-speech-streaming-*-0.6b`) as the default live model, a hardware probe + explicit
backend ladder as the selection spine, and **two inference substrates deliberately retained**
— ggml (Vulkan/Metal/CUDA, via `transcribe-cpp`/`whisper-rs`) and ONNX Runtime
(DirectML/CUDA, via `parakeet-rs`/`ort`) — because the evidence forks exactly there and a
1-week bake-off (Phase 1) settles which one is the *default* streaming path. The reasons this
is the bet and not a hedge: (1) the model question is settled — a 600M natively-streaming
transducer does 9–10× real time on a weak 2020 laptop CPU where `whisper-large-v3-turbo` does
0.7×, at 2.31% LS-clean *in streaming mode* (`model-landscape.md` §6); every "streaming
Whisper" scheme has a ≥1.2–3.3 s latency floor the maintainer's latency priority rules out;
(2) the runtime question is genuinely open — ggml/Vulkan has measured ASR numbers, zero
redistributable bytes, and a ledger-shaped streaming C API, while ort/DirectML costs +0 MB on
Windows and is already in the tree via `parakeet-rs`, and **no DirectML-vs-Vulkan ASR
head-to-head exists anywhere** (`gpu-backend-landscape.md` §14); (3) most of the parts are
already in `Cargo.toml` — the gap is organization (trait, probe, EP ladder, degraded-state
surfacing), not new science (`rust-inference-stacks.md` §Implications-1). The two
single-crate alternatives both fail on evidence: sherpa-onnx-max is CPU-only as pinned with a
3.88%-WER model floor and no streaming diarization; candle/kyutai has no Vulkan/ROCm/DirectML
backend at all, i.e. zero GPU for the median Windows user. Total estimated dedicated-branch
effort: **~9–13 engineer-weeks** across seven verifiable phases, of which the first
(probe + trait spine, ~1.5–2 wk) proves the architecture end-to-end before any engine swap.

---

## 2. Current-state constraints that shape every option (from `audit-current-stack.md`)

| Constraint | Fact |
|---|---|
| No provider trait | `settings::AsrProvider` enum + one `run_<x>_speech_processor` free function per provider inside 14,865-line `speech/mod.rs`. Only in-repo trait sketch: Moonshine's 3-trait seam (`Adapter`/`NativeRuntime`/`Loader`), never exercised. |
| Default local engine is batch | Whisper, `TranscriptFinalOnly`, `supports_streaming: false`. Directly contradicts the latency priority. |
| Only streaming local ASR is CPU-only | `sherpa_streaming.rs` hardcodes `provider="cpu"`, `num_threads=2`, `greedy_search`; the pinned `sherpa-onnx` 1.13.5 crate has **no GPU Cargo feature at all** (features = `default/shared/static` only — verified 3× independently). |
| Hotword surface is inert | sherpa hotwords require transducer + `modified_beam_search`; repo uses `greedy_search`. Live latent gap, days to fix. |
| Diarization backends mutually exclusive | `diarization` (parakeet-rs Sortformer) ⊥ `diarization-clustering` (sherpa-onnx) via `compile_error!` — two statically-linked ONNX Runtimes. `ort load-dynamic` + `SHERPA_ONNX_LIB_DIR shared` is the designed fix (~2–3 day spike). |
| Silent degradation is structural | `make_diarization_config` picks backend by `Path::exists()` only, ignores `DiarizationSettings.mode` (seed **586b**); `StageStatus` has no `Degraded` variant; `ModelReadiness`/`get_model_status` exists but is unused by the selection path. |
| GPU features fork the artifact | `cuda`/`vulkan` are opt-in, apply only to whisper-rs/llama-cpp-2; `whisper-rs-sys` statically links ONE ggml backend per binary (a cuda build **fails to start** without cuBLAS DLLs — loader error before `main()`). `llama-cpp-2` and `transcribe-cpp` both expose `dynamic-backends`; whisper-rs does not. |
| Nothing is bundled | No `resources`/`externalBin` in `tauri.conf.json`; models download post-install via hand-rolled `reqwest::blocking` (no checksums, no revision pinning, no resume, roaming `%APPDATA%`). |

---

## 3. Candidate architectures

### Architecture A — "Sherpa-Max": expand the existing sherpa-onnx path to its ceiling

What it is: keep `sherpa-onnx` as the sole local engine; fix `modified_beam_search` +
hotwords; add better zipformer/NeMo models through it; GPU via self-compiled ONNX Runtime
pointed at `SHERPA_ONNX_LIB_DIR`.

- **Users get:** real streaming ASR with the best-in-class terminology surface
  (`hotwords_buf`, per-stream hotwords at endpoint boundaries, HLG graphs, ITN FSTs) and
  unbounded-speaker offline diarization. But: quality floor is the zipformer-20M at 3.88%
  LS-clean (vs 2.31% Nemotron); **CPU-only on every user machine as pinned** — the official
  crate's maintainer explicitly refuses GPU features (k2-fsa/sherpa-onnx#3606:
  "we only support CPU, compile your own library"); no streaming diarization exists in
  sherpa-onnx at all (offline clustering only); Windows CUDA archive is 358 MiB **plus** a
  user-side CUDA/cuDNN install; there is no DirectML archive upstream at all.
- **Cost:** days for the hotword fix; ~2–4 weeks to stand up and *maintain* a custom
  GPU-ORT build lane (CUDA-only; DirectML = build sherpa from source, "a project you
  probably shouldn't start" — `rust-inference-stacks.md` §4). Nemotron-through-sherpa is
  possible (≥1.13.4 required or it decodes badly *silently*) but whether that impl supports
  `modified_beam_search` is UNVERIFIED — if not, quality and hotwords can't coexist here.
- **Risk register:** silent-correctness trap on Nemotron <1.13.4; hotword/quality fork;
  permanent custom-ORT CI burden; `sherpa-rs` (the community crate with `cuda`/`directml`
  features) is **ARCHIVED** (GitHub `archived: true`, 2026-03-08) — `diarization-local.md`'s
  "worth a spike" suggestion is overruled by `rust-inference-stacks.md`'s live GitHub check.
- **Forecloses:** vendor-neutral Windows GPU, streaming diarization, the sub-second-latency
  model tier.
- **Verdict: rejected as the whole answer** — this is precisely the "smallest available
  subset" the maintainer ruled out, and the evidence shows it's even smaller than assumed
  (CPU-only). **Sherpa stays as a component**: the terminology side-path engine and the
  unbounded-speaker refine diarizer.

### Architecture B — candle-native / kyutai-derived engine

What it is: build or adopt a pure-Rust streaming engine on candle (kyutai `stt-rs` shape:
80 ms frames, 0.5 s structural delay, semantic VAD, model-native word timestamps).

- **Users get (in theory):** the architecturally best streaming design surveyed, single
  static Rust binary, no C++.
- **Why it fails on evidence:** candle 0.11.0's complete backend list is
  `accelerate/cuda/cudnn/metal/mkl/nccl/ug` — **no Vulkan, no DirectML, no ROCm**
  (`rust-inference-stacks.md` §1), so Windows AMD/Intel GPU users get CPU, and a 1B
  autoregressive audio LM on CPU has **no published real-time factor** (UNVERIFIED = unproven
  for live use). `kyutai-stt-rs` is unpublished (0.1.0 path crate), pins candle 0.9.1 (two
  minors stale), repo untouched since 2026-01-26; its `-candle` HF weights have ~zero
  downloads. burn has the right backend matrix (wgpu→Vulkan/DX12) and **zero ASR models**;
  `whisper-burn` dead since 2024.
- **Cost:** 4–6 weeks minimum to vendor + revive, months if porting to burn; permanent
  maintenance of an abandoned-upstream stack.
- **Forecloses:** the primary product target (Windows GPU coverage), for the duration.
- **Verdict: rejected.** Steal its *requirements* (80 ms frames, bounded delay, semantic VAD
  as model output, model-native word timestamps) as acceptance criteria for whatever engine
  wins. Revisit only if kyutai publishes on candle ≥0.11 with a Vulkan story, or DSM models
  get ONNX exports (at which point they become an ort engine like everything else).

### Architecture C — capability-matrix multi-engine behind a real trait (RECOMMENDED)

What it is: introduce `trait AsrEngineRuntime` (the Moonshine 3-trait seam is the in-repo
sketch; `transcribe.h`'s `committed_text`/`tentative_text` + monotonic `revision` +
`is_updated`/`is_complete` contracts from `model-landscape.md` §5 and
`rust-inference-stacks.md` §7 are the external references — both are supersets of
`AsrSpanRevisionPayload` needs). Behind it: a hardware probe, an explicit backend ladder, a
capability matrix declared in `provider-registry` (the most future-proofed crate in the
tree), and 3–4 engines each kept for what it uniquely does:

| Engine | Role | Unique capability |
|---|---|---|
| **Streaming default** (C1 or C2, below) | live ledger | Nemotron cache-aware streaming, word+token timestamps on a 0.08 s grid, GPU |
| sherpa-onnx (fixed config) | terminology side-path + refine diarizer | only working local hotword mechanism; only unbounded-speaker diarizer |
| whisper-rs + Vulkan | batch quality/multilingual escape hatch | 99 languages, GBNF grammar constraints, zero bundle cost |
| (optional) Moonshine native C API | live-vocab spike | only true mid-stream `set_keyterms()` with no stream recreation |

**The one genuinely open fork inside C — the default streaming substrate:**

- **C1: ort-first** (`parakeet-rs` 0.3.7 promoted from diarization-only to primary ASR).
  Pros: already a dependency; DirectML costs **+0 MB** (ort's default Windows download has no
  CPU-only row — it *is* the DirectML archive); covers NVIDIA+AMD+Intel+Qualcomm with no user
  install; streaming ASR (Nemotron 560 ms, EOU 160 ms) *and* Sortformer streaming diarization
  *and* Multitalker speaker-attributed ASR in one crate; `load-dynamic` turns the CUDA
  decision into a post-install download; prebuilt archives = short compile chain; the only
  on-ramp to NPUs/Windows ML later. Cons: ort **silently falls back to CPU** when EPs fail
  (worst failure mode for live transcription; must defeat with `.error_on_failure()` +
  `is_available()` probes); a shipped product (Handy) *dropped* `ort-directml` because pyke's
  prebuilt ORT is compiled `/arch:AVX2` and **crashes at startup on pre-Haswell CPUs**;
  DirectML is frozen ("sustained engineering"), opset ≤20, degrades on variable shapes;
  parakeet-rs has no hotword API, bus factor 1, weights partly in personal HF namespaces;
  ort is still an RC after 4 years; **zero published DirectML ASR benchmarks**.
- **C2: ggml-first** (`transcribe-cpp` 0.2.1 as the new runtime; Vulkan default). Pros:
  Vulkan = **zero redistributable** (driver-supplied `vulkan-1.dll`, all three GPU vendors
  incl. RDNA2 that AMD's own Windows ROCm drops), measured at a few %–30% off CUDA *on ASR*;
  `dynamic-backends` feature solves the one-backend-per-binary problem whisper-rs has;
  `transcribe_model_backend()` reports the actually-bound backend — a built-in
  CPU-fallback detector; the C API's committed/tentative/revision contract is the closest
  external match to the transcript ledger; **one runtime can also carry the diarizer
  (Sortformer v2.1 GGUF, Q8_0 139 MB) and the batch quality tier (Granite/Cohere/MOSS)** —
  dissolving the ORT link conflict by removing ORT from the streaming path entirely; same
  cuda/vulkan/metal feature shapes already wired for whisper-rs. Cons: the Rust crate is
  **3 days old** at research time (0.2.1, 2026-08-20) — the single freshest load-bearing
  dependency in this plan; C++ CMake vendored build lengthens the compile chain (relevant to
  this box's one-Rust-lane constraint); whether **word timestamps arrive on partials** (vs
  only at finalize) is UNVERIFIED and load-bearing for diarization alignment; no hotwords
  either (same as C1).

**Recommendation inside C:** lean **C2 (ggml/Vulkan)** as the default streaming path
*conditional on* the Phase-1 bake-off passing three gates (crate maturity smoke test,
word-timestamps-on-partials, Vulkan ≥ DirectML on the same GPU), with **C1 kept wired
regardless** — parakeet-rs stays in the tree for Sortformer/Multitalker and becomes the
default if any C2 gate fails. This is a ~1-week experiment, not a leap of faith, and the
trait makes the loser a fallback rather than dead code.

- **Users get (C either way):** sub-second streaming partials at 2.3–3.1% LS-clean, GPU
  acceleration on NVIDIA/AMD/Intel with zero extra installs, visible "Transcribing on: X"
  tier reporting, streaming ≤4-speaker diarization + post-session unbounded refine, real
  terminology seeding (side-path now, decode-time later), CPU floor that actually keeps up
  (7–10× RT at Q4_K_M).
- **Cost:** ~9–13 weeks phased (see §7). **Build** only the thin layer nothing supplies:
  trait, probe/ladder, degraded-state surfacing, CUDA-pack fetch, correction pass. **Adopt**
  everything else.
- **Risk register:** young crates (transcribe-cpp 3 days; parakeet-rs bus factor 1);
  community-maintained Sortformer ONNX export (NVIDIA explicitly declined to support it);
  NVIDIA-OML/OpenMDW licenses need a legal read; ort RC churn (rc-to-rc renames are real).
- **Forecloses:** nothing — that is the point of the trait. The marginal cost over A is
  ~4–6 weeks; the return is the difference between a CPU-only 3.88%-WER product and a
  GPU-accelerated 2.31%-WER one on the same hardware.

---

## 4. Acceleration matrix → shipping decision

Collapsed from `gpu-backend-landscape.md` §11 (ship-feasibility grades) and
`rust-inference-stacks.md` distribution arithmetic:

**Commit in v2:**

| Backend | Where | Why | Payload |
|---|---|---|---|
| **Vulkan (ggml)** | Windows default GPU (pending Phase-1 gate) | vendor-neutral (NVIDIA+AMD-all-gens+Intel), measured ASR numbers, zero user install | ~0 MB |
| **DirectML (ort)** | Windows GPU floor / C1 default | +0 MB over the default ort download; any DX12 GPU since Win10 1903 | 12.5–29.6 MB already in ORT dl |
| **CPU** (ggml tinyBLAS + ORT MLAS) | everywhere, always | the floor; Nemotron Q4_K_M = 7–10× RT on a 2020 laptop | 0 |
| **Metal** | macOS | already unconditionally on; ~4.4× over CPU; 151× RT for Nemotron on M4 Max | 0 |
| **CUDA — opt-in downloaded pack only** | NVIDIA users who want it | never in the installer: ort ships CUDA 13 ONLY (driver ≥580 floor), provider DLL 62 MB (ort) / 328 MB (sherpa), **neither bundles cudart/cuBLAS/cuDNN** (cuBLAS alone = 553 MB); gate on an `nvml-wrapper` driver probe; deliver via `ort load-dynamic` / ggml `dynamic-backends` through the model-download machinery | 0 default |

**Defer (write the ADR, don't build):** Windows ML EP catalog (`WinMLEpCatalog*` C API →
`ort::Environment::register_ep_library`) — the only NPU on-ramp (QNN/VitisAI/OpenVINO/
TensorRT-RTX) with zero vendor bytes shipped; needs Win11 24H2 hardware and Microsoft's own
docs contradict each other on C/C++ self-contained deployment. 2027 lane.

**Reject with reasons on the record (candidate ADR content):** ROCm on any OS (whisper-rs
`hipblas` is Linux-only; Vulkan covers strictly more AMD hardware incl. the RDNA2 cards
AMD's Windows HIP SDK marks unsupported); wgpu/WebGPU compute for ASR (coop-matrix 8×8 f32
only; ORT webgpu EP experimental); `cuda-oxide` (abandoned 2021); `mlx-rs` (stale, no Whisper
binding); `sherpa-rs` (archived 2026-03-08); AVX-512 CPU variants (measured to buy ~nothing —
the levers are BLAS choice and int8 quant); building an inference engine on burn/candle.

**Cross-cutting invariants (all engines):** never compile-time-link a GPU dep not guaranteed
present (dlopen only); probe once, cache keyed on `{backend, adapter_luid, driver_version,
app_version}`; demote mid-session on device-lost to a warm CPU context without dropping audio
(continuity > WER for the graph); fixed-shape chunk padding on ORT paths; warm-up at tier
selection, not first utterance; surface the active tier in the UI with manual override.
Probe primitives all exist today: DXGI via the `windows` crate already in-tree,
`libloading` for `vulkan-1.dll`, `nvml-wrapper` for the CUDA gate, `is_available()` on ort,
`transcribe_model_backend()` on ggml.

---

## 5. Model lineup

| Slot | Model | Disk | RAM/VRAM signal | License | Notes |
|---|---|---|---|---|---|
| **Live default** | `nemotron-speech-streaming-en-0.6b` Q8_0 (or `nemotron-3.5-asr-streaming-0.6b` Q8_0 if multilingual) | 696 / 716 MB | ~2.3 GB f32 RSS, less quantized | NVIDIA-OML / OpenMDW-1.1 — **legal read required** (Sortformer-OML precedent already shipped) | 2.31% LS-clean *streaming*; lookahead 0/80/480/1040 ms is a runtime knob, one download; word+token timestamps; PnC native |
| **Smaller-download live** | same model, Q4_K_M | 453 / 473 MB | lower | same | +0.07 pp WER — quant is a safe user-facing toggle |
| **Quality post-hoc pass** | `granite-speech-4.1-2b-nar` (text) or `MOSS-Transcribe-Diarize` Q5_K_M (speaker+timestamp repair) or `cohere-transcribe-03-2026` Q4_K_M (14 langs) | 2.33 GB / ~700 MB / 1.55 GB | MOSS: ~85 MB RAM per audio-minute | all Apache-2.0 | all run under the same runtime as the default |
| **Smallest option** | `moonshine-streaming-small` Q8_0 | 189 MB | small | MIT | 2.54% LS-clean, 9–15× RT CPU — but **no timestamps, English-only, ~17 min/session cap**; a "smallest download" option, NOT the fallback tier (fallback = Nemotron Q4_K_M, which keeps timestamps) |
| **Endpointing signal** | `parakeet_realtime_eou_120m-v1` | ~125 MB | small | NVIDIA-OML | emits `<EOU>`/`<EOB>` at 80–160 ms — replaces silence-duration heuristics; **never use its text** (10.9% WER) |
| **Diarization live** | Sortformer v2.1 (GGUF Q8_0 or community ONNX) | 139–237 MB | small | NVIDIA-OML | ≤4 speakers; **do not quantize below Q8_0** (K-quants withdrawn — 4-bit error permutes speaker labels mid-stream) |
| **Diarization refine** | pyannote-seg-3.0 re-host + embedding (spike CAM++ EN ~28 MB vs current TitaNet-small) | tens of MB | small | MIT-derived re-host | unbounded speakers, offline |
| **Multilingual/batch escape hatch** | `whisper-large-v3-turbo` Q8_0 (existing) | 845 MB | — | MIT | 99 languages, GBNF grammar; batch only — stop pretending it streams |

**Download-manager implications** (`model-distribution.md`): every candidate is a single GGUF
(or small ONNX) at a stable HF `resolve/<sha>/` URL — the existing bare-file
`ModelDef` + `.download` temp + verify + atomic-rename path covers all of them. Required
hardening regardless of engine choice: **(1)** `app_local_data_dir()` not `app_data_dir()`
(multi-GB models out of the roaming profile — one line); **(2)** SHA-256 per file alongside
the size check; **(3)** pin every HF URL to a commit SHA (the LFM2 filename-casing 404 in
`models/mod.rs:83-85` is the in-repo proof this bites); **(4)** externalize the `ModelDef`
table into a versioned, checksummed JSON manifest (bundled + re-fetchable) so model fixes are
data updates, not releases; **(5)** honor HF's 429 `RateLimit` header (3,000 anon
requests/5 min *per source IP* — corporate NAT is the realistic trigger). **Do not adopt
`hf-hub`**: 1.x is a 6-week-old total rewrite with zero ecosystem uptake whose Windows cache
layer unconditionally *copies* blobs (2× disk, conditional-requests disabled by its own
source comment). Do not build delta updates or content-addressed dedup — no incumbent at this
scale (~10 fixed models) needs either. Keep the re-host-gated-models pattern (sherpa GitHub
releases for pyannote) — never route end users through HF's browser gate.

---

## 6. Diarization + terminology integration against the existing seams

**Diarization (two tiers, both engines kept — `diarization-local.md` §9):**
- *Live:* Sortformer streaming (AOSC speaker cache) — the only architecturally-true streaming
  diarizer reachable from Rust; its trained cache beats cache-free windowed re-diarization by
  ~30 DER points on AMI-SDM, which means AudioGraph's ADR-0017 rolling-window +
  `stabilize.rs` centroid matching is a materially weaker approximation, not just a latency
  tradeoff. Surface the 4-speaker cap as a readiness/degraded signal.
- *Refine:* sherpa `OfflineSpeakerDiarization` post-session (unbounded speakers); spike the
  same-day TitaNet→CAM++ embedding swap. Both tiers in ONE build requires killing the ORT
  link conflict (`load-dynamic` spike) or moving the live diarizer to the ggml runtime (C2).
- *Contract:* emit into the existing `DiarizationSpanRevisionPayload`
  (Provisional/Stable/Final + `basis_*_ids`) — the one clean seam the audit found. Design
  the KG ingestion to accept provisional-speaker utterances now and label backfill later
  (WhoSpeaks queue-consumer pattern; RealtimeSTT's author: real-time diarization "isn't
  solved in opensource still" — set expectations to *lagging*, not frame-synchronous).
- *Seed 586b lands first or with Phase 1:* add `StageStatus::Degraded`, make
  `make_diarization_config` consult `DiarizationSettings.mode` + `ModelReadiness`
  (`get_model_status` already exists and is unused there), and report fallback hops into
  `PipelineStatus`. The new engine inherits this blind spot otherwise.
- *Gate change:* replace the `num_speakers >= 1` assertion with a **cpWER-style
  word-attribution test** on the curated multi-speaker clip ADR-0017 already needs — DER can
  read 15.1% ("good") while a third of words are misattributed (30.7% cpWER). Highest
  leverage, lowest cost item in the diarization lane. Add the nearest-fallback case to
  `overlap_speaker_for_segment` (it already does aggregate-overlap better than the public
  reference implementations; it just returns `None` on zero overlap).
- *Multitalker option:* parakeet-rs's speaker-attributed streaming ASR (80 ms–1.12 s
  LatencyMode) could collapse ASR+diarization into one pass — worth a spike against the
  two-stage design, not a Phase-1 commitment.
- *Speaker-ladder epic 9509:* referenced in planning context but **not found in the repo**
  by grep (`diarization-local.md`) — treat as forthcoming; the provisional-label +
  backfill contract above is the integration point it will need.

**Terminology seeding (three tiers, own ticket per tier — do NOT let it ride along with the
engine swap; `terminology-biasing.md`, `model-landscape.md` §8):**
1. **v1 — engine-agnostic correction pass** (highest leverage, zero lock-in): candidate list
   from session graph entities + attendees (exactly what seeds **3d3e/c9b3** are about) →
   fuzzy+phonetic scoring → top-K to the existing LLM route with an entity-only
   constrained-edit prompt → deterministic verification before touching transcript or graph.
   The RECOVER/DeRAGEC literature is unanimous: never an unconstrained cleanup pass.
2. **v1.5 — sherpa hotword side-path**: switch `sherpa_streaming.rs` to
   `modified_beam_search` on a transducer (measure the latency delta), populate
   `hotwords_buf` + `modeling_unit`/`bpe_vocab` from model-bundle metadata, and recreate the
   `OnlineStream` (not the recognizer) at the existing `is_endpoint` reset — mid-session
   vocab updates with no reconnect, structurally easier than the Deepgram reconnect problem
   blocking **c9b3**. Note in c9b3 that the local variant is strictly easier, so its design
   doesn't anchor to cloud-reconnect constraints.
3. **v2 — decode-time biasing on the streaming default**: port sherpa's `ContextGraph`
   (shallow-fusion, Aho-Corasick) into the MIT transducer beam decoder of
   transcribe.cpp/parakeet.cpp — the only option that biases the *default* engine, and it is
   upstreamable. Scoped C++ change against an existing beam decoder, not research.
- *Moonshine fork must be an explicit recorded decision:* native `moonshine-ai/moonshine`
  (Path B) has the best mid-session story anywhere (`set_keyterms()` live, no recreation);
  the sherpa-hosted Moonshine ONNX path (Path A) has **no biasing hook at all**. If
  `moonshine.rs` ever gets wired, the choice of path silently decides whether biasing exists.
- *Seed d163 is not this:* verified to be graphChangeFeed UI copy, unrelated to ASR
  vocabulary — don't conflate in ticket planning. Relevant prior art is 3d3e/c9b3 only.

**Orchestration layer (engine-agnostic, port from RealtimeSTT concepts):** dual-VAD cascade
(webrtc-vad gate → Silero confirm off-thread, state reset at utterance boundaries), ~1 s
preroll ring buffer, outlier-rejecting partial-text stabilizer between raw partials and
UI/KG, dynamic endpointing (punctuation + completeness blend, later the EOU model),
speculative early-finalization, drop-partials-before-finals backpressure. All implementable
with verified crates (`webrtc-vad` 0.4.0, several Silero ports).

---

## 7. Phasing — dedicated branch, verifiable units

Sequenced so every phase lands something testable and the branch is mergeable at each
boundary. Estimates assume one competent dev in this codebase.

- **Phase 0 (with/before branch): correctness guards** (~2–3 days). 586b surfacing
  (`StageStatus::Degraded`, mode-aware `make_diarization_config`, `ModelReadiness` reuse);
  `app_local_data_dir` fix; SHA-256 + URL SHA-pinning in `models/mod.rs`. All independent of
  engine choice; all de-risk everything after.
- **Phase 1: the spine — hardware probe + engine trait, proven end-to-end** (~1.5–2 wk).
  `AsrEngineRuntime` trait (Moonshine-seam shape + committed/tentative/revision semantics);
  capability probe (DXGI + Vulkan libloading + nvml + ort `is_available()`); selection
  ladder with cached decision + UI surfacing ("Transcribing on: …") + manual override;
  route the two *existing* engines (Whisper batch, sherpa streaming) through the trait so the
  spine is proven with zero new native deps. Registry: declare the new engine as
  `ProviderStatus::Planned` (the established half-wired pattern). **Exit criterion: a
  session runs through the trait with the probe's chosen tier visible and a forced-failure
  test demotes tiers without dropping audio.**
- **Phase 2: streaming-substrate bake-off (C1 vs C2) + link-conflict spike** (~1–1.5 wk).
  Build both Nemotron paths behind the trait (parakeet-rs/DirectML+`error_on_failure`;
  transcribe-cpp/Vulkan); run the head-to-head on one Windows GPU (the missing measurement);
  test transcribe-cpp partial-word-timestamps and the AVX2-baseline crash claim; spike
  `ort load-dynamic` + `SHERPA_ONNX_LIB_DIR shared`. **Exit criterion: a written decision
  (mini-ADR) naming the default substrate, with numbers.**
- **Phase 3: default-engine promotion + model plumbing** (~2 wk). Winner becomes the default
  local provider (this is a product-level default change — ADR-0033 / `ui_selectable`
  precedent, not "add a provider"); Nemotron default + quant/lookahead settings axes;
  manifest externalization; assert sherpa ≥1.13.4 wherever Nemotron weights can load;
  registry promotion Planned→Implemented. **Exit: live meeting transcribes on GPU tier with
  sub-second partials on a mid Windows box; CPU tier keeps up on the 4750U-class proxy.**
- **Phase 4: diarization convergence** (~1.5–2 wk). Sortformer live tier + clustering refine
  in one build (via Phase-2's link fix or ggml Sortformer); provisional/backfill KG
  contract; cpWER gate + nearest-fallback; 4-speaker-cap degraded signal; CAM++ embedding
  A/B. **Exit: cpWER measured on the curated clip, both tiers active in one binary.**
- **Phase 5: terminology tiers 1 + 1.5** (~1.5 wk). Correction pass (constrained-edit +
  verification); sherpa hotword side-path with endpoint-boundary stream recreation;
  vocabulary source shared with the Deepgram keyterm settings (3d3e). **Exit: seeded-term
  recall measurably up on a scripted test clip, no regression on non-entity WER.**
- **Phase 6: acceleration extras + packaging polish** (~1–1.5 wk). CUDA opt-in pack
  (nvml-gated post-install download through the model manager); whisper-rs Vulkan CI leg for
  the batch escape hatch (BUILDING.md doesn't cover Windows Vulkan — expect friction);
  quality-tier post-hoc pass wiring (Granite-NAR or MOSS); optional first-run bundle per §8
  decision.
- **Phase 7 (parallel/later): ADRs + deferred lanes.** Rejections ADR (§4 list); Windows ML
  ADR (build 2027); shallow-fusion biasing upstream contribution (terminology v2);
  Multitalker single-pass spike; Moonshine native keyterms spike.

Total: ~9–13 weeks. Phases 0–3 (~5–6 wk) deliver the headline capability; 4–6 complete the
brief.

---

## 8. Open maintainer decisions (genuine product/preference calls)

1. **License posture for the default model.** NVIDIA-OML (`nemotron-en`, Sortformer —
   precedent already shipped) and OpenMDW-1.1 (`nemotron-3.5`) both need a real legal read.
   If the answer must be "fully permissive only," the streaming defaults collapse to
   Moonshine-Streaming (MIT, no timestamps) or Voxtral-Realtime (Apache-2.0, ~1× RT on weak
   laptops, no timestamps) — a materially worse product. This is a risk-tolerance call, not
   an experiment.
2. **English-first or multilingual-first default** (nemotron-en 2.31% vs nemotron-3.5 3.04%
   LS-clean, 19–32 locales, different license). Ships as one default; the other stays a
   download.
3. **Bundle a first-run model or not.** Zipformer-20M (~65 MB, streaming, already in the
   table) via `bundle.resources` vs zero-bundle + first-run download of the 453–716 MB
   default. Installer-size ceiling is a product preference (WebView2 offline bootstrapper
   alone is ~127 MB, for scale).
4. **Replace LocalWhisper as the default local provider, or add-alongside?** Registry
   MVP-selectability and settings-migration UX are product calls (ADR-0033 precedent).
5. **Offer the CUDA opt-in pack at all in v2**, given Vulkan/DirectML get within a few
   %–30% and the pack costs a driver-floor support matrix (≥580) and a 62–553 MB download
   surface. Deferring it entirely is a legitimate cut.
6. **>4-speaker meetings:** ship with a visible "diarization capped at 4 live speakers,
   refined after the session" degradation, or fund ADR-0017's rejected Option C (hand-rolled
   online clustering, unbounded + streaming) as a second live tier — extra weeks for a
   segment of meetings whose size is a product question.
7. **Gated-model policy:** confirm "re-host only, never route users through HF's browser
   gate; BYO-token as a future opt-in" as standing policy.
8. **User-facing knob budget:** expose quant (smaller-download) and lookahead (latency vs
   accuracy) as settings, or auto-pick and keep the surface minimal.

## 9. Prototype probes (facts an experiment settles — not maintainer calls)

| Probe | Settles |
|---|---|
| Nemotron via transcribe-cpp/Vulkan vs parakeet-rs/DirectML on the same Windows GPU (latency + RTF + WER on one recorded AudioGraph session) | C1 vs C2 default; the missing DirectML-vs-Vulkan ASR datapoint |
| transcribe-cpp: word timestamps on *partials* or only at finalize? | whether C2 satisfies `AsrSpanRevisionPayload`/diarization alignment; parakeet.cpp's C API definitively does not |
| pyke prebuilt ORT on a pre-Haswell (no-AVX2) VM | whether the Handy `/arch:AVX2` startup-crash report kills DirectML-by-default |
| parakeet-rs DirectML on real AMD + Intel Windows hardware | whether auto-CPU-fallback is masking a broken EP |
| `ort load-dynamic` + `SHERPA_ONNX_LIB_DIR shared` link spike | dissolving the diarization ⊥ clustering `compile_error!` |
| sherpa `modified_beam_search` vs `greedy_search` latency delta on the streaming worker; and whether the NeMo-unified streaming impl supports it | hotword cost; whether quality and hotwords can share one sherpa engine |
| Whisper-rs Vulkan Windows CI build (undocumented in BUILDING.md) | escape-hatch viability + CI cost |
| Run the model shortlist on one real multi-speaker AudioGraph session | whether LibriSpeech ordering survives contact with actual meeting audio (Appen private-track evidence says public ranks don't) |
| cpWER on the curated clip with current stack | baseline before any engine change |
| Moonshine native C-API spike (`is_updated`/`is_complete`, `set_keyterms`) direct FFI, not the 9-day-old `moonshine-rs` | whether the live-vocab tier is worth a v2.x slot |
| CAM++ vs TitaNet-small embedding A/B (same `SpeakerEmbeddingExtractor` API) | refine-tier embedding choice, same-day |

## 10. Frank risks / thin-evidence register

- **transcribe-cpp is 3 days old.** The C++ core (transcribe.cpp) has published WER
  validation tables and sponsors, and its reference consumer (Handy) is a shipping
  Rust/Tauri app — but the crates.io binding has ~no field mileage. If the C2 gates fail,
  C1 is the plan, not a crisis. Mitigation: the trait, plus pinning by exact version.
- **parakeet-rs is a 385-star bus-factor-1 crate**, and AudioGraph's Sortformer path rests
  on the *same person's* community ONNX export that NVIDIA explicitly declined to support
  (NeMo #15077/#15536). No alternative streaming-diarizer export exists. Mitigation: pin
  model revision SHAs + hashes; ggml Sortformer (transcribe.cpp) is a second source.
- **whisper-rs**: GitHub repo archived (canonical home Codeberg, 67 stars), one maintainer,
  5 months quiet, one whisper.cpp minor behind. Fine as an escape hatch; budget for
  vendoring or a fork if it stalls.
- **ort is still an RC after ~4 years** with real rc-to-rc API renames, and its silent-CPU
  fallback default is the worst failure mode for this product. Mitigation is mechanical
  (`error_on_failure`, probes) but must actually be written.
- **Every WER number cited is LibriSpeech test-clean or a leaderboard mean.** AMI columns run
  5–8× higher; Appen's private conversational track reorders the public leaderboard. The
  in-house bake-off on real session audio is not optional.
- **UNVERIFIED and load-bearing:** transcribe-cpp partial-word-timestamps; whether sherpa's
  NeMo-unified streaming impl supports `modified_beam_search` (forks hotwords vs quality);
  parakeet-rs DirectML on real AMD/Intel hardware; the pre-Haswell ORT crash generality;
  GPU-archive-into-`sherpa-onnx`-crate linking via `SHERPA_ONNX_LIB_DIR` (inferred, not
  link-tested); Windows ML self-contained C/C++ deployment (Microsoft docs self-contradict);
  kyutai CPU RTF (all published numbers are L40S).
- **Conflicting investigator claims, resolved:** `sherpa-rs` was flagged "worth a spike" by
  the diarization angle but verified **archived** (GitHub API) by the inference-stacks angle
  — resolved as archived/do-not-plan-around. DirectML "deprecated" reports are resolved as
  "frozen in sustained engineering, succeeded by Windows ML" — still the guaranteed-present
  Windows GPU floor.
- **Licensing tail risks:** Moonshine non-English models are non-commercial; NVIDIA-OML and
  OpenMDW-1.1 are unread by counsel; CC-BY-4.0 weights (Parakeet/Canary/Kyutai) need an
  attribution surface in About/credits.
