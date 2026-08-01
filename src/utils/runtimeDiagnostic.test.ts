import { describe, expect, expectTypeOf, it } from "vitest";
import { CREDENTIAL_CONTRACT } from "../generated/credentialContract";
import {
  RUNTIME_DIAGNOSTIC_CONTRACT,
  type RuntimeDiagnostic,
  type RuntimeDiagnosticAppError,
} from "../generated/runtimeDiagnostic";
import {
  normalizeRuntimeDiagnostic,
  parseRuntimeDiagnostic,
  parseRuntimeDiagnosticAppError,
  type RuntimeDiagnosticTranslationKey,
  runtimeDiagnosticPresentation,
  SAFE_INTERNAL_RUNTIME_DIAGNOSTIC,
} from "./runtimeDiagnostic";

const CANARIES = [
  "sk-secret-runtime-canary-919191",
  "provider native prose canary 828282",
  "response body canary 737373",
  "/home/private-user/provider.json",
  "request-id-canary-646464",
  "exact-length-canary-555551",
] as const;

const runtimeDiagnostic = (code = "timeout") => ({
  kind: "runtime",
  detail: {
    code,
    retryable: true,
    recovery_action: "retry",
    context: {
      operation: "transcription",
      transport: "sdk",
      status_class: "server_error",
      retry_delay: "short",
    },
  },
});

describe("generated runtime diagnostic contract", () => {
  it("exports the exact closed ec13 vocabularies", () => {
    expect(RUNTIME_DIAGNOSTIC_CONTRACT.error_codes).toHaveLength(12);
    expect(RUNTIME_DIAGNOSTIC_CONTRACT.recovery_actions).toHaveLength(8);
    expect(RUNTIME_DIAGNOSTIC_CONTRACT.operations).toHaveLength(7);
    expect(RUNTIME_DIAGNOSTIC_CONTRACT.transports).toEqual([
      "native",
      "http",
      "websocket",
      "sdk",
    ]);
    expect(RUNTIME_DIAGNOSTIC_CONTRACT.status_classes).toEqual([
      "redirect",
      "client_error",
      "server_error",
    ]);
    expect(RUNTIME_DIAGNOSTIC_CONTRACT.retry_delay_buckets).toEqual([
      "immediate",
      "short",
      "medium",
      "long",
    ]);
  });

  it("types the distinct AppError envelope without a message field", () => {
    expectTypeOf<RuntimeDiagnosticAppError>().toMatchTypeOf<{
      code: "runtime_diagnostic";
      diagnostic: RuntimeDiagnostic;
    }>();
    expectTypeOf<RuntimeDiagnosticTranslationKey>().toEqualTypeOf<
      | `runtimeDiagnostics.credential.${(typeof CREDENTIAL_CONTRACT.vocabulary.error_codes)[number]}`
      | `runtimeDiagnostics.runtime.${(typeof RUNTIME_DIAGNOSTIC_CONTRACT.error_codes)[number]}`
    >();
  });
});

describe("runtime diagnostic trust boundary", () => {
  it("accepts every generated closed value and reconstructs safe fields", () => {
    for (const code of RUNTIME_DIAGNOSTIC_CONTRACT.error_codes) {
      expect(parseRuntimeDiagnostic(runtimeDiagnostic(code))?.kind).toBe(
        "runtime",
      );
    }
    for (const recovery_action of RUNTIME_DIAGNOSTIC_CONTRACT.recovery_actions) {
      expect(
        parseRuntimeDiagnostic({
          ...runtimeDiagnostic(),
          detail: { ...runtimeDiagnostic().detail, recovery_action },
        }),
      ).not.toBeNull();
    }
    for (const operation of RUNTIME_DIAGNOSTIC_CONTRACT.operations) {
      expect(
        parseRuntimeDiagnostic({
          ...runtimeDiagnostic(),
          detail: {
            ...runtimeDiagnostic().detail,
            context: { operation },
          },
        }),
      ).not.toBeNull();
    }
    for (const transport of RUNTIME_DIAGNOSTIC_CONTRACT.transports) {
      expect(
        parseRuntimeDiagnostic({
          ...runtimeDiagnostic(),
          detail: {
            ...runtimeDiagnostic().detail,
            context: { operation: "provider_request", transport },
          },
        }),
      ).not.toBeNull();
    }
    for (const status_class of RUNTIME_DIAGNOSTIC_CONTRACT.status_classes) {
      expect(
        parseRuntimeDiagnostic({
          ...runtimeDiagnostic(),
          detail: {
            ...runtimeDiagnostic().detail,
            context: { operation: "provider_request", status_class },
          },
        }),
      ).not.toBeNull();
    }
    for (const retry_delay of RUNTIME_DIAGNOSTIC_CONTRACT.retry_delay_buckets) {
      expect(
        parseRuntimeDiagnostic({
          ...runtimeDiagnostic(),
          detail: {
            ...runtimeDiagnostic().detail,
            context: { operation: "provider_request", retry_delay },
          },
        }),
      ).not.toBeNull();
    }
  });

  it("accepts each credential code while enforcing canonical set identifiers", () => {
    for (const code of CREDENTIAL_CONTRACT.vocabulary.error_codes) {
      expect(
        parseRuntimeDiagnostic({
          kind: "credential",
          detail: {
            code,
            retryable: false,
            recovery_action: "none",
            set_id: "aws",
          },
        }),
      ).toEqual({
        kind: "credential",
        detail: {
          code,
          retryable: false,
          recovery_action: "none",
          set_id: "aws",
        },
      });
    }

    expect(
      parseRuntimeDiagnostic({
        kind: "credential",
        detail: {
          code: "missing",
          retryable: false,
          recovery_action: "reenter_credential",
          set_id: "custom.123e4567-e89b-12d3-a456-426614174000",
        },
      }),
    ).not.toBeNull();
    expect(
      parseRuntimeDiagnostic({
        kind: "credential",
        detail: {
          code: "missing",
          retryable: false,
          recovery_action: "reenter_credential",
          set_id: "custom./home/private-user/provider.json",
        },
      }),
    ).toBeNull();
  });

  it("drops extra secret, body, path, request-id, and length fields", () => {
    const [secret, prose, body, path, requestId, exactLength] = CANARIES;
    const parsed = parseRuntimeDiagnosticAppError({
      code: "runtime_diagnostic",
      message: prose,
      diagnostic: {
        ...runtimeDiagnostic(),
        secret,
        detail: {
          ...runtimeDiagnostic().detail,
          body,
          request_id: requestId,
          message_length: exactLength,
          context: {
            ...runtimeDiagnostic().detail.context,
            private_path: path,
          },
        },
      },
    });

    expect(parsed).not.toBeNull();
    const serialized = JSON.stringify(parsed);
    for (const canary of CANARIES) expect(serialized).not.toContain(canary);
    expect(serialized).not.toContain("message");
    expect(serialized).not.toContain("request_id");
  });

  it("maps every malformed or unknown payload to one fixed internal tuple", () => {
    const malformed = [
      null,
      "provider prose canary",
      { code: "runtime_diagnostic", message: CANARIES[0] },
      { ...runtimeDiagnostic("unknown_provider_code"), body: CANARIES[2] },
      {
        ...runtimeDiagnostic(),
        detail: {
          ...runtimeDiagnostic().detail,
          context: { operation: "/private/path/canary" },
        },
      },
    ];

    for (const value of malformed) {
      expect(normalizeRuntimeDiagnostic(value)).toBe(
        SAFE_INTERNAL_RUNTIME_DIAGNOSTIC,
      );
      expect(JSON.stringify(normalizeRuntimeDiagnostic(value))).not.toContain(
        "canary",
      );
    }
  });

  it("returns only closed localization and recovery metadata", () => {
    const presentation = runtimeDiagnosticPresentation({
      code: "runtime_diagnostic",
      diagnostic: runtimeDiagnostic("rate_limited"),
      message: CANARIES[1],
    });

    expect(presentation.translationKey).toBe(
      "runtimeDiagnostics.runtime.rate_limited",
    );
    expect(presentation).toMatchObject({
      retryable: true,
      recoveryAction: "retry",
      operation: "transcription",
    });
    for (const canary of CANARIES) {
      expect(JSON.stringify(presentation)).not.toContain(canary);
    }
  });
});
