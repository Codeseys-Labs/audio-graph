import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useAudioGraphStore } from "../store";
import type {
  TranscriptSegment,
  TurnEventKind,
  TurnLifecycleEvent,
} from "../types";

/**
 * ADR-0013 step 2 — converse pipelined "front leg".
 *
 * When the user is in converse mode with the pipelined engine
 * (`conversationMode === "converse" && converseEngine === "pipelined"`),
 * finalized speech transcripts should be fed into the graph-grounded streaming
 * chat (`sendChatMessage` → `start_streaming_chat`), which already grounds the
 * reply in the knowledge graph and drives speak-aloud TTS backend-side. The
 * LLM→TTS "back leg" already works; this hook supplies the missing
 * speech→chat "front leg".
 *
 * `transcript-update` events arrive per ~2s finalized segment, not per
 * conversational turn, so we aggregate consecutive segments into an
 * *endpointed turn* and only send once the turn ends. A turn is flushed when
 * either:
 *   1. an explicit endpoint `turn-event` arrives (Deepgram/AssemblyAI: an
 *      `end_of_turn` / `utterance_end` / `speech_final` kind), or
 *   2. a silence timeout elapses with no new finalized segment (works for
 *      providers that don't emit turn lifecycle events).
 *
 * Guards: never sends while a chat stream is already in flight (so two turns
 * don't interleave) — the pending turn is held and retried when the stream
 * completes. Entity extraction continues backend-side regardless of mode, so
 * the conversation keeps enriching the graph while the user talks to it.
 */

/** Silence (ms) with no new finalized segment that ends a turn. */
export const TURN_SILENCE_MS = 2500;
/** Backoff (ms) before retrying a flush that was blocked by an in-flight stream. */
export const BUSY_RETRY_MS = 600;

/** Turn-event kinds that mark the end of a speaker's turn. */
const ENDPOINT_KINDS: ReadonlySet<TurnEventKind> = new Set<TurnEventKind>([
  "end_of_turn",
  "utterance_end",
  "speech_final",
]);

export function isTurnEndpoint(kind: TurnEventKind): boolean {
  return ENDPOINT_KINDS.has(kind);
}

/**
 * Normalize a list of finalized segment texts into a single turn string:
 * trims each, drops empties, collapses internal whitespace, and joins with a
 * single space. Pure + side-effect free so it can be unit-tested directly.
 */
export function buildTurnText(segments: readonly string[]): string {
  return segments
    .map((s) => s.replace(/\s+/g, " ").trim())
    .filter((s) => s.length > 0)
    .join(" ")
    .trim();
}

export function useConverseFrontLeg(): void {
  const conversationMode = useAudioGraphStore((s) => s.conversationMode);
  const converseEngine = useAudioGraphStore((s) => s.converseEngine);

  useEffect(() => {
    // Only run (and only subscribe) when the pipelined converse front-leg is
    // the active mode. Switching modes tears down and rebuilds the listeners.
    if (conversationMode !== "converse" || converseEngine !== "pipelined") {
      return;
    }

    let cancelled = false;
    let unlisten: Array<(() => void) | null> = [];
    let buffer: string[] = [];
    let timer: ReturnType<typeof setTimeout> | null = null;

    const clearTimer = () => {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
    };

    const flush = () => {
      clearTimer();
      const turn = buildTurnText(buffer);
      if (turn.length === 0) {
        buffer = [];
        return;
      }
      const store = useAudioGraphStore.getState();
      // Don't interleave turns: if a stream is still draining, hold this turn
      // (keep it buffered as a single normalized entry) and retry shortly. The
      // guard depends on `sendChatMessage` setting `isChatLoading` *synchronously*
      // before it awaits start_streaming_chat (it does), so the window before
      // streamingChatRequestId is assigned is still covered — a future refactor
      // making that flag async would silently reintroduce double-sends.
      if (store.streamingChatRequestId !== null || store.isChatLoading) {
        buffer = [turn];
        timer = setTimeout(flush, BUSY_RETRY_MS);
        return;
      }
      buffer = [];
      void store.sendChatMessage(turn);
    };

    const onSegmentText = (text: string) => {
      // Half-duplex / echo guard: while a converse reply is streaming (and being
      // spoken via TTS), ignore incoming transcripts. With loopback/system-audio
      // capture the assistant's own spoken reply would otherwise be transcribed
      // and fed back as a new turn — a self-sustaining echo loop. This coarse gate
      // also drops user barge-in during a reply; true full-duplex needs AEC /
      // pipeline-side self-capture suppression (tracked follow-up, ADR-0013 §3).
      if (useAudioGraphStore.getState().isChatLoading) return;
      const t = text.trim();
      if (t.length === 0) return;
      buffer.push(t);
      // (Re)arm the silence timeout; an explicit endpoint event flushes sooner.
      clearTimer();
      timer = setTimeout(flush, TURN_SILENCE_MS);
    };

    async function safeListen<T>(
      eventName: string,
      cb: (payload: T) => void,
    ): Promise<(() => void) | null> {
      try {
        return await listen<T>(eventName, (event) => cb(event.payload));
      } catch (err) {
        console.error(
          `useConverseFrontLeg: failed to subscribe to ${eventName}:`,
          err,
        );
        return null;
      }
    }

    async function setup() {
      const handles = await Promise.all([
        safeListen<TranscriptSegment>("transcript-update", (seg) =>
          onSegmentText(seg.text),
        ),
        safeListen<TurnLifecycleEvent>("turn-event", (evt) => {
          if (isTurnEndpoint(evt.kind)) flush();
        }),
      ]);
      if (cancelled) {
        for (const fn of handles) if (fn) fn();
        return;
      }
      unlisten = handles;
    }

    void setup();

    return () => {
      cancelled = true;
      clearTimer();
      buffer = [];
      for (const fn of unlisten) if (fn) fn();
    };
  }, [conversationMode, converseEngine]);
}
