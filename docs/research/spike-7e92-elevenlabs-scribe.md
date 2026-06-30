# Spike 7e92 — ElevenLabs Scribe v2 Realtime STT: provider-adapter decision

**Status:** decision-grade research note
**Date:** 2026-06-30
**Question:** Is ElevenLabs Scribe v2 Realtime worth a provider adapter in audio-graph now, or watch-only?
**TL;DR recommendation:** **watch-only.** The model is GA and technically credible (WebSocket, PCM16, ~150ms predictive latency, strong WER), but it **has no realtime speaker diarization** — and every streaming ASR provider already wired into this app carries an `enable_diarization` flag because audio-graph is a multi-speaker app. Scribe RT would be the only cloud provider that can't diarize, it adds no capability the existing seven providers lack, and it still has open turn-commit / duplicated-text reliability bugs. Revisit when ElevenLabs ships realtime diarization (they explicitly say it is "not a priority at the moment").

---

## 1. Options compared

The decision is not "Scribe vs nothing" — audio-graph already ships seven streaming/cloud ASR providers (`AsrProvider` enum in `src-tauri/src/settings/mod.rs:176`: `DeepgramStreaming`, `AssemblyAI`, `Soniox`, `AwsTranscribe`, `OpenAiRealtimeTranscription`, plus `Api` and local `Sherpa`/`Moonshine`/`Whisper`). So the real options are about whether to *add* Scribe to that roster.

| Option | What it means | Effort | Net new capability vs. current roster | Source |
|---|---|---|---|---|
| **A. Implement Scribe v2 RT adapter now** | New `asr/elevenlabs.rs` parser + `AsrProvider::ElevenLabs` variant + transport wiring + settings/credentials/commands plumbing | ~2.5–4 dev-days (see §4) | None that matters for this app: it's a 7th cloud STT with strong accuracy but **no realtime diarization** | [Realtime API ref][1], existing adapters |
| **B. Watch-only (recommended)** | Track the model; add adapter when realtime diarization ships or a concrete user asks for it | ~0.5 day to file a tracking issue + this note | n/a | [Diarization FAQ][6] |
| **C. Use Scribe v2 *batch* for offline files** | Different product (`scribe_v2`, not realtime): supports up to 32 speaker diarization, 90+ langs | separate spike; not the question asked | offline diarized transcription | [STT capabilities][2] |
| **Incumbents already shipped** | Deepgram Nova/Flux, AssemblyAI Universal, Soniox stt-rt-v4, Speechmatics RT | already done | realtime + diarization + EOT | code + [Soniox benchmark][8], [Coval][7] |

### Scribe v2 Realtime — verified facts

| Property | Value | Source |
|---|---|---|
| GA status | **GA**, launched **Nov 11, 2025**; "available today through the API"; integrated into ElevenLabs Agents but **not the default agent model yet** | [Launch blog][3], [Agents note][6] |
| Model ID | `scribe_v2_realtime` | [Realtime ref][1] |
| Protocol | WebSocket `wss://api.elevenlabs.io/v1/speech-to-text/realtime?model_id=scribe_v2_realtime` | [Server-side ref][5] |
| Wire format | JSON frames; `input_audio_chunk` with `audio_base_64` (base64 PCM), `commit` bool, `sample_rate`; server emits `partial_transcript`, `committed_transcript`, `committed_transcript_with_timestamps`, `error` | [Realtime ref][1], [Server-side ref][5] |
| Audio in | PCM16 LE mono, 8k–48k Hz (16k recommended), also μ-law 8k; chunks 0.1–1 s (32,000 bytes = 1 s @16k) | [Commit strategies][4] |
| Commit model | **Manual** (default) or **VAD** (auto-commit on silence). Auto-commit at ~36 s if neither fires. Commit clears segment, keeps context | [Commit strategies][4] |
| Latency claim | "~150ms†" (vendor), "negative latency" predictive next-word/punctuation | [STT capabilities][2], [Launch blog][3] |
| Accuracy claim | 93.5% across 30 EU/Asian langs; built for noisy/agentic audio; beats peers on 500 hard samples (vendor) | [Launch blog][3] |
| **Realtime diarization** | **NOT supported.** "No currently. For multi-speaker identification, use Scribe v2 (batch)." | [Diarization FAQ][6], [Deepgram comparison][9] |
| Dual-channel | "not planned" for realtime | [Diarization FAQ][6] |
| Keyterm prompting | up to 50 keyterms (20 chars) on RT; `no_verbatim` mode available | [STT capabilities][2] |
| Auth | API key server-side, or 15-min single-use token client-side | [Client-side ref][1] |
| Pricing | **$0.39/hr** ($0.0065/min) of audio; +$0.07/hr entity detection, +$0.05/hr keyterms. May 7 2026 reset cut STT ~45%; Business annual ~$0.28/hr | [API pricing][10], [Coval][7] |
| Compliance | SOC 2, ISO 27001, PCI DSS L1, HIPAA (BAA, sales-gated), GDPR; EU + India residency; zero-retention mode | [Launch blog][3], [STT capabilities][2] |

---

## 2. Key trade-offs

- **Latency / perf:** Vendor "~150ms" is plausible for time-to-first-*partial* with predictive emission. Independent streaming benchmarks are mixed and methodology-sensitive: Soniox's Pipecat run lists a `scribe_v2_realtime`-class entry at ~281ms TTFS median ([8], competitive); the Coval/Gradium benchmark's "Scribe v2" 2,080ms TTFT figure is the **batch** model, not RT, so it's not apples-to-apples ([7]). Net: RT latency is competitive but NOT clearly better than Deepgram Nova-3 (~247ms TTFS [8]) or Soniox (~249ms [8]) already in the app.
- **Accuracy:** Genuinely strong. Scribe v2 (3.1% WER) is consistently top-2/3 on real-world audio; resists hallucination; good noise/accent handling ([7], webaistack review). This is the one real selling point — but the app's incumbents (AssemblyAI, Soniox) are within 1–2 points.
- **Diarization — the decisive gap:** Every cloud `AsrProvider` in this codebase exposes `enable_diarization` (Deepgram, AssemblyAI, Soniox, AWS all default it `true`; `settings/mod.rs:198–260`). Audio-graph is a *speaker-attributed transcript graph*. A provider that can't diarize live is a second-class citizen here. You could pair Scribe RT with the app's own local diarization (`diarization/` Sortformer/clustering modules), but that's extra integration cost for a path the incumbents give you for free, and ElevenLabs themselves call live diarization "not a priority."
- **License / cross-platform:** Pure cloud HTTP/WebSocket — no native deps, no platform-specific build concerns. Same posture as the existing cloud adapters. No licensing issue (BYOK, user's own API key).
- **Maintenance / reliability:** Two open, real bugs found: (a) `committed_transcript` **triplicated text** intermittently (since fixed server-side per the issue thread, [11]); (b) **turn never commits / commits very late** with VAD, and "generates random text on noise" — an active LiveKit integration pain point with no clean fix ([12]). This is the exact failure mode that would hurt a live transcript UI.
- **Effort:** Moderate but non-trivial — it's a bespoke WebSocket protocol (base64-in-JSON, dual commit strategies, ~14 distinct error event types), not the simple HTTP request/response that `cloud.rs` handles. Closest existing template is `speechmatics.rs` (1,099 lines, parser-only) or `soniox.rs`.

---

## 3. RECOMMENDATION — **watch-only**

**Do not build a Scribe v2 Realtime adapter now.** Rationale, in priority order:

1. **No incremental capability for this app.** Audio-graph already has seven ASR backends covering the same low-latency-streaming niche, and all the cloud ones diarize. Scribe RT adds a strong-accuracy option but subtracts diarization — a net downgrade against incumbents *for this product's core job* (speaker-attributed transcripts).
2. **The one differentiator (accuracy) is marginal here.** Top streaming models sit within 1–2 WER points; AssemblyAI/Soniox are already close, and "test on your own audio" is the universal caveat ([7],[9]). Accuracy alone doesn't justify a new bespoke adapter.
3. **Open reliability bugs touch our exact use case.** Late/never-firing commits and noise-triggered hallucinations ([12]) are the failure modes a live captioning UI is most sensitive to.
4. **The blocking gap is explicitly on ElevenLabs' roadmap as deprioritized.** They say realtime diarization "isn't a priority at the moment" ([6]) — so the watch trigger is clear and externally observable.

**Watch triggers (revisit and likely flip to implement):**
- ElevenLabs ships **realtime speaker diarization** (the gating feature), OR
- A concrete user/customer specifically requests ElevenLabs (e.g. they're already on the ElevenLabs stack / Agents and want one vendor), OR
- The commit-timing bug class ([12]) is closed and confirmed stable in production integrations.

If a watch trigger fires, Option A becomes a clean, well-scoped 2.5–4 day task using the `speechmatics.rs`/`soniox.rs` adapter as the template, optionally paired with the in-app `diarization/` Sortformer backend.

---

## 4. Integration risks + rough effort estimate

**If/when implemented (Option A), the work is:**

| Piece | Files | Est. |
|---|---|---|
| Parser + config (WS URL, dual commit strategy, base64 framing, error-event taxonomy) | new `src-tauri/src/asr/elevenlabs.rs` (model on `speechmatics.rs` / `soniox.rs`) | 1–1.5 d |
| `AsrProvider::ElevenLabs` variant + serde + provider-id + `requires_cloud` + secret plumbing | `settings/mod.rs` (5+ match arms incl. `:419`, `:436`, `:1039`, `:1403`), `credentials/mod.rs` | 0.5–1 d |
| Transport/reconnect wiring into the span-revision contract + commands.rs registration | `asr/transport.rs`, `asr/reconnect.rs`, `commands.rs` | 0.5–1 d |
| Tests (parser fixtures, egress-policy, settings round-trip) + frontend provider option + config defaults | `asr/fixtures.rs`, `config/default.toml`, frontend | 0.5 d |

**Total: ~2.5–4 developer-days.**

**Risks:**
- **Diarization mismatch (high):** Either ship Scribe RT as a non-diarizing provider (UX regression vs. peers) or take on extra work to fuse it with the local `diarization/` module (word-timestamp + RTTM-style join; expect 1–3 s added lag and speaker flip-flop smoothing per [9]).
- **Commit-strategy correctness (medium):** Must pick VAD vs manual and tune `vad_silence_threshold_secs`/`vad_threshold`; the LiveKit thread ([12]) shows server VAD is noise-sensitive and can hallucinate post-commit, while pure local-VAD commit has false positives. Needs real-audio tuning, not just unit tests.
- **Protocol churn (low-medium):** Realtime API surface is young (GA Nov 2025); query-param and event shapes may still shift. Parser-only adapters (the house pattern) limit blast radius.
- **Single-use-token flow (low):** Only relevant if transcribing client-side; the desktop app would use the API key server-side, avoiding the 15-min token dance.

---

## 5. Sources

- [1] ElevenLabs — Realtime STT API reference (events, schemas, client-side): https://elevenlabs.io/docs/api-reference/speech-to-text/v-1-speech-to-text-realtime and https://elevenlabs.io/docs/eleven-api/guides/how-to/speech-to-text/realtime/client-side-streaming
- [2] ElevenLabs — Speech to Text capabilities overview: https://elevenlabs.io/docs/overview/capabilities/speech-to-text
- [3] ElevenLabs — "Introducing Scribe v2 Realtime" (launch blog, 2025-11-11): https://elevenlabs.io/blog/introducing-scribe-v2-realtime
- [4] ElevenLabs — Transcripts and commit strategies (manual/VAD, formats, chunking): https://elevenlabs.io/docs/eleven-api/guides/how-to/speech-to-text/realtime/transcripts-and-commit-strategies
- [5] ElevenLabs skills — realtime server-side reference (direct WebSocket, message format, audio reqs): https://github.com/elevenlabs/skills/blob/main/speech-to-text/references/realtime-server-side.md
- [6] ElevenLabs — Realtime STT product/FAQ page (no realtime diarization; not default agent model; dual-channel not planned): https://elevenlabs.io/realtime-speech-to-text
- [7] Coval — "Best STT Providers 2026" independent benchmarks (latency/WER market structure; Scribe v1 deprecated; May 7 pricing reset): https://www.coval.ai/blog/best-speech-to-text-providers-in-2026-independent-benchmarks-and-how-to-choose/ ; Gradium/Coval TTFT+WER table: https://gradium.ai/content/stt-api-benchmark-2026-latency-accuracy
- [8] Soniox / Pipecat STT benchmark (TTFS + semantic WER, includes scribe_v2_realtime row): https://soniox.com/benchmarks
- [9] Deepgram — "ElevenLabs Transcription vs Deepgram" (confirms Realtime does NOT diarize; live-diarization fusion guidance): https://deepgram.com/learn/elevenlabs-transcription-vs-deepgram
- [10] ElevenLabs API pricing ($0.39/hr Scribe v2 Realtime): https://elevenlabs.io/pricing/api
- [11] GitHub elevenlabs-python #686 — "Scribe v2 Realtime WebSocket: committed_transcript returns triplicated text" (server-side bug, reported fixed): https://github.com/elevenlabs/elevenlabs-python/issues/686
- [12] GitHub livekit/agents #4087 — "ElevenLabs Scribe v2 never commits the turn" (commit-timing + noise hallucination, unresolved): https://github.com/livekit/agents/issues/4087
- [adversarial] stayingahead.com long-form test, webaistack.com review (accuracy nuance, top-2/3 not always #1): https://www.stayingahead.com/p/elevenlabs-scribe-v2 ; https://webaistack.com/revolutionizing-audio-transcription-a-deep-dive-into-eleven-labs-ai-technology/
- Codebase grounding: `src-tauri/src/settings/mod.rs` (`AsrProvider` enum), `src-tauri/src/asr/{speechmatics,soniox,deepgram,cloud,transport}.rs`, `src-tauri/src/diarization/mod.rs`.
