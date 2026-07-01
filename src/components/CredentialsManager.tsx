/**
 * Local-models readiness panel — sub-form in `SettingsPage`.
 *
 * Despite the "CredentialsManager" name (kept for import stability), this
 * component now renders ONLY the local models-readiness section: one card per
 * `ModelInfo` (Whisper / llama / sortformer) with `ModelStatus` badges,
 * download / progress / ETA affordances (see `describeDownloadProgress`,
 * `formatBytes`, `formatEta`), and per-model download guidance. Its single
 * render path outputs `#settings-models-section` and nothing else.
 *
 * The per-credential-key controls this file used to host (show/hide, save,
 * delete, and "Test connection" for the provider test commands) now live in
 * the settings credential panels (`src/components/settings/CredentialsPanel.tsx`
 * for credential health + readiness). The backend log-level control lives
 * solely in `LoggingSettings.tsx` (the Logging tab) after the logging dedup;
 * it is not rendered here.
 *
 * The `ALLOWED_CREDENTIAL_KEYS` allow-list referenced by the credential
 * surfaces stays consistent across `src/types/index.ts` and
 * `src-tauri/src/credentials/mod.rs`.
 *
 * Parent: `SettingsPage.tsx`. Props are the model list + readiness/download
 * state + translation handle; see the inline type below.
 */
import type { TFunction } from "i18next";
import {
  LFM2_EXTRACT_MODEL_FILENAME,
  WHISPER_SMALL_EN_MODEL_FILENAME,
} from "../modelConstants";
import type {
  DownloadProgress,
  ModelInfo,
  ModelReadiness,
  ModelStatus,
} from "../types";
import Button from "./Button";
import { readinessBadge, type SettingsState } from "./settingsTypes";

/** Format bytes to a human-readable size string (e.g. "466 MB"). */
function formatSize(bytes: number | null): string {
  if (bytes === null || bytes === 0) return "—";
  const mb = bytes / (1024 * 1024);
  if (mb >= 1024) {
    return `${(mb / 1024).toFixed(1)} GB`;
  }
  return `${Math.round(mb)} MB`;
}

/** Compact "MB" string used inside progress lines (always shows the unit). */
function formatDownloadedMB(bytes: number): string {
  const mb = bytes / (1024 * 1024);
  if (mb >= 1024) {
    return `${(mb / 1024).toFixed(1)} GB`;
  }
  return `${Math.round(mb)} MB`;
}

/**
 * Format a remaining-time estimate as `Xs`, `Xm Ys`, or `Xh Ym`. We prefer a
 * compact spoken-length form over raw seconds so large downloads don't read as
 * "3600s remaining". Returns `—` for non-finite inputs.
 */
export function formatEta(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return "—";
  const s = Math.max(1, Math.round(seconds));
  if (s < 60) return `${s}s`;
  if (s < 3600) {
    const m = Math.floor(s / 60);
    const rem = s % 60;
    return rem === 0 ? `${m}m` : `${m}m ${rem}s`;
  }
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return m === 0 ? `${h}h` : `${h}h ${m}m`;
}

/**
 * Render the text line shown next to the progress bar. Handles three cases:
 *   - unknown total (`total_bytes === 0`): show downloaded-only
 *   - error status: show the translated error
 *   - otherwise: show downloaded / total + ETA
 *
 * ETA = (total - downloaded) * elapsed / downloaded. While `bytes_downloaded`
 * is 0 we can't divide yet, so we fall back to the downloaded-only string
 * rather than rendering `NaN`.
 */
export function describeDownloadProgress(
  progress: DownloadProgress,
  t: TFunction,
): string {
  if (progress.status === "error") {
    return t("settings.models.downloadError", {
      message: progress.model_name,
    });
  }
  const downloaded = formatDownloadedMB(progress.bytes_downloaded);
  if (progress.total_bytes === 0 || progress.bytes_downloaded === 0) {
    return t("settings.models.downloadProgressUnknown", { downloaded });
  }
  const remainingBytes = Math.max(
    0,
    progress.total_bytes - progress.bytes_downloaded,
  );
  const etaSeconds =
    (remainingBytes * (progress.elapsed_ms / 1000)) / progress.bytes_downloaded;
  return t("settings.models.downloadProgressKnown", {
    downloaded,
    total: formatDownloadedMB(progress.total_bytes),
    eta: formatEta(etaSeconds),
  });
}

/**
 * Map a model filename to an `settings.modelGuidance.*` i18n key, or null if
 * the model has no tier-based guidance. Keyed off filename (stable identifier)
 * rather than the display name so translated model names don't break lookup.
 */
function guidanceKeyForModel(filename: string): string | null {
  switch (filename) {
    case "ggml-tiny.en.bin":
      return "settings.modelGuidance.tinyEn";
    case "ggml-base.en.bin":
      return "settings.modelGuidance.baseEn";
    case WHISPER_SMALL_EN_MODEL_FILENAME:
      return "settings.modelGuidance.smallEn";
    case "ggml-medium.en.bin":
      return "settings.modelGuidance.mediumEn";
    case "ggml-large-v3.bin":
      return "settings.modelGuidance.largeV3";
    case LFM2_EXTRACT_MODEL_FILENAME:
      return "settings.modelGuidance.lfm2_350m";
    default:
      return null;
  }
}

function readinessForModel(
  model: ModelInfo,
  modelStatus: ModelStatus | null,
): ModelReadiness {
  if (modelStatus && model.filename === WHISPER_SMALL_EN_MODEL_FILENAME) {
    return modelStatus.whisper;
  }
  if (modelStatus && model.filename === LFM2_EXTRACT_MODEL_FILENAME) {
    return modelStatus.llm;
  }
  if (
    modelStatus &&
    model.filename === "diar_streaming_sortformer_4spk-v2.onnx"
  ) {
    return modelStatus.sortformer;
  }

  if (!model.is_downloaded) return "NotDownloaded";
  return model.is_valid ? "Ready" : "Invalid";
}

interface CredentialsManagerProps {
  state: Pick<SettingsState, "confirmDelete">;
  t: TFunction;
  models: ModelInfo[];
  modelStatus: ModelStatus | null;
  isDownloading: boolean;
  isDeletingModel: string | null;
  downloadProgress: DownloadProgress | null;
  downloadModel: (filename: string) => void;
  handleDeleteClick: (filename: string) => void;
}

/**
 * Managed stores shown to the user: downloaded model files (the primary
 * on-disk credential-like assets). Kept here so SettingsPage stays a thin
 * orchestrator.
 */
export default function CredentialsManager({
  state,
  t,
  models,
  modelStatus,
  isDownloading,
  isDeletingModel,
  downloadProgress,
  downloadModel,
  handleDeleteClick,
}: CredentialsManagerProps) {
  const { confirmDelete } = state;

  return (
    <div id="settings-models-section" className="settings-section">
      <h3 className="settings-section__title">
        {t("settings.sections.models")}
      </h3>
      {models.map((model) => {
        const status = readinessForModel(model, modelStatus);
        const badge = readinessBadge(status);
        // Match on model_id (== filename) when available; fall back to
        // display name for compatibility with events that haven't been
        // re-emitted since the payload shape widened.
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
          <div className="model-card" key={model.filename}>
            <div className="model-card__header">
              <div>
                <span className="model-card__name">{model.name}</span>
                <span className={`status-badge ${badge.cls}`}>
                  {t(badge.labelKey)}
                </span>
              </div>
              <span className="model-card__size">
                {formatSize(model.size_bytes)}
              </span>
            </div>
            {model.description && (
              <p className="model-card__description">{model.description}</p>
            )}
            {(() => {
              const gk = guidanceKeyForModel(model.filename);
              return gk ? (
                <p
                  className="model-card__hint"
                  data-testid={`model-guidance-${model.filename}`}
                >
                  {t(gk)}
                </p>
              ) : null;
            })()}

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
          </div>
        );
      })}
      {models.length === 0 && (
        <p className="settings-section__empty">{t("settings.models.empty")}</p>
      )}
    </div>
  );
}
