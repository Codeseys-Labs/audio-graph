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
  vi,
} from "vitest";
import { useNativeCapture } from "../hooks/useNativeCapture";
import { useTauriEvents } from "../hooks/useTauriEvents";
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
 * 4. the backend never handed out an untrue snapshot;
 * 5. after a Stop (that completed, and that no later Start overtook)
 *    nothing runs;
 * 6. the live transcript keeps every segment of its session spoken since
 *    its Start.
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

/**
 * A fresh JavaScript realm: the store's initial state (every piece of
 * capture state lives in it) and freshly mounted hooks.
 */
const productionRealm: RealmFactory = async (realm) => {
  net = realm;
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
        transcriptIds: state.transcriptSegments.map((segment) => segment.id),
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

const STARTUP = [
  "subscribe",
  "deliver capture_attach",
  "reply capture_attach",
  "deliver get_pipeline_status",
  "reply get_pipeline_status",
];
/** Start A, acknowledged and announced. */
const STARTED = [
  ...STARTUP,
  "press Start",
  "deliver start_capture",
  "commit",
  "reply start_capture",
  "deliver start_transcribe",
  "reply start_transcribe",
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

const report = process.env.CAPTURE_SIM_REPORT === "1";
const explored: string[] = [];
let actEnvironment: unknown;

beforeAll(() => {
  // The store changes outside React here, as it does in the app.
  actEnvironment = Reflect.get(globalThis, "IS_REACT_ACT_ENVIRONMENT");
  Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", false);
  // The hooks' periodic reads and tray refreshes would interleave with the
  // explored schedules: the scheduler decides when a status read happens.
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
});

afterAll(() => {
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
    expect(await scripted({ starts: 1, duplicates: 1 }, resentStart)).toEqual(
      [],
    );
  });

  it("2: catches a backend that answers the resent Start as ended", async () => {
    expect(
      await scripted({ starts: 1, duplicates: 1 }, resentStart, {
        repeatedStartEndsLive: true,
      }),
    ).toContain("4: idle(1,1) stated while it was running");
  });

  it("3: Start is unavailable until attached, and a tray Stop meanwhile holds", async () => {
    const { sim, take } = await manual({ starts: 1, trayStops: 1 });
    await take("subscribe");
    await take("deliver capture_attach");
    // The attachment has not replied: nothing can be deferred behind it.
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

  it("4: a continuation of A acts on A only, not on its replacement B", async () => {
    // A's Stop reply arrives after B started and began transcribing.
    const failures = await scripted({ starts: 2, stops: 1 }, [
      ...STARTED,
      "press Stop",
      "deliver stop_capture",
      // Stopping(A), then Idle(A): Start is offered again.
      "event pipeline-status",
      "event pipeline-status",
      "press Start",
      "deliver start_capture",
      "commit",
      "reply start_capture",
      "deliver start_transcribe",
      "reply start_transcribe",
      "reply stop_capture",
    ]);
    expect(failures).toEqual([]);
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
    // unreadable. Until a snapshot settles A the state is unknown, with Stop.
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
    const failures = await scripted(
      { starts: 1, drops: 2, lostReplies: 1 },
      [
        ...STARTUP,
        "press Start",
        "deliver start_capture",
        "commit",
        "drop event pipeline-status",
        "drop event pipeline-status",
        "lose reply start_capture",
      ],
    );
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
    const failures = await scripted(
      { starts: 2, sourceDeaths: 1, stopTranscriptions: 1 },
      [
        ...STARTED,
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
      ],
    );
    expect(failures).toEqual([]);
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
    staleSnapshots: 2,
    duplicates: 2,
    drops: 2,
    lostReplies: 1,
    unreadableReplies: 1,
    reconciles: 2,
    speech: 2,
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
