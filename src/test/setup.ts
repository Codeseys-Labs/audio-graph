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
