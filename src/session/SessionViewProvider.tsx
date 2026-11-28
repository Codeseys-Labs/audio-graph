/**
 * SessionViewProvider + useSessionView — SHELL-R1 shim (seed
 * audio-graph-59fb, parent audio-graph-19c7, plan §R1), extended for
 * Review-while-Live (issue #140, ADR-0028).
 *
 * Today every session-scoped reader (`NotesPanel`, `LiveTranscript`,
 * `KnowledgeGraphViewer`, `SeekTimeline`) reads straight off the global
 * `useAudioGraphStore`: `transcriptSegments`, `graphSnapshot`,
 * `materializedNotes`, `sessionTimeline`, `sessionProjectionEvents`. That is
 * correct for Live because the store holds exactly one live/loaded session's
 * worth of this data at a time.
 *
 * This module is the seam that keeps a second, historical session out of
 * that store. `useSessionView()` returns the same five fields either way:
 *   - Wrapped in a `SessionViewProvider` with no `reviewSessionId`: the
 *     provider's own store reads (the same global-store selectors).
 *   - Not wrapped (e.g. a component test that mounts `NotesPanel` etc. in
 *     isolation, as several already do): falls back to reading the global
 *     store directly, so no test needs to add a wrapper for this to keep
 *     working.
 *   - Wrapped with a `reviewSessionId`: a Review subtree. Transcript reads
 *     come from the query-backed Review store (`reviewStore.ts`); every
 *     field Review does not hold (graph, notes, timeline, projection
 *     events) is EMPTY rather than falling through to Live data, and the
 *     provider's global selectors collapse to constants so Live content
 *     events neither re-render nor reach the subtree. Lenses that are not
 *     migrated yet stay gated by their caller (`SessionsBrowser`).
 *
 * `useTranscriptView()` is the transcript pane's read: the segments plus the
 * speaker state (`speakers`, the diarization ledger, transcript events) that
 * `LiveTranscript` used to pull from the global store directly.
 *
 * Known deviation, left as-is: `useSessionView()` unconditionally calls the
 * five global-store selectors on every render (the fallback path), so a
 * wrapped consumer still holds a live global subscription underneath the
 * context read. No Review pane reads `useSessionView()` directly today
 * (`LiveTranscript` uses `useTranscriptView()`, whose selectors collapse
 * under Review); the Notes/Graph migration (#140 slice 2) must tighten it.
 */
import { createContext, type ReactNode, useContext, useMemo } from "react";
import { useAudioGraphStore } from "../store";
import type {
  AsrPartialEvent,
  DiarizationSpanRevisionEvent,
  GraphSnapshot,
  MaterializedNotes,
  ProjectionPatch,
  SpeakerInfo,
  TimelineEntry,
  TranscriptEvent,
  TranscriptSegment,
} from "../types";
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
}: {
  children: ReactNode;
  /**
   * Historical session shown to the panes below. `null` (default) keeps the
   * Live behaviour: every read comes straight from the global store.
   */
  reviewSessionId?: string | null;
}) {
  const reviewing = reviewSessionId !== null;
  const review = useReviewTranscript(reviewSessionId);
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
  const reviewedTranscript = review?.snapshot?.transcript;
  const value = useMemo<SessionView>(
    () => ({
      transcriptSegments: reviewing
        ? (reviewedTranscript ?? EMPTY_SEGMENTS)
        : transcriptSegments,
      graphSnapshot,
      materializedNotes,
      sessionTimeline,
      sessionProjectionEvents,
    }),
    [
      reviewing,
      reviewedTranscript,
      transcriptSegments,
      graphSnapshot,
      materializedNotes,
      sessionTimeline,
      sessionProjectionEvents,
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
 * Session-scoped selector set. Always calls the same global-store hooks
 * (rules-of-hooks-safe — no conditional hook calls) so it behaves
 * identically whether or not a `SessionViewProvider` is mounted above it;
 * see the module doc for why that is a real invariant this unit needs, not
 * an accident of the current implementation.
 */
export function useSessionView(): SessionView {
  const transcriptSegments = useAudioGraphStore((s) => s.transcriptSegments);
  const graphSnapshot = useAudioGraphStore((s) => s.graphSnapshot);
  const materializedNotes = useAudioGraphStore((s) => s.materializedNotes);
  const sessionTimeline = useAudioGraphStore((s) => s.sessionTimeline);
  const sessionProjectionEvents = useAudioGraphStore(
    (s) => s.sessionProjectionEvents,
  );
  const ctx = useContext(SessionViewContext);
  return (
    ctx ?? {
      transcriptSegments,
      graphSnapshot,
      materializedNotes,
      sessionTimeline,
      sessionProjectionEvents,
    }
  );
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
