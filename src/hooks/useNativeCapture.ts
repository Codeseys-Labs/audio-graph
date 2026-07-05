import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { safeInvoke } from "../analytics/safeInvoke";
import { useAudioGraphStore } from "../store";

/**
 * Native capture-UX bridge (epic 5c24): system tray recording indicator
 * (audio-graph-a156) + global start/stop shortcut (audio-graph-f67e).
 *
 * Call once at the app root (alongside `useTauriEvents` / `useKeyboardShortcuts`).
 *
 * Responsibilities:
 *   1. **Global shortcut** — the backend registers Cmd/Ctrl+Shift+R globally
 *      (fires even when unfocused) and emits `global-shortcut-toggle-capture`.
 *      We route it through the SAME store `startCapture`/`stopCapture` path the
 *      UI Start/Stop button uses — no parallel logic — so a no-source-selected
 *      start still surfaces the existing "No audio source selected" error.
 *   2. **Tray Stop menu item** — the backend emits `tray-stop-capture`; we call
 *      the store's `stopCapture` (same path as the UI Stop button).
 *   3. **Tray indicator sync** — capture state lives frontend-side (the store's
 *      `isCapturing` spans multiple sources), so we push it to the tray via the
 *      `update_tray_capturing` command: the backend swaps the icon (red dot),
 *      refreshes the CONTENT-FREE duration tooltip, and enables/disables the
 *      tray Stop item. We send only a boolean + an elapsed-seconds count —
 *      never any captured content (transcript/notes/titles).
 *
 * Double-fire note: the window-focus `useKeyboardShortcuts` owns plain
 * Cmd/Ctrl+R (no Shift) and explicitly ignores every Shift combo except
 * Shift+S. The global shortcut is Cmd/Ctrl+**Shift**+R — a distinct
 * accelerator — so the two never both fire for one keypress.
 */
export function useNativeCapture(): void {
  const isCapturing = useAudioGraphStore((s) => s.isCapturing);
  const captureStartTime = useAudioGraphStore((s) => s.captureStartTime);

  // ── Global shortcut + tray Stop → store toggle path ──────────────────────
  useEffect(() => {
    let unlisten: Array<(() => void) | null> = [];
    let cancelled = false;

    async function setup() {
      const handles = await Promise.all([
        listen("global-shortcut-toggle-capture", () => {
          // Read the latest state at fire time (not the effect's closure) so a
          // stale `isCapturing` can't flip the wrong direction.
          const { isCapturing: capturing } = useAudioGraphStore.getState();
          if (capturing) {
            void useAudioGraphStore.getState().stopCapture();
          } else {
            void useAudioGraphStore.getState().startCapture();
          }
        }).catch((err) => {
          console.error(
            "Failed to subscribe to global-shortcut-toggle-capture:",
            err,
          );
          return null;
        }),
        listen("tray-stop-capture", () => {
          void useAudioGraphStore.getState().stopCapture();
        }).catch((err) => {
          console.error("Failed to subscribe to tray-stop-capture:", err);
          return null;
        }),
      ]);
      if (cancelled) {
        for (const fn of handles) if (fn) fn();
        return;
      }
      unlisten = handles;
    }

    setup();
    return () => {
      cancelled = true;
      for (const fn of unlisten) if (fn) fn();
    };
    // startCapture/stopCapture are stable store actions; getState() reads the
    // live values, so this subscribes exactly once.
  }, []);

  // ── Tray indicator sync (icon swap + content-free duration tooltip) ───────
  useEffect(() => {
    // Push the current capture state to the tray. Fire-and-forget; a tray sync
    // failure must never disrupt capture, and the backend no-ops when there is
    // no tray (headless / unsupported platform).
    const push = () => {
      const elapsed =
        isCapturing && captureStartTime !== null
          ? Math.max(0, Math.floor((Date.now() - captureStartTime) / 1000))
          : null;
      void safeInvoke("update_tray_capturing", {
        capturing: isCapturing,
        elapsedSecs: elapsed,
      }).catch(() => {
        // Swallowed: tray sync is best-effort. safeInvoke already relays a
        // diagnostic; the capture flow is unaffected.
      });
    };

    push(); // immediate update on transition
    if (!isCapturing || captureStartTime === null) return;

    // Keep the content-free duration tooltip live while capturing.
    const interval = setInterval(push, 1000);
    return () => clearInterval(interval);
  }, [isCapturing, captureStartTime]);
}
