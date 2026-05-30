/**
 * First-launch quickstart wizard.
 *
 * Rendered once when `App.tsx` detects no cloud provider credentials on
 * launch. Offers a narrowed choice of ASR + LLM providers (Gemini /
 * Deepgram / AssemblyAI / local Whisper × OpenAI / Anthropic / local
 * llama / OpenRouter) and writes the selected credentials via
 * `save_credential_cmd` plus the provider pick via `save_settings_cmd`.
 *
 * Props:
 *   - `onDismiss`: close the modal (`Skip` or successful save).
 *   - `onOpenAdvanced`: hand off to the full `SettingsPage` — the parent
 *     `App.tsx` sets `expressSetupVisible = false` then opens Settings
 *     so the two modals don't stack.
 *
 * Focus-trapped via `useFocusTrap`. No store binding beyond the store
 * actions it triggers via the backend.
 */

import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useFocusTrap } from "../hooks/useFocusTrap";
import { useAudioGraphStore } from "../store";
import type {
  AppSettings,
  AsrProvider,
  GeminiSettings,
  LlmApiConfig,
  LlmProvider,
} from "../types";
import { errorToMessage } from "../utils/errorToMessage";
import IconButton from "./IconButton";

interface ExpressSetupProps {
  onDismiss: () => void;
  onOpenAdvanced: () => void;
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

const isCloudAsr = (c: AsrChoice) => c !== "local_whisper";
const isCloudLlm = (c: LlmChoice) => c !== "local_llama";

function ExpressSetup({ onDismiss, onOpenAdvanced }: ExpressSetupProps) {
  const { t } = useTranslation();
  const modalRef = useFocusTrap<HTMLDivElement>();
  const { settings, fetchSettings } = useAudioGraphStore();

  const [asrChoice, setAsrChoice] = useState<AsrChoice>("gemini");
  const [asrKey, setAsrKey] = useState("");
  const [showAsrKey, setShowAsrKey] = useState(false);

  const [llmChoice, setLlmChoice] = useState<LlmChoice>("openai");
  const [llmKey, setLlmKey] = useState("");
  const [showLlmKey, setShowLlmKey] = useState(false);

  const [enableGeminiLive, setEnableGeminiLive] = useState(false);
  const [geminiLiveKey, setGeminiLiveKey] = useState("");
  const [showGeminiLiveKey, setShowGeminiLiveKey] = useState(false);

  // Speak-aloud opt-in. Only meaningful when ASR=Deepgram (the same key
  // works for STT and TTS). Hidden / forced false otherwise.
  const [enableSpeakAloud, setEnableSpeakAloud] = useState(false);

  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

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

  const asrNeedsKey = isCloudAsr(asrChoice);
  const llmNeedsKey = isCloudLlm(llmChoice);
  const canSave =
    !saving &&
    (!asrNeedsKey || asrKey.trim().length > 0) &&
    (!llmNeedsKey || llmKey.trim().length > 0) &&
    (!enableGeminiLive || geminiLiveKey.trim().length > 0);

  const buildAsrProvider = (): AsrProvider => {
    switch (asrChoice) {
      case "gemini":
        // Gemini ASR is handled via Gemini Live separately; for the
        // "ASR provider" slot we route via generic cloud API (OpenAI-
        // compatible with a Gemini key) so the user can still run the
        // standard transcribe pipeline. Users who want real-time
        // Gemini Live have the dedicated checkbox below.
        return {
          type: "api",
          endpoint: "https://generativelanguage.googleapis.com/v1beta/openai",
          api_key: "",
          model: "gemini-2.5-flash",
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
          endpoint: "https://api.openai.com/v1",
          api_key: "",
          model: "gpt-4o-mini",
        };
      case "anthropic":
        return {
          type: "api",
          endpoint: "https://api.anthropic.com/v1",
          api_key: "",
          model: "claude-3-5-haiku-latest",
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
          model: "openai/gpt-4o-mini",
          base_url: "https://openrouter.ai/api/v1",
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
    if (!asrNeedsKey) return;
    const key =
      asrChoice === "gemini"
        ? "gemini_api_key"
        : asrChoice === "deepgram"
          ? "deepgram_api_key"
          : "assemblyai_api_key";
    await invoke("save_credential_cmd", { key, value: asrKey });
  };

  const saveLlmCredential = async () => {
    if (!llmNeedsKey) return;
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
    if (!enableGeminiLive) return;
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

      // Gemini settings: if user opted in to Gemini Live OR picked
      // Gemini ASR, seed the Gemini auth block with the entered key.
      // Otherwise preserve whatever's already there.
      const existingGemini = settings?.gemini;
      const gemini: GeminiSettings = enableGeminiLive
        ? {
            auth: { type: "api_key", api_key: "" },
            model: existingGemini?.model ?? "gemini-3.1-flash-live-preview",
          }
        : asrChoice === "gemini"
          ? {
              auth: { type: "api_key", api_key: "" },
              model: existingGemini?.model ?? "gemini-3.1-flash-live-preview",
            }
          : (existingGemini ?? {
              auth: { type: "api_key", api_key: "" },
              model: "gemini-3.1-flash-live-preview",
            });

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
          channels: 1,
        },
        gemini,
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
                  {t("express.apiKey")}
                </label>
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
                  {t("express.apiKey")}
                </label>
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
                <span>
                  Speak chatbot replies aloud (Deepgram Aura — uses the same
                  Deepgram key)
                </span>
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
            {enableGeminiLive && (
              <div className="settings-field">
                <label
                  className="settings-field__label"
                  htmlFor="express-gemini-key"
                >
                  {t("express.apiKey")}
                </label>
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
              {saving ? t("express.saving") : t("express.saveAndStart")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

export default ExpressSetup;
