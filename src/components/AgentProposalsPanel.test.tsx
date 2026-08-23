import { readFileSync } from "node:fs";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type {
  AgentProposalEvent,
  AgentStatusEvent,
  LiveAssistCardRecord,
} from "../types";
import AgentProposalsPanel, {
  AgentTileHeaderActions,
} from "./AgentProposalsPanel";
import { WorkspaceTile } from "./workspace/WorkspaceTile";

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

  it("renders a designed idle state (not null) when there are no proposals and the agent is idle — R3: the tile is always mounted", () => {
    render(<AgentProposalsPanel />);
    const empty = screen.getByTestId("agent-empty");
    expect(empty).toBeInTheDocument();
    expect(screen.getByText("No suggestions yet")).toBeInTheDocument();
    expect(screen.queryByTestId("agent-body")).not.toBeInTheDocument();
  });

  it("renders the working message while the agent is running with no proposals, without falling into the idle state", () => {
    resetStore({
      agentStatus: {
        state: "running",
        message: "Synthesizing graph",
        timestamp_ms: 1,
      },
    });
    render(<AgentProposalsPanel />);
    expect(screen.getByText("Synthesizing graph")).toBeInTheDocument();
    expect(screen.queryByTestId("agent-empty")).not.toBeInTheDocument();
  });

  it("renders a proposal's title, body, kind, confidence, and a real (non-fallback) Pending status chip in the queue", () => {
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
    // The queue section only, not the feed (nothing resolved yet).
    expect(screen.getByText("Needs you")).toBeInTheDocument();
  });

  it("fills its container's full height with no bottom-strip cap or border (ticket W4/W8: the panel lives inside a full-height bento tile body, not a bottom strip)", () => {
    resetStore({ agentProposals: [proposal()] });
    render(<AgentProposalsPanel />);
    const body = screen.getByTestId("agent-body");
    expect(body).toHaveClass("h-full");
    expect(body).not.toHaveClass("max-h-[240px]");
    expect(body).not.toHaveClass("border-t");
    expect(body).not.toHaveClass("shrink-0");
  });

  it("orders queue proposals newest-first by created_at_ms", () => {
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

  it("renders persisted approved and dismissed cards in the feed, with a real status chip and outcome/patch evidence", () => {
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

    // Feed rows never dump the full `body` text (item 5: "no content dumps").
    expect(
      screen.queryByText("Alice now owns the launch milestone"),
    ).not.toBeInTheDocument();
    expect(screen.queryByText("No longer relevant")).not.toBeInTheDocument();

    const approved = itemForText("Approved relationship");
    expect(within(approved).getByText("Approved")).toBeInTheDocument();
    expect(
      within(approved).getByText("Added Alice to the launch milestone"),
    ).toBeInTheDocument();
    expect(within(approved).getByText("Patch sequence 17")).toBeInTheDocument();
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

    expect(screen.getByText("Recent activity")).toBeInTheDocument();
  });

  it("a feed row's truncated title carries a native title= tooltip, and its body is reachable behind a per-row disclosure toggle (review finding: nothing in the feed may be permanently unreachable)", () => {
    resetStore({
      liveAssistCards: [
        card({
          status: "dismissed",
          proposal: {
            id: "dismissed-disclosure",
            kind: "note",
            title: "Dismissed reminder",
            body: "No longer relevant",
          },
        }),
      ],
    });
    render(<AgentProposalsPanel />);

    const item = itemForText("Dismissed reminder");
    expect(within(item).getByText("Dismissed reminder")).toHaveAttribute(
      "title",
      "Dismissed reminder",
    );

    // Body is not dumped by default (item 5 holds)...
    expect(screen.queryByText("No longer relevant")).not.toBeInTheDocument();

    // ...but it is reachable: opening the row's own disclosure reveals it.
    const toggle = within(item).getByRole("button", { name: "Details" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(toggle);
    expect(within(item).getByText("No longer relevant")).toBeInTheDocument();
    expect(
      within(item).getByRole("button", { name: "Hide details" }),
    ).toHaveAttribute("aria-expanded", "true");

    // And it collapses again.
    fireEvent.click(within(item).getByRole("button", { name: "Hide details" }));
    expect(screen.queryByText("No longer relevant")).not.toBeInTheDocument();
  });

  it("an approved card with a NULL outcome does NOT render the success/Approved chip — renders the demoted Unverified chip instead (design-a §8 S3, mutation-pinned)", () => {
    resetStore({
      liveAssistCards: [
        card({
          status: "approved",
          proposal: {
            id: "approved-no-outcome",
            title: "Unevidenced approval",
          },
          outcome: null,
        }),
      ],
    });
    render(<AgentProposalsPanel />);

    const row = itemForText("Unevidenced approval");
    expect(within(row).queryByText("Approved")).not.toBeInTheDocument();
    const chip = within(row).getByText("Unverified");
    expect(chip).toHaveAttribute("data-tone", "neutral");
    expect(chip).not.toHaveAttribute("data-tone", "success");
  });

  it("an approved card WITH a recorded outcome renders the Approved chip at success tone", () => {
    resetStore({
      liveAssistCards: [
        card({
          status: "approved",
          proposal: {
            id: "approved-with-outcome",
            title: "Evidenced approval",
          },
          outcome: {
            proposal_id: "approved-with-outcome",
            action: "graph_update",
            message: "Done",
            graph_updated: true,
            timestamp_ms: 1,
          },
        }),
      ],
    });
    render(<AgentProposalsPanel />);

    const row = itemForText("Evidenced approval");
    const chip = within(row).getByText("Approved");
    expect(chip).toHaveAttribute("data-tone", "success");
  });

  it("renders loaded pending cards in the feed as history without live actions", () => {
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
    // The queue heading must not appear — nothing actionable exists.
    expect(screen.queryByText("Needs you")).not.toBeInTheDocument();
  });

  it("keeps resolved cards rendered in the feed when a queued proposal's actions run", () => {
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

  it("calls approveAgentProposal when Add to graph is clicked on a queued note", () => {
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

  it("renders queued question proposals with Ask AI and the added-to-graph note", () => {
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
    expect(
      screen.queryByRole("button", { name: /add to graph/i }),
    ).not.toBeInTheDocument();
  });

  it("omits empty confidence for a non-finite value", () => {
    resetStore({
      agentProposals: [proposal({ title: "no conf", confidence: Number.NaN })],
    });
    render(<AgentProposalsPanel />);
    expect(screen.queryByText(/%$/)).not.toBeInTheDocument();
  });
});

describe("AgentTileHeaderActions (ticket W8: the Clear action moved out of the panel body into the tile's headerSlot)", () => {
  beforeEach(() => {
    seq = 0;
    resetStore();
  });

  it("renders nothing when there are no pending proposals", () => {
    const { container } = render(<AgentTileHeaderActions />);
    expect(container).toBeEmptyDOMElement();
  });

  it("clears all proposals via the Clear button", () => {
    const clearAgentProposals = vi.fn();
    resetStore({ agentProposals: [proposal(), proposal()] });
    useAudioGraphStore.setState({ clearAgentProposals });
    render(<AgentTileHeaderActions />);
    const clear = screen.getByRole("button", { name: /^clear$/i });
    fireEvent.click(clear);
    expect(clearAgentProposals).toHaveBeenCalledTimes(1);
  });

  it("disables the Clear button while any proposal is approving, and a disabled click never fires the handler", () => {
    const clearAgentProposals = vi.fn();
    resetStore({
      agentProposals: [proposal({ id: "pc1" }), proposal({ id: "pc2" })],
      approvingAgentProposalIds: ["pc1"],
    });
    useAudioGraphStore.setState({ clearAgentProposals });
    render(<AgentTileHeaderActions />);
    const clear = screen.getByRole("button", { name: /^clear$/i });
    expect(clear).toBeDisabled();
    fireEvent.click(clear);
    expect(clearAgentProposals).not.toHaveBeenCalled();
  });
});

describe("agent tile — single named region (seed 913d duplicate-landmark fix, ticket W8)", () => {
  beforeEach(() => {
    seq = 0;
    resetStore();
  });

  it("AgentProposalsPanel renders NO region/landmark of its own when mounted inside WorkspaceTile — exactly one region total", () => {
    resetStore({ agentProposals: [proposal()] });
    render(
      <WorkspaceTile id="agent" title="Agent">
        <AgentProposalsPanel />
      </WorkspaceTile>,
    );
    const regions = screen.getAllByRole("region");
    expect(regions).toHaveLength(1);
    expect(regions[0]).toHaveAttribute("data-tile", "agent");
  });

  it("the empty (idle) state also carries no second region", () => {
    render(
      <WorkspaceTile id="agent" title="Agent">
        <AgentProposalsPanel />
      </WorkspaceTile>,
    );
    expect(screen.getAllByRole("region")).toHaveLength(1);
    expect(screen.getByTestId("agent-empty")).toBeInTheDocument();
  });

  it("the panel's rendered root carries no aria-label of its own (pre-W8 regression guard: 'Agent proposals' was the duplicate region's accessible name)", () => {
    resetStore({ agentProposals: [proposal()] });
    render(
      <WorkspaceTile id="agent" title="Agent">
        <AgentProposalsPanel />
      </WorkspaceTile>,
    );
    expect(screen.queryByLabelText("Agent proposals")).not.toBeInTheDocument();
  });
});

describe("statusClass() deletion — grep-pin (ticket W8: 0922 discipline, delete not shadow)", () => {
  it("AgentProposalsPanel.tsx no longer defines or calls statusClass()", () => {
    const source = readFileSync(
      "src/components/AgentProposalsPanel.tsx",
      "utf8",
    );
    expect(source).not.toMatch(/statusClass/);
  });

  it("the status chip renders via .ag-chip[data-tone], not a hand-rolled border/text class string", () => {
    seq = 0;
    resetStore({
      agentProposals: [proposal({ title: "chip check" })],
    });
    render(<AgentProposalsPanel />);
    const chip = screen.getByText("Pending");
    expect(chip).toHaveClass("ag-chip");
    expect(chip).toHaveAttribute("data-tone");
    expect(chip.className).not.toMatch(/border-accent-(green|blue)/);
  });
});
