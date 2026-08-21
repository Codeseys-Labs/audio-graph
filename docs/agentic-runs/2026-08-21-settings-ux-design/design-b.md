# Design B — Settings that explain themselves (the INFORMATIVE angle)

Read-only design pass over `/home/codeseys/DevBox/audio-graph` @ master. Constraint sheet at
`/tmp/settings-ux-design/constraints.md`. Every claim below is code-verified with file:line, or is
marked as a judgement/estimate.

---

## 0. Thesis

Settings today is a **configurator**: it accepts values. The informative version is a **state
explainer**: it tells you what is true, what would happen, and what it cannot know.

The single most important discovery of this pass is that **the information is almost entirely
already fetched and already localized**. It is *mislocated*, not missing:

- `get_provider_readiness_cmd` already returns a `ProviderReadiness` row for **every** descriptor in
  the registry. `src-tauri/src/commands.rs:9295-9302` says so explicitly — *"A readiness response
  still includes passive/base metadata for the full registry"* — and `should_probe_provider`
  (`commands.rs:9289`) returns `false` for any provider outside the caller's `active_ids`, so the
  non-active rows are **passive-only, zero-egress**. The frontend keeps all of them
  (`applyProviderReadiness` → `providerReadinessFromEntries`,
  `useSettingsController.tsx:2399-2402`) and then `visibleProviderReadiness`
  (`useSettingsController.tsx:1291`) throws most away for display.
- `settings.providerReadiness.*` is **175 localized keys** in en+pt, including
  `status.*`, `recovery.*`, `credentialSource.*` (9 sources), `dataBoundary.*` (5),
  `roadmapStatus.*`, `runtimeStatus.*`, and a 40-key `fidelity.*` vocabulary. Plus
  `settings.hints.*` (19) and `settings.modelGuidance.*` (6).

So the informative redesign is mostly a **relocation + tone-discipline** job with a small copy
budget — not a copy-writing project. That is what makes it affordable under the i18n constraint.

---

## 1. Code findings that changed the design space

Five findings materially constrain any "informative" design. All verified.

**F1 — The STT chooser is a one-option radiogroup.** In `src/generated/providerRegistry.ts`, of 23
`asr.*` descriptors exactly **one** (`asr.deepgram`) has `ui_selectable: true`.
`ASR_PROVIDER_OPTIONS` (`useSettingsController.tsx:118`) filters on that axis via
`selectableProviderOptionsForStage` (`providerRegistryHelpers.ts:72`). Test-pinned:
`SettingsPage.test.tsx:5327` *"renders only ui_selectable ASR radio options"* asserts
`radios.length === 1`. LLM has 7 selectable; TTS 2.
→ A fresh user opens Speech-to-Text and sees **one** engine with no explanation that six others are
implemented. The existing `settings.providerDeferred.notice` only fires when the *active* provider is
deferred (`AsrProviderSettings.tsx:291-296`) — i.e. only for legacy configs, never for a new user.
This is the largest single "informative" hole in the app, and the constraint sheet's "7 sub-forms"
framing understates it: they are not 7 choices, they are 1 choice and 6 ghosts.

**F2 — Non-active readiness rows mean "nobody looked", not "broken".** Because
`should_probe_provider` short-circuits on `!active_ids.contains(id)`, every non-active provider comes
back `unchecked` (or `missing_credentials`). Rendering those rows' `status` as chips would manufacture
~30 fake problems. The design must treat **non-active rows as prerequisite data only**, never as
status.

**F3 — `descriptor.supports_diarization` exists but is not used, and disagrees with the shipped
gate.** `providerDiarizationSupported` (`useSettingsController.tsx:1522-1526`) is a hardcoded
`["aws_transcribe","deepgram","assemblyai"]`. The generated registry says `asr.local_whisper` has
`supports_diarization: true`. **Migrating the gate to the registry would change behavior** (it would
enable Provider-labels diarization for Local Whisper). The registry may be read for *explanatory
copy*; it must **not** silently become the gate. That separation is load-bearing — see §6/W8.

**F4 — Chips inside a radio `<label>` join its accessible name and break 7 tests.**
`AsrProviderSettings.tsx:270-288` wraps the input + text in `<label className="settings-radio">`; the
one existing in-label badge (whisper model status) already pollutes that name. 7 assertions use
anchored names (`getByRole("radio", { name: /^deepgram streaming$/i })`). Annotations therefore must
live **outside** the label, associated by `aria-describedby` on the input. There are currently **zero**
`aria-describedby` uses anywhere in the settings tree — so this is a net a11y gain, and free on the
test side (`.settings-radio` appears in **0** test selectors).

**F5 — `providerRegistryHelpers.ts` returns hardcoded English.** `providerNotSelectableLabel`,
`providerRoadmapAuthLabel`, `providerCredentialKeysLabel`, `providerCapabilityCredentialLabel`
("Saved: …" / "Needs: …"), `providerStatusLabel`, plus a bare `"Not declared"` literal at
`ProviderCapabilityCard.tsx:238`. These render today inside "Show advanced". **Promoting registry
capability copy to a default-visible surface therefore drags an i18n debt payment with it** (~19 keys).
Design consequence: M1/M2 are built from the *localized* `settings.providerReadiness.*` vocabulary and
touch none of these helpers; the helper i18n port is a separately-priced Tier 2.

---

## 2. The information model: three axes, never conflated

This is the spine of the proposal. Everything else is application of it.

| Axis | Question it answers | Source (all zero new egress) | Chip tones allowed |
|---|---|---|---|
| **1. PLANNED** | What *would* this do if I picked it? | Generated registry: `privacy.data_boundary`, `supports_streaming`, `supports_diarization`, `credential_keys`, `local_models`, `lifecycle.auth`, `status`/`ui_selectable` | `info`, `neutral` |
| **2. PREREQUISITES** | Are the declared inputs present on this machine? | Passive reads only: store `credentialPresence` / `readiness.credentials[].present`, `modelStatus`, `awsProfiles` | `neutral`, `warning` |
| **3. OBSERVED** | Did a check actually succeed, and when? | `readiness.status` + `checked_at` + `stale` + `automatic_probe_available` — active providers only | `success`, `warning`, `danger`, `neutral` |

**The tone-allocation rule (the whole discipline in one line):**
`data-tone="success"` and the word **Ready** are reserved for Axis 3, and only when
`status === "ready" && stale === false`. Axes 1 and 2 may never render success tone.

That mechanizes ADR-0030's own sentence — *"Credential presence alone never means Ready"* (line 76 of
`docs/adr/0030-…md`) — as a lint-able invariant rather than a copy convention. It also matches
ADR-0034's asymmetry: positive evidence is cheap, negative/absence evidence proves nothing.

Three corollaries fall straight out, and each kills a specific lie:

- **C1 — `stale` demotes.** `readiness.stale` today only appends a sentence to a paragraph
  (`ProviderReadinessPanel.tsx:602`, `CredentialsPanel.tsx:203`). If status is promoted to a
  prominent chip, `stale` must demote the tone off `success` — otherwise the redesign makes a stale
  claim *more* prominent than the caveat that qualifies it.
- **C2 — `automatic_probe_available: false` is neutral, not warning.** Verified real: `asr.gladia`
  ("No automatic health probe is available for this provider yet") and Gemini Vertex
  ("Vertex AI readiness is not probed automatically yet") — `ProviderReadinessPanel.test.tsx:535,549`.
  `providerRecoveryAction` already correctly returns `null` for these. A warning chip here reads as
  "you did something wrong" when in fact nothing *can* be checked.
- **C3 — a data-boundary chip is a PLANNED claim.** `descriptor.privacy.data_boundary` is a declared
  property, not observed proof. It must read **"Planned: local only"**, never "nothing leaves your
  device". Under ADR-0034 the observed claim lives in the session data-route report and stays
  `Unknown` until the backend coverage marker exists. Existing localized values are reused verbatim
  (`dataBoundary.local_only` = "Local only", etc.) with a *planned* qualifier supplied by the label,
  not the value.

---

## 3. Mechanism 1 — Annotated provider chooser + deferred roster

**Problem.** You pick a provider blind. `ProviderReadinessPanel` renders only for the *active*
provider (`AsrProviderSettings.tsx:257`, `entry={activeProviderReadiness}`), so the radiogroup itself
carries zero information; you choose, then read the consequence. And on STT there is only one thing to
choose, unexplained (F1).

**Mechanism.** Each radio row gets a sibling annotation line (never inside the `<label>` — F4),
`aria-describedby`-linked to the input. At most **2 chips + 1 short line**. Non-active rows get Axis 1
+ 2 only (F2); the selected row additionally gets Axis 3.

Text sketch — LLM tab (7 selectable rows; annotations for 3 shown):

```
Language model engine
─────────────────────────────────────────────────────────────────
( ) Local Llama          [Planned: local only]  [Model not downloaded]
    Runs on this machine. No key needed. Download the model in General → Models.

(•) Cerebras             [Planned: vendor cloud]  [Ready · checked 14:02]
    Uses your saved cerebras_api_key (OS keychain).

( ) AWS Bedrock          [Planned: configured region]  [Needs aws_access_key +2 more]
    Access keys or an AWS profile. Profile-backed credentials can't be
    verified from here.
```

Text sketch — STT tab, the F1 fix:

```
Speech-to-text engine
─────────────────────────────────────────────────────────────────
(•) Deepgram streaming   [Planned: vendor cloud]  [Ready · checked 14:02]
    Speaker labels available. Uses your saved deepgram_api_key (OS keychain).

▸ 6 more engines are built but not selectable in this version
   Local Whisper · Cloud API · AWS Transcribe · AssemblyAI · OpenAI Realtime · Sherpa ONNX
   These are implemented and keep working if already saved, but can't be chosen
   as a new engine in this build.
```

Notes that make it real:
- "Ready · checked 14:02" uses `readiness.status` + `formatCredentialCheckedAt`
  (`useSettingsController.tsx:408`). If `stale` → `[Ready · cached]` at `warning` tone (C1). If
  `automatic_probe_available === false` → `[No automatic check]` at `neutral` (C2).
- "Needs aws_access_key +2 more" reuses the existing `formatProviderCredentialKeys` truncation shape
  (`providerRegistryHelpers.ts:84`) — but rendered from a localized key, not the hardcoded English
  helper (F5).
- The deferred roster is a `<details>`, collapsed, and its items are **plain text, not controls** —
  ADR-0030: *"Deferred providers appear only as non-actionable Planned or Not in MVP information."*
- CSS: `.settings-radio` sets `display: flex; align-items: center`
  (`src/styles/settings.css:1192-1199`). A two-line row needs those two properties **deleted at that
  site**, not overridden — the seed-0922 unlayered barrel means an added `.ag-field[data-layout]`
  silently no-ops against them. Recipe-wise this is `.ag-chip` (existing) + one BEM block; **no new
  ADR-0047 recipe required.**

---

## 4. Mechanism 2 — Requirement line at the point of disablement

**Problem.** Constraint-sheet effects #1/#2/#3/#4 are all the same shape: a control is disabled,
hidden, or about to discard data, and the *causal* setting is elsewhere and unnamed. Today's
diarization hint is one generic sentence for four different causes
(`settings.diarization.unavailable`, rendered at `SttPanel.tsx:102-106`) — *"not available for the
current provider or local model state"*: it names neither the provider nor the model.

**Mechanism.** Replace the one generic hint with a **cause-specific** line that names the blocking
setting and where to change it. Same `<p className="settings-hint">` slot (styled at
`settings.css:1245`), same test anchor position, three authored strings instead of one.

Text sketch — diarization:

```
Diarization mode  [ Provider labels ▾ ]        ← "Local timeline" + "Hybrid" greyed
  Provider labels come from the STT engine. Deepgram streaming provides them.
  Local timeline needs the speaker model — download it in General → Models.
```

Text sketch — the TTS silent-discard (effect #1; the highest-severity one, because it destroys saved
config with no warning):

```
Provider  [ None (text-only chat) ▾ ]
  ⚠ Saving with None will clear the saved voice (aura-asteria-en), speed (1.0),
    and speak-aloud setting. Switch back before saving to keep them.
```

Text sketch — the disabled-not-hidden case, done right (this one is already good, just buried under
"Advanced provider controls"):

```
Maximum speakers  [ 4 ]  (disabled)
  Used only when Speaker count is Fixed. It's currently Auto.
```

Rules:
- The unlock-hint may name **only `ui_selectable` providers** (see §6/M5). `supports_diarization` is
  read from the registry for *copy*; the gate stays `providerDiarizationSupported` (F3).
- Prefer **disable-with-reason** over hide. The codebase already has the good pattern
  (`maxSpeakers`); TTS's vanish-on-switch is the bad one.
- One authored sentence per cause. Do **not** auto-compose two hints adjacently: `maxSpeakersHint`
  ("the global setting overrides any provider cap") and the provider-cap hint
  (`hints.diarizationOverrideActive`) describe the same override from opposite directions and read as
  a contradiction when stacked.

---

## 5. Mechanism 3 — Errors that teach and point

**Problem.** One global `saveError: string | null` (`useSettingsController.tsx:1576`) renders once in
the footer (`SettingsPage.tsx:136-148`). A validation failure anywhere across ~31 field-sets surfaces
as a generic banner on whatever tab happens to be open, with Retry as the only action.

**Mechanism.** `humanizeError` (`src/utils/humanizeError.ts`) already maps raw strings to
`{ titleKey, causeKey }` via a 5-entry ordered meta table. Add an **optional `route`** to that table's
entries, typed as the existing `SettingsControlRoute` (`{ tab, fieldId, activate?, apply? }`,
`useSettingsController.tsx:238`), and render a third action in the footer alert that calls the
existing `openSettingsControlRoute` (`:1993` — it already does `setActiveTab` + `focusSettingsField`).
Nothing new is invented; two existing seams are joined.

Text sketch:

```
┌────────────────────────────────────────────────────────────┐
│ ⚠  Couldn't save — the provider rejected your key           │
│    Cerebras returned 401. The saved key is unchanged.       │
│    [ Go to Cerebras key ]   [ Retry ]   ▸ Details          │
└────────────────────────────────────────────────────────────┘
```

- "Go to Cerebras key" = `openSettingsControlRoute({ tab: "llm", fieldId: "llm-cerebras-key" })`.
- The 401 → auth mapping already exists (`errors.auth.title`/`.cause` in the meta table).
- `data-testid="settings-save-error"` already exists on that div — one of the few testids in the
  file, so extending it is safe.
- Honest limit: this works only for errors that classify to a field. See W5.

---

## 6. i18n budget

Existing: **495** `settings.*` keys, en+pt fully parallel (0 missing). Add-only-first.

**Tier 1 — the three mechanisms (required):**

| Area | New en keys | Reused existing keys |
|---|---:|---|
| M1 chooser annotations | 5 (`prereq.keySaved`, `prereq.keysNeeded`, `prereq.noKeyNeeded`, `observed.noAutomaticCheck`, `planned.boundaryPrefix`) | `providerReadiness.status.*` (4), `credentialSource.*` (9), `dataBoundary.*` (5), `lastChecked`, `stale`, `notChecked`, `modelReadiness.notDownloaded` |
| M1 deferred roster | 3 (`deferredRoster.summary` w/ count, `.help`, `.a11yLabel`) | `providerDeferred.notice` |
| M2 requirement lines | 5 (`diarization.requiresProviderLabels`, `.requiresLocalModel`, `.requiresBoth`, `tts.switchDiscards`, `diarization.maxSpeakersDisabled`) | `diarization.modes.*`, `hints.*` |
| M3 targeted errors | 2 (`errors.goToField`, `errors.failedWhileSaving`) | `errors.auth.*`, `errors.network.*`, `errors.rateLimit.*`, `notifications.retry/details` |
| **Tier 1 total** | **15 keys → 30 strings (en+pt)** | ~35 keys reused as-is |

Tier 1 is **+3.0%** on the settings namespace. One string (`diarization.unavailable`) becomes
redundant — mark deprecated, do not delete in the same PR (add-only-first).

**Tier 2 — optional, only if registry capability copy is promoted out of "Show advanced":**
port `providerRegistryHelpers.ts`'s hardcoded English (F5) — `providerNotSelectableLabel` 5,
`providerRoadmapAuthLabel` 4, `providerCredentialKeysLabel` 3, `providerCapabilityCredentialLabel` 6,
`formatProviderCredentialKeys` "+N more" 1 = **19 keys → 38 strings**. `providerStatusLabel`'s 5 arms
need **0** new keys (`providerReadiness.roadmapStatus.*` already covers all five).

**Grand total if both tiers land: 34 keys / 68 strings (+6.9%).** Recommendation: ship Tier 1 alone.
Tier 2 is a correctness/i18n-debt PR that should not be gated on a UX redesign.

**Unbudgeted risk (see W4):** pt expansion. `.ag-chip` sets `white-space: nowrap`
(`src/styles.css:911`). "No automatic check" → "Sem verificação automática" is 28 chars; two such
chips plus a provider name will not fit one row at the 720px breakpoint. Chip strings need a
**pt-first length review before the en copy is frozen**, and a hard ≤18-char budget per chip in both
locales. I did not measure this.

---

## 7. What could mislead users if done wrong

Ranked by damage.

1. **A success-tone chip sourced from credential presence.** The cardinal sin. A saved key with
   `status: "error"` (401) would render green. ADR-0030 line 76 is explicit. Mitigation is structural,
   not editorial: the tone-allocation rule (§2) forbids `success` on Axes 1–2, and the honest
   prerequisite string is "Key saved", never "Ready".
2. **Promoting `status` while dropping `stale`.** Making a cached claim more visible than its caveat
   is strictly worse than today's paragraph. C1 (stale demotes tone) is not optional.
3. **Rendering passive rows for 36 providers as status.** F2: they read `unchecked` because nobody
   probed them. Thirty warning chips = thirty invented problems, and it trains users to ignore the
   chip vocabulary entirely.
4. **A data-boundary chip read as a privacy guarantee.** C3 / ADR-0034. "Local only" as a bare chip
   invites "nothing left my device", which the app cannot prove until the exhaustive-coverage marker
   exists. The word **Planned** must be in the label, not implied by placement.
5. **Naming a `planned`/`watch` provider as the thing that would unlock a capability.** The registry
   marks `supports_diarization: true` on `asr.moonshine`, `asr.soniox`, `asr.speechmatics`,
   `asr.elevenlabs_scribe`, `asr.revai`, `asr.google_chirp3`, `asr.azure_speech`,
   `asr.xai_grok_stt` — all `status: "planned"`/`"watch"`, none selectable. "Switch to Soniox to get
   speaker labels" would be a promise the build cannot keep. Unlock hints filter on `ui_selectable`.
6. **Two credential-presence sources drifting inside one modal.** Store array
   (`store/index.ts:2911`, fetched at App mount) vs the controller's independently-refetched local map
   (`useSettingsController.tsx:2389`/`3145`). A new chip reading the store while the adjacent
   credential-health row reads the local map can show the same key in two states in one session. Every
   new surface must read **one** source; inside Settings, that is the controller's map.
7. **"Unused" when readiness simply failed.** `CredentialsPanel.tsx:288` already guards this with a
   distinct `unavailable` chip and a comment explaining why. New surfaces must reuse
   `credentialStatusChip` + that guard, not re-derive the logic.
8. **`ui_selectable: false` framed as "coming soon".** The honest frame is
   *"built, deliberately not offered in this build; saved configs keep working"* — which is what
   `providerDeferred.notice` already says. Roadmap language would misrepresent an MVP scoping
   decision (ADR-0033) as a schedule.
9. **Explaining more of `ProviderCapabilityCard`.** Its ~30-row `<dl>` is an engineer-facing registry
   dump. "Informative" must not be read as "un-collapse the advanced disclosure". More rows is the
   opposite of this design.

---

## 8. Test cost (priced, not feared)

- **M1** — chips as siblings + `aria-describedby` leaves all 47 `getByRole("radio")` queries intact;
  the 7 anchored-name ones are the tripwire that proves it (F4). `.settings-radio` appears in **0**
  test selectors, so the flex→grid change is free. `.closest(".settings-section")` scopes are
  untouched. New tests: ~8–10 (one per tone rule, one per axis, one deferred-roster a11y case).
- **M2** — `SettingsPage.test.tsx:4626` asserts `getByText(/not available for the current provider/i)`;
  splitting one hint into three cause-specific strings **breaks exactly this one assertion**, replaced
  by three. New tests: ~4.
- **M3** — extends an existing `data-testid`. New tests: ~3.
- **Zero rail changes** ⇒ the adjacency test (`~:3799`) and `settingsRail.test.tsx` are untouched.
- Estimated total: **~1 assertion rewritten, ~15–17 tests added.** This is the cheap end of the
  cost model in the constraint sheet (§3 category (a)). The IA-navigation angle is where the
  expensive categories (b)/(d) live; this design deliberately avoids them.

---

## 9. Honest weaknesses

**W1 — The zero-egress ceiling means the honest answer is usually "we don't know."** ADR-0028's
passive-read boundary is right, but it caps what M1 can say about a provider you have not selected: at
best "you have the key" and "here is what it would do". A user asking *"which of these actually
works?"* still cannot find out without selecting it and running checks. This design makes the
*reason* for that ignorance visible, which is honest but less satisfying than a green row of ticks.
I consider the alternative (broadening the probe set) genuinely worse and out of bounds — but I am
choosing honesty over usefulness here, and that is a value judgement, not a finding.

**W2 — Density risk; this is `ProviderCapabilityCard`'s failure mode at smaller scale.** Every chip is
a scan-cost tax on the common case (a user who wants Deepgram and is done). My cap of 2 chips + 1 line
is asserted, not measured, and I provide no user-level way to dismiss the annotations.

**W3 — M1's biggest lever barely applies to the tab that needs it most.** STT has one radio (F1), so
"annotate the chooser" has ~1 row of surface there; the real payload for STT is the deferred roster,
which is *a list of things you cannot pick*. Some users will read that as a teaser, or as a bug
report. I have no better idea that stays inside ADR-0033's scoping decision.

**W4 — pt overflow is unbudgeted and probably real.** `.ag-chip` is `nowrap`; pt strings run ~15–25%
longer; the 720px breakpoint is preserved. I did not build or measure anything. If pt chip copy
doesn't fit, the mitigation is shorter chips + more of the payload in the prose line, which weakens
scannability — the one axis chips were chosen for.

**W5 — M3 degrades exactly when the user is most lost.** A route can only be attached to errors that
classify to a field. A generic backend save failure ("failed to write settings") identifies nothing,
so "Go to the field" is absent in precisely the opaque case. M3 is a partial win on a real problem,
not a fix.

**W6 — Forced-colors / high-contrast unverified.** `.ag-chip` tones are tint-background only, with no
border. Two or three tinted chips on one row may collapse to visually identical in forced-colors mode,
turning the tone vocabulary into noise for the users who most need the distinction. ADR-0047 has
non-text-contrast edge tokens; chips do not use them. This needs an `a11y-review` pass, and it might
force a 7th recipe variant (`.ag-chip[data-tone]` + border) — which would be an **ADR-0047
amendment**, i.e. a cost I have not priced.

**W7 — No user research.** "Users want to know why an option is greyed out" is inferred from the
maintainer's brief and from the codebase's own apologetic comments, not measured. The ranking in §7
is my judgement of damage, not evidence.

**W8 — One tempting cleanup inside this design's blast radius is a functional regression.** F3:
`providerDiarizationSupported`'s hardcoded array excludes `asr.local_whisper`, which the registry
marks `supports_diarization: true`. Anyone implementing M2 will be tempted to "just read the
registry" and will thereby enable Provider-labels diarization for Local Whisper. The registry is for
**copy only** here. This must be an explicit, called-out non-goal in the implementation ticket, or it
will be done accidentally.

**W9 — I did not resolve the ADR-0028 citation.** The brief attributes the credential-display boundary
to ADR-0028, but `docs/adr/0028-…` in this repo is *"Separate capture lifecycle from foreground
workspace"*. The substantive rules I relied on are in **ADR-0019** (credential storage /
`CredentialSource` vocabulary), **ADR-0030** ("Credential presence alone never means Ready"), **ADR-0033**
(`ui_selectable` enforced at content start), and **ADR-0034** (negative-egress evidence). The *rule* is
unambiguous and I honored it; only the citation needs fixing before this is quoted in a PR body.

---

## 10. If only one thing ships

**The tone-allocation rule from §2, applied to the surfaces that already exist.** It requires zero new
markup, zero new i18n, and no IA change: audit every current `.ag-chip` render site
(`ProductModeSummaryCards.tsx:86-98`, `CredentialsPanel.tsx:188/308`,
`ProviderCapabilityCard.tsx:132-142`) against "success tone ⇒ Axis 3, non-stale, only" and fix the
violations. That is the entire credential-presence-never-means-Ready discipline, delivered as a small
correctness PR, before any new surface is built on top of it.
