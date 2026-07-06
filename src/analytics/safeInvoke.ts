/**
 * `safeInvoke` — a thin wrapper around the Tauri `invoke` that relays command
 * failures to the backend diagnostics channel and rethrows.
 *
 * Every command reaching the frontend can surface an error; wrapping the call
 * here captures a structured diagnostic (category `frontend`, surface `invoke`,
 * component = the command name) without per-call-site instrumentation, then
 * rethrows so the caller's existing error handling (toasts, panels) is
 * unchanged.
 *
 * The captured event carries no free text — [`captureFrontendError`] forwards
 * only controlled ids to `report_frontend_diagnostic`, and the caught error is
 * never forwarded, so its message/stack never leave the renderer.
 */

import type { InvokeArgs, InvokeOptions } from "@tauri-apps/api/core";
import { invoke } from "@tauri-apps/api/core";
import { captureFrontendError } from "./sentry";

/**
 * Invoke a Tauri command; on failure, relay a `frontend`/`invoke` diagnostic
 * tagged with the command name (as `component`) and rethrow the original error
 * unchanged.
 *
 * This is a drop-in replacement for `@tauri-apps/api/core`'s `invoke`: modules
 * adopt it by aliasing the import (`import { safeInvoke as invoke }`), so the
 * call sites themselves are unchanged. To stay a faithful drop-in we forward
 * the caller's EXACT arity — appending a trailing `undefined` would be recorded
 * as an extra positional argument by a test's `invoke` mock and break the
 * arity-sensitive `toHaveBeenCalledWith("cmd", args)` assertions those call
 * sites rely on. Only the command NAME (never `args`) is attached to the
 * diagnostic, upholding the ADR-0023 privacy invariant.
 */
export async function safeInvoke<T>(
  cmd: string,
  args?: InvokeArgs,
  options?: InvokeOptions,
): Promise<T> {
  try {
    // Preserve the caller's positional arity so this reads identically to a
    // direct `invoke` call at the boundary (see doc above).
    if (options !== undefined) return await invoke<T>(cmd, args, options);
    if (args !== undefined) return await invoke<T>(cmd, args);
    return await invoke<T>(cmd);
  } catch (error) {
    captureFrontendError("frontend.invoke.error", {
      category: "frontend",
      surface: "invoke",
      component: cmd,
    });
    throw error;
  }
}
