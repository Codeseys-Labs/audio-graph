import type { Event } from "@tauri-apps/api/event";
import { listen } from "@tauri-apps/api/event";
import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import {
  BUSY_RETRY_MS,
  buildTurnText,
  isTurnEndpoint,
  STREAM_WATCHDOG_MS,
  TURN_SILENCE_MS,
  useConverseFrontLeg,
} from "./useConverseFrontLeg";

type Handler = (event: Event<unknown>) => void;

function makeEvent<T>(name: string, payload: T): Event<T> {
  return { event: name, id: 0, payload } as Event<T>;
}

describe("buildTurnText", () => {
  it("trims, drops empties, collapses whitespace, and joins with one space", () => {
    expect(buildTurnText(["  hello ", "", "  world\n", "   "])).toBe(
      "hello world",
    );
  });
  it("collapses internal runs of whitespace", () => {
    expect(buildTurnText(["a\t\tb", "c   d"])).toBe("a b c d");
  });
  it("returns empty string for all-empty input", () => {
    expect(buildTurnText(["", "   ", "\n"])).toBe("");
  });
});

describe("isTurnEndpoint", () => {
  it("treats end_of_turn / utterance_end / speech_final as endpoints", () => {
    expect(isTurnEndpoint("end_of_turn")).toBe(true);
    expect(isTurnEndpoint("utterance_end")).toBe(true);
    expect(isTurnEndpoint("speech_final")).toBe(true);
  });
  it("does not treat speech_started / eager_end_of_turn / local_window as endpoints", () => {
    expect(isTurnEndpoint("speech_started")).toBe(false);
    expect(isTurnEndpoint("eager_end_of_turn")).toBe(false);
    expect(isTurnEndpoint("local_window")).toBe(false);
  });
});

describe("useConverseFrontLeg", () => {
  const handlers = new Map<string, Handler>();

  function fire<T>(name: string, payload: T) {
    handlers.get(name)?.(makeEvent(name, payload));
  }

  function segment(text: string) {
    return {
      id: "seg",
      source_id: "src",
      speaker_id: null,
      speaker_label: null,
      text,
      start_time: 0,
      end_time: 1,
      confidence: 0.9,
    };
  }

  beforeEach(() => {
    vi.useFakeTimers();
    handlers.clear();
    vi.mocked(listen).mockImplementation((async (
      eventName: string,
      cb: Handler,
    ) => {
      handlers.set(eventName, cb);
      return () => {};
    }) as typeof listen);
    useAudioGraphStore.setState({
      conversationMode: "converse",
      converseEngine: "pipelined",
      isChatLoading: false,
      streamingChatRequestId: null,
      sendChatMessage: vi.fn(async () => {}),
      notify: vi.fn(() => "ntf-test"),
    } as never);
  });

  afterEach(() => {
    vi.runOnlyPendingTimers();
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  async function mounted() {
    const view = renderHook(() => useConverseFrontLeg());
    // Flush the effect's async listen() registration (fake timers + waitFor
    // deadlock, so flush microtasks inside act instead).
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(handlers.has("transcript-update")).toBe(true);
    return view;
  }

  it("does not subscribe when not in converse/pipelined mode", () => {
    useAudioGraphStore.setState({ conversationMode: "notes" } as never);
    renderHook(() => useConverseFrontLeg());
    expect(handlers.has("transcript-update")).toBe(false);
  });

  it("aggregates finalized segments and sends one turn after the silence timeout", async () => {
    await mounted();
    fire("transcript-update", segment("hello"));
    fire("transcript-update", segment("there"));
    expect(
      useAudioGraphStore.getState().sendChatMessage,
    ).not.toHaveBeenCalled();

    vi.advanceTimersByTime(TURN_SILENCE_MS);
    expect(useAudioGraphStore.getState().sendChatMessage).toHaveBeenCalledTimes(
      1,
    );
    expect(useAudioGraphStore.getState().sendChatMessage).toHaveBeenCalledWith(
      "hello there",
    );
  });

  it("flushes immediately on an endpoint turn-event", async () => {
    await mounted();
    fire("transcript-update", segment("quick question"));
    fire("turn-event", {
      provider: "deepgram",
      source_id: "src",
      kind: "end_of_turn",
      timestamp_ms: 0,
    });
    expect(useAudioGraphStore.getState().sendChatMessage).toHaveBeenCalledWith(
      "quick question",
    );
  });

  it("ignores non-endpoint turn-events", async () => {
    await mounted();
    fire("transcript-update", segment("still talking"));
    fire("turn-event", {
      provider: "deepgram",
      source_id: "src",
      kind: "speech_started",
      timestamp_ms: 0,
    });
    expect(
      useAudioGraphStore.getState().sendChatMessage,
    ).not.toHaveBeenCalled();
  });

  it("does not send while a stream is in flight, then retries when free", async () => {
    useAudioGraphStore.setState({ streamingChatRequestId: "req-1" } as never);
    await mounted();
    fire("transcript-update", segment("held turn"));
    vi.advanceTimersByTime(TURN_SILENCE_MS);
    // Stream busy → not sent yet.
    expect(
      useAudioGraphStore.getState().sendChatMessage,
    ).not.toHaveBeenCalled();

    // Stream finishes; the retry timer fires and the held turn goes out once.
    useAudioGraphStore.setState({
      streamingChatRequestId: null,
      isChatLoading: false,
    } as never);
    vi.advanceTimersByTime(BUSY_RETRY_MS);
    expect(useAudioGraphStore.getState().sendChatMessage).toHaveBeenCalledTimes(
      1,
    );
    expect(useAudioGraphStore.getState().sendChatMessage).toHaveBeenCalledWith(
      "held turn",
    );
  });

  it("never sends an empty turn", async () => {
    await mounted();
    fire("transcript-update", segment("   "));
    vi.advanceTimersByTime(TURN_SILENCE_MS);
    expect(
      useAudioGraphStore.getState().sendChatMessage,
    ).not.toHaveBeenCalled();
  });

  it("ignores transcripts while a reply is streaming (echo guard)", async () => {
    await mounted();
    // A converse reply is streaming + being spoken; loopback-captured TTS must
    // not be aggregated into a new turn.
    useAudioGraphStore.setState({ isChatLoading: true } as never);
    fire("transcript-update", segment("this is the assistant's own voice"));
    vi.advanceTimersByTime(TURN_SILENCE_MS);
    expect(
      useAudioGraphStore.getState().sendChatMessage,
    ).not.toHaveBeenCalled();
  });

  it("resets the wedged streaming state + notifies when the Done is lost (FINDING #56 P3)", async () => {
    await mounted();
    // A stream goes in-flight (the store subscription arms the watchdog) and
    // the terminal chat-token-done never arrives.
    act(() => {
      useAudioGraphStore.setState({
        isChatLoading: true,
        streamingChatRequestId: "req-wedged",
      } as never);
    });

    // Before the watchdog fires the state is still stuck.
    vi.advanceTimersByTime(STREAM_WATCHDOG_MS - 1);
    expect(useAudioGraphStore.getState().isChatLoading).toBe(true);

    // Watchdog trips: streaming state is reset so converse can recover and a
    // warning notification is surfaced.
    act(() => {
      vi.advanceTimersByTime(1);
    });
    const s = useAudioGraphStore.getState();
    expect(s.isChatLoading).toBe(false);
    expect(s.streamingChatRequestId).toBeNull();
    expect(s.notify).toHaveBeenCalledWith(
      expect.objectContaining({ severity: "warning" }),
    );
  });

  it("does NOT trip the watchdog when the stream completes normally (FINDING #56 P3)", async () => {
    await mounted();
    act(() => {
      useAudioGraphStore.setState({
        isChatLoading: true,
        streamingChatRequestId: "req-ok",
      } as never);
    });
    // Stream finishes well before the watchdog window.
    act(() => {
      vi.advanceTimersByTime(STREAM_WATCHDOG_MS / 2);
      useAudioGraphStore.setState({
        isChatLoading: false,
        streamingChatRequestId: null,
      } as never);
    });
    // Now run past the original deadline — the watchdog must have been
    // disarmed by the clear, so notify is never called.
    act(() => {
      vi.advanceTimersByTime(STREAM_WATCHDOG_MS);
    });
    expect(useAudioGraphStore.getState().notify).not.toHaveBeenCalled();
  });

  it("re-arms the watchdog on a new request id (progress restarts the clock)", async () => {
    await mounted();
    act(() => {
      useAudioGraphStore.setState({
        isChatLoading: true,
        streamingChatRequestId: "req-A",
      } as never);
    });
    // Most of the way through the first window, a NEW turn starts streaming.
    act(() => {
      vi.advanceTimersByTime(STREAM_WATCHDOG_MS - 5);
      useAudioGraphStore.setState({
        isChatLoading: true,
        streamingChatRequestId: "req-B",
      } as never);
    });
    // The original deadline passes — but the clock restarted on req-B, so no
    // trip yet.
    act(() => {
      vi.advanceTimersByTime(10);
    });
    expect(useAudioGraphStore.getState().notify).not.toHaveBeenCalled();
    // The full window from req-B elapses → now it trips.
    act(() => {
      vi.advanceTimersByTime(STREAM_WATCHDOG_MS);
    });
    expect(useAudioGraphStore.getState().notify).toHaveBeenCalledWith(
      expect.objectContaining({ severity: "warning" }),
    );
  });
});
