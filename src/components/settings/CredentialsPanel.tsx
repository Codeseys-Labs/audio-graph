/**
 * Credentials & readiness rail section (blueprint §1.2 / §7, Phase 4; WS3 /
 * ADR-0006 B1).
 *
 * Hosts BOTH credential pivots so the "Credentials & readiness" tab is the one
 * place to check provider setup:
 *   1. The by-provider readiness rollup (moved here from Overview by WS3). It
 *      reads `visibleProviderReadiness`, which INCLUDES providers with
 *      `missing_credentials` / `unchecked` (no saved key yet), preserving the
 *      "here's a provider you haven't set up" affordance.
 *   2. The per-key credential-health rows (by-key pivot). Each row gains a
 *      scannable status chip (Ready / Needs validation / Failing / Unused)
 *      derived from the linked provider readiness, keeps Replace/Retest inline,
 *      and moves the destructive Clear behind a `⋯` overflow with consequence
 *      copy (handled by the existing `handleClearCredential` confirm).
 *
 * Reads everything from `useSettings()`. The `.settings-credential-health__item`
 * markup + `<code>{key}</code>` shape is preserved so deep-link routing and the
 * credential tests keep resolving.
 */

import type { ProviderReadiness } from "../../types";
import {
  credentialSourceLabel,
  ProviderReadinessDetails,
  providerCatalogLabel,
  providerCatalogSummary,
  providerRecoveryAction,
} from "../ProviderReadinessPanel";
import { PROVIDER_DESCRIPTORS } from "../providerRegistryHelpers";
import Badge, { type BadgeTone, readinessTone } from "./Badge";
import ReadinessModelActions from "./ReadinessModelActions";
import { useSettings } from "./SettingsContext";
import {
  formatCredentialCheckedAt,
  PROVIDER_READINESS_LABELS,
} from "./useSettingsController";

type CredentialChip =
  | "ready"
  | "needsValidation"
  | "failing"
  | "unused"
  | "unavailable";

const CREDENTIAL_CHIP_TONE: Record<CredentialChip, BadgeTone> = {
  ready: "success",
  needsValidation: "warning",
  failing: "danger",
  unused: "neutral",
  unavailable: "neutral",
};

function credentialStatusChip(related: ProviderReadiness[]): CredentialChip {
  if (related.length === 0) return "unused";
  if (related.some((entry) => entry.status === "error")) return "failing";
  if (related.every((entry) => entry.status === "ready")) return "ready";
  return "needsValidation";
}

export default function CredentialsPanel() {
  const {
    t,
    savedCredentialEntries,
    relatedReadinessForCredential,
    providerLabelsForCredential,
    latestValidationForCredential,
    credentialRouteForKey,
    handleOpenCredentialKey,
    refreshProviderReadiness,
    providerReadinessLoading,
    handleClearCredential,
    providerReadinessError,
    providerReadinessStatusSummary,
    visibleProviderReadiness,
    activeReadinessProviderIdSet,
    selectedModelForProvider,
    credentialRouteForReadiness,
    credentialPresence,
    handleOpenCredentialRoute,
  } = useSettings();
  // NOTE: The model-action state (models/downloadModel/handleDeleteClick/
  // confirmDelete/downloadProgress/isDownloading/isDeletingModel) is read inside
  // `ReadinessModelActions` from `useSettings()`, not destructured here. The
  // `MODEL_DOWNLOAD_PROGRESS` listener mutates `downloadProgress`/`isDownloading`
  // on every progress tick; keeping the read scoped to that subtree avoids
  // naming the fast-changing fields in this panel's own destructure.

  return (
    <>
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
                      <Badge tone={readinessTone(entry.status)}>
                        {t(`settings.providerReadiness.status.${entry.status}`)}
                      </Badge>
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
                    {/* LOCAL model-backed providers get inline Download/Delete
                        controls for the model files they require. Cloud
                        providers have no `local_models`, so this renders
                        nothing for them (design 2026-07-02 §2 / §4). */}
                    <ReadinessModelActions
                      providerId={entry.provider_id}
                      t={t}
                    />
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
      <section
        className="settings-credential-health"
        aria-labelledby="settings-credential-health-title"
      >
        <div className="settings-credential-health__header">
          <div>
            <h3
              id="settings-credential-health-title"
              className="settings-credential-health__title"
            >
              {t("settings.credentialHealth.title")}
            </h3>
            <p className="settings-credential-health__help">
              {t("settings.credentialHealth.help")}
            </p>
          </div>
        </div>
        {savedCredentialEntries.length === 0 ? (
          <p className="settings-credential-health__empty">
            {t("settings.credentialHealth.empty")}
          </p>
        ) : (
          <div className="settings-credential-health__list">
            {savedCredentialEntries.map((credential) => {
              const relatedReadiness = relatedReadinessForCredential(
                credential.key,
              );
              const providerLabels = providerLabelsForCredential(
                credential.key,
                relatedReadiness,
              );
              const latestCheckedAt = formatCredentialCheckedAt(
                latestValidationForCredential(relatedReadiness),
              );
              const route = credentialRouteForKey(credential.key);
              // When the latest readiness fetch FAILED, `relatedReadiness` is
              // empty for every saved key — not because the credential is
              // unused, but because we have no data. Do not downgrade a present,
              // configured credential to the misleading "unused"/noProviders
              // states in that case; show a neutral "status unavailable"
              // instead. Behavior is identical when there is no error.
              const chip: CredentialChip = providerReadinessError
                ? "unavailable"
                : credentialStatusChip(relatedReadiness);

              return (
                <div
                  key={credential.key}
                  className="settings-credential-health__item"
                >
                  <div className="settings-credential-health__item-main">
                    <code>{credential.key}</code>
                    <Badge tone={CREDENTIAL_CHIP_TONE[chip]}>
                      {t(`settings.credentialHealth.statusChip.${chip}`)}
                    </Badge>
                  </div>
                  <dl className="settings-credential-health__details">
                    <div>
                      <dt>{t("settings.credentialHealth.source")}</dt>
                      <dd>{credentialSourceLabel(credential.source, t)}</dd>
                    </div>
                    <div>
                      <dt>{t("settings.credentialHealth.lastValidation")}</dt>
                      <dd>
                        {latestCheckedAt ??
                          t("settings.providerReadiness.notChecked")}
                      </dd>
                    </div>
                    <div>
                      <dt>{t("settings.credentialHealth.unlocks")}</dt>
                      <dd>
                        {providerLabels.length > 0
                          ? providerLabels.join(", ")
                          : t("settings.credentialHealth.noProviders")}
                      </dd>
                    </div>
                  </dl>
                  {providerReadinessError ? (
                    <p className="settings-credential-health__providers">
                      {t("settings.credentialHealth.readinessUnavailable")}
                    </p>
                  ) : (
                    relatedReadiness.length > 0 && (
                      <p className="settings-credential-health__providers">
                        {t("settings.credentialHealth.providerStatus")}{" "}
                        {relatedReadiness
                          .map((entry) => {
                            const label =
                              PROVIDER_READINESS_LABELS.get(
                                entry.provider_id,
                              ) ?? entry.provider_id;
                            return `${label}: ${t(
                              `settings.providerReadiness.status.${entry.status}`,
                            )}`;
                          })
                          .join(" • ")}
                      </p>
                    )
                  )}
                  <div className="settings-credential-health__actions">
                    {route && (
                      <button
                        type="button"
                        className="settings-btn settings-btn--secondary"
                        onClick={() => handleOpenCredentialKey(credential.key)}
                      >
                        {t("settings.credentialHealth.replace")}
                      </button>
                    )}
                    <button
                      type="button"
                      className="settings-btn settings-btn--secondary"
                      onClick={() =>
                        void refreshProviderReadiness({ force: true })
                      }
                      disabled={providerReadinessLoading}
                    >
                      {t("settings.credentialHealth.retest")}
                    </button>
                    {/* Destructive Clear lives behind a ⋯ overflow with a
                        confirm (handleClearCredential prompts) so it is not a
                        one-click mistake (blueprint §1.2 / §4). */}
                    <details className="settings-credential-health__overflow">
                      <summary
                        className="settings-btn settings-btn--ghost"
                        aria-label={t("settings.credentialHealth.moreActions")}
                      >
                        ⋯
                      </summary>
                      <div className="settings-credential-health__overflow-menu">
                        <button
                          type="button"
                          className="settings-btn settings-btn--danger"
                          onClick={() =>
                            void handleClearCredential(
                              credential.key,
                              credential.key,
                              () => {},
                            )
                          }
                        >
                          {t("settings.credentialHealth.clear")}
                        </button>
                      </div>
                    </details>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </section>
    </>
  );
}
