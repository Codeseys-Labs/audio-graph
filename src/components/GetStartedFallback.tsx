import { useTranslation } from "react-i18next";
import Icon from "./Icon";

/**
 * First-run fallback rendered inside the During workspace when the
 * `load_credential_presence_cmd` probe throws (backend not ready, keychain
 * locked, fresh-install race). Without this, a failed probe leaves a first-run
 * user staring at empty Notes/Transcript panels plus a raw error toast — a
 * dead cockpit (seed audio-graph-fbf0, review item A3).
 *
 * This is deliberately narrow: it is *only* the probe-FAILURE recovery path.
 * The full first-run onboarding vision is owned by seed 75a1 (ExpressSetup +
 * the sample-session preview + the hand-off nudge). Here we give the user a
 * friendly one-line explanation (never the raw error) and three escape hatches:
 *   - Preview a sample session (reuses the existing sample-preview flow),
 *   - Retry the probe,
 *   - Open settings.
 *
 * Styling mirrors the Audio Sources / Live Transcript empty states (the
 * quality bar called out in the review): a muted glyph, a positive-framing
 * title, a single explanatory line, then the actions.
 */
interface GetStartedFallbackProps {
  /** Launch the existing sample-session preview (parent-owned handoff). */
  onPreviewSample: () => void;
  /** Re-run the credential presence probe. */
  onRetry: () => void;
  /** Open the Settings modal as a manual escape hatch. */
  onOpenSettings: () => void;
  /** True while a retry probe is in flight (disables + relabels Retry). */
  retrying?: boolean;
}

function GetStartedFallback({
  onPreviewSample,
  onRetry,
  onOpenSettings,
  retrying = false,
}: GetStartedFallbackProps) {
  const { t } = useTranslation();

  return (
    <section
      className="flex-1 min-w-0 min-h-0 flex flex-col items-center justify-center gap-(--space-5) p-(--space-6) text-center bg-bg-secondary overflow-auto"
      aria-label={t("onboarding.fallbackTitle")}
      data-testid="get-started-fallback"
    >
      <span className="text-text-muted opacity-40" aria-hidden="true">
        <Icon name="start" size={32} />
      </span>
      <div className="flex flex-col gap-(--space-2) max-w-[440px]">
        <h2 className="m-0 text-text-primary text-lg font-semibold">
          {t("onboarding.fallbackTitle")}
        </h2>
        <p className="m-0 text-text-secondary text-sm leading-normal">
          {t("onboarding.fallbackBody")}
        </p>
      </div>
      <div className="flex flex-wrap items-center justify-center gap-(--space-4)">
        <button
          type="button"
          className="inline-flex items-center gap-(--space-3) py-(--space-3) px-(--space-5) rounded-md text-sm font-semibold cursor-pointer bg-accent-blue text-white border-none transition-opacity hover:opacity-90"
          onClick={onPreviewSample}
        >
          <Icon name="start" size={16} />
          {t("onboarding.fallbackPreviewSample")}
        </button>
        <button
          type="button"
          className="inline-flex items-center gap-(--space-2) py-(--space-3) px-(--space-5) rounded-md text-sm font-semibold cursor-pointer bg-none border border-accent-blue text-accent-blue transition-[background-color] duration-[150ms] ease-[ease] hover:not-disabled:bg-(--tint-accent-info-strong) disabled:opacity-50 disabled:cursor-not-allowed"
          onClick={onRetry}
          disabled={retrying}
          aria-busy={retrying}
        >
          <Icon name="refresh" size={16} />
          {retrying
            ? t("onboarding.fallbackRetrying")
            : t("onboarding.fallbackRetry")}
        </button>
        <button
          type="button"
          className="inline-flex items-center gap-(--space-2) py-(--space-3) px-(--space-4) rounded-md text-sm cursor-pointer bg-transparent border-none text-text-secondary underline transition-colors hover:text-text-primary"
          onClick={onOpenSettings}
        >
          <Icon name="settings" size={16} />
          {t("onboarding.fallbackOpenSettings")}
        </button>
      </div>
    </section>
  );
}

export default GetStartedFallback;
