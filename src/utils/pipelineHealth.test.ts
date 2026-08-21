import { describe, expect, it } from "vitest";
import type { PipelineStatus } from "../types";
import { computeCompositeHealth } from "./pipelineHealth";

const idle: PipelineStatus = {
  capture: { type: "Idle" },
  pipeline: { type: "Idle" },
  asr: { type: "Idle" },
  diarization: { type: "Idle" },
  entity_extraction: { type: "Idle" },
  graph: { type: "Idle" },
};

function baseInput() {
  return {
    pipelineStatus: idle,
    consumerDroppedChunks: 0,
    persistenceQueueBackpressure: {},
    backpressuredSourceCount: 0,
  };
}

describe("computeCompositeHealth", () => {
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
          graph: { type: "Error", message: "boom" },
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

  it("error outranks degraded when both conditions hold", () => {
    expect(
      computeCompositeHealth({
        ...baseInput(),
        pipelineStatus: {
          ...idle,
          capture: { type: "Error", message: "boom" },
        },
        consumerDroppedChunks: 3,
      }),
    ).toBe("error");
  });
});
