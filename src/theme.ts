/**
 * Theme choice + application (ADR-0009, Wave 4).
 *
 * The visual theme is a semantic-token swap defined in `styles.css`:
 *   - `[data-theme="light"]` / `[data-theme="dark"]` — an explicit user choice.
 *   - `@media (prefers-color-scheme: light)` on the bare `:root` — the system
 *     default, used when the user has not pinned a theme ("system").
 *
 * This module is the single place that resolves the persisted choice and
 * reflects it onto `document.documentElement.dataset.theme`. It is invoked
 * once from `main.tsx` before React mounts (so the first paint already has the
 * right palette and there is no flash), and again from the store's `setTheme`
 * whenever the user changes it in Settings.
 */

/** User-facing theme preference. `system` defers to `prefers-color-scheme`. */
export type ThemeChoice = "system" | "light" | "dark";

/** localStorage key. Namespaced like the app's other UI prefs (`ag.*`). */
export const THEME_STORAGE_KEY = "ag.theme";

const THEME_CHOICES: readonly ThemeChoice[] = ["system", "light", "dark"];

function isThemeChoice(value: string | null): value is ThemeChoice {
  return value !== null && (THEME_CHOICES as readonly string[]).includes(value);
}

/** Read the persisted theme choice, defaulting to `system`. */
export function readStoredTheme(): ThemeChoice {
  try {
    const stored = localStorage.getItem(THEME_STORAGE_KEY);
    return isThemeChoice(stored) ? stored : "system";
  } catch {
    return "system";
  }
}

/**
 * Reflect `choice` onto the document root. For `system` we remove the
 * `data-theme` attribute so the `prefers-color-scheme` media query in
 * `styles.css` takes over; for an explicit choice we set it so the
 * `[data-theme="…"]` rules win regardless of the OS setting.
 */
export function applyTheme(choice: ThemeChoice): void {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  if (choice === "system") {
    delete root.dataset.theme;
  } else {
    root.dataset.theme = choice;
  }
}

/** Persist + apply a theme choice. Tolerates an unavailable localStorage. */
export function persistTheme(choice: ThemeChoice): void {
  try {
    localStorage.setItem(THEME_STORAGE_KEY, choice);
  } catch {
    /* ignore — storage may be unavailable (private mode, etc.) */
  }
  applyTheme(choice);
}
