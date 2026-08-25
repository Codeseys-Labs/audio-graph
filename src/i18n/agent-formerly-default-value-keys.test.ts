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

/**
 * New in W9 (the Signal/All queue-quality toggle). Review finding: three of
 * these four are INCIDENTALLY covered elsewhere (`filterSignal`/`filterAll`
 * by `AgentProposalsPanel.test.tsx`'s `getByRole("tab", { name: ... })`
 * accessible-name assertions, `lowSignal` by its `getByText("Low signal")`
 * assertion plus the chip-length-budget file) — but `agent.filterLabel`, the
 * tablist's bare `t("agent.filterLabel")` `aria-label` with no
 * `defaultValue`, was asserted NOWHERE: deleting it from both catalogs would
 * silently ship the raw key string as the control's accessible name with
 * the full gate suite green. `agent.duplicateCount` is listed here too
 * (interpolated, so it needs its own `count` argument) rather than relying
 * solely on the chip-length-budget file's coverage, per this file's own
 * discipline of covering EVERY key a ticket introduces, not just the
 * ones another file happens to also touch. */
const NEW_W9_KEYS = [
  "agent.filterLabel",
  "agent.filterSignal",
  "agent.filterAll",
  "agent.lowSignal",
] as const;

/**
 * New in audio-graph-83cc T4 (composer + threaded `AnswerThread` rendering).
 * Fix-round finding: the ticket's own report claimed these were "key-parity
 * enforced by locale-parity.test.ts", but that file (per this file's own
 * doc above) structurally cannot catch a key absent from BOTH locales — and
 * four of these nine keys (`answerFailedGeneric`, `answerTruncatedMarker`,
 * pt's `answerRetry`, pt's `answerTruncatedHint`) had no OTHER incidental
 * test coverage either. Listed here per this file's own stated discipline:
 * cover every key a ticket introduces, not just the ones another file
 * happens to also touch.
 */
const NEW_T4_KEYS = [
  "agent.answerFailedGeneric",
  "agent.answerInterrupted",
  "agent.answerRetry",
  "agent.answerTruncatedHint",
  "agent.answerTruncatedMarker",
] as const;

/** Interpolated, plural-form T4 keys — need a `count` arg to resolve
 * meaningfully, same reason `agent.duplicateCount` gets its own loop below
 * rather than folding into `NEW_T4_KEYS`. Asserted at both `count: 1` (the
 * `_one` form) and `count: 2` (the `_other` form) so a missing/deleted
 * plural variant in EITHER locale fails here, not just the base key. */
const NEW_T4_PLURAL_KEYS = [
  "agent.answerEvidenceSpans",
  "agent.answerEvidenceGraph",
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

describe("agent.* keys new in W9 (Signal/All queue-quality toggle)", () => {
  for (const key of NEW_W9_KEYS) {
    for (const lng of ["en", "pt"] as const) {
      it(`${lng}.json resolves "${key}" to a real translation, not the raw key`, () => {
        const rendered = instance.t(key, { lng });
        expect(typeof rendered).toBe("string");
        expect(rendered).not.toBe(key);
        expect(rendered.length).toBeGreaterThan(0);
      });
    }
  }

  // Interpolated separately (needs a `count` arg to resolve meaningfully) —
  // same "does it actually resolve" proof, not folded into the plain-key
  // loop above.
  for (const lng of ["en", "pt"] as const) {
    it(`${lng}.json resolves "agent.duplicateCount" to a real translation containing the count, not the raw key`, () => {
      const rendered = instance.t("agent.duplicateCount", { lng, count: 3 });
      expect(typeof rendered).toBe("string");
      expect(rendered).not.toBe("agent.duplicateCount");
      expect(rendered).toContain("3");
    });
  }
});

describe("agent.* keys new in audio-graph-83cc T4 (composer + AnswerThread)", () => {
  for (const key of NEW_T4_KEYS) {
    for (const lng of ["en", "pt"] as const) {
      it(`${lng}.json resolves "${key}" to a real translation, not the raw key (mutation: delete the key -> this fails)`, () => {
        const rendered = instance.t(key, { lng });
        expect(typeof rendered).toBe("string");
        expect(rendered).not.toBe(key);
        expect(rendered.length).toBeGreaterThan(0);
      });
    }
  }

  for (const key of NEW_T4_PLURAL_KEYS) {
    for (const lng of ["en", "pt"] as const) {
      for (const count of [1, 2]) {
        it(`${lng}.json resolves "${key}" to a real translation containing the count for count=${count}, not the raw key`, () => {
          const rendered = instance.t(key, { lng, count });
          expect(typeof rendered).toBe("string");
          expect(rendered).not.toBe(key);
          expect(rendered).toContain(String(count));
        });
      }
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
    for (const dotted of [
      ...FORMERLY_DEFAULT_VALUE_KEYS,
      ...NEW_W8_KEYS,
      ...NEW_W9_KEYS,
      ...NEW_T4_KEYS,
      "agent.duplicateCount",
    ]) {
      const leaf = dotted.split(".")[1];
      expect(agentEn, `en.json agent.${leaf}`).toHaveProperty(leaf);
      expect(agentPt, `pt.json agent.${leaf}`).toHaveProperty(leaf);
    }
    // Plural keys carry `_one`/`_other` suffixed leaves rather than a bare
    // leaf (i18next's pluralization convention) — checked separately so the
    // loop above stays a simple direct-property check for every other key.
    for (const dotted of NEW_T4_PLURAL_KEYS) {
      const base = dotted.split(".")[1];
      for (const suffix of ["_one", "_other"]) {
        expect(agentEn, `en.json agent.${base}${suffix}`).toHaveProperty(
          `${base}${suffix}`,
        );
        expect(agentPt, `pt.json agent.${base}${suffix}`).toHaveProperty(
          `${base}${suffix}`,
        );
      }
    }
  });
});
