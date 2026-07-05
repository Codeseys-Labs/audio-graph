/**
 * Settings modal — the full configuration surface for the app.
 *
 * Phase 1 (audio-graph-settings-refactor): the ~2300-line orchestration mass
 * (state/effects/handlers/derived values) has been hoisted into
 * `settings/useSettingsController.ts` and is provided via `settings/
 * SettingsContext.tsx`. This file is now the SHELL: it renders the dialog/
 * focus-trap, the rail (tablist), the panel column, and the Save/confirm-close
 * footer, reading only nav/save/close state from the controller via context.
 *
 * Phase 4: the inline ~821-line Overview region was extracted into
 * `settings/OverviewPanel.tsx` (+ `ProductModeSummaryCards` /
 * `ProviderCapabilityCard`), the registry capability cards moved into the
 * provider panels' advanced disclosures, and the rail was extracted into
 * `settings/settingsRailConfig.ts` (single source of truth) + the presentational
 * `settings/settingsRail.tsx`. The shell now mounts one panel component per rail
 * section by `activeTab` and renders `<SettingsRail/>`; every panel consumes
 * `SettingsContext` directly so the shell stays thin and nothing is
 * prop-drilled (blueprint §5).
 *
 * Parent: `App.tsx` (rendered conditionally when `settingsOpen` is true).
 * No props.
 */

import HumanizedError from "./HumanizedError";
import IconButton from "./IconButton";
import CredentialsPanel from "./settings/CredentialsPanel";
import GeminiPanel from "./settings/GeminiPanel";
import GeneralPanel from "./settings/GeneralPanel";
import LlmPanel from "./settings/LlmPanel";
import LoggingPanel from "./settings/LoggingPanel";
import OverviewPanel from "./settings/OverviewPanel";
import { SettingsProvider } from "./settings/SettingsContext";
import SttPanel from "./settings/SttPanel";
import SettingsRail from "./settings/settingsRail";
import TtsPanel from "./settings/TtsPanel";

function SettingsPage() {
  return (
    <SettingsProvider>
      {(c) => {
        const {
          SETTINGS_TABS,
          activeTab,
          clearSaveError,
          confirmingClose,
          handleDiscardAndClose,
          handleSave,
          modalRef,
          requestClose,
          saveError,
          setConfirmingClose,
          settingsLoading,
          t,
          tabButtonId,
          tabPanelId,
        } = c;
        return (
          <div
            className="settings-overlay"
            role="none"
            onClick={requestClose}
            onKeyDown={(e) => {
              if (e.key === "Escape") requestClose();
            }}
          >
            <div
              ref={modalRef}
              className="settings-modal"
              onClick={(e) => e.stopPropagation()}
              onKeyDown={(e) => e.stopPropagation()}
              role="dialog"
              aria-modal="true"
              aria-labelledby="settings-header-title"
              tabIndex={-1}
            >
              {/* Header */}
              <div className="settings-header">
                <h2
                  id="settings-header-title"
                  className="settings-header__title"
                >
                  {t("settings.title")}
                </h2>
                <IconButton
                  icon="close"
                  label={t("settings.close")}
                  variant="ghost"
                  className="settings-header__close"
                  onClick={requestClose}
                />
              </div>

              {settingsLoading ? (
                <div className="settings-content settings-content--loading">
                  <p>{t("settings.loading")}</p>
                </div>
              ) : (
                <div className="settings-body">
                  <SettingsRail />

                  <div className="settings-panelcol">
                    <div
                      id={tabPanelId(activeTab)}
                      className="settings-tab-panel"
                      role="tabpanel"
                      aria-labelledby={tabButtonId(activeTab)}
                      // biome-ignore lint/a11y/noNoninteractiveTabindex: APG Tabs pattern (blueprint §2) — the active tabpanel is intentionally focusable so Tab from the rail always lands in the panel and the SR announces it via aria-labelledby.
                      tabIndex={0}
                    >
                      {activeTab === "overview" && <OverviewPanel />}
                      {activeTab === "general" && <GeneralPanel />}
                      {activeTab === "credentials" && <CredentialsPanel />}
                      {activeTab === "stt" && <SttPanel />}
                      {activeTab === "llm" && <LlmPanel />}
                      {activeTab === "gemini" && <GeminiPanel />}

                      {activeTab === "tts" && <TtsPanel />}

                      {activeTab === "logging" && <LoggingPanel />}
                    </div>
                    {SETTINGS_TABS.filter((tab) => tab.id !== activeTab).map(
                      (tab) => (
                        <div
                          key={tab.id}
                          id={tabPanelId(tab.id)}
                          role="tabpanel"
                          aria-labelledby={tabButtonId(tab.id)}
                          hidden
                        />
                      ),
                    )}

                    {/* Footer — pinned below the single scroller (blueprint §1.3). */}
                    <div className="settings-footer">
                      {saveError !== null && (
                        <div
                          role="alert"
                          data-testid="settings-save-error"
                          className="flex items-start gap-(--space-3) w-full py-(--space-3) px-(--space-4) rounded-md bg-(--tint-danger) text-(--text-on-tint-danger) text-sm"
                        >
                          <span className="flex-1 min-w-0">
                            <HumanizedError
                              raw={saveError}
                              onRetry={handleSave}
                            />
                          </span>
                          <IconButton
                            icon="close"
                            label={t("notifications.dismiss")}
                            variant="ghost"
                            className="shrink-0 opacity-70 hover:opacity-100"
                            onClick={clearSaveError}
                          />
                        </div>
                      )}
                      {confirmingClose && (
                        <div
                          className="settings-confirm-close"
                          role="alertdialog"
                          aria-label={t("settings.confirmClose.prompt")}
                        >
                          <span className="settings-confirm-close__text">
                            {t("settings.confirmClose.prompt")}
                          </span>
                          <button
                            type="button"
                            className="settings-btn settings-btn--secondary"
                            onClick={() => setConfirmingClose(false)}
                          >
                            {t("settings.confirmClose.keepEditing")}
                          </button>
                          <button
                            type="button"
                            className="settings-btn settings-btn--danger"
                            onClick={handleDiscardAndClose}
                          >
                            {t("settings.confirmClose.discard")}
                          </button>
                        </div>
                      )}
                      <button
                        type="button"
                        className="settings-btn settings-btn--primary"
                        onClick={handleSave}
                        disabled={settingsLoading}
                      >
                        {t("settings.buttons.save")}
                      </button>
                    </div>
                  </div>
                </div>
              )}
            </div>
          </div>
        );
      }}
    </SettingsProvider>
  );
}

export default SettingsPage;
