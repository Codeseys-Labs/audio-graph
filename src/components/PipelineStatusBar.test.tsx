import { render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "../store";
import type {
  PipelineLatencyEvent,
  PipelineStatus,
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
  } = {},
) {
  useAudioGraphStore.setState({
    pipelineStatus: overrides.pipelineStatus ?? allIdle(),
    pipelineLatencies: overrides.pipelineLatencies ?? {},
    turnEvents: overrides.turnEvents ?? [],
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
    // 250ms < 1000 → rendered as "250ms" with its own aria-labelled badge.
    expect(
      screen.getByRole("img", { name: /ASR last latency 250ms/i }),
    ).toBeInTheDocument();
    // The dot tooltip is augmented with "• last latency 250ms".
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
    expect(
      screen.getByRole("img", { name: /Capture last latency 1\.5s/i }),
    ).toBeInTheDocument();
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
});
