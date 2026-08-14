# Cerebras and OpenRouter Projection Transport Capabilities

Date: 2026-08-13

Status: Wayfinder research input; no product contract chosen

Scope: small structured notes patches, temporal-graph patches, and final Session
refinement output

Review correction (2026-08-14): clarified OpenRouter content logging, data-use,
and metadata retention; replaced obsolete API-reference links; reframed product
mandates as Wayfinder candidates; and narrowed routed-skin claims pending a
future authenticated synthetic route probe. All 33 unique first-party citation
targets returned HTTP 200; the targeted credential-pattern scan and
`git diff --check` passed.

## Question

What do direct Cerebras and Cerebras reached through OpenRouter currently expose
for structured output, API-format compatibility, streaming completion,
truncation detection, usage and latency provenance, limits, failures, model
identity, and content egress?

This note uses current first-party documentation and unauthenticated public
catalog APIs only. It does not validate a content-bearing request and does not
choose AudioGraph's production transport contract.

## Decision-relevant summary

| Concern | Direct Cerebras | Cerebras through OpenRouter |
| --- | --- | --- |
| Documented generation surface | OpenAI-style `POST /v1/chat/completions` | OpenRouter documents generic Chat Completions, OpenAI Responses, and Anthropic Messages skins; this review did not prove Responses or Messages parity for the selected Gemma-through-Cerebras route |
| Strict structured output | Cerebras says `strict: true` uses constrained decoding and guarantees exact schema adherence | OpenRouter supports JSON Schema, but explicitly warns that exact enforcement varies by upstream endpoint; endpoint capability and local validation still matter |
| Gemma 4 model identity | `gemma-4-31b` | `google/gemma-4-31b-it`; the provider endpoint may then be Cerebras or another host |
| Cerebras Gemma limits observed on 2026-08-13 | 131,072-token context; 40,960 maximum completion | The Cerebras endpoint reports the same limits, although the aggregate OpenRouter model reports 262,144 context and other providers expose different completion limits |
| Default routing behavior | One provider: Cerebras | OpenRouter load-balances providers and permits provider fallback by default unless constrained |
| Terminal success/truncation | Chat `finish_reason`; `length` is explicit truncation | Generic skin semantics (selected Gemma/Cerebras route unprobed): Chat `finish_reason`, Responses `status`/`incomplete_details`, Messages `stop_reason`; some length errors are deliberately transformed into successful responses with finish reason `length` |
| Usage/latency | Token usage, backend fingerprint, and `time_info` in documented Chat response | Token/cost usage on every response; generation ID, selected provider, routing metadata, native finish reason, and asynchronous generation metadata are available |
| Egress path | AudioGraph to Cerebras | AudioGraph to OpenRouter to the selected upstream endpoint; routing constraints are therefore part of the egress boundary |

The routed column combines public endpoint-catalog evidence with OpenRouter's
generic skin documentation. It does not prove that
`google/gemma-4-31b-it` on the selected Cerebras endpoint accepts every
Messages or Responses parameter or preserves each skin's semantics. An
authenticated synthetic route probe is future validation and was not performed
for this report.

The public catalogs are dynamic. Model IDs, endpoint availability, feature
flags, context/output limits, quantization, and privacy attributes are
discovery-time facts rather than constants. Cerebras documents its
public models endpoint as unauthenticated, capability-bearing metadata, while
OpenRouter's model and endpoint APIs likewise expose per-model and per-provider
metadata ([Cerebras public models API](https://inference-docs.cerebras.ai/api-reference/models/public-models),
[OpenRouter models documentation](https://openrouter.ai/docs/guides/overview/models)).

## Direct Cerebras

### API format

For projection generation, Cerebras documents an OpenAI-style Chat Completions
endpoint at `POST https://api.cerebras.ai/v1/chat/completions`. The request uses
`model`, `messages`, `max_completion_tokens`, `response_format`, and `stream`;
the response uses `choices`, `message`, `finish_reason`, `model`, `usage`,
`system_fingerprint`, and `time_info`
([Chat Completions reference](https://inference-docs.cerebras.ai/api-reference/chat-completions)).

As of this review, the Cerebras documentation index and generation API
reference do not document native OpenAI Responses (`/v1/responses`) or Anthropic
Messages (`/v1/messages`) endpoints. This is an inference from the documented
surface, not a claim that an undocumented compatibility route can never exist.
An adapter targeting direct Cerebras therefore has first-party evidence for
Chat Completions, not for those other two wire formats
([Cerebras documentation index](https://inference-docs.cerebras.ai/llms.txt)).

### Structured outputs

Cerebras accepts OpenAI-shaped `response_format` values:

- `type: "json_schema"` with `json_schema.strict: true` uses constrained
  decoding; Cerebras describes the result as guaranteed to match the schema.
- `strict: false` or an omitted `strict` treats the schema as a hint and may
  omit required fields, add fields, or emit incorrect types.
- `type: "json_object"` guarantees valid JSON, not schema adherence, and its
  Chat API reference says this legacy mode cannot be streamed.

These are provider guarantees only for schema syntax Cerebras supports. The
current strict subset requires an object root and `additionalProperties: false`
for every object. It limits a schema to 5,000 characters, 10 nesting levels,
500 object properties, and 500 total enum values. Unsupported constraints
include recursive schemas, external references, string `pattern`/`format`, and
array `minItems`/`maxItems`
([Cerebras structured outputs](https://inference-docs.cerebras.ai/capabilities/structured-outputs)).

API version 2 became the default in July 2026 and applies the stricter nested
`additionalProperties: false` validation. A candidate compatibility constraint
for Wayfinder is whether the response parser tolerates additional response
fields, because Cerebras reserves those additions as a non-breaking change
([Cerebras API versions](https://inference-docs.cerebras.ai/api-reference/versions)).

### Streaming and completion boundary

Cerebras supports streaming Chat Completions, including structured-output
requests. It emits standard SSE chunks and, since March 2026, guarantees one
terminal `data: [DONE]` marker as the last SSE event. Cerebras also notes that
it may batch multiple tokens into a stream event, so token-by-token delivery
must not be assumed
([streaming guide](https://inference-docs.cerebras.ai/capabilities/streaming),
[change log](https://inference-docs.cerebras.ai/support/change-log)).

`choices[].finish_reason` is documented as one of `stop`, `length`,
`content_filter`, or `tool_calls`. `length` means the output limit or total
context bound stopped generation; it is not a complete projection patch. The
`[DONE]` marker proves transport termination, not semantic or schema validity.
The final content, finish reason, and independent application schema validation
are separate signals
([Chat Completions response schema](https://inference-docs.cerebras.ai/api-reference/chat-completions)).

**Candidate constraint for the Wayfinder ticket:** should streamed JSON deltas
be buffered for latency while admission waits for a non-truncated terminal
choice and full schema validation? The sources establish separate transport,
terminal-status, and schema-validity signals; they do not choose AudioGraph's
admission rule.

### Current Gemma 4 identity and limits

The live unauthenticated Cerebras public model catalog returned this record on
2026-08-13:

- model ID: `gemma-4-31b`
- `streaming`, `structured_outputs`, `response_format`, and function calling:
  supported
- maximum context: 131,072 tokens
- maximum completion: 40,960 tokens
- deprecated: false; preview: false

Source: [`GET https://api.cerebras.ai/public/v1/models`](https://api.cerebras.ai/public/v1/models).
The public catalog contract explicitly identifies its `limits` and capability
fields and states that the endpoint needs no API key
([public models API reference](https://inference-docs.cerebras.ai/api-reference/models/public-models)).

`max_completion_tokens` includes reasoning tokens, and the sum of input and
generated tokens cannot exceed the model context. For small structured patches,
reasoning can therefore consume output budget even when the visible JSON is
short
([Chat Completions request schema](https://inference-docs.cerebras.ai/api-reference/chat-completions)).

### Usage, latency, and identity provenance

A non-streaming documented response includes prompt, completion, total, cached,
and reasoning-token usage where applicable. It also includes:

- the returned `model`;
- `system_fingerprint` for the model/backend;
- `time_info.queue_time`, `prompt_time`, `completion_time`, and `total_time`.

Those are provider measurements, not application-side request-attempt or
wall-clock timestamps. Whether Wayfinder requires the latter is a product
contract question
([Chat Completions response schema](https://inference-docs.cerebras.ai/api-reference/chat-completions)).

### Errors, retry behavior, and quotas

Cerebras documents 400, 401, 402, 403, 404, 422, 429, 500, and 503-class
errors. Its SDK retries connection errors, 408, 429, and 500-or-higher responses
twice by default with short exponential backoff; the one-minute default timeout
is also retried. These retries can be configured or disabled
([Cerebras error handling](https://inference-docs.cerebras.ai/support/error)).

Rate limits are organization/model scoped and can be hit by request or token
buckets. Cerebras estimates a request's token demand using the input plus the
declared `max_completion_tokens` (or the remaining maximum sequence length), so
an unnecessarily large output ceiling can cause pre-processing rate rejection.
Responses expose remaining/reset quota headers; exceeding a limit yields 429
([Cerebras rate limits](https://inference-docs.cerebras.ai/support/rate-limits)).

**Unresolved direct-transport fact:** the first-party docs do not advertise an
idempotency key for Chat Completions. SDK or application retries may therefore
represent distinct billed generations. **Candidate question for the Wayfinder
ticket:** does the product contract require a stable attempt identity and
single-admission rule even when the provider retries internally? This research
does not select that rule.

## Cerebras through OpenRouter

### Three wire skins, one routing layer

OpenRouter exposes:

- `POST /api/v1/chat/completions`, OpenAI Chat Completions format;
- `POST /api/v1/responses`, an OpenAI-compatible Responses API currently
  documented as beta and stateless;
- `POST /api/v1/messages`, Anthropic Messages format.

OpenRouter describes these as three API skins over the same internal provider
error vocabulary, with different wire locations for terminal state and errors
([OpenRouter errors and debugging](https://openrouter.ai/docs/api/reference/errors-and-debugging),
[Responses overview](https://openrouter.ai/docs/api/reference/responses/overview),
[`/messages` OpenAPI operation](https://openrouter.ai/openapi.json#/paths/~1messages/post)).

The Responses skin uses output-item SSE events and ends successfully with a
`response.done` carrying `status: "completed"`. It is stateless: OpenRouter says
`store: true` and `previous_response_id` are unsupported, so prior state must be
sent explicitly
([Responses basic usage](https://openrouter.ai/docs/api/reference/responses/basic-usage)).

The Messages skin accepts an OpenRouter `provider` object and an
`output_config` with structured-output format, returning Anthropic-style content
blocks, `stop_reason`, and usage. The live OpenAPI declares a generic `model`
string but does not establish a per-model/per-provider compatibility matrix
([Messages request and response schemas](https://openrouter.ai/openapi.json#/components/schemas/MessagesRequest)).
This review did not make an authenticated call proving Gemma-on-Cerebras parity
through the Messages skin.

### Structured outputs and endpoint variability

OpenRouter accepts Chat `response_format.type: "json_schema"` and supports
streaming structured output. However, its own documentation makes two important
qualifications:

1. support is per provider endpoint, not just per model, and can change; and
2. even with `strict: true`, some endpoints provide native exact enforcement,
   while others translate the schema or treat it as a strong hint.

OpenRouter therefore advises `provider.require_parameters: true`, but that only
filters endpoints by advertised parameter support; it does not turn every
endpoint's semantics into Cerebras's constrained-decoding guarantee
([OpenRouter structured outputs](https://openrouter.ai/docs/guides/features/structured-outputs)).

OpenRouter's public Gemma record uses model slug `google/gemma-4-31b-it`. The
live endpoint catalog on 2026-08-13 listed many providers with differing
quantization, context, maximum completion, supported parameters, and health.
The Cerebras endpoint specifically reported:

- provider: Cerebras (`cerebras/fp16`)
- context: 131,072 tokens
- maximum completion: 40,960 tokens
- `structured_outputs` and `response_format`: supported

Source: [`GET /api/v1/models/google/gemma-4-31b-it-20260402/endpoints`](https://openrouter.ai/api/v1/models/google/gemma-4-31b-it-20260402/endpoints).
By contrast, the aggregate model record reports 262,144 context because another
endpoint can provide it
([OpenRouter Gemma 4 model page](https://openrouter.ai/google/gemma-4-31b-it/api)).

**Candidate constraint for the Wayfinder ticket:** should admission evaluate
capacity against the selected endpoint rather than the aggregate OpenRouter
model? A refinement request that fits the aggregate 262K declaration may not
fit the Cerebras endpoint's 131K context; this report does not select the
product rule.

### Routing and model identity

OpenRouter load-balances among providers and defaults `allow_fallbacks` to
`true`. Its provider preferences can constrain `only` or ordered providers,
disable fallbacks, require request-parameter support, deny endpoints that may
collect data, and require ZDR. These fields are independent; asking for
structured outputs does not by itself pin Cerebras or disable fallback
([provider routing](https://openrouter.ai/docs/guides/routing/provider-selection)).

OpenRouter can return the model and selected provider. `X-Generation-Id` is
available on all generation endpoints. Opting into `X-OpenRouter-Metadata:
enabled` adds routing data, including the requested route, selected endpoint
summary, and attempt count; for a stream it arrives in the terminal chunk/event.
The authenticated generation metadata API can later expose `provider_name`,
`model`, `native_finish_reason`, latency, generation time, upstream ID, and token
counts
([streaming API](https://openrouter.ai/docs/api/reference/streaming),
[router metadata](https://openrouter.ai/docs/guides/features/router-metadata),
[generation metadata API](https://openrouter.ai/docs/api/api-reference/generations/get-generation)).

### Streaming, truncation, and in-band failures

OpenRouter streams with SSE. Before any token is emitted, an error can still be
returned as an HTTP failure and OpenRouter can silently try another provider if
fallback is enabled. After the first token, HTTP status is already 200 and
OpenRouter cannot fail over; a disconnect, timeout, token limit, or output
filter is reported in-band as an SSE error
([OpenRouter streaming error behavior](https://openrouter.ai/docs/api/reference/errors-and-debugging)).

The terminal signals differ by skin:

- Chat Completions: `finish_reason`, with `length` or `error` rejecting
  completeness. Mid-stream errors carry a top-level error and
  `finish_reason: "error"`.
- Responses: a completed response has `status: "completed"`; the response schema
  exposes `incomplete_details.reason`, including `max_output_tokens`, and failed
  streams use terminal failure/error events.
- Messages: `stop_reason`; token exhaustion is represented in Anthropic-style
  semantics, while mid-stream failures are SSE `error` events containing
  OpenRouter's stable `error_type`.

OpenRouter deliberately transforms several length-limit provider errors into
successful responses with finish reason `length`. Consequently, HTTP 200 and
the absence of a top-level error are insufficient completion tests
([OpenRouter error transformations](https://openrouter.ai/docs/api/reference/errors-and-debugging),
[Responses `IncompleteDetails` OpenAPI schema](https://openrouter.ai/openapi.json#/components/schemas/IncompleteDetails)).

### Usage, cost, and latency

OpenRouter automatically includes token usage and cost on every non-streaming
response or final SSE message. Documented fields include prompt, completion,
reasoning, cached-token, total-token, total-cost, and upstream-cost data where
available. The generation metadata API adds provider and latency attribution
([usage accounting](https://openrouter.ai/docs/cookbook/administration/usage-accounting)).

The final usage event can itself be lost if the connection fails. **Candidate
questions for the Wayfinder ticket:** may a projection be admitted when billing
metadata is absent, and should missing usage/latency be represented explicitly
rather than as zero? This report does not select those contract answers.

### Errors and retries

OpenRouter documents stable typed `error_type` values across the three skins.
Relevant status classes include invalid request, insufficient credits,
forbidden/guardrail, timeout, rate limit, provider invalid/down, and no provider
meeting routing requirements. A `Retry-After` header may accompany 429 and 503;
official SDKs honor it. Mid-stream failures remain HTTP 200 and must be parsed
from the stream. On 500-class errors, provider details are masked
([OpenRouter errors and debugging](https://openrouter.ai/docs/api/reference/errors-and-debugging)).

OpenRouter does not document an idempotency key for these generation calls in
the reviewed references. As with direct Cerebras, retrying is a new generation
unless a separate guarantee is established.

## Content-egress implications

Direct Cerebras sends transcript-derived content to Cerebras. Cerebras's public
terms reserve use of service content to provide the service, comply with law,
and enforce terms, while explicitly saying this does not grant a right to train
or fine-tune models on that content. The public terms do not establish a blanket
zero-retention guarantee for every direct API request
([Cerebras terms of service](https://www.cerebras.ai/terms-of-service)).

Cerebras does state that its automatic prompt caches are ZDR-compliant,
ephemeral in memory, not persisted, and isolated by organization. That cache
statement is narrower than a complete inference-service retention contract
([Cerebras prompt caching](https://inference-docs.cerebras.ai/capabilities/prompt-caching)).

Through OpenRouter, both OpenRouter and the selected provider process the
request. OpenRouter documents two independent, off-by-default content options:
Private Input & Output Logging stores full prompts and completions for the
account's review, with a minimum three-month retention, but OpenRouter says it
does not access or use that logged content; a separate Privacy setting permits
OpenRouter to use inputs and outputs to improve the product in exchange for a
discount. Either, both, or neither can be enabled. Separately, OpenRouter stores
request metadata such as token counts and latency even when prompt/response
content is not stored; it says that metadata excludes the content itself
([OpenRouter data collection](https://openrouter.ai/docs/guides/privacy/data-collection),
[input/output logging and independent data-use opt-in](https://openrouter.ai/docs/guides/features/input-output-logging)).

Provider policies remain endpoint-specific. Per-request `provider.zdr: true`
filters to ZDR endpoints, while `data_collection: "deny"` filters endpoints that
may collect data
([OpenRouter ZDR](https://openrouter.ai/docs/guides/features/zdr),
[provider logging](https://openrouter.ai/docs/guides/privacy/provider-logging/)).

OpenRouter's current provider directory labels Cerebras as no-training and ZDR,
but OpenRouter describes those properties as dynamic endpoint policy metadata
([OpenRouter providers](https://openrouter.ai/providers),
[Cerebras provider page](https://openrouter.ai/provider/cerebras)).

**Candidate questions for the Wayfinder ticket:** should “Cerebras through
OpenRouter” be authorized as a distinct egress route from direct Cerebras, even
if Cerebras ultimately performs inference, and which provider pinning, fallback,
ZDR/data-collection, and logging-state evidence should accompany an attempt?
This report identifies the distinct processing path but does not select the
authorization or provenance contract.

## Facts a transport contract can normalize

Across direct Cerebras and OpenRouter's generic wire-skin documentation, the
sources expose the following candidate normalized facts without proving their
availability for every selected route/model/skin combination:

- requested route, requested model, actual model, and actual provider;
- transport format: Chat Completions, Responses, or Messages;
- attempt/generation ID and backend fingerprint or upstream identity when
  supplied;
- terminal status: completed, truncated, refused/filtered, failed, or transport
  lost;
- complete buffered output plus independent schema-validation result;
- prompt/completion/reasoning/cache token counts and cost when present;
- request, queue, prompt, first-token, generation, and total timing when present;
- provider routing/fallback attempt metadata;
- the content-egress policy snapshot that admitted the request.

This is an evidence inventory, not the product decision about which fields are
mandatory, which skin is preferred, or when to retry.

## Unresolved unknowns

1. No authenticated synthetic request was made and no credentials were
   requested or used. Future validation could run a content-egress-approved
   synthetic route probe for strict schemas, refusal, `length`, and mid-stream
   failure against direct Cerebras and the pinned OpenRouter Cerebras endpoint.
2. OpenRouter documents generic Responses and Messages skins, but this review
   did not prove that every Gemma/Cerebras parameter is preserved identically
   through each skin. `debug.echo_upstream_body` can show the transformed
   upstream request during an authorized synthetic streaming probe.
3. The direct Cerebras documentation does not expose a Chat Completions
   idempotency mechanism or fully specify usage/timing placement in structured
   streams. Future validation options include fixtures or an authorized live
   probe.
4. Direct Cerebras's full retention/deletion commitments may depend on account
   terms or an enterprise data-processing agreement not available in public
   documentation.
5. OpenRouter endpoint limits and privacy attributes are dynamic. The public
   catalogs establish current discoverability, not a long-term SLA.

## Source discipline

All external claims above come from official Cerebras or OpenRouter pages and
their public APIs, initially fetched on 2026-08-13 and link-checked again on
2026-08-14. No secondary provider comparison, benchmark blog, community post,
or model aggregator was used.
