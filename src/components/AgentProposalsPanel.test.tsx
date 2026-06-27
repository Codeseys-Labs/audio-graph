import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type {
  AgentProposalEvent,
  AgentStatusEvent,
  LiveAssistCardRecord,
} from "../types";
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

function card(
  overrides: Omit<Partial<LiveAssistCardRecord>, "proposal"> & {
    proposal?: Partial<AgentProposalEvent>;
  } = {},
): LiveAssistCardRecord {
  const { proposal: proposalOverrides, ...recordOverrides } = overrides;
  const baseProposal = proposal(proposalOverrides ?? {});
  return {
    session_id: "session-1",
    status: "pending",
    source_span_ids: [baseProposal.source_segment_id],
    graph_context_ids: [],
    outcome: null,
    projection_patch_sequence: null,
    created_at_ms: baseProposal.created_at_ms,
    updated_at_ms: baseProposal.created_at_ms,
    ...recordOverrides,
    proposal: { ...baseProposal, ...(proposalOverrides ?? {}) },
  };
}

function itemForText(text: string): HTMLElement {
  const item = screen.getByText(text).closest("li");
  expect(item).not.toBeNull();
  return item as HTMLElement;
}

function resetStore(
  overrides: {
    agentProposals?: AgentProposalEvent[];
    liveAssistCards?: LiveAssistCardRecord[];
    approvingAgentProposalIds?: string[];
    agentStatus?: AgentStatusEvent | null;
  } = {},
) {
  useAudioGraphStore.setState({
    agentProposals: overrides.agentProposals ?? [],
    liveAssistCards: overrides.liveAssistCards ?? [],
    approvingAgentProposalIds: overrides.approvingAgentProposalIds ?? [],
    agentStatus: overrides.agentStatus ?? null,
    approveAgentProposal: vi.fn(async () => null),
    askAgentProposal: vi.fn(async () => {}),
    dismissAgentProposal: vi.fn(async () => null),
    clearAgentProposals: vi.fn(async () => []),
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
    expect(screen.getByText("Pending")).toBeInTheDocument();
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

  it("renders persisted approved and dismissed cards with status and evidence", () => {
    resetStore({
      liveAssistCards: [
        card({
          status: "approved",
          proposal: {
            id: "approved-card",
            kind: "graph_suggestion",
            title: "Approved relationship",
            body: "Alice now owns the launch milestone",
            confidence: 0.91,
          },
          outcome: {
            proposal_id: "approved-card",
            action: "graph_update",
            message: "Added Alice to the launch milestone",
            graph_updated: true,
            timestamp_ms: 20,
          },
          projection_patch_sequence: 17,
        }),
        card({
          status: "dismissed",
          proposal: {
            id: "dismissed-card",
            kind: "note",
            title: "Dismissed reminder",
            body: "No longer relevant",
            confidence: 0.33,
          },
        }),
      ],
    });
    render(<AgentProposalsPanel />);

    const approved = itemForText("Approved relationship");
    expect(within(approved).getByText("Approved")).toBeInTheDocument();
    expect(
      within(approved).getByText("Added Alice to the launch milestone"),
    ).toBeInTheDocument();
    expect(
      within(approved).getByText("Patch sequence 17"),
    ).toBeInTheDocument();
    expect(
      within(approved).queryByRole("button", { name: /add to graph/i }),
    ).not.toBeInTheDocument();
    expect(
      within(approved).queryByRole("button", { name: /dismiss/i }),
    ).not.toBeInTheDocument();

    const dismissed = itemForText("Dismissed reminder");
    expect(within(dismissed).getByText("Dismissed")).toBeInTheDocument();
    expect(
      within(dismissed).queryByRole("button", { name: /dismiss/i }),
    ).not.toBeInTheDocument();
  });

  it("renders loaded pending cards as history without live actions", () => {
    resetStore({
      liveAssistCards: [
        card({
          status: "pending",
          proposal: {
            id: "historical-pending",
            kind: "note",
            title: "Loaded pending card",
            body: "Visible but not resolvable in this runtime",
          },
        }),
      ],
    });
    render(<AgentProposalsPanel />);

    const item = itemForText("Loaded pending card");
    expect(within(item).getByText("Pending")).toBeInTheDocument();
    expect(
      within(item).queryByRole("button", { name: /add to graph/i }),
    ).not.toBeInTheDocument();
    expect(
      within(item).queryByRole("button", { name: /dismiss/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /^clear$/i }),
    ).not.toBeInTheDocument();
  });

  it("keeps resolved cards rendered when pending proposal actions run", () => {
    const approveAgentProposal = vi.fn(async () => null);
    const dismissAgentProposal = vi.fn(async () => null);
    resetStore({
      liveAssistCards: [
        card({
          status: "approved",
          proposal: {
            id: "resolved-card",
            kind: "note",
            title: "Already approved",
            body: "Persisted history stays visible",
          },
          outcome: {
            proposal_id: "resolved-card",
            action: "chat_note",
            message: "Recorded in the graph",
            graph_updated: false,
            timestamp_ms: 21,
          },
          projection_patch_sequence: 21,
        }),
      ],
      agentProposals: [
        proposal({
          id: "pending-card",
          kind: "note",
          title: "Pending proposal",
          body: "Still actionable",
        }),
      ],
    });
    useAudioGraphStore.setState({ approveAgentProposal, dismissAgentProposal });
    render(<AgentProposalsPanel />);

    fireEvent.click(screen.getByRole("button", { name: /add to graph/i }));
    expect(approveAgentProposal).toHaveBeenCalledWith("pending-card");
    expect(screen.getByText("Already approved")).toBeInTheDocument();
    expect(screen.getByText("Recorded in the graph")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /dismiss/i }));
    expect(dismissAgentProposal).toHaveBeenCalledWith("pending-card");
    expect(screen.getByText("Already approved")).toBeInTheDocument();
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

  it("clears all proposals via the Clear button", () => {
    const clearAgentProposals = vi.fn();
    resetStore({ agentProposals: [proposal(), proposal()] });
    useAudioGraphStore.setState({ clearAgentProposals });
    render(<AgentProposalsPanel />);
    const clear = screen.getByRole("button", { name: /^clear$/i });
    fireEvent.click(clear);
    expect(clearAgentProposals).toHaveBeenCalledTimes(1);
  });

  it("disables the Clear button while any proposal is approving", () => {
    const clearAgentProposals = vi.fn();
    resetStore({
      agentProposals: [proposal({ id: "pc1" }), proposal({ id: "pc2" })],
      // Put the store into an approving state: Clear must disable so the user
      // can't wipe proposals mid-apply (AgentProposalsPanel `disabled={approving.size > 0}`).
      approvingAgentProposalIds: ["pc1"],
    });
    useAudioGraphStore.setState({ clearAgentProposals });
    render(<AgentProposalsPanel />);
    const clear = screen.getByRole("button", { name: /^clear$/i });
    expect(clear).toBeDisabled();
    // A disabled button must not invoke the handler when clicked.
    fireEvent.click(clear);
    expect(clearAgentProposals).not.toHaveBeenCalled();
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
