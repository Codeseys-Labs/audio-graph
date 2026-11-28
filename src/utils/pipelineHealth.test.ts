import { describe, expect, it } from "vitest";
import type { RuntimeDiagnostic } from "../generated/runtimeDiagnostic";
import { NO_PIPELINE_STATUS } from "../store/captureFacts";
import type { PipelineStatus } from "../types";
import { computeCompositeHealth } from "./pipelineHealth";

// Closed, content-free diagnostic every `Error`/`Degraded` stage now carries
// (#155) — the fixtures only need a contract-valid value; which one is
// irrelevant to the behavior under test.
const diagnostic: RuntimeDiagnostic = {
  kind: "runtime",
  detail: {
    code: "unsupported",
    retryable: false,
    recovery_action: "review_configuration",
    context: { operation: "transcription", transport: "native" },
  },
};

const idle: PipelineStatus = NO_PIPELINE_STATUS;

function baseInput() {
  return {
    pipelineStatus: idle,
    consumerDroppedChunks: 0,
    persistenceQueueBackpressure: {},
    backpressuredSourceCount: 0,
  };
}

describe("computeCompositeHealth", () => {
  it("treats a reconnect as degraded and preserves other stage failures", () => {
    const pipelineStatus: PipelineStatus = {
      ...idle,
      asr: {
        type: "Reconnecting",
        reason: "provider_retry_later",
        retry_delay: "medium",
      },
    };
    expect(computeCompositeHealth({ ...baseInput(), pipelineStatus })).toBe(
      "degraded",
    );
    expect(
      computeCompositeHealth({
        ...baseInput(),
        pipelineStatus: {
          ...pipelineStatus,
          graph: { type: "Error", diagnostic },
        },
      }),
    ).toBe("error");
  });

  it("is healthy when every stage is idle and nothing is dropping", () => {
    expect(computeCompositeHealth(baseInput())).toBe("healthy");
  });

  it("stays healthy while a stage is actively Running (not a problem)", () => {
    expect(
      computeCompositeHealth({
        ...baseInput(),
        pipelineStatus: {
          ...idle,
          asr: { type: "Running", processed_count: 12 },
        },
      }),
    ).toBe("healthy");
  });

  it("is error when any stage reports Error, even if others are fine", () => {
    expect(
      computeCompositeHealth({
        ...baseInput(),
        pipelineStatus: {
          ...idle,
          asr: { type: "Running", processed_count: 12 },
          graph: { type: "Error", diagnostic },
        },
      }),
    ).toBe("error");
  });

  it("is degraded when a processed-audio consumer is dropping chunks", () => {
    expect(
      computeCompositeHealth({ ...baseInput(), consumerDroppedChunks: 3 }),
    ).toBe("degraded");
  });

  it("is degraded when a persistence writer queue has dropped events", () => {
    expect(
      computeCompositeHealth({
        ...baseInput(),
        persistenceQueueBackpressure: {
          transcript_event: {
            writer: "transcript_event",
            is_backpressured: true,
            queue_capacity: 2048,
            dropped_count: 2,
          },
        },
      }),
    ).toBe("degraded");
  });

  it("is degraded when a capture source's ring buffer is dropping chunks", () => {
    expect(
      computeCompositeHealth({ ...baseInput(), backpressuredSourceCount: 1 }),
    ).toBe("degraded");
  });

  it("is degraded when the diarization stage reports Degraded (audio-graph-586b)", () => {
    expect(
      computeCompositeHealth({
        ...baseInput(),
        pipelineStatus: {
          ...idle,
          diarization: {
            type: "Degraded",
            reason: "engine_not_compiled",
            diagnostic,
          },
        },
      }),
    ).toBe("degraded");
  });

  it("error outranks degraded when both conditions hold", () => {
    expect(
      computeCompositeHealth({
        ...baseInput(),
        pipelineStatus: {
          ...idle,
          capture: { type: "Error", diagnostic },
        },
        consumerDroppedChunks: 3,
      }),
    ).toBe("error");
  });

  it("error outranks a Degraded stage when both conditions hold", () => {
    expect(
      computeCompositeHealth({
        ...baseInput(),
        pipelineStatus: {
          ...idle,
          capture: { type: "Error", diagnostic },
          diarization: {
            type: "Degraded",
            reason: "engine_not_compiled",
            diagnostic,
          },
        },
      }),
    ).toBe("error");
  });
});
