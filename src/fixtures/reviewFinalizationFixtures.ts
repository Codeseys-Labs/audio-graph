/**
 * Fixture/mock session data for the Finalizing / Finalization Blocked
 * prototype (seed audio-graph-1d92).
 *
 * This is the "data boundary" fake: it stands in for a future
 * `get_session_finalization_status_cmd` the same way `SessionsBrowser.test.tsx`
 * fakes `list_sessions` and `ProjectionRuntimeStatusPanel.test.tsx` fakes
 * `get_projection_runtime_status_cmd` — by shaping the response an `invoke`
 * mock would return, not by teaching the real backend anything new.
 *
 * Each fixture session id is deliberately prefixed `fx-` so it can never
 * collide with a real backend-issued session id.
 */
import type { SessionMetadata } from "../types";
import type { SessionFinalizationStatus } from "../types/reviewFinalization";

const BASE_MS = 1_700_000_000_000;
const MIN = 60_000;
const HOUR = 60 * MIN;

function session(
  overrides: Partial<SessionMetadata> & { id: string },
): SessionMetadata {
  return {
    title: null,
    created_at: BASE_MS,
    ended_at: BASE_MS + HOUR,
    duration_seconds: 3600,
    status: "complete",
    segment_count: 180,
    speaker_count: 3,
    entity_count: 14,
    transcript_path: "",
    graph_path: "",
    deleted: false,
    deleted_at: null,
    ...overrides,
  };
}

/**
 * A session still Finalizing in the background: no blocker, notes lane has a
 * small pending backlog, graph lane has a much larger one (used to exercise
 * the Q4 per-lane-visibility variants — a lagging, non-gating graph lane).
 */
export const FX_FINALIZING = session({
  id: "fx-finalizing",
  title: "Weekly sync — finalizing",
  created_at: BASE_MS,
  ended_at: BASE_MS + HOUR,
});

/**
 * Finalization Blocked on the dominant ADR-0035 cause: an external,
 * uncertain remote-LLM-route failure. Not auto-retry eligible — any retry
 * needs explicit cost/egress authorization regardless of variant.
 */
export const FX_BLOCKED_EXTERNAL = session({
  id: "fx-blocked-external",
  title: "Vendor call — blocked on retry",
  created_at: BASE_MS + HOUR,
  ended_at: BASE_MS + 2 * HOUR,
});

/**
 * Blocked{UserCancelled} — the special case that stays retryable
 * indefinitely and that Review "does not nag about" (ADR-0036, verbatim).
 */
export const FX_BLOCKED_USER_CANCELLED = session({
  id: "fx-blocked-user-cancelled",
  title: "1:1 — paused by you",
  created_at: BASE_MS + 2 * HOUR,
  ended_at: BASE_MS + 3 * HOUR,
});

/**
 * Blocked on a class ADR-0036 lets auto-retry for free (`never_dispatched`):
 * the ledger already shows a later, qualifying success, so re-deriving this
 * fixture on render already reads as healed — zero cost, zero egress, zero
 * clicks.
 */
export const FX_BLOCKED_AUTOHEALED = session({
  id: "fx-blocked-autohealed",
  title: "Standup — auto-cleared",
  created_at: BASE_MS + 3 * HOUR,
  ended_at: BASE_MS + 4 * HOUR,
});

/**
 * Finalized: notes lane covered. Graph lane is deliberately still behind —
 * proof that graph coverage never gates Finalized.
 */
export const FX_FINALIZED = session({
  id: "fx-finalized",
  title: "Design review — finalized",
  created_at: BASE_MS + 4 * HOUR,
  ended_at: BASE_MS + 5 * HOUR,
});

export const FIXTURE_SESSIONS: SessionMetadata[] = [
  FX_FINALIZING,
  FX_BLOCKED_EXTERNAL,
  FX_BLOCKED_USER_CANCELLED,
  FX_BLOCKED_AUTOHEALED,
  FX_FINALIZED,
];

export const FIXTURE_FINALIZATION_STATUSES: Record<
  string,
  SessionFinalizationStatus
> = {
  [FX_FINALIZING.id]: {
    session_id: FX_FINALIZING.id,
    lane_coverage: [
      {
        lane: "notes",
        required: true,
        covered: false,
        pending_span_count: 3,
        oldest_pending_since_ms: BASE_MS + HOUR + 2 * MIN,
      },
      {
        lane: "graph",
        required: false,
        covered: false,
        pending_span_count: 41,
        oldest_pending_since_ms: BASE_MS + HOUR + 1 * MIN,
      },
    ],
    remote_attempt_ledger: [
      {
        id: "att-1",
        lane: "notes",
        attempted_at_ms: BASE_MS + HOUR + 3 * MIN,
        outcome: "success",
        dispatched: true,
        cost_incurred: true,
      },
      {
        id: "att-2",
        lane: "graph",
        attempted_at_ms: BASE_MS + HOUR + 4 * MIN,
        outcome: "success",
        dispatched: true,
        cost_incurred: true,
      },
    ],
    blocked_record: null,
    knowledge_gaps: [
      {
        id: "gap-1",
        kind: "unmet_evidence_obligation",
        summary: "Q3 budget figure has no cited source span yet.",
        related_note_id: "note-budget",
      },
    ],
    transcript_confirmation: {
      confirmed_count: 176,
      interim_count: 4,
      lines: [
        {
          id: "line-177",
          text: "…so the budget lands around 40k.",
          confirmed: false,
        },
        {
          id: "line-178",
          text: "and we ship the draft Friday.",
          confirmed: false,
        },
        {
          id: "line-179",
          text: "sounds good, I'll follow up.",
          confirmed: false,
        },
        { id: "line-180", text: "great, talk next week.", confirmed: false },
      ],
    },
  },

  [FX_BLOCKED_EXTERNAL.id]: {
    session_id: FX_BLOCKED_EXTERNAL.id,
    lane_coverage: [
      {
        lane: "notes",
        required: true,
        covered: false,
        pending_span_count: 12,
        oldest_pending_since_ms: BASE_MS + 2 * HOUR + 5 * MIN,
      },
      {
        lane: "graph",
        required: false,
        covered: false,
        pending_span_count: 12,
        oldest_pending_since_ms: BASE_MS + 2 * HOUR + 5 * MIN,
      },
    ],
    remote_attempt_ledger: [
      {
        id: "att-1",
        lane: "notes",
        attempted_at_ms: BASE_MS + 2 * HOUR + 6 * MIN,
        outcome: "rate_limited",
        dispatched: true,
        cost_incurred: false,
      },
      {
        id: "att-2",
        lane: "notes",
        attempted_at_ms: BASE_MS + 2 * HOUR + 20 * MIN,
        outcome: "timeout",
        dispatched: true,
        cost_incurred: true,
      },
    ],
    blocked_record: {
      class: "external_uncertain",
      summary: "The remote LLM route is rate-limited.",
      detail:
        "Three attempts to reach the configured notes-lane provider have failed (rate-limited, then timed out). This is outside AudioGraph — retrying will contact the provider again and may incur cost.",
      since_ms: BASE_MS + 2 * HOUR + 20 * MIN,
      retry_requested_at_ms: null,
    },
    knowledge_gaps: [],
    transcript_confirmation: {
      confirmed_count: 90,
      interim_count: 12,
      lines: [
        {
          id: "line-91",
          text: "let's revisit the vendor terms.",
          confirmed: false,
        },
        {
          id: "line-92",
          text: "I'll send the redline tonight.",
          confirmed: false,
        },
      ],
    },
  },

  [FX_BLOCKED_USER_CANCELLED.id]: {
    session_id: FX_BLOCKED_USER_CANCELLED.id,
    lane_coverage: [
      {
        lane: "notes",
        required: true,
        covered: false,
        pending_span_count: 2,
        oldest_pending_since_ms: BASE_MS + 3 * HOUR + 4 * MIN,
      },
      {
        lane: "graph",
        required: false,
        covered: true,
        pending_span_count: 0,
        oldest_pending_since_ms: null,
      },
    ],
    remote_attempt_ledger: [],
    blocked_record: {
      class: "user_cancelled",
      summary: "You paused finalization for this session.",
      detail:
        "Finalization stays paused until you resume it. Nothing is retried in the background, and nothing is owed here — resume whenever you're ready.",
      since_ms: BASE_MS + 3 * HOUR + 4 * MIN,
      retry_requested_at_ms: null,
    },
    knowledge_gaps: [],
    transcript_confirmation: {
      confirmed_count: 40,
      interim_count: 2,
      lines: [
        {
          id: "line-41",
          text: "let's pick this back up later.",
          confirmed: false,
        },
      ],
    },
  },

  [FX_BLOCKED_AUTOHEALED.id]: {
    session_id: FX_BLOCKED_AUTOHEALED.id,
    lane_coverage: [
      {
        lane: "notes",
        required: true,
        covered: false,
        pending_span_count: 1,
        oldest_pending_since_ms: BASE_MS + 4 * HOUR + 2 * MIN,
      },
      {
        lane: "graph",
        required: false,
        covered: false,
        pending_span_count: 6,
        oldest_pending_since_ms: BASE_MS + 4 * HOUR + 2 * MIN,
      },
    ],
    remote_attempt_ledger: [
      {
        id: "att-1",
        lane: "notes",
        attempted_at_ms: BASE_MS + 4 * HOUR + 6 * MIN,
        outcome: "success",
        dispatched: true,
        cost_incurred: false,
      },
    ],
    blocked_record: {
      class: "never_dispatched",
      summary: "A notes-lane job never reached the dispatch queue.",
      detail:
        "AudioGraph detected the job was never sent — provably free of cost or egress, so it re-tried on its own once the queue recovered.",
      since_ms: BASE_MS + 4 * HOUR + 1 * MIN,
      retry_requested_at_ms: null,
    },
    knowledge_gaps: [],
    transcript_confirmation: {
      confirmed_count: 60,
      interim_count: 1,
      lines: [
        { id: "line-61", text: "who owns the follow-up?", confirmed: false },
      ],
    },
  },

  [FX_FINALIZED.id]: {
    session_id: FX_FINALIZED.id,
    lane_coverage: [
      {
        lane: "notes",
        required: true,
        covered: true,
        pending_span_count: 0,
        oldest_pending_since_ms: null,
      },
      {
        lane: "graph",
        required: false,
        covered: false,
        pending_span_count: 9,
        oldest_pending_since_ms: BASE_MS + 5 * HOUR + 1 * MIN,
      },
    ],
    remote_attempt_ledger: [
      {
        id: "att-1",
        lane: "notes",
        attempted_at_ms: BASE_MS + 5 * HOUR + 2 * MIN,
        outcome: "success",
        dispatched: true,
        cost_incurred: true,
      },
    ],
    blocked_record: null,
    knowledge_gaps: [
      {
        id: "gap-2",
        kind: "unconfirmed_high_impact_inference",
        summary: '"Ship by the 15th" was inferred, not stated outright.',
        related_note_id: "note-timeline",
      },
    ],
    transcript_confirmation: {
      confirmed_count: 205,
      interim_count: 0,
      lines: [],
    },
  },
};

/** Looks up a fixture status; returns `undefined` for any non-fixture session id. */
export function getFixtureFinalizationStatus(
  sessionId: string,
): SessionFinalizationStatus | undefined {
  return FIXTURE_FINALIZATION_STATUSES[sessionId];
}
