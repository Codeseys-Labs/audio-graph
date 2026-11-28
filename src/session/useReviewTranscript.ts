/**
 * `useReviewTranscript` — binds a mounted Review surface to `reviewStore`.
 *
 * Opens the given session on mount / id change, re-queries on window focus and
 * when the tab becomes visible again (Review is refreshable, not immutable —
 * see `reviewStore.ts`), and releases the selection on unmount so a late
 * response cannot land after the surface is gone. `lens` names the artifact
 * lens that is open (Notes, Graph, Timeline), whose data is read in the same
 * round as the snapshot; `null` is the Transcript lens.
 *
 * Passing `null` means "no Review selection": the hook is inert, which lets
 * callers invoke it unconditionally (rules of hooks) and fall back to Live.
 *
 * It only ever reads `reviewStore`; nothing here touches the Live store.
 */
import { useCallback, useLayoutEffect, useMemo, useRef } from "react";
import {
  type ArtifactKind,
  type ReviewErrorCode,
  type ReviewSnapshot,
  type ReviewStatus,
  useReviewStore,
} from "./reviewStore";

export interface ReviewTranscriptView {
  /** The reviewed session id this view answers for. */
  sessionId: string;
  status: Exclude<ReviewStatus, "idle">;
  /** Present only when it belongs to `sessionId`, never a previous session. */
  snapshot: ReviewSnapshot | null;
  /** A focus/lifecycle re-query is running over the shown snapshot. */
  refreshing: boolean;
  /** The last re-query failed: `snapshot` was valid when read, may be old. */
  stale: boolean;
  error: ReviewErrorCode | null;
  /** Re-run the query (retry after an error, or manual refresh). */
  retry: () => void;
}

export function useReviewTranscript(
  sessionId: string | null,
  lens: ArtifactKind | null = null,
): ReviewTranscriptView | null {
  const select = useReviewStore((s) => s.select);
  const activateLens = useReviewStore((s) => s.activateLens);
  const refresh = useReviewStore((s) => s.refresh);
  const close = useReviewStore((s) => s.close);
  const storeSessionId = useReviewStore((s) => s.sessionId);
  const status = useReviewStore((s) => s.status);
  const snapshot = useReviewStore((s) => s.snapshot);
  const refreshing = useReviewStore((s) => s.refreshing);
  const stale = useReviewStore((s) => s.stale);
  const error = useReviewStore((s) => s.error);

  // The lens whose artifact the selection's first round fetches, and the lens
  // the store was last told about (so mounting does not start a second round).
  const lensRef = useRef(lens);
  lensRef.current = lens;
  const appliedLens = useRef<ArtifactKind | null>(lens);

  // The ONE owner of Review fetching and refreshing: open the session (its
  // snapshot and the active lens's artifact in a single round), then re-query
  // on window focus and tab visibility. The store coalesces overlapping
  // requests, so none of these can pile up IPC calls.
  useLayoutEffect(() => {
    if (sessionId === null) return;
    appliedLens.current = lensRef.current;
    void select(sessionId, lensRef.current);
    const onFocus = () => void refresh();
    const onVisibility = () => {
      if (document.visibilityState === "visible") void refresh();
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
      close();
    };
  }, [sessionId, select, refresh, close]);

  // A different lens activated within the same selection.
  useLayoutEffect(() => {
    if (sessionId === null || appliedLens.current === lens) return;
    appliedLens.current = lens;
    void activateLens(lens);
  }, [sessionId, lens, activateLens]);

  const retry = useCallback(() => void refresh(), [refresh]);

  return useMemo(() => {
    if (sessionId === null) return null;
    // The store updates in an effect, so for one render after the id changes
    // it can still describe the previous session: answer "loading" instead.
    const current = storeSessionId === sessionId;
    return {
      sessionId,
      status: current && status !== "idle" ? status : "loading",
      snapshot: current ? snapshot : null,
      refreshing: current && refreshing,
      stale: current && stale,
      error: current ? error : null,
      retry,
    };
  }, [
    sessionId,
    storeSessionId,
    status,
    snapshot,
    refreshing,
    stale,
    error,
    retry,
  ]);
}
