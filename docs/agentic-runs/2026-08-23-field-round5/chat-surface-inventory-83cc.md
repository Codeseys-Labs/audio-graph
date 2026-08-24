# Chat/agent-surface inventory for epic 83cc (agent-chat design panel)

Read-only recon, current source state as of commit `b31109f` (bento workspace
landed through ticket W9 / `audio-graph-a6b5`). File:line pointers throughout;
no code changed to produce this document.

## 1. Every current agent/chat surface

### Agent proposal surface — the bento "agent" tile

The live capture workspace is a 4-tile bento grid (`WorkspaceTile`s: document,
graph, agent, transcript — App.tsx:617-684). The agent tile is **always
mounted**, R3-ratified (App.tsx:656-660 comment), and renders
`AgentProposalsPanel` as its body:

```
<WorkspaceTile id="agent" title={t("agent.title")} headerSlot={...}>
  <AgentProposalsPanel filter={agentQueueFilter} />
</WorkspaceTile>
```
— App.tsx:661-678.

**Header slot** (App.tsx:664-675) composes two controls into `WorkspaceTile`'s
single `headerSlot` region:
- `AgentQueueFilterToggle` (AgentProposalsPanel.tsx:145-201) — a Signal/All
  `role="tablist"` toggle (ticket W9), full APG roving-tabindex keyboard
  contract, persisted to `localStorage` key `ag.agentQueueFilter`
  (AgentProposalsPanel.tsx:57, 86-93). Lifted once in `App.tsx` via
  `useAgentQueueFilter()` (App.tsx:523) so the header toggle and the panel
  body never desync (same pattern as `useGraphStripMode`/`useLiveDocumentModel`).
- `AgentTileHeaderActions` (AgentProposalsPanel.tsx:510-530) — a single
  "Clear" button (`agent.clear` → `clearAgentProposals`), rendered only when
  `agentProposals.length > 0`, disabled while any card is mid-approval.

**Panel body** (AgentProposalsPanel.tsx:544-648) splits into two lists via the
pure selector `selectAgentQueue` (workspace/agentQueue.ts:345-403):
- **Queue** (top, `agent.queueTitle`) — actionable cards only, rendered by
  `AgentQueueRow` (AgentProposalsPanel.tsx:328-402) with real per-card actions:
  - `kind === "question"` → **"Ask AI"** button (`agent.askAi`, calls
    `askAgentProposal(proposal.id)`) + **"Dismiss"** (`agent.dismiss`, calls
    `dismissAgentProposal`). A question card also shows a static
    "✓ question added to graph" line (AgentProposalsPanel.tsx:360-362) because
    questions are auto-recorded as graph nodes locally, no LLM call, before
    the card even renders (store/index.ts, `addAgentProposal` handler,
    ~1790-1808).
  - `kind === "graph_suggestion" | "note"` → **"Add to graph"** /
    "Applying…" while in flight (`agent.addToGraph`, calls
    `approveAgentProposal`) + **"Dismiss"**.
- **Feed** (below, `agent.feedTitle`) — read-only history via `AgentFeedRow`
  (AgentProposalsPanel.tsx:437-504): kind + status chip + optional
  fragment-suspect/duplicate-count markers, title (truncated), approved
  outcome text, projection-patch-sequence evidence, and a per-row
  details-disclosure toggle for the full proposal body. **No actions
  anywhere in the feed** — approve/ask/dismiss exist only in the queue
  (named deviation from the original design doc, recorded in the module
  comment at AgentProposalsPanel.tsx:425-436: an actionable
  `fragment_suspect` card that lands in the feed under Signal mode has no
  in-row recovery path, only the header's Signal/All toggle).
- **Idle state** (queue empty, feed empty, no agent status running) — a
  designed empty state (icon + `agent.idleTitle`/`agent.idleBody`), not
  `null` and not "coming soon" (AgentProposalsPanel.tsx:577-597).

**The queue/feed classification** (`classifyQueueEntry`,
workspace/agentQueue.ts:219-240, and the duplicate-collapse pass inside
`selectAgentQueue`) is the seed-104f fragment-suppression mechanism —
confidence floor 0.5, min 4 tokens / 16 chars, dangling-clause detection, all
scoped to `kind === "question"` only (workspace/agentQueue.ts:84-86,
193-217). Not itself a chat feature, but any 83cc chat-turn view needs to
know this quality gate exists upstream of what reaches the queue at all.

### The "Ask AI" → chat dismissal pipe

`askAgentProposal` (store/index.ts, `agentProposals` action block — the
`kind === "question"` branch begins immediately after the local
`add_question_to_graph` invoke, function itself at the line starting
`askAgentProposal: async (proposalId: string) => {`, ~store/index.ts:1810):

```
askAgentProposal: async (proposalId: string) => {
  const proposal = get().agentProposals.find((p) => p.id === proposalId);
  if (!proposal) return;
  const question = proposal.body?.replace(/^Consider answering.../i, "") || proposal.title;
  const dismissed = await get().dismissAgentProposal(proposalId);
  if (!dismissed) return;
  await get().sendChatMessage(question);
},
```

This is the **entire** bridge between "Ask AI" and chat: it dismisses the
card (preserved as a `dismissed` live-assist record — still visible read-only
in the feed) and then calls the exact same `sendChatMessage` that
`ChatSidebar`'s send button calls. **There is no dedicated chat surface
mounted anywhere in the live bento workspace.** Clicking "Ask AI" during live
capture pushes a user+assistant-placeholder pair into `chatMessages` store
state that nothing in the capture view renders — see §2.

### ChatSidebar — the only chat-turn rendering surface in the app

`ChatSidebar.tsx` (mounted exactly once, SessionsBrowser.tsx:825-828):

```
{canAsk && askOpen && (
  <div className="w-[320px] min-w-[260px] shrink-0 border-l ...">
    <ChatSidebar />
```

Gating chain, all in SessionsBrowser.tsx:
- `canAsk = askAvailable(lens)` (SessionsBrowser.tsx:719), where
  `askAvailable` (SessionsBrowser.tsx:223-227) returns true **only** for the
  `notes` and `graph` detail lenses — not `transcript`, `timeline`, or
  `route`.
- `askOpen` is local `useState(false)` (SessionsBrowser.tsx:662), toggled by
  a header button (`disabled={!canAsk}`, SessionsBrowser.tsx:744-745) — no
  store state, doesn't survive a lens switch away and back in a way that's
  externally observable/restorable.
- The entire `SessionDetail` (which is `ChatSidebar`'s only parent) renders a
  **lockout screen** instead of any content whenever `reviewLocked` is true
  (SessionsBrowser.tsx ~660-680, `sessions.reviewLockedWhileLive` copy) — and
  `reviewLocked` is driven by `isCapturing`/`isTranscribing` (plan §R2,
  ADR-0046: "concurrent Live+Review is not delivered"). **This means
  Sessions' Ask lens — the only chat UI in the app — is completely
  unreachable while a capture is live**, not merely un-mounted like the bento
  tile gap above.

`ChatSidebar.tsx` internals (doc comment lines 1-16):
- Reads `chatMessages`, `isChatLoading`, `sendChatMessage`, `clearChatHistory`,
  `graphSnapshot`, `loadedSessionId` from the store (ChatSidebar.tsx:26-34).
- `historicalReview = loadedSessionId !== null` (ChatSidebar.tsx:34).
- `handleSend` early-returns when `historicalReview` is true
  (ChatSidebar.tsx:52).
- The `<input>` and send `IconButton` are both `disabled={... ||
  historicalReview}` (ChatSidebar.tsx:192, 199), and a warning line
  (`chat.reviewSendBlocked`) renders above the input whenever
  `historicalReview` is true (ChatSidebar.tsx:172-179).

**Practical consequence**: `ChatSidebar` only ever mounts as a child of
`SessionDetail`, which only renders a real detail view once a row is
selected — and selecting a row calls `loadSession`, which sets
`loadedSessionId` (store/index.ts:3336). So in every real (non-sample-preview)
path a user can reach `ChatSidebar` through, `historicalReview` is already
true and the input is disabled on arrival. The one path where
`loadedSessionId` can be `null` while `SessionDetail` still renders content is
`samplePreviewActive` (SessionsBrowser.tsx's `!samplePreviewActive && !row`
empty-state guard implies sample preview bypasses the "no row" empty state) —
worth an explicit repro check before the design panel assumes chat input is
reachable at all today outside the sample-preview demo.

### Chat state lifecycle notes (not a surface, but load-bearing for any redesign)

- `chatMessages: []` is reset in the capture-start reset block
  (store/index.ts, block containing `agentProposals: [], liveAssistCards: [],
  ..., chatMessages: [], isChatLoading: false, streamingChatRequestId: null,
  loadedSessionId: null` — store/index.ts ~1960-1985) — i.e. starting a new
  capture clears chat.
- `loadSession`'s own `set(...)` block (store/index.ts:3308-3351) does **not**
  touch `chatMessages`/`isChatLoading`/`streamingChatRequestId` at all.
  Switching between two different loaded (historical) sessions in Sessions
  does not clear chat history between them.
- `LoadedSession` (the `load_session` return type, src-tauri/src/commands.rs)
  has no chat field — chat turns are **never persisted**. A live-capture
  "Ask AI" answer exists only in memory; stopping capture and later loading
  that same session's Notes/Graph lens plus opening Ask will show an empty
  chat, not the earlier exchange.

## 2. Where a free-text chat input would live

### The agent tile's contract (what's fixed vs. what's open)

`WorkspaceTileId` is a **frozen** union — `"transcript" | "graph" |
"document" | "agent"` (workspace/WorkspaceTile.tsx:14) — explicitly called
out as a public contract because phase-2's (unshipped)
`WorkspaceLayoutPrefs` will persist these four strings verbatim to
`localStorage` (key `ag.workspaceLayout`, reserved but unused —
workspace/WorkspaceTile.tsx:16-36). **Adding a fifth tile id, or renaming
"agent," is a breaking change to an already-committed persistence schema** —
`WorkspaceTile.test.tsx` pins the exact set. A chat surface should not be
pitched as tile id `"chat"`; it has to live *inside* the existing `"agent"`
tile (or as new content of one of the other three) rather than as a new tile,
unless the panel is prepared to revisit that frozen contract deliberately.

`WorkspaceTileProps` (workspace/WorkspaceTile.tsx:38-49) gives each tile
exactly **one** `headerSlot` (already double-occupied on the agent tile by
the Signal/All toggle + Clear button, per §1) and one `children` body region.
`WorkspaceTile`'s frame is `min-width:0; min-height:0; overflow:hidden`
(workspace/WorkspaceTile.tsx:69-71) — **the tile's own content owns its
scroll region**, not the shell. `AgentProposalsPanel`'s body is currently one
`overflow-y-auto` div containing both the queue and feed sections stacked
vertically (AgentProposalsPanel.tsx:599-647, `id={AGENT_QUEUE_PANEL_ID}`).

**Where a chat input would fit, structurally**: `ChatSidebar`'s existing
layout (ChatSidebar.tsx:71-207) is a 3-region flex column — fixed header,
flex-1 scrollable message log (`role="log"`, `aria-live="polite"`), fixed
input row pinned to the bottom (`border-t`, `shrink-0`). Reusing that shape
inside the agent tile means the agent tile's body can no longer be a single
scrolling column of queue+feed — it would need the same fixed-input-row-at-
bottom treatment, which pushes on the "one `children` slot, tile owns its own
scroll" contract but doesn't break it (nothing here is bento-grid-level; it's
internal to the tile's own body markup). Whether chat messages, the proposal
queue, and the feed all share one scroll region or get their own sub-scroll
areas inside the tile is an open design question this inventory intentionally
does not answer — it belongs to the design panel, not this recon pass.

### The W8/W9 queue seam, and why chat turns don't fit it as-is

`selectAgentQueue` (workspace/agentQueue.ts:345-403) is explicitly a
**zero-new-store-state, store-data-only** selector (design-b §4.3, "ZERO NEW
STORE STATE" — workspace/agentQueue.ts:8-12): it derives `queue`/`feed` purely
from the existing `liveAssistCards`/`agentProposals` arrays, with no
knowledge of `chatMessages` at all. A chat turn spawned by "Ask AI" is
**not** one of the three `QueueEntryClassification`s (`actionable` / `info` /
`fragment_suspect`, workspace/agentQueue.ts:77-80) — the underlying proposal
card is dismissed (moves to feed as `status: "dismissed"`, read-only) at the
exact moment the chat turn begins, so today there is no queue/feed row that
represents "a question I asked and am waiting on an answer for." If the
panel wants the agent tile to show live chat turns alongside the queue, that
is new state and a new render path, not an extension of `selectAgentQueue`'s
existing three-way classification.

### The review surface (Sessions' Ask lens)

Already covered in §1: `askAvailable` restricts Ask to the `notes`/`graph`
lenses (SessionsBrowser.tsx:225-227), and the entire Sessions detail view —
including Ask — is unreachable while capturing (`reviewLocked`). Any redesign
that wants chat visible *during* live capture cannot reuse this surface's
current gating; it is architecturally the "after the fact, one session at a
time, capture must be stopped" surface, not a live one.

## 3. What a chat-turn view-model needs from the store

Current primitives (store/index.ts unless noted):

- `chatMessages: ChatMessage[]` — `ChatMessage = { role: "user" | "assistant"
  | "system"; content: string }` (types/index.ts:2666-2669). **No id, no
  timestamp, no per-message status.** `ChatSidebar.tsx`'s own list key is
  `` `${msg.role}-${idx}` `` with an explicit comment: "ChatMessage carries no
  unique id to use instead" (ChatSidebar.tsx:113-118). Any view-model that
  needs stable identity (for animation, per-turn actions, or correlating a
  turn back to the agent-proposal card that spawned it) has nothing to key
  off today.
- `isChatLoading: boolean` — one global flag, not per-message. The
  in-flight assistant turn is identified purely by *position* (the last
  array entry, `role === "assistant"`, `content === ""` — see
  store/index.ts:2928-2933's `isPlaceholder` check in the non-streaming
  fallback path). A richer view-model (e.g. showing which of several
  concurrent turns is still streaming) can't be built on this positional
  convention as-is.
- `streamingChatRequestId: string | null` — correlates in-flight delta
  frames to the active stream, but is **not stored per-message** — it's a
  single store field, and the mapping from `request_id` to "which message
  index this belongs to" lives only in the closure-local variables inside
  `sendChatMessage` (`requestId`, `pendingDelta`, `flushTimer`,
  store/index.ts:2827-2905), not in any inspectable state. A view-model
  wanting to show streaming status per-turn (rather than assuming "the last
  message is always the one streaming") would need this correlation
  promoted into actual state.
- `ChatTokenDeltaEvent { request_id, delta, finish_reason? }` and
  `ChatTokenDoneEvent { request_id, full_text, finish_reason, usage? }`
  (types/index.ts:2686-2711) carry richer per-turn data (finish reason,
  token usage) that's consumed transiently by `appendChatTokenDelta`/
  `finalizeChatStream` but **not retained on the `ChatMessage` itself** —
  today's `ChatMessage` shape throws usage/finish-reason away once the
  stream finalizes. A view-model wanting to show "N tokens" or distinguish a
  cancelled/errored turn from a normal one needs that data kept somewhere
  addressable per-message, not just passed through.
- `graphSnapshot` — the only other piece of context `ChatSidebar` reads
  today, purely for the entity-count badge in its header
  (ChatSidebar.tsx:80-84, `chat.entities`); not part of a chat turn's own
  shape.
- Nothing in `ChatMessage` links a turn back to its origin — an "Ask AI"
  turn (spawned from a specific `AgentProposalEvent`/`LiveAssistCardRecord`)
  and a turn typed directly into `ChatSidebar` are indistinguishable once
  they land in `chatMessages`. If the design wants the agent tile to show
  "this chat turn came from that proposal card," the origin needs to be
  captured at creation time (in `askAgentProposal`/`sendChatMessage`) since
  nothing downstream can reconstruct it.
- Persistence: as noted in §1, `chatMessages` has no backend counterpart at
  all — no `chat_messages` field on `LoadedSession`, no send/receive
  logging (confirmed independently by the round-5 logs angle: zero `chat`
  literal hits across both analyzed log files). A view-model that expects
  chat history to survive a session reload, or to be inspectable from a
  past session the way notes/graph/transcript are, is not supported by
  anything that exists today.
