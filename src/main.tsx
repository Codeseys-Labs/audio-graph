import { invoke } from "@tauri-apps/api/core";
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ErrorBoundary } from "./analytics/ErrorBoundary";
import { captureFrontendError, initSentry } from "./analytics/sentry";
import "./styles.css";
// Initialize i18next before React mounts so the first render has
// translations available. Side-effect import — do not remove.
import "./i18n";
import { applyTheme, readStoredTheme } from "./theme";
import type { AnalyticsInfo } from "./types";

// Apply the persisted theme before the first paint so there is no
// dark→light flash when a light-theme user reloads (ADR-0009, Wave 4).
applyTheme(readStoredTheme());

// Install global error handlers once. These fire even if analytics is off;
// `captureFrontendError` is a no-op until Sentry is initialised, so gating
// stays consistent (no events go out when analytics is disabled).
//
// We pass the real error (`event.error` / `event.reason`) so the exception
// TYPE reaches Sentry for triage. This stays privacy-safe: `scrubEvent` in
// `beforeSend` nulls the exception value, basenames + clears every stack
// frame, and keeps only the allowlisted structured tags — so the error's
// message, source, and locals never leave the machine, only its type does.
window.addEventListener("error", (event: ErrorEvent) => {
  captureFrontendError(
    "window.error",
    {
      category: "frontend",
      surface: "window",
    },
    event.error,
  );
});
window.addEventListener(
  "unhandledrejection",
  (event: PromiseRejectionEvent) => {
    captureFrontendError(
      "window.unhandledrejection",
      {
        category: "frontend",
        surface: "unhandledrejection",
      },
      event.reason,
    );
  },
);

function mount(): void {
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </React.StrictMode>,
  );
}

// Fetch the opt-in analytics setting at startup (mirrors how the rest of the
// app learns settings — the `get_analytics_info` Tauri command). Initialise
// Sentry only when enabled, then mount. A failed/absent read leaves analytics
// off (fail-closed) and never blocks the UI from mounting.
void (async () => {
  try {
    const info = await invoke<AnalyticsInfo>("get_analytics_info");
    initSentry(info.enabled);
  } catch {
    // Fail closed: no analytics, but the app must still render.
    initSentry(false);
  } finally {
    mount();
  }
})();
