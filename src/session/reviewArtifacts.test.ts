import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { type ArtifactKind, useReviewStore } from "./reviewStore";
import { deferred, loadedSession } from "./reviewTestUtils";

const mockedInvoke = vi.mocked(invoke);

const NOTES = {
  notes: {
    schema_version: 1,
    session_id: "sess-a",
    last_sequence: 2,
    notes: [],
  },
  projection_events: [],
};
const TIMELINE = { entries: [], total_count: 0 };
const GRAPH = {
  schema_version: 1,
  session_id: "sess-a",
  last_sequence: 1,
  nodes: [],
  edges: [],
};

const PAYLOAD: Record<ArtifactKind, unknown> = {
  notes: NOTES,
  timeline: TIMELINE,
  graph: GRAPH,
};

const COMMAND: Record<ArtifactKind, string> = {
  notes: "load_session_notes_artifacts_cmd",
  timeline: "build_session_timeline_cmd",
  graph: "load_session_graph_artifact_cmd",
};

const KINDS = ["notes", "timeline", "graph"] as const;

function artifacts() {
  return useReviewStore.getState().artifacts;
}

async function openSession(id: string): Promise<void> {
  mockedInvoke.mockResolvedValueOnce(loadedSession("seg-1", "line", "Alice"));
  await useReviewStore.getState().select(id);
}

describe("reviewStore lens artifacts", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useReviewStore.getState().close();
  });

  it.each(
    KINDS,
  )("loads the %s artifact for the selected session only", async (kind) => {
    await openSession("sess-a");
    mockedInvoke.mockResolvedValueOnce(PAYLOAD[kind]);

    await useReviewStore.getState().loadArtifact(kind);

    expect(artifacts()[kind].status).toBe("ready");
    expect(artifacts()[kind].data).toEqual(PAYLOAD[kind]);
    const [command, args] = mockedInvoke.mock.calls.at(-1) ?? [];
    expect(command).toBe(COMMAND[kind]);
    expect(args).toMatchObject({ sessionId: "sess-a" });
    if (kind === "timeline") expect(args).toHaveProperty("limit");
    // The other lenses were not fetched.
    for (const other of KINDS.filter((k) => k !== kind)) {
      expect(artifacts()[other].status).toBe("idle");
    }
  });

  it("does nothing without a selection", async () => {
    await useReviewStore.getState().loadArtifact("notes");
    expect(mockedInvoke).not.toHaveBeenCalled();
    expect(artifacts().notes.status).toBe("idle");
  });

  it("carries the snapshot's live-lane graph next to the transcript", async () => {
    const payload = loadedSession("seg-1", "line", "Alice");
    payload.graph = {
      nodes: [],
      links: [],
      stats: { total_nodes: 7, total_edges: 0, total_episodes: 0 },
    };
    mockedInvoke.mockResolvedValueOnce(payload);
    await useReviewStore.getState().select("sess-a");
    expect(useReviewStore.getState().snapshot?.graph.stats.total_nodes).toBe(7);
  });

  it.each(
    KINDS,
  )("drops a %s reply from an earlier selection (A, B, A)", async (kind) => {
    const first = deferred<unknown>();
    const store = useReviewStore.getState();
    await openSession("sess-a");
    mockedInvoke.mockReturnValueOnce(first.promise as Promise<never>);
    const staleA = store.loadArtifact(kind); // request under the first A

    await openSession("sess-b");
    await openSession("sess-a");
    mockedInvoke.mockResolvedValueOnce(PAYLOAD[kind]);
    await store.loadArtifact(kind); // request under the second A
    const fresh = artifacts()[kind].data;
    expect(fresh).toEqual(PAYLOAD[kind]);

    // The first A's reply lands last and must not overwrite the newer one.
    first.resolve(
      kind === "timeline"
        ? { entries: [], total_count: 99 }
        : { ...(PAYLOAD[kind] as object), last_sequence: 99 },
    );
    await staleA;
    expect(artifacts()[kind].data).toBe(fresh);
  });

  it("lets the latest request for a kind win over an older one still in flight", async () => {
    await openSession("sess-a");
    const older = deferred<unknown>();
    mockedInvoke.mockReturnValueOnce(older.promise as Promise<never>);
    const olderRequest = useReviewStore.getState().loadArtifact("notes");
    mockedInvoke.mockResolvedValueOnce({ ...NOTES, notes: null });
    await useReviewStore.getState().loadArtifact("notes");

    older.resolve(NOTES);
    await olderRequest;
    expect(artifacts().notes.data).toEqual({ ...NOTES, notes: null });
  });

  it("drops a reply that lands after close()", async () => {
    await openSession("sess-a");
    const reply = deferred<unknown>();
    mockedInvoke.mockReturnValueOnce(reply.promise as Promise<never>);
    const request = useReviewStore.getState().loadArtifact("notes");
    useReviewStore.getState().close();
    reply.resolve(NOTES);
    await request;
    expect(artifacts().notes.status).toBe("idle");
    expect(artifacts().notes.data).toBeNull();
  });

  it("resets artifacts and targets when the selected session changes, and keeps them for a re-select of the same one", async () => {
    await openSession("sess-a");
    mockedInvoke.mockResolvedValueOnce(NOTES);
    await useReviewStore.getState().loadArtifact("notes");
    useReviewStore.getState().seekTranscriptToSegment("seg-1");
    useReviewStore.getState().focusGraphEdges(["e1"]);

    await openSession("sess-a"); // same session: re-query in place
    expect(artifacts().notes.status).toBe("ready");
    expect(useReviewStore.getState().transcriptSeekTarget).not.toBeNull();

    await openSession("sess-b");
    expect(artifacts().notes.status).toBe("idle");
    expect(artifacts().notes.data).toBeNull();
    expect(useReviewStore.getState().transcriptSeekTarget).toBeNull();
    expect(useReviewStore.getState().graphEdgeFocus).toBeNull();
  });

  it.each(
    KINDS,
  )("keeps the last good %s data, marked stale, when a re-query fails, and clears it on success", async (kind) => {
    await openSession("sess-a");
    mockedInvoke.mockResolvedValueOnce(PAYLOAD[kind]);
    await useReviewStore.getState().loadArtifact(kind);

    mockedInvoke.mockRejectedValueOnce(new Error("disk hiccup"));
    await useReviewStore.getState().loadArtifact(kind);
    expect(artifacts()[kind].status).toBe("ready");
    expect(artifacts()[kind].data).toEqual(PAYLOAD[kind]);
    expect(artifacts()[kind].stale).toBe(true);

    mockedInvoke.mockResolvedValueOnce(PAYLOAD[kind]);
    await useReviewStore.getState().loadArtifact(kind);
    expect(artifacts()[kind].stale).toBe(false);
  });

  it.each(
    KINDS,
  )("fails a first %s load closed, storing no backend text", async (kind) => {
    await openSession("sess-a");
    mockedInvoke.mockRejectedValueOnce({
      code: "io",
      message: "read C:\\secret\\notes.json failed",
    });
    await useReviewStore.getState().loadArtifact(kind);

    expect(artifacts()[kind].status).toBe("error");
    expect(artifacts()[kind].data).toBeNull();
    expect(JSON.stringify(useReviewStore.getState())).not.toContain("secret");
  });

  it("maps a corrupt payload to the closed error state", async () => {
    await openSession("sess-a");
    mockedInvoke.mockResolvedValueOnce({ entries: "nope" });
    await useReviewStore.getState().loadArtifact("timeline");
    expect(artifacts().timeline.status).toBe("error");
  });

  it("keeps the typed byte-ceiling refusal as numbers, not prose", async () => {
    await openSession("sess-a");
    mockedInvoke.mockRejectedValueOnce({
      code: "artifact_too_large",
      message: {
        artifact_class: "notes",
        size_bytes: 10,
        ceiling_bytes: 5,
      },
    });
    await useReviewStore.getState().loadArtifact("notes");
    expect(artifacts().notes.status).toBe("refused");
    expect(artifacts().notes.refusal).toEqual({
      artifactClass: "notes",
      sizeBytes: 10,
      ceilingBytes: 5,
    });
  });

  it("treats a session without a graph artifact as ready with no data", async () => {
    await openSession("sess-a");
    mockedInvoke.mockResolvedValueOnce(null);
    await useReviewStore.getState().loadArtifact("graph");
    expect(artifacts().graph.status).toBe("ready");
    expect(artifacts().graph.data).toBeNull();
  });

  it("bumps seek and edge-focus nonces so re-activating the same target re-fires", () => {
    const store = useReviewStore.getState();
    store.seekTranscriptToSegment("seg-1");
    store.seekTranscriptToSegment("seg-1");
    expect(useReviewStore.getState().transcriptSeekTarget).toEqual({
      segmentId: "seg-1",
      nonce: 2,
    });
    store.seekTranscriptToSegment(null);
    expect(useReviewStore.getState().transcriptSeekTarget).toBeNull();

    store.focusGraphEdges(["e1"]);
    store.focusGraphEdges(["e1"]);
    expect(useReviewStore.getState().graphEdgeFocus?.nonce).toBe(2);
    store.focusGraphEdges([]);
    expect(useReviewStore.getState().graphEdgeFocus).toBeNull();
  });
});
