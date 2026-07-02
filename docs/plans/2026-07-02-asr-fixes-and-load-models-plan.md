# ASR Fixes & Load-Models Button — Action Plan

**Date:** 2026-07-02
**Status:** DECISION-GRADE PLAN — synthesized from 3 investigation reports, cited files spot-checked against HEAD.
**Sources:** `/tmp/cred-save-rootcause.md`, `/tmp/model-ids-investigation.md`, `docs/plans/2026-07-02-load-models-button-design.md`
**Verified:** all cited file:line anchors below were read and confirmed accurate.

---

## Executive summary

Two lanes:

- **IMMEDIATE FIXES** — the user's active 401/invalid_model blockers. Three items, all small, all independent of each other and of the feature.
  1. **Credential-save bug** — inconclusive by static analysis (class **D**), but the leading mechanism is a frontend empty-string guard skip. Ship a cheap diagnostic log + a no-rebuild Express-Setup cross-check to decide, then a ~1-line fix.
  2. **OpenAI Realtime `invalid_model`** — NOT a model-name problem. The model `gpt-realtime-whisper` is correct. The bug is `?model=` in the WebSocket URL; fix is one line + one test.
  3. **Deepgram default model** — no correctness fix needed. `nova-3` and `nova-3-general` are identical aliases. The observed Deepgram 401 is auth, not model.
- **FEATURE** — a uniform "Load models" button across provider tabs, reusing the existing LLM-tab (Cerebras/OpenRouter) pattern. Effort **M**. Independent of the immediate fixes, but mitigates #2 and #3 by letting users pick a valid model from the account's real catalog.

**Sequencing:** the three IMMEDIATE FIXES are mutually independent and can land in any order / in parallel. The FEATURE is independent of all three (it does not block or depend on them), but should land after the OpenAI URL fix so the realtime path is already correct when model discovery arrives.

---

## LANE 1 — IMMEDIATE FIXES

### 1A. Credential-save bug (the active blocker) — Class D, leading mechanism frontend empty-string skip

**Symptom:** user saves a new Deepgram key in Settings; log shows credential *reads* but zero save/set lines; old key still on disk; app keeps sending the old key and gets 401.

**Root cause (verified).** The redesigned Settings has **no per-credential Save button**. The Deepgram key persists only as a side-effect of the global footer **Save** → `handleSave` → `saveCredentialIfPresent("deepgram_api_key", …)` → `invoke("save_credential_cmd")`. That path has two silent early-exit guards that each reproduce the exact symptom:

1. **Frontend empty-value guard** — `saveCredentialIfPresent` returns with no invoke and no log when `!value.trim()`.
   - `src/components/settings/useSettingsController.tsx:301` — `if (!value.trim()) return;` (confirmed).
2. **Provider gate forces empty** — the Deepgram arm only forwards a non-empty value when `asrType === "deepgram" || ttsType === "deepgram_aura"`; otherwise it forwards `""`, which trips guard #1.
   - `src/components/settings/useSettingsController.tsx:2937-2942` (confirmed).

Two contributing conditions make the field empty at save time:
- **Hydrate blanks the field** every time Settings opens: `patch.deepgramApiKey = "";` at `useSettingsController.tsx:2686` (confirmed). The IPC `settings` object is redacted, so the form never rehydrates the stored secret. On open, `deepgramApiKey === ""`.
- **Edit-gated input** — `SecretCredentialControl` starts with `editing = hasDraft` (false for a blank value); `showInput = editing || hasDraft`. When a key is already saved, it shows a **"Replace"** button and the `<input>` only appears after the user clicks it. `src/components/SecretCredentialControl.tsx:53-57` (confirmed). So `deepgramApiKey` stays `""` unless the user clicks Replace AND types in the current session.

Net: if the user typed the key anywhere that did not land in `deepgramApiKey` (wrong provider tab, or expected the credential-health "Replace" row to be editable in place), the global Save silently sends `""` and skips the invoke — producing "zero save lines."

**Why Class D (inconclusive), not a clean A/B/C:**
- **Not clean A** (wiring lost): the STT-tab field + `handleSave` invoke path exists and is exercised by tests. What regressed is *discoverability* — the "Credentials & readiness" tab rows are status-only; the editable field lives a tab away on STT.
- **Not classic B** (masked value sent back): the input is a real `type="password"` bound to `deepgramApiKey` with no masked pre-fill, so "send the mask back" is impossible. The best-fit mechanism is the *empty-string* skip, which is B-adjacent.
- **Not C** (backend silent fail): no path was found where a non-empty value succeeds without writing. `save_credential_cmd` (`src-tauri/src/commands.rs:6319-6335`) has no success info-log, so "zero save lines" is *also* consistent with a successful invoke that simply doesn't log — a genuine diagnostic gap.
- The keyring "get password" reads in the log imply the **OS keychain is the live backend** (`file_backend == false`), which reframes the stale `credentials.yaml` mtime as a **non-signal**: a successful keychain write leaves yaml untouched anyway (yaml is only the fallback path, `src-tauri/src/credentials/mod.rs:1007-1026`).

**Classification: `D_inconclusive`** — two indistinguishable states both fit the log ("invoke never fired" vs "invoke fired, wrote to keychain, but has no success log"). A decisive test separates them.

**Decisive test (do this first — cheap, no user rebuild for the cross-check):**
1. **No-rebuild cross-check:** have the user save the SAME Deepgram key via **Express Setup** (known-wired: `src/components/ExpressSetup.tsx:617`). If that persists and clears the 401, the defect is isolated to the Settings-panel path (frontend A/B), not the backend.
2. **Diagnostic log (one rebuild):** add `log::info!("save_credential_cmd: key={key} len={}", value.len())` (length only, never the secret) at `commands.rs:6320`, plus an info log at the backend empty-skip (`mod.rs:438` / `mod.rs:798`). Rebuild, repeat the save, read the log:
   - **No `save_credential_cmd` line** → frontend guard skipped the invoke → confirmed A/B; the key never reached `deepgramApiKey`. **Fix is frontend.**
   - **`save_credential_cmd len=40` line appears but key still 401** → backend/keychain persistence or wrong-key-at-runtime → confirmed C-adjacent.

**Minimal fix (once the test points to A/B — the leading path):**
- Primary UX fix: make the credential-health rows editable in place, OR ensure the empty-string skip is surfaced (a visible "no change saved" state) so a silent no-op is impossible. The cheapest correctness fix is to add the missing **success info-log** regardless (closes the diagnostic gap permanently), then address discoverability.
- If the cross-check shows the STT-tab Replace→type→Save flow works, the fix is purely making the editable field reachable from where users look (the "Credentials & readiness" tab), not backend.

**Effort: S** (diagnostic log + likely a small frontend/UX fix). Independent of 1B/1C and the feature.

---

### 1B. OpenAI Realtime `invalid_model` — URL bug, not a model-name bug — Model id: `gpt-realtime-whisper`

**Verdict:** the model id `gpt-realtime-whisper` is **correct and requires no change**. The `invalid_model` error comes from putting `?model=gpt-realtime-whisper` in the WebSocket upgrade URL. OpenAI's native `/v1/realtime` treats `?model=` as selecting a **conversation** (speech-to-speech) session; when the subsequent `session.update` arrives with `type: "transcription"`, the server rejects with `invalid_model` ("not supported in transcription mode") and closes 4000.

**Correct URL:** `wss://api.openai.com/v1/realtime?intent=transcription` (no `?model=`). The model is conveyed solely via `session.update → session.audio.input.transcription.model`.

**Doc source:** OpenAI Realtime Transcription guide (`developers.openai.com/api/docs/guides/realtime-transcription`) + model page (`developers.openai.com/api/docs/models/gpt-realtime-whisper`). Cross-confirmed by LiveKit agents-js issue #1756 (same `invalid_model` bug, 2026-06-11) fixed by PR #1767.

**Exact changes (all verified):**
1. **`src-tauri/src/asr/openai_realtime.rs:536`** — the bug.
   - Current: `format!("wss://api.openai.com/v1/realtime?model={model}")`
   - Change to: `"wss://api.openai.com/v1/realtime?intent=transcription".to_string()`
   - The `model` param becomes unused by the URL; the model is already correctly injected into the `session.update` payload at `openai_realtime.rs:510` (`json!({ "model": config.model })` → `session.audio.input.transcription.model`) — **leave line 510 as-is**. (Confirmed: 510 is the transcription payload; 536 is the URL.)
2. **`src-tauri/src/asr/openai_realtime.rs:1590-1596`** — test `realtime_url_carries_model` asserts the old buggy URL. Update it to assert `?intent=transcription` with no `?model=`, and rename it (e.g. `realtime_url_uses_transcription_intent`) since it no longer carries the model. (Confirmed.)

**Constants — NO change (both correct, keep as-is):**
- `src-tauri/src/asr/openai_realtime.rs:70` — `pub const DEFAULT_MODEL: &str = "gpt-realtime-whisper";` (confirmed).
- `src-tauri/crates/provider-registry/src/lib.rs:63` — `pub const OPENAI_REALTIME_TRANSCRIPTION_DEFAULT_MODEL: &str = "gpt-realtime-whisper";` (confirmed).

**The URL-vs-transcription.model distinction (the crux):** the model must live in exactly ONE place — the `session.update` `transcription.model` field (line 510), NOT the URL (line 536). The current code duplicates it into both; the URL copy is what OpenAI rejects.

**Effort: S** (1-line fix + 1 test update). Independent.

---

### 1C. Deepgram default model — no fix needed — Verdict: `nova-3` and `nova-3-general` are identical aliases

**Verdict:** both `nova-3` and `nova-3-general` are valid Deepgram streaming-API model strings and are **exact aliases** for the same underlying weights. The Deepgram model-metadata API reports `name: "general"`, `canonical_name: "nova-3-general"` for the general Nova-3 model; the docs use `model=nova-3` as shorthand. **No correctness change is required for either string.**

**Doc source:** Deepgram Model Options (`developers.deepgram.com/docs/model`) + Model Metadata API (`developers.deepgram.com/guides/fundamentals/model-metadata`). The observed 401 is an **auth failure** — Deepgram never evaluates the model string when the key is rejected. Fixing 1A resolves the 401.

**Where the default lives (no change needed; for reference):**
- `src-tauri/crates/provider-registry/src/lib.rs:1532` — `default_model: Some("nova-3")` (confirmed).
- `src-tauri/src/provider_registry.rs:87` — `model: "nova-3".to_string()`.
- `src-tauri/src/commands.rs:8346` — `is_default: id == "nova-3"`.

The `nova-3-general` seen in logs comes from the user's saved config or the catalog fetch, not a hardcoded default. **Recommendation: leave `nova-3` as the default.** If unifying strings is ever desired for log consistency, changing the default to `nova-3-general` is safe but cosmetic — not part of this fix set.

**Effort: none (verification only).** Independent.

---

## LANE 2 — FEATURE: uniform "Load models" button

**Goal:** add a "Load models" (refresh catalog) button to every provider tab that can offer a live catalog, modeled exactly on the LLM tab (Cerebras/OpenRouter). Turns a blind free-typed / single-default model choice into a pick from the account's real catalog.

### Backend command inventory (verified against `commands.rs` + `lib.rs`)

| Provider | Backend `list_*_models_cmd` | Status |
|---|---|---|
| llm.cerebras | `list_cerebras_models_cmd` (`commands.rs:8565`) | Exists + registered (`lib.rs:505`) — **reference impl, has UI button** |
| llm.openrouter | `list_openrouter_models_cmd` (`commands.rs:8779`) | Exists + registered (`lib.rs:507`) — **reference impl, has UI button** |
| llm.api (OpenAI-compat) | `list_openai_compatible_llm_models_cmd` (`commands.rs:8291`) | Exists + registered (`lib.rs:506`) — **no UI button yet** |
| asr.deepgram | `list_deepgram_models_cmd` (`commands.rs:8519`) | Exists + registered (`lib.rs:503`) — **no UI button yet** |
| asr.soniox | `list_soniox_models_cmd` (`commands.rs:8539`) | Exists + registered (`lib.rs:504`) — **no UI button; provider is `planned`, no block surfaced yet** |
| asr.api (OpenAI-compat STT) | none (could reuse the llm-compat cmd) | Optional — flip descriptor to `remote_command` |
| ~14 fixed providers (openai_realtime, assemblyai, gladia, speechmatics, aws_transcribe, tts.*, realtime_agent.*, …) | none | Static catalogs — curated dropdown, no API button |

**Key finding: no new backend command is required to match the LLM tab.** Deepgram and Soniox already have working, registered commands that no UI ever calls. The gap is almost entirely **frontend wiring**.

### Why the catalog is empty without a button (verified)

The readiness probe seeds `ProviderReadiness.model_catalog` via `fixed_model_catalog_for_descriptor` (`commands.rs:6529-6571`), which returns entries only for `Fixed`/`LocalFiles` policies and falls through to `vec![]` for `RemoteCommand`/`UserSupplied` — deliberately, so opening Settings never fans out live per-provider API calls. Consequence: `remote_command` providers (Deepgram, Soniox, llm.api) show only their `default_model` (e.g. Deepgram picker = just `["nova-3"]`) until the user clicks Load models. Confirmed: Deepgram's descriptor declares `model_catalog: RemoteCommand` + `model_catalog_command: Some("list_deepgram_models_cmd")` (`provider-registry/src/lib.rs:1529,1534`), yet its mount point `AsrProviderSettings.tsx:599-608` renders `<ModelCatalogPicker>` with no refresh button (confirmed).

### Shared UI approach (reuse, do not reinvent)

Everything needed exists: the shared `ModelCatalogPicker` (already mounted on every relevant block), the `ProviderModelCatalogItem` type (all cmds return `Vec<ProviderModelCatalogItem>`), the registry seam (`descriptor.model_catalog === "remote_command"` + `model_catalog_command`), and the i18n button keys.

**Recommended shape:**
1. Extract a presentational `ModelCatalogField` = `<ModelCatalogPicker>` + refresh `<button className="settings-btn settings-btn--secondary">` + tri-state status line, driven by props `{ value, onChange, catalog, loading, error, credentialAvailable, onRefresh, hasRemoteCommand }`. When `hasRemoteCommand === false` it renders the picker only — so the same component serves API-backed and curated-static providers. Migrate Cerebras/OpenRouter onto it first to prove parity (the reference handlers are `handleRefreshCerebrasModels`/`handleRefreshOpenRouterModels`, confirmed at `LlmProviderSettings.tsx:562,708`).
2. Generalize the controller: a `liveCatalog: Record<providerId, ProviderModelCatalogItem[]>` map (generalizing today's single `cerebrasModels` useState), a generic `handleRefreshModels(providerId)` that looks up `descriptor.model_catalog_command`, resolves the per-provider arg shape (Deepgram/Soniox `{ apiKey }`; llm.api/asr.api `{ endpoint, apiKey }`; OpenRouter `{ apiKey, baseUrl }`) via a small arg-builder map, invokes, and stashes keyed by provider id. Per-provider catalog memos overlay `liveCatalog[id] ?? readinessCatalog ?? generatedCatalog`.
3. Genericize the i18n triplet (`modelsFailed`/`modelsLoading`/`noModels` with a provider-name interpolation) instead of N Cerebras/OpenRouter copies.

**Mount points to touch:** Deepgram `AsrProviderSettings.tsx:599-608`; llm.api `LlmProviderSettings.tsx:440-452`; optionally flip asr.api (`AsrProviderSettings.tsx:325`) to `remote_command` → `list_openai_compatible_llm_models_cmd`. Static providers render the field with `hasRemoteCommand=false`.

**Tests:** mirror `src/components/SettingsPage.test.tsx` (Cerebras `:5200-5325`, OpenRouter `:5819-6078`) — assert the Deepgram / llm.api button invokes the right cmd with the right args and that empty/error states render.

### Build order (from the design doc)
1. Extract `ModelCatalogField`, migrate Cerebras/OpenRouter (prove parity, no behavior change).
2. Generalize controller (`liveCatalog` map + generic handler + arg-builder + i18n).
3. Wire **Deepgram** (highest value, cmd exists).
4. Wire **llm.api** + flip **asr.api** to `remote_command` → this is what actually mitigates OpenAI-compat `invalid_model` for the file/compat path.
5. Populate `fixed_model_catalog` for static providers so their dropdowns list more than one option; surface Soniox when it graduates from `planned`.
6. (Optional) Add `list_gemini_models_cmd` for the realtime/Gemini tab (`test_gemini_api_key` already fetches `/v1beta/models` but discards the list, `commands.rs:8677`).

**Synergy with Lane 1:** wiring Deepgram surfaces the account's actual streaming models (mitigates 1C — a user lacking `nova-3` access can pick a valid one). Wiring llm.api/asr.api turns free-typed OpenAI-compatible model strings into a pick from `/v1/models` (mitigates the general `invalid_model` class, distinct from the realtime URL bug in 1B).

**Effort: M** — no backend work for the core ASR scope; the cost is the shared-component extraction + controller generalization + tests. Independent of Lane 1 (does not block or depend on any immediate fix; best landed after 1B).

---

## Grouped effort & independence summary

| Item | Lane | Effort | Independent? | Fix |
|---|---|---|---|---|
| 1A Credential-save | IMMEDIATE | S | Yes | Class D → run decisive test (Express-Setup cross-check + diagnostic log), then ~1-line frontend/UX fix; add missing success info-log regardless |
| 1B OpenAI `invalid_model` | IMMEDIATE | S | Yes | `openai_realtime.rs:536` → `?intent=transcription` (drop `?model=`); update test `:1590-1596`; model id `gpt-realtime-whisper` unchanged |
| 1C Deepgram default | IMMEDIATE | none | Yes | No change; `nova-3` == `nova-3-general`; 401 was auth |
| 2 Load-models button | FEATURE | M | Yes (best after 1B) | Extract `ModelCatalogField` + generic controller handler; wire Deepgram + llm.api; no new backend cmd needed for ASR scope |

**Do first:** 1B (clean 1-line win, unblocks realtime) and 1A's no-rebuild Express-Setup cross-check (decides the blocker cheaply). 1C is verification-only. The feature follows independently.
