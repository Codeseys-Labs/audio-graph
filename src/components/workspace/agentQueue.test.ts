import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { AgentProposalEvent, LiveAssistCardRecord } from "../../types";
import {
  admitToQueue,
  classifyQueueEntry,
  liveAssistCardFromProposal,
  mergeLiveAssistCards,
  selectAgentQueue,
} from "./agentQueue";

let seq = 0;

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

describe("classifyQueueEntry — the classification table (actionable / info / fragment_suspect)", () => {
  it("classifies an actionable card as 'actionable'", () => {
    expect(classifyQueueEntry(card(), true)).toBe("actionable");
  });

  it("classifies a non-actionable card (resolved, or historical pending) as 'info'", () => {
    expect(classifyQueueEntry(card({ status: "approved" }), false)).toBe(
      "info",
    );
    expect(classifyQueueEntry(card({ status: "dismissed" }), false)).toBe(
      "info",
    );
    expect(classifyQueueEntry(card({ status: "pending" }), false)).toBe("info");
  });

  it("never returns 'fragment_suspect' in W8 — that class is reserved for seed 104f / W9's heuristic", () => {
    expect(classifyQueueEntry(card(), true)).not.toBe("fragment_suspect");
    expect(classifyQueueEntry(card(), false)).not.toBe("fragment_suspect");
  });
});

describe("admitToQueue — the 104f seam, shipped as an unconditional admit", () => {
  it("admits every classification today ('() => true' in substance)", () => {
    expect(admitToQueue(card(), "actionable")).toBe(true);
    expect(admitToQueue(card(), "info")).toBe(true);
    expect(admitToQueue(card(), "fragment_suspect")).toBe(true);
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
