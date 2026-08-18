# Session Control Contract — Decision Packet

Three tickets, one sitting. Read §0 first: it contains three questions that are **not** ticket-local, and answering them inside a ticket silently re-decides the other two.

Everything below is a proposal for you to accept, revise, or reject. **No ticket is resolved here and no agreement of yours is recorded here.**

---

## §0 — Decision order, and the three cross-cutting questions that come first

**Order: three shared questions → `70c8` (finalization state) → `a668` (per-item evidence) → `21e9` (route contract).**

Why this order, not the frontier order:

- **`70c8` produces what the other two consume**: the state set, the closed `Finalization Blocked` reason taxonomy, and the per-lane coverage predicate. `a668`'s absence-claim class cites that predicate; `21e9`'s retry classification fails closed into that state. What `70c8` needs back is mostly already closed work — never-dispatched is 5e41's `DurableQueued`, provably-absent is 5e41's `AbsentRetryAuthorized`. Its one real ask of `21e9` ("give me ≥3 disjoint machine-readable failure classes") is a one-line requirement, not a decision.
- **`a668` before `21e9`** because `21e9`'s capability grading is graded *against a schema that does not exist yet*. `21e9`'s own brief concedes its "strict schema is a hard capability requirement" position "collapses" depending on what `a668` needs. Deciding `21e9` first pins a gate against a phantom.
- **`21e9` last, at zero regret**, by its own construction: "option A configured with empty fallback lists IS option D." Its two open parts (per-operation override surface, whether automatic fallback exists) are exactly the parts the other two constrain.

**Counter-current on implementation order — do not let it follow decision order.** `a668`'s stricter validator must ship *after* `21e9`'s fallback removal. Every validator rejection today escalates the repair prompt to the **next provider in the chain** — `src-tauri/src/llm/executor.rs:774-780` doc comment: "Run the repair prompt against the NEXT backend in the fallback chain after the one that produced the invalid draft" — authorized only by a privacy boolean (`src-tauri/src/commands.rs:2026-2028`), with no ADR-0033 gate anywhere in `executor.rs`. Tightening evidence first turns the wave's hardening into an unauthorized-egress amplifier.

### The three shared questions

**Q0.1 — Do you authorize splitting ADR-0028:77-78?** Today: "An incomplete canonical drain **or finalization** enters RecoveryRequired rather than reporting Review or Saved," and RecoveryRequired "cannot be cosmetically dismissed into a Saved or healthy state" (`docs/adr/0028-...md:50-53`). The split: incomplete *local drain* stays capture-scoped and may raise RecoveryRequired; post-stop *finalization* failure becomes a per-session `Finalization Blocked` record.

> **DEFAULT: yes, authorize the split.** **⚠ Expensive to get wrong, and it is not really `70c8`'s question.** If you decline, `ADR-0028` as written makes one Cerebras organization-scoped 429 take the whole app modal — and the only survivable configuration then is a non-empty automatic cross-provider fallback list, i.e. the exact `executor.rs:664-699` behaviour `21e9` exists to delete. **A "no" here is a silent "yes" to keeping unauthorized cross-provider fallback.** Two of the three briefs assume `Finalization Blocked` exists as available infrastructure without noticing it is contingent on your answer.

**Q0.2 — Does an unmet evidence obligation, or an unconfirmed High-Impact Inference, hold the `Finalized` boundary?** `CONTEXT.md` line 82 says a High-Impact Inference "remains a proposal until the user explicitly confirms it"; the `Finalized Session Memory` definition requires only that "final refinement has admitted durable canonical artifacts." Neither settles it.

> **DEFAULT: no — partial admission and unconfirmed proposals do not block `Finalized`; unmet obligations are recorded as typed gaps.** **⚠ Expensive to get wrong.** `70c8`'s recommended option derives its central property (Finalizing needs no wall-clock deadline and no human in the loop) from this default. Answer "yes" and `70c8` must be **re-derived, not patched** — `Finalized` becomes user-input-gated, and the whole "no fixed wait" posture goes with it. `a668` raised this as its own Q3; it is the same question.

**Q0.3 — Is Original Session Audio retained on disk in this slice?** `SessionArtifactKind` has twelve members and no audio kind (`src-tauri/src/persistence/mod.rs:370-383`); no production code writes session audio (only `aec_vad_fixtures.rs` / `source_separation_fixtures.rs`).

> **DEFAULT: no, not in this slice.** Cheap to reverse, but it decides whether retained-audio-range Evidence Annotations are a *satisfiable requirement* or an *optional enrichment*. Saying yes adds a member to the one typed manifest that ADR-0027:96-101 makes drive "load, export, backup, delete, purge, recovery, retention, and usage" — which adds a residual-failure class to `70c8`'s deletion path and a producer row to ADR-0034's inventory.

---

## §1 — `audio-graph-70c8`: what is authoritative for a session's finalization state

**The decision:** whether finalization state is a persisted stage enum or a set of predicates re-derived from durable canonical watermarks plus a durable remote-attempt ledger — and where capture returns to `Idle` so a second Session can start while the previous one finalizes.

### Already bound

| Constraint | Source |
|---|---|
| Split-brain has no deterministic winner; dual authority rejected for storage | `docs/adr/0027-...md:145` |
| Derived state is disposable, head-vector-stamped; mismatch → rebuild, never authority arbitration | `docs/adr/0029-...md:59-62` |
| Coverage and failure state are **deliberately not persisted**; a restored in-flight job is demoted to pending | `src-tauri/src/projection_scheduler.rs:606-625, 845-857` |
| Restart rules are all "re-derive from durable evidence," never "resume at stage N" | 5e41 prototype §Admission state and crash reconciliation |
| `Finalization Blocked` retains "an exact reason and a retry path"; Finalization Phase explicitly avoids "fixed wait" | `CONTEXT.md` |
| Release-blocking tier is a deterministic offline fixture; "A command may claim only what it asserts" | `docs/adr/0032-...md:63` |
| A failed lane on an unchanged basis returns `Idle` forever — no watchdog, no owner | `src-tauri/src/projection_scheduler.rs:355-357` |
| Session status is a bare `"active" \| "complete" \| "crashed"` string | `src-tauri/src/sessions/mod.rs:62` |

### Options

| Option | Core property | Fatal / limiting |
|---|---|---|
| **Persisted stage machine** | One field says where it is; trivial Review display; explicit transitions | Second authority next to head vectors — the pattern ADR-0027:145 rejected. Load must reconcile anyway, so the field buys legibility, not correctness. Over-serializes safely concurrent work. Manifest migration per new lane. |
| **✅ Derived barrier reconciler** (watermarks + attempt ledger + Blocked record) | Restart recovery *is* normal progress — one code path, which is what makes ADR-0032 tier-3 offline proof affordable. Blocked cannot rot: re-derived before shown or retried, so a satisfiable Blocked clears with zero cost and zero egress. Permits safe drain/flush overlap. | Predicates need a cached head vector or they replay per query. Progress is computed, not glanced at. The attempt ledger is still durable state — "derived except where it cannot be." Needs Q0.1. |
| **Stop-blocking serialized finalization** | Smallest delta — it is literally today's `commands.rs:7148-7177` gate. ADR-0028 stays true as written. | Violates the ticket's back-to-back capture requirement. A slow remote refinement holds the microphone hostage. Pushes toward the fixed wait `CONTEXT.md` forbids. |

### Recommendation — **Derived barrier reconciler. Confidence: medium.**

Three independent signals agree: the codebase already refuses to persist finalization progress while accepted patches carry their own basis (so lane coverage is *already* derived); the project already ruled against dual authority twice (ADR-0027, ADR-0029); and none of 5e41's accepted restart rules is expressible as "resume at stage N."

Shape: **one** wall-clock deadline in the whole machine — a bounded *Local Durable Stop* barrier (final flush chunk or explicit discontinuity, provider close classified, pre-stop events `Accepted`, stop cut recorded with head vector), using a named p99-tuned constant in the style of `state.rs:1112-1128`. Capture returns to `Idle` there. `Finalizing` gets no deadline: per-attempt budgets, per-lane budgets with backoff, and a **no-progress rule** → Blocked. Cancellation yields `Blocked{UserCancelled}`, never `Finalized`. Deletion always wins. Back-to-back capture is preserved by *scoping* the quiesce gate — capture-scoped producers still quiesce (`commands.rs:684-712` keeps its meaning), the detached finalization owner registers in a separate per-epoch registry never consulted by `ensure_session_idle_for_rotation`.

**Why medium, not high:** the central exit barrier is phrased over `Accepted`, which the runtime does not provide — `ProjectionEventWriter::append` is a non-blocking `try_send`, and the `BufWriter` flushes only at shutdown (`src-tauri/src/persistence/mod.rs:2313-2320, 2435-2465`). This spec is correct-but-unimplementable until `audio-graph-90f3` / `8e73` land. That is not a defect in the choice; it is a defect in all three tickets (see §4).

### Questions only you can answer

1. **Which Projection Lanes are REQUIRED for `Finalized`?** `CONTEXT.md` says "required Projection Lanes" and names none.
   > **DEFAULT: notes required, graph recorded-but-not-required.** ⚠ **Watch the hidden coupling:** this also decides whether `a668`'s absence-claim class can ever apply to graph facts. "Notes only" makes a graph absence claim permanently inert — you'd be choosing `a668`'s option shape while believing you answered a lifecycle question.
2. **May the app auto-retry a Blocked refinement without asking, when the failure is provably never-dispatched or provably Absent?**
   > **DEFAULT: yes for those two classes only; explicit cost-and-egress authorization stays required for the externally-uncertain class (already settled by 5e41).** Cheap. Note both facts come from *your own durable scheduler record* (5e41's `DurableQueued`), not from the route layer — a route layer cannot know whether the socket closed before or after the provider began work.
3. **Is user-initiated "cancel finalization" offered, and does it stay retryable forever?**
   > **DEFAULT: offered, lands as `Blocked{UserCancelled}`, retryable indefinitely, Review does not nag.** Cheap.
4. **Q0.1 (ADR-0028 split)** — restated here because it is this ticket's blocker. ⚠ Expensive.

---

## §2 — `audio-graph-a668`: the canonical per-item evidence record

**The decision:** what evidence every admitted unit of Session Memory must carry, who produces it (model vs. backend), and how strictly the requirement differentiates by claim type.

### Already bound

| Constraint | Source |
|---|---|
| **No per-item evidence exists today** — provenance is per-patch (`basis` + `provenance` + one `confidence`) | `src-tauri/src/projections.rs:1085, 1246-1250, 1262` |
| The runtime validator, not the schema, is the admission authority; the strict schema is "marginally looser … on ranges only — never on structure or kind" | `src-tauri/src/projection_llm.rs:281-287, 644` |
| Layered per-item citation is **already shipped** for one class: spans **OR** graph context | `src-tauri/src/persistence/mod.rs:345-349` |
| The only existing graph citation is an untyped, revision-less `source_segment_id` | `src-tauri/src/graph/temporal.rs:38`, `graph/entities.rs:48` |
| Notes have `DeleteNote` but no `InvalidateNote`; graph has both Invalidate ops | `src-tauri/src/projections.rs:1269, 1285, 1299` |
| A revised covered span is `Revised` and may never mutate notes or graph state | `docs/adr/0031-...md` Decision Outcome |
| Trusted metadata is stamped by trusted code, never the model | ADR-0024 §3; `projections.rs:1246` |
| Cerebras strict subset: object root, `additionalProperties:false` everywhere, **≤5,000 characters**, 10 nesting levels, 500 properties, **no external `$ref`**, **no array `minItems`** | research doc §Structured outputs |

### Options

| Option | Core property | Fatal / limiting |
|---|---|---|
| **Uniform span-citation floor** (one `{span_id, revision_number}` per item) | Smallest schema delta; generalizes a shipped rule; trivially exhaustive fixtures | Cannot represent an absence-shaped gap without fabricating a citation. Treats a verbatim decision and a whole-session summary as evidentially identical. |
| **✅ Layered claim-class table** (per-class minimums; model supplies anchors, backend derives revisions/offsets/hashes/speaker refs) | Only option that can admit absence claims without fabricating evidence. Verified-substring evidence is *checkable*, not trusted. Gives promotion the typed item kind it already demands (`promotion.rs:48-56`). Keeps cheap notes cheap. | Largest spec surface. Adds a class-assignment failure mode structural validation cannot catch. **Character budget unmeasured** (see below). |
| **Verified verbatim quote on every item** | Maximum precision; mechanical verification; one rule | Structurally excludes distributed/aggregate/inferential support. Excludes absence claims. Duplicates transcript text into notes/graph artifacts, widening the redaction surface `projections.rs:1131-1200` keeps narrow. Highest token cost and rejection rate. |
| **Defer all per-item evidence to final refinement** | Zero live-path change; one checkpoint | Contradicts `1d92` evidence inspection; concentrates all failure at the most expensive call; refinement must reconstruct attribution after the fact. Defers rather than decides. |

### Recommendation — **Layered claim-class table. Confidence: medium-to-low.**

The strongest arguments for it are real: layering is precedent, not novelty (`persistence/mod.rs:345-349` already admits two evidence forms for one class); ADR-0032's "a command may claim only what it asserts" is the same principle one layer up; audio-range annotations can be specified as enrichment-that-degrades-to-`Unavailable Evidence` rather than as an unsatisfiable requirement. Authority split: model supplies **anchors only** (span ids, and for quoted assertions the verbatim substring — verifiable by containment); backend derives revisions, offsets, hashes, speaker refs, and class satisfaction. Corrections/retractions are **derived, not model-authored** — ADR-0031 already gives the mechanical trigger when a pinned revision advances — which requires adding `InvalidateNote` for parity.

**Two honest corrections to the brief, before you weigh it:**

1. **It over-claims its mandate.** The brief says "two accepted ADRs jointly rule out the uniform options," citing ADR-0034. ADR-0034's five conditions (`:44-50`) are exclusively about **data egress** — "every content-bearing **producer** enabled in the build." Extending "positive evidence and negative evidence have different logic" (`:18`) from egress to *knowledge* claims may be right, but it is **a new decision this ticket is proposing**, not settled ground. Read as settled, it makes the recommendation look forced when it is a choice.
2. **The character budget is unmeasured and may decide this for you.** No external `$ref` means a per-class annotation shape must be **inlined at every variant** — and `projection_llm.rs:287-415` already fully inlines 14 `variant(...)` constructions across 13 `ProjectionOperation` variants, against a hard 5,000-character ceiling with no compression available. Separately, no array `minItems` means "at least one Evidence Annotation" and "at least one alternative" — the two core requirements — **cannot be expressed in the strict schema at all**; they are validator-only.

   > **Ask for a serialized character count per `ProjectionKind` before accepting this option's ADR.** That is a measurement, not a judgement, and it may void the option.

### Questions only you can answer

1. **May an evidence-repair LLM call be spent on a deterministic validation failure — and must it stay on the producing route?**
   > **DEFAULT: yes, one repair per patch, but pinned to the producing route.** ⚠ **Read the second half.** Today `run_projection_repair_escalation` sends the repair to the *next provider*, so "one repair call" is currently **also a silent cross-provider egress event**. This must be asked as two questions, and the route-pinning half depends on `21e9`.
2. **Q0.3 (audio retention)** — this ticket's blocker for audio-range annotations. Default: no.
3. **Q0.2 (do unmet obligations / unconfirmed proposals block `Finalized`)** — same question as `70c8`'s. ⚠ Expensive.
4. **Terminology:** `CONTEXT.md:93-94` defines **Knowledge Gap** as "an explicitly unresolved part of the **User World**," and the User World is out of scope for this map. Both briefs use it as a session-scoped per-item output.
   > **DEFAULT: amend `CONTEXT.md` to admit a session-scoped sense, rather than invent a second term.** Cheap, but it is the term the recommendation is most justified by, so leaving it ambiguous is not free.

---

## §3 — `audio-graph-21e9`: the internal route contract

**The decision:** the one internal route contract for the three LLM operations — route identity, default, optional per-operation override, pre-authorized fallback list, capability check, normalized termination status, retry classification, provenance.

### Already bound

| Constraint | Source |
|---|---|
| Chat Completions is the only generation surface first-party-documented for direct Cerebras; Messages/Responses parity for the selected Gemma endpoint was **deliberately left unproven** | research doc §Endpoints, Unresolved unknowns 1-2 |
| Never auto-reissue externally uncertain remote work without idempotency proof + cost/egress authorization; neither route documents an idempotency key | 5e41 closeReason; research lines 176-181, 320-322 |
| Exactly **one** global LLM provider setting; no per-operation route concept exists | `src-tauri/src/settings/mod.rs:1225` |
| Cross-provider fallback exists, is hardcoded per provider, authorized only by a privacy boolean | `src-tauri/src/llm/executor.rs:664-699`; `commands.rs:2026-2028` |
| The ADR-0033 start gate is **never applied at the fallback dispatch site** (no `ensure_llm_provider_start_enabled` anywhere in `executor.rs`) | `src-tauri/src/commands.rs:638, 645, 3029` |
| Neither blocking client deserializes `finish_reason`; OpenRouter converts some length errors into HTTP 200 with `finish_reason: "length"` | `openrouter.rs:756-764`; `api_client.rs:148-155`; research §Streaming |
| Provenance discards the served route: `.map(\|(text, telemetry)\| (text, telemetry.usage.total_tokens...))` drops `selected_provider`/`served_model`; the patch records the **requested** model | `openrouter.rs:1615-1616`; `executor.rs:967` |
| Direct Cerebras **cannot express `json_schema`**: `ResponseFormat` carries only `format_type`, and `prefers_vllm_structured_outputs()` matches only localhost/vllm | `api_client.rs:128-131, 423-429` |
| Capability facts are discovery-time: selected Cerebras endpoint 131,072 ctx / 40,960 completion vs. aggregate record 262,144 | research lines 229-248 |
| The research selected **no** product contract — every rule here is this ticket's to propose | 3d0c closeReason |

### Options

| Option | Core property | Fatal / limiting |
|---|---|---|
| **✅ Single-skin named route table** (Chat Completions the only MVP-admitted wire skin; Messages/Responses reserved enum variants behind a gated probe) | Ships exactly as much neutrality as evidence supports. Fits the built shape — a route table plus provenance plumbing, not a new transport layer. Makes "never silent" mechanical: empty fallback list = no code path. Forces the four verified defects fixed as part of the contract. | Refactors a tested path. Third way to name a provider (must reconcile with settings variant + registry id). Reads as under-delivering vs. the ticket's literal wording. Persisted shapes need ADR-0027 migration. |
| **Three-skin adapter abstraction now** | Literal fulfillment; terminal-status normalizer becomes first-class | Two of three adapters encode **unvalidated** semantics the research explicitly refused to claim — which then get asserted in ADR-0032 release-blocking fixtures, i.e. a command claiming more than it asserts. Buys nothing for either proof route. |
| **Capability-negotiated dynamic selection** | Absorbs catalog churn | Implicit selection **is** the silent fallback the ticket forbids, in a costume. Breaks the deterministic golden fixture and ADR-0030's "planned route" label. Advertised ≠ guaranteed, so the negotiation can lie. |
| **One pinned route, no fallback at all** | "Never silent" becomes structural. Closest to 5e41's posture. Smallest surface. | Answers a different question. One org-scoped 429 blocks a lane with no recovery. Final refinement is exactly where one authorized alternate is cheapest. |

### Recommendation — **Single-skin named route table. Confidence: high on the core.**

High confidence attaches to: one named-route table; Chat Completions as the only MVP-admitted skin with Messages/Responses as gated reserved variants; per-**endpoint** capability checks (131,072/40,960, not 262,144); a normalized terminal status (Completed / Truncated / Refused / Failed / TransportLost); a four-class retry classification whose uncertain class is never auto-retried; and **served**-route provenance stamped by trusted code. Every one rests on something read. And it is low-regret: A with empty fallback lists *is* the pinned-route option, whereas pinned-route cannot reach A without a new decision.

It is also the only option that forces the four verified defects to be fixed inside the contract: no `finish_reason` on either blocking path (so a truncated patch fails as invalid JSON and escalates to the **next provider** — a silent cross-provider hop caused purely by truncation); direct Cerebras never requesting the one documented strict-decoding guarantee it has; requested-not-served model in provenance; and no ADR-0033 gate at the fallback dispatch site.

**One correction to the brief.** Its fourth retry class is named `OutcomeUncertain` — a name 5e41 already assigned to a **canonical-write** state ("restart from canonical `Pending` becomes `OutcomeUncertain`"). The existing remote-attempt word is `ExternalEffectUnknown`. Mechanical, but the collision would land inside `70c8`'s Blocked taxonomy, which reads both vocabularies.

### Questions only you can answer

1. **Per-operation override surface:** (a) one global route, (b) global + distinct final-refinement route, (c) independent per-operation.
   > **DEFAULT: (b).** The two incremental lanes want the same low-latency small-patch route; refinement has a materially different context and output budget. This is a product call about configuration surface, not an evidence call. Cheap to widen later.
2. **May automatic cross-provider fallback exist in the MVP at all — and must each list entry be individually authorized by you with its cost and egress consequence shown?**
   > **DEFAULT: ship with empty fallback lists; each future entry individually authorized.** ⚠ **Coupled to Q0.1.** If you decline the ADR-0028 split, empty lists become unsurvivable and this default inverts under pressure. Answer Q0.1 first.
3. **Is "Cerebras through OpenRouter" a distinct authorized egress route from "Cerebras direct" for authorization and provenance?**
   > **DEFAULT: yes, distinct.** It is a distinct processing path (AudioGraph → OpenRouter → upstream), and OpenRouter defaults `allow_fallbacks` to true, so asking for structured outputs does not even pin Cerebras. This also decides whether ADR-0034's producer inventory lists one LLM producer or two. Moderately expensive to reverse (it is a persisted authorization record shape).
4. **On terminal status `Truncated`, may the MVP auto-spend one larger-budget attempt on the same route?**
   > **DEFAULT: no — go to `Finalization Blocked`.** Cost-bearing, not derivable from the ADRs, and note that a larger declared `max_completion_tokens` also raises the Cerebras **pre-generation** rate-limit charge, so the "safe" retry makes a 429 more likely.
5. **Does the capability check *gate* dispatch on guaranteed constrained decoding, or only *record and grade* it?**
   > **DEFAULT: record and grade; never gate.** Applied literally today, a hard gate rejects **both designated proof routes** — direct Cerebras cannot express the request (`api_client.rs:128-131`) and OpenRouter warns enforcement varies by endpoint even with `strict: true`. The validator must remain the sole admission authority (`projection_llm.rs:644`). The brief already contains this concession; it did not notice the concession is unconditional given the code.

---

## §4 — What the three briefs could not determine, and why

1. **`Accepted` does not exist in the runtime.** All three tickets state `Accepted`-gated rules; only `70c8` flags that `ProjectionEventWriter::append` is a non-blocking `try_send` and the `BufWriter` flushes only at shutdown (`persistence/mod.rs:2313-2320, 2435-2465`). All three specs are **correct-but-unimplementable** until `audio-graph-90f3` / `8e73` land. This belongs stated once, at the wave level, not rediscovered per ticket.

2. **"Coverage marker" means three incompatible things.** `a668` wants ADR-0034's named/versioned marker to serve as a *transcript*-coverage predicate; `70c8` refuses to persist or version coverage at all (that refusal is its central virtue); `21e9` reads ADR-0034 correctly as a *producer inventory for egress*. `21e9`'s reading is the ADR's text — so `a668`'s absence class needs its **own** separately named and versioned transcript-coverage marker, which nobody has specified and which cannot be derived from `70c8`'s disposable predicate. Unresolvable inside any one ticket.

3. **Final refinement has at least four claimed owners and no declared seam.** 5e41 deferred "final refinement semantics" to `3b48`. `audio-graph-fbca` claims "cited reconciliation, output budgets, coverage accounting." `21e9` claims per-route completion budgets; `70c8` claims attempt budgets; `a668` claims the per-item cited admission gate. **None of the three briefs lists a single assumption about `fbca`.** At minimum, "output budgets" and "coverage accounting" cannot be owned simultaneously by three tickets. This is a seam only you can cut.

4. **Whether `a668`'s recommended schema physically fits.** Requires a serialized character count per `ProjectionKind` against the 5,000-character strict ceiling, with no `$ref` reuse available. Measurable, unmeasured, and potentially option-voiding.

5. **How reliably a Cerebras-class model attaches correct per-item anchors under a strict schema.** No fixture in this repo measures it. `a668`'s confidence is capped by this, and the fix is a deterministic offline fixture *before* the ADR is written, not after.

6. **Whether a route pinned in a durable attempt record may be re-routed if it later becomes non-`ui_selectable`.** ADR-0033:48-52 requires every content-bearing start to resolve its **actual** descriptor; `:58-65` exempts only stop/cancel/drain/cleanup of an *active* session — not a new refinement start after rotation or an app update. A pinned route that loses enablement yields a Blocked session whose only retry path is rejected forever. Neither `21e9` nor `70c8` specifies this, and re-routing would itself need your authorization under `21e9`'s own "never silent" rule.

7. **Retry progression was deferred to `3b48`, and two briefs specify it anyway.** Also unowned today: `projection_scheduler.rs:355-357` returns `Idle` when `last_failed_basis == basis`, so a failed lane on an unchanged basis **stalls forever**. Only `70c8` noticed.

---

**The one thing not to do:** do not answer Q0.1, Q0.2, and `21e9`'s fallback question in three separate sittings. Q0.1 silently decides the fallback question; Q0.2 silently re-derives `70c8`'s recommendation. Those three answers are one decision wearing three ticket numbers.
