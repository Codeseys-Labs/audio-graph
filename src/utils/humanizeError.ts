/**
 * Error-humanization layer (ADR-0011, review item A2 / seed 5c24).
 *
 * `errorToMessage` turns a rejected `invoke(...)` into a *string*, but that
 * string is often still developer-facing: a raw JS `TypeError`
 * ("Cannot read properties of undefined (reading 'invoke')") when the Tauri
 * IPC bridge is absent, or a bare technical fallback. Rendering those verbatim
 * on a user surface — a sticky `role=alert` banner, or the Analysis projection
 * diagnostics panel — is the defect this module fixes.
 *
 * This is a *display-layer* mapper: it classifies a message string into a
 * known failure shape and returns plain-language copy keys plus behavior hints
 * (severity, transient auto-dismiss, retryable). The original string is always
 * preserved on `raw` so a collapsed "Details" affordance can reveal it for
 * debugging.
 *
 * Deliberately conservative: strings that are already user-facing prose (the
 * friendly output `errorToMessage` produces for structured `AppErrorPayload`
 * codes) are passed through verbatim rather than clobbered into a generic
 * "Something went wrong". Only a recognizably *technical* unknown gets the
 * generic treatment.
 */

export type ErrorClass =
  | "ipc_unavailable"
  | "command_not_found"
  | "network"
  | "auth"
  | "rate_limit"
  | "unknown";

export interface HumanizedError {
  /** Which known failure shape the raw message matched. */
  kind: ErrorClass;
  /**
   * i18n key for the plain-language title, or `null` when the message is
   * already user-facing prose and should be shown verbatim (`title`).
   */
  titleKey: string | null;
  /** Verbatim title, set only when `titleKey` is `null` (passthrough). */
  title: string | null;
  /** i18n key for the cause/explanation line, or `null` for passthrough. */
  causeKey: string | null;
  /** Effective severity — transient probe noise is a warning, not an error. */
  severity: "error" | "warning";
  /** Transient/startup errors auto-expire instead of sticking. */
  transient: boolean;
  /** Whether a Retry affordance is meaningful for this class. */
  retryable: boolean;
  /** The original developer-facing string, preserved for a Details reveal. */
  raw: string;
}

// The flagship offender: the Tauri IPC bridge is undefined (browser preview or
// pre-`__TAURI_INTERNALS__` startup), so `invoke` reads off `undefined`.
const IPC_UNAVAILABLE =
  /cannot read propert.*\binvoke\b|\binvoke\b.*(?:is not a function|of undefined|of null)|window\.__tauri|__tauri_internals__|__tauri_ipc__/i;

// A command the frontend called isn't registered / allow-listed by the shell.
const COMMAND_NOT_FOUND =
  /command\s+\S+\s+not found|not found in the allowlist|not allowed by the acl|is not allowed on the configured|unknown command/i;

const RATE_LIMIT = /\b429\b|rate.?limit|too many requests|quota exceeded/i;

const AUTH =
  /\b401\b|\b403\b|unauthorized|forbidden|invalid api key|rejected the api key|authentication failed|access denied|missing credential/i;

const NETWORK =
  /network|timeout|timed out|failed to fetch|econnrefused|enotfound|\bdns\b|offline|unreachable|connection (?:refused|reset|error)|could not connect/i;

// A recognizably *technical* string — a raw exception rather than user prose.
// Used to decide whether an unclassified message earns the generic title or is
// passed through verbatim (so we never regress an already-friendly message).
const TECHNICAL =
  /\b(?:TypeError|ReferenceError|SyntaxError|RangeError|EvalError|URIError)\b|is not a function|is not iterable|cannot read propert|(?:undefined|null) is not|\[object \w+\]|uncaught|unwrap\(\)|panicked at/i;

interface ClassMeta {
  titleKey: string;
  causeKey: string;
  severity: "error" | "warning";
  transient: boolean;
  retryable: boolean;
}

const CLASS_META: Record<Exclude<ErrorClass, "unknown">, ClassMeta> = {
  ipc_unavailable: {
    titleKey: "errors.ipcUnavailable.title",
    causeKey: "errors.ipcUnavailable.cause",
    severity: "warning",
    transient: true,
    retryable: true,
  },
  command_not_found: {
    titleKey: "errors.commandNotFound.title",
    causeKey: "errors.commandNotFound.cause",
    severity: "error",
    transient: false,
    retryable: false,
  },
  network: {
    titleKey: "errors.network.title",
    causeKey: "errors.network.cause",
    severity: "warning",
    transient: true,
    retryable: true,
  },
  auth: {
    titleKey: "errors.auth.title",
    causeKey: "errors.auth.cause",
    severity: "error",
    transient: false,
    retryable: false,
  },
  rate_limit: {
    titleKey: "errors.rateLimit.title",
    causeKey: "errors.rateLimit.cause",
    severity: "warning",
    transient: false,
    retryable: true,
  },
};

/**
 * Classify a raw error message string into a known failure shape. Order is
 * significant — the most specific / highest-signal patterns win first.
 */
export function classifyError(raw: string): ErrorClass {
  if (IPC_UNAVAILABLE.test(raw)) return "ipc_unavailable";
  if (COMMAND_NOT_FOUND.test(raw)) return "command_not_found";
  if (RATE_LIMIT.test(raw)) return "rate_limit";
  if (AUTH.test(raw)) return "auth";
  if (NETWORK.test(raw)) return "network";
  return "unknown";
}

/**
 * Map a raw error message into plain-language copy keys + behavior hints. The
 * original string is always retained on `raw` for a Details reveal.
 */
export function humanizeError(raw: string): HumanizedError {
  const message = raw.trim();
  const kind = classifyError(message);

  if (kind !== "unknown") {
    const meta = CLASS_META[kind];
    return {
      kind,
      titleKey: meta.titleKey,
      title: null,
      causeKey: meta.causeKey,
      severity: meta.severity,
      transient: meta.transient,
      retryable: meta.retryable,
      raw: message,
    };
  }

  // Unknown: only a recognizably technical string earns the generic title +
  // Details. Already-friendly prose (structured `errorToMessage` output that
  // didn't match a bucket) is passed through verbatim so we never downgrade a
  // good message to "Something went wrong".
  if (TECHNICAL.test(message) || message.length === 0) {
    return {
      kind: "unknown",
      titleKey: "errors.unknown.title",
      title: null,
      causeKey: "errors.unknown.cause",
      severity: "error",
      transient: false,
      retryable: false,
      raw: message,
    };
  }

  return {
    kind: "unknown",
    titleKey: null,
    title: message,
    causeKey: null,
    severity: "error",
    transient: false,
    retryable: false,
    raw: message,
  };
}
