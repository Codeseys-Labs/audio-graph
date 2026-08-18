---
status: accepted
date: 2026-08-18
deciders: [maintainer]
consulted: [wayfinder 8873 frontier decision packet, docs/agentic-runs/2026-08-17-wayfinder-8873-frontier/]
---

# ADR-0038: Route LLM Operations Through a Single-Skin Named Route Table

> **Provenance.** The maintainer decided this on 2026-08-18 during the wayfinder
> grilling of ticket `audio-graph-21e9`, choosing among agent-prepared options in
> §3 of
> [`decision-packet.md`](../agentic-runs/2026-08-17-wayfinder-8873-frontier/decision-packet.md).
> The reasoning below restates the decision packet's case; the maintainer
> reviewed the distilled trade-offs and caveats, not the full packet. See
> **More Information** for how to reverse it.

## Context and Problem Statement

Three LLM operations — the two incremental projection lanes and post-stop final
refinement — share one route today, and that route has no contract. What exists
instead:

- Exactly **one** global LLM provider setting, with no per-operation route
  concept (`src-tauri/src/settings/mod.rs:1225`).
- A hardcoded per-provider cross-provider fallback chain
  (`src-tauri/src/llm/executor.rs:664-699`) authorized solely by a privacy
  boolean (`src-tauri/src/commands.rs:2026-2028`). For an OpenRouter user the
  chain is `[projection_openrouter, projection_api, projection_native,
  projection_mistralrs]`.
- No [ADR-0033](0033-enforce-mvp-provider-enablement-at-content-start.md) start
  gate anywhere in `executor.rs` — `ensure_llm_provider_start_enabled` appears
  only at `provider_registry.rs:82,287` and `commands.rs:638,645,3029`.
- Provenance that names the **requested** model, not the served one:
  `chat_completion_with_schema_cached` ends
  `.map(|(text, telemetry)| (text, telemetry.usage.total_tokens...))`
  (`openrouter.rs:1607-1617`), discarding `selected_provider` and
  `served_model`, and the patch takes `client.config().model.clone()`
  (`executor.rs:967`). `ProjectionProvenance` is `{provider, model, prompt_id}`
  (`projections.rs:1246-1250`).
- No `finish_reason` deserialization on either blocking client
  (`openrouter.rs:756-764`; `api_client.rs:148-155`), while OpenRouter converts
  some length errors into HTTP 200 with `finish_reason: "length"`.
- A direct-Cerebras client that **cannot express** `json_schema` at all:
  `ResponseFormat` carries only `format_type` (`api_client.rs:128-131`) and
  `prefers_vllm_structured_outputs()` matches only localhost/vllm hosts
  (`api_client.rs:423-429`).

The research that preceded this ticket (`audio-graph-3d0c`) selected **no**
product contract, so every rule here is this decision's to make. That research
did establish two things the contract must respect: Chat Completions is the only
generation surface first-party-documented for direct Cerebras, and
Messages/Responses parity for the selected Gemma endpoint was **deliberately
left unproven**. It also recorded that capability facts are per-endpoint — the
selected Cerebras endpoint is 131,072 context / 40,960 max completion, against
an aggregate model record of 262,144.

The combination is worse than the sum. Because no path reads `finish_reason`, a
truncated response fails as invalid JSON and escalates the repair prompt to the
**next provider in the chain** (`executor.rs:774-780`, whose own doc comment
says so) — a silent cross-provider egress event caused purely by truncation.
The ticket exists to make that impossible, and to give the three operations one
named, authorized, provenance-stamped route contract.

## Decision Drivers

- **Never silent.** No content-bearing LLM call may reach a provider the
  maintainer did not authorize for that route, and no substitution may happen
  without being recorded.
- Ship exactly as much provider neutrality as the evidence supports — the
  research explicitly refused to claim Messages/Responses parity, and
  [ADR-0032](0032-layer-validation-evidence-by-claim.md)'s "a command may claim
  only what it asserts" forbids encoding unvalidated semantics in
  release-blocking fixtures.
- The runtime validator must remain the sole admission authority
  (`projection_llm.rs:644`); a route contract may grade capability but must not
  become a second admission gate.
- Route identity must be legible to
  [ADR-0030](0030-organize-mvp-shell-around-ready-livenow-review-inspect.md)'s
  planned-vs-observed route label and to
  [ADR-0034](0034-require-exhaustive-evidence-for-negative-data-egress-claims.md)'s
  producer inventory.
- A route-exhausted lane needs a per-Session resting place, which
  [ADR-0035](0035-record-post-stop-finalization-failure-as-per-session-finalization-blocked.md)
  now provides.
- Externally uncertain remote work must never be auto-reissued without
  idempotency proof plus cost/egress authorization (`audio-graph-5e41`, closed);
  neither route documents an idempotency key.

## Considered Options

- **A. Single-skin named route table** — Chat Completions is the only
  MVP-admitted wire skin; Messages and Responses exist as reserved enum variants
  behind a gated probe. Named routes, per-endpoint capability checks, a
  normalized terminal status, a four-class retry classification, and
  served-route provenance.
- **B. Three-skin adapter abstraction now** — build Chat Completions, Messages,
  and Responses adapters behind one interface, as the ticket's literal wording
  asks.
- **C. Capability-negotiated dynamic selection** — discover capabilities at
  dispatch time and pick the route that satisfies the request.
- **D. One pinned route, no fallback machinery at all** — a single route per
  operation, no list, no substitution code.

## Decision Outcome

Chosen option: **"A. Single-skin named route table"**, because it ships only the
neutrality the evidence supports, fits the shape the codebase already has (a
route table plus provenance plumbing, not a new transport layer), makes "never
silent" mechanical rather than procedural — an empty fallback list is *no code
path*, not a rule someone must remember — and is the only option that forces the
four verified defects to be fixed as part of the contract rather than deferred.
**Confidence is high on the core**: every element below rests on a line that was
read, not inferred.

The core contract:

- **One named route table.** Routes are named entities, not provider strings
  assembled at the call site.
- **Chat Completions is the only MVP-admitted wire skin.** `Messages` and
  `Responses` are reserved enum variants, gated, and unreachable until a probe
  proves parity for the selected endpoint.
- **Capability checks are per-ENDPOINT.** The selected Cerebras endpoint's
  131,072 context / 40,960 max completion — never the aggregate 262,144 model
  record.
- **One normalized terminal status**: `Completed` / `Truncated` / `Refused` /
  `Failed` / `TransportLost`.
- **A four-class retry classification whose uncertain class is never
  auto-retried.** That class is named **`ExternalEffectUnknown`**, renamed from
  the packet's `OutcomeUncertain`: `audio-graph-5e41` (closed and accepted)
  already assigned `OutcomeUncertain` to a **canonical-write** state ("restart
  from canonical `Pending` becomes `OutcomeUncertain`"), and its remote-attempt
  vocabulary is `DurableQueued` / `RemoteInFlight` / `ExternalEffectUnknown`.
  The rename is mechanical, but the collision would otherwise land inside the
  one taxonomy that reads both vocabularies.
- **SERVED-route provenance is stamped by trusted code**, never model-echoed and
  never config-echoed, consistent with ADR-0024 §3 and `projections.rs:1246`.

### Accepted sub-question defaults

These are part of this decision, not separate ones:

1. **Per-operation override surface: a global route plus a distinct
   final-refinement route.** The two incremental lanes want the same low-latency
   small-patch route; refinement has a materially different context and output
   budget. This is a configuration-surface product call, cheap to widen later.
2. **Ship with empty fallback lists; every future entry individually
   authorized**, with its cost and egress consequence shown at authorization
   time. ADR-0035 is what makes empty lists survivable — without the per-Session
   `Finalization Blocked` resting place, one organization-scoped 429 would take
   the whole app modal and this default would invert under pressure. Because
   ADR-0035 is accepted, the default holds.
3. **"Cerebras through OpenRouter" is a distinct authorized egress route from
   "Cerebras direct"**, for both authorization and provenance. It is a distinct
   processing path (AudioGraph → OpenRouter → upstream), and OpenRouter defaults
   `allow_fallbacks` to true, so requesting structured outputs does not even pin
   Cerebras.
4. **On terminal status `Truncated`, do not auto-spend a larger-budget attempt.**
   Go to `Finalization Blocked` (ADR-0035).
5. **The capability check records and grades constrained decoding — guaranteed-
   constrained vs. advertised-hint — but never gates dispatch on it.** Applied
   literally, a hard gate rejects **both** designated proof routes: direct
   Cerebras cannot express the request (`api_client.rs:128-131`) and OpenRouter's
   own docs warn enforcement varies by endpoint even with `strict: true`. The
   validator stays the sole admission authority.

### The four verified defects this contract fixes inside itself

The contract is not complete until all four are closed:

1. **No `finish_reason` deserialized on either blocking path**
   (`openrouter.rs:756-764`; `api_client.rs:148-155`), so a truncated response
   fails as invalid JSON and escalates to the next provider — a silent
   cross-provider hop caused purely by truncation.
2. **Direct Cerebras never requests its one documented strict-decoding
   guarantee**, because `ResponseFormat` has no `json_schema` field to populate
   (`api_client.rs:128-131`) and `prefers_vllm_structured_outputs()` is false
   for `api.cerebras.ai` (`api_client.rs:423-429`).
3. **Provenance records the requested, not served, model**
   (`openrouter.rs:1607-1617`, `executor.rs:967`). `fallback_evidence`
   (`openrouter.rs:1090`) does distinguish preferred from served, but it feeds
   only the global telemetry aggregate (`record_global`, `:1024`), never the
   patch.
4. **No ADR-0033 gate at the fallback dispatch site**
   (`executor.rs:664-699` and the repair escalation at `:774-780`).

### Consequences

- **Positive**: "never silent" becomes structural. With empty lists there is no
  cross-provider code path to forget to gate.
- **Positive**: the three operations get one named route identity that
  ADR-0030's planned-vs-observed label and ADR-0034's producer inventory can
  both cite.
- **Positive**: the four verified defects above are fixed as contract work, not
  filed as follow-ups.
- **Positive**: low-regret by construction — option A with empty fallback lists
  *is* option D, whereas option D cannot reach A without a new decision.
- **Positive**: served-route provenance makes every downstream per-item
  provenance claim honest instead of inheriting a requested-model label.
- **Negative — this option with empty lists carries option D's cost.** An
  organization-scoped 429 blocks a lane with no recovery until an entry is
  individually authorized. Final refinement is exactly where one authorized
  alternate would be cheapest, and this decision declines to pre-authorize it.
  The honest statement is that the accepted option *is* the pinned-route option
  plus an authorized path to add entries — the difference is the path, not
  today's behaviour.
- **Negative**: it refactors a tested path. `executor.rs`'s fallback and repair
  escalation carry existing coverage that must be replaced, not merely deleted.
- **Negative**: it introduces a **third** way to name a provider, which must be
  reconciled with the settings provider variant and the provider-registry id.
  Three naming schemes for one concept is a standing source of drift.
- **Negative**: it reads as under-delivering against the ticket's literal
  wording ("provider-neutral route contract"), and anyone comparing the ticket
  text to the shipped surface will see two of three skins unbuilt.
- **Negative**: persisted route/authorization shapes need
  [ADR-0027](0027-file-canonical-durable-session-store.md) migration treatment,
  and the Cerebras-via-OpenRouter split makes the authorization record shape
  load-bearing from the first write.
- **Negative — the `Truncated` default costs a lane.** Refusing the larger-budget
  retry means a truncation ends in `Finalization Blocked` rather than
  self-healing. The compensating fact is real but narrow: a larger declared
  `max_completion_tokens` also raises the Cerebras **pre-generation**
  rate-limit charge (the bucket is charged from input plus the declared
  completion budget *before* generation), so the "safe" retry makes a 429 more
  likely.
- **Negative — the capability check grades something it cannot enforce.**
  Recording "advertised hint" in provenance does not make output conform; every
  route remains dependent on the validator, and the graded field risks being
  read later as a guarantee it never was.
- **Negative — the per-endpoint pin is also a downstream evidence budget.**
  Pinning 40,960 completion tokens caps how many evidence-rich items one
  refinement partition may emit, so this capability pin silently sets the
  items-per-partition ceiling for the per-item evidence work (ADR-0037) and
  collides with `audio-graph-fbca`'s "safe-fit calculation".
- **Negative — the retry classification has no never-dispatched class, and must
  not grow one.** A route layer cannot know whether the socket closed before or
  after the provider began work; that fact lives in `audio-graph-5e41`'s durable
  scheduler record (`DurableQueued`), and provably-Absent is 5e41's
  `AbsentRetryAuthorized`. Consumers asking the route layer for
  "definitely-not-dispatched" must be redirected to their own durable record.
- **Negative — retry progression was deferred to `audio-graph-3b48`, and this
  decision specifies a classification anyway.** The overlap is deliberate and
  bounded (classification only, not progression), but it is an encroachment that
  `3b48` will have to accept or renegotiate. Relatedly,
  `projection_scheduler.rs:355-357` returns `Idle` when
  `last_failed_basis == basis`, so a failed lane on an unchanged basis stalls
  forever today — genuinely unowned, and not fixed here.
- **Negative — the contract is correct-but-unimplementable in one respect until
  other work lands.** It states `Accepted`-gated rules, but
  `ProjectionEventWriter::append` is a non-blocking `try_send`
  (`persistence/mod.rs:2313-2320`) and the `BufWriter` flushes only at shutdown
  (`:2435-2465`). `Accepted` does not exist in the runtime until
  `audio-graph-90f3` / `audio-graph-8e73` land.
- **Negative — one route-lifetime question is left open.** A route pinned in a
  durable attempt record that later becomes non-`ui_selectable` (after rotation
  or an app update) yields a Blocked Session whose only retry path is rejected
  forever: ADR-0033:48-52 requires every content-bearing start to resolve its
  *actual* descriptor, and `:58-65` exempts only stop/cancel/drain/cleanup of an
  *active* session — not a new refinement start. Re-routing would itself need
  authorization under this ADR's own "never silent" rule. Not decided here.
- **Neutral**: final refinement still has multiple claimed owners. This decision
  claims per-route completion budgets only; `audio-graph-fbca` claims output
  budgets and coverage accounting, and ADR-0036 claims attempt budgets. "Output
  budgets" cannot be co-owned, and the seam is not cut here.

## Pros and Cons of the Options

### A. Single-skin named route table

- Good, because it ships exactly as much neutrality as the evidence supports —
  the research refused to claim Messages/Responses parity, and this option does
  not claim it either.
- Good, because it fits the built shape: a route table plus provenance plumbing,
  not a new transport layer.
- Good, because it makes "never silent" mechanical — an empty fallback list is
  the absence of a code path.
- Good, because it forces the four verified defects to be fixed inside the
  contract.
- Good, because it is reachable-from and reducible-to option D, which makes it
  the low-regret choice.
- Bad, because it refactors a path that has tests today.
- Bad, because it adds a third provider-naming scheme to reconcile with settings
  and the registry.
- Bad, because it reads as under-delivering against the ticket's literal wording.
- Bad, because persisted route and authorization shapes need ADR-0027 migration.

### B. Three-skin adapter abstraction now

- Good, because it is literal fulfilment of the ticket as written, with no gap
  between text and surface.
- Good, because a terminal-status normalizer becomes first-class rather than a
  single-skin convenience.
- Good, because it front-loads the abstraction work if a second skin is ever
  admitted.
- Bad, because two of the three adapters would encode **unvalidated** semantics
  the research (`audio-graph-3d0c`) explicitly refused to claim, which then get
  asserted in ADR-0032 release-blocking fixtures — a command claiming more than
  it asserts.
- Bad, because it buys nothing for either designated proof route; both speak
  Chat Completions.
- Bad, because it is the largest surface for the least evidence.

### C. Capability-negotiated dynamic selection

- Good, because it absorbs provider catalog churn without a decision per change.
- Good, because it centralizes capability knowledge in one place instead of
  scattering per-route constants.
- Bad, because implicit selection **is** the silent fallback the ticket forbids,
  in a costume — the substitution just has a friendlier name.
- Bad, because it breaks the deterministic golden fixture and ADR-0030's
  "planned route" label, which presuppose a route knowable before dispatch.
- Bad, because advertised capability ≠ guaranteed capability, so the negotiation
  can lie — the same gap that makes a hard constrained-decoding gate unusable.

### D. One pinned route, no fallback machinery at all

- Good, because "never silent" becomes structural with no configuration to get
  wrong.
- Good, because it is closest to `audio-graph-5e41`'s accepted posture on
  uncertain remote work.
- Good, because it is the smallest surface of the four.
- Bad, because it answers a different question than the ticket asked — it
  removes the route contract rather than defining one.
- Bad, because one organization-scoped 429 blocks a lane with no recovery at all,
  and final refinement is exactly where one authorized alternate is cheapest.
- Bad, because reaching option A later requires a new decision, whereas option A
  reaches D by configuration.
- Honest note: **the accepted option, configured with empty fallback lists, IS
  this option** — plus an individually authorized path to add entries. The
  practical difference today is zero; the difference is what it costs to change.

## More Information

- **Relationship to existing ADRs.**
  - [ADR-0033](0033-enforce-mvp-provider-enablement-at-content-start.md) is
    extended in reach, not amended: its start gate must now apply at the LLM
    fallback and repair-escalation dispatch sites, where it is absent today.
  - [ADR-0035](0035-record-post-stop-finalization-failure-as-per-session-finalization-blocked.md)
    is the precondition for the empty-fallback default; it is the per-Session
    resting place a route-exhausted lane fails closed into.
  - [ADR-0034](0034-require-exhaustive-evidence-for-negative-data-egress-claims.md)
    is read here as a **producer inventory for egress** (its actual text at
    `:44-50`), not a transcript-coverage predicate. Sub-decision 3 above decides
    that the inventory lists Cerebras-direct and Cerebras-via-OpenRouter as
    **two** LLM producers.
  - [ADR-0030](0030-organize-mvp-shell-around-ready-livenow-review-inspect.md)'s
    planned-vs-observed route rule is what served-route provenance feeds.
  - [ADR-0032](0032-layer-validation-evidence-by-claim.md)'s "a command may claim
    only what it asserts" is the argument that rejects option B.
  - [ADR-0027](0027-file-canonical-durable-session-store.md) governs migration of
    the persisted route and authorization records.
  - [ADR-0005](0005-openrouter-as-recommended-llm-endpoint.md) recommends
    OpenRouter as the cloud LLM endpoint; this record does not change that
    recommendation, but it stops treating "via OpenRouter" and "direct" as the
    same authorized egress.
  - [ADR-0019](0019-credential-and-config-storage.md) (proposed) will hold
    wherever per-route authorization records are stored.
- **What downstream tickets own.** `audio-graph-21e9` owns the route table, the
  removal of automatic cross-provider fallback, the four defect fixes, and the
  served-route provenance plumbing. `audio-graph-3b48` owns retry *progression*
  (this record owns classification only) and the stalled-lane behaviour at
  `projection_scheduler.rs:355-357`. `audio-graph-fbca` owns refinement
  partitioning and safe-fit; the "output budgets" overlap with this record's
  per-route completion budget needs a seam cut. `audio-graph-90f3` /
  `audio-graph-8e73` own the `Accepted` commit boundary this contract's rules
  presume. Whether a pinned route that loses `ui_selectable` may be re-routed is
  unowned and needs its own authorization.
- **Sequencing constraint — implementation order must not follow decision
  order.** This ticket's automatic-fallback removal must land **before** the
  stricter per-item validator of `audio-graph-a668`
  ([ADR-0037](0037-admit-session-memory-items-through-a-layered-claim-class-evidence-table.md)).
  Every validator rejection today escalates the repair prompt to the *next
  provider in the chain* (`src-tauri/src/llm/executor.rs:774-780`), authorized
  only by a privacy boolean (`src-tauri/src/commands.rs:2026-2028`) with no
  ADR-0033 gate anywhere in `executor.rs`. Tightening evidence first would turn
  this wave's hardening into an unauthorized-egress amplifier.
- **How to reverse, and what it costs.** The core is cheap to reverse: an
  empty-fallback single-skin route table is the minimal commitment in this space,
  and widening it — admitting a second wire skin, adding an authorized fallback
  entry, moving to independent per-operation routes — is **additive**, done by
  superseding this record and authorizing the new entries. The one expensive
  exception is sub-decision 3: treating Cerebras-via-OpenRouter as a route
  distinct from Cerebras-direct fixes a **persisted authorization record shape**
  and an ADR-0034 producer row, so collapsing the two later is moderately
  expensive — it requires migrating authorization records and re-versioning the
  producer inventory.
- **Not decided here.** Whether a durably pinned route may be re-routed after
  losing `ui_selectable`; the final-refinement ownership seam between this
  record, ADR-0036, and `audio-graph-fbca`; and retry progression, which stays
  with `audio-graph-3b48`.
