import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type { AgentProposalEvent, LiveAssistCardRecord } from "../types";
import Icon from "./Icon";

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

function statusLabel(status: LiveAssistCardRecord["status"]): {
  key: string;
  fallback: string;
} {
  switch (status) {
    case "approved":
      return { key: "agent.statusApproved", fallback: "Approved" };
    case "dismissed":
      return { key: "agent.statusDismissed", fallback: "Dismissed" };
    case "pending":
      return { key: "agent.statusPending", fallback: "Pending" };
  }
}

function statusClass(status: LiveAssistCardRecord["status"]): string {
  const base =
    "rounded-[999px] border px-[6px] py-[1px] text-[11px] leading-[14px]";
  switch (status) {
    case "approved":
      return `${base} border-accent-green text-accent-green`;
    case "dismissed":
      return `${base} border-border-color text-text-muted`;
    case "pending":
      return `${base} border-accent-blue text-accent-blue`;
  }
}

function liveAssistCardFromProposal(
  proposal: AgentProposalEvent,
): LiveAssistCardRecord {
  return {
    session_id: "",
    proposal,
    status: "pending",
    source_span_ids: proposal.source_segment_id
      ? [proposal.source_segment_id]
      : [],
    graph_context_ids: [],
    outcome: null,
    projection_patch_sequence: null,
    created_at_ms: proposal.created_at_ms,
    updated_at_ms: proposal.created_at_ms,
  };
}

function mergeLiveAssistCards(
  liveAssistCards: LiveAssistCardRecord[],
  pendingProposals: AgentProposalEvent[],
): LiveAssistCardRecord[] {
  const byProposalId = new Map<string, LiveAssistCardRecord>();
  for (const card of liveAssistCards) {
    byProposalId.set(card.proposal.id, card);
  }
  for (const proposal of pendingProposals) {
    if (!byProposalId.has(proposal.id)) {
      byProposalId.set(proposal.id, liveAssistCardFromProposal(proposal));
    }
  }
  return [...byProposalId.values()].sort(
    (a, b) =>
      b.updated_at_ms - a.updated_at_ms || b.created_at_ms - a.created_at_ms,
  );
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
  const clearAgentProposals = useAudioGraphStore((s) => s.clearAgentProposals);

  const ordered = useMemo(
    () => mergeLiveAssistCards(liveAssistCards, proposals),
    [liveAssistCards, proposals],
  );
  const approving = useMemo(() => new Set(approvingIds), [approvingIds]);
  const actionableProposalIds = useMemo(
    () => new Set(proposals.map((proposal) => proposal.id)),
    [proposals],
  );
  const pendingCount = proposals.length;

  if (ordered.length === 0 && status?.state !== "running") {
    return null;
  }

  return (
    <section
      className="border-t border-border-color py-[10px] px-(--space-5) max-h-[240px] overflow-y-auto shrink-0"
      aria-label={t("agent.label")}
    >
      <div className="flex items-center justify-between gap-(--space-4) mb-(--space-4)">
        <h2 className="panel-title">{t("agent.title")}</h2>
        {pendingCount > 0 ? (
          <button
            type="button"
            className="border border-border-color rounded-[4px] bg-transparent text-text-secondary text-xs leading-[20px] py-0 px-(--space-4) cursor-pointer hover:text-text-primary hover:border-accent-blue disabled:cursor-not-allowed disabled:opacity-55"
            disabled={approving.size > 0}
            onClick={() => void clearAgentProposals()}
          >
            {t("agent.clear")}
          </button>
        ) : null}
      </div>
      {status?.state === "running" ? (
        <div className="text-accent-blue text-sm mb-(--space-4)">
          {status.message ?? t("agent.working")}
        </div>
      ) : null}
      <ul className="flex flex-col gap-(--space-4) list-none m-0 p-0">
        {ordered.map((card) => {
          const proposal = card.proposal;
          const isApproving = approving.has(proposal.id);
          const status = card.status;
          const label = statusLabel(status);
          const isPending = status === "pending";
          const isActionable =
            isPending && actionableProposalIds.has(proposal.id);
          const approvedOutcome =
            status === "approved" ? formatApprovedOutcome(card) : null;
          const projectionPatchEvidence =
            status === "approved" ? formatProjectionPatchEvidence(card) : null;
          return (
            <li
              key={proposal.id}
              className="border border-border-color rounded-[6px] p-(--space-4) bg-bg-tertiary"
            >
              <div className="flex justify-between text-text-muted text-xs mb-(--space-2)">
                <div className="flex min-w-0 flex-wrap items-center gap-(--space-2)">
                  <span>{t(proposalKindKey(proposal.kind))}</span>
                  <span className={statusClass(status)}>
                    {t(label.key, { defaultValue: label.fallback })}
                  </span>
                </div>
                <span>{formatConfidence(proposal.confidence)}</span>
              </div>
              <h3 className="text-text-primary text-md leading-[1.3] m-0 mb-(--space-2)">
                {proposal.title}
              </h3>
              <p className="text-text-secondary text-sm leading-[1.4] m-0 mb-(--space-4) [overflow-wrap:anywhere]">
                {proposal.body}
              </p>
              {approvedOutcome || projectionPatchEvidence ? (
                <div className="text-xs text-text-secondary leading-[1.4] mb-(--space-4)">
                  {approvedOutcome ? (
                    <p className="m-0 [overflow-wrap:anywhere]">
                      <span className="text-text-muted">
                        {t("agent.outcome", { defaultValue: "Outcome" })}:{" "}
                      </span>
                      {approvedOutcome}
                    </p>
                  ) : null}
                  {projectionPatchEvidence ? (
                    <p className="m-0 [overflow-wrap:anywhere]">
                      <span className="text-text-muted">
                        {t("agent.projectionPatch", {
                          defaultValue: "Projection patch",
                        })}
                        :{" "}
                      </span>
                      {projectionPatchEvidence}
                    </p>
                  ) : null}
                </div>
              ) : null}
              {!isActionable ? null : proposal.kind === "question" ? (
                <>
                  <p className="text-accent-green text-xs m-0 mb-(--space-4)">
                    <Icon name="check" size={14} /> {t("agent.questionAdded")}
                  </p>
                  <div className="flex gap-(--space-3) justify-end">
                    <button
                      type="button"
                      className="border border-accent-green rounded-[4px] bg-transparent text-accent-green cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:bg-(--tint-success) hover:text-accent-green disabled:cursor-not-allowed disabled:opacity-55"
                      onClick={() => void askAgentProposal(proposal.id)}
                    >
                      {t("agent.askAi")}
                    </button>
                    <button
                      type="button"
                      className="border border-border-color rounded-[4px] bg-transparent text-text-secondary cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:text-text-primary hover:border-accent-blue disabled:cursor-not-allowed disabled:opacity-55"
                      onClick={() => void dismissAgentProposal(proposal.id)}
                    >
                      {t("agent.dismiss")}
                    </button>
                  </div>
                </>
              ) : (
                <div className="flex gap-(--space-3) justify-end">
                  <button
                    type="button"
                    className="border border-accent-green rounded-[4px] bg-transparent text-accent-green cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:bg-(--tint-success) hover:text-accent-green disabled:cursor-not-allowed disabled:opacity-55"
                    disabled={isApproving}
                    onClick={() => void approveAgentProposal(proposal.id)}
                  >
                    {isApproving ? t("agent.applying") : t("agent.addToGraph")}
                  </button>
                  <button
                    type="button"
                    className="border border-border-color rounded-[4px] bg-transparent text-text-secondary cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:text-text-primary hover:border-accent-blue disabled:cursor-not-allowed disabled:opacity-55"
                    disabled={isApproving}
                    onClick={() => void dismissAgentProposal(proposal.id)}
                  >
                    {t("agent.dismiss")}
                  </button>
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}

export default AgentProposalsPanel;
