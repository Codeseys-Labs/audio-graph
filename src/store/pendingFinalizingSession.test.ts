import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PendingFinalizingSession } from "../types";
import { useAudioGraphStore } from "./index";

/**
 * Invariant (declared at the slot in `store/index.ts`):
 * `pendingFinalizingSession` names the session whose data is resident in the
 * global store after Stop; any action that replaces or clears that data must
 * clear it. `SessionsBrowser` reads the slot (with `loadedSessionId === null`)
 * to decide whether the global-store lenses may show a row, so a stale slot
 * would present empty or foreign data as the just-stopped session's own.
 */

const PENDING: PendingFinalizingSession = {
  id: "just-stopped",
  title: null,
  created_at: 1_700_000_000_000,
  ended_at: 1_700_000_060_000,
  duration_seconds: 60,
  status: "active",
  segment_count: 1,
  speaker_count: 0,
  entity_count: 0,
  transcript_path: "",
  graph_path: "",
  deleted: false,
  deleted_at: null,
  optimistic: true,
};

function pendingId(): string | undefined {
  return useAudioGraphStore.getState().pendingFinalizingSession?.id;
}

describe("pendingFinalizingSession invariant", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    useAudioGraphStore.setState({
      pendingFinalizingSession: PENDING,
      samplePreviewActive: false,
      loadedSessionId: null,
    });
  });

  it("is cleared by resetSessionView (the Token panel's New session)", () => {
    useAudioGraphStore.getState().resetSessionView();
    expect(pendingId()).toBeUndefined();
  });

  it("is cleared when a sample preview replaces the resident data", () => {
    useAudioGraphStore.getState().loadSampleSessionPreview("en");
    expect(useAudioGraphStore.getState().samplePreviewActive).toBe(true);
    expect(pendingId()).toBeUndefined();
  });

  it("stays cleared when the sample preview is left", () => {
    useAudioGraphStore.getState().loadSampleSessionPreview("en");
    useAudioGraphStore.setState({ pendingFinalizingSession: PENDING });
    useAudioGraphStore.getState().clearTranscript();
    expect(useAudioGraphStore.getState().samplePreviewActive).toBe(false);
    expect(pendingId()).toBeUndefined();
  });

  it("is cleared by clearTranscript outside sample preview", () => {
    useAudioGraphStore.getState().clearTranscript();
    expect(pendingId()).toBeUndefined();
  });

  it("is cleared by loadSessionTranscript, which replaces the transcript without a loadedSessionId", async () => {
    vi.mocked(invoke).mockResolvedValueOnce([]);
    await useAudioGraphStore.getState().loadSessionTranscript("other");
    expect(pendingId()).toBeUndefined();
  });

  it("is kept when loadSession loads the pending session itself, and cleared for any other", async () => {
    const payload = {
      transcript: [],
      graph: {
        nodes: [],
        links: [],
        stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
      },
      transcript_events: [],
    };
    vi.mocked(invoke).mockResolvedValue(payload);
    await useAudioGraphStore.getState().loadSession("just-stopped");
    expect(pendingId()).toBe("just-stopped");

    await useAudioGraphStore.getState().loadSession("other");
    expect(pendingId()).toBeUndefined();
  });
});
