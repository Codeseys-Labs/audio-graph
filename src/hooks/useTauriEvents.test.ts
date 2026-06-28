import type { Event } from "@tauri-apps/api/event";
import { listen } from "@tauri-apps/api/event";
import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import type { AwsErrorPayload, GeminiErrorCategory } from "../types";
import {
  awsErrorToMessage,
  routeGeminiError,
  useTauriEvents,
} from "./useTauriEvents";

// The global setup (src/test/setup.ts) already mocks @tauri-apps/api/event
// with a `listen` that returns a no-op unlisten. Here we redefine its
// behavior per-test so we can capture handlers and assert payload routing.
type Handler = (event: Event<unknown>) => void;

function makeEvent<T>(name: string, payload: T): Event<T> {
  return { event: name, id: 0, payload } as Event<T>;
}

function resetStore() {
  useAudioGraphStore.setState({
    transcriptSegments: [],
    graphSnapshot: {
      nodes: [],
      links: [],
      stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
    },
    pipelineStatus: {
      capture: { type: "Idle" },
      pipeline: { type: "Idle" },
      asr: { type: "Idle" },
      diarization: { type: "Idle" },
      entity_extraction: { type: "Idle" },
      graph: { type: "Idle" },
    },
    pipelineLatencies: {},
    latestAudioConsumerHealth: null,
    asrPartial: null,
    asrSpanRevisions: [],
    diarizationSpanRevisions: [],
    sessionProjectionEvents: [],
    materializedNotes: null,
    materializedProjectionGraph: null,
    turnEvents: [],
    agentStatus: null,
    agentProposals: [],
    speakers: [],
    backpressuredSources: [],
    persistenceQueueBackpressure: {},
    geminiTranscripts: [],
    error: null,
    notifications: [],
    isGeminiActive: true,
  });
}

describe("useTauriEvents", () => {
  const handlers = new Map<string, Handler>();
  const unlisteners: Array<ReturnType<typeof vi.fn>> = [];

  beforeEach(() => {
    handlers.clear();
    unlisteners.length = 0;
    resetStore();

    vi.mocked(listen).mockImplementation(
      async (eventName: string, cb: Handler) => {
        handlers.set(eventName, cb);
        const unlisten = vi.fn();
        unlisteners.push(unlisten);
        return unlisten;
      },
    );
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  // The hook's setup() runs `TOTAL_LISTENERS` listen() calls concurrently
  // via Promise.all; waitFor polls until all handlers are present.
  //
  // When adding a new listener, update both this constant and the
  // `expected` list in the "subscribes to all expected events on mount"
  // test. The count is also exercised by the unlisten-cleanup test and
  // the partial-failure test (which drops exactly one).
  const TOTAL_LISTENERS = 29;
  async function waitForAllHandlers() {
    await waitFor(() => {
      expect(handlers.size).toBe(TOTAL_LISTENERS);
    });
  }

  it("subscribes to all expected events on mount", async () => {
    const { unmount } = renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    const expected = [
      "transcript-update",
      "asr-partial",
      "asr-span-revision",
      "diarization-span-revision",
      "turn-event",
      "agent-status",
      "agent-proposal",
      "graph-update",
      "graph-delta",
      "projection-patch",
      "materialized-notes-update",
      "materialized-graph-update",
      "pipeline-status",
      "audio-consumer-health",
      "pipeline-latency",
      "speaker-detected",
      "capture-error",
      "capture-backpressure",
      "persistence-queue-backpressure",
      "capture-storage-full",
      "model-download-progress",
      "gemini-transcription",
      "gemini-response",
      "gemini-status",
      "openai-realtime-response",
      "openai-realtime-status",
      "aws-error",
      // Streaming chat (plan A3 / ADR-0006).
      "chat-token-delta",
      "chat-token-done",
    ];
    for (const name of expected) {
      expect(handlers.has(name)).toBe(true);
    }
    expect(handlers.size).toBe(expected.length);
    unmount();
  });

  it("invokes every registered unlisten on unmount", async () => {
    const { unmount } = renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    const count = unlisteners.length;
    expect(count).toBe(TOTAL_LISTENERS);
    unmount();

    for (const fn of unlisteners) {
      expect(fn).toHaveBeenCalledTimes(1);
    }
  });

  it("routes transcript-update payload into the store", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    const segment = {
      id: "seg-1",
      speaker_id: "spk-1",
      text: "hello",
      start_time: 0,
      end_time: 1,
      confidence: 0.9,
    };
    handlers.get("transcript-update")?.(
      makeEvent("transcript-update", segment),
    );

    expect(useAudioGraphStore.getState().transcriptSegments).toEqual([segment]);
  });

  it("routes asr-partial payload into the store", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("asr-partial")?.(
      makeEvent("asr-partial", {
        provider: "deepgram",
        source_id: "system-default",
        text: "hel",
        start_time: 0,
        end_time: 0.5,
        confidence: 0.7,
        timestamp_ms: 1_700_000_000_000,
      }),
    );

    // asr-partial is throttled (latest-wins, ~100ms); poll until it flushes.
    await waitFor(() => {
      expect(useAudioGraphStore.getState().asrPartial).toMatchObject({
        provider: "deepgram",
        source_id: "system-default",
        text: "hel",
      });
    });
  });

  it("routes asr-span-revision payload into the store", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    const revision = {
      span_id: "deepgram:system-default:0-500",
      provider: "deepgram",
      source_id: "system-default",
      provider_item_id: null,
      transcript_segment_id: null,
      speaker_id: "speaker-0",
      speaker_label: "Speaker 0",
      channel: null,
      text: "hello",
      start_time: 0,
      end_time: 0.5,
      confidence: 0.7,
      is_final: false,
      stability: "partial",
      revision_number: 1,
      supersedes: null,
      turn_id: null,
      end_of_turn: false,
      raw_event_ref: null,
      received_at_ms: 1_700_000_000_000,
    };

    handlers.get("asr-span-revision")?.(
      makeEvent("asr-span-revision", revision),
    );

    expect(useAudioGraphStore.getState().asrSpanRevisions).toEqual([revision]);
  });

  it("routes diarization-span-revision payload into the store", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    const revision = {
      span_id: "local_clustering:session:0-500:speaker-c-0",
      provider: "local_clustering",
      timeline_id: "session",
      source_id: null,
      speaker_id: "speaker-c-0",
      speaker_label: "Speaker 1",
      channel: null,
      start_time: 0,
      end_time: 0.5,
      confidence: null,
      is_final: false,
      stability: "provisional",
      revision_number: 1,
      supersedes: null,
      basis_asr_span_ids: [],
      basis_transcript_segment_ids: [],
      raw_event_ref: "window_start_sample:0",
      received_at_ms: 1_700_000_000_000,
    };

    handlers.get("diarization-span-revision")?.(
      makeEvent("diarization-span-revision", revision),
    );

    expect(useAudioGraphStore.getState().diarizationSpanRevisions).toEqual([
      revision,
    ]);
  });

  it("routes turn-event payload into the store", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("turn-event")?.(
      makeEvent("turn-event", {
        provider: "deepgram",
        source_id: "system-default",
        kind: "end_of_turn",
        text: "hello world",
        start_time: 0,
        end_time: 1.2,
        confidence: 0.91,
        turn_index: 3,
        timestamp_ms: 1_700_000_000_100,
      }),
    );

    expect(useAudioGraphStore.getState().turnEvents).toEqual([
      expect.objectContaining({
        provider: "deepgram",
        source_id: "system-default",
        kind: "end_of_turn",
      }),
    ]);
  });

  it("routes agent status and proposal payloads into the store", async () => {
    useAudioGraphStore.setState({ notifications: [] });
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("agent-status")?.(
      makeEvent("agent-status", {
        state: "running",
        source_segment_id: "seg-1",
        message: "Reviewing transcript segment",
        timestamp_ms: 1_700_000_000_000,
      }),
    );
    expect(useAudioGraphStore.getState().agentStatus).toMatchObject({
      state: "running",
      source_segment_id: "seg-1",
    });

    handlers.get("agent-proposal")?.(
      makeEvent("agent-proposal", {
        id: "proposal-1",
        source_segment_id: "seg-1",
        source_id: "system-default",
        speaker_label: "Speaker 1",
        kind: "question",
        title: "Question from Speaker 1",
        body: "Consider answering or linking this question: What changed?",
        confidence: 0.87,
        created_at_ms: 1_700_000_000_100,
      }),
    );

    expect(useAudioGraphStore.getState().agentProposals).toHaveLength(1);
    expect(useAudioGraphStore.getState().agentProposals[0]).toMatchObject({
      id: "proposal-1",
      kind: "question",
    });
    expect(useAudioGraphStore.getState().notifications).toContainEqual(
      expect.objectContaining({
        severity: "info",
        message: "Question from Speaker 1",
      }),
    );
  });

  it("applies graph-delta payloads to the graph snapshot", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("graph-delta")?.(
      makeEvent("graph-delta", {
        added_nodes: [
          {
            id: "node-1",
            name: "Alice",
            entity_type: "Person",
            val: 3,
            color: "#4ade80",
            first_seen: 0,
            last_seen: 1,
            mention_count: 1,
          },
          {
            id: "node-2",
            name: "Acme",
            entity_type: "Organization",
            val: 3,
            color: "#60a5fa",
            first_seen: 0,
            last_seen: 1,
            mention_count: 1,
          },
        ],
        updated_nodes: [],
        added_edges: [
          {
            id: "edge-1",
            source: "node-1",
            target: "node-2",
            relation_type: "WORKS_AT",
            weight: 1,
            color: "#999999",
            label: "WORKS_AT",
          },
        ],
        removed_node_ids: [],
        removed_edge_ids: [],
        timestamp: 1,
      }),
    );

    expect(useAudioGraphStore.getState().graphSnapshot.nodes).toHaveLength(2);
    expect(useAudioGraphStore.getState().graphSnapshot.links).toContainEqual(
      expect.objectContaining({ id: "edge-1", relation_type: "WORKS_AT" }),
    );
    expect(useAudioGraphStore.getState().graphSnapshot.stats).toMatchObject({
      total_nodes: 2,
      total_edges: 1,
    });
  });

  it("routes live projection patch and materialized artifacts into the store", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    const patch = {
      sequence: 1,
      kind: "notes",
      llm_request_id: "llm-live-1",
      basis: { transcript_hash: "fnv1a64:live" },
      operations: [],
      confidence: 0.9,
      provenance: {
        provider: "test",
        model: "projection-test",
        prompt_id: "projection_patch_v1_test",
      },
      created_at_ms: 1_700_000_000_001,
    };
    const notes = {
      schema_version: 1,
      session_id: "session-live",
      last_sequence: 1,
      notes: [
        {
          id: "note-live",
          title: "Live note",
          body: "Live body",
          tags: [],
          updated_by_sequence: 1,
          updated_at_ms: 1_700_000_000_001,
          basis: { transcript_hash: "fnv1a64:live" },
          provenance: {
            provider: "test",
            model: "projection-test",
            prompt_id: "projection_patch_v1_test",
          },
        },
      ],
    };
    const graph = {
      schema_version: 1,
      session_id: "session-live",
      last_sequence: 2,
      nodes: [{ id: "node-live", name: "Live node" }],
      edges: [],
    };

    handlers.get("projection-patch")?.(makeEvent("projection-patch", patch));
    handlers.get("materialized-notes-update")?.(
      makeEvent("materialized-notes-update", notes),
    );
    handlers.get("materialized-graph-update")?.(
      makeEvent("materialized-graph-update", graph),
    );

    const state = useAudioGraphStore.getState();
    expect(state.sessionProjectionEvents).toEqual([patch]);
    expect(state.materializedNotes).toEqual(notes);
    expect(state.materializedProjectionGraph).toEqual(graph);
  });

  it("routes pipeline-status and speaker-detected payloads", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    const running = { type: "Running" } as const;
    const status = {
      capture: running,
      pipeline: running,
      asr: running,
      diarization: running,
      entity_extraction: running,
      graph: running,
    };
    handlers.get("pipeline-status")?.(makeEvent("pipeline-status", status));
    expect(useAudioGraphStore.getState().pipelineStatus).toEqual(status);

    const health = {
      consumers: [
        {
          id: "speech",
          stage: "speech",
          provider: null,
          active: true,
          queue_len: 2,
          queue_capacity: 1024,
          sent_chunks: 12,
          dropped_chunks: 1,
          drop_policy: "drop_oldest",
          source_filter: { type: "all" },
          mixing_mode: "per_source",
        },
      ],
    };
    handlers.get("audio-consumer-health")?.(
      makeEvent("audio-consumer-health", health),
    );
    expect(useAudioGraphStore.getState().latestAudioConsumerHealth).toEqual(
      health,
    );

    const speaker = { id: "spk-1", label: "Alice", color: "#ff0000" };
    handlers.get("speaker-detected")?.(makeEvent("speaker-detected", speaker));
    expect(useAudioGraphStore.getState().speakers).toContainEqual(speaker);
  });

  it("routes pipeline-latency payloads into the latest stage sample map", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("pipeline-latency")?.(
      makeEvent("pipeline-latency", {
        stage: "asr",
        source_id: "system-default",
        segment_id: "seg-1",
        latency_ms: 123.4,
        timestamp_ms: 1_700_000_000_000,
      }),
    );

    // pipeline-latency is throttled (latest-wins, ~100ms); poll until flush.
    await waitFor(() => {
      expect(useAudioGraphStore.getState().pipelineLatencies.asr).toMatchObject(
        {
          stage: "asr",
          latency_ms: 123.4,
          source_id: "system-default",
          segment_id: "seg-1",
        },
      );
    });
  });

  it("sets store.error from capture-error payload", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("capture-error")?.(
      makeEvent("capture-error", {
        source_id: "mic-1",
        error: "device disconnected",
      }),
    );

    expect(useAudioGraphStore.getState().error).toBe("device disconnected");
    errSpy.mockRestore();
  });

  it("tracks capture-backpressure add and clear transitions", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("capture-backpressure")?.(
      makeEvent("capture-backpressure", {
        source_id: "mic-1",
        is_backpressured: true,
      }),
    );
    expect(useAudioGraphStore.getState().backpressuredSources).toContain(
      "mic-1",
    );

    handlers.get("capture-backpressure")?.(
      makeEvent("capture-backpressure", {
        source_id: "mic-1",
        is_backpressured: false,
      }),
    );
    expect(useAudioGraphStore.getState().backpressuredSources).not.toContain(
      "mic-1",
    );
  });

  it("tracks persistence queue backpressure add and clear transitions", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("persistence-queue-backpressure")?.(
      makeEvent("persistence-queue-backpressure", {
        writer: "transcript_event",
        is_backpressured: true,
        queue_capacity: 2048,
        dropped_count: 2,
      }),
    );
    expect(
      useAudioGraphStore.getState().persistenceQueueBackpressure
        .transcript_event,
    ).toMatchObject({
      writer: "transcript_event",
      queue_capacity: 2048,
      dropped_count: 2,
    });

    handlers.get("persistence-queue-backpressure")?.(
      makeEvent("persistence-queue-backpressure", {
        writer: "transcript_event",
        is_backpressured: false,
        queue_capacity: 2048,
        dropped_count: 2,
      }),
    );
    expect(useAudioGraphStore.getState().persistenceQueueBackpressure).toEqual(
      {},
    );
  });

  it("appends gemini-transcription events to the transcript list", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("gemini-transcription")?.(
      makeEvent("gemini-transcription", {
        text: "hi there",
        is_final: true,
      }),
    );

    const entries = useAudioGraphStore.getState().geminiTranscripts;
    expect(entries).toHaveLength(1);
    expect(entries[0]).toMatchObject({
      text: "hi there",
      is_final: true,
      source: "gemini",
    });
  });

  it("tolerates partial listen() failures and still cleans up successful listeners", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    vi.mocked(listen).mockImplementation(
      async (eventName: string, cb: Handler) => {
        if (eventName === "graph-update") {
          throw new Error("listen boom");
        }
        handlers.set(eventName, cb);
        const unlisten = vi.fn();
        unlisteners.push(unlisten);
        return unlisten;
      },
    );

    const { unmount } = renderHook(() => useTauriEvents());
    await waitFor(() => {
      // All listeners but the one that was made to throw.
      expect(handlers.size).toBe(TOTAL_LISTENERS - 1);
    });
    expect(handlers.has("graph-update")).toBe(false);

    unmount();
    for (const fn of unlisteners) {
      expect(fn).toHaveBeenCalledTimes(1);
    }
    errSpy.mockRestore();
  });

  it("flips isGeminiActive off when gemini-status 'disconnected' fires", async () => {
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();
    expect(useAudioGraphStore.getState().isGeminiActive).toBe(true);

    handlers.get("gemini-status")?.(
      makeEvent("gemini-status", { type: "disconnected" }),
    );
    expect(useAudioGraphStore.getState().isGeminiActive).toBe(false);
  });

  it("clears a stale error banner when gemini-status 'reconnected' fires (FINDING #56 P2)", async () => {
    useAudioGraphStore.setState({
      error: "Gemini: transient network blip",
      notifications: [],
    });
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("gemini-status")?.(
      makeEvent("gemini-status", { type: "reconnected", resumed: true }),
    );

    // The sticky legacy banner is cleared by recovery (nothing else clears
    // it), and a success notification is queued.
    expect(useAudioGraphStore.getState().error).toBeNull();
    expect(useAudioGraphStore.getState().notifications).toHaveLength(1);
  });

  it("clears a stale error banner when gemini-status 'connected' fires (FINDING #56 P2)", async () => {
    useAudioGraphStore.setState({ error: "Gemini: prior failure" });
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("gemini-status")?.(
      makeEvent("gemini-status", { type: "connected" }),
    );

    expect(useAudioGraphStore.getState().error).toBeNull();
  });

  // ------------------------------------------------------------------
  // ag#13 — AWS error translation
  // ------------------------------------------------------------------
  //
  // The aws-error event is the contract between the backend's
  // UiAwsError taxonomy and the frontend's i18n-backed user messaging.
  // This covers:
  //   1. the category → i18n-key mapping via awsErrorToMessage
  //   2. the listener wiring (payload lands in store.error)
  // Those two together guarantee that a backend event with
  // `category: "invalid_access_key"` surfaces as a localized,
  // actionable message in the global error banner.

  it("translates aws-error payloads via awsErrorToMessage and routes to store.error", async () => {
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    // Invalid access key → check the core mapping is exercised.
    const invalidKey: AwsErrorPayload = {
      error: { category: "invalid_access_key" },
      raw_message: "InvalidClientTokenId: The security token is invalid",
    };
    handlers.get("aws-error")?.(makeEvent("aws-error", invalidKey));
    expect(useAudioGraphStore.getState().error).toContain(
      "Access Key ID not recognized",
    );
    // And the exported helper returns the same string, so future
    // consumers (a diagnostics panel, tests in other modules) stay
    // in sync without duplicating the switch.
    expect(awsErrorToMessage(invalidKey)).toBe(
      useAudioGraphStore.getState().error,
    );

    // Region-parameterised payload renders the region into the message.
    const regionErr: AwsErrorPayload = {
      error: {
        category: "region_not_supported",
        region: "ap-south-2",
      },
      raw_message: "UnrecognizedClientException: wrong region",
    };
    handlers.get("aws-error")?.(makeEvent("aws-error", regionErr));
    expect(useAudioGraphStore.getState().error).toContain("ap-south-2");

    // AccessDenied with a parsed permission surfaces the action name
    // so the user knows which IAM policy is missing.
    const accessDenied: AwsErrorPayload = {
      error: {
        category: "access_denied",
        permission: "transcribe:StartStreamTranscription",
      },
      raw_message:
        "not authorized to perform: transcribe:StartStreamTranscription",
    };
    handlers.get("aws-error")?.(makeEvent("aws-error", accessDenied));
    expect(useAudioGraphStore.getState().error).toContain(
      "transcribe:StartStreamTranscription",
    );

    errSpy.mockRestore();
  });

  // Exhaustive routing check for every GeminiErrorCategory variant:
  // confirms the (kind → i18n key, toast variant) pairing stays stable.
  // If the category spec ever grows a new variant this test should fail
  // at the switch exhaustiveness check rather than surface the wrong
  // toast severity in production.
  it("routes every Gemini error category to the right i18n key + toast variant", () => {
    const cases: Array<{
      category: GeminiErrorCategory;
      expectedKey: string;
      expectedVariant: "warning" | "info" | "error";
    }> = [
      {
        category: { kind: "auth" },
        expectedKey: "gemini.error.auth",
        expectedVariant: "warning",
      },
      {
        category: { kind: "auth_expired" },
        expectedKey: "gemini.error.authExpired",
        expectedVariant: "warning",
      },
      {
        category: { kind: "rate_limit", retry_after_secs: 30 },
        expectedKey: "gemini.error.rateLimit",
        expectedVariant: "warning",
      },
      {
        category: { kind: "network" },
        expectedKey: "gemini.error.network",
        expectedVariant: "info",
      },
      {
        category: { kind: "server" },
        expectedKey: "gemini.error.server",
        expectedVariant: "error",
      },
      {
        category: { kind: "unknown" },
        expectedKey: "gemini.error.unknown",
        expectedVariant: "error",
      },
    ];

    for (const { category, expectedKey, expectedVariant } of cases) {
      const { key, variant } = routeGeminiError(category);
      expect(key).toBe(expectedKey);
      expect(variant).toBe(expectedVariant);
    }
  });

  it("fires a toast when gemini-status 'error' arrives with a category", async () => {
    useAudioGraphStore.setState({ notifications: [] });
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("gemini-status")?.(
      makeEvent("gemini-status", {
        type: "error",
        message: "WS close 1008 api key invalid",
        category: { kind: "auth" },
      }),
    );

    // Classified errors route through a notification (warning) — they do
    // NOT set the global error banner, because auth failures are
    // recoverable via Settings → Gemini Live.
    const notes = useAudioGraphStore.getState().notifications;
    expect(notes).toHaveLength(1);
    expect(notes[0].severity).toBe("warning");
  });

  it("falls back to the error banner when gemini-status 'error' has no category", async () => {
    useAudioGraphStore.setState({ notifications: [], error: null });
    renderHook(() => useTauriEvents());
    await waitForAllHandlers();

    handlers.get("gemini-status")?.(
      makeEvent("gemini-status", {
        type: "error",
        message: "legacy plain-string error",
      }),
    );

    // Legacy events without `category` preserve the prior behavior:
    // the message lands in the banner so existing backend paths keep
    // working during the migration.
    expect(useAudioGraphStore.getState().error).toBe(
      "Gemini: legacy plain-string error",
    );
    expect(useAudioGraphStore.getState().notifications).toHaveLength(0);
  });
});
