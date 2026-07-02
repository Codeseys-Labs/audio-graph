/**
 * Per-provider Download / Delete model actions on the readiness rollup cards.
 *
 * Implements docs/plans/2026-07-02-readiness-model-actions-design.md: for a
 * LOCAL model-backed provider, join the descriptor's declared model files to
 * the in-memory `ModelInfo[]` catalog and render the shared `ModelActionButtons`
 * (Download when not on disk, Delete when it is) per matched file.
 *
 * Join key (pure, in-memory, frontend):
 *   ProviderReadiness.provider_id
 *     -> PROVIDER_DESCRIPTORS.get(provider_id).local_models[].model_id
 *        === ModelInfo.filename   (both are the models/mod.rs filename constants)
 *
 * CLOUD providers have `local_models: []` (and a non-`local_files` catalog
 * policy), so `localReqs` is empty and this component renders NOTHING — that is
 * the mechanism that keeps Download/Delete off cloud provider cards. No backend
 * command or store state is added; everything is reused from `useSettings()`.
 *
 * The fast-changing download state (`downloadProgress`/`isDownloading`, plus the
 * `models`/`confirmDelete`/`isDeletingModel`/action handlers only these buttons
 * use) is read from `useSettings()` HERE rather than threaded through props from
 * `CredentialsPanel`. The `MODEL_DOWNLOAD_PROGRESS` listener updates that state
 * on every progress tick; keeping the read local means only this per-provider
 * subtree references it, instead of the parent panel naming it in its own
 * destructure. Context reads are cheap, so each mapped instance reading the
 * context is fine (CodeRabbit PR #25 minor perf finding).
 */
import type { TFunction } from "i18next";
import type { ModelInfo } from "../../types";
import { PROVIDER_DESCRIPTORS } from "../providerRegistryHelpers";
import ModelActionButtons from "./ModelActionButtons";
import { useSettings } from "./SettingsContext";

export interface ReadinessModelActionsProps {
  providerId: string;
  t: TFunction;
}

export default function ReadinessModelActions({
  providerId,
  t,
}: ReadinessModelActionsProps) {
  const {
    models,
    isDownloading,
    isDeletingModel,
    confirmDelete,
    downloadProgress,
    downloadModel,
    handleDeleteClick,
  } = useSettings();
  const descriptor = PROVIDER_DESCRIPTORS.get(providerId);
  const localReqs = descriptor?.local_models ?? [];

  // Join each declared requirement to its catalog entry. Filter out any
  // requirement with no matching `ModelInfo` (e.g. a filename not surfaced by
  // `list_available_models`) so we never render a dead button.
  const rows = localReqs
    .map((req) => models.find((m) => m.filename === req.model_id))
    .filter((m): m is ModelInfo => Boolean(m));

  if (rows.length === 0) return null;

  return (
    <div className="settings-readiness__models">
      {rows.map((model) => (
        <div
          key={model.filename}
          className="settings-readiness__model"
          data-testid={`readiness-model-${model.filename}`}
        >
          <ModelActionButtons
            model={model}
            t={t}
            isDownloading={isDownloading}
            isDeletingModel={isDeletingModel}
            confirmDelete={confirmDelete}
            downloadProgress={downloadProgress}
            downloadModel={downloadModel}
            handleDeleteClick={handleDeleteClick}
          />
        </div>
      ))}
    </div>
  );
}
