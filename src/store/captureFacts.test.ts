import { describe, expect, it } from "vitest";
import { snapshot, statusFor } from "../test/captureBackend";
import type { CaptureId } from "../types";
import {
  captureToggle,
  captureUnknownOf,
  compareCaptureIds,
  errorSnapshot,
  isCaptureAttachment,
  isCaptureCommandReply,
  isPipelineStatus,
  NO_PIPELINE_STATUS,
  replySnapshot,
  START_OUTCOMES,
  STOP_OUTCOMES,
  sessionAdmits,
  snapshotView,
  startSettledBy,
  stopTarget,
} from "./captureFacts";

const id = (epoch: number, seq: number): CaptureId => ({ epoch, seq });

describe("capture ids", () => {
  it("order by epoch, then sequence", () => {
    expect(compareCaptureIds(id(1, 9), id(2, 0))).toBeLessThan(0);
    expect(compareCaptureIds(id(2, 3), id(2, 1))).toBeGreaterThan(0);
    expect(compareCaptureIds(id(2, 3), id(2, 3))).toBe(0);
  });
});

describe("snapshotView", () => {
  it("derives every flag from the snapshot alone", () => {
    const shown = id(1, 2);
    const view = snapshotView(
      statusFor(shown, "running", {
        transcription: { capture: shown, generation: 1 },
        backpressure: [
          { capture: id(1, 1), source_index: 0 },
          { capture: shown, source_index: 2 },
        ],
        realtime: { gemini: false, converse: false, openai_realtime: true },
      }),
    );
    expect(view).toEqual({
      captureStatus: "running",
      isCapturing: true,
      isTranscribing: true,
      backpressuredSources: ["1.2/2"],
      isGeminiActive: true,
      activeGeminiCommand: "start_openai_realtime",
    });
  });

  it("is idle for the placeholder before any snapshot", () => {
    expect(snapshotView(NO_PIPELINE_STATUS)).toMatchObject({
      captureStatus: "idle",
      isCapturing: false,
      isTranscribing: false,
      isGeminiActive: false,
      activeGeminiCommand: null,
    });
  });
});

describe("startSettledBy", () => {
  const start = id(1, 3);
  it.each([
    ["pending", true],
    ["running", true],
    ["stopping", true],
  ] as const)("a snapshot showing the start %s settles it", (phase, settled) => {
    expect(
      startSettledBy(
        statusFor(start, phase, { retired_through: id(1, 2) }),
        start,
      ),
    ).toBe(settled);
  });

  it("a snapshot retiring it settles it; one below it does not", () => {
    expect(startSettledBy(snapshot({ retired_through: start }), start)).toBe(
      true,
    );
    expect(startSettledBy(snapshot({ retired_through: id(2, 0) }), start)).toBe(
      true,
    );
    expect(startSettledBy(snapshot({ retired_through: id(1, 2) }), start)).toBe(
      false,
    );
    // Another capture shown, this one not yet retired: still unknown.
    expect(
      startSettledBy(
        statusFor(id(1, 1), "running", { retired_through: id(1, 1) }),
        start,
      ),
    ).toBe(false);
  });
});

describe("stopTarget", () => {
  it("addresses the newest known id, or none", () => {
    const base = {
      pipelineStatus: NO_PIPELINE_STATUS,
      startPending: null,
      unknownStart: null,
    };
    expect(stopTarget(base)).toBeNull();
    expect(
      stopTarget({ ...base, pipelineStatus: statusFor(id(1, 1), "idle") }),
    ).toBeNull();
    expect(
      stopTarget({ ...base, pipelineStatus: statusFor(id(1, 1), "stopping") }),
    ).toEqual(id(1, 1));
    expect(
      stopTarget({
        pipelineStatus: statusFor(id(1, 1), "running"),
        startPending: id(1, 2),
        unknownStart: null,
      }),
    ).toEqual(id(1, 2));
    expect(
      stopTarget({
        pipelineStatus: statusFor(id(2, 1), "running"),
        startPending: null,
        unknownStart: id(1, 4),
      }),
    ).toEqual(id(2, 1));
  });
});

describe("captureToggle", () => {
  const idle = {
    captureStatus: "idle" as const,
    captureUnknown: false,
    startPending: null,
    stopPending: null,
    captureAttach: "ready" as const,
    selectedSourceIds: ["system-default"],
  };

  it("offers Stop unless the window can vouch for an idle backend", () => {
    for (const captureStatus of ["pending", "running", "stopping"] as const) {
      expect(captureToggle({ ...idle, captureStatus }).offersStop).toBe(true);
    }
    expect(captureToggle({ ...idle, captureUnknown: true }).offersStop).toBe(
      true,
    );
    expect(captureToggle({ ...idle, startPending: id(1, 1) }).offersStop).toBe(
      true,
    );
    // A Stop of this window in flight: its own outcome is not in yet.
    expect(captureToggle({ ...idle, stopPending: 1 }).offersStop).toBe(true);
    expect(captureToggle(idle)).toEqual({
      offersStop: false,
      startEnabled: true,
    });
  });

  it("enables Start only attached (or to retry a failed attach) with a source", () => {
    expect(
      captureToggle({ ...idle, captureAttach: "attaching" }).startEnabled,
    ).toBe(false);
    expect(
      captureToggle({ ...idle, captureAttach: "failed" }).startEnabled,
    ).toBe(true);
    expect(captureToggle({ ...idle, selectedSourceIds: [] }).startEnabled).toBe(
      false,
    );
  });
});

describe("captureUnknownOf", () => {
  it("is unknown until a snapshot is applied, and while a command's outcome is", () => {
    const known = {
      pipelineStatus: snapshot(),
      unknownStart: null,
      unknownStop: false,
    };
    expect(
      captureUnknownOf({ ...known, pipelineStatus: NO_PIPELINE_STATUS }),
    ).toBe(true);
    expect(captureUnknownOf({ ...known, unknownStart: id(1, 1) })).toBe(true);
    expect(captureUnknownOf({ ...known, unknownStop: true })).toBe(true);
    expect(captureUnknownOf(known)).toBe(false);
  });
});

describe("sessionAdmits", () => {
  it("admits no session before a snapshot, then only the snapshot's", () => {
    expect(sessionAdmits(NO_PIPELINE_STATUS, "any")).toBe(false);
    expect(sessionAdmits(NO_PIPELINE_STATUS, "")).toBe(false);
    const status = snapshot({ session: { id: "s-1", epoch: 1 } });
    expect(sessionAdmits(status, "s-1")).toBe(true);
    expect(sessionAdmits(status, "s-0")).toBe(false);
  });
});

describe("reply guards", () => {
  it("accept the contract's shapes and reject anything else", () => {
    const status = snapshot();
    expect(isPipelineStatus(status)).toBe(true);
    expect(isPipelineStatus({ ...status, revision: -1 })).toBe(false);
    expect(
      isPipelineStatus({
        ...status,
        capture_state: { id: id(0, 0), phase: "toString" },
      }),
    ).toBe(false);
    expect(isPipelineStatus({ ...status, transcription: { capture: 1 } })).toBe(
      false,
    );
    const { asr: _asr, ...noStage } = status;
    expect(isPipelineStatus(noStage)).toBe(false);
    const { transcription_generation: _generation, ...noGeneration } = status;
    expect(isPipelineStatus(noGeneration)).toBe(false);
    expect(isPipelineStatus({ ...status, transcription_generation: -1 })).toBe(
      false,
    );

    // Outcomes are a closed set per command; anything else tells nothing.
    for (const outcome of ["started", "repeated"]) {
      expect(
        isCaptureCommandReply({ outcome, snapshot: status }, START_OUTCOMES),
      ).toBe(true);
      expect(
        isCaptureCommandReply({ outcome, snapshot: status }, STOP_OUTCOMES),
      ).toBe(false);
    }
    for (const outcome of ["stopped", "not_running"]) {
      expect(
        isCaptureCommandReply({ outcome, snapshot: status }, STOP_OUTCOMES),
      ).toBe(true);
    }
    for (const outcome of ["nothing_running", "toString", "", "garbled"]) {
      expect(
        isCaptureCommandReply({ outcome, snapshot: status }, START_OUTCOMES),
      ).toBe(false);
      expect(
        isCaptureCommandReply({ outcome, snapshot: status }, STOP_OUTCOMES),
      ).toBe(false);
    }
    expect(isCaptureCommandReply({ snapshot: status }, START_OUTCOMES)).toBe(
      false,
    );
    // The snapshot beside any outcome is still read by its revision.
    expect(replySnapshot({ outcome: "garbled", snapshot: status })).toBe(
      status,
    );
    expect(replySnapshot({ outcome: "started", snapshot: {} })).toBeNull();
    expect(replySnapshot(undefined)).toBeNull();
    expect(isCaptureAttachment({ epoch: 3, snapshot: status })).toBe(true);
    expect(isCaptureAttachment({ epoch: 0, snapshot: status })).toBe(false);

    expect(
      errorSnapshot({
        code: "capture_stop_failed",
        message: { snapshot: status },
      }),
    ).toBe(status);
    expect(errorSnapshot(new Error("IPC transport failure"))).toBeNull();
    expect(
      errorSnapshot({ code: "session_invalid", message: { reason: "x" } }),
    ).toBeNull();
  });
});
