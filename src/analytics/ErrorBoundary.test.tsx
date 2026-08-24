import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import RootErrorFallback from "../components/RootErrorFallback";
import i18n from "../i18n";
import { useAudioGraphStore } from "../store";
import { DEFAULT_SHELL_NAV } from "../store/shellNav";
import { ErrorBoundary } from "./ErrorBoundary";

// Spy on the real module (not a `vi.mock` stub, same convention as
// `safeInvoke.test.ts`) so `sentry.test.ts`'s own privacy-invariant coverage
// (no message/stack ever forwarded) stays the single source of truth for
// that contract; this file only needs to pin call COUNT + the controlled
// fields `ErrorBoundary` passes.
import * as sentry from "./sentry";

// A raw stack frame line always looks like "at <fn> (<file>:<line>:<col>)"
// or "at <file>:<line>:<col>" — used below to prove no stack text leaked.
const STACK_FRAME_RE = /\bat .*:\d+:\d+/;

const SECRET_MESSAGE = "boom: raw transcript content should never leak";

/**
 * A child whose throw/succeed behavior is controlled by a mutable flag
 * OUTSIDE React state — a real error boundary catch discards the failed
 * subtree's React state entirely, so this can't be a prop threaded through
 * the crashed instance; it has to live in a ref the test toggles directly,
 * then forces a remount (via the boundary's own reset) to observe it.
 */
const bomb = { shouldThrow: true };
function ConditionalBomb(): ReactNode {
  if (bomb.shouldThrow) {
    throw new Error(SECRET_MESSAGE);
  }
  return <div data-testid="recovered-child">recovered</div>;
}

/** Mirrors `main.tsx`'s exact composition (`ErrorBoundary` + the real
 * `RootErrorFallback`, wired to the real store action) so this suite proves
 * the actual root-mount wiring, not `ErrorBoundary` against a throwaway test
 * fallback that could drift from what ships. */
function renderRootBoundary(children: ReactNode) {
  return render(
    <ErrorBoundary
      fallback={(errorName) => (
        <RootErrorFallback
          errorName={errorName}
          onReload={() => window.location.reload()}
          onBackToCapture={() =>
            useAudioGraphStore.getState().setNavDest("capture")
          }
        />
      )}
    >
      {children}
    </ErrorBoundary>,
  );
}

describe("ErrorBoundary — root mount composition (audio-graph-16e2)", () => {
  let errorSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(async () => {
    await i18n.changeLanguage("en");
    bomb.shouldThrow = true;
    useAudioGraphStore.setState({ nav: DEFAULT_SHELL_NAV });
    // React logs the caught error to console.error even though a boundary
    // handles it (expected, noisy) — silence it like the repo's existing
    // console-spy convention (StorageBanner.test.tsx) rather than letting it
    // spam the run.
    errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
  });

  afterEach(() => {
    errorSpy.mockRestore();
    vi.restoreAllMocks();
  });

  // Deliverable (a)+(d): a real fallback renders instead of the pre-fix
  // permanent blank window. MUTATION-HONESTY: if `main.tsx`'s `fallback`
  // prop (or `ErrorBoundary.render()`'s `this.props.fallback?.(...)` call)
  // is removed/reverted to the old `?? null` with no fallback supplied,
  // `getByTestId("root-error-fallback-class")` and
  // `getByRole("button", { name: /reload/i })` both throw "not found" —
  // those are the assertions that die.
  it("renders the fallback with the error class visible and a Reload button, instead of blanking the window", () => {
    renderRootBoundary(<ConditionalBomb />);

    expect(screen.getByTestId("root-error-fallback-class")).toHaveTextContent(
      "Error type: Error",
    );
    expect(
      screen.getByRole("button", { name: /reload audiograph/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: /back to capture/i }),
    ).toBeInTheDocument();
  });

  // SECURITY: never regress the "no content/stack in the UI" invariant.
  it("never renders the thrown error's message or stack, only error.name", () => {
    renderRootBoundary(<ConditionalBomb />);

    expect(document.body.textContent).not.toContain(SECRET_MESSAGE);
    expect(document.body.textContent).not.toContain("boom");
    expect(document.body.textContent ?? "").not.toMatch(STACK_FRAME_RE);
  });

  // Deliverable (d): captureFrontendError fires exactly once per catch, with
  // only the pre-existing controlled id fields — the error itself is never
  // passed through. MUTATION-HONESTY: a mutant that calls
  // `captureFrontendError` twice (e.g. once in `getDerivedStateFromError`
  // AND once in `componentDidCatch`) fails `toHaveBeenCalledTimes(1)`; a
  // mutant that forwards the caught error as a 3rd arg still passes this
  // assertion (that leak is `sentry.test.ts`'s job) but WOULD fail the
  // "never renders" test above if it also changed `render()` to display it.
  it("calls captureFrontendError exactly once with only the controlled id fields", () => {
    const capture = vi.spyOn(sentry, "captureFrontendError");

    renderRootBoundary(<ConditionalBomb />);

    expect(capture).toHaveBeenCalledTimes(1);
    expect(capture).toHaveBeenCalledWith("frontend.react.render", {
      category: "frontend",
      component: "root-boundary",
    });
  });

  // Deliverable (b)+(d): recovery semantics. MUTATION-HONESTY: deleting the
  // `componentDidMount` nav subscription, or dropping its
  // `state.nav !== prevState.nav` check (e.g. hardcoding `false`), leaves
  // the fallback on screen after the click — `queryByTestId
  // ("root-error-fallback")` stays truthy and `not.toBeInTheDocument()`
  // fails; `getByTestId("recovered-child")` also never appears.
  it('resets the boundary and remounts children when "Back to Capture" changes nav', () => {
    renderRootBoundary(<ConditionalBomb />);
    expect(screen.getByTestId("root-error-fallback")).toBeInTheDocument();

    // The remounted child must not immediately re-crash, or the boundary
    // would look "stuck" for the wrong reason (the child, not the reset).
    bomb.shouldThrow = false;
    fireEvent.click(screen.getByRole("button", { name: /back to capture/i }));

    expect(screen.queryByTestId("root-error-fallback")).not.toBeInTheDocument();
    expect(screen.getByTestId("recovered-child")).toBeInTheDocument();
  });

  // Regression guard for the specific bug this design avoids: `setNavDest`
  // (not `setWorkspaceView`) is what the fallback must call, because
  // `setWorkspaceView`'s same-value bailout (`navForWorkspaceView`) would
  // return the SAME `nav` object reference when the crash happened while
  // already on Capture (the most common case — Capture is the default
  // destination) — the boundary's `state.nav !== prevState.nav` check would
  // then never fire and the user would be stuck with no way out.
  it('still resets even when the crash happened while nav.dest was already "capture"', () => {
    useAudioGraphStore.setState({ nav: DEFAULT_SHELL_NAV }); // dest: "capture" already
    renderRootBoundary(<ConditionalBomb />);

    bomb.shouldThrow = false;
    fireEvent.click(screen.getByRole("button", { name: /back to capture/i }));

    expect(screen.queryByTestId("root-error-fallback")).not.toBeInTheDocument();
    expect(screen.getByTestId("recovered-child")).toBeInTheDocument();
  });

  // A tripped-but-unmounted boundary must not keep a live store subscription
  // around — that would leak memory and (pre-React-19) risked a "setState on
  // an unmounted component" warning the next time nav changed anywhere else
  // in the app. MUTATION-HONESTY CORRECTION: an earlier version of this test
  // asserted `console.error` stayed silent after unmount + a later nav
  // write, reasoning that a leaked subscription would trigger React's
  // "setState on unmounted component" warning. Probe confirmed that
  // reasoning was wrong for this codebase's React version (19.2.6): React
  // 18+ removed that warning entirely, so setState on an unmounted class
  // instance is now a silent no-op — deleting `componentWillUnmount`'s
  // unsubscribe body left the assertion trivially true. This version instead
  // spies on the store's own `subscribe`, capturing the wrapped unsubscribe
  // function `ErrorBoundary` receives, and asserts THAT function is called
  // on unmount — an observable fact about the subscription lifecycle itself,
  // not a React internal that may or may not warn.
  it("unsubscribes from the store on unmount", () => {
    const realSubscribe = useAudioGraphStore.subscribe;
    const unsubscribeSpy = vi.fn();
    const subscribeSpy = vi
      .spyOn(useAudioGraphStore, "subscribe")
      .mockImplementation((...args: Parameters<typeof realSubscribe>) => {
        const realUnsubscribe = realSubscribe(...args);
        return () => {
          unsubscribeSpy();
          realUnsubscribe();
        };
      });

    const { unmount } = renderRootBoundary(<ConditionalBomb />);
    expect(subscribeSpy).toHaveBeenCalledTimes(1);
    expect(unsubscribeSpy).not.toHaveBeenCalled();

    unmount();

    expect(unsubscribeSpy).toHaveBeenCalledTimes(1);
  });

  // Hardening: a thrown value whose `name` property is a THROWING getter
  // must not itself escape `getDerivedStateFromError` — that would make
  // handling one crash produce a second, unhandled one, blanking the window
  // this component exists to keep from blanking. MUTATION-HONESTY: a mutant
  // that removes the try/catch around the `name` read makes this render
  // throw instead of showing the fallback — `getByTestId
  // ("root-error-fallback-class")` throws "not found" (nothing rendered) or
  // the whole `render()` call throws.
  it("still renders the fallback safely when the thrown error's name getter itself throws", () => {
    function ThrowingNameBomb(): ReactNode {
      const evil: { name?: string } = {};
      Object.defineProperty(evil, "name", {
        get(): string {
          throw new Error("name getter exploded");
        },
      });
      throw evil;
    }

    renderRootBoundary(<ThrowingNameBomb />);

    expect(screen.getByTestId("root-error-fallback-class")).toHaveTextContent(
      "Error type: Error",
    );
  });

  // Hardening: `error.name` is a plain writable string, not an enum — any
  // code (including a hostile/buggy third-party dependency) can set it to
  // arbitrary free text. MUTATION-HONESTY: a mutant that removes the
  // `SAFE_ERROR_NAME_RE` clamp (rendering `name` whenever it's merely a
  // non-empty string) makes this test's `toHaveTextContent("Error type:
  // Error")` fail and the `not.toContain` assertion below fail instead,
  // because the injected free text would render verbatim.
  it("clamps a free-text error.name to the safe literal instead of rendering it verbatim", () => {
    const INJECTED = "<<< raw transcript content should never render >>>";
    function FreeTextNameBomb(): ReactNode {
      const err = new Error("boom");
      err.name = INJECTED;
      throw err;
    }

    renderRootBoundary(<FreeTextNameBomb />);

    expect(screen.getByTestId("root-error-fallback-class")).toHaveTextContent(
      "Error type: Error",
    );
    expect(document.body.textContent ?? "").not.toContain(INJECTED);
  });
});
