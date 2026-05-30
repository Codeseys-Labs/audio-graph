import { describe, expect, it } from "vitest";
import {
  captureTargetModeLabel,
  captureTargetPeerId,
  parseCaptureTargetId,
  processCaptureId,
  processTreeCaptureId,
  removeExclusiveCapturePeer,
} from "./captureTarget";

describe("captureTarget utilities", () => {
  it("parses capture target ids into typed descriptors", () => {
    expect(parseCaptureTargetId("system-default")).toEqual({
      id: "system-default",
      kind: "system_default",
    });
    expect(parseCaptureTargetId("device:mic-1")).toEqual({
      id: "device:mic-1",
      kind: "device",
      deviceId: "mic-1",
    });
    expect(parseCaptureTargetId("app:4242")).toEqual({
      id: "app:4242",
      kind: "process",
      pid: 4242,
    });
    expect(parseCaptureTargetId("process-tree:4242")).toEqual({
      id: "process-tree:4242",
      kind: "process_tree",
      pid: 4242,
    });
  });

  it("finds mutually exclusive process and process-tree peers", () => {
    expect(captureTargetPeerId(processCaptureId(42))).toBe(
      processTreeCaptureId(42),
    );
    expect(captureTargetPeerId(processTreeCaptureId(42))).toBe(
      processCaptureId(42),
    );
    expect(captureTargetPeerId("device:mic-1")).toBeNull();
  });

  it("removes the peer target before adding a process mode selection", () => {
    expect(
      removeExclusiveCapturePeer(
        ["system-default", processCaptureId(42)],
        processTreeCaptureId(42),
      ),
    ).toEqual(["system-default"]);

    expect(
      removeExclusiveCapturePeer(
        ["system-default", processTreeCaptureId(42)],
        processCaptureId(42),
      ),
    ).toEqual(["system-default"]);
  });

  it("formats capture target mode labels", () => {
    expect(captureTargetModeLabel("system-default")).toBe("System");
    expect(captureTargetModeLabel("device:mic-1")).toBe("Device");
    expect(captureTargetModeLabel(processCaptureId(42))).toBe("Process");
    expect(captureTargetModeLabel(processTreeCaptureId(42))).toBe(
      "Process tree",
    );
    expect(captureTargetModeLabel("bad")).toBeNull();
  });
});
