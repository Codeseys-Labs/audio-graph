/**
 * Logging settings — file-logging controls for capturing a session's logs as
 * shareable feedback when reporting/fixing issues, plus an independent,
 * opt-in "Privacy & Diagnostics" toggle for anonymous analytics (Sentry).
 *
 * Self-contained: unlike the other Settings sub-forms it does NOT go through
 * the shared reducer + footer "Save". It reads/writes via dedicated Tauri
 * commands (`get_log_info`, `set_logging_config`, `purge_logs_cmd`,
 * `open_logs_dir`, `get_analytics_info`, `set_analytics_enabled`) and applies
 * changes immediately, because both are side-effecting runtime concerns rather
 * than provider config.
 *
 * The two toggles (file logging vs. anonymous analytics) are fully
 * independent: enable either, both, or neither. They keep separate local
 * state and separate busy/status flows so one cannot disable or mask the
 * other.
 *
 * Parent: `SettingsPage.tsx`.
 */

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { AnalyticsInfo } from "../types";

interface LogFileEntry {
  name: string;
  size_bytes: number;
  modified_ms: number | null;
  is_active: boolean;
}

interface LogInfo {
  enabled: boolean;
  mode: string; // "archive" | "overwrite"
  level: string;
  dir: string;
  active_path: string | null;
  files: LogFileEntry[];
}

const LEVELS = ["off", "error", "warn", "info", "debug", "trace"] as const;

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

export default function LoggingSettings() {
  const { t } = useTranslation();
  const [info, setInfo] = useState<LogInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setInfo(await invoke<LogInfo>("get_log_info"));
    } catch (e) {
      setStatus(t("settings.logging.readFailed", { error: String(e) }));
    }
  }, [t]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Apply a partial change by merging onto current info, then persist+reapply.
  const apply = useCallback(
    async (patch: Partial<Pick<LogInfo, "enabled" | "mode" | "level">>) => {
      if (!info) return;
      setBusy(true);
      setStatus(null);
      try {
        const next = {
          enabled: patch.enabled ?? info.enabled,
          mode: patch.mode ?? info.mode,
          level: patch.level ?? info.level,
        };
        const updated = await invoke<LogInfo>("set_logging_config", {
          enabled: next.enabled,
          mode: next.mode,
          level: next.level,
        });
        setInfo(updated);
        setStatus(t("settings.logging.applied"));
      } catch (e) {
        setStatus(t("settings.logging.applyFailed", { error: String(e) }));
      } finally {
        setBusy(false);
      }
    },
    [info, t],
  );

  const purge = useCallback(async () => {
    setBusy(true);
    setStatus(null);
    try {
      const removed = await invoke<number>("purge_logs_cmd");
      setStatus(t("settings.logging.purged", { count: removed }));
      await refresh();
    } catch (e) {
      setStatus(t("settings.logging.purgeFailed", { error: String(e) }));
    } finally {
      setBusy(false);
    }
  }, [refresh, t]);

  const openDir = useCallback(async () => {
    try {
      await invoke<string>("open_logs_dir");
    } catch (e) {
      setStatus(t("settings.logging.openFailed", { error: String(e) }));
    }
  }, [t]);

  // ── Anonymous analytics (Sentry) — fully independent of file logging ─────
  // Separate state + busy/status so the two toggles never block each other.
  const [analyticsInfo, setAnalyticsInfo] = useState<AnalyticsInfo | null>(
    null,
  );
  const [analyticsBusy, setAnalyticsBusy] = useState(false);
  const [analyticsStatus, setAnalyticsStatus] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const got = await invoke<AnalyticsInfo>("get_analytics_info");
        if (!cancelled) setAnalyticsInfo(got);
      } catch (e) {
        if (!cancelled) {
          setAnalyticsStatus(
            t("settings.analytics.readFailed", { error: String(e) }),
          );
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [t]);

  const applyAnalytics = useCallback(
    async (enabled: boolean) => {
      setAnalyticsBusy(true);
      setAnalyticsStatus(null);
      try {
        const updated = await invoke<AnalyticsInfo>("set_analytics_enabled", {
          enabled,
        });
        setAnalyticsInfo(updated);
        setAnalyticsStatus(t("settings.analytics.applied"));
      } catch (e) {
        setAnalyticsStatus(
          t("settings.analytics.applyFailed", { error: String(e) }),
        );
      } finally {
        setAnalyticsBusy(false);
      }
    },
    [t],
  );

  return (
    <>
      <section className="settings-section">
        <h3 className="settings-section-title">
          {t("settings.logging.title")}
        </h3>
        <p className="settings-section-help">{t("settings.logging.help")}</p>

        <div className="settings-field settings-field--inline">
          <label>
            <input
              type="checkbox"
              checked={info?.enabled ?? true}
              disabled={busy || !info}
              onChange={(e) => apply({ enabled: e.target.checked })}
            />{" "}
            {t("settings.logging.enable")}
          </label>
        </div>

        <div className="settings-field">
          <label htmlFor="log-level-select">
            {t("settings.logging.level")}
          </label>
          <select
            id="log-level-select"
            value={info?.level ?? "info"}
            disabled={busy || !info}
            onChange={(e) => apply({ level: e.target.value })}
          >
            {LEVELS.map((l) => (
              <option key={l} value={l}>
                {l}
              </option>
            ))}
          </select>
        </div>

        <div className="settings-field">
          <span className="settings-field__label">
            {t("settings.logging.startupMode")}
          </span>
          <label className="settings-radio">
            <input
              type="radio"
              name="log-file-mode"
              value="archive"
              checked={(info?.mode ?? "archive") === "archive"}
              disabled={busy || !info?.enabled}
              onChange={() => apply({ mode: "archive" })}
            />{" "}
            {t("settings.logging.modeArchive")}
          </label>
          <label className="settings-radio">
            <input
              type="radio"
              name="log-file-mode"
              value="overwrite"
              checked={info?.mode === "overwrite"}
              disabled={busy || !info?.enabled}
              onChange={() => apply({ mode: "overwrite" })}
            />{" "}
            {t("settings.logging.modeOverwrite")}
          </label>
        </div>

        <div className="settings-field settings-field--inline">
          <button
            type="button"
            className="settings-btn"
            onClick={openDir}
            disabled={!info}
          >
            {t("settings.logging.openFolder")}
          </button>
          <button
            type="button"
            className="settings-btn"
            onClick={purge}
            disabled={busy || !info}
          >
            {t("settings.logging.purge")}
          </button>
          <button
            type="button"
            className="settings-btn"
            onClick={() => void refresh()}
            disabled={busy}
          >
            {t("settings.logging.refresh")}
          </button>
        </div>

        {info && (
          <div className="settings-field">
            <p className="settings-hint">
              {t("settings.logging.folder")} <code>{info.dir}</code>
              {info.active_path && (
                <>
                  <br />
                  {t("settings.logging.active")} <code>{info.active_path}</code>
                </>
              )}
            </p>
            {info.files.length > 0 && (
              <ul className="settings-log-files">
                {info.files.map((f) => (
                  <li key={f.name}>
                    <code>{f.name}</code> — {formatBytes(f.size_bytes)}
                    {f.is_active ? t("settings.logging.fileActiveSuffix") : ""}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}

        {status && <p className="settings-hint">{status}</p>}
      </section>

      <section className="settings-section">
        <h3 className="settings-section-title">
          {t("settings.analytics.title")}
        </h3>
        <p className="settings-section-help">{t("settings.analytics.help")}</p>

        <div className="settings-field settings-field--inline">
          <label>
            <input
              type="checkbox"
              checked={analyticsInfo?.enabled ?? false}
              disabled={analyticsBusy || !analyticsInfo}
              onChange={(e) => applyAnalytics(e.target.checked)}
            />{" "}
            {t("settings.analytics.enable")}
          </label>
        </div>

        <p className="settings-hint">{t("settings.analytics.privacyNote")}</p>

        {analyticsStatus && <p className="settings-hint">{analyticsStatus}</p>}
      </section>
    </>
  );
}
