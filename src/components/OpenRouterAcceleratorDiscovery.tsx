/**
 * OpenRouter accelerator DISCOVERY surface (seed 7809).
 *
 * Replaces the hardcoded `"cerebras, groq"` provider-order default in
 * `LlmProviderSettings` as the source of truth for strict-accelerator routing.
 * Instead of a baked-in list, this panel:
 *   1. fetches the live endpoint + provider catalog via the SAVED-KEY commands
 *      (`list_openrouter_model_endpoints_cmd` + `list_openrouter_providers_cmd`,
 *      both backend-only — no plaintext key readback), surfaced through the
 *      `onDiscover` callback the controller owns;
 *   2. normalizes the payloads into the non-secret accelerator view model
 *      ({@link buildAcceleratorCatalog}) and ranks them per preset
 *      ({@link rankAccelerators});
 *   3. renders a ranked accelerator table with provenance/source labels and
 *      stale / empty / error / missing-metadata states; and
 *   4. applies a chosen preset's {@link acceleratorProviderOrder} into the
 *      OpenRouter routing policy via `onApplyPreset` — the dynamic candidates
 *      drive routing, never a hardcoded constant.
 *
 * PURE PRESENTATION: this component never calls `invoke` and never holds the
 * API key. The controller fetches with the saved-key commands and hands the raw
 * payloads (or the loading/error state) down as props.
 */

import type { TFunction } from "i18next";
import type { OpenRouterModelEndpoints, OpenRouterProvider } from "../types";
import {
  type AcceleratorEndpointView,
  type AcceleratorPreset,
  acceleratorProviderOrder,
  buildAcceleratorCatalog,
  rankAccelerators,
} from "../utils/openrouterCatalog";

/** The three accelerator-discovery presets the table exposes. */
const ACCELERATOR_PRESETS: AcceleratorPreset[] = [
  "low_latency",
  "high_throughput",
  "privacy_zdr",
];

const PRESET_LABEL_KEY: Record<AcceleratorPreset, string> = {
  low_latency: "settings.acceleratorDiscovery.presets.lowLatency",
  high_throughput: "settings.acceleratorDiscovery.presets.highThroughput",
  privacy_zdr: "settings.acceleratorDiscovery.presets.privacyZdr",
};

export interface OpenRouterAcceleratorDiscoveryProps {
  t: TFunction;
  /** Raw endpoint catalog from `list_openrouter_model_endpoints_cmd`, or null. */
  endpoints: OpenRouterModelEndpoints | null;
  /** Raw provider catalog from `list_openrouter_providers_cmd`, or null. */
  providers: OpenRouterProvider[] | null;
  /** The OpenRouter model id whose endpoints are being discovered (for labels). */
  modelId: string;
  /** True while a saved-key discovery fetch is in flight. */
  loading: boolean;
  /** Discovery error message (from the saved-key fetch), or null. */
  error: string | null;
  /** Whether a usable OpenRouter credential (saved or draft) is available. */
  credentialAvailable: boolean;
  /** The discovery preset currently selected in the table. */
  selectedPreset: AcceleratorPreset;
  /** True while the currently-applied routing policy came from a dynamic preset. */
  appliedPreset: AcceleratorPreset | null;
  /** Change the active discovery preset (re-ranks the table). */
  onSelectPreset: (preset: AcceleratorPreset) => void;
  /** Trigger a saved-key catalog fetch for {@link modelId}. */
  onDiscover: () => void;
  /**
   * Apply the ranked accelerator order for {@link preset} into the routing
   * policy. `order` is the deduped slug list from {@link acceleratorProviderOrder}.
   */
  onApplyPreset: (preset: AcceleratorPreset, order: string[]) => void;
}

function formatLatency(seconds: number | null): string {
  if (seconds == null) return "—";
  return `${(seconds * 1000).toFixed(0)} ms`;
}

function formatThroughput(tps: number | null): string {
  if (tps == null) return "—";
  return `${tps.toFixed(0)} tok/s`;
}

/** Human-readable provenance label for a row's slug/policy source. */
function sourceLabel(t: TFunction, view: AcceleratorEndpointView): string {
  const hasPolicy =
    view.privacy.privacyPolicyUrl != null ||
    view.privacy.termsOfServiceUrl != null;
  if (hasPolicy) {
    return t("settings.acceleratorDiscovery.source.catalog");
  }
  return t("settings.acceleratorDiscovery.source.derived");
}

export default function OpenRouterAcceleratorDiscovery({
  t,
  endpoints,
  providers,
  modelId,
  loading,
  error,
  credentialAvailable,
  selectedPreset,
  appliedPreset,
  onSelectPreset,
  onDiscover,
  onApplyPreset,
}: OpenRouterAcceleratorDiscoveryProps) {
  const catalog = buildAcceleratorCatalog(endpoints, providers);
  const ranked = rankAccelerators(catalog.endpoints, selectedPreset);
  const order = acceleratorProviderOrder(ranked);
  const hasDiscovered = endpoints !== null;
  const hasModel = modelId.trim().length > 0;

  return (
    <div className="settings-field accelerator-discovery">
      <div className="settings-field__label" id="accelerator-discovery-label">
        {t("settings.acceleratorDiscovery.title")}
      </div>
      <p className="settings-hint">
        {t("settings.acceleratorDiscovery.intro")}
      </p>

      <div className="settings-inline-row">
        <button
          type="button"
          className="settings-btn settings-btn--secondary"
          disabled={loading || !credentialAvailable || !hasModel}
          onClick={onDiscover}
        >
          {loading
            ? t("settings.acceleratorDiscovery.discovering")
            : t("settings.acceleratorDiscovery.discover")}
        </button>
      </div>

      {!hasModel ? (
        <p className="settings-hint">
          {t("settings.acceleratorDiscovery.needModel")}
        </p>
      ) : null}

      {error ? (
        <p className="settings-error" role="alert">
          {t("settings.acceleratorDiscovery.error", { error })}
        </p>
      ) : loading ? (
        <p className="settings-hint">
          {t("settings.acceleratorDiscovery.loading")}
        </p>
      ) : !hasDiscovered ? (
        <p className="settings-hint">
          {t("settings.acceleratorDiscovery.idle")}
        </p>
      ) : catalog.endpoints.length === 0 ? (
        <p className="settings-hint" role="status">
          {t("settings.acceleratorDiscovery.empty")}
        </p>
      ) : (
        <>
          <div
            className="settings-radio-group"
            role="radiogroup"
            aria-labelledby="accelerator-discovery-label"
          >
            {ACCELERATOR_PRESETS.map((preset) => (
              <label className="settings-radio" key={preset}>
                <input
                  type="radio"
                  name="accelerator-discovery-preset"
                  checked={selectedPreset === preset}
                  onChange={() => onSelectPreset(preset)}
                />
                <span>{t(PRESET_LABEL_KEY[preset])}</span>
              </label>
            ))}
          </div>

          {ranked.length === 0 ? (
            <p className="settings-hint" role="status">
              {t("settings.acceleratorDiscovery.noCandidatesForPreset")}
            </p>
          ) : (
            <table className="accelerator-discovery__table">
              <caption className="settings-hint">
                {t("settings.acceleratorDiscovery.tableCaption", {
                  model: catalog.modelId ?? modelId,
                })}
              </caption>
              <thead>
                <tr>
                  <th scope="col">
                    {t("settings.acceleratorDiscovery.columns.provider")}
                  </th>
                  <th scope="col">
                    {t("settings.acceleratorDiscovery.columns.latency")}
                  </th>
                  <th scope="col">
                    {t("settings.acceleratorDiscovery.columns.throughput")}
                  </th>
                  <th scope="col">
                    {t("settings.acceleratorDiscovery.columns.quantization")}
                  </th>
                  <th scope="col">
                    {t("settings.acceleratorDiscovery.columns.source")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {ranked.map((view) => (
                  <tr key={`${view.providerSlug}:${view.tag ?? ""}`}>
                    <th scope="row">
                      <span className="accelerator-discovery__provider">
                        {view.providerName}
                      </span>{" "}
                      <code className="accelerator-discovery__slug">
                        {view.providerSlug}
                      </code>
                      {view.isFree ? (
                        <span className="accelerator-discovery__badge">
                          {t("settings.acceleratorDiscovery.freeBadge")}
                        </span>
                      ) : null}
                    </th>
                    <td>{formatLatency(view.latencyP50)}</td>
                    <td>{formatThroughput(view.throughputP50)}</td>
                    <td>{view.quantization ?? "—"}</td>
                    <td>
                      {view.privacy.privacyPolicyUrl ? (
                        <a
                          href={view.privacy.privacyPolicyUrl}
                          target="_blank"
                          rel="noreferrer noopener"
                        >
                          {sourceLabel(t, view)}
                        </a>
                      ) : (
                        <span>{sourceLabel(t, view)}</span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}

          <div className="settings-inline-row">
            <button
              type="button"
              className="settings-btn"
              disabled={order.length === 0}
              onClick={() => onApplyPreset(selectedPreset, order)}
            >
              {t("settings.acceleratorDiscovery.apply", {
                preset: t(PRESET_LABEL_KEY[selectedPreset]),
              })}
            </button>
            {appliedPreset ? (
              <span
                className="accelerator-discovery__applied"
                role="status"
                aria-live="polite"
              >
                {t("settings.acceleratorDiscovery.applied", {
                  preset: t(PRESET_LABEL_KEY[appliedPreset]),
                })}
              </span>
            ) : null}
          </div>
          <p className="settings-hint">
            {order.length > 0
              ? t("settings.acceleratorDiscovery.applyHint", {
                  order: order.join(", "),
                })
              : t("settings.acceleratorDiscovery.applyEmptyHint")}
          </p>
        </>
      )}
    </div>
  );
}
