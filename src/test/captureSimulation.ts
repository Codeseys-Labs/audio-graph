import type { CaptureFact, CaptureId, PipelineStatus } from "../types";

/**
 * A simulation of the capture control protocol (#141, amendment H): a model
 * backend that implements the protocol, a network that delivers commands
 * once or twice and replies and events in any order, and a scheduler that
 * explores those orders. The renderer side is the production store and
 * hooks, driven through mocked `invoke` and `listen`; this module knows them
 * only through {@link RendererRealm}.
 */

// ── Ids and facts ────────────────────────────────────────────────────────

export function compareIds(a: CaptureId, b: CaptureId): number {
  return a.epoch !== b.epoch ? a.epoch - b.epoch : a.seq - b.seq;
}

function maxId(a: CaptureId, b: CaptureId): CaptureId {
  return compareIds(a, b) >= 0 ? a : b;
}

const sameId = (a: CaptureId | null, b: CaptureId | null) =>
  a !== null && b !== null && compareIds(a, b) === 0;

const idText = (id: CaptureId) => `(${id.epoch},${id.seq})`;

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

function realmArg(args: unknown): string | null {
  if (typeof args !== "object" || args === null || !("realm" in args)) {
    return null;
  }
  return typeof args.realm === "string" ? args.realm : null;
}

// ── The model backend ────────────────────────────────────────────────────

/** Deliberate deviations, to show the simulation catches them. */
export interface BackendDefects {
  /** Round 4: a repeated Start of the live id was refused with Ended. */
  repeatedStartEndsLive?: boolean;
  /** Round 4: the tray Stop was an event the renderer had to handle. */
  trayStopIsRendererEvent?: boolean;
}

type CommandOutcome =
  | { ok: true; value: unknown }
  | { ok: false; error: unknown }
  /** Answered later, by the start it repeats. */
  | { deferred: true };

interface PendingStart {
  id: CaptureId;
  /** Deliveries of this start waiting for its outcome. */
  waiters: Array<(outcome: CaptureFact) => void>;
}

export class ModelBackend {
  epoch = 0;
  attachedRealm: string | null = null;
  cancelledThrough: CaptureId = { epoch: 0, seq: 0 };
  lastRegistered: CaptureId = { epoch: 0, seq: 0 };
  pending: PendingStart | null = null;
  live: CaptureId | null = null;
  /** The id the status snapshot is stamped with. */
  statusId: CaptureId = { epoch: 0, seq: 0 };
  transcribing = false;
  /** Every fact it stated about a live id that was not true. */
  readonly violations: string[] = [];
  nextStopFails: "cleanup" | "transport" | null = null;
  nextAttachFails = false;
  /** The last status emitted while capturing, for a slow thread to repeat. */
  private lastRunningStatus: PipelineStatus | null = null;

  constructor(
    readonly emit: (name: string, payload: unknown) => void,
    readonly defects: BackendDefects = {},
  ) {}

  /** Record a fact the backend states, checking it is true. */
  private state(fact: CaptureFact): CaptureFact {
    if (fact.phase === "ended" && sameId(this.live, fact.id)) {
      this.violations.push(`Ended${idText(fact.id)} while it was live`);
    }
    if (
      fact.phase === "running" &&
      !sameId(this.live, fact.id) &&
      !sameId(this.pending?.id ?? null, fact.id)
    ) {
      this.violations.push(`Running${idText(fact.id)} while it was not live`);
    }
    return fact;
  }

  snapshot(): CaptureFact {
    return this.state({
      id: this.statusId,
      phase: this.live === null ? "ended" : "running",
    });
  }

  status(): PipelineStatus {
    const fact = this.snapshot();
    const idle = { type: "Idle" } as const;
    const running = { type: "Running", processed_count: 0 } as const;
    return {
      capture_id: fact.id,
      capture: fact.phase === "running" ? running : idle,
      pipeline: fact.phase === "running" ? running : idle,
      asr: this.transcribing ? running : idle,
      diarization: idle,
      entity_extraction: idle,
      graph: idle,
    };
  }

  private emitStatus(): void {
    const status = this.status();
    if (status.capture.type === "Running") this.lastRunningStatus = status;
    this.emit("pipeline-status", status);
  }

  /** A slower thread emits a snapshot it read while capture was running. */
  emitStaleSnapshot(): void {
    if (this.lastRunningStatus !== null) {
      this.emit("pipeline-status", this.lastRunningStatus);
    }
  }

  private reply(fact: CaptureFact): CommandOutcome {
    return {
      ok: true,
      value: { fact: this.state(fact), snapshot: this.snapshot() },
    };
  }

  private refusal(id: CaptureId, reason: string): CommandOutcome {
    return {
      ok: false,
      error: {
        code: "capture_start_failed",
        message: {
          reason,
          source_index: null,
          sources: [],
          capture: this.state({ id, phase: "ended" }),
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
        return this.attach(realmArg(args));
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
          return { ok: false, error: new Error("IPC transport failure") };
        }
        return this.stop(captureIdArg(args, "id"), failure === "cleanup");
      }
      case "start_transcribe": {
        const id = captureIdArg(args, "captureId");
        if (id === null || !sameId(this.live, id)) {
          return {
            ok: false,
            error: {
              code: "session_invalid",
              message: {
                reason: "Cannot start transcription: capture is not running",
              },
            },
          };
        }
        this.transcribing = true;
        return { ok: true, value: null };
      }
      case "stop_transcribe":
        this.transcribing = false;
        return { ok: true, value: null };
      case "get_pipeline_status":
        return { ok: true, value: this.status() };
      case "new_session_cmd":
        if (this.live !== null || this.pending !== null) {
          return {
            ok: false,
            error: {
              code: "session_invalid",
              message: { reason: "capture is running" },
            },
          };
        }
        return { ok: true, value: "next-session" };
      default:
        throw new Error(`not a capture command: ${cmd}`);
    }
  }

  private attach(realm: string | null): CommandOutcome {
    if (this.nextAttachFails) {
      this.nextAttachFails = false;
      return { ok: false, error: new Error("IPC transport failure") };
    }
    if (realm === null || realm !== this.attachedRealm) {
      this.attachedRealm = realm;
      this.epoch += 1;
      this.cancelThrough({ epoch: this.epoch, seq: 0 });
      if (this.live === null) this.statusId = { epoch: this.epoch, seq: 0 };
    }
    return {
      ok: true,
      value: { epoch: this.epoch, snapshot: this.snapshot() },
    };
  }

  private start(
    id: CaptureId,
    answer: (outcome: CommandOutcome) => void,
  ): CommandOutcome {
    if (sameId(this.live, id) && !this.defects.repeatedStartEndsLive) {
      return this.reply({ id, phase: "running" });
    }
    if (this.pending !== null && sameId(this.pending.id, id)) {
      this.pending.waiters.push((fact) =>
        answer(
          fact.phase === "running"
            ? this.reply(fact)
            : this.refusal(id, "cancelled"),
        ),
      );
      return { deferred: true };
    }
    if (
      compareIds(id, this.cancelledThrough) <= 0 ||
      compareIds(id, this.lastRegistered) <= 0
    ) {
      return this.refusal(id, "cancelled");
    }
    this.lastRegistered = id;
    if (this.pending !== null || this.live !== null) {
      return this.refusal(id, "already_running");
    }
    this.pending = {
      id,
      waiters: [
        (fact) =>
          answer(
            fact.phase === "running"
              ? this.reply(fact)
              : this.refusal(id, "cancelled"),
          ),
      ],
    };
    return { deferred: true };
  }

  /** The pending start's preparation finished: it commits unless cancelled. */
  commit(): void {
    const pending = this.pending;
    if (pending === null) return;
    const cancelled = compareIds(pending.id, this.cancelledThrough) <= 0;
    if (!cancelled) {
      this.live = pending.id;
      this.statusId = pending.id;
      this.transcribing = false;
    }
    const outcome: CaptureFact = {
      id: pending.id,
      phase: cancelled ? "ended" : "running",
    };
    this.pending = null;
    if (!cancelled) this.emitStatus();
    for (const waiter of pending.waiters) waiter(outcome);
  }

  private cancelThrough(through: CaptureId): void {
    this.cancelledThrough = maxId(this.cancelledThrough, through);
  }

  private newestId(): CaptureId {
    return maxId(
      maxId(this.lastRegistered, this.live ?? this.lastRegistered),
      this.cancelledThrough,
    );
  }

  private end(): void {
    this.live = null;
    this.transcribing = false;
    this.emitStatus();
  }

  /** End every capture up to `id` (`null`: the newest the backend knows). */
  stop(id: CaptureId | null, cleanupFails = false): CommandOutcome {
    const through = id ?? this.newestId();
    this.cancelThrough(through);
    if (this.live !== null && compareIds(this.live, through) <= 0) this.end();
    if (cleanupFails) {
      return {
        ok: false,
        error: {
          code: "capture_stop_failed",
          message: { snapshot: this.snapshot() },
        },
      };
    }
    return this.reply({ id: through, phase: "ended" });
  }

  /** The live capture's last source died; reconciliation ends it. */
  sourceDies(): void {
    if (this.live !== null) this.end();
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

/** What the simulation reads from, and does to, one renderer realm. */
export interface RendererView {
  isCapturing: boolean;
  captureStarting: boolean;
  isTranscribing: boolean;
  /** The capture the renderer shows, when it shows one. */
  captureId: CaptureId | null;
  /** The capture it shows transcribing, when it shows that. */
  transcribingId: CaptureId | null;
  /** The toggle offers Start (enabled) or Stop. */
  startOffered: boolean;
  stopOffered: boolean;
}

export interface RendererRealm {
  view(): RendererView;
  pressStart(): void;
  pressStop(): void;
  newSession(): void;
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
  stopFailures: number;
  /** Attaches whose delivery fails in the transport. */
  attachFailures: number;
  /** Status snapshots a slower thread emits late. */
  staleSnapshots: number;
  /** Commands that may be delivered twice. */
  duplicates: number;
}

export const NO_BUDGET: Budgets = {
  starts: 0,
  stops: 0,
  reloads: 0,
  trayStops: 0,
  sourceDeaths: 0,
  newSessions: 0,
  stopFailures: 0,
  attachFailures: 0,
  staleSnapshots: 0,
  duplicates: 0,
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
  /** The user's last capture action, for the intent invariant. */
  private lastIntent: "start" | "stop" | null = null;
  /** The last Stop's intent cannot hold: it never ran. */
  private lastStopUnreliable = false;
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
        if (message.outcome.ok)
          message.invocation.resolve(message.outcome.value);
        else message.invocation.reject(message.outcome.error);
        return;
      case "commit":
        this.backend.commit();
        return;
      case "tray":
        this.backend.stop(null);
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
          });
        };
        const hadPending = this.backend.pending !== null;
        const outcome = this.backend.execute(message.cmd, message.args, answer);
        if (!hadPending && this.backend.pending !== null) {
          this.push({ kind: "commit", seq: this.nextSeq() });
        }
        if (
          message.cmd === "stop_capture" &&
          "ok" in outcome &&
          !outcome.ok &&
          outcome.error instanceof Error
        ) {
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
    this.messages.forEach((message, age) => {
      const take = () => {
        this.messages = this.messages.filter((m) => m !== message);
      };
      choices.push({
        label: this.messageLabel(message),
        delay: age,
        apply: () => {
          take();
          this.deliver(message, false);
        },
      });
      if (
        message.kind === "command" &&
        message.deliveries === 0 &&
        this.budgets.duplicates > 0
      ) {
        choices.push({
          label: `${this.messageLabel(message)} twice`,
          delay: age,
          apply: () => {
            // Delivered now, and again later: only the later reply is read.
            this.budgets.duplicates -= 1;
            this.deliver(message, true);
          },
        });
      }
    });
    const view = this.realm?.view();
    const b = this.budgets;
    if (view && b.starts > 0 && view.startOffered) {
      choices.push({
        label: "press Start",
        apply: () => {
          b.starts -= 1;
          this.lastIntent = "start";
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
    if (b.trayStops > 0) {
      choices.push({
        label: "tray Stop",
        apply: () => {
          b.trayStops -= 1;
          this.lastIntent = "stop";
          // A start already sent and not yet registered is newer than
          // anything the tray Stop can name.
          this.lastStopUnreliable = this.messages.some(
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
    if (b.sourceDeaths > 0 && this.backend.live !== null) {
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
    if (b.stopFailures > 0) {
      for (const kind of ["cleanup", "transport"] as const) {
        choices.push({
          label: `next Stop fails (${kind})`,
          apply: () => {
            b.stopFailures -= 1;
            this.backend.nextStopFails = kind;
          },
        });
      }
    }
    if (b.attachFailures > 0) {
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
  }

  /** Deliver everything pending, oldest first, until nothing is. */
  async drain(): Promise<void> {
    for (let guard = 0; !this.quiescent; guard += 1) {
      if (guard > 200) throw new Error("the system never went quiet");
      const [oldest] = [...this.messages].sort((a, b) => a.seq - b.seq);
      const label = this.messageLabel(oldest);
      const choice = this.choices().find((c) => c.label === label);
      if (!choice) throw new Error(`no choice for ${label}`);
      await this.step(choice);
    }
  }

  /** The invariants of amendment H at quiescence; empty when all hold. */
  invariantFailures(): string[] {
    const failures: string[] = [];
    const view = this.view();
    const { live, transcribing, violations } = this.backend;
    const shown = view.isCapturing ? view.captureId : null;
    if (
      view.isCapturing !== (live !== null) ||
      !sameIdOrBothNull(shown, live)
    ) {
      failures.push(
        `1: renderer shows ${shown ? `capture ${idText(shown)}` : "no capture"}, backend has ${live ? idText(live) : "none"}`,
      );
    }
    const backendTranscribing = transcribing && live !== null ? live : null;
    const shownTranscribing = view.isTranscribing ? view.transcribingId : null;
    if (
      view.isTranscribing !== (backendTranscribing !== null) ||
      !sameIdOrBothNull(shownTranscribing, backendTranscribing)
    ) {
      failures.push(
        `2: renderer transcribing ${shownTranscribing ? idText(shownTranscribing) : view.isTranscribing ? "(unaddressed)" : "nothing"}, backend ${backendTranscribing ? idText(backendTranscribing) : "nothing"}`,
      );
    }
    if ((live !== null || this.backend.pending !== null) && !view.stopOffered) {
      failures.push("3: the backend holds a capture and no Stop is offered");
    }
    for (const violation of violations) failures.push(`4: ${violation}`);
    if (
      this.lastIntent === "stop" &&
      !this.lastStopUnreliable &&
      (live !== null || this.backend.pending !== null)
    ) {
      failures.push("5: the last action was a Stop and a capture is running");
    }
    return failures;
  }

  /**
   * Invariant 3's second half: this Stop, over a working transport (faults
   * armed and not yet spent are disarmed), leads to an idle backend.
   */
  async finalStop(kind: "window" | "tray"): Promise<string[]> {
    const failures: string[] = [];
    this.backend.nextStopFails = null;
    this.backend.nextAttachFails = false;
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
    if (this.backend.live !== null || this.backend.pending !== null) {
      failures.push(`3: a ${kind} Stop left the backend capturing`);
    }
    if (view.isCapturing || view.captureStarting) {
      failures.push(`3: after a ${kind} Stop the renderer still shows capture`);
    }
    return failures;
  }
}

function sameIdOrBothNull(a: CaptureId | null, b: CaptureId | null): boolean {
  return (a === null && b === null) || sameId(a, b);
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
      if (step > 200) throw new Error("schedule too long");
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
    const failures = sim.invariantFailures();
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

/**
 * A named schedule: each step takes the first choice whose label starts
 * with the next entry (or matches it, for a pattern); afterwards everything
 * pending is delivered oldest first.
 */
export function scriptedPick(script: readonly (string | RegExp)[]): Pick {
  let next = 0;
  return (choices, quiescent) => {
    if (next < script.length) {
      const step = script[next];
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
    // The oldest message, delivered once.
    const oldest = choices.findIndex(
      ({ label, delay }) => delay === 0 && /#\d+$/.test(label),
    );
    if (oldest < 0) throw new Error("nothing to drain");
    return oldest;
  };
}
