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

// `document.recency.asOf`/`.current`/`.behind` are the ONLY visible-label
// keys for BOTH the document AND graph recency chips — `GraphRecencyChip`
// (LiveGraphStrip.tsx) reuses these three directly rather than keeping a
// byte-identical `graphStrip.recency.asOf`/`.current`/`.behind` set
// (design-a §7's reuse mandate; see that component's module doc). Only the
// `*Aria` strings differ per lane, and those aren't width-constrained
// (`.ag-chip`'s `white-space: nowrap` never applies to `.sr-only` text), so
// they're out of this file's scope. `document.recency.current` ("Up to
// date"/"Atualizado") takes no interpolation values — included here anyway
// (not just via the raw-literal settings-budget file) so a future edit that
// adds interpolation to it is re-checked the same way the other two keys
// are.
const CHIP_CASES: readonly ChipCase[] = [
  { key: "document.recency.asOf", values: { time: TIME_WORST_CASE } },
  { key: "document.recency.current", values: {} },
  { key: "document.recency.behind", values: { count: TURNS_WORST_CASE } },
  // Ticket W8: the agent tile's status chip (`AgentStatusChip`,
  // `AgentProposalsPanel.tsx`), routed through `agentOutcomeChipTone`
  // (`liveWorkspaceTone.ts`). None of these four interpolate, but they
  // still belong in this file rather than only in `locale-parity.test.ts` —
  // this is the file that actually measures the RENDERED chip width.
  { key: "agent.statusApproved", values: {} },
  { key: "agent.statusDismissed", values: {} },
  { key: "agent.statusPending", values: {} },
  { key: "agent.statusUnverified", values: {} },
  // Ticket W9: the fragment-suspect marker chip
  // (`FragmentSuspectMarker`, `AgentProposalsPanel.tsx`) — an
  // `.ag-chip[data-tone="neutral"]` rendered on both queue (All mode) and
  // feed (Signal mode) rows, so it belongs in this file for the same reason
  // the W8 status chips do.
  { key: "agent.lowSignal", values: {} },
  // Ticket W9: the duplicate-count marker chip (`DuplicateCountBadge`,
  // `AgentProposalsPanel.tsx`) — interpolates `{{count}}`. Worst case is the
  // store's own `agentProposals` cap (`store/index.ts`'s `.slice(-49)`, 50
  // max entries), so a single duplicate-collapse group can never exceed 50.
  { key: "agent.duplicateCount", values: { count: 50 } },
  // audio-graph-83cc T4: the answered-card evidence chips
  // (`AnswerEvidenceChips`, `AgentProposalsPanel.tsx`) and the interrupted
  // marker (`AnswerThread`). `TURNS_WORST_CASE` is reused as a generously
  // oversized bound for the evidence counts too — the design panel
  // synthesis's actual retrieval caps are far smaller (an anchored window of
  // ±6/±2 spans, a query-conditioned top-40 graph context), but nothing in
  // this unit enforces that bound at the type level, so the test should not
  // assume it either.
  { key: "agent.answerEvidenceSpans", values: { count: TURNS_WORST_CASE } },
  { key: "agent.answerEvidenceGraph", values: { count: TURNS_WORST_CASE } },
  { key: "agent.answerInterrupted", values: {} },
  // audio-graph-83cc T5: the auto-answer session budget chip
  // (`AutoAnswerCountChip`, `AgentComposer.tsx`) — interpolates BOTH
  // `{{count}}` and `{{cap}}`. `max_per_session` has no type-level upper
  // bound (a `u32` on the Rust side, `settings/mod.rs`), so this reuses
  // `TURNS_WORST_CASE` as the same generously oversized bound the evidence
  // chips above use, for both interpolation slots at once (the pathological
  // case where a session somehow both dispatches and is capped at a huge
  // number).
  {
    key: "agent.autoAnswerCount",
    values: { count: TURNS_WORST_CASE, cap: TURNS_WORST_CASE },
  },
  // Ticket W10: the out-of-viewport change anchor (`DocChangeAnchor`,
  // `LiveDocument.tsx`) — styled as a small pill in `layout.css`
  // (`.ag-doc-anchor`), so it belongs in this file even though it's a
  // `<button>`, not an `.ag-chip[data-tone]`. Worst case is bounded by the
  // outline's own node count, not an unbounded counter — `TURNS_WORST_CASE`
  // is reused here too rather than inventing a second "big number" constant
  // for the same purpose.
  { key: "document.changeAnchor.above", values: { count: TURNS_WORST_CASE } },
  { key: "document.changeAnchor.below", values: { count: TURNS_WORST_CASE } },
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
