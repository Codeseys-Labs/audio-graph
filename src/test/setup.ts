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
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(),
}));
