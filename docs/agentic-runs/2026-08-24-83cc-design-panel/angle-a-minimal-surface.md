# Angle A — Minimal surface, ship fast (epic audio-graph-83cc, agent chat + threaded auto-answers)

Design-panel artifact, 2026-08-24. Read-only recon against the working tree at
this checkout; every file:line below was opened this pass. No build, test, or
git command was run.

**Thesis in one sentence:** the app already owns a durable, per-session,
evidence-validated, `load_session`-round-tripping record for exactly this kind
of object — `LiveAssistCardRecord` in `live_assist/<session>.current.json` —
so v1 should make **the card the thread**, add one optional `answer` field to
it, add one Rust command that fills that field through the existing streaming
chat machinery, and delete the unreachable second chat surface. No new
persistence subsystem, no new store slice for history, no agent loop, no
`chat_history` durability work, and one new frontend component (a composer
row) that survives the deletion test.

---

## 1. Constraints I hold (verified this pass, not inherited)

**Ratified and not relitigated.** Agent tile gains a free-form chatbox;
detected questions that pass the Signal quality bar are auto-answered with the
answer **threaded under the card**; fragments keep manual Ask AI. Approvals
stay explicit (2026-08-23 live-workspace constraints §"Ratified" item 4);
auto-**apply** remains deferred and unratified — auto-**answer** is a different
thing and this design keeps them separate.

**Field failure to fix.** `askAgentProposal` (`src/store/index.ts:1828-1841`)
strips the canned prefix, **dismisses the card**, then calls the shared
`sendChatMessage`. The answer lands in `chatMessages`, which nothing in the
capture workspace renders, and the card the user wanted to discuss is gone.
74 pending cards in 59 minutes while chat was touched twice.

**C1 — The chat surface that exists is unreachable, and that is structural.**
`ChatSidebar` mounts in exactly one place (`SessionsBrowser.tsx:825-828`),
behind `askAvailable(lens)` (notes/graph only, `:223-227`) and a local
`askOpen` (`:662`), inside a `SessionDetail` that renders a lockout screen
whenever `reviewLocked` (driven by `isCapturing`/`isTranscribing`, ADR-0046
"concurrent Live+Review is not delivered"). And its input is
`disabled={... || historicalReview}` where `historicalReview =
loadedSessionId !== null` (`ChatSidebar.tsx:34,52,192,199`) — which
`loadSession` sets before any content renders. Live: no input exists. Review:
input exists, disabled on arrival.

**C2 — `WorkspaceTileId` is a frozen union** — `"transcript" | "graph" |
"document" | "agent"` (`workspace/WorkspaceTile.tsx:14`), pinned by
`WorkspaceTile.test.tsx` and reserved by the phase-2 `ag.workspaceLayout`
persistence key. No fifth tile. Chat lives *inside* the `agent` tile. Each
tile has exactly **one** `headerSlot` (`:38-49`), already double-occupied on
the agent tile by `AgentQueueFilterToggle` + `AgentTileHeaderActions`
(`App.tsx:661-678`). New controls go in the body, not the header.

**C3 — There is exactly one active chat stream, by design.**
`spawn_stream_task` calls `state.stream_registry.cancel_all()` before
registering (`commands.rs:3374-3405`, AUD-STR1 P1). A naive auto-answer would
therefore *cancel the user's in-flight answer*, and N auto-answers would
mutually cancel each other. `StreamRegistry::is_empty()` already exists
(`llm/streaming.rs:1091`) and is the cheap way to refuse instead of cancel.

**C4 — `chat_history` is one shared 200-message buffer, and converse mode is a
live consumer.** `state.rs:239`, capped by `cap_chat_history`
(`commands.rs:3265-3273`), cloned whole into every request by
`prepare_chat_request` (`commands.rs:3178-3240`). `useConverseFrontLeg.ts`
(mounted at `App.tsx:724`, ADR-0013 step 2) drives STT-final →
`sendChatMessage` and uses `isChatLoading` / `streamingChatRequestId` as its
re-entrance and echo guard (`:166-176`). So `chatMessages`, `isChatLoading`,
`sendChatMessage` are **not deletable**, and auto-answers must not be pushed
into `chat_history` — 74 auto-answers would poison every subsequent converse
turn's prompt and eat the context budget. `spawn_stream_task` already takes
`persist_to_history: bool` (`commands.rs:3383,3501`), so opting out is a
parameter, not a refactor.

**C5 — The durable card store already exists and already round-trips.**
`LiveAssistCardRecord` (`events.rs:467-491`) carries `proposal`, `status`,
`source_span_ids`, `graph_context_ids`, `outcome`, `projection_patch_sequence`.
`upsert_live_assist_card` appends the full record to an audit `.jsonl` and
rewrites `live_assist/<id>.current.json` (`persistence/mod.rs:1442-1471`);
`load_session` returns `live_assist_cards` under an 8 MB ceiling
(`commands.rs:141`, `:7972-8007`); the store hydrates it at
`src/store/index.ts:3432`. `validate_live_assist_card`
(`persistence/mod.rs:335-373`) enforces the CONTEXT.md Evidence-Annotation
invariant: a card must cite transcript spans **or** graph context.

**C6 — Pending cards are NOT durable today.** Only approve / dismiss / clear
write records (`commands.rs:4332-4341`, `:4535-4544`, `:4574-4583`).
`add_question_to_graph_impl` (`:4364-4426`) writes the graph and no card. So
persisting an *answered but unresolved* card at `Pending` is new behaviour —
supported by the validator, and it degrades correctly on reload (a loaded
pending card is classified `"info"` by `selectAgentQueue` because
`actionableProposalIds` comes from the live `agentProposals` array).

**C7 — The Signal bar lives in the frontend.** `classifyQueueEntry`
(`workspace/agentQueue.ts:219-249`), `admitToQueue` (`:267-269`), the
duplicate-collapse pass inside `selectAgentQueue` (`:345-403`), thresholds at
`:84-86`. `selectAgentQueue` is a pure, zero-store-state selector. W9 shipped
it; 104f sharpens it; no backend quality field exists.

**C8 — Proposals are free; answers are the new spend.**
`run_agent_proposal_task` (`speech/mod.rs:1073-1140`) mints proposals from a
local heuristic (`agent_proposal_kind`), no LLM, capped at 200 pending
(`:85`, pruned oldest-first at `:1024-1038`). Every auto-answer is a paid
provider call that no human clicked.

**C9 — The chat path is gated but NOT route-authorized.** `start_streaming_chat`
(`commands.rs:3590-3655`) runs `enforce_chat_provider_start` (ADR-0033,
`:1055`) and `enforce_session_content_policy` (`:841`) with action `"llm_chat"`
and data classes `["user_message","transcript","graph_context","prompt"]`.
It does **not** touch `authorize_route_dispatch` / `AuthorizedRoute`
(`llm/route.rs:386,568`) — `grep` for `route::` in `llm/streaming.rs` returns
zero non-test hits. Only the projection executor is ADR-0038-sealed
(`llm/executor.rs:526-566`). Likewise only the projection path writes ledger
rows (`projection_data_movement.rs:263-280`); chat writes none.

**C10 — Tool calling does not exist.** `terminal_status_from_finish_reason`
maps `"tool_calls"` → `TerminalStatus::Failed` with the comment "No request in
this repository sends tools" (`llm/route.rs:641-649`). v1 cannot use tools.

**C11 — Retrieval context that exists today.** `prepare_chat_request` builds
`build_graph_chat_context(snapshot, message, 40)` plus the last 10 transcript
segments (`commands.rs:3186-3218`). Notes are **not** included. A cheap fix
exists: `AppState::materialized_notes_snapshot_for_session`
(`state.rs:570-573`) is a clone-and-release accessor, and notes now carry
`heading_level` (626c, `projections.rs:1650`).

**C12 — Enum widening is a downgrade hazard.** `LiveAssistCardStatus`
(`events.rs:460-465`) has no unknown-variant fallback, so adding an
`Answered` variant makes an older build's `load_live_assist_cards` **fail the
whole file**. Additive *fields* are safe (serde ignores unknown fields on
read); additive *variants* are not. This design adds fields only.

---

## 2. The design

### 2.1 One model: the card is the thread

A question and its answer are one durable object.

```rust
// events.rs — additive fields only (C12)
pub struct LiveAssistCardRecord {
    ...existing...
    /// Who authored the question. Absent ⇒ "agent" (every pre-83cc record).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<CardOrigin>,          // Agent | User
    /// The threaded answer. Written ONCE, on the stream's terminal frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<CardAnswer>,
}

pub struct CardAnswer {
    pub status: CardAnswerStatus,            // Answered | Failed | Interrupted
    pub text: String,                        // full_text from the Done frame
    /// Grounded Inference provenance (CONTEXT.md): what fed the answer.
    pub evidence_span_ids: Vec<String>,      // transcript-window span ids
    pub evidence_graph_ids: Vec<String>,     // build_graph_chat_context node ids
    pub notes_last_sequence: Option<u64>,    // notes basis, when notes were used
    /// ADR-0038: route id resolved by trusted code, never model- or config-echoed.
    pub route_id: String,                    // e.g. "route.openrouter"
    pub requested_by: CardOrigin,            // Agent (auto) | User (manual/typed)
    pub finish_reason: Option<String>,
    pub total_tokens: Option<u64>,           // None when the provider omitted usage
    pub answered_at_ms: u64,
}
```

Why this and not a new store: it is *already* durable, already
per-session, already evidence-validated, already loaded by `load_session`,
already rendered read-only in review, already size-capped. The alternative
(persisting `chatMessages`) means a new artifact class, a new ceiling, a new
`LoadedSession` field, a new migration — for a strictly weaker object with no
identity, no provenance, and no link to the card it came from
(`types/index.ts:2666-2669`: `ChatMessage` has no id, no timestamp, no status).

**A free-form chatbox question mints a card too.** `origin: User`,
`confidence: 1.0`, `kind: Question`, `source_segment_id` = the newest span in
the ledger at ask time (honest: "asked at this point in the conversation"),
`source_span_ids` = the transcript-window ids that fed retrieval — which also
satisfies `validate_live_assist_card`'s citation invariant without relaxing it.
Consequence: one data model, one render path, one persistence store, one
lifecycle, and the chatbox is durable on day one for free.

**What this deliberately does not do:** it does not make `chat_history`
durable. That buffer stays what its doc comment says it is — "a UX
convenience, not a correctness invariant" (`commands.rs:3246`) — serving
converse mode and the legacy blocking path. **Durability decision: answered
question cards are the durable chat record; `chatMessages` stays ephemeral and
is never written to disk.**

### 2.2 Surfaces

**Agent tile body becomes three regions** (the shape `ChatSidebar.tsx:71-207`
already proves works): scrollable queue+feed (unchanged markup), then a fixed
composer row pinned to the bottom with `border-t shrink-0`. This is internal
to the tile's own `children`; it does not touch `WorkspaceTile`'s contract
(C2), and the tile already owns its own scroll region
(`WorkspaceTile.tsx:69-71`).

**Threads render inside the card row, not in a separate log.** An answer
renders under its own card:
- queue row (`AgentProposalsPanel.tsx:328-402`) — a `<AnswerThread>` block
  between the body paragraph and the action row while streaming or failed;
- feed row (`:437-504`) — the answer goes inside the **existing** per-row
  details disclosure, so a resolved thread costs almost no new markup.

**No standalone message log in the agent tile.** The composer's own turns
appear as cards in the same queue/feed lists. That is the single biggest
component-count saving in this design, and it is why the tile does not need a
fourth scroll region.

**`ChatSidebar.tsx` is deleted; the Sessions "Ask" lens is re-pointed** at a
read-only `SessionThreads` view over `liveAssistCards` (already hydrated at
`store/index.ts:3432`). This *upgrades* the lens: today it shows an empty
in-memory chat with a disabled input; after this it shows the session's real
Q&A with evidence and route provenance. **Consolidation verdict: the agent-tile
chatbox ABSORBS `ChatSidebar`. Two chat surfaces are not defensible when one is
unreachable in every real path (C1).**

**Verified no-regression for converse mode:** converse only runs during
capture, and during capture `reviewLocked` already replaces the entire
`SessionDetail` (and therefore `ChatSidebar`) with a lockout screen. Converse
reply *text* is already unrenderable today; deleting `ChatSidebar` loses
nothing currently reachable. Converse remains audio-only via speak-aloud.
That pre-existing gap is not 83cc's to fix; it deserves its own seed.

**One deletion the consolidation unit must include:** `approveAgentProposal`
pushes its outcome message into `chatMessages` (`store/index.ts:1884`) and
`approve_agent_proposal_impl` pushes the same text into `chat_history`
(`commands.rs:4302-4312`). Both write to a surface nobody renders; the outcome
already renders in the feed row via `agent.outcome`. Drop both.

### 2.3 Backend commands and events

Two new commands, both thin wrappers over one internal answer engine.

```rust
/// Answer an existing question card. proposal_id threads the answer;
/// `question` is renderer-supplied text (untrusted input to the gate, same
/// posture as today's chat message).
#[tauri::command] pub async fn answer_question_card(
    proposal_id: String, question: String, auto: bool,
    channel: tauri::ipc::Channel<ChatStreamEvent>,
    app: AppHandle, state: State<'_, AppState>,
) -> AppResult<AnswerDispatch>          // { request_id } | Err(AnswerRefused)

/// Mint a user-authored question card and answer it in one call.
#[tauri::command] pub async fn ask_question_card(
    text: String,
    channel: tauri::ipc::Channel<ChatStreamEvent>,
    app: AppHandle, state: State<'_, AppState>,
) -> AppResult<AskDispatch>             // { record, request_id }
```

Shared body, in order:

1. `read_settings_for_session_content` + `enforce_chat_provider_start`
   (unchanged, `commands.rs:1055`).
2. `enforce_session_content_policy` with a **distinct action id** —
   `"llm_agent_answer"` for `auto: false`, `"llm_agent_auto_answer"` for
   `auto: true` — and data classes
   `["user_message","transcript","graph_context","notes","prompt"]`. Distinct
   ids are the ADR-0034 honesty move: an automatic egress producer must be
   distinguishable in the policy surface and the ledger from a click.
3. **The spend gate** (auto only; §2.4).
4. Route resolution: `resolve_route(&settings.llm_provider)`
   (`llm/route.rs:291`) → `route_id`, stamped by trusted code onto
   `CardAnswer.route_id`.
5. `prepare_card_answer_request(state, &question)` — a **stateless sibling** of
   `prepare_chat_request` that returns `(messages, graph_context, evidence)`
   and **never touches `chat_history`**: one system context block + the one
   question. No history ⇒ no cross-contamination (C4), a smaller prompt, and a
   reproducible-from-evidence answer.
6. `spawn_stream_task(..., persist_to_history: false)` — the existing task,
   the existing `Channel<ChatStreamEvent>`, the existing coalescer, the
   existing cancellation.
7. **On the terminal frame** (a new callback on the task, or a wrapper that
   observes the `Done` frame): build `CardAnswer`, `upsert_live_assist_card`
   **once**, append the ledger row, emit `AGENT_CARD_UPDATE`.

**Persist exactly once, on terminal.** `upsert_live_assist_card` appends the
whole record to the audit `.jsonl` every call (`persistence/mod.rs:1448-1452`),
so a per-delta upsert would write a growing copy of the answer 20-100×/sec.
This is a hard rule, and a test pins it.

**One new event:** `AGENT_CARD_UPDATE` (`events.rs`), payload
`LiveAssistCardRecord`. The frontend already has an unused-in-production
`upsertLiveAssistCard` store action (`store/index.ts:1774-1799`) that consumes
exactly this shape — the event costs one name and one line in
`useTauriEvents.ts`.

**Ledger rows** via `append_data_movement_event`
(`persistence/mod.rs:1373-1387`): `provider_call_started` before dispatch and
one terminal `provider_call_succeeded|failed|cancelled`, with
`actor: "system"` for auto and `actor: "user"` for manual — both vocabularies
already exist in the generated contract (`src/generated/sessionDataMovement.ts:8`),
so **no contract regeneration and no `verify:contracts` churn**.

**Telemetry (S9 precondition, 81a5-adjacent):** one `log::info!` per dispatch
and per refusal, content-free, house style —
`agent_answer dispatch=auto|manual card_id=… route=route.x session_auto_count=3/12 elapsed_ms=…`
and `agent_answer skipped reason=busy|capped|interval|disabled|converse|policy`.
This is the first backend observability the question subsystem has ever had
(field round 5 §2/S9: zero `question`/`chat` hits in 18k log lines).

### 2.4 Routing and cost: the gate policy

Auto-answer is new spend that no human clicked. Five controls, all enforced in
Rust because Rust is where the spend authority lives; the frontend only mirrors
them for legibility.

1. **Eligibility — which quality classes auto-answer.** Only
   `kind === "question"`, and only if the card is admitted to the queue by the
   **existing Signal predicate** — not a second copy of the rules. The
   frontend exports one pure helper:

   ```ts
   // workspace/agentQueue.ts (additive)
   export function autoAnswerAdmits(
     proposal: AgentProposalEvent,
     cards: LiveAssistCardRecord[], proposals: AgentProposalEvent[],
   ): boolean {
     if (proposal.kind !== "question") return false;
     const { queue, fragmentSuspectIds } = selectAgentQueue(cards, proposals); // Signal mode
     return queue.some(c => c.proposal.id === proposal.id)
         && !fragmentSuspectIds.has(proposal.id);
   }
   ```

   Gate == Signal bar by construction: the same fixture that puts a card in the
   feed proves no dispatch. Fragments and `note`/`graph_suggestion` keep manual
   Ask AI, exactly as ratified.
2. **Single-flight, drop-don't-queue.** Refuse when
   `!state.stream_registry.is_empty()` (`llm/streaming.rs:1091`) rather than
   cancelling (C3). Auto-answer is **best-effort and never a backlog**: no
   invisible queue, no stampede, and the manual Ask AI button remains on the
   card. A user-typed question is never dropped — it takes the normal
   cancel-priors path, and an auto-answer it interrupts is recorded
   `Interrupted` and stays manually askable.
3. **Rate limit + per-session cap, counted in Rust.** `min_interval` (default
   45 s) and `max_per_session` (default 12), counters on `AppState`, reset on
   session rotation next to the existing reset block. **Count dispatches, not
   tokens** — `persist_llm_usage_for_session` early-returns when the provider
   reports no usage (`commands.rs:3319-3321`), and `append_llm_chat_usage` only
   records non-zero totals (`sessions/usage.rs:373`), so a token budget is
   unenforceable against providers that omit usage. A count cap is honest and
   exact. Tokens are still *reported* (`SessionUsage.llm_total/llm_turns`) for
   display.
4. **Off switch.** `AppSettings.agent_auto_answer: AgentAutoAnswerSettings
   { enabled, max_per_session, min_interval_secs }`, `#[serde(default)]` +
   `skip_serializing_if` so old settings files round-trip byte-identically
   (the house pattern, `settings/mod.rs:1244-1312`). Recommended default:
   `enabled: true`, cap 12, interval 45 s — bounded worst case, legible, and
   what the ratified shape asks for. Flagged as open decision D1.
5. **Mode exclusion.** No auto-answer while
   `conversationMode === "converse"` — converse *is* the user talking to the
   graph; auto-answering their spoken questions duplicates the reply and
   speak-aloud would read both. One condition, and it protects
   `useConverseFrontLeg`'s busy guard from auto-answer contention.

**ADR-0038 posture, stated honestly.** v1 stamps the **resolved** route id from
`resolve_route`, the same resolver `authorize_route_dispatch` uses, into
`CardAnswer.route_id`. It does **not** obtain an `AuthorizedRoute` seal,
because no chat path does (C9) and retrofitting the seal onto
`llm/streaming.rs` is its own Rust lane touching five provider skins. So
auto-answer is a new automatic producer on the app's least-ADR-0038-sealed LLM
path. That is a real gap; it is named, bounded (count cap + ledger row +
distinct action id), and assigned to the agent-runtime lane rather than
smuggled.

### 2.5 ADR-0013 write-path boundary: zero new write paths

**An answer never mutates the graph.** The only graph writes reachable from a
question card stay exactly what they are today:
`add_question_to_graph` (local, no LLM, fires at card creation,
`commands.rs:4350-4426`) and `approve_agent_proposal`
(`:4154-4341`, the ADR-0013-governed path, 4b52-timestamp-safe). The answer is
a **Grounded Inference** in CONTEXT.md terms: a derived claim carrying its
Inference Chain (`evidence_span_ids`, `evidence_graph_ids`,
`notes_last_sequence`, `route_id`), permanently distinguished from asserted
knowledge, living on a Session Artifact and never in the temporal graph.

Two corollaries the reviewer should check the diff against:
- **User-typed questions are NOT auto-added to the graph.** Today's automatic
  `add_question_to_graph` fires for *transcript-detected* questions (content of
  the meeting). A user's query about the meeting is not meeting content;
  writing it in would pollute the graph with "what did she mean by X". `origin:
  User` ⇒ no graph write. (Open decision D2.)
- **"Promote this answer into the graph" is DEFERRED**, not designed. It needs
  a claim class and per-item evidence under ADR-0037 and a real approval
  affordance. v1 ships zero affordance for it, so there is nothing to review
  for a boundary violation.

### 2.6 Lifecycle state machine

Card status stays the shipped three (`Pending | Approved | Dismissed`) — no
new variant (C12). Answer state is a separate, additive axis.

```
proposal event ──► agentProposals (memory, cap 49 FE / 200 BE)
                     │  kind==question ⇒ add_question_to_graph (local, always)
                     │
        ┌────────────┴─────────────┐
        │ autoAnswerAdmits && gate │            manual "Ask AI" / composer send
        ▼                          ▼                          ▼
   answer_question_card(auto)   (no dispatch: stays actionable)  ask_question_card
        │                                                       │
        ├─ refused (busy|capped|interval|disabled|converse|policy)
        │     ⇒ NO durable write, card unchanged, log line only
        ▼
   streaming ── answerDrafts[cardId] (transient, FE only) ──┐
        │                                                   │
        ├─ Done      ⇒ CardAnswer{Answered}    ──┐          │
        ├─ Error     ⇒ CardAnswer{Failed}      ──┤ upsert   │ deltas render in
        └─ Cancelled ⇒ CardAnswer{Interrupted} ──┘ ONCE     │ the card row
                                                            ▼
   card (Pending + answer) ──► classifyQueueEntry ⇒ "info" when the answer is
                               terminal-and-not-Failed ⇒ row moves to FEED with
                               its thread in the existing details disclosure.
                               Failed stays actionable so Retry is reachable.
        │
        ├─ Dismiss  ⇒ status Dismissed, answer PRESERVED on the record
        ├─ Clear    ⇒ all pending dismissed, answers preserved
        └─ session end ⇒ already on disk ⇒ load_session ⇒ Sessions "Ask" lens
                          renders the thread read-only
```

The "answered ⇒ feed" rule is a **3-line addition to the pure selector**
(`classifyQueueEntry`, before the quality rules) plus tests, and it is this
design's answer to the 74-card pileup on the answered subset: an answered card
resolves itself out of the actionable queue **with its answer attached** —
the opposite of today's dismiss-then-dump.

Known wart, stated: an answered-but-undismissed proposal stays in
`agentProposals` and in the Rust `pending_agent_proposals` map until dismissed
or pruned at 200 (`speech/mod.rs:85,1024`). Removing it frontend-side without a
backend dismiss would desync the two, so this design leaves it; feed placement
is the user-visible resolution.

---

## 3. What I explicitly DEFER

| Deferred | Why | Owner |
|---|---|---|
| Multi-turn follow-up in a thread | Needs conversation state per thread + a history budget; v1 answers are stateless single turns by design (C4) | b437 agent loop (U1-U7) |
| Tool calling / read-only agent tools | `tool_calls` ⇒ `Failed` today (C10); the route-invariant flip is a security-reviewed change | b437 U1-U2 |
| `AuthorizedRoute` seal on the chat path | No chat path is sealed (C9); five provider skins, its own Rust lane | agent-runtime lane |
| Asking questions **of a finalized session** | `historicalReview` blocks it deliberately; needs retrieval over the materialized graph, not the live snapshot | post-83cc |
| Promoting an answer into the graph | Needs ADR-0037 claim class + evidence table | post-83cc |
| Semantic/whole-session retrieval, Admitted Transcript read model | v1 uses the existing top-k graph + transcript tail + notes headings | architecture-review candidate |
| Backend quality field replacing W9's heuristic | 104f's durable half; the gate reads the frontend predicate until then | 104f |
| Streaming answers into the Sessions lens; expand-thread overlay | Tile real estate mitigation, not v1 | phase 2 |
| Making `chatMessages` durable | Explicitly rejected — the card store is the durable record (§2.1) | — |
| Converse reply text rendering | Pre-existing gap, unreachable today either way | new seed |

---

## 4. Ticket-shaped unit breakdown

Rust units A1-A2 are **one lane, serialized** (project memory: max one
Rust-compiling lane on this box). A3-A6 are frontend and can interleave with
each other but not with a Rust cargo run. Every unit is independently
landable and leaves the app in a shippable state.

### A1 — Card answer schema + persistence (Rust) — **S/M**
- **Scope.** `CardOrigin`, `CardAnswer`, `CardAnswerStatus`; additive
  `origin` + `answer` fields on `LiveAssistCardRecord`; validator rules
  (`answer.text` non-empty unless `Failed`; `route_id` non-empty;
  `origin: User` may cite graph context only when the transcript window is
  empty — the one place the citation invariant is relaxed, and it is relaxed
  *narrowly*, not removed); `live_assist_card_record` helper carries the fields
  through.
- **Files.** `src-tauri/src/events.rs`, `src-tauri/src/persistence/mod.rs`
  (`validate_live_assist_card:335`), `src-tauri/src/commands.rs:3971-4029`
  (record helpers), `src/types/index.ts` (mirror types).
- **Gates.** `cargo fmt --check`; `cargo clippy --locked --all-targets -D warnings`;
  `cargo test` — new: old-shape JSON (no `origin`/`answer`) deserializes;
  new-shape JSON deserializes in a reader that ignores unknown fields;
  `upsert → load` round-trip preserves the answer; validator rejects each bad
  shape. `bun run tsc --noEmit`.
- **Deps.** none. **Risk.** low; additive-fields-only is the whole point.

### A2 — Answer engine + gate + ledger + telemetry (Rust) — **L**
- **Scope.** `prepare_card_answer_request` (stateless, no `chat_history`,
  graph top-k + transcript tail + **notes outline headings capped at ~1,500
  chars** via `state.rs:570`); terminal-frame hook that builds `CardAnswer`,
  upserts **once**, appends the ledger row, emits `AGENT_CARD_UPDATE`;
  `answer_question_card` + `ask_question_card` commands; the spend gate
  (enabled / interval / per-session cap / `stream_registry.is_empty()` /
  converse exclusion) with a typed `AnswerRefused { reason }`;
  `AgentAutoAnswerSettings` on `AppSettings`; counters on `AppState` reset on
  session rotation; content-free log lines.
- **Files.** `src-tauri/src/commands.rs` (new block next to the chat commands,
  `:3170-3700`), `src-tauri/src/state.rs`, `src-tauri/src/settings/mod.rs`,
  `src-tauri/src/events.rs` (event name + payload), `src-tauri/src/lib.rs`
  (register 2 commands), `src/types/index.ts` + `src/hooks/useTauriEvents.ts`
  (event wiring only).
- **Gates.** `cargo fmt`/`clippy`/`test`; new tests: gate-off ⇒ `Err`, zero
  provider call; cap reached ⇒ `Err`; interval unmet ⇒ `Err`; registry
  non-empty ⇒ `Err` **and no `cancel_all`**; converse mode ⇒ `Err`;
  `privacy_mode` blocking ⇒ zero provider call (mirrors the existing
  `llm_chat` policy test at `commands.rs:12261`); exactly one
  `upsert_live_assist_card` per answered card; exactly one
  `provider_call_started` + one terminal ledger row per dispatch;
  `chat_history` **unchanged** after an auto-answer; answer carries the
  resolved `route_id`. `bun run verify:contracts` (expected: no diff — the
  ledger vocabulary is reused, not extended).
- **Deps.** A1. **Risk.** medium — this is the spend boundary; review as a
  security change, not plumbing. Split A2b (notes context block) out if the
  lane is tight.

### A3 — Composer + thread rendering in the agent tile (frontend) — **M**
- **Scope.** `AgentComposer` (fixed bottom row, `ChatSidebar`'s 3-region shape
  minus the message log); `AnswerThread` leaf (streaming dots / answer text /
  failure text + Retry / evidence + route chip); `answerDrafts:
  Record<string, {text, status, requestId}>` transient store slice + the
  delta-coalescing helper extracted once and shared with the existing
  `sendChatMessage` coalescer; `askQuestion(text)` and
  `answerQuestionCard(id)` store actions; i18n add-only (en+pt, pt chips ≤18
  chars) reusing `chat.inputPlaceholder`/`inputLabel`/`send`/`thinking`.
- **Files.** `src/components/AgentProposalsPanel.tsx` (body region +
  `AgentQueueRow`/`AgentFeedRow` answer slots), new
  `src/components/workspace/AgentComposer.tsx`, `src/store/index.ts`, new
  `src/store/chatStream.ts` (extracted coalescer),
  `src/i18n/locales/{en,pt}.json`, tests.
- **Gates.** `bun run check`; `bun run tsc --noEmit`; `bun run test:coverage` —
  send ⇒ one `ask_question_card` invoke; deltas render into the target card
  only; terminal frame clears the draft; failure shows Retry; a11y (composer
  labelled, thread in the existing `role="log"`-free card markup with a status
  region, keyboard-only, 200 % zoom == compact tier per ADR-0046's matrix).
- **Deps.** A1 + A2 (for the real commands; can be built against a stub).

### A4 — Auto-answer trigger + Signal-bar parity (frontend) — **S/M**
- **Scope.** `autoAnswerAdmits` in `agentQueue.ts`; hook it into
  `addAgentProposal` (`store/index.ts:1800-1826`) — the one funnel each card
  passes through exactly once, already the precedent site for a
  fire-and-forget side effect (`add_question_to_graph`). **Never** in a render
  path or an effect over the queue (remount/StrictMode would double-spend).
  Converse-mode check; `answeredCount`/`cap` mirrored from settings for the
  counter chip; the `classifyQueueEntry` "terminal answer ⇒ info" rule.
- **Files.** `src/components/workspace/agentQueue.ts`, `src/store/index.ts`,
  `src/components/AgentProposalsPanel.tsx` (counter chip in the composer row,
  **not** the header slot — C2), tests.
- **Gates.** `bun run test:coverage` — a fragment-suspect fixture dispatches
  nothing; an admitted fixture dispatches exactly once even when
  `addAgentProposal` is called twice with the same id (upsert semantics);
  duplicate-collapse victim dispatches nothing; converse mode dispatches
  nothing; answered card lands in `feed` in both Signal and All modes;
  Failed answer stays in `queue`.
- **Deps.** A2, A3.

### A5 — Surface consolidation: delete `ChatSidebar`, re-point the Ask lens — **M**
- **Scope.** Delete `src/components/ChatSidebar.tsx`; new read-only
  `SessionThreads` over `liveAssistCards`; `SessionsBrowser` Ask aside renders
  it (keep `askAvailable`'s notes/graph gating, keep the honest "asking a
  finalized session is not delivered" line in place of the disabled input);
  drop the `chatMessages` push in `approveAgentProposal`
  (`store/index.ts:1884`) and the `chat_history` push in
  `approve_agent_proposal_impl` (`commands.rs:4302-4312` — a 10-line Rust
  change, so this unit carries a small Rust tail; land it with A2 if lanes
  collide).
- **Files.** `src/components/ChatSidebar.tsx` (deleted), its test file, new
  `src/components/SessionThreads.tsx`, `src/components/SessionsBrowser.tsx`,
  `src/store/index.ts`, `src-tauri/src/commands.rs`, `e2e/specs/*` and any
  contract test naming the sidebar.
- **Gates.** `bun run check`; `tsc`; `test:coverage`; `bun run build`;
  `bun run test:e2e` (Sessions-detail × each lens at 1440/1024/768 per
  ADR-0046's restated matrix). Grep gate: zero remaining references to
  `ChatSidebar`.
- **Deps.** A3. **Risk.** medium — it deletes a shipped component; the
  no-regression argument is C1 + the `reviewLocked` fact, and the reviewer
  should re-verify both.

### A6 — Settings control + budget legibility — **S**
- **Scope.** Auto-answer toggle + cap + interval in the Settings LLM section
  via the existing `saveSettings` → `save_settings_cmd` path
  (`store/index.ts:3231`); the "3/12 auto-answers this session" chip in the
  composer row; the refusal reason surfaced inline when a dispatch is refused
  for a *user* action (policy-blocked ⇒ reuse the existing
  `PRIVACY_POLICY_BLOCKED` event, `events.rs:154`).
- **Files.** Settings panel + controller, `AgentComposer`, i18n, tests.
- **Gates.** `bun run check`; `tsc`; `test:coverage`; settings round-trip test
  (absent field ⇒ default, save ⇒ reload identical).
- **Deps.** A2 (settings shape), A3.

**Shortest path to the ratified behaviour: A1 → A2 → A3 → A4 (four units).**
A5 and A6 are the honesty/consolidation tail and can land after the user
already has a working chatbox with threaded auto-answers.

### Deletion test (every component added)

| Added | Deleted ⇒ what breaks |
|---|---|
| `CardAnswer` field | no durable answer; threads die with the process |
| `answer_question_card` / `ask_question_card` | no answer path that threads; back to dismiss-then-dump |
| spend gate | 74 uncapped paid calls in 59 min, mutually cancelling |
| `AgentComposer` | no free-form input anywhere reachable (C1) |
| `AnswerThread` | answers invisible; the field failure persists |
| `answerDrafts` | no progress, no error text — a silent 3-9 s dead card |
| `autoAnswerAdmits` | either no auto-answer, or a second copy of the Signal rules that can drift |
| `AGENT_CARD_UPDATE` | the UI can't see the durable record; draft/durable never reconcile |
| `SessionThreads` | the Ask lens keeps showing an empty in-memory chat |

---

## 5. Honest weaknesses of this angle

1. **Stateless answers, no follow-up.** A thread is one question + one answer.
   Asking "why?" mints a new card with no memory of the previous exchange. This
   is the single largest UX compromise and it is exactly what b437's loop
   fixes. Chosen deliberately: a shared-history prompt is what makes 74
   auto-answers catastrophic (C4).
2. **Best-effort answering.** Single-flight + drop + interval + cap means that
   under the observed load (74 questions / 59 min) roughly 12 get answered and
   the rest keep only the manual button. Spend is bounded; "the user's question
   gets answered" is not solved.
3. **Thin retrieval.** Graph top-40 + last-10 segments + notes headings. A
   question about something said 20 minutes ago will be answered ungroundedly,
   and the design has no way to detect or admit that. No "I don't have that
   context" signal exists.
4. **Route provenance is partial.** Resolved-route stamping, not an
   `AuthorizedRoute` seal (C9). Auto-answer becomes a new *automatic* egress
   producer on the least ADR-0038-sealed LLM path in the app. Mitigated by
   count cap + ledger row + distinct action id; not eliminated.
5. **The quality bar is renderer-authored.** Rust caps volume but cannot
   independently verify the Signal decision. If W9's heuristics are wrong
   (104f), we auto-answer fragments up to the cap — and the frontend can be
   made to dispatch whatever it wants within the cap.
6. **`AgentProposalPayload` is stretched.** It is documented as "emitted by the
   agent/react loop" (`events.rs:434`); `origin: User` makes it also carry
   user-authored questions. An `origin` enum keeps it honest, but a proper
   `SessionQuestion` type would be cleaner. This is a deliberate vocabulary
   compromise for unit count.
7. **Tile real estate.** Queue + feed + composer + inline answers inside one
   quarter of a bento grid, at 768 px and 200 % zoom (== compact tier). Real
   risk the tile becomes unusable once two answers are open; the expand-to-
   overlay mitigation is deferred.
8. **The 8 MB card-file ceiling fails silently and totally.** Over the ceiling,
   `load_session` degrades `live_assist_cards` to an **empty vec**
   (`commands.rs:7977-7985`) — all cards and all answers vanish from review,
   not truncated. Caps make it unlikely (tens of KB expected); the failure mode
   is nonetheless total loss, and the audit `.jsonl` (append-per-upsert) grows
   faster than `current.json`.
9. **Deleting `ChatSidebar` rests on a negative claim.** "Converse text is
   already unrenderable during capture" is verified via `reviewLocked` +
   ADR-0046, but it is the kind of claim ADR-0034 says to prove exhaustively,
   and the sample-preview path is the one place `historicalReview` can be false
   (`SessionsBrowser.tsx`'s `!samplePreviewActive && !row` guard). Re-verify
   before landing A5.
10. **Answered cards stay "pending" in two places.** Feed placement is
    cosmetic resolution; `agentProposals` and the Rust
    `pending_agent_proposals` map still hold them until dismiss or the 200-cap
    prune.

---

## 6. Open decisions for the maintainer

- **D1 — Auto-answer default and caps.** Recommend `enabled: true`,
  `max_per_session: 12`, `min_interval_secs: 45`. Default-ON matches the
  ratified wording; the caps are what make the worst case bounded. Say if you
  want default-OFF for the first field build instead.
- **D2 — Does a user-typed question become a `Question` node in the graph?**
  Recommend **no** (a query is not meeting content). Transcript-detected
  questions keep today's automatic write.
- **D3 — Drop vs. queue.** Recommend drop (best-effort, never a backlog). If
  every admitted question must eventually be answered, that is a durable
  answer queue with its own scheduler and retry class — a materially larger
  design.
- **D4 — Fate of the Sessions "Ask" lens.** Recommend read-only thread viewer
  (this design). Allowing questions *of a finalized session* needs retrieval
  over the materialized graph and is real work.
- **D5 — Promoting an answer into the graph.** Recommend defer (ADR-0037
  claim class + evidence). Confirm you do not want an affordance in v1.
- **D6 — Composer in-tile vs. expandable overlay** once a thread is open
  (weakness 7). Recommend in-tile for v1, overlay as a phase-2 ticket.
- **D7 — Manual ask under a blocking privacy mode.** Recommend keeping today's
  behaviour (the gate returns `Err`, the card shows the reason via the existing
  `PRIVACY_POLICY_BLOCKED` event) rather than hiding the composer.

---

## 7. Dependency honesty

- **W9 Signal classification** is the auto-answer gate and it is **shipped**
  (`agentQueue.ts:219-269`, `:345-403`). A4 consumes it; it invents no rules.
- **104f (fragment fix, P1)** sharpens the same predicate. It **does not block**
  A1-A6: the chatbox and threading work whatever the gate admits, and 104f's
  eventual backend quality field replaces `classifyQueueEntry`'s body without
  touching any surface in this design ("one line, no tile change").
  Interaction risk is stated as weakness 5.
- **81a5 telemetry** lands in parallel. A2's log lines are additive and use
  house style (`elapsed_ms=` / counters, content-free) — if 81a5 introduces
  proper counters for the question subsystem, A2's lines fold into them; there
  is no shared file to conflict on beyond `commands.rs`.
- **b437 / U1-U7 agent runtime.** v1 uses the **existing single-turn streaming
  route** (`start_streaming_chat`'s task, ADR-0038 route resolution, no tools).
  The seam is `answer_question_card`'s body, specifically steps 5-6: when the
  Rust loop lands, `prepare_card_answer_request` + `spawn_stream_task` are
  replaced by the loop's entry point behind the **same command signature, the
  same channel, the same `CardAnswer` record, and the same gate**. The
  invariants the loop must preserve, stated now so the seam is real: persist
  once on terminal; single-flight refusal (a multi-turn loop makes N provider
  calls where the cap assumed one, so the **cap must become per-answer,
  not per-call**); `actor: "system"` ledger row per model call;
  `route_id` stamped by trusted code; no graph write without the approve path.
  Nothing in this design has to be unbuilt for that lane to land.
- **Ratified agent-runtime verdict** (`synthesis-agent-runtime.md` §5 item 7)
  already places the auto-answer policy "exactly where the manual Ask AI click
  sits". This design does that, and additionally fixes the two things that
  click does wrong: it stops dismissing the card, and it stops dumping into a
  surface nobody renders.
