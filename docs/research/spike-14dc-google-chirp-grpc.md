# Spike 14dc — Google Chirp 3 / Cloud Speech-to-Text v2 gRPC streaming adapter for Rust

**Status:** research complete — decision-grade
**Date:** 2026-06-30
**Author:** research analyst (subagent)
**Scope:** Can we add a Google Cloud Speech-to-Text v2 (Chirp 3) streaming ASR provider to audio-graph, and what is the right Rust gRPC/tonic plumbing? What are the risks and effort?

---

## TL;DR — Recommendation

**SPIKE FURTHER / conditional implement.** Google Chirp 3 is a strong transcription engine and the
tonic/prost gRPC stack is *already in our lockfile* (transitively via `surrealdb`), so the plumbing is
feasible. **But there is one disqualifying mismatch for our use case: Chirp 3 (and STT v2 in general)
does NOT provide speaker diarization on the streaming path.** Diarization is `BatchRecognize`-only.
Every other cloud ASR provider we ship (Deepgram, AssemblyAI, AWS Transcribe, Soniox) gives us *live,
streaming* speaker labels. audio-graph is fundamentally a live diarization-driven app, so adopting
Google streaming means accepting transcription-only (no live speaker tags), plus a non-trivial gRPC
adapter (half-close semantics, 5-minute stream cap with reconnect stitching, regional endpoint config).

Implement **only if** there is concrete demand for Google STT transcription *without* requiring live
speaker labels (e.g. single-speaker dictation, or pairing Google text with our **local** diarization
worker `src-tauri/src/diarization/`). Otherwise this is lower-value than finishing the existing
streaming providers. Recommend a short 2-3 day proof-of-concept gated behind a feature flag before
committing to a full adapter.

---

## 1. Options compared

We are choosing **(A)** which Google API surface to target, and **(B)** which Rust crate to talk gRPC with.

### A. API surface for live audio

| Option | Live speaker labels? | Latency | Notes | Source |
|---|---|---|---|---|
| **STT v2 `StreamingRecognize` + `chirp_3`** | **No** — diarization not supported on streaming; only utterance-level timestamps | Real-time (gRPC bidi) | The only real-time path. gRPC-only, no REST/WS. 5-min stream cap. | [Chirp 3 model docs — feature table][chirp3], [streaming docs][stream] |
| STT v2 `BatchRecognize` + `chirp_3` | **Yes** (single-channel diarization, GA) | Async / long-running op (not real-time) | Wrong shape for our live pipeline; would be a per-utterance batch call like `cloud.rs` does today, but with diarization. | [Chirp 3 model docs][chirp3] |
| STT v2 `Recognize` (sync) + `chirp_3` | No (diarization batch-only) | ~sub-minute audio only | Per-utterance request/response; fits our `cloud.rs` pattern but no diarization and audio < 1 min. | [Chirp 3 model docs][chirp3] |
| STT **v1** `StreamingRecognize` (legacy) | Yes (streaming diarization, beta) | Real-time | Old API, no Chirp 3, lower accuracy, Google steering everyone to v2. Not recommended for new work. | [STT multiple-voices][voices] |

### B. Rust crate for the gRPC client

| Crate | Streaming support | tonic/prost version | Maturity | Source |
|---|---|---|---|---|
| **`google-cloud-speech-v2`** (official, codyoss / Google) | **No streaming RPC.** Crate explicitly warns: *"some RPCs have no corresponding Rust function... typically these are streaming RPCs."* Only `recognize`, `batch_recognize`, recognizer CRUD. | hides tonic behind `google-cloud-gax`; auth + rustls handled for you | Official, stable, GA-quality auth; but **cannot do `StreamingRecognize` today** | [docs.rs warning][gcrust], [crates.io][gcrust-crate] |
| **`googleapis-tonic-google-cloud-speech-v2`** (community, bouzuya) | **Yes** — raw tonic-generated `SpeechClient` with `streaming_recognize`. | `tonic ^0.14.2`, `prost ^0.14.3`, `prost-types ^0.14.3` | Auto-generated from googleapis protos; ~21k downloads; updated 2026-05; MIT/Apache. You wire auth + channel yourself. | [crates.io v0.36.0][bouzuya] |
| **Hand-roll: `tonic-build` on googleapis protos** | Yes (you own the generated client) | pin to our `tonic 0.14.6` / `prost 0.14.4` | Most control, most build complexity (protoc, build.rs, vendored protos) | [tonic docs][tonic] |

**Our lockfile already has `tonic 0.14.6` and `prost 0.14.4`** (pulled transitively by `surrealdb-protocol`),
and `gcp_auth 0.12` is already a direct dependency used by the Gemini/Vertex adapter
(`src-tauri/src/gemini/mod.rs`). So the version surface lines up cleanly — the community tonic crate's
`tonic ^0.14.2` requirement is satisfied by our `0.14.6`.

---

## 2. Key trade-offs

**Latency / performance.** gRPC bidi streaming over HTTP/2 is the right transport for real-time and is
what Google recommends (100 ms frame size, real-time pacing, 25 KB/message cap). Comparable to our
existing WebSocket providers in wire latency. Chirp 3 is a generative ASR model marketed for
"state-of-the-art accuracy and speed."

**Diarization (the decider).** Streaming = no speaker labels. This is the single biggest issue. Our
other cloud providers stream speaker tags live; switching a session to Google would silently drop
diarized output, *unless* we route Google's text through our local diarization worker. That is a real
option (`src-tauri/src/diarization/{worker,clustering,stabilize}.rs` already exists) but it's
additional integration and a behavior difference users would notice.

**License / cross-platform.** tonic + prost + rustls are pure-Rust, MIT/Apache, and already build on
all our targets (they're in the tree). No protoc needed if we use the pre-generated `googleapis-tonic-*`
crate. `gcp_auth` already cross-compiles for us (Windows/macOS/Linux). No new native/system deps. Good.

**Maintenance.** Two flavors of risk: (1) the *official* crate is maintained by Google but **lacks
streaming** — betting on it means waiting for them to ship streaming RPCs (open-ended). (2) the
*community tonic* crate is a thin auto-generated binding — low surface area but depends on one
maintainer (bouzuya) and on us owning auth/channel/reconnect glue ourselves. Hand-rolling generation
adds a `build.rs` + vendored protos maintenance burden but removes the third-party dependency.

**Effort.** Materially higher than a WebSocket provider because: gRPC half-close semantics, the 5-minute
stream cap requiring "endless streaming" reconnect/stitching, channel keepalive tuning, and a new auth
flow (bearer token minted via `gcp_auth`, attached as a gRPC metadata `authorization` header on a
rustls TLS channel). Our existing adapters are all `tokio-tungstenite` WS; this is the first gRPC one.

---

## 3. Recommendation + rationale

**Recommendation: `spike-further`** — build a 2-3 day flagged proof-of-concept using the
**community `googleapis-tonic-google-cloud-speech-v2`** crate, *not* the official Google crate
(which can't stream), and *not* a hand-rolled `tonic-build` setup (premature for a PoC). Validate the
end-to-end streaming path and confirm accuracy, then decide on full adoption.

Rationale:
1. **The version surface is already paid for.** `tonic 0.14.6` / `prost 0.14.4` are in the lockfile and
   `gcp_auth 0.12` is a direct dep with a working Vertex AI token flow we can mirror. The community crate
   pins `tonic ^0.14.2` — compatible. Minimal new dependency weight.
2. **The official crate is a dead end for our need.** It explicitly does not implement streaming RPCs.
   Picking it would block us on Google's roadmap. Avoid.
3. **The diarization gap must be validated against product intent before full build.** If the requester
   only needs Google transcription (no live speaker labels), or accepts pairing with our local
   diarizer, proceed to full implement. If they expected Deepgram-style live diarization, **reject** —
   Google streaming cannot deliver it and the spike should not become a shipped provider.
4. A flag-gated PoC de-risks the two hard unknowns (gRPC half-close correctness against Google's server,
   and the 5-minute reconnect stitching) cheaply before we commit adapter-sized effort.

If product confirms diarization is required and non-negotiable for this provider: **reject** Google
streaming and keep investing in the existing streaming-diarization providers.

---

## 4. Integration risks + effort estimate

### gRPC / tonic plumbing risks

1. **Half-close (`WritesDone`) is a known tonic footgun.** Google's `StreamingRecognize` needs an
   explicit end-of-stream signal; if the client just drops the sender without a proper `END_STREAM`
   frame, Google returns `OutOfRange: Audio Timeout Error`. This exact failure is documented against
   Google's Dialogflow/Speech APIs with tonic. Mitigation: drive the request stream with a
   `tokio_stream` that ends cleanly so tonic emits `END_STREAM`; test against the real endpoint.
   ([tonic #1066][tonic1066], [SO half-close][so-halfclose])
2. **Bidi stream "hang on connect" with non-Rust servers.** tonic can block waiting for the server's
   first HTTP/2 HEADERS frame if the client waits for the RPC to "complete" before sending. The known
   workaround is to have the config message ready to send immediately (which matches Google's protocol:
   first message is config-only). Low risk if we send config first, but must be tested.
   ([tonic #515][tonic515])
3. **5-minute hard stream cap.** A `StreamingRecognize` stream may stay open **max 5 minutes**; audio
   must be paced ~real-time. Long sessions require the "endless streaming" pattern: detect the cap, close
   the stream, re-open a new one, and stitch transcripts across the boundary (replaying the trailing
   un-finalized audio). This is real code, not config. ([STT v2 quotas][quotas], [infinite-streaming sample][infinite])
4. **Reconnect / keepalive on long-lived channels.** tonic does not transparently recover a dropped
   HTTP/2 connection; teams report frozen streams after network blips. Must configure
   `http2_keep_alive_interval` + `keep_alive_while_idle` and build our own reconnect (we already have a
   reconnect state machine pattern in `src-tauri/src/asr/reconnect.rs`). ([tonic #1254][tonic1254], [gRPC keepalive][keepalive])
5. **Auth wiring differs from our REST/WS path.** Need an OAuth2 bearer token (scope
   `https://www.googleapis.com/auth/cloud-platform`) minted via `gcp_auth` — either ADC
   (`gcp_auth::provider()`) or service-account-from-file (`CustomServiceAccount::from_file`), exactly as
   `gemini/mod.rs` already does — then attach it as a gRPC `authorization: Bearer <tok>` **metadata**
   header via a tonic interceptor, and **refresh it** before expiry on reconnect. TLS via rustls (already
   our stack). ([STT endpoints][endpoints])
6. **Endpoint / region config.** Streaming uses regional host `{region}-speech.googleapis.com` or
   `speech.googleapis.com` (global); recognizer resource path is
   `projects/{PROJECT}/locations/{LOCATION}/recognizers/_` and the location segment **must match** the
   endpoint. Data-residency-sensitive users want `us-`/`eu-speech.googleapis.com`; multi-language is only
   on global/US/EU. Must be a configurable setting, validated against the recognizer path. ([endpoints][endpoints], [quotas][quotas])
7. **Privacy/egress guard.** Must thread `ProviderContentEgressPolicy` through the new client exactly
   like `cloud.rs` / the WS providers, and redact the bearer token + project id in all error/debug
   output (mirror `gemini/mod.rs`'s `redacted_provider_diagnostic`). The token and project id appear in
   headers/URLs and must never leak into logs.
8. **`stability` / no real word-confidence.** Chirp 3 returns a confidence value that "isn't truly a
   confidence score," and word-level timestamps/confidence aren't supported on streaming. Our
   `TranscriptSegment.confidence` mapping needs a sane proxy (e.g. result `stability` or a constant),
   like `cloud.rs` already does for Whisper. ([Chirp 3 docs][chirp3])

### Effort estimate

| Phase | Effort |
|---|---|
| Flagged PoC: tonic channel + gcp_auth interceptor + config-first stream + parse responses, single 5-min stream | **2-3 dev-days** |
| Full adapter: endless-streaming reconnect/stitch, settings (region/project/SA path), egress guard + redaction, error taxonomy, tests/fixtures matching the other providers | **+5-8 dev-days** |
| Optional: route Google text through local diarization worker for speaker labels | **+2-3 dev-days** |
| **Total to ship a real provider** | **~8-13 dev-days** |

For comparison this is roughly 2x a new WebSocket provider, driven by the gRPC transport novelty and the
5-minute reconnect stitching.

---

## 5. Sources

- [chirp3] Chirp 3 Transcription model — feature support table (diarization = BatchRecognize only; streaming = utterance timestamps only): https://cloud.google.com/speech-to-text/v2/docs/chirp_3-model
- [stream] Transcribe audio from streaming input (gRPC-only; 25 KB/msg; `chirp_3`; recognizer path): https://cloud.google.com/speech-to-text/v2/docs/streaming-recognize
- [quotas] STT v2 quotas & limits (5-min stream cap; 300 concurrent/region; 3000 req/min; multi-language on global/US/EU): https://cloud.google.com/speech-to-text/v2/quotas
- [endpoints] Specify a regional endpoint (US/EU residency, `*-speech.googleapis.com`, parent/location matching): https://cloud.google.com/speech-to-text/docs/endpoints
- [voices] Detect different speakers / speaker diarization (v1 streaming diarization, beta): https://cloud.google.com/speech-to-text/docs/multiple-voices
- [gcrust] Official Rust crate docs.rs — streaming-RPC warning: https://docs.rs/google-cloud-speech-v2/latest/google_cloud_speech_v2/
- [gcrust-crate] Official crate on crates.io (codyoss): https://crates.io/crates/google-cloud-speech-v2
- [bouzuya] Community tonic-generated crate `googleapis-tonic-google-cloud-speech-v2` v0.36.0 (tonic ^0.14.2 / prost ^0.14.3): https://crates.io/crates/googleapis-tonic-google-cloud-speech-v2
- [tonic] tonic crate (gRPC over hyper/rustls): https://docs.rs/tonic/latest/tonic/
- [tonic1066] tonic #1066 — half-close / END_STREAM on bidi gRPC: https://github.com/hyperium/tonic/issues/1066
- [so-halfclose] StackOverflow — half-close on tonic bidi stream (Google Audio Timeout Error): https://stackoverflow.com/questions/67610502/how-do-i-perform-a-half-close-on-a-grpc-bidirectional-stream-using-tonic
- [tonic515] grpc-rust/tonic #515 — client hangs waiting for server HEADERS on bidi connect: https://github.com/grpc/grpc-rust/issues/515
- [tonic1254] tonic #1254 — connection not reconnected after disconnect (keepalive needed): https://github.com/hyperium/tonic/issues/1254
- [keepalive] gRPC keepalive guide (HTTP/2 PING, KEEPALIVE_TIME/TIMEOUT): https://grpc.io/docs/guides/keepalive/
- [infinite] Infinite/endless streaming sample (~290s STREAMING_LIMIT, restart pattern): https://cloud.google.com/speech-to-text/docs/samples/speech-transcribe-infinite-streaming
- Pricing (per-minute, 1s increments): https://cloud.google.com/speech-to-text/pricing — STT v2 standard recognition $0.016/min (0-500k min/mo), tiered down to $0.004/min above 2M min; dynamic batch $0.003/min; first 60 min/mo free (v1 SKU). Chirp models bill under standard recognition SKUs.

### Codebase cross-references (for the implementer)
- Existing GCP auth pattern to mirror: `src-tauri/src/gemini/mod.rs` (`gcp_auth::CustomServiceAccount::from_file` + `gcp_auth::provider()`, scope `cloud-platform`, secret redaction).
- Per-utterance cloud ASR pattern (closest existing shape for a sync `Recognize` fallback): `src-tauri/src/asr/cloud.rs`.
- Reconnect state machine to reuse for the 5-minute stitch: `src-tauri/src/asr/reconnect.rs`.
- Egress guard to thread through the new client: `ProviderContentEgressPolicy` in `src-tauri/src/asr/mod.rs`.
- Local diarization worker (the only way to get speaker labels on a Google streaming session): `src-tauri/src/diarization/`.
- Dependency reality: `tonic 0.14.6` + `prost 0.14.4` already in `src-tauri/Cargo.lock`; `gcp_auth = "0.12"` already a direct dep in `src-tauri/Cargo.toml`.
