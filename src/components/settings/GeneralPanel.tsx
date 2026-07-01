/**
 * General rail section (blueprint §5). Theme + language prefs, the
 * `<AudioSettings>` capture form, and the `<CredentialsManager>` models/log
 * controls. Consumes the settings controller via `useSettings()` so the shell
 * no longer prop-drills these into the inline branch (Phase 2).
 */

import AudioSettings from "../AudioSettings";
import CredentialsManager from "../CredentialsManager";
import { useSettings } from "./SettingsContext";
import { LANGUAGE_OPTIONS, THEME_OPTIONS } from "./useSettingsController";

export default function GeneralPanel() {
  const {
    t,
    i18n,
    theme,
    setTheme,
    state,
    dispatch,
    models,
    modelStatus,
    isDownloading,
    isDeletingModel,
    downloadProgress,
    downloadModel,
    handleDeleteClick,
  } = useSettings();
  return (
    <>
      <section className="settings-section">
        <h3 className="settings-section-title">{t("settings.theme.label")}</h3>
        <p className="settings-section-help">{t("settings.theme.help")}</p>
        <fieldset
          className="theme-segmented"
          aria-label={t("settings.theme.label")}
        >
          {THEME_OPTIONS.map((opt) => (
            <label
              key={opt}
              className={`theme-segmented__option ${
                theme === opt ? "theme-segmented__option--active" : ""
              }`}
            >
              <input
                type="radio"
                name="app-theme"
                className="sr-only"
                value={opt}
                checked={theme === opt}
                onChange={() => setTheme(opt)}
              />
              {t(`settings.theme.${opt}`)}
            </label>
          ))}
        </fieldset>
      </section>
      <section className="settings-section">
        <h3 className="settings-section-title">{t("language.label")}</h3>
        <p className="settings-section-help">{t("language.help")}</p>
        <div className="settings-field">
          <label
            className="settings-field__label"
            htmlFor="app-language-select"
          >
            {t("language.label")}
          </label>
          <select
            id="app-language-select"
            className="settings-input"
            // i18n.resolvedLanguage is the actual active language after
            // fallback resolution (e.g. "en-US" → "en"); using it keeps
            // the control in sync with what's rendered.
            value={
              LANGUAGE_OPTIONS.includes(
                i18n.resolvedLanguage as (typeof LANGUAGE_OPTIONS)[number],
              )
                ? i18n.resolvedLanguage
                : "en"
            }
            onChange={(e) => {
              // changeLanguage persists to localStorage via the
              // browser-languagedetector cache (key `i18nextLng`),
              // so the choice survives restarts.
              void i18n.changeLanguage(e.target.value);
            }}
          >
            {LANGUAGE_OPTIONS.map((lng) => (
              <option key={lng} value={lng}>
                {t(`language.${lng}`)}
              </option>
            ))}
          </select>
        </div>
      </section>
      <AudioSettings state={state} dispatch={dispatch} t={t} />
      <CredentialsManager
        state={state}
        t={t}
        models={models}
        modelStatus={modelStatus}
        isDownloading={isDownloading}
        isDeletingModel={isDeletingModel}
        downloadProgress={downloadProgress}
        downloadModel={downloadModel}
        handleDeleteClick={handleDeleteClick}
      />
    </>
  );
}
