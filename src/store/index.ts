/**
 * Global frontend store (Zustand) — the single source of truth for UI
 * state plus the invoke-bridge to the Rust backend.
 *
 * Slice layout:
 *   - Audio sources          — `audioSources`, `selectedSourceIds`,
 *                              `searchFilter`, `processes` + fetchers.
 *   - Capture lifecycle      — `isCapturing`, `captureStartTime`,
 *                              `startCapture` / `stopCapture` (wrap
 *                              `start_capture` / `stop_capture`).
 *   - Transcribe pipeline    — `isTranscribing`, `startTranscribe` /
 *                              `stopTranscribe` + the live
 *                              `transcriptSegments` buffer populated by
 *                              `TRANSCRIPT_UPDATE` events.
 *   - Gemini Live            — `isGeminiActive`, `startGemini` /
 *                              `stopGemini`, plus the separate
 *                              `geminiTranscripts` buffer appended by
 *                              `GEMINI_TRANSCRIPTION` events.
 *   - Knowledge graph        — `graphSnapshot` (refreshed on
 *                              `GRAPH_UPDATE`) + the `exportGraph` and
 *                              `getSessionId` command wrappers.
 *   - Speakers               — `speakers` (upserted on
 *                              `SPEAKER_DETECTED` events).
 *   - Pipeline status        — `pipelineStatus` + per-source
 *                              `backpressuredSources` set.
 *   - Chat                   — `chatMessages`, `isChatLoading`,
 *                              `sendChatMessage`, `clearChatHistory`.
 *   - Settings / UI          — `settings`, `loadSettings`,
 *                              `settingsOpen` / `sessionsBrowserOpen`
 *                              modal flags + `rightPanelTab` tab state.
 *   - Error + toast wiring   — `error`, `setError`, `clearError`.
 *
 * The invoke-bridge contract: each async action that touches Rust
 * wraps `invoke<T>(command, args)` and translates thrown errors via
 * `utils/errorToMessage`. Events flow the other way — `useTauriEvents`
 * mutates this store on every backend event. See that hook for the
 * full list of subscriptions.
 *
 * Unit tests that exercise slices pull `useAudioGraphStore.getState()`
 * directly and `setState` to seed fixtures.
 */
import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import type {
    AgentActionResult,
    ApiEndpointConfig,
    AppSettings,
    AgentProposalEvent,
    AgentStatusEvent,
    AudioGraphStore,
    AsrPartialEvent,
    AudioSourceInfo,
    ChatMessage,
    ChatResponse,
    ChatTokenDeltaEvent,
    ChatTokenDoneEvent,
    GeminiTranscriptEntry,
    GraphDelta,
    GraphLink,
    GraphNode,
    LoadedSession,
    ModelInfo,
    ModelStatus,
    PipelineLatencyEvent,
    ProcessInfo,
    SessionRecoveryReport,
    SessionMetadata,
    StageStatus,
    TranscriptSegment,
    TurnLifecycleEvent,
} from "../types";
import { removeExclusiveCapturePeer } from "../utils/captureTarget";
import { errorToMessage } from "../utils/errorToMessage";

const idleStage: StageStatus = { type: "Idle" };

function graphEndpointId(endpoint: GraphLink["source"]): string {
    return typeof endpoint === "string" ? endpoint : endpoint.id;
}

function graphLinkId(link: GraphLink): string {
    return (
        link.id ??
        `${graphEndpointId(link.source)}->${graphEndpointId(link.target)}:${link.relation_type}:${link.label ?? ""}`
    );
}

// react-force-graph stores live x/y on the node objects. New nodes arrive
// without coordinates and default to (0,0) — when several appear at once they
// pile on the origin and every edge fans to that single point (the "jank").
// Seed each new node near an already-positioned neighbour (or near the centre
// with jitter) so it enters the layout in a sensible place instead of origin.
type PositionedNode = GraphNode & { x?: number; y?: number };
function seedNodePositions(
    nextNodes: GraphNode[],
    links: GraphLink[],
    positioned: Map<string, PositionedNode>,
) {
    const adjacency = new Map<string, string[]>();
    for (const link of links) {
        const s = graphEndpointId(link.source);
        const t = graphEndpointId(link.target);
        (adjacency.get(s) ?? adjacency.set(s, []).get(s)!).push(t);
        (adjacency.get(t) ?? adjacency.set(t, []).get(t)!).push(s);
    }
    for (const node of nextNodes as PositionedNode[]) {
        if (typeof node.x === "number" && typeof node.y === "number") continue;
        let seeded = false;
        for (const neighborId of adjacency.get(node.id) ?? []) {
            const nb = positioned.get(neighborId);
            if (nb && typeof nb.x === "number" && typeof nb.y === "number") {
                node.x = nb.x + (Math.random() - 0.5) * 40;
                node.y = nb.y + (Math.random() - 0.5) * 40;
                seeded = true;
                break;
            }
        }
        if (!seeded) {
            // Spread around the centre rather than stacking on (0,0).
            const angle = Math.random() * Math.PI * 2;
            const radius = 60 + Math.random() * 120;
            node.x = Math.cos(angle) * radius;
            node.y = Math.sin(angle) * radius;
        }
    }
}

export const useAudioGraphStore = create<AudioGraphStore>((set, get) => ({
    // ── Audio sources ────────────────────────────────────────────────────
    audioSources: [],
    selectedSourceIds: [],
    setAudioSources: (sources) => set({ audioSources: sources }),
    toggleSourceId: (id) =>
        set((state) => {
            const idx = state.selectedSourceIds.indexOf(id);
            if (idx >= 0) {
                return { selectedSourceIds: state.selectedSourceIds.filter((sid) => sid !== id) };
            }
            return {
                selectedSourceIds: [...removeExclusiveCapturePeer(state.selectedSourceIds, id), id],
            };
        }),
    clearSelectedSources: () => set({ selectedSourceIds: [] }),
    fetchSources: async () => {
        try {
            const sources = await invoke<AudioSourceInfo[]>("list_audio_sources");
            set({ audioSources: sources, error: null });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },

    // ── Processes ────────────────────────────────────────────────────────
    processes: [],
    searchFilter: '',
    fetchProcesses: async () => {
        try {
            const processes = await invoke<ProcessInfo[]>("list_running_processes");
            set({ processes });
        } catch (err) {
            console.error("Failed to fetch processes:", err);
        }
    },
    setSearchFilter: (filter: string) => set({ searchFilter: filter }),

    // ── Transcript ───────────────────────────────────────────────────────
    transcriptSegments: [],
    asrPartial: null,
    turnEvents: [],
    agentStatus: null,
    agentProposals: [],
    approvingAgentProposalIds: [],
    addTranscriptSegment: (segment) =>
        set((state) => ({
            transcriptSegments: [...state.transcriptSegments.slice(-499), segment],
            asrPartial: null,
        })),
    setAsrPartial: (partial: AsrPartialEvent | null) => set({ asrPartial: partial }),
    addTurnEvent: (event: TurnLifecycleEvent) =>
        set((state) => ({
            turnEvents: [...state.turnEvents.slice(-99), event],
        })),
    setAgentStatus: (status: AgentStatusEvent | null) => set({ agentStatus: status }),
    addAgentProposal: (proposal: AgentProposalEvent) => {
        set((state) => ({
            agentProposals: [...state.agentProposals.slice(-49), proposal],
        }));
        // Questions default to the graph: auto-record a Question node (local,
        // no LLM, never rate-limits). The card then only offers the OPTIONAL
        // "Ask AI" action to fetch a possible answer.
        if (proposal.kind === "question") {
            const text =
                proposal.body?.replace(
                    /^Consider answering or linking this question:\s*/i,
                    "",
                ) || proposal.title;
            void Promise.resolve(
                invoke("add_question_to_graph", {
                    text,
                    speaker: proposal.speaker_label ?? null,
                    sourceSegmentId: proposal.source_segment_id ?? null,
                }),
            ).catch((err) =>
                console.error("auto add_question_to_graph failed:", err),
            );
        }
    },
    askAgentProposal: async (proposalId: string) => {
        const proposal = get().agentProposals.find((p) => p.id === proposalId);
        if (!proposal) return;
        const question =
            proposal.body?.replace(
                /^Consider answering or linking this question:\s*/i,
                "",
            ) || proposal.title;
        // Drop the card + clear the server-side pending entry, then route the
        // question through the normal streaming chat (429-safe: errors surface
        // in the chat bubble rather than throwing).
        set((state) => ({
            agentProposals: state.agentProposals.filter((p) => p.id !== proposalId),
            approvingAgentProposalIds: state.approvingAgentProposalIds.filter(
                (id) => id !== proposalId,
            ),
        }));
        void invoke("dismiss_agent_proposal", { proposalId }).catch(() => {});
        await get().sendChatMessage(question);
    },
    approveAgentProposal: async (proposalId: string) => {
        const proposal = get().agentProposals.find((item) => item.id === proposalId);
        if (!proposal) {
            set({ error: "Agent proposal no longer exists" });
            return null;
        }
        if (get().approvingAgentProposalIds.includes(proposalId)) {
            return null;
        }
        set((state) => ({
            approvingAgentProposalIds: [
                ...state.approvingAgentProposalIds,
                proposalId,
            ],
        }));
        try {
            const result = await invoke<AgentActionResult>("approve_agent_proposal", {
                proposalId,
            });
            const message: ChatMessage = {
                role: "assistant",
                content: result.message,
            };
            set((state) => ({
                agentProposals: state.agentProposals.filter((item) => item.id !== proposalId),
                approvingAgentProposalIds: state.approvingAgentProposalIds.filter(
                    (id) => id !== proposalId,
                ),
                chatMessages: [...state.chatMessages, message],
                error: null,
            }));
            return result;
        } catch (e) {
            set((state) => ({
                approvingAgentProposalIds: state.approvingAgentProposalIds.filter(
                    (id) => id !== proposalId,
                ),
                error: errorToMessage(e),
            }));
            return null;
        }
    },
    dismissAgentProposal: (proposalId: string) => {
        void invoke("dismiss_agent_proposal", { proposalId }).catch((err) => {
            console.error("Failed to dismiss agent proposal:", err);
        });
        set((state) => ({
            agentProposals: state.agentProposals.filter((item) => item.id !== proposalId),
            approvingAgentProposalIds: state.approvingAgentProposalIds.filter(
                (id) => id !== proposalId,
            ),
        }));
    },
    clearAgentProposals: () => {
        void invoke("clear_agent_proposals").catch((err) => {
            console.error("Failed to clear agent proposals:", err);
        });
        set({ agentProposals: [], approvingAgentProposalIds: [] });
    },
    clearTranscript: () =>
        set({
            transcriptSegments: [],
            asrPartial: null,
            turnEvents: [],
            agentStatus: null,
            agentProposals: [],
            approvingAgentProposalIds: [],
        }),

    // ── Knowledge graph ──────────────────────────────────────────────────
    graphSnapshot: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
    },
    setGraphSnapshot: (snapshot) =>
        set((state) => {
            // Preserve node object identity across snapshots. react-force-graph
            // stores each node's live simulation state (x/y/vx/vy/fx/fy) ON the
            // node object; if we hand it brand-new objects every GRAPH_UPDATE the
            // D3 force sim reheats and all nodes jump. Reuse the prior object for
            // any node whose id we already have (refreshing its data fields), so
            // the layout stays warm and stable.
            const prev = new Map(state.graphSnapshot.nodes.map((n) => [n.id, n]));
            const nodes = snapshot.nodes.map((incoming) => {
                const existing = prev.get(incoming.id);
                return existing ? Object.assign(existing, incoming) : incoming;
            });
            // Seed positions for any node that doesn't have one yet (new nodes),
            // using already-positioned nodes from the merged set.
            const positioned = new Map(nodes.map((n) => [n.id, n]));
            seedNodePositions(nodes, snapshot.links, positioned);
            return { graphSnapshot: { ...snapshot, nodes } };
        }),
    applyGraphDelta: (delta: GraphDelta) =>
        set((state) => {
            const removedNodes = new Set(delta.removed_node_ids);
            const removedEdges = new Set(delta.removed_edge_ids);
            const prev = new Map(state.graphSnapshot.nodes.map((n) => [n.id, n]));
            const nodes = new Map<string, GraphNode>();

            for (const node of state.graphSnapshot.nodes) {
                if (!removedNodes.has(node.id)) {
                    nodes.set(node.id, node);
                }
            }
            for (const node of delta.added_nodes) {
                nodes.set(node.id, node);
            }
            for (const node of delta.updated_nodes) {
                // Merge onto the existing object to keep its x/y; a fresh object
                // would reset the node to origin and reheat the layout.
                const existing = prev.get(node.id);
                nodes.set(node.id, existing ? Object.assign(existing, node) : node);
            }

            const nextNodes = [...nodes.values()];
            const links = new Map<string, GraphLink>();
            for (const link of state.graphSnapshot.links) {
                const source = graphEndpointId(link.source);
                const target = graphEndpointId(link.target);
                const id = graphLinkId(link);
                if (
                    !removedEdges.has(id) &&
                    !removedNodes.has(source) &&
                    !removedNodes.has(target)
                ) {
                    links.set(id, link);
                }
            }
            for (const edge of delta.added_edges) {
                links.set(edge.id, edge);
            }

            const nextLinks = [...links.values()];
            seedNodePositions(nextNodes, nextLinks, nodes as Map<string, PositionedNode>);
            return {
                graphSnapshot: {
                    nodes: nextNodes,
                    links: nextLinks,
                    stats: {
                        total_nodes: nextNodes.length,
                        total_edges: nextLinks.length,
                        total_episodes: state.graphSnapshot.stats.total_episodes,
                    },
                },
            };
        }),

    // ── Exports ──────────────────────────────────────────────────────────
    exportTranscript: async () => {
        return await invoke<string>("export_transcript");
    },
    exportGraph: async () => {
        return await invoke<string>("export_graph");
    },
    getSessionId: async () => {
        return await invoke<string>("get_session_id");
    },

    // ── Pipeline status ──────────────────────────────────────────────────
    pipelineStatus: {
        capture: idleStage,
        pipeline: idleStage,
        asr: idleStage,
        diarization: idleStage,
        entity_extraction: idleStage,
        graph: idleStage,
    },
    setPipelineStatus: (status) => set({ pipelineStatus: status }),
    pipelineLatencies: {},
    setPipelineLatency: (sample: PipelineLatencyEvent) =>
        set((state) => ({
            pipelineLatencies: {
                ...state.pipelineLatencies,
                [sample.stage]: sample,
            },
        })),

    // ── Speakers ─────────────────────────────────────────────────────────
    speakers: [],
    addOrUpdateSpeaker: (speaker) =>
        set((state) => {
            const idx = state.speakers.findIndex((s) => s.id === speaker.id);
            if (idx >= 0) {
                const updated = [...state.speakers];
                updated[idx] = speaker;
                return { speakers: updated };
            }
            return { speakers: [...state.speakers, speaker] };
        }),
    clearSpeakers: () => set({ speakers: [] }),

    // ── Capture state ────────────────────────────────────────────────────
    isCapturing: false,
    captureStartTime: null,
    setIsCapturing: (capturing) => set({ isCapturing: capturing }),
    backpressuredSources: [],
    setSourceBackpressure: (sourceId, isBackpressured) =>
        set((state) => {
            const present = state.backpressuredSources.includes(sourceId);
            if (isBackpressured && !present) {
                return { backpressuredSources: [...state.backpressuredSources, sourceId] };
            }
            if (!isBackpressured && present) {
                return {
                    backpressuredSources: state.backpressuredSources.filter(
                        (id) => id !== sourceId,
                    ),
                };
            }
            return {};
        }),
    startCapture: async () => {
        const { selectedSourceIds } = get();
        if (selectedSourceIds.length === 0) {
            set({ error: "No audio source selected" });
            return;
        }
        const startedSourceIds: string[] = [];
        try {
            for (const sourceId of selectedSourceIds) {
                await invoke("start_capture", { sourceId });
                startedSourceIds.push(sourceId);
            }
            set({
                isCapturing: true,
                captureStartTime: Date.now(),
                error: null,
            });
        } catch (e) {
            await Promise.allSettled(
                startedSourceIds.map((sourceId) =>
                    invoke("stop_capture", { sourceId }),
                ),
            );
            set({ error: errorToMessage(e) });
        }
    },
    stopCapture: async () => {
        const { selectedSourceIds } = get();
        if (selectedSourceIds.length === 0) return;
        try {
            for (const sourceId of selectedSourceIds) {
                await invoke("stop_capture", { sourceId });
            }
            set({
                isCapturing: false,
                isTranscribing: false,
                isGeminiActive: false,
                captureStartTime: null,
                backpressuredSources: [],
                error: null,
            });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },

    // ── Transcribe state ────────────────────────────────────────────────────────
    isTranscribing: false,
    startTranscribe: async () => {
        const { isCapturing } = get();
        if (!isCapturing) {
            set({ error: "Cannot start transcription: capture is not running" });
            return;
        }
        try {
            await invoke("start_transcribe");
            set({
                isTranscribing: true,
                error: null,
            });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },
    stopTranscribe: async () => {
        try {
            await invoke("stop_transcribe");
            set({
                isTranscribing: false,
                error: null,
            });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },

    // ── Gemini Live dual pipeline ─────────────────────────────────────────
    isGeminiActive: false,
    geminiTranscripts: [],
    addGeminiTranscript: (entry: GeminiTranscriptEntry) =>
        set((state) => ({
            geminiTranscripts: [...state.geminiTranscripts.slice(-499), entry],
        })),
    clearGeminiTranscripts: () => set({ geminiTranscripts: [] }),
    startGemini: async () => {
        const { isCapturing } = get();
        if (!isCapturing) {
            set({ error: "Cannot start Gemini: capture is not running" });
            return;
        }
        try {
            await invoke("start_gemini");
            set({
                isGeminiActive: true,
                error: null,
            });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },
    stopGemini: async () => {
        try {
            await invoke("stop_gemini");
            set({
                isGeminiActive: false,
                error: null,
            });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },

    // ── Error state ──────────────────────────────────────────────────────
    error: null,
    setError: (error) => set({ error }),
    clearError: () => set({ error: null }),

    // ── Chat ─────────────────────────────────────────────────────────────
    chatMessages: [],
    isChatLoading: false,
    streamingChatRequestId: null,
    rightPanelTab: "transcript",
    setRightPanelTab: (tab) => set({ rightPanelTab: tab }),
    sendChatMessage: async (message: string) => {
        // Optimistic user message + empty assistant placeholder for the
        // streaming reply to grow into. Streaming-token-delta events
        // append onto the placeholder; finalizeChatStream replaces its
        // content with the authoritative full_text from the Done event.
        const userMsg: ChatMessage = { role: "user", content: message };
        const assistantPlaceholder: ChatMessage = {
            role: "assistant",
            content: "",
        };
        set((state) => ({
            chatMessages: [...state.chatMessages, userMsg, assistantPlaceholder],
            isChatLoading: true,
        }));

        // Try streaming first (Api / OpenRouter providers). If the active
        // provider doesn't support streaming yet, fall back to the
        // blocking command — its events-vs-promise contract is identical
        // from the UI's perspective: replace the placeholder with the
        // final assistant message.
        try {
            const requestId = await invoke<string>("start_streaming_chat", { message });
            set({ streamingChatRequestId: requestId });
            // The chat-token-delta / chat-token-done event listeners in
            // useTauriEvents take it from here. They use `requestId` to
            // route into the placeholder we just inserted.
            return;
        } catch (streamErr) {
            // Streaming failed (most likely: provider doesn't support it).
            // Fall through to the legacy blocking path.
            console.info(
                "Streaming chat unavailable; using blocking path:",
                streamErr,
            );
        }

        try {
            const response = await invoke<ChatResponse>("send_chat_message", { message });
            set((state) => {
                // Replace the empty placeholder (last message) with the
                // real assistant message. If the placeholder is no longer
                // last (concurrent agent proposal etc.), append.
                const last = state.chatMessages[state.chatMessages.length - 1];
                const isPlaceholder =
                    last && last.role === "assistant" && last.content === "";
                const trimmed = isPlaceholder
                    ? state.chatMessages.slice(0, -1)
                    : state.chatMessages;
                return {
                    chatMessages: [...trimmed, response.message],
                    isChatLoading: false,
                    streamingChatRequestId: null,
                };
            });
        } catch (e) {
            const errorMsg: ChatMessage = {
                role: "assistant",
                content: `Error: ${errorToMessage(e)}`,
            };
            set((state) => {
                const last = state.chatMessages[state.chatMessages.length - 1];
                const isPlaceholder =
                    last && last.role === "assistant" && last.content === "";
                const trimmed = isPlaceholder
                    ? state.chatMessages.slice(0, -1)
                    : state.chatMessages;
                return {
                    chatMessages: [...trimmed, errorMsg],
                    isChatLoading: false,
                    streamingChatRequestId: null,
                };
            });
        }
    },
    appendChatTokenDelta: (event: ChatTokenDeltaEvent) => {
        // Only apply deltas for the currently-tracked request id. Reject
        // when there is NO active stream (current === null) — that means
        // either we never registered this request_id, or clearChatHistory
        // ran mid-stream. Reject mismatched ids too (user started a second
        // stream while the first was still draining — rare but possible).
        const current = get().streamingChatRequestId;
        if (current === null || current !== event.request_id) {
            return;
        }
        set((state) => {
            if (state.chatMessages.length === 0) return {};
            const last = state.chatMessages[state.chatMessages.length - 1];
            if (last.role !== "assistant") return {};
            const updated: ChatMessage = {
                ...last,
                content: last.content + event.delta,
            };
            return {
                chatMessages: [
                    ...state.chatMessages.slice(0, -1),
                    updated,
                ],
            };
        });
    },
    finalizeChatStream: (event: ChatTokenDoneEvent) => {
        const current = get().streamingChatRequestId;
        if (current !== null && current !== event.request_id) {
            return;
        }
        set((state) => {
            if (state.chatMessages.length === 0) {
                return {
                    isChatLoading: false,
                    streamingChatRequestId: null,
                };
            }
            const last = state.chatMessages[state.chatMessages.length - 1];
            if (last.role !== "assistant") {
                return {
                    isChatLoading: false,
                    streamingChatRequestId: null,
                };
            }
            // Authoritative: replace whatever the deltas accumulated with
            // the final full_text. Handles three cases:
            //   1. The provider revised the reply on the terminal chunk.
            //   2. We dropped a delta event (network glitch, IPC backlog).
            //   3. The stream terminated with an error (e.g. HTTP 429 rate
            //      limit) — surface it in the bubble instead of leaving it
            //      blank, which previously looked like a silent hang.
            let finalContent: string;
            if (event.finish_reason?.startsWith("error:")) {
                const detail = event.finish_reason.slice("error:".length).trim();
                const friendly = /429|rate.?limit|too many requests/i.test(detail)
                    ? "Rate limited by the model provider (HTTP 429). The free tier is capped — switch to a non-`:free` OpenRouter model or add credits, then try again."
                    : detail || "the request failed";
                finalContent =
                    (event.full_text ? event.full_text + "\n\n" : "") +
                    `⚠️ Chat failed: ${friendly}`;
            } else if (event.finish_reason === "cancelled" && event.full_text === "") {
                finalContent = last.content + " [cancelled]";
            } else {
                finalContent = event.full_text;
            }
            const updated: ChatMessage = { ...last, content: finalContent };
            return {
                chatMessages: [...state.chatMessages.slice(0, -1), updated],
                isChatLoading: false,
                streamingChatRequestId: null,
            };
        });
    },
    clearChatHistory: async () => {
        try {
            await invoke("clear_chat_history");
            set({ chatMessages: [], streamingChatRequestId: null });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },

    // ── Models ────────────────────────────────────────────────────────────
    models: [],
    isDownloading: false,
    downloadProgress: null,
    fetchModels: async () => {
        try {
            const models = await invoke<ModelInfo[]>("list_available_models");
            set({ models, error: null });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },
    downloadModel: async (filename: string) => {
        set({ isDownloading: true, downloadProgress: null });
        try {
            await invoke("download_model_cmd", { modelFilename: filename });
            // Refresh model list after download
            const models = await invoke<ModelInfo[]>("list_available_models");
            set({ models, isDownloading: false, error: null });
        } catch (e) {
            set({
                isDownloading: false,
                error: errorToMessage(e),
            });
        }
    },

    // ── API endpoint ──────────────────────────────────────────────────────
    apiConfig: null,
    configureApiEndpoint: async (config: ApiEndpointConfig) => {
        try {
            await invoke("configure_api_endpoint", {
                endpoint: config.endpoint,
                apiKey: config.apiKey ?? null,
                model: config.model,
            });
            set({ apiConfig: config, error: null });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },
    clearApiEndpoint: () => set({ apiConfig: null }),

    // ── Settings ──────────────────────────────────────────────────────────
    settings: null,
    modelStatus: null,
    settingsOpen: false,
    settingsLoading: false,
    isDeletingModel: null,

    openSettings: () => {
        set({ settingsOpen: true });
        const { fetchSettings, fetchModels, fetchModelStatus } = get();
        fetchSettings();
        fetchModels();
        fetchModelStatus();
    },
    closeSettings: () => set({ settingsOpen: false }),

    fetchSettings: async () => {
        set({ settingsLoading: true });
        try {
            const settings = await invoke<AppSettings>("load_settings_cmd");
            set({ settings, settingsLoading: false, error: null });
        } catch (e) {
            set({
                settingsLoading: false,
                error: errorToMessage(e),
            });
        }
    },
    saveSettings: async (settings: AppSettings) => {
        try {
            await invoke("save_settings_cmd", { settings });
            set({ settings, error: null });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },
    fetchModelStatus: async () => {
        try {
            const modelStatus = await invoke<ModelStatus>("get_model_status");
            set({ modelStatus, error: null });
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },
    deleteModel: async (filename: string) => {
        set({ isDeletingModel: filename });
        try {
            await invoke("delete_model_cmd", { modelFilename: filename });
            // Refresh models and model status after deletion
            const models = await invoke<ModelInfo[]>("list_available_models");
            const modelStatus = await invoke<ModelStatus>("get_model_status");
            set({ models, modelStatus, isDeletingModel: null, error: null });
        } catch (e) {
            set({
                isDeletingModel: null,
                error: errorToMessage(e),
            });
        }
    },

    // ── Credentials ───────────────────────────────────────────────────────
    saveCredential: async (key: string, value: string) => {
        await invoke("save_credential_cmd", { key, value });
    },
    loadCredential: async (key: string) => {
        const value = await invoke<string | null>("load_credential_cmd", { key });
        return value;
    },
    deleteCredential: async (key: string) => {
        await invoke("delete_credential_cmd", { key });
    },

    // ── AWS profile discovery ─────────────────────────────────────────────
    listAwsProfiles: async () => {
        try {
            return await invoke<string[]>("list_aws_profiles");
        } catch (e) {
            console.error("Failed to list AWS profiles:", e);
            return [];
        }
    },

    // ── Sessions (v2) ─────────────────────────────────────────────────────
    sessionsBrowserOpen: false,
    sessions: [],
    sessionsLoading: false,
    openSessionsBrowser: () => {
        set({ sessionsBrowserOpen: true });
        const { listSessions, purgeExpiredSessions } = get();
        // Lazy cleanup of expired trash on every open. Fire-and-forget —
        // purge failures must not block the browser from rendering.
        void purgeExpiredSessions().catch(() => {});
        // Fetch fresh on open; ignore errors (handled inside listSessions).
        // Larger limit (200) than v1's 10 — the browser has its own search/
        // sort UI, so a bigger pool makes filtering meaningful.
        void listSessions(200).catch(() => {});
    },
    closeSessionsBrowser: () => set({ sessionsBrowserOpen: false }),
    listSessions: async (limit?: number) => {
        set({ sessionsLoading: true });
        try {
            const sessions = await invoke<SessionMetadata[]>("list_sessions", {
                limit: limit ?? null,
            });
            set({ sessions, sessionsLoading: false, error: null });
            return sessions;
        } catch (e) {
            set({
                sessionsLoading: false,
                error: errorToMessage(e),
            });
            return [];
        }
    },
    loadSessionTranscript: async (sessionId: string) => {
        try {
            const segments = await invoke<TranscriptSegment[]>(
                "load_session_transcript",
                { sessionId },
            );
            // Replace current transcript view with the loaded session's segments.
            set({ transcriptSegments: segments, error: null });
            return segments;
        } catch (e) {
            set({ error: errorToMessage(e) });
            return [];
        }
    },
    loadSession: async (sessionId: string) => {
        try {
            const loaded = await invoke<LoadedSession>("load_session", { sessionId });
            set({
                transcriptSegments: loaded.transcript,
                graphSnapshot: loaded.graph,
                error: null,
            });
            return loaded;
        } catch (e) {
            set({ error: errorToMessage(e) });
            return null;
        }
    },
    // Soft-delete: flips `deleted = true` in the index; files stay on disk.
    deleteSession: async (sessionId: string) => {
        try {
            await invoke("delete_session", { sessionId });
            set((state) => ({
                sessions: state.sessions.map((s) =>
                    s.id === sessionId
                        ? { ...s, deleted: true, deleted_at: Date.now() }
                        : s,
                ),
                error: null,
            }));
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },
    restoreSession: async (sessionId: string) => {
        try {
            await invoke("restore_session", { sessionId });
            set((state) => ({
                sessions: state.sessions.map((s) =>
                    s.id === sessionId
                        ? { ...s, deleted: false, deleted_at: null }
                        : s,
                ),
                error: null,
            }));
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },
    deleteSessionPermanently: async (sessionId: string) => {
        try {
            await invoke("delete_session_permanently", { sessionId });
            set((state) => ({
                sessions: state.sessions.filter((s) => s.id !== sessionId),
                error: null,
            }));
        } catch (e) {
            set({ error: errorToMessage(e) });
        }
    },
    purgeExpiredSessions: async () => {
        try {
            const purged = await invoke<string[]>("purge_expired_sessions");
            if (purged.length > 0) {
                set((state) => ({
                    sessions: state.sessions.filter(
                        (s) => !purged.includes(s.id),
                    ),
                }));
            }
            return purged;
        } catch (e) {
            // Purge is best-effort housekeeping; don't stomp error state
            // because the user didn't initiate it explicitly.
            console.warn("purge_expired_sessions failed:", e);
            return [];
        }
    },
    recoverOrphanedSessions: async () => {
        try {
            const report = await invoke<SessionRecoveryReport>(
                "recover_orphaned_sessions",
            );
            const sessions = await get().listSessions(200);
            set({
                sessions,
                error:
                    report.errors.length > 0
                        ? `Recovered ${report.recovered} session(s); ${report.errors.length} file(s) had recoverable errors.`
                        : null,
            });
            return report;
        } catch (e) {
            set({ error: errorToMessage(e) });
            return null;
        }
    },
}));
