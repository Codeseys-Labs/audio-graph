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
import {
  afterEach,
  beforeAll,
  beforeEach,
  describe,
  expect,
  it,
  vi,
} from "vitest";
import i18n from "../i18n";
import { useReviewStore } from "../session/reviewStore";
import { type Deferred, deferred, segment } from "../session/reviewTestUtils";
import { useAudioGraphStore } from "../store";
import { preloadGraphLens } from "../test/lazyChunks";
import type {
  GraphNode,
  GraphSnapshot,
  MaterializedGraph,
  MaterializedNote,
  SessionMetadata,
  SessionNotesArtifacts,
  SessionTimelineFold,
} from "../types";
import { downloadAsFile } from "../utils/download";
import SessionsBrowser from "./SessionsBrowser";

// react-force-graph-2d draws to a canvas jsdom cannot drive.
const forceGraphProps: { current: Record<string, unknown> | null } = {
  current: null,
};
vi.mock("react-force-graph-2d", async () => {
  const React = await vi.importActual<typeof import("react")>("react");
  return {
    default: React.forwardRef(
      (props: Record<string, unknown>, ref: React.Ref<unknown>) => {
        forceGraphProps.current = props;
        React.useImperativeHandle(ref, () => ({
          zoomToFit: () => {},
          d3ReheatSimulation: () => {},
          d3Force: () => ({ strength: () => {}, distance: () => {} }),
        }));
        return React.createElement("div", { "data-testid": "force-graph" });
      },
    ),
  };
});

vi.mock("../utils/download", async (importOriginal) => ({
  ...(await importOriginal<Record<string, unknown>>()),
  downloadAsFile: vi.fn(),
}));

const mockedInvoke = vi.mocked(invoke);
const mockedDownload = vi.mocked(downloadAsFile);

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

// The two graph representations deliberately differ in size, so a count
// assertion tells which one a lens shows: `load_session`'s live-lane snapshot
// (LEGACY_NODES) or the richer materialized artifact the Graph lens prefers.
const LEGACY_NODES: Record<string, number> = { "session-a": 2, "session-b": 3 };
const MATERIALIZED_NODES: Record<string, number> = {
  "session-a": 4,
  "session-b": 5,
};

function graphSnapshot(id: string): GraphSnapshot {
  const count = LEGACY_NODES[id] ?? 1;
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
    nodes: Array.from({ length: MATERIALIZED_NODES[id] ?? 1 }, (_, i) => ({
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
    edges: ["edge-1", "edge-2"].map((edgeId, i) => ({
      id: edgeId,
      source: `${id}-m${i}`,
      target: `${id}-m${i + 1}`,
      relation_type: "relates_to",
      weight: 1,
      confidence: 1,
      valid_from_ms: 0,
      updated_by_sequence: 1,
      updated_at_ms: 1,
      basis: null,
      provenance: null,
    })),
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
  "samplePreviewActive",
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
      await within(pane).findByText(`Nodes: ${MATERIALIZED_NODES[id] ?? 1}`),
    ).toBeInTheDocument();
  }
}

beforeAll(preloadGraphLens);

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
        reply: Deferred<unknown>;
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
      expect(within(pane).queryByText("Nodes: 5")).not.toBeInTheDocument();
      // Nor the live-lane snapshot's count.
      expect(within(pane).queryByText("Nodes: 2")).not.toBeInTheDocument();
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
            artifact_class: "materialized_notes",
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
      const badge = screen.getByTestId("seek-timeline-edge-badge");
      badge.focus();
      expect(document.activeElement).toBe(badge);
      fireEvent.click(badge); // what Enter / Space do on a button
      expect(useReviewStore.getState().graphEdgeFocus?.edgeIds).toEqual([
        "edge-1",
      ]);
      // The Live targets were never written.
      expect(useAudioGraphStore.getState().transcriptSeekTarget).toBeNull();
      expect(useAudioGraphStore.getState().graphEdgeFocus).toBeNull();

      // The badge click also brought this Review pane to its Graph lens, with
      // the reviewed graph on screen.
      expect(screen.getByRole("tab", { name: /^graph$/i })).toHaveAttribute(
        "aria-selected",
        "true",
      );
      await expectLensShows("graph", "session-a");
      // The badge unmounted with the Timeline lens: focus moved to the Graph tab.
      expect(document.activeElement).toBe(
        screen.getByRole("tab", { name: /^graph$/i }),
      );

      // Opening the Transcript lens consumes Review's target.
      await openLens("transcript");
      await waitFor(() => expect(scrollIntoView).toHaveBeenCalledTimes(1));

      // Opening the Graph lens shows the reviewed graph with the focused edge
      // emphasised: the focused edge keeps its colour and gets thicker, the
      // other edge is dimmed.
      await openLens("graph");
      await expectLensShows("graph", "session-a");
      type EdgeStyle = (link: unknown) => string | number;
      const style = (name: "linkColor" | "linkWidth"): EdgeStyle => {
        const fn = forceGraphProps.current?.[name];
        if (typeof fn !== "function") throw new Error(`${name} not set`);
        return fn as EdgeStyle;
      };
      const focused = { id: "edge-1", color: "#123456", weight: 1 };
      const other = { id: "edge-2", color: "#123456", weight: 1 };
      await waitFor(() => expect(style("linkColor")(focused)).toBe("#123456"));
      expect(style("linkColor")(other)).toBe("#12345615");
      expect(Number(style("linkWidth")(focused))).toBeGreaterThan(
        Number(style("linkWidth")(other)),
      );

      // Its export is the reviewed session's own graph, built locally: the
      // backend's `export_graph` dumps the LIVE session's.
      mockedInvoke.mockClear();
      mockedDownload.mockClear();
      await act(async () => {
        fireEvent.click(
          screen.getByRole("button", {
            name: /export knowledge graph as json/i,
          }),
        );
      });
      await waitFor(() => expect(mockedDownload).toHaveBeenCalledTimes(1));
      const [json, filename] = mockedDownload.mock.calls[0];
      expect(filename).toMatch(/^graph-session-a-/);
      const exported = JSON.parse(json);
      expect(exported.session_id).toBe("session-a");
      expect(exported.materialized_graph.nodes).toHaveLength(4);
      expect(exported.snapshot.stats.total_nodes).toBe(4);
      expect(JSON.stringify(exported)).not.toContain("session-b");
      expect(mockedInvoke).not.toHaveBeenCalledWith("export_graph");
      expect(mockedInvoke).not.toHaveBeenCalledWith("get_session_id");
    } finally {
      useReviewStore.setState({ transcriptSeekTarget: null });
      HTMLElement.prototype.scrollIntoView = original;
    }
  });

  describe("one consistent read per round", () => {
    function transcriptEvent(spanId: string, segmentId: string) {
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

    function snapshotWith(id: string, spans: string[]) {
      return {
        ...loadSessionPayload(id),
        transcript: spans.map((span) =>
          segment(span.replace("span", "seg"), `transcript of ${id}`),
        ),
        transcript_events: spans.map((span) =>
          transcriptEvent(span, span.replace("span", "seg")),
        ),
      };
    }

    function foldOf(spans: string[]): SessionTimelineFold {
      return {
        entries: spans.map((span, i) => ({
          span_id: span,
          start_ms: i * 1000,
          end_ms: i * 1000 + 900,
          received_at_ms: 1,
          speaker_id: null,
          speaker_label: null,
          text: `said ${span}`,
          related_edge_ids: [],
        })),
        total_count: spans.length,
      };
    }

    function blockSpans(): Array<string | null> {
      return screen
        .queryAllByTestId("seek-timeline-block")
        .map((el) => el.getAttribute("data-span-id"));
    }

    it("renders neither half of a transcript-then-timeline activation alone, then seeks a new span's block to its own segment", async () => {
      const snapshotTwo = deferred<unknown>();
      const timelineTwo = deferred<unknown>();
      let loads = 0;
      mockBackend((cmd, id) => {
        if (cmd === "load_session") {
          loads += 1;
          return loads === 1
            ? snapshotWith(id, ["span-1"])
            : snapshotTwo.promise;
        }
        if (cmd === COMMAND.timeline) return timelineTwo.promise;
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      await screen.findByText("transcript of session-a"); // snapshot one

      // Finalization persisted another span; Timeline activates. The round
      // re-reads the snapshot AND the fold together.
      const pane = await openLens("timeline");
      await waitFor(() => expect(loads).toBe(2));

      // The fold (with the new span) arrives first: it must not render next
      // to the older snapshot.
      await act(async () => {
        timelineTwo.resolve(foldOf(["span-1", "span-2"]));
      });
      expect(blockSpans()).toEqual([]);
      expect(within(pane).getByRole("status")).toHaveTextContent(/loading/i);

      // Both halves land together.
      await act(async () => {
        snapshotTwo.resolve(snapshotWith("session-a", ["span-1", "span-2"]));
      });
      await waitFor(() => expect(blockSpans()).toEqual(["span-1", "span-2"]));

      fireEvent.click(
        screen
          .getAllByTestId("seek-timeline-block")
          .find(
            (el) => el.getAttribute("data-span-id") === "span-2",
          ) as HTMLElement,
      );
      expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
        "seg-2",
      );
      expect(loads).toBe(2); // resolved from the same round: no extra read
    });

    it("refreshes the snapshot once, then seeks, when a block names a span the snapshot lacks", async () => {
      const refresh = deferred<unknown>();
      let loads = 0;
      mockBackend((cmd, id) => {
        if (cmd === "load_session") {
          loads += 1;
          if (loads <= 2) return snapshotWith(id, ["span-1"]); // behind the fold
          return refresh.promise;
        }
        if (cmd === COMMAND.timeline) return foldOf(["span-1", "span-2"]);
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      await openLens("timeline");
      await waitFor(() => expect(blockSpans()).toEqual(["span-1", "span-2"]));

      fireEvent.click(
        screen
          .getAllByTestId("seek-timeline-block")
          .find(
            (el) => el.getAttribute("data-span-id") === "span-2",
          ) as HTMLElement,
      );
      await waitFor(() => expect(loads).toBe(3));
      // Not a raw span id, and nothing yet: the refresh is still pending.
      expect(useReviewStore.getState().transcriptSeekTarget).toBeNull();

      await act(async () => {
        refresh.resolve(snapshotWith("session-a", ["span-1", "span-2"]));
      });
      await waitFor(() =>
        expect(useReviewStore.getState().transcriptSeekTarget?.segmentId).toBe(
          "seg-2",
        ),
      );
      expect(loads).toBe(3); // one refresh, not more
    });

    it("keeps the previous pair, marked stale, when a focus refresh gets a new snapshot but a failing fold", async () => {
      let loads = 0;
      let failTimeline = false;
      mockBackend((cmd, id) => {
        if (cmd === "load_session") {
          loads += 1;
          return snapshotWith(
            id,
            loads <= 2 ? ["span-1"] : ["span-1", "span-2"],
          );
        }
        if (cmd === COMMAND.timeline) {
          if (failTimeline) throw { code: "io", message: "disk gone" };
          return foldOf(["span-1"]);
        }
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      await openLens("timeline");
      await waitFor(() => expect(blockSpans()).toEqual(["span-1"]));

      failTimeline = true;
      await act(async () => {
        window.dispatchEvent(new Event("focus"));
      });
      // The newer snapshot was NOT published next to the old fold.
      expect(
        useReviewStore
          .getState()
          .snapshot?.transcriptEvents.map((e) => e.span_id),
      ).toEqual(["span-1"]);
      expect(await screen.findByTestId("review-lens-stale")).toBeVisible();
      expect(blockSpans()).toEqual(["span-1"]);
    });
  });

  describe("refresh pile-up", () => {
    it("starts no overlapping refresh for focus and visibility, and queues one trailing round", async () => {
      const gate = deferred<unknown>();
      let loads = 0;
      mockBackend((cmd, id) => {
        if (cmd === "load_session") {
          loads += 1;
          // Read 1 is the Transcript lens's opening round; read 2 is the Notes
          // round the user opens next, which is the one held back.
          return loads === 2 ? gate.promise : loadSessionPayload(id);
        }
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      await screen.findByText("transcript of session-a");
      await openLens("notes");
      await waitFor(() => expect(loads).toBe(2));

      // While the Notes round is in flight: focus x3 and a visibility change.
      await act(async () => {
        window.dispatchEvent(new Event("focus"));
        window.dispatchEvent(new Event("focus"));
        window.dispatchEvent(new Event("focus"));
        document.dispatchEvent(new Event("visibilitychange"));
      });
      expect(loads).toBe(2);

      await act(async () => {
        gate.resolve(loadSessionPayload("session-a"));
      });
      await expectLensShows("notes", "session-a");
      await act(async () => {});
      // The Notes round plus ONE trailing round for all four triggers.
      expect(loads).toBeLessThanOrEqual(3);
    });

    it("lets an explicit lens switch replace the round in flight instead of queueing behind it", async () => {
      const gate = deferred<unknown>();
      let loads = 0;
      mockBackend((cmd, id) => {
        if (cmd === "load_session") {
          loads += 1;
          return loads === 2 ? gate.promise : loadSessionPayload(id);
        }
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      await screen.findByText("transcript of session-a");
      await openLens("notes");
      await waitFor(() => expect(loads).toBe(2));

      // The Notes round never settles; the user moves on to Timeline.
      await openLens("timeline");
      await expectLensShows("timeline", "session-a");
      expect(loads).toBe(3);

      // The abandoned round's reply lands late and changes nothing.
      await act(async () => {
        gate.resolve({
          ...loadSessionPayload("session-a"),
          transcript: [segment("late", "late abandoned text")],
        });
      });
      await openLens("transcript");
      expect(await screen.findByText("transcript of session-a")).toBeVisible();
      expect(screen.queryByText("late abandoned text")).toBeNull();
    });
  });

  describe("keyboard focus after a retry", () => {
    it("moves focus to the lens region when the first-load retry control goes away", async () => {
      let failures = 1;
      mockBackend((cmd, id) => {
        if (cmd === COMMAND.notes && failures > 0) {
          failures -= 1;
          throw { code: "io", message: "disk gone" };
        }
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      const pane = await openLens("notes");
      const retry = await within(pane).findByRole("button", {
        name: /try again/i,
      });
      retry.focus();
      expect(document.activeElement).toBe(retry);

      fireEvent.click(retry); // what Enter / Space do on a button
      await expectLensShows("notes", "session-a");
      expect(document.activeElement).toBe(
        screen.getByTestId("review-lens-region"),
      );
    });

    it("moves focus to the lens region when the stale retry control goes away on recovery", async () => {
      let failing = false;
      mockBackend((cmd, id) => {
        if (cmd === COMMAND.notes && failing) {
          throw { code: "io", message: "disk gone" };
        }
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      await openLens("notes");
      await expectLensShows("notes", "session-a");

      failing = true;
      await act(async () => {
        window.dispatchEvent(new Event("focus"));
      });
      const stale = await screen.findByTestId("review-lens-stale");
      const retry = within(stale).getByRole("button", { name: /try again/i });
      retry.focus();
      failing = false;
      fireEvent.click(retry);

      await waitFor(() =>
        expect(screen.queryByTestId("review-lens-stale")).toBeNull(),
      );
      expect(document.activeElement).toBe(
        screen.getByTestId("review-lens-region"),
      );
    });

    it("moves focus to the transcript region when the transcript's retry control goes away", async () => {
      let failures = 1;
      mockBackend((cmd, id) => {
        if (cmd === "load_session" && failures > 0) {
          failures -= 1;
          throw { code: "io", message: "disk gone" };
        }
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      const pane = screen.getByRole("tabpanel");
      const retry = await within(pane).findByRole("button", {
        name: /try again/i,
      });
      retry.focus();
      fireEvent.click(retry);
      await screen.findByText("transcript of session-a");
      expect(document.activeElement).toBe(
        within(screen.getByRole("tabpanel")).getByRole("log"),
      );
    });
  });

  describe("keyboard focus is never stolen after a retry", () => {
    it("leaves focus alone when the user moved it elsewhere before the retry control went away", async () => {
      const gate = deferred<unknown>();
      let failures = 1;
      let loads = 0;
      mockBackend((cmd, id) => {
        if (cmd === COMMAND.notes) {
          if (failures > 0) {
            failures -= 1;
            throw { code: "io", message: "disk gone" };
          }
          loads += 1;
          return gate.promise;
        }
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      const pane = await openLens("notes");
      const retry = await within(pane).findByRole("button", {
        name: /try again/i,
      });
      retry.focus();
      fireEvent.click(retry);
      await waitFor(() => expect(loads).toBe(1));

      // While it loads, the user tabs to a different control.
      const otherTab = screen.getByRole("tab", { name: /^transcript$/i });
      otherTab.focus();
      expect(document.activeElement).toBe(otherTab);

      await act(async () => {
        gate.resolve(notesPayload("session-a"));
      });
      await expectLensShows("notes", "session-a");
      expect(document.activeElement).toBe(otherTab);
    });

    it("does not move focus for a retry the keyboard never activated", async () => {
      let failures = 1;
      mockBackend((cmd, id) => {
        if (cmd === COMMAND.notes && failures > 0) {
          failures -= 1;
          throw { code: "io", message: "disk gone" };
        }
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      const pane = await openLens("notes");
      const retry = await within(pane).findByRole("button", {
        name: /try again/i,
      });
      // Focus is on the body (e.g. a pointer click on a browser that does not
      // focus buttons): the region is not forced either unless the retry
      // control itself had focus.
      (document.activeElement as HTMLElement | null)?.blur();
      fireEvent.click(retry);
      await expectLensShows("notes", "session-a");
      expect(document.activeElement).toBe(document.body);
    });
  });

  describe("malformed typed refusals", () => {
    it.each([
      ["no message", undefined],
      [
        "an unexpected artifact class",
        { artifact_class: "../../etc/passwd", size_bytes: 1, ceiling_bytes: 1 },
      ],
      [
        "a negative size",
        {
          artifact_class: "materialized_notes",
          size_bytes: -1,
          ceiling_bytes: 1,
        },
      ],
    ])("show the closed unavailable state, never the raw class, for %s", async (_name, message) => {
      mockBackend((cmd, id) => {
        if (cmd === COMMAND.notes)
          throw { code: "artifact_too_large", message };
        return defaultRespond(cmd, id);
      });
      render(<SessionsBrowser />);
      await selectRow("session-a");
      const pane = await openLens("notes");
      expect(
        await within(pane).findByTestId("review-lens-unavailable"),
      ).toBeVisible();
      expect(pane.textContent).not.toMatch(/etc\/passwd/);
      expect(within(pane).queryByText(/^loading/i)).toBeNull();
    });
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
