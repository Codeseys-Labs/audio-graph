/**
 * Convert an error from a rejected `invoke(...)` call into a user-facing
 * message string.
 *
 * Pilot for loop10 MEDIUM #8: commands that return `Result<_, AppError>` on
 * the Rust side reject with a structured `AppErrorPayload` (see
 * `types/index.ts`). Commands that still return `Result<_, String>` reject
 * with a bare string. This helper handles both, returning a readable
 * message either way so callers (toast, panels, hooks) don't care which
 * shape they got.
 *
 * Future: once all commands are migrated, this helper is the one place the
 * frontend needs to change to localize error codes via `i18n` — each `code`
 * maps to a translation key.
 */
import type { AppErrorPayload } from "../types";

/**
 * Narrow an unknown value to an `AppErrorPayload` if it has the shape
 * serde emits (`{ code: string, message?: ... }`).
 */
function isAppErrorPayload(e: unknown): e is AppErrorPayload {
    if (typeof e !== "object" || e === null) return false;
    const obj = e as Record<string, unknown>;
    return typeof obj.code === "string";
}

/**
 * Format an `AppErrorPayload` as a user-friendly string. Central home for
 * the human-readable copy — swap these literals for i18n keys in a later
 * loop.
 */
function formatAppError(err: AppErrorPayload): string {
    switch (err.code) {
        case "io":
            return `I/O error: ${err.message}`;
        case "credential_missing":
            return `Missing credential: ${err.message.key}. Open Settings to configure it.`;
        case "credential_file_error":
            return `Could not save credential: ${err.message.reason}`;
        case "aws_credential_expired":
            return "AWS credentials have expired. Please refresh them.";
        case "aws_region_invalid":
            return `Invalid AWS region: "${err.message.region}". Open Settings to pick a valid region.`;
        case "gemini_rate_limited":
            return "Gemini API rate limit exceeded. Please wait a moment and try again.";
        case "model_not_found":
            return `Model "${err.message.name}" is not available. Download it in Settings.`;
        case "session_invalid":
            return `Invalid session state: ${err.message.reason}`;
        case "network_timeout":
            return `Network timeout calling ${err.message.service}. Check your connection and retry.`;
        case "unknown":
            return err.message;
    }
}

/**
 * Convert a thrown error from `invoke(...)` (or any async rejection) into
 * a string safe to display to the user. Falls back to `String(e)` when the
 * value doesn't match the structured shape, which covers:
 *   - commands still returning `Result<_, String>` (pre-migration)
 *   - native JS exceptions (`Error`, thrown strings)
 *   - anything else (objects, undefined, numbers)
 */
export function errorToMessage(e: unknown): string {
    if (isAppErrorPayload(e)) {
        return formatAppError(e);
    }
    if (e instanceof Error) {
        return e.message;
    }
    return String(e);
}
