# audio-graph-fcca implementation report

Date: 2026-08-14

Seed: `audio-graph-fcca`

Parent: `audio-graph-ada2`

Branch: `work/fcca-readiness-ui-wave6`

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/fcca-readiness-ui-wave6`

Exact base: `8eef50ca46b1fe1c784649d161f3409b3de60bc4`

Review-correction starting tip: `f02abc17ea4749e4fbab7d7c715356f09b8fe805`

## Outcome

The frontend now presents operational provider readiness separately from the
selected STT configuration's transcription fidelity. A healthy final-only
provider remains visibly `Ready` while a separate, labelled Transcription
fidelity region explains final-only revisions, AudioGraph-estimated timing,
and unavailable confidence, turn, speaker, and channel evidence. The copy is
explicitly recovery-neutral: these limits describe the selected configuration
and require no action.

The same provider-neutral renderer is used by the active STT readiness panel
and the selected registry-backed provider capability card. Unselected cards
omit cached effective fidelity. The renderer accepts only
`effective_stt_fidelity`, never a provider id or static registry flags.
Deepgram speaker-label states and all seven generated turn-detection booleans
therefore follow typed backend fields only. The two page-level regions have
localized, context-specific accessible names.

Missing `effective_stt_fidelity` preserves the previous UI. Unknown or partial
future payloads, including absent or malformed `degradations`, render
localized `Fidelity details incomplete` / `Not reported` copy without crashing,
claiming full fidelity, or echoing raw values. Closed degradation codes map to
static localized copy; no transcript, speaker label, credential, endpoint
body, or free-form backend diagnostic is admitted to the fidelity region.

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
- The same sensitivity test changes speech-start, speech-final, endpointing,
  utterance-end, end-of-turn, eager-end, and turn-resume rows only by changing
  typed `turn_detection` booleans.
- The Settings capability-card tests prove unselected cached fidelity is
  omitted and selected fidelity is present.
- Page-level accessibility coverage proves the active-provider and selected
  capability-card regions have distinct localized landmark names.
- Missing-field and hostile unknown-code fixtures do not crash and do not echo
  the raw transcript-like or credential-like diagnostic fixture.
- Missing or malformed degradation arrays render incomplete details and never
  `Full transcript detail`.
- Compact repository-conventional CSS supplies spacing, typography, and list
  indentation without changing the broader Settings layout.
- English and Portuguese locale trees remain structurally identical.
- The generated provider registry is byte-identical to the base. Base/current
  SHA-256 are both:

```text
a50060bca2b93ac3afef3591ee085b9f1bb392708ec86682b406ae84942ffbfc
```

## Deep module

`providerSttFidelityPresentation(effectiveFidelity, t)` is the single
presentation module. Its small interface owns validation of runtime values,
closed degradation mapping, incomplete/reduced/full classification, speaker
configuration semantics, generated turn-detection rows, accessible labels,
localization, de-duplication, and safe future-value fallback.
`ProviderSttFidelityDetails` is the shared renderer at both public UI seams and
accepts a localized landmark context. Neither interface accepts `provider_id`,
so provider-id inference is structurally unavailable to callers.

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

### Review-correction RED/GREEN

Typed turn detection RED before generated booleans were rendered:

```text
FAIL ProviderReadinessPanel > uses typed effective fields, not the Deepgram provider id, for enabled and disabled speaker labels
Expected /Speech-start events\s*Enabled/i
Test Files 1 failed (1)
Tests 1 failed | 24 skipped (25)
```

GREEN after rendering all seven typed fields:

```text
Test Files 1 passed (1)
Tests 1 passed | 24 skipped (25)
```

Version-skew RED before missing `degradations` became incomplete:

```text
FAIL ProviderReadinessPanel > treats missing or malformed degradation arrays as incomplete version-skewed fidelity
Expected /Fidelity details incomplete/i
Received Full transcript detail
Test Files 1 failed (1)
Tests 1 failed | 25 skipped (26)
```

GREEN after conservative classification:

```text
Test Files 1 passed (1)
Tests 1 passed | 25 skipped (26)
```

Unselected cached-card RED before the selected gate:

```text
FAIL SettingsPage > renders provider capability cards by stage from registry and readiness metadata
expected document not to contain the transcription fidelity region
Test Files 1 failed (1)
Tests 1 failed | 123 skipped (124)
```

GREEN after gating on the existing selected flag:

```text
Test Files 1 passed (1)
Tests 1 passed | 123 skipped (124)
```

Page-landmark RED before context labels:

```text
FAIL SettingsPage > gives active and selected-card fidelity regions unique accessible names
Unable to find role="region" and name `/active provider transcription fidelity/i`
Test Files 1 failed (1)
Tests 1 failed | 124 skipped (125)
```

GREEN with localized active/selected labels:

```text
Test Files 1 passed (1)
Tests 1 passed | 124 skipped (125)
```

## Files

- `src/components/ProviderReadinessPanel.tsx`
- `src/components/ProviderReadinessPanel.test.tsx`
- `src/components/settings/ProviderCapabilityCard.tsx`
- `src/components/SettingsPage.test.tsx`
- `src/i18n/locales/en.json`
- `src/i18n/locales/pt.json`
- `src/styles/settings.css`
- this report

## Gates and real results

- Focused repository-runner UI/controller/store/locale slice:

```text
Test Files 9 passed (9)
Tests 309 passed (309)
Duration 30.48s
```

- `bun run typecheck`: PASS, no diagnostics.
- `bun run check`: PASS.

```text
Checked 174 files in 287ms. No fixes applied.
```

- `bun run build`: PASS.

```text
2940 modules transformed
built in 4.04s
```

- `bun run verify:contracts`: PASS; audio-source, provider-registry,
  session-data-movement, endpoint-credential-routing, and speech-span contracts
  are current.
- Exact full `bun run test:local`: PASS.

```text
Test Files 70 passed (70)
Tests 968 passed (968)
Duration 101.60s
```

- Repository-pinned Seeds override:

```text
SEEDS_CLI_ROOT=$PWD/node_modules/@os-eco/seeds-cli bun run verify:fast
Checked 174 files in 287ms. No fixes applied.
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
scanned ~484424 bytes (484.42 KB)
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

Rollback removes the shared fidelity view-model/renderer, its two selected
call sites, localized copy, compact styles, and focused tests. It does not alter
saved settings, secrets, session artifacts, generated contracts, backend
readiness behavior, or the selectable provider set.
