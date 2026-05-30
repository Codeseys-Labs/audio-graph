/**
 * Shared types, reducer, and constants for the `SettingsPage` form.
 *
 * The settings modal uses a single `useReducer` snapshot (`SettingsState`)
 * so every sub-form (`AudioSettings`, `AsrProviderSettings`,
 * `LlmProviderSettings`, `GeminiSettings`, `CredentialsManager`)
 * dispatches against the same slice. This file holds:
 *   - The discriminated unions for the pick-your-provider selectors
 *     (`AsrType`, `LlmType`, `AwsCredentialMode`, `GeminiAuthType`).
 *   - `SettingsState` (the full in-memory form) + `initialSettingsState`.
 *   - The `settingsReducer` + the `setField(key, value)` action creator.
 *   - Helpers for rendering model-readiness badges (`readinessBadge`) and
 *     converting the flat reducer state into the nested Rust
 *     `AwsCredentialSource` payload on save (`buildAwsCredentialSource`).
 *
 * Not a component — pure TypeScript. Kept next to the settings components
 * because it is their private contract.
 */

import {
  LFM2_EXTRACT_MODEL_FILENAME,
  WHISPER_SMALL_EN_MODEL_FILENAME,
} from "../modelConstants";
import type { AwsCredentialSource, ModelReadiness } from "../types";

export type AsrType =
  | "local_whisper"
  | "api"
  | "aws_transcribe"
  | "deepgram"
  | "assemblyai"
  | "sherpa_onnx";
export type LlmType =
  | "local_llama"
  | "api"
  | "openrouter"
  | "aws_bedrock"
  | "mistralrs";
export type AwsCredentialMode = "default_chain" | "profile" | "access_keys";
export type GeminiAuthType = "api_key" | "vertex_ai";
export type SampleRate = 22050 | 32000 | 44100 | 48000 | 88200 | 96000;
export type ChannelCount = 1 | 2;
export type LogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace";
export type TestKey =
  | "asr_api"
  | "deepgram"
  | "assemblyai"
  | "gemini"
  | "aws_asr"
  | "aws_bedrock"
  | "openrouter";
export type TestResults = Partial<
  Record<TestKey, { ok: boolean; msg: string }>
>;

/**
 * Endpoint-keyed API credentials loaded from the backend store, kept in the
 * draft so the OpenAI-compatible (`api`) ASR/LLM branches can repopulate the
 * key field when the user switches the endpoint URL between providers that
 * each have their own saved key (e.g. OpenAI ↔ Groq ↔ Together ↔ Fireworks ↔
 * Gemini). Without this, changing the endpoint would blank the visible key and
 * force a re-type even though the key is already saved. (W3.5)
 */
export type EndpointCredentialKey =
  | "openai_api_key"
  | "openrouter_api_key"
  | "groq_api_key"
  | "together_api_key"
  | "fireworks_api_key"
  | "gemini_api_key";
export type EndpointCredentialCache = Partial<
  Record<EndpointCredentialKey, string>
>;

/**
 * Map an OpenAI-compatible endpoint URL to the credential-store key its API
 * key is saved under. Mirrors the backend's per-endpoint credential routing so
 * the UI can resolve the right saved key for whatever endpoint is selected.
 */
export function endpointCredentialKey(endpoint: string): EndpointCredentialKey {
  const lower = endpoint.toLowerCase();
  if (
    lower.includes("generativelanguage.googleapis.com") ||
    lower.includes("gemini")
  ) {
    return "gemini_api_key";
  }
  if (lower.includes("openrouter")) return "openrouter_api_key";
  if (lower.includes("groq")) return "groq_api_key";
  if (lower.includes("together")) return "together_api_key";
  if (lower.includes("fireworks")) return "fireworks_api_key";
  return "openai_api_key";
}

export interface SettingsState {
  // ASR
  asrType: AsrType;
  whisperModel: string;
  asrEndpoint: string;
  asrApiKey: string;
  asrModel: string;
  // AWS Transcribe
  awsAsrRegion: string;
  awsAsrLanguageCode: string;
  awsAsrCredentialMode: AwsCredentialMode;
  awsAsrProfileName: string;
  awsAsrAccessKey: string;
  awsAsrSecretKey: string;
  awsAsrSessionToken: string;
  awsAsrDiarization: boolean;
  // Deepgram
  deepgramApiKey: string;
  deepgramModel: string;
  deepgramDiarization: boolean;
  deepgramEndpointingMs: number;
  deepgramUtteranceEndMs: number;
  deepgramVadEvents: boolean;
  deepgramEotThreshold: number;
  deepgramEagerEotThreshold: number;
  deepgramEotTimeoutMs: number;
  // AssemblyAI
  assemblyaiApiKey: string;
  assemblyaiDiarization: boolean;
  // Sherpa-ONNX
  sherpaModelDir: string;
  sherpaEndpointDetection: boolean;
  // LLM
  llmType: LlmType;
  llmEndpoint: string;
  llmApiKey: string;
  llmModel: string;
  llmMaxTokens: number;
  llmTemperature: number;
  /**
   * Enable streaming/incremental prefill — only meaningful for the local
   * llama.cpp backend (ADR-0012). The UI gates the toggle on
   * `llmType === "local_llama"`; persisted as `streaming_prefill`.
   */
  streamingPrefill: boolean;
  // OpenRouter (first-class provider — ADR-0005)
  openrouterApiKey: string;
  openrouterModel: string;
  openrouterBaseUrl: string;
  openrouterIncludeUsageInStream: boolean;
  /** Cached catalog from `list_openrouter_models_cmd`. */
  openrouterModels: import("../types").OpenRouterModel[];
  /** Unix-ms when `openrouterModels` was last refreshed. `0` = never. */
  openrouterModelsLoadedAt: number;
  /** True while a list_openrouter_models_cmd is in flight. */
  openrouterModelsLoading: boolean;
  // Mistral.rs
  mistralrsModelId: string;
  // AWS Bedrock
  awsBedrockRegion: string;
  awsBedrockModelId: string;
  awsBedrockCredentialMode: AwsCredentialMode;
  awsBedrockProfileName: string;
  awsBedrockAccessKey: string;
  awsBedrockSecretKey: string;
  awsBedrockSessionToken: string;
  // Gemini
  geminiAuthMode: GeminiAuthType;
  geminiApiKey: string;
  geminiModel: string;
  geminiProjectId: string;
  geminiLocation: string;
  geminiServiceAccountPath: string;
  // Audio + diagnostics
  audioSampleRate: SampleRate;
  audioChannels: ChannelCount;
  logLevel: LogLevel;
  // UI
  confirmDelete: string | null;
  awsProfiles: string[];
  testResults: TestResults;
  testingKey: TestKey | null;
  /**
   * Cache of saved per-endpoint API keys (keyed by credential-store key), so
   * the `api` ASR/LLM branches can re-fill the key field when the user swaps
   * the endpoint to another provider that already has a stored key. (W3.5)
   */
  endpointCredentials: EndpointCredentialCache;
}

/**
 * Discriminated union of actions. `SET_FIELD` covers every plain scalar form
 * field; compound effects (hydration from settings, test lifecycle, shared
 * AWS credential mirroring) get dedicated actions so callers don't have to
 * dispatch multiple times.
 */
export type SettingsAction =
  | {
      type: "SET_FIELD";
      field: keyof SettingsState;
      value: SettingsState[keyof SettingsState];
    }
  | { type: "HYDRATE_FROM_SETTINGS"; patch: Partial<SettingsState> }
  | { type: "SET_AWS_SHARED_SECRET"; secret: string }
  | { type: "SET_AWS_SHARED_SESSION_TOKEN"; token: string }
  | { type: "CLEAR_AWS_SHARED_KEYS" }
  | { type: "SET_AWS_PROFILES"; profiles: string[] }
  | { type: "TEST_START"; key: TestKey }
  | { type: "TEST_RESULT"; key: TestKey; result: { ok: boolean; msg: string } }
  | { type: "TEST_FINISH" }
  | { type: "SET_CONFIRM_DELETE"; filename: string | null }
  | {
      type: "SET_ENDPOINT_CREDENTIALS";
      credentials: EndpointCredentialCache;
    }
  | {
      type: "SET_OPENROUTER_MODELS";
      models: import("../types").OpenRouterModel[];
      loadedAt: number;
    }
  | { type: "SET_OPENROUTER_MODELS_LOADING"; loading: boolean };

/** Type-safe helper for dispatching `SET_FIELD` without widening the value. */
export function setField<K extends keyof SettingsState>(
  field: K,
  value: SettingsState[K],
): SettingsAction {
  return {
    type: "SET_FIELD",
    field,
    value: value as SettingsState[keyof SettingsState],
  };
}

export const initialSettingsState: SettingsState = {
  asrType: "local_whisper",
  whisperModel: WHISPER_SMALL_EN_MODEL_FILENAME,
  asrEndpoint: "",
  asrApiKey: "",
  asrModel: "",
  awsAsrRegion: "us-east-1",
  awsAsrLanguageCode: "en-US",
  awsAsrCredentialMode: "default_chain",
  awsAsrProfileName: "",
  awsAsrAccessKey: "",
  awsAsrSecretKey: "",
  awsAsrSessionToken: "",
  awsAsrDiarization: true,
  deepgramApiKey: "",
  deepgramModel: "nova-3",
  deepgramDiarization: true,
  deepgramEndpointingMs: 300,
  deepgramUtteranceEndMs: 1000,
  deepgramVadEvents: true,
  deepgramEotThreshold: 0.5,
  deepgramEagerEotThreshold: 0,
  deepgramEotTimeoutMs: 0,
  assemblyaiApiKey: "",
  assemblyaiDiarization: true,
  sherpaModelDir: "streaming-zipformer-en-20M",
  sherpaEndpointDetection: true,
  llmType: "api",
  llmEndpoint: "http://localhost:11434/v1",
  llmApiKey: "",
  llmModel: "llama3.2",
  llmMaxTokens: 2048,
  llmTemperature: 0.7,
  streamingPrefill: false,
  openrouterApiKey: "",
  openrouterModel: "",
  openrouterBaseUrl: "https://openrouter.ai/api/v1",
  openrouterIncludeUsageInStream: true,
  openrouterModels: [],
  openrouterModelsLoadedAt: 0,
  openrouterModelsLoading: false,
  mistralrsModelId: LFM2_EXTRACT_MODEL_FILENAME,
  awsBedrockRegion: "us-east-1",
  awsBedrockModelId: "",
  awsBedrockCredentialMode: "default_chain",
  awsBedrockProfileName: "",
  awsBedrockAccessKey: "",
  awsBedrockSecretKey: "",
  awsBedrockSessionToken: "",
  geminiAuthMode: "api_key",
  geminiApiKey: "",
  geminiModel: "gemini-3.1-flash-live-preview",
  geminiProjectId: "",
  geminiLocation: "",
  geminiServiceAccountPath: "",
  audioSampleRate: 48000,
  audioChannels: 1,
  logLevel: "info",
  confirmDelete: null,
  awsProfiles: [],
  testResults: {},
  testingKey: null,
  endpointCredentials: {},
};

export function settingsReducer(
  state: SettingsState,
  action: SettingsAction,
): SettingsState {
  switch (action.type) {
    case "SET_FIELD":
      return { ...state, [action.field]: action.value } as SettingsState;
    case "HYDRATE_FROM_SETTINGS":
      return { ...state, ...action.patch };
    case "SET_AWS_SHARED_SECRET":
      return {
        ...state,
        awsAsrSecretKey: action.secret,
        awsBedrockSecretKey: action.secret,
      };
    case "SET_AWS_SHARED_SESSION_TOKEN":
      return {
        ...state,
        awsAsrSessionToken: action.token,
        awsBedrockSessionToken: action.token,
      };
    case "CLEAR_AWS_SHARED_KEYS":
      return {
        ...state,
        awsAsrSecretKey: "",
        awsBedrockSecretKey: "",
        awsAsrSessionToken: "",
        awsBedrockSessionToken: "",
      };
    case "SET_AWS_PROFILES":
      return { ...state, awsProfiles: action.profiles };
    case "TEST_START":
      return {
        ...state,
        testingKey: action.key,
        testResults: { ...state.testResults, [action.key]: undefined },
      };
    case "TEST_RESULT":
      return {
        ...state,
        testResults: { ...state.testResults, [action.key]: action.result },
      };
    case "TEST_FINISH":
      return { ...state, testingKey: null };
    case "SET_CONFIRM_DELETE":
      return { ...state, confirmDelete: action.filename };
    case "SET_ENDPOINT_CREDENTIALS":
      return {
        ...state,
        endpointCredentials: {
          ...state.endpointCredentials,
          ...action.credentials,
        },
      };
    case "SET_OPENROUTER_MODELS":
      return {
        ...state,
        openrouterModels: action.models,
        openrouterModelsLoadedAt: action.loadedAt,
        openrouterModelsLoading: false,
      };
    case "SET_OPENROUTER_MODELS_LOADING":
      return { ...state, openrouterModelsLoading: action.loading };
  }
}

/** Map a ModelReadiness value to a CSS modifier and translation key. */
export function readinessBadge(status: ModelReadiness): {
  cls: string;
  labelKey: string;
} {
  switch (status) {
    case "Ready":
      return {
        cls: "status-badge--ready",
        labelKey: "settings.modelReadiness.ready",
      };
    case "NotDownloaded":
      return {
        cls: "status-badge--not-downloaded",
        labelKey: "settings.modelReadiness.notDownloaded",
      };
    case "Invalid":
      return {
        cls: "status-badge--invalid",
        labelKey: "settings.modelReadiness.invalid",
      };
  }
}

export function buildAwsCredentialSource(
  mode: AwsCredentialMode,
  profileName: string,
  accessKey: string,
): AwsCredentialSource {
  switch (mode) {
    case "profile":
      return { type: "profile", name: profileName };
    case "access_keys":
      return { type: "access_keys", access_key: accessKey };
    default:
      return { type: "default_chain" };
  }
}
