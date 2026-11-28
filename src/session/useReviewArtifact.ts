/**
 * `useReviewArtifact` — a read-only view of one heavy lens artifact (Notes,
 * Graph or Timeline) for the Review selection.
 *
 * It fetches nothing: the provider's `useReviewTranscript` owns every query, so
 * the artifact is always read in the same round as the snapshot (see
 * `reviewStore.ts`). `retry` re-runs that round. Outside a Review subtree it is
 * inert and returns `null`.
 */
import { useCallback } from "react";
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
  /** Re-run the round (retry after an error, or manual refresh). */
  retry: () => void;
}

export function useReviewArtifact<K extends ArtifactKind>(
  kind: K,
  reviewSessionId: string | null,
): ReviewArtifactView<K> | null {
  const refresh = useReviewStore((s) => s.refresh);
  const slot = useReviewStore((s) => s.artifacts[kind]) as ReviewArtifacts[K];
  const selected = useReviewStore((s) => s.sessionId);
  const retry = useCallback(() => void refresh({ explicit: true }), [refresh]);

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
