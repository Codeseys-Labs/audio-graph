import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useReviewStore } from "./reviewStore";
import { deferred, loadedSession } from "./reviewTestUtils";

const mockedInvoke = vi.mocked(invoke);

/** A `load_session` call the test resolves or rejects by hand. */
function deferredLoads() {
  const pending: Array<{
    sessionId: string;
    resolve: (value: unknown) => void;
    reject: (reason: unknown) => void;
  }> = [];
  mockedInvoke.mockImplementation((_cmd: string, args?: unknown) => {
    const { sessionId } = args as { sessionId: string };
    const { promise, resolve, reject } = deferred<unknown>();
    pending.push({ sessionId, resolve, reject });
    return promise;
  });
  return pending;
}

describe("reviewStore", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useReviewStore.getState().close();
  });

  it("hydrates the transcript and persisted speaker state from a load_session payload", async () => {
    mockedInvoke.mockResolvedValue(loadedSession("seg-1", "hello", "Alice"));

    await useReviewStore.getState().select("sess-a");

    const state = useReviewStore.getState();
    expect(mockedInvoke).toHaveBeenCalledWith("load_session", {
      sessionId: "sess-a",
    });
    expect(state.status).toBe("ready");
    expect(state.snapshot?.sessionId).toBe("sess-a");
    expect(state.snapshot?.transcript.map((s) => s.text)).toEqual(["hello"]);
    expect(state.snapshot?.diarizationEvents).toHaveLength(1);
    expect(state.snapshot?.diarizationEvents[0].speaker_label).toBe("Alice");
  });

  it("treats a missing diarization log as empty speaker state", async () => {
    const payload = loadedSession("seg-1", "hello", "Alice");
    delete payload.diarization_events;
    mockedInvoke.mockResolvedValue(payload);

    await useReviewStore.getState().select("sess-a");

    expect(useReviewStore.getState().snapshot?.diarizationEvents).toEqual([]);
  });

  it("rejects the older response when the selection goes A → B → A", async () => {
    const pending = deferredLoads();
    const store = useReviewStore.getState();

    const firstA = store.select("sess-a");
    const toB = store.select("sess-b");
    const secondA = store.select("sess-a");
    expect(pending.map((p) => p.sessionId)).toEqual([
      "sess-a",
      "sess-b",
      "sess-a",
    ]);

    // The newest A resolves first; then the stale A and the stale B arrive.
    pending[2].resolve(loadedSession("seg-new", "newest A", "Alice"));
    await secondA;
    pending[0].resolve(loadedSession("seg-old", "stale A", "Alice"));
    pending[1].resolve(loadedSession("seg-b", "stale B", "Bob"));
    await Promise.all([firstA, toB]);

    const state = useReviewStore.getState();
    expect(state.sessionId).toBe("sess-a");
    expect(state.snapshot?.transcript.map((s) => s.text)).toEqual(["newest A"]);
  });

  it("rejects a stale response that arrives before the newest one", async () => {
    const pending = deferredLoads();
    const store = useReviewStore.getState();

    const firstA = store.select("sess-a");
    const toB = store.select("sess-b");
    pending[0].resolve(loadedSession("seg-a", "stale A", "Alice"));
    await firstA;

    // Still loading B: the late A must not have been applied.
    expect(useReviewStore.getState().status).toBe("loading");
    expect(useReviewStore.getState().snapshot).toBeNull();

    pending[1].resolve(loadedSession("seg-b", "B", "Bob"));
    await toB;
    expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-b");
  });

  it("drops a response that lands after close()", async () => {
    const pending = deferredLoads();
    const load = useReviewStore.getState().select("sess-a");
    useReviewStore.getState().close();
    pending[0].resolve(loadedSession("seg-a", "late", "Alice"));
    await load;

    expect(useReviewStore.getState().snapshot).toBeNull();
    expect(useReviewStore.getState().status).toBe("idle");
  });

  it("maps a rejected load to a closed error code and never keeps the backend string", async () => {
    mockedInvoke.mockRejectedValue({
      code: "session_invalid",
      message: { reason: "Session files not found: C:\\secret\\path" },
    });

    await useReviewStore.getState().select("sess-missing");

    const state = useReviewStore.getState();
    expect(state.status).toBe("error");
    expect(state.error).toBe("unavailable");
    expect(state.snapshot).toBeNull();
    expect(JSON.stringify(state)).not.toContain("secret");
  });

  it("maps a corrupt payload to the same closed error", async () => {
    mockedInvoke.mockResolvedValue({ transcript: "not-an-array" });

    await useReviewStore.getState().select("sess-corrupt");

    expect(useReviewStore.getState().status).toBe("error");
    expect(useReviewStore.getState().error).toBe("unavailable");
  });

  it("marks the last good snapshot stale when a refresh fails, and clears it on the next success", async () => {
    mockedInvoke.mockResolvedValueOnce(
      loadedSession("seg-1", "first", "Alice"),
    );
    await useReviewStore.getState().select("sess-a");
    expect(useReviewStore.getState().stale).toBe(false);

    mockedInvoke.mockRejectedValueOnce(new Error("disk hiccup"));
    await useReviewStore.getState().refresh();
    expect(useReviewStore.getState().status).toBe("ready");
    expect(useReviewStore.getState().snapshot?.transcript[0].text).toBe(
      "first",
    );
    expect(useReviewStore.getState().refreshing).toBe(false);
    expect(useReviewStore.getState().stale).toBe(true);

    mockedInvoke.mockResolvedValueOnce(
      loadedSession("seg-1", "finalized", "Alice"),
    );
    await useReviewStore.getState().refresh();
    expect(useReviewStore.getState().snapshot?.transcript[0].text).toBe(
      "finalized",
    );
    expect(useReviewStore.getState().stale).toBe(false);
  });

  it("does not carry a stale mark over to a different session", async () => {
    mockedInvoke.mockResolvedValueOnce(loadedSession("a-1", "A", "Alice"));
    await useReviewStore.getState().select("sess-a");
    mockedInvoke.mockRejectedValueOnce(new Error("hiccup"));
    await useReviewStore.getState().refresh();
    expect(useReviewStore.getState().stale).toBe(true);

    mockedInvoke.mockResolvedValueOnce(loadedSession("b-1", "B", "Bob"));
    await useReviewStore.getState().select("sess-b");
    expect(useReviewStore.getState().stale).toBe(false);
    expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-b");
  });

  it("recovers from an error state via refresh()", async () => {
    mockedInvoke.mockRejectedValueOnce(new Error("missing"));
    await useReviewStore.getState().select("sess-a");
    expect(useReviewStore.getState().status).toBe("error");

    mockedInvoke.mockResolvedValueOnce(loadedSession("seg-1", "back", "Alice"));
    await useReviewStore.getState().refresh();
    expect(useReviewStore.getState().status).toBe("ready");
    expect(useReviewStore.getState().error).toBeNull();
  });
});
