# audio-graph-10ac — Graph projection decode + missing-`type` root cause (P2)

**Investigation stage — read-only against master @ `8989667`.**
Session `a26e85c0`, build `d5eaa91`. Model `openai/gpt-oss-120b` via OpenRouter,
`structured_output_mode=OpenRouterJsonSchema` on every projection call.

## TL;DR

Two distinct failure modes, both **Graph-only** (0 Notes generation failures in
the session; 20 Graph failures + 26 Graph deltas = the ~50% seed rate):

1. **17× "error decoding response body"** — NOT transient truncation and NOT the
   60s request timeout. The whole 3-attempt retry sequence for a single failure
   spans ~2.5s (e.g. `23:07:58.224 → 23:08:00.770`), i.e. each HTTP round-trip +
   decode returns **sub-second**, dominated by the 0.4s/1.0s retry backoffs. A
   body that decodes-fails deterministically and identically on all 3 attempts,
   fast, is a **response-envelope shape mismatch**, not a mid-body TCP drop. The
   blocking client's `response.json::<ChatCompletionResponse>()` (openrouter.rs
   ~1722) collapses shape-mismatch and transport-truncation into the same
   `reqwest::Error::is_decode()` bucket, which `is_retryable_chat_decode_error`
   (openrouter.rs:1899) then treats as transient and retries 3× — masking a
   deterministic decode failure as a "transient gateway failure."

2. **3× (seed says 6) "missing field `type`"** — surfaces at the *projection
   parse* layer (`parse_projection_patch_draft`), DESPITE OpenRouterJsonSchema
   strict mode, with 0 schema-rejection warnings (the provider accepted the
   schema; `require_parameters=true` did not 404). The error columns are deep
   into the compact JSON (`line 1 column 172/211/255/417`), i.e. a *later*
   operation in the array is missing its `type` discriminator while earlier ones
   are well-formed — the model rushing the tail of a large, budget-constrained
   graph patch.

The two modes share one upstream driver: **`max_tokens` is 512 for the common
OpenRouter-only user**, and `gpt-oss-120b` is a reasoning model whose reasoning
tokens consume that completion budget *before* content is emitted. Graph patches
are much larger than notes (10 operation variants, node+edge output) so graph is
where the 512 ceiling bites — either the model is cut off during/just-after
reasoning leaving `content` null/absent (→ envelope decode failure, mode 1) or it
truncates the tail of a multi-op patch (→ trailing op missing `type`, mode 2).

## Failure mode 1 — "error decoding response body" (17/20)

### What it is NOT
- **Not the 60s timeout / mid-body truncation.** `HTTP_REQUEST_TIMEOUT =
  Duration::from_secs(60)` (openrouter.rs:24). Measured per-failure wall time is
  ~2.5s across all 3 attempts (openrouter.rs retry backoffs 400ms + 1000ms =
  1.4s, leaving ~1.1s for the three actual round-trips). Nowhere near a read
  timeout.
- **Not random transport noise.** Notes calls traverse the identical
  send/decode path and the identical `ChatCompletionResponse` struct and had
  **0** decode failures in the session. A random gateway drop would not be
  100%-correlated with `kind=Graph`.

### What it is
`ChatCompletionResponse` (openrouter.rs:734-754) requires
`choices: Vec<Choice>` where `Choice { message: ChoiceMessage { content: String
} }` — **`content` is a required, non-nullable `String`** (openrouter.rs:761-764).
For a reasoning model routed by OpenRouter, a completion that is cut off during
reasoning (or that returns a 200 error/empty envelope) yields
`choices[0].message.content = null` (or the field absent, or a top-level
`{"error":{…}}` with no `choices`). Any of those makes serde fail
deserialization → reqwest reports `is_decode()` → the string "error decoding
response body". This is graph-biased because graph, under the 512-token ceiling,
is the kind that runs out of budget mid-reasoning.

The decode surface **cannot currently distinguish** truncated-body from
shape-mismatch (both are `is_decode()`), which is the observability gap called
out in the seed. `response.json()` also *consumes* the body, so the terminal
error carries neither body length nor the routed provider.

## Failure mode 2 — "missing field `type`" (3–6/20)

### Schema audit result: the schema is CORRECT; `type` IS required at the right level
`projection_patch_strict_json_schema` (projection_llm.rs:287-415) builds each
operation as a closed object via `variant()` (projection_llm.rs:305-322), which
inserts `type` into `properties` AND pushes `"type"` onto `required` first, with
`additionalProperties:false`. The Graph branch (projection_llm.rs:353-401) emits
all 10 graph variants under `operations.items.anyOf`, each with `type` required.
This is already unit-asserted (`strict_schema_requires_every_operation_field`,
`strict_schema_partitions_operations_by_kind`, projection_llm.rs:1631-1686) and
matches what `trusted_projection_patch_from_model_json` →
`parse_projection_patch_draft` requires (the internally-tagged
`ProjectionOperation`, `#[serde(tag = "type")]`, projections.rs:1120-1178).

**So the schema is not the bug.** The residual `missing field type` is the
provider emitting an operation object without the discriminator *despite* an
accepted strict schema. Two contributing factors, in order of likelihood:

1. **Budget-truncated tail.** The error columns (172–417) show earlier ops are
   well-formed and a *later* op lost its `type` — the signature of a model
   rushing the tail of a large patch under a tight completion ceiling
   (`max_tokens=512`). This is the same 512 driver as mode 1.
2. **Weak `anyOf` discriminator enforcement.** Many strict-mode engines enforce
   a top-level closed object tightly but under-enforce the tagged-union
   discriminator across a 10-branch `anyOf`. Graph has 10 branches vs Notes' 3,
   so graph is far more exposed. This is provider-side and not fully fixable
   client-side, but it is *observable* once the routed provider is logged.

Repair does not rescue these (3 of the failures survived repair) because the
single-provider chain re-runs the **same** OpenRouter backend with the **same**
schema and the **same** budget (executor.rs `run_projection_repair_escalation`
falls back to the producing backend when no next backend is configured — the
common OpenRouter-only case).

## Observability gap (confirmed part of the fix set)
- The projection path calls `chat_completion_with_schema_cached`
  (openrouter.rs:1607-1617), which **discards the routing telemetry** —
  `.map(|(text, telemetry)| (text, …total_tokens…))` throws away
  `telemetry.selected_provider` / `served_model`. So neither success nor failure
  logs *which upstream provider served the graph call*. We therefore cannot
  today distinguish "provider ignored the schema" from "provider honored it but
  the model truncated." `Projection patch backend output:` (executor.rs:846)
  logs only `provider=openrouter, model=…` — the OpenRouter slug, never the
  routed upstream (Cerebras/Together/etc.).

## Bounded fix set (exact locations)

### Fix 1 — Raise projection `max_tokens` (root driver for BOTH modes)
- **`commands.rs:901-905`** (`openrouter_config_from_runtime_settings`): the
  fallback when `llm_api_config` is `None` is `(512, 0.1)`. `llm_api_config`
  defaults to `None` (settings/mod.rs:1320), so an OpenRouter-only user gets a
  512-token completion ceiling for a reasoning model. Raise the projection floor.
- Preferred: make it **projection-kind-aware**. Thread a max_tokens override into
  the schema-cached send so `ProjectionKind::Graph` gets a larger budget than
  Notes. Options, least-invasive first:
  - (a) Bump the `None` fallback (and/or clamp projection requests to a floor,
    e.g. `>= 2048` for graph) in `commands.rs`.
  - (b) Add a `max_tokens` override parameter to
    `chat_completion_with_schema_cached` (openrouter.rs:1607) →
    `chat_completion_send` → `build_chat_completion_request`
    (openrouter.rs:485-509), set by `projection_openrouter`
    (executor.rs:952-1033) from `cache.kind`.
  - Note the existing eval harness uses `config.max_tokens = 384`
    (projection_eval.rs:742) — that is a *smoke* budget and must NOT be the
    production graph value; production graph needs MORE, not less.

### Fix 2 — Make the response envelope tolerant of reasoning-model shapes
- **`openrouter.rs:761-764`** (`ChoiceMessage`): change `content: String` to
  `#[serde(default)] content: Option<String>` (or default-to-`""`), and add
  `#[serde(default)] reasoning: Option<String>` / `reasoning_content` so a
  null/absent content no longer hard-fails the whole envelope as "error decoding
  response body". A missing/empty completion should then surface as an explicit
  "empty completion from OpenRouter" error (routable to the repair path or a
  clean skip), NOT a misclassified transient decode retry burning 3 attempts.
- Consider also tolerating the 200-with-`{"error":{…}}` envelope explicitly
  (map to a terminal, logged error rather than a blind decode retry).

### Fix 3 — Distinguish truncated-body from shape-mismatch + log routed provider
- **`openrouter.rs:1721-1738`** (the success-path decode branch in
  `chat_completion_send`): decode via `response.text()` then
  `serde_json::from_str::<ChatCompletionResponse>(&body)` so on failure we can
  log **body length** (short ⇒ truncation; full ⇒ shape mismatch) and a shape
  probe (has `choices`? has `error`? `content` null?), instead of letting
  `response.json()` consume the body and erase the evidence. Only retry when the
  body is genuinely partial/absent; a full body that fails shape decode is
  terminal (do not burn 3 attempts).
- **`openrouter.rs:1607-1617`** (`chat_completion_with_schema_cached`): stop
  discarding telemetry — return or log `selected_provider` / `served_model` so
  the projection layer can record the routed upstream on both success and
  failure. Thread it into `Projection patch backend output:` (executor.rs:846)
  and into the decode/validation error strings.
- **`openrouter.rs:1899-1901`** (`is_retryable_chat_decode_error`): tighten so a
  full-body shape mismatch is not classified retryable (only true
  partial-body/timeout is).

### Schema (mode 2): no structural change required
`type` is already `required` at the correct (per-variant) nesting level. Do NOT
add `minimum`/`maxLength`-style keywords (the a324 comment at
projection_llm.rs:280-286 explains strict engines 400 on those). The actionable
levers for missing-`type` are Fix 1 (budget) + Fix 3 (log the routed provider to
confirm anyOf-enforcement vs truncation). Optionally probe replacing the
single-value `enum` discriminator with `const` (projection_llm.rs:309-310) if the
routed-provider logs show a schema-honoring provider still dropping `type` — but
only after the logs implicate anyOf enforcement.

## Test plan (unit only — no live API calls)

**Schema variant (projection_llm.rs `#[cfg(test)]`)** — mostly present; add:
- Assert every Graph variant (all 10) lists `type` in `required` (extend
  `strict_schema_requires_every_operation_field` beyond `upsert_note`).
- Regression: a schema-obeying multi-op graph fixture with `type` on every op
  passes `parse_projection_patch_draft` (guards the "later op missing type"
  shape once budget is fixed).

**Request construction (openrouter.rs `#[cfg(test)]`)** — reuse the existing
in-process request-capture harness (`json_schema_request_carries_strict_schema_…`,
openrouter.rs:2831):
- A projection/graph request serializes the raised/kind-aware `max_tokens` into
  the body (assert `sent["max_tokens"]` is the graph value, not 512).
- Envelope tolerance: `serde_json::from_str::<ChatCompletionResponse>` succeeds
  when `content` is `null`/absent and when a top-level `reasoning` is present;
  and an `{"error":{…}}`/no-`choices` body maps to a terminal (non-retryable)
  error, not `is_decode` retry. (Pure struct-decode unit tests — no network.)
- `is_retryable_chat_decode_error` returns false for a full-body shape mismatch
  and true only for partial-body/timeout (may require a small classifier seam so
  it's testable without a live `reqwest::Error`).

**Executor (executor.rs `#[cfg(test)]`)** — extend the recorder-closure tests:
- The routed provider (`selected_provider`) is threaded into the projection
  backend output / error string (assert it appears in the logged/returned
  diagnostic).

## Evidence index (session a26e85c0, `audio-graph.log`)
- 21 "error decoding response body" total; 17 terminal on `kind=Graph`
  projection, rest on extraction. 0 on Notes.
- 3 "missing field `type`" projection failures, all `kind=Graph`, columns
  172/211/255/417 (trailing-op discriminator loss).
- 32 "response decode failed (attempt 1/3)" + 25 "(attempt 2/3)" retry warns —
  the bounded retry is firing but the failures are persistent, not transient.
- 40 "Projection patch backend output: provider=openrouter,
  model=openai/gpt-oss-120b, structured_output_mode=OpenRouterJsonSchema" — every
  call used strict schema mode; 0 schema-rejection ("structured projection
  output … falling back") warns, so `require_parameters` never 404'd.
- Single-failure wall time ~2.5s across 3 attempts ⇒ no timeout truncation.
- 20 Graph generation failures vs 26 Graph deltas ⇒ the ~50% seed rate;
  degraded, not dead.
