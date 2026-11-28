import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { renderHook } from "@testing-library/react";
import {
  afterAll,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  type MockInstance,
  vi,
} from "vitest";
import { useNativeCapture } from "../hooks/useNativeCapture";
import { CAPTURE_RECONCILE_MS, useTauriEvents } from "../hooks/useTauriEvents";
import i18n from "../i18n";
import {
  type BackendDefects,
  type Budgets,
  CaptureSimulation,
  exploreExhaustively,
  NO_BUDGET,
  type RealmFactory,
  type RealmNet,
  randomPick,
  runSchedule,
  SETTLE,
  scriptedPick,
  seededRandom,
} from "../test/captureSimulation";
import type { AppSettings } from "../types";
import { useAudioGraphStore } from ".";
import { captureToggle } from "./captureFacts";

/**
 * The capture snapshot contract (#141) as a simulation: the production store
 * and hooks against a model backend, over every delivery order within small
 * bounds and seeded random orders beyond them. The oracle:
 * 1. after every step the window shows the state of the highest-revision
 *    snapshot delivered to it;
 * 2. after a status read it shows the backend's state, and nothing unknown;
 * 3. while the backend holds a capture a Stop is offered, and a window Stop
 *    and a tray Stop each lead to an idle backend shown idle;
 * 4. the backend never handed out an untrue snapshot, nor received a
 *    command the contract forbids;
 * 5. after a Stop (that completed, and that no later Start overtook)
 *    nothing runs;
 * 6. the live transcript keeps every segment of its session spoken since
 *    its Start (one delivered before any snapshot is fenced by the first);
 * 7. after every step the window offers no Stop unless the latest applied
 *    snapshot is idle, no Start or Stop of its own is unsettled and a
 *    snapshot was applied since load; and while its state is unknown,
 *    pending, running or stopping, or a Start is in flight, a read is
 *    scheduled;
 * 8. (the model) no snapshot shows an id pending or running at or below a
 *    `retired_through` handed out before it;
 * 9. (the model) after every command, delivering any earlier capture
 *    command again changes nothing the backend holds;
 * 10. after every step the window shows no session-tagged content of a
 *    session other than the applied snapshot's.
 */

function selectableSettings(): AppSettings {
  return {
    asr_provider: {
      type: "deepgram",
      model: "nova-3",
      enable_diarization: true,
    },
    whisper_model: "ggml-small.en.bin",
    llm_provider: {
      type: "openrouter",
      model: "openai/gpt-4.1-mini",
      base_url: "https://openrouter.ai/api/v1",
      include_usage_in_stream: true,
    },
    llm_api_config: null,
    audio_settings: { sample_rate: 48_000, channels: 1 },
    gemini: {
      auth: { type: "api_key" },
      model: "gemini-2.0-flash-live-001",
    },
    tts_provider: { type: "none" },
    speak_aloud: false,
    log_level: "info",
  };
}

/** The IPC of the realm loaded last. */
let net: RealmNet | null = null;

function realmNet(): RealmNet {
  if (net === null) throw new Error("no realm loaded");
  return net;
}

/** The hooks' intervals, as the fake timers schedule them. */
let intervals: {
  set: MockInstance<typeof setInterval>;
  clear: MockInstance<typeof clearInterval>;
} | null = null;

/** Whether a periodic read of the backend's state is scheduled now. */
function readScheduled(): boolean {
  if (intervals === null) throw new Error("the intervals are not tracked");
  const { set, clear } = intervals;
  const cleared = new Set(clear.mock.calls.map(([handle]) => handle));
  return set.mock.calls.some(
    ([, ms], index) =>
      ms === CAPTURE_RECONCILE_MS &&
      !cleared.has(set.mock.results[index]?.value),
  );
}

/**
 * A fresh JavaScript realm: the store's initial state (every piece of
 * capture state lives in it) and freshly mounted hooks.
 */
const productionRealm: RealmFactory = async (realm) => {
  net = realm;
  // The previous realm's intervals were cleared when it unmounted.
  intervals?.set.mockClear();
  intervals?.clear.mockClear();
  useAudioGraphStore.setState(useAudioGraphStore.getInitialState(), true);
  useAudioGraphStore.setState({
    selectedSourceIds: ["system-default"],
    settings: selectableSettings(),
  });
  const hooks = renderHook(() => {
    useTauriEvents();
    useNativeCapture();
  });
  const store = () => useAudioGraphStore.getState();
  return {
    view: () => {
      const state = store();
      const toggle = captureToggle(state);
      return {
        revision: state.pipelineStatus.revision,
        captureStatus: state.captureStatus,
        captureId: state.pipelineStatus.capture_state.id,
        isCapturing: state.isCapturing,
        isTranscribing: state.isTranscribing,
        backpressuredSources: state.backpressuredSources,
        sessionEpoch: state.pipelineStatus.session.epoch,
        unknown: state.captureUnknown,
        startInFlight: state.startPending !== null,
        startOffered: toggle.startEnabled,
        stopOffered: toggle.offersStop,
        readScheduled: readScheduled(),
        transcriptIds: state.transcriptSegments.map((segment) => segment.id),
        notesSession: state.materializedNotes?.session_id ?? null,
      };
    },
    pressStart: () => {
      void store().startCaptureAndTranscribe();
    },
    pressStop: () => {
      void store().stopCapture();
    },
    pressStopTranscription: () => {
      void store().stopTranscribe();
    },
    // What the token panel's New Session does: the rotation resets the
    // session view when its snapshot is applied, not when this replies.
    newSession: () => {
      void invoke("new_session_cmd").then(
        () => store().reconcileCapture(),
        () => {},
      );
    },
    reconcile: () => {
      void store().reconcileCapture();
    },
    unload: () => hooks.unmount(),
  };
};

/** A realm subscribes, attaches and reads the backend's state. */
const STARTUP = ["subscribe", SETTLE];
/** Start A, committed, acknowledged and transcribed, and all of it shown. */
const STARTED = [
  ...STARTUP,
  "press Start",
  "deliver start_capture",
  "commit",
  "reply start_capture",
  "deliver start_transcribe",
  "reply start_transcribe",
  SETTLE,
];

function budgets(overrides: Partial<Budgets>): Budgets {
  return { ...NO_BUDGET, ...overrides };
}

async function scripted(
  limits: Partial<Budgets>,
  script: (string | RegExp)[],
  defects: BackendDefects = {},
) {
  const failures: string[] = [];
  for (const finalStop of ["window", "tray"] as const) {
    const result = await runSchedule(
      productionRealm,
      { budgets: budgets(limits), defects },
      scriptedPick(script),
      finalStop,
    );
    failures.push(...result.failures);
  }
  return failures;
}

/** A simulation stepped by hand: `take` runs the first choice named so. */
async function manual(limits: Partial<Budgets>, defects: BackendDefects = {}) {
  const sim = new CaptureSimulation(productionRealm, budgets(limits), defects);
  await sim.loadRealm();
  const labels = () => sim.choices().map((c) => c.label);
  const take = async (label: string | RegExp) => {
    const choice = sim
      .choices()
      .find((c) =>
        typeof label === "string"
          ? c.label.startsWith(label)
          : label.test(c.label),
      );
    if (!choice) {
      throw new Error(
        `${label} is not enabled; enabled: ${labels().join(", ")}`,
      );
    }
    await sim.step(choice);
  };
  /** Take every choice matching `pattern`, oldest first, until none is left. */
  const takeAll = async (pattern: RegExp) => {
    for (let guard = 0; labels().some((l) => pattern.test(l)); guard += 1) {
      if (guard > 50) throw new Error(`${pattern} never ran out`);
      await take(pattern);
    }
  };
  /** The realm subscribes, attaches and reads the backend's state. */
  const attach = async () => {
    await take("subscribe");
    await sim.drain();
  };
  /** Start A and transcribe it, and show all of it. */
  const started = async () => {
    await attach();
    for (const label of [
      "press Start",
      "deliver start_capture",
      "commit",
      "reply start_capture",
      "deliver start_transcribe",
      "reply start_transcribe",
    ]) {
      await take(label);
    }
    await sim.drain();
  };
  return { sim, take, takeAll, labels, attach, started };
}

const report = process.env.CAPTURE_SIM_REPORT === "1";
const explored: string[] = [];
let actEnvironment: unknown;

beforeAll(() => {
  // The store changes outside React here, as it does in the app.
  actEnvironment = Reflect.get(globalThis, "IS_REACT_ACT_ENVIRONMENT");
  Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", false);
  // The hooks' periodic reads and tray refreshes would interleave with the
  // explored schedules: the scheduler decides when a status read happens.
  // Which reads are scheduled is still observed, through the fake timers.
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  intervals = {
    set: vi.spyOn(globalThis, "setInterval"),
    clear: vi.spyOn(globalThis, "clearInterval"),
  };
});

afterAll(() => {
  intervals?.set.mockRestore();
  intervals?.clear.mockRestore();
  intervals = null;
  vi.useRealTimers();
  Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", actEnvironment);
  if (report) process.stdout.write(`${explored.join("\n")}\n`);
});

beforeEach(() => {
  vi.mocked(invoke).mockImplementation((cmd, args) =>
    realmNet().invoke(cmd, args),
  );
  vi.mocked(listen).mockImplementation((name, handler) =>
    realmNet().listen(name, (event) => handler(event as never)),
  );
  // A failed attach or Stop logs; the invariants decide.
  vi.spyOn(console, "error").mockImplementation(() => {});
});

describe("capture snapshot simulation: the round-4 findings", () => {
  it("1: a reload never leaves Running shown after capture ended", async () => {
    // A's last source dies while the page reloads, so its Idle is lost; the
    // new page's attach fails; a slow thread repeats a Running(A) snapshot
    // before the status read returns Idle(A).
    const failures = await scripted(
      {
        starts: 1,
        reloads: 1,
        sourceDeaths: 1,
        attachFailures: 1,
        staleSnapshots: 1,
      },
      [
        ...STARTED,
        "reload",
        "source dies",
        "subscribe",
        "next attach fails",
        "deliver capture_attach",
        "stale snapshot",
        "deliver get_pipeline_status",
        "event pipeline-status",
        "reply capture_attach",
        "reply get_pipeline_status",
      ],
    );
    expect(failures).toEqual([]);
  });

  const resentStart = [
    ...STARTUP,
    "press Start",
    /^deliver start_capture#\d+ twice$/,
    "commit",
    "deliver start_capture",
    "reply start_capture",
  ];

  it("2: a resent Start of the live capture is the same success", async () => {
    const { sim, take, attach } = await manual({ starts: 1, duplicates: 1 });
    await attach();
    for (const step of resentStart.slice(STARTUP.length)) await take(step);
    // The resent Start's reply is the same success: nothing is refused.
    expect(useAudioGraphStore.getState().error).toBeNull();
    await sim.drain();
    const view = sim.view();
    expect(view.isCapturing).toBe(true);
    expect(view.isTranscribing).toBe(true);
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("2: catches a backend that answers the resent Start as ended", async () => {
    const failures = await scripted({ starts: 1, duplicates: 1 }, resentStart, {
      repeatedStartEndsLive: true,
    });
    // The window shows the untrue Idle the backend handed out.
    expect(
      failures.some((failure) =>
        failure.startsWith("2: after a status read the window shows"),
      ),
    ).toBe(true);
  });

  it("3: Start is unavailable until attached, and a tray Stop meanwhile holds", async () => {
    const { sim, take } = await manual({ starts: 1, trayStops: 1 });
    await take("subscribe");
    await take("deliver capture_attach");
    // The attachment's own snapshot is shown, idle, but the attachment has
    // not replied: nothing can be deferred behind it.
    await take("event pipeline-status");
    expect(sim.view().unknown).toBe(false);
    expect(sim.choices().map((c) => c.label)).not.toContain("press Start");
    await take("tray Stop");
    await take("tray task");
    await take("reply capture_attach");
    await sim.drain();
    expect(await sim.invariantFailures()).toEqual([]);
    expect(sim.choices().map((c) => c.label)).toContain("press Start");
    sim.close();
  });

  const trayStopDuringReload = [...STARTED, "reload", "tray Stop", ...STARTUP];

  it("3: the tray Stop ends capture while the window reloads", async () => {
    expect(
      await scripted(
        { starts: 1, reloads: 1, trayStops: 1 },
        trayStopDuringReload,
      ),
    ).toEqual([]);
  });

  it("3: catches a tray Stop that depends on a renderer listener", async () => {
    expect(
      await scripted(
        { starts: 1, reloads: 1, trayStops: 1 },
        trayStopDuringReload,
        { trayStopIsRendererEvent: true },
      ),
    ).toContain("5: the last action was a Stop and a capture is running");
  });

  it("4: a Stop's own reply settles it before another Start is offered, so its continuation never touches B", async () => {
    const { sim, take } = await manual({ starts: 2, stops: 1 });
    await take("subscribe");
    await sim.drain();
    for (const label of [
      "press Start",
      "deliver start_capture",
      "commit",
      "reply start_capture",
      "deliver start_transcribe",
      "reply start_transcribe",
    ]) {
      await take(label);
    }
    await sim.drain();
    await take("press Stop");
    await take("deliver stop_capture");
    // Stopping(A), then Idle(A): the Stop is still in flight, so the window
    // still offers Stop (its own outcome is not in), not Start.
    await take("event pipeline-status");
    await take("event pipeline-status");
    expect(sim.view().captureStatus).toBe("idle");
    expect(sim.view().stopOffered).toBe(true);
    expect(sim.choices().map((c) => c.label)).not.toContain("press Start");
    await take("reply stop_capture");
    await take("press Start");
    await sim.drain();
    expect(sim.view().isTranscribing).toBe(true);
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("4: A's late start reply never starts transcription for B", async () => {
    // A commits; its reply waits while A ends and another window's B starts
    // and commits. (This window offers no second Start while A's is in
    // flight: B can only come from elsewhere.)
    const { sim, take, attach } = await manual({
      starts: 1,
      sourceDeaths: 1,
      foreignStarts: 1,
    });
    await attach();
    await take("press Start");
    await take("deliver start_capture");
    await take("commit");
    await take("source dies");
    await take("another window starts");
    // A pending, running, stopping and idle; the new attachment; B pending
    // and running.
    for (let i = 0; i < 7; i += 1) await take("event pipeline-status");
    expect(sim.view().captureId).toEqual({ epoch: 2, seq: 1 });
    // A's reply: its continuation may not transcribe B.
    await take("reply start_capture");
    const transcribesAfterA = sim
      .choices()
      .filter((c) => c.label.startsWith("deliver start_transcribe"));
    expect(transcribesAfterA).toEqual([]);
    await sim.drain();
    expect(sim.view().isTranscribing).toBe(false);
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });
});

describe("capture snapshot simulation: the round-5 and re-check 4 findings", () => {
  it("a delayed commit status with ASR idle never clears transcribing", async () => {
    // The commit's Running(A) snapshot, without transcription, arrives after
    // the transcription start's own snapshot.
    const { sim, take, attach } = await manual({ starts: 1 });
    await attach();
    await take("press Start");
    await take("deliver start_capture");
    await take("commit");
    await take("reply start_capture");
    await take("deliver start_transcribe");
    await take("reply start_transcribe");
    expect(sim.view().isTranscribing).toBe(true);
    // Pending(A), Running(A) and the transcription's, oldest first.
    for (let i = 0; i < 3; i += 1) {
      await take("event pipeline-status");
      expect(sim.view().isTranscribing).toBe(true);
    }
    await sim.drain();
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("a refused Start never hides a live capture", async () => {
    // Another window starts C, and this one misses every emission: its Start
    // is refused as stale, re-attaches, retries, and is refused because C
    // runs. Each refusal's snapshot shows C.
    const { sim, take, attach } = await manual({
      starts: 1,
      foreignStarts: 1,
      drops: 3,
    });
    await attach();
    await take("another window starts");
    for (let i = 0; i < 3; i += 1) await take("drop event pipeline-status");
    expect(sim.view().startOffered).toBe(true);
    await take("press Start");
    await take("deliver start_capture");
    await take("reply start_capture");
    expect(sim.view().isCapturing).toBe(true);
    expect(sim.view().captureId).toEqual({ epoch: 2, seq: 1 });
    await sim.drain();
    const view = sim.view();
    expect(view.isCapturing).toBe(true);
    expect(view.captureId).toEqual({ epoch: 2, seq: 1 });
    expect(view.stopOffered).toBe(true);
    expect(
      useAudioGraphStore.getState().error,
      "the retry, not the stale refusal, is what the user sees",
    ).toMatch(/already running/i);
    expect(await sim.invariantFailures()).toEqual([]);
    expect(await sim.finalStop("window")).toEqual([]);
    sim.close();
  });

  it("an unreadable Start reply never hides a live capture", async () => {
    // A commits while both of its emissions are dropped; its reply arrives
    // with an outcome outside the closed set. Until a snapshot settles A
    // the state is unknown, with Stop.
    const { sim, take, attach } = await manual({
      starts: 1,
      drops: 2,
      unreadableReplies: 1,
    });
    await attach();
    await take("press Start");
    await take("deliver start_capture");
    await take("commit");
    await take("drop event pipeline-status");
    await take("drop event pipeline-status");
    await take("garble reply start_capture");
    await take("reply start_capture");
    let view = sim.view();
    expect(view.unknown).toBe(true);
    expect(view.stopOffered).toBe(true);
    expect(view.startOffered).toBe(false);
    // The read after the reply settles it: A runs.
    await sim.drain();
    view = sim.view();
    expect(view.unknown).toBe(false);
    expect(view.isCapturing).toBe(true);
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("a lost Start reply never hides a live capture", async () => {
    const failures = await scripted({ starts: 1, drops: 2, lostReplies: 1 }, [
      ...STARTUP,
      "press Start",
      "deliver start_capture",
      "commit",
      "drop event pipeline-status",
      "drop event pipeline-status",
      "lose reply start_capture",
    ]);
    expect(failures).toEqual([]);
  });

  const stopTranscriptionThenRestart = [
    ...STARTED,
    "press Stop Transcription",
    "deliver stop_transcribe",
    "source dies",
    // A's transcription stopped, Stopping(A), Idle(A): Start is offered.
    "event pipeline-status",
    "event pipeline-status",
    "event pipeline-status",
    "press Start",
    "deliver start_capture",
    "commit",
    "reply start_capture",
    "deliver start_transcribe",
    "reply start_transcribe",
  ];

  it("a late stop-transcription reply for A never clears B's transcribing", async () => {
    const failures = await scripted(
      { starts: 2, sourceDeaths: 1, stopTranscriptions: 1 },
      [...stopTranscriptionThenRestart, "reply stop_transcribe"],
    );
    expect(failures).toEqual([]);
  });

  it("a late stop-transcription delivery for A never stops B's", async () => {
    const { sim, take, started } = await manual({
      starts: 2,
      sourceDeaths: 1,
      stopTranscriptions: 1,
    });
    await started();
    for (const label of [
      "press Stop Transcription",
      "source dies",
      // Stopping(A), Idle(A): Start is offered.
      "event pipeline-status",
      "event pipeline-status",
      "press Start",
      "deliver start_capture",
      "commit",
      "reply start_capture",
      "deliver start_transcribe",
      "reply start_transcribe",
      "deliver stop_transcribe",
      "reply stop_transcribe",
    ]) {
      await take(label);
    }
    await sim.drain();
    // B's transcription runs, in the backend and in the window.
    expect(sim.backend.transcription?.capture).toEqual({ epoch: 1, seq: 2 });
    expect(sim.view().isTranscribing).toBe(true);
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("a reloaded page adopts a running capture as transcribing", async () => {
    const { sim, take, started } = await manual({ starts: 1, reloads: 1 });
    await started();
    await take("reload");
    await take("subscribe");
    await take("deliver capture_attach");
    await take("reply capture_attach");
    const view = sim.view();
    expect(view.isCapturing).toBe(true);
    expect(view.isTranscribing).toBe(true);
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("a delayed New Session success never clears the new live transcript", async () => {
    // The rotation is published at once; its reply waits while a capture
    // starts, transcribes and speaks in the new session.
    const failures = await scripted({ starts: 1, newSessions: 1, speech: 1 }, [
      ...STARTUP,
      "New Session",
      "deliver new_session_cmd",
      "event pipeline-status",
      "press Start",
      "deliver start_capture",
      "commit",
      "reply start_capture",
      "deliver start_transcribe",
      "reply start_transcribe",
      "speech",
      "event transcript-update",
      "reply new_session_cmd",
    ]);
    expect(failures).toEqual([]);
  });

  it("delayed backpressure for A never shows on B", async () => {
    const { sim, take, started } = await manual({
      starts: 2,
      sourceDeaths: 1,
      backpressures: 1,
    });
    await started();
    await take("backpressure");
    await take("source dies");
    // Idle(A), the newest, first: Start is offered while A's backpressure
    // and Stopping(A) are still in flight.
    const newest = sim
      .choices()
      .filter((c) => c.label.startsWith("event pipeline-status"))
      .at(-1);
    if (!newest) throw new Error("no emission in flight");
    await sim.step(newest);
    await take("press Start");
    await take("deliver start_capture");
    await take("commit");
    await take("reply start_capture");
    expect(sim.view().isCapturing).toBe(true);
    // A's backpressure, Stopping(A), and B's own emissions, oldest first.
    for (let i = 0; i < 4; i += 1) {
      await take("event pipeline-status");
      expect(sim.view().backpressuredSources).toEqual([]);
    }
    await sim.drain();
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });
});

describe("capture snapshot simulation: the round-7 re-check", () => {
  it("item 2: with no snapshot since load the window offers Stop and reads every few seconds, focus or not", async () => {
    // A records; the page reloads; its attach fails and both of its first
    // reads are lost; IPC recovers with no focus change.
    const { sim, take, started } = await manual({
      starts: 1,
      reloads: 1,
      attachFailures: 1,
      lostReplies: 2,
    });
    await started();
    await take("reload");
    await take("subscribe");
    await take("next attach fails");
    await take("deliver capture_attach");
    await take("deliver get_pipeline_status");
    await take("lose reply get_pipeline_status");
    await take("reply get_pipeline_status");
    // The failed attach reads once more.
    await take("reply capture_attach");
    await take("deliver get_pipeline_status");
    await take("lose reply get_pipeline_status");
    await take("reply get_pipeline_status");
    expect(sim.quiescent).toBe(true);
    let view = sim.view();
    expect(view.revision).toBe(-1);
    expect(view.unknown).toBe(true);
    expect(view.stopOffered).toBe(true);
    expect(view.readScheduled).toBe(true);
    // 30 seconds pass: the window reads again on its own.
    vi.advanceTimersByTime(30_000);
    const reads = sim
      .choices()
      .filter((c) => /^deliver get_pipeline_status#\d+$/.test(c.label));
    expect(reads.length).toBeGreaterThanOrEqual(
      Math.floor(30_000 / CAPTURE_RECONCILE_MS),
    );
    await sim.drain();
    view = sim.view();
    expect(view.isCapturing).toBe(true);
    expect(view.unknown).toBe(false);
    expect(view.stopOffered).toBe(true);
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("item 2: a Start whose promise never resolves is settled by the snapshot, as a Start rejected before reaching the backend is", async () => {
    // B registers; Stop addresses it and cancels it; the backend retires it
    // and goes idle. Start's own reply never arrives.
    const { sim, take, takeAll, labels, attach } = await manual({
      starts: 2,
      stops: 1,
    });
    await attach();
    await take("press Start");
    await take("deliver start_capture");
    await take("press Stop");
    await take("deliver stop_capture");
    await take("commit");
    await takeAll(
      /^(event pipeline-status|reply stop_capture|deliver get_pipeline_status|reply get_pipeline_status)/,
    );
    expect(labels()).toContainEqual(
      expect.stringMatching(/^reply start_capture/),
    );
    const view = sim.view();
    expect(view.captureStatus).toBe("idle");
    expect(view.startInFlight).toBe(false);
    expect(view.stopOffered).toBe(false);
    expect(view.startOffered).toBe(true);
    await sim.drain();
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("item 6: session-tagged notes that arrive before the first snapshot are dropped", async () => {
    // The reloaded page hears session-0's notes before any snapshot; the
    // backend rotates to session-1, whose snapshot is the first applied; a
    // read of session-1 follows.
    const { sim, take, attach } = await manual({
      reloads: 1,
      notes: 1,
      newSessions: 1,
    });
    await attach();
    await take("reload");
    await take("subscribe");
    await take("notes");
    await take("New Session");
    await take("deliver new_session_cmd");
    await take("event materialized-notes-update");
    expect(sim.view().notesSession).toBeNull();
    await take("event pipeline-status");
    await sim.drain();
    expect(sim.view().sessionEpoch).toBe(1);
    expect(sim.view().notesSession).toBeNull();
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("item 1: no snapshot shows pending or running an id an earlier one retired", async () => {
    expect(
      await scripted({ starts: 1, stops: 1 }, [...STARTED, "press Stop"]),
    ).toEqual([]);
  });

  it("item 1: catches a backend whose retired_through covers the live start", async () => {
    const failures = await scripted({ starts: 1 }, STARTED, {
      retiredCoversLive: true,
    });
    expect(failures).toContainEqual(
      expect.stringMatching(/^8: pending\(1,1\) shown after retired_through/),
    );
  });

  // A Stop whose reply is lost leaves the state unknown with nothing shown
  // running: the next Stop ends everything as operation 1 of epoch 1. A
  // later Start's B must survive that operation delivered again.
  const stopEverythingThenStart = [
    ...STARTED,
    "press Stop",
    "deliver stop_capture",
    "event pipeline-status",
    "event pipeline-status",
    "lose reply stop_capture",
    "reply stop_capture",
    "press Stop",
    SETTLE,
    "press Start",
    SETTLE,
  ];

  it("item 3: a repeated delivery of any earlier capture command changes nothing", async () => {
    expect(
      await scripted(
        { starts: 2, stops: 2, lostReplies: 1 },
        stopEverythingThenStart,
      ),
    ).toEqual([]);
  });

  it("item 3: catches a stop-everything without a per-epoch floor", async () => {
    const failures = await scripted(
      { starts: 2, stops: 2, lostReplies: 1 },
      stopEverythingThenStart,
      { stopEverythingWithoutFloor: true },
    );
    expect(failures).toContainEqual(
      expect.stringMatching(/^9: a repeated delivery of stop_capture/),
    );
  });

  it("a repeated delivery of a start still preparing waits for its outcome", async () => {
    const { sim, take, labels, attach } = await manual({
      starts: 1,
      duplicates: 1,
    });
    await attach();
    await take("press Start");
    await take(/^deliver start_capture#\d+ twice$/);
    // The second delivery arrives while the first prepares: it waits, with
    // no reply, until that start settles.
    await take("deliver start_capture");
    expect(labels()).not.toContainEqual(
      expect.stringMatching(/^reply start_capture/),
    );
    await take("commit");
    await take("reply start_capture");
    expect(useAudioGraphStore.getState().error).toBeNull();
    await sim.drain();
    const view = sim.view();
    expect(view.isCapturing).toBe(true);
    expect(view.isTranscribing).toBe(true);
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("a repeated delivery whose bounded wait expires answers still_preparing, and the start still commits", async () => {
    const { sim, take, attach } = await manual({ starts: 1, duplicates: 1 });
    await attach();
    await take("press Start");
    await take(/^deliver start_capture#\d+ twice$/);
    await take("deliver start_capture");
    await take("wait expires");
    await take("reply start_capture");
    expect(useAudioGraphStore.getState().error).toBe(
      i18n.t("errors.captureStart.still_preparing"),
    );
    expect(sim.view().stopOffered).toBe(true);
    await sim.drain();
    expect(sim.view().isCapturing).toBe(true);
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("a superseded page's late attach is refused and changes nothing", async () => {
    // The first page's attach is delivered twice; the page reloads, and its
    // second delivery arrives after the new page attached.
    const { sim, take, labels } = await manual({
      starts: 1,
      duplicates: 1,
      reloads: 1,
    });
    await take("subscribe");
    await take(/^deliver capture_attach#\d+ twice$/);
    await take("reload");
    await take("subscribe");
    const attaches = labels().filter((l) =>
      /^deliver capture_attach#\d+$/.test(l),
    );
    const newest = attaches.at(-1);
    if (newest === undefined) throw new Error("no attach in flight");
    await take(newest);
    const { epoch, attachedRealm } = sim.backend;
    await take("deliver capture_attach");
    expect(sim.backend.epoch).toBe(epoch);
    expect(sim.backend.attachedRealm).toBe(attachedRealm);
    await sim.drain();
    await take("press Start");
    await sim.drain();
    expect(sim.view().isCapturing).toBe(true);
    expect(useAudioGraphStore.getState().error).toBeNull();
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });

  it("a transcription start whose speech thread fails is undone, and says so", async () => {
    const { sim, take, attach } = await manual({
      starts: 1,
      transcriptionFailures: 1,
    });
    await attach();
    await take("press Start");
    await take("deliver start_capture");
    await take("commit");
    await take("reply start_capture");
    await take("next transcription start fails");
    await take("deliver start_transcribe");
    await take("reply start_transcribe");
    expect(useAudioGraphStore.getState().error).toBe(
      i18n.t("errors.transcriptionRefused.start_failed"),
    );
    await sim.drain();
    const view = sim.view();
    expect(view.isCapturing).toBe(true);
    expect(view.isTranscribing).toBe(false);
    expect(sim.backend.generations).toBe(1);
    expect(await sim.invariantFailures()).toEqual([]);
    sim.close();
  });
});

describe("capture snapshot simulation: exhaustive within small bounds", () => {
  // Each run starts attached; a reload's own startup is explored. The delay
  // bound caps how many older messages deliveries overtake in total. Each
  // scenario is bounded by `maxSchedules`, so it cannot hang: it has its own
  // time budget rather than the per-test one meant to catch hangs.
  const SCENARIO_BUDGET_MS = 120_000;
  const scenarios: Array<{
    name: string;
    limits: Partial<Budgets>;
    maxDelay: number;
  }> = [
    {
      name: "one start and a Stop",
      limits: { starts: 1, stops: 1 },
      maxDelay: 1,
    },
    {
      name: "a start delivered twice, and its source dying",
      limits: { starts: 1, duplicates: 1, sourceDeaths: 1 },
      maxDelay: 1,
    },
    {
      name: "two starts around a Stop",
      limits: { starts: 2, stops: 1 },
      maxDelay: 0,
    },
    {
      name: "a reload during a capture",
      limits: { starts: 1, reloads: 1, stops: 1 },
      maxDelay: 0,
    },
    {
      name: "a tray Stop",
      limits: { starts: 1, stops: 1, trayStops: 1 },
      maxDelay: 0,
    },
    {
      name: "a Stop whose cleanup fails, pressed again",
      limits: { starts: 1, stops: 2, cleanupFailures: 1 },
      maxDelay: 0,
    },
    {
      name: "a Stop lost in the transport, pressed again",
      limits: { starts: 1, stops: 2, stopTransportFailures: 1 },
      maxDelay: 0,
    },
    {
      name: "a reload whose attach fails",
      limits: { starts: 1, reloads: 1, attachFailures: 1 },
      maxDelay: 0,
    },
    {
      name: "a stale snapshot",
      limits: { starts: 1, stops: 1, staleSnapshots: 1 },
      maxDelay: 0,
    },
    {
      name: "New Session around a capture",
      limits: { starts: 1, stops: 1, newSessions: 1 },
      maxDelay: 0,
    },
    {
      name: "a dropped emission",
      limits: { starts: 1, stops: 1, drops: 1 },
      maxDelay: 0,
    },
    {
      name: "a lost reply",
      limits: { starts: 1, stops: 1, lostReplies: 1 },
      maxDelay: 0,
    },
    {
      name: "an unreadable reply",
      limits: { starts: 1, stops: 1, unreadableReplies: 1 },
      maxDelay: 0,
    },
    {
      name: "another window's start",
      limits: { starts: 1, foreignStarts: 1, drops: 1 },
      maxDelay: 0,
    },
    {
      name: "Stop Transcription and backpressure",
      limits: { starts: 1, stopTranscriptions: 1, backpressures: 1 },
      maxDelay: 0,
    },
    {
      name: "a transcription start that fails",
      limits: { starts: 1, stops: 1, transcriptionFailures: 1 },
      maxDelay: 0,
    },
    {
      name: "notes around a reload",
      limits: { reloads: 1, notes: 1, starts: 1 },
      maxDelay: 0,
    },
  ];

  for (const scenario of scenarios) {
    it(
      `holds every invariant: ${scenario.name}`,
      async () => {
        const failing: string[] = [];
        const schedules = await exploreExhaustively(
          productionRealm,
          {
            budgets: budgets(scenario.limits),
            startAttached: true,
            maxDelay: scenario.maxDelay,
            maxSchedules: 3_000,
          },
          (result) => {
            if (result.failures.length > 0 && failing.length < 3) {
              failing.push(
                `${result.failures.join("; ")}\n  after: ${result.trace.join(" → ")}`,
              );
            }
          },
        );
        explored.push(
          `exhaustive "${scenario.name}" (delay ≤ ${scenario.maxDelay}): ${schedules} schedules`,
        );
        expect(failing).toEqual([]);
        expect(schedules).toBeGreaterThan(1);
      },
      SCENARIO_BUDGET_MS,
    );
  }
});

describe("capture snapshot simulation: seeded random beyond the bounds", () => {
  const limits = budgets({
    starts: 3,
    stops: 2,
    reloads: 1,
    trayStops: 1,
    sourceDeaths: 1,
    newSessions: 1,
    cleanupFailures: 1,
    stopTransportFailures: 1,
    attachFailures: 1,
    transcriptionFailures: 1,
    staleSnapshots: 2,
    duplicates: 2,
    drops: 2,
    lostReplies: 1,
    unreadableReplies: 1,
    reconciles: 2,
    speech: 2,
    notes: 2,
    backpressures: 2,
    foreignStarts: 1,
    stopTranscriptions: 1,
  });
  const SEEDS = 20;
  const RUNS_PER_SEED = 25;
  // Most runs go on past their first quiet point.
  const END_CHANCE = 0.1;
  const SEED_BUDGET_MS = 120_000;

  for (let seed = 1; seed <= SEEDS; seed += 1) {
    it(
      `holds every invariant: seed ${seed}`,
      async () => {
        const random = seededRandom(seed);
        const failing: string[] = [];
        for (let run = 0; run < RUNS_PER_SEED; run += 1) {
          const result = await runSchedule(
            productionRealm,
            { budgets: limits },
            randomPick(random, END_CHANCE),
            run % 2 === 0 ? "window" : "tray",
          );
          if (result.failures.length > 0 && failing.length < 3) {
            failing.push(
              `${result.failures.join("; ")}\n  after: ${result.trace.join(" → ")}`,
            );
          }
        }
        explored.push(`random seed ${seed}: ${RUNS_PER_SEED} schedules`);
        expect(failing).toEqual([]);
      },
      SEED_BUDGET_MS,
    );
  }
});
