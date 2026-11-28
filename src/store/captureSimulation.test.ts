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
 * Amendment H (#141): the production store and hooks against a model
 * backend, over every delivery order within small bounds and seeded random
 * orders beyond them. Invariants, checked once every message is delivered:
 * 1. the renderer shows capturing exactly when the backend has a live
 *    capture, with the same id;
 * 2. it shows transcribing exactly when the backend transcribes that id;
 * 3. while the backend holds a capture a Stop is offered, and a window Stop
 *    and a tray Stop each lead to an idle backend;
 * 4. the backend never stated Ended(id) while id was live;
 * 5. after a Stop (that ran, and that no later Start overtook) nothing runs.
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
  return {
    view: () => {
      const state = useAudioGraphStore.getState();
      const toggle = captureToggle(state);
      return {
        isCapturing: state.isCapturing,
        captureStarting: state.captureStarting,
        isTranscribing: state.isTranscribing,
        captureId: state.captureFact?.id ?? null,
        transcribingId: state.isTranscribing ? state.captureTranscribing : null,
        startOffered: toggle.startEnabled,
        stopOffered: toggle.offersStop,
      };
    },
    pressStart: () => {
      void useAudioGraphStore.getState().startCaptureAndTranscribe();
    },
    pressStop: () => {
      void useAudioGraphStore.getState().stopCapture();
    },
    // What the token panel's New Session does.
    newSession: () => {
      void invoke("new_session_cmd").then(
        () => useAudioGraphStore.getState().resetSessionView(),
        () => {},
      );
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
  "event pipeline-status",
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
});

afterAll(() => {
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

describe("capture protocol simulation: the round-4 findings", () => {
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

  it("2: catches a backend that answers the resent Start with Ended", async () => {
    expect(
      await scripted({ starts: 1, duplicates: 1 }, resentStart, {
        repeatedStartEndsLive: true,
      }),
    ).toContain("4: Ended(1,1) while it was live");
  });

  it("3: Start is unavailable until attached, and a tray Stop meanwhile holds", async () => {
    const sim = new CaptureSimulation(
      productionRealm,
      budgets({ starts: 1, trayStops: 1 }),
    );
    await sim.loadRealm();
    const take = async (label: string) => {
      const choice = sim.choices().find((c) => c.label.startsWith(label));
      if (!choice) throw new Error(`${label} is not enabled`);
      await sim.step(choice);
    };
    await take("subscribe");
    await take("deliver capture_attach");
    // The attachment has not replied: nothing can be deferred behind it.
    expect(sim.choices().map((c) => c.label)).not.toContain("press Start");
    await take("tray Stop");
    await take("tray task");
    await take("reply capture_attach");
    await sim.drain();
    expect(sim.invariantFailures()).toEqual([]);
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
      "deliver start_transcribe",
      "reply start_transcribe",
      "press Stop",
      "deliver stop_capture",
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
    // A commits; its reply waits while A ends and B starts and commits.
    const sim = new CaptureSimulation(
      productionRealm,
      budgets({ starts: 2, sourceDeaths: 1 }),
    );
    await sim.loadRealm();
    const take = async (label: string) => {
      const choice = sim.choices().find((c) => c.label.startsWith(label));
      if (!choice) throw new Error(`${label} is not enabled`);
      await sim.step(choice);
    };
    for (const label of STARTUP) await take(label);
    await take("press Start");
    await take("deliver start_capture");
    await take("commit");
    await take("source dies");
    // Running(A), then Idle(A): A has ended.
    await take("event pipeline-status");
    await take("event pipeline-status");
    await take("press Start");
    await take("deliver start_capture");
    await take("commit");
    // A's reply, then B's: only B's continuation may transcribe B.
    const replies = sim
      .choices()
      .filter((c) => c.label.startsWith("reply start_capture"));
    expect(replies).toHaveLength(2);
    await sim.step(replies[0]);
    const transcribesAfterA = sim
      .choices()
      .filter((c) => c.label.startsWith("deliver start_transcribe"));
    expect(transcribesAfterA).toEqual([]);
    await sim.drain();
    expect(sim.invariantFailures()).toEqual([]);
    sim.close();
  });
});

describe("capture protocol simulation: exhaustive within small bounds", () => {
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
      maxDelay: 2,
    },
    {
      name: "a start delivered twice, and its source dying",
      limits: { starts: 1, duplicates: 1, sourceDeaths: 1 },
      maxDelay: 2,
    },
    {
      name: "two starts around a Stop",
      limits: { starts: 2, stops: 1 },
      maxDelay: 1,
    },
    {
      name: "a reload during a capture",
      limits: { starts: 1, reloads: 1, stops: 1 },
      maxDelay: 1,
    },
    {
      name: "a tray Stop",
      limits: { starts: 1, stops: 1, trayStops: 1 },
      maxDelay: 1,
    },
    {
      name: "a failing Stop, pressed again",
      limits: { starts: 1, stops: 2, stopFailures: 1 },
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
      maxDelay: 1,
    },
    {
      name: "New Session around a capture",
      limits: { starts: 1, stops: 1, newSessions: 1 },
      maxDelay: 1,
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

describe("capture protocol simulation: seeded random beyond the bounds", () => {
  const limits = budgets({
    starts: 3,
    stops: 2,
    reloads: 1,
    trayStops: 1,
    sourceDeaths: 1,
    newSessions: 1,
    stopFailures: 1,
    attachFailures: 1,
    staleSnapshots: 2,
    duplicates: 2,
  });
  const SEEDS = 20;
  const RUNS_PER_SEED = 25;

  for (let seed = 1; seed <= SEEDS; seed += 1) {
    it(`holds every invariant: seed ${seed}`, async () => {
      const random = seededRandom(seed);
      const failing: string[] = [];
      for (let run = 0; run < RUNS_PER_SEED; run += 1) {
        const result = await runSchedule(
          productionRealm,
          { budgets: limits },
          randomPick(random),
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
    });
  }
});
