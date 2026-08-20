# Code reality behind audio-graph-862c and audio-graph-7da4

Investigation only. No code or seeds edited. All citations are against
`master` (current tip includes `3868e02 feat(audio-graph-3624): route every
LLM operation through the single-skin named route table`, which implements
ADR-0038 and is the "post-3624" baseline both seeds reference).

## Seeds

- **audio-graph-862c** — "Unify ledger provider_id convention across
  Configured, Actual, and FailedRoute events." Deferred review finding from
  3624 land time.
- **audio-graph-7da4** — "Decide FailedRoute privacy classification when the
  live client repoints mid-session." Also deferred from 3624 land time.

---

## 1. The three ledger event constructors in `src-tauri/src/speech/mod.rs`

`ProjectionLedgerBackend` enum, `src-tauri/src/speech/mod.rs:2021-2030`:

```rust
enum ProjectionLedgerBackend<'a> {
    Configured,
    Actual(&'a crate::projections::ProjectionProvenance),
    FailedRoute,
}
```

All three feed `projection_movement_facts` (`speech/mod.rs:2083-2130`), whose
match arm (`speech/mod.rs:2093-2116`) derives `provider_id` per variant:

- **`Configured`** (`speech/mod.rs:2099-2104`) — pre-call marker, written by
  `build_started_event` before every job:
  ```rust
  ProjectionLedgerBackend::Configured => (
      dispatch.llm_provider.runtime_provider_id().to_string(),
      dispatch.llm_provider.requires_cloud_content_transfer(),
      matches!(dispatch.llm_provider, LlmProvider::OpenRouter { .. }),
      String::new(),
  ),
  ```
  `provider_id` = **`LlmProvider::runtime_provider_id()`** — the settings
  variant's fixed id (see §2).

- **`Actual(provenance)`** (`speech/mod.rs:2094-2098`) — terminal-success
  event, routed through `actual_backend_identity` (`speech/mod.rs:2040-2079`):
  ```rust
  ProjectionLedgerBackend::Actual(provenance) => {
      let (provider_id, cloud, prefix) =
          actual_backend_identity(dispatch, &provenance.provider);
      (provider_id, cloud, prefix, provenance.model.clone())
  }
  ```
  Inside `actual_backend_identity`, the `llm.*` arms
  (`speech/mod.rs:2044-2049`) are a **pass-through of the route registry's
  provider id already recorded on the patch provenance** (`provenance.provider`,
  itself stamped from the `AuthorizedRoute`/route table at dispatch time — see
  §2). For the generic-endpoint arm (`"llm.api" | "llm.cerebras" |
  "llm.sambanova" | "api"`, `speech/mod.rs:2050-2071`) the `provider_id` is
  `backend.to_string()` when it already starts with `"llm."` (i.e. the sharp
  registry id `llm.cerebras`/`llm.sambanova`/`llm.api` from provenance), so
  `Actual` genuinely reports the served route's registry id (`llm.cerebras`
  for a Cerebras dispatch), NOT `LlmProvider::runtime_provider_id()`.

- **`FailedRoute`** (`speech/mod.rs:2110-2115`) — terminal-failure event, on
  the single authorized route (no fallback chain exists post-3868e02):
  ```rust
  ProjectionLedgerBackend::FailedRoute => (
      dispatch.llm_provider.runtime_provider_id().to_string(),
      dispatch.llm_provider.requires_cloud_content_transfer(),
      false,
      String::new(),
  ),
  ```
  `provider_id` = **`LlmProvider::runtime_provider_id()`** — same fixed id as
  `Configured`, NOT the route that was actually attempted and failed.

**Post-3624 asymmetry, precisely stated**: `Configured` and `FailedRoute` both
key off `LlmProvider::runtime_provider_id()` (the settings-variant tag);
`Actual` keys off the route-registry id carried in provenance. For any
`LlmProvider::Api { endpoint: <cerebras-or-sambanova-url> }` session, a
succeeded call ledgers `llm.cerebras`/`llm.sambanova` (via `Actual`) while the
pre-call marker and any failed call on that same session ledger `llm.api` (via
`Configured`/`FailedRoute`) — exactly the seed 862c description: "A failed
Cerebras attempt is inventoried under a producer that was never dialled."

---

## 2. `LlmProvider::runtime_provider_id()` vs. the route registry's ids

`src-tauri/src/settings/mod.rs:609-617`:

```rust
pub fn runtime_provider_id(&self) -> &'static str {
    match self {
        LlmProvider::LocalLlama => "llm.local_llama",
        LlmProvider::Api { .. } => "llm.api",
        LlmProvider::OpenRouter { .. } => "llm.openrouter",
        LlmProvider::AwsBedrock { .. } => "llm.aws_bedrock",
        LlmProvider::MistralRs { .. } => "llm.mistralrs",
    }
}
```

Every `LlmProvider::Api { endpoint, .. }` value — regardless of what
`endpoint` actually is (`localhost:11434`, `api.openai.com`,
`api.cerebras.ai`, `api.sambanova.ai`, a self-hosted vLLM box) — collapses to
the single literal `"llm.api"`. The match is on the **settings enum variant**,
not on the endpoint string; `endpoint` isn't even read here.

Compare `requires_cloud_content_transfer()` immediately below
(`settings/mod.rs:619-625`), which *is* endpoint-sensitive
(`!endpoint_is_loopback(endpoint)`, loopback check at `settings/mod.rs:407-417`)
— so the settings layer already has two different granularities living side
by side: a coarse fixed id and a fine-grained boolean, and only the boolean
was ever endpoint-aware.

The route registry (`src-tauri/src/llm/route.rs`), added by 3868e02
(ADR-0038), resolves finer ids from the *live endpoint*:

- `RouteDescriptor` table `LLM_ROUTES` (`route.rs:188-281`):
  - `route.cerebras_direct` → `provider_id: "llm.cerebras"` (`route.rs:208-223`)
  - `route.sambanova_direct` → `provider_id: "llm.sambanova"` (`route.rs:224-231`)
  - `route.openai_compatible` → `provider_id: "llm.api"` (`route.rs:232-241`)
  - `route.openrouter` → `provider_id: "llm.openrouter"` (`route.rs:242-252`)
  - `route.cerebras_via_openrouter` → `provider_id: "llm.openrouter"` (shares
    the OpenRouter descriptor by design — comment at `route.rs:155-161`)
    (`route.rs:253-267`)
  - `route.aws_bedrock` → `provider_id: "llm.aws_bedrock"` (`route.rs:268-280`)
- `route_for_api_endpoint` (`route.rs:308-316`) is the resolver that turns an
  `Api` variant's endpoint into one of the three `llm.api`/`llm.cerebras`/
  `llm.sambanova` rows, via exact base-URL match:
  ```rust
  pub fn route_for_api_endpoint(endpoint: &str) -> &'static RouteDescriptor {
      if crate::settings::is_cerebras_endpoint(endpoint) {
          route_by_id("route.cerebras_direct")
      } else if crate::settings::is_sambanova_endpoint(endpoint) {
          route_by_id("route.sambanova_direct")
      } else {
          route_by_id("route.openai_compatible")
      }
  }
  ```
  `is_cerebras_endpoint`/`is_sambanova_endpoint`
  (`src-tauri/crates/ipc-contract/src/endpoint_credential_routing.rs:121-128`)
  are exact, normalized base-URL equality checks against
  `CEREBRAS_BASE_URL`/`SAMBANOVA_BASE_URL` — not substring/domain matches, and
  not loopback-aware.
- `resolve_route` (`route.rs:291-303`) is the single entry point from a
  settings variant to a route, calling `route_for_api_endpoint` for `Api`.
- `AuthorizedRoute::provider_id()` (`route.rs:403-405`) exposes the resolved
  route's registry id to dispatch code, and provenance is stamped from this
  (not from `LlmProvider::runtime_provider_id()`) — which is why `Actual`
  events in §1 carry the sharp id.

So: `LlmProvider::runtime_provider_id()` is a **3-way→1 collapse** (every
`Api` variant is `llm.api`) that predates ADR-0038; the route registry is the
**post-3624 source of truth** for which of `llm.api` / `llm.cerebras` /
`llm.sambanova` a given `Api` endpoint actually resolves to. `Configured` and
`FailedRoute` (§1) still call the pre-3624 coarse function; only `Actual`
(via provenance, itself sourced from the route table at dispatch time) uses
the fine-grained registry id.

---

## 3. ADR-0034 producer-inventory semantics and the cost of a wrong id

`docs/adr/0034-require-exhaustive-evidence-for-negative-data-egress-claims.md`
decision (lines 37-56): the UI may claim positive egress from partial
evidence at any time, but may claim "nothing left the device" only once a
named, versioned, **exhaustive-producer-coverage marker** exists that covers
every content-bearing producer enabled in the build, alongside session/schema
scope and closed capture lifecycle, with zero egress rows in that scope.

**Current state of that marker**: it does not exist yet.
`src/components/sessionDataRoute.ts:180`:
```ts
const EXHAUSTIVE_RUNTIME_MOVEMENT_COVERAGE_VERSION: number | null = null;
```
`hasCompleteMovementEvidence` (`sessionDataRoute.ts:188-195`) is gated on this
constant, so today every negative-egress claim renders Unknown regardless of
ledger content (`sessionDataRoute.ts:344-346`,
`movementEvidenceComplete`/`contentLeftDevice`). **This means: today, a wrong
`provider_id` cannot flip a false "no content left the device" claim (ADR-0034's
core guarantee), because that claim path is not yet armed.** The cost of a
wrong id today is entirely on the *positive*-evidence side.

**Consumers of `destination.provider_id`**:

- `load_session_data_movement_cmd` (`src-tauri/src/commands.rs:6980-6991`) is
  a pure loader/pass-through — it validates the session id, reads the JSONL
  ledger, and returns the stored `DataMovementEvent`s verbatim
  (`.map(crate::persistence::canonical_reader::StrictCanonicalRead::into_payloads)`).
  It does not interpret or transform `provider_id`; whatever string a
  constructor in §1 wrote is what this command returns.
- The actual interpreter is the frontend privacy view,
  `src/components/sessionDataRoute.ts`, `buildSessionDataRouteReport`:
  - **Provider transfers** (`sessionDataRoute.ts:314-338`): `providerId =
    event.destination.provider_id ?? event.model?.provider_id ?? null`, then
    de-duplicated by a `transferKey` that **includes** `providerId`
    (`sessionDataRoute.ts:318-323`). A Cerebras call ledgered as `llm.api`
    collapses into whatever bucket other `llm.api` traffic in the session
    already occupies (or opens a separate wrong-labeled bucket), instead of
    its own Cerebras row.
  - **Redacted errors** (`sessionDataRoute.ts:259-285`): grouped by
    `` `${providerId}|${errorCode}` `` — a Cerebras-route failure ledgered as
    `llm.api` groups under the wrong provider bucket, and if the session also
    has genuine `llm.api` (loopback) failures, the two get merged into one
    error-count row.
  - **Credentials** (`sessionDataRoute.ts:287-306`): keyed by `providerId ??
    source.kind ?? event_id`; same collapsing risk.
- `src/components/SessionDataRoutePanel.tsx` renders these fields **verbatim,
  as the raw id string**, with no lookup through the provider registry's
  human display name:
  - `ProviderTransferRow` (`SessionDataRoutePanel.tsx:91-119`):
    `{transfer.providerId ?? t(...)}` at line 100 — literally prints
    `"llm.api"` or `"llm.cerebras"` as the primary label.
  - `RedactedErrorRow` (`SessionDataRoutePanel.tsx:121-156`): same pattern at
    line 131.
  - `CredentialRow` (`SessionDataRoutePanel.tsx:158-187`): same pattern at
    line 174.
  - Registry display names (e.g. `llm.cerebras` → `"Cerebras"`,
    `data_boundary: "vendor_cloud"` at `src/generated/providerRegistry.ts`
    around lines 2570-2611, vs. `llm.api` → `"OpenAI-compatible LLM"`,
    `data_boundary: "user_configured_endpoint"` around lines 2504-2534) are
    never consulted by this panel — so even if they were, a wrong ledger id
    would still point at the wrong registry entry, and thus the wrong
    `data_boundary`/`retention_policy`/`data_residency` fields entirely.

**What a wrong id costs the user, concretely**: not that the report shows
"local" (boundary is a *separate* field, gated on `requires_cloud_transfer` —
see §4), but that the report **misattributes which vendor's cloud a shown
transfer/error/credential belongs to** — a Cerebras egress event is labeled
`llm.api`, a coarse, non-vendor-specific tag that reads as "some
OpenAI-compatible endpoint" rather than "Cerebras Cloud." This directly
undercuts two of ADR-0034's stated Consequences (lines 60-63): "Known egress
remains visible without waiting for full coverage" is only half-true — it's
visible, but visible under the wrong producer; and "Enabling a provider
cannot silently widen the proof boundary; its producers must join a
versioned coverage matrix first" is put at risk for any *future* coverage
matrix keyed by provider id, since `Configured`/`FailedRoute` rows for a
Cerebras-endpoint session would never actually match a `llm.cerebras`
producer-coverage entry (they're stamped `llm.api`), while `Actual` rows from
the same session would.

---

## 4. audio-graph-7da4: the dropped widening, and today's failure-path trace

### The pre-3624 widening and its removal

```
git log --oneline --all -S"llm_allow_cloud_fallbacks" -- src-tauri/src/speech/mod.rs
3868e02 feat(audio-graph-3624): route every LLM operation through the single-skin named route table
475a1dd feat(mvp): checkpoint durable session hardening
06e129d feat(projection): windowed basis + rolling summary + stable-prefix caching + data-flow ledgering (audio-graph-18ee, d77e, 72d5) (#77)
8a603e4 Backlog-zero wave: event-sourced backend + settings UX + gtk test fix + 3-OS CI (draft — macOS/audio unrun) (#22)
```

`git show 3868e02 -- src-tauri/src/speech/mod.rs` diff hunk (the removal),
against the pre-3624 `ProjectionLedgerBackend::FailedChain` arm (renamed to
`FailedRoute` in the same commit):

```diff
-        ProjectionLedgerBackend::FailedChain => (
+        ProjectionLedgerBackend::FailedRoute => (
             dispatch.llm_provider.runtime_provider_id().to_string(),
-            dispatch.llm_provider.requires_cloud_content_transfer()
-                || dispatch.llm_allow_cloud_fallbacks,
+            dispatch.llm_provider.requires_cloud_content_transfer(),
             false,
             String::new(),
         ),
```

The dropped widening `|| dispatch.llm_allow_cloud_fallbacks` existed for a
**different** mechanism than a mid-session repoint: it covered the pre-3624
executor's automatic cross-provider **fallback chain** (`ChatAttemptFn`/
`ProjectionAttemptFn`/the `_with_policy` trio — all removed by 3868e02), where
a session configured local-only could still have the executor silently
attempt a remote backend as a fallback. The widening said "if this session's
privacy mode *permits* cloud at all, a failed attempt might have reached
cloud even though the configured provider didn't require it." It was a
policy-flag widening, not an endpoint-aware one — it never inspected which
endpoint was actually dialled.

The current code's rationale for dropping it (`speech/mod.rs:2105-2109`):
> A failed attempt on the single authorized route: its cloud-ness is exactly
> the configured provider's. ... ADR-0038 removed that [fallback] chain, so
> widening here would now overstate flow.

This reasoning is correct **for the fallback-chain scenario** it addresses.
It does not address — and the seed's title makes clear it was never meant to
address — a live client repoint.

### Tracing the failure path's inputs to see if a repoint can ledger as local today

1. **The snapshot.** `LlmProvider` is captured once, at capture-start, from
   `AppSettings`:
   `src-tauri/src/commands.rs:2258`: `let llm_provider = settings.llm_provider.clone();`
   This local flows into `speech::SpeechConfig { llm_provider, ... }`
   (`commands.rs:2336-2341`) and into a `std::thread::Builder` spawn
   (`commands.rs:2309-2352`) that starts the long-lived `speech-processor`
   thread. `config.llm_provider` is then threaded into
   `shared_to_transcript_context(shared, config.llm_provider, ...)` at the top
   of every processor entry point, e.g. `speech/mod.rs:3968` (Moonshine) —
   called exactly once, before the processor's `loop { processed_rx.recv... }`
   (`speech/mod.rs:3977` onward) that runs for the rest of the session.
   `shared_to_transcript_context` (`speech/mod.rs:1782-1815`) stores it into
   `TranscriptProcessingContext.llm_provider` (`speech/mod.rs:1619`), a
   **`#[derive(Clone)]` struct that is itself never rebuilt** for the session's
   duration. `TranscriptProcessingContext::projection_dispatch_context()`
   (`speech/mod.rs:1759-1775`) clones `self.llm_provider` into a fresh
   `ProjectionDispatchContext.llm_provider` (`speech/mod.rs:1652`) for every
   individual projection job, but the *value* being cloned never changes —
   it's the same session-start snapshot every time.

2. **The live client diverges independently.** The route module documents
   this explicitly (`src-tauri/src/llm/route.rs:434-442`):
   > the job's `LlmProvider` is a **snapshot** taken at session start, while
   > egress goes through the shared client handle that
   > `sync_llm_api_client_from_settings_cache` /
   > `sync_openrouter_client_from_settings_cache` rebuild on every settings
   > save.
   Confirmed in code: `sync_llm_api_client_from_settings_cache`
   (`src-tauri/src/commands.rs:1133-1148+`) reads the **current**
   `state.app_settings`, recomputes `api_config_from_runtime_settings(&settings)`
   and `content_egress_policy` from `settings.llm_provider.requires_cloud_content_transfer()`
   (`commands.rs:1142`), and swaps `state.api_client` — the same
   `Arc<Mutex<Option<ApiClient>>>` that was cloned into `SpeechShared.api_client`
   and is what the processor thread's blocking dispatch code actually calls
   through. So a settings save mid-session changes what the live dispatch
   talks to, without touching `ctx.llm_provider`.

3. **The gate that would catch a *cross-registry-descriptor* repoint.**
   `AuthorizedRoute::refine_within_authorization` (`route.rs:461-473`) fails
   closed when the live route's `provider_id` differs from the originally
   authorized route's `provider_id` (doc at `route.rs:444-453`: "Refusal ...
   A re-pointed `Api` endpoint moves between `llm.api` / `llm.cerebras` /
   `llm.sambanova`, which is a different authorization, so it is rejected"). A
   repoint from a loopback `Api` endpoint straight to Cerebras's or
   SambaNova's exact base URL *would* trip this gate — the dispatch is refused
   before any network I/O, so ledgering that refusal as "local" is actually
   correct (nothing left the device).

4. **The gap: same-descriptor repoints are accepted, and are NOT rare.**
   `route.rs:455-460` states the residual directly: "an endpoint edit that
   stays inside one registry descriptor (`localhost:11434` → `api.openai.com`,
   both `llm.api`) is not an authorization change and is accepted." Given
   `route_for_api_endpoint` (`route.rs:308-316`) only special-cases the two
   exact Cerebras/SambaNova base URLs and buckets **every other** `Api`
   endpoint — loopback or not, e.g. Ollama on `localhost`, OpenAI, Together,
   Fireworks, Groq's OpenAI-compatible surface, any self-hosted
   OpenAI-compatible box — into the same `route.openai_compatible`/`llm.api`
   descriptor, a mid-session edit from a loopback `Api` endpoint to *any*
   non-Cerebras/non-SambaNova cloud `Api` endpoint is a **same-descriptor
   refinement**, not a refusal. The live dispatch proceeds to the new cloud
   endpoint.

5. **What the ledger records if that dispatch fails.** If the executor's call
   to the new cloud endpoint fails, `LlmExecutor::generate_projection_patch`
   (`src-tauri/src/llm/executor.rs:322-353`) returns `Err(String)` up through
   `ExecutorProjectionPatchGenerator::generate_projection_patch`
   (`speech/mod.rs:1740-1755`) to `run_projection_job`'s `Err(error)` arm
   (`speech/mod.rs:2292-2325`), which builds `movement_facts` via
   `ProjectionLedgerBackend::FailedRoute` (`speech/mod.rs:2300`). That arm
   (`speech/mod.rs:2110-2115`, quoted in §1) computes `requires_cloud_transfer`
   from `dispatch.llm_provider.requires_cloud_content_transfer()` — the
   **stale, session-start snapshot**, which is still the old loopback `Api`
   value and therefore evaluates `false` (loopback check,
   `settings/mod.rs:622` / `endpoint_is_loopback`, `settings/mod.rs:407-417`).
   `resolved_destination` (`src-tauri/src/projection_data_movement.rs:76-108`)
   then computes `remote = facts.requires_cloud_transfer && facts.cloud_transfer_allowed`
   = `false && anything` = `false`, and returns `DataMovementDestination::local()`
   (`projection_data_movement.rs:95-96`). The resulting `DataMovementEvent` has
   `destination.boundary == "local"`.
   On the frontend, `isEgressBoundary`/`isContentEgress`
   (`src/components/sessionDataRoute.ts:26-40`) classify this event as **not**
   egress — it lands in `localEvents`, not `egressEvents`, and never appears
   in `providerTransfers` at all (that branch is gated on `isContentEgress`,
   `sessionDataRoute.ts:309`).
   The error code itself is also uninformative either way:
   `run_projection_job`'s error arm hardcodes `Some("projection_generation_failed")`
   (`speech/mod.rs:2307`) regardless of whether the underlying failure was a
   genuine remote-network failure or a local `refine_within_authorization`
   refusal — so the ledger cannot distinguish "never left the device" from
   "left the device and failed remotely" by error code either; it relies
   entirely on the (here, stale) `requires_cloud_transfer` boolean.

**Answer to the seed's question**: yes — today, a mid-session repoint of a
`LlmProvider::Api` endpoint from a loopback address to *any* non-Cerebras/
non-SambaNova cloud endpoint (the common case — every generic OpenAI-compatible
cloud provider, not just the two hardcoded accelerators) stays inside the
`route.openai_compatible`/`llm.api` descriptor, passes
`refine_within_authorization` as a same-provider "refinement," and — if that
dispatch subsequently fails — is ledgered by `FailedRoute` as
`requires_cloud_transfer: false` / `destination.boundary: "local"`, because
that computation reads the session-start `LlmProvider` snapshot rather than
the live, just-repointed endpoint. The event is filed as fully local and does
not appear anywhere in the privacy report's egress views. (The narrower case
the seed names literally — repointing straight to Cerebras/SambaNova's exact
base URL — is actually the one case the ADR-0033 gate *does* catch, refusing
the dispatch before it reaches the wire; the real, uncaught gap is same-
descriptor repoints, which are the majority of `Api`-variant endpoint edits.)

Note this staleness is not confined to `FailedRoute`: the generic-endpoint arm
of `actual_backend_identity` (`speech/mod.rs:2057-2065`, used by the `Actual`
success path when the served backend string is the catch-all `"llm.api"`)
also falls back to `dispatch.llm_provider.requires_cloud_content_transfer()`
for its cloud boolean — the same stale snapshot — so a *successful* call
through a same-descriptor repoint carries the same risk of a wrong
`requires_cloud_transfer` value (in either direction), even though its
`provider_id` (from provenance) is correct.
