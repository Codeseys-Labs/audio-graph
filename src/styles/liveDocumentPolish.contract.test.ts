// CSS-source-text contract test for the living-document L1/L2 polish layer
// (ticket W10, synthesis audio-graph-a6b5). Same rationale as
// `layout.bento.contract.test.ts`: vitest's jsdom environment applies no CSS
// (`css: false` in vitest.config.ts) — `prefers-reduced-motion` can't be
// asserted via computed style in a unit test, so this file pins it the same
// way that file does, by reading the CSS SOURCE text and regex-matching the
// exact rules.
//
// This is the durable, mutation-provable half of the reduced-motion
// requirement. The manually-run production `dist/` bundle grep for
// `doc-refined-pulse`/`.ag-doc-refined`/`.ag-doc-anchor` is the second,
// real-build half neither vitest nor this file can substitute for (same
// "do not read a passing manual grep as this file's job" caveat that file's
// own doc states) — reported separately in the implementer's landing report.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const layoutCss = readFileSync("src/styles/layout.css", "utf8") as string;
const keyframesCss = readFileSync("src/styles/keyframes.css", "utf8") as string;

/** Mirrors `layout.bento.contract.test.ts`'s own `ruleBody` helper: the named
 * rule's own declaration body only, never spilling into whatever comes
 * after its closing brace. */
function ruleBody(css: string, selector: string): string {
  const start = css.indexOf(`${selector} {`);
  expect(
    start,
    `expected to find "${selector} {" in the given CSS source`,
  ).toBeGreaterThanOrEqual(0);
  const end = css.indexOf("}", start);
  return css.slice(start, end);
}

/** The FULL SPAN (condition through matching closing brace, via brace
 * counting) of a specific `@media` block, so an assertion against ITS body
 * can't accidentally match a same-named rule declared OUTSIDE that block —
 * same technique `layout.bento.contract.test.ts`'s
 * `canvasRuleBodyWithin`/`withoutMediaBlocks` use for the canvas row-swap
 * override, applied here to the single shared
 * `@media (prefers-reduced-motion: reduce)` block this file's two rules
 * join. */
function reducedMotionBlock(css: string): string {
  const mediaStart = css.indexOf("@media (prefers-reduced-motion: reduce)");
  expect(
    mediaStart,
    "expected to find the shared reduced-motion media block in layout.css",
  ).toBeGreaterThanOrEqual(0);
  const openBrace = css.indexOf("{", mediaStart);
  expect(openBrace, "expected an opening brace after @media").toBeGreaterThan(
    mediaStart,
  );
  let depth = 1;
  let j = openBrace + 1;
  while (depth > 0 && j < css.length) {
    if (css[j] === "{") depth++;
    else if (css[j] === "}") depth--;
    j++;
  }
  expect(
    depth,
    "unbalanced braces while scanning the reduced-motion block",
  ).toBe(0);
  return css.slice(mediaStart, j);
}

describe("refinement pulse — CSS source contract (ticket W10)", () => {
  it("defines the doc-refined-pulse keyframe as a tint fade (background-color only — never a transform/position change, so the pulse itself can never cause a layout shift)", () => {
    const start = keyframesCss.indexOf("@keyframes doc-refined-pulse");
    expect(
      start,
      "expected @keyframes doc-refined-pulse in keyframes.css",
    ).toBeGreaterThanOrEqual(0);
    const end = keyframesCss.indexOf("}", keyframesCss.indexOf("100%", start));
    const body = keyframesCss.slice(start, end);
    expect(body).toContain("background-color");
    expect(body).not.toMatch(/transform|top|left|margin|width|height/);
  });

  it(".ag-doc-refined plays the pulse once, 1.5s ease-out (design-a §1.4's own rate-limit window)", () => {
    const body = ruleBody(layoutCss, ".ag-doc-refined");
    expect(body).toMatch(/animation:\s*doc-refined-pulse\s+1\.5s\s+ease-out/);
  });

  it("disables the pulse animation entirely under prefers-reduced-motion (ticket point 4 — no motion, not a slowed-down motion)", () => {
    const block = reducedMotionBlock(layoutCss);
    expect(block).toMatch(/\.ag-doc-refined\s*\{\s*animation:\s*none;?\s*\}/);
  });
});

describe("change anchor — CSS source contract (ticket W10)", () => {
  it(".ag-doc-anchor is sticky-positioned (glued to the visible scroll edge, not the document's own top/bottom)", () => {
    const body = ruleBody(layoutCss, ".ag-doc-anchor");
    expect(body).toContain("position: sticky");
  });

  it(".ag-doc-anchor transitions opacity — the dissolve `LiveDocument.tsx`'s opacity/pointer-events toggle animates against", () => {
    const body = ruleBody(layoutCss, ".ag-doc-anchor");
    expect(body).toMatch(/transition:\s*opacity/);
  });

  it("sticks the ABOVE anchor to the top edge and the BELOW anchor to the bottom edge", () => {
    const aboveBody = ruleBody(
      layoutCss,
      '.ag-doc-anchor[data-direction="above"]',
    );
    expect(aboveBody).toContain("top:");
    const belowBody = ruleBody(
      layoutCss,
      '.ag-doc-anchor[data-direction="below"]',
    );
    expect(belowBody).toContain("bottom:");
  });

  it("disables the dissolve transition entirely under prefers-reduced-motion — an instant show/hide, not a slowed-down fade", () => {
    const block = reducedMotionBlock(layoutCss);
    expect(block).toMatch(/\.ag-doc-anchor\s*\{\s*transition:\s*none;?\s*\}/);
  });

  it("collapses to zero layout space (max-height/padding/margin) while aria-hidden, so an at-rest anchor never permanently reserves a chip's worth of space in the reading surface", () => {
    const body = ruleBody(layoutCss, '.ag-doc-anchor[aria-hidden="true"]');
    expect(body).toMatch(/max-height:\s*0/);
    expect(body).toMatch(/padding-block:\s*0/);
    expect(body).toMatch(/margin-block:\s*0/);
  });
});
