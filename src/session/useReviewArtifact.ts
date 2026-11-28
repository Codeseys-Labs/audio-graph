/**
 * `useReviewArtifact` — fetches one heavy lens artifact (Notes, Graph or
 * Timeline) for the Review selection, only while the lens that needs it is
 * mounted.
 *
 * Mounting is the activation: the effect queries once on mount and again on
 * window focus / tab visibility (Review is refreshable, see `reviewStore.ts`).
 * The last good data stays on screen while it re-queries and after a failed
 * re-query, which marks it `stale`.
 *
 * Must run under a `SessionViewProvider` that has a `reviewSessionId`; the
 * provider's `useReviewTranscript` selects the session in a layout effect, so
 * the selection exists by the time this passive effect runs. Outside a Review
 * subtree it is inert and returns `null`.
 */
import { useCallback, useEffect } from "react";
import {
  type ArtifactKind,
  type ReviewArtifacts,
  useReviewStore,
} from "./reviewStore";

export interface ReviewArtifactView<K extends ArtifactKind> {
  status: ReviewArtifacts[K]["status"];
  data: ReviewArtifacts[K]["data"];
  stale: boolean;
  refusal: ReviewArtifacts[K]["refusal"];
  /** Re-run the query (retry after an error, or manual refresh). */
  retry: () => void;
}

export function useReviewArtifact<K extends ArtifactKind>(
  kind: K,
  reviewSessionId: string | null,
): ReviewArtifactView<K> | null {
  const loadArtifact = useReviewStore((s) => s.loadArtifact);
  const slot = useReviewStore((s) => s.artifacts[kind]) as ReviewArtifacts[K];
  const selected = useReviewStore((s) => s.sessionId);

  useEffect(() => {
    if (reviewSessionId === null) return;
    void loadArtifact(kind);
    const onFocus = () => void loadArtifact(kind);
    const onVisibility = () => {
      if (document.visibilityState === "visible") void loadArtifact(kind);
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [kind, reviewSessionId, loadArtifact]);

  const retry = useCallback(
    () => void loadArtifact(kind),
    [kind, loadArtifact],
  );

  if (reviewSessionId === null) return null;
  // Until the provider's selection lands the slot belongs to nothing: loading.
  const current = selected === reviewSessionId;
  return {
    status: current ? slot.status : "loading",
    data: current ? slot.data : null,
    stale: current && slot.stale,
    refusal: current ? slot.refusal : null,
    retry,
  };
}
