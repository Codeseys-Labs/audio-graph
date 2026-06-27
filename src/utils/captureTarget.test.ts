import { describe, expect, it } from "vitest";
import {
  captureTargetModeLabel,
  captureTargetPeerId,
  parseCaptureTargetId,
  processCaptureId,
  processTreeCaptureId,
  removeExclusiveCapturePeer,
  sourceCaptureTargetId,
} from "./captureTarget";

describe("captureTarget utilities", () => {
  it("parses capture target ids into typed descriptors", () => {
    expect(parseCaptureTargetId("system")).toEqual({
      id: "system",
      kind: "system_default",
    });
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
    expect(parseCaptureTargetId("tree:4242")).toEqual({
      id: "tree:4242",
      kind: "process_tree",
      pid: 4242,
    });
    expect(parseCaptureTargetId("name:Spotify")).toEqual({
      id: "name:Spotify",
      kind: "application_name",
      name: "Spotify",
    });
  });

  it("treats malformed process ids as unknown targets", () => {
    expect(parseCaptureTargetId("app:not-a-pid")).toEqual({
      id: "app:not-a-pid",
      kind: "unknown",
    });
    expect(parseCaptureTargetId("process-tree:0")).toEqual({
      id: "process-tree:0",
      kind: "unknown",
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

  it("constructs canonical target ids from backend source descriptors", () => {
    expect(
      sourceCaptureTargetId({
        id: "system-default",
        source_type: { type: "SystemDefault" },
      }),
    ).toBe("system");
    expect(
      sourceCaptureTargetId({
        id: "opaque-device-row",
        source_type: { type: "Device", device_id: "mic-1" },
      }),
    ).toBe("device:mic-1");
    expect(
      sourceCaptureTargetId({
        id: "app-name:Spotify",
        source_type: { type: "ApplicationName", app_name: "Spotify" },
      }),
    ).toBe("name:Spotify");
    expect(
      sourceCaptureTargetId({
        id: "process-tree:42",
        source_type: { type: "ProcessTree", pid: 42 },
      }),
    ).toBe("tree:42");
    expect(
      sourceCaptureTargetId({
        id: "opaque",
        source_type: { type: "Device", device_id: "mic-1" },
        capture_target: "device:backend-canonical",
      }),
    ).toBe("device:backend-canonical");
  });
});
