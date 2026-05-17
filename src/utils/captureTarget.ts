import type { SourceId } from "../types";

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
    return `process-tree:${pid}`;
}

function parsePositivePid(value: string): number | null {
    if (!/^\d+$/.test(value)) return null;
    const pid = Number(value);
    return Number.isSafeInteger(pid) && pid > 0 ? pid : null;
}

export function parseCaptureTargetId(id: SourceId): CaptureTargetDescriptor {
    if (id === "system-default") {
        return { id, kind: "system_default" };
    }

    const deviceId = id.match(/^device:(.+)$/)?.[1];
    if (deviceId) {
        return { id, kind: "device", deviceId };
    }

    const processPid = id.match(/^app:(\d+)$/)?.[1];
    if (processPid) {
        const pid = parsePositivePid(processPid);
        return pid === null ? { id, kind: "unknown" } : { id, kind: "process", pid };
    }

    const processTreePid = id.match(/^process-tree:(\d+)$/)?.[1];
    if (processTreePid) {
        const pid = parsePositivePid(processTreePid);
        return pid === null ? { id, kind: "unknown" } : { id, kind: "process_tree", pid };
    }

    const appName = id.match(/^app-name:(.+)$/)?.[1];
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
