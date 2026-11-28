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
  status: ReviewStatus;
  /** Snapshot for `sessionId` only; never another session's data. */
  snapshot: ReviewSnapshot | null;
  /** True while a re-query runs over an already-ready snapshot. */
  refreshing: boolean;
  /**
   * The last re-query failed, so `snapshot` (and the active lens's artifact
   * with it) was valid when read but may be out of date. Cleared by the next
   * successful round. Never set without a snapshot (a first-load failure is
   * `status: "error"` instead).
   */
  stale: boolean;
  error: ReviewErrorCode | null;
  /**
   * Bumped whenever the selected session changes (or closes). Every reply
   * carries the epoch it was requested under and drops on mismatch, which is
   * what rejects the first A's reply in an A → B → A sequence.
   */
  artifactEpoch: number;
  /**
   * Lens artifacts. At most the ACTIVE lens's slot holds data, and only data
   * read in the same round as `snapshot` (see `executeRound`): publishing a
   * round resets every other slot, so no pane ever renders an artifact next
   * to a snapshot from a different read.
   */
  artifacts: ReviewArtifacts;
  /** Timeline click target for the Transcript lens (Review's own, not Live's). */
  transcriptSeekTarget: SeekTarget | null;
  /** Timeline badge target for the Graph lens (Review's own, not Live's). */
  graphEdgeFocus: EdgeFocus | null;
  seekTranscriptToSegment: (segmentId: string | null) => void;
  /**
   * Seek the transcript to the segment behind a timeline span. A span the
   * snapshot does not know means the snapshot is behind the timeline read:
   * refresh first, then seek, instead of seeking to a raw span id.
   */
  seekToSpan: (spanId: string, knownSegmentId: string | null) => Promise<void>;
  focusGraphEdges: (edgeIds: string[] | null) => void;
  /**
   * Open (or re-open) a session for the given lens (`null` = the Transcript
   * lens, which needs no artifact). Returns once its round settled or dropped.
   */
  select: (sessionId: string, lens?: ArtifactKind | null) => Promise<void>;
  /**
   * The lens whose artifact is needed changed. Starts a round (snapshot and
   * the new lens's artifact together); `null` just stops refreshing one.
   */
  activateLens: (lens: ArtifactKind | null) => Promise<void>;
  /** Re-query the snapshot and the active lens's artifact together. */
  refresh: () => Promise<void>;
  /** Drop the selection and invalidate any in-flight round. */
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

/** A failed artifact query as a closed slot: unavailable, or a typed refusal. */
function failedSlot(reason: unknown): ArtifactSlot<never> {
  const refusal = artifactTooLargeDetails(reason);
  return {
    status: refusal ? "refused" : "error",
    data: null,
    stale: false,
    refusal,
  };
}

/** A round in flight for one selection epoch, with at most one queued after it. */
interface InFlightRound {
  epoch: number;
  trailing: boolean;
  done: Promise<void>;
}

export const useReviewStore = create<ReviewState>((set, get) => {
  /** The lens whose artifact rounds fetch; `null` while no artifact lens is open. */
  let activeKind: ArtifactKind | null = null;
  let inFlight: InFlightRound | null = null;

  /**
   * One round: re-query the snapshot and the active lens's artifact TOGETHER
   * and publish both in ONE store update, only when both replies arrived and
   * the selection is unchanged. A round that fails in either half publishes
   * nothing from the other: the previous consistent pair stays on screen,
   * marked stale. (The two reads are still two backend calls; a real
   * guarantee needs a revision token in both replies.)
   */
  const executeRound = async (
    sessionId: string,
    epoch: number,
    kind: ArtifactKind | null,
  ): Promise<void> => {
    const isStale = () => get().artifactEpoch !== epoch;
    const before = get();
    const hadSnapshot = before.snapshot?.sessionId === sessionId;
    const previous = kind === null ? null : before.artifacts[kind];
    // A consistent pair already on screen: the snapshot and, if a lens is
    // active, its artifact from one earlier round.
    const hadPair =
      hadSnapshot && (previous === null || previous.status === "ready");

    set((state) => ({
      ...(hadSnapshot
        ? { refreshing: true }
        : { status: "loading" as const, error: null }),
      artifacts:
        kind !== null && previous !== null && previous.status !== "ready"
          ? {
              ...state.artifacts,
              [kind]: { ...previous, status: "loading", refusal: null },
            }
          : state.artifacts,
    }));

    // Async wrappers, so even a synchronous throw from `invoke` is a rejected
    // read (a failed half), never an exception out of the round.
    const readSnapshot = async (): Promise<ReviewSnapshot> =>
      toSnapshot(
        sessionId,
        await invoke<unknown>("load_session", { sessionId }),
      );
    const readArtifact = async (lens: ArtifactKind): Promise<unknown> => {
      const source = ARTIFACT_SOURCES[lens];
      return source.parse(
        await invoke<unknown>(source.command, source.args(sessionId)),
      );
    };
    const [snapshotRes, artifactRes] = await Promise.allSettled([
      readSnapshot(),
      kind === null ? Promise.resolve(null) : readArtifact(kind),
    ]);
    if (isStale()) return;

    const snapshotOk = snapshotRes.status === "fulfilled";
    const artifactOk = kind === null || artifactRes.status === "fulfilled";

    if (snapshotOk && artifactOk) {
      set({
        snapshot: snapshotRes.value,
        status: "ready",
        refreshing: false,
        stale: false,
        error: null,
        artifacts:
          kind === null
            ? idleArtifacts()
            : {
                ...idleArtifacts(),
                [kind]: {
                  status: "ready",
                  data: (artifactRes as PromiseFulfilledResult<unknown>).value,
                  stale: false,
                  refusal: null,
                },
              },
      });
      return;
    }

    if (hadPair) {
      // Keep the previous consistent pair; neither new half is published.
      set((state) => ({
        refreshing: false,
        stale: true,
        artifacts:
          kind === null
            ? state.artifacts
            : {
                ...state.artifacts,
                [kind]: { ...state.artifacts[kind], stale: true },
              },
      }));
      return;
    }

    // A first load of the snapshot or of this lens's artifact, with no
    // consistent pair to fall back on.
    const failedArtifact =
      kind === null
        ? null
        : failedSlot(
            artifactRes.status === "rejected" ? artifactRes.reason : null,
          );
    if (snapshotOk) {
      // The snapshot stands alone; only this lens is unavailable.
      set({
        snapshot: snapshotRes.value,
        status: "ready",
        refreshing: false,
        stale: false,
        error: null,
        artifacts:
          kind === null || failedArtifact === null
            ? idleArtifacts()
            : { ...idleArtifacts(), [kind]: failedArtifact },
      });
      return;
    }
    set((state) => ({
      ...(hadSnapshot
        ? { refreshing: false, stale: true }
        : {
            snapshot: null,
            status: "error" as const,
            refreshing: false,
            stale: false,
            error: "unavailable" as const,
          }),
      // An artifact read next to a failed snapshot is discarded, never shown.
      artifacts:
        kind === null || failedArtifact === null
          ? state.artifacts
          : { ...state.artifacts, [kind]: failedArtifact },
    }));
  };

  /**
   * Start a round, or coalesce into the one in flight for this selection:
   * never more than one round at a time, and at most ONE trailing round after
   * it (focus, visibility and quick lens switches all land here).
   */
  const requestRound = (): Promise<void> => {
    const { sessionId, artifactEpoch: epoch } = get();
    if (sessionId === null) return Promise.resolve();
    if (inFlight !== null && inFlight.epoch === epoch) {
      inFlight.trailing = true;
      return inFlight.done;
    }
    const round: InFlightRound = {
      epoch,
      trailing: false,
      done: Promise.resolve(),
    };
    inFlight = round;
    const kindUsed = activeKind;
    round.done = (async () => {
      try {
        await executeRound(sessionId, epoch, kindUsed);
      } finally {
        // Whatever happened (published, dropped as stale, or an unexpected
        // throw), this round is over: never leave the selection "in flight".
        if (inFlight === round) inFlight = null;
      }
      const sameSelection =
        get().artifactEpoch === epoch && get().sessionId === sessionId;
      // One more round when something asked for it while this one ran, or the
      // open lens changed to one this round did not fetch (a missed request
      // must never leave a lens loading forever).
      if (
        sameSelection &&
        (round.trailing || (activeKind !== null && activeKind !== kindUsed))
      ) {
        await requestRound();
      }
    })();
    return round.done;
  };

  return {
    sessionId: null,
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
    seekToSpan: async (spanId, knownSegmentId) => {
      if (knownSegmentId) {
        get().seekTranscriptToSegment(knownSegmentId);
        return;
      }
      const epoch = get().artifactEpoch;
      await get().refresh();
      // The selection moved on while refreshing: the click no longer applies.
      if (get().artifactEpoch !== epoch) return;
      const event = get().snapshot?.transcriptEvents.find(
        (candidate) => candidate.span_id === spanId,
      );
      get().seekTranscriptToSegment(
        event ? (event.transcript_segment_id ?? event.span_id) : spanId,
      );
    },
    focusGraphEdges: (edgeIds) =>
      set((state) => ({
        graphEdgeFocus:
          edgeIds && edgeIds.length > 0
            ? { edgeIds, nonce: (state.graphEdgeFocus?.nonce ?? 0) + 1 }
            : null,
      })),
    status: "idle",
    snapshot: null,
    refreshing: false,
    stale: false,
    error: null,
    select: (sessionId, lens = null) => {
      const sameSession = get().snapshot?.sessionId === sessionId;
      const changed = get().sessionId !== sessionId;
      activeKind = lens;
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
        status: sameSession ? ("ready" as const) : ("loading" as const),
        snapshot: sameSession ? get().snapshot : null,
        refreshing: sameSession,
        stale: sameSession ? get().stale : false,
        error: null,
      }));
      return requestRound();
    },
    activateLens: (lens) => {
      activeKind = lens;
      return lens === null ? Promise.resolve() : requestRound();
    },
    refresh: () => requestRound(),
    close: () => {
      activeKind = null;
      inFlight = null;
      set((state) => ({
        sessionId: null,
        status: "idle",
        snapshot: null,
        refreshing: false,
        stale: false,
        error: null,
        artifactEpoch: state.artifactEpoch + 1,
        artifacts: idleArtifacts(),
        transcriptSeekTarget: null,
        graphEdgeFocus: null,
      }));
    },
  };
});
