# Commit State: During/After Shell Wave

Date: 2026-07-04

Branch: `docs/wave8-p6-residual-audit`

HEAD: `94f35892c041af3cfcd81901ed4d05bb7b596c15`

Active Seed: `audio-graph-d633`

## Dirty Tree Caveats

This wave is proceeding in a dirty checkout. The UX shell slice owns these files:

- `.seeds/issues.jsonl`
- `src/App.tsx`
- `src/App.test.tsx`
- `src/i18n/locales/en.json`
- `src/i18n/locales/pt.json`
- `src/styles/layout.css`
- `docs/backlog/commit-state-2026-07-04-during-after-shell-wave.md`
- `docs/designs/2026-07-04-during-after-shell.md`

Existing unrelated untracked backlog and preview-harness files are intentionally left untouched.

## Scope

Implement the first workspace information-architecture wave from `audio-graph-d633`:

- Add a top-level During / After / Analysis shell.
- Make notes, transcript, and live assist primary in During.
- Route sample and loaded sessions to After.
- Keep the graph and runtime diagnostics out of the default first-use surface.
- Validate focused tests and desktop plus narrow screenshots before expanding live-assist or memory views.

## Constraints

- No backend provider, credential, or capture-path changes.
- No Settings redesign.
- No promotion of hidden screen/audio capture behavior.
- Preserve backend-owned source, credential, and provider readiness boundaries.

## Verification Plan

- Focused App tests for workspace routing and diagnostics demotion.
- Typecheck.
- Screenshot pass at desktop and narrow viewports.
- Seeds extension update with evidence and remaining risk.
