/**
 * Shared per-model Download / Delete action markup.
 *
 * Single source of truth for the model-file action buttons + download-progress
 * line. Rendered by BOTH:
 *   1. `CredentialsManager` — the full `#settings-models-section` catalog card.
 *   2. `CredentialsPanel` (via `ReadinessModelActions`) — the per-provider
 *      readiness rollup card, scoped to the model files the provider actually
 *      requires (`ProviderDescriptor.local_models[].model_id === ModelInfo.filename`).
 *
 * Factored out per docs/plans/2026-07-02-readiness-model-actions-design.md §3-4
 * so progress/ETA formatting and the two-click confirm-delete flow have one
 * implementation. Reuses the store actions threaded through `useSettings()`
 * (`downloadModel`/`handleDeleteClick`/`downloadProgress`/…) — no new backend
 * command or store state.
 *
 * Download progress + `isDownloading` are a single GLOBAL slot (one download at
 * a time), so:
 *   - the Download button is `disabled={isDownloading}` for every row while any
 *     download runs (prevents concurrent downloads), and
 *   - the progress line is filename-gated (`downloadProgress.model_id ===
 *     model.filename`) so only the row being downloaded lights up.
 */
import type { TFunction } from "i18next";
import type { DownloadProgress, ModelInfo } from "../../types";
import Button from "../Button";
import { describeDownloadProgress } from "./downloadProgress";

export interface ModelActionButtonsProps {
  model: ModelInfo;
  t: TFunction;
  isDownloading: boolean;
  isDeletingModel: string | null;
  confirmDelete: string | null;
  downloadProgress: DownloadProgress | null;
  downloadModel: (filename: string) => void;
  handleDeleteClick: (filename: string) => void;
}

/**
 * Renders the Download button (when the model is not on disk) or the Delete
 * button (when it is), plus the download progress bar/ETA line for the row
 * currently downloading. Presentational only — all state comes from props.
 */
export default function ModelActionButtons({
  model,
  t,
  isDownloading,
  isDeletingModel,
  confirmDelete,
  downloadProgress,
  downloadModel,
  handleDeleteClick,
}: ModelActionButtonsProps) {
  // Match on model_id (== filename) when available; fall back to display name
  // for compatibility with events that haven't been re-emitted since the
  // payload shape widened.
  const progressMatches = downloadProgress
    ? downloadProgress.model_id === model.filename ||
      downloadProgress.model_name === model.name
    : false;
  const isThisDownloading = isDownloading && progressMatches;
  const showProgressLine =
    progressMatches &&
    downloadProgress !== null &&
    downloadProgress.status !== "complete";
  const isThisDeleting = isDeletingModel === model.filename;

  return (
    <>
      <div className="model-card__actions">
        {!model.is_downloaded && (
          <Button
            variant="primary"
            className="settings-model-action"
            onClick={() => downloadModel(model.filename)}
            disabled={isDownloading}
          >
            {isThisDownloading
              ? t("settings.buttons.downloading")
              : t("settings.buttons.download")}
          </Button>
        )}
        {model.is_downloaded && (
          <Button
            variant="danger"
            className="settings-model-action settings-model-action--danger"
            onClick={() => handleDeleteClick(model.filename)}
            disabled={isThisDeleting}
          >
            {isThisDeleting
              ? t("settings.buttons.deleting")
              : confirmDelete === model.filename
                ? t("settings.buttons.confirmDelete")
                : t("settings.buttons.delete")}
          </Button>
        )}
      </div>

      {/* Download progress bar + ETA text */}
      {showProgressLine && downloadProgress && (
        <>
          <div className="download-progress">
            <div
              className="download-progress__bar"
              style={{ width: `${downloadProgress.percent}%` }}
            />
          </div>
          <p
            className="model-card__hint"
            data-testid={`model-progress-${model.filename}`}
          >
            {describeDownloadProgress(downloadProgress, t)}
          </p>
        </>
      )}
    </>
  );
}
