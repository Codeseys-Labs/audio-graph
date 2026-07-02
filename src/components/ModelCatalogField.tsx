/**
 * Shared "choose a model" field: a {@link ModelCatalogPicker} plus, for
 * providers whose catalog is fetched from a backend command
 * (`hasRemoteCommand`), a "Load models" refresh button and a tri-state status
 * line (loading / error / empty).
 *
 * This is the single surface behind the uniform Load-models rollout across the
 * provider tabs. Providers whose catalog is curated-static (`fixed`) or local
 * (`local_files`) pass `hasRemoteCommand={false}` and get the picker only —
 * the refresh button and status line are suppressed because there is nothing
 * to fetch.
 *
 * The three status strings are genericised via i18n with a provider-name
 * interpolation (`settings.modelCatalog.failed` / `.loading` / `.empty`) so we
 * no longer maintain one copy per provider.
 */

import type { TFunction } from "i18next";
import type { ProviderModelCatalogItem } from "../types";
import ModelCatalogPicker from "./ModelCatalogPicker";

interface ModelCatalogFieldProps {
  /** DOM id for the picker input (label `htmlFor` should match). */
  id: string;
  value: string;
  onChange: (value: string) => void;
  catalog: ProviderModelCatalogItem[];
  t: TFunction;
  /** Human-readable provider name interpolated into the status strings. */
  providerName: string;
  placeholder?: string;
  ariaLabel?: string;
  /** True while a backend catalog fetch is in flight. */
  loading?: boolean;
  /** Last catalog-fetch error message, or null when there is none. */
  error?: string | null;
  /** Whether a usable credential (typed or saved) is available to fetch with. */
  credentialAvailable?: boolean;
  /** Kicks off a backend catalog refresh. Only called when a command exists. */
  onRefresh?: () => void;
  /**
   * Whether this provider has a `model_catalog_command`. When false the field
   * renders the picker only (no button, no status line) — curated-static and
   * local-file providers reuse the same field without a fetch affordance.
   */
  hasRemoteCommand: boolean;
}

export default function ModelCatalogField({
  id,
  value,
  onChange,
  catalog,
  t,
  providerName,
  placeholder,
  ariaLabel,
  loading = false,
  error = null,
  credentialAvailable = false,
  onRefresh,
  hasRemoteCommand,
}: ModelCatalogFieldProps) {
  const picker = (
    <ModelCatalogPicker
      id={id}
      value={value}
      onChange={onChange}
      catalog={catalog}
      t={t}
      placeholder={placeholder}
      ariaLabel={ariaLabel}
    />
  );

  if (!hasRemoteCommand) {
    return picker;
  }

  return (
    <>
      <div className="settings-inline-row">
        {picker}
        <button
          type="button"
          className="settings-btn settings-btn--secondary"
          disabled={loading || !credentialAvailable}
          onClick={onRefresh}
        >
          {loading
            ? t("settings.buttons.refreshing")
            : t("settings.buttons.refreshModels")}
        </button>
      </div>
      {error ? (
        <p className="settings-error" role="alert">
          {t("settings.modelCatalog.failed", { provider: providerName, error })}
        </p>
      ) : loading ? (
        <p className="settings-hint">
          {t("settings.modelCatalog.loading", { provider: providerName })}
        </p>
      ) : catalog.length === 0 ? (
        <p className="settings-hint">
          {t("settings.modelCatalog.empty", { provider: providerName })}
        </p>
      ) : null}
    </>
  );
}
