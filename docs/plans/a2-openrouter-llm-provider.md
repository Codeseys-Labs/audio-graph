# Plan A2: OpenRouter as first-class LLM provider

**Goal:** Add a `LlmProvider::OpenRouter` variant alongside the existing
`Api` variant. Surface it as a labeled choice in settings, validate keys
against `/api/v1/models`, populate a model picker from the live catalog,
ship optional attribution headers by default.

**ADR:** [0005](../adr/0005-openrouter-as-recommended-llm-endpoint.md) (accepted).

**Backlog:** audio-graph-c847.

## Acceptance criteria

- [ ] `src-tauri/src/settings/mod.rs`: new `LlmProvider::OpenRouter` variant
  with `OpenRouterSettings { model: String, base_url: String,
  provider_order: Option<Vec<String>>, include_usage_in_stream: bool }`.
- [ ] Default `base_url`: `https://openrouter.ai/api/v1`. Default
  `include_usage_in_stream`: `true`. Default `model`: empty string until
  user picks one (settings UI must enforce non-empty before save).
- [ ] `src-tauri/src/llm/api_client.rs` or `src-tauri/src/llm/openrouter.rs`
  (new file, mirrors `api_client.rs` but with hardcoded base URL +
  attribution headers): impl `LlmEngine` for OpenRouter. Internally reuses
  the OpenAI-compat HTTP path; only the request builder differs.
- [ ] Default headers per request:
  - `Authorization: Bearer <key>` (always)
  - `HTTP-Referer: https://github.com/Codeseys-Labs/audio-graph` (always; can
    be overridden via Settings if user wants)
  - `X-OpenRouter-Title: AudioGraph` (always; alias `X-Title` accepted)
- [ ] Tauri commands:
  - `test_openrouter_connection_cmd(api_key: String) -> Result<(), String>`
    — calls `GET /api/v1/models` with the key + headers; returns Ok on 200,
    Err with diagnostic on 401/403/network failure.
  - `list_openrouter_models_cmd(api_key: String) -> Result<Vec<OpenRouterModel>, String>`
    — same call, returns parsed catalog.
  - `OpenRouterModel { id, name, context_length, pricing: { prompt, completion } }`
- [ ] Credentials allowlist: add `openrouter_api_key` to `src-tauri/src/credentials/mod.rs`
  + matching string literal in `src/types/index.ts`. Migration: existing
  installs without the key see an empty value, no error.
- [ ] LLM executor (`src-tauri/src/llm/executor.rs`) routing: when
  `LlmProvider::OpenRouter` is selected, dispatch to the OpenRouter engine
  same way `Api` is dispatched today.
- [ ] Frontend `src/components/SettingsPage.tsx`: new "OpenRouter" option in
  the LLM provider dropdown. When selected, show: API key input (with
  Save + Test buttons), model picker (populated by `list_openrouter_models_cmd`,
  cached for 5 min in component state).
- [ ] Frontend types in `src/types/index.ts`: `OpenRouterSettings`,
  `OpenRouterModel`.

## Files

**New:**
- `src-tauri/src/llm/openrouter.rs` (~250–400 LOC: HTTP client, model list
  fetch, test command). Could also live as a section of `api_client.rs`
  if the diff stays small; new file preferred for clarity.

**Modified:**
- `src-tauri/src/settings/mod.rs` — enum variant + struct
- `src-tauri/src/llm/mod.rs` — module declaration if new file
- `src-tauri/src/llm/executor.rs` — routing dispatch
- `src-tauri/src/commands.rs` — two new test/list commands
- `src-tauri/src/credentials/mod.rs` — allowlist entry
- `src/components/SettingsPage.tsx` — UI plumbing (this is the 1910-line
  file flagged in `docs/reviews/audio-graph-review-loop23.md`; touch
  minimally — a new dropdown branch + a new credentials block)
- `src/types/index.ts` — TS types
- `src/store/index.ts` — if model picker state needs store residence

## Steps

1. Read `docs/research/verified-2026-05-19.md` (OpenRouter section).
2. Read `src-tauri/src/llm/api_client.rs` (full file) and
   `src-tauri/src/llm/executor.rs` to understand the dispatch shape.
3. Add the enum variant + struct in `settings/mod.rs`. Run `cargo check`
   to surface every match-statement that needs the new arm.
4. Implement `openrouter.rs` (or section of api_client.rs):
   - `pub async fn test_connection(api_key) -> Result<()>` hits
     `GET /api/v1/models` with timeout 10s.
   - `pub async fn list_models(api_key) -> Result<Vec<OpenRouterModel>>`
     parses `{ data: [...] }`.
   - `pub async fn chat_completion(...) -> ...` mirrors the api_client
     surface but with hardcoded base URL + attribution headers. Streaming
     support is plan A3's scope; this plan's chat impl can be blocking
     just to hit acceptance, then A3 makes it streaming.
5. Wire commands in `commands.rs`. Test commands typed correctly per
   Tauri.
6. Frontend dropdown: add an "OpenRouter" option after "API endpoint".
   Add credentials manager block. Add model picker (a `<select>` populated
   on first key-save, with TTL 5 min in local component state).
7. Run `cargo fmt`, `cargo clippy`, `cargo test --lib llm`. Frontend:
   `bun run typecheck && bun run test`.

## Tests

Unit tests in `llm/openrouter.rs`:

- `test_connection_succeeds_on_200` — mock HTTP server returns 200 +
  empty body; assert Ok.
- `test_connection_fails_on_401` — mock returns 401; assert Err with
  "invalid_key" classification.
- `list_models_parses_data_array` — mock returns canonical OpenRouter
  models response shape; assert N models parsed with correct fields.
- `chat_request_includes_attribution_headers` — assert HTTP-Referer +
  X-OpenRouter-Title in the outgoing request.

Frontend tests:

- `SettingsPage.test.tsx`: extend existing test file to cover the
  OpenRouter dropdown branch (key save + test button click + model
  picker render).

## Dependencies on other plans / ADRs

- ADR-0005 (this plan's spec)
- audio-graph-c847 (this plan's tracker)
- Does NOT depend on A1 (TTS) or A3 (streaming).
- A3 (streaming chat) builds on top of this plan's blocking chat, but
  doesn't strictly block it.

## Rollback

Revert the variant + struct in settings, the new file, the routing
branch, the commands, the UI, the credentials entry, the TS types.
Existing `LlmProvider::Api` continues to work; users who'd configured
OpenRouter manually via base URL keep working.

## Out-of-scope

- Streaming chat — that's plan A3 / ADR-0006.
- Per-message provider routing (`provider.order` per call) — settings-
  level only for v1.
- BYOK passthrough beyond just using the user's OpenRouter key.
- Cost dashboard integration (audio-graph-2e40 separately).
