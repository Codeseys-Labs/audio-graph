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
 */
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type { AgentProposalEvent, LiveAssistCardRecord } from "../types";
import Icon from "./Icon";
import { selectAgentQueue } from "./workspace/agentQueue";
import { agentOutcomeChipTone } from "./workspace/liveWorkspaceTone";

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
  onApprove: (proposalId: string) => void;
  onAsk: (proposalId: string) => void;
  onDismiss: (proposalId: string) => void;
}

/** A queue row — the exact markup/handlers the pre-W8 panel rendered for an
 * actionable card, unchanged (approve/ask/dismiss per-kind actions). Only
 * ever rendered for `selectAgentQueue`'s `queue` list, i.e. `isActionable`
 * was already `true` at the selector — this component never re-checks it. */
function AgentQueueRow({
  card,
  isApproving,
  onApprove,
  onAsk,
  onDismiss,
}: AgentQueueRowProps) {
  const { t } = useTranslation();
  const proposal = card.proposal;
  return (
    <li className="border border-(--edge) rounded-md p-(--space-4) bg-bg-tertiary">
      <div className="flex justify-between text-text-muted text-xs mb-(--space-2)">
        <div className="flex min-w-0 flex-wrap items-center gap-(--space-2)">
          <span>{t(proposalKindKey(proposal.kind))}</span>
          <AgentStatusChip card={card} />
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
          <p className="text-accent-green text-xs m-0 mb-(--space-4)">
            <Icon name="check" size={14} /> {t("agent.questionAdded")}
          </p>
          <div className="flex gap-(--space-3) justify-end">
            <button
              type="button"
              className="border border-accent-green rounded-sm bg-transparent text-accent-green cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:bg-(--tint-success) hover:text-accent-green disabled:cursor-not-allowed disabled:opacity-55"
              onClick={() => void onAsk(proposal.id)}
            >
              {t("agent.askAi")}
            </button>
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
 * collapsed by default. */
function AgentFeedRow({ card }: { card: LiveAssistCardRecord }) {
  const { t } = useTranslation();
  const [bodyExpanded, setBodyExpanded] = useState(false);
  const proposal = card.proposal;
  const approvedOutcome =
    card.status === "approved" ? formatApprovedOutcome(card) : null;
  const projectionPatchEvidence =
    card.status === "approved" ? formatProjectionPatchEvidence(card) : null;
  return (
    <li className="flex flex-col gap-(--space-1) py-(--space-2) border-b border-(--edge-subtle) last:border-b-0">
      <div className="flex min-w-0 items-center gap-(--space-2) text-xs text-text-muted">
        <span>{t(proposalKindKey(proposal.kind))}</span>
        <AgentStatusChip card={card} />
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
      {proposal.body ? (
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
            <p className="m-0 text-xs text-text-secondary [overflow-wrap:anywhere]">
              {proposal.body}
            </p>
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
 */
function AgentProposalsPanel() {
  const { t } = useTranslation();
  const proposals = useAudioGraphStore((s) => s.agentProposals);
  const liveAssistCards = useAudioGraphStore((s) => s.liveAssistCards);
  const approvingIds = useAudioGraphStore((s) => s.approvingAgentProposalIds);
  const status = useAudioGraphStore((s) => s.agentStatus);
  const approveAgentProposal = useAudioGraphStore(
    (s) => s.approveAgentProposal,
  );
  const askAgentProposal = useAudioGraphStore((s) => s.askAgentProposal);
  const dismissAgentProposal = useAudioGraphStore(
    (s) => s.dismissAgentProposal,
  );

  const { queue, feed } = useMemo(
    () => selectAgentQueue(liveAssistCards, proposals),
    [liveAssistCards, proposals],
  );
  const approving = useMemo(() => new Set(approvingIds), [approvingIds]);
  const isRunning = status?.state === "running";

  if (queue.length === 0 && feed.length === 0 && !isRunning) {
    return (
      <div
        className="flex flex-col items-center justify-center h-full gap-(--space-3) py-(--space-6) px-(--space-4) text-center select-none"
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
    );
  }

  return (
    <div
      className="h-full overflow-y-auto py-[10px] px-(--space-5)"
      data-testid="agent-body"
    >
      {isRunning ? (
        <div className="text-accent-blue text-sm mb-(--space-4)">
          {status?.message ?? t("agent.working")}
        </div>
      ) : null}
      {queue.length > 0 ? (
        <div className="mb-(--space-5)">
          <p className="ag-label m-0 mb-(--space-3)">{t("agent.queueTitle")}</p>
          <ul className="flex flex-col gap-(--space-4) list-none m-0 p-0">
            {queue.map((card) => (
              <AgentQueueRow
                key={card.proposal.id}
                card={card}
                isApproving={approving.has(card.proposal.id)}
                onApprove={approveAgentProposal}
                onAsk={askAgentProposal}
                onDismiss={dismissAgentProposal}
              />
            ))}
          </ul>
        </div>
      ) : null}
      <div>
        <p className="ag-label m-0 mb-(--space-3)">{t("agent.feedTitle")}</p>
        {feed.length === 0 ? (
          <p className="text-text-muted text-sm m-0">{t("agent.feedEmpty")}</p>
        ) : (
          <ul className="flex flex-col gap-(--space-2) list-none m-0 p-0">
            {feed.map((card) => (
              <AgentFeedRow key={card.proposal.id} card={card} />
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

export default AgentProposalsPanel;
