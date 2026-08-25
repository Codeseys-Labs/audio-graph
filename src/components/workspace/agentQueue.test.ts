import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { AgentProposalEvent, LiveAssistCardRecord } from "../../types";
import {
  AGENT_QUEUE_CONFIDENCE_FLOOR,
  AGENT_QUEUE_MIN_CHARS,
  AGENT_QUEUE_MIN_TOKENS,
  admitToQueue,
  classifyQueueEntry,
  liveAssistCardFromProposal,
  mergeLiveAssistCards,
  queueContentText,
  queueDuplicateKey,
  selectAgentQueue,
} from "./agentQueue";

let seq = 0;

/** The exact canned body prefixes `agent_proposal_body` (speech/mod.rs)
 * mints in production — used throughout this file to build REAL-SHAPED
 * fixtures rather than hand-written prose that happens to sit in `title`.
 * Mirrors `useTauriEvents.test.ts:317`'s real
 * `title: "Question from Speaker 1"` / `body: "Consider answering or
 * linking this question: ..."` pairing. */
function questionBody(text: string): string {
  return `Consider answering or linking this question: ${text}`;
}
function noteBody(text: string): string {
  return `Keep this context available: ${text}`;
}
function graphSuggestionBody(text: string): string {
  return `Review this for an action item, decision, or relationship: ${text}`;
}

function proposal(
  overrides: Partial<AgentProposalEvent> = {},
): AgentProposalEvent {
  seq += 1;
  return {
    id: `p${seq}`,
    source_segment_id: `seg${seq}`,
    source_id: "system-default",
    speaker_label: null,
    kind: "note",
    title: `Title ${seq}`,
    body: `Body ${seq}`,
    confidence: 0.8,
    created_at_ms: seq,
    ...overrides,
  };
}

function card(
  overrides: Omit<Partial<LiveAssistCardRecord>, "proposal"> & {
    proposal?: Partial<AgentProposalEvent>;
  } = {},
): LiveAssistCardRecord {
  const { proposal: proposalOverrides, ...recordOverrides } = overrides;
  const baseProposal = proposal(proposalOverrides ?? {});
  return {
    session_id: "session-1",
    status: "pending",
    source_span_ids: [baseProposal.source_segment_id],
    graph_context_ids: [],
    outcome: null,
    projection_patch_sequence: null,
    created_at_ms: baseProposal.created_at_ms,
    updated_at_ms: baseProposal.created_at_ms,
    ...recordOverrides,
    proposal: { ...baseProposal, ...(proposalOverrides ?? {}) },
  };
}

describe("liveAssistCardFromProposal / mergeLiveAssistCards", () => {
  it("synthesizes a pending card for a proposal with no matching liveAssistCards entry", () => {
    const p = proposal({ id: "px" });
    const synthesized = liveAssistCardFromProposal(p);
    expect(synthesized.status).toBe("pending");
    expect(synthesized.proposal).toBe(p);
    expect(synthesized.outcome).toBeNull();
  });

  it("mirrors proposal.signal onto the synthesized record (T1 invariant: LiveAssistCardRecord.signal mirrors proposal.signal)", () => {
    const withSignal = proposal({ id: "px-signal", signal: "weak" });
    expect(liveAssistCardFromProposal(withSignal).signal).toBe("weak");

    const withoutSignal = proposal({ id: "px-no-signal" });
    expect(liveAssistCardFromProposal(withoutSignal).signal).toBeUndefined();
  });

  it("prefers the persisted liveAssistCards record over a synthesized one for the same proposal id", () => {
    const persisted = card({
      proposal: { id: "shared" },
      status: "approved",
      updated_at_ms: 999,
    });
    const merged = mergeLiveAssistCards(
      [persisted],
      [proposal({ id: "shared", created_at_ms: 1 })],
    );
    expect(merged).toHaveLength(1);
    expect(merged[0].status).toBe("approved");
  });

  it("orders newest-first by updated_at_ms, falling back to created_at_ms", () => {
    const older = card({ updated_at_ms: 1, created_at_ms: 1 });
    const newer = card({ updated_at_ms: 5, created_at_ms: 5 });
    const merged = mergeLiveAssistCards([older, newer], []);
    expect(merged[0]).toBe(newer);
    expect(merged[1]).toBe(older);
  });
});

describe("queueContentText — recovering transcript content from the canned body (blocker fix: title is formulaic in production)", () => {
  it("strips the real Question canned prefix (speech/mod.rs's agent_proposal_body), recovering the utterance — mirrors commands.rs's question_text_from_body", () => {
    const p = proposal({
      kind: "question",
      title: "Question from Speaker 1",
      body: questionBody("What changed?"),
    });
    expect(queueContentText(p)).toBe("What changed?");
  });

  it("strips the real GraphSuggestion canned prefix", () => {
    const p = proposal({
      kind: "graph_suggestion",
      title: "Possible graph update",
      body: graphSuggestionBody("we need to follow up with legal"),
    });
    expect(queueContentText(p)).toBe("we need to follow up with legal");
  });

  it("strips the real Note canned prefix", () => {
    const p = proposal({
      kind: "note",
      title: "Context from Speaker 2",
      body: noteBody("the migration deadline moved to Friday"),
    });
    expect(queueContentText(p)).toBe("the migration deadline moved to Friday");
  });

  it("falls back to the raw body (never to title) when the canned prefix isn't present", () => {
    const p = proposal({
      kind: "question",
      title: "some title",
      body: "a hand-written body with no canned prefix",
    });
    expect(queueContentText(p)).toBe(
      "a hand-written body with no canned prefix",
    );
  });
});

describe("classifyQueueEntry — the classification table (actionable / info / fragment_suspect, ticket W9)", () => {
  it("classifies an actionable, well-formed non-question card as 'actionable'", () => {
    expect(classifyQueueEntry(card({ proposal: { kind: "note" } }), true)).toBe(
      "actionable",
    );
  });

  it("classifies a non-actionable card (resolved, or historical pending) as 'info' regardless of content", () => {
    expect(classifyQueueEntry(card({ status: "approved" }), false)).toBe(
      "info",
    );
    expect(classifyQueueEntry(card({ status: "dismissed" }), false)).toBe(
      "info",
    );
    expect(classifyQueueEntry(card({ status: "pending" }), false)).toBe("info");
    // Even a short/fragment-shaped question body stays 'info' when not
    // actionable — the content rules only ever run on actionable cards.
    expect(
      classifyQueueEntry(
        card({
          status: "dismissed",
          proposal: { kind: "question", title: "Question from X", body: "hi" },
        }),
        false,
      ),
    ).toBe("info");
  });

  it("PRODUCTION-SHAPED (blocker fix): the real backend title 'Question from {speaker}' is 'actionable', not fragment_suspect, when the underlying utterance is well-formed", () => {
    const c = card({
      proposal: {
        kind: "question",
        title: "Question from Speaker 1",
        body: questionBody("Is this the final budget for the quarter?"),
        confidence: 0.87,
      },
    });
    expect(classifyQueueEntry(c, true)).toBe("actionable");
  });

  it("PRODUCTION-SHAPED (blocker fix): 'Question from Unknown' (no speaker label) is also 'actionable' for a well-formed underlying question", () => {
    const c = card({
      proposal: {
        kind: "question",
        title: "Question from Unknown",
        body: questionBody("What did the team decide about pricing?"),
        confidence: 0.9,
      },
    });
    expect(classifyQueueEntry(c, true)).toBe("actionable");
  });

  it("W9: a low-confidence QUESTION proposal is 'fragment_suspect' (confidence floor) — literal boundary, not derived from the imported constant", () => {
    const c = card({
      proposal: {
        kind: "question",
        title: "Question from Speaker 1",
        body: questionBody("Is this the final budget for the quarter?"),
        confidence: 0.49,
      },
    });
    expect(classifyQueueEntry(c, true)).toBe("fragment_suspect");
  });

  it("W9: confidence exactly AT the floor (0.5, literal) is NOT caught by the floor rule — only strictly-below is", () => {
    const c = card({
      proposal: {
        kind: "question",
        title: "Question from Speaker 1",
        body: questionBody("Is this the final budget for the quarter?"),
        confidence: 0.5,
      },
    });
    expect(classifyQueueEntry(c, true)).toBe("actionable");
  });

  it("the floor is exactly AGENT_QUEUE_CONFIDENCE_FLOOR (sanity check on the two literal-boundary tests above)", () => {
    expect(AGENT_QUEUE_CONFIDENCE_FLOOR).toBe(0.5);
  });

  it("W9: a low-confidence NOTE/graph_suggestion proposal is NOT reclassified — the confidence rule is question-scoped (verified against this file's own regression fixture: a real 0.42-confidence note proposal must stay actionable)", () => {
    const noteCard = card({
      proposal: {
        kind: "note",
        title: "Context from Speaker 1",
        body: noteBody("Ok"),
        confidence: 0.1,
      },
    });
    expect(classifyQueueEntry(noteCard, true)).toBe("actionable");
  });

  it("W9: a short QUESTION underlying utterance below the token/char thresholds is 'fragment_suspect' (the 104f fragment shape, e.g. 'what about') — even though the TITLE is the well-formed-looking production constant", () => {
    const c = card({
      proposal: {
        kind: "question",
        title: "Question from Speaker 1",
        body: questionBody("what about"),
        confidence: 0.9,
      },
    });
    expect(queueContentTokenCountForTest("what about")).toBeLessThan(
      AGENT_QUEUE_MIN_TOKENS,
    );
    expect(classifyQueueEntry(c, true)).toBe("fragment_suspect");
  });

  it("MIN_CHARS clause (mutation guard): 4+ tokens, terminal punctuation, but under 16 chars is still 'fragment_suspect' — a fixture the min-chars clause alone must catch", () => {
    const text = "Is it ok now?";
    expect(queueContentTokenCountForTest(text)).toBeGreaterThanOrEqual(
      AGENT_QUEUE_MIN_TOKENS,
    );
    expect(text.length).toBeLessThan(AGENT_QUEUE_MIN_CHARS);
    const c = card({
      proposal: {
        kind: "question",
        title: "Question from Speaker 1",
        body: questionBody(text),
        confidence: 0.9,
      },
    });
    expect(classifyQueueEntry(c, true)).toBe("fragment_suspect");
  });

  it("W9 (design-a §3.2's ACTUAL rule): a long-enough QUESTION with no terminal punctuation at all is 'actionable' — design-a rejects a dangling clause (trailing ,;:), not 'lacks .?!'", () => {
    const text = "so what about the enterprise pricing tier we discussed";
    expect(text.length).toBeGreaterThanOrEqual(AGENT_QUEUE_MIN_CHARS);
    const c = card({
      proposal: {
        kind: "question",
        title: "Question from Speaker 1",
        body: questionBody(text),
        confidence: 0.9,
      },
    });
    expect(classifyQueueEntry(c, true)).toBe("actionable");
  });

  it("W9 (design-a §3.2's ACTUAL rule): a long-enough QUESTION that trails off in a comma/semicolon/colon IS 'fragment_suspect' (the dangling-clause shape design-a actually specs)", () => {
    const text = "so what about the enterprise pricing tier we discussed,";
    expect(text.length).toBeGreaterThanOrEqual(AGENT_QUEUE_MIN_CHARS);
    const c = card({
      proposal: {
        kind: "question",
        title: "Question from Speaker 1",
        body: questionBody(text),
        confidence: 0.9,
      },
    });
    expect(classifyQueueEntry(c, true)).toBe("fragment_suspect");
  });

  it("W9: a well-formed QUESTION (long enough, confident, no dangling clause) is 'actionable'", () => {
    const c = card({
      proposal: {
        kind: "question",
        title: "Question from Speaker 1",
        body: questionBody("What did the team decide about pricing?"),
        confidence: 0.9,
      },
    });
    expect(classifyQueueEntry(c, true)).toBe("actionable");
  });

  it("LOCALE SAFETY (pt fixture): a short but well-formed Portuguese question is NOT misclassified by English-specific rules — no word lists, only length/punctuation shape. The canned prefix is always English (backend, not locale-aware); the pt utterance sits after it, exactly as production would produce for a pt-speaking session", () => {
    const text = "Você acha isso bom?"; // "Do you think that's good?"
    expect(text.length).toBeGreaterThanOrEqual(AGENT_QUEUE_MIN_CHARS);
    const c = card({
      proposal: {
        kind: "question",
        title: "Question from Speaker 1",
        body: questionBody(text),
        confidence: 0.9,
      },
    });
    expect(classifyQueueEntry(c, true)).toBe("actionable");
  });

  it("audio-graph-83cc T4 fix-round (major): a user-typed question (origin: 'user') is ALWAYS 'actionable' regardless of confidence/length/punctuation — W9's fragment rules exist for transcript-detection guesses, not text the user deliberately typed and submitted", () => {
    const shortLowConfidence = card({
      origin: "user",
      proposal: {
        kind: "question",
        title: "Question",
        body: "hi", // below AGENT_QUEUE_MIN_CHARS, below AGENT_QUEUE_MIN_TOKENS
        confidence: 0, // below AGENT_QUEUE_CONFIDENCE_FLOOR
      },
    });
    expect(classifyQueueEntry(shortLowConfidence, true)).toBe("actionable");

    const danglingClause = card({
      origin: "user",
      proposal: {
        kind: "question",
        title: "Question",
        body: "so what about the enterprise pricing tier we discussed,",
        confidence: 0.9,
      },
    });
    expect(classifyQueueEntry(danglingClause, true)).toBe("actionable");
  });

  it("a user-typed card stays 'info' when genuinely non-actionable — the origin exemption never overrides the structural isActionable check", () => {
    const c = card({ origin: "user", status: "dismissed" });
    expect(classifyQueueEntry(c, false)).toBe("info");
  });
});

/** Word-count helper mirroring `agentQueue.ts`'s own private tokenizer, kept
 * here (not imported — it is intentionally not exported) so this test file
 * can assert a fixture actually crosses the threshold it claims to, rather
 * than asserting on a hand-counted magic number that could silently drift
 * from the real tokenizer. */
function queueContentTokenCountForTest(text: string): number {
  const trimmed = text.trim();
  return trimmed.length === 0 ? 0 : trimmed.split(/\s+/).length;
}

describe("queueDuplicateKey — content-keyed, not title-keyed (blocker fix)", () => {
  it("two proposals with the SAME formulaic constant title but DIFFERENT content produce DIFFERENT keys", () => {
    const a = proposal({
      kind: "graph_suggestion",
      title: "Possible graph update",
      body: graphSuggestionBody("follow up with legal about the NDA"),
    });
    const b = proposal({
      kind: "graph_suggestion",
      title: "Possible graph update",
      body: graphSuggestionBody("decide on the Q3 roadmap"),
    });
    expect(queueDuplicateKey(a)).not.toBe(queueDuplicateKey(b));
  });

  it("two proposals with DIFFERENT titles but the SAME normalized content produce the SAME key", () => {
    const a = proposal({
      kind: "question",
      title: "Question from Speaker 1",
      body: questionBody("What did the team decide about pricing?"),
    });
    const b = proposal({
      kind: "question",
      title: "Question from Speaker 2",
      body: questionBody("  WHAT did the team decide about pricing?  "),
    });
    expect(queueDuplicateKey(a)).toBe(queueDuplicateKey(b));
  });
});

describe("admitToQueue — the Signal-mode fragment-suspect gate (ticket W9; W8 shipped this as an unconditional admit)", () => {
  it("admits 'actionable' and 'info' classifications", () => {
    expect(admitToQueue(card(), "actionable")).toBe(true);
    expect(admitToQueue(card(), "info")).toBe(true);
  });

  it("rejects 'fragment_suspect' — the seam is no longer unconditional", () => {
    expect(admitToQueue(card(), "fragment_suspect")).toBe(false);
  });
});

describe("selectAgentQueue — the queue/feed split (ticket W8)", () => {
  it("puts actionable pending proposals in the queue and everything else in the feed", () => {
    const actionableProposal = proposal({ id: "actionable-1" });
    const resolved = card({
      status: "approved",
      proposal: { id: "resolved-1" },
    });
    const historicalPending = card({
      status: "pending",
      proposal: { id: "historical-1" },
    });

    const { queue, feed } = selectAgentQueue(
      [resolved, historicalPending],
      [actionableProposal],
    );

    expect(queue.map((c) => c.proposal.id)).toEqual(["actionable-1"]);
    expect(feed.map((c) => c.proposal.id).sort()).toEqual([
      "historical-1",
      "resolved-1",
    ]);
  });

  it("returns empty queue and feed when there is nothing at all", () => {
    const { queue, feed } = selectAgentQueue([], []);
    expect(queue).toEqual([]);
    expect(feed).toEqual([]);
  });

  /**
   * MUTATION PROOF (ticket W8 test requirement: "admitToQueue seam pinned —
   * a rejected entry disappears from the queue render — prove the seam is
   * live by mutating the predicate to () => false"). Confirms `admitToQueue`
   * is really consulted (not dead code) by asserting the DEFAULT behavior
   * first, then passing a `() => false` predicate through
   * `selectAgentQueue`'s explicit override parameter — the SAME shape a
   * `vi.spyOn`-based test would prove, but reliable regardless of ESM
   * same-module self-call semantics (spying on an export does not
   * intercept that export's OWN module calling it directly).
   */
  it("proves the seam is live: an admit predicate mutated to () => false empties the queue", () => {
    const actionableProposal = proposal({ id: "actionable-1" });

    const before = selectAgentQueue([], [actionableProposal]);
    expect(before.queue).toHaveLength(1);
    // The default parameter really is the exported `admitToQueue` seam, not
    // a look-alike copy — calling it directly agrees with the default.
    expect(admitToQueue(before.queue[0], "actionable")).toBe(true);

    const after = selectAgentQueue([], [actionableProposal], () => false);
    expect(after.queue).toHaveLength(0);
    // The rejected entry must still be reachable — it moves to the feed,
    // never vanishes outright (design-a §3.1: "the queue is a priority
    // filter, never a capability gate").
    expect(after.feed.map((c) => c.proposal.id)).toEqual(["actionable-1"]);
  });

  /**
   * GREP-PIN (review finding: the seam-liveness test above proves the
   * INJECTED `admit` override is consulted, and separately that
   * `admitToQueue(...) === true` — but neither proves `selectAgentQueue`'s
   * DEFAULT parameter is bound to the real exported `admitToQueue`, not a
   * look-alike `() => true` copy. Mirrors the `statusClass` grep-pin this
   * same ticket ships in `AgentProposalsPanel.test.tsx`.) Reads the live
   * source file so severing the binding (e.g. `= admitToQueue` ->
   * `= () => true`) fails this test even though every behavioral assertion
   * above stays green.
   */
  it("selectAgentQueue's default `admit` parameter is source-bound to the exported admitToQueue seam", () => {
    const source = readFileSync(
      "src/components/workspace/agentQueue.ts",
      "utf8",
    );
    const selectAgentQueueSource = source.slice(
      source.indexOf("export function selectAgentQueue"),
    );
    // A refactor that severs the binding (e.g. `= admitToQueue` ->
    // `= () => true`) leaves every behavioral assertion above green —
    // this is the one assertion that would catch it.
    expect(selectAgentQueueSource).toMatch(
      /\)\s*=>\s*boolean\s*=\s*admitToQueue,/,
    );
  });
});

describe("selectAgentQueue — ticket W9's fragment filtering, default (Signal) admit", () => {
  it("PRODUCTION-SHAPED (blocker fix): a real backend question title ('Question from Speaker 1') with a well-formed underlying question stays in the QUEUE by default — Signal mode does not hide it", () => {
    const wellFormed = proposal({
      id: "well-formed-1",
      kind: "question",
      title: "Question from Speaker 1",
      body: questionBody("Is this the final budget for the quarter?"),
      confidence: 0.87,
    });

    const { queue, feed } = selectAgentQueue([], [wellFormed]);

    expect(queue.map((c) => c.proposal.id)).toEqual(["well-formed-1"]);
    expect(feed).toHaveLength(0);
  });

  it("a low-quality QUESTION fragment is filtered from the queue into the feed by DEFAULT (no override passed) — 'seam is live under the real admitToQueue, not just under an injected () => false'", () => {
    const fragment = proposal({
      id: "fragment-1",
      kind: "question",
      title: "Question from Speaker 1",
      body: questionBody("what about"),
      confidence: 0.9,
    });

    const { queue, feed, fragmentSuspectIds } = selectAgentQueue(
      [],
      [fragment],
    );

    expect(queue).toHaveLength(0);
    expect(feed.map((c) => c.proposal.id)).toEqual(["fragment-1"]);
    expect(fragmentSuspectIds.has("fragment-1")).toBe(true);
  });

  /**
   * DUPLICATE-COLLAPSE INTEGRATION (mutation-verified): a fragment proposal
   * ("what about") followed by the full, well-formed question it was a
   * prefix of — the SAME scenario design-a §3.2 names. The fragment is
   * caught by the sentence-shape rule (too short); the full question passes
   * every rule — net effect, exactly ONE actionable ("Signal") entry reaches
   * the queue, and the fragment is demoted to the feed rather than deleted.
   * Both carry the identical real backend title ("Question from Speaker
   * 1") — proving the split is driven by content, not by title.
   */
  it("fragment then full question -> exactly one Signal (queue) entry; the fragment survives in the feed", () => {
    const fragment = proposal({
      id: "fragment-1",
      kind: "question",
      title: "Question from Speaker 1",
      body: questionBody("what about"),
      confidence: 0.9,
      created_at_ms: 1,
    });
    const fullQuestion = proposal({
      id: "full-1",
      kind: "question",
      title: "Question from Speaker 1",
      body: questionBody(
        "What about the enterprise pricing tier we discussed?",
      ),
      confidence: 0.9,
      created_at_ms: 2,
    });

    const { queue, feed } = selectAgentQueue([], [fragment, fullQuestion]);

    expect(queue.map((c) => c.proposal.id)).toEqual(["full-1"]);
    expect(feed.map((c) => c.proposal.id)).toEqual(["fragment-1"]);
  });

  it("BLOCKER FIX: three DISTINCT graph suggestions sharing the identical constant production title ('Possible graph update') all stay actionable — no false collapse", () => {
    const a = proposal({
      id: "gs-a",
      kind: "graph_suggestion",
      title: "Possible graph update",
      body: graphSuggestionBody("follow up with legal about the NDA"),
      created_at_ms: 1,
    });
    const b = proposal({
      id: "gs-b",
      kind: "graph_suggestion",
      title: "Possible graph update",
      body: graphSuggestionBody("decide on the Q3 roadmap"),
      created_at_ms: 2,
    });
    const c = proposal({
      id: "gs-c",
      kind: "graph_suggestion",
      title: "Possible graph update",
      body: graphSuggestionBody("action item: ship the migration plan"),
      created_at_ms: 3,
    });

    const { queue, feed } = selectAgentQueue([], [a, b, c]);

    expect(queue.map((card) => card.proposal.id).sort()).toEqual([
      "gs-a",
      "gs-b",
      "gs-c",
    ]);
    expect(feed).toHaveLength(0);
  });

  it("the literal duplicate case: an exact verbatim repeat of the underlying CONTENT collapses to one queue entry, newest wins, even with the identical constant title on both", () => {
    const older = proposal({
      id: "dup-older",
      kind: "graph_suggestion",
      title: "Possible graph update",
      body: graphSuggestionBody("Acme evaluates Postgres"),
      created_at_ms: 1,
    });
    const newer = proposal({
      id: "dup-newer",
      kind: "graph_suggestion",
      title: "Possible graph update",
      body: graphSuggestionBody("  ACME evaluates postgres  "), // same after normalization
      created_at_ms: 2,
    });

    const { queue, feed, fragmentSuspectIds, duplicateCounts } =
      selectAgentQueue([], [older, newer]);

    expect(queue.map((c) => c.proposal.id)).toEqual(["dup-newer"]);
    expect(feed.map((c) => c.proposal.id)).toEqual(["dup-older"]);
    expect(fragmentSuspectIds.has("dup-older")).toBe(true);
    expect(fragmentSuspectIds.has("dup-newer")).toBe(false);
    expect(duplicateCounts.get("dup-newer")).toBe(2);
  });

  /**
   * "NOBODY WINS" FIX (review finding): a fragment-suspect NEWEST twin
   * (bad quality, not a duplicate of anything) must NOT burn the content
   * key for an OLDER well-formed twin with identical content. Pre-fix,
   * `seenTitles`/`seenContent` registered every actionable card
   * unconditionally, so the older twin also collapsed to fragment_suspect
   * (via the duplicate rule) even though nothing well-formed survived.
   * Post-fix: only a card that ends up NOT fragment_suspect is registered,
   * so the older, well-formed twin becomes the survivor instead.
   */
  it("a low-confidence newest twin does not suppress an older well-formed twin with identical content — the older one survives as actionable", () => {
    const newestBad = proposal({
      id: "newest-bad",
      kind: "question",
      title: "Question from Speaker 1",
      body: questionBody("Is this the final budget for the quarter?"),
      confidence: 0.1, // below the floor
      created_at_ms: 2,
    });
    const olderGood = proposal({
      id: "older-good",
      kind: "question",
      title: "Question from Speaker 1",
      body: questionBody("Is this the final budget for the quarter?"),
      confidence: 0.9,
      created_at_ms: 1,
    });

    const { queue, feed, fragmentSuspectIds } = selectAgentQueue(
      [],
      [newestBad, olderGood],
    );

    expect(queue.map((c) => c.proposal.id)).toEqual(["older-good"]);
    expect(feed.map((c) => c.proposal.id)).toEqual(["newest-bad"]);
    expect(fragmentSuspectIds.has("newest-bad")).toBe(true);
    expect(fragmentSuspectIds.has("older-good")).toBe(false);
  });

  it("duplicateCounts (design-a §3.2 rule 2's '×N'): a three-way duplicate group reports the surviving id -> total group size of 3", () => {
    const proposals = [1, 2, 3].map((n) =>
      proposal({
        id: `dup-${n}`,
        kind: "note",
        title: `Context from Speaker ${n}`,
        body: noteBody("the migration deadline moved to Friday"),
        created_at_ms: n,
      }),
    );

    const { queue, feed, duplicateCounts } = selectAgentQueue([], proposals);

    expect(queue.map((c) => c.proposal.id)).toEqual(["dup-3"]);
    expect(feed.map((c) => c.proposal.id).sort()).toEqual(["dup-1", "dup-2"]);
    expect(duplicateCounts.get("dup-3")).toBe(3);
    // Victims never carry their own entry in the map.
    expect(duplicateCounts.has("dup-1")).toBe(false);
    expect(duplicateCounts.has("dup-2")).toBe(false);
  });

  it("no duplicateCounts entry at all for a solo (non-duplicated) card", () => {
    const solo = proposal({ id: "solo-1", kind: "note" });
    const { duplicateCounts } = selectAgentQueue([], [solo]);
    expect(duplicateCounts.size).toBe(0);
  });

  it("audio-graph-83cc T4 fix-round (major): two user-typed cards with IDENTICAL content are NOT duplicate-collapsed — asking the same question twice on purpose must not hide the second thread", () => {
    const first = liveAssistCardFromProposal(
      proposal({
        id: "user-q-1",
        kind: "question",
        body: questionBody("What did the team decide about pricing?"),
        confidence: 0.9,
        created_at_ms: 1,
      }),
    );
    first.origin = "user";
    const second = liveAssistCardFromProposal(
      proposal({
        id: "user-q-2",
        kind: "question",
        body: questionBody("What did the team decide about pricing?"),
        confidence: 0.9,
        created_at_ms: 2,
      }),
    );
    second.origin = "user";

    const { queue, feed, fragmentSuspectIds, duplicateCounts } =
      selectAgentQueue([first, second], [first.proposal, second.proposal]);

    expect(queue.map((c) => c.proposal.id).sort()).toEqual([
      "user-q-1",
      "user-q-2",
    ]);
    expect(feed).toHaveLength(0);
    expect(fragmentSuspectIds.size).toBe(0);
    expect(duplicateCounts.size).toBe(0);
  });

  it("audio-graph-83cc T4 fix-round: a user-typed card never registers as a duplicate SURVIVOR either — it does not silently absorb a later transcript-derived card with the same content", () => {
    const userCard = liveAssistCardFromProposal(
      proposal({
        id: "user-q",
        kind: "question",
        body: questionBody("What did the team decide about pricing?"),
        confidence: 0.9,
        created_at_ms: 2,
      }),
    );
    userCard.origin = "user";
    const transcriptDuplicate = proposal({
      id: "transcript-q",
      kind: "question",
      title: "Question from Speaker 1",
      body: questionBody("What did the team decide about pricing?"),
      confidence: 0.9,
      created_at_ms: 1,
    });

    const { queue, feed, duplicateCounts } = selectAgentQueue(
      [userCard],
      [userCard.proposal, transcriptDuplicate],
    );

    // Neither is collapsed onto the other — the user card was never
    // registered as a `seenContent` survivor, so the older transcript-derived
    // card is classified on its own well-formed merits, not as a duplicate.
    expect(queue.map((c) => c.proposal.id).sort()).toEqual([
      "transcript-q",
      "user-q",
    ]);
    expect(feed).toHaveLength(0);
    expect(duplicateCounts.size).toBe(0);
  });
});

describe("selectAgentQueue — the Signal/All toggle's real override (ticket W9)", () => {
  /**
   * TOGGLE MUTATION PROOF: Signal mode (the real `admitToQueue` seam, no
   * override) hides a fragment-suspect card; passing the real All-mode
   * predicate (`() => true`, the exact override `AgentProposalsPanel` uses)
   * reveals it in the queue instead — same underlying store data both
   * times, only the `admit` predicate differs. `fragmentSuspectIds` marks it
   * either way, which is what lets a renderer show the "subtle fragment
   * marker" on whichever side it lands.
   */
  it("Signal mode (default) hides a fragment-suspect card; the All-mode predicate reveals it, unmodified data both times", () => {
    const fragment = proposal({
      id: "fragment-1",
      kind: "question",
      title: "Question from Speaker 1",
      body: questionBody("is good?"),
      confidence: 0.9,
    });

    const signal = selectAgentQueue([], [fragment], admitToQueue);
    expect(signal.queue).toHaveLength(0);
    expect(signal.feed.map((c) => c.proposal.id)).toEqual(["fragment-1"]);
    expect(signal.fragmentSuspectIds.has("fragment-1")).toBe(true);

    const all = selectAgentQueue([], [fragment], () => true);
    expect(all.queue.map((c) => c.proposal.id)).toEqual(["fragment-1"]);
    expect(all.feed).toHaveLength(0);
    // Still flagged even though it is now admitted — this is exactly the
    // bit a renderer checks to draw the marker in All mode.
    expect(all.fragmentSuspectIds.has("fragment-1")).toBe(true);
  });
});
