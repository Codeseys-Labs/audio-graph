import type {
  AudioPermissionRecoveryHint,
  AudioPermissionStatus,
  AudioSourceCapabilities,
  AudioSourceType,
  CredentialPresence,
  ProviderDataBoundary,
  ProviderDescriptor,
  ProviderReadiness,
  ProviderReadinessStatus,
  ProviderStage,
  SourceRecoveryIssue,
} from "../types";
import { sourceCaptureTargetId } from "../utils/captureTarget";
import {
  defaultModelForProvider,
  PROVIDER_DESCRIPTORS,
  providerIsDeferred,
} from "./providerRegistryHelpers";
import { endpointCredentialKey, type SettingsState } from "./settingsTypes";

export type ProviderSetupModeId =
  | "local_private"
  | "cloud_fast"
  | "hybrid"
  | "native_realtime";

export type ProviderSetupProductPath =
  | "durable_notes_graph"
  | "native_realtime_agent";

export type ProviderSetupStagePath =
  | "durable_pipeline"
  | "native_realtime_agent"
  | "speech_output";

export type ProviderSetupStageRole =
  | "durable_transcription"
  | "durable_notes_graph"
  | "speech_output"
  | "native_realtime_agent";

export type ProviderSetupDataBoundary =
  | ProviderDataBoundary
  | "mixed_local_cloud"
  | "mixed_cloud"
  | "not_applicable";

export type ProviderSetupReadinessStatus =
  | "ready"
  | "missing_credentials"
  | "blocked"
  | "error"
  | "unchecked";

export type ProviderSetupBlockerKind =
  | "missing_credential"
  | "missing_config"
  | "model_unselected"
  | "provider_planned"
  | "provider_deferred"
  | "provider_error"
  | "missing_feature"
  | "missing_model"
  | "runtime_unavailable"
  | "load_failed"
  | "source_unselected"
  | "source_unavailable"
  | "source_permission_unavailable"
  | "source_unsupported"
  | "source_policy_conflict";

export interface ProviderSetupBlocker {
  kind: ProviderSetupBlockerKind;
  providerId: string;
  stage: ProviderStage;
  message: string;
  key?: string;
  model?: string;
  feature?: string;
  sourceId?: string;
  sourceName?: string;
  permissionStatus?: AudioPermissionStatus;
  permissionRecovery?: AudioPermissionRecoveryHint;
}

export interface ProviderSetupCredentialStatus {
  key: string;
  present: boolean;
  source: string;
}

export interface ProviderSetupProviderSelection {
  providerId: string;
  providerName: string;
  settingsVariant: string;
  stage: ProviderStage;
  path: ProviderSetupStagePath;
  role: ProviderSetupStageRole;
  transport: ProviderDescriptor["transport"];
  model: string | null;
  status: ProviderDescriptor["status"];
  dataBoundary: ProviderDataBoundary;
  dataLeavesDevice: boolean;
  credentials: ProviderSetupCredentialStatus[];
  readinessStatus: ProviderSetupReadinessStatus;
  readinessMessage: string | null;
  blockers: ProviderSetupBlocker[];
  supportsStreaming: boolean;
  supportsPartialRevisions: boolean;
  supportsDiarization: boolean;
}

export interface ProviderSetupStageCoverage {
  stage: ProviderStage;
  path: ProviderSetupStagePath;
  role: ProviderSetupStageRole;
  covered: boolean;
  providerId: string;
  providerName: string;
  model: string | null;
  readinessStatus: ProviderSetupReadinessStatus;
  dataBoundary: ProviderDataBoundary;
}

export interface ProviderSetupModeCard {
  id: ProviderSetupModeId;
  label: string;
  description: string;
  productPath: ProviderSetupProductPath;
  selected: boolean;
  selectedProviders: ProviderSetupProviderSelection[];
  stageCoverage: ProviderSetupStageCoverage[];
  dataBoundary: ProviderSetupDataBoundary;
  dataLeavesDevice: boolean;
  readinessStatus: ProviderSetupReadinessStatus;
  missingBlockers: ProviderSetupBlocker[];
}

export type CredentialPresenceInput =
  | readonly CredentialPresence[]
  | Partial<Record<string, CredentialPresence | boolean>>;

export type ProviderReadinessInput =
  | readonly ProviderReadiness[]
  | Partial<Record<string, ProviderReadiness>>;

export interface ProviderSetupTtsState {
  ttsType?: "none" | "deepgram_aura";
  auraVoice?: string;
  speakAloud?: boolean;
}

export interface ProviderSetupAudioSource {
  id: string;
  name: string;
  source_type: AudioSourceType;
  capture_target?: string | null;
  capabilities?: AudioSourceCapabilities | null;
  permission_status?: AudioPermissionStatus | null;
  permission_recovery?: AudioPermissionRecoveryHint | null;
}

export interface ProviderSetupSourceState {
  sources: readonly ProviderSetupAudioSource[];
  selectedSourceIds: readonly string[];
}

export interface ProviderSetupModesInput {
  settings: SettingsState;
  credentialPresence?: CredentialPresenceInput;
  providerReadiness?: ProviderReadinessInput;
  registry?:
    | readonly ProviderDescriptor[]
    | ReadonlyMap<string, ProviderDescriptor>;
  tts?: ProviderSetupTtsState;
  conversationMode?: "notes" | "converse";
  converseEngine?: "pipelined" | "native";
  nativeRealtimeEnabled?: boolean;
  sourceState?: ProviderSetupSourceState;
}

interface RegistryLookup {
  all: readonly ProviderDescriptor[];
  byId: ReadonlyMap<string, ProviderDescriptor>;
}

interface SelectionContext {
  settings: SettingsState;
  registry: RegistryLookup;
  credentialPresence: ReadonlyMap<string, CredentialPresence>;
  providerReadiness: ReadonlyMap<string, ProviderReadiness>;
  tts: Required<ProviderSetupTtsState>;
  sourceState: NormalizedProviderSetupSourceState | null;
}

interface CredentialPlan {
  keys: readonly string[];
  typedPresentKeys: ReadonlySet<string>;
  configBlockers: readonly ProviderSetupBlocker[];
}

interface NormalizedProviderSetupSourceState {
  selectedSourceIds: readonly string[];
  selectedSources: readonly ProviderSetupAudioSource[];
  unavailableSourceIds: readonly string[];
}

interface SourceCapabilityRequirement {
  key: keyof Pick<
    AudioSourceCapabilities,
    | "supports_system_capture"
    | "supports_application_capture"
    | "supports_process_tree_capture"
    | "supports_device_selection"
  >;
  label: string;
}

const MODE_COPY: Record<
  ProviderSetupModeId,
  { label: string; description: string; productPath: ProviderSetupProductPath }
> = {
  local_private: {
    label: "Local private",
    description:
      "Durable notes and temporal graph stay on local ASR and local LLM providers.",
    productPath: "durable_notes_graph",
  },
  cloud_fast: {
    label: "Cloud fast",
    description:
      "Durable notes and temporal graph use cloud ASR and cloud LLM providers.",
    productPath: "durable_notes_graph",
  },
  hybrid: {
    label: "Hybrid",
    description:
      "Durable notes and temporal graph mix local and cloud providers.",
    productPath: "durable_notes_graph",
  },
  native_realtime: {
    label: "Native realtime",
    description:
      "Realtime agent audio/text runs through a native realtime provider.",
    productPath: "native_realtime_agent",
  },
};

const LOCAL_ASR_PROVIDER_PRIORITY = [
  "asr.local_whisper",
  "asr.sherpa_onnx",
  "asr.moonshine",
] as const;

const CLOUD_ASR_PROVIDER_PRIORITY = [
  "asr.deepgram",
  "asr.openai_realtime",
  "asr.assemblyai",
  "asr.api",
  "asr.aws_transcribe",
] as const;

const LOCAL_LLM_PROVIDER_PRIORITY = [
  "llm.local_llama",
  "llm.mistralrs",
] as const;

const CLOUD_LLM_PROVIDER_PRIORITY = [
  "llm.openrouter",
  "llm.cerebras",
  "llm.api",
  "llm.aws_bedrock",
] as const;

export function deriveProviderSetupModeCards(
  input: ProviderSetupModesInput,
): ProviderSetupModeCard[] {
  const context = buildSelectionContext(input);
  const selectedMode = selectedModeId(input, context);

  return [
    buildModeCard("local_private", selectedMode, [
      selectProvider(
        pickLocalAsrProviderId(context),
        "durable_pipeline",
        "durable_transcription",
        context,
      ),
      selectProvider(
        pickLocalLlmProviderId(context),
        "durable_pipeline",
        "durable_notes_graph",
        context,
      ),
      selectProvider("tts.none", "speech_output", "speech_output", context),
    ]),
    buildModeCard("cloud_fast", selectedMode, [
      selectProvider(
        pickCloudAsrProviderId(context),
        "durable_pipeline",
        "durable_transcription",
        context,
      ),
      selectProvider(
        pickCloudLlmProviderId(context),
        "durable_pipeline",
        "durable_notes_graph",
        context,
      ),
      selectProvider(
        currentTtsProviderId(context),
        "speech_output",
        "speech_output",
        context,
      ),
    ]),
    buildModeCard("hybrid", selectedMode, [
      selectProvider(
        pickHybridAsrProviderId(context),
        "durable_pipeline",
        "durable_transcription",
        context,
      ),
      selectProvider(
        pickHybridLlmProviderId(context),
        "durable_pipeline",
        "durable_notes_graph",
        context,
      ),
      selectProvider(
        currentTtsProviderId(context),
        "speech_output",
        "speech_output",
        context,
      ),
    ]),
    buildModeCard("native_realtime", selectedMode, [
      selectProvider(
        "realtime_agent.gemini_live",
        "native_realtime_agent",
        "native_realtime_agent",
        context,
      ),
    ]),
  ];
}

export function providerSetupModeCardById(
  input: ProviderSetupModesInput,
): ReadonlyMap<ProviderSetupModeId, ProviderSetupModeCard> {
  return new Map(
    deriveProviderSetupModeCards(input).map((card) => [card.id, card]),
  );
}

export function providerSetupSourceRecoveryIssues(
  card: ProviderSetupModeCard,
): SourceRecoveryIssue[] {
  return card.missingBlockers.flatMap<SourceRecoveryIssue>((blocker) => {
    switch (blocker.kind) {
      case "source_unselected":
        return [
          {
            kind: "unselected",
            message: blocker.message,
          },
        ];
      case "source_unavailable":
        return [
          {
            kind: "unavailable",
            sourceId: blocker.sourceId,
            sourceName: blocker.sourceName,
            message: blocker.message,
          },
        ];
      case "source_unsupported":
        return [
          {
            kind: "unsupported",
            sourceId: blocker.sourceId,
            sourceName: blocker.sourceName,
            message: blocker.message,
          },
        ];
      case "source_permission_unavailable":
        return [
          {
            kind: "permission",
            sourceId: blocker.sourceId,
            sourceName: blocker.sourceName,
            permissionStatus: blocker.permissionStatus,
            permissionRecovery: blocker.permissionRecovery,
            message: blocker.message,
          },
        ];
      case "source_policy_conflict":
        return [
          {
            kind: "policy_conflict",
            sourceId: blocker.sourceId,
            sourceName: blocker.sourceName,
            message: blocker.message,
          },
        ];
      default:
        return [];
    }
  });
}

function buildSelectionContext(
  input: ProviderSetupModesInput,
): SelectionContext {
  return {
    settings: input.settings,
    registry: normalizeRegistry(input.registry),
    credentialPresence: normalizeCredentialPresence(input.credentialPresence),
    providerReadiness: normalizeProviderReadiness(input.providerReadiness),
    tts: {
      ttsType: input.tts?.ttsType ?? "none",
      auraVoice:
        input.tts?.auraVoice ?? defaultModelForProvider("tts.deepgram_aura"),
      speakAloud: input.tts?.speakAloud ?? false,
    },
    sourceState: normalizeSourceState(input.sourceState),
  };
}

function normalizeRegistry(
  registry: ProviderSetupModesInput["registry"],
): RegistryLookup {
  if (!registry) {
    return {
      all: Array.from(PROVIDER_DESCRIPTORS.values()),
      byId: PROVIDER_DESCRIPTORS,
    };
  }

  if (isProviderRegistryMap(registry)) {
    return {
      all: Array.from(registry.values()),
      byId: registry,
    };
  }

  return {
    all: registry,
    byId: new Map(registry.map((provider) => [provider.id, provider])),
  };
}

function isProviderRegistryMap(
  registry: NonNullable<ProviderSetupModesInput["registry"]>,
): registry is ReadonlyMap<string, ProviderDescriptor> {
  return (
    typeof (registry as ReadonlyMap<string, ProviderDescriptor>).get ===
      "function" &&
    typeof (registry as ReadonlyMap<string, ProviderDescriptor>).values ===
      "function"
  );
}

function normalizeCredentialPresence(
  input: CredentialPresenceInput | undefined,
): ReadonlyMap<string, CredentialPresence> {
  if (!input) return new Map();

  if (Array.isArray(input)) {
    return new Map(input.map((entry) => [entry.key, entry]));
  }

  return new Map(
    Object.entries(input).flatMap(([key, value]) => {
      if (value === undefined) return [];
      if (typeof value === "boolean") {
        return [
          [
            key,
            {
              key,
              present: value,
              source: value ? "" : "missing",
            },
          ],
        ];
      }
      return [[key, value]];
    }),
  );
}

function normalizeProviderReadiness(
  input: ProviderReadinessInput | undefined,
): ReadonlyMap<string, ProviderReadiness> {
  if (!input) return new Map();

  if (Array.isArray(input)) {
    return new Map(input.map((entry) => [entry.provider_id, entry]));
  }

  return new Map(
    Object.entries(input).flatMap(([providerId, readiness]) =>
      readiness ? [[providerId, readiness]] : [],
    ),
  );
}

function normalizeSourceState(
  input: ProviderSetupSourceState | undefined,
): NormalizedProviderSetupSourceState | null {
  if (!input) return null;

  const sourcesBySelectionId = new Map<string, ProviderSetupAudioSource>();
  for (const source of input.sources) {
    sourcesBySelectionId.set(source.id, source);
    sourcesBySelectionId.set(sourceCaptureTargetId(source), source);
  }

  const selectedSources: ProviderSetupAudioSource[] = [];
  const unavailableSourceIds: string[] = [];
  for (const sourceId of input.selectedSourceIds) {
    const source = sourcesBySelectionId.get(sourceId);
    if (source) {
      selectedSources.push(source);
    } else {
      unavailableSourceIds.push(sourceId);
    }
  }

  return {
    selectedSourceIds: input.selectedSourceIds,
    selectedSources,
    unavailableSourceIds,
  };
}

function buildModeCard(
  id: ProviderSetupModeId,
  selectedMode: ProviderSetupModeId,
  selections: (ProviderSetupProviderSelection | null)[],
): ProviderSetupModeCard {
  const selectedProviders = selections.filter(
    (selection): selection is ProviderSetupProviderSelection =>
      selection !== null,
  );
  const missingBlockers = selectedProviders.flatMap(
    (selection) => selection.blockers,
  );
  const dataBoundary = aggregateDataBoundary(selectedProviders);
  const copy = MODE_COPY[id];

  return {
    id,
    label: copy.label,
    description: copy.description,
    productPath: copy.productPath,
    selected: selectedMode === id,
    selectedProviders,
    stageCoverage: selectedProviders.map(stageCoverageForSelection),
    dataBoundary,
    dataLeavesDevice: selectedProviders.some(
      (selection) => selection.dataLeavesDevice,
    ),
    readinessStatus: aggregateReadiness(selectedProviders),
    missingBlockers,
  };
}

function selectProvider(
  providerId: string,
  path: ProviderSetupStagePath,
  role: ProviderSetupStageRole,
  context: SelectionContext,
): ProviderSetupProviderSelection | null {
  const descriptor = context.registry.byId.get(providerId);
  if (!descriptor) return null;

  const credentialPlan = credentialPlanForProvider(descriptor, context);
  const readiness = context.providerReadiness.get(providerId) ?? null;
  const credentials = credentialPlan.keys.map((key) =>
    credentialStatus(key, credentialPlan.typedPresentKeys, context, readiness),
  );
  const model = selectedModelForProvider(providerId, context);
  const blockers = providerBlockers(
    descriptor,
    path,
    role,
    model,
    credentials,
    credentialPlan,
    readiness,
    context,
  );

  return {
    providerId,
    providerName: descriptor.display_name,
    settingsVariant: descriptor.settings_variant,
    stage: descriptor.stage,
    path,
    role,
    transport: descriptor.transport,
    model,
    status: descriptor.status,
    dataBoundary: descriptor.privacy.data_boundary,
    dataLeavesDevice: descriptor.privacy.data_leaves_device,
    credentials,
    readinessStatus: providerReadinessStatus(descriptor, blockers, readiness),
    readinessMessage: readiness?.message ?? null,
    blockers,
    supportsStreaming: descriptor.supports_streaming,
    supportsPartialRevisions: descriptor.supports_partial_revisions,
    supportsDiarization: descriptor.supports_diarization,
  };
}

function credentialStatus(
  key: string,
  typedPresentKeys: ReadonlySet<string>,
  context: SelectionContext,
  readiness: ProviderReadiness | null,
): ProviderSetupCredentialStatus {
  if (typedPresentKeys.has(key)) {
    return { key, present: true, source: "draft_settings" };
  }

  const presence = context.credentialPresence.get(key);
  if (presence) {
    return { key, present: presence.present, source: presence.source };
  }

  const readinessCredential = readiness?.credentials.find(
    (credential) => credential.key === key,
  );
  if (readinessCredential) {
    return {
      key,
      present: readinessCredential.present,
      source: readinessCredential.present ? "" : "missing",
    };
  }

  return { key, present: false, source: "missing" };
}

function credentialPlanForProvider(
  descriptor: ProviderDescriptor,
  context: SelectionContext,
): CredentialPlan {
  const { settings } = context;

  switch (descriptor.id) {
    case "asr.api":
      return singleCredentialPlan(
        endpointCredentialKey(settings.asrEndpoint),
        settings.asrApiKey,
      );
    case "asr.aws_transcribe":
      return awsCredentialPlan(
        descriptor,
        settings.awsAsrCredentialMode,
        settings.awsAsrProfileName,
        settings.awsAsrAccessKey,
        settings.awsAsrSecretKey,
      );
    case "asr.deepgram":
    case "tts.deepgram_aura":
      return singleCredentialPlan("deepgram_api_key", settings.deepgramApiKey);
    case "asr.assemblyai":
      return singleCredentialPlan(
        "assemblyai_api_key",
        settings.assemblyaiApiKey,
      );
    case "asr.openai_realtime":
    case "realtime_agent.openai_realtime":
      return singleCredentialPlan(
        "openai_api_key",
        settings.openaiRealtimeApiKey,
      );
    case "llm.api":
      return singleCredentialPlan(
        endpointCredentialKey(settings.llmEndpoint),
        settings.llmApiKey,
      );
    case "llm.cerebras":
      return singleCredentialPlan("cerebras_api_key", settings.llmApiKey);
    case "llm.openrouter":
      return singleCredentialPlan(
        "openrouter_api_key",
        settings.openrouterApiKey,
      );
    case "llm.aws_bedrock":
      return awsCredentialPlan(
        descriptor,
        settings.awsBedrockCredentialMode,
        settings.awsBedrockProfileName,
        settings.awsBedrockAccessKey,
        settings.awsBedrockSecretKey,
      );
    case "realtime_agent.gemini_live":
      return geminiCredentialPlan(descriptor, settings);
    default:
      return {
        keys: descriptor.credential_keys,
        typedPresentKeys: new Set(),
        configBlockers: [],
      };
  }
}

function singleCredentialPlan(key: string, typedValue: string): CredentialPlan {
  return {
    keys: [key],
    typedPresentKeys: new Set(trimmed(typedValue) ? [key] : []),
    configBlockers: [],
  };
}

function awsCredentialPlan(
  descriptor: ProviderDescriptor,
  mode: SettingsState["awsAsrCredentialMode"],
  profileName: string,
  accessKey: string,
  secretKey: string,
): CredentialPlan {
  if (mode === "access_keys") {
    return {
      keys: ["aws_access_key", "aws_secret_key"],
      typedPresentKeys: new Set([
        ...(trimmed(accessKey) ? ["aws_access_key"] : []),
        ...(trimmed(secretKey) ? ["aws_secret_key"] : []),
      ]),
      configBlockers: [],
    };
  }

  if (mode === "profile" && !trimmed(profileName)) {
    return {
      keys: [],
      typedPresentKeys: new Set(),
      configBlockers: [
        {
          kind: "missing_config",
          providerId: descriptor.id,
          stage: descriptor.stage,
          message: "AWS profile name is required for profile credentials.",
        },
      ],
    };
  }

  return { keys: [], typedPresentKeys: new Set(), configBlockers: [] };
}

function geminiCredentialPlan(
  descriptor: ProviderDescriptor,
  settings: SettingsState,
): CredentialPlan {
  if (settings.geminiAuthMode === "vertex_ai") {
    const configBlockers: ProviderSetupBlocker[] = [];

    if (!trimmed(settings.geminiProjectId)) {
      configBlockers.push({
        kind: "missing_config",
        providerId: descriptor.id,
        stage: descriptor.stage,
        message: "Vertex AI project ID is required.",
      });
    }
    if (!trimmed(settings.geminiLocation)) {
      configBlockers.push({
        kind: "missing_config",
        providerId: descriptor.id,
        stage: descriptor.stage,
        message: "Vertex AI location is required.",
      });
    }

    return {
      keys: ["google_service_account_path"],
      typedPresentKeys: new Set(
        trimmed(settings.geminiServiceAccountPath)
          ? ["google_service_account_path"]
          : [],
      ),
      configBlockers,
    };
  }

  return singleCredentialPlan("gemini_api_key", settings.geminiApiKey);
}

function providerBlockers(
  descriptor: ProviderDescriptor,
  path: ProviderSetupStagePath,
  role: ProviderSetupStageRole,
  model: string | null,
  credentials: readonly ProviderSetupCredentialStatus[],
  credentialPlan: CredentialPlan,
  readiness: ProviderReadiness | null,
  context: SelectionContext,
): ProviderSetupBlocker[] {
  const blockers: ProviderSetupBlocker[] = [...credentialPlan.configBlockers];

  if (descriptor.status === "planned") {
    blockers.push({
      kind: "provider_planned",
      providerId: descriptor.id,
      stage: descriptor.stage,
      message: `${descriptor.display_name} is planned but not implemented.`,
    });
  } else if (providerIsDeferred(descriptor)) {
    // Deferred-but-implemented (MVP scoping, audio-graph-ad56): the runtime
    // exists, so this is not a "planned" gap — surface it as a distinct
    // deferred blocker so a saved session pointing here reads honestly and the
    // user is prompted to switch to a selectable provider.
    blockers.push({
      kind: "provider_deferred",
      providerId: descriptor.id,
      stage: descriptor.stage,
      message: `${descriptor.display_name} is implemented but deferred for the current MVP. Switch to a selectable provider.`,
    });
  }

  if (modelIsRequired(descriptor) && !trimmed(model)) {
    blockers.push({
      kind: "model_unselected",
      providerId: descriptor.id,
      stage: descriptor.stage,
      message: `${descriptor.display_name} needs a selected model.`,
    });
  }

  for (const credential of credentials) {
    if (!credential.present) {
      blockers.push({
        kind: "missing_credential",
        providerId: descriptor.id,
        stage: descriptor.stage,
        key: credential.key,
        message: `${descriptor.display_name} is missing ${credential.key}.`,
      });
    }
  }

  if (readiness?.status === "missing_credentials" && credentials.length === 0) {
    blockers.push({
      kind: "missing_credential",
      providerId: descriptor.id,
      stage: descriptor.stage,
      message: readiness.message,
    });
  }

  if (readiness?.status === "error") {
    blockers.push({
      kind: "provider_error",
      providerId: descriptor.id,
      stage: descriptor.stage,
      message: readiness.message,
    });
  }

  const runtimeBlocker = runtimeReadinessBlocker(descriptor, readiness);
  if (runtimeBlocker) blockers.push(runtimeBlocker);

  blockers.push(...sourceBlockersForProvider(descriptor, path, role, context));

  return blockers;
}

function sourceBlockersForProvider(
  descriptor: ProviderDescriptor,
  path: ProviderSetupStagePath,
  role: ProviderSetupStageRole,
  context: SelectionContext,
): ProviderSetupBlocker[] {
  const { sourceState } = context;
  if (!sourceState || !providerConsumesAudioInput(descriptor, path, role)) {
    return [];
  }

  const blockers: ProviderSetupBlocker[] = [];
  const provider = {
    providerId: descriptor.id,
    stage: descriptor.stage,
  };

  if (sourceState.selectedSourceIds.length === 0) {
    blockers.push({
      ...provider,
      kind: "source_unselected",
      message: `${descriptor.display_name} needs an audio source selection.`,
    });
    return blockers;
  }

  if (
    descriptor.source_policy === "single_session" &&
    sourceState.selectedSourceIds.length > 1
  ) {
    blockers.push({
      ...provider,
      kind: "source_policy_conflict",
      message: `${descriptor.display_name} supports one selected audio source at a time; ${sourceState.selectedSourceIds.length} are selected.`,
    });
  }

  for (const sourceId of sourceState.unavailableSourceIds) {
    blockers.push({
      ...provider,
      kind: "source_unavailable",
      sourceId,
      message: `Selected audio source ${sourceId} is not available.`,
    });
  }

  for (const source of sourceState.selectedSources) {
    const unsupportedReason = unsupportedSourceReason(source);
    if (unsupportedReason) {
      blockers.push({
        ...provider,
        kind: "source_unsupported",
        sourceId: sourceCaptureTargetId(source),
        sourceName: source.name,
        message: `${source.name} cannot be captured: ${unsupportedReason}`,
      });
    }

    const permissionBlocker = sourcePermissionBlocker(descriptor, source);
    if (permissionBlocker) blockers.push(permissionBlocker);
  }

  return blockers;
}

function providerConsumesAudioInput(
  descriptor: ProviderDescriptor,
  path: ProviderSetupStagePath,
  role: ProviderSetupStageRole,
): boolean {
  if (descriptor.stage === "asr") return true;
  return (
    descriptor.stage === "realtime_agent" &&
    path === "native_realtime_agent" &&
    role === "native_realtime_agent"
  );
}

function sourcePermissionBlocker(
  descriptor: ProviderDescriptor,
  source: ProviderSetupAudioSource,
): ProviderSetupBlocker | null {
  const permissionStatus = source.permission_status ?? null;
  if (
    !permissionStatus ||
    permissionStatus === "Granted" ||
    permissionStatus === "NotRequired"
  ) {
    return null;
  }

  return {
    kind: "source_permission_unavailable",
    providerId: descriptor.id,
    stage: descriptor.stage,
    sourceId: sourceCaptureTargetId(source),
    sourceName: source.name,
    permissionStatus,
    permissionRecovery: source.permission_recovery ?? undefined,
    message: sourcePermissionRecoveryMessage(source, permissionStatus),
  };
}

function sourcePermissionLabel(status: AudioPermissionStatus): string {
  switch (status) {
    case "Denied":
      return "denied";
    case "NotDetermined":
      return "not granted";
    case "Unknown":
      return "unavailable";
    case "Granted":
      return "granted";
    case "NotRequired":
      return "not required";
  }
}

function sourcePermissionRecoveryMessage(
  source: ProviderSetupAudioSource,
  status: AudioPermissionStatus,
): string {
  const recovery = source.permission_recovery ?? null;
  if (recovery) return `${source.name}: ${recovery.summary} ${recovery.body}`;
  return `Audio capture permission is ${sourcePermissionLabel(status)} for ${source.name}.`;
}

function unsupportedSourceReason(
  source: ProviderSetupAudioSource,
): string | null {
  const capabilities = source.capabilities ?? null;
  if (!capabilities) return null;

  const explicitReason = trimmed(capabilities.unsupported_reason);
  if (capabilities.capture_supported === false) {
    return explicitReason ?? "the selected source is not supported";
  }

  const requirement = sourceCapabilityRequirement(source);
  if (requirement && capabilities[requirement.key] === false) {
    return `${requirement.label} capture is not supported`;
  }

  return null;
}

function sourceCapabilityRequirement(
  source: ProviderSetupAudioSource,
): SourceCapabilityRequirement | null {
  const target = source.capture_target ?? source.id;
  if (target === "system" || target === "system-default") {
    return { key: "supports_system_capture", label: "System" };
  }
  if (target.startsWith("device:")) {
    return { key: "supports_device_selection", label: "Device" };
  }
  if (target.startsWith("tree:") || target.startsWith("process-tree:")) {
    return {
      key: "supports_process_tree_capture",
      label: "Process-tree",
    };
  }
  if (
    target.startsWith("app:") ||
    target.startsWith("name:") ||
    target.startsWith("app-name:")
  ) {
    return { key: "supports_application_capture", label: "Application" };
  }

  switch (source.source_type.type) {
    case "SystemDefault":
      return { key: "supports_system_capture", label: "System" };
    case "Device":
      return { key: "supports_device_selection", label: "Device" };
    case "Application":
    case "ApplicationName":
      return { key: "supports_application_capture", label: "Application" };
    case "ProcessTree":
      return {
        key: "supports_process_tree_capture",
        label: "Process-tree",
      };
  }
}

function runtimeReadinessBlocker(
  descriptor: ProviderDescriptor,
  readiness: ProviderReadiness | null,
): ProviderSetupBlocker | null {
  const runtime = readiness?.runtime;
  if (!runtime || runtime.status === "healthy") return null;

  const common = {
    providerId: descriptor.id,
    stage: descriptor.stage,
    message: runtime.message,
  };

  switch (runtime.status) {
    case "feature_missing":
      return {
        ...common,
        kind: "missing_feature",
        feature: runtime.required_feature ?? undefined,
      };
    case "model_missing":
      return {
        ...common,
        kind: "missing_model",
        model: runtime.model_id ?? undefined,
      };
    case "runtime_unavailable":
      return { ...common, kind: "runtime_unavailable" };
    case "load_failed":
      return { ...common, kind: "load_failed" };
  }
}

function providerReadinessStatus(
  descriptor: ProviderDescriptor,
  blockers: readonly ProviderSetupBlocker[],
  readiness: ProviderReadiness | null,
): ProviderSetupReadinessStatus {
  if (blockers.some((blocker) => blocker.kind === "missing_credential")) {
    return "missing_credentials";
  }
  if (blockers.some((blocker) => blocker.kind === "provider_error")) {
    return "error";
  }
  if (blockers.length > 0) return "blocked";
  if (readiness) return readinessStatusFromBackend(readiness.status);
  if (descriptor.status === "planned") return "blocked";
  // A deferred-but-implemented provider is not offered for new selection; a
  // saved session still pointing at one reads as blocked until switched.
  if (providerIsDeferred(descriptor)) return "blocked";
  if (descriptor.required_features.length > 0) return "unchecked";
  if (descriptor.model_catalog === "local_files") return "unchecked";
  if (descriptor.health_check_command) return "unchecked";
  return "ready";
}

function readinessStatusFromBackend(
  status: ProviderReadinessStatus,
): ProviderSetupReadinessStatus {
  switch (status) {
    case "ready":
      return "ready";
    case "missing_credentials":
      return "missing_credentials";
    case "error":
      return "error";
    case "unchecked":
      return "unchecked";
  }
}

function aggregateReadiness(
  selections: readonly ProviderSetupProviderSelection[],
): ProviderSetupReadinessStatus {
  const statuses = selections.map((selection) => selection.readinessStatus);

  if (statuses.includes("missing_credentials")) return "missing_credentials";
  if (statuses.includes("error")) return "error";
  if (statuses.includes("blocked")) return "blocked";
  if (statuses.includes("unchecked")) return "unchecked";
  return "ready";
}

function aggregateDataBoundary(
  selections: readonly ProviderSetupProviderSelection[],
): ProviderSetupDataBoundary {
  const boundaries = selections
    .filter((selection) => selection.providerId !== "tts.none")
    .map((selection) => selection.dataBoundary);

  if (boundaries.length === 0) return "not_applicable";

  const uniqueBoundaries = Array.from(new Set(boundaries));
  if (uniqueBoundaries.length === 1) return uniqueBoundaries[0];

  return uniqueBoundaries.includes("local_only")
    ? "mixed_local_cloud"
    : "mixed_cloud";
}

function stageCoverageForSelection(
  selection: ProviderSetupProviderSelection,
): ProviderSetupStageCoverage {
  return {
    stage: selection.stage,
    path: selection.path,
    role: selection.role,
    covered: selection.providerId !== "tts.none",
    providerId: selection.providerId,
    providerName: selection.providerName,
    model: selection.model,
    readinessStatus: selection.readinessStatus,
    dataBoundary: selection.dataBoundary,
  };
}

function selectedModeId(
  input: ProviderSetupModesInput,
  context: SelectionContext,
): ProviderSetupModeId {
  const runtimeModeProvided =
    input.conversationMode !== undefined || input.converseEngine !== undefined;
  const nativeRealtimeSelected = runtimeModeProvided
    ? input.conversationMode === "converse" && input.converseEngine === "native"
    : input.nativeRealtimeEnabled === true;
  if (nativeRealtimeSelected) return "native_realtime";

  const currentAsr = context.registry.byId.get(currentAsrProviderId(context));
  const currentLlm = context.registry.byId.get(currentLlmProviderId(context));
  const ttsDescriptor = context.registry.byId.get(
    currentTtsProviderId(context),
  );
  const asrLocal = currentAsr ? providerIsLocal(currentAsr) : false;
  const llmLocal = currentLlm ? providerIsLocal(currentLlm) : false;
  const ttsLocalOrDisabled =
    !ttsDescriptor ||
    currentTtsProviderId(context) === "tts.none" ||
    providerIsLocal(ttsDescriptor);

  if (asrLocal && llmLocal && ttsLocalOrDisabled) return "local_private";
  if (!asrLocal && !llmLocal && ttsLocalOrDisabled) return "cloud_fast";
  if (!asrLocal && !llmLocal && !ttsLocalOrDisabled) return "cloud_fast";
  return "hybrid";
}

function currentAsrProviderId(context: SelectionContext): string {
  return providerIdForSettingsVariant(
    context.registry,
    "asr",
    context.settings.asrType,
  );
}

function currentLlmProviderId(context: SelectionContext): string {
  return providerIdForSettingsVariant(
    context.registry,
    "llm",
    context.settings.llmType,
  );
}

function currentTtsProviderId(context: SelectionContext): string {
  return context.tts.ttsType === "deepgram_aura"
    ? "tts.deepgram_aura"
    : "tts.none";
}

function providerIdForSettingsVariant(
  registry: RegistryLookup,
  stage: ProviderStage,
  settingsVariant: string,
): string {
  return (
    registry.all.find(
      (provider) =>
        provider.stage === stage &&
        provider.settings_variant === settingsVariant,
    )?.id ?? `${stage}.${settingsVariant}`
  );
}

function pickLocalAsrProviderId(context: SelectionContext): string {
  const current = currentAsrProviderId(context);
  const descriptor = context.registry.byId.get(current);
  if (descriptor?.stage === "asr" && providerIsLocal(descriptor))
    return current;
  return firstSelectableProviderId(context, LOCAL_ASR_PROVIDER_PRIORITY);
}

function pickCloudAsrProviderId(context: SelectionContext): string {
  const current = currentAsrProviderId(context);
  const descriptor = context.registry.byId.get(current);
  if (descriptor?.stage === "asr" && !providerIsLocal(descriptor))
    return current;
  return firstSelectableProviderId(context, CLOUD_ASR_PROVIDER_PRIORITY);
}

function pickLocalLlmProviderId(context: SelectionContext): string {
  const current = currentLlmProviderId(context);
  const descriptor = context.registry.byId.get(current);
  if (descriptor?.stage === "llm" && providerIsLocal(descriptor))
    return current;
  return firstSelectableProviderId(context, LOCAL_LLM_PROVIDER_PRIORITY);
}

function pickCloudLlmProviderId(context: SelectionContext): string {
  const current = currentLlmProviderId(context);
  const descriptor = context.registry.byId.get(current);
  if (descriptor?.stage === "llm" && !providerIsLocal(descriptor))
    return current;
  return firstSelectableProviderId(context, CLOUD_LLM_PROVIDER_PRIORITY);
}

function pickHybridAsrProviderId(context: SelectionContext): string {
  const currentAsr = currentAsrProviderId(context);
  const currentLlm = context.registry.byId.get(currentLlmProviderId(context));
  const currentAsrDescriptor = context.registry.byId.get(currentAsr);

  if (
    currentAsrDescriptor &&
    currentLlm &&
    providerIsLocal(currentAsrDescriptor) !== providerIsLocal(currentLlm)
  ) {
    return currentAsr;
  }

  return pickLocalAsrProviderId(context);
}

function pickHybridLlmProviderId(context: SelectionContext): string {
  const currentLlm = currentLlmProviderId(context);
  const currentAsr = context.registry.byId.get(currentAsrProviderId(context));
  const currentLlmDescriptor = context.registry.byId.get(currentLlm);

  if (
    currentAsr &&
    currentLlmDescriptor &&
    providerIsLocal(currentAsr) !== providerIsLocal(currentLlmDescriptor)
  ) {
    return currentLlm;
  }

  return pickCloudLlmProviderId(context);
}

function firstSelectableProviderId(
  context: SelectionContext,
  providerIds: readonly string[],
): string {
  // Auto-selection (Express setup modes) must land on a provider the UI
  // actually offers — gate on `ui_selectable`, not `status`, so a deferred
  // implemented provider is skipped in favor of the next selectable one.
  return (
    providerIds.find(
      (providerId) =>
        context.registry.byId.get(providerId)?.ui_selectable === true,
    ) ?? providerIds[0]
  );
}

function providerIsLocal(descriptor: ProviderDescriptor): boolean {
  return !descriptor.privacy.data_leaves_device;
}

function selectedModelForProvider(
  providerId: string,
  context: SelectionContext,
): string | null {
  const { settings } = context;

  switch (providerId) {
    case "asr.local_whisper":
      return selectedOrDefault(settings.whisperModel, providerId);
    case "asr.api":
      return selectedOrDefault(settings.asrModel, providerId);
    case "asr.openai_realtime":
      return selectedOrDefault(settings.openaiRealtimeModel, providerId);
    case "asr.deepgram":
      return selectedOrDefault(settings.deepgramModel, providerId);
    case "asr.assemblyai":
      return selectedOrDefault("", providerId);
    case "asr.sherpa_onnx":
      return selectedOrDefault(settings.sherpaModelDir, providerId);
    case "asr.moonshine":
      return selectedOrDefault("", providerId);
    case "llm.local_llama":
      return settings.llmType === "local_llama"
        ? selectedOrDefault(settings.llmModel, providerId)
        : selectedOrDefault("", providerId);
    case "llm.api":
      return selectedOrDefault(settings.llmModel, providerId);
    case "llm.cerebras":
      return settings.llmType === "cerebras"
        ? selectedOrDefault(settings.llmModel, providerId)
        : selectedOrDefault("", providerId);
    case "llm.openrouter":
      return selectedOrDefault(settings.openrouterModel, providerId);
    case "llm.aws_bedrock":
      return selectedOrDefault(settings.awsBedrockModelId, providerId);
    case "llm.mistralrs":
      return selectedOrDefault(settings.mistralrsModelId, providerId);
    case "realtime_agent.gemini_live":
      return selectedOrDefault(settings.geminiModel, providerId);
    case "tts.deepgram_aura":
      return selectedOrDefault(context.tts.auraVoice, providerId);
    default:
      return selectedOrDefault("", providerId);
  }
}

function selectedOrDefault(value: string, providerId: string): string | null {
  return trimmed(value) ?? trimmed(defaultModelForProvider(providerId));
}

function modelIsRequired(descriptor: ProviderDescriptor): boolean {
  return (
    descriptor.model_catalog !== "none" &&
    descriptor.id !== "tts.none" &&
    descriptor.stage !== "diarization"
  );
}

function trimmed(value: string | null | undefined): string | null {
  const next = value?.trim();
  return next ? next : null;
}
