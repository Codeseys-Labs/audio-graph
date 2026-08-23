/**
 * `liveWorkspaceTone` — the workspace tone-law module (ticket W6, synthesis
 * audio-graph-a6b5 §2 "The recency chip, resolved"; ticket W8 extended it to
 * a third surface, §8's "S3"). Extends the T2 tone law
 * (`settings/readinessTone.ts`) to the live-workspace tiles that make an
 * OBSERVED-shaped claim: the document tile (notes-lane freshness), the
 * graph tile (graph-lane freshness), and — as of W8 — the agent tile's
 * proposal status chip (an "approved" outcome claim, not a freshness claim,
 * but the SAME law: an unevidenced claim demotes to neutral). See
 * `agentOutcomeChipTone` below for the third wrapper.
 *
 * PHASE-1 HONESTY (L3 law, design-a §0/§8): this app has NO observation of
 * either projection lane's health yet — only an inference from event
 * timestamps (`lastAppliedAtMs`) and from ASR turn-finalization events
 * (`turnsBehind`). W3 (`basis_currency_at_apply`, additive on the emitted
 * patch) is the ONLY thing that can ever turn a lane's chip green, and it
 * has NOT landed. `LaneRecencyToneInput.evidence` therefore always arrives
 * as `null` from every real call site in this ticket — the type carries the
 * field now (design-a §8's "one boolean is the entire distance between
 * honest-neutral and an earned green") so a later ticket wires the real
 * value through. NOT a zero-shape-change wire-up, though: the real backend
 * field (`AppliedBasisCurrency`, `src-tauri/src/projections.rs`) is
 * `#[serde(tag = "type", ...)]` — it serializes as an object
 * (`{"type":"current"}` / `{"type":"appended_tail","staleness":{...}}`),
 * not a bare string. `BasisCurrencyEvidence` narrows to the two tag VALUES
 * this module's tone logic cares about; W3's call site will need a
 * `patch.basis_currency_at_apply?.type` mapping step to produce this type,
 * not a direct assignment. `laneRecencyChipTone` is written so
 * `evidence !== "current"` structurally cannot reach `tone: "success"` —
 * see `liveWorkspaceTone.test.ts`'s pin.
 *
 * `selectLaneRecency` is the ONE computation design-a §2.4 promises ("one
 * function, two call sites") — `kind: "notes"` drives the document chip,
 * `kind: "graph"` drives the graph chip. `lastLanePatchAtMs` generalizes
 * ticket W7's own `lastGraphPatchAtMs` (previously a `LiveGraphStrip.tsx`
 * local) by a `kind` parameter — W7's call site now imports this instead of
 * keeping its own copy, per this ticket's "reuse the graph lane's existing
 * derivation, do not fork a second one" constraint.
 *
 * KNOWN LIMIT, disclosed rather than fixed here: `turnsBehind` counts
 * DISTINCT finalized ASR span revisions after the cutoff, not conversational
 * turns. Providers that emit several finalized revisions per spoken
 * utterance without a stable `turn_id` (verified: Deepgram, connected with
 * `interim_results: true` in `src-tauri/src/asr/deepgram.rs`, which never
 * sets `turn_id` and hardcodes a fresh `provider_start_span_id(...)` per
 * `is_final` result) will over-count relative to real turns. The store
 * separately carries `turnEvents: TurnLifecycleEvent[]`
 * (`store/index.ts`/`types/index.ts`), fed from Deepgram's `speech_final`
 * signal — a truer per-turn stream — but its own type comment scopes it to
 * "Deepgram/local providers" only, so swapping to it wholesale would silently
 * zero out `turnsBehind` for every other adapter (assemblyai/speechmatics/
 * gladia/revai/soniox/moonshine), which is the FLATTERING direction design-a
 * §0 forbids and would need backend-side verification this frontend-only
 * ticket cannot perform. Overcounting is the safer of the two failure
 * directions (it reads LESS fresh than reality, never more) so it ships as
 * a disclosed limitation rather than blocking on that cross-provider audit.
 */

import type { LiveAssistCardStatus } from "../../types";
import { agentProposalStatusTone, type BadgeTone } from "../settings/badgeTone";
import { readinessChipTone } from "../settings/readinessTone";

/** Every projection lane this tone law currently covers. `ProjectionKind`
 * (`types/index.ts`) is wider (backend enum), but only these two lanes have
 * a live-workspace tile making a freshness claim today. */
export type ProjectionLaneKind = "notes" | "graph";

/** Structurally identical to the fields of `ProjectionPatch` this module
 * needs — declared locally so this file stays a small, framework-free,
 * trivially-unit-testable pure module (mirrors `liveDocumentModel.ts`'s own
 * "zero import surface" posture). Real `ProjectionPatch[]` values are
 * structurally compatible and pass directly. */
export interface LaneRecencySourcePatch {
  kind: string;
  created_at_ms: number;
  /** When present, the moment the patch's generation was QUEUED, strictly
   * before ASR/LLM generation ran (`commands.rs`'s
   * `projection_queue_ms = patch.created_at_ms - patch.queued_at_ms`).
   * `selectLaneRecency` uses this — not `created_at_ms` — as the cutoff for
   * counting turns "behind": a revision that arrived while the patch was
   * still being generated is provably NOT reflected in that patch, even
   * though its `received_at_ms` predates the patch's `created_at_ms`.
   * Falls back to `created_at_ms` when absent (matches every existing
   * caller/test unchanged). */
  queued_at_ms?: number | null;
}

/** Structurally identical to the fields of `AsrSpanRevisionEvent` this
 * module needs. `turn_id` is nullable on the wire; a revision with no
 * `turn_id` still counts (keyed by `span_id` instead — see
 * `selectTurnsBehind`) rather than being silently dropped from the count. */
export interface LaneRecencySourceRevision {
  is_final: boolean;
  end_of_turn: boolean;
  stability: "partial" | "final";
  turn_id?: string | null;
  span_id: string;
  received_at_ms: number;
}

/**
 * Find the most recent patch (by `created_at_ms`) among patches of the
 * given `kind`, or `null` when the lane has never produced an accepted
 * patch this session. Pure, O(n), single pass. Shared internal so
 * `lastLanePatchAtMs` and `selectLaneRecency` never scan the array twice or
 * disagree on which patch is "latest".
 */
function findLatestLanePatch(
  patches: readonly LaneRecencySourcePatch[],
  kind: ProjectionLaneKind,
): LaneRecencySourcePatch | null {
  let latest: LaneRecencySourcePatch | null = null;
  for (const patch of patches) {
    if (patch.kind !== kind) continue;
    if (latest === null || patch.created_at_ms > latest.created_at_ms) {
      latest = patch;
    }
  }
  return latest;
}

/**
 * Find the most recent `created_at_ms` among patches of the given `kind`,
 * or `null` when the lane has never produced an accepted patch this
 * session. The identical shape `LiveGraphStrip`'s pre-W6 `lastGraphPatchAtMs`
 * used, generalized by `kind` so notes and graph share one derivation
 * instead of two near-identical local copies.
 */
export function lastLanePatchAtMs(
  patches: readonly LaneRecencySourcePatch[],
  kind: ProjectionLaneKind,
): number | null {
  return findLatestLanePatch(patches, kind)?.created_at_ms ?? null;
}

/**
 * Mirrors the EXACT gate `observe_projection_schedulers_for_asr_revision`
 * uses (`src-tauri/src/speech/mod.rs`) to decide whether an ASR revision
 * event is worth re-observing the projection schedulers over — verified
 * against the code, not assumed from the design docs. That function's real
 * condition is:
 *
 *   `payload.is_final || payload.end_of_turn || payload.stability == Final`
 *
 * i.e. an OR of three conditions, NOT the `is_final && end_of_turn` AND
 * design-a §2.4 / the epic synthesis both state — that is a real
 * documentation-drift item (worth a wording-only correction against
 * `docs/agentic-runs/2026-08-23-live-workspace-design/`).
 *
 * DISCLOSURE: this OR-vs-AND distinction is currently BEHAVIORALLY INERT.
 * Every production emitter that constructs an `AsrSpanRevisionEvent` sets
 * `is_final`/`end_of_turn`/`stability` in lockstep from one boolean (see
 * `emit_transcript_and_extract_with_meta`, `emit_asr_partial_with_meta`, and
 * each provider adapter in `src-tauri/src/asr/*.rs` — verified across all 12
 * construction sites), so no event exists today with exactly one or two of
 * the three set. AND, OR, and a bare `is_final` check are all equivalent in
 * practice. This mirror uses the real OR gate (future-proofing against a
 * backend change that breaks the lockstep, and matching the literal backend
 * predicate) — not because the AND is currently under-counting anything.
 */
function isFinalizedTurnRevision(revision: LaneRecencySourceRevision): boolean {
  return (
    revision.is_final || revision.end_of_turn || revision.stability === "final"
  );
}

/**
 * Count DISTINCT finalized turns strictly after `sinceMs` — `null` (no
 * patch has ever landed for this lane) short-circuits to `0`: there is
 * nothing to be "behind" relative to yet, and a fresh session with no
 * accepted patch must not open with a false "3 turns behind" claim.
 *
 * Deduplicates by `turn_id`, falling back to `span_id` for a revision that
 * carries no `turn_id` (rather than excluding it from the count) — a real
 * finalized turn with an unset `turn_id` is still one turn, and dropping it
 * would silently under-report exactly the sessions where turn identity is
 * weakest.
 */
export function selectTurnsBehind(
  revisions: readonly LaneRecencySourceRevision[],
  sinceMs: number | null,
): number {
  if (sinceMs === null) return 0;
  const turns = new Set<string>();
  for (const revision of revisions) {
    if (revision.received_at_ms <= sinceMs) continue;
    if (!isFinalizedTurnRevision(revision)) continue;
    turns.add(revision.turn_id ?? revision.span_id);
  }
  return turns.size;
}

/** The one shared computation (design-a §2.4): `kind: "notes"` drives the
 * document chip, `kind: "graph"` drives the graph chip. Both read from the
 * SAME two arrays (`sessionProjectionEvents`, `asrSpanRevisions`) already
 * in the store — zero Rust, zero new events, zero polling.
 *
 * `lastAppliedAtMs` (the DISPLAYED "as of" fact) is the patch's
 * `created_at_ms` — correct as a display fact, it is genuinely when the
 * patch finished. The turn-count cutoff fed to `selectTurnsBehind` is a
 * DIFFERENT, earlier timestamp: `queued_at_ms` (when generation for that
 * patch was queued), falling back to `created_at_ms` when absent. Using
 * `created_at_ms` for both would put the tile in the flattering direction
 * design-a §0 forbids — a revision that arrived while the patch was still
 * being generated (`received_at_ms` between `queued_at_ms` and
 * `created_at_ms`) is provably NOT reflected in that patch's content, yet
 * `received_at_ms <= created_at_ms` would silently exclude it from the
 * count, understating how far behind the lane actually is. */
export function selectLaneRecency(
  kind: ProjectionLaneKind,
  patches: readonly LaneRecencySourcePatch[],
  revisions: readonly LaneRecencySourceRevision[],
): { lastAppliedAtMs: number | null; turnsBehind: number } {
  const latestPatch = findLatestLanePatch(patches, kind);
  const lastAppliedAtMs = latestPatch?.created_at_ms ?? null;
  const turnCountSinceMs = latestPatch
    ? (latestPatch.queued_at_ms ?? latestPatch.created_at_ms)
    : null;
  return {
    lastAppliedAtMs,
    turnsBehind: selectTurnsBehind(revisions, turnCountSinceMs),
  };
}

/** `turnsBehind >= this` is the ONLY thing that ever escalates a lane's
 * chip to warning in phase 1 (design-a §2.4's ratified threshold). */
export const LANE_RECENCY_WARNING_TURNS_THRESHOLD = 3;

/** W3's (not-yet-landed) `basis_currency_at_apply`, narrowed to the two
 * values that matter for tone (synthesis §2): `"current"` is the ONLY value
 * that can ever unlock `success`; `"appended_tail"` is present evidence of
 * LAG and stays neutral — it is not, by itself, a warning (the
 * `turnsBehind` threshold owns the warning arm independently). */
export type BasisCurrencyEvidence = "current" | "appended_tail";

export type LaneRecencyStatus = "ready" | "behind";

export interface LaneRecencyToneInput {
  /** ms epoch of the lane's most recent accepted patch, or `null` when the
   * lane has never produced one this session — there is no observed fact
   * to make ANY freshness claim about yet. */
  lastAppliedAtMs: number | null;
  turnsBehind: number;
  /**
   * W3's evidence field. ALWAYS `null` from every real call site in this
   * ticket (see module doc) — the ONLY caller that may ever pass a non-null
   * value is a future ticket that has actually wired
   * `basis_currency_at_apply` through from the backend.
   */
  evidence: BasisCurrencyEvidence | null;
  /** `false` for a loaded/reviewed session — no freshness claim is ever
   * made about a finished session's own history (design-a §1.7's "loaded
   * session" row, mirrored for both lanes). */
  isLiveSession: boolean;
}

export interface LaneRecencyChipResult {
  /** `false` — the caller renders NO chip at all: either there is nothing
   * to claim (`lastAppliedAtMs === null`) or this is not a live session. */
  render: boolean;
  /** Typed as the full `BadgeTone` union (matching `readinessChipTone`'s own
   * return type) even though this function's `statusToneMap` only ever
   * produces `"warning"` or `"success"` — narrowing it here would fight the
   * tone law's generic signature for no real benefit. */
  tone: BadgeTone;
  /** `true` at/above `LANE_RECENCY_WARNING_TURNS_THRESHOLD`. Callers switch
   * their copy key on THIS flag, not on `tone` — `tone` alone can't
   * distinguish "no evidence yet" neutral from a future evidenced
   * `success`, and copy must never claim more than `behind` says. */
  behind: boolean;
  turnsBehind: number;
  lastAppliedAtMs: number | null;
}

/**
 * The tone-law wrapper (design-a §8's `laneRecencyChipTone`). Routes
 * through `readinessChipTone` exactly like every other tone-law surface in
 * this repo: `automaticProbeAvailable: evidence === "current"` is the ONE
 * boolean gate that decides whether a `"ready"` status is even reachable —
 * with `evidence` anything other than `"current"` (which is every real call
 * site today: always `null`), the law itself demotes `"ready"` to neutral
 * "unchecked" before this function ever sees a tone, so `tone: "success"`
 * is structurally unreachable in phase 1. `liveWorkspaceTone.test.ts` pins
 * this with a fabricated "looks current" input (turnsBehind: 0, a fresh
 * `lastAppliedAtMs`) that still must not render success while
 * `evidence: null`.
 */
export function laneRecencyChipTone(
  input: LaneRecencyToneInput,
): LaneRecencyChipResult {
  if (input.lastAppliedAtMs === null || !input.isLiveSession) {
    return {
      render: false,
      tone: "neutral",
      behind: false,
      turnsBehind: input.turnsBehind,
      lastAppliedAtMs: input.lastAppliedAtMs,
    };
  }

  const behind = input.turnsBehind >= LANE_RECENCY_WARNING_TURNS_THRESHOLD;
  const axis = readinessChipTone<LaneRecencyStatus>(
    {
      status: behind ? "behind" : "ready",
      active: true,
      stale: false,
      automaticProbeAvailable: input.evidence === "current",
    },
    (status) => (status === "behind" ? "warning" : "success"),
  );

  return {
    render: true,
    tone: axis.tone,
    behind,
    turnsBehind: input.turnsBehind,
    lastAppliedAtMs: input.lastAppliedAtMs,
  };
}

/** The agent chip's post-law axis — `"approved"` is remapped to the law's
 * `"ready"` sentinel before `readinessChipTone` ever sees it (mirroring
 * `LaneRecencyStatus`'s own convention above); `"pending"`/`"dismissed"`
 * pass through unchanged since neither is the OBSERVED-eligible claim the
 * law gates. */
export type AgentChipAxisStatus = "ready" | "pending" | "dismissed";

export interface AgentOutcomeChipInput {
  /** The live-assist card's raw status. */
  status: LiveAssistCardStatus;
  /** Whether this card carries a recorded `AgentActionResult` outcome
   * (`card.outcome != null`). `false` for a null/undefined outcome — an
   * `"approved"` card with no outcome is NOT evidence of success
   * (design-a §8/S3: "an `approved` with a null/failed outcome must NOT
   * claim success"). Only consulted when `status === "approved"`. */
  hasOutcome: boolean;
}

export interface AgentOutcomeChipResult {
  tone: BadgeTone;
  /** The post-law axis status. Callers key their COPY off THIS field, never
   * off the raw `status` input — `"unchecked"` is what an unevidenced
   * `"approved"` demotes to, and the law "gates the claim, not just its
   * color" (readinessTone.ts): the rendered label must ALSO stop saying
   * "Approved" when this is `"unchecked"`. */
  effectiveStatus: AgentChipAxisStatus | "unchecked";
}

/**
 * The agent tile's status-chip tone-law wrapper (design-a §8's S3,
 * synthesis §"Agent tile": "approved-without-outcome demotes to neutral").
 * `statusClass()` (deleted from `AgentProposalsPanel.tsx`, ticket W8) is
 * replaced by this + `.ag-chip[data-tone]` + `agentProposalStatusTone`
 * (badgeTone.ts) for the concrete color, mirroring exactly how
 * `laneRecencyChipTone` above composes with `readinessChipTone`.
 *
 * `active: true` always — unlike the lane-recency chips (Axis 3 gates on
 * "is anyone actively probing THIS provider"), a proposal's own status is
 * always a claim about a specific card, not about whether some OTHER thing
 * is in use, so Axis 3's non-active short-circuit never applies here and
 * this wrapper always renders a chip.
 */
export function agentOutcomeChipTone(
  input: AgentOutcomeChipInput,
): AgentOutcomeChipResult {
  const axisStatus: AgentChipAxisStatus =
    input.status === "approved" ? "ready" : input.status;
  const axis = readinessChipTone<AgentChipAxisStatus>(
    {
      status: axisStatus,
      active: true,
      // Only the "ready" arm of readinessAxisTone ever consults
      // `stale`/`automaticProbeAvailable` — an unevidenced approval IS
      // exactly that: a "ready" this app cannot verify actually happened.
      stale: axisStatus === "ready" && !input.hasOutcome,
      automaticProbeAvailable: true,
    },
    agentProposalStatusTone,
  );
  return { tone: axis.tone, effectiveStatus: axis.effectiveStatus };
}
