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
import LiveTranscript from "../components/LiveTranscript";
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import type { AsrPartialEvent, SpeakerInfo } from "../types";
import { downloadAsFile } from "../utils/download";
import { useReviewStore } from "./reviewStore";
import {
  deferred,
  diarizationSpan,
  loadedSession,
  segment,
} from "./reviewTestUtils";
import { SessionViewProvider } from "./SessionViewProvider";

vi.mock("../utils/download", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  downloadAsFile: vi.fn(),
}));

const mockedInvoke = vi.mocked(invoke);
const mockedDownload = vi.mocked(downloadAsFile);

const LIVE_SPEAKER: SpeakerInfo = {
  id: "spk-live",
  label: "Live Speaker",
  color: "#123456",
  total_speaking_time: 1,
  segment_count: 1,
} as SpeakerInfo;

const LIVE_PARTIAL: AsrPartialEvent = {
  provider: "deepgram",
  source_id: "system-default",
  text: "live partial words",
  start_time: 0,
  end_time: 1,
  confidence: 0.5,
  timestamp_ms: 0,
};

function seedLiveSession() {
  useAudioGraphStore.setState({
    isCapturing: true,
    isTranscribing: true,
    transcriptSegments: [segment("live-1", "live line one")],
    speakers: [],
    asrPartial: null,
    diarizationSpanRevisions: [],
    sessionTranscriptEvents: [],
    transcriptSeekTarget: null,
    exportTranscript: vi.fn(async () => '{"live":true}'),
    getSessionId: vi.fn(async () => "live-session-id"),
  });
}

describe("Review pane isolation from Live (issue #140)", () => {
  beforeEach(async () => {
    mockedInvoke.mockReset();
    mockedDownload.mockReset();
    useReviewStore.getState().close();
    await i18n.changeLanguage("en");
    seedLiveSession();
  });

  afterEach(async () => {
    await i18n.changeLanguage("en");
    useAudioGraphStore.setState({
      isCapturing: false,
      isTranscribing: false,
      transcriptSegments: [],
      speakers: [],
      asrPartial: null,
      diarizationSpanRevisions: [],
    });
  });

  it("renders the reviewed transcript with persisted speaker attribution, not Live content", async () => {
    mockedInvoke.mockResolvedValue(
      loadedSession("past-1", "historical line", "Alice"),
    );

    render(
      <SessionViewProvider reviewSessionId="past">
        <LiveTranscript />
      </SessionViewProvider>,
    );

    expect(await screen.findByText("historical line")).toBeInTheDocument();
    // The speaker label comes from the persisted diarization ledger (the
    // segment itself has no inline speaker fields).
    expect(screen.getByText("Alice")).toBeInTheDocument();
    expect(screen.queryByText("live line one")).not.toBeInTheDocument();
    expect(
      screen.getByRole("log", { name: /session transcript/i }),
    ).toBeInTheDocument();
    expect(mockedInvoke.mock.calls.map(([cmd]) => cmd)).toEqual([
      "load_session",
    ]);
  });

  it("applies Live mutations to Live only; the Review view is unchanged and Live resumes uninterrupted", async () => {
    mockedInvoke.mockResolvedValue(
      loadedSession("past-1", "historical line", "Alice"),
    );
    const { container, unmount } = render(
      <SessionViewProvider reviewSessionId="past">
        <LiveTranscript />
      </SessionViewProvider>,
    );
    await screen.findByText("historical line");
    const reviewHtml = container.innerHTML;
    const liveSegmentsBefore = useAudioGraphStore.getState().transcriptSegments;

    // Live content events land while Review is mounted.
    act(() => {
      useAudioGraphStore.setState({
        transcriptSegments: [
          ...liveSegmentsBefore,
          segment("live-2", "live line two", {
            speaker_id: "spk-live",
            speaker_label: "Live Speaker",
          }),
        ],
        speakers: [LIVE_SPEAKER],
        diarizationSpanRevisions: [
          diarizationSpan("live-2", "spk-live", "Live Speaker"),
        ],
        asrPartial: LIVE_PARTIAL,
      });
    });

    expect(container.innerHTML).toBe(reviewHtml);
    expect(screen.queryByText("live line two")).not.toBeInTheDocument();
    expect(screen.queryByText("live partial words")).not.toBeInTheDocument();
    expect(screen.queryByText("Live Speaker")).not.toBeInTheDocument();

    // Review never wrote a Live field.
    const live = useAudioGraphStore.getState();
    expect(live.transcriptSegments.map((s) => s.text)).toEqual([
      "live line one",
      "live line two",
    ]);
    expect(live.speakers).toEqual([LIVE_SPEAKER]);
    expect(live.isCapturing).toBe(true);
    expect(live.isTranscribing).toBe(true);

    // Back to Capture: the Live pane shows the uninterrupted live session.
    unmount();
    render(<LiveTranscript />);
    expect(screen.getByText("live line one")).toBeInTheDocument();
    expect(screen.getByText("live line two")).toBeInTheDocument();
    expect(screen.getByText("live partial words")).toBeInTheDocument();
    expect(screen.queryByText("historical line")).not.toBeInTheDocument();
  });

  it("does not leak the previous session while switching Review selection", async () => {
    const second = deferred<unknown>();
    mockedInvoke.mockImplementation((_cmd: string, args?: unknown) => {
      const { sessionId } = args as { sessionId: string };
      return sessionId === "past-a"
        ? Promise.resolve(loadedSession("a-1", "line from A", "Alice"))
        : second.promise;
    });

    const { rerender } = render(
      <SessionViewProvider reviewSessionId="past-a">
        <LiveTranscript />
      </SessionViewProvider>,
    );
    await screen.findByText("line from A");

    rerender(
      <SessionViewProvider reviewSessionId="past-b">
        <LiveTranscript />
      </SessionViewProvider>,
    );
    expect(screen.queryByText("line from A")).not.toBeInTheDocument();
    expect(screen.getByText(/loading session transcript/i)).toBeInTheDocument();

    await act(async () => {
      second.resolve(loadedSession("b-1", "line from B", "Bob"));
    });
    expect(await screen.findByText("line from B")).toBeInTheDocument();
  });

  it("shows a closed, localized error (en + pt) for a missing artifact and never the backend string", async () => {
    mockedInvoke.mockRejectedValue({
      code: "session_invalid",
      message: { reason: "Session files not found: C:\\data\\sessions\\gone" },
    });

    render(
      <SessionViewProvider reviewSessionId="gone">
        <LiveTranscript />
      </SessionViewProvider>,
    );

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(
      "This session's transcript is missing or could not be read.",
    );
    expect(document.body.textContent).not.toMatch(/Session files not found/);
    expect(document.body.textContent).not.toMatch(/C:\\data/);

    await act(async () => {
      await i18n.changeLanguage("pt");
    });
    expect(screen.getByRole("alert")).toHaveTextContent(
      "A transcrição desta sessão está ausente ou não pôde ser lida.",
    );
    expect(document.body.textContent).not.toMatch(/Session files not found/);
  });

  it("shows the same closed error for a corrupt payload and retries on request", async () => {
    mockedInvoke.mockResolvedValueOnce({ transcript: { not: "an array" } });

    render(
      <SessionViewProvider reviewSessionId="corrupt">
        <LiveTranscript />
      </SessionViewProvider>,
    );
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/missing or could not be read/i);

    mockedInvoke.mockResolvedValueOnce(
      loadedSession("c-1", "recovered line", "Alice"),
    );
    const retry = within(document.body).getByRole("button", {
      name: /try again/i,
    });
    fireEvent.click(retry);
    expect(await screen.findByText("recovered line")).toBeInTheDocument();
  });

  it("exports the reviewed session by its own id, from the snapshot, with no backend command", async () => {
    mockedInvoke.mockResolvedValue(
      loadedSession("past-1", "historical line", "Alice"),
    );
    render(
      <SessionViewProvider reviewSessionId="past-session-7">
        <LiveTranscript />
      </SessionViewProvider>,
    );
    await screen.findByText("historical line");
    mockedInvoke.mockClear();

    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: /export transcript as json/i }),
      );
    });
    await waitFor(() => expect(mockedDownload).toHaveBeenCalledTimes(1));
    const [json, filename] = mockedDownload.mock.calls[0];
    expect(filename).toMatch(/^transcript-past-session-7-/);
    expect(filename).not.toContain("live-session-id");
    expect(JSON.parse(json)).toMatchObject({
      session_id: "past-session-7",
      segments: [{ text: "historical line", speaker_label: "Alice" }],
    });
    expect(
      useAudioGraphStore.getState().exportTranscript,
    ).not.toHaveBeenCalled();
    expect(useAudioGraphStore.getState().getSessionId).not.toHaveBeenCalled();
    expect(mockedInvoke).not.toHaveBeenCalled();
  });
});
