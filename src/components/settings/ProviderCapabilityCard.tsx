/**
 * A single registry-backed provider-capability card (blueprint §1.2, Phase 4).
 *
 * Extracted from the inline Overview block (behavior-preserving in STEP 1). The
 * markup — `.settings-provider-capability-card`, the badges, the `<dl>` of
 * capability rows, the readiness line, and the Select/Open-settings action — is
 * relocated verbatim so the capability-card tests keep passing against the same
 * DOM. Reads readiness/route/credential data from the settings controller via
 * `useSettings()`; the parent passes only the `descriptor` and its `stage`.
 */

import type { ProviderDescriptor } from "../../types";
import { providerCatalogSummary } from "../ProviderReadinessPanel";
import {
  modelCatalogForProvider,
  providerCapabilityCredentialLabel,
  providerCredentialKeysLabel,
  providerNotSelectableLabel,
  providerRoadmapAuthLabel,
  providerStatusLabel,
} from "../providerRegistryHelpers";
import { useSettings } from "./SettingsContext";
import {
  providerAudioFormatLabel,
  providerAudioTransportEncodingLabel,
  providerAuthLifecycleLabel,
  providerCapabilityBooleanLabel,
  providerCapabilityCatalogCountLabel,
  providerCloseLifecycleLabel,
  providerDataBoundaryMetadataLabel,
  providerDefaultModelLabel,
  providerEndpointModesLabel,
  providerEventSemanticsLabel,
  providerGeneratedCatalogKind,
  providerHealthProbesLabel,
  providerKeepaliveLabel,
  providerModelCatalogPolicyLabel,
  providerPlatformBlockersLabel,
  providerRuntimePackagingLabel,
  providerRuntimeReadinessLabel,
  providerSessionLifecycleLabel,
  providerSourcePolicyLabel,
  providerSpeakerSemanticsLabel,
  providerTransportLabel,
} from "./useSettingsController";

export default function ProviderCapabilityCard({
  descriptor,
  stageLabel,
}: {
  descriptor: ProviderDescriptor;
  /** Human-facing stage label (e.g. "ASR") shown in the card's Stage row. */
  stageLabel: string;
}) {
  const {
    t,
    providerReadiness,
    providerRouteForProviderId,
    activeReadinessProviderIdSet,
    credentialPresence,
    openSettingsControlRoute,
  } = useSettings();

  const readiness = providerReadiness[descriptor.id] ?? null;
  const providerRoute = providerRouteForProviderId(descriptor.id);
  const selectable =
    descriptor.status === "implemented" && providerRoute != null;
  const nonImplemented = descriptor.status !== "implemented";
  const selectabilityStatus = nonImplemented
    ? "planned"
    : selectable
      ? "selectable"
      : "setup";
  const selectabilityLabel = nonImplemented
    ? providerStatusLabel(descriptor.status)
    : selectable
      ? "Selectable"
      : "Readiness only";
  const selected = activeReadinessProviderIdSet.has(descriptor.id);
  const readinessStatus = readiness?.status ?? "unchecked";
  const readinessLabel = readiness
    ? t(`settings.providerReadiness.status.${readiness.status}`)
    : "Not checked";
  const backendCatalogSummary = readiness
    ? providerCatalogSummary(readiness)
    : null;
  const generatedCatalogCount = modelCatalogForProvider(
    providerReadiness,
    descriptor.id,
  ).length;
  const catalogCount =
    backendCatalogSummary?.count ??
    (generatedCatalogCount > 0 ? generatedCatalogCount : null);
  const catalogKind =
    backendCatalogSummary?.kind ?? providerGeneratedCatalogKind(descriptor);

  return (
    <article
      className={`settings-provider-capability-card ${
        selected ? "settings-provider-capability-card--selected" : ""
      } ${nonImplemented ? "settings-provider-capability-card--planned" : ""}`}
      aria-labelledby={`settings-provider-capability-${descriptor.id}`}
    >
      <div className="settings-provider-capability-card__header">
        <div>
          <h5
            id={`settings-provider-capability-${descriptor.id}`}
            className="settings-provider-capability-card__title"
          >
            {descriptor.display_name}
          </h5>
          <p className="settings-provider-capability-card__id">
            {descriptor.id}
          </p>
        </div>
        <div className="settings-provider-capability-card__badges">
          {selected && (
            <span className="settings-provider-capability-card__badge settings-provider-capability-card__badge--selected">
              Selected
            </span>
          )}
          <span
            className={`settings-provider-capability-card__badge settings-provider-capability-card__badge--${selectabilityStatus}`}
          >
            {selectabilityLabel}
          </span>
          <span
            className={`settings-provider-capability-card__badge settings-provider-capability-card__badge--${readinessStatus}`}
          >
            {readinessLabel}
          </span>
        </div>
      </div>

      <dl className="settings-provider-capability-card__details">
        <div>
          <dt>Stage</dt>
          <dd>{stageLabel}</dd>
        </div>
        <div>
          <dt>Streaming</dt>
          <dd>
            {providerCapabilityBooleanLabel(descriptor.supports_streaming)}
          </dd>
        </div>
        <div>
          <dt>Partial revisions</dt>
          <dd>
            {providerCapabilityBooleanLabel(
              descriptor.supports_partial_revisions,
            )}
          </dd>
        </div>
        <div>
          <dt>Diarization</dt>
          <dd>
            {providerCapabilityBooleanLabel(descriptor.supports_diarization)}
          </dd>
        </div>
        <div>
          <dt>Pipeline audio</dt>
          <dd>
            {providerAudioFormatLabel(descriptor.audio_input?.pipeline_format)}
          </dd>
        </div>
        <div>
          <dt>Provider audio</dt>
          <dd>
            {providerAudioFormatLabel(descriptor.audio_input?.provider_format)}
          </dd>
        </div>
        <div>
          <dt>Wire encoding</dt>
          <dd>
            {providerAudioTransportEncodingLabel(
              descriptor.audio_input?.transport_encoding,
            )}
          </dd>
        </div>
        <div>
          <dt>Resampling</dt>
          <dd>
            {descriptor.audio_input
              ? descriptor.audio_input.adapter_resamples
                ? "Adapter resamples"
                : "No adapter resampling"
              : "Not declared"}
          </dd>
        </div>
        <div>
          <dt>Multichannel</dt>
          <dd>
            {descriptor.audio_input
              ? providerCapabilityBooleanLabel(
                  descriptor.audio_input.supports_multichannel,
                )
              : "Not declared"}
          </dd>
        </div>
        <div>
          <dt>Events</dt>
          <dd>{providerEventSemanticsLabel(descriptor.event_semantics)}</dd>
        </div>
        <div>
          <dt>Source policy</dt>
          <dd>{providerSourcePolicyLabel(descriptor.source_policy)}</dd>
        </div>
        <div>
          <dt>Auth</dt>
          <dd>{providerAuthLifecycleLabel(descriptor.lifecycle.auth)}</dd>
        </div>
        <div>
          <dt>Credential keys</dt>
          <dd>{providerCredentialKeysLabel(descriptor)}</dd>
        </div>
        <div>
          <dt>Credential state</dt>
          <dd>
            {providerCapabilityCredentialLabel(descriptor, credentialPresence)}
          </dd>
        </div>
        {descriptor.roadmap && (
          <div>
            <dt>Roadmap auth</dt>
            <dd>{providerRoadmapAuthLabel(descriptor) ?? "Not declared"}</dd>
          </div>
        )}
        {descriptor.roadmap && (
          <div>
            <dt>Roadmap source</dt>
            <dd>
              {descriptor.roadmap.source_date}
              {descriptor.roadmap.source_url
                ? ` ${descriptor.roadmap.source_url}`
                : ""}
            </dd>
          </div>
        )}
        <div>
          <dt>Transport</dt>
          <dd>{providerTransportLabel(descriptor.transport)}</dd>
        </div>
        <div>
          <dt>Session</dt>
          <dd>{providerSessionLifecycleLabel(descriptor.lifecycle.session)}</dd>
        </div>
        <div>
          <dt>Keepalive</dt>
          <dd>{providerKeepaliveLabel(descriptor.lifecycle.keepalive)}</dd>
        </div>
        <div>
          <dt>Close</dt>
          <dd>{providerCloseLifecycleLabel(descriptor.lifecycle.close)}</dd>
        </div>
        <div>
          <dt>Model catalog</dt>
          <dd>{providerModelCatalogPolicyLabel(descriptor.model_catalog)}</dd>
        </div>
        <div>
          <dt>Default model</dt>
          <dd>{providerDefaultModelLabel(descriptor)}</dd>
        </div>
        <div>
          <dt>Catalog count</dt>
          <dd>
            {providerCapabilityCatalogCountLabel(catalogCount, catalogKind)}
          </dd>
        </div>
        <div>
          <dt>Data boundary</dt>
          <dd>
            {providerDataBoundaryMetadataLabel(
              descriptor.privacy.data_boundary,
            )}
          </dd>
        </div>
        <div>
          <dt>Endpoint modes</dt>
          <dd>{providerEndpointModesLabel(descriptor.enterprise)}</dd>
        </div>
        <div>
          <dt>Runtime packaging</dt>
          <dd>{providerRuntimePackagingLabel(descriptor)}</dd>
        </div>
        <div>
          <dt>Speaker labels</dt>
          <dd>{providerSpeakerSemanticsLabel(descriptor.enterprise)}</dd>
        </div>
        <div>
          <dt>Health probes</dt>
          <dd>{providerHealthProbesLabel(descriptor)}</dd>
        </div>
        <div>
          <dt>Platform blockers</dt>
          <dd>{providerPlatformBlockersLabel(descriptor)}</dd>
        </div>
        <div>
          <dt>Readiness</dt>
          <dd>{readinessLabel}</dd>
        </div>
        <div>
          <dt>Runtime</dt>
          <dd>
            {readiness?.runtime
              ? `${providerRuntimeReadinessLabel(
                  readiness.runtime.status,
                )}: ${readiness.runtime.message}`
              : "Not reported"}
          </dd>
        </div>
      </dl>

      <p className="settings-provider-capability-card__readiness">
        {readiness?.message ?? "No readiness check has run for this provider."}
      </p>

      <div className="settings-provider-capability-card__actions">
        {selectable && providerRoute ? (
          <button
            type="button"
            className="settings-btn settings-btn--secondary"
            aria-label={`Select ${descriptor.display_name}`}
            onClick={() => openSettingsControlRoute(providerRoute)}
          >
            {selected ? "Open settings" : "Select"}
          </button>
        ) : (
          <span className="settings-provider-capability-card__planned-note">
            {providerNotSelectableLabel(descriptor)}
          </span>
        )}
      </div>
    </article>
  );
}
