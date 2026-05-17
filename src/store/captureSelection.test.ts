import { beforeEach, describe, expect, it } from "vitest";
import { useAudioGraphStore } from "./index";

describe("capture source selection", () => {
    beforeEach(() => {
        useAudioGraphStore.setState({
            audioSources: [],
            selectedSourceIds: [],
            isCapturing: false,
            captureStartTime: null,
            error: null,
        });
    });

    it("keeps process and process-tree selections mutually exclusive", () => {
        useAudioGraphStore.getState().toggleSourceId("app:42");
        expect(useAudioGraphStore.getState().selectedSourceIds).toEqual(["app:42"]);

        useAudioGraphStore.getState().toggleSourceId("process-tree:42");
        expect(useAudioGraphStore.getState().selectedSourceIds).toEqual(["process-tree:42"]);

        useAudioGraphStore.getState().toggleSourceId("app:42");
        expect(useAudioGraphStore.getState().selectedSourceIds).toEqual(["app:42"]);
    });
});
