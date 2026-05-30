/**
 * Settings modal — the full configuration surface for the app.
 *
 * Composes five sub-forms (`AudioSettings`, `AsrProviderSettings`,
 * `LlmProviderSettings`, `GeminiSettings`, `CredentialsManager`) around a
 * shared `useReducer`-based form state (see `settingsTypes.ts`). The
 * reducer lives in this component so every sub-form dispatches against
 * the same snapshot; the top-level "Save" button invokes
 * `save_settings_cmd` once with the full patched `AppSettings`.
 *
 * Focus is trapped in the modal via `useFocusTrap` and release on unmount.
 * Escape is handled by `useKeyboardShortcuts` at the App level.
 *
 * Store bindings: `settings` (seed), `loadSettings`, `settingsOpen`,
 * `closeSettings` — `openSettings` is invoked from `ControlBar` /
 * `App.tsx` keyboard handler / `ExpressSetup` Advanced link.
 *
 * Parent: `App.tsx` (rendered conditionally when `settingsOpen` is true).
 * No props.
 */
import { useEffect, useMemo, useReducer, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { useAudioGraphStore } from "../store";
import { useFocusTrap } from "../hooks/useFocusTrap";
import { errorToMessage } from "../utils/errorToMessage";
import type {
  AsrProvider,
  GeminiAuthMode,
  GeminiSettings as GeminiSettingsType,
  LlmApiConfig,
  LlmProvider,
} from "../types";
import { TTS_AURA_VOICES } from "../types";
import {
  buildAwsCredentialSource,
  initialSettingsState,
  setField,
  settingsReducer,
  type ChannelCount,
  type LogLevel,
  type SampleRate,
  type SettingsState,
  type TestKey,
} from "./settingsTypes";
import AudioSettings from "./AudioSettings";
import AsrProviderSettings from "./AsrProviderSettings";
import LlmProviderSettings from "./LlmProviderSettings";
import GeminiSettings from "./GeminiSettings";
import CredentialsManager from "./CredentialsManager";
import LoggingSettings from "./LoggingSettings";
import Icon from "./Icon";
import IconButton from "./IconButton";

const CLOUD_CREDENTIAL_KEYS = [
  "openai_api_key",
  "openrouter_api_key",
  "groq_api_key",
  "together_api_key",
  "fireworks_api_key",
  "deepgram_api_key",
  "assemblyai_api_key",
  "gemini_api_key",
  "aws_access_key",
] as const;

type CloudCredentialKey = (typeof CLOUD_CREDENTIAL_KEYS)[number];
type WritableCredentialKey = CloudCredentialKey | "aws_secret_key" | "aws_session_token";
type CredentialSnapshot = Partial<Record<CloudCredentialKey, string>>;

function credentialKeyForEndpoint(endpoint: string): CloudCredentialKey {
  const lower = endpoint.toLowerCase();
  if (lower.includes("generativelanguage.googleapis.com") || lower.includes("gemini")) {
    return "gemini_api_key";
  }
  if (lower.includes("groq")) return "groq_api_key";
  if (lower.includes("together")) return "together_api_key";
  if (lower.includes("fireworks")) return "fireworks_api_key";
  return "openai_api_key";
}

function credentialForEndpoint(
  endpoint: string,
  credentials: CredentialSnapshot,
): string {
  return credentials[credentialKeyForEndpoint(endpoint)] ?? "";
}

async function loadCredentialSnapshot(): Promise<CredentialSnapshot> {
  const entries = await Promise.all(
    CLOUD_CREDENTIAL_KEYS.map(async (key) => {
      const value = await invoke<string | null>("load_credential_cmd", { key });
      return [key, value?.trim() ? value : undefined] as const;
    }),
  );
  return entries.reduce<CredentialSnapshot>((acc, [key, value]) => {
    if (value) acc[key] = value;
    return acc;
  }, {});
}

async function saveCredentialIfPresent(
  key: WritableCredentialKey,
  value: string,
): Promise<void> {
  if (!value.trim()) return;
  await invoke("save_credential_cmd", { key, value });
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
  "openrouterModelsLoading",
  "endpointCredentials",
];

// Local (non-reducer) editable state that also participates in dirty tracking.
interface TtsLocalState {
  ttsType: "none" | "deepgram_aura";
  auraVoice: string;
  auraSpeed: number;
  speakAloud: boolean;
}

/**
 * Serialise the editable slice of the settings form (reducer state minus the
 * ephemeral UI fields, plus the TTS local state) into a stable string we can
 * compare against a baseline snapshot to detect unsaved changes.
 */
function settingsFingerprint(
  state: SettingsState,
  tts: TtsLocalState,
): string {
  const content: Record<string, unknown> = { ...tts };
  (Object.keys(state) as (keyof SettingsState)[]).forEach((key) => {
    if (!DIRTY_IGNORED_FIELDS.includes(key)) {
      content[key as string] = state[key];
    }
  });
  return JSON.stringify(content);
}

function SettingsPage() {
  const { t } = useTranslation();
  const modalRef = useFocusTrap<HTMLDivElement>();
  const {
    settings,
    models,
    modelStatus,
    settingsLoading,
    isDownloading,
    downloadProgress,
    isDeletingModel,
    closeSettings,
    saveSettings,
    downloadModel,
    deleteModel,
    listAwsProfiles,
  } = useAudioGraphStore();
  const nativeS2sEnabled = useAudioGraphStore((s) => s.nativeS2sEnabled);
  const setNativeS2sEnabled = useAudioGraphStore((s) => s.setNativeS2sEnabled);
  const notify = useAudioGraphStore((s) => s.notify);

  const [state, dispatch] = useReducer(settingsReducer, initialSettingsState);
  const {
    asrType,
    whisperModel,
    asrEndpoint,
    asrApiKey,
    asrModel,
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
    assemblyaiApiKey,
    assemblyaiDiarization,
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
    openrouterModelsLoadedAt,
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
  const [ttsType, setTtsType] = useState<"none" | "deepgram_aura">("none");
  const [auraVoice, setAuraVoice] = useState<string>("aura-asteria-en");
  const [auraSpeed, setAuraSpeed] = useState<number>(1.0);
  const [speakAloud, setSpeakAloud] = useState<boolean>(false);
  const [testingTts, setTestingTts] = useState<boolean>(false);
  const [ttsTestResult, setTtsTestResult] = useState<{
    ok: boolean;
    msg: string;
  } | null>(null);

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
  const fingerprint = useMemo(
    () => settingsFingerprint(state, ttsLocal),
    // ttsLocal is reconstructed each render from its constituent fields, so
    // depend on those primitives rather than the wrapper object identity.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [state, ttsType, auraVoice, auraSpeed, speakAloud],
  );
  const dirty = baselineRef.current !== null && baselineRef.current !== fingerprint;

  // Capture (or recapture) the dirty baseline whenever a hydration cycle
  // completes. Runs after the synchronous + async HYDRATE_FROM_SETTINGS
  // dispatches have flushed into `state`, so the fingerprint reflects the
  // freshly loaded settings rather than the pre-hydration defaults.
  useEffect(() => {
    if (baselineEpoch === 0) return;
    baselineRef.current = fingerprint;
    setConfirmingClose(false);
    // We deliberately depend only on the epoch: recapturing on every
    // fingerprint change would defeat dirty tracking entirely.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [baselineEpoch]);

  // Settings are grouped into tabs to keep the modal navigable.
  type SettingsTab = "general" | "stt" | "llm" | "gemini" | "tts" | "logging";
  const SETTINGS_TABS: { id: SettingsTab; label: string }[] = [
    { id: "general", label: "General" },
    { id: "stt", label: "Speech-to-Text" },
    { id: "llm", label: "Language Model" },
    { id: "gemini", label: "Gemini" },
    { id: "tts", label: "Text-to-Speech" },
    { id: "logging", label: "Logging" },
  ];
  const [activeTab, setActiveTab] = useState<SettingsTab>("general");

  const refreshAwsProfiles = async () => {
    dispatch({ type: "SET_AWS_PROFILES", profiles: await listAwsProfiles() });
  };

  // Upper bound on any Test Connection invocation. Without this, a hung
  // network call (e.g. provider stuck in TLS handshake, firewall silently
  // dropping packets) leaves the button forever stuck on "Testing…".
  const TEST_TIMEOUT_MS = 10_000;

  const runTest = async (
    key: TestKey,
    invocation: () => Promise<string>,
  ) => {
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
    key: string,
    label: string,
    clearLocal: () => void,
  ) => {
    const ok = window.confirm(
      t("settings.credentialConfirm.clearPrompt", { label }),
    );
    if (!ok) return;
    try {
      await invoke("delete_credential_cmd", { key });
      clearLocal();
    } catch (e) {
      console.error(`Failed to clear ${key}:`, e);
      window.alert(t("settings.errors.failedToClear", { error: errorToMessage(e) }));
    }
  };

  const handleTestAsrApi = () =>
    runTest("asr_api", () =>
      invoke<string>("test_cloud_asr_connection", {
        endpoint: asrEndpoint,
        apiKey: asrApiKey,
      }),
    );

  const handleTestDeepgram = () =>
    runTest("deepgram", () =>
      invoke<string>("test_deepgram_connection", { apiKey: deepgramApiKey }),
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
        apiKey: deepgramApiKey,
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
      invoke<string>("test_assemblyai_connection", { apiKey: assemblyaiApiKey }),
    );

  const handleTestGemini = () =>
    runTest("gemini", () =>
      invoke<string>("test_gemini_api_key", { apiKey: geminiApiKey }),
    );

  const handleTestAwsAsr = async () => {
    // If user is in access_keys mode, persist the secret + session to the
    // credential store first so the backend `test_aws_credentials` command
    // (which reads from credentials.yaml) can see them.
    if (awsAsrCredentialMode === "access_keys") {
      try {
        if (awsAsrSecretKey) {
          await invoke("save_credential_cmd", {
            key: "aws_secret_key",
            value: awsAsrSecretKey,
          });
        }
        if (awsAsrSessionToken) {
          await invoke("save_credential_cmd", {
            key: "aws_session_token",
            value: awsAsrSessionToken,
          });
        }
      } catch (e) {
        console.error("Failed to stage AWS credentials before test:", e);
      }
    }
    const credential_source = buildAwsCredentialSource(
      awsAsrCredentialMode,
      awsAsrProfileName,
      awsAsrAccessKey,
    );
    return runTest("aws_asr", () =>
      invoke<string>("test_aws_credentials", {
        region: awsAsrRegion,
        credentialSource: credential_source,
      }),
    );
  };

  // OpenRouter model catalog cache TTL (ms). 5 min keeps the dropdown fresh
  // while avoiding hammering /api/v1/models on every settings render.
  const OPENROUTER_MODELS_CACHE_TTL_MS = 5 * 60 * 1000;

  const handleTestOpenRouter = async () => {
    // Persist the key so subsequent app launches can route through it.
    if (openrouterApiKey.trim()) {
      try {
        await invoke("save_credential_cmd", {
          key: "openrouter_api_key",
          value: openrouterApiKey,
        });
      } catch (e) {
        console.error("Failed to save openrouter_api_key before test:", e);
      }
    }
    return runTest("openrouter", () =>
      invoke<string>("test_openrouter_connection_cmd", {
        apiKey: openrouterApiKey,
      }),
    );
  };

  const handleRefreshOpenRouterModels = async () => {
    if (!openrouterApiKey.trim()) return;
    // Skip if cached payload is still fresh (avoid re-hitting the catalog
    // when the user toggles the radio repeatedly within the TTL).
    if (
      openrouterModelsLoadedAt > 0 &&
      Date.now() - openrouterModelsLoadedAt < OPENROUTER_MODELS_CACHE_TTL_MS
    ) {
      return;
    }
    dispatch({ type: "SET_OPENROUTER_MODELS_LOADING", loading: true });
    try {
      // Save the key first so other commands (and a later launch) see it.
      await invoke("save_credential_cmd", {
        key: "openrouter_api_key",
        value: openrouterApiKey,
      });
      const models = await invoke<
        import("../types").OpenRouterModel[]
      >("list_openrouter_models_cmd", { apiKey: openrouterApiKey });
      dispatch({
        type: "SET_OPENROUTER_MODELS",
        models,
        loadedAt: Date.now(),
      });
    } catch (e) {
      console.error("Failed to load OpenRouter models:", e);
      dispatch({ type: "SET_OPENROUTER_MODELS_LOADING", loading: false });
    }
  };

  const handleTestAwsBedrock = async () => {
    if (awsBedrockCredentialMode === "access_keys") {
      try {
        if (awsBedrockSecretKey) {
          await invoke("save_credential_cmd", {
            key: "aws_secret_key",
            value: awsBedrockSecretKey,
          });
        }
        if (awsBedrockSessionToken) {
          await invoke("save_credential_cmd", {
            key: "aws_session_token",
            value: awsBedrockSessionToken,
          });
        }
      } catch (e) {
        console.error("Failed to stage AWS credentials before test:", e);
      }
    }
    const credential_source = buildAwsCredentialSource(
      awsBedrockCredentialMode,
      awsBedrockProfileName,
      awsBedrockAccessKey,
    );
    return runTest("aws_bedrock", () =>
      invoke<string>("test_aws_credentials", {
        region: awsBedrockRegion,
        credentialSource: credential_source,
      }),
    );
  };

  /** Render a test result line (green/red) for a given provider key. */
  const renderTestResult = (key: TestKey) => {
    const r = testResults[key];
    if (!r) return null;
    return (
      <div className={r.ok ? "settings-test-ok" : "settings-test-err"}>
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

    // Audio capture format — clamp to the UI whitelist so an out-of-band
    // value from a hand-edited settings.json doesn't leave the dropdown
    // in a "Custom (n/a)" state. The backend does the same fallback in
    // `resolve_audio_settings`.
    const ALLOWED_RATES: SampleRate[] = [22050, 32000, 44100, 48000, 88200, 96000];
    const ALLOWED_CHANNELS: ChannelCount[] = [1, 2];
    const sr = settings.audio_settings?.sample_rate;
    const ch = settings.audio_settings?.channels;
    const patch: Partial<SettingsState> = {
      audioSampleRate: ALLOWED_RATES.includes(sr as SampleRate)
        ? (sr as SampleRate)
        : 48000,
      audioChannels: ALLOWED_CHANNELS.includes(ch as ChannelCount)
        ? (ch as ChannelCount)
        : 1,
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
      patch.asrApiKey = asr.api_key ?? "";
      patch.asrModel = asr.model;
    } else if (asr.type === "aws_transcribe") {
      patch.awsAsrRegion = asr.region;
      patch.awsAsrLanguageCode = asr.language_code;
      patch.awsAsrDiarization = asr.enable_diarization;
      const cred = asr.credential_source;
      patch.awsAsrCredentialMode = cred.type;
      if (cred.type === "profile") patch.awsAsrProfileName = cred.name;
      if (cred.type === "access_keys") patch.awsAsrAccessKey = cred.access_key ?? "";
    } else if (asr.type === "deepgram") {
      patch.deepgramApiKey = asr.api_key ?? "";
      patch.deepgramModel = asr.model;
      patch.deepgramDiarization = asr.enable_diarization;
      patch.deepgramEndpointingMs = asr.endpointing_ms ?? 300;
      patch.deepgramUtteranceEndMs = asr.utterance_end_ms ?? 1000;
      patch.deepgramVadEvents = asr.vad_events ?? true;
      patch.deepgramEotThreshold = asr.eot_threshold ?? 0.5;
      patch.deepgramEagerEotThreshold = asr.eager_eot_threshold ?? 0;
      patch.deepgramEotTimeoutMs = asr.eot_timeout_ms ?? 0;
    } else if (asr.type === "assemblyai") {
      patch.assemblyaiApiKey = asr.api_key ?? "";
      patch.assemblyaiDiarization = asr.enable_diarization;
    } else if (asr.type === "sherpa_onnx") {
      patch.sherpaModelDir = asr.model_dir;
      patch.sherpaEndpointDetection = asr.enable_endpoint_detection;
    }

    // LLM provider
    const llm = settings.llm_provider;
    patch.llmType = llm.type;
    if (llm.type === "api") {
      patch.llmEndpoint = llm.endpoint;
      patch.llmApiKey = llm.api_key ?? "";
      patch.llmModel = llm.model;
    } else if (llm.type === "aws_bedrock") {
      patch.awsBedrockRegion = llm.region;
      patch.awsBedrockModelId = llm.model_id;
      const cred = llm.credential_source;
      patch.awsBedrockCredentialMode = cred.type;
      if (cred.type === "profile") patch.awsBedrockProfileName = cred.name;
      if (cred.type === "access_keys")
        patch.awsBedrockAccessKey = cred.access_key ?? "";
    } else if (llm.type === "mistralrs") {
      patch.mistralrsModelId = llm.model_id;
    } else if (llm.type === "openrouter") {
      patch.openrouterModel = llm.model;
      patch.openrouterBaseUrl = llm.base_url;
      patch.openrouterIncludeUsageInStream = llm.include_usage_in_stream;
      patch.openrouterApiKey = llm.api_key ?? "";
    }

    // LLM config (advanced — max_tokens / temperature)
    if (settings.llm_api_config) {
      patch.llmMaxTokens = settings.llm_api_config.max_tokens;
      patch.llmTemperature = settings.llm_api_config.temperature;
    }

    // Streaming prefill (local llama.cpp only — ADR-0012). Missing in older
    // settings files → default off.
    patch.streamingPrefill = settings.streaming_prefill ?? false;

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
      if (auth.type === "api_key") {
        patch.geminiApiKey = auth.api_key ?? "";
      } else if (auth.type === "vertex_ai") {
        patch.geminiProjectId = auth.project_id;
        patch.geminiLocation = auth.location;
        patch.geminiServiceAccountPath = auth.service_account_path ?? "";
      }
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

    // Pre-populate AWS secret key + session token from credentials.yaml.
    // Both AWS ASR and AWS Bedrock share the same aws_secret_key / aws_session_token
    // in the backend credential store, so we load once and mirror into both forms.
    (async () => {
      try {
        const credentials = await loadCredentialSnapshot();
        const credentialPatch: Partial<SettingsState> = {};

        // Hydrate EVERY known credential field from the store (not just the
        // active provider's) so switching provider/model never forces the
        // user to re-type a key they've already saved. Only fields with a
        // stored value are set, so we never blank a field the user is editing.
        if (credentials.deepgram_api_key) {
          credentialPatch.deepgramApiKey = credentials.deepgram_api_key;
        }
        if (credentials.assemblyai_api_key) {
          credentialPatch.assemblyaiApiKey = credentials.assemblyai_api_key;
        }
        if (credentials.openrouter_api_key) {
          credentialPatch.openrouterApiKey = credentials.openrouter_api_key;
        }
        if (credentials.gemini_api_key) {
          credentialPatch.geminiApiKey = credentials.gemini_api_key;
        }
        if (credentials.aws_access_key) {
          credentialPatch.awsAsrAccessKey = credentials.aws_access_key;
          credentialPatch.awsBedrockAccessKey = credentials.aws_access_key;
        }
        // API-endpoint keys are keyed by endpoint URL; resolve for whichever
        // endpoint each form currently points at.
        if (asr.type === "api") {
          credentialPatch.asrApiKey = credentialForEndpoint(asr.endpoint, credentials);
        }
        if (llm.type === "api") {
          credentialPatch.llmApiKey = credentialForEndpoint(llm.endpoint, credentials);
        }

        if (Object.keys(credentialPatch).length > 0) {
          dispatch({ type: "HYDRATE_FROM_SETTINGS", patch: credentialPatch });
        }

        // Stash the full set of per-endpoint API keys in the draft so the
        // `api` ASR/LLM branches can re-fill the visible key when the user
        // swaps the endpoint to another provider that already has a saved key
        // (W3.5 — never re-type a key just to switch providers/models).
        const endpointCache: import("./settingsTypes").EndpointCredentialCache = {};
        if (credentials.openai_api_key) endpointCache.openai_api_key = credentials.openai_api_key;
        if (credentials.openrouter_api_key) endpointCache.openrouter_api_key = credentials.openrouter_api_key;
        if (credentials.groq_api_key) endpointCache.groq_api_key = credentials.groq_api_key;
        if (credentials.together_api_key) endpointCache.together_api_key = credentials.together_api_key;
        if (credentials.fireworks_api_key) endpointCache.fireworks_api_key = credentials.fireworks_api_key;
        if (credentials.gemini_api_key) endpointCache.gemini_api_key = credentials.gemini_api_key;
        if (Object.keys(endpointCache).length > 0) {
          dispatch({ type: "SET_ENDPOINT_CREDENTIALS", credentials: endpointCache });
        }
      } catch {
        // Silently tolerate missing credentials.
      }

      try {
        const secret = await invoke<string | null>("load_credential_cmd", {
          key: "aws_secret_key",
        });
        if (secret) {
          dispatch({ type: "SET_AWS_SHARED_SECRET", secret });
        }
      } catch {
        // Silently tolerate missing credentials.
      }
      try {
        const token = await invoke<string | null>("load_credential_cmd", {
          key: "aws_session_token",
        });
        if (token) {
          dispatch({ type: "SET_AWS_SHARED_SESSION_TOKEN", token });
        }
      } catch {
        // Silently tolerate missing credentials.
      }
      // Recapture the baseline after async credential mirroring so loaded
      // keys count as "saved" rather than as unsaved edits.
      setBaselineEpoch((e) => e + 1);
    })();
  }, [settings]);

  // Fetch AWS profiles whenever settings load or the user switches an AWS
  // section into "profile" credential mode. Cheap Tauri call — just parses
  // two small files — so it's fine to re-run on mode change.
  useEffect(() => {
    if (!settings) return;
    if (
      awsAsrCredentialMode === "profile" ||
      awsBedrockCredentialMode === "profile"
    ) {
      refreshAwsProfiles();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings, awsAsrCredentialMode, awsBedrockCredentialMode]);

  // ── Handlers ──────────────────────────────────────────────────────────
  const handleSave = async () => {
    await saveCredentialIfPresent(
      credentialKeyForEndpoint(asrEndpoint),
      asrType === "api" ? asrApiKey : "",
    );
    await saveCredentialIfPresent(
      "deepgram_api_key",
      asrType === "deepgram" ? deepgramApiKey : "",
    );
    await saveCredentialIfPresent(
      "assemblyai_api_key",
      asrType === "assemblyai" ? assemblyaiApiKey : "",
    );
    await saveCredentialIfPresent(
      credentialKeyForEndpoint(llmEndpoint),
      llmType === "api" ? llmApiKey : "",
    );
    await saveCredentialIfPresent(
      "openrouter_api_key",
      llmType === "openrouter" ? openrouterApiKey : "",
    );
    await saveCredentialIfPresent(
      "gemini_api_key",
      geminiAuthMode === "api_key" ? geminiApiKey : "",
    );

    if (asrType === "aws_transcribe" && awsAsrCredentialMode === "access_keys") {
      await saveCredentialIfPresent("aws_access_key", awsAsrAccessKey);
    }
    if (llmType === "aws_bedrock" && awsBedrockCredentialMode === "access_keys") {
      await saveCredentialIfPresent("aws_access_key", awsBedrockAccessKey);
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
          enable_diarization: awsAsrDiarization,
        };
        break;
      case "deepgram":
        asrProvider = {
          type: "deepgram",
          api_key: "",
          model: deepgramModel,
          enable_diarization: deepgramDiarization,
          endpointing_ms: Math.max(0, Math.round(deepgramEndpointingMs)),
          utterance_end_ms: Math.max(0, Math.round(deepgramUtteranceEndMs)),
          vad_events: deepgramVadEvents,
          eot_threshold: Math.max(0, Math.min(1, deepgramEotThreshold)),
          eager_eot_threshold: Math.max(
            0,
            Math.min(deepgramEotThreshold, deepgramEagerEotThreshold),
          ),
          eot_timeout_ms: Math.max(0, Math.round(deepgramEotTimeoutMs)),
        };
        break;
      case "assemblyai":
        asrProvider = {
          type: "assemblyai",
          api_key: "",
          enable_diarization: assemblyaiDiarization,
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
          base_url: openrouterBaseUrl || "https://openrouter.ai/api/v1",
          provider_order: null,
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
      llmType === "api" && llmEndpoint
        ? {
            endpoint: llmEndpoint,
            api_key: null,
            model: llmModel,
            max_tokens: llmMaxTokens,
            temperature: llmTemperature,
          }
        : null;

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
      llm_api_config: llmConfig,
      audio_settings: {
        sample_rate: audioSampleRate,
        channels: audioChannels,
      },
      gemini,
      log_level: logLevel,
      // Preserve the stored TTS-provider decision; the SettingsPage TTS
      // section will add a UI for this field in a follow-up. For now we
      // pass the existing value through unchanged so the AppSettings
      // shape is satisfied.
      // TTS provider is built from local state — the user picks it
      // through the UI section we added in Wave C / 0.1.0-rc1.
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

    // Persist AWS secret key + session token to credentials.yaml when the user
    // is using access_keys mode. ASR and Bedrock share the same credential
    // entries in the backend, so we prefer whichever form the user actually
    // filled in (ASR first, then Bedrock as fallback). We NEVER overwrite
    // stored credentials with empty strings — that would silently wipe them.
    const usingAwsAsrKeys =
      asrType === "aws_transcribe" && awsAsrCredentialMode === "access_keys";
    const usingAwsBedrockKeys =
      llmType === "aws_bedrock" && awsBedrockCredentialMode === "access_keys";

    if (usingAwsAsrKeys || usingAwsBedrockKeys) {
      const secretCandidate =
        (usingAwsAsrKeys && awsAsrSecretKey) ||
        (usingAwsBedrockKeys && awsBedrockSecretKey) ||
        "";
      if (secretCandidate) {
        try {
          await invoke("save_credential_cmd", {
            key: "aws_secret_key",
            value: secretCandidate,
          });
        } catch (e) {
          console.error("Failed to save aws_secret_key:", e);
        }
      }

      const sessionCandidate =
        (usingAwsAsrKeys && awsAsrSessionToken) ||
        (usingAwsBedrockKeys && awsBedrockSessionToken) ||
        "";
      if (sessionCandidate) {
        try {
          await invoke("save_credential_cmd", {
            key: "aws_session_token",
            value: sessionCandidate,
          });
        } catch (e) {
          console.error("Failed to save aws_session_token:", e);
        }
      }
    }

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

  // Apply a log-level change immediately (takes effect for every subsequent
  // `log::*!` macro on the backend) AND kick off persistence so it survives
  // restart. We intentionally call the dedicated command rather than relying
  // on the user clicking Save — a verbosity change is most useful *now*.
  const handleLogLevelChange = async (next: LogLevel) => {
    dispatch(setField("logLevel", next));
    try {
      await invoke("set_log_level", { level: next });
    } catch (e) {
      console.error("Failed to set log level:", e);
    }
  };

  const handleDeleteClick = (filename: string) => {
    if (confirmDelete === filename) {
      deleteModel(filename);
      dispatch({ type: "SET_CONFIRM_DELETE", filename: null });
    } else {
      dispatch({ type: "SET_CONFIRM_DELETE", filename });
    }
  };

  // ── Render ────────────────────────────────────────────────────────────
  return (
    <div className="settings-overlay" onClick={requestClose}>
      <div
        ref={modalRef}
        className="settings-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-header-title"
        tabIndex={-1}
      >
        {/* Header */}
        <div className="settings-header">
          <h2
            id="settings-header-title"
            className="settings-header__title"
          >
            {t("settings.title")}
          </h2>
          <IconButton
            icon="close"
            label={t("settings.close")}
            variant="ghost"
            className="settings-header__close"
            onClick={requestClose}
          />
        </div>

        {settingsLoading ? (
          <div className="settings-content settings-content--loading">
            <p>{t("settings.loading")}</p>
          </div>
        ) : (
          <div className="settings-content">
            <div className="settings-tabs" role="tablist">
              {SETTINGS_TABS.map((tab) => (
                <button
                  key={tab.id}
                  type="button"
                  role="tab"
                  aria-selected={activeTab === tab.id}
                  className={`settings-tab ${activeTab === tab.id ? "settings-tab--active" : ""}`}
                  onClick={() => setActiveTab(tab.id)}
                >
                  {tab.label}
                </button>
              ))}
            </div>

            {activeTab === "general" && (
              <>
                <AudioSettings state={state} dispatch={dispatch} t={t} />
                <CredentialsManager
                  state={state}
                  t={t}
                  models={models}
                  modelStatus={modelStatus}
                  isDownloading={isDownloading}
                  isDeletingModel={isDeletingModel}
                  downloadProgress={downloadProgress}
                  downloadModel={downloadModel}
                  handleDeleteClick={handleDeleteClick}
                  handleLogLevelChange={handleLogLevelChange}
                />
              </>
            )}
            {activeTab === "stt" && (
              <AsrProviderSettings
                state={state}
                dispatch={dispatch}
                t={t}
                modelStatus={modelStatus}
                refreshAwsProfiles={refreshAwsProfiles}
                handleTestAsrApi={handleTestAsrApi}
                handleTestDeepgram={handleTestDeepgram}
                handleTestAssemblyAI={handleTestAssemblyAI}
                handleTestAwsAsr={handleTestAwsAsr}
                handleClearCredential={handleClearCredential}
                renderTestResult={renderTestResult}
              />
            )}
            {activeTab === "llm" && (
              <LlmProviderSettings
                state={state}
                dispatch={dispatch}
                t={t}
                modelStatus={modelStatus}
                refreshAwsProfiles={refreshAwsProfiles}
                handleTestAwsBedrock={handleTestAwsBedrock}
                handleTestOpenRouter={handleTestOpenRouter}
                handleRefreshOpenRouterModels={handleRefreshOpenRouterModels}
                handleClearCredential={handleClearCredential}
                renderTestResult={renderTestResult}
              />
            )}
            {activeTab === "gemini" && (
              <>
                <section className="settings-section">
                  <h3 className="settings-section-title">Conversation mode</h3>
                  <p className="settings-section-help">
                    Choose how spoken audio is processed. Cascading runs the
                    pipeline Speech-to-Text → Language Model → Text-to-Speech.
                    Native speech-to-speech streams audio directly to a realtime
                    model (Gemini Live / OpenAI gpt-realtime) — enabling this
                    shows the Gemini control in the top bar.
                  </p>
                  <div className="settings-field settings-field--inline">
                    <label>
                      <input
                        type="checkbox"
                        checked={nativeS2sEnabled}
                        onChange={(e) => setNativeS2sEnabled(e.target.checked)}
                      />
                      {" "}Enable native speech-to-speech (shows Gemini in the top bar)
                    </label>
                  </div>
                </section>
                <GeminiSettings
                  state={state}
                  dispatch={dispatch}
                  t={t}
                  handleTestGemini={handleTestGemini}
                  renderTestResult={renderTestResult}
                />
              </>
            )}

            {activeTab === "tts" && (
            <>
            {/* ── Text-to-Speech (Wave C / ADR-0004 + ADR-0006) ─────────── */}
            <section className="settings-section">
              <h3 className="settings-section-title">
                Text-to-Speech &amp; Speak-aloud
              </h3>
              <p className="settings-section-help">
                Optional. When enabled, chatbot replies are spoken aloud
                through your output device using Deepgram Aura. The same
                Deepgram API key works for STT and TTS.
              </p>

              <div className="settings-field">
                <label htmlFor="tts-provider-select">Provider</label>
                <select
                  id="tts-provider-select"
                  value={ttsType}
                  onChange={(e) =>
                    setTtsType(e.target.value as "none" | "deepgram_aura")
                  }
                  disabled={settingsLoading}
                >
                  <option value="none">None (text-only chat)</option>
                  <option value="deepgram_aura">Deepgram Aura</option>
                </select>
              </div>

              {ttsType === "deepgram_aura" && (
                <>
                  <div className="settings-field">
                    <label htmlFor="aura-voice-select">Voice</label>
                    <select
                      id="aura-voice-select"
                      value={auraVoice}
                      onChange={(e) => setAuraVoice(e.target.value)}
                      disabled={settingsLoading}
                    >
                      {TTS_AURA_VOICES.map((v) => (
                        <option key={v.id} value={v.id}>
                          {v.label}
                        </option>
                      ))}
                    </select>
                  </div>

                  <div className="settings-field">
                    <label htmlFor="aura-speed-input">Speed (0.7 – 1.5)</label>
                    <input
                      id="aura-speed-input"
                      type="number"
                      step="0.1"
                      min="0.7"
                      max="1.5"
                      value={auraSpeed}
                      onChange={(e) =>
                        setAuraSpeed(
                          Math.max(0.7, Math.min(1.5, Number(e.target.value))),
                        )
                      }
                      disabled={settingsLoading}
                    />
                  </div>

                  <div className="settings-field settings-field--inline">
                    <label htmlFor="speak-aloud-toggle">
                      <input
                        id="speak-aloud-toggle"
                        type="checkbox"
                        checked={speakAloud}
                        onChange={(e) => setSpeakAloud(e.target.checked)}
                        disabled={settingsLoading}
                      />
                      &nbsp;Speak chatbot replies aloud
                    </label>
                  </div>

                  <div className="settings-field">
                    <button
                      type="button"
                      className="settings-btn"
                      onClick={handleTestTts}
                      disabled={
                        settingsLoading || testingTts || !deepgramApiKey
                      }
                    >
                      {testingTts ? "Testing…" : "Test Connection"}
                    </button>
                    {!deepgramApiKey && (
                      <p className="settings-hint">
                        Save a Deepgram API key in the ASR section above first.
                      </p>
                    )}
                    {ttsTestResult && (
                      <div
                        className={
                          ttsTestResult.ok
                            ? "settings-test-result settings-test-result--ok"
                            : "settings-test-result settings-test-result--err"
                        }
                      >
                        {ttsTestResult.msg}
                      </div>
                    )}
                  </div>
                </>
              )}
            </section>
            </>
            )}

            {activeTab === "logging" && <LoggingSettings />}
          </div>
        )}

        {/* Footer */}
        <div className="settings-footer">
          {confirmingClose && (
            <div className="settings-confirm-close" role="alertdialog" aria-label={t("settings.confirmClose.prompt")}>
              <span className="settings-confirm-close__text">
                {t("settings.confirmClose.prompt")}
              </span>
              <button
                type="button"
                className="settings-btn settings-btn--secondary"
                onClick={() => setConfirmingClose(false)}
              >
                {t("settings.confirmClose.keepEditing")}
              </button>
              <button
                type="button"
                className="settings-btn settings-btn--danger"
                onClick={handleDiscardAndClose}
              >
                {t("settings.confirmClose.discard")}
              </button>
            </div>
          )}
          <button
            className="settings-btn settings-btn--primary"
            onClick={handleSave}
            disabled={settingsLoading}
          >
            {t("settings.buttons.save")}
          </button>
        </div>
      </div>
    </div>
  );
}

export default SettingsPage;
