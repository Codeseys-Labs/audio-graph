import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useShellLayout } from "./useShellLayout";

type ChangeListener = () => void;

interface MatchMediaControl {
  setWidth: (next: number) => void;
  listenerCount: () => number;
}

/** Parses the `(min-width: NNNpx)` queries `useShellLayout` issues and drives
 * `.matches` off a single controllable `width` variable, mirroring the
 * `window.matchMedia` mock convention already used in
 * `ChatSidebar.test.tsx` (jsdom implements neither `matchMedia` nor
 * `scrollIntoView`, so every consumer test supplies its own). */
function installMatchMedia(initialWidth: number): MatchMediaControl {
  let width = initialWidth;
  const listeners = new Map<string, Set<ChangeListener>>();

  function minWidthOf(query: string): number {
    const match = query.match(/min-width:\s*(\d+)px/);
    return match ? Number(match[1]) : 0;
  }

  window.matchMedia = vi.fn().mockImplementation((query: string) => {
    const threshold = minWidthOf(query);
    return {
      get matches() {
        return width >= threshold;
      },
      media: query,
      onchange: null,
      addEventListener: (_type: string, cb: ChangeListener) => {
        if (!listeners.has(query)) listeners.set(query, new Set());
        listeners.get(query)?.add(cb);
      },
      removeEventListener: (_type: string, cb: ChangeListener) => {
        listeners.get(query)?.delete(cb);
      },
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    } as unknown as MediaQueryList;
  }) as unknown as typeof window.matchMedia;

  return {
    setWidth(next: number) {
      width = next;
      for (const set of listeners.values()) {
        for (const cb of set) cb();
      }
    },
    listenerCount() {
      let total = 0;
      for (const set of listeners.values()) total += set.size;
      return total;
    },
  };
}

describe("useShellLayout", () => {
  afterEach(() => {
    // @ts-expect-error -- test-only teardown of the mock installed above.
    delete window.matchMedia;
  });

  it.each([
    [767, "compact"],
    [768, "compact"],
    [1023, "compact"],
    [1024, "standard"],
    [1279, "standard"],
    [1280, "wide"],
  ] as const)("returns %i -> %s at the exact boundary", (width, expected) => {
    installMatchMedia(width);
    const { result } = renderHook(() => useShellLayout());
    expect(result.current).toBe(expected);
  });

  it("has no boundary at 768px — 767 and 768 both resolve to compact", () => {
    installMatchMedia(767);
    const below = renderHook(() => useShellLayout());
    expect(below.result.current).toBe("compact");

    installMatchMedia(768);
    const at = renderHook(() => useShellLayout());
    expect(at.result.current).toBe("compact");
  });

  it("reacts to a change event after mount", () => {
    const control = installMatchMedia(1400);
    const { result } = renderHook(() => useShellLayout());
    expect(result.current).toBe("wide");

    act(() => {
      control.setWidth(800);
    });
    expect(result.current).toBe("compact");

    act(() => {
      control.setWidth(1100);
    });
    expect(result.current).toBe("standard");
  });

  it("removes both matchMedia listeners on unmount", () => {
    const control = installMatchMedia(1400);
    const { unmount } = renderHook(() => useShellLayout());
    expect(control.listenerCount()).toBe(2);

    unmount();
    expect(control.listenerCount()).toBe(0);
  });

  it("defaults to wide when matchMedia is unavailable (most jsdom test environments)", () => {
    // @ts-expect-error -- simulate an environment with no matchMedia at all.
    delete window.matchMedia;
    const { result } = renderHook(() => useShellLayout());
    expect(result.current).toBe("wide");
  });
});
