/**
 * Credentials rail section (blueprint §1.2 / §7, Phase 4).
 *
 * Promotes the per-key credential-health rows out of the dense Overview into
 * their own first-class section. Each row gains a scannable status chip
 * (Ready / Needs validation / Failing / Unused) derived from the linked
 * provider readiness, keeps Replace/Retest inline, and moves the destructive
 * Clear behind a `⋯` overflow with consequence copy (handled by the existing
 * `handleClearCredential` confirm). Reads everything from `useSettings()`.
 *
 * The `.settings-credential-health__item` markup + `<code>{key}</code>` shape is
 * preserved so deep-link routing and the credential tests keep resolving.
 */

import type { ProviderReadiness } from "../../types";
import { credentialSourceLabel } from "../ProviderReadinessPanel";
import Badge, { type BadgeTone } from "./Badge";
import { useSettings } from "./SettingsContext";
import {
  formatCredentialCheckedAt,
  PROVIDER_READINESS_LABELS,
} from "./useSettingsController";

type CredentialChip = "ready" | "needsValidation" | "failing" | "unused";

const CREDENTIAL_CHIP_TONE: Record<CredentialChip, BadgeTone> = {
  ready: "success",
  needsValidation: "warning",
  failing: "danger",
  unused: "neutral",
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
  } = useSettings();

  return (
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
            const chip = credentialStatusChip(relatedReadiness);

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
                {relatedReadiness.length > 0 && (
                  <p className="settings-credential-health__providers">
                    {t("settings.credentialHealth.providerStatus")}{" "}
                    {relatedReadiness
                      .map((entry) => {
                        const label =
                          PROVIDER_READINESS_LABELS.get(entry.provider_id) ??
                          entry.provider_id;
                        return `${label}: ${t(
                          `settings.providerReadiness.status.${entry.status}`,
                        )}`;
                      })
                      .join(" • ")}
                  </p>
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
  );
}
