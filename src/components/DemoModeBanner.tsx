import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import Icon from "./Icon";
import { modelRouteForProviderId } from "./settings/settingsRoutes";

/**
 * Banner shown at the top of the app when the first-launch demo-mode
 * decision (made by the Rust backend) selected local-only providers AND
 * the user hasn't yet downloaded the required local models. Its job is to
 * point the user at the Models section of Settings so the app can actually
 * do work.
 *
 * Visibility is derived — there is no local "dismiss" state. The banner
 * disappears on its own once both the Whisper and Llama models report
 * `Ready`, which keeps it honest: closing it manually and never
 * downloading would leave the app unusable with no hint as to why.
 */
function DemoModeBanner() {
  const { t } = useTranslation();
  const settings = useAudioGraphStore((s) => s.settings);
  const modelStatus = useAudioGraphStore((s) => s.modelStatus);
  const openSettings = useAudioGraphStore((s) => s.openSettings);
  const fetchSettings = useAudioGraphStore((s) => s.fetchSettings);
  const fetchModelStatus = useAudioGraphStore((s) => s.fetchModelStatus);

  // Settings aren't auto-loaded at app boot, so prime them here the first
  // time this banner mounts. We always fetch settings (we need to know
  // `demo_mode` to decide visibility), and fetch model status only once
  // we know demo mode is on — no point probing the disk otherwise.
  // Errors are already surfaced via the store's `error` field.
  useEffect(() => {
    if (settings === null) {
      void fetchSettings();
    }
  }, [settings, fetchSettings]);

  useEffect(() => {
    if (settings?.demo_mode === true && modelStatus === null) {
      void fetchModelStatus();
    }
  }, [settings?.demo_mode, modelStatus, fetchModelStatus]);

  if (settings?.demo_mode !== true) return null;

  // Both models must be Ready before we hide — either one missing and
  // the pipeline still can't run end-to-end.
  const bothReady =
    modelStatus !== null &&
    modelStatus.whisper === "Ready" &&
    modelStatus.llm === "Ready";
  if (bothReady) return null;

  const handleOpen = () => {
    // T4a moved the Models section General -> Credentials
    // (`#settings-models-section` now renders inside `CredentialsPanel.tsx`,
    // settingsRoutes.ts's `modelRouteForProviderId("llm.local_llama")`).
    // Previously this called bare `openSettings()` + a manual
    // `getElementById("settings-models-section")` scroll — since only the
    // ACTIVE panel mounts (`SettingsPage.tsx`), that scroll was already a
    // silent no-op pre-T4a (General doesn't default-render on open) and
    // stayed one after T4a's move (Credentials isn't the default tab
    // either). Routing through `openSettings(route)` (T1's own addressing
    // mechanism) both lands on the right tab AND focuses/scrolls the field
    // for real, via `useSettingsController`'s `focusSettingsField`.
    openSettings(modelRouteForProviderId("llm.local_llama") ?? undefined);
  };

  return (
    <div
      className="banner-on-accent flex items-center gap-(--space-5) py-[10px] px-(--space-6) bg-banner-demo text-white text-md shadow-1 z-[var(--z-banner)]"
      // role=alert (critique B7 / WCAG 4.1.3): the demo-mode banner signals the
      // app can't run end-to-end until local models download, so it warrants an
      // assertive announcement (implicit aria-live="assertive" + aria-atomic)
      // rather than the polite status it previously used.
      role="alert"
      data-testid="demo-banner"
    >
      <span className="text-xl shrink-0" aria-hidden="true">
        <Icon name="demo" />
      </span>
      <div className="flex flex-col flex-1 gap-(--space-1) leading-[1.3]">
        <strong className="font-semibold">{t("demo.title")}</strong>
        <span className="opacity-95">{t("demo.message")}</span>
      </div>
      <button
        type="button"
        className="bg-[rgba(255,255,255,0.18)] border border-[rgba(255,255,255,0.45)] text-white cursor-pointer text-md py-[5px] px-(--space-5) rounded-sm shrink-0 transition-colors hover:bg-[rgba(255,255,255,0.3)]"
        onClick={handleOpen}
        data-testid="demo-banner-open-settings"
      >
        {t("demo.openSettings")}
      </button>
    </div>
  );
}

export default DemoModeBanner;
