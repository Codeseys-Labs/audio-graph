import { invoke } from "@tauri-apps/api/core";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import { useReviewStore } from "../session/reviewStore";
import {
  deferred,
  diarizationSpan,
  loadedSession,
  segment,
} from "../session/reviewTestUtils";
import { useAudioGraphStore } from "../store";
import type { SessionMetadata } from "../types";
import { downloadAsFile } from "../utils/download";
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

vi.mock("../utils/download", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  downloadAsFile: vi.fn(),
}));

const mockedInvoke = vi.mocked(invoke);
const mockedDownload = vi.mocked(downloadAsFile);

/**
 * Review across Start and Stop (PR #536 review rounds 1 and 2).
 *
 * Rule A: the global store is loaded only by an explicit user action (row
 * selection, the "Load session" / retry button), never by an effect, and never
 * while capture runs or between Start and its acknowledgement.
 * Rule B: one predicate (`loadedSessionId` equals the selected id, and no run
 * is active) decides whether anything that reads the global store or the
 * backend's current buffers may be shown or run for the selected row.
 *
 * Live session B's data is resident in the global store while the user reviews
 * completed session A.
 */

const SESSION_A: SessionMetadata = {
  id: "session-a",
  title: "Session A",
  created_at: 1_700_000_000_000,
  ended_at: null,
  duration_seconds: 60,
  status: "complete",
  segment_count: 1,
  speaker_count: 1,
  entity_count: 0,
  transcript_path: "",
  graph_path: "",
  deleted: false,
  deleted_at: null,
};

const A_TEXT = "line from session A";
const B_TEXT = "line from live session B";

function payloadA() {
  const payload = loadedSession("a-1", A_TEXT, "Alice");
  payload.diarization_events = [diarizationSpan("a-1", "spk-a", "Alice")];
  return payload;
}

/** Live session B: capture running, B's data resident in the global store. */
function startLiveSessionB(): void {
  useAudioGraphStore.setState({
    sessions: [SESSION_A],
    sessionsLoading: false,
    isCapturing: true,
    isTranscribing: true,
    selectedSourceIds: ["system-default"],
    captureStartTime: Date.now(),
    transcriptSegments: [segment("b-1", B_TEXT)],
    loadedSessionId: null,
    exportTranscript: vi.fn(async () => '{"live-export":"B"}'),
    getSessionId: vi.fn(async () => "session-b"),
  });
}

/** Capture ends but `nav` keeps naming A (Stop could not resolve B's id). */
function stopCaptureWithoutReroute(): void {
  useAudioGraphStore.setState({ isCapturing: false, isTranscribing: false });
}

interface BackendControl {
  /** Make the next notes-artifact query fail (a first-load failure). */
  failNotesOnce: () => void;
}

/** `load_session` and the lens artifact commands answer A's data. */
function mockBackend(): BackendControl {
  let failNotes = false;
  mockedInvoke.mockImplementation(async (cmd: string) => {
    if (cmd === "list_sessions") return [SESSION_A];
    if (cmd === "load_session") return payloadA();
    if (cmd === "load_session_notes_artifacts_cmd") {
      if (failNotes) {
        failNotes = false;
        throw { code: "io", message: "disk gone: C:\\secret" };
      }
      return { notes: null, projection_events: [] };
    }
    if (cmd === "load_session_graph_artifact_cmd") return null;
    if (cmd === "purge_expired_sessions") return [];
    if (cmd === "build_session_timeline_cmd")
      return { entries: [], total_count: 0 };
    return null;
  });
  return {
    failNotesOnce: () => {
      failNotes = true;
    },
  };
}

function loadSessionCalls(): number {
  return mockedInvoke.mock.calls.filter(([cmd]) => cmd === "load_session")
    .length;
}

async function exportJson(): Promise<Array<Record<string, unknown>>> {
  mockedDownload.mockClear();
  await act(async () => {
    fireEvent.click(
      screen.getByRole("button", { name: /export transcript as json/i }),
    );
  });
  await waitFor(() => expect(mockedDownload).toHaveBeenCalledTimes(1));
  expect(mockedDownload.mock.calls[0][1]).toMatch(/^transcript-session-a-/);
  return JSON.parse(mockedDownload.mock.calls[0][0] as string);
}

async function exportTxt(): Promise<string> {
  mockedDownload.mockClear();
  await act(async () => {
    fireEvent.click(
      screen.getByRole("button", { name: /export transcript as plain text/i }),
    );
  });
  await waitFor(() => expect(mockedDownload).toHaveBeenCalledTimes(1));
  expect(mockedDownload.mock.calls[0][1]).toMatch(/^transcript-session-a-/);
  return mockedDownload.mock.calls[0][0] as string;
}

/** Both exports of the Transcript lens are A's, never B's or the live one's. */
async function expectBothExportsBoundToA(): Promise<void> {
  fireEvent.click(screen.getByRole("tab", { name: /^transcript$/i }));
  const pane = screen.getByRole("tabpanel");
  expect(await within(pane).findByText(A_TEXT)).toBeVisible();
  expect(within(pane).queryByText(B_TEXT)).not.toBeInTheDocument();

  const txt = await exportTxt();
  expect(txt).toContain(A_TEXT);
  expect(txt).not.toContain(B_TEXT);

  const segments = await exportJson();
  expect(segments).toMatchObject([{ text: A_TEXT, speaker_label: "Alice" }]);
  expect(JSON.stringify(segments)).not.toContain(B_TEXT);
  expect(useAudioGraphStore.getState().exportTranscript).not.toHaveBeenCalled();
  expect(mockedInvoke).not.toHaveBeenCalledWith("export_transcript");
}

/** No command that reads the backend's current buffers was ever issued. */
function expectNoBackendBufferCommand(): void {
  const commands = mockedInvoke.mock.calls.map(([cmd]) => cmd);
  expect(commands).not.toContain("synthesize_notes");
  expect(commands).not.toContain("export_transcript");
}

function moreActions(): HTMLElement | null {
  return screen.queryByRole("button", { name: /more actions/i });
}

async function openSessionAWhileLive(): Promise<void> {
  fireEvent.click(await screen.findByTestId("session-select-session-a"));
  expect(await screen.findByText(A_TEXT)).toBeVisible();
}

/**
 * Notes, Timeline and Graph read A's Review data after the run ended: B's
 * resident data never shows, no Load step is needed, and the global store was
 * not loaded (B's resident data is untouched).
 */
async function expectLensesShowAOnly(): Promise<void> {
  for (const lens of [/^notes$/i, /^timeline$/i, /^graph$/i]) {
    fireEvent.click(screen.getByRole("tab", { name: lens }));
    const pane = screen.getByRole("tabpanel");
    await waitFor(() =>
      expect(within(pane).queryByText(/^loading/i)).not.toBeInTheDocument(),
    );
    expect(within(pane).queryByText(B_TEXT)).not.toBeInTheDocument();
    expect(
      within(pane).queryByRole("button", { name: /load session/i }),
    ).not.toBeInTheDocument();
  }
  expect(mockedInvoke).toHaveBeenCalledWith(
    "load_session_notes_artifacts_cmd",
    { sessionId: "session-a" },
  );
  expect(mockedInvoke).toHaveBeenCalledWith("load_session_graph_artifact_cmd", {
    sessionId: "session-a",
  });
  expect(mockedInvoke).toHaveBeenCalledWith("build_session_timeline_cmd", {
    sessionId: "session-a",
    limit: expect.any(Number),
  });
  expect(useAudioGraphStore.getState().loadedSessionId).toBeNull();
  expect(useAudioGraphStore.getState().transcriptSegments[0].text).toBe(B_TEXT);
}

describe("SessionsBrowser: Review across Start and Stop", () => {
  beforeEach(async () => {
    mockedInvoke.mockReset();
    mockedDownload.mockReset();
    localStorage.clear();
    useReviewStore.getState().close();
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    useAudioGraphStore.setState({
      sessions: [],
      isCapturing: false,
      captureStarting: false,
      captureSourceIds: [],
      isTranscribing: false,
      selectedSourceIds: [],
      captureStartTime: null,
      transcriptSegments: [],
      loadedSessionId: null,
      materializedNotes: null,
    });
  });

  describe("unresolved Stop of B while A is reviewed", () => {
    beforeEach(() => startLiveSessionB());

    it("keeps every lens and both exports on A, loads nothing into the global store, and offers no summary", async () => {
      mockBackend();
      render(<SessionsBrowser />);
      await openSessionAWhileLive();

      act(() => stopCaptureWithoutReroute());
      await act(async () => {});

      // Rule A: nothing wrote the global store; B's resident data is intact.
      expect(useAudioGraphStore.getState().loadedSessionId).toBeNull();
      expect(useAudioGraphStore.getState().transcriptSegments[0].text).toBe(
        B_TEXT,
      );
      // Only what needs the backend's current buffers stays unavailable.
      expect(moreActions()).toBeNull();

      await expectLensesShowAOnly();
      await expectBothExportsBoundToA();
      expect(moreActions()).toBeNull();
      expectNoBackendBufferCommand();
    });

    it("shows a closed localized error with retry when a lens query fails, never backend text, with exports still A's", async () => {
      const backend = mockBackend();
      render(<SessionsBrowser />);
      await openSessionAWhileLive();
      act(() => stopCaptureWithoutReroute());
      await act(async () => {});

      backend.failNotesOnce();
      fireEvent.click(screen.getByRole("tab", { name: /^notes$/i }));
      const pane = screen.getByRole("tabpanel");
      expect(await within(pane).findByRole("alert")).toHaveTextContent(
        "This session could not be loaded.",
      );
      expect(pane.textContent).not.toContain("disk gone");
      expect(pane.textContent).not.toContain("secret");
      expect(within(pane).queryByText(B_TEXT)).not.toBeInTheDocument();
      expect(moreActions()).toBeNull();

      // Retry recovers the lens; the transcript exports are still A's.
      fireEvent.click(within(pane).getByRole("button", { name: /try again/i }));
      await waitFor(() =>
        expect(
          screen.queryByTestId("review-lens-unavailable"),
        ).not.toBeInTheDocument(),
      );
      await expectBothExportsBoundToA();
      expectNoBackendBufferCommand();
    });

    it("after leaving Sessions before Stop and returning, still shows A on every lens and loads nothing globally", async () => {
      mockBackend();
      const first = render(<SessionsBrowser />);
      await openSessionAWhileLive();
      first.unmount();
      act(() => stopCaptureWithoutReroute());
      expect(useAudioGraphStore.getState().nav.sessionId).toBe("session-a");

      render(<SessionsBrowser />);
      await screen.findByRole("tabpanel");
      await act(async () => {});

      expect(moreActions()).toBeNull();
      await expectLensesShowAOnly();
      await expectBothExportsBoundToA();
      expectNoBackendBufferCommand();
    });
  });

  it("shows A's own data on Notes, Timeline and Graph for an auto-selected A after an unresolved Stop of a later B", async () => {
    // An earlier successful Stop routed `nav` to A (its row has reconciled, so
    // it is no longer the optimistic row) and the user never clicked A. B then
    // ran and its unresolved Stop left `nav` on A; the global store holds B.
    mockBackend();
    useAudioGraphStore.setState({
      sessions: [SESSION_A],
      sessionsLoading: false,
      isCapturing: false,
      isTranscribing: false,
      loadedSessionId: null,
      transcriptSegments: [segment("b-1", B_TEXT)],
      nav: { dest: "sessions", sessionId: "session-a", lens: "notes" },
    });
    render(<SessionsBrowser />);
    await screen.findByRole("tabpanel");

    await expectLensesShowAOnly();
    expect(moreActions()).toBeNull();
    // Only Review's own reads ran: the global loader never did.
    expect(loadSessionCalls()).toBeGreaterThan(0);
    expect(useAudioGraphStore.getState().loadedSessionId).toBeNull();
  });

  it("does not reload history into the Live store when Start is pressed on a hydrated A", async () => {
    mockBackend();
    const startAck = deferred<unknown>();
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_sessions") return [SESSION_A];
      if (cmd === "load_session") return payloadA();
      if (cmd === "start_capture") return startAck.promise;
      if (cmd === "purge_expired_sessions") return [];
      return null;
    });
    useAudioGraphStore.setState({
      sessions: [SESSION_A],
      sessionsLoading: false,
      selectedSourceIds: ["system-default"],
      isCapturing: false,
      isTranscribing: false,
    });
    render(
      <>
        <NowStrip />
        <SessionsBrowser />
      </>,
    );

    // Open A normally: row selection is the explicit action that hydrates.
    fireEvent.click(await screen.findByTestId("session-select-session-a"));
    await waitFor(() =>
      expect(useAudioGraphStore.getState().loadedSessionId).toBe("session-a"),
    );
    expect(useAudioGraphStore.getState().transcriptSegments[0].text).toBe(
      A_TEXT,
    );
    const callsBeforeStart = loadSessionCalls();

    // Press Start; capture is not acknowledged yet.
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await act(async () => {});
    await waitFor(() =>
      expect(mockedInvoke).toHaveBeenCalledWith(
        "start_capture",
        expect.anything(),
      ),
    );
    await act(async () => {});

    // startCapture cleared the store; nothing reloaded history into it.
    expect(loadSessionCalls()).toBe(callsBeforeStart);
    const mid = useAudioGraphStore.getState();
    expect(mid.loadedSessionId).toBeNull();
    expect(mid.transcriptSegments).toEqual([]);
    expect(mid.graphSnapshot.nodes).toEqual([]);

    // Acknowledged: Capture would mount with none of A's data resident.
    await act(async () => {
      startAck.resolve({ generation: 1, sources: ["system-default"] });
    });
    await waitFor(() =>
      expect(useAudioGraphStore.getState().isCapturing).toBe(true),
    );
    await act(async () => {});
    expect(loadSessionCalls()).toBe(callsBeforeStart);
    const after = useAudioGraphStore.getState();
    expect(after.loadedSessionId).toBeNull();
    expect(after.transcriptSegments).toEqual([]);
    expect(after.graphSnapshot.nodes).toEqual([]);
  });

  it("keeps a row click during an unacknowledged Start from loading history", async () => {
    mockBackend();
    const startAck = deferred<unknown>();
    mockedInvoke.mockImplementation(async (cmd: string) => {
      if (cmd === "list_sessions") return [SESSION_A];
      if (cmd === "load_session") return payloadA();
      if (cmd === "start_capture") return startAck.promise;
      if (cmd === "purge_expired_sessions") return [];
      return null;
    });
    useAudioGraphStore.setState({
      sessions: [SESSION_A],
      sessionsLoading: false,
      selectedSourceIds: ["system-default"],
      isCapturing: false,
      isTranscribing: false,
      loadedSessionId: null,
    });
    render(
      <>
        <NowStrip />
        <SessionsBrowser />
      </>,
    );
    const row = await screen.findByTestId("session-select-session-a");

    fireEvent.click(screen.getByRole("button", { name: "Start" }));
    await waitFor(() =>
      expect(useAudioGraphStore.getState().captureStarting).toBe(true),
    );
    // Start is pending: choosing a row may open A's own Review pane, but must
    // not load A into the global store the capture is about to open into.
    fireEvent.click(row);
    await act(async () => {});
    expect(useAudioGraphStore.getState().loadedSessionId).toBeNull();
    expect(useAudioGraphStore.getState().transcriptSegments).toEqual([]);

    await act(async () => {
      startAck.resolve({ generation: 1, sources: ["system-default"] });
    });
    await waitFor(() =>
      expect(useAudioGraphStore.getState().isCapturing).toBe(true),
    );
    await act(async () => {});
    const after = useAudioGraphStore.getState();
    expect(after.captureStarting).toBe(false);
    expect(after.loadedSessionId).toBeNull();
    expect(after.transcriptSegments).toEqual([]);
  });
});
