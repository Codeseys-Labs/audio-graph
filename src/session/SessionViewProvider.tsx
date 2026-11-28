/**
 * SessionViewProvider + useSessionView — SHELL-R1 shim (seed
 * audio-graph-59fb, parent audio-graph-19c7, plan §R1), extended for
 * Review-while-Live (issue #140, ADR-0028).
 *
 * Every session-scoped reader (`NotesPanel`, `LiveTranscript`,
 * `KnowledgeGraphViewer`, `SeekTimeline`) reads through this module instead of
 * straight off the global `useAudioGraphStore`, so a second, historical
 * session can be shown without ever being written into that store.
 *
 *   - Wrapped in a `SessionViewProvider` with no `reviewSessionId`: Live. The
 *     provider's own store reads (the same global-store selectors).
 *   - Not wrapped (a component test that mounts `NotesPanel` etc. in
 *     isolation): the hooks fall back to reading the global store directly.
 *   - Wrapped with a `reviewSessionId`: a Review subtree. Every read comes from
 *     the query-backed Review store (`reviewStore.ts`): the transcript and
 *     live-lane graph from the `load_session` snapshot, and Notes, Graph and
 *     Timeline from the open lens's artifact, read in the SAME round as the
 *     snapshot and published with it (`reviewLens`). The global selectors collapse to constants, so
 *     Live content events neither re-render nor reach the subtree, and the
 *     seek / edge-focus targets are Review's own.
 *
 * `useSessionView()` subscribes to the global store ONLY when no provider is
 * above it; under any provider it reads the provider's value alone.
 *
 * `useTranscriptView()`, `useTimelineView()` and `useSessionInteractions()` are
 * the transcript, timeline and cross-lens (seek / edge focus) reads.
 */
import { createContext, type ReactNode, useContext, useMemo } from "react";
import { useAudioGraphStore } from "../store";
import type {
  AsrPartialEvent,
  DiarizationSpanRevisionEvent,
  GraphSnapshot,
  MaterializedGraph,
  MaterializedNotes,
  ProjectionPatch,
  SpeakerInfo,
  TimelineEntry,
  TranscriptEvent,
  TranscriptSegment,
} from "../types";
import { type ArtifactKind, useReviewStore } from "./reviewStore";
import {
  type ReviewTranscriptView,
  useReviewTranscript,
} from "./useReviewTranscript";

export interface SessionView {
  transcriptSegments: TranscriptSegment[];
  graphSnapshot: GraphSnapshot;
  materializedNotes: MaterializedNotes | null;
  sessionTimeline: TimelineEntry[] | null;
  sessionProjectionEvents: ProjectionPatch[];
  /** The richer materialized graph, when this session has loaded one. */
  materializedProjectionGraph: MaterializedGraph | null;
}

const SessionViewContext = createContext<SessionView | null>(null);

/** Non-null only under a `SessionViewProvider` that has a Review selection. */
const ReviewViewContext = createContext<ReviewTranscriptView | null>(null);

// Stable empties: a Review pane must never fall through to Live data for a
// field Review does not hold, and stable identities keep memoised joins calm.
const EMPTY_SEGMENTS: TranscriptSegment[] = [];
const EMPTY_TRANSCRIPT_EVENTS: TranscriptEvent[] = [];
const EMPTY_DIARIZATION: DiarizationSpanRevisionEvent[] = [];
const EMPTY_SPEAKERS: SpeakerInfo[] = [];
const EMPTY_PROJECTION_EVENTS: ProjectionPatch[] = [];
const EMPTY_GRAPH: GraphSnapshot = {
  nodes: [],
  links: [],
  stats: { total_nodes: 0, total_edges: 0, total_episodes: 0 },
};

export function SessionViewProvider({
  children,
  reviewSessionId = null,
  reviewLens = null,
}: {
  children: ReactNode;
  /**
   * Historical session shown to the panes below. `null` (default) keeps the
   * Live behaviour: every read comes straight from the global store.
   */
  reviewSessionId?: string | null;
  /**
   * The artifact lens (Notes, Graph, Timeline) currently open below, whose
   * data is queried in the same round as the session snapshot; `null` for the
   * Transcript lens, which needs none.
   */
  reviewLens?: ArtifactKind | null;
}) {
  const reviewing = reviewSessionId !== null;
  const review = useReviewTranscript(reviewSessionId, reviewLens);
  // Under a Review selection these selectors return constants, so Live
  // content events never re-render (or leak into) the Review subtree.
  const transcriptSegments = useAudioGraphStore((s) =>
    reviewing ? EMPTY_SEGMENTS : s.transcriptSegments,
  );
  const graphSnapshot = useAudioGraphStore((s) =>
    reviewing ? EMPTY_GRAPH : s.graphSnapshot,
  );
  const materializedNotes = useAudioGraphStore((s) =>
    reviewing ? null : s.materializedNotes,
  );
  const sessionTimeline = useAudioGraphStore((s) =>
    reviewing ? null : s.sessionTimeline,
  );
  const sessionProjectionEvents = useAudioGraphStore((s) =>
    reviewing ? EMPTY_PROJECTION_EVENTS : s.sessionProjectionEvents,
  );
  const materializedProjectionGraph = useAudioGraphStore((s) =>
    reviewing ? null : s.materializedProjectionGraph,
  );
  // The Review store's artifacts, but only once they belong to THIS session
  // (a render can precede the layout effect that selects it).
  const artifacts = useReviewStore((s) =>
    reviewing && s.sessionId === reviewSessionId ? s.artifacts : null,
  );
  const reviewedTranscript = review?.snapshot?.transcript;
  const reviewedGraph = review?.snapshot?.graph;
  const value = useMemo<SessionView>(
    () =>
      reviewing
        ? {
            transcriptSegments: reviewedTranscript ?? EMPTY_SEGMENTS,
            graphSnapshot: reviewedGraph ?? EMPTY_GRAPH,
            materializedNotes: artifacts?.notes.data?.notes ?? null,
            sessionTimeline: artifacts?.timeline.data?.entries ?? null,
            sessionProjectionEvents:
              artifacts?.notes.data?.projection_events ??
              EMPTY_PROJECTION_EVENTS,
            materializedProjectionGraph: artifacts?.graph.data ?? null,
          }
        : {
            transcriptSegments,
            graphSnapshot,
            materializedNotes,
            sessionTimeline,
            sessionProjectionEvents,
            materializedProjectionGraph,
          },
    [
      reviewing,
      reviewedTranscript,
      reviewedGraph,
      artifacts,
      transcriptSegments,
      graphSnapshot,
      materializedNotes,
      sessionTimeline,
      sessionProjectionEvents,
      materializedProjectionGraph,
    ],
  );
  return (
    <SessionViewContext.Provider value={value}>
      <ReviewViewContext.Provider value={review}>
        {children}
      </ReviewViewContext.Provider>
    </SessionViewContext.Provider>
  );
}

/**
 * Session-scoped selector set. Calls the same global-store hooks on every
 * render (rules-of-hooks-safe), but their selectors collapse to constants as
 * soon as a `SessionViewProvider` sits above: the provider's value is then the
 * only source, so a consumer under a Review provider holds no Live
 * subscription and cannot re-render on Live content events. Without a
 * provider (an isolated component test) they read the global store directly.
 */
export function useSessionView(): SessionView {
  const ctx = useContext(SessionViewContext);
  const provided = ctx !== null;
  const transcriptSegments = useAudioGraphStore((s) =>
    provided ? EMPTY_SEGMENTS : s.transcriptSegments,
  );
  const graphSnapshot = useAudioGraphStore((s) =>
    provided ? EMPTY_GRAPH : s.graphSnapshot,
  );
  const materializedNotes = useAudioGraphStore((s) =>
    provided ? null : s.materializedNotes,
  );
  const sessionTimeline = useAudioGraphStore((s) =>
    provided ? null : s.sessionTimeline,
  );
  const sessionProjectionEvents = useAudioGraphStore((s) =>
    provided ? EMPTY_PROJECTION_EVENTS : s.sessionProjectionEvents,
  );
  const materializedProjectionGraph = useAudioGraphStore((s) =>
    provided ? null : s.materializedProjectionGraph,
  );
  return (
    ctx ?? {
      transcriptSegments,
      graphSnapshot,
      materializedNotes,
      sessionTimeline,
      sessionProjectionEvents,
      materializedProjectionGraph,
    }
  );
}

/** The reviewed session id below a Review provider; `null` for Live. */
export function useReviewSessionId(): string | null {
  return useContext(ReviewViewContext)?.sessionId ?? null;
}

/** The Review snapshot's load state below a Review provider; `null` for Live. */
export function useReviewView(): ReviewTranscriptView | null {
  return useContext(ReviewViewContext);
}

/** What a transcript pane renders: Live from the global store, or Review. */
export interface TranscriptView {
  /** Reviewed session id, or `null` when this pane shows the Live session. */
  reviewSessionId: string | null;
  /** Review load state; `null` for Live. */
  review: ReviewTranscriptView | null;
  segments: TranscriptSegment[];
  transcriptEvents: TranscriptEvent[];
  /** Speaker-timeline ledger joined onto `segments` for attribution. */
  diarizationSpanRevisions: DiarizationSpanRevisionEvent[];
  speakers: SpeakerInfo[];
  asrPartial: AsrPartialEvent | null;
}

/**
 * Transcript + speaker reads for `LiveTranscript`. Under a Review selection
 * every field comes from the Review snapshot (speakers and the in-flight ASR
 * partial are Live-only and empty here); the global-store selectors collapse
 * to constants so Live mutations cannot re-render or alter a Review pane.
 * Without a provider (or with no Review selection) it reads the global store
 * exactly as before.
 */
export function useTranscriptView(): TranscriptView {
  const review = useContext(ReviewViewContext);
  const reviewing = review !== null;
  const transcriptSegments = useAudioGraphStore((s) =>
    reviewing ? EMPTY_SEGMENTS : s.transcriptSegments,
  );
  const sessionTranscriptEvents = useAudioGraphStore((s) =>
    reviewing ? EMPTY_TRANSCRIPT_EVENTS : s.sessionTranscriptEvents,
  );
  const diarizationSpanRevisions = useAudioGraphStore((s) =>
    reviewing ? EMPTY_DIARIZATION : s.diarizationSpanRevisions,
  );
  const speakers = useAudioGraphStore((s) =>
    reviewing ? EMPTY_SPEAKERS : s.speakers,
  );
  const asrPartial = useAudioGraphStore((s) =>
    reviewing ? null : s.asrPartial,
  );
  const snapshot = review?.snapshot ?? null;
  return {
    reviewSessionId: review?.sessionId ?? null,
    review,
    segments: reviewing
      ? (snapshot?.transcript ?? EMPTY_SEGMENTS)
      : transcriptSegments,
    transcriptEvents: reviewing
      ? (snapshot?.transcriptEvents ?? EMPTY_TRANSCRIPT_EVENTS)
      : sessionTranscriptEvents,
    diarizationSpanRevisions: reviewing
      ? (snapshot?.diarizationEvents ?? EMPTY_DIARIZATION)
      : diarizationSpanRevisions,
    speakers,
    asrPartial,
  };
}

/** What the seek-timeline renders. */
export interface TimelineView {
  timeline: TimelineEntry[] | null;
  /** Fold length before the backend's tail cap; `null` when not known. */
  totalCount: number | null;
  loading: boolean;
  transcriptEvents: TranscriptEvent[];
  speakers: SpeakerInfo[];
}

/**
 * Timeline reads. Review: the timeline fold slot (a first query is `loading`)
 * and the snapshot's transcript events; speakers are Live-only and empty.
 */
export function useTimelineView(): TimelineView {
  const review = useContext(ReviewViewContext);
  const reviewing = review !== null;
  // The snapshot and the timeline slot are read from the store in the same
  // render, so they are always the pair one round published.
  const slot = useReviewStore((s) =>
    reviewing && s.sessionId === review.sessionId ? s.artifacts.timeline : null,
  );
  const reviewSnapshot = useReviewStore((s) =>
    reviewing && s.sessionId === review.sessionId ? s.snapshot : null,
  );
  const timeline = useAudioGraphStore((s) =>
    reviewing ? null : s.sessionTimeline,
  );
  const totalCount = useAudioGraphStore((s) =>
    reviewing ? null : s.sessionTimelineTotalCount,
  );
  const loading = useAudioGraphStore((s) =>
    reviewing ? false : s.sessionTimelineLoading,
  );
  const transcriptEvents = useAudioGraphStore((s) =>
    reviewing ? EMPTY_TRANSCRIPT_EVENTS : s.sessionTranscriptEvents,
  );
  const speakers = useAudioGraphStore((s) =>
    reviewing ? EMPTY_SPEAKERS : s.speakers,
  );
  if (!reviewing) {
    return { timeline, totalCount, loading, transcriptEvents, speakers };
  }
  return {
    timeline: slot?.data?.entries ?? null,
    totalCount: slot?.data?.total_count ?? null,
    loading: slot === null || slot.status === "loading",
    transcriptEvents:
      reviewSnapshot?.transcriptEvents ?? EMPTY_TRANSCRIPT_EVENTS,
    speakers,
  };
}

/** Cross-lens targets: the Timeline lens drives Transcript and Graph. */
export interface SessionInteractions {
  transcriptSeekTarget: { segmentId: string; nonce: number } | null;
  seekTranscriptToSegment: (segmentId: string | null) => void;
  /**
   * Seek to the segment behind a timeline span; `knownSegmentId` is what the
   * caller's transcript events resolve it to (or `null`). Review refreshes its
   * snapshot first when the span is unknown to it.
   */
  seekToSpan: (spanId: string, knownSegmentId: string | null) => void;
  graphEdgeFocus: { edgeIds: string[]; nonce: number } | null;
  focusGraphEdges: (edgeIds: string[] | null) => void;
}

/**
 * Seek and edge-focus targets. A Review subtree has its own (in the Review
 * store), so a click in a reviewed session's Timeline never writes a Live
 * field, and a Review pane holds no subscription to the Live targets.
 */
export function useSessionInteractions(): SessionInteractions {
  const reviewing = useContext(ReviewViewContext) !== null;
  const liveSeekTarget = useAudioGraphStore((s) =>
    reviewing ? null : s.transcriptSeekTarget,
  );
  const liveSeek = useAudioGraphStore((s) => s.seekTranscriptToSegment);
  const liveEdgeFocus = useAudioGraphStore((s) =>
    reviewing ? null : s.graphEdgeFocus,
  );
  const liveFocus = useAudioGraphStore((s) => s.focusGraphEdges);
  const reviewSeekTarget = useReviewStore((s) =>
    reviewing ? s.transcriptSeekTarget : null,
  );
  const reviewSeek = useReviewStore((s) => s.seekTranscriptToSegment);
  const reviewSeekToSpan = useReviewStore((s) => s.seekToSpan);
  const reviewEdgeFocus = useReviewStore((s) =>
    reviewing ? s.graphEdgeFocus : null,
  );
  const reviewFocus = useReviewStore((s) => s.focusGraphEdges);
  return reviewing
    ? {
        transcriptSeekTarget: reviewSeekTarget,
        seekTranscriptToSegment: reviewSeek,
        seekToSpan: (spanId, known) => void reviewSeekToSpan(spanId, known),
        graphEdgeFocus: reviewEdgeFocus,
        focusGraphEdges: reviewFocus,
      }
    : {
        transcriptSeekTarget: liveSeekTarget,
        seekTranscriptToSegment: liveSeek,
        seekToSpan: (spanId, known) => liveSeek(known ?? spanId),
        graphEdgeFocus: liveEdgeFocus,
        focusGraphEdges: liveFocus,
      };
}
