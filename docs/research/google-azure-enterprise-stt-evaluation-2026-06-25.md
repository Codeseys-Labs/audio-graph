# Google Chirp and Azure Speech Enterprise STT Evaluation - 2026-06-25

Fetched 2026-06-25. Scope: close `audio-graph-473b` by deciding whether Google
Chirp 3 and Azure Speech should become near-term native ASR providers or remain
enterprise adapters behind the WebSocket-first provider roadmap.

## Recommendation

Keep Google Chirp 3 and Azure Speech out of the next default provider wave.
They are credible enterprise options, but they add heavier auth, transport, and
packaging costs than Soniox, AssemblyAI v3, Gladia, and Speechmatics.

Recommended order remains:

| Rank | Provider | Decision | Why |
| --- | --- | --- | --- |
| 1 | Soniox v5 realtime | Implement first net-new STT runtime | Direct WebSocket, binary audio, model listing, token speaker/language metadata, lower desktop packaging risk. |
| 2 | AssemblyAI v3 Universal-Streaming | Upgrade existing provider | Existing product surface and credentials; v3 event model maps well to normalized span revisions. |
| 3 | Gladia Solaria live | Implement after parser/config readiness | Strong live fit, but two-step REST-init plus tokenized WebSocket lifecycle is heavier than Soniox/AssemblyAI. |
| 4 | Speechmatics realtime enhanced | Implement after shared WebSocket harness | Mature enterprise WebSocket API with rich config, but larger Settings/readiness surface. |
| 5 | Azure Speech | Optional enterprise adapter | Real-time diarization is attractive, but SDK/native dependency and endpoint/auth complexity should be isolated from the core provider lane. |
| 6 | Google Chirp 3 | Optional enterprise/gRPC adapter | Strong model and procurement fit, but streaming is gRPC-only and current Rust client coverage is not enough for a low-risk native implementation. |

Do not start implementation for Google or Azure until the provider registry can
represent SDK/gRPC transports, endpoint/auth mode, private endpoint shape,
speaker semantics, and a headless health probe.

## Google Chirp 3

### Fit

Google Chirp 3 is a serious enterprise candidate. The official docs list it as
Speech-to-Text API V2 only, model id `chirp_3`, with `us` and `eu`
multi-region GA availability. It supports `StreamingRecognize`, `Recognize`,
and `BatchRecognize`, and has broad multilingual coverage.

### Transport and packaging

The blocker for AudioGraph is not model quality; it is integration shape.
Google streaming recognition is gRPC-only. That means a Rust desktop runtime
needs one of these paths:

- custom `tonic`/protobuf integration against `google.cloud.speech.v2`;
- a Go/Node/Python sidecar process;
- wait for official Rust client streaming RPC coverage to mature.

The current `google-cloud-speech-v2` Rust crate docs warn that some streaming
RPCs may have no Rust function. Since Chirp 3 streaming depends on
`StreamingRecognize`, this is too much risk for the next provider wave.

### Auth and configuration

Google is not an API-key-only desktop provider. A usable provider design needs:

- project id;
- location, likely `us` or `eu` initially;
- recognizer name or `_` implicit recognizer;
- Application Default Credentials or service-account JSON;
- OAuth scope `https://www.googleapis.com/auth/cloud-platform`;
- IAM permission `speech.recognizers.recognize`.

This should not be stored as plaintext in normal settings. If supported, the
credential path should be a local secret reference, not a serialized service
account blob in `config.yaml`.

### Diarization and timing

Be conservative: do not advertise Google Chirp as a real-time diarization
provider. The Chirp 3 page states streaming is supported, but its diarization
feature details limit speaker diarization to non-streaming recognition paths.
For AudioGraph, Google live mode should emit transcript span revisions and let
the local/provider-neutral SpeakerTimeline pipeline supply real-time speaker
updates.

### Health and model discovery

Google readiness must be more than "key exists":

- validate ADC or service-account token acquisition;
- validate project/location/recognizer access with a non-audio metadata call;
- list supported locations/languages/features through the locations/model
  metadata API before populating Settings;
- report `auth_failed`, `permission_denied`, `api_disabled`, `region_mismatch`,
  and `streaming_rpc_unavailable` distinctly.

## Azure Speech

### Fit

Azure Speech is stronger than Google for enterprise real-time diarization today.
Microsoft's real-time diarization quickstart uses the Speech SDK
`ConversationTranscriber`, returns interim and final text, and exposes
`speaker_id` values such as `Guest-1`/`Guest-2`. This maps naturally into the
future provider-neutral SpeakerTimeline event schema.

### Transport and packaging

Azure is still not a clean WebSocket-first provider for this app. The supported
real-time path is SDK-centered. The SDK is cross-platform, but packaging is a
real product cost:

- Windows requires 64-bit target architecture and the Microsoft Visual C++
  Redistributable.
- Linux support is distro/architecture constrained and needs system libraries
  such as OpenSSL certificates and ALSA.
- macOS is supported, but C++ SDK packaging involves native framework assets.
- Go/C++ SDK paths carry native binary/header setup; Python is not acceptable
  as the normal desktop runtime.

An Azure adapter should therefore be an optional enterprise feature or sidecar
until packaging is proven in Blacksmith/GitHub on Windows, macOS, and Linux.

### Auth and endpoint modes

Azure supports API key/region style setup, endpoint-based setup, and Microsoft
Entra ID/managed identity patterns. Enterprise deployments may also require
private endpoints, custom domains, or sovereign-cloud endpoints. Settings must
model these choices explicitly:

- `azure_speech_key` or managed identity/token mode;
- `speech_region` for normal public regional endpoints;
- full `speech_endpoint` for custom/private/sovereign endpoints;
- resource id for managed identity where needed;
- language/locale and optional diarization-intermediate-results toggle.

Private endpoints are not just a networking checkbox: Microsoft docs require
custom domain/DNS setup and endpoint URL changes. The provider registry should
represent endpoint mode before Azure is selectable.

### Diarization and timing

Azure is the better enterprise diarization candidate. But the early-interim
speaker IDs can be `Unknown`, and speaker identity is generic provider-assigned
state, not a stable human identity. AudioGraph should ingest Azure speaker IDs
as provider speaker revisions, not overwrite stable local speaker labels.

The short-audio REST API does not support real-time diarization, so the
real-time path should not be implemented as a simple REST probe plus upload.

### Health and model discovery

Azure readiness must validate:

- key or Entra token acquisition;
- endpoint/region consistency;
- SDK/native dependency availability;
- real-time endpoint connectivity without starting capture;
- language support and diarization capability for the chosen endpoint.

For headless CI, use file-based or metadata-only validation. Do not require a
microphone to prove credentials or endpoint shape.

## Provider Registry Implications

Before either provider is exposed, add registry fields for:

- `transport`: `grpc_bidi`, `sdk_native`, `sidecar_process`, in addition to the
  existing WebSocket shapes;
- `auth_lifecycle`: service-account/ADC, Azure key, Azure Entra token, private
  endpoint/custom domain;
- `packaging`: native SDK assets, system library prerequisites, supported OS and
  architecture matrix;
- `speaker_semantics`: streaming diarization supported, batch-only
  diarization, unknown interim speaker, provider speaker id stability;
- `health_probe`: metadata-only, token-only, SDK dependency probe, live
  env-gated smoke.

## Concrete Follow-Ups

Create implementation Seeds only after the WebSocket-first provider queue is
stable:

- Google adapter spike: choose `tonic` direct gRPC vs sidecar, prove
  `StreamingRecognize` in Rust, and define service-account/ADC local credential
  storage without plaintext config leakage.
- Azure adapter spike: prove SDK/native packaging on Windows/macOS/Linux,
  define endpoint/private-domain settings, and map `ConversationTranscriber`
  speaker IDs into SpeakerTimeline revisions.
- Shared registry work: add `grpc_bidi` and `sdk_native` transport metadata
  before either provider is selectable.

## Sources

- Google Chirp 3 model docs:
  <https://cloud.google.com/speech-to-text/v2/docs/chirp_3-model>
- Google Speech-to-Text v2 streaming docs:
  <https://cloud.google.com/speech-to-text/v2/docs/streaming-recognize>
- Google Speech-to-Text v2 RPC reference:
  <https://cloud.google.com/speech-to-text/v2/docs/reference/rpc/google.cloud.speech.v2>
- Rust `google-cloud-speech-v2` crate docs:
  <https://docs.rs/google-cloud-speech-v2>
- Azure real-time diarization quickstart:
  <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/get-started-stt-diarization>
- Azure Speech SDK installation/platform requirements:
  <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/quickstarts/setup-platform>
- Azure speech recognition how-to:
  <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/how-to-recognize-speech>
- Azure Speech private endpoint docs:
  <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/speech-services-private-link>
- Azure Speech sovereign clouds docs:
  <https://learn.microsoft.com/en-us/azure/ai-services/speech-service/sovereign-clouds>
