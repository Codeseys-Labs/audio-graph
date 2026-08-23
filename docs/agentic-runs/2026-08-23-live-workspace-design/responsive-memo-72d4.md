# Responsive-layout architecture for the bento workspace (W4) — seed audio-graph-72d4

Decision memo for the W4 implementer. Research date 2026-08-22. Every repo claim below was
read in the working tree; every external claim carries a URL in §6.

## 1. Recommendation

**Split authority by what is being decided: the grid *shape* is a viewport question, a tile's
*internal* layout is a container question.** Media queries aligned to `useShellLayout`'s
1280/1024 own `grid-template-areas`, track counts, and which tier stacks; per-tile `@container`
size queries own header wrapping, chip-row density, gutter affordances, and feed row shape. Put
`container-type: inline-size` (never `size`) plus a `container-name` on every `.workspace-tile`
root in phase 1 even though phase 1 has no user resize — the moment phase 2 lets someone drag a
tile to 380px, viewport width stops describing that tile at all, and retrofitting containment
later means re-auditing every rule inside every tile for the containing-block and
stacking-context side effects `container-type` brings with it. All `@container` blocks live in
`src/styles/layout.css` (inside the unlayered `styles/index.css` barrel imported at
`App.tsx:132`), which is what makes them beat Tailwind's layered utilities. Delete the 1120px
tier — but **split** that media block rather than deleting it wholesale (§3).

## 2. The hybrid, spelled out

**Viewport tiers own (media queries, in `layout.css`):** `grid-template-areas`,
`grid-template-columns`, `grid-template-rows`, the `[data-graph-mode="canvas"]` row swap, and
whether a tier is one column or three. Two boundaries only, matching
`useShellLayout.ts:34-35`:

| Tier | Query | Shape |
|---|---|---|
| wide (default, ≥1280) | base rule, no query | `"transcript graph agent" / "transcript document agent"` |
| standard (1024–1279) | `@media (width < 1280px)` | agent drops under document; two columns |
| compact (<1024) | `@media (width < 1024px)` | single column, `graph / document / agent / transcript` |

Use MQ Level 4 range syntax (`width < 1280px`), not `max-width: 1279.98px` — exact, no
fencepost, Baseline. Keep `≥1280` in the unquery'd base rule so the ratified default layout is
what you get with no media query evaluated at all.

**Container queries own (per tile):** everything inside a tile. On the tile root:

```css
.workspace-tile { container: tile / inline-size; min-width: 0; min-height: 0; overflow: hidden; }
@container tile (width < 28rem) { /* stack the header, drop the counter chip, etc. */ }
```

`inline-size`, not `size`: `container-type: size` applies size containment on **both** axes,
and size containment means "the size of the size-contained element's children cannot affect the
size of the element itself — its size is computed as if it had no children", with MDN warning
that without `contain-intrinsic-size` "the element risks being zero-sized in most cases". The
standard tier's agent row is `auto`-sized, and a `size` container in an `auto` track collapses.
`inline-size` contains only the inline axis, so block-axis auto sizing still works.

Name the container: `@container` without a name resolves against the *nearest* ancestor query
container, so an unnamed container added later inside `LiveDocument` or `LiveGraphStrip` would
silently capture a query written for the tile root. Pick tile-internal thresholds from content,
not devices, cap at two per tile, and borrow Tailwind v4's container scale (`@md` 448px, `@lg`
512px, `@2xl` 672px) so they stay expressible as `@lg:` utilities if a tile is ever authored in
Tailwind rather than plain CSS.

**Exact cascade placement — this is the 0922 trap in a new costume.** CSS Conditional Rules 3
§2: "When the condition is true, CSS processors **must** apply the rules inside the group rule
as though they were at the group rule's location." So an `@container` block is exactly as
layered as its surroundings, and MDN is unambiguous that "styles that are not defined in a
layer always override styles declared in named and anonymous layers." `layout.css` is
unlayered (reached via `App.tsx:132` → `styles/index.css`), while `styles.css:25-27` declares
`@layer theme, base, components, utilities` and imports Tailwind into `theme`/`utilities`.
Therefore: **tile `@container` rules go in `layout.css` and win. Do not put them in
`styles.css`'s `@layer components` `.ag-*` block** — there they lose to any unlayered rule and
reproduce 0922 somewhere new. Also note `@container` contributes **no** specificity, so a
container-query override of a base rule in the same file must either follow it or win on the
selector.

**Performance at 4–6 containers is a non-issue.** Containment narrows rather than widens work
(`contain: layout` tells the browser "it only needs to check this element"), and the pattern
container queries replace is the JS resize listener — Netflix's write-up frames the win as
"avoiding runtime calculations." That is also what makes W4's "zero JS viewport reads in tiles"
true by construction rather than by discipline. The only real hazard, a query that changes a
property affecting the container's own size in the queried axis, is structurally impossible
under inline-size containment.

## 3. The 1120px tier: confirm R1, with a correction the ticket needs

**Confirmed — the workspace's 1120px reflow dies and nothing replaces it.** The two authorities
verifiably disagree: `useShellLayout.ts:34-35` (1280/1024) vs `layout.css:144` (1120). At 1100px
the aside is a drawer *and* the workspace is already single-column; at 1200px the aside is a
drawer but the workspace is still two columns. Either boundary set beats having both, and
1024/1280 is the one with a hook, a test file, and an ADR behind it.

**Amendment W4 must not miss:** `@media (max-width: 1120px)` at `layout.css:144-197` is not only
a workspace block — it also carries the `.now-strip` two-row reflow (`:172-196`), added for a
measured, unrelated reason (below ~1120px the single-row header occludes the green Start button).
Deleting the block wholesale regresses that fix for 1024–1119px. Split it: workspace rules move
to `(width < 1024px)`; the NowStrip rules move **up** to `(width < 1280px)`, not down, because
1280 > 1120 means the header reflows strictly earlier than the width where it was observed
breaking, so the fix cannot regress. The only thing left to check is that stacking at 1200–1279px
does not look gratuitous — screenshot that range as part of W4.

**Floor obligations.** `src-tauri/tauri.conf.json` declares `1400×900`, `resizable: true`, and
**no `minWidth`**, so the window can be dragged arbitrarily narrow; 200% zoom of the default
window is ~700px CSS, which is why `useShellLayout`'s doc comment calls the tier plumbing the
WCAG 1.4.4 mechanism. WCAG 1.4.10 Reflow requires no two-dimensional scrolling at 320 CSS px, so
design A §4.1's `minmax(320px, …)` column floors are right for the three-column wide template
but must **not** survive into the compact single-column template — use `minmax(0, 1fr)` there or
the grid overflows horizontally below 320px and breaks 1.4.10.

## 4. Phase-1 markup obligations for phase 2

Design A §4.3's seven requirements all stand and are the right list. Four additions:

8. **`container: tile / inline-size` on every tile root in phase 1.** Adding it in phase 2
   changes the containing block and stacking context for everything already inside every tile;
   adding it now means nothing ever depended on its absence. Pair it with A's requirement 4 —
   the `--tile-fr-*` fallbacks — and note in-file that phase-2 resize writes those vars,
   because same-structure `fr` changes are the only interpolable part of this system (§5).
9. **DOM order = the compact stacking order (`graph, document, agent, transcript`), not the
   wide visual order.** Grid placement is visual-only, and the CSS fix for the mismatch —
   `reading-flow` / `reading-order` — is Chrome 137+, flagged experimental and "limited
   availability" on MDN, and the `deb`/`appimage` targets run WebKitGTK, so it cannot be a
   dependency. The principled tiebreak: a two-dimensional bento presents no single reading
   sequence for WCAG 1.3.2 to be measured against, while a single-column stack does — so make
   source order correct in compact (also the 200%-zoom tier) and let wide reorder visually.
   Not a new divergence: `App.tsx:494-513` already renders `NotesPanel` before `LiveTranscript`.
10. **No `order:` and no `*-reverse` anywhere in the tier reflow** — placement via
    `grid-template-areas` + `[data-tile]` only. `order:` is precisely what
    `reading-flow: grid-order` exists to repair, and that repair is unavailable here.
11. **Audit tile-local `position: fixed` / `absolute` now.** `container-type` applies layout
    containment, which creates a containing block for absolutely- *and fixed*-positioned
    descendants and establishes a stacking context. Anything that must escape a tile — the
    document gutter's Radix Popover, tooltips, 586b's future banner — must portal to `body`.

## 5. Risks and rejected alternatives

- **Rejected: container queries for the grid shape itself.** A container cannot query itself —
  `@container` resolves against the nearest *ancestor* query container — so the bento would
  need a wrapper purely to be queried, and with no eligible ancestor there is nothing to match
  against, i.e. a whole tier can silently vanish. Worse, tier shape genuinely *is* a viewport
  property: whether the aside is a pinned column or a drawer is a window decision
  `useShellLayout` already owns, and the bento must agree with it.
- **Rejected: `ResizeObserver` per tile** — reintroduces the runtime-measurement pattern
  container queries exist to delete, and violates W4's "zero JS viewport reads."
- **Rejected: `container-type: size`** — collapses in `auto` tracks, needs
  `contain-intrinsic-size` bookkeeping, buys block-axis queries nothing needs.
- **Rejected: `@custom-media` to single-source 1280/1024 between the hook and CSS.** Custom
  properties are invalid in media query conditions, and `@custom-media` is Media Queries
  Level 5, experimental, with overriding behavior still under discussion. Keep the numbers
  duplicated but *pinned*: reciprocal comments in `layout.css` and `useShellLayout.ts:34-35`,
  plus the boundary values in W4's acceptance notes.
- **Risk, and a thing not to build: there is no reflow animation to reduce.** Per MDN,
  `grid-template-areas` has animation type **discrete**, and `grid-template-columns`/`rows`
  interpolate only when the two lists "differ only in the values of the length, percentage, or
  calc components." A tier change alters the track *count*, so it is a discrete snap by
  specification — nothing animates and `prefers-reduced-motion` has nothing to suppress at a
  tier boundary. Do not add a `transition` on `grid-template-columns` there hoping to soften
  it; it will not fire, and it will mislead the next reader. Reduced-motion is load-bearing for
  phase-2 resize (same-structure `fr` changes *are* interpolable) and for W10's pulse; both
  already have gates.
- **Risk: the Linux browser floor, mitigated architecturally.** Container queries are Baseline
  widely available since February 2023 (Chrome/Edge 105–106, Safari 16.0, Firefox 110).
  WebView2 and macOS 14.4's WebKit are far past it; the exposure is `deb`/`appimage`, which run
  the *host's* WebKitGTK, and pre-2.38 hosts have no `@container` at all. Because container
  queries only refine tile internals, such a host renders every tile at its base internal
  layout with the grid untouched — so author the base rules narrow-first and the no-CQ path
  degrades to usable rather than broken. Do **not** gate anything behind
  `@supports (container-type: inline-size)`; the fork buys nothing and doubles the surface.
- **Risk: relayering.** If anyone later moves `layout.css` under `@layer`, every tile
  `@container` rule silently loses to Tailwind utilities. Leave the 0922 note in-file beside
  the container rules, not only in the ticket.

## 6. Sources

- Containment applied per `container-type`, nearest-ancestor resolution, `cq*` units and their viewport fallback, `container-name` rationale — https://developer.mozilla.org/en-US/docs/Web/CSS/CSS_containment/Container_queries
- `@container` as a conditional group rule; Baseline widely available since February 2023; `style()`/`scroll-state()` are separate newer surfaces — https://developer.mozilla.org/en-US/docs/Web/CSS/@container
- Size containment computes size "as if it had no children", the zero-size warning, and layout containment creating a containing block for absolutely/fixed descendants plus a stacking context — https://developer.mozilla.org/en-US/docs/Web/CSS/CSS_containment/Using_CSS_containment
- CSS Conditional Rules 3 §2, rules inside a conditional group rule apply "as though they were at the group rule's location" — https://drafts.csswg.org/css-conditional-3/
- "Styles that are not defined in a layer always override styles declared in named and anonymous layers" — https://developer.mozilla.org/en-US/docs/Web/CSS/@layer
- Container query support: Chrome/Edge 105 partial → 106 full, Safari 16.0, Firefox 110 — https://caniuse.com/css-container-queries · https://webkit.org/blog/13152/webkit-features-in-safari-16-0
- Animation types: `grid-template-areas` discrete, `grid-template-columns`/`rows` interpolable only when the lists differ solely in numeric components — https://developer.mozilla.org/en-US/docs/Web/CSS/grid-template
- `reading-flow`/`reading-order`, experimental and limited availability, Chrome 137+ — https://developer.mozilla.org/en-US/docs/Web/CSS/reading-flow · https://developer.chrome.com/blog/reading-flow
- "Don't define breakpoints based on device classes… let the content determine how its layout changes" — https://web.dev/articles/responsive-web-design-basics
- Material Design 3: "focusing on breakpoints ensures layouts work across a wide range of devices"; 5 window classes (<600 / 600–839 / 840–1199 / 1200–1599 / ≥1600 dp) — https://m3.material.io/foundations/layout/breakpoints
- Fluent/WinUI: 3 tiers (<640 / 641–1007 / ≥1008 px) — https://learn.microsoft.com/en-us/windows/apps/design/layout/screen-sizes-and-breakpoints-for-responsive-design
- Tailwind v4 viewport scale 640/768/1024/1280/1536 and the separate 13-step container scale (`@md` 448, `@lg` 512, `@2xl` 672) — https://tailwindcss.com/docs/responsive-design
- WCAG 1.4.10 Reflow, 320 CSS px derived from a 1280px window at 400% zoom — https://www.w3.org/WAI/WCAG22/Understanding/reflow · 1.4.4 Resize Text (200%) — https://www.w3.org/WAI/WCAG21/Understanding/resize-text.html
- `@custom-media` experimental, overriding behavior under discussion — https://developer.mozilla.org/en-US/docs/Web/CSS/@custom-media · https://www.w3.org/TR/mediaqueries-5/#custom-mq
- Netflix: replacing JS resize listeners "improves performance by avoiding runtime calculations" — https://web.dev/case-studies/netflix-cq
