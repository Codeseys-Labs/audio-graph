# Provider-selection → configuration → pipeline-dispatch audit (2026-07-05)

Scope: the full chain from the Settings UI provider/model picks, through
serialization (`config.yaml` + IPC payloads), deserialization (serde +
migrations), to the pipeline dispatch (`speech/mod.rs`) and connection-param
mapping (`deepgram_listen_url` and siblings). Motivated by the historical
Deepgram `model="general"` drift bug — the class where a selected config value
silently maps to a different wire value.

## 1. Chain map (where each value is chosen / serialized / deserialized / mapped)

### (a) Chosen in UI

- `src/components/settings/useSettingsController.tsx`
  - `ASR_PROVIDER_SETTINGS_VARIANTS` / `LLM_PROVIDER_SETTINGS_VARIANTS` /
    `TTS_PROVIDER_SETTINGS_VARIANTS` (lines ~92–112) are the hardcoded
    variant lists; `implementedProviderOptionsForStage` intersects them with
    the generated registry (`src/generated/providerRegistry.ts`) and drops
    anything not `status: "implemented"`.
  - `handleSave` (line ~3143) builds the `AsrProvider` / `LlmProvider` /
    `TtsProvider` IPC payloads via a `switch (asrType)` / `switch (llmType)`.
  - Hydration (load path, line ~2878) maps the redacted `AppSettings` from
    `load_settings_cmd` back into reducer fields with `?? fallback` defaults.
- `src/components/settingsTypes.ts` — `AsrType`/`LlmType` unions,
  `initialSettingsState` (frontend defaults), `endpointCredentialKey`
  (endpoint → credential-slot routing, mirrors Rust
  `credential_key_for_endpoint`).
- `src/components/AsrProviderSettings.tsx` — per-provider field widgets;
  `snapDeepgramModelAlias` normalizes `flux`/`flux-general` on blur.

### (b) Serialized

- Frontend → backend: `saveSettings` (store) → `invoke("save_settings_cmd",
  { settings })` with the whole `AppSettings` object. Secrets are sent as
  `""`/omitted; `#[serde(skip_serializing)]` keeps them out of `config.yaml`.
- Backend → disk: `settings::save_settings_to_path` writes redacted YAML;
  `analytics_enabled` is preserve-on-None (dual-writer guard).

### (c) Deserialized

- `settings::parse_settings_yaml` / `parse_settings_json` →
  `migrate_asr_provider_model` (one-shot Deepgram model migration:
  alias-upgrade → valid-check → clamp to `nova-3`).
- Serde defaults per field (`default_deepgram_model` = `nova-3`,
  `default_deepgram_endpointing_ms` = 300, `default_max_speakers` = 0, …).
- Runtime hydration: `hydrate_runtime_credentials` fills `api_key` fields
  from the credential store (per-endpoint routing via
  `credential_key_for_endpoint`).

### (d) Mapped into dispatch / connection params

- `speech/mod.rs run_speech_processor` (~line 2950): if-let chain over
  `AsrProvider` variants → per-provider worker configs. Deepgram:
  `u32/f32 → Option` gating (`endpointing_ms > 0`, `eot_threshold > 0.0`,
  eager-EOT validity rule `0 < eager <= eot`).
- `asr/deepgram.rs deepgram_listen_url`: sanitize → v1 (Nova) vs v2 (Flux)
  endpoint routing → param sets (Nova: `endpointing`, `utterance_end_ms`,
  `vad_events`; Flux: `eot_threshold`, `eager_eot_threshold`,
  `eot_timeout_ms`).
- `provider_registry.rs descriptor_for_{asr,llm,tts}_provider`: settings enum
  → registry descriptor id (used for readiness/telemetry).
- `AsrProvider::runtime_provider_id` / `requires_cloud_content_transfer`:
  privacy-gate classification per variant.
- `commands.rs start_transcription`: `apply_diarization_settings` overrides
  provider-level `enable_diarization`/`max_speakers` from the global policy
  before dispatch.

## 2. Silent defaults / fallbacks / drift points found

| # | Location | Behavior | Assessment |
|---|----------|----------|------------|
| 1 | `useSettingsController.tsx:2906` | Deepgram hydration fallback `asr.max_speakers ?? 2` while backend default + `initialSettingsState` are both `0` (BUG-4 "no cap") | **BUG (fixed in this PR)** — a persisted config without `max_speakers` (pre-BUG-4 file) hydrates the form to 2; next Save persists `max_speakers: 2`, silently re-capping speakers to 2. Backend serde default and the documented BUG-4 default are 0. Changed to `?? 0`; pinned by test. |
| 2 | `speech/mod.rs:3003` | Eager-EOT: sent only when `0 < eager <= eot`; UI clamps `min(eot, eager)` on save | Consistent pair; now pinned by tests on both layers (mapping rule extracted to `deepgram_config_from_settings`). |
| 3 | `speech/mod.rs:3010–3015` | `0` means "not configured" for `endpointing_ms`/`utterance_end_ms`/`eot_timeout_ms`/`eot_threshold` | Deliberate sentinel; now pinned by unit test so a future `>=` typo can't silently send `endpointing=0`. |
| 4 | `useSettingsController.tsx` hydration | LLM `api` endpoint re-classified to `cerebras`/`sambanova` variants via `endpointCredentialKey`; save maps them back to `type: "api"` with the canonical base URL | Round-trips correctly; pinned by round-trip test. |
| 5 | `settingsTypes.ts endpointCredentialKey` vs Rust `credential_key_for_endpoint` | Two hand-maintained copies of endpoint→slot routing (substring matching, `gemini` before `groq` etc.) | Drift risk. Both sides individually tested (Rust `endpoint_credential_routing_covers_known_openai_compatible_hosts`, TS `SettingsPage.test.tsx`); added a shared-vector contract test asserting the TS table matches the Rust table for all known endpoints. |
| 6 | `useSettingsController.tsx` UI variant lists vs `settings/mod.rs` serde tags | `ASR_PROVIDER_SETTINGS_VARIANTS` is hand-listed; a new backend variant (e.g. `soniox`, currently `status: planned` in the registry but fully implemented in the backend dispatch) does not appear in the UI | Deliberate gating by registry `status`, but **the UI variant list itself was untested** — a typo'd variant would silently drop a provider from Settings. Added contract test: every UI variant string must exist in the generated registry for its stage, and every `implemented` registry entry of that stage must be present in the UI list. |
| 7 | `provider_registry.rs` | `descriptor_for_*` mapping is exhaustive (compile-time enforced by match) and covered by existing tests | OK. |
| 8 | `AsrProvider` serde | All numeric/bool fields carry `#[serde(default = ...)]`; **no** `skip_serializing_if` on provider config fields (only `analytics_enabled` and logging options have it) | Round-trip is field-preserving; pinned by new serde round-trip test (non-default values for every `DeepgramStreaming`/`Soniox` field survive YAML). |
| 9 | `deepgram_listen_url` | Sanitizes at the last moment; alias upgrade lives in both load path (`migrate_asr_provider_model`) and request path (`sanitize_deepgram_model`) | Already in lockstep + tested; added default-model cross-layer consistency test (settings default == deepgram clamp target == registry `default_model` == generated TS registry). |
| 10 | `useSettingsController.tsx` save | `soniox` is constructible in `SettingsState` but absent from `ASR_PROVIDER_SETTINGS_VARIANTS`; the save `switch` still has a `soniox` case (dead until registry promotes it) | Consistent with the "backend/manual config until registry promotion" comment in `settingsTypes.ts`. No action; noted for the promotion PR. |
| 11 | Save-path `default:` fallthrough | `switch (asrType)` `default:` maps any unknown variant to `local_whisper`; same for LLM → `local_llama` | Silent fallback, but unreachable while `asrType` is the closed `AsrType` union. Noted; a runtime-invalid persisted `asrType` cannot occur because hydration derives it from the backend-tagged enum. |
| 12 | Hydration `asr.endpointing_ms ?? 300` etc. | Frontend re-applies backend serde defaults for optional TS fields | Values match backend `default_deepgram_*` fns; pinned by the same contract test as #1. |

## 3. Findings too big to fix here (reported, not fixed)

- **Dual endpoint-routing tables (#5)**: the real fix is generating the TS
  table from the Rust source (like `providerRegistry.ts`) or exposing it via
  the registry crate. The added contract test freezes the current vectors but
  new endpoints still need two edits.
- **Registry `status` vs backend dispatch drift (#6/#10)**: `asr.soniox` is
  `planned` in the registry yet fully dispatchable in `speech/mod.rs` and
  constructible via config-file edits. Harmless (config-only users get a
  working provider the UI doesn't offer), but the registry `status` field is
  doing double duty as "UI-selectable" and "implemented"; worth a dedicated
  field when Soniox is promoted.
- **`max_speakers` semantics split**: `DiarizationSettings::provider_max_speakers`
  can override the provider-level value at dispatch time (`apply_diarization_settings`),
  so the Settings form value is not always what runs. Documented behavior, but
  the Settings UI gives no hint the global diarization policy may override the
  per-provider cap.

## 4. Tests added

### Rust (CI-verified — local cargo capped)

- `src-tauri/src/speech/mod.rs` (`tests_provider_dispatch`, new):
  - `deepgram_config_from_settings` mapping extracted into a named helper
    (pure refactor — behavior identical) and pinned:
    - zero-valued `endpointing_ms`/`utterance_end_ms`/`eot_timeout_ms`/
      `eot_threshold` map to `None` (not `Some(0)`);
    - configured values map to `Some`;
    - eager-EOT threshold only passes when `0 < eager <= eot`;
    - model/api_key/diarization/vad flags pass through verbatim.
- `src-tauri/src/settings/mod.rs`:
  - serde YAML round-trip preserving every non-default `DeepgramStreaming`
    field and every non-default `Soniox` field (guards a future
    `skip_serializing_if` regression);
  - default-model lockstep: `default_deepgram_model()` ==
    `deepgram::DEEPGRAM_DEFAULT_STREAMING_MODEL` == registry
    `asr.deepgram.default_model` (and is a valid streaming model — i.e. not
    `general`);
  - empty `asr_provider: {type: deepgram}` YAML yields the full documented
    default set (nova-3 / 300 / 1000 / vad on / 0.5 / 0.0 / 0 / cap 0);
  - `runtime_provider_id` ↔ `provider_registry` descriptor id equality for
    every ASR/LLM/TTS variant (dispatch-id lockstep).

### Frontend (vitest, verified locally)

- `src/components/settings/useSettingsController.test.tsx` (new describe
  blocks):
  - Deepgram save/load round-trip: a hydrated non-default Deepgram config
    survives `handleSave` byte-identical (model, all five tuning params, cap);
  - `max_speakers` hydration default regression: settings without
    `max_speakers` hydrate to 0 and Save persists 0 (the #1 bug);
  - save-payload clamping: negative/fractional ms round to `>= 0` integers,
    `eot_threshold` clamps to `[0,1]`, eager-EOT clamps to `<= eot`;
  - provider-selection accuracy: picking each implemented ASR variant saves a
    payload whose `type` tag matches the backend serde tag.
- `src/components/settings/providerVariantContract.test.ts` (new):
  - every UI settings variant exists in the generated registry for its stage;
  - every `implemented` registry provider for asr/llm/tts is present in the
    UI variant list (no phantom or missing options);
  - UI initial defaults match registry `default_model` for deepgram/soniox/
    openai_realtime.
- `src/components/settings/endpointCredentialContract.test.ts` (new):
  - TS `endpointCredentialKey` matches the Rust `credential_key_for_endpoint`
    table (shared vectors incl. trailing-slash + case variants), reading the
    Rust source via `?raw` to fail loudly if either side changes alone.

## 5. Bugs fixed in this PR

1. **Deepgram `max_speakers` hydration default 2 → 0**
   (`useSettingsController.tsx:2906`). One-line change; regression test added.
   Symptom before fix: users with pre-BUG-4 configs (no `max_speakers` key)
   who opened Settings and hit Save were silently re-capped to 2 speakers —
   exactly the "stuck on 2 speakers" class BUG-4 removed.
2. **Deepgram eager-EOT clamped against the raw (not clamped) eot input**
   (`useSettingsController.tsx` save path, found BY the new clamp test).
   `eager_eot_threshold: min(deepgramEotThreshold, eager)` used the raw
   `deepgramEotThreshold`; with an out-of-range input (e.g. eot=1.4,
   eager=2.0) the persisted pair became `eot_threshold: 1,
   eager_eot_threshold: 1.4` — eager > eot, an invalid pair the dispatch
   layer then silently drops (the user's eager-EOT setting vanishes).
   Fixed by clamping eot first and bounding eager by the clamped value.
