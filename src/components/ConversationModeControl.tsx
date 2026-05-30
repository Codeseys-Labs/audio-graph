/**
 * Conversation-mode control (ADR-0013).
 *
 * The discoverable, intent-first selector that replaces the hidden
 * `nativeS2sEnabled` flag. The user chooses *what they're doing*:
 *
 *   - Notes    — transcribe speech to build the knowledgebase (graph + notes).
 *   - Converse — talk *to* the knowledgebase. When Converse is active, the user
 *                picks the engine: Pipelined (STT → graph-grounded LLM → TTS,
 *                reusing the working chat + speak-aloud path) or Native (Gemini
 *                Live; OpenAI Realtime later).
 *
 * Availability is computed honestly from settings so we never offer a control
 * that silently no-ops: Native needs a Gemini key; Pipelined needs an LLM.
 * Always visible (even before capture) so the value proposition isn't hidden.
 */
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import Icon from "./Icon";

export default function ConversationModeControl() {
  const { t } = useTranslation();
  const conversationMode = useAudioGraphStore((s) => s.conversationMode);
  const setConversationMode = useAudioGraphStore((s) => s.setConversationMode);
  const converseEngine = useAudioGraphStore((s) => s.converseEngine);
  const setConverseEngine = useAudioGraphStore((s) => s.setConverseEngine);
  const settings = useAudioGraphStore((s) => s.settings);
  const openSettings = useAudioGraphStore((s) => s.openSettings);

  const hasGeminiKey =
    settings?.gemini?.auth?.type === "api_key" ||
    settings?.gemini?.auth?.type === "vertex_ai";
  // Pipelined converse needs an LLM provider configured (chat + speak-aloud).
  const hasLlm = Boolean(settings?.llm_provider);

  const isConverse = conversationMode === "converse";

  return (
    <div
      className="conv-mode"
      role="group"
      aria-label={t("controlBar.conversationMode")}
    >
      <div className="conv-mode__segments" role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={!isConverse}
          className={`conv-mode__seg ${!isConverse ? "conv-mode__seg--active" : ""}`}
          onClick={() => setConversationMode("notes")}
          title={t("controlBar.modeNotesHint")}
        >
          <Icon name="notes" size={14} /> {t("controlBar.modeNotes")}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={isConverse}
          className={`conv-mode__seg ${isConverse ? "conv-mode__seg--active" : ""}`}
          onClick={() => setConversationMode("converse")}
          title={t("controlBar.modeConverseHint")}
        >
          <Icon name="chat" size={14} /> {t("controlBar.modeConverse")}
        </button>
      </div>

      {isConverse && (
        <div
          className="conv-mode__engines"
          role="group"
          aria-label={t("controlBar.converseEngine")}
        >
          <button
            type="button"
            className={`conv-mode__engine ${converseEngine === "pipelined" ? "conv-mode__engine--active" : ""}`}
            aria-pressed={converseEngine === "pipelined"}
            onClick={() => setConverseEngine("pipelined")}
            title={
              hasLlm
                ? t("controlBar.enginePipelinedHint")
                : t("controlBar.engineNeedsLlm")
            }
          >
            {t("controlBar.enginePipelined")}
            {!hasLlm && (
              <span className="conv-mode__badge">
                {t("controlBar.needsSetup")}
              </span>
            )}
          </button>
          <button
            type="button"
            className={`conv-mode__engine ${converseEngine === "native" ? "conv-mode__engine--active" : ""}`}
            aria-pressed={converseEngine === "native"}
            onClick={() => setConverseEngine("native")}
            title={
              hasGeminiKey
                ? t("controlBar.engineNativeHint")
                : t("controlBar.engineNeedsKey")
            }
          >
            {t("controlBar.engineNative")}
            {!hasGeminiKey && (
              <button
                type="button"
                className="conv-mode__badge conv-mode__badge--action"
                onClick={(e) => {
                  e.stopPropagation();
                  openSettings();
                }}
              >
                {t("controlBar.configure")}
              </button>
            )}
          </button>
        </div>
      )}
    </div>
  );
}
