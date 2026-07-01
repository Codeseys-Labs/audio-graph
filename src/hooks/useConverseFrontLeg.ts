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
/**
 * Watchdog timeout (ms). If a chat stream stays "in flight"
 * (`isChatLoading` true / `streamingChatRequestId` set) this long with no
 * progress, we assume the terminal `chat-token-done` event was lost (IPC
 * drop, backend crash mid-stream). Without this, the busy-guard in `flush`
 * holds every subsequent turn forever and the front leg silently drops all
 * incoming transcripts (FINDING #56 P3). On trip we reset the streaming
 * state so converse can recover, and surface a notify.
 */
export const STREAM_WATCHDOG_MS = 30_000;

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

    // ── Lost-Done watchdog (FINDING #56 P3) ────────────────────────────
    // Arm a timer whenever a chat stream goes in-flight; disarm it when the
    // stream clears (the normal chat-token-done path) OR re-arm it when a
    // *new* stream starts (request id changed = progress). If it ever trips,
    // the terminal Done was lost and the front leg would otherwise wedge:
    // reset the streaming state so converse recovers and notify the user.
    let watchdog: ReturnType<typeof setTimeout> | null = null;
    let watchedId: string | null = null;
    const clearWatchdog = () => {
      if (watchdog !== null) {
        clearTimeout(watchdog);
        watchdog = null;
      }
    };
    const tripWatchdog = () => {
      watchdog = null;
      const store = useAudioGraphStore.getState();
      // Only reset if still wedged (a Done could have landed between the
      // timer firing and this callback running on a busy loop).
      if (!store.isChatLoading && store.streamingChatRequestId === null) return;
      useAudioGraphStore.setState({
        isChatLoading: false,
        streamingChatRequestId: null,
      });
      store.notify({
        severity: "warning",
        message:
          "Converse stalled waiting for the assistant reply; resetting so you can continue.",
      });
    };
    const syncWatchdog = (
      isChatLoading: boolean,
      streamingChatRequestId: string | null,
    ) => {
      const inFlight = isChatLoading || streamingChatRequestId !== null;
      if (!inFlight) {
        watchedId = null;
        clearWatchdog();
        return;
      }
      // (Re)arm on a fresh in-flight state or when the request id advances
      // (a new turn streaming = progress, so the clock restarts). A stable
      // id with the timer already running means no progress — leave it.
      if (watchdog === null || streamingChatRequestId !== watchedId) {
        watchedId = streamingChatRequestId;
        clearWatchdog();
        watchdog = setTimeout(tripWatchdog, STREAM_WATCHDOG_MS);
      }
    };
    const unsubscribeWatchdog = useAudioGraphStore.subscribe((state) =>
      syncWatchdog(state.isChatLoading, state.streamingChatRequestId),
    );
    // Seed from current state in case a stream was already in flight at mount.
    {
      const s0 = useAudioGraphStore.getState();
      syncWatchdog(s0.isChatLoading, s0.streamingChatRequestId);
    }

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
      clearWatchdog();
      unsubscribeWatchdog();
      buffer = [];
      for (const fn of unlisten) if (fn) fn();
    };
  }, [conversationMode, converseEngine]);
}
