import { describe, expect, it } from "vitest";
import { classifyError, humanizeError } from "./humanizeError";

describe("classifyError", () => {
  it("classifies the Tauri IPC-bridge TypeError as ipc_unavailable", () => {
    expect(
      classifyError("Cannot read properties of undefined (reading 'invoke')"),
    ).toBe("ipc_unavailable");
    expect(classifyError("invoke is not a function")).toBe("ipc_unavailable");
    expect(
      classifyError("Cannot read properties of null (reading 'invoke')"),
    ).toBe("ipc_unavailable");
  });

  it("classifies an unregistered command as command_not_found", () => {
    expect(classifyError("Command foo_cmd not found")).toBe(
      "command_not_found",
    );
    expect(
      classifyError("bar_cmd not allowed by the ACL for this window"),
    ).toBe("command_not_found");
  });

  it("classifies rate-limit shapes as rate_limit", () => {
    expect(classifyError("HTTP 429 Too Many Requests")).toBe("rate_limit");
    expect(classifyError("Gemini quota exceeded")).toBe("rate_limit");
  });

  it("classifies auth shapes as auth", () => {
    expect(classifyError("401 Unauthorized")).toBe("auth");
    expect(classifyError("provider returned 403 Forbidden")).toBe("auth");
    expect(classifyError("Invalid API key")).toBe("auth");
  });

  it("classifies network/timeout shapes as network", () => {
    expect(classifyError("network timeout calling Deepgram")).toBe("network");
    expect(classifyError("Failed to fetch")).toBe("network");
    expect(classifyError("ECONNREFUSED 127.0.0.1:443")).toBe("network");
  });

  it("returns unknown for anything unmatched", () => {
    expect(classifyError("some brand-new failure")).toBe("unknown");
  });
});

describe("humanizeError", () => {
  it("maps a known IPC shape to friendly copy keys and marks it transient", () => {
    const h = humanizeError(
      "Cannot read properties of undefined (reading 'invoke')",
    );
    expect(h.kind).toBe("ipc_unavailable");
    expect(h.titleKey).toBe("errors.ipcUnavailable.title");
    expect(h.causeKey).toBe("errors.ipcUnavailable.cause");
    expect(h.severity).toBe("warning");
    expect(h.transient).toBe(true);
    // Raw string is always retained for a Details reveal.
    expect(h.raw).toContain("invoke");
  });

  it("gives a recognizably technical unknown the generic title + Details", () => {
    const h = humanizeError("TypeError: x.y is not a function");
    expect(h.kind).toBe("unknown");
    expect(h.titleKey).toBe("errors.unknown.title");
    expect(h.causeKey).toBe("errors.unknown.cause");
    expect(h.severity).toBe("error");
    expect(h.transient).toBe(false);
    expect(h.raw).toContain("TypeError");
  });

  it("passes an already-friendly unknown message through verbatim", () => {
    // Structured `errorToMessage` output that didn't match a bucket must not
    // be clobbered into "Something went wrong".
    const friendly =
      "Model “whisper” is not available. Download it in Settings.";
    const h = humanizeError(friendly);
    expect(h.kind).toBe("unknown");
    expect(h.titleKey).toBeNull();
    expect(h.title).toBe(friendly);
    expect(h.causeKey).toBeNull();
  });

  it("gives an empty string the generic treatment (never a blank banner)", () => {
    const h = humanizeError("");
    expect(h.titleKey).toBe("errors.unknown.title");
  });

  it("marks network as transient+retryable and auth as sticky", () => {
    expect(humanizeError("Failed to fetch").transient).toBe(true);
    expect(humanizeError("Failed to fetch").retryable).toBe(true);
    expect(humanizeError("401 Unauthorized").transient).toBe(false);
    expect(humanizeError("401 Unauthorized").retryable).toBe(false);
  });
});
