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

### Execution pass (2026-05-29) — audit items landed

| Item | Status | Notes |
|---|---|---|
| 2.1 Biome formatter + linter + CI gate | ✅ done | Biome 2.4, 2-space; formatted 59 files; `biome ci .` gate in CI. Linter `recommended` ON; 15 rules with pre-existing violations demoted to `warn` (see ratchet below). |
| 2.3 Code-split bundle | ✅ done | `React.lazy` graph + Settings/Sessions/ExpressSetup modals; `manualChunks` react-vendor. Initial chunk 585→372 KB; 500 KB warning gone. Live-verified. |
| 2.4 React 18 → 19 | ✅ done | 19.2; clean (no source changes); deps already expected 19. Live-verified. |
| 2.6 tsconfig ES2020 → ES2022 | ✅ done | target + lib. |
| 2.8 Pin toolchain | ✅ done | `packageManager: bun@1.3.14` + `engines`. |
| 2.9 Verify `lucide-react` | ✅ done | 1.17.0 resolves and renders; left as-is. |
| 2.10 Coverage gate | ✅ already present | `vitest.config.ts` enforces 60/50/55/60 thresholds. |
| 2.2 Clippy `-D warnings` | ◑ partial | Safe cloud-feature autofixes applied (unused import, derivable Default). **Enforcement deferred** — see below. |
| 2.5 Rust edition 2021 → 2024 | ⏸ deferred | See below. |

**New follow-up — Biome lint ratchet (a11y-heavy). ✅ a11y wave done (2026-05-30).**
The linter surfaced **123 warnings**, overwhelmingly accessibility:
`noLabelWithoutControl` ×42 (settings form fields need `htmlFor`/`id`),
`useButtonType` ×23 (buttons missing `type="button"`), `useKeyWithClickEvents`
×8, `useSemanticElements` ×7, `useAriaPropsSupportedByRole` ×5, plus
`noNonNullAssertion` ×17 and `useExhaustiveDependencies` ×7. These pre-dated
linting and were demoted to `warn` so CI stayed green.

**Outcome:** all **93 a11y warnings fixed** across 18 files and the **9 a11y
rules promoted from `warn` → `error`** in `biome.json` (now CI-enforced). Fixes:
`htmlFor`/`id` association on every settings label (also strengthens the
`getByLabelText` tests), `type="button"` on raw action buttons, `role="none"` +
`onKeyDown` Escape on modal backdrops, keyboard handlers + `role="separator"`
arrow-key resizing on `ResizeDivider`, and correct semantic elements / roles
elsewhere. Five `biome-ignore` suppressions remain, each justified (custom
`role="checkbox"` source rows, the `role="meter"` confidence bar, the drag
separator) — all *used*, so `biome ci` errors on any that become stale.
Verified: `biome ci` exit 0, `tsc` clean, 148 tests pass, `vite build` clean.

**Still `warn` (separate, non-a11y — out of this wave):** `noNonNullAssertion`
×17, `useExhaustiveDependencies` ×7, `useTemplate` ×2, `noArrayIndexKey` ×2,
`noUselessSwitchCase` ×1, `useOptionalChain` ×1 (30 total). A future hygiene
ratchet can pick these off and promote them too.

**Why 2.2 / 2.5 are deferred (not skipped).** `default = ["local-ml"]`, so CI
lints/builds the heavy native ML tree (whisper-rs / llama-cpp-2 / mistralrs).
That tree does **not** build on the current Windows dev host, and `cargo test`
is broken on Windows (ADR-0007), so neither clippy `-D warnings` enforcement nor
a `cargo fix --edition` migration can be *fully verified* here — doing them
cloud-only risks breaking the default-feature CI build. **Do these on Linux/CI:**
(a) confirm `cargo clippy --all-targets` (default features) is warning-clean, fix
remainder, then change CI line 83 to `cargo clippy --all-targets -- -D warnings`;
(b) `cargo fix --edition` on default features, bump `edition = "2024"`, verify the
per-platform CI build + tests.

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
