import { act, fireEvent, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../store";
import { useKeyboardShortcuts } from "./useKeyboardShortcuts";

// The store's openSettings/openSessionsBrowser internally invoke Tauri
// commands to hydrate content; those are mocked to noop via src/test/setup.ts.
// We only care here about the boolean flags and capture toggling.

function resetStore() {
  useAudioGraphStore.setState({
    settingsOpen: false,
    sessionsBrowserOpen: false,
    isCapturing: false,
    selectedSourceIds: ["mic-1"],
    error: null,
  });
}

describe("useKeyboardShortcuts", () => {
  beforeEach(() => {
    resetStore();
  });

  it("Cmd+R toggles capture on when not capturing", () => {
    const startCaptureAndTranscribe = vi.fn();
    useAudioGraphStore.setState({
      startCaptureAndTranscribe,
      isCapturing: false,
    });

    renderHook(() => useKeyboardShortcuts());

    act(() => {
      fireEvent.keyDown(window, { key: "r", metaKey: true });
    });

    expect(startCaptureAndTranscribe).toHaveBeenCalledTimes(1);
  });

  it("Ctrl+R toggles capture off when currently capturing", () => {
    const stopCapture = vi.fn();
    useAudioGraphStore.setState({ stopCapture, isCapturing: true });

    renderHook(() => useKeyboardShortcuts());

    act(() => {
      fireEvent.keyDown(window, { key: "R", ctrlKey: true });
    });

    expect(stopCapture).toHaveBeenCalledTimes(1);
  });

  it("does NOT fire Cmd+R without any modifier", () => {
    const startCaptureAndTranscribe = vi.fn();
    useAudioGraphStore.setState({ startCaptureAndTranscribe });

    renderHook(() => useKeyboardShortcuts());

    act(() => {
      fireEvent.keyDown(window, { key: "r" });
    });

    expect(startCaptureAndTranscribe).not.toHaveBeenCalled();
  });

  it("Cmd+, opens the settings modal", () => {
    const openSettings = vi.fn();
    useAudioGraphStore.setState({ openSettings });

    renderHook(() => useKeyboardShortcuts());

    act(() => {
      fireEvent.keyDown(window, { key: ",", metaKey: true });
    });

    expect(openSettings).toHaveBeenCalledTimes(1);
  });

  it("Cmd+Shift+S opens sessions browser (not plain Cmd+S)", () => {
    const openSessionsBrowser = vi.fn();
    const startCaptureAndTranscribe = vi.fn();
    useAudioGraphStore.setState({
      openSessionsBrowser,
      startCaptureAndTranscribe,
    });

    renderHook(() => useKeyboardShortcuts());

    // Plain Cmd+S should not trigger either handler (no binding for it).
    act(() => {
      fireEvent.keyDown(window, { key: "s", metaKey: true });
    });
    expect(openSessionsBrowser).not.toHaveBeenCalled();
    expect(startCaptureAndTranscribe).not.toHaveBeenCalled();

    // Cmd+Shift+S opens sessions browser.
    act(() => {
      fireEvent.keyDown(window, {
        key: "S",
        metaKey: true,
        shiftKey: true,
      });
    });
    expect(openSessionsBrowser).toHaveBeenCalledTimes(1);
  });

  it("skips modifier shortcuts when focus is inside an <input>", () => {
    const startCaptureAndTranscribe = vi.fn();
    useAudioGraphStore.setState({ startCaptureAndTranscribe });

    renderHook(() => useKeyboardShortcuts());

    const input = document.createElement("input");
    document.body.appendChild(input);
    input.focus();

    act(() => {
      fireEvent.keyDown(input, { key: "r", metaKey: true });
    });

    expect(startCaptureAndTranscribe).not.toHaveBeenCalled();
    document.body.removeChild(input);
  });

  it("Escape closes settings modal even when typing in an input", () => {
    const closeSettings = vi.fn();
    useAudioGraphStore.setState({ closeSettings, settingsOpen: true });

    renderHook(() => useKeyboardShortcuts());

    const input = document.createElement("input");
    document.body.appendChild(input);
    input.focus();

    act(() => {
      fireEvent.keyDown(input, { key: "Escape" });
    });

    expect(closeSettings).toHaveBeenCalledTimes(1);
    document.body.removeChild(input);
  });

  it("Escape does NOT swallow the keystroke when sessionsBrowserOpen is stale (SHELL-R2: Sessions is a destination, not a modal — regression guard for the dead-keystroke bug)", () => {
    const closeSessionsBrowser = vi.fn();
    const closeSettings = vi.fn();
    useAudioGraphStore.setState({
      closeSessionsBrowser,
      closeSettings,
      settingsOpen: false,
      // `sessionsBrowserOpen` latches true on open and has no remaining
      // reader (see useKeyboardShortcuts.ts's module doc) — Escape must be
      // a true no-op here, not a preventDefaulted early return.
      sessionsBrowserOpen: true,
    });

    renderHook(() => useKeyboardShortcuts());

    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });

    expect(closeSessionsBrowser).not.toHaveBeenCalled();
    expect(closeSettings).not.toHaveBeenCalled();
  });

  it("Escape is a no-op when no modal is open", () => {
    const closeSettings = vi.fn();
    const closeSessionsBrowser = vi.fn();
    useAudioGraphStore.setState({
      closeSettings,
      closeSessionsBrowser,
      settingsOpen: false,
      sessionsBrowserOpen: false,
    });

    renderHook(() => useKeyboardShortcuts());

    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });

    expect(closeSettings).not.toHaveBeenCalled();
    expect(closeSessionsBrowser).not.toHaveBeenCalled();
  });

  it("removes its keydown listener on unmount", () => {
    const startCaptureAndTranscribe = vi.fn();
    useAudioGraphStore.setState({ startCaptureAndTranscribe });

    const { unmount } = renderHook(() => useKeyboardShortcuts());
    unmount();

    act(() => {
      fireEvent.keyDown(window, { key: "r", metaKey: true });
    });

    expect(startCaptureAndTranscribe).not.toHaveBeenCalled();
  });
});
