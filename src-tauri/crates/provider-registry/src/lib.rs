//! Backend-owned provider capability registry.
//!
//! This is the first slice of the provider-registry architecture: a stable,
//! read-only catalog of implemented and planned provider surfaces. It gives the
//! backend one place to describe credentials, transport, source fan-out policy,
//! health/model commands, and local runtime-file requirements before the
//! Settings UI migrates to generated forms.

use serde::Serialize;

const OPENAI_COMPAT_CREDENTIAL_KEYS: &[&str] = &[
    "openai_api_key",
    "cerebras_api_key",
    "openrouter_api_key",
    "groq_api_key",
    "together_api_key",
    "fireworks_api_key",
];
const CEREBRAS_CREDENTIAL_KEYS: &[&str] = &["cerebras_api_key"];
const AWS_CREDENTIAL_KEYS: &[&str] = &[
    "aws_access_key",
    "aws_secret_key",
    "aws_session_token",
    "aws_profile",
    "aws_region",
];
const GEMINI_CREDENTIAL_KEYS: &[&str] = &["gemini_api_key", "google_service_account_path"];
const GOOGLE_STT_CREDENTIAL_KEYS: &[&str] = &[];
const AZURE_SPEECH_CREDENTIAL_KEYS: &[&str] = &[];
const SONIOX_CREDENTIAL_KEYS: &[&str] = &["soniox_api_key"];
const GLADIA_CREDENTIAL_KEYS: &[&str] = &["gladia_api_key"];
const SPEECHMATICS_CREDENTIAL_KEYS: &[&str] = &["speechmatics_api_key"];
const ELEVENLABS_CREDENTIAL_KEYS: &[&str] = &["elevenlabs_api_key"];
const REVAI_CREDENTIAL_KEYS: &[&str] = &["revai_api_key"];
const LOCAL_WHISPER_FEATURES: &[&str] = &["local-ml", "asr-whisper"];
const SHERPA_STREAMING_FEATURES: &[&str] = &["sherpa-streaming"];
const MOONSHINE_FEATURES: &[&str] = &["asr-moonshine"];
const LOCAL_LLAMA_FEATURES: &[&str] = &["local-ml", "llm-llama"];
const MISTRALRS_FEATURES: &[&str] = &["local-ml", "llm-mistralrs"];

pub const WHISPER_MODEL_SMALL_EN: &str = "ggml-small.en.bin";
pub const LLM_MODEL_FILENAME: &str = "lfm2-350m-extract-q4_k_m.gguf";
pub const SHERPA_ZIPFORMER_20M: &str = "streaming-zipformer-en-20M";
pub const MOONSHINE_TINY_STREAMING_EN: &str = "moonshine-tiny-streaming-en";
pub const MOONSHINE_SMALL_STREAMING_EN: &str = "moonshine-small-streaming-en";
pub const MOONSHINE_MEDIUM_STREAMING_EN: &str = "moonshine-medium-streaming-en";
pub const SHERPA_ZIPFORMER_REQUIRED_FILES: &[&str] = &[
    "encoder-epoch-99-avg-1.onnx",
    "decoder-epoch-99-avg-1.onnx",
    "joiner-epoch-99-avg-1.onnx",
    "tokens.txt",
];
pub const MOONSHINE_STREAMING_REQUIRED_FILES: &[&str] = &[
    "adapter.ort",
    "cross_kv.ort",
    "decoder_kv.ort",
    "decoder_kv_with_attention.ort",
    "encoder.ort",
    "frontend.ort",
    "streaming_config.json",
    "tokenizer.bin",
];
pub const OPENAI_REALTIME_TRANSCRIPTION_DEFAULT_MODEL: &str = "gpt-realtime-whisper";
pub const CEREBRAS_DEFAULT_MODEL: &str = "gpt-oss-120b";
pub const CEREBRAS_PREVIEW_MODEL: &str = "zai-glm-4.7";
pub const SORTFORMER_MODEL_FILENAME: &str = "diar_streaming_sortformer_4spk-v2.onnx";
pub const DIAR_SEG_PYANNOTE_DIR: &str = "sherpa-onnx-pyannote-segmentation-3-0";
pub const DIAR_SEG_PYANNOTE_REQUIRED_FILES: &[&str] = &["model.onnx", "model.int8.onnx"];
pub const DIAR_EMB_TITANET_FILENAME: &str = "nemo_en_titanet_small.onnx";

const LOCAL_WHISPER_FILES: &[&str] = &[WHISPER_MODEL_SMALL_EN];
const LOCAL_LLM_FILES: &[&str] = &[LLM_MODEL_FILENAME];
const SORTFORMER_FILES: &[&str] = &[SORTFORMER_MODEL_FILENAME];
const TITANET_FILES: &[&str] = &[DIAR_EMB_TITANET_FILENAME];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStage {
    Asr,
    Diarization,
    Llm,
    Tts,
    RealtimeAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    Implemented,
    Planned,
    Watch,
    EnterpriseWatch,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderTransport {
    Local,
    Http,
    WebSocket,
    RestInitWebSocket,
    AwsSdk,
    GrpcBidi,
    SdkNative,
    SidecarProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSourcePolicy {
    /// Each selected source can be processed independently.
    MultiSourceIndependent,
    /// Selected sources are mixed into one provider stream.
    MultiSourceMixed,
    /// Provider runtime owns one session and cannot fan out yet.
    SingleSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCatalogPolicy {
    None,
    Fixed,
    LocalFiles,
    RemoteCommand,
    UserSupplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEventSemantics {
    /// Provider returns complete transcript spans without streaming revisions.
    TranscriptFinalOnly,
    /// Provider emits partial and final transcript span revisions.
    TranscriptPartialFinal,
    /// Provider emits partial/final transcript revisions plus turn boundaries.
    TranscriptPartialFinalTurns,
    /// Provider owns a native realtime audio/text session rather than only STT.
    NativeRealtimeAudioText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAudioFrameFormat {
    F32,
    PcmS16Le,
    WavPcmS16Le,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAudioTransportEncoding {
    LocalBuffer,
    WebSocketBinary,
    WebSocketJsonBase64,
    AwsEventStream,
    GrpcStreaming,
    SdkNative,
    MultipartWav,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSettingsGroup {
    Basic,
    ModelCatalog,
    Health,
    Advanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAuthLifecycle {
    None,
    SavedApiKey,
    #[serde(rename = "openai_compatible_api_key")]
    OpenAiCompatibleApiKey,
    AwsCredentialChain,
    GoogleApiKeyOrServiceAccount,
    GoogleAdcOrServiceAccount,
    AzureSpeechKeyOrEntraToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSessionLifecycle {
    Noop,
    PerRequest,
    LocalInProcess,
    LocalStreamingRuntime,
    LongLivedWebSocket,
    AwsStreamingSdk,
    GrpcBidirectionalStream,
    NativeSdkConversation,
    SidecarProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKeepaliveStrategy {
    None,
    ClientAudioStream,
    ClientControlMessage,
    ProviderSpecific,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCloseStrategy {
    Noop,
    RequestCompletes,
    DropRuntime,
    WebSocketCloseFrame,
    EndStreamThenCloseFrame,
    TerminateMessageThenCloseFrame,
    ProviderCloseMessageThenCloseFrame,
    AwsEndStream,
    ProviderSpecific,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderDataBoundary {
    LocalOnly,
    UserConfiguredEndpoint,
    UserConfiguredRegion,
    ProviderAccountBoundary,
    VendorCloud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderDataClass {
    Audio,
    TranscriptText,
    PromptText,
    Notes,
    GraphContext,
    GeneratedText,
    GeneratedAudio,
    SpeakerLabels,
    TimingMetadata,
    ModelCatalogMetadata,
    ProviderConfiguration,
    CredentialAuth,
    UsageMetadata,
    ProviderDiagnostics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPolicyStatus {
    Unknown,
    NotApplicable,
    UserConfigured,
    ProviderDocsLinked,
    EnterpriseOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSensitiveErrorPolicy {
    Unknown,
    LocalOnly,
    AudioGraphRedacted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderEndpointMode {
    DefaultRegion,
    CustomEndpoint,
    PrivateEndpoint,
    SovereignCloud,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPackagingRequirement {
    ProtobufGrpcClient,
    NativeSdkAssets,
    NativeFrameworkAssets,
    SystemLibraries,
    SystemCertificates,
    VisualCppRedistributable,
    SidecarProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSpeakerLabelSupport {
    None,
    BatchOnly,
    StreamingProviderLabels,
    StreamingUnverified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderSpeakerSemantics {
    pub label_support: ProviderSpeakerLabelSupport,
    pub interim_labels_may_be_unknown: bool,
    /// Provider speaker ids are generic diarization labels, not stable people.
    pub speaker_ids_are_stable_identity: bool,
    /// AudioGraph should still run or join a provider-neutral speaker timeline.
    pub local_timeline_recommended: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderHealthProbeKind {
    TokenAcquisition,
    MetadataOnly,
    SdkDependency,
    EndpointConnectivity,
    StreamingRpcAvailability,
    LiveEnvGatedSmoke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialSchemaStatus {
    NotRequired,
    Wired,
    RequiredNotWired,
    FlexibleExternal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderRoadmapMetadata {
    pub source_url: &'static str,
    pub source_date: &'static str,
    pub auth_schema: ProviderCredentialSchemaStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_selectable_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderEnterpriseMetadata {
    pub endpoint_modes: &'static [ProviderEndpointMode],
    pub packaging: &'static [ProviderPackagingRequirement],
    pub speaker_semantics: ProviderSpeakerSemantics,
    pub health_probes: &'static [ProviderHealthProbeKind],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderLifecycleDescriptor {
    pub auth: ProviderAuthLifecycle,
    pub session: ProviderSessionLifecycle,
    pub keepalive: ProviderKeepaliveStrategy,
    pub close: ProviderCloseStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderPrivacyDescriptor {
    /// True when user audio, transcript text, prompts, or generated text leave
    /// the device for this provider surface.
    pub data_leaves_device: bool,
    pub data_boundary: ProviderDataBoundary,
    pub data_classes_sent: &'static [ProviderDataClass],
    pub data_classes_returned: &'static [ProviderDataClass],
    pub health_check_data_classes: &'static [ProviderDataClass],
    pub cloud_transfer_acknowledgement_required: bool,
    pub retention_policy: ProviderPolicyStatus,
    pub training_policy: ProviderPolicyStatus,
    pub deletion_policy: ProviderPolicyStatus,
    /// Official provider policy URL backing the `*_policy` claims above. `None`
    /// means no verifiable official policy was sourced and the claims stay
    /// `Unknown` (never fabricate a URL or a non-`Unknown` claim without one).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_url: Option<&'static str>,
    /// ISO-8601 date the `policy_url` was last verified against official docs.
    /// HONESTY INVARIANT: a `Some(policy_url)` MUST carry a `Some(source_date)`
    /// and vice versa — a policy link with no verification date (or a date with
    /// no link) is not allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_url_source_date: Option<&'static str>,
    /// Official subprocessors / data-residency list URL, when published. Kept
    /// separate from `policy_url` because providers publish it on its own page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subprocessors_url: Option<&'static str>,
    pub enterprise_no_training_config: ProviderPolicyStatus,
    pub data_residency: ProviderPolicyStatus,
    pub sensitive_error_policy: ProviderSensitiveErrorPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processor_identity: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderAudioFormat {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub frame_format: ProviderAudioFrameFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAttributionMode {
    None,
    Speaker,
    Channel,
    SpeakerAndChannel,
    ExperimentalSourceSeparation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderChannelLayout {
    Mono,
    SourceNative,
    GeneratedSpeakerLanes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderChannelLabelSemantics {
    None,
    ProviderChannelIndex,
    SourceChannelId,
    GeneratedSpeakerLane,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderAttributionDescriptor {
    pub mode: ProviderAttributionMode,
    pub max_channels: u16,
    pub accepted_layouts: &'static [ProviderChannelLayout],
    pub channel_label_semantics: ProviderChannelLabelSemantics,
    pub requires_source_native_channels: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_source_url: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_source_date: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderAudioInputDescriptor {
    /// Format emitted by the processed-audio bus before any provider adapter.
    pub pipeline_format: ProviderAudioFormat,
    /// Format sent to the provider/runtime after adapter conversion.
    pub provider_format: ProviderAudioFormat,
    pub transport_encoding: ProviderAudioTransportEncoding,
    pub adapter_resamples: bool,
    /// True only when the current adapter can preserve source/channel semantics.
    pub supports_multichannel: bool,
    pub attribution: ProviderAttributionDescriptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalModelKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LocalModelRequirement {
    pub model_id: &'static str,
    pub kind: LocalModelKind,
    pub required_files: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderModelCatalogItem {
    pub id: &'static str,
    pub display_name: &'static str,
    pub is_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ProviderDescriptor {
    pub id: &'static str,
    pub display_name: &'static str,
    pub stage: ProviderStage,
    pub settings_variant: &'static str,
    pub status: ProviderStatus,
    pub transport: ProviderTransport,
    pub credential_keys: &'static [&'static str],
    /// Cargo features that can make this provider available in a build.
    ///
    /// Empty means the provider is always present in the compiled app. Multiple
    /// entries are alternatives when an umbrella feature enables the same code
    /// path as a narrower provider feature.
    pub required_features: &'static [&'static str],
    pub model_catalog: ModelCatalogPolicy,
    pub local_models: &'static [LocalModelRequirement],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed_model_catalog: Option<&'static [ProviderModelCatalogItem]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_command: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_catalog_command: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_policy: Option<ProviderSourcePolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_policy_label: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_semantics: Option<ProviderEventSemantics>,
    pub settings_groups: &'static [ProviderSettingsGroup],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_input: Option<ProviderAudioInputDescriptor>,
    pub lifecycle: ProviderLifecycleDescriptor,
    pub privacy: ProviderPrivacyDescriptor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enterprise: Option<ProviderEnterpriseMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roadmap: Option<ProviderRoadmapMetadata>,
    pub supports_streaming: bool,
    pub supports_partial_revisions: bool,
    pub supports_diarization: bool,
}

pub const PIPELINE_AUDIO_SAMPLE_RATE_HZ: u32 = 16_000;
pub const PIPELINE_AUDIO_CHANNELS: u16 = 1;
pub const PIPELINE_AUDIO_FRAME_FORMAT: ProviderAudioFrameFormat = ProviderAudioFrameFormat::F32;

pub const PIPELINE_F32_16K_MONO: ProviderAudioFormat = ProviderAudioFormat {
    sample_rate_hz: PIPELINE_AUDIO_SAMPLE_RATE_HZ,
    channels: PIPELINE_AUDIO_CHANNELS,
    frame_format: PIPELINE_AUDIO_FRAME_FORMAT,
};

const PROVIDER_F32_16K_MONO: ProviderAudioFormat = ProviderAudioFormat {
    sample_rate_hz: 16_000,
    channels: 1,
    frame_format: ProviderAudioFrameFormat::F32,
};

const PROVIDER_PCM16_16K_MONO: ProviderAudioFormat = ProviderAudioFormat {
    sample_rate_hz: 16_000,
    channels: 1,
    frame_format: ProviderAudioFrameFormat::PcmS16Le,
};

const PROVIDER_PCM16_24K_MONO: ProviderAudioFormat = ProviderAudioFormat {
    sample_rate_hz: 24_000,
    channels: 1,
    frame_format: ProviderAudioFrameFormat::PcmS16Le,
};

const PROVIDER_WAV_PCM16_16K_MONO: ProviderAudioFormat = ProviderAudioFormat {
    sample_rate_hz: 16_000,
    channels: 1,
    frame_format: ProviderAudioFrameFormat::WavPcmS16Le,
};

const MONO_CHANNEL_LAYOUTS: &[ProviderChannelLayout] = &[ProviderChannelLayout::Mono];

const NO_ATTRIBUTION: ProviderAttributionDescriptor = ProviderAttributionDescriptor {
    mode: ProviderAttributionMode::None,
    max_channels: 1,
    accepted_layouts: MONO_CHANNEL_LAYOUTS,
    channel_label_semantics: ProviderChannelLabelSemantics::None,
    requires_source_native_channels: false,
    capability_source_url: None,
    capability_source_date: None,
};

const SPEAKER_ATTRIBUTION: ProviderAttributionDescriptor = ProviderAttributionDescriptor {
    mode: ProviderAttributionMode::Speaker,
    max_channels: 1,
    accepted_layouts: MONO_CHANNEL_LAYOUTS,
    channel_label_semantics: ProviderChannelLabelSemantics::None,
    requires_source_native_channels: false,
    capability_source_url: None,
    capability_source_date: None,
};

const XAI_STT_SPEAKER_ATTRIBUTION: ProviderAttributionDescriptor = ProviderAttributionDescriptor {
    capability_source_url: Some(
        "https://docs.x.ai/developers/rest-api-reference/inference/voice#speech-to-text---streaming",
    ),
    capability_source_date: Some("2026-06-26"),
    ..SPEAKER_ATTRIBUTION
};

const LOCAL_F32_AUDIO_INPUT: ProviderAudioInputDescriptor = ProviderAudioInputDescriptor {
    pipeline_format: PIPELINE_F32_16K_MONO,
    provider_format: PROVIDER_F32_16K_MONO,
    transport_encoding: ProviderAudioTransportEncoding::LocalBuffer,
    adapter_resamples: false,
    supports_multichannel: false,
    attribution: NO_ATTRIBUTION,
};

const LOCAL_F32_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        attribution: SPEAKER_ATTRIBUTION,
        ..LOCAL_F32_AUDIO_INPUT
    };

const WS_BINARY_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        attribution: SPEAKER_ATTRIBUTION,
        ..WS_BINARY_PCM16_16K_AUDIO_INPUT
    };

const WS_BINARY_PCM16_16K_XAI_SPEAKER_LABEL_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        attribution: XAI_STT_SPEAKER_ATTRIBUTION,
        ..WS_BINARY_PCM16_16K_AUDIO_INPUT
    };

const AWS_EVENTSTREAM_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        attribution: SPEAKER_ATTRIBUTION,
        ..AWS_EVENTSTREAM_PCM16_16K_AUDIO_INPUT
    };

const GRPC_STREAMING_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        attribution: SPEAKER_ATTRIBUTION,
        ..GRPC_STREAMING_PCM16_16K_AUDIO_INPUT
    };

const SDK_NATIVE_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        attribution: SPEAKER_ATTRIBUTION,
        ..SDK_NATIVE_PCM16_16K_AUDIO_INPUT
    };

const DIARIZATION_LOCAL_TIMELINE_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        attribution: SPEAKER_ATTRIBUTION,
        ..LOCAL_F32_AUDIO_INPUT
    };

const BATCH_WAV_AUDIO_INPUT: ProviderAudioInputDescriptor = ProviderAudioInputDescriptor {
    pipeline_format: PIPELINE_F32_16K_MONO,
    provider_format: PROVIDER_WAV_PCM16_16K_MONO,
    transport_encoding: ProviderAudioTransportEncoding::MultipartWav,
    adapter_resamples: false,
    supports_multichannel: false,
    attribution: NO_ATTRIBUTION,
};

const WS_BINARY_PCM16_16K_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        pipeline_format: PIPELINE_F32_16K_MONO,
        provider_format: PROVIDER_PCM16_16K_MONO,
        transport_encoding: ProviderAudioTransportEncoding::WebSocketBinary,
        adapter_resamples: false,
        supports_multichannel: false,
        attribution: NO_ATTRIBUTION,
    };

const WS_JSON_BASE64_PCM16_16K_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        pipeline_format: PIPELINE_F32_16K_MONO,
        provider_format: PROVIDER_PCM16_16K_MONO,
        transport_encoding: ProviderAudioTransportEncoding::WebSocketJsonBase64,
        adapter_resamples: false,
        supports_multichannel: false,
        attribution: NO_ATTRIBUTION,
    };

const WS_JSON_BASE64_PCM16_24K_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        pipeline_format: PIPELINE_F32_16K_MONO,
        provider_format: PROVIDER_PCM16_24K_MONO,
        transport_encoding: ProviderAudioTransportEncoding::WebSocketJsonBase64,
        adapter_resamples: true,
        supports_multichannel: false,
        attribution: NO_ATTRIBUTION,
    };

const AWS_EVENTSTREAM_PCM16_16K_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        pipeline_format: PIPELINE_F32_16K_MONO,
        provider_format: PROVIDER_PCM16_16K_MONO,
        transport_encoding: ProviderAudioTransportEncoding::AwsEventStream,
        adapter_resamples: false,
        supports_multichannel: false,
        attribution: NO_ATTRIBUTION,
    };

const GRPC_STREAMING_PCM16_16K_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        pipeline_format: PIPELINE_F32_16K_MONO,
        provider_format: PROVIDER_PCM16_16K_MONO,
        transport_encoding: ProviderAudioTransportEncoding::GrpcStreaming,
        adapter_resamples: false,
        supports_multichannel: false,
        attribution: NO_ATTRIBUTION,
    };

const SDK_NATIVE_PCM16_16K_AUDIO_INPUT: ProviderAudioInputDescriptor =
    ProviderAudioInputDescriptor {
        pipeline_format: PIPELINE_F32_16K_MONO,
        provider_format: PROVIDER_PCM16_16K_MONO,
        transport_encoding: ProviderAudioTransportEncoding::SdkNative,
        adapter_resamples: false,
        supports_multichannel: false,
        attribution: NO_ATTRIBUTION,
    };

const BASIC_ONLY_GROUPS: &[ProviderSettingsGroup] = &[ProviderSettingsGroup::Basic];
const BASIC_MODEL_GROUPS: &[ProviderSettingsGroup] = &[
    ProviderSettingsGroup::Basic,
    ProviderSettingsGroup::ModelCatalog,
];
const BASIC_MODEL_ADVANCED_GROUPS: &[ProviderSettingsGroup] = &[
    ProviderSettingsGroup::Basic,
    ProviderSettingsGroup::ModelCatalog,
    ProviderSettingsGroup::Advanced,
];
const BASIC_MODEL_HEALTH_GROUPS: &[ProviderSettingsGroup] = &[
    ProviderSettingsGroup::Basic,
    ProviderSettingsGroup::ModelCatalog,
    ProviderSettingsGroup::Health,
];
const BASIC_MODEL_HEALTH_ADVANCED_GROUPS: &[ProviderSettingsGroup] = &[
    ProviderSettingsGroup::Basic,
    ProviderSettingsGroup::ModelCatalog,
    ProviderSettingsGroup::Health,
    ProviderSettingsGroup::Advanced,
];
const BASIC_HEALTH_ADVANCED_GROUPS: &[ProviderSettingsGroup] = &[
    ProviderSettingsGroup::Basic,
    ProviderSettingsGroup::Health,
    ProviderSettingsGroup::Advanced,
];

const LOCAL_IN_PROCESS_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::None,
    session: ProviderSessionLifecycle::LocalInProcess,
    keepalive: ProviderKeepaliveStrategy::None,
    close: ProviderCloseStrategy::DropRuntime,
};

const LOCAL_STREAMING_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::None,
    session: ProviderSessionLifecycle::LocalStreamingRuntime,
    keepalive: ProviderKeepaliveStrategy::None,
    close: ProviderCloseStrategy::DropRuntime,
};

const NOOP_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::None,
    session: ProviderSessionLifecycle::Noop,
    keepalive: ProviderKeepaliveStrategy::None,
    close: ProviderCloseStrategy::Noop,
};

const OPENAI_COMPAT_HTTP_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::OpenAiCompatibleApiKey,
    session: ProviderSessionLifecycle::PerRequest,
    keepalive: ProviderKeepaliveStrategy::None,
    close: ProviderCloseStrategy::RequestCompletes,
};

const SAVED_KEY_HTTP_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::SavedApiKey,
    session: ProviderSessionLifecycle::PerRequest,
    keepalive: ProviderKeepaliveStrategy::None,
    close: ProviderCloseStrategy::RequestCompletes,
};

const AWS_STREAMING_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::AwsCredentialChain,
    session: ProviderSessionLifecycle::AwsStreamingSdk,
    keepalive: ProviderKeepaliveStrategy::ClientAudioStream,
    close: ProviderCloseStrategy::AwsEndStream,
};

const AWS_REQUEST_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::AwsCredentialChain,
    session: ProviderSessionLifecycle::PerRequest,
    keepalive: ProviderKeepaliveStrategy::None,
    close: ProviderCloseStrategy::RequestCompletes,
};

const DEEPGRAM_LISTEN_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::SavedApiKey,
    session: ProviderSessionLifecycle::LongLivedWebSocket,
    keepalive: ProviderKeepaliveStrategy::ClientControlMessage,
    close: ProviderCloseStrategy::EndStreamThenCloseFrame,
};

const ASSEMBLYAI_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::SavedApiKey,
    session: ProviderSessionLifecycle::LongLivedWebSocket,
    keepalive: ProviderKeepaliveStrategy::ClientAudioStream,
    close: ProviderCloseStrategy::TerminateMessageThenCloseFrame,
};

const OPENAI_REALTIME_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::SavedApiKey,
    session: ProviderSessionLifecycle::LongLivedWebSocket,
    keepalive: ProviderKeepaliveStrategy::ClientAudioStream,
    close: ProviderCloseStrategy::EndStreamThenCloseFrame,
};

const PLANNED_STREAMING_STT_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::SavedApiKey,
    session: ProviderSessionLifecycle::LongLivedWebSocket,
    keepalive: ProviderKeepaliveStrategy::ProviderSpecific,
    close: ProviderCloseStrategy::ProviderSpecific,
};

const GLADIA_LIVE_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::SavedApiKey,
    session: ProviderSessionLifecycle::LongLivedWebSocket,
    keepalive: ProviderKeepaliveStrategy::ClientAudioStream,
    close: ProviderCloseStrategy::ProviderCloseMessageThenCloseFrame,
};

const GOOGLE_REALTIME_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::GoogleApiKeyOrServiceAccount,
    session: ProviderSessionLifecycle::LongLivedWebSocket,
    keepalive: ProviderKeepaliveStrategy::ClientAudioStream,
    close: ProviderCloseStrategy::EndStreamThenCloseFrame,
};

const GOOGLE_STT_GRPC_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::GoogleAdcOrServiceAccount,
    session: ProviderSessionLifecycle::GrpcBidirectionalStream,
    keepalive: ProviderKeepaliveStrategy::ClientAudioStream,
    close: ProviderCloseStrategy::ProviderSpecific,
};

const AZURE_SPEECH_SDK_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::AzureSpeechKeyOrEntraToken,
    session: ProviderSessionLifecycle::NativeSdkConversation,
    keepalive: ProviderKeepaliveStrategy::ClientAudioStream,
    close: ProviderCloseStrategy::ProviderSpecific,
};

const WATCHLIST_GRPC_AUTH_REQUIRED_LIFECYCLE: ProviderLifecycleDescriptor =
    ProviderLifecycleDescriptor {
        auth: ProviderAuthLifecycle::SavedApiKey,
        session: ProviderSessionLifecycle::GrpcBidirectionalStream,
        keepalive: ProviderKeepaliveStrategy::ClientAudioStream,
        close: ProviderCloseStrategy::ProviderSpecific,
    };

const DEEPGRAM_AURA_LIFECYCLE: ProviderLifecycleDescriptor = ProviderLifecycleDescriptor {
    auth: ProviderAuthLifecycle::SavedApiKey,
    session: ProviderSessionLifecycle::LongLivedWebSocket,
    keepalive: ProviderKeepaliveStrategy::ClientControlMessage,
    close: ProviderCloseStrategy::ProviderCloseMessageThenCloseFrame,
};

const NO_DATA_CLASSES: &[ProviderDataClass] = &[];
const AUTH_HEALTH_DATA_CLASSES: &[ProviderDataClass] = &[
    ProviderDataClass::CredentialAuth,
    ProviderDataClass::ProviderConfiguration,
];
const LOCAL_MODEL_HEALTH_DATA_CLASSES: &[ProviderDataClass] =
    &[ProviderDataClass::ModelCatalogMetadata];
const ASR_CONTENT_SENT: &[ProviderDataClass] = &[
    ProviderDataClass::Audio,
    ProviderDataClass::ProviderConfiguration,
];
const ASR_CONTENT_RETURNED: &[ProviderDataClass] = &[
    ProviderDataClass::TranscriptText,
    ProviderDataClass::SpeakerLabels,
    ProviderDataClass::TimingMetadata,
    ProviderDataClass::UsageMetadata,
    ProviderDataClass::ProviderDiagnostics,
];
const LLM_CONTENT_SENT: &[ProviderDataClass] = &[
    ProviderDataClass::PromptText,
    ProviderDataClass::TranscriptText,
    ProviderDataClass::Notes,
    ProviderDataClass::GraphContext,
    ProviderDataClass::ProviderConfiguration,
];
const LLM_CONTENT_RETURNED: &[ProviderDataClass] = &[
    ProviderDataClass::GeneratedText,
    ProviderDataClass::UsageMetadata,
    ProviderDataClass::ProviderDiagnostics,
];
const TTS_CONTENT_SENT: &[ProviderDataClass] = &[
    ProviderDataClass::GeneratedText,
    ProviderDataClass::ProviderConfiguration,
];
const TTS_CONTENT_RETURNED: &[ProviderDataClass] = &[
    ProviderDataClass::GeneratedAudio,
    ProviderDataClass::UsageMetadata,
    ProviderDataClass::ProviderDiagnostics,
];
const REALTIME_AGENT_CONTENT_SENT: &[ProviderDataClass] = &[
    ProviderDataClass::Audio,
    ProviderDataClass::TranscriptText,
    ProviderDataClass::PromptText,
    ProviderDataClass::Notes,
    ProviderDataClass::GraphContext,
    ProviderDataClass::ProviderConfiguration,
];
const REALTIME_AGENT_CONTENT_RETURNED: &[ProviderDataClass] = &[
    ProviderDataClass::TranscriptText,
    ProviderDataClass::GeneratedText,
    ProviderDataClass::GeneratedAudio,
    ProviderDataClass::TimingMetadata,
    ProviderDataClass::UsageMetadata,
    ProviderDataClass::ProviderDiagnostics,
];
const LOCAL_CONTENT_RETURNED: &[ProviderDataClass] = &[
    ProviderDataClass::TranscriptText,
    ProviderDataClass::GeneratedText,
    ProviderDataClass::SpeakerLabels,
    ProviderDataClass::TimingMetadata,
];

const LOCAL_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_leaves_device: false,
    data_boundary: ProviderDataBoundary::LocalOnly,
    data_classes_sent: NO_DATA_CLASSES,
    data_classes_returned: LOCAL_CONTENT_RETURNED,
    health_check_data_classes: LOCAL_MODEL_HEALTH_DATA_CLASSES,
    cloud_transfer_acknowledgement_required: false,
    retention_policy: ProviderPolicyStatus::NotApplicable,
    training_policy: ProviderPolicyStatus::NotApplicable,
    deletion_policy: ProviderPolicyStatus::NotApplicable,
    policy_url: None,
    policy_url_source_date: None,
    subprocessors_url: None,
    enterprise_no_training_config: ProviderPolicyStatus::NotApplicable,
    data_residency: ProviderPolicyStatus::NotApplicable,
    sensitive_error_policy: ProviderSensitiveErrorPolicy::LocalOnly,
    processor_identity: Some("Local device"),
};

const CLOUD_POLICY_UNKNOWN: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_leaves_device: true,
    data_boundary: ProviderDataBoundary::VendorCloud,
    data_classes_sent: NO_DATA_CLASSES,
    data_classes_returned: NO_DATA_CLASSES,
    health_check_data_classes: AUTH_HEALTH_DATA_CLASSES,
    cloud_transfer_acknowledgement_required: true,
    retention_policy: ProviderPolicyStatus::Unknown,
    training_policy: ProviderPolicyStatus::Unknown,
    deletion_policy: ProviderPolicyStatus::Unknown,
    policy_url: None,
    policy_url_source_date: None,
    subprocessors_url: None,
    enterprise_no_training_config: ProviderPolicyStatus::Unknown,
    data_residency: ProviderPolicyStatus::Unknown,
    sensitive_error_policy: ProviderSensitiveErrorPolicy::AudioGraphRedacted,
    processor_identity: None,
};

// --- Sourced provider data-boundary policies -------------------------------
//
// HONESTY RULE (item fee1): every non-local provider below either carries an
// official policy URL + verification date OR keeps `CLOUD_POLICY_UNKNOWN`'s
// `Unknown` status. No retention/training/deletion claim is made without a
// verifiable official source. URLs and source dates were verified against the
// providers' own documentation on the dates recorded here; providers whose
// official policy could not be confirmed remain `Unknown` (e.g. Soniox,
// AssemblyAI training-use, and every planned/roadmap candidate that still maps
// to `CLOUD_POLICY_UNKNOWN`).

// OpenAI (API / Realtime). Source: developers.openai.com/api/docs/guides/your-data
// — API inputs/outputs are NOT used for training by default (since 2023-03-01);
// abuse-monitoring logs retained up to 30 days; Zero Data Retention available to
// eligible customers; objects deletable via API/dashboard.
const OPENAI_DATA_USAGE_URL: &str = "https://developers.openai.com/api/docs/guides/your-data";
const OPENAI_DATA_USAGE_SOURCE_DATE: &str = "2026-06-30";
const OPENAI_SOURCED_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    retention_policy: ProviderPolicyStatus::ProviderDocsLinked,
    training_policy: ProviderPolicyStatus::ProviderDocsLinked,
    deletion_policy: ProviderPolicyStatus::ProviderDocsLinked,
    policy_url: Some(OPENAI_DATA_USAGE_URL),
    policy_url_source_date: Some(OPENAI_DATA_USAGE_SOURCE_DATE),
    ..CLOUD_POLICY_UNKNOWN
};

// Deepgram (ASR / TTS Aura). Sources verified 2026-06-30:
// - Model Improvement Partnership Program (training-by-default, opt-out via
//   `mip_opt_out=true`): developers.deepgram.com/docs/the-deepgram-model-improvement-partnership-program
// - Subprocessors list: deepgram.com/privacy/subprocessors
// - Regional/EU+AU data residency: developers.deepgram.com/trust-security/data-privacy-compliance
// Deepgram DOES retain a sample of customer audio for model training by default
// (MIP); training_policy is therefore ProviderDocsLinked, NOT a no-training claim.
const DEEPGRAM_MIP_URL: &str =
    "https://developers.deepgram.com/docs/the-deepgram-model-improvement-partnership-program";
const DEEPGRAM_SOURCE_DATE: &str = "2026-06-30";
const DEEPGRAM_SUBPROCESSORS_URL: &str = "https://deepgram.com/privacy/subprocessors";
const DEEPGRAM_SOURCED_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    retention_policy: ProviderPolicyStatus::ProviderDocsLinked,
    training_policy: ProviderPolicyStatus::ProviderDocsLinked,
    deletion_policy: ProviderPolicyStatus::ProviderDocsLinked,
    policy_url: Some(DEEPGRAM_MIP_URL),
    policy_url_source_date: Some(DEEPGRAM_SOURCE_DATE),
    subprocessors_url: Some(DEEPGRAM_SUBPROCESSORS_URL),
    ..CLOUD_POLICY_UNKNOWN
};

// AWS AI services (Transcribe / Bedrock). Sources verified 2026-06-30:
// - AI services opt-out policy (AWS MAY use customer content for service
//   improvement / model training unless you opt out via AWS Organizations):
//   docs.aws.amazon.com/organizations/latest/userguide/orgs_manage_policies_ai-opt-out.html
// - Data protection (region residency, VPC/PrivateLink):
//   docs.aws.amazon.com/transcribe/latest/dg/data-protection.html
// Region residency is user-configured (the user picks the AWS Region).
const AWS_AI_OPT_OUT_URL: &str =
    "https://docs.aws.amazon.com/organizations/latest/userguide/orgs_manage_policies_ai-opt-out.html";
const AWS_AI_SOURCE_DATE: &str = "2026-06-30";
const AWS_SOURCED_REGION_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_boundary: ProviderDataBoundary::UserConfiguredRegion,
    retention_policy: ProviderPolicyStatus::ProviderDocsLinked,
    training_policy: ProviderPolicyStatus::ProviderDocsLinked,
    deletion_policy: ProviderPolicyStatus::ProviderDocsLinked,
    policy_url: Some(AWS_AI_OPT_OUT_URL),
    policy_url_source_date: Some(AWS_AI_SOURCE_DATE),
    data_residency: ProviderPolicyStatus::UserConfigured,
    ..CLOUD_POLICY_UNKNOWN
};

// AssemblyAI (ASR). Source verified 2026-06-30: assemblyai.com/legal/privacy-policy
// documents retention + deletion rights; it does NOT state whether customer
// audio/transcripts are used for model training, so training_policy stays
// Unknown (no fabricated no-training claim). Subprocessors via Trust Center.
const ASSEMBLYAI_PRIVACY_URL: &str = "https://www.assemblyai.com/legal/privacy-policy";
const ASSEMBLYAI_SOURCE_DATE: &str = "2026-06-30";
const ASSEMBLYAI_SUBPROCESSORS_URL: &str =
    "https://app.vanta.com/assemblyai/trust/7n80syl8zln1bn1qm3x8eg/subprocessors";
const ASSEMBLYAI_SOURCED_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    retention_policy: ProviderPolicyStatus::ProviderDocsLinked,
    // training_policy intentionally left Unknown (inherited from
    // CLOUD_POLICY_UNKNOWN): the privacy policy does not address model training.
    deletion_policy: ProviderPolicyStatus::ProviderDocsLinked,
    policy_url: Some(ASSEMBLYAI_PRIVACY_URL),
    policy_url_source_date: Some(ASSEMBLYAI_SOURCE_DATE),
    subprocessors_url: Some(ASSEMBLYAI_SUBPROCESSORS_URL),
    ..CLOUD_POLICY_UNKNOWN
};

const USER_ENDPOINT_ASR_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_boundary: ProviderDataBoundary::UserConfiguredEndpoint,
    data_classes_sent: ASR_CONTENT_SENT,
    data_classes_returned: ASR_CONTENT_RETURNED,
    data_residency: ProviderPolicyStatus::UserConfigured,
    ..CLOUD_POLICY_UNKNOWN
};

const USER_ENDPOINT_ASR_NO_HEALTH_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    health_check_data_classes: NO_DATA_CLASSES,
    ..USER_ENDPOINT_ASR_PRIVACY
};

const USER_REGION_ASR_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_boundary: ProviderDataBoundary::UserConfiguredRegion,
    data_classes_sent: ASR_CONTENT_SENT,
    data_classes_returned: ASR_CONTENT_RETURNED,
    data_residency: ProviderPolicyStatus::UserConfigured,
    ..CLOUD_POLICY_UNKNOWN
};

const USER_REGION_ASR_NO_HEALTH_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    health_check_data_classes: NO_DATA_CLASSES,
    ..USER_REGION_ASR_PRIVACY
};

const PROVIDER_ACCOUNT_ASR_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_boundary: ProviderDataBoundary::ProviderAccountBoundary,
    data_classes_sent: ASR_CONTENT_SENT,
    data_classes_returned: ASR_CONTENT_RETURNED,
    data_residency: ProviderPolicyStatus::EnterpriseOnly,
    ..CLOUD_POLICY_UNKNOWN
};

const PROVIDER_ACCOUNT_ASR_NO_HEALTH_PRIVACY: ProviderPrivacyDescriptor =
    ProviderPrivacyDescriptor {
        health_check_data_classes: NO_DATA_CLASSES,
        ..PROVIDER_ACCOUNT_ASR_PRIVACY
    };

const VENDOR_CLOUD_ASR_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_classes_sent: ASR_CONTENT_SENT,
    data_classes_returned: ASR_CONTENT_RETURNED,
    ..CLOUD_POLICY_UNKNOWN
};

const VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    health_check_data_classes: NO_DATA_CLASSES,
    ..VENDOR_CLOUD_ASR_PRIVACY
};

const USER_ENDPOINT_LLM_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_boundary: ProviderDataBoundary::UserConfiguredEndpoint,
    data_classes_sent: LLM_CONTENT_SENT,
    data_classes_returned: LLM_CONTENT_RETURNED,
    data_residency: ProviderPolicyStatus::UserConfigured,
    ..CLOUD_POLICY_UNKNOWN
};

// Unsourced user-region LLM template. AWS Bedrock now uses the sourced
// `AWS_BEDROCK_LLM_PRIVACY`; kept as the `Unknown` template for the next
// user-region LLM provider that lands without a verifiable policy.
#[allow(dead_code)]
const USER_REGION_LLM_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_boundary: ProviderDataBoundary::UserConfiguredRegion,
    data_classes_sent: LLM_CONTENT_SENT,
    data_classes_returned: LLM_CONTENT_RETURNED,
    data_residency: ProviderPolicyStatus::UserConfigured,
    ..CLOUD_POLICY_UNKNOWN
};

const VENDOR_CLOUD_LLM_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_classes_sent: LLM_CONTENT_SENT,
    data_classes_returned: LLM_CONTENT_RETURNED,
    ..CLOUD_POLICY_UNKNOWN
};

// Unsourced vendor-cloud TTS template. Deepgram Aura now uses the sourced
// `DEEPGRAM_TTS_PRIVACY`; kept as the `Unknown` template for the next cloud TTS
// provider that lands without a verifiable policy.
#[allow(dead_code)]
const VENDOR_CLOUD_TTS_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_classes_sent: TTS_CONTENT_SENT,
    data_classes_returned: TTS_CONTENT_RETURNED,
    ..CLOUD_POLICY_UNKNOWN
};

const PROVIDER_ACCOUNT_REALTIME_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_boundary: ProviderDataBoundary::ProviderAccountBoundary,
    data_classes_sent: REALTIME_AGENT_CONTENT_SENT,
    data_classes_returned: REALTIME_AGENT_CONTENT_RETURNED,
    data_residency: ProviderPolicyStatus::EnterpriseOnly,
    ..CLOUD_POLICY_UNKNOWN
};

// The only vendor-cloud realtime agent (OpenAI Realtime) now uses the sourced
// `OPENAI_REALTIME_AGENT_PRIVACY` below, so the prior unsourced
// `VENDOR_CLOUD_REALTIME[_NO_HEALTH]_PRIVACY` templates were removed. Add a
// fresh unsourced realtime template here if a new realtime provider lands
// without a verifiable policy (keep it `Unknown`, no fabricated URL).

// --- Stage-specific sourced privacy descriptors ----------------------------
// Each layers the verified sourced policy base onto the stage's data classes.

const DEEPGRAM_ASR_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_classes_sent: ASR_CONTENT_SENT,
    data_classes_returned: ASR_CONTENT_RETURNED,
    ..DEEPGRAM_SOURCED_PRIVACY
};

const DEEPGRAM_TTS_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_classes_sent: TTS_CONTENT_SENT,
    data_classes_returned: TTS_CONTENT_RETURNED,
    ..DEEPGRAM_SOURCED_PRIVACY
};

const ASSEMBLYAI_ASR_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_classes_sent: ASR_CONTENT_SENT,
    data_classes_returned: ASR_CONTENT_RETURNED,
    ..ASSEMBLYAI_SOURCED_PRIVACY
};

const AWS_TRANSCRIBE_ASR_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_classes_sent: ASR_CONTENT_SENT,
    data_classes_returned: ASR_CONTENT_RETURNED,
    ..AWS_SOURCED_REGION_PRIVACY
};

const AWS_BEDROCK_LLM_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_classes_sent: LLM_CONTENT_SENT,
    data_classes_returned: LLM_CONTENT_RETURNED,
    ..AWS_SOURCED_REGION_PRIVACY
};

// OpenAI realtime transcription ASR: no provider health/model probe wired, so
// health-check egress is empty (matches the prior _NO_HEALTH treatment).
const OPENAI_ASR_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_classes_sent: ASR_CONTENT_SENT,
    data_classes_returned: ASR_CONTENT_RETURNED,
    health_check_data_classes: NO_DATA_CLASSES,
    ..OPENAI_SOURCED_PRIVACY
};

const OPENAI_REALTIME_AGENT_PRIVACY: ProviderPrivacyDescriptor = ProviderPrivacyDescriptor {
    data_classes_sent: REALTIME_AGENT_CONTENT_SENT,
    data_classes_returned: REALTIME_AGENT_CONTENT_RETURNED,
    health_check_data_classes: NO_DATA_CLASSES,
    ..OPENAI_SOURCED_PRIVACY
};

const ARTIFICIAL_ANALYSIS_STREAMING_STT_SOURCE_URL: &str =
    "https://artificialanalysis.ai/speech-to-text/streaming";
const ARTIFICIAL_ANALYSIS_STREAMING_STT_SOURCE_DATE: &str = "2026-06-25";
const AUTH_REQUIRED_SCHEMA_NOT_WIRED: ProviderRoadmapMetadata = ProviderRoadmapMetadata {
    source_url: ARTIFICIAL_ANALYSIS_STREAMING_STT_SOURCE_URL,
    source_date: ARTIFICIAL_ANALYSIS_STREAMING_STT_SOURCE_DATE,
    auth_schema: ProviderCredentialSchemaStatus::RequiredNotWired,
    not_selectable_reason: Some(
        "Docs-only roadmap candidate; credential schema and runtime adapter are not wired.",
    ),
};

const LOCAL_WHISPER_MODELS: &[LocalModelRequirement] = &[LocalModelRequirement {
    model_id: WHISPER_MODEL_SMALL_EN,
    kind: LocalModelKind::File,
    required_files: LOCAL_WHISPER_FILES,
}];

const SHERPA_MODELS: &[LocalModelRequirement] = &[LocalModelRequirement {
    model_id: SHERPA_ZIPFORMER_20M,
    kind: LocalModelKind::Directory,
    required_files: SHERPA_ZIPFORMER_REQUIRED_FILES,
}];

const MOONSHINE_MODELS: &[LocalModelRequirement] = &[
    LocalModelRequirement {
        model_id: MOONSHINE_SMALL_STREAMING_EN,
        kind: LocalModelKind::Directory,
        required_files: MOONSHINE_STREAMING_REQUIRED_FILES,
    },
    LocalModelRequirement {
        model_id: MOONSHINE_MEDIUM_STREAMING_EN,
        kind: LocalModelKind::Directory,
        required_files: MOONSHINE_STREAMING_REQUIRED_FILES,
    },
    LocalModelRequirement {
        model_id: MOONSHINE_TINY_STREAMING_EN,
        kind: LocalModelKind::Directory,
        required_files: MOONSHINE_STREAMING_REQUIRED_FILES,
    },
];

const SORTFORMER_MODELS: &[LocalModelRequirement] = &[LocalModelRequirement {
    model_id: SORTFORMER_MODEL_FILENAME,
    kind: LocalModelKind::File,
    required_files: SORTFORMER_FILES,
}];

const CLUSTERING_DIARIZATION_MODELS: &[LocalModelRequirement] = &[
    LocalModelRequirement {
        model_id: DIAR_SEG_PYANNOTE_DIR,
        kind: LocalModelKind::Directory,
        required_files: DIAR_SEG_PYANNOTE_REQUIRED_FILES,
    },
    LocalModelRequirement {
        model_id: DIAR_EMB_TITANET_FILENAME,
        kind: LocalModelKind::File,
        required_files: TITANET_FILES,
    },
];

const LOCAL_LLM_MODELS: &[LocalModelRequirement] = &[LocalModelRequirement {
    model_id: LLM_MODEL_FILENAME,
    kind: LocalModelKind::File,
    required_files: LOCAL_LLM_FILES,
}];

const CEREBRAS_MODEL_CATALOG: &[ProviderModelCatalogItem] = &[
    ProviderModelCatalogItem {
        id: CEREBRAS_DEFAULT_MODEL,
        display_name: "OpenAI GPT OSS 120B",
        is_default: true,
    },
    ProviderModelCatalogItem {
        id: CEREBRAS_PREVIEW_MODEL,
        display_name: "Z.ai GLM 4.7 (preview)",
        is_default: false,
    },
];

const DEEPGRAM_AURA_VOICE_CATALOG: &[ProviderModelCatalogItem] = &[
    ProviderModelCatalogItem {
        id: "aura-asteria-en",
        display_name: "Asteria (en, female)",
        is_default: true,
    },
    ProviderModelCatalogItem {
        id: "aura-luna-en",
        display_name: "Luna (en, female)",
        is_default: false,
    },
    ProviderModelCatalogItem {
        id: "aura-stella-en",
        display_name: "Stella (en, female)",
        is_default: false,
    },
    ProviderModelCatalogItem {
        id: "aura-athena-en",
        display_name: "Athena (en, female)",
        is_default: false,
    },
    ProviderModelCatalogItem {
        id: "aura-hera-en",
        display_name: "Hera (en, female)",
        is_default: false,
    },
    ProviderModelCatalogItem {
        id: "aura-orion-en",
        display_name: "Orion (en, male)",
        is_default: false,
    },
    ProviderModelCatalogItem {
        id: "aura-arcas-en",
        display_name: "Arcas (en, male)",
        is_default: false,
    },
    ProviderModelCatalogItem {
        id: "aura-perseus-en",
        display_name: "Perseus (en, male)",
        is_default: false,
    },
    ProviderModelCatalogItem {
        id: "aura-angus-en",
        display_name: "Angus (en, male)",
        is_default: false,
    },
    ProviderModelCatalogItem {
        id: "aura-orpheus-en",
        display_name: "Orpheus (en, male)",
        is_default: false,
    },
    ProviderModelCatalogItem {
        id: "aura-helios-en",
        display_name: "Helios (en, male)",
        is_default: false,
    },
    ProviderModelCatalogItem {
        id: "aura-zeus-en",
        display_name: "Zeus (en, male)",
        is_default: false,
    },
];

const GOOGLE_CHIRP_ENDPOINT_MODES: &[ProviderEndpointMode] = &[
    ProviderEndpointMode::DefaultRegion,
    ProviderEndpointMode::CustomEndpoint,
];

const GOOGLE_CHIRP_PACKAGING: &[ProviderPackagingRequirement] =
    &[ProviderPackagingRequirement::ProtobufGrpcClient];

const GOOGLE_CHIRP_HEALTH_PROBES: &[ProviderHealthProbeKind] = &[
    ProviderHealthProbeKind::TokenAcquisition,
    ProviderHealthProbeKind::MetadataOnly,
    ProviderHealthProbeKind::StreamingRpcAvailability,
    ProviderHealthProbeKind::LiveEnvGatedSmoke,
];

const GOOGLE_CHIRP_ENTERPRISE: ProviderEnterpriseMetadata = ProviderEnterpriseMetadata {
    endpoint_modes: GOOGLE_CHIRP_ENDPOINT_MODES,
    packaging: GOOGLE_CHIRP_PACKAGING,
    speaker_semantics: ProviderSpeakerSemantics {
        label_support: ProviderSpeakerLabelSupport::StreamingUnverified,
        interim_labels_may_be_unknown: true,
        speaker_ids_are_stable_identity: false,
        local_timeline_recommended: true,
    },
    health_probes: GOOGLE_CHIRP_HEALTH_PROBES,
};

const AZURE_SPEECH_ENDPOINT_MODES: &[ProviderEndpointMode] = &[
    ProviderEndpointMode::DefaultRegion,
    ProviderEndpointMode::CustomEndpoint,
    ProviderEndpointMode::PrivateEndpoint,
    ProviderEndpointMode::SovereignCloud,
];

const AZURE_SPEECH_PACKAGING: &[ProviderPackagingRequirement] = &[
    ProviderPackagingRequirement::NativeSdkAssets,
    ProviderPackagingRequirement::NativeFrameworkAssets,
    ProviderPackagingRequirement::SystemLibraries,
    ProviderPackagingRequirement::SystemCertificates,
    ProviderPackagingRequirement::VisualCppRedistributable,
];

const AZURE_SPEECH_HEALTH_PROBES: &[ProviderHealthProbeKind] = &[
    ProviderHealthProbeKind::TokenAcquisition,
    ProviderHealthProbeKind::SdkDependency,
    ProviderHealthProbeKind::EndpointConnectivity,
    ProviderHealthProbeKind::LiveEnvGatedSmoke,
];

const AZURE_SPEECH_ENTERPRISE: ProviderEnterpriseMetadata = ProviderEnterpriseMetadata {
    endpoint_modes: AZURE_SPEECH_ENDPOINT_MODES,
    packaging: AZURE_SPEECH_PACKAGING,
    speaker_semantics: ProviderSpeakerSemantics {
        label_support: ProviderSpeakerLabelSupport::StreamingProviderLabels,
        interim_labels_may_be_unknown: true,
        speaker_ids_are_stable_identity: false,
        local_timeline_recommended: true,
    },
    health_probes: AZURE_SPEECH_HEALTH_PROBES,
};

const NEMOTRON_ASR_ENDPOINT_MODES: &[ProviderEndpointMode] = &[
    ProviderEndpointMode::CustomEndpoint,
    ProviderEndpointMode::PrivateEndpoint,
];

const NEMOTRON_ASR_PACKAGING: &[ProviderPackagingRequirement] = &[
    ProviderPackagingRequirement::ProtobufGrpcClient,
    ProviderPackagingRequirement::SidecarProcess,
];

const NEMOTRON_ASR_HEALTH_PROBES: &[ProviderHealthProbeKind] = &[
    ProviderHealthProbeKind::MetadataOnly,
    ProviderHealthProbeKind::StreamingRpcAvailability,
    ProviderHealthProbeKind::LiveEnvGatedSmoke,
];

const NEMOTRON_ASR_ENTERPRISE: ProviderEnterpriseMetadata = ProviderEnterpriseMetadata {
    endpoint_modes: NEMOTRON_ASR_ENDPOINT_MODES,
    packaging: NEMOTRON_ASR_PACKAGING,
    speaker_semantics: ProviderSpeakerSemantics {
        label_support: ProviderSpeakerLabelSupport::None,
        interim_labels_may_be_unknown: false,
        speaker_ids_are_stable_identity: false,
        local_timeline_recommended: true,
    },
    health_probes: NEMOTRON_ASR_HEALTH_PROBES,
};

const ALIBABA_QWEN_ASR_FLASH_ENDPOINT_MODES: &[ProviderEndpointMode] = &[
    ProviderEndpointMode::DefaultRegion,
    ProviderEndpointMode::CustomEndpoint,
];

const ALIBABA_QWEN_ASR_FLASH_PACKAGING: &[ProviderPackagingRequirement] =
    &[ProviderPackagingRequirement::SystemCertificates];

const ALIBABA_QWEN_ASR_FLASH_ENTERPRISE: ProviderEnterpriseMetadata = ProviderEnterpriseMetadata {
    endpoint_modes: ALIBABA_QWEN_ASR_FLASH_ENDPOINT_MODES,
    packaging: ALIBABA_QWEN_ASR_FLASH_PACKAGING,
    speaker_semantics: ProviderSpeakerSemantics {
        label_support: ProviderSpeakerLabelSupport::None,
        interim_labels_may_be_unknown: false,
        speaker_ids_are_stable_identity: false,
        local_timeline_recommended: true,
    },
    health_probes: &[],
};

pub const PROVIDER_REGISTRY: &[ProviderDescriptor] = &[
    // ASR providers
    ProviderDescriptor {
        id: "asr.local_whisper",
        display_name: "Local Whisper",
        stage: ProviderStage::Asr,
        settings_variant: "local_whisper",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::Local,
        credential_keys: &[],
        required_features: LOCAL_WHISPER_FEATURES,
        model_catalog: ModelCatalogPolicy::LocalFiles,
        local_models: LOCAL_WHISPER_MODELS,
        fixed_model_catalog: None,
        default_model: Some(WHISPER_MODEL_SMALL_EN),
        health_check_command: None,
        model_catalog_command: Some("list_available_models"),
        source_policy: Some(ProviderSourcePolicy::MultiSourceIndependent),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::TranscriptFinalOnly),
        settings_groups: BASIC_MODEL_GROUPS,
        audio_input: Some(LOCAL_F32_AUDIO_INPUT),
        lifecycle: LOCAL_IN_PROCESS_LIFECYCLE,
        privacy: LOCAL_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: false,
        supports_partial_revisions: false,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.api",
        display_name: "OpenAI-compatible batch ASR",
        stage: ProviderStage::Asr,
        settings_variant: "api",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::Http,
        credential_keys: OPENAI_COMPAT_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::UserSupplied,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: None,
        health_check_command: Some("test_cloud_asr_connection"),
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::MultiSourceIndependent),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::TranscriptFinalOnly),
        settings_groups: BASIC_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(BATCH_WAV_AUDIO_INPUT),
        lifecycle: OPENAI_COMPAT_HTTP_LIFECYCLE,
        privacy: USER_ENDPOINT_ASR_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: false,
        supports_partial_revisions: false,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "asr.aws_transcribe",
        display_name: "AWS Transcribe streaming",
        stage: ProviderStage::Asr,
        settings_variant: "aws_transcribe",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::AwsSdk,
        credential_keys: AWS_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("transcribe-streaming"),
        health_check_command: Some("test_aws_credentials"),
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("AWS Transcribe streaming"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(AWS_EVENTSTREAM_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: AWS_STREAMING_LIFECYCLE,
        privacy: AWS_TRANSCRIBE_ASR_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.deepgram",
        display_name: "Deepgram streaming",
        stage: ProviderStage::Asr,
        settings_variant: "deepgram",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::WebSocket,
        credential_keys: &["deepgram_api_key"],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::RemoteCommand,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("nova-3"),
        health_check_command: Some("test_deepgram_connection"),
        model_catalog_command: Some("list_deepgram_models_cmd"),
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: DEEPGRAM_LISTEN_LIFECYCLE,
        privacy: DEEPGRAM_ASR_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.assemblyai",
        display_name: "AssemblyAI streaming",
        stage: ProviderStage::Asr,
        settings_variant: "assemblyai",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::WebSocket,
        credential_keys: &["assemblyai_api_key"],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("universal-3-5-pro"),
        health_check_command: Some("test_assemblyai_connection"),
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("AssemblyAI streaming"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: ASSEMBLYAI_LIFECYCLE,
        privacy: ASSEMBLYAI_ASR_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.sherpa_onnx",
        display_name: "Sherpa-ONNX streaming",
        stage: ProviderStage::Asr,
        settings_variant: "sherpa_onnx",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::Local,
        credential_keys: &[],
        required_features: SHERPA_STREAMING_FEATURES,
        model_catalog: ModelCatalogPolicy::LocalFiles,
        local_models: SHERPA_MODELS,
        fixed_model_catalog: None,
        default_model: Some(SHERPA_ZIPFORMER_20M),
        health_check_command: None,
        model_catalog_command: Some("list_available_models"),
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("Sherpa-ONNX streaming"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinal),
        settings_groups: BASIC_MODEL_GROUPS,
        audio_input: Some(LOCAL_F32_AUDIO_INPUT),
        lifecycle: LOCAL_STREAMING_LIFECYCLE,
        privacy: LOCAL_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "asr.moonshine",
        display_name: "Moonshine local streaming",
        stage: ProviderStage::Asr,
        settings_variant: "moonshine",
        status: ProviderStatus::Planned,
        transport: ProviderTransport::Local,
        credential_keys: &[],
        required_features: MOONSHINE_FEATURES,
        model_catalog: ModelCatalogPolicy::LocalFiles,
        local_models: MOONSHINE_MODELS,
        fixed_model_catalog: None,
        default_model: Some(MOONSHINE_SMALL_STREAMING_EN),
        health_check_command: None,
        model_catalog_command: Some("list_available_models"),
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("Moonshine local streaming"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_GROUPS,
        audio_input: Some(LOCAL_F32_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: LOCAL_STREAMING_LIFECYCLE,
        privacy: LOCAL_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.openai_realtime",
        display_name: "OpenAI Realtime transcription",
        stage: ProviderStage::Asr,
        settings_variant: "openai_realtime",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::WebSocket,
        credential_keys: &["openai_api_key"],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some(OPENAI_REALTIME_TRANSCRIPTION_DEFAULT_MODEL),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinal),
        settings_groups: BASIC_MODEL_HEALTH_GROUPS,
        audio_input: Some(WS_JSON_BASE64_PCM16_24K_AUDIO_INPUT),
        lifecycle: OPENAI_REALTIME_LIFECYCLE,
        privacy: OPENAI_ASR_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "asr.soniox",
        display_name: "Soniox realtime",
        stage: ProviderStage::Asr,
        settings_variant: "soniox",
        status: ProviderStatus::Planned,
        transport: ProviderTransport::WebSocket,
        credential_keys: SONIOX_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::RemoteCommand,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("stt-rt-v5"),
        health_check_command: Some("test_soniox_connection"),
        model_catalog_command: Some("list_soniox_models_cmd"),
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.gladia",
        display_name: "Gladia Solaria live",
        stage: ProviderStage::Asr,
        settings_variant: "gladia",
        status: ProviderStatus::Planned,
        transport: ProviderTransport::RestInitWebSocket,
        credential_keys: GLADIA_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("solaria-1"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_AUDIO_INPUT),
        lifecycle: GLADIA_LIVE_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "asr.speechmatics",
        display_name: "Speechmatics realtime enhanced",
        stage: ProviderStage::Asr,
        settings_variant: "speechmatics",
        status: ProviderStatus::Planned,
        transport: ProviderTransport::WebSocket,
        credential_keys: SPEECHMATICS_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("enhanced"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.elevenlabs_scribe",
        display_name: "ElevenLabs Scribe realtime",
        stage: ProviderStage::Asr,
        settings_variant: "elevenlabs_scribe",
        status: ProviderStatus::Planned,
        transport: ProviderTransport::WebSocket,
        credential_keys: ELEVENLABS_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("scribe_v2_realtime"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.revai",
        display_name: "Rev AI realtime",
        stage: ProviderStage::Asr,
        settings_variant: "revai",
        status: ProviderStatus::Planned,
        transport: ProviderTransport::WebSocket,
        credential_keys: REVAI_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("machine_v2"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinal),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.google_chirp3",
        display_name: "Google Chirp 3",
        stage: ProviderStage::Asr,
        settings_variant: "google_chirp3",
        status: ProviderStatus::Planned,
        transport: ProviderTransport::GrpcBidi,
        credential_keys: GOOGLE_STT_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("chirp_3"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: Some("Google Speech-to-Text v2 streaming"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinal),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(GRPC_STREAMING_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: GOOGLE_STT_GRPC_LIFECYCLE,
        privacy: PROVIDER_ACCOUNT_ASR_NO_HEALTH_PRIVACY,
        enterprise: Some(GOOGLE_CHIRP_ENTERPRISE),
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.azure_speech",
        display_name: "Azure Speech",
        stage: ProviderStage::Asr,
        settings_variant: "azure_speech",
        status: ProviderStatus::Planned,
        transport: ProviderTransport::SdkNative,
        credential_keys: AZURE_SPEECH_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::None,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: None,
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: Some("Azure Speech SDK conversation stream"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(SDK_NATIVE_PCM16_16K_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: AZURE_SPEECH_SDK_LIFECYCLE,
        privacy: USER_REGION_ASR_NO_HEALTH_PRIVACY,
        enterprise: Some(AZURE_SPEECH_ENTERPRISE),
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.xai_grok_stt",
        display_name: "xAI Grok Speech to Text Streaming",
        stage: ProviderStage::Asr,
        settings_variant: "xai_grok_stt",
        status: ProviderStatus::Watch,
        transport: ProviderTransport::WebSocket,
        credential_keys: &[],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("grok-stt"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("xAI STT watch metadata"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinal),
        settings_groups: BASIC_MODEL_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_XAI_SPEAKER_LABEL_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY,
        enterprise: None,
        roadmap: Some(AUTH_REQUIRED_SCHEMA_NOT_WIRED),
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "asr.nvidia_nemotron_asr",
        display_name: "NVIDIA/Together Nemotron ASR",
        stage: ProviderStage::Asr,
        settings_variant: "nvidia_nemotron_asr",
        status: ProviderStatus::EnterpriseWatch,
        transport: ProviderTransport::GrpcBidi,
        credential_keys: &[],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("nemotron-asr"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("NIM/Together deployment profile watch metadata"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinal),
        settings_groups: BASIC_MODEL_ADVANCED_GROUPS,
        audio_input: Some(GRPC_STREAMING_PCM16_16K_AUDIO_INPUT),
        lifecycle: WATCHLIST_GRPC_AUTH_REQUIRED_LIFECYCLE,
        privacy: USER_ENDPOINT_ASR_NO_HEALTH_PRIVACY,
        enterprise: Some(NEMOTRON_ASR_ENTERPRISE),
        roadmap: Some(AUTH_REQUIRED_SCHEMA_NOT_WIRED),
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "asr.inworld_stt1",
        display_name: "Inworld STT 1 Realtime",
        stage: ProviderStage::Asr,
        settings_variant: "inworld_stt1",
        status: ProviderStatus::Watch,
        transport: ProviderTransport::WebSocket,
        credential_keys: &[],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("inworld-stt-1"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("Inworld STT watch metadata"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_ADVANCED_GROUPS,
        audio_input: Some(WS_JSON_BASE64_PCM16_16K_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY,
        enterprise: None,
        roadmap: Some(AUTH_REQUIRED_SCHEMA_NOT_WIRED),
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "asr.smallest_pulse",
        display_name: "Smallest.ai Pulse realtime",
        stage: ProviderStage::Asr,
        settings_variant: "smallest_pulse",
        status: ProviderStatus::Watch,
        transport: ProviderTransport::WebSocket,
        credential_keys: &[],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("pulse"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("Smallest.ai Pulse watch metadata"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinal),
        settings_groups: BASIC_MODEL_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY,
        enterprise: None,
        roadmap: Some(AUTH_REQUIRED_SCHEMA_NOT_WIRED),
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "asr.gradium_stt",
        display_name: "Gradium STT Realtime",
        stage: ProviderStage::Asr,
        settings_variant: "gradium_stt",
        status: ProviderStatus::Watch,
        transport: ProviderTransport::WebSocket,
        credential_keys: &[],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("gradium-stt-realtime"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("Gradium STT watch metadata"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_ADVANCED_GROUPS,
        audio_input: Some(WS_JSON_BASE64_PCM16_16K_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY,
        enterprise: None,
        roadmap: Some(AUTH_REQUIRED_SCHEMA_NOT_WIRED),
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "asr.mistral_voxtral_realtime",
        display_name: "Mistral Voxtral Mini Transcribe Realtime",
        stage: ProviderStage::Asr,
        settings_variant: "mistral_voxtral_realtime",
        status: ProviderStatus::Watch,
        transport: ProviderTransport::WebSocket,
        credential_keys: &[],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("voxtral-mini-transcribe-realtime-2602"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("Mistral Voxtral realtime watch metadata"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinal),
        settings_groups: BASIC_MODEL_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY,
        enterprise: None,
        roadmap: Some(AUTH_REQUIRED_SCHEMA_NOT_WIRED),
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "asr.alibaba_qwen3_asr_flash",
        display_name: "Alibaba/Qwen3 ASR Flash Realtime",
        stage: ProviderStage::Asr,
        settings_variant: "alibaba_qwen3_asr_flash",
        status: ProviderStatus::EnterpriseWatch,
        transport: ProviderTransport::WebSocket,
        credential_keys: &[],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("qwen3-asr-flash-realtime"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("DashScope/Qwen regional endpoint watch metadata"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinal),
        settings_groups: BASIC_MODEL_ADVANCED_GROUPS,
        audio_input: Some(WS_JSON_BASE64_PCM16_16K_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: USER_REGION_ASR_NO_HEALTH_PRIVACY,
        enterprise: Some(ALIBABA_QWEN_ASR_FLASH_ENTERPRISE),
        roadmap: Some(AUTH_REQUIRED_SCHEMA_NOT_WIRED),
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "asr.cartesia_ink2",
        display_name: "Cartesia Ink-2 Realtime STT",
        stage: ProviderStage::Asr,
        settings_variant: "cartesia_ink2",
        status: ProviderStatus::Watch,
        transport: ProviderTransport::WebSocket,
        credential_keys: &[],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("ink-2"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("Cartesia Ink-2 auto/manual STT watch metadata"),
        event_semantics: Some(ProviderEventSemantics::TranscriptPartialFinalTurns),
        settings_groups: BASIC_MODEL_ADVANCED_GROUPS,
        audio_input: Some(WS_BINARY_PCM16_16K_AUDIO_INPUT),
        lifecycle: PLANNED_STREAMING_STT_LIFECYCLE,
        privacy: VENDOR_CLOUD_ASR_NO_HEALTH_PRIVACY,
        enterprise: None,
        roadmap: Some(AUTH_REQUIRED_SCHEMA_NOT_WIRED),
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "diarization.sortformer",
        display_name: "Sortformer speaker diarization",
        stage: ProviderStage::Diarization,
        settings_variant: "sortformer",
        status: ProviderStatus::Planned,
        transport: ProviderTransport::Local,
        credential_keys: &[],
        required_features: &["diarization"],
        model_catalog: ModelCatalogPolicy::LocalFiles,
        local_models: SORTFORMER_MODELS,
        fixed_model_catalog: None,
        default_model: Some(SORTFORMER_MODEL_FILENAME),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("local_rolling_speaker_timeline"),
        event_semantics: None,
        settings_groups: BASIC_MODEL_ADVANCED_GROUPS,
        audio_input: Some(DIARIZATION_LOCAL_TIMELINE_AUDIO_INPUT),
        lifecycle: LOCAL_STREAMING_LIFECYCLE,
        privacy: LOCAL_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    ProviderDescriptor {
        id: "diarization.clustering",
        display_name: "Unbounded clustering diarization",
        stage: ProviderStage::Diarization,
        settings_variant: "clustering",
        status: ProviderStatus::Planned,
        transport: ProviderTransport::Local,
        credential_keys: &[],
        required_features: &["diarization-clustering"],
        model_catalog: ModelCatalogPolicy::LocalFiles,
        local_models: CLUSTERING_DIARIZATION_MODELS,
        fixed_model_catalog: None,
        default_model: None,
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::SingleSession),
        source_policy_label: Some("local_rolling_speaker_timeline"),
        event_semantics: None,
        settings_groups: BASIC_MODEL_ADVANCED_GROUPS,
        audio_input: Some(DIARIZATION_LOCAL_TIMELINE_AUDIO_INPUT),
        lifecycle: LOCAL_STREAMING_LIFECYCLE,
        privacy: LOCAL_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: true,
    },
    // LLM providers
    ProviderDescriptor {
        id: "llm.local_llama",
        display_name: "Local llama.cpp",
        stage: ProviderStage::Llm,
        settings_variant: "local_llama",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::Local,
        credential_keys: &[],
        required_features: LOCAL_LLAMA_FEATURES,
        model_catalog: ModelCatalogPolicy::LocalFiles,
        local_models: LOCAL_LLM_MODELS,
        fixed_model_catalog: None,
        default_model: Some(LLM_MODEL_FILENAME),
        health_check_command: None,
        model_catalog_command: Some("list_available_models"),
        source_policy: None,
        source_policy_label: None,
        event_semantics: None,
        settings_groups: BASIC_MODEL_GROUPS,
        audio_input: None,
        lifecycle: LOCAL_IN_PROCESS_LIFECYCLE,
        privacy: LOCAL_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: false,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "llm.api",
        display_name: "OpenAI-compatible LLM",
        stage: ProviderStage::Llm,
        settings_variant: "api",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::Http,
        credential_keys: OPENAI_COMPAT_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::RemoteCommand,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: None,
        health_check_command: Some("test_openai_compatible_llm_connection_cmd"),
        model_catalog_command: Some("list_openai_compatible_llm_models_cmd"),
        source_policy: None,
        source_policy_label: None,
        event_semantics: None,
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: None,
        lifecycle: OPENAI_COMPAT_HTTP_LIFECYCLE,
        privacy: USER_ENDPOINT_LLM_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: false,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "llm.cerebras",
        display_name: "Cerebras",
        stage: ProviderStage::Llm,
        settings_variant: "cerebras",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::Http,
        credential_keys: CEREBRAS_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::RemoteCommand,
        local_models: &[],
        fixed_model_catalog: Some(CEREBRAS_MODEL_CATALOG),
        default_model: Some(CEREBRAS_DEFAULT_MODEL),
        health_check_command: Some("test_cerebras_connection_cmd"),
        model_catalog_command: Some("list_cerebras_models_cmd"),
        source_policy: None,
        source_policy_label: None,
        event_semantics: None,
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: None,
        lifecycle: SAVED_KEY_HTTP_LIFECYCLE,
        privacy: VENDOR_CLOUD_LLM_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: false,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "llm.openrouter",
        display_name: "OpenRouter",
        stage: ProviderStage::Llm,
        settings_variant: "openrouter",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::Http,
        credential_keys: &["openrouter_api_key"],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::RemoteCommand,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: None,
        health_check_command: Some("test_openrouter_connection_cmd"),
        model_catalog_command: Some("list_openrouter_models_cmd"),
        source_policy: None,
        source_policy_label: None,
        event_semantics: None,
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: None,
        lifecycle: SAVED_KEY_HTTP_LIFECYCLE,
        privacy: USER_ENDPOINT_LLM_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: false,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "llm.aws_bedrock",
        display_name: "AWS Bedrock",
        stage: ProviderStage::Llm,
        settings_variant: "aws_bedrock",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::AwsSdk,
        credential_keys: AWS_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::UserSupplied,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: None,
        health_check_command: Some("test_aws_credentials"),
        model_catalog_command: None,
        source_policy: None,
        source_policy_label: None,
        event_semantics: None,
        settings_groups: BASIC_HEALTH_ADVANCED_GROUPS,
        audio_input: None,
        lifecycle: AWS_REQUEST_LIFECYCLE,
        privacy: AWS_BEDROCK_LLM_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: false,
        supports_partial_revisions: false,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "llm.mistralrs",
        display_name: "mistral.rs local",
        stage: ProviderStage::Llm,
        settings_variant: "mistralrs",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::Local,
        credential_keys: &[],
        required_features: MISTRALRS_FEATURES,
        model_catalog: ModelCatalogPolicy::LocalFiles,
        local_models: LOCAL_LLM_MODELS,
        fixed_model_catalog: None,
        default_model: Some(LLM_MODEL_FILENAME),
        health_check_command: None,
        model_catalog_command: Some("list_available_models"),
        source_policy: None,
        source_policy_label: None,
        event_semantics: None,
        settings_groups: BASIC_MODEL_GROUPS,
        audio_input: None,
        lifecycle: LOCAL_IN_PROCESS_LIFECYCLE,
        privacy: LOCAL_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: false,
        supports_partial_revisions: false,
        supports_diarization: false,
    },
    // TTS providers
    ProviderDescriptor {
        id: "tts.none",
        display_name: "TTS disabled",
        stage: ProviderStage::Tts,
        settings_variant: "none",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::Local,
        credential_keys: &[],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::None,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: None,
        health_check_command: None,
        model_catalog_command: None,
        source_policy: None,
        source_policy_label: None,
        event_semantics: None,
        settings_groups: BASIC_ONLY_GROUPS,
        audio_input: None,
        lifecycle: NOOP_LIFECYCLE,
        privacy: LOCAL_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: false,
        supports_partial_revisions: false,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "tts.deepgram_aura",
        display_name: "Deepgram Aura",
        stage: ProviderStage::Tts,
        settings_variant: "deepgram_aura",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::WebSocket,
        credential_keys: &["deepgram_api_key"],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: Some(DEEPGRAM_AURA_VOICE_CATALOG),
        default_model: Some("aura-asteria-en"),
        health_check_command: Some("test_tts_connection_cmd"),
        model_catalog_command: None,
        source_policy: None,
        source_policy_label: None,
        event_semantics: None,
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: None,
        lifecycle: DEEPGRAM_AURA_LIFECYCLE,
        privacy: DEEPGRAM_TTS_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: false,
        supports_diarization: false,
    },
    // Native realtime-agent surfaces
    ProviderDescriptor {
        id: "realtime_agent.gemini_live",
        display_name: "Gemini Live",
        stage: ProviderStage::RealtimeAgent,
        settings_variant: "gemini",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::WebSocket,
        credential_keys: GEMINI_CREDENTIAL_KEYS,
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("gemini-2.0-flash-live-001"),
        health_check_command: Some("test_gemini_api_key"),
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::NativeRealtimeAudioText),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(WS_JSON_BASE64_PCM16_16K_AUDIO_INPUT),
        lifecycle: GOOGLE_REALTIME_LIFECYCLE,
        privacy: PROVIDER_ACCOUNT_REALTIME_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
    ProviderDescriptor {
        id: "realtime_agent.openai_realtime",
        display_name: "OpenAI Realtime voice agent",
        stage: ProviderStage::RealtimeAgent,
        settings_variant: "openai_realtime_agent",
        status: ProviderStatus::Implemented,
        transport: ProviderTransport::WebSocket,
        credential_keys: &["openai_api_key"],
        required_features: &[],
        model_catalog: ModelCatalogPolicy::Fixed,
        local_models: &[],
        fixed_model_catalog: None,
        default_model: Some("gpt-realtime-2"),
        health_check_command: None,
        model_catalog_command: None,
        source_policy: Some(ProviderSourcePolicy::MultiSourceMixed),
        source_policy_label: None,
        event_semantics: Some(ProviderEventSemantics::NativeRealtimeAudioText),
        settings_groups: BASIC_MODEL_HEALTH_ADVANCED_GROUPS,
        audio_input: Some(WS_JSON_BASE64_PCM16_24K_AUDIO_INPUT),
        lifecycle: OPENAI_REALTIME_LIFECYCLE,
        privacy: OPENAI_REALTIME_AGENT_PRIVACY,
        enterprise: None,
        roadmap: None,
        supports_streaming: true,
        supports_partial_revisions: true,
        supports_diarization: false,
    },
];

pub fn provider_registry() -> &'static [ProviderDescriptor] {
    PROVIDER_REGISTRY
}

pub fn provider_registry_typescript_module() -> String {
    let json = serde_json::to_string_pretty(provider_registry())
        .expect("provider registry descriptors must serialize to JSON");
    format!(
        "// @generated by src-tauri/crates/provider-registry/src/lib.rs. Do not edit manually.\n\
         import type {{ ProviderDescriptor }} from \"../types\";\n\n\
         export const GENERATED_PROVIDER_REGISTRY = {json} satisfies ProviderDescriptor[];\n"
    )
}

pub fn descriptor_by_id(id: &str) -> &'static ProviderDescriptor {
    provider_registry()
        .iter()
        .find(|descriptor| descriptor.id == id)
        .unwrap_or_else(|| panic!("provider descriptor missing for id: {id}"))
}

#[cfg(test)]
mod registry_tests {
    use std::collections::HashSet;

    use super::*;

    fn is_runtime_content_data_class(data_class: &ProviderDataClass) -> bool {
        matches!(
            data_class,
            ProviderDataClass::Audio
                | ProviderDataClass::TranscriptText
                | ProviderDataClass::PromptText
                | ProviderDataClass::Notes
                | ProviderDataClass::GraphContext
                | ProviderDataClass::GeneratedText
                | ProviderDataClass::GeneratedAudio
        )
    }

    fn is_readiness_only_data_class(data_class: &ProviderDataClass) -> bool {
        matches!(
            data_class,
            ProviderDataClass::CredentialAuth
                | ProviderDataClass::ProviderConfiguration
                | ProviderDataClass::ModelCatalogMetadata
        )
    }

    fn provider_sends_runtime_content(descriptor: &ProviderDescriptor) -> bool {
        descriptor
            .privacy
            .data_classes_sent
            .iter()
            .any(is_runtime_content_data_class)
    }

    #[test]
    fn provider_ids_are_unique() {
        let mut ids = HashSet::new();
        for descriptor in provider_registry() {
            assert!(
                ids.insert(descriptor.id),
                "duplicate provider id {}",
                descriptor.id
            );
        }
    }

    #[test]
    fn generated_typescript_mentions_the_lightweight_crate() {
        let module = provider_registry_typescript_module();
        assert!(module.contains("@generated by src-tauri/crates/provider-registry/src/lib.rs"));
        assert!(module.contains("GENERATED_PROVIDER_REGISTRY"));
        assert!(module.contains("asr.soniox"));
        assert!(module.contains("asr.xai_grok_stt"));
    }

    #[test]
    fn planned_streaming_stt_candidates_have_runtime_contracts() {
        for (id, event_semantics) in [
            (
                "asr.soniox",
                ProviderEventSemantics::TranscriptPartialFinalTurns,
            ),
            (
                "asr.speechmatics",
                ProviderEventSemantics::TranscriptPartialFinalTurns,
            ),
            (
                "asr.elevenlabs_scribe",
                ProviderEventSemantics::TranscriptPartialFinalTurns,
            ),
            ("asr.revai", ProviderEventSemantics::TranscriptPartialFinal),
        ] {
            let descriptor = descriptor_by_id(id);
            assert_eq!(descriptor.stage, ProviderStage::Asr);
            assert_eq!(descriptor.status, ProviderStatus::Planned);
            assert_eq!(descriptor.transport, ProviderTransport::WebSocket);
            assert_eq!(descriptor.event_semantics, Some(event_semantics));
            assert!(descriptor.supports_streaming);
            assert!(descriptor.supports_partial_revisions);
            assert!(descriptor.supports_diarization);
            assert_eq!(
                descriptor.audio_input.unwrap().pipeline_format,
                PIPELINE_F32_16K_MONO
            );
        }

        let gladia = descriptor_by_id("asr.gladia");
        assert_eq!(gladia.stage, ProviderStage::Asr);
        assert_eq!(gladia.status, ProviderStatus::Planned);
        assert_eq!(gladia.transport, ProviderTransport::RestInitWebSocket);
        assert_eq!(
            gladia.event_semantics,
            Some(ProviderEventSemantics::TranscriptPartialFinalTurns)
        );
        assert!(gladia.supports_streaming);
        assert!(gladia.supports_partial_revisions);
        assert!(!gladia.supports_diarization);
        assert_eq!(
            gladia.audio_input.unwrap().transport_encoding,
            ProviderAudioTransportEncoding::WebSocketBinary
        );
        assert_eq!(gladia.lifecycle, GLADIA_LIVE_LIFECYCLE);
    }

    #[test]
    fn remote_content_egress_providers_declare_runtime_registry_contracts() {
        for descriptor in provider_registry() {
            if !descriptor.privacy.data_leaves_device || !provider_sends_runtime_content(descriptor)
            {
                continue;
            }

            assert_ne!(
                descriptor.privacy.data_boundary,
                ProviderDataBoundary::LocalOnly,
                "{} remote content egress must declare a non-local data boundary",
                descriptor.id
            );
            assert!(
                !descriptor.privacy.data_classes_sent.is_empty(),
                "{} remote content egress must declare sent data classes",
                descriptor.id
            );
            assert!(
                !descriptor.privacy.data_classes_returned.is_empty(),
                "{} remote content egress must declare returned data classes",
                descriptor.id
            );
            assert_eq!(
                descriptor.privacy.sensitive_error_policy,
                ProviderSensitiveErrorPolicy::AudioGraphRedacted,
                "{} remote content diagnostics must stay redacted",
                descriptor.id
            );

            let has_probe = descriptor.health_check_command.is_some()
                || descriptor.model_catalog_command.is_some();
            if has_probe {
                assert!(
                    !descriptor.privacy.health_check_data_classes.is_empty(),
                    "{} readiness/model probes must declare their non-content data classes",
                    descriptor.id
                );
            } else {
                assert!(
                    descriptor.privacy.health_check_data_classes.is_empty(),
                    "{} cannot declare readiness data egress without a readiness/model command",
                    descriptor.id
                );
            }
            assert!(
                descriptor
                    .privacy
                    .health_check_data_classes
                    .iter()
                    .all(is_readiness_only_data_class),
                "{} readiness/model probes must not send audio, transcript, prompt, notes, graph, generated, or org content",
                descriptor.id
            );

            match descriptor.transport {
                ProviderTransport::Local => panic!(
                    "{} cannot use local transport metadata while declaring off-device content egress",
                    descriptor.id
                ),
                ProviderTransport::Http => assert_eq!(
                    descriptor.lifecycle.session,
                    ProviderSessionLifecycle::PerRequest,
                    "{} HTTP content egress must declare per-request lifecycle",
                    descriptor.id
                ),
                ProviderTransport::WebSocket | ProviderTransport::RestInitWebSocket => assert_eq!(
                    descriptor.lifecycle.session,
                    ProviderSessionLifecycle::LongLivedWebSocket,
                    "{} streaming socket content egress must declare long-lived socket lifecycle",
                    descriptor.id
                ),
                ProviderTransport::AwsSdk => assert!(
                    matches!(
                        descriptor.lifecycle.session,
                        ProviderSessionLifecycle::PerRequest
                            | ProviderSessionLifecycle::AwsStreamingSdk
                    ),
                    "{} AWS content egress must declare request or streaming SDK lifecycle",
                    descriptor.id
                ),
                ProviderTransport::GrpcBidi => assert_eq!(
                    descriptor.lifecycle.session,
                    ProviderSessionLifecycle::GrpcBidirectionalStream,
                    "{} gRPC content egress must declare bidirectional stream lifecycle",
                    descriptor.id
                ),
                ProviderTransport::SdkNative => assert_eq!(
                    descriptor.lifecycle.session,
                    ProviderSessionLifecycle::NativeSdkConversation,
                    "{} native SDK content egress must declare native conversation lifecycle",
                    descriptor.id
                ),
                ProviderTransport::SidecarProcess => assert_eq!(
                    descriptor.lifecycle.session,
                    ProviderSessionLifecycle::SidecarProcess,
                    "{} sidecar content egress must declare sidecar lifecycle",
                    descriptor.id
                ),
            }

            if descriptor
                .privacy
                .data_classes_sent
                .contains(&ProviderDataClass::Audio)
            {
                let audio_input = descriptor.audio_input.unwrap_or_else(|| {
                    panic!(
                        "{} audio egress needs an audio input contract",
                        descriptor.id
                    )
                });
                assert_eq!(
                    audio_input.pipeline_format, PIPELINE_F32_16K_MONO,
                    "{} audio egress must start from the canonical backend pipeline format",
                    descriptor.id
                );
                assert!(
                    descriptor.source_policy.is_some(),
                    "{} audio egress must declare source fan-out policy",
                    descriptor.id
                );
                assert!(
                    descriptor.event_semantics.is_some(),
                    "{} audio egress must declare event/session semantics",
                    descriptor.id
                );
            }
        }
    }

    #[test]
    fn future_content_egress_candidates_wait_for_blocked_policy_harnesses() {
        for id in [
            "asr.gladia",
            "asr.speechmatics",
            "asr.elevenlabs_scribe",
            "asr.revai",
            "realtime_agent.openai_realtime",
        ] {
            let descriptor = descriptor_by_id(id);
            assert!(
                descriptor.privacy.data_leaves_device,
                "{id} should remain classified as off-device content egress"
            );
            assert!(
                provider_sends_runtime_content(descriptor),
                "{id} should keep explicit runtime content data classes"
            );
            assert!(
                matches!(
                    descriptor.status,
                    ProviderStatus::Planned
                        | ProviderStatus::Watch
                        | ProviderStatus::EnterpriseWatch
                ),
                "{id} must not be marked Implemented until the runtime crate has a blocked-policy harness"
            );
        }
    }

    #[test]
    fn audio_capable_providers_use_canonical_pipeline_format() {
        for descriptor in provider_registry() {
            let Some(audio_input) = descriptor.audio_input else {
                continue;
            };

            assert_eq!(
                audio_input.pipeline_format, PIPELINE_F32_16K_MONO,
                "{} must consume the backend processed-audio bus contract before adapter conversion",
                descriptor.id
            );
            assert_eq!(audio_input.pipeline_format.sample_rate_hz, 16_000);
            assert_eq!(audio_input.pipeline_format.channels, 1);
            assert_eq!(
                audio_input.pipeline_format.frame_format,
                ProviderAudioFrameFormat::F32
            );
            assert_eq!(
                audio_input.attribution.max_channels, 1,
                "{} current adapter must not claim more than mono channel capacity",
                descriptor.id
            );
            assert!(
                !audio_input.attribution.requires_source_native_channels,
                "{} current adapter must not require source-native channels until the source-channel contract lands",
                descriptor.id
            );
            assert!(
                !matches!(
                    audio_input.attribution.mode,
                    ProviderAttributionMode::Channel
                        | ProviderAttributionMode::SpeakerAndChannel
                        | ProviderAttributionMode::ExperimentalSourceSeparation
                ),
                "{} current mono adapter must not claim provider/source channel attribution",
                descriptor.id
            );
            assert_eq!(
                audio_input.attribution.channel_label_semantics,
                ProviderChannelLabelSemantics::None,
                "{} current adapter must not expose channel labels without source-native channel proof",
                descriptor.id
            );
            assert_eq!(
                audio_input.attribution.accepted_layouts, MONO_CHANNEL_LAYOUTS,
                "{} current adapter must accept only the mono pipeline layout",
                descriptor.id
            );

            let provider_changes_rate = audio_input.provider_format.sample_rate_hz
                != audio_input.pipeline_format.sample_rate_hz;
            assert_eq!(
                audio_input.adapter_resamples, provider_changes_rate,
                "{} adapter_resamples must describe whether provider_format changes sample rate",
                descriptor.id
            );
        }
    }

    #[test]
    fn source_native_channel_claims_require_multichannel_proof_and_provenance() {
        for descriptor in provider_registry() {
            let Some(audio_input) = descriptor.audio_input else {
                continue;
            };
            let attribution = audio_input.attribution;
            let claims_channel_attribution = attribution.requires_source_native_channels
                || attribution.max_channels > 1
                || matches!(
                    attribution.mode,
                    ProviderAttributionMode::Channel
                        | ProviderAttributionMode::SpeakerAndChannel
                        | ProviderAttributionMode::ExperimentalSourceSeparation
                );

            if attribution.requires_source_native_channels {
                assert!(
                    audio_input.supports_multichannel,
                    "{} cannot require source-native channels without adapter multichannel support",
                    descriptor.id
                );
                assert!(
                    audio_input.provider_format.channels > 1,
                    "{} cannot require source-native channels while provider format is mono",
                    descriptor.id
                );
                assert_ne!(
                    descriptor.source_policy,
                    Some(ProviderSourcePolicy::MultiSourceMixed),
                    "{} cannot preserve source-native channels after mixed-source routing",
                    descriptor.id
                );
            }

            if claims_channel_attribution {
                let source_url = attribution.capability_source_url.unwrap_or_else(|| {
                    panic!("{} channel claim needs provenance URL", descriptor.id)
                });
                let source_date = attribution.capability_source_date.unwrap_or_else(|| {
                    panic!("{} channel claim needs provenance date", descriptor.id)
                });
                assert!(!source_url.trim().is_empty(), "{}", descriptor.id);
                assert_eq!(source_date.len(), 10, "{}", descriptor.id);
            }
        }
    }

    #[test]
    fn provider_privacy_metadata_is_unknown_aware_and_stage_specific() {
        for descriptor in provider_registry() {
            let privacy = descriptor.privacy;
            assert_eq!(
                privacy.cloud_transfer_acknowledgement_required, privacy.data_leaves_device,
                "{} cloud acknowledgement must follow device-egress truth",
                descriptor.id
            );

            if privacy.data_leaves_device {
                assert!(
                    !privacy.data_classes_sent.is_empty(),
                    "{} cloud provider must declare outbound data classes",
                    descriptor.id
                );
                let has_provider_probe = descriptor.health_check_command.is_some()
                    || descriptor.model_catalog_command.is_some();
                if has_provider_probe {
                    assert_eq!(
                        privacy.health_check_data_classes, AUTH_HEALTH_DATA_CLASSES,
                        "{} health/model probes must be auth/config only until proven otherwise",
                        descriptor.id
                    );
                } else {
                    assert!(
                        privacy.health_check_data_classes.is_empty(),
                        "{} cannot declare health-check egress without a health/model command",
                        descriptor.id
                    );
                }
                assert_eq!(
                    privacy.sensitive_error_policy,
                    ProviderSensitiveErrorPolicy::AudioGraphRedacted,
                    "{} cloud diagnostics must stay behind AudioGraph redaction",
                    descriptor.id
                );
                assert_ne!(
                    privacy.enterprise_no_training_config,
                    ProviderPolicyStatus::EnterpriseOnly,
                    "{} cannot imply enterprise no-training support without sourced policy metadata",
                    descriptor.id
                );

                let claims_linked_policy = matches!(
                    privacy.retention_policy,
                    ProviderPolicyStatus::ProviderDocsLinked
                ) || matches!(
                    privacy.training_policy,
                    ProviderPolicyStatus::ProviderDocsLinked
                ) || matches!(
                    privacy.deletion_policy,
                    ProviderPolicyStatus::ProviderDocsLinked
                );
                assert_eq!(
                    claims_linked_policy,
                    privacy.policy_url.is_some(),
                    "{} cannot imply provider policy proof without a policy URL",
                    descriptor.id
                );

                // HONESTY INVARIANT: a policy URL must carry a verification
                // date and vice versa — no undated link, no dangling date.
                assert_eq!(
                    privacy.policy_url.is_some(),
                    privacy.policy_url_source_date.is_some(),
                    "{} policy_url and policy_url_source_date must be set together",
                    descriptor.id
                );

                // Any sourced URL on a cloud provider must be an official https
                // link (no fabricated/placeholder values).
                for url in privacy
                    .policy_url
                    .into_iter()
                    .chain(privacy.subprocessors_url)
                {
                    assert!(
                        url.starts_with("https://"),
                        "{} privacy URL must be an official https link, got {url}",
                        descriptor.id
                    );
                }

                // A provider with NO sourced policy URL must keep every policy
                // field Unknown — it may not silently assert retention/training/
                // deletion behavior without a citation.
                if privacy.policy_url.is_none() {
                    assert_eq!(
                        privacy.retention_policy,
                        ProviderPolicyStatus::Unknown,
                        "{} retention claim requires a sourced policy URL",
                        descriptor.id
                    );
                    assert_eq!(
                        privacy.training_policy,
                        ProviderPolicyStatus::Unknown,
                        "{} training claim requires a sourced policy URL",
                        descriptor.id
                    );
                    assert_eq!(
                        privacy.deletion_policy,
                        ProviderPolicyStatus::Unknown,
                        "{} deletion claim requires a sourced policy URL",
                        descriptor.id
                    );
                    assert!(
                        privacy.subprocessors_url.is_none(),
                        "{} cannot list subprocessors without a sourced policy URL",
                        descriptor.id
                    );
                }
            } else {
                assert!(
                    privacy.data_classes_sent.is_empty(),
                    "{} local provider should not declare off-device sent classes",
                    descriptor.id
                );
                assert_eq!(
                    privacy.retention_policy,
                    ProviderPolicyStatus::NotApplicable
                );
                assert_eq!(privacy.training_policy, ProviderPolicyStatus::NotApplicable);
                assert_eq!(privacy.deletion_policy, ProviderPolicyStatus::NotApplicable);
                assert!(
                    privacy.policy_url.is_none()
                        && privacy.policy_url_source_date.is_none()
                        && privacy.subprocessors_url.is_none(),
                    "{} local provider must not carry remote policy/subprocessor links",
                    descriptor.id
                );
            }
        }

        let asr = descriptor_by_id("asr.deepgram");
        assert!(
            asr.privacy
                .data_classes_sent
                .contains(&ProviderDataClass::Audio)
        );
        assert!(
            asr.privacy
                .data_classes_returned
                .contains(&ProviderDataClass::TranscriptText)
        );

        let llm = descriptor_by_id("llm.openrouter");
        assert!(
            llm.privacy
                .data_classes_sent
                .contains(&ProviderDataClass::PromptText)
        );
        assert!(
            llm.privacy
                .data_classes_returned
                .contains(&ProviderDataClass::GeneratedText)
        );

        let tts = descriptor_by_id("tts.deepgram_aura");
        assert!(
            tts.privacy
                .data_classes_sent
                .contains(&ProviderDataClass::GeneratedText)
        );
        assert!(
            tts.privacy
                .data_classes_returned
                .contains(&ProviderDataClass::GeneratedAudio)
        );
    }

    #[test]
    fn sourced_provider_policies_are_dated_and_official() {
        // Providers whose official data-boundary policy was verified against
        // their own docs: each carries a dated official URL and a non-Unknown
        // claim grounded in that source.
        let openai_asr = descriptor_by_id("asr.openai_realtime").privacy;
        assert_eq!(openai_asr.policy_url, Some(OPENAI_DATA_USAGE_URL));
        assert_eq!(
            openai_asr.policy_url_source_date,
            Some(OPENAI_DATA_USAGE_SOURCE_DATE)
        );
        assert_eq!(
            openai_asr.training_policy,
            ProviderPolicyStatus::ProviderDocsLinked,
            "OpenAI publishes an official no-training-by-default API policy",
        );

        assert_eq!(
            descriptor_by_id("realtime_agent.openai_realtime")
                .privacy
                .policy_url,
            Some(OPENAI_DATA_USAGE_URL)
        );

        // Deepgram DOES train on a sample of customer audio by default (Model
        // Improvement Program, opt-out via mip_opt_out); we record the sourced
        // policy + subprocessors list, not a no-training claim.
        for id in ["asr.deepgram", "tts.deepgram_aura"] {
            let privacy = descriptor_by_id(id).privacy;
            assert_eq!(privacy.policy_url, Some(DEEPGRAM_MIP_URL), "{id}");
            assert_eq!(
                privacy.policy_url_source_date,
                Some(DEEPGRAM_SOURCE_DATE),
                "{id}"
            );
            assert_eq!(
                privacy.subprocessors_url,
                Some(DEEPGRAM_SUBPROCESSORS_URL),
                "{id}"
            );
            assert_eq!(
                privacy.training_policy,
                ProviderPolicyStatus::ProviderDocsLinked,
                "{id} Deepgram MIP training-by-default is a sourced policy",
            );
        }

        // AWS AI services may use content for model improvement unless the user
        // opts out; region is user-configured.
        for id in ["asr.aws_transcribe", "llm.aws_bedrock"] {
            let privacy = descriptor_by_id(id).privacy;
            assert_eq!(privacy.policy_url, Some(AWS_AI_OPT_OUT_URL), "{id}");
            assert_eq!(
                privacy.policy_url_source_date,
                Some(AWS_AI_SOURCE_DATE),
                "{id}"
            );
            assert_eq!(
                privacy.data_residency,
                ProviderPolicyStatus::UserConfigured,
                "{id} AWS region residency is user-selected",
            );
            assert_eq!(privacy.training_policy, ProviderPolicyStatus::ProviderDocsLinked);
        }

        // AssemblyAI: retention + deletion are sourced, but its privacy policy
        // does NOT address model training, so training stays explicitly Unknown.
        let assemblyai = descriptor_by_id("asr.assemblyai").privacy;
        assert_eq!(assemblyai.policy_url, Some(ASSEMBLYAI_PRIVACY_URL));
        assert_eq!(
            assemblyai.policy_url_source_date,
            Some(ASSEMBLYAI_SOURCE_DATE)
        );
        assert_eq!(
            assemblyai.retention_policy,
            ProviderPolicyStatus::ProviderDocsLinked
        );
        assert_eq!(
            assemblyai.deletion_policy,
            ProviderPolicyStatus::ProviderDocsLinked
        );
        assert_eq!(
            assemblyai.training_policy,
            ProviderPolicyStatus::Unknown,
            "AssemblyAI's privacy policy does not address training; do not fabricate a claim",
        );
        assert_eq!(
            assemblyai.subprocessors_url,
            Some(ASSEMBLYAI_SUBPROCESSORS_URL)
        );

        // Soniox has no verified official policy in the registry: it must stay
        // fully Unknown with no policy/subprocessor links.
        let soniox = descriptor_by_id("asr.soniox").privacy;
        assert_eq!(soniox.policy_url, None);
        assert_eq!(soniox.policy_url_source_date, None);
        assert_eq!(soniox.subprocessors_url, None);
        assert_eq!(soniox.retention_policy, ProviderPolicyStatus::Unknown);
        assert_eq!(soniox.training_policy, ProviderPolicyStatus::Unknown);
        assert_eq!(soniox.deletion_policy, ProviderPolicyStatus::Unknown);
    }

    #[test]
    fn enterprise_stt_candidates_do_not_masquerade_as_websocket_providers() {
        let google = descriptor_by_id("asr.google_chirp3");
        let google_enterprise = google
            .enterprise
            .expect("Google Chirp 3 should carry enterprise adapter metadata");

        assert_eq!(google.stage, ProviderStage::Asr);
        assert_eq!(google.status, ProviderStatus::Planned);
        assert_eq!(google.transport, ProviderTransport::GrpcBidi);
        assert!(
            google.credential_keys.is_empty(),
            "ADC/service-account auth is flexible and must not be reported as one mandatory saved key"
        );
        assert_eq!(
            google.lifecycle.session,
            ProviderSessionLifecycle::GrpcBidirectionalStream
        );
        assert_eq!(
            google.lifecycle.auth,
            ProviderAuthLifecycle::GoogleAdcOrServiceAccount
        );
        assert_eq!(
            google.audio_input.unwrap().transport_encoding,
            ProviderAudioTransportEncoding::GrpcStreaming
        );
        assert!(
            google_enterprise
                .packaging
                .contains(&ProviderPackagingRequirement::ProtobufGrpcClient)
        );
        assert!(
            google_enterprise
                .health_probes
                .contains(&ProviderHealthProbeKind::StreamingRpcAvailability)
        );
        assert_eq!(
            google_enterprise.speaker_semantics.label_support,
            ProviderSpeakerLabelSupport::StreamingUnverified
        );
        assert!(
            google_enterprise
                .speaker_semantics
                .local_timeline_recommended
        );

        let azure = descriptor_by_id("asr.azure_speech");
        let azure_enterprise = azure
            .enterprise
            .expect("Azure Speech should carry enterprise adapter metadata");

        assert_eq!(azure.stage, ProviderStage::Asr);
        assert_eq!(azure.status, ProviderStatus::Planned);
        assert_eq!(azure.transport, ProviderTransport::SdkNative);
        assert_eq!(azure.model_catalog, ModelCatalogPolicy::None);
        assert_eq!(azure.default_model, None);
        assert!(
            azure.credential_keys.is_empty(),
            "Azure key/Entra auth is flexible and must not be reported as one mandatory saved key"
        );
        assert_eq!(
            azure.lifecycle.session,
            ProviderSessionLifecycle::NativeSdkConversation
        );
        assert_eq!(
            azure.lifecycle.auth,
            ProviderAuthLifecycle::AzureSpeechKeyOrEntraToken
        );
        assert_eq!(
            azure.audio_input.unwrap().transport_encoding,
            ProviderAudioTransportEncoding::SdkNative
        );
        assert!(
            azure_enterprise
                .endpoint_modes
                .contains(&ProviderEndpointMode::PrivateEndpoint)
        );
        assert!(
            azure_enterprise
                .endpoint_modes
                .contains(&ProviderEndpointMode::SovereignCloud)
        );
        assert!(
            azure_enterprise
                .packaging
                .contains(&ProviderPackagingRequirement::VisualCppRedistributable)
        );
        assert!(
            azure_enterprise
                .packaging
                .contains(&ProviderPackagingRequirement::SystemLibraries)
        );
        assert!(
            azure_enterprise
                .health_probes
                .contains(&ProviderHealthProbeKind::SdkDependency)
        );
        assert_eq!(
            azure_enterprise.speaker_semantics.label_support,
            ProviderSpeakerLabelSupport::StreamingProviderLabels
        );
        assert!(
            azure_enterprise
                .speaker_semantics
                .interim_labels_may_be_unknown
        );
        assert!(
            !azure_enterprise
                .speaker_semantics
                .speaker_ids_are_stable_identity
        );
    }

    #[test]
    fn roadmap_watch_candidates_carry_source_and_unwired_auth_schema() {
        let xai = descriptor_by_id("asr.xai_grok_stt");
        let xai_roadmap = xai.roadmap.expect("xAI should carry roadmap metadata");

        assert_eq!(xai.stage, ProviderStage::Asr);
        assert_eq!(xai.status, ProviderStatus::Watch);
        assert_eq!(xai.transport, ProviderTransport::WebSocket);
        assert!(xai.credential_keys.is_empty());
        assert_eq!(xai.lifecycle.auth, ProviderAuthLifecycle::SavedApiKey);
        assert!(xai.supports_diarization);
        assert_eq!(
            xai.audio_input.expect("xAI audio input").attribution.mode,
            ProviderAttributionMode::Speaker
        );
        assert_eq!(
            xai_roadmap.source_url,
            ARTIFICIAL_ANALYSIS_STREAMING_STT_SOURCE_URL
        );
        assert_eq!(
            xai_roadmap.source_date,
            ARTIFICIAL_ANALYSIS_STREAMING_STT_SOURCE_DATE
        );
        assert_eq!(
            xai_roadmap.auth_schema,
            ProviderCredentialSchemaStatus::RequiredNotWired
        );
        assert!(xai_roadmap.not_selectable_reason.is_some());
        assert_eq!(xai.settings_groups, BASIC_MODEL_ADVANCED_GROUPS);
        assert_ne!(xai.status, ProviderStatus::Implemented);

        let nemotron = descriptor_by_id("asr.nvidia_nemotron_asr");
        let nemotron_roadmap = nemotron
            .roadmap
            .expect("Nemotron ASR should carry roadmap metadata");
        let nemotron_enterprise = nemotron
            .enterprise
            .expect("Nemotron ASR should carry enterprise metadata");

        assert_eq!(nemotron.stage, ProviderStage::Asr);
        assert_eq!(nemotron.status, ProviderStatus::EnterpriseWatch);
        assert_eq!(nemotron.transport, ProviderTransport::GrpcBidi);
        assert!(nemotron.credential_keys.is_empty());
        assert_eq!(nemotron.lifecycle, WATCHLIST_GRPC_AUTH_REQUIRED_LIFECYCLE);
        assert_eq!(
            nemotron_roadmap.auth_schema,
            ProviderCredentialSchemaStatus::RequiredNotWired
        );
        assert_eq!(
            nemotron_roadmap.source_date,
            ARTIFICIAL_ANALYSIS_STREAMING_STT_SOURCE_DATE
        );
        assert!(
            nemotron_enterprise
                .endpoint_modes
                .contains(&ProviderEndpointMode::PrivateEndpoint)
        );
        assert!(
            nemotron_enterprise
                .packaging
                .contains(&ProviderPackagingRequirement::SidecarProcess)
        );
        assert_ne!(nemotron.status, ProviderStatus::Implemented);

        let cartesia = descriptor_by_id("asr.cartesia_ink2");
        let cartesia_roadmap = cartesia
            .roadmap
            .expect("Cartesia Ink-2 should carry roadmap metadata");

        assert_eq!(cartesia.stage, ProviderStage::Asr);
        assert_eq!(cartesia.status, ProviderStatus::Watch);
        assert_eq!(cartesia.transport, ProviderTransport::WebSocket);
        assert!(cartesia.credential_keys.is_empty());
        assert_eq!(cartesia.health_check_command, None);
        assert_eq!(cartesia.model_catalog_command, None);
        assert_eq!(
            cartesia.event_semantics,
            Some(ProviderEventSemantics::TranscriptPartialFinalTurns)
        );
        assert_eq!(
            cartesia_roadmap.auth_schema,
            ProviderCredentialSchemaStatus::RequiredNotWired
        );
        assert_ne!(cartesia.status, ProviderStatus::Implemented);

        let qwen = descriptor_by_id("asr.alibaba_qwen3_asr_flash");
        let qwen_roadmap = qwen
            .roadmap
            .expect("Alibaba/Qwen3 ASR Flash should carry roadmap metadata");
        let qwen_enterprise = qwen
            .enterprise
            .expect("Alibaba/Qwen3 ASR Flash should carry enterprise metadata");

        assert_eq!(qwen.stage, ProviderStage::Asr);
        assert_eq!(qwen.status, ProviderStatus::EnterpriseWatch);
        assert_eq!(qwen.transport, ProviderTransport::WebSocket);
        assert!(qwen.credential_keys.is_empty());
        assert_eq!(qwen.health_check_command, None);
        assert_eq!(qwen.model_catalog_command, None);
        assert_eq!(qwen.privacy, USER_REGION_ASR_NO_HEALTH_PRIVACY);
        assert_eq!(qwen.audio_input, Some(WS_JSON_BASE64_PCM16_16K_AUDIO_INPUT));
        assert_eq!(
            qwen_roadmap.auth_schema,
            ProviderCredentialSchemaStatus::RequiredNotWired
        );
        assert!(
            qwen_enterprise
                .endpoint_modes
                .contains(&ProviderEndpointMode::DefaultRegion)
        );
        assert!(
            qwen_enterprise
                .endpoint_modes
                .contains(&ProviderEndpointMode::CustomEndpoint)
        );
        assert_eq!(
            qwen_enterprise.speaker_semantics.label_support,
            ProviderSpeakerLabelSupport::None
        );
        assert_ne!(qwen.status, ProviderStatus::Implemented);
    }

    #[test]
    fn deepgram_aura_declares_fixed_voice_catalog() {
        let descriptor = descriptor_by_id("tts.deepgram_aura");
        let catalog = descriptor
            .fixed_model_catalog
            .expect("Deepgram Aura must expose fixed voice catalog metadata");

        assert_eq!(descriptor.model_catalog, ModelCatalogPolicy::Fixed);
        assert_eq!(descriptor.default_model, Some("aura-asteria-en"));
        assert_eq!(catalog.len(), 12);
        assert_eq!(catalog[0].id, "aura-asteria-en");
        assert!(catalog[0].is_default);
        assert!(catalog.iter().any(|item| item.id == "aura-zeus-en"));
    }

    #[test]
    fn deepgram_declares_remote_model_catalog_command() {
        let descriptor = descriptor_by_id("asr.deepgram");

        assert_eq!(descriptor.model_catalog, ModelCatalogPolicy::RemoteCommand);
        assert_eq!(
            descriptor.model_catalog_command,
            Some("list_deepgram_models_cmd")
        );
        assert_eq!(
            descriptor.health_check_command,
            Some("test_deepgram_connection")
        );
        assert_eq!(descriptor.default_model, Some("nova-3"));
    }

    #[test]
    fn soniox_declares_planned_remote_model_catalog_command() {
        let descriptor = descriptor_by_id("asr.soniox");

        assert_eq!(descriptor.status, ProviderStatus::Planned);
        assert_eq!(descriptor.model_catalog, ModelCatalogPolicy::RemoteCommand);
        assert_eq!(
            descriptor.model_catalog_command,
            Some("list_soniox_models_cmd")
        );
        assert_eq!(
            descriptor.health_check_command,
            Some("test_soniox_connection")
        );
        assert_eq!(descriptor.default_model, Some("stt-rt-v5"));
    }

    #[test]
    fn cerebras_declares_remote_model_catalog_command() {
        let descriptor = descriptor_by_id("llm.cerebras");

        assert_eq!(descriptor.status, ProviderStatus::Implemented);
        assert_eq!(descriptor.settings_variant, "cerebras");
        assert_eq!(descriptor.credential_keys, CEREBRAS_CREDENTIAL_KEYS);
        assert_eq!(descriptor.model_catalog, ModelCatalogPolicy::RemoteCommand);
        assert_eq!(
            descriptor.model_catalog_command,
            Some("list_cerebras_models_cmd")
        );
        assert_eq!(
            descriptor.health_check_command,
            Some("test_cerebras_connection_cmd")
        );
        assert_eq!(descriptor.default_model, Some(CEREBRAS_DEFAULT_MODEL));
        assert!(
            descriptor
                .fixed_model_catalog
                .unwrap_or_default()
                .iter()
                .any(|model| model.id == CEREBRAS_PREVIEW_MODEL)
        );
    }

    #[test]
    fn local_providers_declare_required_cargo_features() {
        let expectations = [
            ("asr.local_whisper", &["local-ml", "asr-whisper"] as &[&str]),
            ("asr.sherpa_onnx", &["sherpa-streaming"] as &[&str]),
            ("asr.moonshine", &["asr-moonshine"] as &[&str]),
            ("llm.local_llama", &["local-ml", "llm-llama"] as &[&str]),
            ("llm.mistralrs", &["local-ml", "llm-mistralrs"] as &[&str]),
        ];

        for (id, expected) in expectations {
            assert_eq!(
                descriptor_by_id(id).required_features,
                expected,
                "{id} feature metadata must stay aligned with Cargo.toml"
            );
        }

        assert!(
            provider_registry()
                .iter()
                .filter(|descriptor| descriptor.transport == ProviderTransport::Local)
                .all(|descriptor| {
                    descriptor.id == "tts.none" || !descriptor.required_features.is_empty()
                }),
            "local runtime providers should declare their enabling feature unless they are always available"
        );
    }

    #[test]
    fn moonshine_declares_native_local_runtime_contract() {
        let descriptor = descriptor_by_id("asr.moonshine");

        assert_eq!(descriptor.status, ProviderStatus::Planned);
        assert_eq!(descriptor.transport, ProviderTransport::Local);
        assert_eq!(descriptor.required_features, &["asr-moonshine"]);
        assert_eq!(descriptor.model_catalog, ModelCatalogPolicy::LocalFiles);
        assert_eq!(descriptor.default_model, Some(MOONSHINE_SMALL_STREAMING_EN));
        assert_eq!(
            descriptor.event_semantics,
            Some(ProviderEventSemantics::TranscriptPartialFinalTurns)
        );
        assert_eq!(
            descriptor.audio_input,
            Some(LOCAL_F32_PROVIDER_SPEAKER_LABEL_AUDIO_INPUT)
        );
        assert_eq!(descriptor.lifecycle, LOCAL_STREAMING_LIFECYCLE);
        assert_eq!(descriptor.privacy, LOCAL_PRIVACY);
        assert!(descriptor.supports_streaming);
        assert!(descriptor.supports_partial_revisions);
        assert!(descriptor.supports_diarization);
        assert_eq!(descriptor.local_models.len(), 3);
        assert!(
            descriptor
                .local_models
                .iter()
                .all(|model| model.required_files == MOONSHINE_STREAMING_REQUIRED_FILES)
        );
        assert!(
            descriptor
                .local_models
                .iter()
                .any(|model| model.model_id == MOONSHINE_MEDIUM_STREAMING_EN)
        );
    }

    #[test]
    fn diarization_runtimes_declare_local_model_dependencies() {
        let sortformer = descriptor_by_id("diarization.sortformer");

        assert_eq!(sortformer.stage, ProviderStage::Diarization);
        assert_eq!(sortformer.status, ProviderStatus::Planned);
        assert_eq!(sortformer.transport, ProviderTransport::Local);
        assert_eq!(sortformer.required_features, &["diarization"]);
        assert_eq!(sortformer.model_catalog, ModelCatalogPolicy::LocalFiles);
        assert_eq!(sortformer.lifecycle, LOCAL_STREAMING_LIFECYCLE);
        assert_eq!(sortformer.privacy, LOCAL_PRIVACY);
        assert_eq!(
            sortformer.audio_input,
            Some(DIARIZATION_LOCAL_TIMELINE_AUDIO_INPUT)
        );
        assert_eq!(sortformer.default_model, Some(SORTFORMER_MODEL_FILENAME));
        assert_eq!(sortformer.local_models.len(), 1);
        assert_eq!(sortformer.local_models[0].kind, LocalModelKind::File);
        assert_eq!(
            sortformer.local_models[0].model_id,
            SORTFORMER_MODEL_FILENAME
        );
        assert_eq!(sortformer.local_models[0].required_files, SORTFORMER_FILES);
        assert!(sortformer.supports_streaming);
        assert!(sortformer.supports_partial_revisions);
        assert!(sortformer.supports_diarization);

        let clustering = descriptor_by_id("diarization.clustering");
        assert_eq!(clustering.stage, ProviderStage::Diarization);
        assert_eq!(clustering.status, ProviderStatus::Planned);
        assert_eq!(clustering.transport, ProviderTransport::Local);
        assert_eq!(clustering.required_features, &["diarization-clustering"]);
        assert_eq!(clustering.model_catalog, ModelCatalogPolicy::LocalFiles);
        assert_eq!(clustering.lifecycle, LOCAL_STREAMING_LIFECYCLE);
        assert_eq!(clustering.privacy, LOCAL_PRIVACY);
        assert_eq!(
            clustering.audio_input,
            Some(DIARIZATION_LOCAL_TIMELINE_AUDIO_INPUT)
        );
        assert_eq!(clustering.local_models.len(), 2);
        assert_eq!(clustering.local_models[0].kind, LocalModelKind::Directory);
        assert_eq!(clustering.local_models[0].model_id, DIAR_SEG_PYANNOTE_DIR);
        assert_eq!(
            clustering.local_models[0].required_files,
            DIAR_SEG_PYANNOTE_REQUIRED_FILES
        );
        assert_eq!(clustering.local_models[1].kind, LocalModelKind::File);
        assert_eq!(
            clustering.local_models[1].model_id,
            DIAR_EMB_TITANET_FILENAME
        );
        assert_eq!(clustering.local_models[1].required_files, TITANET_FILES);
        assert!(clustering.supports_streaming);
        assert!(clustering.supports_partial_revisions);
        assert!(clustering.supports_diarization);
    }
}
