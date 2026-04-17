import { useEffect } from "react";
import { useAudioGraphStore } from "../store";

/**
 * Registers global keyboard shortcuts for the app. Should be called once near
 * the root (alongside `useTauriEvents`).
 *
 * Bindings:
 *   - Cmd/Ctrl+R         → toggle capture (start/stop)
 *   - Cmd/Ctrl+,         → open Settings
 *   - Cmd/Ctrl+Shift+S   → open SessionsBrowser
 *   - Escape             → close any open modal (Settings / SessionsBrowser)
 *
 * Typing-context guard: shortcuts are ignored when the event target is an
 * `<input>`, `<textarea>`, or any element with `contenteditable`. Escape is
 * still honored for closing modals so users can bail out without losing focus
 * awkwardly mid-edit.
 */
export function useKeyboardShortcuts(): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const isTypingContext =
        !!target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable);

      const state = useAudioGraphStore.getState();
      const mod = e.metaKey || e.ctrlKey;

      // Escape closes any open modal. Intentionally works even inside inputs
      // so you can back out of a field without reaching for the mouse.
      if (e.key === "Escape") {
        if (state.settingsOpen) {
          e.preventDefault();
          state.closeSettings();
          return;
        }
        if (state.sessionsBrowserOpen) {
          e.preventDefault();
          state.closeSessionsBrowser();
          return;
        }
        return;
      }

      // All remaining shortcuts require the modifier key and must skip typing
      // contexts so they don't collide with e.g. Cmd+R in a URL-style field.
      if (!mod) return;
      if (isTypingContext) return;

      // Cmd/Ctrl+Shift+S → Sessions browser. Must be checked before the
      // plain Cmd/Ctrl+R / Cmd/Ctrl+, branches since those don't use shift.
      if (e.shiftKey && (e.key === "s" || e.key === "S")) {
        e.preventDefault();
        state.openSessionsBrowser();
        return;
      }

      // Any remaining shortcut here must NOT have shift.
      if (e.shiftKey) return;

      if (e.key === "r" || e.key === "R") {
        e.preventDefault();
        if (state.isCapturing) {
          void state.stopCapture();
        } else {
          void state.startCapture();
        }
        return;
      }

      if (e.key === ",") {
        e.preventDefault();
        state.openSettings();
        return;
      }
    };

    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);
}
