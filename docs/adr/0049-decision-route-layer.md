---
status: proposed
date: 2026-10-18
deciders: [maintainer]
consulted: [seed audio-graph-8b3e, epic audio-graph-6dfb, seeds audio-graph-ad35 / d273 / 0e61 / 60c7]
---

# ADR-0049: Route System-One Decisions Through a Named Decision Route Table

> **Provenance.** Drafted for seed `audio-graph-8b3e` (S1-T6). One element is
> already maintainer-decided and is restated here, not reopened: **native
> engines report the calibrated probability of the reported answer (`max(p)`;
> noul `max(p, 1 - p)`), HTTP engines pass vendor confidence through unchanged,
> and confidence thresholds are therefore per route, not global.** That rule is
> already in the frozen contract's rustdoc
> (`decision-contract/src/lib.rs:118`, `src/decision/mod.rs:378-379`,
> `decision-engine/src/d3/mod.rs:230-233`). Everything else here is proposed and
> needs sign-off before S1-T7 (`audio-graph-0ac0`) starts.

## Context and Problem Statement

The System-1 work (epic `audio-graph-6dfb`) added typed decisions: a
`DecisionRequest { state, questions }` of `choice` / `noul` / `score` questions,
answered with probabilities and a `confidence`, with no text generation
(`src-tauri/crates/decision-contract/src/lib.rs:12-127`). Four engine families
now implement `DecisionEngine`:

- **HTTP** (`src-tauri/src/decision/mod.rs:20-35`): TypeSafe Jev, Cloudflare
  Workers AI (Jev, Clef-flash), Ollama, and vLLM. The module doc says
  "Evaluation-only... No product route or UI selectability" (`:1`). Local
  endpoints must be loopback IP literals, and Ollama `:cloud` model names are
  rejected (`:98-124`). Cloud credentials live in the existing keychain slots
  `typesafe_api_key` and `cloudflare_workers_ai_token` (`:38-44`).
- **Native Laya** (ONNX Runtime). English is pinned to
  `receptron/laya-onnx@d42786bdff8a7550316339c5fbf8f46caa83ea1b`
  (`src-tauri/src/models/mod.rs:160-161`). Multilingual comes from a hashed
  local export (seed `audio-graph-fb61`, `export/multilingual-manifest.json`).
  Per-type/cardinality temperatures from `laya_config.json` are applied in the
  engine.
- **Native D3** (candle). `vllm-sr/d3-{edge,lite,nano,mini}` are pinned by HF
  revision (`models/mod.rs:195-202`). Temperature is 1.0. FP32 runs everywhere
  except BF16 for nano/mini on CUDA. BF16 moves probabilities by up to 2.1e-2
  and flipped one d3-edge near-tie with a 0.0031 margin
  (`docs/designs/d3-native-runtime-2026-10-18.md:189-192, :330-331`).
- **Native Clef**: reserved. `EngineRegistry::create("clef-flash")` returns
  `EngineUnavailable { feature: "decision-clef" }`
  (`decision-engine/src/lib.rs:104-109`; seed `audio-graph-4bc6`).

Product code calls none of these yet. The judgments the epic targets are still
hand-written:

- `agent_proposal_kind` (`src-tauri/src/speech/mod.rs:975-999`): `ends_with('?')`,
  six English interrogative prefixes, and English keyword lists.
- `agent_signal_grade` (`speech/mod.rs:1144-1166`): an ASR-confidence floor of
  0.5, at least 4 tokens and 16 UTF-16 units, and a dangling `,;:` check. Its
  `Strong` grade is the Rust spend authority for live-assist auto-answer
  (`commands.rs:4890-4893`, epic `83cc`). Its `confidence` input is the **ASR
  segment confidence** (`speech/mod.rs:1265-1269`), not any semantic score.
- Entity resolution is an exact lowercase name index plus Jaro-Winkler above a
  caller threshold (`src-tauri/src/graph/temporal.rs:342-362`). Relation types
  are an open "preferred" list (`src-tauri/src/ontology.rs:87-95`). Claim
  classes are judged only by span resolution and quote containment
  (`src-tauri/src/claim_evidence.rs:42-52`). No World Promotion code exists.

The evidence so far is narrow. Laya and D3 match their Python references to
within 1e-3 on CPU and CUDA. That is runtime parity, not AudioGraph accuracy.
The vendor's own Laya benchmark shows 0.362 base accuracy and 0.766 after
fine-tuning (seed `audio-graph-f284`). The replay harness measures agreement
with the heuristics, not with human truth (`src/decision/eval.rs:1`;
`docs/research/system-one-eval-2026-10-18.md:56`). Live Jev numbers are
**pending**: the run stopped on a missing credential before dispatch (`:77`,
`:115`; seed `ad35` `live_eval.status`). Native latency on the reference
RTX 5090 / Ryzen 7700X:

- Laya, CUDA: p50 11-17 ms. Laya, CPU: p95 45-434 ms across checkpoints and
  1/3-question batches (seed `0e61`; report
  `docs/research/laya-native-2026-10-18.md` on `ao/audio-graph/root`).
- d3-edge, CUDA: p50 6.3 ms. d3-edge, CPU: p95 400 ms. d3-lite, CPU: p95
  764 ms (`docs/research/d3-native-latency-2026-10-18.md:29-34`).

This record decides how a decision reaches an engine, what it may send where,
how its confidence becomes an action, and in what order the heuristics move.
[ADR-0038](0038-route-llm-operations-through-a-single-skin-named-route-table.md)
answered the same question for LLM generation. The constraints here are the
same: never silent, egress authorized per route, served identity stamped by
trusted code.

## Decision Drivers

- **Never silent egress.** A decision about transcript or graph content is
  content-bearing. It must not reach a provider the user did not authorize for
  that route. A local failure must never become a cloud call.
- **Confidence is engine-relative.** Under the recorded semantics, `0.8` from
  Laya, D3, Jev, and Ollama are four different quantities. Any threshold not
  bound to a route and a model pin is meaningless.
- **Heuristics are the floor, not the enemy.** They are deterministic, local,
  pinned by mirrored fixtures (`speech/mod.rs:10350`), and mirrored in the
  frontend (`agentQueue.ts`). A decision route must strictly improve on them on
  measured data before it gets authority.
- **S1 decides about the graph; it does not author it.** Generation, entity
  minting, and Inference Chains stay System-2. Under `CONTEXT.md`, a confidence
  score is explicitly *not* an Inference Chain.
- **Validators stay sole admission authorities**
  ([ADR-0038](0038-route-llm-operations-through-a-single-skin-named-route-table.md)
  drivers; [ADR-0037](0037-admit-session-memory-items-through-a-layered-claim-class-evidence-table.md)
  part 3). A decision may refuse or downgrade. It never admits what the
  deterministic checks refuse.
- **Cross-platform honesty.** CUDA is the only GPU path measured. Metal for D3
  (`377a`), DirectML for Laya (`7e3d`), and Vulkan-only GPUs (`26b4`) are open.
  Route availability must be per OS and execution provider, never "GPU assumed".

## Considered Options

- **A. A named decision route table beside the LLM route table**: `route.decision.*`
  entities with a fixed engine kind, a derived egress class, a model pin,
  per-question calibration records, and no fallback walker.
- **B. Add decision rows to ADR-0038's `LLM_ROUTES`**: one table for every
  model call.
- **C. Engine id as the route**: call sites call `EngineRegistry::create(id)` or
  build a `SystemOneHttpEngine` directly, with global confidence thresholds.
- **D. Availability-negotiated selection**: try native, then local HTTP, then
  cloud, picking the first engine that is ready.

## Decision Outcome

Chosen option: **"A. A named decision route table"**. It is the only option that
keeps both of these properties: egress authorization per route, as ADR-0038
requires, and thresholds bound to a route, as the recorded confidence semantics
require. B breaks `LLM_ROUTES`' own invariants: every row is
`WireSkin::ChatCompletions`, and `finish_reason` maps to `TerminalStatus`.
Neither invariant means anything for `/v1/systemone`. C has no authorization
point and needs a global threshold. D is ADR-0038's rejected option C in
another costume: a local failure silently becomes cloud egress.

### 1. The decision route

A decision route is a row in one `const DECISION_ROUTES` table in the app crate
(`src-tauri/src/decision/route.rs`), with the same placement rationale as
`src-tauri/src/llm/route.rs:17-25`. Each row has:

| Field | Meaning |
| --- | --- |
| `id` | Stable `route.decision.<name>`, stamped into provenance by trusted code. Never a registry id or settings tag. |
| `provider_id` | A `decision.*` registry descriptor the route authorizes against under ADR-0033. This requires a new `ProviderStage::Decision` (`crates/provider-registry/src/lib.rs:82-88`). Every descriptor starts `ui_selectable: false`. |
| `engine` | `EngineKind`: `hosted_jev`, `hosted_cloudflare`, `local_http`, `native_laya`, `native_d3`, or `native_clef`. `native_clef` is a reserved variant with no constructor until `4bc6` lands. Like ADR-0038's reserved skins, it has no dispatch path at all. |
| `egress` | **Derived from `engine`, never configured**. `hosted_*` → `Egress::Cloud`. `local_http` and `native_*` → `Egress::Local`. A table test pins the mapping. |
| `model_pin` | Native: HF repo, 40-hex revision, and the SHA-256 of every component file, plus `precision` (`fp32`/`bf16`). Hosted: the requested alias (`jev-latest`, `typesafe/jev`, `clef-flash`) plus the served model read from `DecisionResponse.model`, tagged `Served`/`Requested` exactly like `ModelIdentitySource` (`llm/route.rs:730-741`). `local_http`: the configured model name plus a server-reported digest when one exists. |
| `limits` | The engine's `EngineLimits`, checked before dispatch (`lib.rs:143-170`). |
| `calibrations` | One `CalibrationRecord` per admitted question schema. See §3. |

Initial rows: `route.decision.laya` (native_laya), `route.decision.d3_edge`,
`d3_lite`, `d3_nano`, `d3_mini` (native_d3), `route.decision.ollama_loopback`
and `route.decision.vllm_loopback` (local_http), `route.decision.jev_typesafe`
(hosted_jev), `route.decision.jev_cloudflare`, and
`route.decision.clef_flash_cloudflare` (hosted_cloudflare).

Sub-decisions:

1. **Jev via Cloudflare is a different route from Jev direct.** The reasoning is
   ADR-0038 sub-decision 3: a different processing path
   (AudioGraph → Cloudflare account → TypeSafe model), a different credential
   slot, and a different producer row.
2. **No fallback walker, and no `fallback` field.** A route that fails or
   abstains returns that outcome to its caller. It does not try another route,
   local or cloud. Adding an alternate route needs a new ADR and new dispatch
   code, not a configuration change (`llm/route.rs:8-15`).
3. **The heuristic is not a route.** Each migrated decision keeps its
   deterministic function as the **heuristic fallback**. That fallback is
   local, has zero egress, and is recorded with `source: heuristic`. Using it
   is not a substitution in ADR-0038's sense: nothing leaves the device that
   would not have left it anyway, and provenance names what answered.
4. **One route per decision family, chosen in settings.** There are three
   families: `live_assist` (§4 M1-M2), `projection` (M3-M5), and `world`
   (M6). This mirrors ADR-0038 sub-decision 1. Every family defaults to
   **no route**, which means the heuristic. A Cloud route is never a default.
   The only automatic default allowed later is an installed, calibrated native
   route.
5. **Laya's script router.** It switches between English and multilingual
   checkpoints inside one engine. `route.decision.laya` pins **both**
   checkpoints and stamps the served one per decision. Calibration is looked up
   by the served checkpoint. When that checkpoint has no record, the decision
   runs in shadow (§3). The multilingual pin stays unreachable until
   `audio-graph-4022` approves a distribution source (`fb61`).
6. **Pin gaps close before authority.** English Laya and D3 are pinned by
   revision only today. Per-file SHA-256 exists only in the multilingual export
   manifest. A native route may not hold authority (§3) until every component
   file of its pin has a recorded digest that the model manager verifies.
7. **Authorization uses ADR-0038's seal.** `authorize_decision_route` is the
   only constructor of `AuthorizedDecisionRoute`, mirroring the `_seal`
   discipline at `llm/route.rs:378-392`. Every dispatch site takes one.

### 2. Egress

**Cloud routes** (`hosted_jev`, `hosted_cloudflare`) are content egress from the
first question. Before any content-bearing use, including shadow mode, a Cloud
route must have all of the following:

- the full Provider Addition Content-Egress Checklist
  (`docs/designs/provider-architecture.md:28-77`):
  - registry privacy metadata naming `TranscriptText` (M1-M2), plus `Notes` and
    `GraphContext` for M3-M5;
  - a no-content readiness probe;
  - a session command gate;
  - a low-level guard before the HTTP write;
  - a blocked-policy fixture proving no body is sent;
  - no payload echo;
- ADR-0033 promotion through the single registry table;
- a producer row in ADR-0034's versioned coverage inventory.

The evaluation CLI's `--allow-cloud` switch is explicitly not this gate
(`docs/research/system-one-eval-2026-10-18.md:41`). The `world` family (M6)
is **Local-only** under this record, because its state carries cross-session
User World content. A Cloud route there needs its own ADR.

**Local routes** are not `Local` because someone asserted it. Each one carries
the following zero-egress evidence, recorded where stated:

| Evidence | Where it is recorded |
| --- | --- |
| Native inference links no HTTP client. `decision-engine` disables `tokenizers` default features (no `http`) and uses `ort` with `load-dynamic` (no binary download) (`crates/decision-engine/Cargo.toml:26-27`). On 2026-10-18, `cargo tree --offline -p audio-graph-decision-engine --features decision-laya,decision-d3 -e normal` listed no `reqwest`/`hyper`/`ureq`/`hf-hub`/`rustls`/`native-tls`/`curl`. | A CI check runs the same `cargo tree` assertion for every native feature combination. Its result is the route table's evidence reference. |
| Model weights arrive only through the existing model manager, as a pinned, non-content download. English Laya uses a `resolve/<revision>` URL (`models/mod.rs:160-161`), and a test asserts pinned D3 revisions (`models/mod.rs:1652-1655`). | The model manager's download events, and the route's `model_pin`. |
| `local_http` reaches loopback IP literals only, with `:cloud` names rejected (`src/decision/mod.rs:98-124`) and proxies and redirects disabled (`docs/research/system-one-eval-2026-10-18.md:41`). | Existing decision tests. A route-table test pins that `local_http` cannot be constructed with a non-loopback URL. |
| Every dispatch, local or cloud, writes `ProviderCallStarted` / `Succeeded` / `Failed` rows with `DestinationBoundary::Local` or `Provider` and its data classes (`crates/ipc-contract/src/session_data_movement.rs:54-123`). | The session data-movement ledger. Each `decision.*` producer joins ADR-0034's inventory **before** it is enabled. |

`local_http` keeps one residual risk that AudioGraph cannot observe: a loopback
server can relay to the cloud (Ollama serves cloud models). So `local_http`
descriptors use `ProviderDataBoundary::UserConfiguredEndpoint`, not `LocalOnly`
(`crates/provider-registry/src/lib.rs:273-276`). Only `native_*` routes are
`LocalOnly`.

### 3. Calibration policy

**Engine output is never mutated.** Raw `probabilities` and `confidence` are
stamped as returned. The calibrated view is derived by the route layer and
stamped next to them.

**Calibration record.** Each `(route, served model pin, precision, question
schema id + version)` tuple has at most one `CalibrationRecord`:

- `temperature` T, fitted by minimizing negative log-likelihood on a held-out,
  **human-labeled** AudioGraph set. The f284 dataset is the source. The
  12-span public fixture and heuristic-agreement replays do not qualify.
- `accept` and `abstain` thresholds, where `abstain ≤ accept`.
- Dataset id, sample count, per-option counts, ECE before and after T (ten
  equal-width bins, the harness definition), and the target metric value at
  the chosen thresholds.

Minimum fit set: at least 200 labeled items per question schema and at least 20
per option. Below that, no record exists.

**Applying T.** Native routes apply T as `p_i ∝ p_i^(1/T)`, which equals
rescaling the engine's logits, then recompute `confidence` with the native
definition. HTTP routes keep T = 1. Their vendor confidence is passed through,
per the recorded decision, and the per-route thresholds absorb vendor
miscalibration. A missing `confidence` (noul `confidence` is optional on the
wire, `lib.rs:59-65`) counts as below `abstain`.

**Authority is derived, not configured.** A route is **authoritative** for a
question only when a record matches the decision's actual served pin,
precision, and schema version. Otherwise it runs in **shadow**: the decision is
dispatched and stamped, and the heuristic answers. No setting can make an
uncalibrated route authoritative. Each of these drops the route to shadow on
the next decision:

- editing question text (which bumps the schema version);
- a BF16/FP32 switch;
- a vendor alias moving (Jev's `jev-latest` alias tracks the current model);
- a served-model mismatch.

**Score questions** act on the argmax level, with ties going to the lower
index, as in the harness. They do not act on the expected score.

**Threshold vocabulary and its `CONTEXT.md` meaning.** Let `c` be the
calibrated confidence:

| Outcome | Condition | Live-assist spend gate (M1-M2) | Session Memory (M3-M5) | User World (M6) |
| --- | --- | --- | --- | --- |
| `decided` | `c ≥ accept` | The route's answer is used. | It may refuse or downgrade an item's claimed support. It never admits. | A non-High-Impact change may proceed as a World Promotion proposal. |
| `proposed` | `abstain ≤ c < accept` | **Refusal** of automatic spend. The card stays, with manual Ask AI. | No effect on admission. Stamped for review. | The change stays a **proposal** needing user confirmation. |
| `abstained` | `c < abstain` or no confidence | Heuristic fallback. | Heuristic fallback, which is today's deterministic judge. | **Knowledge Gap**, with the reason "decision below threshold on `<route>`". |
| `unavailable` | Transport, credential, limit, or engine error | Heuristic fallback. | Heuristic fallback. | Knowledge Gap with the error class. No retry on another route. |

Three standing rules:

- A **refusal** is an action not taken. It is never a knowledge claim, and it
  never creates a Knowledge Gap. Utterance classification is not User World
  knowledge.
- A **High-Impact Inference** stays a proposal at any confidence. Decision
  confidence can never confirm it.
- A decision is **never an Inference Chain**. It can grade a premise. It cannot
  stand in for one.

Session-level Knowledge Gaps stay unadmittable until
`ClaimClass::KnowledgeGap` becomes admissible (`claim_evidence.rs:47-49`).

**Provenance stamped into Session Artifacts.** Each dispatch produces one
content-free `DecisionRecord`:

- **Route:** route id, provider id, engine kind, egress.
- **Model:** requested model, served model and its source, model pin digest,
  precision.
- **Question:** question id and schema version.
- **Input:** a state digest (SHA-256 of the canonical state JSON, never the
  text) and the basis, as span ids plus revision numbers.
- **Answer:** the raw answer, raw probabilities, and raw confidence.
- **Calibration:** T, calibrated probabilities and confidence, and a snapshot
  of the thresholds plus the calibration record id.
- **Outcome:** outcome, authority (`shadow` or `authoritative`), `source`
  (`route` or `heuristic`), and the heuristic's answer whenever both ran.
- **Cost:** latency and usage.

Option keys are fixed schema identifiers, never content. Entity names and
transcript text go only in `state`.

Where the record goes:

- M1-M2: on `LiveAssistCardRecord` / `AgentProposalPayload`, as an additive
  optional field (the ADR-0027 pattern `signal` used).
- M3-M5: one record per question per projection patch, with items referencing
  it by id. Per-item byte cost governs `ProjectionProvenance`
  (`projections.rs:1791-1792`).

No new canonical stream is opened
([ADR-0043](0043-freeze-canonical-event-stream-registry.md)). Decisions ride
the artifact they decide about.

### 4. Ordered migration list

Every migration passes the same gates before it gets authority. G1-G5 apply in
shadow; G6 is the switch.

- **G1:** The route exists in `DECISION_ROUTES`, is ADR-0033-gated, and has an
  ADR-0034 producer row. For Cloud routes, the §2 checklist is complete.
- **G2:** A matching `CalibrationRecord` exists (§3).
- **G3:** On the record's labeled set, at the chosen thresholds, the route beats
  the heuristic on the migration's target metric and does not regress its
  safety metric.
- **G4:** p95 latency is recorded for CPU and for each execution provider the
  route claims, on the reference machine. A route whose CPU p95 exceeds 500 ms
  is not offered on CPU-only machines. By today's numbers, that keeps
  Laya and d3-edge and excludes d3-lite and larger on CPU.
- **G5:** Replay determinism holds: the same pin and state give the same answer
  within parity tolerance. BF16 near-ties (margin < 0.05) are flagged in the
  stamp.
- **G6:** The heuristic's mirrored fixtures still pass unchanged as the
  fallback.

Migrations run in order. Later ones start only when their host contract exists.

1. **M1 `agent_proposal_kind`** (`live_assist`; host `run_agent_proposal_task`).
   - **Question:** `choice` with criteria `question`, `graph_suggestion`,
     `note`, `none`, using the harness schema at `src/decision/eval.rs:110-122`.
   - **State:** `{ text, asr_confidence }` of one final span. Widening the state
     requires re-fitting.
   - **Outcome rule:** `decided` → the route's kind. `proposed` → the route's
     kind, with the card's signal capped below `Strong`. Otherwise the
     heuristic.
   - **Target metric:** question-class recall, which matters for non-English and
     prefix-less questions.
   - **Safety metric:** `question` precision at `decided` must be at least the
     heuristic's.
2. **M2 `agent_signal_grade`** (`live_assist`; ships in the same request as M1).
   - **Question:** `noul` `answerable_question`, meaning "a complete,
     intelligible question safe to auto-answer".
   - **Changes from the harness:** the harness's three-level `score`
     (`eval.rs:123-126`) asks the model to compare ASR confidence with 0.5.
     That is arithmetic, so the `weak` level leaves the schema. The ASR floor
     stays deterministic: `Strong` requires both `asr_confidence ≥ 0.5` and a
     `decided` true answer. A `decided` false answer gives `Fragment`.
     Otherwise the heuristic grades.
   - **Unchanged:** the Q2 dual gate (Rust grade plus frontend admit), the
     12-per-session cap, and the unported duplicate-collapse caveat
     (`events.rs:464-478`).
   - **Target metric:** `Fragment` recall on truncated utterances.
   - **Safety metric:** `Strong` precision, because each false `Strong` buys a
     paid answer.
   - **Owner:** `audio-graph-0ac0`, which also decides whether `agentQueue.ts`
     consumes the backend grade or stays byte-identical as fallback.
3. **M3 claim support** (`projection`; ADR-0037). This starts only after
   ADR-0038's fallback removal and ADR-0037's validator have shipped.
   - **Question:** `choice` `claim_support` with criteria `supports`,
     `does_not_address`, `contradicts`, per `grounded_inference` or
     `verified_quote` item.
   - **State:** the item text plus its resolved anchor span.
   - **Outcome rule:** a `decided` `does_not_address` or `contradicts` refuses
     the item, or downgrades it to the class the deterministic checks still
     support. It is an input to `judge_claim_evidence`, never a second judge.
   - **Heuristic fallback:** today's deterministic checks alone.
   - **Safety metric:** false refusals of correctly supported items.
4. **M4 entity resolution, same referent** (`projection`; within one Session).
   - **Question:** `choice` `same_referent` with criteria `same`, `different`,
     `unsure`.
   - **State:** the new mention with its span, plus the candidate node's name,
     type, and evidence span.
   - **Outcome rule:** `decided` `same` merges through `supersede_entity`.
     `decided` `different` blocks a Jaro-Winkler merge. Anything else keeps
     exact-name matches only and does not fuzzy-merge.
   - **Rationale:** a wrong merge invalidates edges; a missed merge leaves a
     duplicate node, which is cheaper ("duplication over loss",
     [ADR-0045](0045-schedule-projections-on-per-kind-coalescing-slots-with-derived-coverage-heads.md)).
   - **Safety metric:** wrong-merge rate.
5. **M5 relation type** (`projection`).
   - **Question:** `choice` `relation_type` over the nine `RELATION_TYPES` names
     plus `other`. Ten options fit the Ollama, vLLM, and Laya limits.
   - **Correcting the brief:** ADR-0008's set is not closed in code. It is a
     preferred list, and System-2 may emit a free verb
     (`ontology.rs:87-95`). This migration does not close it. `decided`
     normalizes the System-2 relation to a listed type. `other` and every
     non-`decided` outcome keep the System-2 verb unchanged, which is the
     heuristic fallback.
   - **Target metric:** agreement with labeled types.
6. **M6 World Promotion gate** (`world`; Local-only; blocked until World
   Promotion exists, since nothing implements it today).
   - **Questions:**
     - `noul` `high_impact`, asking whether the change concerns identity,
       intent, commitments, sensitive traits, or legal, medical, or financial
       circumstances;
     - `choice` `world_relation` with criteria `new`, `same`, `updates`,
       `contradicts`, against the matched User World item.
   - **Outcome rule:** only a `decided` false `high_impact` together with a
     `decided` `world_relation` may proceed without per-item confirmation.
     Everything else stays a proposal or becomes a Knowledge Gap (§3 table).
   - **Heuristic fallback:** everything is a proposal.
   - **Safety metric:** High-Impact recall must equal the all-proposal baseline
     of 1.0 on the labeled set. No route may lower it.

### Consequences

- **Positive**: "No silent egress" is structural again. A decision route has no
  path to a second route, so a native failure cannot become a cloud call.
- **Positive**: thresholds mean something. Each threshold is bound to a route,
  pin, precision, and schema. Each `0.8` is comparable only with itself, which
  matches the recorded semantics.
- **Positive**: authority is derived from calibration evidence. Shadow mode
  generates the labeled comparison data at no product risk.
- **Positive**: every decision is replayable and auditable from Session
  Artifacts without storing transcript text twice.
- **Negative**: a fourth provider-naming surface (`decision.*`, `route.decision.*`)
  joins the three ADR-0038 already flagged. A new `ProviderStage` churns the
  generated `src/generated/providerRegistry.ts`.
- **Negative**: no route gets authority without about 200 human labels per
  question and per pin. Until f284 produces a dataset, every route is shadow
  and the heuristics keep answering. That is deliberate, but it means the S1
  layer ships with no user-visible behavior change at first.
- **Negative**: every pin or precision change, including BF16 defaults that
  vary by GPU, needs a refit or drops to shadow. Per-machine precision
  differences can make one route authoritative on one machine and shadow on
  another.
- **Negative**: HTTP routes keep T = 1. Their miscalibration is absorbed only
  by thresholds, so hosted vendor model updates can silently shift behavior
  inside a pin that reports the same alias. The served-model check catches
  this only when the vendor changes the reported id.
- **Negative**: M2 departs from the evaluated harness schema (`weak` removed,
  `score` → `noul`). The harness must add the production schema before G3 can
  be measured.
- **Negative**: shadow mode on a Cloud route is still egress. It needs the full
  §2 checklist, so "shadow first" is not a cheap way to evaluate Jev on real
  sessions.
- **Neutral**: the decision layer serves the notes/graph pipeline and the
  live-assist spend gate only. The realtime speech-to-speech product is
  untouched.

## Pros and Cons of the Options

### A. Named decision route table

- Good, because egress is derived from engine kind, and authorization reuses
  ADR-0033/0038 machinery.
- Good, because calibration binds thresholds to exactly the quantity the
  recorded confidence semantics define.
- Good, because it works with empty settings (heuristic everywhere) and grows
  additively.
- Bad, because it adds a table, a registry stage, and a calibration record
  store to maintain.

### B. Decision rows in `LLM_ROUTES`

- Good, because there is one table and one authorization function.
- Bad, because `WireSkin`, `TerminalStatus`, `finish_reason` normalization, and
  completion budgets have no meaning for `/v1/systemone`. Every decision row
  would need exemptions from tests that pin those invariants.
- Bad, because `RouteRecord` (`llm/route.rs:817-840`) has nowhere to put
  probabilities, pins, or thresholds.

### C. Engine id as the route, global thresholds

- Good, because it is the smallest change. `EngineRegistry` already exists.
- Bad, because it has no ADR-0033 gate and no producer row, so egress is
  unauthorized.
- Bad, because a global threshold contradicts the recorded per-route
  confidence semantics.

### D. Availability-negotiated selection

- Good, because a decision always gets an answer when any engine is up.
- Bad, because the native → HTTP → cloud hop is silent egress, the exact
  failure ADR-0038 removed.
- Bad, because the answering engine, and with it the confidence semantics,
  changes per request, so no threshold is valid.

## More Information

- **Relationships.**
  - [ADR-0038](0038-route-llm-operations-through-a-single-skin-named-route-table.md):
    the discipline mirrored here (named routes, sealed authorization, no
    walker, served identity). It is not amended. Its open question about a
    pinned route losing `ui_selectable` applies equally here and is not decided.
  - [ADR-0033](0033-enforce-mvp-provider-enablement-at-content-start.md) and
    [ADR-0034](0034-require-exhaustive-evidence-for-negative-data-egress-claims.md):
    extended in reach to `decision.*` producers.
  - [ADR-0037](0037-admit-session-memory-items-through-a-layered-claim-class-evidence-table.md):
    M3 feeds its judge and does not replace it.
  - [ADR-0008](0008-conversation-ontology.md): M5 normalizes to its list
    without closing it.
  - [ADR-0043](0043-freeze-canonical-event-stream-registry.md): no new stream.
  - [ADR-0045](0045-schedule-projections-on-per-kind-coalescing-slots-with-derived-coverage-heads.md):
    M3-M5 decisions run inside the projection lane's attempt and inherit its
    retry shape. A crash re-derives them.
- **Ownership.**
  - `audio-graph-0ac0`: implements the route table, M1, M2, and the
    card-record stamp.
  - `audio-graph-f284`: owns the labeled set and the fitting.
  - `audio-graph-ad35`: owns live Jev numbers, which are pending a credential.
  - `audio-graph-fb61` / `audio-graph-4022`: own multilingual distribution.
  - `audio-graph-377a`, `audio-graph-26b4`, `audio-graph-7e3d`: own Metal,
    Vulkan, and DirectML coverage.
- **Non-goals.**
  - Fine-tuning (`f284`). A fine-tuned checkpoint is simply a new pin with a
    new calibration record.
  - Clef (27B) and Clef-omni, which are out per the epic.
  - Any System-1 text generation or entity minting.
  - Cloud routes for the `world` family.
  - Recalibrating vendor HTTP confidence inside the engine.
- **Open questions.**
  - Whether HTTP routes should get a fitted T ≠ 1 once vendor probability
    vectors are proven complete for every question type.
  - How `local_http` pins a mutable Ollama tag. Server digest availability is
    unverified here.
  - Where a decision lives when it has no host artifact. M6 has none today.
  - Whether multilingual Laya ships at all before `4022`.
  - How D3 routes behave on macOS until `377a`: CPU-only, and therefore subject
    to G4.
  - Whether Vulkan-only machines get CPU native routes or none (`26b4`).
- **Reversal cost.** Low for the table: empty settings are the heuristic-only
  state, and dropping a route is additive in reverse. Higher for the
  `DecisionRecord` and calibration-record shapes once persisted (ADR-0027
  migration). Highest for the `decision.*` registry stage, because it flows
  into generated frontend code.
