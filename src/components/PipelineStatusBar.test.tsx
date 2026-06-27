import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "../store";
import type {
  PersistenceQueueBackpressurePayload,
  PipelineLatencyEvent,
  PipelineStatus,
  ProcessedAudioConsumerHealthPayload,
  StageStatus,
  TurnLifecycleEvent,
} from "../types";
import PipelineStatusBar from "./PipelineStatusBar";

const idle: StageStatus = { type: "Idle" };

function allIdle(): PipelineStatus {
  return {
    capture: idle,
    pipeline: idle,
    asr: idle,
    diarization: idle,
    entity_extraction: idle,
    graph: idle,
  };
}

function resetStore(
  overrides: {
    pipelineStatus?: PipelineStatus;
    pipelineLatencies?: Record<string, PipelineLatencyEvent>;
    turnEvents?: TurnLifecycleEvent[];
    latestAudioConsumerHealth?: ProcessedAudioConsumerHealthPayload | null;
    persistenceQueueBackpressure?: Record<
      string,
      PersistenceQueueBackpressurePayload
    >;
  } = {},
) {
  useAudioGraphStore.setState({
    pipelineStatus: overrides.pipelineStatus ?? allIdle(),
    pipelineLatencies: overrides.pipelineLatencies ?? {},
    turnEvents: overrides.turnEvents ?? [],
    latestAudioConsumerHealth: overrides.latestAudioConsumerHealth ?? null,
    persistenceQueueBackpressure: overrides.persistenceQueueBackpressure ?? {},
  });
}

describe("PipelineStatusBar", () => {
  beforeEach(() => {
    resetStore();
  });

  it("renders the status landmark with an accessible name", () => {
    render(<PipelineStatusBar />);
    const nav = screen.getByRole("status", { name: /pipeline status/i });
    expect(nav).toBeInTheDocument();
  });

  it("labels every pipeline stage in processing order", () => {
    render(<PipelineStatusBar />);
    for (const name of [
      "Capture",
      "Resample",
      "ASR",
      "Diarization",
      "Extraction",
      "Graph",
    ]) {
      expect(screen.getByText(name)).toBeInTheDocument();
    }
  });

  it("renders one status dot per stage, defaulting to the Idle tooltip", () => {
    render(<PipelineStatusBar />);
    // Each stage dot is a role=img with an aria-label embedding its tooltip.
    const captureDot = screen.getByRole("img", { name: /Capture: Idle/i });
    expect(captureDot).toBeInTheDocument();
    const graphDot = screen.getByRole("img", { name: /Graph: Idle/i });
    expect(graphDot).toBeInTheDocument();
  });

  it("surfaces the processed count in a Running stage's accessible label", () => {
    resetStore({
      pipelineStatus: {
        ...allIdle(),
        asr: { type: "Running", processed_count: 42 },
      },
    });
    render(<PipelineStatusBar />);
    expect(
      screen.getByRole("img", { name: /ASR: Running — 42 processed/i }),
    ).toBeInTheDocument();
  });

  it("surfaces the error message in an Error stage's accessible label", () => {
    resetStore({
      pipelineStatus: {
        ...allIdle(),
        graph: { type: "Error", message: "boom" },
      },
    });
    render(<PipelineStatusBar />);
    expect(
      screen.getByRole("img", { name: /Graph: Error: boom/i }),
    ).toBeInTheDocument();
  });

  it("appends a formatted latency to the tooltip and renders a latency badge", () => {
    resetStore({
      pipelineLatencies: {
        asr: {
          stage: "asr",
          latency_ms: 250,
          timestamp_ms: 0,
        },
      },
    });
    render(<PipelineStatusBar />);
    // 250ms < 1000 → visible "250ms" text (aria-hidden) plus a visually-hidden
    // sr-only sibling carrying the full "ASR ... latency 250ms" label (A11Y-1:
    // dropped role="img" from the text node). Assert on that sr-only label.
    expect(screen.getByText(/ASR last latency 250ms/i)).toBeInTheDocument();
    // The dot status indicator keeps role="img" (empty/color-only element) and
    // its tooltip is augmented with "• last latency 250ms".
    expect(
      screen.getByRole("img", { name: /ASR: Idle • last latency 250ms/i }),
    ).toBeInTheDocument();
  });

  it("formats sub-second-and-up latency in seconds with one decimal", () => {
    resetStore({
      pipelineLatencies: {
        capture: {
          stage: "capture",
          latency_ms: 1500,
          timestamp_ms: 0,
        },
      },
    });
    render(<PipelineStatusBar />);
    expect(screen.getByText(/Capture last latency 1\.5s/i)).toBeInTheDocument();
  });

  it("ignores a non-finite latency sample (no badge rendered)", () => {
    resetStore({
      pipelineLatencies: {
        asr: {
          stage: "asr",
          latency_ms: Number.NaN,
          timestamp_ms: 0,
        },
      },
    });
    render(<PipelineStatusBar />);
    expect(
      screen.queryByRole("img", { name: /ASR last latency/i }),
    ).not.toBeInTheDocument();
  });

  it("renders the arrow separators between stages as decorative (aria-hidden)", () => {
    const { container } = render(<PipelineStatusBar />);
    const hidden = container.querySelectorAll('[aria-hidden="true"]');
    // At minimum the 5 inter-stage arrows plus per-stage decorative icons.
    expect(hidden.length).toBeGreaterThanOrEqual(5);
  });

  it("shows a Turn chip with the latest turn event's provider + kind", () => {
    resetStore({
      turnEvents: [
        {
          provider: "gemini",
          source_id: "system-default",
          kind: "end_of_turn",
          timestamp_ms: 1,
        },
      ],
    });
    render(<PipelineStatusBar />);
    const nav = screen.getByRole("status", { name: /pipeline status/i });
    // "end_of_turn" → "end of turn"; provider prefix "gemini:".
    expect(within(nav).getByText(/gemini: end of turn/i)).toBeInTheDocument();
  });

  it("uses the most recent turn event when several have been recorded", () => {
    resetStore({
      turnEvents: [
        {
          provider: "deepgram",
          source_id: "system-default",
          kind: "speech_started",
          timestamp_ms: 1,
        },
        {
          provider: "gemini",
          source_id: "system-default",
          kind: "speech_final",
          timestamp_ms: 2,
        },
      ],
    });
    render(<PipelineStatusBar />);
    const nav = screen.getByRole("status", { name: /pipeline status/i });
    expect(within(nav).getByText(/gemini: speech final/i)).toBeInTheDocument();
    expect(
      within(nav).queryByText(/deepgram: speech started/i),
    ).not.toBeInTheDocument();
  });

  it("omits the Turn chip when no turn events have arrived", () => {
    render(<PipelineStatusBar />);
    expect(screen.queryByText(/^Turn$/)).not.toBeInTheDocument();
  });

  it("shows compact audio-consumer queue health", () => {
    resetStore({
      latestAudioConsumerHealth: {
        consumers: [
          {
            id: "speech",
            stage: "speech",
            provider: null,
            active: true,
            queue_len: 2,
            queue_capacity: 1024,
            sent_chunks: 12,
            dropped_chunks: 0,
            drop_policy: "drop_oldest",
            source_filter: { type: "all" },
            mixing_mode: "per_source",
          },
          {
            id: "gemini-notes",
            stage: "notes",
            provider: "gemini",
            active: false,
            queue_len: 1,
            queue_capacity: 16,
            sent_chunks: 3,
            dropped_chunks: 0,
            drop_policy: "drop_oldest",
            source_filter: { type: "all" },
            mixing_mode: "per_source",
          },
        ],
      },
    });
    render(<PipelineStatusBar />);

    expect(screen.getByText("Audio")).toBeInTheDocument();
    expect(screen.getByText("1/2 on · q 3/1040 · d 0")).toBeInTheDocument();
    expect(
      screen.getByRole("img", {
        name: /Audio consumers: 1\/2 active, queue 3\/1040, 0 dropped/i,
      }),
    ).toBeInTheDocument();
  });

  it("surfaces dropped audio-consumer chunks in the status label", () => {
    resetStore({
      latestAudioConsumerHealth: {
        consumers: [
          {
            id: "speech",
            stage: "speech",
            provider: null,
            active: true,
            queue_len: 0,
            queue_capacity: 1024,
            sent_chunks: 64,
            dropped_chunks: 5,
            drop_policy: "drop_oldest",
            source_filter: { type: "all" },
            mixing_mode: "per_source",
          },
        ],
      },
    });
    render(<PipelineStatusBar />);

    expect(screen.getByText("1/1 on · q 0/1024 · d 5")).toBeInTheDocument();
    expect(
      screen.getByRole("img", {
        name: /Audio consumers: 1\/1 active, queue 0\/1024, 5 dropped/i,
      }),
    ).toBeInTheDocument();
  });

  it("surfaces persistence queue pressure distinctly from storage-full", () => {
    resetStore({
      persistenceQueueBackpressure: {
        transcript_event: {
          writer: "transcript_event",
          is_backpressured: true,
          queue_capacity: 2048,
          dropped_count: 2,
        },
        projection_event: {
          writer: "projection_event",
          is_backpressured: true,
          queue_capacity: 2048,
          dropped_count: 5,
        },
      },
    });
    render(<PipelineStatusBar />);

    expect(screen.getByText("Persist")).toBeInTheDocument();
    expect(screen.getByText("drop 7")).toBeInTheDocument();
    expect(
      screen.getByRole("img", {
        name: /Persistence queue pressure: transcript, notes\/graph writer queue is full, capacity 4096, 7 events dropped/i,
      }),
    ).toBeInTheDocument();
    expect(screen.queryByText(/storage full/i)).not.toBeInTheDocument();
  });
});
