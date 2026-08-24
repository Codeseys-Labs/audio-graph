import { useTranslation } from "react-i18next";
import Icon from "./Icon";

/**
 * The root `ErrorBoundary`'s default fallback (audio-graph-16e2). Rendered
 * in place of the ENTIRE app when a render/lifecycle error escapes to the
 * root boundary (`main.tsx`) — there is no other chrome on screen at that
 * point, so this owns its own full-viewport centering rather than assuming
 * any surrounding layout.
 *
 * Two escape hatches, same split as `GetStartedFallback`'s probe-failure
 * card (styling deliberately mirrors it — `.ag-card`, muted glyph, one
 * explanatory line, then actions):
 *   - Reload: `window.location.reload()` — reloads the frontend WebView
 *     only. In the Tauri shell this does NOT reset the Rust backend, so an
 *     in-flight capture pipeline (or other backend state) survives the
 *     reload; this is a frontend-only reset, not a full process restart.
 *   - Back to Capture: a lighter in-app recovery that resets navigation to
 *     the Capture destination; `ErrorBoundary` itself listens for that nav
 *     change and clears its caught state in response (see its module doc).
 *
 * SECURITY: `errorName` is the ONLY error-derived value this component ever
 * receives or renders — `ErrorBoundary` deliberately never forwards the
 * caught error's `message` or `stack` (which could echo transcript/notes
 * content interpolated into a thrown error). Never add a prop that would
 * carry either.
 */
export interface RootErrorFallbackProps {
  /** The caught error's `name` only (e.g. `"TypeError"`) — see the SECURITY
   * note above; never pass a message or stack through here. */
  errorName: string;
  /** Reload the whole window (`window.location.reload()`), owned by the
   * caller so this component stays a pure/testable presentational unit. */
  onReload: () => void;
  /** Navigate back to the Capture destination, owned by the caller for the
   * same reason. */
  onBackToCapture: () => void;
}

function RootErrorFallback({
  errorName,
  onReload,
  onBackToCapture,
}: RootErrorFallbackProps) {
  const { t } = useTranslation();
  const title = t("app.errorBoundary.title");

  return (
    <div
      className="h-full w-full flex items-center justify-center p-(--space-6)"
      role="alert"
    >
      <section
        className="ag-card flex flex-col items-center gap-(--space-5) p-(--space-6) text-center max-w-[440px]"
        aria-label={title}
        data-testid="root-error-fallback"
      >
        <span className="text-accent-red opacity-80" aria-hidden="true">
          <Icon name="error" size={32} />
        </span>
        <div className="flex flex-col gap-(--space-2)">
          <h1 className="m-0 text-text-primary text-lg font-semibold">
            {title}
          </h1>
          <p className="m-0 text-text-secondary text-sm leading-normal">
            {t("app.errorBoundary.body")}
          </p>
          <span
            className="ag-chip self-center mt-(--space-2)"
            data-tone="danger"
            data-testid="root-error-fallback-class"
          >
            {t("app.errorBoundary.errorClass", { name: errorName })}
          </span>
        </div>
        <div className="flex flex-wrap items-center justify-center gap-(--space-4)">
          <button
            type="button"
            className="inline-flex items-center gap-(--space-3) py-(--space-3) px-(--space-5) rounded-md text-sm font-semibold cursor-pointer bg-accent-blue text-(--on-accent-blue) border-none transition-opacity hover:opacity-90"
            onClick={() => onReload()}
          >
            <Icon name="refresh" size={16} />
            {t("app.errorBoundary.reload")}
          </button>
          <button
            type="button"
            className="inline-flex items-center gap-(--space-2) py-(--space-3) px-(--space-5) rounded-md text-sm font-semibold cursor-pointer bg-none border border-accent-blue text-accent-blue transition-[background-color] duration-[150ms] ease-[ease] hover:bg-(--tint-accent-info-strong)"
            onClick={() => onBackToCapture()}
          >
            {t("app.errorBoundary.backToCapture")}
          </button>
        </div>
      </section>
    </div>
  );
}

export default RootErrorFallback;
