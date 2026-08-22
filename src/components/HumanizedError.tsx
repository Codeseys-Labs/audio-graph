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
 *
 * `onGoToRoute` (settings T1, seed audio-graph-2b9a) is an optional escape
 * hatch for the one caller that has a Settings modal to navigate — the
 * footer save-error alert. Kept as a callback rather than importing any
 * settings machinery here so this component stays usable from the
 * non-settings surfaces (`Notifications`, the Analysis diagnostics panel)
 * that don't pass it and never render the button.
 */
import { useTranslation } from "react-i18next";
import type { SettingsRoute } from "../types";
import { humanizeError } from "../utils/humanizeError";

export interface HumanizedErrorProps {
  /** The raw error string (e.g. `errorToMessage(err)` output). */
  raw: string;
  /** Optional retry handler; a Retry button renders only when provided. */
  onRetry?: () => void;
  /** Optional Settings-navigation handler; a "Go to" action renders only
   * when provided AND the classified error carries a `route`. */
  onGoToRoute?: (route: SettingsRoute) => void;
}

export default function HumanizedError({
  raw,
  onRetry,
  onGoToRoute,
}: HumanizedErrorProps) {
  const { t } = useTranslation();
  const humanized = humanizeError(raw);
  const title = humanized.titleKey ? t(humanized.titleKey) : humanized.title;
  const cause = humanized.causeKey ? t(humanized.causeKey) : null;
  // Don't duplicate the raw string in Details when it is already the title
  // (verbatim passthrough of an already-friendly message).
  const showDetails = humanized.raw.length > 0 && humanized.raw !== title;
  const goToRoute = humanized.route;

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
      {(onRetry || showDetails || (goToRoute && onGoToRoute)) && (
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
          {goToRoute && onGoToRoute && (
            <button
              type="button"
              className="btn btn--ghost btn--sm"
              onClick={() => onGoToRoute(goToRoute)}
            >
              {/* Reuses the existing "Configure" fix-action label
                  (`controlBar.configure`) rather than a new i18n key. */}
              {t("controlBar.configure")}
            </button>
          )}
          {showDetails && (
            <details className="min-w-0 text-xs">
              <summary className="cursor-pointer select-none text-text-muted [&::-webkit-details-marker]:hidden">
                {t("notifications.details")}
              </summary>
              <pre className="m-0 mt-(--space-2) max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-sm border border-(--edge) bg-bg-tertiary px-(--space-3) py-(--space-2) font-mono text-xs text-text-secondary leading-[1.4]">
                {humanized.raw}
              </pre>
            </details>
          )}
        </div>
      )}
    </div>
  );
}
