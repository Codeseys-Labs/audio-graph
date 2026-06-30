/**
 * Overview rail section — the orientation "homepage" (blueprint §1.1, Phase 4).
 *
 * STEP 2 relocated the registry capability cards out of Overview into each
 * provider panel's advanced disclosure (blueprint §1.2), so this panel now holds
 * the product-mode summary cards and the cross-provider readiness rollup. The
 * mode-card and readiness markup is unchanged; only the capability section
 * moved. Reads everything from the settings controller via `useSettings()`.
 */

import {
  ProviderReadinessDetails,
  providerCatalogLabel,
  providerCatalogSummary,
  providerRecoveryAction,
} from "../ProviderReadinessPanel";
import { PROVIDER_DESCRIPTORS } from "../providerRegistryHelpers";
import ProductModeSummaryCards from "./ProductModeSummaryCards";
import { useSettings } from "./SettingsContext";
import { PROVIDER_READINESS_LABELS } from "./useSettingsController";

export default function OverviewPanel() {
  const {
    t,
    providerReadinessLoading,
    providerReadinessError,
    providerReadinessStatusSummary,
    visibleProviderReadiness,
    activeReadinessProviderIdSet,
    selectedModelForProvider,
    credentialRouteForReadiness,
    credentialPresence,
    handleOpenCredentialRoute,
    refreshProviderReadiness,
  } = useSettings();

  return (
    <>
      <ProductModeSummaryCards />
      <section
        className="settings-readiness"
        aria-labelledby="settings-readiness-title"
      >
        <div className="settings-readiness__header">
          <div>
            <h3
              id="settings-readiness-title"
              className="settings-readiness__title"
            >
              {t("settings.providerReadiness.title")}
            </h3>
            <p className="settings-readiness__help">
              {t("settings.providerReadiness.help")}
            </p>
          </div>
          <div className="settings-readiness__actions">
            {providerReadinessLoading && (
              <span className="settings-readiness__loading">
                {t("settings.providerReadiness.checking")}
              </span>
            )}
            <button
              type="button"
              className="settings-btn settings-btn--secondary settings-readiness__refresh"
              onClick={() => void refreshProviderReadiness()}
              disabled={providerReadinessLoading}
            >
              {t("settings.providerReadiness.runChecks")}
            </button>
          </div>
        </div>
        <p
          id="settings-readiness-status-summary"
          className="sr-only"
          role="status"
          aria-live="polite"
          aria-atomic="true"
          aria-busy={providerReadinessLoading}
          aria-label={t("settings.providerReadiness.title")}
        >
          {providerReadinessStatusSummary}
        </p>
        <div className="settings-readiness__status">
          {providerReadinessError ? (
            <div className="settings-readiness__error">
              <p className="settings-readiness__message">
                {t("settings.providerReadiness.error", {
                  error: providerReadinessError,
                })}
              </p>
              <p className="settings-readiness__recovery">
                <span>{t("settings.providerReadiness.recoveryLabel")}</span>{" "}
                {t("settings.providerReadiness.credentialFileRecovery")}
              </p>
            </div>
          ) : visibleProviderReadiness.length === 0 ? (
            <p className="settings-readiness__empty">
              {t("settings.providerReadiness.empty")}
            </p>
          ) : (
            <div className="settings-readiness__list">
              {visibleProviderReadiness.map((entry) => {
                const catalogSummary = providerCatalogSummary(entry);
                const selectedModel = activeReadinessProviderIdSet.has(
                  entry.provider_id,
                )
                  ? selectedModelForProvider(entry.provider_id)
                  : null;
                const recoveryAction = providerRecoveryAction(
                  entry,
                  t,
                  PROVIDER_DESCRIPTORS.get(entry.provider_id),
                );
                const credentialRoute = credentialRouteForReadiness(entry);

                return (
                  <div
                    key={entry.provider_id}
                    className="settings-readiness__item"
                  >
                    <div className="settings-readiness__item-main">
                      <span className="settings-readiness__provider">
                        {PROVIDER_READINESS_LABELS.get(entry.provider_id) ??
                          entry.provider_id}
                      </span>
                      <span
                        className={`settings-readiness__badge settings-readiness__badge--${entry.status}`}
                      >
                        {t(`settings.providerReadiness.status.${entry.status}`)}
                      </span>
                    </div>
                    {selectedModel && (
                      <p className="settings-readiness__meta">
                        {t("settings.providerReadiness.selected", {
                          value: selectedModel,
                        })}
                      </p>
                    )}
                    <p className="settings-readiness__message">
                      {entry.message}
                      {entry.stale
                        ? ` ${t("settings.providerReadiness.stale")}`
                        : ""}
                      {catalogSummary
                        ? ` ${providerCatalogLabel(catalogSummary, t)}`
                        : ""}
                    </p>
                    {recoveryAction && (
                      <p className="settings-readiness__recovery">
                        <span>
                          {t("settings.providerReadiness.recoveryLabel")}
                        </span>{" "}
                        {recoveryAction}
                      </p>
                    )}
                    {credentialRoute && (
                      <button
                        type="button"
                        className="settings-btn settings-btn--secondary settings-readiness__open"
                        onClick={() => handleOpenCredentialRoute(entry)}
                      >
                        {t("settings.providerReadiness.openCredential")}
                      </button>
                    )}
                    <ProviderReadinessDetails
                      entry={entry}
                      credentialPresence={credentialPresence}
                      t={t}
                    />
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </section>
    </>
  );
}
