/**
 * "Find a setting" jump palette (audio-graph-4850, settings T4b, synthesis
 * §T4b). Mounted once near the app root (`App.tsx`, alongside
 * `useKeyboardShortcuts()`) so ⌘F / Ctrl+F / `/` open it from anywhere, not
 * only while the Settings modal happens to already be open.
 *
 * JUMP, not filter (synthesis §T4b): Enter navigates via the SAME external
 * mechanism every other outside-the-modal caller uses —
 * `useAudioGraphStore.getState().openSettings(route)` / T1's
 * `pendingSettingsRoute` — it never mounts a panel to search inside it (that
 * would require mounting all 8 panels simultaneously and duplicate ~50 DOM
 * ids across the app, defeating `focusSettingsField`'s `getElementById`
 * lookups).
 *
 * ARIA: WAI-ARIA "combobox with a listbox popup, list autocomplete with
 * manual selection" pattern — `role="combobox"` on the input itself,
 * `aria-controls` pointing at the `role="listbox"` results,
 * `aria-activedescendant` tracking the highlighted `role="option"`. This is
 * the REAL rendered component under test (not a hand-built test fixture —
 * T3's review caught exactly that gap on the annotated chooser).
 *
 * The panel itself is `role="dialog"` + `aria-modal="true"` (a11y review,
 * T4b), with a minimal focus trap: Tab is swallowed while open (the
 * mount-level capture listener below), since the combobox is the only real
 * focusable element in this pattern and there is nothing else inside the
 * panel to cycle to. This stops the KEYBOARD path into background content
 * (previously: Tab moved real focus behind the still-open overlay).
 * Disclosed limitation, not fixed here: background content is NOT marked
 * `inert`/`aria-hidden`, so a screen reader's own browse-mode navigation
 * (not Tab) can still reach it — doing so would mean toggling `inert` on
 * every sibling of this component in `App.tsx`'s render tree, a
 * structural change to the app root out of scope for this fix.
 *
 * Closed state renders NOTHING (`return null`) — the 210+ existing role
 * queries elsewhere in the app must never see a stray combobox/listbox node
 * just because this component happens to be mounted.
 */

import { useEffect, useId, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAudioGraphStore } from "../../store";
import { PALETTE_ENTRIES, type PaletteEntry } from "./settingsPaletteManifest";

function isTypingContext(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  return (
    !!el &&
    (el.tagName === "INPUT" ||
      el.tagName === "TEXTAREA" ||
      el.isContentEditable)
  );
}

/** `${kind label} — ${provider qualifier}` for provider/credential/model
 * entries; the bare kind label (no qualifier — tabs are never ambiguous)
 * for tab entries. */
function entryLabel(entry: PaletteEntry, t: (key: string) => string): string {
  const kind = t(entry.kindLabelKey);
  return entry.qualifier ? `${kind} — ${entry.qualifier}` : kind;
}

export default function SettingsFindPalette() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listboxId = useId();

  const close = () => {
    setOpen(false);
    setQuery("");
    setActiveIndex(0);
  };

  // `open`/`close` read via refs inside the listener below (registered ONCE,
  // empty deps — see that effect's comment for why re-registering per
  // `open` toggle broke Escape priority).
  const openRef = useRef(open);
  openRef.current = open;
  const closeRef = useRef(close);
  closeRef.current = close;

  // Global open-shortcut AND in-palette Escape, ONE capture-phase window
  // listener, registered ONCE at mount and never torn down/re-added while
  // this component stays mounted (settings T4b review fix, audio-graph-4850
  // — see below for why that matters).
  //
  // The Settings modal's own dialog stops propagation on every keydown that
  // bubbles through it (`SettingsPage.tsx`'s `.settings-modal` `onKeyDown`),
  // so a bubble-phase window listener would never fire while focus is
  // inside an open Settings modal — hence capture phase on `window`, the
  // same pattern `useSettingsController.tsx`'s own Escape interceptor uses.
  //
  // Escape is handled HERE, at capture phase, rather than in the input's own
  // (bubble-phase) `onKeyDown`, and in the SAME listener as the open
  // shortcut rather than a separate one keyed off `open`. Two capture-phase
  // window listeners can both want Escape: this one (closing the palette)
  // and `useSettingsController.tsx`'s (opening the dirty-draft confirm bar
  // / closing Settings). Multiple listeners on the SAME target+phase run in
  // REGISTRATION order, and only `stopImmediatePropagation()` (not
  // `stopPropagation()`) stops a later-registered sibling listener on that
  // same target — so whichever of the two registers FIRST wins deterministically.
  // `SettingsFindPalette` mounts unconditionally at the App root
  // (`App.tsx`) on the very first render, before Settings has ever been
  // opened; the controller's listener only registers later, when the user
  // actually opens Settings. Keeping THIS listener registered from that
  // first mount onward (never removed/re-added by an `open` dependency)
  // guarantees it is always the earlier registration, so it always wins:
  // the palette's own Escape always closes just the palette, self-contained,
  // never double-closing the Settings modal underneath it (clean-draft
  // case) and never getting silently swallowed by the modal's dirty-draft
  // interceptor before the palette itself ever sees the key (dirty-draft
  // case) — both were previously reachable bugs when Escape was only
  // handled at bubble phase on the input.
  useEffect(() => {
    const onKeyDownCapture = (e: KeyboardEvent) => {
      if (openRef.current) {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopImmediatePropagation();
          closeRef.current();
          return;
        }
        // Minimal focus trap (a11y review — the combobox is the ONLY
        // focusable element in this ARIA pattern; options are virtual-focus
        // only, via `aria-activedescendant`, never real DOM `tabindex`). Tab
        // must not move real focus into the app or the Settings modal
        // hidden behind this overlay while it's open — there is nothing
        // else inside the palette to cycle to, so it's simply swallowed
        // rather than cycled.
        if (e.key === "Tab") {
          e.preventDefault();
          return;
        }
        return; // other keys: the input's own onKeyDown (ArrowUp/Down/Enter, typing) handles them
      }
      if (isTypingContext(e.target)) return;
      const mod = e.metaKey || e.ctrlKey;
      const isFindShortcut =
        (mod && (e.key === "f" || e.key === "F")) || (!mod && e.key === "/");
      if (!isFindShortcut) return;
      e.preventDefault();
      setOpen(true);
    };
    window.addEventListener("keydown", onKeyDownCapture, true);
    return () => window.removeEventListener("keydown", onKeyDownCapture, true);
  }, []);

  useEffect(() => {
    if (open) inputRef.current?.focus();
  }, [open]);

  const results = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (needle === "") return PALETTE_ENTRIES;
    return PALETTE_ENTRIES.filter((entry) =>
      entryLabel(entry, t).toLowerCase().includes(needle),
    );
  }, [query, t]);

  // A fresh search always highlights the first result.
  // biome-ignore lint/correctness/useExhaustiveDependencies: intentionally re-runs only on query text, not on every `results` identity change
  useEffect(() => {
    setActiveIndex(0);
  }, [query]);

  if (!open) return null;

  const jump = (entry: PaletteEntry) => {
    useAudioGraphStore.getState().openSettings({
      tab: entry.tab,
      fieldId: entry.fieldId,
      activate: entry.activate,
    });
    close();
  };

  // Defensive clamp for render safety: `results` can shrink between the
  // query changing and the reset-to-0 effect above committing.
  const safeActiveIndex =
    results.length === 0 ? -1 : Math.min(activeIndex, results.length - 1);
  const activeEntry =
    safeActiveIndex >= 0 ? results[safeActiveIndex] : undefined;
  const activeOptionId = activeEntry
    ? `${listboxId}-option-${safeActiveIndex}`
    : undefined;

  return (
    <div className="settings-palette-overlay" role="none" onClick={close}>
      {/* Stops the click-to-dismiss backdrop above from firing when the click
          actually landed on the palette itself; it introduces no NEW
          keyboard-operable surface of its own — Escape is owned entirely by
          the mount-level capture-phase window listener above (deterministic
          priority over the Settings modal's own Escape interceptor; a
          bubble-phase handler here or on the input could never win that
          race), and ArrowUp/Down/Enter/typing are the input's own
          `onKeyDown` — so it is intentionally role="none" with no added
          handler here. */}
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: onClick here is only a
          click-bubbling guard (stops the backdrop's dismiss-on-click above
          from also firing for a click that landed inside the panel), not a
          keyboard-operable action of its own — same rationale as the two
          ignores below for the option rows. */}
      <div
        className="settings-palette"
        role="dialog"
        aria-modal="true"
        aria-label={t("settings.palette.title")}
        onClick={(e) => e.stopPropagation()}
      >
        <input
          ref={inputRef}
          role="combobox"
          type="text"
          className="settings-palette__input"
          aria-expanded={true}
          aria-controls={listboxId}
          aria-autocomplete="list"
          aria-activedescendant={activeOptionId}
          aria-label={t("settings.palette.title")}
          placeholder={t("settings.palette.placeholder")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            // Escape is handled at capture phase on `window` (see the
            // mount effect above) — it never reaches this bubble-phase
            // handler at all, deliberately, so there's no branch for it
            // here.
            if (e.key === "ArrowDown") {
              e.preventDefault();
              if (results.length > 0) {
                setActiveIndex((i) => (i + 1) % results.length);
              }
              return;
            }
            if (e.key === "ArrowUp") {
              e.preventDefault();
              if (results.length > 0) {
                setActiveIndex(
                  (i) => (i - 1 + results.length) % results.length,
                );
              }
              return;
            }
            if (e.key === "Enter") {
              e.preventDefault();
              if (activeEntry) jump(activeEntry);
            }
          }}
        />
        <p className="settings-palette__hint">{t("settings.palette.hint")}</p>
        {results.length === 0 ? (
          <p className="settings-palette__empty">
            {t("settings.palette.empty")}
          </p>
        ) : (
          <div
            id={listboxId}
            role="listbox"
            aria-label={t("settings.palette.resultsLabel")}
            className="settings-palette__list"
          >
            {results.map((entry, index) => (
              // Options are deliberately NOT focusable/tabbable — the WAI-ARIA
              // "combobox with list autocomplete" pattern keeps real DOM focus
              // on the input the whole time and drives selection purely via
              // `aria-activedescendant` (ArrowUp/Down/Enter on the input,
              // handled above); this onClick is a mouse-only convenience path
              // to the SAME `jump`, not a second, independently-operable
              // keyboard target.
              // biome-ignore lint/a11y/useKeyWithClickEvents: see comment above — mouse-only convenience; Enter on the combobox input is the keyboard equivalent
              // biome-ignore lint/a11y/useFocusableInteractive: see comment above — options are intentionally not focusable in this ARIA pattern; virtual focus via aria-activedescendant
              <div
                key={entry.id}
                id={`${listboxId}-option-${index}`}
                role="option"
                aria-selected={index === safeActiveIndex}
                className={`settings-palette__option ${
                  index === safeActiveIndex
                    ? "settings-palette__option--active"
                    : ""
                }`}
                onMouseEnter={() => setActiveIndex(index)}
                onClick={() => jump(entry)}
              >
                {entryLabel(entry, t)}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
