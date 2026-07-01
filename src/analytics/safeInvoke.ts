/**
 * `safeInvoke` — a thin wrapper around the Tauri `invoke` that reports command
 * failures to the frontend diagnostics channel and rethrows.
 *
 * Every command reaching the frontend can surface an error; wrapping the call
 * here captures a structured diagnostic (category `frontend`, surface `invoke`,
 * name = the command) without per-call-site instrumentation, then rethrows so
 * the caller's existing error handling (toasts, panels) is unchanged.
 *
 * The captured event carries no free text — [`captureFrontendError`] attaches
 * only controlled ids and the `beforeSend` scrubber strips everything else.
 */

import type { InvokeArgs, InvokeOptions } from "@tauri-apps/api/core";
import { invoke } from "@tauri-apps/api/core";
import { captureFrontendError } from "./sentry";

/**
 * Invoke a Tauri command; on failure, capture a `frontend`/`invoke` diagnostic
 * tagged with the command name and rethrow the original error unchanged.
 */
export async function safeInvoke<T>(
  cmd: string,
  args?: InvokeArgs,
  options?: InvokeOptions,
): Promise<T> {
  try {
    return await invoke<T>(cmd, args, options);
  } catch (error) {
    captureFrontendError(
      cmd,
      { category: "frontend", surface: "invoke" },
      error,
    );
    throw error;
  }
}
