import { Channel, invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import type {
  AgentProposalEvent,
  AppSettings,
  AsrSpanRevisionEvent,
  AudioGraphStore,
  AudioSourceInfo,
  LiveAssistCardRecord,
  ProjectionPatch,
} from "../types";
import { useAudioGraphStore } from "./index";

function selectableSettings(overrides: Partial<AppSettings> = {}): AppSettings {
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
    ...overrides,
  };
}

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

/**
 * Thin `setState` alias for tests that only need to layer a few fields on
 * top of the baseline the outer `beforeEach` already reset — named so the
 * answer-card action tests below read as "reset to this shape" rather than
 * a bare `setState` call, matching this file's existing house style
 * (`resetStore` in `AgentProposalsPanel.test.tsx` is the same idea, one
 * level up at the component layer).
 */
function resetStoreForTest(overrides: Partial<AudioGraphStore> = {}) {
  useAudioGraphStore.setState(overrides);
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
      // audio-graph-83cc T4: reset between tests same as every other agent
      // slice above it — otherwise a draft/composer error set by one test
      // could leak into an unrelated later test in this file.
      answerDrafts: {},
      composerError: null,
      // audio-graph-83cc T5: same reasoning — a dispatch count or an
      // attempted-id from one test must never leak into another.
      autoAnswerDispatchCount: 0,
      autoAnswerAttemptedProposalIds: new Set(),
      chatMessages: [],
      isChatLoading: false,
      streamingChatRequestId: null,
      isCapturing: false,
      isTranscribing: false,
      captureStartTime: null,
      loadedSessionId: null,
      error: null,
      graphSnapshot: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
      speakers: [],
      transcriptSeekTarget: null,
      graphEdgeFocus: null,
    });
  });

  it("starts with empty state", () => {
    const s = useAudioGraphStore.getState();
    expect(s.audioSources).toEqual([]);
    expect(s.selectedSourceIds).toEqual([]);
    expect(s.isCapturing).toBe(false);
  });

  it("reconciles frontend lifecycle from authoritative backend pipeline status", () => {
    useAudioGraphStore.getState().setPipelineStatus({
      capture: { type: "Running", processed_count: 0 },
      pipeline: { type: "Running", processed_count: 0 },
      asr: { type: "Running", processed_count: 0 },
      diarization: { type: "Running", processed_count: 0 },
      entity_extraction: { type: "Running", processed_count: 0 },
      graph: { type: "Running", processed_count: 0 },
    });
    expect(useAudioGraphStore.getState().isCapturing).toBe(true);
    expect(useAudioGraphStore.getState().isTranscribing).toBe(true);
    expect(useAudioGraphStore.getState().captureStartTime).not.toBeNull();

    useAudioGraphStore.getState().setPipelineStatus({
      capture: { type: "Idle" },
      pipeline: { type: "Idle" },
      asr: { type: "Idle" },
      diarization: { type: "Idle" },
      entity_extraction: { type: "Idle" },
      graph: { type: "Idle" },
    });
    expect(useAudioGraphStore.getState().isCapturing).toBe(false);
    expect(useAudioGraphStore.getState().isTranscribing).toBe(false);
    expect(useAudioGraphStore.getState().captureStartTime).toBeNull();
  });

  it("starts transcription for selected MVP-enabled ASR and LLM providers", async () => {
    useAudioGraphStore.setState({
      isCapturing: true,
      isTranscribing: false,
      settings: selectableSettings(),
    });
    vi.mocked(invoke).mockResolvedValue(undefined);

    await useAudioGraphStore.getState().startTranscribe();

    expect(invoke).toHaveBeenCalledWith("start_transcribe");
    expect(useAudioGraphStore.getState().isTranscribing).toBe(true);
    expect(useAudioGraphStore.getState().error).toBeNull();
  });

  it("blocks persisted deferred ASR without invoking or clearing sample preview", async () => {
    useAudioGraphStore.setState({
      isCapturing: true,
      isTranscribing: false,
      samplePreviewActive: true,
      settings: selectableSettings({
        asr_provider: { type: "local_whisper" },
      }),
    });

    await useAudioGraphStore.getState().startTranscribe();

    expect(invoke).not.toHaveBeenCalled();
    expect(useAudioGraphStore.getState().isTranscribing).toBe(false);
    expect(useAudioGraphStore.getState().samplePreviewActive).toBe(true);
    expect(useAudioGraphStore.getState().error).toMatch(
      /Local Whisper.*new sessions.*current MVP/i,
    );
  });

  it("fails closed while provider settings are not hydrated", async () => {
    useAudioGraphStore.setState({
      isCapturing: true,
      settings: null,
    });

    await useAudioGraphStore.getState().startTranscribe();

    expect(invoke).not.toHaveBeenCalled();
    expect(useAudioGraphStore.getState().error).toMatch(
      /provider settings are still loading/i,
    );
  });

  it("does not optimistically mutate chat before provider settings hydrate", async () => {
    useAudioGraphStore.setState({
      settings: null,
      chatMessages: [],
      isChatLoading: false,
    });

    await useAudioGraphStore.getState().sendChatMessage("private draft");

    expect(invoke).not.toHaveBeenCalled();
    expect(useAudioGraphStore.getState().chatMessages).toEqual([]);
    expect(useAudioGraphStore.getState().isChatLoading).toBe(false);
    expect(useAudioGraphStore.getState().error).toMatch(
      /provider settings are still loading/i,
    );
  });

  it("localizes the settings-hydration fail-closed path", async () => {
    await i18n.changeLanguage("pt");
    try {
      useAudioGraphStore.setState({
        isCapturing: true,
        settings: null,
      });

      await useAudioGraphStore.getState().startTranscribe();

      expect(invoke).not.toHaveBeenCalled();
      expect(useAudioGraphStore.getState().error).toMatch(
        /configurações dos provedores ainda estão carregando/i,
      );
    } finally {
      await i18n.changeLanguage("en");
    }
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

  it("saveSettings sets the global error AND rethrows on a failed persist (seed 9289)", async () => {
    // Regression guard (Codex P2): saveSettings used to swallow the invoke
    // rejection after recording the global error, so the Settings controller
    // could not observe the failure — it cleared its inline error, reset the
    // dirty baseline, and toasted success on a FAILED save. The action must
    // keep the global error surface and rethrow to its caller.
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "save_settings_cmd") {
        throw new Error("save_settings_cmd exploded");
      }
      return undefined;
    });

    const draft = {
      asr_provider: { type: "local_whisper" },
      whisper_model: "ggml-small.en.bin",
      llm_provider: { type: "local_llama" },
      llm_api_config: null,
      audio_settings: { sample_rate: 48000, channels: 2 },
      gemini: {
        auth: { type: "api_key", api_key: "" },
        model: "gemini-2.0-flash-live-001",
      },
      tts_provider: { type: "none" },
      speak_aloud: false,
      log_level: "info",
    } as AppSettings;

    await expect(
      useAudioGraphStore.getState().saveSettings(draft),
    ).rejects.toThrow(/save_settings_cmd exploded/);
    expect(useAudioGraphStore.getState().error).toMatch(
      /save_settings_cmd exploded/,
    );
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
      nodes: [
        {
          id: "node-1",
          name: "Canonical node",
          entity_type: "Topic",
          description: null,
          confidence: 0.9,
          valid_from_ms: 0,
          valid_until_ms: null,
          updated_by_sequence: 1,
          updated_at_ms: 1_700_000_000_001,
          basis: { transcript_hash: "fnv1a64:test" },
          provenance: {
            provider: "test",
            model: "test",
            prompt_id: "graph-v1",
          },
        },
      ],
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
    // Notes/materialized-graph/projection-events moved off `load_session`
    // into their own lens-fetch commands (seed audio-graph-4fa5 deliverable
    // a) — `load_session`'s own response no longer carries them.
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "load_session") {
        return {
          transcript,
          graph: {
            nodes: [],
            links: [],
            stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
          },
          transcript_events: transcriptEvents,
          diarization_events: diarizationEvents,
          live_assist_cards: [pendingCard, approvedCard],
        };
      }
      if (command === "load_session_notes_artifacts_cmd") {
        return { notes, projection_events: projectionEvents };
      }
      if (command === "load_session_graph_artifact_cmd") {
        return materializedGraph;
      }
      if (command === "build_session_timeline_cmd")
        return { entries: [], total_count: 0 };
      return undefined;
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
    // `load_session` alone must not carry the heavy lenses — they are idle
    // until their own lens-fetch action runs.
    expect(state.sessionProjectionEvents).toEqual([]);
    expect(state.materializedNotes).toBeNull();
    expect(state.materializedProjectionGraph).toBeNull();
    expect(state.notesLensStatus).toEqual({ type: "idle" });
    expect(state.graphLensStatus).toEqual({ type: "idle" });
    expect(state.graphSnapshot.nodes).toEqual([]);
    expect(state.liveAssistCards).toEqual([pendingCard, approvedCard]);
    expect(state.agentProposals).toEqual([]);
    // Loading a historical session records its id so the data-route / privacy
    // report (seed audio-graph-51e0) can fetch its data-movement ledger.
    expect(state.loadedSessionId).toBe("session-1");

    // The Notes lens (default-active) and Graph lens each fetch their own
    // artifacts independently once activated.
    await useAudioGraphStore.getState().loadSessionNotesArtifacts("session-1");
    await useAudioGraphStore.getState().loadSessionGraphArtifact("session-1");
    const afterLenses = useAudioGraphStore.getState();
    expect(invoke).toHaveBeenCalledWith("load_session_notes_artifacts_cmd", {
      sessionId: "session-1",
    });
    expect(invoke).toHaveBeenCalledWith("load_session_graph_artifact_cmd", {
      sessionId: "session-1",
    });
    expect(afterLenses.sessionProjectionEvents).toEqual(projectionEvents);
    expect(afterLenses.materializedNotes).toEqual(notes);
    expect(afterLenses.materializedProjectionGraph).toEqual(materializedGraph);
    expect(afterLenses.notesLensStatus).toEqual({ type: "ready" });
    expect(afterLenses.graphLensStatus).toEqual({ type: "ready" });
    // The Graph lens's materialized artifact now wins over the live snapshot
    // via `useActiveGraphSnapshot`'s fallback rule — pinned separately in
    // that hook's own tests; here we only confirm the store field landed.
  });

  // seed audio-graph-4fa5 deliverable b: a byte-ceiling refusal from
  // `load_session_notes_artifacts_cmd` must set `notesLensStatus` to the
  // typed `refused` shape (never throw to the caller, never fall back to
  // the generic `error` string) so `SessionsBrowser` can render the
  // dedicated refusal notice instead of a blank panel.
  it("loadSessionNotesArtifacts sets notesLensStatus to refused on an artifact_too_large rejection", async () => {
    useAudioGraphStore.setState({ loadedSessionId: "big-session" });
    vi.mocked(invoke).mockRejectedValueOnce({
      code: "artifact_too_large",
      message: {
        artifact_class: "materialized_notes",
        size_bytes: 19_063_321,
        ceiling_bytes: 8 * 1024 * 1024,
      },
    });

    await useAudioGraphStore
      .getState()
      .loadSessionNotesArtifacts("big-session");

    expect(useAudioGraphStore.getState().notesLensStatus).toEqual({
      type: "refused",
      artifactClass: "materialized_notes",
      sizeBytes: 19_063_321,
      ceilingBytes: 8 * 1024 * 1024,
    });
    // A refusal must not blank pre-existing notes/projection state.
    expect(useAudioGraphStore.getState().error).toBeNull();
  });

  it("loadSessionGraphArtifact sets graphLensStatus to refused on an artifact_too_large rejection", async () => {
    useAudioGraphStore.setState({ loadedSessionId: "big-session" });
    vi.mocked(invoke).mockRejectedValueOnce({
      code: "artifact_too_large",
      message: {
        artifact_class: "materialized_graph",
        size_bytes: 156_579_416,
        ceiling_bytes: 24 * 1024 * 1024,
      },
    });

    await useAudioGraphStore.getState().loadSessionGraphArtifact("big-session");

    expect(useAudioGraphStore.getState().graphLensStatus).toEqual({
      type: "refused",
      artifactClass: "materialized_graph",
      sizeBytes: 156_579_416,
      ceilingBytes: 24 * 1024 * 1024,
    });
  });

  it("loadSessionNotesArtifacts falls back to an error status for a non-ceiling failure", async () => {
    useAudioGraphStore.setState({ loadedSessionId: "broken-session" });
    vi.mocked(invoke).mockRejectedValueOnce({
      code: "session_invalid",
      message: { reason: "Session files not found: broken-session" },
    });

    await useAudioGraphStore
      .getState()
      .loadSessionNotesArtifacts("broken-session");

    const status = useAudioGraphStore.getState().notesLensStatus;
    expect(status.type).toBe("error");
  });

  it("keeps historical Review isolated while live capture is active", async () => {
    const historicalTranscript = [
      {
        id: "historical-segment",
        source_id: "stored-source",
        speaker_id: null,
        speaker_label: null,
        text: "already visible live transcript",
        start_time: 0,
        end_time: 1,
        confidence: 0.9,
      },
    ];
    useAudioGraphStore.setState({
      isCapturing: true,
      transcriptSegments: historicalTranscript,
      loadedSessionId: null,
    });

    const loaded = await useAudioGraphStore
      .getState()
      .loadSession("past-session");

    expect(loaded).toBeNull();
    expect(invoke).not.toHaveBeenCalledWith("load_session", expect.anything());
    expect(useAudioGraphStore.getState().transcriptSegments).toEqual(
      historicalTranscript,
    );
    expect(useAudioGraphStore.getState().loadedSessionId).toBeNull();
    expect(useAudioGraphStore.getState().error).toBe(
      i18n.t("sessions.reviewLockedWhileLive"),
    );
  });

  it("ignores an older historical load that resolves after a newer session", async () => {
    type Resolver = (value: unknown) => void;
    let resolveA: Resolver = () => {};
    let resolveB: Resolver = () => {};
    const pendingA = new Promise((resolve) => {
      resolveA = resolve;
    });
    const pendingB = new Promise((resolve) => {
      resolveB = resolve;
    });
    vi.mocked(invoke).mockImplementation(async (command, args) => {
      if (command === "load_session") {
        return (args as { sessionId: string }).sessionId === "session-a"
          ? pendingA
          : pendingB;
      }
      if (command === "build_session_timeline_cmd")
        return { entries: [], total_count: 0 };
      return undefined;
    });
    const payload = (id: string) => ({
      transcript: [
        {
          id: `${id}-segment`,
          source_id: "stored",
          speaker_id: null,
          speaker_label: null,
          text: id,
          start_time: 0,
          end_time: 1,
          confidence: 1,
        },
      ],
      graph: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
      transcript_events: [],
      diarization_events: [],
      projection_events: [],
      notes: null,
      materialized_graph: null,
      live_assist_cards: [],
    });

    const loadA = useAudioGraphStore.getState().loadSession("session-a");
    const loadB = useAudioGraphStore.getState().loadSession("session-b");
    resolveB(payload("session-b"));
    await loadB;
    resolveA(payload("session-a"));
    await loadA;

    expect(useAudioGraphStore.getState().loadedSessionId).toBe("session-b");
    expect(useAudioGraphStore.getState().transcriptSegments[0]?.text).toBe(
      "session-b",
    );
  });

  it("clears a stale pendingFinalizingSession once it successfully loads a DIFFERENT session (R2 adversary finding #1: the pending row's resident-data premise dies the moment its data is overwritten)", async () => {
    useAudioGraphStore.setState({
      pendingFinalizingSession: {
        id: "just-stopped-session",
        title: null,
        created_at: 1_700_000_000_000,
        ended_at: 1_700_000_060_000,
        duration_seconds: 60,
        status: "active",
        segment_count: 2,
        speaker_count: 1,
        entity_count: 0,
        transcript_path: "",
        graph_path: "",
        deleted: false,
        deleted_at: null,
        optimistic: true,
      },
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "load_session") {
        return {
          transcript: [],
          graph: {
            nodes: [],
            links: [],
            stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
          },
        };
      }
      if (command === "build_session_timeline_cmd")
        return { entries: [], total_count: 0 };
      return undefined;
    });

    await useAudioGraphStore.getState().loadSession("a-different-session");

    expect(useAudioGraphStore.getState().pendingFinalizingSession).toBeNull();
  });

  it("invalidates an in-flight historical load when capture starts", async () => {
    let resolveHistorical: (value: unknown) => void = () => {};
    const pendingHistorical = new Promise((resolve) => {
      resolveHistorical = resolve;
    });
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "load_session") return pendingHistorical;
      return undefined;
    });
    useAudioGraphStore.setState({ selectedSourceIds: ["system-default"] });

    const load = useAudioGraphStore
      .getState()
      .loadSession("historical-session");
    await useAudioGraphStore.getState().startCapture();
    resolveHistorical({
      transcript: [
        {
          id: "late-historical",
          source_id: "stored",
          speaker_id: null,
          speaker_label: null,
          text: "must be ignored",
          start_time: 0,
          end_time: 1,
          confidence: 1,
        },
      ],
      graph: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
    });
    expect(await load).toBeNull();

    const state = useAudioGraphStore.getState();
    expect(state.isCapturing).toBe(true);
    expect(state.loadedSessionId).toBeNull();
    expect(state.transcriptSegments).toEqual([]);
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

  it("loadSessionTimeline folds the backend command result into state", async () => {
    const timeline = [
      {
        span_id: "span-1",
        start_ms: 0,
        end_ms: 1000,
        received_at_ms: 1_700_000_000_000,
        turn_id: "t1",
        speaker_id: "spk-1",
        speaker_label: "Alice",
        text: "hello",
        related_edge_ids: ["e1"],
      },
    ];
    vi.mocked(invoke).mockResolvedValueOnce({
      entries: timeline,
      total_count: timeline.length,
    });

    // In production loadSession sets loadedSessionId before firing the fold;
    // the stale-async guard checks the response against it.
    useAudioGraphStore.setState({ loadedSessionId: "session-t" });
    const result = await useAudioGraphStore
      .getState()
      .loadSessionTimeline("session-t");

    expect(invoke).toHaveBeenCalledWith("build_session_timeline_cmd", {
      sessionId: "session-t",
      limit: 200,
    });
    expect(result).toEqual(timeline);
    const state = useAudioGraphStore.getState();
    expect(state.sessionTimeline).toEqual(timeline);
    expect(state.sessionTimelineTotalCount).toBe(timeline.length);
    expect(state.sessionTimelineLoading).toBe(false);
  });

  it("loadSessionTimeline keeps the pre-cap total_count even when entries were tail-capped", async () => {
    // Fix-round finding: the backend's `limit` equals the frontend's own
    // render window, so `entries.length` alone can never again exceed what
    // `SeekTimeline` shows — `total_count` is the only way its "showing the
    // last N of TOTAL" notice can still fire.
    const entries = [
      {
        span_id: "span-199",
        start_ms: 0,
        end_ms: 100,
        received_at_ms: 1,
        turn_id: null,
        speaker_id: null,
        speaker_label: null,
        text: "tail entry",
        related_edge_ids: [],
      },
    ];
    vi.mocked(invoke).mockResolvedValueOnce({
      entries,
      total_count: 5_000,
    });

    useAudioGraphStore.setState({ loadedSessionId: "session-truncated" });
    await useAudioGraphStore
      .getState()
      .loadSessionTimeline("session-truncated");

    const state = useAudioGraphStore.getState();
    expect(state.sessionTimeline).toEqual(entries);
    expect(state.sessionTimelineTotalCount).toBe(5_000);
  });

  it("loadSessionTimeline degrades to an empty timeline + error on failure", async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error("fold blew up"));

    useAudioGraphStore.setState({ loadedSessionId: "session-bad" });
    const result = await useAudioGraphStore
      .getState()
      .loadSessionTimeline("session-bad");

    // Never throws; falls back to an empty timeline so the strip renders its
    // graceful empty state rather than a perpetual spinner.
    expect(result).toEqual([]);
    const state = useAudioGraphStore.getState();
    expect(state.sessionTimeline).toEqual([]);
    expect(state.sessionTimelineTotalCount).toBe(0);
    expect(state.sessionTimelineLoading).toBe(false);
    expect(state.error).toMatch(/fold blew up/i);
  });

  it("a stale fold response never clobbers the newer session's timeline", async () => {
    // Session A's fold resolves LATE — after the user has already loaded
    // session B. A's response (and its loading-flag write) must be dropped.
    const timelineA = [
      {
        span_id: "a-1",
        start_ms: 0,
        end_ms: 100,
        received_at_ms: 1,
        turn_id: null,
        speaker_id: "spk-a",
        speaker_label: "Stale Speaker",
        text: "stale utterance",
        related_edge_ids: [],
      },
    ];
    const timelineB = [
      {
        span_id: "b-1",
        start_ms: 0,
        end_ms: 200,
        received_at_ms: 2,
        turn_id: null,
        speaker_id: "spk-b",
        speaker_label: "Current Speaker",
        text: "current utterance",
        related_edge_ids: [],
      },
    ];
    let resolveA: (value: unknown) => void = () => {};
    vi.mocked(invoke).mockImplementation(async (_cmd, args) => {
      const sessionId = (args as { sessionId: string }).sessionId;
      if (sessionId === "session-a") {
        return new Promise((resolve) => {
          resolveA = resolve;
        });
      }
      return { entries: timelineB, total_count: timelineB.length };
    });

    // Load A (fold hangs), then the user switches to B (fold resolves).
    useAudioGraphStore.setState({ loadedSessionId: "session-a" });
    const pendingA = useAudioGraphStore
      .getState()
      .loadSessionTimeline("session-a");
    useAudioGraphStore.setState({ loadedSessionId: "session-b" });
    await useAudioGraphStore.getState().loadSessionTimeline("session-b");
    expect(useAudioGraphStore.getState().sessionTimeline).toEqual(timelineB);

    // A's late response lands now — it must be ignored.
    resolveA({ entries: timelineA, total_count: timelineA.length });
    await pendingA;
    const state = useAudioGraphStore.getState();
    expect(state.sessionTimeline).toEqual(timelineB);
    expect(state.sessionTimelineLoading).toBe(false);
  });

  it("a stale fold FAILURE never blanks the newer session's timeline", async () => {
    const timelineB = [
      {
        span_id: "b-1",
        start_ms: 0,
        end_ms: 200,
        received_at_ms: 2,
        turn_id: null,
        speaker_id: "spk-b",
        speaker_label: "Current Speaker",
        text: "current utterance",
        related_edge_ids: [],
      },
    ];
    let rejectA: (reason?: unknown) => void = () => {};
    vi.mocked(invoke).mockImplementation(async (_cmd, args) => {
      const sessionId = (args as { sessionId: string }).sessionId;
      if (sessionId === "session-a") {
        return new Promise((_resolve, reject) => {
          rejectA = reject;
        });
      }
      return { entries: timelineB, total_count: timelineB.length };
    });

    useAudioGraphStore.setState({ loadedSessionId: "session-a" });
    const pendingA = useAudioGraphStore
      .getState()
      .loadSessionTimeline("session-a");
    useAudioGraphStore.setState({ loadedSessionId: "session-b" });
    await useAudioGraphStore.getState().loadSessionTimeline("session-b");

    // A's late FAILURE lands now — it must not blank B's timeline or set the
    // error banner for a session the user already left.
    rejectA(new Error("late stale failure"));
    await pendingA;
    const state = useAudioGraphStore.getState();
    expect(state.sessionTimeline).toEqual(timelineB);
    expect(state.sessionTimelineLoading).toBe(false);
    expect(state.error).toBeNull();
  });

  it("seekTranscriptToSegment sets a target with a monotonically bumped nonce", () => {
    const store = useAudioGraphStore.getState();
    store.seekTranscriptToSegment("seg-a");
    const first = useAudioGraphStore.getState().transcriptSeekTarget;
    expect(first).toEqual({ segmentId: "seg-a", nonce: 1 });

    // Re-selecting the SAME segment must still re-fire (bumped nonce), so the
    // transcript's scroll effect runs again.
    store.seekTranscriptToSegment("seg-a");
    const second = useAudioGraphStore.getState().transcriptSeekTarget;
    expect(second).toEqual({ segmentId: "seg-a", nonce: 2 });

    // Clearing resets the target.
    store.seekTranscriptToSegment(null);
    expect(useAudioGraphStore.getState().transcriptSeekTarget).toBeNull();
  });

  it("focusGraphEdges sets a focus with a monotonically bumped nonce", () => {
    const store = useAudioGraphStore.getState();
    store.focusGraphEdges(["edge-1", "edge-2"]);
    const first = useAudioGraphStore.getState().graphEdgeFocus;
    expect(first).toEqual({ edgeIds: ["edge-1", "edge-2"], nonce: 1 });

    // Re-activating the SAME badge must still re-fire (bumped nonce) so the
    // Analysis view-switch effect runs again.
    store.focusGraphEdges(["edge-1", "edge-2"]);
    const second = useAudioGraphStore.getState().graphEdgeFocus;
    expect(second).toEqual({ edgeIds: ["edge-1", "edge-2"], nonce: 2 });

    // An empty list is treated as a clear (no edges to focus).
    store.focusGraphEdges([]);
    expect(useAudioGraphStore.getState().graphEdgeFocus).toBeNull();

    // Explicit null clears too.
    store.focusGraphEdges(["edge-3"]);
    expect(useAudioGraphStore.getState().graphEdgeFocus).not.toBeNull();
    store.focusGraphEdges(null);
    expect(useAudioGraphStore.getState().graphEdgeFocus).toBeNull();
  });

  it("loadSession triggers the seek-timeline fold for the loaded session", async () => {
    const timeline = [
      {
        span_id: "span-1",
        start_ms: 0,
        end_ms: 500,
        received_at_ms: 1_700_000_000_000,
        turn_id: null,
        speaker_id: "spk-1",
        speaker_label: "Alice",
        text: "hi",
        related_edge_ids: [],
      },
    ];
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "load_session") {
        return {
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
        };
      }
      if (cmd === "build_session_timeline_cmd") {
        return { entries: timeline, total_count: timeline.length };
      }
      return undefined;
    });

    await useAudioGraphStore.getState().loadSession("session-fold");
    // The fold is fire-and-forget; flush the microtask queue so it settles.
    await Promise.resolve();
    await Promise.resolve();

    expect(invoke).toHaveBeenCalledWith("build_session_timeline_cmd", {
      sessionId: "session-fold",
      limit: 200,
    });
    expect(useAudioGraphStore.getState().sessionTimeline).toEqual(timeline);
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

  it("dedupes one live-transcript row when asr-span-revision (final) arrives before transcript-update for the same utterance (audio-graph-a35a)", () => {
    // This is the ACTUAL emission order on the Rust side: every ASR worker's
    // `emit_transcript_and_extract_with_meta` / local-diarization tail emits
    // `ASR_SPAN_REVISION` (final) and THEN `TRANSCRIPT_UPDATE` for the same
    // segment, synchronously, in the same handler.
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
        span_id: "deepgram:system-default:0-500",
        transcript_segment_id: "seg-a35a-1",
        text: "hello world",
        confidence: 0.93,
        is_final: true,
        stability: "final",
        speaker_id: "speaker-0",
        speaker_label: "Speaker 0",
        end_of_turn: true,
      }),
    );
    store.addTranscriptSegment({
      id: "seg-a35a-1",
      source_id: "system-default",
      speaker_id: "speaker-0",
      speaker_label: "Speaker 0",
      text: "hello world",
      start_time: 0,
      end_time: 0.5,
      confidence: 0.93,
    });

    const state = useAudioGraphStore.getState();
    expect(state.transcriptSegments).toHaveLength(1);
    expect(state.transcriptSegments).toEqual([
      expect.objectContaining({
        id: "deepgram:system-default:0-500",
        text: "hello world",
      }),
    ]);
  });

  it("dedupes one live-transcript row when transcript-update arrives before asr-span-revision (final) for the same utterance — the other event order (audio-graph-a35a)", () => {
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
    store.addTranscriptSegment({
      id: "seg-a35a-2",
      source_id: "system-default",
      speaker_id: "speaker-0",
      speaker_label: "Speaker 0",
      text: "hello world",
      start_time: 0,
      end_time: 0.5,
      confidence: 0.93,
    });
    store.addAsrSpanRevision(
      asrSpanRevision(1, {
        span_id: "deepgram:system-default:0-501",
        transcript_segment_id: "seg-a35a-2",
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
    expect(state.transcriptSegments).toHaveLength(1);
    expect(state.transcriptSegments).toEqual([
      expect.objectContaining({
        id: "deepgram:system-default:0-501",
        text: "hello world",
      }),
    ]);
  });

  it("still appends a live utterance whose transcript_segment_id collides with a stale/superseded ASR revision on the same span (audio-graph-a35a)", () => {
    const store = useAudioGraphStore.getState();
    // The winning (non-stale) final revision for this span materializes a
    // row and claims its own transcript_segment_id.
    store.addAsrSpanRevision(
      asrSpanRevision(2, {
        span_id: "deepgram:system-default:0-500",
        transcript_segment_id: "seg-a35a-3-current",
        text: "current final",
        is_final: true,
        stability: "final",
      }),
    );
    // A later revision for the SAME span with a lower revision_number (e.g.
    // a reconnect-induced span-id collision) is stale: it is still buffered
    // into asrSpanRevisions for history, but
    // applyAsrRevisionToTranscriptSegments leaves transcriptSegments
    // untouched because isStaleAsrRevision rejects it — no row is ever
    // materialized under its transcript_segment_id.
    store.addAsrSpanRevision(
      asrSpanRevision(1, {
        span_id: "deepgram:system-default:0-500",
        transcript_segment_id: "seg-a35a-3-stale",
        text: "stale revision text",
        is_final: true,
        stability: "final",
      }),
    );
    expect(useAudioGraphStore.getState().transcriptSegments).toHaveLength(1);

    // A live transcript-update whose id happens to match the STALE
    // revision's transcript_segment_id is a genuinely new utterance —
    // nothing materialized that id — and must still be appended.
    store.addTranscriptSegment({
      id: "seg-a35a-3-stale",
      source_id: "system-default",
      speaker_id: null,
      speaker_label: null,
      text: "genuinely new utterance",
      start_time: 1,
      end_time: 1.5,
      confidence: 0.9,
    });

    const state = useAudioGraphStore.getState();
    expect(state.transcriptSegments).toHaveLength(2);
    expect(state.transcriptSegments).toEqual([
      expect.objectContaining({
        id: "deepgram:system-default:0-500",
        text: "current final",
      }),
      expect.objectContaining({
        id: "seg-a35a-3-stale",
        text: "genuinely new utterance",
      }),
    ]);
  });

  it("does not let a stale sample-preview ASR revision suppress a live segment when the preview exits (audio-graph-a35a)", () => {
    useAudioGraphStore.setState({
      samplePreviewActive: true,
      transcriptSegments: [
        {
          id: "sample-segment-3",
          source_id: "system-default",
          speaker_id: null,
          speaker_label: null,
          text: "sample preview text",
          start_time: 0,
          end_time: 1,
          confidence: 1,
        },
      ],
      asrSpanRevisions: [
        asrSpanRevision(1, {
          span_id: "sample-span-3",
          // Collides with the live segment id added below — proves the
          // `state.samplePreviewActive ? [] : state.asrSpanRevisions` guard
          // (not just the transcriptSegments guard) is load-bearing.
          transcript_segment_id: "seg-live-collide",
          text: "sample preview text",
          is_final: true,
          stability: "final",
        }),
      ],
    });

    useAudioGraphStore.getState().addTranscriptSegment({
      id: "seg-live-collide",
      source_id: "system-default",
      speaker_id: "speaker-0",
      speaker_label: "Speaker 0",
      text: "live utterance",
      start_time: 5,
      end_time: 5.5,
      confidence: 0.9,
    });

    const state = useAudioGraphStore.getState();
    expect(state.samplePreviewActive).toBe(false);
    expect(state.transcriptSegments).toEqual([
      expect.objectContaining({
        id: "seg-live-collide",
        text: "live utterance",
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

  // seed audio-graph-e700 sub-fix 2 (UPSERT KEYING): mirrors the Rust
  // `upsert_collision_same_model_id_different_names_does_not_merge` test —
  // the live incremental view must resolve the SAME collision the same way
  // a replayed session's `MaterializedGraph::apply_patch` does.
  it("does not merge two different names that collide on the same raw model id across ticks", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "node1",
          name: "Alice",
          entity_type: "Person",
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(2, [
        {
          type: "upsert_graph_node",
          id: "node1",
          name: "Bob",
          entity_type: "Person",
        },
      ]),
    );

    const graph = useAudioGraphStore.getState().materializedProjectionGraph;
    const activeNodes = graph?.nodes.filter(
      (node) => node.valid_until_ms == null,
    );
    expect(activeNodes?.map((node) => node.name).sort()).toEqual([
      "Alice",
      "Bob",
    ]);
    const alice = activeNodes?.find((node) => node.name === "Alice");
    const bob = activeNodes?.find((node) => node.name === "Bob");
    expect(alice?.id).toBe("node1");
    expect(bob?.id).toBe("node1~2");
  });

  // seed audio-graph-e700 sub-fix 3 (FUZZY RESOLUTION): mirrors the Rust
  // `fuzzy_resolution_merges_near_duplicate_entity_names_across_ids` test.
  it("merges a near-duplicate entity name minted under a different id across ticks", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "product-1",
          name: "Postgres",
          entity_type: "Product",
          description: "A relational database.",
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(2, [
        {
          type: "upsert_graph_node",
          id: "product-7",
          name: "PostgreSQL",
          entity_type: "Product",
          description: "Open-source object-relational database.",
        },
      ]),
    );

    const graph = useAudioGraphStore.getState().materializedProjectionGraph;
    const activeNodes = graph?.nodes.filter(
      (node) => node.valid_until_ms == null,
    );
    expect(activeNodes).toHaveLength(1);
    expect(activeNodes?.[0]).toMatchObject({
      id: "product-1",
      name: "PostgreSQL",
      description: "Open-source object-relational database.",
    });
  });

  // seed audio-graph-e700 REPLAY COMPATIBILITY: mirrors the Rust
  // `resurrection_after_invalidate_keeps_the_same_id_for_later_cross_patch_reference`
  // test — a raw id that was invalidated and then re-upserted under the SAME
  // id must resurrect in place, not fork, so a LATER, separate patch's raw-id
  // reference (an edge here) still resolves.
  it("resurrects an invalidated node under its original id for a later cross-patch edge reference", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "node1",
          name: "Alice",
          entity_type: "Person",
        },
        {
          type: "upsert_graph_node",
          id: "node2",
          name: "Roadmap",
          entity_type: "Topic",
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(2, [{ type: "invalidate_graph_node", id: "node1" }]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(3, [
        {
          type: "upsert_graph_node",
          id: "node1",
          name: "Alice",
          entity_type: "Person",
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(4, [
        {
          type: "upsert_graph_edge",
          id: "edge1",
          source: "node1",
          target: "node2",
          relation_type: "discussed",
          label: null,
          weight: 0.5,
        },
      ]),
    );

    const graph = useAudioGraphStore.getState().materializedProjectionGraph;
    expect(
      graph?.nodes.filter((n) => n.valid_until_ms == null).map((n) => n.id),
    ).toEqual(expect.arrayContaining(["node1", "node2"]));
    expect(graph?.nodes).toHaveLength(2);
    expect(graph?.edges.find((e) => e.id === "edge1")?.source).toBe("node1");
  });

  // seed audio-graph-e700 REPLAY COMPATIBILITY: mirrors the Rust
  // `cross_patch_reference_to_a_fuzzy_absorbed_raw_id_resolves_via_persistent_alias`
  // test — a raw id fuzzy-absorbed into a DIFFERENT node's id never gets a
  // row of its own, so a LATER, separate patch referencing that raw id must
  // resolve through the persisted `id_aliases`, not fail.
  it("resolves a later cross-patch reference to a fuzzy-absorbed raw id via the persisted alias", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "n1",
          name: "Postgres",
          entity_type: "Product",
        },
        {
          type: "upsert_graph_node",
          id: "n20",
          name: "Deployment",
          entity_type: "Topic",
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(2, [
        {
          type: "upsert_graph_node",
          id: "n7",
          name: "PostgreSQL",
          entity_type: "Product",
        },
      ]),
    );

    let graph = useAudioGraphStore.getState().materializedProjectionGraph;
    expect(graph?.nodes.some((n) => n.id === "n7")).toBe(false);
    expect(graph?.id_aliases?.n7).toBe("n1");

    store.addProjectionPatch(
      graphProjectionPatch(3, [
        {
          type: "upsert_graph_edge",
          id: "edge1",
          source: "n7",
          target: "n20",
          relation_type: "used_for",
          label: null,
          weight: 0.5,
        },
      ]),
    );

    graph = useAudioGraphStore.getState().materializedProjectionGraph;
    expect(graph?.edges.find((e) => e.id === "edge1")?.source).toBe("n1");
  });

  // seed audio-graph-e700 REPLAY COMPATIBILITY: mirrors the Rust
  // `same_patch_merge_into_its_own_fuzzy_absorption_target_is_a_no_op` test
  // (and pins the TS `sourceId === targetId` guard the Rust side lacked).
  it("treats a same-patch merge into its own fuzzy absorption target as a no-op", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "n1",
          name: "Postgres",
          entity_type: "Product",
        },
        {
          type: "upsert_graph_node",
          id: "n7",
          name: "PostgreSQL",
          entity_type: "Product",
        },
        {
          type: "merge_graph_nodes",
          source_id: "n7",
          target_id: "n1",
        },
      ]),
    );

    const graph = useAudioGraphStore.getState().materializedProjectionGraph;
    const active = graph?.nodes.filter((n) => n.valid_until_ms == null);
    expect(active).toHaveLength(1);
    expect(active?.[0].id).toBe("n1");
  });

  // seed audio-graph-e700 (reviewer finding 3): mirrors the Rust
  // `same_patch_displaced_upsert_cross_reference_lands_on_the_disambiguated_id`
  // test — a same-patch reference to a raw id displaced by a tier-3
  // disambiguation (collision with an unrelated PRE-EXISTING node) must
  // follow THIS patch's own displacement, not the pre-existing node that
  // still literally owns the id.
  it("resolves a same-patch reference to a displaced upsert onto the disambiguated id", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "node1",
          name: "Alice",
          entity_type: "Person",
        },
        {
          type: "upsert_graph_node",
          id: "topic:standup",
          name: "Standup",
          entity_type: "Topic",
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(2, [
        {
          type: "upsert_graph_node",
          id: "node1",
          name: "Bob",
          entity_type: "Person",
        },
        {
          type: "upsert_graph_edge",
          id: "edge1",
          source: "node1",
          target: "topic:standup",
          relation_type: "discussed",
          label: null,
          weight: 0.5,
        },
      ]),
    );

    const graph = useAudioGraphStore.getState().materializedProjectionGraph;
    const bob = graph?.nodes.find((n) => n.name === "Bob");
    expect(bob?.id).toBe("node1~2");
    expect(graph?.edges.find((e) => e.id === "edge1")?.source).toBe("node1~2");
  });

  // Reviewer finding (major, disclosed trade-off): mirrors the Rust
  // `cross_patch_reference_after_a_collision_binds_to_the_first_occupant_not_the_latest`
  // test — a LATER, separate patch's raw-id reference after a collision
  // binds to the FIRST occupant of that literal id, not the
  // most-recently-written entity. Pinned so a future change to this
  // semantic is deliberate.
  it("binds a later cross-patch reference after a collision to the first occupant, not the latest", () => {
    const store = useAudioGraphStore.getState();
    store.addProjectionPatch(
      graphProjectionPatch(1, [
        {
          type: "upsert_graph_node",
          id: "node1",
          name: "Alice",
          entity_type: "Person",
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(2, [
        {
          type: "upsert_graph_node",
          id: "node1",
          name: "Bob",
          entity_type: "Person",
        },
      ]),
    );
    store.addProjectionPatch(
      graphProjectionPatch(3, [{ type: "invalidate_graph_node", id: "node1" }]),
    );

    const graph = useAudioGraphStore.getState().materializedProjectionGraph;
    const alice = graph?.nodes.find((n) => n.name === "Alice");
    const bob = graph?.nodes.find((n) => n.name === "Bob");
    expect(alice?.valid_until_ms).not.toBeNull();
    expect(bob?.valid_until_ms).toBeNull();
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

  it("clears historical Review projections before accepting new live events", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      selectedSourceIds: ["system-default"],
      loadedSessionId: "historical-session",
      transcriptSegments: [
        {
          id: "historical-segment",
          source_id: "stored",
          speaker_id: null,
          speaker_label: null,
          text: "historical text",
          start_time: 0,
          end_time: 1,
          confidence: 0.9,
        },
      ],
      sessionProjectionEvents: [noteProjectionPatch(1, [])],
      materializedNotes: {
        schema_version: 1,
        session_id: "historical-session",
        last_sequence: 1,
        notes: [],
      },
      liveAssistCards: [liveAssistCard("historical-card")],
      graphSnapshot: {
        nodes: [],
        links: [],
        stats: { total_nodes: 3, total_edges: 2, total_episodes: 1 },
      },
    });

    await useAudioGraphStore.getState().startCapture();

    const afterStart = useAudioGraphStore.getState();
    expect(afterStart.loadedSessionId).toBeNull();
    expect(afterStart.transcriptSegments).toEqual([]);
    expect(afterStart.sessionProjectionEvents).toEqual([]);
    expect(afterStart.materializedNotes).toBeNull();
    expect(afterStart.liveAssistCards).toEqual([]);
    expect(afterStart.graphSnapshot.stats.total_nodes).toBe(0);

    afterStart.addTranscriptSegment({
      id: "live-segment",
      source_id: "system-default",
      speaker_id: null,
      speaker_label: null,
      text: "new live text",
      start_time: 0,
      end_time: 1,
      confidence: 0.95,
    });
    expect(useAudioGraphStore.getState().transcriptSegments).toEqual([
      expect.objectContaining({ id: "live-segment", text: "new live text" }),
    ]);
  });

  it("clears a stale pendingFinalizingSession when a fresh capture starts (R2 adversary finding #1: a prior stop's resident-data premise is void once a new capture overwrites the store)", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      selectedSourceIds: ["system-default"],
      pendingFinalizingSession: {
        id: "prior-stop-session",
        title: null,
        created_at: 1_700_000_000_000,
        ended_at: 1_700_000_060_000,
        duration_seconds: 60,
        status: "active",
        segment_count: 2,
        speaker_count: 1,
        entity_count: 0,
        transcript_path: "",
        graph_path: "",
        deleted: false,
        deleted_at: null,
        optimistic: true,
      },
    });

    await useAudioGraphStore.getState().startCapture();

    expect(useAudioGraphStore.getState().pendingFinalizingSession).toBeNull();
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
  // Answer-card actions (audio-graph-83cc T4) — `askQuestion` (composer,
  // mints a card via `ask_question_card`) and `answerQuestionCard` (manual
  // "Ask AI"/Retry, `answer_question_card`, `auto: false`). Both commands are
  // named in the design panel synthesis but NOT YET backed by a real Rust
  // command in this tree (T3 unlanded) — the "rejecting invoke" tests below
  // are exactly that real-world case today, not a hypothetical.
  // -----------------------------------------------------------------------

  type AnswerChannelLike = { onmessage: ((m: unknown) => void) | null };
  /** Generalizes `captureStreamChannel` above to any command name and any
   * resolved value shape — `answer_question_card`/`ask_question_card` return
   * different payloads (`{request_id}` vs `{record, request_id}`) but both
   * pass a `channel` arg the mock needs to capture identically. */
  function captureAnswerChannel<T>(commandName: string) {
    let channel: AnswerChannelLike | null = null;
    let resolve: (value: T) => void = () => {};
    let reject: (err: unknown) => void = () => {};
    vi.mocked(invoke).mockImplementation(async (cmd, args) => {
      if (cmd === commandName) {
        const argsRecord = args as { channel?: AnswerChannelLike } | undefined;
        channel = argsRecord?.channel ?? null;
        return new Promise<T>((res, rej) => {
          resolve = res;
          reject = rej;
        });
      }
      return undefined;
    });
    return {
      getChannel: (): AnswerChannelLike => {
        if (channel === null) throw new Error("channel not captured yet");
        return channel;
      },
      resolveDispatch: (value: T) => resolve(value),
      rejectDispatch: (err: unknown) => reject(err),
    };
  }

  describe("answerQuestionCard (manual Ask AI / Retry)", () => {
    beforeEach(() => {
      useAudioGraphStore.setState({ settings: selectableSettings() });
    });

    it("sets a streaming draft immediately, then coalesces deltas and clears the draft on a non-error terminal frame", async () => {
      const { getChannel, resolveDispatch } = captureAnswerChannel<{
        request_id: string;
      }>("answer_question_card");
      resetStoreForTest({
        agentProposals: [
          liveAssistCard("card-1", {
            proposal: { kind: "question", body: "q body" },
          }).proposal,
        ],
      });

      const promise = useAudioGraphStore
        .getState()
        .answerQuestionCard("card-1");
      expect(useAudioGraphStore.getState().answerDrafts["card-1"]).toEqual({
        status: "streaming",
        text: "",
        requestId: null,
      });

      const channel = getChannel();
      resolveDispatch({ request_id: "req-1" });
      await promise;
      expect(
        useAudioGraphStore.getState().answerDrafts["card-1"]?.requestId,
      ).toBe("req-1");

      channel.onmessage?.({
        event: "delta",
        data: { request_id: "req-1", delta: "Partial " },
      });
      channel.onmessage?.({
        event: "delta",
        data: { request_id: "req-1", delta: "answer." },
      });
      await vi.waitFor(() => {
        expect(useAudioGraphStore.getState().answerDrafts["card-1"]?.text).toBe(
          "Partial answer.",
        );
      });

      channel.onmessage?.({
        event: "done",
        data: {
          request_id: "req-1",
          full_text: "Partial answer.",
          finish_reason: "stop",
        },
      });
      expect(
        useAudioGraphStore.getState().answerDrafts["card-1"],
      ).toBeUndefined();
    });

    it("sets a failed draft (never throwing) on a done frame carrying an error finish_reason", async () => {
      const { getChannel, resolveDispatch } = captureAnswerChannel<{
        request_id: string;
      }>("answer_question_card");
      resetStoreForTest({
        agentProposals: [
          liveAssistCard("card-err", {
            proposal: { kind: "question", body: "q body" },
          }).proposal,
        ],
      });

      const promise = useAudioGraphStore
        .getState()
        .answerQuestionCard("card-err");
      const channel = getChannel();
      resolveDispatch({ request_id: "req-err" });
      await promise;

      channel.onmessage?.({
        event: "done",
        data: {
          request_id: "req-err",
          full_text: "",
          finish_reason: "error: rate limited",
        },
      });

      expect(useAudioGraphStore.getState().answerDrafts["card-err"]).toEqual({
        status: "failed",
        text: "rate limited",
        requestId: "req-err",
      });
    });

    it("audio-graph-83cc T4 fix-round (minor): a STALE terminal frame from a superseded dispatch does not clobber a newer dispatch's draft for the same card (double-click on Ask AI before React re-renders)", async () => {
      type PendingCall = {
        channel: AnswerChannelLike;
        resolve: (value: { request_id: string }) => void;
      };
      const calls: PendingCall[] = [];
      vi.mocked(invoke).mockImplementation(async (cmd, args) => {
        if (cmd !== "answer_question_card") return undefined;
        const argsRecord = args as { channel?: AnswerChannelLike } | undefined;
        return new Promise<{ request_id: string }>((resolve) => {
          calls.push({
            channel: argsRecord?.channel ?? { onmessage: null },
            resolve,
          });
        });
      });
      resetStoreForTest({
        agentProposals: [
          liveAssistCard("card-race", {
            proposal: { kind: "question", body: "q body" },
          }).proposal,
        ],
      });

      const promiseA = useAudioGraphStore
        .getState()
        .answerQuestionCard("card-race");
      const promiseB = useAudioGraphStore
        .getState()
        .answerQuestionCard("card-race");
      expect(calls).toHaveLength(2);

      calls[0].resolve({ request_id: "req-A" });
      calls[1].resolve({ request_id: "req-B" });
      await Promise.all([promiseA, promiseB]);
      // The later dispatch's own arm won the race for the current draft.
      expect(
        useAudioGraphStore.getState().answerDrafts["card-race"]?.requestId,
      ).toBe("req-B");

      // req-A's STALE terminal frame must be a no-op — it must not clear or
      // overwrite req-B's active streaming draft.
      calls[0].channel.onmessage?.({
        event: "done",
        data: {
          request_id: "req-A",
          full_text: "stale",
          finish_reason: "stop",
        },
      });
      expect(useAudioGraphStore.getState().answerDrafts["card-race"]).toEqual({
        status: "streaming",
        text: "",
        requestId: "req-B",
      });

      // req-B's own terminal frame still applies normally.
      calls[1].channel.onmessage?.({
        event: "done",
        data: {
          request_id: "req-B",
          full_text: "final",
          finish_reason: "stop",
        },
      });
      expect(
        useAudioGraphStore.getState().answerDrafts["card-race"],
      ).toBeUndefined();
    });

    it("degrades gracefully on a REJECTING invoke: sets a failed draft, never throws, never silently drops the attempt", async () => {
      vi.mocked(invoke).mockRejectedValueOnce(
        new Error("Backend command not found: answer_question_card"),
      );
      resetStoreForTest({
        agentProposals: [
          liveAssistCard("card-rej", {
            proposal: { kind: "question", body: "q body" },
          }).proposal,
        ],
      });

      await expect(
        useAudioGraphStore.getState().answerQuestionCard("card-rej"),
      ).resolves.toBeUndefined();

      expect(useAudioGraphStore.getState().answerDrafts["card-rej"]).toEqual({
        status: "failed",
        text: "Backend command not found: answer_question_card",
        requestId: null,
      });
    });

    it("does nothing (no invoke call) when settings have not loaded yet", async () => {
      useAudioGraphStore.setState({ settings: null });
      resetStoreForTest({
        agentProposals: [
          liveAssistCard("card-nosettings", {
            proposal: { kind: "question", body: "q body" },
          }).proposal,
        ],
      });
      await useAudioGraphStore.getState().answerQuestionCard("card-nosettings");
      expect(invoke).not.toHaveBeenCalledWith(
        "answer_question_card",
        expect.anything(),
      );
      expect(useAudioGraphStore.getState().error).not.toBeNull();
    });
  });

  describe("askQuestion (composer)", () => {
    beforeEach(() => {
      useAudioGraphStore.setState({ settings: selectableSettings() });
    });

    it("mints a card via ask_question_card, upserts it, and arms the draft with the returned request id", async () => {
      const { getChannel, resolveDispatch } = captureAnswerChannel<{
        record: LiveAssistCardRecord;
        request_id: string;
      }>("ask_question_card");
      resetStoreForTest();

      const minted = liveAssistCard("minted-1", {
        origin: "user",
        proposal: { kind: "question" },
      });
      const promise = useAudioGraphStore
        .getState()
        .askQuestion("  what now?  ");
      const channel = getChannel();
      resolveDispatch({ record: minted, request_id: "req-mint-1" });
      await promise;

      expect(
        useAudioGraphStore
          .getState()
          .liveAssistCards.some((c) => c.proposal.id === "minted-1"),
      ).toBe(true);
      expect(useAudioGraphStore.getState().answerDrafts["minted-1"]).toEqual({
        status: "streaming",
        text: "",
        requestId: "req-mint-1",
      });
      expect(useAudioGraphStore.getState().composerError).toBeNull();

      channel.onmessage?.({
        event: "done",
        data: {
          request_id: "req-mint-1",
          full_text: "answer",
          finish_reason: "stop",
        },
      });
      expect(
        useAudioGraphStore.getState().answerDrafts["minted-1"],
      ).toBeUndefined();
    });

    it("audio-graph-83cc T4 fix-round (minor): a STALE terminal frame does not clobber a draft a LATER write armed for the same card", async () => {
      const { getChannel, resolveDispatch } = captureAnswerChannel<{
        record: LiveAssistCardRecord;
        request_id: string;
      }>("ask_question_card");
      resetStoreForTest();

      const minted = liveAssistCard("minted-race", {
        origin: "user",
        proposal: { kind: "question" },
      });
      const promise = useAudioGraphStore.getState().askQuestion("what now?");
      const channel = getChannel();
      resolveDispatch({ record: minted, request_id: "req-orig" });
      await promise;
      expect(
        useAudioGraphStore.getState().answerDrafts["minted-race"]?.requestId,
      ).toBe("req-orig");

      // Simulate a later write superseding this draft for the same card id
      // (e.g. a Retry re-arming it with a new request id).
      useAudioGraphStore.getState().setAnswerDraft("minted-race", {
        status: "streaming",
        text: "",
        requestId: "req-newer",
      });

      // req-orig's now-STALE terminal frame must be a no-op.
      channel.onmessage?.({
        event: "done",
        data: {
          request_id: "req-orig",
          full_text: "stale",
          finish_reason: "stop",
        },
      });
      expect(useAudioGraphStore.getState().answerDrafts["minted-race"]).toEqual(
        {
          status: "streaming",
          text: "",
          requestId: "req-newer",
        },
      );
    });

    it("degrades gracefully on a REJECTING invoke: sets composerError, never throws, liveAssistCards unchanged", async () => {
      vi.mocked(invoke).mockRejectedValueOnce(
        new Error("Backend command not found: ask_question_card"),
      );
      resetStoreForTest();

      await expect(
        useAudioGraphStore.getState().askQuestion("what now?"),
      ).resolves.toBeUndefined();

      expect(useAudioGraphStore.getState().composerError).toBe(
        "Backend command not found: ask_question_card",
      );
      expect(useAudioGraphStore.getState().liveAssistCards).toEqual([]);
    });

    it("never dispatches for an empty/whitespace-only question", async () => {
      resetStoreForTest();
      await useAudioGraphStore.getState().askQuestion("   ");
      expect(invoke).not.toHaveBeenCalled();
    });

    it("clears any previous composerError at the start of a new attempt", async () => {
      resetStoreForTest();
      useAudioGraphStore.setState({ composerError: "stale error" });
      vi.mocked(invoke).mockImplementation(
        () => new Promise(() => {}), // never resolves — only the pre-dispatch clear matters here
      );
      void useAudioGraphStore.getState().askQuestion("hello");
      expect(useAudioGraphStore.getState().composerError).toBeNull();
    });
  });

  describe("the auto-answer trigger, hooked into addAgentProposal (audio-graph-83cc T5)", () => {
    beforeEach(() => {
      useAudioGraphStore.setState({
        settings: selectableSettings({
          agent_auto_answer: {
            enabled: true,
            max_per_session: 12,
            min_interval_secs: 45,
          },
        }),
      });
    });

    function wellFormedQuestionProposal(
      overrides: Partial<AgentProposalEvent> = {},
    ): AgentProposalEvent {
      return {
        id: "trigger-q",
        source_segment_id: "segment-trigger-q",
        source_id: "system",
        speaker_label: "Speaker 1",
        kind: "question",
        title: "Question from Speaker 1",
        body: "Consider answering or linking this question: What did the team decide about the launch date?",
        confidence: 0.9,
        created_at_ms: 100,
        ...overrides,
      };
    }

    it("dispatches answer_question_card with auto:true for a well-formed, Signal-admitted question proposal", () => {
      captureAnswerChannel<{ request_id: string }>("answer_question_card");
      resetStoreForTest({ agentProposals: [], liveAssistCards: [] });

      useAudioGraphStore
        .getState()
        .addAgentProposal(wellFormedQuestionProposal());

      expect(invoke).toHaveBeenCalledWith("answer_question_card", {
        proposalId: "trigger-q",
        question: "What did the team decide about the launch date?",
        auto: true,
        channel: expect.any(Channel),
      });
    });

    it("does NOT dispatch when agent_auto_answer.enabled is false — the belt check (deliverable f)", () => {
      useAudioGraphStore.setState({
        settings: selectableSettings({
          agent_auto_answer: {
            enabled: false,
            max_per_session: 12,
            min_interval_secs: 45,
          },
        }),
      });
      resetStoreForTest({ agentProposals: [], liveAssistCards: [] });

      useAudioGraphStore
        .getState()
        .addAgentProposal(wellFormedQuestionProposal());

      expect(invoke).not.toHaveBeenCalledWith(
        "answer_question_card",
        expect.anything(),
      );
    });

    it("does NOT dispatch when settings/agent_auto_answer are absent (never enabled by omission)", () => {
      useAudioGraphStore.setState({ settings: selectableSettings() }); // no agent_auto_answer field at all
      resetStoreForTest({ agentProposals: [], liveAssistCards: [] });

      useAudioGraphStore
        .getState()
        .addAgentProposal(wellFormedQuestionProposal());

      expect(invoke).not.toHaveBeenCalledWith(
        "answer_question_card",
        expect.anything(),
      );
    });

    it("does NOT dispatch a fragment-suspect question (below the Signal bar) — the FE half of the ratified 'both gates' rule", () => {
      resetStoreForTest({ agentProposals: [], liveAssistCards: [] });

      useAudioGraphStore.getState().addAgentProposal(
        wellFormedQuestionProposal({
          id: "trigger-fragment",
          body: "Consider answering or linking this question: what about",
        }),
      );

      expect(invoke).not.toHaveBeenCalledWith(
        "answer_question_card",
        expect.anything(),
      );
    });

    it("does NOT dispatch while conversationMode is 'converse' with the DEFAULT (pipelined) engine — Rust's own Converse refusal only covers native S2S, so this exclusion must be FE-side (fix round, scope-honesty blocker)", () => {
      useAudioGraphStore.setState({
        conversationMode: "converse",
        converseEngine: "pipelined",
      });
      resetStoreForTest({ agentProposals: [], liveAssistCards: [] });

      useAudioGraphStore
        .getState()
        .addAgentProposal(wellFormedQuestionProposal());

      expect(invoke).not.toHaveBeenCalledWith(
        "answer_question_card",
        expect.anything(),
      );
    });

    it("does NOT dispatch while conversationMode is 'converse' with the native engine either", () => {
      useAudioGraphStore.setState({
        conversationMode: "converse",
        converseEngine: "native",
      });
      resetStoreForTest({ agentProposals: [], liveAssistCards: [] });

      useAudioGraphStore
        .getState()
        .addAgentProposal(wellFormedQuestionProposal());

      expect(invoke).not.toHaveBeenCalledWith(
        "answer_question_card",
        expect.anything(),
      );
    });

    it("dispatches normally once conversationMode returns to 'notes'", () => {
      captureAnswerChannel<{ request_id: string }>("answer_question_card");
      useAudioGraphStore.setState({ conversationMode: "notes" });
      resetStoreForTest({ agentProposals: [], liveAssistCards: [] });

      useAudioGraphStore
        .getState()
        .addAgentProposal(wellFormedQuestionProposal());

      expect(invoke).toHaveBeenCalledWith(
        "answer_question_card",
        expect.objectContaining({ auto: true }),
      );
    });

    it("dispatches EXACTLY ONCE for a duplicate addAgentProposal call with the same id (upsert semantics)", () => {
      captureAnswerChannel<{ request_id: string }>("answer_question_card");
      resetStoreForTest({ agentProposals: [], liveAssistCards: [] });

      const p = wellFormedQuestionProposal();
      useAudioGraphStore.getState().addAgentProposal(p);
      useAudioGraphStore.getState().addAgentProposal(p);

      const dispatchCalls = vi
        .mocked(invoke)
        .mock.calls.filter(([cmd]) => cmd === "answer_question_card");
      expect(dispatchCalls).toHaveLength(1);
    });

    it("drop-not-queue (deliverable c): a REFUSED auto dispatch sets no error banner and no failed draft, and repeated refusals across DIFFERENT cards never accumulate any of that state", async () => {
      vi.mocked(invoke).mockImplementation(async (cmd) => {
        if (cmd === "answer_question_card") {
          throw new Error("Rejected: 12/session cap reached");
        }
        return undefined;
      });
      resetStoreForTest({ agentProposals: [], liveAssistCards: [] });

      useAudioGraphStore
        .getState()
        .addAgentProposal(wellFormedQuestionProposal({ id: "refused-1" }));
      useAudioGraphStore
        .getState()
        .addAgentProposal(
          wellFormedQuestionProposal({ id: "refused-2", created_at_ms: 200 }),
        );
      // Let the fire-and-forget `answerQuestionCard` promises settle.
      //
      // Waiting only for the invoke call COUNT (a prior version of this
      // test did) is vacuous: `mock.calls` is populated synchronously the
      // instant `invoke` is called, which happens before the rejection has
      // unwound through `safeInvoke`'s own `try/await/catch` wrapper AND
      // `answerQuestionCard`'s `catch` — two more awaited hops. A `vi.waitFor`
      // whose predicate is already true resolves without ever yielding to a
      // macrotask, so it would pass unconditionally even with the entire
      // drop-not-queue branch deleted. A real macrotask boundary
      // (`setTimeout`) is timing-independent: by spec every pending
      // microtask, however many hops deep, drains before the next macrotask
      // runs, so this guarantees both refusals have fully settled before the
      // assertions below run.
      await new Promise((resolve) => setTimeout(resolve, 0));
      const dispatchCalls = vi
        .mocked(invoke)
        .mock.calls.filter(([cmd]) => cmd === "answer_question_card");
      expect(dispatchCalls).toHaveLength(2);

      expect(useAudioGraphStore.getState().error).toBeNull();
      expect(useAudioGraphStore.getState().answerDrafts).toEqual({});
      expect(useAudioGraphStore.getState().autoAnswerDispatchCount).toBe(0);
    });

    it("increments autoAnswerDispatchCount only once the dispatch is ACCEPTED, never on a refusal", async () => {
      const { getChannel, resolveDispatch } = captureAnswerChannel<{
        request_id: string;
      }>("answer_question_card");
      resetStoreForTest({ agentProposals: [], liveAssistCards: [] });

      useAudioGraphStore
        .getState()
        .addAgentProposal(wellFormedQuestionProposal());
      expect(useAudioGraphStore.getState().autoAnswerDispatchCount).toBe(0);

      getChannel();
      resolveDispatch({ request_id: "auto-req-1" });
      await vi.waitFor(() => {
        expect(useAudioGraphStore.getState().autoAnswerDispatchCount).toBe(1);
      });
      // No error, no draft ghost, from an ACCEPTED auto dispatch either —
      // the streaming draft carries the real request id, not a failure.
      expect(useAudioGraphStore.getState().error).toBeNull();
    });
  });

  describe("answer drafts are cleaned up on dismiss/clear (audio-graph-83cc T4)", () => {
    it("dismissAgentProposal clears only that card's draft", async () => {
      resetStoreForTest({
        agentProposals: [
          liveAssistCard("keep-me").proposal,
          liveAssistCard("drop-me").proposal,
        ],
      });
      useAudioGraphStore.setState({
        answerDrafts: {
          "keep-me": { status: "streaming", text: "", requestId: "r1" },
          "drop-me": { status: "failed", text: "oops", requestId: null },
        },
      });
      vi.mocked(invoke).mockResolvedValue(null);

      await useAudioGraphStore.getState().dismissAgentProposal("drop-me");

      const drafts = useAudioGraphStore.getState().answerDrafts;
      expect(drafts["drop-me"]).toBeUndefined();
      expect(drafts["keep-me"]).toEqual({
        status: "streaming",
        text: "",
        requestId: "r1",
      });
    });

    it("clearAgentProposals clears drafts ONLY for the cards it actually dismissed — not every draft in the store", async () => {
      // audio-graph-83cc T4 fix-round finding: a draft can outlive
      // `"pending"` (`approveAgentProposal` never clears one), so a
      // wholesale `{}` reset here would silently delete an unrelated
      // approved card's failure text/Retry affordance. `card "b"` below
      // stands in for exactly that — it is NOT among the cards this call
      // dismisses (the mock only resolves with "a"'s record), so its draft
      // must survive.
      resetStoreForTest({
        agentProposals: [liveAssistCard("a").proposal],
      });
      useAudioGraphStore.setState({
        answerDrafts: {
          a: { status: "streaming", text: "", requestId: "r1" },
          b: { status: "failed", text: "oops", requestId: null },
        },
      });
      vi.mocked(invoke).mockResolvedValue([
        liveAssistCard("a", { status: "dismissed" }),
      ]);

      await useAudioGraphStore.getState().clearAgentProposals();

      const drafts = useAudioGraphStore.getState().answerDrafts;
      expect(drafts.a).toBeUndefined();
      expect(drafts.b).toEqual({
        status: "failed",
        text: "oops",
        requestId: null,
      });
    });
  });

  describe("resetSessionView clears the T4 answer-draft/composer-error state (audio-graph-83cc T4 fix-round finding, major)", () => {
    it("clears answerDrafts and composerError alongside every other session-scoped projection", () => {
      useAudioGraphStore.setState({
        answerDrafts: {
          "stale-card": {
            status: "streaming",
            text: "partial",
            requestId: "r1",
          },
        },
        composerError: "Backend command not found: ask_question_card",
      });

      useAudioGraphStore.getState().resetSessionView();

      const s = useAudioGraphStore.getState();
      expect(s.answerDrafts).toEqual({});
      expect(s.composerError).toBeNull();
    });
  });

  // -----------------------------------------------------------------------
  // Realtime action boundary — every Gemini/OpenAI realtime start route is
  // deferred by the current registry. Persisted state and direct store calls
  // must not bypass ui_selectable; stop routes remain available for teardown.
  // -----------------------------------------------------------------------

  it("blocks Gemini native converse while the provider is outside the MVP", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      isCapturing: true,
      isGeminiActive: false,
      activeGeminiCommand: null,
      conversationMode: "converse",
      converseEngine: "native",
    });

    await useAudioGraphStore.getState().startGemini();

    expect(invoke).not.toHaveBeenCalled();
    const s = useAudioGraphStore.getState();
    expect(s.isGeminiActive).toBe(false);
    expect(s.activeGeminiCommand).toBeNull();
    expect(s.error).toMatch(
      /Gemini Live is not available for new sessions in the current MVP/i,
    );
  });

  it("blocks the legacy Gemini notes start route while outside the MVP", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      isCapturing: true,
      isGeminiActive: false,
      activeGeminiCommand: null,
      conversationMode: "notes",
      converseEngine: "native",
    });

    await useAudioGraphStore.getState().startGemini();

    expect(invoke).not.toHaveBeenCalled();
    expect(useAudioGraphStore.getState().activeGeminiCommand).toBeNull();
    expect(useAudioGraphStore.getState().error).toMatch(/current MVP/i);
  });

  it("blocks the legacy Gemini pipelined start route while outside the MVP", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      isCapturing: true,
      isGeminiActive: false,
      activeGeminiCommand: null,
      conversationMode: "converse",
      converseEngine: "pipelined",
    });

    await useAudioGraphStore.getState().startGemini();

    expect(invoke).not.toHaveBeenCalled();
    expect(useAudioGraphStore.getState().error).toMatch(/current MVP/i);
  });

  it("blocks OpenAI native realtime while the provider is outside the MVP", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    useAudioGraphStore.setState({
      isCapturing: true,
      isGeminiActive: false,
      activeGeminiCommand: null,
      conversationMode: "converse",
      converseEngine: "native",
      converseRealtimeAgentProvider: "openai",
    });

    await useAudioGraphStore.getState().startGemini();

    expect(invoke).not.toHaveBeenCalled();
    expect(useAudioGraphStore.getState().error).toMatch(
      /OpenAI Realtime voice agent is not available for new sessions in the current MVP/i,
    );
  });

  it("leaves sample preview untouched when a realtime start is deferred", async () => {
    useAudioGraphStore.setState({
      isCapturing: true,
      isGeminiActive: false,
      samplePreviewActive: true,
      conversationMode: "converse",
      converseEngine: "native",
      converseRealtimeAgentProvider: "gemini",
    });

    await useAudioGraphStore.getState().startGemini();

    expect(invoke).not.toHaveBeenCalled();
    expect(useAudioGraphStore.getState().samplePreviewActive).toBe(true);
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
// SHELL-R2 (audio-graph-e0c4, plan §R2, ADR-0046): "stop lands you on your own
// session" — `stopCapture` reads the active session id before stopping and
// routes `nav` to it directly, plus writes the optimistic "finalizing" row
// for the 1d92 gap (`sessions.json` may not have the just-ended session's
// entry yet).
// ---------------------------------------------------------------------------
describe("stopCapture — SHELL-R2 session routing", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useAudioGraphStore.setState({
      selectedSourceIds: ["system-default"],
      isCapturing: true,
      isTranscribing: true,
      captureStartTime: 1_700_000_000_000,
      transcriptSegments: [],
      speakers: [],
      graphSnapshot: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
      nav: { dest: "capture", sessionId: null, lens: "notes" },
      pendingFinalizingSession: null,
      sessions: [],
      error: null,
    });
  });

  it("routes nav to the Sessions destination on the just-ended session, Notes lens", async () => {
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "get_session_id") return "session-abc";
      if (cmd === "stop_capture") return null;
      if (cmd === "list_sessions") return [];
      return undefined;
    });

    await useAudioGraphStore.getState().stopCapture();

    expect(useAudioGraphStore.getState().nav).toEqual({
      dest: "sessions",
      sessionId: "session-abc",
      lens: "notes",
    });
    expect(useAudioGraphStore.getState().isCapturing).toBe(false);
  });

  it("writes the optimistic finalizing row from in-memory capture state, before listSessions() is refreshed", async () => {
    useAudioGraphStore.setState({
      transcriptSegments: [
        {
          id: "seg-1",
          source_id: "system-default",
          speaker_id: null,
          speaker_label: null,
          text: "hi",
          start_time: 0,
          end_time: 1,
          confidence: 0.9,
        },
      ],
      speakers: [
        {
          id: "spk-1",
          label: "Speaker 1",
          color: "#000",
          total_speaking_time: 1,
          segment_count: 1,
        },
      ],
    });
    let listSessionsCalls = 0;
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "get_session_id") return "session-abc";
      if (cmd === "stop_capture") return null;
      if (cmd === "list_sessions") {
        listSessionsCalls += 1;
        // The 1d92 gap: the index hasn't caught up yet.
        return [];
      }
      return undefined;
    });

    await useAudioGraphStore.getState().stopCapture();

    const pending = useAudioGraphStore.getState().pendingFinalizingSession;
    expect(pending).toMatchObject({
      id: "session-abc",
      optimistic: true,
      segment_count: 1,
      speaker_count: 1,
      created_at: 1_700_000_000_000,
    });
    expect(listSessionsCalls).toBeGreaterThanOrEqual(1);
  });

  it("does not route or write an optimistic row when the session id read fails, but still stops capture", async () => {
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "get_session_id") throw new Error("no active session");
      if (cmd === "stop_capture") return null;
      return undefined;
    });

    await useAudioGraphStore.getState().stopCapture();

    expect(useAudioGraphStore.getState().nav).toEqual({
      dest: "capture",
      sessionId: null,
      lens: "notes",
    });
    expect(useAudioGraphStore.getState().pendingFinalizingSession).toBeNull();
    expect(useAudioGraphStore.getState().isCapturing).toBe(false);
  });

  it("clears a PRIOR stop's pending row when this stop's own session id read fails (R2 adversary finding #1: a failed id read voids the resident-data premise, it doesn't inherit a stale one)", async () => {
    useAudioGraphStore.setState({
      pendingFinalizingSession: {
        id: "earlier-stopped-session",
        title: null,
        created_at: 1_690_000_000_000,
        ended_at: 1_690_000_060_000,
        duration_seconds: 60,
        status: "active",
        segment_count: 3,
        speaker_count: 1,
        entity_count: 0,
        transcript_path: "",
        graph_path: "",
        deleted: false,
        deleted_at: null,
        optimistic: true,
      },
    });
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "get_session_id") throw new Error("no active session");
      if (cmd === "stop_capture") return null;
      return undefined;
    });

    await useAudioGraphStore.getState().stopCapture();

    expect(useAudioGraphStore.getState().pendingFinalizingSession).toBeNull();
  });

  it("does not route to or create a pending row for the sample-preview sentinel id (getSessionId degrades to the truthy SAMPLE_SESSION_ID during sample preview rather than throwing — R2 adversary finding #4)", async () => {
    useAudioGraphStore.getState().loadSampleSessionPreview();
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "get_session_id") {
        throw new Error(
          "getSessionId() should short-circuit on samplePreviewActive and never invoke the backend",
        );
      }
      if (cmd === "stop_capture") return null;
      if (cmd === "list_sessions") return [];
      return undefined;
    });

    await useAudioGraphStore.getState().stopCapture();

    expect(useAudioGraphStore.getState().nav).toEqual({
      dest: "capture",
      sessionId: null,
      lens: "notes",
    });
    expect(useAudioGraphStore.getState().pendingFinalizingSession).toBeNull();
    expect(useAudioGraphStore.getState().isCapturing).toBe(false);
  });
});

describe("openSessionsBrowser — SHELL-R2 navigates instead of opening a modal", () => {
  it("sets sessionsBrowserOpen (untouched, per SHELL-R1) AND navigates to the Sessions destination", async () => {
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "list_sessions") return [];
      if (cmd === "purge_expired_sessions") return [];
      return undefined;
    });
    useAudioGraphStore.setState({
      nav: { dest: "capture", sessionId: null, lens: "notes" },
      sessionsBrowserOpen: false,
    });

    useAudioGraphStore.getState().openSessionsBrowser();

    expect(useAudioGraphStore.getState().sessionsBrowserOpen).toBe(true);
    expect(useAudioGraphStore.getState().nav.dest).toBe("sessions");
  });
});

// ---------------------------------------------------------------------------
// ONE START (SHELL-R3, plan §R3, ADR-0046): the merged-start gating proof.
// The critical, verify-before-coding claim this ticket makes is that in CI
// (no durable notes route configured) the NOW STRIP's Start button issues
// EXACTLY `start_capture` — `start_transcribe` must never fire, because an
// unmocked `start_transcribe` falls through to the real Rust command in the
// E2E binary (`e2e/specs/shell.e2e.ts:186`'s fetch-bridge fallthrough) and
// an ERROR-level line from that would break the suite's zero-frontend-
// errors gate (test 5). `local_whisper` — the REAL Rust default
// `AsrProvider` (`src-tauri/src/settings/mod.rs`'s `impl Default`) — is the
// registry's one `ui_selectable: false` ASR descriptor, so it is the
// faithful "no route configured" fixture, not a synthetic stand-in.
// ---------------------------------------------------------------------------
describe("startCaptureAndTranscribe — SHELL-R3 merged Start (ADR-0046)", () => {
  beforeEach(() => {
    useAudioGraphStore.setState({
      selectedSourceIds: ["system-default"],
      isCapturing: false,
      isTranscribing: false,
      error: null,
    });
  });

  it("issues EXACTLY start_capture — never start_transcribe — when no durable route is configured (the real CI default)", async () => {
    const invoked: string[] = [];
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      invoked.push(cmd);
      if (cmd === "start_capture") return null;
      return undefined;
    });
    useAudioGraphStore.setState({
      settings: selectableSettings({
        // The real Rust default: `ui_selectable: false` in the registry —
        // the ADR-0033 gate the old standalone Transcribe button rendered
        // its aria-disabled from, mirrored here via
        // `deferredProviderForDurableStart`.
        asr_provider: { type: "local_whisper" },
      }),
    });

    await useAudioGraphStore.getState().startCaptureAndTranscribe();

    expect(invoked).toContain("start_capture");
    expect(invoked).not.toContain("start_transcribe");
    expect(useAudioGraphStore.getState().isCapturing).toBe(true);
    expect(useAudioGraphStore.getState().isTranscribing).toBe(false);
    // Independent teeth for the outer gate (not just startTranscribe's own
    // internal re-check): if `startCaptureAndTranscribe` dropped its own
    // `deferredProviderForDurableStart` check, execution would still reach
    // `startTranscribe`, whose internal guard would bail WITHOUT invoking
    // `start_transcribe` — but it would also stamp a provider-deferred
    // error onto an otherwise-successful capture start. Asserting `error`
    // stays null here fails under that mutation even though the invoke
    // list alone would not.
    expect(useAudioGraphStore.getState().error).toBeNull();
  });

  it("composes start_transcribe when the durable route IS configured and capture starts", async () => {
    const invoked: string[] = [];
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      invoked.push(cmd);
      if (cmd === "start_capture") return null;
      if (cmd === "start_transcribe") return null;
      return undefined;
    });
    useAudioGraphStore.setState({ settings: selectableSettings() });

    await useAudioGraphStore.getState().startCaptureAndTranscribe();

    expect(invoked).toEqual(["start_capture", "start_transcribe"]);
    expect(useAudioGraphStore.getState().isCapturing).toBe(true);
    expect(useAudioGraphStore.getState().isTranscribing).toBe(true);
  });

  it("skips the transcribe leg when settings have not hydrated yet (mirrors the old canTranscribe's settings !== null facet)", async () => {
    const invoked: string[] = [];
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      invoked.push(cmd);
      if (cmd === "start_capture") return null;
      return undefined;
    });
    // Reachable pre-hydration window: App's credential probe can resolve
    // before `load_settings_cmd`, leaving `settings` null at Start-click
    // time (see the sibling minor-finding fix on this gate).
    useAudioGraphStore.setState({ settings: null });

    await useAudioGraphStore.getState().startCaptureAndTranscribe();

    expect(invoked).toContain("start_capture");
    expect(invoked).not.toContain("start_transcribe");
    expect(useAudioGraphStore.getState().isCapturing).toBe(true);
    expect(useAudioGraphStore.getState().isTranscribing).toBe(false);
    // No spurious "provider settings are still loading" error should
    // surface on top of an otherwise-successful capture start — the outer
    // gate should skip quietly, exactly as the old aria-disabled Transcribe
    // button did.
    expect(useAudioGraphStore.getState().error).toBeNull();
  });

  it("skips the transcribe leg when startCapture itself fails (no atomicity claim — re-reads state, not a pre-await snapshot)", async () => {
    const invoked: string[] = [];
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      invoked.push(cmd);
      if (cmd === "start_capture") throw new Error("mocked-capture-failure");
      return undefined;
    });
    useAudioGraphStore.setState({ settings: selectableSettings() });

    await useAudioGraphStore.getState().startCaptureAndTranscribe();

    // `safeInvoke` relays its own `report_frontend_diagnostic` call on a
    // failed invoke (audio-graph-3e71) — assert on presence/absence, not
    // exact array equality, so that diagnostic doesn't false-fail this test.
    expect(invoked).toContain("start_capture");
    expect(invoked).not.toContain("start_transcribe");
    expect(useAudioGraphStore.getState().isCapturing).toBe(false);
    // Pin the ACTUAL failure message, not just non-null: if
    // `startCaptureAndTranscribe` dropped its own `!isCapturing` re-read
    // gate, it would still reach `startTranscribe`, whose internal
    // `!isCapturing` guard bails without invoking `start_transcribe` — but
    // it overwrites this error with "Cannot start transcription: capture
    // is not running", masking the real capture failure from the user.
    expect(useAudioGraphStore.getState().error).toMatch(
      /mocked-capture-failure/,
    );
  });
});

// ---------------------------------------------------------------------------
// safeInvoke adoption (audio-graph-3e71): the store routes every Rust IPC
// through `safeInvoke` (imported as `invoke`). A failed store action must both
// (a) keep its existing catch → error-state behavior AND (b) relay EXACTLY ONE
// analytics diagnostic — tagged with the command NAME, never the args — so a
// failure is reported once and no payload content leaks (ADR-0023).
// ---------------------------------------------------------------------------
describe("AudioGraphStore ⇄ safeInvoke analytics chokepoint (3e71)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useAudioGraphStore.setState({ models: [], error: null });
  });

  it("a failed store action sets error state AND captures one command-name diagnostic (never args)", async () => {
    const SECRET_ARG = "sk-should-never-be-relayed";
    // The store command rejects; the SECOND invoke is safeInvoke's telemetry
    // relay (`report_frontend_diagnostic`), which resolves.
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "list_available_models") {
        throw new Error(`model list failed: ${SECRET_ARG}`);
      }
      return undefined;
    });

    await useAudioGraphStore.getState().fetchModels();

    // (a) Existing catch behavior preserved: the humanized error lands in state.
    expect(useAudioGraphStore.getState().error).toMatch(/model list failed/i);

    // (b) Exactly one diagnostic relayed for the failing command.
    const diagnosticCalls = vi
      .mocked(invoke)
      .mock.calls.filter((c) => c[0] === "report_frontend_diagnostic");
    expect(diagnosticCalls).toHaveLength(1);

    // The diagnostic carries the command NAME as `component` and no free text.
    const payload = diagnosticCalls[0][1] as {
      name: string;
      category: string;
      component: string | null;
      surface: string | null;
    };
    expect(payload).toEqual({
      name: "frontend.invoke.error",
      category: "frontend",
      component: "list_available_models",
      surface: "invoke",
    });
    // Privacy: nothing about the args/payload or the error message rides along.
    const json = JSON.stringify(diagnosticCalls[0]);
    expect(json).not.toContain(SECRET_ARG);
    expect(json).not.toContain("model list failed");
  });

  it("a successful store action relays NO diagnostic (telemetry is failure-only)", async () => {
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "list_available_models") return [];
      return undefined;
    });

    await useAudioGraphStore.getState().fetchModels();

    expect(useAudioGraphStore.getState().error).toBeNull();
    expect(invoke).not.toHaveBeenCalledWith(
      "report_frontend_diagnostic",
      expect.anything(),
    );
  });

  it("the non-streaming chat fallback (expected-Err capability probe) relays NO diagnostic", async () => {
    // start_streaming_chat is a CAPABILITY PROBE: the backend documents
    // returning Err when the provider doesn't stream so sendChatMessage falls
    // back to the blocking send_chat_message (commands.rs:2292-2294). That
    // rejection is expected control flow — a successful blocking-chat session
    // must NOT emit a frontend.invoke.error (the probe bypasses safeInvoke via
    // rawInvoke; the fallback command itself stays captured).
    useAudioGraphStore.setState({ chatMessages: [], isChatLoading: false });
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "start_streaming_chat") {
        throw new Error("streaming unsupported by provider");
      }
      if (cmd === "send_chat_message") {
        return {
          message: { role: "assistant", content: "blocking reply" },
          tokens_used: 3,
        };
      }
      return undefined;
    });

    await useAudioGraphStore.getState().sendChatMessage("hello");

    // The fallback completed successfully…
    const s = useAudioGraphStore.getState();
    expect(s.chatMessages.at(-1)?.content).toBe("blocking reply");
    expect(s.isChatLoading).toBe(false);
    // …and ZERO diagnostics were relayed for the expected probe rejection.
    const diagnosticCalls = vi
      .mocked(invoke)
      .mock.calls.filter((c) => c[0] === "report_frontend_diagnostic");
    expect(diagnosticCalls).toHaveLength(0);
  });

  it("a REAL fallback failure (send_chat_message rejects) still relays exactly one diagnostic", async () => {
    // Guard the counterpart: bypassing the probe must not blind us to genuine
    // chat failures — the blocking command stays on the safeInvoke chokepoint.
    useAudioGraphStore.setState({ chatMessages: [], isChatLoading: false });
    vi.mocked(invoke).mockImplementation(async (cmd) => {
      if (cmd === "start_streaming_chat") {
        throw new Error("streaming unsupported by provider");
      }
      if (cmd === "send_chat_message") {
        throw new Error("provider exploded");
      }
      return undefined;
    });

    await useAudioGraphStore.getState().sendChatMessage("hello");

    // Existing UI behavior: the error lands in the assistant slot.
    expect(useAudioGraphStore.getState().chatMessages.at(-1)?.content).toMatch(
      /provider exploded/i,
    );
    // Exactly one capture — for the fallback command, not the probe.
    const diagnosticCalls = vi
      .mocked(invoke)
      .mock.calls.filter((c) => c[0] === "report_frontend_diagnostic");
    expect(diagnosticCalls).toHaveLength(1);
    expect(
      (diagnosticCalls[0][1] as { component: string | null }).component,
    ).toBe("send_chat_message");
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

  it("carries source_segment_id from an added delta edge onto the link (b272)", () => {
    seed();
    useAudioGraphStore.getState().applyGraphDelta({
      ...emptyDelta,
      added_edges: [
        {
          id: "edge-EdgeIndex(1)",
          source: "b",
          target: "a",
          relation_type: "mentions",
          weight: 1,
          color: "#999999",
          label: "mentions",
          source_segment_id: "seg-42",
        },
      ],
    });

    const link = useAudioGraphStore
      .getState()
      .graphSnapshot.links.find((l) => l.id === "edge-EdgeIndex(1)");
    expect(link?.source_segment_id).toBe("seg-42");
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
