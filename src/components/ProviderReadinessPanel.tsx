import type { TFunction } from "i18next";
import { useId } from "react";
import type {
  CredentialPresence,
  ProviderDataClass,
  ProviderDescriptor,
  ProviderPolicyStatus,
  ProviderReadiness,
  ProviderSensitiveErrorPolicy,
} from "../types";
import { providerRoadmapAuthLabel } from "./providerRegistryHelpers";

export type CredentialPresenceLookup = Partial<
  Record<string, CredentialPresence>
>;

interface ProviderReadinessPanelProps {
  entry: ProviderReadiness | null;
  descriptor?: ProviderDescriptor | null;
  credentialPresence?: CredentialPresenceLookup;
  loading: boolean;
  t: TFunction;
}

type ProviderCatalogKind = "models" | "voices" | "languages";

interface ProviderCatalogSummary {
  count: number;
  kind: ProviderCatalogKind;
}

export function providerCatalogSummary(
  entry: ProviderReadiness,
): ProviderCatalogSummary | null {
  const voiceCatalogCount = entry.voice_catalog?.length ?? 0;
  if (voiceCatalogCount > 0) {
    return { count: voiceCatalogCount, kind: "voices" };
  }

  const languageCatalogCount = entry.language_catalog?.length ?? 0;
  if (languageCatalogCount > 0) {
    return { count: languageCatalogCount, kind: "languages" };
  }

  if (typeof entry.model_count === "number") {
    return { count: entry.model_count, kind: "models" };
  }

  const modelCatalogCount = entry.model_catalog?.length ?? 0;
  if (modelCatalogCount > 0) {
    return { count: modelCatalogCount, kind: "models" };
  }

  const openrouterModelCount = entry.openrouter_models?.length ?? 0;
  return openrouterModelCount > 0
    ? { count: openrouterModelCount, kind: "models" }
    : null;
}

export function providerCatalogLabel(
  summary: ProviderCatalogSummary | null,
  t: TFunction,
): string | null {
  return summary
    ? t(`settings.providerReadiness.${summary.kind}`, {
        count: summary.count,
      })
    : null;
}

function hasCredentialSlots(entry: ProviderReadiness): boolean {
  return entry.credentials.length > 0;
}

function hasPresentCredential(entry: ProviderReadiness): boolean {
  return entry.credentials.some((credential) => credential.present);
}

function formatCheckedAt(value: number | null | undefined): string | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
    return null;
  }
  return new Date(value).toLocaleString();
}

function providerDataClassLabel(dataClass: ProviderDataClass, t: TFunction) {
  return t(`settings.providerReadiness.dataClass.${dataClass}`);
}

function providerDataClassListLabel(
  dataClasses: readonly ProviderDataClass[],
  t: TFunction,
): string {
  if (dataClasses.length === 0) {
    return t("settings.providerReadiness.dataClass.none");
  }

  return dataClasses
    .map((dataClass) => providerDataClassLabel(dataClass, t))
    .join(", ");
}

function providerPolicyStatusLabel(
  status: ProviderPolicyStatus,
  t: TFunction,
): string {
  return t(`settings.providerReadiness.policyStatus.${status}`);
}

function providerSensitiveErrorPolicyLabel(
  policy: ProviderSensitiveErrorPolicy,
  t: TFunction,
): string {
  return t(`settings.providerReadiness.sensitiveErrorPolicy.${policy}`);
}

// Backend `CredentialPresence.source` values that have a localized label.
// `os_keychain`/`imported_file`/`file_fallback`/`file_override`/
// `credentials_yaml`/`missing` are all emitted by `source_for()` on the live
// `load_credential_presence_cmd` IPC path (and the parity is enforced by
// `credentialSourceContract.test.ts`). `error` is the one exception: it is a
// DEFENSIVE UI fallback only — the live load path returns
// `AppError::CredentialFileError` on a read failure rather than a presence row
// with `source: "error"`, so the backend never actually emits it today. The
// label is kept so a future backend that does surface a per-key read error has
// a localized string instead of a raw passthrough. (The only `"source":
// "error"` literal in `credentials/mod.rs` lives inside `#[cfg(test)]`, which
// the contract test strips, so it is not part of the live source vocabulary.)
const LOCALIZED_CREDENTIAL_SOURCES = new Set([
  "os_keychain",
  "imported_file",
  "file_fallback",
  "file_override",
  "credentials_yaml",
  "missing",
  "error",
]);

export function credentialSourceLabel(
  source: string | undefined,
  t: TFunction,
): string {
  if (source && LOCALIZED_CREDENTIAL_SOURCES.has(source)) {
    return t(`settings.providerReadiness.credentialSource.${source}`);
  }
  return (
    source?.trim() || t("settings.providerReadiness.credentialSource.unknown")
  );
}

// Stable text prefix the backend wraps around a provider probe's HTTP 401
// (Unauthorized) detail message before it reaches `ProviderReadiness.message`
// — mirrors `crate::error::CREDENTIAL_REJECTED_PREFIX` (src-tauri/src/error.rs)
// and every 401-capable probe path that routes through
// `classify_credential_rejected_message` (Deepgram, Soniox, the generic
// OpenAI-compatible arm, AssemblyAI, Gemini, OpenRouter). A stable PREFIX
// (checked with `startsWith`, not just a "401" substring search) is
// deliberate: a provider error body can coincidentally contain "401" in
// unrelated data (a request id, a project id, ...), so a substring match
// would be a false-positive-prone classifier. This is the same
// stable-prefix-over-substring design `isCredentialFileParseError` (App.tsx)
// already uses for the `Failed to parse ` credential-file-parse marker.
// (audio-graph-57cc)
const CREDENTIAL_REJECTED_PREFIX = "Credential rejected (401):";

export function isCredentialRejectedReadinessMessage(message: string): boolean {
  return message.startsWith(CREDENTIAL_REJECTED_PREFIX);
}

export function providerRecoveryAction(
  entry: ProviderReadiness,
  t: TFunction,
  descriptor?: ProviderDescriptor | null,
): string | null {
  switch (entry.status) {
    case "missing_credentials":
      return t("settings.providerReadiness.recovery.missingCredentials");
    case "error":
      if (isCredentialRejectedReadinessMessage(entry.message)) {
        return t("settings.providerReadiness.recovery.credentialRejected");
      }
      return hasPresentCredential(entry)
        ? t("settings.providerReadiness.recovery.errorWithCredentials")
        : t("settings.providerReadiness.recovery.errorWithoutCredentials");
    case "unchecked":
      if (!hasCredentialSlots(entry) || !hasPresentCredential(entry)) {
        return null;
      }
      if (entry.automatic_probe_available === false) {
        return null;
      }
      if (
        entry.automatic_probe_available !== true &&
        descriptor &&
        !descriptor.health_check_command
      ) {
        return null;
      }
      return t("settings.providerReadiness.recovery.uncheckedWithCredentials");
    case "ready":
      return null;
  }
}

interface ProviderReadinessDetailsProps {
  entry: ProviderReadiness;
  credentialPresence?: CredentialPresenceLookup;
  t: TFunction;
}

export function ProviderReadinessDetails({
  entry,
  credentialPresence,
  t,
}: ProviderReadinessDetailsProps) {
  const catalogSummary = providerCatalogSummary(entry);

  return (
    <details className="settings-provider-readiness__details">
      <summary>{t("settings.providerReadiness.details")}</summary>
      <dl className="settings-provider-readiness__detail-grid">
        <div>
          <dt>{t("settings.providerReadiness.lastChecked")}</dt>
          <dd>
            {formatCheckedAt(entry.checked_at) ??
              t("settings.providerReadiness.notChecked")}
          </dd>
        </div>
        <div>
          <dt>{t("settings.providerReadiness.catalog")}</dt>
          <dd>
            {providerCatalogLabel(catalogSummary, t) ??
              t("settings.providerReadiness.noCatalog")}
          </dd>
        </div>
        {entry.runtime && (
          <div>
            <dt>{t("settings.providerReadiness.runtime")}</dt>
            <dd>
              {t(
                `settings.providerReadiness.runtimeStatus.${entry.runtime.status}`,
              )}
              {`: ${entry.runtime.message}`}
            </dd>
          </div>
        )}
      </dl>
      {entry.credentials.length > 0 && (
        <dl className="settings-provider-readiness__credential-list">
          <dt>{t("settings.providerReadiness.credentials")}</dt>
          {entry.credentials.map((credential) => {
            const presence = credentialPresence?.[credential.key];
            const present = presence?.present ?? credential.present;
            const source = presence?.source?.trim()
              ? presence.source
              : present
                ? undefined
                : "missing";
            return (
              <dd key={credential.key}>
                <code>{credential.key}</code>
                <span>
                  {present
                    ? t("settings.providerReadiness.credentialPresent")
                    : t("settings.providerReadiness.credentialMissing")}
                </span>
                <span>{credentialSourceLabel(source, t)}</span>
              </dd>
            );
          })}
        </dl>
      )}
    </details>
  );
}

export default function ProviderReadinessPanel({
  entry,
  descriptor,
  credentialPresence,
  loading,
  t,
}: ProviderReadinessPanelProps) {
  // Stable id so the free-form backend not-selectable reason is programmatically
  // associated with the roadmap status rather than appended as a bare string.
  // Declared before the early return so the hook order stays unconditional.
  const notSelectableReasonId = useId();

  if (!entry && !descriptor && !loading) return null;

  const catalogSummary = entry ? providerCatalogSummary(entry) : null;
  const recoveryAction = entry
    ? providerRecoveryAction(entry, t, descriptor)
    : null;

  return (
    <div
      className={`settings-provider-readiness ${
        entry ? `settings-provider-readiness--${entry.status}` : ""
      }`}
    >
      <div
        className="settings-provider-readiness__status"
        role="status"
        aria-live="polite"
        aria-atomic="true"
        aria-busy={loading}
      >
        <div className="settings-provider-readiness__main">
          <span className="settings-provider-readiness__label">
            {entry
              ? t(`settings.providerReadiness.status.${entry.status}`)
              : loading
                ? t("settings.providerReadiness.checking")
                : t("settings.providerReadiness.status.unchecked")}
          </span>
          {loading && (
            <span className="settings-provider-readiness__checking">
              {t("settings.providerReadiness.checking")}
            </span>
          )}
        </div>
        {entry && (
          <p className="settings-provider-readiness__message">
            {entry.message}
            {entry.stale ? ` ${t("settings.providerReadiness.stale")}` : ""}
            {catalogSummary
              ? ` ${providerCatalogLabel(catalogSummary, t)}`
              : ""}
          </p>
        )}
      </div>
      {recoveryAction && (
        <p className="settings-provider-readiness__recovery">
          <span>{t("settings.providerReadiness.recoveryLabel")}</span>{" "}
          {recoveryAction}
        </p>
      )}
      {entry && (
        <ProviderReadinessDetails
          entry={entry}
          credentialPresence={credentialPresence}
          t={t}
        />
      )}
      {descriptor && (
        <dl className="settings-provider-readiness__metadata">
          <div>
            <dt>{t("settings.providerReadiness.metadata.data")}</dt>
            <dd>
              {t(
                `settings.providerReadiness.dataBoundary.${descriptor.privacy.data_boundary}`,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.sent")}</dt>
            <dd>
              {providerDataClassListLabel(
                descriptor.privacy.data_classes_sent,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.returned")}</dt>
            <dd>
              {providerDataClassListLabel(
                descriptor.privacy.data_classes_returned,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.healthCheck")}</dt>
            <dd>
              {providerDataClassListLabel(
                descriptor.privacy.health_check_data_classes,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.cloudTransfer")}</dt>
            <dd>
              {descriptor.privacy.cloud_transfer_acknowledgement_required
                ? t("settings.providerReadiness.cloudTransfer.required")
                : t("settings.providerReadiness.cloudTransfer.notRequired")}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.retention")}</dt>
            <dd>
              {providerPolicyStatusLabel(
                descriptor.privacy.retention_policy,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.training")}</dt>
            <dd>
              {providerPolicyStatusLabel(descriptor.privacy.training_policy, t)}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.deletion")}</dt>
            <dd>
              {providerPolicyStatusLabel(descriptor.privacy.deletion_policy, t)}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.residency")}</dt>
            <dd>
              {providerPolicyStatusLabel(descriptor.privacy.data_residency, t)}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.noTraining")}</dt>
            <dd>
              {providerPolicyStatusLabel(
                descriptor.privacy.enterprise_no_training_config,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.errors")}</dt>
            <dd>
              {providerSensitiveErrorPolicyLabel(
                descriptor.privacy.sensitive_error_policy,
                t,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.processor")}</dt>
            <dd>
              {descriptor.privacy.processor_identity ??
                t("settings.providerReadiness.unknown")}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.policy")}</dt>
            <dd>
              {descriptor.privacy.policy_url ??
                t("settings.providerReadiness.noPolicyUrl")}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.session")}</dt>
            <dd>
              {t(
                `settings.providerReadiness.session.${descriptor.lifecycle.session}`,
              )}
            </dd>
          </div>
          <div>
            <dt>{t("settings.providerReadiness.metadata.auth")}</dt>
            <dd>
              {providerRoadmapAuthLabel(descriptor) ??
                t(
                  `settings.providerReadiness.auth.${descriptor.lifecycle.auth}`,
                )}
            </dd>
          </div>
          {descriptor.roadmap && (
            <div>
              <dt>{t("settings.providerReadiness.roadmap")}</dt>
              <dd
                aria-describedby={
                  descriptor.roadmap.not_selectable_reason
                    ? notSelectableReasonId
                    : undefined
                }
              >
                <span>
                  {t(
                    `settings.providerReadiness.roadmapStatus.${descriptor.status}`,
                  )}
                </span>
                {descriptor.roadmap.not_selectable_reason && (
                  <span id={notSelectableReasonId}>
                    {" "}
                    <span className="settings-provider-readiness__not-selectable-label">
                      {t("settings.providerReadiness.notSelectableReasonLabel")}
                    </span>{" "}
                    {descriptor.roadmap.not_selectable_reason}
                  </span>
                )}
              </dd>
            </div>
          )}
          {descriptor.lifecycle.keepalive !== "none" && (
            <div>
              <dt>{t("settings.providerReadiness.metadata.keepalive")}</dt>
              <dd>
                {t(
                  `settings.providerReadiness.keepalive.${descriptor.lifecycle.keepalive}`,
                )}
              </dd>
            </div>
          )}
          {descriptor.lifecycle.close !== "noop" && (
            <div>
              <dt>{t("settings.providerReadiness.metadata.close")}</dt>
              <dd>
                {t(
                  `settings.providerReadiness.close.${descriptor.lifecycle.close}`,
                )}
              </dd>
            </div>
          )}
        </dl>
      )}
    </div>
  );
}
