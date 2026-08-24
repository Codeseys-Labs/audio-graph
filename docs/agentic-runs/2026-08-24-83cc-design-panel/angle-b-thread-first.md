# Angle B — Thread-first card lifecycle (epic `audio-graph-83cc`)

Design panel, 2026-08-24. Read-only recon against the working tree; no code
changed, no gates run. Every file:line below was read this pass.

**Prior (assigned):** the card *plus its answer thread* is the durable domain
object — a Session Memory citizen: persisted, replayable, provenance-carrying
under [ADR-0037](../../adr/0037-admit-session-memory-items-through-a-layered-claim-class-evidence-table.md)'s
Evidence Annotation conventions. The chatbox is a **view over threads**, not a
second store. So: lifecycle machine first, surfaces and commands derived from
it.

**The field failure this design must make structurally impossible:** "Ask AI"
today dismisses the card and pipes its text into a chat log that nothing in the
capture workspace renders (`src/store/index.ts:1828-1841` →
`sendChatMessage` :2872; `ChatSidebar` mounts only under
`SessionsBrowser` :827 and is input-disabled the moment a row is selected,
`ChatSidebar.tsx:34,52,192,199`). 74 pending cards accumulated in 59 minutes
while chat was touched twice. The mechanism is that **an answer has nowhere to
live**. Angle B's answer: give it a home that is already durable, already
replayed on session open, and already citation-validated.

---

## 1. Constraints I hold

Hard, verified, non-negotiable in this design:

1. **`WorkspaceTileId` is frozen** — `"transcript" | "graph" | "document" |
   "agent"` (`src/components/workspace/WorkspaceTile.tsx:14`), pinned by
   `WorkspaceTile.test.tsx`, and persisted verbatim by phase-2
   `WorkspaceLayoutPrefs`. No fifth tile. The chatbox lives **inside** the
   `agent` tile's single `children` slot.
2. **One `headerSlot` per tile, already double-occupied** on `agent`
   (`AgentQueueFilterToggle` + `AgentTileHeaderActions`,
   `src/App.tsx:663-676`). Any new header control composes into that same
   `<span>` or does not exist.
3. **`AGENT_QUEUE_PANEL_ID` is the Signal/All tablist's `aria-controls`
   target** (`AgentProposalsPanel.tsx:70,186,580,601`). The composer is *not*
   controlled by that toggle, so the id must stay on the scroll region only
   when the body splits.
4. **The live-assist card is ALREADY a durable, replayable Session Artifact.**
   `LiveAssistCardRecord` (`src-tauri/src/events.rs:467-489`) persists to
   `live_assist/<session>.current.json` + an append-only
   `live_assist/<session>.jsonl` audit stream
   (`persistence/mod.rs:1442-1471`), is citation-validated
   (`validate_live_assist_card` :335-360 — spans **or** graph context, the
   ADR-0037 §1 precedent), and is returned by `load_session`
   (`commands.rs:70,8007`) straight into `liveAssistCards`
   (`store/index.ts:3432`). **This is the seam. The thread hangs off it.**
5. **`current.json` is a whole-file rewrite per upsert and degrades to an
   empty vec above its ceiling.** `upsert_live_assist_card` loads the whole
   array, replaces, sorts, `save_json` (`persistence/mod.rs:1454-1470`);
   `MAX_LIVE_ASSIST_CARDS_BYTES = 8 MiB` and exceeding it silently yields
   `live_assist_cards: vec![]` on load (`commands.rs:141,7972-7986`).
   Embedding growing answer text in the card head reproduces the
   `audio-graph-cfa1` growth shape and risks losing *every* card on open.
   **Answer bodies must not live in the card head.**
6. **`chat_history` is process memory only** — `Arc<RwLock<Vec<ChatMessage>>>`
   (`state.rs:239`), capped at 200 (`commands.rs:3268`), cloned whole into
   every request (`prepare_chat_request` :3230-3240), no field on
   `LoadedSession`, reset on capture start (`store/index.ts:1998-2008`).
   `ChatMessage` has no id, no timestamp, no per-turn status
   (`types/index.ts:2666-2669`). It cannot be the durability story and it
   cannot carry provenance.
7. **`start_streaming_chat` cancels every prior in-flight stream**
   (`commands.rs:3395-3404`, `stream_registry.cancel_all()`), documented as the
   single-active-stream invariant. An auto-answer that reuses this command
   would **kill the user's own in-flight turn**. Automatic dispatch must be
   queued and user-preemptible, never a peer caller of that path.
8. **The only graph writer for agent cards is `approve_agent_proposal`**
   (`commands.rs:4154` → `approved_agent_projection_patch` :4066-4151 →
   `apply_runtime_projection_patch`), plus the local, LLM-free
   `add_question_to_graph` (:4350) that already fires automatically for every
   question card (`store/index.ts:1800-1808`). Chat must not add a third
   writer. (ADR-0013 governs conversation modes; the write-path rules in force
   here are ADR-0024's projection event path and ADR-0037's per-item evidence
   table — the approve path is the one door.)
9. **ADR-0038 route discipline:** named routes only, empty fallback lists,
   per-endpoint capability, served-route provenance stamped by trusted code
   (`llm/route.rs:188-281,378-389,494-553`). A new spending operation gets its
   own named route and its own authorization, not a reuse of the chat route by
   coincidence.
10. **`finish_reason: "tool_calls"` is `TerminalStatus::Failed` today**
    (`route.rs:646-649`); zero `tools`/`tool_choice` anywhere in `src-tauri`.
    v1 sends no tools. Full stop.
11. **The chat path appends NO data-movement ledger row.** Verified: the only
    producers are capture lifecycle (`commands.rs:1730-1766`) and projections
    (`projection_data_movement.rs`). `"tool_calls"` is a reserved `DataClass`
    (`src/generated/sessionDataMovement.ts:49`). Auto-answer is a new ADR-0034
    egress producer and must appear in the ledger.
12. **The live-assist subsystem has zero backend telemetry** (round-5 S9: 0
    hits for `question`/`AgentProposal`/`live_assist`/`chat` across 18k log
    lines). Anything this epic ships is also the first observability this
    subsystem has ever had.
13. **`agentQueue.ts` is a zero-new-store-state pure selector**
    (`src/components/workspace/agentQueue.ts:8-12`). I am about to spend part
    of that budget deliberately (see §11 weakness 2) — but every *derivation*
    stays pure and testable without a Zustand harness.
14. **Proposal creation is local and LLM-free** —
    `run_agent_proposal_task` (`speech/mod.rs:1073-1140`) mints one card per
    finalized segment from `agent_proposal_kind(&text)`; backend pending map
    caps at 200 (`speech/mod.rs:85,1024-1040`), frontend slices to 49
    (`store/index.ts:1802`). Card *creation* costs nothing. Only answering
    costs money.

---

## 2. The domain object: an Assist Thread

One new noun, defined in the project's vocabulary:

> **Assist Thread** — the durable, replayable conversation attached to exactly
> one live-assist card: the card's proposal, the turns exchanged about it, and
> the Evidence Annotations each answer turn carries. It belongs to one Session
> and is Provisional Session Memory until the Session finalizes.
> _Avoid_: chat log, conversation history, message list.

### 2.1 Storage shape — bounded head, append-only body

Three files under `live_assist/`, two of which already exist:

| Artifact | Status | Shape | Rewrite cost |
|---|---|---|---|
| `live_assist/<id>.current.json` | **exists** | array of card **heads** | O(cards) per upsert — must stay bounded |
| `live_assist/<id>.jsonl` | **exists** | append-only audit of card heads | O(1) append |
| `live_assist/<id>.turns.jsonl` | **NEW (B1)** | append-only turn log, one line per turn | O(1) append |

The card head gains only **bounded scalars** (all `#[serde(default)]`, the exact
forward-compat technique `source_span_ids`/`graph_context_ids` already use, so
every pre-thread record deserializes unchanged — ADR-0027):

```rust
// src-tauri/src/events.rs — additive to LiveAssistCardRecord (:467)
#[serde(default)] pub origin: AssistOrigin,          // Detected | UserPrompt
#[serde(default)] pub thread_state: AssistThreadState,
#[serde(default)] pub turn_count: u32,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub last_turn_at_ms: Option<u64>,
/// Collapsed-row preview ONLY. Hard-capped at 240 chars at construction.
/// Never the answer of record — that lives in `turns.jsonl`.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub answer_preview: Option<String>,
/// Backend signal grade (B3). Present from B3 forward; `None` on older rows.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub signal: Option<SignalGrade>,
```

The turn record — the thing that is genuinely new:

```rust
pub struct AssistTurnRecord {
    pub session_id: String,
    pub thread_id: String,          // == proposal.id. One thread per card.
    pub turn_seq: u32,              // 1-based, dense, per thread
    pub turn_kind: AssistTurnKind,  // User | Answer | ToolCall | ToolResult
    pub content: String,
    /// ADR-0037 anchors for an Answer turn. Empty for a User turn (the user
    /// asserts; they do not claim about the session).
    #[serde(default)] pub evidence: Vec<crate::claim_evidence::EvidenceAnchor>,
    /// What was actually fed to the model (B6). Span ids + note ids + graph
    /// node ids, never text — the answer's audit trail.
    #[serde(default)] pub retrieval_manifest: Option<AssistRetrievalManifest>,
    /// Trusted-code-stamped served route identity (ADR-0038 :494-553).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<crate::llm::route::AttemptedRouteIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_total_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// `Automatic` vs `Manual` — which gate authorized this spend.
    pub dispatch: AssistDispatch,
    pub created_at_ms: u64,
    /// Reserved for the b437 agent loop (U1-U7). Always empty in v1.
    #[serde(default)] pub tool_calls: Vec<AssistToolCall>,
}
```

Everything the round-5 inventory said a chat view-model lacks — stable id
(`thread_id` + `turn_seq`), timestamp, per-turn status, finish reason, usage,
origin linkage back to the proposal — is present here **because the record is
the card's, not the chat log's**.

Own ceiling + lazy read: `MAX_ASSIST_TURNS_BYTES = 4 MiB` in
`session_artifact_ceilings` (`commands.rs:112-150`). `load_session` does **not**
read it; a per-thread `load_assist_thread(session_id, thread_id)` command does,
with the same `enforce_artifact_ceiling` stat-only guard (:171). This is
deliberate: constraint 5's degrade-to-empty-vec behavior means one oversized
artifact must never be able to erase the card heads.

### 2.2 Every chat message belongs to a thread

The one structural move that kills the field failure: **there is no unthreaded
turn.** A free-form message typed into the composer with no card selected mints
a card with `origin: UserPrompt`, `kind: Question`, `confidence: 1.0`,
`source_span_ids` = the anchored transcript window at send time, and
`thread_state: Answering`. It renders as a thread in the same list, persists to
the same artifacts, replays on session open like any other.

Consequence: the composer needs **no separate persistence, no separate view
model, and no separate render path.** `chat_history` (`state.rs:239`) survives
only as a prompt-assembly convenience for the legacy blocking path and is
removed from the assist path entirely (B4 passes `persist_to_history: false`,
the flag `spawn_stream_task` already takes at `commands.rs:3383`).

One honest cost: `validate_live_assist_card` requires a non-empty
`proposal.source_segment_id` (`persistence/mod.rs:346-351`). A user question
typed before any speech has no span. See open decision **D3**.

---

## 3. The lifecycle state machine

Two orthogonal axes. `status` (shipped: `Pending | Approved | Dismissed`,
`events.rs:460-465`) keeps meaning **"what happened to the proposal's graph
write"**. `thread_state` (new) means **"where the conversation is"**. They never
collide, and `Approved`-requires-`outcome` validation (`persistence/mod.rs:357`)
stays untouched.

```
                       ┌─────────────────────────────────────────┐
   segment finalized   │                                         │
        │              │            (user types, no card)        │
        ▼              │                     │                   │
   ┌─────────┐  B3     │                     ▼                   │
   │ Created │──grade──┤            ┌──────────────────┐         │
   └─────────┘         │            │ Created(UserPrompt)│───────┤
        │              │            └──────────────────┘         │
        ▼              │                                         │
   ┌──────────────┐    │                                         │
   │ Classified   │    │                                         │
   │ Strong/Weak/ │    │                                         │
   │ Fragment     │    │                                         │
   └──────────────┘    │                                         │
     │   │      │      │                                         │
     │   │      └──────┴──► ┌──────────┐ user dismisses          │
     │   │                  │Dismissed │◄────────────────(any)   │
     │   │  Weak/Fragment   └──────────┘  terminal, replayable   │
     │   │  (All-mode Ask)                                       │
     │   └──────────┐                                            │
     │ Strong &     ▼                                            ▼
     │ authorized  ┌──────────┐  dispatch  ┌───────────┐   ┌──────────┐
     └────────────►│ Queued   │───────────►│ Answering │──►│ Answered │
                   └──────────┘            └───────────┘   └──────────┘
                        │ gate refuses          │ error/cancel   │  │
                        ▼                       ▼                │  │
                   ┌──────────────┐      ┌──────────────┐        │  │
                   │ NotAuthorized│      │ AnswerFailed │◄───────┘  │
                   │ (typed reason)│      │(typed reason)│  retry    │
                   └──────────────┘      └──────────────┘           │
                                                │ retry             │
                                                └──────► Answering  │
                                                                    │
                        follow-up turn ─────────────────────────────┘
                        (Answered → Answering, turn_seq + 1)

     Answered ──"Add to graph"──► mints a Note proposal ──► approve_agent_proposal
                                  (status: Pending → Approved; thread_state unchanged)
     capture stop ──► thread frozen in place; reopens read-only in Sessions
```

### 3.1 Transition table (who fires, what it costs, what it persists)

| From | Event | To | Actor | Spend | Persists |
|---|---|---|---|---|---|
| — | segment finalized, `agent_proposal_kind` matched | `Created` | `speech/mod.rs:1105-1140` | none | card head (`Pending`, `thread_state: None`) |
| `Created` | B3 grades the proposal | `Classified` | Rust, `agent_signal_grade` | none | `signal` on the head |
| `Classified(Strong)` | auto-answer policy authorizes | `Queued` | `authorize_auto_answer` (B5) | none | head + one log line |
| `Classified(*)` | policy refuses (off / cap / budget / grade) | `NotAuthorized{reason}` | B5 | none | head; **typed reason rendered**, never silent |
| `Classified(*)` | user clicks **Ask** | `Queued` (front of queue) | FE → `start_assist_answer` | authorized | head |
| `Queued` | dispatcher slot free, no user stream in flight | `Answering{request_id}` | B4 dispatcher | **1 provider call** | head + ledger `provider_call_started` |
| `Answering` | terminal `Done` | `Answered` | B4 turn sink | — | `turns.jsonl` Answer row + head `answer_preview`/`turn_count` + ledger `..._succeeded` + `sessions::usage` |
| `Answering` | error / `Truncated` / `Refused` / cancel | `AnswerFailed{class}` | B4 | — | `turns.jsonl` Answer row w/ `finish_reason` + ledger `..._failed`/`..._cancelled` |
| `Answered` | user sends a follow-up in this thread | `Answering` (`turn_seq+1`) | FE | authorized | User turn row first, then Answer row |
| `AnswerFailed` | user retries | `Queued` | FE | authorized | head |
| any non-terminal | user dismisses | `Dismissed` | `dismiss_agent_proposal` (:4520, unchanged) | — | head (`status: Dismissed`); **turns are never deleted** |
| `Answered` | user clicks **Add to graph** | proposal minted → existing approve path | B9 + `approve_agent_proposal` | none (local) | `ProjectionOperation` **only** via approve |
| any | capture stops | frozen; `status`/`thread_state` unchanged | — | — | artifacts already on disk |

### 3.2 Invariants the machine buys (each one a test)

- **I1 — No orphan answers.** Every `AssistTurnRecord` has a `thread_id` that
  resolves to a card head in the same session. A turn cannot exist without a
  card. *(The field bug is now unrepresentable.)*
- **I2 — Dismissal never destroys a thread.** `Dismissed` is a head-status
  change; `turns.jsonl` is append-only. Today "Ask AI" dismisses the card and
  loses the connection — here, dismiss-after-answer keeps the answer readable
  in the feed.
- **I3 — Exactly one provider call per `Queued → Answering` edge**, 1:1 with a
  ledger row and one `sessions::usage` increment (`usage.rs:373-387`).
- **I4 — Automatic dispatch never preempts the user.** The dispatcher refuses
  to leave `Queued` while `stream_registry` holds a user-initiated request; a
  user action cancels or jumps the automatic queue, never the reverse.
- **I5 — No projection write from the answer path.** Zero `projection-patch`
  events and zero `ProjectionOperation` rows from `start_assist_answer` under
  test.
- **I6 — Refusals are typed and visible.** `NotAuthorized`/`AnswerFailed` carry
  a stable snake_case reason class the frontend translates (same convention as
  `AppError::ArtifactTooLarge`'s `artifact_class`, `commands.rs:165-166`). No
  state ever means "nothing happened, we don't know why."
- **I7 — Grade is advisory for display, authoritative for spend.** The frontend
  W9 filter may hide a row; only the backend grade may authorize a call.

---

## 4. Surfaces, derived

### 4.1 The agent tile becomes the assist surface (v1)

`AgentProposalsPanel`'s body (`AgentProposalsPanel.tsx:599-647`) goes from one
scrolling column to a two-region flex column — the exact shape `ChatSidebar`
already proves works (`ChatSidebar.tsx:71-207`: fixed header, `flex-1` log,
pinned input row):

```
WorkspaceTile id="agent"  headerSlot=[Signal/All toggle][auto-answer chip][Clear]
├─ <div id={AGENT_QUEUE_PANEL_ID} class="flex-1 overflow-y-auto">   ← keeps the id
│   ├─ "Needs you"    — actionable, non-question cards (approve/dismiss) + Weak
│   │                    question cards with a manual Ask
│   ├─ "Threads"      — question cards WITH a thread, newest activity first.
│   │                    Collapsed row = kind chip · thread-state chip ·
│   │                    question text · answer_preview (2 lines) · ×N dupes.
│   │                    Expanded = full turns (lazy `load_assist_thread`),
│   │                    per-answer evidence disclosure, follow-up affordance,
│   │                    "Add to graph", "Dismiss".
│   └─ "Recent activity" — the existing read-only feed, unchanged semantics
└─ <div class="shrink-0 border-t">  composer  </div>                ← NOT in the panel id
```

- The composer renders in **every** state including idle (today's designed
  idle state at :577-597 returns before any input could exist — that is
  precisely why the chatbox is unreachable). Placeholder distinguishes
  "ask about this conversation" (live) from the read-only review copy.
- Send with no thread focused → new `origin: UserPrompt` thread. Send with a
  thread expanded → follow-up turn in that thread. The composer shows which,
  above the input, so the target is never ambiguous.
- The header gets **one** new chip inside the existing `<span>` (constraint 2):
  `auto ▸ 4/20 · ~12k tok`, click-through to the off switch. Data comes from
  the shipped `LLM_USAGE_UPDATE` event (`events.rs:149`,
  `commands.rs:3319-3336`) — no new telemetry needed for the cost display.
- A11y: the log region carries `role="log" aria-live="polite"` migrated
  verbatim from `ChatSidebar.tsx:106-107`; one debounced sr-only announcement
  per thread-state change, never per delta (W8's "no per-row alerts" rule).
  The Signal/All roving-tabindex contract (`:183-192`) is untouched because
  `AGENT_QUEUE_PANEL_ID` stays on the scroll region and the composer sits
  outside it.

### 4.2 Sessions: `assist` becomes a lens, the "Ask" aside dies (v1.1)

`DETAIL_LENSES` (`SessionsBrowser.tsx:208-215`) gains `"assist"`;
`askAvailable` (:227), `askOpen` (:662) and the `ChatSidebar` aside (:825-828)
are deleted. The lens renders the **same** `AssistThreadPanel` component in
read-only mode over the threads `load_session` already returns. This dissolves
the `historicalReview` paradox: in review the composer is legitimately
read-only for a finalized session (and says so once, in one place), while
during live capture it is the primary input.

### 4.3 Surface-consolidation verdict

**Absorb, in two steps, not coexist.** `ChatSidebar`'s *rendering* migrates
into `AssistThreadPanel` in B2; the file, `askAvailable`, and `askOpen` are
deleted in B8. Between B2 and B8 the sidebar stays mounted exactly as-is —
which is safe precisely because it is already unreachable in every real path,
so there is no field divergence to manage. ADR-0046's two destinations are
unchanged; no new tab, no new tile, no new modal.

---

## 5. Backend commands and events

New commands (all `#[tauri::command]`; the two read commands are `async` per
round-5 S5's rule that heavy reads leave the main thread):

| Command | Signature | Notes |
|---|---|---|
| `start_assist_answer` | `(thread_id, follow_up: Option<String>, channel: Channel<AssistStreamEvent>) -> request_id` | Manual dispatch. Gate → route → `spawn_stream_task(persist_to_history=false)` + turn sink. |
| `send_assist_prompt` | `(text, channel) -> (thread_id, request_id)` | Mints a `UserPrompt` card, then the same path. |
| `cancel_assist_answer` | `(request_id)` | Delegates to the existing `stream_registry.cancel` (:3665-3677). |
| `load_assist_thread` (async) | `(session_id, thread_id) -> Vec<AssistTurnRecord>` | Ceiling-guarded, per-thread. |
| `load_assist_thread_heads` (async) | `(session_id) -> Vec<LiveAssistCardRecord>` | Already covered by `load_session`; exists for the Sessions lens' lazy refresh. |
| `promote_assist_answer` | `(thread_id) -> AgentProposalPayload` | Mints a `Note` proposal into `pending_agent_proposals`. **Writes nothing.** |
| `set_auto_answer_enabled` | `(enabled: bool)` | The off switch; persists through `settings`. |

New events (additive; the frontend already tolerates unknown variants on
tagged unions — pin it with a serde round-trip test, runtime-synthesis probe
P4):

- `assist-thread-update` → `{ thread_id, thread_state, turn_count, answer_preview?, reason_class? }`
- `assist-turn-delta` → delivered over the per-invocation
  `tauri::ipc::Channel`, **not** a global event, reusing the ordered channel
  the chat hot path already uses (`commands.rs:3385-3452`). No new transport.

Unchanged and deliberately so: `approve_agent_proposal` (:4154),
`dismiss_agent_proposal` (:4520), `clear_agent_proposals` (:4550),
`add_question_to_graph` (:4350), `start_streaming_chat` (:3590).

---

## 6. LLM routing and cost policy

Auto-answers are **new spend on a schedule the user did not click.** Policy,
all enforced in Rust:

1. **A distinct named route.** `route.assist_answer` added to `LLM_ROUTES`
   (`route.rs:188-281`), empty fallback list, per-endpoint capability row,
   served-route provenance stamped onto each Answer turn
   (`AttemptedRouteIdentity`, :494-553). Rationale: ADR-0038 sub-decision 1's
   own shape — operations may differ in route — and the router-proxy sketch's
   U6. An answer's completion budget is clamped by
   `RouteDescriptor::clamp_completion_budget` (:177), not by an ad-hoc literal.
2. **A distinct egress action id.** `"assist_auto_answer"` and
   `"assist_manual_answer"`, both through `enforce_session_content_policy`
   (`commands.rs:841`) with data classes
   `["user_message","transcript","graph_context","notes","prompt"]`. Distinct
   from `"llm_chat"` so a user can permit manual asks and forbid automatic
   ones, and so `PRIVACY_POLICY_BLOCKED` telemetry says which one was refused.
3. **The gate, in order, cheapest predicate first:**
   `auto_answer_enabled` → `kind == Question` → `signal.grade == Strong` →
   not a duplicate-collapse victim → in-flight slot free → per-minute bucket →
   per-session count → per-session token budget. First refusal wins and is
   recorded as the typed `NotAuthorized` reason.
4. **Defaults** (new `AutoAnswerPolicy` in `settings/mod.rs` next to
   `privacy_mode` :1261, with an ADR-0027 migration default):
   `enabled: true`, `min_grade: Strong`, `max_concurrent: 1`,
   `per_minute: 3`, `per_session: 20`, `session_token_budget: None`.
   Calibration against the field: 74 cards in 59 min ≈ 1.25 cards/min, so
   `per_session: 20` is the binding cap on a one-hour session, not
   `per_minute`. That is intentional — a hard, legible ceiling on a
   pathological session.
5. **Off switch in two places:** the header chip's click-through and Settings.
   Turning it off mid-session drains the queue to `NotAuthorized{disabled}` and
   leaves manual Ask fully available.
6. **Budget source of truth is durable, not in-memory:**
   `sessions::usage::append_llm_chat_usage` already maintains per-session
   `llm_total` / `llm_turns` on disk (`usage.rs:373-387`), so a cap survives a
   restart mid-session.
7. **Ledger.** Every dispatch appends `provider_call_started` and one of
   `succeeded`/`failed`/`cancelled` through a new `assist_data_movement.rs`
   modeled on `projection_data_movement.rs:211-330`. This closes a verified
   hole that also exists for manual chat today (constraint 11) — B7 fixes both,
   because they share the dispatch site.
8. **No semantic/LLM-judge routing, ever, for this feature.** ADR-0038 rejected
   capability-negotiated selection as "silent fallback in a costume"; a judge
   call is itself a content-bearing egress. The grade is a deterministic
   function of fields we already have.

---

## 7. Graph-grounded retrieval — a first-class input

Today's context assembly (`prepare_chat_request`, `commands.rs:3179-3240`):
top-40 `build_graph_chat_context(snapshot, message, 40)` + the **last 10**
transcript segments + the **whole global** `chat_history`. Three defects for
this feature: notes are absent entirely (confirmed by the runtime synthesis
§5.4), the transcript window is the tail rather than the question's
neighbourhood, and the history is cross-contaminated across unrelated
questions.

**v1 `AssistRetrieval`** (new `src-tauri/src/assist_retrieval.rs`), assembled
in Rust from the thread's own anchors:

1. **Anchored transcript window** — spans around `card.source_span_ids`
   (default 6 before / 2 after) from the `TranscriptLedger` snapshot
   (`runtime.transcript_ledger_snapshot()`, the same handle
   `approved_agent_projection_patch` uses at :4069), *not* the tail of
   `transcript_buffer`. Highest-value single change: a question needs the words
   around it.
2. **Query-conditioned graph top-k** — `build_graph_chat_context(snapshot,
   question, 40)` reused verbatim. Already the right shape.
3. **Notes slice** — the notes outline headings plus at most 3 note bodies
   whose `evidence.span_id` falls inside the anchored window. Closes the
   documented gap with a join over data already in hand, not a new engine.
4. **Thread-local history** — the turns of *this* thread only.

The assembled bundle emits an `AssistRetrievalManifest { span_ids, note_ids,
graph_node_ids, basis }` persisted on the Answer turn, which (a) makes each
answer's Evidence Annotations derivable rather than model-asserted, (b)
finally populates `graph_context_ids` on the card head — always empty today
(`commands.rs:3985`) — and (c) satisfies ADR-0037's anchoring for an answer
that is, in CONTEXT.md's vocabulary, a **Grounded Inference**: default
`ClaimClass::GroundedInference` with the resolved `span_id`, degrading to
`UnavailableEvidence` with a reason note when a cited span has left the ledger
— the exact honest-degradation pattern `live_assist_evidence_anchor` already
implements (:4031-4066). Reuse that function; do not write a second one.

**Interaction with the architecture-review "Admitted Transcript read model"
candidate:** v1 reads the ledger snapshot directly inside
`assist_retrieval::anchored_window()`. When the read model lands, it replaces
that one function body and nothing else — same seam discipline W9 used for
104f. Recorded, not designed here.

**DEFER:** embeddings / vector retrieval (no store, no eval harness),
cross-session User World retrieval (needs World Promotion, out of epic),
whole-transcript retrieval (the cfa1 lesson), and any retrieval that reads the
materialized graph artifact from disk on the live path.

---

## 8. The write-path boundary

Explicit rules, each with a test:

1. **The answer path writes only Session Artifacts, never projections.**
   `turns.jsonl` + the card head. No `ProjectionOperation`, no
   `apply_runtime_projection_patch`, no `projection-patch` event. (I5)
2. **Promotion goes through the shipped approve path, unchanged.**
   `promote_assist_answer` mints an `AgentProposalKind::Note` into
   `pending_agent_proposals`; the user then clicks the existing "Add to graph",
   which runs the unmodified `approve_agent_proposal` →
   `approved_agent_projection_patch`. One graph door, still.
3. **No auto-promotion, ever, in v1.** Auto-answer authorizes *a paid read*, not
   a write. (The live-workspace synthesis §5 already lists "auto-apply with
   undo" as a non-goal needing its own ratification; this design does not
   reopen it.)
4. **No tools in v1.** `route.rs:646-649` keeps mapping `tool_calls → Failed`;
   we send none. When b437 lands read-only tools, they are still reads.
5. **Noted, not changed:** `add_question_to_graph` already writes a `Question`
   node automatically for every question card, with no approval
   (`store/index.ts:1800-1808`). It is local and LLM-free, so it is not new
   egress — but with auto-answer on, a single detected question now produces
   two automatic effects. The tile must therefore stop presenting
   "✓ question added to graph" (`AgentProposalsPanel.tsx:360-362`) as the
   card's only feedback and show the thread state as the primary signal.

---

## 9. The agent-runtime seam (b437 / U1-U7)

**v1 uses the existing single-turn substrate** — `spawn_stream_task`
(`commands.rs:3380-3572`), the five shipped providers, the ordered
`ipc::Channel`, `stream_registry` cancellation. It does **not** use
`start_streaming_chat` itself, for three reasons that are facts, not
preferences: that command cancels all priors (constraint 7), it hardcodes
`persist_to_history: true` into `chat_history`, and it stamps the `"llm_chat"`
action id we need to keep distinct for the egress gate.

The seam is left open in three specific places, each additive:

- `AssistTurnRecord.turn_kind` already admits `ToolCall`/`ToolResult` and
  `tool_calls` is a `#[serde(default)]` empty vec — U1-U7's tool lifecycle
  appends into the same stream with **no schema break and no migration**.
- `AssistAnswerRequest` (the struct B4's dispatcher takes) is the one type the
  future Rust loop replaces one-for-one; the gate, the route, the ledger, and
  the turn sink stay exactly where they are.
- The thread's `Answering` state already tolerates N provider calls per edge
  becoming M>1 — the ledger's 1:1 rule is per *call*, not per edge, so a
  multi-turn loop is a counting change, not a contract change.

What v1 explicitly does not do: flip the `tool_calls` terminal-status
invariant, add `tools`/`tool_choice` serialization, or build the localhost
listener from the router-proxy sketch (U3-U5 — needed only by an
out-of-process client, which this design does not introduce).

---

## 10. Dependency honesty

| Dependency | Relationship | If it slips |
|---|---|---|
| **W9 `classifyQueueEntry`** (shipped, `agentQueue.ts:221-241`) | Stays the **display** filter. B3's backend `signal` is the **spend** gate. Recommend auto-answer require *both* Strong **and** FE-actionable for one release, so the two can only be conservative together. | Already landed. |
| **104f** (fragment fix, P1) | Improves `agent_proposal_kind` *upstream* of the grade. It **does not block B1/B2** (the chatbox) and it is not in the auto-answer critical path — B3 ports W9's shipped rules, which exist today. | Auto-answer quality is worse; caps and the off switch bound the damage. Do not serialize the chatbox behind it. |
| **81a5 telemetry** (parallel) | B3/B4 emit house-style counters (`assist_answer.dispatch thread_id= grade= dispatch= elapsed_ms=`, `assist_answer.refused reason=`). These are the **first** log lines this subsystem has ever had (round-5 S9). | Ship the log lines in-house; hand the counter names to 81a5 later. Never block on it. |
| **`audio-graph-cfa1` / S2 load-path work** | Already partly landed (artifact ceilings + read logging at `commands.rs:112-150,7972-8012`). B1's bounded-head + separate-stream design is calibrated to it. | No coupling. |
| **b437 agent runtime** | Seam only (§9). | No coupling. |

**One Rust lane at a time** (project memory): B1, B3, B4, B5, B6, B7, B9 are
all Rust-compiling. They must be serialized on this box; B2 and B8 are the only
lanes that can run alongside a Rust lane, and only via the individual frontend
scripts (not `verify:fast`, which shells to cargo).

---

## 11. Explicitly deferred

Marked so the judge can cut cleanly.

**v1 (this epic):** B1 thread model · B2 tile composer + thread rendering ·
B3 backend signal grade · B4 answer command + turn sink · B5 auto-answer policy
· B6 anchored retrieval (window + notes + thread-local history) · B7 ledger
rows.

**v1.1 (same epic, after the field sees v1):** B8 Sessions `assist` lens +
`ChatSidebar` deletion · B9 answer→proposal promotion.

**Deferred out of the epic, deliberately:**
- Tool-calling / multi-turn agent loop (b437 U1-U7). Seam only.
- Speak-aloud on assist answers. `spawn_stream_task` already supports it
  (`SpeakAloudPipe`, :3440-3460) — but an *automatic* TTS burst during a live
  meeting is a product decision nobody has made. Wire the seam, ship it off.
- Auto-apply / auto-promotion with undo (already a named non-goal).
- Embedding/vector retrieval; cross-session User World retrieval.
- Thread split/merge, thread rename, cross-thread references.
- Multiple concurrent answers (`max_concurrent > 1`).
- Any second wire skin or the localhost listener (ADR-0038 / router-proxy
  U3-U5).
- Making the `agent` tile resizable/collapsible so the composer has room —
  that is phase-2 bento, and v1 must be legible in the *fixed* layout.
- Retiring `chat_history` from the blocking `send_chat_message` path. It stays
  until B8 removes its last reader.

---

## 12. Ticket-shaped units

Sizes are S (≤1 day) / M (2-4 days) / L (≥1 week) at this repo's test standard.

### B1 — Assist Thread data model + turn store · Rust · **M**
- **Scope:** `AssistThreadState`, `AssistOrigin`, `AssistTurnKind`,
  `AssistTurnRecord`, `AssistRetrievalManifest`, `SignalGrade`; additive
  `#[serde(default)]` fields on `LiveAssistCardRecord`; `turns.jsonl`
  path + append + per-thread load; `MAX_ASSIST_TURNS_BYTES`; widen
  `validate_live_assist_card` for `origin: UserPrompt` (see **D3**);
  `load_assist_thread` command.
- **Files:** `src-tauri/src/events.rs` (:435-490), `persistence/mod.rs`
  (:335-360, :995-1010, :1442-1490), `user_data.rs` (:219-230),
  `commands.rs` (:112-150 ceilings, new command), `src/types/index.ts` +
  the generated IPC contract crate if the type crosses it.
- **Gates:** cargo unit — round-trip append/load; a pre-thread
  `current.json` fixture deserializes with defaults (ADR-0027); oversized
  `turns.jsonl` refuses with `ArtifactTooLarge` and does **not** blank the
  card heads; `validate_live_assist_card` accepts/rejects the new origin cases.
- **Deps:** none. **Rust lane.**

### B2 — Composer + thread rendering in the agent tile · FE · **M**
- **Scope:** split `AgentProposalsPanel`'s body into scroll region (keeping
  `AGENT_QUEUE_PANEL_ID`) + pinned composer, rendered in **all** states
  including idle; new `AssistThreadRow` (collapsed preview / expanded turns
  with lazy fetch); thread-aware additions to `selectAgentQueue` (still pure);
  store actions + thread slice; header cost chip; i18n keys in en **and** pt.
- **Files:** `src/components/AgentProposalsPanel.tsx` (:299-420, :544-648),
  new `src/components/workspace/AssistThreadRow.tsx`,
  `src/components/workspace/agentQueue.ts`, `src/store/index.ts`,
  `src/i18n/locales/{en,pt}.json`, `src/App.tsx:661-678` (header slot only).
- **Gates:** vitest (`agentQueue.test.ts` extended; new panel tests),
  `src/i18n/locale-parity.test.ts`, a11y — `role="log"`/`aria-live` present
  once, Signal/All roving tabindex unchanged, composer outside the
  `aria-controls` panel; `WorkspaceTile.test.tsx` frozen-id test still green.
- **Deps:** B1 for types (can start against hand-written TS). **FE lane —
  the only unit that can run beside a Rust lane.**

### B3 — Backend signal grade · Rust · **S**
- **Scope:** `agent_signal_grade(&AgentProposalPayload) -> SignalGrade` next to
  `agent_proposal_kind`, porting W9's shipped constants (0.5 / 4 tokens / 16
  chars / dangling-clause) — **no English word lists**; stamp it on the
  payload at mint time; the first two log lines this subsystem has had.
- **Files:** `src-tauri/src/speech/mod.rs` (:1073-1140, near
  `agent_proposal_title`/`_body`), `events.rs`.
- **Gates:** cargo unit tests mirroring `agentQueue.test.ts`'s fixtures 1:1
  (same inputs, both languages, asserted equal classification) so FE and BE
  cannot drift silently; log lines assert no transcript content
  (`text_len=` house style).
- **Deps:** none (B1 for the `signal` field placement). **Rust lane.**

### B4 — `start_assist_answer` + dispatcher + turn sink · Rust · **L**
- **Scope:** the three dispatch commands; a single-slot,
  user-preemptible `AssistAnswerQueue`; `route.assist_answer` row; new egress
  action ids; a `TurnSink` that streams deltas over the channel and writes one
  `turns.jsonl` row + one head update at terminal; typed reason classes; usage
  increment.
- **Files:** `src-tauri/src/commands.rs` (:3380-3572 reuse, :841 gate, new
  commands), `src-tauri/src/llm/route.rs` (:188-281), `events.rs`, new
  `src-tauri/src/assist.rs` (queue + policy plumbing).
- **Gates:** privacy-mode-blocked ⇒ **zero** provider calls and a
  `PRIVACY_POLICY_BLOCKED` event; cancel mid-stream tears down upstream; **an
  automatic dispatch never cancels a user stream** (regression test against
  `cancel_all` at :3395); exactly one turn row per answer; zero
  `projection-patch` events (I5); serde round-trip of unknown
  `AssistStreamEvent` variants (probe P4).
- **Deps:** B1. **Rust lane.**

### B5 — Auto-answer authorization policy · Rust + FE(settings) · **M**
- **Scope:** `AutoAnswerPolicy` in settings + migration default; the ordered
  gate; per-minute token bucket; per-session count; durable token budget read
  from `sessions::usage`; off switch command + Settings row + header
  click-through.
- **Files:** `src-tauri/src/settings/mod.rs` (:1231-1345),
  `src-tauri/src/assist.rs`, `commands.rs`, `src/components/Settings*`,
  i18n.
- **Gates:** gate-off ⇒ no dispatch; each cap ⇒ the *specific* typed reason;
  over-budget renders, never silently skips; a settings-migration test proving
  an old settings file gets the documented defaults.
- **Deps:** B3, B4. **Rust lane.**

### B6 — Anchored retrieval bundle · Rust · **M**
- **Scope:** new `assist_retrieval.rs` (anchored window, notes slice, manifest);
  refactor `prepare_chat_request`'s context assembly into a shared
  `assemble_assist_context` so chat and assist cannot drift; thread-local
  history; `graph_context_ids` finally populated; evidence anchors via the
  existing `live_assist_evidence_anchor`.
- **Files:** new `src-tauri/src/assist_retrieval.rs`, `commands.rs`
  (:3179-3240, :4031-4066 reuse), `graph/entities.rs` (read-only reuse).
- **Gates:** every manifest span id resolves in the ledger snapshot; notes
  slice honours the anchored window; prompt size clamped by
  `clamp_completion_budget`; **no note/transcript text in any log line**;
  a degraded-anchor case yields `UnavailableEvidence` with a reason note.
- **Deps:** B1, B4. **Rust lane.**

### B7 — Data-movement ledger rows for the assist/chat producer · Rust · **S**
- **Scope:** `assist_data_movement.rs` modeled on
  `projection_data_movement.rs:211-330`; `provider_call_started/succeeded/
  failed/cancelled` with `["transcript_text","graph_context","notes","prompts",
  "usage_metadata"]`; register the new producer in the ADR-0034 inventory doc.
  Covers manual chat in the same change (shared dispatch site) — closing a
  pre-existing hole.
- **Files:** new `src-tauri/src/assist_data_movement.rs`, `commands.rs`,
  `docs/adr/0034-*` More-Information producer list.
- **Gates:** rows 1:1 with provider calls under a multi-answer test; Route lens
  renders the new rows; ceiling on the ledger still honoured.
- **Deps:** B4. **Rust lane.**

### B8 — Sessions `assist` lens + `ChatSidebar` deletion · FE · **M**
- **Scope:** `DETAIL_LENSES += "assist"` + icon; render `AssistThreadPanel`
  read-only over loaded threads; delete `askAvailable`, `askOpen`, the aside,
  `ChatSidebar.tsx` and its test; one honest read-only copy string replacing
  `chat.reviewSendBlocked`.
- **Files:** `src/components/SessionsBrowser.tsx` (:208-230, :662, :719-745,
  :825-828), delete `src/components/ChatSidebar.tsx` (+ test),
  `e2e/specs/shell.e2e.ts`, `App.contract.test.tsx` if lens ids are pinned,
  i18n both locales.
- **Gates:** e2e lens matrix at 1440/1024/768 light+dark; keyboard-only lens
  traversal; no dangling i18n keys (parity test); no orphan imports.
- **Deps:** B1, B2, B4. **FE lane.**

### B9 — Answer → proposal promotion · Rust + FE · **S**
- **Scope:** `promote_assist_answer` mints a `Note` proposal carrying the
  answer + its evidence into `pending_agent_proposals`; FE action on an
  answered thread; approve path untouched.
- **Files:** `commands.rs` (near :4350), `AssistThreadRow.tsx`, i18n.
- **Gates:** promotion emits zero `ProjectionOperation`s until approve; the
  approved patch's provenance/evidence snapshot matches the existing
  `approve_agent_proposal_impl` tests (:12091); dismiss-after-promote leaves
  the turn readable.
- **Deps:** B4. **Rust lane.**

**Suggested serialization (one Rust lane at a time):**
B1 → B3 → B4 → B5 → B6 → B7 → B9, with B2 running alongside B3/B4 and B8 after
B4 lands. First field-visible value at B2+B4; auto-answer becomes real at B5.

---

## 13. Open decisions for the maintainer

| # | Decision | My recommendation | Why it needs you |
|---|---|---|---|
| **D1** | Auto-answer default **on** (with caps) or **off** for the first release? | **On**, `min_grade: Strong`, 3/min, 20/session, visible chip + off switch. | Off-by-default would make the ratified feature invisible in the field — the exact round-5 failure. But on-by-default spends money in a subsystem that had zero telemetry three days ago. This is a risk call, not a design one. |
| **D2** | Do auto-answers speak aloud when `speak_aloud` is on? | **No** in v1; wire the seam, ship it off. | `spawn_stream_task` already drives `SpeakAloudPipe`; an automatic TTS burst mid-meeting is a product judgement nobody has made. |
| **D3** | Widen `validate_live_assist_card` (`persistence/mod.rs:346-351`) so a `UserPrompt` card with no cited span is admissible? | **Yes**, narrowly: empty `source_segment_id` permitted **only** when `origin == UserPrompt`, test-pinned. | It relaxes a shipped admission rule that ADR-0037 cites as its layering precedent. Alternative (v1 refuses free-form questions until at least one span exists) is honest but a bad product. |
| **D4** | Is one thread per card enough, or does a follow-up that changes subject need to fork a new thread? | One thread per card in v1; no split/merge. | Cheap to live with, expensive to retrofit if you disagree. |
| **D5** | Should the FE W9 filter and the BE grade be required to agree (auto-answer needs Strong **and** FE-actionable), or is BE alone authoritative? | **Both**, for one release. | Belt-and-braces costs one boolean; disagreeing gates that both spend money cost a field incident. |
| **D6** | Does the `--features cloud` light build ship the assist composer? | Yes — zero size cost; it is the same route table. | Fixes the feature matrix (runtime-synthesis §6.5, still open). |
| **D7** | `NotAuthorized{budget_exhausted}` — does the tile offer a one-click "spend anyway" manual Ask? | **Yes**, manual Ask always available; caps bind only automatic dispatch. | It is the difference between a budget and a lockout. |

---

## 14. Honest weaknesses of this angle

1. **Slowest path to the field-visible fix.** B1 and B4 are Rust units that
   ship no pixels. A view-first angle puts a working composer in the agent tile
   in days by reusing `chatMessages`; Angle B gets there at B2+B4. If the
   maintainer's priority is "make the chatbox exist this week," this design is
   the wrong shape and the judge should say so.
2. **It spends the agent tile's just-ratified "zero new store state" budget**
   (`agentQueue.ts:8-12`, design-b §4.3). Two-axis card state
   (`status` × `thread_state`) plus a thread slice is more state than W8
   promised, three days after W8 promised it.
3. **A third artifact stream in `live_assist/`.** Every new stream is an
   ADR-0027 migration story, a load-path ceiling, a `SessionExportBundle`
   field, an orphan-cleanup case, and a Route-lens row. That cost is real and I
   am choosing to pay it rather than embed turns in `current.json` — but "just
   embed them, the ceiling is 8 MiB" is a defensible cheaper answer for a
   20-answer cap.
4. **D3 widens a shipped admission rule.** ADR-0037 cites the live-assist
   citation rule as its layering precedent; relaxing it for user-authored cards
   is a small, test-pinned change to a rule with ADR standing. It should be
   reviewed as a contract change, not plumbing.
5. **Anchored retrieval (±6/±2 spans) is an unevaluated heuristic.** For a
   question answered much later in the conversation it may be strictly worse
   than today's tail-10, and this design ships no eval harness to detect that.
   The manifest at least makes the failure auditable after the fact.
6. **The preemption rule changes a documented invariant.**
   `start_streaming_chat`'s `cancel_all()` (:3395) is deliberate and
   test-adjacent. Introducing a second dispatcher with a "never preempt the
   user" rule means two policies about one registry; getting the interleaving
   wrong produces exactly the kind of silent cancellation this epic exists to
   stop.
7. **The grade is graded twice.** B3's Rust port and W9's shipped TS heuristic
   are two implementations of one rule, kept honest only by mirrored fixtures.
   That is a standing drift source until 104f collapses them.
8. **Auto-answer under a 20/session cap will feel arbitrary the first time it
   binds.** A user watching card 21 refuse with "session limit" has no mental
   model for why 20. The typed reason makes it legible, not justified.
9. **`turn_count`/`answer_preview` on the head are denormalized** and can drift
   from `turns.jsonl` if a write half-fails (append succeeds, head rewrite
   fails). The append-only stream is the source of truth, so the recovery is a
   rebuild-head pass — designed, but not written here.
10. **This design touches nine files in Rust before any of it is fun.** If the
    epic gets cut for time, the honest minimum that still fixes the field bug
    is **B1 (heads + turns) + B2 (composer + threads) + B4 (manual answer
    only)** — auto-answer, retrieval upgrades, and the ledger are all
    severable. The judge should know that the auto-answer half of the ratified
    shape is the severable half, not the thread half.
