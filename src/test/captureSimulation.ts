import { isPipelineStatus } from "../store/captureFacts";
import type {
  CaptureId,
  CapturePhase,
  PipelineStatus,
  SourceBackpressure,
  TranscriptionSnapshot,
} from "../types";

/**
 * A simulation of the capture snapshot contract (#141): a model backend that
 * publishes a revisioned snapshot on every change and answers every command
 * with the snapshot after its effect, a network that delivers commands once
 * or twice and replies and events in any order (dropping, losing or
 * garbling some), and a scheduler that explores those orders. The renderer
 * side is the production store and hooks, driven through mocked `invoke` and
 * `listen`; this module knows them only through {@link RendererRealm}.
 *
 * The oracle: after every step the renderer displays the state of the
 * highest-revision snapshot delivered to it, and after a status read it
 * displays the model backend's state.
 */

// ── Ids ──────────────────────────────────────────────────────────────────

export function compareIds(a: CaptureId, b: CaptureId): number {
  return a.epoch !== b.epoch ? a.epoch - b.epoch : a.seq - b.seq;
}

function maxId(a: CaptureId, b: CaptureId): CaptureId {
  return compareIds(a, b) >= 0 ? a : b;
}

const sameId = (a: CaptureId, b: CaptureId) => compareIds(a, b) === 0;

const idText = (id: CaptureId) => `(${id.epoch},${id.seq})`;

const ORIGIN: CaptureId = { epoch: 0, seq: 0 };

function captureIdArg(
  args: unknown,
  key: "id" | "captureId",
): CaptureId | null {
  if (typeof args !== "object" || args === null || !(key in args)) return null;
  const value: unknown = Reflect.get(args, key);
  if (
    typeof value === "object" &&
    value !== null &&
    "epoch" in value &&
    "seq" in value &&
    typeof value.epoch === "number" &&
    typeof value.seq === "number"
  ) {
    return { epoch: value.epoch, seq: value.seq };
  }
  return null;
}

function stringArg(args: unknown, key: "realm" | "operation"): string | null {
  if (typeof args !== "object" || args === null || !(key in args)) {
    return null;
  }
  const value: unknown = Reflect.get(args, key);
  return typeof value === "string" ? value : null;
}

function numberArg(args: unknown, key: "generation"): number | null {
  if (typeof args !== "object" || args === null || !(key in args)) {
    return null;
  }
  const value: unknown = Reflect.get(args, key);
  return typeof value === "number" ? value : null;
}

// ── The model backend ────────────────────────────────────────────────────

/** Deliberate deviations, to show the simulation catches them. */
export interface BackendDefects {
  /** Round 4: a repeated Start of the live id was refused as ended. */
  repeatedStartEndsLive?: boolean;
  /** Round 4: the tray Stop was an event the renderer had to handle. */
  trayStopIsRendererEvent?: boolean;
}

type CommandOutcome =
  | { ok: true; value: unknown }
  | { ok: false; error: unknown }
  /** Answered later, when the start it registered commits. */
  | { deferred: true };

interface PendingStart {
  id: CaptureId;
  /** A Stop or a newer attachment fenced it while it prepared. */
  cancelled: boolean;
  /** Deliveries of this start waiting for its outcome. */
  waiters: Array<(outcome: CommandOutcome) => void>;
}

const transportFailure = () => new Error("IPC transport failure");

export class ModelBackend {
  revision = 0;
  epoch = 0;
  attachedRealm: string | null = null;
  capture: { id: CaptureId; phase: CapturePhase } = {
    id: ORIGIN,
    phase: "idle",
  };
  retired: CaptureId = ORIGIN;
  transcription: TranscriptionSnapshot | null = null;
  session = { id: "session-0", epoch: 0 };
  backpressure: SourceBackpressure[] = [];
  pending: PendingStart | null = null;
  /** What each stop-everything ended through, on its first delivery. */
  private operations = new Map<string, CaptureId>();
  private generations = 0;
  private foreignWindows = 0;
  /** Every segment spoken, with the session it belongs to. */
  readonly segments: Array<{ id: string; session: number }> = [];
  /** Every snapshot it handed out that was not true. */
  readonly violations: string[] = [];
  nextStopFails: "cleanup" | "transport" | null = null;
  nextAttachFails = false;
  /** The last snapshot published while capturing, for a slow thread to repeat. */
  private lastRunning: PipelineStatus | null = null;

  constructor(
    readonly emit: (name: string, payload: unknown) => void,
    readonly defects: BackendDefects = {},
  ) {}

  /** The state now, at the current revision. */
  snapshot(): PipelineStatus {
    const idle = { type: "Idle" } as const;
    const running = { type: "Running", processed_count: 0 } as const;
    const capturing = this.capture.phase === "running";
    return {
      revision: this.revision,
      capture_state: { ...this.capture },
      retired_through: this.retired,
      transcription: this.transcription,
      realtime: { gemini: false, converse: false, openai_realtime: false },
      session: { ...this.session },
      backpressure: [...this.backpressure],
      capture: capturing ? running : idle,
      pipeline: capturing ? running : idle,
      asr: this.transcription !== null ? running : idle,
      diarization: idle,
      entity_extraction: idle,
      graph: idle,
    };
  }

  /** Every change bumps the revision and is published. */
  private publish(): void {
    this.revision += 1;
    const status = this.snapshot();
    if (status.capture_state.phase === "running") this.lastRunning = status;
    this.emit("pipeline-status", status);
  }

  /** A slower thread emits a snapshot it took while capture was running. */
  emitStaleSnapshot(): void {
    if (this.lastRunning !== null) this.emit("pipeline-status", this.lastRunning);
  }

  private reply(outcome: string): CommandOutcome {
    return { ok: true, value: { outcome, snapshot: this.snapshot() } };
  }

  private startRefusal(reason: string): CommandOutcome {
    return {
      ok: false,
      error: {
        code: "capture_start_failed",
        message: {
          reason,
          source_index: null,
          sources: [],
          snapshot: this.snapshot(),
        },
      },
    };
  }

  /** Execute one delivery of `cmd`. */
  execute(
    cmd: string,
    args: unknown,
    answer: (outcome: CommandOutcome) => void,
  ): CommandOutcome {
    switch (cmd) {
      case "capture_attach":
        if (this.nextAttachFails) {
          // Lost in the transport: the attach never ran.
          this.nextAttachFails = false;
          return { ok: false, error: transportFailure() };
        }
        return this.attach(stringArg(args, "realm"));
      case "start_capture": {
        const id = captureIdArg(args, "id");
        if (id === null) throw new Error("start_capture without an id");
        return this.start(id, answer);
      }
      case "stop_capture": {
        // Only a Stop that crosses the IPC can fail in transport.
        const failure = this.nextStopFails;
        this.nextStopFails = null;
        if (failure === "transport") {
          return { ok: false, error: transportFailure() };
        }
        return this.stop(
          captureIdArg(args, "id"),
          stringArg(args, "operation"),
          failure === "cleanup",
        );
      }
      case "start_transcribe":
        return this.startTranscribe(captureIdArg(args, "captureId"));
      case "stop_transcribe":
        return this.stopTranscribe(
          captureIdArg(args, "captureId"),
          numberArg(args, "generation"),
        );
      case "get_pipeline_status":
        return { ok: true, value: this.snapshot() };
      case "new_session_cmd":
        return this.rotate();
      default:
        throw new Error(`not a capture command: ${cmd}`);
    }
  }

  private attach(realm: string | null): CommandOutcome {
    if (realm === null) throw new Error("capture_attach without a realm");
    // The same realm's attach is idempotent; another one supersedes it.
    if (realm !== this.attachedRealm) this.supersede(realm);
    return {
      ok: true,
      value: { epoch: this.epoch, snapshot: this.snapshot() },
    };
  }

  /**
   * A new attachment: a new epoch, every id of the earlier ones retired,
   * and the start the previous attachment left preparing cancelled.
   */
  private supersede(realm: string): void {
    this.attachedRealm = realm;
    this.epoch += 1;
    this.retired = maxId(this.retired, { epoch: this.epoch, seq: 0 });
    if (this.pending !== null && !this.pending.cancelled) {
      this.pending.cancelled = true;
      this.capture = { id: this.pending.id, phase: "stopping" };
    }
    this.publish();
  }

  private start(
    id: CaptureId,
    answer: (outcome: CommandOutcome) => void,
  ): CommandOutcome {
    // Identity before validation: a repeat of the live or the preparing
    // start reports that start.
    if (this.capture.phase === "running" && sameId(this.capture.id, id)) {
      if (this.defects.repeatedStartEndsLive) {
        this.revision += 1;
        this.violations.push(`idle${idText(id)} stated while it was running`);
        return {
          ok: false,
          error: {
            code: "capture_start_failed",
            message: {
              reason: "cancelled",
              source_index: null,
              sources: [],
              snapshot: {
                ...this.snapshot(),
                capture_state: { id, phase: "idle" },
              },
            },
          },
        };
      }
      return this.reply("repeated");
    }
    if (this.pending !== null && sameId(this.pending.id, id)) {
      return this.startRefusal("still_preparing");
    }
    if (id.epoch !== this.epoch) return this.startRefusal("stale_attachment");
    if (compareIds(id, this.retired) <= 0) {
      return this.startRefusal("cancelled");
    }
    // Registered, refused or not, it is retired.
    this.retired = id;
    if (this.capture.phase !== "idle") {
      this.publish();
      return this.startRefusal(
        this.capture.phase === "stopping"
          ? "previous_capture_stopping"
          : "already_running",
      );
    }
    this.capture = { id, phase: "pending" };
    this.transcription = null;
    this.backpressure = [];
    this.pending = { id, cancelled: false, waiters: [answer] };
    this.publish();
    return { deferred: true };
  }

  /** The pending start's preparation finished: it commits unless cancelled. */
  commit(): void {
    const pending = this.pending;
    if (pending === null) return;
    this.pending = null;
    if (pending.cancelled) {
      // Its cleanup: what was torn down is idle now.
      if (sameId(this.capture.id, pending.id)) {
        this.capture = { id: pending.id, phase: "idle" };
      }
      this.publish();
      for (const waiter of pending.waiters) {
        waiter(this.startRefusal("cancelled"));
      }
      return;
    }
    this.capture = { id: pending.id, phase: "running" };
    this.publish();
    for (const waiter of pending.waiters) waiter(this.reply("started"));
  }

  /** End every capture up to `id`, or (no id) every one `operation` covers. */
  stop(
    id: CaptureId | null,
    operation: string | null,
    cleanupFails = false,
  ): CommandOutcome {
    let through: CaptureId;
    if (id !== null) {
      through = id;
    } else {
      if (operation === null) {
        this.violations.push("stop_capture(null) without an operation");
        return { ok: false, error: transportFailure() };
      }
      // Recorded on first delivery: a replay never ends a later capture.
      through = this.operations.get(operation) ?? this.retired;
      this.operations.set(operation, through);
    }
    const retiring = compareIds(through, this.retired) > 0;
    this.retired = maxId(this.retired, through);
    const { id: shown, phase } = this.capture;
    if (phase === "idle" || compareIds(shown, through) > 0) {
      if (retiring) this.publish();
      return this.reply("nothing_running");
    }
    if (phase === "pending") {
      if (this.pending !== null) this.pending.cancelled = true;
      this.capture = { id: shown, phase: "stopping" };
      this.publish();
      return this.reply("stopped");
    }
    // Stopping is published before cleanup.
    if (phase === "running") {
      this.capture = { id: shown, phase: "stopping" };
      this.publish();
    } else if (retiring) {
      this.publish();
    }
    if (cleanupFails) {
      return {
        ok: false,
        error: {
          code: "capture_stop_failed",
          message: { snapshot: this.snapshot() },
        },
      };
    }
    this.end();
    return this.reply("stopped");
  }

  private end(): void {
    this.capture = { id: this.capture.id, phase: "idle" };
    this.transcription = null;
    this.backpressure = [];
    this.publish();
  }

  /** The live capture's last source died; reconciliation ends it. */
  sourceDies(): void {
    if (this.capture.phase !== "running") return;
    this.capture = { id: this.capture.id, phase: "stopping" };
    this.publish();
    this.end();
  }

  private startTranscribe(id: CaptureId | null): CommandOutcome {
    if (
      id === null ||
      this.capture.phase !== "running" ||
      !sameId(this.capture.id, id)
    ) {
      return {
        ok: false,
        error: {
          code: "transcription_refused",
          message: { reason: "capture_not_running", snapshot: this.snapshot() },
        },
      };
    }
    if (this.transcription !== null && sameId(this.transcription.capture, id)) {
      return this.reply("repeated");
    }
    this.generations += 1;
    this.transcription = { capture: id, generation: this.generations };
    this.publish();
    return this.reply("started");
  }

  private stopTranscribe(
    id: CaptureId | null,
    generation: number | null,
  ): CommandOutcome {
    const current = this.transcription;
    if (
      current === null ||
      id === null ||
      !sameId(current.capture, id) ||
      current.generation !== generation
    ) {
      return this.reply("not_running");
    }
    this.transcription = null;
    this.publish();
    return this.reply("stopped");
  }

  private rotate(): CommandOutcome {
    if (this.capture.phase !== "idle" || this.pending !== null) {
      return {
        ok: false,
        error: {
          code: "session_invalid",
          message: { reason: "capture is running" },
        },
      };
    }
    const epoch = this.session.epoch + 1;
    this.session = { id: `session-${epoch}`, epoch };
    this.publish();
    return { ok: true, value: this.session.id };
  }

  /** A running capture's first source starts or stops dropping chunks. */
  toggleBackpressure(): void {
    if (this.capture.phase !== "running") return;
    const id = this.capture.id;
    this.backpressure =
      this.backpressure.length > 0 ? [] : [{ capture: id, source_index: 0 }];
    this.publish();
  }

  /** A transcribed capture produces a segment. */
  speak(): void {
    if (this.transcription === null) return;
    const id = `seg-${this.segments.length + 1}`;
    this.segments.push({ id, session: this.session.epoch });
    this.emit("transcript-update", {
      id,
      source_id: "system-default",
      speaker_id: null,
      speaker_label: null,
      text: id,
      start_time: 0,
      end_time: 1,
      confidence: 1,
    });
  }

  /**
   * Another window attaches, superseding this one's attachment, and starts
   * a capture when none is held.
   */
  foreignStart(): void {
    this.foreignWindows += 1;
    this.supersede(`foreign-${this.foreignWindows}`);
    if (this.capture.phase !== "idle") return;
    const id = { epoch: this.epoch, seq: 1 };
    this.retired = id;
    this.capture = { id, phase: "pending" };
    this.transcription = null;
    this.backpressure = [];
    this.publish();
    this.capture = { id, phase: "running" };
    this.publish();
  }

  /** The backend holds a capture: registered, owned or being torn down. */
  get holdsCapture(): boolean {
    return this.capture.phase !== "idle" || this.pending !== null;
  }
}

// ── The network and the scheduler ────────────────────────────────────────

interface Invocation {
  realm: number;
  resolve: (value: unknown) => void;
  reject: (error: unknown) => void;
}

type Message =
  | {
      kind: "command";
      seq: number;
      realm: number;
      cmd: string;
      args: unknown;
      invocation: Invocation;
      deliveries: number;
    }
  | {
      kind: "reply";
      seq: number;
      cmd: string;
      invocation: Invocation;
      outcome: { ok: true; value: unknown } | { ok: false; error: unknown };
      /** A fault already replaced it. */
      faulted: boolean;
    }
  | {
      kind: "event";
      seq: number;
      realm: number;
      name: string;
      payload: unknown;
    }
  | { kind: "subscribe"; seq: number; realm: number }
  | { kind: "commit"; seq: number }
  | { kind: "tray"; seq: number };

/** The commands the model backend answers; every other one resolves now. */
const CAPTURE_COMMANDS: Record<string, true> = {
  capture_attach: true,
  start_capture: true,
  stop_capture: true,
  start_transcribe: true,
  stop_transcribe: true,
  get_pipeline_status: true,
  new_session_cmd: true,
};

/** What the simulation reads from one renderer realm. */
export interface RendererView {
  /** The applied snapshot's revision (-1 before any). */
  revision: number;
  captureStatus: CapturePhase;
  /** The capture the applied snapshot names. */
  captureId: CaptureId;
  isCapturing: boolean;
  isTranscribing: boolean;
  backpressuredSources: string[];
  sessionEpoch: number;
  /** The state is shown as unknown. */
  unknown: boolean;
  /** A Start's spinner is on. */
  startInFlight: boolean;
  /** The toggle offers Start (enabled) or Stop. */
  startOffered: boolean;
  stopOffered: boolean;
  /** The live transcript's segment ids. */
  transcriptIds: string[];
}

export interface RendererRealm {
  view(): RendererView;
  pressStart(): void;
  pressStop(): void;
  pressStopTranscription(): void;
  newSession(): void;
  /** A focus, visibility change or timer: read the backend's state again. */
  reconcile(): void;
  /** Unmount the realm's hooks; its pending replies never arrive. */
  unload(): void;
}

/** Builds a realm whose `invoke` and `listen` go through `net`. */
export type RealmFactory = (net: RealmNet) => Promise<RendererRealm>;

/** The mocked IPC a realm's `invoke` and `listen` call. */
export interface RealmNet {
  invoke(cmd: string, args?: unknown): Promise<unknown>;
  listen(name: string, handler: (event: unknown) => void): Promise<() => void>;
}

export interface Budgets {
  starts: number;
  stops: number;
  reloads: number;
  trayStops: number;
  sourceDeaths: number;
  newSessions: number;
  /** Stops whose cleanup does not finish. */
  cleanupFailures: number;
  /** Stops lost in the transport before they ran. */
  stopTransportFailures: number;
  /** Attaches whose delivery fails in the transport. */
  attachFailures: number;
  /** Status snapshots a slower thread emits late. */
  staleSnapshots: number;
  /** Commands that may be delivered twice. */
  duplicates: number;
  /** `pipeline-status` emissions the transport drops. */
  drops: number;
  /** Replies lost in the transport after their command ran. */
  lostReplies: number;
  /** Replies that arrive unreadable. */
  unreadableReplies: number;
  /** Focus, visibility or timer status reads. */
  reconciles: number;
  /** Segments a transcribed capture produces. */
  speech: number;
  /** Backpressure changes on the running capture. */
  backpressures: number;
  /** Captures another window attaches and starts. */
  foreignStarts: number;
  /** Stop Transcription presses. */
  stopTranscriptions: number;
}

export const NO_BUDGET: Budgets = {
  starts: 0,
  stops: 0,
  reloads: 0,
  trayStops: 0,
  sourceDeaths: 0,
  newSessions: 0,
  cleanupFailures: 0,
  stopTransportFailures: 0,
  attachFailures: 0,
  staleSnapshots: 0,
  duplicates: 0,
  drops: 0,
  lostReplies: 0,
  unreadableReplies: 0,
  reconciles: 0,
  speech: 0,
  backpressures: 0,
  foreignStarts: 0,
  stopTranscriptions: 0,
};

/** A transition, and how far it departs from delivering oldest first. */
export interface ChoiceInfo {
  label: string;
  /** Older messages it overtakes; 0 for actions. */
  delay: number;
}

interface Choice {
  label: string;
  delay?: number;
  apply: () => Promise<void> | void;
}

/** A Promise turn for every await an action can chain. */
async function settle(): Promise<void> {
  for (let i = 0; i < 64; i += 1) await Promise.resolve();
}

/** The snapshot a delivered reply value or refusal carries, if any. */
function deliveredSnapshot(value: unknown): PipelineStatus | null {
  if (isPipelineStatus(value)) return value;
  if (typeof value !== "object" || value === null) return null;
  for (const key of ["snapshot", "message"] as const) {
    if (key in value) {
      const inner = deliveredSnapshot(Reflect.get(value, key));
      if (inner !== null) return inner;
    }
  }
  return null;
}

/** What the window must display for a snapshot, derived independently. */
function displayOf(status: PipelineStatus): string {
  const { id, phase } = status.capture_state;
  return JSON.stringify({
    revision: status.revision,
    captureStatus: phase,
    captureId: idText(id),
    isCapturing: phase === "running",
    isTranscribing: status.transcription !== null,
    backpressured: status.backpressure
      .filter((entry) => sameId(entry.capture, id))
      .map((entry) => `${id.epoch}.${id.seq}/${entry.source_index}`)
      .sort(),
    sessionEpoch: status.session.epoch,
  });
}

function displayed(view: RendererView): string {
  return JSON.stringify({
    revision: view.revision,
    captureStatus: view.captureStatus,
    captureId: idText(view.captureId),
    isCapturing: view.isCapturing,
    isTranscribing: view.isTranscribing,
    backpressured: [...view.backpressuredSources].sort(),
    sessionEpoch: view.sessionEpoch,
  });
}

export class CaptureSimulation {
  readonly backend: ModelBackend;
  private messages: Message[] = [];
  private seq = 0;
  private realmNumber = 0;
  private realm: RendererRealm | null = null;
  private handlers = new Map<string, (event: unknown) => void>();
  private subscribed = false;
  private pendingListens: Array<() => void> = [];
  private budgets: Budgets;
  /** The highest-revision snapshot delivered to the loaded realm. */
  private highest: PipelineStatus | null = null;
  /** Segments the loaded realm must show: spoken to it since its last Start. */
  private owedSegments: Array<{ id: string; session: number }> = [];
  /** The user's last capture action, for the intent invariant. */
  private lastIntent: "start" | "stop" | null = null;
  /** The last Stop's intent cannot hold: it did not complete. */
  private lastStopUnreliable = false;
  /** Oracle failures seen after some step, with that step. */
  private stepFailures: string[] = [];
  readonly trace: string[] = [];

  constructor(
    private readonly factory: RealmFactory,
    budgets: Budgets,
    readonly defects: BackendDefects = {},
  ) {
    this.budgets = { ...budgets };
    this.backend = new ModelBackend(
      (name, payload) => this.emit(name, payload),
      defects,
    );
  }

  private push(message: Message): void {
    this.messages.push(message);
  }

  private nextSeq(): number {
    this.seq += 1;
    return this.seq;
  }

  private emit(name: string, payload: unknown): void {
    // An event sent before the realm's listeners subscribed is lost.
    if (!this.subscribed) return;
    if (name === "transcript-update") {
      const segment = this.backend.segments.at(-1);
      if (segment) this.owedSegments.push(segment);
    }
    this.push({
      kind: "event",
      seq: this.nextSeq(),
      realm: this.realmNumber,
      name,
      payload,
    });
  }

  /** The IPC of realm `realm`. */
  private netFor(realm: number): RealmNet {
    return {
      invoke: (cmd, args) => {
        if (!(cmd in CAPTURE_COMMANDS)) return Promise.resolve(undefined);
        return new Promise((resolve, reject) => {
          this.push({
            kind: "command",
            seq: this.nextSeq(),
            realm,
            cmd,
            args,
            invocation: { realm, resolve, reject },
            deliveries: 0,
          });
        });
      },
      listen: (name, handler) =>
        new Promise((resolve) => {
          const unlisten = () => {
            if (this.realmNumber === realm) this.handlers.delete(name);
          };
          this.handlers.set(name, handler);
          this.pendingListens.push(() => resolve(unlisten));
        }),
    };
  }

  async loadRealm(): Promise<void> {
    this.realm?.unload();
    this.realmNumber += 1;
    this.handlers = new Map();
    this.subscribed = false;
    this.pendingListens = [];
    this.highest = null;
    this.owedSegments = [];
    this.realm = await this.factory(this.netFor(this.realmNumber));
    this.push({
      kind: "subscribe",
      seq: this.nextSeq(),
      realm: this.realmNumber,
    });
    await settle();
  }

  /** Unmount the loaded realm: nothing of this run outlives it. */
  close(): void {
    this.realm?.unload();
    this.realm = null;
  }

  view(): RendererView {
    if (this.realm === null) throw new Error("no realm loaded");
    return this.realm.view();
  }

  private observe(status: PipelineStatus | null): void {
    if (status === null) return;
    if (this.highest === null || status.revision > this.highest.revision) {
      this.highest = status;
    }
  }

  private deliver(message: Message, again: boolean): void {
    switch (message.kind) {
      case "subscribe":
        if (message.realm !== this.realmNumber) return;
        this.subscribed = true;
        for (const resolve of this.pendingListens) resolve();
        this.pendingListens = [];
        return;
      case "event": {
        if (message.realm !== this.realmNumber) return;
        if (message.name === "pipeline-status") {
          this.observe(deliveredSnapshot(message.payload));
        }
        this.handlers.get(message.name)?.({
          event: message.name,
          id: 0,
          payload: message.payload,
        });
        return;
      }
      case "reply":
        // A reloaded realm's replies never arrive.
        if (message.invocation.realm !== this.realmNumber) return;
        if (message.outcome.ok) {
          this.observe(deliveredSnapshot(message.outcome.value));
          message.invocation.resolve(message.outcome.value);
        } else {
          this.observe(deliveredSnapshot(message.outcome.error));
          message.invocation.reject(message.outcome.error);
        }
        return;
      case "commit":
        this.backend.commit();
        return;
      case "tray":
        this.backend.stop(null, `tray-${message.seq}`);
        return;
      case "command": {
        message.deliveries += 1;
        const answer = (outcome: CommandOutcome) => {
          if ("deferred" in outcome) return;
          // Only the last delivery's reply is read.
          if (again) return;
          this.push({
            kind: "reply",
            seq: this.nextSeq(),
            cmd: message.cmd,
            invocation: message.invocation,
            outcome,
            faulted: false,
          });
        };
        const hadPending = this.backend.pending !== null;
        const outcome = this.backend.execute(message.cmd, message.args, answer);
        if (!hadPending && this.backend.pending !== null) {
          this.push({ kind: "commit", seq: this.nextSeq() });
        }
        // A Stop that failed did not complete what it was pressed for.
        if (message.cmd === "stop_capture" && "ok" in outcome && !outcome.ok) {
          this.lastStopUnreliable = true;
        }
        answer(outcome);
        return;
      }
    }
  }

  private messageLabel(message: Message): string {
    switch (message.kind) {
      case "command":
        return `deliver ${message.cmd}#${message.seq}`;
      case "reply":
        return `reply ${message.cmd}#${message.seq}`;
      case "event":
        return `event ${message.name}#${message.seq}`;
      case "subscribe":
        return `subscribe#${message.seq}`;
      case "commit":
        return `commit#${message.seq}`;
      case "tray":
        return `tray task#${message.seq}`;
    }
  }

  /** Every transition enabled now. */
  choices(): Choice[] {
    const choices: Choice[] = [];
    const b = this.budgets;
    this.messages.forEach((message, age) => {
      const take = () => {
        this.messages = this.messages.filter((m) => m !== message);
      };
      const label = this.messageLabel(message);
      choices.push({
        label,
        delay: age,
        apply: () => {
          take();
          this.deliver(message, false);
        },
      });
      if (
        message.kind === "command" &&
        message.deliveries === 0 &&
        b.duplicates > 0
      ) {
        choices.push({
          label: `${label} twice`,
          delay: age,
          apply: () => {
            // Delivered now, and again later: only the later reply is read.
            b.duplicates -= 1;
            this.deliver(message, true);
          },
        });
      }
      if (
        message.kind === "event" &&
        message.name === "pipeline-status" &&
        b.drops > 0
      ) {
        choices.push({
          label: `drop ${label}`,
          apply: () => {
            b.drops -= 1;
            take();
          },
        });
      }
      if (message.kind === "reply" && !message.faulted) {
        if (b.lostReplies > 0) {
          choices.push({
            label: `lose ${label}`,
            apply: () => {
              b.lostReplies -= 1;
              message.faulted = true;
              message.outcome = { ok: false, error: transportFailure() };
            },
          });
        }
        if (b.unreadableReplies > 0) {
          choices.push({
            label: `garble ${label}`,
            apply: () => {
              b.unreadableReplies -= 1;
              message.faulted = true;
              message.outcome = { ok: true, value: { unreadable: true } };
            },
          });
        }
      }
    });
    const view = this.realm?.view();
    if (view && b.starts > 0 && view.startOffered) {
      choices.push({
        label: "press Start",
        apply: () => {
          b.starts -= 1;
          this.lastIntent = "start";
          this.owedSegments = [];
          this.realm?.pressStart();
        },
      });
    }
    if (view && b.stops > 0 && view.stopOffered) {
      choices.push({
        label: "press Stop",
        apply: () => {
          b.stops -= 1;
          this.lastIntent = "stop";
          this.lastStopUnreliable = false;
          this.realm?.pressStop();
        },
      });
    }
    if (view && b.stopTranscriptions > 0 && view.isTranscribing) {
      choices.push({
        label: "press Stop Transcription",
        apply: () => {
          b.stopTranscriptions -= 1;
          this.realm?.pressStopTranscription();
        },
      });
    }
    if (b.trayStops > 0) {
      choices.push({
        label: "tray Stop",
        apply: () => {
          b.trayStops -= 1;
          this.lastIntent = "stop";
          // A start already sent and not yet registered, or one this window
          // will still send (a start in flight may be retried), is newer
          // than anything the tray Stop can name.
          this.lastStopUnreliable =
            view?.startInFlight === true ||
            this.messages.some(
              (m) => m.kind === "command" && m.cmd === "start_capture",
            );
          if (this.defects.trayStopIsRendererEvent) {
            this.emit("tray-stop-capture", null);
          } else {
            this.push({ kind: "tray", seq: this.nextSeq() });
          }
        },
      });
    }
    if (b.reloads > 0) {
      choices.push({
        label: "reload",
        apply: async () => {
          b.reloads -= 1;
          await this.loadRealm();
        },
      });
    }
    if (b.sourceDeaths > 0 && this.backend.capture.phase === "running") {
      choices.push({
        label: "source dies",
        apply: () => {
          b.sourceDeaths -= 1;
          this.backend.sourceDies();
        },
      });
    }
    if (view && b.newSessions > 0) {
      choices.push({
        label: "New Session",
        apply: () => {
          b.newSessions -= 1;
          this.realm?.newSession();
        },
      });
    }
    if (view && b.reconciles > 0) {
      choices.push({
        label: "reconcile",
        apply: () => {
          b.reconciles -= 1;
          this.realm?.reconcile();
        },
      });
    }
    if (b.speech > 0 && this.backend.transcription !== null) {
      choices.push({
        label: "speech",
        apply: () => {
          b.speech -= 1;
          this.backend.speak();
        },
      });
    }
    if (b.backpressures > 0 && this.backend.capture.phase === "running") {
      choices.push({
        label: "backpressure",
        apply: () => {
          b.backpressures -= 1;
          this.backend.toggleBackpressure();
        },
      });
    }
    if (b.foreignStarts > 0) {
      choices.push({
        label: "another window starts",
        apply: () => {
          b.foreignStarts -= 1;
          this.lastIntent = "start";
          this.backend.foreignStart();
        },
      });
    }
    // Arming a fault only matters for the next delivery it affects, so it is
    // offered only while such a command is in flight: arming it any earlier
    // explores nothing new.
    const inFlight = (cmd: string) =>
      this.messages.some((m) => m.kind === "command" && m.cmd === cmd);
    for (const [kind, budget] of [
      ["cleanup", "cleanupFailures"],
      ["transport", "stopTransportFailures"],
    ] as const) {
      if (b[budget] > 0 && inFlight("stop_capture")) {
        choices.push({
          label: `next Stop fails (${kind})`,
          apply: () => {
            b[budget] -= 1;
            this.backend.nextStopFails = kind;
          },
        });
      }
    }
    if (b.attachFailures > 0 && inFlight("capture_attach")) {
      choices.push({
        label: "next attach fails",
        apply: () => {
          b.attachFailures -= 1;
          this.backend.nextAttachFails = true;
        },
      });
    }
    if (b.staleSnapshots > 0) {
      choices.push({
        label: "stale snapshot",
        apply: () => {
          b.staleSnapshots -= 1;
          this.backend.emitStaleSnapshot();
        },
      });
    }
    return choices;
  }

  get quiescent(): boolean {
    return this.messages.length === 0;
  }

  async step(choice: Choice): Promise<void> {
    this.trace.push(choice.label);
    await choice.apply();
    await settle();
    // The oracle, after every step: the highest revision delivered is what
    // the window shows.
    if (this.realm !== null && this.highest !== null) {
      const expected = displayOf(this.highest);
      const shown = displayed(this.view());
      if (shown !== expected && this.stepFailures.length === 0) {
        this.stepFailures.push(
          `1: after "${choice.label}" the window shows ${shown}, the highest revision delivered is ${expected}`,
        );
      }
    }
  }

  /** Deliver everything pending, oldest first, until nothing is. */
  async drain(): Promise<void> {
    for (let guard = 0; !this.quiescent; guard += 1) {
      if (guard > 300) throw new Error("the system never went quiet");
      const [oldest] = [...this.messages].sort((a, b) => a.seq - b.seq);
      const label = this.messageLabel(oldest);
      const choice = this.choices().find((c) => c.label === label);
      if (!choice) throw new Error(`no choice for ${label}`);
      await this.step(choice);
    }
  }

  /** Disarm every fault: what follows runs over a working transport. */
  private disarm(): void {
    this.backend.nextStopFails = null;
    this.backend.nextAttachFails = false;
    for (const key of [
      "drops",
      "lostReplies",
      "unreadableReplies",
      "duplicates",
    ] as const) {
      this.budgets[key] = 0;
    }
  }

  /**
   * The invariants at quiescence; empty when all hold. A status read is
   * delivered first: the oracle's second half is that afterwards the window
   * shows the backend's state.
   */
  async invariantFailures(): Promise<string[]> {
    const failures = [...this.stepFailures];
    this.disarm();
    this.realm?.reconcile();
    await settle();
    await this.drain();
    const view = this.view();
    const backend = this.backend.snapshot();
    if (displayed(view) !== displayOf(backend)) {
      failures.push(
        `2: after a status read the window shows ${displayed(view)}, the backend holds ${displayOf(backend)}`,
      );
    }
    if (view.unknown) failures.push("2: after a status read the state is unknown");
    if (this.backend.holdsCapture && !view.stopOffered) {
      failures.push("3: the backend holds a capture and no Stop is offered");
    }
    for (const violation of this.backend.violations) {
      failures.push(`4: ${violation}`);
    }
    if (
      this.lastIntent === "stop" &&
      !this.lastStopUnreliable &&
      this.backend.holdsCapture
    ) {
      failures.push("5: the last action was a Stop and a capture is running");
    }
    const shownSegments = new Set(view.transcriptIds);
    const lost = this.owedSegments.filter(
      (segment) =>
        segment.session === this.backend.session.epoch &&
        !shownSegments.has(segment.id),
    );
    if (lost.length > 0) {
      failures.push(
        `6: the live transcript lost ${lost.map((s) => s.id).join(", ")}`,
      );
    }
    return failures;
  }

  /**
   * Invariant 3's second half: this Stop, over a working transport, leads
   * to an idle backend and a window that shows it idle.
   */
  async finalStop(kind: "window" | "tray"): Promise<string[]> {
    const failures: string[] = [];
    this.disarm();
    if (kind === "window") {
      if (this.view().stopOffered) this.realm?.pressStop();
    } else if (this.defects.trayStopIsRendererEvent) {
      this.emit("tray-stop-capture", null);
    } else {
      this.push({ kind: "tray", seq: this.nextSeq() });
    }
    await settle();
    await this.drain();
    const view = this.view();
    if (this.backend.holdsCapture) {
      failures.push(`3: a ${kind} Stop left the backend capturing`);
    }
    if (
      view.captureStatus !== "idle" ||
      view.startInFlight ||
      view.unknown
    ) {
      failures.push(
        `3: after a ${kind} Stop the window shows ${view.captureStatus}${view.unknown ? " (unknown)" : ""}`,
      );
    }
    return failures;
  }
}

// ── Exploration ──────────────────────────────────────────────────────────

export interface RunResult {
  trace: string[];
  failures: string[];
}

/** Picks the next transition, or ends a quiet run. */
export type Pick = (
  choices: ChoiceInfo[],
  quiescent: boolean,
) => number | "end";

export interface RunOptions {
  budgets: Budgets;
  defects?: BackendDefects;
  /** Deliver the first realm's startup in order before anything else. */
  startAttached?: boolean;
}

/** One run: choices follow `pick` until it ends the run at quiescence. */
export async function runSchedule(
  factory: RealmFactory,
  options: RunOptions,
  pick: Pick,
  finalStop: "window" | "tray",
): Promise<RunResult> {
  const sim = new CaptureSimulation(factory, options.budgets, options.defects);
  try {
    await sim.loadRealm();
    if (options.startAttached) await sim.drain();
    for (let step = 0; ; step += 1) {
      if (step > 300) throw new Error("schedule too long");
      const choices = sim.choices();
      const picked = pick(
        choices.map((c) => ({ label: c.label, delay: c.delay ?? 0 })),
        sim.quiescent,
      );
      if (picked === "end") {
        if (!sim.quiescent) throw new Error("a run ends only when quiet");
        break;
      }
      await sim.step(choices[picked]);
    }
    const failures = await sim.invariantFailures();
    failures.push(...(await sim.finalStop(finalStop)));
    return { trace: [...sim.trace], failures };
  } finally {
    sim.close();
  }
}

/**
 * Every schedule within `options.budgets` whose deliveries overtake at most
 * `maxDelay` older messages in total (delay bounding): depth-first over the
 * option at each step (each enabled transition within the bound, and ending
 * the run when quiet), replaying the prefix from a fresh system each time.
 * Each schedule runs twice, ending once with a window Stop and once with a
 * tray Stop. Returns the number of schedules.
 */
export async function exploreExhaustively(
  factory: RealmFactory,
  options: RunOptions & { maxDelay: number; maxSchedules: number },
  onRun: (result: RunResult) => void,
): Promise<number> {
  let prefix: number[] = [];
  let schedules = 0;
  for (;;) {
    let taken: number[] = [];
    let widths: number[] = [];
    for (const finalStop of ["window", "tray"] as const) {
      const path: number[] = [];
      const pathWidths: number[] = [];
      let delayLeft = options.maxDelay;
      const result = await runSchedule(
        factory,
        options,
        (choices, quiescent) => {
          // This step's options in a fixed order; -1 ends the run.
          const open = choices.flatMap((choice, index) =>
            choice.delay <= delayLeft ? [index] : [],
          );
          if (quiescent) open.push(-1);
          const position =
            path.length < prefix.length ? prefix[path.length] : 0;
          path.push(position);
          pathWidths.push(open.length);
          const option = open[position];
          if (option === -1) return "end";
          delayLeft -= choices[option].delay;
          return option;
        },
        finalStop,
      );
      onRun(result);
      taken = path;
      widths = pathWidths;
    }
    schedules += 1;
    if (schedules > options.maxSchedules) {
      throw new Error(
        `more than ${options.maxSchedules} schedules: not exhaustive`,
      );
    }
    let depth = taken.length - 1;
    while (depth >= 0 && taken[depth] + 1 >= widths[depth]) depth -= 1;
    if (depth < 0) return schedules;
    prefix = [...taken.slice(0, depth), taken[depth] + 1];
  }
}

/** Deterministic PRNG (mulberry32). */
export function seededRandom(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** A random schedule: ends at quiescence with probability `endChance`. */
export function randomPick(random: () => number, endChance = 0.3): Pick {
  return (choices, quiescent) => {
    if (quiescent && (choices.length === 0 || random() < endChance)) {
      return "end";
    }
    if (choices.length === 0) throw new Error("stuck with messages pending");
    return Math.floor(random() * choices.length);
  };
}

/** A script step: deliver everything pending, oldest first, until quiet. */
export const SETTLE = "(deliver everything pending)";

/** The oldest pending message, delivered once. */
function oldestDelivery(choices: ChoiceInfo[]): number {
  const oldest = choices.findIndex(
    ({ label, delay }) =>
      delay === 0 && /^(deliver|reply|event|subscribe|commit|tray)/.test(label),
  );
  if (oldest < 0) throw new Error("nothing to drain");
  return oldest;
}

/**
 * A named schedule: each step takes the first choice whose label starts
 * with the next entry (or matches it, for a pattern), or, for
 * {@link SETTLE}, delivers everything pending; afterwards everything pending
 * is delivered oldest first.
 */
export function scriptedPick(script: readonly (string | RegExp)[]): Pick {
  let next = 0;
  return (choices, quiescent) => {
    while (next < script.length && script[next] === SETTLE && quiescent) {
      next += 1;
    }
    if (next < script.length) {
      const step = script[next];
      if (step === SETTLE) return oldestDelivery(choices);
      const index = choices.findIndex(({ label }) =>
        typeof step === "string" ? label.startsWith(step) : step.test(label),
      );
      if (index < 0) {
        throw new Error(
          `step "${step}" is not enabled; enabled: ${choices.map((c) => c.label).join(", ")}`,
        );
      }
      next += 1;
      return index;
    }
    if (quiescent) return "end";
    return oldestDelivery(choices);
  };
}
