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
 * `beforeSend` scrubber ([`scrubEvent`]) enforces the equivalent contract —
 * after it runs, NO free text, file path, source snippet, local variable, or
 * identity survives, only the allowlisted structured tags plus the exception
 * type and basenamed stack-frame filenames:
 *
 *   - drop `event.message`, `event.request`, `event.user`, `event.breadcrumbs`,
 *     and `event.extra` (all of which can carry free prose / PII);
 *   - null the other free-prose identifiers the browser SDK populates:
 *     `event.logentry` (interpolated message + params), `event.transaction`,
 *     `event.server_name`, and `event.fingerprint` (a custom fingerprint can
 *     encode prose);
 *   - scrub EVERY stack frame — across `exception.values[].stacktrace`, every
 *     `threads[].stacktrace`, and the deprecated top-level `event.stacktrace` —
 *     down to basename `filename`/`abs_path` with `context_line`,
 *     `pre_context`, `post_context`, and `vars` cleared (`captureException`
 *     always attaches a stacktrace, so unscrubbed frames would otherwise ship
 *     file URLs and possibly source/locals on every error);
 *   - keep only the non-identifying `contexts` allowlist (`os`, `device`,
 *     `runtime` — mirroring the backend's os/device/rust/runtime intent); and
 *   - keep ONLY the allowlisted structured tags
 *     ({@link ALLOWLISTED_TAG_KEYS}: `event.name`, `category`, `component`,
 *     `surface`), each value shape-checked against a strict id pattern and
 *     otherwise dropped.
 *
 * Because callers only ever set structured, controlled tags via
 * [`captureFrontendError`], and `beforeSend` nulls the message and strips
 * everything else, an error object's message (which can interpolate transcript
 * text, credentials, or paths) can never be transmitted.
 *
 * Gating: [`initSentry`]`(false)` is a no-op — the SDK is never initialised, so
 * nothing is ever sent. When analytics is off, all capture helpers are inert.
 */

import type {
  Event,
  ErrorEvent as SentryErrorEvent,
  StackFrame,
} from "@sentry/browser";
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
 *
 * `event.name` is the primary triage id (a stable, controlled id such as
 * `"asr.stream.error"` set by [`captureFrontendError`]); it is shape-validated
 * with the same {@link SAFE_ID} pattern as every other allowlisted value, so a
 * caller cannot smuggle free prose through it.
 */
export const ALLOWLISTED_TAG_KEYS = [
  "event.name",
  "category",
  "component",
  "surface",
] as const;

/**
 * The ONLY `contexts` keys allowed to survive [`scrubEvent`]. Mirrors the
 * backend's non-identifying context allowlist (`os` / `device` / `rust` /
 * `runtime`) — the browser SDK's equivalents are `os`, `device`, and
 * `runtime`. Every other context (e.g. `app`, `culture`, `trace`, `state`,
 * `response`, or anything a caller attached) is dropped.
 */
export const ALLOWLISTED_CONTEXT_KEYS: readonly string[] = [
  "os",
  "device",
  "runtime",
];

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
 * Reduce a filesystem path to its last segment (basename), so absolute
 * developer/build paths — which can embed usernames or private directory
 * structure — never leave the machine. Mirrors the backend `basename`: split
 * on both POSIX and Windows separators and keep the final component.
 */
function basename(path: string): string {
  const segments = path.split(/[/\\]/);
  return segments[segments.length - 1] ?? path;
}

/**
 * Scrub a slice of stack frames in place so they carry no private data:
 * basename `filename`/`abs_path` and clear every free-text source field —
 * `context_line`, `pre_context`, `post_context` (source snippets), and `vars`
 * (local variable captures can hold transcript text or credentials). Mirrors
 * the backend `scrub_frames`, applied uniformly to exception, thread, and the
 * deprecated top-level stacktrace frames so no frame source escapes.
 */
function scrubFrames(frames: StackFrame[] | undefined): void {
  if (!frames) return;
  for (const frame of frames) {
    if (typeof frame.abs_path === "string") {
      frame.abs_path = basename(frame.abs_path);
    }
    if (typeof frame.filename === "string") {
      frame.filename = basename(frame.filename);
    }
    // Source/locals can carry transcript text or credentials — drop them.
    frame.context_line = undefined;
    frame.pre_context = undefined;
    frame.post_context = undefined;
    frame.vars = undefined;
  }
}

/**
 * `beforeSend` scrubber: the frontend privacy chokepoint. Strips every free-text
 * / PII-bearing field and keeps only the allowlisted, shape-valid structured
 * tags. Reaches parity with the backend `scrub_event`: after it runs, NO free
 * text, file path, source snippet, local variable, or identity survives — only
 * the allowlisted structured tags, the exception `type`, and basenamed frame
 * filenames. Exported for direct unit testing.
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

  // Other free-prose / identity fields the browser SDK populates. `logentry`
  // carries an interpolated message + positional params; `transaction` and
  // `server_name` are free-text identifiers; a custom `fingerprint` can encode
  // prose. Null them wholesale (matching the backend, which reduces these to
  // sentinels and drops all remaining prose).
  event.logentry = undefined;
  event.transaction = undefined;
  event.server_name = undefined;
  event.fingerprint = undefined;

  // Exception values are free prose too — drop each value (keep the type for
  // triage, matching the backend which retains the exception `type`). Scrub the
  // attached stacktrace frames: `captureException` ALWAYS attaches a
  // stacktrace, so unscrubbed frames would ship file URLs and possibly
  // source/locals on every error.
  if (event.exception?.values) {
    for (const exception of event.exception.values) {
      exception.value = undefined;
      scrubFrames(exception.stacktrace?.frames);
    }
  }

  // Scrub the deprecated top-level `stacktrace` frames the same way. The
  // browser `Event` type omits this legacy field (unlike the Rust protocol),
  // but the wire format still permits it and a manually-constructed event can
  // carry it — read it defensively so no frame source escapes if present.
  const topStacktrace = (event as { stacktrace?: { frames?: StackFrame[] } })
    .stacktrace;
  scrubFrames(topStacktrace?.frames);

  // Scrub every thread's stacktrace frames (NOT covered by the exception loop;
  // these can ship absolute paths + locals independently).
  if (event.threads?.values) {
    for (const thread of event.threads.values) {
      scrubFrames(thread.stacktrace?.frames);
    }
  }

  // Keep only safe, non-identifying contexts (os / device / runtime); drop all
  // others (e.g. `app`, `culture`, `trace`, `state`, or anything a caller
  // attached that could carry prose).
  if (event.contexts) {
    const keptContexts: NonNullable<Event["contexts"]> = {};
    for (const key of ALLOWLISTED_CONTEXT_KEYS) {
      const value = event.contexts[key];
      if (value !== undefined) {
        keptContexts[key] = value;
      }
    }
    event.contexts = keptContexts;
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
