/**
 * `safeInvoke` — the analytics-capturing IPC chokepoint (audio-graph-3e71).
 *
 * These tests pin the three load-bearing guarantees the codebase relies on when
 * it aliases `safeInvoke` in as a drop-in for the Tauri `invoke`:
 *
 *   1. **Passthrough** — on success it returns the backend value unchanged and
 *      never touches analytics (telemetry is a failure-only signal).
 *   2. **Report-once + rethrow** — on failure it relays EXACTLY ONE diagnostic
 *      (so a caller's own catch does not double-report) then rethrows the
 *      ORIGINAL error unchanged, so each call site's existing
 *      catch → `errorToMessage` humanization is untouched.
 *   3. **Privacy (ADR-0023)** — the diagnostic carries only the command NAME
 *      (as `component`); the `args`/payload and the error's message/stack are
 *      NEVER serialized into the event.
 *
 * It also pins the arity contract: `safeInvoke` forwards the caller's exact
 * positional arity to `invoke` (no trailing `undefined`), so the arity-sensitive
 * `toHaveBeenCalledWith("cmd", args)` assertions at migrated call sites keep
 * matching.
 */

import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { safeInvoke } from "./safeInvoke";
import * as sentry from "./sentry";

const mockedInvoke = vi.mocked(invoke);

describe("safeInvoke", () => {
  afterEach(() => {
    mockedInvoke.mockReset();
    vi.restoreAllMocks();
  });

  it("returns the backend value on success and never reports to analytics", async () => {
    const capture = vi.spyOn(sentry, "captureFrontendError");
    mockedInvoke.mockResolvedValueOnce(["a", "b"]);

    const result = await safeInvoke<string[]>("list_audio_sources");

    expect(result).toEqual(["a", "b"]);
    // Success is not a telemetry signal — nothing is captured.
    expect(capture).not.toHaveBeenCalled();
  });

  it("captures EXACTLY ONE diagnostic tagged with the command name, then rethrows the original error", async () => {
    const capture = vi.spyOn(sentry, "captureFrontendError");
    const original = new Error("backend exploded");
    mockedInvoke.mockRejectedValueOnce(original);

    await expect(
      safeInvoke("save_settings_cmd", { settings: {} }),
    ).rejects.toBe(original);

    // Reported once (a caller's own catch does not cause a second report).
    expect(capture).toHaveBeenCalledTimes(1);
    const [name, fields, ...rest] = capture.mock.calls[0];
    expect(name).toBe("frontend.invoke.error");
    expect(fields).toEqual({
      category: "frontend",
      surface: "invoke",
      component: "save_settings_cmd",
    });
    // No extra positional argument carrying the error object / args.
    expect(rest).toEqual([]);
  });

  it("NEVER serializes the invoke args or the error into the diagnostic (ADR-0023)", async () => {
    const capture = vi.spyOn(sentry, "captureFrontendError");

    const SECRET = "sk-supersecret-key-1234567890";
    const TRANSCRIPT = "patient said their SSN aloud";
    const sensitiveArgs = { key: "openai_api_key", value: SECRET, TRANSCRIPT };
    const error = new Error(`boom: ${SECRET} / ${TRANSCRIPT}`);
    error.stack = `Error: ${SECRET}\n    at /Users/alice/secret/x.ts:1:1`;
    mockedInvoke.mockRejectedValueOnce(error);

    await expect(safeInvoke("save_credential_cmd", sensitiveArgs)).rejects.toBe(
      error,
    );

    expect(capture).toHaveBeenCalledTimes(1);
    // The whole captured payload (name + fields) is serialized and asserted to
    // contain no secret, transcript value, arg key, or error prose/stack.
    const json = JSON.stringify(capture.mock.calls[0]);
    expect(json).not.toContain(SECRET);
    expect(json).not.toContain(TRANSCRIPT);
    expect(json).not.toContain("openai_api_key");
    expect(json).not.toContain("boom");
    expect(json).not.toContain("/Users/alice");
    // Only the command name rides along, as `component`.
    expect(json).toContain("save_credential_cmd");
  });

  it("forwards the caller's exact positional arity to invoke (no trailing undefined)", async () => {
    // Arity-sensitivity contract: a trailing `undefined` would be recorded as an
    // extra positional arg and break call sites' `toHaveBeenCalledWith` matches.
    mockedInvoke.mockResolvedValue(undefined);

    await safeInvoke("get_session_id");
    await safeInvoke("delete_session", { sessionId: "s1" });

    const oneArg = mockedInvoke.mock.calls.find(
      (c) => c[0] === "get_session_id",
    );
    const twoArg = mockedInvoke.mock.calls.find(
      (c) => c[0] === "delete_session",
    );
    // No trailing args beyond what the caller supplied.
    expect(oneArg).toHaveLength(1);
    expect(twoArg).toHaveLength(2);
    // And the 2-arg call matches an arity-sensitive assertion exactly.
    expect(mockedInvoke).toHaveBeenCalledWith("delete_session", {
      sessionId: "s1",
    });
  });

  it("does not clobber the caller's error when the analytics relay itself fails", async () => {
    // Regression guard: `captureFrontendError` must fail silent. If the relay's
    // own IPC throws (e.g. a mock returning a non-thenable), the ORIGINAL
    // command error must still be what surfaces to the caller — not the relay's.
    const original = new Error("fold blew up");
    // First call (the real command) rejects; the second call is the relay's
    // `report_frontend_diagnostic`, which returns undefined (non-thenable) —
    // the pre-fix bug turned this into "Cannot read properties of undefined".
    mockedInvoke
      .mockRejectedValueOnce(original)
      .mockReturnValueOnce(undefined as never);

    await expect(safeInvoke("loadSessionTimeline")).rejects.toBe(original);
  });
});
