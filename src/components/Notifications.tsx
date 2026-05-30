/**
 * Notifications host (ADR-0011).
 *
 * Single, unified surface for transient feedback. Replaces the old dual system
 * (the single-slot module `Toast` + the persistent root `error-toast`):
 *
 *  - Reads the store `notifications` queue (`notify(...)`), rendering them
 *    stacked (newest at the bottom). Non-sticky items auto-dismiss; sticky
 *    items stay until dismissed.
 *  - Bridges the legacy single `error` string as a sticky error notification so
 *    the existing `setError`/`clearError` contract keeps working through one
 *    visual surface.
 *
 * Stacks above modals via `--z-notification` (ADR-0009). Severity controls the
 * accent (semantic tokens) and aria-live politeness (assertive for errors).
 *
 * Mounted once in `App.tsx`; no props.
 */
import { useEffect, useRef } from "react";
import { useAudioGraphStore } from "../store";
import type { NotificationSeverity } from "../types";
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
            label="Dismiss notification"
            size={14}
            variant="ghost"
            onClick={() => dismissNotification(n.id)}
          />
        </div>
      ))}

      {error && (
        <div
          className="notification notification--error"
          role="alert"
          aria-live="assertive"
        >
          <Icon name="error" size={18} className="notification__icon" />
          <div className="notification__body">
            <div className="notification__message">{error}</div>
          </div>
          <IconButton
            icon="close"
            label="Dismiss error"
            size={14}
            variant="ghost"
            onClick={clearError}
          />
        </div>
      )}
    </div>
  );
}
