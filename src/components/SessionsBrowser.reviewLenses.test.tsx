import { invoke } from "@tauri-apps/api/core";
import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { Profiler } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../i18n";
import { useReviewStore } from "../session/reviewStore";
import { deferred, segment } from "../session/reviewTestUtils";
import { useAudioGraphStore } from "../store";
import type {
  GraphNode,
  GraphSnapshot,
  MaterializedGraph,
  MaterializedNote,
  SessionMetadata,
  SessionNotesArtifacts,
  SessionTimelineFold,
} from "../types";
import SessionsBrowser from "./SessionsBrowser";

// react-force-graph-2d draws to a canvas jsdom cannot drive.
vi.mock("react-force-graph-2d", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  return {
    default: React.forwardRef((_props: unknown, ref: React.Ref<unknown>) => {
      React.useImperativeHandle(ref, () => ({
        zoomToFit: () => {},
        d3ReheatSimulation: () => {},
        d3Force: () => ({ strength: () => {}, distance: () => {} }),
      }));
      return React.createElement("div", { "data-testid": "force-graph" });
    }),
  };
});

const mockedInvoke = vi.mocked(invoke);

/**
 * Slice 2 of #140: while capture runs, a completed session's Notes, Timeline
 * and Graph lenses (like its Transcript lens since slice 1) render from Review
 * data, never from the global store.
 */

type Lens = "notes" | "timeline" | "graph";

const COMMAND: Record<Lens, string> = {
  notes: "load_session_notes_artifacts_cmd",
  timeline: "build_session_timeline_cmd",
  graph: "load_session_graph_artifact_cmd",
};

function metadata(id: string, title: string): SessionMetadata {
  return {
    id,
    title,
    created_at: 1_700_000_000_000,
    ended_at: 1_700_000_060_000,
    duration_seconds: 60,
    status: "complete",
    segment_count: 1,
    speaker_count: 0,
    entity_count: 0,
    transcript_path: "",
    graph_path: "",
    deleted: false,
    deleted_at: null,
  };
}

const SESSION_A = metadata("session-a", "Session A");
const SESSION_B = metadata("session-b", "Session B");

const NODES: Record<string, number> = { "session-a": 2, "session-b": 3 };

function graphSnapshot(id: string): GraphSnapshot {
  const count = NODES[id] ?? 1;
  const nodes: GraphNode[] = Array.from({ length: count }, (_, i) => ({
    id: `${id}-n${i}`,
    name: `${id} node ${i}`,
    entity_type: "Topic",
    val: 1,
    color: "#60a5fa",
    first_seen: 0,
    last_seen: 0,
    mention_count: 1,
  }));
  return {
    nodes,
    links: [],
    stats: { total_nodes: count, total_edges: 0, total_episodes: 0 },
  };
}

function loadSessionPayload(id: string) {
  return {
    transcript: [segment(`${id}-1`, `transcript of ${id}`)],
    graph: graphSnapshot(id),
    transcript_events: [],
    diarization_events: [],
  };
}

function noteFor(id: string, title: string): MaterializedNote {
  return {
    id: `${id}-note`,
    title,
    body: `body of ${title}`,
    tags: [],
    updated_by_sequence: 1,
    updated_at_ms: 1,
    basis: null,
    provenance: null,
    valid_from_ms: 0,
  };
}

function notesPayload(id: string, title = `Note of ${id}`) {
  const payload: SessionNotesArtifacts = {
    notes: {
      schema_version: 1,
      session_id: id,
      last_sequence: 4,
      notes: [noteFor(id, title)],
    },
    projection_events: [],
  };
  return payload;
}

function timelinePayload(id: string): SessionTimelineFold {
  return {
    entries: [
      {
        span_id: `span-${id}`,
        start_ms: 0,
        end_ms: 1000,
        received_at_ms: 1,
        speaker_id: null,
        speaker_label: null,
        text: `said in ${id}`,
        related_edge_ids: [],
      },
    ],
    total_count: 1,
  };
}

function graphArtifactPayload(id: string): MaterializedGraph {
  return {
    schema_version: 1,
    session_id: id,
    last_sequence: 3,
    nodes: Array.from({ length: NODES[id] ?? 1 }, (_, i) => ({
      id: `${id}-m${i}`,
      name: `${id} materialized ${i}`,
      entity_type: "Topic",
      confidence: 1,
      valid_from_ms: 0,
      updated_by_sequence: 1,
      updated_at_ms: 1,
      basis: null,
      provenance: null,
    })),
    edges: [],
  };
}

function artifactPayload(kind: Lens, id: string): unknown {
  if (kind === "notes") return notesPayload(id);
  if (kind === "timeline") return timelinePayload(id);
  return graphArtifactPayload(id);
}

function sessionIdOf(args: unknown): string {
  if (typeof args === "object" && args !== null && "sessionId" in args) {
    return String(args.sessionId);
  }
  throw new Error("command called without a sessionId");
}

/** Per-test backend: every lens query is answered by `respond`. */
function mockBackend(
  respond: (cmd: string, id: string) => unknown | Promise<unknown>,
): void {
  mockedInvoke.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === "list_sessions") return [SESSION_A, SESSION_B];
    if (cmd === "purge_expired_sessions") return [];
    return respond(cmd, args === undefined ? "" : sessionIdOf(args));
  });
}

function defaultRespond(cmd: string, id: string): unknown {
  if (cmd === "load_session") return loadSessionPayload(id);
  for (const kind of ["notes", "timeline", "graph"] as const) {
    if (cmd === COMMAND[kind]) return artifactPayload(kind, id);
  }
  return null;
}

/** Live session: capture running, its own data resident in the global store. */
const LIVE_TEXT = "live capture line";

function startLiveCapture(): void {
  useAudioGraphStore.setState({
    sessions: [SESSION_A, SESSION_B],
    sessionsLoading: false,
    isCapturing: true,
    isTranscribing: true,
    selectedSourceIds: ["system-default"],
    captureStartTime: Date.now(),
    loadedSessionId: null,
    pendingFinalizingSession: null,
    transcriptSegments: [segment("live-1", LIVE_TEXT)],
    graphSnapshot: graphSnapshot("live"),
    nav: { dest: "sessions", sessionId: null, lens: "notes" },
  });
}

const CONTENT_FIELDS = [
  "transcriptSegments",
  "graphSnapshot",
  "materializedNotes",
  "materializedProjectionGraph",
  "sessionTimeline",
  "sessionTimelineTotalCount",
  "sessionTimelineLoading",
  "sessionProjectionEvents",
  "sessionTranscriptEvents",
  "diarizationSpanRevisions",
  "speakers",
  "notesLensStatus",
  "graphLensStatus",
  "loadedSessionId",
  "pendingFinalizingSession",
  "transcriptSeekTarget",
  "graphEdgeFocus",
  "asrPartial",
  "liveAssistCards",
] as const;

function liveContent(): Record<string, unknown> {
  const state = useAudioGraphStore.getState();
  return Object.fromEntries(
    CONTENT_FIELDS.map((field) => [field, structuredClone(state[field])]),
  );
}

async function openLens(lens: Lens | "transcript"): Promise<HTMLElement> {
  fireEvent.click(
    screen.getByRole("tab", { name: new RegExp(`^${lens}$`, "i") }),
  );
  return screen.getByRole("tabpanel");
}

async function selectRow(id: string): Promise<void> {
  fireEvent.click(await screen.findByTestId(`session-select-${id}`));
}

/** What a lens shows for `id` once its data has loaded. */
async function expectLensShows(lens: Lens, id: string): Promise<void> {
  const pane = screen.getByRole("tabpanel");
  if (lens === "notes") {
    expect(await within(pane).findByText(`Note of ${id}`)).toBeVisible();
  } else if (lens === "timeline") {
    await waitFor(() =>
      expect(
        within(pane)
          .getAllByTestId("seek-timeline-block")
          .map((el) => el.getAttribute("data-span-id")),
      ).toEqual([`span-${id}`]),
    );
  } else {
    expect(
      await within(pane).findByText(`Nodes: ${NODES[id] ?? 1}`),
    ).toBeInTheDocument();
  }
}

describe("SessionsBrowser: Notes, Timeline and Graph from Review while capture runs", () => {
  beforeEach(async () => {
    mockedInvoke.mockReset();
    localStorage.clear();
    useReviewStore.getState().close();
    await i18n.changeLanguage("en");
    startLiveCapture();
  });

  afterEach(() => {
    useAudioGraphStore.setState({
      sessions: [],
      isCapturing: false,
      isTranscribing: false,
      selectedSourceIds: [],
      captureStartTime: null,
      transcriptSegments: [],
      graphSnapshot: graphSnapshot("empty"),
      loadedSessionId: null,
      pendingFinalizingSession: null,
    });
  });

  it("shows the selected session's data on every lens and leaves the global store's content untouched", async () => {
    mockBackend(defaultRespond);
    const before = liveContent();
    render(<SessionsBrowser />);
    await selectRow("session-a");

    await openLens("notes");
    await expectLensShows("notes", "session-a");
    await openLens("timeline");
    await expectLensShows("timeline", "session-a");
    await openLens("graph");
    await expectLensShows("graph", "session-a");
    await openLens("transcript");
    expect(
      await within(screen.getByRole("tabpanel")).findByText(
        "transcript of session-a",
      ),
    ).toBeVisible();

    // No Live field changed and none of the lens data is the live session's.
    expect(liveContent()).toEqual(before);
    expect(screen.queryByText(LIVE_TEXT)).not.toBeInTheDocument();
    // Only Review's own read-only commands ran: no capture or writer command,
    // and no global lens loader's commands beyond the per-lens artifacts.
    const commands = new Set(mockedInvoke.mock.calls.map(([cmd]) => cmd));
    expect(commands).toEqual(
      new Set([
        "list_sessions",
        "load_session",
        COMMAND.notes,
        COMMAND.timeline,
        COMMAND.graph,
      ]),
    );
    // Ask and the prose summary need the backend's current buffers: not offered.
    expect(
      screen.queryByRole("button", { name: /more actions/i }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^ask$/i })).toBeDisabled();
  });

  it("fetches a heavy artifact only when its lens activates", async () => {
    mockBackend(defaultRespond);
    render(<SessionsBrowser />);
    await selectRow("session-a");
    // Landing lens while live is the Transcript lens.
    await screen.findByText("transcript of session-a");
    const asked = () => mockedInvoke.mock.calls.map(([cmd]) => cmd);
    expect(asked()).not.toContain(COMMAND.notes);
    expect(asked()).not.toContain(COMMAND.timeline);
    expect(asked()).not.toContain(COMMAND.graph);

    await openLens("timeline");
    await expectLensShows("timeline", "session-a");
    expect(asked()).toContain(COMMAND.timeline);
    expect(asked()).not.toContain(COMMAND.notes);
    expect(asked()).not.toContain(COMMAND.graph);
  });

  describe.each(["notes", "timeline", "graph"] as const)("%s lens", (lens) => {
    it("shows only the latest selection when replies arrive out of order (A, B, A)", async () => {
      const pending: Array<{
        id: string;
        reply: ReturnType<typeof deferred<unknown>>;
      }> = [];
      mockBackend((cmd, id) => {
        if (cmd !== COMMAND[lens]) return defaultRespond(cmd, id);
        const reply = deferred<unknown>();
        pending.push({ id, reply });
        return reply.promise;
      });
      render(<SessionsBrowser />);
      await selectRow("session-a"); // request 0: A
      await openLens(lens);
      await waitFor(() => expect(pending).toHaveLength(1));
      await selectRow("session-b"); // request 1: B
      await openLens(lens);
      await waitFor(() => expect(pending).toHaveLength(2));
      await selectRow("session-a"); // request 2: A again
      await openLens(lens);
      await waitFor(() => expect(pending).toHaveLength(3));
      expect(pending.map((p) => p.id)).toEqual([
        "session-a",
        "session-b",
        "session-a",
      ]);

      // The newest A lands first, then the stale B and the stale first A.
      await act(async () => {
        pending[2].reply.resolve(artifactPayload(lens, "session-a"));
      });
      await expectLensShows(lens, "session-a");
      await act(async () => {
        pending[1].reply.resolve(artifactPayload(lens, "session-b"));
        pending[0].reply.resolve(
          lens === "notes"
            ? notesPayload("session-a", "Note of stale A")
            : artifactPayload(lens, "session-b"),
        );
      });
      await expectLensShows(lens, "session-a");
      const pane = screen.getByRole("tabpanel");
      expect(
        within(pane).queryByText("Note of stale A"),
      ).not.toBeInTheDocument();
      expect(within(pane).queryByText("Nodes: 3")).not.toBeInTheDocument();
      expect(
        screen
          .queryAllByTestId("seek-timeline-block")
          .map((el) => el.getAttribute("data-span-id")),
      ).not.toContain("span-session-b");
    });

    it("fails closed on a first-load failure, with no backend text and a working retry", async () => {
      let failures = 1;
      mockBackend((cmd, id) => {
        if (cmd === COMMAND[lens] && failures > 0) {
          failures -= 1;
          throw { code: "io", message: "disk gone: C:\\secret\\path" };
        }
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      const pane = await openLens(lens);

      const notice = await within(pane).findByTestId("review-lens-unavailable");
      expect(notice).toHaveTextContent("This session could not be loaded.");
      expect(pane.textContent).not.toMatch(/disk gone|secret/);

      fireEvent.click(within(pane).getByRole("button", { name: /try again/i }));
      await expectLensShows(lens, "session-a");
      expect(screen.queryByTestId("review-lens-unavailable")).toBeNull();
    });

    it("keeps the last good data and shows the stale state when a refresh fails, then recovers", async () => {
      let failing = false;
      mockBackend((cmd, id) => {
        if (cmd === COMMAND[lens] && failing) {
          throw { code: "io", message: "disk gone" };
        }
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      await openLens(lens);
      await expectLensShows(lens, "session-a");

      // Window focus re-queries; this refresh fails.
      failing = true;
      await act(async () => {
        window.dispatchEvent(new Event("focus"));
      });
      const stale = await screen.findByTestId("review-lens-stale");
      expect(stale).toHaveTextContent(/may be out of date/i);
      expect(stale).not.toHaveTextContent(/disk gone/);
      await expectLensShows(lens, "session-a");

      // Portuguese copy for the same state.
      await act(async () => {
        await i18n.changeLanguage("pt");
      });
      expect(screen.getByTestId("review-lens-stale")).toHaveTextContent(
        /pode estar desatualizada/i,
      );
      await act(async () => {
        await i18n.changeLanguage("en");
      });

      failing = false;
      fireEvent.click(
        within(screen.getByTestId("review-lens-stale")).getByRole("button", {
          name: /try again/i,
        }),
      );
      await waitFor(() =>
        expect(screen.queryByTestId("review-lens-stale")).toBeNull(),
      );
      await expectLensShows(lens, "session-a");
    });
  });

  it("shows the closed byte-ceiling refusal for an oversized artifact", async () => {
    mockBackend((cmd, id) => {
      if (cmd === COMMAND.notes) {
        throw {
          code: "artifact_too_large",
          message: {
            artifact_class: "notes",
            size_bytes: 80 * 1024 * 1024,
            ceiling_bytes: 64 * 1024 * 1024,
          },
        };
      }
      return defaultRespond(cmd, id);
    });
    render(<SessionsBrowser />);
    await selectRow("session-a");
    const pane = await openLens("notes");
    expect(
      await within(pane).findByTestId("artifact-refusal-notice"),
    ).toBeVisible();
  });

  it("lets a reviewed session's Timeline drive its own Transcript and Graph, writing no Live target", async () => {
    mockBackend((cmd, id) => {
      if (cmd === "load_session") {
        return {
          ...loadSessionPayload(id),
          transcript_events: [
            {
              span_id: `span-${id}`,
              provider: "deepgram",
              source_id: "system-default",
              transcript_segment_id: `${id}-1`,
              text: `transcript of ${id}`,
              start_time: 0,
              end_time: 1,
              confidence: 1,
              is_final: true,
              stability: "final",
              revision_number: 1,
              end_of_turn: true,
              received_at_ms: 1,
            },
          ],
        };
      }
      if (cmd === COMMAND.timeline) {
        const fold = timelinePayload(id);
        fold.entries[0].related_edge_ids = ["edge-1"];
        return fold;
      }
      return defaultRespond(cmd, id);
    });
    const scrollIntoView = vi.fn();
    const original = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = scrollIntoView;
    try {
      render(<SessionsBrowser />);
      await selectRow("session-a");
      await openLens("timeline");
      await expectLensShows("timeline", "session-a");

      fireEvent.click(screen.getByTestId("seek-timeline-block"));
      expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
        "session-a-1",
      );
      fireEvent.click(screen.getByTestId("seek-timeline-edge-badge"));
      expect(useReviewStore.getState().graphEdgeFocus?.edgeIds).toEqual([
        "edge-1",
      ]);
      // The Live targets were never written.
      expect(useAudioGraphStore.getState().transcriptSeekTarget).toBeNull();
      expect(useAudioGraphStore.getState().graphEdgeFocus).toBeNull();

      // Opening the Transcript lens consumes Review's target.
      await openLens("transcript");
      await waitFor(() => expect(scrollIntoView).toHaveBeenCalledTimes(1));
    } finally {
      useReviewStore.setState({ transcriptSeekTarget: null });
      HTMLElement.prototype.scrollIntoView = original;
    }
  });

  describe("Live updates while Review is open", () => {
    it.each([
      "transcript",
      "notes",
      "timeline",
      "graph",
    ] as const)("do not re-render the Review detail pane on the %s lens", async (lens) => {
      mockBackend(defaultRespond);
      let commits = 0;
      render(
        <Profiler
          id="sessions"
          onRender={() => {
            commits += 1;
          }}
        >
          <SessionsBrowser />
        </Profiler>,
      );
      await selectRow("session-a");
      await openLens(lens);
      if (lens === "transcript") {
        await screen.findByText("transcript of session-a");
      } else {
        await expectLensShows(lens, "session-a");
      }
      // Let every in-flight query and effect settle before counting.
      await act(async () => {});
      const settled = commits;
      const liveBefore = liveContent();

      // Live transcript, graph, notes, timeline and speaker updates land.
      act(() => {
        useAudioGraphStore.setState({
          transcriptSegments: [
            segment("live-1", LIVE_TEXT),
            segment("live-2", "another live line"),
          ],
          graphSnapshot: graphSnapshot("live-bigger"),
          asrPartial: {
            provider: "deepgram",
            source_id: "system-default",
            text: "live partial",
            start_time: 0,
            end_time: 1,
            confidence: 0.5,
            timestamp_ms: 0,
          },
          sessionTimeline: [],
          sessionProjectionEvents: [],
          sessionTranscriptEvents: [],
          diarizationSpanRevisions: [],
          speakers: [],
          materializedNotes: null,
          materializedProjectionGraph: null,
          sessionTimelineLoading: true,
        });
      });
      await act(async () => {});

      expect(commits).toBe(settled);
      expect(liveContent()).not.toEqual(liveBefore); // the updates did land
      expect(screen.queryByText("another live line")).toBeNull();
      expect(screen.queryByText("live partial")).toBeNull();
    });
  });
});
