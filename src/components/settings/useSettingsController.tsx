// AUTO-EXTRACTED in Phase 1 (audio-graph-settings-refactor): the orchestration
// hook hoisted out of SettingsPage's body. Behavior-preserving — every binding
// that the shell render referenced is returned verbatim. See blueprint §5.
import { invoke } from "@tauri-apps/api/core";
import {
  type KeyboardEvent as ReactKeyboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { useFocusTrap } from "../../hooks/useFocusTrap";
import { useAudioGraphStore } from "../../store";
import type {
  AsrProvider,
  CredentialPresence,
  DiarizationMode,
  DiarizationSpeakerCount,
  GeminiAuthMode,
  GeminiSettings as GeminiSettingsType,
  LlmApiConfig,
  LlmProvider,
  OpenRouterModelEndpoints,
  OpenRouterProvider,
  ProviderDescriptor,
  ProviderModelCatalogItem,
  ProviderReadiness,
  ProviderStage,
} from "../../types";
import { errorToMessage } from "../../utils/errorToMessage";
import type { AcceleratorPreset } from "../../utils/openrouterCatalog";
import Icon from "../Icon";
import type { CredentialPresenceLookup } from "../ProviderReadinessPanel";
import {
  defaultModelForProvider,
  implementedProviderOptionsForStage,
  modelCatalogForProvider,
  PROVIDER_DESCRIPTORS,
  providerIdForSettingsVariant,
  providerStatusLabel,
} from "../providerRegistryHelpers";
import {
  deriveProviderSetupModeCards,
  type ProviderSetupBlocker,
  type ProviderSetupBlockerKind,
  type ProviderSetupDataBoundary,
  type ProviderSetupModeCard,
  type ProviderSetupProviderSelection,
  type ProviderSetupReadinessStatus,
  type ProviderSetupStageCoverage,
  type ProviderSetupStageRole,
  providerSetupSourceRecoveryIssues,
} from "../providerSetupModes";
import {
  type AsrType,
  buildAwsCredentialSource,
  CEREBRAS_BASE_URL,
  type ChannelCount,
  endpointCredentialKey,
  initialSettingsState,
  type LlmType,
  type LogLevel,
  type SampleRate,
  type SettingsState,
  setField,
  settingsReducer,
  type TestKey,
} from "../settingsTypes";
import {
  buildOpenRouterRoutingPolicy,
  DEFAULT_OPENROUTER_BASE_URL,
  inferOpenRouterRoutingPreset,
  normalizeOpenRouterBaseUrl,
  openRouterModelsCacheKey,
  openRouterProviderOrderTextForSettings,
  parseOpenRouterProviderList,
} from "./settingsControllerHelpers";
import {
  RAIL_GROUP_LABEL_KEYS,
  RAIL_GROUP_ORDER,
  RAIL_SECTIONS,
  type SettingsTab,
} from "./settingsRailConfig";

// Theme choices surfaced in the General tab segmented control. Order mirrors
// the escalation from "let the OS decide" → explicit light → explicit dark.
export const THEME_OPTIONS = ["system", "light", "dark"] as const;
const ASR_PROVIDER_SETTINGS_VARIANTS = [
  "local_whisper",
  "api",
  "openai_realtime",
  "aws_transcribe",
  "deepgram",
  "assemblyai",
  "sherpa_onnx",
  "moonshine",
] as const;
const LLM_PROVIDER_SETTINGS_VARIANTS = [
  "local_llama",
  "api",
  "cerebras",
  "openrouter",
  "aws_bedrock",
  "mistralrs",
] as const;
const TTS_PROVIDER_SETTINGS_VARIANTS = ["none", "deepgram_aura"] as const;
export type TtsType = (typeof TTS_PROVIDER_SETTINGS_VARIANTS)[number];
export const ASR_PROVIDER_OPTIONS = implementedProviderOptionsForStage(
  "asr",
  ASR_PROVIDER_SETTINGS_VARIANTS,
);
export const LLM_PROVIDER_OPTIONS = implementedProviderOptionsForStage(
  "llm",
  LLM_PROVIDER_SETTINGS_VARIANTS,
);
export const TTS_PROVIDER_OPTIONS = implementedProviderOptionsForStage(
  "tts",
  TTS_PROVIDER_SETTINGS_VARIANTS,
);
export const DEFAULT_AURA_VOICE = defaultModelForProvider("tts.deepgram_aura");
const DIARIZATION_MODES: DiarizationMode[] = [
  "off",
  "provider",
  "local",
  "hybrid",
];
const DIARIZATION_SPEAKER_COUNTS: DiarizationSpeakerCount[] = [
  "auto",
  "unbounded",
  "fixed",
];

// Languages the app ships translations for. Kept in sync with the
// `supportedLngs` list in `src/i18n/index.ts`. Each maps to a
// `language.<code>` display label in the locale files.
export const LANGUAGE_OPTIONS = ["en", "pt"] as const;
export const PROVIDER_READINESS_LABELS = new Map(
  [...PROVIDER_DESCRIPTORS.values()].map((provider) => [
    provider.id,
    provider.display_name,
  ]),
);
export const PROVIDER_CAPABILITY_STAGES = [
  {
    stage: "asr",
    label: "ASR",
    description: "Speech-to-text capture and transcript providers.",
  },
  {
    stage: "llm",
    label: "LLM",
    description: "Language model providers for chat, notes, and graph work.",
  },
  {
    stage: "tts",
    label: "TTS",
    description: "Speech output providers for spoken responses.",
  },
  {
    stage: "realtime_agent",
    label: "Realtime",
    description: "Native speech-to-speech agents that bypass the staged path.",
  },
] satisfies ReadonlyArray<{
  stage: ProviderStage;
  label: string;
  description: string;
}>;

type CloudCredentialKey =
  | "openai_api_key"
  | "cerebras_api_key"
  | "openrouter_api_key"
  | "groq_api_key"
  | "together_api_key"
  | "fireworks_api_key"
  | "gemini_api_key"
  | "deepgram_api_key"
  | "assemblyai_api_key"
  | "soniox_api_key";
type WritableCredentialKey =
  | CloudCredentialKey
  | "google_service_account_path"
  | "aws_access_key"
  | "aws_secret_key"
  | "aws_session_token";
type CredentialPresenceMap = CredentialPresenceLookup;
type SettingsControlRoute = {
  tab: SettingsTab;
  fieldId: string;
  activate?: boolean;
  apply?: () => void;
};
type CredentialRoute = SettingsControlRoute;

const PROVIDER_SETUP_STAGE_LABELS: Record<ProviderSetupStageRole, string> = {
  durable_transcription: "Speech-to-text",
  durable_notes_graph: "Notes and graph",
  speech_output: "Speech output",
  native_realtime_agent: "Realtime agent",
};

export function providerSetupStatusLabel(
  status: ProviderSetupReadinessStatus,
): string {
  switch (status) {
    case "ready":
      return "Ready";
    case "missing_credentials":
      return "Missing key";
    case "blocked":
      return "Blocked";
    case "error":
      return "Error";
    case "unchecked":
      return "Unchecked";
  }
}

export function providerSetupDataBoundaryLabel(
  boundary: ProviderSetupDataBoundary,
): string {
  switch (boundary) {
    case "local_only":
      return "Local only";
    case "user_configured_endpoint":
      return "Configured endpoint";
    case "user_configured_region":
      return "Configured region";
    case "provider_account_boundary":
      return "Provider account";
    case "vendor_cloud":
      return "Vendor cloud";
    case "mixed_local_cloud":
      return "Mixed local and cloud";
    case "mixed_cloud":
      return "Cloud providers";
    case "not_applicable":
      return "Not applicable";
  }
}

export function providerSetupBlockerKindLabel(
  kind: ProviderSetupBlockerKind,
): string {
  switch (kind) {
    case "missing_credential":
      return "Credential";
    case "missing_config":
      return "Setup";
    case "model_unselected":
    case "missing_model":
      return "Model";
    case "provider_planned":
      return "Provider";
    case "provider_error":
      return "Provider health";
    case "missing_feature":
      return "Feature";
    case "runtime_unavailable":
    case "load_failed":
      return "Runtime";
    case "source_unselected":
    case "source_unavailable":
    case "source_permission_unavailable":
    case "source_unsupported":
    case "source_policy_conflict":
      return "Source";
  }
}

function providerSetupBlockerIsSource(blocker: ProviderSetupBlocker): boolean {
  return (
    blocker.kind === "source_unselected" ||
    blocker.kind === "source_unavailable" ||
    blocker.kind === "source_permission_unavailable" ||
    blocker.kind === "source_unsupported" ||
    blocker.kind === "source_policy_conflict"
  );
}

export function providerSetupCardHasSourceBlocker(
  card: ProviderSetupModeCard,
): boolean {
  return card.missingBlockers.some(providerSetupBlockerIsSource);
}

export function providerSetupStageLabel(
  coverage: ProviderSetupStageCoverage,
): string {
  return PROVIDER_SETUP_STAGE_LABELS[coverage.role];
}

async function saveCredentialIfPresent(
  key: WritableCredentialKey,
  value: string,
): Promise<void> {
  if (!value.trim()) return;
  await invoke("save_credential_cmd", { key, value });
}

function credentialPresenceFromEntries(
  entries: CredentialPresence[],
): CredentialPresenceMap {
  return entries.reduce<CredentialPresenceMap>((acc, entry) => {
    acc[entry.key] = entry;
    return acc;
  }, {});
}

function credentialIsPresent(
  presence: CredentialPresenceMap,
  key: string,
): boolean {
  return presence[key]?.present === true;
}

function providerReadinessFromEntries(
  entries: ProviderReadiness[],
): Record<string, ProviderReadiness> {
  return entries.reduce<Record<string, ProviderReadiness>>((acc, entry) => {
    acc[entry.provider_id] = entry;
    return acc;
  }, {});
}

type ProviderReadinessStatusWithBlocked =
  | ProviderReadiness["status"]
  | "blocked";

function providerReadinessStatusSummaryLabel(
  status: ProviderReadinessStatusWithBlocked,
  t: (key: string) => string,
): string {
  const translated = t(`settings.providerReadiness.status.${status}`);
  if (translated !== `settings.providerReadiness.status.${status}`) {
    return translated;
  }
  return status === "blocked" ? "Blocked" : status.replace(/_/g, " ");
}

function hasProviderCredentials(entry: ProviderReadiness): boolean {
  return entry.credentials.some((credential) => credential.present);
}

function nonEmptyProviderSelection(
  value: string | null | undefined,
): string | null {
  const trimmed = value?.trim() ?? "";
  return trimmed.length > 0 ? trimmed : null;
}

export function formatCredentialCheckedAt(
  value: number | null | undefined,
): string | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
    return null;
  }
  return new Date(value).toLocaleString();
}

function shouldShowProviderReadiness(
  entry: ProviderReadiness,
  activeProviderIds: Set<string>,
): boolean {
  return (
    activeProviderIds.has(entry.provider_id) ||
    entry.status !== "unchecked" ||
    hasProviderCredentials(entry) ||
    entry.runtime != null ||
    entry.checked_at != null
  );
}

function sortProviderReadinessForDashboard(
  entries: ProviderReadiness[],
  activeProviderIds: string[],
): ProviderReadiness[] {
  const activeOrder = new Map<string, number>();
  for (const providerId of activeProviderIds) {
    if (!activeOrder.has(providerId)) {
      activeOrder.set(providerId, activeOrder.size);
    }
  }

  return [...entries].sort((a, b) => {
    const aOrder = activeOrder.get(a.provider_id);
    const bOrder = activeOrder.get(b.provider_id);

    if (aOrder != null && bOrder != null) return aOrder - bOrder;
    if (aOrder != null) return -1;
    if (bOrder != null) return 1;

    return a.provider_id.localeCompare(b.provider_id);
  });
}

export function providerCapabilityDescriptorsForStage(
  stage: ProviderStage,
): ProviderDescriptor[] {
  return [...PROVIDER_DESCRIPTORS.values()].filter(
    (provider) => provider.stage === stage,
  );
}

export function providerCapabilityBooleanLabel(value: boolean): string {
  return value ? "Yes" : "No";
}

function formatProviderCapabilityList(values: string[]): string {
  if (values.length === 0) return "Not declared";
  return values.join(", ");
}

function providerAudioFrameFormatLabel(
  format: NonNullable<
    ProviderDescriptor["audio_input"]
  >["provider_format"]["frame_format"],
): string {
  switch (format) {
    case "f32":
      return "f32";
    case "pcm_s16_le":
      return "PCM s16 LE";
    case "wav_pcm_s16_le":
      return "WAV PCM s16 LE";
  }
}

function providerAudioRateLabel(sampleRateHz: number): string {
  return sampleRateHz % 1000 === 0
    ? `${sampleRateHz / 1000} kHz`
    : `${sampleRateHz} Hz`;
}

function providerAudioChannelLabel(channels: number): string {
  if (channels === 1) return "mono";
  if (channels === 2) return "stereo";
  return `${channels} channels`;
}

export function providerAudioFormatLabel(
  format:
    | NonNullable<ProviderDescriptor["audio_input"]>["provider_format"]
    | undefined,
): string {
  if (!format) return "Not declared";
  return `${providerAudioRateLabel(
    format.sample_rate_hz,
  )} ${providerAudioChannelLabel(format.channels)} ${providerAudioFrameFormatLabel(
    format.frame_format,
  )}`;
}

export function providerAudioTransportEncodingLabel(
  encoding:
    | NonNullable<ProviderDescriptor["audio_input"]>["transport_encoding"]
    | undefined,
): string {
  switch (encoding) {
    case "local_buffer":
      return "Local buffer";
    case "web_socket_binary":
      return "WebSocket binary";
    case "web_socket_json_base64":
      return "WebSocket JSON base64";
    case "aws_event_stream":
      return "AWS event stream";
    case "grpc_streaming":
      return "gRPC streaming";
    case "sdk_native":
      return "Native SDK";
    case "multipart_wav":
      return "Multipart WAV";
    default:
      return "Not declared";
  }
}

export function providerEventSemanticsLabel(
  semantics: ProviderDescriptor["event_semantics"],
): string {
  switch (semantics) {
    case "transcript_final_only":
      return "Transcript final only";
    case "transcript_partial_final":
      return "Transcript partial + final";
    case "transcript_partial_final_turns":
      return "Transcript partial/final/turn events";
    case "native_realtime_audio_text":
      return "Native realtime audio/text";
    default:
      return "Not declared";
  }
}

export function providerSourcePolicyLabel(
  policy: ProviderDescriptor["source_policy"],
): string {
  switch (policy) {
    case "multi_source_independent":
      return "Independent per source";
    case "multi_source_mixed":
      return "Mixed selected sources";
    case "single_session":
      return "Single active source";
    default:
      return "Not applicable";
  }
}

export function providerTransportLabel(
  transport: ProviderDescriptor["transport"],
): string {
  switch (transport) {
    case "local":
      return "Local runtime";
    case "http":
      return "HTTP";
    case "web_socket":
      return "WebSocket";
    case "rest_init_web_socket":
      return "REST init + WebSocket";
    case "aws_sdk":
      return "AWS SDK";
    case "grpc_bidi":
      return "gRPC bidirectional";
    case "sdk_native":
      return "Native SDK";
    case "sidecar_process":
      return "Sidecar process";
  }
}

export function providerAuthLifecycleLabel(
  auth: ProviderDescriptor["lifecycle"]["auth"],
): string {
  switch (auth) {
    case "none":
      return "No auth";
    case "saved_api_key":
      return "Saved key";
    case "openai_compatible_api_key":
      return "OpenAI-compatible key";
    case "aws_credential_chain":
      return "AWS credentials";
    case "google_api_key_or_service_account":
      return "Google auth";
    case "google_adc_or_service_account":
      return "Google ADC/service account";
    case "azure_speech_key_or_entra_token":
      return "Azure key or Entra token";
  }
}

export function providerKeepaliveLabel(
  keepalive: ProviderDescriptor["lifecycle"]["keepalive"],
): string {
  switch (keepalive) {
    case "none":
      return "None";
    case "client_audio_stream":
      return "Audio stream";
    case "client_control_message":
      return "Control message";
    case "provider_specific":
      return "Provider-specific";
  }
}

export function providerSessionLifecycleLabel(
  session: ProviderDescriptor["lifecycle"]["session"],
): string {
  switch (session) {
    case "noop":
      return "No session";
    case "per_request":
      return "Per request";
    case "local_in_process":
      return "Local in-process";
    case "local_streaming_runtime":
      return "Local streaming runtime";
    case "long_lived_web_socket":
      return "WebSocket";
    case "aws_streaming_sdk":
      return "AWS streaming SDK";
    case "grpc_bidirectional_stream":
      return "gRPC bidirectional stream";
    case "native_sdk_conversation":
      return "Native SDK conversation";
    case "sidecar_process":
      return "Sidecar process";
  }
}

export function providerCloseLifecycleLabel(
  close: ProviderDescriptor["lifecycle"]["close"],
): string {
  switch (close) {
    case "noop":
      return "No close";
    case "request_completes":
      return "Request completes";
    case "drop_runtime":
      return "Drop runtime";
    case "web_socket_close_frame":
      return "WebSocket close frame";
    case "end_stream_then_close_frame":
      return "End stream then close";
    case "terminate_message_then_close_frame":
      return "Terminate then close";
    case "provider_close_message_then_close_frame":
      return "Provider close message";
    case "aws_end_stream":
      return "AWS end stream";
    case "provider_specific":
      return "Provider-specific close";
  }
}

export function providerDataBoundaryMetadataLabel(
  boundary: ProviderDescriptor["privacy"]["data_boundary"],
): string {
  switch (boundary) {
    case "local_only":
      return "Local only";
    case "user_configured_endpoint":
      return "Configured endpoint";
    case "user_configured_region":
      return "Configured region";
    case "provider_account_boundary":
      return "Provider account";
    case "vendor_cloud":
      return "Vendor cloud";
  }
}

export function providerModelCatalogPolicyLabel(
  policy: ProviderDescriptor["model_catalog"],
): string {
  switch (policy) {
    case "none":
      return "No catalog";
    case "fixed":
      return "Fixed catalog";
    case "local_files":
      return "Local files";
    case "remote_command":
      return "Remote catalog";
    case "user_supplied":
      return "User supplied";
  }
}

function providerEndpointModeLabel(
  mode: NonNullable<ProviderDescriptor["enterprise"]>["endpoint_modes"][number],
): string {
  switch (mode) {
    case "default_region":
      return "Default region";
    case "custom_endpoint":
      return "Custom endpoint";
    case "private_endpoint":
      return "Private endpoint";
    case "sovereign_cloud":
      return "Sovereign cloud";
  }
}

function providerPackagingRequirementLabel(
  requirement: NonNullable<
    ProviderDescriptor["enterprise"]
  >["packaging"][number],
): string {
  switch (requirement) {
    case "protobuf_grpc_client":
      return "Protobuf/gRPC client";
    case "native_sdk_assets":
      return "Native SDK assets";
    case "native_framework_assets":
      return "Native framework assets";
    case "system_libraries":
      return "System libraries";
    case "system_certificates":
      return "System certificates";
    case "visual_cpp_redistributable":
      return "Visual C++ redistributable";
    case "sidecar_process":
      return "Sidecar process";
  }
}

function providerSpeakerLabelSupportLabel(
  support: NonNullable<
    ProviderDescriptor["enterprise"]
  >["speaker_semantics"]["label_support"],
): string {
  switch (support) {
    case "none":
      return "No speaker labels";
    case "batch_only":
      return "Batch labels only";
    case "streaming_provider_labels":
      return "Streaming provider labels";
    case "streaming_unverified":
      return "Streaming labels unverified";
  }
}

export function providerSpeakerSemanticsLabel(
  enterprise: ProviderDescriptor["enterprise"],
): string {
  if (!enterprise) return "Not declared";

  const flags = [
    enterprise.speaker_semantics.interim_labels_may_be_unknown
      ? "interim may be unknown"
      : null,
    enterprise.speaker_semantics.speaker_ids_are_stable_identity
      ? "stable speaker IDs"
      : "speaker IDs are not stable identities",
    enterprise.speaker_semantics.local_timeline_recommended
      ? "local timeline recommended"
      : null,
  ].filter((value): value is string => value != null);

  return formatProviderCapabilityList([
    providerSpeakerLabelSupportLabel(
      enterprise.speaker_semantics.label_support,
    ),
    ...flags,
  ]);
}

function providerHealthProbeKindLabel(
  probe: NonNullable<ProviderDescriptor["enterprise"]>["health_probes"][number],
): string {
  switch (probe) {
    case "token_acquisition":
      return "Token acquisition";
    case "metadata_only":
      return "Metadata only";
    case "sdk_dependency":
      return "SDK dependency";
    case "endpoint_connectivity":
      return "Endpoint connectivity";
    case "streaming_rpc_availability":
      return "Streaming RPC availability";
    case "live_env_gated_smoke":
      return "Live env-gated smoke";
  }
}

export function providerEndpointModesLabel(
  enterprise: ProviderDescriptor["enterprise"],
): string {
  return formatProviderCapabilityList(
    enterprise?.endpoint_modes.map(providerEndpointModeLabel) ?? [],
  );
}

export function providerRuntimePackagingLabel(
  descriptor: ProviderDescriptor,
): string {
  const packaging =
    descriptor.enterprise?.packaging.map(providerPackagingRequirementLabel) ??
    [];
  const features = descriptor.required_features.map(
    (feature) => `Feature: ${feature}`,
  );
  return formatProviderCapabilityList([...packaging, ...features]);
}

export function providerHealthProbesLabel(
  descriptor: ProviderDescriptor,
): string {
  const probes =
    descriptor.enterprise?.health_probes.map(providerHealthProbeKindLabel) ??
    [];
  const command = descriptor.health_check_command
    ? [`Command: ${descriptor.health_check_command}`]
    : [];
  return formatProviderCapabilityList([...probes, ...command]);
}

export function providerPlatformBlockersLabel(
  descriptor: ProviderDescriptor,
): string {
  const blockers = [
    descriptor.status !== "implemented"
      ? `${providerStatusLabel(descriptor.status)} provider gate`
      : null,
    descriptor.roadmap?.auth_schema === "required_not_wired"
      ? "Credential schema not wired"
      : null,
    ...descriptor.required_features.map((feature) => `Feature: ${feature}`),
    ...(descriptor.enterprise?.packaging.map(
      providerPackagingRequirementLabel,
    ) ?? []),
    descriptor.roadmap?.not_selectable_reason ?? null,
  ].filter((value): value is string => value != null);

  return blockers.length === 0 ? "None declared" : blockers.join(", ");
}

export function providerCapabilityCatalogCountLabel(
  catalogCount: number | null,
  catalogKind: "models" | "voices" | "languages" = "models",
): string {
  if (catalogCount == null) return "Unknown";
  const labels = {
    models: catalogCount === 1 ? "model" : "models",
    voices: catalogCount === 1 ? "voice" : "voices",
    languages: catalogCount === 1 ? "language" : "languages",
  };
  return `${catalogCount} ${labels[catalogKind]}`;
}

export function providerGeneratedCatalogKind(
  descriptor: ProviderDescriptor,
): "models" | "voices" | "languages" {
  return descriptor.id === "tts.deepgram_aura" ? "voices" : "models";
}

export function providerRuntimeReadinessLabel(
  status: NonNullable<ProviderReadiness["runtime"]>["status"],
): string {
  switch (status) {
    case "feature_missing":
      return "Feature missing";
    case "model_missing":
      return "Model missing";
    case "runtime_unavailable":
      return "Runtime unavailable";
    case "load_failed":
      return "Load failed";
    case "healthy":
      return "Healthy";
  }
}

export function providerDefaultModelLabel(
  descriptor: ProviderDescriptor,
): string {
  return descriptor.default_model?.trim() || "Not set";
}

function firstCredentialKey(entry: ProviderReadiness): string | null {
  return entry.credentials[0]?.key ?? null;
}

function coerceDiarizationMode(
  value: DiarizationMode | undefined,
): DiarizationMode {
  return value && DIARIZATION_MODES.includes(value) ? value : "provider";
}

function coerceDiarizationSpeakerCount(
  value: DiarizationSpeakerCount | undefined,
): DiarizationSpeakerCount {
  return value && DIARIZATION_SPEAKER_COUNTS.includes(value) ? value : "auto";
}

// Fields that are transient UI state (test results, in-flight flags, fetched
// catalogs, confirm-delete latch, AWS profile list) rather than user-editable
// settings content. They are excluded from the unsaved-changes ("dirty")
// comparison so that e.g. running a Test Connection or loading the OpenRouter
// catalog does not falsely mark the form as modified.
const DIRTY_IGNORED_FIELDS: ReadonlyArray<keyof SettingsState> = [
  "confirmDelete",
  "awsProfiles",
  "testResults",
  "testingKey",
  "openrouterModels",
  "openrouterModelsLoadedAt",
  "openrouterModelsCacheKey",
  "openrouterModelsLoading",
  "endpointCredentials",
];

// Local (non-reducer) editable state that also participates in dirty tracking.
interface TtsLocalState {
  ttsType: TtsType;
  auraVoice: string;
  auraSpeed: number;
  speakAloud: boolean;
}

/**
 * Serialise the editable slice of the settings form (reducer state minus the
 * ephemeral UI fields, plus the TTS local state) into a stable string we can
 * compare against a baseline snapshot to detect unsaved changes.
 */
function settingsFingerprint(state: SettingsState, tts: TtsLocalState): string {
  const content: Record<string, unknown> = { ...tts };
  (Object.keys(state) as (keyof SettingsState)[]).forEach((key) => {
    if (!DIRTY_IGNORED_FIELDS.includes(key)) {
      content[key as string] = state[key];
    }
  });
  return JSON.stringify(content);
}

export function useSettingsController() {
  const { t, i18n } = useTranslation();
  const modalRef = useFocusTrap<HTMLDivElement>();
  const {
    settings,
    models,
    modelStatus,
    settingsLoading,
    isDownloading,
    downloadProgress,
    isDeletingModel,
    audioSources,
    selectedSourceIds,
    closeSettings,
    requestSourceRecovery,
    saveSettings,
    downloadModel,
    deleteModel,
    listAwsProfiles,
  } = useAudioGraphStore();
  const conversationMode = useAudioGraphStore((s) => s.conversationMode);
  const setConversationMode = useAudioGraphStore((s) => s.setConversationMode);
  const converseEngine = useAudioGraphStore((s) => s.converseEngine);
  const setConverseEngine = useAudioGraphStore((s) => s.setConverseEngine);
  const converseRealtimeAgentProvider = useAudioGraphStore(
    (s) => s.converseRealtimeAgentProvider,
  );
  const nativeRealtimeSelected =
    conversationMode === "converse" && converseEngine === "native";
  const notify = useAudioGraphStore((s) => s.notify);
  const theme = useAudioGraphStore((s) => s.theme);
  const setTheme = useAudioGraphStore((s) => s.setTheme);

  const [state, dispatch] = useReducer(settingsReducer, initialSettingsState);
  const {
    asrType,
    whisperModel,
    asrEndpoint,
    asrApiKey,
    asrModel,
    openaiRealtimeApiKey,
    openaiRealtimeModel,
    openaiRealtimeLanguage,
    awsAsrRegion,
    awsAsrLanguageCode,
    awsAsrCredentialMode,
    awsAsrProfileName,
    awsAsrAccessKey,
    awsAsrSecretKey,
    awsAsrSessionToken,
    awsAsrDiarization,
    deepgramApiKey,
    deepgramModel,
    deepgramDiarization,
    deepgramEndpointingMs,
    deepgramUtteranceEndMs,
    deepgramVadEvents,
    deepgramEotThreshold,
    deepgramEagerEotThreshold,
    deepgramEotTimeoutMs,
    deepgramMaxSpeakers,
    assemblyaiApiKey,
    assemblyaiDiarization,
    sonioxApiKey,
    sonioxModel,
    sonioxDiarization,
    sonioxLanguageIdentification,
    sonioxLanguageHints,
    sonioxMaxSpeakers,
    diarizationMode,
    diarizationSpeakerCount,
    diarizationMaxSpeakers,
    privacyMode,
    sherpaModelDir,
    sherpaEndpointDetection,
    llmType,
    llmEndpoint,
    llmApiKey,
    llmModel,
    llmMaxTokens,
    llmTemperature,
    streamingPrefill,
    mistralrsModelId,
    openrouterApiKey,
    openrouterModel,
    openrouterBaseUrl,
    openrouterIncludeUsageInStream,
    openrouterRoutingPreset,
    openrouterRoutingPolicy,
    openrouterProviderOrderText,
    openrouterModelsLoadedAt,
    openrouterModelsCacheKey,
    awsBedrockRegion,
    awsBedrockModelId,
    awsBedrockCredentialMode,
    awsBedrockProfileName,
    awsBedrockAccessKey,
    awsBedrockSecretKey,
    awsBedrockSessionToken,
    geminiAuthMode,
    geminiApiKey,
    geminiModel,
    geminiProjectId,
    geminiLocation,
    geminiServiceAccountPath,
    audioSampleRate,
    audioChannels,
    logLevel,
    confirmDelete,
    testResults,
    testingKey,
  } = state;

  // ── TTS + speak-aloud (Wave C / ADR-0004 / ADR-0006) ──────────────
  // Kept in local state rather than the heavy settingsReducer to avoid
  // adding 4-6 reducer-action types for a single dropdown + checkbox.
  // Hydrated on settings change in the useEffect block below.
  const [ttsType, setTtsType] = useState<TtsType>("none");
  const [auraVoice, setAuraVoice] = useState<string>(DEFAULT_AURA_VOICE);
  const [auraSpeed, setAuraSpeed] = useState<number>(1.0);
  const [speakAloud, setSpeakAloud] = useState<boolean>(false);
  const [testingTts, setTestingTts] = useState<boolean>(false);
  const [ttsTestResult, setTtsTestResult] = useState<{
    ok: boolean;
    msg: string;
  } | null>(null);
  const [credentialPresence, setCredentialPresence] =
    useState<CredentialPresenceMap>({});
  const [providerReadiness, setProviderReadiness] = useState<
    Record<string, ProviderReadiness>
  >({});
  const [providerReadinessLoading, setProviderReadinessLoading] =
    useState(false);
  const [providerReadinessError, setProviderReadinessError] = useState<
    string | null
  >(null);
  const providerReadinessRequestRef = useRef<string | null>(null);
  const providerReadinessRequestSeqRef = useRef(0);
  const [openrouterModelsError, setOpenrouterModelsError] = useState<
    string | null
  >(null);
  // Accelerator-discovery catalog (seed 7809): the saved-key endpoint + provider
  // payloads, the selected/applied discovery preset, and fetch loading/error.
  // Replaces the hardcoded `"cerebras, groq"` strict-accelerator default — the
  // routing order now comes from the live catalog, not a baked-in constant.
  const [openrouterAcceleratorEndpoints, setOpenrouterAcceleratorEndpoints] =
    useState<OpenRouterModelEndpoints | null>(null);
  const [openrouterAcceleratorProviders, setOpenrouterAcceleratorProviders] =
    useState<OpenRouterProvider[] | null>(null);
  const [openrouterAcceleratorLoading, setOpenrouterAcceleratorLoading] =
    useState(false);
  const [openrouterAcceleratorError, setOpenrouterAcceleratorError] = useState<
    string | null
  >(null);
  const [openrouterAcceleratorPreset, setOpenrouterAcceleratorPreset] =
    useState<AcceleratorPreset>("low_latency");
  const [
    openrouterAppliedAcceleratorPreset,
    setOpenrouterAppliedAcceleratorPreset,
  ] = useState<AcceleratorPreset | null>(null);
  const [cerebrasModels, setCerebrasModels] = useState<
    ProviderModelCatalogItem[]
  >([]);
  const [cerebrasModelsLoading, setCerebrasModelsLoading] = useState(false);
  const [cerebrasModelsError, setCerebrasModelsError] = useState<string | null>(
    null,
  );
  const [cerebrasTesting, setCerebrasTesting] = useState(false);
  const [cerebrasTestResult, setCerebrasTestResult] = useState<{
    ok: boolean;
    msg: string;
  } | null>(null);
  const asrEndpointSavedKeyPresent = credentialIsPresent(
    credentialPresence,
    endpointCredentialKey(asrEndpoint),
  );
  const llmEndpointSavedKeyPresent = credentialIsPresent(
    credentialPresence,
    endpointCredentialKey(llmEndpoint),
  );
  const openaiSavedKeyPresent = credentialIsPresent(
    credentialPresence,
    "openai_api_key",
  );
  const openrouterSavedKeyPresent = credentialIsPresent(
    credentialPresence,
    "openrouter_api_key",
  );
  const cerebrasSavedKeyPresent = credentialIsPresent(
    credentialPresence,
    "cerebras_api_key",
  );
  const openrouterCredentialAvailable =
    openrouterSavedKeyPresent || openrouterApiKey.trim().length > 0;
  const cerebrasCredentialAvailable =
    cerebrasSavedKeyPresent ||
    (llmType === "cerebras" && llmApiKey.trim().length > 0);
  const geminiSavedKeyPresent = credentialIsPresent(
    credentialPresence,
    "gemini_api_key",
  );
  const geminiServiceAccountPathSavedPresent = credentialIsPresent(
    credentialPresence,
    "google_service_account_path",
  );
  const geminiCredentialAvailable =
    geminiSavedKeyPresent || geminiApiKey.trim().length > 0;
  const deepgramSavedKeyPresent = credentialIsPresent(
    credentialPresence,
    "deepgram_api_key",
  );
  const deepgramCredentialAvailable =
    deepgramSavedKeyPresent || deepgramApiKey.trim().length > 0;
  const assemblyaiSavedKeyPresent = credentialIsPresent(
    credentialPresence,
    "assemblyai_api_key",
  );
  const assemblyaiCredentialAvailable =
    assemblyaiSavedKeyPresent || assemblyaiApiKey.trim().length > 0;
  const awsAccessKeySavedPresent = credentialIsPresent(
    credentialPresence,
    "aws_access_key",
  );
  const awsSecretKeySavedPresent = credentialIsPresent(
    credentialPresence,
    "aws_secret_key",
  );
  const awsSessionTokenSavedPresent = credentialIsPresent(
    credentialPresence,
    "aws_session_token",
  );
  const awsSavedKeysPresent =
    awsAccessKeySavedPresent && awsSecretKeySavedPresent;
  const awsAsrAccessKeysAvailable =
    (awsAsrAccessKey.trim().length > 0 && awsAsrSecretKey.trim().length > 0) ||
    (awsAsrAccessKey.trim().length > 0 && awsSecretKeySavedPresent) ||
    (awsAccessKeySavedPresent && awsAsrSecretKey.trim().length > 0) ||
    (awsAccessKeySavedPresent && awsSecretKeySavedPresent);
  const awsBedrockAccessKeysAvailable =
    (awsBedrockAccessKey.trim().length > 0 &&
      awsBedrockSecretKey.trim().length > 0) ||
    (awsBedrockAccessKey.trim().length > 0 && awsSecretKeySavedPresent) ||
    (awsAccessKeySavedPresent && awsBedrockSecretKey.trim().length > 0) ||
    (awsAccessKeySavedPresent && awsSecretKeySavedPresent);
  const activeAsrProviderId = providerIdForSettingsVariant("asr", asrType);
  const activeLlmProviderId = providerIdForSettingsVariant("llm", llmType);
  const activeTtsProviderId = providerIdForSettingsVariant("tts", ttsType);
  const geminiProviderId = "realtime_agent.gemini_live";
  const openaiRealtimeAgentProviderId = "realtime_agent.openai_realtime";
  // When native speech-to-speech is selected, surface the readiness of the
  // realtime agent the user actually runs — Gemini Live or OpenAI Realtime.
  // Previously only the Gemini agent was appended, so a native+OpenAI setup
  // never surfaced OpenAI Realtime agent readiness (WS3 decision 3).
  const activeRealtimeAgentProviderId = nativeRealtimeSelected
    ? converseRealtimeAgentProvider === "openai"
      ? openaiRealtimeAgentProviderId
      : geminiProviderId
    : null;
  const activeReadinessProviderIds = useMemo(
    () => [
      activeAsrProviderId,
      activeLlmProviderId,
      ...(activeRealtimeAgentProviderId ? [activeRealtimeAgentProviderId] : []),
      activeTtsProviderId,
    ],
    [
      activeAsrProviderId,
      activeLlmProviderId,
      activeTtsProviderId,
      activeRealtimeAgentProviderId,
    ],
  );
  const activeReadinessProviderIdSet = useMemo(
    () => new Set(activeReadinessProviderIds),
    [activeReadinessProviderIds],
  );
  const providerReadinessEntries = useMemo(
    () => Object.values(providerReadiness),
    [providerReadiness],
  );
  const visibleProviderReadiness = useMemo(() => {
    return sortProviderReadinessForDashboard(
      providerReadinessEntries.filter((entry) =>
        shouldShowProviderReadiness(entry, activeReadinessProviderIdSet),
      ),
      activeReadinessProviderIds,
    );
  }, [
    activeReadinessProviderIds,
    activeReadinessProviderIdSet,
    providerReadinessEntries,
  ]);
  const providerReadinessStatusEntries = useMemo(() => {
    const visibleProviderIds = new Set(
      visibleProviderReadiness.map((entry) => entry.provider_id),
    );

    return providerReadinessEntries.filter(
      (entry) => visibleProviderIds.has(entry.provider_id) || entry.stale,
    );
  }, [providerReadinessEntries, visibleProviderReadiness]);
  const providerReadinessStatusSummary = useMemo(() => {
    const title = t("settings.providerReadiness.title");
    if (providerReadinessLoading) {
      return `${title}: ${t("settings.providerReadiness.checking")}`;
    }
    if (providerReadinessError) {
      return `${title}: ${t("settings.providerReadiness.status.error")}.`;
    }
    if (providerReadinessStatusEntries.length === 0) {
      return `${title}: 0.`;
    }

    const staleCount = providerReadinessStatusEntries.reduce(
      (acc, entry) => acc + (entry.stale ? 1 : 0),
      0,
    );
    const counts = providerReadinessStatusEntries.reduce<
      Record<string, number>
    >((acc, entry) => {
      const status = entry.status as ProviderReadinessStatusWithBlocked;
      acc[status] = (acc[status] ?? 0) + 1;
      return acc;
    }, {});
    const statuses: ProviderReadinessStatusWithBlocked[] = [
      "ready",
      "missing_credentials",
      "unchecked",
      "error",
      "blocked",
    ];
    const statusSummary = statuses
      .flatMap((status) => {
        const count = counts[status];
        return count
          ? [`${providerReadinessStatusSummaryLabel(status, t)} ${count}`]
          : [];
      })
      .join(". ");
    const staleSummary =
      staleCount > 0 ? ` ${t("settings.providerReadiness.stale")}` : "";
    return `${title}: ${providerReadinessStatusEntries.length}. ${statusSummary}.${staleSummary}`;
  }, [
    providerReadinessError,
    providerReadinessLoading,
    providerReadinessStatusEntries,
    t,
  ]);
  const savedCredentialEntries = useMemo(
    () =>
      Object.values(credentialPresence)
        .filter((entry): entry is CredentialPresence => entry?.present === true)
        .sort((a, b) => a.key.localeCompare(b.key)),
    [credentialPresence],
  );
  const activeAsrProviderReadiness =
    providerReadiness[activeAsrProviderId] ?? null;
  const activeLlmProviderReadiness =
    providerReadiness[activeLlmProviderId] ?? null;
  const activeTtsProviderReadiness =
    providerReadiness[activeTtsProviderId] ?? null;
  const geminiProviderReadiness = providerReadiness[geminiProviderId] ?? null;
  const activeAsrProviderDescriptor =
    PROVIDER_DESCRIPTORS.get(activeAsrProviderId) ?? null;
  const activeLlmProviderDescriptor =
    PROVIDER_DESCRIPTORS.get(activeLlmProviderId) ?? null;
  const activeTtsProviderDescriptor =
    PROVIDER_DESCRIPTORS.get(activeTtsProviderId) ?? null;
  const geminiProviderDescriptor =
    PROVIDER_DESCRIPTORS.get(geminiProviderId) ?? null;
  const openaiRealtimeModelCatalog = useMemo(
    () => modelCatalogForProvider(providerReadiness, "asr.openai_realtime"),
    [providerReadiness],
  );
  const asrApiModelCatalog = useMemo(
    () => modelCatalogForProvider(providerReadiness, "asr.api"),
    [providerReadiness],
  );
  const deepgramModelCatalog = useMemo(
    () => modelCatalogForProvider(providerReadiness, "asr.deepgram"),
    [providerReadiness],
  );
  const llmApiModelCatalog = useMemo(
    () => modelCatalogForProvider(providerReadiness, "llm.api"),
    [providerReadiness],
  );
  const cerebrasReadinessModelCatalog = useMemo(
    () => modelCatalogForProvider(providerReadiness, "llm.cerebras"),
    [providerReadiness],
  );
  const cerebrasModelCatalog = useMemo(
    () =>
      cerebrasModels.length > 0
        ? cerebrasModels
        : cerebrasReadinessModelCatalog,
    [cerebrasModels, cerebrasReadinessModelCatalog],
  );
  const sherpaModelCatalog = useMemo(
    () => modelCatalogForProvider(providerReadiness, "asr.sherpa_onnx"),
    [providerReadiness],
  );
  const mistralrsModelCatalog = useMemo(
    () => modelCatalogForProvider(providerReadiness, "llm.mistralrs"),
    [providerReadiness],
  );
  const geminiModelCatalog = useMemo(
    () =>
      modelCatalogForProvider(providerReadiness, "realtime_agent.gemini_live"),
    [providerReadiness],
  );
  const auraVoiceCatalog = useMemo(
    () => modelCatalogForProvider(providerReadiness, "tts.deepgram_aura"),
    [providerReadiness],
  );
  const selectedModelForProvider = useCallback(
    (providerId: string): string | null => {
      switch (providerId) {
        case "asr.local_whisper":
          return nonEmptyProviderSelection(whisperModel);
        case "asr.api":
          return nonEmptyProviderSelection(asrModel);
        case "asr.openai_realtime":
          return nonEmptyProviderSelection(openaiRealtimeModel);
        case "asr.deepgram":
          return nonEmptyProviderSelection(deepgramModel);
        case "asr.assemblyai":
          return nonEmptyProviderSelection(
            defaultModelForProvider("asr.assemblyai"),
          );
        case "asr.sherpa_onnx":
          return nonEmptyProviderSelection(sherpaModelDir);
        case "llm.local_llama":
        case "llm.api":
        case "llm.cerebras":
          return nonEmptyProviderSelection(llmModel);
        case "llm.openrouter":
          return nonEmptyProviderSelection(openrouterModel);
        case "llm.aws_bedrock":
          return nonEmptyProviderSelection(awsBedrockModelId);
        case "llm.mistralrs":
          return nonEmptyProviderSelection(mistralrsModelId);
        case "realtime_agent.gemini_live":
          return nonEmptyProviderSelection(geminiModel);
        case "tts.deepgram_aura":
          return nonEmptyProviderSelection(auraVoice);
        default:
          return null;
      }
    },
    [
      asrModel,
      auraVoice,
      awsBedrockModelId,
      deepgramModel,
      geminiModel,
      llmModel,
      mistralrsModelId,
      openaiRealtimeModel,
      openrouterModel,
      sherpaModelDir,
      whisperModel,
    ],
  );
  const providerDiarizationSupported = [
    "aws_transcribe",
    "deepgram",
    "assemblyai",
  ].includes(asrType);
  const localDiarizationReady = modelStatus?.sortformer === "Ready";
  const providerDiarizationRequested =
    diarizationMode === "provider" || diarizationMode === "hybrid";
  const selectedDiarizationModeUnavailable =
    (providerDiarizationRequested && !providerDiarizationSupported) ||
    ((diarizationMode === "local" || diarizationMode === "hybrid") &&
      !localDiarizationReady);
  const providerSetupModeCards = useMemo(
    () =>
      deriveProviderSetupModeCards({
        settings: state,
        credentialPresence,
        providerReadiness,
        sourceState: { sources: audioSources, selectedSourceIds },
        tts: { ttsType, auraVoice, speakAloud },
        conversationMode,
        converseEngine,
      }),
    [
      audioSources,
      auraVoice,
      conversationMode,
      converseEngine,
      credentialPresence,
      providerReadiness,
      selectedSourceIds,
      speakAloud,
      state,
      ttsType,
    ],
  );

  // ── Unsaved-changes tracking (W3.5) ───────────────────────────────────
  // `baselineRef` holds the fingerprint of the last loaded/saved form so we
  // can detect whether the working draft diverges (i.e. is "dirty"). It is
  // (re)set after hydration from `settings` and after a successful Save.
  // `confirmingClose` drives the inline "Discard unsaved changes?" bar shown
  // when the user tries to close (X / overlay / Escape) with pending edits.
  const baselineRef = useRef<string | null>(null);
  // Bumped whenever a fresh hydration completes (including the async
  // credential mirroring) so the effect below can recapture the baseline
  // fingerprint from the now-settled reducer state.
  const [baselineEpoch, setBaselineEpoch] = useState(0);
  const [confirmingClose, setConfirmingClose] = useState(false);
  const ttsLocal: TtsLocalState = { ttsType, auraVoice, auraSpeed, speakAloud };
  // ttsLocal is reconstructed each render from its constituent fields, so we
  // depend on those primitives rather than the wrapper object identity (which
  // would change every render and defeat the memo).
  // biome-ignore lint/correctness/useExhaustiveDependencies: depend on ttsLocal's primitive fields, not its per-render identity
  const fingerprint = useMemo(
    () => settingsFingerprint(state, ttsLocal),
    [state, ttsType, auraVoice, auraSpeed, speakAloud],
  );
  const dirty =
    baselineRef.current !== null && baselineRef.current !== fingerprint;

  // Capture (or recapture) the dirty baseline whenever a hydration cycle
  // completes. Runs after the synchronous + async HYDRATE_FROM_SETTINGS
  // dispatches have flushed into `state`, so the fingerprint reflects the
  // freshly loaded settings rather than the pre-hydration defaults.
  // We deliberately depend only on the epoch: recapturing on every fingerprint
  // change would defeat dirty tracking entirely.
  // biome-ignore lint/correctness/useExhaustiveDependencies: recapture baseline only on hydration epoch bumps, not on every fingerprint change
  useEffect(() => {
    if (baselineEpoch === 0) return;
    baselineRef.current = fingerprint;
    setConfirmingClose(false);
  }, [baselineEpoch]);

  // Settings are grouped into a left rail (vertical tablist) to keep the modal
  // navigable. The rail items, grouping (Setup / Providers / App), group order,
  // and the `SettingsTab` union are the single source of truth in
  // `settingsRailConfig.ts` (blueprint §1.1, Phase 4 STEP 4); the controller and
  // the presentational rail both read from there. Aliased to SETTINGS_TABS for
  // the keyboard handler + context-value key that consumers already reference.
  const SETTINGS_TABS = RAIL_SECTIONS;
  const [activeTab, setActiveTab] = useState<SettingsTab>("overview");
  // Below the narrow breakpoint the rail flips to a horizontal tablist, so the
  // announced orientation flips too (blueprint §1.4). The doubled arrow
  // handlers (Up/Down AND Left/Right) keep working regardless.
  const [railHorizontal, setRailHorizontal] = useState(false);
  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const query = window.matchMedia("(max-width: 720px)");
    const apply = () => setRailHorizontal(query.matches);
    apply();
    query.addEventListener?.("change", apply);
    return () => query.removeEventListener?.("change", apply);
  }, []);
  const tabRefs = useRef<
    Partial<Record<SettingsTab, HTMLButtonElement | null>>
  >({});
  const tabButtonId = (tab: SettingsTab) => `settings-tab-${tab}`;
  const tabPanelId = (tab: SettingsTab) => `settings-panel-${tab}`;

  const selectSettingsTab = (tab: SettingsTab) => {
    setActiveTab(tab);
    tabRefs.current[tab]?.focus();
  };

  const focusSettingsField = (fieldId: string, activate = false) => {
    window.setTimeout(() => {
      const element = document.getElementById(fieldId);
      if (!(element instanceof HTMLElement)) return;
      if (activate && element instanceof HTMLButtonElement) {
        element.click();
        focusSettingsField(fieldId);
        return;
      }
      // Reduced-motion users get an instant scroll and no pulse (blueprint §3).
      const prefersReducedMotion =
        typeof window.matchMedia === "function" &&
        window.matchMedia("(prefers-reduced-motion: reduce)").matches;
      element.scrollIntoView?.({
        block: "nearest",
        behavior: prefersReducedMotion ? "auto" : "smooth",
      });
      if (!element.matches("input, select, textarea, button, [tabindex]")) {
        element.setAttribute("tabindex", "-1");
      }
      element.focus();
      // Landed-field highlight pulse — paired with focus so SR + sighted users
      // get the same "you are here" signal after a deep-link (blueprint §3).
      // The CSS animation is itself gated on prefers-reduced-motion, so the
      // class is harmless to add either way; we still skip the timer churn when
      // motion is reduced.
      if (!prefersReducedMotion) {
        element.classList.add("settings-landed");
        window.setTimeout(() => {
          element.classList.remove("settings-landed");
        }, 1500);
      }
    }, 0);
  };

  const credentialRouteForProviderCredential = (
    providerId: string,
    credentialKey: string | null,
  ): CredentialRoute | null => {
    switch (providerId) {
      case "asr.api":
        return {
          tab: "stt",
          fieldId: "asr-api-key",
          activate: true,
          apply: () => dispatch(setField("asrType", "api")),
        };
      case "asr.openai_realtime":
        return {
          tab: "stt",
          fieldId: "openai-realtime-api-key",
          activate: true,
          apply: () => dispatch(setField("asrType", "openai_realtime")),
        };
      case "realtime_agent.openai_realtime":
        if (credentialKey !== "openai_api_key") return null;
        return {
          tab: "stt",
          fieldId: "openai-realtime-api-key",
          activate: true,
          apply: () => dispatch(setField("asrType", "openai_realtime")),
        };
      case "asr.deepgram":
        return {
          tab: "stt",
          fieldId: "deepgram-api-key",
          activate: true,
          apply: () => dispatch(setField("asrType", "deepgram")),
        };
      case "asr.assemblyai":
        return {
          tab: "stt",
          fieldId: "assemblyai-api-key",
          activate: true,
          apply: () => dispatch(setField("asrType", "assemblyai")),
        };
      case "asr.aws_transcribe":
        return {
          tab: "stt",
          fieldId: "aws-asr-access-key",
          activate: true,
          apply: () => {
            dispatch(setField("asrType", "aws_transcribe"));
            dispatch(setField("awsAsrCredentialMode", "access_keys"));
          },
        };
      case "llm.api":
        return {
          tab: "llm",
          fieldId: "llm-custom-api-key",
          activate: true,
          apply: () => dispatch(setField("llmType", "api")),
        };
      case "llm.cerebras":
        return {
          tab: "llm",
          fieldId: "llm-cerebras-api-key",
          activate: true,
          apply: () => dispatch(setField("llmType", "cerebras")),
        };
      case "llm.openrouter":
        return {
          tab: "llm",
          fieldId: "llm-openrouter-api-key",
          activate: true,
          apply: () => dispatch(setField("llmType", "openrouter")),
        };
      case "llm.aws_bedrock":
        return {
          tab: "llm",
          fieldId: "llm-bedrock-access-key",
          activate: true,
          apply: () => {
            dispatch(setField("llmType", "aws_bedrock"));
            dispatch(setField("awsBedrockCredentialMode", "access_keys"));
          },
        };
      case "realtime_agent.gemini_live":
        if (credentialKey !== "gemini_api_key") return null;
        return {
          tab: "gemini",
          fieldId: "gemini-api-key",
          activate: true,
          apply: () => dispatch(setField("geminiAuthMode", "api_key")),
        };
      default:
        return null;
    }
  };

  const credentialRouteForReadiness = (
    entry: ProviderReadiness,
    credentialKey = firstCredentialKey(entry),
  ): CredentialRoute | null => {
    return credentialRouteForProviderCredential(
      entry.provider_id,
      credentialKey,
    );
  };

  const activeOpenAiCredentialRoute = (): CredentialRoute | null => {
    if (
      llmType === "api" &&
      endpointCredentialKey(llmEndpoint) === "openai_api_key"
    ) {
      return credentialRouteForProviderCredential("llm.api", "openai_api_key");
    }
    if (
      asrType === "api" &&
      endpointCredentialKey(asrEndpoint) === "openai_api_key"
    ) {
      return credentialRouteForProviderCredential("asr.api", "openai_api_key");
    }
    if (asrType === "openai_realtime") {
      return credentialRouteForProviderCredential(
        "asr.openai_realtime",
        "openai_api_key",
      );
    }

    return null;
  };

  const readinessOpenAiCredentialRoute = (
    readinessEntries: ProviderReadiness[],
  ): CredentialRoute | null => {
    const readinessPriority = ["llm.api", "asr.api", "asr.openai_realtime"];
    for (const providerId of readinessPriority) {
      const entry = readinessEntries.find(
        (candidate) => candidate.provider_id === providerId,
      );
      if (!entry) continue;
      const route = credentialRouteForReadiness(entry, "openai_api_key");
      if (route) return route;
    }

    return (
      readinessEntries
        .map((entry) => credentialRouteForReadiness(entry, "openai_api_key"))
        .find((route): route is CredentialRoute => route != null) ?? null
    );
  };

  const activeProviderCredentialRouteForKey = (
    key: string,
  ): CredentialRoute | null => {
    if (key === "openai_api_key") return activeOpenAiCredentialRoute();

    for (const providerId of activeReadinessProviderIds) {
      if (
        providerId === "asr.api" &&
        endpointCredentialKey(asrEndpoint) !== key
      )
        continue;
      if (
        providerId === "llm.api" &&
        endpointCredentialKey(llmEndpoint) !== key
      )
        continue;
      const descriptor = PROVIDER_DESCRIPTORS.get(providerId);
      if (!descriptor?.credential_keys.includes(key)) continue;
      const route = credentialRouteForProviderCredential(providerId, key);
      if (route) return route;
    }

    return null;
  };

  const fallbackCredentialRouteForKey = (
    key: string,
  ): CredentialRoute | null => {
    const activeRoute = activeProviderCredentialRouteForKey(key);
    if (activeRoute) return activeRoute;

    switch (key) {
      case "openai_api_key":
        return null;
      case "openrouter_api_key":
        return {
          tab: "llm",
          fieldId: "llm-openrouter-api-key",
          activate: true,
          apply: () => dispatch(setField("llmType", "openrouter")),
        };
      case "cerebras_api_key":
        return {
          tab: "llm",
          fieldId: "llm-cerebras-api-key",
          activate: true,
          apply: () => dispatch(setField("llmType", "cerebras")),
        };
      case "deepgram_api_key":
        return {
          tab: "stt",
          fieldId: "deepgram-api-key",
          activate: true,
          apply: () => dispatch(setField("asrType", "deepgram")),
        };
      case "assemblyai_api_key":
        return {
          tab: "stt",
          fieldId: "assemblyai-api-key",
          activate: true,
          apply: () => dispatch(setField("asrType", "assemblyai")),
        };
      case "gemini_api_key":
        return {
          tab: "gemini",
          fieldId: "gemini-api-key",
          activate: true,
          apply: () => dispatch(setField("geminiAuthMode", "api_key")),
        };
      case "aws_access_key":
      case "aws_secret_key":
      case "aws_session_token":
        return {
          tab: "stt",
          fieldId: "aws-asr-access-key",
          activate: true,
          apply: () => {
            dispatch(setField("asrType", "aws_transcribe"));
            dispatch(setField("awsAsrCredentialMode", "access_keys"));
          },
        };
      default:
        return null;
    }
  };

  const relatedReadinessForCredential = (key: string): ProviderReadiness[] =>
    providerReadinessEntries.filter((entry) =>
      entry.credentials.some((credential) => credential.key === key),
    );

  const providerLabelsForCredential = (
    key: string,
    readinessEntries: ProviderReadiness[],
  ): string[] => {
    const labels = new Set<string>();
    for (const entry of readinessEntries) {
      labels.add(
        PROVIDER_READINESS_LABELS.get(entry.provider_id) ?? entry.provider_id,
      );
    }
    for (const descriptor of PROVIDER_DESCRIPTORS.values()) {
      if (descriptor.credential_keys.includes(key)) {
        labels.add(descriptor.display_name);
      }
    }
    return [...labels].sort((a, b) => a.localeCompare(b));
  };

  const latestValidationForCredential = (
    readinessEntries: ProviderReadiness[],
  ): number | null => {
    let latest: number | null = null;
    for (const entry of readinessEntries) {
      if (
        typeof entry.checked_at === "number" &&
        Number.isFinite(entry.checked_at) &&
        entry.checked_at > 0 &&
        (latest == null || entry.checked_at > latest)
      ) {
        latest = entry.checked_at;
      }
    }
    return latest;
  };

  const credentialRouteForKey = (key: string): CredentialRoute | null => {
    const relatedReadiness = relatedReadinessForCredential(key);
    if (key === "openai_api_key") {
      return (
        activeOpenAiCredentialRoute() ??
        readinessOpenAiCredentialRoute(relatedReadiness)
      );
    }

    const activeReadinessRoute = activeReadinessProviderIds
      .flatMap((providerId) =>
        relatedReadiness.filter((entry) => entry.provider_id === providerId),
      )
      .map((entry) => credentialRouteForReadiness(entry, key))
      .find((route): route is CredentialRoute => route != null);
    if (activeReadinessRoute) return activeReadinessRoute;

    const activeConfiguredRoute = activeProviderCredentialRouteForKey(key);
    if (activeConfiguredRoute) return activeConfiguredRoute;

    const readinessRoute = relatedReadiness
      .map((entry) => credentialRouteForReadiness(entry, key))
      .find((route): route is CredentialRoute => route != null);
    return readinessRoute ?? fallbackCredentialRouteForKey(key);
  };

  const openSettingsControlRoute = (route: SettingsControlRoute) => {
    route.apply?.();
    setActiveTab(route.tab);
    focusSettingsField(route.fieldId, route.activate);
  };

  const openCredentialRoute = (route: CredentialRoute) => {
    openSettingsControlRoute(route);
  };

  const handleProviderSetupSourceRecovery = (card: ProviderSetupModeCard) => {
    requestSourceRecovery({
      origin: "provider_setup",
      issues: providerSetupSourceRecoveryIssues(card),
    });
    requestClose();
  };

  // Interactive mode selection (settings redesign WS1 / FINAL DECISION 1):
  // pick a product-mode card and drive the store + reducer so
  // `selectedModeId()` re-classifies to that card.
  //
  //  - `native_realtime` is the clean two-flag toggle: conversationMode
  //    "converse" + converseEngine "native" (keeps legacy `nativeS2sEnabled`
  //    in sync via the store setter).
  //  - The three durable cards (`local_private`/`cloud_fast`/`hybrid`) leave
  //    native (notes + pipelined) AND swap the ASR/LLM provider selection to
  //    the exact providers the card was DERIVED from. `selectedModeId()`
  //    classifies local/cloud/hybrid from ASR/LLM provider locality, so a bare
  //    flag flip cannot move between them — we mirror the card's derived
  //    `stageCoverage` provider ids into the reducer's `asrType`/`llmType`
  //    (settings variant = provider id minus its `${stage}.` prefix). Routing
  //    through `setField` flows into `state`, which is both the
  //    `deriveProviderSetupModeCards` input and the dirty-tracking fingerprint
  //    source, so Save picks the change up.
  const handleSelectProductMode = (card: ProviderSetupModeCard) => {
    if (card.productPath === "native_realtime_agent") {
      setConversationMode("converse");
      setConverseEngine("native");
      return;
    }

    setConversationMode("notes");
    setConverseEngine("pipelined");

    for (const coverage of card.stageCoverage) {
      const variant = coverage.providerId.startsWith(`${coverage.stage}.`)
        ? coverage.providerId.slice(coverage.stage.length + 1)
        : coverage.providerId;
      if (coverage.stage === "asr") {
        dispatch(setField("asrType", variant as AsrType));
      } else if (coverage.stage === "llm") {
        dispatch(setField("llmType", variant as LlmType));
      }
    }
  };

  const handleOpenCredentialRoute = (entry: ProviderReadiness) => {
    const route = credentialRouteForReadiness(entry);
    if (!route) return;
    openCredentialRoute(route);
  };

  const handleOpenCredentialKey = (key: string) => {
    const route = credentialRouteForKey(key);
    if (!route) return;
    openCredentialRoute(route);
  };

  const providerRouteForProviderId = (
    providerId: string,
  ): SettingsControlRoute | null => {
    switch (providerId) {
      case "asr.local_whisper":
        return {
          tab: "stt",
          fieldId: "asr-whisper-model",
          apply: () => dispatch(setField("asrType", "local_whisper")),
        };
      case "asr.api":
        return {
          tab: "stt",
          fieldId: "asr-endpoint",
          apply: () => dispatch(setField("asrType", "api")),
        };
      case "asr.openai_realtime":
        return {
          tab: "stt",
          fieldId: "openai-realtime-model",
          apply: () => dispatch(setField("asrType", "openai_realtime")),
        };
      case "asr.aws_transcribe":
        return {
          tab: "stt",
          fieldId: "aws-asr-region",
          apply: () => dispatch(setField("asrType", "aws_transcribe")),
        };
      case "asr.deepgram":
        return {
          tab: "stt",
          fieldId: "deepgram-model",
          apply: () => dispatch(setField("asrType", "deepgram")),
        };
      case "asr.assemblyai":
        return {
          tab: "stt",
          fieldId: "assemblyai-api-key",
          apply: () => dispatch(setField("asrType", "assemblyai")),
        };
      case "asr.sherpa_onnx":
        return {
          tab: "stt",
          fieldId: "sherpa-model-dir",
          apply: () => dispatch(setField("asrType", "sherpa_onnx")),
        };
      case "llm.local_llama":
        return {
          tab: "llm",
          fieldId: "streaming-prefill-toggle",
          apply: () => dispatch(setField("llmType", "local_llama")),
        };
      case "llm.api":
        return {
          tab: "llm",
          fieldId: "llm-custom-endpoint",
          apply: () => dispatch(setField("llmType", "api")),
        };
      case "llm.cerebras":
        return {
          tab: "llm",
          fieldId: "llm-cerebras-model",
          apply: () => dispatch(setField("llmType", "cerebras")),
        };
      case "llm.openrouter":
        return {
          tab: "llm",
          fieldId: "llm-openrouter-model",
          apply: () => dispatch(setField("llmType", "openrouter")),
        };
      case "llm.aws_bedrock":
        return {
          tab: "llm",
          fieldId: "llm-bedrock-region",
          apply: () => dispatch(setField("llmType", "aws_bedrock")),
        };
      case "llm.mistralrs":
        return {
          tab: "llm",
          fieldId: "llm-mistralrs-model-id",
          apply: () => dispatch(setField("llmType", "mistralrs")),
        };
      case "realtime_agent.gemini_live":
        return { tab: "gemini", fieldId: "gemini-model" };
      case "tts.none":
        return {
          tab: "tts",
          fieldId: "tts-provider-select",
          apply: () => setTtsType("none"),
        };
      case "tts.deepgram_aura":
        return {
          tab: "tts",
          fieldId: "tts-provider-select",
          apply: () => setTtsType("deepgram_aura"),
        };
      default:
        return null;
    }
  };

  const modelRouteForProviderId = (
    providerId: string,
  ): SettingsControlRoute | null => {
    switch (providerId) {
      case "asr.local_whisper":
        return providerRouteForProviderId(providerId);
      case "asr.api":
        return {
          tab: "stt",
          fieldId: "asr-model",
          apply: () => dispatch(setField("asrType", "api")),
        };
      case "asr.openai_realtime":
        return {
          tab: "stt",
          fieldId: "openai-realtime-model",
          apply: () => dispatch(setField("asrType", "openai_realtime")),
        };
      case "asr.deepgram":
        return {
          tab: "stt",
          fieldId: "deepgram-model",
          apply: () => dispatch(setField("asrType", "deepgram")),
        };
      case "asr.sherpa_onnx":
        return {
          tab: "stt",
          fieldId: "sherpa-model-dir",
          apply: () => dispatch(setField("asrType", "sherpa_onnx")),
        };
      case "llm.local_llama":
        return { tab: "general", fieldId: "settings-models-section" };
      case "llm.api":
        return {
          tab: "llm",
          fieldId: "llm-custom-model",
          apply: () => dispatch(setField("llmType", "api")),
        };
      case "llm.cerebras":
        return {
          tab: "llm",
          fieldId: "llm-cerebras-model",
          apply: () => dispatch(setField("llmType", "cerebras")),
        };
      case "llm.openrouter":
        return {
          tab: "llm",
          fieldId: "llm-openrouter-model",
          apply: () => dispatch(setField("llmType", "openrouter")),
        };
      case "llm.aws_bedrock":
        return {
          tab: "llm",
          fieldId: "llm-bedrock-model-id",
          apply: () => dispatch(setField("llmType", "aws_bedrock")),
        };
      case "llm.mistralrs":
        return {
          tab: "llm",
          fieldId: "llm-mistralrs-model-id",
          apply: () => dispatch(setField("llmType", "mistralrs")),
        };
      case "realtime_agent.gemini_live":
        return { tab: "gemini", fieldId: "gemini-model" };
      case "tts.deepgram_aura":
        return {
          tab: "tts",
          fieldId: "aura-voice-select",
          apply: () => setTtsType("deepgram_aura"),
        };
      default:
        return null;
    }
  };

  const credentialRouteForProviderSetupSelection = (
    selection: ProviderSetupProviderSelection,
    credentialKey: string | null = selection.credentials[0]?.key ?? null,
  ): SettingsControlRoute | null => {
    if (selection.providerId === "tts.deepgram_aura") {
      return {
        tab: "tts",
        fieldId: "tts-deepgram-api-key",
        apply: () => setTtsType("deepgram_aura"),
      };
    }
    if (
      selection.providerId === "realtime_agent.gemini_live" &&
      credentialKey === "google_service_account_path"
    ) {
      return {
        tab: "gemini",
        fieldId: "gemini-service-account-path",
        apply: () => dispatch(setField("geminiAuthMode", "vertex_ai")),
      };
    }
    return credentialRouteForProviderCredential(
      selection.providerId,
      credentialKey,
    );
  };

  const providerSetupSelectionForBlocker = (
    card: ProviderSetupModeCard,
    blocker: ProviderSetupBlocker,
  ): ProviderSetupProviderSelection | null => {
    return (
      card.selectedProviders.find(
        (selection) => selection.providerId === blocker.providerId,
      ) ?? null
    );
  };

  const firstProviderSetupRoute = (
    card: ProviderSetupModeCard,
    routeForSelection: (
      selection: ProviderSetupProviderSelection,
    ) => SettingsControlRoute | null,
  ): SettingsControlRoute | null => {
    for (const selection of card.selectedProviders) {
      const route = routeForSelection(selection);
      if (route) return route;
    }

    return null;
  };

  const providerSetupProviderRoute = (
    card: ProviderSetupModeCard,
  ): SettingsControlRoute | null =>
    firstProviderSetupRoute(card, (selection) =>
      providerRouteForProviderId(selection.providerId),
    );

  const providerSetupCredentialRoute = (
    card: ProviderSetupModeCard,
  ): SettingsControlRoute | null => {
    for (const blocker of card.missingBlockers) {
      if (blocker.kind !== "missing_credential") continue;
      const selection = providerSetupSelectionForBlocker(card, blocker);
      if (!selection) continue;
      const route = credentialRouteForProviderSetupSelection(
        selection,
        blocker.key ?? null,
      );
      if (route) return route;
    }

    for (const selection of card.selectedProviders) {
      const route = credentialRouteForProviderSetupSelection(selection);
      if (route) return route;
    }

    return null;
  };

  const providerSetupModelRoute = (
    card: ProviderSetupModeCard,
  ): SettingsControlRoute | null => {
    for (const blocker of card.missingBlockers) {
      if (
        blocker.kind !== "model_unselected" &&
        blocker.kind !== "missing_model"
      )
        continue;
      const selection = providerSetupSelectionForBlocker(card, blocker);
      if (!selection) continue;
      const route = modelRouteForProviderId(selection.providerId);
      if (route) return route;
    }

    return firstProviderSetupRoute(card, (selection) =>
      modelRouteForProviderId(selection.providerId),
    );
  };

  const handleSettingsTabKeyDown = (
    e: ReactKeyboardEvent<HTMLButtonElement>,
    tab: SettingsTab,
  ) => {
    const currentIndex = SETTINGS_TABS.findIndex((item) => item.id === tab);
    if (currentIndex < 0) return;

    let nextIndex: number | null = null;
    if (e.key === "ArrowRight" || e.key === "ArrowDown") {
      nextIndex = (currentIndex + 1) % SETTINGS_TABS.length;
    } else if (e.key === "ArrowLeft" || e.key === "ArrowUp") {
      nextIndex =
        (currentIndex - 1 + SETTINGS_TABS.length) % SETTINGS_TABS.length;
    } else if (e.key === "Home") {
      nextIndex = 0;
    } else if (e.key === "End") {
      nextIndex = SETTINGS_TABS.length - 1;
    }

    if (nextIndex === null) return;
    e.preventDefault();
    selectSettingsTab(SETTINGS_TABS[nextIndex].id);
  };

  const refreshAwsProfiles = async () => {
    dispatch({ type: "SET_AWS_PROFILES", profiles: await listAwsProfiles() });
  };

  const refreshCredentialPresence = async () => {
    try {
      const presence = await invoke<CredentialPresence[]>(
        "load_credential_presence_cmd",
      );
      setProviderReadinessError(null);
      setCredentialPresence(credentialPresenceFromEntries(presence));
    } catch (e) {
      setProviderReadinessError(errorToMessage(e));
      setCredentialPresence({});
    }
  };

  const applyProviderReadiness = useCallback(
    (readiness: ProviderReadiness[], openrouterCatalogBaseUrl: string) => {
      setProviderReadinessError(null);
      setProviderReadiness(providerReadinessFromEntries(readiness));
      const openrouterReadiness = readiness.find(
        (entry) =>
          entry.provider_id === "llm.openrouter" &&
          entry.openrouter_models &&
          entry.openrouter_models.length > 0,
      );
      if (openrouterReadiness?.openrouter_models?.length) {
        setOpenrouterModelsError(null);
        dispatch({
          type: "SET_OPENROUTER_MODELS",
          models: openrouterReadiness.openrouter_models,
          loadedAt: Date.now(),
          cacheKey: openRouterModelsCacheKey(
            openrouterCatalogBaseUrl,
            "__saved__",
          ),
        });
      }
    },
    [],
  );

  const cancelProviderReadinessRequest = useCallback(
    (requestId: string | null) => {
      if (!requestId) return;
      void invoke<boolean>("cancel_provider_readiness_cmd", {
        requestId,
      }).catch((e) => {
        console.error("Failed to cancel provider readiness:", e);
      });
    },
    [],
  );

  const beginProviderReadinessRequest = useCallback(() => {
    providerReadinessRequestSeqRef.current += 1;
    const requestId = `settings-readiness-${Date.now()}-${providerReadinessRequestSeqRef.current}`;
    const previousRequestId = providerReadinessRequestRef.current;
    if (previousRequestId) {
      cancelProviderReadinessRequest(previousRequestId);
    }
    providerReadinessRequestRef.current = requestId;
    return requestId;
  }, [cancelProviderReadinessRequest]);

  const isCurrentProviderReadinessRequest = useCallback(
    (requestId: string) => providerReadinessRequestRef.current === requestId,
    [],
  );

  const clearProviderReadinessRequest = useCallback((requestId: string) => {
    if (providerReadinessRequestRef.current === requestId) {
      providerReadinessRequestRef.current = null;
      return true;
    }
    return false;
  }, []);

  const refreshProviderReadiness = async (
    options: {
      force?: boolean;
      conversationMode?: "notes" | "converse";
      converseEngine?: "native" | "pipelined";
    } = { force: true },
  ) => {
    const requestId = beginProviderReadinessRequest();
    setProviderReadinessLoading(true);
    try {
      const readiness =
        (await invoke<ProviderReadiness[]>("get_provider_readiness_cmd", {
          refresh: true,
          force: options.force ?? false,
          conversationMode: options.conversationMode ?? conversationMode,
          converseEngine: options.converseEngine ?? converseEngine,
          requestId,
        })) ?? [];
      if (!isCurrentProviderReadinessRequest(requestId)) return;
      applyProviderReadiness(
        readiness,
        normalizeOpenRouterBaseUrl(openrouterBaseUrl),
      );
    } catch (e) {
      if (!isCurrentProviderReadinessRequest(requestId)) return;
      console.error("Failed to load provider readiness:", e);
      setProviderReadinessError(errorToMessage(e));
      setProviderReadiness({});
    } finally {
      if (clearProviderReadinessRequest(requestId)) {
        setProviderReadinessLoading(false);
      }
    }
  };

  const handleNativeRealtimeToggle = (enabled: boolean) => {
    if (enabled) {
      setConversationMode("converse");
      setConverseEngine("native");
      return;
    }
    setConverseEngine("pipelined");
  };

  // Upper bound on any Test Connection invocation. Without this, a hung
  // network call (e.g. provider stuck in TLS handshake, firewall silently
  // dropping packets) leaves the button forever stuck on "Testing…".
  const TEST_TIMEOUT_MS = 10_000;

  const runTest = async (key: TestKey, invocation: () => Promise<string>) => {
    // Debounce: reject rapid re-clicks while a test is already in flight.
    if (testingKey !== null) return;
    dispatch({ type: "TEST_START", key });
    try {
      const msg = await Promise.race([
        invocation(),
        new Promise<never>((_, reject) =>
          setTimeout(
            () =>
              reject(
                new Error(
                  t("settings.errors.testTimeout", {
                    seconds: TEST_TIMEOUT_MS / 1000,
                  }),
                ),
              ),
            TEST_TIMEOUT_MS,
          ),
        ),
      ]);
      dispatch({ type: "TEST_RESULT", key, result: { ok: true, msg } });
    } catch (e) {
      dispatch({
        type: "TEST_RESULT",
        key,
        result: { ok: false, msg: errorToMessage(e) },
      });
    } finally {
      dispatch({ type: "TEST_FINISH" });
    }
  };

  // Clear a stored credential (mirrors the Rust `delete_credential` path).
  const handleClearCredential = async (
    key: string | string[],
    label: string,
    clearLocal: () => void,
  ) => {
    const ok = window.confirm(
      t("settings.credentialConfirm.clearPrompt", { label }),
    );
    if (!ok) return;
    const keys = Array.isArray(key) ? key : [key];
    try {
      for (const credentialKey of keys) {
        await invoke("delete_credential_cmd", { key: credentialKey });
      }
      clearLocal();
      await refreshCredentialPresence();
      void refreshProviderReadiness();
    } catch (e) {
      console.error(`Failed to clear ${keys.join(", ")}:`, e);
      window.alert(
        t("settings.errors.failedToClear", { error: errorToMessage(e) }),
      );
    }
  };

  const handleTestAsrApi = () =>
    runTest("asr_api", () =>
      invoke<string>("test_cloud_asr_connection", {
        endpoint: asrEndpoint,
        apiKey: asrApiKey.trim() || null,
      }),
    );

  const handleTestDeepgram = () =>
    runTest("deepgram", () =>
      invoke<string>("test_deepgram_connection", {
        apiKey: deepgramApiKey.trim() || null,
      }),
    );

  // TTS connection test — uses the dedicated test_tts_connection_cmd which,
  // for the Aura provider, just delegates to the Deepgram STT probe (same
  // key works for both surfaces). Outside the runTest reducer infrastructure
  // because TTS state lives in local useState; reusing runTest would force
  // us to add a TestKey variant and a reducer arm just for this.
  const handleTestTts = async () => {
    if (testingTts) return;
    setTestingTts(true);
    setTtsTestResult(null);
    try {
      const msg = await invoke<string>("test_tts_connection_cmd", {
        provider: ttsType === "deepgram_aura" ? "deepgram_aura" : "none",
        apiKey: deepgramApiKey.trim() || null,
      });
      setTtsTestResult({ ok: true, msg });
    } catch (err) {
      setTtsTestResult({ ok: false, msg: errorToMessage(err) });
    } finally {
      setTestingTts(false);
    }
  };

  const handleTestAssemblyAI = () =>
    runTest("assemblyai", () =>
      invoke<string>("test_assemblyai_connection", {
        apiKey: assemblyaiApiKey.trim() || null,
      }),
    );

  const handleTestGemini = () =>
    runTest("gemini", () =>
      invoke<string>("test_gemini_api_key", {
        apiKey: geminiApiKey.trim() || null,
      }),
    );

  const handleTestAwsAsr = async () => {
    const credential_source = buildAwsCredentialSource(
      awsAsrCredentialMode,
      awsAsrProfileName,
      awsAsrAccessKey,
    );
    return runTest("aws_asr", () =>
      invoke<string>("test_aws_credentials", {
        region: awsAsrRegion,
        credentialSource: credential_source,
        secretAccessKey:
          awsAsrCredentialMode === "access_keys"
            ? awsAsrSecretKey.trim() || null
            : null,
        sessionToken:
          awsAsrCredentialMode === "access_keys"
            ? awsAsrSessionToken.trim() || null
            : null,
      }),
    );
  };

  // OpenRouter model catalog cache TTL (ms). 5 min keeps the dropdown fresh
  // while avoiding hammering /api/v1/models on every settings render.
  const OPENROUTER_MODELS_CACHE_TTL_MS = 5 * 60 * 1000;

  const handleTestOpenRouter = async () => {
    const baseUrl = normalizeOpenRouterBaseUrl(openrouterBaseUrl);
    return runTest("openrouter", () =>
      invoke<string>("test_openrouter_connection_cmd", {
        apiKey: openrouterApiKey.trim() || null,
        baseUrl,
      }),
    );
  };

  const handleRefreshOpenRouterModels = async () => {
    if (!openrouterCredentialAvailable) return;
    setOpenrouterModelsError(null);
    const baseUrl = normalizeOpenRouterBaseUrl(openrouterBaseUrl);
    const cacheKey = openRouterModelsCacheKey(
      baseUrl,
      openrouterApiKey.trim() || (openrouterSavedKeyPresent ? "__saved__" : ""),
    );
    // Skip if cached payload is still fresh (avoid re-hitting the catalog
    // when the user toggles the radio repeatedly within the TTL). The cache is
    // scoped to non-secret request inputs so a base URL change always refetches.
    if (
      openrouterModelsCacheKey === cacheKey &&
      openrouterModelsLoadedAt > 0 &&
      Date.now() - openrouterModelsLoadedAt < OPENROUTER_MODELS_CACHE_TTL_MS
    ) {
      return;
    }
    dispatch({ type: "SET_OPENROUTER_MODELS_LOADING", loading: true });
    try {
      const models = await invoke<import("../../types").OpenRouterModel[]>(
        "list_openrouter_models_cmd",
        { apiKey: openrouterApiKey.trim() || null, baseUrl },
      );
      dispatch({
        type: "SET_OPENROUTER_MODELS",
        models,
        loadedAt: Date.now(),
        cacheKey,
      });
    } catch (e) {
      console.error("Failed to load OpenRouter models:", e);
      setOpenrouterModelsError(errorToMessage(e));
      dispatch({ type: "SET_OPENROUTER_MODELS_LOADING", loading: false });
    }
  };

  // Discover accelerator endpoints for the selected OpenRouter model using ONLY
  // the saved-key catalog commands (no plaintext key readback). Fetches the
  // per-model endpoint list and the provider metadata in parallel, then stashes
  // both raw payloads — the view model + ranking happen in the presentation
  // layer so a partial catalog still renders. (seed 7809)
  const handleDiscoverOpenRouterAccelerators = async () => {
    if (!openrouterCredentialAvailable) return;
    const model = openrouterModel.trim();
    if (!model) return;
    setOpenrouterAcceleratorError(null);
    setOpenrouterAcceleratorLoading(true);
    const baseUrl = normalizeOpenRouterBaseUrl(openrouterBaseUrl);
    try {
      // `list_openrouter_model_endpoints_cmd` is the load-bearing call; provider
      // metadata is best-effort enrichment (policy URLs, datacenters) — a
      // provider-fetch failure must not blank the endpoint table.
      const [endpoints, providers] = await Promise.all([
        invoke<OpenRouterModelEndpoints>(
          "list_openrouter_model_endpoints_cmd",
          { modelId: model, baseUrl },
        ),
        invoke<OpenRouterProvider[]>("list_openrouter_providers_cmd", {
          baseUrl,
        }).catch((e) => {
          console.warn("OpenRouter provider metadata unavailable:", e);
          return [] as OpenRouterProvider[];
        }),
      ]);
      setOpenrouterAcceleratorEndpoints(endpoints);
      setOpenrouterAcceleratorProviders(providers);
    } catch (e) {
      console.error("Failed to discover OpenRouter accelerators:", e);
      setOpenrouterAcceleratorError(errorToMessage(e));
    } finally {
      setOpenrouterAcceleratorLoading(false);
    }
  };

  // Apply a discovered accelerator preset's ranked slug order into the routing
  // policy. We map every dynamic preset onto `strict_accelerator` so the order
  // flows through the existing `buildOpenRouterRoutingPolicy` path (writing
  // `provider.order` + `provider.only`). This is the replacement for the
  // hardcoded `"cerebras, groq"` default — the slugs come from the live catalog.
  const handleApplyAcceleratorPreset = (
    preset: AcceleratorPreset,
    order: string[],
  ) => {
    if (order.length === 0) return;
    dispatch(setField("openrouterRoutingPreset", "strict_accelerator"));
    dispatch(setField("openrouterProviderOrderText", order.join(", ")));
    setOpenrouterAppliedAcceleratorPreset(preset);
  };

  const handleTestCerebras = async () => {
    if (cerebrasTesting) return;
    setCerebrasTesting(true);
    setCerebrasTestResult(null);
    try {
      const msg = await invoke<string>("test_cerebras_connection_cmd", {
        apiKey: llmApiKey.trim() || null,
      });
      setCerebrasTestResult({ ok: true, msg });
    } catch (e) {
      setCerebrasTestResult({ ok: false, msg: errorToMessage(e) });
    } finally {
      setCerebrasTesting(false);
    }
  };

  const handleRefreshCerebrasModels = async () => {
    if (!cerebrasCredentialAvailable) return;
    setCerebrasModelsError(null);
    setCerebrasModelsLoading(true);
    try {
      const models = await invoke<ProviderModelCatalogItem[]>(
        "list_cerebras_models_cmd",
        { apiKey: llmApiKey.trim() || null },
      );
      setCerebrasModels(models);
    } catch (e) {
      console.error("Failed to load Cerebras models:", e);
      setCerebrasModelsError(errorToMessage(e));
    } finally {
      setCerebrasModelsLoading(false);
    }
  };

  const handleTestAwsBedrock = async () => {
    const credential_source = buildAwsCredentialSource(
      awsBedrockCredentialMode,
      awsBedrockProfileName,
      awsBedrockAccessKey,
    );
    return runTest("aws_bedrock", () =>
      invoke<string>("test_aws_credentials", {
        region: awsBedrockRegion,
        credentialSource: credential_source,
        secretAccessKey:
          awsBedrockCredentialMode === "access_keys"
            ? awsBedrockSecretKey.trim() || null
            : null,
        sessionToken:
          awsBedrockCredentialMode === "access_keys"
            ? awsBedrockSessionToken.trim() || null
            : null,
      }),
    );
  };

  /** Render a test result line (green/red) for a given provider key. */
  const renderTestResult = (key: TestKey) => {
    const r = testResults[key];
    if (!r) return null;
    return (
      // A connection-test result appears dynamically after the user clicks
      // "Test" — announce it to screen readers (WCAG 4.1.3 Status Messages)
      // instead of requiring them to tab back and find it. The check/error
      // Icon already gives a non-color cue (WCAG 1.4.1).
      <div
        className={r.ok ? "settings-test-ok" : "settings-test-err"}
        role="status"
        aria-live="polite"
        aria-atomic="true"
      >
        {r.ok ? (
          <Icon name="check" size={14} />
        ) : (
          <Icon name="error" size={14} />
        )}{" "}
        {r.msg}
      </div>
    );
  };

  // Sync local state when settings are loaded
  useEffect(() => {
    if (!settings) return;
    let cancelled = false;

    // Audio capture format — clamp to the UI whitelist so an out-of-band
    // value from a hand-edited settings.json doesn't leave the dropdown
    // in a "Custom (n/a)" state. The backend does the same fallback in
    // `resolve_audio_settings`.
    const ALLOWED_RATES: SampleRate[] = [
      22050, 32000, 44100, 48000, 88200, 96000,
    ];
    const ALLOWED_CHANNELS: ChannelCount[] = [1, 2];
    const sr = settings.audio_settings?.sample_rate;
    const ch = settings.audio_settings?.channels;
    const patch: Partial<SettingsState> = {
      audioSampleRate: ALLOWED_RATES.includes(sr as SampleRate)
        ? (sr as SampleRate)
        : 48000,
      audioChannels: ALLOWED_CHANNELS.includes(ch as ChannelCount)
        ? (ch as ChannelCount)
        : 2,
    };

    // Whisper model selection
    if (settings.whisper_model) {
      patch.whisperModel = settings.whisper_model;
    }

    // ASR provider
    const asr = settings.asr_provider;
    patch.asrType = asr.type;
    if (asr.type === "api") {
      patch.asrEndpoint = asr.endpoint;
      patch.asrApiKey = "";
      patch.asrModel = asr.model;
    } else if (asr.type === "openai_realtime") {
      patch.openaiRealtimeApiKey = "";
      patch.openaiRealtimeModel = asr.model;
      patch.openaiRealtimeLanguage = asr.language ?? "";
    } else if (asr.type === "aws_transcribe") {
      patch.awsAsrRegion = asr.region;
      patch.awsAsrLanguageCode = asr.language_code;
      patch.awsAsrDiarization = asr.enable_diarization;
      const cred = asr.credential_source;
      patch.awsAsrCredentialMode = cred.type;
      if (cred.type === "profile") patch.awsAsrProfileName = cred.name;
      if (cred.type === "access_keys") patch.awsAsrAccessKey = "";
    } else if (asr.type === "deepgram") {
      patch.deepgramApiKey = "";
      patch.deepgramModel = asr.model;
      patch.deepgramDiarization = asr.enable_diarization;
      patch.deepgramEndpointingMs = asr.endpointing_ms ?? 300;
      patch.deepgramUtteranceEndMs = asr.utterance_end_ms ?? 1000;
      patch.deepgramVadEvents = asr.vad_events ?? true;
      patch.deepgramEotThreshold = asr.eot_threshold ?? 0.5;
      patch.deepgramEagerEotThreshold = asr.eager_eot_threshold ?? 0;
      patch.deepgramEotTimeoutMs = asr.eot_timeout_ms ?? 0;
      patch.deepgramMaxSpeakers = asr.max_speakers ?? 2;
    } else if (asr.type === "assemblyai") {
      patch.assemblyaiApiKey = "";
      patch.assemblyaiDiarization = asr.enable_diarization;
    } else if (asr.type === "soniox") {
      patch.sonioxApiKey = "";
      patch.sonioxModel = asr.model;
      patch.sonioxDiarization = asr.enable_diarization;
      patch.sonioxLanguageIdentification =
        asr.enable_language_identification ?? true;
      patch.sonioxLanguageHints = (asr.language_hints ?? []).join(", ");
      patch.sonioxMaxSpeakers = asr.max_speakers ?? 0;
    } else if (asr.type === "sherpa_onnx") {
      patch.sherpaModelDir = asr.model_dir;
      patch.sherpaEndpointDetection = asr.enable_endpoint_detection;
    }

    // LLM provider
    const llm = settings.llm_provider;
    const existingOpenRouterRoutingPolicy =
      settings.openrouter_routing_policy ?? null;
    patch.openrouterRoutingPolicy = existingOpenRouterRoutingPolicy;
    patch.llmType = llm.type;
    if (llm.type === "api") {
      patch.llmType =
        endpointCredentialKey(llm.endpoint) === "cerebras_api_key"
          ? "cerebras"
          : "api";
      patch.llmEndpoint = llm.endpoint;
      patch.llmApiKey = "";
      patch.llmModel =
        llm.model ||
        (endpointCredentialKey(llm.endpoint) === "cerebras_api_key"
          ? defaultModelForProvider("llm.cerebras")
          : "");
    } else if (llm.type === "aws_bedrock") {
      patch.awsBedrockRegion = llm.region;
      patch.awsBedrockModelId = llm.model_id;
      const cred = llm.credential_source;
      patch.awsBedrockCredentialMode = cred.type;
      if (cred.type === "profile") patch.awsBedrockProfileName = cred.name;
      if (cred.type === "access_keys") patch.awsBedrockAccessKey = "";
    } else if (llm.type === "mistralrs") {
      patch.mistralrsModelId = llm.model_id;
    } else if (llm.type === "openrouter") {
      const legacyProviderOrder = llm.provider_order ?? [];
      patch.openrouterModel = llm.model;
      patch.openrouterBaseUrl = llm.base_url;
      patch.openrouterIncludeUsageInStream = llm.include_usage_in_stream;
      patch.openrouterRoutingPreset = inferOpenRouterRoutingPreset(
        existingOpenRouterRoutingPolicy,
        legacyProviderOrder,
      );
      patch.openrouterProviderOrderText =
        openRouterProviderOrderTextForSettings(
          existingOpenRouterRoutingPolicy,
          legacyProviderOrder,
        );
      patch.openrouterApiKey = "";
    }

    // LLM config (advanced — max_tokens / temperature)
    if (settings.llm_api_config) {
      patch.llmMaxTokens = settings.llm_api_config.max_tokens;
      patch.llmTemperature = settings.llm_api_config.temperature;
    }

    // Streaming prefill (local llama.cpp only — ADR-0012). Missing in older
    // settings files → default off.
    patch.streamingPrefill = settings.streaming_prefill ?? false;

    const diarization = settings.diarization;
    patch.diarizationMode = coerceDiarizationMode(diarization?.mode);
    patch.diarizationSpeakerCount = coerceDiarizationSpeakerCount(
      diarization?.speaker_count,
    );
    patch.diarizationMaxSpeakers = Math.max(
      0,
      Math.round(diarization?.max_speakers ?? 0),
    );
    patch.privacyMode = settings.privacy_mode ?? "byok_cloud";

    // Diagnostics: log level — default to "info" if missing or malformed so
    // the dropdown always has a legitimate selection.
    const LOG_LEVELS: LogLevel[] = [
      "off",
      "error",
      "warn",
      "info",
      "debug",
      "trace",
    ];
    const raw = (settings.log_level ?? "info").toLowerCase() as LogLevel;
    patch.logLevel = LOG_LEVELS.includes(raw) ? raw : "info";

    // Gemini settings
    if (settings.gemini) {
      patch.geminiModel = settings.gemini.model;
      const auth = settings.gemini.auth;
      patch.geminiAuthMode = auth.type;
      if (auth.type === "vertex_ai") {
        patch.geminiProjectId = auth.project_id;
        patch.geminiLocation = auth.location;
        patch.geminiServiceAccountPath = auth.service_account_path ?? "";
      }
      // NOTE: we deliberately do NOT seed `geminiApiKey` from `auth.api_key`.
      // The IPC `settings` object is ALWAYS redacted (`skip_serializing` +
      // `redacted_settings`), so `auth.api_key` is the empty string here — and
      // because HYDRATE_FROM_SETTINGS overwrites (`{...state, ...patch}`),
      // including it would blank the field the user just saved (BUG-2: the key
      // is safely stored, but the form went empty after Save). The credential
      // store is the single source of truth for this field; Settings only reads
      // non-secret presence and backend commands resolve the saved value when
      // this field is blank. Same rationale as the ASR/LLM `api_key` fields.
    }

    // TTS hydration — local state, not reducer.
    const tts = settings.tts_provider ?? { type: "none" };
    if (tts.type === "deepgram_aura") {
      setTtsType("deepgram_aura");
      setAuraVoice(tts.voice);
      setAuraSpeed(tts.speed);
    } else {
      setTtsType("none");
    }
    setSpeakAloud(settings.speak_aloud ?? false);

    dispatch({ type: "HYDRATE_FROM_SETTINGS", patch });
    // Establish the dirty baseline from the synchronously-hydrated state.
    // The async credential load below may add more fields and will bump the
    // epoch again once those settle.
    setBaselineEpoch((e) => e + 1);

    // Mirror non-secret credential presence first. Provider readiness and
    // saved-key affordances should come from this path instead of secret
    // readback.
    (async () => {
      try {
        const presence = await invoke<CredentialPresence[]>(
          "load_credential_presence_cmd",
        );
        if (!cancelled) {
          setCredentialPresence(credentialPresenceFromEntries(presence));
        }
      } catch (e) {
        if (!cancelled) {
          setProviderReadinessError(errorToMessage(e));
          setCredentialPresence({});
        }
      }
    })();

    const readinessRequestId = beginProviderReadinessRequest();
    (async () => {
      setProviderReadinessLoading(true);
      try {
        const {
          conversationMode: readinessConversationMode,
          converseEngine: readinessConverseEngine,
        } = useAudioGraphStore.getState();
        const readiness =
          (await invoke<ProviderReadiness[]>("get_provider_readiness_cmd", {
            refresh: true,
            conversationMode: readinessConversationMode,
            converseEngine: readinessConverseEngine,
            requestId: readinessRequestId,
          })) ?? [];
        if (
          cancelled ||
          !isCurrentProviderReadinessRequest(readinessRequestId)
        ) {
          return;
        }
        applyProviderReadiness(
          readiness,
          settings.llm_provider.type === "openrouter"
            ? normalizeOpenRouterBaseUrl(settings.llm_provider.base_url)
            : DEFAULT_OPENROUTER_BASE_URL,
        );
      } catch (e) {
        if (
          !cancelled &&
          isCurrentProviderReadinessRequest(readinessRequestId)
        ) {
          console.error("Failed to load provider readiness:", e);
          setProviderReadinessError(errorToMessage(e));
          setProviderReadiness({});
        }
      } finally {
        if (!cancelled && clearProviderReadinessRequest(readinessRequestId)) {
          setProviderReadinessLoading(false);
        }
      }
    })();

    // Secret inputs are replace-only. Saved credentials are surfaced through
    // load_credential_presence_cmd above and resolved inside backend commands;
    // Settings must not hydrate stored plaintext keys into React state.
    return () => {
      cancelled = true;
      if (providerReadinessRequestRef.current === readinessRequestId) {
        providerReadinessRequestRef.current = null;
        cancelProviderReadinessRequest(readinessRequestId);
      }
    };
  }, [
    settings,
    applyProviderReadiness,
    beginProviderReadinessRequest,
    cancelProviderReadinessRequest,
    clearProviderReadinessRequest,
    isCurrentProviderReadinessRequest,
  ]);

  // Fetch AWS profiles whenever settings load or the user switches an AWS
  // section into "profile" credential mode. Cheap Tauri call — just parses
  // two small files — so it's fine to re-run on mode change.
  // refreshAwsProfiles is recreated every render (not memoized); including it
  // would re-run this effect on every render and spam the Tauri call. We
  // intentionally re-run only when settings load or a credential mode switches.
  // biome-ignore lint/correctness/useExhaustiveDependencies: refreshAwsProfiles is unstable; re-run only on settings/mode change
  useEffect(() => {
    if (!settings) return;
    if (
      awsAsrCredentialMode === "profile" ||
      awsBedrockCredentialMode === "profile"
    ) {
      refreshAwsProfiles();
    }
  }, [settings, awsAsrCredentialMode, awsBedrockCredentialMode]);

  // ── Handlers ──────────────────────────────────────────────────────────
  const handleSave = async () => {
    const asrEndpointCredentialKey = endpointCredentialKey(asrEndpoint);
    await saveCredentialIfPresent(
      asrEndpointCredentialKey,
      asrType === "api" ? asrApiKey : "",
    );
    await saveCredentialIfPresent(
      "openai_api_key",
      asrType === "openai_realtime" ? openaiRealtimeApiKey : "",
    );
    await saveCredentialIfPresent(
      "deepgram_api_key",
      asrType === "deepgram" || ttsType === "deepgram_aura"
        ? deepgramApiKey
        : "",
    );
    await saveCredentialIfPresent(
      "assemblyai_api_key",
      asrType === "assemblyai" ? assemblyaiApiKey : "",
    );
    await saveCredentialIfPresent(
      "soniox_api_key",
      asrType === "soniox" ? sonioxApiKey : "",
    );
    const llmEndpointCredentialKey =
      llmType === "cerebras"
        ? "cerebras_api_key"
        : endpointCredentialKey(llmEndpoint);
    await saveCredentialIfPresent(
      llmEndpointCredentialKey,
      llmType === "api" || llmType === "cerebras" ? llmApiKey : "",
    );
    await saveCredentialIfPresent(
      "openrouter_api_key",
      llmType === "openrouter" ? openrouterApiKey : "",
    );
    await saveCredentialIfPresent(
      "gemini_api_key",
      geminiAuthMode === "api_key" ? geminiApiKey : "",
    );
    await saveCredentialIfPresent(
      "google_service_account_path",
      geminiAuthMode === "vertex_ai" ? geminiServiceAccountPath : "",
    );

    if (
      asrType === "aws_transcribe" &&
      awsAsrCredentialMode === "access_keys"
    ) {
      await saveCredentialIfPresent("aws_access_key", awsAsrAccessKey);
    }
    if (
      llmType === "aws_bedrock" &&
      awsBedrockCredentialMode === "access_keys"
    ) {
      await saveCredentialIfPresent("aws_access_key", awsBedrockAccessKey);
    }

    // Persist AWS secret key + session token before saving settings so the
    // backend runtime cache and readiness probes see a coherent credential set
    // immediately after `save_settings_cmd` reloads the credential store.
    const usingAwsAsrKeys =
      asrType === "aws_transcribe" && awsAsrCredentialMode === "access_keys";
    const usingAwsBedrockKeys =
      llmType === "aws_bedrock" && awsBedrockCredentialMode === "access_keys";

    if (usingAwsAsrKeys || usingAwsBedrockKeys) {
      const secretCandidate =
        (usingAwsAsrKeys && awsAsrSecretKey) ||
        (usingAwsBedrockKeys && awsBedrockSecretKey) ||
        "";
      await saveCredentialIfPresent("aws_secret_key", secretCandidate);

      const sessionCandidate =
        (usingAwsAsrKeys && awsAsrSessionToken) ||
        (usingAwsBedrockKeys && awsBedrockSessionToken) ||
        "";
      await saveCredentialIfPresent("aws_session_token", sessionCandidate);
    }

    let asrProvider: AsrProvider;
    switch (asrType) {
      case "api":
        asrProvider = {
          type: "api",
          endpoint: asrEndpoint,
          api_key: "",
          model: asrModel,
        };
        break;
      case "openai_realtime":
        asrProvider = {
          type: "openai_realtime",
          api_key: "",
          model:
            openaiRealtimeModel.trim() ||
            defaultModelForProvider("asr.openai_realtime"),
          language: openaiRealtimeLanguage.trim() || null,
        };
        break;
      case "aws_transcribe":
        asrProvider = {
          type: "aws_transcribe",
          region: awsAsrRegion,
          language_code: awsAsrLanguageCode,
          credential_source: buildAwsCredentialSource(
            awsAsrCredentialMode,
            awsAsrProfileName,
            "",
          ),
          enable_diarization: providerDiarizationRequested && awsAsrDiarization,
        };
        break;
      case "deepgram":
        asrProvider = {
          type: "deepgram",
          api_key: "",
          model: deepgramModel,
          enable_diarization:
            providerDiarizationRequested && deepgramDiarization,
          endpointing_ms: Math.max(0, Math.round(deepgramEndpointingMs)),
          utterance_end_ms: Math.max(0, Math.round(deepgramUtteranceEndMs)),
          vad_events: deepgramVadEvents,
          eot_threshold: Math.max(0, Math.min(1, deepgramEotThreshold)),
          eager_eot_threshold: Math.max(
            0,
            Math.min(deepgramEotThreshold, deepgramEagerEotThreshold),
          ),
          eot_timeout_ms: Math.max(0, Math.round(deepgramEotTimeoutMs)),
          max_speakers: Math.max(0, Math.round(deepgramMaxSpeakers)),
        };
        break;
      case "assemblyai":
        asrProvider = {
          type: "assemblyai",
          api_key: "",
          enable_diarization:
            providerDiarizationRequested && assemblyaiDiarization,
        };
        break;
      case "soniox":
        asrProvider = {
          type: "soniox",
          api_key: "",
          model: sonioxModel.trim() || defaultModelForProvider("asr.soniox"),
          enable_diarization: providerDiarizationRequested && sonioxDiarization,
          enable_language_identification: sonioxLanguageIdentification,
          language_hints: sonioxLanguageHints
            .split(",")
            .map((hint) => hint.trim())
            .filter(Boolean),
          max_speakers: Math.max(0, Math.round(sonioxMaxSpeakers)),
        };
        break;
      case "sherpa_onnx":
        asrProvider = {
          type: "sherpa_onnx",
          model_dir: sherpaModelDir,
          enable_endpoint_detection: sherpaEndpointDetection,
        };
        break;
      default:
        asrProvider = { type: "local_whisper" };
    }

    const legacyOpenRouterProviderOrder =
      openrouterRoutingPreset === "legacy"
        ? parseOpenRouterProviderList(openrouterProviderOrderText)
        : [];
    let llmProvider: LlmProvider;
    switch (llmType) {
      case "api":
        llmProvider = {
          type: "api",
          endpoint: llmEndpoint,
          api_key: "",
          model: llmModel,
        };
        break;
      case "cerebras":
        llmProvider = {
          type: "api",
          endpoint: CEREBRAS_BASE_URL,
          api_key: "",
          model: llmModel.trim() || defaultModelForProvider("llm.cerebras"),
        };
        break;
      case "aws_bedrock":
        llmProvider = {
          type: "aws_bedrock",
          region: awsBedrockRegion,
          model_id: awsBedrockModelId,
          credential_source: buildAwsCredentialSource(
            awsBedrockCredentialMode,
            awsBedrockProfileName,
            "",
          ),
        };
        break;
      case "openrouter":
        llmProvider = {
          type: "openrouter",
          model: openrouterModel,
          base_url: normalizeOpenRouterBaseUrl(openrouterBaseUrl),
          provider_order:
            legacyOpenRouterProviderOrder.length > 0
              ? legacyOpenRouterProviderOrder
              : null,
          include_usage_in_stream: openrouterIncludeUsageInStream,
          api_key: "",
        };
        break;
      case "mistralrs":
        llmProvider = {
          type: "mistralrs",
          model_id: mistralrsModelId,
        };
        break;
      default:
        llmProvider = { type: "local_llama" };
    }

    const llmConfig: LlmApiConfig | null =
      llmType === "cerebras" || (llmType === "api" && llmEndpoint.trim())
        ? {
            endpoint: llmType === "cerebras" ? CEREBRAS_BASE_URL : llmEndpoint,
            api_key: null,
            model:
              llmType === "cerebras"
                ? llmModel.trim() || defaultModelForProvider("llm.cerebras")
                : llmModel,
            max_tokens: llmMaxTokens,
            temperature: llmTemperature,
          }
        : null;
    const nextOpenRouterRoutingPolicy =
      llmType === "openrouter"
        ? buildOpenRouterRoutingPolicy(
            openrouterRoutingPreset,
            openrouterProviderOrderText,
            openrouterRoutingPolicy,
          )
        : (settings?.openrouter_routing_policy ?? null);
    const geminiAuth: GeminiAuthMode =
      geminiAuthMode === "vertex_ai"
        ? {
            type: "vertex_ai",
            project_id: geminiProjectId,
            location: geminiLocation,
            ...(geminiServiceAccountPath
              ? { service_account_path: geminiServiceAccountPath }
              : {}),
          }
        : { type: "api_key", api_key: "" };

    const gemini: GeminiSettingsType = {
      auth: geminiAuth,
      model: geminiModel,
    };

    await saveSettings({
      asr_provider: asrProvider,
      whisper_model: whisperModel,
      llm_provider: llmProvider,
      openrouter_routing_policy: nextOpenRouterRoutingPolicy,
      llm_api_config: llmConfig,
      audio_settings: {
        sample_rate: audioSampleRate,
        channels: audioChannels,
      },
      gemini,
      diarization: {
        mode: diarizationMode,
        speaker_count: diarizationSpeakerCount,
        max_speakers:
          diarizationSpeakerCount === "fixed"
            ? Math.max(1, Math.round(diarizationMaxSpeakers || 1))
            : null,
      },
      privacy_mode: privacyMode,
      log_level: logLevel,
      // TTS provider is built from local state — the user picks it through the
      // Settings TTS section (Wave C / ADR-0006). `none` disables speak-aloud.
      tts_provider:
        ttsType === "deepgram_aura"
          ? {
              type: "deepgram_aura",
              voice: auraVoice,
              sample_rate: 24_000,
              speed: auraSpeed,
            }
          : { type: "none" },
      speak_aloud: speakAloud,
      // Streaming/incremental prefill (ADR-0012). Persisted regardless of the
      // active backend; only honored by supporting local backends. The toggle
      // is gated to local_llama in the UI, but we pass the stored value through
      // so switching providers doesn't silently drop the user's choice.
      streaming_prefill: streamingPrefill,
      // Preserve the stored demo-mode decision across a Settings save.
      // The settings page itself has no UI for this field; dropping it
      // would regress to `undefined` and cause the backend to re-run the
      // first-launch decision on next boot.
      demo_mode: settings?.demo_mode,
    });

    await refreshCredentialPresence();
    void refreshProviderReadiness();

    // Persisted successfully: the current draft is now the saved baseline, so
    // clear the dirty flag and surface a success toast (ADR-0011). Closing
    // behaviour is unchanged — Save does not itself close the modal.
    baselineRef.current = settingsFingerprint(state, {
      ttsType,
      auraVoice,
      auraSpeed,
      speakAloud,
    });
    setConfirmingClose(false);
    notify({ severity: "success", message: t("settings.saved") });
  };

  // Centralised close gate (W3.5): when the draft has unsaved edits, intercept
  // the close attempt and reveal the inline confirm bar instead of discarding
  // silently. When clean, close immediately as before. Returns true when the
  // close was actually performed (used by the Escape capture handler to decide
  // whether to swallow the event).
  const requestClose = (): boolean => {
    if (dirty) {
      setConfirmingClose(true);
      return false;
    }
    closeSettings();
    return true;
  };

  const handleDiscardAndClose = () => {
    setConfirmingClose(false);
    closeSettings();
  };

  // Intercept Escape at the capture phase so we can show the confirm bar
  // before the App-level `useKeyboardShortcuts` handler reaches the store's
  // `closeSettings`. Only swallow the event when there are unsaved edits (or
  // the confirm bar is already open); otherwise let the global handler close
  // the modal as it always has.
  useEffect(() => {
    const onKeyDownCapture = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      if (confirmingClose) {
        // Escape while confirming = "Keep editing".
        e.preventDefault();
        e.stopImmediatePropagation();
        setConfirmingClose(false);
        return;
      }
      if (dirty) {
        e.preventDefault();
        e.stopImmediatePropagation();
        setConfirmingClose(true);
      }
    };
    window.addEventListener("keydown", onKeyDownCapture, true);
    return () => window.removeEventListener("keydown", onKeyDownCapture, true);
  }, [dirty, confirmingClose]);

  const handleDeleteClick = (filename: string) => {
    if (confirmDelete === filename) {
      deleteModel(filename);
      dispatch({ type: "SET_CONFIRM_DELETE", filename: null });
    } else {
      dispatch({ type: "SET_CONFIRM_DELETE", filename });
    }
  };

  return {
    OPENROUTER_MODELS_CACHE_TTL_MS,
    RAIL_GROUP_LABEL_KEYS,
    RAIL_GROUP_ORDER,
    SETTINGS_TABS,
    TEST_TIMEOUT_MS,
    activeAsrProviderDescriptor,
    activeAsrProviderId,
    activeAsrProviderReadiness,
    activeLlmProviderDescriptor,
    activeLlmProviderId,
    activeLlmProviderReadiness,
    activeOpenAiCredentialRoute,
    activeProviderCredentialRouteForKey,
    activeReadinessProviderIdSet,
    activeReadinessProviderIds,
    activeTab,
    activeTtsProviderDescriptor,
    activeTtsProviderId,
    activeTtsProviderReadiness,
    applyProviderReadiness,
    asrApiKey,
    asrApiModelCatalog,
    asrEndpoint,
    asrEndpointSavedKeyPresent,
    asrModel,
    asrType,
    assemblyaiApiKey,
    assemblyaiCredentialAvailable,
    assemblyaiDiarization,
    assemblyaiSavedKeyPresent,
    audioChannels,
    audioSampleRate,
    audioSources,
    auraSpeed,
    auraVoice,
    auraVoiceCatalog,
    awsAccessKeySavedPresent,
    awsAsrAccessKey,
    awsAsrAccessKeysAvailable,
    awsAsrCredentialMode,
    awsAsrDiarization,
    awsAsrLanguageCode,
    awsAsrProfileName,
    awsAsrRegion,
    awsAsrSecretKey,
    awsAsrSessionToken,
    awsBedrockAccessKey,
    awsBedrockAccessKeysAvailable,
    awsBedrockCredentialMode,
    awsBedrockModelId,
    awsBedrockProfileName,
    awsBedrockRegion,
    awsBedrockSecretKey,
    awsBedrockSessionToken,
    awsSavedKeysPresent,
    awsSecretKeySavedPresent,
    awsSessionTokenSavedPresent,
    baselineEpoch,
    baselineRef,
    beginProviderReadinessRequest,
    cancelProviderReadinessRequest,
    cerebrasCredentialAvailable,
    cerebrasModelCatalog,
    cerebrasModels,
    cerebrasModelsError,
    cerebrasModelsLoading,
    cerebrasReadinessModelCatalog,
    cerebrasSavedKeyPresent,
    cerebrasTestResult,
    cerebrasTesting,
    clearProviderReadinessRequest,
    closeSettings,
    confirmDelete,
    confirmingClose,
    conversationMode,
    converseEngine,
    credentialPresence,
    credentialRouteForKey,
    credentialRouteForProviderCredential,
    credentialRouteForProviderSetupSelection,
    credentialRouteForReadiness,
    deepgramApiKey,
    deepgramCredentialAvailable,
    deepgramDiarization,
    deepgramEagerEotThreshold,
    deepgramEndpointingMs,
    deepgramEotThreshold,
    deepgramEotTimeoutMs,
    deepgramMaxSpeakers,
    deepgramModel,
    deepgramModelCatalog,
    deepgramSavedKeyPresent,
    deepgramUtteranceEndMs,
    deepgramVadEvents,
    deleteModel,
    diarizationMaxSpeakers,
    diarizationMode,
    diarizationSpeakerCount,
    dirty,
    dispatch,
    downloadModel,
    downloadProgress,
    fallbackCredentialRouteForKey,
    fingerprint,
    firstProviderSetupRoute,
    focusSettingsField,
    geminiApiKey,
    geminiAuthMode,
    geminiCredentialAvailable,
    geminiLocation,
    geminiModel,
    geminiModelCatalog,
    geminiProjectId,
    geminiProviderDescriptor,
    geminiProviderId,
    geminiProviderReadiness,
    geminiSavedKeyPresent,
    geminiServiceAccountPath,
    geminiServiceAccountPathSavedPresent,
    handleApplyAcceleratorPreset,
    handleClearCredential,
    handleDeleteClick,
    handleDiscardAndClose,
    handleDiscoverOpenRouterAccelerators,
    handleNativeRealtimeToggle,
    handleOpenCredentialKey,
    handleOpenCredentialRoute,
    handleProviderSetupSourceRecovery,
    handleSelectProductMode,
    handleRefreshCerebrasModels,
    handleRefreshOpenRouterModels,
    handleSave,
    handleSettingsTabKeyDown,
    handleTestAsrApi,
    handleTestAssemblyAI,
    handleTestAwsAsr,
    handleTestAwsBedrock,
    handleTestCerebras,
    handleTestDeepgram,
    handleTestGemini,
    handleTestOpenRouter,
    handleTestTts,
    i18n,
    isCurrentProviderReadinessRequest,
    isDeletingModel,
    isDownloading,
    latestValidationForCredential,
    listAwsProfiles,
    llmApiKey,
    llmApiModelCatalog,
    llmEndpoint,
    llmEndpointSavedKeyPresent,
    llmMaxTokens,
    llmModel,
    llmTemperature,
    llmType,
    localDiarizationReady,
    logLevel,
    mistralrsModelCatalog,
    mistralrsModelId,
    modalRef,
    modelRouteForProviderId,
    modelStatus,
    models,
    nativeRealtimeSelected,
    notify,
    openCredentialRoute,
    openSettingsControlRoute,
    openaiRealtimeApiKey,
    openaiRealtimeLanguage,
    openaiRealtimeModel,
    openaiRealtimeModelCatalog,
    openaiSavedKeyPresent,
    openrouterAcceleratorEndpoints,
    openrouterAcceleratorError,
    openrouterAcceleratorLoading,
    openrouterAcceleratorPreset,
    openrouterAcceleratorProviders,
    openrouterApiKey,
    openrouterAppliedAcceleratorPreset,
    openrouterBaseUrl,
    openrouterCredentialAvailable,
    openrouterIncludeUsageInStream,
    openrouterModel,
    openrouterModelsCacheKey,
    openrouterModelsError,
    openrouterModelsLoadedAt,
    openrouterProviderOrderText,
    openrouterRoutingPolicy,
    openrouterRoutingPreset,
    openrouterSavedKeyPresent,
    privacyMode,
    providerDiarizationRequested,
    providerDiarizationSupported,
    providerLabelsForCredential,
    providerReadiness,
    providerReadinessEntries,
    providerReadinessError,
    providerReadinessLoading,
    providerReadinessRequestRef,
    providerReadinessRequestSeqRef,
    providerReadinessStatusEntries,
    providerReadinessStatusSummary,
    providerRouteForProviderId,
    providerSetupCredentialRoute,
    providerSetupModeCards,
    providerSetupModelRoute,
    providerSetupProviderRoute,
    providerSetupSelectionForBlocker,
    railHorizontal,
    readinessOpenAiCredentialRoute,
    refreshAwsProfiles,
    refreshCredentialPresence,
    refreshProviderReadiness,
    relatedReadinessForCredential,
    renderTestResult,
    requestClose,
    requestSourceRecovery,
    runTest,
    saveSettings,
    savedCredentialEntries,
    selectSettingsTab,
    selectedDiarizationModeUnavailable,
    selectedModelForProvider,
    selectedSourceIds,
    setActiveTab,
    setAuraSpeed,
    setAuraVoice,
    setConfirmingClose,
    setField,
    setOpenrouterAcceleratorPreset,
    setSpeakAloud,
    setTheme,
    setTtsType,
    settings,
    settingsLoading,
    sherpaEndpointDetection,
    sherpaModelCatalog,
    sherpaModelDir,
    sonioxApiKey,
    sonioxDiarization,
    sonioxLanguageHints,
    sonioxLanguageIdentification,
    sonioxMaxSpeakers,
    sonioxModel,
    speakAloud,
    state,
    streamingPrefill,
    t,
    tabButtonId,
    tabPanelId,
    tabRefs,
    testResults,
    testingKey,
    testingTts,
    theme,
    ttsLocal,
    ttsTestResult,
    ttsType,
    visibleProviderReadiness,
    whisperModel,
  };
}

export type SettingsControllerValue = ReturnType<typeof useSettingsController>;
