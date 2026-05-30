# Modernization Audit

**Date:** 2026-05-29
**Scope:** Frontend + tooling + backend modernization status and opportunities.
**Method:** Inspection of `package.json`, `src-tauri/Cargo.toml`, `tsconfig.json`,
`vitest.config.ts`, `.github/workflows/{ci,release}.yml`, and the styling layer.

This document (1) records the modernization already completed and (2) catalogs
remaining opportunities, prioritized with effort/risk so they can be worked down
deliberately. It is a living audit — update as items are addressed.

---

## 1. Completed this cycle

| Area | What | Reference |
|---|---|---|
| CSS organization | Split the 3,114-line `App.css` monolith into per-component modules under `src/styles/` (barrel `index.css`). | ADR-0015 |
| CSS toolchain | Adopted **Tailwind v4** via `@tailwindcss/vite`, **without Preflight**, tokens bridged through `@theme inline` (styles.css stays source of truth). Migrated 13 component-specific modules to utilities; retired dead `toasts.css`; consolidated 9 `@keyframes` into a retained `keyframes.css`. Shared design-system classes (`.btn`/`.icon-btn`, settings form system, app shell) deliberately retained as component-layer CSS. | ADR-0016, `docs/research/css-modernization-tailwind-shadcn.md` |
| Accessibility | ARIA tablists (right panel, conversation mode), focus-trapped `PopoverOverlay` (Escape + `aria-modal`), `role=log`/`aria-live` chat + transcript, `role=status` panels, reduced-motion-aware JS scrolling, `.sr-only` utility. | commits `a04ddd5`, `76b766c`, `3842f7f` |
| Verification | Live render check (Vite + Playwright) confirming migrated utilities and retained CSS coexist with no regression; a11y roles intact. | — |

Honest framing (per ADR-0016): the Tailwind move is a **toolchain
modernization**, not a CSS reduction — the bundle is roughly flat and styling
relocated into `className`. Benefit is consistency / Tailwind-native component
styling, not size.

---

## 2. Opportunities (prioritized)

### P1 — High value, do next

**2.1 Add a frontend linter + formatter (currently NONE).**
There is no ESLint / Biome / Prettier config; CI's only lint job is Rust
`fmt`+`clippy`. Indentation is inconsistent across files (2- vs 4-space), a
direct symptom. This is the single biggest tooling gap.
- *Recommendation:* adopt **Biome** (one fast binary = lint + format, zero-config-ish,
  no plugin sprawl) — or ESLint 9 flat config + Prettier if the team prefers the
  ecosystem. Add a `format`/`lint` script and a CI step in the existing frontend job.
- *Effort:* S–M. *Risk:* low (formatting churn is mechanical; do it in one commit).

**2.2 Enforce Clippy as `-D warnings` in CI.**
`ci.yml` runs `cargo clippy --all-targets` but intentionally does **not** fail on
warnings ("flip to `-D warnings` once clippy-clean"). Close the loop: clean the
tree, then enforce so new warnings can't land.
- *Effort:* M (depends on current warning count). *Risk:* low.

**2.3 Code-split the JS bundle.**
`vite build` warns: single `index.js` ≈ 585 KB (≈178 KB gzip), over the 500 KB
limit. `react-force-graph-2d` is the heavy contributor.
- *Recommendation:* `React.lazy` + dynamic `import()` for the graph viewer and the
  settings modal; add `build.rollupOptions.output.manualChunks` to split vendor.
- *Effort:* S–M. *Risk:* low–medium (lazy boundaries need Suspense fallbacks).

### P2 — Worthwhile, plan deliberately

**2.4 React 18 → React 19.**
React 19 is stable (Actions, `use`, ref-as-prop, improved Suspense, optional
React Compiler). The codebase (zustand, function components, hooks) is
compatible. `react-i18next`/`react-force-graph-2d` peer-dep support must be
verified first.
- *Effort:* M. *Risk:* medium (broad surface; gate on the full test suite + a
  live smoke test). Worth an ADR.

**2.5 Rust edition 2021 → 2024.**
Edition 2024 is stabilized. Migrate via `cargo fix --edition` then bump
`edition = "2024"` (and `rust-version` floor).
- *Effort:* S–M (mostly mechanical). *Risk:* low–medium; CI per-platform build +
  tests are the gate.

**2.6 `tsconfig` target ES2020 → ES2022.**
The app only runs in modern engines (WebView2 / WKWebView / WebKitGTK), so
ES2022 (top-level await, `.at()`, `Error.cause`, class fields) is safe and trims
transpilation. Pair with `lib` bump.
- *Effort:* S. *Risk:* low.

**2.7 Modernize / fix the Windows test harness.**
`cargo test` aborts on Windows (`STATUS_ENTRYPOINT_NOT_FOUND`, ADR-0007); tests
run via `scripts/run-core-tests.ps1` on a curated subset. Frontend `vitest`
emits pre-existing `act()` warnings in `SettingsPage`. Investigate the Windows
linker/runtime issue so the full suite runs locally on Windows, and clean the
`act()` warnings.
- *Effort:* M. *Risk:* low (tooling only).

### P3 — Low priority / hygiene

- **2.8 Pin the JS toolchain.** No `engines` field or `.nvmrc`/bun pin; CI uses
  `setup-bun@v2` unpinned-to-version. Pin a bun version for reproducibility. (S)
- **2.9 Verify `lucide-react ^1.17.0`.** The version is unusual for lucide-react
  (normally `0.x`); confirm it is the intended package/version, or pin
  deliberately. (XS)
- **2.10 Coverage gate.** `vitest --coverage` exists but no threshold is enforced;
  consider a floor in CI once a baseline is known. (S)
- **2.11 Trim Tailwind default theme.** `@import "tailwindcss/theme.css"` emits
  ~7 KB of unused default theme variables. `@theme { --*: initial; }` + re-adding
  only the used namespaces would reclaim it — but it risks breaking
  theme-derived utilities (`font-bold`, etc.), so only with careful verification.
  Low ROI given ~10 KB gzip total CSS. (S, deferred)
- **2.12 Bundle-analyze** with `rollup-plugin-visualizer` to confirm 2.3 targets. (XS)

---

## 3. Explicitly NOT recommended

- **shadcn/ui** — would duplicate/replace the hand-built, tested, accessible
  Tabs/Dialog/Buttons and add runtime Radix deps for no capability gain
  (`docs/research/css-modernization-tailwind-shadcn.md`, ADR-0016).
- **`@apply`-converting the retained component CSS** — churn over already-clean
  token-based CSS with no functional gain; Tailwind's own guidance discourages it.
- **Big-bang dependency bumps** — the Rust deps are current (tauri 2.10, tokio
  1.50, serde 1.0.228, thiserror 2, reqwest 0.13, mistralrs 0.8); no broad bump
  is warranted. Prefer Dependabot (already implied by SHA-pinned CI actions).

---

## 4. Suggested sequencing

1. **2.1 linter/formatter** (unblocks consistent style; mechanical churn first).
2. **2.2 clippy `-D warnings`** + **2.6 ES2022** + **2.8/2.9 pins** (cheap hardening).
3. **2.3 code-split** (perceptible startup win).
4. **2.5 Rust 2024 edition** (mechanical, well-gated by CI).
5. **2.4 React 19** (largest; own ADR + live smoke test).
6. Revisit P3 hygiene as capacity allows.

Each item should land as its own verified change (tsc + vitest + `cargo
test`/clippy + a live smoke test for UI-affecting ones), consistent with the
project's per-change verification discipline.
