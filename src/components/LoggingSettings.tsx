/**
 * Logging settings — file-logging controls for capturing a session's logs as
 * shareable feedback when reporting/fixing issues.
 *
 * Self-contained: unlike the other Settings sub-forms it does NOT go through
 * the shared reducer + footer "Save". It reads/writes via dedicated Tauri
 * commands (`get_log_info`, `set_logging_config`, `purge_logs_cmd`,
 * `open_logs_dir`) and applies changes immediately, because logging is a
 * side-effecting runtime concern rather than provider config.
 *
 * Parent: `SettingsPage.tsx`.
 */

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

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

const LEVELS = ["error", "warn", "info", "debug", "trace"] as const;

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

  return (
    <section className="settings-section">
      <h3 className="settings-section-title">{t("settings.logging.title")}</h3>
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
        <label htmlFor="log-level-select">{t("settings.logging.level")}</label>
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
  );
}
