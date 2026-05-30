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

// Tailwind utility groups (ADR-0016), faithfully translated from the former
// conversation-mode.css module. Colors/radii/fonts resolve through design
// tokens via the @theme bridge; spacing uses the token shorthand.
const SEGMENTS =
  "inline-flex bg-bg-tertiary border border-border-color rounded-md p-px";
const SEG =
  "inline-flex items-center gap-(--space-2) py-(--space-2) px-(--space-4) border-none bg-transparent text-text-secondary text-sm font-medium rounded-sm cursor-pointer transition-colors duration-[120ms] hover:text-text-primary";
const SEG_ACTIVE = "bg-bg-elevated text-text-primary";
const ENGINE =
  "inline-flex items-center gap-(--space-2) py-(--space-2) px-(--space-4) border border-border-color bg-transparent text-text-secondary text-xs rounded-full cursor-pointer hover:text-text-primary hover:border-divider-color";
const ENGINE_ACTIVE = "text-accent border-accent bg-[rgba(108,140,255,0.12)]";
const BADGE =
  "ml-(--space-2) py-0 px-(--space-2) text-2xs rounded-sm bg-bg-tertiary text-text-muted border-none";
const BADGE_ACTION =
  "ml-(--space-2) py-0 px-(--space-2) text-2xs rounded-sm bg-bg-tertiary border-none text-accent cursor-pointer hover:underline";

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
    <fieldset
      className="inline-flex items-center gap-(--space-4) border-none p-0 m-0 min-w-0"
      aria-label={t("controlBar.conversationMode")}
    >
      <div className={SEGMENTS} role="tablist">
        <button
          type="button"
          role="tab"
          aria-selected={!isConverse}
          className={`${SEG} ${!isConverse ? SEG_ACTIVE : ""}`}
          onClick={() => setConversationMode("notes")}
          title={t("controlBar.modeNotesHint")}
        >
          <Icon name="notes" size={14} /> {t("controlBar.modeNotes")}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={isConverse}
          className={`${SEG} ${isConverse ? SEG_ACTIVE : ""}`}
          onClick={() => setConversationMode("converse")}
          title={t("controlBar.modeConverseHint")}
        >
          <Icon name="chat" size={14} /> {t("controlBar.modeConverse")}
        </button>
      </div>

      {isConverse && (
        <fieldset
          className="inline-flex gap-(--space-2) border-none p-0 m-0 min-w-0"
          aria-label={t("controlBar.converseEngine")}
        >
          <button
            type="button"
            className={`${ENGINE} ${converseEngine === "pipelined" ? ENGINE_ACTIVE : ""}`}
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
              <span className={BADGE}>{t("controlBar.needsSetup")}</span>
            )}
          </button>
          <button
            type="button"
            className={`${ENGINE} ${converseEngine === "native" ? ENGINE_ACTIVE : ""}`}
            aria-pressed={converseEngine === "native"}
            onClick={() => setConverseEngine("native")}
            title={
              hasGeminiKey
                ? t("controlBar.engineNativeHint")
                : t("controlBar.engineNeedsKey")
            }
          >
            {t("controlBar.engineNative")}
          </button>
          {!hasGeminiKey && (
            // Sibling of the Native button (NOT nested — a button inside a
            // button is invalid HTML and breaks the accessible name).
            <button
              type="button"
              className={BADGE_ACTION}
              onClick={() => openSettings()}
              title={t("controlBar.engineNeedsKey")}
            >
              {t("controlBar.configure")}
            </button>
          )}
        </fieldset>
      )}
    </fieldset>
  );
}
