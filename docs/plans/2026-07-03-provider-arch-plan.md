# Provider Architecture — Prioritized Fix + Build Plan

- **Date:** 2026-07-03
- **Companion ADR:** `docs/plans/2026-07-03-provider-contract-adr.md`
- **Source lanes:** `/tmp/provider-arch/{flux-regression,401-readpath,flux-catalog,tauri-testing,arch-survey,hover-metadata}.md`
- **Build baseline:** master `8f11450`, branch `fix/gtk-test-harness-65f0`

## Verdict gate (what is / is not P0)

Only bug fixes whose adversarial verify verdict was **confirmed** are P0 bugs.

- **flux clobber (`flux-regression`) — CONFIRMED.** Migration downgrades bare `flux`→`nova-3` silently. → P0.1.
- **401 read-path stale cache (`401-readpath`) — CONFIRMED.** `save_credential_cmd` never re-hydrates the `app_settings` cache. → P0.2 (add fingerprint diagnostic first, then the re-hydrate fix).
- **flux catalog gap (`flux-catalog`) — NOT A BUG (`isBug:false`, no adversarial verdict).** `/v1/models` genuinely omits flux; the parser is correct. It is a user-facing discoverability gap that directly compounds the P0.1 regression (no key ⇒ empty picker ⇒ users hand-type `flux`). Included in P0 as a **UX gap fix**, explicitly flagged as not-a-defect.
- **Design lanes (`arch-survey`, `hover-metadata`, `tauri-testing`) — `severity:none`/`n/a`, no verdict.** These are the P1/P2 contract-formalization workstreams; no live-bug claim.

No lane was refuted or downgraded below its stated severity on re-check; all cited file:line anchors were spot-verified in the working tree.

---

## P0 — Live bug fixes (ship first, independently)

### P0.1 — Flux clobber regression [CONFIRMED bug] · effort S · independent
Preserve user flux intent instead of silently downgrading to `nova-3`.

- **Backend alias-upgrade (primary):** in a shared helper called by both `migrate_asr_provider_model` (`src-tauri/src/settings/mod.rs:1858-1872`) and `sanitize_deepgram_model` (`src-tauri/src/asr/deepgram.rs:671-681`), map case-insensitive exact `flux`→`flux-general-en` (optionally `nova`→`nova-3`) **before** the generic clamp-to-`nova-3` fallback. Placing the alias in one helper keeps load-path (`mod.rs:1860`) and request-path (`deepgram.rs:688`) in lockstep.
- **Do NOT** loosen `is_valid_deepgram_streaming_model` (`deepgram.rs:648-661`) — it is API-correct; bare `flux` is a 400 on Deepgram's `v2/listen` enum (allowed: `flux-general-en`/`flux-general-multi` only).
- **Frontend guard:** in the Deepgram model combobox (`src/components/ModelCatalogPicker.tsx:84-102`, wired at `src/components/AsrProviderSettings.tsx:621-636`), snap a typed `flux`→`flux-general-en` on blur, or show a non-destructive warning instead of silent accept-then-clobber.
- **Tests:** update `settings/mod.rs:4286`-style migration test to assert `flux`→`flux-general-en` (not `nova-3`); add validator table cases.
- **Ordering:** independent of P0.2/P0.3, but the backend alias + P0.3 curated catalog should land together for a coherent flux story.

### P0.2 — 401 read-path fingerprint diagnostic + cache re-hydrate [CONFIRMED bug] · effort S (diag) + M (fix) · ordered (diag → fix)
The runtime read-path serves the api_key from the long-lived `app_settings` cache, which `save_credential_cmd` never re-hydrates, so an in-place row save can leave the cache serving a stale key → 401 with no provider fault.

- **Step 1 — fingerprint diagnostic (S, ship first):** add `secret_fingerprint(v) -> "sha256:<first 4 bytes hex> len=<n>"` (helper in `src-tauri/src/credentials/mod.rs`; `sha2` likely already a dep). Emit at BOTH ends:
  - SAVE end: `src-tauri/src/commands.rs:6325` (`save_credential_cmd` log, alongside `value_len`).
  - READ/CONNECT end: `src-tauri/src/asr/deepgram.rs:269-273` (connect log, alongside `len`).
  - **Never** log first4+last4 of the raw 40-char key; the sha256 prefix leaks nothing.
- **Step 2 — decision:** SAVE fp == CONNECT fp ⇒ genuine provider reject (confirm with live curl, close). SAVE fp != CONNECT fp ⇒ stale-cache defect confirmed → apply the fix.
- **Step 3 — cache re-hydrate fix (M):** have `save_credential_cmd` re-hydrate `app_settings` after `set_credential` (mirror `src-tauri/src/lib.rs:410-413`: `load_credentials()` + `hydrate_runtime_credentials(&settings, &store)` + `*state.app_settings.write() = ...`) — this requires adding a `State<AppState>` param to the command. **No-signature-change alternative:** have the frontend row-save (`src/components/settings/useSettingsController.tsx:2458` `handleSaveCredentialValue`) also call `save_settings_cmd`/`load_settings_cmd` after `save_credential_cmd`.
- **Ordering:** ship the diagnostic first (proves the mechanism before the fix); the re-hydrate fix follows. Independent of P0.1/P0.3.

### P0.3 — Flux catalog discoverability [UX gap, NOT a defect] · effort S · independent
Deepgram's `/v1/models` returns zero flux entries (proven live; `/tmp/provider-arch/dg-v1-models.json`); the parser is correct. Add flux as curated entries so the picker offers it.

- **Backend (primary):** in `deepgram_stt_model_catalog_from_response` (`src-tauri/src/commands.rs:8389-8427`), after the loop append `flux-general-en` / `flux-general-multi` if absent. Update the unit test at `commands.rs:12531/12549`.
- **Registry (optional polish):** add `DEEPGRAM_STT_MODEL_CATALOG` const near `provider-registry/src/lib.rs:1285` (nova-3 default + two flux ids) and set `fixed_model_catalog: Some(...)` at `lib.rs:2022` so flux shows before the first Load-models click; keep `model_catalog_command` so live override still works.
- **Ordering:** independent; pairs naturally with P0.1.

---

## P1 — Contract formalization + test-model + hover metadata

### P1.1 — Extend `ProviderDescriptor` with capabilities + `test_model_command` (Seam A) · effort M · independent (foundation)
Add `ProviderCapabilities { has_model_select, has_load_models, has_test_model_connection, has_model_info }` and `test_model_command: Option<&'static str>` (+ optional `advanced_settings: &[AdvancedField]`) to `provider-registry/src/lib.rs:471`; populate all ~30 `PROVIDER_REGISTRY` entries; regenerate the TS mirror (`src/types/index.ts:1147`). Foundation for P1.2/P1.4/P2.

### P1.2 — Per-MODEL test-connection commands (Seam C) · effort L · partially ordered (after P1.1)
Add `test_model_command` per provider exercising the model on its real transport (Deepgram WS + silence frame; OpenAI/Cerebras/OpenRouter 1-token; Bedrock tiny Converse w/ region; Transcribe start+stop; AssemblyAI v3 socket smoke currently skipped at `commands.rs:8654`; local file-validate + warm load). Bound every smoke with a timeout; never log keys. Each provider's command is independent of the others once the descriptor field exists.

### P1.3 — Model-metadata hover-combobox (Seam B + D) · effort M · independent of P1.2
Add 5 optional fields (`mode`/`endpoint`/`languages`/`features`/`description`, all `skip_serializing_if`) to `ProviderModelCatalogItem` in all three definitions (registry `lib.rs:463`, runtime `commands.rs:208`, TS `types/index.ts:1188`); use a `const EMPTY_META` base + struct-update spread to avoid editing every literal. Stop discarding in the parse structs (`DeepgramModelDescriptor`, `SonioxModelDescriptor` transcription_mode→mode, `OpenAiCompatibleModelDescriptor` description, OpenRouter adapter). Add `merge_catalog_metadata(live, curated)` keyed on id (`live.or(curated)` / `if !live.is_empty()`). Frontend: `ModelMetadataPopover` on `ModelCatalogPicker` (already a custom ARIA combobox), `role=tooltip` via `aria-describedby`, shown on hover AND keyboard `activeIndex`, `hasMeta` gate. Ship details-panel variant first if the popover is fiddly. Wire-compatible; independent of P1.1/P1.2.

### P1.4 — Layered FE↔Rust test strategy (Seam E) · effort M · ordered (arg schema after P1.1)
(a) Extend the registry generator to emit a per-command arg schema + a vitest key-normalization test + a Rust/build check that every command name is in `generate_handler!` (`lib.rs:417`). (b) `mockIPC` unit tests for the provider settings flow (local `vi.unmock` of `@tauri-apps/api/core`, `window.crypto` polyfill) asserting transport `(cmd, args)`. (c) `tauri::test` `get_ipc_response`/`InvokeRequest` tests firing camelCase bodies through `generate_handler!` (`Cargo.toml:273` `test` feature already present). Layer (a) depends on P1.1's generator extension; (b)/(c) are independent.

---

## P2 — Full rollout across all providers

### P2.1 — `ProviderSettingsBase.tsx` + advanced-slot registry (Seam E-FE) · effort L · ordered (after P1.1, P1.3)
New `src/components/settings/ProviderSettingsBase.tsx` rendering the invariant base surface + `Record<settings_variant, AdvancedFC>` advanced slot; incrementally migrate providers off `AsrProviderSettings.tsx` (879L) / `LlmProviderSettings.tsx` (1023L). Per-provider migration is independent once the base component exists.

### P2.2 — Curated metadata tables for all Fixed providers · effort M · independent (per provider)
Author curated `mode`/`endpoint`/`languages`/`features`/`description` under the HONESTY rule (omit unknowns) for OpenAI Realtime, AssemblyAI (needs a real `fixed_model_catalog`), AWS Transcribe, Gladia, Cerebras, Aura, Speechmatics-when-promoted. Each table is independent.

### P2.3 — `ProviderRuntime` super-trait consolidation (Seam D-backend) · effort L · ordered last
Unify `TtsProvider` (`tts/mod.rs:419`), ASR streaming clients, and a new `LlmProvider` under one `ProviderRuntime { descriptor(); list_models(); test_model() }` super-trait. Highest blast radius; land after P1.2 has proven per-model test paths per provider. Advanced settings remain in the per-variant config enums (`settings/mod.rs:176`).

---

## Independence / parallelization summary

- **Parallel now (P0):** P0.1, P0.2-diagnostic, P0.3 are mutually independent. P0.2-fix follows its diagnostic.
- **P1 fan-out:** P1.1 is the foundation; P1.3 and P1.4(b)(c) run parallel to it; P1.2 and P1.4(a) follow P1.1.
- **P2:** P2.1 after P1.1+P1.3; P2.2 per-provider parallel; P2.3 last.
