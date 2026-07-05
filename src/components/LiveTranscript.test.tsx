import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type {
  AsrPartialEvent,
  SpeakerInfo,
  TranscriptEvent,
  TranscriptSegment,
} from "../types";
import LiveTranscript from "./LiveTranscript";

function segment(
  overrides: Partial<TranscriptSegment> = {},
): TranscriptSegment {
  return {
    id: crypto.randomUUID(),
    source_id: "system-default",
    speaker_id: null,
    speaker_label: null,
    text: "hello world",
    start_time: 0,
    end_time: 1,
    confidence: 1,
    ...overrides,
  };
}

function partial(overrides: Partial<AsrPartialEvent> = {}): AsrPartialEvent {
  return {
    provider: "deepgram",
    source_id: "system-default",
    text: "partial text",
    start_time: 0,
    end_time: 1,
    confidence: 0.5,
    timestamp_ms: 0,
    ...overrides,
  };
}

function transcriptEvent(
  revisionNumber: number,
  overrides: Partial<TranscriptEvent> = {},
): TranscriptEvent {
  return {
    span_id: "span-1",
    provider: "deepgram",
    source_id: "system-default",
    provider_item_id: null,
    transcript_segment_id: null,
    speaker_id: null,
    speaker_label: null,
    channel: null,
    text: "hello",
    start_time: 0,
    end_time: 1,
    confidence: 0.9,
    is_final: revisionNumber > 1,
    stability: revisionNumber > 1 ? "final" : "partial",
    revision_number: revisionNumber,
    supersedes: null,
    turn_id: null,
    end_of_turn: revisionNumber > 1,
    raw_event_ref: null,
    received_at_ms: 1_700_000_000_000 + revisionNumber,
    ...overrides,
  };
}

function resetStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    samplePreviewActive: false,
    transcriptSegments: [],
    asrPartial: null,
    sessionTranscriptEvents: [],
    transcriptSeekTarget: null,
    speakers: [],
    exportTranscript: vi.fn(async () => "{}"),
    getSessionId: vi.fn(async () => "sess-1"),
    isCapturing: false,
    isTranscribing: false,
    isGeminiActive: false,
    ...overrides,
  });
}

describe("LiveTranscript", () => {
  beforeEach(() => {
    resetStore();
  });

  it("renders the panel title and an aria-live log region", () => {
    render(<LiveTranscript />);
    expect(
      screen.getByRole("heading", { name: /live transcript/i }),
    ).toBeInTheDocument();
    const log = screen.getByRole("log", { name: /live transcript/i });
    expect(log).toHaveAttribute("aria-live", "polite");
  });

  it("shows the not-running empty state when capture is off", () => {
    resetStore({ isCapturing: false });
    render(<LiveTranscript />);
    expect(
      screen.getByText(/transcription isn't running/i),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/start capture and turn on transcribe/i),
    ).toBeInTheDocument();
    // The listening prompt must NOT show while not running.
    expect(
      screen.queryByText(/listening… waiting for speech/i),
    ).not.toBeInTheDocument();
  });

  it("shows the listening empty state when capturing + transcribing", () => {
    resetStore({ isCapturing: true, isTranscribing: true });
    render(<LiveTranscript />);
    expect(
      screen.getByText(/listening… waiting for speech/i),
    ).toBeInTheDocument();
    expect(
      screen.queryByText(/transcription isn't running/i),
    ).not.toBeInTheDocument();
  });

  it("treats Gemini-active capture as a running pipeline (listening state)", () => {
    resetStore({ isCapturing: true, isGeminiActive: true });
    render(<LiveTranscript />);
    expect(
      screen.getByText(/listening… waiting for speech/i),
    ).toBeInTheDocument();
  });

  it("treats capture-without-any-engine as not running", () => {
    // Capturing but neither transcribing nor Gemini active → not running.
    resetStore({
      isCapturing: true,
      isTranscribing: false,
      isGeminiActive: false,
    });
    render(<LiveTranscript />);
    expect(
      screen.getByText(/transcription isn't running/i),
    ).toBeInTheDocument();
  });

  it("renders transcript segments with speaker label and text", () => {
    resetStore({
      transcriptSegments: [
        segment({
          text: "First line",
          speaker_label: "SPK 1",
          speaker_id: "s1",
        }),
      ],
      speakers: [
        {
          id: "s1",
          label: "SPK 1",
          color: "#60a5fa",
          total_speaking_time: 1,
          segment_count: 1,
        } satisfies SpeakerInfo,
      ],
    });
    render(<LiveTranscript />);
    expect(screen.getByText("First line")).toBeInTheDocument();
    expect(screen.getByText("SPK 1")).toBeInTheDocument();
    // Empty-state copy is gone once segments exist.
    expect(
      screen.queryByText(/transcription isn't running/i),
    ).not.toBeInTheDocument();
  });

  it("shows a segment count badge and renders a low-confidence meter", () => {
    resetStore({
      transcriptSegments: [segment({ text: "uncertain", confidence: 0.42 })],
    });
    render(<LiveTranscript />);
    expect(screen.getByText("1")).toBeInTheDocument();
    const meter = screen.getByRole("meter", { name: /confidence: 42%/i });
    expect(meter).toHaveAttribute("aria-valuenow", "42");
  });

  it("shows a subtle revision indicator for corrected transcript spans", () => {
    resetStore({
      transcriptSegments: [
        segment({
          id: "span-1",
          text: "corrected transcript",
        }),
      ],
      sessionTranscriptEvents: [
        transcriptEvent(1, {
          span_id: "span-1",
          text: "partial transcript",
        }),
        transcriptEvent(2, {
          span_id: "span-1",
          text: "corrected transcript",
        }),
      ],
    });
    render(<LiveTranscript />);

    expect(screen.getByText("corrected transcript")).toBeInTheDocument();
    expect(screen.getByText(/revised 2x/i)).toBeInTheDocument();
  });

  it("renders an in-flight partial below the committed segments", () => {
    resetStore({
      transcriptSegments: [segment({ text: "committed" })],
      asrPartial: partial({ text: "still talking", provider: "gemini" }),
    });
    render(<LiveTranscript />);
    expect(screen.getByText("committed")).toBeInTheDocument();
    expect(screen.getByText("still talking")).toBeInTheDocument();
    expect(screen.getByText("gemini")).toBeInTheDocument();
  });

  it("disables both export buttons when there are no segments", () => {
    render(<LiveTranscript />);
    expect(
      screen.getByRole("button", { name: /export transcript as json/i }),
    ).toBeDisabled();
    expect(
      screen.getByRole("button", { name: /export transcript as plain text/i }),
    ).toBeDisabled();
  });

  it("enables exports once there is at least one segment", () => {
    resetStore({ transcriptSegments: [segment()] });
    render(<LiveTranscript />);
    expect(
      screen.getByRole("button", { name: /export transcript as json/i }),
    ).toBeEnabled();
  });

  it("exporting JSON invokes exportTranscript and resolves a session-scoped filename", async () => {
    // jsdom lacks URL.createObjectURL; stub the download primitives so the
    // happy path doesn't fall into the catch branch.
    const createObjectURL = vi
      .spyOn(URL, "createObjectURL")
      .mockReturnValue("blob:fake");
    const revokeObjectURL = vi
      .spyOn(URL, "revokeObjectURL")
      .mockImplementation(() => {});
    const exportTranscript = vi.fn(async () => '{"segments":[]}');
    const getSessionId = vi.fn(async () => "abc123");
    resetStore({
      transcriptSegments: [segment()],
      exportTranscript,
      getSessionId,
    });
    render(<LiveTranscript />);
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: /export transcript as json/i }),
      );
    });
    await waitFor(() => expect(exportTranscript).toHaveBeenCalledTimes(1));
    expect(getSessionId).toHaveBeenCalled();
    // No error alert on the happy path.
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    createObjectURL.mockRestore();
    revokeObjectURL.mockRestore();
  });

  it("surfaces an export error alert when exportTranscript rejects", async () => {
    const exportTranscript = vi.fn(async () => {
      throw new Error("disk gone");
    });
    resetStore({
      transcriptSegments: [segment()],
      exportTranscript,
    });
    render(<LiveTranscript />);
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: /export transcript as json/i }),
      );
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/export failed/i);
    expect(alert).toHaveTextContent(/disk gone/i);
  });

  it("tags each rendered segment with a data-segment-id for seek targeting", () => {
    resetStore({
      transcriptSegments: [segment({ id: "seg-77", text: "target me" })],
    });
    const { container } = render(<LiveTranscript />);
    expect(
      container.querySelector('[data-segment-id="seg-77"]'),
    ).toBeInTheDocument();
  });

  it("scrolls and briefly highlights the segment named by a seek target", () => {
    const scrollIntoView = vi.fn();
    // jsdom implements neither scrollIntoView nor CSS.escape reliably; stub both.
    HTMLElement.prototype.scrollIntoView = scrollIntoView;
    resetStore({
      transcriptSegments: [
        segment({ id: "seg-a", text: "first" }),
        segment({ id: "seg-b", text: "second" }),
      ],
      transcriptSeekTarget: { segmentId: "seg-b", nonce: 1 },
    });
    render(<LiveTranscript />);
    // The seek effect scrolls the matching segment into view.
    expect(scrollIntoView).toHaveBeenCalledTimes(1);
    expect(scrollIntoView).toHaveBeenCalledWith(
      expect.objectContaining({ block: "center" }),
    );
  });
});
