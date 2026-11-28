/**
 * Review store — the query-backed snapshot of ONE historical session
 * (issue #140, ADR-0028 frontend half).
 *
 * Live stays in the global `useAudioGraphStore`, owned by the backend's one
 * live session. Review is deliberately a separate store with no subscription
 * to live content events: it calls the read-only `load_session` command
 * directly and holds the result here, so opening a past session while
 * capture runs can never write a Live field (`transcriptSegments`,
 * `speakers`, `diarizationSpanRevisions`, …) or rotate the live session.
 *
 * Review is refreshable, not immutable: a reviewed session that is still
 * finalizing changes on disk (#169, ADR-0036), so `refresh()` re-queries on
 * open and on window focus (see `useReviewTranscript`).
 *
 * Stale responses: every `select()` / `refresh()` takes a new selection
 * generation, and a response is applied only if its generation is still the
 * latest. That is what rejects the older reply in an A → B → A sequence even
 * though both A requests name the same session id.
 *
 * Heavy artifacts (Notes, Graph, Timeline) are fetched per lens, only when the
 * lens activates (`useReviewArtifact`), into their own slots. Every artifact
 * request is fenced twice: by the selection epoch (bumped whenever the
 * selected session changes, so an A → B → A sequence drops the first A's
 * reply) and by a per-kind sequence (the latest request for a kind wins).
 * A failed re-query keeps the slot's last good data and marks it `stale`.
 *
 * Seek and edge-focus targets (the Timeline lens driving the Transcript and
 * Graph lenses) live here too, so a Review never writes a Live field.
 *
 * Errors are a CLOSED set (`ReviewErrorCode`): the backend's message is never
 * stored or rendered, because it can embed paths and artifact names. The one
 * typed exception is the byte-ceiling refusal, which carries only numbers and
 * a stable artifact class name.
 */
import { create } from "zustand";
import { safeInvoke as invoke } from "../analytics/safeInvoke";
import { TRANSCRIPT_WINDOW_SIZE } from "../constants/transcript";
import type {
  DiarizationSpanRevisionEvent,
  GraphSnapshot,
  LoadedSession,
  MaterializedGraph,
  SessionNotesArtifacts,
  SessionTimelineFold,
  TranscriptEvent,
  TranscriptSegment,
} from "../types";
import { artifactTooLargeDetails } from "../utils/errorToMessage";

/** Transcript + persisted speaker state for one reviewed session. */
export interface ReviewSnapshot {
  sessionId: string;
  transcript: TranscriptSegment[];
  transcriptEvents: TranscriptEvent[];
  /** Persisted diarization span revisions (the speaker timeline ledger). */
  diarizationEvents: DiarizationSpanRevisionEvent[];
  /** The live-lane graph snapshot `load_session` returns next to the transcript. */
  graph: GraphSnapshot;
  /** Wall-clock ms the snapshot was read, for "refreshed" diagnostics. */
  loadedAtMs: number;
}

/** Closed error vocabulary. `unavailable` = missing, unreadable, or corrupt. */
export type ReviewErrorCode = "unavailable";

export type ReviewStatus = "idle" | "loading" | "ready" | "error";

/** The heavy per-lens artifacts, each fetched only when its lens activates. */
export type ArtifactKind = "notes" | "graph" | "timeline";

/** A byte-ceiling refusal: stable class name plus numbers, never prose. */
export interface ArtifactRefusal {
  artifactClass: string;
  sizeBytes: number;
  ceilingBytes: number;
}

export interface ArtifactSlot<T> {
  status: "idle" | "loading" | "ready" | "error" | "refused";
  /** Last good data for the selected session (`ready`, or `stale` below). */
  data: T | null;
  /** The last re-query failed; `data` was valid when read, may be out of date. */
  stale: boolean;
  refusal: ArtifactRefusal | null;
}

export interface ReviewArtifacts {
  notes: ArtifactSlot<SessionNotesArtifacts>;
  /** `data: null` with `status: "ready"` means the session has no graph. */
  graph: ArtifactSlot<MaterializedGraph | null>;
  timeline: ArtifactSlot<SessionTimelineFold>;
}

interface SeekTarget {
  segmentId: string;
  nonce: number;
}

interface EdgeFocus {
  edgeIds: string[];
  nonce: number;
}

function idleSlot<T>(): ArtifactSlot<T> {
  return { status: "idle", data: null, stale: false, refusal: null };
}

function idleArtifacts(): ReviewArtifacts {
  return {
    notes: idleSlot(),
    graph: idleSlot(),
    timeline: idleSlot(),
  };
}

const EMPTY_GRAPH: GraphSnapshot = {
  nodes: [],
  links: [],
  stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parseNotes(raw: unknown): SessionNotesArtifacts {
  if (!isRecord(raw) || !Array.isArray(raw.projection_events)) {
    throw new Error("malformed notes artifact");
  }
  if (raw.notes !== null && raw.notes !== undefined && !isRecord(raw.notes)) {
    throw new Error("malformed notes artifact");
  }
  return {
    notes: (raw.notes ?? null) as SessionNotesArtifacts["notes"],
    projection_events:
      raw.projection_events as SessionNotesArtifacts["projection_events"],
  };
}

function parseGraph(raw: unknown): MaterializedGraph | null {
  if (raw === null || raw === undefined) return null;
  if (!isRecord(raw) || !Array.isArray(raw.nodes)) {
    throw new Error("malformed graph artifact");
  }
  return raw as unknown as MaterializedGraph;
}

function parseTimeline(raw: unknown): SessionTimelineFold {
  if (
    !isRecord(raw) ||
    !Array.isArray(raw.entries) ||
    typeof raw.total_count !== "number"
  ) {
    throw new Error("malformed timeline artifact");
  }
  return raw as unknown as SessionTimelineFold;
}

/** Per-kind command and payload check; one table, no per-kind branching. */
const ARTIFACT_SOURCES: {
  [K in ArtifactKind]: {
    command: string;
    args: (sessionId: string) => Record<string, unknown>;
    parse: (raw: unknown) => ReviewArtifacts[K]["data"];
  };
} = {
  notes: {
    command: "load_session_notes_artifacts_cmd",
    args: (sessionId) => ({ sessionId }),
    parse: parseNotes,
  },
  graph: {
    command: "load_session_graph_artifact_cmd",
    args: (sessionId) => ({ sessionId }),
    parse: parseGraph,
  },
  timeline: {
    command: "build_session_timeline_cmd",
    // Cap the fold to the tail the timeline renders, as the Live loader does.
    args: (sessionId) => ({ sessionId, limit: TRANSCRIPT_WINDOW_SIZE }),
    parse: parseTimeline,
  },
};

interface ReviewState {
  /** The selected session id, or null when no Review is open. */
  sessionId: string | null;
  /** Latest selection generation; responses from older generations drop. */
  generation: number;
  status: ReviewStatus;
  /** Snapshot for `sessionId` only; never another session's data. */
  snapshot: ReviewSnapshot | null;
  /** True while a re-query runs over an already-ready snapshot. */
  refreshing: boolean;
  /**
   * The last re-query failed, so `snapshot` was valid when read but may be out
   * of date. Cleared by the next successful query. Never set without a
   * snapshot (a first-load failure is `status: "error"` instead).
   */
  stale: boolean;
  error: ReviewErrorCode | null;
  /**
   * Bumped whenever the selected session changes (or closes). Artifact
   * replies carry the epoch they were requested under and drop on mismatch.
   */
  artifactEpoch: number;
  artifacts: ReviewArtifacts;
  /** Timeline click target for the Transcript lens (Review's own, not Live's). */
  transcriptSeekTarget: SeekTarget | null;
  /** Timeline badge target for the Graph lens (Review's own, not Live's). */
  graphEdgeFocus: EdgeFocus | null;
  seekTranscriptToSegment: (segmentId: string | null) => void;
  focusGraphEdges: (edgeIds: string[] | null) => void;
  /**
   * Fetch (or re-fetch) one lens artifact for the selected session. Called
   * when the lens activates and on window focus; keeps the last good data
   * visible while re-querying.
   */
  loadArtifact: (kind: ArtifactKind) => Promise<void>;
  /** Open (or re-open) a session. Returns once its response settled/dropped. */
  select: (sessionId: string) => Promise<void>;
  /** Re-query the selected session, keeping the current snapshot meanwhile. */
  refresh: () => Promise<void>;
  /** Drop the selection and invalidate any in-flight request. */
  close: () => void;
}

/** Narrow an untrusted `load_session` payload; throws on a corrupt shape. */
function toSnapshot(sessionId: string, loaded: unknown): ReviewSnapshot {
  if (typeof loaded !== "object" || loaded === null) {
    throw new Error("review payload is not an object");
  }
  const payload = loaded as Partial<LoadedSession>;
  const transcriptEvents = payload.transcript_events ?? [];
  const diarizationEvents = payload.diarization_events ?? [];
  if (
    !Array.isArray(payload.transcript) ||
    !Array.isArray(transcriptEvents) ||
    !Array.isArray(diarizationEvents)
  ) {
    throw new Error("review payload has a malformed artifact");
  }
  const graph = payload.graph ?? EMPTY_GRAPH;
  if (!isRecord(graph) || !Array.isArray(graph.nodes)) {
    throw new Error("review payload has a malformed graph");
  }
  return {
    sessionId,
    transcript: payload.transcript,
    transcriptEvents,
    diarizationEvents,
    graph,
    loadedAtMs: Date.now(),
  };
}

export const useReviewStore = create<ReviewState>((set, get) => {
  /** One request under a fresh generation; applies only if still latest. */
  const query = async (sessionId: string): Promise<void> => {
    const generation = get().generation + 1;
    set({ generation });
    const isStale = () =>
      get().generation !== generation || get().sessionId !== sessionId;
    try {
      const loaded = await invoke<LoadedSession>("load_session", { sessionId });
      if (isStale()) return;
      set({
        snapshot: toSnapshot(sessionId, loaded),
        status: "ready",
        refreshing: false,
        stale: false,
        error: null,
      });
    } catch {
      if (isStale()) return;
      // A failed refresh keeps the last good snapshot (it was valid when
      // read) but marks it `stale` so the pane can say it may be out of date
      // and offer a retry; only a first load becomes the error state.
      if (get().snapshot?.sessionId === sessionId) {
        set({ refreshing: false, stale: true });
        return;
      }
      set({
        snapshot: null,
        status: "error",
        refreshing: false,
        stale: false,
        error: "unavailable",
      });
    }
  };

  /** Latest request per kind; an older reply for the same kind drops. */
  const latestRequest: Record<ArtifactKind, number> = {
    notes: 0,
    graph: 0,
    timeline: 0,
  };

  const setSlot = <K extends ArtifactKind>(
    kind: K,
    slot: ReviewArtifacts[K],
  ): void =>
    set((state) => ({ artifacts: { ...state.artifacts, [kind]: slot } }));

  const fetchArtifact = async <K extends ArtifactKind>(
    kind: K,
  ): Promise<void> => {
    const sessionId = get().sessionId;
    if (sessionId === null) return;
    const epoch = get().artifactEpoch;
    const request = ++latestRequest[kind];
    const isStale = () =>
      get().artifactEpoch !== epoch || latestRequest[kind] !== request;
    const source = ARTIFACT_SOURCES[kind];
    const previous = get().artifacts[kind] as ReviewArtifacts[K];
    // Re-querying keeps the last good data on screen; a first query shows loading.
    if (previous.data === null && previous.status !== "ready") {
      setSlot(kind, { ...previous, status: "loading", refusal: null });
    }
    try {
      const raw = await invoke<unknown>(source.command, source.args(sessionId));
      if (isStale()) return;
      setSlot(kind, {
        status: "ready",
        data: source.parse(raw),
        stale: false,
        refusal: null,
      } as ReviewArtifacts[K]);
    } catch (e) {
      if (isStale()) return;
      const current = get().artifacts[kind] as ReviewArtifacts[K];
      // A failed re-query keeps the last good data, marked stale.
      if (current.status === "ready") {
        setSlot(kind, { ...current, stale: true });
        return;
      }
      const refusal = artifactTooLargeDetails(e);
      setSlot(kind, {
        status: refusal ? "refused" : "error",
        data: null,
        stale: false,
        refusal,
      } as ReviewArtifacts[K]);
    }
  };

  return {
    sessionId: null,
    generation: 0,
    artifactEpoch: 0,
    artifacts: idleArtifacts(),
    transcriptSeekTarget: null,
    graphEdgeFocus: null,
    seekTranscriptToSegment: (segmentId) =>
      set((state) => ({
        transcriptSeekTarget: segmentId
          ? {
              segmentId,
              nonce: (state.transcriptSeekTarget?.nonce ?? 0) + 1,
            }
          : null,
      })),
    focusGraphEdges: (edgeIds) =>
      set((state) => ({
        graphEdgeFocus:
          edgeIds && edgeIds.length > 0
            ? { edgeIds, nonce: (state.graphEdgeFocus?.nonce ?? 0) + 1 }
            : null,
      })),
    loadArtifact: (kind) => fetchArtifact(kind),
    status: "idle",
    snapshot: null,
    refreshing: false,
    stale: false,
    error: null,
    select: (sessionId) => {
      const sameSession = get().snapshot?.sessionId === sessionId;
      const changed = get().sessionId !== sessionId;
      set((state) => ({
        // A different session starts from empty artifacts and fresh targets,
        // and drops every reply still in flight for the previous selection.
        ...(changed
          ? {
              artifactEpoch: state.artifactEpoch + 1,
              artifacts: idleArtifacts(),
              transcriptSeekTarget: null,
              graphEdgeFocus: null,
            }
          : {}),
        sessionId,
        // Re-selecting the session already shown re-queries in place; any
        // other session starts from a clean loading state, never showing the
        // previous session's transcript under the new title.
        status: sameSession ? "ready" : "loading",
        snapshot: sameSession ? get().snapshot : null,
        refreshing: sameSession,
        stale: sameSession ? get().stale : false,
        error: null,
      }));
      return query(sessionId);
    },
    refresh: () => {
      const { sessionId, snapshot } = get();
      if (sessionId === null) return Promise.resolve();
      if (snapshot?.sessionId === sessionId) set({ refreshing: true });
      else set({ status: "loading", error: null });
      return query(sessionId);
    },
    close: () =>
      set((state) => ({
        sessionId: null,
        generation: state.generation + 1,
        status: "idle",
        snapshot: null,
        refreshing: false,
        stale: false,
        error: null,
        artifactEpoch: state.artifactEpoch + 1,
        artifacts: idleArtifacts(),
        transcriptSeekTarget: null,
        graphEdgeFocus: null,
      })),
  };
});
