/**
 * Local-models readiness panel — sub-form in `SettingsPage`.
 *
 * Despite the "CredentialsManager" name (kept for import stability), this
 * component now renders ONLY the local models-readiness section: one card per
 * `ModelInfo` (Whisper / llama / sortformer) with `ModelStatus` badges, the
 * shared `ModelActionButtons` download / progress / ETA affordances (see
 * `describeDownloadProgress` in `settings/downloadProgress.ts`), and per-model
 * download guidance. Its single render path outputs `#settings-models-section`
 * and nothing else.
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
import ModelActionButtons from "./settings/ModelActionButtons";
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
        );
      })}
      {models.length === 0 && (
        <p className="settings-section__empty">{t("settings.models.empty")}</p>
      )}
    </div>
  );
}
