/**
 * AnswerDrafts — transient, frontend-only progress state for a live-assist
 * card's in-flight answer stream (audio-graph-83cc, T4).
 *
 * Extracted-slice precedent: `shellNav.ts`'s `createShellNavSlice` (this
 * file's factory/wiring shape is deliberately identical — a plain factory
 * function typed against its own minimal slice interface, spread into the
 * single `create<AudioGraphStore>(...)` call in `store/index.ts`, since this
 * repo has no multi-store slice machinery).
 *
 * Why this exists, and why it is NOT part of `LiveAssistCardRecord`: the
 * durable `CardAnswer` (see `types/index.ts`, mirrors Rust `events.rs`) is
 * written ONCE by the backend on the answer stream's terminal frame and
 * carries trusted provenance (route id, evidence ids) this frontend must
 * never fabricate. Everything BEFORE that terminal frame — the accumulating
 * delta text, or a client-observed stream failure — has nowhere else to
 * live, and the design-a/T4 deletion test names the failure mode exactly:
 * "no progress, no error text — a silent 3-9s dead card." `answerDrafts` is
 * that progress/error surface, keyed by the card's `proposal.id`.
 *
 * Lifecycle: a draft is set to `"streaming"` the moment a dispatch starts
 * (`askQuestion`/`answerQuestionCard`, `store/index.ts`), accumulates delta
 * text via `appendAnswerDraftDelta`, and is CLEARED the moment the stream's
 * terminal frame lands with a non-error `finish_reason` — at that point the
 * durable answer is expected to already be on its way to `card.answer`
 * (today: nothing repopulates it, since the backend command/event T3 owns
 * is unlanded; `AnswerThread`, `AgentProposalsPanel.tsx`, documents this gap
 * explicitly rather than papering over it with a client-synthesized answer).
 * A terminal frame carrying an error, or a synchronously-rejected dispatch,
 * instead sets `status: "failed"` and PRESERVES the draft (never cleared
 * automatically) so `AnswerThread` can render the typed failure + a Retry
 * affordance. `dismissAgentProposal`/`clearAgentProposals` (`store/index.ts`)
 * both clear any draft for the card(s) they dismiss, so a dismissed card
 * never keeps a stale "streaming"/"failed" ghost around.
 *
 * `composerError` is a separate, single slot (not keyed by card id): the
 * composer's `askQuestion` dispatch can fail before any card exists to key a
 * draft against (the mint-and-ask round trip is one call), so its error has
 * nowhere to thread onto a per-card draft — it renders inline in the
 * composer itself instead. Cleared at the start of the next submit attempt.
 *
 * `autoAnswerDispatchCount` (audio-graph-83cc T5, deliverable e) lives here
 * too — not a draft, but the same session-scoped lifecycle: reset in
 * `resetSessionView` alongside `answerDrafts`/`composerError` (see that
 * action's own comment), mutated only from the same `answerQuestionCard`
 * action that owns every other piece of state in this file. See
 * `types/index.ts`'s doc on the field for why it's a counter incremented on
 * successful dispatch rather than something derived from `liveAssistCards`.
 */

/** One card's transient answer-stream progress. `requestId` is `null` only
 * in the brief window between a dispatch starting and its `invoke` resolving
 * (mirrors `sendChatMessage`'s pre-arm window, `store/index.ts`) — deltas
 * arriving before that point are held by the channel coalescer, never lost,
 * and applied once armed. */
export interface AnswerDraftState {
  status: "streaming" | "failed";
  /** Accumulated delta text while `status === "streaming"`; the typed
   * failure message while `status === "failed"`. */
  text: string;
  requestId: string | null;
}

export interface AnswerDraftsSlice {
  answerDrafts: Record<string, AnswerDraftState>;
  composerError: string | null;
  autoAnswerDispatchCount: number;
  setAnswerDraft: (cardId: string, draft: AnswerDraftState) => void;
  /** No-ops (returns the same object reference behind the scenes via a `{}`
   * partial) unless `requestId` matches the draft's currently-armed id —
   * the same staleness guard `appendChatTokenDelta` uses, so a delta from a
   * superseded/cancelled dispatch can never appear to append onto a newer
   * one. */
  appendAnswerDraftDelta: (
    cardId: string,
    requestId: string,
    delta: string,
  ) => void;
  clearAnswerDraft: (cardId: string) => void;
  setComposerError: (message: string | null) => void;
  recordAutoAnswerDispatch: () => void;
}

type AnswerDraftsSet = (
  partial:
    | Partial<AnswerDraftsSlice>
    | ((state: AnswerDraftsSlice) => Partial<AnswerDraftsSlice>),
) => void;
type AnswerDraftsGet = () => AnswerDraftsSlice;

export function createAnswerDraftsSlice(
  set: AnswerDraftsSet,
  _get: AnswerDraftsGet,
): AnswerDraftsSlice {
  return {
    answerDrafts: {},
    composerError: null,
    autoAnswerDispatchCount: 0,
    setAnswerDraft: (cardId, draft) =>
      set((state) => ({
        answerDrafts: { ...state.answerDrafts, [cardId]: draft },
      })),
    appendAnswerDraftDelta: (cardId, requestId, delta) =>
      set((state) => {
        const current = state.answerDrafts[cardId];
        if (!current || current.requestId !== requestId) return {};
        return {
          answerDrafts: {
            ...state.answerDrafts,
            [cardId]: { ...current, text: current.text + delta },
          },
        };
      }),
    clearAnswerDraft: (cardId) =>
      set((state) => {
        if (!(cardId in state.answerDrafts)) return {};
        const next = { ...state.answerDrafts };
        delete next[cardId];
        return { answerDrafts: next };
      }),
    setComposerError: (message) => set({ composerError: message }),
    recordAutoAnswerDispatch: () =>
      set((state) => ({
        autoAnswerDispatchCount: state.autoAnswerDispatchCount + 1,
      })),
  };
}
