import type {
  AudioSourceInfo,
  AudioSourceType,
  ProcessInfo,
  SourceId,
} from "../types";

export type CaptureTargetKind =
  | "system_default"
  | "device"
  | "process"
  | "process_tree"
  | "application_name"
  | "unknown";

export interface CaptureTargetDescriptor {
  id: SourceId;
  kind: CaptureTargetKind;
  pid?: number;
  deviceId?: string;
  name?: string;
}

export function processCaptureId(pid: number): SourceId {
  return `app:${pid}`;
}

export function processTreeCaptureId(pid: number): SourceId {
  return `tree:${pid}`;
}

export function applicationNameCaptureId(name: string): SourceId {
  return `name:${name}`;
}

export interface CaptureTargetSourceLike {
  id: SourceId;
  source_type: AudioSourceType;
  capture_target?: SourceId | null;
}

export function sourceCaptureTargetId(
  source: CaptureTargetSourceLike,
): SourceId {
  if (source.capture_target) return source.capture_target;
  switch (source.source_type.type) {
    case "SystemDefault":
      return "system";
    case "Device":
      return source.id.startsWith("device:")
        ? source.id
        : `device:${source.source_type.device_id}`;
    case "Application":
      return processCaptureId(source.source_type.pid);
    case "ApplicationName":
      return applicationNameCaptureId(source.source_type.app_name);
    case "ProcessTree":
      return processTreeCaptureId(source.source_type.pid);
  }
}

function parsePositivePid(value: string): number | null {
  if (!/^\d+$/.test(value)) return null;
  const pid = Number(value);
  return Number.isSafeInteger(pid) && pid > 0 ? pid : null;
}

export function parseCaptureTargetId(id: SourceId): CaptureTargetDescriptor {
  if (id === "system" || id === "system-default") {
    return { id, kind: "system_default" };
  }

  const deviceId = id.match(/^device:(.+)$/)?.[1];
  if (deviceId) {
    return { id, kind: "device", deviceId };
  }

  const processPid = id.match(/^app:(\d+)$/)?.[1];
  if (processPid) {
    const pid = parsePositivePid(processPid);
    return pid === null
      ? { id, kind: "unknown" }
      : { id, kind: "process", pid };
  }

  const processTreePid = id.match(/^(?:tree|process-tree):(\d+)$/)?.[1];
  if (processTreePid) {
    const pid = parsePositivePid(processTreePid);
    return pid === null
      ? { id, kind: "unknown" }
      : { id, kind: "process_tree", pid };
  }

  const appName = id.match(/^(?:name|app-name):(.+)$/)?.[1];
  if (appName) {
    return { id, kind: "application_name", name: appName };
  }

  return { id, kind: "unknown" };
}

export function captureTargetPeerId(id: SourceId): SourceId | null {
  const target = parseCaptureTargetId(id);
  if (target.kind === "process" && target.pid !== undefined) {
    return processTreeCaptureId(target.pid);
  }
  if (target.kind === "process_tree" && target.pid !== undefined) {
    return processCaptureId(target.pid);
  }
  return null;
}

export function removeExclusiveCapturePeer(
  selectedSourceIds: SourceId[],
  nextId: SourceId,
): SourceId[] {
  const peerId = captureTargetPeerId(nextId);
  if (peerId === null) {
    return selectedSourceIds;
  }
  return selectedSourceIds.filter((id) => id !== peerId);
}

export function captureTargetModeLabel(id: SourceId): string | null {
  const target = parseCaptureTargetId(id);
  switch (target.kind) {
    case "system_default":
      return "System";
    case "device":
      return "Device";
    case "process":
      return "Process";
    case "process_tree":
      return "Process tree";
    case "application_name":
      return "Application";
    case "unknown":
      return null;
  }
}

/**
 * Resolves each selected source id to a human-readable label, e.g.
 * "Zoom application" or "Built-in Microphone device".
 *
 * SHELL-R5 (fold of seed audio-graph-4a22): this is the pre-SHELL-R3
 * `ControlBar`'s `selectedLabels` resolution recovered VERBATIM from git
 * history (`git show d12b754^:src/components/ControlBar.tsx`) — SHELL-R3
 * dropped it when `ControlBar` became `NowStrip` (the strip only ever needed
 * a bare count, not the resolved name). The preflight card's Sources row is
 * the first R5+ consumer to need the name again.
 *
 * Two resolution paths, in order:
 *   1. The id matches a live `AudioSourceInfo` from the last `fetchSources()`
 *      — use its `name` + a source-type suffix.
 *   2. It doesn't (source list not yet fetched, or a persisted selection from
 *      a source that's since disappeared) — fall back to parsing the id
 *      itself via `parseCaptureTargetId`, resolving a process/process-tree
 *      pid against the last `fetchProcesses()` list when possible, else
 *      falling back further to the bare pid or the raw id string so the UI
 *      never renders blank.
 */
export function describeSelectedSourceLabels(
  selectedSourceIds: readonly SourceId[],
  sources: readonly AudioSourceInfo[],
  processes: readonly ProcessInfo[],
): string[] {
  return selectedSourceIds.map((id) => {
    const source = sources.find((s) => s.id === id);
    if (source) {
      if (source.source_type.type === "SystemDefault")
        return `${source.name} system`;
      if (source.source_type.type === "Device") return `${source.name} device`;
      if (source.source_type.type === "Application")
        return `${source.name} application`;
      if (source.source_type.type === "ApplicationName")
        return `${source.name} application`;
      if (source.source_type.type === "ProcessTree")
        return `${source.name} process tree`;
      return source.name;
    }

    const target = parseCaptureTargetId(id);
    if (target.kind === "process_tree" && target.pid !== undefined) {
      const proc = processes.find((p) => p.pid === target.pid);
      return proc
        ? `${proc.name} process tree`
        : `PID ${target.pid} process tree`;
    }
    if (target.kind === "process" && target.pid !== undefined) {
      const proc = processes.find((p) => p.pid === target.pid);
      return proc ? `${proc.name} process` : `PID ${target.pid} process`;
    }
    if (target.kind === "application_name" && target.name) {
      return `${target.name} application`;
    }

    return id;
  });
}
