import { describe, expect, it } from "vitest";
import type { AudioSourceInfo, ProcessInfo } from "../types";
import {
  captureTargetModeLabel,
  captureTargetPeerId,
  describeSelectedSourceLabels,
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

  // SHELL-R5 (fold of seed audio-graph-4a22): `describeSelectedSourceLabels`
  // is the pre-SHELL-R3 `ControlBar`'s `selectedLabels` resolution recovered
  // verbatim from git history — the preflight card's Sources row is its
  // first R5+ consumer.
  describe("describeSelectedSourceLabels", () => {
    function src(overrides: Partial<AudioSourceInfo> = {}): AudioSourceInfo {
      return {
        id: "system-default",
        name: "System Audio",
        source_type: { type: "SystemDefault" },
        is_active: false,
        ...overrides,
      };
    }
    function proc(overrides: Partial<ProcessInfo> = {}): ProcessInfo {
      return { pid: 100, name: "zoom", exe_path: null, ...overrides };
    }

    it("suffixes a matched source's name with its source-type kind", () => {
      expect(
        describeSelectedSourceLabels(
          ["system-default"],
          [src({ id: "system-default", name: "Built-in" })],
          [],
        ),
      ).toEqual(["Built-in system"]);

      expect(
        describeSelectedSourceLabels(
          ["device:mic-1"],
          [
            src({
              id: "device:mic-1",
              name: "USB Mic",
              source_type: { type: "Device", device_id: "mic-1" },
            }),
          ],
          [],
        ),
      ).toEqual(["USB Mic device"]);

      expect(
        describeSelectedSourceLabels(
          ["app:100"],
          [
            src({
              id: "app:100",
              name: "Zoom",
              source_type: { type: "Application", pid: 100, app_name: "zoom" },
            }),
          ],
          [],
        ),
      ).toEqual(["Zoom application"]);

      expect(
        describeSelectedSourceLabels(
          ["name:Spotify"],
          [
            src({
              id: "name:Spotify",
              name: "Spotify",
              source_type: { type: "ApplicationName", app_name: "Spotify" },
            }),
          ],
          [],
        ),
      ).toEqual(["Spotify application"]);

      expect(
        describeSelectedSourceLabels(
          ["tree:100"],
          [
            src({
              id: "tree:100",
              name: "Zoom",
              source_type: { type: "ProcessTree", pid: 100 },
            }),
          ],
          [],
        ),
      ).toEqual(["Zoom process tree"]);
    });

    it("falls back to parsing the raw id + a process lookup when no source list entry matches", () => {
      expect(
        describeSelectedSourceLabels(["tree:100"], [], [proc({ pid: 100 })]),
      ).toEqual(["zoom process tree"]);
      expect(
        describeSelectedSourceLabels(["app:100"], [], [proc({ pid: 100 })]),
      ).toEqual(["zoom process"]);
      expect(describeSelectedSourceLabels(["name:Spotify"], [], [])).toEqual([
        "Spotify application",
      ]);
    });

    it("degrades to a PID placeholder when the process list hasn't caught up either, and to the raw id as the last resort", () => {
      expect(describeSelectedSourceLabels(["app:999"], [], [])).toEqual([
        "PID 999 process",
      ]);
      expect(describeSelectedSourceLabels(["tree:999"], [], [])).toEqual([
        "PID 999 process tree",
      ]);
      expect(describeSelectedSourceLabels(["mystery-id"], [], [])).toEqual([
        "mystery-id",
      ]);
    });

    it("resolves each selected id independently and preserves order", () => {
      expect(
        describeSelectedSourceLabels(
          ["system-default", "device:mic-1"],
          [
            src({ id: "system-default", name: "Built-in" }),
            src({
              id: "device:mic-1",
              name: "USB Mic",
              source_type: { type: "Device", device_id: "mic-1" },
            }),
          ],
          [],
        ),
      ).toEqual(["Built-in system", "USB Mic device"]);
    });
  });
});
