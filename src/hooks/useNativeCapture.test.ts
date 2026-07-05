import { invoke } from "@tauri-apps/api/core";
import type { Event } from "@tauri-apps/api/event";
import { listen } from "@tauri-apps/api/event";
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import { useNativeCapture } from "./useNativeCapture";

type Handler = (event: Event<unknown>) => void;

function evt(name: string): Event<unknown> {
  return { event: name, id: 0, payload: undefined } as Event<unknown>;
}

describe("useNativeCapture", () => {
  const handlers = new Map<string, Handler>();
  const startCapture = vi.fn(async () => {});
  const stopCapture = vi.fn(async () => {});

  beforeEach(() => {
    handlers.clear();
    startCapture.mockClear();
    stopCapture.mockClear();
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue(undefined);

    vi.mocked(listen).mockImplementation(async (eventName, cb) => {
      handlers.set(eventName as string, cb as Handler);
      return () => {};
    });

    useAudioGraphStore.setState({
      isCapturing: false,
      captureStartTime: null,
      startCapture,
      stopCapture,
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
  });

  it("subscribes to the global-shortcut and tray-stop events", async () => {
    renderHook(() => useNativeCapture());
    await waitFor(() => {
      expect(handlers.has("global-shortcut-toggle-capture")).toBe(true);
      expect(handlers.has("tray-stop-capture")).toBe(true);
    });
  });

  it("global shortcut STARTS capture when idle (same store path as UI)", async () => {
    renderHook(() => useNativeCapture());
    await waitFor(() =>
      expect(handlers.has("global-shortcut-toggle-capture")).toBe(true),
    );

    act(() => {
      handlers.get("global-shortcut-toggle-capture")?.(
        evt("global-shortcut-toggle-capture"),
      );
    });

    expect(startCapture).toHaveBeenCalledTimes(1);
    expect(stopCapture).not.toHaveBeenCalled();
  });

  it("global shortcut STOPS capture when already capturing", async () => {
    useAudioGraphStore.setState({
      isCapturing: true,
      captureStartTime: Date.now(),
    });
    renderHook(() => useNativeCapture());
    await waitFor(() =>
      expect(handlers.has("global-shortcut-toggle-capture")).toBe(true),
    );

    act(() => {
      handlers.get("global-shortcut-toggle-capture")?.(
        evt("global-shortcut-toggle-capture"),
      );
    });

    expect(stopCapture).toHaveBeenCalledTimes(1);
    expect(startCapture).not.toHaveBeenCalled();
  });

  it("tray Stop routes through the store stopCapture", async () => {
    useAudioGraphStore.setState({
      isCapturing: true,
      captureStartTime: Date.now(),
    });
    renderHook(() => useNativeCapture());
    await waitFor(() => expect(handlers.has("tray-stop-capture")).toBe(true));

    act(() => {
      handlers.get("tray-stop-capture")?.(evt("tray-stop-capture"));
    });

    expect(stopCapture).toHaveBeenCalledTimes(1);
  });

  // safeInvoke forwards a third `options` arg (undefined) to `invoke`, so we
  // assert against the recorded call's first two positional args directly
  // rather than `toHaveBeenCalledWith` (which is arity-sensitive).
  function trayCalls() {
    return vi
      .mocked(invoke)
      .mock.calls.filter((c) => c[0] === "update_tray_capturing")
      .map((c) => c[1] as { capturing: boolean; elapsedSecs: number | null });
  }

  it("pushes idle tray state on mount (content-free: only a boolean + null secs)", async () => {
    const { unmount } = renderHook(() => useNativeCapture());
    await waitFor(() => {
      expect(trayCalls()).toContainEqual({
        capturing: false,
        elapsedSecs: null,
      });
    });
    unmount();
  });

  it("syncs a content-free elapsed-seconds count while capturing", async () => {
    useAudioGraphStore.setState({
      isCapturing: true,
      captureStartTime: Date.now(),
    });
    const { unmount } = renderHook(() => useNativeCapture());

    // Immediate push on transition carries capturing=true + a numeric (never
    // content) elapsed count.
    await waitFor(() => {
      const capturingCall = trayCalls().find((p) => p.capturing);
      expect(capturingCall).toBeDefined();
      expect(typeof capturingCall?.elapsedSecs).toBe("number");
    });

    // The payload only ever contains the two allowed keys — no content field
    // can carry transcript/note/title text.
    for (const payload of trayCalls()) {
      expect(Object.keys(payload).sort()).toEqual(["capturing", "elapsedSecs"]);
    }

    // Stop the 1s tooltip-refresh interval so it doesn't leak past the test.
    unmount();
  });
});
