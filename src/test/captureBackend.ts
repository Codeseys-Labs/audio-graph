import { compareCaptureIds, NO_PIPELINE_STATUS } from "../store/captureFacts";
import type {
  CaptureId,
  CapturePhase,
  PipelineStatus,
  TranscriptionSnapshot,
} from "../types";

/**
 * Test doubles for the capture snapshot contract (#141): snapshots stamped
 * with ever higher revisions, and a backend that commits every start and
 * completes every Stop, answering each command with the snapshot after its
 * effect. Call {@link resetCaptureBackend} before each test that uses it.
 */

let revision = 0;

/** A snapshot at the next revision: idle, in session `session-0`, unless overridden. */
export function snapshot(
  overrides: Partial<PipelineStatus> = {},
): PipelineStatus {
  revision += 1;
  return {
    ...NO_PIPELINE_STATUS,
    session: { id: "session-0", epoch: 0 },
    revision,
    ...overrides,
  };
}

const ORIGIN: CaptureId = { epoch: 0, seq: 0 };

/**
 * The newest id `retired_through` may name while `id` is in `phase`: below
 * a capture that is pending or running, which it never covers; `id` itself
 * once that capture is stopping or ended.
 */
function retiredWith(id: CaptureId, phase: CapturePhase): CaptureId {
  if (phase !== "pending" && phase !== "running") return id;
  return id.seq > 0 ? { epoch: id.epoch, seq: id.seq - 1 } : ORIGIN;
}

/**
 * A snapshot at the next revision showing capture `id` in `phase` (its
 * capture stage written to match, and `retired_through` as high as the
 * contract allows: just below `id` while it is pending or running, `id`
 * itself otherwise).
 */
export function statusFor(
  id: CaptureId,
  phase: CapturePhase,
  overrides: Partial<PipelineStatus> = {},
): PipelineStatus {
  return snapshot({
    capture_state: { id, phase },
    retired_through: retiredWith(id, phase),
    capture:
      phase === "running"
        ? { type: "Running", processed_count: 0 }
        : { type: "Idle" },
    ...overrides,
  });
}

interface BackendState {
  attachments: number;
  capture: { id: CaptureId; phase: CapturePhase };
  retired: CaptureId;
  transcription: TranscriptionSnapshot | null;
  generation: number;
  last: PipelineStatus | null;
}

let backend: BackendState = {
  attachments: 0,
  capture: { id: ORIGIN, phase: "idle" },
  retired: ORIGIN,
  transcription: null,
  generation: 0,
  last: null,
};

/** A fresh backend: idle, nothing attached. Revisions keep increasing. */
export function resetCaptureBackend(): void {
  backend = {
    attachments: 0,
    capture: { id: ORIGIN, phase: "idle" },
    retired: ORIGIN,
    transcription: null,
    generation: 0,
    last: null,
  };
}

function older(a: CaptureId, b: CaptureId): CaptureId {
  return compareCaptureIds(a, b) <= 0 ? a : b;
}

/** Publish the backend's state as a new revision. */
function publish(): PipelineStatus {
  const { id, phase } = backend.capture;
  backend.last = statusFor(id, phase, {
    retired_through: older(backend.retired, retiredWith(id, phase)),
    transcription: backend.transcription,
    transcription_generation: backend.generation,
  });
  return backend.last;
}

function idArg(args: unknown, key: "id" | "captureId"): CaptureId | null {
  if (typeof args !== "object" || args === null || !(key in args)) {
    throw new Error(`a capture command without ${key}`);
  }
  const id: unknown = Reflect.get(args, key);
  if (id === null) return null;
  if (typeof id === "object" && "epoch" in id && "seq" in id) {
    return { epoch: Number(id.epoch), seq: Number(id.seq) };
  }
  throw new Error(`a capture command with an unreadable ${key}`);
}

function newer(a: CaptureId, b: CaptureId): CaptureId {
  return compareCaptureIds(a, b) >= 0 ? a : b;
}

/**
 * The reply to `cmd` from a backend that commits every start and completes
 * every Stop, or `undefined` when `cmd` is not a capture control command.
 * Each attach issues a new epoch.
 */
export function captureBackendReply(cmd: string, args?: unknown): unknown {
  switch (cmd) {
    case "capture_attach": {
      backend.attachments += 1;
      backend.retired = newer(backend.retired, {
        epoch: backend.attachments,
        seq: 0,
      });
      return { epoch: backend.attachments, snapshot: publish() };
    }
    case "start_capture": {
      const id = idArg(args, "id");
      if (id === null) throw new Error("start_capture without an id");
      backend.capture = { id, phase: "running" };
      backend.retired = newer(backend.retired, id);
      backend.transcription = null;
      return { outcome: "started", snapshot: publish() };
    }
    case "stop_capture": {
      // A stop-everything (no id) ends everything up to the newest id.
      const id =
        idArg(args, "id") ?? newer(backend.retired, backend.capture.id);
      backend.retired = newer(backend.retired, id);
      if (
        backend.capture.phase === "idle" ||
        compareCaptureIds(backend.capture.id, id) > 0
      ) {
        return { outcome: "not_running", snapshot: publish() };
      }
      backend.capture = { id: backend.capture.id, phase: "idle" };
      backend.transcription = null;
      return { outcome: "stopped", snapshot: publish() };
    }
    case "start_transcribe": {
      const id = idArg(args, "captureId");
      if (
        id === null ||
        backend.capture.phase !== "running" ||
        compareCaptureIds(backend.capture.id, id) !== 0
      ) {
        throw {
          code: "transcription_refused",
          message: { reason: "capture_not_running", snapshot: publish() },
        };
      }
      const expected: unknown =
        typeof args === "object" && args !== null
          ? Reflect.get(args, "expectedGeneration")
          : undefined;
      if (expected !== backend.generation + 1) {
        return { outcome: "repeated", snapshot: backend.last ?? publish() };
      }
      backend.generation = expected;
      backend.transcription = { capture: id, generation: backend.generation };
      return { outcome: "started", snapshot: publish() };
    }
    case "stop_transcribe": {
      const id = idArg(args, "captureId");
      const generation: unknown =
        typeof args === "object" && args !== null
          ? Reflect.get(args, "generation")
          : undefined;
      const current = backend.transcription;
      if (
        current === null ||
        id === null ||
        compareCaptureIds(current.capture, id) !== 0 ||
        current.generation !== generation
      ) {
        return { outcome: "not_running", snapshot: backend.last ?? publish() };
      }
      backend.transcription = null;
      return { outcome: "stopped", snapshot: publish() };
    }
    case "get_pipeline_status":
      return backend.last ?? publish();
    default:
      return undefined;
  }
}
