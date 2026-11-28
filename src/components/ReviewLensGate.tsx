/**
 * ReviewLensGate — the loading / closed-error / stale / refusal frame around a
 * lens (Notes, Graph, Timeline) that renders a reviewed session from Review
 * data instead of the global store (issue #140, slice 2).
 *
 * It fetches nothing: the provider (`SessionViewProvider` via
 * `useReviewTranscript`) owns every Review query, reading the session snapshot
 * and the open lens's artifact in ONE round and publishing them together, so
 * the children never see an artifact next to a snapshot from a different read.
 * The gate renders `children` only once that pair has been published; they then
 * read the data through `useSessionView()` / `useTimelineView()`.
 *
 * States, all localized, none showing backend text:
 *   - loading: the first round is in flight.
 *   - unavailable: a first load failed (missing, unreadable or corrupt), with a
 *     retry. Backend message text is never stored, so it cannot be rendered.
 *   - refused: the typed byte-ceiling refusal (`ArtifactRefusalNotice`).
 *   - stale: a round failed in either half; the last good pair stays visible
 *     under a notice with a retry.
 *
 * Keyboard focus: a retry control disappears as soon as loading starts (or the
 * stale notice, on recovery), so focus is parked on the lens region instead of
 * being dropped to the document body.
 *
 * Must be rendered under a `SessionViewProvider` with a `reviewSessionId`.
 */
import { type ReactNode, useRef } from "react";
import { useTranslation } from "react-i18next";
import type { ArtifactKind } from "../session/reviewStore";
import {
  useReviewSessionId,
  useReviewView,
} from "../session/SessionViewProvider";
import { useReviewArtifact } from "../session/useReviewArtifact";
import { ArtifactRefusalNotice } from "./ArtifactRefusalNotice";
import Icon from "./Icon";
import { useRetryFocus } from "./useRetryFocus";

export interface ReviewLensGateProps {
  kind: ArtifactKind;
  children: ReactNode;
}

export function ReviewLensGate({ kind, children }: ReviewLensGateProps) {
  const { t } = useTranslation();
  const reviewSessionId = useReviewSessionId();
  const review = useReviewView();
  const artifact = useReviewArtifact(kind, reviewSessionId);

  // Keyboard focus after a retry control goes away (see `useRetryFocus`).
  const regionRef = useRef<HTMLElement>(null);
  const markRetry = useRetryFocus(regionRef, regionRef);

  if (review === null || artifact === null) return <>{children}</>;

  const frame = (content: ReactNode) => (
    <section
      ref={regionRef}
      tabIndex={-1}
      aria-label={t(`sessions.lens.${kind}`)}
      className="flex min-h-0 flex-1 flex-col outline-none"
      data-testid="review-lens-region"
    >
      {content}
    </section>
  );

  if (artifact.status === "refused" && artifact.refusal) {
    return frame(
      <ArtifactRefusalNotice
        status={{ type: "refused", ...artifact.refusal }}
      />,
    );
  }

  if (review.status === "error" || artifact.status === "error") {
    // One round re-queries the snapshot and this lens together.
    const retry = review.retry;
    return frame(
      <section
        className="flex h-full flex-col items-center justify-center gap-(--space-3) p-(--space-6) text-center"
        data-testid="review-lens-unavailable"
      >
        <p
          className="m-0 max-w-[420px] text-sm text-text-secondary"
          role="alert"
        >
          {t("sessions.review.viewUnavailable")}
        </p>
        <button
          type="button"
          className="ag-btn-micro"
          data-review-retry
          onClick={(event) => {
            markRetry(event);
            retry();
          }}
        >
          <Icon name="refresh" size={14} /> {t("sessions.review.retry")}
        </button>
      </section>,
    );
  }

  const loading =
    review.snapshot === null ||
    artifact.status === "idle" ||
    artifact.status === "loading";
  if (loading) {
    return frame(
      <div className="flex flex-1 min-w-0 items-center justify-center p-(--space-6)">
        <p className="m-0 text-sm italic text-text-muted" role="status">
          {t("common.loading")}
        </p>
      </div>,
    );
  }

  const data = artifact.data;
  const projection =
    kind === "notes" && data && "projection_events" in data
      ? data.notes
      : kind === "graph" && data && "nodes" in data
        ? data
        : null;
  const unavailable =
    projection?.reviewProjectionStatuses?.includes("basis_unavailable");
  const behind =
    projection?.reviewProjectionStatuses?.includes("behind_transcript");
  const gap =
    projection?.reviewProjectionStatuses?.includes("recompute_required");
  const stale = review.stale || artifact.stale;
  return frame(
    <>
      {stale && (
        <div
          className="mx-(--space-5) mt-(--space-4) flex items-center justify-between gap-(--space-3) rounded-sm border border-(--tint-border-warning) bg-(--tint-warning) px-(--space-4) py-(--space-3) text-xs text-(--text-on-tint-warning)"
          role="alert"
          data-testid="review-lens-stale"
        >
          <span>{t("sessions.review.staleView")}</span>
          <button
            type="button"
            className="ag-btn-micro"
            data-review-retry
            onClick={(event) => {
              markRetry(event);
              review.retry();
            }}
          >
            <Icon name="refresh" size={14} /> {t("sessions.review.retry")}
          </button>
        </div>
      )}
      {(unavailable || behind || gap) && (
        <p
          className="mx-(--space-5) mt-(--space-4) text-sm text-text-secondary"
          role="status"
        >
          {[
            unavailable && t("sessions.review.projectionBasisUnavailable"),
            behind && t("sessions.review.projectionBehindTranscript"),
            gap && t("sessions.review.projectionGap"),
          ]
            .filter(Boolean)
            .join(" ")}
        </p>
      )}
      {children}
    </>,
  );
}

export default ReviewLensGate;
