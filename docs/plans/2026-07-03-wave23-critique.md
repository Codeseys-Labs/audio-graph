# Wave-2/3 Critique — P6 Adjudication (2026-07-03)

Adjudicator: P6. App: `/mnt/e/CS/github/audio-graph`.

Scope reviewed:
- SambaNova credential slot — PR #47, commit `969fad7` (task 43 / `ff45`). NOTE: `969fad7` is **not** an ancestor of the currently-checked-out master `HEAD` (`8f11450`); it lives on the merged-wave branch. All SambaNova findings were re-verified against `git show 969fad7:...`.
- Export bundle (#43, `9c89`) + data-route UI (#42, `51e0`) — verified against merged snapshot `e93fb51`.

Verdict: 6 raw findings, **6 kept**, 0 dropped. All six reproduce against the reviewed artifacts. Ranked by severity×blocking below.

---

## KEPT (seedworthy)

### 1. [BLOCKING, sev 1] endpoint_api_key_from_store missing sambanova arm → readiness/health probe sends wrong key (or none)
- Type: bug
- Location: `src-tauri/src/commands.rs:8315-8330` (`969fad7:...:8570`)
- VERIFIED. `endpoint_api_key_from_store` matches `credential_key_for_endpoint(endpoint)` with arms for cerebras/openrouter/gemini/groq/together/fireworks and `_ => store.openai_api_key.as_deref()`. There is no `"sambanova_api_key"` arm. Meanwhile `settings/mod.rs:1341` `credential_value_for_endpoint` DOES have `"sambanova_api_key" => option_non_empty_secret(&store.sambanova_api_key)`, so the asymmetry is real: chat requests resolve the correct key, but the health/readiness/model-list surface does not.
- Reachability confirmed: `openai_compatible_readiness_arm` (`969fad7:...:7877-7896`) always calls `endpoint_api_key_from_draft_or_store(endpoint, None)` → store path → `endpoint_api_key_from_store`. The `"llm.sambanova"` readiness arm (`969fad7:...:7944`) routes through it, as do `test_sambanova_connection_cmd`/`list_sambanova_models_cmd` (`969fad7:...:8932,8949`) when `api_key: None`.
- Failure scenario: user saves valid `sambanova_api_key`, no `openai_api_key` → probe sends no auth → 401 → readiness reports FAILED for a valid saved key. If an `openai_api_key` is also saved, the OpenAI key is sent to `api.sambanova.ai` → 401 with a foreign credential.
- Test gap confirmed: `endpoint_api_key_resolution_routes_openai_compatible_slots` (`969fad7:...:12780`) covers the six existing slots but was not extended for sambanova.
- Acceptance: add `"sambanova_api_key" => store.sambanova_api_key.as_deref(),` to the match, and extend the resolution test to assert `SAMBANOVA_BASE_URL` resolves to the sambanova slot.

### 2. [sev 3] load_session_data_movement_cmd omits validate_session_id — path-traversal / defense-in-depth gap
- Type: gap
- Location: `src-tauri/src/commands.rs:5986-5990` (snapshot `e93fb51`)
- VERIFIED. Command body is `crate::persistence::load_data_movement_events(&session_id).map_err(AppError::from)` with no `validate_session_id`. Path is built at `user_data.rs:111` via `ledgers_dir()?.join(format!("{session_id}.movements.jsonl"))` — zero sanitization — and `load_jsonl` opens whatever path results (no canonicalize/containment check). Every sibling session command validates (`load_session_impl:5981`, and 6111/6120/6131/6182).
- Failure scenario: a `session_id` containing `..`/`/`/`\\` (e.g. `../../../../home/user/.audiograph/other/foo`) escapes `ledgers_dir` and reads any `*.movements.jsonl` outside it — an out-of-directory file-read primitive (suffix-constrained) reachable from any webview JS. Non-blocking because in the normal UI flow `session_id` is the app-controlled `loadedSessionId`, not attacker-supplied — hence a defense-in-depth deviation, not a live break.
- Acceptance: add `validate_session_id(&session_id).map_err(AppError::from)?` as the first line, and a test that a `..`/separator session_id errors instead of loading.

### 3. [sev 3] demo_credential_slot / DEMO_CREDENTIAL_KEYS omit sambanova_api_key → SambaNova-only user misdetected as credential-empty at first launch
- Type: edge-case
- Location: `src-tauri/src/settings/mod.rs:2127-2166` (`969fad7`)
- VERIFIED for the backend. `DEMO_CREDENTIAL_KEYS` lists openai/cerebras/openrouter/gemini/deepgram/assemblyai/soniox/gladia/speechmatics/elevenlabs/revai/groq/aws — no `sambanova_api_key`; `demo_credential_slot` has no sambanova arm. `all_demo_credentials_empty` iterates only that list, so a saved sambanova key is never counted.
- Failure scenario: first launch, `demo_mode: None`, only credential saved is `sambanova_api_key` → treated as all-empty → auto-dropped into demo mode, ignoring the configured cloud LLM. Narrow cohort (first-launch-only, SambaNova as sole pre-populated key).
- Note: the finding's secondary claim about `src/App.tsx FIRST_TIME_CREDENTIAL_KEYS` was NOT confirmed — grep of `969fad7:src/App.tsx` finds neither that symbol nor `sambanova`. Seed scoped to the backend omission only.
- Acceptance: add `"sambanova_api_key"` to `DEMO_CREDENTIAL_KEYS` + a `"sambanova_api_key" => Some(&store.sambanova_api_key)` arm in `demo_credential_slot`.

### 4. [sev 2] SambaNova has no frontend UI wiring — provider unselectable, Load-models no-ops, no Test button
- Type: incomplete
- Location: `src/components/settings/useSettingsController.tsx:101-108, 2680-2696` (`969fad7`)
- VERIFIED. `LLM_PROVIDER_SETTINGS_VARIANTS` = `[local_llama, api, cerebras, openrouter, aws_bedrock, mistralrs]` — no `sambanova`, so SambaNova never appears in the LLM provider dropdown. `modelCatalogCommandArgs` has cases for asr.deepgram/asr.soniox/llm.cerebras/llm.api/asr.api and `default: return null` — no `llm.sambanova`, so `handleRefreshModels` early-returns and `list_sambanova_models_cmd` is never invoked. No `handleTestSambanova`/`test_sambanova_connection_cmd` invocation in `src/`. Frontend refs to sambanova exist only in the credential-key type union and generated `providerRegistry.ts`.
- The commit's stated acceptance was scoped to credential slot + allowlist + endpoint routing (backend), so this is an incomplete-feature gap rather than a broken claim — but as shipped a user cannot select SambaNova, enter/test its key, or load its models from Settings.
- Acceptance: add `"sambanova"` to `LLM_PROVIDER_SETTINGS_VARIANTS`, add a `case "llm.sambanova"` to `modelCatalogCommandArgs`, wire the provider-change branch to `SAMBANOVA_BASE_URL`, and add a Test handler — OR explicitly track UI wiring as a documented follow-up.

### 5. [sev 2] SAMBANOVA_PREVIEW_MODEL "DeepSeek-V3.1-Terminus" is a deprecated/removed model ID
- Type: bug
- Location: `src-tauri/crates/provider-registry/src/lib.rs:69` (`969fad7`)
- Code presence VERIFIED (`SAMBANOVA_PREVIEW_MODEL = "DeepSeek-V3.1-Terminus"`, surfaced in the fixed catalog at lib.rs:1296-1297). The **removal/deprecation claim** rests on external SambaNova docs (reviewer: removal date 4/6/2026, replacement `DeepSeek-V3.1`) and was not re-verified against the live `/v1/models` list in this pass — treat as PLAUSIBLE pending a live catalog check.
- Failure scenario: a user on the fixed catalog (before/if the remote catalog loads) selects the "DeepSeek V3.1 Terminus" preview option → chat completion with id `DeepSeek-V3.1-Terminus` → model-not-found. Default `Meta-Llama-3.3-70B-Instruct` is unaffected, and per finding #4 the preview is currently unreachable from the UI, so blast radius is small.
- Acceptance: update `SAMBANOVA_PREVIEW_MODEL` (+ display_name) to a current id (e.g. `DeepSeek-V3.1`) verified against the live `/v1/models` list.

### 6. [sev 4] Data-route panel renders one DOM node per failed ledger event — redactedErrors un-deduped/un-capped
- Type: edge-case
- Location: `src/components/sessionDataRoute.ts:150-193` (`e93fb51`)
- VERIFIED. `buildSessionDataRouteReport` pushes a `RedactedError` for every event with `result.status === "failed"` and an error_code/message, with no dedup and no cap — unlike `providerTransfers`/`credentials`/`captureSources` which are Map/`pushUnique`-deduplicated. `SessionDataRoutePanel.tsx` renders `report.redactedErrors.map(...)` one `<li>` per entry.
- Failure scenario: a long-running session that logged many transient `provider_call_failed` events (whole ledger loaded into memory by `load_jsonl`, no streaming/limit) produces a proportional number of DOM rows, degrading render. Self-inflicted (app-written, append-only ledger — not network-attacker-controlled), so bounded by app behavior. Lowest severity.
- Acceptance: cap/collapse `redactedErrors` (group by provider_id+error_code with a count, or slice to most-recent N with a "+M more" affordance).

---

## DROPPED

None. All six raw findings reproduced against the reviewed artifacts with concrete failure scenarios.

## Notes on line-number drift
Raw findings cited a "merged snapshot e93fb51" for the SambaNova commit, but SambaNova code actually lives in commit `969fad7` (PR #47), which is not an ancestor of the working-tree HEAD `8f11450`. Cited line numbers for the SambaNova findings matched the `969fad7` tree once located (the crate path is `src-tauri/crates/provider-registry/src/lib.rs`, not top-level `provider-registry/`). This does not change any verdict — all findings were re-anchored and confirmed.
