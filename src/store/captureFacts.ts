import type {
  CaptureAttachment,
  CaptureFact,
  CaptureId,
  CaptureReply,
  PipelineStatus,
} from "../types";

/**
 * The renderer half of the capture control protocol (#141): the backend
 * states facts `(capture id, phase)`, the renderer keeps the maximum fact it
 * has seen, and every capture flag derives from that fact and the user's own
 * Start and Stop intents. Max is commutative and idempotent, so the order in
 * which replies and status events arrive cannot change the result.
 */

/** Orders capture ids: epoch first, then sequence. */
export function compareCaptureIds(a: CaptureId, b: CaptureId): number {
  return a.epoch !== b.epoch ? a.epoch - b.epoch : a.seq - b.seq;
}

const PHASE_ORDER = { running: 0, ended: 1 } as const;

/** Orders facts: by capture id, then `running` before `ended`. */
export function compareCaptureFacts(a: CaptureFact, b: CaptureFact): number {
  return (
    compareCaptureIds(a.id, b.id) || PHASE_ORDER[a.phase] - PHASE_ORDER[b.phase]
  );
}

/** The newer of two facts; `null` is older than every fact. */
export function newestCaptureFact(
  current: CaptureFact | null,
  next: CaptureFact,
): CaptureFact {
  return current !== null && compareCaptureFacts(current, next) >= 0
    ? current
    : next;
}

/** The newer of two ids; `null` is older than every id. */
export function newestCaptureId(
  a: CaptureId | null,
  b: CaptureId | null,
): CaptureId | null {
  if (a === null) return b;
  if (b === null) return a;
  return compareCaptureIds(a, b) >= 0 ? a : b;
}

/** The capture fact a status snapshot states, written with its stage. */
export function pipelineCaptureFact(status: PipelineStatus): CaptureFact {
  const running =
    status.capture.type === "Running" || status.capture.type === "Degraded";
  return { id: status.capture_id, phase: running ? "running" : "ended" };
}

/** What the UI may show about capture, from facts and intents alone. */
export interface CaptureView {
  /** The newest fact is Running, for a capture at least as new as Start. */
  isCapturing: boolean;
  /** A Start is newer than every fact seen so far. */
  captureStarting: boolean;
  /** A Stop is not yet covered by an Ended fact. */
  captureStopping: boolean;
  /** The backend transcribes exactly the capture shown running. */
  isTranscribing: boolean;
}

export function captureView(
  fact: CaptureFact | null,
  startIntent: CaptureId | null,
  stopIntent: CaptureId | null,
  transcribing: CaptureId | null,
): CaptureView {
  const isCapturing =
    fact !== null &&
    fact.phase === "running" &&
    (startIntent === null || compareCaptureIds(fact.id, startIntent) >= 0);
  const captureStarting =
    startIntent !== null &&
    (fact === null ||
      compareCaptureFacts(fact, { id: startIntent, phase: "running" }) < 0);
  const captureStopping =
    stopIntent !== null &&
    (fact === null ||
      compareCaptureFacts(fact, { id: stopIntent, phase: "ended" }) < 0);
  const isTranscribing =
    isCapturing &&
    transcribing !== null &&
    fact !== null &&
    compareCaptureIds(fact.id, transcribing) === 0;
  return { isCapturing, captureStarting, captureStopping, isTranscribing };
}

/** The capture slice of the store: the newest fact and the user's intents. */
export interface CaptureState {
  captureFact: CaptureFact | null;
  captureStartIntent: CaptureId | null;
  captureStopIntent: CaptureId | null;
  captureTranscribing: CaptureId | null;
  isCapturing: boolean;
  captureStartTime: number | null;
}

/** Backend facts to merge and intents to install, all optional. */
export interface CaptureChange {
  facts?: readonly CaptureFact[];
  startIntent?: CaptureId;
  stopIntent?: CaptureId | null;
  /**
   * A capture the backend said it transcribes (kept if older than the one
   * already known: transcription only ends with its capture), or `null`
   * when this renderer stopped transcription.
   */
  transcribing?: CaptureId | null;
}

/**
 * The one way capture state changes: merge backend facts into the newest
 * fact seen, install the user's intents, and derive every capture flag from
 * those alone. `now` stamps the start of a capture first seen running.
 */
export function reduceCaptureState(
  state: CaptureState,
  change: CaptureChange,
  now: number,
): CaptureState & CaptureView {
  let fact = state.captureFact;
  for (const next of change.facts ?? []) fact = newestCaptureFact(fact, next);
  const captureStartIntent = change.startIntent ?? state.captureStartIntent;
  const captureStopIntent =
    change.stopIntent !== undefined
      ? change.stopIntent
      : state.captureStopIntent;
  const captureTranscribing =
    change.transcribing === undefined
      ? state.captureTranscribing
      : change.transcribing === null
        ? null
        : newestCaptureId(state.captureTranscribing, change.transcribing);
  const view = captureView(
    fact,
    captureStartIntent,
    captureStopIntent,
    captureTranscribing,
  );
  return {
    captureFact: fact,
    captureStartIntent,
    captureStopIntent,
    captureTranscribing,
    ...view,
    captureStartTime: view.isCapturing
      ? state.isCapturing
        ? (state.captureStartTime ?? now)
        : now
      : null,
  };
}

/** What the capture toggle offers. */
export interface CaptureToggle {
  /** Stop, while a capture runs or starts; it stays enabled. */
  offersStop: boolean;
  /**
   * Start is enabled once this realm is attached (a failed attachment's
   * Start retries it) and a source is selected.
   */
  startEnabled: boolean;
}

export function captureToggle(state: {
  isCapturing: boolean;
  captureStarting: boolean;
  captureAttach: "detached" | "attaching" | "ready" | "failed";
  selectedSourceIds: readonly string[];
}): CaptureToggle {
  const offersStop = state.isCapturing || state.captureStarting;
  return {
    offersStop,
    startEnabled:
      !offersStop &&
      (state.captureAttach === "ready" || state.captureAttach === "failed") &&
      state.selectedSourceIds.length > 0,
  };
}

function isCaptureId(value: unknown): value is CaptureId {
  if (typeof value !== "object" || value === null) return false;
  const id = value as Partial<CaptureId>;
  return (
    Number.isSafeInteger(id.epoch) &&
    Number.isSafeInteger(id.seq) &&
    (id.epoch as number) >= 0 &&
    (id.seq as number) >= 0
  );
}

export function isCaptureFact(value: unknown): value is CaptureFact {
  if (typeof value !== "object" || value === null) return false;
  const fact = value as Partial<CaptureFact>;
  return (
    isCaptureId(fact.id) && (fact.phase === "running" || fact.phase === "ended")
  );
}

export function isCaptureReply(value: unknown): value is CaptureReply {
  if (typeof value !== "object" || value === null) return false;
  const reply = value as Partial<CaptureReply>;
  return isCaptureFact(reply.fact) && isCaptureFact(reply.snapshot);
}

export function isCaptureAttachment(
  value: unknown,
): value is CaptureAttachment {
  if (typeof value !== "object" || value === null) return false;
  const attachment = value as Partial<CaptureAttachment>;
  return (
    Number.isSafeInteger(attachment.epoch) &&
    (attachment.epoch as number) > 0 &&
    isCaptureFact(attachment.snapshot)
  );
}

/** The backend's current fact, when a structured error carries one. */
export function errorCaptureSnapshot(error: unknown): CaptureFact | null {
  if (typeof error !== "object" || error === null || !("message" in error)) {
    return null;
  }
  const { message } = error;
  if (typeof message !== "object" || message === null) return null;
  if (!("snapshot" in message)) return null;
  return isCaptureFact(message.snapshot) ? message.snapshot : null;
}
