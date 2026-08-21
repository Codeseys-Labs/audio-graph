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

/* --------------------------------------------------------------------------
   Alias-resolving token reader (UI-T3/audio-graph-99aa)
   --------------------------------------------------------------------------
   --accent-blue/--accent-blue-hover/--on-accent-blue are no longer literal
   hex declarations — they alias --accent/--accent-hover/--on-accent
   (ratified decision D3: the two hues sat ~11-12deg apart, so 163 call
   sites split across two names now render one color under both). A plain
   hex-literal regex (tokenValues above) would see zero declarations for
   these three names. rawTokenValues captures the raw declaration text
   (hex OR `var(--other)`) per occurrence; resolvedTokenValues follows a
   single level of var() aliasing back to the referenced token's value at
   the SAME positional index (declaration order === theme order: [0] dark
   :root, [1] the light media query, [2] the explicit data-theme override —
   same convention the --edge suite below already relies on), so the
   contrast math below is computed from styles.css's literal values either
   way, never hardcoded. */
function rawTokenValues(name: string): string[] {
  return Array.from(
    css.matchAll(new RegExp(`--${name}:\\s*([^;]+);`, "g")),
    (match) => match[1].trim(),
  );
}

function resolvedTokenValues(
  name: string,
  seen: Set<string> = new Set(),
): string[] {
  if (seen.has(name)) {
    throw new Error(`circular --${name} alias chain in styles.css`);
  }
  seen.add(name);
  return rawTokenValues(name).map((raw, index) => {
    if (/^#[0-9a-fA-F]{6}$/.test(raw)) {
      return raw;
    }
    const aliasMatch = raw.match(/^var\(--([\w-]+)\)$/);
    if (aliasMatch) {
      // A fresh copy per recursive call, not the same mutable Set: each
      // declaration-index resolution is independent (different theme
      // occurrence), so resolving --accent at index 1 must not "remember"
      // that index 0 already visited --accent — that isn't a cycle, it's
      // the same alias resolved once per theme.
      const resolved = resolvedTokenValues(aliasMatch[1], new Set(seen))[index];
      if (!resolved) {
        throw new Error(
          `--${name} declaration #${index} aliases --${aliasMatch[1]}, which has no declaration at the same theme position`,
        );
      }
      return resolved;
    }
    throw new Error(
      `--${name} declaration #${index} ("${raw}") is neither a hex color nor a single-level var() alias`,
    );
  });
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
  // Extended from the single --accent-blue pair (UI-T2) to all 7
  // accent/on-accent pairs, both themes (UI-T3/audio-graph-99aa) — the
  // 4.5:1 floor is the exact one the original --accent-blue-only test
  // enforced; every pair must clear it, never a weaker one. --accent-blue
  // is now an alias of --accent (see the styles.css comment), so its pair
  // here is expected to mirror --accent's numbers exactly — that IS the
  // point of the alias, not a test bug.
  const ACCENT_NAMES = [
    "accent",
    "accent-red",
    "accent-green",
    "accent-gemini",
    "accent-blue",
    "accent-yellow",
    "accent-purple",
  ];

  for (const name of ACCENT_NAMES) {
    it(`keeps every --${name}/--on-${name} declaration pair at AA contrast (>=4.5:1)`, () => {
      const backgrounds = resolvedTokenValues(name);
      const foregrounds = resolvedTokenValues(`on-${name}`);

      expect(backgrounds.length, name).toBeGreaterThanOrEqual(2);
      expect(foregrounds, name).toHaveLength(backgrounds.length);
      for (const [index, background] of backgrounds.entries()) {
        const ratio = contrast(background, foregrounds[index]);
        expect(
          ratio,
          `--${name} decl #${index} (${background}) vs --on-${name} (${foregrounds[index]}) = ${ratio.toFixed(3)}:1`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    });
  }
});

/* --------------------------------------------------------------------------
   Accent-as-text on realistic surfaces (gate-fix review blocker fix,
   UI-T3/audio-graph-99aa)
   --------------------------------------------------------------------------
   The suite above only checks the fill/on-fill pairing. Accent tokens are
   ALSO rendered directly as TEXT color on ordinary surfaces and tints
   (ConversationModeControl's badges, TokenUsagePanel's gemini total,
   ProjectionRuntimeStatusPanel/SessionDataRoutePanel/AgentProposalsPanel's
   green status text) — a usage this file did not check before the OKLCH
   re-derivation shipped 6+ live sub-4.5:1 pairs in the light theme with the
   fill/on-fill test staying green throughout. This locks the specific real
   call-site pairings (theme, accent name, surface/tint name) so a future
   token edit that only re-verifies fills can't silently regress text
   usage again. Light theme only: dark-theme fills sit at 7.0-7.5:1 with
   enough headroom that no dark surface/tint combination in this palette
   can push a dark accent below 4.5:1 as text. */
describe("accent-as-text on realistic surfaces (gate-fix review blocker fix)", () => {
  // Reads the CURRENT light-theme hex directly (tokenValues' hex-only
  // regex), not resolvedTokenValues: --tint-* tokens declare an rgba() in
  // the dark :root block, which resolvedTokenValues would try to parse as
  // an alias and throw on. Every name used here (accent/accent-green/
  // accent-gemini/bg-tertiary/bg-secondary/tint-accent/tint-success/
  // tint-gemini) is a plain hex literal in BOTH light entry points, and
  // the identity test below independently pins those two entries equal —
  // so the last hex match is always "the current light value" regardless
  // of whether the dark declaration above it was hex (2 light matches at
  // indices [1],[2]) or non-hex (2 light matches at indices [0],[1]).
  function lightHex(name: string): string {
    const values = tokenValues(name);
    expect(values.length, name).toBeGreaterThanOrEqual(2);
    return values[values.length - 1];
  }

  const LIGHT_PAIRS: Array<[accent: string, surface: string]> = [
    ["accent", "tint-accent"], // ConversationModeControl ENGINE_ACTIVE
    ["accent", "bg-tertiary"], // ConversationModeControl BADGE_ACTION
    ["accent-green", "bg-tertiary"], // ProjectionRuntimeStatusPanel success text
    ["accent-green", "bg-secondary"], // AgentProposalsPanel diff text
    ["accent-green", "tint-success"], // SessionDataRoutePanel success banner
    ["accent-gemini", "bg-tertiary"], // TokenUsagePanel ddTotal
    ["accent-gemini", "tint-gemini"], // NowStrip Gemini-active hover (was ControlBar)
  ];

  for (const [accentName, surfaceName] of LIGHT_PAIRS) {
    it(`keeps light --${accentName} at AA contrast (>=4.5:1) as text on --${surfaceName}`, () => {
      const accentHex = lightHex(accentName);
      const surfaceHex = lightHex(surfaceName);
      const ratio = contrast(accentHex, surfaceHex);
      expect(
        ratio,
        `--${accentName} (${accentHex}) vs --${surfaceName} (${surfaceHex}) = ${ratio.toFixed(3)}:1`,
      ).toBeGreaterThanOrEqual(4.5);
    });
  }
});

/* --------------------------------------------------------------------------
   Accent alias identity (gate-fix review, UI-T3/audio-graph-99aa)
   --------------------------------------------------------------------------
   Two gaps a review probe found: (1) nothing locked --accent-blue's alias
   FORM, so a future edit could reintroduce an independent (but
   numerically-AA-passing) literal hex and the contrast test above would
   stay green — defeating the D3 "one color under two names" invariant
   silently; (2) the accent/on-accent/hover block is hand-duplicated between
   the `@media (prefers-color-scheme: light)` default and the explicit
   `[data-theme="light"]` override (same structural risk the --edge suite
   below already guards for bg/edge/border tokens), but the existing
   "identical between the two light entry points" test only covers
   ALL_TOKEN_NAMES, which excludes every accent name. A probe that replaced
   `--accent-blue: var(--accent);` with an AA-passing literal in ONLY the
   `[data-theme="light"]` block left the full suite green before this fix. */
describe("accent alias identity (gate-fix review, UI-T3/audio-graph-99aa)", () => {
  it.each([
    ["accent-blue", "accent"],
    ["accent-blue-hover", "accent-hover"],
    ["on-accent-blue", "on-accent"],
  ])("keeps --%s a var(--%s) alias — raw declaration text, not just the resolved value — in every theme block", (aliasName, targetName) => {
    const raws = rawTokenValues(aliasName);
    expect(raws.length, aliasName).toBeGreaterThanOrEqual(3);
    for (const [index, raw] of raws.entries()) {
      expect(raw, `--${aliasName} decl #${index}`).toBe(`var(--${targetName})`);
    }
  });

  // Extends the --edge suite's "identical between the two light entry
  // points" guard to every accent/on-accent/hover name, not just
  // ALL_TOKEN_NAMES (bg/edge/border). Works whether a declaration is a hex
  // literal or a var() alias, since it compares raw declaration text.
  const ACCENT_AND_FRIENDS = [
    "accent",
    "accent-hover",
    "on-accent",
    "accent-red",
    "accent-red-hover",
    "on-accent-red",
    "accent-green",
    "accent-green-hover",
    "on-accent-green",
    "accent-gemini",
    "accent-gemini-hover",
    "on-accent-gemini",
    "accent-blue",
    "accent-blue-hover",
    "on-accent-blue",
    "accent-yellow",
    "on-accent-yellow",
    "accent-purple",
    "accent-purple-hover",
    "on-accent-purple",
  ];

  it("keeps every accent/on-accent/hover declaration identical between the prefers-color-scheme default and the explicit data-theme override", () => {
    for (const name of ACCENT_AND_FRIENDS) {
      const raws = rawTokenValues(name);
      expect(raws.length, name).toBeGreaterThanOrEqual(3);
      const [, mediaQueryValue, dataThemeValue] = raws;
      expect(dataThemeValue, name).toBe(mediaQueryValue);
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
