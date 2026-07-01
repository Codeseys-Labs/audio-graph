# Settings redesign — bug root-cause, logging dedup, and IA restructure

Date: 2026-07-01
Scope: `src/components/settings/*`, `src/components/providerSetupModes.ts`, `src/components/ConversationModeControl.tsx`, `src/i18n/locales/{en,pt}.json`. Backend is untouched.

This answers four user asks: (1) the "stuck on Native realtime" product-mode bug, (2) the duplicate logging control, (3) moving provider-readiness into a renamed "Providers & Models" panel, (4) the `pipeline{stt,llm,tts} | native-s2s` provider grouping. Verdicts and a concrete change list are at the end.

---

## 1. BUG — "product mode is stuck on Native realtime; no way to select another mode"

### Root cause: the control is MISSING, not gated

The Settings > Overview "Product mode overview" cards (`ProductModeSummaryCards.tsx`) are a **read-only rollup, not a selector.** There is no too-strict gate to relax and no credential lock — there is simply no "select this mode" affordance rendered on the cards. Verified directly:

- The card is a plain `<article>` with **no `onClick`** (`ProductModeSummaryCards.tsx:53-60`). The `settings-mode-card--selected` class is cosmetic only.
- The `Selected` badge is **derived**, not interactive: `{card.selected && <Badge tone="accent">Selected</Badge>}` (`:81`), where `card.selected` comes from `selectedModeId()`.
- The only interactive elements are deep-link buttons that jump to config sections and never change the mode: stage-link (`:124`), Provider (`:166`), Credential (`:176`), Model (`:186`), and Sources recovery (`:196`) — all call `openSettingsControlRoute(...)` / `handleProviderSetupSourceRecovery(...)`.
- There is no `setConversationMode` / `setConverseEngine` anywhere in the component, and the controller memo that feeds it only *reads* the mode (`useSettingsController.tsx:1367-1390`).

**Why it always shows Native realtime as "Selected".** The classifier's first branch wins whenever the store is in converse+native (`providerSetupModes.ts:1164-1169`):

```ts
const nativeRealtimeSelected = runtimeModeProvided
  ? input.conversationMode === "converse" && input.converseEngine === "native"
  : input.nativeRealtimeEnabled === true;
if (nativeRealtimeSelected) return "native_realtime";
```

The controller always passes both runtime flags, so `native_realtime` is Selected exactly when the store is `converse` + `native`. The user most likely got there via one of two write paths: the legacy `localStorage["ag.nativeS2sEnabled"] === "true"` migration (`store/index.ts:2102-2132`), or completing Express Setup with Gemini Live (`ExpressSetup.tsx:709-711`), both of which hard-set converse+native. The Overview page then mirrors that state with no way to leave it.

Also note: only `native_realtime` is a clean two-flag toggle. The other three cards (`local_private`/`cloud_fast`/`hybrid`) are **derived from ASR/LLM provider locality** (`selectedModeId` lines 1171-1186), so "selecting" one of those means swapping the pipeline providers, not flipping a flag.

### The mode IS switchable today — just not here

The real selector already exists in the ControlBar: `ConversationModeControl.tsx` exposes Notes/Converse (`:68-88` → `setConversationMode`), then Pipelined/Native when in Converse (`:106-134` → `setConverseEngine`), then Gemini/OpenAI when Native (`:157-178`). `setConverseEngine` already keeps the legacy flag + backend in sync, so no backend change is needed to switch. This is a **discoverability bug**: Overview reads like a chooser but is a read-only summary.

### Concrete fix (recommended: make the cards selectable — Small)

1. Add a controller callback `handleSelectProductMode(card)` in `useSettingsController.tsx` and thread it into `ProductModeSummaryCards`:
   - `native_realtime` → `setConversationMode("converse")` + `setConverseEngine("native")`.
   - the three durable cards → `setConversationMode("notes")` (or converse+pipelined) **and** apply the card's local/cloud/hybrid provider selection so `selectedModeId()` re-classifies to the chosen card — a flag flip alone will not move between local/cloud/hybrid.
2. Render a "Use this mode" button on each `<article>` (or make the card `role="radio"` in a `radiogroup`), disabled + `aria-pressed` on the currently-selected card. Keep the existing deep-links.

If minimizing effort: at minimum add one line of copy + a link to the ControlBar selector so the cards stop reading as a dead chooser. But the durable/native distinction is genuinely a mode toggle users expect to set from Settings, so shipping the real selector is the right call.

---

## 2. LOGGING — duplicate log-level control

### Verdict: CONFIRMED true duplication

The log **verbosity/level** select appears in two panels, wired to two different Tauri commands that mutate the *same* backend state, and they even collide on the same DOM id `log-level-select`.

- **Settings > App > General** renders `<CredentialsManager>`, whose "Diagnostics" section contains a `Backend Log Level` `<select id="log-level-select">` (`CredentialsManager.tsx:306-336`) → `handleLogLevelChange` → `setField("logLevel", …)` + `invoke("set_log_level", …)` (`useSettingsController.tsx:3230-3237`). Options: **off / error / warn / info / debug / trace**.
- **Settings > App > Logging** renders `<LoggingSettings>`, whose "Logging" section has a `Log level` `<select id="log-level-select">` (`LoggingSettings.tsx:186-202`) → `apply({ level })` → `invoke("set_logging_config", …)`. Options: **error / warn / info / debug / trace** (no `off`).

Both call `crate::logging::apply_log_level(...)` and both write `state.app_settings.log_level` — same in-memory field, same on-disk `config.yaml log_level`. They hold separate frontend state (reducer `logLevel` vs LoggingSettings-local `info.level`), so they can display conflicting values in one session, and the shared DOM id is invalid HTML.

### Exactly what to remove, and from where

**Keep the Logging panel as the canonical home** (it groups level with the file-logging enable/mode/purge controls that only make sense together, persists immediately, and self-refreshes via `get_log_info`). Remove the General/CredentialsManager copy:

1. Delete the "Diagnostics" `<FieldRow>`/`<select>` block in `CredentialsManager.tsx:306-336` (the level select is its only content, so the whole section goes).
2. Prune the now-dead wiring: `handleLogLevelChange` prop threading (`GeneralPanel.tsx:28,108`; `CredentialsManager.tsx:177,195,325`), the `logLevel` destructure (`CredentialsManager.tsx:197`) and the `Pick<... "logLevel">` (`:168`); and `handleLogLevelChange` in `useSettingsController.tsx:3230-3237` + its export (`:3373`) if no other consumer remains.
3. **Before deleting**, add `"off"` to `LEVELS` in `LoggingSettings.tsx:42` — the surviving control lacks `off`, so that capability would otherwise be lost.

Do **not** touch the `logLevel` reducer field, the `set_log_level` command, or `save_settings_cmd`'s `log_level` payload — those are still used for startup persistence. Only the redundant UI + its dedicated handler go. Removing one select also fixes the duplicate-id automatically.

The "Privacy & Diagnostics" (Sentry) block in LoggingSettings is unrelated and stays.

---

## 3 & 4. IA — "Providers & Models" merge + `pipeline | native-s2s` split

### Verdict: YES, it makes sense better — it is literally the codebase's own architecture (ADR-0006 B1). Ship it with three refinements.

The proposed split maps onto a real, first-class seam in the code: `ProviderStage = asr | diarization | llm | tts | realtime_agent` (`types/index.ts:850-855`, "matches Rust ProviderStage"). ADR-0006 decision B1 says verbatim: *"Settings UI splits cleanly: STT/LLM/TTS (pipeline) and Realtime Voice Agent (RealtimeAgent)."* The proposal is that decision. It will not fight the domain model. But four code facts require refinements before it's fully faithful:

**Refinement A — the right branch is "Realtime agent (native S2S)", and it must host BOTH agents, not just Gemini.** The current `gemini` tab is already a misnomer: `GeminiPanel.tsx` renders the native-S2S toggle (`handleNativeRealtimeToggle`) **and** `<ProviderCapabilityStageSection stage="realtime_agent" />` (`:72`), which enumerates *all* `realtime_agent` providers. The registry has two, both implemented: `realtime_agent.gemini_live` and `realtime_agent.openai_realtime` (`providerRegistry.ts:2755,2844`). So the node should be labeled "Realtime agent" / "Native S2S", not "gemini" and not "Gemini Live only". This is a free win — you're fixing an existing misnomer, not inventing scope.

**Refinement B — do not let the "pipeline" label imply "not conversation".** STT/LLM/TTS are NOT notes-only. When `conversationMode==="converse" && converseEngine==="pipelined"`, the very same STT→LLM→TTS trio *is* one of the two converse engines (`useConverseFrontLeg.ts:10-33`; ADR-0018 treats "pipelined" and "native" as peer engines under one FSM). Frame it as "Pipeline (STT → LLM → TTS)" — a composed path used by both notes and pipelined-converse — rather than opposing it to "conversation".

**Refinement C — STT (and OpenAI Realtime) straddle the line; call it out, don't hide it.** `OpenAI Realtime` exists as two distinct registry providers: `asr.openai_realtime` (a pipeline STT provider) and `realtime_agent.openai_realtime` (the voice agent). And native agents feed transcripts back into the same temporal graph the pipeline builds (`gemini/mod.rs:817-841`). Any IA that files provider *names* into one branch will mis-file OpenAI Realtime. File by **stage**, which the code already does, and the problem disappears. (Also: `diarization` is a fifth stage the proposal omits — fine to leave under STT's advanced disclosure as today, just don't claim `{stt,llm,tts}` is the whole pipeline.)

### The merge (readiness → renamed Credentials panel): YES, low-risk

Overview's provider-readiness section (`OverviewPanel.tsx:41-175`) and CredentialsPanel are two projections of the **same** `providerReadinessEntries` array — Overview by-provider, Credentials by-key — with shared status vocabulary, shared `refreshProviderReadiness`, and shared source/last-checked facts. Merging removes genuine duplication. Two things to preserve:

- **The pivot difference:** Overview surfaces providers with *no saved key yet* (`missing_credentials`/`unchecked`); CredentialsPanel only lists present keys (`savedCredentialEntries` filters `present===true`). Keep the by-provider readiness so "here's a provider you haven't set up" doesn't vanish.
- **Rename the label only, keep the tab id `credentials`.** The label is `settings.tabs.credentials` in en.json (`:538`) + pt.json (parity enforced by `locale-parity.test.ts`). The tab **id must stay `credentials`** so `SettingsPage.tsx:110`, the rail, and any `tab=credentials` deep-links keep resolving. Also refresh the in-panel `credentialHealth.title/.help` copy.

### What happens to Overview

After readiness moves out, Overview holds only `<ProductModeSummaryCards />` (which, per §1, should become the mode *selector*). That's a legitimate standalone concern — mode selection + per-stage rollup — and it's the natural home for the bug-fix selector. **Recommendation: keep Overview, rename it "Modes" (or "Get started"), and make its cards selectable.** That turns the readiness move into a clean separation of concerns: Overview = pick your mode; Providers & Models = configure providers + credential/readiness health. Do not delete the `overview` tab (it's the default landing tab and dropping it ripples into the initial-tab/default-route logic).

---

## Proposed rail structure

The rail already supports two-level grouping via `RailGroup` (`setup | providers | app`). The `pipeline > {stt,llm,tts}` nesting is expressed as a rail *group*, not a new nesting mechanism — the panel switch in `SettingsPage.tsx:108-117` stays one-panel-per-tab. Concretely, `RAIL_SECTIONS` becomes:

```ts
// SettingsTab union: rename group label "providers" -> "Providers & Models";
// rename the "gemini" LABEL to "Realtime agent" (KEEP id "gemini" to preserve routes).
export const RAIL_SECTIONS: RailSection[] = [
  { id: "overview",    labelKey: "settings.tabs.overview",    group: "setup" },     // now "Modes": selectable mode cards
  // --- Providers & Models group ---
  { id: "stt",         labelKey: "settings.tabs.stt",         group: "providers" }, // Pipeline
  { id: "llm",         labelKey: "settings.tabs.llm",         group: "providers" }, // Pipeline
  { id: "tts",         labelKey: "settings.tabs.tts",         group: "providers" }, // Pipeline
  { id: "gemini",      labelKey: "settings.tabs.gemini",      group: "providers" }, // -> label "Realtime agent (native S2S)"; hosts Gemini Live + OpenAI Realtime
  { id: "credentials", labelKey: "settings.tabs.credentials", group: "providers" }, // -> label "Credentials & readiness"; absorbs Overview provider-readiness
  // --- App group ---
  { id: "general",     labelKey: "settings.tabs.general",     group: "app" },
  { id: "logging",     labelKey: "settings.tabs.logging",     group: "app" },       // sole home of log level
];
```

Group render order: `setup → providers → app` (unchanged); rename the `providers` group header (`settings.railGroups.providers`) to "Providers & Models". Optionally split the group visually into a "Pipeline" subhead (stt/llm/tts) and the realtime-agent + credentials rows — but that's a presentational tweak in `settingsRail`, not a data change. `credentials` moves out of the `app` group into `providers`.

---

## Ordered change list

These three are **independent** and can ship in any order / separate PRs:

| # | Change | Effort | Notes |
|---|--------|--------|-------|
| 1 | **Bug fix — make mode cards selectable.** Add `handleSelectProductMode` in the controller + "Use this mode"/radiogroup on each card; native = two-flag set, durable = flag + provider-locality apply. | **S–M** | S if native-only toggle; M to correctly select among local/cloud/hybrid (needs provider swap). Self-contained; no backend. |
| 2 | **Logging dedup.** Add `"off"` to `LoggingSettings` LEVELS, delete the CredentialsManager Diagnostics select + prune dead `handleLogLevelChange` wiring. | **S** | Independent. Keep the reducer field/command for persistence. |
| 3 | **IA restructure.** Move Overview provider-readiness into CredentialsPanel; rename `credentials` label → "Credentials & readiness", `gemini` label → "Realtime agent", `providers` group → "Providers & Models"; regroup `credentials` under `providers`; reorder stt/llm/tts/gemini. Update en.json + pt.json (parity test). | **M** | Panels share the controller context so markup moves cheaply; risk is preserving the by-provider pivot + keeping tab **ids** stable so deep-links resolve. |

Suggested sequence: 2 (trivial, unblocks nothing but clears noise) → 1 (bug, user-facing) → 3 (IA). 1 and 3 touch Overview, so do them in that order to avoid a rebase, but they are not hard-coupled.
