/**
 * Review store — the query-backed snapshot of ONE historical session
 * (issue #140, ADR-0028 frontend half).
 *
 * Live stays in the global `useAudioGraphStore`, owned by the backend's one
 * live session. Review is deliberately a separate store with no subscription
 * to live content events: it calls the read-only `load_session` command and the
 * per-lens artifact commands directly and holds the results here, so opening a
 * past session while capture runs can never write a Live field
 * (`transcriptSegments`, `speakers`, `diarizationSpanRevisions`, …) or rotate
 * the live session. Seek and edge-focus targets (the Timeline lens driving the
 * Transcript and Graph lenses) live here too, for the same reason.
 *
 * Review is refreshable, not immutable: a reviewed session that is still
 * finalizing changes on disk (#169, ADR-0036). `useReviewTranscript` (the one
 * owner of fetching) opens the session and re-queries on window focus and tab
 * visibility; the rules below say what a query does. They are written once, at
 * `executeRound`, and are the whole story: there is no per-kind sequencing and
 * no generation counter beyond them. In short:
 *
 *   - the unit of reading is a ROUND: the snapshot and the open lens's artifact
 *     (Notes, Graph or Timeline; the Transcript lens needs none), together;
 *   - a round belongs to (selection epoch, lens open at its start) and is
 *     dropped when the selection changes (the A, B, A case) or an explicit user
 *     action supersedes it; passive triggers coalesce instead;
 *   - a pair is replaced only when both reads succeed; a failure keeps the
 *     previous pair, marked stale, and records itself on the failing lens;
 *   - a timeline seek for a span the snapshot does not know stays pending until
 *     a published snapshot maps it to a segment that exists.
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

/** Statuses admitted at the read boundary of one historical lens round. */
export type ReviewProjectionStatus = "behind_transcript" | "recompute_required";
type ReviewProjectionInfo = {
  reviewProjectionStatuses?: ReviewProjectionStatus[];
};
type ReviewNotesArtifacts = Omit<SessionNotesArtifacts, "notes"> & {
  notes:
    | (NonNullable<SessionNotesArtifacts["notes"]> & ReviewProjectionInfo)
    | null;
};
type ReviewGraphArtifact = MaterializedGraph & ReviewProjectionInfo;

export interface ReviewArtifacts {
  notes: ArtifactSlot<ReviewNotesArtifacts>;
  /** `data: null` with `status: "ready"` means the session has no graph. */
  graph: ArtifactSlot<ReviewGraphArtifact | null>;
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

function parseProjectionStatus(raw: Record<string, unknown>) {
  const {
    projection_recovery,
    transcript_coverage,
    reviewProjectionStatuses: _incoming,
    ...materialized
  } = raw;
  const statuses: ReviewProjectionStatus[] = [];
  if (transcript_coverage === "behind_transcript")
    statuses.push(transcript_coverage);
  if (projection_recovery === "recompute_required")
    statuses.push(projection_recovery);
  return statuses.length > 0
    ? { ...materialized, reviewProjectionStatuses: statuses }
    : materialized;
}

function parseNotes(raw: unknown): ReviewNotesArtifacts {
  if (!isRecord(raw) || !Array.isArray(raw.projection_events)) {
    throw new Error("malformed notes artifact");
  }
  if (raw.notes !== null && raw.notes !== undefined && !isRecord(raw.notes)) {
    throw new Error("malformed notes artifact");
  }
  return {
    notes: isRecord(raw.notes)
      ? (parseProjectionStatus(
          raw.notes,
        ) as unknown as ReviewNotesArtifacts["notes"])
      : null,
    projection_events:
      raw.projection_events as SessionNotesArtifacts["projection_events"],
  };
}

function parseGraph(raw: unknown): ReviewGraphArtifact | null {
  if (raw === null || raw === undefined) return null;
  if (!isRecord(raw) || !Array.isArray(raw.nodes)) {
    throw new Error("malformed graph artifact");
  }
  return parseProjectionStatus(raw) as unknown as ReviewGraphArtifact;
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
   * The open lens changed (an explicit user action). Supersedes the round in
   * flight and starts one for the lens now open: the snapshot and its artifact
   * together, or the snapshot alone for the Transcript lens (`null`).
   */
  activateLens: (lens: ArtifactKind | null) => Promise<void>;
  /**
   * Re-query the snapshot and the open lens's artifact together. Passive
   * callers (focus, visibility) coalesce; `{ explicit: true }` (a retry click,
   * a timeline click needing fresh data) supersedes a round in flight.
   */
  refresh: (options?: { explicit?: boolean }) => Promise<void>;
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
  /**
   * An explicit user action (a lens switch or a retry) replaced this round:
   * its replies are dropped and it queues nothing behind it.
   */
  superseded: boolean;
  /** The lens whose artifact this round reads (`null` = Transcript). */
  kind: ArtifactKind | null;
  /**
   * What that lens's slot was before this round marked it `loading`, so a
   * round that is replaced can hand the slot back instead of leaving it loading.
   */
  settledSlot: ArtifactSlot<never> | null;
  done: Promise<void>;
}

const KINDS: readonly ArtifactKind[] = ["notes", "graph", "timeline"];

export const useReviewStore = create<ReviewState>((set, get) => {
  /** The lens whose artifact rounds fetch; `null` while no artifact lens is open. */
  let activeKind: ArtifactKind | null = null;
  let inFlight: InFlightRound | null = null;
  /** Monotonic id of the latest seek request (see `seekToSpan`). */
  let seekSeq = 0;
  /**
   * The one timeline seek still waiting for a snapshot that knows its span.
   * Replaced by any newer seek, dropped on a selection change, and resolved by
   * the next round that publishes a snapshot containing the span.
   */
  let pendingSeek: { spanId: string; request: number } | null = null;
  /**
   * The segment behind `spanId` in the published snapshot, or `null` while the
   * snapshot cannot say. The transcript is derived from the LATEST revision of
   * each span (highest `revision_number`; the first of two equal revisions
   * stands), and the events carry the whole revision history, so the same rule
   * picks the span's event here. The id is a target only if it is a segment
   * that exists in the snapshot's transcript.
   */
  const segmentForSpan = (spanId: string): string | null => {
    const snapshot = get().snapshot;
    if (snapshot === null) return null;
    let latest: TranscriptEvent | null = null;
    for (const event of snapshot.transcriptEvents) {
      if (event.span_id !== spanId) continue;
      if (latest === null || event.revision_number > latest.revision_number) {
        latest = event;
      }
    }
    if (latest === null) return null;
    const segmentId = latest.transcript_segment_id ?? latest.span_id;
    return snapshot.transcript.some((segment) => segment.id === segmentId)
      ? segmentId
      : null;
  };
  const resolvePendingSeek = (): void => {
    const pending = pendingSeek;
    if (pending === null) return;
    if (pending.request !== seekSeq) {
      pendingSeek = null; // a newer seek replaced it
      return;
    }
    const segmentId = segmentForSpan(pending.spanId);
    if (segmentId === null) return; // still unknown: stays pending
    pendingSeek = null;
    applySeek(segmentId);
  };
  const applySeek = (segmentId: string | null): void =>
    set((state) => ({
      transcriptSeekTarget: segmentId
        ? { segmentId, nonce: (state.transcriptSeekTarget?.nonce ?? 0) + 1 }
        : null,
    }));

  /**
   * THE ROUND RULES (everything below follows from these).
   *
   *  1. A round is the unit of reading: the snapshot and the artifact of the
   *     lens that was open when it started (`null` = the Transcript lens: the
   *     snapshot alone), queried together. The two are still two backend calls;
   *     a real guarantee needs a revision token in both replies.
   *  2. A round belongs to (selection epoch, that lens). Its replies are
   *     dropped when the selection changed or an explicit user action (a lens
   *     switch, a retry click, a timeline click needing fresh data) superseded
   *     it. Passive triggers (window focus, tab visibility) coalesce instead.
   *     So a lens that is no longer open can neither mark the open lens's data
   *     stale nor block it.
   *  3. The pair (snapshot + that lens's artifact) is REPLACED only when both
   *     reads of the round succeed, in one store update that also resets every
   *     other lens's slot (they were read against an older snapshot).
   *  4. When either read fails and a previous pair exists (a ready artifact, or
   *     a Transcript round over a snapshot), that pair stays untouched and is
   *     marked stale; a lens with no data of its own records the failure on
   *     ITS slot (closed error or typed refusal, with retry), so going back to
   *     the lens that has data still shows it.
   *  5. With no previous pair at all, a snapshot that succeeds may stand alone
   *     (its lens's artifact failure is recorded on that lens's slot), and a
   *     first snapshot failure is the closed `error` state.
   */
  const executeRound = async (
    round: InFlightRound,
    sessionId: string,
    kind: ArtifactKind | null,
  ): Promise<void> => {
    const isStale = () =>
      get().artifactEpoch !== round.epoch || round.superseded;
    const before = get();
    const hadSnapshot = before.snapshot?.sessionId === sessionId;
    const previous = kind === null ? null : before.artifacts[kind];
    round.settledSlot = previous as ArtifactSlot<never> | null;
    const anyReady = KINDS.some((k) => before.artifacts[k].status === "ready");
    // A consistent pair on screen: the snapshot with a ready artifact, or (for
    // a Transcript round) just the snapshot.
    const hadPair = hadSnapshot && (kind === null || anyReady);

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

    // Rule 3: both reads succeeded: replace the pair.
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
      resolvePendingSeek();
      return;
    }

    // What this lens's slot records when its read failed, or when its read
    // succeeded but could not be shown (the snapshot failed): closed, never
    // backend text.
    const lensFailure =
      kind === null
        ? null
        : failedSlot(
            artifactRes.status === "rejected" ? artifactRes.reason : null,
          );

    // Rule 4: keep the previous pair, marked stale.
    if (hadPair) {
      set((state) => {
        const stale = <T>(slot: ArtifactSlot<T>): ArtifactSlot<T> =>
          slot.status === "ready" ? { ...slot, stale: true } : slot;
        const marked: ReviewArtifacts = {
          notes: stale(state.artifacts.notes),
          graph: stale(state.artifacts.graph),
          timeline: stale(state.artifacts.timeline),
        };
        // The open lens has no data of its own: it records the failure.
        const artifacts =
          kind !== null &&
          lensFailure !== null &&
          state.artifacts[kind].status !== "ready"
            ? { ...marked, [kind]: lensFailure }
            : marked;
        return { refreshing: false, stale: true, artifacts };
      });
      return;
    }

    // Rule 5: no previous pair.
    if (snapshotOk) {
      set({
        snapshot: snapshotRes.value,
        status: "ready",
        refreshing: false,
        stale: false,
        error: null,
        artifacts:
          kind === null || lensFailure === null
            ? idleArtifacts()
            : { ...idleArtifacts(), [kind]: lensFailure },
      });
      resolvePendingSeek();
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
        kind === null || lensFailure === null
          ? state.artifacts
          : { ...state.artifacts, [kind]: lensFailure },
    }));
  };

  /** Would `lens` show data that is missing or stale right now? */
  const lensNeedsRound = (lens: ArtifactKind | null): boolean => {
    const state = get();
    if (state.sessionId === null) return false;
    if (state.snapshot === null || state.stale) return true;
    if (lens === null) return false;
    const slot = state.artifacts[lens];
    return slot.status !== "ready" || slot.stale;
  };

  /**
   * A superseded round marked its lens's slot `loading`; if that lens is no
   * longer the one a replacement reads, hand the slot back to what it was
   * (idle, or its last good data) instead of leaving it loading.
   */
  const releaseAbandonedLens = (
    old: InFlightRound,
    replacementKind: ArtifactKind | null,
  ): void => {
    const lens = old.kind;
    if (lens === null || lens === replacementKind || old.settledSlot === null) {
      return;
    }
    const settled = old.settledSlot;
    set((state) =>
      state.artifacts[lens].status === "loading"
        ? { artifacts: { ...state.artifacts, [lens]: settled } }
        : {},
    );
  };

  /**
   * Start a round for the lens that is open now.
   *
   * Passive triggers (window focus, tab visibility) coalesce into the round in
   * flight for this selection: never more than one at a time, and at most ONE
   * trailing round after it. An EXPLICIT user action (a lens switch or a retry
   * click) instead supersedes the round in flight and starts a new one: the
   * replies of the superseded round are dropped, and a read that never settles
   * cannot block anything for the lens now open.
   */
  const requestRound = (explicit = false): Promise<void> => {
    const { sessionId, artifactEpoch: epoch } = get();
    if (sessionId === null) return Promise.resolve();
    if (inFlight !== null && inFlight.epoch === epoch) {
      if (!explicit) {
        inFlight.trailing = true;
        return inFlight.done;
      }
      inFlight.superseded = true;
      releaseAbandonedLens(inFlight, activeKind);
    }
    const round: InFlightRound = {
      epoch,
      trailing: false,
      superseded: false,
      kind: activeKind,
      settledSlot: null,
      done: Promise.resolve(),
    };
    inFlight = round;
    const kindUsed = activeKind;
    round.done = (async () => {
      try {
        await executeRound(round, sessionId, kindUsed);
      } finally {
        // Whatever happened (published, dropped as stale, or an unexpected
        // throw), this round is over: never leave the selection "in flight".
        if (inFlight === round) inFlight = null;
      }
      const sameSelection =
        get().artifactEpoch === epoch && get().sessionId === sessionId;
      // One more round when something asked for it while this one ran, or the
      // open lens changed to one this round did not fetch (a missed request
      // must never leave a lens loading forever). Never after a round an
      // explicit action already replaced.
      if (
        sameSelection &&
        !round.superseded &&
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
    seekTranscriptToSegment: (segmentId) => {
      // Any seek supersedes a deferred one still waiting on a refresh.
      seekSeq += 1;
      pendingSeek = null;
      applySeek(segmentId);
    },
    seekToSpan: async (spanId, knownSegmentId) => {
      // Each seek request has its own monotonic id: a deferred seek applies
      // only if it is still the latest one for this selection, so an older
      // click can never overwrite a newer one.
      const request = ++seekSeq;
      pendingSeek = null;
      if (knownSegmentId) {
        applySeek(knownSegmentId);
        return;
      }
      // Unknown to the snapshot: the snapshot is behind the timeline read. Keep
      // the seek PENDING and refresh; the round that publishes a snapshot with
      // the span resolves it (now, or on a later retry). Until then the target
      // is left alone: an unresolved span id is never a segment target.
      pendingSeek = { spanId, request };
      await get().refresh({ explicit: true });
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
      if (changed) {
        seekSeq += 1;
        pendingSeek = null;
      }
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
      // With a round in flight it belongs to the lens just left: replace it.
      // With none in flight, a round starts only if what the lens now open
      // would show is missing or stale (Transcript: the snapshot alone); a
      // lens whose data is fresh starts nothing.
      if (inFlight === null && !lensNeedsRound(lens)) return Promise.resolve();
      return requestRound(true);
    },
    refresh: (options) => requestRound(options?.explicit === true),
    close: () => {
      activeKind = null;
      inFlight = null;
      seekSeq += 1;
      pendingSeek = null;
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
