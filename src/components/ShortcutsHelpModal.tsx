/**
 * Keyboard shortcuts help modal — user-facing reference for the global
 * hotkeys registered by `useKeyboardShortcuts`.
 *
 * Kept in sync manually with the hook (the `SHORTCUTS` list here is
 * documentation, not a source of truth). Opened via Cmd/Ctrl+/ or "?"
 * and dismissed via Escape, the close button, or a backdrop click.
 *
 * Props:
 *   - `onClose`: invoked on dismiss; parent (`App.tsx`) clears its
 *     local `shortcutsOpen` state.
 *
 * Focus-trapped via `useFocusTrap`.
 */
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ONBOARDING_HANDOFF_SEEN_KEY } from "../constants/storageKeys";
import { useFocusTrap } from "../hooks/useFocusTrap";
import IconButton from "./IconButton";

interface ShortcutsHelpModalProps {
  onClose: () => void;
}

// localStorage flag App.tsx uses to gate the post-Express onboarding hand-off
// nudge (B20): "1" means the user has already seen it, so it stays hidden.
// Clearing the key (below) re-arms the hand-off so it can surface again on the
// next Express-Setup dismissal / launch. The key now lives in the shared
// constants module (src/constants/storageKeys.ts), so this modal and App.tsx
// reference one source of truth and can no longer drift apart (B34).

type ShortcutEntry = {
  id: string;
  keys: string[];
};

// Mirrors the bindings declared in useKeyboardShortcuts.ts. Keep in sync
// manually — this list is user-facing documentation, not the source of truth.
const SHORTCUTS: readonly ShortcutEntry[] = [
  { id: "toggleCapture", keys: ["Cmd/Ctrl", "R"] },
  { id: "openSettings", keys: ["Cmd/Ctrl", ","] },
  { id: "openSessions", keys: ["Cmd/Ctrl", "Shift", "S"] },
  { id: "openHelp", keys: ["Cmd/Ctrl", "/"] },
  { id: "closeModal", keys: ["Esc"] },
];

function ShortcutsHelpModal({ onClose }: ShortcutsHelpModalProps) {
  const { t } = useTranslation();
  const modalRef = useFocusTrap<HTMLDivElement>();
  // Latches once the user re-arms the getting-started hand-off so we can show a
  // brief inline confirmation. App.tsx picks the cleared key up on its own
  // schedule (next Express-Setup dismissal / launch); this modal does not — and
  // must not — reach into App's render state directly.
  const [handoffReArmed, setHandoffReArmed] = useState(false);

  // Re-arm the B20 onboarding hand-off by clearing its show-once flag. App.tsx
  // owns when the nudge actually renders; we only flip the persisted gate.
  const handleShowGettingStarted = () => {
    try {
      localStorage.removeItem(ONBOARDING_HANDOFF_SEEN_KEY);
      setHandoffReArmed(true);
    } catch {
      /* ignore storage quota/availability errors — best-effort re-arm */
    }
  };

  // Local Escape handler: the global useKeyboardShortcuts hook only closes
  // Settings/SessionsBrowser on Escape. We don't want to add this modal to
  // that hook (the task forbids touching it), so handle Escape here.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  return (
    <div
      className="settings-overlay"
      role="none"
      onClick={onClose}
      onKeyDown={(e) => {
        if (e.key === "Escape") onClose();
      }}
    >
      <div
        ref={modalRef}
        className="settings-modal shortcuts-modal"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="shortcuts-modal-title"
        tabIndex={-1}
      >
        <div className="settings-header">
          <h2 id="shortcuts-modal-title" className="settings-header__title">
            {t("shortcuts.title")}
          </h2>
          <IconButton
            icon="close"
            label={t("shortcuts.close")}
            variant="ghost"
            className="settings-header__close"
            onClick={onClose}
          />
        </div>

        <div className="settings-content">
          <ul className="shortcuts-list">
            {SHORTCUTS.map((s) => (
              <li key={s.id} className="shortcuts-list__item">
                <span className="shortcuts-list__keys">
                  {s.keys.map((k, i) => (
                    <span
                      key={`${s.id}-${k}`}
                      className="shortcuts-list__key-group"
                    >
                      <kbd className="shortcuts-list__kbd">{k}</kbd>
                      {i < s.keys.length - 1 && (
                        <span
                          className="shortcuts-list__plus"
                          aria-hidden="true"
                        >
                          +
                        </span>
                      )}
                    </span>
                  ))}
                </span>
                <span className="shortcuts-list__desc">
                  {t(`shortcuts.items.${s.id}`)}
                </span>
              </li>
            ))}
          </ul>

          <div className="shortcuts-getting-started">
            <button
              type="button"
              className="settings-btn settings-btn--secondary"
              onClick={handleShowGettingStarted}
            >
              {t("shortcuts.showGettingStarted")}
            </button>
            {handoffReArmed && (
              <p className="settings-hint" role="status">
                {t("shortcuts.gettingStartedReArmed")}
              </p>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

export default ShortcutsHelpModal;
