import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./analytics/ErrorBoundary";
import { captureFrontendError } from "./analytics/sentry";
import "./styles.css";
// Initialize i18next before React mounts so the first render has
// translations available. Side-effect import — do not remove.
import "./i18n";
import { applyTheme, readStoredTheme } from "./theme";

// Apply the persisted theme before the first paint so there is no
// dark→light flash when a light-theme user reloads (ADR-0009, Wave 4).
applyTheme(readStoredTheme());

// Install global error handlers once. Each relays a CONTROLLED, id-shaped
// diagnostic name to the backend Sentry channel via `captureFrontendError`;
// the raw `ErrorEvent` / rejection `reason` is never forwarded (no message,
// stack, or free text leaves the renderer). `captureFrontendError` is
// fail-silent and the backend `capture_diagnostic` no-ops when analytics is
// off, so this stays inert when the user has not opted in — no init/gate here.
window.addEventListener("error", () => {
  captureFrontendError("frontend.window.error", {
    category: "frontend",
    surface: "window",
  });
});
window.addEventListener("unhandledrejection", () => {
  captureFrontendError("frontend.unhandledrejection", {
    category: "frontend",
    surface: "unhandledrejection",
  });
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
