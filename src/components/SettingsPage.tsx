import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { useAudioGraphStore } from "../store";
import { useFocusTrap } from "../hooks/useFocusTrap";
import type {
  AsrProvider,
  AwsCredentialSource,
  GeminiAuthMode,
  GeminiSettings,
  LlmApiConfig,
  LlmProvider,
  ModelReadiness,
} from "../types";

/** Format bytes to a human-readable size string (e.g. "466 MB"). */
function formatSize(bytes: number | null): string {
  if (bytes === null || bytes === 0) return "—";
  const mb = bytes / (1024 * 1024);
  if (mb >= 1024) {
    return `${(mb / 1024).toFixed(1)} GB`;
  }
  return `${Math.round(mb)} MB`;
}

/** Map a ModelReadiness value to a CSS modifier and translation key. */
function readinessBadge(status: ModelReadiness): {
  cls: string;
  labelKey: string;
} {
  switch (status) {
    case "Ready":
      return { cls: "status-badge--ready", labelKey: "settings.modelReadiness.ready" };
    case "NotDownloaded":
      return {
        cls: "status-badge--not-downloaded",
        labelKey: "settings.modelReadiness.notDownloaded",
      };
    case "Invalid":
      return { cls: "status-badge--invalid", labelKey: "settings.modelReadiness.invalid" };
  }
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

  // ── Local form state ──────────────────────────────────────────────────
  const [asrType, setAsrType] = useState<
    "local_whisper" | "api" | "aws_transcribe" | "deepgram" | "assemblyai" | "sherpa_onnx"
  >("local_whisper");
  const [whisperModel, setWhisperModel] = useState("ggml-small.en.bin");
  const [asrEndpoint, setAsrEndpoint] = useState("");
  const [asrApiKey, setAsrApiKey] = useState("");
  const [asrModel, setAsrModel] = useState("");

  // AWS Transcribe fields
  const [awsAsrRegion, setAwsAsrRegion] = useState("us-east-1");
  const [awsAsrLanguageCode, setAwsAsrLanguageCode] = useState("en-US");
  const [awsAsrCredentialMode, setAwsAsrCredentialMode] = useState<
    "default_chain" | "profile" | "access_keys"
  >("default_chain");
  const [awsAsrProfileName, setAwsAsrProfileName] = useState("");
  const [awsAsrAccessKey, setAwsAsrAccessKey] = useState("");
  const [awsAsrSecretKey, setAwsAsrSecretKey] = useState("");
  const [awsAsrSessionToken, setAwsAsrSessionToken] = useState("");
  const [awsAsrDiarization, setAwsAsrDiarization] = useState(true);

  // Deepgram fields
  const [deepgramApiKey, setDeepgramApiKey] = useState("");
  const [deepgramModel, setDeepgramModel] = useState("nova-3");
  const [deepgramDiarization, setDeepgramDiarization] = useState(true);

  // AssemblyAI fields
  const [assemblyaiApiKey, setAssemblyaiApiKey] = useState("");
  const [assemblyaiDiarization, setAssemblyaiDiarization] = useState(true);

  // Sherpa-ONNX fields
  const [sherpaModelDir, setSherpaModelDir] = useState("streaming-zipformer-en-20M");
  const [sherpaEndpointDetection, setSherpaEndpointDetection] = useState(true);

  const [llmType, setLlmType] = useState<"local_llama" | "api" | "aws_bedrock" | "mistralrs">(
    "api",
  );
  const [llmEndpoint, setLlmEndpoint] = useState("http://localhost:11434/v1");
  const [llmApiKey, setLlmApiKey] = useState("");
  const [llmModel, setLlmModel] = useState("llama3.2");
  const [llmMaxTokens, setLlmMaxTokens] = useState(2048);
  const [llmTemperature, setLlmTemperature] = useState(0.7);

  // Mistral.rs fields
  const [mistralrsModelId, setMistralrsModelId] = useState("ggml-small-extract.gguf");

  // AWS Bedrock fields
  const [awsBedrockRegion, setAwsBedrockRegion] = useState("us-east-1");
  const [awsBedrockModelId, setAwsBedrockModelId] = useState("");
  const [awsBedrockCredentialMode, setAwsBedrockCredentialMode] = useState<
    "default_chain" | "profile" | "access_keys"
  >("default_chain");
  const [awsBedrockProfileName, setAwsBedrockProfileName] = useState("");
  const [awsBedrockAccessKey, setAwsBedrockAccessKey] = useState("");
  const [awsBedrockSecretKey, setAwsBedrockSecretKey] = useState("");
  const [awsBedrockSessionToken, setAwsBedrockSessionToken] = useState("");

  // Gemini settings
  const [geminiAuthMode, setGeminiAuthMode] = useState<"api_key" | "vertex_ai">(
    "api_key",
  );
  const [geminiApiKey, setGeminiApiKey] = useState("");
  const [geminiModel, setGeminiModel] = useState("gemini-3.1-flash-live-preview");
  const [geminiProjectId, setGeminiProjectId] = useState("");
  const [geminiLocation, setGeminiLocation] = useState("");
  const [geminiServiceAccountPath, setGeminiServiceAccountPath] = useState("");

  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  // ── Audio capture settings ───────────────────────────────────────────
  // Sample rate + channels that flow into rsac's AudioCaptureBuilder on the
  // Rust side. The pipeline still downmixes to 16 kHz mono for ASR
  // regardless — these only control what the OS / driver is asked to
  // deliver. The allowed values mirror the Rust `sample_rate_is_valid` /
  // `channels_is_valid` whitelists (task #79); keep them in sync.
  type SampleRate = 16000 | 22050 | 44100 | 48000 | 88200 | 96000;
  type ChannelCount = 1 | 2;
  const [audioSampleRate, setAudioSampleRate] = useState<SampleRate>(16000);
  const [audioChannels, setAudioChannels] = useState<ChannelCount>(1);

  // ── Diagnostics: runtime log level ───────────────────────────────────
  // Kept as a plain string so an unknown value from an older settings file
  // round-trips unchanged; the backend's parse_level() coerces anything it
  // doesn't recognise to Info.
  type LogLevel = "off" | "error" | "warn" | "info" | "debug" | "trace";
  const [logLevel, setLogLevel] = useState<LogLevel>("info");

  // ── AWS profile dropdown ─────────────────────────────────────────────
  // Populated from `list_aws_profiles` Tauri command (parses ~/.aws/config
  // and ~/.aws/credentials). Shared by both the AWS Transcribe (ASR) and
  // AWS Bedrock (LLM) "Profile" credential modes.
  const [awsProfiles, setAwsProfiles] = useState<string[]>([]);
  const refreshAwsProfiles = async () => {
    setAwsProfiles(await listAwsProfiles());
  };

  // ── Test connection state ────────────────────────────────────────────
  // Each cloud provider gets its own result slot (keyed by provider name)
  // so multiple tests can coexist without clobbering each other.
  type TestKey = "asr_api" | "deepgram" | "assemblyai" | "gemini" | "aws_asr" | "aws_bedrock";
  const [testResults, setTestResults] = useState<
    Partial<Record<TestKey, { ok: boolean; msg: string }>>
  >({});
  const [testingKey, setTestingKey] = useState<TestKey | null>(null);

  // Upper bound on any Test Connection invocation. Without this, a hung
  // network call (e.g. provider stuck in TLS handshake, firewall silently
  // dropping packets) leaves the button forever stuck on "Testing…".
  const TEST_TIMEOUT_MS = 10_000;

  const runTest = async (
    key: TestKey,
    invocation: () => Promise<string>,
  ) => {
    // Debounce: reject rapid re-clicks while a test is already in flight.
    // `testingKey` is already used to disable buttons, but users can click
    // a different provider's button mid-test — we swallow those instead
    // of racing two concurrent backend calls.
    if (testingKey !== null) return;
    setTestingKey(key);
    setTestResults((prev) => ({ ...prev, [key]: undefined }));
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
      setTestResults((prev) => ({ ...prev, [key]: { ok: true, msg } }));
    } catch (e) {
      setTestResults((prev) => ({ ...prev, [key]: { ok: false, msg: String(e) } }));
    } finally {
      setTestingKey(null);
    }
  };

  // Clear a stored credential (mirrors the Rust `delete_credential` path).
  // Uses window.confirm since the app is single-user and this isn't
  // destructive to uncommitted UI state — only to the on-disk YAML.
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
      window.alert(t("settings.errors.failedToClear", { error: String(e) }));
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
        {r.ok ? "✓ " : "✗ "}
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
    const ALLOWED_RATES: SampleRate[] = [16000, 22050, 44100, 48000, 88200, 96000];
    const ALLOWED_CHANNELS: ChannelCount[] = [1, 2];
    const sr = settings.audio_settings?.sample_rate;
    const ch = settings.audio_settings?.channels;
    setAudioSampleRate(
      ALLOWED_RATES.includes(sr as SampleRate) ? (sr as SampleRate) : 16000,
    );
    setAudioChannels(
      ALLOWED_CHANNELS.includes(ch as ChannelCount) ? (ch as ChannelCount) : 1,
    );

    // Whisper model selection
    if (settings.whisper_model) {
      setWhisperModel(settings.whisper_model);
    }

    // ASR provider
    const asr = settings.asr_provider;
    setAsrType(asr.type);
    if (asr.type === "api") {
      setAsrEndpoint(asr.endpoint);
      setAsrApiKey(asr.api_key);
      setAsrModel(asr.model);
    } else if (asr.type === "aws_transcribe") {
      setAwsAsrRegion(asr.region);
      setAwsAsrLanguageCode(asr.language_code);
      setAwsAsrDiarization(asr.enable_diarization);
      const cred = asr.credential_source;
      setAwsAsrCredentialMode(cred.type);
      if (cred.type === "profile") setAwsAsrProfileName(cred.name);
      if (cred.type === "access_keys") setAwsAsrAccessKey(cred.access_key);
    } else if (asr.type === "deepgram") {
      setDeepgramApiKey(asr.api_key);
      setDeepgramModel(asr.model);
      setDeepgramDiarization(asr.enable_diarization);
    } else if (asr.type === "assemblyai") {
      setAssemblyaiApiKey(asr.api_key);
      setAssemblyaiDiarization(asr.enable_diarization);
    } else if (asr.type === "sherpa_onnx") {
      setAsrType("sherpa_onnx");
      setSherpaModelDir(asr.model_dir);
      setSherpaEndpointDetection(asr.enable_endpoint_detection);
    }

    // LLM provider
    const llm = settings.llm_provider;
    setLlmType(llm.type);
    if (llm.type === "api") {
      setLlmEndpoint(llm.endpoint);
      setLlmApiKey(llm.api_key);
      setLlmModel(llm.model);
    } else if (llm.type === "aws_bedrock") {
      setAwsBedrockRegion(llm.region);
      setAwsBedrockModelId(llm.model_id);
      const cred = llm.credential_source;
      setAwsBedrockCredentialMode(cred.type);
      if (cred.type === "profile") setAwsBedrockProfileName(cred.name);
      if (cred.type === "access_keys") setAwsBedrockAccessKey(cred.access_key);
    } else if (settings.llm_provider.type === "mistralrs") {
      setLlmType("mistralrs");
      setMistralrsModelId(settings.llm_provider.model_id);
    }

    // LLM config (advanced — max_tokens / temperature)
    if (settings.llm_api_config) {
      setLlmMaxTokens(settings.llm_api_config.max_tokens);
      setLlmTemperature(settings.llm_api_config.temperature);
    }

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
    setLogLevel(LOG_LEVELS.includes(raw) ? raw : "info");

    // Gemini settings
    if (settings.gemini) {
      setGeminiModel(settings.gemini.model);
      const auth = settings.gemini.auth;
      setGeminiAuthMode(auth.type);
      if (auth.type === "api_key") {
        setGeminiApiKey(auth.api_key);
      } else if (auth.type === "vertex_ai") {
        setGeminiProjectId(auth.project_id);
        setGeminiLocation(auth.location);
        setGeminiServiceAccountPath(auth.service_account_path ?? "");
      }
    }

    // Pre-populate AWS secret key + session token from credentials.yaml.
    // Both AWS ASR and AWS Bedrock share the same aws_secret_key / aws_session_token
    // in the backend credential store, so we load once and mirror into both forms.
    (async () => {
      try {
        const secret = await invoke<string | null>("load_credential_cmd", {
          key: "aws_secret_key",
        });
        if (secret) {
          setAwsAsrSecretKey(secret);
          setAwsBedrockSecretKey(secret);
        }
      } catch {
        // Silently tolerate missing credentials.
      }
      try {
        const token = await invoke<string | null>("load_credential_cmd", {
          key: "aws_session_token",
        });
        if (token) {
          setAwsAsrSessionToken(token);
          setAwsBedrockSessionToken(token);
        }
      } catch {
        // Silently tolerate missing credentials.
      }
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

  // ── Helpers ───────────────────────────────────────────────────────────
  const buildAwsCredentialSource = (
    mode: "default_chain" | "profile" | "access_keys",
    profileName: string,
    accessKey: string,
  ): AwsCredentialSource => {
    switch (mode) {
      case "profile":
        return { type: "profile", name: profileName };
      case "access_keys":
        return { type: "access_keys", access_key: accessKey };
      default:
        return { type: "default_chain" };
    }
  };

  // ── Handlers ──────────────────────────────────────────────────────────
  const handleSave = async () => {
    let asrProvider: AsrProvider;
    switch (asrType) {
      case "api":
        asrProvider = {
          type: "api",
          endpoint: asrEndpoint,
          api_key: asrApiKey,
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
            awsAsrAccessKey,
          ),
          enable_diarization: awsAsrDiarization,
        };
        break;
      case "deepgram":
        asrProvider = {
          type: "deepgram",
          api_key: deepgramApiKey,
          model: deepgramModel,
          enable_diarization: deepgramDiarization,
        };
        break;
      case "assemblyai":
        asrProvider = {
          type: "assemblyai",
          api_key: assemblyaiApiKey,
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
          api_key: llmApiKey,
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
            awsBedrockAccessKey,
          ),
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
            api_key: llmApiKey || null,
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
        : { type: "api_key", api_key: geminiApiKey };

    const gemini: GeminiSettings = {
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
  };

  // Apply a log-level change immediately (takes effect for every subsequent
  // `log::*!` macro on the backend) AND kick off persistence so it survives
  // restart. We intentionally call the dedicated command rather than relying
  // on the user clicking Save — a verbosity change is most useful *now*.
  const handleLogLevelChange = async (next: LogLevel) => {
    setLogLevel(next);
    try {
      await invoke("set_log_level", { level: next });
    } catch (e) {
      console.error("Failed to set log level:", e);
    }
  };

  const handleDeleteClick = (filename: string) => {
    if (confirmDelete === filename) {
      deleteModel(filename);
      setConfirmDelete(null);
    } else {
      setConfirmDelete(filename);
    }
  };

  // ── Render ────────────────────────────────────────────────────────────
  return (
    <div className="settings-overlay" onClick={closeSettings}>
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
          <button
            className="settings-header__close"
            onClick={closeSettings}
            aria-label={t("settings.close")}
          >
            ✕
          </button>
        </div>

        {settingsLoading ? (
          <div className="settings-content settings-content--loading">
            <p>{t("settings.loading")}</p>
          </div>
        ) : (
          <div className="settings-content">
            {/* ── Audio Capture Section ──────────────────────────── */}
            {/* Task #79: expose sample_rate + channels that flow into     */}
            {/* rsac. The downstream pipeline always resamples to 16 kHz  */}
            {/* mono for ASR, so these controls only change what the OS   */}
            {/* driver delivers — useful for matching a specific          */}
            {/* interface's native rate (e.g. studio interfaces at 96 k). */}
            <div className="settings-section">
              <h3 className="settings-section__title">
                {t("settings.sections.audio")}
              </h3>
              <div className="settings-section__api-fields">
                <div className="settings-field">
                  <label
                    className="settings-field__label"
                    htmlFor="audio-sample-rate-select"
                  >
                    {t("settings.fields.captureSampleRate")}
                  </label>
                  <select
                    id="audio-sample-rate-select"
                    className="settings-input"
                    value={audioSampleRate}
                    onChange={(e) =>
                      setAudioSampleRate(Number(e.target.value) as SampleRate)
                    }
                  >
                    <option value={16000}>{t("settings.sampleRates.hz16000")}</option>
                    <option value={22050}>{t("settings.sampleRates.hz22050")}</option>
                    <option value={44100}>{t("settings.sampleRates.hz44100")}</option>
                    <option value={48000}>{t("settings.sampleRates.hz48000")}</option>
                    <option value={88200}>{t("settings.sampleRates.hz88200")}</option>
                    <option value={96000}>{t("settings.sampleRates.hz96000")}</option>
                  </select>
                </div>
                <div className="settings-field">
                  <label
                    className="settings-field__label"
                    htmlFor="audio-channels-select"
                  >
                    {t("settings.fields.captureChannels")}
                  </label>
                  <select
                    id="audio-channels-select"
                    className="settings-input"
                    value={audioChannels}
                    onChange={(e) =>
                      setAudioChannels(Number(e.target.value) as ChannelCount)
                    }
                  >
                    <option value={1}>{t("settings.channels.mono")}</option>
                    <option value={2}>{t("settings.channels.stereo")}</option>
                  </select>
                  <p className="settings-hint">
                    {t("settings.hints.audioDownmix")}
                  </p>
                </div>
              </div>
            </div>

            {/* ── Models Section ─────────────────────────────────── */}
            <div className="settings-section">
              <h3 className="settings-section__title">{t("settings.sections.models")}</h3>
              {models.map((model) => {
                const status =
                  modelStatus && model.name.toLowerCase().includes("whisper")
                    ? modelStatus.whisper
                    : modelStatus && model.name.toLowerCase().includes("sortformer")
                      ? modelStatus.sortformer
                      : modelStatus
                        ? modelStatus.llm
                        : ("NotDownloaded" as ModelReadiness);

                const badge = readinessBadge(status);
                const isThisDownloading =
                  isDownloading && downloadProgress?.model_name === model.name;
                const isThisDeleting = isDeletingModel === model.filename;

                return (
                  <div className="model-card" key={model.filename}>
                    <div className="model-card__header">
                      <div>
                        <span className="model-card__name">{model.name}</span>
                        <span className={`status-badge ${badge.cls}`}>
                          {t(badge.labelKey)}
                        </span>
                      </div>
                      <span className="model-card__size">
                        {formatSize(model.size_bytes)}
                      </span>
                    </div>
                    {model.description && (
                      <p className="model-card__description">
                        {model.description}
                      </p>
                    )}

                    <div className="model-card__actions">
                      {!model.is_downloaded && (
                        <button
                          className="settings-btn settings-btn--primary"
                          onClick={() => downloadModel(model.filename)}
                          disabled={isDownloading}
                        >
                          {isThisDownloading ? t("settings.buttons.downloading") : t("settings.buttons.download")}
                        </button>
                      )}
                      {model.is_downloaded && (
                        <button
                          className="settings-btn settings-btn--danger"
                          onClick={() => handleDeleteClick(model.filename)}
                          disabled={isThisDeleting}
                        >
                          {isThisDeleting
                            ? t("settings.buttons.deleting")
                            : confirmDelete === model.filename
                              ? t("settings.buttons.confirmDelete")
                              : t("settings.buttons.delete")}
                        </button>
                      )}
                    </div>

                    {/* Download progress bar */}
                    {isThisDownloading && downloadProgress && (
                      <div className="download-progress">
                        <div
                          className="download-progress__bar"
                          style={{ width: `${downloadProgress.percent}%` }}
                        />
                      </div>
                    )}
                  </div>
                );
              })}
              {models.length === 0 && (
                <p className="settings-section__empty">{t("settings.models.empty")}</p>
              )}
            </div>

            {/* ── ASR Provider Section ───────────────────────────── */}
            <div className="settings-section">
              <h3 className="settings-section__title">{t("settings.sections.asr")}</h3>
              <div className="settings-radio-group">
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="asr-provider"
                    checked={asrType === "local_whisper"}
                    onChange={() => setAsrType("local_whisper")}
                  />
                  <span>{t("settings.asrProviders.localWhisper")}</span>
                  {asrType === "local_whisper" && modelStatus && (
                    <span
                      className={`status-badge ${readinessBadge(modelStatus.whisper).cls}`}
                    >
                      {t(readinessBadge(modelStatus.whisper).labelKey)}
                    </span>
                  )}
                </label>

              {asrType === "local_whisper" && (
                <div className="settings-section__api-fields">
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.whisperModelSize")}</label>
                    <select
                      className="settings-input"
                      value={whisperModel}
                      onChange={(e) => setWhisperModel(e.target.value)}
                    >
                      <option value="ggml-tiny.en.bin">{t("settings.whisperModels.tiny")}</option>
                      <option value="ggml-base.en.bin">{t("settings.whisperModels.base")}</option>
                      <option value="ggml-small.en.bin">{t("settings.whisperModels.small")}</option>
                      <option value="ggml-medium.en.bin">{t("settings.whisperModels.medium")}</option>
                      <option value="ggml-large-v3.bin">{t("settings.whisperModels.large")}</option>
                    </select>
                  </div>
                </div>
              )}

                <label className="settings-radio">
                  <input
                    type="radio"
                    name="asr-provider"
                    checked={asrType === "api"}
                    onChange={() => setAsrType("api")}
                  />
                  <span>{t("settings.asrProviders.cloudApi")}</span>
                </label>
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="asr-provider"
                    checked={asrType === "aws_transcribe"}
                    onChange={() => setAsrType("aws_transcribe")}
                  />
                  <span>{t("settings.asrProviders.awsTranscribe")}</span>
                </label>
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="asr-provider"
                    checked={asrType === "deepgram"}
                    onChange={() => setAsrType("deepgram")}
                  />
                  <span>{t("settings.asrProviders.deepgram")}</span>
                </label>
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="asr-provider"
                    checked={asrType === "assemblyai"}
                    onChange={() => setAsrType("assemblyai")}
                  />
                  <span>{t("settings.asrProviders.assemblyai")}</span>
                </label>
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="asr-provider"
                    checked={asrType === "sherpa_onnx"}
                    onChange={() => setAsrType("sherpa_onnx")}
                  />
                  <span>{t("settings.asrProviders.sherpaOnnx")}</span>
                </label>
              </div>

              {asrType === "api" && (
                <div className="settings-section__api-fields">
                  <div className="settings-field">
                    <label className="settings-field__label">
                      {t("settings.fields.endpoint")}
                    </label>
                    <input
                      className="settings-input"
                      type="text"
                      value={asrEndpoint}
                      onChange={(e) => setAsrEndpoint(e.target.value)}
                      placeholder="https://api.openai.com/v1"
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.apiKey")}</label>
                    <input
                      className="settings-input"
                      type="password"
                      value={asrApiKey}
                      onChange={(e) => setAsrApiKey(e.target.value)}
                      placeholder="sk-..."
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.model")}</label>
                    <input
                      className="settings-input"
                      type="text"
                      value={asrModel}
                      onChange={(e) => setAsrModel(e.target.value)}
                      placeholder="whisper-1"
                    />
                  </div>
                  <div className="settings-field">
                    <button
                      type="button"
                      className="settings-btn settings-btn--secondary"
                      disabled={testingKey !== null || !asrEndpoint}
                      onClick={handleTestAsrApi}
                    >
                      {testingKey === "asr_api" ? t("settings.buttons.testing") : t("settings.buttons.testConnection")}
                    </button>
                    {renderTestResult("asr_api")}
                  </div>
                </div>
              )}

              {asrType === "aws_transcribe" && (
                <div className="settings-section__api-fields">
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.region")}</label>
                    <input
                      className="settings-input"
                      type="text"
                      value={awsAsrRegion}
                      onChange={(e) => setAwsAsrRegion(e.target.value)}
                      placeholder="us-east-1"
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">
                      {t("settings.fields.languageCode")}
                    </label>
                    <input
                      className="settings-input"
                      type="text"
                      value={awsAsrLanguageCode}
                      onChange={(e) => setAwsAsrLanguageCode(e.target.value)}
                      placeholder="en-US"
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">
                      {t("settings.fields.credentialMode")}
                    </label>
                    <select
                      className="settings-input"
                      value={awsAsrCredentialMode}
                      onChange={(e) =>
                        setAwsAsrCredentialMode(
                          e.target.value as
                            | "default_chain"
                            | "profile"
                            | "access_keys",
                        )
                      }
                    >
                      <option value="default_chain">{t("settings.credentialModes.defaultChain")}</option>
                      <option value="profile">{t("settings.credentialModes.profile")}</option>
                      <option value="access_keys">{t("settings.credentialModes.accessKeys")}</option>
                    </select>
                  </div>
                  {awsAsrCredentialMode === "profile" && (
                    <div className="settings-field">
                      <label className="settings-field__label">
                        {t("settings.fields.awsProfile")}
                      </label>
                      <div className="settings-inline-row">
                        <select
                          className="settings-input"
                          value={awsAsrProfileName}
                          onChange={(e) =>
                            setAwsAsrProfileName(e.target.value)
                          }
                        >
                          <option value="">{t("settings.placeholders.selectProfile")}</option>
                          {awsProfiles.map((name) => (
                            <option key={name} value={name}>
                              {name}
                            </option>
                          ))}
                        </select>
                        <button
                          type="button"
                          className="settings-btn settings-btn--secondary"
                          onClick={refreshAwsProfiles}
                        >
                          {t("settings.buttons.refresh")}
                        </button>
                      </div>
                      {awsProfiles.length === 0 && (
                        <p className="settings-hint">
                          {t("settings.hints.noAwsProfiles")}{" "}
                          <code>aws configure</code>{" "}
                          {t("settings.hints.noAwsProfilesSuffix")}
                        </p>
                      )}
                    </div>
                  )}
                  {awsAsrCredentialMode === "access_keys" && (
                    <>
                      <div className="settings-field">
                        <label className="settings-field__label">
                          {t("settings.fields.accessKeyId")}
                        </label>
                        <input
                          className="settings-input"
                          type="password"
                          value={awsAsrAccessKey}
                          onChange={(e) => setAwsAsrAccessKey(e.target.value)}
                          placeholder="AKIA..."
                        />
                      </div>
                      <div className="settings-field">
                        <label className="settings-field__label">
                          {t("settings.fields.secretAccessKey")}
                        </label>
                        <input
                          className="settings-input"
                          type="password"
                          value={awsAsrSecretKey}
                          onChange={(e) => setAwsAsrSecretKey(e.target.value)}
                          placeholder="wJalr..."
                        />
                      </div>
                      <div className="settings-field">
                        <label className="settings-field__label">
                          {t("settings.fields.sessionTokenOptional")}
                        </label>
                        <input
                          className="settings-input"
                          type="password"
                          value={awsAsrSessionToken}
                          onChange={(e) =>
                            setAwsAsrSessionToken(e.target.value)
                          }
                          placeholder={t("settings.placeholders.sessionTokenHint")}
                        />
                      </div>
                      <div className="settings-field">
                        <button
                          type="button"
                          className="settings-btn settings-btn--danger"
                          onClick={() =>
                            // AWS secret + token are shared between ASR and Bedrock
                            // forms, so clear both UI mirrors at once.
                            handleClearCredential(
                              "aws_secret_key",
                              t("settings.credentialConfirm.awsKeysLabel"),
                              () => {
                                setAwsAsrSecretKey("");
                                setAwsBedrockSecretKey("");
                                setAwsAsrSessionToken("");
                                setAwsBedrockSessionToken("");
                                // Also drop the session token entry from the
                                // store; keep calls sequential so one failure
                                // doesn't leave a half-cleared state silently.
                                invoke("delete_credential_cmd", {
                                  key: "aws_session_token",
                                }).catch((e) =>
                                  console.error(
                                    "Failed to clear aws_session_token:",
                                    e,
                                  ),
                                );
                              },
                            )
                          }
                        >
                          {t("settings.buttons.clearSavedAwsKeys")}
                        </button>
                      </div>
                    </>
                  )}
                  <div className="settings-field">
                    <label className="settings-radio">
                      <input
                        type="checkbox"
                        checked={awsAsrDiarization}
                        onChange={(e) => setAwsAsrDiarization(e.target.checked)}
                      />
                      <span>{t("settings.fields.enableDiarization")}</span>
                    </label>
                  </div>
                  <div className="settings-field">
                    <button
                      type="button"
                      className="settings-btn settings-btn--secondary"
                      disabled={testingKey !== null || !awsAsrRegion}
                      onClick={handleTestAwsAsr}
                    >
                      {testingKey === "aws_asr" ? t("settings.buttons.testing") : t("settings.buttons.testConnection")}
                    </button>
                    {renderTestResult("aws_asr")}
                  </div>
                </div>
              )}

              {asrType === "deepgram" && (
                <div className="settings-section__api-fields">
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.apiKey")}</label>
                    <input
                      className="settings-input"
                      type="password"
                      value={deepgramApiKey}
                      onChange={(e) => setDeepgramApiKey(e.target.value)}
                      placeholder="dg-..."
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.model")}</label>
                    <input
                      className="settings-input"
                      type="text"
                      value={deepgramModel}
                      onChange={(e) => setDeepgramModel(e.target.value)}
                      placeholder="nova-3"
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-radio">
                      <input
                        type="checkbox"
                        checked={deepgramDiarization}
                        onChange={(e) =>
                          setDeepgramDiarization(e.target.checked)
                        }
                      />
                      <span>{t("settings.fields.enableDiarization")}</span>
                    </label>
                  </div>
                  <div className="settings-field">
                    <button
                      type="button"
                      className="settings-btn settings-btn--secondary"
                      disabled={testingKey !== null || !deepgramApiKey}
                      onClick={handleTestDeepgram}
                    >
                      {testingKey === "deepgram" ? t("settings.buttons.testing") : t("settings.buttons.testConnection")}
                    </button>
                    {renderTestResult("deepgram")}
                  </div>
                </div>
              )}

              {asrType === "assemblyai" && (
                <div className="settings-section__api-fields">
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.apiKey")}</label>
                    <input
                      className="settings-input"
                      type="password"
                      value={assemblyaiApiKey}
                      onChange={(e) => setAssemblyaiApiKey(e.target.value)}
                      placeholder={t("settings.placeholders.assemblyaiApiKey")}
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-radio">
                      <input
                        type="checkbox"
                        checked={assemblyaiDiarization}
                        onChange={(e) =>
                          setAssemblyaiDiarization(e.target.checked)
                        }
                      />
                      <span>{t("settings.fields.enableDiarization")}</span>
                    </label>
                  </div>
                  <div className="settings-field">
                    <button
                      type="button"
                      className="settings-btn settings-btn--secondary"
                      disabled={testingKey !== null || !assemblyaiApiKey}
                      onClick={handleTestAssemblyAI}
                    >
                      {testingKey === "assemblyai" ? t("settings.buttons.testing") : t("settings.buttons.testConnection")}
                    </button>
                    {renderTestResult("assemblyai")}
                  </div>
                </div>
              )}

              {asrType === "sherpa_onnx" && (
                <div className="settings-section__api-fields">
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.modelDirectory")}</label>
                    <input
                      className="settings-input"
                      type="text"
                      value={sherpaModelDir}
                      onChange={(e) => setSherpaModelDir(e.target.value)}
                      placeholder="streaming-zipformer-en-20M"
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-radio">
                      <input
                        type="checkbox"
                        checked={sherpaEndpointDetection}
                        onChange={(e) => setSherpaEndpointDetection(e.target.checked)}
                      />
                      <span>{t("settings.fields.enableEndpointDetection")}</span>
                    </label>
                  </div>
                </div>
              )}
            </div>

            {/* ── LLM Provider Section ───────────────────────────── */}
            <div className="settings-section">
              <h3 className="settings-section__title">{t("settings.sections.llm")}</h3>
              <div className="settings-radio-group">
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="llm-provider"
                    checked={llmType === "local_llama"}
                    onChange={() => setLlmType("local_llama")}
                  />
                  <span>{t("settings.llmProviders.localLlama")}</span>
                  {llmType === "local_llama" && modelStatus && (
                    <span
                      className={`status-badge ${readinessBadge(modelStatus.llm).cls}`}
                    >
                      {t(readinessBadge(modelStatus.llm).labelKey)}
                    </span>
                  )}
                </label>
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="llm-provider"
                    checked={llmType === "api"}
                    onChange={() => setLlmType("api")}
                  />
                  <span>{t("settings.llmProviders.openaiCompatible")}</span>
                </label>
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="llm-provider"
                    checked={llmType === "aws_bedrock"}
                    onChange={() => setLlmType("aws_bedrock")}
                  />
                  <span>{t("settings.llmProviders.awsBedrock")}</span>
                </label>
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="llm-provider"
                    checked={llmType === "mistralrs"}
                    onChange={() => setLlmType("mistralrs")}
                  />
                  <span>{t("settings.llmProviders.mistralrs")}</span>
                </label>
              </div>

              {llmType === "api" && (
                <div className="settings-section__api-fields">
                  <div className="settings-field">
                    <label className="settings-field__label">
                      {t("settings.fields.endpoint")}
                    </label>
                    <input
                      className="settings-input"
                      type="text"
                      value={llmEndpoint}
                      onChange={(e) => setLlmEndpoint(e.target.value)}
                      placeholder="https://openrouter.ai/api/v1"
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.apiKey")}</label>
                    <input
                      className="settings-input"
                      type="password"
                      value={llmApiKey}
                      onChange={(e) => setLlmApiKey(e.target.value)}
                      placeholder="sk-..."
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.model")}</label>
                    <input
                      className="settings-input"
                      type="text"
                      value={llmModel}
                      onChange={(e) => setLlmModel(e.target.value)}
                      placeholder="gpt-4o-mini"
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">
                      {t("settings.fields.maxTokens", { count: llmMaxTokens })}
                    </label>
                    <input
                      className="settings-input"
                      type="number"
                      value={llmMaxTokens}
                      onChange={(e) => setLlmMaxTokens(Number(e.target.value))}
                      min={1}
                      max={32768}
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">
                      {t("settings.fields.temperature", { value: llmTemperature })}
                    </label>
                    <input
                      className="settings-input"
                      type="number"
                      step="0.1"
                      value={llmTemperature}
                      onChange={(e) =>
                        setLlmTemperature(Number(e.target.value))
                      }
                      min={0}
                      max={2}
                    />
                  </div>
                </div>
              )}

              {llmType === "aws_bedrock" && (
                <div className="settings-section__api-fields">
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.region")}</label>
                    <input
                      className="settings-input"
                      type="text"
                      value={awsBedrockRegion}
                      onChange={(e) => setAwsBedrockRegion(e.target.value)}
                      placeholder="us-east-1"
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.modelId")}</label>
                    <input
                      className="settings-input"
                      type="text"
                      value={awsBedrockModelId}
                      onChange={(e) => setAwsBedrockModelId(e.target.value)}
                      placeholder="anthropic.claude-3-haiku-20240307-v1:0"
                    />
                  </div>
                  <div className="settings-field">
                    <label className="settings-field__label">
                      {t("settings.fields.credentialMode")}
                    </label>
                    <select
                      className="settings-input"
                      value={awsBedrockCredentialMode}
                      onChange={(e) =>
                        setAwsBedrockCredentialMode(
                          e.target.value as
                            | "default_chain"
                            | "profile"
                            | "access_keys",
                        )
                      }
                    >
                      <option value="default_chain">{t("settings.credentialModes.defaultChain")}</option>
                      <option value="profile">{t("settings.credentialModes.profile")}</option>
                      <option value="access_keys">{t("settings.credentialModes.accessKeys")}</option>
                    </select>
                  </div>
                  {awsBedrockCredentialMode === "profile" && (
                    <div className="settings-field">
                      <label className="settings-field__label">
                        {t("settings.fields.awsProfile")}
                      </label>
                      <div className="settings-inline-row">
                        <select
                          className="settings-input"
                          value={awsBedrockProfileName}
                          onChange={(e) =>
                            setAwsBedrockProfileName(e.target.value)
                          }
                        >
                          <option value="">{t("settings.placeholders.selectProfile")}</option>
                          {awsProfiles.map((name) => (
                            <option key={name} value={name}>
                              {name}
                            </option>
                          ))}
                        </select>
                        <button
                          type="button"
                          className="settings-btn settings-btn--secondary"
                          onClick={refreshAwsProfiles}
                        >
                          {t("settings.buttons.refresh")}
                        </button>
                      </div>
                      {awsProfiles.length === 0 && (
                        <p className="settings-hint">
                          {t("settings.hints.noAwsProfiles")}{" "}
                          <code>aws configure</code>{" "}
                          {t("settings.hints.noAwsProfilesSuffix")}
                        </p>
                      )}
                    </div>
                  )}
                  {awsBedrockCredentialMode === "access_keys" && (
                    <>
                      <div className="settings-field">
                        <label className="settings-field__label">
                          {t("settings.fields.accessKeyId")}
                        </label>
                        <input
                          className="settings-input"
                          type="password"
                          value={awsBedrockAccessKey}
                          onChange={(e) =>
                            setAwsBedrockAccessKey(e.target.value)
                          }
                          placeholder="AKIA..."
                        />
                      </div>
                      <div className="settings-field">
                        <label className="settings-field__label">
                          {t("settings.fields.secretAccessKey")}
                        </label>
                        <input
                          className="settings-input"
                          type="password"
                          value={awsBedrockSecretKey}
                          onChange={(e) =>
                            setAwsBedrockSecretKey(e.target.value)
                          }
                          placeholder="wJalr..."
                        />
                      </div>
                      <div className="settings-field">
                        <label className="settings-field__label">
                          {t("settings.fields.sessionTokenOptional")}
                        </label>
                        <input
                          className="settings-input"
                          type="password"
                          value={awsBedrockSessionToken}
                          onChange={(e) =>
                            setAwsBedrockSessionToken(e.target.value)
                          }
                          placeholder={t("settings.placeholders.sessionTokenHint")}
                        />
                      </div>
                      <div className="settings-field">
                        <button
                          type="button"
                          className="settings-btn settings-btn--danger"
                          onClick={() =>
                            handleClearCredential(
                              "aws_secret_key",
                              t("settings.credentialConfirm.awsKeysLabel"),
                              () => {
                                setAwsAsrSecretKey("");
                                setAwsBedrockSecretKey("");
                                setAwsAsrSessionToken("");
                                setAwsBedrockSessionToken("");
                                invoke("delete_credential_cmd", {
                                  key: "aws_session_token",
                                }).catch((e) =>
                                  console.error(
                                    "Failed to clear aws_session_token:",
                                    e,
                                  ),
                                );
                              },
                            )
                          }
                        >
                          {t("settings.buttons.clearSavedAwsKeys")}
                        </button>
                      </div>
                    </>
                  )}
                  <div className="settings-field">
                    <button
                      type="button"
                      className="settings-btn settings-btn--secondary"
                      disabled={testingKey !== null || !awsBedrockRegion}
                      onClick={handleTestAwsBedrock}
                    >
                      {testingKey === "aws_bedrock"
                        ? t("settings.buttons.testing")
                        : t("settings.buttons.testConnection")}
                    </button>
                    {renderTestResult("aws_bedrock")}
                  </div>
                </div>
              )}

              {llmType === "mistralrs" && (
                <div className="settings-section__api-fields">
                  <div className="settings-field">
                    <label className="settings-field__label">{t("settings.fields.modelId")}</label>
                    <input
                      className="settings-input"
                      type="text"
                      value={mistralrsModelId}
                      onChange={(e) => setMistralrsModelId(e.target.value)}
                      placeholder="ggml-small-extract.gguf"
                    />
                  </div>
                </div>
              )}
            </div>

            {/* ── Gemini Live Section ──────────────────────────── */}
            <div className="settings-section">
              <h3 className="settings-section__title">{t("settings.sections.gemini")}</h3>
              <div className="settings-radio-group">
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="gemini-auth"
                    checked={geminiAuthMode === "api_key"}
                    onChange={() => setGeminiAuthMode("api_key")}
                  />
                  <span>{t("settings.geminiAuth.apiKey")}</span>
                </label>
                <label className="settings-radio">
                  <input
                    type="radio"
                    name="gemini-auth"
                    checked={geminiAuthMode === "vertex_ai"}
                    onChange={() => setGeminiAuthMode("vertex_ai")}
                  />
                  <span>{t("settings.geminiAuth.vertexAi")}</span>
                </label>
              </div>

              <div className="settings-section__api-fields">
                {geminiAuthMode === "api_key" && (
                  <>
                    <div className="settings-field">
                      <label className="settings-field__label">
                        {t("settings.fields.geminiApiKey")}
                      </label>
                      <input
                        className="settings-input"
                        type="password"
                        value={geminiApiKey}
                        onChange={(e) => setGeminiApiKey(e.target.value)}
                        placeholder="AIza..."
                      />
                    </div>
                    <div className="settings-field">
                      <button
                        type="button"
                        className="settings-btn settings-btn--secondary"
                        disabled={testingKey !== null || !geminiApiKey}
                        onClick={handleTestGemini}
                      >
                        {testingKey === "gemini"
                          ? t("settings.buttons.testing")
                          : t("settings.buttons.testConnection")}
                      </button>
                      {renderTestResult("gemini")}
                    </div>
                  </>
                )}

                {geminiAuthMode === "vertex_ai" && (
                  <>
                    <div className="settings-field">
                      <label className="settings-field__label">
                        {t("settings.fields.projectId")}
                      </label>
                      <input
                        className="settings-input"
                        type="text"
                        value={geminiProjectId}
                        onChange={(e) => setGeminiProjectId(e.target.value)}
                        placeholder="my-gcp-project"
                      />
                    </div>
                    <div className="settings-field">
                      <label className="settings-field__label">{t("settings.fields.location")}</label>
                      <input
                        className="settings-input"
                        type="text"
                        value={geminiLocation}
                        onChange={(e) => setGeminiLocation(e.target.value)}
                        placeholder="us-central1"
                      />
                    </div>
                    <div className="settings-field">
                      <label className="settings-field__label">
                        {t("settings.fields.serviceAccountPathOptional")}
                      </label>
                      <input
                        className="settings-input"
                        type="text"
                        value={geminiServiceAccountPath}
                        onChange={(e) =>
                          setGeminiServiceAccountPath(e.target.value)
                        }
                        placeholder="/path/to/service-account.json"
                      />
                    </div>
                  </>
                )}

                <div className="settings-field">
                  <label className="settings-field__label">{t("settings.fields.model")}</label>
                  <input
                    className="settings-input"
                    type="text"
                    value={geminiModel}
                    onChange={(e) => setGeminiModel(e.target.value)}
                    placeholder="gemini-3.1-flash-live-preview"
                  />
                </div>
              </div>
            </div>

            {/* ── Diagnostics Section ─────────────────────────── */}
            <div className="settings-section">
              <h3 className="settings-section__title">
                {t("settings.sections.diagnostics")}
              </h3>
              <div className="settings-section__api-fields">
                <div className="settings-field">
                  <label className="settings-field__label" htmlFor="log-level-select">
                    {t("settings.fields.backendLogLevel")}
                  </label>
                  <select
                    id="log-level-select"
                    className="settings-input"
                    value={logLevel}
                    onChange={(e) =>
                      handleLogLevelChange(e.target.value as LogLevel)
                    }
                  >
                    <option value="off">{t("settings.logLevels.off")}</option>
                    <option value="error">{t("settings.logLevels.error")}</option>
                    <option value="warn">{t("settings.logLevels.warn")}</option>
                    <option value="info">{t("settings.logLevels.info")}</option>
                    <option value="debug">{t("settings.logLevels.debug")}</option>
                    <option value="trace">{t("settings.logLevels.trace")}</option>
                  </select>
                  <p className="settings-hint">
                    {t("settings.hints.logLevelPrefix")}{" "}
                    <code>RUST_LOG</code>{" "}
                    {t("settings.hints.logLevelSuffix")}
                  </p>
                </div>
              </div>
            </div>
          </div>
        )}

        {/* Footer */}
        <div className="settings-footer">
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
