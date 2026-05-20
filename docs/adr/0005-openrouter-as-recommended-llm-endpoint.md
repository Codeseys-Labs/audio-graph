# ADR-0005: OpenRouter as Recommended Cloud LLM Endpoint

## Status

Accepted 2026-05-19 for phased implementation.

## Context

AudioGraph has a generic `LlmProvider::Api` variant that points at any
OpenAI-compatible base URL. A user can already wire OpenRouter today by
manually entering `https://openrouter.ai/api/v1` and pasting their API
key. But the experience is inferior to a first-class option: the model
picker doesn't pre-populate, the credentials UI doesn't have an
"OpenRouter" entry, and there's no `test_openrouter_connection_cmd` to
validate the key without firing a real chat completion.

The goal correction (2026-05-19) names OpenRouter as the **recommended**
cloud LLM for the speech-to-graph/notes pipeline. That makes a UX
difference: a user landing on the settings page should see "OpenRouter"
as a labeled option, not be forced to know it's an OpenAI-compat endpoint.

OpenRouter's API is a strict superset of the OpenAI Chat Completions
schema with three additions: optional attribution headers (`HTTP-Referer`,
`X-OpenRouter-Title`), a `provider.order` request body field for upstream
preference, and `cache_control` for Anthropic models. Rate limits are
per-key, and a `/api/v1/models` endpoint returns the live catalog.

## Decision Drivers

- User has stated OpenRouter is the chosen cloud LLM. UX must reflect that.
- Model catalog is dynamic — OpenRouter adds and deprecates models weekly.
  Hardcoding a default is fragile; the model picker must hit
  `/api/v1/models` at settings-load time.
- Credential storage already exists (`credentials.yaml` + the
  `credentials/mod.rs` allowlist); OpenRouter must reuse it, not introduce
  a parallel keying surface.
- Token usage should flow into the existing lifetime-usage tracker (see
  audio-graph-2e40). OpenRouter returns usage in the final SSE chunk when
  `stream_options: { include_usage: true }` is set; we should opt into that.
- Streaming: this ADR enables the streaming chat path (which the
  speak-aloud loop, audio-graph-92c7, depends on). See ADR-0006 for
  streaming itself; this ADR is about provider wiring.

## Considered Options

- **Option A**: Add a first-class `LlmProvider::OpenRouter` variant with
  its own credentials key, settings shape (model picker, optional
  attribution headers), and `test_openrouter_connection_cmd`. Internally
  it reuses `api_client.rs` with a hardcoded base URL.
- **Option B**: Keep `LlmProvider::Api` as the only mechanism. Add an
  "OpenRouter preset" to the existing UI that pre-fills base URL + key
  field labels but is otherwise just the generic flow.
- **Option C**: Hardcode OpenRouter as the default `LlmProvider::Api`
  base URL on fresh installs. No new variant, no new UI.

## Decision Outcome

Chosen option: **Option A** (first-class OpenRouter variant). Rationale:
the settings UI grows a labeled OpenRouter entry, the credentials
allowlist gets a dedicated `openrouter_api_key` slot, and the test-
connection command validates against `/api/v1/models` without needing a
generic "ping a chat-completion endpoint" path. Future provider features
(model picker auto-populate, attribution headers, provider.order pass-
through) have a natural home on the variant.

### Consequences

- **Positive**: Settings UI surfaces OpenRouter as a labeled option, not
  a generic "OpenAI-compatible". Onboarding friction drops.
- **Positive**: `test_openrouter_connection_cmd` calls `/api/v1/models` —
  fast, free, validates key + network without spending tokens.
- **Positive**: Attribution headers (`HTTP-Referer: github.com/...`,
  `X-OpenRouter-Title: AudioGraph`) ship by default; we get free placement
  on openrouter.ai's leaderboard.
- **Positive**: Future per-provider features (cache_control for Anthropic
  models, provider routing, BYOK passthrough) have a natural home.
- **Negative**: Adds a new `LlmProvider` variant to the settings enum,
  which means migrating settings.json on existing installs (low risk —
  the migration path is already tested for `AsrProvider`).
- **Negative**: Model picker UI requires a new `list_openrouter_models_cmd`
  + frontend-side caching (TTL ~5 min) so we don't hammer the catalog
  endpoint on every settings render.
- **Neutral**: Generic `LlmProvider::Api` stays for users who want to
  self-host vLLM, run an Ollama proxy, or use a non-OpenRouter
  OpenAI-compat service.

## Pros and Cons of the Options

### Option A: First-class OpenRouter variant

- Good, because: labeled UI matches user mental model.
- Good, because: dedicated test command + dedicated model-list command
  give clear failure modes.
- Good, because: future OpenRouter-specific features (caching, plugins,
  routing) extend the variant naturally.
- Bad, because: variant proliferation — each new "this is just
  OpenAI-compat with a twist" provider would tempt the same treatment.
- Bad, because: settings migration cost on existing installs (small but
  real).

### Option B: Generic Api variant + UI preset

- Good, because: zero settings-shape churn.
- Good, because: works today already (users can already do this manually).
- Bad, because: doesn't address the UX gap. Users still see "API endpoint"
  and have to know it means OpenRouter.
- Bad, because: provider-specific features (attribution headers, model
  picker) become awkward "preset-aware" branches in generic code.

### Option C: Hardcode OpenRouter default base URL

- Good, because: fastest implementation.
- Bad, because: a user who wanted self-hosted vLLM (a real future use
  case per audio-graph-0af2) gets surprised.
- Bad, because: makes generic `Api` variant dual-use, blurring its
  contract.

## Implementation outline (informational)

```rust
// src-tauri/src/settings/mod.rs
pub enum LlmProvider {
    LocalLlama,
    Api,                    // generic OpenAI-compat (vLLM, Ollama, etc.)
    OpenRouter,             // NEW — first-class
    AwsBedrock,
    MistralRs,
}

pub struct OpenRouterSettings {
    pub model: String,                  // populated by /api/v1/models picker
    pub base_url: String,               // default https://openrouter.ai/api/v1
    pub provider_order: Option<Vec<String>>, // passthrough to provider.order
    pub include_usage_in_stream: bool,  // default true
}
```

New Tauri commands:
- `test_openrouter_connection_cmd(key: String) -> Result<(), String>`
  hits `GET /api/v1/models` with the key.
- `list_openrouter_models_cmd() -> Result<Vec<OpenRouterModel>, String>`
  same endpoint, returns the catalog for the model picker.

Streaming chat is tracked separately in ADR-0006.

## References

- `docs/research/verified-2026-05-19.md` — OpenRouter protocol facts
  verified from openrouter.ai docs on 2026-05-19
- audio-graph-c847 (seeds issue)
- audio-graph-2e40 (token usage tracking — depends on this)
