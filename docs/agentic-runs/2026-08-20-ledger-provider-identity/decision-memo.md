# Decision memo: ledger provider identity and failed-route classification

Date: 2026-08-20. Scope: seeds audio-graph-862c ("Unify ledger provider_id
convention across Configured, Actual, and FailedRoute events") and
audio-graph-7da4 ("Decide FailedRoute privacy classification when the live
client repoints mid-session"). Both are open, priority 2, deferred design
decisions from the 3624/ADR-0038 land (commit 3868e02). Evidence base:
`code-reality.md` in this directory (414 lines, all citations verified
against master). This memo recommends; it does not close either seed.

Shared context. The three ledger event constructors live in
`ProjectionLedgerBackend` (src-tauri/src/speech/mod.rs:2021-2030) and feed
`projection_movement_facts` (mod.rs:2093-2116). Configured and FailedRoute
stamp `provider_id` from `LlmProvider::runtime_provider_id()`
(settings/mod.rs:609-617), which collapses every `Api` endpoint to the
literal `llm.api`; Actual stamps from provenance, which carries the route
registry's sharp id (`llm.cerebras`, `llm.sambanova`, `llm.api`) via
`AuthorizedRoute::provider_id()` (route.rs:403-405). One mitigating fact
bounds today's blast radius: ADR-0034's exhaustive-producer-coverage marker
is unimplemented (`EXHAUSTIVE_RUNTIME_MOVEMENT_COVERAGE_VERSION = null`,
sessionDataRoute.ts:180), so negative "nothing left the device" claims
always render Unknown and a wrong id or boolean cannot yet flip a false-safe
claim. The cost today is confined to misattribution and hidden positive
evidence, but the marker is designed to arm, and both decisions should be
made for the armed world.

## Seed audio-graph-862c: unify the ledger provider_id convention

### Decision in one sentence

Decide whether Configured and FailedRoute events keep stamping
`provider_id` from the coarse settings-variant tag
(`runtime_provider_id()`, always `llm.api` for any `Api` endpoint) or move
to the endpoint-resolved route-registry id that Actual events already carry
through provenance.

### Option 1: stamp terminal events from the route registry

Sharpen FailedRoute (and Configured, if it is meant as a prediction rather
than a declaration) to the registry id, ideally captured from the same
`AuthorizedRoute` that stamps provenance so every arm shares one source of
truth. What it costs: plumbing resolved-route identity into the failure
path (the `Err` arm at speech/mod.rs:2292-2325 sees only the dispatch
snapshot today); a mixed-convention history, since existing ledgers hold
coarse rows that any future coverage matrix must version around; and, for
the pre-call Configured event, an assertion that can still diverge from the
wire under a mid-session repoint (seed 7da4's territory). Against ADR-0034:
this protects "known egress remains visible" in an attributable form, and
it protects "producers must join a versioned coverage matrix" because a
Cerebras-endpoint session's failed rows would actually match a
`llm.cerebras` coverage entry instead of silently missing it while the same
session's Actual rows match.

### Option 2: coarsen Actual to the settings-variant tag

Make all three arms report `runtime_provider_id()` so the convention is
uniform. What it costs: it destroys the only vendor-sharp attribution in
the ledger. A Cerebras egress renders as the raw string `llm.api` in
SessionDataRoutePanel (which does no display-name lookup), merges into the
wrong `providerTransfers` and `redactedErrors` buckets
(sessionDataRoute.ts:259-338), and points at the wrong provider-registry
entry, so `data_boundary`, retention, and residency all read wrong. Against
ADR-0034: it achieves trivial internal consistency but strains "known
egress remains visible" (visible, but under a producer that was never
dialled) and forecloses any future coverage matrix keyed by vendor-specific
provider ids.

### Recommendation

Option 1. A privacy auditor reading the ledger alone can check a sharp
registry id against the provider registry and against the session's other
events; a coarse id is uniform but unverifiable, because `llm.api` cannot
be traced to a vendor from the ledger. The mechanism already exists
(provenance stamping from `AuthorizedRoute`), so this extends a proven
path rather than inventing one.

### What only the maintainer can answer

What the Configured event semantically means: a declaration of the user's
configured intent (in which case the coarse settings tag is arguably
correct there, and only FailedRoute should move) or a prediction of the
route about to be dialled (in which case all three arms should speak
registry ids). And whether the convention change should ride a
movement-event schema or convention version bump so the future coverage
matrix can tell legacy coarse rows from sharp rows.

## Seed audio-graph-7da4: FailedRoute classification when the live client repoints mid-session

### Decision in one sentence

Decide what a FailedRoute event should claim about cloud-ness when the live
dispatch client has been rebuilt away from the session-start `LlmProvider`
snapshot, given that today a loopback-to-cloud repoint inside the `llm.api`
descriptor is accepted as a refinement (route.rs:455-460) and a subsequent
failure ledgers `requires_cloud_transfer: false`, destination local, and
disappears from every egress view (traced end to end in code-reality.md
section 4).

### Option 1: stamp the failure from the live attempted route

Capture the attempted route's identity (endpoint cloud-ness, and its
provider id, which dovetails with 862c) at dispatch time from the refined
`AuthorizedRoute` or the live client, and compute FailedRoute's
`requires_cloud_transfer` from that instead of the stale
`dispatch.llm_provider`; fix the same stale fallback in
`actual_backend_identity`'s generic `llm.api` arm (speech/mod.rs:2057-2065)
in the same stroke. What it costs: threading attempted-route identity
through the executor error path (the `Err(String)` return carries no route
identity today), and accepting that Configured and FailedRoute within one
job can now disagree, which is truthful but needs documenting. Against
ADR-0034: it protects the positive-evidence claim today (a failed remote
attempt appears in the egress views instead of hiding in `localEvents`),
and it protects the future negative claim, because a stale false boolean is
exactly the row that would let an armed "nothing left the device" render
falsely.

### Option 2: refuse same-descriptor loopback-to-cloud refinements

Extend `refine_within_authorization` (route.rs:461-473) to treat a loopback
to non-loopback endpoint change as an authorization change and fail closed,
so the snapshot and the wire cannot diverge on cloud-ness and the ledgered
local boundary stays accurate by construction. What it costs: mid-session
endpoint edits stop applying to in-flight sessions, so users must restart
capture to switch endpoints; and the ledger's accuracy becomes an
out-of-band invariant the auditor must trust rather than a property visible
in the rows, since a refusal and a genuine remote failure share the same
hardcoded `projection_generation_failed` error code (speech/mod.rs:2307).
Against ADR-0034: it protects the future negative claim by construction
(no divergence can exist), but a gate bug would produce silent false-local
rows with no ledger trace.

### Recommendation

Option 1. Under it every FailedRoute row self-evidences what was actually
dialled, which is the property a privacy auditor can verify from the ledger
alone; under option 2 the same auditor sees a local row and must trust
route-layer code they cannot see from the ledger, and a regression there
would be invisible. The two options are not exclusive, but option 1 is the
ledger-side decision this seed actually asks for.

### What only the maintainer can answer

Whether a mid-session settings edit is supposed to apply to an in-flight
session at all: if sessions are meant to be pinned to their start-time
authorization, option 2 is the true fix and the ledger question mostly
dissolves; if live repointing is intended product behavior, option 1 is
mandatory. Secondarily, whether refusals deserve a distinct error code so
the ledger can distinguish "refused before the wire" from "failed remotely"
without leaning on the boolean.

## Closing note

The two recommendations cohere: both say terminal ledger events should be
stamped from the attempted route identity captured at dispatch time, the
same discipline provenance already applies to Actual's provider id. Seeds
862c and 7da4 remain open pending the maintainer answers above.
