# Spike 0bdc — Rust VAD + AEC Crate Bakeoff (local, cross-platform Tauri)

**Date:** 2026-06-30
**Spike:** audio-graph-0bdc
**Scope:** Pick ONE local VAD crate + ONE local AEC path for a cross-platform (Win/macOS/Linux) Tauri desktop audio app. Also covers turn-detection / barge-in feasibility.
**Decision owner:** user. This doc is the grounded recommendation; the call stays with you.

---

## 0. Codebase constraints (the single most important input)

These are facts from this repo, not assumptions — they dominate the recommendation:

- **The app already links ONNX Runtime via `sherpa-onnx 1.13`** (`src-tauri/Cargo.toml:192`), used for streaming ASR (Zipformer) and diarization (Sortformer / clustering). `sherpa-onnx` ships and loads **its own `libonnxruntime`**.
- **The audio pipeline already produces a 16 kHz mono "processed bus"** (`src-tauri/src/audio/pipeline.rs`: `PROCESSED_AUDIO_SAMPLE_RATE_HZ = 16_000`, 48 kHz stereo → 16 kHz mono via `rubato`). A VAD that wants 16 kHz mono frames is a drop-in consumer of this bus.
- The pipeline is per-source and chunk-based, so a frame-by-frame VAD or AEC stage slots in naturally.

**Consequence:** adding a *second, differently-versioned* ONNX Runtime (which is what every Rust Silero-VAD crate pulls in via the `ort` crate) is a known footgun in exactly this configuration. `pykeio/ort` issues [#388](https://github.com/pykeio/ort/issues/388) and #106 document `sherpa` + `ort` colliding on `libonnxruntime`, producing runtime errors like *"The requested API version [20] is not available, only API versions [1,17] are supported in this build"* — the sherpa-shipped dylib wins the load and the `ort`-based crate breaks. This is the central integration risk and it steers the VAD pick.

---

## 1. Options compared

### VAD

| Crate | Version / date | Approach | ONNX dep | Latency / model | Cross-platform build | License | Maintenance | Source |
|---|---|---|---|---|---|---|---|---|
| **`webrtc-vad`** (kaegi) | 0.4.0, **2019** | GMM (libfvad C), classic WebRTC VAD | **None** (pure C via `cc`) | Sub-ms; 10/20/30 ms frames @ 8/16/32/48 kHz | Builds from source with a C compiler + `cc` crate — no system libs, no pkg-config. Cleanest cross-platform story. | MIT (libfvad BSD) | **Effectively frozen** since 2019; 363k downloads, 7 rev-deps. Stable but unmaintained. | [crates.io](https://crates.io/crates/webrtc-vad), [libfvad](https://github.com/dpirch/libfvad) |
| **`voice_activity_detector`** (nkeenan38) | 0.2.1, 2025-08 | Silero **V5** ONNX | **`ort =2.0.0-rc.10`** (exact pin) | ~1 ms/chunk CPU; fixed window (512 @16k / 256 @8k); higher accuracy in noise | Bundled ORT download by default, or `load-dynamic`. **Pins a second ORT** → collides with sherpa's runtime (see §0). | non-standard (MIT-style) | Light but alive (0.2.1 Aug 2025), 77k downloads, 10 deps | [GitHub](https://github.com/nkeenan38/voice_activity_detector), [crates.io](https://crates.io/crates/voice_activity_detector) |
| **`silero-vad-rust`** (sheldonix) / `silero` / `silero-vad-rs` | 6.2.x, 2025-11 | Silero ONNX, model **bundled in-crate** | `ort` (load-dynamic), wants ORT **1.22.x** | Same Silero accuracy; bundles weights so no download | Same ORT-coexistence problem as above; pins a specific ORT build | MIT | Newer/active but **young, single-author, low adoption** | [docs.rs](https://docs.rs/crate/silero-vad-rust/latest), [crates.io](https://crates.io/crates/silero-vad-rust) |
| **Silero via existing `sherpa-onnx`** (`VoiceActivityDetector` in sherpa) | rides 1.13 | Silero ONNX **through the ORT the app already loads** | **Reuses sherpa's ORT** — no new ORT | Silero accuracy, no second runtime | Zero new native deps; gated behind the same feature flags | Apache-2.0 (sherpa) | Actively maintained (k2-fsa) | [sherpa-onnx VAD](https://github.com/k2-fsa/sherpa-onnx) |

### AEC

| Crate | Version / date | Algorithm | Native build | Cross-platform | License | Maintenance | Quality (adversarial) | Source |
|---|---|---|---|---|---|---|---|---|
| **`aec-rs`** (thewh1teagle) | 1.0.0, 2024-12 | **SpeexDSP** echo canceller + NS | Bundles speexdsp C via `aec-rs-sys`; vendored submodule, `cargo build` only (needs a C compiler). **Precompiled libs + C header in releases.** | **Explicitly Win x86/ARM64, Linux x86/ARM64, macOS x86/ARM64, Android, iOS, WASM, RISC-V.** Best portability of any option here. | MIT | Single-author, low rev-deps (1), but self-contained and small | Attenuation-style; keeps working at high echo levels; slight residual echo; weaker double-talk than AEC3 | [GitHub](https://github.com/thewh1teagle/aec), [BUILDING.md](https://github.com/thewh1teagle/aec/blob/main/BUILDING.md), [crates.io](https://crates.io/crates/aec-rs) |
| **`webrtc-audio-processing`** (tonarino) | 2.1.0, **2026-05** | **WebRTC APM / AEC3** (+NS, AGC, VAD) | Dynamic-link to system lib OR `bundled` feature → needs **meson + ninja** and abseil-cpp; MSVC needs patches | Linux clean (`apt`/`pacman`); **Windows/macOS painful**: `bundled` needs meson/ninja, abseil resolution is fragile, **docs.rs build of 2.1.0 currently fails**, MSVC needs upstream patches (see vcpkg PR #52402, MSYS2 PKGBUILD patches) | non-standard (BSD, WebRTC) | **Actively maintained** (sys 2.1.0 May 2026) | Best-in-class quality / double-talk / drift compensation, BUT can chop near-end speech (half-duplex) under hard double-talk; sensitive to delay alignment & far/near ratio | [crates.io](https://crates.io/crates/webrtc-audio-processing-sys), [GitHub](https://github.com/tonarino/webrtc-audio-processing), [vcpkg PR](https://github.com/microsoft/vcpkg/pull/52402) |
| **`speexdsp`** (rust-av) | 0.1.x | SpeexDSP (pure-Rust port or `sys`) | `sys` feature needs system speexdsp + clang/libclang + pkg-config; pure-Rust path avoids C but is incomplete | Pure-Rust path is portable but the project is partial ("Clean pure-rust reimplementation" still a TODO) | MIT | Low activity, 17 open issues | Same Speex algorithm class as `aec-rs` but rougher packaging | [GitHub](https://github.com/rust-av/speexdsp-rs) |

---

## 2. Key trade-offs

**VAD — the deciding axis is ONNX Runtime coexistence, not accuracy.**
- Silero (V5) is meaningfully more accurate in noise and multi-language than the 2011-era GMM `webrtc-vad`. That is real and well-documented (Silero is the de-facto modern VAD; [snakers4/silero-vad](https://github.com/snakers4/silero-vad), MIT).
- BUT every standalone Rust Silero crate drags in the `ort` crate with a **pinned ORT version** that fights `sherpa-onnx`'s own `libonnxruntime` (pykeio/ort #388, #106, #299). In this repo that is a runtime-breaking, hard-to-debug class of bug (`links` collisions, API-version mismatch, dylib load order).
- `webrtc-vad` sidesteps all of it: it's a C VAD compiled in-tree via `cc`, **zero ONNX, zero system libs, builds identically on all three desktop OSes**. Its only downside is age (frozen 2019) and lower noise accuracy.
- The third option — **getting Silero through the ORT sherpa already loads** — gets you Silero accuracy with no second runtime. It's contingent on the diarization/ASR features being compiled in (they're feature-gated and mutually exclusive in places per ADR-0017), so it's not always available.

**AEC — the deciding axis is cross-platform build cost vs. quality.**
- `webrtc-audio-processing` (AEC3) is the quality leader and is actively maintained, but its cross-platform build on **Windows/macOS is the documented pain point**: the `bundled` feature needs meson + ninja + abseil, the 2.1.0 sys crate **fails to build on docs.rs**, and MSVC requires upstream patches (vcpkg PR #52402, balacoon notes, MSYS2 PKGBUILD). For a Tauri app that must build on all three OSes in CI, this is a real schedule risk.
- `aec-rs` (SpeexDSP) is the portability leader: it advertises and ships precompiled libraries for Win (x86/ARM64), macOS (x86/ARM64), Linux, plus mobile/WASM, and builds with a plain `cargo build` + C compiler. The cost is quality: SpeexDSP is an echo **attenuator**, not a full adaptive canceller — slightly weaker, leaves faint residual echo, weaker double-talk than AEC3.
- Adversarially, AEC3's quality edge is conditional: practitioners report it goes **half-duplex / chops near-end speech** under hard double-talk and high far/near energy ratios, and is very sensitive to reference-signal delay alignment ([forasoft](https://www.forasoft.com/learn/ai-for-video-engineering/articles-ai/echo-cancellation-aec-ai-hybrid-webrtc), [Rhasspy threads](https://community.rhasspy.org/t/software-ec-with-pulseaudio-webrtc/1405), DEV.to porting report). SpeexDSP "keeps working" where AEC3 collapses at high echo. So AEC3 is not a strict dominance win for a desktop conferencing/transcription use case where mic/speaker isolation is usually decent.

**The shared, non-negotiable AEC requirement (both options):** the canceller must receive the **far-end reference** (what was played out the speaker) and the **near-end capture** with accurate timing. This repo's pipeline currently captures inputs and resamples them — it does **not yet tap the playback/render path as a reference signal**. Wiring that reference tap is the bulk of the AEC integration work regardless of which crate is chosen (Switchboard, Fora Soft, and the Android AEC_DUMP debugging guide all stress this).

---

## 3. RECOMMENDATION

### VAD → `webrtc-vad` (kaegi 0.4.0) as the default; Silero-through-sherpa as an opt-in upgrade

**Rationale:** It is the only VAD that adds **zero ONNX Runtime risk** to a codebase that already ships `sherpa-onnx`'s own `libonnxruntime`. It builds cleanly and identically on Win/macOS/Linux from a single C dependency, has sub-millisecond latency, and consumes the existing 16 kHz mono bus directly (10/20/30 ms frames). It is "frozen, not broken" — 363k downloads, MIT, stable API. For barge-in/turn-detection, sub-ms GMM VAD with hangover smoothing is entirely adequate; accuracy gaps vs. Silero matter for offline transcription quality, not for "is the user talking right now."

**Upgrade lever:** when higher noise robustness is needed, run **Silero through the ORT that `sherpa-onnx` already loads** (sherpa exposes a Silero VAD) rather than adding a standalone `ort`-pinned crate. This keeps a single ONNX Runtime in the process. Treat standalone `voice_activity_detector` / `silero-vad-rust` as a **last resort**, only if you accept isolating ORT (separate process or `links`/symbol-prefix work) — do not adopt them naively alongside sherpa.

### AEC → `aec-rs` (SpeexDSP)

**Rationale:** For a cross-platform Tauri desktop app, **predictable builds on all three OSes win over peak cancellation quality.** `aec-rs` is the only AEC option with a first-class Win + macOS + Linux build story (precompiled libs + plain `cargo build`, no meson/ninja/abseil/MSVC-patch saga). SpeexDSP quality is "good enough" for the common desktop case (decent mic/speaker separation, mostly single-talk), and it degrades gracefully (residual echo) rather than catastrophically (half-duplex chopping) where AEC3 struggles. MIT-licensed, self-contained, small surface area.

**When to revisit:** if real-world testing shows unacceptable residual echo on loud speakerphone / open-air setups, escalate to `webrtc-audio-processing` (AEC3) — but budget for the Windows/macOS build hardening it demands. Do **not** stack both cancellers (the documented half-duplex trap).

**Overall confidence: medium.** The VAD pick is high-confidence (constraint-driven). The AEC pick is medium: it trades quality for buildability, and final quality can only be confirmed by on-device A/B testing with a correct reference tap.

---

## 4. Integration risks + rough effort

| Risk | Likelihood | Mitigation |
|---|---|---|
| **(VAD) Second ORT collides with sherpa's `libonnxruntime`** | High *if* a standalone Silero crate is used | Pick `webrtc-vad` (no ORT) or Silero-via-sherpa (one ORT). Documented in pykeio/ort #388/#106. |
| **(AEC) No far-end reference tap exists yet** | Certain | Add a render-path tap so the canceller gets the played-out signal time-aligned with capture. This is the main AEC work item. |
| **(AEC) Reference/capture delay misalignment** → poor cancellation | Medium | Measure & feed `stream_delay_ms`; validate with recorded near/far/out triples (AEC_DUMP-style). |
| **(AEC) SpeexDSP residual echo on loud/open setups** | Medium | A/B test early; keep AEC3 as a documented escape hatch. |
| **(AEC alt) `webrtc-audio-processing` Win/macOS build** | High if chosen | meson+ninja+abseil toolchain in CI; MSVC patches; pin a version that builds (2.1.0 docs.rs build currently fails). |
| **Frame-size / sample-rate mismatch** | Low | Both VAD and Speex AEC want 16 kHz mono frames — the existing processed bus already supplies this; webrtc-vad wants 10/20/30 ms frames, Speex wants a fixed frame/filter length. Add a small framer. |
| **License hygiene** | Low | webrtc-vad MIT/BSD, aec-rs MIT, sherpa Apache-2.0 — all permissive. Record BSD/Speex notices in NOTICE. |

**Rough effort estimate (recommended path: `webrtc-vad` + `aec-rs`):**
- VAD stage (framer + hangover/turn-state, wire to processed bus): **~1–2 days.**
- AEC: far-end reference tap + delay handling + Speex stage wiring: **~3–5 days** (the reference tap is the long pole, not the crate).
- Cross-platform CI build + smoke on Win/macOS/Linux: **~1 day** (low, since neither dep needs exotic toolchains).
- Barge-in / turn-detection state machine on top of VAD (ties into existing converse turn state machine, ADR-0018): **~1–2 days.**
- **Total: ~1.5–2 weeks** for a working, cross-platform VAD+AEC+barge-in path.
- The AEC3 alternative adds **~1 week+** of Windows/macOS build hardening on top.

---

## 5. Sources

- Codebase: `src-tauri/Cargo.toml` (sherpa-onnx 1.13 pin), `src-tauri/src/audio/pipeline.rs` (16 kHz mono bus, rubato resampler), ADR-0017 (diarization feature gating), ADR-0018 (converse turn state machine).
- `webrtc-vad` — https://crates.io/crates/webrtc-vad ; libfvad https://github.com/dpirch/libfvad
- `voice_activity_detector` — https://github.com/nkeenan38/voice_activity_detector ; https://crates.io/crates/voice_activity_detector (ort =2.0.0-rc.10 pin)
- `silero-vad-rust` — https://docs.rs/crate/silero-vad-rust/latest ; https://crates.io/crates/silero-vad-rust
- Silero VAD upstream — https://github.com/snakers4/silero-vad (MIT, Silero V5/V6)
- `aec-rs` (SpeexDSP) — https://github.com/thewh1teagle/aec ; BUILDING.md https://github.com/thewh1teagle/aec/blob/main/BUILDING.md ; https://crates.io/crates/aec-rs
- `webrtc-audio-processing` — https://github.com/tonarino/webrtc-audio-processing ; https://crates.io/crates/webrtc-audio-processing-sys (2.1.0, 2026-05; docs.rs build failing)
- `speexdsp` (rust-av) — https://github.com/rust-av/speexdsp-rs
- ORT coexistence footgun — pykeio/ort https://github.com/pykeio/ort/issues/388 , #106, https://github.com/pykeio/ort/issues/299 ; ort linking docs https://ort.pyke.io/setup/linking
- AEC3 vs Speex quality / double-talk (adversarial) — https://switchboard.audio/hub/how-webrtc-aec3-works/ ; https://www.forasoft.com/learn/ai-for-video-engineering/articles-ai/echo-cancellation-aec-ai-hybrid-webrtc ; Rhasspy https://community.rhasspy.org/t/software-ec-with-pulseaudio-webrtc/1405 ; DEV.to porting report https://dev.to/zhangzhuyue/best-solution-for-aec-by-porting-matlaboctave-algorithm-to-c-ogg ; Meta Beryl https://atscaleconference.com/improving-audio-quality-for-calling-across-metas-family-of-apps/
- Windows build pain for webrtc-audio-processing — vcpkg PR https://github.com/microsoft/vcpkg/pull/52402 ; MSYS2 PKGBUILD https://github.com/msys2/MINGW-packages ; balacoon notes https://github.com/balacoon/webrtc_audio_processing
- Reference-signal/delay alignment requirement — https://www.forasoft.com/learn/audio-for-video/articles-audio/webrtc-audio-pipeline-end-to-end ; AEC_DUMP debugging https://dev.to/snowlyg/debugging-android-webrtc-audio-3a-with-aecdump-and-audacity-2406
