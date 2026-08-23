import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createBatchedChangeAnnouncer } from "./docChangeAnnouncer";

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("createBatchedChangeAnnouncer", () => {
  it("3 rapid folds within the window collapse to exactly ONE announcement", () => {
    const onFlush = vi.fn();
    const announcer = createBatchedChangeAnnouncer(onFlush, 2000);

    announcer.push(["a"]);
    vi.advanceTimersByTime(500);
    announcer.push(["b"]);
    vi.advanceTimersByTime(500);
    announcer.push(["c"]);

    // Still inside the (reset) window — nothing has fired yet.
    vi.advanceTimersByTime(1999);
    expect(onFlush).not.toHaveBeenCalled();

    vi.advanceTimersByTime(1);
    expect(onFlush).toHaveBeenCalledTimes(1);
    expect(onFlush).toHaveBeenCalledWith(3);
  });

  it("once a window flushes, the NEXT push starts a fresh window and produces its own separate announcement", () => {
    const onFlush = vi.fn();
    const announcer = createBatchedChangeAnnouncer(onFlush, 2000);

    announcer.push(["a"]);
    vi.advanceTimersByTime(2000);
    expect(onFlush).toHaveBeenCalledTimes(1);
    expect(onFlush).toHaveBeenNthCalledWith(1, 1);

    announcer.push(["b", "c"]);
    vi.advanceTimersByTime(2000);
    expect(onFlush).toHaveBeenCalledTimes(2);
    expect(onFlush).toHaveBeenNthCalledWith(2, 2);
  });

  it("counts DISTINCT ids, not raw push events — the same id pushed twice in one window is one passage, not two", () => {
    const onFlush = vi.fn();
    const announcer = createBatchedChangeAnnouncer(onFlush, 2000);

    announcer.push(["a"]);
    announcer.push(["a"]);
    announcer.push(["a", "b"]);
    vi.advanceTimersByTime(2000);

    expect(onFlush).toHaveBeenCalledTimes(1);
    expect(onFlush).toHaveBeenCalledWith(2);
  });

  it("an empty-array push is a true no-op: it neither starts nor extends a window", () => {
    const onFlush = vi.fn();
    const announcer = createBatchedChangeAnnouncer(onFlush, 2000);

    announcer.push([]);
    vi.advanceTimersByTime(5000);
    expect(onFlush).not.toHaveBeenCalled();

    // Prove the announcer is still live (the empty push above didn't leave it
    // in some broken state).
    announcer.push(["a"]);
    vi.advanceTimersByTime(2000);
    expect(onFlush).toHaveBeenCalledTimes(1);
  });

  it("an empty-array push does NOT reset an already-running window's timer", () => {
    const onFlush = vi.fn();
    const announcer = createBatchedChangeAnnouncer(onFlush, 2000);

    announcer.push(["a"]);
    vi.advanceTimersByTime(1900);
    announcer.push([]); // must not push the deadline back out
    vi.advanceTimersByTime(100);

    expect(onFlush).toHaveBeenCalledTimes(1);
    expect(onFlush).toHaveBeenCalledWith(1);
  });

  it("cancel() discards a pending window without ever flushing it", () => {
    const onFlush = vi.fn();
    const announcer = createBatchedChangeAnnouncer(onFlush, 2000);

    announcer.push(["a"]);
    announcer.cancel();
    vi.advanceTimersByTime(5000);

    expect(onFlush).not.toHaveBeenCalled();
  });

  it("a push AFTER cancel() starts a clean new window (no leaked ids from the cancelled one)", () => {
    const onFlush = vi.fn();
    const announcer = createBatchedChangeAnnouncer(onFlush, 2000);

    announcer.push(["a", "b"]);
    announcer.cancel();
    announcer.push(["c"]);
    vi.advanceTimersByTime(2000);

    expect(onFlush).toHaveBeenCalledTimes(1);
    expect(onFlush).toHaveBeenCalledWith(1);
  });
});
