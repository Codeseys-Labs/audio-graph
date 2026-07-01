/**
 * Frontend anonymous diagnostics — a thin relay to the backend Sentry channel.
 *
 * The WebView does NOT run its own Sentry SDK. A prior design embedded
 * `@sentry/browser`, but its egress (a POST to `*.ingest.us.sentry.io`) is
 * blocked by the app's CSP `connect-src` (`ipc: http://ipc.localhost` only), so
 * every frontend envelope was silently dropped. Instead, the frontend forwards
 * structured, controlled ids to the backend via the `report_frontend_diagnostic`
 * Tauri command, and the (CSP-exempt) Rust Sentry — which already has the
 * transport, the mature scrubber, and the opt-in toggle — does the actual send.
 *
 * Load-bearing privacy invariant: **no free text ever leaves the renderer**.
 * Callers pass only short, controlled ids (an event `name` plus optional
 * `component`/`surface`); this module NEVER forwards an error's `message`,
 * `stack`, or any other free prose. The backend clamps every field to an id
 * shape and its scrubber is the belt-and-suspenders backstop.
 *
 * Fail-silent: telemetry must never throw into the caller's control flow, and
 * the backend `capture_diagnostic` already no-ops when analytics is disabled, so
 * there is no separate init/gate on this side.
 */

import { invoke } from "@tauri-apps/api/core";

/**
 * Structured, controlled fields attached to a frontend diagnostic. Callers pass
 * only these ids — never free text — so they physically cannot leak prose.
 */
export interface FrontendDiagFields {
  /** Coarse area id, e.g. `"frontend"`. */
  category: string;
  /** Optional component id, e.g. `"root-boundary"`. */
  component?: string;
  /** Optional surface id, e.g. `"invoke"`, `"window"`, `"unhandledrejection"`. */
  surface?: string;
}

/**
 * Capture a frontend diagnostic by relaying controlled ids to the backend.
 *
 * `name` is a stable, id-shaped triage id (e.g. `"frontend.invoke.error"` or a
 * Tauri command name); `fields` carries only controlled ids. The optional
 * `error` argument exists for call-site ergonomics ONLY — its contents
 * (message, stack, locals) are deliberately never read or forwarded, so an
 * error object's prose can never reach the wire.
 *
 * Fails silent: any failure to reach the backend (command missing, IPC error,
 * not running under Tauri) is swallowed — telemetry must never disrupt the app.
 */
export function captureFrontendError(
  name: string,
  fields: FrontendDiagFields,
  _error?: unknown,
): void {
  // Forward ONLY controlled ids. `_error` is intentionally ignored so no
  // message/stack/free text is ever transmitted; the backend derives the
  // Category enum and clamps every field to an id shape.
  void invoke("report_frontend_diagnostic", {
    name,
    category: fields.category,
    component: fields.component ?? null,
    surface: fields.surface ?? null,
  }).catch(() => {
    // Fail silent: telemetry must never throw into the caller.
  });
}
