/**
 * `AgentComposer` — the agent tile's pinned free-form question input
 * (audio-graph-83cc T4, graft G3 from the design panel synthesis:
 * "the composer renders in every state, including idle").
 *
 * Rendered by `AgentProposalsPanel` as a fixed row BELOW its scroll region,
 * in EVERY render branch (idle / queue-empty / streaming) — never inside the
 * conditionally-rendered idle-vs-body split, and never inside
 * `AGENT_QUEUE_PANEL_ID` (that id is the Signal/All tablist's
 * `aria-controls` target; the composer is not part of that contract — see
 * `AgentProposalsPanel.tsx`'s module doc). This is the actual fix for the
 * field bug this epic exists to kill: the pre-T4 idle branch returned before
 * any input could exist at all — see the design panel synthesis's graft G3
 * doc, quoting the exact line ("today's idle branch returns before any input
 * could exist — that is precisely why the chatbox is unreachable").
 *
 * Submit mints a new thread via `askQuestion` (T4 store action,
 * `store/index.ts`) — see that action's doc for the `ask_question_card`
 * dispatch and its graceful degradation while T3 (the backend answer
 * engine) is unlanded: a rejected dispatch renders inline here via
 * `composerError`, never throws, never silently drops the attempt.
 *
 * A11y: labelled input (`aria-label`, reusing `chat.inputLabel` — the same
 * copy the pre-existing `ChatSidebar` composer already ships, per the
 * ticket's "reuse chat.inputPlaceholder/inputLabel/send/thinking" note), a
 * real `<button>` submit via `IconButton` (keyboard + screen-reader
 * reachable without any custom key handling beyond Enter-to-send), and a
 * `role="alert"` error region that only exists in the DOM when there is an
 * error to announce (no persistent, always-mounted live region to avoid
 * announcing on every unrelated re-render). Local `input`/`isSubmitting`
 * state lives in THIS component, uncoupled from any store field that
 * mutates when a card updates elsewhere — `AgentProposalsPanel` renders this
 * component at a stable position in every branch, so a card update never
 * unmounts/remounts it and never steals focus from an in-progress edit.
 *
 * The `<input>` itself is NEVER disabled while submitting (audio-graph-83cc
 * T4 fix-round finding, minor): disabling the currently-focused element is
 * a real-browser blur-to-`<body>` (jsdom does not reproduce this, which is
 * why an earlier version of this file shipped `disabled={isSubmitting}`
 * with a passing focus test) — a keyboard user pressing Enter-to-send would
 * lose focus on every submit and have to Tab back in. Double-submission is
 * already prevented at the `handleSend` guard (`if (!trimmed ||
 * isSubmitting) return`), so disabling the input bought nothing;  only the
 * send `IconButton` is disabled during submit.
 *
 * `AutoAnswerCountChip` (audio-graph-83cc T5, deliverable e) renders above
 * the input row, in this composer, not the tile's `headerSlot` (already
 * double-occupied by `AgentQueueFilterToggle`/`AgentTileHeaderActions`, and
 * each `WorkspaceTile` gets exactly one, per `WorkspaceTile.tsx`'s frozen
 * contract) — matching the design panel synthesis's own placement ("the
 * chip in the composer row").
 */
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../../store";
import Icon from "../Icon";
import IconButton from "../IconButton";

/**
 * The auto-answer session budget chip: `"{{count}}/{{cap}}"` (e.g. `"3/12"`),
 * fed from `autoAnswerDispatchCount` — a count of SUCCESSFULLY dispatched
 * auto-answers this session, incremented only on acceptance, never on a
 * refusal (see that store field's own doc, `types/index.ts`, for why it
 * isn't derived from `liveAssistCards`/`CardAnswer.requested_by`). Hidden
 * entirely while auto-answer is disabled (deliverable f's off switch):
 * showing a budget for a feature that will never spend it is more
 * confusing than showing nothing. `max_per_session` falls back to the same
 * `12` Rust defaults to (`AgentAutoAnswerSettings::default`,
 * `settings/mod.rs`) for the brief window before settings load — this chip
 * itself is unreachable in that window anyway, since `enabled` is
 * `undefined` (not `=== true`) until settings load.
 */
function AutoAnswerCountChip() {
  const { t } = useTranslation();
  const enabled = useAudioGraphStore(
    (s) => s.settings?.agent_auto_answer?.enabled === true,
  );
  const count = useAudioGraphStore((s) => s.autoAnswerDispatchCount);
  const cap = useAudioGraphStore(
    (s) => s.settings?.agent_auto_answer?.max_per_session ?? 12,
  );
  if (!enabled) return null;
  return (
    <span className="ag-chip self-start" data-tone="neutral">
      <span aria-hidden="true">
        {t("agent.autoAnswerCount", { count, cap })}
      </span>
      <span className="sr-only">
        {t("agent.autoAnswerCountAria", { count, cap })}
      </span>
    </span>
  );
}

export function AgentComposer() {
  const { t } = useTranslation();
  const askQuestion = useAudioGraphStore((s) => s.askQuestion);
  const composerError = useAudioGraphStore((s) => s.composerError);
  const [input, setInput] = useState("");
  const [isSubmitting, setIsSubmitting] = useState(false);

  const handleSend = async () => {
    const trimmed = input.trim();
    if (!trimmed || isSubmitting) return;
    setIsSubmitting(true);
    try {
      await askQuestion(trimmed);
      // `askQuestion` degrades gracefully (never throws — see its own
      // doc), so success vs. failure is read back from the store rather
      // than a thrown exception. Only clear the typed text on success
      // (audio-graph-83cc T4 fix-round finding, minor): on failure the
      // question is preserved so the user can retry without retyping it —
      // `composerError` alone told them WHAT failed, never preserved WHAT
      // they asked.
      if (!useAudioGraphStore.getState().composerError) {
        setInput("");
      }
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void handleSend();
    }
  };

  return (
    <div
      className="flex flex-col gap-(--space-2) py-[10px] px-(--space-5) border-t border-(--edge) bg-bg-secondary shrink-0"
      data-testid="agent-composer"
    >
      <AutoAnswerCountChip />
      {composerError ? (
        <p
          className="m-0 rounded-sm border border-(--tint-border-danger) bg-(--tint-danger) px-(--space-4) py-(--space-3) text-xs leading-[1.4] text-(--text-on-tint-danger)"
          role="alert"
        >
          <Icon name="error" size={14} /> {composerError}
        </p>
      ) : null}
      <div className="flex gap-(--space-3)">
        <input
          type="text"
          className="flex-1 py-(--space-4) px-(--space-5) border border-(--edge) rounded-lg bg-bg-primary text-text-primary text-[0.85rem] outline-none transition-[border-color] duration-200 focus:border-accent-blue placeholder:text-text-muted"
          placeholder={t("chat.inputPlaceholder")}
          aria-label={t("chat.inputLabel")}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={handleKeyDown}
        />
        <IconButton
          className="py-(--space-4) px-[14px] border-none rounded-lg bg-accent-blue text-(--on-accent-blue) text-[1rem] cursor-pointer transition-[background-color,transform,opacity] duration-(--motion-base) ease-(--ease-standard) shrink-0 hover:not-disabled:bg-(--accent-blue-hover) hover:not-disabled:scale-105 disabled:opacity-40 disabled:cursor-not-allowed"
          icon="send"
          label={t("chat.send")}
          onClick={() => void handleSend()}
          disabled={!input.trim() || isSubmitting}
        />
      </div>
    </div>
  );
}

export default AgentComposer;
