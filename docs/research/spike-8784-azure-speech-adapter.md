# Spike 8784 — Azure Speech (Cognitive Services) realtime streaming STT enterprise adapter for Rust

**Status:** Research complete — decision-grade recommendation below.
**Date:** 2026-06-30
**Question:** How should we add an Azure AI Speech realtime streaming STT adapter to the Rust backend? Official C++ SDK binding vs. raw WebSocket. Cover auth, streaming protocol, diarization, pricing, region/compliance, and a recommended integration approach + effort estimate.

---

## TL;DR

**Recommend: IMPLEMENT a native WebSocket adapter** (`src-tauri/src/asr/azure_speech.rs`) modeled directly on the existing `deepgram.rs` / `speechmatics.rs` / `soniox.rs` adapters, talking to Azure's documented Speech WebSocket protocol over `tokio-tungstenite` and reusing our `asr/transport.rs` write-guard + `asr/reconnect.rs` backoff.

**Reject** the official Microsoft Speech SDK (C++ with FFI bindings). It drags a closed-source native `.so/.dll/.dylib` per platform, GStreamer for compressed audio, VC++ redistributables on Windows, and a restricted set of Linux distros — directly at odds with this app's cross-platform Tauri packaging and the Windows native-link test fragility already documented in `docs/ops/windows-rust-test-crt-skew.md` / ADR-0007.

The existing codebase already proves the WS pattern works across **6 cloud streaming providers** sharing one transport abstraction. Azure is the same shape. Estimated effort: **4–7 working days** for production-grade parity (interim/final results, diarization, reconnect, privacy guard, tests).

---

## 1. Options compared

| # | Option | What it is | Native deps | Diarization | License | Maintenance risk | Fit with codebase |
|---|--------|-----------|-------------|-------------|---------|------------------|-------------------|
| **A** | **Native WebSocket adapter (recommended)** | Hand-written client against Azure's documented Speech WS protocol, on `tokio-tungstenite` (already a dep) | **None** — pure Rust, reuses existing stack | Real-time diarization is exposed on the WS `conversation` endpoint (the SDK's `ConversationTranscriber` is a thin wrapper over it) [[diarization GA]](https://techcommunity.microsoft.com/blog/azure-ai-foundry-blog/announcing-general-availability-of-real-time-diarization/4147556) | Our own code, MIT/Apache as the repo | We own it — same as our other 6 ASR adapters | **Excellent** — mirrors `deepgram.rs`, reuses `transport.rs` + `reconnect.rs` |
| **B** | `cognitive-services-speech-sdk-rs` (FFI binding) | Thin Rust bindings over Microsoft's native C API [[crate]](https://crates.io/crates/cognitive-services-speech-sdk-rs) | **Heavy**: downloads MS Speech SDK shared libs; needs Clang, `libasound2`, `libssl`, `LD_LIBRARY_PATH`/`DYLD_*` at runtime [[crate README]](https://crates.io/crates/cognitive-services-speech-sdk-rs) | Yes (full SDK feature set) | Apache-2.0 binding over **proprietary** MS binary | High — binding tracks a closed binary; runtime lib-path config; CI native-link pain | **Poor** — collides with cross-platform packaging + Windows CRT/native-link issues |
| **C** | `azure-speech` (pure-Rust SDK, jBernavaPrah) | Unofficial pure-Rust reimplementation of the JS SDK over `tokio-websockets` [[crate]](https://crates.io/crates/azure-speech) [[repo]](https://github.com/jbernavaprah/azure-speech-sdk-rs) | None | **Not implemented** — "Conversation Transcriber – Real-time Diarization (Work in Progress)" [[README]](https://github.com/jbernavaprah/azure-speech-sdk-rs) | Apache-2.0 | **High**: 11 stars, ~1 maintainer, open bug "first stream always fails" (#44), uses `tokio-websockets` (not our `tokio-tungstenite`) | Medium — would add a second WS stack and an unmaintained dep on the hot path |
| **D** | Official Speech SDK via subprocess/sidecar | Shell out to `spx` CLI or a microservice | Same native deps as B, out of process | Yes (`spx ct`) | Proprietary binary | High — process mgmt, brittle CLI flags [[spx diarization issue]](https://github.com/Azure-Samples/cognitive-services-speech-sdk/issues/2576) | **Poor** — heaviest deploy footprint |

### Why a native WS adapter is viable here (codebase evidence)
- `src-tauri/Cargo.toml` already pins `tokio-tungstenite = "0.29"` and uses it for all streaming ASR.
- `src-tauri/src/asr/mod.rs` registers 6 cloud streaming adapters: `deepgram`, `assemblyai`, `gladia`, `revai`, `soniox`, `speechmatics`.
- `src-tauri/src/asr/transport.rs` is a shared `AsrWsWriteGuard` (content-egress policy at the write primitive) consumed by every provider.
- `src-tauri/src/asr/reconnect.rs` gives shared backoff (`next_reconnect_step`).
- `deepgram.rs` documents the exact pattern Azure needs: open WSS with query params, auth via header, stream i16 LE PCM binary frames, receive JSON interim/final, send keepalive, send terminal frame. Azure's protocol is the same family.

---

## 2. Azure protocol / auth / diarization facts (grounded)

**Streaming protocol.** Azure's realtime STT runs over a documented WebSocket protocol (IETF RFC 6455): HTTP upgrade → `101 Switching Protocols`, then text frames (header+body, HTTP-style) for control/results and **binary frames for audio** (big-endian 2-byte header-size prefix). Default audio format is 16 kHz / 16-bit / mono PCM WAV. The official SDKs are reference implementations over this same protocol. [[WS protocol]](https://github.com/MicrosoftDocs/azure-docs/commit/b877b435b33286a1326eabe3b53ea3cc0b5b365a) [[how-to-recognize-speech]](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/how-to-recognize-speech)
- Endpoint host: `wss://<region>.stt.speech.microsoft.com/...` (regional) or the resource custom-subdomain host `https://<resource>.cognitiveservices.azure.com/...` for token/Entra flows.
- Our PCM pipeline already produces the right shape: `SpeechSegment.audio` is 16 kHz mono (`asr/mod.rs`), so we send i16 LE PCM exactly like Deepgram.

**Auth model (3 ways).** [[authentication]](https://learn.microsoft.com/en-us/azure/ai-services/authentication) [[Entra auth]](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/how-to-configure-azure-ad-auth)
1. **Resource key** — `Ocp-Apim-Subscription-Key: <key>` header. Simplest; works with all endpoint formats. This is our BYOK default (parallels Deepgram's `Authorization: Token`).
2. **STS bearer token** — POST the key to `https://<region>.api.cognitive.microsoft.com/sts/v1.0/issueToken` → JWT valid **10 minutes** (refresh ~every 9). Sent as `Authorization: Bearer <token>`. Tokens are scoped to the issuing host.
3. **Microsoft Entra ID** — managed identity / service principal; token format `aad#<resourceId>#<entra-token>`. **Requires a custom subdomain** (regional endpoints don't support Entra). This is the enterprise-preferred path (no static keys, Key Vault rotation, RBAC).
- Adapter implication: support key (BYOK, default) **and** a bearer-token provider so enterprises can plug an Entra token broker. Our `credentials/` + `ProviderContentEgressPolicy` already model per-provider secrets and egress gating.

**Diarization.** Real-time diarization is **GA** as an add-on. The SDK surfaces it via `ConversationTranscriber` (speakers tagged `Guest-1/2/3...`); the underlying transport is the same STT WS `conversation` endpoint, so a native client can request it. Up to **35 speakers**; **mono single-channel** input (stereo silently ignored historically — known bug class). Word-level timestamps + phrase lists supported. [[diarization GA]](https://techcommunity.microsoft.com/blog/azure-ai-foundry-blog/announcing-general-availability-of-real-time-diarization/4147556) [[diarization quickstart]](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/get-started-stt-diarization) [[STT overview]](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/speech-to-text)
- Maps cleanly onto our `TranscriptSegment.speaker_id` / `speaker_label` fields (currently filled by local diarization). Azure can populate them server-side.
- **Adversarial caveat:** diarization quality degrades badly on overlapping speech and similar voices — speakers get clubbed into one ID or mis-split [[overlap issue #2615]](https://github.com/Azure-Samples/cognitive-services-speech-sdk/issues/2615) [[batch diarization #1051]](https://github.com/Azure-Samples/cognitive-services-speech-sdk/issues/1051). REST short-audio API does **not** support real-time diarization — only the streaming WS / SDK path does.

**Pricing (per-second billing).** [[Azure pricing]](https://azure.microsoft.com/en-us/pricing/details/speech/) [[apio summary]](https://apio.sh/apis/azure-speech-to-text) [[review]](https://blocksentient.com/review/microsoft-azure-speech-service/)
- Standard real-time STT: **$1.00 / audio hour** ($0.0167/min). Custom model real-time: $1.20/hr.
- **Diarization add-on (real-time): +$0.30 / audio hour / feature.** (Language ID and pronunciation assessment are each also +$0.30/hr.) Free for batch.
- Free F0 tier: 5 audio hours/month, 1 concurrent request, no SLA.
- Commitment tiers: $1,600/mo for 2,000 hrs ($0.80/hr) → $25,000/mo for 50,000 hrs ($0.50/hr). Unused committed hours are not refunded.
- Position vs. our other providers: comparable to Google ($0.016/min); pricier per-minute than Deepgram/Soniox, but bundles diarization/translation/TTS under one compliance umbrella.

**Region / compliance posture (enterprise).** [[compliance matrix]](https://gist.github.com/manishtiwari25/6c34ec94d1bf0d851e91f6be4abbc908) [[SOC 2]](https://learn.microsoft.com/en-us/azure/ai-services/) [[sovereign clouds]](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/sovereign-clouds) [[HIPAA BAA]](https://learn.microsoft.com/en-us/compliance/regulatory/offering-hipaa-hitech)
- Azure AI Speech carries **SOC 1/2/3, ISO 27001/27017/27018/27701, HIPAA BAA, HITRUST, PCI DSS, GDPR, Germany C5**, available across **30+ regions** for data residency.
- **Sovereign clouds**: Azure Government (US Gov Arizona/Virginia, FedRAMP, `*.cognitive.microsoft.us`) and Azure China (21Vianet, `*.cognitive.azure.cn`). Adapter must allow a configurable region/host so sovereign endpoints work — our `speechmatics.rs` already does region-host selection (EU/US constants), so this is an established pattern.
- Real-time synthesis/recognition input is not retained by Microsoft (data-privacy docs); strongest enterprise story of the providers we integrate.

---

## 3. Key trade-offs (latency / perf / license / cross-platform / maintenance / effort)

| Dimension | Native WS (A) | Official SDK FFI (B) | Pure-Rust crate (C) |
|-----------|---------------|----------------------|---------------------|
| **Latency/perf** | Direct WS, no marshalling; same path as Deepgram (proven low-latency) | Native, fast, but FFI boundary + extra threads | WS, but extra abstraction + open "first stream fails" bug |
| **License** | Ours | Proprietary binary + Apache binding | Apache-2.0, unofficial |
| **Cross-platform** | Pure Rust — builds anywhere our app builds | Needs MS libs + GStreamer + VC++ redist; Linux limited to Ubuntu 20/22/24, Debian 11/12, Amazon Linux 2023, Azure Linux 3 [[setup-platform]](https://learn.microsoft.com/en-us/azure/ai-services/speech-service/quickstarts/setup-platform) | Pure Rust, but second WS stack (`tokio-websockets`) |
| **Maintenance** | We own it; matches 6 existing adapters | Track closed binary + runtime lib paths in CI; aggravates Windows native-link test issues (ADR-0007) | Single-maintainer, 11-star repo; diarization unimplemented |
| **Diarization today** | Implement against WS conversation endpoint (GA) | Built-in | **Missing** (WIP) — disqualifying for the spike's core requirement |
| **Effort to first transcript** | Moderate (protocol framing) | Low (SDK does it) — offset by huge build/deploy cost | Low, but blocked on missing diarization |
| **Effort to production parity** | 4–7 days | Build/packaging weeks of risk | Would require upstream contribution for diarization |

---

## 4. Recommendation + rationale

**Implement a native WebSocket Azure Speech adapter (Option A).**

Rationale:
1. **Diarization is a core requirement and only A and B can do it today.** Option C (the only other pure-Rust path) has diarization explicitly marked Work-In-Progress and unmerged.
2. **The official SDK (B/D) is a packaging liability for this exact app.** It requires platform-specific native binaries, GStreamer, VC++ redistributables, and a narrow Linux support matrix — and the repo already has documented Windows native-link/CRT test fragility (`docs/ops/windows-rust-test-crt-skew.md`, ADR-0007) that an FFI ASR dep would worsen.
3. **The pattern is already proven 6×.** Azure's WS protocol is the same family as Deepgram/Speechmatics (WSS + query params + header auth + binary PCM + JSON results + keepalive + terminal frame). We reuse `transport.rs` (egress guard), `reconnect.rs` (backoff), the `DeepgramEvent`-style JSON enum, and the sync-facade-over-tokio threading model verbatim.
4. **Enterprise auth/compliance maps onto existing abstractions.** Key + bearer-token + (Entra-broker-friendly) auth modes plug into `credentials/` and `ProviderContentEgressPolicy`; configurable region/host (incl. sovereign clouds) matches the `speechmatics.rs` region-constant pattern.

**Caveat to surface to the user (the decision is theirs):** if a future requirement is *embedded/offline* speech or features only in the native SDK (keyword spotting, custom on-device models), revisit Option B as a separate gated feature — do **not** make it the default cloud path.

---

## 5. Integration risks + effort estimate

### Risks
- **Protocol framing.** Azure's text frames use an HTTP-style header+body and binary frames carry a big-endian size-prefixed header — more structured than Deepgram's plain JSON. Budget time to encode/parse `Path`/`X-RequestId`/`Content-Type` headers and the `turn.start` / `speech.hypothesis` / `speech.phrase` / `turn.end` message lifecycle. (Mitigation: the JS SDK source and the WS-protocol doc are the spec; write fixture tests like the existing `ws_fixture.rs`.)
- **Token lifecycle (if STS/Entra used).** 10-minute token expiry needs a refresh loop before the socket drops; key-auth avoids this. Ship key-auth first; add bearer/Entra second.
- **Diarization accuracy ceiling.** Overlapping speakers and similar voices degrade server-side diarization [[#2615]](https://github.com/Azure-Samples/cognitive-services-speech-sdk/issues/2615). Set expectations; keep our local diarization path as an alternative.
- **Mono-only diarization.** Stereo input is silently dropped historically — enforce mono downmix before send (our pipeline is already 16 kHz mono).
- **Region/sovereign-cloud host config.** Must be configurable, not hardcoded, for enterprise data residency and Gov/China clouds.
- **Cost surprise.** Diarization is a +$0.30/hr add-on on top of $1/hr — make it an explicit opt-in setting, not always-on.

### Effort estimate (native WS adapter, parity with existing providers)
| Task | Est. |
|------|------|
| Protocol framing (header/body text frames, binary audio, message lifecycle) + fixtures | 1.5–2.5 d |
| Auth: resource-key (BYOK default) + bearer-token mode + region/host config | 1 d |
| Event enum + interim/final + diarization speaker mapping into `TranscriptSegment` | 1 d |
| Reconnect/keepalive/terminal wiring (reuse `reconnect.rs`/`transport.rs`) | 0.5 d |
| Settings/credentials/commands registration (`AsrProvider::AzureSpeech`, key descriptor) + privacy-mode gating | 0.5–1 d |
| Tests (unit + WS fixture round-trip, egress-policy, model-catalog) + docs | 1 d |
| **Total** | **~4–7 working days** |

Entra-ID/managed-identity token broker is an additional ~1–2 days if/when an enterprise customer needs keyless auth; not required for v1.

---

## 6. Sources

- Azure Speech WebSocket protocol (frame structure, RFC 6455 upgrade, binary audio): https://github.com/MicrosoftDocs/azure-docs/commit/b877b435b33286a1326eabe3b53ea3cc0b5b365a
- How to recognize speech (SpeechConfig, push/pull streams, container WS endpoints): https://learn.microsoft.com/en-us/azure/ai-services/speech-service/how-to-recognize-speech
- STT overview (real-time vs batch, diarization up to 35 speakers): https://learn.microsoft.com/en-us/azure/ai-services/speech-service/speech-to-text
- Real-time diarization GA announcement (ConversationTranscriber/MeetingTranscriber, mono): https://techcommunity.microsoft.com/blog/azure-ai-foundry-blog/announcing-general-availability-of-real-time-diarization/4147556
- Real-time diarization quickstart (REST short-audio does NOT support real-time diarization): https://learn.microsoft.com/en-us/azure/ai-services/speech-service/get-started-stt-diarization
- Authentication (Ocp-Apim-Subscription-Key, STS issueToken, 10-min token): https://learn.microsoft.com/en-us/azure/ai-services/authentication
- Microsoft Entra auth (aad# token format, custom subdomain required): https://learn.microsoft.com/en-us/azure/ai-services/speech-service/how-to-configure-azure-ad-auth
- Pricing (per-second; $1/hr STT, +$0.30/hr diarization add-on, commitment tiers): https://azure.microsoft.com/en-us/pricing/details/speech/
- Pricing/compliance summary (SOC2/HIPAA/ISO/PCI, 30+ regions): https://apio.sh/apis/azure-speech-to-text
- Sovereign clouds (Gov + China endpoints, region identifiers): https://learn.microsoft.com/en-us/azure/ai-services/speech-service/sovereign-clouds
- HIPAA/HITECH BAA coverage: https://learn.microsoft.com/en-us/compliance/regulatory/offering-hipaa-hitech
- Speech SDK platform install reqs (VC++ redist, limited Linux distros, x64/ARM): https://learn.microsoft.com/en-us/azure/ai-services/speech-service/quickstarts/setup-platform
- Compressed audio needs GStreamer (not bundled, licensing): https://learn.microsoft.com/en-us/azure/ai-services/speech-service/how-to-use-codec-compressed-audio-input-streams
- `cognitive-services-speech-sdk-rs` (FFI binding, native lib download, LD_LIBRARY_PATH): https://crates.io/crates/cognitive-services-speech-sdk-rs
- `azure-speech` pure-Rust crate (diarization WIP, tokio-websockets): https://crates.io/crates/azure-speech and https://github.com/jbernavaprah/azure-speech-sdk-rs
- Adversarial — overlapping-speaker diarization failure: https://github.com/Azure-Samples/cognitive-services-speech-sdk/issues/2615
- Adversarial — batch diarization silently ignored / stereo bug: https://github.com/Azure-Samples/cognitive-services-speech-sdk/issues/1051
- Adversarial — spx CLI diarization confusion: https://github.com/Azure-Samples/cognitive-services-speech-sdk/issues/2576

### Codebase anchors (existing patterns to reuse)
- `src-tauri/src/asr/mod.rs` — 6 cloud streaming adapters registered; `SpeechSegment` is 16 kHz mono; `ProviderContentEgressPolicy`.
- `src-tauri/src/asr/transport.rs` — shared `AsrWsWriteGuard` egress check at the write primitive.
- `src-tauri/src/asr/deepgram.rs` — canonical WSS + header-auth + binary-PCM + JSON-result + keepalive + terminal pattern; sync-facade-over-tokio threading.
- `src-tauri/src/asr/speechmatics.rs` — region-host constants + bearer-token auth pattern (template for Azure region/sovereign-cloud config).
- `src-tauri/src/asr/reconnect.rs` — shared backoff. `src-tauri/Cargo.toml` — `tokio-tungstenite = "0.29"` already present.
- `docs/ops/windows-rust-test-crt-skew.md`, ADR-0007 — documented native-link/CRT test fragility that argues against an FFI/native-binary ASR dependency.
