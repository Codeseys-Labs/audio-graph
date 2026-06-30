/**
 * IPC contract between the React frontend and the Rust backend.
 *
 * Every type in this file mirrors a serde-serialized struct or enum on
 * the Rust side (look for matching `Serialize`/`Deserialize` derives
 * under `src-tauri/src/`). Changes here require a matching change in
 * Rust — and vice versa.
 *
 * Roughly grouped into:
 *   - Audio capture (`AudioSourceInfo`, `ProcessInfo`, `AudioChunk`).
 *   - Transcript + speaker (`TranscriptSegment`, `SpeakerInfo`).
 *   - Knowledge graph (`GraphSnapshot`, `GraphNode`, `GraphLink`,
 *     `GraphDelta`, `GraphStats`).
 *   - Pipeline status + events (`PipelineStatus`, `StageStatus`,
 *     `CaptureErrorPayload`, `CaptureBackpressurePayload`,
 *     `CaptureStorageFullPayload`, `AwsErrorPayload`).
 *   - Settings (`AppSettings` and the provider sub-types).
 *   - Gemini Live events (`GeminiTranscriptionEvent`,
 *     `GeminiResponseEvent`, `GeminiStatusEvent`,
 *     `GeminiErrorCategory`, `UsageMetadata`).
 *   - Error envelope (`AppErrorPayload`) — the structured shape Rust
 *     emits when a command returns `Result<_, AppError>`. See
 *     `src-tauri/src/error.rs`.
 *   - Store type (`AudioGraphStore`) — the Zustand slice that reuses
 *     most of the IPC types above.
 *
 * `ALLOWED_CREDENTIAL_KEYS` must stay in lockstep with
 * `src-tauri/src/credentials/mod.rs::ALLOWED_CREDENTIAL_KEYS`.
 */
import type {
  AudioPermissionRecoveryHint,
  AudioPermissionStatus,
  AudioSourceInfo,
  SourceId,
} from "../generated/audioSource";

export type {
  AudioChannelProvenanceKind,
  AudioDeviceKind,
  AudioFormatInfo,
  AudioPermissionKind,
  AudioPermissionRecoveryAction,
  AudioPermissionRecoveryActionKind,
  AudioPermissionRecoveryHint,
  AudioPermissionRecoveryPlatform,
  AudioPermissionStatus,
  AudioSampleFormat,
  AudioSourceCapabilities,
  AudioSourceChannelInfo,
  AudioSourceChannelLayout,
  AudioSourceChannelProvenance,
  AudioSourceInfo,
  AudioSourceType,
  SourceId,
} from "../generated/audioSource";

export type SegmentId = string;

export type SourceRecoveryIssueKind =
  | "unselected"
  | "unavailable"
  | "unsupported"
  | "permission"
  | "policy_conflict";

export interface SourceRecoveryIssue {
  kind: SourceRecoveryIssueKind;
  message: string;
  sourceId?: SourceId;
  sourceName?: string;
  permissionStatus?: AudioPermissionStatus;
  permissionRecovery?: AudioPermissionRecoveryHint;
}

export interface SourceRecoveryIntent {
  id: number;
  origin: "provider_setup";
  requestedAt: number;
  issues: SourceRecoveryIssue[];
}

export interface ProcessInfo {
  pid: number;
  name: string;
  exe_path: string | null;
}

// Transcript types
export interface TranscriptSegment {
  id: string; // UUID
  source_id: SourceId;
  speaker_id: string | null;
  speaker_label: string | null;
  text: string;
  start_time: number; // seconds since capture start
  end_time: number;
  confidence: number;
}

/** Interim streaming ASR hypothesis shown before a final transcript lands. */
export interface AsrPartialEvent {
  provider: string;
  source_id: SourceId;
  text: string;
  start_time: number;
  end_time: number;
  confidence: number;
  timestamp_ms: number;
}

export type AsrSpanStability = "partial" | "final";

/** Provider-neutral ASR span revision for event-sourced projections. */
export interface AsrSpanRevisionEvent {
  span_id: string;
  provider: string;
  source_id: SourceId;
  provider_item_id?: string | null;
  transcript_segment_id?: string | null;
  speaker_id?: string | null;
  speaker_label?: string | null;
  channel?: string | null;
  text: string;
  start_time: number;
  end_time: number;
  confidence: number;
  is_final: boolean;
  stability: AsrSpanStability;
  revision_number: number;
  supersedes?: string | null;
  turn_id?: string | null;
  end_of_turn: boolean;
  raw_event_ref?: string | null;
  capture_latency_ms?: number | null;
  asr_latency_ms?: number | null;
  received_at_ms: number;
}

export type DiarizationSpanStability = "provisional" | "stable" | "final";

/** Provider-neutral speaker timeline span revision for diffable diarization. */
export interface DiarizationSpanRevisionEvent {
  span_id: string;
  provider: string;
  timeline_id: string;
  source_id?: SourceId | null;
  speaker_id?: string | null;
  speaker_label?: string | null;
  channel?: string | null;
  start_time: number;
  end_time: number;
  confidence?: number | null;
  is_final: boolean;
  stability: DiarizationSpanStability;
  revision_number: number;
  supersedes?: string | null;
  basis_asr_span_ids: string[];
  basis_transcript_segment_ids: string[];
  raw_event_ref?: string | null;
  capture_latency_ms?: number | null;
  asr_latency_ms?: number | null;
  received_at_ms: number;
}

export type TurnEventKind =
  | "speech_started"
  | "speech_final"
  | "utterance_end"
  | "eager_end_of_turn"
  | "end_of_turn"
  | "turn_resumed"
  | "local_window";

/** Normalized speech turn lifecycle event from Deepgram/local providers. */
export interface TurnLifecycleEvent {
  provider: string;
  source_id: SourceId;
  kind: TurnEventKind;
  text?: string | null;
  start_time?: number | null;
  end_time?: number | null;
  confidence?: number | null;
  turn_index?: number | null;
  timestamp_ms: number;
}

export type AgentStatusState = "idle" | "running" | "error";

export interface AgentStatusEvent {
  state: AgentStatusState;
  source_segment_id?: string | null;
  message?: string | null;
  timestamp_ms: number;
}

export type AgentProposalKind = "note" | "question" | "graph_suggestion";

export interface AgentProposalEvent {
  id: string;
  source_segment_id: string;
  source_id: SourceId;
  speaker_label?: string | null;
  kind: AgentProposalKind;
  title: string;
  body: string;
  confidence: number;
  created_at_ms: number;
}

export interface AgentActionResult {
  proposal_id: string;
  action: string;
  message: string;
  graph_updated: boolean;
  timestamp_ms: number;
}

export type LiveAssistCardStatus = "pending" | "approved" | "dismissed";

export interface LiveAssistCardRecord {
  session_id: string;
  proposal: AgentProposalEvent;
  status: LiveAssistCardStatus;
  source_span_ids: string[];
  graph_context_ids: string[];
  outcome?: AgentActionResult | null;
  projection_patch_sequence?: number | null;
  created_at_ms: number;
  updated_at_ms: number;
}

// ---------------------------------------------------------------------------
// Knowledge graph internal types
// ---------------------------------------------------------------------------

export interface GraphEntity {
  id: string;
  name: string;
  entity_type: string; // PERSON, ORG, LOCATION, EVENT, CONCEPT
  mention_count: number;
  first_seen: number;
  last_seen: number;
  aliases: string[];
  description?: string;
  speakers: string[];
}

// ---------------------------------------------------------------------------
// react-force-graph compatible types (sent from backend via events)
// ---------------------------------------------------------------------------

/** A graph node ready for react-force-graph rendering. */
export interface GraphNode {
  id: string;
  name: string;
  entity_type: string;
  /** Node size (based on mention_count). */
  val: number;
  /** Hex color by entity_type. */
  color: string;
  first_seen: number;
  last_seen: number;
  mention_count: number;
  description?: string;
}

/** A graph link ready for react-force-graph rendering. */
export interface GraphLink {
  /** Stable edge id when provided by the backend. */
  id?: string;
  /** Source node id. */
  source: string | GraphNode;
  /** Target node id. */
  target: string | GraphNode;
  relation_type: string;
  weight: number;
  color: string;
  label?: string;
}

/** A single graph edge as carried by incremental graph deltas. */
export interface GraphDeltaEdge {
  id: string;
  source: string;
  target: string;
  relation_type: string;
  weight: number;
  color: string;
  label?: string;
}

/**
 * Incremental graph changes emitted by the backend on `graph-delta`.
 *
 * Contract: deltas are low-latency updates between authoritative
 * `graph-update` snapshots. If a snapshot is received after a delta, the
 * snapshot replaces canonical graph state; only view-only force-layout fields
 * are preserved by the store. Stale deltas must not be replayed after a newer
 * snapshot unless a future sequence/basis field proves they belong after it.
 */
export interface GraphDelta {
  added_nodes: GraphNode[];
  updated_nodes: GraphNode[];
  added_edges: GraphDeltaEdge[];
  /**
   * Edges whose weight/label changed since the last delta. Merged onto
   * existing links by `id`. Optional for backwards/test compatibility; the
   * backend always sends it (possibly empty).
   */
  updated_edges?: GraphDeltaEdge[];
  removed_node_ids: string[];
  removed_edge_ids: string[];
  timestamp: number;
}

/** Aggregate graph statistics. */
export interface GraphStats {
  total_nodes: number;
  total_edges: number;
  total_episodes: number;
}

/** A point-in-time snapshot of the knowledge graph for frontend rendering. */
export interface GraphSnapshot {
  /** All nodes in react-force-graph format. */
  nodes: GraphNode[];
  /** All links in react-force-graph format. */
  links: GraphLink[];
  /** Aggregate statistics. */
  stats: GraphStats;
}

// Pipeline status types
export type StageStatus =
  | { type: "Idle" }
  | { type: "Running"; processed_count: number }
  | { type: "Error"; message: string };

export interface PipelineStatus {
  capture: StageStatus;
  pipeline: StageStatus;
  asr: StageStatus;
  diarization: StageStatus;
  entity_extraction: StageStatus;
  graph: StageStatus;
}

/**
 * Per-stage latency sample emitted by the Rust backend. Keys match
 * `PipelineStatus` where possible; future stages such as `agent` can be
 * added without changing the status enum.
 */
export interface PipelineLatencyEvent {
  stage: keyof PipelineStatus | "agent" | "turn_detection";
  source_id?: string | null;
  segment_id?: string | null;
  latency_ms: number;
  timestamp_ms: number;
}

export type ProcessedAudioConsumerStage =
  | "speech"
  | "notes"
  | "native_converse"
  | "realtime_agent"
  | "other";

export type ProcessedAudioDropPolicy = "drop_oldest" | "drop_newest";

export type ProcessedAudioMixingMode = "per_source" | "mixed_mono";

export type ProcessedAudioSourceFilter =
  | { type: "all" }
  | { type: "sources"; source_ids: SourceId[] };

export interface ProcessedAudioConsumerHealth {
  id: string;
  stage: ProcessedAudioConsumerStage;
  provider?: string | null;
  conflict_group?: string | null;
  active: boolean;
  queue_len: number;
  queue_capacity?: number | null;
  sent_chunks: number;
  dropped_chunks: number;
  drop_policy: ProcessedAudioDropPolicy;
  source_filter: ProcessedAudioSourceFilter;
  mixing_mode: ProcessedAudioMixingMode;
}

export interface ProcessedAudioConsumerHealthPayload {
  consumers: ProcessedAudioConsumerHealth[];
}

// Speaker types
export interface SpeakerInfo {
  id: string;
  label: string;
  color: string; // hex color for UI
  total_speaking_time: number; // seconds
  segment_count: number;
}

// Capture configuration
export interface CaptureSessionConfig {
  source_id: SourceId;
  sample_rate?: number;
  channels?: number;
}

// Event payloads
export interface CaptureErrorPayload {
  source_id: string;
  error: string;
  recoverable: boolean;
}

export interface CaptureBackpressurePayload {
  source_id: string;
  is_backpressured: boolean;
}

export interface PersistenceQueueBackpressurePayload {
  writer: string;
  is_backpressured: boolean;
  queue_capacity: number;
  dropped_count: number;
}

/**
 * Payload for `capture-storage-full` events — emitted when a persistence
 * write (transcript JSONL, graph snapshot) fails because the underlying
 * storage is full. `bytes_written` is best-effort and may be `0` when the
 * error happened on the initial open; `bytes_lost` is the size of the
 * buffer the app was trying to persist.
 */
export interface CaptureStorageFullPayload {
  path: string;
  bytes_written: number;
  bytes_lost: number;
}

// ---------------------------------------------------------------------------
// AWS error taxonomy (ag#13)
// ---------------------------------------------------------------------------

/**
 * Structured classification of aws-sdk errors surfaced by the backend, keyed
 * on `category`. Matches Rust `crate::aws_util::UiAwsError` serialized with
 * `#[serde(tag = "category", rename_all = "snake_case")]`.
 *
 * The frontend uses `category` to pick an `aws.error.*` i18n key and to
 * decide which recovery hint to show (e.g. "check Settings → AWS").
 */
export type UiAwsError =
  | { category: "invalid_access_key" }
  | { category: "signature_mismatch" }
  | { category: "expired_token" }
  | { category: "access_denied"; permission: string | null }
  | { category: "region_not_supported"; region: string }
  | { category: "network_unreachable" }
  | { category: "unknown"; message: string };

/**
 * Payload for the `aws-error` event (ag#13). `error` is the structured
 * classification; `raw_message` is the original aws-sdk error string,
 * retained for debugging / disclosure when the category is `unknown`.
 */
export interface AwsErrorPayload {
  error: UiAwsError;
  raw_message: string;
}

// ---------------------------------------------------------------------------
// Model management types
// ---------------------------------------------------------------------------

export interface ModelInfo {
  name: string;
  filename: string;
  url: string;
  size_bytes: number | null;
  is_downloaded: boolean;
  is_valid: boolean;
  description: string;
  local_path: string | null;
}

export interface DownloadProgress {
  /** Stable identifier — matches `ModelInfo.filename`. */
  model_id: string;
  /** Display name kept for legacy consumers keyed off the friendly label. */
  model_name: string;
  bytes_downloaded: number;
  /** `0` when the server omitted `Content-Length` (treat as unknown). */
  total_bytes: number;
  /** Wall-clock milliseconds since the download started. */
  elapsed_ms: number;
  percent: number;
  /** One of: "downloading", "complete", "error" */
  status: string;
}

// ---------------------------------------------------------------------------
// API endpoint configuration
// ---------------------------------------------------------------------------

/** Configuration for an OpenAI-compatible API endpoint. */
export interface ApiEndpointConfig {
  /** Base URL, e.g. "https://openrouter.ai/api/v1" or "http://localhost:11434/v1" */
  endpoint: string;
  /** Bearer token. Omit for local servers (Ollama, LM Studio). */
  apiKey?: string;
  /** Model identifier, e.g. "gpt-4o-mini", "llama3.2", "qwen2.5:3b" */
  model: string;
}

// ---------------------------------------------------------------------------
// Settings & model readiness types
// ---------------------------------------------------------------------------

/** Model readiness status (matches Rust ModelReadiness enum) */
export type ModelReadiness = "Ready" | "NotDownloaded" | "Invalid";

/** Aggregate model status (matches Rust ModelStatus struct) */
export interface ModelStatus {
  whisper: ModelReadiness;
  llm: ModelReadiness;
  sortformer: ModelReadiness;
}

/** AWS credential source (matches Rust AwsCredentialSource enum with serde tag) */
export type AwsCredentialSource =
  | { type: "default_chain" }
  | { type: "profile"; name: string }
  | {
      type: "access_keys";
      access_key?: string;
      /** Legacy import-only secret material; never persisted in config.yaml. */
      secret_key?: string | null;
      /** Legacy import-only STS token; never persisted in config.yaml. */
      session_token?: string | null;
    };

/** ASR provider configuration (matches Rust AsrProvider enum with serde tag) */
export type AsrProvider =
  | { type: "local_whisper" }
  | { type: "api"; endpoint: string; api_key?: string; model: string }
  | {
      type: "aws_transcribe";
      region: string;
      language_code: string;
      credential_source: AwsCredentialSource;
      enable_diarization: boolean;
    }
  | {
      type: "deepgram";
      api_key?: string;
      model: string;
      enable_diarization: boolean;
      endpointing_ms?: number;
      utterance_end_ms?: number;
      vad_events?: boolean;
      eot_threshold?: number;
      eager_eot_threshold?: number;
      eot_timeout_ms?: number;
      max_speakers?: number;
    }
  | { type: "assemblyai"; api_key?: string; enable_diarization: boolean }
  | {
      type: "soniox";
      api_key?: string;
      model: string;
      enable_diarization: boolean;
      enable_language_identification: boolean;
      language_hints?: string[];
      max_speakers?: number;
    }
  | {
      type: "openai_realtime";
      api_key?: string;
      model: string;
      language?: string | null;
    }
  | {
      type: "sherpa_onnx";
      model_dir: string;
      enable_endpoint_detection: boolean;
    }
  | {
      type: "moonshine";
      model_dir: string;
      enable_speaker_hints: boolean;
    };

/** LLM provider configuration (matches Rust LlmProvider enum with serde tag) */
export type LlmProvider =
  | { type: "local_llama" }
  | { type: "api"; endpoint: string; api_key?: string; model: string }
  | {
      type: "openrouter";
      model: string;
      base_url: string;
      provider_order?: string[] | null;
      include_usage_in_stream: boolean;
      api_key?: string;
    }
  | {
      type: "aws_bedrock";
      region: string;
      model_id: string;
      credential_source: AwsCredentialSource;
    }
  | { type: "mistralrs"; model_id: string };

/**
 * Settings shape for the first-class OpenRouter provider (ADR-0005).
 * Mirrors the payload inside Rust `LlmProvider::OpenRouter`.
 */
export interface OpenRouterSettings {
  model: string;
  base_url: string;
  provider_order?: string[] | null;
  include_usage_in_stream: boolean;
}

export type OpenRouterDataCollectionPolicy = "allow" | "deny";

export type OpenRouterQuantization =
  | "int4"
  | "int8"
  | "fp4"
  | "fp6"
  | "fp8"
  | "fp16"
  | "bf16"
  | "fp32"
  | "unknown";

export type OpenRouterRoutingSortMetric = "price" | "throughput" | "latency";

export type OpenRouterRoutingSort =
  | OpenRouterRoutingSortMetric
  | {
      by: OpenRouterRoutingSortMetric;
      partition?: "model" | "none";
    };

export type OpenRouterPerformancePreference =
  | number
  | {
      p50?: number;
      p75?: number;
      p90?: number;
      p99?: number;
    };

export interface OpenRouterMaxPrice {
  prompt?: number;
  completion?: number;
  request?: number;
  image?: number;
}

export interface OpenRouterRoutingPolicy {
  order: string[];
  only: string[];
  ignore: string[];
  allow_fallbacks?: boolean;
  require_parameters?: boolean;
  data_collection?: OpenRouterDataCollectionPolicy;
  zdr?: boolean;
  enforce_distillable_text?: boolean;
  quantizations: OpenRouterQuantization[];
  sort?: OpenRouterRoutingSort;
  preferred_min_throughput?: OpenRouterPerformancePreference;
  preferred_max_latency?: OpenRouterPerformancePreference;
  max_price?: OpenRouterMaxPrice;
}

/**
 * Pricing block on an OpenRouter model entry. Strings because OpenRouter
 * returns scientific-notation floats as strings (e.g. "0.000003").
 */
export interface OpenRouterPricing {
  prompt: string;
  completion: string;
}

/**
 * A single entry in the OpenRouter model catalog (`GET /api/v1/models`).
 * Returned by `list_openrouter_models_cmd`.
 */
export interface OpenRouterModel {
  id: string;
  name: string;
  context_length?: number | null;
  pricing?: OpenRouterPricing | null;
}

/**
 * Provider metadata returned by the saved-key `list_openrouter_providers_cmd`
 * (`GET /providers`). Mirrors Rust `openrouter::OpenRouterProvider`. The catalog
 * is metadata-only and may grow fields over time, so every non-identity field is
 * optional. `privacy_policy_url` / `terms_of_service_url` are the verifiable
 * data/privacy policy links — never fabricate them; absence means "unknown".
 */
export interface OpenRouterProvider {
  name: string;
  slug: string;
  privacy_policy_url?: string | null;
  terms_of_service_url?: string | null;
  status_page_url?: string | null;
  headquarters?: string | null;
  datacenters: string[];
}

/** Latency/throughput percentile stats on an OpenRouter endpoint. */
export interface OpenRouterPercentileStats {
  p50?: number | null;
  p75?: number | null;
  p90?: number | null;
  p99?: number | null;
}

/**
 * Per-endpoint pricing on an OpenRouter accelerator endpoint. Strings because
 * OpenRouter returns scientific-notation floats as strings (e.g. `"0.000003"`).
 */
export interface OpenRouterEndpointPricing {
  prompt?: string | null;
  completion?: string | null;
  request?: string | null;
  image?: string | null;
  image_token?: string | null;
  image_output?: string | null;
  audio?: string | null;
  audio_output?: string | null;
  input_audio_cache?: string | null;
  input_cache_read?: string | null;
  input_cache_write?: string | null;
  input_cache_write_1h?: string | null;
  internal_reasoning?: string | null;
  web_search?: string | null;
  discount?: number | null;
}

/**
 * A single concrete provider endpoint serving a model, as returned by the
 * saved-key `list_openrouter_model_endpoints_cmd`
 * (`GET /models/{author}/{slug}/endpoints`). This is the accelerator endpoint
 * the view model normalizes — `provider_name`/`tag` identify the accelerator
 * (e.g. Cerebras, Groq, SambaNova), and the latency/throughput/quantization
 * fields drive the low-latency / high-throughput / Nitro ranking.
 */
export interface OpenRouterEndpoint {
  name?: string | null;
  model_id?: string | null;
  model_name?: string | null;
  context_length?: number | null;
  pricing?: OpenRouterEndpointPricing | null;
  provider_name?: string | null;
  tag?: string | null;
  quantization?: string | null;
  max_completion_tokens?: number | null;
  max_prompt_tokens?: number | null;
  supported_parameters: string[];
  uptime_last_30m?: number | null;
  uptime_last_5m?: number | null;
  uptime_last_1d?: number | null;
  supports_implicit_caching?: boolean | null;
  latency_last_30m?: OpenRouterPercentileStats | null;
  throughput_last_30m?: OpenRouterPercentileStats | null;
  status?: unknown;
}

export interface OpenRouterEndpointArchitecture {
  tokenizer?: string | null;
  instruct_type?: string | null;
  modality?: string | null;
  input_modalities: string[];
  output_modalities: string[];
}

/**
 * Endpoint catalog for one model, returned by
 * `list_openrouter_model_endpoints_cmd`. Mirrors Rust
 * `openrouter::OpenRouterModelEndpoints`.
 */
export interface OpenRouterModelEndpoints {
  id?: string | null;
  name?: string | null;
  created?: number | null;
  description?: string | null;
  architecture?: OpenRouterEndpointArchitecture | null;
  endpoints: OpenRouterEndpoint[];
}

/** LLM API configuration for persistence */
export interface LlmApiConfig {
  endpoint: string;
  api_key?: string | null;
  model: string;
  max_tokens: number;
  temperature: number;
}

/**
 * TTS provider configuration (matches Rust `TtsProvider` enum, plan A1 +
 * ADR-0004). v1 ships `none` (TTS disabled) and `deepgram_aura`; local
 * engines (Kokoro, Piper, Coqui) will be additional variants in their own
 * plans.
 *
 * The Deepgram API key for `deepgram_aura` reuses `deepgram_api_key` -- the
 * same credential slot used by the STT provider.
 */
export type TtsProviderConfig =
  | { type: "none" }
  | {
      type: "deepgram_aura";
      /** Aura voice id, e.g. `aura-asteria-en` or `aura-2-thalia-en`. */
      voice: string;
      /** PCM sample rate in Hz; Aura streaming default is 24000. */
      sample_rate: number;
      /** Speed multiplier (Aura accepts 0.7..=1.5). */
      speed: number;
    };

export type DiarizationMode = "off" | "provider" | "local" | "hybrid";
export type DiarizationSpeakerCount = "auto" | "fixed" | "unbounded";

/** Non-secret user policy for local/provider/hybrid speaker attribution. */
export interface DiarizationSettings {
  mode: DiarizationMode;
  speaker_count: DiarizationSpeakerCount;
  max_speakers?: number | null;
}

export type PrivacyMode =
  | "local_only"
  | "byok_cloud"
  | "cloud_disabled_readiness_only"
  | "org_promotion";

// ---------------------------------------------------------------------------
// Provider registry
// ---------------------------------------------------------------------------

/** Provider capability stage (matches Rust `ProviderStage`). */
export type ProviderStage =
  | "asr"
  | "diarization"
  | "llm"
  | "tts"
  | "realtime_agent";

/** Implementation readiness (matches Rust `ProviderStatus`). */
export type ProviderStatus =
  | "implemented"
  | "planned"
  | "watch"
  | "enterprise_watch"
  | "rejected";

/** Runtime transport family (matches Rust `ProviderTransport`). */
export type ProviderTransport =
  | "local"
  | "http"
  | "web_socket"
  | "rest_init_web_socket"
  | "aws_sdk"
  | "grpc_bidi"
  | "sdk_native"
  | "sidecar_process";

/**
 * Audio source fan-out policy (matches Rust `ProviderSourcePolicy`).
 *
 * `multi_source_mixed` means selected sources are summed into one provider
 * stream; `multi_source_independent` means each source can be processed as its
 * own unit; `single_session` means the provider runtime can handle only one
 * active source today.
 */
export type ProviderSourcePolicy =
  | "multi_source_independent"
  | "multi_source_mixed"
  | "single_session";

/** Model catalog strategy (matches Rust `ModelCatalogPolicy`). */
export type ModelCatalogPolicy =
  | "none"
  | "fixed"
  | "local_files"
  | "remote_command"
  | "user_supplied";

/** Provider event contract (matches Rust `ProviderEventSemantics`). */
export type ProviderEventSemantics =
  | "transcript_final_only"
  | "transcript_partial_final"
  | "transcript_partial_final_turns"
  | "native_realtime_audio_text";

/** Provider audio frame format (matches Rust `ProviderAudioFrameFormat`). */
export type ProviderAudioFrameFormat = "f32" | "pcm_s16_le" | "wav_pcm_s16_le";

/**
 * Provider audio transport encoding (matches Rust
 * `ProviderAudioTransportEncoding`).
 */
export type ProviderAudioTransportEncoding =
  | "local_buffer"
  | "web_socket_binary"
  | "web_socket_json_base64"
  | "aws_event_stream"
  | "grpc_streaming"
  | "sdk_native"
  | "multipart_wav";

export interface ProviderAudioFormat {
  sample_rate_hz: number;
  channels: number;
  frame_format: ProviderAudioFrameFormat;
}

export type ProviderAttributionMode =
  | "none"
  | "speaker"
  | "channel"
  | "speaker_and_channel"
  | "experimental_source_separation";

export type ProviderChannelLayout =
  | "mono"
  | "source_native"
  | "generated_speaker_lanes";

export type ProviderChannelLabelSemantics =
  | "none"
  | "provider_channel_index"
  | "source_channel_id"
  | "generated_speaker_lane";

export interface ProviderAttributionDescriptor {
  mode: ProviderAttributionMode;
  max_channels: number;
  accepted_layouts: ProviderChannelLayout[];
  channel_label_semantics: ProviderChannelLabelSemantics;
  requires_source_native_channels: boolean;
  capability_source_url?: string;
  capability_source_date?: string;
}

export interface ProviderAudioInputDescriptor {
  pipeline_format: ProviderAudioFormat;
  provider_format: ProviderAudioFormat;
  transport_encoding: ProviderAudioTransportEncoding;
  adapter_resamples: boolean;
  supports_multichannel: boolean;
  attribution: ProviderAttributionDescriptor;
}

/** Settings UI grouping hints (matches Rust `ProviderSettingsGroup`). */
export type ProviderSettingsGroup =
  | "basic"
  | "model_catalog"
  | "health"
  | "advanced";

/** Provider credential/auth shape (matches Rust `ProviderAuthLifecycle`). */
export type ProviderAuthLifecycle =
  | "none"
  | "saved_api_key"
  | "openai_compatible_api_key"
  | "aws_credential_chain"
  | "google_api_key_or_service_account"
  | "google_adc_or_service_account"
  | "azure_speech_key_or_entra_token";

/** Provider session shape (matches Rust `ProviderSessionLifecycle`). */
export type ProviderSessionLifecycle =
  | "noop"
  | "per_request"
  | "local_in_process"
  | "local_streaming_runtime"
  | "long_lived_web_socket"
  | "aws_streaming_sdk"
  | "grpc_bidirectional_stream"
  | "native_sdk_conversation"
  | "sidecar_process";

/** Provider keepalive strategy (matches Rust `ProviderKeepaliveStrategy`). */
export type ProviderKeepaliveStrategy =
  | "none"
  | "client_audio_stream"
  | "client_control_message"
  | "provider_specific";

/** Provider teardown strategy (matches Rust `ProviderCloseStrategy`). */
export type ProviderCloseStrategy =
  | "noop"
  | "request_completes"
  | "drop_runtime"
  | "web_socket_close_frame"
  | "end_stream_then_close_frame"
  | "terminate_message_then_close_frame"
  | "provider_close_message_then_close_frame"
  | "aws_end_stream"
  | "provider_specific";

/** App-visible data boundary (matches Rust `ProviderDataBoundary`). */
export type ProviderDataBoundary =
  | "local_only"
  | "user_configured_endpoint"
  | "user_configured_region"
  | "provider_account_boundary"
  | "vendor_cloud";

export type ProviderDataClass =
  | "audio"
  | "transcript_text"
  | "prompt_text"
  | "notes"
  | "graph_context"
  | "generated_text"
  | "generated_audio"
  | "speaker_labels"
  | "timing_metadata"
  | "model_catalog_metadata"
  | "provider_configuration"
  | "credential_auth"
  | "usage_metadata"
  | "provider_diagnostics";

export type ProviderPolicyStatus =
  | "unknown"
  | "not_applicable"
  | "user_configured"
  | "provider_docs_linked"
  | "enterprise_only";

export type ProviderSensitiveErrorPolicy =
  | "unknown"
  | "local_only"
  | "audio_graph_redacted";

export type ProviderEndpointMode =
  | "default_region"
  | "custom_endpoint"
  | "private_endpoint"
  | "sovereign_cloud";

export type ProviderPackagingRequirement =
  | "protobuf_grpc_client"
  | "native_sdk_assets"
  | "native_framework_assets"
  | "system_libraries"
  | "system_certificates"
  | "visual_cpp_redistributable"
  | "sidecar_process";

export type ProviderSpeakerLabelSupport =
  | "none"
  | "batch_only"
  | "streaming_provider_labels"
  | "streaming_unverified";

export interface ProviderSpeakerSemantics {
  label_support: ProviderSpeakerLabelSupport;
  interim_labels_may_be_unknown: boolean;
  speaker_ids_are_stable_identity: boolean;
  local_timeline_recommended: boolean;
}

export type ProviderHealthProbeKind =
  | "token_acquisition"
  | "metadata_only"
  | "sdk_dependency"
  | "endpoint_connectivity"
  | "streaming_rpc_availability"
  | "live_env_gated_smoke";

export type ProviderCredentialSchemaStatus =
  | "not_required"
  | "wired"
  | "required_not_wired"
  | "flexible_external";

export interface ProviderRoadmapMetadata {
  source_url: string;
  source_date: string;
  auth_schema: ProviderCredentialSchemaStatus;
  not_selectable_reason?: string;
}

export interface ProviderEnterpriseMetadata {
  endpoint_modes: ProviderEndpointMode[];
  packaging: ProviderPackagingRequirement[];
  speaker_semantics: ProviderSpeakerSemantics;
  health_probes: ProviderHealthProbeKind[];
}

export interface ProviderLifecycleDescriptor {
  auth: ProviderAuthLifecycle;
  session: ProviderSessionLifecycle;
  keepalive: ProviderKeepaliveStrategy;
  close: ProviderCloseStrategy;
}

export interface ProviderPrivacyDescriptor {
  data_leaves_device: boolean;
  data_boundary: ProviderDataBoundary;
  data_classes_sent: ProviderDataClass[];
  data_classes_returned: ProviderDataClass[];
  health_check_data_classes: ProviderDataClass[];
  cloud_transfer_acknowledgement_required: boolean;
  retention_policy: ProviderPolicyStatus;
  training_policy: ProviderPolicyStatus;
  deletion_policy: ProviderPolicyStatus;
  /** Official provider policy URL backing the *_policy claims; absent when no
   * verifiable source was found (claims then stay "unknown"). */
  policy_url?: string;
  /** ISO date the policy_url was verified; always paired with policy_url. */
  policy_url_source_date?: string;
  /** Official subprocessors / data-residency list URL, when published. */
  subprocessors_url?: string;
  enterprise_no_training_config: ProviderPolicyStatus;
  data_residency: ProviderPolicyStatus;
  sensitive_error_policy: ProviderSensitiveErrorPolicy;
  processor_identity?: string;
}

/** Local runtime model artifact shape (matches Rust `LocalModelKind`). */
export type LocalModelKind = "file" | "directory";

export interface LocalModelRequirement {
  model_id: string;
  kind: LocalModelKind;
  required_files: string[];
}

/**
 * Backend-owned provider metadata returned by `get_provider_registry_cmd`.
 * This is the intended source of truth for future provider settings rendering,
 * readiness checks, and provider expansion work.
 */
export interface ProviderDescriptor {
  id: string;
  display_name: string;
  stage: ProviderStage;
  settings_variant: string;
  status: ProviderStatus;
  transport: ProviderTransport;
  credential_keys: string[];
  required_features: string[];
  model_catalog: ModelCatalogPolicy;
  local_models: LocalModelRequirement[];
  fixed_model_catalog?: ProviderModelCatalogItem[];
  default_model?: string;
  health_check_command?: string;
  model_catalog_command?: string;
  source_policy?: ProviderSourcePolicy;
  source_policy_label?: string;
  event_semantics?: ProviderEventSemantics;
  settings_groups: ProviderSettingsGroup[];
  audio_input?: ProviderAudioInputDescriptor;
  lifecycle: ProviderLifecycleDescriptor;
  privacy: ProviderPrivacyDescriptor;
  enterprise?: ProviderEnterpriseMetadata;
  roadmap?: ProviderRoadmapMetadata;
  supports_streaming: boolean;
  supports_partial_revisions: boolean;
  supports_diarization: boolean;
}

/** Provider health/readiness state returned by `get_provider_readiness_cmd`. */
export type ProviderReadinessStatus =
  | "ready"
  | "missing_credentials"
  | "unchecked"
  | "error";

export interface ProviderCredentialReadiness {
  key: string;
  present: boolean;
}

export interface ProviderModelCatalogItem {
  id: string;
  display_name: string;
  is_default: boolean;
}

export type ProviderRuntimeReadinessStatus =
  | "feature_missing"
  | "model_missing"
  | "runtime_unavailable"
  | "load_failed"
  | "healthy";

export interface ProviderRuntimeReadiness {
  status: ProviderRuntimeReadinessStatus;
  message: string;
  required_feature?: string | null;
  runtime_version?: string | null;
  model_id?: string | null;
}

export interface ProviderReadiness {
  provider_id: string;
  status: ProviderReadinessStatus;
  message: string;
  automatic_probe_available?: boolean;
  checked_at?: number | null;
  stale: boolean;
  credential_epoch: number;
  credentials: ProviderCredentialReadiness[];
  model_count?: number | null;
  model_catalog?: ProviderModelCatalogItem[];
  voice_catalog?: ProviderModelCatalogItem[];
  language_catalog?: ProviderModelCatalogItem[];
  openrouter_models?: OpenRouterModel[];
  runtime?: ProviderRuntimeReadiness | null;
}

/**
 * Normalized TTS event emitted by the backend `TtsSession::events()` stream
 * (mirrors Rust `crate::tts::TtsEvent`). The audio playback subsystem
 * (Wave B) consumes `audio_chunk`; the UI consumes `status` for connection
 * indicators and `error` for toast surfaces.
 */
export type TtsEvent =
  | {
      type: "audio_chunk";
      /** i16 PCM samples; `Vec<i16>` on the Rust side. */
      samples: number[];
      /** Hz, e.g. 24000 for Aura linear16 default. */
      sample_rate: number;
    }
  | { type: "status"; kind: TtsStatusKind & Record<string, unknown> }
  | { type: "error"; kind: TtsErrorKind; message: string };

/** TTS lifecycle / acknowledgement signals (matches Rust `TtsStatus`). */
export type TtsStatusKind =
  | { kind: "connected" }
  | { kind: "flushed"; sequence: number }
  | { kind: "cleared" }
  | { kind: "metadata"; json: string }
  | { kind: "disconnected" }
  | { kind: "reconnecting"; attempt: number; backoff_secs: number }
  | { kind: "reconnected" };

/** TTS error category surfaced over IPC (matches Rust `TtsErrorKind`). */
export type TtsErrorKind =
  | "auth"
  | "rate_limit"
  | "bad_request"
  | "server"
  | "network"
  | "protocol"
  | "exhausted"
  | "unknown";

/** Audio processing settings */
export interface AudioSettings {
  sample_rate: number;
  channels: number;
}

/** Top-level application settings (matches Rust AppSettings) */
export interface AppSettings {
  asr_provider: AsrProvider;
  whisper_model: string;
  llm_provider: LlmProvider;
  openrouter_routing_policy?: OpenRouterRoutingPolicy | null;
  llm_api_config: LlmApiConfig | null;
  audio_settings: AudioSettings;
  gemini: GeminiSettings;
  diarization?: DiarizationSettings;
  privacy_mode?: PrivacyMode;
  /**
   * TTS provider config (plan A1 + ADR-0004). Defaults to `{ type: "none" }`
   * so chat replies stay text-only until the user opts in.
   */
  tts_provider: TtsProviderConfig;
  /**
   * Speak chat replies aloud through the configured TTS provider.
   * Default `false` — opt-in. When true and `tts_provider` is not
   * `{ type: "none" }`, each streaming chat reply is also piped to the
   * TTS provider and audio playback subsystem (Wave C / audio-graph-92c7).
   */
  speak_aloud: boolean;
  /**
   * Enable streaming / incremental prefill on supported local LLM backends
   * (llama.cpp only). Optional — older settings files omit it and the backend
   * defaults it to `false`. Only honored when `llm_provider` is a supporting
   * backend (see ADR-0012); ignored for mistral.rs and remote/API providers.
   */
  streaming_prefill?: boolean;
  /**
   * Runtime log-verbosity preference. One of
   * "off" | "error" | "warn" | "info" | "debug" | "trace".
   * Optional because older settings files won't have it; backend
   * treats `undefined` / missing as "info".
   */
  log_level?: string;
  /**
   * Demo mode — set once on first launch when no cloud credentials are
   * present. `undefined` means "not yet decided"; `true` means the app is
   * running local-only and the demo banner should show until models are
   * downloaded; `false` means the user has already configured providers.
   */
  demo_mode?: boolean;
  /**
   * Opt-in anonymous analytics (Sentry). Off by default — `undefined` /
   * missing is treated as disabled by the backend. Independent of file
   * logging (`log_level`); either, both, or neither may be enabled. No
   * transcripts, audio, credentials, or IP addresses are ever sent; reports
   * are anonymous and scrubbed. Mirrors the Rust `AppSettings.analytics_enabled`
   * field (default `Some(false)`).
   */
  analytics_enabled?: boolean;
}

/**
 * Runtime status of the anonymous-analytics (Sentry) subsystem, returned by
 * the `get_analytics_info` Tauri command. Mirrors the Rust `AnalyticsInfo`
 * shape. `pii_disabled` is always `true` (the client is initialised with
 * `send_default_pii = false`); `dsn_configured` reflects whether a Sentry DSN
 * is available to send to.
 */
export interface AnalyticsInfo {
  enabled: boolean;
  dsn_configured: boolean;
  pii_disabled: boolean;
}

// ---------------------------------------------------------------------------
// Gemini types
// ---------------------------------------------------------------------------

/** Gemini transcription event payload (matches Rust GeminiEvent::Transcription). */
export interface GeminiTranscriptionEvent {
  type: "transcription";
  text: string;
  is_final: boolean;
}

/** Gemini model response event payload (matches Rust GeminiEvent::ModelResponse). */
export interface GeminiResponseEvent {
  type: "model_response";
  text: string;
}

/** Per-modality token count (matches Rust ModalityTokenCount). */
export interface ModalityTokenCount {
  modality: string;
  tokenCount: number;
}

/**
 * Token usage metadata from Gemini Live `usageMetadata` frames.
 * Matches Rust {@link UsageMetadata} (camelCase preserved via serde).
 *
 * All counters are optional: the server only populates fields that are
 * meaningful for the current frame, and `undefined` means "not reported"
 * (distinct from `0`, which means "reported as zero"). Detail arrays are
 * empty when the server omits them.
 */
export interface UsageMetadata {
  promptTokenCount?: number;
  cachedContentTokenCount?: number;
  responseTokenCount?: number;
  toolUsePromptTokenCount?: number;
  thoughtsTokenCount?: number;
  totalTokenCount?: number;
  promptTokensDetails?: ModalityTokenCount[];
  cacheTokensDetails?: ModalityTokenCount[];
  responseTokensDetails?: ModalityTokenCount[];
  toolUsePromptTokensDetails?: ModalityTokenCount[];
}

/**
 * Categorized failure reason attached to every `gemini-status` event of
 * type `"error"`. Matches Rust {@link GeminiErrorCategory} (snake_case via
 * serde). The `kind` field is the routing key for i18n + toast severity
 * (auth/authExpired/rateLimit → warning, network → info, server/unknown
 * → error). See `gemini/mod.rs::classify_close_frame` /
 * `classify_tungstenite_error` for the mapping rules.
 */
export type GeminiErrorCategory =
  | { kind: "auth" }
  | { kind: "auth_expired" }
  | { kind: "rate_limit"; retry_after_secs?: number }
  | { kind: "server" }
  | { kind: "network" }
  | { kind: "unknown" };

/** Gemini status event payload (matches Rust GeminiEvent variants). */
export interface GeminiStatusEvent {
  type:
    | "connected"
    | "disconnected"
    | "error"
    | "reconnecting"
    | "reconnected"
    | "turn_complete";
  message?: string;
  /**
   * Present on `error` events. Carries the structured classification
   * determined at the error site so the frontend can route to the
   * correct i18n key + toast severity without re-parsing `message`.
   */
  category?: GeminiErrorCategory;
  /** Present on `reconnecting` events — 1-based retry number. */
  attempt?: number;
  /** Present on `reconnecting` events — seconds until the next retry. */
  backoff_secs?: number;
  /**
   * Present on `reconnected` events. `true` means the reconnect used a
   * cached session-resumption handle (prior conversation context was
   * requested from the server); `false` means the new socket started from
   * a fresh session. Hint only — server-side rejection of the handle is
   * not observable here.
   */
  resumed?: boolean;
  /**
   * Present on `turn_complete` events when the server attached a
   * `usageMetadata` block to this frame. `undefined` when the frame
   * carries no usage accounting (e.g. mid-stream turn boundaries). The
   * frontend can safely sum `totalTokenCount` across turns for
   * cumulative session usage.
   */
  usage?: UsageMetadata;
}

/** A single Gemini / realtime-agent transcript entry for display. */
export interface GeminiTranscriptEntry {
  id: string;
  text: string;
  timestamp: number;
  is_final: boolean;
  source: "gemini" | "openai-realtime";
}

/**
 * OpenAI Realtime S2S assistant spoken-reply transcript event payload
 * (`openai-realtime-response`). Emitted by the converse driver's
 * `emit_transcript` for the OpenAI voice agent (sibling of `GeminiResponseEvent`,
 * but carries the `{ text, final }` shape the converse sink emits).
 */
export interface OpenAiRealtimeResponseEvent {
  text: string;
  final: boolean;
}

/**
 * Categorized failure reason on every `openai-realtime-status` event of type
 * `"error"`. Matches Rust {@link OpenAiRealtimeErrorCategory} (snake_case via
 * serde).
 */
export type OpenAiRealtimeErrorCategory =
  | { kind: "auth" }
  | { kind: "auth_expired" }
  | { kind: "rate_limit"; retry_after_secs?: number }
  | { kind: "server" }
  | { kind: "network" }
  | { kind: "unknown" };

/**
 * OpenAI Realtime S2S status event payload (`openai-realtime-status`). The
 * backend re-emits the serialized `OpenAiRealtimeEvent` envelope for
 * transport/lifecycle frames; `error` events carry the redacted message +
 * category. Mirrors {@link GeminiStatusEvent} so the frontend can route both
 * engines through one status handler.
 */
export interface OpenAiRealtimeStatusEvent {
  type: "connected" | "disconnected" | "error" | "reconnecting" | "reconnected";
  message?: string;
  category?: OpenAiRealtimeErrorCategory;
  attempt?: number;
  backoff_secs?: number;
  resumed?: boolean;
}

/** Gemini auth mode (matches Rust GeminiAuthMode enum with serde tag). */
export type GeminiAuthMode =
  | { type: "api_key"; api_key?: string }
  | {
      type: "vertex_ai";
      project_id: string;
      location: string;
      service_account_path?: string;
    };

/** Gemini settings (matches Rust GeminiSettings). */
export interface GeminiSettings {
  auth: GeminiAuthMode;
  model: string;
  /**
   * Prebuilt voice for converse-mode AUDIO sessions (B18 / ADR-0018). Empty /
   * omitted falls back to the engine default. Ignored by the notes/graph TEXT
   * pipeline. Serde-default on the backend, so optional here.
   */
  voice?: string;
}

// ---------------------------------------------------------------------------
// Session management types (v1: list + load transcript + delete)
// ---------------------------------------------------------------------------

export interface SessionMetadata {
  id: string;
  title: string | null;
  created_at: number; // unix millis
  ended_at: number | null; // unix millis
  duration_seconds: number | null;
  status: "active" | "complete" | "crashed";
  segment_count: number;
  speaker_count: number;
  entity_count: number;
  transcript_path: string;
  graph_path: string;
  /**
   * Soft-delete flag. Trashed sessions stay on disk but are hidden from
   * the default list view. Older sessions.json files (pre-SessionsBrowser
   * v2) omit this field — treat `undefined` as `false`.
   */
  deleted?: boolean;
  /**
   * Unix-millis timestamp of when the session was soft-deleted. Used for
   * the 30-day retention countdown before auto-purge.
   */
  deleted_at?: number | null;
}

export interface SessionRecoveryReport {
  discovered: number;
  recovered: number;
  skipped: number;
  errors: string[];
}

export type TranscriptEventStability = "partial" | "final";

export interface TranscriptEvent {
  span_id: string;
  provider: string;
  source_id: string;
  provider_item_id?: string | null;
  transcript_segment_id?: string | null;
  speaker_id?: string | null;
  speaker_label?: string | null;
  channel?: string | null;
  text: string;
  start_time: number;
  end_time: number;
  confidence: number;
  is_final: boolean;
  stability: TranscriptEventStability;
  revision_number: number;
  supersedes?: string | null;
  turn_id?: string | null;
  end_of_turn: boolean;
  raw_event_ref?: string | null;
  capture_latency_ms?: number | null;
  asr_latency_ms?: number | null;
  received_at_ms: number;
}

export type ProjectionKind = "notes" | "graph";

export interface GraphNodeDraft {
  id: string;
  name: string;
  entity_type: string;
  description?: string | null;
}

export type ProjectionOperation =
  | {
      type: "upsert_note";
      id: string;
      title: string;
      body: string;
      tags: string[];
    }
  | {
      type: "delete_note";
      id: string;
    }
  | {
      type: "reorder_note";
      id: string;
      after_id?: string | null;
    }
  | {
      type: "upsert_graph_node";
      id: string;
      name: string;
      entity_type: string;
      description?: string | null;
    }
  | {
      type: "remove_graph_node";
      id: string;
    }
  | {
      type: "invalidate_graph_node";
      id: string;
    }
  | {
      type: "upsert_graph_edge";
      id: string;
      source: string;
      target: string;
      relation_type: string;
      label?: string | null;
      weight: number;
    }
  | {
      type: "remove_graph_edge";
      id: string;
    }
  | {
      type: "invalidate_graph_edge";
      id: string;
    }
  | {
      type: "strengthen_graph_edge";
      id: string;
      weight_delta: number;
    }
  | {
      type: "weaken_graph_edge";
      id: string;
      weight_delta: number;
    }
  | {
      type: "merge_graph_nodes";
      source_id: string;
      target_id: string;
    }
  | {
      type: "split_graph_node";
      id: string;
      replacement_nodes: GraphNodeDraft[];
    };

export interface ProjectionPatch {
  sequence: number;
  kind: ProjectionKind;
  llm_request_id: string;
  basis: unknown;
  operations: ProjectionOperation[];
  confidence: number;
  provenance: unknown;
  queued_at_ms?: number | null;
  generation_latency_ms?: number | null;
  apply_latency_ms?: number | null;
  created_at_ms: number;
}

export interface MaterializedNote {
  id: string;
  title: string;
  body: string;
  tags: string[];
  updated_by_sequence: number;
  updated_at_ms: number;
  basis: unknown;
  provenance: unknown;
}

export interface MaterializedNotes {
  schema_version: number;
  session_id: string;
  last_sequence: number;
  notes: MaterializedNote[];
}

export interface MaterializedGraphNode {
  id: string;
  name: string;
  entity_type: string;
  description?: string | null;
  confidence: number;
  valid_from_ms: number;
  valid_until_ms?: number | null;
  updated_by_sequence: number;
  updated_at_ms: number;
  basis: unknown;
  provenance: unknown;
}

export interface MaterializedGraphEdge {
  id: string;
  source: string;
  target: string;
  relation_type: string;
  label?: string | null;
  weight: number;
  confidence: number;
  valid_from_ms: number;
  valid_until_ms?: number | null;
  updated_by_sequence: number;
  updated_at_ms: number;
  basis: unknown;
  provenance: unknown;
}

export interface MaterializedGraph {
  schema_version: number;
  session_id: string;
  last_sequence: number;
  nodes: MaterializedGraphNode[];
  edges: MaterializedGraphEdge[];
}

export type PromotionSourceObjectType =
  | "materialized_note"
  | "graph_node_fact"
  | "graph_edge_fact"
  | "live_assist_card"
  | "transcript_span";

export type PromotionStatus =
  | "draft"
  | "redaction_required"
  | "ready_to_promote"
  | "rejected"
  | "queued"
  | "validated"
  | "blocked_by_stale_source"
  | "blocked_by_redaction"
  | "approved_local"
  | "queued_sync"
  | "synced"
  | "failed"
  | "revoked";

export type OrgKnowledgeKind =
  | "note"
  | "graph_fact"
  | "live_card"
  | "decision"
  | "commitment"
  | "question"
  | "risk";

export type OrgKnowledgeState =
  | "active"
  | "superseded"
  | "retracted"
  | "deleted"
  | "retention_expired"
  | "purge_pending"
  | "purged";

export type PromotionConflictState =
  | "none"
  | "remote_newer"
  | "local_redaction_changed"
  | "source_superseded"
  | "acl_conflict"
  | "retention_conflict"
  | "tombstone_conflict"
  | "manual_resolution_required";

export type PromotionSyncTargetKind =
  | "surrealdb_remote"
  | "api_server"
  | "file_export"
  | "disabled";

export type PromotionSyncStatus =
  | "not_configured"
  | "not_synced"
  | "queued"
  | "sync_pending"
  | "in_flight"
  | "syncing"
  | "synced"
  | "conflict"
  | "permission_denied"
  | "redaction_required"
  | "retryable_error"
  | "permanent_error"
  | "auth_required"
  | "failed"
  | "revoked";

export type AclVisibility =
  | "private"
  | "workspace"
  | "org"
  | "principals"
  | "public_link";

export type AclInheritanceMode =
  | "none"
  | "workspace_default"
  | "collection_default"
  | "narrower_of_source_and_target";

export type RetentionCategory =
  | "personal_note"
  | "meeting_memory"
  | "org_knowledge"
  | "regulated"
  | "ephemeral";

export type DeleteBehavior =
  | "tombstone"
  | "retract_remote"
  | "purge_local_and_remote"
  | "preserve_approved_snapshot";

export interface PromotionActor {
  actor_user_id: string;
  actor_local_profile_id?: string | null;
  actor_device_id: string;
  delegated_service_id?: string | null;
}

export interface PromotionTarget {
  source_workspace_id?: string | null;
  target_org_id: string;
  target_workspace_id: string;
  target_collection_id?: string | null;
}

export interface PromotionSourceProvenance {
  asr_provider?: string | null;
  source_id?: string | null;
  speaker_ids: string[];
  span_revisions: unknown[];
  llm?: unknown | null;
  confidence?: number | null;
  created_at_ms: number;
  updated_at_ms: number;
}

export interface PromotionSourceReference {
  source_object_type: PromotionSourceObjectType;
  source_object_id: string;
  source_object_version: string;
  source_session_id: string;
  source_span_ids: string[];
  source_projection_sequence?: number | null;
  source_basis_hash: string;
  source_hash: string;
  source_basis: unknown;
  source_provenance: PromotionSourceProvenance;
}

export interface RedactionDiffEntry {
  field: string;
  reason: string;
  before_hash: string;
  after_hash: string;
}

export interface PromotionRedactionSummary {
  redaction_policy_id: string;
  redaction_policy_version: string;
  redaction_snapshot_hash: string;
  redaction_diff: RedactionDiffEntry[];
  redacted_fields: string[];
  manual_redaction_overrides: string[];
}

export interface ApprovedOrgPayload {
  kind: OrgKnowledgeKind;
  title?: string | null;
  body?: string | null;
  fields: Record<string, unknown>;
  approved_payload_hash: string;
}

export interface PromotionAcl {
  acl_policy_id: string;
  acl_visibility: AclVisibility;
  acl_principals: string[];
  acl_inheritance_mode: AclInheritanceMode;
}

export interface PromotionRetention {
  retention_policy_id: string;
  retention_legal_basis: string;
  retention_category: RetentionCategory;
  expires_at_ms?: number | null;
  delete_behavior: DeleteBehavior;
}

export interface PromotionLineage {
  parent_promotion_id?: string | null;
  supersedes_promotion_id?: string | null;
  conflict_group_id?: string | null;
}

export interface PromotionSyncSnapshot {
  target_kind: PromotionSyncTargetKind;
  sync_target_id?: string | null;
  status: PromotionSyncStatus;
  remote_id?: string | null;
  remote_revision?: string | null;
  remote_etag?: string | null;
  sync_error_code?: string | null;
  sync_error_message_redacted?: string | null;
}

export interface PromotionEvent {
  id: string;
  schema_version: number;
  created_at_ms: number;
  actor: PromotionActor;
  target: PromotionTarget;
  source: PromotionSourceReference;
  redaction: PromotionRedactionSummary;
  reviewer_user_id: string;
  approved_payload_hash: string;
  payload_snapshot: ApprovedOrgPayload;
  acl: PromotionAcl;
  retention: PromotionRetention;
  sync: PromotionSyncSnapshot;
  lineage: PromotionLineage;
  conflict_state: PromotionConflictState;
  requested_at_ms: number;
  approved_at_ms?: number | null;
  status: PromotionStatus;
}

export interface RedactionSnapshot {
  id: string;
  schema_version: number;
  promotion_event_id: string;
  source_object_type: PromotionSourceObjectType;
  source_object_id: string;
  policy_id: string;
  policy_version: string;
  redacted_fields: string[];
  removed_span_ids: string[];
  speaker_alias_map: Record<string, string>;
  entity_alias_map: Record<string, string>;
  manual_overrides: string[];
  payload_before_hash: string;
  payload_after_hash: string;
  approved_payload_hash: string;
  reviewed_by_user_id: string;
  reviewed_at_ms: number;
}

export interface OrgKnowledgeItem {
  id: string;
  schema_version: number;
  org_id: string;
  workspace_id: string;
  kind: OrgKnowledgeKind;
  current_revision_id: string;
  revision_number: number;
  title?: string | null;
  body?: string | null;
  tags: string[];
  content_hash: string;
  redacted_payload: ApprovedOrgPayload;
  graph_subject_id?: string | null;
  graph_object_id?: string | null;
  relation_type?: string | null;
  confidence?: number | null;
  source_promotion_event_id: string;
  promotion_event_ids: string[];
  source_local_object_fingerprint: string;
  source_session_fingerprint: string;
  provenance_summary: string;
  full_provenance_pointer: string;
  acl: PromotionAcl;
  retention: PromotionRetention;
  created_by_user_id: string;
  created_at_ms: number;
  updated_at_ms: number;
  valid_from_ms: number;
  valid_until_ms?: number | null;
  deleted_at_ms?: number | null;
  delete_reason?: string | null;
  state: OrgKnowledgeState;
  conflict_state: PromotionConflictState;
  sync_state: PromotionSyncSnapshot;
  remote_revision?: string | null;
}

export interface PromotionSyncState {
  promotion_event_id: string;
  target_kind: PromotionSyncTargetKind;
  remote_id?: string | null;
  remote_revision?: string | null;
  remote_etag?: string | null;
  queued_at_ms?: number | null;
  last_attempt_at_ms?: number | null;
  last_success_at_ms?: number | null;
  retry_count: number;
  status: PromotionSyncStatus;
  last_error_code?: string | null;
  last_error_message_redacted?: string | null;
}

export interface ProjectionSchedulerMetrics {
  jobs_started: number;
  completed_jobs: number;
  failed_jobs: number;
  generation_failures: number;
  coalesced_updates: number;
  coalesced_span_count: number;
  stale_discards: number;
  repair_jobs_started: number;
  follow_up_jobs_started: number;
  accepted_patches: number;
  apply_failures: number;
  tokens_used: number;
  last_job_lag_ms: number;
  max_job_lag_ms: number;
  last_generation_latency_ms: number;
  max_generation_latency_ms: number;
  last_apply_latency_ms: number;
  max_apply_latency_ms: number;
}

export type ProjectionTtftEstimateSource =
  | "default"
  | "configured"
  | "observed_generation";

export interface ProjectionSchedulerTelemetry {
  kind: ProjectionKind;
  ttft_estimate_ms: number;
  ttft_estimate_source: ProjectionTtftEstimateSource;
  in_flight_job_id?: string | null;
  in_flight_age_ms: number;
  in_flight_span_count: number;
  pending_span_count: number;
  metrics: ProjectionSchedulerMetrics;
}

export interface ProjectionSchedulersTelemetry {
  notes: ProjectionSchedulerTelemetry;
  graph: ProjectionSchedulerTelemetry;
}

export interface ProjectionMaterializedStatus {
  notes_last_sequence: number;
  note_count: number;
  graph_last_sequence: number;
  graph_node_count: number;
  graph_edge_count: number;
}

/** Non-secret runtime diagnostics from `get_projection_runtime_status_cmd`. */
export interface ProjectionRuntimeStatus {
  session_id: string;
  ledger_session_id: string;
  materialized_session_id: string;
  accepted_transcript_event_count: number;
  transcript_span_count: number;
  latest_asr_event_age_ms?: number | null;
  projection_event_writer_available: boolean;
  schedulers: ProjectionSchedulersTelemetry;
  materialized: ProjectionMaterializedStatus;
}

export type ProjectionReplayArtifactStatus =
  | "missing"
  | "current"
  | "stale"
  | "ahead";

export interface ProjectionReplayArtifactReport {
  present: boolean;
  status: ProjectionReplayArtifactStatus;
  stored_last_sequence: number;
  replayed_last_sequence: number;
  stored_item_count: number;
  replayed_item_count: number;
}

export interface ProjectionReplayEvaluationMetrics {
  note_operation_count: number;
  graph_operation_count: number;
  graph_retcon_operation_count: number;
  correction_patch_count: number;
  stale_discard_count: number;
  invalidated_graph_node_count: number;
  invalidated_graph_edge_count: number;
  active_graph_node_count: number;
  active_graph_edge_count: number;
  duplicate_active_node_key_count: number;
  duplicate_active_edge_key_count: number;
}

export interface ProjectionReplayKindLatencyMetrics {
  patch_count: number;
  measured_patch_count: number;
  missing_basis_timestamp_count: number;
  total_basis_to_patch_lag_ms: number;
  max_basis_to_patch_lag_ms: number;
  capture_asr: ProjectionReplayStageLatencyMetrics;
  asr_to_queue: ProjectionReplayStageLatencyMetrics;
  projection_queue: ProjectionReplayStageLatencyMetrics;
  generation: ProjectionReplayStageLatencyMetrics;
  apply: ProjectionReplayStageLatencyMetrics;
}

export interface ProjectionReplayStageLatencyMetrics {
  measured_count: number;
  total_ms: number;
  max_ms: number;
}

export interface ProjectionReplayLatencyMetrics
  extends ProjectionReplayKindLatencyMetrics {
  notes: ProjectionReplayKindLatencyMetrics;
  graph: ProjectionReplayKindLatencyMetrics;
}

/** Non-secret replay parity report from `get_projection_replay_report_cmd`. */
export interface ProjectionReplayReport {
  session_id: string;
  transcript_event_count: number;
  transcript_replay_error?: string | null;
  transcript_span_count: number;
  projection_event_count: number;
  projection_checked_patch_count: number;
  projection_invalid_basis_count: number;
  projection_replay_error?: string | null;
  replayed: ProjectionMaterializedStatus;
  notes_artifact: ProjectionReplayArtifactReport;
  graph_artifact: ProjectionReplayArtifactReport;
  evaluation: ProjectionReplayEvaluationMetrics;
  latency: ProjectionReplayLatencyMetrics;
}

/** Transcript plus graph payload returned when loading a past session. */
export interface LoadedSession {
  transcript: TranscriptSegment[];
  graph: GraphSnapshot;
  transcript_events: TranscriptEvent[];
  projection_events: ProjectionPatch[];
  live_assist_cards?: LiveAssistCardRecord[];
  notes?: MaterializedNotes | null;
  materialized_graph?: MaterializedGraph | null;
}

/**
 * Per-session token usage record returned by `get_session_usage` /
 * `get_current_session_usage`. Matches Rust `sessions::usage::SessionUsage`
 * (snake_case preserved by serde).
 */
export interface SessionUsage {
  session_id: string;
  prompt: number;
  response: number;
  cached: number;
  thoughts: number;
  tool_use: number;
  total: number;
  turns: number;
  llm_total: number;
  llm_turns: number;
  /** Unix millis of the last update; `0` means never updated. */
  updated_at: number;
}

/**
 * Aggregate token usage across every `~/.audiograph/usage/*.json` file.
 * Returned by the `get_lifetime_usage` command. Has no `session_id` — it's
 * a sum. `sessions` counts how many session files contributed.
 */
export interface LifetimeUsage {
  prompt: number;
  response: number;
  cached: number;
  thoughts: number;
  tool_use: number;
  total: number;
  turns: number;
  llm_total: number;
  llm_turns: number;
  sessions: number;
}

// ---------------------------------------------------------------------------
// Structured error payloads (matches Rust AppError enum)
// ---------------------------------------------------------------------------

/**
 * Structured error payload emitted by commands that return `Result<T, AppError>`.
 *
 * Shape: `{ code: "<snake_case>", message: <variant-specific-payload> }`.
 * Unit variants (e.g. `aws_credential_expired`) omit the `message` key
 * entirely — serde's internally-tagged enum does not emit `null` for empty
 * content. The `message` field is therefore `null | undefined` for those.
 *
 * Fallible commands should reject with this shape. Legacy string errors are
 * wrapped by the Rust command boundary as `{ code: "unknown", message }`, and
 * `errorToMessage` still falls back to `String(e)` for any older bare-string
 * rejection that reaches the UI.
 */
export type AppErrorPayload =
  | { code: "io"; message: string }
  | { code: "credential_missing"; message: { key: string } }
  | { code: "credential_file_error"; message: { reason: string } }
  | { code: "aws_credential_expired"; message?: null }
  | { code: "aws_region_invalid"; message: { region: string } }
  | { code: "gemini_rate_limited"; message?: null }
  | { code: "model_not_found"; message: { name: string } }
  | {
      code: "provider_unavailable";
      message: { provider: string; required_feature: string };
    }
  | {
      code: "privacy_policy_blocked";
      message: {
        mode: string;
        action: string;
        provider: string;
        data_classes: string[];
        reason: string;
      };
    }
  | { code: "session_invalid"; message: { reason: string } }
  | { code: "network_timeout"; message: { service: string } }
  | { code: "unknown"; message: string };

/**
 * Canonical list of credential keys accepted by the `save_credential` and
 * `delete_credential` Tauri commands, plus the non-secret credential presence
 * read path. Plaintext loadback is internal/test-only; normal UI flows should
 * use provider readiness and credential presence instead.
 *
 * IMPORTANT: this list must stay in sync with the Rust constant
 * `ALLOWED_CREDENTIAL_KEYS` in `src-tauri/src/credentials/mod.rs`. There
 * is no runtime cross-check — this is a convention only. If you add or
 * remove a credential field, update both places.
 */
export const ALLOWED_CREDENTIAL_KEYS: readonly string[] = [
  "openai_api_key",
  "cerebras_api_key",
  "openrouter_api_key",
  "groq_api_key",
  "together_api_key",
  "fireworks_api_key",
  "deepgram_api_key",
  "assemblyai_api_key",
  "soniox_api_key",
  "gladia_api_key",
  "speechmatics_api_key",
  "elevenlabs_api_key",
  "revai_api_key",
  "azure_speech_key",
  "gemini_api_key",
  "google_service_account_path",
  "aws_access_key",
  "aws_secret_key",
  "aws_session_token",
  "aws_profile",
  "aws_region",
];

/** Credential store for sensitive API keys. */
export interface CredentialStore {
  openai_api_key?: string;
  cerebras_api_key?: string;
  openrouter_api_key?: string;
  groq_api_key?: string;
  together_api_key?: string;
  fireworks_api_key?: string;
  deepgram_api_key?: string;
  assemblyai_api_key?: string;
  soniox_api_key?: string;
  gladia_api_key?: string;
  speechmatics_api_key?: string;
  elevenlabs_api_key?: string;
  revai_api_key?: string;
  azure_speech_key?: string;
  gemini_api_key?: string;
  google_service_account_path?: string;
  aws_access_key?: string;
  aws_secret_key?: string;
  aws_session_token?: string;
  aws_profile?: string;
  aws_region?: string;
}

/** Non-secret credential readiness returned by `load_credential_presence_cmd`. */
export interface CredentialPresence {
  key: string;
  present: boolean;
  source: "credentials_yaml" | "missing" | string;
}

// ---------------------------------------------------------------------------
// Chat types
// ---------------------------------------------------------------------------

export interface ChatMessage {
  role: "user" | "assistant" | "system";
  content: string;
}

export interface ChatResponse {
  message: ChatMessage;
  tokens_used: number;
}

/**
 * Streaming-chat token-delta event payload (plan A3 / ADR-0006).
 *
 * Fired from `start_streaming_chat` for every chunk of generated content.
 * `request_id` correlates back to the call that started the stream so a
 * UI showing multiple in-flight chats can route deltas correctly.
 */
export interface ChatTokenDeltaEvent {
  request_id: string;
  delta: string;
  finish_reason?: string;
}

/**
 * Streaming-chat terminal event payload. Fired exactly once per request —
 * on success, error, or cancel. `finish_reason`:
 *   - `"stop"` / `"length"` / `"content_filter"` etc. — normal LLM stop.
 *   - `"cancelled"`                                  — user pressed stop.
 *   - `"error: <message>"`                           — stream failed; the
 *     `full_text` is whatever was accumulated before the error.
 */
export interface ChatTokenDoneEvent {
  request_id: string;
  full_text: string;
  finish_reason: string;
  usage?: {
    prompt_tokens?: number;
    completion_tokens?: number;
    total_tokens?: number;
  };
}

/** Emitted after provider-reported chat/LLM token usage is persisted. */
export interface LlmUsageUpdateEvent {
  session_id: string;
  total_tokens: number;
  session_llm_total: number;
  session_llm_turns: number;
}

// ---------------------------------------------------------------------------
// Notifications (ADR-0011)
// ---------------------------------------------------------------------------

export type NotificationSeverity = "info" | "success" | "warning" | "error";

/** A transient (or sticky) user-facing notification rendered by the
 *  unified <Notifications> host. */
export interface AppNotification {
  id: string;
  severity: NotificationSeverity;
  message: string;
  /** When true, the notification stays until dismissed (no auto-timeout).
   *  Defaults to false (auto-dismiss). */
  sticky?: boolean;
  /** Optional inline action (e.g. "Open Settings", "Retry"). */
  action?: { label: string; onClick: () => void };
  createdAt: number;
}

/** Options accepted by `notify()`. `id` and `createdAt` are assigned by the
 *  store if omitted. */
export interface NotifyOptions {
  severity?: NotificationSeverity;
  message: string;
  sticky?: boolean;
  action?: { label: string; onClick: () => void };
  id?: string;
}

// ---------------------------------------------------------------------------
// Store type
// ---------------------------------------------------------------------------

/** Shape of the Zustand audio-graph store. */
export interface AudioGraphStore {
  // Audio sources
  audioSources: AudioSourceInfo[];
  selectedSourceIds: SourceId[];
  setAudioSources: (sources: AudioSourceInfo[]) => void;
  toggleSourceId: (id: SourceId) => void;
  removeSelectedSourceIds: (ids: SourceId[]) => void;
  clearSelectedSources: () => void;
  fetchSources: () => Promise<void>;
  sourceRecoveryIntent: SourceRecoveryIntent | null;
  requestSourceRecovery: (
    intent: Omit<SourceRecoveryIntent, "id" | "requestedAt">,
  ) => void;
  clearSourceRecoveryIntent: () => void;

  // Processes
  processes: ProcessInfo[];
  searchFilter: string;
  fetchProcesses: () => Promise<void>;
  setSearchFilter: (filter: string) => void;

  // Transcript
  samplePreviewActive: boolean;
  transcriptSegments: TranscriptSegment[];
  asrPartial: AsrPartialEvent | null;
  asrSpanRevisions: AsrSpanRevisionEvent[];
  diarizationSpanRevisions: DiarizationSpanRevisionEvent[];
  sessionTranscriptEvents: TranscriptEvent[];
  sessionProjectionEvents: ProjectionPatch[];
  materializedNotes: MaterializedNotes | null;
  materializedProjectionGraph: MaterializedGraph | null;
  turnEvents: TurnLifecycleEvent[];
  agentStatus: AgentStatusEvent | null;
  agentProposals: AgentProposalEvent[];
  liveAssistCards: LiveAssistCardRecord[];
  approvingAgentProposalIds: string[];
  addTranscriptSegment: (segment: TranscriptSegment) => void;
  setAsrPartial: (partial: AsrPartialEvent | null) => void;
  addAsrSpanRevision: (revision: AsrSpanRevisionEvent) => void;
  addDiarizationSpanRevision: (revision: DiarizationSpanRevisionEvent) => void;
  addTurnEvent: (event: TurnLifecycleEvent) => void;
  addProjectionPatch: (patch: ProjectionPatch) => void;
  setMaterializedNotes: (notes: MaterializedNotes) => void;
  setMaterializedProjectionGraph: (graph: MaterializedGraph) => void;
  setAgentStatus: (status: AgentStatusEvent | null) => void;
  addAgentProposal: (proposal: AgentProposalEvent) => void;
  upsertLiveAssistCard: (card: LiveAssistCardRecord) => void;
  approveAgentProposal: (
    proposalId: string,
  ) => Promise<AgentActionResult | null>;
  askAgentProposal: (proposalId: string) => Promise<void>;
  dismissAgentProposal: (
    proposalId: string,
  ) => Promise<LiveAssistCardRecord | null>;
  clearAgentProposals: () => Promise<LiveAssistCardRecord[]>;
  clearTranscript: () => void;
  loadSampleSessionPreview: (language?: string) => void;

  // Knowledge graph
  graphSnapshot: GraphSnapshot;
  setGraphSnapshot: (snapshot: GraphSnapshot) => void;
  applyGraphDelta: (delta: GraphDelta) => void;

  // Exports (backend → JSON string)
  exportTranscript: () => Promise<string>;
  exportGraph: () => Promise<string>;
  getSessionId: () => Promise<string>;

  // Pipeline status
  pipelineStatus: PipelineStatus;
  setPipelineStatus: (status: PipelineStatus) => void;
  pipelineLatencies: Partial<
    Record<PipelineLatencyEvent["stage"], PipelineLatencyEvent>
  >;
  setPipelineLatency: (sample: PipelineLatencyEvent) => void;
  latestAudioConsumerHealth: ProcessedAudioConsumerHealthPayload | null;
  setAudioConsumerHealth: (
    payload: ProcessedAudioConsumerHealthPayload,
  ) => void;
  persistenceQueueBackpressure: Record<
    string,
    PersistenceQueueBackpressurePayload
  >;
  setPersistenceQueueBackpressure: (
    payload: PersistenceQueueBackpressurePayload,
  ) => void;

  // Speakers
  speakers: SpeakerInfo[];
  addOrUpdateSpeaker: (speaker: SpeakerInfo) => void;
  clearSpeakers: () => void;

  // Capture state
  isCapturing: boolean;
  captureStartTime: number | null;
  setIsCapturing: (capturing: boolean) => void;
  startCapture: () => Promise<void>;
  stopCapture: () => Promise<void>;

  /// IDs of sources currently reporting backpressure. Updated by the
  /// `capture-backpressure` event listener. Non-empty means at least one
  /// active source's ring buffer is dropping chunks — surface a warning in
  /// the UI so the user can slow the pipeline (e.g. disable Gemini) before
  /// transcript quality degrades.
  backpressuredSources: string[];
  setSourceBackpressure: (sourceId: string, isBackpressured: boolean) => void;

  // Transcribe state (manual transcription)
  isTranscribing: boolean;
  startTranscribe: () => Promise<void>;
  stopTranscribe: () => Promise<void>;

  // Error state
  error: string | null;
  setError: (error: string | null) => void;
  clearError: () => void;

  // Notifications (ADR-0011) — unified transient feedback queue.
  notifications: AppNotification[];
  notify: (opts: NotifyOptions) => string;
  dismissNotification: (id: string) => void;
  clearNotifications: () => void;

  // ── Chat ─────────────────────────────────────────────────────────────
  chatMessages: ChatMessage[];
  isChatLoading: boolean;
  rightPanelTab: "transcript" | "chat";
  setRightPanelTab: (tab: "transcript" | "chat") => void;
  agentOverlayOpen: boolean;
  setAgentOverlayOpen: (open: boolean) => void;
  toggleAgentOverlay: () => void;
  tokenOverlayOpen: boolean;
  setTokenOverlayOpen: (open: boolean) => void;
  toggleTokenOverlay: () => void;
  nativeS2sEnabled: boolean;
  setNativeS2sEnabled: (enabled: boolean) => void;
  /**
   * Visual theme preference (ADR-0009, Wave 4). `system` defers to the OS
   * `prefers-color-scheme`; `light`/`dark` pin the palette. Persisted to
   * localStorage under `ag.theme` and reflected onto
   * `document.documentElement.dataset.theme` (see `src/theme.ts`).
   */
  theme: "system" | "light" | "dark";
  setTheme: (theme: "system" | "light" | "dark") => void;
  // Conversation mode (ADR-0013): notes/graph-building vs converse-with-kb.
  conversationMode: "notes" | "converse";
  setConversationMode: (mode: "notes" | "converse") => void;
  converseEngine: "native" | "pipelined";
  setConverseEngine: (engine: "native" | "pipelined") => void;
  /**
   * Which native cloud-native S2S voice agent the `native` converse engine
   * routes to: Gemini Live (`gemini`, the default) or the OpenAI Realtime
   * voice agent (`openai`, `gpt-realtime-2`). Persisted to localStorage under
   * `ag.converseRealtimeAgentProvider`. Only consulted when
   * `conversationMode === "converse" && converseEngine === "native"`.
   */
  converseRealtimeAgentProvider: "gemini" | "openai";
  setConverseRealtimeAgentProvider: (provider: "gemini" | "openai") => void;
  sendChatMessage: (message: string) => Promise<void>;
  clearChatHistory: () => Promise<void>;

  /**
   * `request_id` of the streaming chat reply currently being assembled
   * (plan A3 / ADR-0006). `null` when no stream is in flight; otherwise
   * the last entry in `chatMessages` is the assistant placeholder being
   * grown by `appendChatTokenDelta`.
   */
  streamingChatRequestId: string | null;
  /** Append a token delta to the in-progress assistant message. */
  appendChatTokenDelta: (event: ChatTokenDeltaEvent) => void;
  /**
   * Finalize the in-progress assistant message: replace its content with
   * `full_text` (which is authoritative — handles cases where the
   * provider streamed token deltas and then revised them on the terminal
   * chunk), clear `streamingChatRequestId`, and clear `isChatLoading`.
   */
  finalizeChatStream: (event: ChatTokenDoneEvent) => void;

  // ── Models ────────────────────────────────────────────────────────────
  models: ModelInfo[];
  isDownloading: boolean;
  downloadProgress: DownloadProgress | null;
  fetchModels: () => Promise<void>;
  downloadModel: (filename: string) => Promise<void>;

  // ── API endpoint ──────────────────────────────────────────────────────
  apiConfig: ApiEndpointConfig | null;
  configureApiEndpoint: (config: ApiEndpointConfig) => Promise<void>;
  clearApiEndpoint: () => void;

  // ── Gemini Live dual pipeline ───────────────────────────────────────────
  isGeminiActive: boolean;
  geminiTranscripts: GeminiTranscriptEntry[];
  // Which backend command the active Gemini/converse session was started with,
  // so `stopGemini` calls the matching stop command. `null` when idle.
  activeGeminiCommand:
    | "start_gemini"
    | "start_converse"
    | "start_openai_realtime"
    | null;
  addGeminiTranscript: (entry: GeminiTranscriptEntry) => void;
  clearGeminiTranscripts: () => void;
  startGemini: () => Promise<void>;
  stopGemini: () => Promise<void>;

  // ── Settings ──────────────────────────────────────────────────────────
  settings: AppSettings | null;
  modelStatus: ModelStatus | null;
  settingsOpen: boolean;
  settingsLoading: boolean;
  isDeletingModel: string | null;
  openSettings: () => void;
  closeSettings: () => void;
  fetchSettings: () => Promise<void>;
  saveSettings: (settings: AppSettings) => Promise<void>;
  fetchModelStatus: () => Promise<void>;
  deleteModel: (filename: string) => Promise<void>;

  // ── Credentials ──────────────────────────────────────────────────────
  saveCredential: (key: string, value: string) => Promise<void>;
  deleteCredential: (key: string) => Promise<void>;

  // ── AWS profile discovery ────────────────────────────────────────────
  /** List profile names discovered in ~/.aws/config and ~/.aws/credentials. */
  listAwsProfiles: () => Promise<string[]>;

  // ── Sessions (v2: list, load transcript, soft-delete + restore) ──────
  sessionsBrowserOpen: boolean;
  sessions: SessionMetadata[];
  sessionsLoading: boolean;
  openSessionsBrowser: () => void;
  closeSessionsBrowser: () => void;
  listSessions: (limit?: number) => Promise<SessionMetadata[]>;
  loadSessionTranscript: (sessionId: string) => Promise<TranscriptSegment[]>;
  loadSession: (sessionId: string) => Promise<LoadedSession | null>;
  /** Soft-delete: flag as trashed, files stay on disk, restorable. */
  deleteSession: (sessionId: string) => Promise<void>;
  /** Restore a soft-deleted session back to the active list. */
  restoreSession: (sessionId: string) => Promise<void>;
  /** Permanently delete a session (unlinks files). Bypasses trash. */
  deleteSessionPermanently: (sessionId: string) => Promise<void>;
  /** Lazy cleanup: ask backend to hard-delete trash entries older than 30d. */
  purgeExpiredSessions: () => Promise<string[]>;
  /** Scan session artifact files and rebuild missing sessions-index entries. */
  recoverOrphanedSessions: () => Promise<SessionRecoveryReport | null>;
}
