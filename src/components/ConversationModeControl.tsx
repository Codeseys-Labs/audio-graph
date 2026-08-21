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
import { PROVIDER_DESCRIPTORS } from "./providerRegistryHelpers";

// Tailwind utility groups (ADR-0016), faithfully translated from the former
// conversation-mode.css module. Colors/radii/fonts resolve through design
// tokens via the @theme bridge; spacing uses the token shorthand.
const SEGMENTS =
  "inline-flex bg-bg-tertiary border border-(--edge) rounded-md p-px";
const SEG =
  "inline-flex items-center gap-(--space-2) py-(--space-2) px-(--space-4) border-none bg-transparent text-text-secondary text-sm font-medium rounded-sm cursor-pointer transition-colors duration-[120ms] hover:text-text-primary";
const SEG_ACTIVE = "bg-bg-elevated text-text-primary";
const ENGINE =
  "inline-flex items-center gap-(--space-2) py-(--space-2) px-(--space-4) border border-(--edge) bg-transparent text-text-secondary text-xs rounded-full cursor-pointer hover:text-text-primary hover:border-divider-color";
const ENGINE_ACTIVE = "text-accent border-accent bg-(--tint-accent)";
const BADGE =
  "ml-(--space-2) py-0 px-(--space-2) text-xs rounded-sm bg-bg-tertiary text-text-muted border-none";
const BADGE_ACTION =
  "ml-(--space-2) py-0 px-(--space-2) text-xs rounded-sm bg-bg-tertiary border-none text-accent cursor-pointer hover:underline";

export default function ConversationModeControl() {
  const { t } = useTranslation();
  const conversationMode = useAudioGraphStore((s) => s.conversationMode);
  const setConversationMode = useAudioGraphStore((s) => s.setConversationMode);
  const converseEngine = useAudioGraphStore((s) => s.converseEngine);
  const setConverseEngine = useAudioGraphStore((s) => s.setConverseEngine);
  const converseRealtimeAgentProvider = useAudioGraphStore(
    (s) => s.converseRealtimeAgentProvider,
  );
  const setConverseRealtimeAgentProvider = useAudioGraphStore(
    (s) => s.setConverseRealtimeAgentProvider,
  );
  const settings = useAudioGraphStore((s) => s.settings);
  const openSettings = useAudioGraphStore((s) => s.openSettings);

  const selectableRealtimeAgents = [
    ["gemini", "realtime_agent.gemini_live"],
    ["openai", "realtime_agent.openai_realtime"],
  ] as const;
  const selectableRealtimeAgentIds = selectableRealtimeAgents.filter(
    ([, providerId]) =>
      PROVIDER_DESCRIPTORS.get(providerId)?.ui_selectable === true,
  );
  const nativeAgentSelectable = selectableRealtimeAgentIds.length > 0;

  const hasGeminiKey =
    settings?.gemini?.auth?.type === "api_key" ||
    settings?.gemini?.auth?.type === "vertex_ai";
  // Pipelined converse needs an LLM provider configured (chat + speak-aloud).
  const hasLlm = Boolean(settings?.llm_provider);

  const isConverse = conversationMode === "converse";
  // Default to Gemini when the store field is unset (e.g. seeded test state).
  const realtimeAgentProvider = converseRealtimeAgentProvider ?? "gemini";

  return (
    <div className="inline-flex items-center gap-(--space-4) min-w-0">
      <fieldset
        className={`${SEGMENTS} m-0 min-w-0`}
        aria-label={t("controlBar.conversationMode")}
      >
        <button
          type="button"
          aria-pressed={!isConverse}
          className={`${SEG} ${!isConverse ? SEG_ACTIVE : ""}`}
          onClick={() => setConversationMode("notes")}
          title={t("controlBar.modeNotesHint")}
        >
          <Icon name="notes" size={14} /> {t("controlBar.modeNotes")}
        </button>
        <button
          type="button"
          aria-pressed={isConverse}
          className={`${SEG} ${isConverse ? SEG_ACTIVE : ""}`}
          onClick={() => setConversationMode("converse")}
          title={t("controlBar.modeConverseHint")}
        >
          <Icon name="chat" size={14} /> {t("controlBar.modeConverse")}
        </button>
      </fieldset>

      {isConverse && (
        <fieldset
          className="inline-flex gap-(--space-2) border-none p-0 m-0 min-w-0"
          aria-label={t("controlBar.converseEngine")}
        >
          {/*
           * The engine choice is mutually exclusive (exactly one of
           * pipelined/native). The semantically-ideal primitive is a radio
           * group, but `role="radio"` on a styled <button> trips biome's
           * useSemanticElements / noNoninteractiveElementToInteractiveRole
           * (it wants a native <input type=radio>, which would mean rebuilding
           * the segmented control). Keeping toggle <button>s with aria-pressed
           * is the lint-clean, AT-supported middle ground: each button exposes
           * its pressed/active state, and the enclosing fieldset+aria-label
           * groups them as one control (A11Y-1).
           */}
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
            disabled={!nativeAgentSelectable}
            onClick={() => setConverseEngine("native")}
            title={
              !nativeAgentSelectable
                ? t("controlBar.engineNotInMvp")
                : hasGeminiKey
                  ? t("controlBar.engineNativeHint")
                  : t("controlBar.engineNeedsKey")
            }
          >
            {t("controlBar.engineNative")}
            {!nativeAgentSelectable && (
              <span className={BADGE}>{t("controlBar.notInMvp")}</span>
            )}
          </button>
          {nativeAgentSelectable && !hasGeminiKey && (
            // Sibling of the Native button (NOT nested — a button inside a
            // button is invalid HTML and breaks the accessible name). The
            // visible text is just "Configure"; give SR users the full intent.
            <button
              type="button"
              className={BADGE_ACTION}
              onClick={() => openSettings()}
              title={t("controlBar.engineNeedsKey")}
              aria-label={t("controlBar.configureGeminiKey")}
            >
              {t("controlBar.configure")}
            </button>
          )}
          {converseEngine === "native" && nativeAgentSelectable && (
            // Native S2S provider selector (realtime-agent): Gemini Live vs.
            // the OpenAI Realtime voice agent (gpt-realtime-2). Only shown for
            // the native engine; the pipelined path is provider-agnostic.
            <fieldset
              className="inline-flex gap-(--space-2) border-none p-0 m-0 min-w-0"
              aria-label={t("controlBar.realtimeAgentProvider")}
            >
              {selectableRealtimeAgentIds.map(([agentId]) => (
                <button
                  key={agentId}
                  type="button"
                  className={`${ENGINE} ${
                    realtimeAgentProvider === agentId ? ENGINE_ACTIVE : ""
                  }`}
                  aria-pressed={realtimeAgentProvider === agentId}
                  onClick={() => setConverseRealtimeAgentProvider?.(agentId)}
                  title={t(
                    agentId === "gemini"
                      ? "controlBar.realtimeAgentGeminiHint"
                      : "controlBar.realtimeAgentOpenAiHint",
                  )}
                >
                  {t(
                    agentId === "gemini"
                      ? "controlBar.realtimeAgentGemini"
                      : "controlBar.realtimeAgentOpenAi",
                  )}
                </button>
              ))}
            </fieldset>
          )}
        </fieldset>
      )}
    </div>
  );
}
