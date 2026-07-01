# OpenRouter API Surface Decision

Date: 2026-06-26
Seed: audio-graph-70cf
Status: architecture decision

## Scope

This note decides how AudioGraph should treat OpenRouter's Chat Completions,
Responses, and Anthropic-compatible Messages APIs.

The decision only covers OpenRouter API surface selection. It does not add new
runtime code, settings UI, workflow changes, credentials, generated provider
registry entries, or live API keys.

## Decision

Keep OpenRouter Chat Completions as AudioGraph's production OpenRouter runtime
surface.

Do not add user-selectable provider surfaces for OpenRouter Responses or
OpenRouter Anthropic Messages now.

- Treat OpenRouter Responses as a deferred beta integration. The official
  OpenRouter docs label it beta, warn that it may have breaking changes, and
  describe it as stateless. That is not a stable production default for
  transcript, notes, graph, or speak-aloud traffic.
- Treat OpenRouter Anthropic Messages as an Anthropic-format compatibility
  integration, not a general AudioGraph LLM provider surface. It may become
  useful for Claude-native request features, cache accounting, extended
  thinking, context management, or external Anthropic-compatible toolchains,
  but those are not required by the current OpenRouter production path.
- Keep health/readiness on no-content catalog and credential checks. Do not use
  `/chat/completions`, `/responses`, or `/messages` as automatic readiness
  probes because they require content-bearing prompt, input, or message bodies.

This keeps ADR-0005 intact: OpenRouter is first-class through one production
LLM adapter, with Chat Completions as the normalized model/provider routing
path. The Responses and Messages APIs stay behind future explicit beta or
compatibility Seeds.

## Current OpenRouter API Status

Source status was checked against current OpenRouter docs on 2026-06-26.

### Chat Completions

OpenRouter's API reference says its schemas are similar to the OpenAI Chat API
and that OpenRouter normalizes schema across models and providers. The Chat
Completions endpoint is:

```text
POST https://openrouter.ai/api/v1/chat/completions
```

The endpoint accepts conversation messages and supports streaming and
non-streaming modes. Its documented request schema includes `stream`,
`provider`, tools, reasoning, plugins, and `cache_control`, with top-level
cache control currently supported for Anthropic Claude models. This is the best
fit for AudioGraph's current normalized LLM adapter because it preserves one
request/streaming/usage shape across OpenRouter models and provider routing.

### Responses

OpenRouter documents:

```text
POST https://openrouter.ai/api/v1/responses
```

The docs call this the "Responses API Beta", say the API is in beta and may
have breaking changes, and say it is stateless, so each request must include
the full conversation history. The documented feature set includes reasoning,
tool calling, and web search integration.

For AudioGraph, those capabilities are not enough to make it selectable now.
They change the request and output shape, and tool or web-search use expands
content-egress risk beyond the current chat-completion path.

### Anthropic Messages

OpenRouter documents:

```text
POST https://openrouter.ai/api/v1/messages
```

The endpoint creates a message using the Anthropic Messages API format and
supports text, images, PDFs, tools, and extended thinking. The request shape
requires `model` and `messages`, and also documents OpenRouter-specific fields
such as `provider`, fallbacks, plugins, metadata, service tier, and
`session_id`. The example response shape is Anthropic-style content blocks and
usage fields, including cache and thinking-token details.

For AudioGraph, that is a compatibility surface. It is valuable if a future
Claude-specific feature needs Anthropic-native request or response semantics,
but it is not a replacement for the existing OpenRouter Chat Completions
adapter.

## Compatibility Constraints

AudioGraph's current OpenRouter path assumes OpenAI-compatible Chat Completion
semantics:

- request bodies use `messages`, model id, OpenRouter provider routing, and
  streaming flags;
- blocking responses normalize through `choices`, assistant message content,
  and usage;
- streaming responses normalize through the existing SSE token-delta path;
- provider routing is a field on the OpenRouter chat request builder;
- saved credentials and readiness avoid plaintext readback and content-bearing
  probes.

Responses and Messages do not share that full shape.

Responses has a separate output-item model and is explicitly beta. If it became
selectable, AudioGraph would need a separate typed adapter for response items,
streaming events, usage, tool-call deltas, web-search annotations, error
normalization, and privacy metadata.

Messages uses Anthropic-format content blocks and Anthropic-style usage
details. If it became selectable, AudioGraph would need a typed adapter that
maps text blocks, refusal/stop details, cache accounting, thinking-token
accounting, server tool use, streaming chunks, OpenRouter routing metadata, and
errors into the same internal chat/projection/speak-aloud contracts as the
Chat Completions path.

Neither surface should be added as a UI branch that bypasses the provider
architecture. Both would need backend-owned typed contracts, fixture coverage,
privacy metadata, readiness boundaries, and request/streaming parity tests.

## Smoke And Readiness Role

Automatic readiness should stay no-content:

- credential presence from the credential store;
- model/catalog calls such as `/api/v1/models` or `/api/v1/models/user`;
- provider/endpoint catalog calls when routing controls need them.

Content-bearing probes are not readiness. A live `input: "ping"` or
`messages: [{ role: "user", content: "ping" }]` request sends prompt content
to OpenRouter and to a routed upstream provider. That can only be a manual smoke
or CI/live-provider validation step with explicit policy gates, synthetic
non-session content, redacted logs, no API keys in docs or Seeds, and no
session transcript, notes, graph context, prompts, or generated audio.

Parser fixtures and recorded redacted response fixtures are acceptable for
adapter development, but they do not prove that runtime blocked-policy behavior
works. Any selectable content-bearing surface must satisfy the Provider
Addition Content-Egress Checklist in `docs/designs/provider-architecture.md`.

## Migration Risks

The main risk is not endpoint availability. The risk is multiplying production
LLM contracts without equivalent guardrails.

- Responses is beta and may change under a production desktop release.
- Responses and Messages require different output, streaming, tool, and usage
  parsers from Chat Completions.
- Tool calling, web search, plugins, files, PDFs, and extended thinking change
  what data classes may leave the device.
- Usage accounting would need to reconcile Chat Completions token counts with
  Responses usage items and Messages cache/thinking/server-tool fields.
- Settings would need to prevent users from accidentally selecting a beta or
  Claude-format path when the normalized Chat Completions path is sufficient.
- OpenRouter fallback/provider routing fields must preserve user intent across
  all surfaces; strict routing cannot silently relax just because an alternate
  API surface is selected.
- Stored credentials must remain backend-only. No smoke, error, debug, docs, or
  Seed entry may contain API keys or plaintext prompt/session content.

## Follow-Up Seed Policy

No implementation Seeds are required from this decision because neither
Responses nor Messages becomes selectable now.

If product requirements later make either surface selectable, create the
implementation Seeds before code work starts:

- OpenRouter Responses beta adapter spike: typed request/response/streaming
  fixtures, beta feature flag, no automatic readiness probe, content-egress
  guard, blocked-policy harness, usage mapping, and an explicit rollback path
  to Chat Completions.
- OpenRouter Messages compatibility adapter: typed Anthropic-format request,
  response, streaming, cache/thinking/server-tool usage mapping, Claude-feature
  scope statement, routing parity tests, blocked-policy harness, and UI copy
  that labels it as an Anthropic-compatible compatibility path.
- OpenRouter content-bearing smoke gates: one backend-owned manual smoke command
  per selectable surface, synthetic prompts only, no session content, redacted
  logs, no plaintext keys, and clear separation from readiness/model discovery.

Until those Seeds exist and pass acceptance, Chat Completions remains the only
production OpenRouter LLM surface.

## Sources

Fetched 2026-06-26 from primary OpenRouter documentation:

- `https://openrouter.ai/docs/api/reference/overview`
- `https://openrouter.ai/docs/api/api-reference/chat/send-chat-completion-request`
- `https://openrouter.ai/docs/api/reference/responses/overview`
- `https://openrouter.ai/docs/api/api-reference/anthropic-messages/create-messages`
- `https://openrouter.ai/docs/api/api-reference/models/get-models`
- `https://openrouter.ai/docs/api/api-reference/models/list-models-user`
