import { describe, expect, it } from "vitest";
import { errorToMessage } from "./errorToMessage";

describe("errorToMessage", () => {
  it("formats a credential_missing AppError with the key in the message", () => {
    // Matches the JSON shape from the Rust backend:
    //   { "code": "credential_missing", "message": { "key": "aws_secret_key" } }
    const err = {
      code: "credential_missing",
      message: { key: "aws_secret_key" },
    };
    const msg = errorToMessage(err);
    expect(msg).toContain("aws_secret_key");
    expect(msg.toLowerCase()).toContain("credential");
  });

  it("formats an aws_region_invalid AppError with the offending region", () => {
    const err = {
      code: "aws_region_invalid",
      message: { region: "xx-fake-1" },
    };
    const msg = errorToMessage(err);
    expect(msg).toContain("xx-fake-1");
    expect(msg.toLowerCase()).toContain("region");
  });

  it("formats a unit variant (aws_credential_expired) even without a message field", () => {
    // Unit variants serialize as just `{ "code": "aws_credential_expired" }`
    // because serde omits the content key entirely.
    const err = { code: "aws_credential_expired" };
    const msg = errorToMessage(err);
    expect(msg.toLowerCase()).toContain("aws");
    expect(msg.toLowerCase()).toContain("expired");
  });

  it("formats unknown AppError payloads without exposing the wrapper", () => {
    expect(
      errorToMessage({
        code: "unknown",
        message: "Storage still unavailable",
      }),
    ).toBe("Storage still unavailable");
  });

  it("falls back to String(e) for legacy bare-string rejections", () => {
    // Older invoke surfaces or JS code may still reject with a plain
    // string. Must still produce a readable message.
    expect(errorToMessage("boom")).toBe("boom");
    expect(errorToMessage(new Error("kaboom"))).toBe("kaboom");
  });
});
