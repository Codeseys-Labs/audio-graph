# ADR: Formalized Provider Contract

- **Status:** Proposed
- **Date:** 2026-07-03
- **Deciders:** Lead architect (provider-arch synthesis)
- **Supersedes:** none (extends the data-only `ProviderDescriptor` introduced with the provider registry)
- **Related:** `docs/plans/2026-07-02-provider-api-audit.md`, `docs/plans/2026-07-01-deepgram-401-rootcause.md`, `docs/plans/2026-07-02-load-models-button-design.md`, and the companion plan `docs/plans/2026-07-03-provider-arch-plan.md`

## Context and Problem Statement

Provider support (ASR + LLM + TTS + realtime) has grown by copy-paste. The only cross-provider artifact is the **data-only** `ProviderDescriptor` (`src-tauri/crates/provider-registry/src/lib.rs:471`), which shares command *names* but declares no behaviour, no explicit base-capability booleans, and no advanced-settings schema. Runtime behaviour is split across three unrelated Rust traits — `CloudAsrRequestConfig` (`asr/cloud.rs:37`), `TtsProvider`/`TtsSession` (`tts/mod.rs:419`/`:395`), `MoonshineStreamingAdapter` (`asr/moonshine.rs:155`) — with LLM having no trait at all, plus per-provider `match` arms in `commands.rs`. The frontend is two ad-hoc ladders: `AsrProviderSettings.tsx` (879 lines) and `LlmProviderSettings.tsx` (1023 lines), each a `type === "x" && (...)` chain hand-wiring a ~40-field prop bag per provider.

Four concrete failures fall out of this ad-hoc design:

1. **The flux clobber (confirmed live bug).** The Deepgram model field is a free-text combobox (`src/components/ModelCatalogPicker.tsx:84-102`). A user hand-typed the marketed name `flux`, which Deepgram's `v2/listen` enum rejects (only `flux-general-en` / `flux-general-multi` are valid). `migrate_asr_provider_model` (`settings/mod.rs:1858-1872`) runs on every load and, because bare `flux` fails the (API-correct) `is_valid_deepgram_streaming_model` validator (`asr/deepgram.rs:648-661`), silently `mem::replace`s it with `nova-3` and only `log::warn!`s. The user's flux intent vanishes with no UI signal. This is a design gap: the picker offers no guardrail, the migration downgrades instead of upgrading intent, and flux is never surfaced as a first-class option.

2. **Generic, not-per-model, test-connection.** Every `test_*_connection` command probes auth or `GET /v1/models` and ignores the selected model (`commands.rs:8132..8953`). A user can select a model their key cannot run (wrong tier/region/deprecated id, streaming-vs-batch mismatch) and still see a green "connection valid".

3. **Discarded model metadata.** `ProviderModelCatalogItem` is `{ id, display_name, is_default }` in all three definitions (registry `lib.rs:463`, runtime `commands.rs:208`, TS `types/index.ts:1188`). The live models APIs return languages, features, modes, and descriptions that the parse structs drop on the floor, so there is nothing to show in a model-info hover.

4. **FE↔BE contract mismatches fail only at runtime.** `src/test/setup.ts:20` does `vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }))`, replacing the whole module so the real `invoke()` never runs. Tests branch on command *name* only and never assert the args object. Arg shapes are hand-written camelCase objects at each call site that Tauri auto-converts to snake_case Rust params; the registry shares names, not shapes. A wrong/renamed arg key, a snake/camel drift, or a command missing from `generate_handler!` (`lib.rs:417`) passes every unit test green and only fails at runtime.

## Decision Drivers

- Preserve user intent (flux) rather than silently destroying it.
- Make "select model → load models → **test the selected model** → view model info → save credential" an invariant base surface across every provider.
- Keep legitimately-different advanced settings per provider without another 900-line ladder.
- Make the FE↔Rust command contract checkable in CI without a compiled binary.
- Every change wire-compatible and additive; no forced migration of persisted configs.

## Considered Options

1. **Keep the status quo** — continue hand-wiring each provider. Rejected: the four failure classes are structural and recur with every new provider.
2. **Full runtime super-trait first** — unify `TtsProvider`/ASR clients/LLM under one `ProviderRuntime` trait before touching the descriptor. Rejected as the *first* step: high blast radius, blocks the P0 bug fixes, and the FE gains nothing until the descriptor and catalog are enriched. Kept as a longer-term seam (see Decision E).
3. **Formalized descriptor-first contract (chosen)** — extend the existing data-only descriptor and catalog item with explicit capabilities, a per-model test command, and hover metadata; add a shared FE base component with a per-provider advanced slot; and layer the FE↔Rust test strategy. Lands incrementally, unblocks the P0 fixes, and the runtime super-trait becomes an optional later consolidation.

## Decision Outcome

Chosen: **Option 3 — a descriptor-first formalized provider contract** implemented at five seams (A–E). Each seam is independently landable and additive.

### (A) Rust ProviderCapability / contract layer + extended `ProviderDescriptor`

Add an explicit capability struct and per-provider schema to `ProviderDescriptor` (`provider-registry/src/lib.rs:471`), so the FE reads base capabilities directly instead of inferring them:

```rust
pub struct ProviderCapabilities {
    pub has_model_select: bool,           // was inferred from model_catalog != None
    pub has_load_models: bool,            // was inferred from model_catalog_command.is_some()
    pub has_test_model_connection: bool,  // NEW — distinct from generic auth probe
    pub has_model_info: bool,             // NEW — drives hover popover
}
// ProviderDescriptor gains:
//   pub capabilities: ProviderCapabilities,
//   pub test_model_command: Option<&'static str>,     // per-MODEL, distinct from health_check_command
//   pub advanced_settings: &'static [AdvancedField],  // declarative advanced schema (optional first cut)
```

`AdvancedField { key, label_i18n, kind: Bool|Int{min,max,step}|Float{..}|Enum{options}|Text, default_json }` gives the FE a declarative schema to render the advanced slot once the bespoke blocks are migrated. The de-facto advanced schema today lives only in the `AsrProvider`/`LlmProvider`/`TtsProvider` config enums (`settings/mod.rs:176`); the declarative form is the migration target, not a day-one requirement.

### (B) Test-MODEL-connection per provider (not generic)

Add a `test_model_command` per provider (Seam C) that receives `{ model, region?, endpoint? }` and exercises the model on its real transport:

- **Deepgram:** open `wss://.../v1/listen?model=<sel>` (or `v2/listen` for flux), send a silence frame, expect a Results/Metadata frame.
- **OpenAI / OpenAI-compatible / Cerebras:** 1-token `chat`/`transcribe` with the selected model.
- **OpenRouter:** 1-token completion, or resolve `/models/{author}/{slug}/endpoints` (already fetched by `list_openrouter_model_endpoints_cmd`, `commands.rs:8880`).
- **Bedrock:** tiny `Converse`/`InvokeModel` with the model id in the selected region (region+model are the real failure axes).
- **AWS Transcribe:** start+immediately-stop a streaming session.
- **AssemblyAI:** the v3 realtime socket smoke that is deliberately skipped today (`commands.rs:8654`).
- **Local (whisper/sherpa/moonshine/llama):** file-validate + warm load.

The generic `health_check_command` stays for the auth probe; `test_model_command` is the model-aware addition.

### (C) Shared frontend `ProviderSettingsBase` component + per-provider advanced slot

Add `src/components/settings/ProviderSettingsBase.tsx` driven by the extended descriptor. It renders the invariant base surface — provider radio, credential control (from `credential_keys`), `ModelCatalogField` (select + Load-models), a **Test model** button (calls `test_model_command`), and the model-info hover — then an advanced slot `<AdvancedSettingsDisclosure>{renderAdvanced(descriptor)}` resolved through a `Record<settings_variant, AdvancedFC>` registry. This keeps the bespoke Deepgram/AWS knob UIs as pluggable advanced blocks and collapses the two 900–1000-line ladders. The reuse seeds already exist: `ModelCatalogField.tsx`, `ModelCatalogPicker.tsx`, `AdvancedSettingsDisclosure.tsx`, `SecretCredentialControl`, `AwsCredentialControl`.

### (D) Model-metadata hover-combobox (live-over-curated merge, ARIA)

Enrich `ProviderModelCatalogItem` with **five optional fields** in all three definitions — `mode`, `endpoint`, `languages`, `features`, `description` — each `skip_serializing_if` (`Option::is_none` / `Vec::is_empty`), so the change is wire-compatible and additive. Stop discarding at the parse structs (`DeepgramModelDescriptor` gains `languages`/`features` + `mode` from the streaming flag; `SonioxModelDescriptor` surfaces its already-parsed `transcription_mode` as `mode` + adds `languages`; `OpenAiCompatibleModelDescriptor` gains best-effort `description`; OpenRouter maps its rich `OpenRouterModel` down via an adapter). Author curated tables for Fixed providers under the HONESTY rule (omit unknowns). Merge per-field, keyed on `id`, inside each `list_*_models_cmd`: `live.or(curated)` for Options, `if !live.is_empty() { live } else { curated }` for Vecs — **live-present wins, curated fills a hole**.

Frontend: `ModelCatalogPicker` is **already** a custom `role=combobox`/`role=listbox` with arrow-key nav (`aria-activedescendant`, filter-as-you-type). Add a `ModelMetadataPopover` (`role=tooltip`, referenced via `aria-describedby` from the active option) shown on **both** pointer-hover and keyboard `activeIndex` for a11y symmetry, with a `hasMeta` gate (no popover when all fields empty). Ship a details-panel variant first if the floating popover proves fiddly. The previously-feared "native `<select>` cannot host a popover" risk does not apply — the picker is already a custom listbox.

### (E) Layered FE↔Rust test strategy (per the Tauri testing guide)

Three complementary layers, cheapest first:

1. **Shared, checked command contract.** Extend the provider-registry generator to emit a per-command arg schema (snake_case params minus injected `app`/`state`/`window`, each required/optional) as both a TS type and a runtime map. Add (a) a vitest test that camelCase→snake_case-normalizes each FE arg object and checks it against the schema, and (b) a Rust/build check that every registry `health_check_command`/`model_catalog_command`/`test_model_command` name is present in `generate_handler!` (`lib.rs:417`).
2. **`mockIPC` unit tests at the seams that matter.** For the provider settings flow only, swap the global `vi.fn()` stub for `@tauri-apps/api/mocks` `mockIPC` (with the jsdom `window.crypto.getRandomValues` polyfill and a local `vi.unmock`), so the real `invoke` runs and tests assert the transport-level `(cmd, args)` — proving the FE dispatches the right command with the right keys.
3. **Real command-contract test (runtime backstop).** `tauri::test` IPC tests using `get_ipc_response`/`InvokeRequest` (the `test` feature is already in `Cargo.toml:273`) that fire the exact camelCase JSON body through `generate_handler!`, proving `{ apiKey, baseUrl }` deserializes into `api_key`/`base_url`. Optionally one `@wdio/tauri-service` embedded-provider smoke.

Where each layer localizes a fault: if (2) passes (FE sends documented keys) but (1)/(3) fail → backend signature drifted; if (1)/(3) pass but (2) fails → frontend built the wrong args. That FE-vs-BE decision is exactly what today's suite cannot make because the module stub erases the boundary.

## Base-vs-Advanced Capability Matrix

Base surface = { select model, load models, **test selected model**, model-info hover, credential }. Advanced = the knobs that legitimately differ per provider. "Test model transport" is what the per-model `test_model_command` should exercise (Seam C). "Model info" marks providers where live/curated metadata can populate the hover.

| Provider | Model select | Load models (live) | Test-MODEL transport | Model info (hover source) | Advanced settings (legitimately per-provider) |
|---|---|---|---|---|---|
| `asr.deepgram` | yes (free-text + catalog) | yes `/v1/models` | WS `v1/listen` / `v2/listen`(flux) + silence frame | live languages/features + curated flux | endpointing_ms, utterance_end_ms, vad_events, eot/eager_eot thresholds, eot_timeout_ms, max_speakers, diarization |
| `asr.openai_realtime` | yes (fixed catalog) | no (curated) | realtime WS transcription smoke | curated mode/description/features | realtime session opts (partials) |
| `asr.soniox` | yes | yes `/v1/models` | realtime WS smoke w/ selected model | live `transcription_mode`→mode + languages | transcription_mode, language hints |
| `asr.assemblyai` | needs real fixed catalog | no (curated) | v3 realtime socket smoke (skipped today, `commands.rs:8654`) | curated features (diarization, turns) | diarization, turn boundaries |
| `asr.aws_transcribe` | yes (fixed default) | no | start+immediately-stop streaming session in region | curated mode/endpoint/features | region, engine, vocab, PII redaction |
| `asr.gladia` | yes (fixed `solaria-1`) | no | REST-init + live WS smoke | curated mode/features | diarization, live opts |
| `asr.speechmatics` (roadmap) | when promoted | when promoted | streaming socket smoke | curated when promoted | TBD |
| `asr.api` (OpenAI-compat batch) | yes | yes `/v1/models` | 1-token/sub-second transcribe | best-effort description | endpoint, language |
| `llm.cerebras` | yes (fixed catalog) | yes (thin `/v1/models`) | 1-token chat | curated description; live fallback | temperature/max_tokens |
| `llm.openrouter` | yes | yes `/api/v1/models` | 1-token completion / endpoints resolve | rich live (description, modality, supported_parameters) | routing/provider prefs, base_url |
| `llm.aws_bedrock` | yes | region-scoped | tiny Converse/InvokeModel, model+region | curated per model | region, model params |
| `llm.api` (OpenAI-compat) | yes | yes `/v1/models` | 1-token chat | best-effort description | endpoint, max_completion_tokens |
| `tts.deepgram_aura` | yes (~130 voice catalog) | curated | open TTS session with selected voice | display_name (lang, gender) → structured | sample_rate, encoding, speed |

## Consequences

**Positive:** flux intent is preserved and surfaced; users can verify the actual model, not just the key; hover metadata gives model context with no wire break; the two ladders collapse to one base + thin advanced blocks; FE↔BE arg mismatches are caught in CI without a binary; new providers implement a known contract instead of copy-paste.

**Negative / risks:** touching `ProviderDescriptor` and both `ProviderModelCatalogItem` definitions is broad (every `const` literal must gain the fields — mitigated by a `const EMPTY_META` base + struct-update spread); per-model test commands add real network smoke paths that must be bounded (timeouts) and must not leak keys in logs; the declarative `advanced_settings` schema is the migration target, so bespoke advanced blocks coexist with it during rollout. All changes are additive and staged so the P0 bug fixes land first, independent of the contract work.
