import { useMemo } from "react";
import { useAudioGraphStore } from "../store";
import type { AgentProposalEvent } from "../types";

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
        <section className="agent-proposals" aria-label="Agent proposals">
            <div className="agent-proposals__header">
                <h2 className="panel-title">Agent</h2>
                {ordered.length > 0 ? (
                    <button
                        type="button"
                        className="agent-proposals__clear"
                        disabled={approving.size > 0}
                        onClick={clearAgentProposals}
                    >
                        Clear
                    </button>
                ) : null}
            </div>
            {status?.state === "running" ? (
                <div className="agent-proposals__status">
                    {status.message ?? "Working"}
                </div>
            ) : null}
            <ul className="agent-proposals__list">
                {ordered.map((proposal) => {
                    const isApproving = approving.has(proposal.id);
                    return (
                        <li key={proposal.id} className="agent-proposals__item">
                            <div className="agent-proposals__meta">
                                <span>{proposalKindLabel(proposal.kind)}</span>
                                <span>{formatConfidence(proposal.confidence)}</span>
                            </div>
                            <h3 className="agent-proposals__title">{proposal.title}</h3>
                            <p className="agent-proposals__body">{proposal.body}</p>
                            {proposal.kind === "question" ? (
                                <>
                                    <p className="agent-proposals__hint">
                                        ✓ Added to graph. Optionally ask the AI for an answer.
                                    </p>
                                    <div className="agent-proposals__actions">
                                        <button
                                            type="button"
                                            className="agent-proposals__button agent-proposals__button--approve"
                                            onClick={() => void askAgentProposal(proposal.id)}
                                        >
                                            Ask AI
                                        </button>
                                        <button
                                            type="button"
                                            className="agent-proposals__button"
                                            onClick={() => dismissAgentProposal(proposal.id)}
                                        >
                                            Dismiss
                                        </button>
                                    </div>
                                </>
                            ) : (
                                <div className="agent-proposals__actions">
                                    <button
                                        type="button"
                                        className="agent-proposals__button agent-proposals__button--approve"
                                        disabled={isApproving}
                                        onClick={() => void approveAgentProposal(proposal.id)}
                                    >
                                        {isApproving ? "Applying" : "Add to graph"}
                                    </button>
                                    <button
                                        type="button"
                                        className="agent-proposals__button"
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
