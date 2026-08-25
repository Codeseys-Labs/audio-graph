/**
 * `createChatStreamCoalescer` — the burst-coalescing mechanism `sendChatMessage`
 * (`store/index.ts`, audio-graph-1534) pioneered for its per-invocation
 * `tauri::ipc::Channel<ChatStreamEvent>`, extracted so the audio-graph-83cc T4
 * answer-card actions (`askQuestion`/`answerQuestionCard`) get the identical
 * behavior without re-deriving it: `onmessage` is wired before the starting
 * `invoke` resolves (so no frame between spawn and handler-registration can be
 * lost); `delta` frames coalesce into the store at most once per
 * `CHAT_STREAM_DELTA_THROTTLE_MS`; a `done` frame that arrives before the
 * request id is "armed" (the starting invoke hasn't resolved yet) is held and
 * applied the moment `arm()` runs.
 *
 * `sendChatMessage` itself is deliberately left as its own inline
 * implementation rather than retrofitted onto this factory: it is
 * timing-sensitive, heavily covered by existing tests
 * (`store/index.test.ts`'s "Streaming chat" suite), and outside this ticket's
 * stated scope. New `Channel<ChatStreamEvent>` callers should prefer this
 * factory over hand-rolling the coalescer again.
 */
import { Channel } from "@tauri-apps/api/core";
import type { ChatStreamEvent, ChatTokenDoneEvent } from "../types";

/** Mirrors `store/index.ts`'s `CHAT_DELTA_THROTTLE_MS` (~30fps, below the
 * human flicker threshold, above the observed provider burst rate). Exported
 * as its own constant rather than imported from that module to keep this
 * file free of a dependency on `store/index.ts` (this file is a leaf helper
 * `store/index.ts` imports, not the reverse). */
export const CHAT_STREAM_DELTA_THROTTLE_MS = 33;

export interface ChatStreamCoalescerHandlers {
  onDelta: (delta: string, finishReason: string | undefined) => void;
  onDone: (event: ChatTokenDoneEvent) => void;
  /** @default CHAT_STREAM_DELTA_THROTTLE_MS */
  throttleMs?: number;
}

export interface ChatStreamCoalescer {
  /** Pass this as the `channel` invoke arg. `onmessage` is already wired. */
  channel: Channel<ChatStreamEvent>;
  /**
   * Arms `requestId` once the starting `invoke` resolves. Synchronously
   * drains any delta held while un-armed, then applies a `done` frame if one
   * already landed (done-before-resolve ordering) — mirrors
   * `sendChatMessage`'s own arm-then-drain sequencing exactly.
   */
  arm: (requestId: string) => void;
  /**
   * Tears the coalescer down (call from the `catch` of a rejected starting
   * invoke) so a stray late frame can never touch state after the caller
   * abandoned this dispatch.
   */
  disarm: () => void;
}

/**
 * Builds one coalescer + its backing `Channel<ChatStreamEvent>` for a single
 * dispatch. Construct a fresh one per call — like `sendChatMessage`'s local
 * closures, this is not reusable across requests.
 */
export function createChatStreamCoalescer(
  handlers: ChatStreamCoalescerHandlers,
): ChatStreamCoalescer {
  const throttleMs = handlers.throttleMs ?? CHAT_STREAM_DELTA_THROTTLE_MS;
  let requestId: string | null = null;
  let doneEvent: ChatTokenDoneEvent | null = null;
  let pendingDelta = "";
  let latestFinishReason: string | undefined;
  let flushTimer: ReturnType<typeof setTimeout> | null = null;
  // audio-graph-83cc T4 fix-round finding (minor): once `onDone` has fired
  // for this dispatch, this coalescer is terminated for good — a stray
  // `delta` frame arriving after the terminal frame (a misbehaving
  // provider, or a reordered transport) must never reopen/append onto a
  // draft `onDone` already resolved. `disarm()` already gives the SAME
  // guarantee for the "caller abandoned this dispatch" case; this covers
  // the "dispatch completed normally" case the same way.
  let terminated = false;

  const flush = () => {
    flushTimer = null;
    if (requestId === null || pendingDelta.length === 0) return;
    const delta = pendingDelta;
    const finishReason = latestFinishReason;
    pendingDelta = "";
    latestFinishReason = undefined;
    handlers.onDelta(delta, finishReason);
  };
  const scheduleFlush = () => {
    if (requestId === null || flushTimer !== null) return;
    flushTimer = setTimeout(flush, throttleMs);
  };
  const drainNow = () => {
    if (flushTimer !== null) {
      clearTimeout(flushTimer);
      flushTimer = null;
    }
    flush();
  };
  const applyDone = () => {
    if (doneEvent === null) return;
    drainNow();
    terminated = true;
    handlers.onDone(doneEvent);
  };

  const channel = new Channel<ChatStreamEvent>();
  channel.onmessage = (msg) => {
    if (terminated) return;
    if (msg.event === "delta") {
      pendingDelta += msg.data.delta;
      if (msg.data.finish_reason) latestFinishReason = msg.data.finish_reason;
      scheduleFlush();
    } else {
      doneEvent = msg.data;
      if (requestId !== null) applyDone();
    }
  };

  return {
    channel,
    arm: (id) => {
      requestId = id;
      drainNow();
      if (doneEvent !== null) applyDone();
    },
    disarm: () => {
      if (flushTimer !== null) {
        clearTimeout(flushTimer);
        flushTimer = null;
      }
      channel.onmessage = () => {};
    },
  };
}
