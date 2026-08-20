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

// Wrapped in an async IIFE rather than a top-level `await`: Vite's configured
// esbuild `build.target` (chrome87/edge88/es2020/firefox78/safari14, see
// vite.config.ts) does not support top-level await, so a bare top-level
// `await import(...)` fails the production build precisely under
// `VITE_WDIO_E2E=1` (the one env the `tauri-shell-e2e.yml` app-under-test
// build always sets) with "Top-level await is not available in the
// configured target environment". `await` inside an async function has no
// such target restriction, so this preserves the original ordering (the WDIO
// bridge installs before React mounts) without touching the shared build
// target.
async function bootstrap() {
  // WebdriverIO shell E2E bridge (seed audio-graph-f9e0, Wave 3): installs
  // `window.wdioTauri`, console forwarding, and invoke interception for
  // `browser.tauri.execute()`/`mock()`. Dynamically imported and gated behind
  // a build-time Vite env flag that is only ever set by the
  // `tauri-shell-e2e.yml` CI workflow's app-under-test build — a normal
  // `bun run build` never sets `VITE_WDIO_E2E`, so this branch is false and
  // the mocking/invoke-interception surface is never fetched into a shipped
  // app.
  if (import.meta.env.VITE_WDIO_E2E) {
    await import("@wdio/tauri-plugin");
  }

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
}

void bootstrap();
