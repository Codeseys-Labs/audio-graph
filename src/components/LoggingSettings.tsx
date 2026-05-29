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
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
  const [info, setInfo] = useState<LogInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setInfo(await invoke<LogInfo>("get_log_info"));
    } catch (e) {
      setStatus(`Failed to read log info: ${e}`);
    }
  }, []);

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
        setStatus("Applied.");
      } catch (e) {
        setStatus(`Failed: ${e}`);
      } finally {
        setBusy(false);
      }
    },
    [info],
  );

  const purge = useCallback(async () => {
    setBusy(true);
    setStatus(null);
    try {
      const removed = await invoke<number>("purge_logs_cmd");
      setStatus(`Purged ${removed} archived log file(s).`);
      await refresh();
    } catch (e) {
      setStatus(`Purge failed: ${e}`);
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const openDir = useCallback(async () => {
    try {
      await invoke<string>("open_logs_dir");
    } catch (e) {
      setStatus(`Could not open folder: ${e}`);
    }
  }, []);

  return (
    <section className="settings-section">
      <h3 className="settings-section-title">Logging</h3>
      <p className="settings-section-help">
        Write a log file you can attach as feedback when reporting issues. By
        default the previous log is archived and a fresh one is started each
        launch.
      </p>

      <div className="settings-field settings-field--inline">
        <label>
          <input
            type="checkbox"
            checked={info?.enabled ?? true}
            disabled={busy || !info}
            onChange={(e) => apply({ enabled: e.target.checked })}
          />
          {" "}Write logs to a file
        </label>
      </div>

      <div className="settings-field">
        <label htmlFor="log-level-select">Log level</label>
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
        <label>Startup file mode</label>
        <label className="settings-radio">
          <input
            type="radio"
            name="log-file-mode"
            value="archive"
            checked={(info?.mode ?? "archive") === "archive"}
            disabled={busy || !info?.enabled}
            onChange={() => apply({ mode: "archive" })}
          />
          {" "}Archive previous, start fresh (recommended)
        </label>
        <label className="settings-radio">
          <input
            type="radio"
            name="log-file-mode"
            value="overwrite"
            checked={info?.mode === "overwrite"}
            disabled={busy || !info?.enabled}
            onChange={() => apply({ mode: "overwrite" })}
          />
          {" "}Overwrite the single log file each launch
        </label>
      </div>

      <div className="settings-field settings-field--inline">
        <button className="settings-btn" onClick={openDir} disabled={!info}>
          Open logs folder
        </button>
        <button className="settings-btn" onClick={purge} disabled={busy || !info}>
          Purge archived logs
        </button>
        <button className="settings-btn" onClick={() => void refresh()} disabled={busy}>
          Refresh
        </button>
      </div>

      {info && (
        <div className="settings-field">
          <p className="settings-hint">
            Folder: <code>{info.dir}</code>
            {info.active_path && (
              <>
                <br />
                Active: <code>{info.active_path}</code>
              </>
            )}
          </p>
          {info.files.length > 0 && (
            <ul className="settings-log-files">
              {info.files.map((f) => (
                <li key={f.name}>
                  <code>{f.name}</code> — {formatBytes(f.size_bytes)}
                  {f.is_active ? " (active)" : ""}
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
