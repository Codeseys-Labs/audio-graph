import "@testing-library/jest-dom";
import { invoke } from "@tauri-apps/api/core";
import { configure } from "@testing-library/react";
import { beforeEach, vi } from "vitest";
// Initialize the i18next singleton for the whole test run so components that
// call `t()` render real English copy instead of raw key strings, regardless
// of each test file's import graph or execution order.
import "../i18n";
import { useReviewStore } from "../session/reviewStore";
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
//
// `systemDrawerOpen`/`credentialPresence` (SHELL-R3, ADR-0046) join the same
// beforeEach for the same class of reason: they're new store-wide state this
// unit introduces, read by `NowStrip`/`SystemDrawer` but not necessarily
// known to every OTHER test file's own local reset helper, so a test that
// opens the drawer or seeds a route-probe fixture in one `it()` must not
// leak into an unrelated one later in the same file.
//
// The Review store (#140) is the same kind of singleton: an open session, an
// in-flight round or a held artifact from one test must not be the starting
// state of the next, whichever order the files and tests run in
// (`bun run test:flake -- --shuffle`). `close()` also bumps its generation, so
// a load still in flight from the previous test cannot land in this one.
//
// The mocked `invoke` is the same: `vi.clearAllMocks()` (what most files call
// in their own `beforeEach`) clears recorded calls but KEEPS the
// `mockImplementation` a previous test installed, so a test that relies on the
// default (`undefined`) answer, or on its own implementation being the only
// one, sees the previous test's backend whenever order changes. Reset it to the
// bare `vi.fn()` first; every file's own `beforeEach` then installs what it needs.
beforeEach(() => {
  vi.mocked(invoke).mockReset();
  useReviewStore.getState().close();
  useAudioGraphStore.setState({
    nav: DEFAULT_SHELL_NAV,
    // Gates `getSessionId()`, so a sample preview left on by one test turns
    // the next test's `stopCapture` into a no-route.
    samplePreviewActive: false,
    // Persisted UI choices whose initial value is the localStorage-less
    // default; the auto-answer trigger and the native-converse gate read them.
    conversationMode: "notes",
    converseEngine: "pipelined",
    pendingFinalizingSession: null,
    systemDrawerOpen: false,
    credentialPresence: [],
  });
});

// RTL's `waitFor`/`findBy*` give up after 1 s by default, well inside the
// per-test budget, so on an oversubscribed machine they fail with "unable to
// find" while the render is merely slow. Scale them with the same budget as
// `testTimeout` (see `vitest.config.ts`). A wait still returns the moment its
// condition holds, so only a failing wait is any slower.
configure({ asyncUtilTimeout: process.env.GITHUB_ACTIONS ? 1_000 : 10_000 });

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
