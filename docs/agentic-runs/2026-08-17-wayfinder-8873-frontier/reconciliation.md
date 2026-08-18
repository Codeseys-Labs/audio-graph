# Reconciliation of the three parallel decision briefs

**Scope note:** read-only pass over `/home/codeseys/DevBox/audio-graph` on `master`, plus the two wave artifacts that exist only on `integration/session-memory-wave-20260814` (read via `git show`; `docs/prototypes/` does not exist on `master`, as the map warns). Every code and ADR line below I opened myself. Nothing was edited, committed, or resolved. **This is a reconciliation brief, not a decision. No ticket is resolved by it and no human agreement is recorded in it.**

---

## 1. CONTRADICTIONS

### C1 — Does an unmet evidence obligation block `Finalized`? (70c8 assumes "no"; a668 escalates it)

- **70c8 side:** "I assume partial admission does NOT by itself block Finalized" and "I assume 70c8 … pending user confirmation is NOT a finalization barrier … If a668 requires confirmation before Finalized, then Finalized becomes user-input-gated and my claim that Finalizing needs no wall-clock deadline and no human in the loop is wrong."
- **a668 side:** open question #3 — "Can a Session reach Finalized while it still contains unconfirmed High-Impact Inferences or unresolved absence-class Knowledge Gaps … or are those Finalization Blocked reasons?"
- **Evidence:** `CONTEXT.md:82` — a High-Impact Inference "remains a proposal until the user explicitly confirms it." `CONTEXT.md:22` — Finalized Session Memory requires "final refinement has admitted durable canonical artifacts." Neither settles whether an *unadmitted* item blocks the boundary.
- **Who dominates:** neither. **This is a human choice**, and it is the single most load-bearing one in the wave: 70c8's recommended option derives its central property (Finalizing needs no wall-clock deadline and no human in the loop) from the answer it assumed. If the human answers a668's Q3 with "yes, unconfirmed proposals block," 70c8's recommendation must be re-derived, not patched. The two questions must be answered in the same sitting, not in two tickets.

### C2 — Is guaranteed constrained decoding a hard pre-dispatch capability gate, or optional?

- **21e9 side:** the capability check should "grade schema enforcement as guaranteed-constrained vs advertised-hint," and 21e9's recommendation states "my 'strict schema is a hard capability requirement' position collapses to 'advertised hint plus mandatory local validation and repair'" only if a668 needs unsupported constructs.
- **a668 side:** "I assume the route contract keeps the runtime validator as the sole admission authority regardless of route, and that a route lacking strict structured-output support is still usable for projections (falling back to the schemars schema plus validator) rather than being allowed to weaken the evidence requirements."
- **Evidence:** 21e9's own reading is decisive against itself. `src-tauri/src/llm/api_client.rs:128-131` — the generic client's `ResponseFormat` carries only `#[serde(rename = "type")] format_type: String`; there is no `json_schema` field to populate. `src-tauri/src/llm/api_client.rs:424-429` — `prefers_vllm_structured_outputs()` matches only `localhost:8000`, `127.0.0.1:8000`, `0.0.0.0:8000`, or `vllm`, so it is false for `api.cerebras.ai`. Applied literally today, a hard "guaranteed constrained decoding" gate rejects **both designated proof routes**: direct Cerebras cannot express the request at all, and OpenRouter's own docs (research §Structured outputs and endpoint variability) warn that enforcement varies by endpoint even with `strict: true`.
- **Who dominates:** **a668.** The validator must remain the sole admission authority (`src-tauri/src/projection_llm.rs:644` `validate_projection_patch_draft`, and the module's own contract at `:254-287` that the schema is "at least as strict as the runtime validator" but never the reverse). 21e9's capability check should *record and grade* schema enforcement in provenance, not gate dispatch on it. 21e9's brief already contains the concession; it just did not notice the concession is unconditional given the code.

### C3 — 21e9 assumed a668's schema fits the Cerebras strict subset; a668 chose the option most likely to break it, and never mentions the budget

- **21e9 side:** "I assume a668's admission schema fits Cerebras's strict-mode subset — object root, `additionalProperties:false` at every level, under 5,000 characters, 10 nesting levels, 500 object properties, 500 total enum values, and no recursion, external refs, string pattern/format, or array minItems/maxItems."
- **a668 side:** recommends the layered claim-class table, and concedes only that it "multiplies variant count in the hand-authored schema." The words "5,000", "character", "minItems", and "external ref" appear nowhere in a668's brief.
- **Evidence (verified):** the research doc on `integration/session-memory-wave-20260814`, `docs/research/cerebras-openrouter-projection-transports-2026-08-13.md` §Structured outputs: "The current strict subset requires an object root and `additionalProperties: false` for every object. It limits a schema to 5,000 characters, 10 nesting levels, 500 object properties, and 500 total enum values. Unsupported constraints include recursive schemas, external references, string `pattern`/`format`, and array `minItems`/`maxItems`." Two consequences a668 did not price:
  1. **No external references** means no `$ref` reuse, so a per-class annotation shape must be **inlined at every variant**. `src-tauri/src/projection_llm.rs:287-415` already builds the strict schema by fully inlining helper-produced JSON (`string()`, `nullable_string()`, `string_array()`, `variant()`) across 14 `variant(...)` constructions over 13 `ProjectionOperation` variants (`src-tauri/src/projections.rs:1262-1318`). Adding an evidence-annotation array plus an Inference Chain to every variant multiplies character count against a hard 5,000-character ceiling with no compression mechanism available.
  2. **No array `minItems`** means "at least one Evidence Annotation" and "at least one plausible alternative" — the two core requirements of a668's recommendation — **cannot be expressed in the strict schema at all**. They are validator-only, exactly like the existing numeric ranges documented at `projection_llm.rs:281-287`.
- **Who dominates:** neither yet — **this is measurable, not a judgement call.** a668 owes a serialized character count of its proposed strict schema per `ProjectionKind` before its ADR is written. If it exceeds 5,000, 21e9's assumption is void and C2 resolves in a668's favour by force rather than by argument.

### C4 — "Coverage marker" means three incompatible things across the three briefs

- **a668 side:** absence-class Knowledge Gaps are "admissible only against a named, versioned lane-coverage marker over the final transcript … per ADR-0034," and it assumes 70c8 exposes "a per-lane, versioned coverage-complete predicate over the final transcript that an absence-class Knowledge Gap can cite **as ADR-0034's exhaustive-coverage marker**."
- **70c8 side:** derives per-lane coverage from accepted patch basis fields precisely *because* coverage is deliberately not persisted, and never describes it as named or versioned. Its recommendation's whole virtue is that coverage is "derived, disposable, head-vector-stamped."
- **21e9 side:** reads ADR-0034 correctly — "the LLM half of ADR-0034's versioned exhaustive-runtime-coverage matrix … 'add LLM projection producers to coverage version N' becomes a concrete ticket."
- **Evidence:** `docs/adr/0034-require-exhaustive-evidence-for-negative-data-egress-claims.md:46-47` — the marker must be "named, versioned" and must cover "every **content-bearing producer** enabled in the build." That is a producer inventory for **egress**. It is not, and cannot be, a statement about transcript coverage.
- **Who dominates:** **21e9's reading of ADR-0034**, because it is the ADR's text. a668's absence class needs its own separately named and versioned transcript-coverage marker, and 70c8's derived predicate cannot serve as one as currently specified (70c8 explicitly refuses to persist or version it). Three tickets, three meanings of one phrase — this is the kind of collision that survives review and then costs an ADR amendment.

### C5 — `OutcomeUncertain` is already taken, and 21e9 reuses it for a different concept

- **21e9 side:** retry classification is "RetryableTransport / RetryableCapacity / TerminalRequest / **OutcomeUncertain**," where OutcomeUncertain means the remote attempt's effect is unknown.
- **70c8 side (correctly quoting the accepted artifact):** "Restart from `RemoteInFlight` becomes **ExternalEffectUnknown**; restart from canonical `Pending` becomes **OutcomeUncertain**."
- **Evidence:** the 5e41 prototype's admission table — "`OutcomeUncertain` | The caller lost the result after an effect may have begun | No | Reconcile exact bytes before any append or visible advancement." That is a **canonical-write** state, not a remote-attempt state. The prototype's remote-attempt vocabulary is `DurableQueued` / `RemoteInFlight` / `ExternalEffectUnknown` under §Scheduler restart and lane independence.
- **Who dominates:** **5e41**, which is closed and accepted. 21e9 must rename its fourth class (`ExternalEffectUnknown` is the existing word). This is small and purely mechanical, but 70c8's Blocked-reason taxonomy consumes both vocabularies simultaneously, so the collision would land inside the one state machine that reads both.

### C6 — 70c8 asks 21e9 for a class that 21e9 does not have and does not own

- **70c8 side:** "audio-graph-21e9 will expose a retry classification with at least three DISJOINT and machine-readable classes … **definitely-not-dispatched**, definitely-failed-and-retryable, and externally-uncertain," and its open question #3 asks whether auto-retry is permitted when the route layer classifies a failure as "provably never-dispatched or provably Absent."
- **21e9 side:** its four classes contain no never-dispatched class. `RetryableTransport` is silent on whether the request reached the provider.
- **Evidence:** the class already exists, in the scheduler, not the route layer — 5e41 prototype §Scheduler restart: "A job may cross the remote boundary only after its scheduler record is `DurableQueued`. Restart from `DurableQueued` remains safe to dispatch **because no remote attempt began**." And "provably Absent" is the canonical-write `AbsentRetryAuthorized` capability, also 5e41's.
- **Who dominates:** **5e41.** Neither brief owns this. 70c8 should read never-dispatched from its own durable scheduler/attempt record rather than asking the route layer to infer it from a transport error — a route layer *cannot* know whether the socket closed before or after the provider began work, which is exactly why 5e41 put the fact in the durable record.

### C7 — a668 asserts as present-tense fact something 21e9 proves false

- **a668 side:** "I assume `ProjectionProvenance` **continues to** name the actually-served provider and model (never the merely-preferred one) after any pre-authorized fallback; `openrouter.rs:1090` `fallback_evidence` already distinguishes preferred from served, and I assume 21e9 preserves that distinction."
- **21e9 side:** "the patch records `client.config().model` — the REQUESTED model, not the served one," and `chat_completion_with_schema_cached` "keeps only `total_tokens` and drops `OpenRouterRoutingTelemetry`."
- **Evidence (21e9 is right):** `src-tauri/src/llm/openrouter.rs:1607-1617` — `chat_completion_with_schema_cached` ends `.map(|(text, telemetry)| (text, telemetry.usage.total_tokens.unwrap_or(0)))`, discarding `selected_provider` and `served_model`. `src-tauri/src/llm/executor.rs:967` — `let model = client.config().model.clone();`. `fallback_evidence` at `openrouter.rs:1090` does exist, but it feeds the separate global telemetry aggregate (`record_global`, `:1024`), never the patch. `ProjectionProvenance` is `{provider, model, prompt_id}` at `src-tauri/src/projections.rs:1246-1250`.
- **Who dominates:** **21e9.** a668 must restate this as a *requirement it is imposing on 21e9*, not an assumption about existing behaviour. As written, every per-item provenance claim a668 builds inherits a requested-model label today.

### C8 — a668's evidence-repair call is already a silent cross-provider egress event

- **a668 side:** open question #2 — "is one evidence-repair LLM call per patch pre-authorized spend (the repair path at `projection_llm.rs:610` replays the whole prompt)?" a668 frames repair as a cost question on the same route.
- **21e9 side:** "cross-provider fallback is never silent."
- **Evidence:** `src-tauri/src/llm/executor.rs:774-803`, `run_projection_repair_escalation` — its own doc comment reads "Run the repair prompt against the **NEXT backend in the fallback chain** after the one that produced the invalid draft (`produced_index`)," and only falls back to the producing backend when the chain is exhausted (`run(&attempts[produced_index], repair_messages)` at the tail). The chain for an OpenRouter user is `[projection_openrouter, projection_api, projection_native, projection_mistralrs]` (`executor.rs:664-699`), authorized solely by `settings.privacy_mode.allows_session_cloud_content_transfer()` (`src-tauri/src/commands.rs:2026-2028`), and `ensure_llm_provider_start_enabled` is never called in `executor.rs` (grep: only `provider_registry.rs:82,287` and `commands.rs:638,645,3029`).
- **Who dominates:** **21e9's "never silent" rule**, and a668's question must be re-asked as *two* questions: (a) may a repair be spent at all, and (b) must it stay on the producing route. This also matters for order — see §3.

### C9 — Finalization Blocked's existence depends on an ADR-0028 amendment that only 70c8 flags

- **70c8 side:** flags it as a maintainer-only authorization — "ADR-0028:75-79 currently sends an incomplete canonical drain OR finalization to the app-global, non-dismissible RecoveryRequired. Do you accept splitting that…?"
- **21e9 side:** treats Finalization Blocked as available infrastructure. Its retry classification "fails closed into that state"; its option D rests on "rely on Finalization Blocked"; and its low-regret argument ("A configured with empty fallback lists IS option D") is only safe if a route-exhausted lane has a per-session resting place.
- **a668 side:** "I assume Finalization Blocked can carry a typed evidence-condition reason."
- **Evidence:** `docs/adr/0028-separate-capture-lifecycle-from-foreground-workspace.md:77-78` — "An incomplete canonical drain **or finalization** enters RecoveryRequired rather than reporting Review or Saved," and `:50-53` — RecoveryRequired "cannot be cosmetically dismissed into a Saved or healthy state."
- **Who dominates:** **human choice, and it must be first.** If the amendment is declined, ADR-0028 as written turns every finalization failure into an app-global modal, and two of the three briefs lose their failure destination without saying so.

### C10 — a668 assumes exactly one refinement pass; 70c8's recommendation makes refinement retryable

- **a668 side:** "I assume final refinement runs at exactly one barrier after the backlog drain … If 70c8 admits multiple or resumable refinement passes, the 'passes final refinement' condition needs a per-pass definition."
- **70c8 side:** its recommendation admits "per-lane attempt budgets with backoff," Blocked with "a typed retry authorization," and a return-to-progress path that "re-derives from disk first" and can reach a remote retry.
- **Who dominates:** **70c8**, because `CONTEXT.md:30` requires Finalization Blocked to retain "an exact reason and **a retry path**." The assumption is therefore already violated, and a668 owes the per-pass definition it named as its own contingency.

### C11 — Final refinement has at least four claimed owners and no declared seam

- 5e41 (closed, accepted) explicitly deferred it: the prototype's closing line under §Scheduler restart — "The model does not choose backlog coalescing, priority, concurrency, or **final refinement semantics**. Those remain in the scheduler design under `audio-graph-3b48`."
- `audio-graph-fbca` ("Define evidence-aware long-Session refinement") owns, per its own description: "safe-fit calculation, topic turn and time-based partitioning, per-partition outputs, **cited reconciliation, output budgets**, overlap and contradiction handling, **coverage accounting**."
- 21e9 makes refinement one of three route operations with a per-route completion budget and capability-pinned context limits.
- 70c8 makes it a barrier with attempt budgets.
- a668 makes it the per-item admission gate with "cited" per-class evidence.
- **Evidence of the gap:** none of the three briefs lists a single assumption about `fbca`. 21e9's assumptions name 70c8, a668, and 3b48; 70c8's name 3b48, 1d92, 44c1, 90f3, 8e73; a668 mentions fbca only under `fog_now_specifiable`. Three briefs are specifying inside fbca's stated territory while treating it as absent.
- **Who dominates:** **human choice on the seam.** At minimum, "output budgets" and "coverage accounting" cannot be simultaneously owned by 21e9's route capability record, 70c8's barrier, and fbca's algorithm.

---

## 2. HIDDEN COUPLING

These are decisions that read as local to one ticket but silently close options in another.

**H1 — Answering 70c8's "which lanes are required for Finalized" also decides whether a668's absence class can ever be admitted.**
70c8's open question #2 asks whether both notes and graph lanes are required. If the answer is "notes only," then a graph-lane absence claim has no coverage basis over the final transcript, and a668's absence-class Knowledge Gap — the class its entire recommendation is justified by (see C4) — is permanently inert for graph facts. The human would be choosing a668's option shape while believing they are answering a lifecycle question.

**H2 — Declining the ADR-0028 amendment silently forces 21e9 to keep automatic cross-provider fallback.** *(sharpest one)*
21e9's recommended option A is defended as low-regret because "option A configured with empty fallback lists IS option D," i.e. exhaustion is safe because it lands in Finalization Blocked. Without the amendment, `ADR-0028:77-78` sends a route-exhausted finalization to app-global, non-dismissible `RecoveryRequired`. A single Cerebras organization-scoped 429 (research §Errors, retry behavior, and quotas — the bucket is charged from input plus the *declared* `max_completion_tokens` **before** generation) would then take the whole app modal. Under that pressure the only survivable configuration is a non-empty automatic fallback list — precisely the `executor.rs:664-699` behaviour 21e9 exists to delete. A "no" to 70c8's ADR question is a "yes" to keeping unauthorized cross-provider fallback, and no brief says so.

**H3 — Answering a668's audio-retention question adds a member to an artifact enum that 70c8's deletion and blocked paths must then cover.**
`src-tauri/src/persistence/mod.rs:370-383` — `SessionArtifactKind` has twelve members and no audio kind. `ADR-0027:96-101` makes that one typed manifest drive "load, export, backup, delete, purge, recovery, retention, and usage" and requires deletion to return "an exact residual manifest if any removal fails." Adding audio therefore adds a residual-failure class to 70c8's deletion/fence path and a producer/privacy-class row to ADR-0034's inventory. a668 correctly flags this as blocked on the human; neither of the other two briefs knows the question exists.

**H4 — 21e9's per-operation route pinning can make a Blocked session permanently unsatisfiable, and neither brief covers it.**
21e9 recommends pinning a capability snapshot and route identity at content-start, and assumes "restart recovery re-derives route identity from the durable attempt record, NOT from live settings." 70c8's detached finalization owner then dispatches refinement *after* rotation, possibly after an app update. But `ADR-0033:48-52` requires every content-bearing start to "resolve its **actual** … descriptor and reject a descriptor whose `ui_selectable` value is false," and `:58-65` exempts only "stop, cancel, disconnect, drain, or cleanup of an active legacy session" — not a *new* refinement start. A route pinned in a durable attempt record that later becomes non-`ui_selectable` yields a Blocked session whose only retry path is rejected forever. Neither brief specifies whether such a session may be re-routed (which needs a human authorization under 21e9's own "never silent" rule) or stays Blocked.

**H5 — a668's verified-verbatim-quote requirement moves 21e9's quota math and 70c8's artifact privacy classes.**
a668's recommended class (a) requires the model to emit the verbatim supporting substring, and a668 itself notes for its option 3 that copying transcript text "duplicates content into the notes/graph artifacts, widening the redaction and privacy surface." Applied to class (a) it does the same, at lower volume: it increases per-item output tokens (raising the Cerebras pre-generation rate-limit charge), and it changes the content class of `MaterializedNotes`/`MaterializedGraph` that 70c8's deletion inventory and ADR-0034's redaction rules govern. It looks like a schema choice inside a668.

**H6 — 70c8's "re-derive Blocked before showing or retrying" acquires a668's per-item cost.**
70c8's best property is that "Blocked cannot rot into a lie: because the reason is re-derived before it is shown or retried." It budgets for head-vector comparison and names naive replay as its one performance con. But if a668's admission gate is per-item and evidence-resolving ("every annotation resolves to a live revision of the final transcript"), then re-derivation is O(items × annotations) per load, not O(streams). 70c8's mitigation — a cached head vector — does not cache item-level annotation resolution. Neither brief priced this.

**H7 — 21e9's pinned capability snapshot is also a per-item evidence budget.**
21e9 correctly insists the capability check evaluate the *selected endpoint* (Cerebras behind `google/gemma-4-31b-it`: 131,072 context / 40,960 max completion) rather than the aggregate model record (262,144). Pinning 40,960 completion tokens caps how many evidence-rich items a refinement partition can emit; evidence-annotated items cost materially more tokens each. So 21e9's capability pin silently sets a668's items-per-partition ceiling and collides with fbca's "safe-fit calculation" (C11).

---

## 3. DECISION ORDER

The Seeds graph has all three with empty `dependencies` (verified via `jq` over `.seeds/issues.jsonl`), so they are formally parallel. The domain says they are not.

**Step 0 — one human sitting, three questions, before any of the three tickets is decided.** None of these is derivable from the accepted ADRs, and each one is load-bearing for at least two briefs:
1. The ADR-0028:77-78 split (C9/H2) — because it decides whether a per-session Blocked state exists at all, which two briefs assume.
2. Does an unmet evidence obligation or unconfirmed High-Impact Inference hold the Finalized boundary (C1) — because 70c8 assumed the answer and a668 escalated it.
3. Is Original Session Audio retained (H3) — because it decides an artifact-enum change that touches all three.

**Then: 70c8 → a668 → 21e9.**

- **70c8 first.** It *produces* what the other two consume: the state set, the closed Blocked-reason taxonomy, and the per-lane coverage predicate that a668's absence class and 21e9's fail-closed destination both cite. What it consumes in return is largely already available from closed work: never-dispatched is 5e41's `DurableQueued`, and provably-Absent is 5e41's `AbsentRetryAuthorized` (C6). Its one genuine ask of 21e9 — "give me ≥3 disjoint machine-readable classes" — can be pinned as a one-line requirement without deciding the route contract.
- **a668 second.** Its output schema is the input to 21e9's capability grading, and 21e9's own brief concedes its strict-capability position "collapses" depending on what a668 needs (C2/C3). Deciding 21e9 first would pin a capability gate against a schema that does not exist yet. a668 also needs 70c8's coverage predicate for its absence class, hence second not first.
- **21e9 last.** Its two open parts — the per-operation override surface and whether cross-provider fallback exists at all — are exactly the parts the other two constrain (H2, H4). And its recommended option is explicitly designed to be decidable last without regret: "option A configured with empty fallback lists IS option D." Deciding it last costs nothing; deciding it first costs a re-derivation.

**One honest counter-current, because it inverts:** *implementation* order must not follow *decision* order for one specific pairing. a668's stricter validator must not ship before 21e9's fallback deletion. Tightening per-item evidence raises the validator rejection rate (a668 concedes this for its option 3, and its recommended option raises it too), and every rejection today escalates the repair prompt to the **next provider in the chain** (`src-tauri/src/llm/executor.rs:774-803`), authorized only by a privacy boolean (`commands.rs:2026-2028`) with no ADR-0033 gate at that site. So a668-decided-second must be **a668-implemented-after-21e9's-fallback-removal**, or the wave's own evidence hardening becomes an unauthorized-egress amplifier. That seam belongs in the handoff, not left to whoever picks up the implementation tickets.

---

## 4. SHARED CONSTRAINTS

### Genuinely agreed, correctly cited by all three (safe to treat as fixed)

- **5e41's three accepted policies** — Saved only after durable `Accepted`/`AlreadyAccepted`; no automatic reissue of externally uncertain remote work without provider idempotency proof plus explicit cost and egress authorization; immediate deletion fencing with late-result discard. All three briefs quote the closeReason accurately.
- **ADR-0027's `Accepted` boundary as the only durability authority** (`0027:66-70`), and its rejection of dual authority ("split-brain recovery has no deterministic winner," `0027:145`).
- **ADR-0031's single classifier**, Revised never mutating notes or graph state, and "Classification authorizes semantic applicability, not durability" (`0031:80-84`).
- **ADR-0032's tier 3 as release-blocking** and "A command may claim only what it asserts" (`0032:63`).
- **ADR-0033's gate at every content-bearing start** (`0033:48-52`), with the stop/cancel/cleanup carve-out (`0033:58-65`).
- **ADR-0030's planned-vs-observed route rule** (`0030:64-66`).
- **Trusted metadata is never model-stamped**, and today's provenance is exactly `{provider, model, prompt_id}` (`src-tauri/src/projections.rs:1246-1250`).
- **The timeout seam is independently agreed** — 21e9 owns the per-attempt deadline, 70c8 owns totals and the no-progress rule. Both briefs state it from opposite sides and converge. Worth recording as validated, because it is the only place they independently agreed on an ownership boundary.

### Disagreements about what is already settled — defects worth flagging

**D1 — Two of three briefs treat `Accepted` as available; it is not.**
70c8 says so plainly ("the central exit barrier is phrased over an `Accepted` acknowledgement that the runtime does not yet provide"). Verified: `src-tauri/src/persistence/mod.rs:2313-2320` — `ProjectionEventWriter::append` is a non-blocking `try_send` returning "enqueued"; `:2435-2465` — the writer thread flushes its `BufWriter` at shutdown. 21e9's constraint #1 and a668's ADR-0027 constraint both state `Accepted`-gated rules with no note that the primitive is missing. All three specifications are correct-but-unimplementable until the `audio-graph-90f3` / `audio-graph-8e73` commit-boundary work lands. That should be stated once, in one place, rather than discovered by one brief in three.

**D2 — a668 presents an analogical extension of ADR-0034 as a citation.**
a668's constraint says ADR-0034 "governs any absence-shaped Knowledge Gap ('nothing was said about X')," and its recommendation says "two accepted ADRs jointly rule out the uniform options." ADR-0034's text is exclusively about data egress: its five conditions (`:44-50`) turn on "every content-bearing **producer** enabled in the build" and a content-egress row. Extending "positive and negative evidence have different logic" (`0034:18`) from egress claims to knowledge claims may well be the right principle — but it is **a new decision a668 is proposing**, not settled ground that forecloses its own options 1 and 3. Presented as settled, it makes a668's recommendation look forced when it is in fact a choice. 21e9 reads the ADR correctly, which is how the discrepancy surfaced.

**D3 — "Knowledge Gap" is a User World term, and the User World is out of scope for this map.**
`CONTEXT.md:93-94`: "**Knowledge Gap**: An explicitly unresolved part of the **User World** for which the available evidence is missing, insufficient, or conflicting, together with the reason it remains unresolved." The map's Out of scope: "World Promotion and reconciliation into the User World; this receives its own map after trustworthy Finalized Session Memory exists." Both 70c8 ("some items admitted, some recorded as Knowledge Gaps") and a668 (its entire absence class) use Knowledge Gap as a session-scoped per-item output. Either `CONTEXT.md`'s definition needs amending to admit a session-scoped sense, or the briefs need a different term. This matters because it is the term a668's recommendation is most justified by (C4, D2).

**D4 — 70c8 cites ADR-0029 as licensing what ADR-0029 mostly gates.**
70c8's only answer to its own performance con is "a rebuildable cached head vector, which must be built strictly as an ADR-0029-class disposable derivative." The disposability language it quotes is real (`0029:59-62`). But ADR-0029's Decision Outcome opens with seven *preconditions* — "A query-index proposal may proceed only when: (1) a committed cross-session product query and representative corpus are named; (2) canonical file replay exceeds an agreed and **measured** UX latency or memory budget…" (`0029:45-57`). Whether a per-session finalization cache falls inside "query-index proposal" is genuinely unresolved; I am flagging the ambiguity, not asserting the gate applies. It matters because H6 makes that cache more expensive than 70c8 budgeted.

**D5 — The briefs disagree about whether today's fallback chain is a feature or a defect.**
21e9 calls the existing chain "authorized only by a privacy-mode boolean — not by any per-route user authorization" and treats deleting it as a benefit. a668 assumes provider substitution is normal and benign ("a route lacking strict structured-output support is still usable … falling back"). Verified: `executor.rs:664-699` plus `commands.rs:2026-2028`. This is the same disagreement as C2/C8 but at the level of *status quo assessment*, and it is worth naming separately because a reviewer reading only a668 would conclude the current chain is fine.

**D6 — Retry progression was deferred to 3b48, and two briefs specify it anyway.**
The 5e41 prototype's deferral is explicit: "The model does not choose backlog coalescing, priority, concurrency, or final refinement semantics. Those remain in the scheduler design under `audio-graph-3b48`," and 3b48's own description claims "failure and retry progression." 70c8 nonetheless specifies per-lane attempt budgets with backoff (and does flag the stop-time-flush seam, to its credit); 21e9 specifies a retry classification and asks the human about auto-retry on truncation. Also unmentioned: `src-tauri/src/projection_scheduler.rs:354-356` returns `Idle` when `last_failed_basis == basis`, so a failed lane on an unchanged basis stalls forever today — 70c8 is the only brief that noticed, and it is genuinely unowned.

---

### One-line summary of what the human must not do

Do not answer a668's Q3 (does an unmet obligation block Finalized) or 70c8's ADR-0028 question in isolation. Each one silently re-decides the other's recommendation, and H2 means the ADR-0028 answer also silently decides 21e9's fallback question. Those three answers are one decision wearing three ticket numbers.
