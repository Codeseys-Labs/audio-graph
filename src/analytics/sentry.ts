/**
 * Frontend anonymous diagnostics (Sentry browser SDK).
 *
 * This is the browser-side mirror of the backend analytics channel
 * (`src-tauri/src/analytics/mod.rs`). Same embedded DSN (a client-side public
 * key, safe to embed), same opt-in gating on the `analytics_enabled` setting,
 * and the same load-bearing privacy invariant: **no free text ever leaves the
 * machine**.
 *
 * The backend's `scrub_event` reduces every free-prose field to a redaction
 * sentinel and keeps only a small allowlist of structured tags. The frontend
 * `beforeSend` scrubber ([`scrubEvent`]) enforces the equivalent contract:
 *
 *   - drop `event.message`, `event.request`, `event.user`, `event.breadcrumbs`,
 *     and `event.extra` (all of which can carry free prose / PII); and
 *   - keep ONLY the allowlisted structured tags
 *     ({@link ALLOWLISTED_TAG_KEYS}: `category`, `component`, `surface`), each
 *     value shape-checked against a strict id pattern and otherwise dropped.
 *
 * Because callers only ever set structured, controlled tags via
 * [`captureFrontendError`], and `beforeSend` nulls the message and strips
 * everything else, an error object's message (which can interpolate transcript
 * text, credentials, or paths) can never be transmitted.
 *
 * Gating: [`initSentry`]`(false)` is a no-op — the SDK is never initialised, so
 * nothing is ever sent. When analytics is off, all capture helpers are inert.
 */

import type { Event, ErrorEvent as SentryErrorEvent } from "@sentry/browser";
import * as Sentry from "@sentry/browser";

/**
 * The same embedded Sentry DSN as the backend (`DEFAULT_DSN` in
 * `src-tauri/src/analytics/mod.rs`). A DSN is a client-side public key that
 * only authorises *sending* events to a project — it is not a secret and is
 * safe to embed. Kept literally in sync with the backend copy.
 */
const DSN =
  "https://1e39b03ea3018d02551500bf428306b9@o4511644093448192.ingest.us.sentry.io/4511644102885381";

/**
 * The ONLY event tag keys allowed to survive [`scrubEvent`]. Mirrors the
 * backend allowlist's frontend lane: structured, controlled identifiers with
 * no free prose. Every other tag key is dropped.
 */
export const ALLOWLISTED_TAG_KEYS = [
  "category",
  "component",
  "surface",
] as const;

/**
 * Shape check for an allowlisted tag value: a short, controlled id. Mirrors the
 * backend's per-value shape validation (`^[a-z0-9._:-]{1,48}$`) so that even an
 * allowlisted key cannot smuggle free prose through as its value. Any value
 * failing this check is dropped rather than kept.
 */
const SAFE_ID = /^[a-z0-9._:-]{1,48}$/;

/**
 * Structured, controlled fields attached to a frontend diagnostic. Callers pass
 * only these ids — never free text — so they physically cannot leak prose.
 */
export interface FrontendDiagFields {
  /** Coarse area, e.g. `"frontend"`. */
  category: string;
  /** Optional component id, e.g. `"root-boundary"`. */
  component?: string;
  /** Optional surface id, e.g. `"invoke"`, `"window"`, `"unhandledrejection"`. */
  surface?: string;
}

/**
 * `beforeSend` scrubber: the frontend privacy chokepoint. Strips every free-text
 * / PII-bearing field and keeps only the allowlisted, shape-valid structured
 * tags. Exported for direct unit testing.
 *
 * @returns the scrubbed event (always kept — we never drop the event itself,
 *   only its unsafe contents).
 */
export function scrubEvent(event: Event): Event | null {
  // Null out free prose and PII-bearing fields. `message` can interpolate
  // transcript text / paths / secrets; request/user carry network + identity
  // data; breadcrumbs/extra can carry anything the SDK auto-collected.
  event.message = undefined;
  event.request = undefined;
  event.user = undefined;
  event.breadcrumbs = undefined;
  event.extra = undefined;

  // Exception values are free prose too — drop each value (keep the type for
  // triage, matching the backend which retains the exception `type`).
  if (event.exception?.values) {
    for (const exception of event.exception.values) {
      exception.value = undefined;
    }
  }

  // Keep ONLY allowlisted tag keys whose value passes the id shape check.
  if (event.tags) {
    const kept: NonNullable<Event["tags"]> = {};
    for (const key of ALLOWLISTED_TAG_KEYS) {
      const value = event.tags[key];
      if (typeof value === "string" && SAFE_ID.test(value)) {
        kept[key] = value;
      }
    }
    event.tags = kept;
  }

  return event;
}

/**
 * Initialise the frontend Sentry client, gated on the analytics setting.
 *
 * `initSentry(false)` is a no-op: the SDK is never initialised, so no events
 * are ever sent. Mirrors the backend's unbound-hub gating. Idempotent — if a
 * client is already initialised, subsequent calls are ignored.
 */
export function initSentry(enabled: boolean): void {
  if (!enabled) return;
  if (Sentry.getClient()) return;

  Sentry.init({
    dsn: DSN,
    environment: import.meta.env.DEV ? "development" : "production",
    // ANONYMOUS: never attach IP / cookies / request bodies.
    sendDefaultPii: false,
    // No release-health / session envelopes: `init` does not enable browser
    // session tracking unless `browserSessionIntegration` is added, and we
    // deliberately do not add it (mirrors the backend's disabled sessions).
    beforeSend: (event: SentryErrorEvent) =>
      scrubEvent(event) as SentryErrorEvent,
  });
}

/**
 * Capture a frontend diagnostic with structured, controlled tags only.
 *
 * `name` is a stable id (e.g. a Tauri command name) attached as the
 * `event.name` tag; the `error` (if any) supplies the exception type for
 * triage. All free prose is removed by [`scrubEvent`] in `beforeSend`.
 *
 * No-op when Sentry is not initialised (analytics off).
 */
export function captureFrontendError(
  name: string,
  fields: FrontendDiagFields,
  error?: unknown,
): void {
  if (!Sentry.getClient()) return;

  Sentry.withScope((scope) => {
    scope.setTag("event.name", name);
    scope.setTag("category", fields.category);
    if (fields.component !== undefined) {
      scope.setTag("component", fields.component);
    }
    if (fields.surface !== undefined) {
      scope.setTag("surface", fields.surface);
    }
    Sentry.captureException(error instanceof Error ? error : new Error(name));
  });
}
