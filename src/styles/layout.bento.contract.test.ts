// CSS-source-text contract test for the bento workspace grid (ticket W4,
// synthesis audio-graph-a6b5). vitest's jsdom environment applies no CSS
// (`css: false` in vitest.config.ts), so properties like `container-type`
// can't be asserted via computed style — this file pins them the same way
// `styles.a11y.test.ts` pins design-token declarations: read the CSS
// SOURCE text and regex-match the exact rules.
//
// The 0922 "deleted, not shadowed" claim (the old
// `.workspace-panel__primary`/`__transcript`/`__assist` properties never
// reappear) has TWO independent gates, not one: the source-text regex below
// (durable, runs on every `bun run test`) plus a production `dist/` bundle
// grep for the same three class names (run manually per landing, reported
// in the implementer's report — a real build step neither vitest nor this
// file can substitute for, since `css: false` means nothing here observes
// bundler output). Do not read a passing manual grep as this file's
// job — the regex below is the part that is actually mutation-provable.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const css = readFileSync("src/styles/layout.css", "utf8") as string;

/** The `.workspace-panel--capture` rule block plus everything up to (but
 * not including) the next top-level rule — i.e. just its own declaration
 * body, for assertions that must not accidentally match a DIFFERENT rule
 * later in the file. */
function ruleBody(selector: string): string {
  const start = css.indexOf(`${selector} {`);
  expect(
    start,
    `expected to find "${selector} {" in layout.css`,
  ).toBeGreaterThanOrEqual(0);
  const end = css.indexOf("}", start);
  return css.slice(start, end);
}

describe("bento workspace grid — CSS source contract (ticket W4)", () => {
  it("puts the wide-tier (>=1280) grid on the unquery'd base .workspace-panel--capture rule, per responsive-memo-72d4 §1", () => {
    const body = ruleBody(".workspace-panel--capture");
    expect(body).toContain("display: grid");
    expect(body).toMatch(
      /grid-template-areas:\s*\n\s*"notice\s+notice\s+notice"\s*\n\s*"transcript graph\s+agent"\s*\n\s*"transcript document agent"/,
    );
  });

  it("reserves an unclaimed, zero-height `notice` row at every tier for 586b's future notice banner (synthesis §W4 tripwire)", () => {
    const wideBody = ruleBody(".workspace-panel--capture");
    expect(wideBody).toMatch(/grid-template-rows:\s*\n\s*0px/);

    const standardStart = css.indexOf("@media (width < 1280px)");
    const standardEnd = css.indexOf("@media (width < 1024px)");
    const standardBlock = css.slice(standardStart, standardEnd);
    expect(standardBlock).toMatch(/grid-template-rows:\s*0px/);
    expect(standardBlock).toMatch(
      /grid-template-areas:\s*\n\s*"notice\s+notice"/,
    );

    const compactStart = css.indexOf("@media (width < 1024px)");
    const compactBlock = css.slice(compactStart);
    expect(compactBlock).toMatch(/grid-template-rows:\s*\n\s*0px/);
    expect(compactBlock).toMatch(
      /grid-template-areas:\s*\n\s*"notice"\s*\n\s*"document"/,
    );
  });

  it('assigns grid-area from [data-tile="…"] 1:1 for all four frozen tile ids, never inline (design-a §4.3 req 2/3)', () => {
    for (const id of ["transcript", "graph", "document", "agent"]) {
      const rule = new RegExp(
        `\\[data-tile="${id}"\\]\\s*\\{\\s*grid-area:\\s*${id};`,
      );
      expect(css, `missing [data-tile="${id}"] { grid-area: ${id}; }`).toMatch(
        rule,
      );
    }
  });

  it("puts container-type:inline-size (via the `container` shorthand) + a container-name on every .workspace-tile root (memo §2/§4 item 8)", () => {
    const body = ruleBody(".workspace-tile");
    // `container: <name> / inline-size` is the shorthand for
    // `container-name: <name>; container-type: inline-size;` — assert the
    // shorthand names BOTH a type of inline-size and a non-empty name, not
    // just the substring "inline-size" (which `container-type: size` would
    // also fail to satisfy, but a plain `contain: inline-size` unrelated
    // property could otherwise false-positive a naive substring check).
    const match = body.match(/container:\s*([\w-]+)\s*\/\s*inline-size\s*;/);
    expect(
      match,
      `expected "container: <name> / inline-size;" in .workspace-tile, got: ${body}`,
    ).not.toBeNull();
    expect(match?.[1]).toBeTruthy();
    // never `size` — collapses in the standard tier's `auto`-sized agent
    // row (memo §2 "Rejected: container-type: size").
    expect(body).not.toMatch(/container-type:\s*size\b/);
  });

  it("frames every tile with min-width:0/min-height:0/overflow:hidden so a phase-2 resize can't blow out the grid (design-a §4.3 req 6)", () => {
    const body = ruleBody(".workspace-tile");
    expect(body).toContain("min-width: 0");
    expect(body).toContain("min-height: 0");
    expect(body).toContain("overflow: hidden");
  });

  it("makes `.workspace-tile__body`'s own scroll container self-enforcing (overflow: auto, not hidden) — a future child that forgets to self-scroll gets a real scrollbar here instead of silent clipping", () => {
    const body = ruleBody(".workspace-tile__body");
    expect(body).toContain("overflow: auto");
    expect(body).not.toMatch(/overflow:\s*hidden/);
  });

  it("defines the standard tier at (width < 1280px) and the compact tier at (width < 1024px) — MQ Level 4 range syntax, reciprocal with useShellLayout.ts:34-35", () => {
    expect(css).toMatch(/@media \(width < 1280px\)/);
    expect(css).toMatch(/@media \(width < 1024px\)/);
  });

  it("stacks the compact (<1024) tier as document, graph, agent, transcript — RATIFIED R2, overriding responsive-memo-72d4's own graph-first draft table", () => {
    const compactStart = css.indexOf("@media (width < 1024px)");
    expect(compactStart).toBeGreaterThanOrEqual(0);
    const compactBlock = css.slice(compactStart);
    expect(compactBlock).toMatch(
      /grid-template-areas:\s*\n\s*"notice"\s*\n\s*"document"\s*\n\s*"graph"\s*\n\s*"agent"\s*\n\s*"transcript"/,
    );
  });

  it("stacks the standard (1024-1279) tier as transcript | graph/document/agent — agent drops UNDER the document, two columns (memo §2 table / design-a §4.2)", () => {
    const standardStart = css.indexOf("@media (width < 1280px)");
    const standardEnd = css.indexOf("@media (width < 1024px)");
    expect(standardStart).toBeGreaterThanOrEqual(0);
    expect(standardEnd).toBeGreaterThan(standardStart);
    const standardBlock = css.slice(standardStart, standardEnd);
    expect(standardBlock).toMatch(
      /grid-template-areas:\s*\n\s*"notice\s+notice"\s*\n\s*"transcript graph"\s*\n\s*"transcript document"\s*\n\s*"transcript agent"/,
    );
  });

  it("never reintroduces a 1120px media query (0922/R1: the old CSS-only 1120px workspace tier disagreed with useShellLayout's 1280/1024)", () => {
    expect(css).not.toMatch(/@media\s*\(\s*max-width:\s*1120px\s*\)/);
  });

  it("carries the NowStrip two-row reflow forward into the (width < 1280px) tier, not dropped by the 1120px deletion (memo §3 amendment)", () => {
    const standardStart = css.indexOf("@media (width < 1280px)");
    const standardEnd = css.indexOf("@media (width < 1024px)");
    expect(standardStart).toBeGreaterThanOrEqual(0);
    expect(standardEnd).toBeGreaterThan(standardStart);
    const standardBlock = css.slice(standardStart, standardEnd);
    expect(standardBlock).toContain(".now-strip {");
    expect(standardBlock).toContain(".now-strip__center {");
  });

  it("keeps the compact tier's columns floor-free (minmax(0, …)), never the wide tier's minmax(320px, …) — WCAG 1.4.10 Reflow at 320 CSS px (memo §3)", () => {
    const compactStart = css.indexOf("@media (width < 1024px)");
    const compactBlock = css.slice(compactStart);
    const captureBody = compactBlock.slice(
      compactBlock.indexOf(".workspace-panel--capture {"),
      compactBlock.indexOf(
        "}",
        compactBlock.indexOf(".workspace-panel--capture {"),
      ),
    );
    expect(captureBody).toMatch(/grid-template-columns:\s*minmax\(0,\s*1fr\)/);
    expect(captureBody).not.toMatch(/minmax\(320px/);
  });

  it("keeps the compact tier's ROWS floor-free too (minmax(0, …fr), never a px floor) — `.workspace-panel` is a fixed, non-scrolling, overflow:hidden box all the way up, so a px floor on this axis clips the bottom tiles with no scrollbar to recover them", () => {
    const compactStart = css.indexOf("@media (width < 1024px)");
    const compactBlock = css.slice(compactStart);
    const captureBody = compactBlock.slice(
      compactBlock.indexOf(".workspace-panel--capture {"),
      compactBlock.indexOf(
        "}",
        compactBlock.indexOf(".workspace-panel--capture {"),
      ),
    );
    const rowsMatch = captureBody.match(
      /grid-template-rows:\s*\n\s*0px\s*\n\s*minmax\(0,\s*1\.2fr\)\s*\n\s*minmax\(0,\s*0\.5fr\)\s*\n\s*minmax\(0,\s*0\.6fr\)\s*\n\s*minmax\(0,\s*0\.8fr\)/,
    );
    expect(
      rowsMatch,
      `expected compact grid-template-rows to be all minmax(0, …fr) (no px floor), got: ${captureBody}`,
    ).not.toBeNull();
    // Belt-and-suspenders: no px floor anywhere on this axis (the bug this
    // pins was four `minmax(NNNpx, …)` floors summing to 783px of hard
    // minimum block size inside a non-scrolling ancestor chain).
    expect(captureBody).not.toMatch(/minmax\(\s*\d+px/);
  });

  it("never lets the pre-W4 `.workspace-panel__primary`/`__transcript`/`__assist` BEM properties reappear as an actual rule (0922: deleted wholesale, not shadowed by a later rule) — durable source-text half of the manually-run production-bundle grep", () => {
    // Matches a real selector use (followed by a combinator/rule-opener),
    // not the class names' own doc-comment mentions elsewhere in this file
    // (backtick-quoted prose explaining the 0922 deletion, e.g. at the top
    // of `.workspace-panel--capture`'s comment) — those are intentional and
    // should stay.
    expect(css).not.toMatch(
      /\.workspace-panel__(primary|transcript|assist)\s*[,{.:[]/,
    );
  });
});
