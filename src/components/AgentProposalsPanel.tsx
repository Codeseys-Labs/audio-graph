/**
 * `AgentProposalsPanel` — the agent tile's body (ticket W8, synthesis
 * audio-graph-a6b5 §"Agent tile"). Mounts INSIDE the bento agent tile's
 * `WorkspaceTile` (`App.tsx`), which R3 keeps ALWAYS mounted regardless of
 * activity.
 *
 * Two exports, for the same reason ticket W5's `LiveDocument`/
 * `LiveDocumentHeaderActions` split exists (see that file's own module
 * doc): `WorkspaceTile`'s header bar already renders the tile's title
 * ("Agent", `agent.title`) with a real accessible name. The pre-W8 version
 * of this file rendered its OWN `<section aria-label="Agent proposals">`
 * wrapper — a SECOND named region nested inside the tile's own region (seed
 * 913d's duplicate-landmark half — "W8 fixes the agent tile's duplicate-
 * landmark half"). `AgentTileHeaderActions` (the Clear action) now renders
 * in `WorkspaceTile`'s `headerSlot`; `AgentProposalsPanel` renders ONLY the
 * body — no internal header, no `aria-label`, no second named region.
 *
 * The queue/feed split (design-a §3.1 "queue on top, feed below"; ratified
 * decision 4) is computed by `selectAgentQueue` (`workspace/agentQueue.ts`)
 * — this component stays a thin renderer over that pure selector, zero new
 * store state (design-b §4.3).
 *
 * Approval semantics are UNCHANGED: `approveAgentProposal`/
 * `askAgentProposal`/`dismissAgentProposal`/`clearAgentProposals` are the
 * exact same store actions as before this ticket, still routing through the
 * ADR-0013-governed `approve_agent_proposal` write path (4b52 made it
 * timestamp-safe). This ticket only moves markup and tone plumbing around
 * those calls — it never changes what they do or when they fire.
 *
 * Ticket W9 (ratified R6) adds the Signal/All queue-quality toggle on top of
 * that unchanged foundation: `useAgentQueueFilter`/`AgentQueueFilterToggle`
 * below are this file's second exported pair (mirrors
 * `LiveGraphStrip.tsx`'s `useGraphStripMode`/`GraphStripModeSwitcher` W7
 * precedent exactly — same `localStorage`-backed `useState` idiom, same
 * "lifted once in `App.tsx`, passed down as props" shape, for the identical
 * reason: the header toggle and this panel's body must never desync, and a
 * second internal `useState` copy in each would do exactly that). `filter`
 * defaults to `"signal"` so every pre-W9 render call site (and every
 * pre-W9 test) keeps compiling and behaving unchanged.
 *
 * Ticket T4 (audio-graph-83cc) is the one deliberate exception to the
 * "approval semantics are UNCHANGED" claim above: the queue row's manual
 * "Ask AI" button now dispatches `answerQuestionCard` (threads the answer
 * under the card via `answerDrafts`/`CardAnswer`) instead of the pre-T4
 * `askAgentProposal` (dismissed the card, dumped the reply into the
 * unreachable `chatMessages` — the exact field failure this epic exists to
 * kill). `approveAgentProposal`/`dismissAgentProposal`/`clearAgentProposals`
 * are untouched. T4 also adds `<AgentComposer>` as a permanent sibling below
 * the queue/feed scroll region, in every render branch including idle (see
 * that component's module doc), and `<AnswerThread>` rendering inside both
 * row types — see those two components' own doc comments.
 */
import type { KeyboardEvent as ReactKeyboardEvent } from "react";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type {
  AgentProposalEvent,
  AnswerDraftState,
  CardAnswer,
  LiveAssistCardRecord,
} from "../types";
import Icon from "./Icon";
import { AgentComposer } from "./workspace/AgentComposer";
import { admitToQueue, selectAgentQueue } from "./workspace/agentQueue";
import { agentOutcomeChipTone } from "./workspace/liveWorkspaceTone";

/** Ticket W9: the persisted Signal/All queue filter. `"signal"` (default,
 * ratified) applies `admitToQueue`'s fragment-suspect gate; `"all"` shows
 * every actionable card regardless of classification — filtered entries are
 * never deleted, only re-admitted (see `agentQueue.ts`'s updated
 * `admitToQueue` doc comment). */
export type AgentQueueFilterMode = "signal" | "all";

const AGENT_QUEUE_FILTER_STORAGE_KEY = "ag.agentQueueFilter";
const AGENT_QUEUE_FILTER_MODES: readonly AgentQueueFilterMode[] = [
  "signal",
  "all",
];

/** The single body region both tabs control — unlike
 * `GraphStripModeSwitcher`'s three MUTUALLY EXCLUSIVE mounted panels (one
 * per mode), Signal/All never unmount `AgentProposalsPanel`'s body; both
 * tabs point `aria-controls` at the SAME region because it is the one DOM
 * node whose content changes when the selected tab changes — a valid,
 * well-established `aria-controls` shape for a filter toggle (as opposed to
 * a view switcher). */
const AGENT_QUEUE_PANEL_ID = "agent-queue-filter-panel";

/** The real "All" admit predicate — a stable module-level reference (not an
 * inline arrow) so passing it into `selectAgentQueue` doesn't change
 * `useMemo`'s dependency identity on every render. */
const ADMIT_ALL: Parameters<typeof selectAgentQueue>[2] = () => true;

function isAgentQueueFilterMode(
  value: string | null,
): value is AgentQueueFilterMode {
  return (
    value !== null &&
    (AGENT_QUEUE_FILTER_MODES as readonly string[]).includes(value)
  );
}

function loadAgentQueueFilterMode(): AgentQueueFilterMode {
  try {
    const raw = localStorage.getItem(AGENT_QUEUE_FILTER_STORAGE_KEY);
    return isAgentQueueFilterMode(raw) ? raw : "signal";
  } catch {
    return "signal";
  }
}

/**
 * The persisted filter-choice hook — see the module doc for why this is
 * `localStorage`-backed `useState` (the `processScope`/`useGraphStripMode`
 * precedent) rather than a store slice, and why it must be called ONCE, up
 * in `App.tsx`, with the result threaded down as props to both the header
 * toggle and this panel's body.
 */
export function useAgentQueueFilter(): [
  AgentQueueFilterMode,
  (mode: AgentQueueFilterMode) => void,
] {
  const [mode, setModeState] = useState<AgentQueueFilterMode>(
    loadAgentQueueFilterMode,
  );
  const setMode = (next: AgentQueueFilterMode) => {
    setModeState(next);
    try {
      localStorage.setItem(AGENT_QUEUE_FILTER_STORAGE_KEY, next);
    } catch {
      // Persistence failure is non-fatal — the in-memory choice still
      // applies for the rest of this session, same posture as
      // `useGraphStripMode`'s setter.
    }
  };
  return [mode, setMode];
}

/** Header-slot content — the two-way Signal/All toggle (design-a §3.1's
 * `[Signal|All]`, mounted alongside `AgentTileHeaderActions` in the SAME
 * `headerSlot` per `App.tsx`'s W6/W7 dual-composition pattern for this tile
 * head). `role="tablist"`/`role="tab"`/`aria-selected` — the same shape
 * `AudioSourceSelector.tsx`'s `processScope` toggle and
 * `LiveGraphStrip.tsx`'s `GraphStripModeSwitcher` both use for an identical
 * "switch which content this same region shows" control (also sidesteps
 * `lint/a11y/useSemanticElements`'s `role="group"` -> `<fieldset>`
 * suggestion, which doesn't fit a toggle that isn't a form). Plain buttons,
 * not `.ag-chip`s — same reason `GraphStripModeSwitcher` uses plain buttons
 * (synthesis W7: sidesteps the pt chip-length budget for the control
 * itself; the marker chip this ticket separately adds to rows IS an
 * `.ag-chip` and IS budget-checked).
 *
 * Implements the FULL WAI-ARIA APG tabs keyboard contract, matching
 * `GraphStripModeSwitcher` exactly (review finding: an earlier version of
 * this control copied that switcher's classNames/roles but shipped none of
 * its roving-`tabIndex`/arrow-key behavior, so it announced itself as
 * "tab, 1 of 2" to assistive tech while not responding to arrow keys — the
 * repo's own established contract for this exact tile-header region,
 * pinned elsewhere for the workspace tablist and the sessions browser) —
 * roving `tabIndex` (only the selected tab is a Tab stop) plus
 * ArrowLeft/ArrowRight/Home/End moving both selection and focus together. */
export function AgentQueueFilterToggle({
  mode,
  onModeChange,
}: {
  mode: AgentQueueFilterMode;
  onModeChange: (mode: AgentQueueFilterMode) => void;
}) {
  const { t } = useTranslation();
  const handleKeyDown = (e: ReactKeyboardEvent<HTMLButtonElement>) => {
    const NAV = ["ArrowLeft", "ArrowRight", "Home", "End"];
    if (!NAV.includes(e.key)) return;
    e.preventDefault();
    const currentIndex = AGENT_QUEUE_FILTER_MODES.indexOf(mode);
    const nextIndex =
      e.key === "Home"
        ? 0
        : e.key === "End"
          ? AGENT_QUEUE_FILTER_MODES.length - 1
          : e.key === "ArrowLeft"
            ? (currentIndex - 1 + AGENT_QUEUE_FILTER_MODES.length) %
              AGENT_QUEUE_FILTER_MODES.length
            : (currentIndex + 1) % AGENT_QUEUE_FILTER_MODES.length;
    const next = AGENT_QUEUE_FILTER_MODES[nextIndex];
    onModeChange(next);
    const tablist = e.currentTarget.parentElement;
    const tabs = tablist?.querySelectorAll<HTMLButtonElement>('[role="tab"]');
    tabs?.[nextIndex]?.focus();
  };
  return (
    <div
      role="tablist"
      aria-label={t("agent.filterLabel")}
      className="flex items-center gap-(--space-1)"
    >
      {AGENT_QUEUE_FILTER_MODES.map((m) => (
        <button
          key={m}
          type="button"
          role="tab"
          id={`agent-queue-filter-tab-${m}`}
          aria-selected={mode === m}
          aria-controls={AGENT_QUEUE_PANEL_ID}
          tabIndex={mode === m ? 0 : -1}
          className={`py-[2px] px-(--space-3) text-xs font-semibold rounded-sm border cursor-pointer whitespace-nowrap ${
            mode === m
              ? "bg-bg-elevated text-accent border-accent"
              : "border-transparent bg-transparent text-text-muted hover:text-text-primary"
          }`}
          onClick={() => onModeChange(m)}
          onKeyDown={handleKeyDown}
        >
          {t(m === "signal" ? "agent.filterSignal" : "agent.filterAll")}
        </button>
      ))}
    </div>
  );
}

function proposalKindKey(kind: AgentProposalEvent["kind"]): string {
  switch (kind) {
    case "graph_suggestion":
      return "agent.kindGraph";
    case "question":
      return "agent.kindQuestion";
    case "note":
      return "agent.kindNote";
  }
}

function formatConfidence(value: number): string {
  if (!Number.isFinite(value)) return "";
  return `${Math.round(value * 100)}%`;
}

/**
 * Maps `agentOutcomeChipTone`'s post-law axis status to its i18n key.
 * `"unchecked"` — an `"approved"` card with no recorded outcome — gets its
 * OWN copy (`agent.statusUnverified`), never `agent.statusApproved`: the law
 * gates the claim, not just the color (readinessTone.ts). This function is
 * the ONLY place `LiveAssistCardStatus`'s render copy is decided; the raw
 * `card.status` value is never used for copy directly.
 */
function agentChipCopyKey(
  status: "ready" | "pending" | "dismissed" | "unchecked",
): string {
  switch (status) {
    case "ready":
      return "agent.statusApproved";
    case "pending":
      return "agent.statusPending";
    case "dismissed":
      return "agent.statusDismissed";
    case "unchecked":
      return "agent.statusUnverified";
  }
}

function formatApprovedOutcome(card: LiveAssistCardRecord): string | null {
  return card.outcome?.message?.trim() || null;
}

function formatProjectionPatchEvidence(
  card: LiveAssistCardRecord,
): string | null {
  if (card.projection_patch_sequence === null) return null;
  if (card.projection_patch_sequence === undefined) return null;
  return `Patch sequence ${card.projection_patch_sequence}`;
}

/** The tone-law-routed status chip (ticket W8: the pre-W8 hand-rolled
 * status-to-border/text-class helper this file used to define is deleted
 * outright — not shadowed — and replaced by `.ag-chip[data-tone]`; see
 * `liveWorkspaceTone.ts`'s `agentOutcomeChipTone` and `badgeTone.ts`'s
 * `agentProposalStatusTone` for the law + concrete-color halves). Shared by
 * both the queue and feed rows so the two surfaces can never render a
 * different tone for the same status. */
function AgentStatusChip({ card }: { card: LiveAssistCardRecord }) {
  const { t } = useTranslation();
  const chip = agentOutcomeChipTone({
    status: card.status,
    hasOutcome: card.outcome != null,
  });
  return (
    <span className="ag-chip" data-tone={chip.tone}>
      {t(agentChipCopyKey(chip.effectiveStatus))}
    </span>
  );
}

interface AgentQueueRowProps {
  card: LiveAssistCardRecord;
  isApproving: boolean;
  isFragmentSuspect: boolean;
  duplicateCount: number;
  /** `answerDrafts[card.proposal.id]` (audio-graph-83cc T4) — `undefined`
   * when no dispatch has ever been made for this card this session. */
  draft: AnswerDraftState | undefined;
  onApprove: (proposalId: string) => void;
  onAsk: (proposalId: string) => void;
  onDismiss: (proposalId: string) => void;
}

/** The W9 "subtle fragment marker" (design-a §3.2): renders on a queue row
 * ONLY in All mode (a card that Signal mode would have filtered, but is
 * still shown, still fully actionable) and on a feed row when Signal mode
 * filtered it there instead. A plain `.ag-chip[data-tone="neutral"]` — this
 * is a QUALITY flag, not a freshness/status claim, so it deliberately does
 * NOT route through `agentOutcomeChipTone`/the T2 tone law (that law governs
 * claims about being current/verified; "low signal" is neither). Reuses the
 * existing neutral chip styling (`styles.css`) — zero new CSS. */
function FragmentSuspectMarker() {
  const { t } = useTranslation();
  return (
    <span className="ag-chip" data-tone="neutral">
      {t("agent.lowSignal")}
    </span>
  );
}

/** design-a §3.2 rule 2's other half: "the original [surviving] row renders
 * `agent.duplicateCount` — `×{{count}}`". Renders on the SURVIVING row of a
 * duplicate-collapse group (queue or feed, whichever it landed in) — see
 * `AgentQueueSelection.duplicateCounts`'s doc for why a duplicate's own
 * (demoted) row never carries this badge, only the survivor's does. Callers
 * only mount this for `count > 1`; this component itself doesn't gate on
 * that so the "when to render at all" decision stays visible at the call
 * site rather than hidden in this leaf. */
function DuplicateCountBadge({ count }: { count: number }) {
  const { t } = useTranslation();
  return (
    <span className="ag-chip" data-tone="neutral">
      {t("agent.duplicateCount", { count })}
    </span>
  );
}

/** Compact, count-only evidence chips for a `CardAnswer` (audio-graph-83cc
 * T4, deliverable c: "evidence ids render as compact chips (no content
 * lookups in this unit)"). Deliberately renders COUNTS, not the raw
 * `evidence_span_ids`/`evidence_graph_ids` themselves — a query-conditioned
 * retrieval bundle can carry up to ~40 graph ids (design panel synthesis
 * §4.3), and dumping that many opaque ids as individual chips would be
 * exactly the "content dump" ticket W8 already rejected for the feed row.
 * Neither chip performs a lookup into the transcript/graph store to resolve
 * what an id refers to — resolving evidence content is explicitly out of
 * this unit's scope. */
function AnswerEvidenceChips({ answer }: { answer: CardAnswer }) {
  const { t } = useTranslation();
  const spanCount = answer.evidence_span_ids.length;
  const graphCount = answer.evidence_graph_ids.length;
  if (spanCount === 0 && graphCount === 0) return null;
  return (
    <div className="flex flex-wrap gap-(--space-2)">
      {spanCount > 0 ? (
        <span className="ag-chip" data-tone="neutral">
          {t("agent.answerEvidenceSpans", { count: spanCount })}
        </span>
      ) : null}
      {graphCount > 0 ? (
        <span className="ag-chip" data-tone="neutral">
          {t("agent.answerEvidenceGraph", { count: graphCount })}
        </span>
      ) : null}
    </div>
  );
}

/** The three-dot "thinking" indicator, migrated verbatim from
 * `ChatSidebar.tsx`'s `isChatLoading` block (same CSS animation utility,
 * `chat-dot-bounce`) — reused here rather than imported from that file
 * since `ChatSidebar` is out of this unit's scope (T7 deletes it) and this
 * is presentation-only markup, not shared behavior. */
function AnswerStreamingDots() {
  const { t } = useTranslation();
  return (
    <div
      className="flex gap-(--space-2) py-(--space-3) px-(--space-4) bg-bg-tertiary border border-(--edge) rounded-lg w-fit"
      role="status"
    >
      <span className="sr-only">{t("chat.thinking")}</span>
      <span
        className="w-[6px] h-[6px] rounded-full bg-text-secondary animate-[chat-dot-bounce_1.4s_infinite_ease-in-out_both] [animation-delay:-0.32s]"
        aria-hidden="true"
      />
      <span
        className="w-[6px] h-[6px] rounded-full bg-text-secondary animate-[chat-dot-bounce_1.4s_infinite_ease-in-out_both] [animation-delay:-0.16s]"
        aria-hidden="true"
      />
      <span
        className="w-[6px] h-[6px] rounded-full bg-text-secondary animate-[chat-dot-bounce_1.4s_infinite_ease-in-out_both] [animation-delay:0s]"
        aria-hidden="true"
      />
    </div>
  );
}

/**
 * The threaded answer for one live-assist card (audio-graph-83cc T4,
 * deliverable c). Reads TWO independent sources, in priority order:
 *
 * 1. `draft` (`answerDrafts[proposal.id]`, `store/answerDrafts.ts`) — the
 *    transient in-flight state. `"streaming"` renders the thinking dots;
 *    `"failed"` renders the typed failure text + a Retry affordance.
 * 2. `card.answer` (the durable `CardAnswer`, absent for every legacy
 *    pre-83cc record) — rendered only once there is no active draft, so a
 *    fresh dispatch's progress always wins over a stale durable answer from
 *    a previous turn.
 *
 * Returns `null` (renders nothing) when neither exists — the exact legacy
 * shape: a card with no `answer`/`origin`/`signal` and no draft renders
 * exactly as it did before this ticket (pinned by
 * `AgentProposalsPanel.test.tsx`'s legacy-card regression test).
 */
function AnswerThread({
  answer,
  draft,
  onRetry,
  readOnly = false,
}: {
  answer: CardAnswer | null | undefined;
  draft: AnswerDraftState | undefined;
  /** `undefined` (not just omitted) when `readOnly` — the feed row never
   * gains a capability the queue row doesn't already have (see this file's
   * `AgentFeedRow` doc: "no approve/ask/dismiss anywhere in this file, for
   * ANY card"). Required otherwise. */
  onRetry?: () => void;
  /** `true` for `AgentFeedRow` — suppresses the Retry button so a failed
   * thread's ONLY reachable retry is via the queue (Signal/All toggle, same
   * pre-existing "recoverable via All + marker" limitation
   * `FragmentSuspectMarker` already accepts for this row). The failure text
   * itself still renders — read-only means no NEW action, not no content. */
  readOnly?: boolean;
}) {
  const { t } = useTranslation();

  if (draft?.status === "streaming") {
    return <AnswerStreamingDots />;
  }
  if (draft?.status === "failed") {
    return (
      <div className="flex flex-col gap-(--space-2)">
        <p
          className="m-0 text-xs text-(--text-on-tint-danger) bg-(--tint-danger) border border-(--tint-border-danger) rounded-sm py-(--space-2) px-(--space-3) [overflow-wrap:anywhere]"
          role="alert"
        >
          {draft.text || t("agent.answerFailedGeneric")}
        </p>
        {readOnly ? null : (
          <button
            type="button"
            className="self-start border border-(--edge) rounded-sm bg-transparent text-text-secondary cursor-pointer text-xs leading-[20px] py-0 px-(--space-3) hover:text-text-primary hover:border-accent-blue"
            onClick={onRetry}
          >
            {t("agent.answerRetry")}
          </button>
        )}
      </div>
    );
  }

  if (!answer) return null;

  if (answer.status === "failed") {
    return (
      <div className="flex flex-col gap-(--space-2)">
        <p
          className="m-0 text-xs text-(--text-on-tint-danger) bg-(--tint-danger) border border-(--tint-border-danger) rounded-sm py-(--space-2) px-(--space-3) [overflow-wrap:anywhere]"
          role="alert"
        >
          {answer.text || t("agent.answerFailedGeneric")}
        </p>
        {readOnly ? null : (
          <button
            type="button"
            className="self-start border border-(--edge) rounded-sm bg-transparent text-text-secondary cursor-pointer text-xs leading-[20px] py-0 px-(--space-3) hover:text-text-primary hover:border-accent-blue"
            onClick={onRetry}
          >
            {t("agent.answerRetry")}
          </button>
        )}
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-(--space-2)">
      {answer.status === "interrupted" ? (
        <span className="ag-chip self-start" data-tone="neutral">
          {t("agent.answerInterrupted")}
        </span>
      ) : null}
      <p className="m-0 text-sm text-text-secondary leading-[1.4] [overflow-wrap:anywhere]">
        {answer.text}
        {answer.truncated ? (
          <span
            className="text-text-muted cursor-help"
            title={t("agent.answerTruncatedHint")}
          >
            {" "}
            {t("agent.answerTruncatedMarker")}
            <span className="sr-only">{t("agent.answerTruncatedHint")}</span>
          </span>
        ) : null}
      </p>
      <AnswerEvidenceChips answer={answer} />
    </div>
  );
}

/** A queue row — the exact markup/handlers the pre-W8 panel rendered for an
 * actionable card, unchanged (approve/ask/dismiss per-kind actions). Only
 * ever rendered for `selectAgentQueue`'s `queue` list, i.e. `isActionable`
 * was already `true` at the selector — this component never re-checks it.
 * Ticket W9: `isFragmentSuspect` is ONLY ever `true` here in All mode (Signal
 * mode never admits a fragment-suspect card into the queue at all) — see
 * `FragmentSuspectMarker`'s doc for why this doesn't touch the approve/ask/
 * dismiss controls below at all (design-a §3.1: "the queue is a priority
 * filter, never a capability gate" — this ticket does not weaken that for
 * All mode either). */
function AgentQueueRow({
  card,
  isApproving,
  isFragmentSuspect,
  duplicateCount,
  draft,
  onApprove,
  onAsk,
  onDismiss,
}: AgentQueueRowProps) {
  const { t } = useTranslation();
  const proposal = card.proposal;
  // audio-graph-83cc T4: a question card that has ever had a dispatch (a
  // draft) or already carries a durable answer shows the thread instead of
  // the old static "✓ Added to graph" line + Ask AI button — graft G3's
  // adopted-at-zero-cost item from the design panel synthesis ("the tile
  // must stop presenting '✓ question added to graph' as the card's only
  // feedback once a thread exists"). A card with NEITHER (every legacy
  // pre-83cc record, and every question nobody has asked yet this session)
  // renders exactly as it did before this ticket — pinned by
  // `AgentProposalsPanel.test.tsx`'s legacy-card regression test.
  const hasThread = draft !== undefined || card.answer != null;
  return (
    <li className="border border-(--edge) rounded-md p-(--space-4) bg-bg-tertiary">
      <div className="flex justify-between text-text-muted text-xs mb-(--space-2)">
        <div className="flex min-w-0 flex-wrap items-center gap-(--space-2)">
          <span>{t(proposalKindKey(proposal.kind))}</span>
          <AgentStatusChip card={card} />
          {isFragmentSuspect ? <FragmentSuspectMarker /> : null}
          {duplicateCount > 1 ? (
            <DuplicateCountBadge count={duplicateCount} />
          ) : null}
        </div>
        <span>{formatConfidence(proposal.confidence)}</span>
      </div>
      <h3 className="text-text-primary text-md leading-[1.3] m-0 mb-(--space-2)">
        {proposal.title}
      </h3>
      <p className="text-text-secondary text-sm leading-[1.4] m-0 mb-(--space-4) [overflow-wrap:anywhere]">
        {proposal.body}
      </p>
      {proposal.kind === "question" ? (
        <>
          {hasThread ? (
            <div className="mb-(--space-4)">
              <AnswerThread
                answer={card.answer}
                draft={draft}
                onRetry={() => onAsk(proposal.id)}
              />
            </div>
          ) : (
            <p className="text-accent-green text-xs m-0 mb-(--space-4)">
              <Icon name="check" size={14} /> {t("agent.questionAdded")}
            </p>
          )}
          <div className="flex gap-(--space-3) justify-end">
            {hasThread ? null : (
              <button
                type="button"
                className="border border-accent-green rounded-sm bg-transparent text-accent-green cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:bg-(--tint-success) hover:text-accent-green disabled:cursor-not-allowed disabled:opacity-55"
                onClick={() => void onAsk(proposal.id)}
              >
                {t("agent.askAi")}
              </button>
            )}
            <button
              type="button"
              className="border border-(--edge) rounded-sm bg-transparent text-text-secondary cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:text-text-primary hover:border-accent-blue disabled:cursor-not-allowed disabled:opacity-55"
              onClick={() => void onDismiss(proposal.id)}
            >
              {t("agent.dismiss")}
            </button>
          </div>
        </>
      ) : (
        <div className="flex gap-(--space-3) justify-end">
          <button
            type="button"
            className="border border-accent-green rounded-sm bg-transparent text-accent-green cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:bg-(--tint-success) hover:text-accent-green disabled:cursor-not-allowed disabled:opacity-55"
            disabled={isApproving}
            onClick={() => void onApprove(proposal.id)}
          >
            {isApproving ? t("agent.applying") : t("agent.addToGraph")}
          </button>
          <button
            type="button"
            className="border border-(--edge) rounded-sm bg-transparent text-text-secondary cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:text-text-primary hover:border-accent-blue disabled:cursor-not-allowed disabled:opacity-55"
            disabled={isApproving}
            onClick={() => void onDismiss(proposal.id)}
          >
            {t("agent.dismiss")}
          </button>
        </div>
      )}
    </li>
  );
}

/** A feed row — compact, read-only, "counts and short labels, no content
 * dumps" (ticket W8 item 5): title only BY DEFAULT, never the proposal
 * `body` text inlined. An approved card's outcome message is short
 * evidence, not a content dump, so it still renders here unconditionally
 * (it was already visible in the pre-W8 single-list view).
 *
 * The truncated title carries a native `title=` tooltip, and — when the
 * proposal has a body — a per-row disclosure toggle reveals it on demand
 * (reuses the existing `notifications.details`/`hideDetails` copy rather
 * than minting new i18n keys). This keeps the default render a genuine
 * "no content dumps" summary while keeping the review finding's concern
 * satisfied: nothing in the feed is permanently unreachable, it is just
 * collapsed by default.
 *
 * Ticket W9: `isFragmentSuspect` is `true` here in Signal mode (the default)
 * for a card that WOULD be actionable but was filtered — it renders the same
 * `FragmentSuspectMarker` the queue row does, but this component's own
 * read-only shape (no approve/ask/dismiss anywhere in this file, for ANY
 * card) is completely unaffected: adding the marker prop does not, and must
 * not, add a capability.
 *
 * DEVIATION FROM design-a §3.1, NAMED (review finding, ticket item 4/(d)):
 * that section's feed contract reads "a row's overflow `Popover` still
 * exposes approve/dismiss, so nothing in the feed is unreachable — the
 * queue is a priority filter, never a capability gate." This component has
 * no popover and never has (pre-existing since W8; this ticket only adds
 * the marker). Before W9, that gap was inert — nothing actionable could
 * ever land here. As of W9's Signal default, an actionable
 * `fragment_suspect` card DOES land here routinely, and the ONLY way back
 * to actionable is the header's Signal/All toggle, not an in-row control —
 * a narrower guarantee than design-a's, accepted under R6's "recoverable
 * via All + marker" framing rather than the doc's stronger claim. Recorded
 * here as a deviation rather than asserted as compliance. */
function AgentFeedRow({
  card,
  isFragmentSuspect,
  duplicateCount,
  draft,
}: {
  card: LiveAssistCardRecord;
  isFragmentSuspect: boolean;
  duplicateCount: number;
  /** `answerDrafts[card.proposal.id]` (audio-graph-83cc T4) — see
   * `AgentQueueRowProps.draft`'s doc. Reaches this row today only via the
   * Signal-mode fragment-suspect path (a still-actionable card `admitToQueue`
   * filtered here) — see `AnswerThread`'s `readOnly` doc for why Retry does
   * not render here. */
  draft: AnswerDraftState | undefined;
}) {
  const { t } = useTranslation();
  const [bodyExpanded, setBodyExpanded] = useState(false);
  const proposal = card.proposal;
  const hasThread = draft !== undefined || card.answer != null;
  const approvedOutcome =
    card.status === "approved" ? formatApprovedOutcome(card) : null;
  const projectionPatchEvidence =
    card.status === "approved" ? formatProjectionPatchEvidence(card) : null;
  return (
    <li className="flex flex-col gap-(--space-1) py-(--space-2) border-b border-(--edge-subtle) last:border-b-0">
      <div className="flex min-w-0 items-center gap-(--space-2) text-xs text-text-muted">
        <span>{t(proposalKindKey(proposal.kind))}</span>
        <AgentStatusChip card={card} />
        {isFragmentSuspect ? <FragmentSuspectMarker /> : null}
        {duplicateCount > 1 ? (
          <DuplicateCountBadge count={duplicateCount} />
        ) : null}
      </div>
      <p
        className="m-0 text-sm text-text-secondary truncate"
        title={proposal.title}
      >
        {proposal.title}
      </p>
      {approvedOutcome ? (
        <p className="m-0 text-xs text-text-muted [overflow-wrap:anywhere]">
          <span className="text-text-muted">{t("agent.outcome")}: </span>
          {approvedOutcome}
        </p>
      ) : null}
      {projectionPatchEvidence ? (
        <p className="m-0 text-xs text-text-muted [overflow-wrap:anywhere]">
          <span className="text-text-muted">
            {t("agent.projectionPatch")}:{" "}
          </span>
          {projectionPatchEvidence}
        </p>
      ) : null}
      {proposal.body || hasThread ? (
        <>
          <button
            type="button"
            className="self-start border-none bg-transparent p-0 m-0 text-xs text-text-muted underline cursor-pointer hover:text-text-primary"
            aria-expanded={bodyExpanded}
            onClick={() => setBodyExpanded((expanded) => !expanded)}
          >
            {bodyExpanded
              ? t("notifications.hideDetails")
              : t("notifications.details")}
          </button>
          {bodyExpanded ? (
            <>
              {proposal.body ? (
                <p className="m-0 text-xs text-text-secondary [overflow-wrap:anywhere]">
                  {proposal.body}
                </p>
              ) : null}
              {hasThread ? (
                <AnswerThread answer={card.answer} draft={draft} readOnly />
              ) : null}
            </>
          ) : null}
        </>
      ) : null}
    </li>
  );
}

/** Header-slot content (the Clear action) — see the module doc for why this
 * renders in `WorkspaceTile`'s `headerSlot` rather than inside
 * `AgentProposalsPanel`'s own body. Mirrors `LiveDocumentHeaderActions`'s
 * split exactly. */
export function AgentTileHeaderActions() {
  const { t } = useTranslation();
  const pendingCount = useAudioGraphStore((s) => s.agentProposals.length);
  const hasApproving = useAudioGraphStore(
    (s) => s.approvingAgentProposalIds.length > 0,
  );
  const clearAgentProposals = useAudioGraphStore((s) => s.clearAgentProposals);

  if (pendingCount === 0) return null;

  return (
    <button
      type="button"
      className="border border-(--edge) rounded-sm bg-transparent text-text-secondary text-xs leading-[20px] py-0 px-(--space-4) cursor-pointer hover:text-text-primary hover:border-accent-blue disabled:cursor-not-allowed disabled:opacity-55"
      disabled={hasApproving}
      onClick={() => void clearAgentProposals()}
    >
      {t("agent.clear")}
    </button>
  );
}

/**
 * The agent tile's body. R3 (ratified): the tile is ALWAYS mounted, so this
 * component must render a designed idle state when there is genuinely
 * nothing to show — not `null` (a `null` body inside an always-mounted tile
 * is an empty room with no explanation) and not "coming soon" (dishonest —
 * there is nothing pending, not something withheld).
 *
 * `filter` (ticket W9): optional, defaults to `"signal"` (the ratified
 * default) so every pre-W9 call site keeps compiling and behaving
 * identically. The real `App.tsx` call site always passes the lifted
 * `useAgentQueueFilter()` value explicitly.
 *
 * audio-graph-83cc T4, graft G3: the return below is now a stable outer
 * `<div className="h-full flex flex-col">` with exactly ONE conditionally-
 * rendered scroll region (idle empty state OR the queue/feed body — never
 * both, `AGENT_QUEUE_PANEL_ID` lives on whichever one renders, never
 * duplicated) plus `<AgentComposer />` as a SIBLING below it in every
 * branch. This is the actual fix for the field bug this epic exists to
 * kill: the pre-T4 idle branch returned a whole different JSX tree with no
 * composer in it at all — see `AgentComposer.tsx`'s module doc for the exact
 * synthesis quote. The composer sits outside `AGENT_QUEUE_PANEL_ID` on
 * purpose (that id is the Signal/All tablist's `aria-controls` target, an
 * unrelated contract this ticket must not touch).
 */
function AgentProposalsPanel({
  filter = "signal",
}: {
  filter?: AgentQueueFilterMode;
} = {}) {
  const { t } = useTranslation();
  const proposals = useAudioGraphStore((s) => s.agentProposals);
  const liveAssistCards = useAudioGraphStore((s) => s.liveAssistCards);
  const approvingIds = useAudioGraphStore((s) => s.approvingAgentProposalIds);
  const status = useAudioGraphStore((s) => s.agentStatus);
  const answerDrafts = useAudioGraphStore((s) => s.answerDrafts);
  const approveAgentProposal = useAudioGraphStore(
    (s) => s.approveAgentProposal,
  );
  // audio-graph-83cc T4: the manual "Ask AI" action now dispatches through
  // `answerQuestionCard` (threads the answer under the card) instead of the
  // pre-T4 `askAgentProposal` (dismissed the card, dumped into the
  // unreachable `chatMessages` — the exact field failure this epic exists
  // to kill). `askAgentProposal` itself is left in the store, unchanged and
  // still tested, for T7's later cleanup pass rather than deleted here —
  // see this ticket's final report for the explicit scope note.
  const answerQuestionCard = useAudioGraphStore((s) => s.answerQuestionCard);
  const dismissAgentProposal = useAudioGraphStore(
    (s) => s.dismissAgentProposal,
  );

  // Ticket W9: the ONLY thing the toggle changes is which `admit` predicate
  // `selectAgentQueue` runs — "All" swaps in a stable unconditional
  // `() => true` (`ADMIT_ALL`), never touching the underlying store data, so
  // toggling back to "Signal" always reproduces the exact same filtered
  // view. `admitToQueue` itself (Signal mode) is passed explicitly rather
  // than relying on `selectAgentQueue`'s default parameter, so this call
  // site's behavior is legible without cross-referencing that default.
  const admit = filter === "all" ? ADMIT_ALL : admitToQueue;
  const { queue, feed, fragmentSuspectIds, duplicateCounts } = useMemo(
    () => selectAgentQueue(liveAssistCards, proposals, admit),
    [liveAssistCards, proposals, admit],
  );
  const approving = useMemo(() => new Set(approvingIds), [approvingIds]);
  const isRunning = status?.state === "running";
  const isIdle = queue.length === 0 && feed.length === 0 && !isRunning;

  return (
    <div className="h-full flex flex-col">
      {isIdle ? (
        <div
          id={AGENT_QUEUE_PANEL_ID}
          // `overflow-y-auto` (audio-graph-83cc T4 fix-round finding, minor
          // a11y gate: "200% zoom == compact tier"): the non-idle body below
          // already scrolls instead of overflowing when its content
          // outgrows the tile at a narrow/zoomed compact tier — this empty
          // state had no such handling, so its content could bleed past the
          // tile boundary into the pinned `<AgentComposer>` below it at 200%
          // zoom on a compact tile. Scrolling on overflow rather than
          // clipping or spilling matches the non-idle body's own contract.
          className="flex flex-1 min-h-0 flex-col items-center justify-center gap-(--space-3) overflow-y-auto py-(--space-6) px-(--space-4) text-center select-none"
          data-testid="agent-empty"
        >
          <span className="text-text-muted opacity-40" aria-hidden="true">
            <Icon name="agent" size={32} />
          </span>
          <div className="flex flex-col gap-(--space-2) max-w-[280px]">
            <p className="m-0 text-text-secondary text-md font-medium">
              {t("agent.idleTitle")}
            </p>
            <p className="m-0 text-text-muted text-sm leading-normal">
              {t("agent.idleBody")}
            </p>
          </div>
        </div>
      ) : (
        <div
          id={AGENT_QUEUE_PANEL_ID}
          className="flex-1 min-h-0 overflow-y-auto py-[10px] px-(--space-5)"
          data-testid="agent-body"
        >
          {isRunning ? (
            <div className="text-accent-blue text-sm mb-(--space-4)">
              {status?.message ?? t("agent.working")}
            </div>
          ) : null}
          {queue.length > 0 ? (
            <div className="mb-(--space-5)">
              <p className="ag-label m-0 mb-(--space-3)">
                {t("agent.queueTitle")}
              </p>
              <ul className="flex flex-col gap-(--space-4) list-none m-0 p-0">
                {queue.map((card) => (
                  <AgentQueueRow
                    key={card.proposal.id}
                    card={card}
                    isApproving={approving.has(card.proposal.id)}
                    isFragmentSuspect={fragmentSuspectIds.has(card.proposal.id)}
                    duplicateCount={duplicateCounts.get(card.proposal.id) ?? 1}
                    draft={answerDrafts[card.proposal.id]}
                    onApprove={approveAgentProposal}
                    onAsk={answerQuestionCard}
                    onDismiss={dismissAgentProposal}
                  />
                ))}
              </ul>
            </div>
          ) : null}
          <div>
            <p className="ag-label m-0 mb-(--space-3)">
              {t("agent.feedTitle")}
            </p>
            {feed.length === 0 ? (
              <p className="text-text-muted text-sm m-0">
                {t("agent.feedEmpty")}
              </p>
            ) : (
              <ul className="flex flex-col gap-(--space-2) list-none m-0 p-0">
                {feed.map((card) => (
                  <AgentFeedRow
                    key={card.proposal.id}
                    card={card}
                    isFragmentSuspect={fragmentSuspectIds.has(card.proposal.id)}
                    duplicateCount={duplicateCounts.get(card.proposal.id) ?? 1}
                    draft={answerDrafts[card.proposal.id]}
                  />
                ))}
              </ul>
            )}
          </div>
        </div>
      )}
      <AgentComposer />
    </div>
  );
}

export default AgentProposalsPanel;
