import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { seededRandom } from "../test/captureSimulation";
import type { AudioGraphStore, PipelineStatus } from "../types";
import { useAudioGraphStore } from ".";
import { compareCaptureIds, isPipelineStatus } from "./captureFacts";

/**
 * Conformance traces (#141): every snapshot a Rust conformance test saw the
 * backend hand out, in the order it produced them, replayed through the
 * production ingestion path in several orders. After the deliveries the
 * window shows the highest-revision snapshot delivered; after the final
 * status read it shows that read's state.
 */

interface CaptureTrace {
  name: string;
  deliveries: PipelineStatus[];
  final: PipelineStatus;
}

const traces = Object.entries(
  import.meta.glob<CaptureTrace>("../test/fixtures/capture-traces/*.json", {
    eager: true,
    import: "default",
  }),
);

/** What the window must show for a snapshot, derived independently. */
function expectedDisplay(status: PipelineStatus) {
  const { id, phase } = status.capture_state;
  const modes = (["gemini", "converse", "openai_realtime"] as const).filter(
    (mode) => status.realtime[mode],
  );
  return {
    phase,
    id,
    isCapturing: phase === "running",
    isTranscribing: status.transcription !== null,
    backpressure: status.backpressure
      .filter((entry) => compareCaptureIds(entry.capture, id) === 0)
      .map((entry) => `${id.epoch}.${id.seq}/${entry.source_index}`)
      .sort(),
    realtime: modes.length > 0,
    realtimeCommand:
      modes.length === 1
        ? {
            gemini: "start_gemini",
            converse: "start_converse",
            openai_realtime: "start_openai_realtime",
          }[modes[0]]
        : null,
    session: status.session,
  };
}

function shownDisplay(state: AudioGraphStore) {
  return {
    phase: state.captureStatus,
    id: state.pipelineStatus.capture_state.id,
    isCapturing: state.isCapturing,
    isTranscribing: state.isTranscribing,
    backpressure: [...state.backpressuredSources].sort(),
    realtime: state.isGeminiActive,
    realtimeCommand: state.activeGeminiCommand,
    session: state.pipelineStatus.session,
  };
}

/** The orders a trace is replayed in, each named. */
function replayOrders(
  deliveries: PipelineStatus[],
): Array<[string, PipelineStatus[]]> {
  const orders: Array<[string, PipelineStatus[]]> = [
    ["recorded", deliveries],
    ["reversed", [...deliveries].reverse()],
  ];
  for (let seed = 1; seed <= 5; seed += 1) {
    const random = seededRandom(seed);
    const shuffled = [...deliveries];
    for (let i = shuffled.length - 1; i > 0; i -= 1) {
      const j = Math.floor(random() * (i + 1));
      [shuffled[i], shuffled[j]] = [shuffled[j], shuffled[i]];
    }
    orders.push([`shuffled (seed ${seed})`, shuffled]);
  }
  deliveries.forEach((_, dropped) => {
    orders.push([
      `recorded without delivery ${dropped}`,
      deliveries.filter((__, index) => index !== dropped),
    ]);
  });
  return orders;
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("capture conformance traces", () => {
  it("has at least one trace to replay", () => {
    expect(traces.length).toBeGreaterThan(0);
  });

  for (const [file, trace] of traces) {
    describe(trace.name, () => {
      it(`is a well-formed trace (${file})`, () => {
        for (const status of [...trace.deliveries, trace.final]) {
          expect(isPipelineStatus(status)).toBe(true);
        }
        // Non-decreasing revisions; a repeated revision is the same state.
        const seen = new Map<number, string>();
        trace.deliveries.forEach((status, index) => {
          if (index > 0) {
            expect(status.revision).toBeGreaterThanOrEqual(
              trace.deliveries[index - 1].revision,
            );
          }
          const state = JSON.stringify(expectedDisplay(status));
          expect(seen.get(status.revision) ?? state).toBe(state);
          seen.set(status.revision, state);
        });
        expect(trace.final.revision).toBeGreaterThanOrEqual(
          Math.max(...trace.deliveries.map((status) => status.revision)),
        );
        // No id at or below a `retired_through` handed out is pending or
        // running in that snapshot or any later one.
        let retired = trace.deliveries[0]?.retired_through ?? null;
        for (const status of [...trace.deliveries, trace.final]) {
          if (
            retired === null ||
            compareCaptureIds(status.retired_through, retired) > 0
          ) {
            retired = status.retired_through;
          }
          const { id, phase } = status.capture_state;
          if (phase === "pending" || phase === "running") {
            expect(compareCaptureIds(id, retired)).toBeGreaterThan(0);
          }
        }
      });

      for (const [order, deliveries] of replayOrders(trace.deliveries)) {
        it(`shows the newest delivered, then the final read: ${order}`, async () => {
          useAudioGraphStore.setState(
            useAudioGraphStore.getInitialState(),
            true,
          );
          for (const status of deliveries) {
            useAudioGraphStore.getState().setPipelineStatus(status);
          }
          const newest = deliveries.reduce<PipelineStatus | null>(
            (best, status) =>
              best === null || status.revision > best.revision ? status : best,
            null,
          );
          if (newest !== null) {
            expect(shownDisplay(useAudioGraphStore.getState())).toEqual(
              expectedDisplay(newest),
            );
          }
          vi.mocked(invoke).mockImplementation(async (cmd) =>
            cmd === "get_pipeline_status" ? trace.final : undefined,
          );
          await useAudioGraphStore.getState().reconcileCapture();
          expect(shownDisplay(useAudioGraphStore.getState())).toEqual(
            expectedDisplay(trace.final),
          );
        });
      }
    });
  }
});
