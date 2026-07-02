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
 */
import type { TFunction } from "i18next";
import type { DownloadProgress, ModelInfo } from "../../types";
import { PROVIDER_DESCRIPTORS } from "../providerRegistryHelpers";
import ModelActionButtons from "./ModelActionButtons";

export interface ReadinessModelActionsProps {
  providerId: string;
  t: TFunction;
  models: ModelInfo[];
  isDownloading: boolean;
  isDeletingModel: string | null;
  confirmDelete: string | null;
  downloadProgress: DownloadProgress | null;
  downloadModel: (filename: string) => void;
  handleDeleteClick: (filename: string) => void;
}

export default function ReadinessModelActions({
  providerId,
  t,
  models,
  isDownloading,
  isDeletingModel,
  confirmDelete,
  downloadProgress,
  downloadModel,
  handleDeleteClick,
}: ReadinessModelActionsProps) {
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
