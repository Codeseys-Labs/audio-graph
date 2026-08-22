/**
 * Product-mode overview cards (blueprint §1.1, Phase 4).
 *
 * STEP 1 extraction is behavior-preserving: the `.settings-mode-overview`
 * section markup (the 4 Local/Cloud/Hybrid/Native cards with data-boundary,
 * affected stages, per-stage providers, blockers, and the Provider/Credential/
 * Model/Sources action buttons) is relocated verbatim from the inline Overview
 * block so the mode-overview tests keep passing against the same DOM. Reads
 * everything it needs from the settings controller via `useSettings()`.
 */

import { useTranslation } from "react-i18next";
import { modeReadinessTone } from "./badgeTone";
import { readinessChipTone } from "./readinessTone";
import { useSettings } from "./SettingsContext";
import {
  providerSetupBlockerKindLabel,
  providerSetupCardHasSourceBlocker,
  providerSetupDataBoundaryLabel,
  providerSetupStageLabel,
  providerSetupStatusLabel,
} from "./useSettingsController";

export default function ProductModeSummaryCards() {
  const { t } = useTranslation();
  const {
    providerSetupModeCards,
    providerSetupProviderRoute,
    providerSetupCredentialRoute,
    providerSetupModelRoute,
    providerRouteForProviderId,
    openSettingsControlRoute,
    handleProviderSetupSourceRecovery,
    handleSelectProductMode,
  } = useSettings();

  return (
    <section
      className="settings-mode-overview"
      aria-labelledby="settings-mode-overview-title"
    >
      <div className="settings-mode-overview__header">
        <h3
          id="settings-mode-overview-title"
          className="settings-mode-overview__title"
        >
          {t("settings.modes.title")}
        </h3>
      </div>
      <div className="settings-mode-overview__grid">
        {providerSetupModeCards.map((card) => {
          const providerRoute = providerSetupProviderRoute(card);
          const credentialRoute = providerSetupCredentialRoute(card);
          const modelRoute = providerSetupModelRoute(card);
          const hasSourceBlocker = providerSetupCardHasSourceBlocker(card);
          // The tone LAW (audio-graph-2554, settings T2): a mode card's
          // aggregate status may claim "Ready" only for the mode that is
          // ACTUALLY the one running (`card.selected` stands in for Axis 3 at
          // this granularity — a candidate mode nobody has switched to is
          // not "actively probed" no matter what its providers' cached
          // readiness says). `modeReadinessTone`/`providerSetupStatusLabel`
          // still own the concrete tone/copy for every other status
          // (including "blocked", which the shared law doesn't know about).
          // `readinessChipTone` (settings T3, audio-graph-9d2b) is the one
          // seam that folds `forceNeutral` into the tone instead of every
          // caller hand-applying the same branch.
          const readinessChip = readinessChipTone(
            { status: card.readinessStatus, active: card.selected },
            modeReadinessTone,
          );

          return (
            <article
              key={card.id}
              className={`ag-card settings-mode-card ${
                card.selected ? "settings-mode-card--selected" : ""
              }`}
              data-elevation="flat"
              aria-labelledby={`settings-mode-card-${card.id}`}
            >
              <div className="settings-mode-card__header">
                <div>
                  <h4
                    id={`settings-mode-card-${card.id}`}
                    className="settings-mode-card__title"
                  >
                    {card.label}
                  </h4>
                  {/* Glanceable density (blueprint §1.1): the data-boundary value
                      rides inline next to the title as a chip instead of a
                      verbose "Data boundary"/"Affected stages" definition list —
                      the affected stages are already enumerated by the per-stage
                      rollup rows below, so the dl was duplicate dense text. */}
                  <p className="settings-mode-card__meta">
                    <span className="settings-mode-card__boundary">
                      {providerSetupDataBoundaryLabel(card.dataBoundary)}
                    </span>
                  </p>
                </div>
                <div className="settings-mode-card__badges">
                  {card.selected && (
                    <span className="ag-chip" data-tone="accent">
                      Selected
                    </span>
                  )}
                  {!card.uiSelectable && (
                    <span className="ag-chip" data-tone="warning">
                      {t("settings.modes.notInMvp")}
                    </span>
                  )}
                  {card.uiSelectable && readinessChip.render && (
                    <span className="ag-chip" data-tone={readinessChip.tone}>
                      {providerSetupStatusLabel(readinessChip.effectiveStatus)}
                    </span>
                  )}
                </div>
              </div>

              <ul className="settings-mode-card__providers">
                {card.stageCoverage.map((coverage) => {
                  // Summary-that-links (blueprint §1.1): each per-stage rollup
                  // deep-links into the provider section it summarises rather
                  // than inlining config. Fall back to a static row when the
                  // stage has no routable provider (e.g. an empty coverage).
                  // Saved deferred routes remain inspectable even though they
                  // cannot be selected or started (ADR-0033). Navigating to a
                  // provider's settings is recovery, not runtime enablement.
                  const stageRoute = providerRouteForProviderId(
                    coverage.providerId,
                  );
                  const rowContent = (
                    <>
                      <span className="settings-mode-card__stage">
                        {providerSetupStageLabel(coverage)}
                      </span>
                      <span className="settings-mode-card__provider-name">
                        {coverage.providerName}
                      </span>
                      {coverage.model && (
                        <span className="settings-mode-card__model">
                          {coverage.model}
                        </span>
                      )}
                    </>
                  );
                  return (
                    <li
                      key={`${card.id}-${coverage.path}-${coverage.providerId}`}
                      className="settings-mode-card__provider"
                    >
                      {stageRoute ? (
                        <button
                          type="button"
                          className="settings-mode-card__provider-link"
                          aria-label={`Open ${coverage.providerName} ${providerSetupStageLabel(
                            coverage,
                          )} settings`}
                          onClick={() => openSettingsControlRoute(stageRoute)}
                        >
                          {rowContent}
                        </button>
                      ) : (
                        rowContent
                      )}
                    </li>
                  );
                })}
              </ul>

              {/* Glanceable density (blueprint §1.1): blockers collapse to an
                  inline status line — "No blockers" when clear, otherwise the
                  kind-tagged message rows — dropping the standalone "Blockers"
                  subhead. The Provider/Credential/Model/Sources action buttons
                  below remain the deep-link affordance for resolving them. */}
              <div className="settings-mode-card__blockers">
                {!card.uiSelectable && (
                  <p className="settings-mode-card__empty">
                    {t("settings.modes.notInMvpDetail")}
                  </p>
                )}
                {card.uiSelectable && card.missingBlockers.length === 0 ? (
                  <p className="settings-mode-card__empty">No blockers</p>
                ) : card.missingBlockers.length > 0 ? (
                  <ul>
                    {card.missingBlockers.map((blocker) => (
                      <li
                        key={`${card.id}-${blocker.providerId}-${blocker.kind}-${blocker.key ?? blocker.model ?? blocker.message}`}
                      >
                        <span>
                          {providerSetupBlockerKindLabel(blocker.kind)}:
                        </span>{" "}
                        {blocker.message}
                      </li>
                    ))}
                  </ul>
                ) : null}
              </div>

              <div className="settings-mode-card__actions">
                {/* Interactive mode selection (settings redesign WS1): the
                    toggle-button pattern from ConversationModeControl —
                    aria-pressed reflects the derived `selected` state, disabled
                    when already active. Avoids role="radio" on styled buttons
                    (biome useSemanticElements) while exposing pressed state to
                    assistive tech. */}
                <button
                  type="button"
                  className="settings-btn settings-btn--primary"
                  aria-pressed={card.selected}
                  aria-label={`${card.label}: ${t(
                    card.uiSelectable
                      ? "settings.modes.useThisMode"
                      : "settings.modes.notInMvp",
                  )}`}
                  disabled={card.selected || !card.uiSelectable}
                  onClick={() => handleSelectProductMode(card)}
                >
                  {card.uiSelectable
                    ? t("settings.modes.useThisMode")
                    : t("settings.modes.notInMvp")}
                </button>
                {providerRoute && (
                  <button
                    type="button"
                    className="settings-btn settings-btn--secondary"
                    aria-label={`Configure ${card.label} provider`}
                    onClick={() => openSettingsControlRoute(providerRoute)}
                  >
                    Provider
                  </button>
                )}
                {credentialRoute && (
                  <button
                    type="button"
                    className="settings-btn settings-btn--secondary"
                    aria-label={`Fix ${card.label} credential`}
                    onClick={() => openSettingsControlRoute(credentialRoute)}
                  >
                    Credential
                  </button>
                )}
                {modelRoute && (
                  <button
                    type="button"
                    className="settings-btn settings-btn--secondary"
                    aria-label={`Choose ${card.label} model`}
                    onClick={() => openSettingsControlRoute(modelRoute)}
                  >
                    Model
                  </button>
                )}
                {hasSourceBlocker && (
                  <button
                    type="button"
                    className="settings-btn settings-btn--secondary"
                    aria-label={`Review ${card.label} source selection`}
                    onClick={() => handleProviderSetupSourceRecovery(card)}
                  >
                    Sources
                  </button>
                )}
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}
