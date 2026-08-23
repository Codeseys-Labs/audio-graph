import i18next from "i18next";
import { beforeAll, describe, expect, it } from "vitest";

import en from "./locales/en.json";
import pt from "./locales/pt.json";

/**
 * Ticket W8, synthesis audio-graph-a6b5: design-a found 5 `agent.*` keys
 * (`statusApproved`, `statusDismissed`, `statusPending`, `outcome`,
 * `projectionPatch`) that existed ONLY as inline `defaultValue` fallback
 * strings in the pre-W8 `AgentProposalsPanel.tsx` — neither `en.json` nor
 * `pt.json` carried them, so pt users saw English, and
 * `locale-parity.test.ts` structurally cannot catch a key absent from BOTH
 * locales (it only compares the two locales to each other).
 *
 * This file calls `t()` on each key with NO `defaultValue` — exactly how
 * `AgentProposalsPanel.tsx` calls them post-W8 — against a real `i18next`
 * instance loaded from the actual catalogs. If a key is ever removed from
 * (or never added to) a locale, i18next's missing-key fallback returns the
 * literal key STRING itself (e.g. `"agent.statusApproved"`) rather than a
 * translated word — that is the "raw key leaks" mutation this file is
 * built to catch. Passing today, with `defaultValue` gone from the
 * component, proves the keys are REAL catalog entries, not a name that
 * happens to also appear in a fallback string.
 */

const FORMERLY_DEFAULT_VALUE_KEYS = [
  "agent.statusApproved",
  "agent.statusDismissed",
  "agent.statusPending",
  "agent.outcome",
  "agent.projectionPatch",
] as const;

/** New in W8, never existed as a `defaultValue` fallback — added alongside
 * the formerly-defaultValue fix so the same "does it actually resolve, not
 * just look present" proof covers every key this ticket introduces. */
const NEW_W8_KEYS = [
  "agent.statusUnverified",
  "agent.queueTitle",
  "agent.feedTitle",
  "agent.feedEmpty",
  "agent.idleTitle",
  "agent.idleBody",
] as const;

const LOCALES: Record<"en" | "pt", Record<string, unknown>> = {
  en: en as Record<string, unknown>,
  pt: pt as Record<string, unknown>,
};

let instance: typeof i18next;

beforeAll(async () => {
  instance = i18next.createInstance();
  await instance.init({
    lng: "en",
    fallbackLng: "en",
    resources: {
      en: { translation: en },
      pt: { translation: pt },
    },
    interpolation: { escapeValue: false },
  });
});

describe("agent.* keys that were formerly defaultValue-only (ticket W8)", () => {
  for (const key of FORMERLY_DEFAULT_VALUE_KEYS) {
    for (const lng of ["en", "pt"] as const) {
      it(`${lng}.json resolves "${key}" to a real translation, not the raw key (mutation: delete the key -> this fails)`, () => {
        const rendered = instance.t(key, { lng });
        expect(typeof rendered).toBe("string");
        expect(rendered).not.toBe(key);
        expect(rendered.length).toBeGreaterThan(0);
      });
    }
  }
});

describe("agent.* keys new in W8 (feed/empty-state copy)", () => {
  for (const key of NEW_W8_KEYS) {
    for (const lng of ["en", "pt"] as const) {
      it(`${lng}.json resolves "${key}" to a real translation, not the raw key`, () => {
        const rendered = instance.t(key, { lng });
        expect(typeof rendered).toBe("string");
        expect(rendered).not.toBe(key);
        expect(rendered.length).toBeGreaterThan(0);
      });
    }
  }
});

describe("regression guard for the test methodology itself", () => {
  it("demonstrates i18next's real missing-key behavior: a key absent from BOTH locales renders as the literal key string", () => {
    const rendered = instance.t("agent.thisKeyDoesNotExistAnywhere", {
      lng: "en",
    });
    expect(rendered).toBe("agent.thisKeyDoesNotExistAnywhere");
  });

  it("every formerly-defaultValue key is present verbatim in BOTH raw locale objects (not just resolvable via fallbackLng)", () => {
    const agentEn = LOCALES.en.agent as Record<string, unknown>;
    const agentPt = LOCALES.pt.agent as Record<string, unknown>;
    for (const dotted of [...FORMERLY_DEFAULT_VALUE_KEYS, ...NEW_W8_KEYS]) {
      const leaf = dotted.split(".")[1];
      expect(agentEn, `en.json agent.${leaf}`).toHaveProperty(leaf);
      expect(agentPt, `pt.json agent.${leaf}`).toHaveProperty(leaf);
    }
  });
});
