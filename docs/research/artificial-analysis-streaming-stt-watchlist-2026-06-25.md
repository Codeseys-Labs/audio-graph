# Artificial Analysis Streaming STT Watchlist Backfill - 2026-06-25

**Benchmark source:** https://artificialanalysis.ai/speech-to-text/streaming

**Seed:** `audio-graph-dc5b`

**Registry descriptor follow-up:** `audio-graph-b6a6`

This backfills the Artificial Analysis tail-provider set that was not already
covered by the main streaming STT provider ranking. These entries are roadmap
classifications only. None should become selectable until runtime adapters,
credential storage, readiness checks, parser fixtures, and cross-platform smoke
evidence exist.

## Registry Decision

The initial `audio-graph-dc5b` slice did not add provider-registry descriptors.
At that point the registry could block `status: "planned"` providers, but it
could not accurately record this watchlist without misleading Settings:

- `ProviderStatus` has only `Implemented` and `Planned`; it has no `Watch`,
  `EnterpriseWatch`, or `Rejected` classification.
- Provider-specific credential keys must already exist in the Rust and
  TypeScript credential allowlists. Most of this watchlist would need new keys
  such as `xai_api_key`, `inworld_api_key`, `smallest_api_key`,
  `gradium_api_key`, `mistral_api_key`, `dashscope_api_key`, or
  `cartesia_api_key`, which is outside this Seed's ownership.
- A planned descriptor with `credential_keys: []` would make Settings report
  "No credential required", even though each cloud provider requires auth.
- A planned descriptor with an existing unrelated key would create a false
  readiness contract.

Seed `audio-graph-f8e0` added the required roadmap schema. Seed
`audio-graph-b6a6` then promoted every non-rejected provider in this watchlist
to generated provider-registry descriptors using:

- `status: "watch"` or `status: "enterprise_watch"`.
- `credential_keys: []` paired with `roadmap.auth_schema:
  "required_not_wired"`.
- No `health_check_command` or `model_catalog_command`.
- `roadmap.source_url` and `roadmap.source_date` pointing back to the
  2026-06-25 Artificial Analysis streaming STT snapshot.

These descriptors remain roadmap metadata only. They are not selectable provider
implementations until credential storage, runtime adapters, readiness checks,
parser fixtures, and cross-platform smoke evidence exist.

## Classification

| Provider from benchmark | Classification | Roadmap decision | Main blocker before registry descriptor |
| --- | --- | --- | --- |
| xAI Grok Speech to Text Streaming | Watch | Direct WebSocket STT shape appears relevant for future provider work. | Needs xAI credential key, retention/privacy review, parser fixtures for `transcript.partial` / `transcript.done`, and turn/diarization mapping. |
| NVIDIA/Together Nemotron ASR | Enterprise watch | Valuable for enterprise/self-hosted and hosted Together profiles, but not a simple WebSocket-first provider. | Needs separate NIM/gRPC/self-hosted vs Together-hosted profile model, packaging metadata, endpoint mode, and health probes. |
| Inworld STT 1 Realtime | Watch | Bidirectional WebSocket STT with interim/final transcripts and speech boundary events. | Needs Inworld credential key, Voice Profile handling, config schema, and event adapter proof. |
| Smallest.ai Pulse realtime | Watch | Realtime Pulse WebSocket is plausible; Pulse Pro remains HTTP-only for now. | Needs Smallest.ai credential key, model/profile decision, word timestamp mapping, and finalize/close handling. |
| Gradium STT Realtime | Watch | Realtime WebSocket supports semantic VAD, flush, multiplexing, and multiple audio formats. | Needs Gradium credential key, event adapter for `text` / `end_text` / `step` / `flushed`, and source/channel semantics review. |
| Mistral Voxtral Mini Transcribe Realtime | Watch | Realtime transcription API is relevant but should stay separate from local `mistral.rs` LLM support. | Needs Mistral cloud credential key, SDK/transport decision, realtime event fixtures, and explicit no-diarization handling. |
| Alibaba/Qwen3 ASR Flash Realtime | Enterprise watch | Region-aware WebSocket STT is worth tracking for DashScope/Qwen deployments. | Needs DashScope credential key, China/international endpoint mode, region/data-boundary metadata, and base64 audio adapter proof. |
| Cartesia Ink-2 endpoint variants | Watch | Realtime auto/manual STT endpoints and turn lifecycle events align with the normalized ASR roadmap. | Needs Cartesia credential key, endpoint variant model, English-only limitation copy, and manual vs auto turn adapter tests. |

No benchmark-listed provider in this backfill is rejected as of 2026-06-25. The
classification is "watch" unless the provider requires explicit deployment,
region, packaging, or enterprise endpoint modeling; those are marked
"enterprise watch".

## Generated Registry Status

| Provider from benchmark | Registry id | Registry status | Roadmap auth schema | Health check command |
| --- | --- | --- | --- | --- |
| xAI Grok Speech to Text Streaming | `asr.xai_grok_stt` | `watch` | `required_not_wired` | None |
| NVIDIA/Together Nemotron ASR | `asr.nvidia_nemotron_asr` | `enterprise_watch` | `required_not_wired` | None |
| Inworld STT 1 Realtime | `asr.inworld_stt1` | `watch` | `required_not_wired` | None |
| Smallest.ai Pulse realtime | `asr.smallest_pulse` | `watch` | `required_not_wired` | None |
| Gradium STT Realtime | `asr.gradium_stt` | `watch` | `required_not_wired` | None |
| Mistral Voxtral Mini Transcribe Realtime | `asr.mistral_voxtral_realtime` | `watch` | `required_not_wired` | None |
| Alibaba/Qwen3 ASR Flash Realtime | `asr.alibaba_qwen3_asr_flash` | `enterprise_watch` | `required_not_wired` | None |
| Cartesia Ink-2 endpoint variants | `asr.cartesia_ink2` | `watch` | `required_not_wired` | None |

## Source Notes

- xAI STT docs checked again on 2026-06-26: `wss://api.x.ai/v1/stt`, binary
  audio frames, partial/done transcript events, endpointing, multichannel,
  keyterms, Smart Turn, and `diarize=true` speaker labels on words in
  `transcript.partial` and `transcript.done` events. AudioGraph should
  advertise provider speaker attribution for xAI watch metadata, but keep it
  non-selectable until the credential schema, runtime adapter, parser fixtures,
  and provider speaker-timeline mapping are wired.
- NVIDIA/Together Nemotron ASR docs: NVIDIA Speech NIM exposes Nemotron ASR
  Streaming through deployable realtime/gRPC services; Together exposes the
  Nemotron ASR streaming model through hosted audio transcription APIs.
- Inworld STT docs: bidirectional WebSocket with initial `transcribeConfig`,
  audio chunks, interim/final transcription, speech start/stop, end-turn, and
  close-stream controls.
- Smallest.ai Pulse docs: realtime WebSocket with bearer auth, raw audio binary
  frames, final/last transcription messages, word timestamps, finalize, and
  close-stream controls.
- Gradium docs: WebSocket ASR with `setup`, `audio`, `end_of_stream`, `text`,
  `end_text`, `step`, `flushed`, semantic VAD, flush, multiplexing, and PCM/WAV
  / Opus / mu-law / a-law support.
- Mistral Voxtral docs: realtime transcription stream with audio byte input,
  session/text delta/done/error events, target streaming delay, and no realtime
  diarization.
- Qwen/Alibaba docs: `qwen3-asr-flash-realtime` over WebSocket with DashScope
  auth, VAD/manual modes, base64 audio append/commit, PCM/Opus formats,
  emotion recognition, and regional endpoints.
- Cartesia docs: Realtime STT Auto `/stt/turns/websocket` and Manual
  `/stt/websocket` endpoints with binary audio, finalize, Ink-2 lifecycle
  events, query configured model/encoding/sample rate/version, and English-only
  Ink-2 constraints.

## Registry Schema Foundation

The schema foundation required before generated descriptor backfill was:

- `roadmap_status`: `implemented`, `planned`, `watch`, `enterprise_watch`, or
  `rejected`.
- `roadmap_source_url` and `roadmap_source_date`.
- `selection_state` or `not_selectable_reason` so non-runtime entries cannot be
  interpreted as selectable setup modes.
- Credential schema metadata that can say "auth required, key not wired" without
  adding plaintext fields to `credentials.yaml`.
- Enterprise endpoint/deployment profile metadata for NIM/gRPC, regional
  DashScope, private endpoints, and hosted provider variants.

## Schema Status Update

Seed `audio-graph-f8e0` added the first registry schema foundation for this:
`ProviderStatus` can now represent `watch`, `enterprise_watch`, and `rejected`,
and generated descriptors can carry `roadmap.source_url`,
`roadmap.source_date`, and `roadmap.auth_schema: "required_not_wired"`. The
first generated docs-only descriptors are xAI Grok STT (`watch`) and
NVIDIA/Together Nemotron ASR (`enterprise_watch`).

Seed `audio-graph-b6a6` completed the remaining descriptor backfill for
Inworld STT 1, Smallest.ai Pulse, Gradium STT, Mistral Voxtral realtime,
Alibaba/Qwen3 ASR Flash, and Cartesia Ink-2. These generated entries still have
empty `credential_keys`, but that is paired with `roadmap.auth_schema:
"required_not_wired"` and no health-check command so Settings does not treat
them as auth-free or implemented.
