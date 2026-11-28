import type {
  AudioGraphStore,
  CaptureAttachment,
  CaptureCommandReply,
  CaptureId,
  CapturePhase,
  PipelineStatus,
  RealtimeModes,
  RealtimeStartCommand,
  StageStatus,
} from "../types";

/**
 * The renderer half of the capture snapshot contract (#141): the backend
 * publishes one revisioned snapshot of everything the window shows about
 * capture, transcription, the realtime modes and the session; the store
 * applies a snapshot only when its revision is above the last one applied
 * and derives every flag from the applied one alone. A command's own outcome
 * only settles its spinner, or marks the state unknown when its reply is
 * lost.
 */

/** Orders capture ids: epoch first, then sequence. */
export function compareCaptureIds(a: CaptureId, b: CaptureId): number {
  return a.epoch !== b.epoch ? a.epoch - b.epoch : a.seq - b.seq;
}

const idle: StageStatus = { type: "Idle" };

/** The store's snapshot until the first one arrives: revision -1, all idle. */
export const NO_PIPELINE_STATUS: PipelineStatus = {
  revision: -1,
  capture_state: { id: { epoch: 0, seq: 0 }, phase: "idle" },
  retired_through: { epoch: 0, seq: 0 },
  transcription: null,
  realtime: { gemini: false, converse: false, openai_realtime: false },
  session: { id: "", epoch: 0 },
  backpressure: [],
  capture: idle,
  pipeline: idle,
  asr: idle,
  diarization: idle,
  entity_extraction: idle,
  graph: idle,
};

const REALTIME_START_COMMANDS: Record<
  keyof RealtimeModes,
  RealtimeStartCommand
> = {
  gemini: "start_gemini",
  converse: "start_converse",
  openai_realtime: "start_openai_realtime",
};

const REALTIME_MODES = ["gemini", "converse", "openai_realtime"] as const;

/** Every store flag one snapshot determines. */
export interface SnapshotView {
  captureStatus: CapturePhase;
  isCapturing: boolean;
  isTranscribing: boolean;
  backpressuredSources: string[];
  isGeminiActive: boolean;
  activeGeminiCommand: RealtimeStartCommand | null;
}

export function snapshotView(status: PipelineStatus): SnapshotView {
  const { id, phase } = status.capture_state;
  const modes = REALTIME_MODES.filter((mode) => status.realtime[mode]);
  return {
    captureStatus: phase,
    isCapturing: phase === "running",
    isTranscribing: status.transcription !== null,
    // Only the shown capture's sources: an entry for another capture
    // describes a capture the window does not show.
    backpressuredSources: status.backpressure
      .filter((entry) => compareCaptureIds(entry.capture, id) === 0)
      .map((entry) => `${id.epoch}.${id.seq}/${entry.source_index}`),
    isGeminiActive: modes.length > 0,
    // Several modes at once name no single stop: `stopGemini` stops them all.
    activeGeminiCommand:
      modes.length === 1 ? REALTIME_START_COMMANDS[modes[0]] : null,
  };
}

/**
 * Whether `status` settles a Start whose reply was lost: it shows that id
 * registered (pending, running or stopping), or shows that it can no longer
 * begin.
 */
export function startSettledBy(status: PipelineStatus, id: CaptureId): boolean {
  const shown = status.capture_state;
  return (
    (shown.phase !== "idle" && compareCaptureIds(shown.id, id) === 0) ||
    compareCaptureIds(status.retired_through, id) >= 0
  );
}

/**
 * The newest capture id this renderer knows of, which a Stop addresses: the
 * shown capture unless idle, a start in flight, or a start whose reply was
 * lost. `null` when there is none.
 */
export function stopTarget(
  state: Pick<AudioGraphStore, "pipelineStatus" | "startPending" | "unknownStart">,
): CaptureId | null {
  const { id, phase } = state.pipelineStatus.capture_state;
  let newest = phase === "idle" ? null : id;
  for (const candidate of [state.startPending, state.unknownStart]) {
    if (
      candidate !== null &&
      (newest === null || compareCaptureIds(candidate, newest) > 0)
    ) {
      newest = candidate;
    }
  }
  return newest;
}

/** What the capture toggle offers. */
export interface CaptureToggle {
  /**
   * Stop, while the backend holds or may hold a capture: it is pending,
   * running or stopping, its state is unknown, or a start is in flight.
   * Stop stays enabled throughout.
   */
  offersStop: boolean;
  /**
   * Start is enabled once this realm is attached (a failed attachment's
   * Start retries it) and a source is selected.
   */
  startEnabled: boolean;
}

export function captureToggle(
  state: Pick<
    AudioGraphStore,
    | "captureStatus"
    | "captureUnknown"
    | "startPending"
    | "captureAttach"
    | "selectedSourceIds"
  >,
): CaptureToggle {
  const offersStop =
    state.captureStatus !== "idle" ||
    state.captureUnknown ||
    state.startPending !== null;
  return {
    offersStop,
    startEnabled:
      !offersStop &&
      (state.captureAttach === "ready" || state.captureAttach === "failed") &&
      state.selectedSourceIds.length > 0,
  };
}

/**
 * A start is in flight or the backend prepares one: the selection it
 * resolved, and the store it opens into, are the capture's.
 */
export function captureStarting(
  state: Pick<AudioGraphStore, "startPending" | "captureStatus">,
): boolean {
  return state.startPending !== null || state.captureStatus === "pending";
}

/**
 * Whether the global store belongs to live capture: the backend holds, or
 * may hold, a capture, or transcribes. A historical load must neither begin
 * nor land meanwhile.
 */
export function liveCaptureHoldsStore(
  state: Pick<
    AudioGraphStore,
    "captureStatus" | "isTranscribing" | "startPending" | "captureUnknown"
  >,
): boolean {
  return (
    state.captureStatus !== "idle" ||
    state.isTranscribing ||
    state.startPending !== null ||
    state.captureUnknown
  );
}

/**
 * Whether an event tagged with `sessionId` belongs to the applied snapshot's
 * session. Before any snapshot is applied every session is admitted.
 */
export function sessionAdmits(
  status: PipelineStatus,
  sessionId: string,
): boolean {
  return status.revision < 0 || status.session.id === sessionId;
}

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isCaptureId(value: unknown): value is CaptureId {
  return (
    typeof value === "object" &&
    value !== null &&
    "epoch" in value &&
    "seq" in value &&
    isCount(value.epoch) &&
    isCount(value.seq)
  );
}

const CAPTURE_PHASES: Record<CapturePhase, true> = {
  pending: true,
  running: true,
  stopping: true,
  idle: true,
};

const STAGES = [
  "capture",
  "pipeline",
  "asr",
  "diarization",
  "entity_extraction",
  "graph",
] as const;

/** A complete snapshot, as the contract shapes it. */
export function isPipelineStatus(value: unknown): value is PipelineStatus {
  if (
    typeof value !== "object" ||
    value === null ||
    !("revision" in value) ||
    !("capture_state" in value) ||
    !("retired_through" in value) ||
    !("transcription" in value) ||
    !("realtime" in value) ||
    !("session" in value) ||
    !("backpressure" in value)
  ) {
    return false;
  }
  const {
    revision,
    capture_state,
    retired_through,
    transcription,
    realtime,
    session,
    backpressure,
  } = value;
  return (
    isCount(revision) &&
    typeof capture_state === "object" &&
    capture_state !== null &&
    "id" in capture_state &&
    "phase" in capture_state &&
    isCaptureId(capture_state.id) &&
    typeof capture_state.phase === "string" &&
    Object.hasOwn(CAPTURE_PHASES, capture_state.phase) &&
    isCaptureId(retired_through) &&
    (transcription === null ||
      (typeof transcription === "object" &&
        "capture" in transcription &&
        "generation" in transcription &&
        isCaptureId(transcription.capture) &&
        isCount(transcription.generation))) &&
    typeof realtime === "object" &&
    realtime !== null &&
    REALTIME_MODES.every(
      (mode) => mode in realtime && typeof Reflect.get(realtime, mode) === "boolean",
    ) &&
    typeof session === "object" &&
    session !== null &&
    "id" in session &&
    "epoch" in session &&
    typeof session.id === "string" &&
    isCount(session.epoch) &&
    Array.isArray(backpressure) &&
    backpressure.every(
      (entry: unknown) =>
        typeof entry === "object" &&
        entry !== null &&
        "capture" in entry &&
        "source_index" in entry &&
        isCaptureId(entry.capture) &&
        isCount(entry.source_index),
    ) &&
    STAGES.every((stage) => {
      const status: unknown = Reflect.get(value, stage);
      return (
        typeof status === "object" &&
        status !== null &&
        "type" in status &&
        typeof status.type === "string"
      );
    })
  );
}

export function isCaptureCommandReply(
  value: unknown,
): value is CaptureCommandReply {
  return (
    typeof value === "object" &&
    value !== null &&
    "outcome" in value &&
    "snapshot" in value &&
    typeof value.outcome === "string" &&
    isPipelineStatus(value.snapshot)
  );
}

export function isCaptureAttachment(
  value: unknown,
): value is CaptureAttachment {
  return (
    typeof value === "object" &&
    value !== null &&
    "epoch" in value &&
    "snapshot" in value &&
    isCount(value.epoch) &&
    value.epoch > 0 &&
    isPipelineStatus(value.snapshot)
  );
}

/** The snapshot a structured refusal carries, when it carries a readable one. */
export function errorSnapshot(error: unknown): PipelineStatus | null {
  if (typeof error !== "object" || error === null || !("message" in error)) {
    return null;
  }
  const { message } = error;
  if (typeof message !== "object" || message === null) return null;
  if (!("snapshot" in message)) return null;
  return isPipelineStatus(message.snapshot) ? message.snapshot : null;
}
