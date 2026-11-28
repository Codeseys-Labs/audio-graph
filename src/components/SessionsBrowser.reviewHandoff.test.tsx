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
import SessionsBrowser from "./SessionsBrowser";

vi.mock("../utils/download", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  downloadAsFile: vi.fn(),
}));

const mockedInvoke = vi.mocked(invoke);
const mockedDownload = vi.mocked(downloadAsFile);

/**
 * Stop handoff for a session opened in Review while capture ran (PR #536
 * review). The live session B's data is resident in the global store; the user
 * is reviewing completed session A. When capture stops without re-routing
 * `nav` (Stop could not resolve B's id), nothing about A's pane or exports may
 * ever be sourced from B's global data.
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

/**
 * `load_session` answers. Call 1 is the Review query made while live; call 2 is
 * the global-store reload that follows the run ending (`later`); any other call
 * (a Review query after remounting) resolves with A's payload.
 */
function mockLoadSession(later: () => Promise<unknown>): void {
  let loadCalls = 0;
  mockedInvoke.mockImplementation(async (cmd: string) => {
    if (cmd === "list_sessions") return [SESSION_A];
    if (cmd === "load_session") {
      loadCalls += 1;
      return loadCalls === 2 ? later() : payloadA();
    }
    if (cmd === "purge_expired_sessions") return [];
    if (cmd === "build_session_timeline_cmd")
      return { entries: [], total_count: 0 };
    return null;
  });
}

async function exportJson(): Promise<Record<string, unknown>> {
  mockedDownload.mockClear();
  await act(async () => {
    fireEvent.click(
      screen.getByRole("button", { name: /export transcript as json/i }),
    );
  });
  await waitFor(() => expect(mockedDownload).toHaveBeenCalledTimes(1));
  return JSON.parse(mockedDownload.mock.calls[0][0] as string);
}

async function exportTxt(): Promise<{ text: string; filename: string }> {
  mockedDownload.mockClear();
  await act(async () => {
    fireEvent.click(
      screen.getByRole("button", { name: /export transcript as plain text/i }),
    );
  });
  await waitFor(() => expect(mockedDownload).toHaveBeenCalledTimes(1));
  return {
    text: mockedDownload.mock.calls[0][0] as string,
    filename: mockedDownload.mock.calls[0][1] as string,
  };
}

async function openSessionAWhileLive(): Promise<void> {
  fireEvent.click(await screen.findByTestId("session-select-session-a"));
  expect(await screen.findByText(A_TEXT)).toBeVisible();
}

describe("SessionsBrowser: Review across the Stop handoff", () => {
  beforeEach(async () => {
    mockedInvoke.mockReset();
    mockedDownload.mockReset();
    localStorage.clear();
    useReviewStore.getState().close();
    await i18n.changeLanguage("en");
    startLiveSessionB();
  });

  afterEach(() => {
    useAudioGraphStore.setState({
      sessions: [],
      isCapturing: false,
      isTranscribing: false,
      selectedSourceIds: [],
      captureStartTime: null,
      transcriptSegments: [],
      loadedSessionId: null,
    });
  });

  it("keeps A's transcript and both exports bound to A while the post-stop reload is still pending", async () => {
    const reload = deferred<unknown>();
    mockLoadSession(() => reload.promise);
    render(<SessionsBrowser />);
    await openSessionAWhileLive();

    act(() => stopCaptureWithoutReroute());

    // Reload of A into the global store has started but not settled; B's data
    // is still what the global store holds.
    await waitFor(() =>
      expect(
        mockedInvoke.mock.calls.filter(([cmd]) => cmd === "load_session"),
      ).toHaveLength(2),
    );
    expect(useAudioGraphStore.getState().transcriptSegments[0].text).toBe(
      B_TEXT,
    );

    const pane = screen.getByRole("tabpanel");
    expect(within(pane).getByText(A_TEXT)).toBeVisible();
    expect(within(pane).queryByText(B_TEXT)).not.toBeInTheDocument();

    const txt = await exportTxt();
    expect(txt.text).toContain(A_TEXT);
    expect(txt.text).not.toContain(B_TEXT);
    expect(txt.filename).toMatch(/^transcript-session-a-/);

    mockedInvoke.mockClear();
    const json = await exportJson();
    expect(json.session_id).toBe("session-a");
    expect(JSON.stringify(json)).not.toContain(B_TEXT);
    expect(
      useAudioGraphStore.getState().exportTranscript,
    ).not.toHaveBeenCalled();
    expect(mockedInvoke).not.toHaveBeenCalledWith("export_transcript");

    // The reload settles: the pane is still A.
    await act(async () => {
      reload.resolve(payloadA());
    });
    await waitFor(() =>
      expect(useAudioGraphStore.getState().loadedSessionId).toBe("session-a"),
    );
    expect(
      within(screen.getByRole("tabpanel")).getByText(A_TEXT),
    ).toBeVisible();
  });

  it("never shows B under A's title when the reload fails, and says so on the lenses that need it", async () => {
    mockLoadSession(() => Promise.reject({ code: "io", message: "disk gone" }));
    render(<SessionsBrowser />);
    await openSessionAWhileLive();

    act(() => stopCaptureWithoutReroute());
    await waitFor(() =>
      expect(
        mockedInvoke.mock.calls.filter(([cmd]) => cmd === "load_session"),
      ).toHaveLength(2),
    );

    // Transcript: still A, and A's exports are A's.
    await waitFor(() =>
      expect(useAudioGraphStore.getState().error).not.toBeNull(),
    );
    expect(
      within(screen.getByRole("tabpanel")).getByText(A_TEXT),
    ).toBeVisible();
    expect((await exportTxt()).text).not.toContain(B_TEXT);

    // Notes reads the global store, which still holds B: it must show a closed
    // error with a retry instead.
    fireEvent.click(screen.getByRole("tab", { name: /^notes$/i }));
    const pane = screen.getByRole("tabpanel");
    expect(await within(pane).findByRole("alert")).toHaveTextContent(
      "This session could not be loaded.",
    );
    expect(within(pane).queryByText(B_TEXT)).not.toBeInTheDocument();
    expect(pane.textContent).not.toContain("disk gone");
    expect(
      within(pane).getByRole("button", { name: /try again/i }),
    ).toBeVisible();
  });

  it("still shows A, and still loads A, after leaving Sessions before Stop and returning afterwards", async () => {
    const reload = deferred<unknown>();
    mockLoadSession(() => reload.promise);
    const first = render(<SessionsBrowser />);
    await openSessionAWhileLive();

    // Leave Sessions (component unmounts), then Stop while away.
    first.unmount();
    act(() => stopCaptureWithoutReroute());
    expect(useAudioGraphStore.getState().nav.sessionId).toBe("session-a");

    render(<SessionsBrowser />);
    // Back on Sessions, not live any more: the browser opens on its default
    // (Notes) lens, which must not show B; the Transcript lens shows A.
    const landing = await screen.findByRole("tabpanel");
    expect(within(landing).queryByText(B_TEXT)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: /^transcript$/i }));
    const pane = screen.getByRole("tabpanel");
    expect(await within(pane).findByText(A_TEXT)).toBeVisible();
    expect(within(pane).queryByText(B_TEXT)).not.toBeInTheDocument();

    // The pending hydration of A into the global store still happens, and the
    // lens that reads the global store never shows B meanwhile.
    await waitFor(() =>
      expect(
        mockedInvoke.mock.calls.filter(([cmd]) => cmd === "load_session")
          .length,
      ).toBeGreaterThanOrEqual(2),
    );
    fireEvent.click(screen.getByRole("tab", { name: /^notes$/i }));
    const notes = screen.getByRole("tabpanel");
    expect(within(notes).getByRole("status")).toHaveTextContent(/loading/i);
    expect(within(notes).queryByText(B_TEXT)).not.toBeInTheDocument();

    await act(async () => {
      reload.resolve(payloadA());
    });
    await waitFor(() =>
      expect(useAudioGraphStore.getState().loadedSessionId).toBe("session-a"),
    );
  });
});
