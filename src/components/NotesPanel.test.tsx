import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "../store";
import type { GraphNode, GraphSnapshot, TranscriptSegment } from "../types";
import NotesPanel from "./NotesPanel";

let nodeSeq = 0;

// Minimal GraphNode builder — only `entity_type`, `name`, and `mention_count`
// drive NotesPanel's categorization + sorting.
function node(entity_type: string, name: string, mention_count = 1): GraphNode {
  nodeSeq += 1;
  return {
    id: `n${nodeSeq}`,
    name,
    entity_type,
    val: 1,
    color: "#fff",
    first_seen: 0,
    last_seen: 0,
    mention_count,
  };
}

function snapshot(nodes: GraphNode[]): GraphSnapshot {
  return {
    nodes,
    links: [],
    stats: {} as GraphSnapshot["stats"],
  };
}

function segment(
  overrides: Partial<TranscriptSegment> = {},
): TranscriptSegment {
  return {
    id: crypto.randomUUID(),
    source_id: "system-default",
    speaker_id: null,
    speaker_label: null,
    text: "hello",
    start_time: 0,
    end_time: 1,
    confidence: 1,
    ...overrides,
  };
}

function resetStore(
  overrides: {
    transcriptSegments?: TranscriptSegment[];
    graphSnapshot?: GraphSnapshot;
  } = {},
) {
  useAudioGraphStore.setState({
    transcriptSegments: overrides.transcriptSegments ?? [],
    graphSnapshot: overrides.graphSnapshot ?? snapshot([]),
  });
}

describe("NotesPanel", () => {
  beforeEach(() => {
    nodeSeq = 0;
    resetStore();
  });

  it("always renders the Notes header", () => {
    render(<NotesPanel />);
    expect(screen.getByText(/^Notes$/)).toBeInTheDocument();
  });

  it("shows the empty-state copy when there are no segments or graph nodes", () => {
    render(<NotesPanel />);
    expect(
      screen.getByText(/notes build automatically from the conversation/i),
    ).toBeInTheDocument();
  });

  it("renders a Participants section from diarized speaker labels", () => {
    resetStore({
      transcriptSegments: [
        segment({ speaker_label: "Alice" }),
        segment({ speaker_label: "Bob" }),
        // Duplicate label must be de-duplicated (Set-backed).
        segment({ speaker_label: "Alice" }),
      ],
    });
    render(<NotesPanel />);
    const section = screen
      .getByRole("heading", { name: /Participants/i })
      .closest("section") as HTMLElement;
    expect(within(section).getByText("Alice")).toBeInTheDocument();
    expect(within(section).getByText("Bob")).toBeInTheDocument();
    // Only two distinct participants despite three segments.
    expect(within(section).getAllByText("Alice")).toHaveLength(1);
  });

  it("falls back to Person graph nodes for participants when no speakers are diarized", () => {
    resetStore({
      graphSnapshot: snapshot([
        node("Person", "Carol"),
        node("Person", "Dave"),
      ]),
    });
    render(<NotesPanel />);
    const section = screen
      .getByRole("heading", { name: /Participants/i })
      .closest("section") as HTMLElement;
    expect(within(section).getByText("Carol")).toBeInTheDocument();
    expect(within(section).getByText("Dave")).toBeInTheDocument();
  });

  it("categorizes Question / Task / Decision nodes into their sections", () => {
    resetStore({
      graphSnapshot: snapshot([
        node("Question", "What is the deadline?"),
        node("Task", "Send the report"),
        node("Decision", "Adopt ADR-0013"),
      ]),
    });
    render(<NotesPanel />);
    expect(
      screen.getByRole("heading", { name: /Open questions/i }),
    ).toBeInTheDocument();
    expect(screen.getByText("What is the deadline?")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: /Action items/i }),
    ).toBeInTheDocument();
    expect(screen.getByText("Send the report")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: /^Decisions$/i }),
    ).toBeInTheDocument();
    expect(screen.getByText("Adopt ADR-0013")).toBeInTheDocument();
  });

  it("groups Topic/Organization/Product/Event nodes under Key topics", () => {
    resetStore({
      graphSnapshot: snapshot([
        node("Topic", "Latency"),
        node("Organization", "Acme"),
        node("Product", "Widget"),
        node("Event", "Launch"),
      ]),
    });
    render(<NotesPanel />);
    const section = screen
      .getByRole("heading", { name: /Key topics/i })
      .closest("section") as HTMLElement;
    for (const label of ["Latency", "Acme", "Widget", "Launch"]) {
      expect(within(section).getByText(label)).toBeInTheDocument();
    }
  });

  it("appends a mention-count suffix to topics mentioned more than once", () => {
    resetStore({
      graphSnapshot: snapshot([node("Topic", "Latency", 3)]),
    });
    render(<NotesPanel />);
    // chip text concatenates name + " ·N" when mention_count > 1.
    expect(screen.getByText(/Latency\s*·3/)).toBeInTheDocument();
  });

  it("does not append a suffix to a topic mentioned exactly once", () => {
    resetStore({
      graphSnapshot: snapshot([node("Topic", "Latency", 1)]),
    });
    render(<NotesPanel />);
    const chip = screen.getByText("Latency");
    expect(chip).toBeInTheDocument();
    expect(chip.textContent).not.toMatch(/·/);
  });

  it("matches entity types case-insensitively", () => {
    resetStore({
      graphSnapshot: snapshot([node("question", "lowercased type?")]),
    });
    render(<NotesPanel />);
    expect(
      screen.getByRole("heading", { name: /Open questions/i }),
    ).toBeInTheDocument();
    expect(screen.getByText("lowercased type?")).toBeInTheDocument();
  });

  it("omits sections that have no matching nodes", () => {
    resetStore({
      graphSnapshot: snapshot([node("Task", "Only a task")]),
    });
    render(<NotesPanel />);
    expect(
      screen.getByRole("heading", { name: /Action items/i }),
    ).toBeInTheDocument();
    // No questions / decisions / participants / topics → headings absent.
    expect(
      screen.queryByRole("heading", { name: /Open questions/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: /^Decisions$/i }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole("heading", { name: /Participants/i }),
    ).not.toBeInTheDocument();
  });

  it("orders nodes within a section by descending mention count", () => {
    resetStore({
      graphSnapshot: snapshot([
        node("Task", "low", 1),
        node("Task", "high", 5),
        node("Task", "mid", 3),
      ]),
    });
    render(<NotesPanel />);
    const items = screen.getAllByRole("listitem").map((li) => li.textContent);
    expect(items).toEqual(["high", "mid", "low"]);
  });
});
