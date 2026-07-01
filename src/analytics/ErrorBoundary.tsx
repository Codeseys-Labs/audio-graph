/**
 * Root React error boundary.
 *
 * Catches render/lifecycle errors from the subtree, reports a structured
 * frontend diagnostic (category `frontend`, component `root-boundary`) via the
 * anonymous analytics channel, and renders a minimal fallback so a render crash
 * does not leave a blank window. No free text is transmitted — the diagnostic
 * carries only a controlled, id-shaped name plus controlled tags; the caught
 * error itself is never forwarded (its message/stack stay in the renderer).
 *
 * A class component is required: `componentDidCatch` / `getDerivedStateFromError`
 * have no hook equivalent.
 */

import { Component, type ErrorInfo, type ReactNode } from "react";
import { captureFrontendError } from "./sentry";

interface Props {
  children: ReactNode;
  /** Optional fallback UI shown after a caught error. */
  fallback?: ReactNode;
}

interface State {
  hasError: boolean;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { hasError: false };

  static getDerivedStateFromError(): State {
    return { hasError: true };
  }

  componentDidCatch(_error: Error, _info: ErrorInfo): void {
    // Relay a controlled id only — the caught error is not forwarded, so its
    // message/stack never leave the renderer.
    captureFrontendError("frontend.react.render", {
      category: "frontend",
      component: "root-boundary",
    });
  }

  render(): ReactNode {
    if (this.state.hasError) {
      return this.props.fallback ?? null;
    }
    return this.props.children;
  }
}
