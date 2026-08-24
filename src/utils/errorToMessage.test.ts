import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../i18n";
import { artifactTooLargeDetails, errorToMessage } from "./errorToMessage";

describe("errorToMessage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

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

  it("formats a provider_unavailable AppError with feature recovery copy", () => {
    const err = {
      code: "provider_unavailable",
      message: {
        provider: "LocalWhisper",
        required_feature: "local-ml or asr-whisper",
      },
    };
    const msg = errorToMessage(err);
    expect(msg).toContain("LocalWhisper");
    expect(msg).toContain("local-ml or asr-whisper");
    expect(msg.toLowerCase()).toContain("cloud provider");
  });

  it("formats a content-free provider_deferred AppError with migration-safe copy", () => {
    const msg = errorToMessage({
      code: "provider_deferred",
      message: {
        provider_id: "asr.local_whisper",
        display_name: "Local Whisper",
      },
    });

    expect(msg).toContain("Local Whisper");
    expect(msg).toContain("current MVP");
    expect(msg).toContain("saved settings are unchanged");
    expect(msg).not.toContain("endpoint");
  });

  it("localizes provider_deferred without adding route or credential content", async () => {
    await i18n.changeLanguage("pt");

    const msg = errorToMessage({
      code: "provider_deferred",
      message: {
        provider_id: "asr.local_whisper",
        display_name: "Local Whisper",
      },
    });

    expect(msg).toContain("Local Whisper");
    expect(msg).toContain("não está disponível");
    expect(msg).toContain("configurações salvas");
    expect(msg).not.toContain("asr.local_whisper");
    expect(msg).not.toContain("endpoint");
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

  it("formats an artifact_too_large AppError with the size and ceiling in MB", () => {
    // Matches the JSON shape from the Rust backend (seed audio-graph-4fa5):
    //   { "code": "artifact_too_large", "message": { "artifact_class": ...,
    //     "size_bytes": ..., "ceiling_bytes": ... } }
    const msg = errorToMessage({
      code: "artifact_too_large",
      message: {
        artifact_class: "materialized_graph",
        size_bytes: 156_579_416,
        ceiling_bytes: 24 * 1024 * 1024,
      },
    });
    expect(msg).toContain("149.3");
    expect(msg).toContain("24.0");
  });

  it("artifactTooLargeDetails narrows an artifact_too_large payload to its fields", () => {
    const details = artifactTooLargeDetails({
      code: "artifact_too_large",
      message: {
        artifact_class: "materialized_notes",
        size_bytes: 19_063_321,
        ceiling_bytes: 8 * 1024 * 1024,
      },
    });
    expect(details).toEqual({
      artifactClass: "materialized_notes",
      sizeBytes: 19_063_321,
      ceilingBytes: 8 * 1024 * 1024,
    });
  });

  it("artifactTooLargeDetails returns null for any other error shape", () => {
    expect(
      artifactTooLargeDetails({
        code: "session_invalid",
        message: { reason: "not found" },
      }),
    ).toBeNull();
    expect(artifactTooLargeDetails(new Error("boom"))).toBeNull();
    expect(artifactTooLargeDetails("boom")).toBeNull();
  });

  it("falls back to String(e) for legacy bare-string rejections", () => {
    // Older invoke surfaces or JS code may still reject with a plain
    // string. Must still produce a readable message.
    expect(errorToMessage("boom")).toBe("boom");
    expect(errorToMessage(new Error("kaboom"))).toBe("kaboom");
  });
});
