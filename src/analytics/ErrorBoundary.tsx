/**
 * Root React error boundary.
 *
 * Catches render/lifecycle errors from the subtree, reports a structured
 * frontend diagnostic (category `frontend`, component `root-boundary`) via the
 * anonymous analytics channel, and — audio-graph-16e2 — actually RECOVERS:
 * it renders a real fallback (installed at the root mount, `main.tsx`, so a
 * render crash never blanks the window for the rest of the session) and it
 * heals itself the moment the app's navigation/location state changes, so a
 * fault in one view does not permanently poison every other view.
 *
 * Previously this component's `render()` fell back to `this.props.fallback
 * ?? null`, and the root mount (`main.tsx`) passed no `fallback` at all — ANY
 * caught render error blanked the whole window PERMANENTLY (nothing here
 * ever cleared `hasError`), indistinguishable from a process crash. That gap
 * is what this unit closes: `fallback` is now a render-prop the caller uses
 * to show a real recovery UI, and this class resets its own caught state
 * whenever `useAudioGraphStore`'s `nav` changes reference — the fallback's
 * own "back to Capture" control (today the only surface that can change
 * `nav` while the boundary is tripped, since the crashed subtree is what
 * would normally render any other navigation UI) drives that, but the
 * listener itself is generic: it reacts to the nav object changing, not to
 * any specific button, so any future nav writer heals the boundary too.
 *
 * SECURITY (never regress this): the caught error's `message` and `stack`
 * are NEVER read into state, rendered, or forwarded anywhere — only
 * `error.name` is kept, and it is the only error-derived value the fallback
 * render-prop below ever receives. `name` is a plain writable string
 * property (not an enum), so `getDerivedStateFromError` clamps it against
 * `SAFE_ERROR_NAME_RE` before it ever reaches state — that clamp, not the
 * type system, is what makes "small, closed vocabulary like `TypeError` or
 * `RangeError`" true rather than aspirational; anything shaped like free
 * text (e.g. a class overriding `name` with interpolated transcript
 * content) collapses to the literal `"Error"`. `componentDidCatch` still
 * relays only the controlled, id-shaped diagnostic name to
 * `captureFrontendError`; the raw error is not forwarded there either — no
 * transcript/notes content, no stack, no free text ever leaves this
 * component.
 *
 * A class component is required: `componentDidCatch` / `getDerivedStateFromError`
 * have no hook equivalent.
 */

import { Component, type ErrorInfo, type ReactNode } from "react";
import { useAudioGraphStore } from "../store";
import { captureFrontendError } from "./sentry";

/**
 * Real `Error.prototype.name` values are short identifiers (`TypeError`,
 * `RangeError`, `AggregateError`, a custom `class FooError extends Error`'s
 * name, ...). `name` is a plain writable string property though, so nothing
 * in the language stops arbitrary code from setting it to unbounded free
 * text. `getDerivedStateFromError` below tests every candidate name against
 * this pattern and falls back to the literal `"Error"` on a miss, so the
 * fallback UI can only ever display an identifier-shaped, length-bounded
 * string — never smuggled prose.
 */
const SAFE_ERROR_NAME_RE = /^[A-Za-z$_][\w$ ]{0,63}$/;

interface Props {
  children: ReactNode;
  /**
   * Render-prop fallback invoked with the caught error's `name` ONLY (see
   * this file's SECURITY note) — never its message or stack. A plain
   * `ReactNode` cannot express that, since the error isn't known yet at the
   * point the caller builds its JSX tree, hence the function shape.
   */
  fallback?: (errorName: string) => ReactNode;
}

interface State {
  hasError: boolean;
  /** `error.name` only — see this file's SECURITY note. `null` while
   * `hasError` is false. */
  errorName: string | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { hasError: false, errorName: null };

  private unsubscribeNav: (() => void) | null = null;

  static getDerivedStateFromError(error: unknown): State {
    // `error` is sometimes a non-Error throw (e.g. a thrown string) in
    // pathological code, and even a well-formed-looking thrown object can
    // define `name` as a throwing getter — `"name" in error` does not invoke
    // it, but the property read on the next line does, so the whole read is
    // wrapped in try/catch. Without this, handling ONE pathological crash
    // would itself throw out of the boundary's own crash handler, blanking
    // the window this component exists to keep from blanking.
    let name: unknown;
    try {
      name =
        error && typeof error === "object" && "name" in error
          ? (error as { name?: unknown }).name
          : undefined;
    } catch {
      name = undefined;
    }
    return {
      hasError: true,
      errorName:
        typeof name === "string" && SAFE_ERROR_NAME_RE.test(name)
          ? name
          : "Error",
    };
  }

  componentDidCatch(_error: Error, _info: ErrorInfo): void {
    // Relay a controlled id only — the caught error is not forwarded, so its
    // message/stack never leave the renderer.
    captureFrontendError("frontend.react.render", {
      category: "frontend",
      component: "root-boundary",
    });
  }

  componentDidMount(): void {
    // Recovery semantics (audio-graph-16e2): listen for the app's nav/
    // location state changing and heal a tripped boundary automatically.
    // `state.nav` is a plain object the store slice replaces wholesale on
    // every actual navigation write (`shellNav.ts`), so a reference
    // inequality is exactly "navigation happened" — cheaper and more
    // reliable than deep-comparing `dest`/`sessionId`/`lens` by hand.
    this.unsubscribeNav = useAudioGraphStore.subscribe((state, prevState) => {
      if (this.state.hasError && state.nav !== prevState.nav) {
        this.reset();
      }
    });
  }

  componentWillUnmount(): void {
    this.unsubscribeNav?.();
    this.unsubscribeNav = null;
  }

  private reset = (): void => {
    this.setState({ hasError: false, errorName: null });
  };

  render(): ReactNode {
    if (this.state.hasError) {
      return this.props.fallback?.(this.state.errorName ?? "Error") ?? null;
    }
    return this.props.children;
  }
}
