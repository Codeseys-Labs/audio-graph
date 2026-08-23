import i18next from "i18next";
import { beforeAll, describe, expect, it } from "vitest";

import en from "./locales/en.json";
import pt from "./locales/pt.json";

/**
 * The interpolating sibling of `settings-chip-length-budget.test.ts` (ticket
 * W6, synthesis audio-graph-a6b5 — design-a §7's "two test obligations").
 *
 * `settings-chip-length-budget.test.ts` measures a key's RAW literal
 * `.length` — a latent gap design-a §7/point 2 names explicitly: a template
 * like `"−{{count}} turnos"` is 17 chars as a literal (passes the 18-char
 * budget) but its RENDERED value at a realistic worst-case `count` can be
 * longer, and the raw check would never catch that. This file interpolates
 * every chip key through a real `i18next` instance BEFORE measuring, so a
 * template that looks safe unrendered but overflows once interpolated fails
 * here instead of shipping into a `white-space: nowrap` `.ag-chip`
 * (`styles.css`) where it would clip or wrap the tile header.
 *
 * A sibling file, not an extension of the settings one, for the same reason
 * that file's own module doc gives (`settings-chip-length-budget.test.ts:5-18`):
 * that file's name and doc are settings-scoped.
 */

const CHIP_LENGTH_BUDGET = 18;

/** Worst-case interpolation values, chosen per-key below. `TURNS_WORST_CASE`
 * intentionally exceeds the top-level task's own "-12 turns" example — a
 * lane nobody applies a patch to for a long stretch can accumulate turns
 * without bound, so the budget must hold at 3 digits, not just 2. */
const TURNS_WORST_CASE = 999;
/** `Date#toLocaleTimeString()` called with NO arguments resolves to the
 * OS/runtime locale, not the app's i18n language — on an en-US host that
 * renders AM/PM ("11:59:59 PM"), 3 chars wider than this value. Both chip
 * call sites (`DocRecencyChip`/`GraphRecencyChip`) pass an explicit locale
 * plus `{ hour12: false }`, which is what makes "23:59:59" (fixed-width
 * digits/colons, no AM/PM) the actual rendered worst case rather than just
 * an assumption about default formatting — see those components' own
 * comments on `toLocaleTimeString`. */
const TIME_WORST_CASE = "23:59:59";

interface ChipCase {
  key: string;
  values: Record<string, unknown>;
}

// `document.recency.asOf`/`document.recency.behind` are the ONLY visible-label
// keys for BOTH the document AND graph recency chips — `GraphRecencyChip`
// (LiveGraphStrip.tsx) reuses these two directly rather than keeping a
// byte-identical `graphStrip.recency.asOf`/`behind` pair (design-a §7's reuse
// mandate; see that component's module doc). Only the `*Aria` strings differ
// per lane, and those aren't width-constrained (`.ag-chip`'s `white-space:
// nowrap` never applies to `.sr-only` text), so they're out of this file's
// scope.
const CHIP_CASES: readonly ChipCase[] = [
  { key: "document.recency.asOf", values: { time: TIME_WORST_CASE } },
  { key: "document.recency.behind", values: { count: TURNS_WORST_CASE } },
] as const;

const LOCALES: Record<string, Record<string, unknown>> = {
  en: en as Record<string, unknown>,
  pt: pt as Record<string, unknown>,
};

let instance: typeof i18next;

beforeAll(async () => {
  instance = i18next.createInstance();
  await instance.init({
    lng: "pt",
    fallbackLng: "pt",
    resources: {
      en: { translation: en },
      pt: { translation: pt },
    },
    interpolation: { escapeValue: false },
  });
});

describe("workspace recency chip pt length budget — INTERPOLATED, not raw literal", () => {
  for (const { key, values } of CHIP_CASES) {
    it(`pt.json "${key}" fits the ${CHIP_LENGTH_BUDGET}-char chip budget AFTER interpolating ${JSON.stringify(values)}`, () => {
      const rendered = instance.t(key, { lng: "pt", ...values });
      expect(typeof rendered).toBe("string");
      expect(rendered.length).toBeLessThanOrEqual(CHIP_LENGTH_BUDGET);
    });
  }
});

describe("workspace recency chip pt length budget — the raw-literal gap this file exists to close", () => {
  it("demonstrates the raw literal can look safe while under-measuring the interpolated worst case (regression guard for the test methodology itself)", () => {
    const key = "document.recency.behind";
    const raw = LOCALES.pt.document as Record<string, unknown> as {
      recency: { behind: string };
    };
    const rawLiteral = raw.recency.behind;
    const rendered = instance.t(key, { lng: "pt", count: TURNS_WORST_CASE });
    // The raw literal contains the placeholder text, not the rendered
    // digits — asserting they can differ in length is the whole point: a
    // raw-only check is measuring the wrong string.
    expect(rawLiteral).not.toBe(rendered);
    expect(rendered.length).toBeLessThanOrEqual(CHIP_LENGTH_BUDGET);
  });
});
