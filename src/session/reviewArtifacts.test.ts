import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { type ArtifactKind, useReviewStore } from "./reviewStore";
import {
  type Deferred,
  deferred,
  loadedSession,
  segment,
} from "./reviewTestUtils";

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

  describe("a round belongs to the lens that was open when it started", () => {
    it("lets a switch to Transcript win over a Notes refresh whose snapshot succeeds but whose Notes read fails", async () => {
      const staleSnapshot = deferred<unknown>();
      const staleNotes = deferred<unknown>();
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          if (nth === 1) return sessionPayload(id, "first");
          if (nth === 2) return staleSnapshot.promise; // the abandoned refresh
          return sessionPayload(id, "fresh"); // the Transcript round
        }
        if (cmd === COMMAND.notes) {
          return nth === 1 ? NOTES : staleNotes.promise;
        }
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      await store.select("sess-a", "notes");

      const abandoned = store.refresh(); // focus refresh with Notes open
      await store.activateLens(null); // the user switches to Transcript
      expect(useReviewStore.getState().snapshot?.transcript[0].text).toBe(
        "fresh",
      );

      // The abandoned round's replies land now: snapshot ok, Notes fails.
      staleSnapshot.resolve(
        sessionPayload("sess-a", "from the abandoned round"),
      );
      staleNotes.reject(new Error("hiccup"));
      await abandoned;

      const state = useReviewStore.getState();
      expect(state.snapshot?.transcript[0].text).toBe("fresh");
      expect(state.stale).toBe(false);
      expect(state.refreshing).toBe(false);
    });

    it("is not blocked by a Notes read that never settles: Transcript refreshes, and Notes retries", async () => {
      let notesCalls = 0;
      backend((cmd, id, nth) => {
        if (cmd === "load_session") return sessionPayload(id, `read ${nth}`);
        if (cmd === COMMAND.notes) {
          notesCalls += 1;
          if (notesCalls === 2) return new Promise(() => {}); // hangs forever
          return NOTES;
        }
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      await store.select("sess-a", "notes");

      void store.refresh(); // Notes round that will never settle
      await Promise.resolve();

      // Switching to Transcript supersedes it and completes on its own.
      await store.activateLens(null);
      expect(useReviewStore.getState().snapshot?.transcript[0].text).toBe(
        "read 3",
      );
      // A later passive refresh on the Transcript lens is not blocked either.
      await store.refresh();
      expect(useReviewStore.getState().snapshot?.transcript[0].text).toBe(
        "read 4",
      );

      // And Notes itself can be retried while an older Notes read is hung.
      notesCalls = 1; // the next Notes read hangs again
      void store.activateLens("notes");
      await Promise.resolve();
      await store.refresh({ explicit: true }); // a retry click
      expect(artifacts().notes.status).toBe("ready");
    });

    it("starts a snapshot-only round when switching to Transcript right after a Notes refresh failed, and only then", async () => {
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          return sessionPayload(id, ["first", "refreshed", "fresh"][nth - 1]);
        }
        if (cmd === COMMAND.notes && nth === 2) throw new Error("hiccup");
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      await store.select("sess-a", "notes");
      await store.refresh(); // snapshot succeeds, Notes fails: nothing published
      expect(useReviewStore.getState().stale).toBe(true);
      expect(useReviewStore.getState().snapshot?.transcript[0].text).toBe(
        "first",
      );
      const loadsBefore = countOf("load_session");

      await store.activateLens(null); // no round in flight, Transcript is stale

      expect(countOf("load_session") - loadsBefore).toBe(1);
      expect(countOf(COMMAND.notes)).toBe(2); // the Transcript round reads no artifact
      expect(useReviewStore.getState().snapshot?.transcript[0].text).toBe(
        "fresh",
      );
      expect(useReviewStore.getState().stale).toBe(false);
    });

    it("starts nothing when switching to a lens whose data is fresh", async () => {
      backend(standard);
      const store = useReviewStore.getState();
      await store.select("sess-a", "notes");
      await store.activateLens(null);
      await store.activateLens("notes");
      expect(countOf("load_session")).toBe(1);
      expect(countOf(COMMAND.notes)).toBe(1);
    });

    it("hands an abandoned lens's slot back to its settled state when its round is replaced and fails", async () => {
      backend((cmd, id, nth) => {
        if (cmd === "load_session" && nth >= 3) throw new Error("hiccup");
        if (cmd === COMMAND.notes) return new Promise(() => {}); // hangs
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      await store.select("sess-a"); // Transcript: a snapshot only
      void store.activateLens("notes"); // first Notes read: slot is loading
      await Promise.resolve();
      expect(artifacts().notes.status).toBe("loading");

      await store.activateLens(null); // replaced by a Transcript round that fails

      expect(artifacts().notes.status).toBe("idle"); // not left "loading"
      expect(useReviewStore.getState().stale).toBe(true);
    });

    it("keeps passive triggers coalescing: focus does not supersede a round in flight", async () => {
      const gate = deferred<unknown>();
      let loads = 0;
      backend((cmd, id) => {
        if (cmd === "load_session") {
          loads += 1;
          return loads === 1 ? gate.promise : sessionPayload(id);
        }
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      const opening = store.select("sess-a", "notes");
      void store.refresh();
      void store.refresh();
      expect(loads).toBe(1);
      gate.resolve(sessionPayload("sess-a"));
      await opening;
      expect(loads).toBe(2);
    });
  });

  describe("a lens that fails must not discard the last good pair", () => {
    it("keeps the previous snapshot with its Notes when the new snapshot succeeds but the Timeline read fails", async () => {
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          return sessionPayload(id, nth === 1 ? "first" : "second");
        }
        if (cmd === COMMAND.timeline)
          throw { code: "io", message: "disk gone" };
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      await store.select("sess-a", "notes");
      await store.activateLens("timeline");

      const state = useReviewStore.getState();
      // The pair was NOT replaced: the old snapshot, still with its Notes.
      expect(state.snapshot?.transcript[0].text).toBe("first");
      expect(artifacts().notes.status).toBe("ready");
      expect(artifacts().notes.data).toEqual(NOTES);
      expect(state.stale).toBe(true);
      expect(artifacts().notes.stale).toBe(true);
      // The failure is recorded on the lens that failed, which can retry.
      expect(artifacts().timeline.status).toBe("error");
      expect(artifacts().timeline.data).toBeNull();
    });

    it("still shows the last good Notes when returning to them while every read keeps failing", async () => {
      let failing = false;
      backend((cmd, id) => {
        if (
          failing &&
          (cmd === "load_session" ||
            cmd === COMMAND.timeline ||
            cmd === COMMAND.notes)
        ) {
          throw { code: "io", message: "disk gone" };
        }
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      await store.select("sess-a", "notes");
      failing = true;
      await store.activateLens("timeline");
      await store.activateLens("notes");

      expect(artifacts().notes.status).toBe("ready");
      expect(artifacts().notes.data).toEqual(NOTES);
      expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-a");
    });

    it("keeps the pair too when the Timeline read succeeds but the snapshot fails, and replaces it only when both succeed", async () => {
      let snapshotFails = true;
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          if (nth > 1 && snapshotFails) throw new Error("hiccup");
          return sessionPayload(id, nth === 1 ? "first" : "second");
        }
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      await store.select("sess-a", "notes");
      await store.activateLens("timeline");
      // The fold arrived but could not be shown next to a missing snapshot.
      expect(useReviewStore.getState().snapshot?.transcript[0].text).toBe(
        "first",
      );
      expect(artifacts().notes.data).toEqual(NOTES);
      expect(artifacts().timeline.data).toBeNull();
      expect(artifacts().timeline.status).toBe("error");

      snapshotFails = false;
      await store.refresh({ explicit: true }); // retry on the Timeline lens
      expect(useReviewStore.getState().snapshot?.transcript[0].text).toBe(
        "second",
      );
      expect(artifacts().timeline.status).toBe("ready");
      expect(artifacts().notes.status).toBe("idle"); // the old pair was replaced
      expect(useReviewStore.getState().stale).toBe(false);
    });

    it("lets a snapshot stand alone when there was no previous pair at all", async () => {
      backend((cmd, id) => {
        if (cmd === COMMAND.timeline)
          throw { code: "io", message: "disk gone" };
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      await store.select("sess-a"); // Transcript lens: a snapshot, no artifact
      await store.activateLens("timeline");
      expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-a");
      expect(artifacts().timeline.status).toBe("error");
    });
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
      // Passive triggers (window focus, tab visibility) all land while the
      // first round is still in flight.
      void store.refresh();
      void store.refresh();
      void store.refresh();

      gate.resolve(sessionPayload("sess-a"));
      await opening;

      // The opening round, plus exactly one trailing round for all three requests.
      expect(countOf("load_session")).toBe(2);
      expect(countOf(COMMAND.notes)).toBe(2);
      expect(maxInFlight).toBe(1);
    });

    it("fetches a lens that registered while a round was in flight", async () => {
      const gate = deferred<unknown>();
      let loads = 0;
      backend((cmd, id) => {
        if (cmd === "load_session") {
          loads += 1;
          return loads === 1 ? gate.promise : sessionPayload(id);
        }
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      const opening = store.select("sess-a", "notes");
      // Timeline registers while the Notes round is still pending.
      void store.activateLens("timeline");
      gate.resolve(sessionPayload("sess-a"));
      await opening;

      expect(artifacts().timeline.status).toBe("ready");
      expect(artifacts().timeline.data).toEqual(TIMELINE);
    });

    it("fetches a lens that registered while the trailing round was already running", async () => {
      const first = deferred<unknown>();
      const second = deferred<unknown>();
      let loads = 0;
      backend((cmd, id) => {
        if (cmd === "load_session") {
          loads += 1;
          if (loads === 1) return first.promise;
          if (loads === 2) return second.promise;
          return sessionPayload(id);
        }
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      const opening = store.select("sess-a", "notes");
      void store.refresh(); // queues the trailing round
      first.resolve(sessionPayload("sess-a"));
      await vi.waitFor(() => expect(loads).toBe(2)); // trailing round started
      void store.activateLens("graph"); // too late for it: it picked "notes"
      second.resolve(sessionPayload("sess-a"));
      await opening;

      expect(artifacts().graph.status).toBe("ready");
      expect(artifacts().graph.data).toEqual(GRAPH);
    });

    it("is not wedged by a synchronous throw from invoke: a later refresh still runs", async () => {
      mockedInvoke.mockImplementationOnce(() => {
        throw new Error("bridge down");
      });
      await useReviewStore.getState().select("sess-a");
      expect(useReviewStore.getState().status).toBe("error");

      backend(standard);
      await useReviewStore.getState().refresh();
      expect(useReviewStore.getState().status).toBe("ready");
      expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-a");
    });

    it("is not wedged by a round dropped as stale: the next selection and refresh both run", async () => {
      const gate = deferred<unknown>();
      backend((cmd, id) =>
        cmd === "load_session" && id === "sess-a"
          ? gate.promise
          : standard(cmd, id),
      );
      const store = useReviewStore.getState();
      const dropped = store.select("sess-a", "notes");
      store.close(); // epoch bump drops the pending round
      gate.resolve(sessionPayload("sess-a"));
      await dropped;

      await store.select("sess-b", "notes");
      await store.refresh();
      expect(useReviewStore.getState().snapshot?.sessionId).toBe("sess-b");
      expect(artifacts().notes.status).toBe("ready");
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
        {
          artifact_class: "materialized_notes",
          size_bytes: -1,
          ceiling_bytes: 1,
        },
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
        "a size sent as a string",
        {
          artifact_class: "materialized_notes",
          size_bytes: "10",
          ceiling_bytes: 5,
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
            ? {
                ...payload,
                transcript: [segment("seg-1", "one")],
                transcript_events: [eventFor("span-1", "seg-1")],
              }
            : {
                ...payload,
                transcript: [segment("seg-1", "one"), segment("seg-2", "two")],
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

    function snapshotWithSpans(id: string, spans: string[]) {
      return {
        ...sessionPayload(id),
        // The transcript holds the segment each span's latest revision names.
        transcript: spans.map((span) =>
          segment(span.replace("span", "seg"), `text of ${span}`),
        ),
        transcript_events: spans.map((span) =>
          eventFor(span, span.replace("span", "seg")),
        ),
      };
    }

    it("lets a newer known-block click win over an older click still waiting on a refresh", async () => {
      const refresh = deferred<unknown>();
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          return nth === 1
            ? snapshotWithSpans(id, ["span-1"])
            : refresh.promise;
        }
        return standard(cmd, id);
      });
      await useReviewStore.getState().select("sess-a", "timeline");

      const older = useReviewStore.getState().seekToSpan("span-2", null); // needs a refresh
      await useReviewStore.getState().seekToSpan("span-1", "seg-1"); // newer, known
      expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
        "seg-1",
      );

      refresh.resolve(snapshotWithSpans("sess-a", ["span-1", "span-2"]));
      await older;
      // The first click's refresh resolved last and must not overwrite.
      expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
        "seg-1",
      );
    });

    it.each([
      ["the older refresh resolving first", true],
      ["the newer refresh resolving first", false],
    ])("applies only the latest of two deferred clicks, with %s", async (_name, olderFirst) => {
      const replies: Array<Deferred<unknown>> = [];
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          if (nth === 1) return snapshotWithSpans(id, ["span-1"]);
          const reply = deferred<unknown>();
          replies.push(reply);
          return reply.promise;
        }
        return standard(cmd, id);
      });
      await useReviewStore.getState().select("sess-a", "timeline");

      const first = useReviewStore.getState().seekToSpan("span-2", null);
      const second = useReviewStore.getState().seekToSpan("span-3", null);
      expect(replies).toHaveLength(2); // the second click superseded the first round

      const fresh = snapshotWithSpans("sess-a", ["span-1", "span-2", "span-3"]);
      if (olderFirst) {
        replies[0].resolve(fresh);
        replies[1].resolve(fresh);
      } else {
        replies[1].resolve(fresh);
        replies[0].resolve(fresh);
      }
      await Promise.all([first, second]);

      expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
        "seg-3",
      );
    });

    it("leaves the target alone, never a raw span id, when the refresh cannot resolve the span, and resolves it on a later successful refresh", async () => {
      let timelineFails = false;
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          return nth === 1
            ? snapshotWithSpans(id, ["span-1"])
            : snapshotWithSpans(id, ["span-1", "span-2"]);
        }
        if (cmd === COMMAND.timeline && timelineFails) {
          throw { code: "io", message: "disk gone" };
        }
        return standard(cmd, id);
      });
      await useReviewStore.getState().select("sess-a", "timeline");

      // The refresh's lens read fails, so the old pair (without span-2) stays.
      timelineFails = true;
      await useReviewStore.getState().seekToSpan("span-2", null);
      expect(useReviewStore.getState().transcriptSeekTarget).toBeNull();

      // The next successful refresh for the same selection replays that seek.
      timelineFails = false;
      await useReviewStore.getState().refresh();
      expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
        "seg-2",
      );
    });

    it("does not replay a pending seek once a newer seek replaced it, or the selection changed", async () => {
      let timelineFails = false;
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          return nth === 1
            ? snapshotWithSpans(id, ["span-1"])
            : snapshotWithSpans(id, ["span-1", "span-2"]);
        }
        if (cmd === COMMAND.timeline && timelineFails) {
          throw { code: "io", message: "disk gone" };
        }
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      await store.select("sess-a", "timeline");

      timelineFails = true;
      await store.seekToSpan("span-2", null); // pending
      await store.seekToSpan("span-1", "seg-1"); // newer, known: replaces it
      timelineFails = false;
      await store.refresh();
      expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
        "seg-1",
      );

      // A pending seek does not survive a selection change either.
      timelineFails = true;
      await store.seekToSpan("span-2", null);
      timelineFails = false;
      store.seekTranscriptToSegment(null);
      await store.select("sess-b", "timeline");
      expect(useReviewStore.getState().transcriptSeekTarget).toBeNull();
    });

    it("resolves a span by its LATEST revision, and never to a segment the transcript lacks", async () => {
      const partial = {
        ...eventFor("span-2", "ignored"),
        transcript_segment_id: null,
        is_final: false,
        stability: "partial",
        revision_number: 1,
      };
      const final = {
        ...eventFor("span-2", "seg-2"),
        revision_number: 2,
      };
      const orphan = {
        ...eventFor("span-3", "seg-9"), // names a segment the transcript lacks
        revision_number: 1,
      };
      backend((cmd, id, nth) => {
        if (cmd === "load_session") {
          if (nth === 1) return snapshotWithSpans(id, ["span-1"]);
          const payload = snapshotWithSpans(id, ["span-1"]);
          return {
            ...payload,
            transcript: [segment("seg-1", "one"), segment("seg-2", "two")],
            // The revision history: the partial (no segment id) comes first.
            transcript_events: [
              ...payload.transcript_events,
              partial,
              final,
              orphan,
            ],
          };
        }
        return standard(cmd, id);
      });
      const store = useReviewStore.getState();
      await store.select("sess-a", "timeline");

      await store.seekToSpan("span-2", null);
      expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
        "seg-2",
      );

      // A span whose latest revision names a segment that does not exist is not
      // a target: the seek stays pending and the target is unchanged.
      await store.seekToSpan("span-3", null);
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
