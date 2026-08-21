# Settings UX Redesign — Constraint Sheet

Read-only inventory of `/home/codeseys/DevBox/audio-graph` (master, post-SHELL-R8 / commit 2fd7d0e) for the settings modal/options redesign. All facts below are code- or test-verified as of this pass; no repo files were edited.

## 1. Current IA

**Rail = single source of truth**: `src/components/settings/settingsRailConfig.ts`. 8 tabs (`SettingsTab` union), 2 groups:
- `providers` group: `overview`("Modes") → `stt` → `llm` → `tts` → `gemini`("Realtime agent") → `credentials`
- `app` group: `general` → `logging`

Rendered by `settings/settingsRail.tsx` (thin, reads `useSettings()` only) and mounted one-at-a-time by `SettingsPage.tsx` (`activeTab === "x" && <XPanel/>`).

**Per-panel inventory** (file, purpose, rough field count, goal):

| Panel | File(s) | Field count / depth | Goal |
|---|---|---|---|
| Overview ("Modes") | `OverviewPanel.tsx` → `ProductModeSummaryCards.tsx` (256 lines) | 4 mode cards (Local/Cloud/Hybrid/Native-converse), each with data-boundary chip, selected/readiness/notInMvp chip, per-stage provider rollup rows (deep-link), blockers list, 4 action buttons | One-shot holistic pick: "what kind of setup do I want" |
| General | `GeneralPanel.tsx` (110) + `AudioSettings.tsx` (86) + `CredentialsManager.tsx` (180) | ~2 fields (theme/lang) + AudioSettings custom markup (no `.settings-field` at all — different convention) + CredentialsManager (models/log, 0 `.settings-field`) | 3 unrelated concerns bundled: app prefs, capture device, model files |
| STT | `SttPanel.tsx` (223) + `AsrProviderSettings.tsx` (934, 29 field markers) | 3 diarization fields + **7 provider sub-forms** (local_whisper/api/openai_realtime/aws_transcribe/deepgram/assemblyai/sherpa_onnx), one visible at a time via radiogroup | Configure speech-to-text + diarization |
| LLM | `LlmPanel.tsx` (155) + `LlmProviderSettings.tsx` (1196, 45 field markers) | **7 provider sub-forms** (local_llama/api/aws_bedrock/openrouter/cerebras/sambanova/mistralrs) + OpenRouter accelerator preset picker | The single deepest panel in the app |
| Gemini ("Realtime agent") | `GeminiPanel.tsx` (75) + `GeminiSettings.tsx` (250, 8 fields) | native-realtime toggle + 2 auth-mode sub-forms (api_key/vertex_ai) | Configure native speech-to-speech |
| TTS | `TtsPanel.tsx` (190, 5 fields) | provider select (deepgram_aura/none) + voice/speed/speakAloud/key/test, all gated on `deepgram_aura` | Configure speech output |
| Credentials | `CredentialsPanel.tsx` (462) | 2 pivots: by-provider readiness rollup + by-key credential-health cards (in-place edit, Replace/Retest, Clear behind ⋯) | "Is my setup actually working" |
| Logging | `LoggingPanel.tsx` (12) → `LoggingSettings.tsx` (326, 6 fields incl. 3 dead-modifier rows) | ~6 fields | Diagnostics/analytics opt-in |

Every provider tab also renders `ProviderCapabilityStageSection` (registry capability cards, ~30-row `<dl>` each) behind an `AdvancedSettingsDisclosure`.

**Why "31-panel-or-so"**: there are only 8 rail tabs, but they front ~25-31 distinct field-sets once you count provider-variant sub-forms: STT 7 + LLM 7 + TTS 2 + Gemini 2 + Overview 4 mode cards + General/Credentials/Logging subsections. The rail is shallow; the *provider radiogroups* are where the real depth lives — picking a radio swaps in a completely different field set inline, with no sub-nav/breadcrumb showing which variant is currently live.

**Express Setup ↔ full Settings**: `ExpressSetup.tsx` is a separate first-launch-only modal, shown when `!hasConfiguredDurableNotesRoute(...)` at `App.tsx` mount. It reuses the exact same `deriveProviderSetupModeCards` derivation Overview's mode cards use, but offers a filtered subset (4 ASR × 3 LLM choices, gated on `ui_selectable`). Its "Advanced" button (`onOpenAdvanced`) does `setExpressSetupVisible(false)` then bare `openSettings()` — **no route/tab handoff**; the user's in-progress Express picks are not carried into Settings, and Settings opens on Overview regardless of what they were configuring.

**PreflightCard / NowStrip deep-link mechanics — the single most concrete navigability gap found**: `<SettingsPage/>` takes **zero props** (`App.tsx:1096`), and `activeTab` is `useState<SettingsTab>("overview")` inside `useSettingsController.tsx:1609` — it resets to Overview on every mount. Both `PreflightCard`'s Route-row fix action (`onAction={openSettings}`) and `NowStrip`'s settings gear (`onClick={openSettings}`) call the bare store action `openSettings()` (`store/index.ts:2905`), which only does `set({ settingsOpen: true })` + three fetches. **There is no mechanism today for anything outside the modal to land on a specific tab or field.** Only *in-modal* navigation (`openSettingsControlRoute` in `useSettingsController.tsx:1993`, consumed by `ProductModeSummaryCards`/`ProviderCapabilityCard`/`CredentialsPanel`'s own deep-link buttons) can jump tabs + focus a field. If the redesign wants PreflightCard's "fix" actions to actually land on the broken provider's tab, `openSettings` needs a route parameter and `activeTab`'s initial state needs to read it — a real, scoped backend-free wiring change, not a styling change.

## 2. Information already available to surface

- **Provider readiness** (`ProviderReadiness[]` from `get_provider_readiness_cmd`) lives ONLY as local `useState` inside `useSettingsController.tsx` (~line 1101) — fetched fresh each time Settings mounts, **not in the global Zustand store**. NowStrip/PreflightCard cannot see it; they fall back to the coarser `hasConfiguredDurableNotesRoute` heuristic, which can't distinguish "credentials present but provider errored" from "ready."
- **Credential presence** exists **twice**: global store `credentialPresence: CredentialPresence[]` (array; `store/index.ts:2911`, fetched once at App mount, used by PreflightCard/NowStrip/ExpressSetup) and a second, independently-fetched local map inside `useSettingsController` (`load_credential_presence_cmd` called again at controller-mount, `useSettingsController.tsx:2389`/`3145`). Two shapes, two fetches, real drift risk within one session.
- **Model status** (`ModelStatus`) is global store state (`fetchModelStatus`/`fetchModels`), feeds `ReadinessModelActions`/`ModelActionButtons` and the route heuristic.
- **`ui_selectable` / deferred**: `ProviderDescriptor.ui_selectable` (boolean, independent of `status`) is the single axis that should always gate "can be picked." Drives ExpressSetup's filtering, `ProviderCapabilityCard`'s Selectable/Deferred/Readiness-only/Planned badge, and Overview's "Not in MVP" chip. Helper: `providerIsDeferred` in `providerRegistryHelpers.ts`.
- **Validation errors**: exactly ONE global `saveError: string | null` (`useSettingsController.tsx:1576`), rendered once in the SettingsPage footer via `<HumanizedError>` (generic message + Retry only — no field/tab targeting anywhere in the codebase).

## 3. Test/selector coupling

`src/components/SettingsPage.test.tsx`: 6822 lines, **120** `it()`/`test()` cases (2 top-level `describe`s — `settingsReducer` unit tests and `SettingsPage` integration — plus 5 nested: unsaved-confirm-on-close, Cerebras/SambaNova/OpenRouter LLM provider, Uniform Load-models button).

Selector kinds (all role-based, RTL-idiomatic — track accessible name/role, not DOM position or `data-testid`; **zero** `getByTestId` calls in this file):

| Role | Count |
|---|---:|
| button | 89 |
| radio | 47 |
| heading | 44 |
| tab | 24 |
| combobox | 22 |
| option | 12 |
| radiogroup | 4 |
| checkbox | 4 |
| tabpanel | 3 |
| status | 3 |
| dialog | 2 |
| tablist | 1 |
| region | 1 |
| group | 1 |

`goToTab(name: RegExp)` helper (click `getByRole("tab",{name})`) is called **89** times, keyed to current rail labels: `language model` ×39, `speech-to-text` ×27, `general` ×10, `realtime agent` ×8, `text-to-speech` ×5. Because these match by accessible name, renaming a tab breaks every regex targeting it; removing/merging a tab breaks every call that targeted it specifically; **splitting** a provider's sub-forms into new sibling tabs only costs re-pointing calls to new labels (name-based, not order-based).

**One test hard-codes rail adjacency**: `"wires Settings tabs to tabpanels and supports keyboard navigation"` (line ~3799) asserts `ArrowDown` from Overview → STT, `End` → Logging (last), `Home` → Overview (first), wrap-around `ArrowUp` from first → last. This is the one test that breaks on ANY reorder/insert/regroup of `RAIL_SECTIONS`, independent of renames.

**~20 `.closest(".settings-section")`** scoping calls (`within(scope)` pattern) assume every panel wraps fields in `<section className="settings-section">`; these break if internal wrapper markup changes shape (e.g. `.settings-section` → `.ag-card`) even with the rail untouched. Also pinned via `.closest()`: `.settings-credential-health__item` ×2, `.settings-mode-card` ×1, `.settings-provider-capability-card` ×1; plus `document.querySelector(".settings-readiness__*"/".settings-overlay")` ×6.

`src/components/settings/settingsRail.test.tsx` (144 lines) is a **separate, cheap** a11y contract test — it fully mocks `useSettings()`, so rail-only markup/label/grouping changes are nearly free to re-verify there; the 6822-line file above is the expensive surface.

**Cost model**: (a) re-label/re-icon/reorder-within-group ≈ cheap (regexes + the one adjacency test); (b) merge/remove/split a rail tab ≈ moderate (every `goToTab` targeting it + the adjacency test + any `.closest(".settings-section")` inside moved content); (c) changing a panel's DOM wrapper convention (`.settings-section` → `.ag-card`) ≈ moderate-to-wide but mechanical (~20+ scope-selector edits, no logic changes); (d) changing accessible roles/patterns (tabs, radiogroups) ≈ expensive — touches the bulk of the 210+ role queries. 120 (or ~125 counting `settingsRail.test.tsx`/`CredentialsPanel.test.tsx`) is a priced upper bound on touch surface, not a difficulty signal.

## 4. Known UX debt

- **Dead `.settings-field--inline` BEM modifier**: used in 6 sites (`LlmProviderSettings.tsx:416`, `LoggingSettings.tsx:190/248/308`, `GeminiPanel.tsx:42`, `TtsPanel.tsx:123`) but **zero CSS rule** exists for it in `settings.css` — `.settings-field` only sets `margin-bottom`. Whatever inline-checkbox layout was intended is gone. Don't propagate this dead class; either give it a real rule (or migrate to `.ag-field[data-layout="row"]`, which already exists) or drop it.
- **Bare/inconsistent field markup in `TtsPanel`**: `tts-provider-select`/`aura-speed-input`/`speak-aloud-toggle` labels skip the shared `settings-field__label` class that `aura-voice-select`'s label uses, in the same panel.
- **`ModelCatalogPicker`** (shared combobox, used by TTS Aura voice and provider model pickers) is a good existing reuse seam — already carries loading/error/empty states — worth extending rather than reinventing per-provider.
- **Provider-variant depth** (7 STT + 7 LLM sub-forms behind radiogroups) is the single biggest navigability problem: switching provider swaps the entire visible field set with no sub-nav/breadcrumb for "which variant is live."
- **Duplicate/drifting `credentialPresence`** and provider-readiness-only-inside-the-modal (§2) — structural, not cosmetic.
- **No deep-link from outside the modal** (§1) — the most user-visible gap given the maintainer's ask, since PreflightCard's whole premise is "fix actions that deep-link into settings."
- **Single global `saveError`, no field targeting** (§2) — a validation failure anywhere across ~31 sub-panels surfaces as one generic banner on whichever tab happens to be open.
- **`ProviderCapabilityCard`'s ~30-row `<dl>`** (Stage/Streaming/Diarization/Wire encoding/Resampling/Multichannel/Events/Source policy/Auth/Credential keys+state/Roadmap auth+source/Transport/Session/Keepalive/Close/Model catalog/Default model/Catalog count/Data boundary/Endpoint modes/Runtime packaging/Speaker labels/Health probes/Platform blockers/Readiness/Runtime) is an engineer-facing registry dump wearing user-facing chrome. Already gated behind "Show advanced" — good — but a cautionary example if "informative" is read as "show more of this by default."
- **Seed audio-graph-0922 (open, P2)**: `src/styles/index.css`'s barrel (`layout.css`/`keyframes.css`/`primitives.css`/`settings.css`/`shortcuts-modal.css`/`express-setup.css`) loads via `App.tsx`'s separate import, **entirely unlayered** relative to `styles.css`'s `@layer theme, base, components, utilities` stack. Unlayered beats layered regardless of specificity — adding an `.ag-*` recipe class alongside an existing BEM rule in `settings.css` **silently no-ops for every overlapping property** unless the overlapping BEM box-model properties are deleted at the same site. SHELL-R8 already worked around this per-adoption-site; any redesign PR touching `settings.css` must do the same or verify the barrel-layering fix has landed first.
- **Silent setting-requires-another-setting chains**: (a) TTS voice/speed/speakAloud/key/test only render for `deepgram_aura` — switch away and they vanish with no trace, likely reverting to compiled defaults on save; (b) `maxSpeakers` is disabled-not-hidden when `speakerCount !== "fixed"` (good pattern) but buried behind "Advanced provider controls"; (c) diarization capability depends transitively on ASR provider choice, with the causal link invisible except via a generic unavailable hint; (d) every one of the 14 total provider radiogroup entries (7 STT + 7 LLM) is this same silent-swap pattern.

**Recipe-adoption scale for planning**: `settings.css` has only 6 recipe-selector rules; settings-adjacent TSX (`settings/*.tsx` + the 6 provider/manager components) uses `.ag-*` classes at 15 sites (mostly `.ag-chip`), versus **57** remaining `.settings-field` BEM usages across the same files. Field-level migration to the recipe layer is a separate, large, mechanical lift from the IA/navigation redesign — don't conflate the two in scoping.

## 5. What "informative" could mean — top 10 non-obvious effects

1. **TTS provider switch** (`ttsType` away from `deepgram_aura`) silently drops voice/speed/speak-aloud to compiled defaults on save — no preview of what's about to be discarded.
2. **Diarization mode × speaker count**: `provider`/`hybrid` modes disable in the `<select>` (visible-but-unpickable, good) when unsupported, but the *why* (which provider/model would unlock it) isn't shown at the disable point.
3. **ASR provider choice gates diarization capability transitively** — the causal link (this STT provider → that diarization option) is invisible unless already known.
4. **`ui_selectable` vs `status`**: a provider can be fully implemented yet not selectable (MVP scoping) — correctly labeled in `ProviderCapabilityCard`, but that card is hidden behind "Show advanced," so the default view gives no reason for a greyed/missing option.
5. **Credential presence ≠ provider readiness**: a saved key can still be `status: "error"` — CredentialsPanel distinguishes this correctly, but it's a tab away from the provider's own config tab where the key was entered.
6. **AWS profile-backed credentials** (Bedrock LLM, Transcribe STT) aren't enumerated into the global store — the readiness heuristic outside Settings can never fully verify a profile-backed route (a documented gap in `App.tsx`/`NowStrip`/`PreflightCard` comments).
7. **OpenRouter accelerator preset** picking ≠ applying (`openrouterAcceleratorPreset` vs `openrouterAppliedAcceleratorPreset`) — nothing outside the LLM tab shows whether the applied preset matches the picked one.
8. **Overview mode cards' "Use this mode"** mutates the underlying ASR/LLM provider selection as a side effect — overwrites, not merges, whatever the user had already configured on the STT/LLM tabs.
9. **Native-realtime mode is settable from three surfaces** (Gemini panel's toggle, Overview's native_realtime card, PreflightCard's `ConversationModeControl`) writing the same two flags (`conversationMode`/`converseEngine`), none showing what the other two currently hold.
10. **Model download is a single global slot**: starting one provider's model download disables the Download button on every OTHER provider's model row app-wide, with only a generic disabled state — no "download in progress for X" message on the blocked rows.

## Hard constraints recap (for the plan, not re-derived here)

- ADR-0047 owns the `.ag-*` recipe layer (`.ag-label`/`.ag-chip[data-tone]`/`.ag-card[data-elevation]`/`.ag-btn-micro`/`.ag-field`(+`data-layout="row"`)/`.ag-panel-head`) — a 7th recipe or a new closed variant set needs an ADR-0047 amendment, not a silent addition.
- ADR-0013 (Conversation modes) owns the `conversationMode`/`converseEngine` two-flag model — see finding 9 above; any consolidation of its 3 write sites is an ADR-scoped decision, not a UI polish.
- Credential-boundary security rules: credential presence may only be DISPLAYED from the existing passive read (`load_credential_presence_cmd`/store `credentialPresence`) — no new provider egress/active probe from a redesigned surface (ADR-0028).
- i18n: 495 `settings.*` keys today, fully parallel en/pt (0 missing) — any new copy needs matching en+pt additions, add-only-first per project discipline.
- SHELL-R8 (2fd7d0e) intentionally left rail IA untouched and preserved the 720px breakpoint with zero copy changes — this redesign is the first pass explicitly authorized to touch IA/copy.
