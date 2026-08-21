# Settings UX — Synthesis & Ticket Cut

Judge pass over Design A (navigability, `/tmp/settings-ux-design/design-a.md`), Design B
(informative, `/tmp/settings-ux-design/design-b.md`), and the constraint sheet
(`/tmp/settings-ux-design/constraints.md`). Read-only; every load-bearing claim below was
re-verified against master before this cut. Maintainer intent (verbatim): *"improve the uiux of the
settings modal/options cause we'd like settings to be easily navigable and configurable but also
informative."*

**Citation corrections carried forward (fix before any PR body quotes them):**
- The credential-display boundary is **not** ADR-0028 (that ADR is "Separate capture lifecycle from
  foreground workspace"). The real rules are **ADR-0019** (credential storage), **ADR-0030**
  ("Credential presence alone never means Ready"), **ADR-0033** (`ui_selectable` enforced at content
  start), **ADR-0034** (negative-egress evidence). Design B found this; verified.
- Design A cites `providerVariantContract.test.ts:133` for the `["deepgram"]` pin; the actual pins
  are `src/components/providerRegistryHelpers.test.ts:32` and `src/components/ExpressSetup.test.tsx:224`.
- Registry re-parse: 36 descriptors; selectable = `asr.deepgram` (1 of 23 ASR), **all 7** `llm.*`
  (Design A said 6-7), both `tts.*`, **zero** `realtime_agent.*`.

---

## 1. The recommended direction

The two designs share one diagnosis and split one product cleanly. Shared diagnosis: **settings has
no addressing scheme and no state discipline.** Every external "fix" lands on Modes because
`openSettings()` takes no route (`store/index.ts:2905`, `useSettingsController.tsx:1609` —
`useState("overview")`), and the information users need is already fetched and localized but
mislocated (readiness trapped in the controller, `ui_selectable` absence unexplained, one untargeted
`saveError`).

**The graft: A's addressing, under B's information discipline, with A's IA/search held behind an
appetite gate.**

- From **A**, take the mechanism spine: `openSettings(route?)` + the extraction of the ~50-entry
  route table into a pure `settingsRoutes.ts`, the panel head naming the live variant, and (gated)
  the dual-label rail + jump palette. A's M1 is the single highest value-per-line change in either
  doc: it makes PreflightCard's premise ("fix actions that deep-link INTO settings") true.
- From **B**, take the law that keeps every new pixel honest: the **three-axes model**
  (PLANNED / PREREQUISITES / OBSERVED) and the **tone-allocation rule** — `data-tone="success"` and
  the word *Ready* only from `status === "ready" && !stale` on an actively-probed provider. Plus its
  concrete mechanisms: sibling `aria-describedby` chooser annotations (the 7 anchored radio-name
  tests make in-label chips a hard no), the collapsed deferred roster, cause-specific requirement
  lines, and route-carrying errors.
- **Resolve the overlaps in B's favor where B is cheaper and more honest** (see §2), and **in A's
  favor where A is structural** (routes, panel head, rail).

What this buys against the verbatim ask: *navigable* = every external entry lands on the actual
problem, and (if ratified) the rail speaks goal language and settings become searchable;
*configurable* = the chooser explains each candidate before you commit and warns before it discards;
*informative* = state is displayed under a lintable honesty rule instead of ad-hoc chips, with zero
new provider egress (passive reads only, per ADR-0019/0030/0033/0034).

## 2. Graft decisions (where the designs disagreed)

| Conflict | Decision | Why |
|---|---|---|
| STT n=1 dead end: A drops the lonely radio for prose (breaks `SettingsPage.test.tsx:5327`); B keeps the radio + adds a collapsed roster (0 breaks) | **B's shape** (keep the 1-item radiogroup, add the `<details>` roster), plus A's panel head above it | Cheaper by one forced rewrite; keeps the control consistent when a second ASR becomes selectable; the roster items stay plain text (ADR-0030: deferred providers are non-actionable) |
| Discard warning: A computes a generalized change-preview from a hand-authored ~60-entry field map; B authors 3-5 cause-specific strings for the known cases | **B's authored strings now; A's generalized preview deferred** | A itself flags the map as "a correctness bug in the exact mechanism meant to build trust" (under-reporting discards). The honest general fix is a registry-generated field map — a Rust change, out of this pass |
| Targeted errors: A widens `saveError` to `{message, route?}`; B adds `route` to `humanizeError`'s meta table + renders "Go to field" via the existing `openSettingsControlRoute` (`:1993`) | **Both, as one mechanism** — classification lives in `humanizeError` (B), the widened state shape is A's | Two existing seams joined; same `data-testid="settings-save-error"` node (`SettingsPage.tsx:139`) |
| Chooser annotations: A shows transport/credential/model/readiness per row; B caps at 2 chips + 1 line, siblings only, non-active rows get Axes 1-2 only | **A's content, B's constraints** | B's F2 is decisive: non-active rows read `unchecked` because nobody probed them — rendering that as status manufactures ~30 fake problems |
| Disclosure tiers (basic/Tuning/capability dump) | **Deferred** with the generalized preview | Same missing source of truth (`settings_groups` declares group presence, not field membership) |
| Rail IA + search | **Kept, but ASK-gated** (Ticket 4) | Real copy volume (~18-26 keys, pt lands later), and search-that-jumps-not-filters is an expectation call only the maintainer can make |

Standing constraints honored throughout: no new `.ag-*` recipes (no ADR-0047 amendment, one
contingency in R6); new chrome on new nodes and any recipe-vs-BEM overlap resolved by **deleting**
the BEM properties at the site (seed audio-graph-0922 — the settings barrel is unlayered); the 57
`.settings-field` BEM sites are **not** migrated; ADR-0013's three `conversationMode` write sites are
displayed, never consolidated; i18n add-only-first, en+pt in the same PR.

---

## 3. Tickets

Dependency graph: **T2 → T3**; **T1 → T4b**; T1 and T2 are independent and can land in either order.
T4 (both halves) additionally requires ratification (§5).

```
T2 (S, MUST) ──► T3 (M, MUST*)        T1 (M, MUST) ──► T4b (palette, ASK)
   tone law        chooser+lines         addressing        │
                       *roster copy      + errors       T4a (rail, ASK) — needs T2's law for its chips
                        gated on R1
```

### T1 — Give every setting an address — **M — MUST**

**WHAT.** Widen `openSettings` to `openSettings(route?: {tab: SettingsTab; fieldId?: string;
activate?: boolean})`, park it as `pendingSettingsRoute`, read it as `activeTab`'s initial state,
consume + `focusSettingsField` after first hydration. `apply` is deliberately absent from the
external shape — outside callers navigate, they never mutate (ADR-0013 write sites stay at three).
Extract the ~50 route entries (`useSettingsController.tsx:1676-2272`) into pure
`settings/settingsRoutes.ts` (`credentialRouteForKey`, `providerRouteForStage`, `ROUTE_INDEX`).
Rewire the six bare call sites where a better destination is computable: `PreflightCard.tsx:316`
(per-leg route from data the shell already holds), `App.tsx:1109` (Express "Advanced" carries the
in-progress stage), `NowStrip.tsx:361` / `DemoModeBanner.tsx:55` / `ConversationModeControl.tsx:164`
/ `useKeyboardShortcuts.ts:87` (unchanged behavior, new signature). Add optional `route` to
`humanizeError`'s meta table entries and render a "Go to {field}" action in the footer alert via
`openSettingsControlRoute`.

**FILES.** `src/types/index.ts`, `src/store/index.ts` (~:2905), `src/components/settings/useSettingsController.tsx`
(~:1609, table extraction), new `src/components/settings/settingsRoutes.ts`, `src/components/PreflightCard.tsx`,
`src/App.tsx`, `src/utils/humanizeError.ts`, `src/components/SettingsPage.tsx` (footer alert action).

**ACCEPTANCE (mutation-testable).**
- New controller test: mount with `pendingSettingsRoute = {tab:"llm", fieldId:<cerebras key id>}` →
  active tab is `llm`, the field receives focus, `.settings-landed` applies, and the route is
  consumed (second mount lands on `overview`). Mutating the initial-state read back to
  `useState("overview")` fails this test.
- PreflightCard test: with a fixture missing the LLM key, the Route-row fix action calls
  `openSettings` **with** a `{tab:"llm", fieldId}` argument (asserted on the mock's args).
  `PreflightCard.test.tsx:235`'s `toHaveBeenCalledTimes(1)` still passes.
- Express handoff test: "Advanced" from an in-progress ASR stage calls
  `openSettings(providerRouteForStage("asr", …))`.
- Type-level: the exported external route type has no `apply` member (a `// @ts-expect-error`
  assignment test pins it).
- Error routing test: a 401-classified save error renders a "Go to …" action inside the existing
  `data-testid="settings-save-error"` node; clicking it changes tab + focuses the field. The three
  existing save-error tests (`SettingsPage.test.tsx:489/493/521`) pass unmodified.
- Drift tripwire: a contract test iterates `ROUTE_INDEX` and, for every entry reachable under the
  default fixture, mounts that tab and asserts `document.getElementById(fieldId)` resolves. (Entries
  gated on non-default variants are exercised for the variants the suite already configures;
  residual drift risk for exotic variants is accepted and stated.)

**TEST COST.** 0 forced rewrites in `SettingsPage.test.tsx`; ~8-10 added. **i18n:** ≤5 new keys ×2
(`errors.goToField` + optional per-leg fix labels; default reuse of `controlBar.configure` keeps it
near 0). **RISK:** the route table becomes load-bearing for the shell (A §7.7) — the drift tripwire
above is the mitigation; a wrong entry now fails a test instead of silently misrouting.

### T2 — The tone law, retrofitted to existing chips — **S — MUST**

**WHAT.** Adopt B's three-axes model and enforce its rule on the chips that already exist:
`data-tone="success"` and the word *Ready* render **only** from `status === "ready" && stale ===
false` on an actively-probed provider (Axis 3). Corollaries: `stale` demotes tone (never just an
appended sentence); `automatic_probe_available: false` renders neutral, never warning; data-boundary
chips read as planned claims ("Planned: local only"), never as observed proof (ADR-0034). Audit and
fix the current render sites: `ProductModeSummaryCards.tsx:86-98`, `CredentialsPanel.tsx:188/308`,
`ProviderCapabilityCard.tsx:132-142`. Extract the tone derivation into one helper so later surfaces
(T3 chooser, T4a rail chips) consume it instead of re-deriving.

**FILES.** The three components above + one new small helper module (e.g.
`src/components/settings/readinessTone.ts`).

**ACCEPTANCE (mutation-testable).**
- Helper unit tests, one per rule: presence-only fixture → neutral tone + presence copy (reusing
  existing `providerReadiness.*` keys, never "Ready"); `ready && stale` → demoted tone + cached
  wording; `automatic_probe_available:false` → neutral (real fixtures: `asr.gladia`, Gemini Vertex —
  `ProviderReadinessPanel.test.tsx:535,549` stay green); non-active provider → no Axis-3 chip at
  all. Mutating the helper to emit success on credential presence fails the first test — that IS
  ADR-0030 line 76 as a test.
- Per-site assertions that each of the three audited components routes its tone through the helper
  (a saved-key-but-`status:"error"` fixture must not render success anywhere).
- Zero new markup patterns, **zero new i18n keys**, zero rail/IA changes.

**TEST COST.** 0-2 existing assertions retargeted only if a current site provably violates the rule
(each such change is the point, not collateral); ~6-8 added. **NOTE for ratification (R4):** this
can *remove* green chips users currently see. That is the feature.

### T3 — Chooser that explains, warnings at the point of damage — **M — MUST** (roster copy gated on R1)

**WHAT.** Three pieces, all inside existing panels, zero rail changes.
1. **Panel head** (A's M3 shell): an `.ag-panel-head` **inside** the existing
   `<section className="settings-section">` naming the goal + live variant + one T2-derived chip
   ("Language model · Cerebras [ready]").
2. **Annotated chooser** (B's M1 under B's constraints): each radio row gains a **sibling**
   annotation node linked by `aria-describedby` (first `aria-describedby` uses in the settings tree
   — net a11y gain), never inside the `<label>` (7 anchored radio-name assertions are the tripwire).
   Cap: ≤2 chips + 1 line. Non-active rows show Axes 1-2 only (planned boundary, credential/model
   prerequisites via passive reads); the selected row adds Axis 3 via T2's helper. STT keeps its
   1-item radiogroup and gains the collapsed **deferred roster** (`<details>`, plain-text items,
   non-actionable per ADR-0030/0033): "6 more engines are built but not selectable in this version."
   **Roster copy ships only after R1 is answered.**
3. **Requirement lines** (B's M2): replace the one generic `diarization.unavailable` hint with three
   cause-specific strings naming the blocking setting; add the TTS silent-discard warning (names the
   exact saved values about to be cleared); keep disable-with-reason over hide (`maxSpeakers`
   pattern). The diarization **gate stays** the hardcoded `providerDiarizationSupported`
   (`useSettingsController.tsx:1522`) — the registry's `supports_diarization` is copy only, and
   unlock hints may name **only** `ui_selectable` providers.

CSS: the two-line radio row requires **deleting** `display:flex; align-items:center` at
`settings.css:1192` (`.settings-radio`) — not overriding; the unlayered barrel (seed 0922) makes an
added recipe class silently no-op against them. `.settings-radio` appears in 0 test selectors.
Also fold in the hygiene fix: delete the dead `.settings-field--inline` class at its 6 use sites.

**FILES.** `AsrProviderSettings.tsx`, `LlmProviderSettings.tsx`, `TtsPanel.tsx`, `SttPanel.tsx`,
`GeminiPanel.tsx`, `LoggingSettings.tsx` (dead class), `src/styles/settings.css`,
`src/i18n/locales/en.json` + `pt.json`, T2's tone helper (consume).

**ACCEPTANCE (mutation-testable).**
- The 7 anchored `getByRole("radio", {name:/^…$/i})` queries pass unmodified — moving any annotation
  inside the label fails them (the tripwire is pre-existing, free).
- `SettingsPage.test.tsx:5327` (`radios.length === 1`) passes unmodified (radio kept).
- Roster test: `within(roster).queryAllByRole(...)` finds zero interactive controls; the summary
  string renders the count from the registry (mutating the `ui_selectable` filter changes the count
  and fails it).
- Non-active-row test: a non-selected provider with readiness `unchecked` renders no status chip
  (mutation: rendering raw `status` on all rows fails it).
- `:4626`'s `getByText(/not available for the current provider/i)` is the **one forced rewrite**,
  replaced by three cause-specific assertions each naming the blocking setting.
- TTS discard test: with saved aura voice/speed/speakAloud and provider switched to `none`, the
  warning renders the three current values verbatim; switching back removes it.
- Anti-regression pin for F3: a test asserts `local_whisper` does **not** enable provider-labels
  diarization (this makes B's W8 — "implementer reads the registry as the gate" — a red build
  instead of a silent behavior change).
- All ~20 `.closest(".settings-section")` scopes pass unmodified (panel head is inside the section).
- Review checklist item (not a unit test): `.settings-radio` flex properties deleted, not
  overridden; verify computed layout in the running app.

**TEST COST.** 1 forced rewrite (`:4626`); ~15-17 added. **i18n:** Tier-1 budget = **15 new en keys
→ 30 strings (+3.0% on 495 `settings.*` keys)**, ~35 existing keys reused
(`providerReadiness.status.*`, `credentialSource.*`, `dataBoundary.*`, `modelReadiness.notDownloaded`,
`errors.*`); `diarization.unavailable` marked deprecated, not deleted (add-only-first). Chip strings
get a **pt-first length review with a ≤18-char budget** before en copy freezes (`.ag-chip` is
`nowrap`; "Sem verificação automática" is 28 chars — B's W4). Tier 2 (porting
`providerRegistryHelpers.ts`'s hardcoded English, 19 keys) is explicitly **not** in this ticket.

### T4 — Goal vocabulary & find-a-setting — **L combined — ASK-MAINTAINER** (two severable halves)

**T4a — Rail dual-labels + live value + one regroup (M, needs R2).** Two lines inside each
`<button role="tab">`: goal phrase + engine phrase + live provider + one T2-derived chip. Rail
**ids unchanged** (all ~50 routes keep resolving); group rename "Providers & Models" → "Conversation
pipeline"; Models section moves General → Credentials ("Setup health") — one route-table edit
(`{tab:"general", fieldId:"settings-models-section"}` → `{tab:"credentials", …}`). Below 720px the
goal line hides; the horizontal strip stays single-line. Copy rule (hard): no cross-row token
collisions with the 8 reserved test tokens (`modes`, `general`, `logging`, `language model`,
`speech-to-text`, `text-to-speech`, `realtime agent`, `credentials`). The dedicated `models` tab
(A's 3.5b) is **deferred** — it breaks the rail adjacency test and earns its keep only when model
management grows.
- FILES: `settingsRailConfig.ts`, `settingsRail.tsx`, `settings.css` (new nodes only),
  `settingsRoutes.ts`, locales, `SettingsPage.test.tsx` (2 retargets), `settingsRail.test.tsx`.
- ACCEPTANCE: all 89 `goToTab` regexes pass unmodified (the collision rule is enforced by the suite
  itself — a violating copy change fails `getByRole` on multiple matches); adjacency test
  (`~:3799`) untouched; forced rewrites exactly `:3849` + `:4883` (one-line retargets to
  `goToTab(/credentials/i)`); rail chips route through T2's helper (presence-only → no green);
  `settingsRail.test.tsx` re-verified (cheap, fully mocked). **i18n:** ~18 keys ×2. **TEST COST:**
  2 forced rewrites, ~6 added.
- Known compromise to state up front (A §7.1): below 720px users keep today's engine-first
  vocabulary.

**T4b — "Find a setting" jump palette (M, needs R3 + T1).** Manifest-backed
(`ROUTE_INDEX` + 37 `settings.fields.*` labels + 36 registry descriptors), `⌘F` or `/`,
combobox+listbox, Enter jumps via T1's route. Qualifier mandatory ("API Key — Deepgram · …" — 7
providers share `settings.fields.apiKey`). **Jump, not filter** — a live cross-tab filter requires
mounting all 8 panels, duplicating ~50 DOM ids and defeating `getElementById` focus; scope cut
stated plainly to the maintainer in R3.
- ACCEPTANCE: closed palette renders zero nodes (a snapshot of role counts before/after proves none
  of the 210+ existing role queries can see it); Enter on a result calls the T1 route with the
  manifest's `{tab, fieldId}` (mock-asserted); manifest↔`ROUTE_INDEX` drift is a contract test
  (every manifest `fieldId` must exist in `ROUTE_INDEX`, which T1's tripwire already resolves
  against the DOM). **i18n:** ~8 keys ×2. **TEST COST:** 0 forced, ~6 added.

---

## 4. MUST vs ASK-MAINTAINER

**MUST (clear wins, low risk, no appetite question):**
- **T1** — external deep-links + targeted errors. Makes PreflightCard's premise true. 0 forced test
  rewrites, ≤5 keys.
- **T2** — the tone law on existing chips. Zero i18n, zero IA; it is ADR-0030 turned into tests.
- **T3** — annotated chooser + requirement lines (1 forced rewrite, 15 keys), *except* the deferred
  roster's copy, which waits on R1.

**ASK-MAINTAINER (appetite decisions — the shape is designed, the go/no-go is yours):**
- **Rail IA restructure depth (T4a):** two-line goal-vocabulary rows + group rename + Models→Setup
  health. Cost is real but small (2 rewrites, ~18 keys); the risk is taste + pt-BR landing later +
  the 720px fallback keeping old vocabulary. Options: full T4a / rename+move only / nothing.
- **Search-in-settings (T4b):** a jump palette, explicitly not a filter. If "type and the field
  appears right here" is the expectation, this will disappoint — say no rather than half-yes.
- **Copy volume ceiling:** MUST tickets total ≤20 new keys; with T4 it's ~46 (×2 locales). Where do
  you want the line, given add-only-first and the pt translation pass trailing en?

## 5. Ratify before build (mirroring how SHELL-R8 was ratified)

1. **R1 — Publishing the MVP scoping decision (blocks T3's roster).** "6 more engines are built but
   not selectable in this version" makes `ui_selectable:false` user-visible. If Whisper *should* be
   selectable, the right fix is a registry change and the roster shrinks to nothing. Ratify the
   framing ("built, deliberately not offered; saved configs keep working" — never "coming soon"), or
   flip the registry instead.
2. **R2 — Rail IA depth** (blocks T4a). Approve/trim per §4.
3. **R3 — Search palette, jump-not-filter** (blocks T4b).
4. **R4 — The tone law will remove green** (T2). Mode cards / credential rows that today show
   success from presence will demote to neutral/warning. It will look like a regression in
   screenshots; it is the ADR-0030 discipline. Ratify so the review doesn't relitigate it.
5. **R5 — ADR citations.** Credential rules = ADR-0019/0030/0033/0034; ADR-0028 is capture
   lifecycle. All four tickets must cite correctly in PR bodies.
6. **R6 — Contingent ADR-0047 amendment.** Chip tones are tint-only; a forced-colors/a11y pass may
   demand a border variant on `.ag-chip` — that is a recipe change and needs an ADR-0047 amendment.
   Pre-authorize the amendment path or park forced-colors explicitly; don't let it appear mid-PR.
7. **Decision taken, veto if you disagree:** STT keeps its one-item radiogroup (consistency +
   `:5327` untouched + n grows later) rather than A's prose replacement.

## 6. Explicitly out of scope (named so nobody "helpfully" does them)

- **Field-level BEM→recipe migration** (57 `.settings-field` sites) — separate mechanical lift;
  0922's unlayered-barrel trap makes piecemeal mixing actively dangerous.
- **Readiness into the store / `credentialPresence` dedup** (global array vs the controller's second
  fetched map). Structural state fix; shell and rail can still disagree within a session until it
  lands. Recommend seeding it as the follow-up right after T1 — T1's `settingsRoutes.ts` is where
  the shell-side consumers will want it.
- **Generalized change-preview + disclosure tiers** — blocked on a registry-generated
  field→group/variant map (Rust change). Shipping them from a hand-authored map risks under-reporting
  discards — a lie in the trust mechanism.
- **ADR-0013 write-site consolidation** (3 native-realtime writers) — display the state, never merge
  the writers in a UI pass.
- **Live cross-tab filter** and **A's 3.5b `models` tab** — priced, deferred.
- **Registry `supports_diarization` as the gate** — copy only; T3 adds the regression pin.

## 7. Sequencing

1. **T1** (addressing + targeted errors) — shippable alone, zero copy, largest user-visible win.
2. **T2** (tone law) — small, independent; land before any new chip renders.
3. **T3** (chooser + requirement lines) — after T2; roster component held until R1 answers.
4. **T4a / T4b** — each independently shippable and independently droppable, after R2/R3; T4b after T1.

Aggregate bill if everything ships: **3 forced test rewrites** (`:4626`, `:3849`, `:4883`) against
the 6822-line suite, ~40 tests added, ~46 en keys ×2 locales (15 if only MUSTs), zero new recipes,
zero new provider egress, rail ids and all roles unchanged.
