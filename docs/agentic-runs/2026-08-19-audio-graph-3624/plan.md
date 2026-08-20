# Implementation Plan — ADR-0038 Single-Skin Named Route Table (`audio-graph-3624`)

Authority: [`docs/adr/0038-route-llm-operations-through-a-single-skin-named-route-table.md`](../../adr/0038-route-llm-operations-through-a-single-skin-named-route-table.md)
(accepted), read in full, plus
[ADR-0033](../../adr/0033-enforce-mvp-provider-enablement-at-content-start.md) and
[ADR-0035](../../adr/0035-record-post-stop-finalization-failure-as-per-session-finalization-blocked.md).

The design under implementation is the one prepared for this seed, **as amended by
every required change in the critique**. The four critique findings are folded in
as §7 below; they are not optional.

## 0. Order of work

1. RED-1 — behavioural fallback-removal tests that compile against today's code
   and fail on today's behaviour. Recorded verbatim in `report.md`.
2. RED-2 — the remaining defect tests, which reference API that does not exist
   yet, so they fail at compile time. Recorded verbatim in `report.md`.
3. Implement `src-tauri/src/llm/route.rs`, then the executor rewrite, then the
   two wire layers, then provenance, then the docs.
4. Run every gate to green; fix until green.
5. `check-anchors.py` for every `file:line` citation in this directory.

## 1. The route table

New module `src-tauri/src/llm/route.rs`, `pub mod route;` in `llm/mod.rs`. It owns:

- `WireSkin` / `AdmittedSkin` — Chat Completions is the only MVP-admitted skin.
  `AdmittedSkin` has exactly one variant and the only constructor is
  `WireSkin::admitted()`, which returns `None` for `Messages` / `Responses`. Every
  dispatch entry point takes `AdmittedSkin`, so the reserved variants have no
  dispatch arm at all — not a rejected one.
- `EndpointCapability` — per-ENDPOINT facts. Both token fields are `Option<u32>`
  (critique finding 3): only `route.cerebras_direct` has an in-repo citation
  (ADR-0038:54-56 → 131,072 / 40,960). Every other row declares `None`, which
  makes the clamp a no-op rather than a fabricated number.
- `ConstrainedDecodingGrade` — `GuaranteedConstrained` / `AdvertisedHint` /
  `Unconstrained`. Recorded and graded, never a dispatch gate (ADR-0038
  sub-decision 5).
- `RouteDescriptor` + `LLM_ROUTES` (8 rows) + `resolve_route(&LlmProvider)`.
- **No `authorized_fallbacks` field and no chain walker.** ADR-0038:196-197 and
  :328-330 make option A with empty lists identical to option D. An empty list
  plus a live walker is the config default the seed forbids, so the mechanism is
  absent. Adding an alternate later needs a new ADR *and* new dispatch code.

Route rows and their ADR-0033 gate descriptor:

| Route id | `provider_id` | Resolved from |
|---|---|---|
| `route.local_llama` | `llm.local_llama` | `LlmProvider::LocalLlama` |
| `route.mistralrs` | `llm.mistralrs` | `LlmProvider::MistralRs` |
| `route.cerebras_direct` | `llm.cerebras` | `Api{endpoint}` + `is_cerebras_endpoint` |
| `route.sambanova_direct` | `llm.sambanova` | `Api{endpoint}` + `is_sambanova_endpoint` |
| `route.openai_compatible` | `llm.api` | any other `Api{..}` |
| `route.openrouter` | `llm.openrouter` | `OpenRouter{..}` (unpinned) |
| `route.cerebras_via_openrouter` | `llm.openrouter` | `OpenRouter{..}` with a **singleton** Cerebras pin |
| `route.aws_bedrock` | `llm.aws_bedrock` | `AwsBedrock{..}`; `blocking_backend: None` |

`route.aws_bedrock` gets an honest terminal error ("no blocking route; Bedrock
serves streaming chat only") instead of today's misleading "API LLM client is not
configured".

## 2. The ADR-0033 gate as a capability token

```rust
pub struct AuthorizedRoute { route, skin, completion_budget, _seal: () }
pub fn authorize_route_dispatch(provider: &LlmProvider) -> Result<AuthorizedRoute, AppError>
```

`_seal` is private, so `authorize_route_dispatch` is the only constructor in this
crate. It calls `ensure_provider_id_start_enabled(route.provider_id)`
unconditionally and rejects a reserved skin. Every backend attempt function takes
`&AuthorizedRoute`, so **an ungated dispatch does not compile**. Minted once per
job at the top of `run_extraction` / `run_chat` / `run_projection_patch`; the
repair attempt reuses the same token.

## 3. Per-endpoint capability

Three placements, exactly:

1. `AuthorizedRoute::completion_budget` — clamped at mint time from the route's
   `max_completion_tokens`. Structural: shipped defaults are 2048 / 512, both
   under 40,960, so **the clamp does not fire in the shipped configuration**. The
   RED test drives it with an over-budget config.
2. `max_context_tokens` is **recorded, not enforced** — nothing in this repo
   measures prompt tokens and no count is fabricated.
3. `constrained_decoding` picks the structured-output request form and is
   recorded in the route record. Never used to reject.

A test asserts **no route declares `262_144`**.

## 4. Terminal status and retry classification

`TerminalStatus { Completed, Truncated, Refused, Failed, TransportLost }` and
`terminal_status_from_finish_reason(Option<&str>)`, trimming + ASCII-lowercasing
like `bedrock::map_stop_reason`:

| input | → |
|---|---|
| `None` / `""` / `stop` / `end_turn` / `stop_sequence` | `Completed` |
| `length` / `max_tokens` / `model_length` | `Truncated` |
| `content_filter` / `guardrail_intervened` / `refusal` | `Refused` |
| `tool_calls` / `function_call` / any other non-empty token | `Failed` |

`RetryClass { PermanentRejection, TransientAvailability, UnusableCompletion,
ExternalEffectUnknown }`. `ExternalEffectUnknown` — **not** `OutcomeUncertain`,
which `audio-graph-5e41` owns for a canonical-write state — is never auto-retried.
There is deliberately no never-dispatched class (ADR-0038:235-240).

`Truncated` short-circuits **before** the JSON parse: the attempt fails with a
content-free error naming the terminal status, does **not** enter the repair path,
and does **not** raise `max_tokens`.

## 5. Served-route provenance

`WireOutcome { terminal_status, served_model, served_upstream_provider,
constrained_decoding }` produced by both blocking wire layers.

- `api_client.rs`: `ChatCompletionResponse` gains `model`, `Choice` gains
  `finish_reason`; `chat_completion_inner` returns `(String, u32, WireOutcome)`.
- `openrouter.rs`: `Choice` gains `finish_reason`; the two lossy
  `.map(|(text, telemetry)| …)` calls stop discarding `selected_provider` /
  `served_model` — those two `.map`s **are** ADR-0038 defect 3.
- `sanitize_metadata_value` + `looks_credential_shaped` move to `route.rs` as
  `sanitize_route_metadata`; `openrouter.rs` keeps a one-line delegate so the
  existing redaction tests pass unchanged.
- `ProjectionProvenance` gains `route_id: Option<String>` and
  `model_source: ModelIdentitySource` (defaults to `Requested` so pre-contract
  records are read honestly). Richer content-free evidence goes on
  `ProjectionPatch` as one optional `route: Option<RouteRecord>` — patch-level,
  not multiplied per materialized item.
- `ProjectionBackendOutput.provider` stops being `"api"` / `"openrouter"` /
  `"local_llama"` / `"mistralrs"` and becomes the route's registry
  `provider_id`. `actual_backend_identity` becomes a pass-through for `llm.*`
  ids and keeps its legacy arms for records written by older builds.

Defect (b): `api_client::ResponseFormat` gains `json_schema`, and the projection
API path becomes **route-driven, not host-substring-driven** —
`GuaranteedConstrained` sends `{type:"json_schema", json_schema:{name, strict:true,
schema}}`; vLLM keeps `structured_outputs`; otherwise `json_object`. A 4xx on the
strict request downgrades to `json_object` **on the same route** and records
`Unconstrained` — a mode downgrade, explicitly not a provider substitution.

## 6. Fallback removal — what happens at each former site

Deleted outright: the `!allow_cloud_fallbacks` local-only chains in
`run_extraction`; all four 4-deep `.or_else` chains; `ChatAttemptFn`;
`ProjectionAttemptFn`; `run_attempts`; `run_projection_attempts`;
**`run_projection_repair_escalation`**; the `allow_cloud_fallbacks` field on all
three `LlmJob` variants; the three `_with_policy` methods.

`llm_allow_cloud_fallbacks` survives **only** as a privacy-report input
(`SpeechConfig`, `ExtractionDeps`, `TranscriptProcessingContext`,
`ProjectionDispatchContext`, `ProjectionMovementFacts.cloud_transfer_allowed`) —
it is persisted in session data-movement events with tests, so deleting it would
be an ADR-0027 migration for no safety gain. It no longer authorizes anything.
`ProjectionLedgerBackend::FailedChain` becomes `FailedRoute` and loses its
`|| dispatch.llm_allow_cloud_fallbacks` widening.

Deleting the privacy-boolean's `false` arm is safe and is an improvement: both
production client constructors apply `provider_content_egress_policy_from_settings`
and both `new()` default to `block("explicit_policy_required")`, so a cloud route
under a non-`ByokCloud` mode becomes an explicit, content-free privacy refusal
instead of a silent downgrade to a local model.

## 7. Critique amendments (mandatory)

1. **Live-config consistency check at every dispatch.** The job's `LlmProvider` is
   a snapshot taken at session start, while egress goes through the shared client
   handle that `sync_*_from_settings_cache` rebuilds on every settings save. So
   `route.rs` re-derives the route from the **dispatched client's own config** and
   `AuthorizedRoute::ensure_serves(live)` fails closed, content-free, when the
   live route id differs from the authorized one. Provenance is stamped from the
   live route, never from the snapshot.
2. **Singleton pin for `route.cerebras_via_openrouter`.** `strict_accelerator`
   builds `order` **and** `only` from a provider *list*, and
   `preferred_provider()` returns only the first entry, so
   `order=["cerebras","groq"], allow_fallbacks=false` would satisfy a
   first-entry discriminator while Groq legitimately serves. The discriminator
   therefore requires a genuine singleton pin: the effective pin list is exactly
   one entry that normalizes to `cerebras`, `only` is empty or the same
   singleton, and `allow_fallbacks == Some(false)`. Anything else is
   `route.openrouter`.
3. **`Option<u32>` capability fields**, unknown ⇒ clamp is a no-op. Only the
   Cerebras row carries numbers, cited to ADR-0038:54-56.
4. **The retry split is stated explicitly.** `is_retryable_chat_transport_error`
   narrows to `is_connect()` only — a pre-status `.send()` timeout may be
   post-send, which is `ExternalEffectUnknown` and is never auto-retried.
   `is_retryable_chat_decode_error` keeps its `is_timeout()` arm because it only
   runs **after** a 2xx status line, where the remote effect is known to have
   happened: that is `UnusableCompletion`, which the ADR does not forbid
   retrying, and deleting it would regress seed `audio-graph-a324`. Both mock
   behaviours are pinned by name: hang **before** the status line ⇒ one request;
   2xx headers then truncate ⇒ two requests.

## 8. Out of scope, stated plainly

`crates/provider-registry` (no new descriptor, no `ui_selectable` change, so
`src/generated/providerRegistry.ts` is untouched); `crates/ipc-contract` (no
persisted-shape change, so no ADR-0027 migration); the streaming path's behaviour
(`finish_reason` is already deserialized there and nobody acts on `"length"` — a
real defect, but not one of the four, and acting on it changes the frontend
terminal-frame contract); the runtime validator's strictness; retry
*progression* and the stalled lane at `projection_scheduler.rs`
(`audio-graph-3b48`); `Finalization Blocked` as a runtime state
(`audio-graph-70c8`); the `Accepted` commit boundary (`audio-graph-90f3` /
`audio-graph-8e73`); re-authorizing a pinned route that loses `ui_selectable`
(explicitly unowned).
