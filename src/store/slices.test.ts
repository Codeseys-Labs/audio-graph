import { beforeEach, describe, expect, it, vi } from "vitest";
import { THEME_STORAGE_KEY } from "../theme";
import { useAudioGraphStore } from "./index";

describe("store: notifications slice", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({ notifications: [] });
  });

  it("notify pushes a notification and returns a generated id", () => {
    const id = useAudioGraphStore.getState().notify({ message: "hello" });
    const list = useAudioGraphStore.getState().notifications;
    expect(list).toHaveLength(1);
    expect(list[0].id).toBe(id);
    expect(list[0].message).toBe("hello");
    // Default severity is info.
    expect(list[0].severity).toBe("info");
    expect(typeof list[0].createdAt).toBe("number");
  });

  it("notify honours an explicit id and severity", () => {
    const id = useAudioGraphStore.getState().notify({
      id: "fixed",
      severity: "error",
      message: "boom",
      sticky: true,
    });
    expect(id).toBe("fixed");
    const n = useAudioGraphStore.getState().notifications[0];
    expect(n.severity).toBe("error");
    expect(n.sticky).toBe(true);
  });

  it("notify appends newest-last (stack order)", () => {
    const store = useAudioGraphStore.getState();
    store.notify({ id: "a", message: "first" });
    store.notify({ id: "b", message: "second" });
    expect(
      useAudioGraphStore.getState().notifications.map((n) => n.id),
    ).toEqual(["a", "b"]);
  });

  it("dismissNotification removes only the matching id", () => {
    const store = useAudioGraphStore.getState();
    store.notify({ id: "a", message: "first" });
    store.notify({ id: "b", message: "second" });
    store.dismissNotification("a");
    expect(
      useAudioGraphStore.getState().notifications.map((n) => n.id),
    ).toEqual(["b"]);
  });

  it("clearNotifications empties the queue", () => {
    const store = useAudioGraphStore.getState();
    store.notify({ message: "first" });
    store.notify({ message: "second" });
    store.clearNotifications();
    expect(useAudioGraphStore.getState().notifications).toEqual([]);
  });
});

describe("store: speakers slice (addOrUpdateSpeaker upsert)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({ speakers: [] });
  });

  const speaker = (id: string, segment_count: number) => ({
    id,
    label: id,
    color: "#60a5fa",
    total_speaking_time: segment_count,
    segment_count,
  });

  it("adds a new speaker", () => {
    useAudioGraphStore.getState().addOrUpdateSpeaker(speaker("s1", 1));
    expect(useAudioGraphStore.getState().speakers).toHaveLength(1);
  });

  it("updates an existing speaker in place (no duplicate)", () => {
    const store = useAudioGraphStore.getState();
    store.addOrUpdateSpeaker(speaker("s1", 1));
    store.addOrUpdateSpeaker(speaker("s1", 9));
    const speakers = useAudioGraphStore.getState().speakers;
    expect(speakers).toHaveLength(1);
    expect(speakers[0].segment_count).toBe(9);
  });

  it("preserves order and appends new speakers after existing ones", () => {
    const store = useAudioGraphStore.getState();
    store.addOrUpdateSpeaker(speaker("s1", 1));
    store.addOrUpdateSpeaker(speaker("s2", 1));
    store.addOrUpdateSpeaker(speaker("s1", 5));
    expect(useAudioGraphStore.getState().speakers.map((s) => s.id)).toEqual([
      "s1",
      "s2",
    ]);
  });

  it("clearSpeakers empties the list", () => {
    useAudioGraphStore.getState().addOrUpdateSpeaker(speaker("s1", 1));
    useAudioGraphStore.getState().clearSpeakers();
    expect(useAudioGraphStore.getState().speakers).toEqual([]);
  });
});

describe("store: source backpressure slice", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({ backpressuredSources: [] });
  });

  it("adds a source when newly backpressured", () => {
    useAudioGraphStore.getState().setSourceBackpressure("src-1", true);
    expect(useAudioGraphStore.getState().backpressuredSources).toEqual([
      "src-1",
    ]);
  });

  it("does not duplicate an already-backpressured source", () => {
    const store = useAudioGraphStore.getState();
    store.setSourceBackpressure("src-1", true);
    store.setSourceBackpressure("src-1", true);
    expect(useAudioGraphStore.getState().backpressuredSources).toEqual([
      "src-1",
    ]);
  });

  it("removes a source when backpressure clears", () => {
    const store = useAudioGraphStore.getState();
    store.setSourceBackpressure("src-1", true);
    store.setSourceBackpressure("src-1", false);
    expect(useAudioGraphStore.getState().backpressuredSources).toEqual([]);
  });

  it("clearing an absent source is a no-op", () => {
    useAudioGraphStore.getState().setSourceBackpressure("ghost", false);
    expect(useAudioGraphStore.getState().backpressuredSources).toEqual([]);
  });
});

describe("store: persistence queue backpressure slice", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({ persistenceQueueBackpressure: {} });
  });

  it("tracks a writer while its persistence queue is backpressured", () => {
    useAudioGraphStore.getState().setPersistenceQueueBackpressure({
      writer: "transcript_event",
      is_backpressured: true,
      queue_capacity: 2048,
      dropped_count: 3,
    });

    expect(
      useAudioGraphStore.getState().persistenceQueueBackpressure,
    ).toMatchObject({
      transcript_event: {
        writer: "transcript_event",
        queue_capacity: 2048,
        dropped_count: 3,
      },
    });
  });

  it("replaces a writer snapshot instead of duplicating it", () => {
    const store = useAudioGraphStore.getState();
    store.setPersistenceQueueBackpressure({
      writer: "projection_event",
      is_backpressured: true,
      queue_capacity: 2048,
      dropped_count: 1,
    });
    store.setPersistenceQueueBackpressure({
      writer: "projection_event",
      is_backpressured: true,
      queue_capacity: 2048,
      dropped_count: 4,
    });

    expect(
      Object.keys(useAudioGraphStore.getState().persistenceQueueBackpressure),
    ).toEqual(["projection_event"]);
    expect(
      useAudioGraphStore.getState().persistenceQueueBackpressure
        .projection_event.dropped_count,
    ).toBe(4);
  });

  it("clears a writer when queue pressure recovers", () => {
    const store = useAudioGraphStore.getState();
    store.setPersistenceQueueBackpressure({
      writer: "transcript_event",
      is_backpressured: true,
      queue_capacity: 2048,
      dropped_count: 3,
    });
    store.setPersistenceQueueBackpressure({
      writer: "transcript_event",
      is_backpressured: false,
      queue_capacity: 2048,
      dropped_count: 3,
    });

    expect(useAudioGraphStore.getState().persistenceQueueBackpressure).toEqual(
      {},
    );
  });
});

describe("store: theme slice (setTheme persists + reflects)", () => {
  beforeEach(() => {
    localStorage.clear();
    delete document.documentElement.dataset.theme;
  });

  it("setTheme persists the choice to localStorage", () => {
    useAudioGraphStore.getState().setTheme("dark");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");
    expect(useAudioGraphStore.getState().theme).toBe("dark");
  });

  it("setTheme to an explicit value sets data-theme on the document root", () => {
    useAudioGraphStore.getState().setTheme("light");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("setTheme to system removes the data-theme attribute", () => {
    useAudioGraphStore.getState().setTheme("dark");
    useAudioGraphStore.getState().setTheme("system");
    expect(document.documentElement.dataset.theme).toBeUndefined();
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("system");
  });
});

describe("store: conversation-mode + converse-engine setters", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("setConversationMode persists the mode", () => {
    useAudioGraphStore.getState().setConversationMode("converse");
    expect(localStorage.getItem("ag.conversationMode")).toBe("converse");
    expect(useAudioGraphStore.getState().conversationMode).toBe("converse");
  });

  it("setConverseEngine to native + converse mode flips the legacy native-S2S flag on", () => {
    useAudioGraphStore.setState({ conversationMode: "converse" });
    useAudioGraphStore.getState().setConverseEngine("native");
    expect(localStorage.getItem("ag.converseEngine")).toBe("native");
    expect(localStorage.getItem("ag.nativeS2sEnabled")).toBe("true");
    expect(useAudioGraphStore.getState().converseEngine).toBe("native");
    expect(useAudioGraphStore.getState().nativeS2sEnabled).toBe(true);
  });

  it("setConverseEngine native while in notes mode keeps the legacy flag off", () => {
    useAudioGraphStore.setState({ conversationMode: "notes" });
    useAudioGraphStore.getState().setConverseEngine("native");
    expect(localStorage.getItem("ag.nativeS2sEnabled")).toBe("false");
    expect(useAudioGraphStore.getState().nativeS2sEnabled).toBe(false);
  });

  it("setConverseEngine pipelined turns the legacy flag off", () => {
    useAudioGraphStore.setState({ conversationMode: "converse" });
    useAudioGraphStore.getState().setConverseEngine("pipelined");
    expect(localStorage.getItem("ag.nativeS2sEnabled")).toBe("false");
    expect(useAudioGraphStore.getState().converseEngine).toBe("pipelined");
  });

  it("setNativeS2sEnabled persists the flag", () => {
    useAudioGraphStore.getState().setNativeS2sEnabled(true);
    expect(localStorage.getItem("ag.nativeS2sEnabled")).toBe("true");
    expect(useAudioGraphStore.getState().nativeS2sEnabled).toBe(true);
  });
});

// The conversationMode/converseEngine initial values are computed at module
// init from localStorage, migrating from the legacy `ag.nativeS2sEnabled`
// flag. Re-import the module under different localStorage states to exercise
// those migration branches.
describe("store: conversation-mode / converse-engine migration on init", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.resetModules();
  });

  it("defaults to notes + pipelined with no stored prefs", async () => {
    const mod = await import("./index");
    const s = mod.useAudioGraphStore.getState();
    expect(s.conversationMode).toBe("notes");
    expect(s.converseEngine).toBe("pipelined");
  });

  it("migrates a native-S2S user to converse + native", async () => {
    localStorage.setItem("ag.nativeS2sEnabled", "true");
    const mod = await import("./index");
    const s = mod.useAudioGraphStore.getState();
    expect(s.conversationMode).toBe("converse");
    expect(s.converseEngine).toBe("native");
    expect(s.nativeS2sEnabled).toBe(true);
  });

  it("honours an explicit stored conversationMode/converseEngine", async () => {
    localStorage.setItem("ag.conversationMode", "converse");
    localStorage.setItem("ag.converseEngine", "pipelined");
    const mod = await import("./index");
    const s = mod.useAudioGraphStore.getState();
    expect(s.conversationMode).toBe("converse");
    expect(s.converseEngine).toBe("pipelined");
  });
});

describe("store: pipeline latency slice", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({ pipelineLatencies: {} });
  });

  it("records a latency sample keyed by stage", () => {
    useAudioGraphStore.getState().setPipelineLatency({
      stage: "asr",
      latency_ms: 42,
      timestamp_ms: 1,
    });
    expect(
      useAudioGraphStore.getState().pipelineLatencies.asr?.latency_ms,
    ).toBe(42);
  });

  it("overwrites the previous sample for the same stage", () => {
    const store = useAudioGraphStore.getState();
    store.setPipelineLatency({
      stage: "graph",
      latency_ms: 1,
      timestamp_ms: 1,
    });
    store.setPipelineLatency({
      stage: "graph",
      latency_ms: 9,
      timestamp_ms: 2,
    });
    expect(
      useAudioGraphStore.getState().pipelineLatencies.graph?.latency_ms,
    ).toBe(9);
  });
});
