import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type {
  GraphNode,
  GraphSnapshot,
  MaterializedNotes,
  ProjectionPatch,
  TranscriptSegment,
} from "../types";
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
    materializedNotes?: MaterializedNotes | null;
    sessionProjectionEvents?: ProjectionPatch[];
  } = {},
) {
  useAudioGraphStore.setState({
    transcriptSegments: overrides.transcriptSegments ?? [],
    graphSnapshot: overrides.graphSnapshot ?? snapshot([]),
    materializedNotes: overrides.materializedNotes ?? null,
    sessionProjectionEvents: overrides.sessionProjectionEvents ?? [],
  });
}

function materializedNotes(
  body = "Ship projection notes.",
  sequence = 1,
): MaterializedNotes {
  return {
    schema_version: 1,
    session_id: "session-1",
    last_sequence: sequence,
    notes: [
      {
        id: "note:decision",
        title: "Decision",
        body,
        tags: ["decision", "projection"],
        updated_by_sequence: sequence,
        updated_at_ms: 1_700_000_000_000 + sequence,
        basis: { transcript_hash: `fnv1a64:${sequence}` },
        provenance: {
          provider: "test",
          model: "projection-test",
          prompt_id: "projection_patch_v1",
        },
      },
    ],
  };
}

function notePatch(sequence: number, body: string): ProjectionPatch {
  return {
    sequence,
    kind: "notes",
    llm_request_id: `llm-notes-${sequence}`,
    basis: { transcript_hash: `fnv1a64:${sequence}` },
    operations: [
      {
        type: "upsert_note",
        id: "note:decision",
        title: "Decision",
        body,
        tags: ["decision"],
      },
    ],
    confidence: 0.9,
    provenance: {
      provider: "test",
      model: "projection-test",
      prompt_id: "projection_patch_v1",
    },
    created_at_ms: 1_700_000_000_000 + sequence,
  };
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

  it("renders an empty-state hero with a positive title and a sample-session CTA", () => {
    render(<NotesPanel />);
    const hero = screen.getByTestId("notes-empty-hero");
    // Positive-framing title (icon + title + explanatory copy + single CTA),
    // matching the Audio Sources / Live Transcript empty-state quality bar.
    expect(
      within(hero).getByText(/your notes will appear here/i),
    ).toBeInTheDocument();
    expect(
      within(hero).getByRole("button", { name: /preview sample session/i }),
    ).toBeInTheDocument();
  });

  it("loads the sample-session preview when the empty-state CTA is clicked", () => {
    const loadSampleSessionPreview = vi.fn();
    useAudioGraphStore.setState({ loadSampleSessionPreview });
    render(<NotesPanel />);
    fireEvent.click(
      screen.getByRole("button", { name: /preview sample session/i }),
    );
    expect(loadSampleSessionPreview).toHaveBeenCalledTimes(1);
  });

  it("renders materialized projection notes as live notes content", () => {
    resetStore({ materializedNotes: materializedNotes() });

    render(<NotesPanel />);

    expect(
      screen.queryByText(/notes build automatically/i),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: /live notes/i }),
    ).toBeInTheDocument();
    const note = screen.getByText("Decision").closest("li") as HTMLElement;
    expect(note).toHaveAttribute("data-note-id", "note:decision");
    expect(
      within(note).getByText("Ship projection notes."),
    ).toBeInTheDocument();
    expect(within(note).getByText("decision")).toBeInTheDocument();
    expect(within(note).getByText("projection")).toBeInTheDocument();
    expect(within(note).getByText(/seq 1/i)).toBeInTheDocument();
  });

  it("retcons a materialized note in place when the same stable id updates", () => {
    const { rerender } = render(<NotesPanel />);

    useAudioGraphStore.setState({
      materializedNotes: materializedNotes("Initial assumption.", 1),
      sessionProjectionEvents: [notePatch(1, "Initial assumption.")],
    });
    rerender(<NotesPanel />);
    const first = screen.getByText("Decision").closest("li") as HTMLElement;
    expect(first).toHaveAttribute("data-note-id", "note:decision");
    expect(within(first).getByText("Initial assumption.")).toBeInTheDocument();

    useAudioGraphStore.setState({
      materializedNotes: materializedNotes("Corrected after later context.", 2),
      sessionProjectionEvents: [
        notePatch(1, "Initial assumption."),
        notePatch(2, "Corrected after later context."),
      ],
    });
    rerender(<NotesPanel />);

    expect(screen.queryByText("Initial assumption.")).not.toBeInTheDocument();
    const updated = screen.getByText("Decision").closest("li") as HTMLElement;
    expect(updated).toHaveAttribute("data-note-id", "note:decision");
    expect(
      within(updated).getByText("Corrected after later context."),
    ).toBeInTheDocument();
    expect(within(updated).getByText(/seq 2/i)).toBeInTheDocument();
    expect(within(updated).getByText(/revised 2x/i)).toBeInTheDocument();
    expect(screen.getAllByText("Decision")).toHaveLength(1);
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
