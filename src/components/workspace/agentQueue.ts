/**
 * `agentQueue` — pure store-derived selectors for the agent tile (ticket W8,
 * synthesis audio-graph-a6b5 §"Agent tile"). Extracted out of
 * `AgentProposalsPanel.tsx` so the component stays a thin renderer: this
 * module owns the merge, the queue/feed split, and the 104f admission seam,
 * none of which need a DOM.
 *
 * ZERO NEW STORE STATE (design-b §4.3, ratified by the synthesis): every
 * function here is a pure derivation over the EXISTING `agentProposals` /
 * `liveAssistCards` store fields. `selectAgentQueue` is the one call site
 * `AgentProposalsPanel` uses; it does not read the store itself, so it stays
 * trivially unit-testable without a Zustand harness.
 */
import type { AgentProposalEvent, LiveAssistCardRecord } from "../../types";

/**
 * `liveAssistCardFromProposal`/`mergeLiveAssistCards` — moved verbatim from
 * `AgentProposalsPanel.tsx` (pre-W8: module-local functions with the exact
 * same bodies). A pending proposal that has not yet round-tripped through a
 * `live_assist_card` backend event (i.e. it only exists in `agentProposals`)
 * gets a synthesized record so the merge has one shape to sort/filter,
 * mirroring the exact synthesis the pre-W8 component already did.
 */
export function liveAssistCardFromProposal(
  proposal: AgentProposalEvent,
): LiveAssistCardRecord {
  return {
    session_id: "",
    proposal,
    status: "pending",
    source_span_ids: proposal.source_segment_id
      ? [proposal.source_segment_id]
      : [],
    graph_context_ids: [],
    outcome: null,
    projection_patch_sequence: null,
    created_at_ms: proposal.created_at_ms,
    updated_at_ms: proposal.created_at_ms,
  };
}

export function mergeLiveAssistCards(
  liveAssistCards: LiveAssistCardRecord[],
  pendingProposals: AgentProposalEvent[],
): LiveAssistCardRecord[] {
  const byProposalId = new Map<string, LiveAssistCardRecord>();
  for (const card of liveAssistCards) {
    byProposalId.set(card.proposal.id, card);
  }
  for (const proposal of pendingProposals) {
    if (!byProposalId.has(proposal.id)) {
      byProposalId.set(proposal.id, liveAssistCardFromProposal(proposal));
    }
  }
  return [...byProposalId.values()].sort(
    (a, b) =>
      b.updated_at_ms - a.updated_at_ms || b.created_at_ms - a.created_at_ms,
  );
}

/**
 * The three-way classification design-a's agent-tile section names
 * (`classifyQueueEntry`, "actionable / info / fragment-suspect"):
 *
 * - `"actionable"` — a pending card the current session can still act on
 *   (approve/ask/dismiss) — the ONLY class the queue ever shows.
 * - `"info"` — a resolved (approved/dismissed) card, or a pending card this
 *   session can no longer act on (a loaded/reviewed session's historical
 *   pending card) — renders in the feed, read-only.
 * - `"fragment_suspect"` — RESERVED for seed 104f's fix (question entries
 *   minted from utterance fragments, e.g. "so what about the"). W8 ships NO
 *   code path that ever produces this value — see `admitToQueue` below for
 *   why the seam is a separate function from this one. W9 (ratified R6)
 *   is the ticket that teaches `classifyQueueEntry` to actually detect
 *   fragments (confidence floor, duplicate-title collapse, a locale-safe
 *   sentence-shape test) and return this class for them.
 */
export type QueueEntryClassification =
  | "actionable"
  | "info"
  | "fragment_suspect";

/**
 * Classifies one merged card. `isActionable` is a STRUCTURAL fact the caller
 * already has (pending status AND present in the live `agentProposals`
 * array, i.e. not merely historical) — not a quality heuristic, so passing
 * it in here (rather than re-deriving it from `card` alone) keeps this
 * function honest about what it currently knows: nothing about the
 * PROPOSAL'S CONTENT yet. W9 changes that by inspecting `card.proposal`
 * (title/body/confidence) to additionally return `"fragment_suspect"` for an
 * otherwise-actionable card; W8 intentionally does not.
 */
export function classifyQueueEntry(
  _card: LiveAssistCardRecord,
  isActionable: boolean,
): QueueEntryClassification {
  return isActionable ? "actionable" : "info";
}

/**
 * The 104f admission seam (design-b §4.3's `admitToQueue`, ratified by the
 * synthesis over design-a's `classifyQueueEntry`-as-the-whole-fix framing —
 * see synthesis.md §2 "Agent tile": "B's ... `admitToQueue` predicate as
 * 104f's slot"). Shipped as an unconditional admit — `() => true` in
 * substance, though the signature takes the classification so a future
 * body can gate on it without a call-site change.
 *
 * The admission DECISION is this one function — that much holds regardless
 * of what else W9 touches. Do not read this as "W9's only diff is this
 * function's body," though: per review, at least three other edits are
 * already known to be in scope for that ticket and are NOT covered by this
 * seam alone —
 *   1. `classifyQueueEntry`'s signature grows a `seenTitles` (or similar)
 *      param for design-a §3.2's duplicate-title collapse, which also means
 *      threading that map through `selectAgentQueue`'s loop.
 *   2. The Signal/All toggle (ratified R6) needs to reach `admit` from the
 *      component — i.e. `AgentProposalsPanel`'s one call site starts
 *      passing a third argument, which it does not today.
 *   3. The moment `admit` can return `false` for an actionable card, that
 *      card lands in `AgentFeedRow`, which renders no approve/ask/dismiss
 *      controls today (by design — feed rows are read-only) and W9 must
 *      confirm that demotion path still respects design-a §3.1 ("the queue
 *      is a priority filter, never a capability gate").
 * When 104f's heuristic lands, expect this function's BODY to change to
 * `return classification !== "fragment_suspect"` (or the toggle's "All"
 * branch, unconditionally `true`) — but budget for the three edits above
 * alongside it, not instead of it.
 */
export function admitToQueue(
  _card: LiveAssistCardRecord,
  _classification: QueueEntryClassification,
): boolean {
  return true;
}

export interface AgentQueueSelection {
  /** Actionable, admitted, newest-first — the ONLY cards the queue renders
   * approve/ask/dismiss actions for. */
  queue: LiveAssistCardRecord[];
  /** Everything else (resolved cards, historical pending cards, and any
   * card `admitToQueue` rejects once W9 lands), newest-first. Read-only —
   * "Recent activity", never "All activity" (design-b §4.3: the store slices
   * `agentProposals` to `-49` and `liveAssistCards` is session-scoped, so
   * this is never the FULL history). */
  feed: LiveAssistCardRecord[];
}

/**
 * The one selector `AgentProposalsPanel` calls. Reads nothing from a store
 * itself (design-b's zero-new-state posture, kept honest by NOT importing
 * `useAudioGraphStore` here at all) — the caller passes the two existing
 * store fields straight through.
 *
 * `admit` defaults to the real `admitToQueue` seam and is ONLY ever
 * overridden by a test — never by a real call site — so
 * `agentQueue.test.ts` can prove the seam is genuinely load-bearing (pass a
 * `() => false` in and watch the queue empty into the feed) without relying
 * on same-module `vi.spyOn` self-mocking, which does not intercept a
 * function's calls to another export of its OWN module (the call binds to
 * the local declaration, not the module's exports object).
 */
export function selectAgentQueue(
  liveAssistCards: LiveAssistCardRecord[],
  agentProposals: AgentProposalEvent[],
  admit: (
    card: LiveAssistCardRecord,
    classification: QueueEntryClassification,
  ) => boolean = admitToQueue,
): AgentQueueSelection {
  const merged = mergeLiveAssistCards(liveAssistCards, agentProposals);
  const actionableProposalIds = new Set(
    agentProposals.map((proposal) => proposal.id),
  );

  const queue: LiveAssistCardRecord[] = [];
  const feed: LiveAssistCardRecord[] = [];
  for (const card of merged) {
    const isActionable =
      card.status === "pending" && actionableProposalIds.has(card.proposal.id);
    const classification = classifyQueueEntry(card, isActionable);
    if (isActionable && admit(card, classification)) {
      queue.push(card);
    } else {
      feed.push(card);
    }
  }
  return { queue, feed };
}
