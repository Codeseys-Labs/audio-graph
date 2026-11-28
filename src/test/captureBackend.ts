import type {
  CaptureFact,
  CaptureId,
  PipelineStatus,
  StageStatus,
} from "../types";

/**
 * Test doubles for the capture control protocol (#141): what a backend that
 * accepts every start and every Stop replies, and status snapshots stamped
 * with a capture id.
 */

let attachments = 0;

export function running(id: CaptureId): CaptureFact {
  return { id, phase: "running" };
}

export function ended(id: CaptureId): CaptureFact {
  return { id, phase: "ended" };
}

function captureIdArg(args: unknown): CaptureId {
  if (typeof args === "object" && args !== null && "id" in args) {
    const { id } = args;
    if (typeof id === "object" && id !== null && "epoch" in id && "seq" in id) {
      return { epoch: Number(id.epoch), seq: Number(id.seq) };
    }
  }
  throw new Error("a capture command without a capture id");
}

/**
 * The reply to `cmd` from a backend that commits every start and completes
 * every Stop, or `undefined` when `cmd` is not a capture control command.
 * Each attach issues a new epoch.
 */
export function captureBackendReply(cmd: string, args?: unknown): unknown {
  switch (cmd) {
    case "capture_attach": {
      attachments += 1;
      return {
        epoch: attachments,
        snapshot: ended({ epoch: attachments, seq: 0 }),
      };
    }
    case "start_capture": {
      const id = captureIdArg(args);
      return { fact: running(id), snapshot: running(id) };
    }
    case "stop_capture": {
      const id = captureIdArg(args);
      return { fact: ended(id), snapshot: ended(id) };
    }
    default:
      return undefined;
  }
}

const idle: StageStatus = { type: "Idle" };

/** A status snapshot whose capture stage states `fact`, other stages idle. */
export function statusFor(
  fact: CaptureFact,
  stages: Partial<Omit<PipelineStatus, "capture_id" | "capture">> = {},
): PipelineStatus {
  return {
    capture_id: fact.id,
    capture:
      fact.phase === "running" ? { type: "Running", processed_count: 0 } : idle,
    pipeline: idle,
    asr: idle,
    diarization: idle,
    entity_extraction: idle,
    graph: idle,
    ...stages,
  };
}
