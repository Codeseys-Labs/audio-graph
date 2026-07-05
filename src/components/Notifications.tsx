/**
 * Notifications host (ADR-0011).
 *
 * Single, unified surface for transient feedback. Replaces the old dual system
 * (the single-slot module `Toast` + the persistent root `error-toast`):
 *
 *  - Reads the store `notifications` queue (`notify(...)`), rendering them
 *    stacked (newest at the bottom). Non-sticky items auto-dismiss; sticky
 *    items stay until dismissed.
 *  - Bridges the legacy single `error` string as an error notification so the
 *    existing `setError`/`clearError` contract keeps working through one visual
 *    surface. The raw string is run through the error humanizer (ADR-0011 / A2)
 *    so backend/IPC failures render plain-language copy + Details, never a raw
 *    `TypeError`; transient/startup-probe classes auto-expire like queued items.
 *
 * Stacks above modals via `--z-notification` (ADR-0009). Severity controls the
 * accent (semantic tokens) and aria-live politeness (assertive for errors).
 *
 * Mounted once in `App.tsx`; no props.
 */
import { useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../store";
import type { NotificationSeverity } from "../types";
import { humanizeError } from "../utils/humanizeError";
import HumanizedError from "./HumanizedError";
import Icon, { type IconName } from "./Icon";
import IconButton from "./IconButton";

const AUTO_DISMISS_MS = 4000;

const SEVERITY_ICON: Record<NotificationSeverity, IconName> = {
  info: "info",
  success: "success",
  warning: "warning",
  error: "error",
};

export default function Notifications() {
  const { t } = useTranslation();
  const notifications = useAudioGraphStore((s) => s.notifications);
  const dismissNotification = useAudioGraphStore((s) => s.dismissNotification);
  const error = useAudioGraphStore((s) => s.error);
  const clearError = useAudioGraphStore((s) => s.clearError);

  // Track timers so each non-sticky notification auto-dismisses exactly once.
  const timers = useRef<Map<string, number>>(new Map());
  useEffect(() => {
    for (const n of notifications) {
      if (n.sticky) continue;
      if (timers.current.has(n.id)) continue;
      const handle = window.setTimeout(() => {
        timers.current.delete(n.id);
        dismissNotification(n.id);
      }, AUTO_DISMISS_MS);
      timers.current.set(n.id, handle);
    }
    // Clean up timers for notifications that are gone.
    const live = new Set(notifications.map((n) => n.id));
    for (const [id, handle] of timers.current) {
      if (!live.has(id)) {
        window.clearTimeout(handle);
        timers.current.delete(id);
      }
    }
  }, [notifications, dismissNotification]);

  useEffect(() => {
    const map = timers.current;
    return () => {
      for (const handle of map.values()) window.clearTimeout(handle);
      map.clear();
    };
  }, []);

  // Humanize the legacy `error` string (A2): map known backend/IPC failure
  // shapes to plain-language copy + Details, and derive whether this class is
  // transient/startup noise (severity=warning) — those auto-expire like queued
  // items instead of sticking as an assertive banner forever.
  const humanizedError = useMemo(
    () => (error ? humanizeError(error) : null),
    [error],
  );

  // Auto-dismiss the legacy error when its class is transient (probe/startup
  // failures). Re-arms whenever the underlying string changes; sticky
  // (severity=error) classes never get a timer, matching queued-item behavior.
  useEffect(() => {
    if (!humanizedError?.transient) return;
    const handle = window.setTimeout(clearError, AUTO_DISMISS_MS);
    return () => window.clearTimeout(handle);
  }, [humanizedError, clearError]);

  if (notifications.length === 0 && !error) return null;

  return (
    <div className="notifications">
      {notifications.map((n) => (
        <div
          key={n.id}
          className={`notification notification--${n.severity}`}
          role={n.severity === "error" ? "alert" : "status"}
          aria-live={n.severity === "error" ? "assertive" : "polite"}
        >
          <Icon
            name={SEVERITY_ICON[n.severity]}
            size={18}
            className="notification__icon"
          />
          <div className="notification__body">
            <div className="notification__message">{n.message}</div>
            {n.action && (
              <button
                type="button"
                className="btn btn--ghost btn--sm notification__action"
                onClick={() => {
                  n.action?.onClick();
                  dismissNotification(n.id);
                }}
              >
                {n.action.label}
              </button>
            )}
          </div>
          <IconButton
            icon="close"
            label={t("notifications.dismiss")}
            size={14}
            variant="ghost"
            onClick={() => dismissNotification(n.id)}
          />
        </div>
      ))}

      {error && humanizedError && (
        <div
          className={`notification notification--${humanizedError.severity}`}
          role={humanizedError.severity === "error" ? "alert" : "status"}
          aria-live={
            humanizedError.severity === "error" ? "assertive" : "polite"
          }
        >
          <Icon
            name={SEVERITY_ICON[humanizedError.severity]}
            size={18}
            className="notification__icon"
          />
          <div className="notification__body">
            {/* No `onRetry` here: the legacy `error` string is disconnected
                from its originating call site, so a Retry that only cleared
                the banner would be misleading (the X already dismisses). Real
                retry lives where a reload callback exists (e.g. the panel). */}
            <HumanizedError raw={error} />
          </div>
          <IconButton
            icon="close"
            label={t("notifications.dismissError")}
            size={14}
            variant="ghost"
            onClick={clearError}
          />
        </div>
      )}
    </div>
  );
}
