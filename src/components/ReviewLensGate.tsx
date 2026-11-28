/**
 * ReviewLensGate — the loading / closed-error / stale / refusal frame around a
 * lens (Notes, Graph, Timeline) that renders a reviewed session from Review
 * data instead of the global store (issue #140, slice 2).
 *
 * Mounting it IS the lens activation: it starts the lens's artifact query
 * (`useReviewArtifact`), and re-queries on window focus. It renders `children`
 * only once both the session snapshot and the lens artifact have loaded; the
 * children then read the data through `useSessionView()` / `useTimelineView()`.
 *
 * States, all localized, none showing backend text:
 *   - loading: the first query is in flight.
 *   - unavailable: a first load failed (missing, unreadable or corrupt), with a
 *     retry. Backend message text is never stored, so it cannot be rendered.
 *   - refused: the typed byte-ceiling refusal (`ArtifactRefusalNotice`).
 *   - stale: a re-query failed; the last good data stays visible under a notice
 *     with a retry.
 *
 * Must be rendered under a `SessionViewProvider` with a `reviewSessionId`.
 */
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type { ArtifactKind } from "../session/reviewStore";
import {
  useReviewSessionId,
  useReviewView,
} from "../session/SessionViewProvider";
import { useReviewArtifact } from "../session/useReviewArtifact";
import { ArtifactRefusalNotice } from "./ArtifactRefusalNotice";
import Icon from "./Icon";

export interface ReviewLensGateProps {
  kind: ArtifactKind;
  children: ReactNode;
}

export function ReviewLensGate({ kind, children }: ReviewLensGateProps) {
  const { t } = useTranslation();
  const reviewSessionId = useReviewSessionId();
  const review = useReviewView();
  const artifact = useReviewArtifact(kind, reviewSessionId);

  if (review === null || artifact === null) return <>{children}</>;

  if (artifact.status === "refused" && artifact.refusal) {
    return (
      <ArtifactRefusalNotice
        status={{ type: "refused", ...artifact.refusal }}
      />
    );
  }

  if (review.status === "error" || artifact.status === "error") {
    const retry = () => {
      if (review.status === "error") review.retry();
      if (artifact.status === "error") artifact.retry();
    };
    return (
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
        <button type="button" className="ag-btn-micro" onClick={retry}>
          <Icon name="refresh" size={14} /> {t("sessions.review.retry")}
        </button>
      </section>
    );
  }

  const loading =
    review.snapshot === null ||
    artifact.status === "idle" ||
    artifact.status === "loading";
  if (loading) {
    return (
      <div className="flex flex-1 min-w-0 items-center justify-center p-(--space-6)">
        <p className="m-0 text-sm italic text-text-muted" role="status">
          {t("common.loading")}
        </p>
      </div>
    );
  }

  const stale = review.stale || artifact.stale;
  return (
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
            onClick={() => {
              if (review.stale) review.retry();
              if (artifact.stale) artifact.retry();
            }}
          >
            <Icon name="refresh" size={14} /> {t("sessions.review.retry")}
          </button>
        </div>
      )}
      {children}
    </>
  );
}

export default ReviewLensGate;
