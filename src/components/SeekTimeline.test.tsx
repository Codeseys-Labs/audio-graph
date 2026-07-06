import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { TimelineEntry, TranscriptEvent } from "../types";
import SeekTimeline from "./SeekTimeline";
import "../i18n";

function entry(overrides: Partial<TimelineEntry> = {}): TimelineEntry {
  return {
    span_id: "span-1",
    start_ms: 0,
    end_ms: 1000,
    received_at_ms: 1_700_000_000_000,
    turn_id: "turn-1",
    speaker_id: "spk-1",
    speaker_label: "Alice",
    text: "hello there",
    related_edge_ids: [],
    ...overrides,
  };
}

function transcriptEvent(
  overrides: Partial<TranscriptEvent> = {},
): TranscriptEvent {
  return {
    span_id: "span-1",
    provider: "test",
    source_id: "system-default",
    provider_item_id: null,
    transcript_segment_id: null,
    speaker_id: "spk-1",
    speaker_label: "Alice",
    channel: null,
    text: "hello there",
    start_time: 0,
    end_time: 1,
    confidence: 0.9,
    is_final: true,
    stability: "final",
    revision_number: 2,
    supersedes: null,
    turn_id: "turn-1",
    end_of_turn: true,
    raw_event_ref: null,
    received_at_ms: 1_700_000_000_000,
    ...overrides,
  };
}

function resetStore(
  overrides: Partial<ReturnType<typeof useAudioGraphStore.getState>> = {},
) {
  useAudioGraphStore.setState({
    sessionTimeline: null,
    sessionTimelineLoading: false,
    transcriptSeekTarget: null,
    sessionTranscriptEvents: [],
    speakers: [],
    seekTranscriptToSegment: vi.fn(),
    ...overrides,
  });
}

describe("SeekTimeline", () => {
  beforeEach(() => {
    resetStore();
  });

  it("renders the empty state when no timeline is loaded", () => {
    render(<SeekTimeline />);
    expect(screen.getByText(/no timeline yet/i)).toBeInTheDocument();
    expect(screen.queryByTestId("seek-timeline-block")).not.toBeInTheDocument();
  });

  it("renders the empty state for a session with zero utterances", () => {
    resetStore({ sessionTimeline: [] });
    render(<SeekTimeline />);
    expect(screen.getByText(/no timeline yet/i)).toBeInTheDocument();
  });

  it("shows a loading state while the fold is in flight", () => {
    resetStore({ sessionTimeline: null, sessionTimelineLoading: true });
    render(<SeekTimeline />);
    expect(
      screen.getByText(/building the session timeline/i),
    ).toBeInTheDocument();
  });

  it("renders one lane per resolved speaker", () => {
    resetStore({
      sessionTimeline: [
        entry({ span_id: "a", speaker_id: "spk-1", speaker_label: "Alice" }),
        entry({
          span_id: "b",
          speaker_id: "spk-2",
          speaker_label: "Bob",
          start_ms: 2000,
          end_ms: 3000,
        }),
        entry({
          span_id: "c",
          speaker_id: "spk-1",
          speaker_label: "Alice",
          start_ms: 4000,
          end_ms: 5000,
        }),
      ],
    });
    render(<SeekTimeline />);
    // Two distinct speakers → two lanes, three blocks total.
    expect(screen.getAllByTestId("seek-timeline-lane")).toHaveLength(2);
    expect(screen.getAllByTestId("seek-timeline-block")).toHaveLength(3);
  });

  it("positions blocks by media time (left/width % from start/end)", () => {
    resetStore({
      sessionTimeline: [
        entry({ span_id: "a", start_ms: 0, end_ms: 1000 }),
        entry({ span_id: "b", start_ms: 5000, end_ms: 10000 }),
      ],
    });
    render(<SeekTimeline />);
    const blocks = screen.getAllByTestId("seek-timeline-block");
    const first = blocks.find((b) => b.dataset.spanId === "a");
    const second = blocks.find((b) => b.dataset.spanId === "b");
    // Domain is [0, 10000]. First block starts at 0%, second at 50%.
    expect(first?.style.left).toBe("0%");
    expect(second?.style.left).toBe("50%");
    // Second block spans 5000/10000 = 50% width.
    expect(second?.style.width).toBe("50%");
  });

  it("click → dispatches seek with the bridged transcript segment id", () => {
    const seek = vi.fn();
    resetStore({
      sessionTimeline: [entry({ span_id: "span-1" })],
      // The transcript event bridges span-1 → its transcript_segment_id.
      sessionTranscriptEvents: [
        transcriptEvent({
          span_id: "span-1",
          transcript_segment_id: "seg-42",
        }),
      ],
      seekTranscriptToSegment: seek,
    });
    render(<SeekTimeline />);
    fireEvent.click(screen.getByTestId("seek-timeline-block"));
    expect(seek).toHaveBeenCalledWith("seg-42");
  });

  it("falls back to span_id when no transcript_segment_id bridge exists", () => {
    const seek = vi.fn();
    resetStore({
      sessionTimeline: [entry({ span_id: "span-9" })],
      sessionTranscriptEvents: [],
      seekTranscriptToSegment: seek,
    });
    render(<SeekTimeline />);
    fireEvent.click(screen.getByTestId("seek-timeline-block"));
    expect(seek).toHaveBeenCalledWith("span-9");
  });

  it("keyboard activation (Enter) triggers the same seek as a click", () => {
    const seek = vi.fn();
    resetStore({
      sessionTimeline: [entry({ span_id: "span-1" })],
      sessionTranscriptEvents: [
        transcriptEvent({ span_id: "span-1", transcript_segment_id: "seg-1" }),
      ],
      seekTranscriptToSegment: seek,
    });
    render(<SeekTimeline />);
    const block = screen.getByTestId("seek-timeline-block");
    // A native <button> fires click on Enter/Space; simulate the resulting
    // click to assert the activation path is wired.
    block.focus();
    expect(block).toHaveFocus();
    fireEvent.click(block);
    expect(seek).toHaveBeenCalledWith("seg-1");
  });

  it("surfaces related graph edges as a count badge", () => {
    resetStore({
      sessionTimeline: [
        entry({ span_id: "a", related_edge_ids: ["e1", "e2"] }),
        entry({ span_id: "b", related_edge_ids: [], start_ms: 2000 }),
      ],
    });
    render(<SeekTimeline />);
    const badges = screen.getAllByTestId("seek-timeline-edge-badge");
    // Only the utterance with edges shows a badge, reading "→2".
    expect(badges).toHaveLength(1);
    expect(badges[0]).toHaveTextContent("→2");
  });

  it("gives every block an accessible name for keyboard/SR users", () => {
    resetStore({
      sessionTimeline: [entry({ speaker_label: "Alice", text: "we ship" })],
    });
    render(<SeekTimeline />);
    const block = screen.getByTestId("seek-timeline-block");
    expect(block).toHaveAttribute(
      "aria-label",
      expect.stringContaining("Alice"),
    );
    expect(block.getAttribute("aria-label")).toContain("we ship");
  });

  it("caps to the LAST 200 blocks — the same window LiveTranscript mounts", () => {
    const many: TimelineEntry[] = Array.from({ length: 205 }, (_, i) =>
      entry({ span_id: `s-${i}`, start_ms: i * 10, end_ms: i * 10 + 5 }),
    );
    resetStore({ sessionTimeline: many });
    render(<SeekTimeline />);
    const blocks = screen.getAllByTestId("seek-timeline-block");
    expect(blocks).toHaveLength(200);
    // LiveTranscript renders segments.slice(-200); the strip must show the
    // same tail window so every rendered block has a mounted seek target.
    // Entries 0-4 (dropped) must not render; entries 5 and 204 must.
    const spanIds = new Set(blocks.map((b) => b.dataset.spanId));
    expect(spanIds.has("s-0")).toBe(false);
    expect(spanIds.has("s-4")).toBe(false);
    expect(spanIds.has("s-5")).toBe(true);
    expect(spanIds.has("s-204")).toBe(true);
    // The cap note states WHICH window is shown (the last N).
    expect(
      screen.getByText(/showing the last 200 of 205/i),
    ).toBeInTheDocument();
  });

  it("a click on a rendered block in a >200-entry timeline still seeks", () => {
    const seek = vi.fn();
    const many: TimelineEntry[] = Array.from({ length: 205 }, (_, i) =>
      entry({ span_id: `s-${i}`, start_ms: i * 10, end_ms: i * 10 + 5 }),
    );
    resetStore({
      sessionTimeline: many,
      sessionTranscriptEvents: [
        transcriptEvent({ span_id: "s-204", transcript_segment_id: "seg-204" }),
      ],
      seekTranscriptToSegment: seek,
    });
    render(<SeekTimeline />);
    const last = screen
      .getAllByTestId("seek-timeline-block")
      .find((b) => b.dataset.spanId === "s-204");
    expect(last).toBeDefined();
    if (last) fireEvent.click(last);
    expect(seek).toHaveBeenCalledWith("seg-204");
  });

  it("labels the lane region for assistive tech", () => {
    resetStore({ sessionTimeline: [entry()] });
    render(<SeekTimeline />);
    const region = screen.getByRole("group", { name: /speaker lanes/i });
    expect(within(region).getAllByTestId("seek-timeline-block").length).toBe(1);
  });
});
