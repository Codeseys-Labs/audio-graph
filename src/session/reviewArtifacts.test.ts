import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { type ArtifactKind, useReviewStore } from "./reviewStore";
import { type Deferred, deferred, loadedSession } from "./reviewTestUtils";

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

function sessionIdOf(args: unknown): string {
  if (typeof args === "object" && args !== null && "sessionId" in args) {
    return String(args.sessionId);
  }
  return "";
}

/** Every `[command, sessionId]` the store has issued so far. */
function calls(): Array<[string, string]> {
  return mockedInvoke.mock.calls.map(([cmd, args]) => [cmd, sessionIdOf(args)]);
}

function countOf(command: string): number {
  return calls().filter(([cmd]) => cmd === command).length;
}

/** A backend double answering each command from `answer`, in call order. */
function backend(
  answer: (cmd: string, id: string, nth: number) => unknown,
): void {
  const seen = new Map<string, number>();
  mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
    const nth = (seen.get(cmd) ?? 0) + 1;
    seen.set(cmd, nth);
    return answer(cmd, sessionIdOf(args), nth) as never;
  });
}

function sessionPayload(id: string, text = "line") {
  return loadedSession(`${id}-seg`, text, "Alice");
}

/** Default answers: session payload plus every artifact. */
function standard(cmd: string, id: string): unknown {
  if (cmd === "load_session") return sessionPayload(id);
  for (const kind of KINDS) if (cmd === COMMAND[kind]) return PAYLOAD[kind];
  return null;
}

function artifacts() {
  return useReviewStore.getState().artifacts;
}

describe("reviewStore rounds: snapshot and lens artifact together", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useReviewStore.getState().close();
  });

  it.each(
    KINDS,
  )("reads the snapshot and the %s artifact in one round and publishes both", async (kind) => {
    backend(standard);
    await useReviewStore.getState().select("sess-a", kind);

    expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-a");
    expect(artifacts()[kind].status).toBe("ready");
    expect(artifacts()[kind].data).toEqual(PAYLOAD[kind]);
    for (const other of KINDS.filter((k) => k !== kind)) {
      expect(artifacts()[other].status).toBe("idle");
    }
    const [, args] =
      mockedInvoke.mock.calls.find(([cmd]) => cmd === COMMAND[kind]) ?? [];
    expect(args).toMatchObject({ sessionId: "sess-a" });
    if (kind === "timeline") expect(args).toHaveProperty("limit");
    // Exactly one read of each half.
    expect(countOf("load_session")).toBe(1);
    expect(countOf(COMMAND[kind])).toBe(1);
  });

  it("fetches no artifact for the transcript lens", async () => {
    backend(standard);
    await useReviewStore.getState().select("sess-a");
    for (const kind of KINDS) expect(countOf(COMMAND[kind])).toBe(0);
  });

  it("publishes neither half until both replies have arrived (no mixed state)", async () => {
    const snapshotReply = deferred<unknown>();
    const timelineReply = deferred<unknown>();
    backend((cmd, id) => {
      if (cmd === "load_session") return snapshotReply.promise;
      if (cmd === COMMAND.timeline) return timelineReply.promise;
      return standard(cmd, id);
    });
    const round = useReviewStore.getState().select("sess-a", "timeline");

    timelineReply.resolve(TIMELINE);
    await Promise.resolve();
    await Promise.resolve();
    // The artifact arrived first: nothing is published yet.
    expect(useReviewStore.getState().snapshot).toBeNull();
    expect(artifacts().timeline.data).toBeNull();

    snapshotReply.resolve(sessionPayload("sess-a"));
    await round;
    expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-a");
    expect(artifacts().timeline.data).toEqual(TIMELINE);
  });

  it("activating another lens re-reads the snapshot with it and resets the old lens's slot", async () => {
    backend(standard);
    await useReviewStore.getState().select("sess-a", "notes");
    expect(artifacts().notes.status).toBe("ready");

    await useReviewStore.getState().activateLens("timeline");

    expect(artifacts().timeline.status).toBe("ready");
    // The notes read belongs to the previous snapshot: dropped, not shown.
    expect(artifacts().notes.status).toBe("idle");
    expect(artifacts().notes.data).toBeNull();
    expect(countOf("load_session")).toBe(2);
  });

  it("activating the transcript lens starts no round", async () => {
    backend(standard);
    await useReviewStore.getState().select("sess-a", "notes");
    mockedInvoke.mockClear();
    await useReviewStore.getState().activateLens(null);
    expect(mockedInvoke).not.toHaveBeenCalled();
  });

  it("keeps the previous pair, stale, when the artifact half of a refresh fails (the new snapshot is not published)", async () => {
    backend((cmd, id, nth) => {
      if (cmd === "load_session") {
        return sessionPayload(id, nth === 1 ? "first" : "newer");
      }
      if (cmd === COMMAND.notes && nth > 1) throw new Error("disk hiccup");
      return standard(cmd, id);
    });
    await useReviewStore.getState().select("sess-a", "notes");
    await useReviewStore.getState().refresh();

    const state = useReviewStore.getState();
    expect(state.snapshot?.transcript[0].text).toBe("first");
    expect(artifacts().notes.data).toEqual(NOTES);
    expect(state.stale).toBe(true);
    expect(artifacts().notes.stale).toBe(true);
    expect(state.refreshing).toBe(false);
  });

  it("keeps the previous pair, stale, when the snapshot half of a refresh fails (the new artifact is not published)", async () => {
    backend((cmd, id, nth) => {
      if (cmd === "load_session") {
        if (nth > 1) throw new Error("disk hiccup");
        return sessionPayload(id, "first");
      }
      if (cmd === COMMAND.notes && nth > 1) {
        return { ...NOTES, notes: { ...NOTES.notes, last_sequence: 99 } };
      }
      return standard(cmd, id);
    });
    await useReviewStore.getState().select("sess-a", "notes");
    await useReviewStore.getState().refresh();

    expect(artifacts().notes.data).toEqual(NOTES);
    expect(useReviewStore.getState().stale).toBe(true);
    expect(useReviewStore.getState().snapshot?.transcript[0].text).toBe(
      "first",
    );
  });

  it("clears stale only when a later round succeeds in both halves", async () => {
    let failing = true;
    backend((cmd, id, nth) => {
      if (nth > 1 && failing && cmd === COMMAND.notes)
        throw new Error("hiccup");
      return standard(cmd, id);
    });
    await useReviewStore.getState().select("sess-a", "notes");
    await useReviewStore.getState().refresh();
    expect(useReviewStore.getState().stale).toBe(true);

    failing = false;
    await useReviewStore.getState().refresh();
    expect(useReviewStore.getState().stale).toBe(false);
    expect(artifacts().notes.stale).toBe(false);
  });

  it("publishes the snapshot alone, with the lens closed as unavailable, when only the first artifact read fails", async () => {
    backend((cmd, id) => {
      if (cmd === COMMAND.notes) throw { code: "io", message: "C:\\secret" };
      return standard(cmd, id);
    });
    await useReviewStore.getState().select("sess-a", "notes");

    expect(useReviewStore.getState().status).toBe("ready");
    expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-a");
    expect(artifacts().notes.status).toBe("error");
    expect(JSON.stringify(useReviewStore.getState())).not.toContain("secret");
  });

  it("discards a successful artifact when a first snapshot read fails", async () => {
    backend((cmd, id) => {
      if (cmd === "load_session") throw new Error("missing");
      return standard(cmd, id);
    });
    await useReviewStore.getState().select("sess-a", "timeline");

    expect(useReviewStore.getState().status).toBe("error");
    expect(useReviewStore.getState().snapshot).toBeNull();
    expect(artifacts().timeline.data).toBeNull();
    expect(artifacts().timeline.status).toBe("error");
  });

  it("carries the snapshot's live-lane graph next to the transcript", async () => {
    backend((cmd, id) => {
      if (cmd === "load_session") {
        return {
          ...sessionPayload(id),
          graph: {
            nodes: [],
            links: [],
            stats: { total_nodes: 7, total_edges: 0, total_episodes: 0 },
          },
        };
      }
      return standard(cmd, id);
    });
    await useReviewStore.getState().select("sess-a");
    expect(useReviewStore.getState().snapshot?.graph.stats.total_nodes).toBe(7);
  });

  it.each(
    KINDS,
  )("drops every %s reply from an earlier selection (A, B, A)", async (kind) => {
    const replies: Array<Deferred<unknown>> = [];
    backend((cmd, id) => {
      if (cmd === COMMAND[kind]) {
        const reply = deferred<unknown>();
        replies.push(reply);
        return reply.promise;
      }
      return standard(cmd, id);
    });
    const store = useReviewStore.getState();
    const firstA = store.select("sess-a", kind);
    const toB = store.select("sess-b", kind);
    const secondA = store.select("sess-a", kind);

    replies[2].resolve(PAYLOAD[kind]);
    await secondA;
    const fresh = artifacts()[kind].data;
    expect(fresh).toEqual(PAYLOAD[kind]);

    // The first A's and B's replies land last and must not overwrite it.
    replies[0].resolve(
      kind === "timeline"
        ? { entries: [], total_count: 99 }
        : { ...(PAYLOAD[kind] as object), last_sequence: 99 },
    );
    replies[1].resolve(PAYLOAD[kind]);
    await Promise.all([firstA, toB]);
    expect(artifacts()[kind].data).toBe(fresh);
    expect(useReviewStore.getState().sessionId).toBe("sess-a");
  });

  it("drops a round that lands after close()", async () => {
    const reply = deferred<unknown>();
    backend((cmd, id) =>
      cmd === COMMAND.notes ? reply.promise : standard(cmd, id),
    );
    const round = useReviewStore.getState().select("sess-a", "notes");
    useReviewStore.getState().close();
    reply.resolve(NOTES);
    await round;
    expect(artifacts().notes.status).toBe("idle");
    expect(useReviewStore.getState().snapshot).toBeNull();
  });

  describe("overlapping refreshes", () => {
    it("never run two rounds at once and queue at most one trailing round", async () => {
      const gate = deferred<unknown>();
      let inFlightNow = 0;
      let maxInFlight = 0;
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          inFlightNow += 1;
          maxInFlight = Math.max(maxInFlight, inFlightNow);
          const answer =
            nth === 1 ? gate.promise : Promise.resolve(sessionPayload(id));
          return answer.finally(() => {
            inFlightNow -= 1;
          });
        }
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      const opening = store.select("sess-a", "notes");
      // Focus, visibility and a quick lens re-activation all land while the
      // first round is still in flight.
      void store.refresh();
      void store.refresh();
      void store.activateLens("notes");
      void store.refresh();

      gate.resolve(sessionPayload("sess-a"));
      await opening;

      // The opening round, plus exactly one trailing round for all four requests.
      expect(countOf("load_session")).toBe(2);
      expect(countOf(COMMAND.notes)).toBe(2);
      expect(maxInFlight).toBe(1);
    });

    it("lets a new selection start its own round while an older selection's is pending", async () => {
      const gate = deferred<unknown>();
      backend((cmd, id) =>
        cmd === "load_session" && id === "sess-a"
          ? gate.promise
          : standard(cmd, id),
      );
      const store = useReviewStore.getState();
      const a = store.select("sess-a");
      await store.select("sess-b");
      expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-b");
      gate.resolve(sessionPayload("sess-a"));
      await a;
      expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-b");
    });
  });

  it("resets artifacts and targets when the selected session changes, and keeps them for a re-select of the same one", async () => {
    backend(standard);
    await useReviewStore.getState().select("sess-a", "notes");
    useReviewStore.getState().seekTranscriptToSegment("seg-1");
    useReviewStore.getState().focusGraphEdges(["e1"]);

    await useReviewStore.getState().select("sess-a", "notes"); // same session
    expect(useReviewStore.getState().transcriptSeekTarget).not.toBeNull();

    await useReviewStore.getState().select("sess-b", "notes");
    expect(useReviewStore.getState().transcriptSeekTarget).toBeNull();
    expect(useReviewStore.getState().graphEdgeFocus).toBeNull();
  });

  it("maps a corrupt artifact payload to the closed error state", async () => {
    backend((cmd, id) =>
      cmd === COMMAND.timeline ? { entries: "nope" } : standard(cmd, id),
    );
    await useReviewStore.getState().select("sess-a", "timeline");
    expect(artifacts().timeline.status).toBe("error");
    // The snapshot half stands alone.
    expect(useReviewStore.getState().status).toBe("ready");
  });

  it("treats a session without a graph artifact as ready with no data", async () => {
    backend((cmd, id) => (cmd === COMMAND.graph ? null : standard(cmd, id)));
    await useReviewStore.getState().select("sess-a", "graph");
    expect(artifacts().graph.status).toBe("ready");
    expect(artifacts().graph.data).toBeNull();
  });

  describe("the typed byte-ceiling refusal", () => {
    function refusal(message: unknown) {
      return { code: "artifact_too_large", message };
    }

    it("is kept as numbers and a closed class, not prose", async () => {
      backend((cmd, id) => {
        if (cmd === COMMAND.notes) {
          throw refusal({
            artifact_class: "materialized_notes",
            size_bytes: 10,
            ceiling_bytes: 5,
          });
        }
        return standard(cmd, id);
      });
      await useReviewStore.getState().select("sess-a", "notes");
      expect(artifacts().notes.status).toBe("refused");
      expect(artifacts().notes.refusal).toEqual({
        artifactClass: "materialized_notes",
        sizeBytes: 10,
        ceilingBytes: 5,
      });
    });

    it.each([
      ["no message at all", undefined],
      ["a null message", null],
      ["a string message", "too large"],
      [
        "an unexpected artifact class",
        { artifact_class: "../../etc/passwd", size_bytes: 1, ceiling_bytes: 1 },
      ],
      ["a missing artifact class", { size_bytes: 1, ceiling_bytes: 1 }],
      [
        "a negative size",
        { artifact_class: "notes", size_bytes: -1, ceiling_bytes: 1 },
      ],
      [
        "a non-finite size",
        {
          artifact_class: "materialized_notes",
          size_bytes: Number.POSITIVE_INFINITY,
          ceiling_bytes: 1,
        },
      ],
      [
        "NaN ceiling",
        {
          artifact_class: "materialized_notes",
          size_bytes: 1,
          ceiling_bytes: Number.NaN,
        },
      ],
      [
        "numbers sent as strings",
        {
          artifact_class: "materialized_notes",
          size_bytes: "10",
          ceiling_bytes: "5",
        },
      ],
    ])("falls back to the closed error state for %s, and never leaves the lens loading", async (_name, message) => {
      backend((cmd, id) => {
        if (cmd === COMMAND.notes) throw refusal(message);
        return standard(cmd, id);
      });
      await useReviewStore.getState().select("sess-a", "notes");
      expect(artifacts().notes.status).toBe("error");
      expect(artifacts().notes.refusal).toBeNull();
    });
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

  describe("seekToSpan", () => {
    function eventFor(spanId: string, segmentId: string) {
      return {
        span_id: spanId,
        provider: "deepgram",
        source_id: "system-default",
        transcript_segment_id: segmentId,
        text: "t",
        start_time: 0,
        end_time: 1,
        confidence: 1,
        is_final: true,
        stability: "final",
        revision_number: 1,
        end_of_turn: true,
        received_at_ms: 1,
      };
    }

    it("seeks straight away to a span the caller already resolved", async () => {
      backend(standard);
      await useReviewStore.getState().select("sess-a", "timeline");
      mockedInvoke.mockClear();
      await useReviewStore.getState().seekToSpan("span-1", "seg-1");
      expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
        "seg-1",
      );
      expect(mockedInvoke).not.toHaveBeenCalled();
    });

    it("refreshes first when the snapshot does not know the span, then seeks to the right segment", async () => {
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          const payload = sessionPayload(id);
          return nth === 1
            ? { ...payload, transcript_events: [eventFor("span-1", "seg-1")] }
            : {
                ...payload,
                transcript_events: [
                  eventFor("span-1", "seg-1"),
                  eventFor("span-2", "seg-2"),
                ],
              };
        }
        return standard(cmd, id);
      });
      await useReviewStore.getState().select("sess-a", "timeline");
      expect(countOf("load_session")).toBe(1);

      await useReviewStore.getState().seekToSpan("span-2", null);

      expect(countOf("load_session")).toBe(2);
      expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
        "seg-2",
      );
    });

    it("does not seek when the selection changed while it refreshed", async () => {
      const gate = deferred<unknown>();
      backend((cmd, id, nth) =>
        cmd === "load_session" && nth === 2 ? gate.promise : standard(cmd, id),
      );
      await useReviewStore.getState().select("sess-a", "timeline");
      const seeking = useReviewStore.getState().seekToSpan("span-2", null);
      await useReviewStore.getState().select("sess-b", "timeline");
      gate.resolve(sessionPayload("sess-a"));
      await seeking;
      expect(useReviewStore.getState().transcriptSeekTarget).toBeNull();
    });
  });
});
