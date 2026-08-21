/**
 * Shell layout tier hook (SHELL-R7, plan §R7, ADR-0046).
 *
 * Generalizes the single-boolean matchMedia pattern at
 * `useSettingsController.tsx:1614-1621` (one query, one `railHorizontal`
 * boolean) to three named tiers so `App.tsx`'s rail/content/aside regions
 * can each decide, from ONE piece of state, whether they're pinned in the
 * flow or collapsed behind a focus-trapped drawer:
 *
 *   - `wide`     (>=1280px):  rail + content + aside are all pinned.
 *   - `standard` (1024-1279px): aside becomes a right drawer; rail stays
 *     pinned.
 *   - `compact`  (<1024px, i.e. 768-1023px AND everything narrower): rail
 *     AND aside both become focus-trapped drawers.
 *
 * There is deliberately NO separate "stack" tier below 768px — Design 3's
 * own weakness analysis cut it; full mobile stacking belongs to the 8055
 * epoch, not this shell. `compact` is the floor: nothing changes again
 * between 767px and 0.
 *
 * WCAG 1.4.4 (200% zoom): the app's default 1400px window at 200% zoom is
 * ~700px CSS width, which lands squarely in `compact` — this hook's tier
 * plumbing (and the drawer chrome it drives) IS the zoom-compliance
 * mechanism, not a separate concern bolted on afterward.
 *
 * Same idiom as the pattern it generalizes: `apply()` runs once synchronously
 * on mount so the very first render reflects the real viewport, then two
 * `matchMedia` listeners keep it live; both are removed on unmount.
 */
import { useEffect, useState } from "react";

export type ShellLayoutTier = "wide" | "standard" | "compact";

const WIDE_QUERY = "(min-width: 1280px)";
const STANDARD_QUERY = "(min-width: 1024px)";

function deriveTier(
  isWide: boolean,
  isStandardOrWide: boolean,
): ShellLayoutTier {
  if (isWide) return "wide";
  if (isStandardOrWide) return "standard";
  return "compact";
}

function readTier(): ShellLayoutTier {
  if (
    typeof window === "undefined" ||
    typeof window.matchMedia !== "function"
  ) {
    // No matchMedia (e.g. most jsdom test environments): default to `wide`
    // so every existing test that never mounts a matchMedia mock keeps
    // exercising today's pinned-everything layout, unchanged.
    return "wide";
  }
  return deriveTier(
    window.matchMedia(WIDE_QUERY).matches,
    window.matchMedia(STANDARD_QUERY).matches,
  );
}

export function useShellLayout(): ShellLayoutTier {
  const [tier, setTier] = useState<ShellLayoutTier>(readTier);

  useEffect(() => {
    if (typeof window.matchMedia !== "function") return;
    const wideQuery = window.matchMedia(WIDE_QUERY);
    const standardQuery = window.matchMedia(STANDARD_QUERY);
    const apply = () =>
      setTier(deriveTier(wideQuery.matches, standardQuery.matches));
    apply();
    wideQuery.addEventListener?.("change", apply);
    standardQuery.addEventListener?.("change", apply);
    return () => {
      wideQuery.removeEventListener?.("change", apply);
      standardQuery.removeEventListener?.("change", apply);
    };
  }, []);

  return tier;
}
