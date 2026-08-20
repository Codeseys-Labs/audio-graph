import "@testing-library/jest-dom";
import { vi } from "vitest";
// Initialize the i18next singleton for the whole test run so components that
// call `t()` render real English copy instead of raw key strings, regardless
// of each test file's import graph or execution order.
import "../i18n";

// jsdom does not implement ResizeObserver, which Radix UI primitives (e.g.
// the Tooltip's positioning) construct on mount. Provide a no-op polyfill so
// those components can render under test without a ReferenceError.
if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as unknown as typeof ResizeObserver;
}

// Some Node/jsdom pairings ship a spec-compliant `localStorage` that is only
// wired up when a `--localstorage-file` path is provided (Node's own Web
// Storage implementation) and jsdom no longer shims one in that case, so
// both the bare `localStorage` global and `window.localStorage` come back
// `undefined` — breaking every test that calls `localStorage.clear()` in
// `beforeEach`/`afterEach` (audio-graph-1d92 discovered this while adding
// unrelated component tests). Provide a minimal in-memory `Storage` polyfill
// only when the real thing is missing, mirroring the `ResizeObserver` shim
// above — a no-op on any Node/jsdom pairing where `localStorage` already
// works.
function needsLocalStoragePolyfill(candidate: unknown): boolean {
  return (
    typeof candidate !== "object" ||
    candidate === null ||
    typeof (candidate as Storage).clear !== "function"
  );
}
if (
  needsLocalStoragePolyfill(globalThis.localStorage) ||
  (typeof window !== "undefined" &&
    needsLocalStoragePolyfill(window.localStorage))
) {
  const store = new Map<string, string>();
  const polyfill: Storage = {
    getItem: (key) => (store.has(key) ? (store.get(key) as string) : null),
    setItem: (key, value) => {
      store.set(key, String(value));
    },
    removeItem: (key) => {
      store.delete(key);
    },
    clear: () => {
      store.clear();
    },
    key: (index) => Array.from(store.keys())[index] ?? null,
    get length() {
      return store.size;
    },
  };
  Object.defineProperty(globalThis, "localStorage", {
    value: polyfill,
    configurable: true,
    writable: true,
  });
  if (typeof window !== "undefined") {
    Object.defineProperty(window, "localStorage", {
      value: polyfill,
      configurable: true,
      writable: true,
    });
  }
}

// Mock the Tauri API so tests don't need a running Tauri runtime.
//
// `Channel` is the streaming-chat IPC transport (audio-graph-1534). The real
// class registers itself with the Tauri runtime; under test we only need an
// object that captures `onmessage` so a test can drive frames by calling it
// directly (mirroring what the Rust `channel.send()` end would deliver).
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: class {
    id = 0;
    onmessage: ((message: unknown) => void) | null = null;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(),
}));
