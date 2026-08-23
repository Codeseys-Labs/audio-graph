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
  AgentQueueFilterToggle,
  AgentTileHeaderActions,
  useAgentQueueFilter,
} from "./AgentProposalsPanel";
import { WorkspaceTile } from "./workspace/WorkspaceTile";

const AGENT_QUEUE_FILTER_STORAGE_KEY = "ag.agentQueueFilter";

/** Harness pairing the header-slot toggle with the persisted hook, the same
 * split `App.tsx` uses (one call site, two consumers) — mirrors
 * `LiveGraphStrip.test.tsx`'s `ModeSwitcherHarness`/`useGraphStripMode`
 * precedent (ticket W7) exactly, for the identical W9 shape. */
function QueueFilterHarness() {
  const [filter, setFilter] = useAgentQueueFilter();
  return (
    <div>
      <AgentQueueFilterToggle mode={filter} onModeChange={setFilter} />
      <AgentProposalsPanel filter={filter} />
    </div>
  );
}

let seq = 0;

/** The exact canned body prefix `agent_proposal_body` (speech/mod.rs) mints
 * for a Question proposal in production — `queueContentText`
 * (`agentQueue.ts`) strips this to recover the underlying utterance, which
 * is what W9's quality/duplicate rules actually inspect (not `title`, which
 * is a formulaic constant in production). Fixtures below that need to stay
 * `actionable`/genuinely-distinct use this rather than relying on `title`
 * text, which the classifier no longer reads for questions. */
function questionBody(text: string): string {
  return `Consider answering or linking this question: ${text}`;
}
function graphSuggestionBody(text: string): string {
  return `Review this for an action item, decision, or relationship: ${text}`;
}

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
      // Ticket W9: the classifier reads the CONTENT recovered from `body`
      // for a question, not `title` (title is a formulaic backend
      // constant in production, e.g. "Question from Speaker 1") — this
      // fixture uses the real production shape (formulaic title, real
      // canned-prefix body) to stay in the queue (this test's INTENT is
      // the Ask AI flow, not fragment classification).
      agentProposals: [
        proposal({
          id: "pq",
          kind: "question",
          title: "Question from Speaker 1",
          body: questionBody("What did the team decide about pricing?"),
        }),
      ],
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

describe("Signal/All queue filter toggle (ticket W9, ratified R6)", () => {
  beforeEach(() => {
    seq = 0;
    localStorage.clear();
  });

  it('defaults to "Signal" with nothing persisted yet', () => {
    resetStore();
    render(<QueueFilterHarness />);
    expect(screen.getByRole("tab", { name: "Signal" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByRole("tab", { name: "All" })).toHaveAttribute(
      "aria-selected",
      "false",
    );
  });

  it("Signal mode hides a fragment-suspect question, All mode reveals it with a low-signal marker — same underlying proposal both times", () => {
    resetStore({
      agentProposals: [
        proposal({
          id: "fragment-1",
          kind: "question",
          title: "what about",
          confidence: 0.9,
        }),
      ],
    });
    render(<QueueFilterHarness />);

    // Signal (default): filtered into the feed, not the queue.
    expect(screen.queryByText("Needs you")).not.toBeInTheDocument();
    const feedRow = itemForText("what about");
    expect(within(feedRow).getByText("Low signal")).toBeInTheDocument();
    expect(
      within(feedRow).queryByRole("button", { name: /ask ai/i }),
    ).not.toBeInTheDocument();

    // Switch to All: the SAME proposal now renders in the queue, fully
    // actionable, still carrying the marker.
    fireEvent.click(screen.getByRole("tab", { name: "All" }));
    expect(screen.getByText("Needs you")).toBeInTheDocument();
    const queueRow = itemForText("what about");
    expect(within(queueRow).getByText("Low signal")).toBeInTheDocument();
    expect(
      within(queueRow).getByRole("button", { name: /ask ai/i }),
    ).toBeInTheDocument();
  });

  /**
   * BLOCKER FIX, END-TO-END THROUGH THE REAL PANEL: production never mints
   * the hand-written title above — every real question is titled
   * "Question from {speaker}" (`agent_proposal_title`, speech/mod.rs), and
   * the actual utterance lives in `body`. Pre-fix, a title-keyed classifier
   * marked EVERY such title `fragment_suspect` unconditionally, so Signal
   * mode (the default) showed zero real questions ever. This proves the
   * fix through the full component tree with the real production shape.
   */
  it("PRODUCTION-SHAPED (blocker fix): the real backend question title 'Question from Speaker 1' stays in the Signal (default) queue when the underlying utterance is well-formed", () => {
    resetStore({
      agentProposals: [
        proposal({
          id: "real-question-1",
          kind: "question",
          title: "Question from Speaker 1",
          body: questionBody("Is this the final budget for the quarter?"),
          confidence: 0.87,
        }),
      ],
    });
    render(<QueueFilterHarness />);

    expect(screen.getByText("Needs you")).toBeInTheDocument();
    const queueRow = itemForText("Question from Speaker 1");
    expect(within(queueRow).queryByText("Low signal")).not.toBeInTheDocument();
    expect(
      within(queueRow).getByRole("button", { name: /ask ai/i }),
    ).toBeInTheDocument();
  });

  /**
   * BLOCKER FIX, END-TO-END THROUGH THE REAL PANEL: three distinct graph
   * suggestions share the identical constant production title ("Possible
   * graph update") — pre-fix, title-keyed duplicate-collapse hid two of
   * these three as "duplicates" even though every body is different.
   */
  it("PRODUCTION-SHAPED (blocker fix): three distinct graph suggestions sharing the identical constant title all render in the queue", () => {
    resetStore({
      agentProposals: [
        proposal({
          id: "gs-a",
          kind: "graph_suggestion",
          title: "Possible graph update",
          body: graphSuggestionBody("follow up with legal about the NDA"),
          created_at_ms: 1,
        }),
        proposal({
          id: "gs-b",
          kind: "graph_suggestion",
          title: "Possible graph update",
          body: graphSuggestionBody("decide on the Q3 roadmap"),
          created_at_ms: 2,
        }),
        proposal({
          id: "gs-c",
          kind: "graph_suggestion",
          title: "Possible graph update",
          body: graphSuggestionBody("action item: ship the migration plan"),
          created_at_ms: 3,
        }),
      ],
    });
    render(<QueueFilterHarness />);

    expect(screen.getAllByText("Possible graph update")).toHaveLength(3);
    expect(screen.queryByText("Low signal")).not.toBeInTheDocument();
    // The feed is empty — nothing was demoted (the pre-fix bug would have
    // demoted two of the three as "duplicates").
    expect(screen.getByText("No activity yet")).toBeInTheDocument();
  });

  /**
   * ×N MARKER (design-a §3.2 rule 2's other half). Three identical-content
   * notes collapse to one Signal queue entry; the surviving row carries
   * "×3".
   */
  it("renders the duplicate-count badge (×N) on the surviving row of a collapsed group", () => {
    resetStore({
      agentProposals: [1, 2, 3].map((n) =>
        proposal({
          id: `dup-note-${n}`,
          kind: "note",
          title: `Context from Speaker ${n}`,
          body: "Keep this context available: the migration deadline moved to Friday",
          created_at_ms: n,
        }),
      ),
    });
    render(<QueueFilterHarness />);

    const queueRow = itemForText("Context from Speaker 3");
    expect(within(queueRow).getByText("×3")).toBeInTheDocument();
  });

  it("persists a filter switch to localStorage and a fresh mount reads it back", () => {
    resetStore();
    const { unmount } = render(<QueueFilterHarness />);
    fireEvent.click(screen.getByRole("tab", { name: "All" }));
    expect(screen.getByRole("tab", { name: "All" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(localStorage.getItem(AGENT_QUEUE_FILTER_STORAGE_KEY)).toBe("all");

    unmount();
    render(<QueueFilterHarness />);
    expect(screen.getByRole("tab", { name: "All" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByRole("tab", { name: "Signal" })).toHaveAttribute(
      "aria-selected",
      "false",
    );
  });

  it('ignores a corrupt/unrecognized persisted value and falls back to "signal"', () => {
    localStorage.setItem(AGENT_QUEUE_FILTER_STORAGE_KEY, "not-a-real-mode");
    resetStore();
    render(<QueueFilterHarness />);
    expect(screen.getByRole("tab", { name: "Signal" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  /**
   * FEED INTEGRITY (ticket W9 item 4): a fragment-suspect ACTIONABLE card
   * filtered from the Signal queue must still be fully actionable in All
   * mode — approve fires the EXACT SAME store action a normal queue row's
   * approve does. This is the assertion that would catch a regression where
   * All mode renders the card but wires it to a read-only/no-op control
   * instead of the real `approveAgentProposal` call.
   */
  it("actionability preserved in All mode: approving a fragment-suspect card calls approveAgentProposal, same as any other queue row", () => {
    const approveAgentProposal = vi.fn(async () => null);
    // Force fragment-suspect via the duplicate-collapse rule (kind-agnostic,
    // so it works without touching the question-only confidence/shape
    // rules): two proposals with the exact same normalized CONTENT (the
    // real production title, "Possible graph update", is IDENTICAL on
    // both — proving the collapse is content-keyed, not title-keyed). The
    // OLDER one gets demoted — that is the one this test approves, proving
    // the demoted-but-still-actionable path.
    resetStore({
      agentProposals: [
        proposal({
          id: "dup-a",
          kind: "graph_suggestion",
          title: "Possible graph update",
          body: graphSuggestionBody("Acme Corp evaluates Postgres"),
          created_at_ms: 1,
        }),
        proposal({
          id: "dup-b",
          kind: "graph_suggestion",
          title: "Possible graph update",
          body: graphSuggestionBody("acme corp evaluates postgres"),
          created_at_ms: 2,
        }),
      ],
    });
    useAudioGraphStore.setState({ approveAgentProposal });
    render(<QueueFilterHarness />);

    fireEvent.click(screen.getByRole("tab", { name: "All" }));
    // Both render in the queue now (All mode), sharing the identical
    // production title "Possible graph update" — the older duplicate is
    // the one carrying the "Low signal" marker (found by marker presence,
    // not by title text, since both rows' titles are now indistinguishable
    // strings by design).
    const rows = screen.getAllByRole("listitem");
    const olderRow = rows.find((row) => within(row).queryByText("Low signal"));
    expect(olderRow).toBeDefined();
    fireEvent.click(
      within(olderRow as HTMLElement).getByRole("button", {
        name: /add to graph/i,
      }),
    );
    expect(approveAgentProposal).toHaveBeenCalledWith("dup-a");
  });

  /**
   * ARIA/KEYBOARD CONTRACT (review finding): `AgentQueueFilterToggle`
   * copied `GraphStripModeSwitcher`'s classNames/roles but originally
   * shipped none of that switcher's roving-tabIndex/arrow-key behavior —
   * this pins the restored contract, mirroring the ACTUAL WAI-ARIA APG
   * tabs pattern (`id`/`aria-controls`/roving `tabIndex`/arrow-key
   * traversal) the repo establishes elsewhere for this exact kind of
   * control.
   */
  it("carries id/aria-controls and roving tabIndex (only the selected tab is a Tab stop)", () => {
    resetStore();
    render(<QueueFilterHarness />);
    const signalTab = screen.getByRole("tab", { name: "Signal" });
    const allTab = screen.getByRole("tab", { name: "All" });

    expect(signalTab).toHaveAttribute("tabindex", "0");
    expect(allTab).toHaveAttribute("tabindex", "-1");
    expect(signalTab).toHaveAttribute("aria-controls");
    expect(allTab).toHaveAttribute(
      "aria-controls",
      signalTab.getAttribute("aria-controls") as string,
    );
    // The controlled id must resolve to a real, currently-mounted element.
    const controlledId = signalTab.getAttribute("aria-controls") as string;
    expect(document.getElementById(controlledId)).not.toBeNull();

    fireEvent.click(allTab);
    expect(signalTab).toHaveAttribute("tabindex", "-1");
    expect(allTab).toHaveAttribute("tabindex", "0");
  });

  it("ArrowRight/ArrowLeft move both selection and focus (WAI-ARIA APG tabs pattern)", () => {
    resetStore();
    render(<QueueFilterHarness />);
    const signalTab = screen.getByRole("tab", { name: "Signal" });
    const allTab = screen.getByRole("tab", { name: "All" });

    fireEvent.keyDown(signalTab, { key: "ArrowRight" });
    expect(allTab).toHaveAttribute("aria-selected", "true");
    expect(allTab).toHaveFocus();

    fireEvent.keyDown(allTab, { key: "ArrowLeft" });
    expect(signalTab).toHaveAttribute("aria-selected", "true");
    expect(signalTab).toHaveFocus();
  });

  it("Home/End jump to the first/last tab", () => {
    resetStore();
    render(<QueueFilterHarness />);
    const signalTab = screen.getByRole("tab", { name: "Signal" });
    const allTab = screen.getByRole("tab", { name: "All" });

    fireEvent.keyDown(signalTab, { key: "End" });
    expect(allTab).toHaveAttribute("aria-selected", "true");

    fireEvent.keyDown(allTab, { key: "Home" });
    expect(signalTab).toHaveAttribute("aria-selected", "true");
  });
});
