import type { TFunction } from "i18next";
import {
  readinessBadge,
  type LogLevel,
  type SettingsState,
} from "./settingsTypes";
import type {
  DownloadProgress,
  ModelInfo,
  ModelReadiness,
  ModelStatus,
} from "../types";

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
    case "ggml-small.en.bin":
      return "settings.modelGuidance.smallEn";
    case "ggml-medium.en.bin":
      return "settings.modelGuidance.mediumEn";
    case "ggml-large-v3.bin":
      return "settings.modelGuidance.largeV3";
    case "lfm2-350m-extract-q4_k_m.gguf":
      return "settings.modelGuidance.lfm2_350m";
    default:
      return null;
  }
}

interface CredentialsManagerProps {
  state: Pick<SettingsState, "confirmDelete" | "logLevel">;
  t: TFunction;
  models: ModelInfo[];
  modelStatus: ModelStatus | null;
  isDownloading: boolean;
  isDeletingModel: string | null;
  downloadProgress: DownloadProgress | null;
  downloadModel: (filename: string) => void;
  handleDeleteClick: (filename: string) => void;
  handleLogLevelChange: (next: LogLevel) => Promise<void>;
}

/**
 * Managed stores shown to the user: downloaded model files (the primary
 * on-disk credential-like assets) and backend diagnostic log level. These
 * two regions live together here so SettingsPage stays a thin orchestrator.
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
  handleLogLevelChange,
}: CredentialsManagerProps) {
  const { confirmDelete, logLevel } = state;

  return (
    <>
      <div id="settings-models-section" className="settings-section">
        <h3 className="settings-section__title">{t("settings.sections.models")}</h3>
        {models.map((model) => {
          const status =
            modelStatus && model.name.toLowerCase().includes("whisper")
              ? modelStatus.whisper
              : modelStatus && model.name.toLowerCase().includes("sortformer")
                ? modelStatus.sortformer
                : modelStatus
                  ? modelStatus.llm
                  : ("NotDownloaded" as ModelReadiness);

          const badge = readinessBadge(status);
          const isThisDownloading =
            isDownloading && downloadProgress?.model_name === model.name;
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
                <p className="model-card__description">
                  {model.description}
                </p>
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
                  <button
                    className="settings-btn settings-btn--primary"
                    onClick={() => downloadModel(model.filename)}
                    disabled={isDownloading}
                  >
                    {isThisDownloading ? t("settings.buttons.downloading") : t("settings.buttons.download")}
                  </button>
                )}
                {model.is_downloaded && (
                  <button
                    className="settings-btn settings-btn--danger"
                    onClick={() => handleDeleteClick(model.filename)}
                    disabled={isThisDeleting}
                  >
                    {isThisDeleting
                      ? t("settings.buttons.deleting")
                      : confirmDelete === model.filename
                        ? t("settings.buttons.confirmDelete")
                        : t("settings.buttons.delete")}
                  </button>
                )}
              </div>

              {/* Download progress bar */}
              {isThisDownloading && downloadProgress && (
                <div className="download-progress">
                  <div
                    className="download-progress__bar"
                    style={{ width: `${downloadProgress.percent}%` }}
                  />
                </div>
              )}
            </div>
          );
        })}
        {models.length === 0 && (
          <p className="settings-section__empty">{t("settings.models.empty")}</p>
        )}
      </div>

      <div className="settings-section">
        <h3 className="settings-section__title">
          {t("settings.sections.diagnostics")}
        </h3>
        <div className="settings-section__api-fields">
          <div className="settings-field">
            <label className="settings-field__label" htmlFor="log-level-select">
              {t("settings.fields.backendLogLevel")}
            </label>
            <select
              id="log-level-select"
              className="settings-input"
              value={logLevel}
              onChange={(e) =>
                handleLogLevelChange(e.target.value as LogLevel)
              }
            >
              <option value="off">{t("settings.logLevels.off")}</option>
              <option value="error">{t("settings.logLevels.error")}</option>
              <option value="warn">{t("settings.logLevels.warn")}</option>
              <option value="info">{t("settings.logLevels.info")}</option>
              <option value="debug">{t("settings.logLevels.debug")}</option>
              <option value="trace">{t("settings.logLevels.trace")}</option>
            </select>
            <p className="settings-hint">
              {t("settings.hints.logLevelPrefix")}{" "}
              <code>RUST_LOG</code>{" "}
              {t("settings.hints.logLevelSuffix")}
            </p>
          </div>
        </div>
      </div>
    </>
  );
}
