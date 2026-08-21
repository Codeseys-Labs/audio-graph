/**
 * SessionViewProvider + useSessionView — SHELL-R1 shim (seed
 * audio-graph-59fb, parent audio-graph-19c7, plan §R1).
 *
 * Today every session-scoped reader (`NotesPanel`, `LiveTranscript`,
 * `KnowledgeGraphViewer`, `SeekTimeline`) reads straight off the global
 * `useAudioGraphStore`: `transcriptSegments`, `graphSnapshot`,
 * `materializedNotes`, `sessionTimeline`, `sessionProjectionEvents`. That is
 * correct because the store holds exactly one live/loaded session's worth of
 * this data at a time — there is no per-session isolation
 * (`store/index.ts:2100`'s note on per-session store isolation is explicitly
 * OUT of scope for this unit).
 *
 * This module exists purely as the seam that lets that isolation land later
 * without reopening every consumer. `useSessionView()` returns the same five
 * fields either way:
 *   - Wrapped in a `SessionViewProvider`: the provider's own store reads
 *     (today: the same global-store selectors).
 *   - Not wrapped (e.g. a component test that mounts `NotesPanel` etc. in
 *     isolation, as several already do): falls back to reading the global
 *     store directly, so no test needs to add a wrapper for this to keep
 *     working.
 * Both paths read the identical live values today — this is a genuine
 * zero-behavior-change shim, not a new isolation mechanism. When per-session
 * view isolation lands in the store, `SessionViewProvider` starts accepting
 * a `sessionId` prop and threading session-scoped selectors instead.
 *
 * Two known deviations from "decoupled by construction", left as-is for
 * this contract-neutral unit and flagged for whoever wires real isolation:
 * `useSessionView()` unconditionally calls the five global-store selectors
 * on every render (the fallback path), so a wrapped consumer still holds a
 * live global subscription underneath the context read — the fields can't
 * be removed from the global store without touching this hook first. And
 * the provider builds a fresh `value` object every render (no `useMemo`),
 * so every consumer under it re-renders on every provider render. Neither
 * has a behavioral effect today (no consumer below is `memo()`-wrapped —
 * `NotesPanel`, `LiveTranscript`, `KnowledgeGraphViewer`, `SeekTimeline`),
 * but both should be tightened before anything relies on memoization here.
 */
import { createContext, type ReactNode, useContext } from "react";
import { useAudioGraphStore } from "../store";
import type {
  GraphSnapshot,
  MaterializedNotes,
  ProjectionPatch,
  TimelineEntry,
  TranscriptSegment,
} from "../types";

export interface SessionView {
  transcriptSegments: TranscriptSegment[];
  graphSnapshot: GraphSnapshot;
  materializedNotes: MaterializedNotes | null;
  sessionTimeline: TimelineEntry[] | null;
  sessionProjectionEvents: ProjectionPatch[];
}

const SessionViewContext = createContext<SessionView | null>(null);

export function SessionViewProvider({ children }: { children: ReactNode }) {
  const transcriptSegments = useAudioGraphStore((s) => s.transcriptSegments);
  const graphSnapshot = useAudioGraphStore((s) => s.graphSnapshot);
  const materializedNotes = useAudioGraphStore((s) => s.materializedNotes);
  const sessionTimeline = useAudioGraphStore((s) => s.sessionTimeline);
  const sessionProjectionEvents = useAudioGraphStore(
    (s) => s.sessionProjectionEvents,
  );
  const value: SessionView = {
    transcriptSegments,
    graphSnapshot,
    materializedNotes,
    sessionTimeline,
    sessionProjectionEvents,
  };
  return (
    <SessionViewContext.Provider value={value}>
      {children}
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
