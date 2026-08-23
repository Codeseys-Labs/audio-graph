import { describe, expect, it } from "vitest";
import {
  agentOutcomeChipTone,
  LANE_RECENCY_WARNING_TURNS_THRESHOLD,
  type LaneRecencySourcePatch,
  type LaneRecencySourceRevision,
  laneRecencyChipTone,
  lastLanePatchAtMs,
  selectLaneRecency,
  selectTurnsBehind,
} from "./liveWorkspaceTone";

function patch(
  kind: string,
  createdAtMs: number,
  queuedAtMs?: number | null,
): LaneRecencySourcePatch {
  return { kind, created_at_ms: createdAtMs, queued_at_ms: queuedAtMs };
}

function revision(
  overrides: Partial<LaneRecencySourceRevision> & { receivedAtMs: number },
): LaneRecencySourceRevision {
  return {
    is_final: false,
    end_of_turn: false,
    stability: "partial",
    turn_id: null,
    span_id: `span-${overrides.receivedAtMs}`,
    received_at_ms: overrides.receivedAtMs,
    ...overrides,
  };
}

describe("liveWorkspaceTone — phase-1-never-success pin (design-a §8, synthesis §2)", () => {
  it("evidence:null never renders success even at turnsBehind:0 with a fresh lastAppliedAtMs", () => {
    const result = laneRecencyChipTone({
      lastAppliedAtMs: Date.now(),
      turnsBehind: 0,
      evidence: null,
      isLiveSession: true,
    });

    expect(result.render).toBe(true);
    expect(result.tone).not.toBe("success");
    expect(result.tone).toBe("neutral");
    expect(result.behind).toBe(false);
  });

  it("evidence:null never renders success even when the caller fabricates a current-looking input shape", () => {
    // A hostile/careless caller might pass every OTHER field as if the
    // lane were maximally healthy (turnsBehind 0, a just-now timestamp).
    // Only `evidence` gates success — nothing else may substitute for it.
    const result = laneRecencyChipTone({
      lastAppliedAtMs: Date.now() - 1,
      turnsBehind: 0,
      evidence: null,
      isLiveSession: true,
    });

    expect(result.tone).not.toBe("success");
  });

  it("evidence:'appended_tail' (lag IS present evidence) stays neutral, never success and never warning by itself", () => {
    const result = laneRecencyChipTone({
      lastAppliedAtMs: Date.now(),
      turnsBehind: 0,
      evidence: "appended_tail",
      isLiveSession: true,
    });

    expect(result.tone).toBe("neutral");
  });

  it("evidence:'current' is the ONLY input that can ever unlock success (future W3 call site, not exercised by any real caller today)", () => {
    const result = laneRecencyChipTone({
      lastAppliedAtMs: Date.now(),
      turnsBehind: 0,
      evidence: "current",
      isLiveSession: true,
    });

    expect(result.tone).toBe("success");
  });
});

describe("liveWorkspaceTone — warning threshold boundary (design-a §2.4: >=3 turns behind)", () => {
  it("2 turns behind renders neutral, not warning", () => {
    const result = laneRecencyChipTone({
      lastAppliedAtMs: 1000,
      turnsBehind: 2,
      evidence: null,
      isLiveSession: true,
    });

    expect(result.behind).toBe(false);
    expect(result.tone).toBe("neutral");
  });

  it("3 turns behind renders warning (the ratified threshold, inclusive)", () => {
    const result = laneRecencyChipTone({
      lastAppliedAtMs: 1000,
      turnsBehind: LANE_RECENCY_WARNING_TURNS_THRESHOLD,
      evidence: null,
      isLiveSession: true,
    });

    expect(result.behind).toBe(true);
    expect(result.tone).toBe("warning");
  });
});

describe("liveWorkspaceTone — render gate", () => {
  it("renders nothing when the lane has never produced an accepted patch (lastAppliedAtMs: null)", () => {
    const result = laneRecencyChipTone({
      lastAppliedAtMs: null,
      turnsBehind: 99,
      evidence: null,
      isLiveSession: true,
    });

    expect(result.render).toBe(false);
  });

  it("renders nothing for a loaded/reviewed session — no freshness claim about finished history", () => {
    const result = laneRecencyChipTone({
      lastAppliedAtMs: 1000,
      turnsBehind: 0,
      evidence: null,
      isLiveSession: false,
    });

    expect(result.render).toBe(false);
  });
});

describe("lastLanePatchAtMs — kind-scoped max created_at_ms", () => {
  it("ignores patches of the other kind and returns the latest matching one", () => {
    const patches = [
      patch("notes", 100),
      patch("graph", 500),
      patch("notes", 300),
      patch("graph", 200),
    ];

    expect(lastLanePatchAtMs(patches, "notes")).toBe(300);
    expect(lastLanePatchAtMs(patches, "graph")).toBe(500);
  });

  it("returns null when the lane has no patches at all", () => {
    expect(lastLanePatchAtMs([patch("graph", 500)], "notes")).toBeNull();
    expect(lastLanePatchAtMs([], "notes")).toBeNull();
  });
});

// NOTE: every case below isolates exactly ONE of the three OR conditions to
// pin the literal gate this module mirrors. In PRODUCTION these three fields
// are always set in lockstep by every emitter (see the module doc's
// DISCLOSURE comment on `isFinalizedTurnRevision`) — no real
// `AsrSpanRevisionEvent` has exactly one or two of them set, so these cases
// exercise shapes that cannot occur on the wire today. They still earn their
// keep as a literal-fidelity pin against the backend source, and as a
// tripwire if a future backend change breaks that lockstep.
describe("selectTurnsBehind — mirrors the backend's real OR gate (verified against src-tauri/src/speech/mod.rs, NOT the design docs' stated AND)", () => {
  it("counts a revision with is_final:true even when end_of_turn is false", () => {
    const revisions = [
      revision({
        receivedAtMs: 200,
        is_final: true,
        end_of_turn: false,
        turn_id: "t1",
      }),
    ];

    expect(selectTurnsBehind(revisions, 100)).toBe(1);
  });

  it("counts a revision with end_of_turn:true even when is_final is false — the AND from the design docs would wrongly drop this", () => {
    const revisions = [
      revision({
        receivedAtMs: 200,
        is_final: false,
        end_of_turn: true,
        turn_id: "t1",
      }),
    ];

    expect(selectTurnsBehind(revisions, 100)).toBe(1);
  });

  it("counts a revision with stability:'final' even when neither is_final nor end_of_turn is set", () => {
    const revisions = [
      revision({
        receivedAtMs: 200,
        is_final: false,
        end_of_turn: false,
        stability: "final",
        turn_id: "t1",
      }),
    ];

    expect(selectTurnsBehind(revisions, 100)).toBe(1);
  });

  it("does NOT count a revision that satisfies none of the three gate conditions", () => {
    const revisions = [
      revision({
        receivedAtMs: 200,
        is_final: false,
        end_of_turn: false,
        stability: "partial",
        turn_id: "t1",
      }),
    ];

    expect(selectTurnsBehind(revisions, 100)).toBe(0);
  });

  it("excludes revisions at or before sinceMs (only strictly-after counts)", () => {
    const revisions = [
      revision({ receivedAtMs: 100, is_final: true, turn_id: "t1" }),
      revision({ receivedAtMs: 101, is_final: true, turn_id: "t2" }),
    ];

    expect(selectTurnsBehind(revisions, 100)).toBe(1);
  });

  it("deduplicates by turn_id — multiple finalizing revisions of the same turn count once", () => {
    const revisions = [
      revision({
        receivedAtMs: 200,
        end_of_turn: true,
        turn_id: "t1",
      }),
      revision({
        receivedAtMs: 250,
        is_final: true,
        turn_id: "t1",
      }),
      revision({
        receivedAtMs: 300,
        is_final: true,
        turn_id: "t2",
      }),
    ];

    expect(selectTurnsBehind(revisions, 100)).toBe(2);
  });

  it("falls back to span_id when turn_id is null, so a turn-id-less finalized revision still counts", () => {
    const revisions = [
      revision({ receivedAtMs: 200, is_final: true, turn_id: null }),
    ];

    expect(selectTurnsBehind(revisions, 100)).toBe(1);
  });

  it("returns 0 when sinceMs is null (no patch to be behind relative to yet)", () => {
    const revisions = [
      revision({ receivedAtMs: 200, is_final: true, turn_id: "t1" }),
    ];

    expect(selectTurnsBehind(revisions, null)).toBe(0);
  });
});

describe("selectLaneRecency — the one shared computation, two call sites", () => {
  it("computes lastAppliedAtMs and turnsBehind together for a given lane kind", () => {
    const patches = [patch("notes", 100), patch("graph", 400)];
    const revisions = [
      revision({ receivedAtMs: 150, is_final: true, turn_id: "t1" }),
      revision({ receivedAtMs: 450, is_final: true, turn_id: "t2" }),
    ];

    const notes = selectLaneRecency("notes", patches, revisions);
    expect(notes.lastAppliedAtMs).toBe(100);
    expect(notes.turnsBehind).toBe(2);

    const graph = selectLaneRecency("graph", patches, revisions);
    expect(graph.lastAppliedAtMs).toBe(400);
    expect(graph.turnsBehind).toBe(1);
  });

  it("counts a revision that arrived DURING generation (between queued_at_ms and created_at_ms) as behind — the flattering-direction fix", () => {
    // Patch queued at 100, finished (created) at 500 — generation took 400ms.
    // A revision landing at 300 (mid-generation) is provably NOT reflected
    // in this patch's content, even though 300 < 500 (created_at_ms).
    const patches = [patch("notes", 500, 100)];
    const revisions = [
      revision({ receivedAtMs: 300, is_final: true, turn_id: "t1" }),
    ];

    const notes = selectLaneRecency("notes", patches, revisions);
    expect(notes.lastAppliedAtMs).toBe(500); // display fact: unchanged
    expect(notes.turnsBehind).toBe(1); // counted: cutoff is queued_at_ms
  });

  it("does NOT count a revision that arrived before queueing even started", () => {
    const patches = [patch("notes", 500, 100)];
    const revisions = [
      revision({ receivedAtMs: 50, is_final: true, turn_id: "t1" }),
    ];

    const notes = selectLaneRecency("notes", patches, revisions);
    expect(notes.turnsBehind).toBe(0);
  });

  it("falls back to created_at_ms as the turn-count cutoff when queued_at_ms is absent (every pre-existing caller/test)", () => {
    const patches = [patch("notes", 500)]; // no queued_at_ms
    const revisions = [
      revision({ receivedAtMs: 300, is_final: true, turn_id: "t1" }), // before created_at_ms
      revision({ receivedAtMs: 600, is_final: true, turn_id: "t2" }), // after
    ];

    const notes = selectLaneRecency("notes", patches, revisions);
    expect(notes.lastAppliedAtMs).toBe(500);
    expect(notes.turnsBehind).toBe(1); // only the post-500 revision counts
  });
});

describe("agentOutcomeChipTone — the law's third surface (ticket W8, design-a §8 S3)", () => {
  it("an approved card WITH a recorded outcome renders success, effectiveStatus 'ready'", () => {
    const result = agentOutcomeChipTone({
      status: "approved",
      hasOutcome: true,
    });
    expect(result.tone).toBe("success");
    expect(result.effectiveStatus).toBe("ready");
  });

  it("an approved card with NO recorded outcome (null/undefined) does NOT render success — demotes to 'unchecked'/neutral (the pinned regression case)", () => {
    const result = agentOutcomeChipTone({
      status: "approved",
      hasOutcome: false,
    });
    expect(result.tone).not.toBe("success");
    expect(result.tone).toBe("neutral");
    expect(result.effectiveStatus).toBe("unchecked");
  });

  it("pending renders accent (a planned action, not yet an observed success/failure), effectiveStatus passes through unchanged", () => {
    const result = agentOutcomeChipTone({
      status: "pending",
      hasOutcome: false,
    });
    expect(result.tone).toBe("accent");
    expect(result.effectiveStatus).toBe("pending");
  });

  it("dismissed renders neutral, effectiveStatus passes through unchanged", () => {
    const result = agentOutcomeChipTone({
      status: "dismissed",
      hasOutcome: false,
    });
    expect(result.tone).toBe("neutral");
    expect(result.effectiveStatus).toBe("dismissed");
  });

  it("dismissed ignores hasOutcome entirely — only the 'approved'->'ready' axis arm ever consults it", () => {
    const withOutcome = agentOutcomeChipTone({
      status: "dismissed",
      hasOutcome: true,
    });
    const withoutOutcome = agentOutcomeChipTone({
      status: "dismissed",
      hasOutcome: false,
    });
    expect(withOutcome.tone).toBe(withoutOutcome.tone);
    expect(withOutcome.effectiveStatus).toBe(withoutOutcome.effectiveStatus);
  });
});
