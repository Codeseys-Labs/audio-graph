# ADR-0010: Icon system (lucide-react) replacing emoji iconography

## Status

Accepted 2026-05-29.

## Context

The app has **no icon system**. Every UI glyph is a literal emoji or Unicode
dingbat embedded in JSX (inventory in `docs/reviews/2026-05-29-uiux-deep-dive.md`):
`📝 💬 ⚠️ ✕` (App), `⏹ ⏺ 🎧 🤖 📊 ⚙️` (ControlBar), `🎙️ 🔄 📝 👥 🔍 🕸️`
(PipelineStatusBar), `🖥️ 🎤 🔊 📱 📦 🗂 ✓` (AudioSourceSelector), `➤ 🗑️`
(ChatSidebar), etc.

Problems this causes:

- **Inconsistent rendering** across OS/font versions; variation selectors
  (`U+FE0F`) occasionally render as stray glyphs; baseline/size is unpredictable
  (hence scattered `line-height:1` patches).
- **Not recolorable** via CSS `color`, so icons can't follow the theme tokens
  from ADR-0009 and can't express state (active/disabled/error).
- **Accessibility:** several emoji buttons lack `aria-label` (Refresh `🔄`, chat
  Send `➤`, Clear `🗑️`); screen readers may announce emoji names.
- **Duplication:** the close `✕` is reimplemented independently in 8+ components.

## Decision Drivers

- Crisp, consistent, theme-colorable icons that respond to state.
- One accessible, reusable icon-button primitive (label + focus ring + sizing).
- Small bundle impact (tree-shakeable), no icon-font/sprite build step.
- Works offline inside Tauri (no CDN).

## Considered Options

- **Option A — `lucide-react` + a thin `<Icon>` / `<IconButton>` wrapper.**
  Per-icon React components, tree-shaken by Vite; `currentColor` so icons take
  the token color; one `<IconButton>` enforces `aria-label` + focus-visible +
  token sizing.
- **Option B — Keep emoji**, just add missing `aria-label`s and `line-height`
  patches. Minimal effort.
- **Option C — Hand-rolled SVG sprite** (`<svg><use href>`) maintained in-repo.
- **Option D — Heavier icon set** (Phosphor / Tabler / Font Awesome).

## Decision Outcome

Chosen: **Option A**. `lucide-react` is MIT, tree-shakeable, has the full glyph
set this UI needs, renders as inline SVG using `currentColor` (so it inherits
ADR-0009 token colors and state), and bundles offline. The `<IconButton>`
wrapper makes the accessible-name and focus-ring non-optional, killing the
recurring `aria-label`-missing and duplicate-`✕` problems. Option B leaves the
rendering/recolor/duplication issues unsolved. Option C reinvents what Lucide
already maintains. Option D adds weight without benefit for this set.

### Consequences

- **Positive:** Consistent, themeable, stateful icons; one accessible icon
  button; removal of `line-height:1` hacks and 8+ ad-hoc close buttons.
- **Positive:** Icons participate in the token/theme system and reduced-motion
  rules from ADR-0009.
- **Negative:** One new dependency (~small with tree-shaking) and a mechanical
  sweep across all components to swap glyphs.
- **Neutral:** A few brand glyphs (e.g. provider logos) may still need custom
  SVG; those live in the same `<Icon>` registry.

## Implementation (intended)

- Add `lucide-react`; create `src/components/Icon.tsx` (name → Lucide component,
  size/`aria-hidden` defaults) and `src/components/IconButton.tsx` (requires
  `aria-label`, applies the global focus-visible ring and token sizing).
- Wave 2 of the plan: replace all emoji; provide one `<IconButton variant>` close
  control reused everywhere.

## References

- `docs/reviews/2026-05-29-uiux-deep-dive.md`
- lucide-react docs; Vite tree-shaking guidance for named icon imports.
- ADR-0009 (tokens/theming — icons consume `currentColor`/token colors).
