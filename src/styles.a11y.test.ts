// Node types are ambient here only because the E2E devDependency chain
// (seed audio-graph-f9e0) transitively pulls in @types/node — the browser
// bundle itself still never imports "node:fs" outside test-only code.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync("src/styles.css", "utf8") as string;

function tokenValues(name: string): string[] {
  return Array.from(
    css.matchAll(new RegExp(`--${name}:\\s*(#[0-9a-fA-F]{6})`, "g")),
    (match) => match[1],
  );
}

function luminance(hex: string): number {
  const channels = [1, 3, 5].map(
    (offset) => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255,
  );
  const [r, g, b] = channels.map((channel) =>
    channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4,
  );
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
  const [lighter, darker] = [luminance(a), luminance(b)].sort(
    (left, right) => right - left,
  );
  return (lighter + 0.05) / (darker + 0.05);
}

describe("semantic accent foregrounds", () => {
  it("keeps every --accent-blue/--on-accent-blue declaration pair at AA contrast", () => {
    const backgrounds = tokenValues("accent-blue");
    const foregrounds = tokenValues("on-accent-blue");

    expect(backgrounds.length).toBeGreaterThanOrEqual(2);
    expect(foregrounds).toHaveLength(backgrounds.length);
    for (const [index, background] of backgrounds.entries()) {
      expect(contrast(background, foregrounds[index])).toBeGreaterThanOrEqual(
        4.5,
      );
    }
  });
});

/* --------------------------------------------------------------------------
   Non-text contrast — --edge (WCAG 1.4.11, ADR-0047)
   --------------------------------------------------------------------------
   --edge is the component-boundary border token. It must clear the 3:1
   floor for "visual information required to identify UI components" against
   EVERY surface it can be drawn on top of — --bg-primary/-secondary/
   -tertiary/-elevated — in both themes. Every ratio below is computed from
   the literal hex values declared in styles.css, not hardcoded, so a future
   edit to any of these five tokens re-runs the real math instead of a stale
   snapshot.

   `--edge-subtle` is intentionally NOT held to the same floor — it marks
   decorative-only rules (WCAG 1.4.11 exempts those), so this file makes no
   contrast assertion about it beyond confirming it's declared.
   -------------------------------------------------------------------------- */
describe("non-text contrast — --edge against all four surfaces (WCAG 1.4.11)", () => {
  const SURFACE_NAMES = [
    "bg-primary",
    "bg-secondary",
    "bg-tertiary",
    "bg-elevated",
  ];

  // Every theme-scoped token this suite cares about, including the legacy
  // --border-color (still declared per-theme even though it's out of the
  // --edge contract) so an edit to any one theme block that forgets its
  // sibling block is caught structurally, not just by ratio math.
  const ALL_TOKEN_NAMES = [
    ...SURFACE_NAMES,
    "edge",
    "edge-subtle",
    "border-color",
  ];

  // Declaration order in styles.css: [0] :root (dark default), [1] the
  // `@media (prefers-color-scheme: light)` override, [2] the explicit
  // `[data-theme="light"]` override. Pinned (not just counted) by the
  // "pins the declaration-order comment to fact" test below, which checks
  // each declaration's source position against the block boundaries rather
  // than trusting array index alone.
  const THEME_INDEX = { dark: 0, light: 1 } as const;

  function surfacesFor(theme: keyof typeof THEME_INDEX): string[] {
    return SURFACE_NAMES.map((name) => {
      const values = tokenValues(name);
      expect(values.length).toBeGreaterThanOrEqual(3);
      return values[THEME_INDEX[theme]];
    });
  }

  it("declares --edge and --edge-subtle for both the dark default and every light entry point", () => {
    expect(tokenValues("edge")).toHaveLength(3);
    expect(tokenValues("edge-subtle")).toHaveLength(3);
  });

  it("pins the declaration-order comment above to fact: index 0 precedes the light media query, index 1 sits inside it, index 2 sits inside the data-theme override", () => {
    const lightMediaStart = css.indexOf("@media (prefers-color-scheme: light)");
    const dataThemeLightStart = css.search(/^\[data-theme="light"\] \{/m);
    expect(lightMediaStart).toBeGreaterThan(0);
    expect(dataThemeLightStart).toBeGreaterThan(lightMediaStart);

    for (const name of ALL_TOKEN_NAMES) {
      const positions = Array.from(
        css.matchAll(new RegExp(`--${name}:\\s*#[0-9a-fA-F]{6}`, "g")),
        (match) => match.index ?? -1,
      );
      expect(positions.length, name).toBeGreaterThanOrEqual(3);
      expect(positions[THEME_INDEX.dark], name).toBeLessThan(lightMediaStart);
      expect(positions[1], name).toBeGreaterThan(lightMediaStart);
      expect(positions[1], name).toBeLessThan(dataThemeLightStart);
      expect(positions[2], name).toBeGreaterThan(dataThemeLightStart);
    }
  });

  it("keeps every light-theme token identical between the prefers-color-scheme default and the explicit data-theme override (all seven theme-scoped tokens, not just edge/edge-subtle/bg-elevated)", () => {
    for (const name of ALL_TOKEN_NAMES) {
      const [, mediaQueryValue, dataThemeValue] = tokenValues(name);
      expect(dataThemeValue, name).toBe(mediaQueryValue);
    }
  });

  it.each([
    "dark",
    "light",
  ] as const)("keeps --bg-elevated visually distinct from --bg-primary in the %s theme (regression guard: light entry points computed 1.00:1 — indistinguishable — before ADR-0047)", (theme) => {
    const bgPrimary = tokenValues("bg-primary")[THEME_INDEX[theme]];
    const bgElevated = tokenValues("bg-elevated")[THEME_INDEX[theme]];
    const ratio = contrast(bgPrimary, bgElevated);
    expect(
      ratio,
      `--bg-elevated vs --bg-primary (${theme}) = ${ratio.toFixed(3)}:1`,
    ).toBeGreaterThanOrEqual(1.02);
  });

  it.each([
    "dark",
    "light",
  ] as const)("computes --edge at >=3:1 against every %s-theme surface", (theme) => {
    const edge = tokenValues("edge")[THEME_INDEX[theme]];
    for (const [index, surface] of surfacesFor(theme).entries()) {
      const ratio = contrast(edge, surface);
      expect(
        ratio,
        `--edge vs --${SURFACE_NAMES[index]} (${theme}) = ${ratio.toFixed(2)}:1`,
      ).toBeGreaterThanOrEqual(3);
    }
  });
});
