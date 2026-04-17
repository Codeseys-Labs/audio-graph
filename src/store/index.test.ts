import { describe, it, expect, beforeEach } from "vitest";
import { useAudioGraphStore } from "./index";

describe("AudioGraphStore", () => {
    beforeEach(() => {
        useAudioGraphStore.setState({
            audioSources: [],
            selectedSourceIds: [],
            transcriptSegments: [],
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
});
