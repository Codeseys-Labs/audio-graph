# AudioGraph's existing agent/LLM layer — seams for the 83cc runtime decision

**Verdict.** AudioGraph already has a mature, Rust-native, provider-agnostic LLM
substrate — streaming, structured-output/schema enforcement, prompt-cache
prefix discipline, a formal retry/truncation taxonomy, and privacy-mode content
egress gating — sitting entirely inside `src-tauri` with keys custodied in the
OS keyring and never touched by a webview or a Node process. There is **no
sidecar of any kind today** (no `externalBin`, no shell-plugin invocation, no
Node/pi-agent process) — the trust boundary is currently exactly "webview
(untrusted input) → Tauri command → Rust process holding the key → HTTPS to
provider," and that boundary is enforced by an explicit gate
(`enforce_session_content_policy`) that fires before any content-bearing
dispatch, cloud or local. What does **not** exist yet, on either fork of the
83cc decision, is a multi-step *agentic* loop: today's sanctioned chat route is
single-shot text generation over a **server-assembled** context string (top-k
graph nodes + last 10 transcript segments); the model cannot call a tool to
fetch more graph/transcript/notes context mid-turn, and materialized notes
(the document tile) are **not** included in that context at all. Adopting
pi-agent would mean either (a) giving a second, TypeScript-side process/module
read access to transcript/notes/graph state and routing its own LLM calls —
which duplicates the entire provider-routing/retry/cache/schema stack above
and, if it makes its own HTTP calls, either re-implements key custody in TS or
requires a new Rust→TS key-handoff that changes the trust boundary named in
the SECURITY FACTS — or (b) using pi-agent purely as a client library that
calls back into the existing Rust commands for every LLM operation, which
keeps the trust boundary intact but means pi-agent contributes only the
tool-loop/turn-orchestration shape, not model access. A Rust-native loop reuses
100% of the substrate below with zero trust-boundary change and is the only
option compatible with the "local LLM inference lands in Rust" forward
constraint without a second migration.

---

## 1. What ships today: the question queue (not yet a chat agent)

The **agent tile** in the bento workspace is `AgentProposalsPanel.tsx`, mounted
inside `WorkspaceTile` (`src/components/workspace/WorkspaceTile.tsx:79-108`,
generic shell — no agent-specific logic). It renders two lists — an actionable
**queue** and a read-only **feed** — computed by a pure selector,
`selectAgentQueue` (`src/components/workspace/agentQueue.ts:345-403`). This is
NOT a chat surface: it classifies/dedupes/gates already-minted
`AgentProposalEvent`s (kind: `note` | `question` | `graph_suggestion`,
produced server-side by `run_agent_proposal_task` in `speech/mod.rs`, per the
module doc at `agentQueue.ts:89-102`). Rows offer **Approve** (writes to the
graph via `approve_agent_proposal`), **Dismiss**, and — for `question` kind
only — **Ask AI** (`src/components/AgentProposalsPanel.tsx:366-370`, i18n key
`agent.askAi`).

**The "Ask AI" button is the one existing bridge from the queue to an LLM
turn.** `askAgentProposal` (`src/store/index.ts:1810-1824`):

```ts
askAgentProposal: async (proposalId: string) => {
  const proposal = get().agentProposals.find((p) => p.id === proposalId);
  if (!proposal) return;
  const question = proposal.body?.replace(/^Consider answering or linking this question:\s*/i, "") || proposal.title;
  const dismissed = await get().dismissAgentProposal(proposalId);
  if (!dismissed) return;
  await get().sendChatMessage(question);
},
```

It dismisses the card, then routes the recovered question text through the
**general-purpose chat pipeline** (`sendChatMessage`) — the queue and "chat"
are not two systems today, they're one system with two entry points. This is
exactly the seam 83cc's "gated auto-answer flow" would extend: replace the
manual button click with an automatic dispatch gated by whatever policy 83cc
defines, feeding the same `sendChatMessage` call.

A separate, non-gated auto-write already exists for every `question`-kind
proposal regardless of user action: `addAgentProposal`
(`src/store/index.ts:1782-1809`) fires `invoke("add_question_to_graph", …)`
unconditionally the moment the proposal event lands, purely local/no-LLM. Only
the answer-fetch is optional/gated by the user (or, post-83cc, by an
auto-answer policy).

## 2. The actual chat-turn contract (this IS the 83cc "chat panel" substrate)

`sendChatMessage` (`src/store/index.ts:2786-2959`) is the real "agent turn"
primitive today:

- Optimistic user message + empty assistant placeholder pushed into
  `chatMessages` immediately (`store/index.ts:2801-2809`).
- Tries **`start_streaming_chat`** first via `rawInvoke` (deliberately not the
  `safeInvoke` diagnostics chokepoint — the backend returns `Err` for
  non-streaming-capable providers as expected control flow, not a failure;
  see the comment at `store/index.ts:2882-2892`), passing a
  `tauri::ipc::Channel<ChatStreamEvent>` created **before** the invoke so no
  delta frame can be lost to the old spawn-before-handler-registration race
  (`store/index.ts:2816-2826`, referencing fix `audio-graph-1534`).
- Per-token `delta` frames are coalesced client-side and flushed at most once
  per `CHAT_DELTA_THROTTLE_MS` (`store/index.ts:2833-2852`); the terminal
  `done` frame carries the authoritative `full_text` and can encode a
  mid-stream error (`finish_reason: "error: …"`, surfaced with a friendly
  429/rate-limit message at `store/index.ts:3011-3018`) or a user cancel
  (`finish_reason: "cancelled"`).
- If streaming isn't supported for the active provider, falls back to the
  blocking **`send_chat_message`** command, which internally re-runs the exact
  same streaming producer and drains it synchronously
  (`src-tauri/src/commands.rs:3519-3563` — "the shim doesn't fire IPC events
  itself; it consumes the channel directly").

`ChatMessage` is a minimal `{role: "user"|"assistant"|"system", content:
string}` (`src/types/index.ts:2654-2657`) — **no id, no timestamp, no tool-call
or metadata field.** `ChatStreamEvent` is a discriminated union of `delta`
(`ChatTokenDeltaEvent`) and `done` (`ChatTokenDoneEvent`, carrying
`usage.total_tokens`) frames (`src/types/index.ts:2674-2713`).

**Where this UI currently lives:** `ChatSidebar.tsx` — NOT inside the bento
`WorkspaceTile` grid. Its own doc comment: "Parent: `SessionsBrowser`'s 'Ask'
aside lens … the legacy `App.tsx` right-panel tab it used to live under …
was deleted" (`src/components/ChatSidebar.tsx:1-17`). So today there are two
separate agent-adjacent surfaces — the bento agent tile (queue/feed) and a
free-standing chat aside reached from the sessions browser — bridged only by
`askAgentProposal → sendChatMessage`. 83cc's "chat panel in the agent tile"
is a UI relocation/consolidation of an *existing, working* pipeline, not a
green-field integration.

## 3. Context assembly: graph + transcript, NOT notes (gap, independent of runtime choice)

`prepare_chat_request` (`src-tauri/src/commands.rs:2974-3037`) builds the
per-turn context:

1. Snapshots the knowledge graph (`state.knowledge_graph.lock()...snapshot()`).
2. Takes the **last 10** transcript segments (`transcript_buffer.read()...rev().take(10)`).
3. Calls `build_graph_chat_context(&snapshot, &message, MAX_CONTEXT_NODES=40)`
   (`src-tauri/src/graph/entities.rs:233-263`) — a **top-k relevance** selector:
   scores every graph node by query-token overlap (dominant term) plus
   `ln_1p(mention_count)` (centrality tiebreak for an empty/irrelevant query),
   sorts, takes the top 40. Comment at `commands.rs:2997-3000` is explicit
   about why: "Top-k retrieval instead of dumping the whole graph … avoids
   shipping maximal session data" (references fix `C3`).
4. Appends the recent-transcript block as `"[Speaker]: text\n"` lines onto the
   graph-context string (`commands.rs:3007-3013`).
5. Pushes the user message onto `state.chat_history` (capped at
   `MAX_CHAT_HISTORY = 200`, `commands.rs:3060-3068`) and returns the full
   history plus the graph-context string.

**Materialized notes (the document tile / `LiveDocument`) are not part of
this context at all.** Nothing in `prepare_chat_request` reads
`materializedNotes` or the notes-projection state. 83cc's requirement that the
agent have "context of live transcript, notes, and knowledge graph" is
therefore net-new context-assembly work no matter which runtime wins — it is
not a gap the runtime choice closes or reopens by itself, but a Rust-native
loop can add it as one more field threaded into the same
`prepare_chat_request`-shaped function; a TS runtime would need either a new
Tauri command that returns notes state or would have to read the frontend
Zustand store directly (which already holds `materializedNotes` client-side —
see `src/store/index.ts:1741-1745`, `setMaterializedNotes`).

There is also no tool-call loop: the model gets one shot at a
pre-assembled context string and one shot at a reply per `sendChatMessage`
invocation. Multi-step "let the agent decide what to fetch" behavior (e.g.
"search earlier in the transcript," "look up this entity's full history")
does not exist in the sanctioned route today — it would be new agentic-loop
work under either fork.

## 4. Key custody: physically in the Rust process, keyring-backed, zeroized

`src-tauri/src/credentials/mod.rs:1-13` (module doc): production desktop
builds use the OS credential store (macOS Keychain / Windows Credential
Manager / Linux Secret Service) via the `keyring` crate; legacy
`credentials.yaml` is a non-destructive import/fallback path. `CredentialStore`
derives `Zeroize`/`ZeroizeOnDrop` (`credentials/mod.rs:148` region) — "secrets
are zeroized in memory … mitigates exposure … in memory dumps, swap files, and
cold-boot attacks." A process-wide `CREDENTIAL_IO_LOCK`
(`credentials/mod.rs:39-68`) serializes read-modify-write so a tombstoned key
can't be resurrected by a racing probe. `ALLOWED_CREDENTIAL_KEYS`
(`credentials/mod.rs:108-131`) is a fixed allowlist (`openai_api_key`,
`openrouter_api_key`, `aws_access_key`, etc.), checked against
`is_allowed_key` at the command boundary and — as a build-time assertion — at
`src-tauri/src/provider_registry.rs:96-107` (`descriptor_credential_keys_are_allowed`,
runs every provider descriptor's `credential_keys` through the same allowlist).

**No sidecar process exists in this codebase.** `grep` for `externalBin` /
`"sidecar"` across `src-tauri` returns nothing; `tauri.conf.json` declares no
external binary. The only "other runtime" already in the repo is `bun`, used
purely for build/dev scripts (`package.json` — `vite`, `tsc`, generator
scripts) — it never touches API keys or session content. If pi-agent's own
process needs the key to call a provider directly, that is a **new** class of
component this repo has never shipped, and it is precisely the trust-boundary
change the SECURITY FACTS flag: a Node/TS process holding (or being handed)
the key changes "key lives in the Rust process" to "key lives in the Rust
process AND transiently in a TS process," even if only in memory.

## 5. Privacy-mode content-egress gate — fires before every content-bearing dispatch

`enforce_session_content_policy` (`src-tauri/src/commands.rs:663-706`) is
called at the top of both `start_streaming_chat`
(`commands.rs:3398-3406`) and `send_chat_message`
(`commands.rs:3499-3507`), passing `&["user_message", "transcript",
"graph_context", "prompt"]` as the `data_classes` being transferred. It
resolves to a hard `Err(AppError::PrivacyPolicyBlocked)` — plus a
content-free telemetry emit (`events::PRIVACY_POLICY_BLOCKED`,
`commands.rs:690-702`) — unless either the provider doesn't require cloud
transfer at all, or `settings.privacy_mode.allows_session_cloud_content_transfer()`
is true, which is true **only** for `PrivacyMode::ByokCloud`
(`src-tauri/src/settings/mod.rs:1178-1207`; the other three modes —
`LocalOnly`, `CloudDisabledReadinessOnly`, `OrgPromotion` — all block content
egress). `LlmProvider::requires_cloud_content_transfer()`
(`settings/mod.rs:634-640`) is `false` only for `LocalLlama`/`MistralRs`
(in-process local inference) and for an `Api` provider pointed at a loopback
endpoint; `OpenRouter`/`AwsBedrock` are always `true`.

**This gate is a single, narrow choke point today** — two call sites, both in
Rust, both before any HTTP dispatch. Any 83cc runtime — TS or Rust — that adds
a third way to reach the LLM (e.g. pi-agent's own turn loop calling a provider
directly, or calling a *different* Tauri command that skips this exact
function) reopens the exact class of bug ADR-0033 (`docs/adr/0033-enforce-mvp-provider-enablement-at-content-start.md`)
was written to close. A Rust-native agent loop can call
`enforce_session_content_policy` directly, in-process, with no new command
surface. A pi-agent integration must either (a) proxy every LLM call back
through `send_chat_message`/`start_streaming_chat` (or a new
equally-gated command) so this function still runs, or (b) reimplement
this exact privacy-mode/data-class check in TypeScript against the same
`AppSettings.privacy_mode` value it would need read access to.

## 6. Structured-output, prompt-cache, and retry machinery the sanctioned route already has for free

This is the substrate that a second, TS-side "agent runtime" would either
have to duplicate (if it makes its own model calls) or gets for free
(if it stays behind the existing commands):

**Structured outputs (JSON schema, provider-native strict mode).**
`projection_patch_strict_json_schema` (`src-tauri/src/projection_llm.rs:540-756`)
builds a per-`ProjectionKind` JSON Schema — nullable-but-required fields for
strict-mode compliance, an enum-bound `entity_type` closed to
`ontology::ENTITY_TYPES` (comment cites a field measurement of "31 invented
entity types in one session" pre-fix, `projection_llm.rs:566-570`), and a
compact evidence-anchor object added only to content-creating variants per
ADR-0037 (`projection_llm.rs:604-609`). This schema is wired into OpenRouter's
`response_format: {type: "json_schema", json_schema: {name, strict, schema}}`
(`src-tauri/src/llm/openrouter.rs:453-488`, `1610-1657`), which also pins
`provider.require_parameters = true` when a schema constraint is present so
OpenRouter won't silently route to a backend that ignores it
(`openrouter.rs:2425-2449`, test-verified). A malformed/out-of-schema
completion still fails deserialization at ingest (`projection_llm.rs:757-800`,
`parse_projection_patch_draft`) even on non-strict/local routes — belt-and-
suspenders, not schema-only trust.

**Prompt-cache prefix discipline.** `PROJECTION_STABLE_PREFIX_MESSAGE_COUNT = 2`
(`projection_llm.rs:44`) plus an explicit design rule, reiterated at
`projection_llm.rs:1511-1523` and `1599-1667`: the first two prompt messages
must stay byte-identical across turns so a provider cache breakpoint
(`cache_control`) placed after them keeps hitting; variable per-turn content
(notes snapshot, per-job id) is pinned to live strictly **after** the stable
prefix or in the *last* message specifically so it never busts the cache
(ADR-0025 §2d, seed `audio-graph-d77e`). Three dedicated tests pin this
invariant byte-for-byte (`projection_llm.rs:3109-3287`,
`stable_prefix_is_byte_identical_across_appended_turns`,
`notes_snapshot_placement_never_busts_the_cache_stable_prefix`,
`heading_level_change_between_ticks_never_busts_the_stable_prefix`).

**Retry/truncation taxonomy.** `src-tauri/src/llm/route.rs:590-730` defines
`TerminalStatus` (`Completed`/`Truncated`/`Refused`/`Failed`/`TransportLost`)
and `RetryClass` (`PermanentRejection`/`TransientAvailability`/
`UnusableCompletion`/`ExternalEffectUnknown`), with
`retry_class_for_terminal_status` mapping one to the other and
`auto_retry_permitted()` gating exactly which class may be auto-retried
(only `TransientAvailability` — 408/409/429/5xx/connect-phase failures;
`UnusableCompletion` and `ExternalEffectUnknown` are explicitly NOT
auto-retried, per `route.rs:696-711` and the retry-progression note crediting
seed `audio-graph-3b48`). `TerminalStatus::Truncated` is detected from
`finish_reason` strings `"length" | "max_tokens" | "model_length"`
(`route.rs:644`). `llm/executor.rs` layers a same-route "draft, then repair"
attempt on top (`executor.rs:790-830`, `stamp_attempted_route` folding logic
at `1018-1063`) so a structurally-invalid draft gets one same-route repair
try with the *identical* resolved provider identity ledgered for audit.

**Provider abstraction.** One `LlmProvider` enum (`LocalLlama`, `Api` incl.
Cerebras/SambaNova presets, `OpenRouter`, `AwsBedrock`, `MistralRs`) maps to a
single `ProviderDescriptor` registry shared with the frontend via a generated
TS module (`src-tauri/src/provider_registry.rs:32-46`,
`src/generated/providerRegistry.ts`, kept in sync by a build-time equality
test at `provider_registry.rs:322-339`). `ensure_llm_provider_start_enabled`
(`provider_registry.rs:82-84`) gates provider readiness before any dispatch,
independent of the privacy-mode gate.

**None of this exists on the TypeScript side.** A pi-agent integration that
dispatches its own model calls would need to reimplement or vendor equivalents
of all five items above (schema/strict-mode wiring, cache-prefix discipline,
the retry taxonomy, provider abstraction, and the readiness gate) to reach
parity — or it must not make model calls itself and instead stay a
turn-orchestration layer over the existing Rust commands.

## 7. IPC surface available today

Three distinct mechanisms, already in production use, none of them new:

1. **One-shot `invoke()` commands** — `approve_agent_proposal`,
   `dismiss_agent_proposal`, `clear_agent_proposals`, `add_question_to_graph`,
   `send_chat_message`, `clear_chat_history`
   (`src-tauri/src/commands.rs:3949, 4315, 4346, 4145, 3487, 3748`; call sites
   `src/store/index.ts:1810-1929`). Request/response, no partial delivery.
2. **`tauri::ipc::Channel<T>` for per-token streaming** — `start_streaming_chat`
   (`commands.rs:3385-3453`) takes a `Channel<ChatStreamEvent>` argument created
   client-side before the invoke (`store/index.ts:2868-2896`); this replaced an
   older event-based hot path specifically to close a spawn-before-
   handler-registration race (fix `audio-graph-1534`, noted at
   `commands.rs:3483-3485` and `store/index.ts:2816-2826`).
3. **Global `listen()` broadcast events** — `agent-status` and `agent-proposal`
   (`src/hooks/useTauriEvents.ts:140-141`, wired at `useTauriEvents.ts:326-335`
   to `setAgentStatus`/`addAgentProposal`), alongside `transcript-update`,
   `graph-update`, `graph-delta`, `projection-patch`, etc. These are
   session-wide push notifications, not tied to a specific request.

A chat-turn today uses (2) for the hot path with (1) as a documented fallback
for providers without a streaming code path; the agent-proposal queue uses
(3) for arrival and (1) for every user action on a card. There is no
WebSocket, gRPC, or other transport in the app — everything is Tauri's own
IPC. **There is no sidecar usage of any kind** (confirmed by absence in
`tauri.conf.json` and by `grep -rn "sidecar\|externalBin"` returning nothing
in `src-tauri`) — the only adjacent "second engine" pattern in the repo is the
Gemini Live / OpenAI Realtime speech-to-speech provider
(`src-tauri/src/openai_realtime/mod.rs`, registry ids
`realtime_agent.gemini_live` / `realtime_agent.openai_realtime`,
`provider_registry.rs:310-320` — both currently gated `ProviderDeferred`, i.e.
registered but not yet enabled for users), which is a direct
Rust→cloud-WebSocket connection, not a sidecar process either.

## Implications for the 83cc runtime decision

- **Trust boundary is the load-bearing constraint, not developer convenience.**
  Every fact above — keyring custody, zeroize-on-drop, the single
  `enforce_session_content_policy` choke point, the absence of any sidecar —
  describes a boundary that currently has exactly one hole an attacker or bug
  could exploit to leak a key or session content: the Rust process itself.
  Adopting pi-agent as a same-process TypeScript module has no Rust key
  access and is not a boundary change by itself; adopting it as a
  **separate process** (any pi-agent deployment shape that runs its own
  Node/Bun runtime with its own provider credentials or its own outbound HTTP)
  is a boundary change and must be named as such per the task's SECURITY
  FACTS — it would be the first sidecar this codebase has ever shipped.
- **A Rust-native agent loop is the only fork with zero duplication cost.**
  Sections 6 and 5 above are ~9,000 lines of already-tested, ADR-governed
  machinery (schema enforcement, cache-prefix discipline, retry taxonomy,
  provider registry, privacy gate) that a Rust-native loop calls directly.
  Any TS runtime that makes its own model calls re-derives all of it or ships
  with weaker guarantees (e.g. no stable-prefix cache discipline, a different
  or absent retry taxonomy, a reimplemented privacy gate that can drift from
  the Rust one over time since they're not the same code).
  A TS runtime that does NOT make its own model calls (pure orchestration,
  proxying every LLM operation through `send_chat_message`/
  `start_streaming_chat` or new equally-gated commands) avoids duplication
  but then contributes only turn/tool-loop shape — the harder question becomes
  whether pi-agent's value proposition survives being reduced to an
  orchestration shim with no direct model access.
- **The forward local-inference constraint favors Rust-native.** The
  provider abstraction already treats `LocalLlama`/`MistralRs` as first-class,
  zero-cloud-egress members of the same enum as cloud providers
  (`settings/mod.rs:636`). A future Rust-native vLLM-class server would slot
  into this enum exactly like `MistralRs` does today. A chat/agent runtime
  that already lives in Rust inherits that slot automatically; one that lives
  in TypeScript needs either a second local-inference client (duplicating
  provider-routing work a second time) or a round-trip back into Rust for
  every local-model call anyway — at which point the TS runtime is doing
  strictly less than a Rust-native one for the same request.
- **Neither fork closes the two real gaps found here.** (1) Chat context
  assembly excludes materialized notes (§3) — new work regardless of runtime.
  (2) There is no tool-call/multi-step loop in the sanctioned route today —
  building one is new work regardless of runtime; the only question the
  runtime choice answers is whether that loop's *implementation* sits in Rust
  (reusing §5/§6 directly) or in TypeScript (calling back into §5/§6's Tauri
  commands, or duplicating them).
- **The queue-to-chat bridge (`askAgentProposal`, §1) is the concrete hook
  point for "gated auto-answer."** Whatever gate 83cc designs, it slots in
  exactly where the manual "Ask AI" button click currently sits — this is
  true under either runtime fork, since the bridge is a frontend Zustand
  action calling `sendChatMessage`, not backend logic.
