import "@testing-library/jest-dom";
import { beforeEach, vi } from "vitest";
// Initialize the i18next singleton for the whole test run so components that
// call `t()` render real English copy instead of raw key strings, regardless
// of each test file's import graph or execution order.
import "../i18n";
import { useAudioGraphStore } from "../store";
import { DEFAULT_SHELL_NAV } from "../store/shellNav";

// ShellNav (SHELL-R1): `nav` replaced App-local `workspaceView` `useState`,
// so — unlike the old per-mount local state — it now lives in the
// module-singleton Zustand store and survives across `render(<App />)` calls
// within one test file. A production window mounts `<App />` exactly once,
// so this is purely a test-isolation artifact; reset it here (once, for
// every test file) rather than asking each test file's own store-reset
// helper to know about a slice it didn't introduce (several, including
// `App.contract.test.tsx`, must stay untouched — seed audio-graph-59fb).
//
// `pendingFinalizingSession` (SHELL-R2, audio-graph-e0c4) joins the same
// beforeEach for the same reason: `stopCapture` writes it directly (not
// through a named flag setter a per-file reset helper would already know
// about), so a test that exercises Stop in one `it()` must not leak an
// optimistic row into an unrelated one later in the same file.
beforeEach(() => {
  useAudioGraphStore.setState({
    nav: DEFAULT_SHELL_NAV,
    pendingFinalizingSession: null,
  });
});

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
