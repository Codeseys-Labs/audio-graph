# Synthesis + ticket cut — agent chat with threaded auto-answers (epic audio-graph-83cc)

Design-panel judgment, 2026-08-24. Inputs: `angle-a-minimal-surface.md`,
`angle-b-thread-first.md` (both read in full), the round-5 chat-surface
inventory, the ratified agent-runtime verdict, the router-proxy sketch, and
ADRs 0013/0034/0037/0038/0046. Every adjudication fact below was re-verified
in source this pass, read-only. No build, test, or git command was run.

---

## 1. Verdict

**Base: ANGLE A (minimal surface — the card is the thread), with five grafts
from Angle B.** One durable object (`LiveAssistCardRecord` + an additive
`answer` field), two Rust commands over one stateless answer engine, a
composer pinned inside the agent tile, drop-don't-queue spend gating, and
deletion of the unreachable `ChatSidebar`. The grafts: Angle B's backend
signal grade as the spend authority, its anchored-transcript retrieval window,
its composer-renders-in-idle diagnosis, its speak-aloud catch, and its
manual-chat ledger-hole closure.

Three deciding reasons:

1. **Deletion-test survival.** Every component A adds dies visibly if removed
   (A §4's table). B's genuinely new machinery — a third `live_assist/`
   artifact stream (`turns.jsonl` + ceiling + lazy-load command + ADR-0027
   migration + export field + orphan cleanup), a `Queued` dispatcher, a
   second head-state enum — exists to serve multi-turn threads that **both**
   designs defer to b437. B's own weaknesses 3 and 22 concede this: "just
   embed them, the ceiling is 8 MiB is a defensible cheaper answer under a
   20-answer cap," and "the auto-answer half is the severable half." Under
   A's caps (≤12 single-turn answers/session, persisted once at terminal,
   completion-budget-clamped), embedded answers add tens of KB to a file
   whose observed pathological size was 55 KB — three orders of magnitude
   under the 8 MiB ceiling. The cfa1 growth shape (unbounded per-cycle
   re-cloning) is not this write pattern.
2. **Time-to-first-useful-unit.** A reaches the full ratified behaviour in
   four units (schema → engine → composer → trigger); B needs B1+B2+B4 for
   manual-only and B3+B5 besides for auto-answer, plus the new-stream tax on
   the way. The field failure is answers being *lost*, not answers being
   *insufficiently threaded*; A kills it soonest.
3. **Recon accuracy at the one hazardous seam.** Verified this pass:
   `stream_registry.cancel_all()` lives **inside `spawn_stream_task`**
   (`commands.rs:3374,3396`), not in `start_streaming_chat` as B's
   constraint 7 / §9 state. B's plan — "reuse `spawn_stream_task`, not
   `start_streaming_chat`," plus invariant I4 — would therefore NOT avoid
   cancelling a user's in-flight stream by itself; the protection has to be
   the pre-dispatch registry check, which is exactly A's C3 mechanic
   (`StreamRegistry::is_empty()`, verified at `llm/streaming.rs:1091`,
   refuse-don't-cancel). B's design converges on A's mechanic once the
   misattribution is corrected; its separate dispatcher then buys only the
   queueing A rejects (and B's own weakness 6/19 names two cancellation
   policies over one registry as the silent-cancellation risk this epic
   exists to stop). Also verified: `llm/streaming.rs` has zero `route::`
   references, so B's claim of stamping `AttemptedRouteIdentity`
   (`route.rs:494`) on each turn overstates what is reachable without the
   route-seal retrofit both designs defer — A's "resolved route id, honestly
   not a seal" posture is the accurate one.

### Scorecard (criteria from the panel charter)

| Criterion | Angle A | Angle B | Notes |
|---|---|---|---|
| Field-failure coverage (74-piled / answer-lost dies) | **Strong** | Strong | Both make dismiss-then-dump structurally impossible; A gets there in fewer units |
| ADR-0013 write-path boundary | **Clean** (zero new write paths, promotion deferred with zero affordance) | Clean (promotion via unchanged approve path in B9) | Tie on compliance; A ships less to review |
| ADR-0038 routing | Honest partial (resolved route id, gap named + assigned) | Nominally stronger (`route.assist_answer`) but partially decorative — the streaming path consults no route table today | A, on honesty; B's route row without dispatch-through-the-table is a label, not a control |
| ADR-0046 shell | Compliant (no fifth tile, absorbs ChatSidebar) | Compliant | Tie |
| Cost-control honesty | **Strong** (count dispatches not tokens — providers may omit usage, `commands.rs:3319`; Rust caps; drop-not-queue) | Strong on authority (backend grade), weaker on cap honesty (durable token budget vs usage-optional providers) | Graft resolves: A's counting + B's authority |
| Deletion test | **Every component survives** | turns.jsonl/dispatcher/thread_state fail it for v1 | A |
| Dependency honesty (W9/104f/81a5/b437) | Honest; seam stated with loop invariants | Honest; slightly richer b437 seam | Near-tie; A's per-answer-cap seam note is the load-bearing one |
| Time-to-first-useful-unit | **4 units** | 5-6 units + new stream | A |

---

## 2. Grafts from Angle B (accepted, with reasons)

**G1 — Backend signal grade is the spend authority (B3 + I7 + D5).** A's
weakness 5 is real and it is a trust-boundary defect, not a style point: in
A-as-written, the *eligibility* decision for a paid automatic call is
computed in the renderer, and Rust only counts volume. That is the exact
shape the ratified agent-runtime verdict rejected in option D2 ("Rust
validates what the renderer claims"). The fix is cheap because Rust already
mints the proposal (kind + confidence, `speech/mod.rs:1073-1140`): port W9's
shipped numeric rules (confidence ≥ 0.5, ≥4 tokens / ≥16 chars,
dangling-clause — no English word lists) into an
`agent_signal_grade(&AgentProposalPayload) -> SignalGrade`, stamp it on the
payload and the card head (`signal: Option<SignalGrade>`, `#[serde(default)]`),
and have `answer_question_card(auto: true)` check **Rust's own stamped grade
from its own pending map** before spending. The FE `autoAnswerAdmits` trigger
(A4) stays — as the *initiator* and the display filter — so for one release
auto-answer requires Strong-in-Rust AND admitted-by-W9-in-FE; the two gates
can only be conservative together. Known cost, inherited from B's weakness
20: the grade is implemented twice until 104f collapses them; mirrored
fixtures (same inputs asserted equal in `agentQueue.test.ts` and the cargo
test) are the drift guard, and 104f's durable half ("a backend quality
field") is exactly this function — T2 pre-pays planned work rather than
inventing new.

**G2 — Anchored transcript window for detected-question cards (B6, reduced).**
A's weakness 3 (tail-10 answers a 20-minutes-ago question ungroundedly) bites
hardest on **manual** Ask AI against an old card — precisely the affordance
the ratified shape preserves for fragments. Graft the minimal form into
`prepare_card_answer_request`: when the card has `source_span_ids`, the
transcript slice is the window around them (default 6 before / 2 after) from
the same `transcript_ledger_snapshot()` handle the approve path already uses;
when it does not (user-typed question), keep tail-10. Note: for auto-answers
under drop-don't-queue the anchor ≈ the tail anyway (dispatch happens at
detection or never), so this graft is almost entirely a manual-ask upgrade —
which is why the heuristic-risk B admits (weakness 18) is acceptable: the
fallback path still exists and the `CardAnswer` evidence fields record
exactly what was fed either way. B's `retrieval_manifest` is not grafted as a
separate type — A's `CardAnswer.evidence_span_ids` / `evidence_graph_ids` /
`notes_last_sequence` IS that manifest, flattened.

**G3 — The composer renders in every state, including idle (B §4.1).** B's
diagnosis is the sharpest sentence in either artifact: "today's idle branch
returns before any input could exist — that is precisely why the chatbox is
unreachable" (`AgentProposalsPanel.tsx:577-597`). A's A3 implies it; the
synthesis makes it a pinned requirement with a test: the composer exists in
idle, queue-empty, and streaming states, sits outside `AGENT_QUEUE_PANEL_ID`
(so the Signal/All `aria-controls` contract is untouched), and the idle
empty-state renders *above* it, not instead of it.

**G4 — Auto-answers must not speak aloud (B D2).** Verified this pass:
`spawn_stream_task` builds a `SpeakAloudPipe` whenever `settings.speak_aloud`
is on (`commands.rs:3424-3443`). A-as-written would have 12 auto-answers per
session read aloud mid-meeting. The answer engine forces `speak_aloud: false`
for `auto: true` dispatches (and skips the `tts_speak_aloud` policy gate
accordingly); manual asks and typed questions follow the user's setting.
Maintainer confirmation requested as Q6.

**G5 — Close the manual-chat ledger hole (B7's second half).** Verified by
both angles independently: the chat path appends zero data-movement rows
today — the only producers are capture lifecycle and projections. A2 ledgers
the *new* answer path; the pre-existing hole on
`start_streaming_chat`/converse is a distinct ADR-0034 producer-inventory
defect that B correctly refuses to leave open. It lands as its own S-sized
tail unit (T8) rather than bloating the L-sized engine unit.

Also adopted, at zero design cost: B's "the tile must stop presenting
'✓ question added to graph' as the card's only feedback once a thread
exists" (§8.5) — folded into T4's render scope; and B's two-step-absorb
caution is answered by making the sample-preview re-verification a **named
precondition gate** on T7 rather than by keeping the dead file mounted for an
extra release.

## 3. Rejected from Angle B (with reasons)

- **`live_assist/<id>.turns.jsonl` (third artifact stream).** Real,
  recurring cost (ADR-0027 migration, own ceiling, `SessionExportBundle`
  field, orphan cleanup, Route-lens row, lazy-load command, denormalized
  head-preview drift — B's own weakness 21) bought for multi-turn capacity
  v1 explicitly defers to b437. When the b437 loop lands multi-turn threads,
  the append-only stream is the right home **then**, added by that lane with
  `CardAnswer` degrading gracefully to "terminal summary of turn 1" — an
  additive migration, not a rewrite. Rejected now, on the deletion test.
- **`Queued` state + single-slot `AssistAnswerQueue` dispatcher.** Two
  dispatch policies over one `stream_registry` is B's own named hazard
  (weaknesses 6, 19), and the misattributed `cancel_all` location (§1.3)
  shows how easy it is to get this seam wrong. Drop-don't-queue
  (refuse when `!stream_registry.is_empty()`, no backlog, manual button
  always remains) is strictly simpler and honest about being best-effort.
  Recorded as maintainer question Q3.
- **`route.assist_answer` route-table row now.** The streaming chat path
  does not dispatch through `LLM_ROUTES` (zero `route::` refs in
  `llm/streaming.rs`, verified); a route row nothing resolves through is
  provenance theater — worse than A's honest "resolved route id via
  `resolve_route` (`route.rs:291`), seal deferred." The named route + seal
  belong to the agent-runtime lane (router-proxy U6, "per-consumer route
  authorization"), where the dispatch path actually moves onto the table.
- **`thread_state` as a second persisted head enum.** The conversation axis
  is fully derivable from `Option<CardAnswer>` + `CardAnswer.status`
  (absent / streaming-transient / Answered / Failed / Interrupted); a second
  persisted enum adds states that can contradict the first (and B needs a
  transition-table test matrix to police it). A's additive-fields-only shape
  also avoids the C12 enum-widening downgrade hazard entirely.
- **Persisted `NotAuthorized` refusal states on the card head.** Auto-answer
  refusals are ephemeral by design under drop-don't-queue: log line +
  counter chip, no durable write per weak card. A *user-initiated* ask that
  is refused surfaces its typed reason inline (A's `AnswerRefused { reason }`
  + the existing `PRIVACY_POLICY_BLOCKED` event) — legibility where a human
  is watching, no artifact churn where none is.
- **Durable per-session token budget.** `persist_llm_usage_for_session`
  early-returns when the provider omits usage (`commands.rs:3319-3321`), so
  a token budget is unenforceable against exactly the providers most likely
  to blow it. Dispatch counting is exact and honest; tokens stay
  display-only (`SessionUsage`). B's own default was `None` anyway.
- **`DETAIL_LENSES += "assist"` as a new lens.** A's re-point of the
  existing Ask aside at a read-only `SessionThreads` view is the smaller
  change with the same outcome (real Q&A with evidence instead of an empty
  chat with a disabled input); no lens-id churn in e2e/contract tests.

### Factual corrections recorded (for reviewers of either artifact)

1. `cancel_all()` is inside `spawn_stream_task` (`commands.rs:3396`), not
   `start_streaming_chat`. Angle B constraint 7 / §9 / I4-as-specified are
   wrong on attribution; the pre-dispatch `is_empty()` refusal is the actual
   protection, and T3's regression test pins it ("registry non-empty ⇒ `Err`
   **and no `cancel_all`**").
2. `AttemptedRouteIdentity` exists (`route.rs:494`) but nothing on the
   streaming path constructs one; any design claiming served-route
   provenance on chat answers today is claiming unshipped plumbing.
3. Both angles agree, and it is confirmed at `commands.rs:7972-7990`: the
   8 MiB live-assist ceiling degrades to an **empty vec** with a WARN —
   total silent loss of all cards in review, not truncation. T1 carries a
   size-margin test (worst-case cap × max answer length ≪ ceiling) and the
   ceiling's failure mode is named in the unit so nobody "fixes" it by
   raising the constant.

---

## 4. The unified design (deltas against Angle A, which stands as the base document)

Read `angle-a-minimal-surface.md` §2 as the design of record, amended:

1. **Schema (T1).** `LiveAssistCardRecord` gains `origin: Option<CardOrigin>`,
   `answer: Option<CardAnswer>`, **and `signal: Option<SignalGrade>`** — all
   additive `#[serde(default)]` fields, no new enum variants on `status`.
2. **Spend gate order (T3), cheapest-first, all in Rust:** enabled →
   `kind == Question` → **Rust-stamped `signal == Strong`** → not
   converse mode → `stream_registry.is_empty()` → 45 s interval →
   12/session cap. First refusal wins, typed. FE admit (`autoAnswerAdmits`)
   is additionally required at the trigger for one release (Q2).
3. **Retrieval (T3):** anchored window (±6/±2 around `source_span_ids`) when
   anchors exist, tail-10 otherwise; graph top-40 unchanged; notes outline
   headings (≤ ~1,500 chars) via `state.rs:570` — closing the verified notes
   gap. Evidence recorded on `CardAnswer` as the flattened manifest.
4. **TTS:** `auto: true` ⇒ `speak_aloud` forced off. Manual/typed follows
   settings.
5. **Composer:** rendered in all panel states including idle; outside
   `AGENT_QUEUE_PANEL_ID`; test-pinned.
6. **b437 seam (unchanged from A §7, restated as contract):** the loop
   replaces `prepare_card_answer_request` + `spawn_stream_task` behind the
   same command signature/channel/record/gate; the cap becomes **per-answer,
   not per-provider-call**; one `actor:"system"` ledger row per model call;
   route id stamped by trusted code; no graph write without the approve
   path; when multi-turn lands, turns move to an append-only stream and
   `CardAnswer` becomes the terminal summary.

ADR-0013/0037 posture is A's verbatim: the answer is a Grounded Inference on
a Session Artifact carrying its Inference Chain; zero new graph write paths;
user-typed questions mint no `Question` node (Q5); promotion deferred with
zero v1 affordance (Q8).

---

## 5. TICKET CUT — ordered, landable units

Project memory holds: **max one Rust-compiling lane at a time** on this box.
T1→T2→T3 are one serialized Rust lane. T4 is FE and may run beside them using
individual frontend scripts only (never `verify:fast`). T5/T6 are FE after
T3. T7 carries a small Rust tail; T8 is Rust — both serialize behind T3.

Ratified promises, for the delivery column: **P1** free-form chatbox in the
agent tile · **P2** Signal-bar questions auto-answered · **P3** answer
threaded under the card · **P4** fragments keep manual Ask AI · **P5** the
field failure dies (no dismiss-then-dump; answers durable and visible).

### T1 — Card answer schema + persistence (Rust) — **S/M** — delivers the substrate for P3/P5
- **Scope:** `CardOrigin`, `CardAnswer`, `CardAnswerStatus`, `SignalGrade`;
  additive `origin`/`answer`/`signal` fields on `LiveAssistCardRecord`;
  validator rules (answer text non-empty unless Failed; route_id non-empty;
  the narrow `origin: User` citation relaxation per Q4's outcome);
  `live_assist_card_record` helper carries fields through; TS mirror types.
- **Files:** `src-tauri/src/events.rs`, `src-tauri/src/persistence/mod.rs`
  (`validate_live_assist_card` :335), `src-tauri/src/commands.rs:3971-4029`,
  `src/types/index.ts`.
- **Gates:** cargo fmt/clippy/test — old-shape JSON round-trips; upsert→load
  preserves the answer; validator rejects each bad shape; **size-margin
  test:** 12 max-length answers + 200 cards ≪ `MAX_LIVE_ASSIST_CARDS_BYTES`.
  `bun run tsc --noEmit`.
- **Deps:** none. **Rust lane.**

### T2 — Backend signal grade at mint (Rust) — **S** — delivers the spend authority for P2 (graft G1)
- **Scope:** `agent_signal_grade` next to `agent_proposal_kind`, porting
  W9's shipped constants exactly (0.5 / 4 tokens / 16 chars /
  dangling-clause, no English word lists); stamped on
  `AgentProposalPayload` and the card head at mint; first log lines this
  subsystem has ever had (content-free, `text_len=` house style).
- **Files:** `src-tauri/src/speech/mod.rs` (:1073-1140), `events.rs`.
- **Gates:** cargo tests mirroring `agentQueue.test.ts` fixtures 1:1 (same
  inputs, both locales, asserted equal classification — the FE/BE drift
  guard); no transcript content in logs.
- **Deps:** T1 (field placement). **Rust lane.** 104f note: this function
  IS 104f's designated durable slot ("backend quality field"); when 104f
  sharpens the heuristics it edits this one body and the mirrored fixtures.

### T3 — Answer engine + spend gate + ledger + telemetry (Rust) — **L** — delivers P2/P3/P4 backend, kills P5's dump
- **Scope:** `prepare_card_answer_request` (stateless, never touches
  `chat_history`; anchored window per G2 + graph top-40 + notes headings);
  `answer_question_card` + `ask_question_card` commands; terminal-frame hook
  → build `CardAnswer` → `upsert_live_assist_card` **once** → ledger row →
  `AGENT_CARD_UPDATE` event; the ordered spend gate (§4.2) with typed
  `AnswerRefused`; Rust-grade check from the pending map; `speak_aloud`
  forced off for auto (G4); `AgentAutoAnswerSettings` (`#[serde(default)]`,
  round-trip-identical); counters on `AppState` reset on session rotation;
  distinct egress action ids `llm_agent_answer` / `llm_agent_auto_answer`
  with data classes `[user_message, transcript, graph_context, notes,
  prompt]`; `resolve_route` id stamped; dispatch/refusal log lines.
- **Files:** `src-tauri/src/commands.rs` (new block near :3170-3700),
  `state.rs`, `settings/mod.rs`, `events.rs`, `lib.rs` (register 2
  commands), `src/types/index.ts` + `src/hooks/useTauriEvents.ts` (wiring).
- **Gates:** cargo fmt/clippy/test — gate-off/cap/interval/converse/
  policy-blocked ⇒ `Err` + zero provider calls; registry non-empty ⇒ `Err`
  **and no `cancel_all`** (the §1.3 regression); Weak/Fragment Rust grade ⇒
  `Err` even when the command claims auto-eligibility (renderer-untrusted
  test); exactly one upsert per answered card; ledger rows 1:1 with
  dispatches (`actor: system|auto` vs `user`); `chat_history` unchanged
  after an auto-answer; auto dispatch builds no `SpeakAloudPipe`; anchored
  window resolves every recorded span id. `bun run verify:contracts`
  (expected no diff — ledger vocabulary reused).
- **Deps:** T1, T2. **Rust lane.** Review as a **security change** (spend
  boundary), not plumbing. Named split point if the lane runs long: T3b =
  notes block + anchored window (ship tail-10 first).

### T4 — Composer + thread rendering in the agent tile (FE) — **M** — delivers P1 + P3's pixels
- **Scope:** `AgentComposer` pinned row, rendered in **all** states
  including idle (G3, test-pinned), outside `AGENT_QUEUE_PANEL_ID`;
  `AnswerThread` leaf (streaming dots / answer text / failure + Retry /
  evidence + route chip) in queue rows and inside the feed row's existing
  details disclosure; `answerDrafts` transient slice + extracted delta
  coalescer shared with `sendChatMessage`; `askQuestion` /
  `answerQuestionCard` store actions; thread state replaces "✓ question
  added to graph" as the card's primary feedback; i18n add-only en+pt.
- **Files:** `src/components/AgentProposalsPanel.tsx`, new
  `src/components/workspace/AgentComposer.tsx`, `src/store/index.ts`, new
  `src/store/chatStream.ts`, `src/i18n/locales/{en,pt}.json`, tests.
- **Gates:** `bun run check`; `tsc --noEmit`; `test:coverage` — send ⇒ one
  `ask_question_card` invoke; deltas render into the target card only;
  composer exists in the idle fixture; Signal/All roving-tabindex + frozen
  `WorkspaceTileId` tests untouched; a11y (labelled composer, debounced
  status region, keyboard-only, 200% zoom == compact tier). **FE lane —
  may run beside T2/T3 via individual scripts; buildable against stubs.**
- **Deps:** T1 types; T3 for real commands.

### T5 — Auto-answer trigger + Signal parity (FE) — **S/M** — delivers P2's initiation + the queue-drain half of P5
- **Scope:** `autoAnswerAdmits` (pure, calls `selectAgentQueue` itself so
  the rules cannot drift from W9); hooked into `addAgentProposal`
  (store/index.ts:1800) — the single funnel, never a render path or effect;
  converse exclusion; the 3-line `classifyQueueEntry` rule "terminal
  non-Failed answer ⇒ info ⇒ feed, Failed stays actionable"; counter chip
  (`3/12`) in the composer row.
- **Files:** `src/components/workspace/agentQueue.ts`, `src/store/index.ts`,
  `src/components/AgentProposalsPanel.tsx`, tests.
- **Gates:** `test:coverage` — fragment-suspect fixture dispatches nothing;
  admitted fixture dispatches exactly once under duplicate `addAgentProposal`
  calls (upsert semantics); duplicate-collapse victim dispatches nothing;
  answered card lands in feed in both Signal and All modes; Failed stays in
  queue.
- **Deps:** T3, T4.

### T6 — Settings control + budget legibility (FE) — **S** — delivers the off switch (P2's honesty)
- **Scope:** auto-answer toggle + cap + interval in Settings LLM section via
  existing `saveSettings` path; refusal reason surfaced inline for
  user-initiated asks (reuse `PRIVACY_POLICY_BLOCKED`); settings round-trip
  test.
- **Files:** Settings panel + controller, `AgentComposer`, i18n, tests.
- **Deps:** T3 (settings shape), T4.

### T7 — Surface consolidation: delete `ChatSidebar`, re-point the Ask lens (FE + small Rust tail) — **M** — completes P5
- **Precondition gate (hard):** re-verify the sample-preview path — the one
  place `historicalReview` can be false while `SessionDetail` renders — and
  record the repro result in the PR before deletion (A weakness 9).
- **Scope:** delete `src/components/ChatSidebar.tsx` (+ test); new read-only
  `SessionThreads` over `liveAssistCards` rendered by the Sessions Ask
  aside (keep `askAvailable` notes/graph gating; honest "asking a finalized
  session is not delivered" copy); drop the `chatMessages` push in
  `approveAgentProposal` (store:1884) and the `chat_history` push in
  `approve_agent_proposal_impl` (commands.rs:4302-4312) — two writes to a
  surface nobody renders.
- **Gates:** `bun run check`; `tsc`; `test:coverage`; `bun run build`;
  e2e Sessions-detail × lens at 1440/1024/768; grep gate: zero `ChatSidebar`
  references. Converse regression note: converse only runs during capture,
  where `reviewLocked` already blanks `SessionDetail` — nothing reachable is
  lost; the converse-text-rendering gap gets its own seed, not this epic.
- **Deps:** T4. Rust tail serializes behind T3 (or folds into T3 if lanes
  collide).

### T8 — Data-movement ledger rows for the legacy chat path (Rust) — **S** — ADR-0034 debt closure (graft G5)
- **Scope:** `provider_call_started` + terminal row on
  `start_streaming_chat` / `send_chat_message` dispatches (converse
  included), same vocabulary T3 uses; register the producer in ADR-0034's
  inventory doc.
- **Files:** `src-tauri/src/commands.rs`, `docs/adr/0034-*` producer list.
- **Gates:** rows 1:1 with provider calls under a multi-turn converse test;
  Route lens renders them; no contract regeneration.
- **Deps:** T3 (shared helpers). **Rust lane.**

**Shortest path to the full ratified behaviour: T1 → T2 → T3 → T4 → T5.**
T6/T7/T8 are the honesty-and-consolidation tail. Severability, stated
plainly: T1+T3(manual only)+T4 already kill the field failure; T2+T5 are the
auto-answer half and are the severable half if the epic is cut for time.

---

## 6. OPEN MAINTAINER DECISIONS

Crisp either/or, with recommendation. Q1-Q4 block T3's final shape; the rest
can trail.

- **Q1 — Auto-answer default: ON with caps (12/session, 45 s interval,
  visible counter + off switch) or OFF for the first field build?**
  Recommend **ON** — off-by-default makes the ratified feature invisible in
  the field, the exact round-5 failure mode; the caps bound the worst case
  at 12 paid calls.
- **Q2 — Spend eligibility: Rust grade alone authoritative, or Rust grade
  AND the FE Signal admit both required for one release?** Recommend
  **both** — belt-and-braces costs one boolean; disagreeing gates that both
  spend money cost a field incident. Collapse to Rust-alone when 104f lands.
- **Q3 — When a stream is already in flight: drop the auto-answer
  (best-effort, never a backlog) or queue it (single-slot dispatcher)?**
  Recommend **drop** — a queue is a second cancellation policy over one
  registry and an invisible backlog; the manual button remains on every
  unanswered card.
- **Q4 — A user-typed question before any speech exists: widen
  `validate_live_assist_card` narrowly (empty `source_segment_id` permitted
  only when `origin == User`, test-pinned) or refuse the ask until the first
  span exists?** Recommend **widen narrowly** — the alternative is honest
  but a bad product; note this touches the ADR-0037 layering-precedent rule
  and should be reviewed as a contract change.
- **Q5 — Does a user-typed question also mint a `Question` node in the
  graph (as transcript-detected questions do today)?** Recommend **no** — a
  query about the meeting is not meeting content.
- **Q6 — Speak-aloud: auto-answers never speak (forced off), manual/typed
  asks follow the user's setting — confirm?** Recommend **yes** — an
  automatic TTS burst mid-meeting is a product judgment nobody has made;
  the seam stays wired.
- **Q7 — Fate of the Sessions "Ask" lens: read-only thread viewer over the
  session's answered cards, or keep the disabled-input chat?** Recommend
  **read-only viewer** — an upgrade from an empty log with a dead input;
  asking questions *of a finalized session* is real retrieval work, deferred.
- **Q8 — Promoting an answer into the graph: defer entirely (zero v1
  affordance) or ship a promote-to-Note-proposal button through the
  unchanged approve path?** Recommend **defer** — it needs an ADR-0037
  claim class and per-item evidence; shipping zero affordance means zero
  boundary surface to review.
- **Q9 — Does the `--features cloud` light build ship the composer?**
  Recommend **yes** — zero size cost, same route table; fixes the open
  feature-matrix question from the agent-runtime synthesis (§6.5).
- **Q10 — Thread real estate: in-tile only for v1, with expand-to-overlay
  as a phase-2 ticket?** Recommend **in-tile** — the quarter-bento at
  768 px / 200 % zoom is tight (A weakness 7), but an overlay is a new
  surface class that deserves its own design pass.

---

## 7. Dependency honesty (consolidated)

- **W9** is shipped and is both the FE display filter and (via T2's 1:1
  fixture mirror) the source of the Rust grade's constants. Nothing invents
  new quality rules.
- **104f** does NOT block the chatbox (T1/T3/T4 land regardless of grade
  quality); its durable half lands *in* T2's function body. Interaction risk
  while heuristics are wrong: fragments auto-answered up to the cap — bounded
  by Q1's caps and off switch.
- **81a5** lands in parallel; T2/T3's log lines are this subsystem's first
  telemetry ever (round-5 S9) and use house style so 81a5 can fold them into
  counters; no shared file beyond `commands.rs`.
- **b437 (U1-U7)** is a seam, not a dependency: same command signature,
  channel, record, and gate; cap becomes per-answer; turns stream added by
  that lane when multi-turn lands. Nothing built here is unbuilt then.
- **One-Rust-lane memory** is encoded in the ordering; T4 is the only unit
  cleared to run beside a Rust lane, via individual frontend scripts.
