/**
 * HumanizedError (ADR-0011, review item A2 / seed 5c24).
 *
 * Presentational body for a humanized backend/IPC failure. Given a raw error
 * string it renders plain-language title + cause, an optional Retry button,
 * and a collapsed "Details" disclosure that reveals the original developer-
 * facing string on demand (never shown by default). Shared by the notification
 * host (legacy `error` bridge) and the Analysis projection-diagnostics panel so
 * both surfaces stop echoing raw `TypeError` text.
 *
 * Uses the token-bridged Tailwind utilities (ADR-0016) so it renders correctly
 * inside both the `.notification` BEM host and the panel's Tailwind markup.
 */
import { useTranslation } from "react-i18next";
import { humanizeError } from "../utils/humanizeError";

export interface HumanizedErrorProps {
  /** The raw error string (e.g. `errorToMessage(err)` output). */
  raw: string;
  /** Optional retry handler; a Retry button renders only when provided. */
  onRetry?: () => void;
}

export default function HumanizedError({ raw, onRetry }: HumanizedErrorProps) {
  const { t } = useTranslation();
  const humanized = humanizeError(raw);
  const title = humanized.titleKey ? t(humanized.titleKey) : humanized.title;
  const cause = humanized.causeKey ? t(humanized.causeKey) : null;
  // Don't duplicate the raw string in Details when it is already the title
  // (verbatim passthrough of an already-friendly message).
  const showDetails = humanized.raw.length > 0 && humanized.raw !== title;

  return (
    <div className="flex min-w-0 flex-col gap-(--space-2)">
      <div className="font-semibold leading-[1.35] [overflow-wrap:anywhere]">
        {title}
      </div>
      {cause && (
        <p className="m-0 text-text-secondary text-xs leading-[1.4] [overflow-wrap:anywhere]">
          {cause}
        </p>
      )}
      {(onRetry || showDetails) && (
        <div className="flex flex-wrap items-center gap-(--space-3)">
          {onRetry && (
            <button
              type="button"
              className="btn btn--ghost btn--sm"
              onClick={onRetry}
            >
              {t("notifications.retry")}
            </button>
          )}
          {showDetails && (
            <details className="min-w-0 text-xs">
              <summary className="cursor-pointer select-none text-text-muted [&::-webkit-details-marker]:hidden">
                {t("notifications.details")}
              </summary>
              <pre className="m-0 mt-(--space-2) max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-sm border border-border-color bg-bg-tertiary px-(--space-3) py-(--space-2) font-mono text-2xs text-text-secondary leading-[1.4]">
                {humanized.raw}
              </pre>
            </details>
          )}
        </div>
      )}
    </div>
  );
}
