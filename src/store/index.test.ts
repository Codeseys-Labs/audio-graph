import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "./index";

describe("AudioGraphStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useAudioGraphStore.setState({
      audioSources: [],
      selectedSourceIds: [],
      transcriptSegments: [],
      agentProposals: [],
      approvingAgentProposalIds: [],
      chatMessages: [],
      isChatLoading: false,
      streamingChatRequestId: null,
      isCapturing: false,
      captureStartTime: null,
      error: null,
    });
  });

  it("starts with empty state", () => {
    const s = useAudioGraphStore.getState();
    expect(s.audioSources).toEqual([]);
    expect(s.selectedSourceIds).toEqual([]);
    expect(s.isCapturing).toBe(false);
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

  it("sets and clears error state", () => {
    useAudioGraphStore.getState().setError("boom");
    expect(useAudioGraphStore.getState().error).toBe("boom");
    useAudioGraphStore.getState().clearError();
    expect(useAudioGraphStore.getState().error).toBeNull();
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
    vi.mocked(invoke).mockResolvedValueOnce({
      proposal_id: "proposal-1",
      action: "graph_update",
      message: "Approved agent proposal\n\nAlice met Bob.",
      graph_updated: true,
      timestamp_ms: 20,
    });

    const result = await useAudioGraphStore
      .getState()
      .approveAgentProposal("proposal-1");

    expect(invoke).toHaveBeenCalledWith("approve_agent_proposal", {
      proposalId: "proposal-1",
    });
    expect(result?.graph_updated).toBe(true);
    expect(useAudioGraphStore.getState().agentProposals).toEqual([]);
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

    resolveInvoke({
      proposal_id: "proposal-2",
      action: "chat_note",
      message: "Approved agent proposal for review\n\nRemember this.",
      graph_updated: false,
      timestamp_ms: 40,
    });
    await first;

    expect(useAudioGraphStore.getState().approvingAgentProposalIds).toEqual([]);
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
});
