# Load / Refresh available models — uniform "Load models" button across all providers

**Date:** 2026-07-02
**Status:** DESIGN ONLY — no implementation in this pass.
**Lane:** load-models
**Scope:** Add a "Load models" (refresh catalog) button to every provider tab that can offer one, modeled on the existing LLM tab (Cerebras / OpenRouter). Read-only investigation; this document specifies WHAT to build, not the change itself.

---

## 1. TL;DR

The LLM tab already has the exact feature the user wants — for **Cerebras** and **OpenRouter** only. The button calls a backend `list_*_models_cmd`, populates a live catalog, and feeds it into the shared `ModelCatalogPicker`. The plumbing (backend commands, a per-descriptor `model_catalog_command` field, a `ProviderModelCatalogItem` type, cache/error/loading state pattern) already exists.

The gap is almost entirely **frontend wiring**: two ASR providers (**Deepgram**, **Soniox**) already have registered, working backend list-models commands (`list_deepgram_models_cmd`, `list_soniox_models_cmd`) that **no UI button ever calls**. Their model pickers today only show the single hardcoded `default_model` (e.g. `nova-3`) because the readiness probe does not run `remote_command` catalogs — only fixed/local catalogs (see §4). So the picker's "catalog" for Deepgram is effectively `["nova-3"]`.

For the ~14 `fixed`-catalog ASR/TTS/realtime providers, the correct "button" is **not** an API call — their model set is static. Those get a curated dropdown (already the case) and either no button or an optional inline "these are the known models" note.

---

## 2. The existing pattern (LLM tab), end to end

The canonical "load models" flow lives in the LLM tab for Cerebras and OpenRouter.

### 2.1 UI — the button + picker + status line

`src/components/LlmProviderSettings.tsx`:

- **Cerebras block, lines 539–584.** A `<ModelCatalogPicker>` (`src/components/ModelCatalogPicker.tsx`) sits in a `settings-inline-row` next to a `<button className="settings-btn settings-btn--secondary">` whose label toggles `t("settings.buttons.refreshing")` / `t("settings.buttons.refreshModels")` ("Loading…" / "Load models" — `src/i18n/locales/en.json:211-212`). The button is `disabled={cerebrasModelsLoading || !cerebrasCredentialAvailable}` and its `onClick` is `handleRefreshCerebrasModels`. Below it, a tri-state status line renders `settings.errors.cerebrasModelsFailed` (error) / `settings.hints.cerebrasModelsLoading` (loading) / `settings.hints.cerebrasNoModels` (empty catalog).
- **OpenRouter block, lines 683–730.** Same shape, `onClick={handleRefreshOpenRouterModels}`, gated on `openrouterModelsLoading || !openrouterCredentialAvailable`, with `openrouterModelsFailed` / `openrouterModelsLoading` / `openrouterNoModels` status keys.

The picker itself (`ModelCatalogPicker.tsx`) is a combobox: it takes `catalog: ProviderModelCatalogItem[]`, allows a free-typed custom value, filters as you type, and marks the `is_default` entry. It is already reused by EVERY provider block (LLM api/cerebras/openrouter/mistralrs, ASR api/openai_realtime/deepgram/sherpa, TTS aura voice). The button is the only thing missing on the non-LLM ones.

### 2.2 Controller — the fetch handler

`src/components/settings/useSettingsController.tsx`:

- `handleRefreshCerebrasModels` (lines 2572–2588): guards on `cerebrasCredentialAvailable`, sets loading, `invoke<ProviderModelCatalogItem[]>("list_cerebras_models_cmd", { apiKey: llmApiKey.trim() || null })`, stashes result in local `cerebrasModels` state (`useState`, line 1068), on error sets `cerebrasModelsError`.
- `handleRefreshOpenRouterModels` (lines 2466–2501): same, but with a **freshness cache** (`OPENROUTER_MODELS_CACHE_TTL_MS` + `openRouterModelsCacheKey`) so toggling the radio repeatedly does not re-hit the API, and it dispatches into reducer state (`SET_OPENROUTER_MODELS`) rather than local `useState`.
- The catalog that reaches the picker is a `useMemo` overlay: `cerebrasModelCatalog` (lines 1298–1304) = live-fetched `cerebrasModels` **if non-empty**, else the readiness-snapshot catalog. `modelCatalogForProvider(providerReadiness, providerId)` (`src/components/providerRegistryHelpers.ts:201-215`) is the fallback source.

### 2.3 Backend — the command

`src-tauri/src/commands.rs`:

- `list_cerebras_models_cmd` (line 8565): resolves the API key from draft-or-store, calls `fetch_openai_compatible_model_catalog_with_default(CEREBRAS_BASE_URL, …)`.
- `list_openrouter_models_cmd` (line 8779): resolves key, calls `openrouter::list_models`.
- All are registered in the Tauri invoke handler (`src-tauri/src/lib.rs:503-509`).

### 2.4 Registry — the declarative source of truth

Each provider descriptor already declares HOW its catalog is sourced. In `src-tauri/crates/provider-registry/src/lib.rs` (generated into `src/generated/providerRegistry.ts`, TS type `ProviderDescriptor` in `src/types/index.ts:1156-1161`):

- `model_catalog: ModelCatalogPolicy` — one of `none | fixed | local_files | remote_command | user_supplied`.
- `model_catalog_command: Option<&str>` — the `list_*_models_cmd` to invoke when policy is `remote_command` (or `list_available_models` for local files).
- `fixed_model_catalog: Option<&[ProviderModelCatalogItem]>` — an inline static list (used by e.g. `tts.deepgram_aura` voices, `llm.cerebras`).

This field is the seam the uniform button should key off: **"show a Load-models button iff `model_catalog === 'remote_command'` and `model_catalog_command` is set."**

---

## 3. Gap analysis — provider → {backend cmd exists?, UI button exists?}

Extracted from `src/generated/providerRegistry.ts` (authoritative). Only `implemented`/`planned` rows shown for the tabs in scope; the ~10 `watch`/`enterprise_watch` ASR rows are not surfaced in the UI yet and are omitted (they are all `fixed`).

| Provider (descriptor id) | Tab | `model_catalog` policy | Backend list-models cmd | Cmd registered & working? | UI "Load models" button? | Gap |
|---|---|---|---|---|---|---|
| **llm.cerebras** | LLM | remote_command | `list_cerebras_models_cmd` | Yes (`commands.rs:8565`) | **Yes** (`LlmProviderSettings.tsx:558`) | — reference impl |
| **llm.openrouter** | LLM | remote_command | `list_openrouter_models_cmd` | Yes (`commands.rs:8779`) | **Yes** (`LlmProviderSettings.tsx:702`) | — reference impl |
| **llm.api** (OpenAI-compat) | LLM | remote_command | `list_openai_compatible_llm_models_cmd` | Yes (`commands.rs:8291`) | **No** — picker only | Add button |
| **asr.deepgram** | STT | remote_command | `list_deepgram_models_cmd` | **Yes** (`commands.rs:8519`, registered `lib.rs:503`) | **No** | **Add button — cmd already exists** |
| **asr.soniox** | STT | remote_command | `list_soniox_models_cmd` | **Yes** (`commands.rs:8539`, registered `lib.rs:504`) | **No** | **Add button — cmd already exists** (provider is `planned` status) |
| **asr.api** (OpenAI-compat STT) | STT | user_supplied | (none; `list_openai_compatible_llm_models_cmd` is LLM-shaped) | n/a | No | Optional: reuse the LLM-compat cmd (see §5) |
| **asr.openai_realtime** | STT | fixed | — | n/a | No | Static set → curated dropdown, no API button |
| **asr.assemblyai** | STT | fixed | — | n/a | No | Static set (no model picker at all today) |
| **asr.aws_transcribe** | STT | fixed | — | n/a | No | Static (`transcribe-streaming`) |
| **asr.sherpa_onnx** | STT | local_files | `list_available_models` | Yes (local) | No (uses local catalog) | Local model dir — different flow |
| **asr.gladia / speechmatics / elevenlabs_scribe / revai / google_chirp3 / xai_grok_stt / …** | STT | fixed | — | n/a | No | Static → curated dropdown only |
| **tts.deepgram_aura** | TTS | fixed | — (voices via `fixed_model_catalog`) | n/a (inline static) | No | Voices are a curated static list; a Deepgram `/v1/models` TTS-voice fetch does not exist |
| **realtime_agent.gemini_live** | Realtime | fixed | — | n/a | No | Static; `test_gemini_api_key` calls listModels but discards the list |
| **realtime_agent.openai_realtime** | Realtime | fixed | — | n/a | No | Static (`gpt-realtime-2`) |

### 3.1 Deepgram specifically (the user asked)

`list_deepgram_models_cmd` **exists, is registered, and works** — it calls Deepgram `GET /v1/models`, filters to streaming STT models, dedupes, and marks `nova-3` as default (`commands.rs:8315-8353,8519`). The descriptor `asr.deepgram` declares `model_catalog: remote_command` + `model_catalog_command: list_deepgram_models_cmd` (`providerRegistry.ts:278-282`). **But the STT deepgram block (`AsrProviderSettings.tsx:577-608`) renders only the `<ModelCatalogPicker>` with `catalog={deepgramModelCatalog}` and NO refresh button.** Because the readiness probe never runs `remote_command` catalogs (§4), `deepgramModelCatalog` is empty from readiness and falls back to `generatedModelCatalogForProvider("asr.deepgram")` → the single `nova-3` default. So the user cannot pick any other valid Deepgram model from the picker's dropdown. Wiring a button that calls the existing command is a ~1-block change mirroring Cerebras.

---

## 4. Why the catalog is empty without a button (load-time behavior)

`fixed_model_catalog_for_descriptor` (`commands.rs:6529-6571`), which the readiness probe uses to seed `ProviderReadiness.model_catalog`, only returns entries for `Fixed` / `LocalFiles` policies (plus the inline `fixed_model_catalog`). Its `match` falls through to `vec![]` for `RemoteCommand` and `UserSupplied`. That is a deliberate design choice: readiness must not fan out live per-provider API calls on every settings open. Consequence: **`remote_command` providers show only their `default_model` until the user explicitly clicks Load models.** This is exactly why the LLM tab has the button, and why Deepgram/Soniox need one too.

---

## 5. What it would take to add the button uniformly

### 5.1 Reuse, do not reinvent

Everything needed already exists:
- **Picker:** `ModelCatalogPicker` (shared, already mounted on every relevant block).
- **Type:** `ProviderModelCatalogItem` (all `list_*_models_cmd` return `Vec<ProviderModelCatalogItem>`).
- **Registry seam:** `descriptor.model_catalog === "remote_command"` + `descriptor.model_catalog_command`.
- **i18n:** `settings.buttons.refreshModels` / `refreshing` already exist; add per-provider `*ModelsFailed` / `*ModelsLoading` / `*NoModels` hint/error keys (or genericize the existing Cerebras/OpenRouter ones into `settings.errors.modelsFailed` / `settings.hints.modelsLoading` / `modelsNoModels` taking a provider-name interpolation, to avoid N triplets).

### 5.2 Recommended: one shared refresh component + one generic handler

Rather than copy the Cerebras block N times, extract a small presentational component (e.g. `ModelCatalogField`) that wraps `<ModelCatalogPicker>` + the refresh `<button>` + the tri-state status line, driven by props:
`{ value, onChange, catalog, loading, error, credentialAvailable, onRefresh, placeholder, hasRemoteCommand }`. When `hasRemoteCommand` is false (fixed/static providers) it renders the picker only — so the SAME component serves both "API-backed" and "curated static" providers, and the button appears exactly where a `model_catalog_command` is declared.

In the controller, add a **generic** `handleRefreshModels(providerId)` that:
1. Looks up `descriptor.model_catalog_command`; bails if absent.
2. Resolves the right credential-available gate + draft key argument for that provider (the args differ: Deepgram/Soniox pass `{ apiKey }`; `llm.api`/`asr.api` pass `{ endpoint, apiKey }`; OpenRouter passes `{ apiKey, baseUrl }`). A small per-provider arg-builder map keyed by provider id covers this.
3. `invoke<ProviderModelCatalogItem[]>(cmd, args)`, stashes into a `Record<providerId, ProviderModelCatalogItem[]>` live-catalog map (generalizing today's single `cerebrasModels` useState), sets loading/error keyed by provider id.
4. The per-provider `*ModelCatalog` memos overlay `liveCatalog[providerId] ?? readinessCatalog ?? generatedCatalog` (generalizing the Cerebras overlay at lines 1298–1304).

Optionally fold in the OpenRouter freshness-cache (TTL + cache key) so re-clicks are cheap; start without it for parity with Cerebras.

### 5.3 Where each dropdown lives (the mount points to touch)

- **STT / `asr.deepgram`:** `AsrProviderSettings.tsx:599-608` — wrap the deepgram `ModelCatalogPicker` with the shared field + button. New controller props: `deepgramModelsLoading`, `deepgramModelsError`, `handleRefreshDeepgramModels` (or the generic handler), `deepgramCredentialAvailable` (already exists, controller line 1119).
- **STT / `asr.soniox`:** no UI block exists yet (provider is `planned`, not in `ASR_PROVIDER_OPTIONS`). When Soniox is surfaced, its block gets the same field. Backend cmd is ready today.
- **STT / `asr.api`:** `AsrProviderSettings.tsx:324-333`. Either (a) leave as user_supplied (typed model), or (b) flip descriptor to `remote_command` pointing at `list_openai_compatible_llm_models_cmd` and pass `{ endpoint: asrEndpoint, apiKey }` — the OpenAI-compat `/v1/models` catalog is generic and works for STT endpoints too. This directly fixes "picked an invalid OpenAI model."
- **LLM / `llm.api`:** `LlmProviderSettings.tsx:440-452` — add the button next to the api-model picker (cmd already exists: `list_openai_compatible_llm_models_cmd`, args `{ endpoint: llmEndpoint, apiKey }`).
- **Static providers (fixed):** `asr.openai_realtime`, `asr.gladia`, `asr.speechmatics`, `asr.assemblyai`, all `tts.*`, both `realtime_agent.*` — render the shared field with `hasRemoteCommand=false`: a curated dropdown seeded from `fixed_model_catalog` / `default_model`, **no** API button (or a disabled/"models are fixed for this provider" note). To make these dropdowns richer than a single default, populate each descriptor's `fixed_model_catalog` with the known valid model IDs in `provider-registry/src/lib.rs` (a Rust-side data change, then regenerate `providerRegistry.ts`).

### 5.4 Providers that need a NEW backend command

None are strictly required for the ASR tab if scope = "match the LLM tab": **Deepgram and Soniox already have theirs.** New commands would only be needed to make a *fixed* provider dynamically discoverable, e.g.:
- `list_gemini_models_cmd` — `test_gemini_api_key` already fetches `GET /v1beta/models` (`commands.rs:8677`) but throws the list away; a new command could return the parsed catalog for the realtime/Gemini tab.
- `list_assemblyai_models_cmd`, `list_speechmatics_models_cmd`, etc. — most of these providers do not expose a public streaming-model list endpoint, so `fixed` + curated dropdown is the right call, not a new command.

### 5.5 Tests to mirror

`src/components/SettingsPage.test.tsx` already has the template: "clicking Load models invokes `list_*_models_cmd` and populates the picker" (lines 5819, 5892, 5950 for OpenRouter; 5200-5325 for Cerebras). New tests should assert the Deepgram/`llm.api` button invokes the right cmd with the right args and that empty/error states render the correct hint/error.

---

## 6. Tie-in: this directly fixes the OpenAI invalid_model and Deepgram default issues

- **Deepgram default (`nova-3`) issue.** Today the Deepgram picker only offers `nova-3` (its `default_model`), because the live catalog never loads without a button. A user who needs a different Deepgram model (or whose account lacks `nova-3` access) has no discoverable choice. Wiring `list_deepgram_models_cmd` behind a Load-models button surfaces the account's actual streaming STT models — the user picks a valid one instead of the stale hardcoded default. (Related: `docs/plans/2026-07-01-deepgram-401-rootcause.md`.)
- **OpenAI invalid_model issue.** For `asr.api` / `llm.api` (OpenAI-compatible), the model is a free-typed string today, so a typo or a model the key can't access produces `invalid_model` at request time. A Load-models button calling `list_openai_compatible_llm_models_cmd` (which hits `/v1/models`) lets the user select a model the endpoint+key actually serves — turning a runtime failure into a compile-time-style pick. This is the single highest-value target for the button after Deepgram.

The synergy: the same generic Load-models mechanism (§5.2) closes both bugs at once, since both providers are `remote_command` with working (Deepgram) or reusable (OpenAI-compat) backend commands.

---

## 7. Recommended build order (for a later implementation pass)

1. Extract `ModelCatalogField` (picker + button + status) — pure presentational, no behavior change; migrate Cerebras/OpenRouter onto it first to prove parity.
2. Generalize the controller: `liveCatalog` map + generic `handleRefreshModels(providerId)` + arg-builder map + genericized i18n triplet.
3. Wire **Deepgram** (highest user value, cmd already exists).
4. Wire **`llm.api`** and flip **`asr.api`** to `remote_command` → fixes `invalid_model`.
5. Populate `fixed_model_catalog` for the static providers so their curated dropdowns list more than one option; surface Soniox when it graduates from `planned`.
6. (Optional) Add `list_gemini_models_cmd` to make the realtime/Gemini tab dynamically discoverable.

---

## 8. Key file:line index

- LLM reference button — `src/components/LlmProviderSettings.tsx:558-583` (Cerebras), `:702-729` (OpenRouter)
- Shared picker — `src/components/ModelCatalogPicker.tsx`
- Controller fetch handlers — `src/components/settings/useSettingsController.tsx:2466-2501` (OpenRouter), `:2572-2588` (Cerebras); catalog memos `:1278-1321`; credential gates `:1100-1125`
- Catalog fallback helper — `src/components/providerRegistryHelpers.ts:171-215`
- ASR mount points — `src/components/AsrProviderSettings.tsx:325` (api), `:377` (openai_realtime), `:600` (deepgram), `:821` (sherpa)
- TTS mount point — `src/components/settings/TtsPanel.tsx:91` (aura voice)
- Gemini/realtime mount — `src/components/settings/GeminiPanel.tsx` → `GeminiSettings.tsx`
- Backend commands — `src-tauri/src/commands.rs:8291` (openai-compat llm), `:8519` (deepgram), `:8539` (soniox), `:8565` (cerebras), `:8779` (openrouter); registration `src-tauri/src/lib.rs:503-509`
- Readiness catalog seeding (why remote catalogs are empty at load) — `src-tauri/src/commands.rs:6529-6571`
- Registry source of truth — `src-tauri/crates/provider-registry/src/lib.rs` (`model_catalog`, `model_catalog_command`, `fixed_model_catalog`); generated `src/generated/providerRegistry.ts`; TS type `src/types/index.ts:1156-1161`
- i18n — `src/i18n/locales/en.json:211-212` (buttons), `:112-113` (errors), `:253-255` (hints)
- Existing test template — `src/components/SettingsPage.test.tsx:5200-5325` (Cerebras), `:5819-6078` (OpenRouter)
