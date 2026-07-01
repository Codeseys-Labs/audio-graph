# Settings Redesign — Implementation Plan

Date: 2026-07-01
Scope: `src/components/settings/*`, `src/components/{CredentialsManager,LoggingSettings,ProductModeSummaryCards,ConversationModeControl}.tsx`, `src/components/providerSetupModes.ts`, `src/store/index.ts`, `src/i18n/locales/{en,pt}.json`, tests. Backend (`src-tauri`) is untouched.

This plan realizes ADR-0006 B1 ("Settings UI splits cleanly: STT/LLM/TTS (pipeline) and Realtime Voice Agent") plus three approved decisions: (1) turn Overview into an interactive "Modes" selector (fixes the stuck-on-native bug), (2) dedup the log-level control, (3) restructure the IA into a "Providers & Models" group. All work stays on the frontend; there is no `product_mode` enum in Rust (`commands.rs:7268-7273`), so "product mode" is a pure frontend presentation over the two store flags `conversationMode` + `converseEngine`.

## FINAL DECISIONS (user-approved 2026-07-01 — these OVERRIDE any "recommendation/defer" language below)

1. **Derived modes ARE in scope now (WS1 = M+).** All 4 mode cards fully selectable. `native_realtime` = 2-flag toggle; the 3 durable cards (`local_private`/`cloud_fast`/`hybrid`) ADDITIONALLY swap ASR/LLM provider selection to the card's derived priority providers (`providerSetupModes.ts:248-272`) so `selectedModeId()` re-classifies correctly. This touches the settings reducer, dirty-tracking/baseline (`useSettingsController.tsx:1398-1414`), and Save. Do NOT ship the "defer / hint to ControlBar" fallback from §1.4.
2. **Rail: DELETE the "setup" group.** Move `overview`(→"Modes") to be the FIRST item of the `providers` group. Final `RAIL_GROUP_ORDER` = `["providers","app"]`. Final `RAIL_SECTIONS` order: `overview(providers), stt(providers), llm(providers), tts(providers), gemini(providers), credentials(providers), general(app), logging(app)`. All tab IDS unchanged. `RailGroup` type drops `"setup"`.
3. **Fix the OpenAI-agent readiness gap NOW (part of WS3):** append `realtime_agent.openai_realtime` to `activeReadinessProviderIds` (`useSettingsController.tsx:1149-1156`) when `converseRealtimeAgentProvider==="openai"` and native is selected (mirror the existing `realtime_agent.gemini_live` append). Add a test.
4. **DELETE the orphaned logging i18n keys (WS2):** remove `settings.sections.diagnostics`, `settings.fields.backendLogLevel`, `settings.hints.logLevelPrefix`, `settings.hints.logLevelSuffix` from BOTH en.json and pt.json in the same commit (parity). Grep-confirm no other consumer first (esp. `settings.logLevels.*`).
5. Execution: all 3 workstreams in parallel worktrees + review + reconcile (WS1 & WS3 both touch Overview/rail — reconcile at integration).

## Confirmed ground truth (verified against current source)

- `RAIL_SECTIONS` (`settingsRailConfig.ts:38-47`) currently groups `overview→setup`, `stt/llm/gemini/tts→providers`, `general/credentials/logging→app`. Order is `overview, stt, llm, gemini, tts, general, credentials, logging`.
- `SETTINGS_TABS = RAIL_SECTIONS` and default `useState<SettingsTab>("overview")` (`useSettingsController.tsx:1435-1436`); keyboard nav walks `SETTINGS_TABS` by index (`:2106-2123`). Changing group membership only changes render grouping; changing array **order** changes arrow-key traversal order.
- `ProductModeSummaryCards.tsx` cards are read-only `<article>`s (`:53-60`), hardcoded English title "Product mode overview" (`:43`), passive `Selected` badge (`:81`), and only deep-link buttons. No `setConversationMode`/`setConverseEngine`.
- The controller's card memo (`useSettingsController.tsx:1367-1390`) passes `conversationMode`/`converseEngine` (read from the store at `:925-928`) but exposes no setter to the cards. Both setters are already imported (`setConversationMode` :926, `setConverseEngine` :928).
- `selectedModeId()` (`providerSetupModes.ts:1160-1187`): native branch is a clean flag check (`:1166-1169`); local/cloud/hybrid are DERIVED from ASR/LLM provider locality (`:1171-1186`).
- Store setters: `setConversationMode` (`store/index.ts:2114-2121`) and `setConverseEngine` (`:2133-2149`, keeps legacy `nativeS2sEnabled` in sync). No backend call needed to switch modes.
- Log-level duplication CONFIRMED: `CredentialsManager.tsx:306-336` "Diagnostics" section (options include `off`) vs `LoggingSettings.tsx:186-202` (`LEVELS` at `:42` = error/warn/info/debug/trace, NO `off`). Both use `id="log-level-select"` (duplicate-id bug). Handler `handleLogLevelChange` at `useSettingsController.tsx:3230-3237`, exported `:3373`; threaded via `GeneralPanel.tsx:28,108` → `CredentialsManager` props `:177,195,325`, `logLevel` destructure `:197`, `Pick<...,"logLevel">` `:168`.
- Readiness/Credentials are two projections of the SAME `providerReadinessEntries` (`useSettingsController.tsx:1168-1171`). Overview by-provider via `visibleProviderReadiness` (`:1172-1183`); Credentials by-key via `savedCredentialEntries` filtered `present===true` (`:1240-1246`) + `relatedReadinessForCredential` (`:1729-1732`).
- `activeReadinessProviderIds` (`:1149-1156`) appends only `realtime_agent.gemini_live` when native selected — NOT `realtime_agent.openai_realtime` (a real gap noted below).
- GeminiPanel already renders `<ProviderCapabilityStageSection stage="realtime_agent" />` (`GeminiPanel.tsx:72`), which enumerates BOTH realtime agents; the native toggle uses `settings.conversation.enableNative` (`:49`). The tab is a misnomer, not Gemini-only.
- i18n: tabs at `en.json:531-540`, railGroups `:541-545`, providerReadiness `:344-...`, credentialHealth `:498-517`, logLevels `:102-109`, `sections.diagnostics` `:78`, `fields.backendLogLevel` `:145`, `hints.logLevelPrefix/Suffix` `:254-255`, `conversation.*` `:546-550`. Parity enforced by `locale-parity.test.ts` (exact key-path set match, en↔pt).

---

## Workstream overview and dependency order

Three workstreams. WS2 is fully independent. WS1 and WS3 both edit `OverviewPanel.tsx` / the Overview surface, so sequence them WS1→WS3 to avoid a rebase, though they are not hard-coupled.

| WS | Title | Touches Overview? | Depends on |
|----|-------|-------------------|------------|
| WS1 | Bug fix: Overview → interactive "Modes" selector | Yes (ProductModeSummaryCards, controller, i18n) | none |
| WS2 | Logging dedup | No | none |
| WS3 | IA restructure to ADR-0006 B1 | Yes (moves readiness out of Overview, rail regroup, relabels) | soft-after WS1 |

Suggested landing sequence: **WS2 (clears noise) → WS1 (user-facing bug) → WS3 (IA)**.

---

## Workstream 1 — Overview becomes the interactive "Modes" selector (fixes stuck-on-native bug)

Goal: relabel the "overview" tab to "Modes" (keep id `overview`), and make `ProductModeSummaryCards` interactive so a user can pick a mode. This fixes the discoverability bug where the cards mirror `converse+native` with no way to change it.

### 1.1 New controller handler: `handleSelectProductMode`

File: `src/components/settings/useSettingsController.tsx`

Add a callback near the card memo (`:1367-1390`), using the already-imported store setters (`setConversationMode` :926, `setConverseEngine` :928). It takes a `ProviderSetupModeCard` (or its `id`/`productPath`) and maps to store writes:

- `native_realtime` (productPath `native_realtime_agent`): `setConversationMode("converse")` then `setConverseEngine("native")`. This is the clean two-flag toggle; `selectedModeId()`'s native branch (`providerSetupModes.ts:1166-1169`) will immediately re-classify the card as selected, and `setConverseEngine` keeps `nativeS2sEnabled` in sync (`store/index.ts:2141-2148`).
- The three durable cards (`local_private`/`cloud_fast`/`hybrid`, productPath `durable_notes_graph`): at minimum set `setConversationMode("notes")` + `setConverseEngine("pipelined")` to leave native. **But** flipping flags alone will NOT move between local/cloud/hybrid — `selectedModeId()` (`:1171-1186`) derives those three from ASR/LLM provider locality. See scope decision 1.4.

Export the new handler in the controller's return object (the large object starting at `:3248`; add alphabetically near `handleProviderSetupSourceRecovery`).

Also thread `conversationMode`/`converseEngine` are already exported (`:3323-3324`), no change needed there.

### 1.2 Make the cards interactive

File: `src/components/settings/ProductModeSummaryCards.tsx`

- Pull `handleSelectProductMode` from `useSettings()` (`:22-32`).
- Convert the card grid to a `radiogroup` semantics OR add a per-card "Use this mode" button. Recommended: a "Use this mode" `<button>` inside each `<article>` (`:53-60`), placed in the actions row (`:160-201`), with `aria-pressed={card.selected}` and `disabled={card.selected}`, calling `handleSelectProductMode(card)`. This is the lint-clean pattern already used by `ConversationModeControl.tsx:106-134` (toggle buttons + `aria-pressed`, avoiding `role="radio"` on styled buttons per the biome note there). Keep all existing deep-link buttons.
- Replace the hardcoded title "Product mode overview" (`:43`) with `t("settings.modes.title")` (new key). Add the button label `t("settings.modes.useThisMode")`.
- The `Selected` badge (`:81`) stays as the derived-state indicator (now consistent with the pressed button).

### 1.3 i18n (WS1)

Add to `en.json` and mirror in `pt.json` (parity test):
- `settings.tabs.overview`: rename value "Overview" → "Modes" (`en.json:532`). Keep the KEY and tab id.
- New `settings.modes.title` = "Modes" (replaces the hardcoded card-section title).
- New `settings.modes.useThisMode` = "Use this mode".
- (Optional, if durable cards are deferred — see 1.4) `settings.modes.switchInControlBar` = a one-line hint pointing at the ControlBar for the derived modes.

### 1.4 SCOPE DECISION — derived modes (local/cloud/hybrid): recommend DEFER

The native↔durable toggle is clean and self-contained (S). Correctly "selecting" `local_private` vs `cloud_fast` vs `hybrid` requires swapping the actual ASR/LLM provider selection (`asrType`/`llmType` in the reducer) to the card's derived priority providers (`providerSetupModes.ts:248-272` priority lists), because `selectedModeId` classifies from provider locality — a flag flip cannot move between the three. That is a materially larger change (M+) touching the settings reducer, dirty-tracking/baseline (`useSettingsController.tsx:1398-1414`), and Save.

Recommendation: **Phase 1 makes only `native_realtime` and "leave native → pipelined notes" fully selectable.** The three durable cards either (a) render "Use this mode" that sets `notes`+`pipelined` and shows a hint that ASR/LLM locality is chosen in the STT/LLM tabs, or (b) show a disabled "Use this mode" with the ControlBar/provider-tab pointer. Track true local/cloud/hybrid selection as a follow-up. This is an open question for the user (see Risks).

### 1.5 Effort: **S** (native-only selector) / **M** (if durable-mode provider-swap is in scope now).

---

## Workstream 2 — Logging dedup (remove the General/CredentialsManager copy)

Goal: single canonical log-level control in the Logging tab. Keep the reducer field/command/`save_settings_cmd` payload for startup persistence.

### 2.1 FIRST: add "off" to the surviving control

File: `src/components/LoggingSettings.tsx`
- Change `const LEVELS = ["error","warn","info","debug","trace"]` (`:42`) → prepend `"off"`. The select at `:196-201` renders `LEVELS` directly, so `off` becomes selectable. `apply({ level })` (`:69-94`) forwards it to `set_logging_config` unchanged; backend `apply_log_level` already accepts `off`. Do this BEFORE deleting the General control so the `off` capability is not lost.
- Note the option labels in LoggingSettings are the raw level string (`:197-198`), not i18n'd — `off` will render as "off" (consistent with the others). If we want the descriptive labels, reuse `settings.logLevels.*` (`en.json:102-109`); optional, out of scope.

### 2.2 Delete the duplicate

File: `src/components/CredentialsManager.tsx`
- Remove the entire "Diagnostics" `<div className="settings-section">` block (`:306-336`) — the level select is its only content.
- Remove prop `handleLogLevelChange` from `CredentialsManagerProps` (`:177`) and the destructure (`:195`).
- Remove `logLevel` from the `Pick<SettingsState, ...>` (`:168`) and from the destructure `const { confirmDelete, logLevel } = state;` (`:197`) → `const { confirmDelete } = state;`.
- Drop the now-unused `LogLevel` import if nothing else uses it (check top-of-file imports).

File: `src/components/settings/GeneralPanel.tsx`
- Remove `handleLogLevelChange` from the `useSettings()` destructure (`:28`) and from the `<CredentialsManager>` props (`:108`).

File: `src/components/settings/useSettingsController.tsx`
- Remove `handleLogLevelChange` (`:3230-3237`) and its export line (`:3373`) IF no other consumer remains (grep confirmed only General/CredentialsManager use it).
- **Keep** `setField("logLevel", ...)` reducer field, the `set_log_level` command, and `save_settings_cmd`'s `log_level` payload (`:3141`) — these are still used for startup persistence. Only the UI + dedicated handler go.

### 2.3 i18n (WS2)

- The keys `settings.sections.diagnostics` (`en.json:78`), `settings.fields.backendLogLevel` (`:145`), `settings.hints.logLevelPrefix`/`.logLevelSuffix` (`:254-255`) become unused after the delete. Options:
  - Safe: leave them in place (unused keys don't break parity; parity only checks en↔pt shape equality). Zero risk.
  - Tidy: delete them from BOTH en.json and pt.json in the same commit (parity test will fail if you drop from only one). Confirm no other consumer via grep first (`settings.logLevels.*` is still used elsewhere? — it is only used by the deleted block; verify before removing).
- Removing one `id="log-level-select"` fixes the duplicate-id bug automatically.

### 2.4 Effort: **S**. Independent; no backend.

---

## Workstream 3 — IA restructure to ADR-0006 B1 ("Providers & Models")

Goal: regroup the rail into a "Providers & Models" cluster `[stt, llm, tts, gemini(relabeled), credentials(relabeled)]`; relabel gemini → "Realtime agent (native S2S)" (hosts BOTH agents); relabel credentials → "Credentials & readiness" and ABSORB Overview's provider-readiness section. Keep ALL tab ids unchanged.

### 3.1 Rail changes

File: `src/components/settings/settingsRailConfig.ts`
- `SettingsTab` union (`:14-22`): unchanged (all ids stay).
- `RAIL_SECTIONS` (`:38-47`): move `credentials` from `group:"app"` into `group:"providers"`, and reorder so the providers cluster reads `stt, llm, tts, gemini, credentials`. Target order:
  - `{ overview, setup }` (now "Modes")
  - `{ stt, providers }`, `{ llm, providers }`, `{ tts, providers }`, `{ gemini, providers }`, `{ credentials, providers }`
  - `{ general, app }`, `{ logging, app }`
- `labelKey`s stay pointing at `settings.tabs.*`; only the i18n VALUES change (3.3). `RAIL_GROUP_ORDER` (`:57`) unchanged (`setup→providers→app`).
- Note: reordering `stt/llm/tts/gemini` and moving `credentials` changes arrow-key traversal order (driven by the `SETTINGS_TABS` array in `useSettingsController.tsx:2106-2123`) — expected and desired; update any test asserting order (see Test impact).

### 3.2 Move provider-readiness from Overview into Credentials

File: `src/components/settings/OverviewPanel.tsx`
- Remove the entire `<section className="settings-readiness">` (`:41-175`), leaving only `<ProductModeSummaryCards />` (which WS1 made interactive). Drop the now-unused destructured values from `useSettings()` (`:24-36`): `providerReadinessLoading`, `providerReadinessError`, `providerReadinessStatusSummary`, `visibleProviderReadiness`, `activeReadinessProviderIdSet`, `selectedModelForProvider`, `credentialRouteForReadiness`, `credentialPresence`, `handleOpenCredentialRoute`, `refreshProviderReadiness`, and the `ProviderReadinessDetails`/`providerCatalog*`/`PROVIDER_DESCRIPTORS`/`PROVIDER_READINESS_LABELS` imports.

File: `src/components/settings/CredentialsPanel.tsx`
- Add the by-provider readiness list as a second `<section>` (the moved `.settings-readiness` markup) alongside the existing `.settings-credential-health` list, so the panel shows BOTH pivots. Pull the readiness values from `useSettings()` (all already on the context; both panels share it, so the move is low-friction).
- **Preserve the pivot difference (critical constraint):** the by-provider list uses `visibleProviderReadiness`, which includes providers with `missing_credentials`/`unchecked` (no saved key), whereas `savedCredentialEntries` only lists `present===true`. Keeping the readiness section verbatim preserves the "here's a provider you haven't set up yet" affordance that would otherwise vanish.
- Consolidate the two refresh affordances: the readiness "Run checks" (`OverviewPanel.tsx:63-70`, `refreshProviderReadiness()`) and the per-key "Retest" (`CredentialsPanel.tsx:149-158`, `refreshProviderReadiness({force:true})`) now co-locate. Keep both or unify to one header-level "Run checks / Retest" button — presentational choice; both call the same `refreshProviderReadiness`.
- Section ordering within the panel: recommend readiness (by-provider, includes missing keys) first, then credential-health (by-key). Update `aria-labelledby` ids so the two `<section>`s have distinct heading ids.

### 3.3 i18n (WS3)

Rename VALUES (keep KEYS + ids), mirror in pt.json:
- `settings.tabs.credentials` (`en.json:538`): "Credentials" → "Credentials & readiness".
- `settings.tabs.gemini` (`:536`): "Gemini" → "Realtime agent (native S2S)".
- `settings.railGroups.providers` (`:543`): "Providers" → "Providers & Models".
- In-panel copy: update `settings.credentialHealth.title`/`.help` (`:499-500`) if the panel header should read "Credentials & readiness"; the moved readiness section keeps its own `settings.providerReadiness.title`/`.help`/`.runChecks` (`:345-348`).
- `settings.conversation.*` (`:546-549`) — the GeminiPanel native toggle copy still says "Gemini Live"; optionally broaden the help to name OpenAI Realtime too (aligns with the relabeled tab hosting both agents). Optional; the capability section already lists both.
- All renamed/added keys must exist in BOTH locales or `locale-parity.test.ts` fails.

### 3.4 The realtime-agent tab must host BOTH agents (correctness constraint)

- No structural change needed to GeminiPanel: `<ProviderCapabilityStageSection stage="realtime_agent" />` (`GeminiPanel.tsx:72`) already enumerates both `realtime_agent.gemini_live` and `realtime_agent.openai_realtime` from the registry. The relabel makes the tab honest. Do NOT rename the tab id.
- File by STAGE, not provider name: leave the credential/readiness routing keyed off `providerRegistry` stages. Note `credentialPlanForProvider` already handles `realtime_agent.openai_realtime` (`providerSetupModes.ts:674-679`) via `openai_api_key`, and `asr.openai_realtime` is a distinct STT provider — the split-brain is already handled by stage.
- **Known gap (flag for follow-up, do NOT silently expand scope):** `activeReadinessProviderIds` (`useSettingsController.tsx:1149-1156`) only appends `realtime_agent.gemini_live` for native mode, never `realtime_agent.openai_realtime`. So when the user runs native with the OpenAI agent, the merged Credentials readiness view will not surface OpenAI-Realtime-agent readiness by-provider. Left as-is this is a pre-existing limitation, not introduced here. Call it out as an open question: fix now (append the openai realtime agent id when `converseRealtimeAgentProvider==="openai"`) or defer.

### 3.5 Effort: **M**. Risk concentrated in preserving the by-provider pivot and keeping tab ids stable so `SettingsPage.tsx:108-117` and `tab=credentials` deep-links resolve.

---

## Test impact

### `settingsRail.test.tsx`
- `renders an APG vertical tablist...` asserts `tabs.length >= 8` (`:77`) and group headers match `/settings.railGroups\./` — unaffected by regrouping (still 8 tabs, still 3 groups). No change unless a group becomes empty (it does not).
- `delegates arrow/Home/End...` and `Harness` default `activeTab="overview"` (`:27,43`) — unaffected by relabel (uses the id `overview`, not the label).
- No breakage expected; this test keys off ids and group-key prefixes, not labels.

### `useSettingsController.test.tsx`
- Currently narrow (OpenRouter discovery). ADD: unit tests for `handleSelectProductMode` — assert that selecting the native card calls `setConversationMode("converse")` + `setConverseEngine("native")` (mock the store), and that selecting a durable card sets `notes`/`pipelined` (per 1.4 scope). If durable provider-swap is deferred, assert the deferred behavior explicitly.

### `SettingsPage.test.tsx`
- Uses `screen.getByRole("tab", { name: /overview/i })` in several places (`:302,2947,3421,3469`). After relabel to "Modes", these `/overview/i` name matches BREAK — update to `/modes/i`. The `credentials` tab query `/credentials/i` (`:302`) still matches "Credentials & readiness"; `gemini` queries `/gemini/i` name lookups will BREAK (accessible name becomes "Realtime agent..."). Update those tab lookups to `/realtime agent/i` (the panel headings/content still say "Gemini Live", so `getByRole("heading",{name:/gemini live/i})` stays valid).
- `modeOverviewCard` helper looks up region `/product mode overview/i` (`:333-334`) — BREAKS after the title becomes `t("settings.modes.title")`="Modes". Update the region name matcher.
- Readiness-in-Overview assertions: many tests assert `.settings-readiness__item` and readiness rows while on the default (Overview) landing (`:314-327,1590,2945-2949,1910-2033,3183+`). After WS3 moves readiness into Credentials, these must first navigate to the Credentials tab (a `goToCredentials()` helper already exists at `:301-306`). The test at `:2945-2949` explicitly asserts readiness lives on Overview — that assertion must be inverted to Credentials.
- ADD: a test that the moved readiness list in Credentials still shows a provider with NO saved key (`missing_credentials`/`unchecked`) — guards the pivot-preservation constraint.
- ADD: mode-selector behavior test — clicking "Use this mode" on the native card flips the store and the card's `Selected` badge; on a durable card leaves native.

### `providerSetupModes.test.ts`
- Pure-function tests over `deriveProviderSetupModeCards`/`selectedModeId`. WS1/WS3 do not change these functions. Should stay GREEN. If 1.4's durable provider-swap is implemented, add cases asserting post-swap `selectedModeId` classification.

### `locale-parity.test.ts`
- Any key added/renamed/removed in en.json MUST be mirrored in pt.json in the same commit. Applies to: WS1 (`settings.modes.*`), WS2 (if deleting `sections.diagnostics`/`fields.backendLogLevel`/`hints.logLevel*`), WS3 (value renames do NOT affect parity, only key changes do).

### `LoggingSettings.test.tsx`
- Adding `off` to `LEVELS` — check whether any test asserts the exact option count/set; update to include `off`.

### `CredentialsManager.test.tsx`
- If it asserts the Diagnostics/log-level select exists, that test BREAKS (delete/rewrite). Grep for `log-level-select` / `backendLogLevel` / `diagnostics` before editing.

---

## Risks and open questions for the user

1. **Derived modes (local/cloud/hybrid) — selectable now or deferred?** Recommendation: DEFER (Phase 1 ships native↔durable only). Making the three durable cards truly selectable requires swapping ASR/LLM provider config + touching the settings reducer, dirty-tracking, and Save (M+).
2. **Should the "Modes" tab stay in the "setup" group or move?** Recommendation: keep in `setup`, relabeled "Modes" (it is the default landing; dropping it ripples into `useState("overview")` and arrow-nav).
3. **OpenAI Realtime agent readiness gap (3.4):** `activeReadinessProviderIds` never appends `realtime_agent.openai_realtime`. Fix now or accept the pre-existing limitation? Recommendation: defer, track separately.
4. **WS2 unused i18n keys:** leave in place (zero risk) or delete from both locales (tidy). Recommendation: leave in place this pass.
5. **Do NOT rename tab ids** (`overview`, `gemini`, `credentials`) — only i18n label values — to keep `SettingsPage.tsx:108-117` routing and `tab=…` deep-links resolving.

---

## Effort summary and suggested landing sequence

| WS | Effort | Sequence |
|----|--------|----------|
| WS2 Logging dedup | S | 1st (clears the duplicate-id noise) |
| WS1 Modes selector (native-only) | S (M if durable in scope) | 2nd (user-facing bug) |
| WS3 IA restructure | M | 3rd (soft-after WS1; both touch Overview) |
