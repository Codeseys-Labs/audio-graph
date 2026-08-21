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
 *   - Escape             → close the Settings modal
 *
 * Typing-context guard: shortcuts are ignored when the event target is an
 * `<input>`, `<textarea>`, or any element with `contenteditable`. Escape is
 * still honored for closing Settings so users can bail out without losing
 * focus awkwardly mid-edit.
 *
 * SHELL-R2 (plan §R2, ADR-0046) note: Escape no longer has a SessionsBrowser
 * branch. Sessions stopped being a modal in R2 — it's the "sessions"
 * destination, always rendered, with nothing to "close" — so an Escape
 * branch keyed on `sessionsBrowserOpen` would just swallow the keystroke
 * (preventDefault + early-return with zero visible effect) for the rest of
 * the session once that flag latches true. `sessionsBrowserOpen` itself
 * stays wired in the store (SHELL-R1's explicit "state/actions untouched"
 * decision, and `App.contract.test.tsx`/`App.test.tsx` set it directly and
 * must stay byte-identical per R2's own acceptance criteria) — it simply
 * has no remaining reader anywhere in the app now that this branch is gone.
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

      // Escape closes the Settings modal. Intentionally works even inside
      // inputs so you can back out of a field without reaching for the
      // mouse. Sessions is a destination, not a modal (SHELL-R2) — see the
      // module doc — so there is no second branch here anymore.
      if (e.key === "Escape") {
        if (state.settingsOpen) {
          e.preventDefault();
          state.closeSettings();
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
          // SHELL-R3 (plan §R3, ADR-0046): mirror the NOW STRIP's Start
          // button exactly — `startCaptureAndTranscribe`, not the bare
          // `startCapture`, so the hotkey and the click both compose the
          // same ONE START behavior instead of silently diverging.
          void state.startCaptureAndTranscribe();
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
