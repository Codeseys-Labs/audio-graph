import { Channel, invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  AppSettings,
  AsrSpanRevisionEvent,
  AudioSourceInfo,
  LiveAssistCardRecord,
  ProjectionPatch,
} from "../types";
import { useAudioGraphStore } from "./index";

function asrSpanRevision(
  revisionNumber: number,
  overrides: Partial<AsrSpanRevisionEvent> = {},
): AsrSpanRevisionEvent {
  return {
    span_id: "deepgram:system-default:0-500",
    provider: "deepgram",
    source_id: "system-default",
    provider_item_id: null,
    transcript_segment_id: null,
    speaker_id: null,
    speaker_label: null,
    channel: null,
    text: "hello",
    start_time: 0,
    end_time: 0.5,
    confidence: 0.7,
    is_final: false,
    stability: "partial",
    revision_number: revisionNumber,
    supersedes: null,
    turn_id: null,
    end_of_turn: false,
    raw_event_ref: null,
    received_at_ms: 1_700_000_000_000 + revisionNumber,
    ...overrides,
  };
}

function noteProjectionPatch(
  sequence: number,
  operations: ProjectionPatch["operations"],
): ProjectionPatch {
  return {
    sequence,
    kind: "notes",
    llm_request_id: `llm-notes-${sequence}`,
    basis: { transcript_hash: `fnv1a64:notes:${sequence}` },
    operations,
    confidence: 0.9,
    provenance: {
      provider: "test",
      model: "projection-test",
      prompt_id: "projection_patch_v1_test",
    },
    created_at_ms: 1_700_000_000_000 + sequence,
  };
}

function graphProjectionPatch(
  sequence: number,
  operations: ProjectionPatch["operations"],
): ProjectionPatch {
  return {
    sequence,
    kind: "graph",
    llm_request_id: `llm-graph-${sequence}`,
    basis: { transcript_hash: `fnv1a64:graph:${sequence}` },
    operations,
    confidence: 0.88,
    provenance: {
      provider: "test",
      model: "projection-test",
      prompt_id: "projection_patch_v1_test",
    },
    created_at_ms: 1_700_000_001_000 + sequence,
  };
}

function liveAssistCard(
  proposalId: string,
  overrides: Omit<Partial<LiveAssistCardRecord>, "proposal"> & {
    proposal?: Partial<LiveAssistCardRecord["proposal"]>;
  } = {},
): LiveAssistCardRecord {
  const { proposal: proposalOverrides, ...recordOverrides } = overrides;
  const proposal = {
    id: proposalId,
    source_segment_id: `segment-${proposalId}`,
    source_id: "system",
    speaker_label: null,
    kind: "note" as const,
    title: `Card ${proposalId}`,
    body: `Body ${proposalId}`,
    confidence: 0.8,
    created_at_ms: 10,
    ...(proposalOverrides ?? {}),
  };
  return {
    session_id: "session-1",
    status: "pending",
    source_span_ids: [proposal.source_segment_id],
    graph_context_ids: [],
    outcome: null,
    projection_patch_sequence: null,
    created_at_ms: proposal.created_at_ms,
    updated_at_ms: proposal.created_at_ms,
    ...recordOverrides,
    proposal,
  };
}

describe("AudioGraphStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useAudioGraphStore.setState({
      audioSources: [],
      selectedSourceIds: [],
      sourceRecoveryIntent: null,
      samplePreviewActive: false,
      transcriptSegments: [],
      asrPartial: null,
      asrSpanRevisions: [],
      diarizationSpanRevisions: [],
      sessionTranscriptEvents: [],
      sessionProjectionEvents: [],
      materializedNotes: null,
      materializedProjectionGraph: null,
      agentProposals: [],
      liveAssistCards: [],
      approvingAgentProposalIds: [],
      chatMessages: [],
      isChatLoading: false,
      streamingChatRequestId: null,
      isCapturing: false,
      captureStartTime: null,
      error: null,
      graphSnapshot: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
      speakers: [],
    });
  });

  it("starts with empty state", () => {
    const s = useAudioGraphStore.getState();
    expect(s.audioSources).toEqual([]);
    expect(s.selectedSourceIds).toEqual([]);
    expect(s.isCapturing).toBe(false);
  });

  it("loads a frontend-only sample session preview without backend writes", () => {
    useAudioGraphStore.getState().loadSampleSessionPreview();

    const state = useAudioGraphStore.getState();
    expect(state.transcriptSegments).toHaveLength(4);
    expect(state.transcriptSegments[0]).toMatchObject({
      id: "sample-segment-1",
      source_id: "sample-source",
      speaker_label: "Maya",
    });
    expect(state.asrSpanRevisions).toHaveLength(4);
    expect(state.sessionTranscriptEvents).toHaveLength(4);
    expect(state.materializedNotes).toMatchObject({
      session_id: "sample-session-preview",
      last_sequence: 1,
    });
    expect(state.materializedNotes?.notes.map((note) => note.id)).toEqual([
      "sample-note-setup",
      "sample-note-retcon",
      "sample-note-platform",
    ]);
    expect(state.materializedProjectionGraph).toMatchObject({
      session_id: "sample-session-preview",
      last_sequence: 2,
    });
    expect(
      state.materializedProjectionGraph?.nodes.map((node) => node.id).sort(),
    ).toEqual([
      "sample-decision-retcon",
      "sample-question-provider",
      "sample-task-release",
      "sample-topic-setup",
    ]);
    expect(state.graphSnapshot.stats).toEqual({
      total_nodes: 4,
      total_edges: 2,
      total_episodes: 1,
    });
    expect(state.liveAssistCards).toHaveLength(2);
    expect(state.agentProposals).toEqual([]);
    expect(state.samplePreviewActive).toBe(true);
    expect(state.agentOverlayOpen).toBe(true);
    expect(state.rightPanelTab).toBe("transcript");
    expect(state.isCapturing).toBe(false);
    expect(state.isTranscribing).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("localizes the built-in sample session preview from the active language", () => {
    useAudioGraphStore.getState().loadSampleSessionPreview("pt-BR");

    const state = useAudioGraphStore.getState();
    expect(state.samplePreviewActive).toBe(true);
    expect(state.transcriptSegments[0]?.text).toContain("credenciais salvas");
    expect(state.materializedNotes?.notes[0]).toMatchObject({
      title: "Caminho de configuração com chave salva",
      tags: ["configuração", "credenciais"],
    });
    expect(state.materializedProjectionGraph?.nodes[0]).toMatchObject({
      name: "Credenciais salvas",
    });
    expect(state.liveAssistCards[1]?.outcome?.message).toBe(
      "Cartão de exemplo aprovado apenas na projeção de pré-visualização.",
    );
    expect(invoke).not.toHaveBeenCalled();
  });

  it("exports visible sample transcript and graph data without backend invokes", async () => {
    useAudioGraphStore.getState().loadSampleSessionPreview();

    const transcriptJson = await useAudioGraphStore
      .getState()
      .exportTranscript();
    const graphJson = await useAudioGraphStore.getState().exportGraph();
    const sessionId = await useAudioGraphStore.getState().getSessionId();

    expect(JSON.parse(transcriptJson)).toEqual({
      session_id: "sample-session-preview",
      preview: true,
      segments: useAudioGraphStore.getState().transcriptSegments,
      events: useAudioGraphStore.getState().sessionTranscriptEvents,
    });
    expect(JSON.parse(graphJson)).toEqual({
      session_id: "sample-session-preview",
      preview: true,
      materialized_graph:
        useAudioGraphStore.getState().materializedProjectionGraph,
      snapshot: useAudioGraphStore.getState().graphSnapshot,
    });
    expect(sessionId).toBe("sample-session-preview");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("clears sample preview state before applying real transcript events", () => {
    useAudioGraphStore.getState().loadSampleSessionPreview();

    useAudioGraphStore.getState().addAsrSpanRevision(
      asrSpanRevision(1, {
        span_id: "real-span-1",
        transcript_segment_id: "real-segment-1",
        text: "real transcript",
        is_final: true,
        stability: "final",
      }),
    );

    const state = useAudioGraphStore.getState();
    expect(state.samplePreviewActive).toBe(false);
    expect(state.transcriptSegments).toEqual([
      expect.objectContaining({ id: "real-span-1", text: "real transcript" }),
    ]);
    expect(state.materializedNotes).toBeNull();
    expect(state.materializedProjectionGraph).toBeNull();
    expect(state.graphSnapshot.nodes).toEqual([]);
    expect(state.liveAssistCards).toEqual([]);
    expect(state.speakers).toEqual([]);
  });

  it("toggles source selection", () => {
    useAudioGraphStore.getState().toggleSourceId("mic-1");
    expect(useAudioGraphStore.getState().selectedSourceIds).toContain("mic-1");
    useAudioGraphStore.getState().toggleSourceId("mic-1");
    expect(useAudioGraphStore.getState().selectedSourceIds).not.toContain(
      "mic-1",
    );
  });

  it("clears selected sources", () => {
    useAudioGraphStore.getState().toggleSourceId("mic-1");
    useAudioGraphStore.getState().toggleSourceId("mic-2");
    expect(useAudioGraphStore.getState().selectedSourceIds).toHaveLength(2);
    useAudioGraphStore.getState().clearSelectedSources();
    expect(useAudioGraphStore.getState().selectedSourceIds).toEqual([]);
  });

  it("removes a targeted subset of selected sources", () => {
    useAudioGraphStore.setState({
      selectedSourceIds: ["system-default", "device:stale", "app:42"],
    });

    useAudioGraphStore
      .getState()
      .removeSelectedSourceIds(["device:stale", "missing"]);

    expect(useAudioGraphStore.getState().selectedSourceIds).toEqual([
      "system-default",
      "app:42",
    ]);
  });

  it("records source recovery intents with a monotonic local id", () => {
    useAudioGraphStore.getState().requestSourceRecovery({
      origin: "provider_setup",
      issues: [
        {
          kind: "unavailable",
          sourceId: "device:stale",
          message: "Selected audio source device:stale is not available.",
        },
      ],
    });
    const first = useAudioGraphStore.getState().sourceRecoveryIntent;

    useAudioGraphStore.getState().requestSourceRecovery({
      origin: "provider_setup",
      issues: [
        {
          kind: "unselected",
          message: "Select an audio source before starting capture.",
        },
      ],
    });
    const second = useAudioGraphStore.getState().sourceRecoveryIntent;

    expect(first).toMatchObject({ id: 1, origin: "provider_setup" });
    expect(second).toMatchObject({
      id: 2,
      origin: "provider_setup",
      issues: [expect.objectContaining({ kind: "unselected" })],
    });

    useAudioGraphStore.getState().clearSourceRecoveryIntent();
    expect(useAudioGraphStore.getState().sourceRecoveryIntent).toBeNull();
  });

  it("stores only backend-redacted settings after save", async () => {
    const secretDraft: AppSettings = {
      asr_provider: {
        type: "deepgram",
        api_key: "dg-plaintext",
        model: "nova-3",
        enable_diarization: true,
      },
      whisper_model: "ggml-small.en.bin",
      llm_provider: {
        type: "openrouter",
        model: "anthropic/claude-sonnet-4.5",
        base_url: "https://openrouter.ai/api/v1",
        provider_order: null,
        include_usage_in_stream: true,
        api_key: "or-plaintext",
      },
      llm_api_config: {
        endpoint: "https://api.openai.com/v1",
        api_key: "openai-plaintext",
        model: "gpt-4o-mini",
        max_tokens: 2048,
        temperature: 0.7,
      },
      audio_settings: { sample_rate: 48000, channels: 2 },
      gemini: {
        auth: { type: "api_key", api_key: "gemini-plaintext" },
        model: "gemini-2.0-flash-live-001",
      },
      tts_provider: { type: "none" },
      speak_aloud: false,
      streaming_prefill: false,
      log_level: "info",
    };
    const redactedSecretSettings: AppSettings = {
      ...secretDraft,
      asr_provider: {
        type: "deepgram",
        model: "nova-3",
        enable_diarization: true,
      },
      llm_provider: {
        type: "openrouter",
        model: "anthropic/claude-sonnet-4.5",
        base_url: "https://openrouter.ai/api/v1",
        provider_order: null,
        include_usage_in_stream: true,
      },
      llm_api_config: {
        endpoint: "https://api.openai.com/v1",
        api_key: null,
        model: "gpt-4o-mini",
        max_tokens: 2048,
        temperature: 0.7,
      },
      gemini: {
        auth: { type: "api_key" },
        model: "gemini-2.0-flash-live-001",
      },
    };
    const awsDraft: AppSettings = {
      ...redactedSecretSettings,
      asr_provider: {
        type: "aws_transcribe",
        region: "us-east-1",
        language_code: "en-US",
        credential_source: { type: "access_keys", access_key: "AKIA_ASR" },
        enable_diarization: true,
      },
      llm_provider: {
        type: "aws_bedrock",
        region: "us-east-1",
        model_id: "anthropic.claude-3-5-sonnet",
        credential_source: { type: "access_keys", access_key: "AKIA_LLM" },
      },
      gemini: {
        auth: { type: "api_key", api_key: "gemini-plaintext-again" },
        model: "gemini-2.0-flash-live-001",
      },
    };
    const redactedAwsSettings: AppSettings = {
      ...awsDraft,
      asr_provider: {
        type: "aws_transcribe",
        region: "us-east-1",
        language_code: "en-US",
        credential_source: { type: "access_keys" },
        enable_diarization: true,
      },
      llm_provider: {
        type: "aws_bedrock",
        region: "us-east-1",
        model_id: "anthropic.claude-3-5-sonnet",
        credential_source: { type: "access_keys" },
      },
      gemini: {
        auth: { type: "api_key" },
        model: "gemini-2.0-flash-live-001",
      },
    };
    const loadResponses = [redactedSecretSettings, redactedAwsSettings];
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "save_settings_cmd") return undefined;
      if (cmd === "load_settings_cmd") return loadResponses.shift();
      return undefined;
    });

    await useAudioGraphStore.getState().saveSettings(secretDraft);
    expect(useAudioGraphStore.getState().settings).toEqual(
      redactedSecretSettings,
    );
    expect(useAudioGraphStore.getState().settings).not.toEqual(secretDraft);

    await useAudioGraphStore.getState().saveSettings(awsDraft);
    expect(useAudioGraphStore.getState().settings).toEqual(redactedAwsSettings);
    expect(useAudioGraphStore.getState().settings).not.toEqual(awsDraft);
    expect(invoke).toHaveBeenNthCalledWith(1, "save_settings_cmd", {
      settings: secretDraft,
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "load_settings_cmd");
    expect(invoke).toHaveBeenNthCalledWith(3, "save_settings_cmd", {
      settings: awsDraft,
    });
    expect(invoke).toHaveBeenNthCalledWith(4, "load_settings_cmd");
  });

  it("sets and clears error state", () => {
    useAudioGraphStore.getState().setError("boom");
    expect(useAudioGraphStore.getState().error).toBe("boom");
    useAudioGraphStore.getState().clearError();
    expect(useAudioGraphStore.getState().error).toBeNull();
  });

  it("hydrates projection artifacts when loading a full session", async () => {
    const transcript = [
      {
        id: "seg-1",
        source_id: "system-default",
        speaker_id: null,
        speaker_label: null,
        text: "stored transcript",
        start_time: 0,
        end_time: 1,
        confidence: 0.9,
      },
    ];
    const transcriptEvents = [
      {
        span_id: "span-1",
        provider: "test",
        source_id: "system-default",
        provider_item_id: null,
        transcript_segment_id: "seg-1",
        speaker_id: null,
        speaker_label: null,
        channel: null,
        text: "stored transcript",
        start_time: 0,
        end_time: 1,
        confidence: 0.9,
        is_final: true,
        stability: "final",
        revision_number: 1,
        supersedes: null,
        turn_id: null,
        end_of_turn: true,
        raw_event_ref: null,
        received_at_ms: 1_700_000_000_000,
      },
    ];
    const projectionEvents = [
      {
        sequence: 1,
        kind: "notes",
        llm_request_id: "llm-1",
        basis: { transcript_hash: "fnv1a64:test" },
        operations: [],
        confidence: 0.8,
        provenance: { provider: "test", model: "test", prompt_id: "notes-v1" },
        created_at_ms: 1_700_000_000_001,
      },
    ];
    const notes = {
      schema_version: 1,
      session_id: "session-1",
      last_sequence: 1,
      notes: [
        {
          id: "note-1",
          title: "Loaded note",
          body: "Loaded body",
          tags: [],
          updated_by_sequence: 1,
          updated_at_ms: 1_700_000_000_001,
          basis: { transcript_hash: "fnv1a64:test" },
          provenance: {
            provider: "test",
            model: "test",
            prompt_id: "notes-v1",
          },
        },
      ],
    };
    const materializedGraph = {
      schema_version: 1,
      session_id: "session-1",
      last_sequence: 1,
      nodes: [{ id: "node-1" }],
      edges: [],
    };
    const pendingCard = liveAssistCard("pending-card", {
      proposal: { title: "Pending live card", created_at_ms: 40 },
      updated_at_ms: 40,
    });
    const approvedCard = liveAssistCard("approved-card", {
      status: "approved",
      proposal: { title: "Approved live card", created_at_ms: 30 },
      outcome: {
        proposal_id: "approved-card",
        action: "chat_note",
        message: "Approved card outcome",
        graph_updated: false,
        timestamp_ms: 31,
      },
      projection_patch_sequence: 7,
      updated_at_ms: 31,
    });
    // Persisted diarization span revisions (audio-graph-0b33): a mid-session
    // relabel (rev1 provisional → rev2 stable) that reload must hydrate so the
    // speaker-timeline join resolves trusted latest-wins attribution.
    const diarizationEvents = [
      {
        span_id: "diar-span-1",
        provider: "local_clustering",
        timeline_id: "session-1",
        source_id: null,
        speaker_id: "2",
        speaker_label: "Speaker 2",
        channel: null,
        start_time: 0,
        end_time: 1,
        confidence: 0.7,
        is_final: false,
        stability: "provisional",
        revision_number: 1,
        supersedes: null,
        basis_asr_span_ids: ["diar-span-1-asr"],
        basis_transcript_segment_ids: [],
        raw_event_ref: null,
        received_at_ms: 1_700_000_000_001,
      },
      {
        span_id: "diar-span-1",
        provider: "assemblyai",
        timeline_id: "session-1",
        source_id: null,
        speaker_id: "alice",
        speaker_label: "Alice",
        channel: null,
        start_time: 0,
        end_time: 1,
        confidence: 0.95,
        is_final: true,
        stability: "stable",
        revision_number: 2,
        supersedes: "diar-span-1@rev1",
        basis_asr_span_ids: ["diar-span-1-asr"],
        basis_transcript_segment_ids: [],
        raw_event_ref: null,
        received_at_ms: 1_700_000_000_002,
      },
    ];
    vi.mocked(invoke).mockResolvedValueOnce({
      transcript,
      graph: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
      transcript_events: transcriptEvents,
      diarization_events: diarizationEvents,
      projection_events: projectionEvents,
      notes,
      materialized_graph: materializedGraph,
      live_assist_cards: [pendingCard, approvedCard],
    });

    const loaded = await useAudioGraphStore.getState().loadSession("session-1");

    expect(invoke).toHaveBeenCalledWith("load_session", {
      sessionId: "session-1",
    });
    expect(loaded?.transcript).toEqual(transcript);
    const state = useAudioGraphStore.getState();
    expect(state.transcriptSegments).toEqual(transcript);
    expect(state.sessionTranscriptEvents).toEqual(transcriptEvents);
    // The persisted speaker log is hydrated into the store so the
    // joinSpeakerTimelineToTranscript selector resolves trusted attribution on
    // a loaded session (audio-graph-0b33).
    expect(state.diarizationSpanRevisions).toEqual(diarizationEvents);
    expect(state.sessionProjectionEvents).toEqual(projectionEvents);
    expect(state.materializedNotes).toEqual(notes);
    expect(state.materializedProjectionGraph).toEqual(materializedGraph);
    expect(state.liveAssistCards).toEqual([pendingCard, approvedCard]);
    expect(state.agentProposals).toEqual([]);
    // Loading a historical session records its id so the data-route / privacy
    // report (seed audio-graph-51e0) can fetch its data-movement ledger.
    expect(state.loadedSessionId).toBe("session-1");
  });

  it("resets diarizationSpanRevisions when a loaded session has no speaker log", async () => {
    // A prior session left stale revisions in the store; loading a session
    // whose payload omits diarization_events must clear them so attribution
    // does not leak across sessions (audio-graph-0b33).
    useAudioGraphStore.getState().addDiarizationSpanRevision({
      span_id: "stale-span",
      provider: "local_clustering",
      timeline_id: "prior-session",
      speaker_id: "9",
      speaker_label: "Stale Speaker",
      start_time: 0,
      end_time: 1,
      is_final: true,
      stability: "stable",
      revision_number: 1,
      basis_asr_span_ids: [],
      basis_transcript_segment_ids: [],
      received_at_ms: 1_700_000_000_000,
    });
    expect(useAudioGraphStore.getState().diarizationSpanRevisions).toHaveLength(
      1,
    );

    vi.mocked(invoke).mockResolvedValueOnce({
      transcript: [],
      graph: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
      transcript_events: [],
      projection_events: [],
      notes: null,
      materialized_graph: null,
      live_assist_cards: [],
    });

    await useAudioGraphStore.getState().loadSession("session-no-diar");

    expect(useAudioGraphStore.getState().diarizationSpanRevisions).toEqual([]);
  });

  it("clears ASR revision state when loading a full session", async () => {
    const store = useAudioGraphStore.getState();
    store.setAsrPartial({
      provider: "deepgram",
      source_id: "system-default",
      text: "old partial",
      start_time: 0,
      end_time: 0.5,
      confidence: 0.5,
      timestamp_ms: 1_700_000_000_000,
    });
    store.addAsrSpanRevision(
      asrSpanRevision(3, {
        text: "old session final",
        is_final: true,
        stability: "final",
      }),
    );
    vi.mocked(invoke).mockResolvedValueOnce({
      transcript: [],
      graph: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
      transcript_events: [],
      projection_events: [],
      notes: null,
      materialized_graph: null,
      live_assist_cards: [],
    });

    await store.loadSession("session-2");
    useAudioGraphStore.getState().addAsrSpanRevision(
      asrSpanRevision(1, {
        text: "new session partial",
      }),
    );

    const state = useAudioGraphStore.getState();
    expect(state.asrPartial).toBeNull();
    expect(
      state.asrSpanRevisions.map((revision) => revision.revision_number),
    ).toEqual([1]);
    expect(state.transcriptSegments).toEqual([
      expect.objectContaining({
        id: "deepgram:system-default:0-500",
        text: "new session partial",
      }),
    ]);
  });

  it("applies live projection patch and materialized artifact updates", () => {
    const patch: ProjectionPatch = {
      sequence: 3,
      kind: "graph",
      llm_request_id: "llm-live-graph",
      basis: { transcript_hash: "fnv1a64:live-graph" },
      operations: [],
      confidence: 0.91,
      provenance: {
        provider: "test",
        model: "projection-test",
        prompt_id: "projection_patch_v1_test",
      },
      created_at_ms: 1_700_000_000_003,
    };
    const notes = {
      schema_version: 1,
      session_id: "session-live",
      last_sequence: 2,
      notes: [
        {
          id: "note-live",
          title: "Live note",
          body: "Live body",
          tags: [],
          updated_by_sequence: 2,
          updated_at_ms: 1_700_000_000_002,
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
      last_sequence: 3,
      nodes: [
        {
          id: "node-live",
          name: "Live node",
          entity_type: "Topic",
          description: null,
          confidence: 0.9,
          valid_from_ms: 1_700_000_000_001,
          valid_until_ms: null,
          updated_by_sequence: 3,
          updated_at_ms: 1_700_000_000_003,
          basis: { transcript_hash: "fnv1a64:live" },
          provenance: {
            provider: "test",
            model: "projection-test",
            prompt_id: "projection_patch_v1_test",
          },
        },
      ],
      edges: [
        {
          id: "edge-live",
          source: "node-live",
          target: "node-live",
          relation_type: "mentions",
          label: null,
          weight: 1,
          confidence: 0.8,
          valid_from_ms: 1_700_000_000_001,
          valid_until_ms: null,
          updated_by_sequence: 3,
          updated_at_ms: 1_700_000_000_003,
          basis: { transcript_hash: "fnv1a64:live" },
          provenance: {
            provider: "test",
            model: "projection-test",
            prompt_id: "projection_patch_v1_test",
          },
        },
      ],
    };

    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(patch);
    store.setMaterializedNotes(notes);
    store.setMaterializedProjectionGraph(graph);

    const state = useAudioGraphStore.getState();
    expect(state.sessionProjectionEvents).toEqual([patch]);
    expect(state.materializedNotes).toEqual(notes);
    expect(state.materializedProjectionGraph).toEqual(graph);
  });

  it("applies ASR span revisions to the visible transcript by stable span id", () => {
    const store = useAudioGraphStore.getState();
    store.setAsrPartial({
      provider: "deepgram",
      source_id: "system-default",
      text: "hel",
      start_time: 0,
      end_time: 0.3,
      confidence: 0.6,
      timestamp_ms: 1_700_000_000_000,
    });
    store.addAsrSpanRevision(
      asrSpanRevision(1, {
        text: "hel",
        confidence: 0.6,
      }),
    );
    store.addAsrSpanRevision(
      asrSpanRevision(2, {
        text: "hello world",
        confidence: 0.93,
        is_final: true,
        stability: "final",
        speaker_id: "speaker-0",
        speaker_label: "Speaker 0",
        end_of_turn: true,
      }),
    );

    const state = useAudioGraphStore.getState();
    expect(state.asrPartial).toBeNull();
    expect(state.asrSpanRevisions.map((revision) => revision.text)).toEqual([
      "hel",
      "hello world",
    ]);
    expect(
      state.sessionTranscriptEvents.map((event) => event.revision_number),
    ).toEqual([1, 2]);
    expect(state.transcriptSegments).toEqual([
      expect.objectContaining({
        id: "deepgram:system-default:0-500",
        text: "hello world",
        confidence: 0.93,
        speaker_id: "speaker-0",
        speaker_label: "Speaker 0",
      }),
    ]);
  });

  it("ignores stale ASR span revisions for visible transcript while retaining event history", () => {
    const store = useAudioGraphStore.getState();
    store.addAsrSpanRevision(
      asrSpanRevision(2, {
        text: "current final",
        is_final: true,
        stability: "final",
        confidence: 0.95,
      }),
    );
    const currentSegments = useAudioGraphStore.getState().transcriptSegments;
    store.addAsrSpanRevision(
      asrSpanRevision(1, {
        text: "older partial",
      }),
    );

    const state = useAudioGraphStore.getState();
    expect(
      state.asrSpanRevisions.map((revision) => revision.revision_number),
    ).toEqual([2, 1]);
    expect(
      state.sessionTranscriptEvents.map((event) => event.revision_number),
    ).toEqual([2, 1]);
    expect(state.transcriptSegments).toBe(currentSegments);
    expect(state.transcriptSegments).toEqual([
      expect.objectContaining({
        id: "deepgram:system-default:0-500",
        text: "current final",
        confidence: 0.95,
      }),
    ]);
  });

  it("replaces a legacy transcript segment when an ASR revision references it", () => {
    const store = useAudioGraphStore.getState();
    store.addTranscriptSegment({
      id: "legacy-seg-1",
      source_id: "system-default",
      speaker_id: null,
      speaker_label: null,
      text: "legacy text",
      start_time: 0,
      end_time: 0.5,
      confidence: 0.8,
    });
    store.addAsrSpanRevision(
      asrSpanRevision(1, {
        span_id: "deepgram:system-default:0-500",
        transcript_segment_id: "legacy-seg-1",
        text: "canonical text",
        is_final: true,
        stability: "final",
      }),
    );

    expect(useAudioGraphStore.getState().transcriptSegments).toEqual([
      expect.objectContaining({
        id: "deepgram:system-default:0-500",
        text: "canonical text",
      }),
    ]);
  });

  it("applies notes projection patches directly to materialized notes state", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      noteProjectionPatch(1, [
        {
          type: "upsert_note",
          id: "note-decision",
          title: "Decision",
          body: "Use stable projection ids.",
          tags: ["decision"],
        },
      ]),
    );
    store.addProjectionPatch(
      noteProjectionPatch(2, [
        {
          type: "upsert_note",
          id: "note-decision",
          title: "Decision",
          body: "Use stable projection ids and retcon by sequence.",
          tags: ["decision", "projection"],
        },
        {
          type: "upsert_note",
          id: "note-risk",
          title: "Risk",
          body: "Provider latency can reorder patches.",
          tags: ["risk"],
        },
      ]),
    );

    const notes = useAudioGraphStore.getState().materializedNotes;
    expect(notes?.session_id).toBe("live");
    expect(notes?.last_sequence).toBe(2);
    expect(notes?.notes).toEqual([
      expect.objectContaining({
        id: "note-decision",
        title: "Decision",
        body: "Use stable projection ids and retcon by sequence.",
        tags: ["decision", "projection"],
        updated_by_sequence: 2,
      }),
      expect.objectContaining({
        id: "note-risk",
        title: "Risk",
        updated_by_sequence: 2,
      }),
    ]);
  });

  it("applies notes delete and reorder retcons from projection patches", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      noteProjectionPatch(1, [
        {
          type: "upsert_note",
          id: "note-a",
          title: "A",
          body: "First",
          tags: [],
        },
        {
          type: "upsert_note",
          id: "note-b",
          title: "B",
          body: "Second",
          tags: [],
        },
        {
          type: "upsert_note",
          id: "note-c",
          title: "C",
          body: "Third",
          tags: [],
        },
      ]),
    );
    store.addProjectionPatch(
      noteProjectionPatch(2, [
        {
          type: "reorder_note",
          id: "note-c",
          after_id: null,
        },
        {
          type: "delete_note",
          id: "note-b",
        },
      ]),
    );

    const notes = useAudioGraphStore.getState().materializedNotes;
    expect(notes?.last_sequence).toBe(2);
    expect(notes?.notes.map((note) => note.id)).toEqual(["note-c", "note-a"]);

    store.addProjectionPatch(
      noteProjectionPatch(3, [
        {
          type: "reorder_note",
          id: "note-c",
          after_id: "note-a",
        },
      ]),
    );

    expect(
      useAudioGraphStore
        .getState()
        .materializedNotes?.notes.map((note) => note.id),
    ).toEqual(["note-a", "note-c"]);
  });

  it("ignores stale notes projection patch sequences while retaining event history", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      noteProjectionPatch(2, [
        {
          type: "upsert_note",
          id: "note-current",
          title: "Current",
          body: "Current version",
          tags: [],
        },
      ]),
    );
    const currentNotes = useAudioGraphStore.getState().materializedNotes;

    store.addProjectionPatch(
      noteProjectionPatch(1, [
        {
          type: "delete_note",
          id: "note-current",
        },
      ]),
    );

    const state = useAudioGraphStore.getState();
    expect(
      state.sessionProjectionEvents.map((patch) => patch.sequence),
    ).toEqual([2, 1]);
    expect(state.materializedNotes).toBe(currentNotes);
    expect(state.materializedNotes?.notes).toEqual([
      expect.objectContaining({
        id: "note-current",
        body: "Current version",
        updated_by_sequence: 2,
      }),
    ]);
  });

  it("applies graph projection patches directly to materialized graph state", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "node-a",
          name: "Node A",
          entity_type: "Topic",
          description: "First node",
        },
        {
          type: "upsert_graph_node",
          id: "node-b",
          name: "Node B",
          entity_type: "Project",
          description: null,
        },
        {
          type: "upsert_graph_edge",
          id: "edge-a-b",
          source: "node-a",
          target: "node-b",
          relation_type: "tracks",
          label: "tracks",
          weight: 0.5,
        },
      ]),
    );

    const graph = useAudioGraphStore.getState().materializedProjectionGraph;
    expect(graph?.session_id).toBe("live");
    expect(graph?.last_sequence).toBe(1);
    expect(graph?.nodes).toEqual([
      expect.objectContaining({
        id: "node-a",
        name: "Node A",
        description: "First node",
        valid_until_ms: null,
        updated_by_sequence: 1,
      }),
      expect.objectContaining({
        id: "node-b",
        name: "Node B",
        entity_type: "Project",
        valid_until_ms: null,
      }),
    ]);
    expect(graph?.edges).toEqual([
      expect.objectContaining({
        id: "edge-a-b",
        source: "node-a",
        target: "node-b",
        relation_type: "tracks",
        label: "tracks",
        weight: 0.5,
        valid_until_ms: null,
      }),
    ]);
  });

  it("applies graph invalidation retcons from projection patches", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "node-a",
          name: "Node A",
          entity_type: "Topic",
        },
        {
          type: "upsert_graph_node",
          id: "node-b",
          name: "Node B",
          entity_type: "Topic",
        },
        {
          type: "upsert_graph_edge",
          id: "edge-a-b",
          source: "node-a",
          target: "node-b",
          relation_type: "mentions",
          label: null,
          weight: 1,
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(2, [
        {
          type: "invalidate_graph_node",
          id: "node-b",
        },
      ]),
    );

    const graph = useAudioGraphStore.getState().materializedProjectionGraph;
    expect(graph?.last_sequence).toBe(2);
    expect(
      graph?.nodes.find((node) => node.id === "node-b")?.valid_until_ms,
    ).toBe(1_700_000_001_002);
    expect(
      graph?.edges.find((edge) => edge.id === "edge-a-b")?.valid_until_ms,
    ).toBe(1_700_000_001_002);
  });

  it("applies graph merge and split retcons from projection patches", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "source",
          name: "Source",
          entity_type: "Topic",
        },
        {
          type: "upsert_graph_node",
          id: "target",
          name: "Target",
          entity_type: "Topic",
        },
        {
          type: "upsert_graph_node",
          id: "other",
          name: "Other",
          entity_type: "Project",
        },
        {
          type: "upsert_graph_edge",
          id: "edge-source-other",
          source: "source",
          target: "other",
          relation_type: "tracks",
          label: null,
          weight: 0.4,
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(2, [
        {
          type: "merge_graph_nodes",
          source_id: "source",
          target_id: "target",
        },
      ]),
    );

    let graph = useAudioGraphStore.getState().materializedProjectionGraph;
    expect(
      graph?.nodes.find((node) => node.id === "source")?.valid_until_ms,
    ).toBe(1_700_000_001_002);
    expect(
      graph?.edges.find((edge) => edge.id === "edge-source-other"),
    ).toEqual(
      expect.objectContaining({
        source: "target",
        target: "other",
        valid_until_ms: null,
        updated_by_sequence: 2,
      }),
    );

    store.addProjectionPatch(
      graphProjectionPatch(3, [
        {
          type: "split_graph_node",
          id: "target",
          replacement_nodes: [
            {
              id: "target-a",
              name: "Target A",
              entity_type: "Topic",
            },
            {
              id: "target-b",
              name: "Target B",
              entity_type: "Topic",
            },
          ],
        },
      ]),
    );

    graph = useAudioGraphStore.getState().materializedProjectionGraph;
    expect(
      graph?.nodes.find((node) => node.id === "target")?.valid_until_ms,
    ).toBe(1_700_000_001_003);
    expect(
      graph?.edges.find((edge) => edge.id === "edge-source-other")
        ?.valid_until_ms,
    ).toBe(1_700_000_001_003);
    expect(
      graph?.nodes
        .filter((node) => node.valid_until_ms == null)
        .map((node) => node.id)
        .sort(),
    ).toEqual(["other", "target-a", "target-b"]);
  });

  it("invalidate_graph_edge retcon stamps valid_until_ms so the render view hides the edge (9d93)", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "node-a",
          name: "Node A",
          entity_type: "Topic",
        },
        {
          type: "upsert_graph_node",
          id: "node-b",
          name: "Node B",
          entity_type: "Topic",
        },
        {
          type: "upsert_graph_edge",
          id: "edge-a-b",
          source: "node-a",
          target: "node-b",
          relation_type: "mentions",
          label: null,
          weight: 1,
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(2, [
        {
          type: "invalidate_graph_edge",
          id: "edge-a-b",
        },
      ]),
    );

    const graph = useAudioGraphStore.getState().materializedProjectionGraph;
    // The edge object is retained (full retcon history) but stamped invalid.
    expect(
      graph?.edges.find((edge) => edge.id === "edge-a-b")?.valid_until_ms,
    ).toBe(1_700_000_001_002);
    // Both endpoints remain active — only the edge is hidden.
    expect(
      graph?.nodes
        .filter((node) => node.valid_until_ms == null)
        .map((node) => node.id)
        .sort(),
    ).toEqual(["node-a", "node-b"]);
    // The render layer (materializedGraphToSnapshot) shows only edges whose
    // valid_until_ms is null, so an invalidated edge disappears from the view.
    const activeEdgeIds = graph?.edges
      .filter((edge) => edge.valid_until_ms == null)
      .map((edge) => edge.id);
    expect(activeEdgeIds).toEqual([]);
  });

  it("strengthen/weaken graph-edge retcons clamp the weight into [0, 1] (9d93)", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "node-a",
          name: "Node A",
          entity_type: "Topic",
        },
        {
          type: "upsert_graph_node",
          id: "node-b",
          name: "Node B",
          entity_type: "Topic",
        },
        {
          type: "upsert_graph_edge",
          id: "edge-a-b",
          source: "node-a",
          target: "node-b",
          relation_type: "mentions",
          label: null,
          weight: 0.9,
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(2, [
        { type: "strengthen_graph_edge", id: "edge-a-b", weight_delta: 0.5 },
      ]),
    );
    expect(
      useAudioGraphStore
        .getState()
        .materializedProjectionGraph?.edges.find((e) => e.id === "edge-a-b")
        ?.weight,
    ).toBe(1);

    store.addProjectionPatch(
      graphProjectionPatch(3, [
        { type: "weaken_graph_edge", id: "edge-a-b", weight_delta: 5 },
      ]),
    );
    expect(
      useAudioGraphStore
        .getState()
        .materializedProjectionGraph?.edges.find((e) => e.id === "edge-a-b")
        ?.weight,
    ).toBe(0);
  });

  it("replays out-of-order ASR span revisions without duplicate transcript artifacts (9d93)", () => {
    const store = useAudioGraphStore.getState();
    // Final (rev 2) lands first, then the late partial (rev 1) for the SAME
    // span arrives out of order. The stale revision must neither replace the
    // canonical text nor append a second segment for the same span.
    store.addAsrSpanRevision(
      asrSpanRevision(2, {
        text: "canonical final",
        is_final: true,
        stability: "final",
        confidence: 0.97,
      }),
    );
    store.addAsrSpanRevision(
      asrSpanRevision(1, {
        text: "late partial",
        confidence: 0.5,
      }),
    );

    const state = useAudioGraphStore.getState();
    // Exactly one rendered segment for the span — no duplicate UI artifact.
    expect(state.transcriptSegments).toEqual([
      expect.objectContaining({
        id: "deepgram:system-default:0-500",
        text: "canonical final",
        confidence: 0.97,
      }),
    ]);
    // Event history retains both revisions (append-only ledger) even though the
    // visible transcript dropped the stale one.
    expect(
      state.sessionTranscriptEvents.map((event) => event.revision_number),
    ).toEqual([2, 1]);
  });

  it("ignores stale graph projection patch sequences while retaining event history", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(2, [
        {
          type: "upsert_graph_node",
          id: "node-current",
          name: "Current",
          entity_type: "Topic",
        },
      ]),
    );
    const currentGraph =
      useAudioGraphStore.getState().materializedProjectionGraph;

    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "remove_graph_node",
          id: "node-current",
        },
      ]),
    );

    const state = useAudioGraphStore.getState();
    expect(
      state.sessionProjectionEvents.map((patch) => patch.sequence),
    ).toEqual([2, 1]);
    expect(state.materializedProjectionGraph).toBe(currentGraph);
    expect(state.materializedProjectionGraph?.nodes).toEqual([
      expect.objectContaining({ id: "node-current", valid_until_ms: null }),
    ]);
  });

  it("clears projection artifact state when loading a legacy transcript only", async () => {
    useAudioGraphStore.setState({
      sessionTranscriptEvents: [{ span_id: "old" } as never],
      sessionProjectionEvents: [{ sequence: 99 } as never],
      materializedNotes: {
        schema_version: 1,
        session_id: "old",
        last_sequence: 1,
        notes: [],
      },
      materializedProjectionGraph: {
        schema_version: 1,
        session_id: "old",
        last_sequence: 1,
        nodes: [],
        edges: [],
      },
      agentProposals: [
        {
          id: "old-proposal",
          source_segment_id: "old-span",
          source_id: "system",
          speaker_label: null,
          kind: "note",
          title: "Old proposal",
          body: "Old body",
          confidence: 0.8,
          created_at_ms: 1,
        },
      ],
      liveAssistCards: [liveAssistCard("old-card")],
    });
    const transcript = [
      {
        id: "legacy-seg",
        source_id: "system-default",
        speaker_id: null,
        speaker_label: null,
        text: "legacy transcript",
        start_time: 0,
        end_time: 1,
        confidence: 0.9,
      },
    ];
    vi.mocked(invoke).mockResolvedValueOnce(transcript);

    await useAudioGraphStore.getState().loadSessionTranscript("legacy-session");

    const state = useAudioGraphStore.getState();
    expect(state.transcriptSegments).toEqual(transcript);
    expect(state.sessionTranscriptEvents).toEqual([]);
    expect(state.sessionProjectionEvents).toEqual([]);
    expect(state.materializedNotes).toBeNull();
    expect(state.materializedProjectionGraph).toBeNull();
    expect(state.agentProposals).toEqual([]);
    expect(state.liveAssistCards).toEqual([]);
  });

  it("clears ASR revision state when loading a transcript-only session", async () => {
    const store = useAudioGraphStore.getState();
    store.setAsrPartial({
      provider: "deepgram",
      source_id: "system-default",
      text: "old partial",
      start_time: 0,
      end_time: 0.5,
      confidence: 0.5,
      timestamp_ms: 1_700_000_000_000,
    });
    store.addAsrSpanRevision(
      asrSpanRevision(4, {
        text: "old transcript final",
        is_final: true,
        stability: "final",
      }),
    );
    const transcript = [
      {
        id: "legacy-seg-new",
        source_id: "system-default",
        speaker_id: null,
        speaker_label: null,
        text: "loaded transcript",
        start_time: 0,
        end_time: 1,
        confidence: 0.9,
      },
    ];
    vi.mocked(invoke).mockResolvedValueOnce(transcript);

    await store.loadSessionTranscript("legacy-session-2");
    useAudioGraphStore.getState().addAsrSpanRevision(
      asrSpanRevision(1, {
        text: "new transcript session partial",
      }),
    );

    const state = useAudioGraphStore.getState();
    expect(state.asrPartial).toBeNull();
    expect(
      state.asrSpanRevisions.map((revision) => revision.revision_number),
    ).toEqual([1]);
    expect(state.transcriptSegments).toEqual([
      transcript[0],
      expect.objectContaining({
        id: "deepgram:system-default:0-500",
        text: "new transcript session partial",
      }),
    ]);
  });

  it("rolls back already-started capture sources if a later source fails", async () => {
    useAudioGraphStore.setState({
      selectedSourceIds: ["system-default", "device:mic"],
    });
    vi.mocked(invoke).mockImplementation(async (cmd, args) => {
      if (cmd === "start_capture") {
        const sourceId = (args as { sourceId: string }).sourceId;
        if (sourceId === "device:mic") {
          throw new Error("device unavailable");
        }
      }
      return undefined;
    });

    await useAudioGraphStore.getState().startCapture();

    expect(invoke).toHaveBeenCalledWith("start_capture", {
      sourceId: "system-default",
    });
    expect(invoke).toHaveBeenCalledWith("start_capture", {
      sourceId: "device:mic",
    });
    expect(invoke).toHaveBeenCalledWith("stop_capture", {
      sourceId: "system-default",
    });
    expect(useAudioGraphStore.getState().isCapturing).toBe(false);
    expect(useAudioGraphStore.getState().error).toMatch(/device unavailable/i);
  });

  it("passes the selected backend source descriptor when starting capture", async () => {
    const source: AudioSourceInfo = {
      id: "opaque-rsac-row",
      name: "Studio Mic",
      source_type: { type: "Device", device_id: "mic-1" },
      capture_target: "device:mic-1",
      device_kind: "Input",
      channel_provenance: {
        layout: "SourceNative",
        provenance: "Physical",
        source_native: true,
        channel_count: 2,
        channels: [
          {
            index: 0,
            id: "mic-left",
            label: "Left",
            provenance: "Physical",
          },
          {
            index: 1,
            id: "mic-right",
            label: "Right",
            provenance: "Physical",
          },
        ],
        negotiated_format: {
          sample_rate: 48000,
          channels: 2,
          sample_format: "F32",
        },
      },
      is_active: false,
    };
    useAudioGraphStore.setState({
      selectedSourceIds: ["device:mic-1"],
      audioSources: [source],
    });

    await useAudioGraphStore.getState().startCapture();

    expect(invoke).toHaveBeenCalledWith("start_capture", {
      sourceId: "device:mic-1",
      source,
    });
  });

  it("keeps legacy start_capture arguments when no descriptor matches", async () => {
    useAudioGraphStore.setState({
      selectedSourceIds: ["device:stale"],
      audioSources: [
        {
          id: "other-row",
          name: "Other Mic",
          source_type: { type: "Device", device_id: "other" },
          capture_target: "device:other",
          is_active: false,
        },
      ],
    });

    await useAudioGraphStore.getState().startCapture();

    expect(invoke).toHaveBeenCalledWith("start_capture", {
      sourceId: "device:stale",
    });
  });

  it("approves agent proposals by id and records the result", async () => {
    useAudioGraphStore.getState().addAgentProposal({
      id: "proposal-1",
      source_segment_id: "segment-1",
      source_id: "system",
      speaker_label: "Speaker 1",
      kind: "graph_suggestion",
      title: "Possible graph update",
      body: "Review this for a relationship: Alice met Bob.",
      confidence: 0.91,
      created_at_ms: 10,
    });
    const approvedCard = liveAssistCard("proposal-1", {
      status: "approved",
      proposal: {
        id: "proposal-1",
        source_segment_id: "segment-1",
        source_id: "system",
        speaker_label: "Speaker 1",
        kind: "graph_suggestion",
        title: "Possible graph update",
        body: "Review this for a relationship: Alice met Bob.",
        confidence: 0.91,
        created_at_ms: 10,
      },
      outcome: {
        proposal_id: "proposal-1",
        action: "graph_update",
        message: "Approved agent proposal\n\nAlice met Bob.",
        graph_updated: true,
        timestamp_ms: 20,
      },
      projection_patch_sequence: 4,
      updated_at_ms: 20,
    });
    vi.mocked(invoke).mockResolvedValueOnce(approvedCard);

    const result = await useAudioGraphStore
      .getState()
      .approveAgentProposal("proposal-1");

    expect(invoke).toHaveBeenCalledWith("approve_agent_proposal", {
      proposalId: "proposal-1",
    });
    expect(result?.graph_updated).toBe(true);
    expect(useAudioGraphStore.getState().agentProposals).toEqual([]);
    expect(useAudioGraphStore.getState().liveAssistCards).toEqual([
      approvedCard,
    ]);
    expect(useAudioGraphStore.getState().approvingAgentProposalIds).toEqual([]);
    expect(useAudioGraphStore.getState().chatMessages).toContainEqual({
      role: "assistant",
      content: "Approved agent proposal\n\nAlice met Bob.",
    });
  });

  it("does not approve the same proposal twice while the request is pending", async () => {
    let resolveInvoke: (value: unknown) => void = () => {};
    vi.mocked(invoke).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveInvoke = resolve;
        }),
    );
    useAudioGraphStore.getState().addAgentProposal({
      id: "proposal-2",
      source_segment_id: "segment-2",
      source_id: "system",
      speaker_label: null,
      kind: "note",
      title: "Context",
      body: "Remember this.",
      confidence: 0.8,
      created_at_ms: 30,
    });

    const first = useAudioGraphStore
      .getState()
      .approveAgentProposal("proposal-2");
    const second = await useAudioGraphStore
      .getState()
      .approveAgentProposal("proposal-2");

    expect(second).toBeNull();
    expect(invoke).toHaveBeenCalledTimes(1);
    expect(useAudioGraphStore.getState().approvingAgentProposalIds).toEqual([
      "proposal-2",
    ]);

    resolveInvoke(
      liveAssistCard("proposal-2", {
        status: "approved",
        outcome: {
          proposal_id: "proposal-2",
          action: "chat_note",
          message: "Approved agent proposal for review\n\nRemember this.",
          graph_updated: false,
          timestamp_ms: 40,
        },
        projection_patch_sequence: 5,
        updated_at_ms: 40,
      }),
    );
    await first;

    expect(useAudioGraphStore.getState().approvingAgentProposalIds).toEqual([]);
  });

  it("dismisses agent proposals by upserting the returned live-assist card", async () => {
    const dismissedCard = liveAssistCard("proposal-dismiss", {
      status: "dismissed",
      updated_at_ms: 50,
    });
    vi.mocked(invoke).mockResolvedValueOnce(dismissedCard);
    useAudioGraphStore.getState().addAgentProposal({
      id: "proposal-dismiss",
      source_segment_id: "segment-dismiss",
      source_id: "system",
      speaker_label: null,
      kind: "note",
      title: "Dismiss me",
      body: "No longer needed",
      confidence: 0.7,
      created_at_ms: 45,
    });

    const result = await useAudioGraphStore
      .getState()
      .dismissAgentProposal("proposal-dismiss");

    expect(invoke).toHaveBeenCalledWith("dismiss_agent_proposal", {
      proposalId: "proposal-dismiss",
    });
    expect(result).toEqual(dismissedCard);
    expect(useAudioGraphStore.getState().agentProposals).toEqual([]);
    expect(useAudioGraphStore.getState().liveAssistCards).toEqual([
      dismissedCard,
    ]);
  });

  it("asks AI from a question card after preserving the dismissed live-assist record", async () => {
    const dismissedCard = liveAssistCard("proposal-question", {
      status: "dismissed",
      proposal: {
        id: "proposal-question",
        source_segment_id: "segment-question",
        source_id: "system",
        speaker_label: "Speaker 1",
        kind: "question",
        title: "Question",
        body: "Consider answering or linking this question: What changed?",
        confidence: 0.85,
        created_at_ms: 70,
      },
      updated_at_ms: 75,
    });
    vi.mocked(invoke)
      .mockResolvedValueOnce(true)
      .mockResolvedValueOnce(dismissedCard)
      .mockResolvedValueOnce("stream-1");
    useAudioGraphStore.getState().addAgentProposal({
      id: "proposal-question",
      source_segment_id: "segment-question",
      source_id: "system",
      speaker_label: "Speaker 1",
      kind: "question",
      title: "Question",
      body: "Consider answering or linking this question: What changed?",
      confidence: 0.85,
      created_at_ms: 70,
    });

    await useAudioGraphStore.getState().askAgentProposal("proposal-question");

    expect(invoke).toHaveBeenNthCalledWith(1, "add_question_to_graph", {
      text: "What changed?",
      speaker: "Speaker 1",
      sourceSegmentId: "segment-question",
    });
    expect(invoke).toHaveBeenNthCalledWith(2, "dismiss_agent_proposal", {
      proposalId: "proposal-question",
    });
    expect(invoke).toHaveBeenNthCalledWith(3, "start_streaming_chat", {
      message: "What changed?",
      channel: expect.any(Channel),
    });
    expect(useAudioGraphStore.getState().agentProposals).toEqual([]);
    expect(useAudioGraphStore.getState().liveAssistCards).toEqual([
      dismissedCard,
    ]);
  });

  it("clears pending proposals while preserving returned resolved-card history", async () => {
    const firstCard = liveAssistCard("proposal-clear-1", {
      status: "dismissed",
      updated_at_ms: 60,
    });
    const secondCard = liveAssistCard("proposal-clear-2", {
      status: "dismissed",
      updated_at_ms: 61,
    });
    vi.mocked(invoke).mockResolvedValueOnce([firstCard, secondCard]);
    useAudioGraphStore.getState().addAgentProposal({
      id: "proposal-clear-1",
      source_segment_id: "segment-clear-1",
      source_id: "system",
      speaker_label: null,
      kind: "note",
      title: "Clear first",
      body: "First body",
      confidence: 0.7,
      created_at_ms: 45,
    });
    useAudioGraphStore.getState().addAgentProposal({
      id: "proposal-clear-2",
      source_segment_id: "segment-clear-2",
      source_id: "system",
      speaker_label: null,
      kind: "note",
      title: "Clear second",
      body: "Second body",
      confidence: 0.8,
      created_at_ms: 46,
    });

    const result = await useAudioGraphStore.getState().clearAgentProposals();

    expect(invoke).toHaveBeenCalledWith("clear_agent_proposals");
    expect(result).toEqual([firstCard, secondCard]);
    expect(useAudioGraphStore.getState().agentProposals).toEqual([]);
    expect(useAudioGraphStore.getState().liveAssistCards).toEqual([
      secondCard,
      firstCard,
    ]);
  });

  it("does not dismiss or clear proposals while any approval is in flight", async () => {
    useAudioGraphStore.getState().addAgentProposal({
      id: "proposal-busy",
      source_segment_id: "segment-busy",
      source_id: "system",
      speaker_label: null,
      kind: "note",
      title: "Busy",
      body: "Approval is running",
      confidence: 0.8,
      created_at_ms: 90,
    });
    useAudioGraphStore.setState({
      approvingAgentProposalIds: ["proposal-busy"],
    });

    const dismissed = await useAudioGraphStore
      .getState()
      .dismissAgentProposal("proposal-busy");
    const cleared = await useAudioGraphStore.getState().clearAgentProposals();

    expect(dismissed).toBeNull();
    expect(cleared).toEqual([]);
    expect(invoke).not.toHaveBeenCalledWith("dismiss_agent_proposal", {
      proposalId: "proposal-busy",
    });
    expect(invoke).not.toHaveBeenCalledWith("clear_agent_proposals");
    expect(useAudioGraphStore.getState().agentProposals).toHaveLength(1);
  });

  // -----------------------------------------------------------------------
  // Streaming chat (plan A3 / ADR-0006)
  // -----------------------------------------------------------------------

  it("appends streaming-chat token deltas onto the assistant placeholder", () => {
    // Simulate the user-message + assistant-placeholder shape that
    // sendChatMessage installs before invoking start_streaming_chat.
    useAudioGraphStore.setState({
      chatMessages: [
        { role: "user", content: "What did Alice say?" },
        { role: "assistant", content: "" },
      ],
      isChatLoading: true,
      streamingChatRequestId: "req-stream-1",
    });

    useAudioGraphStore.getState().appendChatTokenDelta({
      request_id: "req-stream-1",
      delta: "Alice ",
    });
    useAudioGraphStore.getState().appendChatTokenDelta({
      request_id: "req-stream-1",
      delta: "said ",
    });
    useAudioGraphStore.getState().appendChatTokenDelta({
      request_id: "req-stream-1",
      delta: "hello.",
    });

    const messages = useAudioGraphStore.getState().chatMessages;
    expect(messages).toHaveLength(2);
    expect(messages[1]).toEqual({
      role: "assistant",
      content: "Alice said hello.",
    });
  });

  // audio-graph-1534: the streaming-chat hot path is delivered over a
  // per-invocation `tauri::ipc::Channel<ChatStreamEvent>` that sendChatMessage
  // creates and passes as the invoke arg. These tests capture that channel
  // from the mocked invoke args and drive `channel.onmessage` with the same
  // discriminated `{ event, data }` frames the Rust `channel.send()` end emits.
  type ChannelLike = { onmessage: ((m: unknown) => void) | null };
  function captureStreamChannel(): {
    getChannel: () => ChannelLike;
    resolveStart: (id: string) => void;
  } {
    let channel: ChannelLike | null = null;
    let resolveStart: (id: string) => void = () => {};
    vi.mocked(invoke).mockImplementation(async (cmd, args) => {
      if (cmd === "start_streaming_chat") {
        const argsRecord = args as { channel?: ChannelLike } | undefined;
        channel = argsRecord?.channel ?? null;
        return new Promise<string>((resolve) => {
          resolveStart = resolve;
        });
      }
      return undefined;
    });
    return {
      getChannel: () => {
        if (channel === null) throw new Error("channel not captured yet");
        return channel;
      },
      resolveStart: (id: string) => resolveStart(id),
    };
  }

  it("streams channel delta frames onto the placeholder and finalizes on done (audio-graph-1534)", async () => {
    vi.useFakeTimers();
    try {
      const reqId = "req-chan-1";
      const { getChannel, resolveStart } = captureStreamChannel();

      const sendPromise = useAudioGraphStore.getState().sendChatMessage("hi");
      // Channel is created + onmessage wired synchronously, before the invoke
      // resolves — so the handler exists immediately.
      const channel = getChannel();
      expect(channel.onmessage).toBeTypeOf("function");

      // Arm the id (invoke resolves), then stream delta frames.
      resolveStart(reqId);
      await sendPromise;
      expect(useAudioGraphStore.getState().streamingChatRequestId).toBe(reqId);

      channel.onmessage?.({
        event: "delta",
        data: { request_id: reqId, delta: "Alice " },
      });
      channel.onmessage?.({
        event: "delta",
        data: { request_id: reqId, delta: "said hi." },
      });
      // Deltas are coalesced (33ms) — flush the timer to apply the batch.
      vi.advanceTimersByTime(40);
      expect(useAudioGraphStore.getState().chatMessages.at(-1)?.content).toBe(
        "Alice said hi.",
      );

      // Done frame drains any queued delta synchronously, then finalizes with
      // the authoritative full_text.
      channel.onmessage?.({
        event: "done",
        data: {
          request_id: reqId,
          full_text: "Alice said hi. (final)",
          finish_reason: "stop",
        },
      });
      const s = useAudioGraphStore.getState();
      expect(s.chatMessages.at(-1)?.content).toBe("Alice said hi. (final)");
      expect(s.isChatLoading).toBe(false);
      expect(s.streamingChatRequestId).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("holds channel frames that arrive before the request id is armed, then applies them (audio-graph-1534)", async () => {
    vi.useFakeTimers();
    try {
      const reqId = "req-chan-early";
      const { getChannel, resolveStart } = captureStreamChannel();

      const sendPromise = useAudioGraphStore.getState().sendChatMessage("hi");
      const channel = getChannel();
      // Frames land BEFORE the invoke resolves (backend sends inside the
      // command before returning the id). They must not be dropped.
      channel.onmessage?.({
        event: "delta",
        data: { request_id: reqId, delta: "Lead " },
      });
      expect(useAudioGraphStore.getState().streamingChatRequestId).toBeNull();
      // Nothing applied yet — the id isn't armed, so the closure holds it.
      expect(useAudioGraphStore.getState().chatMessages.at(-1)?.content).toBe(
        "",
      );

      // Arming the id drains the held delta immediately.
      resolveStart(reqId);
      await sendPromise;
      expect(useAudioGraphStore.getState().chatMessages.at(-1)?.content).toBe(
        "Lead ",
      );

      // A subsequent live delta appends after the leading tokens.
      channel.onmessage?.({
        event: "delta",
        data: { request_id: reqId, delta: "tail." },
      });
      vi.advanceTimersByTime(40);
      expect(useAudioGraphStore.getState().chatMessages.at(-1)?.content).toBe(
        "Lead tail.",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("applies a done frame that arrives before the id is armed with authoritative full_text (audio-graph-1534)", async () => {
    const reqId = "req-chan-done-early";
    const { getChannel, resolveStart } = captureStreamChannel();

    const sendPromise = useAudioGraphStore.getState().sendChatMessage("hi");
    const channel = getChannel();
    // A stale leading delta, then a done frame — both before the invoke
    // resolves. The done's full_text is authoritative; the stale lead must
    // not leak into the finalized message.
    channel.onmessage?.({
      event: "delta",
      data: { request_id: reqId, delta: "stale " },
    });
    channel.onmessage?.({
      event: "done",
      data: {
        request_id: reqId,
        full_text: "the real reply",
        finish_reason: "stop",
      },
    });
    // Nothing applied yet (id not armed).
    expect(useAudioGraphStore.getState().chatMessages.at(-1)?.content).toBe("");

    resolveStart(reqId);
    await sendPromise;

    const s = useAudioGraphStore.getState();
    expect(s.chatMessages.at(-1)?.content).toBe("the real reply");
    expect(s.isChatLoading).toBe(false);
    expect(s.streamingChatRequestId).toBeNull();
  });

  it("ignores token deltas for a stale request_id", () => {
    useAudioGraphStore.setState({
      chatMessages: [
        { role: "user", content: "ping" },
        { role: "assistant", content: "" },
      ],
      isChatLoading: true,
      streamingChatRequestId: "active-req",
    });

    useAudioGraphStore.getState().appendChatTokenDelta({
      request_id: "stale-req",
      delta: "should-not-appear",
    });

    const messages = useAudioGraphStore.getState().chatMessages;
    expect(messages[1].content).toBe("");
  });

  it("finalizes a streaming chat with the authoritative full_text", () => {
    useAudioGraphStore.setState({
      chatMessages: [
        { role: "user", content: "What time is it?" },
        { role: "assistant", content: "It is " },
      ],
      isChatLoading: true,
      streamingChatRequestId: "req-final",
    });

    useAudioGraphStore.getState().finalizeChatStream({
      request_id: "req-final",
      full_text: "It is 3 o'clock.",
      finish_reason: "stop",
    });

    const s = useAudioGraphStore.getState();
    expect(s.isChatLoading).toBe(false);
    expect(s.streamingChatRequestId).toBeNull();
    expect(s.chatMessages[1]).toEqual({
      role: "assistant",
      content: "It is 3 o'clock.",
    });
  });

  // -----------------------------------------------------------------------
  // Converse-toggle routing (B18 #46) — startGemini/stopGemini must route to
  // the native S2S converse commands when native-converse is active, and stay
  // on the Gemini Live (notes/text) pipeline otherwise.
  // -----------------------------------------------------------------------

  it("routes startGemini to start_converse in native + converse mode", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      isCapturing: true,
      isGeminiActive: false,
      activeGeminiCommand: null,
      conversationMode: "converse",
      converseEngine: "native",
    });

    await useAudioGraphStore.getState().startGemini();

    expect(invoke).toHaveBeenCalledWith("start_converse");
    expect(invoke).not.toHaveBeenCalledWith("start_gemini");
    const s = useAudioGraphStore.getState();
    expect(s.isGeminiActive).toBe(true);
    expect(s.activeGeminiCommand).toBe("start_converse");
  });

  it("keeps startGemini on start_gemini in notes mode", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      isCapturing: true,
      isGeminiActive: false,
      activeGeminiCommand: null,
      conversationMode: "notes",
      converseEngine: "native",
    });

    await useAudioGraphStore.getState().startGemini();

    expect(invoke).toHaveBeenCalledWith("start_gemini");
    expect(invoke).not.toHaveBeenCalledWith("start_converse");
    expect(useAudioGraphStore.getState().activeGeminiCommand).toBe(
      "start_gemini",
    );
  });

  it("keeps startGemini on start_gemini for pipelined converse", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      isCapturing: true,
      isGeminiActive: false,
      activeGeminiCommand: null,
      conversationMode: "converse",
      converseEngine: "pipelined",
    });

    await useAudioGraphStore.getState().startGemini();

    expect(invoke).toHaveBeenCalledWith("start_gemini");
    expect(invoke).not.toHaveBeenCalledWith("start_converse");
  });

  it("stopGemini calls stop_converse when converse session is active", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      isGeminiActive: true,
      activeGeminiCommand: "start_converse",
    });

    await useAudioGraphStore.getState().stopGemini();

    expect(invoke).toHaveBeenCalledWith("stop_converse");
    expect(invoke).not.toHaveBeenCalledWith("stop_gemini");
    const s = useAudioGraphStore.getState();
    expect(s.isGeminiActive).toBe(false);
    expect(s.activeGeminiCommand).toBeNull();
  });

  it("stopGemini calls stop_gemini when the Gemini Live pipeline is active", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      isGeminiActive: true,
      activeGeminiCommand: "start_gemini",
    });

    await useAudioGraphStore.getState().stopGemini();

    expect(invoke).toHaveBeenCalledWith("stop_gemini");
    expect(invoke).not.toHaveBeenCalledWith("stop_converse");
    expect(useAudioGraphStore.getState().activeGeminiCommand).toBeNull();
  });

  it("stopGemini tears down BOTH backends defensively when the active command is unknown (FINDING #57 P3)", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      isGeminiActive: true,
      activeGeminiCommand: null,
    });

    await useAudioGraphStore.getState().stopGemini();

    // Both idempotent stop commands fire so a live converse session is not
    // left running by a default-to-stop_gemini guess.
    expect(invoke).toHaveBeenCalledWith("stop_converse");
    expect(invoke).toHaveBeenCalledWith("stop_gemini");
    const s = useAudioGraphStore.getState();
    expect(s.isGeminiActive).toBe(false);
    expect(s.activeGeminiCommand).toBeNull();
    expect(s.error).toBeNull();
  });

  it("stopGemini surfaces an error if a defensive stop rejects (unknown command branch)", async () => {
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "stop_converse") throw new Error("converse teardown failed");
      return undefined;
    });
    useAudioGraphStore.setState({
      isGeminiActive: true,
      activeGeminiCommand: null,
    });

    await useAudioGraphStore.getState().stopGemini();

    // stop_gemini still gets attempted (allSettled), and the rejection
    // surfaces in the error banner rather than being swallowed.
    expect(invoke).toHaveBeenCalledWith("stop_gemini");
    expect(useAudioGraphStore.getState().error).toMatch(
      /converse teardown failed/i,
    );
  });
});

// ---------------------------------------------------------------------------
// Graph delta reducer (regression coverage for the edge-id mismatch bug)
// ---------------------------------------------------------------------------

describe("graph delta reducer", () => {
  const node = (id: string) => ({
    id,
    name: id,
    entity_type: "Person",
    val: 1,
    color: "#ffffff",
    first_seen: 0,
    last_seen: 0,
    mention_count: 1,
  });

  const seed = () =>
    useAudioGraphStore.getState().setGraphSnapshot({
      nodes: [node("a"), node("b")],
      links: [
        {
          id: "edge-EdgeIndex(0)",
          source: "a",
          target: "b",
          relation_type: "knows",
          weight: 1,
          color: "#999999",
          label: "knows",
        },
      ],
      stats: { total_nodes: 2, total_edges: 1, total_episodes: 0 },
    });

  const emptyDelta = {
    added_nodes: [],
    updated_nodes: [],
    added_edges: [],
    updated_edges: [],
    removed_node_ids: [],
    removed_edge_ids: [],
    timestamp: 1,
  };

  it("removes a link whose id is in removed_edge_ids (eviction must match)", () => {
    seed();
    expect(useAudioGraphStore.getState().graphSnapshot.links).toHaveLength(1);

    // Backend eviction now emits the SAME `edge-{idx}` id the link carries.
    useAudioGraphStore.getState().applyGraphDelta({
      ...emptyDelta,
      removed_edge_ids: ["edge-EdgeIndex(0)"],
    });

    expect(useAudioGraphStore.getState().graphSnapshot.links).toHaveLength(0);
  });

  it("does NOT remove a link when the removal id uses the old evicted scheme", () => {
    seed();
    // The pre-fix `edge-evicted-{idx}` id should not match — this asserts the
    // failure mode the bug produced, guarding the id contract.
    useAudioGraphStore.getState().applyGraphDelta({
      ...emptyDelta,
      removed_edge_ids: ["edge-evicted-EdgeIndex(0)"],
    });
    expect(useAudioGraphStore.getState().graphSnapshot.links).toHaveLength(1);
  });

  it("merges updated_edges weight onto an existing link", () => {
    seed();
    useAudioGraphStore.getState().applyGraphDelta({
      ...emptyDelta,
      updated_edges: [
        {
          id: "edge-EdgeIndex(0)",
          source: "a",
          target: "b",
          relation_type: "knows",
          weight: 3,
          color: "#999999",
          label: "knows",
        },
      ],
    });

    const links = useAudioGraphStore.getState().graphSnapshot.links;
    expect(links).toHaveLength(1);
    expect(links[0].weight).toBe(3);
  });

  it("tolerates a delta with no updated_edges field (backwards compat)", () => {
    seed();
    const { updated_edges, ...legacyDelta } = emptyDelta;
    void updated_edges;
    useAudioGraphStore.getState().applyGraphDelta({
      ...legacyDelta,
      added_edges: [
        {
          id: "edge-EdgeIndex(1)",
          source: "b",
          target: "a",
          relation_type: "knows",
          weight: 1,
          color: "#999999",
          label: "knows",
        },
      ],
    });
    expect(useAudioGraphStore.getState().graphSnapshot.links).toHaveLength(2);
  });

  it("treats a graph snapshot as authoritative after earlier deltas", () => {
    seed();
    useAudioGraphStore.getState().applyGraphDelta({
      ...emptyDelta,
      added_nodes: [node("transient")],
    });
    expect(
      useAudioGraphStore
        .getState()
        .graphSnapshot.nodes.some((node) => node.id === "transient"),
    ).toBe(true);

    useAudioGraphStore.getState().setGraphSnapshot({
      nodes: [node("authoritative")],
      links: [],
      stats: { total_nodes: 1, total_edges: 0, total_episodes: 0 },
    });

    const nodeIds = useAudioGraphStore
      .getState()
      .graphSnapshot.nodes.map((node) => node.id);
    expect(nodeIds).toEqual(["authoritative"]);
  });
});
