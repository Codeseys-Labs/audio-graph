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
 * Errors are a CLOSED set (`ReviewErrorCode`): the backend's message is never
 * stored or rendered, because it can embed paths and artifact names.
 */
import { create } from "zustand";
import { safeInvoke as invoke } from "../analytics/safeInvoke";
import type {
  DiarizationSpanRevisionEvent,
  LoadedSession,
  TranscriptEvent,
  TranscriptSegment,
} from "../types";

/** Transcript + persisted speaker state for one reviewed session. */
export interface ReviewSnapshot {
  sessionId: string;
  transcript: TranscriptSegment[];
  transcriptEvents: TranscriptEvent[];
  /** Persisted diarization span revisions (the speaker timeline ledger). */
  diarizationEvents: DiarizationSpanRevisionEvent[];
  /** Wall-clock ms the snapshot was read, for "refreshed" diagnostics. */
  loadedAtMs: number;
}

/** Closed error vocabulary. `unavailable` = missing, unreadable, or corrupt. */
export type ReviewErrorCode = "unavailable";

export type ReviewStatus = "idle" | "loading" | "ready" | "error";

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
  error: ReviewErrorCode | null;
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
  return {
    sessionId,
    transcript: payload.transcript,
    transcriptEvents,
    diarizationEvents,
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
        error: null,
      });
    } catch {
      if (isStale()) return;
      // A failed refresh keeps the last good snapshot visible; only a first
      // load (no snapshot for this session yet) becomes the error state.
      if (get().snapshot?.sessionId === sessionId) {
        set({ refreshing: false });
        return;
      }
      set({
        snapshot: null,
        status: "error",
        refreshing: false,
        error: "unavailable",
      });
    }
  };

  return {
    sessionId: null,
    generation: 0,
    status: "idle",
    snapshot: null,
    refreshing: false,
    error: null,
    select: (sessionId) => {
      const sameSession = get().snapshot?.sessionId === sessionId;
      set({
        sessionId,
        // Re-selecting the session already shown re-queries in place; any
        // other session starts from a clean loading state, never showing the
        // previous session's transcript under the new title.
        status: sameSession ? "ready" : "loading",
        snapshot: sameSession ? get().snapshot : null,
        refreshing: sameSession,
        error: null,
      });
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
        error: null,
      })),
  };
});
