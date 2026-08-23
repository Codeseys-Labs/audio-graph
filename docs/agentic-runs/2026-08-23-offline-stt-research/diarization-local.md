# Local speaker diarization + embeddings for a Rust pipeline — 2026 state

## Verdict

No production-ready, GPU-accelerated, fully-streaming, Rust-native diarization stack exists today —
every option trades off one of {streaming, accuracy, license, Rust/ONNX availability}. The two
components AudioGraph already ships (sherpa-onnx `OfflineSpeakerDiarization` clustering, and
Sortformer-streaming via `parakeet-rs`) are the two most viable Rust-reachable options that exist
anywhere, not an accidentally-weak choice — but both have real, previously-undocumented gaps: (1)
sherpa-onnx's bundled pyannote **segmentation** model is the older pyannote-3.0-era model, not the
materially-better `community-1` architecture (no ONNX/Rust port of `community-1` exists anywhere);
(2) the Sortformer streaming ONNX model AudioGraph depends on is an **unofficial, community-patched
export** (NVIDIA's own engineers explicitly declined to support ONNX export for the streaming model);
(3) the official `sherpa-onnx` Rust crate is **CPU-only by maintainer decision** — no CUDA/DirectML
feature exists — while a third-party binding (`sherpa-rs`) does expose `cuda`/`directml` features and
is a concrete, low-effort upgrade path worth prototyping; (4) DiariZen, the current open-source
DER-accuracy leader (13.3% vs pyannoteAI's 11.2%), is **licensed CC-BY-NC-4.0 for weights** and is
Python/PyTorch-only — it is not usable in a commercially-shipped Windows `.exe` and has no ONNX/Rust
path. The two-tier design the maintainer is considering (cheap live labels, refined post-session) is
directionally correct and matches both the academic literature's dominant online/offline split and
AudioGraph's own architecture history (ADR-0017's rejected "Option B": streaming Sortformer live +
offline sherpa refine, blocked only by an ONNX-Runtime link conflict that process-isolation would
remove). Word-level alignment is a solved, standard technique (max-temporal-overlap, nearest-fallback)
that AudioGraph's `overlap_speaker_for_segment` already implements correctly — the real gap is that
DER-only testing (AudioGraph's one remaining accuracy gate) does not catch the short-turn / backchannel
misattribution that the industry now measures with cpWER instead.

---

## 1. Academic sweep (Semantic Scholar rate-limited to 429 throughout this session; arXiv used instead — cross-checked against OpenAlex is UNVERIFIED, not attempted)

| Paper | Venue/Date | Contribution | Rust/ONNX status |
|---|---|---|---|
| **Sortformer** — Park et al., NVIDIA, [arXiv:2409.06656](https://arxiv.org/abs/2409.06656) | ICML 2025 | Permutation-resolved e2e diarization via "Sort Loss" (arrival-time-order objective) instead of pure PIL; embeds speaker labels into an ASR encoder via sinusoidal kernels for joint multi-speaker STT+diarization supervision. | v1 checkpoint (`diar_sortformer_4spk-v1`) — offline, ONNX export requires bypassing audio preprocessing (mel-spectrogram-in only); confirmed working by a community contributor. |
| **Streaming Sortformer (AOSC)** — Medennikov, Park et al., NVIDIA, [arXiv:2507.18446](https://arxiv.org/abs/2507.18446) | Interspeech 2025 | Arrival-Order Speaker Cache: a fixed-length buffer storing per-speaker frame embeddings in arrival order, dynamically updated by highest-scoring frames, resolving cross-chunk speaker permutation for low-latency streaming. This is the mechanism behind `diar_streaming_sortformer_4spk-v2`. | **This is the model AudioGraph already ships** (`diar_streaming_sortformer_4spk-v2.onnx` via `parakeet-rs`). See §3 for the ONNX-export reality. |
| **LS-EEND** — Liang & Li, Westlake Univ., [arXiv:2410.06670](https://arxiv.org/abs/2410.06670) | IEEE TASLP 2025 | Frame-in-frame-out streaming EEND: causal embedding encoder + non-autoregressive self-attention attractor decoder + retention mechanism for **linear** temporal complexity (vs Sortformer's quadratic self-attention over the full context). Handles up to 8 speakers, hour-long audio. New SOTA *online* DER on CALLHOME (12.11%), DIHARD II (27.58%), DIHARD III (19.61%), AMI (20.76%) at publication. | PyTorch only ([Audio-WestlakeU/FS-EEND](https://github.com/Audio-WestlakeU/FS-EEND)), no ONNX export found, no Rust binding. Zero production readiness today — flag as a research direction, not an implementation option. Its linear-complexity claim is the one property Sortformer structurally lacks (Sortformer's self-attention is quadratic in context length, which is *why* it needs a bounded speaker cache rather than unbounded context — directly relevant to AudioGraph's long-meeting use case). |
| **DiariZen tutorial** — Raghav, BUT, [arXiv:2604.21507](https://arxiv.org/html/2604.21507) | 2026-04-23 | Not a new model — a block-by-block explainer of [BUTSpeechFIT/DiariZen](https://github.com/BUTSpeechFIT/DiariZen), described in-paper as "the leading open-source state of the art at the time of writing." Pipeline: pruned WavLM-Large (316M→63M params) → 4-layer Conformer + powerset classification → overlap-add segmentation aggregation → WeSpeaker ResNet34 embeddings (overlap-exclusion masked) → VBx (AHC+PLDA, then VB-HMM) clustering → RTTM. | Python/PyTorch only. See §4 for the license blocker. |
| **Benchmarking Diarization Models** — [arXiv:2509.26177](https://arxiv.org/html/2509.26177v1) | 2025-09-30 | Head-to-head DER across pyannote 3.1, pyannoteAI (commercial "precision-2"), Sortformer v1, Sortformer v2 (chunked), Sortformer v2-streaming, DiariZen, on 196.6h across 5 languages. See §5 for the numbers — this is the single most load-bearing evidence source for the recommendation below. | N/A (evaluation paper). |
| **DiarizationLM** — Google, [arXiv:2401.03506](https://arxiv.org/html/2401.03506v5) | 2024 | LLM-based post-processing of the ASR+diarization *orchestration* output (word-level speaker assignment) to fix speaker-confusion errors using semantic context the acoustic pipeline can't see (e.g. "Good morning Patrick... how are you Tom?" implies a speaker turn even if acoustic diarization missed it). Directly relevant to a future refine-tier: this is a plausible **third** tier beyond "live cheap" / "offline acoustic refine" — a semantic refine pass over AudioGraph's existing extraction LLM (LFM2-350M) infrastructure. | N/A — algorithmic pattern, not a model dependency. |
| **Probabilistic back-ends for online speaker recognition/clustering** — [arXiv:2302.09523](https://arxiv.org/abs/2302.09523) | 2023 | Bayesian/PLDA-style online clustering theory — relevant if AudioGraph pursues ADR-0017's rejected "Option C" (hand-rolled online clustering over a truly-streaming embedding extractor) rather than rolling-window offline reprocessing. | Academic reference only, no code artifact surfaced. |
| **Comparative SA-ASR study** (FD-SOT vs WD-SOT vs TS-ASR) — [arXiv:2203.16834](https://arxiv.org/html/2203.16834v3) | 2022 | Confirms **word-level** diarization (aligning at the word/token, not the segment) reduces speaker-attributed CER by 10.7% relative vs frame-level-then-align on AliMeeting; joint target-speaker separation+ASR (TS-ASR) does another 16.5% relative over that. Evidence that AudioGraph's overlap-at-transcript-segment approach, while correct in kind, will show a measurable gap against word-level alignment when transcript segments are long/multi-sentence. | N/A. |

**Contradicted/superseded claim check:** no paper found that disputes the AOSC streaming-cache mechanism's superiority over cache-free rolling-window re-diarization for long-form audio — this is corroborated independently by two sources (§5's AMI-SDM 56%-vs-26% DER gap, and LS-EEND's own framing of "the difficulty of resolving speaker permutation ambiguity between chunks gets higher... with the increase of audio length" for block-wise methods). This is a load-bearing, doubly-sourced finding for AudioGraph's rolling-window design (ADR-0017).

---

## 2. NVIDIA Sortformer — ONNX + license reality (the model AudioGraph's `diarization` feature already uses)

**Checkpoint lineage and licenses (verified against HuggingFace model cards + the NVIDIA Open Model License PDF):**

| Checkpoint | Type | License | Notes |
|---|---|---|---|
| `nvidia/diar_sortformer_4spk-v1` | Offline | Originally carried an NC (non-commercial) mention under CC-BY-4.0-NC-style framing (per [HF discussion #9](https://huggingface.co/nvidia/diar_sortformer_4spk-v1/discussions/9): "Is it planned to remove the Non Commercial Mention?") | NVIDIA's own reply: NC would be removed once the streaming model shipped. |
| `nvidia/diar_streaming_sortformer_4spk-v2` | Streaming (AOSC) | **CC-BY-4.0**, released 2025-08-29, explicitly "removing NC" per the same discussion thread | **This is the checkpoint AudioGraph already downloads** (`models/mod.rs` Sortformer URL, `altunenes/parakeet-rs` HF mirror). Commercially usable, attribution-only. |
| `nvidia/diar_streaming_sortformer_4spk-v2.1` | Streaming (AOSC), newer | **NVIDIA Open Model License Agreement** (different license string, `license: other` / `nvidia-open-model-license` in the HF card YAML) | Verified via the [full license PDF](https://www.nvidia.com/content/dam/en-zz/Solutions/license-agreements/enterprise-services/nvidia-open-model-agreement-2026-04-02.pdf): "Models are commercially usable... free to create and distribute Derivative Models... perpetual, worldwide, non-exclusive, no-charge, royalty-free... license," conditioned only on including a NOTICE-file attribution ("Licensed by NVIDIA Corporation under the NVIDIA Open Model License") when redistributing. **Practically equivalent to CC-BY-4.0 for AudioGraph's purposes** (commercial Windows `.exe` shipping is fine); the license *name* changed, not the practical permission. Official published DER: DIHARD III eval 1-4spk = 13.24%, 5-9spk = 42.56% (severe degradation past the 4-speaker cap, as architecturally expected), full = 18.91%; CALLHOME part2 3spk = 10.05% (0.25s collar), all at `input_buffer_length=1.04s`. |

**ONNX export is NOT an NVIDIA-supported path.** Two live GitHub issues on `NVIDIA-NeMo/NeMo` confirm this:
- [#15077](https://github.com/NVIDIA-NeMo/NeMo/issues/15077): ONNX export of `diar_streaming_sortformer_4spk-v2` fails on dynamic tensor slicing in `sortformer_modules.py` (`output[batch_idx, start:end] = emb[batch_idx, :length[batch_idx]]` — runtime-dependent slice bounds, incompatible with ONNX's static-shape requirement). A maintainer points to a workaround (`concat_and_pad()` instead of the scripted variant); the reporter (`altunenes`, the author of the `parakeet-rs` crate AudioGraph depends on) confirms this got the export within 0.001 numerical error and ported it to Rust.
- [#15536](https://github.com/NVIDIA-NeMo/NeMo/issues/15536): the same problem recurs for v2.1 (feature-dimension mismatch + a Reshape-node shape error on mixed-length speaker caches). NVIDIA's response: *"this configuration (CPU-only, macOS) isn't one we support or test against for Sortformer ONNX export, we can't prioritize a fix on our side."* The working fix (bypass `streaming_export()`, trace with batch_size=1 and zero-length caches, call `torch.onnx.export` directly with a custom `onnx_forward`) is a **community** script: [`altunenes/parakeet-rs/scripts/export_diar_sortformer.py`](https://github.com/altunenes/parakeet-rs/blob/master/scripts/export_diar_sortformer.py).

**Implication:** AudioGraph's Sortformer path is entirely downstream of one community maintainer's (unofficial) export pipeline for one specific crate. There is no NVIDIA-published ONNX artifact for the streaming model, no guarantee the export script tracks future checkpoints (v3+), and no alternative Rust crate wrapping NeMo's Sortformer today. This is a supply-chain risk worth naming explicitly in any v2 plan, not a reason to avoid the model — it is still the only architecturally-true streaming diarizer reachable from Rust.

**Streaming latency/config tradeoffs** (from FluidInference's Sortformer docs, a Swift/CoreML implementation — not Rust, but the only public source with concrete AOSC config numbers):

| Config | Chunk size | Latency | AMI-SDM DER |
|---|---|---|---|
| `default`/`fastV2_1` | 6 frames | ~1.04s | good |
| `balancedV2_1` | 6 frames | ~1.04s | **best, 20.6%** |
| `highContextV2_1` | 340 frames | ~30.4s | 31.7% |

Critically: on AMI-SDM, an **offline, cache-free windowed** re-diarization (each window diarized independently, no persistent speaker cache — i.e., architecturally identical to AudioGraph's current sherpa-clustering rolling-window approach) scores **~56% DER**, vs **~26% DER** for the streaming path *with* the AOSC speaker cache — "the gap is entirely speaker confusion the `spkcache` prevents." This is the single most important number for evaluating AudioGraph's own cross-window stabilization design (embedding-centroid matching in `stabilize.rs`): it is a weaker mechanism than a trained, in-model persistent speaker cache, and the gap is large, not marginal.

---

## 3. pyannote 3.x/4.x — gating, license, and the community-1 gap

- **`pyannote/speaker-diarization-3.1`** (legacy, what sherpa-onnx's bundled segmentation model is derived from): gated (HF token + accept conditions on two repos: `segmentation-3.0` and `speaker-diarization-3.1`). A live bug ([pyannote-audio#2044](https://github.com/pyannote/pyannote-audio/issues/2044)) shows 3.1 also silently requires accepting a **third**, undocumented gated repo (`speaker-diarization-community-1`) purely because `SpeakerDiarization.__init__` eagerly loads a PLDA it never uses under the 3.1 config's `AgglomerativeClustering` — a maintainer-acknowledged wart, not something AudioGraph needs to solve (sherpa-onnx's ONNX conversion of `segmentation-3.0` sidesteps the whole pyannote.audio Python runtime and its gating).
- **`pyannote/speaker-diarization-community-1`** (current pyannote OSS pipeline, released 2025-09-26): gated (contact-info-only, "will always remain freely accessible"), **CC-BY-4.0**. Per pyannote's own published benchmark table (below), materially better than 3.1 across nearly every dataset. It also ships a **new output mode, `exclusive_speaker_diarization`**, explicitly "backported from our latest commercial model" and designed to "simplify the reconciliation between fine-grained speaker diarization timestamps and (sometimes not so precise) transcription timestamps" — i.e., built for exactly the word-alignment problem AudioGraph has (§6).
- **`pyannote/precision-2`** (commercial, cloud-only via pyannoteAI): the accuracy ceiling in every benchmark found.

| Benchmark (2025-09) | legacy (3.1) | community-1 | precision-2 (commercial) |
|---|---|---|---|
| AMI(SDM) | 22.7 | 19.9 | 15.6 |
| DIHARD 3(full) | 21.4 | 20.2 | 14.7 |
| VoxConverse | 11.2 | 11.2 | 8.5 |
| CALLHOME(part2) | 28.5 | 26.7 | 16.6 |

**No ONNX or Rust port of `community-1` exists anywhere I could find** (searched HF, GitHub, sherpa-onnx's own conversion-script directory). sherpa-onnx's `sherpa-onnx-pyannote-segmentation-3-0.tar.bz2` is a hand-converted export of the **older** `pyannote/segmentation-3.0` model (conversion script at `k2-fsa/sherpa-onnx/scripts/pyannote/segmentation`), predating the `community-1` architectural improvements. **This means AudioGraph's `diarization-clustering` backend is running strictly weaker segmentation than current pyannote OSS SOTA, with no upgrade path short of either (a) waiting for k2-fsa to convert `community-1`, which has not happened, or (b) running the Python pyannote.audio runtime directly** (a much larger packaging change for a Rust/Tauri app — bundling a Python interpreter + PyTorch + model weights is a different category of cost than the current ONNX-only story, and out of scope for "Rust-native local serving").

---

## 4. DiariZen — the accuracy leader, blocked by license

[BUTSpeechFIT/DiariZen](https://github.com/BUTSpeechFIT/DiariZen) (code MIT, **model weights CC-BY-NC-4.0**). Per the maintainers' own `MODEL_LICENSE`: the NC restriction exists because training data (RAMC, MSDWild, DIHARD-3) is itself NC-licensed, and even the CC-BY-SA-licensed portion of the training mix (AISHELL-4, AliMeeting) can't be reconciled into a commercial-safe license once mixed with NC data — "we prioritize maximum compliance with the most restrictive source terms." This is an explicit, deliberate, non-negotiable restriction from the model authors, not an oversight.

**This rules DiariZen out for AudioGraph's shipped Windows `.exe`** despite it being the strongest open-source result in the field (§5) — a commercial product cannot embed CC-BY-NC-4.0 weights. It also has no ONNX export and no Rust binding; its architecture (pruned WavLM-Large 63M params → Conformer → VBx/PLDA clustering) is meaningfully heavier than sherpa-onnx's pyannote-segmentation-3.0 + lightweight embedding models, so even a hypothetical future ONNX port would be a much bigger CPU/GPU compute commitment than the current stack. **Do not plan around DiariZen.** Flag it only as the accuracy ceiling AudioGraph is *not* chasing, for expectation-setting.

---

## 5. Head-to-head DER numbers (Benchmarking Diarization Models, arXiv:2509.26177, 196.6h/5 languages)

| Model | Overall DER | Notes |
|---|---|---|
| pyannoteAI (commercial) | **11.2%** | Best across nearly every language/speaker-count cut; most stable at 5+ speakers (6.6% DER). |
| DiariZen | 13.3% | Best open-source; strongest at 5+ speakers (7.1% DER) — its hybrid EEND-front/clustering-back design scales past a hard speaker cap, unlike Sortformer. **License-blocked, see §4.** |
| Sortformer v2 / v2-streaming | comparable to DiariZen at ≤3 speakers; **degrades hard past 4** (13.2–21.3% DER at exactly 4 speakers depending on variant; architecturally missing/merging beyond 4) | **214.3x realtime factor** — by far the fastest model evaluated. This is AudioGraph's live-tier candidate's real tradeoff: speed and low latency, in exchange for a hard 4-speaker ceiling. |
| legacy pyannote 3.1 | worse than community-1/precision-2 across the board (e.g. 19.9% AMI-SDM community-1 vs 22.7% legacy) | This is what sherpa-onnx's bundled segmentation model is derived from — AudioGraph's clustering backend inherits this gap (§3). |

**Failure-mode finding, doubly relevant to AudioGraph:** the dominant error across *all* models is **missed speech** (avg. missed segment ≈350ms, from imprecise speech onset/offset detection), not speaker confusion — except for Sortformer specifically, which shows *elevated* speaker-confusion error relative to the other models (its 4-speaker architectural cap manifesting as confusion rather than missed speech). This means AudioGraph's own VAD/endpointing quality feeds directly into diarization quality regardless of which diarizer backend is chosen — a shared upstream dependency worth remembering when triaging diarization bugs (a "wrong speaker" symptom may actually be a missed-speech-boundary problem, not a clustering problem).

---

## 6. Speaker embedding model zoo reachable from sherpa-onnx (Rust)

All of these are pretrained ONNX files on `k2-fsa/sherpa-onnx`'s [`speaker-recongition-models`](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models) release (note: upstream tag literally has the "recongition" typo — confirmed still current), all loadable via the exact same `SpeakerEmbeddingExtractorConfig` API AudioGraph already uses:

| Model | Dim | Lang focus | Size | Notes |
|---|---|---|---|---|
| `nemo_en_titanet_small.onnx` | 192 | English | small | **What AudioGraph already ships.** |
| `3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx` | 512 | Chinese-leaning | 37.8 MB | Also usable cross-lingually (3D-Speaker/ModelScope). |
| `3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx` | — | **English (VoxCeleb)** | 28.2 MB | CAM++ architecture: per 3D-Speaker's own runtime benchmark, **CAM++ has the best RTF (0.049) of all their supported architectures** — faster than ERes2Net-Base (0.076) — at 7.18M params. Directly comparable size/speed class to TitaNet-small, English-native, and not currently evaluated by AudioGraph. Worth an A/B before committing further to TitaNet. |
| `wespeaker_en_voxceleb_CAM++.onnx` / `_LM` | — | English | 27.9 MB | WeSpeaker's own CAM++ export (used as the embedding stage in DiariZen itself, per its README — "WeSpeaker ResNet34 model with overlap exclusion masking" is the *reference* combination for DiariZen, and CAM++/ResNet are siblings in the same WeSpeaker family). |
| `wespeaker_en_voxceleb_resnet152/221/293_LM.onnx` | — | English | 75–110 MB | Larger, presumably higher-accuracy ResNet variants; not benchmarked against TitaNet/CAM++ by any source found — UNVERIFIED which wins on AudioGraph's actual meeting audio. |

**Recommendation for embedding choice:** given AudioGraph is English-primary and latency-sensitive, `wespeaker_en_voxceleb_CAM++_LM.onnx` or `3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx` are the strongest candidates to benchmark against the current TitaNet-small baseline — same integration surface (already-wired `SpeakerEmbeddingExtractor`), best published RTF in the family, English-native training data. This is a same-day swap-and-measure, not new integration work.

---

## 7. GPU/acceleration reality for the diarization-specific dependencies

This directly extends the fleet-wide finding in `docs/agentic-runs/2026-08-23-offline-stt-research/audit-current-stack.md` (§5 there): **the official `sherpa-onnx` Rust crate has no GPU feature at all, by explicit maintainer decision.** Confirmed via a live, still-open feature request ([k2-fsa/sherpa-onnx#3606](https://github.com/k2-fsa/sherpa-onnx/issues/3606), "provide backend selection for rust binding"): a user asks for `cuda`/`directml` Cargo features "similar to how... `ort` or `tch` handle backend selection"; the maintainer's reply (translated from Chinese): *"We only support CPU. If you want another backend, compile the corresponding library yourself — our crate lets you point at your own library location."* i.e. GPU acceleration for sherpa-onnx-based diarization today means self-compiling a GPU-enabled ONNX Runtime and wiring `SHERPA_ONNX_LIB_DIR` — a real build/packaging undertaking for a shipped Windows installer, not a Cargo feature flip.

**Concrete, previously-unflagged alternative: the third-party `sherpa-rs` crate** ([thewh1teagle/sherpa-rs](https://github.com/thewh1teagle/sherpa-rs), MIT, v0.6.8, ~82k downloads, explicitly tagged `diarization`/`embeddings` in its own keywords) **does expose `cuda` and `directml` Cargo features** (`sherpa-rs = { version = "0.6.8", features = ["cuda"] }` or `["directml"]`, confirmed on crates.io's feature listing), delegating to `sherpa-rs-sys`'s equivalent features. Its README explicitly lists "Speaker embedding (labeling)" and "Speaker diarization" among its supported functions, on Windows/Linux/macOS/Android/iOS. **This is worth a spike**, not a switch-on-faith: verify (a) whether `sherpa-rs`'s diarization API surface actually matches `OfflineSpeakerDiarization`'s config shape (pyannote segmentation + embedding + `FastClusteringConfig`) closely enough to be a drop-in for `clustering.rs`, and (b) whether enabling `cuda`/`directml` there pulls in the same ORT-link-conflict problem that already forces the `diarization` (parakeet-rs Sortformer) vs `diarization-clustering` (sherpa-onnx) mutual exclusion in `lib.rs` — plausible, since both still ultimately link an ONNX Runtime. If it does, GPU-accelerated clustering diarization and GPU-accelerated Sortformer streaming would *still* be mutually exclusive at build time even after this swap, just with GPU on either side rather than CPU-only.

For the segmentation and embedding models specifically (pyannote-seg-3.0, TitaNet, CAM++, etc.) — these are small (tens of MB) two-stage models compared to Whisper/WavLM-scale encoders, so CPU inference cost is lower-stakes than for ASR; the acceleration story matters more for latency (keeping the rolling-window re-diarization pass fast enough not to visibly lag) than for raw feasibility. This is the one place in the whole offline-STT-v2 surface where "ship CPU-only for now, GPU later" is a genuinely reasonable interim call rather than a compromise — unlike Whisper/streaming-ASR, where CPU-only is the actual latency blocker.

---

## 8. Word-level alignment with ASR timestamps

The industry-standard technique, confirmed independently by three sources — pyannoteAI's own tutorial code, NVIDIA NeMo's `OfflineDiarWithASR.get_transcript_with_speaker_labels`, and Google's DiarizationLM paper's "orchestration module" — is identical in shape:

1. For each ASR word/segment, compute temporal overlap against every diarization span.
2. Assign the speaker with **greatest aggregate overlap** (not first-match — matters when a speaker's speech is split across multiple diarization spans intersecting one word/segment).
3. If no overlap exists, fall back to the **temporally nearest** span (midpoint-distance in the pyannoteAI reference implementation).

**AudioGraph's existing `overlap_speaker_for_segment`** (`src-tauri/src/diarization/mod.rs`) already implements step 1–2 correctly — including the "aggregate per speaker across multiple disjoint spans" refinement that the naive/common implementations (e.g. the AssemblyAI blog's example, the pyannoteAI tutorial's own snippet) do *not* do (they pick the single largest-overlap span, not the summed-per-speaker overlap). AudioGraph's implementation is more correct than the two publicly-published reference implementations found in this sweep. It does **not** implement step 3 (nearest-fallback when no span overlaps) — `overlap_speaker_for_segment` returns `None` and the caller apparently leaves the segment unlabeled. This is a small, well-scoped gap, not an architecture problem.

**The metric gap, not the alignment gap, is what matters most:** AssemblyAI's public comparison (`assemblyai.com/blog/whisper-speaker-diarization`, 2026-08-12) makes a concrete point that generalizes to any pipeline, including AudioGraph's: **DER can score 15.1% ("good") on an output that scores 30.7% on cpWER ("nearly a third of words misattributed")**, because DER is a time-weighted metric that structurally underweights short turns, backchannels ("Right," "No, wait"), and overlap-collapse — exactly the failure modes real meetings produce most often. AudioGraph's own stated "one remaining accuracy gate" (`diarizes_a_clip_into_speaker_segments` asserting only `num_speakers >= 1`) is not just under-specified on speaker *count* (as ADR-0017 already flags) — it is measuring the wrong axis entirely for what a knowledge-graph consumer cares about (attribution correctness of *specific words/claims* to a speaker, not aggregate time-weighted correctness). **A cpWER-style eval — concatenate each speaker's attributed words, compare against ground truth per-speaker under best-permutation matching — would be a materially better accuracy gate than a speaker-count assertion**, and does not require new infrastructure beyond a labeled test clip with per-word ground truth (which the ADR-0017 "curated/labeled multi-speaker clip" data-collection task already needs to produce anyway).

**pyannote community-1's `exclusive_speaker_diarization` mode** (§3) is purpose-built for this exact reconciliation problem and is worth remembering if AudioGraph ever gains any path to running pyannote.audio directly (unlikely near-term given the all-Rust posture, but relevant if a future "cloud/local hybrid refine" tier ever shells out to Python) — but it changes nothing for the ONNX/sherpa-onnx path, since sherpa-onnx's own `OfflineSpeakerDiarization` result already returns non-overlapping per-speaker segments by construction (clustering assigns one speaker per segment, not multi-label), so AudioGraph gets a comparable "exclusivity" property today by default, without needing the pyannote feature.

---

## 9. Implications for AudioGraph — recommended stack per tier

Given the constraints already established by the repo audit (`audit-current-stack.md`): no plugin trait exists, models are never bundled, the two diarization neural backends are mutually exclusive at build time (ORT link conflict), and the product already has a rolling-window / post-session-refine split baked into its architecture (ADR-0017) and roadmap (the "speaker identity ladder" epic referenced in this task — **not found anywhere in the current repo** via grep for "speaker identity ladder"/"identity ladder"/"explicit cue"; treat as forthcoming/external context not yet landed, not a contradiction of anything above).

**Tier 1 — live, cheap labels (streaming):**
- **Keep Sortformer-streaming (`diar_streaming_sortformer_4spk-v2`, CC-BY-4.0) as the primary live-tier engine.** It is the only genuinely end-to-end-streaming, architecturally-true option reachable from Rust today (LS-EEND and the raw AOSC mechanism have no Rust path; sherpa-onnx's diarizer is offline-only by design). Its 4-speaker cap is a real, known limitation (most acute past 4 speakers per §5's DER table) — acceptable for most 1:1/small-group meetings, a known degradation mode to surface in the UI for larger calls (tie into the readiness-signal gap the audit already flagged: `PipelineStatus` has no `Degraded` state to report "diarization capped at 4 speakers" today).
- **Do not build LS-EEND or a hand-rolled EEND from scratch** — zero Rust/ONNX ecosystem support, and the effort to get from "PyTorch research checkpoint" to "shipped Rust inference" for a from-scratch architecture is an order of magnitude larger than porting Sortformer already was (and that port took a dedicated external maintainer real effort against NVIDIA's own unsupported-export stance).
- **Consider Option C from ADR-0017 (hand-rolled online clustering over sherpa-onnx's already-streaming `SpeakerEmbeddingExtractor`) as a second live-tier path for the unbounded-speaker-count case**, gated behind whichever build excludes Sortformer. sherpa-onnx's embedding extractor genuinely is stream-based (`create_stream`/`accept_waveform`/`is_ready`/`compute`, already used in `worker.rs` for per-cluster embedding) — what's offline is the *segmentation+clustering combo* (`OfflineSpeakerDiarization`), not embedding extraction itself. A short, incremental VAD-gated window feeding the streaming embedder + a simple leader/threshold online clusterer (the "Probabilistic back-ends for online speaker recognition" line of work, §1) is a real, previously-underweighted third path beyond "Sortformer" and "sherpa offline rolling-window" — more work than either shipped option, but it is the one path with genuinely unbounded speaker count *and* true incrementality (no rolling-window latency), which the maintainer's stated preference against "smallest available subset" argues for evaluating rather than dismissing outright as ADR-0017 did ("more work, more risk" — true, but not evaluated against the actual product requirement of unbounded+streaming simultaneously, which neither shipped backend delivers).

**Tier 2 — post-session refine (offline, higher accuracy):**
- **Keep sherpa-onnx `OfflineSpeakerDiarization` (pyannote-segmentation-3.0 + embedding) as the refine-tier engine**, but budget a same-day embedding swap-and-measure (TitaNet-small → CAM++ EN VoxCeleb ONNX, §6) before assuming the current embedding choice is optimal — same API, likely faster (best published RTF in the family), English-native.
- **Do not chase pyannote `community-1` or DiariZen for this tier.** `community-1` has no ONNX/Rust path (would require bundling the Python pyannote.audio runtime — a categorically different packaging commitment than "Rust-native local model serving," and out of scope for this planning ask). DiariZen is license-blocked for commercial shipping (CC-BY-NC-4.0 weights) regardless of its accuracy lead. Both are worth re-checking on a ~6-12 month cadence (`community-1`'s architecture is new as of Sept 2025; an ONNX port is plausible but unannounced) rather than building around today.
- **GPU acceleration for this tier is a "later" item, not a blocker** (§7) — the segmentation/embedding models are small enough that CPU cost is a latency-of-the-refine-pass concern, not a feasibility concern, unlike streaming ASR. If pursued, spike `sherpa-rs`'s `cuda`/`directml` features first (§7) before attempting a custom ORT build — it's a crates.io-published, currently-maintained option nobody in the existing ADR-0017 research considered, and may collapse the "self-compile ORT" cost to "flip a Cargo feature" if its diarization API surface is close enough to `sherpa-onnx`'s.

**Cross-cutting:**
- **Word-level alignment is already correctly implemented** (`overlap_speaker_for_segment`); the one gap (nearest-fallback when no overlap exists) is small and well-scoped, not an architecture change.
- **Replace/augment the DER-only accuracy gate with a cpWER-style check** on the same curated multi-speaker clip ADR-0017 already needs for its unbounded-speaker-count assertion — this is the single highest-leverage, lowest-cost improvement available across both tiers, since it's a test methodology change, not a model/architecture change, and it directly measures what the product actually needs (correct speaker attribution per word feeding the knowledge graph), not a proxy (time-weighted correctness) that the literature (§8) shows can diverge sharply from it.
