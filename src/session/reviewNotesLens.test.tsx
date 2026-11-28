import { invoke } from "@tauri-apps/api/core";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import NotesPanel from "../components/NotesPanel";
import ReviewLensGate from "../components/ReviewLensGate";
import { useNotesSynthesis } from "../components/useNotesSynthesis";
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import { useReviewStore } from "./reviewStore";
import { loadedSession, segment } from "./reviewTestUtils";
import { SessionViewProvider } from "./SessionViewProvider";

// A spy that still runs the real hook, so a Live panel keeps working.
vi.mock("../components/useNotesSynthesis", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>();
  return {
    ...actual,
    useNotesSynthesis: vi.fn(actual.useNotesSynthesis as () => unknown),
  };
});

const mockedInvoke = vi.mocked(invoke);
const synthesisHook = vi.mocked(useNotesSynthesis);

const CONTENT_FIELDS = [
  "transcriptSegments",
  "graphSnapshot",
  "materializedNotes",
  "materializedProjectionGraph",
  "sessionTimeline",
  "sessionTimelineTotalCount",
  "sessionProjectionEvents",
  "sessionTranscriptEvents",
  "diarizationSpanRevisions",
  "speakers",
  "notesLensStatus",
  "graphLensStatus",
  "loadedSessionId",
  "pendingFinalizingSession",
  "transcriptSeekTarget",
  "graphEdgeFocus",
  "asrPartial",
  "liveAssistCards",
  "agentProposals",
  "samplePreviewActive",
  "isCapturing",
  "isTranscribing",
] as const;

function liveContent(): Record<string, unknown> {
  const state = useAudioGraphStore.getState();
  return Object.fromEntries(
    CONTENT_FIELDS.map((field) => [field, structuredClone(state[field])]),
  );
}

/** A reviewed session whose Notes are EMPTY: no notes, no entities, no speakers. */
function mockEmptySession(notesFailures = 0): void {
  let failures = notesFailures;
  mockedInvoke.mockImplementation(async (cmd: string) => {
    if (cmd === "load_session") {
      return {
        ...loadedSession("past-1", "a historical line", "ignored"),
        diarization_events: [],
      };
    }
    if (cmd === "load_session_notes_artifacts_cmd") {
      if (failures > 0) {
        failures -= 1;
        throw { code: "io", message: "disk gone" };
      }
      return { notes: null, projection_events: [] };
    }
    return null;
  });
}

function renderReviewNotes() {
  return render(
    <SessionViewProvider reviewSessionId="past" reviewLens="notes">
      <ReviewLensGate kind="notes">
        <NotesPanel />
      </ReviewLensGate>
    </SessionViewProvider>,
  );
}

describe("Review Notes lens", () => {
  beforeEach(async () => {
    mockedInvoke.mockReset();
    synthesisHook.mockClear();
    useReviewStore.getState().close();
    await i18n.changeLanguage("en");
    // A live capture is running with its own content in the global store.
    useAudioGraphStore.setState({
      isCapturing: true,
      isTranscribing: true,
      samplePreviewActive: false,
      loadedSessionId: null,
      pendingFinalizingSession: null,
      transcriptSegments: [segment("live-1", "live capture line")],
    });
  });

  it("offers no sample preview in its empty state, and no control there changes the Live store", async () => {
    mockEmptySession();
    renderReviewNotes();

    const hero = await screen.findByTestId("notes-empty-hero");
    expect(hero).toHaveTextContent("This session has no notes.");
    // `loadSampleSessionPreview` replaces the LIVE session's content.
    expect(
      screen.queryByRole("button", { name: /sample/i }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/preview sample/i)).not.toBeInTheDocument();
    // Nor a synthesis trigger, which reads the backend's current buffers.
    expect(document.querySelector("[data-notes-synthesize]")).toBeNull();
    expect(
      screen.queryByRole("button", { name: /synthes/i }),
    ).not.toBeInTheDocument();

    // Exercise every control the empty state offers.
    const before = liveContent();
    for (const control of screen.queryAllByRole("button")) {
      fireEvent.click(control);
    }
    await act(async () => {});
    expect(liveContent()).toEqual(before);
    expect(mockedInvoke).not.toHaveBeenCalledWith("synthesize_notes");
  });

  it("changes nothing in the Live store from the error state's retry either", async () => {
    mockEmptySession(1);
    renderReviewNotes();
    await screen.findByTestId("review-lens-unavailable");
    const before = liveContent();

    for (const control of screen.queryAllByRole("button")) {
      fireEvent.click(control);
    }
    await screen.findByTestId("notes-empty-hero");
    expect(liveContent()).toEqual(before);
    expect(
      screen.queryByRole("button", { name: /sample/i }),
    ).not.toBeInTheDocument();
  });

  it("shows no graph-derived chips in Review, since the snapshot's graph can still hold what the materialized graph retracted", async () => {
    const topic = {
      id: "topic-1",
      name: "Retracted topic",
      entity_type: "Topic",
      val: 1,
      color: "#60a5fa",
      first_seen: 0,
      last_seen: 0,
      mention_count: 3,
    };
    const graph = {
      nodes: [topic],
      links: [],
      stats: { total_nodes: 1, total_edges: 0, total_episodes: 0 },
    };
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "load_session") {
        return {
          ...loadedSession("past-1", "a historical line", "ignored"),
          diarization_events: [],
          graph,
        };
      }
      if (cmd === "load_session_notes_artifacts_cmd") {
        return { notes: null, projection_events: [] };
      }
      return null;
    });
    renderReviewNotes();
    await screen.findByTestId("notes-empty-hero");
    expect(screen.queryByText(/Retracted topic/)).not.toBeInTheDocument();

    // Positive control: a Live panel derives chips from the Live graph.
    useAudioGraphStore.setState({
      isCapturing: false,
      isTranscribing: false,
      graphSnapshot: graph,
    });
    render(<NotesPanel />);
    expect(await screen.findByText(/Retracted topic/)).toBeInTheDocument();
    useAudioGraphStore.setState({
      graphSnapshot: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
    });
  });

  it("does not mount the synthesis controller, so it adds no settings or loaded-session subscription", async () => {
    mockEmptySession();
    renderReviewNotes();
    await screen.findByTestId("notes-empty-hero");
    expect(synthesisHook).not.toHaveBeenCalled();
  });

  it("still offers the sample preview, and mounts the controller, for a Live Notes panel", async () => {
    // Positive controls: the guard is specific to a Review provider.
    useAudioGraphStore.setState({
      isCapturing: false,
      isTranscribing: false,
      transcriptSegments: [],
    });
    render(<NotesPanel />);
    expect(
      await screen.findByRole("button", { name: /preview sample/i }),
    ).toBeInTheDocument();
    expect(synthesisHook).toHaveBeenCalled();
    await waitFor(() =>
      expect(document.querySelector("[data-notes-synthesize]")).not.toBeNull(),
    );
  });
});
