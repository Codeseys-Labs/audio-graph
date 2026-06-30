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

import Badge, { modeReadinessTone } from "./Badge";
import { useSettings } from "./SettingsContext";
import {
  providerSetupBlockerKindLabel,
  providerSetupCardHasSourceBlocker,
  providerSetupDataBoundaryLabel,
  providerSetupStageLabel,
  providerSetupStatusLabel,
} from "./useSettingsController";

export default function ProductModeSummaryCards() {
  const {
    providerSetupModeCards,
    providerSetupProviderRoute,
    providerSetupCredentialRoute,
    providerSetupModelRoute,
    providerRouteForProviderId,
    openSettingsControlRoute,
    handleProviderSetupSourceRecovery,
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
          Product mode overview
        </h3>
      </div>
      <div className="settings-mode-overview__grid">
        {providerSetupModeCards.map((card) => {
          const providerRoute = providerSetupProviderRoute(card);
          const credentialRoute = providerSetupCredentialRoute(card);
          const modelRoute = providerSetupModelRoute(card);
          const hasSourceBlocker = providerSetupCardHasSourceBlocker(card);

          return (
            <article
              key={card.id}
              className={`settings-mode-card ${
                card.selected ? "settings-mode-card--selected" : ""
              }`}
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
                  {card.selected && <Badge tone="accent">Selected</Badge>}
                  <Badge tone={modeReadinessTone(card.readinessStatus)}>
                    {providerSetupStatusLabel(card.readinessStatus)}
                  </Badge>
                </div>
              </div>

              <ul className="settings-mode-card__providers">
                {card.stageCoverage.map((coverage) => {
                  // Summary-that-links (blueprint §1.1): each per-stage rollup
                  // deep-links into the provider section it summarises rather
                  // than inlining config. Fall back to a static row when the
                  // stage has no routable provider (e.g. an empty coverage).
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
                {card.missingBlockers.length === 0 ? (
                  <p className="settings-mode-card__empty">No blockers</p>
                ) : (
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
                )}
              </div>

              <div className="settings-mode-card__actions">
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
