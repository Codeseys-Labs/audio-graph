# Provider API Audit — Synthesis Report

**Date:** 2026-07-02
**Scope:** All cloud provider integrations in the Tauri app at `/mnt/e/CS/github/audio-graph` — 8 ASR, 1 TTS, 3 LLM providers.
**Method:** Per-provider audit against current official docs, then an adversarial verify pass that tried to REFUTE each claimed discrepancy. Only verify-CONFIRMED items are in the fix list; refuted/downgraded items are called out as false alarms.
**Priority:** Deepgram — the user actually hit a failed streaming session.

---

## 1. Executive Summary (ranked by user impact)

### BROKEN — hard failure for an affected user, fix now
| Rank | Provider | Kind | What breaks | Verify |
|------|----------|------|-------------|--------|
| **1** | **deepgram** | ASR | A persisted `model="general"` is not a real Deepgram streaming model id. It is interpolated raw into the `v1/listen` URL with no mapping/validation, so the WebSocket upgrade is rejected (HTTP 400) and the session fails / yields no transcript. Compounded: only 401 is mapped to an actionable message, so the real cause is hidden behind a generic error. | **CONFIRMED (high)** |

Deepgram is the only provider with a confirmed user-facing break. It is #1 because the user reported it and the verify stage confirmed the high-severity finding.

### SUSPECT — real but narrow; no confirmed hard break in the common path
| Rank | Provider | Kind | Concern | Verify |
|------|----------|------|---------|--------|
| 2 | **llm_openai_compat** (`llm.api`) | LLM | Always sends deprecated `max_tokens`, never `max_completion_tokens` → hard-fails only against OpenAI **o-series/reasoning** models; works for Ollama/vLLM/LM Studio/OpenRouter/gpt-4o. Plus a mislabeled model-list default (`whisper-1`). | **DOWNGRADED** (no confirmed HIGH) |

The verify stage **downgraded** this provider: it did not confirm any HIGH/CRITICAL break. The `max_tokens` issue is genuine but conditional (only OpenAI reasoning models), so it lands as a P1, not a P0.

### CORRECT — matches current docs (nits only)
| Provider | Kind | Residual severity | Notable nit |
|----------|------|-------------------|-------------|
| **deepgram_aura** | TTS | medium (staleness) | Voice catalog exposes only 12 legacy Aura-1 voices; Aura-2 (the now-featured set) and non-English voices are absent from the dropdown. No runtime break. |
| soniox | ASR | none | Fully matches current docs. |
| assemblyai | ASR | low | Correct v3 endpoint + `universal-3-5-pro`; only dead legacy-v2 parse branches remain. |
| aws_transcribe | ASR | low | Official SDK; silent `en-US` fallback on bad language code. |
| gladia | ASR | low | Correct v2 live API; minor optional-config gaps. |
| speechmatics | ASR | low | Correct v2 RT; default `enhanced` diverges from API default `standard` (both valid). |
| revai | ASR | low | Correct streaming STT; every `final` treated as end-of-turn (semantic nuance). |
| openai_realtime | ASR | low | Verbatim structural match to GA transcription docs. |
| bedrock | LLM | low | Correct ConverseStream SDK; registry `supports_streaming:false` is cosmetically wrong (runtime unaffected). |
| openrouter | LLM | low | Correct endpoint/auth/model-list; deprecated `max_tokens` nit only. |

**Bottom line:** 1 broken (Deepgram), 1 suspect (OpenAI-compatible LLM), 10 correct. Fix Deepgram first; everything else is quality/robustness/staleness.

---

## 2. Confirmed Discrepancies + Minimal Fixes

Only verify-CONFIRMED items appear as fixes. False alarms are listed explicitly and excluded.

### 2.1 Deepgram (CONFIRMED, high) — see the dedicated section 3 for full detail.

- **D-1 (high):** `model="general"` is not a valid streaming model id; passed raw with no validation.
  - impl: `src-tauri/src/speech/mod.rs:2953` (`model: model.clone()`) → `src-tauri/src/asr/deepgram.rs:602` (Nova) / `:597` (Flux) interpolate it verbatim.
  - docs: `model` is a **Required enum** on `v1/listen`; the enum has no bare `general` — it only exists as a family suffix (`nova-3-general`, `base-general`). Default is `base-general`; the app's own code default is `nova-3`.
  - fix: clamp/migrate a stale/invalid `model` to `nova-3` before building the URL (see 3.2).
- **D-2 (medium):** invalid-model failure surfaces as a generic error, not an actionable one, because `classify_connect_error` only maps 401.
  - impl: `src-tauri/src/asr/deepgram.rs:563-572` (matches only `StatusCode::UNAUTHORIZED`).
  - docs: an out-of-enum `model` yields a **400 Invalid Request**, not 401.
  - fix: add a `400` arm returning an actionable "invalid/unsupported model — reselect a model in Settings" message (see 3.2).

### 2.2 llm_openai_compat / `llm.api` (DOWNGRADED — not a confirmed HIGH; treat as P1)

The verify stage did not confirm a HIGH/CRITICAL break here. These are the real-but-narrow items to fix opportunistically:

- **L-1 (medium, conditional):** `ChatCompletionRequest.max_tokens` is a non-optional field always serialized.
  - impl: `src-tauri/src/llm/api_client.rs:57` (struct field), set at `:246` from `config.max_tokens` (default 512 at `src-tauri/src/commands.rs:746`).
  - docs: `max_tokens` is **Deprecated** in favor of `max_completion_tokens` and is **not compatible with o-series models** (OpenAI returns 400 `unsupported_parameter`).
  - impact: hard failure ONLY against OpenAI o-series/GPT-5-reasoning models. gpt-4o/gpt-4/gpt-3.5 and all non-OpenAI targets still accept `max_tokens`.
  - fix: send `max_completion_tokens` for OpenAI-host endpoints (detect `api.openai.com`), keep `max_tokens` for the generic/local path where `max_completion_tokens` may be unknown; or send both is unsafe (some servers reject unknown keys), so prefer host-gated selection. Small.
- **L-2 (low):** LLM model-list flags the default with the ASR model id `whisper-1`, so no chat row is ever marked default.
  - impl: `src-tauri/src/commands.rs:8205` passes `Some("whisper-1")`; const `OPENAI_COMPATIBLE_DEFAULT_MODEL = "whisper-1"` at `:7666`; consumed by `list_openai_compatible_llm_models_cmd` at `:8302`.
  - docs: `models/list` returns chat ids in `data[].id`; `whisper-1` is a speech-to-text model that never appears in a chat catalog.
  - fix: pass `None` (or a chat default) for the LLM catalog path; keep `whisper-1` only for the ASR reuse at `commands.rs:8138`. Small.

**Refuted / excluded from the fix list (false alarms):**
- **L-3 — vLLM `structured_outputs:{json}` field.** NOT a bug. It is gated to vLLM endpoints only (`prefers_vllm_structured_outputs()`, `api_client.rs:357-363`), is a legitimate vLLM guided-decoding convention, is never sent to OpenAI, and has a JSON-mode fallback. Excluded.

### 2.3 CORRECT-provider nits worth queuing (verify did not flag any as broken)

None of these are confirmed breaks; they are quality items. Included for the P2 backlog:
- **deepgram_aura voice catalog (medium staleness):** `src-tauri/crates/provider-registry/src/lib.rs:1285` — add Aura-2 voices (`aura-2-*`), the now-featured set. No runtime break (voice is a free-form string; a hand-typed `aura-2-thalia-en` already works).
- **bedrock `supports_streaming:false` (low, cosmetic):** `src-tauri/crates/provider-registry/src/lib.rs:2301` (+ generated `src/generated/providerRegistry.ts:2543`) — flip to `true`. Runtime unaffected (`provider_supports_streaming()` already returns true).
- **aws_transcribe silent language fallback (low):** `src-tauri/src/asr/aws_transcribe.rs:519-522` — surface/log an invalid language code instead of coercing to `en-US`.
- **assemblyai dead v2 branches (low):** `src-tauri/src/asr/assemblyai.rs:1125` — delete dead `message_type`-keyed v2 handling (cleanup only).
- **openrouter/max_tokens (low):** `src-tauri/src/llm/openrouter.rs:327,355` — same deprecation as L-1; lower priority since OpenRouter accepts `max_tokens`.

---

## 3. DEEPGRAM (priority)

### 3.1 Is `model="general"` the cause? — YES (confirmed).

- The app default is `nova-3` (`settings/mod.rs`, registry), so a **fresh install is correct**. The `general` value comes from a **persisted/migrated user config** that predates the `nova-3` default, or a hand-edit.
- `deepgram_listen_url()` builds `wss://api.deepgram.com/v1/listen?...&model=general` because `config.model` flows raw from `speech/mod.rs:2953` → `deepgram.rs:602` with **no friendly-name → id mapping and no validation** anywhere.
- Docs are definitive: `model` is a **Required enum** on `v1/listen`. The enum contains family prefixes and prefix+suffix combos (`nova-3`/`nova-3-general`, `base`/`base-general`, …) but **no bare `general`**. `general` is only ever a suffix. Documented default is `base-general`.
- Result: Deepgram rejects the upgrade with **HTTP 400 Invalid Request** → failed session / empty transcript. This is NOT a 401, so the impl's only actionable branch (`classify_connect_error`, 401-only) does not fire and the user sees a generic redacted error — hiding the true cause. This matches the reported symptom exactly.
- Cross-check: `list_deepgram_models_cmd` (models/list, streaming-filtered) can never emit a bare `general` — its ids are real `canonical_name`s (`nova-3`, `flux-general-en`, …). So the picker cannot reproduce `general`; it is purely a stale persisted value. Confirms the root cause is the missing validation/migration, not the picker.

### 3.2 Correct model id / mapping + the fix

- **Correct ids:** pass a real streaming enum value. For the app's default use `nova-3` (canonical name accepted as `model=nova-3`). `nova-3-general` is also valid; `base-general` is the documented API default.
- **No alias layer exists**, so the minimal fix is a **clamp/migration on read**, not a broad mapping table:
  1. In `speech/mod.rs` (or a dedicated `DeepgramConfig::sanitize_model()`) before building `DeepgramConfig`, validate `model` against the known-good set: if it is not a `flux-*` id and not a valid streaming id, rewrite it to `nova-3` and log a one-line warning. A bare `general` → `nova-3` is the specific case that fixes this user. Small.
  2. Optionally validate against the live models catalog (`list_deepgram_models_cmd` already fetches streaming-only `canonical_name`s) when available, falling back to the static default when offline.
  3. Add a **400 arm** to `classify_connect_error` (`deepgram.rs:563-572`) that returns an actionable "invalid/unsupported model — reselect in Settings" message, so any future bad model is diagnosable rather than generic. Small.
- A one-shot **settings migration** that rewrites any persisted `asr_provider.model == "general"` (and any non-enum value) to `nova-3` closes it permanently for existing installs.

### 3.3 Flux v2 path — mostly correct; one benign nit

- Endpoint routing is CORRECT: `model.starts_with("flux-")` → `wss://api.deepgram.com/v2/listen` (`deepgram.rs:594-599`); Nova → `v1/listen`. Matches listen-flux docs.
- `eot_threshold` / `eager_eot_threshold` / `eot_timeout_ms` names and the `effective_eager_eot` gate (`speech/mod.rs:2948-2950`, eager only forwarded when `<= eot_threshold`) match the Flux ref.
- **Nit (low):** the Flux branch appends `channels=1` (`deepgram.rs:597`), which is not a documented `v2/listen` query param (Flux documents only `encoding`+`sample_rate` for raw audio). Deepgram ignores unknown params, so it is benign; drop it for cleanliness.
- **Nit (low, unverified):** Flux `TurnInfo` inner event-field name is heuristic (`event`|`turn_event`|`state`, `deepgram.rs:1297-1302`) and `ListenV2Connected`/`ConfigureSuccess`/`FatalError` are not handled by explicit name (they fall to generic debug/error arms). No crash; correctness for the Flux turn path could not be fully confirmed from the fetched docs. Only matters if Flux is a supported user selection (Nova is the shipping default).

### 3.4 list-models command — CORRECT (sound)

`list_deepgram_models_cmd` → `GET https://api.deepgram.com/v1/models` with `Authorization: Token <key>`, iterates `response.stt` only (never `tts`), filters `streaming == Some(true)` (excludes batch-only), prefers `canonical_name` for the id, and marks `is_default` on `nova-3`. All match the models/list doc. No changes needed. It is the right source of truth to validate `config.model` against (see 3.2 step 2).

---

## 4. Prioritized Fix Plan (P0/P1/P2)

Effort: S = <1h, M = a few hours, L = a day+. Independence noted for parallelization.

### P0 — Deepgram user-facing break (do first; the reported bug)
| ID | Fix | File:line | Effort | Independent? |
|----|-----|-----------|--------|--------------|
| P0-1 | Clamp/validate `config.model`: rewrite invalid (`general` etc.) → `nova-3` before URL build | `src-tauri/src/speech/mod.rs:2953` (or new `DeepgramConfig::sanitize_model`) | S | Yes |
| P0-2 | One-shot settings migration: persisted `general`/non-enum → `nova-3` | settings migration (`settings/mod.rs`) | S | Yes (pairs with P0-1) |
| P0-3 | Add a 400 arm to `classify_connect_error` → actionable "invalid model" message | `src-tauri/src/asr/deepgram.rs:563-572` | S | Yes |

P0-1/P0-2/P0-3 are mutually independent and parallelizable. P0-1 alone fixes the live user; P0-2 makes it durable; P0-3 makes future bad models diagnosable.

### P1 — Real-but-conditional break (verify-downgraded)
| ID | Fix | File:line | Effort | Independent? |
|----|-----|-----------|--------|--------------|
| P1-1 | Send `max_completion_tokens` for OpenAI-host endpoints; keep `max_tokens` for generic/local | `src-tauri/src/llm/api_client.rs:57,246` | M | Yes |
| P1-2 | LLM model-list: pass `None`/chat default instead of `whisper-1` | `src-tauri/src/commands.rs:8205` (keep `:8138` ASR use) | S | Yes |

Independent of each other and of all P0 items.

### P2 — Quality / staleness / cosmetic (no confirmed break)
| ID | Fix | File:line | Effort | Independent? |
|----|-----|-----------|--------|--------------|
| P2-1 | Add Aura-2 voices to the TTS catalog | `src-tauri/crates/provider-registry/src/lib.rs:1285` | M | Yes |
| P2-2 | Flip Bedrock `supports_streaming` → true (+ regenerate TS) | `.../provider-registry/src/lib.rs:2301`, `src/generated/providerRegistry.ts:2543` | S | Yes |
| P2-3 | Surface/log invalid AWS Transcribe language code (stop silent en-US fallback) | `src-tauri/src/asr/aws_transcribe.rs:519-522` | S | Yes |
| P2-4 | Drop undocumented `channels=1` on the Flux v2 URL | `src-tauri/src/asr/deepgram.rs:597` | S | Yes |
| P2-5 | Delete dead AssemblyAI v2-realtime parse branches | `src-tauri/src/asr/assemblyai.rs:1125` (+ consumers `speech/mod.rs:5099,5179`) | M | Yes |
| P2-6 | OpenRouter: prefer `max_completion_tokens` | `src-tauri/src/llm/openrouter.rs:327,355` | S | Yes (shares approach w/ P1-1) |

All P2 items are independent of one another and of P0/P1 — fully parallelizable. The only ordering constraint anywhere: P2-2 requires regenerating `providerRegistry.ts` from the Rust registry after editing `lib.rs`.

---

## 5. Cross-Provider Patterns

1. **Fake/mislabeled "default model id" recurs across providers.** The single confirmed break (Deepgram `general`) and several nits share the same root shape — a model-id string that is not a real, currently-valid API enum value is persisted or hardcoded and passed through untranslated:
   - Deepgram: persisted `general` (BROKEN — no enum member).
   - llm_openai_compat: `whisper-1` used as the LLM catalog default marker (an ASR id in a chat list).
   - aws_transcribe registry: `default_model = "transcribe-streaming"` (informational; never actually sent).
   - The mitigating factor everywhere except Deepgram is that the value is either never transmitted or is user-supplied. Deepgram is the exception where a stale value is transmitted verbatim with no clamp — which is exactly why it breaks.
   - **Systemic fix:** validate `config.model` against the provider's known/queryable model catalog (which most providers already have via `list_*_models_cmd`) before opening a session, with a fallback to the registry default. Deepgram is the first place to apply it.

2. **Deprecated `max_tokens` vs `max_completion_tokens`** appears in BOTH OpenAI-compatible LLM clients (`llm.api` and `openrouter`). Both always send `max_tokens`. Only real-OpenAI o-series/reasoning models reject it; every other target accepts it. A shared helper that picks the field by host would fix both (P1-1 + P2-6).

3. **Diagnostics only map the "expected" HTTP status.** Deepgram maps only 401 → actionable; a 400 (bad model — the real failure) falls through to a generic message, obscuring root cause. The pattern (map the happy-path auth error, drop the rest into a generic bucket) hides the most common misconfiguration failures. Broaden connect-error classification (P0-3) to cover 400 (and ideally 403/429) with actionable text.

4. **Catalog staleness is a UX-only class, not a correctness class.** deepgram_aura (Aura-2 missing), assemblyai (stale doc, not impl), speechmatics (fixed 1-of-3 catalog) all under-expose real models without breaking calls — because the underlying field is free-form or user-supplied. These are P2 completeness items, never P0.
