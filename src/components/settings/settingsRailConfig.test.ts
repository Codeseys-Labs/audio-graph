import { describe, expect, it } from "vitest";
import en from "../../i18n/locales/en.json";
import { PROVIDER_DESCRIPTORS } from "../providerRegistryHelpers";
import { RAIL_SECTIONS } from "./settingsRailConfig";

/**
 * Reserved rail-tab token guard (audio-graph-4850, T4a, synthesis §T4a copy
 * rule): "no cross-row token collisions with the 8 reserved test tokens" —
 * `modes`, `general`, `logging`, `language model`, `speech-to-text`,
 * `text-to-speech`, `realtime agent`, `credentials`. Every one of these is
 * matched with `getByRole("tab", {name: /token/i})` somewhere in
 * `SettingsPage.test.tsx` and MUST resolve to exactly one tab. The dual-label
 * rail (T4a) adds two new sources of copy per tab — the goal line and the
 * live provider's display name — either of which could silently introduce a
 * SECOND match for another tab's token. This test is the static half of the
 * guard: it exhaustively checks every registry provider's `display_name`
 * (the rail engine line's only per-tab-dynamic content) plus every tab's own
 * static copy, so a new provider or a new goal phrase can't reintroduce a
 * collision without failing here — independent of which specific provider a
 * fixture happens to have selected. The dynamic half (does the REAL rendered
 * rail, with several different active-provider fixtures, still resolve each
 * token to exactly one tab) lives in `SettingsPage.test.tsx`.
 */
const RESERVED_TAB_TOKENS = [
  "modes",
  "general",
  "logging",
  "language model",
  "speech-to-text",
  "text-to-speech",
  "realtime agent",
  "credentials",
] as const;

function readKeyPath(root: unknown, path: string): unknown {
  return path
    .split(".")
    .reduce<unknown>(
      (node, segment) =>
        typeof node === "object" && node !== null
          ? (node as Record<string, unknown>)[segment]
          : undefined,
      root,
    );
}

/** Every reserved token that appears (case-insensitively) in `text`. */
function tokensIn(text: string): string[] {
  const lower = text.toLowerCase();
  return RESERVED_TAB_TOKENS.filter((token) => lower.includes(token));
}

describe("settings rail reserved-token guard (T4a)", () => {
  it("each tab's own labelKey contains exactly its own reserved token, never another tab's", () => {
    // The token a tab's OWN unchanged `labelKey` text is expected to carry —
    // this is the pre-existing mapping the 89 `goToTab` regexes rely on.
    const ownToken: Record<string, string> = {
      overview: "modes",
      general: "general",
      logging: "logging",
      llm: "language model",
      stt: "speech-to-text",
      tts: "text-to-speech",
      gemini: "realtime agent",
      credentials: "credentials",
    };
    for (const tab of RAIL_SECTIONS) {
      const label = readKeyPath(en, tab.labelKey);
      expect(typeof label).toBe("string");
      const found = tokensIn(label as string);
      expect(found).toEqual([ownToken[tab.id]]);
    }
  });

  it("no tab's new goal-line copy introduces any of the 8 reserved tokens", () => {
    for (const tab of RAIL_SECTIONS) {
      const goal = readKeyPath(en, tab.goalLabelKey);
      expect(typeof goal).toBe("string");
      expect(tokensIn(goal as string)).toEqual([]);
    }
  });

  it("no registry provider's display_name (the rail engine line's only per-tab-dynamic content) contains a reserved token", () => {
    const offenders: string[] = [];
    for (const descriptor of PROVIDER_DESCRIPTORS.values()) {
      const found = tokensIn(descriptor.display_name);
      if (found.length > 0) {
        offenders.push(
          `${descriptor.id} ("${descriptor.display_name}") -> ${found.join(", ")}`,
        );
      }
    }
    expect(offenders).toEqual([]);
  });

  it("no T2 readiness chip status label (the engine line's OTHER per-tab-dynamic content, alongside display_name) contains a reserved token", () => {
    // `settingsRail.tsx` also renders
    // `settings.providerReadiness.status.<effectiveStatus>` inside the same
    // tab button as the display_name checked above — a copy edit to any of
    // these 4 status labels (e.g. "Missing key" -> "Missing credentials")
    // could silently introduce a second match for the "credentials" tab's
    // token on every provider-bearing row, breaking all `goToTab` regexes
    // that rely on the token resolving to exactly one tab. This static half
    // is copy-only; the dynamic half (SettingsPage.test.tsx) never produces
    // every status on a provider-bearing tab, so this is the only place a
    // future status-label edit is guaranteed to be checked.
    const statusLabels = readKeyPath(
      en,
      "settings.providerReadiness.status",
    ) as Record<string, string>;
    expect(typeof statusLabels).toBe("object");
    const offenders: string[] = [];
    for (const [status, label] of Object.entries(statusLabels)) {
      const found = tokensIn(label);
      if (found.length > 0) {
        offenders.push(`status.${status} ("${label}") -> ${found.join(", ")}`);
      }
    }
    expect(offenders).toEqual([]);
  });
});
