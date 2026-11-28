import { invoke } from "@tauri-apps/api/core";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import {
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";
import i18n from "../i18n";
import { useReviewStore } from "../session/reviewStore";
import { loadedSession, segment } from "../session/reviewTestUtils";
import { useAudioGraphStore } from "../store";
import { NO_PIPELINE_STATUS } from "../store/captureFacts";
import { captureBackendReply } from "../test/captureBackend";
import { preloadGraphLens } from "../test/lazyChunks";
import type { PendingFinalizingSession, SessionMetadata } from "../types";
import NowStrip from "./NowStrip";
import SessionsBrowser from "./SessionsBrowser";

// react-force-graph-2d draws to a canvas jsdom cannot drive, and importing it
// is slow under whole-suite load: stub it (the Graph lens is lazy-loaded).
vi.mock("react-force-graph-2d", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  return {
    default: React.forwardRef((_props: unknown, ref: React.Ref<unknown>) => {
      React.useImperativeHandle(ref, () => ({
        zoomToFit: () => {},
        d3ReheatSimulation: () => {},
        d3Force: () => ({ strength: () => {}, distance: () => {} }),
      }));
      return React.createElement("div", { "data-testid": "force-graph" });
    }),
  };
});

const mockedInvoke = vi.mocked(invoke);

beforeAll(preloadGraphLens);

/**
 * The just-stopped session B's data stays resident in the global store after
 * Stop. `SessionsBrowser` may show it, and offer "Generate prose summary",
 * exactly while `loadedSessionId === null && pendingFinalizingSession?.id ===
 * row.id` (store truth, see the invariant at the slot's declaration), both
 * while B's row is optimistic and after the sessions list has indexed it. Any
 * action that replaces or clears the resident data clears the slot, so B's row
 * is then gated: never another session's or empty data presented as B's own.
 */

const B_TEXT = "resident line from just-stopped B";

function metadata(id: string, title: string): SessionMetadata {
  return {
    id,
    title,
    created_at: 1_700_000_000_000,
    ended_at: 1_700_000_060_000,
    duration_seconds: 60,
    status: "complete",
    segment_count: 1,
    speaker_count: 0,
    entity_count: 0,
    transcript_path: "",
    graph_path: "",
    deleted: false,
    deleted_at: null,
  };
}

const SESSION_A = metadata("session-a", "Session A");
const SESSION_B = metadata("session-b", "Session B");

const PENDING_B: PendingFinalizingSession = {
  ...SESSION_B,
  status: "active",
  optimistic: true,
};

/** The `sessionId` of a `load_session` call's arguments, narrowed at runtime. */
function sessionIdOf(args: unknown): string {
  if (typeof args === "object" && args !== null && "sessionId" in args) {
    return String(args.sessionId);
  }
  throw new Error("load_session called without a sessionId");
}

function loadSessionIds(): string[] {
  return mockedInvoke.mock.calls
    .filter(([cmd]) => cmd === "load_session")
    .map(([, args]) => sessionIdOf(args));
}

function moreActions(): HTMLElement | null {
  return screen.queryByRole("button", { name: /more actions/i });
}

async function expectResident(): Promise<void> {
  const pane = screen.getByRole("tabpanel");
  expect(
    within(pane).queryByRole("button", { name: /load session/i }),
  ).not.toBeInTheDocument();
  expect(
    within(pane).queryByText(/this view needs the session loaded/i),
  ).not.toBeInTheDocument();
  // The Graph lens is a lazy chunk (preloaded in `beforeAll`), so it mounts
  // after a tick rather than synchronously.
  await waitFor(() => expect(pane).not.toBeEmptyDOMElement());
}

/**
 * B is no longer resident, so its lenses are Review panes: the Transcript lens
 * shows B's OWN transcript read from disk (never the sample's, nothing empty
 * passed off as B's), and the global-store-only actions are withheld.
 */
async function expectReviewOwnData(): Promise<void> {
  fireEvent.click(screen.getByRole("tab", { name: /^transcript$/i }));
  expect(
    await within(screen.getByRole("tabpanel")).findByText("text of session-b"),
  ).toBeVisible();
  for (const lens of [/^notes$/i, /^timeline$/i, /^graph$/i]) {
    fireEvent.click(screen.getByRole("tab", { name: lens }));
    const pane = screen.getByRole("tabpanel");
    expect(
      within(pane).queryByRole("button", { name: /load session/i }),
    ).not.toBeInTheDocument();
    expect(within(pane).queryByText(B_TEXT)).not.toBeInTheDocument();
  }
}

/** Resident B just stopped: capture off, slot names B, nav routed to B. */
function seedJustStoppedB(rows: SessionMetadata[]): void {
  useAudioGraphStore.setState({
    sessions: rows,
    sessionsLoading: false,
    isCapturing: false,
    isTranscribing: false,
    loadedSessionId: null,
    pendingFinalizingSession: PENDING_B,
    transcriptSegments: [segment("b-1", B_TEXT)],
    nav: { dest: "sessions", sessionId: "session-b", lens: "notes" },
  });
}

function mockBackend(): void {
  mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === "list_sessions") return useAudioGraphStore.getState().sessions;
    if (cmd === "load_session") {
      const sessionId = sessionIdOf(args);
      return loadedSession(`${sessionId}-1`, `text of ${sessionId}`, "Alice");
    }
    if (cmd === "purge_expired_sessions") return [];
    if (cmd === "build_session_timeline_cmd")
      return { entries: [], total_count: 0 };
    if (cmd === "load_session_notes_artifacts_cmd")
      return { notes: null, projection_events: [] };
    if (cmd === "load_session_graph_artifact_cmd") return null;
    return captureBackendReply(cmd, args) ?? null;
  });
}

describe("SessionsBrowser: the just-stopped session's resident data", () => {
  beforeEach(async () => {
    mockedInvoke.mockReset();
    localStorage.clear();
    useReviewStore.getState().close();
    await i18n.changeLanguage("en");
    mockBackend();
  });

  afterEach(() => {
    useAudioGraphStore.setState({
      sessions: [],
      isCapturing: false,
      captureStatus: "idle",
      pipelineStatus: NO_PIPELINE_STATUS,
      isTranscribing: false,
      selectedSourceIds: [],
      captureStartTime: null,
      transcriptSegments: [],
      loadedSessionId: null,
      pendingFinalizingSession: null,
      samplePreviewActive: false,
    });
  });

  it("keeps showing B's resident Notes, Timeline and Graph, and the summary action, when Stop resolves B and the row is later indexed, issuing no load", async () => {
    useAudioGraphStore.setState({
      sessions: [],
      sessionsLoading: false,
      isCapturing: true,
      captureStatus: "running",
      pipelineStatus: {
        ...NO_PIPELINE_STATUS,
        revision: 0,
        capture_state: { id: { epoch: 1, seq: 1 }, phase: "running" },
      },
      isTranscribing: true,
      selectedSourceIds: ["system-default"],
      captureStartTime: Date.now(),
      loadedSessionId: null,
      transcriptSegments: [segment("b-1", B_TEXT)],
      getSessionId: vi.fn(async () => "session-b"),
    });
    render(
      <>
        <NowStrip />
        <SessionsBrowser />
      </>,
    );

    // A real Stop that resolves B's id: nav routes to B, B's row is optimistic.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    });
    await waitFor(() =>
      expect(useAudioGraphStore.getState().nav.sessionId).toBe("session-b"),
    );
    const row = await screen.findByTestId("session-session-b");
    expect(row).toHaveTextContent(/finalizing/i);
    expect(useAudioGraphStore.getState().pendingFinalizingSession?.id).toBe(
      "session-b",
    );

    const lenses = [/^notes$/i, /^timeline$/i, /^graph$/i];
    for (const lens of lenses) {
      fireEvent.click(screen.getByRole("tab", { name: lens }));
      await expectResident();
    }
    expect(moreActions()).not.toBeNull();

    // The sessions list refreshes: B's row is no longer optimistic.
    act(() => {
      useAudioGraphStore.setState({ sessions: [SESSION_B] });
    });
    await waitFor(() =>
      expect(screen.getByTestId("session-session-b")).not.toHaveTextContent(
        /finalizing/i,
      ),
    );
    for (const lens of lenses) {
      fireEvent.click(screen.getByRole("tab", { name: lens }));
      await expectResident();
    }
    // The summary stays offered, and nothing was loaded by the transition.
    expect(moreActions()).not.toBeNull();
    expect(loadSessionIds()).toEqual([]);
    expect(mockedInvoke).not.toHaveBeenCalledWith("synthesize_notes");
  });

  it("selecting A loads A (clearing the slot), and returning to B loads B through the ordinary click path", async () => {
    seedJustStoppedB([SESSION_A, SESSION_B]);
    render(<SessionsBrowser />);
    await screen.findByTestId("session-session-b");
    await expectResident();
    expect(moreActions()).not.toBeNull();

    fireEvent.click(screen.getByTestId("session-select-session-a"));
    await waitFor(() =>
      expect(useAudioGraphStore.getState().loadedSessionId).toBe("session-a"),
    );
    expect(new Set(loadSessionIds())).toEqual(new Set(["session-a"]));
    expect(useAudioGraphStore.getState().pendingFinalizingSession).toBeNull();
    // A is loaded: its lenses show, but the summary (backend buffers) is not
    // offered for a loaded historical session.
    await expectResident();
    expect(moreActions()).toBeNull();

    fireEvent.click(screen.getByTestId("session-select-session-b"));
    await waitFor(() =>
      expect(useAudioGraphStore.getState().loadedSessionId).toBe("session-b"),
    );
    expect(new Set(loadSessionIds())).toEqual(
      new Set(["session-a", "session-b"]),
    );
    expect(moreActions()).toBeNull();
  });

  it("shows B's own data from Review after a sample preview is entered and left, never the sample's or an empty resident view", async () => {
    seedJustStoppedB([SESSION_B]);
    render(<SessionsBrowser />);
    await screen.findByTestId("session-session-b");
    await expectResident();

    act(() => {
      useAudioGraphStore.getState().loadSampleSessionPreview("en");
    });
    expect(useAudioGraphStore.getState().pendingFinalizingSession).toBeNull();
    // Sample preview is not B's row.
    expect(
      screen.queryByRole("heading", { name: "Session B" }),
    ).not.toBeInTheDocument();

    act(() => {
      useAudioGraphStore.getState().clearTranscript();
    });
    expect(useAudioGraphStore.getState().samplePreviewActive).toBe(false);
    await screen.findByRole("heading", { name: "Session B" });
    await expectReviewOwnData();
    expect(moreActions()).toBeNull();
    expect(mockedInvoke).toHaveBeenCalledWith("load_session", {
      sessionId: "session-b",
    });
  });

  it("shows B's own data from Review after New session resets the view, with the summary withheld", async () => {
    seedJustStoppedB([SESSION_B]);
    render(<SessionsBrowser />);
    await screen.findByTestId("session-session-b");
    await expectResident();
    expect(moreActions()).not.toBeNull();

    // TokenUsagePanel's "New session": `new_session_cmd`, then this action.
    act(() => {
      useAudioGraphStore.getState().resetSessionView();
    });

    await expectReviewOwnData();
    expect(moreActions()).toBeNull();
    expect(mockedInvoke).not.toHaveBeenCalledWith("synthesize_notes");
  });
});
