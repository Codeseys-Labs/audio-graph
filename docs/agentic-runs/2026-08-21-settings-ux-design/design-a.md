# Design A — Navigability & Configurability

Settings modal/options redesign for `audio-graph`, post-SHELL-R8 (2fd7d0e).
Read-only design pass. Every code claim below is file:line-verified against
`master` on 2026-08-21.

Maintainer intent (verbatim): *"improve the uiux of the settings modal/options
cause we'd like settings to be easily navigable and configurable but also
informative."*

This document owns the **navigability + configurability** angle: getting the
user to the right control, and letting them change it without a dead end.

---

## 0. Two users, one surface

| | **Knows what they want** ("change my LLM to Cerebras", "where's my API key") | **Doesn't know** ("dictation isn't working") |
|---|---|---|
| Needs | A destination, fast, by name | To be told what the parts are and which one is broken |
| Fails today because | 8 rail labels are *stage* names, not the words in their head; nothing is searchable; every external "Fix" lands on Modes | Depth is hidden inside radiogroups; readiness lives on a different tab than the thing that's broken; errors are one untargeted banner |

Both failures share one root cause: **there is no addressing scheme for a
setting.** The modal has a perfectly good internal one (`SettingsControlRoute`)
that nothing outside the modal can use and no user can type into.

---

## 1. Diagnosis (code-verified, including one correction to the constraint sheet)

### 1.1 The rail is not the problem; the addressing is

`settingsRailConfig.ts` is a clean 8-item / 2-group source of truth, rendered by
a 72-line presentational `settingsRail.tsx`. That is not where the pain is.

The pain, precisely:

1. **`<SettingsPage/>` takes zero props** (`App.tsx:1096`) and `activeTab` is
   `useState<SettingsTab>("overview")` (`useSettingsController.tsx:1609`).
   Every external entry point — `PreflightCard.tsx:316` (Route row "Configure"),
   `NowStrip.tsx:361` (gear), `App.tsx:1109` (Express Setup "Advanced"),
   `DemoModeBanner.tsx:55`, `ConversationModeControl.tsx:164`,
   `useKeyboardShortcuts.ts:87` (⌘,) — calls the same bare
   `openSettings()` (`store/index.ts:2905`), which only does
   `set({settingsOpen:true})` + 3 fetches. **Six call sites, one destination:
   Modes.**
2. **The route machinery already exists and is good.**
   `type SettingsControlRoute = { tab; fieldId; activate?; apply? }`
   (`useSettingsController.tsx:238`), applied by `openSettingsControlRoute`
   (`:1993`) → `focusSettingsField` (`:1633`), which scrolls, focuses, and adds a
   1.5s `.settings-landed` pulse that is already reduced-motion-gated
   (`settings.css:1418`). There are **~50 hand-authored route entries** in the
   controller (`:1676`–`:2272`) mapping (provider variant × concern) → `{tab,
   fieldId}`. All of it is trapped inside the controller closure.
3. **Zero search.** There is no way to find a setting by name. The app already
   ships a good search-input pattern (`AudioSourceSelector.tsx:653-676`,
   `#audio-source-search`) — it just was never applied to settings.

### 1.2 Correction: ASR's "7 sub-forms" are not reachable from the picker

The constraint sheet describes STT as "7 provider sub-forms … one visible at a
time via radiogroup." The sub-*forms* exist, but the *picker* does not offer
them. `ASR_PROVIDER_OPTIONS` is `selectableProviderOptionsForStage("asr", …)`
(`useSettingsController.tsx:118`), which filters on `descriptor.ui_selectable`
(`providerRegistryHelpers.ts:72`). In the shipped generated registry
(`src/generated/providerRegistry.ts`, 36 descriptors) **exactly one ASR
descriptor is `ui_selectable`: `asr.deepgram`**. `asr.local_whisper` is
`ui_selectable:false`. This is frozen by a contract test that says so in
prose and asserts it:

```
providerVariantContract.test.ts:133
expect(ASR_PROVIDER_OPTIONS.map(o => o.value)).toEqual(["deepgram"]);
```

and by `SettingsPage.test.tsx:5327` (`radios.length === 1`).

Consequences for this design:

- **The STT tab today renders a one-item radiogroup.** That is an a11y smell and
  a UX dead end ("why is there a radio with one option, and where did Whisper
  go?"). It is also the single loudest unexplained absence in the product: a
  local-first app whose local ASR is not offerable.
- **LLM is the real live depth**: 6 of 7 LLM variants are `ui_selectable`
  (`local_llama`, `api`, `cerebras`, `sambanova`, `openrouter`, `aws_bedrock`,
  `mistralrs` — all true), in a 1196-line file. Both realtime agents are
  `ui_selectable:false`, so the "Realtime agent" tab is likewise a
  currently-unpickable surface.
- Therefore any variant-switcher mechanism must be **correct at n=1 and n=7**,
  and must explain absence (`ui_selectable:false`) rather than silently omit.
  Absence-without-explanation is the largest dead end in the product right now.

### 1.3 Mixed commit model (nobody tells the user)

- Footer **Save Settings** commits the reducer draft via a dirty fingerprint
  (`useSettingsController.tsx:1582-1598`, `store/index.ts:2928`).
- Credential rows commit **immediately** — `invoke("save_credential_cmd")` at
  `useSettingsController.tsx:2615`, delete at `:2569`.
- Log level commits **immediately** (`set_log_level` / `set_logging_config`).

So one panel stages and another applies, with identical-looking controls. This
constrains any "guided flow": a wizard that batches everything to a final Save
would silently change the commit semantics of credentials. Design must keep
per-step commit semantics visible instead of hiding them.

### 1.4 Errors have no address

One global `saveError: string | null` (`:1576`) rendered once in the footer
(`SettingsPage.tsx:136-156`). A validation failure on any of ~25 field-sets
appears as a generic banner on whatever tab happens to be open.

---

## 2. Design principles

1. **Every setting has an address.** One route type, usable from inside the
   modal, from the shell, from search, and from an error.
2. **Goal words in front, engine words behind — in the same accessible name.**
   Users who think "dictation" and users who think "STT" both hit.
3. **Show the current value where you show the door.** A nav row that names its
   live provider and readiness answers most "informative" asks for free from
   data the modal already fetches.
4. **Absence must be explained.** `ui_selectable:false` is a fact the UI has; a
   missing option is a bug report waiting to happen.
5. **Never destroy silently.** Any switch that discards field values previews
   what it discards.
6. **New chrome on new nodes.** Seed audio-graph-0922: the settings CSS barrel
   is unlayered, so `.ag-*` recipes added *alongside* existing BEM on the *same
   element* silently no-op for overlapping properties. New surfaces (search,
   panel head, chooser) have no competing BEM rule, so they are free. This
   design deliberately avoids retrofitting `.ag-field` onto the 57 remaining
   `.settings-field` sites — that migration is a separate lift and is *not* part
   of this pass.
7. **No new recipes.** Everything below uses `.ag-card[data-elevation]`,
   `.ag-chip[data-tone]`, `.ag-label`, `.ag-btn-micro`, `.ag-panel-head`,
   `.ag-field[data-layout="row"]` as already defined
   (`styles.css:878-1060`) plus Tailwind utilities, exactly as
   `PreflightCard.tsx` does. **No ADR-0047 amendment required.**

---

## 3. IA proposal

### 3.1 Before

```
PROVIDERS & MODELS                      ← engine vocabulary
  Modes                                 ← 4 mode cards; also the forced landing
  Speech-to-Text                        ← 1-item radiogroup (!) + 3 diarization fields + 7 latent sub-forms
  Language Model                        ← 6-7 selectable variants, 1196 lines, no sub-nav
  Text-to-Speech                        ← 2 variants; switching away discards 4 fields silently
  Realtime agent (native S2S)           ← both providers ui_selectable:false
  Credentials & readiness               ← the "is it working" pivot, a tab away from every cause
APP
  General                               ← theme + language + capture device + MODEL DOWNLOADS (3 unrelated concerns)
  Logging
```

Structural facts: no search; no external deep-link; model downloads
(`#settings-models-section`) are filed under App▸General even though they are a
*provider prerequisite* and are governed by a single global download slot
(`store/index.ts:2853` `isDownloading`).

### 3.2 After (recommended)

Rail item ids are **unchanged** — `overview | stt | llm | tts | gemini |
credentials | general | logging` — so all ~50 existing route entries keep
resolving. What changes: a search field above the rail, two-line rail rows
(goal phrase + engine phrase + live value), one group rename, one moved
section, and one optional new tab.

```
┌──────────────────────────────────────┐
│ 🔍  Find a setting…             ⌘F   │   NEW — mechanism M2
└──────────────────────────────────────┘

START HERE
  Get started                              ← id: overview
  Modes · Hybrid                    [●]
  ──
  Setup health                             ← id: credentials
  Credentials & readiness      [2 issues]

CONVERSATION PIPELINE                      ← group label rename (was "Providers & Models")
  Turn speech into text                    ← id: stt
  Speech-to-text · Deepgram     [●ready]
  ──
  Write notes & answer                     ← id: llm
  Language model · Cerebras     [needs key]
  ──
  Speak responses aloud                    ← id: tts
  Text-to-speech · Off               [—]
  ──
  Live voice conversation                  ← id: gemini
  Realtime agent (native S2S)  [not in MVP]

APP
  Appearance & audio input                 ← id: general
  General
  ──
  Diagnostics & privacy                    ← id: logging
  Logging
```

Optional phase-3 addition (see §6 pricing):

```
START HERE
  …
  Model files                              ← NEW id: models
  Downloads · 1 downloading      [●]
```

### 3.3 The dual-label rule (this is what makes the IA change affordable)

Each rail button renders **two lines inside one `<button role="tab">`**: line 1
is the goal phrase, line 2 is the engine phrase + live value. Because the
accessible name of a button is the concatenation of its text content, the
accessible name *still contains the old label*:

```
accessible name: "Turn speech into text  Speech-to-text · Deepgram  ready"
```

All 90 `goToTab(/…/i)` calls use **unanchored substring regexes**
(`/language model/i` ×39, `/speech-to-text/i` ×27, `/general/i` ×10,
`/realtime agent/i` ×8, `/text-to-speech/i` ×5) — verified. They keep matching.
So does the adjacency test's `/modes/i` and its
`getByRole("tabpanel", {name:/modes/i})` (the panel is `aria-labelledby` the
tab, so its name tracks the tab's).

**Hard rule for the copy pass — no cross-row token collisions.** A goal phrase
must not contain a token that another row's regex targets, or `getByRole` throws
on multiple matches. Concretely, no other row may contain: `modes`, `general`,
`logging`, `language model`, `speech-to-text`, `text-to-speech`,
`realtime agent`, `credentials`. The phrasing above satisfies this — e.g. the
TTS row says "Text-to-speech", which does not match `/speech-to-text/i`
(different token order), exactly as today.

**Verified low collateral:** `SettingsPage.test.tsx` has only **10** unscoped
`screen.getByText(` calls and none of them match a provider name, so putting
live provider names in the rail does not create ambiguous text queries.

### 3.4 Why goal-phrase-first, and where I stopped

The assignment says regroup around user goals, not provider internals. A pure
goal rail ("Dictation", "Note writing", "Voice replies") would delete the engine
vocabulary that power users and every existing doc/test/issue uses, cost 90 test
regex rewrites, and make the search index the *only* way an expert finds
anything. The dual label buys ~90% of the goal-orientation win for ~5% of the
cost, and the engine phrase doubles as the live-value slot.

I also deliberately did **not** promote provider variants into rail tabs. Seven
sibling "Cerebras / OpenRouter / Bedrock…" rows would put six irrelevant rows in
front of every user permanently. Variant switching is a *within-panel* act
(M3), not a navigation act.

### 3.5 One real regroup: model files leave General

`General` currently bundles theme + language + `<AudioSettings>` (capture
device) + `<CredentialsManager>` (model files + an obsolete diagnostics block).
Model files are a **provider prerequisite** — they belong with "will this run",
next to credentials and readiness, not with theme. Two options:

- **3.5a (recommended, cheap):** move the Models section into the
  `credentials` panel, renaming that tab's goal line "Setup health". One
  route-table edit: `{tab:"general", fieldId:"settings-models-section"}` →
  `{tab:"credentials", …}` (`useSettingsController.tsx:2205`). Breaks exactly
  **2** tests (`SettingsPage.test.tsx:3849` section-headings, `:4883`
  "CredentialsManager renders the Models section header", which does
  `goToTab(/general/i)`); both are one-line retargets to
  `goToTab(/credentials/i)`.
- **3.5b (phase 3, moderate):** a dedicated `models` tab. Better home for the
  global download slot (§5 item 10) and for future local-model management, but
  it adds a `SettingsTab` id → breaks the rail adjacency test
  (`:3799`, `End`→Logging still holds, but `ArrowDown` from Overview would land
  on the new row if inserted before `stt`; insert it in the START HERE group
  *after* `credentials` and `ArrowDown`-from-Overview changes too, since the
  rail is one flat roving-tabindex sequence). Priced at: 1 adjacency test
  rewrite + `settingsRail.test.tsx` re-verify (cheap, fully mocked) + 2 i18n
  keys ×2 locales + the same 2 retargets as 3.5a.

Recommendation: ship 3.5a with this pass; keep 3.5b as a follow-up once model
management grows.

### 3.6 What does not change

- Tab **ids** and the `SettingsTab` union (except optional 3.5b).
- One-panel-at-a-time mounting (`SettingsPage.tsx:111-121`) and the hidden-stub
  tabpanels — the single-scroller layout invariant
  (`settings.css:94-113`, "min-height:0 is load-bearing") stays untouched.
- The 720px rail→horizontal breakpoint (`settings.css:1428`). Two-line rail rows
  need one addition there: below 720px, render **line 2 only** (engine phrase),
  which keeps the horizontal strip single-line and keeps the accessible name
  containing the test-visible token. Sketch: `.settings-tab__goal {display:none}`
  inside the existing media query.
- Tablist/radiogroup roles — no role churn (that's the expensive class of change
  per the constraint sheet's cost model (d)).
- `ADR-0013`'s `conversationMode`/`converseEngine` write sites. This design
  *displays* the cross-surface disagreement (§5 item 9) but does not consolidate
  the three writers — that's an ADR decision, out of scope.
- No new provider egress. Everything readable is already fetched:
  `get_provider_readiness_cmd`, `load_credential_presence_cmd`, `list_available_models`
  (ADR-0028 respected — display only).

---

## 4. Three interaction mechanisms

### M1 — `openSettings(route?)`: make every external "Fix" land on the actual problem

**The change.** Widen one store action and read one field.

```ts
// types/index.ts (~:3003)
export type SettingsRoute = { tab: SettingsTab; fieldId?: string; activate?: boolean };
openSettings: (route?: SettingsRoute) => void;
pendingSettingsRoute: SettingsRoute | null;

// store/index.ts:2905
openSettings: (route) => {
  set({ settingsOpen: true, pendingSettingsRoute: route ?? null });
  const { fetchSettings, fetchModels, fetchModelStatus } = get();
  fetchSettings(); fetchModels(); fetchModelStatus();
},
consumeSettingsRoute: () => { const r = get().pendingSettingsRoute; set({ pendingSettingsRoute: null }); return r; },

// useSettingsController.tsx:1609
const [activeTab, setActiveTab] = useState<SettingsTab>(
  () => useAudioGraphStore.getState().pendingSettingsRoute?.tab ?? "overview",
);
// then, after the first hydration effect, consume + focusSettingsField(route.fieldId, route.activate)
```

Note `apply?: () => void` is **deliberately dropped** from the external shape.
External callers may navigate; they may not mutate settings as a side effect of
navigating. That keeps ADR-0013's write sites at three, not four.

**Extract the route table.** Move the ~50 `{tab, fieldId}` entries at
`useSettingsController.tsx:1676-2272` into a new pure module
`settings/settingsRoutes.ts`:

```ts
export function credentialRouteForKey(key: string, settings: AppSettings): SettingsRoute | null
export function providerRouteForStage(stage: ProviderStage, settings: AppSettings): SettingsRoute
export function modelRouteForStage(stage: ProviderStage, settings: AppSettings): SettingsRoute | null
export const ROUTE_INDEX: SettingsRouteEntry[]   // consumed by M2
```

Pure functions over `AppSettings` — no hooks, no context — so `PreflightCard`,
`NowStrip`, `ExpressSetup` and the store can all call them. The controller keeps
its richer in-modal variants (which may carry `apply`).

**Sketch — PreflightCard Route row today vs after:**

```
BEFORE
  Route   ✗  planned: — → —                 [ Configure ]  → lands on Modes
                                                              (user must guess
                                                               which of 4 tabs)
AFTER
  Route   ✗  needs a language-model key      [ Fix: Cerebras key ]
                                                              → opens on
                                                                "Write notes &
                                                                answer", focuses
                                                                #llm-cerebras-api-key,
                                                                landed-pulse
```

`hasConfiguredDurableNotesRoute` already computes *whether* the route is
configured; the missing half is *which leg* failed. `describePlannedRoute`
already resolves the per-leg providers, and `credentialRouteForKey` already maps
a missing key to a field — so the "which leg" answer is a small pure function
over data the shell already holds (`settings`, `credentialPresence`,
`modelStatus`), no new invoke.

**Also fixed for free:** Express Setup's "Advanced" handoff
(`App.tsx:1109`) becomes `openSettings(providerRouteForStage("asr", settings))`
— the user's in-progress concern survives the modal swap.

**Cost.** `SettingsPage.test.tsx`: **0 breaks** (still zero-prop; route defaults
to null → `"overview"`). `PreflightCard.test.tsx:235`'s
`toHaveBeenCalledTimes(1)` survives an argument. New tests: 1 controller test
(initial tab from route), 2-3 PreflightCard/NowStrip route assertions. i18n:
**0 new keys** if fix-action labels reuse `controlBar.configure`; ~3 if we want
per-leg labels. **This is the highest value-per-line change in the whole
design** and it is shippable alone.

---

### M2 — "Find a setting": a manifest-backed jump palette, not a DOM filter

**Sketch:**

```
┌─ Settings ─────────────────────────────────────────────────┐
│ 🔍 aws                                              ✕      │
│ ┌────────────────────────────────────────────────────────┐ │
│ │ WRITE NOTES & ANSWER (Language model)                  │ │
│ │   AWS Profile — Bedrock                    profile     │ │
│ │   Access Key ID — Bedrock              ● saved         │ │
│ │   Region — Bedrock                                     │ │
│ │ SETUP HEALTH (Credentials)                             │ │
│ │   aws_access_key                       ● saved · error │ │
│ │ TURN SPEECH INTO TEXT (Speech-to-text)                 │ │
│ │   Region — AWS Transcribe        ⚠ not selectable      │ │
│ └────────────────────────────────────────────────────────┘ │
│  ↑↓ move · ⏎ jump · esc close                              │
└────────────────────────────────────────────────────────────┘
```

**Data.** A hand-authored manifest `settings/settingsSearchIndex.ts`, seeded
from three sources that already exist:

| Source | Gives | Count today |
|---|---|---|
| `ROUTE_INDEX` (M1's extracted table) | `{tab, fieldId}` per (variant × concern) | ~50 |
| `settings.fields.*` i18n keys | human labels | 37 |
| `GENERATED_PROVIDER_REGISTRY` | `display_name`, `credential_keys`, `stage`, `ui_selectable`, `transport` | 36 descriptors |

Entry shape: `{ id, tab, fieldId, labelKey, qualifier?, keywords[], selectable? }`.

**The qualifier is mandatory, not decorative.** `settings.fields.apiKey` = "API
Key" is shared by 7 providers, and `.model` / `.endpoint` / `.region` likewise —
a raw label index returns seven indistinguishable "API Key" rows. The qualifier
(`descriptor.display_name` + rail goal phrase) comes from the registry, i.e. no
new i18n. Result rows read "API Key — Deepgram · Turn speech into text".

**Explicitly not built:** a live filter that hides non-matching fields across all
tabs. That would require mounting all 8 panels simultaneously (today exactly one
mounts, `SettingsPage.tsx:111`), which breaks the tabpanel contract, duplicates
~50 DOM ids (`asr-api-key`, `log-level-select`…), and defeats `focusSettingsField`
(`document.getElementById`). **Search jumps; it does not filter in place.** This
is a scope cut, stated plainly.

**Keyboard.** `⌘/Ctrl+F` while the modal has focus, or `/` when focus is not in a
text field. `role="combobox"` + `role="listbox"`/`option` popup,
`aria-activedescendant`, Enter = jump, Esc = close and restore focus.

**Test-safety property:** the palette is **closed by default and renders zero
nodes when closed**, so none of the 210+ existing role queries (including
`option` ×12 for real comboboxes) can see it. New tests are additive and scoped
with `within(palette)`.

**Cost.** ~8 new i18n keys ×2 locales (`settings.search.label/placeholder/
noResults/resultCount/hint/clear/open/group`). Zero existing-test breaks.
Manifest maintenance is the real cost — see Weaknesses §7.2.

---

### M3 — Provider switcher with a change-preview, plus a 3-tier disclosure

This replaces the bare radiogroup at the top of each stage panel
(`AsrProviderSettings.tsx:264-287`, the LLM analogue) with a **panel head that
names the live variant** and a **chooser that explains each candidate**.

**Sketch — collapsed (default) state of the LLM panel:**

```
┌ ag-panel-head ───────────────────────────────────────────────┐
│ Write notes & answer                                         │
│ Language model · Cerebras          [● ready]  [Change ▾]     │
└──────────────────────────────────────────────────────────────┘
  Cerebras API Key       ●●●●●●●●        [Test]   ← basic fields
  Model                  llama-3.3-70b ▾          ← model_catalog

  ▸ Tuning (max tokens, temperature, streaming prefill)   ← `advanced` fields
  ▸ Capability details (30 registry rows)                 ← existing dump, unchanged
```

**Sketch — chooser expanded (`Change ▾`), still one `radiogroup`:**

```
  ○ Local llama.cpp        on-device   no key needed   model file required (2.1 GB) [Download]
  ● Cerebras               cloud       cerebras_api_key ● saved            ready
  ○ OpenRouter             cloud       openrouter_api_key ✗ missing        needs key
  ○ AWS Bedrock            cloud       aws_* ● profile     unverifiable outside Settings
  ○ Custom OpenAI-compat.  cloud       endpoint + key
  ──────────────────────────────────────────────────────────────
  Switching to OpenRouter will keep: nothing from Cerebras.
  It needs: an OpenRouter API key (not saved yet) → you'll be taken to that field.
```

**Sketch — the n=1 case (STT today), which is the current dead end:**

```
┌ ag-panel-head ───────────────────────────────────────────────┐
│ Turn speech into text                                        │
│ Speech-to-text · Deepgram          [● ready]                 │
└──────────────────────────────────────────────────────────────┘
  Deepgram is the only speech-to-text provider available in this build.
  Local Whisper, AWS Transcribe, AssemblyAI, OpenAI Realtime and Sherpa-ONNX
  are implemented but not selectable yet.            [What does that mean?]
```

That last line is `ui_selectable:false` + `status:"implemented"` rendered as
prose. It is the highest-information, lowest-cost sentence in this whole design:
today the user just sees a lonely radio and five absent providers.

**The change preview** (fixes the silent-discard class, §5 items 1/8). Each
variant owns a known field set in the reducer; switching computes
`fieldsOnlyMeaningfulForCurrentVariant` and lists their current values before
committing. For TTS: *"Switching to Off discards voice (aura-asteria-en), speed
(1.0), speak-aloud (on) when you Save."* For Overview's "Use this mode":
*"Applies ASR=Deepgram, LLM=Cerebras — replaces your current picks."*

**Semantics preserved:** the chooser stays `role="radiogroup"` with real
`role="radio"` children, so the 47 radio queries and the 4 radiogroup queries
keep working; only presentation and surrounding text change. Deferred providers
stay **out** of the radiogroup (per `providerVariantContract.test.ts`) and appear
in the explanatory prose instead — that keeps `ui_selectable` as the single
action boundary (ADR-0033) while ending the unexplained absence.

**Disclosure tiers.** The registry has `settings_groups:
("basic"|"model_catalog"|"health"|"advanced")[]` per descriptor
(`types/index.ts:1087`) — and grep confirms **zero UI consumers**; it exists only
as generated data. It declares which groups a provider *has*, not which field
belongs to which group, so the field→tier map must be hand-authored in the
frontend (~60 field ids). Use `settings_groups` as the *presence* check (don't
render a "Tuning" disclosure for a provider whose descriptor has no `advanced`
group) and the hand map for membership. Primitive: the existing
`AdvancedSettingsDisclosure` (`<details>`/`<summary>`, `role="group"` +
`aria-labelledby`) — no new component, no new recipe.

**Cost.** Breaks `SettingsPage.test.tsx:5327` (`radios.length === 1`) by design —
the n=1 case stops rendering a radiogroup. Rewrite it to assert the
only-provider prose + the absent-provider list; that is a *better* test.
The ~20 `.closest(".settings-section")` scopes survive if the panel head is
added *inside* the existing `<section className="settings-section">` rather than
replacing it — do that. i18n: ~12 new keys ×2 locales.

---

## 5. Dead-end elimination: the 10 non-obvious effects, each with an owner

| # | Dead end today | Fix | Mechanism |
|---|---|---|---|
| 1 | TTS switch silently discards voice/speed/speakAloud | change preview lists exact values | M3 |
| 2 | Diarization option disabled, cause not shown at the disable point | attach cause + "switch to a provider that supports it" link | M3 (+ route) |
| 3 | ASR choice transitively gates diarization | panel head states the dependency: "Diarization comes from Speech-to-text · Deepgram" | M3 |
| 4 | `ui_selectable:false` invisible in the default view | n=1 / absence prose, promoted out of "Show advanced" | M3 |
| 5 | Credential presence ≠ readiness, and they live on different tabs | readiness chip in the rail row + credential state inline in the chooser | §3.2 + M3 |
| 6 | AWS profile creds unverifiable outside Settings | say so, verbatim, in the chooser row ("unverifiable outside Settings") instead of a comment in `App.tsx` | M3 |
| 7 | OpenRouter preset picked ≠ applied | show both `openrouterAcceleratorPreset` vs `…Applied…` in the panel head as a chip pair | M3 |
| 8 | Overview "Use this mode" overwrites provider picks | change preview before apply | M3 |
| 9 | Native-realtime writable from 3 surfaces, none showing the others | Gemini panel head shows current `conversationMode`/`converseEngine` and names the other writers | display-only (ADR-0013 untouched) |
| 10 | Global download slot disables every other Download with no reason | store the requested filename (`downloadModel` sets `isDownloading:true` but never records *which*, `store/index.ts:2863`); blocked rows read "Downloading {name}…" | 1-line store add + M3/3.5 |
| — | One untargeted `saveError` | widen to `{ message, route? }`; footer banner gains "Go to field" calling the in-modal route. The alert node, its `role="alert"` and `data-testid="settings-save-error"` stay identical, so the existing test passes | M1 |
| — | Dead `.settings-field--inline` (6 sites, 0 CSS rules) | delete the class at all 6 sites, or point them at `.ag-field[data-layout="row"]` **and delete the overlapping BEM properties at the same site** (0922 trap) | hygiene, ship with M3 |

---

## 6. Test impact — what breaks, and why it's worth it

Priced against `SettingsPage.test.tsx` (6822 lines, 120 cases, 90 `goToTab`
calls, zero `getByTestId`) and the cheap `settingsRail.test.tsx` (144 lines,
fully mocked).

| Change | Breaks | Count | Worth it because |
|---|---|---|---|
| M1 route param | nothing in `SettingsPage.test.tsx` | **0** | PreflightCard's entire premise ("fix actions that deep-link INTO settings") is currently false. This makes it true. |
| Dual rail labels + live value | nothing (unanchored regexes verified) | **0** | Goal vocabulary + live readiness in the nav, at zero test cost. Re-verify via `settingsRail.test.tsx` (cheap). |
| Group rename "Providers & Models" → "Conversation pipeline" | nothing (`goToTab` targets tabs, not group labels; the group `<p>` is `role="presentation"`) | **0** | |
| Models section: General → Setup health (3.5a) | `:3849` section-headings, `:4883` Models-under-General | **2** | Model files are a provider prerequisite; filing them under theme/language is the single most arbitrary placement in the IA. Both fixes are one-line `goToTab` retargets. |
| M3 n=1 STT: drop the 1-item radiogroup | `:5327` `radios.length === 1` | **1** | A one-option radiogroup is an a11y smell *and* the loudest unexplained absence in the app. The replacement test asserts something users care about. |
| M3 panel head inside existing `.settings-section` | nothing (the ~20 `.closest(".settings-section")` scopes still resolve) | **0** | Deliberate constraint on the implementation, chosen to protect these 20 scopes. |
| M2 palette (closed by default) | nothing | **0** | |
| Optional `models` tab (3.5b) | `:3799` rail adjacency | **1** | Only if we take 3.5b. Defer. |
| `saveError` → `{message, route}` | nothing (same node/role/testid) | **0** | |

**Total forced rewrites for the recommended scope: 3 tests** (`:3849`, `:4883`,
`:5327`), plus additive new tests for M1/M2/M3 and one `settingsRail.test.tsx`
re-verify. That is a genuinely small bill against 120 cases — because the design
is built around three properties of the existing suite: names are unanchored,
ids are stable, and roles don't change.

**Suggested ship order** (each independently valuable and independently
revertable):

1. **M1** (route param + `settingsRoutes.ts` extraction + PreflightCard/Express
   wiring + targeted `saveError`). Zero IA change, zero copy change, largest
   user-visible win.
2. **Rail dual labels + live value + group rename + 3.5a** (the copy/i18n pass:
   ~18 new keys ×2 locales, add-only-first).
3. **M3** (panel head, chooser, change preview, n=1 prose, disclosure tiers,
   `.settings-field--inline` cleanup).
4. **M2** (search palette) — last, because its manifest is cheapest to author
   once M1's `ROUTE_INDEX` is already extracted.

---

## 7. Weaknesses — honest

**7.1 The dual label is a compromise and it will read as clutter to someone.**
Two lines per rail row plus a status chip is a lot of ink for eight
destinations, and at 720px I hide the goal line entirely — which means the
narrow layout keeps *exactly today's* engine-first vocabulary. The users most
likely to be on a small window get the least of the redesign. I chose this
because the alternative (goal-only labels) costs 90 test regexes and orphans the
engine vocabulary everywhere else in the product, but I can't claim the narrow
case is solved.

**7.2 M2's manifest will drift, and nothing catches it.** The search index is
hand-authored against `fieldId`s that live in JSX (`id="llm-cerebras-api-key"`).
Add a field and forget the manifest and it is simply unfindable — a silent
regression with no test. Mitigation would be a contract test asserting every
`ROUTE_INDEX`/manifest `fieldId` resolves to a rendered element for its
variant, which means mounting each variant — plausible but not free, and I
haven't priced it. The same drift risk already exists for the 50-entry route
table today; M2 makes it more visible, not worse.

**7.3 Search that jumps but doesn't filter will disappoint someone.** Users who
type "diarization" expecting the field to appear *right there* get taken to
another tab instead. That's the correct call given one-panel-at-a-time mounting,
but it's a real expectation mismatch, and the fix (mount all panels) is
architecturally expensive in exactly the place the constraint sheet says is
expensive (roles/patterns).

**7.4 The change-preview needs a per-variant field map that has no source of
truth.** `settings_groups` declares group *presence*, not field membership, so
both the disclosure tiers and the discard preview depend on a hand-authored
~60-entry map in the frontend. When a provider gains a field and the map isn't
updated, the preview *under-reports* what's about to be discarded — a
correctness bug in the exact mechanism whose whole purpose is trust. A generated
map (extend the Rust registry to emit field ids per group) would fix this
properly; that is a backend change and out of this pass's scope. I'd rather
name this than ship the preview and pretend it's authoritative.

**7.5 I am recommending a copy change that the i18n discipline makes
asymmetric.** ~18-30 new keys ×2 locales, add-only-first, means English ships
with real goal phrasing and Portuguese ships with whatever the pt translation
pass produces later. Between M1's zero-copy win and the copy pass, the copy pass
is the one that can land half-done and look worse than today in pt-BR. Sequence
it as its own PR so it can be held.

**7.6 The `ui_selectable` prose is a product statement, not just UX.** Writing
"Local Whisper is implemented but not selectable yet" in the UI publishes an MVP
scoping decision to users. It's honest and it kills a dead end, but it is a
maintainer call, not a designer's — and if the intent is that Whisper *should*
be selectable, then the right fix is a registry change and this whole sentence
becomes unnecessary. **Ask before building it.**

**7.7 M1 makes the route table load-bearing for the shell.** Extracting
`settingsRoutes.ts` moves ~50 entries from a private closure to a public module
that `PreflightCard`, `NowStrip`, `ExpressSetup` and the store all import. That
is the right shape, but it converts a local refactor hazard into a cross-surface
one: a wrong entry now sends the shell's fix button to the wrong tab, which is
worse than today's "always Modes" because it looks correct.

**7.8 Not addressed by this design, deliberately:** the duplicate/drifting
`credentialPresence` (global array + a second local map in the controller) and
provider readiness living only inside the modal. Both are structural state
problems; the rail's live-value chip renders fine from the in-modal copy, but
the *shell's* rows still read the coarser store heuristic, so PreflightCard and
the rail can disagree within one session. Fixing that means lifting readiness
into the store — worth doing, adjacent to M1, and I'd rather flag it than
pretend a nav redesign resolves it.
