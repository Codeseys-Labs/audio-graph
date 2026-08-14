# audio-graph-fcca implementation report

Date: 2026-08-14

Seed: `audio-graph-fcca`

Parent: `audio-graph-ada2`

Branch: `work/fcca-readiness-ui-wave6`

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/fcca-readiness-ui-wave6`

Exact base: `8eef50ca46b1fe1c784649d161f3409b3de60bc4`

## Outcome

The frontend now presents operational provider readiness separately from the
selected STT configuration's transcription fidelity. A healthy final-only
provider remains visibly `Ready` while a separate, labelled Transcription
fidelity region explains final-only revisions, AudioGraph-estimated timing,
and unavailable confidence, turn, speaker, and channel evidence. The copy is
explicitly recovery-neutral: these limits describe the selected configuration
and require no action.

The same provider-neutral renderer is used by the active STT readiness panel
and registry-backed provider capability cards. It accepts only
`effective_stt_fidelity`, never a provider id or static registry flags.
Deepgram speaker-label enabled, disabled-by-configuration,
unavailable-for-model, and app-remapped states therefore follow typed backend
origins/degradation codes only.

Missing `effective_stt_fidelity` preserves the previous UI. Unknown or partial
future payloads render localized `Not reported` / generic reduced-detail copy
without crashing or echoing raw values. Closed degradation codes map to static
localized copy; no transcript, speaker label, credential, endpoint body, or
free-form backend diagnostic is admitted to the fidelity region.

No Rust/backend, provider registry, generated contract, adapter, ASR/speech,
workflow, Seed, selector-option, store, controller, or `ui_selectable` source
was changed.

## Acceptance evidence

- Operational readiness and fidelity are separate accessible regions: the
  existing polite `role=status` remains concise operational health, while a
  labelled `<section>` contains fidelity and is not live or recovery copy.
- Final-only ready-but-degraded fixture renders `Ready`, `Reduced transcript
  detail`, `Final results only`, `Estimated by AudioGraph`, and explicit
  unavailable confidence/turn/speaker/channel labels.
- A same-provider-id sensitivity test changes `asr.deepgram` from
  provider-reported speaker labels to `Disabled in settings` only by changing
  typed effective fields and `speaker_disabled_by_configuration`.
- The Settings capability-card integration test proves the shared typed view is
  present in the STT provider capability surface.
- Missing-field and hostile unknown-code fixtures do not crash and do not echo
  the raw transcript-like or credential-like diagnostic fixture.
- English and Portuguese locale trees remain structurally identical.
- The generated provider registry is byte-identical to the base. Base/current
  SHA-256 are both:

```text
a50060bca2b93ac3afef3591ee085b9f1bb392708ec86682b406ae84942ffbfc
```

## Deep module

`providerSttFidelityPresentation(effectiveFidelity, t)` is the single
presentation module. Its small interface owns validation of runtime values,
closed degradation mapping, reduced/full classification, speaker configuration
semantics, accessible row labels, localization, de-duplication, and safe
future-value fallback. `ProviderSttFidelityDetails` is the shared renderer at
both public UI seams. Neither interface accepts `provider_id`, so provider-id
inference is structurally unavailable to callers.

## TDD evidence

Initial ProviderReadinessPanel RED, before fidelity presentation existed:

```text
FAIL ProviderReadinessPanel > keeps healthy final-only readiness separate from recovery-neutral fidelity limits
TestingLibraryElementError: Unable to find an accessible element with the role "region" and name `/transcription fidelity/i`
Test Files 1 failed (1)
Tests 1 failed | 22 passed (23)
```

Focused GREEN after the shared view-model/renderer and compatibility slices:

```text
Test Files 1 passed (1)
Tests 25 passed (25)
```

Initial Settings capability-card RED, before the card consumed the shared
renderer:

```text
FAIL SettingsPage > renders provider capability cards by stage from registry and readiness metadata
TestingLibraryElementError: Unable to find an accessible element with the role "region" and name `/transcription fidelity/i`
Test Files 1 failed (1)
Tests 1 failed | 123 skipped (124)
```

Focused Settings capability-card GREEN:

```text
Test Files 1 passed (1)
Tests 1 passed | 123 skipped (124)
```

## Files

- `src/components/ProviderReadinessPanel.tsx`
- `src/components/ProviderReadinessPanel.test.tsx`
- `src/components/settings/ProviderCapabilityCard.tsx`
- `src/components/SettingsPage.test.tsx`
- `src/i18n/locales/en.json`
- `src/i18n/locales/pt.json`
- this report

## Gates and real results

- Focused repository-runner UI/controller/store/locale slice:

```text
Test Files 8 passed (8)
Tests 299 passed (299)
Duration 28.68s
```

- `bun run typecheck`: PASS, no diagnostics.
- `bun run check`: PASS.

```text
Checked 174 files in 307ms. No fixes applied.
```

- `bun run build`: PASS.

```text
2940 modules transformed
built in 5.54s
```

- `bun run verify:contracts`: PASS; audio-source, provider-registry,
  session-data-movement, endpoint-credential-routing, and speech-span contracts
  are current.
- Exact full `bun run test:local`: PASS.

```text
Test Files 70 passed (70)
Tests 966 passed (966)
Duration 106.67s
```

- Repository-pinned Seeds override:

```text
SEEDS_CLI_ROOT=$PWD/node_modules/@os-eco/seeds-cli bun run verify:fast
Checked 174 files in 282ms. No fixes applied.
sd ready --format json: parsed (50)
sd blocked --format json: parsed (89)
sd list --format json: parsed (50)
docs/Seeds secret hygiene scan passed: 0 findings
```

- Base/current generated registry comparison: PASS, byte-identical with the
  SHA-256 above.
- `git diff --check 8eef50ca46b1fe1c784649d161f3409b3de60bc4 --`:
  PASS.
- Final docs/Seeds secret hygiene: PASS, 0 findings.
- Final `betterleaks` scan across the complete implementation/report footprint:

```text
scanned ~435226 bytes (435.23 KB)
no leaks found
```

## Verification setup notes

- A direct multi-file `bun run test -- ...` invocation passed 288 tests but
  failed 11 store tests because Node's experimental web storage left
  `localStorage` undefined. This is why the repository supplies
  `scripts/run-vitest-local.mjs`; the identical slice passed 299/299 through
  `bun run test:focused`.
- The clean worktree initially lacked its lockfile-pinned `node_modules`, so the
  repository runner could not resolve its local Vitest entrypoint. `bun install
  --frozen-lockfile` restored ignored dependency state without changing any
  manifest or lockfile; all required runner gates then passed.

## Findings and open questions

- No in-scope blocker or unresolved product finding remains.
- Static maximum-capability rows remain registry-owned. Selected-configuration
  fidelity is separately labelled and backend-readiness-owned; per-span Speech
  Span Revision v2 evidence remains authoritative during actual sessions.

## Rollback

Rollback removes the shared fidelity view-model/renderer, its two call sites,
localized copy, and focused tests. It does not alter saved settings, secrets,
session artifacts, generated contracts, backend readiness behavior, or the
selectable provider set.
