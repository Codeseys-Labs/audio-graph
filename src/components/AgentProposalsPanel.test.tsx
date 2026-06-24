import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { AgentProposalEvent, AgentStatusEvent } from "../types";
import AgentProposalsPanel from "./AgentProposalsPanel";

let seq = 0;

function proposal(
  overrides: Partial<AgentProposalEvent> = {},
): AgentProposalEvent {
  seq += 1;
  return {
    id: `p${seq}`,
    source_segment_id: `seg${seq}`,
    source_id: "system-default",
    speaker_label: null,
    kind: "note",
    title: `Title ${seq}`,
    body: `Body ${seq}`,
    confidence: 0.8,
    created_at_ms: seq,
    ...overrides,
  };
}

function resetStore(
  overrides: {
    agentProposals?: AgentProposalEvent[];
    approvingAgentProposalIds?: string[];
    agentStatus?: AgentStatusEvent | null;
  } = {},
) {
  useAudioGraphStore.setState({
    agentProposals: overrides.agentProposals ?? [],
    approvingAgentProposalIds: overrides.approvingAgentProposalIds ?? [],
    agentStatus: overrides.agentStatus ?? null,
    approveAgentProposal: vi.fn(async () => null),
    askAgentProposal: vi.fn(async () => {}),
    dismissAgentProposal: vi.fn(),
    clearAgentProposals: vi.fn(),
  });
}

describe("AgentProposalsPanel", () => {
  beforeEach(() => {
    seq = 0;
    resetStore();
  });

  it("renders nothing when there are no proposals and the agent is idle", () => {
    const { container } = render(<AgentProposalsPanel />);
    expect(container).toBeEmptyDOMElement();
  });

  it("renders the working message while the agent is running with no proposals", () => {
    resetStore({
      agentStatus: {
        state: "running",
        message: "Synthesizing graph",
        timestamp_ms: 1,
      },
    });
    render(<AgentProposalsPanel />);
    expect(screen.getByText("Synthesizing graph")).toBeInTheDocument();
  });

  it("renders a proposal's title, body, kind, and confidence", () => {
    resetStore({
      agentProposals: [
        proposal({
          kind: "note",
          title: "Follow up with Bob",
          body: "Bob owns the migration",
          confidence: 0.42,
        }),
      ],
    });
    render(<AgentProposalsPanel />);
    expect(screen.getByText("Follow up with Bob")).toBeInTheDocument();
    expect(screen.getByText("Bob owns the migration")).toBeInTheDocument();
    expect(screen.getByText("Note")).toBeInTheDocument();
    expect(screen.getByText("42%")).toBeInTheDocument();
  });

  it("orders proposals newest-first by created_at_ms", () => {
    resetStore({
      agentProposals: [
        proposal({ title: "older", created_at_ms: 1 }),
        proposal({ title: "newer", created_at_ms: 5 }),
      ],
    });
    render(<AgentProposalsPanel />);
    const items = screen.getAllByRole("listitem");
    expect(within(items[0]).getByText("newer")).toBeInTheDocument();
    expect(within(items[1]).getByText("older")).toBeInTheDocument();
  });

  it("calls approveAgentProposal when Add to graph is clicked on a note", () => {
    const approveAgentProposal = vi.fn(async () => null);
    resetStore({ agentProposals: [proposal({ id: "px", kind: "note" })] });
    useAudioGraphStore.setState({ approveAgentProposal });
    render(<AgentProposalsPanel />);
    fireEvent.click(screen.getByRole("button", { name: /add to graph/i }));
    expect(approveAgentProposal).toHaveBeenCalledWith("px");
  });

  it("calls dismissAgentProposal when Dismiss is clicked", () => {
    const dismissAgentProposal = vi.fn();
    resetStore({ agentProposals: [proposal({ id: "pd", kind: "note" })] });
    useAudioGraphStore.setState({ dismissAgentProposal });
    render(<AgentProposalsPanel />);
    fireEvent.click(screen.getByRole("button", { name: /dismiss/i }));
    expect(dismissAgentProposal).toHaveBeenCalledWith("pd");
  });

  it("shows an applying label and disables actions while a proposal is approving", () => {
    resetStore({
      agentProposals: [proposal({ id: "pa", kind: "graph_suggestion" })],
      approvingAgentProposalIds: ["pa"],
    });
    render(<AgentProposalsPanel />);
    const applying = screen.getByRole("button", { name: /applying/i });
    expect(applying).toBeDisabled();
    expect(screen.getByRole("button", { name: /dismiss/i })).toBeDisabled();
  });

  it("renders question proposals with Ask AI and the added-to-graph note", () => {
    const askAgentProposal = vi.fn(async () => {});
    resetStore({
      agentProposals: [proposal({ id: "pq", kind: "question" })],
    });
    useAudioGraphStore.setState({ askAgentProposal });
    render(<AgentProposalsPanel />);
    expect(screen.getByText("Question")).toBeInTheDocument();
    expect(screen.getByText(/added to graph/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /ask ai/i }));
    expect(askAgentProposal).toHaveBeenCalledWith("pq");
    // Questions don't expose an "Add to graph" action.
    expect(
      screen.queryByRole("button", { name: /add to graph/i }),
    ).not.toBeInTheDocument();
  });

  it("clears all proposals via the Clear button and disables it while approving", () => {
    const clearAgentProposals = vi.fn();
    resetStore({ agentProposals: [proposal(), proposal()] });
    useAudioGraphStore.setState({ clearAgentProposals });
    render(<AgentProposalsPanel />);
    const clear = screen.getByRole("button", { name: /^clear$/i });
    fireEvent.click(clear);
    expect(clearAgentProposals).toHaveBeenCalledTimes(1);
  });

  it("omits empty confidence for a non-finite value", () => {
    resetStore({
      agentProposals: [proposal({ title: "no conf", confidence: Number.NaN })],
    });
    render(<AgentProposalsPanel />);
    // No "%" suffix is rendered for a NaN confidence.
    expect(screen.queryByText(/%$/)).not.toBeInTheDocument();
  });
});
