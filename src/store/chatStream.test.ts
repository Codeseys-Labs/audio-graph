import { describe, expect, it, vi } from "vitest";
import { createChatStreamCoalescer } from "./chatStream";

/**
 * Unit tests for the extracted coalescer (audio-graph-83cc T4). Drives
 * `channel.onmessage` directly with the same discriminated `{event, data}`
 * frames the Rust `channel.send()` end emits — the same technique
 * `store/index.test.ts`'s "Streaming chat" suite uses for `sendChatMessage`,
 * since this factory is the extracted form of that exact mechanism.
 */

type ChannelLike = { onmessage: ((m: unknown) => void) | null };

describe("createChatStreamCoalescer", () => {
  it("wires onmessage synchronously, before arm() is ever called", () => {
    const coalescer = createChatStreamCoalescer({
      onDelta: () => {},
      onDone: () => {},
    });
    expect(coalescer.channel.onmessage).toBeTypeOf("function");
  });

  it("coalesces bursty deltas into at most one onDelta call per throttle window", () => {
    vi.useFakeTimers();
    try {
      const onDelta = vi.fn();
      const coalescer = createChatStreamCoalescer({
        onDelta,
        onDone: () => {},
        throttleMs: 10,
      });
      coalescer.arm("req-1");
      const channel = coalescer.channel as unknown as ChannelLike;

      channel.onmessage?.({
        event: "delta",
        data: { request_id: "req-1", delta: "a" },
      });
      channel.onmessage?.({
        event: "delta",
        data: { request_id: "req-1", delta: "b" },
      });
      channel.onmessage?.({
        event: "delta",
        data: { request_id: "req-1", delta: "c" },
      });
      expect(onDelta).not.toHaveBeenCalled();

      vi.advanceTimersByTime(15);
      expect(onDelta).toHaveBeenCalledTimes(1);
      expect(onDelta).toHaveBeenCalledWith("abc", undefined);
    } finally {
      vi.useRealTimers();
    }
  });

  it("holds delta and done frames that arrive before arm(), then applies them in order once armed", () => {
    const onDelta = vi.fn();
    const onDone = vi.fn();
    const coalescer = createChatStreamCoalescer({ onDelta, onDone });
    const channel = coalescer.channel as unknown as ChannelLike;

    channel.onmessage?.({
      event: "delta",
      data: { request_id: "req-1", delta: "lead " },
    });
    channel.onmessage?.({
      event: "done",
      data: {
        request_id: "req-1",
        full_text: "lead final",
        finish_reason: "stop",
      },
    });
    expect(onDelta).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();

    coalescer.arm("req-1");
    // Drain order: the held delta flushes first, then the terminal frame.
    expect(onDelta).toHaveBeenCalledWith("lead ", undefined);
    expect(onDone).toHaveBeenCalledWith({
      request_id: "req-1",
      full_text: "lead final",
      finish_reason: "stop",
    });
  });

  it("a done frame drains any pending un-flushed delta synchronously before firing onDone", () => {
    vi.useFakeTimers();
    try {
      const onDelta = vi.fn();
      const onDone = vi.fn();
      const coalescer = createChatStreamCoalescer({ onDelta, onDone });
      coalescer.arm("req-1");
      const channel = coalescer.channel as unknown as ChannelLike;

      channel.onmessage?.({
        event: "delta",
        data: { request_id: "req-1", delta: "queued" },
      });
      // No timer has fired yet — the delta is still pending.
      expect(onDelta).not.toHaveBeenCalled();

      channel.onmessage?.({
        event: "done",
        data: {
          request_id: "req-1",
          full_text: "queued done",
          finish_reason: "stop",
        },
      });
      expect(onDelta).toHaveBeenCalledWith("queued", undefined);
      expect(onDone).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("disarm() tears down onmessage so a stray late frame never reaches the handlers", () => {
    const onDelta = vi.fn();
    const onDone = vi.fn();
    const coalescer = createChatStreamCoalescer({ onDelta, onDone });
    coalescer.arm("req-1");
    coalescer.disarm();
    const channel = coalescer.channel as unknown as ChannelLike;

    channel.onmessage?.({
      event: "delta",
      data: { request_id: "req-1", delta: "late" },
    });
    channel.onmessage?.({
      event: "done",
      data: { request_id: "req-1", full_text: "late", finish_reason: "stop" },
    });

    expect(onDelta).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
  });

  it("audio-graph-83cc T4 fix-round (minor): a stray delta frame arriving AFTER onDone has already fired is ignored — the coalescer is terminated for good, not just disarmed by the caller", () => {
    vi.useFakeTimers();
    try {
      const onDelta = vi.fn();
      const onDone = vi.fn();
      const coalescer = createChatStreamCoalescer({
        onDelta,
        onDone,
        throttleMs: 10,
      });
      coalescer.arm("req-1");
      const channel = coalescer.channel as unknown as ChannelLike;

      channel.onmessage?.({
        event: "done",
        data: {
          request_id: "req-1",
          full_text: "final",
          finish_reason: "stop",
        },
      });
      expect(onDone).toHaveBeenCalledTimes(1);

      // A misbehaving provider / reordered transport sends a delta AFTER the
      // terminal frame — must not append onto (or reopen) a resolved draft.
      // Without the terminated-latch, `scheduleFlush()` would still queue a
      // real timer here, so this only proves the guard once the timer is
      // actually given a chance to fire.
      channel.onmessage?.({
        event: "delta",
        data: { request_id: "req-1", delta: "late" },
      });
      vi.advanceTimersByTime(20);
      expect(onDelta).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("carries the latest finish_reason seen across coalesced delta frames", () => {
    vi.useFakeTimers();
    try {
      const onDelta = vi.fn();
      const coalescer = createChatStreamCoalescer({
        onDelta,
        onDone: () => {},
        throttleMs: 10,
      });
      coalescer.arm("req-1");
      const channel = coalescer.channel as unknown as ChannelLike;

      channel.onmessage?.({
        event: "delta",
        data: { request_id: "req-1", delta: "a", finish_reason: "length" },
      });
      vi.advanceTimersByTime(15);
      expect(onDelta).toHaveBeenCalledWith("a", "length");
    } finally {
      vi.useRealTimers();
    }
  });
});
