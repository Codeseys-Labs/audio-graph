/**
 * Download-progress formatting helpers, shared by `CredentialsManager` (the full
 * model catalog card) and `ModelActionButtons` (the readiness-card actions).
 *
 * Extracted from `CredentialsManager.tsx` so the progress/ETA strings have a
 * single implementation across both render paths (no forked formatting). Pure
 * functions — no React, no store access.
 */
import type { TFunction } from "i18next";
import type { DownloadProgress } from "../../types";

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
