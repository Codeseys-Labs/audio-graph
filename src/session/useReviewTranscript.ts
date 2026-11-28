/**
 * `useReviewTranscript` — binds a mounted Review surface to `reviewStore`.
 *
 * Opens the given session on mount / id change, re-queries on window focus and
 * when the tab becomes visible again (Review is refreshable, not immutable —
 * see `reviewStore.ts`), and releases the selection on unmount so a late
 * response cannot land after the surface is gone.
 *
 * Passing `null` means "no Review selection": the hook is inert, which lets
 * callers invoke it unconditionally (rules of hooks) and fall back to Live.
 *
 * It only ever reads `reviewStore`; nothing here touches the Live store.
 */
import { useCallback, useEffect, useMemo } from "react";
import {
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
): ReviewTranscriptView | null {
  const select = useReviewStore((s) => s.select);
  const refresh = useReviewStore((s) => s.refresh);
  const close = useReviewStore((s) => s.close);
  const storeSessionId = useReviewStore((s) => s.sessionId);
  const status = useReviewStore((s) => s.status);
  const snapshot = useReviewStore((s) => s.snapshot);
  const refreshing = useReviewStore((s) => s.refreshing);
  const stale = useReviewStore((s) => s.stale);
  const error = useReviewStore((s) => s.error);

  useEffect(() => {
    if (sessionId === null) return;
    void select(sessionId);
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
