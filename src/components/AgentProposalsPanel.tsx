import { useMemo } from "react";
import { useAudioGraphStore } from "../store";
import type { AgentProposalEvent } from "../types";
import Icon from "./Icon";

function proposalKindLabel(kind: AgentProposalEvent["kind"]): string {
    switch (kind) {
        case "graph_suggestion":
            return "Graph";
        case "question":
            return "Question";
        case "note":
            return "Note";
    }
}

function formatConfidence(value: number): string {
    if (!Number.isFinite(value)) return "";
    return `${Math.round(value * 100)}%`;
}

function AgentProposalsPanel() {
    const proposals = useAudioGraphStore((s) => s.agentProposals);
    const approvingIds = useAudioGraphStore((s) => s.approvingAgentProposalIds);
    const status = useAudioGraphStore((s) => s.agentStatus);
    const approveAgentProposal = useAudioGraphStore((s) => s.approveAgentProposal);
    const askAgentProposal = useAudioGraphStore((s) => s.askAgentProposal);
    const dismissAgentProposal = useAudioGraphStore((s) => s.dismissAgentProposal);
    const clearAgentProposals = useAudioGraphStore((s) => s.clearAgentProposals);

    const ordered = useMemo(
        () => [...proposals].sort((a, b) => b.created_at_ms - a.created_at_ms),
        [proposals],
    );
    const approving = useMemo(() => new Set(approvingIds), [approvingIds]);

    if (ordered.length === 0 && status?.state !== "running") {
        return null;
    }

    return (
        <section
            className="border-t border-border-color py-[10px] px-(--space-5) max-h-[240px] overflow-y-auto shrink-0"
            aria-label="Agent proposals"
        >
            <div className="flex items-center justify-between gap-(--space-4) mb-(--space-4)">
                <h2 className="panel-title">Agent</h2>
                {ordered.length > 0 ? (
                    <button
                        type="button"
                        className="border border-border-color rounded-[4px] bg-transparent text-text-secondary text-xs leading-[20px] py-0 px-(--space-4) cursor-pointer hover:text-text-primary hover:border-accent-blue disabled:cursor-not-allowed disabled:opacity-55"
                        disabled={approving.size > 0}
                        onClick={clearAgentProposals}
                    >
                        Clear
                    </button>
                ) : null}
            </div>
            {status?.state === "running" ? (
                <div className="text-accent-blue text-sm mb-(--space-4)">
                    {status.message ?? "Working"}
                </div>
            ) : null}
            <ul className="flex flex-col gap-(--space-4) list-none m-0 p-0">
                {ordered.map((proposal) => {
                    const isApproving = approving.has(proposal.id);
                    return (
                        <li
                            key={proposal.id}
                            className="border border-border-color rounded-[6px] p-(--space-4) bg-[rgba(255,255,255,0.03)]"
                        >
                            <div className="flex justify-between text-text-muted text-xs mb-(--space-2)">
                                <span>{proposalKindLabel(proposal.kind)}</span>
                                <span>{formatConfidence(proposal.confidence)}</span>
                            </div>
                            <h3 className="text-text-primary text-md leading-[1.3] m-0 mb-(--space-2)">{proposal.title}</h3>
                            <p className="text-text-secondary text-sm leading-[1.4] m-0 mb-(--space-4) [overflow-wrap:anywhere]">{proposal.body}</p>
                            {proposal.kind === "question" ? (
                                <>
                                    <p className="text-accent-green text-xs m-0 mb-(--space-4)">
                                        <Icon name="check" size={14} /> Added to graph. Optionally ask the AI for an answer.
                                    </p>
                                    <div className="flex gap-(--space-3) justify-end">
                                        <button
                                            type="button"
                                            className="border border-accent-green rounded-[4px] bg-transparent text-accent-green cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:bg-[rgba(74,222,128,0.12)] hover:text-accent-green disabled:cursor-not-allowed disabled:opacity-55"
                                            onClick={() => void askAgentProposal(proposal.id)}
                                        >
                                            Ask AI
                                        </button>
                                        <button
                                            type="button"
                                            className="border border-border-color rounded-[4px] bg-transparent text-text-secondary cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:text-text-primary hover:border-accent-blue disabled:cursor-not-allowed disabled:opacity-55"
                                            onClick={() => dismissAgentProposal(proposal.id)}
                                        >
                                            Dismiss
                                        </button>
                                    </div>
                                </>
                            ) : (
                                <div className="flex gap-(--space-3) justify-end">
                                    <button
                                        type="button"
                                        className="border border-accent-green rounded-[4px] bg-transparent text-accent-green cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:bg-[rgba(74,222,128,0.12)] hover:text-accent-green disabled:cursor-not-allowed disabled:opacity-55"
                                        disabled={isApproving}
                                        onClick={() => void approveAgentProposal(proposal.id)}
                                    >
                                        {isApproving ? "Applying" : "Add to graph"}
                                    </button>
                                    <button
                                        type="button"
                                        className="border border-border-color rounded-[4px] bg-transparent text-text-secondary cursor-pointer text-sm leading-[24px] py-0 px-[10px] hover:text-text-primary hover:border-accent-blue disabled:cursor-not-allowed disabled:opacity-55"
                                        disabled={isApproving}
                                        onClick={() => dismissAgentProposal(proposal.id)}
                                    >
                                        Dismiss
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
