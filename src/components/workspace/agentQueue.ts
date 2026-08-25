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
    // audio-graph-83cc T1's own invariant is that `LiveAssistCardRecord.signal`
    // is mirrored from `proposal.signal` (events.rs) — a synthesized record
    // (this function's whole reason for existing: a proposal that has not
    // yet round-tripped through a real `live_assist_card` backend event) must
    // not disagree with a backend-authored record for the identical proposal
    // on this field once one does land (fix-round finding: T5's auto-answer
    // admit is the next reader of this field).
    signal: proposal.signal,
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
 * - `"fragment_suspect"` — seed 104f's problem (question entries minted from
 *   utterance fragments, e.g. "so what about the"). W8 shipped NO code path
 *   that ever produced this value. Ticket W9 (ratified R6) is what you are
 *   reading: `classifyQueueEntry` (quality rules) and `selectAgentQueue`'s
 *   own duplicate-collapse pass (see below) now actually detect fragments
 *   and return this class for them.
 */
export type QueueEntryClassification =
  | "actionable"
  | "info"
  | "fragment_suspect";

/** Ticket W9 thresholds (design-a §3.2's own constants, kept verbatim so the
 * design doc and the implementation never drift in name). */
export const AGENT_QUEUE_CONFIDENCE_FLOOR = 0.5;
export const AGENT_QUEUE_MIN_TOKENS = 4;
export const AGENT_QUEUE_MIN_CHARS = 16;

/**
 * The ONLY production site that constructs an `AgentProposalPayload` is
 * `run_agent_proposal_task` (`speech/mod.rs`). Its `agent_proposal_title`
 * helper mints a FORMULAIC, content-free `title` for every kind —
 * `"Question from {speaker}"`, `"Context from {speaker}"`, and the hard
 * constant `"Possible graph update"` — identical across every distinct
 * utterance from the same speaker (or, for graph suggestions, across EVERY
 * utterance from anyone). The transcript-derived content a quality or
 * duplicate heuristic actually needs to inspect lives in `body`
 * (`agent_proposal_body`, same file), behind one of these three canned
 * prefixes. A heuristic that inspects `title` instead — as this file did
 * pre-fix — cannot distinguish a genuine question from a 104f fragment
 * (both produce the identical title), and cannot distinguish two genuinely
 * distinct graph suggestions from one repeated (both produce the identical
 * constant title): it was keyed on the one field production makes constant.
 *
 * `question_text_from_body` (`commands.rs`) already strips the Question
 * prefix to recover a graph-node label from an approved card; this map
 * generalizes that same strip-the-canned-prefix technique to all three
 * kinds, because duplicate-collapse (below) is kind-agnostic.
 */
const AGENT_PROPOSAL_BODY_PREFIXES: Record<AgentProposalEvent["kind"], string> =
  {
    question: "Consider answering or linking this question: ",
    graph_suggestion:
      "Review this for an action item, decision, or relationship: ",
    note: "Keep this context available: ",
  };

/**
 * Recovers the transcript-derived text a proposal was minted from. Strips
 * the known canned prefix for `proposal.kind` when present; falls back to
 * the raw `body` when it isn't (a hand-written fixture, or a future backend
 * body-format change) — this deliberately never falls back to `title`,
 * because `title` is exactly the formulaic field that hides content in
 * production (see the module-level comment above).
 */
export function queueContentText(proposal: AgentProposalEvent): string {
  const body = (proposal.body ?? "").trim();
  const prefix = AGENT_PROPOSAL_BODY_PREFIXES[proposal.kind];
  if (prefix && body.startsWith(prefix)) {
    return body.slice(prefix.length).trim();
  }
  return body;
}

/**
 * Normalizes text for the duplicate-collapse rule: trim, lowercase,
 * collapse internal whitespace runs to one space. `toLowerCase()` is
 * Unicode-aware (handles "Você" -> "você" correctly), so this stays
 * locale-safe for accented pt text — no ASCII-only regex anywhere in this
 * file.
 */
function normalizeQueueContent(text: string): string {
  return text.trim().toLowerCase().replace(/\s+/g, " ");
}

/**
 * The kind-agnostic duplicate-collapse KEY (design-a §3.2 rule 2, "exact
 * normalized-title"), rebased onto `queueContentText` rather than
 * `proposal.title` — see the module comment above for why keying on the
 * formulaic title collapses every concurrently-pending graph suggestion (or
 * every note from one speaker) into one, regardless of how many genuinely
 * distinct utterances produced them. Returns `null` for empty content so an
 * empty string never falsely "matches" another empty string.
 */
export function queueDuplicateKey(proposal: AgentProposalEvent): string | null {
  const normalized = normalizeQueueContent(queueContentText(proposal));
  return normalized.length > 0 ? normalized : null;
}

/** Word count via whitespace splitting — locale-safe for any
 * space-delimited language (en, pt both qualify); no word lists. */
function queueContentTokenCount(text: string): number {
  const trimmed = text.trim();
  if (trimmed.length === 0) return 0;
  return trimmed.split(/\s+/).length;
}

/**
 * design-a §3.2's actual sentence-shape test: a dangling clause — text
 * that trails off in a comma/semicolon/colon — is fragment-shaped; the
 * absence of TERMINAL punctuation is not, by itself, evidence of a
 * fragment (a complete question or statement need not end in `.`/`?`/`!`
 * once title-derived punctuation is out of the picture). Punctuation-shape
 * only — no interrogative word list, so this is identical en/pt (pt has no
 * inverted `¿`/`¡` marks, unlike Spanish).
 */
function hasDanglingClauseEnding(text: string): boolean {
  return /[,;:]\s*$/.test(text.trim());
}

/**
 * Classifies one ACTIONABLE card's CONTENT QUALITY only. Duplicate
 * detection is a separate, kind-agnostic concern that `selectAgentQueue`
 * runs itself (see that function's comment) before falling back to this
 * function — it doesn't belong here because this function's two rules are
 * deliberately kind-scoped and duplicate-collapse is not.
 *
 * `isActionable` is a STRUCTURAL fact the caller already has (pending
 * status AND present in the live `agentProposals` array, i.e. not merely
 * historical) — not a quality heuristic, so a non-actionable card is always
 * `"info"` without inspecting its content at all (W8's exact behavior,
 * unchanged).
 *
 * For an actionable card, W9 additionally inspects `card.proposal` against
 * two rules, scoped to `kind === "question"` ONLY:
 *
 * 1. **Confidence floor** — `confidence < AGENT_QUEUE_CONFIDENCE_FLOOR`.
 * 2. **Locale-safe sentence-shape test**, evaluated against
 *    `queueContentText` (the transcript-derived text recovered from `body`
 *    — see that function's doc for why `title` is unusable for this):
 *    too few tokens, too few characters, or a dangling clause ending.
 *
 * Both rules stay scoped to questions rather than becoming kind-agnostic:
 * `note`/`graph_suggestion` and `question` proposals are ALL minted at the
 * one production site (`run_agent_proposal_task`, `speech/mod.rs`) from the
 * same `segment.confidence`, so there is no different-code-path reason a
 * low-confidence note is "safer" than a low-confidence question — an
 * earlier version of this comment claimed one and that claim was false.
 * The real reason for the scoping is narrower and verifiable: seed 104f's
 * fragment-minting bug is a QUESTION-detection bug (`agent_proposal_kind`
 * misclassifies a truncated utterance as a question), R6 ratified accepting
 * that risk ONLY for questions ("occasionally hiding a genuine short
 * QUESTION"), and a real PRE-EXISTING regression fixture
 * (`AgentProposalsPanel.test.tsx`'s 0.42-confidence `kind: "note"` case,
 * predating this ticket) already depends on a low-confidence note staying
 * actionable. Widening either rule to notes/graph suggestions would break
 * that fixture and would exceed what R6 ratified — it is a deliberately
 * conservative policy choice, not a fact about production provenance.
 *
 * audio-graph-83cc T4 fix-round finding (major): a card with
 * `card.origin === "user"` (T1 schema — a free-form question the user typed
 * into `AgentComposer`, never a transcript-detection guess) is EXEMPT from
 * every rule below, unconditionally. These rules exist to catch seed 104f's
 * transcript-fragment-detection bug ("so what about the") — a shape that
 * cannot occur for text the user deliberately typed and submitted. Before
 * this exemption, a short user question (or a low/default `confidence` a
 * future backend mint assigns a typed question, since confidence is
 * meaningless for non-detected text) would be demoted to the read-only feed
 * row, where the composer's own flagship answer thread renders behind a
 * collapsed `Details` disclosure with no reachable Retry — the exact
 * regression an adversarial review caught against `askQuestion`'s minted
 * cards.
 */
export function classifyQueueEntry(
  card: LiveAssistCardRecord,
  isActionable: boolean,
): QueueEntryClassification {
  if (!isActionable) return "info";
  if (card.origin === "user") return "actionable";

  const { proposal } = card;
  if (proposal.kind === "question") {
    if (proposal.confidence < AGENT_QUEUE_CONFIDENCE_FLOOR) {
      return "fragment_suspect";
    }
    const contentText = queueContentText(proposal);
    const tooShort =
      queueContentTokenCount(contentText) < AGENT_QUEUE_MIN_TOKENS ||
      contentText.length < AGENT_QUEUE_MIN_CHARS;
    if (tooShort || hasDanglingClauseEnding(contentText)) {
      return "fragment_suspect";
    }
  }

  return "actionable";
}

/**
 * The 104f admission seam (design-b §4.3's `admitToQueue`, ratified by the
 * synthesis over design-a's `classifyQueueEntry`-as-the-whole-fix framing —
 * see synthesis.md §2 "Agent tile": "B's ... `admitToQueue` predicate as
 * 104f's slot"). W8 shipped this as an unconditional admit; ticket W9
 * (ratified R6) is the body change the W8 doc comment anticipated: an
 * actionable card is admitted unless it was classified `"fragment_suspect"`
 * (either by `classifyQueueEntry`'s quality rules, or by
 * `selectAgentQueue`'s own duplicate-collapse pass).
 *
 * This is the **Signal-mode** rule only. The Signal/All toggle's "All"
 * branch does not call this function at all — `AgentProposalsPanel` passes
 * a separate `() => true` predicate into `selectAgentQueue` when the
 * persisted filter is `"all"` (see `AGENT_QUEUE_FILTER_ALL` in that file),
 * so switching to All is a call-site swap, not a body change here. That
 * means the real call site DOES now override `selectAgentQueue`'s `admit`
 * parameter (unlike the "only ever overridden by a test" claim this file
 * carried pre-W9) — see that parameter's own updated comment below.
 *
 * Filtered entries are never deleted: `selectAgentQueue` still pushes a
 * rejected actionable card into `feed` (design-a §3.1: "the queue is a
 * priority filter, never a capability gate"), and the All-mode toggle can
 * always bring it back into an actionable render, because the store data
 * behind it never changed — only which predicate ran over it.
 */
export function admitToQueue(
  _card: LiveAssistCardRecord,
  classification: QueueEntryClassification,
): boolean {
  return classification !== "fragment_suspect";
}

export interface AgentQueueSelection {
  /** Actionable, admitted, newest-first — the ONLY cards the queue renders
   * approve/ask/dismiss actions for. In Signal mode (default) this excludes
   * `"fragment_suspect"` cards; in All mode it includes them (see
   * `fragmentSuspectIds` below for how a caller tells them apart to render
   * the W9 marker). */
  queue: LiveAssistCardRecord[];
  /** Everything else (resolved cards, historical pending cards, and — in
   * Signal mode — any actionable card `admitToQueue` rejects), newest-first.
   * Read-only — "Recent activity", never "All activity" (design-b §4.3: the
   * store slices `agentProposals` to `-49` and `liveAssistCards` is
   * session-scoped, so this is never the FULL history). */
  feed: LiveAssistCardRecord[];
  /**
   * Proposal ids classified `"fragment_suspect"` this pass (by either the
   * quality rules or the duplicate-collapse pass), regardless of which list
   * (`queue` or `feed`) they landed in. Additive (ticket W9): lets a
   * renderer show the "subtle fragment marker" design-a §3.2 calls for — in
   * All mode on a `queue` row, in Signal mode on a `feed` row — by checking
   * membership rather than re-deriving classification, so the marker and
   * the admission decision can never disagree. Never contains an
   * `"info"`-classified id: a non-actionable card is never flagged.
   */
  fragmentSuspectIds: Set<string>;
  /**
   * design-a §3.2 rule 2's other half: "the original [surviving] row
   * renders `agent.duplicateCount` — `×{{count}}`". Keyed by the SURVIVING
   * proposal's id -> total group size (survivor + every duplicate collapsed
   * onto it), so a value of `1` never appears (a solo card is absent from
   * this map entirely — callers should treat "absent" and "1" as "no badge"
   * and only render for `count > 1`). Only ever contains ids that are
   * themselves NOT `fragment_suspect` via the duplicate rule (a duplicate's
   * own id is never a key here — it's a victim, not a survivor).
   */
  duplicateCounts: Map<string, number>;
}

/**
 * The one selector `AgentProposalsPanel` calls. Reads nothing from a store
 * itself (design-b's zero-new-state posture, kept honest by NOT importing
 * `useAudioGraphStore` here at all) — the caller passes the two existing
 * store fields straight through.
 *
 * Runs TWO passes of logic per actionable card, newest-first:
 *
 * 1. **Duplicate-collapse** (kind-agnostic, content-keyed — see
 *    `queueDuplicateKey`): if this card's content already matches an
 *    earlier-iterated (i.e. NEWER, since `merged` is newest-first) card
 *    that survived as `"actionable"`, this card is `"fragment_suspect"` and
 *    the survivor's `duplicateCounts` entry increments. Critically, a card
 *    is only ever registered as a survivor for later duplicates to match
 *    against if IT is not itself fragment-suspect — otherwise a
 *    fragment-suspect newest twin would "burn" the content key and hide an
 *    older, well-formed twin behind it too (review finding: "nobody wins").
 *    Because the newest surviving occurrence is what later duplicates
 *    match against, "newest wins" still holds whenever the newest occurrence
 *    is itself well-formed.
 * 2. **Quality rules** (`classifyQueueEntry`, kind-scoped): only reached
 *    when duplicate-collapse didn't already classify the card.
 *
 * `admit` defaults to the real `admitToQueue` seam (Signal mode). As of
 * ticket W9 it IS also overridden by a real call site: `AgentProposalsPanel`
 * passes an unconditional `() => true` when the user's persisted filter is
 * `"all"`. `agentQueue.test.ts` additionally overrides it with `() => false`
 * to prove the seam is genuinely load-bearing (mutation proof), independent
 * of the real All-mode override — same technique, different purpose. Both
 * rely on the same fact: same-module `vi.spyOn` self-mocking would not
 * intercept these calls (a function's calls to another export of its OWN
 * module bind to the local declaration, not the module's exports object),
 * so an explicit parameter is the only way to substitute either behavior.
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

  // Content key -> surviving (non-fragment-suspect) proposal id, built up
  // ACROSS this loop. `merged` is newest-first, so a later iteration here
  // means an OLDER card — "already registered" means "a newer, well-formed
  // occurrence of this same content already claimed the surviving slot".
  const seenContent = new Map<string, string>();
  const duplicateCounts = new Map<string, number>();
  const fragmentSuspectIds = new Set<string>();
  const queue: LiveAssistCardRecord[] = [];
  const feed: LiveAssistCardRecord[] = [];
  for (const card of merged) {
    const isActionable =
      card.status === "pending" && actionableProposalIds.has(card.proposal.id);

    let classification: QueueEntryClassification;
    if (!isActionable) {
      classification = "info";
    } else {
      // audio-graph-83cc T4 fix-round finding: a user-typed card (see
      // `classifyQueueEntry`'s matching exemption doc above) never
      // participates in duplicate-collapse either — asking the identical
      // question twice on purpose (e.g. retrying with the same wording) must
      // not silently hide the second thread behind a demoted feed row, and a
      // user-origin card must never "burn" a content key that would then
      // wrongly collapse an unrelated transcript-derived card onto it.
      const duplicateKey =
        card.origin === "user" ? null : queueDuplicateKey(card.proposal);
      const survivorId = duplicateKey
        ? seenContent.get(duplicateKey)
        : undefined;
      if (survivorId !== undefined) {
        classification = "fragment_suspect";
        duplicateCounts.set(
          survivorId,
          (duplicateCounts.get(survivorId) ?? 1) + 1,
        );
      } else {
        classification = classifyQueueEntry(card, true);
        if (classification !== "fragment_suspect" && duplicateKey) {
          seenContent.set(duplicateKey, card.proposal.id);
        }
      }
    }

    if (classification === "fragment_suspect") {
      fragmentSuspectIds.add(card.proposal.id);
    }
    if (isActionable && admit(card, classification)) {
      queue.push(card);
    } else {
      feed.push(card);
    }
  }
  return { queue, feed, fragmentSuspectIds, duplicateCounts };
}
