/**
 * Convert an error from a rejected `invoke(...)` call into a user-facing
 * message string.
 *
 * Commands that return `Result<_, AppError>` on the Rust side reject with a
 * structured `AppErrorPayload` (see `types/index.ts`). Older bare-string
 * rejections can still be handled here as a fallback, so callers (toast,
 * panels, hooks) don't care which shape they got.
 *
 * Provider-policy errors already resolve through the bundled i18n resources;
 * remaining legacy codes can migrate here without changing callers.
 */
import i18n from "../i18n";
import {
  type AppErrorPayload,
  CAPTURE_FAILURE_REASONS,
  type NotifyOptions,
} from "../types";
import { runtimeDiagnosticPresentation } from "./runtimeDiagnostic";

/**
 * Narrow an unknown value to an `AppErrorPayload` if it has the shape
 * serde emits (`{ code: string, message?: ... }`).
 */
export function isAppErrorPayload(e: unknown): e is AppErrorPayload {
  if (typeof e !== "object" || e === null) return false;
  const obj = e as Record<string, unknown>;
  return typeof obj.code === "string";
}

/**
 * Narrow a rejected `invoke(...)` error to the `artifact_too_large` variant's
 * fields (seed audio-graph-4fa5 deliverable b), or `null` when it isn't one.
 * The Notes/Graph lens fetch actions (`store/index.ts`) use this to set
 * `notesLensStatus`/`graphLensStatus` to `refused` instead of the generic
 * `error` state, so `SessionsBrowser` can render the typed refusal notice
 * (artifact class + size + ceiling) rather than a plain error string.
 */
export function artifactTooLargeDetails(
  e: unknown,
): { artifactClass: string; sizeBytes: number; ceilingBytes: number } | null {
  if (!isAppErrorPayload(e) || e.code !== "artifact_too_large") return null;
  // Validate at the boundary: this runs inside `catch` handlers, so it must
  // never throw, and a malformed refusal (no message, a class outside the
  // closed set, a number that is not finite and non-negative) is not a typed
  // refusal at all: the caller falls back to its closed error state.
  const message: unknown = e.message;
  if (typeof message !== "object" || message === null) return null;
  const { artifact_class, size_bytes, ceiling_bytes } = message as Record<
    string,
    unknown
  >;
  if (
    typeof artifact_class !== "string" ||
    !(ARTIFACT_REFUSAL_CLASSES as readonly string[]).includes(artifact_class)
  ) {
    return null;
  }
  if (!isByteCount(size_bytes) || !isByteCount(ceiling_bytes)) return null;
  return {
    artifactClass: artifact_class,
    sizeBytes: size_bytes,
    ceilingBytes: ceiling_bytes,
  };
}

/**
 * The closed set of artifact classes the backend's byte ceilings name for the
 * session lenses (`enforce_artifact_ceiling` call sites in `commands.rs`):
 * Notes (`materialized_notes`, `projection_events`), Graph
 * (`materialized_graph`) and Timeline (`transcript_events`,
 * `diarization_events`, `live_graph`). Each has a label in both locales.
 */
export const ARTIFACT_REFUSAL_CLASSES = [
  "materialized_graph",
  "materialized_notes",
  "projection_events",
  "transcript_events",
  "diarization_events",
  "live_graph",
] as const;

/**
 * A real byte count: a safe non-negative integer. Finite but absurd values
 * (1e300, which would render in scientific notation) are not byte counts.
 */
function isByteCount(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) >= 0;
}

/**
 * Format an `AppErrorPayload` as a user-friendly string. This is the central
 * home for human-readable and localized policy copy.
 */
function formatAppError(err: AppErrorPayload): string {
  switch (err.code) {
    case "io":
      return `I/O error: ${err.message}`;
    case "credential_missing":
      return `Missing credential: ${err.message.key}. Open Settings to configure it.`;
    case "aws_credential_expired":
      return "AWS credentials have expired. Please refresh them.";
    case "aws_region_invalid":
      return `Invalid AWS region: "${err.message.region}". Open Settings to pick a valid region.`;
    case "gemini_rate_limited":
      return "Gemini API rate limit exceeded. Please wait a moment and try again.";
    case "model_not_found":
      return `Model "${err.message.name}" is not available. Download it in Settings.`;
    case "provider_unavailable":
      return `${err.message.provider} is not available in this build. Rebuild with ${err.message.required_feature}, or choose a cloud provider in Settings.`;
    case "provider_deferred":
      return i18n.t("errors.providerDeferred", {
        provider: err.message.display_name,
      });
    case "privacy_policy_blocked":
      return `Privacy policy blocked ${err.message.action} for ${err.message.provider}: ${err.message.reason}`;
    case "session_invalid":
      return `Invalid session state: ${err.message.reason}`;
    case "network_timeout":
      return `Network timeout calling ${err.message.service}. Check your connection and retry.`;
    case "artifact_too_large":
      return i18n.t("errors.artifactTooLarge", {
        sizeMb: (err.message.size_bytes / (1024 * 1024)).toFixed(1),
        ceilingMb: (err.message.ceiling_bytes / (1024 * 1024)).toFixed(1),
      });
    case "capture_start_failed":
      return captureStartFailureMessage(err.message?.reason);
    case "capture_stop_failed":
      // Capture has ended when the snapshot says so: nothing to press again.
      return i18n.t(
        err.message?.snapshot?.capture_state?.phase === "idle"
          ? "errors.captureStop.endedIncomplete"
          : "errors.captureStop.incomplete",
      );
    case "transcription_refused":
      // Closed reasons: anything else reads as the capture not running.
      return i18n.t(
        err.message?.reason === "capture_lost"
          ? "errors.transcriptionRefused.capture_lost"
          : "errors.transcriptionRefused.capture_not_running",
      );
    case "unknown":
      return err.message;
  }
  return i18n.t("runtimeDiagnostics.runtime.internal");
}

/** Localized copy for a capture start failure; unknown input fails closed. */
export function captureStartFailureMessage(reason: unknown): string {
  const known = CAPTURE_FAILURE_REASONS.find((value) => value === reason);
  return i18n.t(`errors.captureStart.${known ?? "backend_unavailable"}`);
}

/** A successful write can carry a typed warning when the credential refresh failed. */
export function notifyCredentialWarning(
  warning: unknown,
  notify: (options: NotifyOptions) => string,
) {
  if (warning) {
    notify({
      severity: "warning",
      message: i18n.t(runtimeDiagnosticPresentation(warning).translationKey),
    });
  }
}

/**
 * Convert a thrown error from `invoke(...)` (or any async rejection) into
 * a string safe to display to the user. Falls back to `String(e)` when the
 * value doesn't match the structured shape, which covers:
 *   - older bare-string command rejections
 *   - native JS exceptions (`Error`, thrown strings)
 *   - anything else (objects, undefined, numbers)
 */
export function errorToMessage(e: unknown): string {
  // Never throws, whatever it is given: a refusal-shaped error with no
  // `message`, a hostile getter, an object whose `toString` throws. The
  // fallback is the closed internal-error copy.
  try {
    return formatError(e);
  } catch {
    try {
      return i18n.t("runtimeDiagnostics.runtime.internal");
    } catch {
      return "Unexpected error";
    }
  }
}

function formatError(e: unknown): string {
  if (
    typeof e === "object" &&
    e !== null &&
    "code" in e &&
    e.code === "runtime_diagnostic"
  ) {
    return i18n.t(runtimeDiagnosticPresentation(e).translationKey);
  }
  if (isAppErrorPayload(e)) {
    return formatAppError(e);
  }
  if (e instanceof Error) {
    return e.message;
  }
  return String(e);
}
