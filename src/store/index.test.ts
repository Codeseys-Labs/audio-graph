import { describe, it, expect, beforeEach, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
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
        expect(useAudioGraphStore.getState().selectedSourceIds).not.toContain("mic-1");
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
});
