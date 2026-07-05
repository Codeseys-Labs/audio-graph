# Turning a Raw Real-Time STT Stream into Structured, LLM-Ready Units

Research note. Date: 2026-07-04. Scope: (1) streaming speaker diarization, (2) end-of-turn / endpointing / turn detection, (3) sentence/utterance segmentation + micro-batching of STT output. Includes an adversarial/limitations section. Citations carry title + year + venue/URL.

## Executive summary

A raw streaming STT engine emits words (or partial hypotheses) as a low-latency dribble with no speaker labels, no sentence boundaries, and no notion of "the speaker is done." Three orthogonal subsystems convert that dribble into coherent units an LLM can consume:

1. **Streaming diarization** answers "who spoke when" incrementally. State of the art is streaming EEND-family models (LS-EEND, Streaming Sortformer) that handle a flexible/unknown speaker count with a linear-time online buffer/cache; latency trades directly against DER.
2. **Endpointing / turn detection** answers "has this speaker finished their turn." The field has moved from acoustic VAD (silence timers) to *semantic/acoustic-semantic* turn detectors and *proactive* endpoint anticipation. Deepgram Flux, LiveKit's turn detector, and pipecat's Smart Turn are the three dominant production systems, each exposing an explicit turn state machine.
3. **Segmentation + punctuation + micro-batching** groups the word stream into punctuated sentences/utterances so downstream consumers (MT, LLMs) get coherent units, not fragments. Streaming re-punctuation with dynamic decoding windows is the key technique.

The recurring, unavoidable design tension across all three is **latency vs. accuracy** (for turns: **responsiveness vs. robustness**): committing early cuts latency but causes premature cutoffs / speaker-label errors; waiting longer improves accuracy but hurts interactivity.

---

## 1. Streaming / online speaker diarization

### Taxonomy and problem framing
- **"A Review of Common Online Speaker Diarization Methods" (2024, arXiv:2406.14464, Aperdannier et al.)** — online diarization = producing the speaker label "immediately after the arrival of an audio segment" (low latency), vs. offline which sees the whole file. Gives history, taxonomy (clustering-based online vs. end-to-end online), datasets (CALLHOME, DIHARD, AMI, VoxConverse), and open challenges. Good orientation source. https://arxiv.org/abs/2406.14464

### The two lineages

**(a) Clustering-based online diarization.** Segment → speaker embedding (x-vector / d-vector / pyannote embedding) → incremental online clustering. Represented by DIART (online clustering over pyannote segmentation+embedding), UIS-RNN / UIS-RNN-SML (a supervised, RNN-based online clusterer), and VBx (Bayesian HMM over x-vectors, offline baseline).
- **"Bayesian HMM clustering of x-vector sequences (VBx)" (2020, arXiv:2012.14952)** — the canonical x-vector + VBx pipeline; strong offline baseline that online systems are measured against. https://arxiv.org/abs/2012.14952
- **"Online Target Speaker Voice Activity Detection for Speaker Diarization" (2022, Interspeech, arXiv:2207.05920)** — online TS-VAD variant. https://arxiv.org/abs/2207.05920

**(b) End-to-end neural diarization (EEND).** Reformulates diarization as per-frame multi-label classification, natively handling **overlapping speech** (the weakness of clustering methods).
- **"End-to-End Neural Diarization: Reformulating Speaker Diarization as Simple Multi-label Classification" (2020, arXiv:2003.02966, Fujita et al.)** — the EEND framing. https://arxiv.org/abs/2003.02966
- **EEND-EDA: "End-to-End Speaker Diarization for an Unknown Number of Speakers with Encoder-Decoder Based Attractors" (2020, Interspeech; journal ver. arXiv:2005.09921, Horiguchi et al.)** — the key idea for **flexible speaker counts**: an LSTM encoder-decoder generates a variable number of *attractors* (one per speaker) from the frame embeddings; an attractor-existence probability decides how many speakers there are. This is why EEND-EDA handles an unknown/variable number of speakers rather than a fixed output count. https://arxiv.org/abs/2005.09921
- Attractor variants: **DiaPer (Perceiver-based attractors, 2023, arXiv:2312.04324)**, **Transformer Attractors (2023, arXiv:2312.06253)**, and analysis "Do EEND Attractors Need to Encode Speaker Characteristic Information?" (2024, arXiv:2402.19325). https://arxiv.org/abs/2312.04324

### Making EEND streaming (the core of "online diarization" today)
Vanilla EEND is offline (needs the whole chunk and produces a permutation-free assignment per chunk). Two mechanisms make it streaming while keeping speaker identity consistent across chunks:

- **Speaker-Tracing Buffer (STB): "Online End-to-End Neural Diarization with Speaker-Tracing Buffer" (2020, arXiv:2006.02616, Xue et al.)** — solves the cross-chunk **speaker-permutation** problem: a buffer retains a subset of past frames + their labels so each new chunk's speakers are aligned to previously seen speakers. Foundational for online EEND. https://arxiv.org/abs/2006.02616
- **FS-EEND: "Frame-wise streaming end-to-end speaker diarization with non-autoregressive self-attention-based attractors" (2023, arXiv:2309.13916)** — frame-in/frame-out streaming with online attractors; used as a low-latency baseline in latency studies. https://arxiv.org/abs/2309.13916
- **LS-EEND: "Long-Form Streaming End-to-End Neural Diarization with Online Attractor Extraction" (2024, IEEE TASLP; arXiv:2410.06670, Liang & Li)** — current SOTA online DER. Causal embedding encoder + online attractor decoder, frame-in/frame-out. A **retention mechanism gives linear temporal complexity** for **long-form** (≈1 hour) audio and a **high/flexible speaker count (up to 8)**. Reports SOTA online DER: CALLHOME 12.11%, DIHARD II 27.58%, DIHARD III 19.61%, AMI 20.76%, at several-times-lower real-time-factor than prior online models. https://arxiv.org/abs/2410.06670
- **Streaming Sortformer: "Speaker Cache-Based Online Speaker Diarization with Arrival-Time Ordering" (2025, Interspeech; arXiv:2507.18446, Medennikov et al., NVIDIA)** — streaming extension of Sortformer. Key property: **arrival-time ordering** of output speakers (speaker index = order of first appearance), which makes labels stable/interpretable across the stream. Uses an **Arrival-Order Speaker Cache (AOSC)** storing frame-level embeddings; unlike a fixed STB it stores a *dynamic* number of frames per speaker, chosen by prediction score. Designed to work "even in low-latency setups" and as a foundation for streaming multi-talker ASR. https://arxiv.org/abs/2507.18446
- Also: **"A Reinforcement Learning Framework for Online Speaker Diarization" (2023, arXiv:2302.10924)** and **Mamba-based segmentation for diarization (2024, arXiv:2410.06459)** (linear-time state-space alternative to transformer segmentation). https://arxiv.org/abs/2302.10924

### Latency vs. accuracy trade-off (measured)
- **"Systematic Evaluation of Online Speaker Diarization Systems Regarding their Latency" (2024, arXiv:2407.04293, Aperdannier et al.)** — measures wall-clock latency (audio-in → speaker-label-out) on identical hardware/data. Compares DIART pipelines (pyannote embedding+segmentation lowest latency), a UIS-RNN-SML system, and FS-EEND (similarly low latency). Notes there was *no prior published cross-system online-diarization latency comparison* — the honest takeaway is that latency is under-reported in diarization papers, which mostly report DER on completed files. https://arxiv.org/abs/2407.04293
- **Mechanism of the trade-off:** online EEND/cache systems can widen the lookahead/chunk or grow the tracing buffer/cache to improve DER (especially on overlap and speaker-permutation), at the cost of higher latency; shrinking them cuts latency but raises DER and permutation errors. LS-EEND's linear-complexity retention and Sortformer's dynamic AOSC are both attempts to relax this trade-off (bounded memory, sustained low RTF over long audio).

---

## 2. End-of-turn / endpointing / turn detection

### VAD endpointing vs. semantic endpointing — the core distinction
- **VAD/acoustic endpointing**: declare end-of-turn after N ms of silence (energy/Silero VAD + silence timer). Simple, language-agnostic, but *has no language understanding* → it is "trigger-happy": mid-turn pauses ("I want to go to… uh…") get misread as end-of-turn, causing the agent to interrupt; conversely long trailing silence adds latency. This failure mode is stated explicitly by the LiveKit and pipecat/Speechmatics sources below.
- **Semantic endpointing**: use the *content* (transcript and/or acoustics) to decide whether the utterance is syntactically/pragmatically complete. Two flavors: **text-based** (LLM/transformer over the running transcript) and **audio/acoustic-semantic** (encode the waveform directly, capturing prosody + un-transcribed hesitation).

Academic anchors:
- **"Improving End-of-turn Detection in Spoken Dialogues by Detecting Speaker Intentions as a Secondary Task" (2018, arXiv:1805.06511)** — early neural EOT via intent multi-tasking. https://arxiv.org/abs/1805.06511
- **"Two-pass Endpoint Detection for Speech Recognition" (2024, arXiv:2401.08916)** and **"Adaptive Endpointing with Deep Contextual Multi-armed Bandits" (2023, arXiv:2303.13407)** — ASR-side endpointing that adapts the silence threshold. https://arxiv.org/abs/2303.13407
- **"Streaming Endpointer for Spoken Dialogue using Neural Audio Codecs and Label-Delayed Training" (2025, arXiv:2506.07081)** — streaming endpointer over neural-codec tokens; label-delayed training to trade a little lookahead for accuracy. https://arxiv.org/abs/2506.07081

### Voice Activity Projection (VAP) — the dominant academic turn-taking model family
- **"Voice Activity Projection: Self-supervised Learning of Turn-taking Events" (2022, arXiv:2205.09812, Ekstedt & Skantze)** — the foundational VAP model: self-supervised, directly maps dialogue stereo audio to a *projection window* of future voice activity, from which turn-shift / hold / backchannel / overlap events are derived. Predicts the future rather than reacting to silence. https://arxiv.org/abs/2205.09812
- **"Real-time and Continuous Turn-taking Prediction Using Voice Activity Projection" (2024, IWSDS; arXiv:2401.04868, Inoue et al.)** — real-time VAP (CPC + self-attention + cross-attention transformer) that **runs continuously on CPU** with minimal degradation; studies the effect of input context length. This is the practical, deployable VAP. https://arxiv.org/abs/2401.04868
- **"How Much Does Prosody Help Turn-taking? Investigations using Voice Activity Projection Models" (2022, arXiv:2209.05161)** — ablates prosody's contribution to turn prediction. https://arxiv.org/abs/2209.05161
- **"Multilingual Turn-taking Prediction Using Voice Activity Projection" (2024, arXiv:2403.06487)** — multilingual VAP. https://arxiv.org/abs/2403.06487
- **"Prompt-Guided Turn-Taking Prediction" (2025, SIGDIAL; arXiv:2506.21191, Inoue et al.)** — VAP whose timing behavior is controllable via *textual prompts* ("faster"/"calmer"); trained on 950+ h of dialogue with LLM-generated synthetic prompts. Lets a system tune its turn aggressiveness per context. https://arxiv.org/abs/2506.21191
- **"Lla-VAP: LSTM Ensemble of Llama and VAP for Turn-Taking Prediction" (2024, arXiv:2412.18061)** — fuses an LLM's semantic signal with acoustic VAP. https://arxiv.org/abs/2412.18061
- Multi-party: **"Triadic Multi-party VAP for Turn-taking in Spoken Dialogue Systems" (2025, arXiv:2507.07518)** and **"Adaptive Turn-Taking for Real-time Multi-Party Voice Agents" (2026, arXiv:2606.13544)**. https://arxiv.org/abs/2507.07518

### LLM-based / proactive turn detection
- **"Endpoint Anticipation for Low-Latency Spoken Dialogue" (2026, arXiv:2606.13450, Udupa, Watanabe, Schwarz, Cernocky)** — shifts from *reactive* end-of-turn *detection* to *proactive forecasting*: a speech model anticipates the endpoint **up to 2.56 s in advance**, enabling **speculative execution** of the LLM+TTS pipeline on partial context. Introduces metrics for the trade-off between realized latency reduction and *computational redundancy* (wasted speculative work). This is the same idea Deepgram Flux exposes as "eager end-of-turn." https://arxiv.org/abs/2606.13450

### Production turn-detection systems (fetched docs)

**Deepgram Flux** ("Turn-based Audio," `/v2/listen`, model `flux-general-en` / `flux-general-multi`). Source: Deepgram Docs — Flux quickstart, state machine, and listen-flux reference (fetched 2026-07-04). https://developers.deepgram.com/reference/speech-to-text/listen-flux ; https://developers.deepgram.com/docs/flux/state
- **Model-integrated end-of-turn detection** — turn detection is *inside* the STT model rather than a bolted-on VAD, marketed as "first-of-its-kind," with "Nova-3 level accuracy."
- **Turn state machine / events**: `StartOfTurn`, `Update` (~every 0.25 s of transcribed audio), `EagerEndOfTurn`, `TurnResumed`, `EndOfTurn`. A `turn_index` increments after each `EndOfTurn`. `StartOfTurn` (guaranteed non-empty transcript) is the recommended **barge-in** trigger — more reliable than an external VAD.
- **Eager EOT / speculation**: setting `eager_eot_threshold` (0.3–0.9) makes Flux emit `EagerEndOfTurn` early so you can *start* the LLM response; if the user keeps talking, a `TurnResumed` fires and you cancel. The final `EndOfTurn` transcript is guaranteed to match the immediately preceding `EagerEndOfTurn`. This is the productized version of endpoint anticipation / speculative execution. Constraint: `eager_eot_threshold ≤ eot_threshold`.
- **Tunable thresholds (the latency↔accuracy knobs):** `eot_threshold` (0.5–0.9, default 0.7 — higher = more reliable but more latency), `eot_timeout_ms` (500–10000, default 5000 — max silence before *forcing* an EndOfTurn regardless of confidence, timer resets on new speech), `eager_eot_threshold` (lower = earlier eager triggers but more false starts). Recommends **80 ms audio chunks**.

**LiveKit turn detector** (livekit-plugins-turn-detector). Source: LiveKit Docs — turn-detector (fetched 2026-07-04). https://docs.livekit.io/agents/logic/turns/turn-detector/
- Adds a learned **end-of-turn signal on top of VAD** (VAD still required). Two model types historically: a **text/transformer EOU model** over the running transcript, and a newer **audio TurnDetector** that "encodes user audio directly, capturing both" acoustic + semantic cues. The docs explicitly note the **text model is fooled by mid-turn pauses and commits the turn early, while the audio model waits for the true end of turn.**
- **Multilingual open-weight model** (`livekit/turn-detector` on HF): ~14 languages, **~400 MB RAM, ~25 ms inference**, designed to run **on CPU** on the same server as the agent.
- **Endpointing defaults**: without the audio detector, `min_delay` 0.5 s / `max_delay` 3.0 s; **with** the audio turn detector these drop to `min_delay` 0.3 s / `max_delay` 2.5 s. `unlikely_threshold` (per-language calibrated) controls how confident the model must be before ending a turn (lower = ends sooner). Requires VAD `min_silence_duration ≥ 0.25 s` (Silero's 0.55 s default is fine). Model versions v1 (full, cloud) → falls back to **v1-mini** if the full model is unavailable / times out (~1 s).

**pipecat Smart Turn (open source, BSD-2)**. Source: Daily/pipecat blog "Smart Turn v2" (2025-07-18) + HF model cards + pipecat-ai/smart-turn GitHub. https://www.daily.co/blog/smart-turn-v2-faster-inference-and-13-new-languages-for-voice-ai/ ; https://huggingface.co/pipecat-ai/smart-turn-v2
- An **open-source semantic VAD** / native-audio turn detector: analyzes the **raw waveform** (not the transcript), so it catches filler words ("um", "えーと") and intonation that transcript-only detectors miss. Meant to be used **alongside** a traditional VAD.
- **v2**: wav2vec2 encoder + shallow linear head (94.8M params), **~360 MB** (6× smaller than v1's 2.3 GB), **~12 ms** inference for 8 s audio on an L40S, **14 languages**, ~99% on in-house `human_5_all` English test. **v3**: Whisper-Tiny backbone, ~8M params, int8/fp32, ~65 ms on Pipecat Cloud 1× instance. Fully open (weights + training script + datasets); `LocalSmartTurnAnalyzerV2/V3` in pipecat, or Fal-hosted.

**Common architectural pattern**: a turn model that fuses acoustic + (optionally) semantic cues, gated by a VAD, exposing (a) a confidence threshold to tune responsiveness vs. false-cutoffs and (b) an *eager/early* path for speculative LLM execution, plus a hard silence timeout as a floor. The `<500 MB` CPU-runnable footprint applies to the two **self-hostable local** models (pipecat Smart Turn v2, LiveKit turn-detector); Deepgram Flux is a **cloud** model-integrated turn detector with no local footprint — it shares the confidence-threshold + eager/early pattern but runs provider-side.

---

## 3. Sentence / utterance segmentation & micro-batching of STT output

### The problem
Streaming ASR decoders emit words/partials with no reliable sentence boundaries or punctuation; naïve per-decoder-segment punctuation *over-segments* (breaks sentences at pauses) or *under-segments* (runs sentences together), which degrades any downstream consumer that expects sentence units (MT, LLM prompting, summarization). Fixing this needs bidirectional context, which fights the real-time constraint.

- **Survey: "Capitalization and Punctuation Restoration: a Survey" (2021, arXiv:2111.10746)** — the landscape (sequence-tagging vs. seq2seq, restoring case+punct as the step that makes ASR output readable/segmentable). https://arxiv.org/abs/2111.10746

### Streaming re-punctuation with dynamic windows (the key technique)
- **"Streaming Punctuation: A Novel Punctuation Technique Leveraging Bidirectional Context for Continuous Speech Recognition" (2023, IJNLC; arXiv:2301.03819, Behre et al., Microsoft)** — punctuates/**re-punctuates** ASR output using **dynamic decoding windows** that reach across ASR-decoder segment boundaries, getting the bidirectional context a real-time system normally can't. Directly targets **over-segmentation**: **+13.9% segmentation F0.5**, and **+0.66 BLEU on downstream Machine Translation** — i.e., better segmentation measurably improves the downstream LLM/MT consumer. https://arxiv.org/abs/2301.03819
- Companion long-form version: **"Streaming Punctuation for Long-form Dictation with Transformers" (2022, arXiv:2210.05756)**. https://arxiv.org/abs/2210.05756
- **On-device / low latency: "A light-weight and efficient punctuation and word casing prediction model for on-device streaming ASR" (2024, arXiv:2407.13142)** — tiny joint punct+casing model for on-device streaming. https://arxiv.org/abs/2407.13142
- **"Efficient Punctuation Restoration via Weighted Lookahead Scoring Method for Streaming ASR Systems" (2026, arXiv:2606.05179)** — lookahead scoring to buy bidirectional context cheaply in streaming. https://arxiv.org/abs/2606.05179

### Punctuation-agnostic / robust segmentation (SaT / WtP)
- **"Where's the Point? Self-Supervised Multilingual Punctuation-Agnostic Sentence Segmentation" (2023, ACL; arXiv:2305.18893, Frohmann et al.)** — the WtP / "Segment any Text" (SaT) line: segments sentences **even when punctuation is absent or wrong** (exactly the ASR case), multilingual, self-supervised. The practical tool for turning unpunctuated ASR word streams into sentence units. https://arxiv.org/abs/2305.18893
- Classic rule-based baseline: **"PySBD: Pragmatic Sentence Boundary Disambiguation" (2020, arXiv:2010.09657)** — assumes clean punctuation, so it is fragile on raw ASR. https://arxiv.org/abs/2010.09657

### Micro-batching for LLM consumption — how the pieces compose
The three subsystems are combined into a "unitization" stage that emits coherent chunks:
- **Turn events as the natural batch boundary.** In agent stacks (Flux/LiveKit/pipecat), the `EndOfTurn` (or eager EOT + confirm) event is the trigger that flushes the accumulated words as one utterance to the LLM. Flux's `turn_index`, LiveKit's committed turn, and Smart Turn's endpoint decision each define one LLM input unit. Eager EOT additionally enables *speculative* early flushing.
- **Sentence boundaries as sub-turn units.** For long turns / dictation / captioning, streaming re-punctuation (Streaming Punctuation, SaT) segments a turn into sentences so downstream MT/LLM gets sentence-sized units rather than a whole monologue or word fragments. Behre et al.'s BLEU gain quantifies why this matters for the downstream model.
- **Diarization labels as unit metadata.** Streaming diarization (LS-EEND / Sortformer) attaches a speaker tag to each unit; Sortformer's arrival-time ordering gives stable Speaker-1/Speaker-2 labels for the LLM prompt. Word-level EEND (arXiv:2309.08489) aims to align speaker labels to words directly, tightening ASR+diarization fusion.
- **Result:** the LLM-ready unit is roughly `{speaker_label, punctuated_sentence(s), turn_index, timestamps, is_final|is_eager}` — assembled by micro-batching the word stream at semantic (turn / sentence) rather than acoustic (silence) boundaries.

---

## 4. Adversarial / limitations

**Turn detection — the responsiveness vs. robustness trade-off is fundamental, not solved.**
- **"Semantic-Aware Interruption Detection in Spoken Dialogue Systems: Benchmark, Metric, and Model" (2026, arXiv:2603.24144, Xia et al.)** — states the field is *polarized* between "trigger-happy VAD-based methods that misinterpret **backchannels**" and "robust end-to-end models that exhibit unacceptable response delays." Introduces **SID-Bench** (first real-world interruption benchmark) and the **Average Penalty Time (APT)** metric, which penalizes *both* false alarms and late responses — an explicit acknowledgment that neither latency nor accuracy alone is a valid target. https://arxiv.org/abs/2603.24144
- **"FastTurn: Unifying Acoustic and Streaming Semantic Cues for Low-Latency and Robust Turn Detection" (2026, arXiv:2604.01897)** — critiques existing systems by name: transcript/ASR-based detectors (e.g., "Ten Turn", "Easy Turn") **add latency and degrade under overlapping speech and noise**; **Smart Turn's simple linear head is less effective in complex conversational scenarios**. Also flags a **data problem**: open dialogue corpora "rarely capture realistic interaction dynamics" (overlap, backchannels, noise), so offline benchmarks mismatch real deployment. https://arxiv.org/abs/2604.01897
- **"LiveTurn: A Real-Time Turn Detection System for Voice Agents" (2026, OpenReview)** — even a SOTA acoustic-semantic detector reports a **median EOT latency of ~640 ms** and a **~6% False Start-of-Turn rate under noise** — i.e., false barge-ins remain common in the wild. https://openreview.net/forum?id=JIaOGuEMET
- **Practitioner limitation (the latency floor):** Gradium, "Turn-Taking in Voice Agents: Why Rule-Based VAD Is Broken and What Comes Next" (2026-05-27) — **"Semantic VAD reduces the end-of-turn detection *error rate* … it does not reduce the pipeline's fundamental *latency floor*."** Smarter turn detection fixes *when* you decide, not the STT→LLM→TTS round-trip cost. https://gradium.ai/content/turn-taking-voice-agents-vad
- Speculative/eager EOT (Flux eager, Endpoint Anticipation) buys latency back but at the cost of **computational redundancy** — wasted LLM/TTS work on turns that resume, a trade-off the anticipation paper explicitly meters (arXiv:2606.13450).

**Diarization limitations.**
- **DER is dominated by overlap and speaker confusion.** Clustering-based online systems handle overlap poorly; EEND handles overlap but online EEND still trails offline DER, and streaming buffers/caches (STB/AOSC) can drop or confuse speakers when the buffer is small (the low-latency setting). LS-EEND's own SOTA online DER on DIHARD II is still **27.58%** — high, showing hard conversational audio is far from solved.
- **Latency is under-reported.** arXiv:2407.04293 notes there was no prior cross-system online-diarization latency comparison; most diarization papers report DER on completed audio and hide the online-latency cost.
- **Fixed vs. flexible speaker count.** EEND-EDA/LS-EEND handle flexible counts but with practical caps (LS-EEND up to 8); very-many-speaker or rapidly-changing rosters remain hard, and joining a new speaker mid-stream (cold start into the attractor set / cache) is a known weak point.
- **Balanced metrics.** "BER: Balanced Error Rate For Speaker Diarization" (2022, arXiv:2211.04304) argues DER over-weights talkative speakers — the standard metric itself can mislead. https://arxiv.org/abs/2211.04304

**Segmentation limitations.**
- Streaming punctuation needs bidirectional/lookahead context, so there is an intrinsic **accuracy vs. latency** trade for punctuation too (weighted-lookahead, arXiv:2606.05179, is a mitigation). Rule-based SBD (PySBD) collapses on unpunctuated ASR output, and even learned re-punctuation still fights over/under-segmentation on slow speakers and irregular pauses (the exact scenario Behre et al. target).

---

## Design takeaways for an STT→LLM structuring layer

1. **Unitize at semantic boundaries, not silence.** Use a turn detector (model-integrated like Flux, or a plug-in like LiveKit/Smart Turn) to define LLM input units; keep a VAD only as a gate + hard timeout floor.
2. **Expose a single responsiveness knob** (eot_threshold / unlikely_threshold) and a hard silence timeout; default conservative, let the app tune. Add an **eager/speculative** path if you can afford (and cancel) redundant LLM work.
3. **Re-punctuate the word stream with a dynamic/lookahead window** (Streaming Punctuation / SaT) to split long turns into sentence units — this directly improves downstream MT/LLM quality (+0.66 BLEU shown).
4. **Attach streaming diarization labels** (LS-EEND or Streaming Sortformer for stable arrival-ordered speaker IDs) as unit metadata; budget the buffer/cache for your latency target and expect higher DER at lower latency.
5. **Measure with trade-off-aware metrics** (APT for turns, and report online latency alongside DER for diarization) — single-number accuracy hides the real behavior.

## Source index (title — year — venue/URL)
- A Review of Common Online Speaker Diarization Methods — 2024 — arXiv:2406.14464
- End-to-End Speaker Diarization for an Unknown Number of Speakers with Encoder-Decoder Based Attractors (EEND-EDA) — 2020 — Interspeech / arXiv:2005.09921
- End-to-End Neural Diarization: Reformulating … Multi-label Classification — 2020 — arXiv:2003.02966
- DiaPer: EEND with Perceiver-Based Attractors — 2023 — arXiv:2312.04324
- Online End-to-End Neural Diarization with Speaker-Tracing Buffer — 2020 — arXiv:2006.02616
- Frame-wise streaming EEND (FS-EEND) — 2023 — arXiv:2309.13916
- LS-EEND: Long-Form Streaming EEND with Online Attractor Extraction — 2024 — IEEE TASLP / arXiv:2410.06670
- Streaming Sortformer: Speaker Cache-Based Online Diarization with Arrival-Time Ordering — 2025 — Interspeech / arXiv:2507.18446
- Bayesian HMM clustering of x-vector sequences (VBx) — 2020 — arXiv:2012.14952
- Systematic Evaluation of Online Speaker Diarization Systems Regarding their Latency — 2024 — arXiv:2407.04293
- BER: Balanced Error Rate For Speaker Diarization — 2022 — arXiv:2211.04304
- Voice Activity Projection: Self-supervised Learning of Turn-taking Events — 2022 — arXiv:2205.09812
- Real-time and Continuous Turn-taking Prediction Using VAP — 2024 — IWSDS / arXiv:2401.04868
- Prompt-Guided Turn-Taking Prediction — 2025 — SIGDIAL / arXiv:2506.21191
- Lla-VAP: LSTM Ensemble of Llama and VAP — 2024 — arXiv:2412.18061
- Endpoint Anticipation for Low-Latency Spoken Dialogue — 2026 — arXiv:2606.13450
- Streaming Endpointer using Neural Audio Codecs and Label-Delayed Training — 2025 — arXiv:2506.07081
- Adaptive Endpointing with Deep Contextual Multi-armed Bandits — 2023 — arXiv:2303.13407
- Deepgram Flux docs (quickstart, state machine, listen-flux reference) — 2025/2026 — developers.deepgram.com/docs/flux, /reference/speech-to-text/listen-flux
- LiveKit turn detector docs + livekit/turn-detector (HF) — 2025/2026 — docs.livekit.io/agents/logic/turns/turn-detector
- pipecat Smart Turn v2/v3 (Daily blog + HF + GitHub) — 2025 — daily.co/blog/smart-turn-v2-…, huggingface.co/pipecat-ai/smart-turn-v2
- Streaming Punctuation … Bidirectional Context — 2023 — IJNLC / arXiv:2301.03819
- Streaming Punctuation for Long-form Dictation with Transformers — 2022 — arXiv:2210.05756
- Light-weight punctuation+casing for on-device streaming ASR — 2024 — arXiv:2407.13142
- Efficient Punctuation Restoration via Weighted Lookahead Scoring — 2026 — arXiv:2606.05179
- Where's the Point? Self-Supervised Multilingual Punctuation-Agnostic Sentence Segmentation (WtP/SaT) — 2023 — ACL / arXiv:2305.18893
- Capitalization and Punctuation Restoration: a Survey — 2021 — arXiv:2111.10746
- Semantic-Aware Interruption Detection: Benchmark, Metric, and Model (SID-Bench, APT) — 2026 — arXiv:2603.24144
- FastTurn: Unifying Acoustic and Streaming Semantic Cues — 2026 — arXiv:2604.01897
- LiveTurn: A Real-Time Turn Detection System for Voice Agents — 2026 — OpenReview JIaOGuEMET
- Gradium: Turn-Taking in Voice Agents: Why Rule-Based VAD Is Broken — 2026 — gradium.ai/content/turn-taking-voice-agents-vad
