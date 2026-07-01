/**
 * Frontend diagnostics relay (`captureFrontendError`).
 *
 * The frontend no longer runs its own Sentry SDK — it relays controlled ids to
 * the backend `report_frontend_diagnostic` command, which owns the transport
 * and the scrubber. These tests pin the load-bearing privacy invariant: the
 * relay forwards ONLY the controlled id fields (`name`, `category`,
 * `component`, `surface`) and NEVER an error's message/stack or any free text,
 * and it fails silent so telemetry can never throw into the caller.
 */

import { invoke } from "@tauri-apps/api/core";
import { afterEach, describe, expect, it, vi } from "vitest";
import { captureFrontendError } from "./sentry";

const mockedInvoke = vi.mocked(invoke);

describe("captureFrontendError (backend relay)", () => {
  afterEach(() => {
    mockedInvoke.mockReset();
  });

  it("relays to report_frontend_diagnostic with ONLY the controlled fields", () => {
    mockedInvoke.mockResolvedValueOnce(undefined);

    captureFrontendError("frontend.invoke.error", {
      category: "frontend",
      component: "list_audio_sources",
      surface: "invoke",
    });

    expect(mockedInvoke).toHaveBeenCalledTimes(1);
    const [command, args] = mockedInvoke.mock.calls[0];
    expect(command).toBe("report_frontend_diagnostic");
    // Exactly the controlled id fields — nothing else rides along.
    expect(args).toEqual({
      name: "frontend.invoke.error",
      category: "frontend",
      component: "list_audio_sources",
      surface: "invoke",
    });
  });

  it("passes null (never free text) for omitted optional fields", () => {
    mockedInvoke.mockResolvedValueOnce(undefined);

    captureFrontendError("frontend.window.error", {
      category: "frontend",
      surface: "window",
    });

    const [, args] = mockedInvoke.mock.calls[0];
    expect(args).toEqual({
      name: "frontend.window.error",
      category: "frontend",
      component: null,
      surface: "window",
    });
  });

  it("NEVER forwards the error object's message or stack — no free text leaks", () => {
    mockedInvoke.mockResolvedValueOnce(undefined);

    const SECRET = "sk-supersecret-transcript-and-key-1234567890";
    const TRANSCRIPT = "patient said their social security number aloud";
    const error = new Error(`boom: ${SECRET} / ${TRANSCRIPT}`);
    error.stack = `Error: ${SECRET}\n    at /Users/alice/secret/x.ts:1:1`;

    // The error is passed for call-site ergonomics but must never be forwarded.
    captureFrontendError(
      "frontend.react.render",
      { category: "frontend", component: "root-boundary" },
      error,
    );

    expect(mockedInvoke).toHaveBeenCalledTimes(1);
    const [, args] = mockedInvoke.mock.calls[0];

    // Only the controlled id fields are present.
    expect(args).toEqual({
      name: "frontend.react.render",
      category: "frontend",
      component: "root-boundary",
      surface: null,
    });

    // The whole serialized payload contains no secret, transcript, stack, or
    // message — proof the error's prose never reaches the wire.
    const json = JSON.stringify(args);
    expect(json).not.toContain(SECRET);
    expect(json).not.toContain(TRANSCRIPT);
    expect(json).not.toContain("boom");
    expect(json).not.toContain("/Users/alice");
    expect(json).not.toContain("at ");
  });

  it("fails silent when the backend command rejects (never throws)", async () => {
    // A rejected relay must not surface to the caller. Capture the rejected
    // promise the module swallows so the run has no unhandled rejection.
    let relayed: Promise<unknown> | undefined;
    mockedInvoke.mockImplementationOnce(() => {
      relayed = Promise.reject(new Error("ipc unavailable"));
      return relayed;
    });

    expect(() =>
      captureFrontendError("frontend.window.error", {
        category: "frontend",
        surface: "window",
      }),
    ).not.toThrow();

    // Awaiting the swallowed promise must also not reject out of the module's
    // `.catch`.
    await expect(relayed?.catch(() => "handled")).resolves.toBe("handled");
  });
});
