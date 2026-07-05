/**
 * First-launch quickstart wizard.
 *
 * Rendered once when `App.tsx` detects no cloud provider credentials on
 * launch. Offers a narrowed choice of ASR + LLM providers (Gemini API /
 * Deepgram / AssemblyAI / local Whisper x OpenAI / Anthropic / local
 * llama / OpenRouter) and writes the selected credentials via
 * `save_credential_cmd` plus the provider pick via `save_settings_cmd`.
 *
 * Props:
 *   - `onDismiss`: close the modal (`Skip` or successful save).
 *   - `onOpenAdvanced`: hand off to the full `SettingsPage` — the parent
 *     `App.tsx` sets `expressSetupVisible = false` then opens Settings
 *     so the two modals don't stack.
 *   - `onPreviewSampleSession`: optional parent-owned handoff into a
 *     frontend-only sample session preview.
 *
 * Focus-trapped via `useFocusTrap`. No store binding beyond the store
 * actions it triggers via the backend.
 */

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useFocusTrap } from "../hooks/useFocusTrap";
import { useAudioGraphStore } from "../store";
import type {
  AppSettings,
  AsrProvider,
  CredentialPresence,
  GeminiSettings,
  LlmApiConfig,
  LlmProvider,
  ProviderReadiness,
} from "../types";
import { errorToMessage } from "../utils/errorToMessage";
import IconButton from "./IconButton";
import {
  deriveProviderSetupModeCards,
  type ProviderSetupBlocker,
  type ProviderSetupModeCard,
  type ProviderSetupProviderSelection,
  type ProviderSetupReadinessStatus,
  type ProviderSetupStageRole,
  providerSetupSourceRecoveryIssues,
} from "./providerSetupModes";
import { initialSettingsState, type SettingsState } from "./settingsTypes";

interface ExpressSetupProps {
  onDismiss: () => void;
  onOpenAdvanced: () => void;
  onPreviewSampleSession?: () => void;
}

type AsrChoice = "gemini" | "deepgram" | "assemblyai" | "local_whisper";
type LlmChoice = "openai" | "anthropic" | "local_llama" | "openrouter";

const ASR_CHOICES: readonly AsrChoice[] = [
  "gemini",
  "deepgram",
  "assemblyai",
  "local_whisper",
];
const LLM_CHOICES: readonly LlmChoice[] = [
  "openai",
  "anthropic",
  "local_llama",
  "openrouter",
];
const GEMINI_OPENAI_ENDPOINT =
  "https://generativelanguage.googleapis.com/v1beta/openai";
const OPENAI_ENDPOINT = "https://api.openai.com/v1";
const ANTHROPIC_ENDPOINT = "https://api.anthropic.com/v1";
const OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1";
const DEFAULT_GEMINI_ASR_MODEL = "gemini-2.5-flash";
const DEFAULT_GEMINI_LIVE_MODEL = "gemini-2.0-flash-live-001";
const DEFAULT_OPENAI_LLM_MODEL = "gpt-4o-mini";
const DEFAULT_ANTHROPIC_LLM_MODEL = "claude-3-5-haiku-latest";
const DEFAULT_OPENROUTER_MODEL = "openai/gpt-4o-mini";

const isCloudAsr = (c: AsrChoice) => c !== "local_whisper";
const isCloudLlm = (c: LlmChoice) => c !== "local_llama";

const defaultGeminiLiveSettings = (): GeminiSettings => ({
  auth: { type: "api_key", api_key: "" },
  model: DEFAULT_GEMINI_LIVE_MODEL,
});

interface ExpressSetupDraft {
  asrChoice: AsrChoice;
  asrKey: string;
  llmChoice: LlmChoice;
  llmKey: string;
  geminiLiveKey: string;
  enableSpeakAloud: boolean;
  existingSettings: AppSettings | null;
}

function settingsStateForExpressSetupDraft({
  asrChoice,
  asrKey,
  llmChoice,
  llmKey,
  geminiLiveKey,
  enableSpeakAloud,
  existingSettings,
}: ExpressSetupDraft): SettingsState {
  const existingGemini = existingSettings?.gemini;
  const base: SettingsState = {
    ...initialSettingsState,
    whisperModel: existingSettings?.whisper_model ?? "ggml-small.en.bin",
    streamingPrefill: existingSettings?.streaming_prefill ?? false,
    geminiAuthMode: existingGemini?.auth.type ?? "api_key",
    geminiModel: existingGemini?.model ?? DEFAULT_GEMINI_LIVE_MODEL,
    geminiProjectId:
      existingGemini?.auth.type === "vertex_ai"
        ? existingGemini.auth.project_id
        : "",
    geminiLocation:
      existingGemini?.auth.type === "vertex_ai"
        ? existingGemini.auth.location
        : "",
    geminiServiceAccountPath:
      existingGemini?.auth.type === "vertex_ai"
        ? (existingGemini.auth.service_account_path ?? "")
        : "",
  };

  const next: SettingsState = {
    ...base,
    geminiAuthMode: "api_key",
    geminiApiKey: asrChoice === "gemini" ? asrKey : geminiLiveKey,
  };

  switch (asrChoice) {
    case "gemini":
      next.asrType = "api";
      next.asrEndpoint = GEMINI_OPENAI_ENDPOINT;
      next.asrApiKey = asrKey;
      next.asrModel = DEFAULT_GEMINI_ASR_MODEL;
      break;
    case "deepgram":
      next.asrType = "deepgram";
      next.deepgramApiKey = asrKey;
      next.deepgramModel = "nova-3";
      next.deepgramDiarization = true;
      break;
    case "assemblyai":
      next.asrType = "assemblyai";
      next.assemblyaiApiKey = asrKey;
      next.assemblyaiDiarization = true;
      break;
    case "local_whisper":
      next.asrType = "local_whisper";
      break;
  }

  switch (llmChoice) {
    case "openai":
      next.llmType = "api";
      next.llmEndpoint = OPENAI_ENDPOINT;
      next.llmApiKey = llmKey;
      next.llmModel = DEFAULT_OPENAI_LLM_MODEL;
      break;
    case "anthropic":
      next.llmType = "api";
      next.llmEndpoint = ANTHROPIC_ENDPOINT;
      next.llmApiKey = llmKey;
      next.llmModel = DEFAULT_ANTHROPIC_LLM_MODEL;
      break;
    case "openrouter":
      next.llmType = "openrouter";
      next.openrouterApiKey = llmKey;
      next.openrouterModel = DEFAULT_OPENROUTER_MODEL;
      next.openrouterBaseUrl = OPENROUTER_BASE_URL;
      next.openrouterIncludeUsageInStream = true;
      break;
    case "local_llama":
      next.llmType = "local_llama";
      break;
  }

  if (asrChoice === "deepgram" && enableSpeakAloud) {
    next.deepgramApiKey = asrKey;
  }

  return next;
}

function selectedCard(
  cards: readonly ProviderSetupModeCard[],
): ProviderSetupModeCard {
  return cards.find((card) => card.selected) ?? cards[0];
}

function modeCardById(
  cards: readonly ProviderSetupModeCard[],
  id: ProviderSetupModeCard["id"],
): ProviderSetupModeCard {
  return cards.find((card) => card.id === id) ?? cards[0];
}

function providerForRole(
  card: ProviderSetupModeCard,
  role: ProviderSetupStageRole,
): ProviderSetupProviderSelection | null {
  return (
    card.selectedProviders.find((provider) => provider.role === role) ?? null
  );
}

function shouldRenderCredentialInput(
  provider: ProviderSetupProviderSelection | null,
  draftValue: string,
): boolean {
  if (!provider || provider.credentials.length === 0) return false;
  if (draftValue.trim().length > 0) return true;
  return provider.credentials.some((credential) => !credential.present);
}

function hasSavedCredential(
  provider: ProviderSetupProviderSelection | null,
): boolean {
  return (
    provider != null &&
    provider.credentials.length > 0 &&
    provider.credentials.every((credential) => credential.present)
  );
}

function uniqueMissingCredentialBlockers(
  cards: readonly ProviderSetupModeCard[],
): ProviderSetupBlocker[] {
  const seen = new Set<string>();

  return cards.flatMap((card) =>
    card.missingBlockers.filter((blocker) => {
      if (blocker.kind !== "missing_credential") return false;
      const key = `${blocker.providerId}:${blocker.key ?? blocker.message}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    }),
  );
}

function blockerIsSource(blocker: ProviderSetupBlocker): boolean {
  return (
    blocker.kind === "source_unselected" ||
    blocker.kind === "source_unavailable" ||
    blocker.kind === "source_permission_unavailable" ||
    blocker.kind === "source_unsupported" ||
    blocker.kind === "source_policy_conflict"
  );
}

function cardHasSourceBlocker(card: ProviderSetupModeCard): boolean {
  return card.missingBlockers.some(blockerIsSource);
}

// Minimal translator shape (avoids importing i18next's TFunction generics).
// The Setup-modes label helpers below take this so their copy is localized
// instead of hardcoded English (seed audio-graph-88ad — pt first-run parity).
type SetupModesTranslate = (
  key: string,
  opts?: Record<string, unknown>,
) => string;

function readinessLabel(
  status: ProviderSetupReadinessStatus,
  t: SetupModesTranslate,
): string {
  return t(`express.setupModes.readiness.${status}`);
}

function credentialSummary(
  provider: ProviderSetupProviderSelection,
  t: SetupModesTranslate,
): string {
  if (provider.credentials.length === 0)
    return t("express.setupModes.noCredentialRequired");
  return provider.credentials
    .map((credential) =>
      t("express.setupModes.credentialState", {
        key: credential.key,
        state: credential.present
          ? t("express.setupModes.credentialPresent")
          : t("express.setupModes.credentialMissing"),
      }),
    )
    .join(", ");
}

function dataBoundaryLabel(
  card: ProviderSetupModeCard,
  t: SetupModesTranslate,
): string {
  return t(`express.setupModes.dataBoundary.${card.dataBoundary}`);
}

function ExpressSetup({
  onDismiss,
  onOpenAdvanced,
  onPreviewSampleSession,
}: ExpressSetupProps) {
  const { t } = useTranslation();
  const modalRef = useFocusTrap<HTMLDivElement>();
  const {
    settings,
    fetchSettings,
    audioSources,
    selectedSourceIds,
    conversationMode,
    converseEngine,
    setConversationMode,
    setConverseEngine,
    requestSourceRecovery,
  } = useAudioGraphStore();
  const runtimeNativeRealtimeSelected =
    conversationMode === "converse" && converseEngine === "native";

  const [asrChoice, setAsrChoice] = useState<AsrChoice>("gemini");
  const [asrKey, setAsrKey] = useState("");
  const [showAsrKey, setShowAsrKey] = useState(false);

  const [llmChoice, setLlmChoice] = useState<LlmChoice>("openai");
  const [llmKey, setLlmKey] = useState("");
  const [showLlmKey, setShowLlmKey] = useState(false);

  const [enableGeminiLive, setEnableGeminiLive] = useState(
    runtimeNativeRealtimeSelected,
  );
  const [geminiLiveKey, setGeminiLiveKey] = useState("");
  const [showGeminiLiveKey, setShowGeminiLiveKey] = useState(false);

  // Speak-aloud opt-in. Only meaningful when ASR=Deepgram (the same key
  // works for STT and TTS). Hidden / forced false otherwise.
  const [enableSpeakAloud, setEnableSpeakAloud] = useState(false);

  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [credentialPresence, setCredentialPresence] = useState<
    CredentialPresence[]
  >([]);
  const [providerReadiness, setProviderReadiness] = useState<
    ProviderReadiness[]
  >([]);
  const [readinessLoading, setReadinessLoading] = useState(false);
  const [readinessError, setReadinessError] = useState<string | null>(null);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onDismiss();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onDismiss]);

  useEffect(() => {
    let cancelled = false;

    const loadReadiness = async () => {
      setReadinessLoading(true);
      setReadinessError(null);
      try {
        const [presence, readiness] = await Promise.all([
          invoke<CredentialPresence[]>("load_credential_presence_cmd"),
          invoke<ProviderReadiness[]>("get_provider_readiness_cmd", {
            refresh: true,
            conversationMode: useAudioGraphStore.getState().conversationMode,
            converseEngine: useAudioGraphStore.getState().converseEngine,
          }),
        ]);
        if (cancelled) return;
        setCredentialPresence(presence ?? []);
        setProviderReadiness(readiness ?? []);
      } catch (e) {
        if (cancelled) return;
        setCredentialPresence([]);
        setProviderReadiness([]);
        setReadinessError(errorToMessage(e));
      } finally {
        if (!cancelled) setReadinessLoading(false);
      }
    };

    void loadReadiness();

    return () => {
      cancelled = true;
    };
  }, []);

  const setupSettings = useMemo(
    () =>
      settingsStateForExpressSetupDraft({
        asrChoice,
        asrKey,
        llmChoice,
        llmKey,
        geminiLiveKey,
        enableSpeakAloud,
        existingSettings: settings,
      }),
    [
      asrChoice,
      asrKey,
      llmChoice,
      llmKey,
      geminiLiveKey,
      enableSpeakAloud,
      settings,
    ],
  );

  const providerSetupInput = useMemo(
    () => ({
      settings: setupSettings,
      credentialPresence,
      providerReadiness,
      sourceState: { sources: audioSources, selectedSourceIds },
      tts: {
        ttsType:
          asrChoice === "deepgram" && enableSpeakAloud
            ? ("deepgram_aura" as const)
            : ("none" as const),
        speakAloud: asrChoice === "deepgram" && enableSpeakAloud,
      },
    }),
    [
      asrChoice,
      audioSources,
      credentialPresence,
      enableSpeakAloud,
      providerReadiness,
      selectedSourceIds,
      setupSettings,
    ],
  );

  const durableModeCards = useMemo(
    () =>
      deriveProviderSetupModeCards({
        ...providerSetupInput,
        conversationMode: "notes",
        converseEngine: "pipelined",
      }),
    [providerSetupInput],
  );
  const modeCards = useMemo(
    () =>
      deriveProviderSetupModeCards({
        ...providerSetupInput,
        conversationMode: enableGeminiLive ? "converse" : "notes",
        converseEngine: enableGeminiLive ? "native" : "pipelined",
      }),
    [enableGeminiLive, providerSetupInput],
  );
  const selectedDurableModeCard = selectedCard(durableModeCards);
  const nativeModeCard = modeCardById(modeCards, "native_realtime");
  const asrProviderSelection = providerForRole(
    selectedDurableModeCard,
    "durable_transcription",
  );
  const llmProviderSelection = providerForRole(
    selectedDurableModeCard,
    "durable_notes_graph",
  );
  const geminiLiveProviderSelection = providerForRole(
    nativeModeCard,
    "native_realtime_agent",
  );
  const asrNeedsKey =
    isCloudAsr(asrChoice) &&
    shouldRenderCredentialInput(asrProviderSelection, asrKey);
  const llmNeedsKey =
    isCloudLlm(llmChoice) &&
    shouldRenderCredentialInput(llmProviderSelection, llmKey);
  const geminiLiveNeedsSeparateKey =
    enableGeminiLive &&
    asrChoice !== "gemini" &&
    shouldRenderCredentialInput(geminiLiveProviderSelection, geminiLiveKey);
  const asrUsesSavedKey =
    isCloudAsr(asrChoice) &&
    !asrNeedsKey &&
    asrKey.trim().length === 0 &&
    hasSavedCredential(asrProviderSelection);
  const llmUsesSavedKey =
    isCloudLlm(llmChoice) &&
    !llmNeedsKey &&
    llmKey.trim().length === 0 &&
    hasSavedCredential(llmProviderSelection);
  const geminiLiveUsesSavedKey =
    enableGeminiLive &&
    asrChoice !== "gemini" &&
    !geminiLiveNeedsSeparateKey &&
    geminiLiveKey.trim().length === 0 &&
    hasSavedCredential(geminiLiveProviderSelection);
  const activeSaveCards = enableGeminiLive
    ? [selectedDurableModeCard, nativeModeCard]
    : [selectedDurableModeCard];
  const missingCredentialBlockers =
    uniqueMissingCredentialBlockers(activeSaveCards);
  const canSave = !saving && missingCredentialBlockers.length === 0;

  const handleSourceRecovery = (card: ProviderSetupModeCard) => {
    requestSourceRecovery({
      origin: "provider_setup",
      issues: providerSetupSourceRecoveryIssues(card),
    });
    onDismiss();
  };

  const buildAsrProvider = (): AsrProvider => {
    switch (asrChoice) {
      case "gemini":
        // Durable Gemini ASR runs through Google's OpenAI-compatible API.
        // Native Gemini Live remains a separate realtime mode configured by
        // the dedicated checkbox below.
        return {
          type: "api",
          endpoint: GEMINI_OPENAI_ENDPOINT,
          api_key: "",
          model: DEFAULT_GEMINI_ASR_MODEL,
        };
      case "deepgram":
        return {
          type: "deepgram",
          api_key: "",
          model: "nova-3",
          enable_diarization: true,
        };
      case "assemblyai":
        return {
          type: "assemblyai",
          api_key: "",
          enable_diarization: true,
        };
      case "local_whisper":
        return { type: "local_whisper" };
    }
  };

  const buildLlmProvider = (): LlmProvider => {
    switch (llmChoice) {
      case "openai":
        return {
          type: "api",
          endpoint: OPENAI_ENDPOINT,
          api_key: "",
          model: DEFAULT_OPENAI_LLM_MODEL,
        };
      case "anthropic":
        return {
          type: "api",
          endpoint: ANTHROPIC_ENDPOINT,
          api_key: "",
          model: DEFAULT_ANTHROPIC_LLM_MODEL,
        };
      case "openrouter":
        // First-class OpenRouter variant per ADR-0005. Defaults
        // mirror OpenRouterSettings::default in Rust: empty model
        // (forces a save-time pick), canonical base URL, usage
        // included in stream so the lifetime-usage tracker can see
        // it. Attribution headers (HTTP-Referer + X-OpenRouter-Title)
        // are added per-request inside the OpenRouterClient — no
        // need to mention them here.
        return {
          type: "openrouter",
          model: DEFAULT_OPENROUTER_MODEL,
          base_url: OPENROUTER_BASE_URL,
          include_usage_in_stream: true,
          provider_order: null,
          api_key: "",
        };
      case "local_llama":
        return { type: "local_llama" };
    }
  };

  const buildLlmApiConfig = (p: LlmProvider): LlmApiConfig | null => {
    // LlmApiConfig is the runtime-hydrated companion for the GENERIC
    // LlmProvider::Api variant only. OpenRouter has its own settings
    // shape (the variant carries model/base_url/etc inline); local
    // engines need no companion either. Return null for everything
    // that isn't the literal "api" variant.
    if (p.type !== "api") return null;
    return {
      endpoint: p.endpoint,
      api_key: null,
      model: p.model,
      max_tokens: 2048,
      temperature: 0.7,
    };
  };

  // Persist the ASR API key under whichever credential slot matches the
  // selected provider. Provider settings keep routing metadata only;
  // credentials.yaml is the source of truth for secrets.
  const saveAsrCredential = async () => {
    if (!isCloudAsr(asrChoice) || !asrKey.trim()) return;
    const key =
      asrChoice === "gemini"
        ? "gemini_api_key"
        : asrChoice === "deepgram"
          ? "deepgram_api_key"
          : "assemblyai_api_key";
    await invoke("save_credential_cmd", { key, value: asrKey });
  };

  const saveLlmCredential = async () => {
    if (!isCloudLlm(llmChoice) || !llmKey.trim()) return;
    // OpenRouter has its own credential slot per ADR-0005 — saving the
    // key under `openai_api_key` would work for the chat path (the
    // generic Api variant reads from there) but breaks the first-class
    // OpenRouter wiring's `test_openrouter_connection_cmd` and the
    // model-picker `list_openrouter_models_cmd`, both of which look in
    // `openrouter_api_key`. OpenAI / Anthropic still share
    // openai_api_key — they hit OpenAI-compatible endpoints with a
    // bearer token under whatever the endpoint expects.
    const key =
      llmChoice === "openrouter" ? "openrouter_api_key" : "openai_api_key";
    await invoke("save_credential_cmd", { key, value: llmKey });
  };

  const saveGeminiLiveCredential = async () => {
    if (!enableGeminiLive || asrChoice === "gemini" || !geminiLiveKey.trim()) {
      return;
    }
    await invoke("save_credential_cmd", {
      key: "gemini_api_key",
      value: geminiLiveKey,
    });
  };

  const handleSave = async () => {
    setSaving(true);
    setError(null);
    try {
      const asrProvider = buildAsrProvider();
      const llmProvider = buildLlmProvider();
      const llmApiConfig = buildLlmApiConfig(llmProvider);

      // Gemini settings are only for native Gemini Live. Durable Gemini API
      // ASR is represented by `asr_provider` above and hydrates from the
      // `gemini_api_key` credential slot by endpoint, so selecting it must not
      // rewrite existing Live auth.
      const existingGemini = settings?.gemini;
      const gemini: GeminiSettings = enableGeminiLive
        ? {
            ...(existingGemini ?? defaultGeminiLiveSettings()),
            auth: { type: "api_key", api_key: "" },
            model: existingGemini?.model ?? DEFAULT_GEMINI_LIVE_MODEL,
          }
        : (existingGemini ?? defaultGeminiLiveSettings());

      // Speak-aloud is offered when ASR=Deepgram because the
      // same key authorises Aura. Other ASR choices keep TTS off
      // (the user can still flip it on in the full Settings dialog).
      const ttsProvider: AppSettings["tts_provider"] =
        asrChoice === "deepgram" && enableSpeakAloud
          ? {
              type: "deepgram_aura",
              voice: "aura-asteria-en",
              sample_rate: 24_000,
              speed: 1.0,
            }
          : (settings?.tts_provider ?? { type: "none" });
      const speakAloud =
        asrChoice === "deepgram" && enableSpeakAloud
          ? true
          : (settings?.speak_aloud ?? false);

      const nextSettings: AppSettings = {
        asr_provider: asrProvider,
        whisper_model: settings?.whisper_model ?? "ggml-small.en.bin",
        llm_provider: llmProvider,
        llm_api_config: llmApiConfig,
        tts_provider: ttsProvider,
        speak_aloud: speakAloud,
        streaming_prefill: settings?.streaming_prefill ?? false,
        audio_settings: settings?.audio_settings ?? {
          sample_rate: 48000,
          channels: 2,
        },
        gemini,
        privacy_mode: settings?.privacy_mode ?? "byok_cloud",
        log_level: settings?.log_level ?? "info",
        // Completing ExpressSetup is the definitive "I've configured
        // providers" signal — pin demo_mode to false so the demo
        // banner doesn't reappear on next launch even if the user
        // picked local_* choices here.
        demo_mode: false,
      };

      await saveAsrCredential();
      await saveLlmCredential();
      await saveGeminiLiveCredential();
      await invoke("save_settings_cmd", { settings: nextSettings });
      if (enableGeminiLive) {
        setConversationMode("converse");
        setConverseEngine("native");
      } else {
        setConverseEngine("pipelined");
      }
      await fetchSettings();
      onDismiss();
    } catch (e) {
      setError(errorToMessage(e));
      setSaving(false);
    }
  };

  const handleAdvanced = () => {
    onOpenAdvanced();
    onDismiss();
  };

  return (
    // Backdrop click-to-dismiss is a convenience; the dialog is fully operable
    // via the close button and the global Escape handler, plus a key handler here.
    // biome-ignore lint/a11y/noStaticElementInteractions: see comment above
    <div
      className="settings-overlay"
      onClick={onDismiss}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onDismiss();
        }
      }}
      data-testid="express-setup-overlay"
    >
      <div
        ref={modalRef}
        className="settings-modal express-setup-modal"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="express-setup-title"
        tabIndex={-1}
      >
        <div className="settings-header">
          <h2 id="express-setup-title" className="settings-header__title">
            {t("express.title")}
          </h2>
          <IconButton
            icon="close"
            label={t("express.skip")}
            variant="ghost"
            className="settings-header__close"
            onClick={onDismiss}
          />
        </div>

        <div className="settings-content">
          <p className="express-setup-intro">{t("express.intro")}</p>

          <section
            className="settings-section"
            aria-label={t("express.setupModes.sectionLabel")}
          >
            {readinessLoading && (
              <p className="settings-section__empty" role="status">
                {t("express.setupModes.checkingReadiness")}
              </p>
            )}
            {readinessError && (
              <div className="express-setup-error" role="alert">
                {t("express.setupModes.readinessError", {
                  error: readinessError,
                })}
              </div>
            )}
            <div className="settings-section__api-fields">
              {modeCards.map((card) => (
                <article
                  key={card.id}
                  className={`settings-provider-readiness settings-provider-readiness--${card.readinessStatus}`}
                  aria-labelledby={`express-mode-${card.id}`}
                  data-testid={`express-mode-card-${card.id}`}
                >
                  <div className="settings-provider-readiness__main">
                    <h3
                      id={`express-mode-${card.id}`}
                      className="settings-provider-readiness__label"
                    >
                      {card.selected
                        ? t("express.setupModes.labelSelected", {
                            label: card.label,
                          })
                        : card.label}
                    </h3>
                    <span>{readinessLabel(card.readinessStatus, t)}</span>
                  </div>
                  <p className="settings-provider-readiness__message">
                    {card.description}
                  </p>
                  <dl className="settings-provider-readiness__metadata">
                    <div>
                      <dt>{t("express.setupModes.dataBoundaryLabel")}</dt>
                      <dd>{dataBoundaryLabel(card, t)}</dd>
                    </div>
                    <div>
                      <dt>{t("express.setupModes.productPathLabel")}</dt>
                      <dd>{card.productPath.replaceAll("_", " ")}</dd>
                    </div>
                  </dl>
                  <ul>
                    {card.selectedProviders.map((provider) => (
                      <li key={`${card.id}-${provider.providerId}`}>
                        <strong>{provider.providerName}</strong>
                        {provider.model ? `: ${provider.model}` : ""} -{" "}
                        {readinessLabel(provider.readinessStatus, t)} -{" "}
                        {credentialSummary(provider, t)}
                        {provider.readinessMessage
                          ? ` - ${provider.readinessMessage}`
                          : ""}
                      </li>
                    ))}
                  </ul>
                  {card.missingBlockers.length > 0 ? (
                    <ul
                      aria-label={t("express.setupModes.blockersLabel", {
                        label: card.label,
                      })}
                    >
                      {card.missingBlockers.map((blocker) => (
                        <li
                          key={`${blocker.providerId}-${blocker.kind}-${
                            blocker.key ?? blocker.model ?? blocker.message
                          }`}
                        >
                          {blocker.message}
                        </li>
                      ))}
                    </ul>
                  ) : (
                    <p className="settings-provider-readiness__message">
                      {t("express.setupModes.noBlockers")}
                    </p>
                  )}
                  {cardHasSourceBlocker(card) && (
                    <div className="settings-provider-readiness__recovery">
                      <p>{t("express.setupModes.sourceRecovery")}</p>
                      <button
                        type="button"
                        className="settings-btn settings-btn--secondary"
                        onClick={() => handleSourceRecovery(card)}
                      >
                        {t("express.setupModes.reviewSources")}
                      </button>
                    </div>
                  )}
                </article>
              ))}
            </div>
          </section>

          {/* ASR step */}
          <div className="settings-section">
            <label
              className="settings-field__label"
              htmlFor="express-asr-provider"
            >
              {t("express.asrProvider")}
            </label>
            <select
              id="express-asr-provider"
              className="settings-input"
              value={asrChoice}
              onChange={(e) => setAsrChoice(e.target.value as AsrChoice)}
            >
              {ASR_CHOICES.map((c) => (
                <option key={c} value={c}>
                  {t(`express.asrOptions.${c}`)}
                </option>
              ))}
            </select>
            {asrNeedsKey && (
              <div className="settings-field">
                <label
                  className="settings-field__label"
                  htmlFor="express-asr-key"
                >
                  {t("express.asrApiKey")}
                </label>
                <p className="settings-hint">
                  {t("express.credentialActionHint")}
                </p>
                <div className="express-key-row">
                  <input
                    id="express-asr-key"
                    className="settings-input"
                    type={showAsrKey ? "text" : "password"}
                    value={asrKey}
                    onChange={(e) => setAsrKey(e.target.value)}
                    autoComplete="off"
                  />
                  <button
                    type="button"
                    className="settings-btn settings-btn--secondary"
                    onClick={() => setShowAsrKey((v) => !v)}
                    aria-label={
                      showAsrKey ? t("express.hideKey") : t("express.showKey")
                    }
                  >
                    {showAsrKey ? t("express.hideKey") : t("express.showKey")}
                  </button>
                </div>
              </div>
            )}
            {asrUsesSavedKey && (
              <p className="settings-hint">
                {t("express.credentialSavedHint")}
              </p>
            )}
          </div>

          {/* LLM step */}
          <div className="settings-section">
            <label
              className="settings-field__label"
              htmlFor="express-llm-provider"
            >
              {t("express.llmProvider")}
            </label>
            <select
              id="express-llm-provider"
              className="settings-input"
              value={llmChoice}
              onChange={(e) => setLlmChoice(e.target.value as LlmChoice)}
            >
              {LLM_CHOICES.map((c) => (
                <option key={c} value={c}>
                  {t(`express.llmOptions.${c}`)}
                </option>
              ))}
            </select>
            {llmNeedsKey && (
              <div className="settings-field">
                <label
                  className="settings-field__label"
                  htmlFor="express-llm-key"
                >
                  {t("express.llmApiKey")}
                </label>
                <p className="settings-hint">
                  {t("express.credentialActionHint")}
                </p>
                <div className="express-key-row">
                  <input
                    id="express-llm-key"
                    className="settings-input"
                    type={showLlmKey ? "text" : "password"}
                    value={llmKey}
                    onChange={(e) => setLlmKey(e.target.value)}
                    autoComplete="off"
                  />
                  <button
                    type="button"
                    className="settings-btn settings-btn--secondary"
                    onClick={() => setShowLlmKey((v) => !v)}
                    aria-label={
                      showLlmKey ? t("express.hideKey") : t("express.showKey")
                    }
                  >
                    {showLlmKey ? t("express.hideKey") : t("express.showKey")}
                  </button>
                </div>
              </div>
            )}
            {llmUsesSavedKey && (
              <p className="settings-hint">
                {t("express.credentialSavedHint")}
              </p>
            )}
          </div>

          {/* Optional speak-aloud — only when ASR=Deepgram */}
          {asrChoice === "deepgram" && (
            <div className="settings-section">
              <label className="settings-radio">
                <input
                  type="checkbox"
                  checked={enableSpeakAloud}
                  onChange={(e) => setEnableSpeakAloud(e.target.checked)}
                />
                <span>{t("express.speakAloud")}</span>
              </label>
            </div>
          )}

          {/* Optional Gemini Live */}
          <div className="settings-section">
            <label className="settings-radio">
              <input
                type="checkbox"
                checked={enableGeminiLive}
                onChange={(e) => setEnableGeminiLive(e.target.checked)}
              />
              <span>{t("express.optional")}</span>
            </label>
            {geminiLiveNeedsSeparateKey && (
              <div className="settings-field">
                <label
                  className="settings-field__label"
                  htmlFor="express-gemini-key"
                >
                  {t("express.geminiLiveApiKey")}
                </label>
                <p className="settings-hint">
                  {t("express.credentialActionHint")}
                </p>
                <div className="express-key-row">
                  <input
                    id="express-gemini-key"
                    className="settings-input"
                    type={showGeminiLiveKey ? "text" : "password"}
                    value={geminiLiveKey}
                    onChange={(e) => setGeminiLiveKey(e.target.value)}
                    autoComplete="off"
                  />
                  <button
                    type="button"
                    className="settings-btn settings-btn--secondary"
                    onClick={() => setShowGeminiLiveKey((v) => !v)}
                    aria-label={
                      showGeminiLiveKey
                        ? t("express.hideKey")
                        : t("express.showKey")
                    }
                  >
                    {showGeminiLiveKey
                      ? t("express.hideKey")
                      : t("express.showKey")}
                  </button>
                </div>
              </div>
            )}
            {geminiLiveUsesSavedKey && (
              <p className="settings-hint">
                {t("express.credentialSavedHint")}
              </p>
            )}
          </div>

          {error && (
            <div className="express-setup-error" role="alert">
              {error}
            </div>
          )}
        </div>

        <div className="settings-footer express-setup-footer">
          <button
            type="button"
            className="express-setup-advanced"
            onClick={handleAdvanced}
          >
            {t("express.advanced")}
          </button>
          <div className="express-setup-actions">
            {onPreviewSampleSession && (
              <button
                type="button"
                className="settings-btn settings-btn--secondary"
                onClick={onPreviewSampleSession}
              >
                {t("express.previewSample")}
              </button>
            )}
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              onClick={onDismiss}
            >
              {t("express.skip")}
            </button>
            <button
              type="button"
              className="settings-btn settings-btn--primary"
              onClick={handleSave}
              disabled={!canSave}
            >
              {saving ? t("express.saving") : t("express.saveSetup")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

export default ExpressSetup;
